# 3D Library Review — simLUX `cad_solid` stack

**Reviewer:** 3d mentor · **Date:** 2026-07-14 · **Scope:** every 3D-relevant library the
`cad_solid` sandbox pulls in, what it can do, how far it carries the roadmap, and its cost.
Versions are the **resolved `cargo tree -p cad_solid`** values (not the workspace-wide lock,
which mixes in sibling crates like `cad_app`).

---

## 0. TL;DR

| Question | Answer |
|---|---|
| What is the 3D solid kernel? | **csgrs 0.21.0** (git-pinned) — a **mesh/BSP CSG** library. Not B-rep. |
| How far does it reach? | **Far past current use.** Already ships extrude/revolve/sweep/loft/offset/minkowski/convex-hull + ~18 primitives + SDF/metaballs/smoothing. The sandbox uses **~5%** of it (cuboid, cylinder, 3 booleans, transform). |
| Hard limits? | **No fillet / chamfer / shell.** Triangle-soup output (no exact edges). BSP-robustness, not exact-arithmetic. |
| Hidden cost? | csgrs's `f64` scalar **force-pulls a physics engine** (`rapier3d-f64`) + collision lib (`parry3d-f64`). **Unavoidable via features** — only a fork removes it. |
| Rendering? | Hand-written **OpenGL** via `glow 0.14` + `egui_glow` (FBO→blit). **No wgpu.** |
| Strategic flag | The roadmap's named solids (step/ramp/wall/column) are **parametric extrusions that need no CSG** — see §8. Is csgrs the right bet, or overkill? |

---

## 1. The stack at a glance

| Library | Version | Layer | Role in cad_solid | Purity |
|---|---|---|---|---|
| **csgrs** | 0.21.0 (git `5e7a37a`) | solid kernel | CSG booleans on meshes; primitive meshing | pure-Rust ✓ |
| **glam** | 0.29.3 | app math | `Vec2/3`, `Mat4`, `Quat` — all UI-facing geometry | pure-Rust ✓ |
| **nalgebra** | 0.34.2 | CSG boundary | `Matrix4<f64>` at the glam↔csgrs seam, isolated to `csg.rs` | pure-Rust ✓ |
| **parry3d-f64** | 0.25.3 | *(transitive)* | collision/geometry queries — **pulled by csgrs `f64`, unused** | pure-Rust |
| **rapier3d-f64** | 0.31.0 | *(transitive)* | **physics engine — pulled by csgrs `f64`, unused** | pure-Rust |
| **earcutr** | 0.5.0 (+0.4.3) | triangulation | ear-clipping for csgrs polygon→tris | pure-Rust ✓ |
| **spade** | 2.15.1 | *(transitive)* | Delaunay, via `geo` — not on cad_solid's path | pure-Rust |
| **eframe / egui / egui_glow** | 0.30.0 | UI (dev-dep) | the sandbox window (example only; lib is UI-agnostic) | — |
| **glow** | 0.14.2 | render (dev-dep) | raw OpenGL bindings for the custom `SceneRenderer` | — |
| **cad_kernel** | path | 2D reuse | shared geometry/parser/osnap (byte-identical to RUST_CAD) | pure-Rust ✓ |
| **cad_nurbs** | path | 2D reuse | **2D** NURBS/B-spline **curves** only (no surfaces) | pure-Rust ✓ |
| **serde** | 1.x | persistence | derive on the model types | pure-Rust ✓ |

> **wgpu is NOT in cad_solid's tree** (count = 0). The `wgpu 23.0.1` you'll see in
> `Cargo.lock` belongs to a sibling workspace member, not this crate.

---

## 2. csgrs — the solid-modeling kernel (deep dive)

