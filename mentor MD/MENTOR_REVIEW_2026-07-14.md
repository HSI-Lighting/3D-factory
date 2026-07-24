# Mentor Review — cad_solid modifiers vs. `BASIC_MODIFIERS_RULES.md`

**Reviewer:** 3d mentor · **Date:** 2026-07-14 · **Branch:** `simlux-3d-sandbox`
**Reviewed state:** *working tree* (uncommitted), not GitHub `9f9ec5b`.
`modify.rs md5=b3451c6…`, `sandbox.rs md5=dd6b366…`. Build: **clean**. Tests: **28/28 pass**.

> ⚠️ The tree was **actively edited during this review** and passed through two
> non-compiling transients (Vec2/Vec3 mismatch on `first`; `world_delta_carded`
> used before defined). It is green *now*. Commit soon so the reviewable state stops
> moving — a half-applied refactor on disk is the single biggest review hazard here.

---

## 1. Verdict against the §9 conformance checklist

| §9 item | State | Note |
|---|---|---|
| **Rotate**: pivot→angle, typed°(CCW+), CARD@angle, R, C, °label, pivot cross | ✅ **CONFORMS** | The priority item is **fixed**. See §2. |
| **Scale**: pivot→factor, typed factor, R (old/new len), C, `×f` | ✅ **CONFORMS** | Reference sub-flow + label all present. |
| **Mirror**: A→B→**[Y]/n keep**, dashed axis preview | ❌ **NON-CONFORMANT** | Keep-prompt **missing**; applies on 2nd pick. §3.1 |
| **Move/Copy**: base→dest, **DDE**, CARD@dest; Copy single-drop | 🟡 **PARTIAL** | Single-drop ✅; **DDE missing** for Move/Copy. §3.2 |
| Base/pivot pick: osnap+grid, **no card**; 2nd+ pick: **+card** | 🟡 | Card gating ✅; **grid + extension snap absent** (sandbox scope). §3.5 |
| New command **aborts** in-progress | ✅ **CONFORMS** | `run_modifier`→`abort_3d` first ([sandbox.rs:397](../cad_solid/examples/sandbox.rs#L397)). |
| Recorder: highlighted set + named picks + snap kind + apply params (§8) | ✅ **CONFORMS** | Minor fidelity gap on snap-kind. §4 |
| 2-stage empty-basket cancel (§0.3) | ❌ | Single-stage today. §3.4 |

**Bottom line:** the hard part landed. **Rotate and Scale now match the spec** — pivot
semantics, typed degrees CCW+, CARD 90° snap, R-reference, C-copy toggle, live ghost +
label. That was §9's flagged-broken priority and it is genuinely done. Remaining work is
the shorter tail: Mirror's keep step, Move/Copy DDE, the two missing preview ghosts, and
selection sophistication (2-stage cancel / window-crossing).

---

## 2. What is now correct (do not regress)

- **Rotate default flow** — `feed` `Ref::None` → `angle_from(first_uv, uv, card)` =
  absolute pivot→cursor angle from local +u, CCW+ ([modify.rs:224](../cad_solid/src/modify.rs#L224),
  [modify.rs:412](../cad_solid/src/modify.rs#L412)); CARD snaps to 90° ([modify.rs:415](../cad_solid/src/modify.rs#L415)). Matches spec §3 exactly.
- **Typed degrees** — `type_value` parses f32 as degrees, CCW+ ([modify.rs:300](../cad_solid/src/modify.rs#L300)); `R`/`C` keywords honored ([modify.rs:291-298](../cad_solid/src/modify.rs#L291-L298)).
- **Reference sub-flow** — 3-pick src1→src2→newdir, `Δθ = normalize(tgt − src)` into (−π,π]
  ([modify.rs:228-242](../cad_solid/src/modify.rs#L228-L242)). Test `rotate_reference_three_picks` proves +90°.
- **Pivot is full-3D** — `apply_rotate`/`apply_scale` take `pivot: Vec3`, axis = `plane.normal()`
  ([modify.rs:353](../cad_solid/src/modify.rs#L353), [modify.rs:378](../cad_solid/src/modify.rs#L378)); a pivot snapped to a raised corner keeps its Z. Good 3D call.
- **Copy = single-drop** — `Feed::Applied` on the destination pick, one duplicate, op ends
  ([modify.rs:214-221](../cad_solid/src/modify.rs#L214-L221)); test `copy_adds_one_feature_then_finishes`. Matches RUST_CAD §2. ✅
- **Enter-confirm fix** — `confirm()` finalises a gather even when the cmd box holds focus,
  driven from both the keyboard ([sandbox.rs:887](../cad_solid/examples/sandbox.rs#L887)) and an empty cmd-line Enter
  ([sandbox.rs:1203-1204](../cad_solid/examples/sandbox.rs#L1203-L1204)). The "trapped in gather" bug is closed.

---

## 3. Gaps / owed (ranked)

### 3.1 Mirror is missing the keep-[Y]/n step  — **highest-value gap**
Spec §5 is a **3-step** op: A → B → `"keep original? [Y]/n"` answered on the cmd line,
canvas clicks ignored at that step. The sandbox applies **on the 2nd pick**
([modify.rs:267-280](../cad_solid/src/modify.rs#L267-L280)) with no keep prompt and always flips *in place* (no copy).
So there is no way to mirror-*and-keep*. This is the one place a user hits a wall.
Add an `AwaitingKeep(a,b)` state mirroring `MirrorState` and a cmd-line answer path
(`""|y|yes|keep`→keep copy, `n|no`→flip in place), exactly like §5.

### 3.2 Move/Copy direct-distance entry (DDE) not wired
`type_value` returns `None` for Move/Copy ([modify.rs:326-327](../cad_solid/src/modify.rs#L326-L327)). Worse UX
consequence: typing a number mid-Move falls through to command parsing and prints
`"unknown command: 42"` while silently keeping the op armed ([sandbox.rs:366](../cad_solid/examples/sandbox.rs#L366),
[sandbox.rs:391](../cad_solid/examples/sandbox.rs#L391)). Spec §1/§2 want "type a distance → along the constrained cursor
direction." Until it's built, at minimum swallow numerics during Move/Copy so the error
line doesn't lie.

### 3.3 Two preview ghosts missing (spec §0.6)
The ghost path only renders **Rotate/Scale** ([sandbox.rs:1412-1432](../cad_solid/examples/sandbox.rs#L1412-L1432)).
- **Move/Copy**: only a baseline segment is drawn ([sandbox.rs:1405](../cad_solid/examples/sandbox.rs#L1405)); there is **no
  translucent `translated(v)` box**, no marching-ants, no base blip. §0.6 wants the moved
  ghost. Easy win — you already have `corners_of` + `ghost_box`; add a `translated` case.
- **Mirror**: no mirrored ghost and no **dashed axis extended past both ends** (§5). Only the
  baseline shows.

### 3.4 Empty-basket cancel is single-stage
`confirm()` on an empty selection cancels immediately ([sandbox.rs:319-320](../cad_solid/examples/sandbox.rs#L319-L320)).
Spec §0.3 is **two-stage**: 1st Enter → `"please make a selection (Enter again to cancel)"`,
2nd → cancel. Low urgency, but it's a documented behavior and cheap to match.

### 3.5 Selection is click-toggle only
No window/crossing drag-select (L→R window / R→L crossing, §0.3) and no mid-session
sub-commands (`all/none/remove/w/c/l/…`, single-letter rewrite). Gather is one-click
`toggle_select` ([sandbox.rs:1267-1270](../cad_solid/examples/sandbox.rs#L1267-L1270)). Also **grid snap + extension-track**
from the §0.5 priority chain (`osnap > ext > CARD > grid > raw`) are absent — only vertex
osnap + raw plane exist. All acceptable *sandbox scope*, but each is a **1:1-merge risk**:
app users expect them, so track them explicitly, don't let the merge assume parity.

---

## 4. Recorder (§8) — conforms, one fidelity nit

All three §8 requirements are met:
1. **Highlighted set with handles** — `begin_queued` logs `sel={:?}` ([sandbox.rs:668](../cad_solid/examples/sandbox.rs#L668)). ✅
2. **Named pick + world point + snap** — `"{op} {PICK} = (x,y,z) [snap=…] → {Feed}"`
   logged *before* `feed` mutates ([sandbox.rs:1292-1295](../cad_solid/examples/sandbox.rs#L1292-L1295)). ✅
3. **Apply params** — `"{op} ✓ {last_summary}"` ([sandbox.rs:1304](../cad_solid/examples/sandbox.rs#L1304)); before/after 8-corner
   screen dumps ([sandbox.rs:1308-1314](../cad_solid/examples/sandbox.rs#L1308-L1314)). ✅

**Nit:** `snap` is hard-coded to `"END"` whenever a vertex is hit ([sandbox.rs:1287](../cad_solid/examples/sandbox.rs#L1287)),
and `snap_3d` ignores the declared running set (`snap_enabled: SnapSet::defaults()` =
END/MID/CEN/QUA, [sandbox.rs:184](../cad_solid/examples/sandbox.rs#L184)) — it only does raw mesh-vertex snapping. So the
snap-kind readout is decorative and `snap_enabled` is dead. Fine for now, but don't let a
dump's `[snap=END]` be mistaken for real MID/CEN fidelity.

---

## 5. Intentional divergence to DOCUMENT (not a bug)

Move/Copy now use the **full 3D delta** `to − from` (`world_delta_carded`,
[modify.rs:442-452](../cad_solid/src/modify.rs#L442-L452)). The 2D app (the "byte-identical" ground truth) works in pure
`(u,v)`. This is a **sensible 3D extension** — snap base=top-corner, dest=bottom-corner and
the solid correctly drops by its height — and it preserves the *workflow* (§0's golden rule
is about workflow, not the dimensionality of the math). **But it means Move/Copy no longer
match RUST_CAD's math**, and a merge engineer reading "identical to the app" will be
surprised. **Action:** add a short 3D-addendum to `BASIC_MODIFIERS_RULES.md` stating that
base/pivot picks store full 3D and Move/Copy translate in 3D. Make the divergence explicit
in the contract so the 1:1 merge stays honest.

---

## 6. One design question for you (spec is 2D-only, so it can't answer this)

With **CARD on** and an **out-of-plane snapped destination**, `world_delta_carded` locks the
in-plane `(u,v)` to its dominant axis **but preserves the full normal (Z) component**
([modify.rs:447-451](../cad_solid/src/modify.rs#L447-L451)). So a "carded" move can still travel freely in Z. Is that the
intent (CARD = in-plane discipline only), or should CARD in 3D also flatten the move to the
construction plane (drop the Z)? Both are defensible; the 2D spec doesn't cover it. Tell me
which and I'll write it into the contract.

---

## 7. Recommended next moves (in order)

1. **Commit the current green tree** — stop the moving target. Split note: the 2D→3D-delta
   change is behavioral; consider its own commit message line so history is legible.
2. **Mirror keep-[Y]/n** (§3.1) — the only op with a dead-end today.
3. **Move/Copy + Mirror preview ghosts** (§3.3) — cheap, high perceived-fidelity.
4. **Move/Copy DDE** (§3.2) — or at least stop the lying error line.
5. Document the 3D-delta divergence (§5) and answer the CARD-Z question (§6).
6. Defer (sandbox-scope, merge-time): 2-stage cancel, window/crossing, grid + ext snap.

*No code was changed by this review — MD only, per role.*
