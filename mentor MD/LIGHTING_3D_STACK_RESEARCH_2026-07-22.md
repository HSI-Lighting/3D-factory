# Are we on the right 3D path? — DIALux & peers, researched 2026-07-22

**Owner's worry:** *"our 2D editing is excellent, but in 3D I am scared whether we are in the
right path or not."* This is the evidence-based answer. **Short version: yes — the mesh/CSG-first
path is exactly what the whole lighting-design industry does, including DIALux. The place to be
scared is NOT the modeler — it's the light-calculation engine, which is where all of them
actually compete.**

---

## What DIALux evo actually uses (sourced)

| Layer | DIALux evo | Confidence |
|---|---|---|
| Render API | **OpenGL 3.2**, Windows-only | CONFIRMED (system reqs) |
| App shell | **.NET** (6.0.29 in evo 12.1) → C#/.NET over OpenGL | .NET confirmed; toolkit inferred |
| Geometry kernel | **In-house mesh/facet engine** — NOT a licensed B-rep kernel | INFERENCE (strong) |
| Light calc | **Photon shooting (photon mapping)** since evo; radiosity in 4.x; CPU multicore | CONFIRMED (DIAL's own paper) |
| Pretty pictures | Separate raytracer for presentation images ONLY (not the numbers) | CONFIRMED |

**Why "in-house mesh, not B-rep":** DIALux's 3D interchange is all **tessellated** formats — 3DS,
OBJ, FBX, VRML, IFC (and `.SAT` *import* only) — not STEP/IGES B-rep exchange. No public source
(job posts, credits, license manifest) names OpenCASCADE/ACIS/Parasolid embedded. DIAL's calc
paper talks about *their* CAD subsystem needing "substantially better performance." That is the
signature of a **custom mesh/facet engine**, which is *exactly what cad_solid is*.

Source: DIAL, "DIALux evo – New calculation method"
(dialux.com/fileadmin/documents/DIALux_evo-_New_calculation_method.pdf); dialux.com FAQ /
evo.support-en hardware + format articles.

## The peers agree

- **Relux** — calc = **radiosity + an optimised version of Radiance** (raytrace); CPU. Visual
  eye-candy delegated to **Chaos Enscape (GPU)** since 2024.1 — *the standards calc stays their
  own radiosity module.*
- **AGi32** — full **radiosity** (surfaces → patches → elements, Adaptive Patch Subdivision) +
  a fast Direct-Only method; raytrace is presentation-only, **explicitly excluded from the
  photometric calc**. ±2% illuminance accuracy claim.

## The architecture truth that matters

**Every one of them feeds the light calc with tessellated polygon MESHES + per-surface
reflectance — never B-rep.** Radiosity *requires* a surface mesh by definition (patch/element
energy exchange). Radiance compiles N-sided polygons into an octree. **Even tools that author
with a B-rep kernel tessellate to meshes before lighting.** (CONFIRMED across AGi32 docs, Radiance
refer/long.html, DIAL's paper.)

### B-rep kernel vs mesh/BSP CSG (the csgrs question)
- **B-rep (OpenCASCADE/ACIS/…):** exact curves/NURBS, native STEP/IGES — but booleans are
  historically **fragile** at tangent/coincident cases and "perform extremely poorly" on
  high-res meshes. Precision you don't need for a room shell; fragility you don't want.
- **Mesh/BSP CSG (csgrs lineage):** booleans are **simple and can be provably robust** (BSP
  clipping, or exact/adaptive-precision predicates — which csgrs `main` has in fact moved to).
  Curves are **tessellated approximations**; output is retessellated. For **planar architectural
  geometry (walls/floors/ceilings) this is the natural fit.**

---

## Mentor verdict

1. **The path is sound.** cad_solid (csgrs, mesh) → tessellated room + reflectance → light calc is
   the correct pipeline direction, and it's what the market leader does with an in-house mesh
   engine. You are NOT off in the weeds. Do **not** chase a B-rep kernel — it's overkill for rooms,
   its booleans are the fragile part, and you'd tessellate before the calc anyway.

2. **This validates [[project_simlux_factory_is_the_room]] exactly.** "Factory builds the mesh
   room → SIMLUX lights it," and Phase 1 = **per-surface reflectance**, is confirmed as *the*
   substance. The mesh the Factory already produces is precisely the calc's input.

3. **Where the fear should actually point: the CALC, not the modeler.** DIALux/Relux/AGi32 compete
   on radiosity/photon-mapping accuracy, CIE-171 validation, CPU multicore speed — not on solid
   modeling. So: keep the csgrs modeling *adequate*, and put the real investment into `cad_light`
   (the radiosity/raytrace engine + materials + IES/LDT). Elaborate solid features (multi-body
   booleans, raked walls) are nice-to-have, secondary to the calc.

4. **csgrs caveats to track:** curved elements (round columns, curved walls) are tessellated —
   fine if sampled finely enough for both display and radiosity patches. We pin csgrs at a commit;
   its `main` has newer exact-predicate booleans — worth revisiting if we hit boolean robustness
   issues.

**Net:** the 2D being excellent and the 3D feeling uncertain is normal — but the uncertainty is
misplaced on the modeler. The modeler is on the industry-standard path. The differentiator, and
the hard part, is the light calc on top of the mesh. That's where to aim next.
