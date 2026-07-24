# Planes on DObjects — design + comprehensive TODO

**Author:** 3d mentor · **Date:** 2026-07-15 · **Status:** design; needs 2 owner decisions (§8).
**Trigger:** *"add a new attribute to our dobjects as Plane … how are we going to define planes in
our in-file datastructure … make a comprehensive todo list"* + 3 UI asks (§6).

---

## 0. TL;DR

| Question | Answer |
|---|---|
| Is "plane on the dobject" the right call? | **Yes — and it is FORCED, not a preference.** `Geom` is `Vec2`. A dobject on a tilted plane *cannot* store world coords. §1 |
| Does it conflict with existing plans? | **Yes — head-on with `UCS_Roadmap.md`**, which says UCS is "not 3D" and "not per-dobject". That spec assumed a 2D-only project. §2 |
| What shape? | **Mirror the LAYER pattern**: `Document.planes` + `DObject.plane: PlaneId`. A plane = **ID + 3 GLOBAL points** (owner's refinement) — the identifier anchors it in world space; the frame is derived, never stored. **Plane 0 = STANDARD, fixed, never rotated** ⇒ 2D is byte-identical to today. §3.0 |
| On-disk? | **RSM v7 → v8**: a `planes` table + a per-dobject id. `#[serde(default)]` ⇒ v7 files still load. §4 |
| Bonus | It **deletes the doc-swap** and the entire crash class we just fixed. §5 |
| Cost | **None to the kernel — use the SIDECAR** (decision D5, the pattern simLUX already uses for all of SIMLUX). Promote to kernel fields later only if cross-plane ops need it. §8.1a/b |
| Overhead for 2D users? | **ZERO — and it's a correctness win.** The spatial index is 2D and buckets on `bbox()`, so ONE grid cannot hold two planes. Partition per plane ⇒ 2D = one grid = today's exact code path; multi-plane = smaller grids = *faster*. §9 |
| ⛔ Blocker | `HANDLE_COUNTER` is never bumped on load → new objects collide with loaded handles. **Proven.** Also corrupts hatch boundaries. Fix first. §8.1c |

---

## 1. The decisive constraint — `Geom` is 2D

```rust
pub struct Line { pub a: Vec2, pub b: Vec2 }     // cad_kernel/src/geom.rs
```

Every `Geom` variant is `Vec2`. So:

> A line drawn on a **tilted** plane has **3D** world coordinates.
> **It is impossible to store that in a `Vec2`.**

There are exactly two ways out:

| Option | Consequence |
|---|---|
| Make `Geom` 3D (`Vec3`) | Rewrites the kernel **and** all ~31.8k lines of 2D tools, and destroys `cad_kernel`'s byte-identity with RUST_CAD. **Catastrophic. Reject.** |
| **Store plane-local `(u,v)` + a plane reference on the dobject** | **What you asked for.** The dobject stays `Vec2`; the plane supplies the lift to 3D. |

So the design isn't a preference call — **the type system already decided it.** Your instinct is right.

### 1.1 The elegant property that makes this safe
For **plane 0 = World XY**, `(u, v) ≡ (world x, world y)`. So:

> **For every existing pure-2D drawing the change is a literal no-op.**

That is what makes this survivable: the 2D app never notices, and the merge back is additive.

---

## 2. ⚠️ This collides with `UCS_Roadmap.md` — reconcile it deliberately

`UCS_Roadmap.md` (in **both** simLUX and RUST_CAD) already specs "alternate coordinate system",
and `cad_kernel/src/document.rs` already **reserves the slot**:

```rust
// Reserved for future slices — leave the field list extensible:
// pub ucs_list:    UcsList,
```

But that spec explicitly forbids what you're asking for:

> **Not 3D.** UCS is 2D rotation + translation only … this whole project is 2D.
> **Not per-dobject.** Dobjects always store world coords. The UCS is an **editor lens, not a geometry attribute**.
> **Storage rule: all dobjects stay in WORLD coords on disk and in memory.**

**Who's right?** Both — for different projects. That spec's reasoning is sound *given its stated
premise*: "this whole project is 2D". **3D_Factory broke that premise.** Its storage rule cannot
survive 3D, for the reason in §1: there is no `Vec2` world coord for a tilted plane.

**Therefore: UCS and Plane are TWO DIFFERENT CONCEPTS. Do not merge them.**

| | **UCS** (existing spec, unbuilt) | **Plane** (this design) |
|---|---|---|
| What | An **editor lens** — type/read coords in a rotated 2D frame | A **geometry attribute** — which surface a dobject lives on |
| Storage | Nothing per-dobject | `PlaneId` per dobject |
| Dimensionality | 2D rotation + offset | Full 3D frame (origin + u + v) |
| Affects the file? | Only a UCS table + active index | Dobject coords **mean nothing** without it |

A UCS answers *"what's my typing frame?"*. A Plane answers *"where in 3D does this line exist?"*.
You can have a UCS **on** a plane later; they compose.

**Action:** add a "Superseded for 3D" note to `UCS_Roadmap.md` §"What this isn't", pointing here,
or the next agent will implement the 2D rule and fight this design. **Do this before coding.**

---

## 3. The design — mirror the LAYER pattern exactly

The kernel already has this exact shape and the app already has the machinery. **Reuse it; invent nothing.**

| Layers (exists today) | Planes (this design) |
|---|---|
| `Document.layers: LayerTable` | `Document.planes: PlaneTable` |
| `DObject.style.layer: LayerId` | `DObject.plane: PlaneId` |
| layer 0 reserved (`Layer::layer_zero()`) | **plane 0 = World XY, reserved** |
| current/active layer | **active plane** |
| `doc.layers.renders(e.style.layer)` render gate | `doc.planes.renders(...)` / active-plane gate |
| lock / freeze / visible | **other planes = reference (greyed, snappable, not editable)** — identical semantics to a locked layer |
| serialised as a table in RSM | same |

### 3.0 The plane record = **ID + 3 global points** (owner's refinement, 2026-07-15)

> *"each plane will have an ID and 3 points from global as identifier … if it is standard 2D
> drafting, the plane id is 0 and identifier is standard for this."*

**Adopted.** A plane is identified by **3 world points** — not by stored `u`/`v` vectors. The 3
points are the *authoritative, serialized* form; the orthonormal frame is **derived**.

```rust
pub struct PlaneId(pub u16);                 // 0 = STANDARD (world XY), always present

pub struct DPlane {
    pub id:   PlaneId,
    pub name: String,        // "Top of box", "Wall A"
    /// The IDENTIFIER: 3 non-collinear GLOBAL points that anchor this plane in world space.
    /// p[0] = origin · p[1] = a point on +U · p[2] = a point on the +V side.
    pub p: [[f64; 3]; 3],
    pub color: u8,           // ACI — plane list + 3D outline
}

pub struct PlaneTable { planes: Vec<DPlane> }   // planes[0] == STANDARD

// cad_kernel::Document
pub planes: PlaneTable,
// cad_kernel::DObject
pub plane: PlaneId,          // #[serde(default)] → 0 = STANDARD
```

**Plane 0 is fixed and standard — it is never rotated:**
```
p = [ (0,0,0), (1,0,0), (0,1,0) ]   ⇒  u = (1,0,0)   v = (0,1,0)   n = (0,0,1)
```
i.e. **exactly the world axes**. That is what makes §1.1 true: for plane 0, `(u,v) ≡ world (x,y)`,
so **standard 2D drafting is byte-identical to today**. Plane 0's identifier is a **constant**;
it is not user-editable and not derived from anything.

**Why 3 points beat storing `u`/`v` (same 9 floats either way):**
- **Illegal states become unrepresentable.** A stored `u`/`v` pair can drift non-orthonormal or be
  corrupted → geometry silently **skews**. 3 points always re-derive a clean orthonormal frame —
  a skewed frame cannot be expressed.
- **It matches how a plane is created** (3 picks) *and* the existing `UCS_Roadmap.md` flow:
  *"click ORIGIN → click point on +X axis → click point on +Y axis … the system locks Y = +90°
  from X to stay orthogonal"* — that "lock" **is** the Gram-Schmidt below. Same convention, already
  the project's.
- **Globally anchored provenance**, which is the owner's point: the plane's identity is 3 world
  coordinates, so a dobject's `(u,v)` can always be resolved back to world — nothing is "lost in
  the global system".

**Derivation (fix this convention once — planes 1..N depend on it):**
```
u = normalize(p1 - p0)
w = p2 - p0
v = normalize(w - u * dot(w, u))        // Gram-Schmidt = "lock V +90° from U"
n = u × v
reject if |u| ≈ 0, |v| ≈ 0  (collinear ⇒ degenerate, not a plane)
```
⚠️ **The point ROLES are load-bearing** — `p1` is the +U point, `p2` only picks the +V *side*.
Swapping p1/p2 rotates that plane's frame. Verified numerically: with p1=(1,0,0), p2=(0,1,0) *or*
p2=(1,1,0), the frame is identically `u=(1,0,0), v=(0,1,0)` (Gram-Schmidt absorbs the non-
perpendicular p2) — but feeding `(1,1,0)` as **p1** yields a 45°-rotated frame. So: document the
roles at the field, and **validate on load**.

**Derive ONCE, never per-dobject.** The 3 points are the on-disk truth; cache the derived `Frame`
(`origin,u,v`) in the `PlaneTable` when it loads or changes. Re-deriving per dobject per frame
(normalize + dot + cross) would be exactly the overhead §9 exists to avoid. Plane count is tiny and
changes rarely, so this cache is cheap and — unlike the doc-keyed caches of §9.5 — is keyed to the
plane table, not to `doc.dobjects`.

**Why `DObject.plane` and not `DObject.style.plane`:** `Style` is presentation (colour, linetype,
visible). A plane changes what the coordinates **mean** — that is geometry, not style. Keep it a
peer of `geom`.

**Frame math already exists** — `cad_solid::Frame { origin, u, v }` with `to_uv` / `from_uv` /
`from_point_normal`. `DPlane` is that struct plus identity. Move `Frame` down into `cad_kernel`
(or make `DPlane` convert to it) so both crates share one definition — **do not write a second one.**

### 3.1 The hazard this creates — tools must filter by plane
One document means the tools now see **every** plane's dobjects. That is **wrong** for anything
geometric: TRIM would take cutters from a plane that isn't coplanar, OSNAP would snap to a point
that isn't on your surface, a window-select would grab another wall.

**Mitigation = the layer gate, reused.** Everything geometric filters to the **active plane**:
- candidate iteration (the culling loop — the one that just crashed),
- pick / window-select,
- trim/extend/fillet/chamfer cutters + boundaries,
- osnap targets (with an opt-in "snap to other planes" later, like AutoCAD's projected snaps).

**This is the bulk of the work** and the main risk. It is ~1 predicate applied at ~6 choke points,
*not* 31.8k lines of edits — because the tools all funnel through the candidate list + selection.

---

## 4. On-disk — RSM v7 → v8

Today: `cad_io/src/rsm.rs` → `const VERSION: u16 = 7`.

```
RSM v8 adds:
  TABLE  planes:
      id, name, p[3][3] (the 3 GLOBAL identifier points), color
      (id 0 = STANDARD, written explicitly as [(0,0,0),(1,0,0),(0,1,0)] so the
       file is self-describing; u/v are DERIVED on load, never stored — a stored
       u/v could arrive non-orthonormal and silently skew geometry)
  DOBJECT record gains:
      plane: u16          #[serde(default)] → 0
```

**Compatibility rules (both directions):**
- **v7 → v8 (read old file):** no `planes` table ⇒ synthesise World only; every dobject `plane = 0`.
  Because of §1.1 this is **exactly** the current behaviour. Old drawings are untouched.
- **v8 → v7 (old app reads new file):** dobjects on plane ≠ 0 would be silently misread as World —
  their `(u,v)` would be drawn as world XY. **Decide:** either bump the "minimum reader version"
  so v7 readers refuse, or accept the flattening. **I recommend refusing** — silent geometry
  corruption is worse than a clear error.

**"Keep previews for the user":** the `planes` table + per-dobject id is exactly what makes a
preview possible — on reopen you can rebuild every plane's frame and lift its geometry into 3D
without re-deriving anything. Add to the plane record only what a preview needs and nothing more
(`name`, `color`). **Do not** store a bitmap thumbnail — it's derivable, and it will go stale.

**Also needed:** the 3D solid features themselves (`cad_solid::Model`) still have no persistence
(`SIMLUX_3D_SOLIDS_PLAN.md` proposed "RSM v9"). **Sequence planes as v8 first** — they're a smaller,
self-contained change — then solids. Don't do both in one version bump.

---

## 5. The prize: this DELETES the doc-swap and its crash class

Today `factory_enter_sketch` swaps `self.doc`, which is why we needed `factory_reset_doc_state()`
and why a stale **spatial index** crashed the app (commit `07120ac`).

With one document + a plane id:
- **no swap** → no parked doc, no parked undo stacks, no stale index, **no `factory_reset_doc_state`**;
- **one undo stack** across 2D and 3D — Ctrl+Z just works everywhere;
- everything is displayable **together**, which is what you asked for;
- entering a plane becomes *exactly* as cheap and safe as **switching the current layer**.

That is the strongest argument for this design after §1. **The crash we just fixed becomes
structurally impossible.**

---

## 6. Your three UI asks — all reuse something that exists

1. **"3D Factory active → split display in equal half."** ✅ Already built: the SIMLUX workspace does
   exactly this — `let half = ctx.screen_rect().width() * 0.5; base.exact_width(half)`, gated on
   `light.simlux_mode`. **Reuse that pattern**, don't write a second splitter. Open question: 3D
   Factory and SIMLUX-workspace both want the right half — they must be **mutually exclusive**
   (one workspace mode enum), or they'll fight over the panel.
2. **"Basic 3D objects should have controllers."** Needs a **feature inspector**: pick a feature in
   the 3D view → edit its `Placement { u, v, lift, spin_deg }` + `Primitive` params (box `w/d/h`,
   cylinder `r/h/sides`) + its `BoolOp`. The app already has `param_editor.rs` and an Inspector
   panel — **follow the Inspector's shape**. Must re-eval CSG **only on release**, never per-drag
   frame (csgrs walks a BSP per boolean — this is the known lag source).
3. **"3D viewpoint setter same as sandbox."** The app has **no** viewcube; the sandbox's is
   `sandbox.rs:2150 fn navigator`. **Port it verbatim** — it is the only one of the three that is a
   genuine port rather than a reuse.

---

## 7. THE COMPREHENSIVE TODO

Ordered so each slice is independently mergeable and independently *provable*.

### Phase A — decide + document (before any code)
| # | Task | Done when |
|---|---|---|
| A0 | **⛔ Fix the handle-collision bug** (§8.1c) — additive `reserve_handles_above()` + call in `read_rsm`. Blocks everything handle-keyed, and fixes hatch-boundary corruption today | the `--ignored` known-bug test passes |
| A1 | **Decide §8.1**: sidecar now (recommended, per D5) vs kernel fields now | written down in the memo |
| A2 | **Decide §8.2**: v7 readers refuse v8, or flatten | written down |
| A3 | Add a "**superseded for 3D**" note to `UCS_Roadmap.md` pointing here — Plane ≠ UCS (§2) | the next agent can't mis-implement |
| A4 | Decide the workspace-mode enum (3D Factory vs SIMLUX split are exclusive, §6.1) | one enum, not two bools |

### Phase B — kernel (pure, testable, no UI)
| # | Task | Done when |
|---|---|---|
| B1 | `PlaneId`, `DPlane`, `PlaneTable` in `cad_kernel`; `planes[0] = World XY` | unit tests |
| B2 | Move/share `Frame` (origin/u/v, `to_uv`/`from_uv`/`from_point_normal`) into the kernel; `cad_solid::Frame` re-exports it — **one definition** | `cad_solid` compiles against it |
| B3 | `Document.planes`; `DObject.plane: PlaneId` with `#[serde(default)]` | `cargo test -p cad_kernel` green |
| B4 | `DPlane::to_world(uv) -> Vec3`, `from_world(Vec3) -> Vec2`, round-trip tests incl. tilted planes | property test: `from_world(to_world(p)) ≈ p` |
| B5 | **Prove §1.1**: every existing test still passes with plane 0 ⇒ 2D is a no-op | full kernel suite green, unchanged |

### Phase C — persistence (RSM v8)
| # | Task | Done when |
|---|---|---|
| C1 | RSM `VERSION 7 → 8`; write the `planes` table + per-dobject id | round-trip test |
| C2 | **Read v7** → synthesise World, all dobjects plane 0 | golden v7 file loads byte-identical |
| C3 | v7-reader policy from A2 (refuse or flatten) | explicit test for the chosen behaviour |
| C4 | Round-trip test: 2 planes + geometry on each → save → load → identical frames + ids | test green |

### Phase D — the app: active plane (mirror the layer machinery)
| # | Task | Done when |
|---|---|---|
| D1 | `active_plane: PlaneId` on `CadApp` (peer of the current layer) | — |
| D2 | **PARTITION the spatial index per plane** (§9) — one grid each; query the active plane's. NOT a per-dobject filter: one grid can't hold two planes (`bbox()` is `Vec2`), and a per-dobject check would tax 2D users | 2D path unchanged; `planes.len()==1` ⇒ today's exact code |
| D3 | Filter **pick + window-select** | can't select through a plane |
| D4 | Filter **osnap targets** | can't snap to a non-coplanar point |
| D5 | Filter **trim/extend/fillet/chamfer** cutters + boundaries | ⚠️ the correctness-critical one |
| D6 | New dobjects inherit `active_plane` (like the current layer) | draw on a plane → correct id |
| D7 | Other planes render as **reference** (greyed, snap-optional) — reuse locked-layer semantics | visible but not editable |
| D8 | **Delete `factory_enter_sketch`/`exit`/`factory_reset_doc_state`** + the `SketchSession` doc-swap; "enter a plane" = set `active_plane` | the §5 prize; crash class gone |

### Phase E — 3D view + UI (your §6 asks)
| # | Task | Done when |
|---|---|---|
| E1 | Workspace mode enum; **3D Factory = equal half split** (reuse `simlux_mode`'s `exact_width(half)`) | toggling splits 50/50 |
| E2 | **Port the sandbox viewcube** (`sandbox.rs:2150 fn navigator`) | standard views snap |
| E3 | 3D view lifts each dobject by its plane frame (replaces `factory::sketch_lines`) | 2D work appears on its plane in 3D |
| E4 | **Feature inspector/controllers**: pick a feature → edit Placement + Primitive + BoolOp | drag `w` → box resizes |
| E5 | CSG re-eval **on release only**, never per-drag frame | no lag while dragging |
| E6 | Plane list panel (name, colour, active, visible) — mirror the Layers panel | switch plane from the UI |
| E7 | `plane` commands: `plane` (list), `plane <name>`, `plane new` (3-pick), `plane world`, `plane del` — mirror `UCS_Roadmap` §"How users interact" | typed flow works |

### Phase F — close the loop
| # | Task | Done when |
|---|---|---|
| F1 | Extrude a plane's closed profile → a `cad_solid` feature | 2D → solid |
| F2 | 3D modifiers on the app command line via `run_command`'s cascade (Slice 4, still owed) | move/rotate a solid by typing |
| F3 | `cad_solid::Model` persistence (**RSM v9**, after v8 lands) | solids survive save/reopen |

### Acceptance test for the whole design
```
box → right-click top face → plane created + active
f → r → 10                      (the app's own fillet, on that plane)
save → reopen                   → the plane + its filleted profile are back, in 3D
switch to World                 → the 2D drawing is untouched
```

---

## 9. Overhead — can 2D ignore it? **Yes: literally zero.**

> *"if user uses 2D it means the standard plane … the plane only gets important once we have 3D
> dobjects. Can the overhead be ignored? What's the plan to reduce it if it occurs?"*

### 9.1 The finding that decides it: the index is 2D and buckets on `bbox()`

```rust
pub struct UniformGrid { cell_size: f64, origin: Vec2, cols, rows, cells: Vec<Vec<u32>> }
pub fn build(dobjects: &[DObject], cell_size: f64) -> Self { … let (emin, emax) = e.bbox(); … }
```

`bbox()` is the dobject's own **`Vec2`** bounds. If dobjects hold plane-local `(u,v)`, then two lines
on **perpendicular walls**, each spanning `(0,0)–(10,10)`, bucket into the **same cells** — metres
apart in 3D, identical to the index.

> **One grid cannot hold two planes.** This is **correctness**, not performance.
> ⇒ **The index MUST be partitioned per plane.** And that partition is exactly what makes the
> overhead vanish.

### 9.2 The answer: partition the index by plane → "pay only when you use it"

| Situation | Index | Hot loop cost |
|---|---|---|
| **Pure 2D** (`planes.len() == 1`) | **one** grid = the one that exists today | **ZERO — byte-identical code path.** No filter, no branch, no lookup. |
| **Multi-plane** | one grid **per plane**; query only the **active** plane's | **FASTER than today** — each grid is smaller, so `query_bbox` returns *fewer* candidates than a combined grid would |

The key consequence: **the render loop never asks a dobject which plane it's on.** The grid already
guarantees every candidate is same-plane, so there is no per-dobject check to pay for. The filter
in §3.1 stops being a per-dobject predicate and becomes *"which grid do I query"* — decided **once
per frame**.

### 9.3 Cost table (be concrete)

| Cost | Kernel field | Sidecar (§8.1a) |
|---|---|---|
| Storage / dobject | **+2 bytes** (`u16`), likely absorbed by existing padding — `Geom` already dominates `DObject`'s size. 100k dobjects ⇒ **~200 KB** | **0** |
| Render hot loop (per frame) | **0** — grid partition (§9.2) | **0** — same reason |
| Index build (per **edit**, not per frame) | **0** | **+1 map lookup per dobject** on an already-`O(n)` build |
| Can it go stale? | **No** — travels with the dobject | Only at index build; the reconcile rule covers it |

### 9.4 Why this rescues the sidecar
The sidecar's obvious perf worry — a `BTreeMap<handle, String>` lookup **per dobject per frame** —
would be fatal in the render loop. The partition **moves that lookup out of the frame entirely**:
it happens once per dobject when the index is rebuilt (already gated on `index_dirty`). So the
sidecar costs nothing where it matters. **§8.1a stands.**

### 9.5 If overhead ever *does* show up — the escalation ladder
1. **Hoist, don't filter.** Never write `if d.plane != active` inside a per-dobject loop; decide
   *which grid* once per frame (§9.2). If a per-dobject check ever appears in a hot loop, that's
   the bug.
2. **Single-plane fast path.** `planes.len() == 1` ⇒ skip every plane code path. This is the
   guarantee that 2D users pay literally nothing, forever.
3. **Materialise `Vec<PlaneId>`** parallel to `doc.dobjects` (rebuilt on `index_dirty`) *only if*
   profiling shows the sidecar's build-time lookups matter. ⚠️ This adds a derived cache keyed to
   the document — **the exact class that caused the `07120ac` crash**. Don't do it on a hunch;
   measure first.
4. **Promote to the kernel field** (§8.1b). If step 3 is ever needed, the field is strictly better:
   it *is* the materialised view, and it cannot desync.

> **The rule:** overhead is only ignorable **because** of the partition. The moment someone
> "simplifies" it to one grid + a per-dobject filter, 2D users start paying for a 3D feature they
> don't use — and the index becomes wrong across planes anyway (§9.1).

---

## 8. Decisions I need from you

> ### ⚠️ §8.1 REVISED 2026-07-15 — I was wrong; the owner's instinct was right.
> My original §8.1 recommended accepting kernel divergence and **rejected** the side-table. That
> recommendation **contradicted an existing, documented project decision** I had not read:
>
> > **D5 — Persistence = sidecar.** `drawing.simlux.json` beside `drawing.rsm`
> > *(`SIMLUX_LUX_WORKFLOW.md`)*
>
> and `cad_app/src/simlux_io.rs` states it outright:
> > *"All SIMLUX-specific state … lives here, **NOT in the (2D) `cad_kernel` document**. Keyed by
> > **STABLE NAMES** … `cad_kernel` / `cad_io` stay **UNTOUCHED** (decision D5)."*
>
> simLUX has **already solved "isolate the engine, add alongside" once** — for the entire lighting
> feature. Planes should follow the same precedent. See §8.1a/§8.1b below.

### 8.1 — In what sense does `cad_kernel` change? (the precise answer)

My §3 proposal is **exactly two field additions**, plus additive types:

| Change | Kind | Blast radius |
|---|---|---|
| `DObject.plane: PlaneId` | **struct change** | breaks every struct-literal site: **26** (23 `dobject.rs`, 1 each `blockdiff.rs`, `spatial.rs`, `rsm.rs`) |
| `Document.planes: PlaneTable` | **struct change** | ditto, small |
| `PlaneId` / `DPlane` / `PlaneTable` (new module) | **purely additive** | none |
| `Frame` shared into the kernel | additive | none |

So "the kernel changes" = **2 fields**. Everything else is new files. But those 2 fields are what
break byte-identity with RUST_CAD.

### 8.1a — How to isolate: the SIDECAR (recommended, follows D5)

**Zero kernel change. Zero `cad_io` change. Zero RSM version bump.**

```rust
// cad_app/src/factory_io.rs — a NEW sidecar, or extend SimluxConfig (D5 pattern)
pub struct FactoryConfig {
    /// plane NAME → definition (name-keyed, per D5: ids are positional, names are stable)
    pub planes: BTreeMap<String, PlaneDef>,      // origin[3], u[3], v[3], color
    /// dobject HANDLE → plane name. Handles ARE stable: they round-trip through RSM
    /// (`write_u64(w, d.handle)` / `r.u64()`), unlike positional layer/block ids.
    pub dobject_plane: BTreeMap<u64, String>,
}
```
Reconcile rule: **a dobject whose handle is absent from the map belongs to the active plane.**

**Why this is CORRECT (and why my "two sources of truth" objection was wrong):** it holds under the
invariant the design already enforces — *tools only ever see the active plane* (§3.1, D2–D5). If
every dobject a tool can touch is on the active plane, then every dobject it creates belongs there
too. The rule isn't a guess; it's implied by the filter.

**The two honest limits:**
1. **Cross-plane copy breaks it.** A copy gets a **new handle** (`copy.handle = next_handle()`,
   `app.rs:18137/18183`) → absent from the map → lands on the **active** plane, not the source's.
   Fine while selection is plane-filtered; wrong the moment cross-plane ops exist.
2. **⛔ BLOCKED by a real handle-collision bug — see §8.1c.** Handle-keyed attribution is unsafe
   until it's fixed.

### 8.1b — The staged answer to "isolate now, add later"

| Stage | Where planes live | Kernel | When |
|---|---|---|---|
| **Now** | **Sidecar** (`FactoryConfig`, D5) | **untouched** | prove the design in 3D_Factory |
| **Later** | Promote to `DObject.plane` + `Document.planes` (§3) | 2 fields | **only if** cross-plane ops are needed, and as part of the one big merge |

The sidecar is not a hack — it is **this project's established answer** to exactly this question,
and it makes the whole plane feature revertible by deleting one file.

### 8.1c — ⛔ BLOCKER: `HANDLE_COUNTER` is never bumped on load (real, proven bug)

`cad_kernel/src/dobject.rs`:
```rust
static HANDLE_COUNTER: AtomicU64 = AtomicU64::new(1);   // starts at 1 EVERY session
pub fn next_handle() -> Handle { HANDLE_COUNTER.fetch_add(1, Ordering::Relaxed) }
```
Nothing raises it after a load, but RSM **preserves** handles. Proven by
`cad_io/src/rsm.rs → known_bug_next_handle_collides_with_a_loaded_handle` (`--ignored`):
```
PROBE: loaded handle = 1000000, next_handle() = 2
```
Open a drawing → the next object drawn is handed a handle a loaded object already owns.

**This is bigger than planes.** `Hatch.boundary_handles` resolves its boundary **by handle**
(`rsm.rs:433-435`), so a collision can bind a hatch to the wrong geometry. It exists in **RUST_CAD
too** (byte-identical kernel).

**Fix — additive, ~5 lines, no struct change:**
```rust
// cad_kernel/src/dobject.rs
pub fn reserve_handles_above(max: Handle) {
    HANDLE_COUNTER.fetch_max(max.saturating_add(1), Ordering::Relaxed);
}
```
called at the end of `read_rsm` with the max loaded handle. A **pure bug fix** that upstream wants
anyway → the easiest possible merge, and it unblocks the sidecar.

**8.2 — v7 readers meeting a v8 file:** refuse (my recommendation — silent geometry corruption is
worse than an error), or flatten to World?

**8.3 — Scope check:** Phases B–D are the real work (D5 especially). Confirm you want the full
one-document model, **or** say the word and I'll keep the current per-sketch documents — they work
today, at the cost of the doc-swap, split undo, and no combined display.

*MD only — no code changed by this review.*
