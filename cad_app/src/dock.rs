//! Unified docking abstraction — see WORKSPACE_SYSTEM.md.
//!
//! The application must **never depend on a specific docking engine**. Every
//! dockable panel (Inspector, command bar, future tool panels) is rendered
//! through [`DockHost`]. [`EguiDockHost`] is the hand-rolled egui implementation
//! used today; to replace the engine (e.g. with `egui_dock`), add another
//! `impl DockHost` and point [`HOST`] at it — the call sites don't change.
//!
//! A panel calls [`DockHost::show`] with its content closure and a `&mut
//! DockState`. The host draws the chrome header (title · close · drag-to-undock),
//! the frame, and handles docked↔floating transitions. Behaviour is therefore
//! identical for every panel.

use egui::{Align2, Color32, Context, CursorIcon, FontId, Id, Pos2, Rect, Sense,
           Stroke, Ui, Vec2};

/// Which edge a panel is docked against.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DockRegion { Left, Right, Bottom, Top }

/// Whether a panel is docked (to a region) or floating (at a screen position).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DockState { Docked(DockRegion), Floating(Pos2) }

/// Per-panel docking configuration.
pub struct DockConfig<'a> {
    pub id: &'a str,
    pub title: &'a str,
    /// Optional accent chip shown at the right of the header (before the ×) —
    /// the Inspector uses it for the selected dobject's type ("Line"), per the
    /// design (type shows in the header, not as a field).
    pub badge: Option<&'a str>,
    /// The ONE edge this panel may dock to. A panel never docks anywhere else,
    /// so dragging it toward any other edge just leaves it floating (the command
    /// bar docks only Bottom, the rails only Left, the Inspector only Right).
    pub dock_region: DockRegion,
    /// An optional SECOND edge this panel may dock to (INSPECTOR_DESIGN §7 — the
    /// Inspector docks Right *or* Left). `None` = single-edge (the historical
    /// behaviour). The stored `DockState::Docked(region)` picks between them.
    pub alt_region: Option<DockRegion>,
    /// When TRUE the panel may dock to ANY edge (the toolbars): the header gets a
    /// right-click dock menu (Left / Right / Top / Bottom / Float) and drag-to-dock
    /// accepts all four edges. `dock_region` / `alt_region` are ignored while set,
    /// and a stored `Docked(region)` is honoured as-is rather than re-homed to the
    /// one allowed edge.
    pub any_edge: bool,
    /// Height of the panel when docked to the TOP edge — the strip thickness.
    /// (`size` is the width for L/R; a Bottom dock already reuses `size` as its
    /// height.) Meaningless for panels that cannot dock Top.
    pub strip_h: f32,
    /// When FALSE the panel is FLOATING-ONLY: it can never dock, and a stale
    /// `Docked` state is migrated to `Floating` on show. Only the Inspector is
    /// dockable (owner: the right column must not grab other dialogs).
    pub dockable: bool,
    /// Render the header in the **rail style** (INSPECTOR_DESIGN §Header band):
    /// `#34414B` band, no hairline (soft shadow instead), title in the menu-bar
    /// CATEGORY font, right-end grip → right-click → "Close" flyout, plus a
    /// visible × close button just left of the grip. When
    /// false the shared chrome `header_band` (title + ×) is used.
    pub rail_header: bool,
    /// When true, the rail header shows a **collapse chevron** at the left that
    /// minimises the panel to just its header band (click again to restore). The
    /// collapsed state persists in ctx memory keyed by `id`.
    pub collapsible: bool,
    /// Docked size on the variable axis (width for L/R, height for Bottom).
    pub size: f32,
    pub min: f32,
    pub max: f32,
    pub resizable: bool,
    /// When true the body is rendered EDGE-TO-EDGE (no inset frame) so the panel
    /// can paint full-width, flush chrome of its own (the rails' bottom footer
    /// band). Content panels leave this false and get the standard inset.
    pub flush_body: bool,
    /// Content width when floating. For an L/R panel this usually equals
    /// `size`; a Bottom-docking panel (whose `size` is a height) floats wider.
    pub float_w: f32,
    /// Floating height cap as a fraction of the screen (e.g. 0.5).
    pub float_max_h_frac: f32,
}

