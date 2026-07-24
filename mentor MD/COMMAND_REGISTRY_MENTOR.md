# COMMAND REGISTRY — Design & Migration

Companion to `COMMAND_SYSTEM.md`. Builds a **metadata registry** that drives the
UI surfaces (rails, menus, palette) while **execution stays exactly where it is**.
Delivered in small, independently shippable, reversible phases.

**In one line:** the registry *describes* commands; `run_command` still *executes*
them, unchanged. This sidesteps the Rust borrow trap of storing `&mut CadApp`
closures inside `CadApp`.

---

## Non-Negotiable Architectural Decisions

These hold across every phase. Any change that violates one is out of scope and
must be raised before proceeding.

**D1 — The registry is metadata only.**
It never executes, parses, or holds runtime state.
*Reason:* storing execution in the registry means storing `&mut CadApp` closures
inside `CadApp` — an ownership/borrow conflict.
*Rejected:* a `run`/handler/closure/`FnMut` field on a command → couples UI
metadata to execution and fights the borrow checker.

**D2 — `execute(id)` is the only execution seam.**
Every surface (rail, menu, palette, shortcut) calls `execute(id)`, which looks up
the command and calls `run_command(cmd.dispatch)`.
*Reason:* one place translates identity → execution, so a future executor change
touches one function.
*Rejected:* surfaces calling `run_command` directly, or passing `id` to it →
`run_command("draw.line")` matches nothing (no-op), and every call site couples to
the dispatch detail.

**D3 — `run_command` and the parser are never modified.**
They are outside the migration surface. `run_command`'s `match` arms stay keyed on
the **dispatch token** (`"line"`), never the namespaced id (`"draw.line"`).

**D4 — Registry identity (`id`) ≠ execution token (`dispatch`).**
`id` = namespaced stable string (`"draw.line"`) for UI, palette, and persistence.
`dispatch` = the `run_command` token (`"line"`). Both are derived from the source
arrays; `id` is never passed to `run_command`.

**D5 — Dependencies flow up; the kernel never depends on the app.**
`cad_app` may depend on `cad_kernel`; the kernel does not know the registry exists
(ARCHITECTURE §2). Shared data lives in the kernel and the app reads it.

**D6 — Presentation ≠ parsing.**
Parser shortcuts (`"l"`) are kernel input-parsing. UI discoverability (`keywords`)
is app metadata. They are different concerns that happen to share a letter.
*Rejected:* an `aliases` field, or "the parser reads the registry" → re-fuses the
two systems and forces a kernel→app dependency (violates D5).

**D7 — `Ctx` (context for predicates) is a projection, not a gateway.**
Predicates are pure reads of a read-only snapshot.
*Rejected:* `&mut CadApp`, `Rc<RefCell>`, interior mutability, or `ctx.app.*()` →
reintroduces the coupling the whole design removes.

**D8 — The registry never owns selection, undo, tool state, or multi-stage flows.**
Those belong to the tools and the document (see Scope Boundaries).

---

## Architecture

```
UI (Rails · Menus · Palette)
   │ read metadata → render        │ on click → id
   ▼                               ▼
Registry (metadata only) ────────► execute(id) ──► run_command(dispatch) ──► CadApp
                                    ▲
Command line / Parser ──────────────┘   (typed input → dispatch; frozen; unaware of the registry)
```

Two ways to produce a `dispatch` token — a UI click (via the registry + `execute`)
or typed input (via the parser) — both funnel through the one unchanged
`run_command`. The registry feeds rendering; it is never on the execution path
except to supply `cmd.dispatch` to the seam.

### The `CommandInfo` schema (accumulates across phases — never re-declared)

```
CommandInfo {
    id,        // CommandId  — namespaced identity "draw.line"
    dispatch,  // &'static str — the run_command token "line"
    title,     // display name "Line"
    tooltip,   // hover text "Line  (L)"
    category,  // CommandCategory { Draw, Modify }
    icon,      // IconId
    keywords,  // Phase 5 — UI search terms (NOT parser aliases)
    section,   // Phase 5 — optional sub-group within a category
    visible,   // Phase 6b — fn(&Ctx) -> bool
    enabled,   // Phase 6b — fn(&Ctx) -> bool
}
```

Supporting types:
- **`CommandId`** — an opaque, stable **string** identifier. Today `String`
  (runtime-derived by concatenation, so not `&'static str`); may later become
  `Arc<str>`/`SmolStr` if profiling warrants. Never an int/enum.
- **`IconId`** = `enum { DrawGlyph(&'static str), ModifyGlyph(GlyphKind) }` — spans
  both glyph painters (`draw_draw_glyph` / `draw_cmd_glyph`). A `Lucide` variant is
  added when Lucide UI icons are wired in.
