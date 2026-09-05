# Upstream Sync Audit — Session Summary (2026-08-31)

## 2026-09-03 update — both farzad-dev milestones pushed to origin

- `b280954` — layout tabs/paper-space viewports + command-line bottom
  strip ported from upstream working tree (`cc95970`): Model/Layout tab
  bar with per-space camera + layer-table swap, viewport windows
  (per-viewport camera/scale/CTB/lock), print-preview via shared
  `paint_cached_hatch` w/ per-viewport culling, resizable command strip
  with flyout close. Includes the code-review follow-ups
  (normalize_layers_for_save before writers, grip bounds guards,
  undo-aware viewport deletion, modal-in-layout gates).
- `cc57211` — ported the genuinely-new content of upstream
  `origin/farzad-dev` (windows-ui lineage, cc95970..fcf3079; its 12-commit
  range nets to app.rs only): coincident-grip drag moves every selected
  dobject sharing the dragged grip point (+ closed-4-vertex corner-scale
  the fork's drop loop lacked), erase sweeps orphaned `hatch_aux`
  boundaries of erased hatches, `find_smallest_containing_closed_scoped`
  now ignores invisible dobjects like the candidate collector, and the
  LAYER/LA/LAYERS command (opens Layer Properties Manager; Modify rail
  glyph = "Layer (LA)"; chlayer still reachable via Modify menu + CLI).
  3 regression tests added. Net diff (app.rs +304/−6) also included
  hatch-log designation/Apply markers — fork's own bake-flow logging
  already covers those, so they were not re-added.
- Verification: kernel 416/416, cad_app 1449 passed / 10 pre-existing
  failures (9 mesh_io fbx-texture assets + 1 factory wall env), pytest
  1473/1473.

## 2026-09-03 (later) — review fixes + backlog batch 1

- `ac351d4` — full code-review pass on the branch diff (12 findings, all
  fixed): stale grip `PolyVertex(i)` OOB after mid-drag undo (kernel
  `with_corner_scale` now guards `i < 4`; `restore_doc` clears
  grip/paper drags + re-syncs the canvas camera to the snapshot's active
  space); paper-space viewport move/resize/lock/copy + new/close layout
  now `snapshot_doc()` (unsaved/autosave/close-prompt/undo all work for
  layout-only sessions); closing a tab shifts/clears layout-bound viewport
  dialogs; paper render walks the spatial index per viewport + no
  unbudgeted uncached-hatch fill; viewport lock badge actually painted;
  model Plot gated to the Model tab; shared `Document::erase_dobjects`
  aux-sweep (single pass, O(N+refs)) used by erase / `delete <i>` /
  python `Op::Delete` / ghost preview; `find_smallest_containing_
  closed_scoped` delegates to the candidate scan; entity→sidecar mirrors
  call `Layout::sync_all_viewports`; dead `icon_for` arm removed.
  Kernel 416/416; cad_app 1449 + 10 pre-existing; pytest 1473/1473.
- `5904493` — uniform SPLINE RIBBON WIDTH ported (backlog): `Spline.width`
  + `with_width`, propagation through split/scaled/rotated/mirrored/
  reversed/translated/grip paths, trim preserves width, RSM write + read
  for both lineages (AutoRASM v35 gate; fork v200; v100 factory files gate
  at 7 → 0), plot scene flatten, spline-draw `w`/`width` sub-command
  (sticky), Inspector Spline Width row, `fmt_r`, kernel regression tests
  (width survives transforms, extend ellipse-arc, closed-shape errors).
- `c0bad5f` — backlog quick wins: OOPS restores the last erase (buffer in
  DeleteSelected arm; parser/CLI already had the command); File menu
  Plot… row restored + CTB Test Scene playground (3-viewport A3 layout);
  DDUNITS Drawing-Units dialog (bare `ddunits` intercepts pre-parser;
  `units` keeps the fork's own command) — kernel `Units` already had the
  formatting API.

## 2026-09-03 (evening) — backlog batch 2

- `fd3e35d` — TRIM/EXTEND fence + window/crossing crossing modes: the four
  upstream helpers (`apply_trim_fence` / `apply_trim_window` /
  `apply_extend_fence` / `apply_extend_window`) verbatim; target-phase
  sub-commands typed `f`/`fence` (two-click fence) and `w`/`c` (two-click
  window box); other typed input is swallowed mid-trim/extend (no command
  hijack); `fence_armed`/`fence_first` cleared on Enter/Esc/state reset.
- `0d01011` — QuadBézier (`qb`/`quadbezier`/`quadratic`) wired: previously
  the fork's SetTool arm sent ToolKind::QuadBezier into the POLYLINE tool;
  now it enters the Spline tool with `spline_degree_override = Some(2)`
  (P0..P2 clicks, Enter commits a quadratic B-spline); override cleared on
  any other tool switch, commit or state reset.
- `966a8ef` — File menu Export ▸ flyout (PDF / SVG / PNG quick export) via
  the existing Save-dialog → run_plot flow (dialog format wins).
- Suites after batch 2: kernel 419/419; cad_app suites green (hatch 40,
  command 43, layer 24, numeric-field 1/1, parity 3/3 — full run 1449 + 10
  pre-existing).
- Still open from the backlog (next sessions — all confirmed present in
  source cc95970; fork kernel parsers already carry most Command
  variants; the gaps are app-side): polar/path ARRAY, DIVIDE/MEASURE,
  QSELECT/QDIM, LAYISO/LAYFRZ/LAYOFF/LAYON + LAYWALK, AREA, XLINE/RAY/
  DONUT/CENTERMARK/WIPEOUT (note: end-to-end entity ports need the fork's
  draw/hit-test arms for these geom types — kernel types exist but the
  fork app renders none of Xline/Donut/Wipeout today), XREF/WBLOCK,
  ATTDEF/ATTEDIT, REGION/BOUNDARY, selection cycling, per-layout Plot
  dialog, Point Style picker (fork has the Inspector Point style rows but
  no `current_point_style` stamping or grid picker), plus the two
  architecture decisions (hover/designate hatch flow, rail-card dock
  columns).

## 2026-09-03 (evening, resumed on this machine) — backlog batch 3

Resumed from the other machine's session end (`3b77d70`, clean, pushed).
Its transcript was machine-local, so the handoff was this doc + the repo
state. Commits:

- `ec82238` — AREA + LAYISO/LAYFRZ/LAYOFF/LAYON. AREA: click a closed
  object → measured area+perimeter reported (kernel `measured_area` /
  `measured_perimeter`); empty clicks accumulate a point polygon
  (shoelace + closure perimeter), running total folds in via Enter,
  `a`/`s` (or add/sub/subtract) toggle add/subtract mode, Esc exits;
  pure inspection, never mutates the doc. LAYISO/LAYFRZ/LAYOFF: click a
  dobject, its layer is frozen-isolated / frozen / turned off (one undo
  entry per pick, no-op rolls back + reports); LayOn restores every
  hidden/frozen layer in ONE undo entry. Arms, Esc cancels, pointer
  click chain, gates (modal/snap/click-only), state dumps, and the
  dbg_recorder WatchedState fields all wired. 10 regression tests.
- `e8f8163` — DIVIDE + MEASURE. Pick a curve (or pickfirst) then type a
  whole segment COUNT ≥ 2 (divide) or a positive segment LENGTH
  (measure); marks are kernel `Point` dobjects (one undo entry).
  Ported the upstream sampler trio (`sample_path_points` /
  `point_at_arclen` / `geom_is_closed_loop`) plus a pure
  `divmeasure_positions_for`; divide places N marks around closed loops
  and N−1 interior marks on open curves; measure steps floor(len/dist)
  marks from the start. Bad values re-prompt; Esc / empty-Enter exits.
  5 regression tests.
- `8e5d49c` — selection cycling: pointer-mode Tab cycles the selection
  through stacked dobjects under the cursor (fresh spot = top candidate,
  repeat Tab advances; the next click at the cycle spot honors the
  highlighted candidate; any pointer click consumes the cycle state).
  Invisible dobjects are never candidates. 2 regression tests. ALSO
  fixes a latent purge bug found while testing: `commit_purge` left
  `layers.active` dangling when it purged the active layer, so every
  dobject failed `is_visible`/`is_selectable` until the user switched
  layers — active layer is now clamped back to layer 0 (1 regression
  test).

Verification after batch 3: `cargo check --workspace` clean;
`cargo test -p cad_app --bin simlux` 1477 passed / 0 failed / 50
ignored (was 1469 after batch 1 + 2 — i.e. 28 new tests all green);
pytest 1473/1473. Kernel untouched (416/416 suites unaffected).

Still open (unchanged, app-side only): QSELECT/QDIM, LAYWALK, Point
Style picker (current_point_style stamping + grid picker), XLINE/RAY/
DONUT/CENTERMARK/WIPEOUT end-to-end (kernel types + grips exist; the
fork's egui/GPU draw + creation flows needed), REGION/BOUNDARY,
XREF/WBLOCK, ATTDEF/ATTEDIT, polar/path ARRAY, per-layout Plot dialog,
plus the two architecture decisions. The next items are dialog/UI-heavy
or need draw-pipeline arms, so they were left for interactive sessions.

## 2026-09-05 (evening) — backlog batch 4 (this machine, fe736d3..17728c8)

- `fe736d3` — QDIM (batch aligned linear dims over selected segments) +
  QSELECT panel (type/layer/colour/linetype filters, include/exclude).
- `ce5cd54` — LAYWALK (layer isolation preview panel w/ filter; Apply keeps
  / Close restores the snapshot).
- `fb7732f` — PDMODE/PDSIZE point styles: `point_style_name` /
  `paint_point_style` / `point_half_px`; points now render their stamped
  style+size; Point tool stamps current style/size; typed `pdmode`/`pd`/
  `ddptype` opens the Point Style picker grid.
- `6ff282f` — XLINE / RAY / DONUT / WIPEOUT end-to-end: click flows in the
  pointer router (H/V/A[deg] typed sub-options for xline/ray), rendering
  (xline/ray clip to visible world bounds; donut filled ring; wipeout
  surface mask), kind names. Picking already worked via the kernel
  distance path.
- `6f3a432` — CENTERMARK click-to-place (circle/arc click sizes the mark
  ~18% of radius; `centermark N` override; crosshair render arm).
- `17728c8` — REGION (selected closed curves → filled Region dobjects on
  Enter via QueuedOp::Region) + BOUNDARY/BPOLY (hatch tracer emits outer
  loop + islands as closed polylines per inside-click).
- Full app suite at HEAD: 1476 passed / 10 pre-existing failures (9
  mesh_io fbx-texture asset tests + 1 factory wall-env) / 50 ignored;
  pytest 1473/1473. +21 regression tests this batch.
- Still open (app-side, confirmed in source cc95970): polar/path ARRAY,
  XREF/WBLOCK, ATTDEF/ATTEDIT, per-layout Plot dialog, plus the two
  architecture decisions (hover/designate hatch flow, rail-card dock
  columns). Kernel/parser surfaces for Array/Xref/AttDef/AttEdit already
  exist in the fork.

---

## 2026-09-05 — handoff state (batch 3, prior machine)

- `farzad-dev` at `b1fafc0`, worktree clean, pushed to origin (range
  `3b77d70..b1fafc0`, 5 commits: ec82238, e8f8163, 8e5d49c, 7a3346b
  [this doc], b1fafc0).
- `b1fafc0` — post-review fixup: DIVIDE mark output capped at 4000 (the
  MEASURE path already had the guard).
- Final verification at HEAD `b1fafc0`: `cargo test -p cad_app --bin
  simlux` 1477 passed / 0 failed / 50 ignored; `cargo check
  --workspace` clean; pytest 1473/1473; kernel untouched.
- Still open (app-side only — all confirmed present in upstream source;
  next machine picks the order): QSELECT/QDIM, LAYWALK, Point Style
  picker (current_point_style stamping + grid picker), XLINE/RAY/DONUT/
  CENTERMARK/WIPEOUT end-to-end (kernel types + grips exist; the fork's
  egui/GPU draw + creation flows needed), REGION/BOUNDARY, XREF/WBLOCK,
  ATTDEF/ATTEDIT, polar/path ARRAY, per-layout Plot dialog, plus the two
  architecture decisions (hover/designate hatch flow, rail-card dock
  columns).

---

Audit of what RUST-AutoRASM (upstream) changes are still missing from
3D-factory (SIMLUX) after the 2026-08-29 merge. Everything is on the
`farzad-dev` branch. Worktree clean, pushed to origin.

## Context: why things are missing

The fork and upstream share NO git history (fork root ≠ any upstream
commit). The 2026-08-29 merge (`e632d03`) was a file-level 3-way merge
with the fork's blob-matching base at upstream `92a5cdf` (2026-07-17).
Files were either:

- **Copied from upstream** → current (kernel modules, hatch_resolve.rs,
  patterns.rs, style.rs, hatch_trace.rs, cad_text/cad_plot/cad_script) — OR
- **Kept as the fork's version, adapted only to compile** → STALE.
  The merge's app.rs restored the FORK's app.rs (a 3-way merge produced
  structurally-broken chimeras) and adapted it to the merged kernel
  (Units rename, Geom/DimKind exhaustive arms, Command catch-all). All
  upstream app-level and kernel-level FEATURE/FIX work done after the
  fork's base (~2026-07-17) is MISSING from these files.

Upstream snapshot used for comparison: `03ec3ff` (2026-08-27, last
commit before the fork merge). Upstream HEAD at audit time: `b47dedc`.
The delta `03ec3ff..b47dedc` (2 commits: pytest suite + CLI/script
fixes) was already ported in `8690401`.

## Already done this session (commit 8690401, pushed)

Ported the entire upstream delta `03ec3ff..b47dedc`:

- `cad_cli/src/main.rs` — `stamp_fresh_style` on parser/script entity
  adds (active layer + current color/linetype/lineweight); loud error
  for all-invalid `rasm.copy` indices (3-way applied, fork's SIMLUX
  command-arms hunk preserved).
- `cad_script/src/rasm.rs` — `#[pyfunction(name = "blocks")]` + scale
  factor > 0 ValueError (applied cleanly; now byte-identical upstream).
- `tests/` — full black-box pytest suite for the headless cad_cli
  (29 files, ~4600 lines; run via `./tests/run_tests.sh`).
- `scripts/` — 7 demo scripts incl. rewritten `modify_demo.py`.
- `.gitignore` — Python test-suite artifact entries.
- Verified: `cargo check --workspace` passes (pre-existing warnings only).

## CONFIRMED MISSING — fix list (in suggested order)

### 1. `cad_kernel/src/snap.rs` — panic fix (small, do first)
Missing `d93de487`: stale spatial-grid candidate indices (built before
the doc shrank — delete-then-hover) index past the current slice →
OOB PANIC in `find_all_snaps`/`find_snap`. Fix: retain indices
`< dobjects.len()` in the grid-match loop. Upstream added
`stale_grid_indices_do_not_panic` test.

### 2. `cad_kernel/src/intersect.rs` — scale-relative tolerances
Missing `8e3120ac`: `scaled_tol(r)` for circle-circle tangent gates
(1e-6·r); circle∩ellipse newton accept threshold scaled by char²
(1e-6·char²); ellipse-implicit form stays fixed 1e-6 (dimensionless).
Large-coordinate geometries currently rejected/dropped.

### 3. `cad_kernel/src/fillet.rs` + `cad_kernel/src/modify.rs` — fillet/chamfer
Missing `8e3120ac` (G4: normalize directions before parallel test —
`denom` scaled with |d0|·|d1| at large segment lengths, fixed 1e-12
drops non-parallel pairs), `7078fe03` (line pick decides the corner),
`4e6b14cd` (the ARC's own pick decides the corner), `e8d740ae` (keep
BOTH clicked sides, equal weight, no arc ballooning), `5cac57a9`
(chamfer: `bridge: Option<Geom>` — None for zero-length bridge at
d=0; `g1_new`/`g2_new` still trim to sharp corner). Fork has
`bridge: Geom` (always emits a zero-length line).

### 4. `cad_kernel/src/geom.rs` — mirror reflection + wall extend
- Mirror: reflected EllipseArc must NEGATE start/sweep params
  (`-ea.start_param, -ea.sweep_param`) — reflection flips winding,
  un-negated = wrong quadrant. Polyline mirror must negate per-vertex
  bulge (`bulge: -v.bulge`). Fork has the un-fixed versions.
- `Geom::Wall` "infinite form" arm missing from the infinite-form
  helper: wall TARGET / edge-mode wall boundary fails the
  `hits.is_empty()` guard → "extend a wall to a boundary" silently
  no-ops. Add centerline lengthened by `EXT` both ways.

### 5. `cad_kernel/src/document.rs` — push behavior + layer draw-order
- `126a4afb`: upstream made `Document::push` a PURE APPEND — active-
  layer + current-specs stamping moved to the app layer's
  `stamp_fresh_style`. Fork's `push` still inherits the active layer
  (style-signature heuristic) → default-styled COPIES get re-coloured
  (copy/paste/array/mirror/explode/duplicate all flow through push).
- `a4590e6a`: layer draw-order reorder (#35) missing.

### 6. Hatch fixes — `cad_app/src/app.rs` (biggest single task)
The fork's app.rs hatch code matches upstream ~`869464a` (2026-07-13),
BEFORE the kernel resolver promotion and all Aug fixes. Kernel-side
(hatch_resolve.rs, patterns.rs, style.rs hatch_aux, hatch_trace.rs,
rsm.rs v34 hatch_aux) IS present — the app just doesn't use it.
Missing:
- `0edbd64` — pattern lines through boundary vertices vanish (dedupe
  hit lists before even-odd pairing)
- `48c83b1` — pick-point hatch over a circle divided by a line
  (`outer_has_crossings_with_others_scoped` + endpoint/midpoint dip
  detection + bbox fast path); fork has the old unscoped version
- `05b5152` — even-odd fill across multiple boundaries
- `e14d6ed7` — hover preview shows the region a click would ADD
  (`render_hatch_live_preview`, `hatch_preview_traced_loops`,
  `render_hatch_hover_highlight`)
- `404416c8` — GPU/plot pattern fill shares the even-odd clipper
- `6b6b165a` — cancel/supersede/panic can't materialize a stale trace
  (`cancel_hatch_worker` + drop receiver)
- `d084fce0` — rotate/scale/mirror carry the hatch's boundary (fork
  has NO boundary-carry on transform)
- `0e8a8099` — invalidate the fill cache on geometry, not selection
- `633a3ee9` / `4aebb7b6` / `86ba2ded` — boundary highlight under
  cursor, inspector-style hatch panel, decoupled boundary + live
  command-bar prompt
- Core swap: fork `resolve_hatch_loops` (app.rs:19719, own copy) →
  delegate to `cad_kernel::resolve_hatch_loops` (gains spline
  boundaries, invisible/frozen-layer gating issue #17, hatch_aux
  exception).

### 7. GUI commands "recognised but not available in this build"
12 upstream commands parse fine but hit the fork's GUI catch-all
(app.rs:19245). Kernel modules EXIST; CLI handles them all; only the
GUI wiring is missing. Each is a small arm → existing kernel/CLI logic:
`RevCloud`, `Area`, `Overkill`, `Purge`, `QSelect`, `Ucs`, `Table`,
`Xref`, `LayerState`, `WBlock`, `Boundary`, `WallCleanup`.

### 8. Fresh-style stamping gap in the GUI
Fork's `add_dobject` (app.rs:42507) relies on `push`'s layer-only
inheritance; `stamp_fresh_style` (current color/linetype/lineweight +
active layer) is never called in the fork GUI — new entities ignore
`current_color`/`current_linetype`/`current_lineweight` even though
the fields exist. Port upstream's `stamp_fresh_style` and call it from
`add_dobject` + hatch commit (paired with item 5's push change).

## Checked and NOT issues (intentional fork differences)

- `cad_io/src/dxf.rs` — fork's zero-copy &str parser (documented merge
  superset; upstream borrows Strings).
- `cad_io/src/rsm.rs` — fork's VERSION 200 renumbering (has hatch_aux,
  reads v1-99/v100/v200).
- `cad_kernel/src/units.rs`, `parser.rs` — fork supersets (UnitSource,
  SIMLUX commands).
- `cad_kernel/src/spatial.rs` — fork's own improved impl (SKIP cells,
  cell_range, per-dobject ranges, insert_appended).
- `cad_app/src/calc.rs` — fork additions (looks_like_expr_token etc.).
- `cad_app/src/dock.rs` — fork additions (Top edge, any_edge docking).
- `cad_app/src/main.rs` — fork's own (better) panic hook.
- `cad_app/src/dbg_recorder.rs` — fork + PromptChange/ZoomChange.
- Cargo.tomls — fork deps (serde sidecar, acadrust DWG, simlux name).
- `cad_cli/src/main.rs` — ported (8690401); matches upstream HEAD +
  fork's SIMLUX arms.

## Method (how this list was produced)

1. Hash-compare every shared `.rs`/`.toml` file at fork HEAD vs
   upstream `03ec3ff` → 30 fork-adapted files.
2. For each adapted file: walk upstream commits touching it (up to
   03ec3ff), extract each commit's added lines, check presence in the
   fork's file → commits with <35% of added lines present are missing.
3. Manually verify each flag against the actual diffs (excluded
   intentional fork adaptations, false positives).

## Remaining work (next session)

- Fix items 1-8 in order above; each is independent, one commit each.
- After items 5+8: re-run `cargo test -p cad_kernel` and the pytest
  suite (`./tests/run_tests.sh`).
- Consider the 12 GUI commands as follow-ups; each reuses the existing
  kernel module + CLI logic.

## Continuation session (2026-09-01) — what got fixed

Resumed on this machine; upstream at `/home/farzad/workspace/Rust/RUST-AutoRASM`
(HEAD `b47dedc`). Items 1-5 and 8 are DONE; item 6 done except the four
UI-panel items; item 7 done for 4 of 12 commands. All verified.

- **1. snap.rs** — stale-grid candidate filter (`cand_ents.retain(< len)`) +
  `stale_grid_indices_do_not_panic` test. (trim.rs G3 guard was already in.)
- **2. intersect.rs** — G2 circle-circle gates use `scaled_tol(r.max)`; line-circle
  migrated to `scaled_tol`; 4 scale-invariance tests. (G1/ellipse-ellipse already
  in, fork even has the G7 dedup superset.)
- **3. fillet/chamfer** — G4 `line_line` direction normalization + scale test;
  `ChamferOut.bridge: Option<Geom>` (None at d1=d2=0) in modify.rs + fillet.rs +
  both app.rs chamfer commits + sandbox; 2 new bridge tests. (Corner-pick rules
  already matched upstream HEAD; layer draw-order a4590e6a was already in.)
- **4. geom.rs** — `mirrored` negates EllipseArc start/sweep params + polyline
  per-vertex bulge. (Wall infinite-form arm was already in.)
- **5. document.rs** — `push` is a PURE APPEND; kernel tests updated; demo
  objects and sketch/locked-layer tests now stamp or set layers explicitly.
- **6. hatch (app.rs)** — `resolve_hatch_loops` delegates to
  `cad_kernel::resolve_hatch_loops`; `dedupe_hits` + vertex-hit regression;
  `outer_has_crossings_with_others_scoped` (dip detection + bbox fast path);
  all 4 pattern-clip sites (families/tile/GPU/tile-GPU) use the kernel
  `hatch_line_intervals` (per-loop pairing + XOR); `cancel_hatch_worker`
  single-sourced (Esc, new command, spawn-replace) + worker panic→Failure;
  `hatch_cache` invalidates in `ensure_index` (geometry, not selection);
  `transform_targets_with_hatch_boundaries` wired into move/rotate/scale/
  mirror in-place paths + 5 tests. NOT ported (deferred, feature-scale):
  hover preview (e14d6ed7), boundary highlight (633a3ee9), inspector panel +
  hatch_boundary accumulate (4aebb7b6), decoupled prompt (86ba2ded) — the
  fork's pick-point flow commits one hatch per click with its own confirm
  panel, so these need the accumulate infrastructure ported as a feature.
- **7. GUI commands** — Purge, Overkill (with QueuedOp + selection flow),
  LayerState, Ucs (incl. current_ucs/ucs_to_world/world_to_ucs/ucs_name)
  are wired with tests. NOT ported (deferred): RevCloud, Area, QSelect,
  Table, Xref, WBlock, Boundary, WallCleanup — each needs its GUI state
  machine + click/prompt wiring (note: the CLI ignores these too; the
  upstream app.rs arms are the reference).
- **8. stamp_fresh_style** — added to app.rs and called from `add_dobject`,
  the hatch commit, `script_commit_hatch`, and the launch demo objects
  (paired with item 5's pure push).

### Test infrastructure fix

`docs/scripting_api.md` (458 lines) was missing from the fork since the
merge — the ported pytest suite's `pyhelp`/`rasmhelp` and script tests
failed on it (1149 failures). Restored from fork commit `71be37e`.
`tests/test_rasm_files.py` asserted RSM version 34; the fork writes the
renumbered VERSION 200 (merge decision) — assertion updated.

### Verification (all green)

- `cargo check --workspace` — 0 errors.
- `cargo test -p cad_kernel` — 414 passed (was 406; +8 new).
- `cargo test -p cad_app --bin simlux` — 1402 passed, 10 failed, 50 ignored;
  the 10 are pre-existing on this machine (9× mesh_io/fbx + 1 wall-thickness
  env `WlThk=5` issue, both documented in the 2026-08-29 summary).
- `./tests/run_tests.sh` — 1473 passed, 0 failed.
- Uncommitted: 9 modified files + restored `docs/scripting_api.md`.

### Second continuation (2026-09-01, pm) — upstream hatch fixes imported

Upstream advanced to HEAD `3d845ac` with 4 hatch commits since `b47dedc`; all
ported (only app.rs + hatch_trace.rs touched; kernel unchanged):

- **`0e5db2d`** — invisible boundaries are not pick-point candidates:
  `collect_closed_containing_scoped` skips `!doc.is_visible(i)` (kills the
  "2+ candidates → PARTIAL OVERLAP → slow trace → more hidden boundaries"
  pollution loop); verdict logger mirrors the guard.
- **`849ae94`** — boundary is always independent: `apply_hatch` now binds via
  `materialise_hatch_boundary()` (bakes the picked shape into an invisible
  `hatch_aux` polyline), so every path matches the trace path — hatching a
  rectangle no longer bonds the fill to the drawn shape (owner ruling).
  Plus post-creation instrumentation: `hatch_dbg_selection` (in
  `click_select`) + `hatch_dbg_command_on_selection` (in `run_command_inner`)
  log every selection/command touching a hatch or its boundary.
- **`3d845ac`** — selecting a hatch exposes draggable grips:
  `editable_grip_targets()` substitutes the selected hatch's aux boundary into
  the grip-target set (grab loop now uses it); grip-drag logs which hatch an
  aux-boundary drag reshapes. Locked-layer protection preserved.
- **`c2ec9fe`** — process logging across the workflow: `hatch_dbg` ALWAYS
  records (window is just a viewer); hatch_trace.rs copied wholesale (TraceDiag
  per-stage counts/timings, typed TraceFail reasons, GapProbe at 10x/100x/1000x
  JOIN_EPS — all additive; 18/18 tests pass); worker now calls
  `trace_boundary_at_in_view_diag` and reports the typed failure + gap probe;
  phase banners 1-10 via `hatch_phase`/`hatch_phase_absent` (Phase 1 in
  `hatch_dbg_session_start`, Phase 2 pattern validation incl. UNKNOWN-pattern
  and scale<=0 warnings, Phases 4/5/7 via `hatch_verify_last`); "💾 Save
  Report" button writes `hatch_report.txt` (env + state + full log).

Tests: 4 new (independent-boundary bake, grip exposure, aux-resolve regression
lock earlier, plus the 3 diag tests inside hatch_trace.rs). Verification:
`cargo check --workspace` clean; app 1409 passed / 10 pre-existing failures
(mesh_io + env wall-thickness); kernel 414/414; hatch suites 33+18; pytest
1473/1473.

### Third continuation (2026-09-02) — text glyph fixes + hatch letters-as-shapes

Upstream advanced to HEAD `cc95970` with 2 commits (hatch/text); ported:

- **`ef5b8c1` text: TTF decode fix, per-entity font/bold/italic, TXTEXP**:
  `cad_text` font.rs/lib.rs/render.rs copied wholesale (identical at base);
  kernel text.rs copied (adds `Text::resolved_font_name`, the entity→style→
  standard chain single source of truth); snap.rs gained the
  `text_snaps_at_anchor` test (anchor=END/CEN/NEA/PER, no MID/QUA).
  App: Text Style dialog font list now engine-backed (`borrow_mut().names()`,
  type-ahead + ScrollArea — ported the section the fork still had on egui
  builtins only); ITALIC_RAD hoisted to module const; text entity dialog
  Variant dropdown (Regular/Bold/Italic/Bold Italic) replacing the STUB;
  Properties panel Text section gains Font combo ((inherit) resolution via
  `resolve_style_font`), Bold + Italic checkboxes (undo-safe props_apply);
  TXTEXP arm in `apply_explode` — Text explodes into closed glyph-outline
  polylines (outer + holes as separate loops); draw path routes per-entity
  font through `resolve_text_font` (V1 egui path keeps standard/monospace
  mapping but the resolution chain matches the engine/TXTEXP/hatch).
- **`cc95970` hatch: glyph contours are boundary shapes**: hatch_trace.rs
  copied wholesale (adds `TextTraceGeom`, `text_hatch_geom(doc, fm, scope)`,
  render_explode plumbing; 22/22 tests incl. letters-as-islands, click-inside-
  letter, real-font boundaries); app.rs gains `outer_overlaps_text[_scoped]`,
  `resolve_style_font`, `resolve_text_font`, `text_hatch_geom`,
  `text_hatch_loops_at`; pick-point routing defers single-outer+text-overlap
  to the trace path (comment + `single_outer_overlaps_text`); worker snapshot
  clones text_styles and renders glyph geometry on the main thread into
  `text_for_thread` (passed to `trace_boundary_at_in_view_diag`); worker
  Failure fallback gains TEXT PATH (smallest glyph loop containing the seed);
  refactor: `hatch_bake_loops` + `commit_baked_hatch` helpers extract the
  Success arm's bake+commit+confirm-panel code (reused by the text fallback);
  CPU solid fill (`render_hatch_solid`) now depth-sorted even-odd painting
  (islands-in-holes re-fill; replaces the earlier containment-parity pass —
  the fork GPU cache path keeps `solid_loop_is_fill` since fills/holes lists
  are order-independent).
- Fork-absent features (hatch hover preview, hatch-region TRIM text cutters,
  BOUNDARY-command glyph loops) — no fork equivalent to port; deferred list
  unchanged.

Tests: kernel 416 (snap text anchor + one more); app 1413 passed / 10
pre-existing failures; hatch suites 37 + hatch_trace 22; cad_text 29; cad_io
93; pytest 1473/1473; `cargo test --workspace --no-fail-fast` green except the
10 known. Changes uncommitted.

### Fourth continuation (2026-09-02) — python scripting / plot parity + commit

9446058 committed + pushed to origin/farzad-dev (all hatch/text/audit work).

Then full python-scripting parity port from upstream (9122825-era UI slices
the fork's merge had dropped):
- state: 13 fields (py_editor_*, script_param_dialog/pick, script_preview,
  script_pending_run, scripting_doc_open/text, script_meta_finish_pending) +
  5 module types (PyEditorConfirm, ScriptPickKind, ScriptParamDialog,
  PendingScriptRun, ScriptPreview) + inits.
- fns: run_script_command/parse_named_script_args/fill_catalog_choices/
  run_pending_script, on_script_meta, render_script_param_dialog (typed
  fields incl length/point-pick/entity/color/choice catalogs),
  param helpers (parse_point_param_value, param_display_default,
  param_value_scene, dialog_params_scene), open/render_scripting_doc,
  save_py_script (upstream), py_editor_* + render_py_editor +
  py_editor_toolbar, preview trio (script_preview_start/dirty/
  clear_script_preview), apply_preview_op + preview_transform/preview_style
  (shadow-document ghost pass; module-level in fork → direct calls, no
  Self::), on_script_finished rewritten (meta-finish guard + preview
  finalize), poll_script_engine Meta arm, apply_script_op preview guard.
- wiring: console Editor/Guide buttons; clear_script_preview on pyfile/
  submit/example-run/run paths; dispatch → run_script_command /
  open_scripting_doc / refocus_cmd; Esc interrupts script + disarms param
  pick; canvas-click fills point/entity param pick; ghost dashed overlay in
  draw path; AciPickRequest::ScriptParam + title/apply arms;
  Tools→Scripts flyout (FlyMenu::Scripts, 4 FlyAct variants + handlers +
  activate arms); update() renders py editor + param dialog + scripting
  doc. Tests: 10 upstream script test modules ported (script_hatch_tests
  dropped — tests fork-absent hover/designate machinery); new DragValues
  carry update_while_editing(false) (fork's numeric-field regression test).
- Plotting: verified byte-identical shared plot UI (dialog/preview/CTB/
  ladder/plot_config) — cad_plot scene.rs diff belongs to the deferred
  kernel Spline.width feature; layout_print_preview belongs to the
  not-ported layout-tabs feature. Nothing to port.
Verification: app 1446 passed / 10 pre-existing failures; kernel 416;
pytest 1473/1473. Changes committed + pushed.