/// The replaceable docking-engine boundary. Swap the implementation, not the
/// call sites.
pub trait DockHost {
    /// Render one dockable panel. `body(ui, scroll_cap)` fills the content;
    /// `scroll_cap` is `Some` when floating so the panel can cap its scroll area
    /// at ~`float_max_h_frac` of the screen. Mutates `state`/`open` for
    /// dock/undock/close. Returns the panel's outer rect.
    fn show(&self, ctx: &Context, cfg: &DockConfig, state: &mut DockState,
            open: &mut bool, body: impl FnOnce(&mut Ui, Option<f32>)) -> Rect;
}

// ── palette — all design tokens (THEME_SYSTEM §5); no raw hex here ──────────
const BG:     Color32 = crate::theme::color::SURFACE_1;     // panel surface
fn border() -> Color32 { crate::theme::color::BORDER }
fn chrome() -> Color32 { crate::theme::color::CHROME }
const TEXT:  Color32 = crate::theme::color::TEXT_PRIMARY;
const MUTED: Color32 = crate::theme::color::TEXT_MUTED;

/// The ONE chrome header used by every docked/floating bar — unified per the
/// design system (title Geist 16/500 on `surface-chrome`, THEME_SYSTEM §5.7/§5.3;
/// × close at the right). The whole band is the drag handle; a *click* over the
/// × closes instead of dragging. `cfg.title` may be empty (the icon rails carry
/// no name) — then only the × and the drag band show. Returns
/// `(close_clicked, band_response)`; callers derive undock/drag from the band.
/// Result of the shared header foundation: whether the × was clicked, the band
/// Response (for drag), and the (usually empty) action-slot rect.
pub(crate) struct HeaderBand {
    pub close_clicked: bool,
    pub band: egui::Response,
    pub action_slot: Rect,
}

/// The ONE shared header foundation (HEADER_STANDARD_MENTOR §1). A 32px chrome
/// band with a bottom hairline and three zones left→right: sentence-case Title at
/// panel-edge (Geist 16/500), an optional right-side **action slot** (icon-buttons
/// only, before the ×), and an optional close ×. EVERY titled surface draws from
/// here — docked panels (Panel variant) and the floating palette/dialogs (Floating
/// variant) — so changing 32 / chrome / the title token in this one place moves
/// every header in lockstep.
///
/// `badge` is a legacy in-band pill kept for the command bar / rails until they
/// take their own HEADER_STANDARD pass (the spec moves pills BELOW the band; the
/// Inspector already did — its pill lives in `inspector_body`). The returned
/// `action_slot` (empty by default) is where a surface paints its icon-buttons
/// (help `?`, pin, collapse) with NO change to the band itself (§2).
pub(crate) fn header_band(ui: &mut Ui, title: &str, badge: Option<&str>,
                          closable: bool) -> HeaderBand {
    let edge = crate::theme::space::PANEL_EDGE;
    let w = ui.available_width();
    // ONE widget senses the whole band (click + drag) — a separate hover widget
    // over the same rect made the two fight for the pointer and swallowed the
    // docked undock drag (close worked, undock didn't).
    let (rect, band) = ui.allocate_exact_size(
        egui::vec2(w, crate::theme::space::HEADER_BAND), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, chrome());
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, border()));
    if !title.is_empty() {
        p.text(egui::pos2(rect.left() + edge, rect.center().y), Align2::LEFT_CENTER,
            title, crate::theme::typ::title(), TEXT);
    }
    // Place from the right: 12px inset, then × (if closable), then the legacy pill.
    let mut right_x = rect.right() - 12.0;
    let mut close_clicked = false;
    if closable {
        // × close hit-box (far right). A click on it closes; the rest drags.
        let xr = Rect::from_center_size(
            egui::pos2(right_x - 10.0, rect.center().y), egui::vec2(20.0, 20.0));
        let over_x = ui.rect_contains_pointer(xr);
        if band.hovered() {
            ui.ctx().set_cursor_icon(
                if over_x { CursorIcon::PointingHand } else { CursorIcon::Grab });
        }
        let xcol = if over_x { TEXT } else { MUTED };
        let c = xr.center(); let s = 5.0;
        let st = Stroke::new(1.5, xcol);
        p.line_segment([egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)], st);
        p.line_segment([egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y - s)], st);
        close_clicked = band.clicked() && over_x;
        right_x = xr.left() - 8.0;
    } else if band.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    if let Some(b) = badge {
        let accent = crate::theme::color::ACCENT;
        let font = crate::theme::typ::caption();
        let tw = ui.fonts(|f| f.layout_no_wrap(b.to_owned(), font.clone(), accent)).size().x;
        let pw = tw + 16.0;
        let pill = Rect::from_min_size(
            egui::pos2(right_x - pw, rect.center().y - 9.0), egui::vec2(pw, 18.0));
        p.rect(pill, egui::Rounding::same(9.0),
            Color32::from_rgba_unmultiplied(0x00, 0xe5, 0xff, 26), Stroke::NONE);
        p.text(pill.center(), Align2::CENTER_CENTER, b, font, accent);
        right_x = pill.left() - 8.0;
    }
    // Action slot: title-end → right_x. Empty by default; icon-buttons go here.
    let action_slot = Rect::from_min_max(
        egui::pos2(rect.left() + edge, rect.top()), egui::pos2(right_x, rect.bottom()));
    HeaderBand { close_clicked, band, action_slot }
}

