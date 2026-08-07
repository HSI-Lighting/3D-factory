# 3D Factory — Work Path

A chronological record of the 3D Factory debugging/feature work: the symptom, the
root cause (mostly pinned down from **session-recorder dumps**, not guesswork), the
fix, the files touched, and how it was verified. Newest work at the bottom.

> Build note: the app is `cad_app` (bin `simlux`). Windows holds `simlux.exe` open
> while the app runs, so `cargo build` **fails to relink** (`Access is denied`) if the
> app is open. `cargo check` compiling is **not** proof the exe updated — always close
> the app, rebuild, and relaunch. Verify the running build from a recorder dump (the
> `🧱 FACTORY` detail format changes when the code changes).

---

## 1. 3D zoom-out limit

- **Symptom:** couldn't zoom out far enough to frame a whole imported building plan.
- **Root cause:** camera distance was clamped to a fixed **400 world units** in five
  places (scroll, drag-zoom, `fit`/Zoom-Extents, `zoom_by`, `zoom_window`). Too small
  for a DXF authored in millimetres (a 100 m building spans 100 000 units).
- **Fix:** `FactoryState::max_cam_dist()` scales the ceiling to **20× the model span**
  (floor 400); all five sites route through it. Near-plane in `light3d::mvp` now scales
  with distance (`(dist*0.001).max(0.05)`) to avoid z-fighting when dollied far back.
- **Files:** `cad_app/src/factory.rs`, `cad_app/src/app.rs`, `cad_app/src/light3d.rs`.
- **Verified:** 177 → 180 tests pass across the run; framing confirmed on a large import.

---

## 2. DXF import was ~20 s slow

- **Symptom:** opening a 734 MB DXF (only 4 800 objects) took ~20 s.
- **How found:** added an `ImportStage` recorder event; the dump showed **`parse dxf =
  20 296 ms`** was 100 % of the delay (read 283 ms, install/fit/index all instant).
- **Root cause:** `cad_io::dxf::read_dxf` → `parse_pairs` allocated a fresh `String`
  per value line — tens of millions of tiny heap allocations into one giant
  `Vec<(i32,String)>`, then `read_entities`/`read_blocks` cloned every field again.
- **Fix:** **zero-copy tokenization** — `parse_pairs` now returns `Vec<(i32, &str)>`
  borrowing slices of the file text; pairs are `Copy`, so field collection copies
  instead of allocating. Only retained values (layer/block/linetype names) `.to_string()`.
- **Files:** `cad_io/src/dxf.rs` (reader rewritten to `&str` pairs).
- **Verified:** 26 `cad_io` tests pass (round-trip unchanged). Synthetic bench: **240
  MB/s debug (~3 s for 734 MB), 2 605 MB/s release** — was ~6–70× faster.
- **Note:** DWG opening still needs an external DWG→DXF converter (ODA / LibreDWG) set
  via `RUSTCAD_DWGCONV` or `tools/dwgconv/dwgconv.cmd` — none installed here yet.

---

## 3. Session recorder now sees the 3D Factory

- **Ask:** "record every object generated, its sides, the tool used… so when I say
  something isn't showing up you can look at the dump instead of guessing."
- **Before:** dumps were hundreds of 2D click events with **zero** 3D-Factory info.
- **Fix:** `DbgEvent::FactoryOp { op, source, detail, features_before, features_after,
  bodies, tris }` + `CadApp::factory_op_evt()`. Emitted by:
  - `factory_extrude_sketch` → `extrude` / `furniture-extrude`
  - `factory_cut_sketch` → `cut-through` / `recess` (detail carries targets, probe,
    normal, inward, depths, lift/h, loops, bodies_cut)
  - `import_furniture_obj` → `import-mesh` (mesh tris/verts, asset idx, instance count)
  - `source` = `active-face-sketch` / `finished-sketch` / `2d-selection` / `file`
  - Formatter flags `⚠ NO FEATURE ADDED` (except recompute/delete/import-mesh).
- **Files:** `cad_app/src/dbg_recorder.rs`, `cad_app/src/app.rs`.
- **Impact:** every fix below was diagnosed **from a dump**, not from screenshots alone.

---

## 4. Room-elements CUT — the long thread

The cut tool went through several dump-driven iterations. Final behaviour lives in
`factory_cut_sketch` + `assembly_span` (`cad_app/src/app.rs`).

1. **"Cuts both sides."** Old code used a **symmetric ±(diagonal×1.5)** span centred on
   the face → tunnelled across the building and cut the far wall. → Made the cut
   **one-directional** (orient `inward` by the target centroid).
