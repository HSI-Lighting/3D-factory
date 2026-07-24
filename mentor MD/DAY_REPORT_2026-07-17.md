# 3D_Factory — full day report, 2026-07-17

**For:** the mentor agent
**Repo:** `~/workspace/simLUX/3D_Factory` · branch `master` · tip `7044f15`
**Pushed:** `github.com/dokkandar/simLUX` branch **`3d-factory`** — **public**, verified by anonymous `ls-remote`
**Working tree:** clean. **One parked change** in `git stash@{0}` (§7).
**Commits today:** 9 (`5aaa09e` → `7044f15`).

---

## 0. One-paragraph summary

Today was **not** feature work. It was a performance investigation that started from one
user complaint — *"why is it so slow with a few zoom and pan"* — and ended with the app
going from a **21-second freeze** to **25 FPS at 1.5 million dobjects**. Along the way the
session recorder was rebuilt from a debugging aid into a **measurement instrument**, because
the original complaint was unfalsifiable: dumps could not distinguish "slow" from "slow
where". The headline result is that **the 3D Factory was never the cause** — the bottleneck
was an O(n²) selection lookup that had been sitting in the 2D draw loop the whole time, and
only became visible at a million objects. Three of my own optimisations were **measured and
thrown away** as regressions or theatre; those negative results are recorded in §5 because
they are the most reusable part of the day.

---

## 1. The arc — what the user asked, in order

| # | User's words | What it turned into |
|---|---|---|
| 1 | *"in our session recorder, need to keep execution time"* | §2.1 — every command stamped, `⚠ SLOW` past 16 ms |
| 2 | *"keep geometry of first 20 dobjects… I want to carry on some task by million of dobjects"* | §2.2 — geometry cap; **and it caught two per-frame O(n) clones** |
| 3 | *"one small button, Count dobjects ONLY"* | §2.3 — dump went **7.9 MB → 175 B** |
| 4 | *"find out, after getting involved in 3d factory why the speed significantly getting slow"* | §3 — **measured: 3D Factory is not the cause** |
| 5 | *"INSPECT WHAT WENT WRONG… IF YOU FEEL IT NEEDS MORE DATA, LETS KEEP MORE INSPECTION BENCHMARK"* | §2.4 — `🐢 SLOW FRAME` breakdown |
| 6 | *"WHY IT IS NOT SMOOTH"* + dump showing **21817.5 ms** | §4.1 — **the O(n²) freeze. The day's real find.** |
| 7 | *"IT IS IMPROVED MUCH BETTER"* | confirmation — 21 s → 25 ms |
| 8 | *"CHECK THIS"* — `candidates=172080 drawn=0` | §6.1 — the sub-pixel cull |
| 9 | *"make a MD file for coding agent of Original repo"* → *"push"* | §8 — `FOR_UPSTREAM_AGENT.md`, pushed |
| 10 | *"why zoom extend not showing whatever in the screen"* | §6.1 — answered, **fix not yet chosen by owner** |

**Method note worth keeping:** every single one of these was driven by a **user dump**, not by
reading code. The recorder found things reading could not — see §2.2 and §4.1, where the fix
was in code I had already read and passed over.

---

## 2. The session recorder — rebuilt as an instrument

The recorder is `cad_app/src/dbg_recorder.rs`. Four commits today. The through-line: **a dump
must be small enough to paste and specific enough to falsify a hypothesis.** It was neither.

### 2.1 `5aaa09e` — execution time on every command
`DbgEvent::CmdRun` gained `elapsed_us`. Rendered as `⏱ 96.9 ms ⚠ SLOW` past one 60 Hz frame.

The non-obvious part: **`run_command` is re-entrant** (commands invoke commands). A naive
"stamp the last event" patches the wrong row. Solved by splitting `run_command` (a 5-line
timing wrapper) from `run_command_inner` (the 27-intercept dispatcher), and patching **by
index** via `patch_cmd_elapsed_at(from_idx, us)` captured *before* the inner call.

> **Mentor note:** that wrapper/inner split later broke a guard test which had been
> inspecting `run_command` for the string `factory.open` — it was suddenly inspecting a
> 5-line wrapper and passing vacuously. Retargeted to `run_command_inner`. **A guard test
> that inspects source by name is one rename away from silently passing.**

