# INSPECTOR DESIGN — MENTOR (revised template spec)

The **Inspector** panel, revised. This doc **sits alongside** the original
`INSPECTOR_DESIGN.md` and is the **current authority** for the Inspector; where
the two differ, THIS doc wins. `INSPECTOR_DESIGN.md` stays as historical
reference and should get a one-line pointer at its top: *"Superseded by
INSPECTOR_DESIGN_MENTOR.md."* Place this file at the repo root next to it.

It is a **template**: it defines the reusable panel skeleton (shell, header, type
pill, GENERAL + GEOMETRY, and every field renderer). The **type-specific variable
sections** (per dobject: Line, Circle, Arc, Hatch, Text, Wall, Dim, Block,
Polyline, Point, …) are defined **later, one dobject type at a time**, and slot
into the reserved place in §6.

Parent tokens live in `THEME_SYSTEM.md` §5. **Rule: no raw numbers in code —
every value below is a named token in `theme.rs`.**

---

## 0. Deltas — what changes vs today (apply in-code, docs in the same commit)

| Change | Old | New | Apply in |
|---|---|---|---|
| Shared header band height (ALL docked panels: Inspector, command bar, rails) | 40 | **32** | `dock.rs` `header_band` |
| Type pill location | inside header (right) | **centered row below the header** | `dock.rs` (drop Inspector `badge`); paint pill in `inspector_body` |
| Start↔End / X↔Y column gap (token) | 12 | **8** | `theme::space::COLUMN_GAP`; THEME_SYSTEM §5.1 |
| Inspector min width | 220 | **264** | Inspector `DockConfig.min` |
| `INSPECTOR_DESIGN.md` | authority | **superseded** | add pointer to this file at its top |

The rest of this doc is the full target (some already correct in code, some are
the open gap-fixes: linetype order, lineweight bar, Visible checkbox, coordinate-
field styling, section dividers).

---

## 1. Tokens used (exact)

- **Spacing:** box/control/icon height **24** · row-gap **8** (pitch 32) · label→input **8** ·
  input-pad **8** · section→content **12** · group-gap **12** · **column-gap 8** ·
  panel-edge **16** · header→pill **12** · pill→first section **12**.
- **Radius:** swatch **2** · fields/boxes/dropdowns **4** · buttons **8** · panel/menus **12** · pill **full**.
- **Color:** field fill `surface-0 #141C25` · panel `surface-1 #1A2430` · hover `surface-2` ·
  popover `surface-3` · header `chrome #223040` · border `#34414B` · accent `#00E5FF` ·
  on-accent `#063B45` · text-primary `#DAE3EF` · muted `#93A1AC` · **column-header dim `#66707A`** · danger `#E5484D`.
- **Type:** title Geist 16/500 · body Geist 13/400 · caption Geist 11/500 ·
  hint Geist 11/400 · data-value Mono 12/400 · data-code Mono 11/400. Cap 16px.

---

## 2. Panel shell

```
┌───────────────────────────────┐  card: surface-1, border 1px, radius 12
│ Inspector                   × │  header band: chrome, height 32, bottom hairline
├───────────────────────────────┤
│ (        Line          )      │  type pill: FULL-content-width capsule, text centered
│                               │  12 above (to header) · 12 below (to GENERAL)
│ ▼ GENERAL                     │  (NO divider between pill and GENERAL)
│ …                             │
└───────────────────────────────┘
```

- **Card:** fill `surface-1`, 1px `border`, radius 12. **Min width 264** (see §7).
  Side padding = panel-edge **16**.
- **Header band (SHARED across all docked panels):** fill `chrome`, height **32**, 1px bottom hairline.
  - **Title** — title (Geist 16/500), `text-primary`, at panel-edge (16) from left, vertically centered.
  - **Close ×** — right, ~15px glyph, `text-muted`, brightens on hover. (Already in `header_band`; keep.)
  - No type pill here anymore.
- **Type pill row (below the header):**
  - **12px** from the header bottom to the pill; **12px** from the pill to the first section.
  - Pill = **full**-radius capsule spanning the **full content width** (edge-to-edge inside the 16px
    padding; grows with the panel). The type text sits **centered** inside it.
    Fill = accent @ ~10% (`rgba(0,229,255,.10)`), text = `accent`, caption (11/500), height **18**.
  - Content = dobject type + count: `Line`, `Line (3)`, `Mixed (5)`.
  - **No hairline divider** between the pill row and GENERAL.

---

## 3. Sections

Order: **GENERAL**, **GEOMETRY**, **[type-specific — §6]**, **MISC**. All collapsible.

- **Header row:** height 18. Solid triangle chevron (`▼` open / `▶` collapsed), `text-muted`,
  at the content-column left edge; **8px** to the caption. Caption = caption (Geist 11/500),
  `text-muted`, UPPERCASE.
- **section→content = 12**, **group-gap = 12**.
- **1px hairline `border` divider above each section header EXCEPT the first** (GENERAL none;
  GEOMETRY, type-specific, MISC each get one). *(This is the "no separation line" fix.)*
- Collapsed section shows only its header row.

---

## 4. Rows (label-left)

Every property is one row: height **24**, separated by row-gap **8** → pitch **32**.

- **Label column:** fixed width **84** (unified value start). Text = body (Geist 13/400),
  `text-muted`, left-aligned, vertically centered.
