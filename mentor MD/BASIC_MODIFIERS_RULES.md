# Basic Modifier Rules — the canonical spec (copied from RUST_CAD)

**Status:** authoritative. This is the *contract* the `cad_solid` sandbox modifiers
must satisfy so a later merge into the app has zero surprises. When in doubt, this
file wins over any code already in the sandbox.

**Golden rule (do not relitigate):** the modifier *workflow* — invocation,
select‑first, per‑pick prompts, pick semantics, osnap/card availability, keyword
options, continue‑vs‑single, preview ghost — must be **IDENTICAL** to RUST_CAD. We
implement by mirroring the app's state machines, not by inventing our own.

All `app.rs:NNNN` citations are to `/home/HSI/workspace/RUST_CAD/cad_app/src/app.rs`;
`parser.rs` = `cad_kernel/src/parser.rs`; `dobject.rs`/`geom.rs` = `cad_kernel/src/*`.
Extracted 2026‑07‑13. RUST_CAD and simLUX share this kernel byte‑for‑byte, so
`DObject::translated/rotated/scaled/mirrored` are the *same symbols* — call them
directly, never reimplement the math.

---

## 0. Universal workflow (every modifier)

### 0.1 Invocation + aliases (`parser.rs:353-367`)
```
move   | m                    → Move
copy   | c | cp | co          → Copy
rotate | ro                   → Rotate
scale  | sc                   → Scale
mirror | mi                   → Mirror
delete | erase | e            → Erase
```
Bare verbs — no args. Buttons route through the same `run_command` path (`app.rs:3207`).

### 0.2 Select‑first (pickfirst vs. select‑then‑operate)
Identical for all six: **if the selection is empty → open a selection session and
queue the op; if objects are pre‑selected → skip straight to the first pick.**

- Empty branch → `begin_selection(ForSelect)` + set `queued_op` + a "select
  dobjects, Enter to continue" prompt.
- On **Enter**, `finalise_selection` (`app.rs:5255`) dispatches the queued op into its
  first pick state (`app.rs:5385-5494`).
- Guard: if the finalised basket is still empty → cancel with
  `"! {op}: nothing selected — operation cancelled"` (`app.rs:5374`).

### 0.3 Confirm / cancel selection
- **Confirm = Enter** (or Space when the cmd line is empty). No right‑click‑confirm.
- **2‑stage cancel** on empty‑basket Enter (`app.rs:21176`): first Enter →
  `"please make a selection (Enter again to cancel)"`; second → `cancel_selection()`.
- Sub‑commands mid‑session (`parser.rs:345`, handled `app.rs:4511`): `all`,
  `before/prev`, `none`, `remove`, `addmode`, `window/w`, `crossing/c`, `last/l`.
  Single letters `w/c/cr/a/b/l/n` are rewritten to sub‑commands *before* the parser
  (`app.rs:4349`) so `c` = crossing during a session, not copy.
- Window vs. crossing by **drag direction** (`app.rs:6998`): L→R = window
  (fully‑enclosed), R→L = crossing (any touch).

### 0.4 "New command overrides old"
There is **no blanket auto‑cancel** in the app. Interactive states are resolved by a
**mutually‑exclusive priority cascade** on each canvas click (move → copy → paste →
rotate → scale → … → mirror → select, `app.rs:23498+`); a session ends only by
user Enter/Esc. **BUT** the sandbox rule the user set is stronger and MUST hold:
*running a new command overrides whatever modifier/draw was in progress.* Implement
that as: **every `run_*` entry point calls `abort()` first** (clear all modifier +
draw states) before starting the new one. (This is the sandbox's `run_modifier` →
`abort_3d` pattern — keep it.)

### 0.5 Pick‑point resolution — applies to EVERY point pick
Priority (`app.rs:23402`): **osnap hit > extension‑track > CARD → grid > raw.**
- OSNAP is live for all five interactive modifier states (`app.rs:22779`).
- **CARD‑anchor rule (critical, `card_anchor` `app.rs:17558`):** CARD (H/V lock)
  needs an anchor, which only exists at the *second‑and‑later* pick of an op:
  - **Base points and pivots: OSNAP + grid, but NO card** (no anchor yet).
  - **Destination / angle / factor / 2nd‑axis picks: OSNAP + grid + CARD.**
  - Anchors: move dest←base, copy dest←base, mirror B←A, rotate angle/ref←pivot,
    scale factor/new‑len←pivot, scale ref‑end←ref‑start.

