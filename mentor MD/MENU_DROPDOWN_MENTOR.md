# DROPDOWN MENU — category menus (mentor spec)

The menubar category dropdowns (Draw, Modify, …) and their submenus. Defines the
row layout, icon column, current-method marker, hover, spacing (matched to the
command palette), and the **arrow-column alignment rule** (which applies to every
menu/submenu app-wide).

Shares: the label notation and current-method marker from `METHOD_ACCESS_MENTOR.md`
(§1/§5); the icon language from `THEME_SYSTEM.md` §5.12. Parent tokens in
`THEME_SYSTEM.md` §5. **No raw numbers — every value is a named token in `theme.rs`.**

A menubar dropdown is a **flyout**, not a titled surface → it gets **no header band**.

**Applies to ALL category menus** (Edit, Draw, Modify, View, Formative, Utilities,
Tools, Help). **Draw is the reference** — every other category adopts the same row
layout, icon column, hover, spacing, arrow-column rule, hug width, dividers, and SM(4)
flyout radius.

**Method-specific bits only where a command has methods.** The cyan `(CODE)` marker
(§4), the `▸` method flyout, and the split label-click/hover-open behavior apply **only**
to method-bearing commands (Draw: Circle/Arc; Modify: Fillet; etc.). Non-method
commands are plain rows: icon + name, hover, no code, no arrow (unless they open a real
submenu, which still follows the arrow-column rule).

---

## 1. Row layout

Every command is one row, left → right:

```
[icon]  Name (CODE)                         ▸
└20┘ 14 └──── label zone ────┘  6  └arrow┘
```

- **Icon column (all commands):** every command shows an icon — **20px box**, one
  **uniform thin (~1px) stroke**, muted tone, in a single aligned column. If a command
  genuinely has no glyph, **reserve the empty 20px slot** so names stay aligned.
  Method commands (Circle/Arc/Fillet) show the **method-aware** glyph (current method's
  construction glyph). Icon box follows the **shared icon-box rule** (`box = band − 6`,
  glyph scales to fill) — `METHOD_ACCESS_MENTOR.md` §4/§7 — so an icon is the same
  physical size here as in the palette.
