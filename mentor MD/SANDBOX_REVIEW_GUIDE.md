# 3D Solid Sandbox — Review Guide (for the mentor / reviewer agent)

This tells you **where the sandbox lives, how to review it, and which files are the
authoritative rules** it must conform to. Read this first, then `BASIC_MODIFIERS_RULES.md`.

---

## 1. Where the files are

**Repo:** `~/workspace/simLUX`  ·  **Remote `origin`:** `git@github.com:dokkandar/simLUX.git` (dokkandar, SSH deploy key)
**Branch:** `simlux-3d-sandbox`  ·  **GitHub:** https://github.com/dokkandar/simLUX/tree/simlux-3d-sandbox

> Push target is `origin` (dokkandar) ONLY. `hsi-upstream` is read-only/disabled — never push there.

The sandbox is a **standalone crate** `cad_solid`, deliberately isolated so it can be
built/tested alone now and wired into the monolithic `cad_app` later with a 1:1 merge.

| File | Lines | What it is |
|------|-------|------------|
| `cad_solid/examples/sandbox.rs` | ~2340 | **The sandbox app** (egui/eframe standalone window). Main review target: viewport, select-first flow, 3D modifier picks, flat-sketch split view, session recorder UI, camera. |
| `cad_solid/src/lib.rs` | ~690 | Core model: `Model`, `Feature`, `Plane`, `Frame`, `Sketch`, CSG eval (`csgrs`), ray-picking, AABB. UI-agnostic. |
| `cad_solid/src/modify.rs` | ~570 | **The 3D modifiers** (Move/Copy/Rotate/Scale/Mirror). Conforms to `BASIC_MODIFIERS_RULES.md`. Has unit tests. |
| `cad_solid/src/draw.rs` | ~270 | Interim 2D draw tools for sketches (Line/Pline/Circle/Arc/Rect/Ellipse/Point). To be replaced by the app's verbatim DRAW state machines (Phase C). |
| `cad_solid/src/dbg_recorder.rs` | ~816 | **Session recorder** — copied VERBATIM (byte-identical) from `cad_app/src/dbg_recorder.rs` / RUST_CAD. Do not diverge. |
| `cad_solid/src/csg.rs` | ~134 | Parametric CSG tree (leaf primitives → union/difference/intersection via csgrs). |
| `cad_solid/Cargo.toml` | — | Deps. csgrs pinned to git commit `5e7a37a` (crates.io versions are unbuildable — yanked `core2`). egui/eframe are dev-deps (the sandbox is an example). |

---

## 2. Reference rules — what the sandbox MUST match

**Canonical contract (read this to review the modifiers):**
- `mentor MD/BASIC_MODIFIERS_RULES.md` — the extracted, **file:line-cited** behavior of
  MOVE/COPY/ROTATE/SCALE/MIRROR/ERASE taken from RUST_CAD. State machines, per-pick
  prompts, osnap/CARD-anchor rules, R/C options, single-vs-continue, apply fns, preview
  ghosts. §8 = recorder requirements. §9 = conformance checklist.

**The source it was extracted from (ground truth):**
- RUST_CAD repo: `~/workspace/RUST_CAD` — `cad_app/src/app.rs` (~30k lines, the modifier
  state machines + click cascade), `cad_kernel/src/{dobject.rs,geom.rs,parser.rs,snap.rs}`
  (the transforms + osnap). **simLUX and RUST_CAD share the SAME `cad_kernel` byte-for-byte**
  (only `geom.rs` differs by one Wall bugfix), so the transform + osnap symbols are identical
  and are called directly, never reimplemented.

**Plans / roadmap (context, not review-blocking):**
- `mentor MD/SIMLUX_3D_SANDBOX_PLAN.md` — the sandbox phasing (build isolated → wire in).
- `mentor MD/SIMLUX_3D_SOLIDS_PLAN.md` — the 3D solid primitives (steps/ramps/walls/columns).
- `mentor MD/SIMLUX_DIALUX_PLAN.md`, `SIMLUX_SCENE_AND_DAYLIGHT_PLAN.md` — longer-horizon.

**Merge rule:** the sandbox mirrors `app.rs` structure (state enums, apply_*, card_lock,
queued op, select-first) so a later merge into `cad_app` is 1:1 with no behavioral surprise.

---

## 3. How to review

**On GitHub (fastest for reading):**
- Branch tree: https://github.com/dokkandar/simLUX/tree/simlux-3d-sandbox
- Latest pushed commit `9f9ec5b`: https://github.com/dokkandar/simLUX/commit/9f9ec5b
- Compare against the crate's first commit `1ccd874` for the full sandbox diff.

**Locally:**
```bash
cd ~/workspace/simLUX && git checkout simlux-3d-sandbox
git log --oneline -8
git show 9f9ec5b            # the modifiers + rule-doc + recorder-logging commit
git diff 1ccd874..HEAD      # everything the sandbox added
cargo test -p cad_solid                       # unit tests (modify.rs etc.)
cargo run -p cad_solid --example sandbox      # the interactive window
```

**The session recorder IS the review instrument.** In the sandbox window: open the
recorder panel → **Start** → perform an action (e.g. select a solid, `copy`, pick base,
pick dest) → **dump** (or the Dump button). The dump reports, per §8: the highlighted
handles, each **named pick** (PIVOT/ANGLE/BASE/DEST) with world coords + snap kind, the
apply summary, and each object's **8 AABB corners projected to screen** before/after the
edit. That dump is how behavior is verified without watching the screen.

---

## 4. Current state — IMPORTANT for a reviewer

- **Pushed to GitHub (`9f9ec5b`):** the cad_solid crate + spec-faithful modifiers +
  `BASIC_MODIFIERS_RULES.md` + recorder named-pick logging.
- **NOT yet pushed (local uncommitted, on `cad_solid/examples/sandbox.rs` + `src/modify.rs`):**
  1. **Copy = single-drop** (base→dest→done) matching RUST_CAD, not multi-drop.
  2. **Enter-confirm fix** — Enter now finalises a gather selection even when the command
     box has focus (previously the text box swallowed Enter and trapped users in gather mode).
  3. **Screen-space 8-corner dump** (before/after/recap) in the recorder.
  4. **Fixed camera** — `cam_target` is stored, not recomputed from bounds each frame, so
     the view no longer jumps when an object is added/moved (⌖ Frame = zoom-extents on demand).
  → A reviewer pulling from GitHub will NOT see items 1–4 until they are committed & pushed.

**Done vs owed** — see `BASIC_MODIFIERS_RULES.md` §9. Owed: Mirror keep-[Y]/n prompt;
2D flat-sketch rotate/scale/mirror + window/crossing drag-select; Phase C verbatim DRAW
state machines; extrude sketch → solid + wire-in.