2. **"Through doesn't fully cut the wall."** Single-target cut only subtracted from ONE
   body, but the dump showed **`bodies=17`** — a wall is several overlapping solids
   (shell + room leaf + slab edge). One dump even hit a **5 mm sliver body**
   (`thickness=0.005`). → `assembly_span` ray-marches **every** Union body, walks the
   run of material until the first gap > 0.4 m (the room), and the cut subtracts a
   Difference from **each** body in that run (re-finding by id after each insert).
3. **"Visible inside, not outside."** The through-cut was still one-sided (started at
   the picked face, went inward), so it left the opposite skin. → `assembly_span` is now
   called **both directions** (`inward` and `-inward`); the cutter spans the **full wall
   thickness from both faces** + margin, still gap-bounded per side so it never reaches
   the far wall.
   - **Inward orientation:** picked face uses `-n` directly (pick_face normal is
     outward; robust for thin slivers); 2D selection orients by target centroid.
   - **RECESS** stays a one-sided blind pocket, depth `min(element_height, in_depth-EPS)`
     so it can never break through.
4. **Still "inside not outside" — target selection.** A dump showed `out_depth=0
   bodies_cut=1`: the exterior surface was a **separate body** the single center probe-ray
   missed. Verified the extrusion convention in the kernel (`csg.rs` `extrude(h)` rises
   local z 0→h; `world_matrix` `o = origin + n·lift`, so the cutter spans `[lift, lift+h]`
   along +n — direction was right all along). → Select cut targets by **AABB overlap**:
   build the cutter's swept world box (`frame.from_uv(loop pts) + n·{lift, lift+h}`) and
   cut **every** Union body whose `world_aabb` overlaps it. Over-selection is a harmless
   no-op; the box is gap-bounded so it stays local to the wall.
- **Dump detail:** `🧱 FACTORY [cut-through] … targets=[…] in_depth=… out_depth=…
  bodies_cut=N …`.
- **Verified:** 180 tests pass; iterated against real dumps.

---

## 5. FBX furniture import

- **Ask:** import `.fbx` furniture.
- **Fix:** `cad_app/src/mesh_io.rs::parse_fbx(&[u8]) -> ObjMesh` — **binary** FBX (magic
  `Kaydara FBX Binary  \x00\x1a\x00` + version; 7.5+ uses u64 node offsets). Walks the
  node tree, pulls every `Vertices` (f64) + `PolygonVertexIndex` (i32, bit-negated last
  index closes a polygon), fan-triangulates, converts **Y-up → Z-up** `(x,y,z)→(x,-z,y)`.
  Array props may be zlib-compressed (`Encoding==1`) → inflated with **`miniz_oxide`**
  (new direct dep, already in the tree via `image`/`png`). Wired into
  `import_furniture_obj` + the file dialog (`.obj` / `.3ds` / `.fbx`).
- **Verified:** 3 synthetic round-trip tests (uncompressed, zlib-compressed, reject-garbage).
- **Limits:** ASCII FBX not supported; `UpAxis` override not read (assumes Y-up).

---

## 6. Furniture "not viewable" — placement, not import

- **Symptom:** "furniture still can't import."
- **How found:** the dump proved import **worked**: `🧱 FACTORY [import-mesh]
  file=mashrabiya1 fmt=3ds mesh_tris=69015 furniture_insts=1`.
- **Root cause:** `place_furniture(idx, Vec3::ZERO)` dropped it at world origin, but the
  drawing lives at DXF coords (**X≈3619, Y≈956**) — furniture landed **~3.6 km away**,
  off-screen.
- **Fix:** `import_furniture_obj` now places imports at the **model centre**
  (`self.factory.cached.bounds()` centre; fallback `ZERO`).
- **Files:** `cad_app/src/app.rs`.

---

## Current status

- All `cad_app` tests green (**180 passed**), `cad_io` **26 passed**.
- Latest exe rebuilt with the two-sided cut + furniture-placement fixes.
- **How to confirm you're on the latest build:** a `cut-through` dump line reads
  `primary=#… targets=[…] in_depth=… out_depth=… bodies_cut=…`. If it still says
  `thickness=… tgt_aabb=…`, the app relaunched a stale exe.

## 7. Transform gizmos — rotation (Phase 1)

- **Ask:** rotate furniture & room elements in all axes (chose **drag rings**); scale &
  extrusion by drag (chose **uniform + per-axis**).
