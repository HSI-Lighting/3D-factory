# For the coding agent of the ORIGINAL repo — recorder + performance work

**From:** `3D_Factory` (a full copy of the simLUX workspace, used to build 3D inside the real 2D app)
**Date:** 2026-07-16
**Read this if:** you maintain `simLUX` / `RUST_CAD` and want the session-recorder and
performance work **without** the 3D engine.

> **This is NOT a merge request.** The 3D work stays here. Everything below lives entirely in
> `cad_app/src/{app.rs, dbg_recorder.rs}` and `cad_kernel/src/spatial.rs`, has **zero 3D
> dependency**, and can be cherry-picked à la carte.
>
> Every number here was **measured on a real 1.5M-dobject session**, not estimated. Where I was
> wrong, I've said so — those entries matter more than the wins.

---

## 0. TL;DR — take these, in this order

| # | Commit | What | Win | Risk |
|---|---|---|---|---|
| 1 | `bf50412` | **Selection lookup in the draw loop was O(n²)** | **21,815 ms → 25 ms** | none — pure |
| 2 | `f06e976` | Index rebuild sweeps `bbox()` 3× → 1× | 417 → 233 ms | low — proven identical |
| 3 | `0bb22a6` | Recorder: snapshot cloned the whole `Document` (nothing read it) | O(n) → O(1) | none |
| 4 | `27d0ba5` | Recorder: `🐢 SLOW FRAME` breakdown | — | none, diagnostic |
| 5 | `5aaa09e` | Recorder: per-command execution time | — | none, diagnostic |
| 6 | `64cdbc4` | Recorder: `# Count dobjects ONLY` toggle | dump 7.9 MB → 175 B | none |
| 7 | `72aca9a` | Recorder: `🗂 INDEX REBUILD` + duration | — | none, diagnostic |
| — | `1c5dec5` | Incremental index update | **~0 in practice** — see §4 | see §4 |

**If you take exactly one thing, take #1.** It is a 900× frame-time win, ~20 lines, and it
affects any drawing where a user selects a lot of objects.

---

## 1. ⭐ THE BIG ONE — `selection.contains(&i)` in the draw loop (`bf50412`)

**The bug:** `self.selection.contains(&i)` is a **linear scan of a `Vec`**, and the CPU draw loop
ran it **per drawn dobject, per frame**, at **four** sites.

Measured, from a real dump — identical work, 800× apart:
```
🐢 SLOW FRAME    27.5 ms  (query 2.3 · draw    25.2)  candidates=143856 drawn=141766
🐢 SLOW FRAME 21817.5 ms  (query 2.2 · draw 21815.4)  candidates=143856 drawn=141766
```
The only difference between those two frames was a `✓ SEL 0 → 52650` immediately before.

The arithmetic matches the observation to 3% — a consistent **~3 ns/comparison**, the fingerprint
of a linear scan:

| drawn × selected | comparisons | observed draw |
|---|---|---|
| 141,766 × 0 | 0 | 25.2 ms |
| 141,766 × 52,650 | **7.46 B** | **21,815 ms** |
| 141,766 × 43,840 | **6.22 B** | **18,783 ms** |

**Symptom in the wild:** window-select a few tens of thousands of objects and the app freezes for
**20 seconds**. It also made MOVE unusable — the selection the command needs is the very thing that
froze the frame.

**The fix:** a `sel_mask: Vec<bool>` built **once per frame** → the draw test is O(1). The
allocation is REUSED (`clear()` + `resize()`, no realloc), so the per-frame cost is an O(n) memset
(~0.1 ms at 1.5M) + O(selection) to fill, instead of O(drawn × selection).

```
141766 drawn ×     0 selected → scan     5.5 ms · mask 1.15 ms (    5× faster)
141766 drawn × 43840 selected → scan 20593.7 ms · mask 0.99 ms (20841× faster)
141766 drawn × 52650 selected → scan 24591.3 ms · mask 1.34 ms (18360× faster)
```

⚠️ **`selection.contains()` appears elsewhere too** (`add_window_selection`, grip paths). I only
fixed the four in the draw loop, because that is where the per-frame multiplier lives. **Grep for
the rest** — the same trap is likely in any per-dobject loop.

---

## 2. Index rebuild: `bbox()` swept 3× → 1× (`f06e976`)

`ensure_index` called `auto_cell_size(d, ..)` then `build(d, cs)`. Between them `bbox()` was swept
**three times** — cell-size pass, world-bbox pass, bucketing pass. On real geometry that dominates,
because `bbox()` is **O(verts) for a polyline** and trigonometric for arc/ellipse:

