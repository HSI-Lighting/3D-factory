# METHOD ACCESS — Menu & Palette (mentor spec)

Gives the **menu** and the **command palette** the same drawing-method access the
**rail** ▼ flyouts already have — for the three method-bearing commands:
**arc, circle, fillet**. It mirrors the rail's split interaction so the muscle
memory is identical across surfaces.

This is the menu+palette half of the command-registry roadmap's "command
methods / variants" item. The **command-bar** half stays deferred (it would touch
the frozen parser/flow).

**Scope:** all **app-layer**. Method dispatch flows through the EXISTING
`run_command` / method-dispatch helper. **The parser, `run_command`, and tool
state machines are never modified.**

---

## Non-negotiables

- **App-layer only** — frozen code (parser / `run_command` / tools) untouched.
- **ONE canonical method source** shared by rail + menu + palette. No per-surface
  method lists — they drift.
- **Non-method commands** (line, rectangle, text, …) render **exactly as before** —
  no arrow, no submenu, no variants.
- **Method memory** is the existing command-level `command_method`. Picking a
  method on ANY surface sets it, and it applies everywhere (`execute(id)` already
  reads it).

---

## 1. One canonical method source

Consolidate each command's method list — **full name + short-code** — into a
SINGLE source (today's rail-flyout definitions):

- **circle:** Center, Radius `CR` · 3-Point `3P` · 2-Point `2P` · Tan, Tan, Radius `TTR`
- **arc:** its methods (`SCE`, `3P`, …)
- **fillet:** its methods

**Canonical label format (rail + menu + palette — identical everywhere):**

> **`Full Name (CODE)`** — full method name, one space, code in parentheses,
> **code always UPPERCASE.**
> `Center, Radius (CR)` · `3-Point (3P)` · `2-Point (2P)` · `Tan, Tan, Radius (TTR)`.
> A base/method-command label shows its current method the same way: `Circle (CR)`.

This **fixes the notation drift** (the flyout's `3p` / `3pt` → `3P`) and replaces any
`… CODE` ellipsis form. The code is `data-code` (Mono 11), one space before `(`.

Rename the method-dispatch helper (`rail_flyout_dispatch`) to a surface-neutral name
(e.g. `dispatch_method`) since menu + palette now call it too.

---

## 2. Menu — split-click item (mirrors the rail)

For arc/circle/fillet the Draw/Modify menu item becomes:

```
[glyph] Circle (3P)   ▸
```

- **Point / hover the ▸ arrow** → the **method flyout opens** (no click). It lists all
  methods (full name + code), the current one in **cyan** (see §5; no ●/□). The flyout
  stays open while the pointer is over the arrow **or** the flyout, and closes shortly
  after the pointer leaves both (a small diagonal-travel tolerance so you can move onto
  the flyout).
- **Click the label / body** → `execute(id)` → runs the **remembered** method
  (one-click draw).
- **Click a flyout item** → `dispatch_method(cmd, method)` → runs it AND sets
  `command_method` (so rail + menu + palette all update).
- The label shows the current method `(3P)` and a thin method-aware glyph.

**Click never *opens* the flyout — hover opens, click commits.** Keep the hand-rolled
`Area` flyout (it's what allows a separate label-click-to-run); only its **trigger is
hover, not click**. This is NOT a plain egui hover-submenu — that can't host the
separate label-click-run.

### 2.1 Flyout visual — ONE design (rail + menu)

The method flyout looks **identical** whether opened from the **rail ▼** or the **menu
▸**. The **menu (dropdown) flyout is the reference**; the **rail flyout is updated to
match it** (rail update is a follow-up task — see backlog).

- **Corner radius:** `radius::SM = 4` (user preference — sharper corner). Same on both
  flyouts. *(MD(8) was tried and reverted 2026-07-04.)*
- **Rows:** same as the menu/palette — method-aware glyph in the **icon box (band − 6 =
  20)**, **icon→name gap 14**, label `Full Name (CODE)` UPPERCASE (§1), **current method
  in cyan** (§5; no ●/□), full-width `surface-2` **hover**.
- **Width:** hug the longest method line (§7).
- **Fill / border:** popover `surface-3` + 1px `border`, per theme.

Net: rail ▼ flyout and menu ▸ flyout are the same widget's look — same radius, icons,
gap, label format, and cyan current-method marker.

---

## 3. Palette — base + variants

When results include a method-bearing command:

- **Base entry:** `[glyph] Circle (CR)` → `execute(id)` (remembered method). The
  `(CR)` code is cyan (it names the current method — see §5); the name is normal tone.
- **A "methods" group** below it — one entry per method, using the §1 label format:
  `[glyph] 3-Point (3P)` → `dispatch_method(cmd, method)` → runs **and** sets it.
  The current method's whole row is cyan (§5).
  - **Naming:** variants show the **method only** (`3-Point (3P)`), not `Circle · 3-Point`
    — they're visually nested under the base, so the command word is redundant.
  - **Indent (option A):** method rows are indented so the **method glyph aligns under
    the base command's *name*** (leading spacer = base glyph width + gap). The glyph
    leads each method row; the name follows.
  - **Dim tier:** method names are one shade dimmer than command names (`#97A3AD` vs
    `text-primary`), so the command leads and methods recede.
  - **Code placement:** `(CODE)` sits immediately after the name with a small fixed
    gap — **left-grouped, never right-aligned** to the panel edge.
  - **Presentation:** variants stay **inline** (this is a *search* surface — inline
    keeps every method visible and typeable, e.g. type `2P` to filter). Do NOT hide
    them behind a per-row flyout. (Menu uses arrow→flyout; palette does not — the two
    surfaces have different jobs.)
- Each entry's glyph = that method's own construction glyph.

### 3.1 Palette chrome

- **Header:** the shared header foundation, **Floating** variant — see
  `HEADER_STANDARD_MENTOR.md` (32 chrome band, Geist 16/500 title "Command palette",
  close ×). No ad-hoc header.
- **Draggable:** the Floating band is the **drag handle** — dragging the band moves the
  palette window (Floating-variant behaviour; dialogs inherit it later).
- **Search field:** a **full-round pill**, height **24**, radius = half-height,
  fill `surface-0 #141C25`, 1px `border`, side pad 12, text = body (Geist 13),
  placeholder `text-muted`. Keep the placeholder **short** (e.g. "Search commands") so
  it does not force extra width — see the width rule (§7).
- **Sticky search:** the search pill + a **1px `border` divider** directly beneath it
  form a sticky block **pinned to the top**; the result list scrolls under it.
- **Scroll clip:** the result list is **clipped to the panel's rounded border** with a
  small bottom inset — the last row stops **at** the bottom hairline (no droop past it).
- **Row band + icons:** row / hover-band height **26**; **icon box = band − 6 = 20**
  (3px inset top + bottom); every glyph **scales to fill that box uniformly** — no
  natural-size or band-stretched glyphs (see §4). **icon → name gap = 14.**
- **Width:** **hug** the longest result line — see §7. No arbitrary fixed width.
- **Row hover:** full-width fill `surface-2 #222B34`, text unchanged, instant (no
  animation), matching the rails.
- **Keyboard selection:** the arrow-key cursor row = the same `surface-2` fill **plus a
  2px cyan (`accent`) left bar**. This is independent of the current-method cyan (§5):
  cyan **text** = current method, cyan **left bar** = keyboard cursor.

---

## 5. Current-method marker (menu + palette)

The current/remembered method is indicated by **rendering its label + code in cyan**
(the accent) on **both** surfaces — the menu label's `(3P)` and the current row in the
menu submenu and palette methods group. **No `●` dot, no `□` checkbox** — colour is the
marker. Non-current methods use normal label tone.

---

## 4. Glyphs

Thin (**~0.9–1px** stroke), muted tone, **lighter than the label** (THEME_SYSTEM
§5.12 — CAD glyphs are recessive, monochrome; the label leads, the icon supports).
Each method has a **distinct construction glyph**:

- **CR** = circle + center dot + radius line
- **3P** = circle + three points on the circumference
- **2P** = circle + two points
- **TTR** = circle tangent to a corner (two lines)
- arc / fillet methods analogously.

The base entry's and the rail's icon are **method-aware** — they show the
*current* method's glyph, not a generic one.

**Uniform across every row:** all glyphs — method-bearing or not (pointer, line,
polyline, …) — share **one stroke weight** and sit in a single **aligned icon column**.
No icon is heavier or larger than its neighbours.

**Icon box (size rule):** each glyph draws into an **icon box = hover-band height − 6**
(3px inset top + bottom) so icons never touch the band edges. The shared glyph fns take
a **scale param** and scale to fill that box — glyphs must **not** draw at their natural
size (some end up too small) nor stretch to the full band height (some end up too big).
For the palette: band 26 → **icon box 20**.

---

## 7. Surface width & icon-box rules (shared: palette, menus, flyouts)

- **Width (proportional, no arbitrary fixed width):**
  `width = clamp(content_width, MIN, MAX)`, where
  `content_width = edge + icon(20) + gap(14) + longest line + (arrow gap + arrow, if
  any) + edge`.
  - Menus / flyouts: pure **hug** (MIN = content).
  - Palette: hug the longest **result** line; do **not** pad to the search text (keep
    the placeholder short). A modest MIN keeps it from getting too narrow; MAX caps it.
- **Icon box:** as §4 — `box = band − 6`, glyph scales to fill; identical across
  surfaces so an icon is the same physical size in the palette and the menus.

---

## Exit criteria

- **Menu:** click "Circle" label → draws with the remembered method; click ▸ →
  submenu opens; pick 2P → draws 2P AND the rail + palette now show 2P as current.
- **Palette:** "circle" → base + inline method variants (indent A, dim tier, glyph
  under the command name, `(CODE)` left-grouped after the name); base runs the
  remembered method, a variant runs + sets it. Header = shared Floating band; search =
  full-round 24 pill with a sticky divider; hover = `surface-2`; keyboard cursor =
  `surface-2` + 2px cyan left bar.
- **Consistency:** one label format `Full Name (CODE)` (UPPERCASE) across rail + menu +
  palette — no `3p`/`3pt`, no `… CODE`; ONE method source; one icon stroke/size.
- **Untouched:** non-method commands render as before; the rail is unchanged;
  `run_command` / parser / tools are not modified.
