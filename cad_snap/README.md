# cad_snap

Object-snap engine for 2D CAD — pure Rust, no UI dependencies.

Finds the nearest meaningful point on or near a cursor: endpoints, midpoints,
centres, quadrants, intersections, perpendicular feet, tangent points, or
nearest-on-curve. Same primitives every CAD user already knows from AutoCAD,
LibreCAD, BricsCAD.

## Why use this

- **Drop-in.** One crate, one import surface. Pass a `Vec2` cursor + a slice
  of `DObject`. Get back a `SnapHit` or `None`.
- **Renderer-agnostic.** Returns geometry. You draw — egui, wgpu, vello,
  Tauri canvas, web GL, anything.
- **Scales.** Optional `UniformGrid` spatial index keeps queries bounded at
  million-dobject scale.
- **Tab-cyclable.** `find_all_snaps` returns every viable candidate sorted by
  priority, so your UI can let users cycle past the default with Tab.
- **Extension-aware.** PER/TAN feet on the imaginary extension of an arc or
  line are returned with an extension anchor — your UI draws the dashed
  "deferred perpendicular" cue every CAD user expects.
- **Tested.** 38 unit tests covering the math + the priority rules.

## Quick start

```toml
[dependencies]
cad_snap = "0.1"
```

```rust
use cad_snap::{find_snap, DObject, Line, SnapKind, SnapSet, Vec2};

// `Line { … }.into()` wraps the geometry into a DObject with default style.
let dobjects: Vec<DObject> = vec![
    Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }.into(),
];

let mut snaps = SnapSet::default();
snaps.end = true;
snaps.mid = true;

let hit = find_snap(
    /* cursor   */ Vec2::new(5.1, 0.05),
    /* radius   */ 1.0,
    /* enabled  */ snaps,
    /* forced   */ None,
    /* anchor   */ None,
    /* dobjects */ &dobjects,
    /* grid     */ None,
).unwrap();

assert_eq!(hit.kind, SnapKind::Mid);
assert_eq!(hit.point, Vec2::new(5.0, 0.0));
```

`DObject` is the full drafting object — `geom: Geom` (the geometric shape) +
`style: Style` (layer, color, linetype, lineweight, visibility) + `handle`.
For ad-hoc tests, `From<Line>` / `From<Circle>` / `From<Arc>` / `From<Ellipse>` /
`From<EllipseArc>` impls keep the boilerplate down — see the example above.
Helper functions like `perpendicular_extended` take the inner `&Geom` directly;
when calling them on a Dobject pass `&dobj.geom`.

## The eight snap kinds

| Kind | What it snaps to |
|------|------------------|
| `End` | endpoints of lines, arcs, and elliptical arcs |
| `Mid` | midpoints (line midpoint, parametric midpoint of arc / elliptical arc) |
| `Cen` | centres of circles, arcs, ellipses, and elliptical arcs |
| `Qua` | quadrants of circles & arcs (E/N/W/S of centre) **or** the four axis-end points of an ellipse / elliptical arc (these rotate with the ellipse) |
| `Int` | intersections between two nearby dobjects |
| `Per` | perpendicular foot from a known anchor (needs `from`) |
| `Tan` | tangent point from a known anchor (needs `from`) |
| `Nea` | nearest point on the curve under the cursor |

### Supported dobject types

| DObject | Notes |
|---|---|
| `Line { a, b }` | finite segment |
| `Circle { center, radius }` | full circle |
| `Arc { center, radius, start_angle, sweep_angle }` | partial arc, CCW from `start_angle` |
| `Ellipse { center, major, ratio }` | full ellipse — `major` is the semi-major axis vector (length = a, direction = rotation); `ratio = b/a ∈ (0, 1]` |
| `EllipseArc { ellipse, start_param, sweep_param }` | partial ellipse — parameters are in radians of `t`, NOT geometric angle |

All snap kinds work for every dobject type. PER/TAN on ellipses use a
multi-seed Newton solver under the hood (up to 4 perpendicular feet, up to
2 tangents from an external point); INT goes through the pairwise
intersection dispatcher, which now handles every ellipse pair
(line-ellipse, circle-ellipse, arc-ellipse, ellipse-ellipse, and their
elliptical-arc-filtered counterparts).

Priority when multiple kinds match:

```
END > MID > CEN > QUA > INT > PER > TAN > NEA
```

## Two activation modes

| Kind | Activation |
|------|------------|
| END, MID, QUA, INT | **cursor near the snap point itself** |
| CEN, NEA, PER, TAN | **cursor on the dobject's curve** |

PER and TAN are object-priority because the foot/tangent location is
determined entirely by anchor geometry — the user cannot guess where in
empty space the foot lands, so we activate on the dobject itself.

## Tab cycling

Multiple kinds can match the same cursor. `find_snap` returns the highest-
priority one. To let users override that, call `find_all_snaps` and offer
Tab-cycling in your UI:

```rust
use cad_snap::{find_all_snaps, SnapSet, Vec2};
# let dobjects: Vec<cad_snap::DObject> = Vec::new();
# let mut enabled = SnapSet::default();
# enabled.cen = true; enabled.nea = true;

let mut cycle_index = 0_usize;
// each frame:
let candidates = find_all_snaps(
    Vec2::new(3.5, 3.5), 1.0,
    enabled, None, None, &dobjects, None,
);
let active = candidates.get(cycle_index).copied();
// on Tab keypress:
if !candidates.is_empty() {
    cycle_index = (cycle_index + 1) % candidates.len();
}
```

## Imaginary extension cue

When PER's foot falls past a segment endpoint, or outside an arc's swept
range, the engine still returns the geometric foot — but `extension_anchor`
is set to the on-dobject point where you should anchor the dashed extension
indicator:

```rust
# use cad_snap::{find_snap, DObject, Line, SnapKind, SnapSet, Vec2};
# let dobjects: Vec<DObject> = vec![
#     Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) }.into(),
# ];
# let mut snaps = SnapSet::default();
# snaps.per = true;
let hit = find_snap(
    Vec2::new(15.1, 0.1), 1.0, snaps,
    None, Some(Vec2::new(15.0, 5.0)),       // anchor (first click of line)
    &dobjects, None,
).unwrap();

assert_eq!(hit.point, Vec2::new(15.0, 0.0));
assert!(hit.extension_anchor.is_some());     // → draw dashed line from anchor to point
```

For arcs the dashed extension follows the underlying circle's curvature —
your UI draws it as an arc, not a straight chord.

## Scaling with a spatial index

For drawings with > ~1000 dobjects, supply a `UniformGrid` so the candidate
set scales O(visible cells) instead of O(N):

```rust
use cad_snap::{UniformGrid, find_snap, SnapSet, Vec2};
# let dobjects: Vec<cad_snap::DObject> = Vec::new();
# let snaps = SnapSet::default();

let cell_size = UniformGrid::auto_cell_size(&dobjects, 10.0);
let grid = UniformGrid::build(&dobjects, cell_size);

let _hit = find_snap(
    Vec2::ZERO, 1.0, snaps, None, None,
    &dobjects, Some(&grid),
);
```

Rebuild the grid after each edit (insert/delete/move).

## Verifying it on your own data

Run the example:

```
cargo run --example basic -p cad_snap
```

Or write your own — the public API surface is small: see `lib.rs`.

## License

MIT OR Apache-2.0
