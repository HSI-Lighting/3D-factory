use super::*;

// ============ the left mode command panel (2D | SIMLUX | 3D Factory) ============
// The per-workspace command panels + the room-details modal they launch.
// Child module of `app`; entry methods are `pub(super)` so the shell and
// tests can reach them.

impl CadApp {
    // ---- THE MODE COMMAND PANEL -----------------------------------------
    // The left-hand command list of the ACTIVE workspace. One panel id serves
    // all three modes; the body switches with `self.mode`. Rows dispatch
    // exactly like the rest of the UI: 2D draw/modify rows go through the
    // command registry (`execute`, so remembered construction methods and the
    // rail/palette highlight all stay in sync), the SIMLUX and 3D Factory rows
    // call the same handlers the viewports' own toolbars call.

    /// Header band of the panel: mode name + one-line description.
    fn mode_panel_header(&mut self, ui: &mut egui::Ui) {
        use crate::theme::color as tc;
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "{}  {} — commands",
                    self.mode.glyph(),
                    self.mode.label()
                ))
                .size(13.0)
                .strong()
                .color(tc::ACCENT),
            );
        });
        ui.label(
            egui::RichText::new(self.mode.blurb())
                .size(10.5)
                .color(tc::TEXT_MUTED),
        );
        ui.add_space(2.0);
        ui.separator();
    }

    /// Section heading inside the mode panel ("DRAW", "MODIFY", …) with a
    /// collapse chevron: click the heading to fold/unfold the section's rows.
    /// Returns `true` when the section's content should be drawn. A section's
    /// state is keyed by `id` and shared across the workspaces.
    ///
    /// The heading is a full-width BAND (background fill + top/bottom hairline)
    /// so a category reads at a glance against the row list below it.
    fn mode_section(&mut self, ui: &mut egui::Ui, id: &str, name: &str, tip: &str) -> bool {
        let open = !self.mode_sections_closed.contains(id);
        ui.add_space(3.0);
        let band = egui::Color32::from_rgb(38, 47, 58); // #262F3A
        let band_hi = egui::Color32::from_rgb(46, 57, 70); // hovered
        let hair = egui::Color32::from_rgb(28, 35, 44);
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, if resp.hovered() { band_hi } else { band });
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top() + 1.0),
            ),
            0.0,
            hair,
        );
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::pos2(rect.right(), rect.bottom()),
            ),
            0.0,
            hair,
        );
        // A 3px accent tick on the left marks the band as a category header.
        let tick = if open {
            egui::Color32::from_rgb(0, 200, 235)
        } else {
            egui::Color32::from_rgb(60, 90, 105)
        };
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + 2.0),
                egui::pos2(rect.left() + 3.0, rect.bottom() - 2.0),
            ),
            0.0,
            tick,
        );
        let ink = egui::Color32::from_rgb(178, 194, 214);
        let cy = rect.center().y;
        p.text(
            egui::pos2(rect.left() + 10.0, cy),
            egui::Align2::LEFT_CENTER,
            if open { "▾" } else { "▸" },
            egui::FontId::proportional(10.0),
            egui::Color32::from_rgb(120, 145, 170),
        );
        p.text(
            egui::pos2(rect.left() + 22.0, cy),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(10.5),
            ink,
        );
        let resp = resp.on_hover_text(tip);
        if resp.clicked() {
            if open {
                self.mode_sections_closed.insert(id.to_string());
            } else {
                self.mode_sections_closed.remove(id);
            }
            return !open;
        }
        ui.add_space(1.0);
        open
    }

    /// One clickable row: an emoji/text glyph, the name, and an optional
    /// trailing key hint (shown muted). `on` = highlighted state (a toggle row
    /// that is currently true, or an active command). Returns true when the
    /// row is clicked.
    fn mode_text_row(
        ui: &mut egui::Ui,
        glyph: &str,
        name: &str,
        tip: &str,
        key_hint: Option<&str>,
        on: bool,
    ) -> bool {
        use crate::theme::color as tc;
        let row_h = 25.0;
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_h),
            egui::Sense::click(),
        );
        let p = ui.painter_at(rect);
        let bg = if resp.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else if on {
            egui::Color32::from_rgba_unmultiplied(0x00, 0x88, 0x99, 46)
        } else {
            egui::Color32::TRANSPARENT
        };
        if bg != egui::Color32::TRANSPARENT {
            p.rect_filled(rect, egui::Rounding::ZERO, bg);
        }
        let ink = if on {
            tc::ACCENT
        } else {
            ui.visuals().text_color()
        };
        let gx = rect.left() + 5.0;
        p.text(
            egui::pos2(gx, rect.center().y),
            egui::Align2::LEFT_CENTER,
            glyph,
            egui::FontId::proportional(14.0),
            if on {
                tc::ACCENT
            } else {
                egui::Color32::from_rgb(160, 178, 196)
            },
        );
        p.text(
            egui::pos2(gx + 22.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(13.0),
            ink,
        );
        if let Some(k) = key_hint {
            p.text(
                egui::pos2(rect.right() - 6.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                k,
                egui::FontId::proportional(11.0),
                tc::TEXT_MUTED,
            );
        }
        resp.on_hover_text(tip).clicked()
    }

    /// One 2D draw/modify command row from the registry, with the command's
    /// own vector glyph painted in the same style the toolbars used.
    fn mode_registry_row(&mut self, ui: &mut egui::Ui, id: &crate::command::CommandId) {
        use crate::theme::color as tc;
        let Some(info) = self.command_registry.get(id).cloned() else {
            return;
        };
        let row_h = 25.0;
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_h),
            egui::Sense::click(),
        );
        let p = ui.painter_at(rect);
        let active = self.rail_active == info.dispatch;
        let bg = if resp.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else if active {
            egui::Color32::from_rgba_unmultiplied(0x00, 0x88, 0x99, 46)
        } else {
            egui::Color32::TRANSPARENT
        };
        if bg != egui::Color32::TRANSPARENT {
            p.rect_filled(rect, egui::Rounding::ZERO, bg);
        }
        // The command's glyph, at the same box and tone the toolbar used.
        let c = egui::pos2(rect.left() + 12.0, rect.center().y);
        let pen = egui::Stroke::new(
            1.2,
            if active {
                tc::ACCENT
            } else {
                egui::Color32::from_rgb(0x9f, 0xb2, 0xc3)
            },
        );
        let dot = |q: egui::Pos2| {
            p.circle_filled(q, 1.5, pen.color);
        };
        match info.icon {
            crate::command::IconId::DrawGlyph(s) => {
                draw_draw_glyph(&p, c, s, pen, dot, pen.color, 1.0)
            }
            crate::command::IconId::ModifyGlyph(k) => {
                draw_cmd_glyph(&p, c, k, pen, dot, pen.color, 1.0)
            }
        }
        // Name + key hint (the part of the rail tooltip after "(...").
        let name = info.tooltip.split("  (").next().unwrap_or(&info.tooltip);
        let hint = info.tooltip.find('(').map(|i| &info.tooltip[i..]);
        let ink = if active {
            tc::ACCENT
        } else {
            ui.visuals().text_color()
        };
        p.text(
            egui::pos2(rect.left() + 25.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(13.0),
            ink,
        );
        if let Some(h) = hint {
            p.text(
                egui::pos2(rect.right() - 6.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                h,
                egui::FontId::proportional(11.0),
                tc::TEXT_MUTED,
            );
        }
        let resp = resp.on_hover_text(info.tooltip.as_str());
        if resp.clicked() {
            self.rail_active = info.dispatch.to_string();
            self.execute(id);
        }
    }

    /// The whole panel. Rows are produced per mode; every click is handled
    /// inline (dispatch needs `&mut self`, so it cannot happen inside a shared
    /// iterator that borrows the registry).
    pub(super) fn render_mode_command_panel(&mut self, ctx: &egui::Context) {
        use crate::theme::color as tc;
        egui::SidePanel::left("mode_cmd_panel")
            .resizable(true)
            .default_width(242.0)
            .min_width(190.0)
            .max_width(380.0)
            .frame(egui::Frame::none().fill(tc::SURFACE_1))
            .show(ctx, |ui| {
                self.mode_panel_header(ui);
                egui::ScrollArea::vertical()
                    .id_salt("mode_cmd_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match self.mode {
                            Mode::Cad2D => self.mode_panel_2d(ui),
                            Mode::Simlux => self.mode_panel_simlux(ui),
                            Mode::Factory => self.mode_panel_factory(ui),
                        }
                    });
            });
    }

    /// 2D drafting workspace: the draw/modify registry (the same commands the
    /// DRAW/MODIFY rails carry, as a full list), the plane-sketch actions when
    /// a sketch is open, and the luminaire placement commands (they act on the
    /// plan, so they belong to this workspace).
    fn mode_panel_2d(&mut self, ui: &mut egui::Ui) {
        // A face-sketch takes over the whole window; its actions live HERE (the
        // Factory panel that used to carry them is hidden while the canvas
        // owns the window).
        if self.factory.session.is_some() {
            egui::Frame::none()
                .fill(egui::Color32::from_rgb(52, 38, 12))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 158, 30)))
                .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                .rounding(3.0)
                .show(ui, |ui| {
                    let editing_cutout = self.factory.editing_cutout();
                    ui.label(egui::RichText::new(
                        if editing_cutout { "✎ reshaping an opening" }
                        else { "✎ drafting on a plane" })
                        .color(egui::Color32::from_rgb(255, 178, 60)).strong());
                    if editing_cutout {
                        if ui.button(egui::RichText::new("✔ Apply reshape").strong())
                            .on_hover_text("Re-cut the opening with the edited outline — through if it went through, at its own depth if it was a recess — then show it in 3D")
                            .clicked()
                        {
                            self.factory_apply_cutout_reshape();
                        }
                        if ui.button("✖ Cancel")
                            .on_hover_text("Discard the reshape and leave the opening as it was")
                            .clicked()
                        {
                            self.factory_exit_sketch();
                        }
                    } else if ui.button(egui::RichText::new("✔ Finish sketch").strong()).clicked() {
                        self.factory_exit_sketch();
                    }
                    ui.label(egui::RichText::new(
                        "the canvas is this plane — full toolset below")
                        .small().weak());
                });
            // (The Finish/Apply buttons above are in the banner, which never
            // collapses — these rows are the extras.)
            if self.mode_section(
                ui,
                "2d-sketch",
                "SKETCH ACTIONS",
                "Turn the drawn shape into 3D geometry on this plane",
            ) {
                // Height/depth, shared by Extrude / Recess below.
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("height / depth").size(11.0).weak());
                    let u = self.factory.units.clone();
                    crate::factory::length_ui(
                        ui,
                        &u,
                        &mut self.factory.element_height,
                        0.05,
                        0.001,
                        1e4,
                        &self.calc,
                    );
                });
                if Self::mode_text_row(
                    ui,
                    "⬆",
                    "Extrude",
                    "Extrude the drawn closed shape into a solid element on this face",
                    None,
                    false,
                ) {
                    self.factory_extrude_sketch(false);
                }
                if Self::mode_text_row(
                    ui,
                    "◳",
                    "Cut through",
                    "Cut the drawn shape ALL THE WAY THROUGH the wall/solid (window, door)",
                    None,
                    false,
                ) {
                    self.factory_cut_sketch(true);
                }
                if Self::mode_text_row(ui, "◱", "Recess", "Cut a blind pocket of the given DEPTH into the solid (niche, reveal) — not through", None, false) {
                self.factory_cut_sketch(false);
            }
                if Self::mode_text_row(ui, "🪑", "As furniture", "Extrude the shape as a free-standing furniture solid — never cuts the building", None, false) {
                self.factory_extrude_sketch(true);
            }
                if Self::mode_text_row(ui, "〰", "Path extrude", "Draw the cross-section on a face → Enter → pick a perpendicular view → draw the path → Enter. Sweeps the section along the path.", None, false) {
                self.factory_begin_sweep_flow(false, false);
            }
                if Self::mode_text_row(ui, "〰", "Path cut", "Draw the cross-section on a face → Enter → pick a perpendicular view → draw the path → Enter. Cuts a channel along the path.", None, false) {
                self.factory_begin_sweep_flow(true, false);
            }
                ui.checkbox(&mut self.factory.keep_sketch, "keep shape")
                .on_hover_text("Keep the drawing after Extrude/Cut so you can act on the SAME outline again (e.g. recess + through)");
                ui.checkbox(&mut self.factory.show_other_planes, "other planes")
                .on_hover_text("Show what is drawn on the OTHER planes, projected onto this one. Off by default: a face sketch is its own drawing.");
            }
            ui.separator();
        }

        // DRAW — the registry rows, in the user's rail order.
        if self.mode_section(ui, "2d-draw", "DRAW", "Draw commands — click to run") {
            let items = self.draw_items.clone();
            for id in &items {
                self.mode_registry_row(ui, id);
            }
            ui.add_space(2.0);
        }

        // MODIFY
        if self.mode_section(ui, "2d-modify", "MODIFY", "Modify commands — click to run") {
            let items = self.modify_items.clone();
            for id in &items {
                self.mode_registry_row(ui, id);
            }
            ui.add_space(2.0);
        }

        // LIGHT PLACEMENT — these act on the plan, so they live with the plan.
        if self.mode_section(ui, "2d-light", "LIGHT PLACEMENT",
            "Lighting commands that act on the plan — the SIMLUX tab is where the results are viewed in 3D")
        {
        // What the NEXT click would drop — the right-hand hint names the
        // active fitting, and the tooltip says how to change it.
        let fitting_name = if self.light.profiles.contains_key(&self.light.active_profile) {
            self.light.active_profile.clone()
        } else {
            String::new()
        };
        let tip_owned = if fitting_name.is_empty() {
            "Click the plan to mark each light position. NO fitting is picked yet — what you \
             drop here will need one before it emits (open the Light panel row below and pick \
             a fitting).\nDrag a marker to move it · Esc to stop."
                .to_string()
        } else {
            format!(
                "Click the plan to drop a point with the fitting '{fitting_name}'. Drag a \
                 marker to move it · Esc to stop.\nTo change what is placed (fitting, mount \
                 height): the Light panel row below, or 💡 Illuminaire."
            )
        };
        let hint: Option<&str> = if fitting_name.is_empty() {
            Some("no fitting — edit row below")
        } else {
            Some(&fitting_name)
        };
        if Self::mode_text_row(ui, "＋", "Place luminaire",
            &tip_owned, hint, self.light.place_mode)
        {
            self.light.place_mode = !self.light.place_mode;
            if self.light.place_mode {
                self.light.aim_mode = false;
                self.light.aim_pick = None;
                if !self.light.profiles.contains_key(&self.light.active_profile) {
                    // A point dropped with no fitting emits nothing and the plan gives no
                    // hint why — default to the built-in profile so placement always places
                    // something real, and say which.
                    self.light.active_profile = crate::light::BUILTIN.to_string();
                }
                let what = self.light.active_profile.clone();
                let h = self.light.mount_height;
                self.light.last_msg = format!(
                    "Placing {what} · mounted at {h:.2} m — click the plan · Esc stops. \
                     Fitting + height change in the Light panel."
                );
            }
        }
        // The Light panel IS the editor for what was placed: the fixture list (select a row
        // to find that light on the plan), dimming, mount height, the fitting a point uses,
        // and the rooms. It used to be unreachable from the 2D workspace — the only way to
        // change a placed light's properties was a menu the workspace never surfaced.
        if Self::mode_text_row(ui, "🎛", "Light panel (edit placed lights…)",
            "The editor for your lights: click a fixture row to select it on the plan; dim, \
             mount height, the fitting it uses and the rooms are all edited here.",
            None, self.light.window_open)
        {
            self.light.window_open = !self.light.window_open;
            if self.light.window_open {
                self.light.last_msg =
                    "Light panel — click a fixture row to find that light on the plan. \
                     Dim, height and fitting are edited here.".into();
            }
        }
        if Self::mode_text_row(ui, "⌖", "Aim a light at a point",
            "Click a fitting, then click the point it should light — it stays exactly where it \
             is, only the direction changes",
            None, self.light.aim_mode)
        {
            self.light.aim_mode = !self.light.aim_mode;
            self.light.aim_pick = None;
            if self.light.aim_mode {
                self.light.place_mode = false;
                self.light.last_msg =
                    "Aim: click a fitting, then click the point it should light.".into();
            }
        }
        if Self::mode_text_row(ui, "💡", "Illuminaire (fittings)",
            "Your library of fittings — a 2D block paired with a photometric file. Place one and the drawing gets an ordinary block; SIMLUX gets a luminaire.",
            None, self.light.illuminaire_open)
        {
            self.light.illuminaire_open = !self.light.illuminaire_open;
            if self.light.illuminaire_open {
                self.light.lib_scanned =
                    crate::illuminaire::scan_folder(&self.light.lib_folder);
            }
        }
        if Self::mode_text_row(ui, "▦", "Lux overlay on plan",
            "Paint the computed lux grid on the plan as a false-colour overlay (2D)",
            None, self.light.show_overlay)
        {
            self.light.show_overlay = !self.light.show_overlay;
        }
        }

        // ROOMS — ONE list of every room in the project (built 3D rooms ⌂,
        // plan-made rooms, imported layer rooms ⬚; see `factory.rooms`). Every
        // room is a lux calc target — each footprint gets its own grid, Ē and
        // U₀ — and rooms made here are REAL 3D rooms at once, visible in the
        // 3D Factory view.
        if self.mode_section(ui, "2d-rooms", "ROOMS",
            "Every room here gets its own lux result and exists in the 3D Factory view. Select a              CLOSED outline on the plan and make it a room — it asks for the room's details,              then builds it at once.")
        {
            let n = self.factory.rooms.len();
            let count_hint: Option<String> = if n > 0 {
                Some(format!("{n} room(s)"))
            } else {
                None
            };
            if Self::mode_text_row(ui, "⌂", "Make room (from selected outline)",
                "Select a CLOSED outline (drawn walls/room perimeter) on the plan, then click this.                  You're asked the room's details — name, start height, clear height and slab                  thicknesses — then it is BUILT at once: walls, floor and ceiling solids in the                  3D Factory view, and a lux calculation room with its own grid from the start.",
                count_hint.as_deref(),
                false)
            {
                self.request_make_room();
            }
            // The rooms — one row each: badge, name, height, [⬆ build when
            // unbuilt] and ✕. Height edits the record (a built room's walls
            // follow; an unbuilt one just remembers it).
            let rooms = self.factory.rooms.clone();
            let mut remove: Option<u32> = None;
            let mut build: Option<u32> = None;
            for r in &rooms {
                let name = r.name.clone();
                let origin = r.origin;
                let pts = r.footprint.len();
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    let badge = ui.add(
                        egui::Label::new(egui::RichText::new(origin.glyph()).size(11.0))
                            .sense(egui::Sense::hover()),
                    )
                    .on_hover_text(origin.label());
                    let _ = badge;
                    ui.add(
                        egui::Label::new(egui::RichText::new(&name).size(12.5))
                            .sense(egui::Sense::hover()),
                    )
                    .on_hover_text(format!(
                        "{} · {} · closed outline with {pts} corners (metres)",
                        origin.label(),
                        if origin == crate::factory::RoomOrigin::ImportedLayer {
                            r.layer_name.clone().unwrap_or_default()
                        } else {
                            "footprint captured at designation; the drawing stays editable".into()
                        }
                    ));
                    if !r.is_built() {
                        let b = ui.add(
                            egui::Button::new(egui::RichText::new("⬆").size(11.0))
                                .frame(false)
                                .min_size(egui::vec2(18.0, 16.0)),
                        )
                        .on_hover_text("Build this room into real 3D solids (walls, floor, ceiling) from its footprint");
                        if b.clicked() {
                            build = Some(r.id);
                        }
                    }
                    let x = ui.add(
                        egui::Button::new(egui::RichText::new("✕").size(10.0))
                            .frame(false)
                            .min_size(egui::vec2(14.0, 16.0)),
                    )
                    .on_hover_text("Remove this room — the plan drawing itself is untouched");
                    if x.clicked() {
                        remove = Some(r.id);
                    }
                });
            }
            if let Some(id) = remove {
                let name = self
                    .factory
                    .rooms
                    .iter()
                    .find(|r| r.id == id)
                    .map(|r| r.name.clone())
                    .unwrap_or_default();
                self.factory.delete_room(id);
                self.history.push(format!(
                    "  room '{name}' removed — the drawing is untouched"
                ));
            }
            if let Some(id) = build {
                self.snapshot_factory();
                match self.factory.build_designated_room(id) {
                    Ok(new_id) => {
                        let name = self
                            .factory
                            .rooms
                            .iter()
                            .find(|r| r.id == new_id)
                            .map(|r| r.name.clone())
                            .unwrap_or_default();
                        self.history.push(format!(
                            "  room '{name}' built — its footprint is now walls, floor and ceiling.                              See it in the 3D Factory view."
                        ));
                    }
                    Err(e) => {
                        self.undo_stack.pop();
                        let why = room_error_why(e);
                        self.history.push(format!("  ! room: {why}"));
                        self.light.last_msg = why;
                    }
                }
            }
            if n == 0 {
                ui.add_space(2.0);
                ui.label(egui::RichText::new("no rooms yet — draw a closed outline, select it and                     make it a room above (it appears in the 3D Factory view at once)")
                    .size(11.0).weak());
            }
        }
        ui.add_space(2.0);
    }

    /// "Make room (from selected outline)" row: validate the selection, then
    /// ASK for the room's details instead of building instantly with defaults.
    /// The outline stays in [`Self::room_form`] (it belongs to the 2D plan)
    /// until the user confirms — one undo step — or cancels.
    pub(super) fn request_make_room(&mut self) {
        if self.refuse_plan_action_in_sketch("Make room") {
            return;
        }
        let Some(outline) = self.slab_outline_from_selection() else {
            let msg = "select a CLOSED outline first, then make it a room";
            self.history.push(format!("  ! room: {msg}"));
            self.light.last_msg = msg.into();
            return;
        };
        let f = &self.factory;
        self.room_form = Some(RoomForm {
            outline,
            name: format!("Room {}", f.next_room_id),
            base_z: f.active_base_z(),
            height: f.room_height.max(0.05),
            floor_t: f.room_floor.max(0.02),
            wall_t: f.wall_thickness.max(0.02),
            ceiling_t: f.ceiling_thickness.max(0.02),
        });
        self.room_form_focus_name = true;
    }

    /// The modal the row opened — the room's details (name, start height,
    /// clear height, slab/wall thicknesses) with the settings as defaults.
    ///
    /// A modal backed like the close-confirm dialog: dimmed Order::Middle
    /// backdrop, Foreground window, Esc answers it (the global Esc handler
    /// clears the form and stops — see `update`), and any mode change away
    /// from the drafting workspace abandons the question unanswered.
    pub(super) fn render_room_form(&mut self, ctx: &egui::Context) {
        if self.room_form.is_none() {
            return;
        }
        // The question belongs to the plan; a workspace change (only possible
        // programmatically — the backdrop swallows tab clicks) ends it.
        if self.mode != Mode::Cad2D {
            self.room_form = None;
            self.room_form_focus_name = false;
            return;
        }
        let mut f = self.room_form.take().expect("checked above");
        let u = self.factory.units.clone();
        let calc = &self.calc;
        let mut focus_name = self.room_form_focus_name;
        let mut make = false;
        let mut cancel = false;

        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("room_form_backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
                ui.allocate_rect(screen, egui::Sense::click_and_drag());
            });

        egui::Window::new("Make room")
            .id(egui::Id::new("room_form"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(egui::Frame::window(&ctx.style()))
            .show(ctx, |ui| {
                ui.set_width(370.0);
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "The selected outline becomes a real 3D room — walls, floor and \
                         ceiling solids in the 3D Factory view — and a lux room with its \
                         own grid from the start.",
                    )
                    .weak(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [96.0, 18.0],
                        egui::Label::new(egui::RichText::new("name").small().weak()),
                    );
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut f.name)
                            .desired_width(170.0)
                            .hint_text("room name"),
                    );
                    if focus_name {
                        resp.request_focus();
                        focus_name = false;
                    }
                });
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [96.0, 18.0],
                        egui::Label::new(egui::RichText::new("start height").small().weak()),
                    );
                    crate::factory::length_ui(ui, &u, &mut f.base_z, 0.5, -1e5, 1e5, calc)
                        .on_hover_text(
                            "The Z the room STANDS on — the bottom of the floor slab. \
                             Defaults to the current storey's base; raise it to start the \
                             room above the ground.",
                        );
                });
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [96.0, 18.0],
                        egui::Label::new(egui::RichText::new("clear height").small().weak()),
                    );
                    crate::factory::length_ui(ui, &u, &mut f.height, 0.02, 0.3, 30.0, calc)
                        .on_hover_text(
                            "CLEAR height — floor top to ceiling underside. The floor and \
                             ceiling slabs are extra, so the structure is always this much \
                             taller than the number here.",
                        );
                });
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [96.0, 18.0],
                        egui::Label::new(egui::RichText::new("wall").small().weak()),
                    );
                    crate::factory::length_ui(ui, &u, &mut f.wall_t, 0.01, 0.02, 2.0, calc)
                        .on_hover_text(
                            "The room's OWN perimeter walls — how thick the ring of walls \
                             around the outline is built (a room carved out of a solid \
                             building keeps the building's material instead).",
                        );
                });
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [96.0, 18.0],
                        egui::Label::new(egui::RichText::new("floor").small().weak()),
                    );
                    crate::factory::length_ui(ui, &u, &mut f.floor_t, 0.01, 0.02, 2.0, calc)
                        .on_hover_text("Slab below the room");
                });
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [96.0, 18.0],
                        egui::Label::new(egui::RichText::new("ceiling").small().weak()),
                    );
                    crate::factory::length_ui(ui, &u, &mut f.ceiling_t, 0.01, 0.02, 2.0, calc)
                        .on_hover_text("Slab above the room");
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(egui::RichText::new("Make room").strong()))
                        .clicked()
                    {
                        make = true;
                    }
                    if ui.add(egui::Button::new("Cancel")).clicked() {
                        cancel = true;
                    }
                });
                ui.add_space(4.0);
            });
        self.room_form_focus_name = focus_name;
        if cancel {
            // Nothing was asked for and nothing was built.
            self.room_form_focus_name = false;
            return;
        }
        if !make {
            self.room_form = Some(f); // still editing — keep the entries
            return;
        }
        self.room_form_focus_name = false;
        self.build_room_form(f);
    }

    /// Build the room the form described — the confirmed answer is the
    /// parameter set the room is constructed with (`FactoryState::add_room_spec`),
    /// renamed to whatever was asked, all as ONE undoable step.
    pub(super) fn build_room_form(&mut self, f: RoomForm) {
        if self.refuse_plan_action_in_sketch("Make room") {
            return;
        }
        self.snapshot_factory();
        let spec = crate::factory::RoomBuildSpec {
            base_z: f.base_z,
            floor_t: f.floor_t,
            clear_h: f.height,
            wall_t: f.wall_t,
            ceiling_t: f.ceiling_t,
            open_top: self.factory.room_open_top,
        };
        match self.factory.add_room_spec(&f.outline, spec) {
            Ok(_floor) => {
                // `add_room_spec` pushed the room record last — give it the name
                // that was asked for (empty falls back to "Room N").
                let built_name = if let Some(r) = self.factory.rooms.last_mut() {
                    let n = f.name.trim();
                    if n.is_empty() {
                        format!("Room {}", r.id)
                    } else {
                        r.name = n.to_string();
                        r.name.clone()
                    }
                } else {
                    String::from("Room")
                };
                self.history.push(format!(
                    "  room '{built_name}' made from the plan outline — built in the 3D Factory view and              a lux room with its own grid. Select it on the plan or build another."
                ));
                self.light.last_msg = format!(
                    "'{built_name}' built from the selected outline — see it in the 3D Factory view; ⚡              Calculate reports it as its own room"
                );
            }
            Err(e) => {
                self.undo_stack.pop(); // nothing was created
                let why = room_error_why(e);
                self.history.push(format!("  ! room: {why}"));
                self.light.last_msg = why;
            }
        }
    }

    /// Make a REAL 3D room from a closed outline with the factory's CURRENT
    /// defaults (no questions) — the tests' and programmatic path. The panel
    /// row ([`Self::request_make_room`]) asks for the details first.
    pub(super) fn make_room_from_selected_outline(&mut self, outline: Vec<glam::Vec2>) {
        let f = &self.factory;
        let form = RoomForm {
            outline,
            name: String::new(),
            base_z: f.active_base_z(),
            height: f.room_height.max(0.05),
            floor_t: f.room_floor.max(0.02),
            wall_t: f.wall_thickness.max(0.02),
            ceiling_t: f.ceiling_thickness.max(0.02),
        };
        self.build_room_form(form);
    }

    /// SIMLUX workspace (3D lighting viewport): calculation, results and 3D
    /// display commands. Placing luminaires happens on the plan → the 2D tab.
    fn mode_panel_simlux(&mut self, ui: &mut egui::Ui) {
        if self.mode_section(
            ui,
            "simlux-calc",
            "CALCULATION",
            "Run and collect the lighting calculation",
        ) {
            if Self::mode_text_row(
                ui,
                "⚡",
                "Calculate lux",
                "Run the lux calculation over every room (Express/Thorough per the Light panel)",
                None,
                false,
            ) {
                self.light.window_open = true;
                self.start_calculation();
            }
            if Self::mode_text_row(
                ui,
                "🪟",
                "Light panel  (settings + results)",
                "Show the Light panel — calculation settings, room list and the results tables",
                None,
                self.light.window_open,
            ) {
                self.light.window_open = !self.light.window_open;
            }
            if Self::mode_text_row(ui, "📂", "Import light file…",
            "IES (.ies) or EULUMDAT (.ldt) from the manufacturer — the same picker furniture uses",
            None, false)
        {
            self.open_file_dialog(FileDialogMode::ImportIes, ".ies");
        }
            if Self::mode_text_row(
                ui,
                "⤓",
                "Export report",
                "Build the lighting report (schedule, calculations, false-colour sheets)",
                None,
                false,
            ) {
                self.export_light_report();
            }
        }

        if self.mode_section(
            ui,
            "simlux-display",
            "3D DISPLAY",
            "How the result is drawn in the 3D viewport",
        ) {
            if Self::mode_text_row(
                ui,
                "▦",
                "Heatmap floor",
                "Colour the floor with the computed illuminance (false colour)",
                None,
                self.light.floor_heatmap,
            ) {
                self.light.floor_heatmap = !self.light.floor_heatmap;
            }
            if Self::mode_text_row(
                ui,
                "〰",
                "Isolux lines",
                "Draw equal-illuminance contour lines through the room",
                None,
                self.light.show_isolux,
            ) {
                self.light.show_isolux = !self.light.show_isolux;
            }
        }
        ui.add_space(2.0);
    }

    /// 3D Factory workspace: solids, building, room, furniture, storeys and
    /// view commands — each row dispatches to the same handler the 3D Factory
    /// panel's own toolbar calls.
    fn mode_panel_factory(&mut self, ui: &mut egui::Ui) {
        if self.mode_section(
            ui,
            "factory-solids",
            "3D SOLIDS",
            "Parametric primitives — each opens the Draw3D dialog; nothing is built until Create",
        ) {
            for k in crate::factory::Draw3dKind::ALL {
                if Self::mode_text_row(
                    ui,
                    k.icon(),
                    k.label(),
                    format!(
                        "Create a parametric {} — opens the Draw3D dialog",
                        k.label()
                    )
                    .as_str(),
                    None,
                    false,
                ) {
                    self.factory.draw3d = Some(crate::factory::Draw3dDialog::new(k));
                }
            }
        }

        if self.mode_section(
            ui,
            "factory-building",
            "BUILDING",
            "Turn the selected 2D geometry into 3D — walls, a building mass, floors and ceilings",
        ) {
            if Self::mode_text_row(
                ui,
                "⌂",
                "Make building (from outline)",
                "Extrude the selected closed outline(s) into a building mass",
                None,
                false,
            ) {
                self.do_make_building();
            }
            if Self::mode_text_row(
                ui,
                "⬒",
                "Make 3D walls",
                "Promote the selected 2D geometry to 3D walls",
                None,
                false,
            ) {
                self.do_make_3d_wall();
            }
            if Self::mode_text_row(
                ui,
                "⬓",
                "Make floor",
                "Promote the selection to a floor slab",
                None,
                false,
            ) {
                self.do_make_slab(true);
            }
            if Self::mode_text_row(
                ui,
                "⬒",
                "Make ceiling",
                "Promote the selection to a ceiling slab",
                None,
                false,
            ) {
                self.do_make_slab(false);
            }
            if Self::mode_text_row(
                ui,
                "⬚",
                "Make room (from outline)",
                "Turn the selected closed outline into a room with walls, floor and ceiling",
                None,
                false,
            ) {
                self.do_make_room();
            }
        }

        if self.mode_section(
            ui,
            "factory-furniture",
            "FURNITURE",
            "Import furniture, or extrude/sweep a drawn shape into a piece",
        ) {
            if Self::mode_text_row(
                ui,
                "⭳",
                "Import OBJ / 3DS / FBX / glTF…",
                "Import a furniture mesh file (.obj, .3ds, .fbx, .glb, .gltf)",
                None,
                false,
            ) {
                self.open_file_dialog(FileDialogMode::ImportObj, ".glb");
            }
            if Self::mode_text_row(ui, "⬆", "Extrude drawn shape",
            "Right-click a face → Draw on this face, draw a shape, then this extrudes it into a free-standing furniture solid (never cuts the building)",
            None, false)
        {
            self.factory_extrude_sketch(true);
        }
        }

        if self.mode_section(ui, "factory-storeys", "STOREYS",
            "The building's levels — new geometry is placed on the active storey (see the 3D Factory panel for the full list)")
        {
        if Self::mode_text_row(ui, "＋", "Storey on top",
            "Add an empty storey on top of the building and make it active", None, false)
        {
            self.snapshot_factory();
            let i = self.factory.add_storey_on_top();
            self.history.push(format!(
                "  storey '{}' added (base {:.2} m)",
                self.factory.storeys[i].name,
                self.factory.storey_base_z(i)
            ));
        }
        }

        if self.mode_section(ui, "factory-view", "VIEW", "Camera and display") {
            let n_ceil = self.factory.ceilings.len();
            if Self::mode_text_row(
                ui,
                "▦",
                "Plan underlay",
                "Show the 2D drawing on the ground as a reference underlay",
                None,
                self.factory.show_plan,
            ) {
                self.factory.show_plan = !self.factory.show_plan;
            }
            let xray_enabled = self.factory.show_plan;
            let xray_on = xray_enabled && self.factory.plan_xray;
            if Self::mode_text_row(
                ui,
                "◫",
                "Plan X-ray",
                "Draw the plan THROUGH the model (needs the plan underlay on)",
                None,
                xray_on,
            ) {
                if xray_enabled {
                    self.factory.plan_xray = !self.factory.plan_xray;
                }
            }
            if Self::mode_text_row(
                ui,
                "▤",
                "Hide ceilings",
                "Hide room/ceiling slabs so you can see inside (view only)",
                Some(&format!("({n_ceil})")),
                self.factory.hide_ceilings,
            ) {
                self.factory.hide_ceilings = !self.factory.hide_ceilings;
                self.factory.dirty = true;
                self.factory.recompute();
                if n_ceil == 0 {
                    self.factory.status =
                        "no ceilings to hide — try 'Cutaway' to see inside anything".into();
                }
            }
            let gmode = self.factory.gizmo_mode;
            if Self::mode_text_row(
                ui,
                "↔",
                "Gizmo: move",
                "Drag the arms to move the selection",
                None,
                gmode == crate::factory::GizmoMode::Move,
            ) {
                self.factory.gizmo_mode = crate::factory::GizmoMode::Move;
            }
            if Self::mode_text_row(
                ui,
                "⟳",
                "Gizmo: rotate",
                "Drag a ring to rotate the selection about that axis (X red · Y green · Z blue)",
                None,
                gmode == crate::factory::GizmoMode::Rotate,
            ) {
                self.factory.gizmo_mode = crate::factory::GizmoMode::Rotate;
            }
            if Self::mode_text_row(
                ui,
                "⌖",
                "Frame model",
                "Zoom the camera to fit the whole model",
                None,
                false,
            ) {
                if self.factory.dirty {
                    self.factory.recompute();
                }
                self.factory.fit();
            }
            if Self::mode_text_row(
                ui,
                "🗑",
                "Clear model",
                "Delete every solid, room, storey and sketch — the whole model",
                None,
                false,
            ) {
                self.snapshot_factory();
                self.factory.clear();
            }
            if Self::mode_text_row(ui, "🎨", "Materials Factory",
            "Blender-style shader graph: Texture → Principled BSDF → Output. Edits show live in the 3D Factory view.",
            None, self.materials_open)
        {
            self.materials_open = !self.materials_open;
        }
            if Self::mode_text_row(ui, "⏺", "Render  (path tracer)",
            "Raytraced render of the scene: global illumination, reflections, glass. Progressive; pick CPU or GPU.",
            None, self.render_modal_open)
        {
            self.render_modal_open = !self.render_modal_open;
        }
        }
        ui.add_space(2.0);
    }
}
