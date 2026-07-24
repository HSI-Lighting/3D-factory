//! Design tokens — the single source of every visual value in the app.
//!
//! This is the code side of `THEME_SYSTEM.md` §5 (the locked token registry).
//! **Components must read these tokens, never hard-code colours or sizes.**
//! When a value needs to change, change it *here* and it propagates app-wide.
//!
//! Dark theme only (teal-navy surfaces, cyan accent). A future `ThemeStore`
//! (THEME_SYSTEM §2) will make these runtime-editable; for now they are `const`.

use egui::Color32;

/// Colour tokens. See THEME_SYSTEM §5.3–5.6.
pub mod color {
    use super::Color32;

    // ── Surfaces (tonal elevation ladder) ──────────────────────────────
    pub const SURFACE_0: Color32 = Color32::from_rgb(0x14, 0x1c, 0x25); // canvas / page
    pub const SURFACE_1: Color32 = Color32::from_rgb(0x1a, 0x24, 0x30); // panel
    pub const SURFACE_2: Color32 = Color32::from_rgb(0x22, 0x2b, 0x34); // raised control
    pub const SURFACE_3: Color32 = Color32::from_rgb(0x2a, 0x37, 0x44); // popover / menu
    pub const CHROME:    Color32 = Color32::from_rgb(0x22, 0x30, 0x40); // header / footer band
    pub const FIELD:     Color32 = Color32::from_rgb(0x14, 0x1c, 0x25); // input fill (= surface-0)
    pub const BORDER:    Color32 = Color32::from_rgb(0x34, 0x41, 0x4b);

    // ── Accent ─────────────────────────────────────────────────────────
    pub const ACCENT:    Color32 = Color32::from_rgb(0x00, 0xe5, 0xff);
    pub const ON_ACCENT: Color32 = Color32::from_rgb(0x06, 0x3b, 0x45); // text on the cyan fill

    // ── Text ───────────────────────────────────────────────────────────
    pub const TEXT_PRIMARY:   Color32 = Color32::from_rgb(0xda, 0xe3, 0xef);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xae, 0xb9, 0xc4);
    pub const TEXT_MUTED:     Color32 = Color32::from_rgb(0x93, 0xa1, 0xac); // labels, placeholders
    pub const TEXT_DISABLED:  Color32 = Color32::from_rgb(0x5c, 0x69, 0x75);
    /// Coordinate column headers (Start/End, X/Y) — dimmer than muted, 11/400.
    /// INSPECTOR_DESIGN_MENTOR §1/§5.1.
    pub const COLUMN_HEADER:  Color32 = Color32::from_rgb(0x66, 0x70, 0x7a);

    // ── Semantic (no `info` — cyan carries it) ─────────────────────────
    pub const SUCCESS: Color32 = Color32::from_rgb(0x34, 0xd3, 0x99);
    pub const WARNING: Color32 = Color32::from_rgb(0xf2, 0xb5, 0x3d);
    pub const DANGER:  Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);

    // ── State overlays (apply over any surface) ────────────────────────
    /// Hover lift — white at ~6%.
    pub const HOVER: Color32 = Color32::from_rgba_premultiplied(15, 15, 15, 15);
    /// Hover on cyan-highlight elements — accent at ~12%.
    pub const HOVER_ACCENT: Color32 = Color32::from_rgba_premultiplied(0, 27, 30, 31);
    /// Focus ring colour (2px, 2px offset — see the a11y rules).
    pub const FOCUS: Color32 = ACCENT;
}