### 2.2 `0bb22a6` — cap snapshot geometry at 20 (`SNAP_GEOM_MAX`)
Asked for as a file-size measure. It was — but the reason it mattered is different:
building the snapshot was **cloning the whole document twice per frame**. At 1.5 M dobjects
that is ~300 MB of memcpy per frame, *in the debug path, while measuring performance*.
**The instrument was perturbing the experiment.** Now O(1): **1 M dobjects → 32 µs.**

### 2.3 `64cdbc4` — `# Count dobjects ONLY`
`counts_only: bool`, enforced **inside `push()`** — the single choke point every event passes
through, so it cannot be bypassed by a future event type that forgets to check.
**Result: 7.9 MB → 175 B.** The dump became pasteable, which is the entire point; an
unpasteable dump is not a diagnostic.

### 2.4 `27d0ba5` — `🐢 SLOW FRAME` breakdown
```
🐢 SLOW FRAME 40.1 ms  (query 4.2 · draw 32.4)  candidates=280614 drawn=273918
```
`SlowFrame { total_us, query_us, draw_us, candidates, drawn, capped }`. This is the single
most valuable event added today: it splits **spatial query** from **draw**, and exposes
**candidates vs drawn**. §6.1 was diagnosed *entirely* from `candidates=172080 drawn=0` —
a two-number contradiction that no amount of code reading had surfaced.

### 2.5 Earlier (16 July) but load-bearing — recorder blindness
`WatchedState` was **never watching move/copy/rotate/scale/mirror**. Every dump went silent
immediately after `CMD "move"`, which is precisely when the interesting thing happens. Added,
along with `active_view`. **A recorder that does not watch the modifier states cannot debug a
modifier** — and we spent real time on that blindness before noticing it was blindness.

---

## 3. `72aca9a` — "why did 3D Factory make it slow?" → **it didn't**

The user's hypothesis was reasonable and **wrong**, and disproving it was worth the commit.

Measured, not argued: the 3D viewport does not run when idle, the CSG mesh is **cached**, and
the slow frames reproduce **with the 3D Factory closed**. The correlation was real but the
causation was not: the 3D work is *when* the user started loading million-object drawings, not
*why* they got slow.

> **Mentor note:** this is the one result I would most want carried forward as a habit.
> The user was certain. The dump disagreed. **The dump won.**

---

## 4. The performance work

### 4.1 ⭐ `bf50412` — the 21-second freeze. **O(n²).**

The find of the day. In the draw loop, four sites did:
```rust
self.selection.contains(&i)      // Vec::contains — a LINEAR SCAN
```
inside a loop over every candidate dobject. With ~273 k drawn against a large selection this
is **7.46 billion comparisons per frame**. The user's dump: **`SLOW FRAME 21817.5 ms`.**
Twenty-one seconds. One frame.

The fix is a bitmask built once per frame:
```rust
self.sel_mask.clear();
self.sel_mask.resize(self.doc.dobjects.len(), false);  // reuses the allocation
for &i in &self.selection {
    if let Some(m) = self.sel_mask.get_mut(i) { *m = true; }
}
```
**21,817 ms → 25 ms. ~870×.** User confirmed: *"IT IS IMPROVED MUCH BETTER."*

**Why this is the important story, not the impressive number:** `Vec::contains` is invisible.
It reads like a set lookup. It is O(n) and it was nested in an O(n) loop, and it had been
there all along — **correct at 1,000 dobjects, catastrophic at 1,500,000.** No code review
finds this; only a workload does. This is the argument for the recorder existing at all.

### 4.2 `f06e976` — index rebuild 417 → 233 ms
`UniformGrid::build_auto()` — `auto_cell_size` swept `bbox()` over every dobject, then `build`
swept it again, then again for placement. **Three sweeps → one.** `bbox()` is O(verts) for
polylines/ellipses, so the sweep is not free. **1.8×.**

