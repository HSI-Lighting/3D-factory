use super::*;

// ============ 2D dialogs, scripting and plotting ============
// The remaining docked/floating windows and modal flows of the 2D
// workspace: layer panel chrome + dock-window glue, the hatch pattern
// library and confirm panel, the text-style dialog, the python engine
// pump and script preview ops, ATTDEF/ATTEDIT flows, the in-app python
// editor, the CTB manager + plot-style tables and the plot dialog /
// preview pipeline. Child module of `app`; entry methods are pub(super)
// so the shell, panel and tests can reach them.

impl CadApp {
    // ===================================================================
    // Slice B — Layer panel
    // ===================================================================
    //
    // Egui-port of LibreCAD's `qg_layerwidget`. Operates directly on
    // `self.doc.layers` (the `LayerTable` from cad_kernel). Active layer
    // = the one new Dobjects get assigned to on `Document::push`.

    /// "Choose Hatch Attributes" modal — pattern + scale + angle +
    /// live preview. Mirrors LibreCAD's hatch dialog. OK feeds into
    /// the same `apply_hatch` path as the command-line form; Cancel
    /// drops the pending state. Opens whenever the user types bare
    /// `hatch` with no args.
    /// Width within which a floating Window's edge is treated as "close
    /// enough to dock". Same value the user asked for.
    const DOCK_THRESHOLD_PX: f32 = 50.0;
    /// Minimum width docked top/bottom strips will use. Prevents the
    /// window from shrinking below this when the user resizes it
    /// after docking.
    const DOCK_STRIP_MIN_WIDTH: f32 = 320.0;
    /// Minimum height docked left/right strips will use.
    const DOCK_STRIP_MIN_HEIGHT: f32 = 200.0;
    /// Hold the title bar this long (seconds) to release a docked
    /// window. Long enough that an accidental click doesn't undock,
    /// short enough that a deliberate "I want to move this" feels
    /// instant.
    const DOCK_UNDOCK_HOLD_SEC: f32 = 0.20;

    /// If `id` has a docked state stored, pin its position AND constrain
    /// its size based on which edge it's docked to:
    ///   * Top/Bottom → forces `min_width = screen_width` so the
    ///     window becomes a horizontal strip spanning the screen.
    ///   * Left/Right → forces `min_height = screen_height` for a
    ///     vertical strip.
    ///   * Corners    → pinned position only; size stays content-fit.
    /// Reversible — when the user drags away further than the
    /// threshold, `process_dock_after_show` removes the entry and the
    /// Window goes back to free positioning + sizing.
    pub(super) fn apply_dock_pos<'a>(
        &self,
        id: &'static str,
        _ctx: &egui::Context,
        window: egui::Window<'a>,
    ) -> egui::Window<'a> {
        // Auto-docking + auto-resize disabled per user request — windows are plain
        // floating (user moves/resizes manually). §10 menu-launch positioning: if a
        // menu item asked this panel to open at a specific anchor, use it as the
        // `default_pos` (overriding the window's own default). `default_pos` only
        // applies on the FIRST open / when egui has no remembered position — so once
        // the user drags the panel, its saved geometry wins (WORKSPACE §5).
        if let Some(&anchor) = self.menu_launch_anchor.get(id) {
            window.default_pos(anchor)
        } else {
            window
        }
    }

    /// §10 bring-to-front. Call right after a floating panel's `.show(...)`: if a
    /// menu just toggled it ON (`id` is queued in `raise_windows`), raise its layer
    /// above the other panels and consume the request. Fires once on open, whether
    /// or not the panel used a remembered position. A no-op when nothing queued.
    pub(super) fn raise_after_show<R>(
        &mut self,
        id: &'static str,
        ctx: &egui::Context,
        resp: &Option<egui::InnerResponse<R>>,
    ) {
        if let Some(i) = self.raise_windows.iter().position(|&x| x == id) {
            if let Some(r) = resp {
                ctx.move_to_top(r.response.layer_id);
                self.raise_windows.remove(i);
            }
        }
    }

    /// §10 bring-to-front for a `dock::HOST` panel (Inspector, Command line). Call
    /// after its `.show(...)`. The host's floating area id is `(dock_id, "float")`
    /// at `Order::Middle`; raise it only when actually `floating` (a docked panel is
    /// pinned to its edge — already visible, nothing to raise). Consumes the request.
    pub(super) fn raise_dock_after_show(
        &mut self,
        raise_key: &'static str,
        dock_id: &'static str,
        floating: bool,
        ctx: &egui::Context,
    ) {
        if let Some(i) = self.raise_windows.iter().position(|&x| x == raise_key) {
            if floating {
                ctx.move_to_top(egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new((dock_id, "float")),
                ));
            }
            self.raise_windows.remove(i);
        }
    }

    /// After a Window has been shown, compute whether any of its edges
    /// is within `DOCK_THRESHOLD_PX` of a screen edge. If so, snap it
    /// flush to that edge and store the snapped position. Otherwise
    /// clear any prior snap so subsequent drags are free.
    pub(super) fn process_dock_after_show<R>(
        &mut self,
        id: &'static str,
        ctx: &egui::Context,
        resp: Option<egui::InnerResponse<R>>,
    ) {
        // Auto-snap disabled per user request. Was triggering false
        // positives (windows docking to top on first appear, all
        // windows piling up at the top, etc.). Will be redesigned in
        // a dedicated dock-system pass later. For now: leave windows
        // free-floating — user manages position/size manually.
        let _ = (id, ctx, resp);
    }

    pub(super) fn render_hatch_dialog(&mut self, ctx: &egui::Context) {
        if !self.hatch_dialog_open {
            return;
        }
        let mut open = true;
        let mut clicked_dobjects = false;
        let mut clicked_pick_point = false;
        // ============ COMBINED FLOATING WINDOW ============
        // Single floating window with a 3-row horizontal thumbnail
        // strip at the TOP and Preview + Hatch Parameters BELOW.
        // No anchor, no fixed pos — fully draggable. Initial position
        // centred. Thumbnails fill column-by-column (top→bottom, then
        // next column) so adding new patterns just appends more columns
        // to the right; the strip scrolls horizontally if it overflows
        // the visible area.
        let screen = ctx.screen_rect();
        let win_w = 1100.0_f32;
        let win_h = 620.0_f32;
        let default_pos = egui::pos2(
            screen.center().x - win_w * 0.5,
            screen.center().y - win_h * 0.5,
        );
        let accent = egui::Color32::from_rgb(255, 80, 80);
        let thumb_w = 112.0_f32;
        let thumb_h = 64.0_f32;
        let cell_w = thumb_w + 6.0;
        let cell_h = thumb_h + 22.0;
        let rows = 3_usize;
        egui::Window::new("Choose Hatch Attributes")
            .id(egui::Id::new("hatch_dialog"))
            .open(&mut open)
            .default_size(egui::vec2(win_w, win_h))
            .default_pos(default_pos)
            .resizable(false)
            .collapsible(true)
            .show(ctx, |ui| {
                // ============ TOP: 3-row thumbnail strip ============
                ui.horizontal(|ui| {
                    ui.heading("Pattern");
                    ui.add_space(12.0);
                    ui.checkbox(&mut self.hatch_dialog_solid, "Solid Fill");
                });
                ui.add_space(4.0);
                ui.add_enabled_ui(!self.hatch_dialog_solid, |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("hatch_thumb_scroll")
                        .max_width(win_w - 32.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            let names: Vec<&str> =
                                cad_kernel::patterns::PATTERN_NAMES
                                    .iter()
                                    .copied()
                                    .filter(|n| *n != "SOLID")
                                    .collect();
                            let n_cols = names.len().div_ceil(rows).max(1);
                            let total_w =
                                (n_cols as f32) * cell_w + 4.0;
                            let total_h = (rows as f32) * cell_h + 4.0;
                            let (alloc_rect, _) =
                                ui.allocate_exact_size(
                                    egui::vec2(total_w, total_h),
                                    egui::Sense::hover());
                            let origin = alloc_rect.left_top();
                            // Column-major fill: ANSI31, ANSI32, ANSI33
                            // go down COL 0; ANSI37, CROSS, NET down COL 1
                            // — keeps related patterns vertically grouped.
                            for (i, name) in names.iter().enumerate() {
                                let col = i / rows;
                                let row = i % rows;
                                let cell_origin = origin
                                    + egui::vec2(
                                        col as f32 * cell_w,
                                        row as f32 * cell_h);
                                let thumb_rect = egui::Rect::from_min_size(
                                    cell_origin,
                                    egui::vec2(thumb_w, thumb_h));
                                let label_pos = egui::pos2(
                                    cell_origin.x + thumb_w * 0.5,
                                    cell_origin.y + thumb_h + 11.0);
                                let resp = ui.interact(
                                    thumb_rect,
                                    egui::Id::new(("hatch_thumb", i)),
                                    egui::Sense::click());
                                let selected =
                                    self.hatch_dialog_name == *name;
                                let bg = if selected {
                                    egui::Color32::from_rgb(30, 60, 110)
                                } else {
                                    egui::Color32::from_rgb(18, 22, 28)
                                };
                                let border = if selected {
                                    egui::Color32::from_rgb(120, 180, 255)
                                } else if resp.hovered() {
                                    egui::Color32::from_rgb(150, 160, 175)
                                } else {
                                    egui::Color32::from_rgb(70, 80, 95)
                                };
                                let bw = if selected { 2.0 } else { 1.0 };
                                let p = ui.painter();
                                p.rect_filled(thumb_rect, 3.0, bg);
                                p.rect_stroke(thumb_rect, 3.0,
                                    egui::Stroke::new(bw, border));
                                let pad = 6.0_f32;
                                let inner = egui::Rect::from_min_max(
                                    thumb_rect.left_top()
                                        + egui::vec2(pad, pad),
                                    thumb_rect.right_bottom()
                                        - egui::vec2(pad, pad));
                                paint_pattern_preview(
                                    p, inner, name, 1.0, 0.0,
                                    accent, 3.2, 1.4);
                                let label_col = if selected {
                                    egui::Color32::from_rgb(180, 210, 255)
                                } else {
                                    egui::Color32::from_rgb(200, 210, 220)
                                };
                                p.text(label_pos,
                                    egui::Align2::CENTER_CENTER,
                                    *name,
                                    crate::theme::typ::body(),
                                    label_col);
                                if resp.clicked() {
                                    self.hatch_dialog_name =
                                        (*name).to_string();
                                }
                            }
                        });
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                // ============ BOTTOM: preview LEFT, parameters RIGHT ============
                ui.horizontal(|ui| {
                    // ----- Preview -----
                    ui.vertical(|ui| {
                        ui.heading("Preview");
                        let (rect, _resp) = ui.allocate_exact_size(
                            egui::vec2(260.0, 260.0), egui::Sense::hover());
                        let p = ui.painter_at(rect);
                        p.rect_filled(rect, 0.0,
                            egui::Color32::from_rgb(18, 22, 28));
                        p.rect_stroke(rect, 0.0,
                            egui::Stroke::new(1.0,
                                egui::Color32::from_rgb(70, 80, 95)));
                        let pad = 18.0_f32;
                        let bound = egui::Rect::from_min_max(
                            rect.left_top()  + egui::vec2(pad, pad),
                            rect.right_bottom() - egui::vec2(pad, pad));
                        if self.hatch_dialog_solid {
                            p.rect_filled(bound, 0.0, accent);
                        } else {
                            paint_pattern_preview(
                                &p, bound,
                                &self.hatch_dialog_name,
                                self.hatch_dialog_scale.max(0.05),
                                self.hatch_dialog_angle.to_radians(),
                                accent, 10.0, 1.0);
                        }
                        let c = rect.center();
                        let mk = egui::Stroke::new(1.0,
                            egui::Color32::from_rgb(80, 90, 105));
                        p.line_segment([egui::pos2(c.x - 8.0, c.y),
                                        egui::pos2(c.x + 8.0, c.y)], mk);
                        p.line_segment([egui::pos2(c.x, c.y - 8.0),
                                        egui::pos2(c.x, c.y + 8.0)], mk);
                    });
                    ui.separator();
                    // ----- Hatch Parameters (RIGHT side of bottom row) -----
                    ui.vertical(|ui| {
                        ui.heading("Hatch Parameters");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("Scale:");
                            ui.add_space(4.0);
                            ui.spacing_mut().slider_width = 160.0;
                            ui.add(egui::Slider::new(
                                    &mut self.hatch_dialog_scale, 0.05..=20.0)
                                .logarithmic(true)
                                .show_value(false));
                            ui.add_space(4.0);
                            ui.add(egui::DragValue::new(
                                    &mut self.hatch_dialog_scale).update_while_editing(false)
                                .speed(0.01)
                                .range(0.05..=100.0)
                                .min_decimals(3)
                                .max_decimals(3));
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label("Angle:");
                                ui.add_space(80.0);
                            });
                            polar_angle_picker(ui,
                                &mut self.hatch_dialog_angle, 120.0);
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                ui.add_space(54.0);
                                ui.add(egui::DragValue::new(
                                        &mut self.hatch_dialog_angle).update_while_editing(false)
                                    .speed(1.0)
                                    .range(-360.0..=360.0)
                                    .suffix("°"));
                            });
                        });
                    });
                });
                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui|
                    {
                        if ui.button("Cancel")
                            .on_hover_text("Discard these choices and close the dialog")
                            .clicked()
                        {
                            self.hatch_dialog_open = false;
                            self.hatch_dialog_edit_mode = false;
                            self.hatch_dbg("dialog cancelled".to_string());
                            self.clear_prompt();
                        }
                        if ui.button("OK")
                            .on_hover_text("Save these attributes; pick boundary via Select Objects or Pick Point next")
                            .clicked() { clicked_dobjects = true; }
                        ui.separator();
                        if ui.button("◉ Pick Point")
                            .on_hover_text("Click inside a closed region — the app finds the smallest containing closed dobject and hatches it")
                            .clicked() { clicked_pick_point = true; }
                        if ui.button("☐ Select Objects")
                            .on_hover_text("Select boundary dobjects (closed polylines / circles / ellipses), Enter to fill")
                            .clicked() { clicked_dobjects = true; }
                    });
                });
            });
        // X close button → dismiss (no commit, no follow-up state).
        if !open {
            self.hatch_dialog_open = false;
            self.hatch_dbg("dialog dismissed via X close".to_string());
            self.clear_prompt();
            return;
        }
        if clicked_dobjects || clicked_pick_point {
            self.hatch_dialog_open = false;
            let pattern = if self.hatch_dialog_solid {
                None
            } else {
                Some(self.hatch_dialog_name.clone())
            };
            self.pending_hatch_pattern = (
                pattern.clone(),
                self.hatch_dialog_scale,
                self.hatch_dialog_angle,
            );
            let style = pattern.as_deref().unwrap_or("SOLID").to_string();
            self.hatch_dbg(format!(
                "dialog committed: pattern={}, scale={:.3}, angle={:.2}",
                style, self.hatch_dialog_scale, self.hatch_dialog_angle
            ));
            if clicked_dobjects {
                self.hatch_dbg(format!(
                    "  Dobject/s button — selection.len() = {}",
                    self.selection.len()
                ));
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Hatch;
                    self.set_prompt(format!(
                        "hatch ({}): pick CLOSED boundary dobject(s), Enter to fill  [Esc=cancel]",
                        style
                    ));
                } else {
                    self.apply_hatch();
                }
            } else {
                self.hatch_pick_point_armed = true;
                // Remember the pattern for the whole pick-point session
                // so successive clicks all use it (apply_hatch consumes
                // pending_hatch_pattern; we re-fill it after each click).
                self.hatch_pick_point_session = Some((
                    pattern.clone(),
                    self.hatch_dialog_scale,
                    self.hatch_dialog_angle,
                ));
                self.hatch_dbg(
                    "  Pick Point button — armed canvas click (persistent until Enter)".to_string(),
                );
                self.set_prompt(format!(
                    "hatch ({}): click inside closed region(s); Enter to finish  [Esc=cancel]",
                    style
                ));
            }
        }
    }

    /// Hatch Pattern Library — a SEPARATE floating window that opens
    /// alongside the attributes dialog. Holds just the thumbnail grid:
    /// 3 columns wide, scrollable vertically, room for a 3 × 9 layout
    /// (27 patterns) and beyond. Clicking a thumbnail sets
    /// `hatch_dialog_name` so the attributes dialog's preview reflects
    /// the choice instantly.
    ///
    /// Kept separate from the attributes dialog because nested egui
    /// layouts (ScrollArea inside vertical inside horizontal) had
    /// recurring sizing-pass bugs that hid rows beyond the first one.
    /// A standalone window owns its own root layout pass and renders
    /// every thumbnail deterministically.
    fn render_hatch_pattern_library(&mut self, ctx: &egui::Context) {
        if !self.hatch_dialog_open {
            return;
        }
        let accent = egui::Color32::from_rgb(255, 80, 80);
        let thumb_w = 112.0_f32;
        let thumb_h = 64.0_f32;
        let cell_w = thumb_w + 6.0;
        let cell_h = thumb_h + 22.0;
        let cols = 3_usize;
        let win_w = (cell_w * cols as f32) + 24.0; // 360
        let win_h = 600.0_f32; // ~6 rows visible
                               // Position the library to the LEFT of the centred attributes
                               // dialog. The attributes dialog is 440 wide, anchored centre;
                               // its left edge is at (screen_w - 440)/2. Place the library
                               // immediately left of that with a small gap.
        let screen = ctx.screen_rect();
        let attr_left = (screen.width() - 440.0) * 0.5 + screen.left();
        let lib_pos = egui::pos2(attr_left - win_w - 12.0, screen.top() + 80.0);
        let mut open = true;
        egui::Window::new("Hatch Pattern Library")
            .id(egui::Id::new("hatch_pattern_library"))
            .open(&mut open)
            .default_size(egui::vec2(win_w, win_h))
            .resizable(false)
            .collapsible(true)
            // Float freely — user can drag anywhere. `default_pos` only
            // applies the FIRST time the window appears; after that
            // egui remembers the user's chosen position.
            .default_pos(lib_pos)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Click a thumbnail to choose the pattern")
                        .small()
                        .weak(),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Manual grid: compute the absolute screen
                        // position for every cell using `ui.cursor()`
                        // + advance manually. This bypasses egui's
                        // wrap/grid sizing entirely.
                        let names: Vec<&str> = cad_kernel::patterns::PATTERN_NAMES
                            .iter()
                            .copied()
                            .filter(|n| *n != "SOLID")
                            .collect();
                        let n_rows = (names.len() + cols - 1) / cols;
                        let total_h = (n_rows as f32) * cell_h + 4.0;
                        let (alloc_rect, _) = ui.allocate_exact_size(
                            egui::vec2(cell_w * cols as f32, total_h),
                            egui::Sense::hover(),
                        );
                        let origin = alloc_rect.left_top();
                        for (i, name) in names.iter().enumerate() {
                            let col = i % cols;
                            let row = i / cols;
                            let cell_origin =
                                origin + egui::vec2(col as f32 * cell_w, row as f32 * cell_h);
                            let thumb_rect = egui::Rect::from_min_size(
                                cell_origin,
                                egui::vec2(thumb_w, thumb_h),
                            );
                            let label_pos = egui::pos2(
                                cell_origin.x + thumb_w * 0.5,
                                cell_origin.y + thumb_h + 11.0,
                            );
                            let resp = ui.interact(
                                thumb_rect,
                                egui::Id::new(("hatch_thumb", i)),
                                egui::Sense::click(),
                            );
                            let selected = self.hatch_dialog_name == *name;
                            let bg = if selected {
                                egui::Color32::from_rgb(30, 60, 110)
                            } else {
                                egui::Color32::from_rgb(18, 22, 28)
                            };
                            let border = if selected {
                                egui::Color32::from_rgb(120, 180, 255)
                            } else if resp.hovered() {
                                egui::Color32::from_rgb(150, 160, 175)
                            } else {
                                egui::Color32::from_rgb(70, 80, 95)
                            };
                            let bw = if selected { 2.0 } else { 1.0 };
                            let p = ui.painter();
                            p.rect_filled(thumb_rect, 3.0, bg);
                            p.rect_stroke(thumb_rect, 3.0, egui::Stroke::new(bw, border));
                            let pad = 6.0_f32;
                            let inner = egui::Rect::from_min_max(
                                thumb_rect.left_top() + egui::vec2(pad, pad),
                                thumb_rect.right_bottom() - egui::vec2(pad, pad),
                            );
                            paint_pattern_preview(p, inner, name, 1.0, 0.0, accent, 3.2, 1.4);
                            let label_col = if selected {
                                egui::Color32::from_rgb(180, 210, 255)
                            } else {
                                egui::Color32::from_rgb(200, 210, 220)
                            };
                            p.text(
                                label_pos,
                                egui::Align2::CENTER_CENTER,
                                *name,
                                crate::theme::typ::body(),
                                label_col,
                            );
                            if resp.clicked() {
                                self.hatch_dialog_name = (*name).to_string();
                            }
                        }
                    });
            });
        if !open {
            // Closing the library also closes the attributes dialog
            // — they're a pair.
            self.hatch_dialog_open = false;
            self.hatch_dialog_edit_mode = false;
            self.clear_prompt();
        }
    }

    /// Post-apply confirmation panel. Opens after every successful
    /// `apply_hatch()`. Four actions:
    ///   • Confirm           — close the panel, hatch flow done
    ///   • Change pattern/Scale — re-open the attributes dialog in
    ///                            edit-mode; OK patches the LAST hatch
    ///                            in place rather than pushing a new one
    ///   • + Pick Point      — re-arm pick-point mode so the user can
    ///                         click more interior seeds (each fires a
    ///                         new hatch with the same pattern)
    ///   • + Dobject         — open a fresh boundary-pick selection so
    ///                         the user can build another hatch from
    ///                         specific boundary dobjects.
    /// Esc/X dismisses the panel without consequence.
    pub(super) fn render_hatch_confirm_panel(&mut self, ctx: &egui::Context) {
        if !self.hatch_confirm_open {
            return;
        }
        let Some(idx) = self.hatch_last_idx else {
            self.hatch_confirm_open = false;
            return;
        };
        let (pat_label, scale, angle) = match self.doc.dobjects.get(idx) {
            Some(d) => match &d.geom {
                Geom::Hatch(h) => match &h.pattern {
                    cad_kernel::HatchPattern::Solid => ("SOLID".to_string(), 1.0_f64, 0.0_f64),
                    cad_kernel::HatchPattern::Pattern {
                        name,
                        scale,
                        angle_deg,
                    } => (name.clone(), *scale, *angle_deg),
                },
                _ => {
                    self.hatch_confirm_open = false;
                    return;
                }
            },
            None => {
                self.hatch_confirm_open = false;
                return;
            }
        };
        let mut open = true;
        let mut do_confirm = false;
        let mut do_discard = false;
        let mut do_change = false;
        let mut do_add_point = false;
        let mut do_add_dob = false;
        // Modal-style backdrop — dims the rest of the screen so the
        // confirm dialog is impossible to miss. Painted on a foreground
        // layer below the window itself.
        let screen = ctx.screen_rect();
        let backdrop_layer = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("hatch_confirm_backdrop"),
        );
        ctx.layer_painter(backdrop_layer).rect_filled(
            screen,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 110),
        );
        // Swallow clicks on the backdrop so users can't accidentally
        // click through into the canvas while the modal is up.
        let _ = ctx.read_response(backdrop_layer.id);
        egui::Window::new("Hatch — Confirm preview")
            .id(egui::Id::new("hatch_confirm_panel"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .movable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_min_width(440.0);
                ui.add_space(4.0);
                ui.label(egui::RichText::new(format!(
                    "Hatch #{} preview — pattern: {}  scale: {:.3}  angle: {:.2}°",
                    idx, pat_label, scale, angle))
                    .strong()
                    .size(14.0));
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label("The fill on the canvas is a PREVIEW.");
                ui.label("Confirm to commit, Discard to revert, or pick another action below.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([110.0, 28.0],
                        egui::Button::new(egui::RichText::new("✔ Confirm").font(crate::theme::typ::body_strong()))
                            .fill(egui::Color32::from_rgb(40, 110, 60)))
                        .on_hover_text("Accept this hatch and end the hatch flow  [cmd: c / y / Enter]")
                        .clicked() { do_confirm = true; }
                    if ui.add_sized([110.0, 28.0],
                        egui::Button::new(egui::RichText::new("✗ Discard").font(crate::theme::typ::body_strong()))
                            .fill(egui::Color32::from_rgb(140, 50, 50)))
                        .on_hover_text("Revert to the state BEFORE this hatch flow started  [cmd: d / n]")
                        .clicked() { do_discard = true; }
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("✎ Change pattern/Scale")
                        .on_hover_text("Reopen the attributes dialog; OK will modify THIS hatch in place  [cmd: ch]")
                        .clicked() { do_change = true; }
                    if ui.button("◉ + Pick Point")
                        .on_hover_text("Click more interior seeds to create additional hatches with the same pattern  [cmd: p]")
                        .clicked() { do_add_point = true; }
                    if ui.button("☐ + Dobject")
                        .on_hover_text("Pick more boundary dobjects to build another hatch with the same pattern  [cmd: D]")
                        .clicked() { do_add_dob = true; }
                });
                ui.add_space(4.0);
                ui.label(egui::RichText::new(
                    "tip: Enter = Confirm   ·   Esc = Discard   ·   single-letter aliases work")
                    .small()
                    .weak());
            });
        if !open {
            self.hatch_confirm_discard();
            return;
        }
        if do_confirm {
            self.hatch_confirm_accept();
            return;
        }
        if do_discard {
            self.hatch_confirm_discard();
            return;
        }
        if do_change {
            self.hatch_confirm_change();
            return;
        }
        if do_add_point {
            self.hatch_confirm_add_point();
            return;
        }
        if do_add_dob {
            self.hatch_confirm_add_dobject();
            return;
        }
    }

    /// Helper — fetch the active hatch's pattern label, scale, angle.
    /// Returns None if the panel state is stale (last index out of
    /// range or no longer a Hatch).
    fn hatch_confirm_info(&self) -> Option<(usize, String, f64, f64)> {
        let idx = self.hatch_last_idx?;
        let d = self.doc.dobjects.get(idx)?;
        if let Geom::Hatch(h) = &d.geom {
            match &h.pattern {
                cad_kernel::HatchPattern::Solid => Some((idx, "SOLID".to_string(), 1.0, 0.0)),
                cad_kernel::HatchPattern::Pattern {
                    name,
                    scale,
                    angle_deg,
                } => Some((idx, name.clone(), *scale, *angle_deg)),
            }
        } else {
            None
        }
    }

    /// Confirm-panel action: accept the hatch, end the flow. Promotes
    /// the preview snapshot onto the global undo stack so a single
    /// Ctrl+Z reverts the entire flow.
    pub(super) fn hatch_confirm_accept(&mut self) {
        let idx_for_log = self.hatch_last_idx;
        self.hatch_confirm_open = false;
        if let Some(snap) = self.hatch_preview_snap.take() {
            self.undo_stack.push(UndoStep::Doc(snap));
            self.trim_undo_stack();
            self.redo_stack.clear();
        }
        self.clear_prompt();
        if let Some(i) = idx_for_log {
            self.history.push(format!("  ✔ hatch #{} confirmed", i));
        }
    }

    /// Confirm-panel action: revert to pre-hatch state.
    pub(super) fn hatch_confirm_discard(&mut self) {
        self.hatch_confirm_open = false;
        if let Some(snap) = self.hatch_preview_snap.take() {
            self.doc = snap;
            self.selection.clear();
            self.selected = None;
            self.index_dirty = true;
            self.touch_view();
            self.hatch_last_idx = None;
            self.history
                .push("  ✗ hatch discarded — reverted to pre-hatch state".into());
            self.hatch_dbg(
                "confirm panel: Discard — restored pre-hatch document snapshot".to_string(),
            );
        }
        self.clear_prompt();
    }

    /// Confirm-panel action: reopen the attributes dialog in edit-mode.
    pub(super) fn hatch_confirm_change(&mut self) {
        let Some((idx, pat_label, scale, angle)) = self.hatch_confirm_info() else {
            self.hatch_confirm_open = false;
            return;
        };
        self.hatch_confirm_open = false;
        self.hatch_dialog_edit_mode = true;
        self.hatch_dialog_solid = pat_label == "SOLID";
        if !self.hatch_dialog_solid {
            self.hatch_dialog_name = pat_label;
            self.hatch_dialog_scale = scale;
            self.hatch_dialog_angle = angle;
        }
        self.hatch_dialog_open = true;
        self.hatch_dbg(format!(
            "confirm panel: change pattern/scale on #{} — re-opened dialog in edit mode",
            idx
        ));
    }

    /// Confirm-panel action: re-arm pick-point session with same pattern.
    pub(super) fn hatch_confirm_add_point(&mut self) {
        let Some((_, pat_label, scale, angle)) = self.hatch_confirm_info() else {
            self.hatch_confirm_open = false;
            return;
        };
        self.hatch_confirm_open = false;
        self.hatch_dialog_edit_mode = false;
        let pat_opt = if pat_label == "SOLID" {
            None
        } else {
            Some(pat_label.clone())
        };
        self.pending_hatch_pattern = (pat_opt.clone(), scale, angle);
        self.hatch_pick_point_session = Some((pat_opt, scale, angle));
        self.hatch_pick_point_armed = true;
        self.set_prompt(format!(
            "hatch ({}): click inside closed region(s); Enter to finish  [Esc=cancel]",
            pat_label
        ));
        self.hatch_dbg("confirm panel: + Pick Point — re-armed pick-point session".to_string());
    }

    /// Confirm-panel action: open a fresh boundary-pick selection.
    pub(super) fn hatch_confirm_add_dobject(&mut self) {
        let Some((_, pat_label, scale, angle)) = self.hatch_confirm_info() else {
            self.hatch_confirm_open = false;
            return;
        };
        self.hatch_confirm_open = false;
        self.hatch_dialog_edit_mode = false;
        let pat_opt = if pat_label == "SOLID" {
            None
        } else {
            Some(pat_label.clone())
        };
        self.pending_hatch_pattern = (pat_opt, scale, angle);
        self.selection.clear();
        self.begin_selection(SelectMode::ForSelect);
        self.queued_op = QueuedOp::Hatch;
        self.set_prompt(format!(
            "hatch ({}): pick CLOSED boundary dobject(s), Enter to fill  [Esc=cancel]",
            pat_label
        ));
        self.hatch_dbg("confirm panel: + Dobject — opened selection for new boundary".to_string());
    }

    /// New / Edit Text Style dialog — small modal-style window for
    /// managing TextStyleTable entries. Opens via the `style` cmd or
    /// the future Styles panel. Fields:
    ///   * Name              (text entry — uppercase canonical)
    ///   * Font              (combo: "standard" + any future LFF loaded)
    ///   * Default height    (drag value)
    ///   * Color             (ByLayer checkbox + ACI picker shortcut)
    /// OK commits the form into doc.text_styles (add or update).
    /// Cancel / X dismisses without writes.
    pub(super) fn render_text_style_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.text_style_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut do_ok = false;
        let mut do_cancel = false;
        // Snapshot available font names. v1 has exactly one ("standard");
        // when the LFF parser lands every loaded .lff joins this list
        // and the combo picks up new entries with no UI changes.
        // Font choices: the two egui built-ins (CPU fallback) + every installed
        // system font from the TTF engine.
        let mut font_choices: Vec<String> = vec!["standard".to_string(), "monospace".to_string()];
        font_choices.extend(self.font_manager.borrow_mut().names().iter().cloned());
        egui::Window::new(if dialog.editing_id.is_some() {
            "Edit Text Style"
        } else {
            "New Text Style"
        })
        .order(egui::Order::Foreground)
        .id(egui::Id::new("text_style_dialog"))
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_size(egui::vec2(360.0, 260.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.add_space(4.0);
            egui::Grid::new("text_style_form")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.name)
                            .desired_width(220.0)
                            .hint_text("STANDARD, NOTES, TITLE…"),
                    );
                    ui.end_row();

                    ui.label("Font");
                    egui::ComboBox::from_id_salt("ts_font_combo")
                        .selected_text(&dialog.font_name)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            // First-letter type-ahead: a typed letter jumps
                            // the selection to the next matching font.
                            let mut ch: Option<char> = None;
                            ui.input_mut(|i| {
                                i.events.retain(|e| match e {
                                    egui::Event::Text(t) => {
                                        if let Some(c) = t.chars().next_back() {
                                            ch = Some(c);
                                        }
                                        false
                                    }
                                    _ => true,
                                });
                            });
                            if let (Some(c), false) = (ch, font_choices.is_empty()) {
                                let lc = c.to_ascii_lowercase();
                                let n = font_choices.len();
                                let cur = font_choices
                                    .iter()
                                    .position(|f| f == &dialog.font_name)
                                    .unwrap_or(0);
                                for k in 1..=n {
                                    let idx = (cur + k) % n;
                                    if font_choices[idx].to_lowercase().starts_with(lc) {
                                        dialog.font_name = font_choices[idx].clone();
                                        break;
                                    }
                                }
                            }
                            egui::ScrollArea::vertical()
                                .max_height(280.0)
                                .show(ui, |ui| {
                                    for f in &font_choices {
                                        let r = ui.selectable_value(
                                            &mut dialog.font_name,
                                            f.clone(),
                                            f,
                                        );
                                        if *f == dialog.font_name {
                                            r.scroll_to_me(Some(egui::Align::Center));
                                        }
                                    }
                                });
                        });
                    ui.end_row();

                    ui.label("Default height");
                    ui.add(
                        egui::DragValue::new(&mut dialog.default_height)
                            .update_while_editing(false)
                            .speed(0.05)
                            .range(0.0..=1000.0)
                            .min_decimals(3)
                            .max_decimals(3),
                    );
                    ui.end_row();

                    ui.label("Color");
                    ui.horizontal(|ui| {
                        let mut by_layer = dialog.color_aci.is_none();
                        if ui.checkbox(&mut by_layer, "ByLayer").changed() {
                            dialog.color_aci = if by_layer { None } else { Some(7) };
                        }
                        ui.add_enabled_ui(!by_layer, |ui| {
                            let mut aci = dialog.color_aci.unwrap_or(7);
                            if ui
                                .add(
                                    egui::DragValue::new(&mut aci)
                                        .update_while_editing(false)
                                        .speed(1.0)
                                        .range(1..=255)
                                        .prefix("ACI "),
                                )
                                .changed()
                            {
                                dialog.color_aci = Some(aci);
                            }
                            // Swatch preview.
                            let (r, g, b) = aci_palette(aci);
                            let rect = ui
                                .allocate_exact_size(egui::vec2(28.0, 18.0), egui::Sense::hover())
                                .0;
                            ui.painter()
                                .rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
                            ui.painter().rect_stroke(
                                rect,
                                2.0,
                                egui::Stroke::new(0.7, egui::Color32::from_rgb(70, 80, 95)),
                            );
                        });
                    });
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        do_cancel = true;
                    }
                    if ui.button("OK").clicked() {
                        do_ok = true;
                    }
                });
            });
        });
        // Handle close-via-X (open went false) as cancel.
        if !open || do_cancel {
            self.history.push("  text style: cancelled".into());
            return;
        }
        if do_ok {
            let name = dialog.name.trim().to_string();
            if name.is_empty() {
                self.history
                    .push("  ! text style: name cannot be empty".into());
                // Re-open with the edits preserved so the user can fix.
                self.text_style_dialog = Some(dialog);
                return;
            }
            // Reject duplicate name when CREATING; allow when editing
            // (the existing id may already own that name).
            if let Some(found) = self.doc.text_styles.find(&name) {
                if dialog.editing_id != Some(found) {
                    self.history
                        .push(format!("  ! text style: '{}' already exists", name));
                    self.text_style_dialog = Some(dialog);
                    return;
                }
            }
            let new_style = cad_kernel::TextStyle {
                name: name.clone(),
                font_name: dialog.font_name.clone(),
                width_factor: 1.0,
                oblique: 0.0,
                default_height: dialog.default_height,
                bold: false,
                outline_only: false,
                outline_width: 0.0,
                underline: false,
            };
            match dialog.editing_id {
                Some(id) => {
                    if let Some(s) = self.doc.text_styles.styles.get_mut(id as usize) {
                        *s = new_style;
                        self.history
                            .push(format!("  ⊛ text style #{} '{}' updated", id, name));
                    }
                }
                None => {
                    let id = self.doc.text_styles.add(new_style);
                    self.history.push(format!(
                        "  + text style #{} '{}' created  (font={}, h={})",
                        id, name, dialog.font_name, dialog.default_height
                    ));
                }
            }
            // Color: stored only in the dialog for v1 — the kernel
            // TextStyle has no color field yet. Document the intent so
            // the next slice (when the field lands) can flip it on.
            if let Some(aci) = dialog.color_aci {
                self.history.push(format!(
                    "  (color ACI {} selected — kernel field pending; not persisted yet)",
                    aci
                ));
            }
            return;
        }
        // Window still open — preserve dialog state for next frame.
        self.text_style_dialog = Some(dialog);
    }

    /// Dimension Style Manager — the `dimstyle` page. Styles list + a
    /// live preview (our OWN sample drawing) + Set Current / New… /
    /// Modify… / Override… / Compare…. New…/Modify… launch the
    /// `DimStyleDialog` add/edit sub-form (`render_dim_style_dialog`).
    /// Wall Style Manager — the `wallstyle` page. Styles list + preview +
    /// Set Current / New / Modify. New/Modify launch `WallStyleDialog`.
    pub(super) fn render_wall_style_manager(&mut self, ctx: &egui::Context) {
        if !self.wall_style_manager_open {
            return;
        }
        let mut open = true;
        let mut do_close = false;
        let mut do_new = false;
        let mut do_modify = false;
        let mut do_set_current = false;
        let mut new_sel: Option<u32> = None;

        let styles: Vec<(u32, String)> = self
            .doc
            .wall_styles
            .styles
            .iter()
            .enumerate()
            .map(|(i, s)| (i as u32, s.name.clone()))
            .collect();
        let max_id = styles.len().saturating_sub(1) as u32;
        let sel = self.wall_style_manager_sel.min(max_id);
        let current = self.current_wall_style.min(max_id);
        let name_of = |id: u32| {
            styles
                .get(id as usize)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "STANDARD".into())
        };
        let cur_name = name_of(current);
        let sel_name = name_of(sel);
        let sel_style = self
            .doc
            .wall_styles
            .get(sel)
            .cloned()
            .unwrap_or_else(cad_kernel::WallStyle::standard);
        // Selected style's centerline linetype pattern (app-layer), for the preview.
        let sel_cl_pat: Option<Vec<f32>> = self
            .wall_centerline_ltype
            .get(&sel)
            .and_then(|id| self.doc.linetypes.get(*id))
            .map(|lt| lt.pattern.clone());

        egui::Window::new("Wall Style Manager")
            .id(egui::Id::new("wall_style_manager"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .movable(true)
            .default_size(egui::vec2(640.0, 380.0))
            .default_pos(egui::pos2(160.0, 80.0))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("Current wall style:  {}", cur_name)).strong(),
                );
                ui.add_space(6.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.label("Styles:");
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_width(150.0);
                            egui::ScrollArea::vertical()
                                .max_height(240.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_min_height(240.0);
                                    for (id, nm) in &styles {
                                        let txt = if *id == current {
                                            format!("✔  {}", nm)
                                        } else {
                                            format!("     {}", nm)
                                        };
                                        let r = ui.selectable_label(*id == sel, txt);
                                        if r.clicked() {
                                            new_sel = Some(*id);
                                        }
                                        if r.double_clicked() {
                                            new_sel = Some(*id);
                                            do_set_current = true;
                                        }
                                    }
                                });
                        });
                    });
                    ui.vertical(|ui| {
                        ui.label(format!("Preview of:  {}", sel_name));
                        let (resp, painter) =
                            ui.allocate_painter(egui::vec2(300.0, 240.0), egui::Sense::hover());
                        painter.rect_filled(resp.rect, 4.0, egui::Color32::from_rgb(40, 42, 47));
                        painter.rect_stroke(
                            resp.rect,
                            4.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 95)),
                        );
                        draw_wall_style_preview(
                            &painter,
                            resp.rect,
                            &sel_style,
                            sel_cl_pat.as_deref(),
                        );
                    });
                    ui.vertical(|ui| {
                        ui.add_space(18.0);
                        let bw = 96.0;
                        if ui
                            .add_sized([bw, 24.0], egui::Button::new("Set Current"))
                            .clicked()
                        {
                            do_set_current = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add_sized([bw, 24.0], egui::Button::new("New…"))
                            .clicked()
                        {
                            do_new = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add_sized([bw, 24.0], egui::Button::new("Modify…"))
                            .clicked()
                        {
                            do_modify = true;
                        }
                    });
                });
                ui.add_space(8.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(560.0);
                    ui.set_min_height(30.0);
                    ui.label(format!(
                        "{}  —  thickness {:.3} · fill {} · faces {}{}",
                        sel_name,
                        sel_style.thickness,
                        if sel_style.fill_color == 0 {
                            "none".to_string()
                        } else {
                            format!("ACI {}", sel_style.fill_color)
                        },
                        if sel_style.face_color == 0 {
                            "ByLayer".to_string()
                        } else {
                            format!("ACI {}", sel_style.face_color)
                        },
                        if sel_style.description.is_empty() {
                            String::new()
                        } else {
                            format!("  ·  {}", sel_style.description)
                        }
                    ));
                });
                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            do_close = true;
                        }
                    });
                });
            });

        if let Some(id) = new_sel {
            self.wall_style_manager_sel = id;
        }
        let target = new_sel.unwrap_or(sel);
        if do_set_current {
            self.current_wall_style = target;
            if let Some(s) = self.doc.wall_styles.get(target) {
                self.env.WlThk = s.thickness;
                let _ = self.env.save();
            }
            self.history
                .push(format!("  wall style: '{}' set current", name_of(target)));
        }
        if do_new {
            self.wall_style_dialog = Some(WallStyleDialog::new_blank());
        }
        if do_modify {
            let mut dlg = WallStyleDialog::from_existing(sel, &sel_style);
            dlg.centerline_ltype = self.wall_centerline_ltype.get(&sel).copied();
            self.wall_style_dialog = Some(dlg);
        }
        if !open || do_close {
            self.wall_style_manager_open = false;
        }
    }

    /// Add/Edit Wall Style sub-form (launched by the manager's New/Modify).
    pub(super) fn render_wall_style_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.wall_style_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut do_ok = false;
        let mut do_cancel = false;
        let mut pick_slot: Option<WallColorSlot> = None;
        // Snapshot linetypes for the centerline picker (avoids borrowing self.doc inside
        // the window closure).
        let lt_list: Vec<(u32, String)> = self
            .doc
            .linetypes
            .linetypes
            .iter()
            .enumerate()
            .map(|(i, lt)| (i as u32, lt.name.clone()))
            .collect();
        egui::Window::new(if dialog.editing_id.is_some() {
            "Edit Wall Style"
        } else {
            "New Wall Style"
        })
        .id(egui::Id::new("wall_style_dialog"))
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_size(egui::vec2(360.0, 240.0))
        .default_pos(egui::pos2(240.0, 130.0))
        .show(ctx, |ui| {
            ui.add_space(4.0);
            egui::Grid::new("wall_style_form")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.name)
                            .desired_width(220.0)
                            .hint_text("Dry Wall, Structural…"),
                    );
                    ui.end_row();
                    ui.label("Thickness");
                    ui.add(
                        egui::DragValue::new(&mut dialog.thickness)
                            .update_while_editing(false)
                            .speed(0.01)
                            .range(0.0..=1000.0)
                            .min_decimals(3)
                            .max_decimals(3),
                    );
                    ui.end_row();
                    ui.label("Fill (poché)");
                    ui.horizontal(|ui| {
                        if dim_color_swatch(ui, &mut dialog.fill_aci) {
                            pick_slot = Some(WallColorSlot::Fill);
                        }
                    });
                    ui.end_row();
                    ui.label("Face color");
                    ui.horizontal(|ui| {
                        if dim_color_swatch(ui, &mut dialog.face_aci) {
                            pick_slot = Some(WallColorSlot::Face);
                        }
                    });
                    ui.end_row();
                    ui.label("Insulation");
                    ui.checkbox(&mut dialog.insulation, "batt symbol in cavity")
                        .on_hover_text(
                            "Draw the architectural insulation \
                                            (sine-wave batt) symbol along the wall cavity.",
                        );
                    ui.end_row();
                    ui.label("Centerline");
                    egui::ComboBox::from_id_salt("wall_centerline_lt")
                        .width(220.0)
                        .selected_text(match dialog.centerline_ltype {
                            Some(id) => lt_list
                                .iter()
                                .find(|(i, _)| *i == id)
                                .map(|(_, n)| n.as_str())
                                .unwrap_or("?"),
                            None => "— none (faces only) —",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut dialog.centerline_ltype,
                                None,
                                "— none (faces only) —",
                            );
                            for (id, name) in &lt_list {
                                ui.selectable_value(&mut dialog.centerline_ltype, Some(*id), name);
                            }
                        });
                    ui.end_row();
                    ui.label("Description");
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.description).desired_width(220.0),
                    );
                    ui.end_row();
                });
            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        do_cancel = true;
                    }
                    if ui.button("OK").clicked() {
                        do_ok = true;
                    }
                });
            });
        });
        if !open || do_cancel {
            self.history.push("  wall style: cancelled".into());
            return;
        }
        if do_ok {
            let name = dialog.name.trim().to_string();
            if name.is_empty() {
                self.history
                    .push("  ! wall style: name cannot be empty".into());
                self.wall_style_dialog = Some(dialog);
                return;
            }
            if let Some(found) = self.doc.wall_styles.find(&name) {
                if dialog.editing_id != Some(found) {
                    self.history
                        .push(format!("  ! wall style: '{}' already exists", name));
                    self.wall_style_dialog = Some(dialog);
                    return;
                }
            }
            let new_style = cad_kernel::WallStyle {
                name: name.clone(),
                thickness: dialog.thickness,
                fill_color: dialog.fill_aci.map(|a| a as u32).unwrap_or(0),
                face_color: dialog.face_aci.map(|a| a as u32).unwrap_or(0),
                insulation: dialog.insulation,
                description: dialog.description.clone(),
            };
            let saved_id = match dialog.editing_id {
                Some(id) => {
                    if let Some(s) = self.doc.wall_styles.styles.get_mut(id as usize) {
                        *s = new_style;
                        self.history
                            .push(format!("  ⊛ wall style #{} '{}' updated", id, name));
                    }
                    id
                }
                None => {
                    let id = self.doc.wall_styles.add(new_style);
                    self.history
                        .push(format!("  + wall style #{} '{}' created", id, name));
                    id
                }
            };
            // App-layer centerline linetype (cad_kernel WallStyle untouched).
            match dialog.centerline_ltype {
                Some(lt) => {
                    self.wall_centerline_ltype.insert(saved_id, lt);
                }
                None => {
                    self.wall_centerline_ltype.remove(&saved_id);
                }
            }
            self.touch_view();
            return;
        }
        self.wall_style_dialog = Some(dialog);
        if let Some(slot) = pick_slot {
            self.aci_pick_request = Some(AciPickRequest::WallStyleForm(slot));
        }
    }

    pub(super) fn render_dim_style_manager(&mut self, ctx: &egui::Context) {
        if !self.dim_style_manager_open {
            return;
        }
        let mut open = true;
        let mut do_close = false;
        let mut do_new = false;
        let mut do_modify = false;
        let mut do_set_current = false;
        let mut do_override = false;
        let mut do_compare = false;
        let mut do_help = false;
        let mut new_sel: Option<u32> = None;

        // Snapshot the table so the Window closure doesn't borrow self.doc.
        let styles: Vec<(u32, String)> = self
            .doc
            .dim_styles
            .styles
            .iter()
            .enumerate()
            .map(|(i, s)| (i as u32, s.name.clone()))
            .collect();
        let max_id = styles.len().saturating_sub(1) as u32;
        let sel = self.dim_style_manager_sel.min(max_id);
        let current = self.current_dim_style.min(max_id);
        let name_of = |id: u32| {
            styles
                .get(id as usize)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "STANDARD".into())
        };
        let cur_name = name_of(current);
        let sel_name = name_of(sel);
        let sel_style = self
            .doc
            .dim_styles
            .get(sel)
            .cloned()
            .unwrap_or_else(cad_kernel::DimStyle::standard);

        egui::Window::new("Dimension Style Manager")
            .id(egui::Id::new("dim_style_manager"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .movable(true)
            .default_size(egui::vec2(720.0, 470.0))
            .default_pos(egui::pos2(160.0, 70.0))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!("Current dimension style:  {}", cur_name)).strong(),
                );
                ui.add_space(6.0);
                ui.horizontal_top(|ui| {
                    // ---- Styles list ----
                    ui.vertical(|ui| {
                        ui.label("Styles:");
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_width(150.0);
                            egui::ScrollArea::vertical()
                                .max_height(258.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_min_height(258.0);
                                    for (id, nm) in &styles {
                                        let txt = if *id == current {
                                            format!("✔  {}", nm)
                                        } else {
                                            format!("     {}", nm)
                                        };
                                        let r = ui.selectable_label(*id == sel, txt);
                                        if r.clicked() {
                                            new_sel = Some(*id);
                                        }
                                        if r.double_clicked() {
                                            new_sel = Some(*id);
                                            do_set_current = true;
                                        }
                                    }
                                });
                        });
                    });
                    // ---- Preview (our own sample drawing) ----
                    ui.vertical(|ui| {
                        ui.label(format!("Preview of:  {}", sel_name));
                        let (resp, painter) =
                            ui.allocate_painter(egui::vec2(360.0, 284.0), egui::Sense::hover());
                        painter.rect_filled(resp.rect, 4.0, egui::Color32::from_rgb(40, 42, 47));
                        painter.rect_stroke(
                            resp.rect,
                            4.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 95)),
                        );
                        draw_dim_style_preview(&painter, resp.rect, &sel_style);
                    });
                    // ---- Buttons ----
                    ui.vertical(|ui| {
                        ui.add_space(18.0);
                        let bw = 96.0;
                        if ui
                            .add_sized([bw, 24.0], egui::Button::new("Set Current"))
                            .clicked()
                        {
                            do_set_current = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add_sized([bw, 24.0], egui::Button::new("New…"))
                            .clicked()
                        {
                            do_new = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add_sized([bw, 24.0], egui::Button::new("Modify…"))
                            .clicked()
                        {
                            do_modify = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add_sized([bw, 24.0], egui::Button::new("Override…"))
                            .clicked()
                        {
                            do_override = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add_sized([bw, 24.0], egui::Button::new("Compare…"))
                            .clicked()
                        {
                            do_compare = true;
                        }
                    });
                });
                ui.add_space(8.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.label("List:");
                        egui::ComboBox::from_id_salt("dim_mgr_list")
                            .selected_text("All styles")
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                let _ = ui.selectable_label(true, "All styles");
                                let _ = ui.selectable_label(false, "Styles in use");
                            });
                        let mut xref = false;
                        ui.add_enabled(
                            false,
                            egui::Checkbox::new(&mut xref, "Don't list styles in Xrefs"),
                        );
                    });
                    ui.add_space(14.0);
                    ui.vertical(|ui| {
                        ui.label("Description");
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_width(330.0);
                            ui.set_min_height(46.0);
                            ui.label(format!(
                                "{}\narrow {:.3} · text {:.3} · {} dp · {}",
                                sel_name,
                                sel_style.arrow_size,
                                sel_style.text_height,
                                sel_style.decimal_places,
                                if sel_style.color_dim_line == 0 {
                                    "color ByBlock".to_string()
                                } else {
                                    format!("color ACI {}", sel_style.color_dim_line)
                                }
                            ));
                        });
                    });
                });
                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            do_close = true;
                        }
                        if ui.button("Help").clicked() {
                            do_help = true;
                        }
                    });
                });
            });

        // ---- apply actions (closure done; free to touch self) ----
        if let Some(id) = new_sel {
            self.dim_style_manager_sel = id;
        }
        let target = new_sel.unwrap_or(sel);
        if do_set_current {
            self.current_dim_style = target;
            self.history.push(format!(
                "  dim style: '{}' set current — new dims use it",
                name_of(target)
            ));
        }
        if do_new {
            self.dim_style_dialog = Some(DimStyleDialog::new_blank());
        }
        if do_modify {
            self.dim_style_dialog = Some(DimStyleDialog::from_existing(sel, &sel_style));
        }
        if do_override {
            self.history
                .push("  dim style: Override… not wired yet — use Modify… for now".into());
        }
        if do_compare {
            self.history
                .push("  dim style: Compare… not wired yet".into());
        }
        if do_help {
            self.history.push(
                "  Dimension Style Manager — pick a style, Set Current to make new dims use it; New…/Modify… edit. Preview reflects the selected style.".into());
        }
        if !open || do_close {
            self.dim_style_manager_open = false;
        }
    }

    /// Add/Edit Dim Style dialog — parallel to `render_text_style_dialog`.
    /// On OK the full DimStyle is built by cloning the source style
    /// (STANDARD for new, the edited style for edit) and patching only
    /// the four exposed fields + color, so the remaining ~65 DIMVARs are
    /// preserved untouched.
    pub(super) fn render_dim_style_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dim_style_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut do_ok = false;
        let mut do_cancel = false;
        let mut pick_slot: Option<DimColorSlot> = None;
        egui::Window::new(if dialog.editing_id.is_some() {
            "Edit Dim Style"
        } else {
            "New Dim Style"
        })
        .id(egui::Id::new("dim_style_dialog"))
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_size(egui::vec2(360.0, 280.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.add_space(4.0);
            egui::Grid::new("dim_style_form")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.name)
                            .desired_width(220.0)
                            .hint_text("STANDARD, MM, ARCH…"),
                    );
                    ui.end_row();

                    // ---- Lines & Arrows ------------------------------
                    ui.label(egui::RichText::new("Lines & Arrows").strong());
                    ui.end_row();

                    ui.label("Arrow size");
                    ui.add(
                        egui::DragValue::new(&mut dialog.arrow_size)
                            .update_while_editing(false)
                            .speed(0.01)
                            .range(0.0..=1000.0)
                            .min_decimals(3)
                            .max_decimals(3),
                    );
                    ui.end_row();

                    ui.label("Arrow type");
                    egui::ComboBox::from_id_salt("dim_arrow_kind")
                        .selected_text(match dialog.arrow_kind {
                            ArrowKind::Filled => "Filled",
                            ArrowKind::Hollow => "Hollow",
                            ArrowKind::Tick => "Architectural tick",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut dialog.arrow_kind,
                                ArrowKind::Filled,
                                "Filled",
                            );
                            ui.selectable_value(
                                &mut dialog.arrow_kind,
                                ArrowKind::Hollow,
                                "Hollow",
                            );
                            ui.selectable_value(
                                &mut dialog.arrow_kind,
                                ArrowKind::Tick,
                                "Architectural tick",
                            );
                        });
                    ui.end_row();

                    ui.label("Dim line color");
                    ui.horizontal(|ui| {
                        if dim_color_swatch(ui, &mut dialog.color_aci) {
                            pick_slot = Some(DimColorSlot::DimLine);
                        }
                    });
                    ui.end_row();

                    ui.label("Ext line color");
                    ui.horizontal(|ui| {
                        if dim_color_swatch(ui, &mut dialog.ext_color_aci) {
                            pick_slot = Some(DimColorSlot::ExtLine);
                        }
                    });
                    ui.end_row();

                    // ---- Text ----------------------------------------
                    ui.label(egui::RichText::new("Text").strong());
                    ui.end_row();

                    ui.label("Text height");
                    ui.add(
                        egui::DragValue::new(&mut dialog.text_height)
                            .update_while_editing(false)
                            .speed(0.01)
                            .range(0.0..=1000.0)
                            .min_decimals(3)
                            .max_decimals(3),
                    );
                    ui.end_row();

                    ui.label("Text color");
                    ui.horizontal(|ui| {
                        if dim_color_swatch(ui, &mut dialog.text_color_aci) {
                            pick_slot = Some(DimColorSlot::Text);
                        }
                    });
                    ui.end_row();

                    ui.label("Text placement");
                    egui::ComboBox::from_id_salt("dim_text_vpos")
                        .selected_text(match dialog.text_vert_pos {
                            0 => "Centered (on line)",
                            4 => "Below line",
                            _ => "Above line",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut dialog.text_vert_pos, 1, "Above line");
                            ui.selectable_value(&mut dialog.text_vert_pos, 0, "Centered (on line)");
                            ui.selectable_value(&mut dialog.text_vert_pos, 4, "Below line");
                        });
                    ui.end_row();

                    ui.label("Text alignment");
                    ui.checkbox(&mut dialog.text_aligned, "Align with dimension line");
                    ui.end_row();

                    // ---- Units ---------------------------------------
                    ui.label(egui::RichText::new("Units").strong());
                    ui.end_row();

                    ui.label("Decimal places");
                    ui.add(
                        egui::DragValue::new(&mut dialog.decimal_places)
                            .update_while_editing(false)
                            .speed(1.0)
                            .range(0..=12),
                    );
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        do_cancel = true;
                    }
                    if ui.button("OK").clicked() {
                        do_ok = true;
                    }
                });
            });
        });
        // Close-via-X (open went false) is treated as cancel.
        if !open || do_cancel {
            self.history.push("  dim style: cancelled".into());
            return;
        }
        if do_ok {
            let name = dialog.name.trim().to_string();
            if name.is_empty() {
                self.history
                    .push("  ! dim style: name cannot be empty".into());
                self.dim_style_dialog = Some(dialog);
                return;
            }
            // Reject duplicate name when CREATING; allow when editing
            // the same id (it already owns that name).
            if let Some(found) = self.doc.dim_styles.find(&name) {
                if dialog.editing_id != Some(found) {
                    self.history
                        .push(format!("  ! dim style: '{}' already exists", name));
                    self.dim_style_dialog = Some(dialog);
                    return;
                }
            }
            // Build by cloning the source style and patching the exposed
            // fields, so the other ~65 DIMVARs survive untouched.
            let mut new_style = match dialog.editing_id {
                Some(id) => self
                    .doc
                    .dim_styles
                    .get(id)
                    .cloned()
                    .unwrap_or_else(cad_kernel::DimStyle::standard),
                None => cad_kernel::DimStyle::standard(),
            };
            new_style.name = name.clone();
            new_style.arrow_size = dialog.arrow_size;
            new_style.text_height = dialog.text_height;
            new_style.decimal_places = dialog.decimal_places;
            // Per-element colors. None = ByBlock (0); the renderer falls
            // back to the dobject color when a field is 0.
            let to_col = |o: Option<u8>| o.map(|a| a as u32).unwrap_or(0);
            new_style.color_dim_line = to_col(dialog.color_aci);
            new_style.color_ext_line = to_col(dialog.ext_color_aci);
            new_style.color_text = to_col(dialog.text_color_aci);
            // Text placement (DIMTAD) + alignment (DIMTIH/DIMTOH).
            new_style.text_vert_pos = dialog.text_vert_pos;
            new_style.text_inside_horiz = !dialog.text_aligned;
            new_style.text_outside_horiz = !dialog.text_aligned;
            // Arrowhead kind → arrow_filled + tick_size.
            match dialog.arrow_kind {
                ArrowKind::Filled => {
                    new_style.arrow_filled = true;
                    new_style.tick_size = 0.0;
                }
                ArrowKind::Hollow => {
                    new_style.arrow_filled = false;
                    new_style.tick_size = 0.0;
                }
                ArrowKind::Tick => {
                    // Tick needs a positive size; default it to the arrow size.
                    if new_style.tick_size <= 0.0 {
                        new_style.tick_size = dialog.arrow_size;
                    }
                }
            }
            match dialog.editing_id {
                Some(id) => {
                    if let Some(s) = self.doc.dim_styles.styles.get_mut(id as usize) {
                        *s = new_style;
                        self.history
                            .push(format!("  ⊛ dim style #{} '{}' updated", id, name));
                    }
                }
                None => {
                    let id = self.doc.dim_styles.add(new_style);
                    self.history.push(format!(
                        "  + dim style #{} '{}' created  (arrow={}, txt={}, dec={})",
                        id, name, dialog.arrow_size, dialog.text_height, dialog.decimal_places
                    ));
                }
            }
            return;
        }
        // Window still open — preserve dialog state for next frame.
        self.dim_style_dialog = Some(dialog);
        // Defer opening the shared ACI wheel until the dialog is back in
        // place; the picker writes the chosen ACI into the right slot.
        if let Some(slot) = pick_slot {
            self.aci_pick_request = Some(AciPickRequest::DimStyleForm(slot));
        }
    }

    // ===================================================================
    //           Session Recorder helpers — Slice A wiring
    // ===================================================================

    /// Start a new recording. Snapshots the initial doc state so the
    /// timeline has an anchor.
    #[track_caller]
    pub fn dbg_start(&mut self) {
        self.dbg.start("user pressed Start");
        // Initial snapshot — anchor for "doc at session start".
        let loc = std::panic::Location::caller();
        let undo_d = self.undo_stack.len();
        let redo_d = self.redo_stack.len();
        self.dbg.take_snapshot(
            Self::plan_doc_of(self.factory.session.as_ref(), &self.doc),
            "session start",
            undo_d,
            redo_d,
            describe_verbose,
            loc,
        );
        // …and the 3D side, which the doc snapshot cannot see: it captures `doc`, and the whole
        // model, its materials and its camera live in `factory`. Without this a dump of a
        // rendering bug opens with a 2D line list and never mentions the thing being rendered.
        let scene = self.factory_scene_capture("session start");
        self.dbg.push(scene, loc);
        self.history.push(format!(
            "  🛰 recording started — every action will be captured"
        ));
    }

    /// Stop the recording. Returns nothing — output is read via the
    /// "📋 Copy" button in the Recorder window.
    #[track_caller]
    pub fn dbg_stop(&mut self) {
        // Bracket the session: the closing 3D state, so a dump shows what the recorded actions
        // actually did to the scene rather than only that they ran.
        if self.dbg.recording {
            let scene = self.factory_scene_capture("session end");
            self.dbg.push(scene, std::panic::Location::caller());
        }
        self.dbg.stop("user pressed Stop");
        self.history.push(format!(
            "  🛰 recording stopped — {} events, {} snapshots",
            self.dbg.events.len(),
            self.dbg.snapshots.len()
        ));
    }

    /// Build a `WatchedState` snapshot from current self. Cheap —
    /// the strings are short-format Debug prints.
    fn dbg_watched_now(&self) -> crate::dbg_recorder::WatchedState {
        use crate::dbg_recorder::{WatchedState, WindowFlags};
        WatchedState {
            active_view:        format!("{:?}", self.active_view),
            tool:               format!("{:?}", self.tool),
            select_mode:        format!("{:?}", self.select_mode),
            move_state:         format!("{:?}", self.move_state),
            copy_state:         format!("{:?}", self.copy_state),
            rotate_state:       format!("{:?}", self.rotate_state),
            scale_state:        format!("{:?}", self.scale_state),
            mirror_state:       format!("{:?}", self.mirror_state),
            trim_state:         format!("{:?}", self.trim_state),
            extend_state:       format!("{:?}", self.extend_state),
            fillet_state:       format!("{:?}", self.fillet_state),
            chamfer_state:      format!("{:?}", self.chamfer_state),
            offset_state:       format!("{:?}", self.offset_state),
            dist_state:         format!("{:?}", self.dist_state),
            area_state:         self.area_state.is_some(),
            layer_pick:         format!("{:?}", self.layer_pick),
            ptdist_state:       format!("{:?}", self.ptdist_state),
            text_draft:         format!("{:?}", self.text_draft),
            matchprops_state:   format!("{:?}", self.matchprops_state),
            align_state:        format!("{:?}", self.align_state),
            stretch_state:      format!("{:?}", self.stretch_state),
            break_state:        format!("{:?}", self.break_state),
            lengthen_state:     format!("{:?}", self.lengthen_state),
            block_def_state:    format!("{:?}", self.block_def_state),
            insert_state:       format!("{:?}", self.insert_state),
            block_pick_base:    self.block_dialog_pick_base,
            grip_drag:          self.grip_drag.is_some(),
            queued_op:          format!("{:?}", self.queued_op),
            armed_window_inside: format!("{:?}", self.armed_window_inside),
            window_first:       format!("{:?}", self.window_first),
            doc_dobjects_len:   self.doc.dobjects.len(),
            undo_depth:         self.undo_stack.len(),
            redo_depth:         self.redo_stack.len(),
            window_flags:       WindowFlags {
                cmd_window:        self.cmd_window_open,
                layers_window:     self.layers_window_open,
                pens_window:       self.pens_window_open,
                info_window:       self.info_window_open,
                dobjects_window:   self.dobjects_window_open,
                snap_window:       self.snap_window_open,
                trim_debug:        self.trim_debug_open,
                hatch_debug:       self.hatch_debug_open,
                hatch_dialog:      self.hatch_dialog_open,
                hatch_confirm:     self.hatch_confirm_open,
                text_style_dialog: self.text_style_dialog.is_some(),
                dim_style_dialog:  self.dim_style_dialog.is_some(),
                dbg_window:        self.dbg_window_open,
            },
            // Cherry-pick the SYSVARs most likely to flip user-visibly.
            // Add more here as bugs lead us to them.
            sysvar_summary: format!(
                "TxHt={} WlThk={} WlCnL={} OfsDis={} FltRad={} TrmMd={} EdgMod={} GrpEnb={} LodAnc={} UcsIcn={}",
                self.env.TxHt, self.env.WlThk, self.env.WlCnL,
                self.env.OfsDis, self.env.FltRad, self.env.TrmMd,
                self.env.EdgMod, self.env.GrpEnb, self.env.LodAnc,
                self.env.UcsIcn),
        }
    }

    /// Promote any pending press-release pair into a
    /// `GestureClassification` event. Runs AFTER the click handlers in
    /// the canvas update block — so by the time this fires, the
    /// selection delta + window_first transitions are visible.
    #[track_caller]
    pub fn dbg_emit_pending_gesture(&mut self) {
        let Some(p) = self.dbg_pending_gesture.take() else {
            return;
        };
        if !self.dbg.recording {
            return;
        }
        // Classify the motion vector.
        let dx = p.release_screen.0 - p.press_screen.0;
        let dy = p.release_screen.1 - p.press_screen.1;
        let dir_h = if dx > 0.5 {
            "L→R"
        } else if dx < -0.5 {
            "R→L"
        } else {
            "vertical"
        };
        let dir_v = if dy > 0.5 {
            "↓"
        } else if dy < -0.5 {
            "↑"
        } else {
            "horizontal"
        };
        let motion_dir = if p.motion_px < 1.0 {
            "stationary".to_string()
        } else {
            format!("{} {}", dir_h, dir_v)
        };
        // Hit-test at release.
        let tol = 10.0 / self.scale as f64;
        let hit_at_release = self.nearest_entity_under(p.release_world, tol);
        // Selection delta.
        let selection_after = self.selection.clone();
        let sel_delta = selection_after.len() as i64 - p.selection_before.len() as i64;
        // Infer what the app actually did. Heuristic:
        //   - selection grew/shrank/changed → click_select or add_window_selection
        //   - selection unchanged + window_first set or cleared → 2-click corner capture
        //   - none of the above → NOOP
        let in_select_mode = self.select_mode != SelectMode::Off;
        let action_taken = if selection_after != p.selection_before {
            format!(
                "selection mutated {}{} ({} → {} items)",
                if sel_delta > 0 { "+" } else { "" },
                sel_delta,
                p.selection_before.len(),
                selection_after.len()
            )
        } else if self.window_first.is_some() {
            format!("window_first captured = {:?}", self.window_first)
        } else {
            "NOOP — neither selection nor window_first changed".into()
        };
        // Pointer-mode-idle (Tool::None, no select session, no edit phase)
        // now counts as an implicit select_mode for drag purposes — see
        // feedback_rust_cad_pointer_is_selector.
        let pointer_mode_idle = self.tool == Tool::None && !in_select_mode;
        let drag_window_available = in_select_mode || pointer_mode_idle;
        // Verdict — the punchline a reader can act on at a glance.
        let verdict = if p.motion_px > 5.0
            && selection_after == p.selection_before
            && self.window_first.is_some()
            && drag_window_available
        {
            format!(
                "⚠ {}-px drag DEMOTED TO CLICK. Press position discarded; \
                 only the release became window_first. Drag-window intent LOST.",
                p.motion_px as i32
            )
        } else if p.motion_px > 5.0
            && selection_after == p.selection_before
            && !drag_window_available
        {
            format!(
                "⚠ {}-px motion produced NO selection change. \
                 (Drag-window not available in the current phase.)",
                p.motion_px as i32
            )
        } else if p.motion_px <= 1.0 && hit_at_release.is_none() {
            "click on empty area".into()
        } else if p.motion_px <= 1.0 && hit_at_release.is_some() {
            format!("click on dobject #{}", hit_at_release.unwrap())
        } else if selection_after != p.selection_before {
            format!("OK — selection updated by {} item(s)", sel_delta.abs())
        } else {
            "no change".into()
        };
        // What egui thought.
        let egui_clicked = false; // by definition; the press/release path doesn't see resp directly here
        let egui_drag_stopped = p.motion_px > 5.0;
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::GestureClassification {
                n_before: 0,
                n_after: 0, // stamped by push() from the real vec lens
                press_screen: p.press_screen,
                release_screen: p.release_screen,
                press_world: p.press_world,
                release_world: p.release_world,
                motion_px: p.motion_px,
                motion_dir,
                egui_clicked,
                egui_drag_stopped,
                hit_at_press: p.hit_at_press,
                hit_at_release,
                in_select_mode,
                active_tool: format!("{:?}", self.tool),
                selection_before: p.selection_before,
                selection_after,
                app_action_taken: action_taken,
                outcome_summary: verdict,
            }
        );
    }

    /// Frame-end poller — diff watched state vs last frame, emit one
    /// event per changed field. Agent-inspector behaviour: state
    /// transitions show up automatically, even ones we haven't yet
    /// instrumented at the source.
    #[track_caller]
    pub fn dbg_poll_state(&mut self) {
        if !self.dbg.recording {
            return;
        }
        let curr = self.dbg_watched_now();
        let loc = std::panic::Location::caller();
        if let Some(prev) = self.dbg_last_watched.take() {
            crate::dbg_recorder::diff_watched(&mut self.dbg, &prev, &curr, loc);
        }
        self.dbg_last_watched = Some(curr);
    }

    /// Auto-cadence snapshot — called from the per-frame update hook
    /// to spot when N events have accumulated since the last snap.
    /// Pulled out so the call site can attach the right location.
    #[track_caller]
    pub fn dbg_maybe_auto_snap(&mut self) {
        if self.dbg.want_auto_snap() {
            let undo_d = self.undo_stack.len();
            let redo_d = self.redo_stack.len();
            self.dbg.take_snapshot(
                Self::plan_doc_of(self.factory.session.as_ref(), &self.doc),
                "auto cadence",
                undo_d,
                redo_d,
                describe_verbose,
                std::panic::Location::caller(),
            );
        }
    }

    /// Discoverable popup for entering the text body after a Text-tool
    /// click. Without this, body capture happens silently in the cmd
    /// line and new users can't find it. Positioned near the canvas
    /// anchor so the dialog reads as "type your text HERE". The cmd-
    /// line path stays alive in parallel for power users — but only
    /// when this dialog is closed.
    pub(super) fn render_text_input_dialog(&mut self, ctx: &egui::Context) {
        if !self.text_input_dialog_open {
            return;
        }
        // Auto-select-newest: if "+ New…" armed the count sentinel and
        // a new style has since been added, pick it. Disarm whether the
        // count grew or not (Cancel from the style dialog leaves count
        // unchanged) so we don't keep watching forever.
        if self.text_input_dialog_style_count_before != usize::MAX
            && self.text_style_dialog.is_none()
        {
            let now = self.doc.text_styles.len();
            if now > self.text_input_dialog_style_count_before {
                let new_id = (now - 1) as u32;
                self.text_input_dialog_style_id = new_id;
                self.seed_text_dialog_from_style(new_id);
            }
            self.text_input_dialog_style_count_before = usize::MAX;
        }
        // Dialog-template shell: rail header, collapsible, dockable R/L, ~276.
        let cfg = crate::dock::DockConfig {
            id: "text",
            title: "Text",
            badge: None,
            dock_region: crate::dock::DockRegion::Right,
            alt_region: Some(crate::dock::DockRegion::Left),
            any_edge: false,
            strip_h: 0.0,
            dockable: false, // floating-only (owner: Inspector docks)
            rail_header: true,
            collapsible: true,
            size: 276.0,
            min: 276.0,
            max: 340.0,
            resizable: false,
            flush_body: true,
            float_w: 276.0,
            float_max_h_frac: 0.9,
        };
        let mut state = self.text_dialog_dock_state;
        let mut open = true;
        // Dialogue-box height RULE: cap the body at (canvas height − 300px); if the
        // content is taller, it scrolls. Keeps a tall dialog from overrunning the
        // canvas. `canvas_screen_rect` is the drawing area; fall back to the window.
        let canvas_h = self
            .canvas_screen_rect
            .map(|r| r.height())
            .unwrap_or_else(|| ctx.screen_rect().height());
        let max_body_h = (canvas_h - 300.0).max(200.0);
        // DOCKED → drawn by `render_dock_columns`; here only the FLOATING case.
        if matches!(state, crate::dock::DockState::Floating(_)) {
            crate::dock::HOST.show(ctx, &cfg, &mut state, &mut open, |ui, _cap| {
                egui::Frame::none()
                    .inner_margin(egui::Margin {
                        left: 14.0,
                        right: 14.0,
                        top: 12.0,
                        bottom: 14.0,
                    })
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(max_body_h)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                self.text_dialog_body(ui);
                            });
                    });
            });
        }
        self.text_dialog_dock_state = state;
        // Grip → Close fully shuts the panel and ends the TEXT command.
        if !open {
            self.close_text_dialog(true);
        }
    }

    /// Seed the panel's Height + Oblique from a text style (on open / style
    /// pick / after "+ New…"). Height only overrides when the style forces one.
    pub(super) fn seed_text_dialog_from_style(&mut self, id: u32) {
        if let Some(s) = self.doc.text_styles.get(id) {
            if s.default_height > 1e-9 {
                self.text_input_dialog_height = s.default_height;
            }
            self.text_dialog_oblique_deg = s.oblique.to_degrees();
        }
    }

    /// Close the Text panel and (optionally) end the TEXT command.
    pub(super) fn close_text_dialog(&mut self, end_command: bool) {
        self.text_input_dialog_open = false;
        self.text_input_dialog_buf.clear();
        self.text_input_dialog_anchor = None;
        self.text_input_dialog_focus = false;
        if end_command {
            self.text_draft = TextDraftState::Off;
            self.tool = Tool::None;
            self.clear_prompt();
        }
    }

    /// Live command-pill prompt for the text anchor step. Follows the COMMAND-
    /// PILL RULE: current height/angle are plain NOTES; sub-commands are
    /// clickable `[ … ]` chips (Height/Angle always; Close once a text was
    /// placed, to end the multi-place session). Every chip word is handled by
    /// the text intercepts in `run_command`.
    pub(super) fn text_anchor_prompt(&self, placed: bool) -> String {
        let h = self.env.TxHt;
        let a = self.text_dialog_angle_deg;
        // Esc ends the session (after a placement) or cancels (before) — no
        // Close chip needed. Height/Angle are the only sub-commands.
        let verb = if placed {
            "click next anchor"
        } else {
            "click anchor position"
        };
        format!("text: {verb}   h={h}  a={a}\u{00B0}   [ Height / Angle ]")
    }

    /// Body of the dockable Text dialog: STYLE / PARAMETER / PARAGRAPH-TOOLS
    /// sections, then the Apply button, the "Write here" content box, and the
    /// Enter/Esc hint. Also drives Apply(Enter) / Esc keyboard handling.
    fn text_dialog_body(&mut self, ui: &mut egui::Ui) {
        use crate::theme::color as tc;
        // Snapshot style list once (owned, so the section closures can also
        // borrow &mut self without aliasing self.doc).
        let styles: Vec<(u32, String)> = self
            .doc
            .text_styles
            .styles
            .iter()
            .enumerate()
            .map(|(i, s)| (i as u32, s.name.clone()))
            .collect();
        let sid = self.text_input_dialog_style_id;
        let cur = self
            .doc
            .text_styles
            .get(sid)
            .cloned()
            .unwrap_or_else(cad_kernel::TextStyle::standard);

        // ── §1 STYLE ─────────────────────────────────────────────────────
        pp_section(ui, "text_style_sec", "STYLE", true, false, |ui| {
            self.text_style_section(ui, &styles, sid, &cur);
        });
        // ── §2 PARAMETER ─────────────────────────────────────────────────
        pp_section(ui, "text_param_sec", "PARAMETER", true, true, |ui| {
            self.text_parameter_section(ui, sid);
        });
        // ── §3 PARAGRAPH / TOOLS ─────────────────────────────────────────
        pp_section(ui, "text_para_sec", "PARAGRAPH / TOOLS", true, true, |ui| {
            self.text_paragraph_section(ui);
        });
        // Divider closes the paragraph section (before Apply).
        pp_divider(ui);

        // Content-box focus stored LAST frame — drives the Apply-enabled gate
        // (owner #6: Apply turns on as soon as you click the box). Refreshed after
        // the box renders below.
        let te_id = egui::Id::new("text_dialog_content_edit");
        let focus_key = egui::Id::new("text_dialog_content_focused");
        let was_focused = ui.data(|d| d.get_temp::<bool>(focus_key)).unwrap_or(false);

        // ── Apply — the ONLY commit (owner #3). Enter / Space / Shift+Enter just
        //    edit text; nothing but this button places the text. Enabled once an
        //    anchor is set AND the box is focused or already has text (owner #6).
        ui.add_space(12.0);
        let ready = self.text_input_dialog_anchor.is_some()
            && (was_focused || !self.text_input_dialog_buf.trim().is_empty());
        let aw = (ui.available_width() * 0.6).floor();
        let mut do_apply = false;
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - aw) * 0.5).max(0.0));
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(aw, 24.0), egui::Sense::click());
            let p = ui.painter_at(rect);
            let (fill, txt) = if ready {
                let f = if resp.hovered() {
                    tc::ACCENT
                } else {
                    tc::ACCENT.gamma_multiply(0.88)
                };
                (f, tc::ON_ACCENT)
            } else {
                (tc::SURFACE_2, tc::TEXT_DISABLED)
            };
            p.rect_filled(rect, 12.0, fill);
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Apply",
                crate::theme::typ::body_strong(),
                txt,
            );
            if ready && resp.clicked() {
                do_apply = true;
            }
            if ready && resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        });

        // ── Content box — a multi-line editor. SHIFT+ENTER inserts a new line
        //    (keep typing), plain ENTER = Apply, Space inserts a space. NOTE:
        //    egui 0.30's multiline TextEdit only inserts a newline for its
        //    `return_key` (default = plain Enter); Shift+Enter is otherwise
        //    IGNORED. We consume plain Enter for Apply (below) AND remap the
        //    return_key to Shift+Enter, so Shift+Enter makes the newline and
        //    plain Enter never inserts one.
        ui.add_space(10.0);
        let sf = ui.style().override_font_id.clone();
        let sw = ui.visuals().widgets.clone();
        {
            let vis = ui.visuals_mut();
            for st in [
                &mut vis.widgets.inactive,
                &mut vis.widgets.hovered,
                &mut vis.widgets.active,
            ] {
                st.bg_fill = tc::SURFACE_0;
                st.weak_bg_fill = tc::SURFACE_0;
                st.bg_stroke = egui::Stroke::new(1.0, tc::BORDER);
                st.fg_stroke.color = tc::TEXT_PRIMARY;
            }
            vis.extreme_bg_color = tc::SURFACE_0;
            vis.widgets.inactive.rounding = egui::Rounding::same(crate::theme::radius::SM);
        }
        // While the font popup is open, the write box must NOT keep keyboard
        // focus. The box's TextEdit renders BEFORE the popup, so if it stays
        // focused it swallows the letters the popup's first-letter type-ahead
        // needs (the intermittent "type-ahead sometimes doesn't work"). Surrender
        // focus so keystrokes reach the picker; the picker owns Enter here too.
        if ui.memory(|m| m.is_popup_open(egui::Id::new("text_font_popup"))) {
            ui.memory_mut(|m| m.surrender_focus(te_id));
        }
        // Enter (NO modifiers) = Apply — consume it BEFORE the TextEdit so it
        // never becomes text. Shift+Enter falls THROUGH to the editor's
        // `return_key` to make a new line.
        // ⚠ egui's `consume_key(NONE, …)` matches *logically* — a bare NONE
        // pattern also matches Shift+Enter (it only checks the pattern's required
        // modifiers are present, not that no extra ones are). So we guard on "no
        // modifiers held" ourselves, or Shift+Enter would apply too.
        // Fire Apply whenever a draft is active (`WaitingForString`), NOT only
        // when the box has focus: the box misses focus for the ONE frame right
        // after the anchor click, which made the FIRST Enter a no-op ("first
        // Enter doesn't apply"). Exclude the font popup (Enter commits a font).
        let drafting = matches!(self.text_draft, TextDraftState::WaitingForString(_));
        let font_popup = ui.memory(|m| m.is_popup_open(egui::Id::new("text_font_popup")));
        if (ui.memory(|m| m.has_focus(te_id)) || drafting)
            && !font_popup
            && ui.input(|i| !i.modifiers.any())
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            do_apply = true;
        }
        let edit = egui::TextEdit::multiline(&mut self.text_input_dialog_buf)
            .id(te_id)
            .desired_width(f32::INFINITY)
            .desired_rows(3)
            // Newline on SHIFT+Enter (not plain Enter — that's Apply).
            .return_key(egui::KeyboardShortcut::new(
                egui::Modifiers::SHIFT,
                egui::Key::Enter,
            ))
            .hint_text("Write here");
        let resp = ui.add_sized(egui::vec2(ui.available_width(), 72.0), edit);
        ui.visuals_mut().widgets = sw;
        ui.style_mut().override_font_id = sf;
        // Store this frame's focus for next frame's Apply gate. Read has_focus()
        // FIRST — egui's Context is one RwLock, so calling has_focus() (a read)
        // INSIDE data_mut() (a write) self-deadlocks the whole app (HANG).
        let content_focused = resp.has_focus();
        ui.data_mut(|d| d.insert_temp(focus_key, content_focused));
        if self.text_input_dialog_focus {
            resp.request_focus();
            self.text_input_dialog_focus = false;
        }
        // Owner feature: clicking into the write box RE-RUNS the text command —
        // re-arm the tool so the next canvas click sets an anchor (no need to
        // retype `text`). Only when it isn't already the active tool.
        if (resp.gained_focus() || resp.clicked()) && self.tool != Tool::Text {
            self.tool = Tool::Text;
            self.text_draft = TextDraftState::WaitingForPosition;
            let p = self.text_anchor_prompt(false);
            self.set_prompt(p);
        }

        // ── Hint ─────────────────────────────────────────────────────────
        ui.add_space(4.0);
        let (hr, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 14.0), egui::Sense::hover());
        ui.painter().text(
            hr.center(),
            egui::Align2::CENTER_CENTER,
            "Shift+Enter = new line   ·   Enter or Apply = place",
            crate::theme::typ::hint(),
            tc::TEXT_MUTED,
        );

        if do_apply {
            self.commit_text_dialog();
        }
    }

    /// §1 STYLE — two columns: three equal-width left dropdowns (Style / Font /
    /// Variant) + a right column of New / Set-current pills over a live preview
    /// box, bottom-aligned with the left column.
    fn text_style_section(
        &mut self,
        ui: &mut egui::Ui,
        styles: &[(u32, String)],
        sid: u32,
        cur: &cad_kernel::TextStyle,
    ) {
        use crate::theme::color as tc;
        let cw = ui.available_width();
        let gap = 8.0;
        let lw = (cw * 0.46).floor();
        let rw = cw - lw - gap;
        let col_h = 3.0 * PP_ROW_H + 2.0 * 8.0; // left column total height
        let cur_name = styles
            .iter()
            .find(|(i, _)| *i == sid)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "STANDARD".into());
        let mut pick_style: Option<u32> = None;
        let mut set_font: Option<String> = None;
        let mut do_new = false;
        let mut do_set_current = false;
        // Real installed fonts (from the cad_text engine) offered alongside the
        // two egui built-ins. Fetched once, before the UI closures capture `ui`.
        let sys_fonts = self.font_manager.borrow_mut().names().to_vec();
        // Font-picker keyboard state pulled into locals (nested egui closures
        // can't also borrow `self`). Written back after the layout closure.
        let font_popup_id = egui::Id::new("text_font_popup");
        let mut pf_highlight = self.font_picker_highlight;
        let mut pf_commit = false; // click a row → keep highlighted, close
        let mut pf_live: Option<String> = None; // highlighted font (apply live)
        let mut pf_open_init = false; // popup just opened this frame
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            // Left column — 3 equal boxes.
            ui.vertical(|ui| {
                ui.set_width(lw);
                // Style dropdown.
                let r = pp_dropdown_box(ui, lw, &cur_name, true);
                let id = egui::Id::new("text_style_popup");
                if r.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(id));
                }
                egui::popup_below_widget(
                    ui,
                    id,
                    &r,
                    egui::PopupCloseBehavior::CloseOnClick,
                    |ui| {
                        ui.set_min_width(lw.max(120.0));
                        for (i, n) in styles {
                            if ui.selectable_label(*i == sid, n).clicked() {
                                pick_style = Some(*i);
                            }
                        }
                    },
                );
                ui.add_space(8.0);
                // Font dropdown — keyboard-navigable (↑/↓ + type-to-filter) with
                // LIVE preview: the highlighted font is applied each frame so the
                // canvas ghost updates as you move through the list.
                let fr = pp_dropdown_box(ui, lw, &cur.font_name, true);
                let fid = font_popup_id;
                if fr.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(fid));
                    // Just opened → arm the picker at the current font.
                    if ui.memory(|m| m.is_popup_open(fid)) {
                        pf_open_init = true;
                    }
                }
                egui::popup_below_widget(
                    ui,
                    fid,
                    &fr,
                    egui::PopupCloseBehavior::CloseOnClickOutside,
                    |ui| {
                        ui.set_min_width(lw.max(180.0));
                        // Full list (built-ins + all system fonts). No search box:
                        // type a LETTER to jump to fonts starting with it.
                        let mut choices: Vec<String> = vec!["standard".into(), "monospace".into()];
                        choices.extend(sys_fonts.iter().cloned());
                        let n = choices.len();
                        // Keyboard, all CONSUMED so nothing leaks to the text box:
                        //   letter → jump to next font starting with it (cycles)
                        //   ↑/↓    → move one row
                        //   Enter  → commit + close
                        let mut kb = false;
                        // First-letter type-ahead: read the pressed LETTER via
                        // Event::Key. A focused TextEdit consumes Event::Text but
                        // NOT letter Key events, so this fires reliably regardless
                        // of what holds focus (the old Text-based path was racy).
                        // Jump to the next font whose name starts with the letter.
                        let typed: Option<char> = ui.input(|i| {
                            i.events.iter().rev().find_map(|e| match e {
                                egui::Event::Key {
                                    key,
                                    pressed: true,
                                    modifiers,
                                    ..
                                } if !modifiers.ctrl && !modifiers.command && !modifiers.alt => {
                                    let b = key.name().as_bytes();
                                    (b.len() == 1 && b[0].is_ascii_alphabetic())
                                        .then(|| (b[0] as char).to_ascii_lowercase())
                                }
                                _ => None,
                            })
                        });
                        if let Some(lc) = typed {
                            for k in 1..=n {
                                let idx = (pf_highlight + k) % n.max(1);
                                if choices[idx].to_lowercase().starts_with(lc) {
                                    pf_highlight = idx;
                                    kb = true;
                                    break;
                                }
                            }
                        }
                        ui.input_mut(|i| {
                            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                                pf_highlight += 1;
                                kb = true;
                            }
                            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                                pf_highlight = pf_highlight.saturating_sub(1);
                                kb = true;
                            }
                            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                                pf_commit = true;
                            }
                        });
                        if pf_highlight >= n {
                            pf_highlight = n.saturating_sub(1);
                        }
                        // Hover moves the highlight ONLY when the mouse actually
                        // moves — otherwise a still pointer sitting over the list
                        // would fight the arrow keys (kept snapping back).
                        let mouse_moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
                        egui::ScrollArea::vertical()
                            .max_height(280.0)
                            .show(ui, |ui| {
                                for (i, f) in choices.iter().enumerate() {
                                    let resp = ui.selectable_label(i == pf_highlight, f);
                                    if mouse_moved && resp.hovered() {
                                        pf_highlight = i;
                                    }
                                    if resp.clicked() {
                                        pf_highlight = i;
                                        pf_commit = true;
                                    }
                                    // Auto-scroll only on KEYBOARD nav so the mouse
                                    // isn't fought by scroll-to-highlight.
                                    if kb && i == pf_highlight {
                                        resp.scroll_to_me(Some(egui::Align::Center));
                                    }
                                }
                            });
                        pf_live = choices.get(pf_highlight).cloned();
                    },
                );
                ui.add_space(8.0);
                // Variant — Regular / Bold / Italic / Bold Italic, writing the
                // same style fields as the Row-4 Bold/Italic toggles.
                let cur_var = match (cur.bold, cur.oblique.abs() > 1e-6) {
                    (true, true) => "Bold Italic",
                    (true, false) => "Bold",
                    (false, true) => "Italic",
                    (false, false) => "Regular",
                };
                let mut set_variant: Option<(bool, f64)> = None;
                let vr = pp_dropdown_box(ui, lw, cur_var, true);
                let vid = egui::Id::new("text_variant_popup");
                if vr.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(vid));
                }
                egui::popup_below_widget(
                    ui,
                    vid,
                    &vr,
                    egui::PopupCloseBehavior::CloseOnClick,
                    |ui| {
                        ui.set_min_width(lw.max(120.0));
                        for (label, b, o) in [
                            ("Regular", false, 0.0),
                            ("Bold", true, 0.0),
                            ("Italic", false, ITALIC_RAD),
                            ("Bold Italic", true, ITALIC_RAD),
                        ] {
                            let active =
                                (cur.bold == b) && ((cur.oblique.abs() > 1e-6) == (o > 1e-6));
                            if ui.selectable_label(active, label).clicked() {
                                set_variant = Some((b, o));
                            }
                        }
                    },
                );
                if let Some((b, o)) = set_variant {
                    if let Some(s) = self.doc.text_styles.styles.get_mut(sid as usize) {
                        s.bold = b;
                        s.oblique = o;
                    }
                }
            });
            ui.add_space(gap);
            // Right column — preview on TOP, then the New / Set-current buttons
            // (their combined width + spacing spans the preview box width).
            ui.vertical(|ui| {
                ui.set_width(rw);
                let ph = (col_h - PP_ROW_H - 8.0).max(24.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(rw, ph), egui::Sense::hover());
                ui.painter().rect(
                    rect,
                    egui::Rounding::same(crate::theme::radius::SM),
                    tc::SURFACE_0,
                    egui::Stroke::new(1.0, tc::BORDER),
                );
                let p = ui.painter_at(rect);
                // Preview in the ACTUAL selected font (+ italic/width); fall back
                // to egui text when the engine has no fonts.
                if !self.draw_ttf_swatch(
                    &p,
                    rect,
                    "AaBbCc",
                    &cur.font_name,
                    cur.oblique,
                    cur.width_factor,
                    tc::TEXT_PRIMARY,
                ) {
                    let (letters, digits) = if cur.font_name == "monospace" {
                        (egui::FontId::monospace(15.0), egui::FontId::monospace(12.0))
                    } else {
                        (
                            egui::FontId::proportional(15.0),
                            egui::FontId::proportional(12.0),
                        )
                    };
                    p.text(
                        egui::pos2(rect.center().x, rect.center().y - 9.0),
                        egui::Align2::CENTER_CENTER,
                        "AaBbCc",
                        letters,
                        tc::TEXT_PRIMARY,
                    );
                    p.text(
                        egui::pos2(rect.center().x, rect.center().y + 9.0),
                        egui::Align2::CENTER_CENTER,
                        "0123456789",
                        digits,
                        tc::TEXT_MUTED,
                    );
                }
                ui.add_space(8.0);
                // Pills sized to their OWN labels (original), left-aligned.
                let neww = (ui.fonts(|f| {
                    f.layout_no_wrap("New".into(), crate::theme::typ::body(), tc::TEXT_PRIMARY)
                        .size()
                        .x
                }) + 18.0)
                    .ceil();
                let setw = (ui.fonts(|f| {
                    f.layout_no_wrap(
                        "Set current".into(),
                        crate::theme::typ::body(),
                        tc::TEXT_PRIMARY,
                    )
                    .size()
                    .x
                }) + 18.0)
                    .ceil();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                    if pp_pill(ui, neww, "New", true) {
                        do_new = true;
                    }
                    if pp_pill(ui, setw, "Set current", false) {
                        do_set_current = true;
                    }
                });
            });
        });
        if let Some(i) = pick_style {
            self.text_input_dialog_style_id = i;
            self.seed_text_dialog_from_style(i);
        }
        // ── Font-picker keyboard state: persist + live/commit/cancel ──────────
        if pf_open_init {
            // Arm at the current font: remember it (to restore on Esc) and put
            // the highlight on it.
            self.font_picker_orig = Some(cur.font_name.clone());
            let mut list: Vec<String> = vec!["standard".into(), "monospace".into()];
            list.extend(sys_fonts.iter().cloned());
            pf_highlight = list.iter().position(|f| f == &cur.font_name).unwrap_or(0);
        }
        self.font_picker_highlight = pf_highlight;
        let popup_open = ui.memory(|m| m.is_popup_open(font_popup_id));
        if popup_open {
            // LIVE preview: apply the highlighted font each frame so the canvas
            // ghost updates as the arrow keys move through the list.
            if let Some(f) = pf_live {
                set_font = Some(f);
            }
        }
        if pf_commit {
            ui.memory_mut(|m| m.close_popup());
            self.font_picker_orig = None;
        }
        if let Some(f) = set_font {
            if let Some(s) = self.doc.text_styles.styles.get_mut(sid as usize) {
                s.font_name = f;
            }
        }
        if do_new && self.text_style_dialog.is_none() {
            self.text_style_dialog = Some(TextStyleDialog::new_blank());
            self.text_input_dialog_style_count_before = self.doc.text_styles.len();
        }
        if do_set_current {
            if cur.default_height > 1e-9 {
                self.env.TxHt = cur.default_height;
                let _ = self.env.save();
            }
            self.history
                .push(format!("  text: '{}' set current", cur_name));
        }
    }

    /// §2 PARAMETER — Layer / Color spec bars, then Height / Line-spacing and
    /// Oblique / Tracking (each = a small icon + a value box, two per row).
    fn text_parameter_section(&mut self, ui: &mut egui::Ui, sid: u32) {
        use crate::theme::color as tc;
        let active = self.doc.layers.active;
        let lname = self
            .doc
            .layers
            .get(active)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| "0".into());
        let lcolor = self.resolve_color32(Color::ByLayer, active);
        let ccolor = self.resolve_color32(self.doc.current_color, active);
        let clabel = match self.doc.current_color {
            Color::ByLayer => "By Layer".to_string(),
            Color::ByBlock => "By Block".to_string(),
            Color::Aci(i) => format!("ACI {}", i),
            c @ Color::TrueColorRef(_) => {
                let cc = self.resolve_color32(c, active);
                format!("#{:02X}{:02X}{:02X}", cc.r(), cc.g(), cc.b())
            }
        };
        let layer_list: Vec<(u32, String)> = self
            .doc
            .layers
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| (i as u32, l.name.clone()))
            .collect();
        let mut set_active: Option<u32> = None;
        let mut open_color = false;
        let gap = 8.0;
        let col_w = ((ui.available_width() - gap) / 2.0).floor();
        let icon_w = 20.0;
        let box_w = col_w - icon_w - 6.0;

        // Row 1 — Layer | Color (functional spec bars, like Hatch).
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                let lr = hatch_spec_bar(ui, lcolor, &lname);
                let lid = egui::Id::new("text_layer_popup");
                if lr.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(lid));
                }
                egui::popup_below_widget(
                    ui,
                    lid,
                    &lr,
                    egui::PopupCloseBehavior::CloseOnClick,
                    |ui| {
                        ui.set_min_width(150.0);
                        for (i, n) in &layer_list {
                            if ui.selectable_label(*i == active, n).clicked() {
                                set_active = Some(*i);
                            }
                        }
                    },
                );
            });
            ui.add_space(gap);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                if hatch_spec_bar(ui, ccolor, &clabel).clicked() {
                    open_color = true;
                }
            });
        });
        ui.add_space(8.0);
        // Row 2 — Height | Line spacing (stub).
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                    let (ir, _) =
                        ui.allocate_exact_size(egui::vec2(icon_w, PP_ROW_H), egui::Sense::hover());
                    text_height_glyph(&ui.painter_at(ir), ir, tc::TEXT_MUTED);
                    let hr = hatch_num_box(
                        ui,
                        box_w,
                        &mut self.text_input_dialog_height,
                        0.05,
                        1e-3..=1e6,
                        "",
                        &self.calc,
                    );
                    if hr.changed() {
                        // Memorize as the text-height sysvar so it persists across
                        // placements and the next `text` run.
                        self.env.TxHt = self.text_input_dialog_height.max(1e-3);
                    }
                    if hr.drag_stopped() || hr.lost_focus() {
                        let _ = self.env.save();
                    }
                });
            });
            ui.add_space(gap);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                    let (ir, _) =
                        ui.allocate_exact_size(egui::vec2(icon_w, PP_ROW_H), egui::Sense::hover());
                    text_linespacing_glyph(&ui.painter_at(ir), ir, tc::TEXT_DISABLED);
                    pp_value_static(ui, box_w, "Auto", false)
                        .on_hover_text("Line spacing — arrives with multi-line (MTEXT)");
                });
            });
        });
        ui.add_space(8.0);
        // Row 3 — Rotation angle (per-placement, → Text.angle) | Tracking (stub).
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                    let (ir, _) =
                        ui.allocate_exact_size(egui::vec2(icon_w, PP_ROW_H), egui::Sense::hover());
                    text_oblique_glyph(&ui.painter_at(ir), ir, tc::TEXT_MUTED);
                    hatch_num_box(
                        ui,
                        box_w,
                        &mut self.text_dialog_angle_deg,
                        1.0,
                        -360.0..=360.0,
                        "°",
                        &self.calc,
                    )
                    .on_hover_text("Rotation angle for placed text");
                });
            });
            ui.add_space(gap);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                    let (ir, _) =
                        ui.allocate_exact_size(egui::vec2(icon_w, PP_ROW_H), egui::Sense::hover());
                    text_tracking_glyph(&ui.painter_at(ir), ir, tc::TEXT_DISABLED);
                    pp_value_static(ui, box_w, "Auto", false)
                        .on_hover_text("Letter spacing — not in the kernel yet");
                });
            });
        });
        ui.add_space(8.0);
        // Row 4 — Bold + Italic | Outline mode + pen width (edit the style).
        let (cur_bold, cur_italic, cur_outline, mut cur_owidth) = self
            .doc
            .text_styles
            .get(sid)
            .map(|s| {
                (
                    s.bold,
                    s.oblique.abs() > 1e-6,
                    s.outline_only,
                    s.outline_width,
                )
            })
            .unwrap_or((false, false, false, 0.0));
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                    if ui.selectable_label(cur_bold, " Bold ").clicked() {
                        if let Some(s) = self.doc.text_styles.styles.get_mut(sid as usize) {
                            s.bold = !cur_bold;
                        }
                    }
                    if ui.selectable_label(cur_italic, " Italic ").clicked() {
                        if let Some(s) = self.doc.text_styles.styles.get_mut(sid as usize) {
                            s.oblique = if cur_italic { 0.0 } else { ITALIC_RAD };
                        }
                    }
                });
            });
            ui.add_space(gap);
            ui.vertical(|ui| {
                ui.set_width(col_w);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                    if ui.selectable_label(cur_outline, "Outline").clicked() {
                        if let Some(s) = self.doc.text_styles.styles.get_mut(sid as usize) {
                            s.outline_only = !cur_outline;
                        }
                    }
                    let r = hatch_num_box(
                        ui,
                        box_w * 0.7,
                        &mut cur_owidth,
                        0.1,
                        0.0..=100.0,
                        "",
                        &self.calc,
                    )
                    .on_hover_text("Outline pen width (0 = hairline)");
                    if r.changed() {
                        if let Some(s) = self.doc.text_styles.styles.get_mut(sid as usize) {
                            s.outline_width = cur_owidth.max(0.0);
                        }
                    }
                });
            });
        });
        if let Some(id) = set_active {
            self.doc.layers.active = id;
        }
        if open_color {
            self.aci_pick_request = Some(AciPickRequest::CurrentColor);
        }
    }

    /// §3 PARAGRAPH / TOOLS — list + alignment + abc toggles (alignment L/C/R
    /// wired to `h_align`; the rest are UI stubs), then Spell-check / @ / find,
    /// then Edit-Dictionary. Everything but alignment is visibly disabled.
    fn text_paragraph_section(&mut self, ui: &mut egui::Ui) {
        let sz = 26.0;
        let ha = self.text_dialog_halign;
        let lm = self.text_list_mode;
        let mut set_ha: Option<cad_kernel::TextHAlign> = None;
        // Deferred (avoids borrowing self inside the closure): the click sets
        // this, we apply it after the closure.
        let mut set_list: Option<cad_kernel::TextListKind> = None;
        let mut toggle_underline = false;
        let sid = self.text_input_dialog_style_id;
        let cur_underline = self
            .doc
            .text_styles
            .get(sid)
            .map(|s| s.underline)
            .unwrap_or(false);
        // Row 1 — grouped toggles.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);
            // Lists — bulleted / numbered, mutually exclusive toggles. Clicking
            // the active one turns the list OFF. Applied as a per-line marker in
            // the ghost preview + at commit.
            let num_on = matches!(lm, cad_kernel::TextListKind::Numbered);
            let bul_on = matches!(lm, cad_kernel::TextListKind::Bulleted);
            if pp_glyph_btn(ui, sz, num_on, true, |p, r, c| glyph_list(p, r, c, false))
                .on_hover_text("Numbered list (1. 2. 3.)")
                .clicked()
            {
                set_list = Some(if num_on {
                    cad_kernel::TextListKind::None
                } else {
                    cad_kernel::TextListKind::Numbered
                });
            }
            if pp_glyph_btn(ui, sz, bul_on, true, |p, r, c| glyph_list(p, r, c, true))
                .on_hover_text("Bulleted list (•)")
                .clicked()
            {
                set_list = Some(if bul_on {
                    cad_kernel::TextListKind::None
                } else {
                    cad_kernel::TextListKind::Bulleted
                });
            }
            ui.add_space(6.0);
            // Alignment (L / C / R functional; justify stub).
            for (mode, al) in [
                (0u8, Some(cad_kernel::TextHAlign::Left)),
                (1, Some(cad_kernel::TextHAlign::Center)),
                (2, Some(cad_kernel::TextHAlign::Right)),
                (3, None),
            ] {
                let active = matches!(
                    (al, ha),
                    (
                        Some(cad_kernel::TextHAlign::Left),
                        cad_kernel::TextHAlign::Left
                    ) | (
                        Some(cad_kernel::TextHAlign::Center),
                        cad_kernel::TextHAlign::Center
                    ) | (
                        Some(cad_kernel::TextHAlign::Right),
                        cad_kernel::TextHAlign::Right
                    )
                );
                let enabled = al.is_some();
                let resp = pp_glyph_btn(ui, sz, active, enabled, move |p, r, c| {
                    glyph_align(p, r, c, mode)
                });
                if enabled && resp.clicked() {
                    set_ha = al;
                }
                if !enabled {
                    resp.on_hover_text("Justify — arrives with multi-line (MTEXT)");
                }
            }
            ui.add_space(6.0);
            // abc — underline toggle (edits the current style; snapshot onto
            // each placed Text at commit, like Bold).
            if pp_glyph_btn(ui, sz, cur_underline, true, glyph_abc)
                .on_hover_text("Underline")
                .clicked()
            {
                toggle_underline = true;
            }
        });
        ui.add_space(10.0);
        ui.small(
            egui::RichText::new(
                "Inline codes: \\C1;red\\C0; reset · \\H2x; taller · \\fArial; font · \\P new line",
            )
            .color(egui::Color32::from_rgb(150, 165, 185)),
        );
        ui.add_space(6.0);
        // Row 2 — Spell check checkbox … @ · find.
        ui.horizontal(|ui| {
            pp_check_stub(ui, "Spell Check");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                pp_glyph_btn(ui, sz, false, false, glyph_find)
                    .on_hover_text("Find in text — not in the kernel yet");
                pp_glyph_btn(ui, sz, false, false, |p, r, c| {
                    p.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        "@",
                        crate::theme::typ::body(),
                        c,
                    );
                })
                .on_hover_text("Insert field / symbol — not in the kernel yet");
            });
        });
        ui.add_space(8.0);
        // Row 3 — Edit Dictionary.
        pp_check_stub(ui, "Edit Dictionary");
        if let Some(al) = set_ha {
            self.text_dialog_halign = al;
        }
        if let Some(m) = set_list {
            self.text_list_mode = m;
        }
        if toggle_underline {
            if let Some(s) = self.doc.text_styles.styles.get_mut(sid as usize) {
                s.underline = !cur_underline;
            }
        }
    }

    /// Font a text STYLE resolves to (a style may name a font the engine
    /// doesn't know; resolution falls back to "standard").
    pub(super) fn resolve_style_font(&self, style: u32) -> String {
        self.doc
            .text_styles
            .get(style)
            .map(|s| s.font_name.clone())
            .unwrap_or_else(|| "standard".to_string())
    }

    /// Font a Text entity resolves to: its explicit `font_name`, else its
    /// style's font. Delegates to the kernel's single source of truth so
    /// the renderer, TXTEXP, hatch tracing, and the Properties panel can
    /// never drift apart.
    pub(super) fn resolve_text_font(&self, t: &cad_kernel::Text) -> String {
        t.resolved_font_name(&self.doc.text_styles)
    }

    /// Glyph boundary geometry for every visible Text dobject in `scope` —
    /// letters participate in hatch boundary tracing as shapes. One `src`
    /// per entity keeps intra-word letter overlaps un-split, so connected
    /// Persian/Arabic glyphs trace as whole letters.
    fn text_hatch_geom(&self, scope: &[usize]) -> crate::hatch_trace::TextTraceGeom {
        let mut fm = self.font_manager.borrow_mut();
        crate::hatch_trace::text_hatch_geom(&self.doc, &mut fm, scope)
    }

    /// The glyph loops under `seed`: the smallest letter whose OUTER loop
    /// contains the seed, returned as `[outer, holes…]`. Used when the
    /// cheap path can't find a closed dobject but text does contain the
    /// click (and as the worker-failure text fallback). `scope` limits
    /// which Text dobjects are rendered — pass the viewport scope (text
    /// outside it provably can't contain the in-view seed).
    pub(super) fn text_hatch_loops_at(&self, seed: Vec2, scope: &[usize]) -> Option<Vec<Vec<Vec2>>> {
        let geom = self.text_hatch_geom(scope);
        let area = |pts: &[Vec2]| -> f64 {
            let n = pts.len();
            if n < 3 {
                return f64::INFINITY;
            }
            let mut a = 0.0;
            for i in 0..n {
                let p = pts[i];
                let q = pts[(i + 1) % n];
                a += p.x * q.y - q.x * p.y;
            }
            (a * 0.5).abs()
        };
        let mut best: Option<(f64, Vec<Vec<Vec2>>)> = None;
        for (outer, holes) in &geom.glyphs {
            if outer.len() < 3 || !point_in_polygon(seed, outer.iter().copied()) {
                continue;
            }
            let a = area(outer);
            if best.as_ref().map_or(true, |(ba, _)| a < *ba) {
                let mut loops = Vec::with_capacity(1 + holes.len());
                loops.push(outer.clone());
                loops.extend(holes.iter().cloned());
                best = Some((a, loops));
            }
        }
        best.map(|(_, l)| l)
    }

    /// Shaped pen ADVANCE (world units) of `text` in the given font/height —
    /// used to trim an underline past a list marker. 0 if the engine isn't ready.
    fn ttf_advance(
        &self,
        text: &str,
        font_name: &str,
        height: f64,
        slant: f64,
        x_scale: f64,
    ) -> f64 {
        if text.is_empty() || !self.font_manager.borrow().is_ready() {
            return 0.0;
        }
        let req = cad_text::TextRequest {
            text,
            font_name,
            position: Vec2::ZERO,
            height,
            angle: 0.0,
            h_align: cad_kernel::TextHAlign::Left,
            v_align: cad_kernel::TextVAlign::Baseline,
            fill_mode: cad_text::FillMode::Fill,
            slant,
            x_scale: if x_scale.abs() < 1e-9 { 1.0 } else { x_scale },
        };
        self.font_manager.borrow_mut().render(&req).advance
    }

    /// Commit the panel's text at the current anchor as ONE paragraph dobject
    /// (multi-line stored in a single `Text`; list markers are a render-time
    /// property, never baked). Then STAY OPEN and re-arm for the next anchor
    /// click (place-multiple, per the dialog template).
    fn commit_text_dialog(&mut self) {
        let Some(pos) = self.text_input_dialog_anchor else {
            self.set_prompt("text: click anchor position".to_string());
            return;
        };
        let raw = self.text_input_dialog_buf.clone();
        if raw.trim().is_empty() {
            return;
        }
        // Trim trailing blank lines / whitespace; keep interior line structure.
        let body = raw
            .trim_end_matches(|c| c == '\n' || c == ' ' || c == '\t')
            .to_string();
        if body.trim().is_empty() {
            return;
        }
        let height = self.text_input_dialog_height.max(1e-3);
        let style_id = self.text_input_dialog_style_id;
        let halign = self.text_dialog_halign;
        // One undo entry, one dobject for the whole Apply.
        self.snapshot_doc();
        self.commit_text_line(pos, &body, height, style_id, halign);
        // Stay open: clear the body + anchor, wait for the next anchor click.
        self.text_input_dialog_buf.clear();
        self.text_input_dialog_anchor = None;
        self.text_draft = TextDraftState::WaitingForPosition;
        self.tool = Tool::Text;
        let p = self.text_anchor_prompt(true);
        self.set_prompt(p);
    }

    /// Place ONE Text/paragraph dobject with an explicit horizontal alignment.
    /// `string` may be multi-line ('\n'); the dialog's `list_mode` is stored as
    /// a render-time PROPERTY (markers are never baked into the text). Snapshots
    /// the style's render specs so the entity is independent.
    fn commit_text_line(
        &mut self,
        pos: Vec2,
        string: &str,
        height: f64,
        style_id: u32,
        halign: cad_kernel::TextHAlign,
    ) {
        if string.is_empty() || height <= 1e-9 {
            return;
        }
        let style = if (style_id as usize) < self.doc.text_styles.len() {
            style_id
        } else {
            cad_kernel::TextStyleTable::STANDARD
        };
        // SNAPSHOT the style's render specs onto the entity, so this text is
        // independent — later edits to the style never retro-change it, and the
        // next text placed can differ freely (owner: "individual, not related").
        let s = self
            .doc
            .text_styles
            .get(style)
            .cloned()
            .unwrap_or_else(cad_kernel::TextStyle::standard);
        self.add_dobject(
            Geom::Text(cad_kernel::Text {
                position: pos,
                height,
                angle: self.text_dialog_angle_deg.to_radians(),
                text: string.into(),
                h_align: halign,
                v_align: cad_kernel::TextVAlign::Baseline,
                style,
                font_name: s.font_name,
                bold: s.bold,
                oblique: s.oblique,
                width_factor: s.width_factor,
                outline_only: s.outline_only,
                outline_width: s.outline_width,
                underline: s.underline,
                list_mode: self.text_list_mode,
                line_spacing: 1.5,
            }),
            "canvas",
        );
    }

    /// Render `text` in the given TTF font into `rect` as a coloured mesh —
    /// the text-dialog style preview. Returns false (caller falls back to egui
    /// text) when the engine isn't ready or produced no fills.
    fn draw_ttf_swatch(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        text: &str,
        font_name: &str,
        slant: f64,
        x_scale: f64,
        color: egui::Color32,
    ) -> bool {
        if text.is_empty() || !self.font_manager.borrow().is_ready() {
            return false;
        }
        let req = cad_text::TextRequest {
            text,
            font_name,
            position: Vec2::ZERO,
            height: 1.0,
            angle: 0.0,
            h_align: cad_kernel::TextHAlign::Left,
            v_align: cad_kernel::TextVAlign::Baseline,
            fill_mode: cad_text::FillMode::Fill,
            slant,
            x_scale: if x_scale.abs() < 1e-9 { 1.0 } else { x_scale },
        };
        let g = self.font_manager.borrow_mut().render(&req);
        if g.fills.is_empty() {
            return false;
        }
        let (mn, mx) = g.bbox;
        let w = (mx.x - mn.x).max(1e-6);
        let h = (mx.y - mn.y).max(1e-6);
        let pad = 12.0_f64;
        let scale = (((rect.width() as f64) - pad) / w).min(((rect.height() as f64) - pad) / h);
        let (bcx, bcy) = ((mn.x + mx.x) * 0.5, (mn.y + mx.y) * 0.5);
        let (cx, cy) = (rect.center().x, rect.center().y);
        let to_scr = |p: Vec2| {
            egui::pos2(
                cx + ((p.x - bcx) * scale) as f32,
                cy - ((p.y - bcy) * scale) as f32,
            )
        };
        let mut mesh = egui::Mesh::default();
        for tri in &g.fills {
            let base = mesh.vertices.len() as u32;
            for pt in tri {
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: to_scr(*pt),
                    uv: egui::epaint::WHITE_UV,
                    color,
                });
            }
            mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
        painter.add(egui::Shape::mesh(mesh));
        // Anti-alias the hard triangle-fill edge with a thin feathered perimeter
        // stroke (egui feathers lines) — same trick as `draw_ttf_string`. Without
        // it the swatch glyphs look jagged / low-res.
        let stroke = egui::Stroke::new(1.0, color);
        for contour in &g.outlines {
            let pts: Vec<egui::Pos2> = contour.iter().map(|p| to_scr(*p)).collect();
            painter.add(egui::Shape::closed_line(pts, stroke));
        }
        true
    }

    // ===================================================================
    // WP-SCRIPT — Python engine + docked REPL console (ported from the
    // upstream RUST-AutoRASM). The engine runs on a worker thread; cad_app
    // only ever touches the plain-Rust facade (cad_script::ScriptEngine).
    // ===================================================================

    /// WP-SCRIPT: drain the Python engine's replies (Print / Value / Error /
    /// Finished) into the command history, once per frame. Requests a repaint
    /// when output arrives (rule 6). No-op when no engine exists / no output.
    pub(super) fn poll_script_engine(&mut self, ctx: &egui::Context) {
        // 1) Service the reverse ScriptOp queue (slices 2–3): a running
        //    script blocks (GIL-free) on each op until we answer here, so
        //    this pump must run EVERY frame while a script is busy.
        let ops = match &self.script {
            Some(s) => s.drain_ops(),
            None => return,
        };
        for msg in ops {
            let reply = self.apply_script_op(msg.op);
            if let Some(s) = &self.script {
                s.reply_op(msg.id, reply);
            }
        }
        // 2) Drain worker output into the history AND the console log.
        let replies = match &self.script {
            Some(s) => s.poll(),
            None => return,
        };
        for r in &replies {
            match r {
                cad_script::ScriptReply::Print(s) => {
                    for line in s.trim_end_matches('\n').split('\n') {
                        self.history.push(format!("  {}", line));
                        self.py_log(line.to_string());
                    }
                }
                cad_script::ScriptReply::Value(v) => {
                    self.history.push(format!("  = {}", v));
                    self.py_log(format!("= {}", v));
                }
                cad_script::ScriptReply::Error(e) => {
                    // Never silent (rule 10): traceback → history (red `!`) + status.
                    for line in e.trim_end_matches('\n').split('\n') {
                        self.history.push(format!("  ! {}", line));
                        self.py_log(format!("! {}", line));
                    }
                    self.set_prompt("python: script raised — see the log");
                }
                cad_script::ScriptReply::Finished { ok } => {
                    self.on_script_finished(*ok);
                }
                cad_script::ScriptReply::Meta(meta) => {
                    // Slice 5 — the script's parameter declaration arrived
                    // (answer to the dialog's request_meta). Its Finished is
                    // next in the stream — mark it so `on_script_finished`
                    // doesn't mistake it for the preview's own finish (they
                    // share a poll batch: Meta starts the preview, and the
                    // META job's Finished must not finalize it).
                    self.script_meta_finish_pending = true;
                    // A pending k=v / positional run consumes the spec first
                    // (length conversion); otherwise it feeds the dialog.
                    if self.script_pending_run.is_some() {
                        self.run_pending_script(meta.clone());
                    } else {
                        self.on_script_meta(meta.clone());
                    }
                }
            }
        }
        // 3) Keep the pump alive while a script runs (ops arrive between
        //    frames; the worker waits on our replies).
        if !replies.is_empty() || self.script.as_ref().is_some_and(|s| s.is_busy()) {
            ctx.request_repaint();
        }
    }

    /// Cap the console log (rule 11 — a print-heavy script must not grow
    /// memory without bound).
    fn py_log(&mut self, line: String) {
        self.py_console_log.push(line);
        if self.py_console_log.len() > 800 {
            let cut = self.py_console_log.len() - 500;
            self.py_console_log.drain(0..cut);
        }
    }

    /// Apply one script op against the live document (main thread — the
    /// single writer of `self.doc`, D3). Read ops build owned replies; write
    /// ops go through the SAME seams as typed commands so undo / index / GPU
    /// dirty flags all behave identically (D4).
    pub(super) fn apply_script_op(&mut self, op: cad_script::ScriptOp) -> cad_script::ScriptOpReply {
        use cad_script::{ScriptOp as Op, ScriptOpReply as R};
        // Slice 5 preview: while a ghost pass runs, every op lands in the
        // shadow document — the real doc is never touched (no undo, no
        // history, no GPU invalidation).
        if self.script_preview.as_ref().is_some_and(|p| p.running) {
            return self.apply_preview_op(&op);
        }
        // D5 — lazy undo baseline: the first write of a run captures the
        // pre-run depth; Finished collapses everything above it (see
        // poll_script_engine). The group-boundary list belongs to the same
        // run — reset it here.
        if op.is_write() && self.script_undo_base.is_none() {
            self.script_undo_base = Some(self.undo_stack.len());
            self.script_group_snapshots.clear();
        }
        match op {
            // ---- reads ----
            Op::DocCount => R::Count(self.doc.dobjects.len()),
            Op::DocGet { index } => match self.doc.dobjects.get(index) {
                Some(d) => R::Entity(self.entity_snapshot(d)),
                None => R::Error(format!(
                    "no dobject #{} ({} total)",
                    index,
                    self.doc.dobjects.len()
                )),
            },
            Op::DocAll => R::Entities(
                self.doc
                    .dobjects
                    .iter()
                    .map(|d| self.entity_snapshot(d))
                    .collect(),
            ),
            Op::SelectionGet => R::Indices(self.selection.clone()),
            Op::LayersGet => R::Layers(
                self.doc
                    .layers
                    .layers
                    .iter()
                    .enumerate()
                    .map(|(i, l)| cad_script::LayerInfo {
                        id: i as u32,
                        name: l.name.clone(),
                        visible: l.visible,
                        locked: l.locked,
                        frozen: l.frozen,
                        plottable: l.plottable,
                        color: format!("{:?}", l.color),
                    })
                    .collect(),
            ),
            Op::LayerActive => R::LayerActive(self.doc.layers.active),
            Op::BlocksGet => R::Blocks(
                self.doc
                    .blocks
                    .blocks
                    .iter()
                    .map(|b| b.name.clone())
                    .collect(),
            ),
            Op::SysVarGet { name } => R::SysVar(crate::varreg::env_get(&self.env, &name)),
            Op::ViewGet => R::View(cad_script::ViewInfo {
                center: Vec2::new(-self.world_offset.x as f64, -self.world_offset.y as f64),
                scale: self.scale as f64,
            }),

            // ---- writes ----
            Op::SelectionSet { indices } => {
                let max = self.doc.dobjects.len();
                let valid: Vec<usize> = indices.into_iter().filter(|&i| i < max).collect();
                self.selection = valid.clone();
                self.selected = None;
                self.gpu_dirty = true;
                self.history
                    .push(format!("  python: selection → {} dobject(s)", valid.len()));
                R::Indices(valid)
            }
            Op::AddLine { a, b } => {
                let idx = self.next_add_index();
                self.add_dobject_undoable(Geom::Line(Line { a, b }), "python");
                R::Ok(idx)
            }
            Op::AddCircle { center, radius } => {
                if !(radius > 0.0) {
                    return R::Error("circle radius must be > 0".into());
                }
                let idx = self.next_add_index();
                self.add_dobject_undoable(Geom::Circle(Circle { center, radius }), "python");
                R::Ok(idx)
            }
            Op::AddArc {
                center,
                radius,
                start_deg,
                sweep_deg,
            } => {
                if !(radius > 0.0) {
                    return R::Error("arc radius must be > 0".into());
                }
                let idx = self.next_add_index();
                self.add_dobject_undoable(
                    Geom::Arc(Arc {
                        center,
                        radius,
                        start_angle: start_deg.to_radians(),
                        sweep_angle: sweep_deg.to_radians(),
                    }),
                    "python",
                );
                R::Ok(idx)
            }
            Op::AddEllipse {
                center,
                major,
                ratio,
            } => {
                let idx = self.next_add_index();
                self.add_dobject_undoable(
                    Geom::Ellipse(Ellipse {
                        center,
                        major,
                        ratio,
                    }),
                    "python",
                );
                R::Ok(idx)
            }
            Op::AddPolyline { vertices, closed } => {
                if vertices.len() < 2 {
                    return R::Error("a polyline needs at least 2 points".into());
                }
                let idx = self.next_add_index();
                self.add_dobject_undoable(
                    Geom::Polyline(Polyline {
                        vertices: vertices
                            .iter()
                            .map(|p| PolyVertex {
                                pos: *p,
                                bulge: 0.0,
                            })
                            .collect(),
                        closed,
                        widths: Vec::new(),
                    }),
                    "python",
                );
                R::Ok(idx)
            }
            Op::AddPoint { at } => {
                let idx = self.next_add_index();
                self.add_dobject_undoable(
                    Geom::Point(cad_kernel::Point {
                        location: at,
                        style: 0,
                        size: 0.0,
                    }),
                    "python",
                );
                R::Ok(idx)
            }
            Op::AddText {
                text,
                at,
                height,
                angle_deg,
            } => {
                let idx = self.next_add_index();
                let mut t = cad_kernel::Text::empty();
                t.position = at;
                t.height = height;
                t.angle = angle_deg.to_radians();
                t.text = text;
                self.add_dobject_undoable(Geom::Text(t), "python");
                R::Ok(idx)
            }
            Op::Delete { indices } => {
                self.snapshot_doc();
                let mut gone: std::collections::HashSet<usize> = indices.iter().copied().collect();
                let n = gone.len();
                // Same shared aux-boundary sweep every delete path uses.
                let orphans = self.doc.erase_dobjects(indices);
                gone.extend(orphans.iter().copied());
                self.selection.retain(|i| !gone.contains(i));
                self.selected = None;
                self.intersections.clear();
                self.index_dirty = true;
                self.gpu_dirty = true;
                self.history
                    .push(format!("  python: deleted {} dobject(s)", n));
                R::Ok(n)
            }
            Op::LayerAdd { name } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return R::Error("layer name cannot be empty".into());
                }
                if self.doc.layers.find(&name).is_some() {
                    return R::Error(format!("layer '{}' already exists", name));
                }
                self.snapshot_doc();
                let id = self.doc.layers.add(Layer {
                    name: name.clone(),
                    color: Color::Aci(7),
                    linetype: 0,
                    lineweight: cad_kernel::Lineweight::Custom(0.0),
                    visible: true,
                    locked: false,
                    frozen: false,
                    plottable: true,
                    order: 0,
                });
                self.gpu_dirty = true;
                self.history
                    .push(format!("  python: + layer '{}' (#{})", name, id));
                R::Ok(id as usize)
            }
            Op::LayerSetActive { name } => match self.doc.layers.find(&name) {
                Some(id) => {
                    self.doc.layers.active = id;
                    self.gpu_dirty = true;
                    self.history
                        .push(format!("  python: active layer → '{}' (#{})", name, id));
                    R::OkUnit
                }
                None => R::Error(format!("no layer named '{}'", name)),
            },
            Op::LayerSet {
                name,
                visible,
                locked,
                frozen,
                plottable,
                color_aci,
            } => match self.doc.layers.find(&name) {
                None => R::Error(format!("no layer named '{}'", name)),
                Some(id) => {
                    if id == self.doc.layers.active
                        && (visible == Some(false) || frozen == Some(true))
                    {
                        return R::Error("the active layer cannot be turned off or frozen".into());
                    }
                    self.snapshot_doc();
                    if let Some(l) = self.doc.layers.get_mut(id) {
                        if let Some(v) = visible {
                            l.visible = v;
                        }
                        if let Some(v) = locked {
                            l.locked = v;
                        }
                        if let Some(v) = frozen {
                            l.frozen = v;
                        }
                        if let Some(v) = plottable {
                            l.plottable = v;
                        }
                        if let Some(a) = color_aci {
                            l.color = Color::Aci(a);
                        }
                    }
                    self.gpu_dirty = true;
                    self.history
                        .push(format!("  python: layer '{}' updated", name));
                    R::OkUnit
                }
            },
            Op::BlockCreate { name, base } => {
                if name.trim().is_empty() {
                    return R::Error("block name cannot be empty".into());
                }
                if self.doc.blocks.find(&name).is_some() {
                    return R::Error(format!(
                        "block '{}' already exists (no redefinition yet)",
                        name
                    ));
                }
                if self.selection.is_empty() {
                    return R::Error(
                        "create_block needs a selection — set it with rasm.set_selection first"
                            .into(),
                    );
                }
                self.apply_block_create(&name, base, None, false);
                R::OkUnit
            }
            Op::BlockInsert { name, at, rotation } => match self.doc.blocks.find(&name) {
                None => R::Error(format!("no block named '{}'", name)),
                Some(id) => {
                    if let Some(blk) = self.doc.blocks.get(id) {
                        if !blk.params.is_empty() {
                            return R::Error(format!(
                                    "block '{}' is parametric — insert it from the UI (prompts for values)",
                                    name
                                ));
                        }
                    }
                    self.apply_insert(id, at, rotation);
                    R::OkUnit
                }
            },
            Op::Command { raw } => {
                let h0 = self.history.len();
                self.run_command(&raw);
                let out: Vec<String> = self.history[h0..].to_vec();
                R::CommandOutput(out)
            }
            Op::SysVarSet { name, value } => {
                match crate::varreg::env_set(&mut self.env, &name, &value) {
                    Ok(()) => {
                        let _ = self.env.save();
                        R::OkUnit
                    }
                    Err(e) => R::Error(e),
                }
            }
            Op::ViewSet { center, scale } => {
                if let Some(s) = scale {
                    if !(s > 0.0 && s.is_finite()) {
                        return R::Error("view scale must be a positive number".into());
                    }
                }
                self.view_push_history();
                self.world_offset = egui::vec2(-center.x as f32, -center.y as f32);
                if let Some(s) = scale {
                    self.scale = (s as f32).clamp(1e-6, 5000.0);
                }
                R::OkUnit
            }
            Op::Save { path } => {
                let h0 = self.history.len();
                self.do_save(&path);
                let out: Vec<String> = self.history[h0..].to_vec();
                R::CommandOutput(out)
            }
            Op::Open { path } => {
                let h0 = self.history.len();
                self.do_open(&path);
                let out: Vec<String> = self.history[h0..].to_vec();
                R::CommandOutput(out)
            }

            // ---- P1 entity modification ----
            Op::ModifyMove { indices, delta } => {
                if delta.len() < 1e-12 {
                    return R::Error("move delta is zero".into());
                }
                let n = match self.script_transform(&indices, |g| g.translated(delta), "moved") {
                    Ok(n) => n,
                    Err(e) => return R::Error(e),
                };
                R::Ok(n)
            }
            Op::ModifyCopy { indices, delta } => {
                if delta.len() < 1e-12 {
                    return R::Error("copy delta is zero".into());
                }
                self.snapshot_doc();
                let sources: Vec<DObject> = indices
                    .iter()
                    .filter_map(|&i| self.doc.dobjects.get(i).cloned())
                    .collect();
                if sources.is_empty() {
                    return R::Error("copy: none of the given indices exist".into());
                }
                let n0 = self.doc.dobjects.len();
                let copies = duplicate_dobjects(&sources, |g| g.translated(delta));
                for c in copies {
                    self.doc.push(c);
                }
                let new_idx: Vec<usize> = (n0..self.doc.dobjects.len()).collect();
                self.history
                    .push(format!("  python: copied {} dobject(s)", new_idx.len()));
                self.intersections.clear();
                self.index_dirty = true;
                self.gpu_dirty = true;
                R::Indices(new_idx)
            }
            Op::ModifyRotate {
                indices,
                pivot,
                angle_deg,
            } => {
                let angle = angle_deg.to_radians();
                if angle.abs() < 1e-12 {
                    return R::Error("rotate angle is zero".into());
                }
                let n =
                    match self.script_transform(&indices, |g| g.rotated(pivot, angle), "rotated") {
                        Ok(n) => n,
                        Err(e) => return R::Error(e),
                    };
                R::Ok(n)
            }
            Op::ModifyScale {
                indices,
                pivot,
                factor,
            } => {
                if !(factor > 0.0 && factor.is_finite()) {
                    return R::Error("scale factor must be a positive number".into());
                }
                if (factor - 1.0).abs() < 1e-12 {
                    return R::Error("scale factor is 1.0 (nothing to do)".into());
                }
                let n = match self.script_transform(&indices, |g| g.scaled(pivot, factor), "scaled")
                {
                    Ok(n) => n,
                    Err(e) => return R::Error(e),
                };
                R::Ok(n)
            }
            Op::ModifyMirror { indices, a, b } => {
                if a.dist(b) < 1e-12 {
                    return R::Error("mirror axis is degenerate (a == b)".into());
                }
                let n = match self.script_transform(&indices, |g| g.mirrored(a, b), "mirrored") {
                    Ok(n) => n,
                    Err(e) => return R::Error(e),
                };
                R::Ok(n)
            }
            Op::SetEntityColor { indices, color } => {
                let c = match color {
                    -1 => Color::ByLayer,
                    -2 => Color::ByBlock,
                    n if (0..=255).contains(&n) => Color::Aci(n as u8),
                    other => {
                        return R::Error(format!(
                            "set_color: {} is not a color (0..=255, -1 = bylayer, -2 = byblock)",
                            other
                        ))
                    }
                };
                let n = match self.script_style_set(&indices, |d| d.style.color = c) {
                    Ok(n) => n,
                    Err(e) => return R::Error(e),
                };
                self.history
                    .push(format!("  python: set color on {} dobject(s)", n));
                R::Ok(n)
            }
            Op::SetEntityLinetype { indices, name } => {
                use cad_kernel::LinetypeTable;
                let id = if name.is_empty() || name.eq_ignore_ascii_case("bylayer") {
                    LinetypeTable::BYLAYER
                } else {
                    match self.doc.linetypes.find(&name) {
                        Some(id) => id,
                        None => {
                            return R::Error(format!("no linetype named '{}'", name));
                        }
                    }
                };
                let n = match self.script_style_set(&indices, |d| d.style.linetype = id) {
                    Ok(n) => n,
                    Err(e) => return R::Error(e),
                };
                self.history
                    .push(format!("  python: set linetype on {} dobject(s)", n));
                R::Ok(n)
            }
            Op::SetEntityLayer { indices, name } => match self.doc.layers.find(&name) {
                None => R::Error(format!("no layer named '{}'", name)),
                Some(id) => {
                    let n = match self.script_style_set(&indices, |d| d.style.layer = id) {
                        Ok(n) => n,
                        Err(e) => return R::Error(e),
                    };
                    self.history
                        .push(format!("  python: moved {} dobject(s) to '{}'", n, name));
                    R::Ok(n)
                }
            },
            Op::SetEntityLineweight { indices, mm } => {
                let lw = if mm < 0.0 {
                    Lineweight::ByLayer
                } else {
                    Lineweight::Custom(mm as f32)
                };
                let n = match self.script_style_set(&indices, |d| d.style.lineweight = lw) {
                    Ok(n) => n,
                    Err(e) => return R::Error(e),
                };
                self.history
                    .push(format!("  python: set lineweight on {} dobject(s)", n));
                R::Ok(n)
            }
            Op::SetEntityVisible { indices, visible } => {
                let n = match self.script_style_set(&indices, |d| d.style.visible = visible) {
                    Ok(n) => n,
                    Err(e) => return R::Error(e),
                };
                self.history.push(format!(
                    "  python: {} {} dobject(s)",
                    if visible { "showed" } else { "hid" },
                    n
                ));
                R::Ok(n)
            }
            Op::SetEntityGeom { index, geom } => {
                if index >= self.doc.dobjects.len() {
                    return R::Error(format!(
                        "set_geom: no dobject #{} ({} total)",
                        index,
                        self.doc.dobjects.len()
                    ));
                }
                self.snapshot_doc();
                self.doc.dobjects[index].geom = geom;
                self.intersections.clear();
                self.index_dirty = true;
                self.gpu_dirty = true;
                self.history
                    .push(format!("  python: replaced geometry of #{}", index));
                R::OkUnit
            }

            // ---- P2 document-state reads ----
            Op::DocUnits => R::Units(cad_script::UnitsInfo {
                name: self.doc.units.name.clone(),
                scene_per_unit: self.doc.units.scene_per_unit,
            }),
            Op::DocBounds => {
                let mut min: Option<Vec2> = None;
                let mut max: Option<Vec2> = None;
                for d in &self.doc.dobjects {
                    if matches!(d.geom, Geom::Hatch(_)) {
                        continue; // hatch bbox is a placeholder
                    }
                    let (lo, hi) = match &d.geom {
                        Geom::BlockRef(br) => self.resolved_blockref_bbox(br),
                        _ => d.bbox(),
                    };
                    min = Some(match min {
                        None => lo,
                        Some(m) => Vec2::new(m.x.min(lo.x), m.y.min(lo.y)),
                    });
                    max = Some(match max {
                        None => hi,
                        Some(m) => Vec2::new(m.x.max(hi.x), m.y.max(hi.y)),
                    });
                }
                R::Bounds(min.zip(max))
            }
            Op::LayoutsGet => R::Layouts(
                self.doc
                    .layouts
                    .iter()
                    .enumerate()
                    .map(|(i, l)| cad_script::LayoutInfo {
                        id: i as u32,
                        name: l.name.clone(),
                        active: self.doc.active_layout == Some(i),
                    })
                    .collect(),
            ),
            Op::LayoutSetActive { name } => {
                // This build has no layout tabs — the model is the only space.
                let _ = name;
                R::Error("layouts are not available in this build".into())
            }
            Op::LinetypesGet => R::Linetypes(
                self.doc
                    .linetypes
                    .linetypes
                    .iter()
                    .map(|l| l.name.clone())
                    .collect(),
            ),

            // ---- P3 ----
            Op::UndoGroup => {
                // An explicit boundary: the CURRENT state becomes the
                // pre-state of the next undo unit. Record the snapshot's
                // index; `on_script_finished` keeps pre-run + boundaries.
                self.snapshot_doc();
                self.script_group_snapshots.push(self.undo_stack.len() - 1);
                R::OkUnit
            }
            Op::SetCurrentColor { color } => {
                let c = match color {
                    -1 => Color::ByLayer,
                    -2 => Color::ByBlock,
                    n if (0..=255).contains(&n) => Color::Aci(n as u8),
                    other => {
                        return R::Error(format!("set_current_color: {} is not a color", other))
                    }
                };
                self.doc.current_color = c;
                self.history.push(format!(
                    "  python: current color → {}",
                    Self::script_color_string(c, &self.doc)
                ));
                R::OkUnit
            }
            Op::SetCurrentLinetype { name } => match self.doc.linetypes.find(&name) {
                None => R::Error(format!("no linetype named '{}'", name)),
                Some(id) => {
                    self.doc.current_linetype = id;
                    self.history
                        .push(format!("  python: current linetype → '{}'", name));
                    R::OkUnit
                }
            },
            Op::SetCurrentLineweight { mm } => {
                if !(mm >= 0.0 && mm.is_finite()) {
                    return R::Error("lineweight mm must be >= 0".into());
                }
                self.doc.current_lineweight = Lineweight::Custom(mm as f32);
                self.history
                    .push(format!("  python: current lineweight → {:.2} mm", mm));
                R::OkUnit
            }

            // ---- P4 ----
            Op::ZoomExtents => {
                self.zoom_extents();
                R::OkUnit
            }

            // ---- hatching ----
            Op::AddHatch {
                boundary_indices,
                pattern,
            } => {
                match self.script_hatch_pattern(&pattern) {
                    Err(e) => R::Error(e),
                    Ok(pat) => {
                        // Accepted boundary kinds mirror apply_hatch: closed
                        // polylines / circles / ellipses / closed splines.
                        let mut handles: Vec<cad_kernel::Handle> = Vec::new();
                        let mut skipped = 0usize;
                        for &i in &boundary_indices {
                            let Some(d) = self.doc.dobjects.get(i) else {
                                skipped += 1;
                                continue;
                            };
                            let ok = match &d.geom {
                                Geom::Polyline(p) => polyline_is_effectively_closed(p),
                                Geom::Circle(_) | Geom::Ellipse(_) => true,
                                Geom::Spline(s) => spline_is_effectively_closed(s),
                                _ => false,
                            };
                            if ok {
                                handles.push(d.handle);
                            } else {
                                skipped += 1;
                            }
                        }
                        if handles.is_empty() {
                            return R::Error(format!(
                                "add_hatch: none of the {} given indices is a closed boundary (closed polyline / circle / ellipse / closed spline)",
                                boundary_indices.len(),
                            ));
                        }
                        let idx = self.script_commit_hatch(handles, pat, skipped);
                        R::Ok(idx)
                    }
                }
            }
            Op::HatchAt { point, pattern } => match self.script_hatch_pattern(&pattern) {
                Err(e) => R::Error(e),
                Ok(pat) => match self.find_smallest_containing_closed_scoped(point, None) {
                    None => R::Indices(Vec::new()),
                    Some(i) => {
                        let mut boundary = vec![i];
                        boundary.extend(self.collect_islands_inside_scoped(i, point, None));
                        let mut handles: Vec<cad_kernel::Handle> = Vec::new();
                        for &bi in &boundary {
                            if let Some(d) = self.doc.dobjects.get(bi) {
                                handles.push(d.handle);
                            }
                        }
                        self.script_commit_hatch(handles, pat, 0);
                        R::Indices(boundary)
                    }
                },
            },
            Op::HatchPatternsGet => R::Patterns(
                cad_kernel::patterns::PATTERN_NAMES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
        }
    }

    /// End-of-run handling. Three kinds:
    /// - a METADATA pass finish (marked by `script_meta_finish_pending`):
    ///   ignored entirely — its `Meta` reply already handled the outcome;
    /// - a ghost preview finish: finalizes the shadow's net additions into
    ///   the dashed overlay (or drops a cancelled/zombie pass) and stays out
    ///   of history / console / undo;
    /// - a real run finish: history + console + the one-run-one-undo
    ///   collapse (D5 — one run = one undo unit; P3 — grouped runs keep one
    ///   entry per `rasm.undo_group()` boundary).
    pub(super) fn on_script_finished(&mut self, ok: bool) {
        if self.script_meta_finish_pending {
            self.script_meta_finish_pending = false;
            return;
        }
        let was_preview = self.script_preview.as_ref().is_some_and(|p| p.running);
        if !was_preview {
            if ok {
                self.history.push("  ✔ python: done".into());
            }
            self.py_log(format!("— run {}", if ok { "done" } else { "failed" }));
            if let Some(base) = self.script_undo_base.take() {
                if self.script_group_snapshots.is_empty() {
                    // D5 — one run = one undo unit.
                    let keep = (base + 1).min(self.undo_stack.len());
                    if self.undo_stack.len() > keep {
                        self.undo_stack.truncate(keep);
                    }
                } else {
                    // P3 — grouped run: keep the pre-run snapshot plus ONE
                    // entry per `rasm.undo_group()` boundary; each group is
                    // its own undo unit. Boundary indices are stack indices
                    // recorded at boundary time (the stack only appends
                    // during a run, so they stay valid).
                    let mut kept = Vec::with_capacity(self.script_group_snapshots.len() + 1);
                    kept.push(base);
                    kept.extend(self.script_group_snapshots.iter().copied());
                    let mut out = Vec::with_capacity(kept.len());
                    let mut last = base;
                    for &ix in &kept {
                        if ix >= self.undo_stack.len() || ix < base {
                            continue;
                        }
                        out.push(self.undo_stack[ix].clone());
                        last = ix;
                    }
                    if out.is_empty() {
                        out.push(
                            self.undo_stack[base.min(self.undo_stack.len().saturating_sub(1))]
                                .clone(),
                        );
                    }
                    let _ = last;
                    self.undo_stack.truncate(base);
                    self.undo_stack.extend(out);
                }
                self.script_group_snapshots.clear();
            }
        }
        if let Some(p) = &mut self.script_preview {
            if p.running {
                p.running = false;
                if p.cancelled {
                    self.script_preview = None;
                } else if ok {
                    p.ghosts = p
                        .doc
                        .dobjects
                        .iter()
                        .filter(|d| !p.base_handles.contains(&d.handle))
                        .map(|d| d.geom.clone())
                        .collect();
                }
                // A failed pass keeps the previous ghosts.
            }
        }
    }

    // ---- slice 5 preview: the shadow-document ghost pass ------------------

    /// Start a ghost pass: clone the real doc, snapshot its handles, and
    /// submit the script with the CURRENT dialog values. Ops land in the
    /// shadow; on `Finished` the net additions become the dashed overlay.
    pub(super) fn script_preview_start(
        &mut self,
        name: String,
        path: std::path::PathBuf,
        params: Vec<(String, String)>,
    ) {
        self.script_preview = Some(ScriptPreview {
            base_handles: self.doc.dobjects.iter().map(|d| d.handle).collect(),
            doc: self.doc.clone(),
            running: true,
            cancelled: false,
            dirty: false,
            last_restart: std::time::Instant::now(),
            ghosts: Vec::new(),
        });
        self.script
            .get_or_insert_with(cad_script::ScriptEngine::new)
            .submit_script_preview(path, name, params);
    }

    /// The dialog values changed — mark the preview stale (the restart is
    /// throttled in `render_script_param_dialog`).
    pub(super) fn script_preview_dirty(&mut self) {
        if let Some(p) = &mut self.script_preview {
            if !p.cancelled {
                p.dirty = true;
            }
        }
    }

    /// Drop the preview state. A pass still in flight becomes a zombie —
    /// its ops keep landing in the shadow until `Finished` drops the struct,
    /// so they can never leak into the real document.
    pub(super) fn clear_script_preview(&mut self) {
        match &mut self.script_preview {
            Some(p) if p.running => {
                p.cancelled = true;
                p.dirty = false;
            }
            Some(_) => {
                self.script_preview = None;
            }
            None => {}
        }
    }

    /// One op of a ghost pass: reads see the shadow snapshot (so a script
    /// counts/looks up what IT has drawn so far), writes land in the shadow
    /// only. UI-only surfaces answer as inert no-ops — the preview is
    /// best-effort and the REAL run is what commits (rule 10: the answer
    /// says so).
    fn apply_preview_op(&mut self, op: &cad_script::ScriptOp) -> cad_script::ScriptOpReply {
        use cad_script::{ScriptOp as Op, ScriptOpReply as R};
        let p = self
            .script_preview
            .as_mut()
            .expect("preview op without preview");
        let doc = &mut p.doc;
        let entity = |d: &DObject| {
            let (color, linetype, lineweight, visible) = doc_entity_style_summary(doc, d);
            cad_script::Entity {
                handle: d.handle,
                layer: doc
                    .layers
                    .get(d.style.layer)
                    .map(|l| l.name.clone())
                    .unwrap_or_default(),
                color,
                linetype,
                lineweight,
                visible,
                geom: d.geom.clone(),
                style: d.style,
            }
        };
        match op {
            Op::DocCount => R::Count(doc.dobjects.len()),
            Op::DocGet { index } => match doc.dobjects.get(*index) {
                Some(d) => R::Entity(entity(d)),
                None => R::Error(format!(
                    "no dobject #{} ({} total)",
                    index,
                    doc.dobjects.len()
                )),
            },
            Op::DocAll => R::Entities(doc.dobjects.iter().map(entity).collect()),
            Op::SelectionGet => R::Indices(Vec::new()),
            Op::LayersGet => R::Layers(
                doc.layers
                    .layers
                    .iter()
                    .enumerate()
                    .map(|(i, l)| cad_script::LayerInfo {
                        id: i as u32,
                        name: l.name.clone(),
                        visible: l.visible,
                        locked: l.locked,
                        frozen: l.frozen,
                        plottable: l.plottable,
                        color: format!("{:?}", l.color),
                    })
                    .collect(),
            ),
            Op::LayerActive => R::LayerActive(doc.layers.active),
            Op::BlocksGet => R::Blocks(doc.blocks.blocks.iter().map(|b| b.name.clone()).collect()),
            Op::SysVarGet { name } => R::SysVar(crate::varreg::env_get(&self.env, name)),
            Op::ViewGet => R::View(cad_script::ViewInfo {
                center: Vec2::new(-self.world_offset.x as f64, -self.world_offset.y as f64),
                scale: self.scale as f64,
            }),

            Op::AddLine { a, b } => {
                let i = doc.push(DObject::new(Geom::Line(Line { a: *a, b: *b })));
                R::Ok(i)
            }
            Op::AddCircle { center, radius } => {
                if !(*radius > 0.0) {
                    return R::Error("circle radius must be > 0".into());
                }
                let i = doc.push(DObject::new(Geom::Circle(Circle {
                    center: *center,
                    radius: *radius,
                })));
                R::Ok(i)
            }
            Op::AddArc {
                center,
                radius,
                start_deg,
                sweep_deg,
            } => {
                if !(*radius > 0.0) {
                    return R::Error("arc radius must be > 0".into());
                }
                let i = doc.push(DObject::new(Geom::Arc(Arc {
                    center: *center,
                    radius: *radius,
                    start_angle: start_deg.to_radians(),
                    sweep_angle: sweep_deg.to_radians(),
                })));
                R::Ok(i)
            }
            Op::AddEllipse {
                center,
                major,
                ratio,
            } => {
                let i = doc.push(DObject::new(Geom::Ellipse(Ellipse {
                    center: *center,
                    major: *major,
                    ratio: *ratio,
                })));
                R::Ok(i)
            }
            Op::AddPolyline { vertices, closed } => {
                if vertices.len() < 2 {
                    return R::Error("a polyline needs at least 2 points".into());
                }
                let i = doc.push(DObject::new(Geom::Polyline(Polyline {
                    vertices: vertices
                        .iter()
                        .map(|v| PolyVertex {
                            pos: *v,
                            bulge: 0.0,
                        })
                        .collect(),
                    closed: *closed,
                    widths: Vec::new(),
                })));
                R::Ok(i)
            }
            Op::AddPoint { at } => {
                let i = doc.push(DObject::new(Geom::Point(Point {
                    location: *at,
                    style: 0,
                    size: 0.0,
                })));
                R::Ok(i)
            }
            Op::AddText {
                text,
                at,
                height,
                angle_deg,
            } => {
                let mut t = Text::empty();
                t.position = *at;
                t.height = *height;
                t.angle = angle_deg.to_radians();
                t.text = text.clone();
                let i = doc.push(DObject::new(Geom::Text(t)));
                R::Ok(i)
            }
            Op::Delete { indices } => {
                // Ghost-preview delete — same shared aux-boundary sweep, run
                // on the preview document.
                let n = {
                    let mut s = indices.clone();
                    s.sort_unstable();
                    s.dedup();
                    s.len()
                };
                let _ = doc.erase_dobjects(indices.clone());
                R::Ok(n)
            }
            Op::LayerAdd { name } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    return R::Error("layer name cannot be empty".into());
                }
                if doc.layers.find(&name).is_some() {
                    return R::Error(format!("layer '{}' already exists", name));
                }
                let id = doc.layers.add(Layer {
                    name,
                    color: Color::Aci(7),
                    linetype: 0,
                    lineweight: Lineweight::Custom(0.0),
                    visible: true,
                    locked: false,
                    frozen: false,
                    plottable: true,
                    order: 0,
                });
                R::Ok(id as usize)
            }
            Op::LayerSetActive { name } => match doc.layers.find(name) {
                Some(id) => {
                    doc.layers.active = id;
                    R::OkUnit
                }
                None => R::Error(format!("no layer named '{}'", name)),
            },
            Op::LayerSet {
                name,
                visible,
                locked,
                frozen,
                plottable,
                color_aci,
            } => match doc.layers.find(name) {
                None => R::Error(format!("no layer named '{}'", name)),
                Some(id) => {
                    if let Some(l) = doc.layers.get_mut(id) {
                        if let Some(v) = visible {
                            l.visible = *v;
                        }
                        if let Some(v) = locked {
                            l.locked = *v;
                        }
                        if let Some(v) = frozen {
                            l.frozen = *v;
                        }
                        if let Some(v) = plottable {
                            l.plottable = *v;
                        }
                        if let Some(a) = color_aci {
                            l.color = Color::Aci(*a);
                        }
                    }
                    R::OkUnit
                }
            },
            Op::SelectionSet { .. } => R::Indices(Vec::new()),
            Op::BlockCreate { .. } | Op::BlockInsert { .. } => {
                R::Error("preview does not simulate blocks — the real run applies them".into())
            }
            Op::Command { raw } => match parse(raw) {
                Ok(Command::Add(g)) => {
                    let i = doc.push(DObject::new(g));
                    R::CommandOutput(vec![format!("+ #{} (preview)", i)])
                }
                Ok(_) => R::CommandOutput(vec!["(preview: command not simulated)".into()]),
                Err(e) => R::CommandOutput(vec![format!("(preview: {})", e)]),
            },
            Op::SysVarSet { .. } => R::OkUnit,
            Op::ViewSet { .. } => R::OkUnit,
            Op::Save { .. } | Op::Open { .. } => {
                R::CommandOutput(vec!["(preview: file operations not simulated)".into()])
            }

            // P1 — transforms/style/geometry apply to the shadow so the
            // ghost preview shows their effect.
            Op::ModifyMove { indices, delta } => {
                let n = preview_transform(doc, indices, |g| g.translated(*delta));
                R::Ok(n)
            }
            Op::ModifyCopy { indices, delta } => {
                let sources: Vec<DObject> = indices
                    .iter()
                    .filter_map(|&i| doc.dobjects.get(i).cloned())
                    .collect();
                let n0 = doc.dobjects.len();
                let copies = duplicate_dobjects(&sources, |g| g.translated(*delta));
                for c in copies {
                    doc.push(c);
                }
                R::Indices((n0..doc.dobjects.len()).collect())
            }
            Op::ModifyRotate {
                indices,
                pivot,
                angle_deg,
            } => {
                let a = angle_deg.to_radians();
                let n = preview_transform(doc, indices, |g| g.rotated(*pivot, a));
                R::Ok(n)
            }
            Op::ModifyScale {
                indices,
                pivot,
                factor,
            } => {
                let n = preview_transform(doc, indices, |g| g.scaled(*pivot, *factor));
                R::Ok(n)
            }
            Op::ModifyMirror { indices, a, b } => {
                let n = preview_transform(doc, indices, |g| g.mirrored(*a, *b));
                R::Ok(n)
            }
            Op::SetEntityColor { indices, color } => {
                let c = match color {
                    -1 => Color::ByLayer,
                    -2 => Color::ByBlock,
                    n if (0..=255).contains(n) => Color::Aci(*n as u8),
                    _ => return R::Error("bad color".into()),
                };
                let n = preview_style(doc, indices, |d| d.style.color = c);
                R::Ok(n)
            }
            Op::SetEntityLinetype { indices, name } => {
                use cad_kernel::LinetypeTable;
                let id = if name.is_empty() || name.eq_ignore_ascii_case("bylayer") {
                    LinetypeTable::BYLAYER
                } else {
                    match doc.linetypes.find(name) {
                        Some(id) => id,
                        None => return R::Error(format!("no linetype named '{}'", name)),
                    }
                };
                let n = preview_style(doc, indices, |d| d.style.linetype = id);
                R::Ok(n)
            }
            Op::SetEntityLayer { indices, name } => match doc.layers.find(name) {
                None => R::Error(format!("no layer named '{}'", name)),
                Some(id) => {
                    let n = preview_style(doc, indices, |d| d.style.layer = id);
                    R::Ok(n)
                }
            },
            Op::SetEntityLineweight { indices, mm } => {
                let lw = if *mm < 0.0 {
                    Lineweight::ByLayer
                } else {
                    Lineweight::Custom(*mm as f32)
                };
                let n = preview_style(doc, indices, |d| d.style.lineweight = lw);
                R::Ok(n)
            }
            Op::SetEntityVisible { indices, visible } => {
                let n = preview_style(doc, indices, |d| d.style.visible = *visible);
                R::Ok(n)
            }
            Op::SetEntityGeom { index, geom } => match doc.dobjects.get_mut(*index) {
                Some(d) => {
                    d.geom = geom.clone();
                    R::OkUnit
                }
                None => R::Error(format!("no dobject #{}", index)),
            },
            Op::DocUnits => R::Units(cad_script::UnitsInfo {
                name: self.doc.units.name.clone(),
                scene_per_unit: self.doc.units.scene_per_unit,
            }),
            Op::DocBounds => {
                let mut min: Option<Vec2> = None;
                let mut max: Option<Vec2> = None;
                for d in &doc.dobjects {
                    if matches!(d.geom, Geom::Hatch(_)) {
                        continue;
                    }
                    let (lo, hi) = d.bbox();
                    min = Some(match min {
                        None => lo,
                        Some(m) => Vec2::new(m.x.min(lo.x), m.y.min(lo.y)),
                    });
                    max = Some(match max {
                        None => hi,
                        Some(m) => Vec2::new(m.x.max(hi.x), m.y.max(hi.y)),
                    });
                }
                R::Bounds(min.zip(max))
            }
            Op::LayoutsGet => R::Layouts(
                doc.layouts
                    .iter()
                    .enumerate()
                    .map(|(i, l)| cad_script::LayoutInfo {
                        id: i as u32,
                        name: l.name.clone(),
                        active: doc.active_layout == Some(i),
                    })
                    .collect(),
            ),
            Op::LayoutSetActive { .. } => R::Error("preview does not switch layouts".into()),
            Op::LinetypesGet => R::Linetypes(
                doc.linetypes
                    .linetypes
                    .iter()
                    .map(|l| l.name.clone())
                    .collect(),
            ),
            Op::UndoGroup => R::OkUnit,
            Op::SetCurrentColor { .. }
            | Op::SetCurrentLinetype { .. }
            | Op::SetCurrentLineweight { .. }
            | Op::ZoomExtents => R::OkUnit,
            // Hatching: AddHatch lands in the shadow (the ghost overlay
            // skips hatch fills — the real run is what shows them);
            // HatchAt tracing is app-native and returns "no region".
            Op::AddHatch {
                boundary_indices,
                pattern,
            } => {
                let pattern = pattern.trim();
                let pat = if pattern.eq_ignore_ascii_case("solid") {
                    HatchPattern::Solid
                } else {
                    let canonical = cad_kernel::patterns::PATTERN_NAMES
                        .iter()
                        .find(|n| n.eq_ignore_ascii_case(pattern))
                        .map(|s| s.to_string());
                    match canonical {
                        Some(name) => HatchPattern::Pattern {
                            name,
                            scale: 1.0,
                            angle_deg: 0.0,
                        },
                        None => {
                            return R::Error(format!(
                                "no hatch pattern '{}' — available: {}",
                                pattern,
                                cad_kernel::patterns::PATTERN_NAMES.join(", ")
                            ))
                        }
                    }
                };
                let mut handles: Vec<cad_kernel::Handle> = Vec::new();
                for &i in boundary_indices {
                    if let Some(d) = doc.dobjects.get(i) {
                        let ok = match &d.geom {
                            Geom::Polyline(p) => polyline_is_effectively_closed(p),
                            Geom::Circle(_) | Geom::Ellipse(_) => true,
                            Geom::Spline(s) => spline_is_effectively_closed(s),
                            _ => false,
                        };
                        if ok {
                            handles.push(d.handle);
                        }
                    }
                }
                if handles.is_empty() {
                    return R::Error(
                        "add_hatch: none of the given indices is a closed boundary".into(),
                    );
                }
                let hd: DObject = cad_kernel::Hatch {
                    boundary_handles: handles,
                    pattern: pat,
                }
                .into();
                let i = doc.push(hd);
                R::Ok(i)
            }
            Op::HatchAt { .. } => R::Indices(Vec::new()),
            Op::HatchPatternsGet => R::Patterns(
                cad_kernel::patterns::PATTERN_NAMES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
        }
    }

    /// Parse a script hatch-pattern name: "SOLID" (any case) → Solid; a
    /// catalog name (case-insensitive) → Pattern with scale 1, angle 0.
    /// Unknown names fail loudly with the catalog list.
    fn script_hatch_pattern(&self, name: &str) -> Result<HatchPattern, String> {
        let trimmed = name.trim();
        if trimmed.eq_ignore_ascii_case("solid") {
            return Ok(HatchPattern::Solid);
        }
        let canonical = cad_kernel::patterns::PATTERN_NAMES
            .iter()
            .find(|n| n.eq_ignore_ascii_case(trimmed))
            .map(|s| s.to_string());
        match canonical {
            Some(name) => Ok(HatchPattern::Pattern {
                name,
                scale: 1.0,
                angle_deg: 0.0,
            }),
            None => Err(format!(
                "no hatch pattern '{}' — available: {}",
                trimmed,
                cad_kernel::patterns::PATTERN_NAMES.join(", ")
            )),
        }
    }

    /// Build + push one hatch dobject from boundary handles (script path —
    /// mirrors apply_hatch's push; snapshot taken by the caller's undo
    /// machinery). Returns the new hatch's index.
    fn script_commit_hatch(
        &mut self,
        handles: Vec<cad_kernel::Handle>,
        pattern: HatchPattern,
        skipped: usize,
    ) -> usize {
        self.snapshot_doc();
        let label = match &pattern {
            HatchPattern::Solid => "SOLID".to_string(),
            HatchPattern::Pattern { name, .. } => name.clone(),
        };
        let loop_count = handles.len();
        let mut hd: DObject = cad_kernel::Hatch {
            boundary_handles: handles,
            pattern,
        }
        .into();
        self.stamp_fresh_style(&mut hd.style);
        self.doc.push(hd);
        self.gpu_dirty = true;
        self.index_dirty = true;
        let idx = self.doc.dobjects.len() - 1;
        self.history.push(format!(
            "  + hatch ({}): {} boundary loop(s){}",
            label,
            loop_count,
            if skipped > 0 {
                format!("  ({} non-boundary dobject(s) skipped)", skipped)
            } else {
                String::new()
            },
        ));
        idx
    }

    /// P1 — widen a script's explicit index set with the boundaries of any
    /// hatch in it (a hatch transforms via its boundary dobjects).
    fn script_transform_targets(&self, indices: &[usize]) -> Vec<usize> {
        let mut targets: Vec<usize> = Vec::new();
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for &i in indices {
            if i < self.doc.dobjects.len() && seen.insert(i) {
                targets.push(i);
            }
        }
        let mut extra: Vec<cad_kernel::Handle> = Vec::new();
        for &i in &targets {
            if let Some(d) = self.doc.dobjects.get(i) {
                if let Geom::Hatch(h) = &d.geom {
                    extra.extend(h.boundary_handles.iter().copied());
                }
            }
        }
        for handle in extra {
            if let Some(bi) = self.doc.index_of_handle(handle) {
                if seen.insert(bi) {
                    targets.push(bi);
                }
            }
        }
        targets
    }

    /// P1 — apply an in-place geometry transform to the given entities
    /// (snapshot once; hatch boundaries included). Returns the count;
    /// errors loudly when NONE of the indices exist (rule 10).
    fn script_transform(
        &mut self,
        indices: &[usize],
        f: impl Fn(&Geom) -> Geom,
        label: &str,
    ) -> Result<usize, String> {
        let targets = self.script_transform_targets(indices);
        if targets.is_empty() {
            return Err("none of the given entity indices exist".into());
        }
        self.snapshot_doc();
        let n = targets.len();
        for &i in &targets {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                d.geom = f(&d.geom);
            }
        }
        if n > 0 {
            self.history
                .push(format!("  python: {} {} dobject(s)", label, n));
        }
        self.intersections.clear();
        self.index_dirty = true;
        self.gpu_dirty = true;
        Ok(n)
    }

    /// P1 — apply a per-entity STYLE change (snapshot once, GPU invalidate).
    /// Returns the count touched; errors when none exist (rule 10).
    fn script_style_set(
        &mut self,
        indices: &[usize],
        mut f: impl FnMut(&mut DObject),
    ) -> Result<usize, String> {
        if indices.iter().all(|&i| i >= self.doc.dobjects.len()) {
            return Err("none of the given entity indices exist".into());
        }
        self.snapshot_doc();
        let mut n = 0;
        for &i in indices {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                f(d);
                n += 1;
            }
        }
        self.gpu_dirty = true;
        Ok(n)
    }

    /// Index the NEXT `add_dobject` will land at.
    fn next_add_index(&self) -> usize {
        self.doc.dobjects.len()
    }

    /// Owned snapshot of one dobject for the script boundary (D7).
    pub(super) fn entity_snapshot(&self, d: &DObject) -> cad_script::Entity {
        let (color, linetype, lineweight, visible) = self.entity_style_summary(d);
        cad_script::Entity {
            handle: d.handle,
            layer: self
                .doc
                .layers
                .get(d.style.layer)
                .map(|l| l.name.clone())
                .unwrap_or_default(),
            color,
            linetype,
            lineweight,
            visible,
            geom: d.geom.clone(),
            style: d.style,
        }
    }

    /// RESOLVED style of one dobject, script-facing (P1): color as
    /// `"aci N"` / `"bylayer"` / `"byblock"` / `"#RRGGBB"`, linetype as its
    /// NAME (ByLayer/Continuous → the layer's), lineweight in mm, visible.
    fn entity_style_summary(&self, d: &DObject) -> (String, String, f32, bool) {
        doc_entity_style_summary(&self.doc, d)
    }

    /// A `Color` → script-facing string (no layer resolution — the caller
    /// resolves ByLayer first).
    pub(super) fn script_color_string(c: Color, doc: &Document) -> String {
        match c {
            Color::ByLayer => "bylayer".into(),
            Color::ByBlock => "byblock".into(),
            Color::Aci(n) => format!("aci {}", n),
            Color::TrueColorRef(idx) => match c.rgb_bytes(&doc.truecolors) {
                Some((r, g, b)) => format!("#{:02X}{:02X}{:02X}", r, g, b),
                None => format!("truecolor {}", idx),
            },
        }
    }

    /// Snapshot once, then add — ONE undo entry for a single-object script
    /// add (the same seam every canvas draw uses).

    /// Commit one xline at (base, dir) and stay armed for the next base
    /// click (place-multiple). Degenerate direction fails visibly.
    pub(super) fn commit_xline(&mut self, base: Vec2, dir: Vec2) {
        let x = cad_kernel::Xline::new(base, dir);
        self.add_dobject_undoable(Geom::Xline(x), "canvas");
        self.history.push(format!(
            "  + xline @ ({:.3},{:.3}) angle={:.3}\u{00B0}",
            base.x,
            base.y,
            x.dir.angle().to_degrees()
        ));
        self.xline_state = XlineState::WaitingForBase;
        self.set_prompt("xline: pick base point  [H/V/A/Off  Esc exits]".to_string());
    }

    /// Commit one ray at (base, dir) and stay armed for the next base
    /// click (place-multiple, like xline).
    pub(super) fn commit_ray(&mut self, base: Vec2, dir: Vec2) {
        let r = cad_kernel::Ray::new(base, dir);
        self.add_dobject_undoable(Geom::Ray(r), "canvas");
        self.history.push(format!(
            "  + ray @ ({:.3},{:.3}) angle={:.3}\u{00B0}",
            base.x,
            base.y,
            r.dir.angle().to_degrees()
        ));
        self.ray_state = RayState::WaitingForBase;
        self.set_prompt("ray: pick base point  [H/V/A  Esc exits]".to_string());
    }

    /// Handle the ray sub-options (H/V/A) from the command line while the
    /// ray flow is armed. Returns true when consumed.
    pub(super) fn ray_suboption(&mut self, raw: &str) -> bool {
        let low = raw.trim().to_ascii_lowercase();
        let state = self.ray_state.clone();
        match state {
            RayState::Off => false,
            RayState::WaitingForBase | RayState::WaitingForDir { .. } => {
                let base = match state {
                    RayState::WaitingForDir { base } => base,
                    _ => Vec2::ZERO,
                };
                if low == "h" || low == "horizontal" {
                    self.commit_ray(base, Vec2::new(1.0, 0.0));
                    true
                } else if low == "v" || low == "vertical" {
                    self.commit_ray(base, Vec2::new(0.0, 1.0));
                    true
                } else if low == "a" || low.starts_with("a") {
                    let deg = raw.trim()[1..].trim().parse::<f64>().ok().or_else(|| {
                        raw.trim()
                            .split_whitespace()
                            .nth(1)
                            .and_then(|t| t.parse::<f64>().ok())
                    });
                    match deg {
                        Some(d) => {
                            let rad = d.to_radians();
                            self.commit_ray(base, Vec2::new(rad.cos(), rad.sin()));
                            true
                        }
                        None => {
                            self.fail_op("ray: angle needs a value, e.g. a45");
                            false
                        }
                    }
                } else {
                    false
                }
            }
        }
    }

    /// Handle the xline sub-options (H/V/A/Off) while the xline flow is
    /// armed. Returns true when consumed.
    pub(super) fn xline_suboption(&mut self, raw: &str) -> bool {
        let low = raw.trim().to_ascii_lowercase();
        let state = self.xline_state.clone();
        match state {
            XlineState::Off => false,
            XlineState::WaitingForBase | XlineState::WaitingForDir { .. } => {
                let base = match state {
                    XlineState::WaitingForDir { base } => base,
                    _ => Vec2::ZERO,
                };
                if low == "off" || low == "o" {
                    self.fail_op("xline: offset option needs a direction first — place the base + direction, then use offset");
                    true
                } else if low == "h" || low == "horizontal" {
                    self.commit_xline(base, Vec2::new(1.0, 0.0));
                    true
                } else if low == "v" || low == "vertical" {
                    self.commit_xline(base, Vec2::new(0.0, 1.0));
                    true
                } else if low == "a" || low.starts_with("a") {
                    let deg = raw.trim()[1..].trim().parse::<f64>().ok().or_else(|| {
                        raw.trim()
                            .split_whitespace()
                            .nth(1)
                            .and_then(|t| t.parse::<f64>().ok())
                    });
                    match deg {
                        Some(d) => {
                            let rad = d.to_radians();
                            self.commit_xline(base, Vec2::new(rad.cos(), rad.sin()));
                            true
                        }
                        None => {
                            self.fail_op("xline: angle needs a value, e.g. a45");
                            false
                        }
                    }
                } else {
                    false
                }
            }
        }
    }

    /// Commit one donut (center + outer + inner radii) and stay armed for
    /// the next one (place-multiple). Invalid radii fail visibly and the
    /// flow returns to the OUTER click.
    pub(super) fn commit_donut(&mut self, center: Vec2, outer: f64, inner: f64) {
        if outer < 1e-9 {
            self.fail_op("donut: outer radius is zero");
            self.donut_state = DonutState::WaitingOuter { center };
            self.set_prompt("donut: click OUTER radius point  [Esc exits]".to_string());
            return;
        }
        if inner >= outer - 1e-9 {
            self.fail_op("donut: inner radius must be smaller than outer");
            self.donut_state = DonutState::WaitingOuter { center };
            self.set_prompt("donut: click OUTER radius point  [Esc exits]".to_string());
            return;
        }
        let d = cad_kernel::Donut::new(center, inner, outer);
        self.add_dobject_undoable(Geom::Donut(d), "canvas");
        self.history.push(format!(
            "  + donut @ ({:.3},{:.3})  outer={:.3} inner={:.3}",
            center.x, center.y, outer, inner
        ));
        self.donut_state = DonutState::WaitingCenter;
        self.set_prompt(
            "donut: click CENTER, then outer radius, then inner radius  [Esc exits]".to_string(),
        );
    }

    /// Commit one wipeout rectangle between two opposite corners. Degenerate
    /// rects fail visibly. Single placement (Esc to re-arm).
    pub(super) fn commit_wipeout(&mut self, a: Vec2, b: Vec2) {
        let (mn, mx) = (
            Vec2::new(a.x.min(b.x), a.y.min(b.y)),
            Vec2::new(a.x.max(b.x), a.y.max(b.y)),
        );
        if (mx.x - mn.x) < 1e-9 || (mx.y - mn.y) < 1e-9 {
            self.wipeout_state = WipeoutState::WaitingFirstCorner;
            self.fail_op("wipeout: degenerate rectangle (zero width or height)");
            self.set_prompt("wipeout: pick first corner  [Esc exits]".to_string());
            return;
        }
        let pts = vec![
            Vec2::new(mn.x, mn.y),
            Vec2::new(mx.x, mn.y),
            Vec2::new(mx.x, mx.y),
            Vec2::new(mn.x, mx.y),
        ];
        self.add_dobject_undoable(Geom::Wipeout(cad_kernel::Wipeout { pts }), "canvas");
        self.history.push(format!(
            "  + wipeout @ ({:.3},{:.3})..({:.3},{:.3})",
            mn.x, mn.y, mx.x, mx.y
        ));
        self.wipeout_state = WipeoutState::Off;
        self.clear_prompt();
    }

    /// Commit one center mark at `pos`. Clicking a circle/arc sizes the
    /// mark to ~18% of its radius; a typed size override wins; empty
    /// clicks use 0.5 world units. Place-multiple.
    pub(super) fn commit_centermark_at(&mut self, pos: Vec2, size_override: Option<f64>) {
        let tol = (self.env.PkBxSz.max(8) as f64) / (self.scale as f64).max(1e-6);
        let hit = self
            .nearest_entity_under(pos, tol)
            .and_then(|i| self.doc.dobjects.get(i))
            .map(|d| d.geom.clone());
        // A click ON a circle/arc's CENTRE (or anywhere inside it) should
        // still size to that entity — the edge-distance pick above misses
        // it (distance is measured to the rim). Sweep for a circle/arc
        // whose CENTER is within tolerance of the click.
        let hit = hit.or_else(|| {
            self.doc.dobjects.iter().rev().find_map(|d| match &d.geom {
                Geom::Circle(c) if (c.center - pos).len() <= tol.max(c.radius * 0.5) => {
                    Some(Geom::Circle(*c))
                }
                Geom::Arc(a) if (a.center - pos).len() <= tol.max(a.radius * 0.5) => {
                    Some(Geom::Arc(*a))
                }
                Geom::Ellipse(e) if (e.center - pos).len() <= tol.max(e.semi_major() * 0.5) => {
                    Some(Geom::Ellipse(*e))
                }
                Geom::EllipseArc(ea)
                    if (ea.ellipse.center - pos).len()
                        <= tol.max(ea.ellipse.semi_major() * 0.5) =>
                {
                    Some(Geom::EllipseArc(*ea))
                }
                _ => None,
            })
        });
        let size = match size_override {
            Some(s) => s.max(1e-6),
            None => match hit {
                Some(Geom::Circle(c)) => (c.radius * 0.18).max(1e-3),
                Some(Geom::Arc(a)) => (a.radius * 0.18).max(1e-3),
                Some(Geom::Ellipse(e)) => (e.semi_major() * 0.18).max(1e-3),
                Some(Geom::EllipseArc(ea)) => (ea.ellipse.semi_major() * 0.18).max(1e-3),
                _ => 0.5, // default world size for an empty click
            },
        };
        self.add_dobject_undoable(
            Geom::CenterMark(cad_kernel::CenterMark {
                center: pos,
                size,
                rotation: 0.0,
            }),
            "canvas",
        );
        self.history.push(format!(
            "  + centermark @ ({:.3},{:.3}) size={:.3}",
            pos.x, pos.y, size
        ));
        self.set_prompt("centermark: click another circle/arc or point  [Esc exits]".to_string());
    }

    /// REGION — convert every closed curve in the selection into a Region
    /// dobject (same fill loop; one undo entry for the batch).
    pub(super) fn apply_region(&mut self) {
        let mut converted = 0usize;
        let mut skipped = 0usize;
        let decisions: Vec<(usize, Vec<Vec2>)> = self
            .selection
            .iter()
            .filter_map(|&idx| {
                let d = self.doc.dobjects.get(idx)?;
                let loop_pts = closed_dobject_polygon(&d.geom);
                if loop_pts.len() < 3 {
                    skipped += 1;
                    None
                } else {
                    Some((idx, loop_pts))
                }
            })
            .collect();
        if decisions.is_empty() {
            self.fail_op("region: no closed curve in selection (circle/ellipse/closed polyline/closed spline)");
            return;
        }
        self.snapshot_doc();
        for (idx, loop_pts) in decisions {
            if let Some(d) = self.doc.dobjects.get_mut(idx) {
                d.geom = Geom::Region(cad_kernel::Region { loop_pts });
                converted += 1;
            }
        }
        self.intersections.clear();
        self.index_dirty = true;
        self.gpu_dirty = true;
        self.history.push(format!(
            "  \u{229B} region: converted {converted} closed curve(s){}",
            if skipped > 0 {
                format!(" ({skipped} skipped — not closed)")
            } else {
                String::new()
            }
        ));
    }

    /// BOUNDARY/BPOLY — trace the closed region under `seed` (the SAME
    /// algorithm as hatch pick-point: tessellate → split → cluster →
    /// adjacency → ray-cast → walk) and emit the outer loop + islands as
    /// closed Polyline dobjects. Stays armed for more picks; Esc exits.
    /// A click outside any region fails visibly.
    pub(super) fn commit_boundary_at(&mut self, seed: Vec2) {
        let scope: Vec<usize> = (0..self.doc.dobjects.len()).collect();
        let text = self.text_hatch_geom(&scope);
        let Some(tb) = crate::hatch_trace::trace_boundary_at(&self.doc, seed, &text) else {
            self.history.push(format!(
                "  ! boundary: no closed region under ({:.3},{:.3})",
                seed.x, seed.y
            ));
            self.set_prompt("boundary: click INSIDE a closed region  [Esc exits]".to_string());
            return;
        };
        let mut loops: Vec<Vec<Vec2>> = Vec::new();
        loops.push(tb.outer);
        loops.extend(tb.islands);
        // One undo entry for the whole click.
        self.snapshot_doc();
        let mut made = 0usize;
        for lp in loops {
            if lp.len() < 4 {
                continue;
            } // needs ≥3 real vertices
            let mut pts: Vec<Vec2> = lp;
            if (pts[0] - pts[pts.len() - 1]).len() < 1e-6 {
                pts.pop(); // drop the closing repeat
            }
            if pts.len() < 3 {
                continue;
            }
            let pl = Geom::Polyline(Polyline {
                vertices: pts
                    .iter()
                    .map(|p| PolyVertex {
                        pos: *p,
                        bulge: 0.0,
                    })
                    .collect(),
                closed: true,
                widths: Vec::new(),
            });
            self.add_dobject(pl, "boundary");
            made += 1;
        }
        if made == 0 {
            self.rollback_doc();
            self.history
                .push("  ! boundary: traced region had no usable loops".into());
            return;
        }
        self.history
            .push(format!("  \u{2194} boundary: {made} closed loop(s) traced"));
        self.set_prompt("boundary: click INSIDE another closed region  [Esc exits]".to_string());
    }

    /// Load an external drawing file into a snapshot Document. RSM or DXF
    /// by extension; `None` on read error.
    fn load_external_doc(&self, path: &str) -> Option<cad_kernel::Document> {
        let bytes = std::fs::read(path).ok()?;
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".dxf") {
            cad_io::dxf::read_dxf(&String::from_utf8_lossy(&bytes)).ok()
        } else {
            cad_io::rsm::read_rsm(&bytes).ok()
        }
    }

    /// XREF dispatch — attach / list / detach / reload.
    pub(super) fn apply_xref(&mut self, args: &[String]) {
        if args.is_empty() {
            let n = self
                .doc
                .dobjects
                .iter()
                .filter(|d| matches!(d.geom, Geom::Xref(_)))
                .count();
            let names: Vec<String> = self
                .doc
                .dobjects
                .iter()
                .filter_map(|d| match &d.geom {
                    Geom::Xref(x) => Some(format!("{} -> {}", x.name, x.path)),
                    _ => None,
                })
                .collect();
            if names.is_empty() {
                self.history
                    .push("xref: no references  [attach <path> to add one]".into());
            } else {
                self.history
                    .push(format!("xref: {n} reference(s): {}", names.join("; ")));
            }
            return;
        }
        match args[0].to_ascii_lowercase().as_str() {
            "attach" | "a" => {
                let Some(path) = args.get(1) else {
                    self.fail_op("xref: attach needs a file path");
                    return;
                };
                let Some(doc) = self.load_external_doc(path) else {
                    self.fail_op(format!("xref: cannot read '{path}' (RSM or DXF)"));
                    return;
                };
                let name = std::path::Path::new(path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                self.xref_pending = Some(cad_kernel::Xref {
                    name,
                    path: path.clone(),
                    insert: Vec2::ZERO,
                    scale: 1.0,
                    rotation: 0.0,
                    cached: doc.dobjects,
                });
                self.set_prompt("xref: click the insertion point  [Esc cancels]".to_string());
            }
            "detach" | "d" => {
                let Some(name) = args.get(1) else {
                    self.fail_op("xref: detach needs a name");
                    return;
                };
                self.snapshot_doc();
                let before = self.doc.dobjects.len();
                self.doc.dobjects.retain(|d| {
                    !matches!(&d.geom,
                    Geom::Xref(x) if x.name.eq_ignore_ascii_case(name))
                });
                let removed = before - self.doc.dobjects.len();
                if removed == 0 {
                    self.rollback_doc();
                    self.fail_op(format!("xref: no reference named '{name}'"));
                } else {
                    self.index_dirty = true;
                    self.gpu_dirty = true;
                    self.history.push(format!(
                        "xref: detached {removed} reference(s) named '{name}'"
                    ));
                }
            }
            "reload" | "r" => {
                let Some(name) = args.get(1) else {
                    self.fail_op("xref: reload needs a name");
                    return;
                };
                self.snapshot_doc();
                // Collect paths first (avoid a borrow of `self` while
                // iterating `self.doc.dobjects` mutably).
                let paths: Vec<String> = self
                    .doc
                    .dobjects
                    .iter()
                    .filter_map(|d| match &d.geom {
                        Geom::Xref(x) if x.name.eq_ignore_ascii_case(name) => Some(x.path.clone()),
                        _ => None,
                    })
                    .collect();
                if paths.is_empty() {
                    self.rollback_doc();
                    self.fail_op(format!("xref: no reference named '{name}'"));
                    return;
                }
                let mut changed = 0usize;
                for p in &paths {
                    if let Some(doc) = self.load_external_doc(p) {
                        for d in self.doc.dobjects.iter_mut() {
                            let Geom::Xref(x) = &mut d.geom else { continue };
                            if x.path == *p {
                                x.cached = doc.dobjects.clone();
                                changed += 1;
                            }
                        }
                    }
                }
                if changed == 0 {
                    self.rollback_doc();
                    self.fail_op(format!(
                        "xref: '{name}' file(s) unreadable — nothing reloaded"
                    ));
                } else {
                    self.index_dirty = true;
                    self.gpu_dirty = true;
                    self.history.push(format!(
                        "xref: reloaded {changed} reference(s) named '{name}'"
                    ));
                }
            }
            _ => self.fail_op(format!(
                "xref: unknown option '{}' — attach <path> | detach <name> | reload <name>",
                args[0]
            )),
        }
    }

    /// Commit the pending xref at `pos`, then stay armed for the next
    /// placement (place-multiple).
    pub(super) fn commit_xref_at(&mut self, pos: Vec2) {
        let Some(mut x) = self.xref_pending.take() else {
            return;
        };
        x.insert = pos;
        self.add_dobject_undoable(Geom::Xref(x.clone()), "canvas");
        self.history.push(format!(
            "  + xref '{}' @ ({:.3},{:.3})  ({} children)",
            x.name,
            pos.x,
            pos.y,
            x.cached.len()
        ));
        self.set_prompt("xref: click the insertion point  [Esc cancels]".to_string());
        self.xref_pending = Some(x);
    }

    /// WBLOCK — save the selection (or the whole drawing when the selection
    /// is empty) to its own .rsm/.dxf file via the Save dialog.
    pub(super) fn apply_wblock(&mut self) {
        let mut sub = cad_kernel::Document::default();
        sub.layers = self.doc.layers.clone();
        sub.linetypes = self.doc.linetypes.clone();
        sub.pens = self.doc.pens.clone();
        sub.truecolors = self.doc.truecolors.clone();
        sub.text_styles = self.doc.text_styles.clone();
        sub.dim_styles = self.doc.dim_styles.clone();
        sub.wall_styles = self.doc.wall_styles.clone();
        sub.plot_styles = self.doc.plot_styles.clone();
        sub.units = self.doc.units.clone();
        if self.selection.is_empty() {
            sub.dobjects = self.doc.dobjects.clone();
        } else {
            sub.dobjects = self
                .selection
                .iter()
                .filter_map(|&i| self.doc.dobjects.get(i).cloned())
                .collect();
        }
        if sub.dobjects.is_empty() {
            self.fail_op("wblock: nothing to write — select dobjects or draw first");
            self.clear_prompt();
            return;
        }
        self.open_file_dialog(FileDialogMode::Save, ".rsm");
        self.wblock_subdoc = Some(sub);
        self.save_dialog_purpose = 1;
        self.set_prompt("wblock: choose a filename for the selection  [Esc cancels]".to_string());
    }

    /// Save an ARBITRARY document to `path` (the shared writer used by
    /// `do_save` for the main doc and by WBLOCK for the sub-document).
    pub(super) fn save_doc_to(&mut self, path: &str, doc: &cad_kernel::Document) -> bool {
        let lower = path.to_ascii_lowercase();
        let bytes: Vec<u8> = if lower.ends_with(".dxf") {
            cad_io::dxf::write_dxf(doc).into_bytes()
        } else if lower.ends_with(".rsm") {
            cad_io::rsm::write_rsm(doc)
        } else {
            self.history.push(format!(
                "  ! save '{}': unknown extension (expected .dxf or .rsm)",
                path
            ));
            return false;
        };
        match std::fs::write(path, &bytes) {
            Ok(()) => {
                self.history
                    .push(format!("  saved '{}'  ({} bytes)", path, bytes.len()));
                if lower.ends_with(".dxf") {
                    for line in cad_io::dxf::dxf_export_degradations(doc) {
                        self.history.push(format!("  \u{00B7} {}", line));
                    }
                }
                true
            }
            Err(e) => {
                self.history.push(format!("  ! save '{}': {}", path, e));
                false
            }
        }
    }

    // ---- ATTDEF / ATTEDIT -------------------------------------------------

    /// ATTDEF start — enter the TAG, prompt + default on the command line.
    pub(super) fn attr_def_prompt(&mut self) {
        self.attr_def_flow = AttrDefFlow::AwaitingTag;
        self.set_prompt("attdef: enter TAG name  (Esc cancels)".to_string());
    }

    /// Commit an attribute definition slot at `pos`, then repeat (AutoCAD
    /// ATTDEF keeps arming the next definition).
    pub(super) fn commit_attdef_at(&mut self, pos: Vec2, tag: &str, prompt: &str, default: &str) {
        let tag = tag.trim();
        if tag.is_empty() {
            self.history.push("  ! attdef: empty tag skipped".into());
            self.attr_def_flow = AttrDefFlow::Off;
            self.clear_prompt();
            return;
        }
        let height = self.env.TxHt;
        self.add_dobject_undoable(
            Geom::AttrDef(cad_kernel::AttrDef {
                tag: tag.to_string(),
                prompt: prompt.trim().to_string(),
                default: default.trim().to_string(),
                position: pos,
                height,
                angle: 0.0,
                style: cad_kernel::TextStyleTable::STANDARD,
                visible: true,
            }),
            "canvas",
        );
        self.history.push(format!(
            "  + attdef: <{}> @ ({:.3},{:.3}) h={}",
            tag, pos.x, pos.y, height
        ));
        self.attr_def_flow = AttrDefFlow::Off;
        self.clear_prompt();
        // Stay armed for another definition (AutoCAD ATTDEF repeats).
        self.attr_def_prompt();
    }

    /// ATTEDIT start — wait for a pick.
    pub(super) fn attedit_start(&mut self) {
        self.attedit_state = AttEditState::WaitingForPick;
        self.set_prompt("attedit: click a block instance with attributes  [Esc exits]".to_string());
    }

    /// The dobject index of the block instance under `pos` whose definition
    /// carries at least one AttrDef (only those can be attribute-edited).
    pub(super) fn attedit_pick_block(&self, pos: Vec2) -> Option<usize> {
        let tol = (self.env.PkBxSz.max(8) as f64) / (self.scale as f64).max(1e-6);
        self.nearest_entity_under(pos, tol)
            .and_then(|i| match &self.doc.dobjects.get(i)?.geom {
                Geom::BlockRef(br) => {
                    let blk = self.doc.blocks.get(br.block)?;
                    if blk
                        .dobjects
                        .iter()
                        .any(|d| matches!(d.geom, Geom::AttrDef(_)))
                    {
                        Some(i)
                    } else {
                        None
                    }
                }
                _ => None,
            })
    }

    /// Open the attribute editor dialog for block instance `idx`.
    pub(super) fn attedit_open_for(&mut self, idx: usize) {
        let Some(d) = self.doc.dobjects.get(idx) else {
            self.fail_op("attedit: block instance vanished");
            self.attedit_state = AttEditState::Off;
            self.clear_prompt();
            return;
        };
        let Geom::BlockRef(br) = &d.geom else {
            self.fail_op("attedit: not a block instance");
            self.attedit_state = AttEditState::Off;
            self.clear_prompt();
            return;
        };
        let Some(blk) = self.doc.blocks.get(br.block) else {
            self.fail_op("attedit: block definition missing");
            self.attedit_state = AttEditState::Off;
            self.clear_prompt();
            return;
        };
        // Parallel by index: value i ↔ the i-th AttrDef in definition order.
        let values = blk
            .dobjects
            .iter()
            .filter_map(|d| match &d.geom {
                Geom::AttrDef(a) => Some(a.default.clone()),
                _ => None,
            })
            .collect();
        self.attedit_state = AttEditState::Editing { idx, values };
        self.set_prompt("attedit: edit values in the panel, then Apply  [Esc cancels]".to_string());
    }

    /// Write the edited values back onto instance `idx`'s BlockRef
    /// (parallel by index to the definition's AttrDefs). One undo step.
    pub(super) fn attedit_write_values(&mut self, idx: usize, tags: &[String], values: &[String]) -> bool {
        if self
            .doc
            .dobjects
            .get(idx)
            .map(|d| matches!(d.geom, Geom::BlockRef(_)))
            .unwrap_or(false)
        {
            self.snapshot_doc();
            if let Some(d) = self.doc.dobjects.get_mut(idx) {
                if let Geom::BlockRef(br) = &mut d.geom {
                    let n = tags.len();
                    br.attr_values = values.iter().take(n).cloned().collect();
                    while br.attr_values.len() < n {
                        br.attr_values.push(String::new());
                    }
                    self.history.push(format!(
                        "  \u{270E} attedit: updated {} attribute value(s) on block #{}",
                        tags.len(),
                        idx
                    ));
                    self.index_dirty = true;
                    self.gpu_dirty = true;
                    return true;
                }
            }
        }
        false
    }

    fn add_dobject_undoable(&mut self, geom: Geom, origin: &str) {
        self.snapshot_doc();
        self.add_dobject(geom, origin);
    }

    /// The docked Python REPL console (WP-SCRIPT slice 4).
    pub(super) fn render_py_console(&mut self, ctx: &egui::Context) {
        if !self.py_console_open {
            return;
        }
        let cfg = crate::dock::DockConfig {
            id: "py_console",
            title: "Python console",
            badge: None,
            dock_region: crate::dock::DockRegion::Bottom,
            alt_region: None,
            any_edge: false,
            strip_h: 0.0,
            dockable: true,
            rail_header: false,
            collapsible: false,
            size: 230.0,
            min: 110.0,
            max: 480.0,
            resizable: true,
            flush_body: false,
            float_w: 700.0,
            float_max_h_frac: 0.6,
        };
        let mut open = self.py_console_open;
        // Copy the dock state out so the body closure can borrow `self`.
        let mut dock_state = self.py_console_dock;
        crate::dock::HOST.show(ctx, &cfg, &mut dock_state, &mut open, |ui, _cap| {
            self.py_console_body(ui);
        });
        self.py_console_dock = dock_state;
        self.py_console_open = open;
    }

    /// Console body: output log, input row (Enter = run, ↑/↓ = recall), and
    /// the example-script row.
    fn py_console_body(&mut self, ui: &mut egui::Ui) {
        let busy = self.script.as_ref().is_some_and(|s| s.is_busy());
        // ---- output log (fills the panel above the input row) ----
        let input_h = 64.0;
        let log_h = (ui.available_height() - input_h).max(50.0);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), log_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        // SELECTABLE log (same pattern as the command
                        // history): a per-frame TextEdit copy so the user
                        // can drag-select and copy script output.
                        let mut text = self.py_console_log.join("\n");
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .id(egui::Id::new("py_console_log_text"))
                                .frame(false)
                                .desired_rows(self.py_console_log.len().max(1))
                                .font(egui::TextStyle::Monospace),
                        );
                    });
            },
        );
        ui.add_space(4.0);
        // ---- input row ----
        let mut submit = false;
        ui.horizontal(|ui| {
            let te = ui.add_sized(
                [ui.available_width() - 64.0, 22.0],
                egui::TextEdit::singleline(&mut self.py_console_input)
                    .id(egui::Id::new("py_console_input"))
                    .hint_text(if busy {
                        "python is running — Esc stops it"
                    } else {
                        "python code — Enter to run, ↑/↓ recall, Esc stops a run"
                    }),
            );
            if te.has_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    self.py_console_recall(-1);
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    self.py_console_recall(1);
                }
            }
            let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let run_btn =
                ui.add_enabled(!busy, egui::Button::new(if busy { "busy" } else { "Run" }));
            submit = enter || run_btn.clicked();
        });
        // ---- save-as-script row (slice 5: scripts live in scripts/) ----
        ui.horizontal(|ui| {
            ui.label("script:");
            let name_te = ui.add_sized(
                [130.0, 20.0],
                egui::TextEdit::singleline(&mut self.py_console_name)
                    .id(egui::Id::new("py_console_name"))
                    .hint_text("name"),
            );
            let save_btn = ui
                .button("Save as script")
                .on_hover_text("saves the input above to scripts/<name>.py");
            if ui
                .button("Editor")
                .on_hover_text("open the full script editor")
                .clicked()
            {
                self.py_editor_open_panel();
            }
            if ui
                .button("Guide")
                .on_hover_text("full scripting API reference (pyhelp)")
                .clicked()
            {
                self.open_scripting_doc();
            }
            let entered = name_te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if save_btn.clicked() || entered {
                self.save_py_script();
            }
        });
        // ---- example scripts ----
        ui.horizontal(|ui| {
            ui.label("examples:");
            if self.py_examples.is_empty() {
                ui.weak("none found (drop .py files in scripts/ and refresh)");
            } else {
                let sel = self.py_example_sel.min(self.py_examples.len() - 1);
                egui::ComboBox::from_id_salt("py_examples")
                    .selected_text(&self.py_examples[sel].0)
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for (i, (n, _)) in self.py_examples.iter().enumerate() {
                            ui.selectable_value(&mut self.py_example_sel, i, n);
                        }
                    });
                if ui.button("Run file").clicked() {
                    self.py_console_run_example();
                }
            }
            if ui
                .button("refresh")
                .on_hover_text("rescan scripts/")
                .clicked()
            {
                self.scan_py_examples();
            }
        });
        if submit {
            self.py_console_submit();
            // Keep typing right away (Enter surrendered the TextEdit's focus).
            ui.ctx()
                .memory_mut(|m| m.request_focus(egui::Id::new("py_console_input")));
        }
    }

    /// ↑/↓ recall over submitted lines. `dir` = -1 (older) / +1 (newer).
    fn py_console_recall(&mut self, dir: i32) {
        if self.py_console_hist.is_empty() {
            return;
        }
        let n = self.py_console_hist.len();
        let next = match self.py_console_hist_idx {
            None => {
                if dir < 0 {
                    Some(n - 1)
                } else {
                    None // at the newest edge already
                }
            }
            Some(i) => {
                let j = i as i64 + dir as i64;
                if j < 0 {
                    self.py_console_hist_idx = None;
                    self.py_console_input.clear();
                    return;
                }
                if j >= n as i64 {
                    self.py_console_hist_idx = None;
                    self.py_console_input.clear();
                    return;
                }
                Some(j as usize)
            }
        };
        if let Some(i) = next {
            self.py_console_hist_idx = Some(i);
            self.py_console_input = self.py_console_hist[i].clone();
        }
    }

    /// Submit the console input line to the script worker.
    fn py_console_submit(&mut self) {
        let code = self.py_console_input.trim().to_string();
        if code.is_empty() {
            return;
        }
        if self.py_console_hist.last() != Some(&code) {
            self.py_console_hist.push(code.clone());
        }
        self.py_console_hist_idx = None;
        self.py_log(format!(">>> {}", code));
        self.clear_script_preview();
        self.script
            .get_or_insert_with(cad_script::ScriptEngine::new)
            .submit_text(code);
        self.py_console_input.clear();
    }

    /// Run the selected example script file.
    fn py_console_run_example(&mut self) {
        let Some((name, path)) = self
            .py_examples
            .get(
                self.py_example_sel
                    .min(self.py_examples.len().saturating_sub(1)),
            )
            .cloned()
        else {
            return;
        };
        self.py_log(format!(">>> pyfile {} ({})", name, path.display()));
        self.clear_script_preview();
        self.script
            .get_or_insert_with(cad_script::ScriptEngine::new)
            .submit_file(path);
    }

    /// Scan `scripts/*.py` for the console's example list. Cheap one-shot
    /// directory read — only on console open / refresh, never per frame.
    pub(super) fn scan_py_examples(&mut self) {
        self.py_examples.clear();
        if let Ok(rd) = std::fs::read_dir(Self::script_dir()) {
            let mut found: Vec<(String, std::path::PathBuf)> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "py"))
                .map(|p| {
                    let name = p
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (name, p)
                })
                .collect();
            found.sort();
            self.py_examples = found;
        }
        if self.py_example_sel >= self.py_examples.len() {
            self.py_example_sel = 0;
        }
    }

    /// The scripts folder — the single home of named scripts (slice 5).
    fn script_dir() -> std::path::PathBuf {
        std::path::PathBuf::from("scripts")
    }

    /// Candidate script folders: `./scripts` (dev / repo-root launches),
    /// then `<exe dir>/scripts` (a bundle next to the binary).
    fn script_dirs() -> Vec<std::path::PathBuf> {
        let mut dirs = vec![Self::script_dir()];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                dirs.push(dir.join("scripts"));
            }
        }
        dirs
    }

    /// Resolve a `run <name>` target to `(stem, scripts/<stem>.py)`. Accepts
    /// the bare name or `name.py`; falls back to a case-insensitive match.
    /// Searches `./scripts` then the exe's directory.
    pub(super) fn resolve_script(name: &str) -> Option<(String, std::path::PathBuf)> {
        let raw = name.trim();
        if raw.is_empty() {
            return None;
        }
        for dir in Self::script_dirs() {
            let mut p = dir.join(raw);
            if p.extension().is_none() {
                p.set_extension("py");
            }
            if p.is_file() {
                let stem = p.file_stem()?.to_string_lossy().into_owned();
                return Some((stem, p));
            }
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.filter_map(|e| e.ok()) {
                    let path = e.path();
                    if path.extension().is_some_and(|x| x == "py") {
                        let stem = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if stem.eq_ignore_ascii_case(raw) {
                            return Some((stem, path));
                        }
                    }
                }
            }
        }
        None
    }

    pub(super) fn create_ctb_test_scene(&mut self) {
        use cad_kernel::geom::{
            Arc, Circle, Ellipse, EllipseArc, Hatch, HatchPattern, Line, Point, PolyVertex,
            Polyline, Spline, Wall,
        };
        use cad_kernel::layout::{Layout, ViewportData, ViewportGeom};
        use cad_kernel::plotstyle::{
            EndStyle, JoinStyle, Orientation, PaperSize, PlotLinetype, PlotStyleTable, PlotWidth,
        };
        use cad_kernel::{Color, DObject, Geom, Style, Vec2};

        let mut doc = Document::default();
        doc.linetypes
            .linetypes
            .push(cad_kernel::linetype::Linetype::new("DASH", &[8.0, -4.0]));
        let dash_id = (doc.linetypes.linetypes.len() - 1) as u32;

        let aci = |c: u8| Style {
            color: Color::Aci(c),
            ..Style::default()
        };
        let add = |doc: &mut Document, g: Geom, st: Style| -> cad_kernel::Handle {
            let d = DObject::with_style(g, st);
            let h = d.handle;
            doc.push(d);
            h
        };

        // ACI 1 — red: Square caps / Miter joins (2 mm via the CTB pen).
        add(
            &mut doc,
            Geom::Line(Line {
                a: Vec2::new(-110.0, -20.0),
                b: Vec2::new(-30.0, 60.0),
            }),
            aci(1),
        );
        add(
            &mut doc,
            Geom::Polyline(Polyline {
                vertices: vec![
                    PolyVertex {
                        pos: Vec2::new(20.0, 40.0),
                        bulge: 0.0,
                    },
                    PolyVertex {
                        pos: Vec2::new(45.0, 65.0),
                        bulge: 0.0,
                    },
                    PolyVertex {
                        pos: Vec2::new(70.0, 40.0),
                        bulge: 0.0,
                    },
                    PolyVertex {
                        pos: Vec2::new(95.0, 65.0),
                        bulge: 0.0,
                    },
                ],
                closed: false,
                widths: Vec::new(),
            }),
            aci(1),
        );
        // ACI 3 — green: Butt caps / Bevel joins.
        add(
            &mut doc,
            Geom::Circle(Circle {
                center: Vec2::new(-70.0, 30.0),
                radius: 20.0,
            }),
            aci(3),
        );
        add(
            &mut doc,
            Geom::Arc(Arc {
                center: Vec2::new(110.0, 0.0),
                radius: 28.0,
                start_angle: 0.5,
                sweep_angle: 2.2,
            }),
            aci(3),
        );
        // Closed square + a solid AND a pattern hatch on it.
        let sq = add(
            &mut doc,
            Geom::Polyline(Polyline {
                vertices: vec![
                    PolyVertex {
                        pos: Vec2::new(-30.0, -55.0),
                        bulge: 0.0,
                    },
                    PolyVertex {
                        pos: Vec2::new(10.0, -55.0),
                        bulge: 0.0,
                    },
                    PolyVertex {
                        pos: Vec2::new(10.0, -25.0),
                        bulge: 0.0,
                    },
                    PolyVertex {
                        pos: Vec2::new(-30.0, -25.0),
                        bulge: 0.0,
                    },
                ],
                closed: true,
                widths: Vec::new(),
            }),
            aci(3),
        );
        add(
            &mut doc,
            Geom::Hatch(Hatch {
                boundary_handles: vec![sq],
                pattern: HatchPattern::Solid,
            }),
            aci(3),
        );
        add(
            &mut doc,
            Geom::Hatch(Hatch {
                boundary_handles: vec![sq],
                pattern: HatchPattern::Pattern {
                    name: "ANSI31".into(),
                    scale: 6.0,
                    angle_deg: 0.0,
                },
            }),
            aci(3),
        );
        // ACI 5 — blue: Round caps/joins.
        add(
            &mut doc,
            Geom::Ellipse(Ellipse {
                center: Vec2::new(-60.0, -30.0),
                major: Vec2::new(25.0, 12.0),
                ratio: 0.55,
            }),
            aci(5),
        );
        add(
            &mut doc,
            Geom::EllipseArc(EllipseArc {
                ellipse: Ellipse {
                    center: Vec2::new(30.0, -15.0),
                    major: Vec2::new(18.0, 9.0),
                    ratio: 0.6,
                },
                start_param: 0.3,
                sweep_param: 2.6,
            }),
            aci(5),
        );
        add(
            &mut doc,
            Geom::Spline(Spline::new_bspline(
                3,
                vec![
                    Vec2::new(-95.0, -45.0),
                    Vec2::new(-70.0, -80.0),
                    Vec2::new(-40.0, -35.0),
                    Vec2::new(-15.0, -70.0),
                ],
            )),
            aci(5),
        );
        add(
            &mut doc,
            Geom::Point(Point {
                location: Vec2::new(60.0, -40.0),
                style: 0,
                size: 0.0,
            }),
            aci(5),
        );
        // ACI 7 pen: 2 mm Square/Miter + a dashed linetype override — still
        // defined for manual testing. The scene's own entities use VISIBLE
        // colours (ACI 1/3/5) instead of white: white strokes and text are
        // invisible on the white paper under the CTB Test (UseObject → white)
        // and grayscale (white → white) CTBs — exactly like a real plot.
        let mut dashed_style = aci(1);
        dashed_style.linetype = dash_id;
        add(
            &mut doc,
            Geom::Line(Line {
                a: Vec2::new(60.0, 10.0),
                b: Vec2::new(110.0, 60.0),
            }),
            dashed_style,
        );
        add(
            &mut doc,
            Geom::Wall(Wall {
                start: Vec2::new(60.0, -60.0),
                end: Vec2::new(110.0, -20.0),
                thickness: 8.0,
                style: 0,
                bulge: 0.0,
            }),
            aci(3),
        );
        let mut t = cad_kernel::Text::empty();
        t.position = Vec2::new(-110.0, 85.0);
        t.height = 8.0;
        t.text = "CAP / JOIN TEST".into();
        add(&mut doc, Geom::Text(t), aci(5));

        // The test CTB — distinctive pens per ACI (see the doc comment).
        let mut table = PlotStyleTable::named("CTB Test");
        table.style_mut(1).lineweight = PlotWidth::Fixed(2.0);
        table.style_mut(1).end_style = EndStyle::Square;
        table.style_mut(1).join_style = JoinStyle::Miter;
        table.style_mut(3).lineweight = PlotWidth::Fixed(2.0);
        table.style_mut(3).end_style = EndStyle::Butt;
        table.style_mut(3).join_style = JoinStyle::Bevel;
        table.style_mut(5).lineweight = PlotWidth::Fixed(2.0);
        table.style_mut(5).end_style = EndStyle::Round;
        table.style_mut(5).join_style = JoinStyle::Round;
        table.style_mut(7).lineweight = PlotWidth::Fixed(2.0);
        table.style_mut(7).end_style = EndStyle::Square;
        table.style_mut(7).join_style = JoinStyle::Miter;
        table.style_mut(7).linetype = PlotLinetype::Id(dash_id);
        let ctb_path = ctb_folder().join("CTB Test.pst");
        if let Err(e) = std::fs::create_dir_all(ctb_folder()) {
            self.history
                .push(format!("  ! test CTB folder create failed: {}", e));
        }
        match cad_io::save_plot_table(&ctb_path, &table) {
            Ok(()) => self
                .history
                .push(format!("  test CTB saved → {}", ctb_path.display())),
            Err(e) => self
                .history
                .push(format!("  ! test CTB save failed: {}", e)),
        }

        // The layout: three viewports over A3 landscape, each showing the WHOLE
        // model with a different CTB — same content, three pen interpretations.
        let mut layout = Layout::new("CTB Test", PaperSize::A3, Orientation::Landscape);
        let model_c = Vec2::new(0.0, 5.0);
        let vp_specs: [((f64, f64), (f64, f64), &str); 3] = [
            ((16.0, 40.0), (140.0, 257.0), "CTB Test"),
            ((156.0, 40.0), (280.0, 257.0), "monochrome"),
            ((296.0, 40.0), (420.0, 257.0), "grayscale"),
        ];
        let n_vps = vp_specs.len();
        for &((x0, y0), (x1, y1), ctb) in &vp_specs {
            let vg = ViewportGeom {
                center: Vec2::new((x0 + x1) * 0.5, (y0 + y1) * 0.5),
                width: x1 - x0,
                height: y1 - y0,
                model_center: model_c,
                model_zoom: 1.0,
                model_scale: 0.55,
                frame_visible: true,
            };
            let ent = DObject::new(Geom::Viewport(vg.clone()));
            let h = ent.handle;
            layout.entities.push(ent);
            let mut vd = ViewportData::new((x0, y0), (x1, y1), (model_c.x, model_c.y), 1.0, 0.55);
            vd.shape_handle = Some(h);
            vd.ctb_name = Some(ctb.into());
            layout.viewports.push(vd);
        }
        // Paper camera: fit the A3 sheet on a typical ~1400×900 canvas.
        layout.camera.zoom = 3.0;
        layout.camera.pan_x = 700.0 - 210.0 * 3.0;
        layout.camera.pan_y = 450.0 - 148.5 * 3.0;
        let li = doc.layouts.len();
        doc.layouts.push(layout);

        // Swap in and reset the caches (mirrors the open-file path).
        self.doc = doc;
        self.selection.clear();
        self.selection_prev.clear();
        self.layout_selection.clear();
        self.selected = None;
        self.intersections.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.hatch_cache.clear();
        self.current_file = None;
        self.index = None;
        self.index_dirty = true;
        self.gpu_dirty = true;
        self.switch_to_tab(Some(li));
        self.history.push(format!(
            "  CTB test scene — layout 'CTB Test' ({} viewport(s): CTB Test / monochrome / grayscale); \
             File → Plot previews the pens; exports apply the real cap/join styles",
            n_vps));
    }

    // ---- selection helpers (list / select commands) -------------------

    /// DISPLAY-ONLY sub-command chips for an active selection session, shown in

    /// A failed operation (fork-local): appends the `! ` marker to the
    /// history transcript. (Upstream also surfaces it in a status bar this
    /// build does not have.)
    /// QDIM — batch linear dimensions over the selected lines/polylines
    /// (one aligned linear dim per straight segment, lifted above the
    /// geometry by 1.5 text heights, current dim style). Upstream cc95970.
    pub(super) fn apply_qdim(&mut self) {
        if self.selection.is_empty() {
            self.fail_op("qdim: empty selection — select lines to dimension");
            return;
        }
        let mut segs: Vec<(Vec2, Vec2)> = Vec::new();
        for &i in &self.selection {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            match &d.geom {
                Geom::Line(l) => segs.push((l.a, l.b)),
                Geom::Polyline(p) => {
                    for w in p.vertices.windows(2) {
                        if (w[1].pos - w[0].pos).len() > 1e-9 {
                            segs.push((w[0].pos, w[1].pos));
                        }
                    }
                    if p.closed && p.vertices.len() > 2 {
                        let a = p.vertices.last().unwrap().pos;
                        let b = p.vertices[0].pos;
                        if (b - a).len() > 1e-9 {
                            segs.push((a, b));
                        }
                    }
                }
                _ => {}
            }
        }
        if segs.is_empty() {
            self.fail_op("qdim: no lines or polylines in the selection");
            return;
        }
        self.snapshot_doc();
        let h = self
            .doc
            .dim_styles
            .styles
            .get(self.current_dim_style as usize)
            .map(|s| s.text_height * s.overall_scale)
            .unwrap_or(0.25);
        let lift = (h * 1.5).max(0.5);
        for (a, b) in &segs {
            let chord = *b - *a;
            let d = chord.len();
            if d < 1e-9 {
                continue;
            }
            let u = chord / d;
            let n = Vec2::new(-u.y, u.x);
            let mid = (*a + *b) * 0.5;
            let dimline_pos = mid + n * lift;
            self.doc.dobjects.push(DObject::new(Geom::Dimension(Dim {
                kind: DimKind::Linear {
                    p1: *a,
                    p2: *b,
                    dimline_pos,
                    ortho: cad_kernel::LinearOrtho::Aligned,
                },
                style: self.current_dim_style,
                text_override: None,
            })));
        }
        self.history.push(format!(
            "  ✓ qdim: {} dimension(s) placed over {} segment(s)",
            segs.len(),
            segs.len()
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.gpu_dirty = true;
    }

    /// PDMODE picker — a style/size grid (AutoCAD DDptype-lite). Choosing a
    /// tile sets `current_point_style` immediately (new Points stamp it); the
    /// size row edits `current_point_size` (negative % of view).
    pub(super) fn render_point_style_picker(&mut self, ctx: &egui::Context) {
        use crate::theme::color as tc;
        let mut open = true;
        egui::Window::new("Point Style")
            .order(egui::Order::Foreground)
            .id(egui::Id::new("pt_style_picker"))
            .collapsible(false)
            .resizable(false)
            .default_pos(egui::pos2(300.0, 200.0))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(240.0);
                ui.label(
                    egui::RichText::new("Style (PDMODE) — new points stamp it")
                        .weak()
                        .size(11.0),
                );
                const STYLES: [u8; 8] = [0, 2, 3, 32, 35, 64, 67, 96];
                let mut set_style: Option<u8> = None;
                egui::Grid::new("pt_style_grid")
                    .spacing(egui::vec2(6.0, 6.0))
                    .show(ui, |ui| {
                        for (k, pd) in STYLES.iter().enumerate() {
                            let (rect, resp) = ui
                                .allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
                            let is_cur = self.current_point_style == *pd;
                            ui.painter().rect(
                                rect,
                                egui::Rounding::same(4.0),
                                tc::SURFACE_0,
                                egui::Stroke::new(
                                    if is_cur { 2.0 } else { 1.0 },
                                    if is_cur { tc::ACCENT } else { tc::BORDER },
                                ),
                            );
                            paint_point_style(
                                ui.painter(),
                                rect.center(),
                                *pd,
                                7.0,
                                egui::Stroke::new(
                                    1.6,
                                    if is_cur { tc::ACCENT } else { tc::TEXT_PRIMARY },
                                ),
                            );
                            if resp.clicked() {
                                set_style = Some(*pd);
                            }
                            if resp.hovered() {
                                resp.on_hover_text(format!(
                                    "{} — PDMODE {}",
                                    point_style_name(*pd),
                                    pd
                                ));
                            }
                            if (k + 1) % 4 == 0 {
                                ui.end_row();
                            }
                        }
                    });
                if let Some(pd) = set_style {
                    self.current_point_style = pd;
                    self.history.push(format!(
                        "  point style → {} (PDMODE {})",
                        point_style_name(pd),
                        pd
                    ));
                }
                ui.add_space(6.0);
                ui.label("Size");
                // PDSIZE convention: negative = % of view height.
                let mut pct = (-self.current_point_size).max(0.01);
                let resp = ui.horizontal(|ui| {
                    ui.label("% of view:");
                    ui.add(
                        egui::DragValue::new(&mut pct)
                            .update_while_editing(false)
                            .speed(0.25)
                            .range(0.5..=50.0),
                    )
                });
                if resp.response.changed() {
                    self.current_point_size = -(pct as f32);
                }
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Positive PDSIZE = drawing units; 0 = default 5%.")
                        .small()
                        .weak(),
                );
            });
        if !open {
            self.point_style_picker_open = false;
        }
    }

    /// ATTEDIT attribute-value editor — a small floating panel listing the
    /// selected block instance's attribute tags with editable values. Apply
    /// writes them back into the instance's `BlockRef.attr_values`
    /// (parallel by index to the definition's AttrDef dobjects).
    pub(super) fn render_attedit_dialog(&mut self, ctx: &egui::Context) {
        let AttEditState::Editing { idx, values } = self.attedit_state.clone() else {
            return;
        };
        let mut values = values;
        // Pull the CURRENT tags from the definition each frame.
        let (tags, height): (Vec<String>, f64) = self
            .doc
            .dobjects
            .get(idx)
            .and_then(|d| match &d.geom {
                Geom::BlockRef(br) => self.doc.blocks.get(br.block).map(|blk| {
                    let tags: Vec<String> = blk
                        .dobjects
                        .iter()
                        .filter_map(|cd| match &cd.geom {
                            Geom::AttrDef(a) => Some(a.tag.clone()),
                            _ => None,
                        })
                        .collect();
                    let h = blk
                        .dobjects
                        .iter()
                        .filter_map(|cd| match &cd.geom {
                            Geom::AttrDef(a) => Some(a.height),
                            _ => None,
                        })
                        .next()
                        .unwrap_or(0.25);
                    (tags, h)
                }),
                _ => None,
            })
            .unwrap_or_default();
        let mut open = true;
        let mut apply = false;
        let mut close = false;
        egui::Window::new("Edit Attributes")
            .id(egui::Id::new("attedit_dialog"))
            .open(&mut open)
            .default_pos(egui::pos2(60.0, 60.0))
            .resizable(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(if tags.is_empty() {
                                "This block has no attribute definitions."
                            } else {
                                "Enter values for the block's attributes:"
                            })
                            .size(12.0),
                        );
                        ui.add_space(6.0);
                        for (i, tag) in tags.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("<{tag}>")).strong().size(12.0),
                                );
                                let v = values.get_mut(i).cloned().unwrap_or_default();
                                let mut v = v;
                                let resp = ui.add_sized(
                                    egui::vec2(ui.available_width() - 40.0, 22.0),
                                    egui::TextEdit::singleline(&mut v),
                                );
                                if resp.changed() {
                                    while values.len() <= i {
                                        values.push(String::new());
                                    }
                                    values[i] = v;
                                }
                            });
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Apply").clicked() {
                                apply = true;
                            }
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                        });
                        ui.label(
                            egui::RichText::new(format!(
                                "block instance #{} — text height {:.3}",
                                idx, height
                            ))
                            .color(crate::theme::color::TEXT_MUTED)
                            .size(10.5),
                        );
                    });
            });
        if !open || close {
            self.attedit_state = AttEditState::Off;
            self.clear_prompt();
            return;
        }
        if apply {
            if self.attedit_write_values(idx, &tags, &values) {
                self.attedit_state = AttEditState::Off;
                self.clear_prompt();
            }
        }
    }

    fn close_qselect_dialog(&mut self) {
        self.qselect.open = false;
        if self.current_prompt.starts_with("qselect") {
            self.clear_prompt();
        }
    }

    pub(super) fn apply_qselect(&mut self) {
        let q = self.qselect.clone();
        let kind = q.kind_filter.as_deref();
        let layer = q.layer_filter.as_deref();
        let aci = q.color_filter;
        let lt = q.linetype_filter.as_deref();
        let mut matches: Vec<usize> = Vec::new();
        for (i, d) in self.doc.dobjects.iter().enumerate() {
            let s = &d.style;
            let k_ok = kind.map_or(true, |k| dobject_kind_name(&d.geom) == k);
            let l_ok = layer.map_or(true, |l| {
                self.doc
                    .layers
                    .get(s.layer)
                    .map(|x| x.name.eq_ignore_ascii_case(l))
                    .unwrap_or(false)
            });
            let c_ok = aci.map_or(true, |a| match s.color {
                cad_kernel::color::Color::Aci(c) => c == a,
                _ => false,
            });
            let lt_ok = lt.map_or(true, |l| {
                self.doc
                    .linetypes
                    .get(s.linetype)
                    .map(|x| x.name.eq_ignore_ascii_case(l))
                    .unwrap_or(false)
            });
            if k_ok && l_ok && c_ok && lt_ok {
                matches.push(i);
            }
        }
        let n = self.doc.dobjects.len();
        if q.include {
            self.selection = matches.clone();
        } else {
            let m: std::collections::HashSet<usize> = matches.into_iter().collect();
            self.selection = (0..n).filter(|i| !m.contains(i)).collect();
        }
        self.selected = None;
        self.selection_prev = self.selection.clone();
        let label = if q.include {
            "selected"
        } else {
            "kept (excluded matches)"
        };
        self.history.push(format!(
            "  qselect: {} dobject(s) {}",
            self.selection.len(),
            label
        ));
    }

    pub(super) fn render_qselect_dialog(&mut self, ctx: &egui::Context) {
        if !self.qselect.open {
            return;
        }
        let cfg = crate::dock::DockConfig {
            id: "qselect",
            title: "QSELECT — filter selection",
            badge: None,
            dock_region: crate::dock::DockRegion::Right,
            alt_region: Some(crate::dock::DockRegion::Left),
            any_edge: false,
            strip_h: 0.0,
            dockable: false, // floating-only
            rail_header: true,
            collapsible: true,
            size: 300.0,
            min: 280.0,
            max: 420.0,
            resizable: false,
            flush_body: true,
            float_w: 300.0,
            float_max_h_frac: 0.9,
        };
        let mut state = self.qselect.dock_state.clone();
        let mut open = true;
        if matches!(state, crate::dock::DockState::Floating(_)) {
            crate::dock::HOST.show(ctx, &cfg, &mut state, &mut open, |ui, _cap| {
                egui::Frame::none()
                    .inner_margin(egui::Margin {
                        left: 16.0,
                        right: 16.0,
                        top: 12.0,
                        bottom: 14.0,
                    })
                    .show(ui, |ui| {
                        self.qselect_panel_body(ui);
                    });
            });
        }
        self.qselect.dock_state = state;
        if !open {
            self.close_qselect_dialog();
        }
    }

    fn qselect_panel_body(&mut self, ui: &mut egui::Ui) {
        use egui::RichText;
        let mut apply = false;
        ui.label(RichText::new("Filter the selection set").strong());
        ui.add_space(6.0);

        // Object type.
        let mut kinds: Vec<String> = vec!["All".into()];
        let mut seen = std::collections::HashSet::new();
        for d in &self.doc.dobjects {
            let k = dobject_kind_name(&d.geom).to_string();
            if seen.insert(k.clone()) {
                kinds.push(k);
            }
        }
        kinds.sort_by_key(|k| k.to_lowercase());
        let cur_k = self
            .qselect
            .kind_filter
            .clone()
            .unwrap_or_else(|| "All".into());
        ui.label("Object type");
        egui::ComboBox::from_id_salt("qs_kind")
            .selected_text(&cur_k)
            .show_ui(ui, |ui| {
                for k in &kinds {
                    if ui.selectable_label(cur_k == *k, k).clicked() {
                        self.qselect.kind_filter = if k == "All" { None } else { Some(k.clone()) };
                    }
                }
            });

        // Layer.
        let mut layers: Vec<String> = vec!["All".into()];
        layers.extend(self.doc.layers.layers.iter().map(|l| l.name.clone()));
        let cur_l = self
            .qselect
            .layer_filter
            .clone()
            .unwrap_or_else(|| "All".into());
        ui.label("Layer");
        egui::ComboBox::from_id_salt("qs_layer")
            .selected_text(&cur_l)
            .show_ui(ui, |ui| {
                for l in &layers {
                    if ui.selectable_label(cur_l == *l, l).clicked() {
                        self.qselect.layer_filter = if l == "All" { None } else { Some(l.clone()) };
                    }
                }
            });

        // Color (ACI values actually in use).
        let mut acis: Vec<u8> = Vec::new();
        for d in &self.doc.dobjects {
            if let cad_kernel::color::Color::Aci(c) = d.style.color {
                if !acis.contains(&c) {
                    acis.push(c);
                }
            }
        }
        acis.sort_unstable();
        let cur_c = self
            .qselect
            .color_filter
            .map(|c| format!("ACI {c}"))
            .unwrap_or_else(|| "All".into());
        ui.label("Color");
        egui::ComboBox::from_id_salt("qs_color")
            .selected_text(&cur_c)
            .show_ui(ui, |ui| {
                if ui.selectable_label(cur_c == "All", "All").clicked() {
                    self.qselect.color_filter = None;
                }
                for c in &acis {
                    let label = format!("ACI {c}");
                    if ui.selectable_label(cur_c == label, &label).clicked() {
                        self.qselect.color_filter = Some(*c);
                    }
                }
            });

        // Linetype.
        let mut lts: Vec<String> = vec!["All".into()];
        lts.extend(self.doc.linetypes.linetypes.iter().map(|l| l.name.clone()));
        let cur_t = self
            .qselect
            .linetype_filter
            .clone()
            .unwrap_or_else(|| "All".into());
        ui.label("Linetype");
        egui::ComboBox::from_id_salt("qs_lt")
            .selected_text(&cur_t)
            .show_ui(ui, |ui| {
                for t in &lts {
                    if ui.selectable_label(cur_t == *t, t).clicked() {
                        self.qselect.linetype_filter =
                            if t == "All" { None } else { Some(t.clone()) };
                    }
                }
            });

        ui.add_space(8.0);
        // Include / exclude.
        ui.radio_value(&mut self.qselect.include, true, "Include in new selection");
        ui.radio_value(
            &mut self.qselect.include,
            false,
            "Exclude from new selection",
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Apply").strong()).clicked() {
                apply = true;
            }
            if ui.button("Close").clicked() {
                self.qselect.open = false;
            }
        });
        if apply {
            self.apply_qselect();
        }
        ui.add_space(4.0);
        ui.small(format!("{} dobject(s) in drawing", self.doc.dobjects.len()));
    }

    pub(super) fn fail_op(&mut self, msg: impl Into<String>) {
        self.history.push(format!("  ! {}", msg.into()));
    }

    pub(super) fn run_script_command(&mut self, name: Option<String>, args: Vec<String>) {
        let Some(name) = name else {
            self.py_console_open = true;
            self.scan_py_examples();
            let list: Vec<String> = self.py_examples.iter().map(|(n, _)| n.clone()).collect();
            self.history.push(format!(
                "  run — scripts: {}  (type `run <name> [k=v …]`, or pick from Tools → Scripts)",
                if list.is_empty() {
                    "none".into()
                } else {
                    list.join(", ")
                },
            ));
            return;
        };
        self.scan_py_examples();
        self.clear_script_preview();
        match Self::resolve_script(&name) {
            Some((stem, path)) => {
                if args.is_empty() {
                    // No inputs → dialog: ask the engine for the declaration
                    // (metadata pass) and open the panel; the Meta reply
                    // fills the fields (or runs immediately when the script
                    // declares nothing).
                    self.script_param_dialog = Some(ScriptParamDialog {
                        name: stem.clone(),
                        path: path.clone(),
                        params: Vec::new(),
                        values: Vec::new(),
                        waiting_meta: true,
                        pos: egui::pos2(520.0, 160.0),
                    });
                    self.history.push(format!("  run> {}", stem));
                    self.script
                        .get_or_insert_with(cad_script::ScriptEngine::new)
                        .request_meta(path);
                } else if let Some(params) = Self::parse_named_script_args(&args) {
                    // Named inputs → run now. The meta reply is fetched
                    // first (invisibly — no dialog) so LENGTH values convert
                    // through the document's display unit.
                    let shown: Vec<String> =
                        params.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                    self.history
                        .push(format!("  run> {} {}", stem, shown.join(" ")));
                    self.script_pending_run = Some(PendingScriptRun {
                        name: stem.clone(),
                        path: path.clone(),
                        named: Some(params),
                        positional: None,
                    });
                    self.script
                        .get_or_insert_with(cad_script::ScriptEngine::new)
                        .request_meta(path);
                } else {
                    // Legacy positional inputs — same meta-first treatment
                    // for declared-order length conversion.
                    self.history
                        .push(format!("  run> {} {}", stem, args.join(" ")));
                    self.script_pending_run = Some(PendingScriptRun {
                        name: stem.clone(),
                        path: path.clone(),
                        named: None,
                        positional: Some(args),
                    });
                    self.script
                        .get_or_insert_with(cad_script::ScriptEngine::new)
                        .request_meta(path);
                }
                self.refocus_cmd = true;
            }
            None => {
                let list: Vec<String> = self.py_examples.iter().map(|(n, _)| n.clone()).collect();
                self.fail_op(format!(
                    "run: no script '{}' in scripts/{}",
                    name,
                    if list.is_empty() {
                        " (folder is empty — save one from the Python console)".into()
                    } else {
                        format!(" (available: {})", list.join(", "))
                    },
                ));
            }
        }
    }

    /// `run <name> k=v k2=v2 …` → the named inputs. `Some(_)` only when
    /// EVERY arg is a `k=v` pair (mixed forms are rejected loudly by the
    /// caller treating them as positional).
    pub(super) fn parse_named_script_args(args: &[String]) -> Option<Vec<(String, String)>> {
        if args.is_empty() || !args.iter().all(|a| a.contains('=')) {
            return None;
        }
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            let (k, v) = a.split_once('=')?;
            let k = k.trim();
            if k.is_empty() {
                return None;
            }
            out.push((k.to_string(), v.to_string()));
        }
        Some(out)
    }

    /// Fill the catalog-backed dropdown choices (linetype / layer / block /
    /// hatch pattern) from the LIVE document. Called when the dialog opens
    /// AND before a k=v / positional run converts its inputs, so both paths
    /// validate against the same catalogs.
    pub(super) fn fill_catalog_choices(&self, params: &mut [cad_script::ScriptParamMeta]) {
        for p in params {
            match p.ptype {
                cad_script::ParamType::Linetype => {
                    p.choices = self
                        .doc
                        .linetypes
                        .linetypes
                        .iter()
                        .map(|l| l.name.clone())
                        .collect();
                }
                cad_script::ParamType::Layer => {
                    p.choices = self
                        .doc
                        .layers
                        .layers
                        .iter()
                        .map(|l| l.name.clone())
                        .collect();
                }
                cad_script::ParamType::Block => {
                    p.choices = self
                        .doc
                        .blocks
                        .blocks
                        .iter()
                        .map(|b| b.name.clone())
                        .collect();
                }
                cad_script::ParamType::HatchPattern => {
                    p.choices = cad_kernel::patterns::PATTERN_NAMES
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                }
                _ => {}
            }
        }
    }

    /// Execute a pending `run <name> k=v …` / positional invocation now
    /// that its declaration arrived: LENGTH inputs convert display → scene
    /// units first (a bad length fails loudly and skips the run).
    pub(super) fn run_pending_script(&mut self, meta: Option<cad_script::ScriptMeta>) {
        let Some(pend) = self.script_pending_run.take() else {
            return;
        };
        match meta {
            Some(mut m) => {
                // Catalog dropdowns validate against the live document —
                // fill their choices before converting any input.
                self.fill_catalog_choices(&mut m.params);
                // Build (name, raw) pairs: named as typed, positional
                // mapped onto the declared order.
                let named: Option<Vec<(String, String)>> = pend.named;
                let raw: Vec<(String, String)> = match named {
                    Some(n) => n,
                    None => {
                        let args = pend.positional.unwrap_or_default();
                        m.params
                            .iter()
                            .zip(args.iter())
                            .map(|(p, a)| (p.name.clone(), a.clone()))
                            .collect()
                    }
                };
                // Convert lengths; abort loudly on the first bad value.
                let mut params = Vec::with_capacity(raw.len());
                for (k, v) in &raw {
                    match self.param_value_scene(&m.params, k, v) {
                        Ok(s) => params.push((k.clone(), s)),
                        Err(e) => {
                            self.fail_op(format!("run {}: {}", pend.name, e));
                            return;
                        }
                    }
                }
                let shown: Vec<String> = raw.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                self.py_log(format!(">>> run {} {}", pend.name, shown.join(" ")));
                self.clear_script_preview();
                self.script
                    .get_or_insert_with(cad_script::ScriptEngine::new)
                    .submit_script_with_params(pend.path, pend.name, Vec::new(), params);
            }
            None => {
                // No declaration (plain script) — pass the inputs through
                // untouched (legacy rasm.args semantics).
                self.py_log(format!(">>> run {}", pend.name));
                self.clear_script_preview();
                match (pend.named, pend.positional) {
                    (Some(named), _) => {
                        self.script
                            .get_or_insert_with(cad_script::ScriptEngine::new)
                            .submit_script_with_params(pend.path, pend.name, Vec::new(), named);
                    }
                    (None, Some(args)) => {
                        self.script
                            .get_or_insert_with(cad_script::ScriptEngine::new)
                            .submit_script(pend.path, pend.name, args);
                    }
                    (None, None) => {}
                }
            }
        }
        self.refocus_cmd = true;
    }

    pub(super) fn on_script_meta(&mut self, meta: Option<cad_script::ScriptMeta>) {
        let Some(mut dlg) = self.script_param_dialog.take() else {
            return;
        };
        if !dlg.waiting_meta {
            self.script_param_dialog = Some(dlg);
            return;
        }
        dlg.waiting_meta = false;
        match meta {
            Some(m) => {
                dlg.params = m.params;
                dlg.values = dlg
                    .params
                    .iter()
                    .map(|p| self.param_display_default(p))
                    .collect();
                if dlg.params.is_empty() {
                    let (name, path) = (dlg.name.clone(), dlg.path.clone());
                    self.history
                        .push(format!("  run> {} (no parameters — running)", name));
                    self.py_log(format!(">>> run {}", name));
                    self.script
                        .get_or_insert_with(cad_script::ScriptEngine::new)
                        .submit_script_with_params(path, name, Vec::new(), Vec::new());
                } else {
                    // Catalog-backed dropdowns (linetype / layer / block /
                    // hatch pattern) get their choices from the LIVE
                    // document at dialog time.
                    self.fill_catalog_choices(&mut dlg.params);
                    // Slice 5 preview: with declared inputs, start a ghost
                    // pass immediately so the DEFAULT result is visible while
                    // the user decides.
                    let params = match self.dialog_params_scene(&dlg) {
                        Ok(p) => p,
                        Err(e) => {
                            self.fail_op(format!("run {}: {}", dlg.name, e));
                            self.script_param_dialog = Some(dlg);
                            return;
                        }
                    };
                    let dname = dlg.name.clone();
                    let dpath = dlg.path.clone();
                    self.script_preview_start(dname, dpath, params);
                    self.script_param_dialog = Some(dlg);
                }
            }
            None => {
                // No rasm.main → a plain script: run it straight away.
                let (name, path) = (dlg.name.clone(), dlg.path.clone());
                self.history.push(format!(
                    "  run> {} (no declared parameters — running)",
                    name
                ));
                self.py_log(format!(">>> run {}", name));
                self.script
                    .get_or_insert_with(cad_script::ScriptEngine::new)
                    .submit_script(path, name, Vec::new());
            }
        }
    }

    /// The run-script parameter dialog (slice 5): one labeled, typed field
    /// per declared input, prefilled with the script's defaults. Run
    /// submits the values as `rasm.params`; X/Cancel just closes.
    pub(super) fn render_script_param_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.script_param_dialog.take() else {
            return;
        };
        // Slice 5 preview: restart the ghost pass when the values changed
        // (throttled; never while another job runs on the engine).
        if let Some(p) = &mut self.script_preview {
            let busy = self.script.as_ref().is_some_and(|s| s.is_busy());
            if p.dirty
                && !p.running
                && !p.cancelled
                && !busy
                && p.last_restart.elapsed() >= std::time::Duration::from_millis(300)
            {
                let params = match self.dialog_params_scene(&dlg) {
                    Ok(p) => p,
                    Err(e) => {
                        self.fail_op(format!("run {}: {}", dlg.name, e));
                        Vec::new()
                    }
                };
                if !params.is_empty() {
                    let dname = dlg.name.clone();
                    let dpath = dlg.path.clone();
                    self.script_preview_start(dname, dpath, params);
                }
            }
        }
        let mut do_run = false;
        let mut do_cancel = false;
        let mut keep = true;
        let mut drag_delta = egui::Vec2::ZERO;
        let title = format!("Run script — {}", dlg.name);
        egui::Window::new(title)
            .order(egui::Order::Foreground)
            .id(egui::Id::new("script_param_dialog"))
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .current_pos(dlg.pos)
            .frame(egui::Frame::none()
                .fill(crate::theme::color::SURFACE_1)
                .stroke(egui::Stroke::new(1.0, crate::theme::color::BORDER)))
            .show(ctx, |ui| {
                let hb = crate::dock::header_band(
                    ui, &format!("Run script — {}", dlg.name), None, true);
                if hb.close_clicked {
                    keep = false;
                }
                // Drag handle — the shared band, palette-style.
                if hb.band.dragged() {
                    drag_delta = hb.band.drag_delta();
                }
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        if dlg.waiting_meta {
                            ui.weak("reading the script's parameters…");
                        } else {
                            ui.set_min_width(340.0);
                            for i in 0..dlg.params.len() {
                                let p = &dlg.params[i];
                                let label = if p.help.is_empty() {
                                    p.name.clone()
                                } else {
                                    format!("{} — {}", p.name, p.help)
                                };
                                ui.horizontal(|ui| {
                                    ui.add_sized(
                                        [220.0, 20.0],
                                        egui::Label::new(&label).truncate(),
                                    );
                                    match p.ptype {
                                        cad_script::ParamType::Float => {
                                            let fallback =
                                                p.default.parse().unwrap_or(0.0);
                                            let mut v: f64 = dlg.values[i]
                                                .parse()
                                                .unwrap_or(fallback);
                                            let mut dv = egui::DragValue::new(&mut v)
                                                .update_while_editing(false)
                                                .speed(0.5);
                                            if let (Some(lo), Some(hi)) = (p.min, p.max) {
                                                dv = dv.range(lo..=hi);
                                            }
                                            if ui.add(dv).changed() {
                                                dlg.values[i] = format!("{}", v);
                                                self.script_preview_dirty();
                                            }
                                        }
                                        cad_script::ParamType::Length => {
                                            // Display units: the field shows
                                            // the value in the document's
                                            // unit; the submit paths convert
                                            // back to scene units.
                                            let fallback = self
                                                .param_display_default(p)
                                                .parse()
                                                .unwrap_or(0.0);
                                            let mut v: f64 = dlg.values[i]
                                                .parse()
                                                .unwrap_or(fallback);
                                            let unit = self.doc.units.name.clone();
                                            let mut dv = egui::DragValue::new(&mut v)
                                                .update_while_editing(false)
                                                .speed(0.5)
                                                .suffix(format!(" {}", unit));
                                            if let (Some(lo), Some(hi)) = (p.min, p.max) {
                                                let lo = self.doc.units.to_display(lo);
                                                let hi = self.doc.units.to_display(hi);
                                                dv = dv.range(lo..=hi);
                                            }
                                            if ui.add(dv).changed() {
                                                dlg.values[i] = format!("{}", v);
                                                self.script_preview_dirty();
                                            }
                                        }
                                        cad_script::ParamType::Int => {
                                            let fallback =
                                                p.default.parse().unwrap_or(0);
                                            let mut v: i64 = dlg.values[i]
                                                .parse()
                                                .unwrap_or(fallback);
                                            let mut dv = egui::DragValue::new(&mut v)
                                                .update_while_editing(false)
                                                .speed(1);
                                            if let (Some(lo), Some(hi)) = (p.min, p.max) {
                                                dv = dv.range((lo as i64)..=(hi as i64));
                                            }
                                            if ui.add(dv).changed() {
                                                dlg.values[i] = format!("{}", v);
                                                self.script_preview_dirty();
                                            }
                                        }
                                        cad_script::ParamType::Bool => {
                                            let mut v = matches!(
                                                dlg.values[i].to_ascii_lowercase().as_str(),
                                                "true" | "1" | "yes" | "on"
                                            );
                                            if ui.checkbox(&mut v, "").changed() {
                                                dlg.values[i] =
                                                    if v { "true" } else { "false" }.into();
                                                self.script_preview_dirty();
                                            }
                                        }
                                        cad_script::ParamType::Str => {
                                            if ui.add_sized(
                                                [120.0, 20.0],
                                                egui::TextEdit::singleline(
                                                    &mut dlg.values[i],
                                                ),
                                            ).changed()
                                            {
                                                self.script_preview_dirty();
                                            }
                                        }
                                        cad_script::ParamType::Choice
                                        | cad_script::ParamType::Linetype
                                        | cad_script::ParamType::Layer
                                        | cad_script::ParamType::Block
                                        | cad_script::ParamType::HatchPattern => {
                                            // A dropdown: declared choices
                                            // (`name: help [a, b, c]`) or a
                                            // LIVE catalog (linetypes /
                                            // layers / blocks / patterns —
                                            // filled when the dialog opens).
                                            let current = dlg.values[i].clone();
                                            let choices = p.choices.clone();
                                            let selected = if choices.contains(&current) {
                                                current.clone()
                                            } else if let Some(first) = choices.first() {
                                                first.clone()
                                            } else {
                                                current.clone()
                                            };
                                            let mut label = selected.clone();
                                            egui::ComboBox::from_id_salt(
                                                egui::Id::new(("py_param_choice", i)),
                                            )
                                            .selected_text(&label)
                                            .width(150.0)
                                            .show_ui(ui, |ui| {
                                                for c in &choices {
                                                    if ui.selectable_label(&label == c, c).clicked() {
                                                        label = c.clone();
                                                    }
                                                }
                                            });
                                            if label != dlg.values[i] {
                                                dlg.values[i] = label;
                                                self.script_preview_dirty();
                                            }
                                        }
                                        cad_script::ParamType::FloatList
                                        | cad_script::ParamType::IntList
                                        | cad_script::ParamType::StrList
                                        | cad_script::ParamType::PointList => {
                                            let hint = match p.ptype {
                                                cad_script::ParamType::PointList => {
                                                    "x,y; x,y; …"
                                                }
                                                _ => "1, 2, 3, …",
                                            };
                                            if ui.add_sized(
                                                [220.0, 20.0],
                                                egui::TextEdit::singleline(&mut dlg.values[i])
                                                    .hint_text(hint),
                                            ).changed()
                                            {
                                                self.script_preview_dirty();
                                            }
                                        }
                                        cad_script::ParamType::Point => {
                                            let (mut x, mut y) =
                                                CadApp::parse_point_param_value(
                                                    &dlg.values[i],
                                                )
                                                .unwrap_or((0.0, 0.0));
                                            let rx = ui.add_sized(
                                                [76.0, 20.0],
                                                egui::DragValue::new(&mut x).update_while_editing(false).speed(1.0),
                                            );
                                            let ry = ui.add_sized(
                                                [76.0, 20.0],
                                                egui::DragValue::new(&mut y).update_while_editing(false).speed(1.0),
                                            );
                                            let picking = self
                                                .script_param_pick
                                                .as_ref()
                                                .map(|(n, _)| n == &p.name)
                                                .unwrap_or(false);
                                            if ui
                                                .add(egui::Button::new(if picking {
                                                    "click canvas…"
                                                } else {
                                                    "Pick"
                                                }))
                                                .on_hover_text(
                                                    "pick the position with a click on the canvas",
                                                )
                                                .clicked()
                                            {
                                                self.script_param_pick =
                                                    Some((p.name.clone(), ScriptPickKind::Point));
                                                self.set_prompt(format!(
                                                    "script: click the position for '{}' — Esc cancels",
                                                    p.name
                                                ));
                                            }
                                            if rx.changed() || ry.changed() {
                                                dlg.values[i] = format!("{},{}", x, y);
                                                // A manual edit supersedes an armed pick.
                                                self.script_param_pick = None;
                                                self.script_preview_dirty();
                                            }
                                        }
                                        cad_script::ParamType::Entity => {
                                            let idx: i64 =
                                                dlg.values[i].parse().unwrap_or(-1);
                                            let label = if idx >= 0 {
                                                format!("entity #{}", idx)
                                            } else {
                                                "unpicked".into()
                                            };
                                            ui.add_sized(
                                                [80.0, 20.0],
                                                egui::Label::new(&label),
                                            );
                                            let picking = self
                                                .script_param_pick
                                                .as_ref()
                                                .map(|(n, _)| n == &p.name)
                                                .unwrap_or(false);
                                            if ui
                                                .add(egui::Button::new(if picking {
                                                    "click entity…"
                                                } else {
                                                    "Pick"
                                                }))
                                                .on_hover_text(
                                                    "click the shape on the canvas to pick it",
                                                )
                                                .clicked()
                                            {
                                                self.script_param_pick = Some((
                                                    p.name.clone(),
                                                    ScriptPickKind::Entity,
                                                ));
                                                self.set_prompt(format!(
                                                    "script: click the shape for '{}' — Esc cancels",
                                                    p.name
                                                ));
                                            }
                                            if ui.button("clear").clicked() {
                                                dlg.values[i] = "-1".into();
                                                self.script_param_pick = None;
                                                self.script_preview_dirty();
                                            }
                                        }
                                        cad_script::ParamType::Color => {
                                            let aci: u8 =
                                                dlg.values[i].parse().unwrap_or(0);
                                            let (r, g, b) = cad_kernel::aci_palette(aci);
                                            let (sw, _) = ui.allocate_exact_size(
                                                egui::vec2(22.0, 20.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                sw,
                                                3.0,
                                                egui::Color32::from_rgb(r, g, b),
                                            );
                                            ui.painter().rect_stroke(
                                                sw,
                                                3.0,
                                                egui::Stroke::new(
                                                    1.0,
                                                    egui::Color32::from_gray(90),
                                                ),
                                            );
                                            ui.label(format!("ACI {}", aci));
                                            if ui
                                                .button("color…")
                                                .on_hover_text(
                                                    "open the ACI color wheel",
                                                )
                                                .clicked()
                                            {
                                                self.aci_pick_request =
                                                    Some(AciPickRequest::ScriptParam(i));
                                            }
                                        }
                                    }
                                });
                            }
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if ui.button("Run").clicked() {
                                    do_run = true;
                                }
                                if ui.button("Cancel").clicked() {
                                    do_cancel = true;
                                }
                                ui.weak("values reach the script as rasm.params");
                            });
                        }
                    });
            });
        // Resolve the outcome outside the window closure.
        dlg.pos += drag_delta;
        if !keep || do_cancel || dlg.waiting_meta {
            self.script_param_pick = None; // no orphaned armed pick
            self.clear_script_preview(); // ghosts vanish with the dialog
            if dlg.waiting_meta {
                self.script_param_dialog = Some(dlg); // still waiting for Meta
            }
            return;
        }
        if do_run {
            self.script_param_pick = None;
            self.clear_script_preview();
            let params = match self.dialog_params_scene(&dlg) {
                Ok(p) => p,
                Err(e) => {
                    self.fail_op(format!("run {}: {}", dlg.name, e));
                    return;
                }
            };
            let (name, path) = (dlg.name.clone(), dlg.path.clone());
            let shown: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            self.history
                .push(format!("  run> {} {}", name, shown.join(" ")));
            self.py_log(format!(">>> run {} {}", name, shown.join(" ")));
            self.script
                .get_or_insert_with(cad_script::ScriptEngine::new)
                .submit_script_with_params(path, name, Vec::new(), params);
        } else {
            self.script_param_dialog = Some(dlg); // still open
        }
    }

    pub(super) fn parse_point_param_value(s: &str) -> Option<(f64, f64)> {
        let s = s.trim().trim_start_matches('(').trim_end_matches(')');
        let (a, b) = s.split_once(',')?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
    }

    /// The dialog's initial value for a param: `Length` defaults are SCENE
    /// units (the script's own default) — the field edits DISPLAY units, so
    /// convert once when filling the dialog.
    pub(super) fn param_display_default(&self, p: &cad_script::ScriptParamMeta) -> String {
        if p.ptype == cad_script::ParamType::Length {
            if let Ok(s) = p.default.parse::<f64>() {
                return format!("{}", self.doc.units.to_display(s));
            }
        }
        p.default.clone()
    }

    /// Convert ONE raw value to the string the script should receive.
    /// `Length` values are display units (with optional suffixes — `25`,
    /// `25cm`, `6'`) → scene units via `Units::parse_distance`; everything
    /// else passes through unchanged. Fails loudly on a bad length
    /// (rule 10 — the run must not proceed with a wrong distance).
    pub(super) fn param_value_scene(
        &self,
        spec: &[cad_script::ScriptParamMeta],
        name: &str,
        raw: &str,
    ) -> Result<String, String> {
        let Some(p) = spec.iter().find(|p| p.name == name) else {
            return Ok(raw.to_string());
        };
        match p.ptype {
            cad_script::ParamType::Length => match self.doc.units.parse_distance(raw) {
                Some(s) if s.is_finite() => Ok(format!("{}", s)),
                _ => Err(format!("{}: '{}' is not a valid length", name, raw)),
            },
            // Catalog-backed dropdowns validate against the LIVE catalogs —
            // a value outside the list fails loudly instead of running.
            // Stale dialog values may carry Python repr quotes
            // ("'ANSI31'") — strip them before comparing.
            cad_script::ParamType::Linetype
            | cad_script::ParamType::Layer
            | cad_script::ParamType::Block
            | cad_script::ParamType::HatchPattern
            | cad_script::ParamType::Choice => {
                let cmp = raw.trim().trim_matches(|c| c == '\'' || c == '"');
                let valid =
                    !p.choices.is_empty() && p.choices.iter().any(|c| c.eq_ignore_ascii_case(cmp));
                if valid {
                    // Canonicalize to the catalog/declared spelling.
                    let canonical = p
                        .choices
                        .iter()
                        .find(|c| c.eq_ignore_ascii_case(cmp))
                        .cloned()
                        .unwrap_or_else(|| cmp.to_string());
                    Ok(canonical)
                } else {
                    Err(format!(
                        "{}: '{}' is not one of [{}]",
                        name,
                        raw,
                        p.choices.join(", ")
                    ))
                }
            }
            _ => Ok(raw.to_string()),
        }
    }

    /// The whole dialog's values → (name, scene-value) pairs, converting
    /// `Length` fields through the document's display unit.
    fn dialog_params_scene(
        &self,
        dlg: &ScriptParamDialog,
    ) -> Result<Vec<(String, String)>, String> {
        dlg.params
            .iter()
            .zip(dlg.values.iter())
            .map(|(p, v)| {
                self.param_value_scene(&dlg.params, &p.name, v)
                    .map(|s| (p.name.clone(), s))
            })
            .collect()
    }

    /// Open the scripting API reference window (`pyhelp`), loading the
    /// document once from `docs/scripting_api.md` (cwd, then exe dir).
    pub(super) fn open_scripting_doc(&mut self) {
        if self.scripting_doc_text.is_none() {
            let mut dirs = vec![std::path::PathBuf::from("docs")];
            if let Ok(exe) = std::env::current_exe() {
                if let Some(d) = exe.parent() {
                    dirs.push(d.join("docs"));
                }
            }
            for dir in dirs {
                let p = dir.join("scripting_api.md");
                if let Ok(t) = std::fs::read_to_string(&p) {
                    self.scripting_doc_text = Some(t);
                    break;
                }
            }
            if self.scripting_doc_text.is_none() {
                self.scripting_doc_text = Some(
                    "! cannot find docs/scripting_api.md (looked in ./docs and <exe dir>/docs)"
                        .into(),
                );
            }
        }
        self.scripting_doc_open = true;
    }

    /// The `pyhelp` reference window — the full scripting API document in a
    /// resizable, scrollable floating window.
    pub(super) fn render_scripting_doc(&mut self, ctx: &egui::Context) {
        if !self.scripting_doc_open {
            return;
        }
        let mut keep = true;
        egui::Window::new("Python scripting — API reference")
            .order(egui::Order::Foreground)
            .id(egui::Id::new("scripting_doc_dialog"))
            .title_bar(false)
            .resizable(true)
            .collapsible(false)
            .default_size(egui::vec2(920.0, 680.0))
            .min_size(egui::vec2(480.0, 300.0))
            .frame(
                egui::Frame::none()
                    .fill(crate::theme::color::SURFACE_1)
                    .stroke(egui::Stroke::new(1.0, crate::theme::color::BORDER)),
            )
            .show(ctx, |ui| {
                let hb = crate::dock::header_band(
                    ui,
                    "Python scripting — API reference",
                    Some("AI-agent reference"),
                    true,
                );
                if hb.close_clicked {
                    keep = false;
                }
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for line in self
                                    .scripting_doc_text
                                    .as_deref()
                                    .unwrap_or("(loading…)")
                                    .split('\n')
                                {
                                    if line.starts_with("## ") {
                                        ui.add_space(10.0);
                                        ui.label(
                                            egui::RichText::new(line)
                                                .size(16.0)
                                                .strong()
                                                .color(PP_ACCENT),
                                        );
                                        ui.add_space(2.0);
                                    } else if line.starts_with("# ") {
                                        ui.add_space(8.0);
                                        ui.label(egui::RichText::new(line).size(19.0).strong());
                                        ui.add_space(4.0);
                                    } else if line.trim_start().starts_with("```")
                                        || line.trim_start().starts_with('|')
                                    {
                                        ui.monospace(line);
                                    } else if let Some(rest) = line.strip_prefix("### ") {
                                        ui.add_space(6.0);
                                        ui.label(egui::RichText::new(rest).strong());
                                    } else if let Some(rest) = line.strip_prefix("- ") {
                                        ui.monospace(rest);
                                    } else if line.is_empty() {
                                        ui.add_space(2.0);
                                    } else {
                                        ui.monospace(line);
                                    }
                                }
                            });
                    });
            });
        if !keep {
            self.scripting_doc_open = false;
        }
    }

    /// Save the console's input buffer as scripts/<name>.py (slice 5 — the
    /// scripts folder is the home of named scripts). Rescans so the menu and
    /// the example list pick the new file up immediately.
    fn save_py_script(&mut self) {
        let name = self.py_console_name.trim().to_string();
        let code = self.py_console_input.trim().to_string();
        if name.is_empty() {
            self.py_log("! script name cannot be empty".into());
            return;
        }
        if code.is_empty() {
            self.py_log("! input is empty — type the script body first".into());
            return;
        }
        if let Err(e) = std::fs::create_dir_all(Self::script_dir()) {
            self.py_log(format!("! cannot create the scripts folder: {}", e));
            return;
        }
        let mut p = Self::script_dir().join(&name);
        if p.extension().is_none() {
            p.set_extension("py");
        }
        match std::fs::write(&p, format!("{}\n", code)) {
            Ok(()) => {
                self.scan_py_examples();
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.py_log(format!(
                    "saved script → {}  (run with: run {})",
                    p.display(),
                    stem
                ));
                self.history
                    .push(format!("  python: script saved → {}", p.display()));
            }
            Err(e) => {
                self.py_log(format!("! save failed: {}", e));
            }
        }
    }

    // ---- in-app script editor (slice 5) -----------------------------------

    /// Open the script editor panel (with a fresh script list).
    pub(super) fn py_editor_open_panel(&mut self) {
        self.scan_py_examples();
        self.py_editor_open = true;
    }

    /// Save the editor buffer to scripts/<name>.py. Returns true on success.
    fn py_editor_save(&mut self) -> bool {
        let name = self.py_editor_name.trim().to_string();
        if name.is_empty() {
            self.py_log("! editor: give the script a name first (scripts/<name>.py)".into());
            return false;
        }
        if let Err(e) = std::fs::create_dir_all(Self::script_dir()) {
            self.py_log(format!("! editor: cannot create the scripts folder: {}", e));
            return false;
        }
        let mut p = Self::script_dir().join(&name);
        if p.extension().is_none() {
            p.set_extension("py");
        }
        match std::fs::write(&p, format!("{}\n", self.py_editor_text)) {
            Ok(()) => {
                self.py_editor_path = Some(p.clone());
                self.py_editor_name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or(name);
                self.py_editor_dirty = false;
                self.scan_py_examples();
                self.py_log(format!("saved script → {}", p.display()));
                true
            }
            Err(e) => {
                self.py_log(format!("! editor: save failed: {}", e));
                false
            }
        }
    }

    /// Load a script file into the editor buffer.
    fn py_editor_load(&mut self, path: std::path::PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(src) => {
                self.py_editor_text = src;
                self.py_editor_path = Some(path.clone());
                self.py_editor_name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.py_editor_dirty = false;
                self.py_editor_confirm = None;
            }
            Err(e) => self.py_log(format!("! editor: cannot read {}: {}", path.display(), e)),
        }
    }

    /// Ask to load `target` — immediately when the buffer is clean, else
    /// arm the discard-confirm row.
    fn py_editor_request_open(&mut self, target: Option<std::path::PathBuf>) {
        if self.py_editor_dirty {
            self.py_editor_confirm = Some(match target {
                Some(p) => PyEditorConfirm::File(p),
                None => PyEditorConfirm::New,
            });
        } else {
            match target {
                Some(p) => self.py_editor_load(p),
                None => self.py_editor_new(),
            }
        }
    }

    /// Start a fresh, untitled script.
    fn py_editor_new(&mut self) {
        self.py_editor_text.clear();
        self.py_editor_path = None;
        self.py_editor_name.clear();
        self.py_editor_dirty = false;
        self.py_editor_confirm = None;
    }

    /// Run the editor's script: save (when dirty/untitled) then run it as a
    /// named script — same semantics as `run <name>`.
    pub(super) fn py_editor_run(&mut self) {
        if self.py_editor_name.trim().is_empty() {
            self.py_log("! editor: give the script a name first, then Run".into());
            return;
        }
        if self.py_editor_dirty || self.py_editor_path.is_none() {
            if !self.py_editor_save() {
                return;
            }
        }
        let Some(path) = self.py_editor_path.clone() else {
            return;
        };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.py_log(format!(">>> run {}", stem));
        self.clear_script_preview();
        self.script
            .get_or_insert_with(cad_script::ScriptEngine::new)
            .submit_script(path, stem.clone(), Vec::new());
    }

    /// The script editor panel: open/save/run toolbar over a monospace
    /// multi-line buffer. Unsaved changes persist in the app even when the
    /// panel is closed; only Save writes to scripts/.
    pub(super) fn render_py_editor(&mut self, ctx: &egui::Context) {
        if !self.py_editor_open {
            return;
        }
        let badge = match (&self.py_editor_path, self.py_editor_dirty) {
            (Some(p), true) => Some(format!(
                "{} *",
                p.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )),
            (Some(p), false) => Some(
                p.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            (None, true) => Some("untitled *".into()),
            (None, false) => None,
        };
        let mut keep = true;
        egui::Window::new("Script editor")
            .order(egui::Order::Foreground)
            .id(egui::Id::new("py_editor_dialog"))
            .title_bar(false)
            .resizable(true)
            .collapsible(false)
            .default_size(egui::vec2(760.0, 540.0))
            .min_size(egui::vec2(420.0, 260.0))
            .frame(egui::Frame::none()
                .fill(crate::theme::color::SURFACE_1)
                .stroke(egui::Stroke::new(1.0, crate::theme::color::BORDER)))
            .show(ctx, |ui| {
                let hb = crate::dock::header_band(
                    ui, "Script editor", badge.as_deref(), true);
                if hb.close_clicked { keep = false; }
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                    .show(ui, |ui| {
                        self.py_editor_toolbar(ui);
                        if let Some(target) = self.py_editor_confirm.clone() {
                            let what = match &target {
                                PyEditorConfirm::File(p) => p.display().to_string(),
                                PyEditorConfirm::New => "a new script".into(),
                            };
                            ui.horizontal(|ui| {
                                ui.label(format!("Discard unsaved changes and open {}?", what));
                                if ui.button("Discard & open").clicked() {
                                    match target {
                                        PyEditorConfirm::File(p) => self.py_editor_load(p),
                                        PyEditorConfirm::New => self.py_editor_new(),
                                    }
                                }
                                if ui.button("Cancel").clicked() {
                                    self.py_editor_confirm = None;
                                }
                            });
                        }
                        ui.add_space(4.0);
                        let resp = ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::multiline(&mut self.py_editor_text)
                                .id(egui::Id::new("py_editor_code"))
                                .code_editor()
                                .desired_rows(24)
                                .hint_text("python — uses the rasm surface (help(rasm)); Ctrl+S saves, Esc stops a run"),
                        );
                        if resp.changed() {
                            self.py_editor_dirty = true;
                        }
                        // Ctrl+S — save the buffer while the editor has focus.
                        if resp.has_focus()
                            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S))
                        {
                            self.py_editor_save();
                        }
                    });
            });
        if !keep {
            self.py_editor_open = false;
        }
    }

    /// Editor toolbar: open/New on the left, name + Save + Run on the right.
    fn py_editor_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut open_target: Option<std::path::PathBuf> = None;
        let mut do_new = false;
        let mut do_save = false;
        let mut do_run = false;
        ui.horizontal(|ui| {
            let current_label = match &self.py_editor_path {
                Some(p) => p
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                None => {
                    if self.py_editor_dirty {
                        "untitled *".into()
                    } else {
                        "open…".into()
                    }
                }
            };
            egui::ComboBox::from_id_salt("py_editor_open")
                .selected_text(current_label)
                .width(170.0)
                .show_ui(ui, |ui| {
                    if self.py_examples.is_empty() {
                        ui.label("(no scripts yet)");
                    }
                    for (name, path) in &self.py_examples {
                        if ui.selectable_label(false, name).clicked() {
                            open_target = Some(path.clone());
                        }
                    }
                });
            if ui.button("New").clicked() {
                do_new = true;
            }
            if ui
                .button("Refresh")
                .on_hover_text("rescan scripts/")
                .clicked()
            {
                self.scan_py_examples();
            }
            ui.separator();
            ui.label("name:");
            ui.add_sized(
                [120.0, 20.0],
                egui::TextEdit::singleline(&mut self.py_editor_name)
                    .id(egui::Id::new("py_editor_name"))
                    .hint_text("script_name"),
            );
            if ui
                .add_enabled(
                    !self.py_editor_name.trim().is_empty(),
                    egui::Button::new("Save"),
                )
                .on_hover_text("saves to scripts/<name>.py  (Ctrl+S)")
                .clicked()
            {
                do_save = true;
            }
            let busy = self.script.as_ref().is_some_and(|s| s.is_busy());
            if ui
                .add_enabled(!busy, egui::Button::new(if busy { "busy" } else { "Run" }))
                .on_hover_text("saves (if changed) then runs as `run <name>`")
                .clicked()
            {
                do_run = true;
            }
        });
        // Commit after the borrow-heavy row loop.
        if let Some(target) = open_target {
            let same = self.py_editor_path.as_ref() == Some(&target);
            if !same {
                self.py_editor_request_open(Some(target));
            }
        }
        if do_new {
            self.py_editor_request_open(None);
        }
        if do_save {
            self.py_editor_save();
        }
        if do_run {
            self.py_editor_run();
        }
    }

    pub(super) fn render_layout_tabs(&mut self, ctx: &egui::Context) {
        use crate::theme::color as tc;
        use cad_kernel::plotstyle::PaperSize;
        let active = self.doc.active_layout;
        let n_layouts = self.doc.layouts.len();
        let nt: Vec<String> = self.doc.layouts.iter().map(|l| l.name.clone()).collect();
        let pt: Vec<(PaperSize, cad_kernel::plotstyle::Orientation)> = self
            .doc
            .layouts
            .iter()
            .map(|l| (l.paper, l.orientation))
            .collect();
        let sk = egui::Id::new("lsi");
        let ck = egui::Id::new("lci");
        let nk = egui::Id::new("lnd");
        egui::TopBottomPanel::top("layout_tabs").min_height(26.0).show(ctx, |ui| { ui.horizontal(|ui| {
            ui.add_space(11.0); let im = active.is_none();
            if ui.add(egui::Button::new(egui::RichText::new("Model").color(if im{tc::ACCENT}else{tc::TEXT_MUTED}).size(12.0)).fill(if im{tc::SURFACE_2}else{egui::Color32::TRANSPARENT}).min_size(egui::vec2(54.0,22.0)).rounding(egui::Rounding::same(4.0))).clicked() {
                ui.ctx().data_mut(|d| d.insert_temp::<Option<usize>>(sk, None)); }
            ui.add_space(2.0);
            for (i, name) in nt.iter().enumerate() { let is = active == Some(i); let r = ui.add(egui::Button::new(egui::RichText::new(name).color(if is{tc::ACCENT}else{tc::TEXT_MUTED}).size(12.0)).fill(if is{tc::SURFACE_2}else{egui::Color32::TRANSPARENT}).min_size(egui::vec2(80.0,22.0)).rounding(egui::Rounding::same(4.0)));
                if r.clicked() { ui.ctx().data_mut(|d| d.insert_temp::<Option<usize>>(sk, Some(i))); }
                r.context_menu(|ui| { if ui.button("Close Layout").clicked() { ui.ctx().data_mut(|d| d.insert_temp::<usize>(ck, i)); ui.close_menu(); } }); }
            ui.add_space(2.0);
            if ui.add(egui::Button::new(egui::RichText::new("+").color(tc::TEXT_MUTED).size(14.0)).fill(egui::Color32::TRANSPARENT).min_size(egui::vec2(22.0,22.0)).rounding(egui::Rounding::same(4.0))).clicked() {
                ui.ctx().data_mut(|d| d.insert_temp::<bool>(nk, true)); }
            ui.add_space(6.0);
            if let Some(i) = active { if let Some((p, o)) = pt.get(i) { ui.label(egui::RichText::new(format!("{} {}", match p{PaperSize::A4=>"A4",PaperSize::A3=>"A3",PaperSize::A2=>"A2",PaperSize::A1=>"A1",PaperSize::A0=>"A0",PaperSize::Letter=>"Let",_=>"?"}, if matches!(o,cad_kernel::plotstyle::Orientation::Landscape){"L"}else{"P"})).color(tc::TEXT_MUTED).size(10.0)); } }
            // Print-preview toggle (only in a layout): model true colours vs plot pen/CTB.
            if active.is_some() {
                ui.add_space(12.0);
                let on = self.layout_print_preview;
                let txt = if on { "◉ Print colors" } else { "○ Model colors" };
                if ui.add(egui::Button::new(egui::RichText::new(txt).size(11.0)
                        .color(if on { tc::ACCENT } else { tc::TEXT_MUTED }))
                        .fill(if on { tc::SURFACE_2 } else { egui::Color32::TRANSPARENT })
                        .rounding(egui::Rounding::same(4.0)))
                    .on_hover_text("Viewport render: OFF = model's true colours; ON = plot pen / CTB colours")
                    .clicked()
                {
                    self.layout_print_preview = !self.layout_print_preview;
                }
            }
        }); });
        if let Some(t) = ctx.data_mut(|d| d.remove_temp::<Option<usize>>(sk)) {
            self.switch_to_tab(t);
        }
        if let Some(i) = ctx.data_mut(|d| d.remove_temp::<usize>(ck)) {
            if i < self.doc.layouts.len() {
                // Closing a layout is an undoable doc edit — mark the drawing
                // dirty too (autosave / unsaved-changes prompt), and snapshot
                // BEFORE the swap so an undo restores this exact arrangement.
                self.snapshot_doc();
                // Any layout-bound dialog/edit state pointing at or past the
                // removed slot must be cleared or shifted, or an open
                // Viewport Properties dialog would silently edit a DIFFERENT
                // layout after the removal.
                let shift = |b: Option<(usize, usize)>| match b {
                    Some((li, vi)) if li == i => None,
                    Some((li, vi)) if li > i => Some((li - 1, vi)),
                    b => b,
                };
                self.vp_edit_dialog = shift(self.vp_edit_dialog);
                self.vp_edit_init_for = shift(self.vp_edit_init_for);
                if self.viewport_draw_li == Some(i) {
                    self.viewport_draw_state = 0;
                    self.viewport_draw_li = None;
                } else if let Some(li) = self.viewport_draw_li {
                    if li > i {
                        self.viewport_draw_li = Some(li - 1);
                    }
                }
                if self.doc.active_layout == Some(i) {
                    self.switch_to_tab(None);
                }
                self.doc.layouts.remove(i);
                self.doc.active_layout = match self.doc.active_layout {
                    Some(a) if a > i => Some(a - 1),
                    o => o,
                };
            }
        }
        if ctx.data_mut(|d| d.remove_temp::<bool>(nk)).unwrap_or(false) {
            self.show_new_layout_dialog = true;
            self.new_layout_name = format!("Layout {}", n_layouts + 1);
            self.new_layout_paper = PaperSize::A4;
            self.new_layout_landscape = true;
        }
    }

    /// LAYOUT TAB paper render: the white page (outline + name), every paper
    /// entity (print-preview applies the layout CTB), then each viewport's
    /// window into the model (true colours, or per-viewport CTB under the
    /// print-preview toggle), selection grips and viewport lock badges.
    pub(super) fn paper_space_render(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        painter: &egui::Painter,
        resp: &egui::Response,
    ) {
        let _ = ui;
        // ---- paper-space layout rendering ----------------------------
        if let Some(li) = self.doc.active_layout {
            if let Some(l) = self.doc.layouts.get(li) {
                let p0 = self.w2s(Vec2::new(0.0, 0.0), rect);
                let p1 = self.w2s(Vec2::new(l.page_w_mm, l.page_h_mm), rect);
                let pmn = egui::pos2(p0.x.min(p1.x), p0.y.min(p1.y));
                let pmx = egui::pos2(p0.x.max(p1.x), p0.y.max(p1.y));
                let pr = egui::Rect::from_min_max(pmn, pmx);
                painter.rect_filled(
                    pr,
                    egui::Rounding::ZERO,
                    egui::Color32::from_rgb(252, 252, 252),
                );
                painter.rect_stroke(
                    pr,
                    egui::Rounding::ZERO,
                    egui::Stroke::new(1.5, egui::Color32::from_rgb(80, 80, 80)),
                );
                painter.text(
                    egui::pos2(pmn.x + 4., pmx.y - 14.),
                    egui::Align2::LEFT_BOTTOM,
                    &l.name,
                    egui::FontId::proportional(11.),
                    egui::Color32::from_rgb(120, 120, 120),
                );
                let ents = l.entities.clone();
                // The layout's CTB table (saved CTBs only — built-ins resolve by
                // name in apply_vp_ctb). The editor's live table wins while open.
                let layout_ctb_table = if self.layout_print_preview {
                    let name = l.ctb_name.as_deref().unwrap_or("");
                    if self.plotstyle_open && self.doc.plot_styles.name.eq_ignore_ascii_case(name) {
                        Some(&self.doc.plot_styles)
                    } else {
                        self.ctb_table_cache.get(name)
                    }
                } else {
                    None
                };
                for d in &ents {
                    if !d.style.visible {
                        continue;
                    }
                    let ego = if matches!(&d.geom, cad_kernel::Geom::Viewport(_)) {
                        egui::Color32::from_rgb(60, 60, 80)
                    } else {
                        let (r, g, b) = cad_kernel::resolve_color(
                            d.style.color,
                            d.style.layer,
                            &self.doc.layers,
                            &self.doc.truecolors,
                        );
                        egui::Color32::from_rgb(r, g, b)
                    };
                    let ego = if self.layout_print_preview {
                        Self::apply_vp_ctb(
                            (ego.r(), ego.g(), ego.b()),
                            style_effective_aci(d.style.color, d.style.layer, &self.doc.layers),
                            l.ctb_name.as_deref().unwrap_or(""),
                            layout_ctb_table,
                        )
                    } else {
                        ego
                    };
                    // Hatches: `draw_dobject` / `paint_dobject_ctb` stub Hatch as
                    // a no-op — short-circuit to the boundary-resolving fill
                    // renderer with the CTB-applied colour.
                    if let cad_kernel::Geom::Hatch(h) = &d.geom {
                        self.render_hatch_fill(&painter, rect, h, ego);
                        continue;
                    }
                    if self.layout_print_preview {
                        // Print-preview: the effective CTB's pen width + linetype.
                        paint_dobject_ctb(
                            &painter,
                            rect,
                            self,
                            d,
                            ego,
                            self.scale,
                            layout_ctb_table,
                            &self.doc.layers,
                        );
                    } else {
                        draw_dobject(&painter, rect, self, &d.geom, ego);
                    }
                }
                let vps = &l.viewports;
                let ssc = self.scale;
                let sof = self.world_offset;
                let page_white = egui::Color32::from_rgb(252, 252, 252);
                let mut hatch_pending: Vec<(u64, HatchCacheEntry)> = Vec::new();
                let mut badges: Vec<(Vec2, bool)> = Vec::new();
                for vp in vps {
                    // Restore paper camera for screen-coordinate math
                    self.scale = ssc;
                    self.world_offset = sof;
                    let Some(h) = vp.shape_handle else {
                        continue;
                    };
                    let Some(e) = ents.iter().find(|e| e.handle == h) else {
                        continue;
                    };
                    let cad_kernel::Geom::Viewport(vg) = &e.geom else {
                        continue;
                    };
                    let a = self.w2s(
                        Vec2::new(vg.center.x - vg.width * 0.5, vg.center.y - vg.height * 0.5),
                        rect,
                    );
                    let b = self.w2s(
                        Vec2::new(vg.center.x + vg.width * 0.5, vg.center.y + vg.height * 0.5),
                        rect,
                    );
                    let vp_sr = egui::Rect::from_min_max(
                        egui::pos2(a.x.min(b.x), a.y.min(b.y)),
                        egui::pos2(a.x.max(b.x), a.y.max(b.y)),
                    );
                    let clip_painter = painter.with_clip_rect(vp_sr);
                    let vc = egui::pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
                    let ms = (vp.model_zoom * vp.model_scale * ssc as f64) as f32;
                    let cx = rect.center().x;
                    let cy = rect.center().y;
                    self.scale = ms;
                    self.world_offset = egui::vec2(
                        (vc.x - cx) / ms - vp.model_center.0 as f32,
                        (cy - vc.y) / ms - vp.model_center.1 as f32,
                    );
                    // Per-viewport CTB, inheriting the LAYOUT's CTB when the
                    // viewport has none (matches the plot scene); empty = full
                    // colour.
                    let vp_ctb: &str = vp
                        .ctb_name
                        .as_deref()
                        .or(l.ctb_name.as_deref())
                        .unwrap_or("");
                    // Its saved table (built-ins resolve by name in apply_vp_ctb).
                    // The editor's live table wins while open.
                    let vp_ctb_table = if self.layout_print_preview {
                        if self.plotstyle_open
                            && self.doc.plot_styles.name.eq_ignore_ascii_case(vp_ctb)
                        {
                            Some(&self.doc.plot_styles)
                        } else {
                            self.ctb_table_cache.get(vp_ctb)
                        }
                    } else {
                        None
                    };
                    // Model-space window covered by this viewport — culls the
                    // per-frame model walk (hatches always draw: their bbox is
                    // a placeholder).
                    let k = (vp.model_zoom * vp.model_scale).max(1e-9);
                    let wmn = Vec2::new(
                        vp.model_center.0 - vg.width * 0.5 / k,
                        vp.model_center.1 - vg.height * 0.5 / k,
                    );
                    let wmx = Vec2::new(
                        vp.model_center.0 + vg.width * 0.5 / k,
                        vp.model_center.1 + vg.height * 0.5 / k,
                    );
                    // Hatch cache warm-up budget for the viewport loops (the
                    // model-space builder is skipped while a layout is active —
                    // its candidates are empty). Bound per frame like the model
                    // path so a dense batch of hatches can't spike one frame.
                    let mut hatch_work = 0usize;
                    // Candidates = the spatial index's window cells PLUS every
                    // hatch (their bbox is a (0,0) placeholder, so they never
                    // land in the window query — they warm independently).
                    // Falls back to the full walk while the index is stale or
                    // missing, exactly like the model path's viewport_scope().
                    let mut cands: Vec<usize> =
                        if let (Some(g), false) = (self.index.as_ref(), self.index_dirty) {
                            g.query_bbox(wmn, wmx)
                                .into_iter()
                                .map(|u| u as usize)
                                .collect()
                        } else {
                            (0..self.doc.dobjects.len()).collect()
                        };
                    cands.extend(
                        self.doc
                            .dobjects
                            .iter()
                            .enumerate()
                            .filter(|(_, d)| matches!(&d.geom, cad_kernel::Geom::Hatch(_)))
                            .map(|(i, _)| i),
                    );
                    cands.sort_unstable();
                    cands.dedup();
                    for &d_idx in &cands {
                        let Some(d) = self.doc.dobjects.get(d_idx) else {
                            continue;
                        };
                        if !d.style.visible {
                            continue;
                        }
                        if !l.layers.renders(d.style.layer) {
                            continue;
                        }
                        let is_hatch = matches!(&d.geom, cad_kernel::Geom::Hatch(_));
                        if !is_hatch {
                            let (bmin, bmax) = d.bbox();
                            if bmax.x < wmn.x || bmin.x > wmx.x || bmax.y < wmn.y || bmin.y > wmx.y
                            {
                                continue;
                            }
                        }
                        // Model content resolves against the MODEL layer table
                        // (held in `l.layers` while this layout is active — layers
                        // are swapped on tab change), NOT the paper layers, so
                        // ByLayer colours match the model exactly.
                        let (r, g, b) = cad_kernel::resolve_color(
                            d.style.color,
                            d.style.layer,
                            &l.layers,
                            &self.doc.truecolors,
                        );
                        // Default = true model colours (identical render). Only the
                        // print-preview toggle applies the plot pen / CTB.
                        let ego = if self.layout_print_preview {
                            Self::apply_vp_ctb(
                                (r, g, b),
                                style_effective_aci(d.style.color, d.style.layer, &l.layers),
                                vp_ctb,
                                vp_ctb_table,
                            )
                        } else {
                            egui::Color32::from_rgb(r, g, b)
                        };
                        if let cad_kernel::Geom::Hatch(_) = &d.geom {
                            // Render the cached geometry (shared painter — the
                            // page's white is the hole over-draw colour), or
                            // warm the cache (budgeted + once per frame).
                            let mut painted = false;
                            if let Some(entry) = self.hatch_cache.get(&d.handle) {
                                paint_cached_hatch(
                                    &clip_painter,
                                    rect,
                                    self,
                                    entry,
                                    ego,
                                    page_white,
                                );
                                painted = true;
                            }
                            if !painted {
                                if hatch_work < HATCH_GEN_WORK_BUDGET
                                    && !hatch_pending.iter().any(|(hh, _)| *hh == d.handle)
                                {
                                    let entry = self.build_hatch_cache_entry(d_idx);
                                    hatch_work += entry.segs.len()
                                        + entry.circs.len()
                                        + entry.solid.iter().map(|(_, t)| t.len()).sum::<usize>();
                                    paint_cached_hatch(
                                        &clip_painter,
                                        rect,
                                        self,
                                        &entry,
                                        ego,
                                        page_white,
                                    );
                                    hatch_pending.push((d.handle, entry));
                                }
                                // Beyond this frame's budget: skip — the entry
                                // warms on a later frame (the budget is consumed
                                // in list order), so an uncached hatch is simply
                                // not painted yet. Rendering it here unbudgeted
                                // regenerated dense fills EVERY frame while a
                                // layout tab was visible.
                            }
                            continue;
                        }
                        if self.layout_print_preview {
                            // Print-preview: the effective CTB's pen width +
                            // linetype (paper scale = the layout tab's camera).
                            paint_dobject_ctb(
                                &clip_painter,
                                rect,
                                self,
                                d,
                                ego,
                                ssc,
                                vp_ctb_table,
                                &l.layers,
                            );
                        } else {
                            draw_dobject(&clip_painter, rect, self, &d.geom, ego);
                        }
                    }
                    // Cache warm-ups land after the borrow of `self.doc` ends.
                    for (hh, entry) in hatch_pending.drain(..) {
                        self.hatch_cache.insert(hh, entry);
                    }
                    if !self.layout_print_preview {
                        // The viewport's top-right corner (paper coords) — the
                        // lock badge paints there under the PAPER camera after
                        // the loop restores it below.
                        let (_mn, mx) = vg.bbox_world();
                        badges.push((Vec2::new(mx.x, mx.y), vp.locked));
                    }
                }
                self.scale = ssc;
                self.world_offset = sof;
                // Lock badges — screen math under the paper camera, matching
                // the click hit-test in `paper_space_input` exactly.
                if !self.layout_print_preview {
                    for (corner, locked) in &badges {
                        let tr = self.w2s(*corner, rect);
                        Self::paint_lock_badge(
                            painter,
                            egui::pos2(tr.x - 12.0, tr.y + 12.0),
                            *locked,
                        );
                    }
                }
                // Rubber-band for viewport creation
                if self.viewport_draw_state == 2 {
                    if let (Some(p1), Some(cs)) = (self.viewport_draw_p1, resp.hover_pos()) {
                        let a = self.w2s(p1, rect);
                        let rb = egui::Rect::from_two_pos(a, cs);
                        painter.rect_stroke(
                            rb,
                            0.0,
                            egui::Stroke::new(1.2, egui::Color32::from_rgb(0, 200, 255)),
                        );
                    }
                }
            }
        }
    }

    /// LAYOUT TAB paper input lane: viewport-frame selection / MOVE / grip
    /// RESIZE (overlap-guarded), viewport-draw corner picking, right-click
    /// context menu (Scale / Lock / Copy / Set CTB / Delete / Create
    /// viewport), and Delete-key removal of paper entities.
    pub(super) fn paper_space_input(
        &mut self,
        ctx: &egui::Context,
        ui: &egui::Ui,
        rect: egui::Rect,
        painter: &egui::Painter,
        resp: &egui::Response,
    ) {
        let _ = ctx;
        let _ = painter;
        let canvas_locked = self.block_editor.is_some() || self.plot_win_pick != 0;
        // ---- Layout entity selection + grip resize + MOVE (paper space) ----
        // Viewport frames behave like 2D dobjects: click INSIDE (or on the
        // frame) to select, drag the body to MOVE, drag a grip to RESIZE.
        // Overlap between viewports is prevented. (Paper is a sheet; the
        // model inside is a view, handled separately.)
        if self.doc.active_layout.is_some() && !canvas_locked && self.viewport_draw_state == 0 {
            let Some(li) = self.doc.active_layout else {
                return;
            };
            let Some(l) = self.doc.layouts.get(li) else {
                return;
            };
            let ents = l.entities.clone();
            let tol = 10.0 / self.scale.max(0.01) as f64;
            // Entity index under a world point: INSIDE a viewport rect, or
            // within `tol` of any entity's frame.
            let hit_at = |w: Vec2| -> Option<usize> {
                let mut best: Option<(usize, f64)> = None;
                for (i, d) in ents.iter().enumerate() {
                    let (inside, dist) = if let cad_kernel::Geom::Viewport(vg) = &d.geom {
                        let (mn, mx) = vg.bbox_world();
                        let ins = w.x >= mn.x && w.x <= mx.x && w.y >= mn.y && w.y <= mx.y;
                        (
                            ins,
                            if ins {
                                0.0
                            } else {
                                d.geom.distance_to_point(w)
                            },
                        )
                    } else {
                        (false, d.geom.distance_to_point(w))
                    };
                    if inside || dist < tol {
                        if best.map_or(true, |(_, bd)| dist < bd) {
                            best = Some((i, dist));
                        }
                    }
                }
                best.map(|(i, _)| i)
            };
            // Clear a stale move-anchor on any non-dragging frame (keeps it
            // set through the stop frame so the window-select handler skips).
            if !resp.dragged_by(egui::PointerButton::Primary) && !resp.drag_stopped() {
                self.layout_move_last = None;
            }
            // Grip hover (selected entities only).
            let mut near_grip: Option<(usize, GripRole)> = None;
            if let Some(pos) = resp.interact_pointer_pos() {
                for &si in &self.layout_selection {
                    if let Some(e) = ents.get(si) {
                        for (gp, role) in e.geom.grip_points() {
                            let gs = self.w2s(gp, rect);
                            if pos.distance(gs) < 12.0 {
                                near_grip = Some((si, role));
                                break;
                            }
                        }
                        if near_grip.is_some() {
                            break;
                        }
                    }
                }
            }
            // Start grip drag (resize).
            if let Some((si, role)) = near_grip {
                if resp.drag_started() {
                    if let Some(e) = ents.get(si) {
                        if let Some(gp) = e
                            .geom
                            .grip_points()
                            .iter()
                            .find(|(_, r)| *r == role)
                            .map(|(p, _)| *p)
                        {
                            // One undo step per gesture — the drag writes the entity
                            // every frame below.
                            self.snapshot_doc();
                            self.layout_grip_drag = Some((si, role, gp));
                        }
                    }
                }
            }
            // Start MOVE drag: press on a viewport body (not a grip). Auto-select.
            if near_grip.is_none() && self.layout_grip_drag.is_none() && resp.drag_started() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let w = self.s2w(pos, rect);
                    if let Some(i) = hit_at(w) {
                        if !self.layout_selection.contains(&i) {
                            if !ui.input(|inp| inp.modifiers.shift) {
                                self.layout_selection.clear();
                            }
                            self.layout_selection.push(i);
                        }
                        // One undo step per gesture (the MOVE below writes
                        // per drag frame).
                        self.snapshot_doc();
                        self.layout_move_last = Some(w);
                    }
                }
            }
            // Process grip RESIZE (with overlap guard).
            if let Some((si, role, origin)) = self.layout_grip_drag {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let w = self.s2w(pos, rect);
                    if resp.dragged_by(egui::PointerButton::Primary) {
                        // Bounds-guarded: the entity may have been deleted (or
                        // the tab switched) mid-drag; abort the resize instead
                        // of indexing out of range or mutating a stranger.
                        let Some(entity) = ents.get(si) else {
                            self.layout_grip_drag = None;
                            self.layout_move_last = None;
                            return;
                        };
                        let new_geom = entity.geom.with_grip_moved(role, origin + (w - origin));
                        let ok = if let cad_kernel::Geom::Viewport(nvg) = &new_geom {
                            let (nmn, nmx) = nvg.bbox_world();
                            !Self::layout_rect_overlaps(&ents, si, (nmn.x, nmn.y), (nmx.x, nmx.y))
                        } else {
                            true
                        };
                        if ok {
                            if let Some(layout) = self.doc.layouts.get_mut(li) {
                                if let Some(entity) = layout.entities.get_mut(si) {
                                    entity.geom = new_geom;
                                }
                                // Entity is authoritative — mirror rect + model
                                // camera into the sidecar via the kernel helper so
                                // the RSM writer / render loop can't drift.
                                layout.sync_all_viewports();
                            }
                        }
                    }
                }
            }
            // Process MOVE drag (selected viewports, overlap-guarded per entity).
            else if let Some(last) = self.layout_move_last {
                if resp.dragged_by(egui::PointerButton::Primary) {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let w = self.s2w(pos, rect);
                        let dv = w - last;
                        let sel = self.layout_selection.clone();
                        if let Some(layout) = self.doc.layouts.get_mut(li) {
                            for &si in &sel {
                                let (nc, nmn, nmx) = {
                                    let Some(entity) = layout.entities.get(si) else {
                                        continue;
                                    };
                                    let cad_kernel::Geom::Viewport(vg) = &entity.geom else {
                                        continue;
                                    };
                                    let nc = vg.center + dv;
                                    (
                                        (nc),
                                        (nc.x - vg.width * 0.5, nc.y - vg.height * 0.5),
                                        (nc.x + vg.width * 0.5, nc.y + vg.height * 0.5),
                                    )
                                };
                                if Self::layout_rect_overlaps(&ents, si, nmn, nmx) {
                                    continue;
                                }
                                if let Some(entity) = layout.entities.get_mut(si) {
                                    if let cad_kernel::Geom::Viewport(vg) = &mut entity.geom {
                                        vg.center = nc;
                                    }
                                }
                            }
                            // Entity is authoritative — mirror into the sidecar in
                            // one place (Layout::sync_all_viewports).
                            layout.sync_all_viewports();
                        }
                        self.layout_move_last = Some(w);
                    }
                }
            }
            // End drags on stop.
            if resp.drag_stopped() {
                self.layout_grip_drag = None;
            }
            // Right-click on a viewport → select it, so the context menu
            // always offers Scale / Lock / Copy for the one under the cursor.
            if resp.secondary_clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let w = self.s2w(pos, rect);
                    if let Some(i) = hit_at(w) {
                        if !self.layout_selection.contains(&i) {
                            self.layout_selection = vec![i];
                        }
                    }
                }
            }
            // Click for selection (inside rect or on frame). A click on a
            // corner LOCK BADGE toggles that viewport's scale lock instead.
            if resp.clicked()
                && self.layout_grip_drag.is_none()
                && self.layout_move_last.is_none()
                && near_grip.is_none()
            {
                if let Some(pos) = resp.interact_pointer_pos() {
                    // Lock badge hit?
                    let mut badge_handle: Option<u64> = None;
                    for e in &ents {
                        if let cad_kernel::Geom::Viewport(vg) = &e.geom {
                            let (_mn, mx) = vg.bbox_world();
                            let tr = self.w2s(Vec2::new(mx.x, mx.y), rect);
                            if pos.distance(egui::pos2(tr.x - 12.0, tr.y + 12.0)) < 11.0 {
                                badge_handle = Some(e.handle);
                                break;
                            }
                        }
                    }
                    if let Some(h) = badge_handle {
                        self.snapshot_doc();
                        if let Some(layout) = self.doc.layouts.get_mut(li) {
                            if let Some(v) = layout
                                .viewports
                                .iter_mut()
                                .find(|v| v.shape_handle == Some(h))
                            {
                                v.locked = !v.locked;
                            }
                        }
                    } else {
                        let w = self.s2w(pos, rect);
                        let best = hit_at(w);
                        if !ui.input(|inp| inp.modifiers.shift)
                            && !ui.input(|inp| inp.modifiers.ctrl)
                        {
                            self.layout_selection.clear();
                        }
                        if let Some(i) = best {
                            let was_selected = self.layout_selection.contains(&i);
                            if resp.double_clicked() {
                                // Double-click opens the viewport's Properties
                                // dialog — checked BEFORE the toggle so a
                                // double-click on an UNSELECTED viewport works
                                // (the second click must not deselect it first).
                                if !was_selected {
                                    self.layout_selection.push(i);
                                }
                                if let Some(layout) = self.doc.layouts.get(li) {
                                    if let Some(e) = ents.get(i) {
                                        if let Some(vi) = layout
                                            .viewports
                                            .iter()
                                            .position(|v| v.shape_handle == Some(e.handle))
                                        {
                                            self.vp_edit_dialog = Some((li, vi));
                                        }
                                    }
                                }
                            } else if was_selected {
                                self.layout_selection.retain(|&x| x != i);
                            } else {
                                self.layout_selection.push(i);
                            }
                        }
                    }
                }
            }
        }

        // ---- Viewport rectangle pick (paper space) -------------------
        if self.viewport_draw_state != 0 && !canvas_locked {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.viewport_draw_state = 0;
                self.viewport_draw_p1 = None;
                self.viewport_draw_li = None;
                self.clear_prompt();
            }
            if resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let w = self.s2w(pos, rect);
                    if self.viewport_draw_state == 1 {
                        // First corner — advance to the second, DO NOT reset here.
                        self.viewport_draw_p1 = Some(w);
                        self.viewport_draw_state = 2;
                        self.set_prompt("Click second corner of viewport (Esc to cancel)");
                    } else if self.viewport_draw_state == 2 {
                        if let Some(p1) = self.viewport_draw_p1 {
                            let mn = Vec2::new(p1.x.min(w.x), p1.y.min(w.y));
                            let mx = Vec2::new(p1.x.max(w.x), p1.y.max(w.y));
                            if (mx.x - mn.x) > 1.0 && (mx.y - mn.y) > 1.0 {
                                // Valid box — open the scale dialog and finish the tool.
                                self.viewport_draw_p1 = Some(mn);
                                self.viewport_draw_p2 = Some(mx);
                                self.viewport_scale_text = String::from("1:100");
                                self.viewport_ctb_name = String::new(); // default = full color (model render)
                                self.viewport_scale_dialog_open = true;
                                self.viewport_draw_state = 0;
                                self.clear_prompt();
                            } else {
                                // Degenerate (zero-size) box — restart from corner 1.
                                self.viewport_draw_p1 = None;
                                self.viewport_draw_state = 1;
                                self.set_prompt(
                                    "Viewport too small — click first corner again (Esc to cancel)",
                                );
                            }
                        } else {
                            // No stored first corner — bail cleanly.
                            self.viewport_draw_state = 0;
                            self.clear_prompt();
                        }
                    }
                }
            }
        }

        // ---- right-click shortcut (context) menu --------------------
        // Opens on a secondary CLICK (a right-DRAG still pans). Shown only
        // when there's a selection or something on the clipboard.
        if !canvas_locked
            && (!self.selection.is_empty()
                || !self.clipboard_dobjects.is_empty()
                || self.doc.active_layout.is_some())
        {
            resp.context_menu(|ui| {
                ui.set_min_width(150.0);
                // Group ▸ (flyout submenu)
                ui.menu_button("Group", |ui| {
                    if ui.button("Group     Ctrl+G").clicked() {
                        self.group_selection();
                        ui.close_menu();
                    }
                    if ui.button("Add to Group").clicked() {
                        self.add_to_group();
                        ui.close_menu();
                    }
                    if ui.button("Ungroup").clicked() {
                        self.ungroup_selection();
                        ui.close_menu();
                    }
                });
                // Clipboard ▸ (flyout submenu)
                ui.menu_button("Clipboard", |ui| {
                    if ui.button("Copy      Ctrl+C").clicked() {
                        self.copy_selection();
                        let n = self.clipboard_dobjects.len();
                        ui.ctx()
                            .copy_text(format!("RUST-AutoRASM: {} object(s) on clipboard", n));
                        ui.close_menu();
                    }
                    if ui.button("Paste     Ctrl+V").clicked() {
                        self.start_paste();
                        ui.close_menu();
                    }
                });
                // DObject Snap ▸ (running-osnap toggles, same set as DSNAP)
                ui.menu_button("DObject Snap", |ui| {
                    for k in SnapKind::ALL {
                        let mut on = self.snap_enabled.is_enabled(k);
                        let label = format!("{:<5}  {}", k.name(), snap_blurb(k));
                        if ui.checkbox(&mut on, label).changed() {
                            self.snap_enabled.set(k, on);
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("All on").clicked() {
                            for k in SnapKind::ALL {
                                self.snap_enabled.set(k, true);
                            }
                        }
                        if ui.button("All off").clicked() {
                            self.snap_enabled = SnapSet::default();
                        }
                        if ui.button("Defaults").clicked() {
                            self.snap_enabled = SnapSet::defaults();
                        }
                    });
                });
                ui.separator();
                if ui.button("Inspector").clicked() {
                    self.info_window_open = true;
                    ui.close_menu();
                }
                if self.doc.active_layout.is_some() {
                    if !self.layout_selection.is_empty() {
                        ui.separator();
                        if ui.button("Viewport Scale / Properties…").clicked() {
                            let ent_idx = self.layout_selection[0];
                            if let Some(li) = self.doc.active_layout {
                                if let Some(layout) = self.doc.layouts.get(li) {
                                    if let Some(e) = layout.entities.get(ent_idx) {
                                        if let Some(vi) = layout
                                            .viewports
                                            .iter()
                                            .position(|v| v.shape_handle == Some(e.handle))
                                        {
                                            self.vp_edit_dialog = Some((li, vi));
                                        }
                                    }
                                }
                            }
                            ui.close_menu();
                        }
                        // Lock / Unlock the scale (freezes the model view).
                        {
                            let li = self.doc.active_layout.unwrap_or(0);
                            let ent_idx = self.layout_selection[0];
                            let cur_locked = self
                                .doc
                                .layouts
                                .get(li)
                                .and_then(|l| l.entities.get(ent_idx))
                                .and_then(|e| {
                                    self.doc
                                        .layouts
                                        .get(li)
                                        .and_then(|l| {
                                            l.viewports
                                                .iter()
                                                .find(|v| v.shape_handle == Some(e.handle))
                                        })
                                        .map(|v| v.locked)
                                })
                                .unwrap_or(false);
                            if ui
                                .button(if cur_locked {
                                    "🔓 Unlock scale"
                                } else {
                                    "🔒 Lock scale"
                                })
                                .clicked()
                            {
                                let sel = self.layout_selection.clone();
                                if let Some(layout) = self.doc.layouts.get_mut(li) {
                                    for &si in &sel {
                                        if let Some(e) = layout.entities.get(si) {
                                            let h = e.handle;
                                            if let Some(v) = layout
                                                .viewports
                                                .iter_mut()
                                                .find(|v| v.shape_handle == Some(h))
                                            {
                                                v.locked = !cur_locked;
                                            }
                                        }
                                    }
                                }
                                ui.close_menu();
                            }
                            if ui.button("Copy Viewport").clicked() {
                                self.copy_selected_viewports();
                                ui.close_menu();
                            }
                        }
                        ui.menu_button("Set CTB", |ui| {
                            let ent_idx = self.layout_selection.first().copied().unwrap_or(0);
                            if ui.button("None").clicked() {
                                self.set_viewport_ctb(ent_idx, None);
                                ui.close_menu();
                            }
                            if ui.button("Monochrome").clicked() {
                                self.set_viewport_ctb(ent_idx, Some("monochrome".into()));
                                ui.close_menu();
                            }
                            if ui.button("Grayscale").clicked() {
                                self.set_viewport_ctb(ent_idx, Some("grayscale".into()));
                                ui.close_menu();
                            }
                            if ui.button("Full Color").clicked() {
                                self.set_viewport_ctb(ent_idx, Some("fullcolor".into()));
                                ui.close_menu();
                            }
                            // The user's own CTBs (saved in the app's CTB folder).
                            let saved = ctb_list();
                            if !saved.is_empty() {
                                ui.separator();
                                for (name, _) in saved {
                                    if CTB_RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
                                        continue;
                                    }
                                    if ui.button(&name).clicked() {
                                        self.set_viewport_ctb(ent_idx, Some(name.clone()));
                                        ui.close_menu();
                                    }
                                }
                            }
                        });
                        if ui.button("Delete Viewport").clicked() {
                            self.delete_selected_viewports();
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button("Create Viewport").clicked() {
                        self.viewport_draw_state = 1;
                        self.viewport_draw_p1 = None;
                        self.viewport_draw_li = self.doc.active_layout;
                        self.set_prompt("Click first corner of viewport (Esc to cancel)");
                        ui.close_menu();
                    }
                }
                pp_cap_ui(ui, "canvas right-click menu");
            });
        }

        // Delete / Backspace: remove the selected paper entities (undo-aware;
        // viewport sidecars follow their frame entity).
        if !self.layout_selection.is_empty()
            && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        {
            // One undoable, shared removal (same path as the context menu).
            self.delete_selected_viewports();
        }
        // Esc clears the paper selection (the global Esc handler also clears
        // it, but this catches the lane's own frames).
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.layout_selection.clear();
        }
    }

    /// Switch the active tab (None = Model). Saves the outgoing space's
    /// camera (model → saved_model_*, layout → its Layout.camera), swaps the
    /// per-space layer tables (kernel tables are swapped, so all doc.layers
    /// consumers read the ACTIVE space's layers), clears both selections, and
    /// restores the incoming space's camera.
    pub(super) fn switch_to_tab(&mut self, target: Option<usize>) {
        if self.doc.active_layout == target {
            return;
        }
        self.layout_selection.clear();
        self.layout_grip_drag = None;
        self.layout_move_last = None;
        // Cancel any in-flight viewport-create flow and close the layout-bound
        // viewport dialogs — their state refers to the outgoing space.
        self.viewport_draw_state = 0;
        self.viewport_draw_p1 = None;
        self.viewport_draw_p2 = None;
        self.viewport_draw_li = None;
        self.viewport_scale_dialog_open = false;
        self.vp_edit_dialog = None;
        self.vp_edit_init_for = None;
        // Selection indices are space-specific: a model selection is meaningless
        // in paper space (and vice-versa). Clear on every switch so a selection
        // made in Model space can't be moved/edited from a layout, and grips
        // don't linger across the boundary.
        self.selection.clear();
        self.selected = None;
        let old = self.doc.active_layout;
        if let Some(oi) = old {
            if let Some(l) = self.doc.layouts.get_mut(oi) {
                l.camera.zoom = self.scale;
                l.camera.pan_x = self.world_offset.x;
                l.camera.pan_y = self.world_offset.y;
                std::mem::swap(&mut self.doc.layers, &mut l.layers);
            }
        } else {
            self.saved_model_scale = Some(self.scale);
            self.saved_model_offset = Some(self.world_offset);
        }
        self.doc.active_layout = target;
        if let Some(ni) = target {
            if let Some(l) = self.doc.layouts.get_mut(ni) {
                std::mem::swap(&mut self.doc.layers, &mut l.layers);
                self.scale = l.camera.zoom;
                self.world_offset = egui::vec2(l.camera.pan_x, l.camera.pan_y);
            }
        } else {
            if let (Some(s), Some(o)) = (self.saved_model_scale, self.saved_model_offset) {
                self.scale = s;
                self.world_offset = o;
                self.saved_model_scale = None;
                self.saved_model_offset = None;
            }
        }
        self.touch_view();
        self.index_dirty = true;
    }

    pub(super) fn open_units_dialog(&mut self) {
        self.units_dialog_draft = self.doc.units.clone();
        // Seed "N mm = M units" from the current calibration: N = mm per unit,
        // M = 1 (the canonical display of the same ratio the user last set).
        let mpu = self.units_dialog_draft.mm_per_unit();
        self.units_dlg_n = if mpu > 0.0 { mpu } else { 1.0 };
        self.units_dlg_m = 1.0;
        self.units_dialog_open = true;
    }

    /// Drawing Units dialog — AutoCAD DDUNITS analog. Length + Angle display
    /// format/precision, the Insertion-scale unit (what 1 app unit represents),
    /// and a live Sample Output. OK applies the draft to `doc.units`.
    pub(super) fn render_units_dialog(&mut self, ctx: &egui::Context) {
        use cad_kernel::{AngleFormat, LengthFormat};
        let mut ok = false;
        let mut cl = false;
        // Precision options: 0 → "0", 1 → "0.0", … 8 → "0.00000000".
        let prec_label = |p: u8| -> String {
            if p == 0 {
                "0".to_string()
            } else {
                format!("0.{}", "0".repeat(p as usize))
            }
        };
        let d = &mut self.units_dialog_draft;
        egui::Window::new("Drawing Units")
            .order(egui::Order::Foreground)
            .id(egui::Id::new("units_dlg"))
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0., 0.))
            .show(ctx, |ui| {
                ui.columns(2, |cols| {
                    // ---- Length ----
                    cols[0].group(|ui| {
                        ui.label(egui::RichText::new("Length").strong());
                        ui.label("Type:");
                        egui::ComboBox::from_id_salt("len_type")
                            .width(140.0)
                            .selected_text(d.length_format.label())
                            .show_ui(ui, |ui| {
                                for f in LengthFormat::ALL {
                                    ui.selectable_value(&mut d.length_format, f, f.label());
                                }
                            });
                        ui.add_space(4.);
                        ui.label("Precision:");
                        egui::ComboBox::from_id_salt("len_prec")
                            .width(140.0)
                            .selected_text(prec_label(d.length_precision))
                            .show_ui(ui, |ui| {
                                for p in 0u8..=8 {
                                    ui.selectable_value(&mut d.length_precision, p, prec_label(p));
                                }
                            });
                    });
                    // ---- Angle ----
                    cols[1].group(|ui| {
                        ui.label(egui::RichText::new("Angle").strong());
                        ui.label("Type:");
                        egui::ComboBox::from_id_salt("ang_type")
                            .width(140.0)
                            .selected_text(d.angle_format.label())
                            .show_ui(ui, |ui| {
                                for f in AngleFormat::ALL {
                                    ui.selectable_value(&mut d.angle_format, f, f.label());
                                }
                            });
                        ui.add_space(4.);
                        ui.label("Precision:");
                        egui::ComboBox::from_id_salt("ang_prec")
                            .width(140.0)
                            .selected_text(prec_label(d.angle_precision))
                            .show_ui(ui, |ui| {
                                for p in 0u8..=8 {
                                    ui.selectable_value(&mut d.angle_precision, p, prec_label(p));
                                }
                            });
                        ui.checkbox(&mut d.angle_clockwise, "Clockwise");
                    });
                });

                ui.add_space(6.);
                // ---- Insertion scale ----
                ui.group(|ui| {
                    ui.label(egui::RichText::new("Insertion scale").strong());
                    ui.label("Unit type:");
                    let cur_label = d.insert_label();
                    egui::ComboBox::from_id_salt("insert_unit")
                        .width(200.0)
                        .selected_text(cur_label)
                        .show_ui(ui, |ui| {
                            for (disp, sym) in cad_kernel::INSERT_UNITS {
                                let sel = d.name.eq_ignore_ascii_case(sym);
                                if ui.selectable_label(sel, *disp).clicked() {
                                    d.name = sym.to_string();
                                }
                            }
                        });
                    ui.add_space(6.);
                    // ---- Drawing scale: N mm = M units ----
                    // The physical calibration: `N` millimetres on paper/reality =
                    // `M` drawing units, measured in the selected unit type.
                    // scene_per_unit is derived on OK: mm_per_unit = N/M, and
                    // mm_per_unit = mm_per_named(name) × scene_per_unit.
                    ui.horizontal(|ui| {
                        ui.label("Scale:");
                        ui.add(
                            egui::DragValue::new(&mut self.units_dlg_n)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(1e-9..=1e12),
                        );
                        ui.label("mm =");
                        ui.add(
                            egui::DragValue::new(&mut self.units_dlg_m)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(1e-9..=1e12),
                        );
                        ui.label("unit(s)");
                    });
                    ui.label(
                        egui::RichText::new(
                            "N mm = M drawing units — e.g. 1 mm = 100 units for a 1:100 \
                     drawing; 1000 mm = 1 unit with metres selected = 1 m per unit.",
                        )
                        .small()
                        .weak(),
                    );
                });

                ui.add_space(6.);
                // ---- Sample Output ----
                ui.group(|ui| {
                    ui.label(egui::RichText::new("Sample Output").strong());
                    let (l1, l2) = d.sample_output();
                    ui.label(egui::RichText::new(l1).monospace());
                    ui.label(egui::RichText::new(l2).monospace());
                });

                ui.add_space(8.);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        ok = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cl = true;
                    }
                });
            });

        if ok {
            // Apply the drawing scale: mm_per_unit = N/M, and
            // mm_per_unit = mm_per_named(unit type) × scene_per_unit, so
            // scene_per_unit = (N/M) / mm_per_named. "1 mm = 1 unit" (mm type)
            // keeps the historical default (scene_per_unit = 1).
            let d = &mut self.units_dialog_draft;
            let ratio = if self.units_dlg_m > 0.0 {
                self.units_dlg_n / self.units_dlg_m
            } else {
                1.0
            };
            let mpn = cad_kernel::Units::mm_per_named(&d.name);
            d.scene_per_unit = if ratio > 0.0 && mpn > 0.0 {
                (ratio / mpn).max(1e-9)
            } else {
                1.0
            };
            self.doc.units = self.units_dialog_draft.clone();
            let u = &self.doc.units;
            self.history.push(format!(
                "  ✔ units → {} ({}, {} dp)  scale {:.6} mm/unit  angle {} ({} dp){}",
                u.insert_label(),
                u.length_format.label(),
                u.length_precision,
                u.mm_per_unit(),
                u.angle_format.label(),
                u.angle_precision,
                if u.angle_clockwise { ", CW" } else { "" }
            ));
            self.units_dialog_open = false;
        }
        if cl {
            self.units_dialog_open = false;
        }
    }

    pub(super) fn render_new_layout_dialog(&mut self, ctx: &egui::Context) {
        use cad_kernel::plotstyle::PaperSize;
        let mut cl = false;
        let mut cr = false;
        egui::Window::new("New Layout")
            .id(egui::Id::new("nld"))
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0., 0.))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.new_layout_name);
                });
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    ui.label("Paper:");
                    let ps = ["A4", "A3", "A2", "A1", "A0", "Letter"];
                    let cur = match self.new_layout_paper {
                        PaperSize::A4 => 0,
                        PaperSize::A3 => 1,
                        PaperSize::A2 => 2,
                        PaperSize::A1 => 3,
                        PaperSize::A0 => 4,
                        _ => 5,
                    };
                    let mut s = cur;
                    egui::ComboBox::from_id_salt("nlpp")
                        .selected_text(ps[s])
                        .show_ui(ui, |ui| {
                            for (i, n) in ps.iter().enumerate() {
                                ui.selectable_value(&mut s, i, *n);
                            }
                        });
                    if s != cur {
                        self.new_layout_paper = match s {
                            0 => PaperSize::A4,
                            1 => PaperSize::A3,
                            2 => PaperSize::A2,
                            3 => PaperSize::A1,
                            4 => PaperSize::A0,
                            _ => PaperSize::Letter,
                        };
                    }
                });
                ui.add_space(4.);
                ui.checkbox(&mut self.new_layout_landscape, "Landscape");
                ui.add_space(4.);
                ui.horizontal(|ui| {
                    ui.label("CTB:");
                    let key = self.new_layout_ctb.clone();
                    let (ctbs, mut ci) = self.ctb_picker_state(&key);
                    egui::ComboBox::from_id_salt("nl_ctb")
                        .selected_text(ctbs[ci].0.clone())
                        .show_ui(ui, |ui| {
                            for (i, (disp, _)) in ctbs.iter().enumerate() {
                                ui.selectable_value(&mut ci, i, disp);
                            }
                        });
                    self.new_layout_ctb = ctbs[ci].1.clone().unwrap_or_default();
                });
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cl = true;
                    }
                    if ui.button("Create").clicked() {
                        cr = true;
                    }
                });
            });
        if cr {
            let n = if self.new_layout_name.trim().is_empty() {
                format!("Layout {}", self.doc.layouts.len() + 1)
            } else {
                self.new_layout_name.trim().to_string()
            };
            let o = if self.new_layout_landscape {
                cad_kernel::plotstyle::Orientation::Landscape
            } else {
                cad_kernel::plotstyle::Orientation::Portrait
            };
            let mut l = cad_kernel::Layout::new(&n, self.new_layout_paper, o);
            l.ctb_name = if self.new_layout_ctb.is_empty() {
                None
            } else {
                Some(self.new_layout_ctb.clone())
            };
            // Creating a layout is an undoable doc edit — mark the drawing
            // dirty too, or layout-only sessions never trigger autosave / the
            // unsaved-changes prompt.
            self.snapshot_doc();
            self.doc.layouts.push(l);
            self.switch_to_tab(Some(self.doc.layouts.len() - 1));
            self.show_new_layout_dialog = false;
        }
        if cl {
            self.show_new_layout_dialog = false;
        }
    }

    fn set_viewport_camera(
        &mut self,
        li: usize,
        vi: usize,
        center: (f64, f64),
        zoom: f64,
        scale: f64,
    ) {
        let Some(layout) = self.doc.layouts.get_mut(li) else {
            return;
        };
        let handle = match layout.viewports.get(vi) {
            Some(v) => v.shape_handle,
            None => return,
        };
        // authoritative: the paper entity
        if let Some(h) = handle {
            if let Some(e) = layout.entities.iter_mut().find(|e| e.handle == h) {
                if let cad_kernel::Geom::Viewport(ref mut vg) = e.geom {
                    vg.model_center.x = center.0;
                    vg.model_center.y = center.1;
                    vg.model_zoom = zoom;
                    vg.model_scale = scale;
                }
            }
        }
        // mirror: the sidecar the render loop + RSM read — single source of
        // truth (Layout::sync_all_viewports), so the two can't drift.
        layout.sync_all_viewports();
    }

    fn set_viewport_ctb(&mut self, ent_idx: usize, new: Option<String>) {
        let Some(li) = self.doc.active_layout else {
            return;
        };
        let Some(layout) = self.doc.layouts.get(li) else {
            return;
        };
        let Some(e) = layout.entities.get(ent_idx) else {
            return;
        };
        let Some(vi) = layout
            .viewports
            .iter()
            .position(|v| v.shape_handle == Some(e.handle))
        else {
            return;
        };
        if layout.viewports[vi].ctb_name.as_deref() == new.as_deref() {
            return;
        }
        self.snapshot_doc();
        if let Some(layout) = self.doc.layouts.get_mut(li) {
            layout.viewports[vi].ctb_name = new;
        }
        // CTB lives only in ViewportData (no geom counterpart) and the camera is
        // unchanged here — no camera sync needed (P0: camera routes through
        // set_viewport_camera only).
    }

    fn layout_rect_overlaps(
        ents: &[cad_kernel::DObject],
        skip_idx: usize,
        nmn: (f64, f64),
        nmx: (f64, f64),
    ) -> bool {
        for (j, oe) in ents.iter().enumerate() {
            if j == skip_idx {
                continue;
            }
            if let cad_kernel::Geom::Viewport(ov) = &oe.geom {
                let (omn, omx) = ov.bbox_world();
                if nmn.0 < omx.x && nmx.0 > omn.x && nmn.1 < omx.y && nmx.1 > omn.y {
                    return true;
                }
            }
        }
        false
    }

    fn paint_lock_badge(p: &egui::Painter, c: egui::Pos2, locked: bool) -> egui::Rect {
        let col = if locked {
            egui::Color32::from_rgb(255, 200, 80)
        } else {
            egui::Color32::from_rgb(130, 140, 155)
        };
        // body
        let body = egui::Rect::from_center_size(c + egui::vec2(0.0, 2.5), egui::vec2(10.0, 8.0));
        p.rect_filled(body, 1.5, col);
        // shackle (half-circle above the body); open lock shifts it aside
        let sh_c = if locked {
            egui::pos2(c.x, c.y - 2.0)
        } else {
            egui::pos2(c.x + 2.0, c.y - 2.0)
        };
        let r = 3.2_f32;
        let pts: Vec<egui::Pos2> = (0..=10)
            .map(|i| {
                let a = std::f32::consts::PI * (i as f32 / 10.0);
                egui::pos2(sh_c.x - r * a.cos(), sh_c.y - r * a.sin())
            })
            .collect();
        p.add(egui::Shape::line(pts, egui::Stroke::new(1.6, col)));
        if locked {
            p.circle_filled(
                c + egui::vec2(0.0, 2.5),
                1.3,
                egui::Color32::from_rgb(30, 30, 30),
            );
        }
        egui::Rect::from_center_size(c, egui::vec2(16.0, 16.0))
    }

    fn apply_vp_ctb(
        rgb: (u8, u8, u8),
        aci: Option<u8>,
        ctb: &str,
        table: Option<&cad_kernel::plotstyle::PlotStyleTable>,
    ) -> egui::Color32 {
        // Same semantics as the plot scene's `apply_ctb` (e0fddd1 / aa95957):
        // empty → monochrome, so the "Print colors" canvas view always matches
        // the plot output. A resolved table (a saved CTB — including an edited
        // built-in) applies its per-ACI colour rules; without one the built-in
        // names keep the hardcoded transform.
        let ctb = if ctb.is_empty() { "monochrome" } else { ctb };
        if let Some(t) = table {
            let (r, g, b) = t.apply_color(aci, rgb);
            return egui::Color32::from_rgb(r, g, b);
        }
        match ctb {
            "monochrome" => egui::Color32::from_rgb(0, 0, 0),
            // Rec.601 luminance — matches PlotStyleTable::apply_color.
            "grayscale" => {
                let g = (0.299 * rgb.0 as f32 + 0.587 * rgb.1 as f32 + 0.114 * rgb.2 as f32)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                egui::Color32::from_rgb(g, g, g)
            }
            _ => egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2),
        }
    }

    /// Remove the selected paper entities (viewport sidecars follow their
    /// frame entity). UNDO-AWARE: one snapshot covers the whole deletion, so
    /// both the context-menu "Delete Viewport" and the Delete-key path share
    /// one undoable implementation.
    fn delete_selected_viewports(&mut self) {
        if self.layout_selection.is_empty() {
            return;
        }
        let li = match self.doc.active_layout {
            Some(i) => i,
            None => return,
        };
        let sel = self.layout_selection.clone();
        self.snapshot_doc();
        if let Some(layout) = self.doc.layouts.get_mut(li) {
            let mut indices = sel.clone();
            indices.sort_unstable_by(|a, b| b.cmp(a));
            for &i in &indices {
                if i < layout.entities.len() {
                    let h = layout.entities[i].handle;
                    layout.viewports.retain(|v| v.shape_handle != Some(h));
                    layout.entities.remove(i);
                }
            }
        }
        self.layout_selection.clear();
        self.layout_grip_drag = None;
        self.layout_move_last = None;
        self.history
            .push(format!("  deleted {} paper entit(y/ies)", sel.len()));
    }

    fn copy_selected_viewports(&mut self) {
        if self.layout_selection.is_empty() {
            return;
        }
        let li = match self.doc.active_layout {
            Some(i) => i,
            None => return,
        };
        let sel = self.layout_selection.clone();
        // One undo step for the whole copy batch.
        self.snapshot_doc();
        let mut new_sel: Vec<usize> = Vec::new();
        let Some(layout) = self.doc.layouts.get_mut(li) else {
            return;
        };
        for &si in &sel {
            let Some(src_e) = layout.entities.get(si).cloned() else {
                continue;
            };
            let cad_kernel::Geom::Viewport(svg) = &src_e.geom else {
                continue;
            };
            // Find a non-overlapping offset (step by the width until clear).
            let mut vg = svg.clone();
            let step = svg.width.max(10.0) * 0.25 + 8.0;
            let mut off = step;
            let mut placed = false;
            for _ in 0..64 {
                let nc = Vec2::new(svg.center.x + off, svg.center.y + off);
                let nmn = (nc.x - svg.width * 0.5, nc.y - svg.height * 0.5);
                let nmx = (nc.x + svg.width * 0.5, nc.y + svg.height * 0.5);
                if !Self::layout_rect_overlaps(&layout.entities, usize::MAX, nmn, nmx) {
                    vg.center = nc;
                    placed = true;
                    break;
                }
                off += step;
            }
            if !placed {
                vg.center = Vec2::new(svg.center.x + off, svg.center.y + off);
            }
            let src_vd = layout
                .viewports
                .iter()
                .find(|v| v.shape_handle == Some(src_e.handle))
                .cloned();
            let d = cad_kernel::DObject::new(cad_kernel::Geom::Viewport(vg.clone()));
            let h = d.handle;
            layout.entities.push(d);
            new_sel.push(layout.entities.len() - 1);
            let mut vd = src_vd.unwrap_or_else(|| {
                cad_kernel::ViewportData::new(
                    (0., 0.),
                    (0., 0.),
                    (0., 0.),
                    vg.model_zoom,
                    vg.model_scale,
                )
            });
            vd.shape_handle = Some(h);
            vd.rect_min = (vg.center.x - vg.width * 0.5, vg.center.y - vg.height * 0.5);
            vd.rect_max = (vg.center.x + vg.width * 0.5, vg.center.y + vg.height * 0.5);
            layout.viewports.push(vd);
        }
        if !new_sel.is_empty() {
            self.layout_selection = new_sel;
            self.history.push("  ✔ viewport copied".into());
        }
    }

    pub(super) fn render_viewport_scale_dialog(&mut self, ctx: &egui::Context) {
        let mut cl = false;
        let mut cr = false;
        egui::Window::new("New Viewport — Scale")
            .id(egui::Id::new("vsd"))
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0., 0.))
            .show(ctx, |ui| {
                ui.label("Viewport scale (e.g. 1:100, 1:50):");
                ui.add_space(4.);
                ui.horizontal(|ui| {
                    ui.label("Scale:  1:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.viewport_scale_text)
                            .desired_width(60.),
                    );
                });
                ui.add_space(4.);
                ui.horizontal(|ui| {
                    for &s in &[1., 2., 5., 10., 20., 50., 100., 200., 500.] {
                        if ui.button(format!("1:{:.0}", s)).clicked() {
                            self.viewport_scale_text = format!("1:{:.0}", s);
                            self.viewport_custom_n.clear();
                            self.viewport_custom_m.clear();
                        }
                    }
                });
                ui.add_space(4.);
                // Custom paper scale: N <unit> = M units (both editable; the unit is
                // the PAPER unit of N — mm/cm/m). Overrides the 1:N field.
                ui.horizontal(|ui| {
                    ui.label("Custom:");
                    let mut unit = self.viewport_custom_unit.clone();
                    egui::ComboBox::from_id_salt("vp_cu")
                        .selected_text(unit.clone())
                        .width(56.0)
                        .show_ui(ui, |ui| {
                            for u in ["mm", "cm", "m"] {
                                if ui.selectable_label(unit == u, u).clicked() {
                                    unit = u.to_string();
                                }
                            }
                        });
                    self.viewport_custom_unit = unit;
                    ui.add(
                        egui::TextEdit::singleline(&mut self.viewport_custom_n)
                            .desired_width(52.)
                            .hint_text("N"),
                    );
                    ui.label("=");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.viewport_custom_m)
                            .desired_width(52.)
                            .hint_text("M"),
                    );
                    ui.label("unit(s) on paper");
                });
                ui.label(
                    egui::RichText::new("(custom overrides the 1:N field — e.g. 2 cm = 5 units)")
                        .small()
                        .weak(),
                );
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    ui.label("CTB:");
                    let key = self.viewport_ctb_name.clone();
                    let (ctbs, mut ci) = self.ctb_picker_state(&key);
                    egui::ComboBox::from_id_salt("vp_ctb")
                        .selected_text(ctbs[ci].0.clone())
                        .show_ui(ui, |ui| {
                            for (i, (disp, _)) in ctbs.iter().enumerate() {
                                ui.selectable_value(&mut ci, i, disp);
                            }
                        });
                    self.viewport_ctb_name = ctbs[ci].1.clone().unwrap_or_default();
                });
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cl = true;
                    }
                    if ui.button("Create Viewport").clicked() {
                        cr = true;
                    }
                });
            });
        if cr {
            if let (Some(mn), Some(mx)) = (self.viewport_draw_p1, self.viewport_draw_p2) {
                let sc: f64 = self
                    .viewport_scale_text
                    .trim()
                    .trim_start_matches("1:")
                    .trim()
                    .parse()
                    .unwrap_or(100.);
                // Units-correct scale (plotting spec Part 5): store paper-mm-per-scene
                // in model_scale so a physical length prints at length×(1:N) regardless
                // of the document unit. For the default doc (1 unit = 1 mm) this equals
                // 1/N — identical to the old behaviour. model_zoom stays the free-zoom.
                // Custom "N <unit> = M units" overrides the 1:N ratio: paper-mm-per-
                // scene = N×mm_per_named(unit)/M, applied directly as model_scale.
                let ms = match (
                    self.eval_field(&self.viewport_custom_n),
                    self.eval_field(&self.viewport_custom_m),
                ) {
                    (Ok(n), Ok(m)) if n > 0.0 && m > 0.0 => {
                        n * cad_kernel::Units::mm_per_named(&self.viewport_custom_unit) / m
                    }
                    _ => {
                        let desired = if sc > 0. { 1. / sc } else { 1. };
                        self.doc.units.viewport_paper_mm_per_scene(desired)
                    }
                };
                // New-viewport camera: centre on the MODEL EXTENTS and zoom to FIT
                // them into the viewport, so a fresh layout actually shows the
                // drawing. A plain 1:100-at-origin default renders the shapes as a
                // ~1px dot on screen and an empty sheet in the plot — "nothing
                // visible in the layout" (only fixed-screen-size points show).
                // `model_scale` keeps the chosen plot scale; the fit lives in
                // `model_zoom` (the free zoom), so the canvas view and the plot
                // always match.
                let (mut mc_x, mut mc_y, mut mz) = (0.0f64, 0.0f64, 1.0f64);
                if let Some((emin, emax)) = self.doc_extents() {
                    let bw = (emax.x - emin.x).max(1e-9);
                    let bh = (emax.y - emin.y).max(1e-9);
                    let vw = (mx.x - mn.x).max(1e-9);
                    let vh = (mx.y - mn.y).max(1e-9);
                    let fit = (vw / bw).min(vh / bh);
                    if fit.is_finite() && fit > 0.0 && ms > 0.0 {
                        mz = (fit / ms).clamp(0.001, 100.0);
                        mc_x = (emin.x + emax.x) * 0.5;
                        mc_y = (emin.y + emax.y) * 0.5;
                    }
                }
                let vg = cad_kernel::ViewportGeom {
                    center: Vec2::new((mn.x + mx.x) * 0.5, (mn.y + mx.y) * 0.5),
                    width: mx.x - mn.x,
                    height: mx.y - mn.y,
                    model_center: Vec2::new(mc_x, mc_y),
                    model_zoom: mz,
                    model_scale: ms,
                    frame_visible: true,
                };
                let vw = vg.width;
                let vh = vg.height;
                let d = cad_kernel::DObject::new(cad_kernel::Geom::Viewport(vg));
                let h = d.handle;
                let vd = cad_kernel::ViewportData {
                    shape_handle: Some(h),
                    rect_min: (mn.x, mn.y),
                    rect_max: (mx.x, mx.y),
                    model_center: (mc_x, mc_y),
                    model_zoom: mz,
                    model_scale: ms,
                    frozen_layers: Vec::new(),
                    ctb_name: if self.viewport_ctb_name.trim().is_empty() {
                        None
                    } else {
                        Some(self.viewport_ctb_name.trim().to_string())
                    },
                    locked: false,
                };
                let li = self
                    .viewport_draw_li
                    .or_else(|| self.doc.active_layout)
                    .unwrap_or(0);
                if let Some(l) = self.doc.layouts.get_mut(li) {
                    l.entities.push(d);
                    l.viewports.push(vd);
                    self.history
                        .push(format!("  Created viewport {}x{}mm", vw, vh));
                }
            }
            self.viewport_scale_dialog_open = false;
            self.viewport_draw_p1 = None;
            self.viewport_draw_p2 = None;
            self.viewport_draw_li = None;
            self.viewport_custom_n.clear();
            self.viewport_custom_m.clear();
        }
        if cl {
            self.viewport_scale_dialog_open = false;
            self.viewport_draw_p1 = None;
            self.viewport_draw_p2 = None;
            self.viewport_draw_li = None;
        }
    }

    pub(super) fn render_vp_edit_dialog(&mut self, ctx: &egui::Context) {
        // Bound to the layout it was opened from — tab switches close the
        // dialog (switch_to_tab), so `li` can never point at another space.
        let Some((li, vp_idx)) = self.vp_edit_dialog else {
            return;
        };
        let Some(layout) = self.doc.layouts.get(li) else {
            self.vp_edit_dialog = None;
            return;
        };
        if vp_idx >= layout.viewports.len() {
            self.vp_edit_dialog = None;
            return;
        }
        let vp = layout.viewports[vp_idx].clone();
        let mut cl = false;
        // SEED the persistent buffers ONCE per open (nominal 1:N from model_scale,
        // inverted through the doc units). Recomputing every frame is what froze
        // the field. Thereafter the user edits the buffers freely.
        if self.vp_edit_init_for != Some((li, vp_idx)) {
            let desired = self.doc.units.viewport_nominal_scale(vp.model_scale);
            self.vp_edit_scale_text = if desired > 0. {
                format!("1:{:.0}", 1.0 / desired)
            } else {
                String::from("1:100")
            };
            self.vp_edit_ctb = vp.ctb_name.clone().unwrap_or_default();
            self.vp_edit_init_for = Some((li, vp_idx));
        }
        // Edit copies (written back after the closure so the closure doesn't hold
        // a `self` field borrow across the whole UI).
        let mut sc_text = self.vp_edit_scale_text.clone();
        let mut ctb_name = self.vp_edit_ctb.clone();
        egui::Window::new("Viewport Properties")
            .id(egui::Id::new("vpe"))
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0., 0.))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Scale:");
                    ui.add(egui::TextEdit::singleline(&mut sc_text).desired_width(60.));
                });
                ui.horizontal(|ui| {
                    for &s in &[1., 2., 5., 10., 20., 50., 100., 200., 500.] {
                        if ui.button(format!("1:{:.0}", s)).clicked() {
                            sc_text = format!("1:{:.0}", s);
                        }
                    }
                });
                ui.add_space(6.);
                ui.horizontal(|ui| {
                    ui.label("CTB:");
                    let (ctbs, mut ci) = self.ctb_picker_state(&ctb_name);
                    egui::ComboBox::from_id_salt("vpe_ctb")
                        .selected_text(ctbs[ci].0.clone())
                        .show_ui(ui, |ui| {
                            for (i, (disp, _)) in ctbs.iter().enumerate() {
                                ui.selectable_value(&mut ci, i, disp);
                            }
                        });
                    ctb_name = ctbs[ci].1.clone().unwrap_or_default();
                });
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cl = true;
                    }
                    if ui.button("Apply").clicked() {
                        let sc: f64 = sc_text
                            .trim()
                            .trim_start_matches("1:")
                            .trim()
                            .parse()
                            .unwrap_or(100.);
                        // Snap to the EXACT units-correct scale: model_scale = paper-mm-per-
                        // scene for 1:N; reset the free-zoom to 1 so effective == nominal.
                        let desired = if sc > 0. { 1. / sc } else { 1. };
                        let ms = self.doc.units.viewport_paper_mm_per_scene(desired);
                        let ctb = ctb_name.clone();
                        self.snapshot_doc();
                        let cur_c = self
                            .doc
                            .layouts
                            .get(li)
                            .and_then(|l| l.viewports.get(vp_idx))
                            .map(|v| v.model_center)
                            .unwrap_or((0.0, 0.0));
                        if let Some(l) = self.doc.layouts.get_mut(li) {
                            if let Some(vd) = l.viewports.get_mut(vp_idx) {
                                vd.ctb_name = if ctb.is_empty() { None } else { Some(ctb) };
                            }
                        }
                        self.set_viewport_camera(li, vp_idx, cur_c, 1.0, ms);
                        cl = true;
                    }
                });
            });
        // Persist edits across frames (this is what un-freezes the field).
        self.vp_edit_scale_text = sc_text;
        self.vp_edit_ctb = ctb_name;
        if cl {
            self.vp_edit_dialog = None;
            self.vp_edit_init_for = None;
        }
    }

    // ===================================================================
    // PLOT — dockable Page Setup, Plot dialog (model space), Plot Style
    // Table Editor (CTB), preview window, and the run_plot pipeline.
    // Ported from the upstream RUST-AutoRASM (model-space parts; the fork
    // has no layout tabs, so the layout plot dialog is not ported).
    // ===================================================================

    /// Rasterise the layer-status glyphs (on/off/frozen/… from `layer_glyphs.rs`)
    /// into `layer_glyph_tex` once, then blit them tinted wherever a panel needs
    /// them. Idempotent — only fills missing keys.
    fn ensure_layer_glyph_textures(&mut self, ctx: &egui::Context) {
        use crate::layer_glyphs as lg;
        const E: &[(&str, &str)] = &[
            ("on", lg::SVG_ON),
            ("off", lg::SVG_OFF),
            ("frozen", lg::SVG_FROZEN),
            ("thawed", lg::SVG_THAWED),
            ("locked", lg::SVG_LOCKED),
            ("unlocked", lg::SVG_UNLOCKED),
            ("check", lg::SVG_CHECK),
            ("close", lg::SVG_CLOSE),
            ("stack", lg::SVG_STACK),
            ("spark", lg::SVG_SPARKLE),
            ("bcur", lg::SVG_BADGE_CUR),
            ("bdel", lg::SVG_BADGE_DEL),
            ("up", lg::SVG_UP),
            ("down", lg::SVG_DOWN),
            ("reset", lg::SVG_RESET),
            ("gpick", lg::SVG_GLYPHPICK),
            ("save", lg::SVG_SAVE),
            ("saveas", lg::SVG_SAVEAS),
            ("new", lg::SVG_NEW),
            ("open", lg::SVG_OPEN),
            ("import", lg::SVG_IMPORT),
            ("export", lg::SVG_EXPORT),
        ];
        for &(key, svg) in E {
            if !self.layer_glyph_tex.contains_key(key) {
                if let Some(t) = raster_layer_glyph(ctx, key, svg) {
                    self.layer_glyph_tex.insert(key, t);
                }
            }
        }
    }

    // ---- CTB manager (CTB menu) ------------------------------------------
    //
    // The CTB menu defines / edits / deletes plot-style tables stored as `.pst`
    // files in the app's `<exe_dir>/ctb` folder. Editing reuses the Plot Style
    // Table Editor (`doc.plot_styles` + `render_plot_style_editor`), bound to
    // its file so "Save changes" writes back to the folder.

    /// The CTB picker options: (display name, canonical stored key) for the
    /// built-ins + every saved CTB (dedup'd case-insensitively against the
    /// built-ins). `None` = unset.
    fn ctb_choices(&self) -> Vec<(String, Option<String>)> {
        let mut out: Vec<(String, Option<String>)> = CTB_BUILTINS
            .iter()
            .map(|(d, k)| (d.to_string(), k.map(str::to_string)))
            .collect();
        for (name, _) in ctb_list() {
            if !out.iter().any(|(d, _)| d.eq_ignore_ascii_case(&name)) {
                out.push((name.clone(), Some(name)));
            }
        }
        out
    }

    /// Picker state for the stored key `key`: the choice list plus the selected
    /// index. A saved name whose `.pst` file no longer exists is APPENDED as its
    /// own entry instead of collapsing to "None" — so opening a dialog (or its
    /// per-frame rewrite) never silently erases the stored value.
    fn ctb_picker_state(&self, key: &str) -> (Vec<(String, Option<String>)>, usize) {
        let mut choices = self.ctb_choices();
        let sel = choices.iter().position(|(_, k)| match k {
            None => key.is_empty(),
            Some(k) => k == key,
        });
        match sel {
            Some(i) => (choices, i),
            None => {
                choices.push((key.to_string(), Some(key.to_string())));
                let n = choices.len() - 1;
                (choices, n)
            }
        }
    }

    /// Rebuild the saved-CTB table cache from the on-disk fingerprint
    /// `(name, len, mtime)` of every `.pst` in the CTB folder. On change:
    /// rebuild changed/new entries and drop missing ones. Runs every frame
    /// from the update loop — cheap (one `read_dir` + stat of a few files),
    /// and it picks up EXTERNAL edits (a `.pst` re-saved outside the app)
    /// without a dirty flag.
    pub(super) fn refresh_ctb_tables(&mut self) {
        let mut list: Vec<(String, u64, Option<std::time::SystemTime>)> = Vec::new();
        let mut paths: Vec<(String, std::path::PathBuf)> = Vec::new();
        for (name, path) in ctb_list() {
            let meta = std::fs::metadata(&path).ok();
            let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let mtime = meta.and_then(|m| m.modified().ok());
            list.push((name.clone(), len, mtime));
            paths.push((name, path));
        }
        if list == self.ctb_fingerprint {
            return;
        }
        // Drop entries whose file disappeared.
        let names: std::collections::HashSet<&str> =
            list.iter().map(|(n, _, _)| n.as_str()).collect();
        self.ctb_table_cache
            .retain(|n, _| names.contains(n.as_str()));
        // Rebuild only the entries that are new or changed.
        for (name, path) in &paths {
            let new_fp = list.iter().find(|(n, _, _)| n == name).cloned();
            let unchanged = new_fp
                .as_ref()
                .map(|fp| self.ctb_fingerprint.contains(fp))
                .unwrap_or(false);
            if self.ctb_table_cache.contains_key(name) && unchanged {
                continue; // unchanged — keep the cached table
            }
            if let Ok(t) = cad_io::load_plot_table(&path) {
                self.ctb_table_cache.insert(name.clone(), t);
            }
        }
        self.ctb_fingerprint = list;
    }

    /// Open a saved `.pst` CTB in the Plot Style Table Editor, bound to its file
    /// so "Save changes" persists the table back to the CTB folder.
    pub(super) fn ctb_open_editor(&mut self, path: &std::path::Path) {
        match cad_io::load_plot_table(path) {
            Ok(t) => {
                let name = t.name.clone();
                self.doc.plot_styles = t;
                self.plotstyle_edit_file = Some(path.to_path_buf());
                self.plotstyle_open = true;
                if self.plotstyle_sel.is_empty() {
                    self.plotstyle_sel = vec![1];
                    self.plotstyle_anchor = Some(1);
                }
                self.history
                    .push(format!("  CTB '{}' opened for editing", name));
            }
            Err(e) => self.history.push(format!("  ! CTB load failed: {}", e)),
        }
    }

    /// CTB menu → a BUILT-IN CTB (Monochrome / Grayscale / Full Color): opens
    /// the Plot Style Table Editor bound to `ctb/<name>.pst` — seeded with the
    /// built-in's semantics when no file exists yet ("Save changes" creates
    /// it). Once the file exists, the plot pipeline applies the edited table
    /// for that name instead of the hardcoded transform.
    pub(super) fn ctb_open_builtin(&mut self, name: &str) {
        let path = ctb_folder().join(format!("{}.pst", name));
        if path.exists() {
            self.ctb_open_editor(&path);
            return;
        }
        let Some(t) = ctb_builtin_seed(name) else {
            self.history
                .push(format!("  ! CTB: unknown built-in '{}'", name));
            return;
        };
        self.doc.plot_styles = t;
        self.plotstyle_edit_file = Some(path);
        self.plotstyle_open = true;
        if self.plotstyle_sel.is_empty() {
            self.plotstyle_sel = vec![1];
            self.plotstyle_anchor = Some(1);
        }
        self.history.push(format!(
            "  Built-in CTB '{}' opened for editing (unsaved)",
            name
        ));
    }

    /// Define a NEW CTB: write a default table named `raw_name` into the app's
    /// CTB folder, then open it in the editor (the user defines its pens there).
    fn ctb_create(&mut self, raw_name: &str) {
        let name: String = raw_name
            .trim()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .trim()
            .to_string();
        if name.is_empty() {
            self.history.push("  ! CTB: enter a name".into());
            return;
        }
        // Reject collisions with the built-ins and existing saved CTBs (case-
        // insensitive): the pickers dedup case-insensitively, so a colliding
        // file could never be selected anywhere.
        let lower = name.to_ascii_lowercase();
        if CTB_RESERVED.contains(&lower.as_str()) {
            self.history
                .push(format!("  ! CTB name '{}' is reserved (built-in)", name));
            return;
        }
        if ctb_list()
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(&name))
        {
            self.history
                .push(format!("  ! CTB '{}' already exists", name));
            return;
        }
        let path = ctb_folder().join(format!("{}.pst", name));
        if path.exists() {
            self.history
                .push(format!("  ! CTB '{}' already exists", name));
            return;
        }
        if let Err(e) = std::fs::create_dir_all(ctb_folder()) {
            self.history
                .push(format!("  ! CTB folder create failed: {}", e));
            return;
        }
        let table = cad_kernel::plotstyle::PlotStyleTable::named(name);
        if let Err(e) = cad_io::save_plot_table(&path, &table) {
            self.history.push(format!("  ! CTB save failed: {}", e));
            return;
        }
        self.ctb_open_editor(&path);
    }

    /// Delete a saved CTB from the app's CTB folder.
    fn ctb_delete(&mut self, name: &str) {
        let path = ctb_folder().join(format!("{}.pst", name));
        match std::fs::remove_file(&path) {
            Ok(()) => {
                if self.plotstyle_edit_file.as_deref() == Some(path.as_path()) {
                    self.plotstyle_edit_file = None;
                }
                self.history.push(format!("  CTB '{}' deleted", name));
            }
            Err(e) => self.history.push(format!("  ! CTB delete failed: {}", e)),
        }
    }

    /// CTB menu → New CTB… — name entry; Create defines the CTB (default table
    /// in the app's CTB folder) and opens it in the Plot Style Table Editor.
    pub(super) fn render_new_ctb_dialog(&mut self, ctx: &egui::Context) {
        if !self.new_ctb_open {
            return;
        }
        let mut ok = false;
        let mut cancel = false;
        egui::Window::new("New CTB")
            .id(egui::Id::new("new_ctb"))
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.new_ctb_name);
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("Create").clicked() {
                        ok = true;
                    }
                });
            });
        if ok {
            let name = self.new_ctb_name.clone();
            self.new_ctb_open = false;
            self.ctb_create(&name);
        }
        if cancel {
            self.new_ctb_open = false;
        }
    }

    // ---- Plot Style Table Editor ------------------------------------------

    /// Selection helpers for the 255-color list (Form view) and the grid column
    /// headers (Table view). `alt` removes one color; `shift` extends from the
    /// anchor; `ctrl` toggles one; plain click selects just that color.
    fn plotstyle_select(&mut self, n: u8, shift: bool, ctrl: bool, alt: bool) {
        if alt {
            if let Some(pos) = self.plotstyle_sel.iter().position(|&x| x == n) {
                self.plotstyle_sel.remove(pos);
            }
            self.plotstyle_anchor = Some(n);
        } else if shift {
            // Add the in-between colors to the current selection (keep anchor so
            // a further Shift+click keeps extending from the same start).
            self.plotstyle_extend_to(n);
        } else if ctrl {
            if let Some(pos) = self.plotstyle_sel.iter().position(|&x| x == n) {
                self.plotstyle_sel.remove(pos);
            } else {
                self.plotstyle_sel.push(n);
            }
            self.plotstyle_anchor = Some(n);
        } else {
            self.plotstyle_sel = vec![n];
            self.plotstyle_anchor = Some(n);
        }
    }

    /// Union every color between the anchor (or `n`) and `n` into the selection
    /// (Shift+click / Shift+End / Shift+Home).
    fn plotstyle_extend_to(&mut self, n: u8) {
        let anchor = self.plotstyle_anchor.unwrap_or(n);
        let (lo, hi) = if anchor <= n {
            (anchor, n)
        } else {
            (n, anchor)
        };
        for c in lo..=hi {
            if !self.plotstyle_sel.contains(&c) {
                self.plotstyle_sel.push(c);
            }
        }
    }

    /// Select all 255 colors (Ctrl+A while the color list / grid is hovered).
    fn plotstyle_select_all(&mut self) {
        self.plotstyle_sel = (1..=255).collect();
    }

    /// Read the shared color-list keyboard shortcuts (gated by the caller on the
    /// list/grid being hovered): Ctrl+A = all, Shift+End = extend to last,
    /// Shift+Home = extend to first.
    fn plotstyle_list_keys(&mut self, ui: &egui::Ui) {
        let (all, to_last, to_first) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::A) && (i.modifiers.command || i.modifiers.ctrl),
                i.key_pressed(egui::Key::End) && i.modifiers.shift,
                i.key_pressed(egui::Key::Home) && i.modifiers.shift,
            )
        });
        if all {
            self.plotstyle_select_all();
        }
        if to_last {
            self.plotstyle_extend_to(255);
        }
        if to_first {
            self.plotstyle_extend_to(1);
        }
    }

    /// Open a child dialog LOCKED to its parent's exact top-left, so it appears in
    /// the SAME position as the current menu (owner §2b). The window gets a FRESH
    /// Id per open (`base_id` + `seq`) so egui holds NO remembered position and
    /// `default_pos(parent.min)` always wins — a stale remembered spot can't
    /// hijack it. Within one open the Id is stable, so the child stays freely
    /// draggable. No parent (opened straight from a menu) → centre on the canvas.
    fn place_child<'a>(
        win: egui::Window<'a>,
        base_id: &str,
        seq: u64,
        parent: Option<egui::Rect>,
        canvas: egui::Rect,
    ) -> egui::Window<'a> {
        let win = win.id(egui::Id::new((base_id, seq)));
        match parent {
            Some(p) => win.pivot(egui::Align2::LEFT_TOP).default_pos(p.min),
            None => win
                .pivot(egui::Align2::CENTER_CENTER)
                .default_pos(canvas.center()),
        }
    }

    /// Clamp a desired centre so a `size`-big CENTER_CENTER-pivoted window sits
    /// fully inside `cr`; if the window is bigger than the canvas on an axis, use
    /// the canvas centre on that axis.
    fn clamp_center_in(cr: egui::Rect, center: egui::Pos2, size: egui::Vec2) -> egui::Pos2 {
        let (hx, hy) = (size.x * 0.5, size.y * 0.5);
        let x = if cr.width() >= size.x {
            center.x.clamp(cr.min.x + hx, cr.max.x - hx)
        } else {
            cr.center().x
        };
        let y = if cr.height() >= size.y {
            center.y.clamp(cr.min.y + hy, cr.max.y - hy)
        } else {
            cr.center().y
        };
        egui::pos2(x, y)
    }

    /// The Plot Style Table Editor (AutoCAD CTB editor analog) — Form View (A2).
    /// Standalone floating window with DIALOG_STANDARD chrome (like the Layer
    /// Manager). Left = 255-color list (multi-select); right = the 12 CTB
    /// properties, applied to every selected color. Edits `doc.plot_styles`.
    pub(super) fn render_plot_style_editor(&mut self, ctx: &egui::Context) {
        if !self.plotstyle_open {
            self.prev_plotstyle_open = false;
            return;
        }
        let ed_just_opened = !self.prev_plotstyle_open;
        self.prev_plotstyle_open = true;
        if ed_just_opened {
            self.plotstyle_open_seq = self.plotstyle_open_seq.wrapping_add(1);
        }
        self.ensure_layer_glyph_textures(ctx);
        let gx_close = self
            .layer_glyph_tex
            .get("close")
            .map(|t| (t.id(), t.size_vec2()));
        use crate::theme::color as tc;

        // Pre-clone the linetype list + ladder so the render closure needs no
        // borrow of self.doc for them.
        let ladder = self.doc.plot_styles.lineweight_ladder.clone();
        let lt_list: Vec<(u32, String)> = self
            .doc
            .linetypes
            .linetypes
            .iter()
            .enumerate()
            .map(|(i, l)| (i as u32, l.name.clone()))
            .collect();
        let table_name = self.doc.plot_styles.name.clone();

        let mut close_panel = false;
        let mut save_close = false;
        let mut open_color_picker = false;
        let mut open_ladder = false;
        let mut open_pst_save = false;
        let mut open_pst_load = false;
        let mut edit = PlotStyleEdit::default();

        let frame = egui::Frame::none()
            .fill(tc::SURFACE_1)
            .rounding(egui::Rounding::same(12.0))
            .stroke(egui::Stroke::new(1.0, tc::BORDER))
            .inner_margin(egui::Margin::ZERO);
        // FULLY FIXED size (NO auto-fit). body_h = Table View's 12 property
        // rows + the "Set" corner + caption + 24px below Fill style. The window
        // is header + tabs + body_h + footer — the SAME for all 3 tabs.
        let ed_w = 600.0_f32;
        let body_h = 24.0                    // "Set →" corner row
            + 12.0 * 24.0 + 11.0 * 2.0       // 12 property rows @24 + 2px spacing
            + 20.0                           // wrapped instruction caption + gap
            + 24.0; // 24px below Fill style (owner)
        let target_h = 30.0 + 34.0 + body_h + 78.0; // header + tabs + body + footer
                                                    // Open LOCKED to the PARENT's top-left (the Plot dialog it replaces),
                                                    // forced on the opening frame so a dragged parent carries the editor.
        let cr = self.canvas_screen_rect.unwrap_or_else(|| ctx.screen_rect());
        let win = egui::Window::new("Plot Style Table Editor")
            .order(egui::Order::Foreground)
            .title_bar(false)
            .frame(frame)
            .fixed_size(egui::vec2(ed_w, target_h))
            .movable(true);
        let win = Self::place_child(
            win,
            "plot_style_editor",
            self.plotstyle_open_seq,
            self.plot_dialog_rect,
            cr,
        );
        let resp = win.show(ctx, |ui| {
            let area = ui.max_rect();
            let full_w = area.width();
            let hdr_font = egui::TextStyle::Button.resolve(ui.style());

            // ===== header band (#34414B, title, Close ×) =====
            let (hdr, _) = ui.allocate_exact_size(egui::vec2(full_w, 30.0), egui::Sense::hover());
            {
                let hp = ui.painter_at(hdr);
                hp.rect_filled(hdr, egui::Rounding { nw: 12.0, ne: 12.0, sw: 0.0, se: 0.0 }, tc::BORDER);
                hp.text(egui::pos2(area.left() + 14.0, hdr.center().y), egui::Align2::LEFT_CENTER,
                    format!("Plot Style Table Editor — {}", table_name), hdr_font, tc::TEXT_PRIMARY);
            }
            let xr = egui::Rect::from_center_size(egui::pos2(area.right() - 13.0, hdr.center().y), egui::vec2(13.5, 13.5));
            let xresp = ui.interact(xr, egui::Id::new("pse_close"), egui::Sense::click());
            {
                let hp = ui.painter_at(hdr);
                let xcol = if xresp.hovered() {
                    hp.rect_filled(xr.expand(3.0), 4.0, egui::Color32::from_rgba_unmultiplied(0xE5, 0x48, 0x4D, 45));
                    LYR_DANGER
                } else { LYR_MUTED };
                blit_layer_glyph(&hp, xr, gx_close, xcol);
            }
            if xresp.clicked() { close_panel = true; }

            // ===== tab bar: General | Table View | Form View =====
            egui::Frame::none().inner_margin(egui::Margin { left: 14.0, right: 14.0, top: 4.0, bottom: 0.0 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (i, name) in ["General", "Table View", "Form View"].iter().enumerate() {
                        if ui.selectable_label(self.plotstyle_tab == i as u8, *name).clicked() {
                            self.plotstyle_tab = i as u8;
                        }
                    }
                });
            });
            ui.separator();

            // ===== the active tab body — fixed-height scroll so the WINDOW
            //       height is identical across the 3 tabs (unified). =====
            egui::Frame::none().inner_margin(egui::Margin::symmetric(14.0, 6.0)).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(body_h).show(ui, |ui| {
                    match self.plotstyle_tab {
                        0 => self.plotstyle_general_body(ui),
                        1 => self.plotstyle_table_body(ui, area, &ladder, &lt_list, &mut edit, &mut open_color_picker),
                        _ => self.plotstyle_form_body(ui, area, &ladder, &lt_list, &mut edit, &mut open_color_picker),
                    }
                });
            });

            // ===== shared footer — shown for ALL tabs (keeps the height unified). =====
            ui.separator();
            egui::Frame::none().inner_margin(egui::Margin::symmetric(14.0, 6.0)).show(ui, |ui| {
                if self.plotstyle_tab != 0 {
                    ui.label(egui::RichText::new(
                        "Dither · Pen # · Virtual pen · Adaptive · Fill (marked *) are stored for CTB fidelity — no PDF effect yet.")
                        .color(tc::TEXT_MUTED).small());
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    if ui.button("Edit Lineweights…").clicked() { open_ladder = true; }
                    if ui.button("Load .pst…").clicked() { open_pst_load = true; }
                    if ui.button("Save As .pst…").clicked() { open_pst_save = true; }
                    // Save changes — edits already apply live to doc.plot_styles, so
                    // this confirms + closes (returns to the Plot dialog). When the
                    // editor is bound to a CTB file (CTB menu → New/Edit CTB), the
                    // table is ALSO written back to that file here.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let save = egui::Button::new(egui::RichText::new("Save changes").color(tc::ON_ACCENT).strong())
                            .fill(tc::ACCENT).rounding(egui::Rounding::same(6.0));
                        if ui.add(save).clicked() { save_close = true; }
                    });
                });
                self.plotstyle_footer_rect = Some(ui.min_rect());
            });
        });
        self.plotstyle_editor_rect = resp.as_ref().map(|r| r.response.rect);
        self.raise_after_show("PlotStyleEditor", ctx, &resp);
        let _ = resp;

        // ---- apply pending edits to every selected color ----
        if open_color_picker {
            self.aci_pick_request = Some(AciPickRequest::PlotStyleColor);
        }
        let sel = self.plotstyle_sel.clone();
        apply_plot_edit(&mut self.doc.plot_styles, &sel, &edit);

        // "Save changes" while bound to a CTB file → write the table back to the
        // app's CTB folder (after the final frame's edits applied above).
        if save_close {
            if let Some(path) = self.plotstyle_edit_file.clone() {
                match cad_io::save_plot_table(&path, &self.doc.plot_styles) {
                    Ok(()) => {
                        self.history.push(format!(
                            "  CTB '{}' saved → {}",
                            self.doc.plot_styles.name,
                            path.display()
                        ));
                    }
                    Err(e) => self.history.push(format!("  ! CTB save failed: {}", e)),
                }
            }
            close_panel = true;
        }

        // Edit Lineweights / .pst — each REPLACES the editor (menu-stack): hide
        // the editor now, mark it to return, and open the sub-dialog.
        if open_ladder {
            self.plotstyle_ladder_open = true;
            self.plotstyle_ladder_work = self.doc.plot_styles.lineweight_ladder.clone();
            self.plotstyle_ladder_sel = None;
            self.plotstyle_open = false;
            self.plotstyle_return_after_sub = true;
        }
        if open_pst_save {
            self.plotstyle_pst_io = Some(true);
            self.open_file_dialog(FileDialogMode::Save, ".pst");
            self.plotstyle_open = false;
            self.plotstyle_return_after_sub = true;
        }
        if open_pst_load {
            self.plotstyle_pst_io = Some(false);
            self.open_file_dialog(FileDialogMode::Open, ".pst");
            self.plotstyle_open = false;
            self.plotstyle_return_after_sub = true;
        }

        if close_panel {
            self.plotstyle_open = false;
            // return to the Plot dialog if the editor replaced it.
            if self.plotstyle_return_to_plot {
                self.plot_dialog_open = true;
                self.plotstyle_return_to_plot = false;
            }
        }
    }

    /// Form View body — left color list + description; right = the 12 shared
    /// property editors (`plotprop_editor`), applied to every selected color.
    fn plotstyle_form_body(
        &mut self,
        ui: &mut egui::Ui,
        _area: egui::Rect,
        ladder: &[f32],
        lt_list: &[(u32, String)],
        edit: &mut PlotStyleEdit,
        open_color_picker: &mut bool,
    ) {
        use crate::theme::color as tc;
        let sel = self.plotstyle_sel.clone();
        ui.horizontal_top(|ui| {
            // ---- LEFT: color list + description ----
            ui.vertical(|ui| {
                ui.set_width(188.0);
                ui.label(
                    egui::RichText::new("Plot styles")
                        .color(tc::COLUMN_HEADER)
                        .small(),
                );
                // Show ~9 colors, then a scroll bar (owner). Row = 18px.
                let list_h = 12.0 * 18.0;
                let out = egui::ScrollArea::vertical()
                    .id_salt("pse_list")
                    .max_height(list_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let row_w = ui.available_width();
                        for n in 1u8..=255 {
                            let is_sel = sel.contains(&n);
                            let (rect, resp) = ui
                                .allocate_exact_size(egui::vec2(row_w, 18.0), egui::Sense::click());
                            if is_sel {
                                ui.painter().rect_filled(rect, 3.0, tc::SURFACE_3);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, 3.0, tc::SURFACE_2);
                            }
                            let (r, g, b) = aci_palette(n);
                            let sw = egui::Rect::from_min_size(
                                egui::pos2(rect.left() + 4.0, rect.center().y - 6.0),
                                egui::vec2(12.0, 12.0),
                            );
                            ui.painter()
                                .rect_filled(sw, 2.0, egui::Color32::from_rgb(r, g, b));
                            ui.painter()
                                .rect_stroke(sw, 2.0, egui::Stroke::new(1.0, tc::BORDER));
                            ui.painter().text(
                                egui::pos2(sw.right() + 8.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                format!("Color {}", n),
                                egui::TextStyle::Body.resolve(ui.style()),
                                if is_sel {
                                    tc::TEXT_PRIMARY
                                } else {
                                    tc::TEXT_SECONDARY
                                },
                            );
                            if resp.clicked() {
                                let m = ui.input(|i| i.modifiers);
                                self.plotstyle_select(n, m.shift, m.command || m.ctrl, m.alt);
                            }
                        }
                    });
                // Keyboard shortcuts while the color list is hovered.
                if ui.rect_contains_pointer(out.inner_rect) {
                    self.plotstyle_list_keys(ui);
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Description")
                        .color(tc::COLUMN_HEADER)
                        .small(),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut self.doc.plot_styles.description)
                        .desired_rows(2)
                        .desired_width(184.0),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_enabled(false, egui::Button::new("Add Style"))
                        .on_disabled_hover_text("Color-dependent table — the 255 colors are fixed");
                    ui.add_enabled(false, egui::Button::new("Delete Style"))
                        .on_disabled_hover_text("Color-dependent table — the 255 colors are fixed");
                });
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            // ---- RIGHT: the 12 shared property editors ----
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "Properties  ({} color{} selected)",
                        sel.len(),
                        if sel.len() == 1 { "" } else { "s" }
                    ))
                    .color(tc::COLUMN_HEADER)
                    .small(),
                );
                ui.add_space(6.0);
                if sel.is_empty() {
                    ui.label("Select one or more colors on the left.");
                    return;
                }
                egui::Grid::new("pse_form_props")
                    .num_columns(2)
                    .spacing([10.0, 7.0])
                    .show(ui, |ui| {
                        for prop in PlotProp::ALL {
                            let mut name = prop.label().to_string();
                            if prop.store_only() {
                                name.push_str(" *");
                            }
                            let l = ui.label(name);
                            if prop.store_only() {
                                l.on_hover_text("Stored — no plot effect yet");
                            }
                            plotprop_editor(
                                ui,
                                prop,
                                &sel,
                                &self.doc.plot_styles,
                                ladder,
                                lt_list,
                                edit,
                                open_color_picker,
                                "form",
                            );
                            ui.end_row();
                        }
                    });
            });
        });
    }

    /// Table View body — rows = the 12 properties, columns = the 255 colors.
    /// Left: property name + the SAME shared editor (`plotprop_editor`) applied
    /// to the selected column range. Right: a virtualized value grid (only the
    /// visible columns are painted). Click a column header to select (Shift =
    /// range, Ctrl = toggle).
    fn plotstyle_table_body(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        ladder: &[f32],
        lt_list: &[(u32, String)],
        edit: &mut PlotStyleEdit,
        open_color_picker: &mut bool,
    ) {
        use crate::theme::color as tc;
        let sel = self.plotstyle_sel.clone();
        ui.label(egui::RichText::new(
            "Click column headers to select colors (Shift = range, Ctrl = toggle), then set a property on the left — it applies to all selected columns.")
            .color(tc::TEXT_MUTED).small());
        ui.add_space(6.0);
        let (row_h, header_h, col_w) = (24.0_f32, 24.0_f32, 52.0_f32);
        let cap = egui::TextStyle::Small.resolve(ui.style());
        ui.horizontal_top(|ui| {
            // ---- LEFT: property name + shared editor per row ----
            ui.vertical(|ui| {
                ui.set_width(266.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                let (corner, _) =
                    ui.allocate_exact_size(egui::vec2(266.0, header_h), egui::Sense::hover());
                ui.painter().text(
                    egui::pos2(corner.left(), corner.center().y),
                    egui::Align2::LEFT_CENTER,
                    format!("Set →  ({} selected)", sel.len()),
                    cap.clone(),
                    tc::COLUMN_HEADER,
                );
                for prop in PlotProp::ALL {
                    ui.allocate_ui_with_layout(
                        egui::vec2(266.0, row_h),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let mut nm = prop.label().to_string();
                            if prop.store_only() {
                                nm.push_str(" *");
                            }
                            let l =
                                ui.add_sized([104.0, row_h - 2.0], egui::Label::new(nm).truncate());
                            if prop.store_only() {
                                l.on_hover_text("Stored — no plot effect yet");
                            }
                            if sel.is_empty() {
                                ui.label("—");
                            } else {
                                plotprop_editor(
                                    ui,
                                    prop,
                                    &sel,
                                    &self.doc.plot_styles,
                                    ladder,
                                    lt_list,
                                    edit,
                                    open_color_picker,
                                    "tbl",
                                );
                            }
                        },
                    );
                }
            });
            ui.add_space(6.0);

            // ---- RIGHT: virtualized value grid ----
            let grid_h = (area.height() - 220.0)
                .max(180.0)
                .min(header_h + 12.0 * row_h + 6.0);
            egui::ScrollArea::horizontal()
                .id_salt("pse_tbl")
                .max_height(grid_h)
                .show_viewport(ui, |ui, vp| {
                    let ncol = 255usize;
                    let content = egui::vec2(ncol as f32 * col_w, header_h + 12.0 * row_h);
                    let (resp, p) = ui.allocate_painter(content, egui::Sense::click());
                    let o = resp.rect.min;
                    let first = ((vp.min.x / col_w).floor() as isize).max(0) as usize;
                    let last = (((vp.max.x / col_w).ceil() as usize) + 1).min(ncol);
                    for ci in first..last {
                        let n = (ci + 1) as u8;
                        let x = o.x + ci as f32 * col_w;
                        let is_sel = sel.contains(&n);
                        // header
                        let hr = egui::Rect::from_min_size(
                            egui::pos2(x, o.y),
                            egui::vec2(col_w, header_h),
                        );
                        if is_sel {
                            p.rect_filled(hr, 0.0, tc::SURFACE_3);
                        }
                        p.rect_stroke(hr, 0.0, egui::Stroke::new(0.4, tc::BORDER));
                        let (r, g, b) = aci_palette(n);
                        let sw = egui::Rect::from_min_size(
                            egui::pos2(x + 3.0, o.y + header_h / 2.0 - 5.0),
                            egui::vec2(10.0, 10.0),
                        );
                        p.rect_filled(sw, 2.0, egui::Color32::from_rgb(r, g, b));
                        p.rect_stroke(sw, 2.0, egui::Stroke::new(0.5, tc::BORDER));
                        p.text(
                            egui::pos2(x + 16.0, o.y + header_h / 2.0),
                            egui::Align2::LEFT_CENTER,
                            format!("{n}"),
                            cap.clone(),
                            if is_sel {
                                tc::TEXT_PRIMARY
                            } else {
                                tc::TEXT_SECONDARY
                            },
                        );
                        // value cells
                        for (ri, prop) in PlotProp::ALL.iter().enumerate() {
                            let y = o.y + header_h + ri as f32 * row_h;
                            let cr = egui::Rect::from_min_size(
                                egui::pos2(x, y),
                                egui::vec2(col_w, row_h),
                            );
                            if is_sel {
                                p.rect_filled(cr, 0.0, tc::SURFACE_2);
                            }
                            p.rect_stroke(cr, 0.0, egui::Stroke::new(0.3, tc::BORDER));
                            let (label, swrgb) =
                                plotprop_cell(*prop, self.doc.plot_styles.style(n), lt_list);
                            let mut tx = cr.left() + 4.0;
                            if let Some((r, g, b)) = swrgb {
                                let d = egui::Rect::from_min_size(
                                    egui::pos2(cr.left() + 3.0, cr.center().y - 5.0),
                                    egui::vec2(10.0, 10.0),
                                );
                                p.rect_filled(d, 2.0, egui::Color32::from_rgb(r, g, b));
                                tx = cr.left() + 16.0;
                            }
                            p.clone().with_clip_rect(cr).text(
                                egui::pos2(tx, cr.center().y),
                                egui::Align2::LEFT_CENTER,
                                label,
                                cap.clone(),
                                tc::TEXT_SECONDARY,
                            );
                        }
                    }
                    // click a header cell → select that column
                    if resp.clicked() {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            if pos.y <= o.y + header_h {
                                let ci = ((pos.x - o.x) / col_w).floor() as isize;
                                if ci >= 0 && (ci as usize) < ncol {
                                    let n = (ci as u8).wrapping_add(1);
                                    let m = ui.input(|i| i.modifiers);
                                    self.plotstyle_select(n, m.shift, m.command || m.ctrl, m.alt);
                                }
                            }
                        }
                    }
                    // Same keyboard shortcuts as the Form list when the grid is hovered.
                    if resp.hovered() {
                        self.plotstyle_list_keys(ui);
                    }
                });
        });
    }

    /// General tab — name (read-only), type, path, editable Description, and the
    /// global-linetype-scale toggle + percent.
    fn plotstyle_general_body(&mut self, ui: &mut egui::Ui) {
        use crate::theme::color as tc;
        egui::Grid::new("pse_general")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Table name");
                ui.label(
                    egui::RichText::new(self.doc.plot_styles.name.clone()).color(tc::TEXT_PRIMARY),
                );
                ui.end_row();
                ui.label("Table type");
                ui.label("Color-Dependent Plot Style Table");
                ui.end_row();
                ui.label("File");
                ui.label(
                    egui::RichText::new("— (not yet saved; .pst save/load)").color(tc::TEXT_MUTED),
                );
                ui.end_row();
                ui.label("Description");
                ui.add(
                    egui::TextEdit::multiline(&mut self.doc.plot_styles.description)
                        .desired_rows(3)
                        .desired_width(340.0),
                );
                ui.end_row();
                ui.label("Linetypes");
                ui.checkbox(
                    &mut self.doc.plot_styles.apply_global_ltscale,
                    "Apply global scale factor to non-ISO linetypes",
                );
                ui.end_row();
                ui.label("Scale factor");
                let mut pct = self.doc.plot_styles.ltscale_percent as f64;
                let enabled = self.doc.plot_styles.apply_global_ltscale;
                if ui
                    .add_enabled(
                        enabled,
                        egui::DragValue::new(&mut pct)
                            .range(1.0..=10000.0)
                            .suffix(" %")
                            .update_while_editing(false),
                    )
                    .changed()
                {
                    self.doc.plot_styles.ltscale_percent = pct as f32;
                }
                ui.end_row();
            });
    }

    /// Edit Lineweights sub-dialog — edits the table's `lineweight_ladder` (the
    /// mm set offered by every Lineweight dropdown). Values stored in mm; the
    /// mm/inch toggle only changes the display. Save writes the sorted/deduped
    /// set back to the table.
    pub(super) fn render_plot_ladder_dialog(&mut self, ctx: &egui::Context) {
        if !self.plotstyle_ladder_open {
            self.prev_plot_ladder_open = false;
            return;
        }
        let lad_just_opened = !self.prev_plot_ladder_open;
        self.prev_plot_ladder_open = true;
        if lad_just_opened {
            self.plot_ladder_open_seq = self.plot_ladder_open_seq.wrapping_add(1);
        }
        use crate::theme::color as tc;
        self.ensure_layer_glyph_textures(ctx);
        let gx_close = self
            .layer_glyph_tex
            .get("close")
            .map(|t| (t.id(), t.size_vec2()));
        let mut commit = false;
        let mut cancel = false;
        let lcr = self.canvas_screen_rect.unwrap_or_else(|| ctx.screen_rect());
        let lframe = egui::Frame::none()
            .fill(tc::SURFACE_1)
            .rounding(egui::Rounding::same(12.0))
            .stroke(egui::Stroke::new(1.0, tc::BORDER))
            .inner_margin(egui::Margin::ZERO);
        let lwin = egui::Window::new("Edit Lineweights")
            .order(egui::Order::Foreground)
            .title_bar(false)
            .frame(lframe)
            .default_size(egui::vec2(300.0, 440.0))
            .resizable(false)
            .movable(true);
        Self::place_child(
            lwin,
            "plot_ladder_dialog",
            self.plot_ladder_open_seq,
            self.plotstyle_editor_rect,
            lcr,
        )
        .show(ctx, |ui| {
            let larea = ui.max_rect();
            let lhf = egui::TextStyle::Button.resolve(ui.style());
            let (lhdr, _) =
                ui.allocate_exact_size(egui::vec2(larea.width(), 30.0), egui::Sense::hover());
            {
                let hp = ui.painter_at(lhdr);
                hp.rect_filled(
                    lhdr,
                    egui::Rounding {
                        nw: 12.0,
                        ne: 12.0,
                        sw: 0.0,
                        se: 0.0,
                    },
                    tc::BORDER,
                );
                hp.text(
                    egui::pos2(larea.left() + 14.0, lhdr.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Edit Lineweights",
                    lhf,
                    tc::TEXT_PRIMARY,
                );
            }
            let lxr = egui::Rect::from_center_size(
                egui::pos2(larea.right() - 13.0, lhdr.center().y),
                egui::vec2(13.5, 13.5),
            );
            let lxresp = ui.interact(lxr, egui::Id::new("ladder_close"), egui::Sense::click());
            {
                let hp = ui.painter_at(lhdr);
                let xcol = if lxresp.hovered() {
                    hp.rect_filled(
                        lxr.expand(3.0),
                        4.0,
                        egui::Color32::from_rgba_unmultiplied(0xE5, 0x48, 0x4D, 45),
                    );
                    LYR_DANGER
                } else {
                    LYR_MUTED
                };
                blit_layer_glyph(&hp, lxr, gx_close, xcol);
            }
            if lxresp.clicked() {
                cancel = true;
            } // × = close (discard)

            egui::Frame::none()
                .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Units:");
                        ui.selectable_value(&mut self.plotstyle_ladder_inch, false, "mm");
                        ui.selectable_value(&mut self.plotstyle_ladder_inch, true, "inch");
                    });
                    ui.separator();
                    let inch = self.plotstyle_ladder_inch;
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut del: Option<usize> = None;
                            for i in 0..self.plotstyle_ladder_work.len() {
                                ui.horizontal(|ui| {
                                    let is_sel = self.plotstyle_ladder_sel == Some(i);
                                    if ui
                                        .selectable_label(is_sel, format!("{:>2}", i + 1))
                                        .clicked()
                                    {
                                        self.plotstyle_ladder_sel = Some(i);
                                    }
                                    let (suffix, speed) =
                                        if inch { (" in", 0.001) } else { (" mm", 0.01) };
                                    let mut disp = if inch {
                                        (self.plotstyle_ladder_work[i] / 25.4) as f64
                                    } else {
                                        self.plotstyle_ladder_work[i] as f64
                                    };
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut disp)
                                                .speed(speed)
                                                .range(0.0..=100.0)
                                                .suffix(suffix)
                                                .update_while_editing(false),
                                        )
                                        .changed()
                                    {
                                        self.plotstyle_ladder_work[i] = if inch {
                                            (disp * 25.4) as f32
                                        } else {
                                            disp as f32
                                        };
                                    }
                                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                                        del = Some(i);
                                    }
                                });
                            }
                            if let Some(i) = del {
                                if self.plotstyle_ladder_work.len() > 1 {
                                    self.plotstyle_ladder_work.remove(i);
                                    self.plotstyle_ladder_sel = None;
                                }
                            }
                        });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Add").clicked() {
                            self.plotstyle_ladder_work.push(0.0);
                            self.plotstyle_ladder_sel = Some(self.plotstyle_ladder_work.len() - 1);
                        }
                        if ui.button("Sort").clicked() {
                            self.plotstyle_ladder_work.sort_by(|a, b| {
                                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                            });
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let save = egui::Button::new(
                                egui::RichText::new("Save").color(tc::ON_ACCENT).strong(),
                            )
                            .fill(tc::ACCENT)
                            .rounding(egui::Rounding::same(6.0));
                            if ui.add(save).clicked() {
                                commit = true;
                            }
                        });
                    });
                });
        });
        if commit {
            let mut v = self.plotstyle_ladder_work.clone();
            v.retain(|w| *w >= 0.0);
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
            if v.is_empty() {
                v.push(0.0);
            }
            self.doc.plot_styles.lineweight_ladder = v;
            self.plotstyle_ladder_open = false;
        }
        if cancel {
            self.plotstyle_ladder_open = false;
        }
        // when the ladder closes, reopen the editor it replaced.
        if !self.plotstyle_ladder_open && self.plotstyle_return_after_sub {
            self.plotstyle_open = true;
            self.plotstyle_return_after_sub = false;
        }
    }

    /// A sensible default plot output path: the drawing's name with the current
    /// format's extension, else `plot.<ext>` in the last-used directory.
    pub(super) fn default_plot_pdf_path(&self) -> String {
        let ext = match self.plot_format.as_str() {
            "svg" => "svg",
            "png" => "png",
            _ => "pdf",
        };
        if let Some(p) = &self.current_file {
            let mut pb = p.clone();
            pb.set_extension(ext);
            return pb.to_string_lossy().to_string();
        }
        let dir = self
            .file_dialog_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        dir.join(format!("plot.{}", ext))
            .to_string_lossy()
            .to_string()
    }

    // ---- Plot dialog + pipeline ------------------------------------------

    /// Fit readout: model units per paper-mm, computed live from the current
    /// window/extents and paper. `None` when there is nothing to plot.
    fn plot_fit_units_per_mm(&self) -> Option<f64> {
        let (mn, mx) = match self.plot_area_kind {
            1 => self.plot_window?,
            _ => self.doc_extents()?,
        };
        let (bw, bh) = ((mx.x - mn.x).max(1e-9), (mx.y - mn.y).max(1e-9));
        let (pw, ph) = self.plot_paper.dims_mm();
        let (pw, ph) = if self.plot_landscape {
            (ph as f64, pw as f64)
        } else {
            (pw as f64, ph as f64)
        };
        let m = 5.0;
        let (prw, prh) = ((pw - 2.0 * m).max(1.0), (ph - 2.0 * m).max(1.0));
        // mm-on-paper per model-unit, then invert to model-units per paper-mm.
        let s = (prw / bw).min(prh / bh);
        if s <= 1e-12 {
            None
        } else {
            Some(1.0 / s)
        }
    }

    /// Paint the paper preview. `full = false` (the dialog's small pane) draws the
    /// sheet + a single **red footprint rectangle** = the plotted area's
    /// proportion + position on the paper (nothing inside). `full = true` (the
    /// Preview window) renders the **real geometry** — pens, linetypes, window
    /// clip — exactly as it will print.
    fn plot_preview_paint(
        &self,
        p: &egui::Painter,
        box_rect: egui::Rect,
        cap: egui::FontId,
        full: bool,
    ) {
        use crate::theme::color as tc;
        let (pw, ph) = self.plot_paper.dims_mm();
        let (pw, ph) = if self.plot_landscape {
            (ph, pw)
        } else {
            (pw, ph)
        };
        // Asymmetric margins: extra on the left for the height label, extra on
        // the bottom for the width label — so neither is clipped by the box.
        let (lm, rm, tm, bm) = (30.0_f32, 14.0_f32, 14.0_f32, 24.0_f32);
        let avail = egui::vec2(
            (box_rect.width() - lm - rm).max(10.0),
            (box_rect.height() - tm - bm).max(10.0),
        );
        let s = (avail.x / pw).min(avail.y / ph);
        let sheet = egui::vec2(pw * s, ph * s);
        let region =
            egui::Rect::from_min_size(egui::pos2(box_rect.left() + lm, box_rect.top() + tm), avail);
        let sheet_rect = egui::Rect::from_center_size(region.center(), sheet);
        // sheet
        p.rect_filled(sheet_rect, 2.0, egui::Color32::from_rgb(0xE9, 0xE7, 0xE2));
        p.rect_stroke(sheet_rect, 2.0, egui::Stroke::new(1.0, tc::BORDER));
        // printable margin (5 mm inset)
        let inner = sheet_rect.shrink(5.0 * s);
        p.rect_stroke(
            inner,
            0.0,
            egui::Stroke::new(0.8, egui::Color32::from_rgb(0x9A, 0xA4, 0xAD)),
        );
        if full {
            // FULL render: build the REAL plot scene (same pipeline as run_plot)
            // and draw it into the sheet — pens, linetype dashes, window clip. `s`
            // is preview px per paper-mm; the page origin is bottom-left.
            let default_table;
            let table: &cad_kernel::plotstyle::PlotStyleTable = if self.plot_with_styles {
                &self.doc.plot_styles
            } else {
                default_table = cad_kernel::plotstyle::PlotStyleTable::default();
                &default_table
            };
            let cfg = self.plot_config(cad_kernel::plotstyle::PlotTarget::PdfFile(
                std::path::PathBuf::new(),
            ));
            let scene = cad_plot::build_scene(&self.doc, table, &cfg);
            let map = |x: f64, y: f64| {
                egui::pos2(
                    sheet_rect.left() + (x as f32) * s,
                    sheet_rect.bottom() - (y as f32) * s,
                )
            };
            for prim in &scene.prims {
                match prim {
                    cad_plot::Prim::Stroke {
                        pts,
                        closed,
                        width_mm,
                        rgb,
                        dash_mm,
                        dash_offset_mm,
                        cap,
                        join,
                        smooth,
                        ..
                    } => {
                        let col = egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2);
                        let w = (*width_mm * s).max(0.6); // floor so it's visible
                        let mut pl: Vec<egui::Pos2> = pts.iter().map(|&(x, y)| map(x, y)).collect();
                        if *closed {
                            if let Some(&f) = pl.first() {
                                pl.push(f);
                            }
                        }
                        // Cap/join emulation: Round/Miter keep the single
                        // strip render (one shape per stroke); Bevel draws butt
                        // segments so the bevel wedges are the only corner
                        // geometry. `smooth` tessellations skip join wedges.
                        let corners = !*smooth;
                        let bevel = *join == cad_kernel::plotstyle::JoinStyle::Bevel;
                        let draw_base = |pl: &[egui::Pos2]| {
                            if bevel {
                                paint_butt_polyline(p, pl, w, col);
                            } else {
                                p.add(egui::Shape::line(pl.to_vec(), egui::Stroke::new(w, col)));
                            }
                        };
                        if dash_mm.is_empty() {
                            draw_base(&pl);
                            paint_caps_joins_screen(p, &pl, *closed, w, *cap, *join, col, corners);
                        } else {
                            // Walk the dash pattern continuously along the
                            // polyline, starting at the scene's dash-phase
                            // offset — the preview must match the exported
                            // PDF/SVG/PNG (which all honour the adaptive
                            // linetype phase). Each dash run is an open stroke:
                            // the pen's cap applies at its ends and the join
                            // style at any corner inside it.
                            let total: f64 = dash_mm.iter().map(|&v| v.abs() as f64).sum();
                            let mut phase = (*dash_offset_mm as f64 * s as f64).abs();
                            phase = if total > 1e-9 { phase % total } else { 0.0 };
                            let mut pi = 0usize;
                            let mut remaining = dash_mm[0].abs() as f64;
                            while phase > 1e-9 {
                                let seg = dash_mm[pi].abs() as f64;
                                if phase >= seg - 1e-9 {
                                    phase -= seg;
                                    pi = (pi + 1) % dash_mm.len();
                                    remaining = dash_mm[pi].abs() as f64;
                                } else {
                                    remaining = seg - phase;
                                    phase = 0.0;
                                }
                            }
                            let mut on = pi % 2 == 0;
                            let mut cur: Vec<egui::Pos2> = Vec::new();
                            let flush_run = |cur: &mut Vec<egui::Pos2>| {
                                if cur.len() >= 2 {
                                    draw_base(cur);
                                    paint_caps_joins_screen(
                                        p, cur, false, w, *cap, *join, col, corners,
                                    );
                                }
                                cur.clear();
                            };
                            for seg in pl.windows(2) {
                                let (a, b) = (seg[0], seg[1]);
                                let len = (b - a).length().max(1e-6);
                                let mut walked = 0.0_f32;
                                while walked < len - 1e-6 {
                                    let take = remaining.min((len - walked) as f64);
                                    let t0 = walked / len;
                                    let t1 = (walked + take as f32) / len;
                                    let p0 = a + (b - a) * t0;
                                    let p1 = a + (b - a) * t1;
                                    if on {
                                        cur.push(p0);
                                        cur.push(p1);
                                    }
                                    walked += take as f32;
                                    remaining -= take;
                                    if remaining <= 1e-9 {
                                        if on {
                                            flush_run(&mut cur);
                                        } else {
                                            cur.clear();
                                        }
                                        on = !on;
                                        pi = (pi + 1) % dash_mm.len();
                                        remaining = dash_mm[pi].abs() as f64;
                                    }
                                }
                            }
                            if on {
                                flush_run(&mut cur);
                            }
                        }
                    }
                    cad_plot::Prim::Fill { loops, rgb, .. } => {
                        let col = egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2);
                        if let Some(outer) = loops.first() {
                            let poly: Vec<egui::Pos2> =
                                outer.iter().map(|&(x, y)| map(x, y)).collect();
                            p.add(egui::Shape::convex_polygon(poly, col, egui::Stroke::NONE));
                        }
                    }
                    cad_plot::Prim::Tris { tris, rgb } => {
                        let col = egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2);
                        for t in tris {
                            let poly: Vec<egui::Pos2> = t.iter().map(|&(x, y)| map(x, y)).collect();
                            p.add(egui::Shape::convex_polygon(poly, col, egui::Stroke::NONE));
                        }
                    }
                }
            }
        } else {
            // SMALL pane: just the plotted-area footprint = its proportion +
            // position on the paper (placed like the real plot; nothing inside).
            let m = 5.0_f32;
            let prw = (pw - 2.0 * m).max(1.0);
            let prh = (ph - 2.0 * m).max(1.0);
            let plotted = if self.plot_area_kind == 1 {
                self.plot_window
            } else {
                self.doc_extents()
            };
            if let Some((mn, mx)) = plotted {
                let (bw, bh) = ((mx.x - mn.x).max(1e-9), (mx.y - mn.y).max(1e-9));
                let unit_mm = if self.plot_unit_inch { 25.4_f64 } else { 1.0 };
                let ps = if self.plot_scale_fit {
                    (prw as f64 / bw).min(prh as f64 / bh)
                } else {
                    unit_mm / (self.plot_scale_n.max(1e-6))
                };
                let cw = (bw * ps) as f32;
                let ch = (bh * ps) as f32;
                let (ox, oy) = if self.plot_offset_center {
                    (m + (prw - cw) * 0.5, m + (prh - ch) * 0.5)
                } else {
                    (
                        m + (self.plot_offset_x as f64 * unit_mm) as f32,
                        m + (self.plot_offset_y as f64 * unit_mm) as f32,
                    )
                };
                let fpr = egui::Rect::from_min_size(
                    egui::pos2(
                        sheet_rect.left() + ox * s,
                        sheet_rect.bottom() - (oy + ch) * s,
                    ),
                    egui::vec2(cw * s, ch * s),
                );
                p.rect_stroke(fpr, 0.0, egui::Stroke::new(1.5, tc::DANGER));
            }
        }
        // dimension labels: width under the sheet (horizontal), height in the
        // left margin ROTATED 90° (reads bottom-to-top) with a mm suffix.
        p.text(
            egui::pos2(sheet_rect.center().x, sheet_rect.bottom() + 8.0),
            egui::Align2::CENTER_CENTER,
            format!("{:.0} mm", pw),
            cap.clone(),
            tc::TEXT_SECONDARY,
        );
        let galley = p.layout_no_wrap(format!("{:.0} mm", ph), cap.clone(), tc::TEXT_SECONDARY);
        let (gw, gh) = (galley.size().x, galley.size().y);
        let gx = sheet_rect.left() - 10.0; // just left of the sheet
        let cy = sheet_rect.center().y;
        let mut ts = egui::epaint::TextShape::new(
            egui::pos2(gx - gh * 0.5, cy + gw * 0.5),
            galley,
            tc::TEXT_SECONDARY,
        );
        ts.angle = -std::f32::consts::FRAC_PI_2; // −90° → reads upward
        p.add(egui::Shape::Text(ts));
    }

    /// The `PlotConfig` for the current dialog state — shared by the live preview
    /// and `run_plot` so they can never diverge. `output` is the only per-caller
    /// field (the preview passes a dummy; run_plot the real path). Honours the
    /// mm/inch unit on the ratio's paper side and the X/Y offset.
    pub(super) fn plot_config(
        &self,
        output: cad_kernel::plotstyle::PlotTarget,
    ) -> cad_kernel::plotstyle::PlotConfig {
        use cad_kernel::plotstyle::{Offset, Orientation, PlotArea, PlotConfig, PlotScale};
        let area = match self.plot_area_kind {
            1 => match self.plot_window {
                Some((mn, mx)) => PlotArea::Window { min: mn, max: mx },
                None => PlotArea::Extents,
            },
            _ => PlotArea::Extents, // Display has no view rect here → Extents.
        };
        let unit_mm = if self.plot_unit_inch { 25.4_f64 } else { 1.0 };
        let scale = if self.plot_scale_fit {
            PlotScale::Fit
        } else {
            PlotScale::Ratio {
                model: self.plot_scale_n.max(1e-6),
                paper_mm: unit_mm,
            }
        };
        let offset = if self.plot_offset_center {
            Offset::Center
        } else {
            Offset::Xy {
                x_mm: (self.plot_offset_x as f64 * unit_mm) as f32,
                y_mm: (self.plot_offset_y as f64 * unit_mm) as f32,
            }
        };
        // "Scale lineweights" OFF = physical mm (the ruling). ON = scale with the
        // drawing (Ratio only). "Plot object lineweights" OFF forces hairline.
        let mut lw_scale = if self.plot_scale_lw {
            match scale {
                PlotScale::Ratio { model, paper_mm } => (paper_mm / model) as f32,
                PlotScale::Fit => 1.0,
            }
        } else {
            1.0
        };
        if !self.plot_object_lw {
            lw_scale = 0.0;
        }
        PlotConfig {
            output,
            paper: self.plot_paper,
            orientation: if self.plot_landscape {
                Orientation::Landscape
            } else {
                Orientation::Portrait
            },
            area,
            scale,
            offset,
            lw_scale,
            // "Plot all black" — effective for MODEL plots only.
            monochrome: self.plot_mono,
            margins_mm: 5.0,
            // Layout tab → plot the ACTIVE LAYOUT (1:1 paper + viewports);
            // model tab → None = the model-space plot.
            plot_layout_index: self.doc.active_layout,
            // The saved CTBs, so a layout's named CTB applies its per-ACI
            // colour rules in the plot output.
            ctb_tables: {
                let mut tables = std::collections::BTreeMap::new();
                for (name, path) in ctb_list() {
                    if let Ok(t) = cad_io::load_plot_table(&path) {
                        tables.insert(name, t);
                    }
                }
                // The Plot Style Table Editor's LIVE table overrides the disk
                // copy while open — so Plot preview + the exported files show
                // CTB edits (widths, joins, caps, …) before "Save changes".
                if self.plotstyle_open {
                    tables.insert(
                        self.doc.plot_styles.name.clone(),
                        self.doc.plot_styles.clone(),
                    );
                }
                tables
            },
        }
    }

    /// Open a written file with the OS default handler (the PDF viewer).
    fn open_in_viewer(path: &std::path::Path) {
        let p = path.to_string_lossy().to_string();
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer").arg(&p).spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open").arg(&p).spawn();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = std::process::Command::new("xdg-open").arg(&p).spawn();
        }
    }

    /// Build a `PlotConfig` from the Plot dialog and run `cad_plot::plot`.
    /// Read-only on the doc; reports the written PDF path in history, then opens
    /// the PDF in the default viewer.
    pub(super) fn run_plot(&mut self) {
        use cad_kernel::plotstyle::{PlotStyleTable, PlotTarget};
        let raw = self.plot_pdf_path.trim().to_string();
        if raw.is_empty() {
            self.history
                .push("  ! plot: no output path — press Plot and choose a file.".into());
            return;
        }
        // Output format: PDF default, SVG / PNG optional.
        let fmt = self.plot_format.clone();
        let ext = match fmt.as_str() {
            "svg" => "svg",
            "png" => "png",
            _ => "pdf",
        };
        let mut path = std::path::PathBuf::from(&raw);
        if path
            .extension()
            .map(|e| e.to_ascii_lowercase() != ext)
            .unwrap_or(true)
        {
            path.set_extension(ext);
        }
        let cfg = self.plot_config(PlotTarget::PdfFile(path.clone()));
        // Window-area only applies to MODEL plots — a layout plot is 1:1 paper
        // (the layout's own paper + viewports), so never demand a picked window.
        if cfg.plot_layout_index.is_none() && self.plot_area_kind == 1 && self.plot_window.is_none()
        {
            self.history
                .push("  ! plot: pick the Window area first".into());
            return;
        }
        // Window-area only applies to MODEL plots — never demand a picked window
        // for Extents.
        if self.plot_area_kind == 1 && self.plot_window.is_none() {
            self.history
                .push("  ! plot: pick the Window area first".into());
            return;
        }

        // Table: with plot styles → the doc's table; without → a default table so
        // object lineweights drive.
        let default_table;
        let table: &PlotStyleTable = if self.plot_with_styles {
            &self.doc.plot_styles
        } else {
            default_table = PlotStyleTable::default();
            &default_table
        };

        // A locked/read-only target (the file open in a viewer) is the common
        // failure — give a short, actionable line instead of the raw OS error.
        let locked_msg = |e: &cad_plot::PlotError| {
            if let cad_plot::PlotError::Io(io) = e {
                let raw = io.raw_os_error();
                io.kind() == std::io::ErrorKind::PermissionDenied
                    || raw == Some(32)
                    || raw == Some(5)
            } else {
                false
            }
        };
        let result: Result<(), String> = match fmt.as_str() {
            "svg" => match cad_plot::export_svg_meta(&self.doc, table, &cfg, &path) {
                Ok(out) => {
                    let dims = if out.skipped_dims > 0 {
                        format!(", {} dimension(s) skipped", out.skipped_dims)
                    } else {
                        String::new()
                    };
                    self.history.push(format!(
                        "  plot → {} ({} KB, SVG, {} prims{})",
                        path.display(),
                        (out.bytes + 512) / 1024,
                        out.prim_count,
                        dims
                    ));
                    if !self.plot_suppress_viewer {
                        Self::open_in_viewer(&path);
                    }
                    Ok(())
                }
                Err(e) => Err(if locked_msg(&e) {
                    "  ! plot: the SVG is open in another program — close it, or Browse… to a new file name.".into()
                } else {
                    format!("  ! SVG plot failed: {}", e)
                }),
            },
            "png" => match cad_plot::export_png_meta(&self.doc, table, &cfg, &path, 300.0) {
                Ok(out) => {
                    let dims = if out.skipped_dims > 0 {
                        format!(", {} dimension(s) skipped", out.skipped_dims)
                    } else {
                        String::new()
                    };
                    self.history.push(format!(
                        "  plot → {} ({} KB, PNG @300dpi, {} prims{})",
                        path.display(),
                        (out.bytes + 512) / 1024,
                        out.prim_count,
                        dims
                    ));
                    if !self.plot_suppress_viewer {
                        Self::open_in_viewer(&path);
                    }
                    Ok(())
                }
                Err(e) => Err(if locked_msg(&e) {
                    "  ! plot: the PNG is open in another program — close it, or Browse… to a new file name.".into()
                } else {
                    format!("  ! PNG plot failed: {}", e)
                }),
            },
            _ => {
                match cad_plot::plot(&self.doc, table, &cfg) {
                    Ok(out) => {
                        let dims = if out.skipped_dims > 0 {
                            format!(", {} dimension(s) skipped", out.skipped_dims)
                        } else {
                            String::new()
                        };
                        self.history.push(format!(
                            "  plot → {} ({} KB, {} prims{})",
                            out.path.display(),
                            (out.bytes + 512) / 1024,
                            out.prim_count,
                            dims
                        ));
                        // Open the finished PDF in the default viewer.
                        if !self.plot_suppress_viewer {
                            Self::open_in_viewer(&out.path);
                        }
                        Ok(())
                    }
                    Err(e) => Err(if locked_msg(&e) {
                        "  ! plot: the PDF is open in another program — close it, or Browse… to a new file name.".into()
                    } else {
                        format!("  ! plot failed: {}", e)
                    }),
                }
            }
        };
        if let Err(msg) = result {
            self.history.push(msg);
        }
        self.plot_dialog_open = false;
        self.clear_prompt();
    }

    /// The Plot dialog (model space) — standalone floating window with
    /// DIALOG_STANDARD chrome. Builds a `PlotConfig` and calls `cad_plot::plot`.
    /// Read-only on the doc. Two dialogs coexist: this one and the Plot Style
    /// Table Editor (which replaces it while open).

    /// LAYOUT-tab plot dialog: the layout plots 1:1 at its OWN paper size —
    /// paper border, paper-space entities and every viewport's model content
    /// (per-viewport camera + CTB) — via `PlotConfig::plot_layout_index`.
    /// Read-only on the doc; the PDF flow (Browse → run_plot) is shared with
    /// the model dialog.
    fn render_layout_plot_dialog(&mut self, ctx: &egui::Context) {
        use crate::theme::color as tc;
        let Some(li) = self.doc.active_layout else {
            self.plot_dialog_open = false;
            return;
        };
        let Some(layout) = self.doc.layouts.get(li).cloned() else {
            self.plot_dialog_open = false;
            return;
        };
        let mut close = false;
        let mut do_plot = false;
        let frame = egui::Frame::none()
            .fill(tc::SURFACE_1)
            .rounding(egui::Rounding::same(12.0))
            .stroke(egui::Stroke::new(1.0, tc::BORDER))
            .inner_margin(egui::Margin::same(16.0));
        let win = egui::Window::new(format!("Plot — {}", layout.name))
            .order(egui::Order::Foreground)
            .id(egui::Id::new("plot_dialog"))
            .title_bar(false)
            .frame(frame)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .collapsible(false)
            .resizable(false);
        win.show(ctx, |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 24.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(
                        egui::RichText::new(format!("Plot — {}", layout.name))
                            .strong()
                            .size(16.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("\u{2715}").clicked() {
                            close = true;
                        }
                    });
                },
            );
            ui.separator();
            ui.add_space(8.0);
            let (pw, ph) = (layout.page_w_mm, layout.page_h_mm);
            ui.label(format!(
                "Paper:  {:.0} \u{00D7} {:.0} mm   ({})",
                pw,
                ph,
                if pw >= ph { "landscape" } else { "portrait" }
            ));
            ui.label(format!("Viewports:  {}", layout.viewports.len()));
            ui.horizontal(|ui| {
                ui.label("CTB:");
                let (ctbs, mut ci) =
                    self.ctb_picker_state(layout.ctb_name.as_deref().unwrap_or(""));
                egui::ComboBox::from_id_salt("lplot_ctb")
                    .selected_text(ctbs[ci].0.clone())
                    .show_ui(ui, |ui| {
                        for (i, (disp, _)) in ctbs.iter().enumerate() {
                            ui.selectable_value(&mut ci, i, disp);
                        }
                    });
                let chosen = ctbs[ci].1.clone();
                if chosen != layout.ctb_name {
                    self.snapshot_doc();
                    if let Some(l) = self.doc.layouts.get_mut(li) {
                        l.ctb_name = chosen;
                    }
                    ui.ctx().request_repaint();
                }
            });
            ui.small(
                "Plots the paper sheet at 1:1 — border, paper-space \
                      entities and every viewport's model content.",
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Format:");
                let fmts = ["PDF", "SVG", "PNG"];
                let mut fi = match self.plot_format.as_str() {
                    "svg" => 1,
                    "png" => 2,
                    _ => 0,
                };
                egui::ComboBox::from_id_salt("plot_fmt2")
                    .selected_text(fmts[fi])
                    .show_ui(ui, |ui| {
                        for (i, n) in fmts.iter().enumerate() {
                            ui.selectable_value(&mut fi, i, *n);
                        }
                    });
                self.plot_format = match fi {
                    1 => "svg".into(),
                    2 => "png".into(),
                    _ => "pdf".into(),
                };
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui
                    .add_sized(
                        egui::vec2(80.0, 32.0),
                        egui::Button::new(
                            egui::RichText::new("Plot").color(tc::ON_ACCENT).strong(),
                        )
                        .fill(tc::ACCENT)
                        .rounding(egui::Rounding::same(6.0)),
                    )
                    .clicked()
                {
                    do_plot = true;
                }
            });
        });
        if close {
            self.plot_dialog_open = false;
        }
        if do_plot {
            self.plot_dialog_open = false;
            self.plot_pdf_browse = true;
            self.plot_run_after_save = true;
            let ext = match self.plot_format.as_str() {
                "svg" => "svg",
                "png" => "png",
                _ => "pdf",
            };
            self.open_file_dialog(FileDialogMode::Save, &format!(".{}", ext));
        }
    }

    pub(super) fn render_plot_dialog(&mut self, ctx: &egui::Context) {
        if !self.plot_dialog_open {
            return;
        }
        // Parked while picking the Window area on the canvas.
        if self.plot_win_pick != 0 {
            return;
        }
        // Layout tab → the layout plot dialog (1:1 paper + viewports).
        if self.doc.active_layout.is_some() {
            self.render_layout_plot_dialog(ctx);
            return;
        }
        self.ensure_layer_glyph_textures(ctx);
        let gx_close = self
            .layer_glyph_tex
            .get("close")
            .map(|t| (t.id(), t.size_vec2()));
        use crate::theme::color as tc;
        use cad_kernel::plotstyle::PaperSize;

        let table_name = self.doc.plot_styles.name.clone();
        let mut close_panel = false;
        let mut do_ok = false;
        let mut open_editor = false;
        let mut pick_window = false;
        let mut load_pst = false;
        let mut open_preview = false;
        let mut edit_anchor: Option<egui::Rect> = None;
        // Fit readout: model units per paper-mm (1 mm = N units), computed live.
        let fit_units = self.plot_fit_units_per_mm();
        // Paper-side unit (mm default / inch) for the scale readout + offset.
        let unit_mm = if self.plot_unit_inch { 25.4_f64 } else { 1.0 };
        let unit_name = if self.plot_unit_inch { "inch" } else { "mm" };

        let frame = egui::Frame::none()
            .fill(tc::SURFACE_1)
            .rounding(egui::Rounding::same(12.0))
            .stroke(egui::Stroke::new(1.0, tc::BORDER))
            .inner_margin(egui::Margin::ZERO);
        // The main Plot dialog opens at the canvas centre, fully INSIDE the
        // canvas — never over the category bar / rails. If the box is taller than
        // the canvas, its height is CAPPED to the canvas and the body SCROLLS.
        let cr = self.canvas_screen_rect.unwrap_or_else(|| ctx.screen_rect());
        let vmargin = 10.0;
        let max_h = (cr.height() - 2.0 * vmargin).max(220.0);
        // Clamp the centre using the CAPPED height so the whole box fits in-canvas.
        let box_h = self
            .plot_dialog_rect
            .map(|r| r.height())
            .unwrap_or(560.0)
            .min(max_h);
        let center = Self::clamp_center_in(cr, cr.center(), egui::vec2(724.0, box_h));
        let win = egui::Window::new("Plot")
            .order(egui::Order::Foreground)
            .id(egui::Id::new("plot_dialog"))
            .title_bar(false)
            .frame(frame)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(center)
            .min_width(724.0)
            .max_width(724.0)
            .max_height(max_h)
            .vscroll(false)
            .resizable(false)
            .movable(true);
        let resp = win.show(ctx, |ui| {
            let area = ui.max_rect();
            let full_w = area.width();
            let hdr_font = egui::TextStyle::Button.resolve(ui.style());
            let cap = crate::theme::typ::caption();

            // ===== header band (#34414B, "Plot" + Close ×) =====
            let (hdr, _) = ui.allocate_exact_size(egui::vec2(full_w, 30.0), egui::Sense::hover());
            {
                let hp = ui.painter_at(hdr);
                hp.rect_filled(hdr, egui::Rounding { nw: 12.0, ne: 12.0, sw: 0.0, se: 0.0 }, tc::BORDER);
                hp.text(egui::pos2(area.left() + 14.0, hdr.center().y), egui::Align2::LEFT_CENTER,
                    "Plot", hdr_font, tc::TEXT_PRIMARY);
            }
            let xr = egui::Rect::from_center_size(egui::pos2(area.right() - 13.0, hdr.center().y), egui::vec2(13.5, 13.5));
            let xresp = ui.interact(xr, egui::Id::new("plot_close"), egui::Sense::click());
            {
                let hp = ui.painter_at(hdr);
                let xcol = if xresp.hovered() {
                    hp.rect_filled(xr.expand(3.0), 4.0, egui::Color32::from_rgba_unmultiplied(0xE5, 0x48, 0x4D, 45));
                    LYR_DANGER
                } else { LYR_MUTED };
                blit_layer_glyph(&hp, xr, gx_close, xcol);
            }
            if xresp.clicked() { close_panel = true; }

            // ---- SCROLLABLE BODY: the custom header (band + Close ×) and the
            //      footer stay PINNED — only the middle content scrolls. ----
            egui::ScrollArea::vertical()
                .id_salt("plot_dialog_body")
                .auto_shrink([false; 2])
                .show(ui, |ui| {

            // ---- reusable painters for a consistent, aligned look ----
            let body_font = crate::theme::typ::body();
            let sh = |ui: &mut egui::Ui, t: &str, first: bool| {
                if first {
                    ui.add_space(2.0);
                } else {
                    ui.add_space(11.0);
                    let xr = ui.max_rect().x_range();
                    let y = ui.cursor().top();
                    ui.painter().hline(xr, y, egui::Stroke::new(1.0, tc::BORDER));
                    ui.add_space(10.0);
                }
                ui.label(egui::RichText::new(t).color(tc::COLUMN_HEADER).size(11.0).strong());
                ui.add_space(8.0);
            };
            // Same primitive the Inspector uses (`pp_box`): 24px tall, radius 4,
            // surface-0 fill, 1px border, body() 13px text, 8px pad, pp_arrow.
            let field_box = |ui: &mut egui::Ui, w: f32, text: &str, chevron: bool, enabled: bool| -> egui::Response {
                let (r, resp) = pp_box(ui, w, enabled);
                ui.painter().text(egui::pos2(r.left() + crate::theme::space::INPUT_PAD, r.center().y),
                    egui::Align2::LEFT_CENTER, text, body_font.clone(),
                    if enabled { PP_TEXT } else { tc::TEXT_DISABLED });
                if chevron { pp_arrow(&ui.painter_at(r), r); }
                resp
            };
            let lbl = |ui: &mut egui::Ui, t: &str, en: bool| {
                ui.add_sized([70.0, PP_ROW_H], egui::Label::new(
                    egui::RichText::new(t).color(if en { tc::TEXT_MUTED } else { tc::TEXT_DISABLED })));
            };
            let check = |ui: &mut egui::Ui, w: f32, label: &str, checked: bool, en: bool| -> bool {
                let (r, resp) = ui.allocate_exact_size(egui::vec2(w, 24.0), egui::Sense::click());
                let bx = egui::Rect::from_min_size(egui::pos2(r.left(), r.center().y - 8.0), egui::vec2(16.0, 16.0));
                if checked {
                    ui.painter().rect_filled(bx, 4.0, if en { tc::ACCENT } else { tc::SURFACE_3 });
                    let c = bx.center();
                    ui.painter().add(egui::Shape::line(
                        vec![egui::pos2(c.x - 3.5, c.y + 0.5), egui::pos2(c.x - 1.0, c.y + 3.0), egui::pos2(c.x + 4.0, c.y - 3.5)],
                        egui::Stroke::new(1.9, if en { tc::ON_ACCENT } else { tc::TEXT_DISABLED })));
                } else {
                    ui.painter().rect_stroke(bx, 4.0, egui::Stroke::new(1.3, if en { tc::TEXT_MUTED } else { tc::TEXT_DISABLED }));
                }
                ui.painter().text(egui::pos2(bx.right() + 8.0, r.center().y), egui::Align2::LEFT_CENTER,
                    label, body_font.clone(), if en { tc::TEXT_PRIMARY } else { tc::TEXT_DISABLED });
                resp.clicked() && en
            };
            let icon_btn = |ui: &mut egui::Ui, kind: &str, en: bool| -> egui::Response {
                let (r, resp) = ui.allocate_exact_size(egui::vec2(PP_ROW_H, PP_ROW_H), egui::Sense::click());
                let brd = if en && resp.hovered() { tc::ACCENT } else { tc::BORDER };
                ui.painter().rect(r, egui::Rounding::same(crate::theme::radius::SM), tc::SURFACE_1, egui::Stroke::new(1.0, brd));
                let col = if en { tc::TEXT_MUTED } else { tc::TEXT_DISABLED };
                let c = r.center();
                let p = ui.painter();
                match kind {
                    "sliders" => {
                        p.line_segment([egui::pos2(c.x - 6.0, c.y - 4.0), egui::pos2(c.x + 6.0, c.y - 4.0)], egui::Stroke::new(1.4, col));
                        p.circle_filled(egui::pos2(c.x + 2.5, c.y - 4.0), 2.2, col);
                        p.line_segment([egui::pos2(c.x - 6.0, c.y + 4.0), egui::pos2(c.x + 6.0, c.y + 4.0)], egui::Stroke::new(1.4, col));
                        p.circle_filled(egui::pos2(c.x - 2.5, c.y + 4.0), 2.2, col);
                    }
                    "save" => {
                        p.rect_stroke(egui::Rect::from_center_size(c, egui::vec2(12.0, 12.0)), 1.0, egui::Stroke::new(1.3, col));
                        p.rect_filled(egui::Rect::from_min_size(egui::pos2(c.x - 3.0, c.y - 6.0), egui::vec2(6.0, 3.5)), 0.0, col);
                    }
                    "pencil" => {
                        p.line_segment([egui::pos2(c.x - 4.5, c.y + 4.5), egui::pos2(c.x + 3.5, c.y - 3.5)], egui::Stroke::new(1.7, col));
                        p.line_segment([egui::pos2(c.x - 5.5, c.y + 5.5), egui::pos2(c.x - 3.5, c.y + 3.5)], egui::Stroke::new(1.7, col));
                    }
                    _ => {}
                }
                resp
            };
            // Numeric field identical to the Inspector's (`pp_num_field`).
            let num_box = |ui: &mut egui::Ui, w: f32, val: &mut f64, en: bool| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(w, PP_ROW_H), egui::Sense::hover());
                if en {
                    pp_num_field(ui, r, val);
                } else {
                    ui.painter().rect(r, egui::Rounding::same(crate::theme::radius::SM),
                        PP_BG_LO, egui::Stroke::new(1.0, PP_BORDER));
                    ui.painter().text(egui::pos2(r.left() + crate::theme::space::INPUT_PAD, r.center().y),
                        egui::Align2::LEFT_CENTER, format!("{:.2}", val),
                        crate::theme::typ::data_value(), tc::TEXT_DISABLED);
                }
            };

            ui.horizontal_top(|ui| {
                // ===================== LEFT column =====================
                egui::Frame::none().inner_margin(egui::Margin { left: 18.0, right: 16.0, top: 8.0, bottom: 8.0 }).show(ui, |ui| {
                  ui.vertical(|ui| {
                    ui.set_width(392.0);
                    ui.spacing_mut().item_spacing.y = 8.0;

                    // ---- DESTINATION ----
                    sh(ui, "DESTINATION", true);
                    ui.horizontal(|ui| {
                        lbl(ui, "Printer", true);
                        field_box(ui, 244.0, "PDF (cad_plot)", true, true);
                        ui.add_space(6.0);
                        icon_btn(ui, "sliders", false).on_hover_text("Printer settings — later phase");
                    });
                    ui.horizontal(|ui| {
                        lbl(ui, "Preset", false);
                        field_box(ui, 244.0, "<None>", true, false);
                        ui.add_space(6.0);
                        icon_btn(ui, "save", false);
                    });
                    ui.horizontal(|ui| {
                        let _ = check(ui, 100.0, "Plot to file", true, true);
                        ui.label(egui::RichText::new("choose folder + name on Plot")
                            .color(tc::TEXT_MUTED).small());
                    });

                    // ---- AREA & SCALE ----
                    sh(ui, "AREA & SCALE", false);
                    ui.horizontal(|ui| {
                        lbl(ui, "What to plot", true);
                        let r = field_box(ui, 244.0, ["Extents", "Window", "Display"][self.plot_area_kind.min(2) as usize], true, true);
                        let pid = egui::Id::new("plot_what_pop");
                        if r.clicked() { ui.memory_mut(|m| m.toggle_popup(pid)); }
                        egui::popup_below_widget(ui, pid, &r, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                            ui.set_min_width(150.0);
                            if ui.selectable_label(self.plot_area_kind == 0, "Extents").clicked() { self.plot_area_kind = 0; }
                            if ui.selectable_label(self.plot_area_kind == 1, "Window").clicked() { self.plot_area_kind = 1; }
                            if ui.selectable_label(self.plot_area_kind == 2, "Display").clicked() { self.plot_area_kind = 2; }
                        });
                    });
                    if self.plot_area_kind == 1 {
                        ui.horizontal(|ui| {
                            lbl(ui, "", true);
                            if ui.button("Pick window <").clicked() { pick_window = true; }
                            if ui.button("Preview")
                                .on_hover_text("Full preview of the plotted output, with a Confirm option")
                                .clicked() { open_preview = true; }
                            let t = match self.plot_window {
                                Some((mn, mx)) => format!("({:.0},{:.0})–({:.0},{:.0})", mn.x, mn.y, mx.x, mx.y),
                                None => "not picked yet".to_string(),
                            };
                            ui.label(egui::RichText::new(t).color(tc::TEXT_MUTED).small());
                        });
                    } else {
                        // Extents / Display have no Pick-window row → put Preview on
                        // the What-to-plot line so it's always reachable.
                        ui.horizontal(|ui| {
                            lbl(ui, "", true);
                            if ui.button("Preview")
                                .on_hover_text("Full preview of the plotted output, with a Confirm option")
                                .clicked() { open_preview = true; }
                        });
                    }
                    ui.horizontal(|ui| {
                        if check(ui, 110.0, "Fit to paper", self.plot_scale_fit, true) { self.plot_scale_fit = !self.plot_scale_fit; }
                        ui.label(egui::RichText::new("1").color(tc::TEXT_MUTED).small());
                        // mm / inch unit selector (paper side of the scale; default mm).
                        let ur = field_box(ui, 58.0, unit_name, true, true);
                        let uid = egui::Id::new("plot_unit_pop");
                        if ur.clicked() { ui.memory_mut(|m| m.toggle_popup(uid)); }
                        egui::popup_below_widget(ui, uid, &ur, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                            ui.set_min_width(80.0);
                            if ui.selectable_label(!self.plot_unit_inch, "mm").clicked() { self.plot_unit_inch = false; }
                            if ui.selectable_label(self.plot_unit_inch, "inch").clicked() { self.plot_unit_inch = true; }
                        });
                        ui.label(egui::RichText::new("=").color(tc::TEXT_MUTED).small());
                        if self.plot_scale_fit {
                            let t = fit_units.map(|u| format!("{:.3}", u * unit_mm)).unwrap_or_else(|| "—".into());
                            field_box(ui, 64.0, &t, false, false);
                        } else {
                            let mut n = self.plot_scale_n;
                            num_box(ui, 64.0, &mut n, true);
                            self.plot_scale_n = n.max(0.001);
                        }
                        ui.label(egui::RichText::new("units").color(tc::TEXT_MUTED).small());
                    });
                    ui.horizontal(|ui| {
                        if check(ui, 110.0, "Center the plot", self.plot_offset_center, true) { self.plot_offset_center = !self.plot_offset_center; }
                        let en = !self.plot_offset_center;
                        ui.label(egui::RichText::new("X").color(tc::TEXT_MUTED));
                        let mut x = self.plot_offset_x as f64;
                        num_box(ui, 60.0, &mut x, en); self.plot_offset_x = x as f32;
                        ui.label(egui::RichText::new("Y").color(tc::TEXT_MUTED));
                        let mut y = self.plot_offset_y as f64;
                        num_box(ui, 60.0, &mut y, en); self.plot_offset_y = y as f32;
                    });

                    // ---- PEN & QUALITY ----
                    sh(ui, "PEN & QUALITY", false);
                    ui.horizontal(|ui| {
                        lbl(ui, "Plot style", true);
                        let r = field_box(ui, 244.0, &table_name, true, true);
                        let pid = egui::Id::new("plot_table_pop");
                        if r.clicked() { ui.memory_mut(|m| m.toggle_popup(pid)); }
                        egui::popup_below_widget(ui, pid, &r, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                            ui.set_min_width(180.0);
                            let _ = ui.selectable_label(true, format!("{} (document)", table_name));
                            if ui.selectable_label(false, "Load .pst…").clicked() { load_pst = true; }
                        });
                        ui.add_space(6.0);
                        let pencil = icon_btn(ui, "pencil", true).on_hover_text("Edit the plot style table");
                        edit_anchor = Some(pencil.rect);
                        if pencil.clicked() { open_editor = true; }
                    });
                    ui.horizontal(|ui| {
                        lbl(ui, "Quality", false);
                        field_box(ui, 168.0, "Vector (N/A)", true, false);
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("DPI").color(tc::TEXT_DISABLED));
                        field_box(ui, 44.0, "—", false, false);
                    });
                    ui.horizontal(|ui| {
                        lbl(ui, "Shade plot", false);
                        field_box(ui, 244.0, "As displayed (N/A)", true, false);
                    });

                    // ---- OPTIONS ----
                    sh(ui, "OPTIONS", false);
                    ui.horizontal(|ui| {
                        if check(ui, 176.0, "Object lineweights", self.plot_object_lw, true) { self.plot_object_lw = !self.plot_object_lw; }
                        if check(ui, 150.0, "Plot styles", self.plot_with_styles, true) { self.plot_with_styles = !self.plot_with_styles; }
                    });
                    ui.horizontal(|ui| {
                        let _ = check(ui, 176.0, "Transparency", false, false);
                        if check(ui, 150.0, "Scale lineweights", self.plot_scale_lw, true) { self.plot_scale_lw = !self.plot_scale_lw; }
                    });
                    ui.horizontal(|ui| {
                        if check(ui, 176.0, "Plot all black", self.plot_mono, true) { self.plot_mono = !self.plot_mono; }
                        let _ = check(ui, 150.0, "Plot in background", false, false);
                    });
                    ui.horizontal(|ui| {
                        let _ = check(ui, 176.0, "Plot stamp", false, false);
                    });
                  });
                });

                // vertical divider (fixed length to match the columns)
                let (dv, _) = ui.allocate_exact_size(egui::vec2(1.0, 366.0), egui::Sense::hover());
                ui.painter().vline(dv.center().x, dv.y_range(), egui::Stroke::new(1.0, tc::BORDER));

                // ===================== RIGHT column =====================
                egui::Frame::none().inner_margin(egui::Margin { left: 16.0, right: 16.0, top: 8.0, bottom: 8.0 }).show(ui, |ui| {
                  ui.vertical(|ui| {
                    ui.set_width(238.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let (pw, ph) = self.plot_paper.dims_mm();
                    let (pw, ph) = if self.plot_landscape { (ph, pw) } else { (pw, ph) };
                    sh(ui, &format!("PREVIEW · {} · {:.0}×{:.0} mm", paper_label(self.plot_paper), pw, ph), true);
                    let (pv, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 208.0), egui::Sense::hover());
                    ui.painter().rect(pv, egui::Rounding::same(8.0), tc::SURFACE_2, egui::Stroke::new(1.0, tc::BORDER));
                    self.plot_preview_paint(&ui.painter_at(pv), pv, cap.clone(), false);

                    // ---- ORIENTATION ----
                    sh(ui, "ORIENTATION", false);
                    ui.horizontal(|ui| {
                        let bw = (ui.available_width() - 8.0) * 0.5;
                        let mk = |sel: bool, t: &str| egui::Button::new(
                            egui::RichText::new(t).color(if sel { tc::ON_ACCENT } else { tc::TEXT_PRIMARY }))
                            .fill(if sel { tc::ACCENT } else { tc::SURFACE_1 })
                            .stroke(egui::Stroke::new(1.0, tc::BORDER))
                            .rounding(egui::Rounding::same(6.0));
                        if ui.add_sized([bw, 34.0], mk(!self.plot_landscape, "Portrait")).clicked() { self.plot_landscape = false; }
                        if ui.add_sized([bw, 34.0], mk(self.plot_landscape, "Landscape")).clicked() { self.plot_landscape = true; }
                    });
                    let _ = check(ui, 130.0, "Upside-down", false, false);

                    // ---- COPIES (greyed) ----
                    sh(ui, "COPIES", false);
                    ui.horizontal(|ui| {
                        icon_btn(ui, "", false);
                        let (r, _) = ui.allocate_exact_size(egui::vec2(46.0, PP_ROW_H), egui::Sense::hover());
                        ui.painter().rect(r, egui::Rounding::same(crate::theme::radius::SM), PP_BG_LO, egui::Stroke::new(1.0, PP_BORDER));
                        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "1", body_font.clone(), tc::TEXT_DISABLED);
                        icon_btn(ui, "", false);
                    });

                    // ---- PAPER SIZE ----
                    sh(ui, "PAPER SIZE", false);
                    let w = ui.available_width();
                    let r = field_box(ui, w, paper_label(self.plot_paper), true, true);
                    let pid = egui::Id::new("plot_paper_pop");
                    if r.clicked() { ui.memory_mut(|m| m.toggle_popup(pid)); }
                    egui::popup_below_widget(ui, pid, &r, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                        ui.set_min_width(200.0);
                        for p in [PaperSize::A4, PaperSize::A3, PaperSize::A2, PaperSize::A1, PaperSize::A0, PaperSize::Letter] {
                            let (w, h) = p.dims_mm();
                            let lab = format!("{}    {:.0} × {:.0} mm", paper_label(p), w, h);
                            if ui.selectable_label(self.plot_paper == p, lab).clicked() { self.plot_paper = p; }
                        }
                    });
                  });
                });
            });

                });

            // ===== OUTPUT FORMAT (PDF / SVG / PNG) =====
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Format:");
                let fmts = ["PDF", "SVG", "PNG"];
                let mut fi = match self.plot_format.as_str() { "svg" => 1, "png" => 2, _ => 0 };
                egui::ComboBox::from_id_salt("plot_fmt2")
                    .selected_text(fmts[fi]).show_ui(ui, |ui| {
                        for (i, n) in fmts.iter().enumerate() {
                            ui.selectable_value(&mut fi, i, *n);
                        }
                    });
                self.plot_format = match fi { 1 => "svg".into(), 2 => "png".into(), _ => "pdf".into() };
                ui.label(egui::RichText::new("PDF vector · SVG vector · PNG 300 dpi")
                    .color(tc::TEXT_MUTED).small());
            });

            // ===== footer: Apply to layout · Cancel · Plot ===== (PINNED)
            ui.separator();
            egui::Frame::none().inner_margin(egui::Margin::symmetric(18.0, 10.0)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let _ = ui.add(egui::Label::new(egui::RichText::new("Apply to layout").color(tc::ACCENT)).sense(egui::Sense::click()));
                    ui.add_space((ui.available_width() - 170.0).max(0.0));
                    let cancel = egui::Button::new("Cancel").min_size(egui::vec2(74.0, 32.0))
                        .fill(tc::SURFACE_1).stroke(egui::Stroke::new(1.0, tc::BORDER)).rounding(egui::Rounding::same(6.0));
                    if ui.add(cancel).clicked() { close_panel = true; }
                    let plot = egui::Button::new(egui::RichText::new("Plot").color(tc::ON_ACCENT).strong())
                        .min_size(egui::vec2(80.0, 32.0)).fill(tc::ACCENT).rounding(egui::Rounding::same(6.0));
                    if ui.add(plot).clicked() { do_ok = true; }
                });
            });
        });
        self.plotstyle_edit_anchor = edit_anchor;
        self.plot_dialog_rect = resp.as_ref().map(|r| r.response.rect);
        self.raise_after_show("PlotDialog", ctx, &resp);
        let _ = resp;

        if pick_window {
            self.plot_win_pick = 1;
            self.plot_win_p1 = None;
            self.set_prompt("plot: drag a window rectangle on the drawing  [Esc=cancel]");
        }
        if load_pst {
            self.plotstyle_pst_io = Some(false);
            self.open_file_dialog(FileDialogMode::Open, ".pst");
        }
        if open_editor {
            self.plotstyle_edit_file = None; // the doc's own table, not a CTB file
            self.plotstyle_open = true;
            if self.plotstyle_sel.is_empty() {
                self.plotstyle_sel = vec![1];
                self.plotstyle_anchor = Some(1);
            }
            // Replace the Plot dialog with the editor (no menu-on-menu); the Plot
            // dialog reappears when the editor closes.
            self.plot_dialog_open = false;
            self.plotstyle_return_to_plot = true;
        }
        if open_preview {
            self.plot_preview_open = true;
        }
        if do_ok {
            // Owner flow: pressing Plot asks the user for the output folder + file
            // name (Save dialog), THEN writes the PDF and opens it (see the file-
            // dialog confirm handler → run_plot). No standalone Browse.
            self.plot_pdf_browse = true;
            self.plot_run_after_save = true;
            let ext = match self.plot_format.as_str() {
                "svg" => "svg",
                "png" => "png",
                _ => "pdf",
            };
            self.open_file_dialog(FileDialogMode::Save, &format!(".{}", ext));
        }
        if close_panel {
            self.plot_dialog_open = false;
            self.clear_prompt();
        }
    }

    /// The full print-preview window (renders the real output + Confirm).
    pub(super) fn render_plot_preview_window(&mut self, ctx: &egui::Context) {
        if !self.plot_preview_open {
            self.prev_plot_preview_open = false;
            return;
        }
        let pv_just_opened = !self.prev_plot_preview_open;
        self.prev_plot_preview_open = true;
        if pv_just_opened {
            self.plot_preview_open_seq = self.plot_preview_open_seq.wrapping_add(1);
        }
        use crate::theme::color as tc;
        self.ensure_layer_glyph_textures(ctx);
        let gx_close = self
            .layer_glyph_tex
            .get("close")
            .map(|t| (t.id(), t.size_vec2()));
        let cap = crate::theme::typ::caption();
        let (pw0, ph0) = self.plot_paper.dims_mm();
        let (pw, ph) = if self.plot_landscape {
            (ph0, pw0)
        } else {
            (pw0, ph0)
        };
        let title = format!(
            "Print preview · {} · {:.0}×{:.0} mm",
            paper_label(self.plot_paper),
            pw,
            ph
        );
        let mut close = false;
        let mut confirm = false;
        let frame = egui::Frame::none()
            .fill(tc::SURFACE_1)
            .rounding(egui::Rounding::same(12.0))
            .stroke(egui::Stroke::new(1.0, tc::BORDER))
            .inner_margin(egui::Margin::ZERO);
        let pvcr = self.canvas_screen_rect.unwrap_or_else(|| ctx.screen_rect());
        let win = egui::Window::new("Plot preview")
            .order(egui::Order::Foreground)
            .title_bar(false)
            .frame(frame)
            .default_size(egui::vec2(620.0, 520.0))
            .resizable(true)
            .movable(true);
        let win = Self::place_child(
            win,
            "plot_preview_win",
            self.plot_preview_open_seq,
            self.plot_dialog_rect,
            pvcr,
        );
        let resp = win.show(ctx, |ui| {
            let area = ui.max_rect();
            let full_w = area.width();
            let hdr_font = egui::TextStyle::Button.resolve(ui.style());
            let (hdr, _) = ui.allocate_exact_size(egui::vec2(full_w, 30.0), egui::Sense::hover());
            {
                let hp = ui.painter_at(hdr);
                hp.rect_filled(
                    hdr,
                    egui::Rounding {
                        nw: 12.0,
                        ne: 12.0,
                        sw: 0.0,
                        se: 0.0,
                    },
                    tc::BORDER,
                );
                hp.text(
                    egui::pos2(area.left() + 14.0, hdr.center().y),
                    egui::Align2::LEFT_CENTER,
                    &title,
                    hdr_font,
                    tc::TEXT_PRIMARY,
                );
            }
            let xr = egui::Rect::from_center_size(
                egui::pos2(area.right() - 13.0, hdr.center().y),
                egui::vec2(13.5, 13.5),
            );
            let xresp = ui.interact(
                xr,
                egui::Id::new("plot_preview_close"),
                egui::Sense::click(),
            );
            {
                let hp = ui.painter_at(hdr);
                let xcol = if xresp.hovered() {
                    hp.rect_filled(
                        xr.expand(3.0),
                        4.0,
                        egui::Color32::from_rgba_unmultiplied(0xE5, 0x48, 0x4D, 45),
                    );
                    LYR_DANGER
                } else {
                    LYR_MUTED
                };
                blit_layer_glyph(&hp, xr, gx_close, xcol);
            }
            if xresp.clicked() {
                close = true;
            }

            egui::Frame::none()
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        let avail = ui.available_size();
                        let box_h = (avail.y - 46.0).max(160.0);
                        let (pv, _) = ui
                            .allocate_exact_size(egui::vec2(avail.x, box_h), egui::Sense::hover());
                        ui.painter().rect(
                            pv,
                            egui::Rounding::same(8.0),
                            tc::SURFACE_2,
                            egui::Stroke::new(1.0, tc::BORDER),
                        );
                        self.plot_preview_paint(&ui.painter_at(pv), pv, cap.clone(), true);
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("This is exactly what will print.")
                                    .color(tc::TEXT_MUTED)
                                    .small(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let plot = egui::Button::new(
                                        egui::RichText::new("Confirm — Plot")
                                            .color(tc::ON_ACCENT)
                                            .strong(),
                                    )
                                    .min_size(egui::vec2(122.0, 30.0))
                                    .fill(tc::ACCENT)
                                    .rounding(egui::Rounding::same(6.0));
                                    if ui.add(plot).clicked() {
                                        confirm = true;
                                    }
                                    let cancel = egui::Button::new("Close")
                                        .min_size(egui::vec2(74.0, 30.0))
                                        .fill(tc::SURFACE_1)
                                        .stroke(egui::Stroke::new(1.0, tc::BORDER))
                                        .rounding(egui::Rounding::same(6.0));
                                    if ui.add(cancel).clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                    });
                });
        });
        self.raise_after_show("PlotPreview", ctx, &resp);

        if confirm {
            self.plot_preview_open = false;
            // Proceed to the plot flow: choose file → write → open.
            self.plot_pdf_browse = true;
            self.plot_run_after_save = true;
            let ext = match self.plot_format.as_str() {
                "svg" => "svg",
                "png" => "png",
                _ => "pdf",
            };
            self.open_file_dialog(FileDialogMode::Save, &format!(".{}", ext));
        }
        if close {
            self.plot_preview_open = false;
        }
    }

    /// Dockable Page Setup dialog (model-space plot configuration).
    pub(super) fn render_pagesetup_dialog(&mut self, ctx: &egui::Context) {
        if !self.pagesetup_open {
            return;
        }
        let cfg = crate::dock::DockConfig {
            id: "pagesetup",
            title: "Page Setup",
            badge: None,
            dock_region: crate::dock::DockRegion::Right,
            alt_region: Some(crate::dock::DockRegion::Left),
            any_edge: false,
            strip_h: 0.0,
            dockable: false,
            rail_header: true,
            collapsible: true,
            size: 320.0,
            min: 300.0,
            max: 420.0,
            resizable: false,
            flush_body: true,
            float_w: 320.0,
            float_max_h_frac: 0.9,
        };
        let mut state = self.pagesetup_dock_state.clone();
        let mut open = true;
        if matches!(state, crate::dock::DockState::Floating(_)) {
            crate::dock::HOST.show(ctx, &cfg, &mut state, &mut open, |ui, _cap| {
                egui::Frame::none()
                    .inner_margin(egui::Margin {
                        left: 16.0,
                        right: 16.0,
                        top: 12.0,
                        bottom: 14.0,
                    })
                    .show(ui, |ui| {
                        self.pagesetup_panel_body(ui);
                    });
            });
        }
        self.pagesetup_dock_state = state;
        if !open {
            self.pagesetup_open = false;
        }
    }

    fn pagesetup_panel_body(&mut self, ui: &mut egui::Ui) {
        use cad_kernel::plotstyle::{Orientation, PaperSize};
        use egui::RichText;
        let mut saved = false;
        ui.label(
            RichText::new("Saved page configuration for model-space plots")
                .small()
                .color(egui::Color32::from_rgb(150, 165, 185)),
        );
        ui.add_space(6.0);

        // Paper size.
        ui.label("Paper size");
        let papers = ["A4", "A3", "A2", "A1", "A0", "Letter"];
        let cur = paper_label(self.doc.page_setup.paper).to_string();
        egui::ComboBox::from_id_salt("ps_paper")
            .selected_text(&cur)
            .show_ui(ui, |ui| {
                for p in papers {
                    if ui.selectable_label(cur == p, p).clicked() {
                        self.doc.page_setup.paper = match p {
                            "A3" => PaperSize::A3,
                            "A2" => PaperSize::A2,
                            "A1" => PaperSize::A1,
                            "A0" => PaperSize::A0,
                            "Letter" => PaperSize::Letter,
                            _ => PaperSize::A4,
                        };
                    }
                }
            });

        // Orientation.
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.radio_value(
                &mut self.doc.page_setup.orientation,
                Orientation::Portrait,
                "Portrait",
            );
            ui.radio_value(
                &mut self.doc.page_setup.orientation,
                Orientation::Landscape,
                "Landscape",
            );
        });

        // Margins (mm).
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Margins (mm)");
            ui.add(
                egui::DragValue::new(&mut self.doc.page_setup.margins_mm)
                    .range(0.0..=50.0)
                    .speed(0.5)
                    .update_while_editing(false),
            );
        });

        // Scale.
        ui.add_space(4.0);
        ui.checkbox(&mut self.doc.page_setup.scale_fit, "Fit to paper");
        if !self.doc.page_setup.scale_fit {
            ui.horizontal(|ui| {
                ui.label("1 :");
                ui.add(
                    egui::DragValue::new(&mut self.doc.page_setup.scale_n)
                        .range(0.001..=1e6)
                        .speed(1.0)
                        .update_while_editing(false),
                );
            });
        }

        // Unit.
        ui.add_space(4.0);
        ui.radio_value(&mut self.doc.page_setup.unit_inch, false, "Millimetres");
        ui.radio_value(&mut self.doc.page_setup.unit_inch, true, "Inches");

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Save").strong()).clicked() {
                saved = true;
            }
            if ui.button("Close").clicked() {
                self.pagesetup_open = false;
            }
        });
        if saved {
            let (w, h) = self.doc.page_setup.paper.dims_mm();
            let (pw, ph) = match self.doc.page_setup.orientation {
                Orientation::Portrait => (w, h),
                Orientation::Landscape => (h, w),
            };
            let scale_txt = if self.doc.page_setup.scale_fit {
                "fit".to_string()
            } else {
                format!("1:{:.3}", self.doc.page_setup.scale_n)
            };
            self.set_prompt(format!(
                "pagesetup: saved {} {:.0}×{:.0} mm  margins {:.1} mm  {}",
                paper_label(self.doc.page_setup.paper),
                pw,
                ph,
                self.doc.page_setup.margins_mm,
                scale_txt
            ));
            self.history.push(format!(
                "  ✓ pagesetup: {} {:.0}×{:.0} mm  margins {:.1} mm  {}",
                paper_label(self.doc.page_setup.paper),
                pw,
                ph,
                self.doc.page_setup.margins_mm,
                scale_txt
            ));
        }
    }

    /// Floating Recorder window. Controls + live counters + buttons
    /// for Note / Snap-now / Clear / Copy. Inspector / replay UI ships
    /// in Slice C.
    pub(super) fn render_dbg_recorder_window(&mut self, ctx: &egui::Context) {
        if !self.dbg_window_open {
            return;
        }
        let mut open = self.dbg_window_open;
        let win = egui::Window::new("🛰 Session Recorder")
            .open(&mut open)
            .default_pos(egui::pos2(40.0, 40.0))
            .default_size(egui::vec2(380.0, 220.0))
            .resizable(true)
            .collapsible(true);
        // §10: anchor to the launching menu row (first open) + raise to front.
        let win = self.apply_dock_pos("Session Recorder", ctx, win);
        let resp = win.show(ctx, |ui| {
            let is_recording = self.dbg.recording;
            ui.horizontal(|ui| {
                let start_btn = egui::Button::new(
                    egui::RichText::new("▶ Start")
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(if is_recording {
                    egui::Color32::from_rgb(50, 90, 50)
                } else {
                    egui::Color32::from_rgb(40, 130, 50)
                });
                if ui.add_enabled(!is_recording, start_btn).clicked() {
                    self.dbg_start();
                }
                let stop_btn = egui::Button::new(
                    egui::RichText::new("■ Stop")
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(if is_recording {
                    egui::Color32::from_rgb(160, 50, 50)
                } else {
                    egui::Color32::from_rgb(80, 50, 50)
                });
                if ui.add_enabled(is_recording, stop_btn).clicked() {
                    self.dbg_stop();
                }
                ui.separator();
                // COUNT DOBJECTS ONLY — keep the NUMBER of selected/affected
                // dobjects, never the index lists. The lists are dropped at CAPTURE
                // (in DbgRecorder::push), so this bounds the recorder's memory too,
                // not just the dump's size.
                let co = ui
                    .selectable_label(self.dbg.counts_only, "# Count dobjects ONLY")
                    .on_hover_text(
                        "Record only HOW MANY dobjects were selected/affected — never \
                         the index lists.\n\nA gesture prints its selection twice \
                         (before → after): ~11 KB at 916 selected, ~11 MB at a million. \
                         Turn this on for large-drawing work and dumps stay readable.",
                    );
                if co.clicked() {
                    self.dbg.counts_only = !self.dbg.counts_only;
                    self.history.push(format!(
                        "  🛰 recorder: count dobjects only {}",
                        if self.dbg.counts_only { "ON" } else { "off" }
                    ));
                }
                ui.separator();
                if ui.button("🗑 Clear").clicked() {
                    self.dbg.clear();
                    self.history.push("  🛰 recorder cleared".into());
                }
                if ui.button("📷 Snap").clicked() {
                    let undo_d = self.undo_stack.len();
                    let redo_d = self.redo_stack.len();
                    self.dbg.take_snapshot(
                        Self::plan_doc_of(self.factory.session.as_ref(), &self.doc),
                        "manual snap",
                        undo_d,
                        redo_d,
                        describe_verbose,
                        std::panic::Location::caller(),
                    );
                }
            });
            ui.add_space(4.0);
            ui.label(format!(
                "Status: {}  ·  {} events  ·  {} snapshots",
                if is_recording {
                    "🔴 RECORDING"
                } else {
                    "⚪ idle"
                },
                self.dbg.events.len(),
                self.dbg.snapshots.len()
            ));
            ui.add_space(6.0);
            ui.separator();
            // ---- Smart-block authoring: capture base geometry ----------
            ui.label("Smart dobject (parametric block) authoring:");
            ui.horizontal(|ui| {
                let n = if !self.selection.is_empty() {
                    self.selection.len()
                } else if self.selected.is_some() {
                    1
                } else {
                    0
                };
                let cap = egui::Button::new(
                    egui::RichText::new("📐 Capture smart dobject")
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(60, 90, 140));
                if ui
                    .add(cap)
                    .on_hover_text(
                        "Snapshot the FULL geometry of the selected dobject(s) into \
                     the timeline — an exploded set OR a block (its definition \
                     contents are dumped too). Pair with the recorded ✂REC \
                     stretches to convert into a parametric smart block.",
                    )
                    .clicked()
                {
                    self.capture_smart_geometry();
                }
                ui.label(format!("{} selected", n));
            });
            ui.small(
                "Start ▶ · select the dobjects · 📐 Capture · do your \
                      stretches (each logs ✂REC) · Stop ■ · 📋 Copy.",
            );
            ui.add_space(6.0);
            ui.separator();
            // ---- Menu layout capture (ANY open menu's geometry) -----------
            ui.label("Menu layout (compare rendered vs design):");
            ui.horizontal(|ui| {
                let lay = egui::Button::new(
                    egui::RichText::new("📐 Capture menu layout")
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(90, 70, 140));
                if ui
                    .add(lay)
                    .on_hover_text(
                        "Records the pixel geometry (x y w h) of every element in WHATEVER \
                     menus are open this frame — the menu-bar dropdowns, the canvas \
                     right-click menu, the object-snap popup, and the Properties panel — \
                     into the timeline. Open the menu(s) you want measured FIRST, keep \
                     them on screen, then click this.",
                    )
                    .clicked()
                {
                    self.props_layout_capture = true;
                    if !self.dbg.recording {
                        self.dbg_start();
                    }
                }
                if self.props_layout_capture {
                    ui.label("⏳ capturing next frame…");
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.label("Annotate (📝 added with current ms):");
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.dbg_note_buf)
                        .desired_width(240.0)
                        .hint_text("bug fired here / picked the wrong dobject / etc."),
                );
                if (ui.button("Drop note").clicked()
                    || (resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter))))
                    && !self.dbg_note_buf.is_empty()
                {
                    let msg = std::mem::take(&mut self.dbg_note_buf);
                    crate::dbg_event!(self, crate::dbg_recorder::DbgEvent::Note { message: msg });
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("📋 Copy timeline").clicked() {
                    let dump = self.dbg.dump_text();
                    ctx.copy_text(dump);
                    self.history.push("  🛰 timeline copied to clipboard".into());
                }
                ui.checkbox(&mut self.dbg.capture_backtrace, "Capture backtrace (slow)");
            });
            ui.horizontal(|ui| {
                ui.label("Auto-snap every:");
                ui.add(
                    egui::DragValue::new(&mut self.dbg.auto_snap_every)
                        .update_while_editing(false)
                        .speed(1.0)
                        .range(0..=10_000)
                        .suffix(" events"),
                );
            });
        });
        self.raise_after_show("Session Recorder", ctx, &resp);
        self.dbg_window_open = open;
    }

    /// Trim Debug floating window — instrumented log of every trim /
    /// extend state transition + canvas click. User pastes the log back
    /// when reporting a bug.
    pub(super) fn render_trim_debug_window(&mut self, ctx: &egui::Context) {
        let mut open = self.trim_debug_open;
        let win = egui::Window::new("Trim Debug Log")
            .open(&mut open)
            .default_width(640.0)
            .default_height(400.0)
            .resizable(true);
        let win = self.apply_dock_pos("Trim Debug Log", ctx, win);
        let resp = win.show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("📋 Copy Log")
                    .on_hover_text("Copy the whole log to the clipboard")
                    .clicked()
                {
                    let text = self.trim_debug_log.join("\n");
                    ui.ctx().copy_text(text);
                    self.history.push("  trim debug log → clipboard".into());
                }
                if ui.button("🗑 Clear").clicked() {
                    self.trim_debug_log.clear();
                }
                ui.label(format!("{} entries", self.trim_debug_log.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let trim_active = matches!(
                        self.trim_state,
                        TrimState::PickingTargets(_) | TrimState::PickingTargetsAll
                    );
                    let extend_active = matches!(
                        self.extend_state,
                        ExtendState::PickingTargets(_) | ExtendState::PickingTargetsAll
                    );
                    if trim_active {
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 170, 60),
                            "● TRIM target-pick active",
                        );
                    } else if extend_active {
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 220, 90),
                            "● EXTEND target-pick active",
                        );
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(140, 140, 150),
                            "○ no session running",
                        );
                    }
                });
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if self.trim_debug_log.is_empty() {
                        ui.colored_label(
                            egui::Color32::from_rgb(140, 140, 150),
                            "(empty — run `trim` or `extend` to start logging)",
                        );
                    }
                    for line in &self.trim_debug_log {
                        ui.monospace(line);
                    }
                });
        });
        self.process_dock_after_show("Trim Debug Log", ctx, resp);
        self.trim_debug_open = open;
    }

    /// Hatch debug window — same shape as the trim debug log. Records
    /// every state transition in the hatch flow so the user can pin
    /// down where the hatch is "falling": dialog open / pattern +
    /// scale + angle changes / Dobjects vs Pick Point button / canvas
    /// click consumed for pick-point / smallest-containing search
    /// candidates + winner / apply_hatch params / resolved loops /
    /// render line counts. Toggle via Tools menu.
    pub(super) fn render_hatch_debug_window(&mut self, ctx: &egui::Context) {
        let mut open = self.hatch_debug_open;
        let mut do_dump_state = false;
        let mut do_save_report = false;
        let win = egui::Window::new("Hatch Debug Log")
            .open(&mut open)
            .default_width(720.0)
            .default_height(420.0)
            .resizable(true);
        let win = self.apply_dock_pos("Hatch Debug Log", ctx, win);
        let resp = win.show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("📋 Copy Log").on_hover_text("Copy the whole log to the clipboard").clicked() {
                        let text = self.hatch_debug_log.join("\n");
                        ui.ctx().copy_text(text);
                        self.history.push("  hatch debug log → clipboard".into());
                    }
                    if ui.button("🗑 Clear").clicked() {
                        self.hatch_debug_log.clear();
                    }
                    if ui.button("📸 Dump Hatch State")
                        .on_hover_text("Append a snapshot of every Hatch dobject in the doc (boundary handles, resolved loop vertex counts, pattern). Press anytime; no per-frame flood.")
                        .clicked()
                    {
                        do_dump_state = true;
                    }
                    if ui.button("💾 Save Report")
                        .on_hover_text("Write the whole log plus a document snapshot \
                                        to hatch_report.txt next to the app — the file \
                                        to send when reporting a hatch problem.")
                        .clicked()
                    {
                        do_save_report = true;
                    }
                    ui.label(format!("{} entries", self.hatch_debug_log.len()));
                    // Live status — what's the hatch flow doing right now?
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.hatch_dialog_open {
                            ui.colored_label(egui::Color32::from_rgb(120, 220, 255),
                                "● dialog open");
                        } else if self.hatch_pick_point_armed {
                            ui.colored_label(egui::Color32::from_rgb(255, 200, 120),
                                "● pick-point armed");
                        } else if matches!(self.queued_op, QueuedOp::Hatch) {
                            ui.colored_label(egui::Color32::from_rgb(255, 220, 90),
                                "● awaiting boundary selection");
                        } else {
                            ui.colored_label(egui::Color32::from_rgb(140, 140, 150),
                                "○ idle");
                        }
                    });
                });
                ui.separator();
                // Real-time hatch attrs readout — always visible at the
                // top of the log so the user can SEE the pattern/scale/
                // angle that the next `apply_hatch` will use.
                let (pat, sc, ang) = &self.pending_hatch_pattern;
                ui.monospace(format!(
                    "pending: pattern={:<12} scale={:<6.3} angle={:>6.2}°    \
                     dialog(solid={}, name={:?}, scale={:.3}, angle={:.2})",
                    pat.as_deref().unwrap_or("(SOLID)"), sc, ang,
                    self.hatch_dialog_solid, self.hatch_dialog_name,
                    self.hatch_dialog_scale, self.hatch_dialog_angle));
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.hatch_debug_log.is_empty() {
                            ui.colored_label(egui::Color32::from_rgb(140, 140, 150),
                                "(empty — run `hatch` to start logging)");
                        }
                        for line in &self.hatch_debug_log {
                            ui.monospace(line);
                        }
                    });
            });
        self.process_dock_after_show("Hatch Debug Log", ctx, resp);
        self.hatch_debug_open = open;
        if do_dump_state {
            self.dump_hatch_state();
        }
        if do_save_report {
            self.save_hatch_report();
        }
    }

    /// Write a self-contained hatch bug report to `hatch_report.txt` in the
    /// working directory: environment, current hatch state, a fresh dump of
    /// every Hatch dobject, then the full log. This is the single file to
    /// attach when reporting a hatch problem — it carries the decision points
    /// (phases 1-10), not just the end result.
    fn save_hatch_report(&mut self) {
        use std::fmt::Write as _;
        // Refresh the per-hatch snapshot so the report always ends with
        // current state rather than whatever was last dumped.
        self.dump_hatch_state();
        let mut s = String::new();
        let _ = writeln!(s, "=== RUST-AutoRASM — HATCH DIAGNOSTIC REPORT ===");
        let _ = writeln!(s, "dobjects        : {}", self.doc.dobjects.len());
        let _ = writeln!(
            s,
            "layers          : {} (active '{}')",
            self.doc.layers.len(),
            self.doc
                .layers
                .get(self.doc.layers.active)
                .map(|l| l.name.clone())
                .unwrap_or_else(|| "?".into())
        );
        let _ = writeln!(s, "units           : {}", self.doc.units.name);
        let _ = writeln!(s, "view scale      : {:.4} px/unit", self.scale);
        let _ = writeln!(s, "selection       : {}", self.selection.len());
        let _ = writeln!(
            s,
            "dialog open     : {}   pick-point armed: {}",
            self.hatch_dialog_open, self.hatch_pick_point_armed
        );
        let _ = writeln!(s, "pattern pending : {:?}", self.pending_hatch_pattern);
        let _ = writeln!(
            s,
            "JOIN_EPS        : {:e}   (endpoint join / de-facto gap tolerance)",
            crate::hatch_trace::JOIN_EPS
        );
        let _ = writeln!(s, "\n--- LOG ({} entries) ---", self.hatch_debug_log.len());
        for l in &self.hatch_debug_log {
            let _ = writeln!(s, "{}", l);
        }
        let path = std::path::PathBuf::from("hatch_report.txt");
        match std::fs::write(&path, s) {
            Ok(()) => {
                let where_ = std::fs::canonicalize(&path)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string());
                self.history
                    .push(format!("  ✔ hatch report written → {}", where_));
            }
            Err(e) => self.history.push(format!("  ! hatch report failed: {}", e)),
        }
    }

    /// Append a one-shot snapshot of every Hatch dobject in the doc
    /// to the debug log. Triggered by the debug window's button. Lists
    /// boundary handle indices + resolved loop vertex counts so the
    /// user can see if "the hatch entity exists but has no rendered
    /// fill" is a render bug or a boundary-resolve bug.
    pub(super) fn dump_hatch_state(&mut self) {
        let mut entries: Vec<String> = Vec::new();
        for (i, d) in self.doc.dobjects.iter().enumerate() {
            if let Geom::Hatch(h) = &d.geom {
                let pat = match &h.pattern {
                    cad_kernel::HatchPattern::Solid => "SOLID".to_string(),
                    cad_kernel::HatchPattern::Pattern {
                        name,
                        scale,
                        angle_deg,
                    } => format!("{} scale={:.3} angle={:.2}", name, scale, angle_deg),
                };
                let resolved = self.resolve_hatch_loops(h);
                let handle_indices: Vec<String> = h
                    .boundary_handles
                    .iter()
                    .map(|hh| {
                        self.doc
                            .index_of_handle(*hh)
                            .map(|ix| format!("#{}", ix))
                            .unwrap_or_else(|| format!("(missing handle {:#x})", hh))
                    })
                    .collect();
                // Per-loop bbox + line-count estimate. This catches the
                // most common pattern-doesn't-show bug: pattern spacing
                // is larger than the loop, so 0 lines cross the boundary
                // and the fill looks blank. The diagnostic tells the
                // user "raise the scale".
                let mut loops_desc: Vec<String> = Vec::new();
                let mut union_min = Vec2::new(f64::INFINITY, f64::INFINITY);
                let mut union_max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
                for l in &resolved {
                    let mut lmin = Vec2::new(f64::INFINITY, f64::INFINITY);
                    let mut lmax = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
                    for v in l {
                        if v.x < lmin.x {
                            lmin.x = v.x;
                        }
                        if v.y < lmin.y {
                            lmin.y = v.y;
                        }
                        if v.x > lmax.x {
                            lmax.x = v.x;
                        }
                        if v.y > lmax.y {
                            lmax.y = v.y;
                        }
                        if v.x < union_min.x {
                            union_min.x = v.x;
                        }
                        if v.y < union_min.y {
                            union_min.y = v.y;
                        }
                        if v.x > union_max.x {
                            union_max.x = v.x;
                        }
                        if v.y > union_max.y {
                            union_max.y = v.y;
                        }
                    }
                    let w = lmax.x - lmin.x;
                    let hh = lmax.y - lmin.y;
                    loops_desc.push(format!("{}v bbox={:.2}x{:.2}", l.len(), w, hh));
                }
                // Estimate line count for each family in the pattern at
                // the dobject's current scale + angle.
                let mut line_estimate = String::new();
                if let cad_kernel::HatchPattern::Pattern { name, scale, .. } = &h.pattern {
                    if union_max.x.is_finite() {
                        let diag = ((union_max.x - union_min.x).powi(2)
                            + (union_max.y - union_min.y).powi(2))
                        .sqrt();
                        match cad_kernel::patterns::lookup(name) {
                            cad_kernel::patterns::Pattern::Families(fams) => {
                                let counts: Vec<String> = fams
                                    .iter()
                                    .map(|f| {
                                        let s = f.spacing * scale.abs().max(1e-9);
                                        let n = (diag / s).ceil() as i64;
                                        format!(
                                            "{}({} lines @ spacing {:.3})",
                                            (f.angle.to_degrees() as i32 % 360),
                                            n,
                                            s
                                        )
                                    })
                                    .collect();
                                line_estimate =
                                    format!("  estimated families: [{}]", counts.join(", "));
                                if counts.iter().any(|c| c.contains("(0 lines")) {
                                    line_estimate.push_str(
                                        "  ⚠ ZERO LINES — raise scale or pick a finer pattern",
                                    );
                                }
                            }
                            cad_kernel::patterns::Pattern::Tile {
                                period_x,
                                period_y,
                                segments,
                                circles,
                            } => {
                                let px = period_x * scale.abs().max(1e-9);
                                let py = period_y * scale.abs().max(1e-9);
                                let nx = (diag / px).ceil() as i64;
                                let ny = (diag / py).ceil() as i64;
                                line_estimate = format!(
                                    "  tile period={:.2}x{:.2} segs={} circles={} cells≈{}x{}",
                                    px,
                                    py,
                                    segments.len(),
                                    circles.len(),
                                    nx,
                                    ny
                                );
                                if px > diag || py > diag {
                                    line_estimate
                                        .push_str("  ⚠ tile period exceeds boundary — lower scale");
                                }
                            }
                        }
                    }
                }
                entries.push(format!(
                    "hatch #{} pattern=[{}] boundary_handles=[{}] resolved_loops=[{}]{}",
                    i,
                    pat,
                    handle_indices.join(", "),
                    loops_desc.join(", "),
                    line_estimate
                ));
            }
        }
        if entries.is_empty() {
            self.hatch_dbg("dump: no hatch dobjects in the doc".to_string());
        } else {
            self.hatch_dbg(format!("dump: {} hatch dobject(s):", entries.len()));
            for e in entries {
                self.hatch_dbg(format!("  {}", e));
            }
        }
    }

    /// Screen Stats window — confirms the renderer's view of the doc
    /// (total / in viewport / drawn / skipped) so the user can verify
    /// the spatial-index broad-phase is doing useful work, the
    /// viewport cull isn't dropping things it shouldn't, etc.
    /// Per-frame numbers from `last_render_stats`. Toggleable via the
    /// Tools menu; open by default because that's the whole point —
    /// the user wanted to SEE this info.
    pub(super) fn render_screen_stats_window(&mut self, ctx: &egui::Context) {
        // Detect false→true edge: user just reopened the window via
        // menu. Clear any stale snap-dock so the window appears at its
        // default_pos instead of getting stuck at an old snap position
        // (which can be off-screen or behind a panel after a resize).
        if self.screen_stats_open && !self.screen_stats_was_open {
            self.docked_window_pos.remove("Screen Stats");
        }
        self.screen_stats_was_open = self.screen_stats_open;
        if !self.screen_stats_open {
            return;
        }
        let mut open = self.screen_stats_open;
        let stats = self.last_render_stats.clone();
        let fps = if stats.frame_dt > 0.0 {
            1.0 / stats.frame_dt
        } else {
            0.0
        };
        let win = egui::Window::new("Screen Stats")
            .open(&mut open)
            .default_pos(egui::pos2(20.0, 110.0))
            .default_size(egui::vec2(280.0, 220.0))
            .resizable(true)
            .collapsible(true);
        let win = self.apply_dock_pos("Screen Stats", ctx, win);
        let resp = win.show(ctx, |ui| {
            ui.style_mut().override_font_id = Some(crate::theme::typ::data_value());
            let cull_ratio = if stats.total > 0 {
                100.0 * stats.in_viewport as f32 / stats.total as f32
            } else {
                0.0
            };
            let draw_ratio = if stats.in_viewport > 0 {
                100.0 * stats.drawn as f32 / stats.in_viewport as f32
            } else {
                0.0
            };

            egui::Grid::new("stats_grid")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.label("total dobjects:");
                    ui.label(format!("{}", stats.total));
                    ui.end_row();

                    ui.label("in viewport:");
                    ui.label(format!(
                        "{}  ({:.1}% of total)",
                        stats.in_viewport, cull_ratio
                    ));
                    ui.end_row();

                    ui.label("drawn:");
                    ui.label(format!("{}  ({:.1}% of viewport)", stats.drawn, draw_ratio));
                    ui.end_row();

                    ui.label("skipped:");
                    ui.label(format!(
                        "{}  (hidden / sub-pixel)",
                        stats.skipped_hidden + stats.skipped_subpx
                    ));
                    ui.end_row();

                    ui.label("");
                    ui.label("");
                    ui.end_row();

                    ui.label("FPS:");
                    ui.label(format!("{:.1}", fps));
                    ui.end_row();

                    ui.label("frame:");
                    ui.label(format!("{:.2} ms", stats.frame_dt * 1000.0));
                    ui.end_row();

                    ui.label("render mode:");
                    ui.label(match self.render_mode {
                        RenderMode::Cpu => "CPU",
                        RenderMode::Gpu => "GPU",
                        RenderMode::Apx => "APX",
                    });
                    ui.end_row();

                    ui.label("spatial idx:");
                    ui.label(&stats.index_label);
                    ui.end_row();
                });

            ui.separator();

            // Health hints — flag the common "renderer doesn't know
            // what's on screen" symptoms the user was worried about.
            if stats.in_viewport == stats.total && stats.total > 32 {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 200, 80),
                    "⚠ viewport cull not active: all dobjects iterated.\n\
                     spatial index is stale or absent (zoom/pan/edit invalidates it).",
                );
            } else if stats.total > 0 && stats.drawn == 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 140, 140),
                    "⚠ 0 drawn — everything filtered (hidden layer? sub-pixel? off-screen?)",
                );
            } else if stats.in_viewport > 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(140, 220, 140),
                    "✓ renderer sees the viewport set",
                );
            }
        });
        self.process_dock_after_show("Screen Stats", ctx, resp);
        self.screen_stats_open = open;
    }

    pub(super) fn render_layer_panel(&mut self, ctx: &egui::Context) {
        let mut open = self.layers_window_open;
        let win = egui::Window::new(format!("Layers ({})", self.doc.layers.len()))
            .open(&mut open)
            .default_pos(egui::pos2(10.0, 70.0))
            .default_size(egui::vec2(320.0, 480.0))
            .min_width(240.0)
            .resizable(true)
            .collapsible(true);
        let win = self.apply_dock_pos("Layers", ctx, win);
        let resp = win.show(ctx, |ui| {
                ui.separator();

                // ---- toolbar row: add + rename + delete -----------------
                ui.horizontal(|ui| {
                    if ui.button("➕ add").on_hover_text("Add a new layer").clicked() {
                        self.layer_name_counter += 1;
                        let mut name = format!("Layer{}", self.layer_name_counter);
                        // Bump the counter until the name is unique.
                        while self.doc.layers.find(&name).is_some() {
                            self.layer_name_counter += 1;
                            name = format!("Layer{}", self.layer_name_counter);
                        }
                        let id = self.doc.layers.add(Layer {
                            name,
                            ..Layer::layer_zero()
                        });
                        self.doc.layers.active = id;
                        self.history.push(format!(
                            "  + layer #{} (active)", id
                        ));
                    }
                    let active = self.doc.layers.active;
                    let can_delete = active != LayerTable::LAYER_ZERO;
                    ui.add_enabled_ui(can_delete, |ui| {
                        if ui.button("🗑 delete")
                            .on_hover_text("Delete the ACTIVE layer (Dobjects on it are NOT deleted; reassign them first)")
                            .clicked()
                        {
                            let name = self.doc.layers.get(active)
                                .map(|l| l.name.clone()).unwrap_or_default();
                            if self.doc.layers.remove(active) {
                                self.history.push(format!(
                                    "  - layer '{}' (#{}) deleted; active → 0", name, active
                                ));
                                // Reassign Dobjects on the removed layer to "0".
                                // Layers above `active` shifted down by 1; we
                                // need to remap their style.layer too.
                                for d in self.doc.dobjects.iter_mut() {
                                    if d.style.layer == active {
                                        d.style.layer = LayerTable::LAYER_ZERO;
                                    } else if d.style.layer > active {
                                        d.style.layer -= 1;
                                    }
                                }
                            }
                        }
                    });
                });
                ui.separator();

                // ---- header row -----------------------------------------
                egui::Grid::new("layer_header_grid")
                    .num_columns(7)
                    .spacing([6.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(""); // active
                        ui.label("👁");
                        ui.label("❄");
                        ui.label("🔒");
                        ui.label("color");
                        ui.label("linetype");
                        ui.label("name");
                        ui.end_row();
                    });

                // ---- one row per layer ----------------------------------
                let active = self.doc.layers.active;
                let mut new_active: Option<LayerId> = None;
                let mut lock_change: Option<(LayerId, bool)> = None;
                let mut rename_commit: Option<(LayerId, String)> = None;
                let mut rename_cancel = false;
                // Color edits can't intern into self.doc.truecolors directly
                // because the layer loop holds &mut self.doc.layers. Capture
                // (layer_id, packed_rgb) here; intern + assign after the loop.
                let mut color_change: Vec<(LayerId, u32)> = Vec::new();
                // Pick-button captures the layer id the user clicked; the
                // window-open assignment runs after the loop to stay clear
                // of the &mut self borrow chain.
                let mut pick_layer_color: Option<LayerId> = None;
                // Linetype changes captured here, applied AFTER the loop —
                // same pattern as color edits (can't borrow doc.linetypes
                // while doc.layers is borrowed mutably).
                let mut linetype_change: Vec<(LayerId, u32)> = Vec::new();
                // Snapshot the available linetypes (id, pattern, name) so the
                // graphical picker can render a sample per row without holding
                // a borrow on self.doc.
                let lt_entries: Vec<(u32, Vec<f32>, String)> = self.doc.linetypes.linetypes
                    .iter().enumerate()
                    .map(|(i, lt)| (i as u32, lt.pattern.clone(), lt.name.clone()))
                    .collect();
                let n = self.doc.layers.len();

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        egui::Grid::new("layer_rows")
                            .num_columns(7)
                            .spacing([6.0, 4.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for id in 0..(n as LayerId) {
                                    // Read-only first: pull the current
                                    // display color while no &mut layer
                                    // borrow exists, so we can dereference
                                    // self.doc.truecolors freely.
                                    let cur_color = self.doc.layers.get(id).map(|l| l.color);
                                    let rgb = match cur_color {
                                        Some(Color::TrueColorRef(idx)) => {
                                            let v = self.doc.truecolors.get(idx).unwrap_or(0xFFFFFF);
                                            (((v >> 16) & 0xFF) as u8,
                                             ((v >>  8) & 0xFF) as u8,
                                             ( v        & 0xFF) as u8)
                                        }
                                        Some(Color::Aci(i)) => aci_palette(i),
                                        _ => (255, 255, 255),
                                    };
                                    let layer = match self.doc.layers.get_mut(id) {
                                        Some(l) => l, None => continue,
                                    };

                                    // ----- active radio -------------------
                                    if ui.radio(id == active, "")
                                        .on_hover_text("Click to make this the active layer")
                                        .clicked()
                                    {
                                        new_active = Some(id);
                                    }

                                    // ----- visible toggle -----------------
                                    let mut v = layer.visible;
                                    if ui.checkbox(&mut v, "")
                                        .on_hover_text("Visible")
                                        .changed()
                                    {
                                        layer.visible = v;
                                    }

                                    // ----- freeze toggle ------------------
                                    let mut f = layer.frozen;
                                    if ui.checkbox(&mut f, "")
                                        .on_hover_text("Frozen (like hidden, also skipped on regen)")
                                        .changed()
                                    {
                                        layer.frozen = f;
                                    }

                                    // ----- lock toggle --------------------
                                    // DEFERRED, unlike visible/frozen: locking has to reach the
                                    // SELECTION as well as the layer, and that needs `self`,
                                    // which this loop has borrowed. Same shape as `new_active`.
                                    let mut l = layer.locked;
                                    if ui.checkbox(&mut l, "")
                                        .on_hover_text("Locked — Dobjects render and snap, but cannot be selected or edited")
                                        .changed()
                                    {
                                        lock_change = Some((id, l));
                                    }

                                    // ----- color swatch -------------------
                                    // Clickable swatch — opens the polar ACI
                                    // picker window for this layer. ACI is
                                    // the primary picker (see memo
                                    // `feedback_rust_cad_color_aci_primary`).
                                    let (swatch_rect, swatch_resp) = ui.allocate_exact_size(
                                        egui::vec2(22.0, 18.0), egui::Sense::click(),
                                    );
                                    ui.painter().rect_filled(
                                        swatch_rect, 2.0,
                                        egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2),
                                    );
                                    ui.painter().rect_stroke(
                                        swatch_rect, 2.0,
                                        egui::Stroke::new(0.7, egui::Color32::from_rgb(70, 80, 95)),
                                    );
                                    if swatch_resp
                                        .on_hover_text("Click to pick an ACI color")
                                        .clicked()
                                    {
                                        pick_layer_color = Some(id);
                                    }

                                    // ----- linetype combo (graphical) -----
                                    // Each row renders a SAMPLE of the pattern
                                    // (line / dashes / dots) next to its name.
                                    // Selecting one updates the layer's default
                                    // linetype; dobjects on this layer pick it
                                    // up via ByLayer in paint_dobject_with_style.
                                    let cur_lt = layer.linetype;
                                    if let Some(picked) = graphical_linetype_combo(
                                        ui, ("layer_lt", id), cur_lt, &lt_entries)
                                    {
                                        linetype_change.push((id, picked));
                                    }

                                    // ----- name (click to rename) ---------
                                    if self.layer_rename == Some(id) {
                                        let resp = ui.text_edit_singleline(&mut self.layer_rename_buf);
                                        // First frame after rename activation —
                                        // steal focus from the always-listen
                                        // command line so keystrokes land here.
                                        if self.layer_rename_focus_pending {
                                            resp.request_focus();
                                            self.layer_rename_focus_pending = false;
                                        }
                                        // Commit on ANY focus loss (Enter, or
                                        // clicking Add / another layer), not just
                                        // Enter — otherwise a typed name is
                                        // silently discarded and the layer falls
                                        // back to its default name. Escape cancels.
                                        if resp.lost_focus() {
                                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                                rename_cancel = true;
                                            } else {
                                                rename_commit = Some((id, self.layer_rename_buf.clone()));
                                            }
                                        }
                                    } else {
                                        let mut label = layer.name.clone();
                                        if id == LayerTable::LAYER_ZERO {
                                            label.push_str("  (reserved)");
                                        }
                                        let resp = ui.selectable_label(false, label);
                                        if resp.double_clicked() && id != LayerTable::LAYER_ZERO {
                                            self.layer_rename = Some(id);
                                            self.layer_rename_focus_pending = true;
                                            self.layer_rename_buf = layer.name.clone();
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                    });

                // Apply deferred mutations (made outside the borrow chain).
                if let Some(id) = new_active {
                    self.doc.layers.active = id;
                    self.history.push(format!("  active layer → #{}", id));
                }
                if let Some((id, on)) = lock_change {
                    self.set_layer_locked(id, on);
                }
                // Color edits captured during the layer loop — intern into
                // the truecolor table, then assign the ref. Two stages
                // because we couldn't borrow `doc.truecolors` mutably
                // inside the &mut layer scope.
                for (id, packed_rgb) in color_change.drain(..) {
                    let idx = self.doc.truecolors.intern(packed_rgb);
                    if let Some(l) = self.doc.layers.get_mut(id) {
                        l.color = Color::TrueColorRef(idx);
                    }
                }
                // Apply linetype changes (deferred to escape borrow chain).
                for (id, lt_id) in linetype_change.drain(..) {
                    if let Some(l) = self.doc.layers.get_mut(id) {
                        let lt_name = self.doc.linetypes.get(lt_id)
                            .map(|lt| lt.name.clone()).unwrap_or_default();
                        l.linetype = lt_id;
                        self.history.push(format!(
                            "  layer '{}' linetype → '{}'", l.name, lt_name));
                        self.touch_view();
                    }
                }
                if let Some((id, new_name)) = rename_commit {
                    let trimmed = new_name.trim().to_string();
                    if !trimmed.is_empty() && self.doc.layers.rename(id, &trimmed) {
                        self.history.push(format!("  layer #{} renamed → '{}'", id, trimmed));
                    } else {
                        self.history.push(format!(
                            "  ! rename failed (empty or duplicate)"
                        ));
                    }
                    // Only close the editor if we're still editing THIS layer —
                    // the user may have clicked straight onto another layer's
                    // name in the same frame, which already armed its rename.
                    if self.layer_rename == Some(id) {
                        self.layer_rename = None;
                        self.layer_rename_buf.clear();
                    }
                }
                if rename_cancel {
                    self.layer_rename = None;
                    self.layer_rename_buf.clear();
                }
                if let Some(id) = pick_layer_color {
                    self.aci_pick_request = Some(AciPickRequest::Layer(id));
                }
            });
        self.raise_after_show("Layers", ctx, &resp);
        self.process_dock_after_show("Layers", ctx, resp);
        self.layers_window_open = open;
    }
}