```
=== 1500000 dobjects (line/circle/arc/ellipse/point/polyline mix) ===
  ONE bbox() sweep         :  98.2 ms   (×3 per rebuild = 295 ms)
  auto_cell_size           : 102.7 ms
  build                    : 314.7 ms
  REBUILD TOTAL (3 sweeps) : 417.4 ms   ← real dumps showed 386-397 ms
  ↳ bbox() is 71% of it
  build_auto (1 sweep)     : 233.1 ms   → 1.76× faster
```

`UniformGrid::build_auto(dobjects, target_per_cell)` caches the bboxes for the build (freed on
return) and derives the cell size from the same pass. `ensure_index` calls it instead of the pair.

**Correctness:** `build_auto_matches_auto_cell_size_plus_build` asserts identical cell size, grid
shape, and identical query results across **196 probes**. It is a pure speed change; a differing
index breaks picking **silently**, so please keep that test if you take this.

> ⚠️ **Benchmark on REAL geometry.** My first benchmarks used flat lines and reported 163 ms while
> reality was 397 ms — a 2.4× error that sent me optimising the wrong thing. Polylines/ellipses are
> where the cost is.

---

## 3. Session recorder — new version

All additive to `DbgEvent` / `WatchedState`. Independent of each other.

### 3.1 The five core modifiers were NEVER watched (`5aaa09e`)
`WatchedState` had `tool`, `select_mode`, `trim_state`, `fillet_state`… but **not `move_state`,
`copy_state`, `rotate_state`, `scale_state`, `mirror_state`.** So a dump of a MOVE showed
`⌨ CMD "move" → Move` and then **silence** — the one state that was live was the one field the
recorder never printed. Users reported "move does nothing"; it was working fine and invisible.

Now watched. A MOVE is reconstructable end-to-end:
```
⌨ CMD "m" → Move  [Typed]  ⏱ 70 µs
🔁 select_mode "Off" → "ForSelect"
🔁 move_state "Off" → "WaitingForBase"
🔁 move_state → "WaitingForDest(Vec2 { x: 68.3, y: 32.6 })"
💾 UNDO-SNAP depth=2
🔁 move_state → "Off"
```

### 3.2 Per-command execution time (`5aaa09e`)
`CmdRun` gains `elapsed_us`, rendered inline; ⚠ SLOW past 16.7 ms (one frame @ 60 Hz — where a
stall becomes visible):
```
⌨ CMD "move" → Move  [Typed]  ⏱ 24 µs
⌨ CMD "E" → DeleteSelected  [Typed]  ⏱ 102.2 ms ⚠ SLOW
```
**`run_command` is re-entrant** (a select session rewrites `p`/`l`/`d` into
`run_command("previous")`). Patching "the last CmdRun" lets a nested call steal the outer command's
slot — so it is patched **by event index**, and an outer command's time correctly *includes* the
nested one. There's a test.

