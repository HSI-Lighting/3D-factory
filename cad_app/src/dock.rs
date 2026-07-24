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
pub enum DockRegion { Left, Right, Bottom }

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

/// Docked-panel header — drag the band out to undock. Returns
/// `(close_clicked, undock_to)`.
fn docked_header(ui: &mut Ui, cfg: &DockConfig) -> (bool, Option<Pos2>) {
    let h = header_band(ui, cfg.title, cfg.badge, true);
    let undock = if h.band.drag_started() {
        h.band.interact_pointer_pos()
            .map(|p| egui::pos2((p.x - 130.0).max(0.0), (p.y - 12.0).max(48.0)))
    } else { None };
    (h.close_clicked, undock)
}

/// Floating-panel header — the band drags the window. Returns
/// `(close_clicked, drag_delta, drag_released)`; the host docks only on release.
fn float_header(ui: &mut Ui, cfg: &DockConfig) -> (bool, Vec2, bool) {
    let h = header_band(ui, cfg.title, cfg.badge, true);
    let delta = if h.band.dragged() { h.band.drag_delta() } else { Vec2::ZERO };
    (h.close_clicked, delta, h.band.drag_stopped())
}

/// The hand-rolled egui docking engine.
pub struct EguiDockHost;

impl DockHost for EguiDockHost {
    fn show(&self, ctx: &Context, cfg: &DockConfig, state: &mut DockState,
            open: &mut bool, body: impl FnOnce(&mut Ui, Option<f32>)) -> Rect {
        if !*open { return Rect::NOTHING; }
        let frame = egui::Frame::none().fill(BG).stroke(Stroke::new(1.0, border()))
            .inner_margin(egui::Margin::ZERO);

        match *state {
            DockState::Docked(_) => {
                // Always dock to the panel's one allowed edge, regardless of what
                // the stored state says — a panel can't be docked anywhere else.
                let region = cfg.dock_region;
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
                                let (c, u) = docked_header(ui, cfg);
                                close = c; undock = u;
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
                            .default_height(cfg.size).min_height(cfg.min).max_height(cfg.max)
                            .frame(frame)
                            .show(ctx, |ui| {
                                let (c, u) = docked_header(ui, cfg);
                                close = c; undock = u;
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
                        // drag-release near the bottom edge.
                        DockRegion::Bottom =>
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
                let area = egui::Area::new(Id::new((cfg.id, "float")))
                    .order(egui::Order::Middle)
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
                                let (c, d, r) = float_header(ui, cfg);
                                close = c; delta = d; released = r;
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
                // released with the panel's own edge inside the snap zone. A
                // panel can dock to its `dock_region` and nowhere else, so it
                // never grabs the wrong side and never re-docks mid-move.
                let np = egui::pos2((pos.x + delta.x).max(0.0), (pos.y + delta.y).max(44.0));
                let sr = ctx.screen_rect();
                let near_edge = match cfg.dock_region {
                    DockRegion::Right  => wr.right()  >= sr.right()  - 48.0,
                    DockRegion::Left   => wr.left()   <= sr.left()   + 48.0,
                    DockRegion::Bottom => wr.bottom() >= sr.bottom() - 48.0,
                };
                *state = if released && near_edge {
                    DockState::Docked(cfg.dock_region)
                } else {
                    DockState::Floating(np)
                };
                wr
            }
        }
    }
}

/// The active docking engine. Replace this (and add an `impl DockHost`) to swap
/// the underlying engine app-wide.
pub const HOST: EguiDockHost = EguiDockHost;