/// Spacing scale (4px base). See THEME_SYSTEM §5.1.
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS:  f32 = 4.0;
    pub const SM:  f32 = 8.0;
    pub const MD:  f32 = 12.0;
    pub const LG:  f32 = 16.0;
    pub const XL:  f32 = 24.0;
    pub const XXL: f32 = 32.0;

    /// Uniform control / field / row height.
    pub const CONTROL_H: f32 = 24.0;
    /// Vertical gap between property rows (pitch = CONTROL_H + ROW_GAP = 32).
    pub const ROW_GAP: f32 = 8.0;
    /// Icon hit-box.
    pub const ICON_BOX: f32 = 24.0;
    /// Shared docked-panel header band height (Inspector, command bar, rails).
    /// INSPECTOR_DESIGN_MENTOR §0/§2.
    pub const HEADER_BAND: f32 = 32.0;

    // ── Relationship spacings (THEME_SYSTEM §5.1, finalized compact scale) ──
    /// Horizontal label → input gap, and the inside-field edge → text padding.
    pub const LABEL_INPUT: f32 = 8.0;
    pub const INPUT_PAD:   f32 = 8.0;
    /// Section header → its first content row.
    pub const SECTION_GAP: f32 = 12.0;
    /// Gap between property groups / sections.
    pub const GROUP_GAP:   f32 = 12.0;
    /// Start ↔ End (coordinate) column gap (INSPECTOR_DESIGN_MENTOR §5.1).
    pub const COLUMN_GAP:  f32 = 8.0;
    /// Panel inner edge padding.
    pub const PANEL_EDGE:  f32 = 16.0;
    /// Panel header band → first content.
    pub const PANEL_HEADER: f32 = 24.0;

    // ── Type-pill row (INSPECTOR_DESIGN_MENTOR §2) ──────────────────────
    /// Type-pill capsule height.
    pub const PILL_H: f32 = 18.0;
    /// Header bottom → type pill.
    pub const HEADER_TO_PILL: f32 = 12.0;
    /// Type pill → first section (GENERAL).
    pub const PILL_TO_SECTION: f32 = 12.0;
}

/// Corner-radius scale. See THEME_SYSTEM §5.2.
pub mod radius {
    pub const XS:   f32 = 2.0;  // swatches, micro chips
    pub const SM:   f32 = 4.0;  // inputs, value boxes
    pub const MD:   f32 = 8.0;  // buttons, icon buttons, dropdowns
    pub const LG:   f32 = 12.0; // cards, panels, menus
    pub const FULL: f32 = 9999.0; // pills, toggles
}

/// Install the brand fonts (THEME_SYSTEM §5.7): **Geist** for UI text and
/// **JetBrains Mono** for data/numbers. All `.ttf`s are embedded in the binary
/// (`include_bytes!`) so they always ship with the exe regardless of the working
/// directory. Call ONCE at startup, before the first frame.
///
/// egui's `FontId` selects a family + size only — it has **no weight axis** and
/// no synthetic bold. So the spec's weight-500 roles (title, body-strong,
/// caption) cannot come from the Regular face; Geist **Medium** is registered as
/// its own named family `GeistMedium` and routed to via the [`typ`] tokens. The
/// weight-400 UI face (Geist Regular) sits at the FRONT of `Proportional` and
/// JetBrains Mono at the FRONT of `Monospace`, so all existing default-family
/// text re-points at once. egui's bundled fonts remain behind ours as glyph
/// fallbacks. Monospace gets no Medium — both mono styles are weight 400 (§5.7).
pub fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;

    const GEIST: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/Geist-Regular.ttf"));
    const GEIST_MEDIUM: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/Geist-Medium.ttf"));
    const JETBRAINS_MONO: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/fonts/JetBrainsMono-Regular.ttf"
    ));

    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("Geist".to_owned(), Arc::new(FontData::from_static(GEIST)));
    fonts.font_data.insert(
        "GeistMedium".to_owned(),
        Arc::new(FontData::from_static(GEIST_MEDIUM)),
    );
    fonts.font_data.insert(
        "JetBrainsMono".to_owned(),
        Arc::new(FontData::from_static(JETBRAINS_MONO)),
    );

    // Front of the family = the default face; keep egui's stock fonts after ours
    // so any glyph the brand faces lack still renders.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Geist".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "JetBrainsMono".to_owned());

    // Medium is a *separate named family* (egui has no weight axis). Its fallback
    // chain = GeistMedium, then whatever Proportional resolves to (Geist Regular +
    // egui defaults), so missing glyphs still render.
    let mut medium_chain = vec!["GeistMedium".to_owned()];
    medium_chain.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts
        .families
        .insert(FontFamily::Name("GeistMedium".into()), medium_chain);

    ctx.set_fonts(fonts);
}

/// Type scale — the §5.7 text roles as tokens. Each returns the egui
/// [`egui::FontId`] (size + family) for that role; components call these instead
/// of constructing `FontId`s or passing raw sizes inline (§1). Weight-500 roles
/// resolve to the `GeistMedium` family registered in [`install_fonts`]; weight-400
/// UI text uses `Proportional` (Geist Regular); all data text uses `Monospace`
/// (JetBrains Mono). Nothing exceeds the 16px cap (§5.7).
pub mod typ {
    use egui::{FontFamily, FontId};