/// Rail-style header (INSPECTOR_DESIGN §Header band) — the Inspector's header,
/// styled like the tool-rail header instead of the chrome `header_band`:
/// `#34414B` band at the rail header height, NO bottom hairline (depth comes
/// from a soft drop shadow onto the body), the title at panel-edge in the
/// menu-bar CATEGORY font (egui `TextStyle::Button` — the File/Edit/Draw… size),
/// a right-end grip whose **right-click opens a single-row "Close" flyout**,
/// and a visible × close button left of the grip. The whole band is the drag
/// handle. Returns `(close_clicked, band)`.
fn rail_header_band(ui: &mut Ui, cfg: &DockConfig) -> (bool, egui::Response) {
    const RAIL_HEADER_H: f32 = 30.0;   // = app SPECS_TRAY_H (rail header height)
    let edge = crate::theme::space::PANEL_EDGE;
    let hdr_fill = crate::theme::color::BORDER;    // #34414B (rail-header tone)
    let grip_col = crate::theme::color::SURFACE_1; // #1A2430
    let w = ui.available_width();
    let (rect, band) = ui.allocate_exact_size(
        egui::vec2(w, RAIL_HEADER_H), Sense::click_and_drag());
    // Paint into a rect a few px taller so the drop-shadow bands below the band
    // aren't clipped by the header's own paint region.
    let p = ui.painter_at(rect.expand2(egui::vec2(0.0, 8.0)));
    p.rect_filled(rect, 0.0, hdr_fill);            // §Header: NO hairline
    // Soft drop shadow onto the body — mirrors the tool-rail header (DRAW_RAIL §2).
    for i in 0..5 {
        let a = (66.0 * (1.0 - i as f32 / 5.0)).max(8.0) as u8;
        p.rect_filled(Rect::from_min_max(
            egui::pos2(rect.left(),  rect.bottom() + i as f32),
            egui::pos2(rect.right(), rect.bottom() + i as f32 + 1.0)),
            egui::Rounding::ZERO, Color32::from_black_alpha(a));
    }
    // Collapse chevron (left) — minimises the panel to its header (▾ expanded /
    // ▸ collapsed). Toggles a `(id,"collapsed")` bool in ctx memory; the dock
    // host reads it to skip the body. Shifts the title right when present.
    let mut title_x = rect.left() + edge;
    let mut collapse_hovered = false;
    if cfg.collapsible {
        let cid = Id::new((cfg.id, "collapsed"));
        let collapsed = ui.ctx().data(|d| d.get_temp::<bool>(cid).unwrap_or(false));
        let cc = egui::pos2(rect.left() + 11.0, rect.center().y);
        let s = 4.0_f32;
        let tri = if collapsed {
            vec![egui::pos2(cc.x - s*0.5, cc.y - s), egui::pos2(cc.x - s*0.5, cc.y + s),
                 egui::pos2(cc.x + s*0.7, cc.y)]                          // ▸
        } else {
            vec![egui::pos2(cc.x - s, cc.y - s*0.5), egui::pos2(cc.x + s, cc.y - s*0.5),
                 egui::pos2(cc.x, cc.y + s*0.7)]                          // ▾
        };
        p.add(egui::Shape::convex_polygon(tri, TEXT, Stroke::NONE));
        let cr = Rect::from_center_size(cc, egui::vec2(18.0, RAIL_HEADER_H));
        let cresp = ui.interact(cr, ui.id().with((cfg.id, "hdr_collapse")), Sense::click());
        collapse_hovered = cresp.hovered();
        if cresp.hovered() { ui.ctx().set_cursor_icon(CursorIcon::PointingHand); }
        if cresp.clicked() { ui.ctx().data_mut(|d| d.insert_temp(cid, !collapsed)); }
        title_x = rect.left() + edge + 12.0;
    }
    // Title — menu-bar CATEGORY font (TextStyle::Button), panel-edge, centered.
    if !cfg.title.is_empty() {
        let font = ui.style().text_styles.get(&egui::TextStyle::Button).cloned()
            .unwrap_or_else(|| egui::FontId::proportional(14.0));
        p.text(egui::pos2(title_x, rect.center().y),
            Align2::LEFT_CENTER, cfg.title, font, TEXT);
    }
    // Grip (right end) — 5 lines, 5×1.5px, #1A2430; right-click → Close flyout.
    let grip_w = 5.0;
    let grip_x = rect.right() - 5.0 - grip_w;
    let total_h = 5.0 * 1.5 + 4.0 * 1.5;
    let mut gy = rect.center().y - total_h * 0.5;
    for _ in 0..5 {
        p.rect_filled(Rect::from_min_size(
            egui::pos2(grip_x, gy), egui::vec2(grip_w, 1.5)), 0.0, grip_col);
        gy += 3.0;
    }
    let grip_r = Rect::from_min_size(
        egui::pos2(grip_x - 2.0, rect.top()), egui::vec2(grip_w + 4.0, RAIL_HEADER_H));
    let grip_resp = ui.interact(grip_r, ui.id().with((cfg.id, "hdr_grip")), Sense::click());
    // Visible × close just LEFT of the grip — the grip's right-click
    // "Close" flyout alone was undiscoverable ("no way to close the
    // panel"). Same hit behaviour as the chrome header's ×: a click on
    // it closes instead of dragging.
    let xr = Rect::from_center_size(
        egui::pos2(grip_x - 16.0, rect.center().y), egui::vec2(18.0, 18.0));
    let over_x = ui.rect_contains_pointer(xr);
    let xcol = if over_x { TEXT } else { MUTED };
    let c = xr.center();
    let s = 4.5;
    let st = Stroke::new(1.5, xcol);
    p.line_segment([egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)], st);
    p.line_segment([egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y - s)], st);
    if band.hovered() && !grip_resp.hovered() && !collapse_hovered && !over_x {
        // owner: the header shows the default ARROW cursor (like other areas) —
        // only the grip / collapse chevron / × use the pointing hand.
        ui.ctx().set_cursor_icon(CursorIcon::Default);
    }
    // Grip right-click → sticky single-row "Close" flyout (state in ctx memory).
    // Click detection is done in SCREEN space (pointer released over the flyout
    // rect) rather than via the nested Area's own click — the latter proved
    // unreliable for the FLOATING panel (owner: "close flyout doesn't function").
    let menu_id = Id::new((cfg.id, "hdr_grip_menu"));
    let mut menu_open = ui.ctx().data(|d| d.get_temp::<bool>(menu_id).unwrap_or(false));
    let just_opened = grip_resp.secondary_clicked();
    if just_opened { menu_open = true; }
    let mut close_clicked = false;
    if menu_open {
        let anchor = egui::pos2(grip_r.left(), rect.bottom() + 2.0);
        let fly_rect = paint_close_flyout(ui.ctx(), Id::new((cfg.id, "hdr_grip_fly")), anchor);
        let pointer = ui.ctx().input(|i| i.pointer.hover_pos());
        let over = pointer.is_some_and(|pp| fly_rect.contains(pp));
        let released = ui.ctx().input(|i| i.pointer.primary_released());
        let pressed  = ui.ctx().input(|i| i.pointer.any_pressed());
        if over && released {
            close_clicked = true; menu_open = false;          // clicked "Close"
        } else if !just_opened && !over && pressed {
            menu_open = false;                                // click-away dismiss
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(menu_id, menu_open));
    // The × click closes directly (same semantics as the flyout's "Close").
    if band.clicked() && over_x {
        close_clicked = true;
        menu_open = false;
        ui.ctx().data_mut(|d| d.insert_temp(menu_id, menu_open));
    }
    (close_clicked, band)
}