- **`CommandRegistry`** — `HashMap<CommandId, CommandInfo>` for lookup, plus a
  **canonical ordered index** (`Vec<CommandId>` in source order) so `by_category`
  is deterministic for **enumeration surfaces** (palette / filtered views). **Menus and
  rails do NOT use `by_category`** — each has its own curated ordered id-list (config):
  rails = `draw_items`; menus = a per-menu `&[CommandId]` list (Phase 6).
- **`Ctx`** — a read-only projection (D7), built once per frame by `CadApp`, holding
  only what predicates read (selection, active tool, clipboard, snap, mode).

---

## Scope Boundaries — never migrate into the registry

The registry knows only *that a command exists* and how it looks (D8). It never absorbs:

- **Multi-stage prompt flows** — LINE's first-point / second-point / close / cancel.
  The tool owns its prompt state machine.
- **Undo** — `run_command → snapshot → execute → undo history`, untouched.
- **Active tool logic** — Mirror's pick → base → second → keep-copy, etc.
- **Selection logic** — the registry never decides which entities are selected or how
  selection works; predicates only *read* `Ctx.selection`.
- **Parser alias resolution** — the parser never reads the registry (D5/D6).

---

## Migration phases

Each phase is small, reversible, and has explicit exit criteria. Status in the
roadmap table. Standing rule: nothing in the freeze list (D3, D8) changes.

### Phase 0 — Freeze
Define what does **not** change: `run_command`, undo, tool state machines, parser
logic, the command-line prompt system, command history. Only the *description* of
commands moves into the registry. This phase writes no code — it is the contract.

### Phase 1 — Create the registry (schema only)
Define the types (schema above): `CommandInfo` with `{ id, dispatch, title,
tooltip, category, icon }`, plus `CommandId`, `IconId`, `CommandCategory`,
`CommandRegistry`. No data (Phase 2), no wiring, no closures. `keywords`/`section`/
`visible`/`enabled` arrive in their phases — do not add them early.
**Exit:** types compile; registry empty; nothing wired; app visually identical.

### Phase 2 — Populate from the arrays
Build the registry *from* `DRAW_CMDS` + `MODIFY_CMDS` at startup (the arrays stay
the single source; the registry is a derived copy — no second hand-written list).
Populate **both** Draw and Modify. Per entry, all derived: `dispatch` = col 2;
`id` = `"<category>." + dispatch`; `tooltip` = col 3; `icon` from col 1; `title`
derived from the tooltip by stripping a trailing `(KEY)` (a **temporary seed** —
wording is refined later, not the permanent model).
**Verify:** a temporary debug dump (Tools ▸ Debug) lists every entry.
**Exit:** registry populated (Draw + Modify), dump-verified; arrays intact; app identical.

### Phase 3 — Rails use ids instead of indexes
`draw_items`/`modify_items`: `Vec<usize>` → `Vec<CommandId>`; rails render from
`registry.get(id)` instead of `DRAW_CMDS[index]`. First behavior-touching phase,
user-visibly identical.
- **Introduce the `execute(id)` seam** (D2): rail clicks → `execute(id)` →
  `run_command(cmd.dispatch)`.
- **Defensive lookup:** `if let Some(cmd) = registry.get(id)` everywhere — a stale
  id is skipped on render / no-op on click, never `.unwrap()`/panic (plugins can
  remove commands later).
- **Icons via `IconId`** (`DrawGlyph → draw_draw_glyph`, `ModifyGlyph → draw_cmd_glyph`).
- **Support logic → id-based:** add/remove/reorder/reset
  (`app.rs:10434/10441/10516/10525`); the "available tools" list from `by_category`.
- **Flyouts** (arc/circle/fillet) key on `cmd.dispatch`, not `id`.
- Arrays stay as the registry seed (rails stop reading them, but they are not deleted).
- Persistence: none today (`draw_items` resets each launch), so no migration needed.

**Exit checklist:**
- [ ] Same commands, same order
- [ ] Drag-and-drop preserves order
- [ ] Add / remove / reset works
- [ ] Every stored `CommandId` resolves (no missing lookups / panics)
- [ ] Rail clicks execute the correct command via `execute(id)`
- [ ] Flyouts behave identically
- [ ] Icons render via `IconId`
- [ ] No user-visible behavior change

*(There is no separate "keep dispatch" phase — that is invariant D3, verified by this checklist.)*

### Phase 5 — UI metadata enrichment
Add two **static presentation** fields so surfaces present commands consistently,
without touching parsing or execution:
- **`keywords`** — UI search terms for the palette (`segment`, `straight` → Line).
  Hand-authored, single-source, no drift. **Not parser aliases** (D6).
- **`section`** — optional sub-group *within* a category, for richer menus
  (`Category → Section → Command`, e.g. Draw ▸ Curves ▸ Circle/Arc).

Not here: `visible`/`enabled` (Phase 6b, as predicates — a static bool can't express
context); no `aliases` field (D6).
**Exit:** registry carries `keywords` + `section`; execution/parser untouched.

