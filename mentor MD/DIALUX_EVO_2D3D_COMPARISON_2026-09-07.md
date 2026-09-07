# DIALux Evo vs 3D-Factory — capabilities comparison (2D→3D mapping focus)

> Date: 2026-09-07 · Status: analysis/reference, no code written
> Method: project side from live code analysis (function + `file:line` refs). DIALux
> side from established DIALux Evo workflow knowledge + `mentor MD/LIGHTING_3D_STACK_RESEARCH_2026-07-22.md`
> (confirms DIALux geometry = in-house facet/mesh engine, .NET/OpenGL/Windows-only, photon-mapping calc).
> DIAL.com unreachable at write time — Evo menu-level detail is version-stable general, not release-specific.

---

## 1. What each system is

| | **DIALux Evo** | **This project (simLUX / 3D-Factory)** |
|---|---|---|
| Core identity | Lighting-design tool. A room-level BIM-lite model exists to serve EN 12464-1 verification: import/draw plan → rooms → luminaires → calc → report | A **complete 2D CAD** (AutoCAD-class, `cad_app/src/app.rs`, ~86k lines) with a CSG 3D solid modeller (`cad_solid`) *built inside it* + photometric engine (`cad_light`) |
| Modelling philosophy | **Room-centric**: rooms (floor outline + height) are the objects; walls/doors/windows are architecture that auto-generates the 3D room shell; the calc is derived from rooms | **Geometry-centric**: closed 2D outlines on construction planes → parametric CSG extrusions/booleans; rooms/walls/storeys are a thin bookkeeping layer over CSG solids |
| Geometry kernel | Facet/mesh engine, no B-rep, tessellated imports (3DS/OBJ/FBX/VRML/IFC; `.SAT` import only) | CSGrs kernel (`cad_solid`, pinned git `5e7a37a`), evaluated to tagged triangle meshes for render/calc |
| Calc method | Photon shooting (photon mapping) since evo; radiosity in DIALux 4 | Point-by-point direct inverse-square + **Monte-Carlo path interreflection** (`cad_light/src/calc.rs`, validated against DIALux reports in `cad_light/tests/identical_dialux.rs`) |

Both converge on the same deliverable: a watertight, tessellated room **mesh with per-surface
reflectance** handed to the light solver — this repo's own stated parity target
(`mentor MD/ROADMAP_3D_TO_MESH_2026-07-22.md`).

---

## 2. How each does 2D→3D mapping (the core question)

**DIALux Evo.** You work in a 2D plan view per storey; the 3D view is a *derived, always-synced*
view of one model. Mapping happens in layers:

1. **Plan import** — DWG/DXF/PDF floor plans brought in as an architectural underlay
   (scaled/aligned), then traced or converted into building elements; wall/room/window/door
   roles are assigned so 2D entities become 3D architecture.
2. **Architecture objects** — walls drawn as centerline + thickness + height (joined at
   corners), doors/windows inserted into walls **automatically cut openings**, rooms created
   from enclosed wall loops / drawn boundaries and get floor, ceiling, boundary walls;
   windows double as **daylight apertures**.
3. **Storeys** — Building → Storey → Room hierarchy; each storey is its own plan; rooms can
   span levels; storeys copy.
4. **Photometric derivation** — each room automatically carries calc planes (working plane
   default 0.75 m, per-room adjustable), wall/ceiling evaluation surfaces, and luminaires
   mount to ceilings; results (lux grids, isolux, UGR, luminance) overlay back onto plan and
   section views for the report.

**This project.** No single automatic mapping; four complementary, shipped mechanisms:

1. **2D outline → solid commands** (`cad_app/src/factory.rs`): Make building
   (`add_building_outline` :6257), Make 3D walls (`make_3d_wall_from_selection` app.rs:7854,
   promotes lines/walls to boxes), Make room (`add_room` :6349 — floor slab + per-edge wall
   boxes + ceiling from one closed outline; a room drawn inside a massing building auto-carves
   it into an annular wall, :6301). Input from the current 2D selection
   (`slab_outline_from_selection` app.rs:10871 — closed polyline, rectangle, or wall loop).
