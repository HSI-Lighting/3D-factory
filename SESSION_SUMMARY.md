# Upstream Sync — Session Summary (2026-08-29)

Merge of RUST-AutoRASM (upstream) changes into 3D-factory (SIMLUX), per the
session's three decisions. Everything below is on the `farzad-dev` branch.

## Decisions made this session (your answers)

1. **Scope** — Everything: shared files AND the new crates
   (`cad_text`, `cad_plot`, `cad_script`) + all new kernel modules.
2. **Uncommitted work** — committed as a checkpoint first
   (`eb61419 WIP checkpoint before upstream sync`).
3. **app.rs** — 3-way merge was attempted; the two files had diverged so far
   (68k vs 63k lines, 228 conflict hunks, 100k conflict lines) that the
   merge produced structurally-broken chimeras (a 2488-line `run_command_inner`
   collapsed to 348 lines, duplicated/misplaced function bodies). **Restored
   the fork's app.rs and adapted it to the merged kernel instead** — the
   original's app-level UI work (text/plot/script dialogs) is NOT ported.
4. **Units** — unify into ONE type. Unified `cad_kernel::Units` serves both
   the 3D side (`metres_per_unit`) and the plot side (`name`, `scene_per_unit`,
   display formats). Default = **1 unit = 1 mm** (the original's convention),
   `UnitSource::Assumed`. Your 3D code reads `doc.units.metres_per_unit`
   (now 0.001 default instead of 1.0 — intentional behavior change).
5. **RSM format** — fresh renumbering: `VERSION = 200`. Reader accepts
   v1-99 (RUST-AutoRASM lineage), v100 (3D-Factory lineage), and v200
   (merged). Writer emits 200 with the unified units block.
6. **DWG converter** — kept the fork's accoreconsole-based `dwgconv.cmd`;
   the original's .NET converter was NOT copied.

## What was merged

### Kernel (cad_kernel)
- All original-only modules copied: `layout`, `ucs`, `mtext`, `xref`, `purge`,
  `dedupe`, `hatch_resolve`, `laystate`, `pagesetup`, `plotstyle`, `table`,
  `units`, `vector_primitive`.
- 3-way merged shared files: math (scale-aware newton, JOIN_MITER_LIMIT,
  scaled_tol, circular_union moved here), snap, fillet, dobject, intersect,
  trim (original's superset incl. G3 clamp guards + full test suite),
  spatial (fork's impl + original's `world_bounds`/`build_with`/
  `auto_cell_size_with`), parser (both command sets), lib.rs.
- New Geom variants from upstream: Table, Xref, Xline, Donut, Wipeout,
  Region, Ray, CenterMark, Leader. New DimKind variants: Angular, ArcLen,
  Ordinate, Jogged. New TextStyle/Text fields (bold, outline, underline,
  list_mode, line_spacing...). BlockRef gained `attr_values` (now non-Copy).
- **Unified `Units`** (`cad_kernel/src/units.rs`): merged `DocUnits` +
  upstream `Units`. Fields: `name`, `scene_per_unit`, `metres_per_unit`,
  `source`, length/angle formats. Invariant:
  `metres_per_unit = mm_per_named(name)/1000/scene_per_unit`.
  Default: `{ name: "mm", scene_per_unit: 1.0, metres_per_unit: 0.001,
  source: Assumed }`. Constructors: `Units::new(name, scene_per_unit)` and
  `Units::from_metres_per_unit(m, source)`.

### IO (cad_io)
- **rsm.rs**: took the original's v34 file as base, renumbered to VERSION=200,
  unified units block, reader branches on lineage (v1-99 / v100 / v200).
  `read_factory_units` handles the 3D-Factory trailer.
- **dxf.rs**: original's feature superset (hatch satellites, MTEXT/DIMENSION/
  SPLINE/LEADER entities, AC1015 writer) + the fork's zero-copy `&str`
  pair parser + `$INSUNITS` header read/write (declared units only) + OCS
  extrusion handling (mirror −Z flips x for object-coord entities, reverses
  arc sweeps, flips polyline bulges; LINE/ELLIPSE/HATCH left alone).
- dwg.rs adapted to merged kernel (Units, Layer.order, BlockRef.attr_values).
- `plot_table` module copied (for cad_plot).

### New crates
- `cad_text`, `cad_plot`, `cad_script` copied wholesale from upstream;
  wired into workspace + cad_app. cad_script links CPython (pyo3 abi3-py311).

### App (cad_app) — fork's app.rs adapted, original's UI NOT ported
- `Units` rename: `DocUnits::new(k, src)` → `Units::from_metres_per_unit`,
  `length_str`/`length_decimals`/`length_ui`/`num` take `&Units`,
  `parse_at_coords` takes `&Units`.
- Geom/DimKind/ToolKind/Command exhaustive matches: added `_` arms +
  mapped new ToolKinds (Polygon/QuadBezier→Polyline, Leader/AttrDef→Text...).
- New Command variants (Python, Script, Plot, PageSetup, Ucs, Xref, etc.)
  hit a catch-all "recognised but not available in this build" arm.
- dock.rs (theirs), settings.rs (theirs), gpu.rs (theirs, incl. `pad` arg +
  LineInstance.flags), dbg_recorder.rs (fork + PromptChange/ZoomChange),
  main.rs (+layer_glyphs), calc.rs (merged), cli (theirs + new-command arms).
- **Sketch docs are now forced metre-space** in `factory_enter_sketch`
  (1 unit = 1 m, Assumed) — required by the new mm default, otherwise
  face-sketch coordinates would read as mm and cuts would fail.

## Tests

- `cargo check --workspace --offline` — PASSES (161 warnings, mostly pre-existing).
- `cargo test -p cad_kernel --offline` — 406 tests PASS.
- `cargo test -p cad_app --offline --bin simlux` — **1369 passed, 18 failed,
  50 ignored** (was 1370/14 at fork HEAD).

### Remaining failures (18) — mostly NOT merge-caused
- 7× `mesh_io::fbx_textures/*` + `mesh_io::tests/*` — fail at fork HEAD too
  (pre-existing on this machine).
- 4× `the_dwg_converter_*` — `.cmd` scripts need Windows; environment issue.
- `drawn_and_imported_walls_agree_on_thickness_in_millimetres`,
  `a_wall_keeps_its_own_thickness` — fail at HEAD too: `~/.config/rust_cad/
  user_env.txt` on this machine sets `WlThk = 5`, so the env-loaded test value
  differs from the code default 0.2. Clean machine / reset config → passes.
- `the_cull_never_drops_what_you_can_see::a_small_view_keeps_only_what_reaches_it`
  — units-default change (test assumed 1 unit = 1 m plan scaling). Needs a
  test update: the 400-unit plan is now 0.4 m, so the 40 m view culls nothing.
- `the_line_cache_cannot_go_stale::redeclaring_the_unit_invalidates_it`
  — same units-default cause; needs test update.
- `promote_tests::an_arc_promotes_as_a_sampled_curve` — likely units-default;
  needs test update.

### Tests updated for the new mm default (now passing)
- `without_a_unit_the_numbers_are_still_taken_as_metres` →
  `..._as_millimetres` (3 units = 3 mm).
- `an_in_memory_document_keeps_assuming_metres` → `..._millimetres`.
- `declaring_a_unit_with_rescale_...` — declares metres first, then rescales.
- `extrude_works_on_a_2d_selection`, `world_metres_project_onto_the_plan_...`
  — declare `units m` explicitly.
- 3× curved-wall cut tests + `an_opening_whose_centre_misses...` — sketch
  docs declared metre-space (matches new invariant).

## Remaining work (for the next session)
1. Port the original's app-level UI features into the fork's app.rs if wanted:
   text-engine dialog (cad_text), plot dialog (cad_plot), script console
   (cad_script) — currently only available at the library level.
2. Update the 2-3 units-default tests listed above (cull, line-cache,
   arc-promote) to the mm default.
3. Decide what to do with the pre-existing failures (mesh_io, dwg .cmd tests)
   — they fail on this Linux box regardless of the merge.
4. `cargo test -p cad_plot`, `-p cad_text`, `-p cad_script` — new crates not
   yet test-verified on this machine (they compile via workspace check).
5. Cargo.lock was updated by the workspace build; review the final diff before
   any release build.

## Files touched (74)
Kernel (merged): lib, math, snap, fillet, dobject, intersect, trim, spatial,
parser, document, units(+new), layer, geom, + 12 new modules.
IO: rsm (v200), dxf (zero-copy+INSUNITS+OCS), dwg, lib, plot_table(+new).
App: app.rs (adapted), factory, gpu, dock, settings, dbg_recorder, main,
theme, calc, illuminaire, mesh_io (tests), layer_glyphs(+new).
New crates: cad_text/, cad_plot/, cad_script/.
Config: Cargo.toml (workspace members), cad_app/Cargo.toml (3 new deps),
cad_io/Cargo.toml (+serde_json), .cargo/config.toml (merged env sections).

## Note on the merge method
The two repos share NO git history (fork root ≠ any upstream commit; blob
matches put the fork at upstream ~92a5cdf, 2026-07-17). All merges were
file-level 3-way merges with that base. The `upstream` remote points at the
original repo path and its `farzad-dev` branch was fetched for reference.
