# SIMLUX — Standalone 3D Solid Sandbox Plan

**Goal:** prototype the interactive 3D solid modeler — **movable drawing plane** +
**boolean (CSG) operations** — as a **standalone `cad_solid` sandbox**, isolated from
the main app, and **wire it into simLUX later** once the model is proven.

Status: **PLAN — awaiting go/no-go.** Author: coding agent, 2026-07-11.

**Confirmed decisions (2026-07-11):**
1. Build as a standalone basic UI first, wire later. *(user proposed)*
2. Boolean engine → **csgrs** (pure-Rust, MIT BSP CSG). (§4)
3. CSG model → **parametric tree**, kept editable (as an ordered feature history). (§3)

**Consequence for the earlier plan:** the RSM-v9 persistence decision in
`SIMLUX_3D_SOLIDS_PLAN.md` is **deferred** — we don't freeze a file format while
booleans are still reshaping the data model. Sandbox → settle the model → then persist.
The Step/Ramp/Wall/Column primitives from that plan are **not wasted**: they become
the **leaf primitives** of the CSG tree here.

---

## 1. Why standalone first

- Movable plane + booleans = a mini solid modeler. The interaction must be *felt and
  iterated*, not designed on paper.
- Prototyping in isolation touches **nothing** in `cad_kernel` / `cad_io` / calc /
  the RSM format — zero risk to the shipped LUX work.
- Wire-in later is cheap: the sandbox's only export is `Vec<Mesh>`, which is already
  the single thing both the renderer (`light3d.rs::build_scene_verts`) and the lux
  calc (`cad_light::calculate`) consume. Emit meshes → drop into the scene.

---

## 2. Crate + shape

New workspace member **`cad_solid`** (the "free 3D / Path B" from the roadmap),
self-contained:

- `cad_solid/src/lib.rs` — model + evaluation (pure, no UI).
- `cad_solid/examples/sandbox.rs` — a standalone eframe window
  (`cargo run -p cad_solid --example sandbox`) with its own trimmed copy of the
  `light3d.rs` FBO renderer. **No panel/command-line/persistence** — just orbit,
  workplane, create, boolean.
- Deps: `eframe`/`glow`/`glam` (viewer), `csgrs` (booleans), `serde` (model, for the
  eventual wire-in). Verify csgrs pulls **nalgebra** — we use glam, so there's a
  glam↔nalgebra conversion **at the csgrs boundary only** (keep it contained in one
  `csg.rs` adapter; the rest of cad_solid stays glam).

---

## 3. Data model (`cad_solid/src/lib.rs`)

Parametric CSG kept as an **ordered feature history** (the usable, editable form of a
CSG tree — easy to add / reorder / delete / re-evaluate):

```rust
/// A construction plane: local (u,v) frame in world space.
pub struct Plane { pub origin: Vec3, pub normal: Vec3, pub x_axis: Vec3 }

/// Pose of a primitive on the active plane (local u,v,elevation + spin).
pub struct Placement { pub u: f32, pub v: f32, pub lift: f32, pub spin_deg: f32 }

/// Leaf shapes. Step/Ramp/Wall/Column carried over from SIMLUX_3D_SOLIDS_PLAN §3.
pub enum Primitive {
    Box { w: f32, d: f32, h: f32 },
    Cylinder { r: f32, h: f32, sides: u32 },
    Prism { profile: Vec<[f32; 2]>, h: f32 },   // arbitrary drawn footprint
    Step { width: f32, tread: f32, riser: f32, count: u32 },
    Ramp { width: f32, run: f32, rise: f32 },
    Wall { length: f32, thickness: f32, height: f32 },   // baseline authored in UI
    Column { profile: ColumnProfile, height: f32 },
}

pub enum BoolOp { Union, Difference, Intersection }

/// One history step: apply `primitive` (posed on `plane`) to the running result.
/// The FIRST feature is the base (op ignored / Union with empty).
pub struct Feature {
    pub id: u32,
    pub op: BoolOp,
    pub plane: Plane,
    pub placement: Placement,
    pub primitive: Primitive,
}

pub struct Model { pub features: Vec<Feature> }

impl Model {
    /// Fold the history left→right through csgrs → one evaluated mesh.
    /// Re-run on ANY param edit (the "generator re-derives" contract).
    pub fn eval(&self) -> Mesh;     // cad_solid's own Mesh (pos + index soup)
}
```

Editing any feature's params, or reordering/deleting features, and calling `eval()`
again reproduces the solid — fully parametric.