/// Paint the single-row "Close" flyout under the header grip (surface-3 fill,
/// 1px border, r4, soft shadow, hover band) and RETURN its screen rect so the
/// caller can do robust screen-space click detection. (Rendering only — the
/// nested Area's own click was unreliable on floating panels.)
fn paint_close_flyout(ctx: &Context, id: Id, anchor: Pos2) -> Rect {
    let surf = crate::theme::color::SURFACE_3;
    let brd  = border();
    let band = crate::theme::color::SURFACE_2;   // hover band
    let font = crate::theme::typ::body();
    let label = "Close";
    let pad_x = 12.0_f32;
    let h = 26.0_f32;
    let tw = ctx.fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), TEXT)).size().x;
    let w = tw + pad_x * 2.0;
    let rect = Rect::from_min_size(anchor, egui::vec2(w, h));
    let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|pp| rect.contains(pp));
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            let p = ui.painter();
            p.rect_filled(rect.translate(egui::vec2(0.0, 2.0)).expand(1.0),
                egui::Rounding::same(4.0), Color32::from_black_alpha(70));
            p.rect(rect, egui::Rounding::same(4.0), surf, Stroke::new(1.0, brd));
            if over {
                p.rect_filled(rect.shrink(1.5), egui::Rounding::same(3.0), band);
                ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            }
            p.text(egui::pos2(rect.left() + pad_x, rect.center().y),
                Align2::LEFT_CENTER, label, font.clone(), TEXT);
        });
    rect
}