- **label→input = 8**.
- **Value field:** fills the remaining width (stretches on resize — §7). Height 24,
  fill `surface-0`, 1px `border`, radius **4**, inner pad **8**.
- **Value text:** names = body (Geist 13/400) `text-primary`; numbers = data-value (Mono 12/400),
  left-aligned to the unified start.
- **Dropdown arrow:** a **solid** filled down-triangle, `text-muted`, ~8px from the field's right edge.
  Keep solid everywhere, including coordinate fields.

---

## 5. Specialized value renderers

| Field | Renders as |
|---|---|
| **Layer** | color **swatch** (13×13, r2) + **9px** + name (Geist 13) + solid ▼. Swatch = layer's resolved color. |
| **Color** | swatch (13×13, r2, 1px border) + 9px + name (`By Layer` / `ACI n` / `RGB #…`) + solid ▼. |
| **Line Type** | **dash preview FIRST** (left, length **L**, 1.5px, `text-primary`) + 9px + **abbreviated name** + solid ▼. |
| **Line Weight** | **thickness bar FIRST** (left, length **L** — SAME as linetype, height = weight, cap ~4px, `text-primary`) + 9px + value (`0.25 mm`, Mono 12) + solid ▼. |
| **Lt Scale / number** | plain field, value = Mono 12, left-aligned. |
| **Visible / bool** | **checkbox**: 16×16, radius 4, 1px `border`; ON → fill `accent`, check glyph `on-accent`. |
| **Coordinate pair** | two fields under `Start`/`End` (or `X`/`Y`) headers; **column-gap 8**; each field styled EXACTLY as §4 (surface-0, border, radius 4, Mono 12) — **not** the egui default look. |
| **Derived (Length, Angle, Area)** | **read-only**: NO box, NO border; value = Mono 12, `text-muted`, at the value start. |
| **Mixed (multi-value differs)** | field shows `Mixed` in `text-muted` italic; editing writes to all. |

**Line Type / Line Weight specifics:**
- **Matched preview length `L`:** the linetype dash line and the lineweight bar are the **same length**
  and **stretch together** with the field, so they stay matched at any width.
- **Truncated linetype name:** first **10 letters** of the name (most names fit fully, e.g. `Continuous`);
  if the linetype has a size variant, append its first letter in parentheses — e.g. `Divide (s)`; names
  longer than 10 truncate. **Show the full name on hover (tooltip).** *(Revised from 3→10 letters, 2026-07-02,
  to match the shipped build — 3 was too cryptic and the field has the room.)*

### 5.1 Coordinate block (GEOMETRY)
```
            Start           End        ← column headers: dim (#66707A), 11/400 REGULAR,
X        [ 1250.00 ]     [ 1310.00 ]      2px above the first row, aligned to each field's left
Y        [  840.50 ]     [  905.00 ]
Length      88.60                       ← derived: muted mono, no box
Angle       47.30°
```
- **Start / End (or X / Y) headers are lighter:** dim color `#66707A`, **regular weight (11/400)**.
- **column-gap = 8** between the two fields; each field = `(value_area − 8) / 2` at min width, both stretch.

---

## 6. Type-specific variables  *(template placeholder — defined later, per type)*

Between GEOMETRY and MISC sits the **type-specific section** for the selected dobject. It follows
the SAME section + row + renderer rules above. Its *content* (which variables, in what order) is
defined **per dobject type, later, one at a time**. Where each will land:

- **Hatch** → pattern + swatch, scale, angle.
- **Text** → style, height, angle, alignment, content.
- **Wall** → style, thickness.
- **Dimension** → dim style.
- **Block** → block name (read-only), scale, rotation.
- **Circle / Arc / Ellipse / Polyline / Point / Spline** → their own GEOMETRY variables.

Until a type's section is specified, show only GENERAL + GEOMETRY + MISC. **Do not invent
type fields** — each is a separate mentor decision.

---

## 7. Responsive

- **Minimum width 264.** The panel may be widened (docked resize / float).
- On widen: the **label column stays 84**; the **value boxes and the preview bars stretch**.
  The linetype dash and lineweight bar stretch together and stay matched.
- No other responsive behavior — just min-width + natural horizontal stretch.

---

## 8. States

- **Hover** (interactive field): border → `accent`, fill unchanged; pointer cursor for dropdowns.
- **Focus** (keyboard): 2px `accent` ring, 2px offset (keyboard focus only).
- **Disabled:** `text-disabled`, desaturated border, no hover.
- **Error:** field border `danger` + a `danger` caption message below (adds one row-gap above the next row).
- **Read-only** (derived): muted, no border, not focusable.

---

## 9. Do / Don't

- **Do** left-align every value to the 84 start; **Do** use Mono for numbers, Geist for names.
- **Do** render derived values borderless + muted; **Do** put a hairline divider above every section but the first.
- **Do** keep dropdown arrows solid; **Do** match the linetype/lineweight preview lengths.
- **Don't** put the dobject type as a field or in the header — it lives in the **centered full-width pill**.
- **Don't** use square field corners (radius 4) or the egui-default look on coordinate fields.
- **Don't** hardcode spacing/sizes — every value is a token from §1.
- **Don't** add type-specific variables until that dobject type's section is specified (§6).
