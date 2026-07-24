# 3D_Factory

**A full working copy of the simLUX workspace where the 3D solid modeller is built inside the
real 2D app** — so every construction plane gets the **complete** 2D drafting + modify toolset,
with **nothing reimplemented**.

> **Status:** fork. Develop here, then **one big merge** back into `simLUX`.

---

## Why this exists

The `cad_solid` sandbox could never satisfy the owner's non-negotiable requirement —
*"A FULL SET OF DRAFTING AND MODIFY TOOLS ON EACH PLANE"* — and the reason was structural,
not effort:

| Fact | Value |
|---|---|
| Complete 2D drafting + modify, already written | **`cad_app/src/app.rs` ≈ 31.8k lines** (39.3k for the crate) |
| Could any crate depend on it? | **No** — `cad_app` is `[[bin]]`-only, no `lib.rs` |
| The sandbox's reimplementation | `cad_solid/src/draw.rs` = **889 lines — 2.8 %** |

An isolated crate **must** reimplement 2D, and a reimplementation is **always a subset** ⇒
"always something missing" was *guaranteed*, forever. Real examples hit in testing:

- **FILLET `r` (Radius)** — exists at `cad_app/src/app.rs:4370` (with `t`/`trim`, `m`/`multiple`,
  `p`/`polyline`). The sandbox's fillet took only a bare number → `unknown: r`.
- **Square+cross drafting cursor / pickbox / crosshair** — exists at `app.rs:18993-19017`, gated
  on `in_click_only_phase`, sized by the `CrsHrS` / `PkBxSz` SYSVARs.

Both were **already built and simply unreachable**. 3D_Factory removes the wall by putting the
3D work *inside* the app that already has all the 2D.

**The rule that follows:** never reimplement 2D. If something is "missing", it is a
**reachability** problem — grep `cad_app/src/app.rs` first; it is almost certainly already there.

Full reasoning: [`mentor MD/VENUE_DECISION_2D_ON_EVERY_PLANE.md`](mentor%20MD/VENUE_DECISION_2D_ON_EVERY_PLANE.md).

---

## What's here

A copy of the **entire** simLUX workspace (all 11 crates), so the 2D code can be changed freely
without destabilising `simLUX`:

| Crate | Role |
|---|---|
| `cad_app` | **the 2D app — the whole point.** 39.3k lines: all draw tools, all modifiers, command line + 27-intercept cascade, cursor, SYSVARs |
| `cad_solid` | **the 3D layer** — `Model`/`Feature`/`Plane`/`Frame`, parametric CSG (csgrs), 3D modifiers. **Now a dependency of `cad_app`.** |
| `cad_kernel` | 2D geometry / parser / snap (byte-identical to RUST_CAD) |
| `cad_light` | lighting engine (IES + lux) · `cad_io` file IO · `cad_param` constraint solver |
| `cad_nurbs` `cad_snap` `cad_wall` `cad_raster` `cad_cli` | supporting crates |

`Cargo.lock` **is committed and must stay that way**: csgrs is pinned to git commit `5e7a37a`
because every crates.io release hard-pins the **yanked** `core2 0.4.0`. A fresh resolve can
break the build — the lock is load-bearing.

---

## 3D Factory toolset

Open **3D Factory** from the menu bar to model in the 3D panel beside the 2D canvas. Every tool
lives in the panel's own toolbar (dropdown menus), and every construction plane keeps the
**complete** 2D drafting + modify toolset — nothing is reimplemented.

**Modelling**
- **3D solids** — box, cylinder, and other primitives. Each is movable and scalable via a
  3-axis gizmo, with a live properties panel (position · dimensions · colour).
- **Building** — extrude a closed 2D outline into a solid mass at a set height (massing).
- **Room** — build a complete enclosed room from an outline: **floor slab + perimeter walls +
  ceiling**, with configurable **room height**, **floor thickness**, and an **open-to-sky**
  option. Draw a room inside a building and the building is automatically **carved into an
  annular wall** around it, so the room can be seen into.
- **Storeys / levels** — stack levels and **duplicate floor** to repeat a plan upward.

**Viewing**
- **Hide ceilings** — hide room ceilings (and lift a solid building's roof over a room) so you
  can see inside from any angle while the walls stay solid. Detection is geometric, so it keeps
  working even when object tags drift.
- **Plan** — toggle the 2D plan as a reference underlay inside the 3D view.
- Orbit (middle-drag) · pan (Shift + L) · zoom (scroll) · **Frame** · navigator cube.

**Detailing**
- **Furniture** — import meshes (**OBJ** and **3DS**) into a per-project library and place
  instances; each is movable, scalable, rotatable, and recolourable. Stored in the project file
  for reuse.
- **Textures / colours** — a colour palette applied per **object** or per **surface** (click an
  individual face).

**Sketch on any face**
- Right-click a solid face → **Draw on this face** to sketch on that plane with the full 2D
  toolset (draw, modify, fillet, …).
- The 3D object is **projected onto the plane as a faint reference** (its feature edges), and
  its edges and corners become live **object-snap** targets.
- Re-picking a face **reopens its existing sketch**, so drawn work persists.

**Persistence** — the whole 3D model (solids, walls, storeys, the furniture library + placed
instances, per-object and per-surface colours, and hidden-ceiling state) is saved in a
`<drawing>.simlux.json` sidecar next to the drawing.

---

## Build

```bash
cd ~/workspace/simLUX/3D_Factory
cargo build --workspace      # whole thing, 2D + 3D
cargo run   -p cad_app       # the app (bin name: `simlux`)
cargo test  --workspace
```

Verified 2026-07-15: workspace builds clean; `cad_app` compiles **with** `cad_solid`; all tests pass.

---

## Plan — 3D in the app

| Slice | Content | Done when |
|---|---|---|
| **1** | `cad_app` depends on `cad_solid`; workspace builds as one | ✅ **done** |
| **2** | 3D viewport as a **panel in `cad_app`** — port `SceneRenderer` + camera + navigator from `cad_solid/examples/sandbox.rs` | a box renders in the app |
| **3** | **Plane/face → `Frame` → sketch `Document`**; route the app's existing pick/tool pipeline through the frame's `(u,v)` | ⭐ **`f` → `r` → `10` on a plane gives a filleted corner** — the acceptance test for the whole project |
| **4** | 3D modifiers (`cad_solid::modify`) on the app's command line, via `run_command`'s cascade | the 3D modifier tests still pass |
| **5** | Retire `cad_solid/examples/sandbox.rs` + `cad_solid/src/draw.rs` (the 889-line reimplementation) | one app, one command line, one cursor |

**Slice 3 is the point.** It's the first moment the non-negotiable is satisfied — and it's
satisfied *permanently*, because nothing was reimplemented.

The mechanism is already proven by the sandbox: a sketch's `Document` lives in the plane
`Frame`'s `(u,v)` (`frame.to_uv(w)`); tools operate on it unchanged; render lifts uv→world. It
just has to happen where the tools live.

---

## Merge back

Everything lands in `simLUX` as **one big merge** when finished. Two consequences to respect:

1. **Divergence is the main risk.** Every day `simLUX`'s `cad_app` and this copy drift, the
   merge gets harder. Keep 2D edits here **minimal and surgical** — this fork is for *adding
   3D*, not for rewriting 2D.
2. **`cad_kernel` is byte-identical to RUST_CAD.** Don't casually diverge it, or the merge
   spreads to a second repo.
