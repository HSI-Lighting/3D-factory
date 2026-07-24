# Spec: boolean is a COMMAND, not a feature property

**Owner critique, 2026-07-17:** *"boolean function is an independent function and can not be
part of 3d dobjects, logic is not allowing. once we are doing boolean we select both and
carry on the function."* Correct — and it exposes a model problem, not a UI one.

---

## The finding — why boolean is in the create dialog today

`cad_solid::Model` is a **single fused CSG tree**:
- `Model { features: Vec<Feature> }`; each `Feature { id, op: BoolOp, primitive, … }`.
- `eval()` folds **every** feature left-to-right by its `op` into **one** `SolidMesh`.
- `factory.rs` pushes each primitive **with** an op: `model.push(next_op, …)`.

So a boolean only exists **as a property of a feature**, chosen at birth. That is exactly why
the radio sits in the Create dialog — there is nowhere else for the op to live. The owner's
instinct ("a lone box carrying Difference has nothing to differ from") is the model talking.

## Why the current model can't host the command cleanly

The workflow wanted is the modifier contract (like move/copy): **select 2+ solids → Enter →
operate → a combined body.** On a fused tree:

| command | on the fused tree | verdict |
|---|---|---|
| `union A,B` | both are already Union features ⇒ already fused | **visual no-op — looks broken** |
| `intersect A,B` | set ops, but fold is global | leaks onto other features |
| `difference keep∖tool` | set tool.op=Difference + reorder | works-ish, but order-global, not pairwise |

`union` being a no-op is the tell: the model has **no notion of "combine these two into one
body."** Everything is already one body.

## What's actually needed — a multi-body model (cad_solid, mentor-only)

Introduce **bodies**: the model holds independent bodies; a boolean **consumes** its operands
and **produces a new body**. Minimum shape:

- A body is either a `Primitive` (as today) **or** a **baked boolean result** (a `SolidMesh`
  plus its provenance), so a boolean can be stored as a first-class body.
- `Model` gains, roughly:
  ```
  fn boolean(&mut self, op: BoolOp, keep: &[u32], tools: &[u32]) -> u32
  //  union/intersect: keep = all picked, tools = []   (n-ary, commutative)
  //  difference:      keep = KEEP set,  tools = CUT set
  //  → evaluates via csgrs, REMOVES the operands, pushes ONE result body, returns its id
  ```
- csgrs already provides the math (`union`/`difference`/`intersection`); this is **structure +
  wiring**, not new geometry code.
- Undo: one entry — operands back, result gone.

**Creation** then stops passing an op at all: every `push` makes an **independent Union body**.
The `BoolOp` field can stay for the baked-result provenance, but new primitives never choose it.

## The command workflow (cad_app — mine)

Joins the SAME `active_view == ThreeD` modifier dispatch as move/copy (one command, select-if-
empty → Enter → operate). No parallel commands.

- **`union` / `intersect`** — n-ary, commutative: select 2+ solids → Enter → one body.
- **`difference`** — **two explicit prompts** (owner's choice, 2026-07-17):
  1. `select body to KEEP` → Enter
  2. `select bodies to SUBTRACT` → Enter
  → `keep ∖ (union of tools)` → one body.
- The **radio leaves the Create dialog**; creation is always an independent body.

## Split of work

- **cad_app (mine):** the three commands + dispatch + prompts + removing the radio + wiring to
  `Model::boolean`. Selection already addresses features by id (`pick_feature`), so the
  operand sets are in hand.
- **cad_solid (MENTOR-ONLY):** the multi-body change + `Model::boolean`. Per
  `feedback_simlux_3d_mentor_role` I spec this, I do not write it — **unless the owner
  authorises writing it directly for this feature.**

## The one decision blocking the build
Who writes the cad_solid multi-body change: **(a)** I write it (owner waives MD-only for this),
or **(b)** it's specced here for the mentor/owner and I build only the cad_app command against
its API once it lands. Everything on the cad_app side is ready to go either way.

## Run
```bash
cd ~/workspace/simLUX/3D_Factory && cargo run -p cad_app
```