- **Phase 1 (done): 3-axis rotation.**
  - **Furniture:** `FurnitureInst.rot_deg` → `rot: [f32;3]` (world X/Y/Z Euler); transform
    is scale → `Mat3::from_euler(XYZ)` → translate (normals too). Sidecar `FurnitureInstRec`
    keeps `rot_deg` (Z) + new `#[serde(default)] rot_xy` (X,Y) for back-compat.
  - **Features:** `Placement` gained `#[serde(default)] pitch_deg` (about local u) and
    `roll_deg` (about local v); `world_matrix` composes `Rz(spin)·Rx(pitch)·Ry(roll)`.
    Setter `set_feature_rotation` / reader `feature_rotation`. (~20 `Placement {…}` literals
    updated for the two new fields.)
  - **Rotation-ring gizmo:** `GizmoMode::{Move,Rotate}` toggle in the 3D bar. Rotate mode
    draws 3 rings (`rotation_rings`), picks the nearest (`pick_ring`), and drags via
    `rot_begin`/`rot_update`/`rot_end` — furniture composes a world-axis quaternion (kept as
    Euler for the fields); a feature adds the swept angle to the ring's plane-local
    placement angle. Rings are world axes for furniture, plane-local (u/v/n) for a feature.
  - **Numeric fallback:** X/Y/Z (furniture) and pitch/roll/spin (feature) angle fields in
    the properties panel, same values the rings write.
- **Verified:** 183 cad_app + 54 cad_solid + 26 cad_io tests pass; 3 new rotation tests.

## 8. Path sweep — extrude / cut along a path

- **Ask:** sweep a drawn cross-section along a drawn path — to CREATE a solid and to CUT
  one; in Room elements and Furniture. Chose: **draw the path**, **perpendicular sweep**.
- **Kernel:** csgrs already has `Sketch::sweep(&[Point3])` — no hand-rolled geometry.
  Added `Path`/`PathId`/`Model.paths` (+`add_path`/`path`) and `Primitive::Sweep { profile,
  path, bmin, bmax }` (bmin/bmax cache the local AABB); `csg.rs` builds it via `.sweep()`.
- **App:** `factory_resolve_sweep` pairs one closed loop (section) + one open polyline
  (path) from the active sketch / 2D selection / finished sketch; `factory_path_extrude`
  (Union) and `factory_path_cut` (Difference, relocated after the target body). UI in ▼ Room
  elements, ▼ Furniture, and the face toolbar. Recorder ops `path-extrude` / `path-cut`.
- **Verified:** 55 cad_solid (new `sweep_along_a_straight_path_makes_a_solid`) + 183 cad_app
  + 26 cad_io tests pass.

## 9. Furniture render lag + a perf monitor for it

- **Symptom:** the app lagged badly right after importing/placing furniture; a library
  re-place showed nothing yet still lagged.
- **How found (dump):** `import-mesh file=Couch3 mesh_tris=94247 mesh_verts=282741`.
- **Root cause 1 (lag):** the 3D render rebuilt the ENTIRE opaque vertex buffer every
  frame — `furniture_verts()` re-transformed + re-shaded all ~282k vertices, re-copied
  the buffer into the paint callback, and re-uploaded ~6.7 MB to the GPU — *every frame*.
- **Fix 1:** cache the opaque buffer behind a cheap signature. `FactoryState` gained
  `render_cache: RefCell<RenderCache{ sig, ready, verts: Arc<Vec<V3>> }>` + `geom_version`
  (bumped in `recompute`). `opaque_sig()` hashes only the cheap inputs (geom_version,
  hide/cutaway toggles, per-instance furniture poses, colour maps combined
  order-independently) — NOT the triangle soup. `opaque_verts() -> Arc<Vec<V3>>` returns
  the cached Arc when unchanged; the paint closure holds the Arc, so a steady/orbit frame
  does zero re-transform and zero re-copy. (GPU upload stays per-frame — the two 3D views
  share one `Scene3dRenderer` and alternate, so a version-keyed static VBO would thrash;
  the CPU transform was the real cost.)
- **Root cause 2 (library re-place invisible):** the ▼ Furniture "place another copy"
  called `place_furniture(i, Vec3::ZERO)` → world origin, ~3.6 km from the DXF-coordinate
  building. Same bug fixed for IMPORT but missed on the library path.
- **Fix 2:** `FactoryState::default_place_at()` (model centre via `cached.bounds()`), used
  by both import and the library menu; the menu now `fit()`s too.
- **Perf MONITOR (the ask):** furniture is a mesh INSTANCE, invisible to `FactoryOp`'s
  feature/body counts, so its load showed up nowhere. New `DbgEvent::FactoryPerf`
  (dbg_recorder.rs) emitted from `render_factory_panel`: phase `buffer-rebuilt` on every
  cache rebuild (the import/edit moment — carries total tris, upload MB, CPU `build_us`,
  detected by `Arc::ptr_eq` vs the previous buffer), and phase `slow-frame` when an
  orbit/drag frame blows the 16.7 ms refresh budget (throttled ≤3/s). Dump line:
  `📊 FACTORY PERF [phase] frame … build … tris=… furn=… heaviest=… upload=… MB ⚠ SLOW`.