**What it is:** a pure-Rust **Constructive Solid Geometry** library operating on **triangle
meshes via a BSP tree** (à la OpenSCAD/three-csg). The scalar is `f64`. Pinned to a git
commit because *every* crates.io release (0.16–0.20) hard-pins `core2 ^0.4`, whose only
release is **yanked** — so the pin is a correctness necessity, well-documented in
[Cargo.toml:9-16](../cad_solid/Cargo.toml#L9-L16).

### 2.1 Capability envelope (what csgrs CAN do)

| Category | Available (0.21.0) |
|---|---|
| **Primitives** | `cube cuboid cylinder frustum frustum_ptp sphere ellipsoid egg torus teardrop teardrop_cylinder icosahedron octahedron polyhedron arrow` + **gears** (`spur_gear_involute`, `spur_gear_cycloid`, `helical_involute_gear`) |
| **Booleans** | `union difference intersection xor inverse` |
| **Transforms** | `transform translate rotate scale mirror center float` (all via `Matrix4<f64>`) |
| **Solid verbs (2D→3D)** | `extrude extrude_vector revolve sweep loft offset offset_rounded minkowski_sum convex_hull` |
| **Advanced surfaces** | `metaballs`, `sdf` (signed-distance fields), `gyroid`/TPMS, `subdivide_triangles`, Laplacian `smoothing` |
| **Analysis** | `mass_properties`, `manifold` checks, `ray_intersections`, `dihedral_angle`, `bounding_box`, `connectivity` |
| **IO (feature-gated, OFF here)** | STL, OBJ, PLY, AMF, glTF, DXF, SVG, Gerber |
| **Bridges** | `to_trimesh`, `to_bevy_mesh`, `to_rapier_shape`, `to_rigid_body` |

### 2.2 What cad_solid ACTUALLY uses

Everything funnels through [csg.rs](../cad_solid/src/csg.rs) — the sole glam↔nalgebra seam:
- **Primitives:** `cuboid`, `cylinder` — *2 of ~18*.
- **Booleans:** `union`, `difference`, `intersection` — folded left→right over the feature
  history ([csg.rs:51-65](../cad_solid/src/csg.rs#L51-L65)).
- **Transform:** one `Matrix4<f64>` place-into-world ([csg.rs:44-47](../cad_solid/src/csg.rs#L44-L47)).
- **Tessellation:** `poly.triangulate()` → f32 triangle soup ([csg.rs:68-81](../cad_solid/src/csg.rs#L68-L81)).

**Utilization ≈ 5%.** The remaining 95% (extrude/revolve/sweep/loft, SDF, metaballs, gears,
analysis) is paid-for-but-idle headroom.

### 2.3 Limits that matter for a CAD solid modeler

1. **No fillet / chamfer / shell.** Confirmed absent from the entire source. For architectural
   massing (chamfered steps, rounded columns) there is **no primitive operation** — you'd
   approximate via boolean tricks or hand-built meshes. This is the biggest capability gap.
2. **Mesh, not B-rep.** Output is triangle soup with per-vertex normals — **no persistent
   exact edges/faces**, so downstream edge-selection, exact offset, and robust fillets are
   off the table. `bmesh` (boundary-rep) and a `nurbs` module exist upstream but the path
   cad_solid uses is pure triangle-BSP.
3. **Robustness = BSP, not exact arithmetic.** Coplanar/near-degenerate booleans can produce
   sliver tris or minor artifacts; there's no exact-predicate guarantee. Fine for massing,
   risky for tight tolerance work.
4. **f64→f32 downcast at the boundary.** cad_solid keeps f64 only inside csgrs, casting to
   f32 for render ([csg.rs:75](../cad_solid/src/csg.rs#L75)). The f64 precision **is the right call for BSP
   robustness** during the boolean — don't "optimize" it to f32 (see §2.4).
5. **Git-pin fragility.** The build depends on one unreleased commit of a fast-moving repo.
   Vendoring or a maintained fork would de-risk supply-chain drift.

### 2.4 The dependency cost — a physics engine you never call

`cargo tree` proves csgrs drags in **`rapier3d-f64 0.31.0`** (a rigid-body physics engine)
and **`parry3d-f64 0.25.3`** (collision/geometry queries), plus `geo` (GIS 2D), `rstar`
(R-tree), `simba`, `spade`. Root cause, from csgrs's own manifest:

```toml
f64 = [ "rapier3d-f64", "parry3d-f64" ]   # ← scalar feature FORCES physics
f32 = [ "rapier3d",     "parry3d"     ]   # ← f32 does the same, just the f32 variants
mesh = [ ]                                 # ← the CSG feature itself adds NOTHING
```

So the physics/collision libs are bound to the **scalar-type** feature, and csgrs needs one
of `f32`/`f64` to define `Real`. **Conclusion:** you cannot shed rapier/parry with feature
flags — *both* precisions pull a physics pair, and `mesh` alone won't compile without a
scalar. Switching to `f32` buys nothing (same weight, worse BSP robustness). **The only way
to drop them is to fork csgrs** and decouple `Real` from rapier/parry. The manifest comment
"unavoidable" ([Cargo.toml:12](../cad_solid/Cargo.toml#L12)) is accurate. Cost is compile-time + binary size,
not runtime — so **low priority**, but log it as a known fork-if-it-bites item.

---

## 3. glam 0.29.3 — the app-facing math

The whole UI/model layer is glam f32 (`Vec2/3`, `Mat4`, `Quat`). Single version in the tree
(no duplicates). This is the correct, idiomatic egui-ecosystem choice and matches how the 2D
kernel thinks. **No concerns.** The only glam↔nalgebra conversion is the ~4 lines in
[csg.rs:17-20](../cad_solid/src/csg.rs#L17-L20) — clean isolation.

## 4. nalgebra 0.34.2 — the CSG-boundary math

Present **only** because csgrs speaks nalgebra `Matrix4<f64>` / `Vector3<f64>`. cad_solid
pins it explicitly ([Cargo.toml:18](../cad_solid/Cargo.toml#L18)) so it **matches csgrs's nalgebra exactly** — a
mismatch here would be a type error at the boundary. Correctly quarantined to `csg.rs`; the
rest of the crate never sees it. **Good discipline — keep it that way.**

## 5. Rendering — eframe/egui + egui_glow + glow (OpenGL)

- **eframe 0.30** with `default-features = false, features = ["glow"]` → **OpenGL backend,
  not wgpu**. Confirmed: 0 wgpu in the tree.
- **Custom `SceneRenderer`** ([sandbox.rs:2446](../cad_solid/examples/sandbox.rs#L2446)): two hand-written GLSL programs
  (a scene shader + a fullscreen blit), an offscreen **FBO** with color texture + depth
  renderbuffer, drawn inside an `egui_glow::CallbackFn` ([sandbox.rs:1640](../cad_solid/examples/sandbox.rs#L1640)). Solids +
  wireframe/grid/ghost lines are uploaded per-frame.
- **Assessment:** minimal, appropriate for a sandbox. **No lighting model beyond flat/normal
  shading, no shadows, no materials, no MSAA config surfaced.** That's fine now, but note
  simLUX's *product* is a **lighting** tool — eventually the 3D view will want real shading
  to preview luminance. The `glow` path can do it, but it's bespoke GL you'll maintain by
  hand. If the 3D view ever needs PBR/shadows, revisit whether a higher-level renderer earns
  its keep.
- `unsafe impl Send/Sync for SceneRenderer` ([sandbox.rs:2463](../cad_solid/examples/sandbox.rs#L2463)) — required for the
  `Arc<Mutex<>>` handoff into the paint callback; correct but worth a comment on why the GL
  handles are safe to move (they're only touched inside the render thread's callback).

## 6. Triangulation — earcutr / spade

- **earcutr 0.5.0** is csgrs's chosen ear-clipping triangulator (the `earcut` feature we
  enabled, deliberately over `delaunay`/`spade` to stay light — [Cargo.toml:14-16](../cad_solid/Cargo.toml#L14-L16)). A
  **second copy 0.4.3** comes in transitively via `geo`. Harmless duplication.
- **spade 2.15.1** (Delaunay) is pulled by `geo`, **not** on cad_solid's triangulation path.
- Ear-clipping is fine for the convex-ish faces csgrs emits; if you later feed it nasty
  concave polygons with holes, Delaunay quality would matter — not today.

## 7. The 2D reuse layer — cad_kernel / cad_nurbs

- **cad_kernel** — pure-Rust 2D kernel, **byte-identical to RUST_CAD** (the whole point of
  the sandbox). Contributes the sketch geometry, parser, osnap. No 3D.
- **cad_nurbs** — despite the name, **2D curves only**: `NurbsCurve`, `BSplineCurve`,
  `KnotVector`, De Boor `evaluate`/`tessellate`. **No NURBS surfaces, no 3D.** So for 3D
  solids it contributes nothing today. If curved 3D surfaces ever matter, that's *new* work
  (or csgrs's own `nurbs` module) — cad_nurbs won't extend to it for free.

## 8. Verdict — how far does this stack carry the 3D roadmap?

Reading [SIMLUX_3D_SOLIDS_PLAN.md](SIMLUX_3D_SOLIDS_PLAN.md) against the stack surfaces a **real architectural
tension the team should decide consciously:**

- The plan's concrete deliverables — **step, ramp, wall, column** — are defined as
  **parametric objects in `cad_kernel/src/solid3d.rs` whose meshes are hand-generated by
  `cad_light/src/extrude.rs`** ("watertight, CCW outward … caps + a side quad per edge",
  §3). That path uses **no csgrs at all** — it's straight prism/extrusion triangulation.
- The **cad_solid sandbox** invests in **csgrs's full CSG** (booleans, BSP) + the modifier
  workflow. CSG booleans are only *needed* for shapes like "box minus cylinder" — which none
  of step/ramp/wall/column require.

**So there are two 3D efforts with different tools**, and the heavy dependency (csgrs +
transitive physics engine) sits on the branch whose named goals don't need it. This isn't
"wrong" — a free-form CSG modeler is a legitimate broader ambition — but **someone should
decide** whether:
  - **(A)** csgrs is the strategic 3D kernel (→ accept the physics-dep cost, and the sandbox
    is the future massing engine), or
  - **(B)** the roadmap solids go the `cad_light` hand-rolled-extrude route (→ csgrs is a
    sandbox-only experiment, and shouldn't be merged into the shipping path).

The mentor's read: **csgrs earns its place *only if* you need general booleans** (holes,
carved massing, intersections). If the near-term product is extruded massing + lighting,
the hand-rolled extruder is lighter, exact-edged, and physics-free — and csgrs is a
research spike, not a dependency to merge yet.

---

## 9. Recommendations (ranked)

1. **Decide A-vs-B (§8) before merging cad_solid into `cad_app`.** This is the load-bearing
   call; everything else is downstream of it. It's a product/architecture decision — yours.
2. **If keeping csgrs:** (a) plan for the **fillet/chamfer/shell gap** (§2.3.1) — it will bite
   architectural detailing; (b) treat the **git-pin** as a risk — vendor or fork-and-track;
   (c) log the **rapier/parry cost** as accepted-and-known (§2.4), fork only if compile time
   or binary size becomes a real problem.
3. **Keep the nalgebra quarantine** to `csg.rs` (§4) — it's the thing preventing csgrs's f64
   world from leaking into the app. Don't let a "convenience" nalgebra import escape.
4. **Do NOT downgrade csgrs to f32** for a perceived win (§2.4) — it sheds no weight and hurts
   BSP robustness; the f64→f32 downcast at the render boundary is already the right split.
5. **Name the two 3D efforts distinctly** in the docs so nobody assumes cad_solid's csgrs
   path and the `cad_light` extrude path are the same thing.

*No code changed by this review — MD only, per role.*
