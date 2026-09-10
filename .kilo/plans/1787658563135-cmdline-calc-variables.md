# Command-Line Calculation & Variable Support

## Goal

Let the user type math expressions and named variables wherever a number is accepted — command-line prompts, flow replies, the 3D Factory command path, and **every numeric dialog field** — instead of plain numbers. Typing `2+3*4` at the `command:` prompt prints `= 14` and stores it in `ans`; `x=5` defines a variable usable everywhere, persisted per drawing.

## Settled decisions (interviewed)

| # | Decision | Choice |
|---|---|---|
| 1 | Expression language | Full math: `+ - * / ^ %`, parentheses, unary minus, functions `sqrt abs round floor ceil min max sin cos tan asin acos atan atan2 exp ln log10`, constants `pi`, `e`, `ans` (last result). **Trig in degrees.** No unit suffixes. |
| 2 | Variable semantics | **Lazy** — `h = w*2` stores the expression; every use re-evaluates, so changing `w` updates `h`. Cycle detection with a clear error. |
| 3 | Idle-prompt behavior | Bare expression → evaluate, print to history, store in `ans`. `x=5` → define. Bare defined name → print value. **Commands always win** (the calc fallback runs only in the parser-`Err` path). |
| 4 | Persistence | Per-drawing, in the existing `<drawing>.simlux.json` sidecar (`SimluxConfig`), serde-default field. No drawing open → session-only. |
| 5 | Dialog coverage | **All** numeric dialog fields via a shared helper (one-line change per site). |
| 6 | Spaces | Typed expressions have **no spaces** (`2+3`, `sqrt(4)`) because Space=Enter submits. Pasting `2 + 3` works — the evaluator tolerates internal spaces. **No change to the Space=Enter machinery.** |
| 7 | SYSVARs | User variables only; no SYSVAR read-back in v1. SYSVAR names keep today's behavior; defining a variable with a SYSVAR name is rejected. |

## Architecture

- **New module `cad_app/src/calc.rs`** — pure, no egui/UI deps:
  - `pub struct CalcStore { vars: BTreeMap<String, String> }` — name → stored expression string (lazy). Variable names: `[A-Za-z_][A-Za-z0-9_]*`, case-sensitive.
  - `pub fn eval(store: &CalcStore, src: &str) -> Result<f64, CalcError>` — lex → recursive-descent parse (precedence: unary > `^` > `* / %` > `+ -`) → evaluate; resolves variables recursively with a visiting set (`cycle: a → b → a` on loops); `CalcError` variants: `UnknownVar(name)`, `Cycle(Vec<String>)`, `Syntax(&'static str)`, `DivZero`, `NonFinite`.
  - `pub fn try_assign(store: &mut CalcStore, src: &str) -> Result<Option<(String, f64)>, CalcError>` — `None` when the input is not an assignment; rejects SYSVAR-name collisions via `crate::varreg::find`.
  - `fn looks_like_expr(s: &str) -> bool` — contains an operator char, `(`, a known function name, or a defined variable name (cheap gate used before attempting evaluation, so plain text/commands are never touched).
  - Exhaustive `#[cfg(test)]` unit tests (all operators, precedence, functions, degrees, `pi`/`e`/`ans`, lazy chains, cycles, malformed input, div-by-zero, non-finite).
- **`cad_kernel` is NOT touched** (byte-identical to RUST_CAD — README rule). Evaluation lives entirely in `cad_app`.
- **Precedence rule (all sites):** existing intercepts → plain `f64::parse` (fast path, unchanged) → `calc::eval` → error. Commands and SYSVARs always beat variables.
- **`ans`**: last idle-prompt expression result, stored as a number in the store; excluded from persistence.

## Task list

1. **`calc.rs` core + unit tests** (pure). `cargo test -p cad_app calc` green before anything else.

2. **Idle-prompt calc intercept** in `run_command_inner` (app.rs:16006). In the path where `cad_kernel::parser::parse` returns `Err` (currently "unknown command"), before printing that error:
   - input matches `name=expr` → `try_assign`; on success echo `  name = value` into `self.history`, mark sidecar dirty (Task 4); on error print `  ! calc: …`.
   - input `looks_like_expr` or is a defined variable name → evaluate; echo `  = value`; store `ans`. Known command tokens (`m`, `move`, …) never reach here because parse succeeds for them.
   - Add `calc` / `calc help` as a tiny intercept printing the syntax summary (assignment, functions list, no-spaces note).

