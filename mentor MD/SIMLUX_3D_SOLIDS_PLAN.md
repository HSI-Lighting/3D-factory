# SIMLUX — 3D Solid Objects Plan

**Goal:** author real 3D massing solids **directly in the 3D engine** — *steps, ramps, walls, columns* — as parametric objects, **separate from 2D-footprint extrusion**.

Status: **PLAN — awaiting review** (user chose "review first"; no code written yet).
Author: coding agent, 2026-07-11.

**Confirmed decisions (2026-07-11):**
1. Persistence → **in the drawing (RSM v9)**, not sidecar. (§4)
2. Wall authoring → **draw a baseline, two picks in plan**. (§5)
3. Step shape → nested solid mass (stand-on-able). (§3)

---

## 1. Why these are NOT 2D extrusion

`cad_light/src/extrude.rs` takes a 2D `Document` entity and lifts it to a height:
one drafted line → one **open vertical quad** (no top/bottom, no thickness). That is
correct for "walls-as-drawn-lines" room shells, but it cannot express:

| Object  | 3D-only parameters that a lifted 2D polyline can't carry |
|---------|----------------------------------------------------------|
| Step    | tread depth, riser height, **step count**, run direction |
| Ramp    | slope / rise-over-run (an inclined top face)             |
| Wall    | **thickness** (a real box, two faces + top + ends)       |
| Column  | base elevation + height + **profile** (rect/round/poly)  |

So these are a new family: **parametric watertight solids**, placed and sized in 3D,
each of which *derives* a `Vec<Mesh>`. They coexist with 2D extrusion — the room
shell can still come from lifted layers; solids are added architecture/furniture.

> Note vs RUST_CAD: RUST_CAD has a separate *2D* "wall command" (junction cleanup,
> justification, openings — its own module). This is a **different, simpler thing**:
> a simLUX **3D massing solid** whose only job is to render and to occlude/receive
> light. Don't conflate the two. (memory `rust_cad_wall_own_module`.)

---

## 2. Where the code lives  (split by the RSM-v9 decision)

Because the solids persist **inside the `.rsm`** (§4), they must live in the
in-memory `Document`, and `Document` is `cad_kernel`. So the code splits along the
existing kernel/feature-crate seam — the same way `Geom::Wall`/`Line`/`Circle` keep
their *params* in `cad_kernel` while `extrude.rs` derives their *meshes* in `cad_light`:

- **`cad_kernel` — the parameter data** (`solid3d.rs`): `Solid3d`, `Placement`,
  `ColumnProfile`, `WallAlign` (plain serde data, no mesh math). Held on the
  Document as a **new, separate** `pub solids3d: Vec<Solid3d>` — **not** a `Geom`
  variant, so 2D select / bbox / extrude never touch them.
- **`cad_io` — the on-disk section** (`rsm.rs`): bump `VERSION` 8 → 9, write/read a
  `solids3d` block gated `if ver >= 9` (older readers skip it, same pattern as the
  IES v8 block).
- **`cad_light` — the mesh derivation** (`solids.rs`): free fns
  `append_meshes(&Solid3d, &mut Vec<Mesh>)` + `bbox(&Solid3d)`, mirroring
  `extrude::extrude_geom`. `cad_light` already depends on `cad_kernel`, so it reads
  the kernel type and emits `Mesh`es.
- **`cad_app`** — UI panel + `solid …` commands + wiring into calc/render, all
  reading `doc.solids3d`.

**Core-touch note.** This adds a field + type to `cad_kernel` and a section to
`cad_io` — the "untouched core" (D5). That is the direct, accepted consequence of
"keep solids in the drawing," and it's the **second** such step after IES-in-drawing
(RSM v8) already added `Document.ies_files`. Consistent, deliberate; upstream merges
will carry these two additive fields. If a clean-core merge ever matters more than
in-drawing travel, the fallback is the sidecar variant (kept in §10).

### Why the mesh is still the only integration point

`Mesh` remains the narrow waist — both consumers already take `&[Mesh]`:
render `cad_app/src/light3d.rs::build_scene_verts`, calc `cad_light::calculate`.
Emitting `Vec<Mesh>` lights solids up in the viewport **and** the lux engine with
one appended line (§6). No renderer/engine change.

---

## 3. Data model  (defined in `cad_kernel/src/solid3d.rs`)

