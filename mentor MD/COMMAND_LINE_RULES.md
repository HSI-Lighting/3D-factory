# Command Line Rules — the canonical spec for `cad_solid`

**Status:** authoritative contract, sibling to `BASIC_MODIFIERS_RULES.md`. Where this file
and any handoff doc disagree, **this file wins** — it is verified against code, not prose (§0).

**Ground truth:** `~/workspace/RUST_CAD/cad_app/src/app.rs` (35,235 lines),
`cad_kernel/src/parser.rs`. **Verified 2026-07-14 — all line numbers below are real, not
inherited from the handoff docs (whose numbers had drifted by ~950 lines).**

**Golden rule:** **one entry point + an ordered intercept cascade + parse/dispatch.**
Every modal sub-command is `state != Off` + a text consumer with an early-return slot.
**Never a global `enum Mode`.**

---

## 0. ⚠️ Doc errata — resolved against code (read first)

The two handoff docs contradict each other on Space-as-Enter. **The newer one is wrong.**

| Doc | Claim | Verdict |
|---|---|---|
| `COMMAND_LINE_AND_MOUSE_RULES.md` §A.1 (2026-07-04) | *"There is **no Space-as-Enter** in this codebase (that's a different project)."* | ❌ **FALSE** |
| `COMMAND_LINE_CURRENT.md` §1/§6/§14 (2026-06-24) | *"Enter **or Space** submits it"* | ✅ **CORRECT** |

**Proof:** `app.rs:12693-12699` (`submit_via_space`) and `app.rs:25696-25711`
(`trigger = enter_now || (space_now && cmd_is_empty && !in_text_body)`).

Also correct the drifted anchors — the docs' line numbers are ~950 short:

| Symbol | Doc says | **Actually** |
|---|---|---|
| `run_command` | 2845 / 3207 | **3801** |
| `dispatch_base` | 10517 | **12162** |
| `begin_selection` | 4729 | **6113** |
| `add_window_selection` | 7037 | **7961** |
| `flow_input_text` | 13666 / 14527 | **16903** |
| `zoom_input_text` | — | **17278** |
| `set_prompt` / `clear_prompt` | 2835 | **3691 / 3695** |
| `command_internal_undo` | 3271 | **3537** |
| empty-Enter cascade | 18888 / 25697 | **25696** |

> **Names are the stable anchor — grep, don't trust a number.**

---

## 1. The architecture in one picture

```
keystrokes ─▶ self.cmd ──Enter │ Space(non-empty, focused)──▶ run_command(raw)
                 │                                                  │
                 │                                     ┌────────────┴─────────────┐
                 │                              27 intercepts           parse + dispatch
                 │                          (state != Off ? consume     (cad_kernel::parser
                 │                           + return early)             → dispatch_base)
                 │
                 └──Enter │ Space(EMPTY, focused, !in_text_body)──▶ update() trigger cascade
                                                          (finish draw / advance flow /
                                                           2-stage cancel / repeat last)
```

Two **independent** submit paths. Implementing only one is the classic bug.

---

## 2. One entry point — `run_command` (`app.rs:3801`)

Order inside it is load-bearing:

1. **History echo FIRST** (`3805-3809`) — `"command: {raw}"` at the idle prompt, `"> {raw}"`
   when replying to an active prompt.
2. **Reset the 2-stage-Enter notice** (`3812`) — any non-empty input cancels it.
3. **Recorder `CmdRun` BEFORE the intercepts** (`3817-3824`) — logs `raw` + `parsed_debug`
   + `source: Typed`. It speculatively parses **only to log the debug string**; the real
   parse happens at the end. This is why every submission appears in the dump even when a
   sub-flow eats it.
4. **27 intercepts** (§3).
5. **Select-letter rewrite** (`5190-5203`) then **`parse(&effective)`** (`5204`).
6. **Commit-on-interrupt** (`5209-5213`) — a real command while PLINE/SPLINE is drawing
   **finishes** it (never discards placed vertices). Snap overrides are exempt.
7. **`last_command`** set **only on a successful parse**, before dispatch (`5214+`).