### Phase 6 — Convert menus (Draw & Modify only)
Route the **plain command items** of the Draw and Modify menus through the registry,
**preserving each menu's current arrangement**. Other menus (File/Edit/View/…) stay
hand-authored — their items aren't registry commands.
1. **Curated ordered id-list, NOT `by_category`.** A menu is a *designed arrangement*,
   not a raw category dump (COMMAND_SYSTEM §4: the menu tree is configuration). Express
   each menu's plain items as an explicit ordered `&[CommandId]` list — like the rails'
   `draw_items` — preserving current order and membership, including cross-category
   placement (e.g. `modify.block` / `modify.insert` live in the Draw menu; the id-list
   doesn't care about category). `by_category` is for raw enumeration surfaces
   (palette / filtered views), not the menus.
2. **Hybrid, not flat** — only plain command items become id-list entries; keep dialogs
   (`Hatch…`, `Block…`, `Array…`), the `Insert Block ▸` submenu, separators, and specials
   (`Wall`, `Inspector…` — note `props` isn't a registry command) hand-authored and interleaved.
3. **Dispatch via `execute(id)`; KEEP current labels.** Each item resolves its id →
   `execute(id)`. Do NOT adopt registry `title`s yet — they're a temporary Phase-2 seed,
   and adopting them would *regress* real wording (`Arc (3pt)` → `Arc`, `Change Layer to
   Current` → `Change layer`). Menus keep their current labels until titles are properly
   authored; then they may adopt them.
**Exit:** plain menu items dispatch via `execute(id)` from a curated id-list; all special
items preserved; order / membership / labels unchanged; **app behaves identically**.

### Phase 6b — Context predicates
Turn `visible`/`enabled` into pure predicates over a read-only context (D7):
```
visible: fn(&Ctx) -> bool,
enabled: fn(&Ctx) -> bool,
```
- **`fn` pointers, not capturing closures** — `|c| … self.x` smuggles hidden state
  into what must be static rules.
- **`Ctx` is a projection, not a gateway** — no `&mut CadApp`, no `Rc<RefCell>`, no
  `ctx.app.*()`. Compute values when building `Ctx`; never fetch inside a predicate.
- **Defaults** `always_visible`/`always_enabled` prevent predicate explosion; most
  commands point at them.
- **`visible`** = should it appear at all; **`enabled`** = clickable right now.
  UI: `if visible { if enabled { active } else { disabled } }`.
- **Feeds every surface** (rails, menus, palette, right-click). Filtering is pure and
  iterates the ordered index — no mutation, no execution.
- **Staging:** add the fields defaulting to `always_*` (app identical), then add real
  predicates command-by-command.

One-way graph: `CadApp → builds Ctx → feeds registry → UI filters → dispatch executes`.
No predicate affects execution; the parser stays blind.
**Exit:** predicates are pure `fn(&Ctx)`; `Ctx` read-only & borrow-safe; execution/parser unchanged.

### Phase 7 — Command palette (+ keyboard shortcuts)
The last surface. `registry.iter() → filter(visible) → search(title + keywords) →
execute(id)`.
- Searches `title` + `keywords` — never parser aliases (D6).
- Dispatch via `execute(id)`; the palette only displays metadata.
- **Shortcut map:** `Accel → CommandId → execute(id)` — resolve to the id, not the
  dispatch token, so every surface stays identical (D2). Includes conflict detection.
**Exit:** palette searches title+keywords and dispatches via `execute(id)`; shortcut
map derived from the registry; execution/parser untouched.

---

## Roadmap

| Phase | What | Status |
|---|---|---|
| 0 | Freeze the architecture | ✔ locked |
| 1 | Create metadata registry (schema) | ✔ locked · implemented |
| 2 | Populate from `DRAW_CMDS` / `MODIFY_CMDS` | ✔ locked · implemented |
| 3 | Rails use ids instead of indexes | ✔ locked · **next to build** |
| 5 | UI metadata enrichment (`keywords` + `section`) | ✔ locked |
| 6 | Convert menus (Draw/Modify, hybrid, ordered) | ✔ locked |
| 6b | Context predicates (`visible`/`enabled` as `fn(&Ctx)`) | ✔ locked |
| 7 | Command palette + shortcut map | ✔ locked |
| — | Command methods / variants (arc/circle/fillet ▼ flyouts) | **method memory done** (command-level `command_method`; `execute` applies it everywhere). **Menu + palette method ACCESS specced** → `mentor MD/METHOD_ACCESS_MENTOR.md` (split-click menu item + palette variants + method-aware glyphs; app-layer). **Still deferred:** the command line reading/writing that memory (touches the frozen parser/flow) |
| — | Plugin registration + reserved fields | deferred |

> Stop-anywhere: shipping through Phase 3 already delivers registry-driven,
> customizable rails; the rest is additive.

> Post-migration cleanup (not now): once implemented, the phase sections can be
> retired and the permanent half (decisions + architecture) kept as the long-term
> reference.