### 3.3 Snapshots cloned the whole Document — and nothing read it (`0bb22a6`)
`DbgSnapshot.doc: Document` was a **full clone per snapshot**, with an auto-snap cadence of every
50 events. **No code ever read it** (snapshots were only `.len()`'d and `.last().event_index`). At
1M+ dobjects that is not a big file — it's a freeze, *while recording*.

Replaced with the first **20** dobjects' geometry via the app's `describe_verbose` (full
coordinates — enough to verify the draw functions, which is what snapshots are FOR). Always reports
what it dropped, so a capped dump can never be misread as complete:
```
--- SNAP[1] geometry (auto cadence) — 20 shown, 1499980 MORE OMITTED (cap 20) ---
  #0    h=1      line  a=(-40.000,-20.000)  b=(40.000,20.000)  len=89.443
```
Measured — snapshot cost is now **flat** in drawing size:
```
      1000 dobjects → 29 µs · dump 1606 bytes
    100000 dobjects → 26 µs · dump 1610 bytes
   1000000 dobjects → 32 µs · dump 1612 bytes
```

**Same commit:** `WatchedState.selection: Vec<usize>` was **cloned every frame** while recording and
**never diffed** (absent from the `diff_field!` list). 1M selected ⇒ an **8 MB allocation per
frame** for a value nobody read. Removed outright — selection changes are already captured at the
source by `SelectChange { basket_before, basket_after, cause }`, which is strictly better
information at zero polling cost.

### 3.4 `# Count dobjects ONLY` toggle (`64cdbc4`)
`SelectChange`/`GestureClassification` carry `Vec<usize>` baskets, and a GESTURE prints its
selection **twice** (before → after). A real 1.2M dump could not be pasted — it blew a 50k-char
limit. The toggle keeps the **count**, never the list, dropping it **at capture** (in
`DbgRecorder::push`, the single choke point every event passes through) so the **recorder's own
memory** is bounded too, not just the text:
```
        916 selected → verbose      4628 bytes · counts-only  171 bytes
     100000 selected → verbose    689048 bytes · counts-only  174 bytes
    1000000 selected → verbose   7889048 bytes · counts-only  175 bytes
```
Defaults **OFF** — a debugging aid must not silently change what a normal session records.

### 3.5 `🐢 SLOW FRAME` breakdown (`27d0ba5`)
```
🐢 SLOW FRAME 23.4 ms  (query 2.7 · draw 17.3)  candidates=172080 drawn=0
🐢 SLOW FRAME 19.1 ms  (query 1.4 · draw 14.4)  candidates=81326 drawn=20000  ⚠ DRAW CAPPED
```
Emitted **only** when a frame exceeds one refresh — free on healthy frames, cannot flood the dump.
Two `Instant`s per frame (~40 ns). **This is what found §1.** Before it, "zoom/pan is slow" was
unfalsifiable: dumps showed clicks seconds apart with no way to separate frame time from
think-time.

### 3.6 `🗂 INDEX REBUILD` + duration (`72aca9a`)
```
🗂 INDEX REBUILD 1500000 dobj  ⏱ 397.3 ms  ⚠ SLOW
```
It previously logged only to the command history, so a dump showed the ~100 ms undo clone and
**hid the ~390 ms index rebuild beside it** — half the per-edit cost was invisible. It fires from
~58 `index_dirty` sites, i.e. essentially every edit.

---

## 4. ⚠️ What NOT to take, and why

### `1c5dec5` — incremental index update: **~zero value in practice**
`UniformGrid::update(dobjects, changed)` re-buckets only changed dobjects — **453× on paper**
(166 ms → 0.37 ms at 1.5M). **It does not fire for real moves.**

`build()` fits the grid to the drawing's **exact** bbox, so *any outward move leaves it* and
`update()` correctly rejects → full rebuild. A real dump: base `(-5492, 10665)` → dest
`(-31241, -10933)`, a delta of `(-25749, -21599)` on a drawing spanning 0..+50000. Moving things
far away is the **normal** case, not the corner case.

It costs `ranges: Vec<[u32;4]>` (16 B/dobject ≈ 24 MB at 1.5M). **Take it only if** you also pad
or grow the grid; otherwise it's memory for nothing. `f06e976` depends on some of the same code —
check the diff before cherry-picking either in isolation.

### The bbox cache — **NOT committed, deliberately**
Caching each dobject's bbox in the grid measured **16.00 ms → 1.45 ms per zoomed-out frame** (the
draw loop recomputes `bbox()` per candidate, ahead of the sub-pixel cull, for dobjects it then
throws away — see §5.2). **I did not commit it**: it means a stale index yields a *wrong bbox*
rather than merely wrong candidates — i.e. it widens any missed `index_dirty` into "geometry
silently disappears". That needs an audit of all ~58 invalidation sites first. Mentioned so you
know the win exists and why it's parked.

### A failed optimisation, recorded so you don't repeat it
I hypothesised the grid's `cells: Vec<Vec<u32>>` (one alloc per cell, ~150k per rebuild) was the
cost and converted it to a flat CSR layout. **It was a 34% REGRESSION (111 → 150 ms).** The cost is
not allocation — it's ~123 ms of **random scatter into 150k scattered Vecs** (cache misses), which
CSR performs identically while adding a 24 MB temporary. **Don't re-try CSR here.**

---

## 5. Bugs found but NOT fixed — you should know about these

### 5.1 ⛔ `HANDLE_COUNTER` collision — **live data-corruption risk, exists in RUST_CAD too**
```rust
static HANDLE_COUNTER: AtomicU64 = AtomicU64::new(1);   // starts at 1 EVERY session
pub fn next_handle() -> Handle { HANDLE_COUNTER.fetch_add(1, Ordering::Relaxed) }
```
Nothing raises it after a load, but RSM **preserves** handles. Proven:
```
PROBE: loaded handle = 1000000, next_handle() = 2
```
Open a drawing → the next object drawn is handed a handle a loaded object already owns.
**`Hatch.boundary_handles` resolves its boundary BY HANDLE** (`rsm.rs`), so a collision can bind a
hatch to the **wrong geometry**.

Kept as an `#[ignore]`d known-bug test: `cargo test -p cad_io -- --ignored`
(`known_bug_next_handle_collides_with_a_loaded_handle`).