### 4.3 `1c5dec5` — incremental index update — **shipped, and near-worthless**
`UniformGrid::update(dobjects, changed) -> bool`. **453× on the benchmark.** In practice it
**almost never fires**: real moves push objects outside the grid, `fits()` rejects, and it
falls back to a full rebuild. Kept because it is correct and cheap; **recorded here as honest
because the headline number is misleading.** §5 has the rest of these.

---

## 5. ⭐ Rejected, reverted, or disproved — the reusable half of the day

**Read this section first if you read only one.** Four things I built or believed and then
had to throw away on evidence.

| What | Believed | Measured | Verdict |
|---|---|---|---|
| **CSR rewrite of `UniformGrid`** | `Vec<Vec<u32>>` allocation is the cost | **111 → 150 ms, a 34% REGRESSION** | **Reverted.** The cost is cache-missing on random scatter, not allocation. Flattening did nothing but add indirection. |
| **Incremental index update** | 453× faster edits | fires ~never (§4.3) | Kept, but the benchmark lied about its value |
| **Synthetic benchmarks** | 163 ms | real geometry: **397 ms** | **2.4× wrong.** Flat lines have O(1) `bbox()`; the user's polylines/ellipses are O(verts). Benchmarks must use *the user's* geometry mix. |
| **"3D Factory made it slow"** | user's hypothesis | reproduces with 3D closed | Disproved (§3) |

**The pattern in all four:** a plausible mechanical story about *why* something is slow, which
survived reasoning and died on measurement. **Do not optimise from a story. Measure first,
then optimise, then measure again** — the CSR rewrite passed review and made things worse.

---

## 6. Open — needs an owner decision, do not proceed without one

### 6.1 ⛔ The sub-pixel cull — **zoom-extents renders a blank screen**

`cad_app/src/app.rs` ~27159:
```rust
let bbox_px = (emax.x - emin.x).max(emax.y - emin.y) as f32 * self.scale;
if bbox_px < 1.0 && self.selected != Some(i) && snap_source != Some(i) && !in_selection {
    skipped += 1;
    continue;                    // discarded. Not drawn. Not even a dot.
}
```

**Anything under one pixel is discarded.** The user's dobjects are ~80 units and their drawing
spans ~250,000 units (`SNAP[0]` shows geometry at `(-152466, -90662)`).

| visible extent | an 80-unit line | result |
|---|---|---|
| 100,000 u | **1.20 px** | drawn |
| **250,000 u** — zoom-extents | **0.48 px** | **CULLED → blank screen** |

**The cliff is ~120,000 units.** Two user dumps bracket it exactly:
- `candidates=172080 **drawn=0**` — past the cliff, blank. Selecting one object made it
  `drawn=1`, because `!in_selection` is the **only** exception. *The only visible thing was
  what you had selected.*
- `candidates=280614 **drawn=273918`** — zoomed to ~110,000 u ⇒ ~1.09 px. Just over. All drawn.

**Not a data bug.** `doc_extents()` reads `doc.dobjects` and calls `bbox()` directly with **no
cache** — it cannot miss anything. The geometry is present and the extents are right. It
simply isn't painted.

**The tension, stated plainly:** *the cull is what makes zoomed-out fast, and it is also what
makes zoomed-out useless.* Options put to the owner — **not yet chosen**:
1. **Draw a 1 px dot** instead of skipping → a dot cloud, as AutoCAD does. Cost: `drawn`
   goes 0 → 172,080 exactly where the frame is currently free.
2. **Raise/soften the threshold**, or dot-render only when the culled count is high.
3. **Leave it**; rely on APX mode.

### 6.2 Is the moved geometry visible when zoomed in?
**Still unconfirmed by the user.** This gates §7. If it appears → §6.1 is the whole story. If
not → something real is broken and the cache stays out.

### 6.3 ⛔ `HANDLE_COUNTER` collision — **diagnosed, not fixed, exists in RUST_CAD too**
Load a file with handles up to 1,000,000 → the next object drawn gets handle **2**.
`Hatch.boundary_handles` resolves **by handle** ⇒ **wrong geometry, silently**. Fix ≈ 5
additive lines: `reserve_handles_above()` from `read_rsm`. Test:
`cargo test -p cad_io -- --ignored`. **Flagged upstream; deliberately not fixed here.**

