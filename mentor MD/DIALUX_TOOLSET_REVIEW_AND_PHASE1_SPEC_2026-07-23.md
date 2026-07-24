# DIALux toolset — repo reconciliation + Phase 1 spec

**Date:** 2026-07-23 · **Status:** spec for owner decision, no code written ·
**Input:** mentor's "3-Phased Integration Plan for DIALux-Style Modeling Tools"

The mentor's plan was written without sight of the tree. Read against the code, roughly
**two thirds of Phase 1 and half of Phase 2 already exist** — in the 2D document. The work is
therefore not "build these tools", it is **"route the existing tools through the 3D view, and
add the one data structure that has no 2D analogue (Storey)."**

This document does three things: (1) reconcile every mentor item against the repo, (2) list the
five collisions with already-settled decisions, (3) spec Phase 1 as it actually reduces here.

---

## 1. Reconciliation — mentor item vs. what is in the tree

Legend: **✅ ships** · **🔶 partial** · **🆕 new** · **⛔ blocked by an owner ruling**

### Phase 1

| Mentor item | Status | Evidence |
|---|---|---|
| 2D plan view + synced 3D view, toggle | ✅ | `ActiveView::{TwoD,ThreeD}`; the `doc`-swap sketch session (`factory.rs`) |
| Camera orbit / pan / zoom | ✅ | `light3d::mvp`, ViewCube nav gizmo, `z` command at 2D parity, wheel dolly |
| **Project → Storey → Room hierarchy** | 🆕 | **zero occurrences of `Storey` in the whole tree** — the one genuinely new structure |
| Storey add/delete, floor-to-floor height | 🆕 | as above |
| Room/Space tool (closed polygon → volume) | 🔶 | `FactoryState::add_wall(footprint, thickness, height)` already extrudes a footprint into one wall per edge. Missing: loop closure + floor/ceiling slabs |
| Wall tool, click-drag, ortho snap | ✅ | the real 2D `wall` command; `cad_snap` (137 `snap_*` call sites) |
| Walls auto-join / trim at intersections | ✅ | `trim` `extend` `fillet` `chamfer` `join` `break` `offset` — all in `MODIFY_CMDS` |
| Floor & ceiling slabs | 🔶 | `Primitive::Box` gives the slab; no room-boundary generator |
| Basic selection, highlight | ✅ | 2D selection; 3D `pick_feature` / `pick_face` + `sync_selection_mesh` |
| Delete | ✅ | `erase` (2D) · `FactoryState::erase_selection` (3D) |
| Vertex editing (drag a vertex) | 🔶 | **engine done, viewport owed** — `wall_move_vertex` written + unit-tested; this is Track A slice 2–3 |
| Window/door cutouts | ⛔ | needs a boolean; owner ruled boolean is an independent command needing a multi-body `cad_solid` decision → `BOOLEAN_AS_COMMAND_2026-07-17.md` |

### Phase 2

| Mentor item | Status | Evidence |
|---|---|---|
| Add point to contour (L-shaped rooms) | 🔶 | `wall_insert_vertex(wi, seg, at)` written + tested = Track A slice 4 |
| Multi-storey void / atrium | ⛔ | boolean, same blocker |
| Roof tool (flat / pitched / shed) | ⛔ | `Feature.plane` is `PlaneKind::{XY,XZ,YZ}` and `Placement` offers only `{u, v, lift, spin_deg}` — **spin about the plane normal, no tilt DOF.** A pitched roof is geometrically unrepresentable today. Same blocker as `rake_deg` (stored, not applied) |
| Furniture / object library | 🆕 | but `SIMLUX_DIALUX_PLAN.md` §9.6 already locked **glTF 2.0** as the import format |
| Copy & Array wizard | ✅ | `copy` and `array` are established 2D commands |
| Boolean subtract | ⛔ | `BoolOp::{Union,Difference,Intersection}` exists in `cad_solid` and `csg::eval` honours it — the *engine* is there; the **command journey** is what the owner blocked |
| Show/hide via eye icon | 🔶 | `cad_kernel::Layer` already carries `visible` / `locked` / `frozen`; no 3D tree UI |
| Zoom to selected | 🔶 | `fit()` does extents; needs a selection-scoped variant |
| Material assignment per surface | 🆕 | **this IS Track B.** `Feature` has no material field; `cad_light::Material` + `default_materials()` (0.20/0.50/0.70) exist and wait |