**Fix is additive, ~5 lines:**
```rust
pub fn reserve_handles_above(max: Handle) {
    HANDLE_COUNTER.fetch_max(max.saturating_add(1), Ordering::Relaxed);
}
```
called at the end of `read_rsm` with the max loaded handle. **Please take this one.**

### 5.2 Zoom-extents on a large drawing renders a BLANK SCREEN
```rust
if bbox_px < 1.0 && self.selected != Some(i) && snap_source != Some(i) && !in_selection {
    skipped += 1;
    continue;          // skipped entirely — not even a dot
}
```
At zoom-extents on 1.5M dobjects every dobject is sub-pixel, so **everything is culled and nothing
draws** — the dump's `candidates=172080 drawn=0`. The only visible thing is what you've selected
(`!in_selection` is the exception — hence `drawn=1` after selecting one). A user could not tell
whether a MOVE had worked, because at that scale the screen is empty. Consider drawing a 1px dot
instead of skipping (costs `drawn` going 0 → 172,080), or route users to `RenderMode::Apx`.

### 5.3 Undo clone: 300 MB / ~80 ms **per edit**
```
💾 UNDO-SNAP depth=1 (~300001792 bytes)
🧠 snapshot_doc (Document clone) ~300001792 bytes in 78546 µs
```
`snapshot_doc()` clones the entire `Document` on every mutating command, and **each undo level
holds another 300 MB** (10 undos ≈ 3 GB). This is now the largest single per-edit cost. Real
feature, not waste — the fix is a different shape (deltas, not snapshots). Untouched: it changes
the established undo system.

### 5.4 `SelDmTm` demotes fast drags to clicks
```
⚠ 47-px drag DEMOTED TO CLICK. Press position discarded … Drag-window intent LOST.
⚠ 91-px drag DEMOTED TO CLICK …
```
Held 96 ms and 134 ms; `SelDmTm` default is **250 ms**. Per
`COMMAND_LINE_AND_MOUSE_RULES` Part B a drag shorter than `SelDmTm` is a click, so a fast user's
window-drag silently becomes a two-click window. **Working as specified** — but the recorder itself
flags it "intent LOST", so the spec may be wrong for a fast user. Owner's call.

---

## 6. Doc errata found while working (worth fixing upstream)

- **`COMMAND_LINE_AND_MOUSE_RULES.md` §A.1** claims *"there is no Space-as-Enter in this codebase"*.
  **False.** Verified at `app.rs:12693-12699` (`submit_via_space`, non-empty) and `app.rs:25696-25711`
  (`trigger = enter_now || (space_now && cmd_is_empty && !in_text_body)`, the empty cascade).
  `COMMAND_LINE_CURRENT.md` is right. *(Since fixed upstream by `83ff025`.)*
- **`run_command` has 27 intercepts**, not "~14" as `COMMAND_LINE_CURRENT.md` §5 says.
- **`COMMAND_LINE_CURRENT.md` §4**: *"`line 0,0 10,10` draws at once"* is true of the **parser** and
  false of the **keyboard** — Space=Enter submits at `line`, so every space-separated form is
  **typed-unreachable** (paste/button-dispatch only). That's the AutoCAD model, but write it down.

---

## 7. How to take it

```bash
git remote add factory <this repo>
git fetch factory
git cherry-pick bf50412            # ⭐ the O(n²) draw-loop freeze — take this first
git cherry-pick 0bb22a6 5aaa09e 64cdbc4 27d0ba5 72aca9a   # recorder
git cherry-pick f06e976            # index rebuild 1.76× (check its spatial.rs overlap with 1c5dec5)
cargo test --workspace             # 194 kernel + full suite must stay green
```
Every one of these touches only `cad_app/src/{app.rs,dbg_recorder.rs}` and
`cad_kernel/src/spatial.rs`. **None of them reference the 3D layer.** The 3D commits
(`cf4b079`, `6a6ecb6`, `d54ad70`, `07120ac`, `c3a370b`, `db712c8`, `3fe73c7`, `869bec5`, `ffa8365`)
are the ones to leave behind.

**Benchmarks travel with the code** — `#[ignore]`d, so they never slow CI:
```bash
cargo test -p cad_app   -- --ignored --nocapture   # frame / query / selection / rebuild costs
cargo test -p cad_kernel -- --ignored --nocapture   # bbox-cache bench
cargo test -p cad_io    -- --ignored               # ⛔ the handle-collision bug (§5.1)
```
Please keep them. Every performance claim in this document is one of these, and **two of my three
optimisation guesses were wrong until I measured** — the benchmarks are the only reason that was
caught rather than shipped.
