# Why 2D is always incomplete — and the venue decision

**Reviewer:** 3d mentor · **Date:** 2026-07-15 · **Status:** decision required from the owner.
**Trigger:** *"i tried to run fillet but it is incomplete… i tried several times and always
something missing… A FULL SET OF DRAFTING AND MODIFY TOOLS ON EACH PLANE… it is not negotiable."*

---

## 1. The diagnosis (verified, not opinion)

**You did not fail three times. The architecture guaranteed it three times.**

| Fact | Value |
|---|---|
| Complete 2D drafting+modify, already written | **`cad_app/src/app.rs` = 31,853 lines** |
| References to the 2D modifier state machines there | **199** (`fillet_state`, `offset_state`, `trim_state`, `extend_state`, `chamfer_state`, `break_state`, `lengthen_state`, `align_state`, `stretch_state`) |
| **Can any crate depend on `cad_app`?** | **NO — it is `[[bin]]`-only. There is no `lib.rs`.** |
| The sandbox's reimplementation of 2D | `cad_solid/src/draw.rs` = **889 lines** |
| Reimplementation as a fraction of the real thing | **2.8 %** |

**The causal chain:**
1. `cad_app` is a binary → **nothing can call its 2D layer.**
2. `cad_solid` is isolated (deliberately, for fast 3D iteration) → it **must reimplement** 2D.
3. A reimplementation is **always a subset**.
4. ⇒ "Something is always missing" — **structurally guaranteed, forever**, until the 2D layer
   is reachable. No amount of effort inside the sandbox fixes this.

### 1.1 Your fillet, precisely
You typed **`r`** (the AutoCAD FILLET **Radius** option). The dump shows
`flat cmd 'r'` → `unknown: r`. **That option exists** — `app.rs:4370`
(`Some("r") | Some("radius")`), alongside `t`/`trim`, `nt`/`notrim`, `m`/`multiple`,
`p`/`polyline` (`4342`/`4352`/`4361`). The sandbox's fillet accepts **only a bare number**
([sandbox.rs `flat_edit_value`](../cad_solid/examples/sandbox.rs)). You weren't hitting a missing
feature — you were hitting a **wall between two implementations of the same feature**.

### 1.2 Your cursor (item 2) — also already built
The **square + cross drafting cursor**, the **pickbox**, and the **crosshair** exist:
`app.rs:18993-19017`, drawn iff `in_click_only_phase` (`app.rs:24477`), sized by the
**`CrsHrS` / `PkBxSz` SYSVARs**. Exactly the "square+cross in drafting, pointer in selection"
behaviour you described — **already shipped, just unreachable from the sandbox.**

> **The pattern:** every item on your list already exists in `cad_app`. None of it is reachable.
> That *is* the bug.

---

## 2. The options

| | Approach | Gets full 2D on every plane? | Cost |
|---|---|---|---|
| **A** | Add `lib.rs` to `cad_app`, call it from `cad_solid` | ❌ **No** — the state machines are `impl CadApp` methods reading ~18 `*_state` fields + `self.doc` + `self.tool`. You cannot call `fillet` without constructing a whole `CadApp`. Exposing a lib target does not make a monolith reusable. | Low effort, **doesn't work** |
| **B** | Extract the 2D interaction layer into a shared `cad_draft` crate; both `cad_app` and `cad_solid` drive it | ✅ Yes, permanently | **Very high** — refactor a 31.8k-line file welded to `CadApp`. Months before you see a fillet on a plane. Also touches "core", against the clean-merge policy. |
| **C** | **Move the 3D viewport INTO `cad_app`.** Keep `cad_solid` as a **library** dependency. | ✅ **Yes — by construction, day one** | **Low–moderate** — move ~2k lines of good code into the app; delete the duplicated shell. |

---

## 3. Recommendation — **C. Move the small thing to the big thing.**

The 2D layer is **31,853 lines** and is the product's crown jewel. `cad_solid`'s *reusable*
core is **~2k lines** and is already **UI-agnostic**. Dragging 31.8k lines of 2D into an
isolated sandbox is backwards; moving 2k lines of 3D into the app is trivial by comparison.

**This is not throwing away your work — most of it is the keeper:**