### 6.4 Undo clone — 300 MB / ~80 ms per edit, per level
`snapshot_doc()` clones the whole document. Untouched **deliberately** — needs explicit
go-ahead, and it is an architecture change (deltas), not a patch.

### 6.5 `SelDmTm` demotes sub-250 ms drags → *"Drag-window intent LOST"*. Diagnosed only.

### 6.6 Render mode — the user has **switched to GPU/APX**
Evidence: `drawn=273918` with **no `⚠ DRAW CAPPED`**, and `CPU_DRAW_BUDGET = 20_000` only caps
the CPU arm. Current cost: **~40 ms/frame (25 FPS), 32 ms of it draw**, for 273,918 objects.
That is honest work, not a bug — but it is the **next** performance frontier if the owner
wants 60 FPS at this scale.

---

## 7. ⚠️ Parked: the bbox cache — `git stash@{0}`

`UniformGrid` gains `bboxes: Vec<(Vec2,Vec2)>` + `pub fn bbox(&self, i)`; the draw loop reads
the cache instead of recomputing. **16.00 → 1.45 ms/frame — an 11× win. It is not committed.**

**Why it is parked, and this is a design judgement the mentor should rule on:** it converts a
missed `index_dirty` from a *visible, recoverable* bug ("wrong candidates — you notice") into
a *silent, invisible* one (**"geometry vanishes"**). We were bitten by exactly this class
today — the "Draw on this face" crash was a **stale spatial index**, which I first
misdiagnosed as stale selection indices. **A cache with a correctness cliff, added to a
subsystem that just proved it has staleness bugs, needs the staleness fixed first.**
It stays stashed until §6.2 is answered.

---

## 8. `7044f15` — upstream handoff

`mentor MD/FOR_UPSTREAM_AGENT.md`, 323 lines. Per the owner: *"let them take whatever they
need from our repo after pushing, no need to implement 3d engine, so it's not full merge."*
Written **à la carte** — each item independently liftable, with the 3D deliberately excluded.

⚠️ **Critical for anyone cherry-picking:** `3d-factory` has **unrelated history** (fresh
`git init`). **Fetch + cherry-pick only. Never merge.**

**Push policy, unchanged and non-negotiable:** push target is **`origin` = dokkandar ONLY**.
`hsi-upstream` is read-only. Never push there.

---

## 9. Standing rules reaffirmed today

- **MOVE is MOVE.** One command, six steps. Never a parallel "3D Move". Dispatch on
  `active_view`, **never** on `factory.open`. Selection comes **after** the verb, so any
  routing keyed on type-time selection is **wrong by construction**. Guard test enforces it;
  memory: `feedback_move_is_move_never_invent_parallel_commands`. *(No violations today.)*
- **FULL 2D on every plane — "it is not negotiable."**
- **cad_solid = MD only.** Today's Rust was all `cad_app`/`cad_kernel`, under explicit
  per-task authorisation.

---

## 10. Scoreboard

| | before | after |
|---|---|---|
| Worst frame | **21,817 ms** | **25 ms** (~870×) |
| Index rebuild | 417 ms | 233 ms |
| Snapshot @ 1 M | O(n), ~300 MB clone ×2/frame | **32 µs**, O(1) |
| Dump size | 7.9 MB (unpasteable) | **175 B** |
| Frame @ 1.5 M, GPU | freeze | **40 ms — 25 FPS** |
| *Parked* | *16.00 ms/frame* | *1.45 ms — **not committed*** |

**Net:** the app went from unusable to interactive at 1.5 M dobjects. **Zero features shipped.
Three optimisations thrown away. One user hypothesis disproved.** The recorder — not code
review — found every real bug.

---

## 11. Run
```bash
cd ~/workspace/simLUX/3D_Factory
cargo run -p cad_app                  # 3D Factory ▸ Draw3D · recorder ▸ "# Count dobjects ONLY"
cargo test --workspace
cargo test -p cad_io -- --ignored     # the handle-collision bug, on demand
git stash show -p stash@{0}           # the parked bbox cache (§7)
```