### Phase 3

| Mentor item | Status | Evidence |
|---|---|---|
| Glass / refraction / frosted | 🆕 | `cad_light::Material` is **Lambertian only** (`reflectance`, `color`). Specular/transmissive is a solver change, not a UI change |
| Reflective / specular surfaces | 🆕 | as above — and it is the *calc* that must learn it |
| Single-face material override | 🆕 | needs `SurfaceId` — Track B move 4 |
| Terrain / site | 🆕 | genuinely absent |
| **DWG/DXF import** | ✅ | `cad_io`, 2,515 lines, shipping |
| DXF-layer → auto 3D walls | 🔶 | **already specced**: `SIMLUX_DIALUX_PLAN.md` §5 "Layer → 3D dialog" with a Role column (Wall/Floor/Ceiling/Opening/Obstruction) |
| IFC import/export | 🆕 | large, and no lighting number depends on it |
| FBX / OBJ / STL / DAE importer | 🆕 | **conflicts** with the locked glTF-only decision (§9.6) |
| General polygon cutout on any surface | ⛔ | boolean blocker |
| **Trim / extend** | ✅ | shipped in 2D since day one — listed in the mentor's Phase 3, three phases too late |

### Cross-phase "prerequisites from day 1"

| Prerequisite | Status |
|---|---|
| Parametric data model | ✅ `Primitive::Box{w,d,h}` etc. are parametric; `Draw3dDialog` already live-edits a selected solid's dimensions; `WallInst` carries `thickness`/`height`/`rake_deg` |
| **Undo/redo stack** | **🔶 and this is the real hole** — `snapshot_doc()` clones the **`Document`** onto a bounded `undo_stack` with a `redo_stack`. It does **not** cover `FactoryState` / `cad_solid::Model`. Every 3D solid operation is currently **not undoable** |
| Snapping service | ✅ `cad_snap` + `FactoryState::snap_vertex` + `cursor_on_plane` |
| Layered rendering (geometry pass / overlay pass) | ✅ enforced already — overlays must paint on `LayerId::new(Order::Foreground, …)` or the opaque 3D texture hides them |

---

## 2. The five collisions — owner decisions needed before any code

**C1 — The plan contradicts the standing strategy.**
`SIMLUX_DIALUX_PLAN.md` §0 says, in bold: *"Do not interpret [a functional DIALux] as 'clone
DIALux the application' — that target is unbounded … chasing the app never converges."* It puts
the gap at **80 % product-surface / 20 % solver** and says *"invest in the product surface; do
not rewrite the physics."* `LIGHTING_3D_STACK_RESEARCH_2026-07-22.md` concludes *"keep the
modeller adequate and invest in `cad_light`."*
The 3-phase plan is a **full modeller clone** — the opposite prioritisation. Phase 3 alone (IFC,
terrain, FBX/OBJ/STL/DAE, glass BSDF) is multi-year and produces **zero lux numbers**.
→ *Decision needed: is the target still EN 12464-1 compliance, or has it moved to modelling
parity? They fund different quarters.*

**C2 — Several Phase 1/2/3 items would violate Rule 3 (never reimplement a 2D feature in 3D).**
Wall drawing, ortho snap, selection, delete, copy, array, trim/extend all exist. The correct
reading of those mentor bullets is **"make the existing command work when `active_view ==
ThreeD`"**, never "build a 3D version".

**C3 — Rule 4 (MOVE is MOVE).** "Room tool", "Copy & Array wizard", "Vertex editing" must not
become new verbs. `array`/`copy` dispatch on the active view at apply time. Only genuinely new
nouns (Storey, Room) earn a new command. There is a test named
`established_commands_are_untouchable`.

**C4 — Roofs and openings are hard-blocked, not merely unbuilt.**
- Openings + atrium void + polygon cutout → all one blocker: the multi-body boolean decision.
- Pitched/shed roof + wall rake → all one blocker: `Feature` has no tilt DOF.
Both are `cad_solid` changes ⇒ Rule 2 ⇒ spec into `mentor MD/` and get sign-off **before** Rust.
Two specs would unblock roughly a third of Phases 2–3.

**C5 — The plan omits the mesh seam entirely.** `cad_solid::SolidMesh` (flat soup, no material,
no grouping) → `Vec<cad_light::Mesh>` (indexed, one material per surface) is unbuilt. Mentor
Phase 2 §5 (material assignment) and Phase 3 §1 (single-face override) are **literally Track B
moves 4–6** and cannot be built before it. Track B is the prerequisite the plan is missing.

---

## 3. Phase 1, as it actually reduces here

Delivering the mentor's Phase 1 goal — *"draw a fully enclosed, multi-storey building box"* —
needs **four** work items, not fifteen. Openings are cut from Phase 1 (C4).

### 1-A · Undo coverage for the Factory  *(do first — it is a prerequisite, and it is cheap)*
- Extend the existing snapshot pattern to the 3D side: every `FactoryState` mutation
  (`add_primitive`, `add_wall`, `wall_move_vertex`, `wall_insert_vertex`, `wall_delete_vertex`,
  `set_wall_height`, `erase_selection`) snapshots `cad_solid::Model` + `walls` before mutating.
- **Same stack, same `UNDO_STACK_CAP`, same redo-invalidation rule** as `snapshot_doc()`. One
  undo command, dispatching on active view — Rule 4.
- Validation: after undo, feature ids must be exactly the pre-op ids (a `rederive_wall` mints
  new ids, so a naive undo would strand any selection — refresh selection on undo).

### 1-B · The Storey hierarchy  *(the only genuinely new data structure)*
```
Project
└── Storey { name, base_z, height }        // ordered, base_z derived from the stack
    └── owns: feature ids, wall indices
