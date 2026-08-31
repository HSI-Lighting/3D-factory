# Upstream Sync Audit — Session Summary (2026-08-31)

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