- **Verified:** 186 cad_app tests (3 new: cache reuse/invalidation, library placement,
  perf-monitor formatting + heaviest-mesh helper). Binary rebuilt.

## 10. Furniture-SELECTION fps crater (O(verts) AABB) + CPU/GPU clarification

- **Symptom:** after the §9 cache fix, fps still dropped — but only *while a furniture is
  selected*; clicking empty space or the building recovered it. Toggling CPU↔GPU didn't help.
- **How found (FactoryPerf dump):** `build 0.0 ms` throughout (opaque cache working), yet
  frames swung 30 ms ↔ 140–200 ms, and the swing tracked selection.
- **Root cause:** when a furniture is selected, `gizmo_view` (app.rs ~6262) AND
  `overlay_lines` (~6542) each call `selection_aabb → furniture_aabb`, which swept ALL ~94k
  vertices via `furniture_point` — which rebuilt `rot_mat()` (`Mat3::from_euler`, 6 trig)
  *per vertex*. Two O(94k×trig) sweeps per frame ≈ 110 ms in a debug build. Deselected,
  neither runs → 30 ms.
- **Fix:** cache each asset's LOCAL AABB once (`FurnitureAsset { local_min, local_max }` via
  a new `FurnitureAsset::new()`; both construction sites route through it), and transform the
  **8 corners** of that box (rot_mat built once) → O(8). Valid enclosing AABB (exact without
  rotation). Test `furniture_aabb_encloses_all_verts_cheaply`.
- **CPU/GPU toggle:** `render_mode` (Cpu/Gpu/Apx) switches only the **2D canvas** renderer;
  `render_factory_panel` always uses the OpenGL `light3d_renderer` and ignores `render_mode`,
  so it correctly has no effect on 3D-furniture perf — working as designed, not a bug. (A
  ~30 ms whole-app baseline with no furniture is the 2D plan + egui, a separate cost.)
- **Verified:** 187 cad_app tests (1 new). Binary rebuilt.

### 3D Factory now honours the render mode (APX proxy)

- **Ask:** "I want the 3D Factory to also utilise the render modes." Since 3D is inherently
  an OpenGL pass (no software 3D rasterizer → CPU==GPU there), APX is the meaningful lever
  (chosen: **APX = fast bounding-box proxy for heavy furniture**).
- **How:** `furniture_verts(apx)` + `opaque_verts(apx)`/`opaque_sig(apx)` (apx hashed into the
  cache signature, so flipping mode rebuilds once). In APX, a piece with tris >
  `APX_FURNITURE_TRIS` (5000) draws as a 12-triangle box (`push_furniture_box`, transforms the
  8 cached local-AABB corners); light pieces + GPU/CPU draw full detail. Render site reads
  `self.render_mode == RenderMode::Apx`. APX is user-selected (never force-switched).
- **Verified:** 188 cad_app tests (2 new: `apx_mode_proxies_heavy_furniture_only`, cache
  apx-vs-full distinct). Binary rebuilt. The FactoryPerf dump shows tris drop under APX.

## 11. Path sweep — guided-pick rewrite (clearer section/path selection)

- **Symptom:** the path extrude/cut selection was unintuitive — you had to draw a closed
  cross-section AND an open path in the same sketch/selection, and the app silently guessed
  which was which (closed=section, open=path). No prompts, no control, brittle detection.
- **Fix (user chose "Guided pick"):** an explicit two-click flow.
  - Menu/toolbar buttons now call `factory_begin_sweep_pick(cut, furniture)` → sets
    `FactoryState.sweep_pick` + a status prompt ("① click the CROSS-SECTION").
  - A click in the 3D view routes to `factory_sweep_pick_click` (added before the
    paint/select branches): 1st click `pick_ground_closed` picks the smallest closed plan
    shape under the cursor (the section, highlighted); 2nd click `pick_ground_path` picks the
    nearest plan object (the path, excluding the section) → `factory_commit_sweep` →
    `factory_build_sweep`. **Esc** cancels.
  - Removed the old `factory_resolve_sweep` / `open_paths_of` / `selected_open_paths` and the
    resolve-based `factory_path_extrude` / `factory_path_cut`; the kernel build logic is now
    the shared `factory_build_sweep` (Union or Difference-relocated-after-target).
  - Guide shapes are kept (not consumed), so you draw freely, then designate each by clicking.
- **Verified:** 189 cad_app + 55 cad_solid tests (1 new: `path_sweep_build_extrudes_and_refuses_empty_cut`). Binary rebuilt.

