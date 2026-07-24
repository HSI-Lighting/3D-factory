# HEADER STANDARD — one foundation, a few variants (mentor spec)

Every titled surface — docked panels (Inspector, command bar, rails), the
**command palette**, and every **dialog / popover** — draws its header from **one
shared foundation**. But surfaces differ (a dialog may want a help icon, a panel a
collapse control), so the foundation is **composable** and has a **small, closed
set of variants** — not one rigid header forced on everything.

Goal: **change the foundation once → every header updates**, while each surface
still gets the affordances it needs.

Parent tokens live in `THEME_SYSTEM.md` §5. **No raw numbers — every value is a
named token in `theme.rs`.**

---

## 1. Foundation (the single source)

One `header_band` widget. Layout is three zones, left→right:

```
┌──────────────────────────────────────────────┐
│ Title …………………………  [action slot]  × │   height 32, chrome, bottom hairline
└──────────────────────────────────────────────┘
```

| Property | Value (token) |
|---|---|
| Height | **32** (`space::HEADER_H`) — same on every surface |
| Fill | `chrome #223040` |
| Bottom edge | 1px `border #34414B` hairline |
| Title | Geist **16 / 500**, `text-primary #DAE3EF`, at **panel-edge 16** from left, vertically centered |
| Action slot | right side, **before** the ×; **normally empty**; holds only muted icon-buttons (see §2) |
| Close × | far right, ~15px, `text-muted #93A1AC` → `text-primary` on hover; 16 from right edge |
| Casing | sentence case ("Command palette") |

**Only these three zones live in the band.** Type pills, tabs, search, subtitles do
**not** — they sit *below* the header (Inspector type pill, palette search field).

**Single source:** exactly one function paints the band; surfaces pass `title`,
optional `actions`, optional `on_close`. No surface hardcodes 32 / chrome / the
title type. Change any of those once → all headers move in lockstep.

---

## 2. Action slot (the escape hatch that prevents future forks)

A constrained, optional zone so new needs don't force a header rewrite:

- Holds **icon-buttons only** — ~15px, `text-muted`, hover-brighten (same as ×).
- Examples it must absorb later without a redesign: help `?`, pin, collapse,
  dock/undock, "reset panel".
- **Never** text buttons, tabs, or inputs. If a surface needs more than a couple of
  icons, that's a body-region toolbar, not the header.

---

## 3. Variants (a closed set — do not grow without a mentor decision)

Both compose §1; they differ only in behavior/affordances, not in look.

| Variant | Surfaces | Adds |
|---|---|---|
| **Panel** | Inspector, command bar, Draw/Modify rails (docked) | collapse / dock controls in the action slot; no × when the panel isn't dismissable |
| **Floating** | command palette, all dialogs (Hatch, Block, Insert Block, DWG, raster, parametric) | close × always present; help `?` in the slot where useful |

If a surface genuinely fits neither, that's a **new mentor decision** — don't invent
a third header ad hoc.

---

## 4. Applies to (audit list)

- Inspector — conforms (`INSPECTOR_DESIGN_MENTOR.md`). → Panel variant.
- Command bar, Draw/Modify rails — docked, share the band. → Panel variant.
- **Command palette** — adopt the band. → Floating variant.
- **All merged dialogs** (Hatch, Block, Insert Block, DWG import/export, raster,
  parametric) — do **not** yet conform (arrived with the dokkandar merge). → Floating
  variant. Part of the Menu & Palette / merged-UI conformance pass.

---

## 5. Exit criteria

- One `header_band` function; grep finds **no** other title-row painter.
- Palette + every dialog show the identical 32px chrome band, Geist 16/500 title,
  hover-brightening ×.
- The action slot renders (empty by default) and accepts an icon-button without any
  change to the band itself.
- Changing `HEADER_H` (or chrome / title token) in one place visibly moves **every**
  surface's header — Panel and Floating alike.