```rust
/// Position + heading of a solid in the world XY plane, Z-up (metres, degrees).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct Placement {
    pub origin: [f32; 2],     // world XY of the object's local origin
    pub base_z: f32,          // elevation the solid sits on
    pub rotation_deg: f32,    // heading about +Z (CCW, 0 = +X)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnProfile {
    Rect { w: f32, d: f32 },
    Circle { r: f32 },
    Poly { sides: u32, r: f32 },   // regular n-gon
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum WallAlign { #[default] Center, Left, Right }

/// A parametric 3D solid. Params are the identity; the mesh is DERIVED in
/// `cad_light::solids` (generator pattern — edit a param, re-derive).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Solid3d {
    /// Straight flight: `count` NESTED boxes (solid mass you can stand on), each
    /// `tread` deep × `riser` tall × `width` wide.
    Step  { id: u32, place: Placement, width: f32, tread: f32, riser: f32, count: u32 },
    /// Wedge: rectangle `run`×`width` at base, top edge rising `rise` over the run.
    Ramp  { id: u32, place: Placement, width: f32, run: f32, rise: f32 },
    /// Box wall along a baseline, given thickness + height. `align` offsets the
    /// box relative to the centerline (Center / Left / Right of start→end dir).
    Wall  { id: u32, start: [f32; 2], end: [f32; 2], base_z: f32,
            height: f32, thickness: f32, align: WallAlign },
    /// Prism: `profile` swept from base_z up `height`, with top + bottom caps.
    Column{ id: u32, place: Placement, height: f32, profile: ColumnProfile },
}

impl Solid3d { pub fn id(&self) -> u32 { /* match */ } }
```

Each variant carries a stable `id: u32` (identity for pick/edit/delete). Material
starts fixed (a `SOLID` id, §6); per-solid material is Slice F.

### Mesh generation (`cad_light`, all watertight, CCW outward)

```rust
pub fn append_meshes(s: &Solid3d, out: &mut Vec<Mesh>);   // in cad_light::solids
pub fn bbox(s: &Solid3d) -> ([f32;3],[f32;3]);            // for pick + framing
```

- **Column** — profile polygon in local XY (rect / n-gon / circle→48-gon); reuse
  `extrude::triangulate` for the two caps + a side quad per edge. Transform by
  `Placement`.
- **Wall** — offset the `start→end` segment by `±thickness/2` (per `align`) into a
  4-corner rectangle footprint → same prism builder as Column, `base_z..+height`.
  (A wall is a rectangular column along a line.)
- **Step** — `count` nested boxes; box *i* spans `y∈[i·tread, run]`,
  `z∈[base_z+i·riser, base_z+(i+1)·riser]`, `x∈[0,width]`. Transform by `Placement`.
- **Ramp** — 6-face wedge: bottom rect, back rect, two right-triangle sides,
  inclined top rect. Transform by `Placement`.

Write one shared `prism(footprint: &[[f32;2]], z0, z1, mat, out)` — it covers
Column + Wall and most of Step.

---

## 4. Persistence → **RSM v9, in the drawing**  *(chosen)*

`cad_io/src/rsm.rs`: `const VERSION: u16 = 9;` and a new section written after the
IES-files (v8) block, read gated `if ver >= 9`:

```rust
// write (after write_ies_files):
write_solids3d(w, &doc.solids3d)?;              // count + serialized records
// read (after read_ies_files):
let solids3d = if ver >= 9 { read_solids3d(r)? } else { Vec::new() };
// ... Document { …, ies_files, solids3d }
```

Backward-compatible: a v≤8 file → `solids3d = Vec::new()`; a v9 file opened by an
older build → the unknown trailing section is skipped by the version guard, same as
every prior bump. Round-trip test `solids3d_round_trip` in `cad_io` mirrors
`ies_files_round_trip`.

**Consequence (accepted):** solids now travel *with the drawing* — no sidecar sync,
and if `Document` is snapshotted for undo, solids come along free. Cost = the
kernel/io additions in §2.

---

## 5. UI / authoring

A new **"③ 3D Solids"** group in the light panel (after the extrude group), matching
the existing panel style, plus command-line verbs (consistent with `luxmetric` /
`luxblock` / `place`):

```
solid step      solid ramp      solid wall      solid column
```