## 12. Move/rotate stutter + FBX diagnostics

- **Symptom:** after the earlier fps fixes, moving/rotating a furniture still stuttered badly.
- **How found (FactoryPerf dump):** idle frames were fine (`rebuilt=false ~17 ms`), but each
  move/rotate produced a cluster of `buffer-rebuilt build 70 ms` (1 couch) → `200 ms` (3) — one
  full rebuild per drag frame.
- **Root cause:** a drag changes the instance pose every frame → `opaque_sig` changes → the
  ENTIRE opaque buffer (building + ALL furniture) re-transforms each frame.
- **Fix:** `opaque_verts` / `opaque_sig` / `furniture_verts` gained `skip: Option<usize>` that
  excludes the dragged instance from the cached buffer, so its per-frame pose change no longer
  invalidates the cache (static scene stays cached; only 2 rebuilds per drag). The dragged
  piece is drawn LIVE via `furniture_ghost_verts` (box proxy if heavy) into the overlay. Render
  site keys off `gizmo_drag`/`rot_drag` + `sel_furniture`. Test: moving the skipped piece keeps
  the cache Arc stable.
- **FBX import (partial — diagnostics):** user reports FBX "mutilated / not recognized / a
  cylinder". No cylinder fallback exists — it's a collapsed/mis-rotated parse. Known limits:
  ASCII unsupported, per-node transforms not applied, fixed Y-up→Z-up axis, units ignored.
  Added `parse_fbx_ex → (ObjMesh, FbxInfo)`; import now gives a clear message for ASCII / 0-tri
  files and logs `fbx_ascii/ver/geoms/verts/indices` in the `import-mesh` dump so the next dump
  identifies the exact failure. **Geometry fix deferred pending a sample .fbx** (exporter-specific).
- **Verified:** 189 cad_app tests. Binary rebuilt.

## 13. Drag keeps full form (GPU model matrix) + FBX still pending a sample

- **Symptom:** move/rotate was smooth but the furniture turned into a rectangular BLOCK during
  the drag (the box proxy from §12), snapping back to full form on release.
- **Root cause:** the drag ghost used the bounding-box proxy for heavy meshes; drawing the full
  mesh CPU-transformed per frame was the ~70 ms cost §12 avoided.
- **Fix:** draw the dragged piece from its LOCAL mesh via a GPU MODEL MATRIX — no per-frame CPU
  transform, full form. `Scene3dRenderer::render` gained a 2nd opaque pass (`dyn_verts`,
  `dyn_mvp`); app builds `furniture_local_mesh` (cached per drag in `factory_drag_mesh`) +
  `furniture_model_matrix`, and passes `mvp·model`. Still excluded from the cached buffer, so
  per-frame cost is one matmul + upload. Test asserts the matrix matches `furniture_point`
  (no jump on release).
- **FBX:** the move/rotate dump had NO import event, so still no FBX diagnostic data. Need the
  user to import a broken `.fbx` with the recorder ON (the `import-mesh` line now shows
  `fbx_ascii/ver/geoms/verts/indices`) or share the file. Geometry fix still pending that.
- **Verified:** 190 cad_app tests (1 new). Binary rebuilt.

## 14. FBX transforms (the real fix) + form-drag finally built

- **Form drag:** last turn's GPU model-matrix fix never compiled (build was blocked by the
  running app), so the user was still on the box-ghost binary. Rebuilt this turn — form is now
  preserved during move/rotate.
- **FBX "mutilated / cylindrical block" — root cause found by probing the user's real files:**
  Koltuk.fbx had 12 geometries but a ±1 bounding box — the parts are UNIT PRIMITIVES positioned
  by Model transforms the old parser ignored. Fix: rewrote `parse_fbx` as a full node-tree parse
  + interpreter that applies each geometry's connected Model transform (Lcl + Geometric
  T/R/S, composed up the parent chain via Connections) and honors UpAxis. After the fix Koltuk =
  200×165×75 cm (a real armchair), Table And Chairs = 108×51×109. Synthetic tests still pass;
  probe test `fbx_probe_real_files` (`#[ignore]`). Limits: XYZ rotation order only (no
  pre/post-rotation/pivots), ASCII still unsupported.
- **Verified:** 190 cad_app tests + real-file probe. Binary rebuilt (both fixes live).

## 15. FBX euler-order fix (couch) + persistent GPU scene buffer (idle lag)