/// Docked-panel header — drag the band out to undock. Returns
/// `(close_clicked, undock_to, band)`; the band also carries the right-click
/// dock menu when the panel is `any_edge`.
fn docked_header(ui: &mut Ui, cfg: &DockConfig) -> (bool, Option<Pos2>, egui::Response) {
    let h = header_band(ui, cfg.title, cfg.badge, true);
    let undock = if h.band.drag_started() {
        h.band.interact_pointer_pos()
            .map(|p| egui::pos2((p.x - 130.0).max(0.0), (p.y - 12.0).max(48.0)))
    } else { None };
    (h.close_clicked, undock, h.band)
}

/// Floating-panel header — the band drags the window. Returns
/// `(close_clicked, drag_delta, drag_released, band)`; the band also carries
/// the right-click dock menu when the panel is `any_edge`.
fn float_header(ui: &mut Ui, cfg: &DockConfig) -> (bool, Vec2, bool, egui::Response) {
    let h = header_band(ui, cfg.title, cfg.badge, true);
    let delta = if h.band.dragged() { h.band.drag_delta() } else { Vec2::ZERO };
    (h.close_clicked, delta, h.band.drag_stopped(), h.band)
}

/// Where a panel floats after leaving a docked edge — clear of the edge it left
/// so it doesn't sit inside the re-dock zone. Shared by the floating-only
/// migration, undock, and the dock menu's "Float" row.
fn float_pos_for(cfg: &DockConfig, sr: egui::Rect, r: DockRegion) -> Pos2 {
    match r {
        DockRegion::Left   => egui::pos2(sr.left() + 60.0, sr.center().y - 120.0),
        DockRegion::Right  => egui::pos2((sr.right() - cfg.float_w - 60.0).max(20.0), sr.center().y - 120.0),
        DockRegion::Bottom => egui::pos2(sr.center().x - cfg.float_w * 0.5, (sr.bottom() - 240.0).max(sr.top() + 60.0)),
        DockRegion::Top    => egui::pos2(sr.center().x - cfg.float_w * 0.5, (sr.top() + 44.0).max(44.0)),
    }
}