---

## 3. The intercept cascade — the full verified list

**27 slots**, in this exact order (the handoff doc said "~14" — it's 27):

| # | `app.rs` | Intercept | Keyed on |
|---|---|---|---|
| 1 | 3827 | SYSVAR value entry | `var_set_pending` |
| 2 | 3856 | Dim: force type (h/v/a/r/d/auto) | `tool == Tool::Dim` |
| 3 | 3901 | LINE `c`/`close` | `tool == Tool::Line` |
| 4 | 3915 | SPLINE `c`/`close` | `tool == Tool::Spline` |
| 5 | 3927 | Hatch panel keywords | `hatch_dialog_open` |
| 6 | 3947 | **Command-internal UNDO (`u`)** | any active command |
| 7 | 3967 | Undo-baseline capture | fresh top-level cmd |
| 8 | 3976 | Prompt-driven flow (CIRCLE) | `cmd_flow` |
| 9 | 4002 | ZOOM flow | `zoom_state != Off` |
| 10 | 4036 | `setvar` access | — |
| 11 | 4103 | Text-tool sub-commands | `text_draft` |
| 12 | 4202 | Rectangle sub-commands | — |
| 13 | 4327 | Rectangle w/h entry | — |
| 14 | **4363** | **MIRROR keep-original Y/n** ⭐ | `MirrorState::AwaitingKeep` |
| 15 | **4387** | **Direct Distance Entry (DDE)** ⭐ | Move/Copy/Line/Stretch dest |
| 16 | 4450 | Block Task Recorder naming | `block_task_rec` |
| 17 | 4484 | Insert ANGLE step | — |
| 18 | 4500 | Insert parametric value | — |
| 19 | 4533 | Pending-sub-arg | — |
| 20 | 4623 | Hatch-confirm shortcuts | — |
| 21 | 4658 | PLINE sub-commands (A/L/C/U/W/H) | `tool == Polyline` |
| 22 | 4852 | Fillet sub-commands | — |
| 23 | 4922 | Chamfer sub-commands | — |
| 24 | 4993 | Offset sub-commands | — |
| 25 | **5095** | **ROTATE `R`/`C`/degrees** ⭐ | `RotateState::WaitingForAngle` |
| 26 | **5130** | **SCALE `R`/`C`/factor** ⭐ | `ScaleState::WaitingForFactor` |
| 27 | **5190** | **Select-letter rewrite** ⭐ | `select_mode != Off` |

⭐ = **directly relevant to `cad_solid` today** (§6).

### 3.1 The shape every intercept follows

```rust
if let SomeState::Waiting(ctx) = self.some_state {      // 1. am I active?
    match trimmed.to_ascii_lowercase().as_str() {
        "r" | "ref" => { /* advance */ return; }        // 2. consume + RETURN
        _ => { if let Ok(n) = trimmed.parse::<f64>() { /* apply */ return; }
               /* 3. NOT mine → fall through to the parser */ }
    }
}
```
**The fall-through is the feature, not an oversight** (`5121-5123`): typing `move` during a
live rotate is *not* consumed → reaches the parser → starts Move → overrides the rotate.
That is the "new command overrides old" rule, implemented by *omission*.

---

## 4. SPACE = ENTER — the exact contract

### 4.1 Non-empty buffer → submit (`app.rs:12690-12705`)

```rust
let enter_pressed = (text_resp.lost_focus() && key_pressed(Enter))
                 || (text_resp.has_focus()  && key_pressed(Enter));   // BOTH
let space_pressed = text_resp.has_focus() && key_pressed(Space);      // FOCUS-GATED
let in_text_body  = matches!(self.text_draft, TextDraftState::WaitingForString(_))
                 || self.text_waiting_height;
let submit_via_space = space_pressed
    && !self.cmd.trim_end_matches(' ').is_empty()      // non-empty AFTER trim
    && !in_text_body;
if submit_via_space { self.cmd = self.cmd.trim_end_matches(' ').to_string(); }  // 12698
if enter_pressed || submit_via_space {
    if !self.cmd.trim().is_empty() { let c = take(&mut self.cmd); self.run_command(&c); }
    self.refocus_cmd = true;
}
```

Four load-bearing details — each is a bug if dropped:
1. **`has_focus()` gate.** Space submits **only when the command input is focused** — else a
   canvas Space (pan/nav) fires commands. Focused + non-empty is the whole rule; do not
   re-gate further.
2. **`trim_end_matches(' ')` before the emptiness test.** egui's `TextEdit` has **already
   inserted the space** by the time you read the key event — the buffer is `"move "`.
3. **Trim the buffer before submitting** (`12698`).
4. **`refocus_cmd = true`** — the caret returns after every submit.

### 4.2 Empty buffer → advance (`app.rs:25696-25711`)

```rust
let enter_now    = key_pressed(Enter);
let space_now    = key_pressed(Space);
let cmd_is_empty = self.cmd.trim().is_empty();          // trim() → " " counts as empty
let in_text_body = /* WaitingForString | text_waiting_height */;
let trigger      = enter_now || (space_now && cmd_is_empty && !in_text_body);
// …then in EVERY arm that fires:
if space_now { self.cmd.clear(); }                       // 25725, 25735, 25750, … 25968
```
RUST_CAD's own comment (`25706-25709`): *"We let the TextEdit also see the space (harmless:
cmd stays trim-empty), then strip any leading whitespace."* — hence the `clear()` repeated
at **~10 arms**. Ugly but load-bearing.

