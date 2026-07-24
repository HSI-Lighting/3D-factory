# Handoff — continuing the 3D work in `3D_Factory`

**From:** the previous 3D coding agent
**Date:** 2026-07-23
**Read this if:** you are picking up **3D development** in `simLUX / 3D_Factory`.

> Read §3 (THE RULES) before you write a single line. Most of the ways to get this repo
> wrong are not technical — they are rules the owner has already settled, sometimes after
> correcting a previous agent. Violating one costs a revert, not a review comment.

---

## 0. TL;DR — where to start

Two tracks are live. **Track A is the immediate work; Track B is the strategic goal.**

| | Track | State | Start at |
|---|---|---|---|
| **A** | **Alive walls** — 2D→3D promoted walls you can reshape in 3D | engine done + tested; **viewport UI owed** | §6, slice 2 |
| **B** | **The mesh seam** — Factory solids → the light calc | designed, 10 moves specced; **not started** | §7 |

If you take exactly one thing: **Track A slice 2** — render + pick draggable vertex handles.
The editing engine underneath it (`wall_move_vertex` / `wall_insert_vertex` /
`wall_delete_vertex`) is already written and unit-tested; only the viewport wiring is missing.

---

## 1. What this repo is

`3D_Factory` is a **full copy of the simLUX 2D CAD app** with a 3D "Factory" built *inside*
it. It is **not** a separate 3D program, and that is the entire point.

**The core trick** (verbatim from `cad_app/src/factory.rs`, `SketchSession`):

> While a sketch session is live, the app's active `doc` **IS** the sketch's `Document`. Every
> 2D tool in `cad_app` only ever knows `self.doc` — so draw, fillet, trim, extend, offset,
> chamfer, break, the command line, snaps and layers ALL operate on the plane, **unchanged and
> complete**, with nothing reimplemented. That is the whole thesis of this fork.

Internalise this. When you need a 2D capability in 3D, **you reuse it — you never rebuild it.**

**Repo facts**

| | |
|---|---|
| Git root | `~/workspace/simLUX/3D_Factory` (the **subdirectory** is the repo, not `simLUX/`) |
| Remote | `git@github.com:dokkandar/simLUX.git` |
| Branch | local `master` **tracks `origin/3d-factory`** |
| Push with | `git push origin HEAD:3d-factory` (names differ, so a bare `git push` may refuse) |
| Last pushed | `2ac90d0` — footprint-driven alive walls |
| Uncommitted | ViewCube face labels + nav-gizmo cleanup (see §5) |

---

## 2. Build, run, test

```bash
cd ~/workspace/simLUX/3D_Factory
cargo run -p cad_app          # binary is named `simlux`
cargo test -p cad_app         # 72 pass, 7 ignored — keep it green
```

There is **no `cad_app/src/lib.rs`** — `cad_app` is `[[bin]]`-only (43.8k lines). Consequences:

- Nothing outside `cad_app` can `use` its types.
- All its tests are **in-file `#[cfg(test)] mod`s**. Follow that pattern; don't add a
  `tests/` dir for `cad_app`.
- The owner has flagged bin-only as a real architectural problem (unreachable code). **Do not
  "fix" it by adding a lib.rs** without asking — it is a known, deliberate open item.

---

## 3. THE RULES — non-negotiable

These are owner decisions, several of them made *after* correcting an agent. They override
your judgement.

1. **`cad_kernel` is BYTE-IDENTICAL to `RUST_CAD`. Do not touch it.** It is kept identical so
   upstream merges stay clean. If a feature seems to need a kernel change, it almost certainly
   needs an **app-layer** solution instead. (Precedent: the wall centerline linetype was asked
   for with "no need to touch kernel" and was implemented entirely app-side.)

2. **`cad_solid` changes need owner sign-off.** House practice: write the spec into
   `mentor MD/` first and get a decision. Do not open with Rust.

3. **FULL 2D on every plane is non-negotiable. Never reimplement a 2D feature in 3D.** If you
   catch yourself writing a 3D point editor, a 3D snap, or a 3D trim — stop. Route through the
   existing 2D machinery (§1).

4. **MOVE is MOVE.** Never invent a parallel or `3d`-prefixed command (`3dmove`, `3dwall`, …).
   An established verb is ONE command that dispatches on the active view / object type at
   **apply** time. The owner rejected a `3dWall` command for exactly this reason; the wall
   journey became *draft in 2D → select → right-click → Make 3D wall*.
   There is a test module literally called `established_commands_are_untouchable`. Respect it.

5. **New features go in independent modules.** Don't change core ABI / loader / renderer.