### 0.6 Preview ghost (every modifier, redrawn each frame at the CONSTRAINED cursor)
Translucent `draw_dobject(transformed_geom, color)`:
- Move `translated(v)` accent RGB(255,200,100)@180 (`app.rs:26366`)
- Copy `translated(v)` RGB(150,230,170)@180 (`app.rs:26411`)
- Rotate `rotated(pivot,θ)` white@130 + degree label (`app.rs:25776`)
- Scale `scaled(pivot,f)` white@130 + `×f` label (`app.rs:25875`)
- Mirror `mirrored(a,b)` RGB(200,160,255)@150 + dashed axis (`app.rs:26485`)
Move/Copy also draw marching‑ants base→cursor + base blip; Rotate/Scale a pivot
cross + baseline + numeric label.

---

## 1. MOVE — SINGLE, 2 picks
`MoveState { Off, WaitingForBase, WaitingForDest(base) }` (`app.rs:1711`).
1. **BASE** — `"move (N): click BASE point"`. osnap✓ grid✓ card✗.
2. **DEST** — `"move: BASE=(x,y) — click DESTINATION"`. osnap✓ grid✓ card✓ + DDE
   (type a distance → along constrained cursor dir). `apply_move(dest−base)` → Off.

`apply_move` (`app.rs:6968`): `d.translated(v)` per selected; **clears selection**.
Esc → Off `"move cancelled"`.

