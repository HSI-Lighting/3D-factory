# TODO — land the bbox cache safely, 2026-07-17

**Status:** design agreed, **not started.** The change itself is written and parked in
`git stash@{0}` (see `DAY_REPORT_2026-07-17.md` §7). This file is the ordered plan for
landing it, plus the reasoning we discussed so the "why the order" isn't lost.

**Owner ruling needed on:** whether to do the prerequisite (step 2) or ship the cache as-is.
My recommendation is below — do not skip step 2.

---

## The task, in order

- [ ] **1. Prove the cliff is real** — a test, not an argument.
      Mutate a dobject's geometry, **deliberately skip** setting `index_dirty`, then assert:
      with the cache the object **vanishes** from the draw; without the cache it **survives**.
      Right now §7 is reasoned from the code, not demonstrated. This test makes it airtight
      (or disproves me).

- [ ] **2. Make `index_dirty` unmissable** — the prerequisite.
      Route every geometry-changing edit through **one choke point** that sets
      `index_dirty = true`, so "mutate without marking dirty" stops being a reachable state.
      The pattern already exists in this codebase: the recorder's `push()` is exactly this —
      one function nothing can bypass. Add a test that a mutation path which skips the flag
      fails.

- [ ] **3. Land the cache** — unstash `git stash@{0}`, confirm the step-1 test now passes
      *safely* (object survives because the flag can't be missed), commit to `3d-factory`.
      **Win: 16.00 → 1.45 ms/frame, ~11×.**

**Do 2 before 3.** Doing 3 first trades a correct-but-slow renderer for a fast one that
vanishes geometry the first time someone adds an edit path and forgets one line.

---

## Why — the short version of what we discussed

**What the cache is.** `UniformGrid` already computes every dobject's bbox during its build
sweep, then throws it away. The stash keeps it (`bboxes: Vec<(Vec2,Vec2)>`) so the draw loop
reads it back instead of recomputing `e.bbox()` per candidate per frame. That recompute was
~11 ms of a 17 ms zoomed-out draw — 65 ns × 172,080 candidates, computing bounds for objects
the sub-pixel cull then discards (`drawn=0`). Hence the 11×.

**The one thing holding it up correct:**
```rust
let idx_bbox = if self.index_dirty { None } else { self.index.as_ref() };
```
The cache is read **only when the index is clean.** Everything rides on `index_dirty` being
true whenever geometry changed.

**The precise risk** (sharper than the day-report's first wording):
Today, the live `e.bbox()` in the draw loop is a **self-correcting safety net** — even with a
stale index, the two culls (sub-pixel, frustum) run against *live* bounds, so the picture you
see is correct; only snapping/hit-testing would be stale. **The cache removes that net.** It
introduces one new failure class:

> A geometry edit that changes an object's **bbox but not its cell** — a vertex edit, a
> stretch, an in-place scale — where `index_dirty` was missed. **Today:** drawn correctly
> (live bbox). **With the cache:** the stale bbox drives the culls, the frustum or sub-pixel
> test wrongly fires, and **the object silently vanishes** — even though the candidate query
> found it.

So it's not "makes all bugs silent." It's precise: **it removes the last live-truth checkpoint
in the render path, so the picture itself now depends on `index_dirty` being perfect.**

**Why that specifically worries me here — two facts from today, not hypotheticals:**
1. The "Draw on this face" crash **was** a missed-dirty stale index on a doc swap (fixed in
   `07120ac`). We have direct evidence the flag gets missed in *this* codebase.
2. `update()` — the incremental path that keeps the cache coherent — **almost never fires**
   (day-report §4.3); real moves fall back to full rebuild. So freshness rides entirely on
   `index_dirty` → `ensure_index()`. One flag carries everything.

Adding a cache whose correctness depends on a flag we watched get missed *today* is the wrong
order. Fix the flag first.

**Credit where due:** the accessor is safe. `g.bbox(i)` uses `.get(i).copied()` → `None` on
out-of-range → falls back to `e.bbox()`. It **cannot** reproduce today's `index out of bounds`
panic. The cache adds no new *crash* surface — only the vanishing-geometry surface above.

---

## Files

- Parked change: `git stash show -p stash@{0}` — `cad_app/src/app.rs` (draw loop) +
  `cad_kernel/src/spatial.rs` (`bboxes` field, `bbox(i)`, `update()` coherence, bench).
- The stash already ships a `#[ignore]` bench `draw_loop_bbox_cache_vs_recompute` modelling
  the owner's real 1.5M line+circle+polyline mix. Step 1's test is different — it proves the
  **correctness cliff**, not the speed.

## Run
```bash
cd ~/workspace/simLUX/3D_Factory
git stash show -p stash@{0}          # the parked change
cargo test --workspace
```