- **FBX couch:** after transforms, Koltuk still had one backrest part standing up; Table &
  Chairs was fine. Root cause (via `RUSTCAD_FBX_DEBUG=1` per-model dump): Table & Chairs is all
  SINGLE-axis rotations (order-independent) so never tested the euler convention; Koltuk has
  MULTI-axis rotations where glam's intrinsic `EulerRot::XYZ` (Rx·Ry·Rz) is the reverse of FBX's
  `Rz·Ry·Rx`. Fixed `fbx_euler` to the FBX/three.js convention (explicit per-order axis product).
  Single-axis parts unaffected (Table stays correct). Also implemented the full FBX transform
  chain (`fbx_local_matrix`: T·Roff·Rp·Rpre·R·Rpost⁻¹·Rp⁻¹·Soff·Sp·S·Sp⁻¹).
- **Idle lag with 4 furniture:** the 406k-tri table made every idle frame re-upload 28 MB (the
  opaque buffer was re-sent to the GPU each frame). Fixed with dedicated version-gated static
  VBO+VAO slots in `Scene3dRenderer` (`render` now takes `scene_ver`/`dyn_ver`); a static heavy
  scene uploads once, a dragged mesh once per drag. Import of a 406k-tri mesh still hitches
  ~300 ms (parse + first build on main thread) — not yet threaded.
- **Verified:** 190 cad_app tests + FBX probe. Binary rebuilt (both fixes live).

## 16. Furniture is GPU-instanced — kills furniture perf for good

- **"What changed / lagging again":** nothing regressed. The user loaded a 2-MILLION-triangle
  furniture (`heaviest=2067216`, `upload=148 MB`, `build 1575 ms`) — 20× the couch. Every
  rebuild CPU-transformed 2M verts (~1.5 s in debug) on import + each move.
- **Root architecture problem:** furniture was folded into the CPU opaque buffer, so ANY
  furniture change re-transformed ALL furniture on the CPU.
- **Fix:** furniture is no longer in the opaque buffer. `opaque_verts()` is CSG-only (rebuilds
  only on a real geometry change). Each furniture is a GPU INSTANCE: the renderer keeps a
  `furn_bufs` map, uploads each mesh once (keyed by asset+colour), and draws it every frame with
  a model matrix — no CPU transform, no re-upload, any tri count. Import/move/rotate are now
  cheap regardless of mesh size. (Removed the old drag-exclusion + single dyn pass + APX box.)
- **Shading:** baked in the local frame (like the old drag ghost) — consistent, slightly
  flatter than world-lit. **Limits:** importing a 2M-tri file still parses ~1 s on the main
  thread + a one-time 148 MB upload; deleted/recoloured furniture leaves a stale GPU buffer.
- **Verified:** 190 cad_app tests (cache test now asserts furniture never rebuilds the opaque
  buffer). Binary rebuilt.

## 17. Path extrusion overhaul — two-sketch flow (section on face → perpendicular path)

- **Ask:** overhaul path extrude/cut. Flow: select a face, draw the cross-section, press Enter,
  the app asks WHICH perpendicular view to draw the path on (front-face → left/right etc.), draw
  the path, press Enter → the sweep is made.
- **Built:** `SweepFlow{stage: Section|ChooseView|Path, section_frame, section_loop, views, path_frame}`
  on `FactoryState`. Menu → `factory_begin_sweep_flow`. Enter/Finish → `factory_finish_sweep_stage`:
  Section captures the loop+frame and offers the two perpendicular planes (`sweep_path_planes`,
  each contains the face normal), an egui overlay lets the user pick (`factory_choose_sweep_view`),
  then Path is drawn and `factory_build_sweep(section_frame, section, path_frame, path, …)` sweeps.
  The path is re-expressed in section-local coords so its normal component is the extrusion depth;
  cut target = body under the path's world centroid. Section+path sketches are consumed after.
- **Decisions (asked):** offer BOTH perpendicular views; the PATH defines placement (section is
  the centred profile). Esc cancels; Enter finishes a stage while sketching.
- **Verified:** 190 cad_app tests. Binary rebuilt. Replaced the guided-pick flow.

## 18. Renderer — SSR, transmission, and a square-on face view

- **Symptom:** the pool water read as flat paint next to the same scene in Blender.
- **Root cause:** three missing things that compound — nothing reflected the surroundings,
  nothing refracted through a medium, and "transparent" was coverage rather than a material
  property.
- **Built:** `SsrSettings` (reflection marched in SCREEN space, sized over the visible part of the
  ray and stepped in **pixels not metres**, so step count doesn't depend on distance; a lost ray
  falls back to the sky). `RefractSettings` (scene colour + depth copy taken BEFORE the
  transparent pass, position/normal reconstructed from depth; transmission **tints** what's
  behind rather than uncovering it). Water as a MEDIUM vs glass as coverage, surviving the
  sidecar; old sidecars with `reflect = 0` migrate to the physical default instead of staying
  matte. `look_at_frame` puts the camera square-on to a picked face, keeping the side it was
  already on and leaving zoom alone.