- **icon → name gap: 14** (matches the palette's shipped gap).
- **Name:** body (Geist 13/400), `text-primary`.
- **(CODE):** only on method commands — the current method, `data-code` (Mono 11),
  **cyan** (see §4), one **name↔code gap (6)** before it. Format `Name (CODE)` UPPERCASE
  (`METHOD_ACCESS` §1).
- **Arrow ▸:** only where the row opens a submenu (method submenu OR real submenu like
  Insert Block). One **unified size + tone** for all arrows (`text-muted`).
- **Shortcut hint (optional):** where a command has a keyboard shortcut, show it
  **right-aligned** in the trailing zone — **Geist 11 (`hint` token), `text-muted`**
  (sans, *not* mono — mono read too code-editor; sans matches the command names).
  Mono stays for `(CODE)` and numbers only. A row has **either** a shortcut **or** a
  submenu arrow, never both. The trailing element (shortcut or arrow) is what the width
  + alignment rule measures. Current Edit shortcuts: `Copy Ctrl+C`, `Paste Ctrl+V`,
  `Group Ctrl+G`, **`Select All Shift+A`**, **`Deselect All Ctrl+D`**.

**Metrics (exact — the SINGLE source; the one shared row painter uses these, no menu
overrides anything):**

| Element | Value |
|---|---|
| Row / hover-band height | **26** |
| Icon box | **20** (= band − 6, 3px inset top + bottom) |
| Left pad — menu inner edge → icon | **12** |
| Icon → name gap | **14** |
| Name → (CODE) gap | **6** |
| Longest line → **arrow** column gap | **32** |
| Trailing → right pad — arrow/shortcut → menu inner edge | **12** |

*(Shortcuts right-align to the 12 right pad; they do not use the arrow-column gap.)*
| Group divider | 1px `border`, 5 above/below |
| Flyout radius | `radius::SM` (4) |

Left/right pads live **inside** the row, so hover is edge-to-edge (§3). **Every category
menu goes through this one painter with these exact values.** A menu that "looks
different" is a menu **not using the shared painter** — the fix is to route it through
the painter, never to re-tune numbers locally.

---

## 2. Arrow-column alignment (rule — applies to EVERY menu/submenu)

- The **arrow column x = (right edge of the longest full line in the menu) + 32**.
  The longest line includes any parenthetical (e.g. `Wall (t = thickness)`).
- **All** submenu arrows in that menu align to that single column.
- The menu is **exactly wide enough** to fit `icon + longest line + gap + arrow +
  edge padding` — no far-edge void, no arbitrary width.
- This is the alignment reference for the whole menu; every arrow follows it.

### 2.1 Painting vs submenu mechanism (non-negotiable)

**Every row is custom-painted — including the rows that open a submenu** (method
submenus like Circle ▸ / Arc ▸, and real submenus like Insert Block ▸). Their icon box,
26 band, cyan `(CODE)`, hover, and **the ▸ on the aligned column** are painted the same
as any other row. No row is exempt from the visual rules because it opens a submenu.

The **submenu open/close mechanism is an implementation detail, never a visual
compromise:**
- **Prefer** the rail's existing hand-rolled `Area` popup (proven in this codebase) —
  toggled by the ▸ click, like the rail flyouts.
- **Only if** a hand-rolled popup is genuinely unstable inside the menubar-menu context
  (parent closing early) after real tuning, fall back to egui's `menu_button` **as an
  invisible interaction hitbox underneath the custom-painted row** — the row is still
  custom-painted and the arrow is still on the column.
- **Never** ship a natively-styled submenu row with a "hair off" arrow or a mismatched
  height/hover. Arrow alignment is a headline rule and the submenu rows are the *only*
  rows it governs — a visible gap there defeats the whole rule.

---

## 3. Hover

**Edge-to-edge full-width** row fill one elevation step above the menu fill
(`surface-2`-equivalent), text unchanged, **instant** (no animation) — identical to the
palette/rails hover. The highlight spans the **entire menu content width** (from the
menu's left inner edge to its right inner edge), **not** just the icon+label zone. The
row's *click* target is the same full-width rect.

**Order matters — hover must not drive width.** The menu width is set **first** by the
hug rule (§2 / §7) from the longest row. The hover then paints across **that** fixed
width. Do **not** derive the hover from `available_width` (or any loose/expanding
measure) — that inflates the menu and still misaligns the edges. Compute the intrinsic
content width, size the menu to it, then paint each row's hover to the menu's own width.

**Why "full-width" hover can still leave a gap (the real root):** two widths must be
made equal, or a sliver always remains:
1. The **menu `Frame` inner width** — egui picks this itself; after you zero the margin
   it is **not** guaranteed to equal the hug width `w` (it can be wider), so a highlight
   drawn at `w` stops short of the frame's true right border.
2. The **highlight width** — if painted at the row's own `w` rect, it can't reach a
   frame that's wider than `w`.

Fix (both, together):
- **Zero the frame's horizontal inner margin**, and move that padding inside each row.
- **Pin the menu content width to exactly `w`** (`ui.set_width(w)`) so the frame inner
  width **equals** the hug width.
- **Paint the highlight across the menu's actual content x-range** (`ui.max_rect()
  .x_range()`, the frame inner edges) — **not** the row's own allocated rect.

When frame width and highlight both derive from the same `w` and the margin is 0, the
highlight physically reaches both borders. If a gap persists, **measure**: log the frame
content rect `x` vs the highlight rect `x` for one row — the delta names the culprit.

> **⚠️ PARKED (2026-07-04) — a sliver still remains after content-rect fixes.** The gap
> is **not** the content inset (zeroed + width-pinned). It lives one layer OUT: the egui
> menu is `Area` + `Frame` (its own `inner_margin` / `stroke` / `rounding`) wrapping the
> content ui. Measuring highlight vs `max_rect()` is circular (same rect → always 0).
> **Resume by measuring the RIGHT pair:** content `ui.max_rect().x` vs the **outer
> menu/Area frame rect `.x`**. That delta names the outer inset (margin vs stroke vs
> rounding) — fix *that*, don't touch the content rect again.

---

## 4. Current-method marker (method commands)

`Circle (CR)`, `Arc (3P)` — the `(CODE)` is **cyan** (`accent`); the command name is
normal tone. **No `□` checkbox, no `●`.** Colour is the only marker (shared with
`METHOD_ACCESS_MENTOR.md` §5). Split behavior unchanged: click the **name** → run the
remembered method; click the **▸** → method submenu (`METHOD_ACCESS` §2).

---

## 5. Labels & dividers

- **Wall:** display `Wall (t = thickness)` — the parenthetical is a **muted hint**
  (`text-muted`); drop the `chained run —` text.
- **Dialog-opening commands** keep the `…` suffix (`Hatch…`, `Block…`).
- **Group dividers:** keep the existing 1px `border` hairlines between groups (e.g.
  above `Block…` / `Insert Block`).

---

## 6. Exit criteria

- Every command has an icon (or a reserved slot); all icons one stroke + one size,
  matched to the palette; method commands show the method-aware glyph.
- No `□`/`●`; current method shown by cyan `(CODE)`.
- Hover fill matches the palette; row height/spacing matches the palette.
- All arrows one size; arrows align to the longest-line + 6 column; menu width hugs
  content (no void). Rule holds for submenus too.
- `Wall (t = thickness)` (no `chained run —`); `…` kept on dialog commands; group
  dividers intact.
- No header band on the dropdown.

---

## 7. Icons — one swappable source (add later / redesign easily)

**All app icons are slated for a full redesign, so icon wiring must be a single swap,
never per-menu edits.**

- Every row's glyph comes from **one icon lookup keyed by command id** (a stable key) —
  `icon_for(command_id)`. Menus **never** hardcode a glyph inline.
- Every row **reserves the 20px icon slot** whether or not a glyph exists (missing →
  empty slot, present → glyph), so names stay aligned and a later icon drops in with
  **zero layout change**.
- **Add an icon later** = one entry in the lookup. **Redesign the whole set** = swap the
  glyph implementations behind the keys, in one place — no menu touches them.
- Where a glyph already exists elsewhere (New / Open / Save on the toolbar), wire it via
  the lookup so the **File** menu (and others) show it too — don't leave existing icons
  unused.

---

## 8. Non-command rows (Formative, Tools, …)

Menus that aren't pure command lists still use the **same chrome + metrics + hover**.
Two extra row types:

- **Section heading** (`Palettes`, `Command rails`, `Inquiry`): non-interactive. Caption
  **11/500 UPPERCASE**, dim (`#66707A`), at the **name column** (icon slot empty), small
  top pad; a group divider above it (except the first). **No hover.**
- **Checkbox / toggle row** (panel-visibility toggles: Command line, Layers, Pens,
  Inspector, …): a normal 26 row; the **checkbox sits in the icon-column slot**, reusing
  the **Inspector checkbox component** (`INSPECTOR_DESIGN_MENTOR.md` §5 / THEME §5.10):
  - **OFF → empty stroked box** (16×16, radius 4, 1px `border`, muted). **Always shown**
    — the empty box is what tells the user this row is a toggle and *can* receive a
    check. Never render OFF as a blank slot.
  - **ON → same box, cyan (`accent`)** with the check (accent check / `on-accent` glyph).
  - Name normal tone; full-width hover like any row. *(If a toggle ever also needs a
    command icon, the check moves right-aligned; default is check-in-slot.)*

---

## 9. Generalized flyout (real submenus)

The `▸` flyout must host **any** submenu, not just method/insert lists — Import,
Dimension, Styles, Debug tools, etc. Generalize it to render an **arbitrary list of
rows** with the **same** row design (icon / name / (CODE) / shortcut / ▸), the **same**
arrow-column + hug width, `SM(4)` radius, and **hover-open / commit-on-click** (§2.1).
Submenus may **nest** (a flyout row opens a further flyout) — identical rules at every
level. This is what lets File (Import), Formative, and Tools conform instead of staying
native.

---

## 10. Menu-launched surface positioning (everywhere)

**Any** surface opened from a menu item — a submenu flyout, a **dialog**, or a **panel**
(Layers, Pens, …) — is **anchored to the launching item**. It must never open behind,
to the left of, or otherwise away from where the user clicked (the user has to be able
to track where it went).

- **Parent menu STAYS open** (e.g. a submenu flyout): the new surface opens **adjacent
  to the right, top-aligned** to the launching row. The method/submenu **flyout is the
  reference — match it exactly.**
- **Parent menu CLOSES** (e.g. a dialog/panel that dismisses the menu): the new surface
  opens at the **same anchor where the click happened** — right at / just below the
  clicked item, so the eye tracks from click → surface.
- **Never** the current Layers/Pens bug: opened from Tools, they appear *backward /
  left of* the dropdown with dead space — wrong. They should open at the click anchor
  (menu side, upward), like the flyout.
- **Bring to front (z-order):** the launched surface is **raised above** the menu and any
  existing panels the moment it opens. It must **never** appear *behind* the dropdown or
  another window — "opens backward the menu" is a bug even when the position is right.
- **Remembered geometry wins:** once the user moves a floating panel, its saved position
  is used (WORKSPACE_SYSTEM §5). This rule governs the **first open / no-remembered-pos**
  case. (Bring-to-front applies **every** open, remembered position or not.)
- **Applies to ALL menu-launched panels — no subset.** Route **every** panel toggled from
  a menu through the same anchor + raise choke point: Layers, Pens, DObjects list,
  Inspector, Snap, Session Recorder, Command line, … A panel left on its own ad-hoc
  positioning (opening left / behind) is the inconsistency this rule exists to remove.

The flyout's placement is the **gold standard** for every menu-launched surface.