```
- Data validations: `height > 0`; storeys are contiguous (storey *n*'s `base_z` = storey
  *n−1*'s `base_z + height`) — inserting or deleting a storey **re-derives every base_z above
  it and shifts the owned features by the delta**; deleting the last storey is refused; the
  active storey is always valid.
- One active storey drives (a) which plane a sketch opens on, (b) what a new wall's `height`
  defaults to, (c) what the 2D plan view shows.
- Interaction: a storey list panel — add above / add below / delete / rename / set height /
  click to activate. No new verb needed if it lives in a panel.

### 1-C · Room from a closed footprint  *(mostly a thin wrapper on shipped code)*
- Journey, matching the confirmed wall journey exactly (*draft in 2D → select → right-click →
  Make 3D wall*): draft a **closed** polyline with the real 2D tools → select → right-click →
  **Make room**.
- Implementation: `add_wall(footprint, …)` already does the walls (one Box per edge). Add a
  floor slab at `base_z` and a ceiling slab at `base_z + height`, both from the same footprint.
- **Preserve the footprint invariant**: floor and ceiling rings derive from the *same* points.
  Never give them independent vertex lists.
- Validations: footprint must be closed, non-self-intersecting, ≥3 points, area > ε; reject
  with a message, never silently.

### 1-D · Track A slices 2–5 — vertex handles in the viewport
This is the already-owed work, and it discharges mentor Phase 1 item 4 *and* Phase 2 item 1.
Engine is written and unit-tested; only the viewport wiring is missing.

| Slice | Gesture (needs owner confirmation — Rule 8) | Calls |
|---|---|---|
| 2 | handles shown only for a **selected** wall | project via `light3d::mvp`, hit-test in screen space |
| 3 | **drag a dot** = move | `wall_move_vertex`, snapped via `snap_vertex` / grid |
| 4 | **click an edge** = add a corner at the cursor | `wall_insert_vertex` + `cursor_on_plane` |
| 5 | **right-click a dot** = delete | `wall_delete_vertex` (never drops below 2 points) |

Handles must paint on a **Foreground layer** or the opaque 3D texture hides them — this is the
mentor's "overlay pass" prerequisite, already enforced here.

### Explicitly deferred out of Phase 1
Windows/doors (C4 · boolean), roofs (C4 · tilt DOF), materials (C5 · Track B first),
IFC / FBX / terrain / glass (C1 · needs the strategy answer).

---

## 4. Recommended sequence

1. **Answer C1.** Everything downstream is scoped by it.
2. **1-A undo**, then **1-D Track A** (owed, engine-ready, highest visible value per hour).
3. **1-B Storey** + **1-C Room** — completes the mentor's "enclosed multi-storey box" goal.
4. **Track B, the mesh seam** — the plan's missing prerequisite; unblocks all material work and
   is the thing that actually feeds `cad_light`.
5. Two specs into `mentor MD/`, for sign-off: **multi-body boolean** (unblocks openings, voids,
   cutouts) and **`Feature` tilt DOF** (unblocks roofs and wall rake).