2. **Sketch-on-face / construction planes** — the "every plane has the full 2D toolset"
   mechanism: pick a face → `Frame::from_point_normal` (cad_solid lib.rs:992) →
   `to_uv`/`from_uv` maps the whole 2D pipeline onto the plane (`factory_enter_sketch`
   app.rs:11107; the plan doc is swapped for the sketch doc). Extrude/cut/recess of sketch
   loops (`factory_extrude_sketch` :11703, `factory_cut_sketch` :11919), nested loops become
   profile holes.
3. **Openings as bound CSG cutters** — sketched rectangle on a wall → thickness-scanning
   `Difference Extrusion` cutters bound to wall segments via `Feature::target` (lib.rs:646),
   surviving wall re-derivation (`rederive_wall` factory.rs:7523); then a window mesh or fully
   parametric door drops into the hole (`factory_draw_aperture` app.rs:10426,
   `cad_solid/src/door.rs`).
4. **Storeys** — pure height stack (`Storey` factory.rs:3545, `storey_base_z` :3939),
   insert/delete shifts levels (:4005), **duplicate floor** repeats a level upward (:4028),
   membership by z-band.
5. **Bidirectional view mapping** — 2D plan drawn as a depth-tested or X-ray underlay inside
   the 3D view (`plan_lines` :10220, `paint_plan_underlay` app.rs:10525); conversely 3D solid
   footprints, room outlines and furniture hulls overlay the 2D canvas
   (`plan_overlay_shapes` app.rs:66951); horizontal light-model sections cut the mesh at z
   (`section_at_z` :4973).

Both genuinely do 2D→3D; the difference is **guided vs. general**: DIALux *derives* building +
calc geometry from rooms automatically; simLUX *builds* CSG geometry from arbitrary 2D and then
records what it built as rooms/storeys.

---

## 3. Capability comparison — 2D→3D mapping scope

Legend: ✅ implemented+tested · 🔶 partial/engine-only · ❌ missing · 🗓 planned in repo docs