/// The header right-click dock menu for `any_edge` panels (the toolbars): dock
/// to any of the four edges, or float. The current state is checkmarked.
/// Returns whether the state was changed — the caller uses that to skip its own
/// trailing drag-release state write in the same frame (a menu click is not a
/// drag, so an unguarded write would immediately undo the menu's choice).
fn dock_menu(ui: &mut Ui, cfg: &DockConfig, state: &mut DockState) -> bool {
    ui.set_min_width(140.0);
    for (r, label) in [
        (DockRegion::Left, "Dock left"),
        (DockRegion::Right, "Dock right"),
        (DockRegion::Top, "Dock top"),
        (DockRegion::Bottom, "Dock bottom"),
    ] {
        let active = matches!(*state, DockState::Docked(x) if x == r);
        if ui.selectable_label(active, label).clicked() {
            *state = DockState::Docked(r);
            ui.close_menu();
            return true;
        }
    }
    ui.separator();
    let floating = matches!(*state, DockState::Floating(_));
    if ui.selectable_label(floating, "Float").clicked() {
        let sr = ui.ctx().screen_rect();
        let pos = match *state {
            DockState::Floating(p) => p,
            DockState::Docked(r) => float_pos_for(cfg, sr, r),
        };
        *state = DockState::Floating(pos);
        ui.close_menu();
        return true;
    }
    false
}

/// The hand-rolled egui docking engine.
pub struct EguiDockHost;