6. **No commit and no push unless the owner asks.** When asked: two commits is the house
   pattern (docs separate from code), and every commit ends with the `Co-Authored-By` trailer.

7. **Push to `origin` (dokkandar) only.** Never to any HSI upstream.

8. **Practical journeys beat clever ones.** The owner has twice rejected a working feature
   with "the concept is ok but the way to use it is not practical." Before building an
   interaction, state the gesture plan and get it confirmed.

---

## 4. Architecture map

| Crate | Lines | Role | Touchable? |
|---|---|---|---|
| `cad_app` | 43,823 | the app: 2D + 3D UI, `factory.rs`, `app.rs` | ✅ **yes — your main workspace** |
| `cad_kernel` | 12,859 | geometry core (`Document`, `Geom`, `Vec2` f64) | ❌ **byte-identical — never** |
| `cad_solid` | 3,397 | 3D solids: `Model` / `Feature` / `Primitive`, csgrs CSG | ⚠️ owner sign-off (spec in `mentor MD/`) |
| `cad_light` | 1,143 | the lighting engine: `Mesh`, `Material`, `calculate()` | ✅ yes |
| `cad_io` | 2,515 | DXF import/export | ✅ yes |
| `cad_wall`, `cad_snap`, `cad_nurbs`, `cad_param`, `cad_raster` | small | feature crates | ✅ yes |

Key files in `cad_app/src/`:

- **`factory.rs`** — the 3D Factory: `FactoryState`, walls, views, zoom, picking. **Start here.**
- **`app.rs`** — the app shell (43k lines of it); 3D viewport handling, nav gizmo, promotion.
- **`light3d.rs`** — `mvp(yaw, pitch, dist, target, aspect, ortho)`, the **single** projection
  source for both scene render and picking. Change it and both follow.
- **`dbg_recorder.rs`** — the session recorder (§9).

---

## 5. The 3D Factory today — what works