| Capability | DIALux Evo | This project | Evidence / gap detail |
|---|---|---|---|
| 2D plan import as tracing underlay | ✅ DWG/DXF/PDF | 🔶 DWG/DXF ✅, **PDF ❌** | DWG/DXF import ships (`cad_io`, 2,515 lines); PDF is export-only (cad_plot). No PDF import anywhere |
| Plan entities → 3D architecture by layer role | ✅ (walls/windows/doors/rooms from imported layers) | 🔶 mechanism specced only | `mentor MD/SIMLUX_DIALUX_PLAN.md` §5 "Layer → 3D dialog": per-layer Role = Wall/Floor/Ceiling/**Opening**/Obstruction + base/top height + reflectance — **never built** |
| Wall drawing (thickness, height, joined corners) | ✅ | ✅ | 2D `Wall` (kernel) + "Make 3D walls" promotion; curved centerlines sampled 16 segs; wall vertex insert/delete/drag engine done + tested (factory.rs:7523+, app.rs 11229–13530 tests) |
| Room from enclosed space | ✅ click-in-room / enclosed loop recognition | 🔶 closed-outline → Make room only | No *automatic* detection that a drawn wall loop encloses a space; rooms never share/compute wall loops from existing walls (`room_at` :6759 exists) |
| Floor + ceiling slabs per room | ✅ auto | ✅ | From the same footprint ring (invariant: `add_room` :6349); open-to-sky option; L/arbitrary shapes exact (Extrusion profile, not bbox) |
| Door/window in wall → **auto opening cut** | ✅ | ✅ (drawn-rectangle / aperture workflow) | CSG Difference cutters bound by `target`, survive wall re-derive, orphans flagged (:7523–7721); grid-scan for curved walls (:12618–12660); parametric door generator (`cad_solid/src/door.rs`) |
| Opening as first-class 2D entity (editable in plan, re-identifiable) | ✅ | 🔶 | No kernel Window/Door dobjects yet (geom.rs:149–151 comment: planned, wall-referencing); openings tracked as CSG cutters + "Openings" list (app.rs:8704); editable via face-sketch reshape (:8815) |
| Pitched/shed roofs, raked walls | ✅ | ❌ | `Feature` has no tilt DOF — `Placement` offers u,v,lift,spin only; `rake_deg` stored inert (factory.rs:117–121). Blocker C4 (`mentor MD/DIALUX_TOOLSET_REVIEW_AND_PHASE1_SPEC_2026-07-23.md`): "geometrically unrepresentable today" |
| Storeys / multi-floor plans | ✅ Building→Storey→Room | ✅ | Storey stack, insert/delete re-derive heights and shift features (:4005–4106), duplicate floor :4028; UI level picker + `[` `]` (app.rs:14717) |
| Multi-storey void / atrium | ✅ | ❌ | Blocked on boolean command journey (`mentor MD/BOOLEAN_AS_COMMAND_2026-07-17.md`); engine `BoolOp` exists, command + multi-body decision owed |
| Live 2D↔3D sync (edit in one, other updates) | ✅ one model, two views | ✅ two-way overlay | 3D→2D footprint overlay app.rs:66951; 2D plan underlay in 3D :10525; sketches are swap-in docs, not views — the one asymmetry |
| Full 2D toolset usable *on* 3D faces | N/A (Evo is plan-only drafting) | ✅ | Sketch-on-face with projected reference edges + osnap (`factory_enter_sketch` app.rs:11107; `frame_face_edges` factory.rs:10078) |
| Per-room working plane + calc grid derived from room | ✅ (0.75 m default, per room) | 🔶 | Auto per-room CalcPlane inset by wall zone at 0.8 m (`room_result` light.rs:1230, `calc_targets` :2738); EN 12464 standard grid (`en12464_spacing` types.rs:305); **height is global default, not per-room; default 0.80 not 0.75** |
| User calc objects (points/lines/surfaces, vertical planes) | ✅ | ❌ | Vertical/scalar/semi-cylindrical/luminance metrics exist engine-only (calc.rs:109–175), nothing reachable from UI |
| Luminaire layout / arrangement | ✅ rows/cols + design scenes, switching groups | 🔶 rows×cols array only | `add_luminaire_grid` light.rs:2305; no auto-design-to-target-lux, no light scenes/groups/circuits, no emergency |
| UGR | ✅ per-observation-point / table (Evo reports) | 🔶 engine ✅, app ❌ | `ugr_at` ugr.rs:124 with occlusion + maintenance; **zero app callers**; map/report/3D layers planned (`mentor MD/SIMLUX_UGR_MAP_PLAN_2026-08-21.md`); 5 known engine defects (aim, double cd, flux, occlusion, maintenance) listed in that doc |
| Daylight through windows | ✅ sky models, climate data, daylight factor/annual | ❌ | `solar.rs` = sun position only (render feed); aperture role (Role=Opening) is the unbuilt prerequisite; EN 17037 plan (`mentor MD/SIMLUX_SCENE_AND_DAYLIGHT_PLAN.md`); CIE overcast formula ready in plan §4.3 |
| Reflectance per surface for calc | ✅ material catalog | 🔶 | Per-face colour/texture exists; but seam `cad_solid::SolidMesh` (flat soup) → `cad_light::Mesh` (indexed, per-surface material) is **unbuilt** (Track B); defaults floor 0.2/wall 0.5/ceiling 0.7 (`default_materials` types.rs:57) |
| Report sheets incl. 2D plans/sections with results | ✅ PDF/DOCX, layouts | ✅ PDF/HTML | `report/layout.rs`, custom PDF writer, schedule/installation/band grids/isolux; no section/elevation sheets of the 3D model yet (sections only feed the light model) |
| Interop: IFC | ✅ export (BIM) | ❌ | absent tree-wide; DXF/RSM/DWG/OBJ/3DS/FBX/glTF imports; **no 3D model export at all** except Radiance scene (daylight-only, radiance_export.rs) |
| Undo over 3D ops | ✅ | ✅ (one snapshot per op) | `snapshot_factory()` single-undo; deeper multi-step 3D undo owed (`mentor MD/DIALUX_TOOLSET_REVIEW_AND_PHASE1_SPEC_2026-07-23.md` item 1-A) |

---

## 4. Gap analysis — what is missing for DIALux-grade 2D→3D mapping

Ranked by leverage. DIALux's mapping is *automatic and room-first*; this project's is *manual
and geometry-first*.

### P0 — make plan→3D automatic (the biggest product gap)

1. **"Layer → 3D" conversion dialog** (`SIMLUX_DIALUX_PLAN.md` §5, decisions D1/D2): imported
   DXF plans get per-layer roles (Wall/Floor/Ceiling/Opening/Obstruction) + height +
   reflectance, then one dialog extrudes the whole plan into a building. This is Evo's
   "insert architecture from plan" equivalent. Everything needed exists underneath (layer
   handles, `add_wall`/extrusion, reflectance table). Nothing built.
