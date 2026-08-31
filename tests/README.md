# Python Test Suite for the headless `cad_cli`

A pytest-based, black-box test suite that drives the headless `cad_cli`
binary (the same command parser, kernel, and rasm scripting engine the GUI
uses), checks the resulting scene (dobject dump, intersections, rasm
json read-back, saved files), and reports every problem found.

## Running

```bash
./tests/run_tests.sh            # build + venv + pytest + report
./tests/run_tests.sh --rebuild  # force `cargo build -p cad_cli` first
```

Requires a Rust toolchain (only for the initial `cargo build -p cad_cli`).
The suite writes `test-results.xml` (junit) at the repo root and the
human-readable report to `tests/report/report.md` +
`tests/report/failures.txt`.

## Layout

| file | purpose |
|---|---|
| `cadcli.py` | subprocess client, scene-dump/reply parsers (single point of change for `describe()` formats) |
| `geommath.py` | independent Python ports of the kernel's arc/intersection math (expected values) |
| `conftest.py` | binary resolution (`$CAD_CLI_BIN` → debug → release → auto-build), per-test fresh `CadCli` |
| `rsm_reader.py` | minimal RSM binary reader (magic/version/tables/dobject count) |
| `test_harness.py` | smoke: binary runs, empty scene, help/list, no panics |
| `test_parser_commands.py` | keywords, aliases, arity, bad tokens, interactive-only replies |
| `test_geometry_*.py` | line/circle/arc/misc/intersections parameter grids |
| `test_modify.py` | `del` / `clear` / `list` |
| `test_rasm_*.py` | the rasm scripting surface: draw/modify/layers/hatch/files/doc |
| `test_scripts.py` | every script in `scripts/` (run forms, params, headless behaviors, idempotency) |
| `test_sequences.py` | ordered pairs (21×21) + length-3 chains with a state simulation |
| `test_fuzz.py` | seeded random rounds (CLI lines + rasm ops) |
| `make_report.py` | junit XML → `report.md` / `failures.txt` |

## Conventions

- **Isolation**: one `CadCli.run(...)` = one fresh subprocess = one fresh
  document. Read-backs that must see the script's state are batched into the
  same process (see `_with_json` in `test_scripts.py`).
- **Tolerances**: the scene dump rounds to 4 decimals → checks use 1e-4;
  exact assertions go through `rasm.doc.get()` json read-back.
- **Invariants everywhere**: exit code 0, no `panicked at` output; expected
  error paths are asserted as loud `! …` lines / tracebacks, never silently
  swallowed.
- **No Rust changes by the suite itself**: it is black-box only. Divergences
  it finds are reported (see below), and fixes land as separate commits.
  Remaining observed behaviors asserted as-is: the RSM writer emits the
  current `VERSION` constant (34, not 1), and the kernel reports no
  intersection for arc-arc endpoint contacts, identical circles, or
  collinear-overlapping segments.

## Findings fixed by the suite (commit log)

- `rasm.doc.blocks` was registered as `doc_blocks` (missing the pyo3
  `name="blocks"` attribute) → fixed in `cad_script/src/rasm.rs`.
- New rasm entities ignored the active layer / current style (default
  `Style` only) → `cad_cli` now stamps fresh style on adds, mirroring the
  GUI's `stamp_fresh_style`.
- `rasm.scale` accepted factor ≤ 0 (docs: "factor must be > 0") → Python-side
  `ValueError` validation.
- `rasm.copy` with all-invalid indices returned `[]` silently (docs: "if
  none of them exist the call raises") → loud error like the other transforms.
- `scripts/modify_demo.py` called non-existent APIs (`set_props`, `edit`,
  `index_of`, single-arg `copy`) → rewritten against the real surface.
- `scripts/grid_circles.py` doc comment said `run grid` (the script is
  resolved by exact stem) → corrected to `run grid_circles`.