### 4.3 The `in_text_body` exception
Space must stay **literal** while typing a string/height, or `"Hello world"` submits at
`"Hello"`. `cad_solid` has no text entry today → expose
`fn in_text_body(&self) -> bool { false }` **now**, so there is exactly one place to update
when the flat sketch gains TEXT.

---

## 5. The empty Enter/Space cascade (`app.rs:25712+`)

**Mutually exclusive — one Enter = exactly one transition per frame.** The user terminates
sessions; the program never auto-chains. Verified arm order:

| `app.rs` | Arm |
|---|---|
| 25720 | Hatch panel → commit previewed hatch |
| 25730 | Hatch pick-point armed → end session |
| 25746 | Block Task Recorder → finish |
| 25759 | — |
| 25771 | Draw tool at first-point → `feed_first_point_from_last()` |
| 25786 | ZOOM flow → advance/exit |
| 25804 | PEDIT menu → exit |
| 25817 | PLINE width/arc sub-flow |
| 25832 | **Select session, empty basket → 2-stage cancel** |
| …later | finish LINE/POLYLINE/SPLINE · **repeat `last_command`** |

`cad_solid` already collapses this into **one function: `confirm()`**
([sandbox.rs:315](../cad_solid/examples/sandbox.rs#L315)). That is the right shape — keep it, and route empty-Space to it.

---

## 6. The four intercepts `cad_solid` needs NOW

> **Headline:** the two gaps from the modifier review (`MENTOR_REVIEW_2026-07-14.md` §3.1
> Mirror-keep, §3.2 DDE) are **cascade intercepts in RUST_CAD, not modifier internals.**
> Porting the cascade fixes both — you are not writing new modifier logic.

### 6.1 MIRROR keep-original (`app.rs:4367-4384`) — closes review §3.1
```rust
if let MirrorState::AwaitingKeep(a, b) = self.mirror_state {
    let keep = match trimmed.to_ascii_lowercase().as_str() {
        "" | "y" | "yes" | "keep" => Some(true),
        "n" | "no" | "ni"         => Some(false),
        _ => None,
    };
    match keep {
        Some(k) => { self.mirror_state = MirrorState::Off; self.apply_mirror(a, b, k); self.clear_prompt(); }
        None => self.history.push("  ! mirror: answer Y (keep a copy) or n (erase original) — Esc cancels".into()),
    }
    return;                       // canvas clicks ignored at this step
}
```
Note: a bad answer **re-prompts, never cancels**. Empty (`""`) = keep — reachable because
empty-Enter/Space routes here via the cascade.

### 6.2 Direct Distance Entry (`app.rs:4394-4447`) — closes review §3.2
```rust
let dde_anchor = match (…) {                      // Line pending[0] | Move/Copy/Stretch base
    MoveState::WaitingForDest(base) => Some(base), CopyState::WaitingForDest(base) => Some(base), … };
if let Some(anchor) = dde_anchor {
    if let Ok(dist) = trimmed.parse::<f64>() {
        match self.last_cursor_raw_world {
            Some(raw) => {
                let constrained = self.apply_constraints(raw);   // ← CARD applied HERE
                let dir = constrained - anchor;
                if dir.len() > EPS { let p = anchor + dir / dir.len() * dist;  /* apply */ }
                else { "! move the cursor to set a direction, then type the distance" }
            }
            None => "! hover the canvas to set a direction, then type the distance",
        }
        return;
    }
}
```
**The mechanism:** DDE = `anchor + normalize(constrained_cursor − anchor) × dist`. It needs a
**live hovered cursor** to supply the *direction*; the typed number supplies only the
*magnitude*. Both "no direction" cases have explicit user-facing errors.

**`cad_solid` already has the prerequisite:** `self.hover_plane_pt` is the exact analog of
`last_cursor_raw_world`, and `world_delta_carded` is the analog of `apply_constraints`. So
DDE is a small addition — and it removes today's lying `"unknown command: 42"`.

### 6.3 ROTATE / SCALE sub-commands (`app.rs:5095-5183`)
Confirms `cad_solid`'s design is already right. RUST_CAD:
`"r"|"ref"|"reference"` → reference sub-flow; `"c"|"cp"|"copy"` → toggle copy + re-prompt
showing `copy ON/off`; a number → apply (degrees CCW+ / factor); **anything else falls
through to the parser**. Also: `ScaleState::WaitingForNewLength` accepts a typed new length
(`factor = new/ref_d`, guarded `> EPS`), and `RotateState::WaitingForRefTgt` accepts typed
degrees (`dtheta = normalize(tgt − src)`).

**`cad_solid`'s `Modify::type_value() -> Option<Feed>` is a 1:1 match** for this pattern:
`Some(_)` = consumed, `None` = fall through. It already implements R/C/degrees/factor and
both reference tails ([modify.rs:287-328](../cad_solid/src/modify.rs#L287-L328)). **This is already ported — do not rewrite it.**
One nit: RUST_CAD resets `rotate_copy = false` on apply; `cad_solid` drops the whole
`Modify`, so copy resets naturally. Equivalent.

### 6.4 Select-letter rewrite (`app.rs:5190-5203`)
```rust
let effective = if self.select_mode != SelectMode::Off {
    match raw.trim().to_ascii_lowercase().as_str() {
        "w" => "window", "c" | "cr" => "crossing", "a" => "all",
        "b" | "bef" => "before", "l" => "last", "n" => "none",
        _ => raw,
    }.to_string()
} else { raw.to_string() };
let parsed = parse(&effective);
```
**Rewrite before the parser — never push context into the parser.** `c` = Copy globally, but
Crossing while selecting. `cad_solid` has no window/crossing yet (review §3.5), so this
lands with that work, not before.

---

## 7. ⚠️ Space=Enter forecloses typed multi-token arguments

Not optional — arithmetic:
- The parser **does** accept space-separated args (`split_whitespace`, `parser.rs:297-298`):
  `f 10`, `line 0,0 10,10`.
- But with §4.1 active, typing `f` then Space **submits `f`**. You never reach `f 10`.

**So in RUST_CAD today every space-separated parser form is typed-unreachable** — surviving
only via **paste** or **programmatic dispatch** (buttons / `dispatch` strings).
`COMMAND_LINE_CURRENT.md` §4's *"`line 0,0 10,10` draws at once"* is true of the parser and
false of the keyboard. This is the AutoCAD model: **one token per submission**, arguments via
successive prompts — which is *why* AutoCAD can afford Space=Enter.

**Cost to `cad_solid` today: zero.** The whole vocabulary is single-token —
`move|m`, `copy|c|co|cp`, `rotate|ro`, `scale|sc`, `mirror|mi`, `erase|delete|e`, `dump`,
`recorder|rec|dbg` ([sandbox.rs:456-463](../cad_solid/examples/sandbox.rs#L456-L463)), plus bare numbers and `r`/`c`. Adopt it now;
this section is the record so it isn't rediscovered painfully. It forecloses: future
`cmd arg` typed forms, and any `move 10 0` spelling of DDE (spell DDE as a bare `10` at the
DEST prompt — which §6.2 and the modifier spec both want anyway).

---

## 8. Supporting model (port these too)

| Concern | RUST_CAD | `cad_solid` today |
|---|---|---|
| **History** | `self.history: Vec<String>`, echo `command: x` / `> x`, canonical name echo | `self.note()` → recorder only; **no visible log book** |
| **Prompt** | `self.current_prompt` + `set_prompt`/`clear_prompt` (`3691/3695`), drawn green | `self.status` (equivalent) ✅ |
| **Repeat-last** | `last_command`, set **only on successful non-empty parse** (`5214+`); empty-Enter re-runs | ❌ **missing** |
| **Refocus** | `refocus_cmd` → caret returns after submit/canvas click | partial ([sandbox.rs:1208](../cad_solid/examples/sandbox.rs#L1208)) |
| **Aliases** | inline in the parser's `match head` | inline `match v.as_str()` ✅ |

> **`last_command` gating is load-bearing** (`COMMAND_LINE_CURRENT.md` §7): set it on *any*
> input and a typo like `1` gets re-run forever by empty-Enter.

---

## 9. Port judgment — what to copy, what NOT to

**Copy the contract. Do NOT copy the shape.**

RUST_CAD's `run_command` is **1,650 lines of inline intercepts** keyed on ~18 scattered
`*_state` fields. That is **known debt** — RUST_CAD's *own* spec (`COMMAND_LINE.md`) exists
to replace it with "one Command IR + pure `resolve()` + `CmdHost`", and says the honest cost
is completing the IR. **Porting the sprawl into `cad_solid` would import debt its owner has
already written a plan to delete.**

`cad_solid` is *already better here*: one `Modify` struct + one `type_value()` consumer
replaces RUST_CAD's four separate rotate/scale intercepts (`5095`, `5130`, `5157`, `5170`).
**Keep that consolidation.** Add intercept *slots*, not 27 inline blocks.

So: **behavioral parity, consolidated structure.** Match every prompt, keyword, and
fall-through rule; do not match the line count.

---

## 10. Port slices

| Slice | Content | Status |
|---|---|---|
| **1** | **Space=Enter** — §4.1 + §4.2 + the `in_text_body()` hook | ✅ **DONE 2026-07-15** — `Sandbox::cmd_submit` + `in_text_body`, wired to **both** command lines (3D `cmd` + flat `flat_cmd`). Build clean · 37/37 tests · launches. |
| **2** | **Fix the `wants_keyboard_input` trap** (§11) | ✅ **DONE (by the owner)** — Esc now runs ungated at [sandbox.rs:1460](../cad_solid/examples/sandbox.rs#L1460). Enter/Space reach the box via `cmd_bar`'s own path, so §4.2 works. |
| **3** | **Mirror keep-[Y]/n** (§6.1) | ⬜ closes the only dead-end op (review §3.1) |
| **4** | **DDE** (§6.2) | ⬜ closes review §3.2; kills the lying error line |
| **5** | `last_command` + visible history (§8) | ⬜ cheap parity |
| **6** | Select-letter rewrite (§6.4) | ⬜ only with window/crossing work |
| **7** | Live prompt in the pill + `[Verb/Verb]` chips | ⬜ UI; needs the command-bar rework |
| **—** | `COMMAND_LINE.md` IR refactor | **not started even in RUST_CAD.** The sandbox must not lead it. |

### 10.1 Slice 1 — as implemented (2026-07-15)

`Sandbox::cmd_submit(ui, r, buf, in_text_body) -> bool` is the **single** submit path for
both lines. Deliberate deviations from RUST_CAD, and why:

| RUST_CAD | `cad_solid` | Why |
|---|---|---|
| Two sites: widget (`12703`, non-empty) **+** ungated global cascade (`25710`, empty), needing `if space_now { self.cmd.clear(); }` at **~10 arms** | **One** focus-gated path; the trim makes `" "` → `""`, which the caller routes to `confirm()` | Same behaviour, no double-fire class of bug, no repeated `clear()`. cad_solid already collapsed the empty cascade into `confirm()` (§5). |
| Empty-Space is **ungated** (fires from the canvas too) | Empty-Space requires **box focus** | The box auto-refocuses whenever nothing else holds focus, so this is ~always true — and it *strengthens* §4.1 rule 1 (a canvas Space must never fire a command). |

**Verified:** builds clean, 37/37 tests pass, launches without panic. **NOT verified:** the
keystrokes themselves — that needs a human at the window (or the recorder). The logic
mirrors RUST_CAD `app.rs:12703-12712` line-for-line, including the load-bearing
`trim_end_matches(' ')` (egui inserts the space *before* the key event is read).

**Flat-line consequence (§7), now concrete:** `Draw::start_verb` uses `split_whitespace()`
and reads a 2nd token as the method (`circle 3p`, `arc sce`). Those one-liners are now
**untypeable** — Space submits at `circle`. **No capability is lost:** `Draw::option()`
accepts the same method tokens mid-tool, so `circle` ␣ `3p` is equivalent. The flat hint
text was updated to teach the verb→option flow instead of the dead one-liner.

---

## 11. ⚠️ Blocking bug for Slice 1 — the `wants_keyboard_input` trap

`update()` gates the whole keyboard cascade behind `if !ctx.wants_keyboard_input()`
([sandbox.rs:864](../cad_solid/examples/sandbox.rs#L864)) — Esc, Enter, Delete all live inside it. But the command box
takes focus and is re-focused after every submit ([sandbox.rs:1208](../cad_solid/examples/sandbox.rs#L1208)). **While it holds
focus that entire block is dead:**

- **Enter** survives only because `cmd_bar` has its own path ([sandbox.rs:1199](../cad_solid/examples/sandbox.rs#L1199)).
- **Esc** has **no** `cmd_bar` path → may never reach the cancel cascade.
- **Empty-Space (§4.2) will land in exactly the same trap.**

**RUST_CAD avoids this by running Enter/Space at BOTH levels** — the widget (`12690-12705`)
*and* an ungated global cascade (`25696+`). Its global cascade is **not** behind a
`wants_keyboard_input` gate. **Port that two-level structure**, don't just add Space to the
widget.

---

## 12. Conformance checklist

- [ ] Space submits when cmd input **focused** + non-empty after `trim_end_matches(' ')` + `!in_text_body`. (§4.1)
- [ ] Buffer trimmed **before** `run_command`; `refocus_cmd` set after. (§4.1)
- [ ] **Empty** Space → `confirm()`, **and clears the stray space**. (§4.2)
- [ ] `in_text_body()` predicate exists (returns `false` today). (§4.3)
- [ ] Space **inert when the canvas has focus**. (§4.1)
- [ ] Enter fires on **both** `has_focus` and `lost_focus`. (§4.1)
- [ ] Keyboard cascade reachable while the cmd line is focused — Esc + empty-Space work. (§11)
- [ ] Recorder logs the raw line **before** the intercepts. (§2.3)
- [ ] Mirror `AwaitingKeep`: `""|y|yes|keep`→keep, `n|no`→flip, bad answer **re-prompts**. (§6.1)
- [ ] DDE: `anchor + normalize(constrained_cursor − anchor) × dist`; both no-direction errors present. (§6.2)
- [ ] `type_value → None` still falls through → new command overrides. (§3.1, §6.3)
- [ ] `last_command` set **only on successful non-empty parse**. (§8)
- [ ] Multi-token constraint (§7) recorded; no `cmd arg` typed forms.
- [ ] Cascade consolidated, **not** 27 inline blocks. (§9)

*No code changed by this review — MD only, per role.*