impl DockHost for EguiDockHost {
    fn show(&self, ctx: &Context, cfg: &DockConfig, state: &mut DockState,
            open: &mut bool, body: impl FnOnce(&mut Ui, Option<f32>)) -> Rect {
        if !*open { return Rect::NOTHING; }
        // Floating-only panels never dock; migrate a stale Docked state (e.g.
        // persisted from before the panel became floating-only) to Floating so
        // the column renderer can't pick it up.
        if !cfg.dockable {
            if let DockState::Docked(r) = *state {
                let sr = ctx.screen_rect();
                *state = DockState::Floating(float_pos_for(cfg, sr, r));
            }
        }
        // Collapsed (minimised to header) — the rail header's chevron sets this;
        // when true, render the header only and skip the body.
        let collapsed = cfg.collapsible
            && ctx.data(|d| d.get_temp::<bool>(Id::new((cfg.id, "collapsed"))).unwrap_or(false));
        let frame = egui::Frame::none().fill(BG).stroke(Stroke::new(1.0, border()))
            .inner_margin(egui::Margin::ZERO);

        match *state {
            DockState::Docked(stored) => {
                // An `any_edge` panel keeps whichever edge its state names; a
                // single-edge panel always docks to its one allowed edge, no
                // matter what the stored state says.
                let region = if cfg.any_edge { stored } else { cfg.dock_region };
                let mut close = false;
                let mut undock: Option<Pos2> = None;
                let rect = match region {
                    DockRegion::Left | DockRegion::Right => {
                        let sp = if region == DockRegion::Right {
                            egui::SidePanel::right(Id::new((cfg.id, "dock")))
                        } else {
                            egui::SidePanel::left(Id::new((cfg.id, "dock")))
                        };
                        sp.resizable(cfg.resizable)
                            .default_width(cfg.size).min_width(cfg.min).max_width(cfg.max)
                            .frame(frame)
                            .show(ctx, |ui| {
                                let (c, u, band) = docked_header(ui, cfg);
                                close = c; undock = u;
                                if cfg.any_edge { band.context_menu(|ui| { dock_menu(ui, cfg, state); }); }
                                if cfg.flush_body {
                                    body(ui, None);
                                } else {
                                    egui::Frame::none()
                                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                        .show(ui, |ui| body(ui, None));
                                }
                            }).response.rect
                    }
                    DockRegion::Bottom => {
                        egui::TopBottomPanel::bottom(Id::new((cfg.id, "dock")))
                            .resizable(cfg.resizable)
                            .default_height(if cfg.any_edge { cfg.strip_h.max(1.0) } else { cfg.size })
                            .min_height(cfg.min).max_height(cfg.max)
                            .frame(frame)
                            .show(ctx, |ui| {
                                let (c, u, band) = docked_header(ui, cfg);
                                close = c; undock = u;
                                if cfg.any_edge { band.context_menu(|ui| { dock_menu(ui, cfg, state); }); }
                                if cfg.flush_body {
                                    body(ui, None);
                                } else {
                                    egui::Frame::none()
                                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                        .show(ui, |ui| body(ui, None));
                                }
                            }).response.rect
                    }
                    DockRegion::Top => {
                        egui::TopBottomPanel::top(Id::new((cfg.id, "dock")))
                            .resizable(cfg.resizable)
                            .default_height(cfg.strip_h.max(1.0))
                            .min_height(cfg.min).max_height(cfg.max)
                            .frame(frame)
                            .show(ctx, |ui| {
                                let (c, u, band) = docked_header(ui, cfg);
                                close = c; undock = u;
                                if cfg.any_edge { band.context_menu(|ui| { dock_menu(ui, cfg, state); }); }
                                if cfg.flush_body {
                                    body(ui, None);
                                } else {
                                    egui::Frame::none()
                                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                        .show(ui, |ui| body(ui, None));
                                }
                            }).response.rect
                    }
                };
                if close { *open = false; }
                if let Some(p) = undock {
                    // Lift the float clear of the edge it just left so it doesn't
                    // sit inside the re-dock zone (esp. Bottom, whose header is
                    // near the screen bottom). Keeps undock → float from snapping
                    // straight back.
                    let sr = ctx.screen_rect();
                    let fp = match region {
                        // Float to the LOWER-CENTRE of the canvas. Horizontally
                        // centred on the window; it may overlay the right bar
                        // (the float draws on top of the side panels). It won't
                        // re-dock on its own — docking only happens on an actual
                        // drag-release near the edge.
                        DockRegion::Bottom | DockRegion::Top =>
                            egui::pos2(sr.center().x - cfg.float_w * 0.5,
                                       (sr.bottom() - 220.0).max(sr.top() + 56.0)),
                        // Centre horizontally on undock (like Bottom) so the
                        // float lands clear of the right edge. Otherwise a wide
                        // panel's right edge stays in the snap zone and it
                        // re-docks the instant the drag is released.
                        DockRegion::Right =>
                            egui::pos2((sr.center().x - cfg.float_w * 0.5).max(20.0), p.y),
                        // Left (rails) are narrow and float fine near where grabbed.
                        DockRegion::Left =>
                            egui::pos2(p.x.max(sr.left() + 60.0), p.y),
                    };
                    *state = DockState::Floating(fp);
                }
                rect
            }
            DockState::Floating(pos) => {
                let cap = (ctx.screen_rect().height() * cfg.float_max_h_frac).max(160.0);
                let mut close = false;
                let mut delta = Vec2::ZERO;
                let mut released = false;
                let mut menu_mutated = false;
                let area = egui::Area::new(Id::new((cfg.id, "float")))
                    // Foreground: a float must stay ABOVE the Middle painter-layer
                    // rail/panel shadows (the "shadows over undocked toolbars /
                    // dialogs" rule).
                    .order(egui::Order::Foreground)
                    .fixed_pos(pos)
                    .constrain(true)
                    .show(ctx, |ui| {
                        egui::Frame::none().fill(BG).stroke(Stroke::new(1.0, border()))
                            .shadow(egui::epaint::Shadow {
                                offset: egui::vec2(0.0, 6.0), blur: 18.0, spread: 0.0,
                                color: Color32::from_black_alpha(120) })
                            .show(ui, |ui| {
                                ui.set_width(cfg.float_w);
                                ui.set_max_width(cfg.float_w);
                                let (c, d, r, band) = float_header(ui, cfg);
                                close = c; delta = d; released = r;
                                if cfg.any_edge {
                                    band.context_menu(|ui| {
                                        menu_mutated = dock_menu(ui, cfg, state);
                                    });
                                }
                                if cfg.flush_body {
                                    body(ui, Some(cap));
                                } else {
                                    egui::Frame::none()
                                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                        .show(ui, |ui| body(ui, Some(cap)));
                                }
                            });
                    });
                let wr = area.response.rect;
                if close { *open = false; }
                // Follow the pointer while dragging; dock ONLY when the drag is
                // released with the panel's own edge inside the snap zone.
                let np = egui::pos2((pos.x + delta.x).max(0.0), (pos.y + delta.y).max(44.0));
                let sr = ctx.screen_rect();
                // Gap between the panel's edge and the screen edge it faces;
                // `None` when the panel is further than 48 px away (in OR out).
                let edge_gap = |r: DockRegion| -> Option<f32> {
                    let g = match r {
                        DockRegion::Right  => wr.right()  - sr.right(),
                        DockRegion::Left   => sr.left()   - wr.left(),
                        DockRegion::Bottom => wr.bottom() - sr.bottom(),
                        DockRegion::Top    => sr.top()    - wr.top(),
                    };
                    (g >= -48.0 && g <= 48.0).then_some(g)
                };
                // An `any_edge` panel docks to whichever allowed edge is nearest
                // (smallest absolute gap); every other panel docks to its
                // `dock_region`/`alt_region` and nowhere else — so it never grabs
                // the wrong side and never re-docks mid-move.
                let dock_to = if cfg.any_edge {
                    [DockRegion::Left, DockRegion::Right, DockRegion::Top, DockRegion::Bottom]
                        .into_iter()
                        .filter_map(|r| edge_gap(r).map(|g| (r, g)))
                        .min_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                        .map(|(r, _)| r)
                } else if cfg.dockable {
                    if edge_gap(cfg.dock_region).is_some() {
                        Some(cfg.dock_region)
                    } else if cfg.alt_region.is_some_and(|r| edge_gap(r).is_some()) {
                        cfg.alt_region
                    } else { None }
                } else { None };
                // A dock-menu choice made THIS frame wins over the drag-release
                // bookkeeping — a menu click is not a drag, and the write below
                // would otherwise undo it immediately.
                if !menu_mutated {
                    *state = match (released, dock_to) {
                        (true, Some(r)) => DockState::Docked(r),
                        _ => DockState::Floating(np),
                    };
                }
                wr
            }
        }
    }
}

/// The active docking engine. Replace this (and add an `impl DockHost`) to swap
/// the underlying engine app-wide.
pub const HOST: EguiDockHost = EguiDockHost;