| Keep (already UI-agnostic, becomes a `cad_app` dependency) | Delete (duplicates what the app already has) |
|---|---|
| `cad_solid/src/lib.rs` — `Model`, `Feature`, `Plane`, `Frame`, `Sketch`, ray-pick, AABB | `cad_solid/src/draw.rs` (889 lines) — the 2.8% reimplementation |
| `cad_solid/src/csg.rs` — the csgrs boundary | `cad_solid/examples/sandbox.rs` (~2,500 lines) — a second app shell: its own command line, cursor, recorder UI, camera |
| `cad_solid/src/modify.rs` — the 3D modifiers (spec-conformant, 37 tests green) | |
| `cad_solid/src/dbg_recorder.rs` — already a verbatim copy of the app's | |

**The mechanism is already proven by your own code.** A sketch's `Document` lives in the plane
`Frame`'s `(u,v)`; tools operate on it unchanged; render lifts uv→world. `cad_solid` already
does exactly this (`model.sketches[idx].frame.to_uv(w)`). It just has to happen **where the
tools live**. Then:

- **Fillet on a plane works — with R/T/M/P** — because it *is* the app's fillet.
- **The square+cross cursor works** — because it *is* the app's cursor, with its SYSVARs.
- **The command line works** — because it *is* the app's command line (`run_command`, the
  27-intercept cascade, Space=Enter, chips, history, `last_command`).
- **Trim/extend/offset/chamfer/break/align/stretch/join all work** — day one, no port.
- **Drift becomes impossible** — there is only ever **one** 2D implementation.

### 3.1 What this costs, honestly
- You iterate inside a 31.8k-line file instead of a 2.5k-line example. **Slower to change 3D.**
- `csgrs` (+ its transitive `rapier3d`/`parry3d`, see `LIBRARY_REVIEW_3D_2026-07-14.md` §2.4)
  enters the shipping binary's dep tree.
- The sandbox's fast-rebuild loop goes away.

**That trade is correct**, because your non-negotiable is *full 2D on every plane*, and the
sandbox **cannot** satisfy it at any effort level. Isolation was buying speed on the thing that
was already working (3D), and paying for it with the thing you actually care about (2D).

### 3.2 On "if sandbox not helping we make another project"
**A new project does not help** — it has the identical topology (isolated crate + unreachable
`cad_app`) and will fail the same way a fourth time. The venue isn't the problem; the
**reachability of the 2D layer** is. Only B or C change that, and C is ~10× cheaper.

---

## 4. Migration plan (C)

| Slice | Content | Proof it worked |
|---|---|---|
| **1** | `cad_app` takes `cad_solid = { path = "../cad_solid" }`. Delete `draw.rs`. `cad_solid` becomes lib-only (drop the example). | `cargo build -p cad_app` |
| **2** | 3D viewport as a **panel in `cad_app`** (port `SceneRenderer` + camera + navigator from `sandbox.rs`). Render the CSG mesh. | a box renders in the app |
| **3** | **Plane/face → `Frame` → sketch `Document`.** Route the app's existing pick/tool pipeline through the frame's `(u,v)`. | **type `f` → `r` → 10 on a plane and get a filleted corner** ← the acceptance test |
| **4** | 3D modifiers (`cad_solid::modify`) on the 3D command line, via the app's `run_command` cascade. | the 37 existing tests still pass |
| **5** | Retire `examples/sandbox.rs`. | one app, one command line, one cursor |

**Slice 3 is the whole point** — it is the first moment the non-negotiable is satisfied, and it
is satisfied *permanently*, because nothing was reimplemented.

---

## 5. Your three items, answered

1. **Floating command window** — do it in `cad_app`'s command bar (one command line, not two).
   Building a third command line in the sandbox is throwaway work. **Blocked on the venue
   decision, not on difficulty.**
2. **Square+cross drafting cursor / pointer in selection** — **already exists**
   (`app.rs:18993-19017`, `in_click_only_phase`, `CrsHrS`/`PkBxSz`). Free under C. Under the
   sandbox it's a fourth reimplementation that will drift.
3. **Full 2D on every plane** — **only C delivers it.** Recorded as a permanent, non-negotiable
   requirement in memory (`project_simlux_2d_complete_on_every_plane`).

---

## 6. The decision I need from you

**Approve C** (3D viewport moves into `cad_app`; `cad_solid` becomes a library; `draw.rs` and
`examples/sandbox.rs` are deleted) — and I'll start at Slice 1.

If you'd rather keep an isolated sandbox, then **the 2D requirement cannot be met there**, and I
should say that plainly every time rather than shipping another subset.

*MD only — no code changed by this review.*