- **Wall** *(chosen input)* — pick **start point**, pick **end point** in the 2D
  plan (chainable like a polyline); the box derives from that baseline + thickness +
  height. Reuses the existing `InsertState::WaitingForPoint` pick.
- **Step / Ramp / Column** — click one insertion point in the plan for `place`,
  then numeric params (tread/riser/count, run/rise, profile/height) in a small dialog.
- **List** of existing `doc.solids3d` with per-row numeric editors + delete; editing
  any param re-derives live (`rebuild_3d`).

No new input stack — this **extends the existing command line** (memory
`simlux_lux_command_line`).

---

## 6. Render + calc integration

In `light.rs` (both `calculate()` and the live-mesh rebuild), after
`extrude_handles_range(...)`:

```rust
for s in &doc.solids3d { cad_light::solids::append_meshes(s, &mut meshes); }
```

One line → solids in the 3D view **and** the ray-traced calc.

- Material: add a dedicated `SOLID` material id (id 3, mid-grey, reflectance ~0.5) in
  `default_materials()` so `build_scene_verts` colours it and the engine treats it as
  a diffuse occluder/receiver. `light3d.rs` currently culls CEILING (id 2) so you can
  see in — `SOLID` **must be a different id** so solids are never culled.
- Watertight meshes ⇒ shadow rays (`RaySettings.shadows`) get correct occlusion for
  free (columns cast shadows, walls block light).

---

## 7. Interactive-3D convergence (the next queued slice)

These solids are the natural **first pickable/gizmo-editable objects in 3D** (todo
"Interactive-3D slice 2"). A 2D-extruded layer is edited via its 2D source; a
`Solid3d` is edited *directly in 3D*:

- **X/Y drag** → move `place.origin` (or wall endpoints), re-derive.
- **Z drag** → move `base_z`.
- **Type handles** → drag to change `height` / `riser` / `rise` / `count`.

Sequence: build solids first (something to select), then the gizmo lands on top.

---

## 8. Slices (each independently mergeable)

| Slice | Scope | Verify |
|-------|-------|--------|
| **A** | `cad_kernel/src/solid3d.rs` (types + `Document.solids3d` + exports) **and** `cad_light/src/solids.rs` (`append_meshes`/`bbox` for all 4) + unit tests (face/tri counts, bbox, closedness). | `cargo test -p cad_light`, `cargo build -p cad_kernel` |
| **B** | `cad_io/src/rsm.rs` v9 write/read of `solids3d`; Document construction; round-trip test. | `cargo test -p cad_io` |
| **C** | scene wiring in `light.rs` (append solid meshes in calc + rebuild) + a debug "add sample column" so it renders + affects lux; `SOLID` material. | run app; save/reopen keeps it |
| **D** | ③ 3D Solids panel + `solid …` commands; **baseline wall pick**; numeric editors; list/delete; full create→save→reopen via `.rsm`. | manual round-trip |
| **E** | 3D ray-pick + move gizmo (X/Y/Z) + type handles (merges "Interactive-3D slice 2"). | manual drag test |
| **F** | per-solid material (reflectance/colour) into calc + UI; watertight-normal audit for shadow rays. | calc delta w/ vs w/o |

Branch: `simlux-3d-solids` for A–D (stacked on the current `simlux-lux-block`
lineage), `simlux-3d-gizmo` for E.

---

## 9. Test plan (what to run)

- `cargo test -p cad_light` — solids kernel unit tests (Slice A).
- `cargo test -p cad_io` — RSM v9 round-trip (Slice B).
- `cargo run -p cad_app` — author a column/wall/step/ramp, confirm it renders,
  **save, reopen the `.rsm`, confirm it persists** (Slices C–D).
- `cad_light/examples/solids_demo.rs` — print tri counts + bbox per type, and a
  scene with a column between a luminaire and the plane to confirm the shadow shows
  up in the lux grid (Slice F).

---

## 10. Decision log

1. **Persistence** — ✅ **RSM v9, in the drawing** (chosen). Fallback if clean-core
   merges ever outweigh in-drawing travel: a sidecar `SimluxConfig.solids: Vec<…>`
   (type would then live in `cad_light`, kernel untouched).
2. **Wall authoring** — ✅ **draw baseline, two picks in plan** (chosen).
3. **Step shape** — ✅ **nested solid mass** (chosen).