    /// Geist Medium — the weight-500 UI family (see [`super::install_fonts`]).
    fn medium() -> FontFamily {
        FontFamily::Name("GeistMedium".into())
    }

    /// Panel / section titles — Geist Medium **16/500**.
    pub fn title() -> FontId {
        FontId::new(16.0, medium())
    }
    /// Plain field labels + general body text — Geist Regular **13/400**.
    pub fn body() -> FontId {
        FontId::new(13.0, FontFamily::Proportional)
    }
    /// Emphasis body — Geist Medium **13/500**.
    pub fn body_strong() -> FontId {
        FontId::new(13.0, medium())
    }
    /// Section headers, column headers, units — Geist Medium **11/500**.
    pub fn caption() -> FontId {
        FontId::new(11.0, medium())
    }
    /// Secondary / subtitle 11px text (e.g. settings subtitles) — Geist Regular
    /// **11/400**. The lighter 11px counterpart to [`caption`] (11/500); use this
    /// when 11px text is *not* a heading.
    pub fn hint() -> FontId {
        FontId::new(11.0, FontFamily::Proportional)
    }
    /// Numbers, coordinates, Length / Angle — JetBrains Mono **12/400**.
    pub fn data_value() -> FontId {
        FontId::new(12.0, FontFamily::Monospace)
    }
    /// Handles, small badges — JetBrains Mono **11/400**.
    pub fn data_code() -> FontId {
        FontId::new(11.0, FontFamily::Monospace)
    }
}

/// Install the token palette as egui's GLOBAL `Visuals`, so every default-styled
/// widget — menus, dialogs, buttons, checkboxes, dropdowns, text fields — reads
/// the one teal-navy theme instead of egui's stock grey. Panels/canvas that set
/// their own frame fill are unaffected. Call once per frame (cheap; idempotent).
pub fn apply(ctx: &egui::Context) {
    use color as c;
    use egui::{Rounding, Stroke};
    let mut v = egui::Visuals::dark();

    // egui shares one `window_fill` between dialog windows AND menus/popups
    // (Frame::window / Frame::menu / Frame::popup all read it — there is no
    // menu-specific fill in Visuals 0.30). Per THEME_SYSTEM §5.3/§5.10 menus are
    // the popover tone (surface-3); §5.9 treats dialogs as overlay layers too, so
    // both share surface-3 and lift off the surface-1 panels behind them.
    v.window_fill = c::SURFACE_3;
    v.window_stroke = Stroke::new(1.0, c::BORDER);
    v.window_rounding = Rounding::same(radius::LG);
    v.menu_rounding = Rounding::ZERO;                 // square menus (kept)
    v.extreme_bg_color = c::SURFACE_0;                // text-field / scroll bg
    v.faint_bg_color = c::SURFACE_2;
    v.hyperlink_color = c::ACCENT;
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0x00, 0xe5, 0xff, 60);
    v.selection.stroke = Stroke::new(1.0, c::ACCENT);

    // Widget state ladder.
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, c::BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, c::TEXT_PRIMARY);
    w.inactive.bg_fill = c::SURFACE_2;
    w.inactive.weak_bg_fill = c::SURFACE_2;
    w.inactive.bg_stroke = Stroke::new(1.0, c::BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, c::TEXT_PRIMARY);
    w.inactive.rounding = Rounding::same(radius::MD);
    w.hovered.bg_fill = c::SURFACE_3;
    w.hovered.weak_bg_fill = c::SURFACE_3;
    w.hovered.bg_stroke = Stroke::new(1.0, c::ACCENT);
    w.hovered.fg_stroke = Stroke::new(1.0, c::TEXT_PRIMARY);
    w.hovered.rounding = Rounding::same(radius::MD);
    w.active.bg_fill = c::SURFACE_3;
    w.active.weak_bg_fill = c::SURFACE_3;
    w.active.bg_stroke = Stroke::new(1.0, c::ACCENT);
    w.active.fg_stroke = Stroke::new(1.0, c::TEXT_PRIMARY);
    w.active.rounding = Rounding::same(radius::MD);
    w.open.bg_fill = c::SURFACE_2;
    w.open.weak_bg_fill = c::SURFACE_2;
    w.open.bg_stroke = Stroke::new(1.0, c::BORDER);

    ctx.set_visuals(v);
}