- **Sketch-on-plane** — the `doc`-swap trick (§1); all 2D tools work on the plane.
- **Primitives** — Box / Cylinder / Sphere / Frustum / Torus / Capsule / Tube / Ellipsoid, via
  `Draw3dDialog` (LWH sliders; live editor re-edits a selected solid's dimensions).
- **Point placement** — pick an initial point *before* Create (`place_pending` →
  `place_primitive`; Box places by corner, others by centre).
- **ViewCube nav gizmo** — a floating circle top-right. Click a **labelled** face
  (TOP/BOTTOM/FRONT/BACK/LEFT/RIGHT) to snap, drag to orbit, double-click for isometric.
  Standard views are **orthographic** (`factory.ortho`) — a perspective Top view wrongly showed
  a cylinder's barrel, so views set ortho and free orbit resets to perspective.
  *(Face labels + the removal of the old button row are the uncommitted changes.)*
- **Zoom at 2D parity** — `z` command: window (drag a box **or** click two corners), extents,
  previous, scale; wheel dollies. The rubber-band preview must be painted on a **Foreground
  layer** or the opaque 3D texture hides it (§10).
- **2D→3D wall promotion** — draft with the real 2D wall tool → select → right-click →
  **Make 3D wall**. Each selected `Geom::Wall` becomes **one alive wall**.
- **Alive walls** — see §6.

**The API you will actually need** (`factory.rs`):

| Function | Use |
|---|---|
| `cursor_on_plane(cursor, rect, &mvp) -> Option<Vec3>` | **screen click → ground-plane point.** Your add/move-vertex workhorse |
| `snap_vertex(...)` | nearest solid mesh vertex under the cursor — handle snapping |
| `pick_feature(cursor, rect, &mvp) -> Option<u32>` | which solid is under the cursor |
| `pick_face(cursor, rect, &mvp) -> Option<Frame>` | which face is under the cursor |
| `overlay_lines() -> Vec<V3>` | overlay geometry the renderer draws |
| `recompute()` | re-eval the CSG tree — **only when idle**, csgrs walks a BSP per boolean |
| `fit()` | zoom-extents |

---

## 6. TRACK A — alive walls (the immediate work)

### The model, and the invariant that matters

A wall is an **extrusion of ONE ground-plane footprint**:

```rust
pub struct WallInst {
    pub footprint: Vec<Vec2>,   // ≥2 pts, glam f32 — shared by BOTH rings
    pub segments: Vec<u32>,     // one Box feature per edge (footprint.len()-1)
    pub thickness: f32, pub height: f32, pub rake_deg: f32,
}
```

**The invariant (owner requirement, do not break it):** the floor ring (`z=0`) and the ceiling
ring (`z=height`) are derived from the **same** footprint points. So a vertex is a full-height
**vertical edge present on both rings by construction** — they can never drift apart.

This is why *"add a vertex in Top view and it lands on top AND bottom"* is automatic rather
than a special case: there is only one set of points driving both rings. **If you ever give the
top and bottom independent vertex lists, you have broken the feature.**

(The single exception is a **raked** wall, where top ≠ bottom — that is blocked, see §8.)

### The engine — already written and tested

| Method | Does |
|---|---|
| `add_wall(footprint, thickness, height) -> Option<usize>` | promote a footprint; one Box per edge |
| `wall_move_vertex(wi, vi, to)` | move a corner → re-derive (**shifts the surface**) |
| `wall_insert_vertex(wi, seg, at) -> Option<usize>` | **add a corner at a desired (x,y)**, splitting edge `seg` |
| `wall_delete_vertex(wi, vi) -> bool` | remove a corner; never drops below 2 points |
| `set_wall_height(fid, h)` | re-derives **in place** — feature ids stay stable, so a selection survives |
| `wall_index(feature_id) -> Option<usize>` | which wall owns this feature |

Note the deliberate asymmetry: **height** edits in place (stable ids); **shape** edits call
`rederive_wall`, which drops and rebuilds the Boxes, so **segment feature ids change**. Any
selection you track across a shape edit must be refreshed.

Covered by `factory::wall_tests::footprint_wall_add_vertex_couples_rings_and_reshapes`.

### What is owed — slices 2–5 (the viewport)

The owner chose **"3D handles now"**: edit the footprint directly in the 3D view.

| Slice | Task | Notes |
|---|---|---|
| **2** | Render + pick draggable dots at each footprint vertex | project with `light3d::mvp`; hit-test in screen space |
| **3** | Drag a dot → `wall_move_vertex` | snap via `snap_vertex` / grid |
| **4** | Click a footprint **edge** → `wall_insert_vertex` at the cursor `(x,y)` | use `cursor_on_plane`; both rings automatic |
| **5** | Delete gesture → `wall_delete_vertex` | right-click a dot, or select + Delete |

**Confirm the four gestures with the owner before building them** (rule 8). The proposed set:
drag = move, click-edge = add, right-click-dot = delete, and handles only shown for a selected
wall.

---

## 7. TRACK B — the mesh seam (the strategic goal)

**The target:** match DIALux's actual deliverable — a **watertight tessellated room mesh with
per-surface reflectance**, handed to the light calc.

This was researched, not assumed: DIALux evo (OpenGL 3.2 / .NET / in-house **mesh-facet**
engine / photon shooting), Relux (radiosity + Radiance) and AGi32 (radiosity) **all feed the
calc tessellated meshes + reflectance, never B-rep**. Conclusion: the csgrs/mesh path here is
correct — **do not chase a B-rep kernel.** The differentiator is the **calc**, not the modeller,
so keep the modeller *adequate* and invest in `cad_light`.
→ `mentor MD/LIGHTING_3D_STACK_RESEARCH_2026-07-22.md`

**The whole of steps 1–2 is one transformation across one seam:**

```
cad_solid::SolidMesh              →   Vec<cad_light::Mesh>
{ positions, normals }                { vertices, triangles, material }
flat soup · no material · no grouping  indexed · ONE material per surface
```

`cad_solid/src/lib.rs` already names this as the planned *"single coupling point"* — **it is
not built yet.** The target end exists as a reference: `cad_light::extrude::box_room()` hands
the calc 6 meshes (floor/wall/ceiling), and the entry is
`cad_light::calculate(meshes: &[Mesh], luminaires, profiles, materials, plane, settings) -> LuxGrid`.

The decomposition into **10 concrete moves** (weld soup → per-tri normal → coplanar faces →
SurfaceId → pick-highlight → Surface→Mesh → assemble → wire calc → dirty gate → box-occludes
proof) is written up, with the real types, in:

- `mentor MD/MESH_HANDLING_STEP1_2.html` ← **open this in a browser; it is the plan**
- `mentor MD/ROADMAP_3D_TO_MESH_2026-07-22.md` ← the 10-step roadmap to the target

Useful head start: `cad_solid::coplanar_face(positions, start) -> Vec<usize>` already exists
and does the adjacency walk that groups triangles into faces.

**Why the owner's instinct is right that this unblocks everything:** roadmap steps 3–10 each
merely *consume* an artifact of these two steps (classify = reuse the per-face normal;
materials = map class → `MaterialId`, and `default_materials()` already ships 0.20/0.50/0.70;
watertight-check = validate the weld). Get the mesh right and the rest is consumption.

---

## 8. Blocked / owner decisions — do not start these

| Item | Blocked on |
|---|---|
| **Wall rake / rise-angle** | `cad_solid::Feature` is **axis-aligned only**; needs a tilt DOF (arbitrary `Frame`). `rake_deg` is stored but **not applied**. |
| **Standalone boolean command** | Owner ruled boolean is an *independent* function (select both solids → operate), **not** part of creation. Needs a cad_solid multi-body decision. → `BOOLEAN_AS_COMMAND_2026-07-17.md` |
| **Openings (doors/windows)** | Depends on the boolean decision. Until then, model openings as separate solids. |
| **bbox-cache landing** | Reverted optimisation, plus a `HANDLE_COUNTER` fix. → `TODO_BBOX_CACHE_2026-07-17.md` |

---

## 9. Debugging — use the session recorder FIRST

`cad_app/src/dbg_recorder.rs` records a decoded event stream (commands, picks, zoom ops with
before/after state, slow frames). **For any interaction bug, read the dump before theorising.**
This is house practice and it has repeatedly found the real cause faster than reading code.

Known blind spot: pick-phase states are not polled, so they can be invisible in a dump. If the
data you need isn't there, **add the event** — that is exactly what was done for zoom
(`DbgEvent::ZoomOp { cmd, choices, action, before, after }`) when a zoom bug couldn't be seen.

---

## 10. Gotchas that already cost time

1. **Don't gate 3D behaviour on `factory.session.is_none()`.** A gate like that silently
   forced `move` down the dead 2D path whenever a sketch was open. Dispatch on
   `active_view == ActiveView::ThreeD` alone.
2. **Overlays must be painted on a Foreground layer.** The 3D scene is an opaque texture; a
   rubber-band drawn in the normal layer is invisible under it.
   Use `ctx().layer_painter(LayerId::new(Order::Foreground, …))`.
3. **Standard views must be orthographic.** In perspective, a Top view shows a cylinder's
   barrel. `set_view()` sets `ortho = true`; free orbit resets it to `false`.
4. **`recompute()` is expensive** — csgrs walks a BSP per boolean. Gate it behind `dirty`;
   never call it per frame.
5. **Two different `Vec2` types.** `cad_app`/`factory.rs` use **glam f32**; `cad_kernel` uses
   its own **f64** `Vec2`. Promotion casts explicitly (`p.x as f32`). Mixing them silently
   compiles in the wrong places — check which one is in scope.
6. **Feature ids are stable keys, not indices.** `Model::remove(id)` does not renumber others.
   But a `rederive_wall` (remove + push) *does* mint new ids for that wall.
7. **`#[derive(Copy)]` breaks the moment a struct gains a `Vec`** — `WallInst` had to drop
   `Copy` when it gained `footprint`.

---

## 11. Read these, in this order

1. **This document.**
2. `mentor MD/BASIC_MODIFIERS_RULES.md` — **the canonical modifier spec.** Read it in full;
   don't re-derive the rules. The workflow (`command → select-if-empty → Enter → operate`)
   must match the 2D app *exactly*.
3. `mentor MD/MESH_HANDLING_STEP1_2.html` — the Track B plan (open in a browser).
4. `mentor MD/ROADMAP_3D_TO_MESH_2026-07-22.md` — the 10-step target.
5. `mentor MD/FACTORY_IS_THE_ROOM_2026-07-17.md` — why the Factory *is* the room the calc lights.
6. `mentor MD/LIGHTING_3D_STACK_RESEARCH_2026-07-22.md` — why this architecture is right.
7. `mentor MD/VENUE_DECISION_2D_ON_EVERY_PLANE.md` + `2D_DRAFTING_PARITY.md` — the 2D-everywhere rule.
8. `mentor MD/DAY_REPORT_2026-07-17.md` + `STATE_2026-07-15.md` — recent history.

---

## 12. First session, suggested

1. Build and run; click the ViewCube faces, promote a 2D wall to 3D, edit its height.
2. Read `factory.rs` end to end (~1k lines) — it is the whole 3D surface.
3. Run `cargo test -p cad_app`; read `factory::wall_tests` to see the footprint invariant asserted.
4. Confirm the slice-2 gesture plan with the owner (§6), then build the vertex handles.

Welcome aboard. Keep the tests green, keep 2D untouched, and ask before inventing a command.

```
cd ~/workspace/simLUX/3D_Factory && cargo run -p cad_app
```