---

## 4. csgrs integration (`cad_solid/src/csg.rs`)

- **First task of Slice 1:** `cargo add csgrs`, pin the version, confirm the API and
  license (MIT/Apache, pure-Rust), and confirm the nalgebra dependency. Everything
  below assumes the current csgrs `CSG` API (`union` / `difference` / `intersection`
  + primitive constructors); adjust to the real signatures.
- **Primitive → csgrs solid:** map each `Primitive` to csgrs (cube/cylinder for
  Box/Cylinder; extrude the `Prism`/`Column`/`Wall`/`Step`/`Ramp` footprints). Apply
  `Placement` on the active `Plane` as an affine transform.
- **Fold:** `Union`→`.union`, `Difference`→`.difference`, `Intersection`→`.intersection`.
- **Extract:** csgrs polygons → triangulate (reuse the ear-clip in
  `cad_light::extrude::triangulate` pattern) → cad_solid `Mesh`.
- Keep **all** glam↔nalgebra conversion inside this one file.

---

## 5. Workplane system

Active plane the user draws/creates on:

- Presets **XY / XZ / YZ** + a numeric **offset** along the plane normal.
- **3-point** constructor for arbitrary planes.
- Later: **pick a face** of an existing solid → its plane.
- The viewport draws the active plane as a **grid** so it's visible; new primitives'
  `Placement` is expressed in that plane's `(u,v,lift)` frame.

---

## 6. Sandbox UI (`examples/sandbox.rs`)

- **Left panel:** workplane picker (XY/XZ/YZ + offset, 3-point) · primitive buttons
  (Box / Cylinder / Prism / Step / Ramp / Wall / Column) · next-op selector
  (Union / Difference / Intersection) · **feature list** (params editable, delete,
  reorder) · tri/feature count readout.
- **Center:** 3D viewport — orbit/zoom (reuse the FBO renderer), draws the evaluated
  mesh + active-plane grid + (later) a move gizmo.

---

## 7. Slices

| Slice | Scope | Verify |
|-------|-------|--------|
| **S1** | `cad_solid` crate; verify+pin csgrs; standalone window + orbit camera; active-plane picker (XY/XZ/YZ + offset) with a visible grid; create a **Box**; one **Box − Box** boolean via csgrs; render the result. Proves the whole pipeline end-to-end. | `cargo run -p cad_solid --example sandbox` |
| **S2** | Full primitive set (Cylinder/Prism/Step/Ramp/Wall/Column) + 3-point plane + draw a `Prism` footprint on the active plane. | manual |
| **S3** | Feature-list **edit UI**: change any param, reorder, delete → live re-`eval()` (parametric proof). | manual edit |
| **S4** | 3D ray-pick a feature + move gizmo (in-plane u/v + normal lift) — merges "Interactive-3D slice 2". | manual drag |
| **S5** | **Wire-in:** `Model::eval() -> Vec<cad_light::Mesh>` converter; drop into the simLUX scene + calc; decide persistence (RSM v9 of the feature history vs sidecar) **now that the model is settled**. | in-app |

Branch: `simlux-3d-sandbox` (independent of the LUX-block lineage; touches no core).

---

## 8. Wire-in contract (S5, not before)

The sandbox's sole coupling to simLUX is one function:

```rust
// in cad_app, which depends on both crates:
let solid_meshes: Vec<cad_light::Mesh> = model.eval_meshes();
scene_meshes.extend(solid_meshes);   // renders + occludes/receives light, free
```

Persistence is deliberately the **last** step, decided once booleans have finished
shaping the `Model`. Until then the sandbox can save/load its own `Model` as JSON for
testing, touching no shared format.

---

## 9. Test plan

- `cargo run -p cad_solid --example sandbox` — the interactive proof (S1–S4).
- `cargo test -p cad_solid` — `eval()` unit tests: Box−Box hole tri-count/bbox,
  Union volume sanity, param-edit determinism (same model → same mesh).
- (S5) a demo placing a csg solid between a luminaire and the plane to confirm the
  shadow lands in the lux grid.

---

## 10. Relationship to the other plan

`SIMLUX_3D_SOLIDS_PLAN.md` is now **subsumed**: its Step/Ramp/Wall/Column become
leaf primitives here (§3), and its RSM-v9 persistence is **deferred to S5**. Keep
that doc for the primitive param definitions; this doc supersedes its
integration/persistence sequencing.