- **Verified:** tests pin the regression-prone parts — march steps in pixels, scene copy precedes
  the transparent pass, SSR replaces rather than adds to the sky term, a medium lying on its
  container isn't drawn twice.

## 19. 2D plan — furniture outlines (FURN badge)

- **Ask:** see placed furniture's outline in 2D as a reference while drawing furniture.
- **Built:** `draw_furniture_outlines_2d` — each instance's cached local bbox posed through its
  own rotation/scale/position, projected to XY and **hulled** (so a rotated piece reads as a
  rotated rectangle, not an inflated axis-aligned one). Uses the same cheap 8-corner transform as
  `furniture_aabb`, so a 90k-tri import costs the same as a box. `FURN` badge beside
  CARD/SNAP/GRID/UCS, shown only while the Factory is open, default on. View-only — read at paint
  time like the lux overlay, no recompute.

## 20. Autosave was overwriting the drawing with an open face-sketch  ⚠ DATA LOSS

- **Symptom (found by audit, never reported):** open a saved .rsm, start a face sketch, wait
  three minutes → the file on disk is replaced by the sketch.
- **Root cause:** `factory_enter_sketch` parks the drawing in `session.saved_doc` and puts the
  sketch in `self.doc`. That swap is the fork's whole thesis — but `self.doc` is *also* what the
  persistence layer reads, and it never asked which document it held. Drawing in the sketch sets
  `unsaved`; `tick_autosave` had no session gate; `spawn_save_thread` cloned `self.doc`;
  `save_file_worker` wrote it atomically over the user's file. The atomic write did its job
  perfectly and committed the wrong document.