3. **Wire command-line numeric sites.** Add a `CadApp` helper:
   `fn eval_number(&self, s: &str) -> Result<f64, String>` — `f64::parse` first, then `calc::eval`, with a unified `! calc: …` error.
   Survey `rg 'parse::<f64>' cad_app/src` (~30 sites in app.rs + flow inputs + pedit) and route each numeric reply through it, including:
   - prompt replies: text height (app.rs:16397, 16462), DDE distance (16561), fillet/chamfer/rotate/scale sub-values, rectangle `W H` tokens,
   - `flow_input_text` circle radius/diameter/TTR (34767–34789),
   - SYSVAR pending-value entry (`var_set_pending` intercept) — `setvar TxHt` then reply `0.3*2` works,
   - multi-token entries (`x,y` coordinates, `W H`): evaluate each whitespace token independently (paste path; typed multi-token is foreclosed by Space=Enter).
   - **3D Factory:** in the `ActiveView::ThreeD` branch (16055), rewrite the trimmed token through the evaluator *before* handing it to `factory.modify.type_value(...)`, so 3D modifiers accept `r*2`; same for factory panel command inputs in `factory.rs`.

4. **Sidecar persistence.**
   - `simlux_io::SimluxConfig` (simlux_io.rs:428) gains `#[serde(default)] vars: BTreeMap<String, String>`.
   - `build_simlux_config*` (app.rs:30749+) writes `self.calc.vars` (minus `ans`); `load_simlux_sidecar` (30827) populates `self.calc` on load.
   - Assignment with a drawing open triggers the existing sidecar save path (`build_simlux_config_common` + `simlux_io::save`, app.rs:30808); no drawing open → session-only, no error.

5. **Dialog wiring.** Shared helper `fn eval_field(&self, buf: &str) -> Result<f64, String>` (same as `eval_number`). Sweep every numeric `TextEdit` commit site (block dialog base X/Y, insert scale/rotation, array dialog, param editor, 3D Factory property panels in `factory.rs`, report options, settings page numeric fields): evaluate at commit (OK/Enter); on error keep the dialog open and show `! calc: …` (field tooltip/red border or dialog status line). **Integer fields** (counts, u8 settings): require an integral result — error otherwise, no silent rounding. Leave the buffer text as typed (user sees the expression they wrote).

6. **Consistency pass.** All calc errors use the `  ! calc: <reason>` history prefix; assignments echo `  name = value`; idle results echo `  = value`. Verify `transcript` command still logs prompt↔reply pairs unchanged.

7. **Validation.**
   - `cargo test --workspace` (all existing suites + new calc tests).
   - `cargo build --workspace` clean.
   - Manual smoke: `2+3*4` → `= 14`; `x=5` → `x = 5`; `x*2` → `= 10`; `ans/5` → `= 2`; `h=w*2` before `w` exists → unknown-variable error at use; `a=b`, `b=a` → cycle error; `sqrt(16)+2^3` → `= 12`; `sin(30)` → `= 0.5` (degrees); paste `2 + 3` → `= 5`; block dialog X = `x*2` → 10; fillet radius prompt reply `r*2` → works; `setvar TxHt` reply `0.3*2` → 0.6; variable named `CrsHrS` rejected; save/reopen drawing → variables survive; sidecar without `vars` (old file) loads clean.
   - Sidecar round-trip preserves expressions exactly (string storage — no float drift).

## Edge cases & failure modes

- **Cycle**: visiting set in `eval`; error names the chain.
- **Unknown variable** mid-expression: `! calc: unknown variable 'w'` — site keeps waiting for input (prompts re-prompt, dialogs stay open).
- **Div-by-zero / NaN/Inf**: explicit errors; never silently clamp.
- **Name collisions**: SYSVAR names rejected at definition; command tokens allowed but unreachable as bare names at idle (commands win) — still usable inside expressions and at numeric prompts.
- **`ans`** before any result: unknown-variable error.
- **No drawing open**: assignments work session-only; sidecar write skipped.
- **Plain numbers**: `f64::parse` fast path is untouched, so existing behavior (validation ranges, positive-only checks) is unchanged.

## Risks / merge-back

- Additive: one new module + `SimluxConfig.vars` (serde-default → old sidecars load) + one-line helper swaps. `cad_kernel`, the parser, and the Space=Enter contract are untouched — consistent with the fork's "minimal and surgical" rule.
- Expression strings stored verbatim: no float drift in persistence.
- The `parse::<f64>` sweep is the widest part; each site is independent and behavior-preserving when the input is a plain number.

## Out of scope (explicit)

- Unit suffixes (`2m+300mm`); SYSVAR read-back in expressions; smart-space submit; `cad_solid/examples/sandbox.rs` command line (being retired, README slice 5); case-insensitive names; undo integration for variable changes.