## 2. COPY — SINGLE‑DROP (⚠ not AutoCAD multi‑drop), 2 picks
`CopyState { Off, WaitingForBase, WaitingForDest(base) }` (`app.rs:1790`).
Same picks/capabilities as Move. `apply_copy(dest−base)` (`app.rs:16870`):
`duplicate_dobjects(sources, |g| g.translated(v))` with fresh handles; **clears
selection**; commits ONCE and returns to Off (`app.rs:23528`).
> **Decision for the sandbox:** match RUST_CAD → **single‑drop**. (User asked
> "then it will complete" = single. Do NOT ship AutoCAD‑style continue unless the
> user later says so; then it'd be a divergence to flag.)

## 3. ROTATE — SINGLE (+copy toggle), pivot → angle  ← the one to fix
`RotateState { Off, WaitingForPivot, WaitingForAngle(pivot),
  WaitingForRefSrc1(pivot), WaitingForRefSrc2(pivot,src1),
  WaitingForRefTgt(pivot,src_angle) }` (`app.rs:1828`) + flag `rotate_copy`.

**Default flow:**
1. **PIVOT** — `"rotate (N): click PIVOT point"`. osnap✓ grid✓ card✗.
   On click → `WaitingForAngle`, prompt:
   `"rotate (pivot=(x,y)): click to pick angle, or type number (CCW=+), R=reference, C=copy"`.
2. **ANGLE** — picked OR typed:
   - **Click:** `θ = (cursor − pivot).angle()` — the **absolute pivot→cursor angle
     from +X, CCW positive** (the +X axis is the implicit zero baseline; there is NO
     second base‑angle pick in the default path). osnap✓ grid✓ card✓ (snaps to
     cardinal dirs, anchored at pivot).
   - **Type a number:** interpreted as **degrees**, CCW+.
   - **`R`** → Reference mode; **`C`** → toggle `rotate_copy`.

**Reference sub‑flow (R):** pick SOURCE‑1, pick SOURCE‑2 (these 2 define the current
direction anywhere), then pick NEW direction anchored at pivot (or type degrees);
`Δθ = normalize(tgt − src_angle)` into (−π,π]. (`app.rs:23569`, prompts quoted there.)

`apply_rotate_or_copy(pivot,θ)` (`app.rs:16909`): if `rotate_copy` → duplicate via
`g.rotated(pivot,θ)`; else `apply_rotate` in‑place `d.geom.rotated(pivot,θ)`
(`geom.rs:577`) — in‑place does NOT clear selection. Preview: pivot cross + baseline
+ white ghost + live `"{deg}° (copy)"` label. Esc → Off, `rotate_copy=false`.

> **Sandbox is WRONG today:** it treats rotate as two arbitrary points with no pivot
> semantics, no typed‑degrees, no CARD, no R/C, no degree label. Rebuild it to this.

## 4. SCALE — SINGLE (+copy toggle), pivot → factor
`ScaleState { Off, WaitingForPivot, WaitingForFactor(pivot),
  WaitingForRefStart(pivot), WaitingForRefEnd(pivot,start),
  WaitingForNewLength(pivot,ref_d) }` (`app.rs:1846`) + flag `scale_copy`.
1. **PIVOT** — `"scale (N): click PIVOT (base point)"`. osnap✓ grid✓ card✗.
2. **FACTOR** — click → `factor = dist(pivot,cursor)` (guard ≥EPS); or type factor;
   `R` reference, `C` copy. osnap✓ grid✓ card✓.
   **Reference:** pick ref‑start, ref‑end (old length), then new length (pick=dist
   from pivot, or typed); `factor = new/old`.
`apply_scale(_or_copy)` (`app.rs:16931`/`16951`): `g.scaled(pivot,factor)` uniform
(`geom.rs:674`). Preview: pivot + baseline + white ghost + `"×{factor}"`. Esc → Off.

## 5. MIRROR — SINGLE (+keep toggle), A → B → keep?
`MirrorState { Off, WaitingForA, WaitingForB(a), AwaitingKeep(a,b) }` (`app.rs:1858`).
1. **A** — `"mirror (N): click FIRST axis point"`. osnap✓ grid✓ card✗.
2. **B** — `"mirror: A=(x,y) — click SECOND axis point"`. osnap✓ grid✓ card✓ (anch A).
3. **KEEP?** — `"mirror: keep original? [Y]/n (Enter=keep a copy, n=erase original)"`.
   Answer via cmd line / Enter (`app.rs:3544`): `""|y|yes|keep`→keep; `n|no`→flip
   in place. Canvas clicks ignored at this step.
`apply_mirror(a,b,keep)` (`app.rs:16976`): `g.mirrored(a,b)` (`geom.rs:751`). Preview:
dashed axis extended past both ends + ghost. Esc → Off.

## 6. ERASE — no picks, select‑then‑commit
No state machine; `QueuedOp::Erase`. Empty → select session, Enter commits; pickfirst
→ deletes immediately (`app.rs:4664`). Removes highest‑index‑first; clears selection;
`"- erased N dobject(s)"`. No `apply_erase` fn (inline).

---

## 7. Kernel transforms (call these — never reimplement)
`DObject` preserves style+handle (`dobject.rs:46`): `translated`(48) `rotated`(57)
`scaled`(63) `mirrored`(69) → delegate to `Geom` (`geom.rs` 1052/577/674/751).
Copies (copy / rotate‑copy / scale‑copy / mirror‑keep) go through
`duplicate_dobjects` (`app.rs:18061`): fresh `next_handle()` + hatch handle remap.

---

## 8. Session‑recorder requirements (user demand, 2026‑07‑13)
The recorder must make a modifier run **reconstructable from the dump alone**. For
every modifier, log:
1. **On command start** — the op AND the highlighted set: object count **and their
   handles/ids** (not just "selection=1"). e.g. `begin rotate — sel=[#3]`.
2. **On each pick** — a *named* step + the resolved world point AND whether osnap
   fired + which kind. e.g. `rotate PIVOT = (x,y) [snap=END #3]`,
   `rotate ANGLE = (x,y) → 37.0° [snap=none]`.
3. **On apply** — the committed parameters: `rotate ✓ 37.0° about (x,y) on [#3]`.
Currently the dump shows only `w=(...) → NeedMore/Applied` with no highlight set and
no "this pick = pivot/base". Fix that. Recorder UI stays byte‑identical to RUST_CAD.

---

## 9. Sandbox conformance checklist
- [ ] Rotate: pivot→angle, typed‑degrees (CCW+), CARD at angle, R reference, C copy,
      degree label, pivot cross preview. **(broken — priority)**
- [ ] Scale: pivot→factor, typed factor, R reference (old/new length), C copy, `×f`.
- [ ] Mirror: A→B→[Y]/n keep, dashed axis preview.
- [ ] Move/Copy: base→dest, DDE, CARD at dest; Copy single‑drop.
- [ ] Every base/pivot pick: osnap+grid, NO card. Every 2nd+ pick: +card.
- [ ] New command aborts the in‑progress one (sandbox override rule).
- [ ] Recorder logs highlighted set + named picks + snap kind + apply params (§8).