- **Fix:** `CadApp::plan_doc()` (session's parked drawing when a sketch is live, else `self.doc`),
  used by `spawn_save_thread`, `build_simlux_config_common` (was resolving SIMLUX layer names
  against the sketch's tables), `do_save_now`, and `light.rebuild_live_meshes` (in split mode it
  re-extruded the sketch's few lines as the room every frame). `apply_loaded` now commits a live
  sketch **before** `self.doc = doc` — otherwise the later exit restored the OLD drawing over the
  newly opened one and filed the new drawing into a sketch slot.
- **Verified:** `saving_mid_sketch_writes_the_plan_not_the_sketch` asserts the bytes written
  mid-sketch are byte-identical to the plan's.

## 21. Units — a drawing now declares its scale (Phase 1)

- **Symptom:** buildings come in wildly out of proportion; the 2D side is millimetres, the
  Factory is metres.
- **Root cause:** nothing converted. `make_3d_wall_from_selection` passed doc coordinates through
  verbatim and paired them with `wall_height` in metres — a 3000-unit (3 m) wall became a
  **3000-metre** wall 2.7 m tall. There was no unit concept anywhere: `Document` had no units
  field, and the DXF reader skips the HEADER wholesale so `$INSUNITS` is discarded.
- **Built (plumbing only, zero behaviour change):** `cad_kernel::DocUnits { metres_per_unit,
  source }` on `Document`, default 1 unit = 1 metre / `Assumed` — every boundary multiplies by
  1.0, so existing drawings behave exactly as before. `source` separates "the user said mm" from
  "nobody said". Conversion at the four doc→3D choke points: `cad_solid::geom_outlines_scaled`
  (scales in **f64 before the f32 cast** — a plan near X=3,619,000 mm has ~0.25 f32 spacing, so
  scaling late loses a quarter-mm a point), `cad_light::extrude` (was pairing mm X/Y with metre Z
  in one vertex — the work plane sat 0.8 **mm** off the floor), and the two hand-rolled casts
  reading `Wall::centerline_polyline`. `closed_loops_of`/`first_polyline_of` scale by each
  document's OWN unit, so one function serves both a mm plan and a metre-space face sketch.
- **Repairs two constants that were already wrong:** the `1e-3` closure test becomes a real 1 mm,
  and `factory_pick_ground_dobject`'s `0.3` becomes a real 0.3 m (it was metres judging drawing
  units, so line-picking from the 3D view never fired).
- **Backward compat:** RSM writes **v8 only when a unit is declared**; untouched drawings still
  write v7 and stay readable by older builds. Once a unit exists an older build *refuses* the file
  via the existing version check rather than loading it, dropping the unit, and writing back a v7
  file whose geometry no longer matches its scale. Refusing is recoverable; stripping isn't.
- **Trap fixed:** `WlThk` (0.20, "200mm") is a metre constant stored verbatim as a doc-unit
  length — converting it at promotion would have made drawn walls 0.2 mm while imported
  centerlines stayed 0.2 m. Now converted where it's authored; a test asserts the two agree.
- **New command:** `units` reports, `units mm|cm|m|in|ft` sets. Never moves geometry — only the
  interpretation at the boundaries changes — so it's safe to set and safe to set back.
- **Verified:** 924 tests across the workspace, incl. `a_millimetre_plan_promotes_to_metres`
  (3000 mm → ~3 m) and `without_a_unit_the_numbers_are_still_taken_as_metres` (zero-delta).

## 22. Units — the reverse direction (3D→2D), completing Phase 1

- **Symptom (would have been):** after §21, declaring `units mm` fixed promotion but broke every
  overlay that paints 3D data onto the plan — they'd draw 1000× too small, clustered near the
  origin. Caught before it shipped; the default (no unit) was never affected.
- **Built:** `w2s_m` / `s2w_m` — the metre-aware pair beside `w2s`/`s2w`. `w2s` maps DRAWING
  UNITS to screen, but the Factory and lux engine hold METRES, so anything crossing back must
  divide by the drawing's unit. Applied to all five painters: `paint_lux_overlay`,
  `paint_luminaires_2d`, `draw_factory_sketch_reference`, `draw_factory_sketches_2d`,
  `draw_furniture_outlines_2d`.
- **Two more 2D→3D crossings found while sweeping** (§21 missed them):
  - `paint_plan_underlay` flattened doc units and projected them into the **3D view's** world
    space — a mm plan would draw its reference kilometres from the model it sits under.
  - the sweeplight "grab a 2D curve as the path" branch fed doc units straight into a
    generator that builds in metres.
- **Swept clean:** `grep cad_solid::geom_outlines(` over `cad_app/src` now returns nothing —
  every call site goes through `geom_outlines_scaled` with an explicit unit. Sketch-space sites
  scale by the SKETCH document's own unit (k = 1 today), so the two spaces stay independent.
- **Verified:** 926 tests. `world_metres_project_onto_the_plan_at_the_drawings_scale` (3 m paints
  at 3000 units on a mm plan) and `the_plan_projection_round_trips_in_millimetres`
  (`w2s_m`/`s2w_m` are inverses).

## Open / not yet done

- **Transform gizmos — Phase 2 (scale + extrude by drag):** uniform corner handle +
  per-axis face handles; per-axis Z on an Extrusion drags its height (drag-to-extrude).
  Routes to `scale_selection` / `set_feature_primitive`. NOT started.

- **Cut:** confirm two-sided through on the real model (send the `in_depth`/`out_depth`
  dump line if a skin remains on one side).
- **DWG open:** needs an external converter installed + configured.
- **FBX:** ASCII variant + `UpAxis` override unsupported.
- **2D furniture blocks in 3D:** the plan's DXF furniture symbols are 2D-only; showing
  them in 3D (extrude footprints, or block→mesh mapping) is a separate feature — not
  started, awaiting a decision on approach.
- **Units Phases 2–3** (Phase 1 done, see §21 — these are what remain):
  - **DXF `$INSUNITS` on import.** `cad_io/src/dxf.rs` still skips the HEADER section
    wholesale, so an mm plan arrives untagged and you must type `units mm` yourself. Honour
    it only when the Factory is empty, so it can never contradict existing 3D work. On
    *export*, only assert a unit when `source != Assumed` (else write 0 / unitless) —
    otherwise a merely-assumed metre drawing gets stamped as a positive claim.
  - **Explicit "rescale the existing 3D model" action** for a project built before its unit
    was declared. Must NOT run at load time (a load must never mutate geometry), and must
    re-derive `surface_key` — the per-face colour/texture keys quantise a WORLD plane offset,
    so rescaling orphans every paint assignment. `paste_clipboard`'s `rekey` closure is the
    pattern to copy.
- **`GrdSpc` means two different things.** It is global (10.0) while `self.doc` alternates
  between a mm plan and a metre face sketch — so grid/snap is 10 mm on the plan and 10 METRES
  in a sketch. Discriminate on `factory.session.is_some()`, **not** on the unit value: a
  metre-declared plan and a face sketch are both `{1.0, Declared}` and cannot be told apart.
  Same class: `DimStyle::standard()`'s metre-shaped text/arrow sizes, and the dobject clipboard
  carrying geometry between the two spaces unconverted.
- **Lux grid clamp:** `cols = (w / cell_size).clamp(8, 64)` clamps the CELL COUNT, not the cell
  size, so any room wider than `cell_size × 64` silently gets a coarser grid than the UI states
  — which biases Emin/Eavg uniformity, the number EN 12464 compliance turns on.