2. **Room detection from enclosed wall loops** — Evo's click-in-enclosed-space. Needs:
   walking a wall/polyline graph for closed rings (2D side already solves junctions in
   `cad_wall`), then feeding the ring to the existing `add_room`/slab/wall path so a drawn
   plan becomes rooms without manual outline re-tracing.
3. **Finish the boolean/openings command journey** — unblocks atria/voids, polygon cutouts in
   slabs/roofs, general subtract (`cad_solid` `BoolOp` + `csg::eval` already honour it; only
   the command + multi-body ownership decision is owed, per
   `mentor MD/BOOLEAN_AS_COMMAND_2026-07-17.md`). Openings are the hinge for daylight and
   Evo-style door/window editing.

### P1 — close the semantic gaps Evo users take for granted

4. **Tilt DOF on `Feature`/`Placement`** → pitched/shed roofs and raked walls (blocker C4).
   Without it, multi-storey buildings with real roofs — the standard Evo test scene — are
   unrepresentable.
5. **Wire UGR into the app** (observer placement, room report, map at 1.2 m/1.6 m) per
   `mentor MD/SIMLUX_UGR_MAP_PLAN_2026-08-21.md`; fix the 5 Phase-0 engine defects first
   (aim ignored ugr.rs:164, double candela :180, flux override, no occlusion, no maintenance).
6. **Daylight**: Role=Opening → window apertures admit sky; CIE overcast → daylight factor in
   `cad_light` (specced with formula + EN 17037 metrics in
   `mentor MD/SIMLUX_SCENE_AND_DAYLIGHT_PLAN.md`).
7. **Per-room calc settings**: per-room working-plane height (0.75 m EN default), per-room
   grid density, wall-zone semantics following room boundaries as they edit. `set_room_height`/
   `rebuild_room_levels` (factory.rs:6528–6606) already keep feature ids stable, so tractable.
8. **`SolidMesh → cad_light::Mesh` material seam** (ROADMAP steps 1–4/9): per-surface
   reflectance grouping is the prerequisite for Evo-equivalent wall luminance results and
   believable interreflection; per-face paint already provides the UI half.

### P2 — breadth DIALux has that this doesn't

9. User calc objects (points/lines/vertical surfaces) + exposing the engine's
   vertical/scalar/semi-cylindrical metrics.
10. PDF underlay import; IFC import/export; OBJ/STL export of the 3D model (round-trip
    furniture/Evo models); glTF placement beyond the furniture library.
11. Light scenes / switching groups / circuits; auto layout to target lux; emergency lighting;
    energy (LENI) — all pushed to v0.3–v0.4 in the master plan.
12. Multi-step 3D undo (currently one snapshot per operation).

---

## 5. Bottom line

- **Philosophically different mappings**: DIALux Evo is a *derived model* (plan/storey/room
  semantics auto-generate walls, slabs, openings, calc planes, daylight apertures — one synced
  model, low geometry freedom); simLUX is a *constructed model* (arbitrary 2D on any plane →
  CSG solids, rooms/storeys recorded on top — far more geometric freedom, nothing automatic).
- **The 2D→3D core here is genuinely strong and mostly shipped**: building/room/slab/walls
  from closed outlines, cutters that survive wall re-derivation, storey stack + duplicate
  floor, sketch-on-face with the full 2D toolset, and two-way plan/3D overlays all exist with
  300+ factory tests and ~600 app tests. That exceeds DIALux on *drafting freedom on faces*
  and equals it on most *object-level* mapping — DIALux cannot sketch arbitrary geometry on a
  wall.
- **What's missing is the automation layer DIALux puts on top**: layer-role conversion of
  imported plans (P0), enclosed-space room detection (P0), the finished boolean command for
  atria/general voids (P0), roof/rake tilt (P1), UGR/daylight wiring (P1), and the calc
  mesh+reflectance seam (P1). The repo's own docs already carry approved specs for most of
  these (`SIMLUX_DIALUX_PLAN.md` §5, UGR map plan, scene/daylight plan, mesh roadmap) — they
  are implementation debt, not open design questions, with two exceptions: the boolean
  multi-body ownership decision and the compliance-vs-modeller strategy question (collision C1
  in `mentor MD/DIALUX_TOOLSET_REVIEW_AND_PHASE1_SPEC_2026-07-23.md`) are still unanswered by
  the owner.
