use super::*;

// ============ windows & dialogs: ACI picker .. fixture sync ============
// Floating windows, inspectors and dialogs that float over the three
// workspaces: ACI picker, pen palette, info/inspector panel + command
// palette, busy/close-confirm/sun dialogs, the Materials Factory window,
// the path-tracer/radiance render dialogs, Groups, and the light fixture
// sync/undo helpers. Child module of `app`; entry methods are pub(super)/
// pub(crate) so the shell, panel and tests can reach them.

impl CadApp {
    // ===================================================================
    // Floating ACI color picker — the polar AutoRasm wheel.
    // ===================================================================
    //
    // One shared window serves every call site. The active request
    // (`aci_pick_request`) names who asked; when the user clicks a slot
    // in pick mode, the resulting ACI is written back to that target
    // and the window closes. Swap mode lets the user tune the wheel
    // arrangement; "Save mapping" persists the permutation to
    // `~/workspace/RUST_CAD/aci_mapping.json`.
    //
    // Spec: ~/workspace/RUST_CAD/ACI_Picker_UI.html
    pub(super) fn render_aci_picker_window(&mut self, ctx: &egui::Context) {
        let Some(target) = self.aci_pick_request else {
            return;
        };

        // Title — use the actual layer / dobject name so the user knows
        // which slot they're editing without having to remember its id.
        let title = match target {
            AciPickRequest::Layer(id) => {
                let name = self
                    .doc
                    .layers
                    .get(id)
                    .map(|l| l.name.clone())
                    .unwrap_or_else(|| format!("layer #{}", id));
                format!("ACI color — {}", name)
            }
            AciPickRequest::Dobject(ix) => {
                let kind = self
                    .doc
                    .dobjects
                    .get(ix)
                    .map(|d| dobject_kind_name(&d.geom).to_string())
                    .unwrap_or_else(|| "dobject".to_string());
                format!("ACI color — {} #{}", kind, ix)
            }
            AciPickRequest::DobjectMany => {
                format!("ACI color — {} dobjects", self.aci_pick_many.len())
            }
            AciPickRequest::DimStyleForm(slot) => {
                let which = match slot {
                    DimColorSlot::DimLine => "dim line",
                    DimColorSlot::ExtLine => "extension lines",
                    DimColorSlot::Text => "text",
                };
                format!("ACI color — dimension {}", which)
            }
            AciPickRequest::WallStyleForm(slot) => match slot {
                WallColorSlot::Fill => "ACI color — wall fill".to_string(),
                WallColorSlot::Face => "ACI color — wall faces".to_string(),
            },
            AciPickRequest::BlockForm => "ACI color — block instance".to_string(),
            AciPickRequest::ScriptParam(ix) => {
                let name = self
                    .script_param_dialog
                    .as_ref()
                    .and_then(|d| d.params.get(ix))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| format!("script parameter #{}", ix));
                format!("ACI color — {}", name)
            }
            AciPickRequest::CurrentColor => "ACI color — current color".to_string(),
            AciPickRequest::PlotStyleColor => {
                format!(
                    "ACI color — plot color ({} styles)",
                    self.plotstyle_sel.len()
                )
            }
        };

        let mut open = true;
        let mut picked: Option<u8> = None;
        let mut do_save = false;
        // Reset the per-frame hover before rendering. Wheel + excluded
        // rows write to it as the cursor moves over them.
        self.aci_picker.hovered_aci = None;

        egui::Window::new(title)
            .id(egui::Id::new("aci_picker_window"))
            .open(&mut open)
            .default_size(egui::vec2(440.0, 640.0))
            .resizable(true)
            .collapsible(true)
            .show(ctx, |ui| {
                // Top control row — swap / reset / save.
                ui.horizontal(|ui| {
                    let swap_label = if self.aci_picker.swap_mode {
                        "Swap mode: ON"
                    } else { "Swap mode: OFF" };
                    if ui.selectable_label(self.aci_picker.swap_mode, swap_label)
                        .on_hover_text("Click two circles to swap their positions.\nApplies only to the main wheel.")
                        .clicked()
                    {
                        self.aci_picker.swap_mode = !self.aci_picker.swap_mode;
                    }
                    if ui.button("Reset layout").clicked() {
                        self.aci_picker.reset_to_default();
                    }
                    if ui.button("Save mapping").clicked() {
                        do_save = true;
                    }
                });

                ui.separator();

                // ---- Excluded row 1: named colors (ACI 1..=9) ----------
                ui.label("Named colors (ACI 1–9)");
                if let Some(aci) = self.aci_picker.excluded_row_ui(
                    ui, crate::aci_picker::EXCLUDED_NAMED.clone())
                {
                    picked = Some(aci);
                }
                ui.add_space(4.0);

                // ---- The wheel itself, centred horizontally ------------
                ui.vertical_centered(|ui| {
                    if let Some(aci) = self.aci_picker.wheel_ui(ui) {
                        picked = Some(aci);
                    }
                });

                ui.add_space(4.0);
                // ---- Excluded row 2: grayscale (ACI 250..=255) ---------
                ui.label("Grays (ACI 250–255)");
                if let Some(aci) = self.aci_picker.excluded_row_ui(
                    ui, crate::aci_picker::EXCLUDED_GRAY.clone())
                {
                    picked = Some(aci);
                }

                ui.separator();

                // ---- Manual ACI entry ----------------------------------
                ui.horizontal(|ui| {
                    ui.label("ACI #:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.aci_picker.manual_entry)
                            .desired_width(56.0)
                            .hint_text("0–255"),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let set   = ui.button("Set").clicked();
                    if enter || set {
                        match self.aci_picker.manual_entry.trim().parse::<u16>() {
                            Ok(n) if n <= 255 => {
                                picked = Some(n as u8);
                                self.aci_picker.manual_entry.clear();
                            }
                            _ => {
                                // Leave the (bad) input visible so the
                                // user can correct it; no toast yet.
                            }
                        }
                    }
                });

                // ---- Hover readout -------------------------------------
                ui.add_space(2.0);
                let hover_label = if let Some(aci) = self.aci_picker.hovered_aci {
                    let (r, g, b) = aci_palette(aci);
                    format!(
                        "Hover: ACI {}   RGB {},{},{}   #{:02X}{:02X}{:02X}",
                        aci, r, g, b, r, g, b
                    )
                } else {
                    String::from("(hover a circle for ACI / RGB)")
                };
                ui.label(hover_label);
            });

        // Apply the pick to whoever asked.
        if let Some(aci) = picked {
            match target {
                AciPickRequest::Layer(id) => {
                    if let Some(l) = self.doc.layers.get_mut(id) {
                        l.color = Color::Aci(aci);
                    }
                    self.touch_view();
                }
                AciPickRequest::Dobject(ix) => {
                    if let Some(d) = self.doc.dobjects.get_mut(ix) {
                        d.style.color = Color::Aci(aci);
                    }
                    self.touch_view();
                }
                AciPickRequest::DobjectMany => {
                    let targets = std::mem::take(&mut self.aci_pick_many);
                    self.props_apply(&targets, true, |d| {
                        d.style.color = Color::Aci(aci);
                    });
                }
                AciPickRequest::DimStyleForm(slot) => {
                    // The Dim Style form is restored to `Some` by the time
                    // this picker render runs (it precedes us each frame).
                    if let Some(d) = self.dim_style_dialog.as_mut() {
                        match slot {
                            DimColorSlot::DimLine => d.color_aci = Some(aci),
                            DimColorSlot::ExtLine => d.ext_color_aci = Some(aci),
                            DimColorSlot::Text => d.text_color_aci = Some(aci),
                        }
                    }
                }
                AciPickRequest::WallStyleForm(slot) => {
                    if let Some(d) = self.wall_style_dialog.as_mut() {
                        match slot {
                            WallColorSlot::Fill => d.fill_aci = Some(aci),
                            WallColorSlot::Face => d.face_aci = Some(aci),
                        }
                    }
                }
                AciPickRequest::BlockForm => {
                    if let Some(d) = self.block_dialog.as_mut() {
                        d.color_aci = Some(aci);
                    }
                }
                AciPickRequest::CurrentColor => {
                    self.doc.current_color = Color::Aci(aci);
                    self.touch_view();
                    self.history.push(format!("  color: current → ACI {}", aci));
                }
                AciPickRequest::PlotStyleColor => {
                    // Set plot_color = Aci(picked) for every selected plot style.
                    let sel = self.plotstyle_sel.clone();
                    for a in sel {
                        self.doc.plot_styles.style_mut(a).plot_color =
                            cad_kernel::plotstyle::PlotColor::Aci(aci);
                    }
                }
                AciPickRequest::ScriptParam(ix) => {
                    // The chosen color lands in the open parameter dialog and
                    // restarts the ghost preview.
                    if let Some(d) = self.script_param_dialog.as_mut() {
                        if let Some(v) = d.values.get_mut(ix) {
                            *v = aci.to_string();
                        }
                    }
                    self.script_preview_dirty();
                }
            }
            self.aci_pick_request = None;
        }

        if do_save {
            match self.aci_picker.save_mapping(&aci_mapping_path()) {
                Ok(()) => self.history.push(format!(
                    "  ACI mapping saved → {}",
                    aci_mapping_path().display()
                )),
                Err(e) => self
                    .history
                    .push(format!("  ! ACI mapping save failed: {}", e)),
            }
        }

        if !open {
            // User dismissed the window — abandon the pending request.
            self.aci_pick_request = None;
        }
    }

    // ===================================================================
    // Slice C — Pen palette
    // ===================================================================
    //
    // Egui-port of LibreCAD's `lc_penpalettewidget`. Each pen is a named
    // bundle of (color, linetype, lineweight). Clicking "Apply" rewrites
    // those three fields on every Dobject in the current selection.
    // Pens themselves are not persisted on Dobjects — they're a UI
    // shortcut for setting multiple style fields together.

    pub(super) fn render_pen_palette(&mut self, ctx: &egui::Context) {
        let mut open = self.pens_window_open;
        let win = egui::Window::new(format!("Pens ({})", self.doc.pens.len()))
            .open(&mut open)
            .default_pos(egui::pos2(340.0, 70.0))
            .default_size(egui::vec2(280.0, 420.0))
            .min_width(220.0)
            .resizable(true)
            .collapsible(true);
        let win = self.apply_dock_pos("Pens", ctx, win);
        let resp = win.show(ctx, |ui| {
            ui.separator();

            if self.selection.is_empty() {
                ui.colored_label(
                    egui::Color32::from_rgb(180, 180, 200),
                    "(select dobjects first — `select` / Shift-click / window)",
                );
            } else {
                ui.label(format!(
                    "Apply to {} selected dobject(s):",
                    self.selection.len()
                ));
            }
            ui.add_space(4.0);

            let mut apply: Option<usize> = None;
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    egui::Grid::new("pen_rows")
                        .num_columns(3)
                        .spacing([8.0, 6.0])
                        .striped(true)
                        .show(ui, |ui| {
                            for (i, pen) in self.doc.pens.pens.iter().enumerate() {
                                // ---- color swatch ----
                                let (r, g, b) = match pen.color {
                                    Color::TrueColorRef(idx) => {
                                        let v = self.doc.truecolors.get(idx).unwrap_or(0x808080);
                                        (
                                            ((v >> 16) & 0xFF) as u8,
                                            ((v >> 8) & 0xFF) as u8,
                                            (v & 0xFF) as u8,
                                        )
                                    }
                                    Color::Aci(idx) => aci_palette(idx),
                                    Color::ByLayer | Color::ByBlock => (180, 180, 200),
                                };
                                let arr = [r, g, b];
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(22.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    rect,
                                    2.0,
                                    egui::Color32::from_rgb(arr[0], arr[1], arr[2]),
                                );
                                ui.painter().rect_stroke(
                                    rect,
                                    2.0,
                                    egui::Stroke::new(0.7, egui::Color32::from_rgb(70, 80, 95)),
                                );

                                // ---- name + description ----
                                ui.vertical(|ui| {
                                    ui.label(&pen.name);
                                    let lt = self
                                        .doc
                                        .linetypes
                                        .get(pen.linetype)
                                        .map(|l| l.name.as_str())
                                        .unwrap_or("?");
                                    let lw = match pen.lineweight {
                                        Lineweight::ByLayer => "ByLayer".to_string(),
                                        Lineweight::ByBlock => "ByBlock".to_string(),
                                        Lineweight::Default => "Default".to_string(),
                                        Lineweight::Custom(mm) => format!("{:.2} mm", mm),
                                    };
                                    ui.small(format!("{} · {}", lt, lw));
                                });

                                // ---- apply button ----
                                let enabled = !self.selection.is_empty();
                                ui.add_enabled_ui(enabled, |ui| {
                                    if ui.button("apply").clicked() {
                                        apply = Some(i);
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });

            // Deferred mutation outside the borrow chain.
            if let Some(i) = apply {
                if let Some(pen) = self.doc.pens.get(i) {
                    let (c, lt, lw) = (pen.color, pen.linetype, pen.lineweight);
                    let pen_name = pen.name.clone();
                    let count = self.selection.len();
                    for &idx in &self.selection {
                        if let Some(d) = self.doc.dobjects.get_mut(idx) {
                            d.style.color = c;
                            d.style.linetype = lt;
                            d.style.lineweight = lw;
                        }
                    }
                    self.history.push(format!(
                        "  pen '{}' applied to {} dobject(s)",
                        pen_name, count
                    ));
                    self.touch_view();
                }
            }
        });
        self.raise_after_show("Pens", ctx, &resp);
        self.process_dock_after_show("Pens", ctx, resp);
        self.pens_window_open = open;
    }

    // ===================================================================
    // Slice D — Entity Info panel
    // ===================================================================
    //
    // Egui-port of LibreCAD's `lc_quickinfowidget`. Two modes:
    //
    //   1. Single Dobject selected (via `self.selected` from the right
    //      panel) → full geometry breakdown + editable style fields.
    //   2. Multi-Dobject selection (`self.selection`) → summary counts +
    //      bulk-edit layer / visibility / color.

    pub(super) fn render_info_panel(&mut self, ctx: &egui::Context) {
        if !self.info_window_open {
            return;
        }
        // Rendered through the UNIFIED dock host (dock.rs) — same docking
        // behaviour as every other dockable panel, and the engine is swappable.
        // Type chip for the header (per the design: the dobject type shows in the
        // header, not as a field). Single = "Line"; uniform many = "Line (3)";
        // mixed = "Mixed (n)"; empty = none.
        let badge_txt: Option<String> = {
            let mut t: Vec<usize> = if !self.selection.is_empty() {
                self.selection.clone()
            } else if let Some(i) = self.selected {
                vec![i]
            } else {
                Vec::new()
            };
            t.retain(|&i| i < self.doc.dobjects.len());
            let cap1 = |s: &str| {
                let mut c = s.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            };
            if t.is_empty() {
                None
            } else if t.len() == 1 {
                self.doc
                    .dobjects
                    .get(t[0])
                    .map(|d| cap1(dobject_kind_name(&d.geom)))
            } else if self.targets_uniform_tag(&t).is_some() {
                self.doc
                    .dobjects
                    .get(t[0])
                    .map(|d| format!("{} ({})", cap1(dobject_kind_name(&d.geom)), t.len()))
            } else {
                Some(format!("Mixed ({})", t.len()))
            }
        };
        let cfg = crate::dock::DockConfig {
            // INSPECTOR_DESIGN_MENTOR §2: the type pill is no longer in the header
            // — it's a centered full-width capsule below it, painted in
            // `inspector_body`. So the shared header carries no badge here.
            id: "inspector",
            title: "Inspector",
            badge: None,
            dock_region: crate::dock::DockRegion::Right,
            size: 264.0,
            min: 264.0,
            max: 520.0,
            // flush_body: the Inspector paints its own padding (panel-edge 16,
            // panel-header→content 24) per INSPECTOR_DESIGN §2.
            resizable: true,
            flush_body: true,
            float_w: 264.0,
            float_max_h_frac: 0.5,
            alt_region: None,
            any_edge: false,
            strip_h: 0.0,
            dockable: true,
            rail_header: false,
            collapsible: false,
        };
        let mut state = self.inspector_dock_state;
        let mut open = self.info_window_open;
        let capturing = self.props_layout_capture;
        let pill = badge_txt.clone();
        let rect = crate::dock::HOST.show(ctx, &cfg, &mut state, &mut open, |ui, cap| {
            self.inspector_body(ui, cap, pill.as_deref());
        });
        self.raise_dock_after_show(
            "Inspector",
            "inspector",
            matches!(state, crate::dock::DockState::Floating(_)),
            ctx,
        );
        self.inspector_dock_state = state;
        self.info_window_open = open;
        if capturing && rect.is_finite() {
            ctx.data_mut(|d| {
                let id = egui::Id::new("pp_cap_buf");
                let mut v: Vec<(String, egui::Rect)> = d.get_temp(id).unwrap_or_default();
                v.push(("Inspector · PANEL FRAME".to_string(), rect));
                d.insert_temp(id, v);
            });
        }
    }

    /// The Inspector's CONTENT — scoped visuals, selection badge, search box,
    /// and the property body. Shared by the docked and floating renderers.
    /// `scroll_max_h`: Some caps the property scroll area (floating, ≤50% screen);
    /// None lets it fill (docked).
    fn inspector_body(&mut self, ui: &mut egui::Ui, scroll_max_h: Option<f32>, pill: Option<&str>) {
        // Design tokens (THEME_SYSTEM §5) — no raw hex; the Inspector's widget
        // visuals now read the SAME palette as its property rows (which use PP_*).
        let bg_lo = crate::theme::color::SURFACE_0;
        let bg_hi = crate::theme::color::SURFACE_2;
        let border = crate::theme::color::BORDER;
        let text = crate::theme::color::TEXT_PRIMARY;
        let muted = crate::theme::color::TEXT_SECONDARY;
        let accent = crate::theme::color::ACCENT;
        {
            let v = ui.visuals_mut();
            v.override_text_color = Some(text);
            v.widgets.inactive.weak_bg_fill = bg_lo;
            v.widgets.inactive.bg_fill = bg_lo;
            v.widgets.hovered.weak_bg_fill = bg_hi;
            v.widgets.hovered.bg_fill = bg_hi;
            v.widgets.active.weak_bg_fill = bg_hi;
            v.widgets.active.bg_fill = bg_hi;
            v.widgets.open.weak_bg_fill = bg_lo;
            v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, border);
            v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
            v.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0, 0xe5, 0xff, 60);
            v.selection.stroke = egui::Stroke::new(1.0, accent);
            v.hyperlink_color = accent;
            v.indent_has_left_vline = false;
            v.extreme_bg_color = bg_lo;
            // Inputs / value boxes = 4px radius (sm), per THEME_SYSTEM §5.2.
            let fld = egui::Rounding::same(crate::theme::radius::SM);
            v.widgets.inactive.rounding = fld;
            v.widgets.hovered.rounding = fld;
            v.widgets.active.rounding = fld;
        }
        // Horizontal item gap = label→input (8); vertical = 0 so EVERY vertical
        // gap is an explicit token (row gap 8, section gap 12, group gap 12) and
        // the measured layout matches the spec exactly.
        ui.spacing_mut().item_spacing = egui::vec2(crate::theme::space::LABEL_INPUT, 0.0);

        // Target set: the basket (group) takes priority; otherwise the single
        // click-selected dobject. Deduped + sorted so the shared-value scan and
        // the apply loop agree.
        let mut targets: Vec<usize> = if !self.selection.is_empty() {
            self.selection.clone()
        } else if let Some(i) = self.selected {
            vec![i]
        } else {
            Vec::new()
        };
        targets.retain(|&i| i < self.doc.dobjects.len());
        targets.sort_unstable();
        targets.dedup();

        // Content padded per INSPECTOR_DESIGN_MENTOR §2: sides = panel-edge (16),
        // top = header→pill (12) — the pill row sits here, above the sections —
        // bottom = 16.
        egui::Frame::none()
            .inner_margin(egui::Margin {
                left: crate::theme::space::PANEL_EDGE,
                right: crate::theme::space::PANEL_EDGE,
                top: crate::theme::space::HEADER_TO_PILL,
                bottom: crate::theme::space::PANEL_EDGE,
            })
            .show(ui, |ui| {
                if targets.is_empty() {
                    ui.colored_label(
                        muted,
                        "No selection — click a dobject to edit its properties.",
                    );
                    return;
                }
                // Type pill (INSPECTOR_DESIGN_MENTOR §2): a full-content-width,
                // full-radius capsule, type text centered, accent @ ~10% fill.
                // Painted ABOVE the scroll area so it stays put; 12px below the
                // header (frame top margin) and 12px above GENERAL.
                if let Some(txt) = pill {
                    let pw = ui.available_width();
                    let (pr, _) = ui.allocate_exact_size(
                        egui::vec2(pw, crate::theme::space::PILL_H),
                        egui::Sense::hover(),
                    );
                    let p = ui.painter();
                    p.rect_filled(
                        pr,
                        egui::Rounding::same(crate::theme::space::PILL_H / 2.0),
                        egui::Color32::from_rgba_unmultiplied(0x00, 0xe5, 0xff, 26),
                    );
                    p.text(
                        pr.center(),
                        egui::Align2::CENTER_CENTER,
                        txt,
                        crate::theme::typ::caption(),
                        accent,
                    );
                    pp_capture(
                        ui,
                        &format!(
                            "Type pill · '{}'  full-width capsule h={}",
                            txt,
                            crate::theme::space::PILL_H
                        ),
                        pr,
                    );
                    ui.add_space(crate::theme::space::PILL_TO_SECTION);
                }
                // Docked: fill height. Floating: grow to content, cap ~50% screen.
                let shrink_v = scroll_max_h.is_some();
                let mut sa = egui::ScrollArea::vertical().auto_shrink([false, shrink_v]);
                if let Some(h) = scroll_max_h {
                    sa = sa.max_height(h);
                }
                sa.show(ui, |ui| {
                    self.render_props_body(ui, &targets, "");
                });
            });
    }

    /// The ONE execution seam (COMMAND_REGISTRY_MENTOR): resolve a registry `id`
    /// (`"draw.line"`) → its `dispatch` token and run it. EVERY UI surface
    /// dispatches through this — **never `run_command(id)`** (an id isn't a
    /// dispatch token; it would no-op). Defensive: an unknown/stale id is a
    /// graceful no-op.
    ///
    /// It **applies the command-level preferred method** (`command_method`, set
    /// via a ▼ flyout): if this command has a remembered method, run it
    /// (`dispatch_method`); otherwise the base/default form (`dispatch_base`).
    /// So the remembered method is honored consistently from every surface —
    /// menu, rail, palette, shortcut. (The command line is exempt; it stays
    /// base/explicit until the deferred command-methods track.)
    pub(super) fn execute(&mut self, id: &str) {
        if let Some(dispatch) = self.command_registry.get(id).map(|c| c.dispatch) {
            if let Some(key) = self.command_method.get(dispatch).cloned() {
                self.dispatch_method(dispatch, &key);
            } else {
                self.dispatch_base(dispatch);
            }
        }
    }

    /// Build the read-only [`crate::command::Ctx`] projection for context
    /// predicates (Phase 6b) — a cheap, `Copy` snapshot of the app state a
    /// predicate may read. Pure (`&self`): no mutation, and it hands the registry
    /// COPIED values, never a gateway back into `CadApp` (D7). Call once per
    /// frame; surfaces read the snapshot to filter/grey commands.
    pub(super) fn build_ctx(&self) -> crate::command::Ctx {
        let selection_count = if !self.selection.is_empty() {
            self.selection.len()
        } else if self.selected.is_some() {
            1
        } else {
            0
        };
        crate::command::Ctx {
            selection_count,
            has_selection: selection_count > 0,
            active_tool: self.tool,
            has_clipboard: !self.clipboard_dobjects.is_empty(),
        }
    }

    /// Command palette (Phase 7): registry-driven fuzzy search over `title` +
    /// `keywords`, dispatch via `execute(id)` (D2 — so it also respects the
    /// `command_method` memory, like menus). Opens on Ctrl+Shift+P or Tools ▸
    /// "Command palette". Applies the 6b predicates (hidden if `!visible`, greyed
    /// if `!enabled`). Keyboard: ↑/↓ move, Enter runs, Esc closes. It NEVER
    /// touches the parser or parser aliases (D6).
    pub(super) fn render_command_palette(&mut self, ctx: &egui::Context) {
        // A single hotkey to OPEN the palette (this is NOT the deferred
        // per-command accel→id shortcut map).
        if ctx.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::P)) {
            self.palette_open = true;
            self.palette_focus = true;
            self.palette_query.clear();
            self.palette_sel = 0;
        }
        if !self.palette_open {
            return;
        }

        let cmd_ctx = self.build_ctx();
        let q = self.palette_query.to_lowercase();

        // Build the result rows (owned — the registry borrow drops before we
        // execute). A method-bearing command (arc/circle/fillet) contributes a
        // BASE entry (method-aware glyph → runs the remembered method) plus an
        // inline "methods" GROUP of variant rows (each its own construction glyph
        // → runs AND sets that method). ONE source (`rail_flyout_items`) + method
        // memory (`command_method`), same as rail + menu. §1 label = `Name (CODE)`.
        // Per-row fuzzy so typing `2P` filters straight to that method (§3).
        enum HitGlyph {
            Base(crate::command::IconId),
            Method { cmd: String, key: String },
        }
        #[derive(Clone)]
        enum HitRun {
            Execute(String),
            Method { cmd: String, key: String },
        }
        struct Hit {
            name: String,
            code: String,
            enabled: bool,
            glyph: HitGlyph,
            run: HitRun,
            variant: bool,
            current: bool,
        }
        let mut hits: Vec<Hit> = Vec::new();
        for id in &self.command_registry.order {
            if let Some(info) = self.command_registry.commands.get(id) {
                if !(info.visible)(&cmd_ctx) {
                    continue;
                } // 6b: hidden
                let ena = (info.enabled)(&cmd_ctx);
                let dispatch = info.dispatch;
                let methods = rail_flyout_items(dispatch);
                if methods.is_empty() {
                    // Non-method command — a plain row (no code).
                    let hay = format!("{} {}", info.title, info.keywords.join(" ")).to_lowercase();
                    if !palette_fuzzy(&q, &hay) {
                        continue;
                    }
                    hits.push(Hit {
                        name: info.title.clone(),
                        code: String::new(),
                        enabled: ena,
                        glyph: HitGlyph::Base(info.icon),
                        run: HitRun::Execute(id.clone()),
                        variant: false,
                        current: false,
                    });
                } else {
                    let cur_key = self.current_method_key(dispatch);
                    let cur_short = self.current_method_short(dispatch);
                    // Base entry — method-aware glyph; `(CODE)` names the current
                    // method (cyan, §3). Matches on command title + keywords.
                    let base_hay =
                        format!("{} {}", info.title, info.keywords.join(" ")).to_lowercase();
                    if palette_fuzzy(&q, &base_hay) {
                        hits.push(Hit {
                            name: info.title.clone(),
                            code: cur_short.clone(),
                            enabled: ena,
                            glyph: HitGlyph::Method {
                                cmd: dispatch.to_string(),
                                key: cur_key.clone(),
                            },
                            run: HitRun::Execute(id.clone()),
                            variant: false,
                            current: false,
                        });
                    }
                    // Methods group — method-only rows (`3-Point (3P)`, §3). Each
                    // matches on the command word + its own name/code, so `2P`
                    // filters to the 2-Point variant. Current row = cyan (§5).
                    for (m_short, m_full, key) in &methods {
                        let var_hay =
                            format!("{} {} {}", info.title, m_full, m_short).to_lowercase();
                        if !palette_fuzzy(&q, &var_hay) {
                            continue;
                        }
                        hits.push(Hit {
                            name: m_full.clone(),
                            code: m_short.clone(),
                            enabled: ena,
                            glyph: HitGlyph::Method {
                                cmd: dispatch.to_string(),
                                key: key.clone(),
                            },
                            run: HitRun::Method {
                                cmd: dispatch.to_string(),
                                key: key.clone(),
                            },
                            variant: true,
                            current: key == &cur_key,
                        });
                    }
                }
            }
        }
        if self.palette_sel >= hits.len() {
            self.palette_sel = hits.len().saturating_sub(1);
        }

        // Keyboard nav (raw key events).
        let (mut down, mut up, mut enter, mut esc) = (false, false, false, false);
        ctx.input(|i| {
            down = i.key_pressed(egui::Key::ArrowDown);
            up = i.key_pressed(egui::Key::ArrowUp);
            enter = i.key_pressed(egui::Key::Enter);
            esc = i.key_pressed(egui::Key::Escape);
        });
        if down && !hits.is_empty() {
            self.palette_sel = (self.palette_sel + 1).min(hits.len() - 1);
        }
        if up {
            self.palette_sel = self.palette_sel.saturating_sub(1);
        }

        let sel = self.palette_sel;
        let mut click: Option<usize> = None;
        let mut close_x = false;
        let mut open = self.palette_open;

        // ---- Layout columns + HUG width (§3.1/§7). The width follows the longest
        // RESULT line (measured), NOT the search text — clamped so it never gets
        // too narrow or too wide. No arbitrary fixed width.
        const PAD: f32 = 12.0;
        const ICON: f32 = 20.0;
        const ICON_GAP: f32 = 14.0; // icon → name (§3.1)
        const CODE_GAP: f32 = 6.0; // name → (CODE)
        const ROW_H: f32 = 26.0; // row / hover band; icon box = ROW_H − 6
        const PAL_MIN_W: f32 = 300.0;
        const PAL_MAX_W: f32 = 560.0;
        let font_name = crate::theme::typ::body();
        let font_code = crate::theme::typ::data_code();
        let mut longest = 0.0_f32;
        for h in &hits {
            let name_w = ctx.fonts(|f| {
                f.layout_no_wrap(h.name.clone(), font_name.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            });
            let code_w = if h.code.is_empty() {
                0.0
            } else {
                CODE_GAP
                    + ctx.fonts(|f| {
                        f.layout_no_wrap(
                            format!("({})", h.code),
                            font_code.clone(),
                            egui::Color32::WHITE,
                        )
                        .size()
                        .x
                    })
            };
            let indent = if h.variant { ICON + ICON_GAP } else { 0.0 };
            longest = longest.max(indent + name_w + code_w);
        }
        let hug_w = (PAD + ICON + ICON_GAP + longest + PAD).clamp(PAL_MIN_W, PAL_MAX_W);

        // Dragging the Floating band moves the window (§3.1): we own the position
        // (`palette_pos`) and feed it via `current_pos`; the band's drag_delta
        // nudges it. `None` until first drag → opens at the default spot.
        let start_pos = self.palette_pos.unwrap_or(egui::pos2(80.0, 70.0));
        let mut drag_delta = egui::Vec2::ZERO;

        // Body: surface-1, 1px rounded border. NO egui title bar — it paints the
        // shared HEADER_STANDARD band itself. Top/side margins 0 so the band is
        // flush; a 6px BOTTOM inset stops the last result row AT the hairline
        // (no droop past the rounded border, §3.1).
        let frame = egui::Frame::none()
            .fill(crate::theme::color::SURFACE_1)
            .stroke(egui::Stroke::new(1.0, crate::theme::color::BORDER))
            .rounding(egui::Rounding::same(crate::theme::radius::SM))
            .inner_margin(egui::Margin {
                left: 0.0,
                right: 0.0,
                top: 0.0,
                bottom: 6.0,
            });
        egui::Window::new("Command palette")
            .open(&mut open)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .default_width(hug_w)
            .current_pos(start_pos)
            .frame(frame)
            .show(ctx, |ui| {
                // ---- shared Floating header band (HEADER_STANDARD) — drag handle ----
                let hb = crate::dock::header_band(ui, "Command palette", None, true);
                if hb.close_clicked {
                    close_x = true;
                }
                if hb.band.dragged() {
                    drag_delta = hb.band.drag_delta();
                }

                // ---- sticky search block: full-round pill + 1px divider ----
                egui::Frame::none()
                    .inner_margin(egui::Margin {
                        left: 12.0,
                        right: 12.0,
                        top: 8.0,
                        bottom: 8.0,
                    })
                    .show(ui, |ui| {
                        egui::Frame::none()
                            .fill(crate::theme::color::SURFACE_0)
                            .rounding(egui::Rounding::same(12.0)) // full-round: r = h/2
                            .stroke(egui::Stroke::new(1.0, crate::theme::color::BORDER))
                            .inner_margin(egui::Margin::symmetric(12.0, 3.0))
                            .show(ui, |ui| {
                                let te = ui.add(
                                    egui::TextEdit::singleline(&mut self.palette_query)
                                        .frame(false)
                                        .desired_width(f32::INFINITY)
                                        .font(crate::theme::typ::body())
                                        .hint_text(
                                            egui::RichText::new("Search commands")
                                                .color(crate::theme::color::TEXT_MUTED),
                                        ),
                                );
                                if self.palette_focus {
                                    te.request_focus();
                                    self.palette_focus = false;
                                }
                                if te.changed() {
                                    self.palette_sel = 0;
                                }
                            });
                    });
                // divider pinned beneath the search (list scrolls under it)
                let dv = ui.available_rect_before_wrap();
                ui.painter().hline(
                    dv.x_range(),
                    dv.top(),
                    egui::Stroke::new(1.0, crate::theme::color::BORDER),
                );

                if hits.is_empty() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.weak("No matching commands.");
                    });
                }
                // ---- result list (full-width rows; hand-painted) ----
                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        // Layout columns (§3/§3.1/§4) — consts defined at fn scope:
                        // left pad, aligned icon column, icon→name gap 14; variants
                        // indent so the method glyph sits under the base command's
                        // NAME (indent A); row band 26, icon box = 26−6.
                        for (i, h) in hits.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), ROW_H),
                                egui::Sense::click(),
                            );
                            let p = ui.painter_at(rect);
                            let is_cursor = i == sel;
                            // Row bg: hover OR keyboard cursor → surface-2 (like rails).
                            if resp.hovered() || is_cursor {
                                p.rect_filled(rect, 0.0, crate::theme::color::SURFACE_2);
                            }
                            // Keyboard cursor also gets a 2px cyan left bar (§3.1) —
                            // independent of the current-method cyan text.
                            if is_cursor {
                                p.rect_filled(
                                    egui::Rect::from_min_size(
                                        rect.left_top(),
                                        egui::vec2(2.0, rect.height()),
                                    ),
                                    0.0,
                                    PP_ACCENT,
                                );
                            }
                            // Uniform glyph — one thin stroke + muted tone for EVERY
                            // row, method or not, in the aligned icon column (§4).
                            let g_ink = if h.enabled {
                                crate::theme::color::TEXT_MUTED
                            } else {
                                crate::theme::color::TEXT_DISABLED
                            };
                            let g_pen = egui::Stroke::new(1.0, g_ink);
                            // Base name column; variants indent by one icon+gap so
                            // the method glyph aligns under the base command's name.
                            let base_name_x = rect.left() + PAD + ICON + ICON_GAP;
                            let (glyph_cx, name_x) = if h.variant {
                                (base_name_x + ICON / 2.0, base_name_x + ICON + ICON_GAP)
                            } else {
                                (rect.left() + PAD + ICON / 2.0, base_name_x)
                            };
                            let gc = egui::pos2(glyph_cx, rect.center().y);
                            // Uniform icon box = row band − 6 (§4/§7). Each glyph
                            // fn normalises its own natural box to this, so all
                            // icons are one physical size regardless of source fn.
                            let icon_box = ROW_H - 6.0;
                            match &h.glyph {
                                HitGlyph::Base(icon) => {
                                    let dot = |qq| {
                                        p.circle_filled(qq, 1.1, g_ink);
                                    };
                                    match *icon {
                                        crate::command::IconId::DrawGlyph(s) => draw_draw_glyph(
                                            &p,
                                            gc,
                                            s,
                                            g_pen,
                                            dot,
                                            g_ink,
                                            icon_box / GLYPH_BOX_DRAW,
                                        ),
                                        crate::command::IconId::ModifyGlyph(k) => draw_cmd_glyph(
                                            &p,
                                            gc,
                                            k,
                                            g_pen,
                                            dot,
                                            g_ink,
                                            icon_box / GLYPH_BOX_CMD,
                                        ),
                                    }
                                }
                                HitGlyph::Method { cmd, key } => draw_method_glyph(
                                    &p,
                                    gc,
                                    cmd,
                                    key,
                                    g_pen,
                                    g_ink,
                                    icon_box / GLYPH_BOX_METHOD,
                                ),
                            }
                            // Name + `(CODE)` — §5 colour marker. Current method =
                            // whole row cyan; base command = name primary + code
                            // cyan; other method rows = dim tier; disabled = greyed.
                            let (name_col, code_col) = if !h.enabled {
                                (
                                    crate::theme::color::TEXT_DISABLED,
                                    crate::theme::color::TEXT_DISABLED,
                                )
                            } else if h.current {
                                (PP_ACCENT, PP_ACCENT)
                            } else if h.variant {
                                (
                                    crate::theme::color::TEXT_MUTED,
                                    crate::theme::color::TEXT_MUTED,
                                )
                            } else {
                                (PP_TEXT, PP_ACCENT)
                            };
                            let nr = p.text(
                                egui::pos2(name_x, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &h.name,
                                crate::theme::typ::body(),
                                name_col,
                            );
                            if !h.code.is_empty() {
                                // Code left-grouped right after the name (never right-aligned).
                                p.text(
                                    egui::pos2(nr.right() + CODE_GAP, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    format!("({})", h.code),
                                    crate::theme::typ::data_code(),
                                    code_col,
                                );
                            }
                            if resp.clicked() {
                                click = Some(i);
                            }
                            pp_capture(
                                ui,
                                &format!(
                                    "palette{} · {} ({})",
                                    if h.variant { " variant" } else { "" },
                                    h.name,
                                    h.code
                                ),
                                rect,
                            );
                        }
                    });
            });
        // Commit a header-band drag to the stored window position (§3.1).
        if drag_delta != egui::Vec2::ZERO {
            self.palette_pos = Some(start_pos + drag_delta);
        }

        // Resolve the action: a click runs the clicked item; else Enter runs the
        // highlighted one. A BASE entry dispatches via `execute(id)`; a method
        // VARIANT via `dispatch_method` (runs AND sets `command_method`). Neither
        // touches the parser / `run_command`.
        let chosen = click.or(if enter { Some(sel) } else { None });
        let picked: Option<(usize, HitRun)> = chosen.and_then(|i| {
            hits.get(i)
                .filter(|h| h.enabled)
                .map(|h| (i, h.run.clone()))
        });
        // Open state: egui window (no × since title_bar off), header ×, or Esc.
        self.palette_open = open && !close_x && !esc;
        if let Some((i, run)) = picked {
            self.palette_sel = i;
            match run {
                HitRun::Execute(id) => self.execute(&id),
                HitRun::Method { cmd, key } => self.dispatch_method(&cmd, &key),
            }
            self.palette_open = false;
            self.palette_query.clear();
        }
    }

    /// The ONE canonical short-code (`"3P"`, `"CR"`, `"F"`, …) for a command's
    /// CURRENT method — the remembered `command_method`, or the command's default
    /// (first ▼-flyout item) when none is set. Sourced from `rail_flyout_items` so
    /// the rail flyout, rail tooltip, and menus all use the SAME notation. Empty
    /// for commands without methods.
    pub(super) fn current_method_short(&self, cmd: &str) -> String {
        let methods = rail_flyout_items(cmd);
        match self.command_method.get(cmd) {
            Some(k) => methods
                .iter()
                .find(|(_, _, key)| key == k)
                .map(|(s, _, _)| s.clone())
                .unwrap_or_default(),
            None => methods
                .first()
                .map(|(s, _, _)| s.clone())
                .unwrap_or_default(),
        }
    }

    /// The canonical KEY of a command's CURRENT method — the remembered
    /// `command_method`, or the default (first `rail_flyout_items` entry) when
    /// none is set. Empty for commands without methods. Companion to
    /// `current_method_short` — returns the dispatch key the glyph +
    /// `dispatch_method` need. Same ONE source, no per-surface lists.
    pub(super) fn current_method_key(&self, cmd: &str) -> String {
        let methods = rail_flyout_items(cmd);
        match self.command_method.get(cmd) {
            Some(k) if methods.iter().any(|(_, _, key)| key == k) => k.clone(),
            _ => methods
                .first()
                .map(|(_, _, key)| key.clone())
                .unwrap_or_default(),
        }
    }

    /// Dispatch a command's **base (default) form** — the path `execute(id)` takes
    /// when the command has no remembered method. Handles the non-`run_command`
    /// specials every surface shares — `pointer` → selection mode, `array` → its
    /// dialog — and otherwise runs `run_command(cmd)`. The preferred-method branch
    /// lives in `execute` (`command_method`), not here.
    fn dispatch_base(&mut self, cmd: &str) {
        match cmd {
            "pointer" => {
                self.tool = Tool::None;
            } // selection mode
            "array" => self.array_open = true,
            other => self.run_command(other),
        }
    }

    /// Start a rail command in a SPECIFIC creation method, chosen from the ▼
    /// flyout. Reuses each command's normal start (via `run_command`) and then
    /// nudges it into the requested method/option so all the usual setup runs.
    pub(super) fn dispatch_method(&mut self, cmd: &str, key: &str) {
        // Remember the chosen method so the rail icon shows its glyph.
        if matches!(cmd, "arc" | "circle" | "fillet") {
            self.command_method.insert(cmd.to_string(), key.to_string());
        }
        match cmd {
            "arc" => {
                self.run_command("arc");
                if let Ok(i) = key.parse::<usize>() {
                    if let Some(&m) = ALL_ARC_METHODS.get(i) {
                        self.arc_method = m;
                    }
                }
                self.rail_active = "arc".into();
            }
            "circle" => {
                self.run_command("circle"); // starts at CircleStep::Center
                let step = match key {
                    "3p" => Some(CircleStep::P3a),
                    "2p" => Some(CircleStep::P2a),
                    "ttr" => Some(CircleStep::TtrObj1),
                    _ => None, // "center" = default start
                };
                if let (Some(s), Some(f)) = (step, self.cmd_flow.as_mut()) {
                    f.circle = s;
                }
                self.flow_show_prompt();
                self.rail_active = "circle".into();
            }
            "fillet" => {
                self.run_command("fillet");
                match key {
                    "poly" => self.run_command("p"),
                    "multi" => self.run_command("m"),
                    _ => {}
                }
                self.rail_active = "fillet".into();
            }
            _ => self.dispatch_base(cmd),
        }
    }

    /// Shared body of the command bar — the history scroll, the current-prompt
    /// line, and the input row. Used by BOTH docking modes through the unified
    /// dock host so the two can't drift. `scroll_cap`: `Some` when floating
    /// (the Area imposes no height, so cap the history to ≤50% of the screen);
    /// `None` when docked (fill the panel).
    pub(super) fn command_bar_body(&mut self, ui: &mut egui::Ui, scroll_cap: Option<f32>) {
        // Reserve space at the bottom for: prompt line (always shown) +
        // the input row.
        let bottom_reserve = 32.0 + 18.0;
        let avail_h = scroll_cap.unwrap_or_else(|| ui.available_height());
        egui::ScrollArea::vertical()
            .id_salt("hist_scroll")
            .stick_to_bottom(true)
            // Full panel width: the log must span the strip edge-to-edge, so
            // the vertical scrollbar hugs the panel's right edge. auto_shrink
            // [horizontal, vertical] — false here means "do not shrink to the
            // content width" (short log lines would otherwise pull the
            // scrollbar into the middle of the bar).
            .auto_shrink([false, true])
            .max_height((avail_h - bottom_reserve).max(24.0))
            .show(ui, |ui| {
                for h in &self.history {
                    ui.monospace(h);
                }
            });
        // The line directly above the input is the CURRENT prompt for the
        // active command — or the idle "command:" ready prompt when nothing
        // is active. It always shows, so the user always knows what's expected.
        if self.current_prompt.is_empty() {
            // The idle prompt names the window it belongs to, so the command line is never
            // ambiguous about which model it is about to act on. An object waiting for a placing
            // click outranks both — that is the one thing the app needs from you right now.
            if self.factory.awaiting_place.is_some() {
                ui.colored_label(
                    egui::Color32::from_rgb(240, 200, 110),
                    "click the 2D or 3D window to place it  ·  Esc leaves it where it is",
                );
            } else if self.command_target() == ActiveView::ThreeD {
                // …and it SAYS where the next object will land, with the word that changes it.
                //
                // Reported as: "the furniture once placed using a mode[,] it keeps placing it in
                // the same mode, there is no option to change." The mode was already changeable —
                // `place` has always done it — but nothing on screen said the setting existed, and
                // a setting you cannot see is a setting you cannot change. The dropdown that used
                // to show it was removed by request, so the readout belongs here instead.
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(150, 200, 235), "3D command:");
                    ui.label(
                        egui::RichText::new(format!(
                            "new objects → {}   ·   type `place` to change",
                            self.factory.place_mode.label(),
                        ))
                        .small()
                        .weak(),
                    );
                });
            } else {
                ui.colored_label(egui::Color32::from_rgb(150, 200, 235), "command:");
            }
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(0x18, 0x4c, 0x04),
                &self.current_prompt,
            );
        }
        ui.horizontal(|ui| {
            ui.label(">");
            let btn_w = 56.0_f32;
            let row_h = ui.spacing().interact_size.y;
            let text_resp = ui.add_sized(
                [(ui.available_width() - btn_w - 8.0).max(40.0), row_h],
                // A STABLE id, so the global Enter handler can recognise this one field as the
                // one that is allowed to drive the command cascade. Every other focused widget
                // owns its own Enter. See `focus_at_frame_start`.
                egui::TextEdit::singleline(&mut self.cmd).id(Self::cmd_line_id()),
            );
            let run_clicked = ui.button("run").clicked();
            let enter_pressed = (text_resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                || (text_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            let space_pressed =
                text_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Space));
            let in_text_body = matches!(self.text_draft, TextDraftState::WaitingForString(_))
                || self.text_waiting_height;
            let submit_via_space =
                space_pressed && !self.cmd.trim_end_matches(' ').is_empty() && !in_text_body;
            if submit_via_space {
                self.cmd = self.cmd.trim_end_matches(' ').to_string();
            }
            if enter_pressed || run_clicked || submit_via_space {
                if !self.cmd.trim().is_empty() {
                    let c = std::mem::take(&mut self.cmd);
                    self.run_command(&c);
                }
                self.refocus_cmd = true;
            }
            let other_focused = ui
                .ctx()
                .memory(|m| m.focused().is_some_and(|id| id != text_resp.id));
            let modal_textedit_active = self.layer_rename.is_some();
            if modal_textedit_active {
                self.refocus_cmd = false;
            } else if self.refocus_cmd && !other_focused {
                text_resp.request_focus();
                self.refocus_cmd = false;
                ui.ctx().request_repaint();
            } else if !other_focused && !text_resp.has_focus() {
                text_resp.request_focus();
                ui.ctx().request_repaint();
            }
        });
    }

    /// THE MODE TAB BAR — 2D view | SIMLUX view | 3D Factory view.
    ///
    /// A slim strip directly under the menu bar, before anything else reserves
    /// screen space. Exactly one workspace owns the window; the tab that owns
    /// it reads as pressed. Clicking a tab calls [`Self::switch_mode`], which
    /// rearranges the view open-flags (and cancels any pending automatic
    /// return-to-Factory after a sketch — a user who switched tabs wants to
    /// stay where they are when the sketch ends).
    pub(super) fn render_mode_tabs(&mut self, ctx: &egui::Context) {
        use crate::theme::color as tc;
        let mut pick: Option<Mode> = None;
        egui::TopBottomPanel::top("mode_tabs")
            .exact_height(34.0)
            .show_separator_line(true)
            .frame(egui::Frame::none().fill(tc::SURFACE_1))
            .show(ctx, |ui| {
                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    // A work-in-progress sketch owns the window too: say so here
                    // so the drafting state is never a mystery.
                    if self.factory.session.is_some() {
                        let fg = egui::Color32::from_rgb(255, 178, 60);
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgb(52, 38, 12))
                            .stroke(egui::Stroke::new(1.0, fg))
                            .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                            .rounding(4.0)
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("✎ drafting on a plane")
                                        .color(fg)
                                        .size(11.5)
                                        .strong(),
                                );
                            });
                        ui.add_space(8.0);
                    }
                    for m in [Mode::Cad2D, Mode::Simlux, Mode::Factory] {
                        let on = self.mode == m;
                        let (fg, bg) = if on {
                            (
                                tc::ACCENT,
                                egui::Color32::from_rgba_unmultiplied(0x00, 0x88, 0x99, 60),
                            )
                        } else {
                            (tc::TEXT_MUTED, egui::Color32::TRANSPARENT)
                        };
                        let label = egui::RichText::new(format!("{}  {}", m.glyph(), m.label()))
                            .color(fg)
                            .size(13.0)
                            .strong();
                        let b = egui::Button::new(label)
                            .rounding(egui::Rounding::ZERO)
                            .fill(bg)
                            .stroke(egui::Stroke::new(if on { 1.5 } else { 0.0 }, fg))
                            .min_size(egui::vec2(0.0, 26.0));
                        let resp = ui.add(b).on_hover_text(m.blurb());
                        if resp.clicked() && !on {
                            pick = Some(m);
                        }
                        ui.add_space(4.0);
                    }
                    // A quick key hint, far right.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(
                                "one workspace at a time — switch any time, nothing is lost",
                            )
                            .size(11.0)
                            .color(egui::Color32::from_rgb(110, 122, 136)),
                        );
                    });
                });
            });
        if let Some(m) = pick {
            self.switch_mode(m);
            self.history.push(format!("  → {}", m.label()));
            self.clear_prompt();
        }
    }

    /// Shared value of a `Style` getter across `targets`, or `None` when
    /// they disagree (rendered as `*VARIES*`). `targets` is assumed
    /// non-empty (the caller guards that).
    fn shared_style<T: PartialEq + Copy>(
        &self,
        targets: &[usize],
        get: impl Fn(&Style) -> T,
    ) -> Option<T> {
        let mut it = targets
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i))
            .map(|d| get(&d.style));
        let first = it.next()?;
        if it.all(|v| v == first) {
            Some(first)
        } else {
            None
        }
    }

    /// Apply a mutation to every target dobject, optionally taking ONE
    /// undo snapshot first, and flag the caches dirty. Used for every
    /// Properties-dialog edit so a single change = a single undo step.
    fn props_apply(&mut self, targets: &[usize], snapshot: bool, mut f: impl FnMut(&mut DObject)) {
        if snapshot {
            self.snapshot_doc();
        }
        for &i in targets {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                f(d);
            }
        }
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// Small integer tag for a Geom variant, used to decide whether the
    /// selection is homogeneous (and therefore shows type-specific
    /// "respected style" + non-coordinate properties).
    /// Small integer tag for a Geom variant, used to decide whether the
    /// selection is homogeneous (and therefore shows type-specific
    /// "respected style" + non-coordinate properties).
    fn geom_tag(g: &Geom) -> u8 {
        match g {
            Geom::Line(_) => 0,
            Geom::Circle(_) => 1,
            Geom::Arc(_) => 2,
            Geom::Ellipse(_) => 3,
            Geom::EllipseArc(_) => 4,
            Geom::Point(_) => 5,
            Geom::Polyline(_) => 6,
            Geom::Hatch(_) => 7,
            Geom::Spline(_) => 8,
            Geom::Wall(_) => 9,
            Geom::Text(_) => 10,
            Geom::Dimension(_) => 11,
            Geom::BlockRef(_) => 12,
            // Merged-in RUST-AutoRASM variants: no dedicated property UI in
            // this build yet, so they get a generic tag.
            _ => 13,
        }
    }

    /// `Some(tag)` when every target shares one Geom variant; `None` for a
    /// mixed-type selection (only general properties apply then).
    fn targets_uniform_tag(&self, targets: &[usize]) -> Option<u8> {
        let mut it = targets
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i))
            .map(|d| Self::geom_tag(&d.geom));
        let first = it.next()?;
        if it.all(|t| t == first) {
            Some(first)
        } else {
            None
        }
    }

    /// Snapshot-once-per-gesture guard for numeric/text fields: snapshots
    /// on the FIRST changed frame of a drag/type, resets when the gesture
    /// ends (drag_stopped / lost_focus). Returns true if a snapshot was
    /// just taken (so the caller doesn't double-snapshot).
    fn props_gesture_snapshot(&mut self, resp: &egui::Response) {
        if resp.changed() && !self.props_edit_gesture {
            self.snapshot_doc();
            self.props_edit_gesture = true;
        }
        if resp.drag_stopped() || resp.lost_focus() {
            self.props_edit_gesture = false;
        }
    }

    /// The whole Properties body for a non-empty target set. Single = one
    /// target (all non-coordinate props + respected style + read-only
    /// geometry); group = N targets (common General props with `*VARIES*`,
    /// plus respected style when the selection is homogeneous).
    fn render_props_body(&mut self, ui: &mut egui::Ui, targets: &[usize], query: &str) {
        let n = targets.len();
        let uniform = self.targets_uniform_tag(targets);

        // ---- GENERAL (common Style) ----------------------------------
        pp_section(ui, "pp_sec_general", "GENERAL", true, false, |ui| {
            self.render_props_general(ui, targets, query);
        });

        // ---- GEOMETRY (per-type coordinate pairs + read-only derived) -
        if n == 1 {
            pp_section(ui, "pp_sec_geometry", "GEOMETRY", true, true, |ui| {
                self.render_props_geometry(ui, targets[0]);
            });
        } else if uniform.is_none() {
            ui.add_space(2.0);
            ui.colored_label(
                crate::theme::color::TEXT_MUTED,
                "Mixed types — only general properties apply.",
            );
            ui.add_space(4.0);
        }

        // ---- Type-specific extras (hatch fill, etc.) -----------------
        if let Some(tag) = uniform {
            if matches!(tag, 7) {
                pp_section(ui, "pp_sec_hatch", "HATCH", false, true, |ui| {
                    self.render_props_type_specific(ui, targets, tag);
                });
            }
        }

        // ---- MISC (handle / index) -----------------------------------
        if n == 1 {
            pp_section(ui, "pp_sec_misc", "MISC", false, true, |ui| {
                let idx = targets[0];
                let handle = self.doc.dobjects.get(idx).map(|d| d.handle).unwrap_or(0);
                pp_kv(ui, "Handle", &format!("0x{:X}", handle));
                pp_kv(ui, "Index", &format!("#{}", idx));
            });
        }
    }

    /// GEOMETRY section — per-type coordinate pairs (editable) + read-only
    /// derived values (Length/Angle/Area), laid out per the finalized design.
    fn render_props_geometry(&mut self, ui: &mut egui::Ui, idx: usize) {
        let geom = match self.doc.dobjects.get(idx) {
            Some(d) => d.geom.clone(),
            None => return,
        };
        let targets = [idx];
        match geom {
            Geom::Line(l) => {
                pp_pair_headers(ui, "Start", "End");
                let (mut ax, mut bx) = (l.a.x, l.b.x);
                let (cx, rx) = pp_pair_row(ui, "X", &mut ax, &mut bx);
                for r in &rx {
                    self.props_gesture_snapshot(r);
                }
                if cx {
                    self.props_apply(&targets, false, |d| {
                        if let Geom::Line(l) = &mut d.geom {
                            l.a.x = ax;
                            l.b.x = bx;
                        }
                    });
                }
                let (mut ay, mut by) = (l.a.y, l.b.y);
                let (cy, ry) = pp_pair_row(ui, "Y", &mut ay, &mut by);
                for r in &ry {
                    self.props_gesture_snapshot(r);
                }
                if cy {
                    self.props_apply(&targets, false, |d| {
                        if let Geom::Line(l) = &mut d.geom {
                            l.a.y = ay;
                            l.b.y = by;
                        }
                    });
                }
                pp_kv(ui, "Length", &format!("{:.2}", (l.b - l.a).len()));
                pp_kv(
                    ui,
                    "Angle",
                    &format!("{:.2}°", (l.b - l.a).angle().to_degrees()),
                );
            }
            Geom::Circle(c) => {
                pp_pair_headers(ui, "X", "Y");
                let (mut cx, mut cy) = (c.center.x, c.center.y);
                let (cc, rr) = pp_pair_row(ui, "Center", &mut cx, &mut cy);
                for r in &rr {
                    self.props_gesture_snapshot(r);
                }
                if cc {
                    self.props_apply(&targets, false, |d| {
                        if let Geom::Circle(c) = &mut d.geom {
                            c.center.x = cx;
                            c.center.y = cy;
                        }
                    });
                }
                let mut r = c.radius;
                let (cr, rp) = pp_num_row(ui, "Radius", &mut r);
                self.props_gesture_snapshot(&rp);
                if cr {
                    self.props_apply(&targets, false, |d| {
                        if let Geom::Circle(c) = &mut d.geom {
                            c.radius = r.max(0.0);
                        }
                    });
                }
                pp_kv(
                    ui,
                    "Area",
                    &format!("{:.2}", std::f64::consts::PI * c.radius * c.radius),
                );
            }
            other => {
                ui.add_space(2.0);
                ui.monospace(describe(&other));
            }
        }
    }

    /// Per-Geom-variant count, e.g. "lines: 3  circles: 1".
    fn props_kind_breakdown(&self, targets: &[usize]) -> String {
        let mut counts: std::collections::BTreeMap<&'static str, usize> =
            std::collections::BTreeMap::new();
        for &i in targets {
            if let Some(d) = self.doc.dobjects.get(i) {
                *counts.entry(dobject_kind_name(&d.geom)).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect::<Vec<_>>()
            .join("   ")
    }

    /// The common-Style editor (layer / color / linetype / lt-scale /
    /// lineweight / visible). Each control shows the shared value or
    /// `*VARIES*`; editing writes to EVERY target with one undo step.
    /// Resolve a `Color` to an on-screen swatch colour (ByLayer follows the
    /// given layer's colour). App-layer only — for the Properties swatches.
    pub(super) fn resolve_color32(&self, c: Color, layer: LayerId) -> egui::Color32 {
        let tc = |idx: u16| -> (u8, u8, u8) {
            let v = self.doc.truecolors.get(idx).unwrap_or(0xFFFFFF);
            (
                ((v >> 16) & 0xFF) as u8,
                ((v >> 8) & 0xFF) as u8,
                (v & 0xFF) as u8,
            )
        };
        let (r, g, b) = match c {
            Color::Aci(i) => aci_palette(i),
            Color::TrueColorRef(idx) => tc(idx),
            Color::ByBlock => (140, 140, 160),
            Color::ByLayer => match self.doc.layers.get(layer).map(|l| l.color) {
                Some(Color::Aci(i)) => aci_palette(i),
                Some(Color::TrueColorRef(idx)) => tc(idx),
                _ => (180, 180, 200),
            },
        };
        egui::Color32::from_rgb(r, g, b)
    }

    fn render_props_general(&mut self, ui: &mut egui::Ui, targets: &[usize], query: &str) {
        let q = query.trim().to_lowercase();
        let show = |label: &str| q.is_empty() || label.to_lowercase().contains(&q);

        let layer0 = self.shared_style(targets, |s| s.layer).unwrap_or(0);

        // ---- Layer (swatch = actual layer colour, click → layer list) ----
        if show("Layer") {
            let shared_layer = self.shared_style(targets, |s| s.layer);
            let (name, lcol) = match shared_layer {
                Some(lid) => (
                    self.doc
                        .layers
                        .get(lid)
                        .map(|l| l.name.clone())
                        .unwrap_or_else(|| "?".into()),
                    self.resolve_color32(Color::ByLayer, lid),
                ),
                None => ("*VARIES*".into(), PP_MUTED),
            };
            let layers: Vec<(LayerId, String)> = (0..self.doc.layers.len() as LayerId)
                .filter_map(|lid| self.doc.layers.get(lid).map(|l| (lid, l.name.clone())))
                .collect();
            let mut pick: Option<LayerId> = None;
            pp_row(ui, "Layer", |ui, w| {
                let (rect, resp) = pp_box(ui, w, true);
                let p = ui.painter_at(rect);
                let sw = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 8.0, rect.center().y - 6.5),
                    egui::vec2(13.0, 13.0),
                );
                p.rect_filled(sw, egui::Rounding::same(crate::theme::radius::XS), lcol);
                pp_capture(
                    ui,
                    &format!("Layer · swatch  size=13 radius=2 color={}", pp_hex(lcol)),
                    sw,
                );
                let tr = p.text(
                    egui::pos2(sw.right() + 9.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &name,
                    crate::theme::typ::body(),
                    PP_TEXT,
                );
                pp_arrow(&p, rect);
                pp_cap_field(ui, "Layer", rect, tr, &name, PP_TEXT, 13.0, true);
                let pid = ui.make_persistent_id("pp_layer_popup");
                if resp.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(pid));
                }
                egui::popup_below_widget(
                    ui,
                    pid,
                    &resp,
                    egui::PopupCloseBehavior::CloseOnClick,
                    |ui| {
                        ui.set_min_width(w);
                        for (lid, nm) in &layers {
                            if ui
                                .selectable_label(shared_layer == Some(*lid), nm)
                                .clicked()
                            {
                                pick = Some(*lid);
                            }
                        }
                    },
                );
            });
            if let Some(lid) = pick {
                self.props_apply(targets, true, |d| d.style.layer = lid);
            }
        }

        // ---- Color (swatch + label box, click → ACI wheel) ----
        if show("Color") {
            let shared_color = self.shared_style(targets, |s| s.color);
            let (label, c32, italic) = match shared_color {
                Some(Color::ByLayer) => (
                    "By Layer".to_string(),
                    self.resolve_color32(Color::ByLayer, layer0),
                    true,
                ),
                Some(Color::ByBlock) => (
                    "By Block".to_string(),
                    egui::Color32::from_rgb(140, 140, 160),
                    true,
                ),
                Some(Color::Aci(i)) => (
                    format!("ACI {}", i),
                    self.resolve_color32(Color::Aci(i), layer0),
                    false,
                ),
                Some(c @ Color::TrueColorRef(_)) => {
                    let cc = self.resolve_color32(c, layer0);
                    (
                        format!("RGB #{:02X}{:02X}{:02X}", cc.r(), cc.g(), cc.b()),
                        cc,
                        false,
                    )
                }
                None => ("*VARIES*".to_string(), PP_MUTED, true),
            };
            let mut open_wheel = false;
            pp_row(ui, "Color", |ui, w| {
                let (rect, resp) = pp_box(ui, w, true);
                let p = ui.painter_at(rect);
                let sw = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 8.0, rect.center().y - 6.5),
                    egui::vec2(13.0, 13.0),
                );
                p.rect(
                    sw,
                    egui::Rounding::same(crate::theme::radius::XS),
                    c32,
                    egui::Stroke::new(1.0, PP_BORDER),
                );
                pp_capture(
                    ui,
                    &format!("Color · swatch  size=13 radius=2 color={}", pp_hex(c32)),
                    sw,
                );
                let col = if italic { PP_MUTED } else { PP_TEXT };
                let tr = p.text(
                    egui::pos2(sw.right() + 9.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &label,
                    crate::theme::typ::body(),
                    col,
                );
                pp_arrow(&p, rect);
                pp_cap_field(ui, "Color", rect, tr, &label, col, 13.0, true);
                let resp = resp.on_hover_text("Click to choose a colour (ACI / TrueColor)");
                if resp.clicked() {
                    open_wheel = true;
                }
            });
            if open_wheel {
                self.aci_pick_many = targets.to_vec();
                self.aci_pick_request = Some(AciPickRequest::DobjectMany);
            }
        }

        // ---- Linetype (name + dash preview, click → list) ----
        if show("Linetype") || show("Line Type") {
            let shared_lt = self.shared_style(targets, |s| s.linetype);
            let name = match shared_lt {
                Some(id) => self
                    .doc
                    .linetypes
                    .get(id)
                    .map(|l| l.name.clone())
                    .unwrap_or_else(|| "?".into()),
                None => "*VARIES*".into(),
            };
            let solid = matches!(shared_lt, Some(id)
                if self.doc.linetypes.get(id).map(|l| l.pattern.is_empty()).unwrap_or(true));
            // Abbreviated field name (+ full name for the hover tooltip).
            let (full, abbr) = match shared_lt {
                Some(_) => (name.clone(), abbrev_linetype(&name)),
                None => ("Mixed".to_string(), "Mixed".to_string()),
            };
            let lts: Vec<(u32, String)> = (0..self.doc.linetypes.len() as u32)
                .filter_map(|id| self.doc.linetypes.get(id).map(|l| (id, l.name.clone())))
                .collect();
            let mut pick: Option<u32> = None;
            pp_row(ui, "Line Type", |ui, w| {
                let (rect, resp) = pp_box(ui, w, true);
                let resp = resp.on_hover_text(full.as_str());
                let p = ui.painter_at(rect);
                let y = rect.center().y;
                // Preview FIRST: dash/solid stroke of the shared length L, then
                // 9px, then the abbreviated name (INSPECTOR_DESIGN_MENTOR §5).
                let x0 = rect.left() + crate::theme::space::INPUT_PAD;
                let l = pp_preview_len(rect.width());
                let s = egui::Stroke::new(1.5, PP_TEXT);
                if solid {
                    p.line_segment([egui::pos2(x0, y), egui::pos2(x0 + l, y)], s);
                } else {
                    let x1 = x0 + l;
                    let mut x = x0;
                    while x < x1 {
                        let e = (x + 7.0).min(x1);
                        p.line_segment([egui::pos2(x, y), egui::pos2(e, y)], s);
                        x += 11.0;
                    }
                }
                let name_x = x0 + l + 9.0;
                let tr = p.text(
                    egui::pos2(name_x, y),
                    egui::Align2::LEFT_CENTER,
                    &abbr,
                    crate::theme::typ::body(),
                    PP_TEXT,
                );
                pp_arrow(&p, rect);
                pp_cap_field(ui, "Line Type", rect, tr, &abbr, PP_TEXT, 13.0, true);
                let pid = ui.make_persistent_id("pp_lt_popup");
                if resp.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(pid));
                }
                egui::popup_below_widget(
                    ui,
                    pid,
                    &resp,
                    egui::PopupCloseBehavior::CloseOnClick,
                    |ui| {
                        ui.set_min_width(w);
                        for (id, nm) in &lts {
                            if ui.selectable_label(shared_lt == Some(*id), nm).clicked() {
                                pick = Some(*id);
                            }
                        }
                    },
                );
            });
            if let Some(id) = pick {
                self.props_apply(targets, true, |d| d.style.linetype = id);
            }
        }

        // ---- Lt Scale (numeric, value left-aligned in the box) ----
        if show("Lt Scale") || show("Linetype scale") {
            let shared_sc = self.shared_style(targets, |s| s.linetype_scale);
            let buf_id = egui::Id::new("pp_ltscale_buf");
            pp_row(ui, "Lt Scale", |ui, w| {
                // Painted box + a FRAMELESS editor placed centered inside it, so
                // the number is vertically centered (a plain add_sized TextEdit
                // top-aligns its text in the 24px box — the "1.000 too high" bug).
                let (rect, _) = pp_box(ui, w, false);
                let disp = shared_sc.map(|v| format!("{:.3}", v)).unwrap_or_default();
                let mut buf = ui
                    .data_mut(|d| d.get_temp::<String>(buf_id))
                    .unwrap_or_else(|| disp.clone());
                let inner = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 8.0, rect.center().y - 8.0),
                    egui::vec2((w - 16.0).max(10.0), 16.0),
                );
                let r = ui.put(
                    inner,
                    egui::TextEdit::singleline(&mut buf)
                        .frame(false)
                        .hint_text("≠"),
                );
                self.props_gesture_snapshot(&r);
                if r.changed() {
                    if let Ok(v) = buf.trim().parse::<f32>() {
                        self.props_apply(targets, false, |d| {
                            d.style.linetype_scale = v.clamp(0.01, 1000.0)
                        });
                    }
                }
                if !r.has_focus() {
                    buf = disp.clone();
                }
                ui.data_mut(|d| d.insert_temp(buf_id, buf));
                pp_cap_field(ui, "Lt Scale", rect, inner, &disp, PP_TEXT, 13.0, false);
            });
        }

        // ---- Lineweight (click → list) ----
        if show("Lineweight") || show("Line Weight") {
            let shared_lw = self.shared_style(targets, |s| s.lineweight);
            let lw_text = match shared_lw {
                None => "*VARIES*".to_string(),
                Some(Lineweight::ByLayer) => "ByLayer".to_string(),
                Some(Lineweight::ByBlock) => "ByBlock".to_string(),
                Some(Lineweight::Default) => "Default".to_string(),
                Some(Lineweight::Custom(mm)) => format!("{:.2} mm", mm),
            };
            let mut pick: Option<Lineweight> = None;
            pp_row(ui, "Line Weight", |ui, w| {
                let (rect, resp) = pp_box(ui, w, true);
                let p = ui.painter_at(rect);
                let y = rect.center().y;
                // Thickness bar FIRST (same length L as the linetype dash so they
                // stay matched); height encodes the weight, capped ~4px. Then the
                // value in Mono 12 (INSPECTOR_DESIGN_MENTOR §5).
                let x0 = rect.left() + crate::theme::space::INPUT_PAD;
                let l = pp_preview_len(rect.width());
                let h = match shared_lw {
                    Some(Lineweight::Custom(mm)) => (mm * 4.0).clamp(1.0, 4.0),
                    _ => 1.0,
                };
                p.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x0, y - h / 2.0),
                        egui::pos2(x0 + l, y + h / 2.0),
                    ),
                    0.0,
                    PP_TEXT,
                );
                let vx = x0 + l + 9.0;
                let tr = p.text(
                    egui::pos2(vx, y),
                    egui::Align2::LEFT_CENTER,
                    &lw_text,
                    crate::theme::typ::data_value(),
                    PP_TEXT,
                );
                pp_arrow(&p, rect);
                pp_cap_field(ui, "Line Weight", rect, tr, &lw_text, PP_TEXT, 12.0, true);
                let pid = ui.make_persistent_id("pp_lw_popup");
                if resp.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(pid));
                }
                egui::popup_below_widget(
                    ui,
                    pid,
                    &resp,
                    egui::PopupCloseBehavior::CloseOnClick,
                    |ui| {
                        ui.set_min_width(w);
                        if ui
                            .selectable_label(shared_lw == Some(Lineweight::ByLayer), "ByLayer")
                            .clicked()
                        {
                            pick = Some(Lineweight::ByLayer);
                        }
                        if ui
                            .selectable_label(shared_lw == Some(Lineweight::ByBlock), "ByBlock")
                            .clicked()
                        {
                            pick = Some(Lineweight::ByBlock);
                        }
                        if ui
                            .selectable_label(shared_lw == Some(Lineweight::Default), "Default")
                            .clicked()
                        {
                            pick = Some(Lineweight::Default);
                        }
                        for mm in [0.05_f32, 0.13, 0.18, 0.25, 0.35, 0.5, 0.7, 1.0, 1.4, 2.0] {
                            let is_this = matches!(shared_lw, Some(Lineweight::Custom(x)) if (x - mm).abs() < 1e-3);
                            if ui
                                .selectable_label(is_this, format!("{:.2} mm", mm))
                                .clicked()
                            {
                                pick = Some(Lineweight::Custom(mm));
                            }
                        }
                    },
                );
            });
            if let Some(v) = pick {
                self.props_apply(targets, true, |d| d.style.lineweight = v);
            }
        }

        // ---- Visible (16×16 checkbox — INSPECTOR_DESIGN_MENTOR §5) ----
        if show("Visible") {
            let shared_vis = self.shared_style(targets, |s| s.visible);
            let on = matches!(shared_vis, Some(true));
            let mixed = shared_vis.is_none();
            let mut toggle = false;
            pp_row(ui, "Visible", |ui, w| {
                // 16×16 box at the value start; the rest of the row is empty.
                let (cell, resp) =
                    ui.allocate_exact_size(egui::vec2(w, PP_ROW_H), egui::Sense::click());
                let sz = 16.0;
                let bx = egui::Rect::from_min_size(
                    egui::pos2(cell.left(), cell.center().y - sz / 2.0),
                    egui::vec2(sz, sz),
                );
                let p = ui.painter_at(cell);
                let r4 = egui::Rounding::same(crate::theme::radius::SM);
                let on_acc = crate::theme::color::ON_ACCENT;
                if on || mixed {
                    p.rect(bx, r4, PP_ACCENT, egui::Stroke::new(1.0, PP_ACCENT));
                    if on {
                        // check glyph in on-accent
                        let c = bx.center();
                        p.line_segment(
                            [
                                egui::pos2(bx.left() + 3.5, c.y + 0.5),
                                egui::pos2(bx.left() + 6.5, c.y + 3.5),
                            ],
                            egui::Stroke::new(1.6, on_acc),
                        );
                        p.line_segment(
                            [
                                egui::pos2(bx.left() + 6.5, c.y + 3.5),
                                egui::pos2(bx.right() - 3.0, c.y - 3.5),
                            ],
                            egui::Stroke::new(1.6, on_acc),
                        );
                    } else {
                        // indeterminate (Mixed): a horizontal dash in on-accent
                        let c = bx.center();
                        p.line_segment(
                            [
                                egui::pos2(bx.left() + 3.5, c.y),
                                egui::pos2(bx.right() - 3.5, c.y),
                            ],
                            egui::Stroke::new(2.0, on_acc),
                        );
                    }
                } else {
                    let edge = if resp.hovered() { PP_ACCENT } else { PP_BORDER };
                    p.rect(bx, r4, PP_BG_LO, egui::Stroke::new(1.0, edge));
                }
                pp_capture(
                    ui,
                    &format!(
                        "Visible · checkbox 16x16 radius=4 state={}",
                        if on {
                            "on"
                        } else if mixed {
                            "mixed"
                        } else {
                            "off"
                        }
                    ),
                    bx,
                );
                if resp.clicked() {
                    toggle = true;
                }
            });
            if toggle {
                let nv = !matches!(shared_vis, Some(true));
                self.props_apply(targets, true, |d| d.style.visible = nv);
            }
        }
    }

    /// Type-specific "respected style" + non-coordinate properties for a
    /// homogeneous selection (`tag` from `geom_tag`). Coordinates are
    /// excluded — only the editable non-geometry fields per variant.
    fn render_props_type_specific(&mut self, ui: &mut egui::Ui, targets: &[usize], tag: u8) {
        // Line: Length + Angle laid out in two columns (small caps label over
        // a numeric field), like the mockup's paired fields.
        if tag == 0 {
            let shared_len = self.props_shared_geom(targets, |g| {
                if let Geom::Line(l) = g {
                    Some(((l.b - l.a).len() * 1e6).round() as i64)
                } else {
                    None
                }
            });
            let shared_ang = self.props_shared_geom(targets, |g| {
                if let Geom::Line(l) = g {
                    Some(((l.b - l.a).angle().to_degrees() * 1e4).round() as i64)
                } else {
                    None
                }
            });
            let mut len = shared_len.map(|x| x as f64 / 1e6).unwrap_or(0.0);
            let mut ang = shared_ang.map(|x| x as f64 / 1e4).unwrap_or(0.0);
            let mut len_set: Option<f64> = None;
            let mut ang_set: Option<f64> = None;
            let cap = |t: &str| egui::RichText::new(t).monospace().size(9.0).color(PP_MUTED);
            let varies = |ui: &mut egui::Ui, half: f32, name: &str| {
                let (rect, _) = pp_box(ui, half, false);
                ui.painter_at(rect).text(
                    egui::pos2(rect.left() + 8.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "various",
                    crate::theme::typ::body(),
                    PP_MUTED,
                );
                pp_capture(ui, name, rect);
            };
            ui.horizontal(|ui| {
                let half = ((ui.available_width() - 8.0) / 2.0).max(50.0);
                ui.vertical(|ui| {
                    ui.set_width(half);
                    ui.label(cap("LENGTH"));
                    ui.add_space(2.0);
                    if shared_len.is_none() {
                        varies(ui, half, "LENGTH · value");
                    } else {
                        let r = ui.add_sized(
                            [half, PP_ROW_H],
                            egui::DragValue::new(&mut len)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(0.0..=1e12),
                        );
                        pp_capture(ui, "LENGTH · value", r.rect);
                        self.props_gesture_snapshot(&r);
                        if r.changed() {
                            len_set = Some(len);
                        }
                    }
                });
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.set_width(half);
                    ui.label(cap("ANGLE"));
                    ui.add_space(2.0);
                    if shared_ang.is_none() {
                        varies(ui, half, "ANGLE · value");
                    } else {
                        let r = ui.add_sized(
                            [half, PP_ROW_H],
                            egui::DragValue::new(&mut ang)
                                .update_while_editing(false)
                                .speed(0.5)
                                .range(-360.0..=360.0)
                                .suffix("°"),
                        );
                        pp_capture(ui, "ANGLE · value", r.rect);
                        self.props_gesture_snapshot(&r);
                        if r.changed() {
                            ang_set = Some(ang);
                        }
                    }
                });
            });
            if let Some(v) = len_set {
                self.props_apply(targets, false, |d| {
                    if let Geom::Line(l) = &mut d.geom {
                        let dir = l.b - l.a;
                        let cur = dir.len();
                        l.b = if cur > 1e-12 {
                            l.a + dir * (v / cur)
                        } else {
                            l.a + Vec2::new(v, 0.0)
                        };
                    }
                });
            }
            if let Some(v) = ang_set {
                self.props_apply(targets, false, |d| {
                    if let Geom::Line(l) = &mut d.geom {
                        let ln = (l.b - l.a).len();
                        let r = v.to_radians();
                        l.b = l.a + Vec2::new(r.cos() * ln, r.sin() * ln);
                    }
                });
            }
            return;
        }

        egui::Grid::new("props_type")
            .num_columns(2)
            .spacing([12.0, 5.0])
            .show(ui, |ui| {
                match tag {
                    // ---- Point: PDMODE style + size ----
                    5 => {
                        ui.label("Point style");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::Point(p) = g {
                                Some(p.style as i64)
                            } else {
                                None
                            }
                        });
                        let mut v = shared.unwrap_or(0) as u8;
                        let resp = ui.add(
                            egui::DragValue::new(&mut v)
                                .update_while_editing(false)
                                .range(0..=99)
                                .prefix(if shared.is_none() { "≠ " } else { "" }),
                        );
                        if resp.changed() {
                            self.props_gesture_snapshot(&resp);
                            self.props_apply(targets, false, |d| {
                                if let Geom::Point(p) = &mut d.geom {
                                    p.style = v;
                                }
                            });
                        } else {
                            self.props_gesture_snapshot(&resp);
                        }
                        ui.end_row();

                        ui.label("Point size");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::Point(p) = g {
                                Some((p.size * 1000.0) as i64)
                            } else {
                                None
                            }
                        });
                        let mut sz = shared.map(|x| x as f32 / 1000.0).unwrap_or(0.0);
                        let resp = ui.add(
                            egui::DragValue::new(&mut sz)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(0.0..=1e6)
                                .prefix(if shared.is_none() { "≠ " } else { "" }),
                        );
                        if resp.changed() {
                            self.props_gesture_snapshot(&resp);
                            self.props_apply(targets, false, |d| {
                                if let Geom::Point(p) = &mut d.geom {
                                    p.size = sz;
                                }
                            });
                        } else {
                            self.props_gesture_snapshot(&resp);
                        }
                        ui.end_row();
                    }
                    // ---- Polyline: closed flag ----
                    6 => {
                        ui.label("Closed");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::Polyline(p) = g {
                                Some(p.closed as i64)
                            } else {
                                None
                            }
                        });
                        let mut closed = shared.map(|x| x != 0).unwrap_or(false);
                        let label = if shared.is_none() {
                            "(varies)"
                        } else if closed {
                            "closed"
                        } else {
                            "open"
                        };
                        let resp = ui.checkbox(&mut closed, label);
                        if resp.changed() {
                            self.props_apply(targets, true, |d| {
                                if let Geom::Polyline(p) = &mut d.geom {
                                    p.closed = closed;
                                }
                            });
                        }
                        ui.end_row();
                    }
                    // ---- Hatch: name + swatch + scale + angle (colour = General) ----
                    7 => {
                        let name = self
                            .props_first_geom_string(targets, |g| {
                                if let Geom::Hatch(h) = g {
                                    Some(match &h.pattern {
                                        cad_kernel::HatchPattern::Solid => "Solid".to_string(),
                                        cad_kernel::HatchPattern::Pattern { name, .. } => {
                                            name.clone()
                                        }
                                    })
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| "—".into());

                        // Name + small pattern preview swatch.
                        ui.label("Pattern");
                        ui.horizontal(|ui| {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(36.0, 16.0), egui::Sense::hover());
                            let p = ui.painter_at(rect);
                            p.rect_filled(
                                rect,
                                egui::Rounding::ZERO,
                                egui::Color32::from_rgb(0x14, 0x1c, 0x25),
                            );
                            let edge =
                                egui::Stroke::new(1.0, egui::Color32::from_rgb(0x3b, 0x49, 0x4c));
                            p.line_segment([rect.left_top(), rect.right_top()], edge);
                            p.line_segment([rect.left_bottom(), rect.right_bottom()], edge);
                            p.line_segment([rect.left_top(), rect.left_bottom()], edge);
                            p.line_segment([rect.right_top(), rect.right_bottom()], edge);
                            let ink = egui::Color32::from_rgb(0xba, 0xc9, 0xcc);
                            if name == "Solid" {
                                p.rect_filled(rect.shrink(3.0), egui::Rounding::ZERO, ink);
                            } else {
                                let s = egui::Stroke::new(1.0, ink);
                                let mut x = rect.left() - rect.height();
                                while x < rect.right() {
                                    p.line_segment(
                                        [
                                            egui::pos2(x, rect.bottom()),
                                            egui::pos2(x + rect.height(), rect.top()),
                                        ],
                                        s,
                                    );
                                    x += 6.0;
                                }
                            }
                            ui.monospace(&name);
                        });
                        ui.end_row();

                        // Scale + Angle apply only to a real Pattern (not Solid).
                        let is_pattern = self
                            .props_first_geom_string(targets, |g| {
                                if let Geom::Hatch(h) = g {
                                    if let cad_kernel::HatchPattern::Pattern { .. } = &h.pattern {
                                        Some("y".to_string())
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            })
                            .is_some();
                        if is_pattern {
                            ui.label("Scale");
                            let shared = self.props_shared_geom(targets, |g| {
                                if let Geom::Hatch(h) = g {
                                    if let cad_kernel::HatchPattern::Pattern { scale, .. } =
                                        &h.pattern
                                    {
                                        Some((*scale * 1e6).round() as i64)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            });
                            let mut sc = shared.map(|x| x as f64 / 1e6).unwrap_or(1.0);
                            let resp = ui.add(
                                egui::DragValue::new(&mut sc)
                                    .update_while_editing(false)
                                    .speed(0.05)
                                    .range(0.001..=1e6)
                                    .prefix(if shared.is_none() { "≠ " } else { "" }),
                            );
                            if resp.changed() {
                                self.props_gesture_snapshot(&resp);
                                self.props_apply(targets, false, |d| {
                                    if let Geom::Hatch(h) = &mut d.geom {
                                        if let cad_kernel::HatchPattern::Pattern { scale, .. } =
                                            &mut h.pattern
                                        {
                                            *scale = sc;
                                        }
                                    }
                                });
                            } else {
                                self.props_gesture_snapshot(&resp);
                            }
                            ui.end_row();

                            ui.label("Angle");
                            let shared = self.props_shared_geom(targets, |g| {
                                if let Geom::Hatch(h) = g {
                                    if let cad_kernel::HatchPattern::Pattern { angle_deg, .. } =
                                        &h.pattern
                                    {
                                        Some((*angle_deg * 1e4).round() as i64)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            });
                            let mut ang = shared.map(|x| x as f64 / 1e4).unwrap_or(0.0);
                            let resp = ui.add(
                                egui::DragValue::new(&mut ang)
                                    .update_while_editing(false)
                                    .speed(0.5)
                                    .range(-360.0..=360.0)
                                    .suffix("°")
                                    .prefix(if shared.is_none() { "≠ " } else { "" }),
                            );
                            if resp.changed() {
                                self.props_gesture_snapshot(&resp);
                                self.props_apply(targets, false, |d| {
                                    if let Geom::Hatch(h) = &mut d.geom {
                                        if let cad_kernel::HatchPattern::Pattern {
                                            angle_deg, ..
                                        } = &mut h.pattern
                                        {
                                            *angle_deg = ang;
                                        }
                                    }
                                });
                            } else {
                                self.props_gesture_snapshot(&resp);
                            }
                            ui.end_row();
                        }

                        ui.label("");
                        ui.small("Colour: General ▸ Color.  Background / origin / type: planned.");
                        ui.end_row();
                    }
                    // ---- Spline: degree (read-only) + uniform ribbon width ----
                    // (tag 8 — the grid arm list skips straight from Hatch 7 to
                    // Wall 9; the spline section lives here.)
                    8 => {
                        let shared_deg = self.props_shared_geom(targets, |g| {
                            if let Geom::Spline(s) = g {
                                Some(s.degree as i64)
                            } else {
                                None
                            }
                        });
                        ui.label("Degree");
                        ui.monospace(
                            shared_deg
                                .map(|d| d.to_string())
                                .unwrap_or_else(|| "≠".into()),
                        );
                        ui.end_row();

                        // Ribbon WIDTH — one uniform value; 0 = thin. Editing
                        // re-renders the ribbon (the spline tool's `Width` option
                        // sets the default captured at commit).
                        ui.label("Width");
                        let shared_w = self.props_shared_geom(targets, |g| {
                            if let Geom::Spline(s) = g {
                                Some((s.width * 1e6).round() as i64)
                            } else {
                                None
                            }
                        });
                        let mut w = shared_w.map(|x| x as f64 / 1e6).unwrap_or(0.0);
                        let resp = ui.add(
                            egui::DragValue::new(&mut w)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(0.0..=1e9)
                                .prefix(if shared_w.is_none() { "≠ " } else { "" }),
                        );
                        if resp.changed() {
                            self.props_gesture_snapshot(&resp);
                            self.props_apply(targets, false, |d| {
                                if let Geom::Spline(s) = &mut d.geom {
                                    s.width = w.max(0.0);
                                }
                            });
                        } else {
                            self.props_gesture_snapshot(&resp);
                        }
                        ui.end_row();
                    }
                    // ---- Wall: wall style + thickness ----
                    9 => {
                        ui.label("Wall style");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::Wall(w) = g {
                                Some(w.style as i64)
                            } else {
                                None
                            }
                        });
                        let cur = match shared {
                            Some(id) => self
                                .doc
                                .wall_styles
                                .get(id as u32)
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| "?".into()),
                            None => "*VARIES*".into(),
                        };
                        let mut pick: Option<u32> = None;
                        egui::ComboBox::from_id_salt("props_wallstyle")
                            .selected_text(cur)
                            .show_ui(ui, |ui| {
                                for id in 0..(self.doc.wall_styles.len() as u32) {
                                    let name = match self.doc.wall_styles.get(id) {
                                        Some(s) => s.name.clone(),
                                        None => continue,
                                    };
                                    if ui
                                        .selectable_label(shared == Some(id as i64), name)
                                        .clicked()
                                    {
                                        pick = Some(id);
                                    }
                                }
                            });
                        if let Some(id) = pick {
                            let thick = self.doc.wall_styles.get(id).map(|s| s.thickness);
                            self.props_apply(targets, true, |d| {
                                if let Geom::Wall(w) = &mut d.geom {
                                    w.style = id;
                                    if let Some(t) = thick {
                                        if t > 0.0 {
                                            w.thickness = t;
                                        }
                                    }
                                }
                            });
                        }
                        ui.end_row();

                        ui.label("Thickness");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::Wall(w) = g {
                                Some((w.thickness * 1e6) as i64)
                            } else {
                                None
                            }
                        });
                        let mut th = shared.map(|x| x as f64 / 1e6).unwrap_or(0.0);
                        let resp = ui.add(
                            egui::DragValue::new(&mut th)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(0.001..=1e9)
                                .prefix(if shared.is_none() { "≠ " } else { "" }),
                        );
                        if resp.changed() {
                            self.props_gesture_snapshot(&resp);
                            self.props_apply(targets, false, |d| {
                                if let Geom::Wall(w) = &mut d.geom {
                                    w.thickness = th;
                                }
                            });
                        } else {
                            self.props_gesture_snapshot(&resp);
                        }
                        ui.end_row();
                    }
                    // ---- Text: text style + height + angle + align + content ----
                    10 => {
                        ui.label("Text style");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::Text(t) = g {
                                Some(t.style as i64)
                            } else {
                                None
                            }
                        });
                        let cur = match shared {
                            Some(id) => self
                                .doc
                                .text_styles
                                .get(id as u32)
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| "?".into()),
                            None => "*VARIES*".into(),
                        };
                        let mut pick: Option<u32> = None;
                        egui::ComboBox::from_id_salt("props_textstyle")
                            .selected_text(cur)
                            .show_ui(ui, |ui| {
                                for id in 0..(self.doc.text_styles.len() as u32) {
                                    let name = match self.doc.text_styles.get(id) {
                                        Some(s) => s.name.clone(),
                                        None => continue,
                                    };
                                    if ui
                                        .selectable_label(shared == Some(id as i64), name)
                                        .clicked()
                                    {
                                        pick = Some(id);
                                    }
                                }
                            });
                        if let Some(id) = pick {
                            self.props_apply(targets, true, |d| {
                                if let Geom::Text(t) = &mut d.geom {
                                    t.style = id;
                                }
                            });
                        }
                        ui.end_row();

                        // Font — per-entity override ("" = inherit the style's
                        // font, shown as `(inherit) <style font>`). Picking a font
                        // writes an EXPLICIT name; undo restores the previous one.
                        ui.label("Font");
                        let fonts: Vec<String> = targets
                            .iter()
                            .filter_map(|&i| self.doc.dobjects.get(i))
                            .map(|d| {
                                if let Geom::Text(t) = &d.geom {
                                    t.font_name.clone()
                                } else {
                                    String::new()
                                }
                            })
                            .collect();
                        let shared_font: Option<String> = fonts
                            .first()
                            .filter(|f0| fonts.iter().all(|f| f == *f0))
                            .cloned();
                        // Style-resolved font for the `(inherit)` display — the
                        // SAME resolver the renderer/TXTEXP use.
                        let style_font: Option<String> = {
                            let styles: Vec<u32> = targets
                                .iter()
                                .filter_map(|&i| self.doc.dobjects.get(i))
                                .filter_map(|d| {
                                    if let Geom::Text(t) = &d.geom {
                                        Some(t.style)
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            let mut it = styles.iter().map(|&sid| self.resolve_style_font(sid));
                            match it.next() {
                                Some(first) if it.all(|f| f == first) => Some(first),
                                _ => None,
                            }
                        };
                        let cur_font = match &shared_font {
                            Some(f) if !f.is_empty() => f.clone(),
                            Some(_) => {
                                format!("(inherit) {}", style_font.as_deref().unwrap_or("standard"))
                            }
                            None => "*VARIES*".into(),
                        };
                        let mut pick_font: Option<String> = None;
                        // System fonts cloned into a local BEFORE the ComboBox
                        // closure (the RefCell borrow must not outlive it); the
                        // name list itself is cached in the engine (no per-frame
                        // sort).
                        let mut font_choices: Vec<String> =
                            vec!["standard".into(), "monospace".into()];
                        {
                            let mut fm = self.font_manager.borrow_mut();
                            font_choices.extend_from_slice(fm.names());
                        }
                        egui::ComboBox::from_id_salt("props_textfont")
                            .selected_text(cur_font)
                            .show_ui(ui, |ui| {
                                for f in &font_choices {
                                    let selected = shared_font.as_deref() == Some(f.as_str());
                                    if ui.selectable_label(selected, f).clicked() {
                                        pick_font = Some(f.clone());
                                    }
                                }
                            });
                        if let Some(f) = pick_font {
                            self.props_apply(targets, true, |d| {
                                if let Geom::Text(t) = &mut d.geom {
                                    t.font_name = f.clone();
                                }
                            });
                        }
                        ui.end_row();

                        ui.label("Bold");
                        let shared_bold = self.props_shared_geom(targets, |g| {
                            if let Geom::Text(t) = g {
                                Some(t.bold as i64)
                            } else {
                                None
                            }
                        });
                        let mut bold = shared_bold.map(|x| x != 0).unwrap_or(false);
                        let resp = ui.checkbox(&mut bold, "");
                        if resp.changed() {
                            self.props_apply(targets, true, |d| {
                                if let Geom::Text(t) = &mut d.geom {
                                    t.bold = bold;
                                }
                            });
                        }
                        ui.end_row();

                        ui.label("Italic");
                        let shared_it = self.props_shared_geom(targets, |g| {
                            if let Geom::Text(t) = g {
                                Some((t.oblique.abs() > 1e-6) as i64)
                            } else {
                                None
                            }
                        });
                        let mut ital = shared_it.map(|x| x != 0).unwrap_or(false);
                        let resp = ui.checkbox(&mut ital, "");
                        if resp.changed() {
                            let shear = if ital { ITALIC_RAD } else { 0.0 };
                            self.props_apply(targets, true, |d| {
                                if let Geom::Text(t) = &mut d.geom {
                                    t.oblique = shear;
                                }
                            });
                        }
                        ui.end_row();

                        ui.label("Height");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::Text(t) = g {
                                Some((t.height * 1e6) as i64)
                            } else {
                                None
                            }
                        });
                        let mut h = shared.map(|x| x as f64 / 1e6).unwrap_or(1.0);
                        let resp = ui.add(
                            egui::DragValue::new(&mut h)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(0.001..=1e9)
                                .prefix(if shared.is_none() { "≠ " } else { "" }),
                        );
                        if resp.changed() {
                            self.props_gesture_snapshot(&resp);
                            self.props_apply(targets, false, |d| {
                                if let Geom::Text(t) = &mut d.geom {
                                    t.height = h;
                                }
                            });
                        } else {
                            self.props_gesture_snapshot(&resp);
                        }
                        ui.end_row();

                        ui.label("Angle°");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::Text(t) = g {
                                Some((t.angle.to_degrees() * 1e3) as i64)
                            } else {
                                None
                            }
                        });
                        let mut a = shared.map(|x| x as f64 / 1e3).unwrap_or(0.0);
                        let resp = ui.add(
                            egui::DragValue::new(&mut a)
                                .update_while_editing(false)
                                .speed(1.0)
                                .range(-360.0..=360.0)
                                .prefix(if shared.is_none() { "≠ " } else { "" }),
                        );
                        if resp.changed() {
                            self.props_gesture_snapshot(&resp);
                            let rad = a.to_radians();
                            self.props_apply(targets, false, |d| {
                                if let Geom::Text(t) = &mut d.geom {
                                    t.angle = rad;
                                }
                            });
                        } else {
                            self.props_gesture_snapshot(&resp);
                        }
                        ui.end_row();

                        // Content — only when a SINGLE text is selected.
                        if targets.len() == 1 {
                            ui.label("Text");
                            let idx = targets[0];
                            let mut s = if let Some(Geom::Text(t)) =
                                self.doc.dobjects.get(idx).map(|d| &d.geom)
                            {
                                t.text.clone()
                            } else {
                                String::new()
                            };
                            let resp = ui.text_edit_singleline(&mut s);
                            if resp.changed() {
                                self.props_gesture_snapshot(&resp);
                                self.props_apply(targets, false, |d| {
                                    if let Geom::Text(t) = &mut d.geom {
                                        t.text = s.clone();
                                    }
                                });
                            } else {
                                self.props_gesture_snapshot(&resp);
                            }
                            ui.end_row();
                        }
                    }
                    // ---- Dimension: dim style ----
                    11 => {
                        ui.label("Dim style");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::Dimension(d) = g {
                                Some(d.style as i64)
                            } else {
                                None
                            }
                        });
                        let cur = match shared {
                            Some(id) => self
                                .doc
                                .dim_styles
                                .get(id as u32)
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| "?".into()),
                            None => "*VARIES*".into(),
                        };
                        let mut pick: Option<u32> = None;
                        egui::ComboBox::from_id_salt("props_dimstyle")
                            .selected_text(cur)
                            .show_ui(ui, |ui| {
                                for id in 0..(self.doc.dim_styles.len() as u32) {
                                    let name = match self.doc.dim_styles.get(id) {
                                        Some(s) => s.name.clone(),
                                        None => continue,
                                    };
                                    if ui
                                        .selectable_label(shared == Some(id as i64), name)
                                        .clicked()
                                    {
                                        pick = Some(id);
                                    }
                                }
                            });
                        if let Some(id) = pick {
                            self.props_apply(targets, true, |d| {
                                if let Geom::Dimension(dd) = &mut d.geom {
                                    dd.style = id;
                                }
                            });
                        }
                        ui.end_row();
                    }
                    // ---- BlockRef: scale + rotation ----
                    12 => {
                        ui.label("Block");
                        let bname = targets
                            .iter()
                            .filter_map(|&i| self.doc.dobjects.get(i))
                            .find_map(|d| {
                                if let Geom::BlockRef(b) = &d.geom {
                                    self.doc.blocks.get(b.block).map(|blk| blk.name.clone())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| "?".into());
                        ui.monospace(bname);
                        ui.end_row();

                        ui.label("Scale");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::BlockRef(b) = g {
                                Some((b.scale * 1e6) as i64)
                            } else {
                                None
                            }
                        });
                        let mut sc = shared.map(|x| x as f64 / 1e6).unwrap_or(1.0);
                        let resp = ui.add(
                            egui::DragValue::new(&mut sc)
                                .update_while_editing(false)
                                .speed(0.05)
                                .range(0.0001..=1e9)
                                .prefix(if shared.is_none() { "≠ " } else { "" }),
                        );
                        if resp.changed() {
                            self.props_gesture_snapshot(&resp);
                            self.props_apply(targets, false, |d| {
                                if let Geom::BlockRef(b) = &mut d.geom {
                                    b.scale = sc;
                                }
                            });
                        } else {
                            self.props_gesture_snapshot(&resp);
                        }
                        ui.end_row();

                        ui.label("Rotation°");
                        let shared = self.props_shared_geom(targets, |g| {
                            if let Geom::BlockRef(b) = g {
                                Some((b.rotation.to_degrees() * 1e3) as i64)
                            } else {
                                None
                            }
                        });
                        let mut rot = shared.map(|x| x as f64 / 1e3).unwrap_or(0.0);
                        let resp = ui.add(
                            egui::DragValue::new(&mut rot)
                                .update_while_editing(false)
                                .speed(1.0)
                                .range(-360.0..=360.0)
                                .prefix(if shared.is_none() { "≠ " } else { "" }),
                        );
                        if resp.changed() {
                            self.props_gesture_snapshot(&resp);
                            let rad = rot.to_radians();
                            self.props_apply(targets, false, |d| {
                                if let Geom::BlockRef(b) = &mut d.geom {
                                    b.rotation = rad;
                                }
                            });
                        } else {
                            self.props_gesture_snapshot(&resp);
                        }
                        ui.end_row();

                        // Parametric PARAMETERS (single instance) — edit a value
                        // and the geometry derives live. This is the parametric
                        // block's payoff: set `width`, see the door resize.
                        if targets.len() == 1 {
                            let idx = targets[0];
                            let param_info: Vec<(String, f64)> = if let Some(Geom::BlockRef(br)) =
                                self.doc.dobjects.get(idx).map(|d| &d.geom)
                            {
                                self.doc
                                    .blocks
                                    .get(br.block)
                                    .map(|blk| {
                                        blk.params
                                            .iter()
                                            .enumerate()
                                            .map(|(k, p)| {
                                                (
                                                    p.name.clone(),
                                                    br.param_values
                                                        .get(k)
                                                        .copied()
                                                        .unwrap_or(p.original),
                                                )
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default()
                            } else {
                                Vec::new()
                            };
                            for (k, (pname, curval)) in param_info.into_iter().enumerate() {
                                ui.label(format!("◉ {}", pname));
                                let mut v = curval;
                                let resp = ui.add(
                                    egui::DragValue::new(&mut v)
                                        .update_while_editing(false)
                                        .speed(1.0),
                                );
                                if resp.changed() {
                                    self.props_gesture_snapshot(&resp);
                                    self.props_apply(targets, false, move |d| {
                                        if let Geom::BlockRef(b) = &mut d.geom {
                                            if k < cad_kernel::MAX_BLOCK_PARAMS {
                                                b.param_values[k] = v;
                                            }
                                        }
                                    });
                                } else {
                                    self.props_gesture_snapshot(&resp);
                                }
                                ui.end_row();
                            }
                        }
                    }
                    _ => {}
                }
            });
    }

    /// Shared integer-encoded geom property across targets, or `None` for
    /// varies. Callers encode f64/bool to a stable i64 so the equality
    /// check is exact (floats are scaled then truncated).
    fn props_shared_geom(
        &self,
        targets: &[usize],
        get: impl Fn(&Geom) -> Option<i64>,
    ) -> Option<i64> {
        let mut it = targets
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i))
            .filter_map(|d| get(&d.geom));
        let first = it.next()?;
        if it.all(|v| v == first) {
            Some(first)
        } else {
            None
        }
    }

    /// First non-None string property among targets (for read-only labels).
    fn props_first_geom_string(
        &self,
        targets: &[usize],
        get: impl Fn(&Geom) -> Option<String>,
    ) -> Option<String> {
        targets
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i))
            .find_map(|d| get(&d.geom))
    }

    // ===================================================================
    // Slice H — File I/O (DXF for now; .rsm in Slice I)
    // ===================================================================

    /// Draw the modal load/save progress overlay: a dimmed backdrop that absorbs input and a
    /// centred themed card with a spinner, the file name, a progress bar, and a time estimate.
    /// Uses the app's applied Visuals (via `theme::apply`), so it matches the rest of the UI.
    pub(super) fn render_busy_overlay(&self, ctx: &egui::Context) {
        let Some(b) = &self.busy else { return };
        let verb = match b.kind {
            BusyKind::Load => "Loading",
            BusyKind::Save => "Saving",
        };
        let elapsed = b.started.elapsed().as_secs_f32();
        let est = (b.est_ms as f32 / 1000.0).max(0.3);
        let frac = (elapsed / est).clamp(0.05, 0.95);
        let screen = ctx.screen_rect();

        // Dimmed backdrop — also swallows clicks so nothing behind it reacts mid-load.
        egui::Area::new(egui::Id::new("busy_backdrop"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
                ui.allocate_rect(screen, egui::Sense::click_and_drag());
            });

        egui::Window::new("busy_overlay")
            .order(egui::Order::Foreground)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(egui::Frame::window(&ctx.style()))
            .show(ctx, |ui| {
                ui.set_width(300.0);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(20.0));
                    ui.add_space(6.0);
                    ui.heading(format!("{verb}…"));
                });
                ui.add_space(2.0);
                ui.label(egui::RichText::new(&b.subtitle).weak());
                ui.add_space(10.0);
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(284.0)
                        .animate(true),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!(
                        "about {:.0} second(s) — please wait",
                        est.max(0.5).round()
                    ))
                    .small()
                    .weak(),
                );
                ui.add_space(8.0);
            });
        ctx.request_repaint(); // keep the spinner/bar alive while busy
    }

    /// Modal "unsaved changes" prompt shown when a close was vetoed. Save / Don't Save / Cancel.
    pub(super) fn render_close_confirm(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        // ESC IS ALWAYS A WAY OUT. A modal with no keyboard escape is one bug away from being a
        // hang, and this one WAS that bug — see below.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close_confirm = false;
            self.close_after_save = false;
            return;
        }
        // Dimmed backdrop that also swallows clicks behind the dialog.
        //
        // ITS ORDER MUST BE BELOW THE DIALOG'S, and that is the whole of a reported hang: "when i
        // close the app and the save or not windows shows, if i click accidentally outside the
        // window the app stops responding."
        //
        // Both this backdrop and the dialog used to be `Order::Foreground`. Within ONE order egui
        // raises an area to the top when you interact with it — so clicking the backdrop put a
        // full-screen, click-swallowing overlay ON TOP of the dialog. The buttons could no longer
        // be reached, `close_confirm` stayed true, and the close stayed vetoed: an app that is
        // running and repainting normally and cannot be answered or quit.
        //
        // `Middle` is strictly below `Foreground`, and areas in different orders cannot be
        // reordered past one another, so no amount of clicking can raise it now. It still covers
        // the panels, which are painted in `Background`.
        egui::Area::new(egui::Id::new("close_confirm_backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
                ui.allocate_rect(screen, egui::Sense::click_and_drag());
            });

        let name = self
            .current_file
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "this drawing".to_string());

        egui::Window::new("close_confirm")
            .order(egui::Order::Foreground)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(egui::Frame::window(&ctx.style()))
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.add_space(6.0);
                ui.heading("Unsaved changes");
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("“{name}” has changes that aren't saved yet."))
                        .weak(),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("Save and close").strong(),
                        ))
                        .clicked()
                    {
                        // Async save; the close happens in apply_saved once bytes are on disk.
                        // If there's no file yet, do_save_current opens Save As.
                        self.close_confirm = false;
                        self.close_after_save = true;
                        self.do_save_current();
                    }
                    if ui.add(egui::Button::new("Close without saving")).clicked() {
                        self.close_confirm = false;
                        self.pending_close = true; // authorise the close next frame
                    }
                    if ui.add(egui::Button::new("Cancel")).clicked() {
                        // Stay open — nothing to do but drop the prompt.
                        self.close_confirm = false;
                        self.close_after_save = false;
                    }
                });
                ui.add_space(6.0);
            });
    }

    /// The ☀ Sun / daylight window — set the building's latitude/longitude, timezone, date and
    /// time-of-day; the sun is located with Radiance's model ([`crate::solar`]) and (when enabled)
    /// lights the whole scene. Shows the computed altitude/bearing so it can be matched in a later
    /// Radiance/Blender render.
    pub(super) fn render_sun_dialog(&mut self, ctx: &egui::Context) {
        let ufu = self.factory.units.clone();
        let mut open = self.sun_modal_open;
        // The environment's own controls are edited as LOCALS and written back after the window
        // closes. Inside, `self.factory.sun` is held as `&mut`, and a closure captures all of
        // `self` with it — so touching another factory field in there would not borrow-check.
        let mut env_strength = self.factory.env_strength;
        let mut env_rot = self.factory.env_rot_deg;
        let env_name = self.factory.env_map.as_ref().map(|m| m.name.clone());
        let env_size = self.factory.env_map.as_ref().map(|m| (m.w, m.h));
        let mut want_env: Option<bool> = None; // Some(true) = pick a file, Some(false) = clear
        egui::Window::new("☀  Sun / daylight")
            .id(egui::Id::new("factory_sun_modal"))
            .collapsible(true)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                let s = &mut self.factory.sun;
                ui.checkbox(&mut s.enabled, "Light the scene by the sun")
                    .on_hover_text("Off = the original fixed studio light (no change). On = shade every surface by the real sun direction for this place, date and time.");
                ui.add_enabled_ui(s.enabled, |ui| {
                    ui.checkbox(&mut s.shadows, "Cast shadows")
                        .on_hover_text("Render a shadow-map pass from the sun so the building and furniture throw shadows. Turn off if shadows look wrong on a scene.");
                    ui.add_enabled_ui(s.shadows, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized([110.0, 18.0], egui::Label::new("Detail steps").selectable(false));
                            ui.add(
                                egui::Slider::new(
                                    &mut s.shadow_cascades,
                                    1..=crate::light3d::MAX_CASCADES as u32,
                                )
                                .show_value(true),
                            )
                            .on_hover_text(
                                "How many shadow maps to split the view across. One map over a \
                                 whole site spends nearly all of itself on ground nobody is \
                                 looking at, so a window mullion's shadow comes out as mush; \
                                 splitting gives the near ground its own tight map. Each step \
                                 costs one more pass over the scene — drop to 1 on a slow machine \
                                 or a small model.",
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.add_sized([110.0, 18.0], egui::Label::new("Sun disc").selectable(false));
                            ui.add(
                                egui::Slider::new(&mut s.sun_angle_deg, 0.0..=8.0)
                                    .suffix("°")
                                    .show_value(true),
                            )
                            .on_hover_text(
                                "How WIDE the sun is in the sky — 0.53° is the real thing. This is \
                                 what makes a shadow crisp under a table leg and soft under a roof \
                                 eave, because the penumbra widens with the distance the shadow \
                                 travels. Raise it to stand in for haze or overcast. Integrated \
                                 over the still-frame refinement, so it needs 'Refine still frame' \
                                 above 1 to show at all; 0 gives the razor edge of a point source.",
                            );
                        });
                    });
                });
                // ── Atmosphere ─────────────────────────────────────────────────────────────────
                // Distance in a render reads almost entirely from how much contrast the air takes
                // out of things. Without it a wall 200 m off is as saturated as one at 2 m, and
                // the whole scene reads as a model on a table rather than as a place.
                let f = &mut self.factory.sun.fog;
                ui.checkbox(&mut f.enabled, "Atmospheric haze")
                    .on_hover_text("Fade distant geometry into the air. Height-based, so haze pools low and thins with altitude — the way it actually behaves over a site.");
                ui.add_enabled_ui(f.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 18.0], egui::Label::new("Density").selectable(false));
                        ui.add(
                            egui::Slider::new(&mut f.density, 0.0..=0.02)
                                .logarithmic(true)
                                .custom_formatter(|v, _| {
                                    // Metres to half extinction reads far better than "per metre".
                                    if v <= 1e-6 { "off".into() } else { format!("{:.0} m", 0.693 / v) }
                                })
                                .show_value(true),
                        )
                        .on_hover_text("Shown as the distance at which HALF the light is gone. Small numbers = thick air.");
                        let mut c = [
                            f.color[0].powf(1.0 / 2.2),
                            f.color[1].powf(1.0 / 2.2),
                            f.color[2].powf(1.0 / 2.2),
                        ];
                        if ui.color_edit_button_rgb(&mut c).on_hover_text(
                            "What distance fades TOWARDS — the light the air itself scatters at you. \
                             Match it to the sky or the horizon; too dark and the haze reads as smoke.",
                        ).changed() {
                            f.color = [c[0].powf(2.2), c[1].powf(2.2), c[2].powf(2.2)];
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 18.0], egui::Label::new("Thins above").selectable(false));
                        crate::factory::length_ui(ui, &ufu, &mut f.base_z, 0.5, -1e5, 1e5, &self.calc)
                            .on_hover_text("The height the density above is measured at — usually ground level.");
                        ui.add(
                            egui::Slider::new(&mut f.falloff, 0.0..=0.2)
                                .custom_formatter(|v, _| {
                                    if v <= 1e-6 { "never".into() } else { format!("½ per {:.0} m", 0.693 / v) }
                                })
                                .show_value(true),
                        )
                        .on_hover_text("How fast the air thins with height. 'never' is uniform fog at every altitude — which is what makes most fog look like a filter rather than weather.");
                    });
                });
                ui.separator();
                ui.checkbox(&mut self.factory.clay_mode, "Clay mode (flat grey — light study)")
                    .on_hover_text("Override every material to a neutral grey (glass stays transparent) so you can read light, shadow and bounce without colour — the villa build's 'light-meter'. Toggle off for full colour.");
                ui.separator();

                // ── Colour management ──────────────────────────────────────────────────────────
                // The viewport renders scene-referred LINEAR light; this is how it becomes pixels.
                // Same three controls Blender puts in Render Properties → Color Management, and the
                // path tracers read them too, so ⏺ Render matches what the viewport shows.
                ui.label(egui::RichText::new("Colour management").small().weak());
                let c = &mut self.factory.color;
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 18.0], egui::Label::new("View transform").selectable(false));
                    egui::ComboBox::from_id_salt("view_transform").width(180.0).selected_text(c.view.label()).show_ui(ui, |ui| {
                        for v in crate::color::ViewTransform::ALL {
                            ui.selectable_value(&mut c.view, v, v.label());
                        }
                    });
                })
                .response
                .on_hover_text(
                    "How bright light is mapped to the screen. AgX (Blender's default) rolls highlights off through a \
                     desaturating film curve instead of clipping them to flat white. Standard clips at 1.0. Raw shows the \
                     raw linear buffer — diagnostic only.",
                );
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 18.0], egui::Label::new("Exposure").selectable(false));
                    ui.add(egui::DragValue::new(&mut c.exposure).update_while_editing(false).speed(0.05).range(-8.0..=8.0).suffix(" EV"))
                        .on_hover_text("Stops. +1 EV is exactly twice the light — the photographic control, not a brightness fudge.");
                    ui.add_sized([44.0, 18.0], egui::Label::new("Look").selectable(false));
                    ui.add(egui::DragValue::new(&mut c.look).update_while_editing(false).speed(0.02).range(-1.0..=1.0))
                        .on_hover_text("Extra saturation applied inside the view transform. 0 = neutral.");
                });
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 18.0], egui::Label::new("Bloom").selectable(false));
                    ui.add(egui::Slider::new(&mut c.bloom, 0.0..=0.5).show_value(true))
                        .on_hover_text(
                            "Light spilling around anything brighter than the threshold — what makes a \
                             luminaire read as a source instead of a pale rectangle. Added to \
                             scene-referred light BEFORE the view transform, so it glows rather than \
                             washing out. 0 turns it off entirely.",
                        );
                    ui.add_sized([44.0, 18.0], egui::Label::new("above").selectable(false));
                    ui.add(egui::DragValue::new(&mut c.bloom_threshold).update_while_editing(false).speed(0.05).range(0.05..=8.0))
                        .on_hover_text(
                            "Where the spill starts, in scene-referred light. 1.0 is about 'brighter \
                             than a white surface in full light', so only real sources and specular \
                             highlights bloom. Lower it to make more of the scene glow.",
                        );
                });
                if ui.small_button("Reset colour").clicked() {
                    *c = crate::color::ColorPipeline::default();
                }
                ui.horizontal(|ui| {
                    ui.add_sized([110.0, 18.0], egui::Label::new("Refine still frame").selectable(false));
                    ui.add(egui::Slider::new(&mut self.factory.taa_samples, 0..=64).show_value(true))
                        .on_hover_text(
                            "Samples averaged while nothing is moving. Each is the same frame drawn \
                             with a sub-pixel camera shift, so edges resolve past what one raster \
                             sample can reach. Costs that many frames after every change — and \
                             nothing once settled, because the finished image is simply re-shown \
                             instead of the scene being redrawn. 0 or 1 turns it off.",
                        );
                });
                ui.separator();

                // ── Environment ────────────────────────────────────────────────────────────────
                // Where the light that ISN'T the sun comes from. The sky is an analytic Preetham
                // dome (see `crate::env`): it is drawn behind the model, integrated into the
                // ambient every surface receives, and reflected by anything glossy — one model,
                // three jobs, so the backdrop and the lighting can never disagree.
                let s = &mut self.factory.sun;
                ui.label(egui::RichText::new("Environment").small().weak());
                ui.add_enabled_ui(s.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 18.0], egui::Label::new("Turbidity").selectable(false));
                        ui.add(egui::Slider::new(&mut s.turbidity, 1.8..=10.0).show_value(true))
                            .on_hover_text(
                                "How much haze is in the air. ~2 is crisp alpine light with a deep blue zenith and a hard \
                                 sun; ~6 is a hazy summer city, whiter and softer; 10 is fog. It changes the sky's colour \
                                 AND its gradient, so it changes the ambient on every surface, not just the backdrop.",
                            );
                    });
                    ui.checkbox(&mut s.sky_backdrop, "Draw the sky behind the model")
                        .on_hover_text("Show the sky itself instead of the flat studio backdrop — so what lights the scene is what you can see.");
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 18.0], egui::Label::new("Reflections").selectable(false));
                        ui.add(egui::Slider::new(&mut s.reflections, 0.0..=1.0).show_value(true))
                            .on_hover_text("How strongly glossy surfaces mirror the sky. This is what gives a metal something to be a metal about.");
                    });
                });
                // ── HDR ENVIRONMENT ───────────────────────────────────────────────────────
                // A photograph of a real place, doing the lighting. It replaces the analytic sky's
                // ambient and backdrop; the SUN above stays, because an image cannot cast a shadow
                // (it is pixels, not a direction) and a daylight study still needs one.
                ui.separator();
                ui.label(egui::RichText::new("Environment (HDRI)").small().weak());
                ui.horizontal(|ui| {
                    if ui
                        .button("🖼  Load HDRI…")
                        .on_hover_text(
                            "An .hdr or .exr environment — Poly Haven and friends. It lights every \
                             surface and gives glossy ones something real to reflect.",
                        )
                        .clicked()
                    {
                        want_env = Some(true);
                    }
                    match (&env_name, env_size) {
                        (Some(n), Some((w, h))) => {
                            ui.label(egui::RichText::new(format!("{n}  ({w}×{h})")).small());
                            if ui.small_button("✕").on_hover_text("back to the analytic sky").clicked() {
                                want_env = Some(false);
                            }
                        }
                        _ => {
                            ui.label(egui::RichText::new("none — analytic sky").small().weak());
                        }
                    }
                });
                if env_name.is_some() {
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 18.0], egui::Label::new("Strength").selectable(false));
                        ui.add(egui::Slider::new(&mut env_strength, 0.0..=4.0).show_value(true))
                            .on_hover_text("Multiplies the environment's light. 1.0 is as photographed.");
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 18.0], egui::Label::new("Rotation").selectable(false));
                        ui.add(egui::Slider::new(&mut env_rot, -180.0..=180.0).suffix("°").show_value(true))
                            .on_hover_text("Turn the world around the model — aim the sky's bright side where you want it.");
                    });
                }
                ui.checkbox(&mut s.ao.enabled, "Ambient occlusion (contact shading)")
                    .on_hover_text(
                        "Darken the ambient light where geometry blocks the sky — under furniture, in corners, along a \
                         wall-to-floor junction. It scales ONLY the ambient term, so a crease in direct sunlight is not \
                         greyed out. Without it, sky lighting reads flatter, not rounder.",
                    );
                ui.add_enabled_ui(s.ao.enabled, |ui| {
                    ui.horizontal(|ui| {
                        crate::factory::length_ui_pre(ui, &ufu, "radius ", &mut s.ao.radius, 0.02, 0.05, 3.0, &self.calc)
                            .on_hover_text("How far a crease reaches, in metres. 0.5 m suits rooms and furniture; raise it for large exteriors.");
                        ui.add(egui::DragValue::new(&mut s.ao.strength).update_while_editing(false).speed(0.02).range(0.0..=2.0).prefix("strength "))
                            .on_hover_text("1.0 is the geometric estimate. Above that is artistic licence.");
                    });
                });
                ui.checkbox(&mut s.gi.enabled, "Bounced light (colour bleed)")
                    .on_hover_text(
                        "One bounce of coloured light between surfaces — a red rug tinting the sofa \
                         above it, a white wall throwing daylight onto the ceiling. Without it the \
                         only indirect light is the sky, which is the same in every direction and \
                         knows nothing about the room, so objects read as composited in rather \
                         than standing in the space.\n\n\
                         OFF by default because it can only gather from surfaces the camera can \
                         already see: pan a bounce source off screen and its contribution fades. \
                         That is a fair trade for a viewport and a poor one for a measurement — \
                         ⏺ Render computes the real thing.",
                    );
                ui.add_enabled_ui(s.gi.enabled, |ui| {
                    ui.horizontal(|ui| {
                        crate::factory::length_ui_pre(ui, &ufu, "radius ", &mut s.gi.radius, 0.05, 0.1, 8.0, &self.calc)
                            .on_hover_text("How far a bounce reaches. 1.5 m suits rooms; raise it for large interiors.");
                        ui.add(egui::DragValue::new(&mut s.gi.strength).update_while_editing(false).speed(0.05).range(0.0..=4.0).prefix("strength "))
                            .on_hover_text(
                                "1.0 is the geometric estimate — one bounce, at the brightness one \
                                 bounce actually has. Screen-space GI only ever sees part of the \
                                 room, so pushing past 1 is a reasonable way to stand in for the \
                                 bounces it cannot reach.",
                            );
                    });
                    ui.label(
                        egui::RichText::new("Noisy until the still-frame refinement settles.")
                            .small()
                            .weak(),
                    );
                });
                ui.checkbox(&mut s.refract.enabled, "Refraction (glass bends what is behind it)")
                    .on_hover_text(
                        "Blended glass shows you the wall behind it, dimmed and tinted, but sitting \
                         exactly where it would be with no glass there at all. Real glass MOVES it. \
                         That displacement is most of what tells you a surface is glass rather than \
                         a tinted hole.\n\n\
                         Screen-space, so it can only bend what is already on screen — a ray sent \
                         toward something outside the frame lands on the nearest edge pixel. \
                         Unnoticeable on a pane set in a wall; visible on a thick lens filling the \
                         view.",
                    );
                ui.add_enabled_ui(s.refract.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.refract.ior).update_while_editing(false).speed(0.01).range(1.0..=2.5).prefix("IOR "))
                            .on_hover_text("Window glass is 1.52, water 1.33, acrylic 1.49. 1.0 is no bend at all.");
                        crate::factory::length_ui_pre(ui, &ufu, "thickness ", &mut s.refract.thickness, 0.005, 0.0, 0.5, &self.calc)
                            .on_hover_text(
                                "How far the displacement reaches. The pass has one surface to work \
                                 with rather than a front and a back, so this stands in for the \
                                 glass a ray crosses instead of measuring the pane.",
                            );
                    });
                });
                ui.checkbox(&mut s.ssr.enabled, "Reflect the scene (not just the sky)")
                    .on_hover_text(
                        "An environment map holds sky, so a reflection taken from one shows sky. A \
                         pool cannot reflect the trees standing beside it out of a sky map, however \
                         good the map — the trees are not in it. This marches the reflected ray \
                         through the scene and returns what it really hits.\n\n\
                         Screen-space: it can only reflect what is already drawn. A tree just \
                         outside the frame, or hidden behind the building from the reflection's \
                         point of view, is not there to be found, so the reflection fades back to \
                         the sky at the edges rather than ending on a hard line.\n\n\
                         Smooth surfaces only — water, glass, polished stone. A rough surface \
                         reflects a wide lobe that a single ray cannot stand in for.",
                    );
                ui.add_enabled_ui(s.ssr.enabled, |ui| {
                    ui.horizontal(|ui| {
                        crate::factory::length_ui_pre(ui, &ufu, "reach ", &mut s.ssr.distance, 1.0, 1.0, 200.0, &self.calc)
                            .on_hover_text("How far a reflected ray may travel. Longer spans a whole site and costs proportionally more steps.");
                        crate::factory::length_ui_pre(ui, &ufu, "thickness ", &mut s.ssr.thickness, 0.05, 0.05, 5.0, &self.calc)
                            .on_hover_text(
                                "How far behind a surface a ray may pass and still count as hitting \
                                 it. The depth buffer records one surface per pixel with no \
                                 thickness, so this stands in for it: too small and rays tunnel \
                                 through walls, too large and everything smears a reflection of \
                                 whatever is in front of it.",
                            );
                    });
                });
                ui.separator();

                ui.label(egui::RichText::new("Location").small().weak());
                // Pick a city → fills latitude / longitude / timezone (Radiance has no city DB, so
                // this table feeds its lat/lon/meridian model). "Custom…" = hand-dialled coordinates.
                let cur = crate::solar::city_at(s.lat_deg, s.lon_deg);
                let sel_text = cur.map(|i| crate::solar::CITIES[i].0).unwrap_or("Custom…");
                egui::ComboBox::from_id_salt("sun_city_picker")
                    .selected_text(sel_text)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for (i, c) in crate::solar::CITIES.iter().enumerate() {
                            if ui.selectable_label(cur == Some(i), c.0).clicked() {
                                s.lat_deg = c.1;
                                s.lon_deg = c.2;
                                s.utc_offset = c.3;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.north_offset_deg).update_while_editing(false).speed(1.0).range(-180.0..=180.0).prefix("building faces N+ ").suffix("°"))
                        .on_hover_text("Rotate true north off world +Y so the building can face any way. This is about ORIENTATION, not location.");
                });
                // Raw coordinates for anywhere not in the list (or fine-tuning) — hidden by default so
                // the common case is just picking a city.
                egui::CollapsingHeader::new(egui::RichText::new("Exact coordinates").small())
                    .id_salt("sun_exact_coords")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut s.lat_deg).update_while_editing(false).speed(0.1).range(-90.0..=90.0).prefix("lat ").suffix("°"))
                                .on_hover_text("Latitude, + north");
                            ui.add(egui::DragValue::new(&mut s.lon_deg).update_while_editing(false).speed(0.1).range(-180.0..=180.0).prefix("lon ").suffix("°"))
                                .on_hover_text("Longitude, + east");
                        });
                        ui.add(egui::DragValue::new(&mut s.utc_offset).update_while_editing(false).speed(0.25).range(-12.0..=14.0).prefix("UTC ").suffix(" h"))
                            .on_hover_text("Timezone offset from UTC, + east (GMT = 0, Gulf = +4, PST = −8). Add an hour for daylight saving.");
                    });

                ui.add_space(4.0);
                ui.label(egui::RichText::new("Date & time").small().weak());
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut s.month).update_while_editing(false).range(1..=12).prefix("mo "));
                    ui.add(egui::DragValue::new(&mut s.day).update_while_editing(false).range(1..=31).prefix("day "));
                });
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut s.hour, 0.0..=24.0).text("hour").show_value(true))
                        .on_hover_text("Local standard clock time (drag to move the sun across the day).");
                });
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut s.intensity, 0.2..=2.0).text("brightness").show_value(true));
                });

                // Readout — the located sun, so it can be reproduced in Radiance/Blender.
                ui.separator();
                let doy = crate::solar::day_of_year(s.month, s.day);
                let p = crate::solar::sun_position(s.lat_deg, s.lon_deg, s.utc_offset, doy, s.hour, s.north_offset_deg);
                let compass = |b: f32| {
                    const N: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
                    N[(((b / 45.0).round() as i32).rem_euclid(8)) as usize]
                };
                if p.up {
                    ui.label(egui::RichText::new(format!(
                        "sun altitude {:.1}° · bearing {:.0}° ({}) · day {}",
                        p.altitude_deg, p.bearing_deg, compass(p.bearing_deg), doy
                    )).small().color(crate::theme::color::ACCENT));
                    ui.label(egui::RichText::new(format!(
                        "direction  x {:+.2}  y {:+.2}  z {:+.2}",
                        p.dir.x, p.dir.y, p.dir.z
                    )).small().weak());
                } else {
                    ui.label(egui::RichText::new(format!(
                        "sun is below the horizon ({:.1}°) — night", p.altitude_deg
                    )).small().color(egui::Color32::from_rgb(150, 160, 200)));
                }
                ui.label(egui::RichText::new("Sun located with the Radiance gensky model (X east · Y north · Z up).").small().weak());

                ui.separator();
                ui.label(egui::RichText::new("Offline render").small().weak());
                ui.horizontal(|ui| {
                    if ui
                        .button("▶  Export + run Radiance")
                        .on_hover_text("Choose an output folder, then write the Radiance scene AND run the full pipeline (oconv → rpict → pfilt → ra_bmp), showing the finished render in-app. Same framing as the viewport / path tracer.")
                        .clicked()
                    {
                        self.pick_radiance_folder(true);
                    }
                    ui.label("Size");
                    egui::ComboBox::from_id_salt("rad_res")
                        .selected_text(["640×480", "960×720", "1280×960", "1600×1200"][self.rad_res.min(3) as usize])
                        .show_ui(ui, |ui| {
                            for (k, name) in ["640×480", "960×720", "1280×960", "1600×1200"].iter().enumerate() {
                                ui.selectable_value(&mut self.rad_res, k as u32, *name);
                            }
                        })
                        .response
                        .on_hover_text("Radiance output resolution — bigger = sharper but rpict takes longer.");
                    if ui
                        .button("⬈  Export only…")
                        .on_hover_text("Choose a folder and write the Radiance scene (.rad geometry + gensky sky + render script) to run yourself later.")
                        .clicked()
                    {
                        self.pick_radiance_folder(false);
                    }
                });
                if !self.factory.status.is_empty() {
                    ui.label(egui::RichText::new(&self.factory.status).small().color(crate::theme::color::ACCENT));
                }
            });
        // Write the environment locals back, and act on a load/clear.
        if (env_strength - self.factory.env_strength).abs() > 1e-6
            || (env_rot - self.factory.env_rot_deg).abs() > 1e-6
        {
            self.factory.env_strength = env_strength;
            self.factory.env_rot_deg = env_rot;
            self.factory.dirty = true;
        }
        match want_env {
            Some(true) => {
                let dir = std::path::Path::new(r"G:\blender dev\hdri");
                if dir.is_dir() {
                    self.file_dialog_dir = Some(dir.to_path_buf());
                }
                self.open_file_dialog(FileDialogMode::ImportHdri, ".hdr");
            }
            Some(false) => {
                let msg = self.factory.set_env_map(None);
                self.factory.status = msg;
            }
            None => {}
        }
        self.sun_modal_open = open;
    }

    // =====================================================================
    // MATERIALS FACTORY — a node-based material editor (Blender-style shader
    // graph). The graph is authored here and COMPILED onto the material's flat
    // renderer params each frame, so the 3D Factory view updates live.
    // =====================================================================

    /// The 🎨 Materials Factory window: a material list, a node canvas (Texture → Principled BSDF →
    /// Output), and an inspector for the selected node. Compiles the edited graph back onto the
    /// selected material every frame — a plain colour is emitted as a live "solid" procedural, so
    /// every edit shows in the 3D view without a GPU re-upload. See [`crate::material_graph`].
    /// Point the Materials Factory at the surface that was just clicked.
    ///
    /// A face that already carries its own material is simply selected — clicking it again is
    /// asking to edit what is there. A face with NONE is given one, copied from whatever it looks
    /// like now, and bound to that face alone.
    ///
    /// Giving it its own is the whole point. The alternative — selecting the object's shared
    /// material — means the first edit repaints every other face using it, which is the bug this
    /// window is being reorganised to fix, arriving by a different route.
    pub(super) fn materials_select_surface(&mut self, key: crate::factory::SurfaceKey) {
        if let Some(&ti) = self.factory.surface_texture.get(&key) {
            self.materials.sel = Some(ti);
            self.factory.status = format!(
                "Materials Factory: editing '{}'",
                self.factory
                    .textures
                    .get(ti)
                    .map(|t| t.name.as_str())
                    .unwrap_or("material"),
            );
            return;
        }
        self.snapshot_factory();
        // Start from what the face looks like TODAY — its own colour, else its feature's, else the
        // neutral default — so opening a material is not also a visible change.
        let base = self
            .factory
            .surface_color
            .get(&key)
            .copied()
            .or_else(|| self.factory.feature_color.get(&key.0).copied())
            .unwrap_or([0.82, 0.82, 0.84]);
        let n = self.factory.textures.len() + 1;
        let idx = self
            .factory
            .add_procedural_texture(format!("Surface {n}"), crate::factory::ProcDef::solid(base));
        self.factory.surface_texture.insert(key, idx);
        self.factory.recompute();
        self.materials.sel = Some(idx);
        self.factory.status =
            format!("Materials Factory: this face now has its own material, 'Surface {n}'");
        self.history
            .push(format!("  surface given its own material 'Surface {n}'"));
    }

    pub(super) fn render_materials_factory(&mut self, ctx: &egui::Context) {
        // Seed a graph for the selected material on first open.
        if let Some(i) = self.materials.sel {
            if i < self.factory.textures.len() {
                if !self.materials.graphs.contains_key(&i) {
                    let g = crate::material_graph::MaterialGraph::from_texture(
                        &self.factory.textures[i],
                    );
                    self.materials.graphs.insert(i, g);
                }
            } else {
                self.materials.sel = None;
            }
        }
        let mut open = self.materials_open;
        egui::Window::new("🎨  Materials Factory")
            .id(egui::Id::new("materials_factory_window"))
            .default_size([980.0, 600.0])
            .min_width(560.0)
            .resizable(true)
            .collapsible(true)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::SidePanel::left("mf_materials")
                    .resizable(true)
                    .default_width(170.0)
                    .show_inside(ui, |ui| self.mf_material_list(ui));
                egui::SidePanel::right("mf_inspector")
                    .resizable(true)
                    .default_width(226.0)
                    .show_inside(ui, |ui| self.mf_inspector(ui));
                egui::CentralPanel::default().show_inside(ui, |ui| self.mf_canvas(ui));
            });
        // CLOSING THE WINDOW PUTS THE BRUSH DOWN.
        //
        // An armed brush is a mode, and a mode with nothing on screen to show it is a trap: shut
        // the window with a material still armed and the next click in the 3D view would have
        // painted a face for reasons the user could not see. The click path already refuses to
        // paint while the window is closed; disarming as well means reopening it does not resume a
        // brush from ten minutes ago either.
        if self.materials_open && !open {
            self.factory.surface_tex_brush = None;
        }
        self.materials_open = open;

        // Compile the edited graph back onto the material (live, uniform-driven — no re-upload).
        if let Some(i) = self.materials.sel {
            if let Some(c) = self.materials.graphs.get(&i).map(|g| g.compile()) {
                self.apply_compiled_material(i, &c);
            }
        }
    }

    /// The material list (left panel): every texture/material, plus "New material".
    fn mf_material_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Materials");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("＋ New").on_hover_text("Create a new (grey) material").clicked() {
                    let n = self.factory.textures.len();
                    let idx = self.factory.add_procedural_texture(
                        format!("Material {}", n + 1),
                        crate::factory::ProcDef::solid([0.75, 0.75, 0.75]),
                    );
                    self.materials.sel = Some(idx);
                    self.materials.sel_node = None;
                }
                if ui.small_button("🖼").on_hover_text("Paste an image from the clipboard as a new material — applied to the selection, or armed for the next face you click").clicked() {
                    // Create it with NO selection required, then apply it the way every other
                    // material is applied. Create-only was the other half of "i cant apply texture
                    // from clip board": the material appeared in the library and went nowhere.
                    if let Some((idx, name, w, h)) = self.paste_texture_as_material() {
                        self.materials.sel = Some(idx);
                        self.materials.sel_node = None;
                        self.apply_texture_index_to_selection(idx, &name, w, h);
                    }
                }
                if ui.small_button("📂").on_hover_text("Load an image file (PNG/JPG) as a new material").clicked() {
                    let cc0 = crate::assets::path("assets/cc0/textures");
                    if cc0.is_dir() {
                        self.file_dialog_dir = Some(cc0.to_path_buf());
                    }
                    self.open_file_dialog(FileDialogMode::ImportTexture, "");
                }
            });
        });
        // ---- APPLY — one obvious "material → surface" flow, right here ------------------------
        // Scope selector (synced with the 3D panel's Textures mode), a live line saying exactly
        // what Apply will hit, and the Apply button (same routing as the ▼ Textures picker).
        if let Some(i) = self.materials.sel {
            if i < self.factory.textures.len() {
                use crate::factory::FurnPaintMode as M;
                ui.separator();
                ui.label(egui::RichText::new("Apply to").small().weak());
                let mut mode = self.factory.furn_paint_mode;
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut mode, M::WholeObject, "Object")
                        .on_hover_text("The whole selected furniture / solid");
                    ui.selectable_value(&mut mode, M::Face, "Face")
                        .on_hover_text("One flat face — click it on the model in the 3D view");
                    ui.selectable_value(&mut mode, M::Piece, "Piece")
                        .on_hover_text("One connected piece (part) — click it on the model");
                });
                if mode != self.factory.furn_paint_mode {
                    self.factory.furn_paint_mode = mode;
                    if mode == M::WholeObject {
                        self.factory.furn_face_sel = None;
                        self.factory.furn_tex_brush = None;
                    }
                }
                // What will Apply hit right now?
                let furn_name = self.factory.sel_furn_primary().and_then(|fi| {
                    self.factory
                        .furniture
                        .get(fi)
                        .and_then(|f| self.factory.furniture_lib.get(f.asset))
                        .map(|a| a.name.clone())
                });
                let face_ready =
                    match (&self.factory.furn_face_sel, self.factory.sel_furn_primary()) {
                        (Some((sfi, _)), Some(fi)) => *sfi == fi,
                        _ => false,
                    };
                let target = match (&furn_name, mode) {
                    (Some(n), M::WholeObject) => format!("→ {n} (whole object)"),
                    (Some(n), _) if face_ready => format!(
                        "→ the clicked {} on {n}",
                        if mode == M::Piece { "piece" } else { "face" }
                    ),
                    (Some(n), _) => format!(
                        "→ click a {} on {n} in the 3D view first (or Apply to arm the brush)",
                        if mode == M::Piece { "piece" } else { "face" }
                    ),
                    (None, _) if !self.factory.selection.is_empty() => {
                        format!("→ {} selected solid(s)", self.factory.selection.len())
                    }
                    (None, _) => "→ nothing selected — click an object in the 3D view".into(),
                };
                ui.label(
                    egui::RichText::new(target)
                        .small()
                        .color(crate::theme::color::ACCENT),
                );
                let label = if mode == M::WholeObject || face_ready {
                    "✓  Apply material"
                } else {
                    "🖌  Arm face brush"
                };
                if ui
                    .button(egui::RichText::new(label).strong())
                    .on_hover_text("Apply this material to the target above. In Face/Piece mode with no face clicked yet, this arms the brush — every face you then click in the 3D view gets painted.")
                    .clicked()
                {
                    let (name, w, h) = {
                        let t = &self.factory.textures[i];
                        (t.name.clone(), t.w, t.h)
                    };
                    self.apply_texture_index_to_selection(i, &name, w, h);
                }
            }
        }
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            // ---- LIBRARY first (collapsed) so it's always discoverable above a long scene list.
            egui::CollapsingHeader::new(egui::RichText::new("➕ Library — ready-made").strong())
                .id_salt("mf_library")
                .default_open(self.factory.textures.is_empty())
                .show(ui, |ui| {
                    let presets = crate::factory::material_presets();
                    let mut cat: &str = "";
                    let mut add: Option<crate::factory::MaterialPreset> = None;
                    for p in &presets {
                        if p.category != cat {
                            cat = p.category;
                            ui.add_space(3.0);
                            ui.label(egui::RichText::new(cat).small().color(egui::Color32::from_rgb(150, 165, 185)));
                        }
                        let c = p.def.avg_color();
                        let swatch = egui::Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
                        ui.horizontal(|ui| {
                            let (r, painter) = ui.allocate_painter(egui::vec2(12.0, 12.0), egui::Sense::hover());
                            painter.rect_filled(r.rect.shrink(1.0), egui::Rounding::same(2.0), swatch);
                            if ui.selectable_label(false, p.name).on_hover_text("Add to the scene's materials and open it in the editor").clicked() {
                                add = Some(*p);
                            }
                        });
                    }
                    if let Some(p) = add {
                        let idx = self.factory.add_preset_material(&p);
                        self.materials.sel = Some(idx);
                        self.materials.sel_node = None;
                        self.factory.status = format!("material '{}' added — tune it, set Apply-to, then ✓ Apply", p.name);
                    }
                });
            ui.separator();
            let n = self.factory.textures.len();
            if n == 0 {
                ui.label(egui::RichText::new("No materials yet.\nAdd one from the Library above, or press ＋ New.").small().weak());
            }
            // Where each material is used (feature/surface paints + furniture), so the list can say
            // "unused" — the reason a selection wouldn't highlight anything in the scene.
            let mut uses = vec![0usize; n];
            for &t in self.factory.feature_texture.values() {
                if t < n { uses[t] += 1; }
            }
            for &t in self.factory.surface_texture.values() {
                if t < n { uses[t] += 1; }
            }
            for inst in &self.factory.furniture {
                if let Some(t) = inst.texture {
                    if t < n { uses[t] += 1; }
                }
                for &t in inst.surface_texture.values() {
                    if t < n { uses[t] += 1; }
                }
            }
            for i in 0..n {
                let name = self.factory.textures[i].name.clone();
                let sel = self.materials.sel == Some(i);
                ui.horizontal(|ui| {
                    if ui.selectable_label(sel, format!("{i}·  {name}")).clicked() {
                        self.materials.sel = Some(i);
                        self.materials.sel_node = None;
                    }
                    if uses[i] == 0 {
                        ui.label(egui::RichText::new("unused").small().color(egui::Color32::from_rgb(210, 150, 90)))
                            .on_hover_text("Not applied to any surface — apply it via ▼ Textures (or pick a used material) to see the highlight in the 3D view.");
                    } else if sel {
                        ui.label(egui::RichText::new(format!("{} use{}", uses[i], if uses[i] == 1 { "" } else { "s" })).small().weak());
                    }
                });
            }
            if self.materials.sel.is_some() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("The selected material pulses cyan in the 3D view so you can see where it is.")
                        .small()
                        .weak(),
                );
            }

        });
    }

    /// The node canvas (centre): draws node boxes, sockets and wires, and handles drag-to-move,
    /// drag-a-wire-to-connect, click-a-socket-to-disconnect, and empty-drag-to-pan.
    fn mf_canvas(&mut self, ui: &mut egui::Ui) {
        use crate::material_graph::NodeKind;
        use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Stroke};

        let Some(i) = self.materials.sel else {
            ui.centered_and_justified(|ui| {
                ui.label(egui::RichText::new("Select or create a material on the left.").weak());
            });
            return;
        };
        // Split disjoint fields of `self.materials` so the canvas needs no `self` access.
        let mf = &mut self.materials;
        let Some(graph) = mf.graphs.get_mut(&i) else {
            return;
        };
        let pan = &mut mf.pan;
        let drag_from = &mut mf.drag_from;
        let drag_node = &mut mf.drag_node;
        let sel_node = &mut mf.sel_node;

        const NODE_W: f32 = 178.0;
        const HEADER_H: f32 = 22.0;
        const ROW_H: f32 = 18.0;
        const SOCK_R: f32 = 5.0;

        // Palette + view controls.
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Add node:").small().weak());
            for kind in crate::material_graph::MaterialGraph::addable() {
                if ui.small_button(kind.title()).clicked() {
                    let p = [40.0 - pan.x, 40.0 - pan.y + 30.0 * (graph.nodes.len() % 6) as f32];
                    let id = graph.add(kind, p);
                    *sel_node = Some(id);
                }
            }
            ui.separator();
            if ui.small_button("⟲ Reset view").clicked() {
                *pan = egui::Vec2::ZERO;
            }
            ui.label(egui::RichText::new("drag node = move · drag from ▶ output to ◀ input = wire · click input = unwire · drag empty = pan").small().weak());
        });

        let (resp, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let origin = resp.rect.min;
        painter.rect_filled(resp.rect, egui::Rounding::same(4.0), Color32::from_gray(26));

        // ---- Layout: owned, Copy-only data so the immutable borrow of `graph` ends here ----------
        struct Sock {
            pos: Pos2,
            name: &'static str,
            col: Color32,
        }
        struct L {
            id: crate::material_graph::NodeId,
            rect: Rect,
            title: &'static str,
            ins: Vec<Sock>,
            outs: Vec<Sock>,
            swatch: Option<Color32>,
        }
        let to_screen = |p: [f32; 2]| origin + *pan + vec2(p[0], p[1]);
        let mut layout: Vec<L> = Vec::with_capacity(graph.nodes.len());
        for node in &graph.nodes {
            let ins_d = node.kind.inputs();
            let outs_d = node.kind.outputs();
            let rows = ins_d.len().max(outs_d.len()).max(1);
            let h = HEADER_H + rows as f32 * ROW_H + 6.0;
            let rect = Rect::from_min_size(to_screen(node.pos), vec2(NODE_W, h));
            let row_y = |k: usize| rect.top() + HEADER_H + k as f32 * ROW_H + ROW_H * 0.5;
            let ins = ins_d
                .iter()
                .enumerate()
                .map(|(k, (nm, ty))| Sock {
                    pos: pos2(rect.left(), row_y(k)),
                    name: nm,
                    col: Color32::from_rgb(ty.color()[0], ty.color()[1], ty.color()[2]),
                })
                .collect();
            let outs = outs_d
                .iter()
                .enumerate()
                .map(|(k, (nm, ty))| Sock {
                    pos: pos2(rect.right(), row_y(k)),
                    name: nm,
                    col: Color32::from_rgb(ty.color()[0], ty.color()[1], ty.color()[2]),
                })
                .collect();
            let swatch = match &node.kind {
                NodeKind::Rgb(c) => Some(Color32::from_rgb(
                    (c[0] * 255.0) as u8,
                    (c[1] * 255.0) as u8,
                    (c[2] * 255.0) as u8,
                )),
                NodeKind::Procedural(d) => {
                    let c = d.avg_color();
                    Some(Color32::from_rgb(
                        (c[0] * 255.0) as u8,
                        (c[1] * 255.0) as u8,
                        (c[2] * 255.0) as u8,
                    ))
                }
                _ => None,
            };
            layout.push(L {
                id: node.id,
                rect,
                title: node.kind.title(),
                ins,
                outs,
                swatch,
            });
        }
        let find = |id: crate::material_graph::NodeId| layout.iter().find(|l| l.id == id);

        // ---- Wires (existing) ----
        let wire = |painter: &egui::Painter, a: Pos2, b: Pos2, col: Color32| {
            let dx = (b.x - a.x).abs().max(40.0) * 0.5;
            let shape = egui::epaint::CubicBezierShape::from_points_stroke(
                [a, pos2(a.x + dx, a.y), pos2(b.x - dx, b.y), b],
                false,
                Color32::TRANSPARENT,
                Stroke::new(2.0, col),
            );
            painter.add(shape);
        };
        for e in &graph.edges {
            if let (Some(sf), Some(st)) = (find(e.from), find(e.to)) {
                if let (Some(o), Some(inp)) = (
                    sf.outs.get(e.from_out as usize),
                    st.ins.get(e.to_in as usize),
                ) {
                    wire(&painter, o.pos, inp.pos, Color32::from_gray(180));
                }
            }
        }

        // ---- Nodes ----
        for l in &layout {
            let selected = *sel_node == Some(l.id);
            painter.rect_filled(l.rect, egui::Rounding::same(6.0), Color32::from_gray(44));
            let header = Rect::from_min_size(l.rect.min, vec2(l.rect.width(), HEADER_H));
            painter.rect_filled(
                header,
                egui::Rounding::same(6.0),
                Color32::from_rgb(60, 66, 78),
            );
            painter.rect_stroke(
                l.rect,
                egui::Rounding::same(6.0),
                Stroke::new(
                    if selected { 2.0 } else { 1.0 },
                    if selected {
                        crate::theme::color::ACCENT
                    } else {
                        Color32::from_gray(80)
                    },
                ),
            );
            painter.text(
                header.left_center() + vec2(8.0, 0.0),
                Align2::LEFT_CENTER,
                l.title,
                FontId::proportional(12.0),
                Color32::from_gray(230),
            );
            if let Some(sw) = l.swatch {
                let s = Rect::from_min_size(
                    l.rect.min + vec2(8.0, HEADER_H + 4.0),
                    vec2(NODE_W - 16.0, 12.0),
                );
                painter.rect_filled(s, egui::Rounding::same(2.0), sw);
            }
            for s in &l.ins {
                painter.circle_filled(s.pos, SOCK_R, s.col);
                painter.text(
                    s.pos + vec2(9.0, 0.0),
                    Align2::LEFT_CENTER,
                    s.name,
                    FontId::proportional(10.5),
                    Color32::from_gray(200),
                );
            }
            for s in &l.outs {
                painter.circle_filled(s.pos, SOCK_R, s.col);
                painter.text(
                    s.pos - vec2(9.0, 0.0),
                    Align2::RIGHT_CENTER,
                    s.name,
                    FontId::proportional(10.5),
                    Color32::from_gray(200),
                );
            }
        }

        // ---- Interaction ----
        let ptr = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let hit_out = |p: Pos2| -> Option<(crate::material_graph::NodeId, u8)> {
            for l in &layout {
                for (k, s) in l.outs.iter().enumerate() {
                    if p.distance(s.pos) <= SOCK_R + 4.0 {
                        return Some((l.id, k as u8));
                    }
                }
            }
            None
        };
        let hit_in = |p: Pos2| -> Option<(crate::material_graph::NodeId, u8)> {
            for l in &layout {
                for (k, s) in l.ins.iter().enumerate() {
                    if p.distance(s.pos) <= SOCK_R + 4.0 {
                        return Some((l.id, k as u8));
                    }
                }
            }
            None
        };
        let hit_node = |p: Pos2| -> Option<crate::material_graph::NodeId> {
            layout
                .iter()
                .rev()
                .find(|l| l.rect.contains(p))
                .map(|l| l.id)
        };

        // Draw the in-progress wire.
        if let (Some((fid, foi)), Some(p)) = (*drag_from, ptr) {
            if let Some(from) = find(fid).and_then(|l| l.outs.get(foi as usize)) {
                wire(&painter, from.pos, p, crate::theme::color::ACCENT);
            }
        }

        if resp.drag_started() {
            if let Some(p) = ptr {
                if let Some((id, oi)) = hit_out(p) {
                    *drag_from = Some((id, oi));
                } else if let Some(id) = hit_node(p) {
                    *drag_node = Some(id);
                    *sel_node = Some(id);
                }
            }
        }
        if resp.dragged() {
            if drag_from.is_some() {
                // handled by the temp-wire draw above
            } else if let Some(id) = *drag_node {
                let d = resp.drag_delta();
                if let Some(n) = graph.node_mut(id) {
                    n.pos[0] += d.x;
                    n.pos[1] += d.y;
                }
            } else {
                *pan += resp.drag_delta();
            }
        }
        if resp.drag_stopped() {
            if let (Some((fid, foi)), Some(p)) = (*drag_from, ptr) {
                if let Some((tid, tii)) = hit_in(p) {
                    graph.connect(fid, foi, tid, tii);
                }
            }
            *drag_from = None;
            *drag_node = None;
        }
        if resp.clicked() {
            if let Some(p) = ptr {
                if let Some((tid, tii)) = hit_in(p) {
                    graph.disconnect_input(tid, tii); // click an input socket to unwire it
                } else if let Some(id) = hit_node(p) {
                    *sel_node = Some(id);
                }
            }
        }
    }

    /// The inspector (right panel): edit the selected node's parameters with full widgets.
    fn mf_inspector(&mut self, ui: &mut egui::Ui) {
        use crate::material_graph::NodeKind;
        let Some(i) = self.materials.sel else {
            ui.strong("Node properties");
            ui.separator();
            ui.label(egui::RichText::new("No material selected.").weak());
            return;
        };
        self.mf_material_ball(ui, i);
        ui.strong("Node properties");
        ui.separator();
        // No node picked → default to the Principled BSDF, so the properties area always shows the
        // material's main parameters (colour, metallic, roughness, Alpha/opacity, emission…).
        let nid = match self.materials.sel_node {
            Some(n) => n,
            None => match self
                .materials
                .graphs
                .get(&i)
                .and_then(|g| g.principled_id())
            {
                Some(p) => {
                    self.materials.sel_node = Some(p);
                    p
                }
                None => {
                    ui.label(
                        egui::RichText::new("Click a node on the canvas to edit it.")
                            .small()
                            .weak(),
                    );
                    return;
                }
            },
        };
        let tex_names: Vec<String> = self
            .factory
            .textures
            .iter()
            .enumerate()
            .map(|(k, t)| format!("{k}: {}", t.name))
            .collect();
        let n_tex = self.factory.textures.len();
        let mut delete = false;
        {
            let Some(graph) = self.materials.graphs.get_mut(&i) else {
                return;
            };
            let pid = graph.principled_id();
            let is_output = matches!(graph.node(nid).map(|n| &n.kind), Some(NodeKind::Output));
            // When the Principled is shown, its Base-Color SOURCE node's properties are rendered
            // inline below (pattern/grain or tiling/rotate) — the whole material in one panel.
            let src_id = if pid == Some(nid) {
                graph
                    .source_of(nid, crate::material_graph::pin::BASE_COLOR)
                    .map(|n| n.id)
            } else {
                None
            };
            if let Some(node) = graph.node_mut(nid) {
                ui.label(
                    egui::RichText::new(node.kind.title())
                        .strong()
                        .color(crate::theme::color::ACCENT),
                );
                ui.add_space(4.0);
                match &mut node.kind {
                    NodeKind::Principled(p) => {
                        ui.horizontal(|ui| {
                            ui.label("Base Color");
                            ui.color_edit_button_rgb(&mut p.base_color);
                        });
                        ui.add(egui::Slider::new(&mut p.metallic, 0.0..=1.0).text("Metallic"));
                        ui.add(egui::Slider::new(&mut p.roughness, 0.0..=1.0).text("Roughness"));
                        ui.add(egui::Slider::new(&mut p.ior, 1.0..=3.0).text("IOR"));
                        ui.add(egui::Slider::new(&mut p.alpha, 0.0..=1.0).text("Alpha"));
                        ui.horizontal(|ui| {
                            ui.label("Emission");
                            ui.color_edit_button_rgb(&mut p.emission);
                        });
                        ui.add(
                            egui::Slider::new(&mut p.emission_strength, 0.0..=20.0)
                                .text("Emission str"),
                        );
                        // Roughness MAP (red channel) — a direct material property (the graph's
                        // Roughness scalar is the fallback where the map isn't set).
                        if let Some(t) = self.factory.textures.get_mut(i) {
                            let cur = match t.rough_map {
                                Some(k) => tex_names
                                    .get(k)
                                    .cloned()
                                    .unwrap_or_else(|| "(missing)".into()),
                                None => "(none)".into(),
                            };
                            egui::ComboBox::from_label("Rough map")
                                .selected_text(cur)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut t.rough_map, None, "(none)");
                                    for k in 0..n_tex {
                                        ui.selectable_value(
                                            &mut t.rough_map,
                                            Some(k),
                                            tex_names[k].clone(),
                                        );
                                    }
                                });
                        }
                        // CLEARCOAT + SHEEN. Written straight onto the material asset, like the
                        // roughness map above: they are surface properties rather than shading
                        // graph values, and neither has an input worth wiring.
                        if let Some(t) = self.factory.textures.get_mut(i) {
                            ui.separator();
                            ui.add(
                                egui::Slider::new(&mut t.clearcoat, 0.0..=1.0).text("Clearcoat"),
                            )
                            .on_hover_text(
                                "A thin varnish over the material, with its own smooth \
                                     reflection. It reflects about the surface's real shape rather \
                                     than its bump, so a lacquered board still shows its grain but \
                                     mirrors a window as one clean shape — which is the difference \
                                     between oiled and lacquered timber, and is not something \
                                     roughness alone can say.",
                            );
                            if t.clearcoat > 0.0 {
                                ui.add(egui::Slider::new(&mut t.clearcoat_rough, 0.01..=1.0).text("Coat rough"))
                                    .on_hover_text("How polished the varnish itself is. Low = a hard glint; high = a satin sheen.");
                            }
                            ui.add(egui::Slider::new(&mut t.sheen, 0.0..=1.0).text("Sheen"))
                                .on_hover_text(
                                    "The pale rim fabric gets where you look along it, from light \
                                     scattering through the fuzz standing off its surface. Without \
                                     it velvet, felt and heavy curtains all render as matte plastic.",
                                );
                            if t.sheen > 0.0 {
                                ui.horizontal(|ui| {
                                    ui.label("Sheen tint");
                                    ui.color_edit_button_rgb(&mut t.sheen_tint).on_hover_text(
                                        "The colour of the fuzz — usually near-white even on a dark \
                                         cloth, which is most of why black velvet reads as velvet.",
                                    );
                                });
                            }
                        }
                        ui.label(egui::RichText::new("A wired input overrides its socket value. Alpha = surface opacity.").small().weak());
                    }
                    NodeKind::Rgb(c) => {
                        ui.horizontal(|ui| {
                            ui.label("Color");
                            ui.color_edit_button_rgb(c);
                        });
                    }
                    NodeKind::Value(v) => {
                        ui.add(egui::Slider::new(v, 0.0..=1.0).text("Value"));
                    }
                    NodeKind::Procedural(def) => {
                        Self::mf_edit_procedural(ui, def);
                    }
                    NodeKind::NormalMap { tex, strength } => {
                        let cur = match tex {
                            Some(k) => tex_names
                                .get(*k)
                                .cloned()
                                .unwrap_or_else(|| "(missing)".into()),
                            None => "(none)".into(),
                        };
                        egui::ComboBox::from_label("Map")
                            .selected_text(cur)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(tex, None, "(none)");
                                for k in 0..n_tex {
                                    ui.selectable_value(tex, Some(k), tex_names[k].clone());
                                }
                            });
                        ui.add(egui::Slider::new(strength, 0.0..=2.0).text("Strength"));
                    }
                    NodeKind::ImageTex => {
                        ui.label(
                            egui::RichText::new(
                                "Uses this material's own bound bitmap as the base colour.",
                            )
                            .small()
                            .weak(),
                        );
                        if let Some(t) = self.factory.textures.get_mut(i) {
                            Self::mf_edit_placement(ui, t);
                        }
                    }
                    NodeKind::Output => {
                        ui.label(
                            egui::RichText::new("The material's final surface.")
                                .small()
                                .weak(),
                        );
                    }
                }
            }
            // Base-Color SOURCE inline (when the Principled is shown): pattern/grain for a
            // procedural, tiling/move/rotate for an image, the swatch for a plain colour — the
            // full material stack edited in ONE panel, no canvas hunting.
            if let Some(sid) = src_id {
                if let Some(src) = graph.node_mut(sid) {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("Base Color — {}", src.kind.title()))
                            .strong()
                            .color(crate::theme::color::ACCENT),
                    );
                    match &mut src.kind {
                        NodeKind::Procedural(def) => Self::mf_edit_procedural(ui, def),
                        NodeKind::Rgb(c) => {
                            ui.horizontal(|ui| {
                                ui.label("Color");
                                ui.color_edit_button_rgb(c);
                            });
                        }
                        NodeKind::ImageTex => {
                            if let Some(t) = self.factory.textures.get_mut(i) {
                                Self::mf_edit_placement(ui, t);
                            }
                        }
                        _ => {}
                    }
                }
            }
            ui.separator();
            let removable = !is_output && pid != Some(nid);
            if removable {
                if ui.button("🗑  Delete node").clicked() {
                    delete = true;
                }
            } else {
                ui.label(
                    egui::RichText::new("Core node — can't delete.")
                        .small()
                        .weak(),
                );
            }
            if delete {
                graph.remove(nid);
            }
        }
        if delete {
            self.materials.sel_node = None;
        }
    }

    /// The **material ball** at the top of the inspector: a sphere of the selected material under a
    /// fixed studio sky.
    ///
    /// A flat swatch cannot show what a material now is. Roughness, metallic, IOR, the grain's own
    /// relief and what the sky puts back into a glossy surface are only legible on a curved surface
    /// with an environment behind it — which is why every tool with materials shows a sphere.
    /// Rendered on the CPU by [`crate::matball`] using the SAME BRDF and sky as the viewport, so if
    /// the two ever disagree, something in between is wrong.
    fn mf_material_ball(&mut self, ui: &mut egui::Ui, i: usize) {
        let Some(t) = self.factory.textures.get(i) else {
            return;
        };
        // Signature of everything the render depends on. Materials are written from a dozen places
        // (the node graph compiles onto them every frame), so a dirty flag would not survive.
        let sig = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            i.hash(&mut h);
            for f in [
                t.roughness,
                t.metallic,
                t.ior,
                t.opacity,
                t.emission_strength,
            ] {
                f.to_bits().hash(&mut h);
            }
            for c in t.avg.iter().chain(t.emission.iter()) {
                c.to_bits().hash(&mut h);
            }
            if let Some(p) = &t.proc {
                (p.pattern as u8).hash(&mut h);
                for f in p
                    .col_a
                    .iter()
                    .chain(p.col_b.iter())
                    .chain(p.scale.iter())
                    .chain(p.ramp.iter())
                    .chain(p.surf_rough.iter())
                {
                    f.to_bits().hash(&mut h);
                }
                for f in [p.detail, p.rough, p.contrast, p.bump] {
                    f.to_bits().hash(&mut h);
                }
            }
            h.finish()
        };
        if self.materials.ball.as_ref().map(|(s, _)| *s) != Some(sig) {
            const N: usize = 128;
            let (sky, sh, sun) = crate::matball::preview_sky();
            let prev = crate::matball::Preview::from_texture(t);
            let rgba = crate::matball::render(&prev, &sky, &sh, sun, self.factory.color, N);
            let img = egui::ColorImage::from_rgba_unmultiplied([N, N], &rgba);
            let handle =
                ui.ctx()
                    .load_texture(format!("matball_{i}"), img, egui::TextureOptions::LINEAR);
            self.materials.ball = Some((sig, handle));
        }
        if let Some((_, h)) = &self.materials.ball {
            let side = ui.available_width().min(150.0);
            ui.vertical_centered(|ui| {
                ui.image((h.id(), egui::vec2(side, side)));
            });
        }
        if ui
            .button("📂  Load PBR texture set…")
            .on_hover_text(
                "Point at a folder of maps from ambientCG / Poly Haven / Quixel and they are sorted by filename: \
                 base colour, normal (GL or DirectX), roughness or glossiness, metallic, occlusion. A single pasted \
                 image can only ever be albedo, which is why an imported brick used to read as wallpaper.",
            )
            .clicked()
        {
            self.mf_pick_texture_set(i);
        }
        ui.separator();
    }

    /// Import a folder of PBR maps as one material — the workflow every free texture library is
    /// built around (see [`crate::texture_set`]). Base colour becomes the material; the normal,
    /// roughness, metallic and occlusion maps are added as hidden textures and bound to it.
    fn mf_pick_texture_set(&mut self, i: usize) {
        self.texset_target = Some(i);
        self.open_file_dialog(FileDialogMode::PickFolder, "");
    }

    /// Apply a chosen folder to material `i` — the second half of [`Self::mf_pick_texture_set`],
    /// run when the folder dialog closes.
    fn mf_load_texture_set(&mut self, i: usize, dir: std::path::PathBuf) {
        let maps = match crate::texture_set::load_folder(&dir) {
            Ok(m) => m,
            Err(e) => {
                self.factory.status = format!("Texture set: {e}");
                return;
            }
        };
        use crate::texture_set::MapKind;
        let set_name = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Texture set")
            .to_string();
        let mut bound: Vec<&'static str> = Vec::new();
        for m in maps {
            let idx = {
                let mut a = crate::factory::TextureAsset::new(
                    format!("{set_name} · {}", m.kind.label()),
                    m.w,
                    m.h,
                    m.rgba,
                );
                // A world-space default: these sets are photographs of real surfaces at a real
                // size, and the geometry they land on here usually has no UVs at all.
                a.triplanar = true;
                a.tiles_per_m = 1.0;
                self.factory.textures.push(a);
                self.factory.textures.len() - 1
            };
            match m.kind {
                MapKind::BaseColor => {
                    // The base colour REPLACES the selected material's pixels, so the material the
                    // user already assigned to surfaces keeps its identity and its assignments.
                    let (w, h, rgba, avg) = {
                        let s = &self.factory.textures[idx];
                        (s.w, s.h, s.rgba.clone(), s.avg)
                    };
                    if let Some(t) = self.factory.textures.get_mut(i) {
                        t.w = w;
                        t.h = h;
                        t.rgba = rgba;
                        t.avg = avg;
                        t.proc = None; // a real bitmap now drives the colour
                        t.triplanar = true;
                        t.tiles_per_m = 1.0;
                        *t.png_cache.borrow_mut() = None;
                    }
                    self.factory.textures.pop(); // the staging copy is no longer needed
                    bound.push("base colour");
                }
                MapKind::Normal => {
                    if let Some(t) = self.factory.textures.get_mut(i) {
                        t.normal_map = Some(idx);
                    }
                    bound.push("normal");
                }
                MapKind::Roughness => {
                    if let Some(t) = self.factory.textures.get_mut(i) {
                        t.rough_map = Some(idx);
                    }
                    bound.push("roughness");
                }
                MapKind::Metallic => {
                    if let Some(t) = self.factory.textures.get_mut(i) {
                        t.metal_map = Some(idx);
                        t.metallic = 1.0; // the map SCALES this, so it must not start at zero
                    }
                    bound.push("metallic");
                }
                MapKind::AmbientOcclusion => {
                    if let Some(t) = self.factory.textures.get_mut(i) {
                        t.ao_map = Some(idx);
                    }
                    bound.push("occlusion");
                }
                // Height and opacity are recognised so they are not mistaken for something else,
                // but nothing consumes them yet; drop the staged texture rather than leak it.
                _ => {
                    self.factory.textures.pop();
                }
            }
        }
        if let Some(t) = self.factory.textures.get_mut(i) {
            t.name = set_name.clone();
        }
        self.materials.graphs.remove(&i); // re-seed the node graph from the new material
        self.factory.status = if bound.is_empty() {
            format!("Texture set '{set_name}': nothing usable found")
        } else {
            format!("Texture set '{set_name}' → {}", bound.join(", "))
        };
    }

    /// Procedural-pattern editor (pattern / ramp colours / grain / contrast / detail) — shared by
    /// the Procedural node arm and the inline Base-Color source section.
    fn mf_edit_procedural(ui: &mut egui::Ui, def: &mut crate::factory::ProcDef) {
        egui::ComboBox::from_label("Pattern")
            .selected_text(def.pattern.label())
            .show_ui(ui, |ui| {
                for p in crate::factory::ProcPattern::ALL {
                    ui.selectable_value(&mut def.pattern, p, p.label());
                }
            });
        ui.horizontal(|ui| {
            ui.label("A");
            ui.color_edit_button_rgb(&mut def.col_a);
            ui.label("B");
            ui.color_edit_button_rgb(&mut def.col_b);
        });
        ui.horizontal(|ui| {
            ui.label("Grain");
            for k in 0..3 {
                ui.add(
                    egui::DragValue::new(&mut def.scale[k])
                        .update_while_editing(false)
                        .speed(0.5)
                        .range(0.1..=400.0),
                );
            }
        });
        ui.add(egui::Slider::new(&mut def.contrast, 0.2..=4.0).text("Contrast"));
        ui.add(egui::Slider::new(&mut def.detail, 1.0..=8.0).text("Detail"));
        // ── the pattern's own SURFACE, not just its colour ──
        // A real material changes its finish wherever it changes its colour: oak's dark grain is
        // duller and lower than the pale wood beside it, mortar is rougher than the tile. Reading
        // both off one field is what stops a procedural reading as a picture printed on plastic.
        ui.add_space(2.0);
        ui.label(egui::RichText::new("Surface").small().weak());
        ui.horizontal(|ui| {
            ui.label("Rough A→B");
            ui.add(egui::DragValue::new(&mut def.surf_rough[0]).update_while_editing(false).speed(0.01).range(0.02..=1.0));
            ui.add(egui::DragValue::new(&mut def.surf_rough[1]).update_while_editing(false).speed(0.01).range(0.02..=1.0));
            if ui.small_button("=").on_hover_text("Uniform finish — use the material's own Roughness instead").clicked() {
                def.surf_rough = [0.5, 0.5];
            }
        })
        .response
        .on_hover_text(
            "Roughness at the two ends of the pattern. Set them EQUAL and the material's own Roughness takes over; \
             set them apart and the finish follows the grain.",
        );
        ui.add(egui::Slider::new(&mut def.bump, 0.0..=1.5).text("Relief"))
            .on_hover_text("Treat the pattern as a height field and tilt the shading normal by its slope, so the grain catches light instead of only tinting it.");
    }

    /// Image placement editor (tiling / move / rotate) — direct material properties (not
    /// graph-compiled), shared by the ImageTex node arm and the inline source section.
    fn mf_edit_placement(ui: &mut egui::Ui, t: &mut crate::factory::TextureAsset) {
        // WORLD-SPACE mapping. Architecture here is CSG extruded from a 2D plan and carries no
        // meaningful UVs at all, so "one image per face" is the only thing UV mapping can do with
        // it — which is why an imported brick texture used to read as wallpaper. Projecting in
        // world space at a real tiles-per-metre gives the texture a physical SIZE instead.
        ui.checkbox(&mut t.triplanar, "World-space (triplanar)")
            .on_hover_text("Project the image on the three world axes and blend by the surface normal, at a fixed size in metres — instead of using the mesh's UVs. The right choice for walls, floors and anything built from a plan.");
        if t.triplanar {
            ui.add(egui::DragValue::new(&mut t.tiles_per_m).update_while_editing(false).speed(0.02).range(0.02..=64.0).prefix("tiles/m "))
                .on_hover_text("How many times the image repeats per metre. 2.0 = a 0.5 m tile, on any surface it lands on.");
        }
        ui.add(egui::DragValue::new(&mut t.scale).update_while_editing(false).speed(0.05).range(0.05..=200.0).prefix("tiling "))
            .on_hover_text("UV tiling: tiles per metre (features) / repeats across the piece (furniture). Ignored in world-space mode.");
        ui.horizontal(|ui| {
            ui.label("move");
            ui.add(
                egui::DragValue::new(&mut t.offset[0])
                    .update_while_editing(false)
                    .speed(0.01)
                    .prefix("u "),
            );
            ui.add(
                egui::DragValue::new(&mut t.offset[1])
                    .update_while_editing(false)
                    .speed(0.01)
                    .prefix("v "),
            );
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut t.rot_deg)
                    .update_while_editing(false)
                    .speed(1.0)
                    .range(-360.0..=360.0)
                    .prefix("rotate ")
                    .suffix("°"),
            );
            if ui.small_button("⟳ 90°").clicked() {
                t.rot_deg = (t.rot_deg + 90.0).rem_euclid(360.0);
            }
        });
    }

    /// Write a compiled graph result onto material `i`'s flat renderer fields. Everything here is
    /// uniform-driven (procedural params / roughness / metallic / opacity / normal map), so the 3D
    /// view updates the SAME frame with no GPU re-upload and no CSG recompute.
    fn apply_compiled_material(&mut self, i: usize, c: &crate::material_graph::CompiledMaterial) {
        let Some(t) = self.factory.textures.get_mut(i) else {
            return;
        };
        t.roughness = c.roughness;
        t.metallic = c.metallic;
        t.ior = c.ior;
        t.opacity = c.opacity.clamp(0.01, 1.0);
        t.reflect = c.reflect;
        t.normal_map = c.normal_map;
        t.emission = c.emission;
        t.emission_strength = c.emission_strength;
        if c.use_image {
            t.proc = None; // keep the material's own bound bitmap
        } else if let Some(def) = c.proc {
            t.proc = Some(def);
            let col = def.avg_color();
            t.avg = col;
            t.w = 1;
            t.h = 1;
            t.rgba = vec![
                (col[0] * 255.0) as u8,
                (col[1] * 255.0) as u8,
                (col[2] * 255.0) as u8,
                255,
            ];
            *t.png_cache.borrow_mut() = None;
        }
    }

    // =====================================================================
    // ⏺ RENDER — the in-app path tracer (raytraced GI / reflections / glass),
    // progressive like Blender's Cycles viewport. Device chosen by the user.
    // =====================================================================

    /// The ⏺ Render window: device (CPU / GPU) + resolution + samples, Start/Cancel, a live
    /// progressive preview, and Save PNG. The scene, camera, sun and materials are gathered from
    /// exactly what the 3D Factory view shows (same tris as the Radiance export, same resolved sun).
    pub(super) fn render_pathtrace_dialog(&mut self, ctx: &egui::Context) {
        use crate::pathtrace::Device;
        // Drive the GPU job: a small pass batch per UI frame keeps the app responsive while the
        // image converges (the GPU draw is async; only readback stalls, and that's throttled below).
        if let (Some(gpu), Some(gl)) = (&mut self.pt_gpu, self.pt_gl.clone()) {
            if !gpu.is_done() {
                gpu.step(&gl, 2);
                ctx.request_repaint();
            }
        }
        let mut open = self.render_modal_open;
        egui::Window::new("⏺  Render — path tracer")
            .id(egui::Id::new("factory_render_modal"))
            .default_width(560.0)
            .resizable(true)
            .collapsible(true)
            .open(&mut open)
            .show(ctx, |ui| {
                let running = self.pt_job.as_ref().map(|j| !j.is_done()).unwrap_or(false)
                    || self.pt_gpu.as_ref().map(|j| !j.is_done()).unwrap_or(false);
                ui.horizontal(|ui| {
                    ui.label("Device");
                    egui::ComboBox::from_id_salt("pt_device")
                        .selected_text(match self.pt_device {
                            Device::Cpu => "CPU (all cores)",
                            Device::Gpu => "GPU (fragment tracer)",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.pt_device, Device::Gpu, "GPU (fragment tracer)")
                                .on_hover_text("Traces on the graphics card (GL 3.3 fragment shader) — much faster. Same image as CPU.");
                            ui.selectable_value(&mut self.pt_device, Device::Cpu, "CPU (all cores)")
                                .on_hover_text("Traces on every CPU core — slower, but works on any machine and any driver.");
                        });
                    ui.label("Size");
                    egui::ComboBox::from_id_salt("pt_res")
                        .selected_text(["640×480", "960×720", "1280×960", "1600×1200"][self.pt_res.min(3) as usize])
                        .show_ui(ui, |ui| {
                            for (k, name) in ["640×480", "960×720", "1280×960", "1600×1200"].iter().enumerate() {
                                ui.selectable_value(&mut self.pt_res, k as u32, *name);
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut self.pt_passes, 8..=512).text("samples").logarithmic(true))
                        .on_hover_text("Samples per pixel. The image refines progressively — more samples = less noise. You can Save at any point.");
                    if ui.checkbox(&mut self.pt_denoise, "denoise").on_hover_text(
                        "Edge-aware à-trous filter, guided by the first hit's albedo, normal and depth — so it removes \
                         noise from the LIGHT without smearing texture or rounding off silhouettes. It backs off \
                         automatically as the render converges. CPU renders only.",
                    ).changed() {
                        self.pt_last_pass = u32::MAX; // force the preview to rebuild
                    }
                });
                ui.horizontal(|ui| {
                    if !running {
                        if ui.button(egui::RichText::new("▶  Start render").strong()).clicked() {
                            self.start_pathtrace();
                        }
                    } else if ui.button("⏹  Cancel").clicked() {
                        if let Some(j) = &self.pt_job {
                            j.cancel();
                        }
                        if let Some(g) = &mut self.pt_gpu {
                            g.cancel();
                        }
                    }
                    // Progress: whichever backend is loaded (only one at a time).
                    let info = self
                        .pt_gpu
                        .as_ref()
                        .map(|g| (g.passes_done(), g.settings.passes, g.started.elapsed().as_secs_f32(), g.scene_tris, "GPU"))
                        .or_else(|| self.pt_job.as_ref().map(|j| (j.passes_done(), j.settings.passes, j.started.elapsed().as_secs_f32(), j.scene_tris, "CPU")));
                    if let Some((done, total, secs, tris, dev)) = info {
                        ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_width(180.0).text(format!("{done}/{total}")));
                        ui.label(egui::RichText::new(format!("{dev} · {secs:.1}s · {tris} tris")).small().weak());
                        if done > 0 && ui.button("💾 Save PNG").clicked() {
                            self.save_pathtrace_png();
                        }
                    }
                });
                // Live preview: re-upload whenever new passes have landed. GPU readback is throttled
                // (every 4 passes) because glReadPixels stalls the pipeline.
                let mut fresh: Option<(usize, usize, Vec<u8>)> = None;
                if let (Some(g), Some(gl)) = (&mut self.pt_gpu, self.pt_gl.clone()) {
                    let pass = g.passes_done();
                    if pass != self.pt_last_pass && (pass.wrapping_sub(self.pt_last_pass) >= 4 || g.is_done()) {
                        fresh = g.snapshot(&gl);
                        self.pt_last_pass = pass;
                    }
                } else if let Some(j) = &self.pt_job {
                    let pass = j.passes_done();
                    if pass != self.pt_last_pass {
                        fresh = j.snapshot_rgba_opt(self.pt_denoise);
                        self.pt_last_pass = pass;
                    }
                    if !j.is_done() {
                        ctx.request_repaint_after(std::time::Duration::from_millis(150));
                    }
                }
                if let Some((w, h, rgba)) = fresh {
                    let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                    match &mut self.pt_preview {
                        Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                        None => self.pt_preview = Some(ctx.load_texture("pt_preview", img, egui::TextureOptions::LINEAR)),
                    }
                }
                if let Some(tex) = &self.pt_preview {
                    let avail = ui.available_width().max(64.0);
                    let sz = tex.size_vec2();
                    let scale = (avail / sz.x).min(1.6);
                    ui.image((tex.id(), sz * scale));
                } else {
                    ui.label(
                        egui::RichText::new("True raytracing of the current 3D Factory scene: global illumination, reflections, soft sun shadows and real glass — with the ☀ Sun and 🎨 Materials you set. The image refines progressively.")
                            .small()
                            .weak(),
                    );
                }
            });
        self.render_modal_open = open;
        // Closing the window cancels a still-running job (CPU Drop joins the worker; the GPU job's
        // GL objects are deleted on its context).
        if !open {
            if self.pt_job.as_ref().map(|j| !j.is_done()).unwrap_or(false) {
                self.pt_job = None;
            }
            if let Some(g) = self.pt_gpu.take() {
                if let Some(gl) = &self.pt_gl {
                    g.destroy(gl);
                }
            }
        }
    }

    /// Gather the scene exactly as displayed and launch the chosen backend — the SAME [`Scene`],
    /// camera, sun and materials either way (Blender's Cycles device switch, ours).
    fn start_pathtrace(&mut self) {
        let tris = self.factory.export_render_tris();
        if tris.is_empty() {
            self.factory.status = "Render: nothing in the scene".into();
            return;
        }
        let (eye, target) = self.factory.export_camera();
        // The resolved sun (world frame) rotated into the export frame like the geometry.
        let (_en, dir, sun_col, env_render) = self.factory.scene_env();
        let off = -self.factory.sun.north_offset_deg.to_radians();
        let (c, s) = (off.cos(), off.sin());
        let sun_dir = glam::Vec3::new(dir.x * c - dir.y * s, dir.x * s + dir.y * c, dir.z);
        // The scene carries the material table AND the north rotation, so procedurals are evaluated
        // in the model's own world space — the frame the viewport shades them in.
        let (tex_pool, tex_of) = self.factory.export_texture_table();
        let scene = crate::pathtrace::Scene::build_full(
            &tris,
            &self.factory.export_proc_table(),
            off,
            tex_pool,
            &tex_of,
        );
        // The offline render is lit by whatever the viewport is lit by — the SAME `EnvMap`, shared
        // by Arc rather than copied, so ⏺ Render is a preview of the scene and not of a different
        // sky that happens to resemble it.
        let sky = crate::pathtrace::Sky::from_env(sun_dir, sun_col, &env_render)
            .with_env(self.factory.env_map.clone());
        let (w, h) =
            [(640, 480), (960, 720), (1280, 960), (1600, 1200)][self.pt_res.min(3) as usize];
        let settings = crate::pathtrace::Settings {
            w,
            h,
            passes: self.pt_passes,
            max_depth: 6,
            color: self.factory.color,
        };
        let cam = crate::pathtrace::Camera {
            eye: glam::Vec3::from(eye),
            target: glam::Vec3::from(target),
            fov_deg: 45.0,
        };
        // Clear whichever backend ran last.
        self.pt_job = None;
        if let Some(g) = self.pt_gpu.take() {
            if let Some(gl) = &self.pt_gl {
                g.destroy(gl);
            }
        }
        self.pt_last_pass = 0;
        self.pt_preview = None;
        match (self.pt_device, self.pt_gl.clone()) {
            (crate::pathtrace::Device::Gpu, Some(gl)) => {
                match crate::pathtrace_gpu::GpuTracer::new(
                    &gl,
                    &scene.pack_gpu(),
                    cam,
                    sky.clone(),
                    settings,
                ) {
                    Ok(t) => self.pt_gpu = Some(t),
                    Err(e) => {
                        // Driver said no — run on the CPU instead and say so.
                        self.factory.status =
                            format!("GPU tracer unavailable ({e}) — rendering on CPU");
                        self.pt_job = Some(crate::pathtrace::RenderJob::start(
                            scene,
                            cam,
                            sky,
                            settings,
                            crate::pathtrace::Device::Cpu,
                        ));
                    }
                }
            }
            _ => {
                self.pt_job = Some(crate::pathtrace::RenderJob::start(
                    scene,
                    cam,
                    sky,
                    settings,
                    crate::pathtrace::Device::Cpu,
                ));
            }
        }
    }

    /// Save the current accumulation as a PNG next to the project (or home), like the Radiance export.
    fn save_pathtrace_png(&mut self) {
        // GPU: the throttled preview readback is cached; CPU: snapshot the shared accumulation.
        let shot = self
            .pt_gpu
            .as_ref()
            .and_then(|g| g.last_rgba.clone())
            .or_else(|| {
                self.pt_job
                    .as_ref()
                    .and_then(|j| j.snapshot_rgba_opt(self.pt_denoise))
            });
        let Some((w, h, rgba)) = shot else { return };
        let dir = self
            .current_file
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| {
                let home = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .unwrap_or_else(|_| ".".into());
                std::path::PathBuf::from(home)
            });
        let stem = self
            .current_file
            .as_ref()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| "scene".into());
        let path = dir.join(format!("{stem}_render.png"));
        match image::save_buffer(&path, &rgba, w as u32, h as u32, image::ColorType::Rgba8) {
            Ok(()) => {
                self.factory.status = format!("Render saved → {}", path.display());
                self.history
                    .push(format!("  saved path-traced render to {}", path.display()));
            }
            Err(e) => self.factory.status = format!("Render save failed: {e}"),
        }
    }

    /// The SUGGESTED Radiance output folder — where the folder-picker dialog starts:
    /// `<current-file-stem>_radiance` next to the project, else `home\3dfactory_radiance`.
    fn radiance_default_dir(&self) -> std::path::PathBuf {
        match self.current_file.as_ref() {
            Some(p) => {
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "scene".into());
                p.parent()
                    .map(|d| d.join(format!("{stem}_radiance")))
                    .unwrap_or_else(|| std::path::PathBuf::from(format!("{stem}_radiance")))
            }
            None => {
                let home = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .unwrap_or_else(|_| ".".into());
                std::path::Path::new(&home).join("3dfactory_radiance")
            }
        }
    }

    /// Open the folder-picker for the Radiance output. `run_after` = execute the pipeline once a
    /// folder is chosen (vs export-only). Starts in the suggested default folder.
    pub(super) fn pick_radiance_folder(&mut self, run_after: bool) {
        self.rad_run_after_pick = run_after;
        let dir = self.radiance_default_dir();
        let _ = std::fs::create_dir_all(&dir); // so the picker can start inside the suggestion
        self.file_dialog_dir = Some(dir);
        self.open_file_dialog(FileDialogMode::PickFolder, "");
    }

    /// Write the Radiance render bundle (scene.rad + sky.rad + render.bat/.sh + README) into the
    /// CHOSEN folder, matched to the ☀ Sun settings + camera. Returns false on failure.
    fn export_radiance_scene_to(&mut self, dir: &std::path::Path) -> bool {
        let tris = self.factory.export_render_tris();
        if tris.is_empty() {
            self.factory.status = "Radiance export: nothing in the scene to export".into();
            return false;
        }
        if let Err(e) = std::fs::create_dir_all(dir) {
            self.factory.status = format!("Radiance export failed: {e}");
            return false;
        }
        let s = &self.factory.sun;
        let (eye, target) = self.factory.export_camera();
        let scene = crate::radiance_export::scene_rad(&tris);
        let sky = crate::radiance_export::sky_rad(
            s.lat_deg,
            s.lon_deg,
            s.utc_offset,
            s.month,
            s.day,
            s.hour,
        );
        let (rw, rh) =
            [(640, 480), (960, 720), (1280, 960), (1600, 1200)][self.rad_res.min(3) as usize];
        let bat = crate::radiance_export::render_bat(eye, target, rw, rh);
        let sh = crate::radiance_export::render_sh(eye, target, rw, rh);
        let readme = crate::radiance_export::readme();
        let writes = [
            ("scene.rad", scene),
            ("sky.rad", sky),
            ("render.bat", bat),
            ("render.sh", sh),
            ("README.txt", readme),
        ];
        for (name, body) in writes {
            if let Err(e) = std::fs::write(dir.join(name), body) {
                self.factory.status = format!("Radiance export: failed writing {name}: {e}");
                return false;
            }
        }
        self.factory.status = format!(
            "Radiance scene exported ({} triangles) → {}",
            tris.len(),
            dir.display()
        );
        self.history.push(format!(
            "  exported Radiance scene ({} tris) to {}",
            tris.len(),
            dir.display()
        ));
        true
    }

    /// Find the LBNL Radiance `bin` folder: anywhere on PATH holding `oconv.exe`, else the
    /// standard install locations. `None` = Radiance is not installed on this machine.
    fn find_radiance_bin() -> Option<std::path::PathBuf> {
        if let Some(path) = std::env::var_os("PATH") {
            for d in std::env::split_paths(&path) {
                if d.join("oconv.exe").is_file() {
                    return Some(d);
                }
            }
        }
        for d in [
            "C:/Radiance/bin",
            "C:/Program Files/Radiance/bin",
            "C:/Program Files (x86)/Radiance/bin",
        ] {
            let p = std::path::PathBuf::from(d);
            if p.join("oconv.exe").is_file() {
                return Some(p);
            }
        }
        None
    }

    /// Export the Radiance bundle AND run the pipeline (`render.bat`: oconv → rpict → pfilt →
    /// ra_bmp) in a background thread, streaming into [`Self::rad_job`]. Preflights the Radiance
    /// install (PATH + standard folders) so a missing toolkit gives ONE clear message, and wires
    /// PATH/RAYPATH for the child so a non-PATH install still runs.
    fn run_radiance_in(&mut self, dir: std::path::PathBuf) {
        if !self.export_radiance_scene_to(&dir) {
            return;
        }
        // NB: this run needs the actual LBNL Radiance PROGRAMS. It is unrelated to the ☀ Sun
        // toggle — the viewport sun is our built-in port of Radiance's solar model, and the
        // exported sky always embeds the dialog's location/time either way.
        let Some(bin) = Self::find_radiance_bin() else {
            self.rad_job = Some(StdArc::new(Mutex::new(RadState {
                done: true,
                ok: false,
                log: format!(
                    "Radiance is not installed on this machine (oconv.exe not found on PATH, C:\\Radiance\\bin, or Program Files).\n\n\
                     The scene was still exported to:\n  {}\n\n\
                     To render with Radiance:\n\
                     1. Download the Windows installer from github.com/LBNL-ETA/Radiance/releases\n\
                     2. Install (the default C:\\Radiance is fine — it does not need to be on PATH)\n\
                     3. Press ▶ Run Radiance again.",
                    dir.display()
                ),
                image: None,
                dir: dir.clone(),
            })));
            self.rad_started = Some(std::time::Instant::now());
            self.rad_preview = None;
            self.rad_loaded = false;
            self.factory.status =
                "Radiance not installed — scene exported; see the Radiance window".into();
            return;
        };
        let shared = StdArc::new(Mutex::new(RadState {
            done: false,
            ok: false,
            log: String::new(),
            image: None,
            dir: dir.clone(),
        }));
        let sh = shared.clone();
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("cmd");
            cmd.args(["/C", "render.bat"]).current_dir(&dir);
            // Make the toolkit visible to the child even when Radiance isn't on the user's PATH,
            // and point RAYPATH at its lib (gensky needs it for the sky primitives).
            let old_path = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{};{}", bin.display(), old_path));
            if std::env::var_os("RAYPATH").is_none() {
                if let Some(lib) = bin.parent().map(|r| r.join("lib")) {
                    if lib.is_dir() {
                        cmd.env("RAYPATH", lib);
                    }
                }
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash
            }
            let out = cmd.output();
            let mut st = sh.lock().unwrap();
            match out {
                Ok(o) => {
                    st.log = format!(
                        "{}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    );
                    let bmp = dir.join("render.bmp");
                    match image::open(&bmp) {
                        Ok(img) => {
                            let rgba = img.to_rgba8();
                            let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                            st.image = Some((w, h, rgba.into_raw()));
                            st.ok = true;
                        }
                        Err(_) => {
                            st.ok = false;
                            if st.log.trim().is_empty() {
                                st.log = "No output — is Radiance installed and on PATH? (https://github.com/LBNL-ETA/Radiance)".into();
                            }
                        }
                    }
                }
                Err(e) => {
                    st.ok = false;
                    st.log = format!("Could not run render.bat: {e}");
                }
            }
            st.done = true;
        });
        self.rad_job = Some(shared);
        self.rad_started = Some(std::time::Instant::now());
        self.rad_preview = None;
        self.rad_loaded = false;
    }

    /// The "Radiance — offline render" window: progress while the pipeline runs, then the result
    /// image (or the log when it failed). Shown whenever a job exists; ✕ clears it.
    /// Everything that happens the moment the 3D Factory comes on screen.
    ///
    /// Asked for as: "theres a chance a user can miss it and draw with the wrong units. lets add a
    /// pop up dialogue box when the user opens 3d factory for the 1st time. this shouldn't show
    /// every time … but make sure a command window opens everytime the user opens 3d factory."
    ///
    /// Two different frequencies, on purpose. The DIALOG is a one-off — a question already answered
    /// is not worth asking again, and a modal on every visit gets dismissed unread, which is how a
    /// unit warning stops warning anybody. The COMMAND WINDOW opens every single time, with the
    /// working unit on its first line: unmissable, and costing nothing to ignore.
    pub(super) fn on_factory_opened(&mut self) {
        // Unmissable. `3D command:` already carries the unit and the placement mode as a live
        // readout (see `command_bar_body`), so this makes sure it is on screen to be read.
        self.cmd_window_open = true;
        let u = self.factory.units.clone();
        self.history.push(format!(
            "  3D Factory — working unit: {}. Everything you type is in {}.",
            u.label(),
            u.label(),
        ));
        // Never asked on this project, and the project did not arrive carrying an answer.
        if !self.factory.unit_asked {
            self.factory.ask_unit = true;
        }
    }

    /// Ask, ONCE, what unit this project is built in.
    ///
    /// The unit selector is the first control on the Factory toolbar and it is still easy to walk
    /// straight past — and drawing a building at 1000× is not a mistake you notice until the model
    /// is a 4.4 km sheet. Every option is a valid answer, so there is no Cancel: whatever is
    /// clicked IS the answer, and the question is not asked again.
    pub(super) fn render_unit_prompt_dialog(&mut self, ctx: &egui::Context) {
        if !self.factory.ask_unit {
            return;
        }
        let cur = self.factory.units.clone();
        let mut chosen: Option<f64> = None;
        egui::Window::new("What unit are you building in?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(
                    "Every length you type in the 3D Factory — wall heights, room sizes, \
                     offsets — is read in this unit.",
                );
                ui.add_space(6.0);
                for (label, m) in [
                    ("Millimetres   mm", cad_kernel::Units::MM),
                    ("Centimetres   cm", cad_kernel::Units::CM),
                    ("Metres   m", cad_kernel::Units::M),
                    ("Inches   in", cad_kernel::Units::INCH),
                    ("Feet   ft", cad_kernel::Units::FOOT),
                ] {
                    let on = (cur.metres_per_unit - m).abs() < 1e-9;
                    if ui
                        .add_sized([340.0, 26.0], egui::SelectableLabel::new(on, label))
                        .clicked()
                    {
                        chosen = Some(m);
                    }
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Geometry is always stored in metres, so this never moves anything and \
                         you can change it later from the first menu on the Factory toolbar. \
                         Asked once per project.",
                    )
                    .small()
                    .weak(),
                );
            });
        if let Some(m) = chosen {
            self.factory.units =
                cad_kernel::Units::from_metres_per_unit(m, cad_kernel::UnitSource::User);
            self.factory.unit_asked = true;
            self.factory.ask_unit = false;
            let l = self.factory.units.label();
            self.factory.status = format!("Working unit: {l}. Nothing moved.");
            self.history.push(format!("  working unit set to {l}"));
        }
    }

    /// The fittings standing in one room, gathered BY TYPE for the schedule.
    ///
    /// Everything here comes out of the photometric file the fitting is linked to — a report that
    /// states an illuminance without saying what produced it cannot be checked, ordered from, or
    /// handed to an installer. A file that declares no manufacturer shows a dash, which is that
    /// file's omission rather than the report's.
    pub(super) fn schedule_for(
        &self,
        fixtures: &[cad_light::Luminaire],
    ) -> Vec<crate::report::layout::ScheduleRow> {
        let mut out: Vec<crate::report::layout::ScheduleRow> = Vec::new();
        for l in fixtures {
            if let Some(row) = out.iter_mut().find(|r| r.profile == l.profile) {
                row.count += 1;
                continue;
            }
            let p = self.light.profiles.get(&l.profile);
            out.push(crate::report::layout::ScheduleRow {
                profile: if l.profile.trim().is_empty() {
                    "— no fitting assigned —".to_string()
                } else {
                    l.profile.clone()
                },
                count: 1,
                manufacturer: p.map(|p| p.manufacturer.clone()).unwrap_or_default(),
                catalogue: p.map(|p| p.catalogue.clone()).unwrap_or_default(),
                lamp: p.map(|p| p.lamp.clone()).unwrap_or_default(),
                watts: p.map(|p| p.watts).unwrap_or(0.0),
                lumens: p.map(|p| p.lumens).unwrap_or(0.0),
                size_m: p
                    .map(|p| (p.length, p.width, p.height))
                    .unwrap_or((0.0, 0.0, 0.0)),
            });
        }
        // Most of a type first — a schedule is read to find what the room is mostly made of.
        out.sort_by(|a, b| b.count.cmp(&a.count).then(a.profile.cmp(&b.profile)));
        out
    }

    /// Gather the state the report is built from. `None` when nothing has been calculated.
    pub(super) fn report_input(&self) -> Option<crate::report::layout::Input<'_>> {
        if self.light.rooms.is_empty() {
            return None;
        }
        let rooms = self
            .light
            .rooms
            .iter()
            .map(|r| crate::report::layout::RoomInput {
                name: r.name.clone(),
                // The mode the ANSWER was computed in, not the switch's current position.
                express: self.light.results_mode == Some(crate::light::CalcMode::Express),
                grid: &r.grid,
                plane: &r.plane,
                // Computed alongside the working grid on every calculation, and — until now —
                // shown nowhere. See `RoomInput::grid_en`.
                grid_en: (!r.grid_en.values.is_empty()).then_some(&r.grid_en),
                plane_en: (!r.grid_en.values.is_empty()).then_some(&r.plane_en),
                mask_en: &r.mask_en,
                mask: &r.mask,
                poly: &r.poly,
                fixtures: &r.fixtures,
                installation: r.installation.as_ref(),
                cylindrical_avg: r.cylindrical_avg,
                schedule: self.schedule_for(&r.fixtures),
            })
            .collect();
        Some(crate::report::layout::Input {
            rooms,
            // CUT AT THE HEIGHT THE LIGHT IS MEASURED AT, so the walls on the drawing are the ones
            // that shaped the field printed over them — and openings read as gaps, because the
            // plane passes through them.
            walls: self.factory.section_at_z(self.light.plane_height),
            apertures: self.factory.aperture_section_at_z(self.light.plane_height),
            surfaces: &self.light.surfaces,
            maintenance: self.light.maintenance,
            eye_height: self.light.eye_height,
            room_height: self.light.room_height,
            materials: self
                .light
                .materials
                .iter()
                .map(|m| (m.name.clone(), m.reflectance))
                .collect(),
            unassigned: self.light.unassigned_count(),
            // The report is drawn in the SAME palette as the screen, so what gets filed matches
            // what was looked at. The SCALE is the report's own — see `report::options::Scale`.
            ramp: self.light.ramp.rgb_fn(),
            mask: Vec::new(),
        })
    }

    /// Open the report dialog. Replaces the old behaviour, which wrote an HTML file to the Desktop
    /// and told you afterwards.
    pub(super) fn export_light_report(&mut self) {
        if self.light.grid.is_none() {
            self.light.last_msg = "Nothing to report — press Calculate first.".into();
            return;
        }
        // Fill in what can be known without asking, so the dialog opens ready rather than blank.
        if self.report_opts.title.trim().is_empty() {
            self.report_opts.title = self.report_default_name();
        }
        if self.report_opts.file_stem.trim().is_empty() {
            self.report_opts.file_stem = format!("{}-lighting", self.report_default_name());
        }
        if self.report_opts.out_dir.trim().is_empty() {
            // Beside the drawing, which is where a report belongs — with the project, not in the
            // pile on someone's Desktop.
            if let Some(d) = self.current_file.as_ref().and_then(|p| p.parent()) {
                self.report_opts.out_dir = d.to_string_lossy().into_owned();
            }
        }
        self.report_open = true;
    }

    fn report_default_name(&self) -> String {
        self.current_file
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    /// Start a calculation on a worker thread.
    ///
    /// THE WINDOW USED TO STOP REPAINTING. A lighting calculation is minutes on a real building
    /// and it ran here, on the UI thread — so Windows greyed the app out and wrote "Not
    /// Responding" in the title bar, and the only honest reading from outside was that it had
    /// crashed. The work was fine; there was no way to see it happening.
    ///
    /// `prepare` is the cheap half and stays here, because it inserts the profiles the model's own
    /// lights need. Everything after it takes no borrow on the app at all.
    pub(super) fn start_calculation(&mut self) {
        // "why is the calculations not being made" — asked against a recording that could not show
        // the button being pressed, let alone which of these two paths swallowed it.
        let n = self.light.luminaires.len();
        if self.calc_rx.is_some() {
            crate::dbg_event!(
                self,
                crate::dbg_recorder::DbgEvent::LightOp {
                    op: "calculate refused".into(),
                    detail:
                        "one is ALREADY RUNNING — a second would fight the first for every core. \
                         A progress bar should be up; if there is none, this is the one to chase."
                            .into(),
                    fittings_before: n,
                    fittings_after: n,
                    message: self.light.last_msg.clone(),
                    elapsed_us: 0,
                }
            );
            return; // one at a time — a second would fight the first for every core
        }
        let plan = Self::plan_doc_of(self.factory.session.as_ref(), &self.doc).clone();
        let Some(job) = self.light.prepare(&plan, Some(&self.factory)) else {
            // `prepare` writes the reason to the status line and this returns without doing
            // anything, which from outside is a button that does nothing at all.
            crate::dbg_event!(
                self,
                crate::dbg_recorder::DbgEvent::LightOp {
                    op: "calculate refused".into(),
                    detail: format!(
                        "`prepare` built no job — nothing to calculate. rooms={} factory_rooms={} \
                     model_tris={}",
                        self.light.rooms.len(),
                        self.factory.rooms.len(),
                        self.factory.cached.positions.len() / 3,
                    ),
                    fittings_before: n,
                    fittings_after: n,
                    message: self.light.last_msg.clone(),
                    elapsed_us: 0,
                }
            );
            return;
        };
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::LightOp {
                op: "calculate".into(),
                detail: format!(
                    "started — mode={} scene={} tris steps={} cell={:.3} m plane_h={:.3} m",
                    self.light.mode.label(),
                    job.scene_triangle_count(),
                    job.steps(),
                    self.light.cell_size,
                    self.light.plane_height,
                ),
                fittings_before: n,
                fittings_after: n,
                message: self.light.last_msg.clone(),
                elapsed_us: 0,
            }
        );
        let selected = crate::light::LightState::selected_room(Some(&self.factory));

        let progress = std::sync::Arc::new(crate::light::CalcProgress::default());
        progress
            .total
            .store(job.steps(), std::sync::atomic::Ordering::Relaxed);
        let p = progress.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("simlux-calc".into())
            .spawn(move || {
                let out = job.run(&p);
                // The receiver is gone only if the app closed; nothing to report to.
                let _ = tx.send(out);
            })
            .expect("the calculation thread must start");

        self.calc_rx = Some(rx);
        self.calc_progress = Some(progress);
        self.calc_selected = selected;
        self.calc_started = Some(std::time::Instant::now());
        self.light.last_msg = "Calculating…".into();
    }

    /// Collect a finished calculation, and show how the running one is getting on.
    /// Ask whether the result on screen still describes the scene.
    ///
    /// The borrow is the only reason this is not one line at the call site: the check needs the
    /// plan document and the factory while holding `&mut self.light`, and the plan is chosen by a
    /// method on `self`. Cloning it once every quarter second is the price; the check bails on the
    /// throttle before the clone whenever there is nothing to check.
    pub(super) fn refresh_light_staleness(&mut self) {
        if self.light.results_fingerprint.is_none() || self.calc_rx.is_some() {
            // Nothing calculated, or a calculation is in flight — mid-run the scene is allowed to
            // differ from the last answer, and saying so would flash a warning about a result that
            // is being replaced anyway.
            return;
        }
        if let Some(t) = self.light.stale_checked {
            if t.elapsed() < crate::light::LightState::STALE_CHECK_INTERVAL {
                return;
            }
        }
        let plan = self.plan_doc().clone();
        if self.light.refresh_staleness(&plan, Some(&self.factory)) {
            self.touch_view(); // the overlay's caption changed
        }
    }

    /// The current calculation, as its persisted record — `None` when there is
    /// nothing to persist (no result, stale, or the fingerprint is missing).
    fn current_stored_results(&self) -> Option<crate::light_store::StoredResults> {
        let fp = self.light.results_fingerprint?;
        if self.light.rooms.is_empty() || self.light.results_stale {
            return None;
        }
        Some(crate::light_store::StoredResults::of(
            &self.light.rooms,
            &self.light.surfaces,
            &self.light.last_timings,
            fp,
            &format!(
                "{} ({})",
                option_env!("SIMLUX_BUILD_NO").unwrap_or("?"),
                option_env!("SIMLUX_BUILD").unwrap_or("unknown"),
            ),
            // THE MODE THE ANSWER WAS COMPUTED IN, not the one the switch is on now -- those are
            // different questions and `results_mode` is the one that describes these numbers.
            self.light.results_mode == Some(crate::light::CalcMode::Express),
        ))
    }

    /// Write the current calculation beside the drawing.
    ///
    /// SIDECAR MODE ONLY — in Embedded mode the results ride inside the file on the
    /// next save (`spawn_save_thread` embeds `current_stored_results`), and after a
    /// fresh calculation `save_calc_results_embedded` writes the file right away.
    ///
    /// Quiet when there is nothing to write or nowhere to write it. An UNSAVED drawing has no
    /// path, so its result has nowhere to live — it is picked up by the next save instead, which
    /// is where the project gets a name.
    fn save_light_results(&mut self) {
        // EMBEDDED MODE: results ride inside the file on the next save. A drawing
        // whose format cannot embed (DWG) is never in that mode — but check the
        // format too, so a stale flag can never strand a result.
        if let Some(p) = &self.current_file {
            if crate::simlux_io::can_embed(self.extra_store, p) {
                return;
            }
        }
        let Some(path) = self.current_file.clone() else {
            return;
        };
        let Some(stored) = self.current_stored_results() else {
            return;
        };
        match crate::light_store::save(&path, &stored) {
            Ok(p) => self.history.push(format!(
                "  calculation saved → '{}' ({} room(s))",
                p.display(),
                stored.rooms.len(),
            )),
            // Never silent. Somebody who is told their result is kept and then loses it has been
            // misled, which is worse than not offering to keep it.
            Err(e) => self.history.push(format!("  ! calculation not saved: {e}")),
        }
    }

    /// After a calculation on an EMBEDDED-mode drawing: the result is not its own
    /// small file, so keeping it crash-safe means re-writing the drawing with the
    /// result inside it. Exactly what the sidecar mode does for a result alone,
    /// only heavier — a background save through the same worker the autosave uses,
    /// so the UI never waits on it. Runs when the app is idle; the re-try flag
    /// makes a save that lost the race (modal save / autosave in flight) fire once
    /// that save is done instead of being dropped.
    fn save_calc_results_embedded(&mut self) {
        // The mode can change (Save As) between a calculation finishing and this
        // running; a drawing that cannot embed has no embedded results to protect.
        if !self.can_embed_current() {
            self.results_due = false;
            return;
        }
        if self.busy.is_some() || self.autosave_rx.is_some() || self.results_rx.is_some() {
            self.results_due = true;
            return;
        }
        let Some(path) = self.current_file.clone() else {
            return;
        };
        if self.current_stored_results().is_none() {
            return; // nothing to persist yet (nothing was calculated)
        }
        self.results_due = false;
        self.results_rx = Some(self.spawn_save_thread(path.to_string_lossy().into_owned()));
    }

    /// Drain a background after-calculation embed save (see [`Self::save_calc_results_embedded`])
    /// and re-try one that was deferred while another save was running.
    pub(super) fn tick_results_save(&mut self) {
        use std::sync::mpsc::TryRecvError;
        if let Some(rx) = &self.results_rx {
            match rx.try_recv() {
                Ok(BusyMsg::Saved(res)) => {
                    self.results_rx = None;
                    match res {
                        Ok(p) => {
                            let where_ = self
                                .current_file
                                .clone()
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            self.history.push(format!(
                                "  calculation saved → embedded in '{where_}' ({} bytes)",
                                p.bytes
                            ));
                        }
                        Err(e) => self.history.push(format!("  ! calculation not saved: {e}")),
                    }
                }
                Ok(_) => self.results_rx = None,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => self.results_rx = None,
            }
        }
        if self.results_due
            && self.busy.is_none()
            && self.autosave_rx.is_none()
            && self.results_rx.is_none()
        {
            self.save_calc_results_embedded();
        }
    }

    /// Whether the CURRENT file is in embedded mode on a format that can embed —
    /// the `can_embed` rule applied to `extra_store` + `current_file`. One
    /// definition, so the results-save path and the deferral gates cannot drift.
    fn can_embed_current(&self) -> bool {
        match &self.current_file {
            Some(p) => crate::simlux_io::can_embed(self.extra_store, p),
            None => false,
        }
    }

    /// Read back the calculation saved beside `drawing`, if it still describes this scene.
    ///
    /// Called after the project is fully installed — the model has to be standing before the
    /// fingerprint means anything, since it is mostly a hash of the model.
    fn restore_light_results(&mut self, drawing: &std::path::Path) {
        let stored = crate::light_store::load(drawing);
        self.restore_light_results_from(stored);
    }

    /// Install a stored calculation (from the sidecar result file, or embedded in
    /// the drawing) if it still describes this scene. See [`Self::restore_light_results`]
    /// for the ordering contract.
    fn restore_light_results_from(&mut self, stored: Option<crate::light_store::StoredResults>) {
        let Some(stored) = stored else { return };
        let plan = self.plan_doc().clone();
        let Some(current) = self.light.current_fingerprint(&plan, Some(&self.factory)) else {
            return;
        };
        if self.light.restore_results(&stored, current) {
            self.history.push(format!(
                "  saved calculation restored ({} room(s), computed by build {})",
                self.light.rooms.len(),
                if stored.build.is_empty() {
                    "?"
                } else {
                    &stored.build
                },
            ));
        } else {
            // SAID OUT LOUD, because the alternative is a project that used to show a result and
            // now shows none, with nothing to explain the difference.
            self.history.push(
                "  a saved calculation was found but the project has changed since — recalculate"
                    .into(),
            );
        }
    }

    pub(super) fn poll_calculation(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.calc_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(out) => {
                let cancelled = out.cancelled;
                let took = self.calc_started.map(|t| t.elapsed()).unwrap_or_default();
                let n = self.light.luminaires.len();
                self.light.apply_outcome(out, self.calc_selected);
                self.calc_rx = None;
                self.calc_progress = None;
                self.calc_started = None;
                // THE OUTCOME, whichever it was. `apply_outcome` can take a finished job and still
                // produce nothing — it returns early on "Nothing to calculate." — so a calculation
                // that ran to completion and left the screen unchanged is a real state, and it has
                // to be distinguishable from one that was never started.
                crate::dbg_event!(
                    self,
                    crate::dbg_recorder::DbgEvent::LightOp {
                        op: if cancelled {
                            "calculation stopped".into()
                        } else {
                            "calculated".into()
                        },
                        detail: format!(
                            "rooms={} grid={} plane={} overlay={} stale={}",
                            self.light.rooms.len(),
                            self.light.grid.is_some(),
                            self.light.plane.is_some(),
                            self.light.show_overlay,
                            self.light.results_stale,
                        ),
                        fittings_before: n,
                        fittings_after: self.light.luminaires.len(),
                        message: self.light.last_msg.clone(),
                        elapsed_us: took.as_micros() as u64,
                    }
                );
                if !cancelled {
                    self.history
                        .push(format!("  calculated in {:.1} s", took.as_secs_f64()));
                    // WRITTEN OUT AS SOON AS IT EXISTS, not on the next project save. What this
                    // protects against is the app never reaching a save — a crash, a power cut, or
                    // somebody closing the window on a result they were still reading — and a
                    // minutes-long answer held only in memory is exactly what those lose.
                    //
                    // The shape depends on where the project lives: a sidecar-mode drawing gets
                    // its small result file; an embedded-mode drawing is re-saved on a background
                    // worker with the result inside it.
                    if self.extra_store == crate::simlux_io::ExtraDataStore::Embedded {
                        self.save_calc_results_embedded();
                    } else {
                        self.save_light_results();
                    }
                }
                self.touch_view();
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // The worker died without sending — a panic in the engine. Say so rather than
                // leaving a progress bar up for ever.
                self.calc_rx = None;
                self.calc_progress = None;
                self.calc_started = None;
                self.light.last_msg = "The calculation stopped unexpectedly.".into();
                self.history
                    .push("  ! calculation thread ended without a result".into());
                crate::dbg_event!(
                    self,
                    crate::dbg_recorder::DbgEvent::LightOp {
                        op: "calculation failed".into(),
                        detail:
                            "the worker thread ended WITHOUT sending a result — a panic inside the \
                             engine. Its message went to stderr, which a dump cannot see; run from \
                             a console to catch it."
                                .into(),
                        fittings_before: self.light.luminaires.len(),
                        fittings_after: self.light.luminaires.len(),
                        message: self.light.last_msg.clone(),
                        elapsed_us: 0,
                    }
                );
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }

        let Some(p) = self.calc_progress.clone() else {
            return;
        };
        // A worker makes no input events, so nothing would repaint the window and the bar would
        // sit still — which looks exactly like the freeze this replaces.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        let elapsed = self
            .calc_started
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("simlux_calculating")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Calculating");
            ui.add_space(6.0);
            let f = p.fraction();
            ui.add(egui::ProgressBar::new(f).show_percentage().animate(true));
            ui.add_space(4.0);
            ui.label(egui::RichText::new(p.label()).small().weak());
            ui.label(
                egui::RichText::new(format!("{elapsed:.0} s elapsed"))
                    .small()
                    .weak(),
            );
            ui.add_space(8.0);
            if p.cancelled() {
                ui.label(egui::RichText::new("Stopping…").small().weak());
            } else if ui.button("Stop").clicked() {
                cancel = true;
            }
        });
        if cancel {
            p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.light.last_msg = "Stopping the calculation…".into();
        }
    }

    /// The report dialog. Bails if closed.
    pub(super) fn render_report_dialog(&mut self, ctx: &egui::Context) {
        if !self.report_open {
            return;
        }
        // THE PREVIEW IS THE DOCUMENT, so it is laid out from the same state and the same options
        // the writer will use — every frame, because every option changes it.
        // The settings a practice keeps, read once — here rather than at startup because the
        // logos need a `Context` to make their preview textures from.
        if !self.report_prefs_loaded {
            self.load_report_prefs(Some(ctx));
        }
        // Taken OUT before the input borrows `self`, and put back at the end. The input holds a
        // shared borrow of the light state for as long as it lives, so the cache cannot be written
        // to while it is alive.
        let cached = self.report_doc.take();
        let cached_key = self.report_doc_key;
        let calc = self.light.results_fingerprint.unwrap_or(0);
        let Some(inp) = self.report_input() else {
            self.report_open = false;
            self.report_doc = cached;
            return;
        };
        // LAID OUT WHEN IT WOULD COME OUT DIFFERENT, not every frame.
        //
        // Reported as "the app starts lagging once the report window is open". Measured at 123 ms
        // a frame on the owner's three-room plan in a RELEASE build — eight frames a second spent
        // rebuilding a document that had not changed. Gathering the input, by contrast, is under
        // a tenth of a millisecond, so it still happens every frame and the key is taken from it.
        //
        // Only the PREVIEW reads this. `write_report` lays the document out again from the state
        // as it stands when Save is pressed, so nothing that leaves the app can be stale here.
        let key = inp.preview_key(&self.report_opts, calc);
        let doc = match cached {
            Some(d) if cached_key == Some(key) => d,
            _ => crate::report::layout::layout(&inp, &self.report_opts),
        };
        // Copied out before the borrow ends — the document is built, the numbers it needed are
        // done with, and what follows edits the state the input borrowed.
        let room_max = inp
            .rooms
            .iter()
            .map(|r| r.reported().max)
            .fold(0.0_f64, f64::max);
        drop(inp);
        self.report_doc_key = Some(key);

        let mut opts = std::mem::take(&mut self.report_opts);
        let mut open = self.report_open;
        let mut page = self.report_page;
        // The preview reads ONE table, in the same order the PDF builds it: renders then logos.
        let mut tex = std::mem::take(&mut self.report_tex);
        let logo_tex = std::mem::take(&mut self.report_logo_tex);
        let cover_tex = std::mem::take(&mut self.report_cover_tex);
        tex.extend(logo_tex.iter().cloned());
        tex.extend(cover_tex.iter().cloned());
        let can_capture = self.pt_job.is_some();
        let act = crate::report::ui::window_ui(
            ctx,
            &mut open,
            &mut opts,
            &doc,
            &mut page,
            &tex,
            can_capture,
            room_max,
            self.light.ramp.rgb_fn(),
            self.light.results_stale,
            &|s| crate::calc::parse_drag(&self.calc, s),
        );
        self.report_opts = opts;
        let n = self.report_opts.images.len().min(tex.len());
        self.report_logo_tex = logo_tex;
        self.report_cover_tex = cover_tex;
        tex.truncate(n);
        self.report_tex = tex;
        self.report_page = page;
        // Back into the cache, so the next frame does not lay it out again. The key was taken
        // BEFORE the dialog ran, so an option changed in this frame gives a different key on the
        // next one and the document is rebuilt exactly once for it.
        self.report_doc = Some(doc);
        // CLOSING IS WHEN THEY ARE KEPT. Saving on every frame would rewrite the file sixty times
        // a second while someone types a header; saving only on Save would lose the settings of
        // anyone who set them up and then thought better of the report.
        if self.report_open && !open {
            self.save_report_prefs();
        }
        self.report_open = open;

        self.apply_report_action(act);
    }

    /// Carry out what the dialog asked for, once the frame is over.
    ///
    /// SEPARATE FROM DRAWING IT, so each of these can be exercised on its own. The image lists and
    /// the slots that name pictures by index are the fiddly part — three lists, three sets of
    /// indices to keep straight — and driving a colour picker through a synthetic frame to reach
    /// them would test egui rather than the bookkeeping.
    pub(super) fn apply_report_action(&mut self, act: crate::report::ui::Action) {
        if act.browse_dir {
            self.report_wants_dir = true;
            self.open_file_dialog(FileDialogMode::PickFolder, "");
        }
        if act.add_images {
            self.report_wants_images = Some(crate::report::ImageSlot::Render);
            self.open_file_dialog(FileDialogMode::ImportImage, "");
        }
        if act.add_logos {
            self.report_wants_images = Some(crate::report::ImageSlot::Logo);
            self.open_file_dialog(FileDialogMode::ImportImage, "");
        }
        if act.add_cover {
            self.report_wants_images = Some(crate::report::ImageSlot::Cover);
            self.open_file_dialog(FileDialogMode::ImportImage, "");
        }
        if let Some(i) = act.remove_image {
            self.report_remove_image(i);
        }
        if let Some(i) = act.remove_logo {
            if i < self.report_opts.logos.len() {
                self.report_opts.logos.remove(i);
                if i < self.report_logo_tex.len() {
                    self.report_logo_tex.remove(i);
                }
                // The header and footer name a logo by INDEX, and everything after the removed
                // one has moved down.
                let repoint = |cur: Option<usize>| match cur {
                    Some(c) if c == i => None,
                    Some(c) if c > i => Some(c - 1),
                    other => other,
                };
                self.report_opts.header_image = repoint(self.report_opts.header_image);
                self.report_opts.footer_image = repoint(self.report_opts.footer_image);
            }
        }
        if let Some(i) = act.remove_cover {
            if i < self.report_opts.covers.len() {
                self.report_opts.covers.remove(i);
                if i < self.report_cover_tex.len() {
                    self.report_cover_tex.remove(i);
                }
                // The cover names a picture by INDEX, and everything after the removed one has
                // moved down. Only THIS list, because only the cover indexes it.
                self.report_opts.cover_image = match self.report_opts.cover_image {
                    Some(c) if c == i => None,
                    Some(c) if c > i => Some(c - 1),
                    other => other,
                };
            }
        }
        if act.capture_render {
            self.report_capture_render();
        }
        if act.save {
            self.write_report();
        }
    }

    /// Read the report settings a practice keeps, and re-read the logo images they name.
    ///
    /// The logos travel as PATHS, so the bytes come back off disk here. A file that has since been
    /// moved leaves the entry in place with no image — the setting is not lost because a drive was
    /// not mounted this morning, and the dialog shows it as a logo without a picture.
    fn load_report_prefs(&mut self, ctx: Option<&egui::Context>) {
        crate::report::Prefs::load().apply(&mut self.report_opts);
        let paths: Vec<String> = self
            .report_opts
            .logos
            .iter()
            .map(|l| l.path.clone())
            .collect();
        let captions: Vec<String> = self
            .report_opts
            .logos
            .iter()
            .map(|l| l.caption.clone())
            .collect();
        self.report_opts.logos.clear();
        self.report_logo_tex.clear();
        for (path, caption) in paths.into_iter().zip(captions) {
            let before = self.report_opts.logos.len();
            self.report_add_image(&path, ctx, crate::report::ImageSlot::Logo);
            if self.report_opts.logos.len() == before {
                // Unreadable — keep the entry so the setting survives, without an image.
                self.report_opts.logos.push(crate::report::ReportImage {
                    path,
                    caption,
                    jpeg: None,
                });
                self.report_logo_tex.push(None);
            } else if let Some(l) = self.report_opts.logos.last_mut() {
                l.caption = caption;
            }
        }
        // The indices were checked against the list `Prefs` restored; the list is the same length
        // now, so they still hold.
        self.report_prefs_loaded = true;
    }

    /// Keep what a practice will want next time.
    fn save_report_prefs(&mut self) {
        if let Err(e) = crate::report::Prefs::of(&self.report_opts).save() {
            self.history
                .push(format!("  ! report settings not saved — {e}"));
        }
    }

    /// Drop a report image, keeping everything that points at one pointing at the right one.
    ///
    /// The cover names an image by INDEX, and every index after the removed one moves down — so a
    /// cover left alone would quietly show a different picture, which is the kind of mistake that
    /// reaches a client. The preview textures are a parallel list and go the same way.
    pub(super) fn report_remove_image(&mut self, i: usize) {
        if i >= self.report_opts.images.len() {
            return;
        }
        self.report_opts.images.remove(i);
        if i < self.report_tex.len() {
            self.report_tex.remove(i);
        }
        // NOTHING TO REPOINT HERE — and this used to repoint three things.
        //
        // `cover_image`, `header_image` and `footer_image` each name an image by INDEX, so back
        // when every kind shared one list they all had to shift when a render was removed. They no
        // longer share it: the header and footer index `logos`, the cover indexes `covers`, and
        // this removes from `images`. Shifting them here therefore did the very damage the
        // shifting existed to prevent — deleting a render silently moved the header logo onto a
        // different picture, or unset it.
        //
        // Each list repoints its own slots when it loses an entry; see the `remove_logo` arm.
    }

    /// Load an image file for the report, as JPEG bytes plus a preview texture.
    pub(super) fn report_add_image(
        &mut self,
        path: &str,
        ctx: Option<&egui::Context>,
        slot: crate::report::ImageSlot,
    ) {
        let img = match image::open(path) {
            // RGBA, AND FLATTENED ONTO WHITE BELOW — not `to_rgb8()`.
            //
            // Reported as: "the logo image i uploaded are png images without background the but in
            // the report they came with black background." `to_rgb8` DISCARDS the alpha channel
            // rather than resolving it, so every transparent pixel keeps whatever colour was
            // sitting underneath it — and PNG writers almost always leave that as black. A logo
            // exported with a transparent background therefore arrived as a black rectangle with
            // the mark cut out of it, which is the exact opposite of what transparency was for.
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.history
                    .push(format!("  ! report: could not read {path} — {e}"));
                return;
            }
        };
        let (w, h) = (img.width(), img.height());
        // ONTO WHITE, because the page is white. A report image ends up as JPEG, which has no
        // alpha at all, so transparency has to be resolved against SOMETHING before it is encoded
        // — and the only honest choice is the paper the logo will be printed on. Composited
        // properly (`src·α + white·(1−α)`) rather than thresholded, so an anti-aliased edge stays
        // smooth instead of turning into a jagged cut-out.
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for px in img.pixels() {
            let [r, g, b, a] = px.0;
            let a = a as u32;
            let over = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
            rgb.push(over(r));
            rgb.push(over(g));
            rgb.push(over(b));
        }
        self.report_push_image(path.to_string(), rgb, w, h, ctx, slot);
    }

    /// Shared by the file loader and the render capture: RGB8 in, report image out.
    fn report_push_image(
        &mut self,
        path: String,
        rgb: Vec<u8>,
        w: u32,
        h: u32,
        ctx: Option<&egui::Context>,
        slot: crate::report::ImageSlot,
    ) {
        // A REPORT IMAGE IS RESIZED ON THE WAY IN. A 6000-pixel render embedded whole makes a
        // 40 MB PDF nobody can email, and it is being printed into a box a few inches across —
        // 1600 across is more than any of that can show.
        const MAX: u32 = 1600;
        let (rgb, w, h) = if w > MAX || h > MAX {
            let k = (MAX as f32 / w as f32).min(MAX as f32 / h as f32);
            let (nw, nh) = (
                ((w as f32 * k) as u32).max(1),
                ((h as f32 * k) as u32).max(1),
            );
            let src = image::RgbImage::from_raw(w, h, rgb).expect("rgb buffer matches its size");
            let dst = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Triangle);
            (dst.into_raw(), nw, nh)
        } else {
            (rgb, w, h)
        };

        let mut jpeg = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88);
        let mut enc = enc;
        if let Err(e) = enc.encode(&rgb, w, h, image::ExtendedColorType::Rgb8) {
            self.history
                .push(format!("  ! report: could not encode {path} — {e}"));
            return;
        }

        use crate::report::ImageSlot;
        let n = match slot {
            ImageSlot::Render => self.report_opts.images.len(),
            ImageSlot::Logo => self.report_opts.logos.len(),
            ImageSlot::Cover => self.report_opts.covers.len(),
        };
        let tex = ctx.map(|c| {
            let img = egui::ColorImage::from_rgb([w as usize, h as usize], &rgb);
            c.load_texture(
                format!("report_{}_{n}", slot.tex_key()),
                img,
                Default::default(),
            )
        });
        let entry = crate::report::ReportImage {
            caption: String::new(),
            path: path.clone(),
            jpeg: Some((jpeg, w, h)),
        };
        match slot {
            ImageSlot::Render => {
                self.report_opts.images.push(entry);
                self.report_tex.push(tex);
            }
            ImageSlot::Logo => {
                self.report_opts.logos.push(entry);
                self.report_logo_tex.push(tex);
            }
            ImageSlot::Cover => {
                self.report_opts.covers.push(entry);
                self.report_cover_tex.push(tex);
            }
        }
        self.history.push(format!(
            "  report: added {} {} ({w}×{h})",
            slot.noun(),
            file_stem_of(&path),
        ));
    }

    /// Take the current path-traced render as a report image.
    fn report_capture_render(&mut self) {
        let Some(job) = self.pt_job.as_ref() else {
            self.light.last_msg = "No render to capture — start one first.".into();
            return;
        };
        let Some((w, h, rgba)) = job.snapshot_rgba() else {
            self.light.last_msg = "The render has not produced a frame yet.".into();
            return;
        };
        let rgb: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        self.report_push_image(
            "render".into(),
            rgb,
            w as u32,
            h as u32,
            None,
            crate::report::ImageSlot::Render,
        );
        self.report_tex_dirty = true;
    }

    /// Write the report where the dialog says.
    pub(super) fn write_report(&mut self) {
        let Some(inp) = self.report_input() else {
            return;
        };
        let path = self.report_opts.out_path();
        let bytes: Vec<u8> = match self.report_opts.format {
            crate::report::Format::Pdf => {
                crate::report::layout::layout(&inp, &self.report_opts).write()
            }
            crate::report::Format::Html => {
                // ONE ROOM PER SECTION, in the same order the PDF puts them — a format that
                // silently reported the first room would be a trap, not a shorter report.
                let room_max = inp
                    .rooms
                    .iter()
                    .map(|r| r.reported().max)
                    .fold(0.0_f64, f64::max);
                let images: Vec<(Vec<u8>, String)> = self
                    .report_opts
                    .images
                    .iter()
                    .filter_map(|i| {
                        i.jpeg
                            .as_ref()
                            .map(|(b, _, _)| (b.clone(), i.caption.clone()))
                    })
                    .collect();
                let last = inp.rooms.len().saturating_sub(1);
                let per: Vec<crate::light_report::ReportInput> = inp
                    .rooms
                    .iter()
                    .enumerate()
                    .map(|(i, r)| crate::light_report::ReportInput {
                        title: if r.name.trim().is_empty() {
                            self.report_opts.title.clone()
                        } else {
                            r.name.clone()
                        },
                        grid: r.grid,
                        plane: r.plane,
                        maintenance: inp.maintenance,
                        installation: r.installation,
                        surfaces: inp.surfaces,
                        cylindrical_avg: r.cylindrical_avg,
                        eye_height: inp.eye_height,
                        room_height: inp.room_height,
                        materials: inp.materials.clone(),
                        unassigned: inp.unassigned,
                        ramp: inp.ramp,
                        scale_top: self.report_opts.scale.top_lx(room_max),
                        scale_auto: self.report_opts.scale.top.is_none(),
                        mask: r.mask.to_vec(),
                        sections: self.report_opts.sections.clone(),
                        // The renders belong to the DOCUMENT, so they go on the last room rather
                        // than once per room.
                        images: if i == last {
                            images.clone()
                        } else {
                            Vec::new()
                        },
                        schedule: r.schedule.clone(),
                        poly: r.poly.to_vec(),
                        fixtures: r.fixtures.to_vec(),
                    })
                    .collect();
                crate::light_report::render_all(&self.report_opts.title, &per).into_bytes()
            }
        };
        match std::fs::write(&path, bytes) {
            Ok(()) => {
                self.save_report_prefs();
                let msg = format!("Report written to {}", path.display());
                self.history.push(format!("  {msg}"));
                self.light.last_msg = msg;
                self.report_open = false;
            }
            Err(e) => {
                let msg = format!("Could not write {}: {e}", path.display());
                self.history.push(format!("  ! {msg}"));
                self.light.last_msg = msg;
            }
        }
    }

    /// Rename a face plane. Asked for as "the user can even rename these view[s] so they can
    /// instantly look at a sketch they made" — the auto-name says what the face is ON, and this is
    /// where it becomes what the drawing is OF.
    pub(super) fn render_rename_plane_dialog(&mut self, ctx: &egui::Context) {
        let Some((i, mut text)) = self.factory.rename_plane.clone() else {
            return;
        };
        let mut open = true;
        let mut commit = false;
        egui::Window::new("Rename plane")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("What is drawn on this face?")
                        .small()
                        .weak(),
                );
                let r = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(240.0)
                        .hint_text("Kitchen elevation"),
                );
                r.request_focus();
                if r.lost_focus() && ui.input(|k| k.key_pressed(egui::Key::Enter)) {
                    commit = true;
                }
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() {
                        commit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.factory.rename_plane = None;
                    }
                });
            });
        if !open {
            self.factory.rename_plane = None;
            return;
        }
        if commit {
            self.factory_commit_plane_rename(&text);
            return;
        }
        // Keep the in-progress text between frames.
        if let Some(s) = self.factory.rename_plane.as_mut() {
            s.1 = text;
        }
        let _ = i;
    }

    /// Apply the pending rename and close the dialog.
    ///
    /// SEPARATE FROM THE WINDOW that collects the text, so which plane gets renamed is a question
    /// a test can ask. It is also the question that was wrong: the pending rename used to name a
    /// ROW, and the view list behind the dialog stays live, so deleting an earlier plane and then
    /// pressing Rename renamed whichever plane had moved into that row.
    pub(super) fn factory_commit_plane_rename(&mut self, text: &str) {
        let Some((id, _)) = self.factory.rename_plane.take() else {
            return;
        };
        let t = text.trim().to_string();
        // An empty name would leave a blank row in the list. Refuse it by keeping the old one
        // rather than by arguing about it.
        if t.is_empty() {
            return;
        }
        if let Some(sk) = self.factory.model.sketch_by_id_mut(id) {
            self.history
                .push(format!("  plane '{}' renamed to '{t}'", sk.name));
            sk.name = t;
            self.factory.dirty = true;
        }
    }

    pub(super) fn render_radiance_dialog(&mut self, ctx: &egui::Context) {
        let Some(shared) = self.rad_job.clone() else {
            return;
        };
        let mut close = false;
        egui::Window::new("☀  Radiance — offline render")
            .id(egui::Id::new("radiance_run_modal"))
            .default_width(560.0)
            .resizable(true)
            .collapsible(true)
            .show(ctx, |ui| {
                let st = shared.lock().unwrap();
                if !st.done {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        let secs = self.rad_started.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                        ui.label(format!("Running oconv → rpict → pfilt → ra_bmp…  {secs}s"));
                    });
                    ui.label(
                        egui::RichText::new("Physically-accurate daylight simulation (LBNL Radiance). A few minutes is normal — rpict traces the whole scene.")
                            .small()
                            .weak(),
                    );
                    ctx.request_repaint_after(std::time::Duration::from_millis(500));
                } else if st.ok {
                    // Upload the finished image once.
                    if !self.rad_loaded {
                        if let Some((w, h, rgba)) = &st.image {
                            let img = egui::ColorImage::from_rgba_unmultiplied([*w, *h], rgba);
                            self.rad_preview = Some(ctx.load_texture("radiance_result", img, egui::TextureOptions::LINEAR));
                            self.rad_loaded = true;
                        }
                    }
                    if let Some(tex) = &self.rad_preview {
                        let avail = ui.available_width().max(64.0);
                        let sz = tex.size_vec2();
                        let scale = (avail / sz.x).min(1.4);
                        ui.image((tex.id(), sz * scale));
                    }
                    ui.label(egui::RichText::new(format!("render.bmp → {}", st.dir.display())).small().weak());
                } else {
                    ui.label(egui::RichText::new("Radiance run failed").color(egui::Color32::from_rgb(230, 120, 90)));
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        ui.label(egui::RichText::new(st.log.clone()).small().monospace());
                    });
                    ui.label(
                        egui::RichText::new("Install Radiance (github.com/LBNL-ETA/Radiance releases) and make sure oconv/rpict are on PATH.")
                            .small()
                            .weak(),
                    );
                }
                ui.separator();
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        if close {
            self.rad_job = None;
            self.rad_preview = None;
        }
    }

    /// The Architecture generator modal — staircase (straight / U-shape), spiral stair, or ramp.
    /// A movable window (closable by its ✕) whose Build button generates the solid in `cad_solid`
    /// and drops it into the scene as a placed furniture object.
    pub(super) fn render_arch_dialog(&mut self, ctx: &egui::Context) {
        let mut open = self.arch_modal_open;
        // The modal is shared, but its identity depends on WHERE it was opened from: a door comes
        // from ▼ Apertures, a cupboard from ▼ Furniture — so title it accordingly (a fixed `id`
        // keeps its position/size stable across the title change).
        let title = match self.arch_tab {
            ArchTab::Door => "🚪  Door",
            ArchTab::Cupboard => "🗄  Cupboard",
            ArchTab::Kitchen => "🍳  Kitchen cabinets",
            ArchTab::Cabin => "🚪  Cabinet unit",
            ArchTab::SweepLight => "💡  Curved light",
            ArchTab::Desk => "🖥  Office desk",
            ArchTab::Couch => "🛋  Sofa",
            _ => "🏛  Architecture",
        };
        egui::Window::new(title)
            .id(egui::Id::new("factory_generator_modal"))
            .open(&mut open)
            .resizable(false)
            .collapsible(true)
            .default_width(330.0)
            .show(ctx, |ui| self.arch_dialog_body(ui));
        self.arch_modal_open = open;
    }

    /// The body of the Architecture modal (split out so the `.open()` borrow and the `&mut self`
    /// body borrow don't overlap). Tab selector + per-generator fields + live feedback + Build.
    fn arch_dialog_body(&mut self, ui: &mut egui::Ui) {
        use cad_solid::architecture as arch;
        // The Factory working unit — every length row below is typed and shown in it.
        let u = self.factory.units.clone();

        // A closure (not a nested fn) so the rows can carry the calculator
        // store — every numeric field accepts expressions like `x*2`.
        let calc_ref = &self.calc;
        let num = |ui: &mut egui::Ui,
                   u: &cad_kernel::Units,
                   label: &str,
                   v: &mut f32,
                   speed: f32,
                   min: f32,
                   max: f32| {
            ui.horizontal(|ui| {
                ui.add_sized([150.0, 18.0], egui::Label::new(label).selectable(false));
                crate::factory::length_ui(
                    ui,
                    &u,
                    v,
                    speed as f64,
                    min as f64,
                    max as f64,
                    calc_ref,
                );
            });
        };
        // Same row, with the one sentence that says what the number is
        // measured FROM — ranges alone don't stop someone entering a backset
        // from the wrong floor.
        let num_tip = |ui: &mut egui::Ui,
                       u: &cad_kernel::Units,
                       label: &str,
                       v: &mut f32,
                       speed: f32,
                       min: f32,
                       max: f32,
                       tip: &str| {
            ui.horizontal(|ui| {
                ui.add_sized([150.0, 18.0], egui::Label::new(label).selectable(false))
                    .on_hover_text(tip);
                crate::factory::length_ui(
                    ui,
                    &u,
                    v,
                    speed as f64,
                    min as f64,
                    max as f64,
                    calc_ref,
                )
                .on_hover_text(tip);
            });
        };
        fn feedback(ui: &mut egui::Ui, text: String) {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(text)
                    .small()
                    .color(crate::theme::color::ACCENT),
            );
        }
        fn error(ui: &mut egui::Ui, text: String) {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("⚠ {text}"))
                    .small()
                    .color(egui::Color32::from_rgb(230, 120, 90)),
            );
        }

        // ── tab selector ── only the ARCHITECTURE generators tab between each other. Door (▼
        // Apertures) and Cupboard (▼ Furniture) open this same modal directly on their own screen,
        // so they get no tab row — they aren't "architecture".
        if !matches!(
            self.arch_tab,
            ArchTab::Door
                | ArchTab::Cupboard
                | ArchTab::Kitchen
                | ArchTab::Cabin
                | ArchTab::SweepLight
                | ArchTab::Desk
                | ArchTab::Couch
        ) {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.arch_tab, ArchTab::Staircase, "🪜 Staircase");
                ui.selectable_value(&mut self.arch_tab, ArchTab::Spiral, "🌀 Spiral");
                ui.selectable_value(&mut self.arch_tab, ArchTab::Ramp, "📐 Ramp");
                ui.selectable_value(&mut self.arch_tab, ArchTab::Dogleg, "🧱 Dog-leg");
                ui.selectable_value(&mut self.arch_tab, ArchTab::SpiralCsg, "🌀 Spiral (CSG)");
                ui.selectable_value(&mut self.arch_tab, ArchTab::HelicalRamp, "🌀 Helical ramp");
            });
            ui.separator();
        }

        // Deferred build request (kept out of the borrow of the per-tab fields).
        let mut build: Option<(Result<cad_solid::SolidMesh, arch::ArchError>, String)> = None;
        let mut do_dogleg = false; // the dog-leg tab builds CSG solids, not furniture
        let mut do_spiral_csg = false; // the spiral-CSG tab builds CSG solids, not furniture
        let mut do_door = false; // the door tab builds a furniture mesh
        let mut do_cupboard = false; // the cupboard tab builds a multi-material furniture mesh
        let mut do_kitchen = false; // the kitchen tab builds a multi-material furniture mesh
        let mut do_cabin = false; // the cabinet-unit tab builds a multi-material furniture mesh
        let mut do_sweep = false; // the curved-light tab builds a multi-material furniture mesh
        let mut do_desk = false; // the office-desk tab builds a multi-material furniture mesh
        let mut do_couch = false; // the sofa tab builds a multi-material furniture mesh

        match self.arch_tab {
            ArchTab::Staircase => {
                let s = &mut self.arch_stair;
                egui::ComboBox::from_label("Layout")
                    .selected_text(s.layout.label())
                    .show_ui(ui, |ui| {
                        for l in arch::StairLayout::ALL {
                            ui.selectable_value(&mut s.layout, l, l.label());
                        }
                    });
                num(
                    ui,
                    &u,
                    "Floor-to-floor height",
                    &mut s.total_height,
                    0.05,
                    0.1,
                    100.0,
                );
                num(ui, &u, "Stair width", &mut s.step_width, 0.05, 0.3, 20.0);
                num(
                    ui,
                    &u,
                    "Tread going (run)",
                    &mut s.step_depth,
                    0.01,
                    0.1,
                    2.0,
                );
                num(
                    ui,
                    &u,
                    "Target riser height",
                    &mut s.desired_riser_height,
                    0.005,
                    0.05,
                    0.5,
                );
                num(
                    ui,
                    &u,
                    "Tread thickness",
                    &mut s.thickness_tread,
                    0.005,
                    0.01,
                    0.3,
                );
                num(
                    ui,
                    &u,
                    "Riser thickness",
                    &mut s.thickness_riser,
                    0.005,
                    0.01,
                    0.3,
                );
                ui.checkbox(&mut s.has_handrails, "Handrails (balustrade)");
                if s.has_handrails {
                    num(
                        ui,
                        &u,
                        "Handrail height",
                        &mut s.handrail_height,
                        0.02,
                        0.4,
                        1.5,
                    );
                }
                ui.checkbox(&mut s.has_stringers, "Side stringers (solid slab)");

                let is_u = s.layout == arch::StairLayout::UShape;
                if is_u {
                    ui.separator();
                    num(
                        ui,
                        &u,
                        "Landing depth",
                        &mut s.landing_depth,
                        0.05,
                        0.1,
                        20.0,
                    );
                    if s.landing_depth < s.step_width {
                        error(
                            ui,
                            format!(
                                "landing depth must be ≥ stair width ({:.2} m) to turn",
                                s.step_width
                            ),
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [150.0, 18.0],
                            egui::Label::new("First-flight split").selectable(false),
                        );
                        ui.add(egui::Slider::new(&mut s.split_ratio, 0.1..=0.9).show_value(false));
                        ui.label(format!("{:.0}%", s.split_ratio * 100.0));
                    });
                }

                let params = *s;
                ui.separator();
                match arch::plan_stairs(&params) {
                    Ok(pl) => {
                        feedback(
                            ui,
                            format!(
                                "{} steps · exact riser {:.3} m · run {:.2} m",
                                pl.num_steps, pl.riser_height, pl.total_run,
                            ),
                        );
                        if is_u {
                            feedback(
                                ui,
                                format!(
                                    "landing at {:.2} m · flights {} + {} steps",
                                    pl.landing_height, pl.flight1_steps, pl.flight2_steps,
                                ),
                            );
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build staircase").strong(),
                            ))
                            .clicked()
                        {
                            let name = if is_u { "U-Stair" } else { "Staircase" };
                            build = Some((arch::build_stairs(&params), name.to_string()));
                        }
                    }
                    Err(e) => error(ui, e.to_string()),
                }
            }
            ArchTab::Spiral => {
                let s = &mut self.arch_spiral;
                num(
                    ui,
                    &u,
                    "Total height",
                    &mut s.total_height,
                    0.05,
                    0.1,
                    100.0,
                );
                num(
                    ui,
                    &u,
                    "Tread length (radial)",
                    &mut s.step_width,
                    0.05,
                    0.3,
                    10.0,
                );
                num(
                    ui,
                    &u,
                    "Inner radius",
                    &mut s.center_radius,
                    0.02,
                    0.0,
                    10.0,
                );
                num(
                    ui,
                    &u,
                    "Tread thickness",
                    &mut s.thickness_tread,
                    0.005,
                    0.01,
                    0.3,
                );
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Steps per turn").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.steps_per_turn)
                            .update_while_editing(false)
                            .speed(0.2)
                            .range(3..=64),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Total turns").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.total_turns)
                            .update_while_editing(false)
                            .speed(0.05)
                            .range(0.1..=20.0),
                    );
                });
                ui.checkbox(&mut s.has_handrail, "Outer handrail");
                if s.has_handrail {
                    num(
                        ui,
                        &u,
                        "Handrail height",
                        &mut s.handrail_height,
                        0.02,
                        0.4,
                        1.5,
                    );
                }
                let params = *s;
                ui.separator();
                match arch::plan_spiral(&params) {
                    Ok(pl) => {
                        feedback(
                            ui,
                            format!(
                                "{} steps · riser {:.3} m · {:.0}° total rotation",
                                pl.num_steps, pl.riser_height, pl.total_rotation_deg,
                            ),
                        );
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build spiral").strong(),
                            ))
                            .clicked()
                        {
                            build = Some((arch::build_spiral(&params), "Spiral Stair".to_string()));
                        }
                    }
                    Err(e) => error(ui, e.to_string()),
                }
            }
            ArchTab::Ramp => {
                let s = &mut self.arch_ramp;
                num(
                    ui,
                    &u,
                    "Vertical height",
                    &mut s.vertical_height,
                    0.05,
                    0.05,
                    50.0,
                );
                num(
                    ui,
                    &u,
                    "Horizontal length",
                    &mut s.horizontal_length,
                    0.05,
                    0.1,
                    100.0,
                );
                num(ui, &u, "Width", &mut s.width, 0.05, 0.2, 20.0);
                num(ui, &u, "Deck thickness", &mut s.thickness, 0.005, 0.02, 1.0);
                let params = *s;
                ui.separator();
                feedback(ui, format!("slope {:.1}°", arch::ramp_slope_deg(&params)));
                if ui
                    .add(egui::Button::new(
                        egui::RichText::new("✚  Build ramp").strong(),
                    ))
                    .clicked()
                {
                    build = Some((arch::build_ramp(&params), "Ramp".to_string()));
                }
            }
            ArchTab::Dogleg => {
                // A HALF-TURN stair built as EDITABLE, boolean-able CSG solids (not furniture).
                let d = &mut self.arch_dogleg;
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Lower-flight steps").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut d.n_lower)
                            .update_while_editing(false)
                            .speed(0.2)
                            .range(1..=100),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Upper-flight steps").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut d.n_upper)
                            .update_while_editing(false)
                            .speed(0.2)
                            .range(1..=100),
                    );
                });
                num(ui, &u, "Going (tread depth)", &mut d.going, 0.01, 0.1, 1.0);
                num(ui, &u, "Stair width", &mut d.width, 0.05, 0.3, 4.0);
                num(ui, &u, "Well (gap)", &mut d.well, 0.01, 0.0, 2.0);
                num(
                    ui,
                    &u,
                    "Floor-to-floor rise",
                    &mut d.total_rise,
                    0.05,
                    0.2,
                    12.0,
                );
                ui.checkbox(&mut self.arch_dogleg_treads, "Stone tread slabs");
                let inp = self.arch_dogleg;
                ui.separator();
                match cad_solid::dogleg::plan(&inp) {
                    Ok((m, warns)) => {
                        feedback(ui, format!(
                            "{} steps ({}+{}) · riser {:.3} m · pitch {:.1}° · footprint {:.2}×{:.2} m",
                            m.n_steps, inp.n_lower, inp.n_upper, m.riser, m.pitch_deg, m.shaft_len, m.shaft_wid,
                        ));
                        feedback(
                            ui,
                            format!(
                                "landing at {:.2} m · top tread {:.2} m · clear width {:.2} m",
                                m.landing_z, m.top_tread_z, m.clear_width,
                            ),
                        );
                        for w in warns.iter().take(2) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build editable stair").strong(),
                            ))
                            .clicked()
                        {
                            do_dogleg = true;
                        }
                    }
                    Err(e) => error(ui, e.to_string()),
                }
            }
            ArchTab::SpiralCsg => {
                // A HELICAL stair built as EDITABLE, boolean-able CSG solids (not furniture).
                let s = &mut self.arch_spiral_csg;
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Number of steps").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.n_steps)
                            .update_while_editing(false)
                            .speed(0.2)
                            .range(3..=200),
                    );
                });
                num(ui, &u, "Number of turns", &mut s.turns, 0.02, 0.25, 6.0);
                num(
                    ui,
                    &u,
                    "Overall height",
                    &mut s.total_height,
                    0.05,
                    0.2,
                    20.0,
                );
                num(
                    ui,
                    &u,
                    "Outer radius (width)",
                    &mut s.radius,
                    0.02,
                    0.3,
                    4.0,
                );
                ui.checkbox(&mut s.clockwise, "Clockwise going up");
                ui.checkbox(&mut s.brackets, "Support brackets");
                ui.checkbox(&mut s.handrail, "Balustrade (rails + handrail)");
                if s.handrail {
                    num(
                        ui,
                        &u,
                        "Handrail height",
                        &mut s.handrail_height,
                        0.02,
                        0.3,
                        1.3,
                    );
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [150.0, 18.0],
                            egui::Label::new("Infill rails").selectable(false),
                        );
                        ui.add(
                            egui::DragValue::new(&mut s.n_infill)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(0..=10),
                        );
                    });
                }
                let inp = self.arch_spiral_csg;
                ui.separator();
                match cad_solid::spiral::plan(&inp) {
                    Ok((m, warns)) => {
                        feedback(
                            ui,
                            format!(
                                "{} steps · {:.2} turns · riser {:.3} m · Ø {:.2} m",
                                m.n_steps, inp.turns, m.riser, m.overall_dia,
                            ),
                        );
                        feedback(
                            ui,
                            format!(
                            "walk-line going {:.3} m · top tread {:.2} m · handrail top {:.2} m",
                            m.going_walk, m.total_rise, m.handrail_top_z,
                        ),
                        );
                        for w in warns.iter().take(2) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build editable spiral").strong(),
                            ))
                            .clicked()
                        {
                            do_spiral_csg = true;
                        }
                    }
                    Err(e) => error(ui, e.to_string()),
                }
            }
            ArchTab::Door => {
                // A parametric panelled DOOR (leaf + lining + casing + hardware) placed as furniture.
                // All sizes in metres. Same "Door classic" proportions as the drawn-aperture door.
                let d = &mut self.arch_door;
                ui.label(egui::RichText::new("Leaf").small().weak());
                num(ui, &u, "Leaf width", &mut d.door_width, 0.005, 0.4, 1.6);
                num(ui, &u, "Leaf height", &mut d.door_height, 0.01, 1.2, 3.2);
                num(
                    ui,
                    &u,
                    "Leaf thickness",
                    &mut d.door_thickness,
                    0.002,
                    0.02,
                    0.1,
                );
                ui.label(egui::RichText::new("Frame / lining").small().weak());
                num(
                    ui,
                    &u,
                    "Wall thickness (depth)",
                    &mut d.frame_depth,
                    0.005,
                    0.05,
                    0.6,
                );
                num(
                    ui,
                    &u,
                    "Lining reveal (face)",
                    &mut d.frame_face_width,
                    0.002,
                    0.005,
                    0.1,
                );
                num(
                    ui,
                    &u,
                    "Stop / rebate depth",
                    &mut d.frame_stop_depth,
                    0.002,
                    0.005,
                    0.05,
                );
                ui.label(egui::RichText::new("Architrave (casing)").small().weak());
                // Legs and head are separate boards. A deeper head over equal legs is a deliberate
                // joinery choice, so it gets its own number rather than following the legs.
                num(ui, &u, "Side width", &mut d.arch_width, 0.005, 0.02, 0.30);
                num(
                    ui,
                    &u,
                    "Head height",
                    &mut d.arch_head_width,
                    0.005,
                    0.02,
                    0.40,
                );
                num(
                    ui,
                    &u,
                    "Projection",
                    &mut d.arch_thickness,
                    0.001,
                    0.004,
                    0.06,
                );
                if ui
                    .small_button("= match head to sides")
                    .on_hover_text("the measured 'Door classic' has both at 81 mm")
                    .clicked()
                {
                    d.arch_head_width = d.arch_width;
                }
                ui.label(egui::RichText::new("Hardware").small().weak());
                // The two numbers that decide where the lever actually lands, at the ranges asked
                // for. Backset is measured from the leaf's LEADING edge (the one away from the
                // hinges) — that is where a lock's backset is measured from on site.
                num_tip(ui, &u, "Handle backset", &mut d.handle_backset, 0.002, 0.050, 0.150,
                    "50–150 mm. Spindle centre in from the leaf's LEADING edge — the one away from\n\
                     the hinges, and the edge a lock's backset is measured from on site.",
                );
                num_tip(
                    ui,
                    &u,
                    "Handle height",
                    &mut d.handle_height,
                    0.005,
                    0.750,
                    1.300,
                    "750–1300 mm above the FINISHED FLOOR, which is how handle heights are\n\
                     specified and set out. The leaf's own bottom sits ~2 mm above that.",
                );
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Hinge side").selectable(false),
                    );
                    ui.selectable_value(&mut d.hinge_side, 1.0, "Right");
                    ui.selectable_value(&mut d.hinge_side, -1.0, "Left");
                });
                let inp = self.arch_door;
                // ── MATERIALS, per component ────────────────────────────────────────────────
                // A door is not one material: the glazed door is timber everywhere except one
                // panel, which is the case this exists for. Ironmongery is absent on purpose —
                // it follows the handle FINISH below, and offering it twice in two vocabularies
                // would only let the two disagree.
                ui.label(egui::RichText::new("Materials").small().weak());
                for slot in crate::door_mat::Slot::ALL {
                    let mut cur = self.door_mats.get(slot);
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [150.0, 18.0],
                            egui::Label::new(slot.label()).selectable(false),
                        );
                        egui::ComboBox::from_id_salt(("door_mat", slot.label()))
                            .selected_text(cur.label())
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for m in crate::door_mat::DoorMaterial::ALL {
                                    ui.selectable_value(&mut cur, m, m.label());
                                }
                            });
                    });
                    self.door_mats.set(slot, cur);
                }
                if self.door_mats.has_glass() {
                    ui.label(
                        egui::RichText::new(
                            "  glazed — the panel keeps its raised field, so it reads as thick glass",
                        )
                        .small()
                        .weak(),
                    );
                }
                ui.separator();
                // HANDLE — a row in Casing / hardware that opens the picker dialog. The dialog
                // itself is drawn at the top level (see `handle_dialog_window`) so it floats over
                // the panel rather than reflowing it.
                self.handle_lib_ensure();
                let cur = self
                    .handle_lib
                    .as_ref()
                    .and_then(|l| l.get(&self.handle_sel))
                    .map(|h| h.name.clone())
                    .unwrap_or_else(|| "none".into());
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 18.0], egui::Label::new("Handle").selectable(false));
                    if ui
                        .button(format!("🔧  {cur}"))
                        .on_hover_text("choose a handle")
                        .clicked()
                    {
                        self.handle_dialog = true;
                    }
                });
                ui.separator();
                match cad_solid::door::plan(&inp) {
                    Ok((m, warns)) => {
                        feedback(
                            ui,
                            format!(
                            "structural opening {:.0} × {:.0} mm — the hole to leave in the wall",
                            m.structural_opening_w * 1000.0, m.structural_opening_h * 1000.0,
                        ),
                        );
                        feedback(
                            ui,
                            format!(
                                "panel {:.0} × {:.0} mm · casing {:.0} × {:.0} mm · depth {:.0} mm",
                                m.panel_w * 1000.0,
                                m.panel_h * 1000.0,
                                m.overall_w * 1000.0,
                                m.overall_h * 1000.0,
                                m.overall_depth * 1000.0,
                            ),
                        );
                        for w in warns.iter().skip(1).take(2) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new("✚  Build door").strong(),
                                ))
                                .clicked()
                            {
                                do_door = true;
                            }
                            // Look at it before it is in the model — see `door_preview_window`.
                            if ui
                                .button("👁  Preview")
                                .on_hover_text("see the door, with its handle, before inserting it")
                                .clicked()
                            {
                                self.door_preview.open = true;
                            }
                        });
                    }
                    Err(e) => error(ui, e.to_string()),
                }
            }
            ArchTab::HelicalRamp => {
                // A sloped annular deck winding around a free inner edge — built as a furniture mesh.
                use cad_solid::architecture::BalustradeEdges;
                let r = &mut self.arch_helical;
                num(
                    ui,
                    &u,
                    "Ramp height (rise)",
                    &mut r.ramp_height,
                    0.05,
                    0.2,
                    40.0,
                );
                num(ui, &u, "Inner radius", &mut r.r_inner, 0.02, 0.0, 20.0);
                num(ui, &u, "Outer radius", &mut r.r_outer, 0.02, 0.3, 30.0);
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 18.0], egui::Label::new("Turns").selectable(false));
                    ui.add(
                        egui::DragValue::new(&mut r.turns)
                            .update_while_editing(false)
                            .speed(0.05)
                            .range(0.1..=20.0),
                    );
                });
                num(
                    ui,
                    &u,
                    "Slab thickness",
                    &mut r.slab_thickness,
                    0.005,
                    0.02,
                    1.0,
                );
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Direction").selectable(false),
                    );
                    ui.selectable_value(&mut r.direction, 1.0, "Anticlockwise");
                    ui.selectable_value(&mut r.direction, -1.0, "Clockwise");
                });
                num(ui, &u, "Rail height", &mut r.rail_height, 0.02, 0.3, 1.5);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Rails per edge").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut r.rail_count)
                            .update_while_editing(false)
                            .speed(0.1)
                            .range(1..=8),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Balustrade").selectable(false),
                    );
                    ui.selectable_value(&mut r.balustrade_edges, BalustradeEdges::Both, "Both");
                    ui.selectable_value(
                        &mut r.balustrade_edges,
                        BalustradeEdges::OuterOnly,
                        "Outer",
                    );
                    ui.selectable_value(
                        &mut r.balustrade_edges,
                        BalustradeEdges::InnerOnly,
                        "Inner",
                    );
                });
                ui.checkbox(&mut r.end_rails, "End rails (close each end)");
                let inp = self.arch_helical;
                ui.separator();
                match cad_solid::architecture::plan_helical_ramp(&inp) {
                    Ok((m, warns)) => {
                        let one_in = |s: f32| if s > 0.0 { 1.0 / s } else { f32::INFINITY };
                        feedback(ui, format!(
                            "deck {:.2} m wide · slope 1:{:.1} mean, 1:{:.1} inner · top {:.2} m",
                            m.deck_width, one_in(m.slope_mean), one_in(m.slope_inner), m.top_of_deck,
                        ));
                        feedback(
                            ui,
                            format!(
                                "headroom {:.2} m · rise/turn {:.2} m · posts {}/{} (outer/inner)",
                                m.headroom, m.rise_per_turn, m.posts_outer, m.posts_inner,
                            ),
                        );
                        for w in warns.iter().take(3) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build helical ramp").strong(),
                            ))
                            .clicked()
                        {
                            build = Some((
                                cad_solid::architecture::build_helical_ramp(&inp),
                                "Helical Ramp".to_string(),
                            ));
                        }
                    }
                    Err(e) => error(ui, e.to_string()),
                }
            }
            ArchTab::Cupboard => {
                // A parametric CABINET: set the overall size, lay out a grid of bays × tiers, and
                // fill each cell (door / glass / drawers / niche / panel). Counts are DERIVED from
                // the grid, never typed (spec §B1.4). Built as a multi-material furniture mesh.
                use cad_solid::cupboard::Cell;
                let cup = &mut self.arch_cupboard;
                num(ui, &u, "Carcass width", &mut cup.width, 0.01, 0.3, 6.0);
                num(
                    ui,
                    &u,
                    "Carcass height",
                    &mut cup.carcass_height,
                    0.01,
                    0.3,
                    3.0,
                );
                num(ui, &u, "Depth", &mut cup.depth, 0.005, 0.1, 1.0);

                // Grid size — resize cols/rows/layout to match (new cells default to a door).
                let (mut n_cols, mut n_rows) = (cup.cols.len(), cup.rows.len());
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Bays × tiers").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut n_cols)
                            .update_while_editing(false)
                            .speed(0.05)
                            .range(1..=6),
                    );
                    ui.label("×");
                    ui.add(
                        egui::DragValue::new(&mut n_rows)
                            .update_while_editing(false)
                            .speed(0.05)
                            .range(1..=6),
                    );
                });
                if n_cols != cup.cols.len() || n_rows != cup.rows.len() {
                    cup.cols.resize(n_cols, 1.0);
                    cup.rows.resize(n_rows, 1.0);
                    for row in cup.layout.iter_mut() {
                        row.resize(n_cols, Cell::Door);
                    }
                    cup.layout.resize(n_rows, vec![Cell::Door; n_cols]);
                }

                // The matrix: one dropdown per cell, top row first — this is the whole tool (§A1).
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Cells (top row first)").small().weak());
                for r in 0..cup.rows.len() {
                    ui.horizontal(|ui| {
                        for c in 0..cup.cols.len() {
                            let cell = &mut cup.layout[r][c];
                            let text = match *cell {
                                Cell::Drawers(n) => format!("Drawers×{n}"),
                                other => other.label().to_string(),
                            };
                            egui::ComboBox::from_id_salt(("cup_cell", r, c))
                                .width(74.0)
                                .selected_text(text)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(cell, Cell::Door, "Door");
                                    ui.selectable_value(cell, Cell::Glass, "Glass");
                                    ui.selectable_value(cell, Cell::Niche, "Niche");
                                    ui.selectable_value(cell, Cell::Panel, "Panel");
                                    if ui
                                        .selectable_label(
                                            matches!(cell, Cell::Drawers(_)),
                                            "Drawers",
                                        )
                                        .clicked()
                                        && !matches!(cell, Cell::Drawers(_))
                                    {
                                        *cell = Cell::Drawers(2);
                                    }
                                });
                            if let Cell::Drawers(n) = cell {
                                ui.add(
                                    egui::DragValue::new(n)
                                        .update_while_editing(false)
                                        .speed(0.1)
                                        .range(1..=8),
                                );
                            }
                        }
                    });
                }
                ui.checkbox(&mut cup.handles, "Handles");

                let inp = cup.clone();
                ui.separator();
                match cad_solid::cupboard::plan(&inp) {
                    Ok((m, warns)) => {
                        feedback(
                            ui,
                            format!(
                                "{} bays × {} tiers · {:.0} × {:.0} mm overall · aspect {:.3}",
                                m.n_cols,
                                m.n_rows,
                                m.total_w * 1000.0,
                                m.total_h * 1000.0,
                                m.aspect,
                            ),
                        );
                        feedback(
                            ui,
                            format!(
                                "{} doors · {} glazed · {} drawers · {} niches · {} handles",
                                m.doors, m.glazed, m.drawers, m.niches, m.handles,
                            ),
                        );
                        for w in warns.iter().take(3) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build cupboard").strong(),
                            ))
                            .clicked()
                        {
                            do_cupboard = true;
                        }
                    }
                    Err(e) => error(ui, e.to_string()),
                }
            }
            ArchTab::Kitchen => {
                // A parametric KITCHEN RUN: base (lower) cabinets always; wall (upper) cabinets
                // optional via the toggle. Worktop, plinth and legs are generated automatically.
                use cad_solid::kitchen::{
                    BaseKind, BaseModule, KitchenShape, WallKind, WallModule,
                };
                let kt = &mut self.arch_kitchen;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Shape").small().weak());
                    egui::ComboBox::from_id_salt("kt_shape")
                        .width(120.0)
                        .selected_text(kt.shape.label())
                        .show_ui(ui, |ui| {
                            for s in [KitchenShape::Straight, KitchenShape::L, KitchenShape::U] {
                                ui.selectable_value(&mut kt.shape, s, s.label());
                            }
                        });
                });
                num(ui, &u, "Main run length", &mut kt.length, 0.02, 0.3, 12.0);
                if matches!(kt.shape, KitchenShape::L | KitchenShape::U) {
                    num(
                        ui,
                        &u,
                        "Return leg B length",
                        &mut kt.length_b,
                        0.02,
                        0.3,
                        12.0,
                    );
                }
                if matches!(kt.shape, KitchenShape::U) {
                    num(
                        ui,
                        &u,
                        "Return leg C length",
                        &mut kt.length_c,
                        0.02,
                        0.3,
                        12.0,
                    );
                }
                if matches!(kt.shape, KitchenShape::L | KitchenShape::U) {
                    ui.label(
                        egui::RichText::new(
                            "return legs auto-fill to length; edit the main run below",
                        )
                        .small()
                        .weak(),
                    );
                }
                ui.checkbox(&mut kt.include_wall, "Upper (wall) cabinets")
                    .on_hover_text("Off = base run only (worktop, plinth, tall units stay). This is the with/without-upper option.");
                ui.checkbox(&mut kt.handles, "Handles");

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Base cabinets (lower) — width 0 = auto")
                        .small()
                        .weak(),
                );
                let mut rm: Option<usize> = None;
                for i in 0..kt.base.len() {
                    ui.horizontal(|ui| {
                        let m = &mut kt.base[i];
                        egui::ComboBox::from_id_salt(("kt_base", i))
                            .width(104.0)
                            .selected_text(m.kind.label())
                            .show_ui(ui, |ui| {
                                for k in [
                                    BaseKind::Door,
                                    BaseKind::Drawers,
                                    BaseKind::Void,
                                    BaseKind::Tall,
                                    BaseKind::Gap,
                                ] {
                                    ui.selectable_value(&mut m.kind, k, k.label());
                                }
                            });
                        crate::factory::length_ui(ui, &u, &mut m.width, 0.01, 0.0, 3.0, &self.calc);
                        if matches!(m.kind, BaseKind::Door | BaseKind::Drawers) {
                            ui.add(
                                egui::DragValue::new(&mut m.count)
                                    .update_while_editing(false)
                                    .range(1..=6),
                            )
                            .on_hover_text(
                                if m.kind == BaseKind::Door {
                                    "door leaves"
                                } else {
                                    "drawers"
                                },
                            );
                        }
                        if ui.small_button("🗑").clicked() {
                            rm = Some(i);
                        }
                    });
                }
                if let Some(i) = rm {
                    if kt.base.len() > 1 {
                        kt.base.remove(i);
                    }
                }
                if ui.small_button("➕ base cabinet").clicked() {
                    kt.base.push(BaseModule {
                        kind: BaseKind::Door,
                        width: 0.6,
                        count: 1,
                    });
                }

                if kt.include_wall {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Wall cabinets (upper) — Gap clears a tall unit")
                            .small()
                            .weak(),
                    );
                    let mut rmw: Option<usize> = None;
                    for i in 0..kt.wall.len() {
                        ui.horizontal(|ui| {
                            let m = &mut kt.wall[i];
                            egui::ComboBox::from_id_salt(("kt_wall", i))
                                .width(104.0)
                                .selected_text(m.kind.label())
                                .show_ui(ui, |ui| {
                                    for k in [WallKind::Door, WallKind::Open, WallKind::Gap] {
                                        ui.selectable_value(&mut m.kind, k, k.label());
                                    }
                                });
                            crate::factory::length_ui(
                                ui,
                                &u,
                                &mut m.width,
                                0.01,
                                0.0,
                                3.0,
                                &self.calc,
                            );
                            if matches!(m.kind, WallKind::Door | WallKind::Open) {
                                ui.add(
                                    egui::DragValue::new(&mut m.count)
                                        .update_while_editing(false)
                                        .range(1..=6),
                                )
                                .on_hover_text(
                                    if m.kind == WallKind::Door {
                                        "door leaves"
                                    } else {
                                        "shelves"
                                    },
                                );
                            }
                            if ui.small_button("🗑").clicked() {
                                rmw = Some(i);
                            }
                        });
                    }
                    if let Some(i) = rmw {
                        if kt.wall.len() > 1 {
                            kt.wall.remove(i);
                        }
                    }
                    if ui.small_button("➕ wall cabinet").clicked() {
                        kt.wall.push(WallModule {
                            kind: WallKind::Door,
                            width: 0.6,
                            count: 1,
                        });
                    }
                }

                let inp = kt.clone();
                ui.separator();
                match cad_solid::kitchen::plan(&inp) {
                    Ok((m, warns)) => {
                        feedback(
                            ui,
                            format!(
                                "{} base · {} wall cabinets · worktop {:.2} m · top {:.0} mm",
                                m.base_modules,
                                m.wall_modules,
                                m.worktop_len,
                                m.worktop_top * 1000.0,
                            ),
                        );
                        feedback(ui, format!(
                            "{} doors · {} drawers · {} voids · {} tall · {} shelves · {} corners · {} legs",
                            m.door_leaves, m.drawers, m.voids, m.talls, m.shelves, m.corners, m.legs,
                        ));
                        for w in warns.iter().take(3) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build kitchen").strong(),
                            ))
                            .clicked()
                        {
                            do_kitchen = true;
                        }
                    }
                    Err(e) => error(ui, e.to_string()),
                }
            }
            ArchTab::Cabin => {
                // A single CLOSE-RANGE cabinet unit: overall size, a grid of cells, a grip system,
                // panel order and banding. Full-overlay fronts tile the outline; counts are DERIVED.
                use cad_solid::cabin::{Cell, EdgeBand, Grip, PanelOrder};
                let cb = &mut self.arch_cabin;
                num(ui, &u, "Width", &mut cb.width, 0.01, 0.2, 3.0);
                num(ui, &u, "Height", &mut cb.height, 0.01, 0.2, 3.0);
                num(
                    ui,
                    &u,
                    "Depth (nominal, incl. leaf)",
                    &mut cb.depth_nominal,
                    0.005,
                    0.1,
                    1.0,
                );

                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Grip").small().weak());
                    egui::ComboBox::from_id_salt("cb_grip")
                        .width(150.0)
                        .selected_text(cb.grip.label())
                        .show_ui(ui, |ui| {
                            for g in [Grip::None, Grip::JGroove, Grip::Bar, Grip::Rail] {
                                ui.selectable_value(&mut cb.grip, g, g.label());
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Panel order").small().weak());
                    egui::ComboBox::from_id_salt("cb_panel")
                        .width(150.0)
                        .selected_text(cb.panel_order.label())
                        .show_ui(ui, |ui| {
                            for p in [PanelOrder::SidesOutside, PanelOrder::TopBottomOutside] {
                                ui.selectable_value(&mut cb.panel_order, p, p.label());
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Edge band").small().weak());
                    egui::ComboBox::from_id_salt("cb_band")
                        .width(110.0)
                        .selected_text(cb.edge_band.label())
                        .show_ui(ui, |ui| {
                            for b in [EdgeBand::Match, EdgeBand::Contrast] {
                                ui.selectable_value(&mut cb.edge_band, b, b.label());
                            }
                        });
                    ui.add(
                        egui::DragValue::new(&mut cb.shelves)
                            .update_while_editing(false)
                            .range(0..=6),
                    )
                    .on_hover_text("shelves per non-drawer cell");
                    ui.label(egui::RichText::new("shelves").small().weak());
                });
                ui.checkbox(&mut cb.pin_rows, "Shelf-pin rows");

                // Grid size — resize cols/rows/layout together (new cells default to a single door).
                let (mut n_cols, mut n_rows) = (cb.cols.len(), cb.rows.len());
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Columns × rows").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut n_cols)
                            .update_while_editing(false)
                            .speed(0.05)
                            .range(1..=6),
                    );
                    ui.label("×");
                    ui.add(
                        egui::DragValue::new(&mut n_rows)
                            .update_while_editing(false)
                            .speed(0.05)
                            .range(1..=6),
                    );
                });
                if n_cols != cb.cols.len() || n_rows != cb.rows.len() {
                    cb.cols.resize(n_cols, 1.0);
                    cb.rows.resize(n_rows, 1.0);
                    for row in cb.layout.iter_mut() {
                        row.resize(n_cols, Cell::Door(1));
                    }
                    cb.layout.resize(n_rows, vec![Cell::Door(1); n_cols]);
                }

                ui.add_space(4.0);
                ui.label(egui::RichText::new("Cells (top row first)").small().weak());
                for r in 0..cb.rows.len() {
                    ui.horizontal(|ui| {
                        for c in 0..cb.cols.len() {
                            let cell = &mut cb.layout[r][c];
                            let text = match *cell {
                                Cell::Door(n) => format!("Door×{n}"),
                                Cell::Drawers(n) => format!("Drawers×{n}"),
                                other => other.label().to_string(),
                            };
                            egui::ComboBox::from_id_salt(("cb_cell", r, c))
                                .width(84.0)
                                .selected_text(text)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(matches!(cell, Cell::Door(_)), "Door")
                                        .clicked()
                                        && !matches!(cell, Cell::Door(_))
                                    {
                                        *cell = Cell::Door(1);
                                    }
                                    if ui
                                        .selectable_label(
                                            matches!(cell, Cell::Drawers(_)),
                                            "Drawers",
                                        )
                                        .clicked()
                                        && !matches!(cell, Cell::Drawers(_))
                                    {
                                        *cell = Cell::Drawers(3);
                                    }
                                    ui.selectable_value(cell, Cell::Open, "Open");
                                    ui.selectable_value(cell, Cell::Panel, "Panel");
                                });
                            match cell {
                                Cell::Door(n) => {
                                    ui.add(
                                        egui::DragValue::new(n)
                                            .update_while_editing(false)
                                            .speed(0.1)
                                            .range(1..=3),
                                    )
                                    .on_hover_text("leaves");
                                }
                                Cell::Drawers(n) => {
                                    ui.add(
                                        egui::DragValue::new(n)
                                            .update_while_editing(false)
                                            .speed(0.1)
                                            .range(1..=8),
                                    )
                                    .on_hover_text("drawers");
                                }
                                _ => {}
                            }
                        }
                    });
                }

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Open pose").small().weak());
                    ui.add(
                        egui::DragValue::new(&mut cb.open_deg)
                            .update_while_editing(false)
                            .speed(1.0)
                            .range(0.0..=110.0)
                            .suffix("°"),
                    )
                    .on_hover_text("swing every door leaf open by this angle (0 = shut)");
                    crate::factory::length_ui(
                        ui,
                        &u,
                        &mut cb.drawer_out,
                        0.005,
                        0.0,
                        0.6,
                        &self.calc,
                    )
                    .on_hover_text("pull every drawer front out by this much");
                });

                let inp = cb.clone();
                ui.separator();
                match cad_solid::cabin::plan(&inp) {
                    Ok((m, warns)) => {
                        feedback(
                            ui,
                            format!(
                                "{} × {} cells · {:.0} × {:.0} × {:.0} mm · carcass depth {:.0} mm",
                                m.cols,
                                m.rows,
                                m.width * 1000.0,
                                m.height * 1000.0,
                                m.depth_nominal * 1000.0,
                                m.carcass_depth * 1000.0,
                            ),
                        );
                        feedback(ui, format!(
                            "{} door leaves · {} drawers · {} open · {} panels · {} shelves · {} pins",
                            m.door_leaves, m.drawer_fronts, m.opens, m.panels, m.shelves, m.pins,
                        ));
                        for w in warns.iter().take(3) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build cabinet unit").strong(),
                            ))
                            .clicked()
                        {
                            do_cabin = true;
                        }
                    }
                    Err(e) => error(ui, e.to_string()),
                }
            }
            ArchTab::SweepLight => {
                use cad_solid::sweeplight::{PathKind, ProfileKind};
                let s = &mut self.arch_sweep;
                ui.label(egui::RichText::new("A lighting profile swept along a path — body, glowing lens and hanging rods. The lens is emissive in the ⏺ raytraced render.").small().weak());
                ui.add_space(4.0);
                // Path kind + its dimensions.
                let cur = match s.path {
                    PathKind::Ring { .. } => "Ring",
                    PathKind::Racetrack { .. } => "Racetrack",
                    PathKind::SCurve { .. } => "S-curve (open)",
                    PathKind::Custom { .. } => "From 2D drawing",
                };
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 18.0], egui::Label::new("Path").selectable(false));
                    egui::ComboBox::from_id_salt("sweep_path")
                        .width(150.0)
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(matches!(s.path, PathKind::Ring { .. }), "Ring")
                                .clicked()
                            {
                                s.path = PathKind::Ring { radius: 0.4 };
                            }
                            if ui
                                .selectable_label(
                                    matches!(s.path, PathKind::Racetrack { .. }),
                                    "Racetrack",
                                )
                                .clicked()
                            {
                                s.path = PathKind::Racetrack {
                                    w: 1.8,
                                    d: 0.9,
                                    fillet: 0.25,
                                };
                            }
                            if ui
                                .selectable_label(
                                    matches!(s.path, PathKind::SCurve { .. }),
                                    "S-curve (open)",
                                )
                                .clicked()
                            {
                                s.path = PathKind::SCurve {
                                    length: 4.0,
                                    width: 0.5,
                                };
                            }
                            if ui
                                .selectable_label(
                                    matches!(s.path, PathKind::Custom { .. }),
                                    "From 2D drawing",
                                )
                                .clicked()
                            {
                                s.path = PathKind::Custom {
                                    pts: Vec::new(),
                                    closed: false,
                                    fillet: 0.15,
                                };
                            }
                        });
                });
                let mut grab = false;
                match &mut s.path {
                    PathKind::Ring { radius } => {
                        num(ui, &u, "Ring radius", radius, 0.01, 0.05, 5.0)
                    }
                    PathKind::Racetrack { w, d, fillet } => {
                        num(ui, &u, "Length", w, 0.02, 0.3, 8.0);
                        num(ui, &u, "Depth", d, 0.02, 0.3, 8.0);
                        num(ui, &u, "Corner fillet", fillet, 0.01, 0.03, 1.0);
                    }
                    PathKind::SCurve { length, width } => {
                        num(ui, &u, "Run length", length, 0.05, 0.5, 15.0);
                        num(ui, &u, "Swing (±)", width, 0.02, 0.0, 3.0);
                    }
                    PathKind::Custom {
                        pts,
                        closed,
                        fillet,
                    } => {
                        // Draw a polyline/arc/circle/ellipse in the 2D view, select it, then grab it.
                        ui.horizontal(|ui| {
                            ui.add_sized([150.0, 18.0], egui::Label::new("2D curve").selectable(false));
                            if ui
                                .button("⤓  Use selected 2D curve")
                                .on_hover_text("Sample the currently selected 2D entity (polyline, arc, circle, ellipse, line) as the sweep path. Draw and select it in the 2D view first. Coordinates are metres 1:1.")
                                .clicked()
                            {
                                grab = true;
                            }
                        });
                        if pts.is_empty() {
                            ui.label(egui::RichText::new("  no curve loaded yet — select one in the 2D view and press the button").small().weak());
                        } else {
                            ui.label(
                                egui::RichText::new(format!(
                                    "  {} points · {}",
                                    pts.len(),
                                    if *closed { "closed loop" } else { "open run" }
                                ))
                                .small()
                                .weak(),
                            );
                        }
                        num(ui, &u, "Corner fillet", fillet, 0.01, 0.0, 1.0);
                    }
                }
                if grab {
                    // Longest sampled outline among the selected 2D entities wins (a click often
                    // catches construction fluff alongside the real curve).
                    let mut got: Option<(Vec<glam::Vec2>, bool)> = None;
                    for &di in &self.selection {
                        let Some(d) = self.doc.dobjects.get(di) else {
                            continue;
                        };
                        // METRES: this curve becomes a sweep PATH the generator builds in
                        // world space, so it is a 2D→3D crossing like any other.
                        for path in
                            cad_solid::geom_outlines_scaled(&d.geom, self.doc.units.metres_per_unit)
                        {
                            if path.len() < 2 {
                                continue;
                            }
                            let len: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
                            let best: f32 = got
                                .as_ref()
                                .map(|(g, _)| g.windows(2).map(|w| w[0].distance(w[1])).sum())
                                .unwrap_or(0.0);
                            if len > best {
                                let mut p = path.clone();
                                let closed =
                                    p.len() > 3 && p[0].distance(*p.last().unwrap()) < 1e-4;
                                if closed {
                                    p.pop();
                                }
                                got = Some((p, closed));
                            }
                        }
                    }
                    match got {
                        Some((p, closed)) => {
                            let n = p.len();
                            let old_fillet = match &s.path {
                                PathKind::Custom { fillet, .. } => *fillet,
                                _ => 0.15,
                            };
                            s.path = PathKind::Custom {
                                pts: p,
                                closed,
                                fillet: old_fillet,
                            };
                            self.factory.status = format!(
                                "2D curve loaded: {n} points ({}) — set the fillet, then Build",
                                if closed { "closed" } else { "open" }
                            );
                        }
                        None => {
                            self.factory.status =
                                "no usable 2D curve selected — select a polyline, arc, circle or ellipse in the 2D view first".into();
                        }
                    }
                }
                // Cross-section (md C9 control 1).
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Cross-section").selectable(false),
                    );
                    egui::ComboBox::from_id_salt("sweep_profile")
                        .width(150.0)
                        .selected_text(s.profile.label())
                        .show_ui(ui, |ui| {
                            for p in ProfileKind::ALL {
                                ui.selectable_value(&mut s.profile, p, p.label());
                            }
                        });
                });
                num(ui, &u, "Profile width", &mut s.width, 0.002, 0.02, 0.3);
                num(ui, &u, "Profile height", &mut s.height, 0.002, 0.02, 0.4);
                num(ui, &u, "Lens size", &mut s.lens, 0.002, 0.01, 0.3);
                // Drop (md C9 control 2) + spacing (control 3).
                num(ui, &u, "Drop (ceiling→top)", &mut s.drop, 0.01, 0.1, 4.0);
                if matches!(s.path, PathKind::SCurve { .. }) {
                    num(ui, &u, "Drop at far end", &mut s.drop_end, 0.01, 0.1, 4.0);
                } else {
                    s.drop_end = s.drop;
                }
                num(ui, &u, "Hanger spacing", &mut s.spacing, 0.02, 0.3, 3.0);

                // ---- OUTPUT: what makes this a light rather than a glowing shape ----------------
                //
                // Until these existed a curved light was furniture with an emissive texture: it
                // glowed in the render and contributed NOTHING to a calculation, which is the most
                // misleading state a lighting tool can be in — it looks lit and computes dark.
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Output").strong());
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 18.0], egui::Label::new("Load").selectable(false));
                    ui.add(
                        egui::DragValue::new(&mut s.watts_per_m)
                            .update_while_editing(false)
                            .speed(0.1)
                            .range(0.0..=100.0)
                            .suffix(" W/m"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Efficacy").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.efficacy_lm_per_w)
                            .update_while_editing(false)
                            .speed(1.0)
                            .range(0.0..=250.0)
                            .suffix(" lm/W"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Colour temperature").selectable(false),
                    );
                    ui.add(
                        egui::DragValue::new(&mut s.cct_k)
                            .update_while_editing(false)
                            .speed(50.0)
                            .range(1800..=8000)
                            .suffix(" K"),
                    );
                });
                // CCT is deliberately NOT in the lux maths, and the UI has to say so or someone
                // will change it expecting the numbers to move. Lux and candela are already
                // V(λ)-weighted: 3000 K and 6000 K at the same flux give the same illuminance.
                ui.label(
                    egui::RichText::new(
                        "Colour temperature sets the lens tint and is reported — it does not change the lux, because photometric units are already eye-weighted.",
                    )
                    .small()
                    .weak(),
                );

                // Live feedback: run the builder for its metrics/validation (fast — a few k tris).
                match cad_solid::sweeplight::build(s) {
                    Ok((m, _, _)) => {
                        feedback(ui, format!(
                            "path {:.2} m · {} hangers at {:.2} m (asked {:.2}) · min radius {:.0} mm · {} tris",
                            m.path_len, m.droppers, m.achieved_spacing, s.spacing, m.min_radius * 1000.0, m.tris,
                        ));
                        // What it will contribute to the CALCULATION, quoted before the build so a
                        // fitting specified with no output is visible as such rather than silently
                        // producing a shape that lights nothing.
                        let spacing = emitter_spacing_for(m.path_len);
                        let em = cad_solid::sweeplight::emitters(s, spacing);
                        let lm: f64 = em.iter().map(|e| e.lumens).sum();
                        let w: f64 = em.iter().map(|e| e.watts).sum();
                        feedback(
                            ui,
                            if em.is_empty() {
                                "no light output — set a load and an efficacy above zero"
                                    .to_string()
                            } else {
                                format!(
                                    "{lm:.0} lm · {w:.1} W · {} emitting points at {spacing:.2} m",
                                    em.len()
                                )
                            },
                        );
                        // Say it BEFORE the build when a long run has been sampled more coarsely
                        // than the default, rather than letting the number appear in a report.
                        if spacing > EMITTER_SPACING_M * 1.001 {
                            ui.label(
                                egui::RichText::new(format!(
                                    "  ⓘ {:.0} m of run — sampled every {spacing:.2} m instead of {EMITTER_SPACING_M:.2} m to keep the calculation finite. Total output is unchanged; only the field within ~{spacing:.1} m of the fitting is coarser.",
                                    m.path_len,
                                ))
                                .small()
                                .weak(),
                            );
                        }
                        for w in m.warnings.iter().take(3) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build curved light").strong(),
                            ))
                            .clicked()
                        {
                            do_sweep = true;
                        }
                    }
                    Err(e) => error(ui, e),
                }
            }
            ArchTab::Desk => {
                use cad_solid::desk::{Grommet, PartPos, PedFace, Preset, Support};
                ui.label(
                    egui::RichText::new(
                        "A workstation desk as a FEATURE TREE — switch any feature off and it is gone from the build (a deleted cable port takes its hole with it).",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);
                // Presets first: they overwrite everything, so they read as "start from".
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Start from").selectable(false),
                    );
                    egui::ComboBox::from_id_salt("desk_preset")
                        .width(150.0)
                        .selected_text("Preset…")
                        .show_ui(ui, |ui| {
                            for p in Preset::ALL {
                                if ui.selectable_label(false, p.label()).clicked() {
                                    self.arch_desk = p.input();
                                }
                            }
                        });
                });
                let d = &mut self.arch_desk;
                num(ui, &u, "Length", &mut d.length, 0.01, 1.0, 3.0);
                num(ui, &u, "Depth", &mut d.width, 0.01, 0.5, 1.4);
                num(ui, &u, "Surface height", &mut d.height, 0.005, 0.60, 0.90);

                ui.separator();
                ui.checkbox(&mut d.partition, "Privacy screen");
                if d.partition {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [150.0, 18.0],
                            egui::Label::new("Screen position").selectable(false),
                        );
                        egui::ComboBox::from_id_salt("desk_partpos")
                            .width(150.0)
                            .selected_text(d.part_pos.label())
                            .show_ui(ui, |ui| {
                                for p in PartPos::ALL {
                                    ui.selectable_value(&mut d.part_pos, p, p.label());
                                }
                            });
                    });
                    num(ui, &u, "Screen span", &mut d.part_w, 0.01, 0.3, 3.0);
                    num(ui, &u, "Screen height", &mut d.part_h, 0.01, 0.1, 1.0);
                    ui.checkbox(&mut d.part_band, "Organiser band at the base");
                }

                ui.separator();
                for (label, sup) in [("Left end", &mut d.sup_l), ("Right end", &mut d.sup_r)] {
                    ui.horizontal(|ui| {
                        ui.add_sized([150.0, 18.0], egui::Label::new(label).selectable(false));
                        egui::ComboBox::from_id_salt(format!("desk_sup_{label}"))
                            .width(150.0)
                            .selected_text(sup.label())
                            .show_ui(ui, |ui| {
                                for s in Support::ALL {
                                    ui.selectable_value(sup, s, s.label());
                                }
                            });
                    });
                }
                if d.sup_l == Support::Drawers || d.sup_r == Support::Drawers {
                    num(ui, &u, "Pedestal width", &mut d.ped_w, 0.01, 0.25, 0.8);
                    ui.horizontal(|ui| {
                        ui.add_sized([150.0, 18.0], egui::Label::new("Drawers").selectable(false));
                        ui.add(
                            egui::DragValue::new(&mut d.ped_n)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(1..=6),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [150.0, 18.0],
                            egui::Label::new("Fronts open on").selectable(false),
                        );
                        egui::ComboBox::from_id_salt("desk_pedface")
                            .width(150.0)
                            .selected_text(d.ped_face.label())
                            .show_ui(ui, |ui| {
                                for f in PedFace::ALL {
                                    ui.selectable_value(&mut d.ped_face, f, f.label());
                                }
                            });
                    });
                }
                ui.checkbox(&mut d.rail, "Under-top rail");

                ui.separator();
                // Cable ports — the only subtractive feature, so each row owns a hole in the top.
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Cable ports").selectable(false),
                    );
                    if ui.button("✚").on_hover_text("Add a cable port").clicked() {
                        d.grommets.push(Grommet::rear_at(0.0));
                    }
                    if ui
                        .button("✖")
                        .on_hover_text("Remove the last cable port — its hole goes with it")
                        .clicked()
                    {
                        d.grommets.pop();
                    }
                });
                for (i, g) in d.grommets.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [56.0, 18.0],
                            egui::Label::new(format!("  #{}", i + 1)).selectable(false),
                        );
                        crate::factory::length_ui_pre(
                            ui, &u, "x ", &mut g.x, 0.01, -1e4, 1e4, &self.calc,
                        );
                        ui.checkbox(&mut g.rear, "rear");
                        if !g.rear {
                            crate::factory::length_ui_pre(
                                ui, &u, "y ", &mut g.y, 0.01, -1e4, 1e4, &self.calc,
                            );
                        }
                    });
                }

                // Live feedback: run the builder for its metrics/validation (fast — a few k tris).
                match cad_solid::desk::build(d) {
                    Ok((m, _, _)) => {
                        feedback(
                            ui,
                            format!(
                                "{:.0}×{:.0} mm at {:.0} mm · {} · {} tris",
                                m.length * 1000.0,
                                m.width * 1000.0,
                                m.height * 1000.0,
                                m.features.join(", "),
                                m.tris,
                            ),
                        );
                        for w in m.warnings.iter().take(3) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build desk").strong(),
                            ))
                            .clicked()
                        {
                            do_desk = true;
                        }
                    }
                    Err(e) => error(ui, e),
                }
            }
            ArchTab::Couch => {
                use cad_solid::couch::{BackKind, Preset, Run};
                ui.label(
                    egui::RichText::new(
                        "A sofa as a RUN CHAIN — each extra run adds a +90° corner unit, so one to three runs give you a two-seater, an L or a U. The seats always face the inside of the turn.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Start from").selectable(false),
                    );
                    egui::ComboBox::from_id_salt("couch_preset")
                        .width(150.0)
                        .selected_text("Preset…")
                        .show_ui(ui, |ui| {
                            for p in Preset::ALL {
                                if ui.selectable_label(false, p.label()).clicked() {
                                    self.arch_couch = p.input();
                                }
                            }
                        });
                });
                let c = &mut self.arch_couch;

                // The chain itself: one row per run, length + cushion count (0 = auto).
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 18.0],
                        egui::Label::new("Runs (chain)").selectable(false),
                    );
                    if ui
                        .button("✚")
                        .on_hover_text("Add a run — each one past the first adds a corner unit")
                        .clicked()
                    {
                        c.runs.push(Run::new(1.350, 0));
                    }
                    if ui
                        .button("✖")
                        .on_hover_text("Remove the last run")
                        .clicked()
                        && c.runs.len() > 1
                    {
                        c.runs.pop();
                    }
                });
                for (i, r) in c.runs.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [56.0, 18.0],
                            egui::Label::new(format!("  #{}", i + 1)).selectable(false),
                        );
                        crate::factory::length_ui(
                            ui,
                            &u,
                            &mut r.length,
                            0.01,
                            0.4,
                            6.0,
                            &self.calc,
                        );
                        ui.add(
                            egui::DragValue::new(&mut r.cushions)
                                .update_while_editing(false)
                                .speed(0.1)
                                .range(0..=8)
                                .custom_formatter(|n, _| {
                                    if n < 0.5 {
                                        "auto".to_string()
                                    } else {
                                        format!("{n:.0}")
                                    }
                                }),
                        )
                        .on_hover_text(
                            "Seat cushions across this run — 0 is auto (aims at a 667 mm pitch)",
                        );
                    });
                }

                ui.separator();
                num(ui, &u, "Depth", &mut c.depth, 0.01, 0.5, 1.2);
                num(ui, &u, "Seat height", &mut c.seat_top, 0.005, 0.25, 0.6);
                num(ui, &u, "Back height", &mut c.back_h, 0.005, 0.3, 1.1);
                num(ui, &u, "Arm height", &mut c.arm_h, 0.005, 0.2, 0.9);
                num(
                    ui,
                    &u,
                    "Cushion thickness",
                    &mut c.cushion_t,
                    0.005,
                    0.05,
                    0.35,
                );

                ui.separator();
                ui.label(
                    egui::RichText::new("  Features — switch any of these off")
                        .small()
                        .weak(),
                );
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 18.0], egui::Label::new("Back").selectable(false));
                    egui::ComboBox::from_id_salt("couch_back")
                        .width(150.0)
                        .selected_text(c.back.label())
                        .show_ui(ui, |ui| {
                            for b in BackKind::ALL {
                                ui.selectable_value(&mut c.back, b, b.label());
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 18.0], egui::Label::new("Arms").selectable(false));
                    ui.checkbox(&mut c.arm_start, "start");
                    ui.checkbox(&mut c.arm_end, "end");
                })
                .response
                .on_hover_text("Arms only ever sit at the two FREE ends of the chain — never on an interior junction");
                ui.checkbox(&mut c.frame, "Frame (seat boxes + corners)");
                ui.checkbox(&mut c.seat_cushions, "Seat cushions");
                ui.checkbox(&mut c.back_cushions, "Back cushions");

                // Live feedback: run the builder for its metrics/validation (fast — a few k tris).
                match cad_solid::couch::build(c) {
                    Ok((m, _, _)) => {
                        feedback(
                            ui,
                            format!(
                                "{} run{} + {} corner{} · {:.2} m overall · {} seats at {:.0} mm pitch · {} tris",
                                m.runs,
                                if m.runs == 1 { "" } else { "s" },
                                m.corners,
                                if m.corners == 1 { "" } else { "s" },
                                m.overall_len,
                                m.seats,
                                m.cushion_pitch * 1000.0,
                                m.tris,
                            ),
                        );
                        for w in m.warnings.iter().take(3) {
                            ui.label(egui::RichText::new(format!("  ⚠ {w}")).small().weak());
                        }
                        if ui
                            .add(egui::Button::new(
                                egui::RichText::new("✚  Build sofa").strong(),
                            ))
                            .clicked()
                        {
                            do_couch = true;
                        }
                    }
                    Err(e) => error(ui, e),
                }
            }
        }

        if let Some((result, name)) = build {
            match result {
                Ok(mesh) => self.arch_build_and_place(mesh, &name),
                Err(e) => self.factory.status = format!("{name}: {e}"),
            }
        }
        if do_dogleg {
            let inp = self.arch_dogleg;
            let with_treads = self.arch_dogleg_treads;
            self.factory_build_dogleg(&inp, with_treads);
        }
        if do_spiral_csg {
            let inp = self.arch_spiral_csg;
            self.factory_build_spiral_csg(&inp);
        }
        if do_door {
            let inp = self.arch_door;
            self.factory_build_door(&inp);
        }
        if do_cupboard {
            let inp = self.arch_cupboard.clone();
            self.factory_build_cupboard(&inp);
        }
        if do_kitchen {
            let inp = self.arch_kitchen.clone();
            self.factory_build_kitchen(&inp);
        }
        if do_sweep {
            let inp = self.arch_sweep.clone();
            self.factory_build_sweeplight(&inp);
        }
        if do_cabin {
            let inp = self.arch_cabin.clone();
            self.factory_build_cabin(&inp);
        }
        if do_desk {
            let inp = self.arch_desk.clone();
            self.factory_build_desk(&inp);
        }
        if do_couch {
            let inp = self.arch_couch.clone();
            self.factory_build_couch(&inp);
        }
    }

    /// Build the parametric sofa chain from the dialog and drop it at the model centre. The oak
    /// frame, arms and spindles ride a procedural veneer; the pillows get a fabric swatch. Each
    /// cushion, arm and frame box is its own selectable, paintable part. Mirrors
    /// [`Self::factory_build_desk`].
    fn factory_build_couch(&mut self, inp: &cad_solid::couch::CouchInput) {
        use cad_solid::couch::Material;
        let features_before = self.factory.model.features.len();
        let (m, mesh, mats) = match cad_solid::couch::build(inp) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Sofa: {e}");
                return;
            }
        };
        let part_ids = mesh.face_ids.clone();
        let obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions,
            normals: mesh.normals,
            color: Some([0.76, 0.69, 0.57]), // upholstery — the Fabric parts ride this
            alpha: Vec::new(),
        };
        self.snapshot_factory();
        let idx = self.factory.add_furniture_asset("Sofa".to_string(), obj);
        if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
            a.alpha_resolved = true;
            if part_ids.len() == a.positions.len() / 3 {
                a.part_ids = part_ids;
            }
        }
        // The frame is one continuous piece of joinery, so the grain wants to run across the boxes —
        // the procedural veneer is evaluated from world position and does exactly that.
        let oak = self
            .factory
            .add_procedural_texture("Sofa oak frame".into(), crate::factory::ProcDef::oak());
        let fabric =
            self.factory
                .add_texture("Sofa upholstery".into(), 1, 1, vec![194, 176, 145, 255]);
        if let Some(t) = self.factory.textures.get_mut(fabric) {
            t.roughness = 0.90; // woven — no specular hotspot on a cushion
        }
        let per_part_tex: Vec<Option<usize>> = mats
            .iter()
            .map(|mat| match mat {
                Material::Oak => Some(oak),
                Material::Fabric => Some(fabric),
            })
            .collect();

        let at = self.factory.place_at();
        self.factory.place_furniture(idx, at);
        let fg_tex = self.factory.furniture_lib.get(idx).map(|a| {
            let g = a.group_geom();
            let ntri = a.positions.len() / 3;
            let mut map: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for t in 0..ntri {
                let part = a.part_ids.get(t).copied().unwrap_or(0) as usize;
                if let Some(Some(gtex)) = per_part_tex.get(part) {
                    map.insert(g.face[t], *gtex);
                }
            }
            map
        });
        if let (Some(fi), Some(fg_tex)) = (self.factory.sel_furn_primary(), fg_tex) {
            if let Some(inst) = self.factory.furniture.get_mut(fi) {
                inst.surface_texture = fg_tex;
            }
        }
        self.factory.open = true;
        self.active_view = ActiveView::ThreeD;
        self.factory.status = format!(
            "Sofa: {} run(s) + {} corner(s), {:.2} m overall, {} seats — {} ({} tris)",
            m.runs,
            m.corners,
            m.overall_len,
            m.seats,
            m.features.join(", "),
            m.tris,
        );
        for w in m.warnings.iter().take(2) {
            self.history.push(format!("  ⚠ sofa: {w}"));
        }
        self.history.push(format!(
            "  furniture: sofa ({} runs, {} seats, {} tris) and placed",
            m.runs, m.seats, m.tris
        ));
        self.factory_op_evt(
            "couch",
            "modal",
            format!(
                "runs={} corners={} overall={:.3} seats={} tris={}",
                m.runs, m.corners, m.overall_len, m.seats, m.tris
            ),
            features_before,
        );
    }

    /// Build the parametric office desk from the dialog and drop it at the model centre as a
    /// furniture piece. Each feature — and each individual drawer front — is its own selectable,
    /// paintable part; the white carcass rides the asset's base colour and everything else gets a
    /// bound swatch. Mirrors [`Self::factory_build_cabin`].
    fn factory_build_desk(&mut self, inp: &cad_solid::desk::DeskInput) {
        use cad_solid::desk::Material;
        let features_before = self.factory.model.features.len();
        let (m, mesh, mats) = match cad_solid::desk::build(inp) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Office desk: {e}");
                return;
            }
        };
        let part_ids = mesh.face_ids.clone();
        let obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions,
            normals: mesh.normals,
            color: Some([0.90, 0.90, 0.89]), // white laminate — the White parts stay on this
            alpha: Vec::new(),
        };
        self.snapshot_factory();
        let idx = self
            .factory
            .add_furniture_asset("Office desk".to_string(), obj);
        if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
            a.alpha_resolved = true;
            if part_ids.len() == a.positions.len() / 3 {
                a.part_ids = part_ids;
            }
        }
        let sw = |app: &mut Self, name: &str, rgb: [u8; 3]| {
            app.factory
                .add_texture(name.into(), 1, 1, vec![rgb[0], rgb[1], rgb[2], 255])
        };
        // Drawer fronts get the PROCEDURAL oak veneer so the grain runs on across the stack, the
        // same trick the cabinet unit uses.
        let oak = self
            .factory
            .add_procedural_texture("Desk oak veneer".into(), crate::factory::ProcDef::oak());
        let fabric = sw(self, "Desk screen fabric", [143, 146, 158]);
        if let Some(t) = self.factory.textures.get_mut(fabric) {
            t.roughness = 0.92; // woven — kills the specular that would make it read as plastic
        }
        let metal = sw(self, "Desk legs / rail", [198, 200, 204]);
        if let Some(t) = self.factory.textures.get_mut(metal) {
            t.metallic = 0.85;
            t.roughness = 0.35;
        }
        let alu = sw(self, "Desk trim (alu)", [204, 204, 209]);
        if let Some(t) = self.factory.textures.get_mut(alu) {
            t.metallic = 1.0;
            t.roughness = 0.30;
        }
        let dark = sw(self, "Desk dark detail", [26, 26, 28]);
        let cap = sw(self, "Desk foot caps", [219, 219, 219]);
        let per_part_tex: Vec<Option<usize>> = mats
            .iter()
            .map(|mat| match mat {
                Material::White => None,
                Material::Oak => Some(oak),
                Material::Fabric => Some(fabric),
                Material::Metal => Some(metal),
                Material::Alu => Some(alu),
                Material::Dark => Some(dark),
                Material::Cap => Some(cap),
            })
            .collect();

        let at = self.factory.place_at();
        self.factory.place_furniture(idx, at);
        let fg_tex = self.factory.furniture_lib.get(idx).map(|a| {
            let g = a.group_geom();
            let ntri = a.positions.len() / 3;
            let mut map: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for t in 0..ntri {
                let part = a.part_ids.get(t).copied().unwrap_or(0) as usize;
                if let Some(Some(gtex)) = per_part_tex.get(part) {
                    map.insert(g.face[t], *gtex);
                }
            }
            map
        });
        if let (Some(fi), Some(fg_tex)) = (self.factory.sel_furn_primary(), fg_tex) {
            if let Some(inst) = self.factory.furniture.get_mut(fi) {
                inst.surface_texture = fg_tex;
            }
        }
        self.factory.open = true;
        self.active_view = ActiveView::ThreeD;
        self.factory.status = format!(
            "Office desk: {:.0}×{:.0} mm at {:.0} mm — {} ({} tris)",
            m.length * 1000.0,
            m.width * 1000.0,
            m.height * 1000.0,
            m.features.join(", "),
            m.tris,
        );
        for w in m.warnings.iter().take(2) {
            self.history.push(format!("  ⚠ desk: {w}"));
        }
        self.history.push(format!(
            "  furniture: office desk ({}, {} tris) and placed",
            m.features.join(", "),
            m.tris
        ));
        self.factory_op_evt(
            "desk",
            "modal",
            format!(
                "L={:.3} W={:.3} H={:.3} features={} tris={}",
                m.length,
                m.width,
                m.height,
                m.features.len(),
                m.tris
            ),
            features_before,
        );
    }

    /// Build the parametric door from the dialog and drop it at the model centre as a furniture
    /// piece (no wall cut) — the "insert by parameters" path. Each component is a selectable piece.
    pub(super) fn factory_build_door(&mut self, inp: &cad_solid::door::DoorInput) {
        let (m, obj, part_ids, handle_note) = match self.door_mesh(inp) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Door: {e}");
                return;
            }
        };
        self.factory_place_furniture_mesh(obj, "Door", "built", part_ids);
        if let Some(fi) = self.factory.sel_furn_primary() {
            self.door_bind_materials(fi);
        }
        let glazed = if self.door_mats.has_glass() {
            " · glazed"
        } else {
            ""
        };
        self.factory.status = format!(
            "Door built{handle_note}{glazed} — structural opening {:.0} × {:.0} mm (leave this hole in the wall)",
            m.structural_opening_w * 1000.0, m.structural_opening_h * 1000.0,
        );
    }

    /// Build the parametric CABINET from the grid dialog and drop it at the model centre as a
    /// furniture piece (each component selectable). Unlike a plain generated mesh this one is
    /// MULTI-MATERIAL: glass panes and chrome pulls get their own flat swatch bound per part (same
    /// mechanism as a glTF import), while wood parts keep the asset's base colour.
    fn factory_build_cupboard(&mut self, inp: &cad_solid::cupboard::CupboardInput) {
        use cad_solid::cupboard::Material;
        let features_before = self.factory.model.features.len();
        let (m, mesh, mats) = match cad_solid::cupboard::build(inp) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Cupboard: {e}");
                return;
            }
        };
        let tris = mesh.tri_count();
        let part_ids = mesh.face_ids.clone();
        let obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions,
            normals: mesh.normals,
            color: Some([0.62, 0.512, 0.348]), // oak — the wood default; glass/chrome bound below
            alpha: Vec::new(),
        };
        self.snapshot_factory();
        let idx = self
            .factory
            .add_furniture_asset("Cupboard".to_string(), obj);
        if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
            a.alpha_resolved = true;
            if part_ids.len() == a.positions.len() / 3 {
                a.part_ids = part_ids;
            }
        }
        // One flat 1×1 swatch each for glass and chrome; wood parts stay untextured (base colour).
        let glass_tex =
            self.factory
                .add_texture("Cupboard glass".into(), 1, 1, vec![194, 202, 205, 255]);
        let chrome_tex =
            self.factory
                .add_texture("Cupboard chrome".into(), 1, 1, vec![196, 200, 205, 255]);
        let per_part_tex: Vec<Option<usize>> = mats
            .iter()
            .map(|mat| match mat {
                Material::Glass => Some(glass_tex),
                Material::Chrome => Some(chrome_tex),
                Material::Wood => None,
            })
            .collect();

        let at = self.factory.place_at();
        self.factory.place_furniture(idx, at); // selects the new instance
                                               // Bind each part's material to its face-groups (mirror the glTF multi-material import). The
                                               // immutable asset borrow is closed before the mutable instance borrow.
        let fg_tex = self.factory.furniture_lib.get(idx).map(|a| {
            let g = a.group_geom();
            let ntri = a.positions.len() / 3;
            let mut map: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for t in 0..ntri {
                let part = a.part_ids.get(t).copied().unwrap_or(0) as usize;
                if let Some(Some(gtex)) = per_part_tex.get(part) {
                    map.insert(g.face[t], *gtex);
                }
            }
            map
        });
        if let (Some(fi), Some(fg_tex)) = (self.factory.sel_furn_primary(), fg_tex) {
            if let Some(inst) = self.factory.furniture.get_mut(fi) {
                inst.surface_texture = fg_tex;
            }
        }
        self.factory.open = true;
        self.factory.status = format!(
            "Cupboard built — {} doors · {} glazed · {} drawers · {} niches ({} tris)",
            m.doors, m.glazed, m.drawers, m.niches, tris,
        );
        self.history.push(format!(
            "  architecture: cupboard ({} bays × {} tiers, {} fronts, {} tris) and placed",
            m.n_cols, m.n_rows, m.total_fronts, tris,
        ));
        self.factory_op_evt(
            "cupboard",
            "modal",
            format!(
                "bays={} tiers={} fronts={} handles={} tris={}",
                m.n_cols, m.n_rows, m.total_fronts, m.handles, tris
            ),
            features_before,
        );
    }

    /// Build the parametric KITCHEN run and drop it at the model centre as a multi-material
    /// furniture piece. Carcass keeps the asset base colour; fronts / stone / plinth / chrome each
    /// get a flat swatch bound per part (same mechanism as [`Self::factory_build_cupboard`]).
    fn factory_build_kitchen(&mut self, inp: &cad_solid::kitchen::KitchenInput) {
        use cad_solid::kitchen::Material;
        let features_before = self.factory.model.features.len();
        let (m, mesh, mats) = match cad_solid::kitchen::build(inp) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Kitchen: {e}");
                return;
            }
        };
        let tris = mesh.tri_count();
        let part_ids = mesh.face_ids.clone();
        let obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions,
            normals: mesh.normals,
            color: Some([0.76, 0.758, 0.75]), // carcass grey (the base colour); other mats bound below
            alpha: Vec::new(),
        };
        self.snapshot_factory();
        let idx = self.factory.add_furniture_asset("Kitchen".to_string(), obj);
        if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
            a.alpha_resolved = true;
            if part_ids.len() == a.positions.len() / 3 {
                a.part_ids = part_ids;
            }
        }
        // One flat swatch per non-carcass material; carcass parts stay at the base colour.
        let sw = |app: &mut Self, name: &str, rgb: [u8; 3]| {
            app.factory
                .add_texture(name.into(), 1, 1, vec![rgb[0], rgb[1], rgb[2], 255])
        };
        let front = sw(self, "Kitchen front", [120, 22, 20]);
        let stone = sw(self, "Kitchen worktop", [179, 173, 156]);
        let edge = sw(self, "Kitchen edge", [31, 31, 32]);
        let plinth = sw(self, "Kitchen plinth", [60, 61, 64]);
        let metal = sw(self, "Kitchen chrome", [189, 192, 196]);
        let per_part_tex: Vec<Option<usize>> = mats
            .iter()
            .map(|mat| match mat {
                Material::Carcass => None,
                Material::Front => Some(front),
                Material::Stone => Some(stone),
                Material::Edge => Some(edge),
                Material::Plinth => Some(plinth),
                Material::Metal => Some(metal),
            })
            .collect();

        let at = self.factory.place_at();
        self.factory.place_furniture(idx, at);
        let fg_tex = self.factory.furniture_lib.get(idx).map(|a| {
            let g = a.group_geom();
            let ntri = a.positions.len() / 3;
            let mut map: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for t in 0..ntri {
                let part = a.part_ids.get(t).copied().unwrap_or(0) as usize;
                if let Some(Some(gtex)) = per_part_tex.get(part) {
                    map.insert(g.face[t], *gtex);
                }
            }
            map
        });
        if let (Some(fi), Some(fg_tex)) = (self.factory.sel_furn_primary(), fg_tex) {
            if let Some(inst) = self.factory.furniture.get_mut(fi) {
                inst.surface_texture = fg_tex;
            }
        }
        self.factory.open = true;
        self.factory.status = format!(
            "Kitchen built — {} base + {} wall cabinets, {} doors, {} drawers ({} tris)",
            m.base_modules, m.wall_modules, m.door_leaves, m.drawers, tris,
        );
        self.history.push(format!(
            "  architecture: kitchen run ({} base, {} wall, upper={}, {} tris) and placed",
            m.base_modules, m.wall_modules, inp.include_wall, tris,
        ));
        self.factory_op_evt(
            "kitchen",
            "modal",
            format!(
                "base={} wall={} upper={} doors={} drawers={} tris={}",
                m.base_modules, m.wall_modules, inp.include_wall, m.door_leaves, m.drawers, tris
            ),
            features_before,
        );
    }

    /// Build the parametric handleless CABINET UNIT from the dialog and drop it at the model centre
    /// as a MULTI-MATERIAL furniture piece: the carcass keeps the asset's white base colour; front /
    /// edge-band-and-pins / metal parts each get their own flat swatch bound per part (same
    /// mechanism as [`Self::factory_build_kitchen`] / a glTF import). Each component is selectable.
    /// Build the curved luminaire as a multi-material furniture asset: dark anodised body, an
    /// EMISSIVE warm lens (glows in the path tracers; bright in the raster), metal suspension.
    fn factory_build_sweeplight(&mut self, inp: &cad_solid::sweeplight::SweepInput) {
        use cad_solid::sweeplight::Material;
        let (m, mesh, mats) = match cad_solid::sweeplight::build(inp) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Curved light: {e}");
                return;
            }
        };
        let part_ids = mesh.face_ids.clone();
        // The emitting points, in the SAME frame the mesh arrives in — so they survive the
        // recentre/rebase the asset library applies to it, below.
        let spacing = emitter_spacing_for(m.path_len);
        let emitters = cad_solid::sweeplight::emitters(inp, spacing);
        let rebase = crate::factory::FactoryState::asset_rebase(&mesh.positions);
        let obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions,
            normals: mesh.normals,
            color: Some([0.13, 0.13, 0.14]), // dark anodised base (body parts stay on this)
            alpha: Vec::new(),
        };
        self.snapshot_factory();
        // A unique name per fitting: each run has its own flux per point, so each needs its own
        // synthesised photometry, and the profile is keyed by this name.
        let n = self
            .factory
            .furniture_lib
            .iter()
            .filter(|a| a.name.starts_with("Curved light"))
            .count()
            + 1;
        let idx = self
            .factory
            .add_furniture_asset(format!("Curved light {n}"), obj);
        let total_lm: f64 = emitters.iter().map(|e| e.lumens).sum();
        let total_w: f64 = emitters.iter().map(|e| e.watts).sum();
        let n_em = emitters.len();
        if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
            a.alpha_resolved = true;
            if part_ids.len() == a.positions.len() / 3 {
                a.part_ids = part_ids;
            }
            let (off, k) = rebase.unwrap_or(([0.0; 3], 1.0));
            a.emitters = emitters
                .iter()
                .map(|e| crate::factory::FurnEmitter {
                    pos: [
                        (e.pos[0] - off[0]) * k,
                        (e.pos[1] - off[1]) * k,
                        (e.pos[2] - off[2]) * k,
                    ],
                    lumens: e.lumens,
                    watts: e.watts,
                })
                .collect();
            a.cct_k = inp.cct_k;
        }
        // Lens: EMISSIVE material — the part that glows in the raytraced render. Its colour is the
        // chosen CCT rather than a fixed warm tint, so a 4000 K fitting no longer renders as 2700 K
        // while its own dialog says otherwise.
        let tint = crate::factory::cct_to_linear_rgb(inp.cct_k);
        let srgb = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
        let lens = self.factory.add_texture(
            format!("Light lens {}K (emissive)", inp.cct_k),
            1,
            1,
            vec![srgb(tint[0]), srgb(tint[1]), srgb(tint[2]), 255],
        );
        if let Some(t) = self.factory.textures.get_mut(lens) {
            t.emission = tint;
            t.emission_strength = 6.0;
            t.roughness = 0.35;
        }
        let rod =
            self.factory
                .add_texture("Light suspension".into(), 1, 1, vec![120, 122, 126, 255]);
        if let Some(t) = self.factory.textures.get_mut(rod) {
            t.metallic = 0.8;
            t.roughness = 0.35;
        }
        let per_part_tex: Vec<Option<usize>> = mats
            .iter()
            .map(|mat| match mat {
                Material::Body => None, // stays on the dark base colour
                Material::Lens => Some(lens),
                Material::Rod => Some(rod),
            })
            .collect();
        let at = self.factory.place_at();
        self.factory.place_furniture(idx, at);
        let fg_tex = self.factory.furniture_lib.get(idx).map(|a| {
            let g = a.group_geom();
            let ntri = a.positions.len() / 3;
            let mut map: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for t in 0..ntri {
                let part = a.part_ids.get(t).copied().unwrap_or(0) as usize;
                if let Some(Some(gtex)) = per_part_tex.get(part) {
                    map.insert(g.face[t], *gtex);
                }
            }
            map
        });
        if let (Some(fi), Some(fg_tex)) = (self.factory.sel_furn_primary(), fg_tex) {
            if let Some(inst) = self.factory.furniture.get_mut(fi) {
                inst.surface_texture = fg_tex;
            }
        }
        self.factory.status = if n_em == 0 {
            format!(
                "Curved light: {:.2} m path · {} hangers at {:.2} m · {} tris — NO LIGHT OUTPUT (set a load and an efficacy above zero)",
                m.path_len, m.droppers, m.achieved_spacing, m.tris,
            )
        } else {
            format!(
                "Curved light: {:.2} m path · {} hangers at {:.2} m · {} tris · {total_lm:.0} lm / {total_w:.1} W over {n_em} emitters at {} K — counted in ⚡ Calculate",
                m.path_len, m.droppers, m.achieved_spacing, m.tris, inp.cct_k,
            )
        };
        self.history.push(format!(
            "  built curved light ({:.2} m, {} hangers)",
            m.path_len, m.droppers
        ));
        self.factory.open = true;
        self.active_view = ActiveView::ThreeD;
    }

    fn factory_build_cabin(&mut self, inp: &cad_solid::cabin::CabinInput) {
        use cad_solid::cabin::Material;
        let features_before = self.factory.model.features.len();
        let (m, mesh, mats) = match cad_solid::cabin::build(inp) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Cabinet unit: {e}");
                return;
            }
        };
        let tris = mesh.tri_count();
        let part_ids = mesh.face_ids.clone();
        let obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions,
            normals: mesh.normals,
            color: Some([0.92, 0.91, 0.885]), // white carcass (the base colour); fronts/edge/metal bound below
            alpha: Vec::new(),
        };
        self.snapshot_factory();
        let idx = self
            .factory
            .add_furniture_asset("Cabinet unit".to_string(), obj);
        if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
            a.alpha_resolved = true;
            if part_ids.len() == a.positions.len() / 3 {
                a.part_ids = part_ids;
            }
        }
        // One flat swatch per non-carcass material; carcass parts stay at the white base colour.
        let sw = |app: &mut Self, name: &str, rgb: [u8; 3]| {
            app.factory
                .add_texture(name.into(), 1, 1, vec![rgb[0], rgb[1], rgb[2], 255])
        };
        // Fronts get a PROCEDURAL oak veneer (evaluated in-shader from world position) so the grain
        // runs continuously across the leaves — Blender's object-coordinate trick, ported here.
        let front = self
            .factory
            .add_procedural_texture("Cabinet oak veneer".into(), crate::factory::ProcDef::oak());
        let edge = sw(self, "Cabinet edge/pins", [40, 40, 42]); // dark banding + pin holes
        let metal = sw(self, "Cabinet metal", [190, 192, 196]); // handles / hardware
        let per_part_tex: Vec<Option<usize>> = mats
            .iter()
            .map(|mat| match mat {
                Material::Carcass => None,
                Material::Front => Some(front),
                Material::Edge => Some(edge),
                Material::Metal => Some(metal),
            })
            .collect();

        let at = self.factory.place_at();
        self.factory.place_furniture(idx, at);
        let fg_tex = self.factory.furniture_lib.get(idx).map(|a| {
            let g = a.group_geom();
            let ntri = a.positions.len() / 3;
            let mut map: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for t in 0..ntri {
                let part = a.part_ids.get(t).copied().unwrap_or(0) as usize;
                if let Some(Some(gtex)) = per_part_tex.get(part) {
                    map.insert(g.face[t], *gtex);
                }
            }
            map
        });
        if let (Some(fi), Some(fg_tex)) = (self.factory.sel_furn_primary(), fg_tex) {
            if let Some(inst) = self.factory.furniture.get_mut(fi) {
                inst.surface_texture = fg_tex;
            }
        }
        self.factory.open = true;
        self.factory.status = format!(
            "Cabinet unit built — {}×{} cells, {} door leaves, {} drawers, grip {} ({} tris)",
            m.cols,
            m.rows,
            m.door_leaves,
            m.drawer_fronts,
            inp.grip.label(),
            tris,
        );
        self.history.push(format!(
            "  furniture: cabinet unit ({}×{} cells, {} leaves, {} drawers, {} tris) and placed",
            m.cols, m.rows, m.door_leaves, m.drawer_fronts, tris,
        ));
        self.factory_op_evt(
            "cabin",
            "modal",
            format!(
                "cols={} rows={} leaves={} drawers={} grip={} tris={}",
                m.cols,
                m.rows,
                m.door_leaves,
                m.drawer_fronts,
                inp.grip.label(),
                tris
            ),
            features_before,
        );
    }

    /// Build the parametric SPIRAL (helical) stair as EDITABLE CSG solids: the pole and one-piece
    /// sleeve are cylinders, each tread and bracket an extruded sector prism, and every balustrade
    /// rail a run of oriented cylinder chords — all UNION'd into the model so each piece is
    /// selectable/movable and the whole stair accepts boolean cuts like any built solid. One undo
    /// step; selects the new solids. Mirrors [`Self::factory_build_dogleg`].
    fn factory_build_spiral_csg(&mut self, inp: &cad_solid::spiral::SpiralInput) {
        use cad_solid::spiral::{Role, SpiralPart};
        let (m, parts) = match cad_solid::spiral::build(inp) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Spiral stair: {e}");
                return;
            }
        };
        let features_before = self.factory.model.features.len();
        self.snapshot_factory();
        let colour = |role: Role| match role {
            Role::Pole => [0.42, 0.44, 0.48],      // dark metal
            Role::Sleeve => [0.50, 0.52, 0.55],    // metal
            Role::Tread => [0.72, 0.62, 0.46],     // wood
            Role::Bracket => [0.40, 0.41, 0.45],   // dark metal
            Role::Infill => [0.55, 0.56, 0.60],    // steel rail
            Role::Handrail => [0.62, 0.50, 0.38],  // wood handrail
            Role::Stanchion => [0.55, 0.56, 0.60], // steel
        };
        let mut ids = Vec::new();
        for part in parts {
            let role = part.role();
            let id = match part {
                SpiralPart::Cyl {
                    cx,
                    cy,
                    r,
                    z0,
                    z1,
                    sides,
                    ..
                } => {
                    let placement = cad_solid::Placement {
                        u: cx,
                        v: cy,
                        lift: z0,
                        spin_deg: 0.0,
                        pitch_deg: 0.0,
                        roll_deg: 0.0,
                    };
                    Some(self.factory.model.push(
                        cad_solid::BoolOp::Union,
                        cad_solid::Plane::default(),
                        placement,
                        cad_solid::Primitive::Cylinder {
                            r,
                            h: z1 - z0,
                            sides,
                        },
                    ))
                }
                SpiralPart::Prism { poly, z0, z1, .. } => {
                    let pts: Vec<glam::Vec2> =
                        poly.iter().map(|p| glam::Vec2::new(p[0], p[1])).collect();
                    match self.factory.model.add_profile(&pts) {
                        Ok((prof, centre, w, d)) => {
                            let placement = cad_solid::Placement {
                                u: centre.x,
                                v: centre.y,
                                lift: z0,
                                spin_deg: 0.0,
                                pitch_deg: 0.0,
                                roll_deg: 0.0,
                            };
                            Some(self.factory.model.push(
                                cad_solid::BoolOp::Union,
                                cad_solid::Plane::default(),
                                placement,
                                cad_solid::Primitive::Extrusion {
                                    profile: prof,
                                    h: z1 - z0,
                                    w,
                                    d,
                                },
                            ))
                        }
                        Err(_) => None,
                    }
                }
                SpiralPart::Seg { a, b, r, sides, .. } => {
                    // A helical-rail chord: an oriented cylinder from a→b (base circle ⟂ the chord).
                    let (a, b) = (glam::Vec3::from(a), glam::Vec3::from(b));
                    let dir = b - a;
                    let len = dir.length();
                    if len < 1e-4 {
                        None
                    } else {
                        let dir = dir / len;
                        let up = if dir.z.abs() > 0.9 {
                            glam::Vec3::X
                        } else {
                            glam::Vec3::Z
                        };
                        let u = up.cross(dir).normalize();
                        let v = dir.cross(u).normalize();
                        let placement = cad_solid::Placement {
                            u: 0.0,
                            v: 0.0,
                            lift: 0.0,
                            spin_deg: 0.0,
                            pitch_deg: 0.0,
                            roll_deg: 0.0,
                        };
                        Some(self.factory.model.push(
                            cad_solid::BoolOp::Union,
                            cad_solid::Plane::from_basis(a, u, v),
                            placement,
                            cad_solid::Primitive::Cylinder { r, h: len, sides },
                        ))
                    }
                }
            };
            if let Some(id) = id {
                self.factory.feature_color.insert(id, colour(role));
                ids.push(id);
            }
        }
        if ids.is_empty() {
            self.undo_stack.pop();
            self.factory.status = "Spiral stair produced no solids".into();
            return;
        }
        self.factory.sel_furniture.clear();
        self.factory.selection = ids.clone();
        self.factory.recompute();
        self.factory.open = true;
        self.factory.status = format!(
            "Spiral stair built — {} editable solids · {:.2} turns · top tread {:.2} m (cut/union as normal)",
            ids.len(), inp.turns, m.total_rise,
        );
        self.history.push(format!(
            "  architecture: spiral stair ({} solids, {} steps)",
            ids.len(),
            m.n_steps
        ));
        self.factory_op_evt(
            "spiral-stair-csg",
            "modal",
            format!(
                "solids={} steps={} turns={:.2} rise={:.2}",
                ids.len(),
                m.n_steps,
                inp.turns,
                inp.total_height
            ),
            features_before,
        );
    }

    /// Build the parametric half-turn (dog-leg) stair as EDITABLE CSG solids: each flight body is an
    /// extruded sawtooth prism, the landing and stone treads are boxes, all UNION'd into the model —
    /// so every piece is selectable/movable in the properties panel and the whole stair accepts
    /// boolean cuts/unions like any built solid. One undo step; selects the new solids.
    fn factory_build_dogleg(&mut self, inp: &cad_solid::dogleg::DoglegInput, with_treads: bool) {
        use cad_solid::dogleg::{DoglegPart, Role};
        let (m, parts) = match cad_solid::dogleg::build(inp, with_treads) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Dog-leg stair: {e}");
                return;
            }
        };
        let features_before = self.factory.model.features.len();
        self.snapshot_factory();
        let colour = |role: Role| match role {
            Role::Tread => [0.72, 0.68, 0.60],      // stone
            Role::Landing => [0.60, 0.60, 0.62],    // concrete
            Role::FlightBody => [0.55, 0.55, 0.57], // concrete
        };
        let mut ids = Vec::new();
        for part in parts {
            let role = part.role();
            let id = match part {
                DoglegPart::Box { min, max, .. } => {
                    let (w, d, h) = (max[0] - min[0], max[1] - min[1], max[2] - min[2]);
                    let placement = cad_solid::Placement {
                        u: (min[0] + max[0]) * 0.5,
                        v: (min[1] + max[1]) * 0.5,
                        lift: min[2],
                        spin_deg: 0.0,
                        pitch_deg: 0.0,
                        roll_deg: 0.0,
                    };
                    Some(self.factory.model.push(
                        cad_solid::BoolOp::Union,
                        cad_solid::Plane::default(),
                        placement,
                        cad_solid::Primitive::Box { w, d, h },
                    ))
                }
                DoglegPart::Flight {
                    profile, y0, y1, ..
                } => {
                    // Sawtooth profile lives in the world X-Z plane; extrude across Y ∈ [y0, y1].
                    let pts: Vec<glam::Vec2> = profile
                        .iter()
                        .map(|p| glam::Vec2::new(p[0], p[1]))
                        .collect();
                    match self.factory.model.add_profile(&pts) {
                        Ok((prof, centre, w, d)) => {
                            let plane = cad_solid::Plane::from_basis(
                                glam::Vec3::new(0.0, y1, 0.0),
                                glam::Vec3::X,
                                glam::Vec3::Z,
                            );
                            let placement = cad_solid::Placement {
                                u: centre.x,
                                v: centre.y,
                                lift: 0.0,
                                spin_deg: 0.0,
                                pitch_deg: 0.0,
                                roll_deg: 0.0,
                            };
                            Some(self.factory.model.push(
                                cad_solid::BoolOp::Union,
                                plane,
                                placement,
                                cad_solid::Primitive::Extrusion {
                                    profile: prof,
                                    h: y1 - y0,
                                    w,
                                    d,
                                },
                            ))
                        }
                        Err(_) => None,
                    }
                }
            };
            if let Some(id) = id {
                self.factory.feature_color.insert(id, colour(role));
                ids.push(id);
            }
        }
        if ids.is_empty() {
            self.undo_stack.pop();
            self.factory.status = "Dog-leg stair produced no solids".into();
            return;
        }
        self.factory.sel_furniture.clear();
        self.factory.selection = ids.clone();
        self.factory.recompute();
        self.factory.open = true;
        self.factory.status = format!(
            "Dog-leg stair built — {} editable solids · top tread {:.2} m (cut/union as normal)",
            ids.len(),
            m.top_tread_z,
        );
        self.history.push(format!(
            "  architecture: dog-leg stair ({} solids, {} steps)",
            ids.len(),
            m.n_steps
        ));
        self.factory_op_evt(
            "dogleg-stair",
            "modal",
            format!(
                "solids={} steps={} rise={:.2}",
                ids.len(),
                m.n_steps,
                inp.total_rise
            ),
            features_before,
        );
    }

    /// Convert a generated [`cad_solid::SolidMesh`] to furniture geometry and drop it into the
    /// scene (snapshotting for undo) — so a generated staircase behaves like any other placed
    /// object. Delegates the snapshot→add→place→status sequence to [`Self::factory_place_furniture_mesh`].
    fn arch_build_and_place(&mut self, mesh: cad_solid::SolidMesh, name: &str) {
        // Per-primitive part ids (each tread/riser/baluster) → carried onto the asset so "select a
        // piece" picks ONE part, not the whole welded run.
        let part_ids = mesh.face_ids.clone();
        let obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions,
            normals: mesh.normals,
            color: None, // neutral default; recolour via Textures like any furniture
            alpha: Vec::new(), // opaque
        };
        self.factory_place_furniture_mesh(obj, name, "generated", part_ids);
    }

    /// Snapshot for undo, add `mesh` to the library, and place it at the model centre — the shared
    /// tail for BOTH the procedural generator and the bundled stair-model importer. `verb` tags the
    /// status/history line ("generated" / "placed"). Marks `alpha_resolved` (opaque, no source to
    /// re-derive glass from) so the load-time refresh skips it. `part_ids` (per-triangle, or empty)
    /// tags each generated primitive as a distinct selectable piece.
    fn factory_place_furniture_mesh(
        &mut self,
        mesh: crate::mesh_io::ObjMesh,
        name: &str,
        verb: &str,
        part_ids: Vec<u32>,
    ) {
        let tris = mesh.tri_count();
        if tris == 0 {
            self.factory.status = format!("{name}: no geometry");
            return;
        }
        self.snapshot_factory();
        let idx = self.factory.add_furniture_asset(name.to_string(), mesh);
        if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
            a.alpha_resolved = true;
            if part_ids.len() == a.positions.len() / 3 {
                a.part_ids = part_ids; // one id per TRIANGLE
            }
        }
        let at = self.factory.place_at();
        self.factory.place_furniture(idx, at);
        self.factory.open = true;
        self.factory.status = format!("{name} {verb} ({tris} tris) — placed at model centre");
        self.history.push(format!(
            "  architecture: {name} {verb} ({tris} tris) and placed"
        ));
    }

    /// Public entry: ARM a deferred open. The loading overlay paints for one frame, then the
    /// actual read/parse runs on a BACKGROUND worker thread ([`load_file_worker`]) so the UI
    /// never freezes; [`Self::apply_loaded`] installs the result back on the main thread.
    pub(super) fn do_open(&mut self, path: &str) {
        let est = self.last_load_ms.max(300);
        self.busy = Some(BusyOp {
            kind: BusyKind::Load,
            path: path.to_string(),
            subtitle: file_stem_of(path),
            started: std::time::Instant::now(),
            est_ms: est,
            painted: false,
            rx: None,
        });
    }

    #[allow(dead_code)] // retained for reference / tests; the live path is the threaded worker.
    fn do_open_now(&mut self, path: &str) {
        let lower = path.to_ascii_lowercase();
        // Whole-open timer + a "begin" marker so the session recorder timeline starts
        // the instant the file is chosen. Every stage below is timed separately, so a
        // slow open pins the blame on ONE stage (read / parse / install / fit / sidecar)
        // instead of showing one opaque gap. All of this is a no-op unless recording.
        let t_open = std::time::Instant::now();
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::ImportStage {
                stage: "begin".into(),
                dobjects: 0,
                elapsed_us: 0,
                detail: path.to_string(),
            }
        );

        // ---- Stage 1+2: read bytes/text from disk, then PARSE into a Document. -------
        let doc_result = if lower.ends_with(".dxf") {
            let t = std::time::Instant::now();
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(e) => {
                    self.stage_evt("read file", 0, t.elapsed(), &format!("READ ERROR: {e}"));
                    self.history
                        .push(format!("  ! open '{}' failed: {}", path, e));
                    return;
                }
            };
            self.stage_evt(
                "read file",
                0,
                t.elapsed(),
                &format!("{} bytes", text.len()),
            );
            let t = std::time::Instant::now();
            let r = cad_io::dxf::read_dxf(&text);
            let n = r.as_ref().map(|d| d.dobjects.len()).unwrap_or(0);
            self.stage_evt(
                "parse dxf",
                n,
                t.elapsed(),
                if r.is_ok() { "" } else { "PARSE ERROR" },
            );
            r
        } else if lower.ends_with(".rsm") {
            let t = std::time::Instant::now();
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    self.stage_evt("read file", 0, t.elapsed(), &format!("READ ERROR: {e}"));
                    self.history
                        .push(format!("  ! open '{}' failed: {}", path, e));
                    return;
                }
            };
            self.stage_evt(
                "read file",
                0,
                t.elapsed(),
                &format!("{} bytes", bytes.len()),
            );
            let t = std::time::Instant::now();
            let r = cad_io::rsm::read_rsm(&bytes);
            let n = r.as_ref().map(|d| d.dobjects.len()).unwrap_or(0);
            self.stage_evt(
                "parse rsm",
                n,
                t.elapsed(),
                if r.is_ok() { "" } else { "PARSE ERROR" },
            );
            r
        } else if lower.ends_with(".dwg") {
            // DWG isn't read natively — convert to DXF via the external
            // converter (ACadSharp), then parse the DXF.
            let t = std::time::Instant::now();
            let conv = self.convert_dwg_to_dxf(path);
            self.stage_evt(
                "dwg→dxf convert",
                0,
                t.elapsed(),
                if conv.is_ok() {
                    "external converter"
                } else {
                    "CONVERT FAILED"
                },
            );
            match conv {
                Ok(dxf) => {
                    let t = std::time::Instant::now();
                    let text = match std::fs::read_to_string(&dxf) {
                        Ok(text) => text,
                        Err(e) => {
                            self.stage_evt(
                                "read file",
                                0,
                                t.elapsed(),
                                &format!("READ ERROR: {e}"),
                            );
                            self.history
                                .push(format!("  ! open '{}' failed: {}", path, e));
                            return;
                        }
                    };
                    self.stage_evt(
                        "read file",
                        0,
                        t.elapsed(),
                        &format!("{} bytes", text.len()),
                    );
                    let t = std::time::Instant::now();
                    let r = cad_io::dxf::read_dxf(&text);
                    let n = r.as_ref().map(|d| d.dobjects.len()).unwrap_or(0);
                    self.stage_evt(
                        "parse dxf",
                        n,
                        t.elapsed(),
                        if r.is_ok() { "" } else { "PARSE ERROR" },
                    );
                    r
                }
                Err(e) => {
                    self.history
                        .push(format!("  ! DWG convert '{}': {}", path, e));
                    return;
                }
            }
        } else {
            Err(format!(
                "unknown extension on '{}': expected .dxf, .dwg or .rsm",
                path
            ))
        };
        match doc_result {
            Ok(doc) => {
                let n = doc.dobjects.len();
                let l = doc.layers.len();
                // ---- Stage 3: install the parsed doc + invalidate caches. -----------
                let t = std::time::Instant::now();
                self.doc = doc;
                self.selection.clear();
                self.selection_prev.clear();
                self.selected = None;
                self.intersections.clear();
                self.index_dirty = true;
                // Tabs: open on MODEL. The file's own `active_layout` may point
                // at a saved layout; the tab strip honours it once the user
                // clicks through (Model-first matches the source repo).
                self.doc.active_layout = None;
                self.layout_selection.clear();
                self.layout_move_last = None;
                self.layout_grip_drag = None;
                self.touch_view();
                self.stage_evt("install doc", n, t.elapsed(), &format!("{l} layer(s)"));
                // ---- Stage 4: frame the drawing (bbox sweep). ----------------------
                let t = std::time::Instant::now();
                self.fit_view_to_drawing(); // jump straight to the drawing — no manual ZOOM
                self.stage_evt("fit view", n, t.elapsed(), "");
                self.current_file = Some(std::path::PathBuf::from(path));
                // ---- Stage 5: SIMLUX sidecar (may rebuild the 3D model). -----------
                let t = std::time::Instant::now();
                self.load_simlux_sidecar(std::path::Path::new(path));
                self.stage_evt("sidecar", n, t.elapsed(), "");
                self.history.push(format!(
                    "  opened '{}'  ({} dobject(s), {} layer(s))",
                    path, n, l
                ));
                self.stage_evt(
                    "TOTAL do_open",
                    n,
                    t_open.elapsed(),
                    "index+GPU+render still deferred — see later IndexRebuild / SlowFrame",
                );
            }
            Err(e) => {
                self.history.push(format!("  ! open '{}': {}", path, e));
                self.stage_evt("TOTAL do_open", 0, t_open.elapsed(), "FAILED");
            }
        }
    }

    /// Emit one `ImportStage` recorder event (no-op unless recording). Centralised so
    /// every `do_open` stage records identically — see [`Self::do_open`].
    #[track_caller]
    fn stage_evt(&mut self, stage: &str, dobjects: usize, dur: std::time::Duration, detail: &str) {
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::ImportStage {
                stage: stage.to_string(),
                dobjects,
                elapsed_us: dur.as_micros() as u64,
                detail: detail.to_string(),
            }
        );
    }

    /// Move the current selection onto a dedicated **SIMLUX** layer (created if
    /// absent) and mark that layer for 3D — the "shift the room into a SIMLUX
    /// layer" step. Undoable.
    pub(super) fn shift_selection_to_simlux_layer(&mut self) {
        // REFUSES inside a sketch rather than reaching for the plan, and for the same reason as
        // the Make-* tools: it needs `&mut` on the document AND `self.selection`, which indexes
        // whichever document is installed. There is no correct answer — plan indices would name
        // the wrong objects and sketch indices would move a drawing on a face onto a room layer.
        if self.factory.session.is_some() {
            self.history.push(
                "  ! SIMLUX: finish the sketch first — this moves objects in the PLAN".into(),
            );
            self.factory.status = "finish the sketch first — SIMLUX layers are the plan's".into();
            return;
        }
        if self.selection.is_empty() {
            self.history
                .push("  ! SIMLUX: select geometry first, then Move".into());
            return;
        }
        self.snapshot_doc();
        let lid = match self.doc.layers.find("SIMLUX") {
            Some(id) => id,
            None => self.doc.layers.add(Layer {
                name: "SIMLUX".into(),
                ..Layer::layer_zero()
            }),
        };
        let mut n = 0;
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                d.style.layer = lid;
                n += 1;
            }
        }
        self.factory.import_layer_as_room(&self.doc, lid); // use it for 3D straight away
        self.index_dirty = true;
        self.touch_view();
        self.history.push(format!(
            "  moved {n} object(s) to layer 'SIMLUX' (now used for 3D)"
        ));
    }

    /// Build the SIMLUX config from live app state, WITHOUT writing it. Pure `&self` so the
    /// heavy `serde`/IO can then run on a worker thread against the returned owned value.
    /// Full furniture geometry is encoded inline — used only by the (dead) synchronous save path.
    #[allow(dead_code)]
    pub(super) fn build_simlux_config(&self) -> crate::simlux_io::SimluxConfig {
        let mut cfg = self.build_simlux_config_common();
        cfg.factory = self.factory.to_persist();
        self.patch_open_sketch_into(&mut cfg);
        cfg
    }

    /// Put the OPEN sketch's live drawing into the record about to be written.
    ///
    /// Entering a sketch MOVES its document into `self.doc` for editing and leaves an empty shell
    /// behind in `model.sketches` (see `factory_enter_sketch`). Saving while a face is open would
    /// otherwise persist that empty shell — the drawing on screen would be the one thing missing
    /// from the file. Same reasoning as `plan_doc`, which already picks the parked model-space
    /// document for the main save.
    fn patch_open_sketch_into(&self, cfg: &mut crate::simlux_io::SimluxConfig) {
        let Some(id) = self.factory.session.as_ref().map(|s| s.plane) else {
            return;
        };
        // `cfg.factory.sketches` is written straight out of `model.sketches` by `to_persist`, in
        // the same order, so the plane's row in the model is its row in the record.
        let Some(i) = self.factory.model.sketch_index(id) else {
            return;
        };
        let Some(rec) = cfg.factory.sketches.get_mut(i) else {
            return;
        };
        use base64::Engine;
        rec.rsm_b64 =
            base64::engine::general_purpose::STANDARD.encode(cad_io::rsm::write_rsm(&self.doc));
    }

    /// Fold LEGACY room storage (pre-unification files) into the ONE room list.
    ///
    /// - `cfg.plan_rooms` → [`crate::factory::RoomOrigin::PlanDesignated`] rooms
    ///   (footprints were captured in metres at designation).
    /// - `cfg.layers_3d` (name → height) → [`crate::factory::RoomOrigin::ImportedLayer`]
    ///   rooms, resolved against the current document's layers.
    ///
    /// Runs only when a loaded config actually carries legacy rooms; a file
    /// saved since the unification has none and nothing moves.
    fn migrate_legacy_rooms(
        &mut self,
        plan: Vec<crate::simlux_io::PlanRoomRec>,
        layers: std::collections::BTreeMap<String, f32>,
    ) {
        let mut n_plan = 0;
        for r in &plan {
            if r.footprint.len() < 3 {
                continue;
            }
            let fp: Vec<glam::Vec2> = r
                .footprint
                .iter()
                .map(|p| glam::Vec2::new(p[0], p[1]))
                .collect();
            self.factory.add_designated_room(&r.name, &fp);
            n_plan += 1;
        }
        let mut n_layers = 0;
        for (name, height) in &layers {
            let Some(lid) = self.doc.layers.find(name) else {
                continue;
            };
            self.factory.import_layer_as_room(&self.doc, lid);
            // The import uses the project default height; restore the one the
            // old file carried.
            if let Some(i) = self.factory.room_index_of_layer(name) {
                self.factory.rooms[i].height = (*height).max(0.05);
            }
            n_layers += 1;
        }
        if n_plan > 0 || n_layers > 0 {
            self.history.push(format!(
                "  rooms: migrated {} plan designation(s) + {} imported layer(s) into one list",
                n_plan, n_layers
            ));
            self.light.last_msg = "Legacy rooms migrated into the single room list.".into();
        }
    }

    /// Config with furniture geometry LEFT OUT (empty blobs) — pair with
    /// [`crate::factory::FactoryState::furniture_geom_flat`] so a save worker does the deflate.
    pub(super) fn build_simlux_config_lite(&self) -> crate::simlux_io::SimluxConfig {
        let mut cfg = self.build_simlux_config_common();
        cfg.factory = self.factory.to_persist_lite();
        self.patch_open_sketch_into(&mut cfg);
        cfg
    }

    /// Everything in the SIMLUX config EXCEPT the 3D-factory block (which the two callers above
    /// fill in with vs. without furniture geometry).
    pub(super) fn build_simlux_config_common(&self) -> crate::simlux_io::SimluxConfig {
        // Resolve against the PLAN: a live face-sketch has its own layers/styles tables, and
        // keying the sidecar off those would persist the sketch's names, not the drawing's.
        let plan = self.plan_doc();
        let mut cfg = self.light.to_config(plan);
        // App-layer wall centerline linetypes, keyed by STABLE names (style → linetype).
        cfg.wall_centerline = self
            .wall_centerline_ltype
            .iter()
            .filter_map(|(&sid, &lid)| {
                let sname = plan.wall_styles.styles.get(sid as usize)?.name.clone();
                let lname = plan.linetypes.linetypes.get(lid as usize)?.name.clone();
                Some((sname, lname))
            })
            .collect();
        // Command-line calculator variables (minus `ans` — a session value).
        cfg.vars = self.calc.persist_map();
        cfg
    }

    /// Write the SIMLUX sidecar (`<drawing>.simlux.json`) next to `drawing`. Synchronous —
    /// used only by the (dead) non-threaded save path; the live path builds the config with
    /// [`Self::build_simlux_config`] and writes it on a worker.
    #[allow(dead_code)]
    fn write_simlux_sidecar(&mut self, drawing: &std::path::Path) {
        let cfg = self.build_simlux_config();
        let solids = cfg.factory.model.features.len();
        match crate::simlux_io::save(drawing, &cfg) {
            Ok(p) => {
                self.history
                    .push(format!("  SIMLUX setup → '{}'", p.display()));
                if solids > 0 {
                    self.history.push(format!(
                        "  3D Factory: {} solid(s), {} wall(s) saved",
                        solids,
                        cfg.factory.walls.len()
                    ));
                }
            }
            Err(e) => self.history.push(format!("  ! SIMLUX save: {}", e)),
        }
    }

    /// Load the SIMLUX sidecar for `drawing` (if present) onto the current doc. Synchronous —
    /// used only by the (dead) non-threaded open path; the live path parses the config on a
    /// worker and installs it with [`Self::install_simlux_config`].
    #[allow(dead_code)]
    fn load_simlux_sidecar(&mut self, drawing: &std::path::Path) {
        match crate::simlux_io::load(drawing) {
            Ok(Some(mut cfg)) => {
                let furniture = crate::factory::FactoryState::decode_furniture_lib(std::mem::take(
                    &mut cfg.factory.furniture_lib,
                ));
                self.install_simlux_config(cfg, furniture, None);
            }
            Ok(None) => {}
            Err(e) => self.history.push(format!("  ! SIMLUX load: {}", e)),
        }
    }

    /// Install an already-parsed SIMLUX config (with its furniture library ALREADY decoded) onto
    /// the current doc + 3D factory. Runs on the MAIN thread (recompute, fit, GL-adjacent state)
    /// but does NOT touch furniture geometry, so it stays fast even for a huge asset.
    pub(super) fn install_simlux_config(
        &mut self,
        mut cfg: crate::simlux_io::SimluxConfig,
        furniture: Vec<crate::factory::FurnitureAsset>,
        textures: Option<Vec<crate::factory::TextureAsset>>,
    ) {
        {
            // Command-line calculator variables, BEFORE `cfg` is consumed
            // below. Loading REPLACES the session store — the drawing's
            // variables are the drawing's.
            self.calc = crate::calc::CalcStore::from_persist(std::mem::take(&mut cfg.vars));
            // Resolve wall centerline linetypes by NAME → current ids (positional).
            self.wall_centerline_ltype.clear();
            for (sname, lname) in &cfg.wall_centerline {
                if let (Some(sid), Some(lid)) = (
                    self.doc.wall_styles.find(sname),
                    self.doc.linetypes.find(lname),
                ) {
                    self.wall_centerline_ltype.insert(sid, lid);
                }
            }
            // Restore the 3D model BEFORE `apply_config` consumes `cfg`. Furniture geometry
            // was already decoded (off-thread on the live path), so this is cheap.
            let fac = cfg.factory.clone();
            let had_solids = !fac.is_empty() || !furniture.is_empty();
            let n_solids = fac.model.features.len();
            let dropped = self
                .factory
                .apply_persist_prebuilt(fac, furniture, textures);
            self.refresh_aperture_transparency();
            // LEGACY ROOM MIGRATION — files written before the ONE-room-list
            // unification carried rooms in two more places: `cfg.plan_rooms`
            // (plan designations) and `cfg.layers_3d` (per-layer imports).
            // Both fold into `factory.rooms` with their provenance; saving
            // then writes the unified format only. Cloned before
            // `apply_config` consumes the config.
            let legacy_plan = cfg.plan_rooms.clone();
            let legacy_layers = cfg.layers_3d.clone();
            self.light.apply_config(cfg, &self.doc);
            if !legacy_plan.is_empty() || !legacy_layers.is_empty() {
                self.migrate_legacy_rooms(legacy_plan, legacy_layers);
            }
            self.history.push("  SIMLUX setup loaded".into());
            if had_solids {
                // Show the building rather than leaving it invisible behind a closed
                // panel — a restored model the user cannot see reads as "not loaded".
                // This is the one place we pay `recompute()` on open; it is gated on
                // the file actually containing solids.
                self.factory.open = true;
                self.factory.recompute();
                self.factory.fit();
                self.history.push(format!(
                    "  3D Factory: {} solid(s), {} wall(s) restored",
                    n_solids,
                    self.factory.walls.len()
                ));
            }
            if dropped > 0 {
                // Never silent — a wall that vanished must say so.
                self.history.push(format!(
                    "  ! 3D Factory: {} wall record(s) dropped (unusable footprint \
                         or missing feature)",
                    dropped
                ));
            }
        }
    }

    /// Public entry: ARM a deferred save. The "Saving…" overlay paints for one frame, then the
    /// serialize + sidecar-write run on a BACKGROUND worker thread ([`save_file_worker`]); the
    /// only main-thread cost is cloning the doc + building the config in [`Self::spawn_busy_worker`].
    /// Metres per drawing unit for the ACTIVE document — the factor every 2D→3D boundary
    /// multiplies by.
    ///
    /// Reads `self.doc`, not `plan_doc()`, and that is deliberate: a face sketch is its own
    /// Document with its own unit, and geometry drawn in a sketch is lifted through the
    /// sketch's frame, not the plan's. Defaults to 1.0 (`Assumed`) so nothing moves until a
    /// drawing's unit is actually set.
    #[inline]
    pub(super) fn doc_k(&self) -> f64 {
        self.doc.units.metres_per_unit
    }

    /// A drawing-unit length → metres, for the 3D side.
    #[inline]
    pub(super) fn dlen_m(&self, v: f64) -> f32 {
        (v * self.doc_k()) as f32
    }

    /// Flatten a geom straight into METRES. The 2D→3D flattening entry point: prefer this over
    /// `cad_solid::geom_outlines` anywhere the result feeds the Factory.
    #[inline]
    pub(super) fn outlines_m(&self, g: &Geom) -> Vec<Vec<glam::Vec2>> {
        cad_solid::geom_outlines_scaled(g, self.doc_k())
    }

    /// The user's DRAWING document — never the face-sketch that temporarily owns `self.doc`.
    ///
    /// `factory_enter_sketch` swaps the app's document for the sketch's (that swap is the whole
    /// thesis of the fork — every 2D tool works on a plane unchanged). But `self.doc` is also
    /// what the persistence layer reads, and nothing in that layer asked which document it was
    /// holding. Drawing inside a sketch sets `unsaved`, `tick_autosave` has no session gate, and
    /// the worker cloned `self.doc` — so an autosave firing while a sketch was open wrote the
    /// SKETCH over the user's plan file. Every save/serialise site must go through here.
    pub(super) fn plan_doc(&self) -> &cad_kernel::Document {
        Self::plan_doc_of(self.factory.session.as_ref(), &self.doc)
    }

    /// The same choice, taken from the two FIELDS rather than from `&self`.
    ///
    /// `plan_doc(&self)` borrows the whole struct, so it cannot be handed to a method on another
    /// field — `self.dbg.take_snapshot(self.plan_doc(), …)` is a borrow error. Splitting it this
    /// way lets the compiler see that `dbg` and `doc`/`factory` are disjoint, which is what the
    /// recorder's snapshot sites need. One definition, so the two cannot drift.
    pub(super) fn plan_doc_of<'a>(
        session: Option<&'a crate::factory::SketchSession>,
        doc: &'a cad_kernel::Document,
    ) -> &'a cad_kernel::Document {
        match session {
            Some(s) => &s.saved_doc,
            None => doc,
        }
    }

    /// THE GRID SPACING, IN WHATEVER THE ACTIVE DOCUMENT MEASURES IN.
    ///
    /// `GrdSpc` is a length in the DRAWING's units — AutoCAD's GRIDUNIT, and 10 on a millimetre
    /// plan means 10 mm. A face sketch is a document of its OWN and its units are metres, so that
    /// same stored 10 became a TEN METRE grid: three orders of magnitude coarser than the wall
    /// being drawn on, which is no grid at all, and a snap rounding every click to the nearest ten
    /// metres.
    ///
    /// The ratio between the two documents is the whole of the fix, and it is deliberately NOT a
    /// test on the unit VALUE: a metre-declared plan and a face sketch are byte-identical
    /// `{1.0, Declared}`, so nothing about the number can tell them apart. With no sketch open the
    /// plan IS the active document, the ratio is exactly 1, and the behaviour is unchanged to the
    /// bit for every existing case.
    pub(super) fn grid_spacing(&self) -> f64 {
        let plan_k = Self::plan_doc_of(self.factory.session.as_ref(), &self.doc)
            .units
            .metres_per_unit;
        let active_k = self.doc.units.metres_per_unit;
        if !(plan_k.is_finite() && active_k.is_finite()) || active_k <= 0.0 {
            return self.env.GrdSpc;
        }
        self.env.GrdSpc * plan_k / active_k
    }
    pub(super) fn do_save(&mut self, path: &str) {
        // While the "which copy of the 3D project data?" question is up, the
        // factory is still EMPTY and its mode unset — saving now would write the
        // blank state over whichever copy the user is about to pick.
        if self.extra_choice.is_some() {
            self.history.push(
                "  ! save deferred — this drawing has two copies of its 3D project data; pick \
                 which one to load first"
                    .into(),
            );
            return;
        }
        let est = self.last_save_ms.max(200);
        self.busy = Some(BusyOp {
            kind: BusyKind::Save,
            path: path.to_string(),
            subtitle: file_stem_of(path),
            started: std::time::Instant::now(),
            est_ms: est,
            painted: false,
            rx: None,
        });
    }

    #[allow(dead_code)] // retained for reference; the live path is the threaded worker.
    fn do_save_now(&mut self, path: &str) {
        let lower = path.to_ascii_lowercase();
        let mut plan = self.plan_doc().clone();
        normalize_layers_for_save(&mut plan);
        let bytes: Vec<u8> = if lower.ends_with(".dxf") {
            cad_io::dxf::write_dxf(&plan).into_bytes()
        } else if lower.ends_with(".rsm") {
            cad_io::rsm::write_rsm(&plan)
        } else {
            self.history.push(format!(
                "  ! save '{}': unknown extension (expected .dxf, .dwg or .rsm)",
                path
            ));
            return;
        };
        match std::fs::write(path, &bytes) {
            Ok(()) => {
                self.current_file = Some(std::path::PathBuf::from(path));
                self.write_simlux_sidecar(std::path::Path::new(path));
                self.history
                    .push(format!("  saved '{}'  ({} bytes)", path, bytes.len()));
            }
            Err(e) => self.history.push(format!("  ! save '{}': {}", path, e)),
        }
    }

    /// Spawn the background worker for the currently-armed `busy` op and stash its receiver.
    /// Called one frame AFTER the overlay first painted, so the window is already on screen and
    /// stays responsive while the worker reads/parses/serializes. For a save, the doc clone +
    /// config build happen here on the main thread (fast) before the worker takes over.
    pub(super) fn spawn_busy_worker(&mut self) {
        let Some(b) = self.busy.as_ref() else { return };
        let kind = b.kind;
        let path = b.path.clone();
        // A modal save must never run beside the background after-calculation
        // save: both write the same drawing, and the older snapshot of the
        // results save could land last and silently revert newer edits. Wait a
        // frame (the state machine above calls this every frame until `rx` is
        // set), so the results worker drains first.
        if kind == BusyKind::Save && self.results_rx.is_some() {
            return;
        }
        let rx = match kind {
            BusyKind::Load => {
                let (tx, rx) = std::sync::mpsc::channel::<BusyMsg>();
                std::thread::spawn(move || {
                    let _ = tx.send(BusyMsg::Loaded(load_file_worker(&path)));
                });
                rx
            }
            // The modal Save and the silent autosave share one spawn helper.
            BusyKind::Save => self.spawn_save_thread(path),
        };
        if let Some(b) = self.busy.as_mut() {
            b.rx = Some(rx);
        }
    }

    /// Prepare the save on the MAIN thread (a doc clone + a config with empty furniture blobs +
    /// the raw flattened geometry — all fast memcpy) and hand it to a background worker that does
    /// the deflate + serialize + write. Returns the result channel. Shared by the modal Save, the
    /// silent autosave, and the after-a-calculation embed save.
    ///
    /// In EMBEDDED mode the current saved calculation is snapshotted here (main thread) and rides
    /// inside the file with the config; in Sidecar mode it is written out separately afterwards
    /// by `save_light_results`, exactly as it always was.
    fn spawn_save_thread(&self, path: String) -> std::sync::mpsc::Receiver<BusyMsg> {
        let (tx, rx) = std::sync::mpsc::channel::<BusyMsg>();
        // The PLAN, not whatever document a live face-sketch has parked in `self.doc`.
        let doc = self.plan_doc().clone();
        let cfg = self.build_simlux_config_lite();
        let geom = self.factory.furniture_geom_flat();
        let store = self.extra_store;
        let results = if crate::simlux_io::can_embed(store, std::path::Path::new(&path)) {
            self.current_stored_results()
        } else {
            None
        };
        std::thread::spawn(move || {
            let _ = tx.send(BusyMsg::Saved(save_file_worker(
                &path, doc, cfg, geom, store, results,
            )));
        });
        rx
    }

    /// Install a drawing parsed by [`load_file_worker`] onto the app — the MAIN-thread half of
    /// an open (state install, view fit, sidecar apply, GL invalidate). Mirrors the install
    /// stages of the old synchronous `do_open_now`.
    pub(super) fn apply_loaded(&mut self, path: &str, payload: Box<LoadPayload>) {
        let LoadPayload {
            doc,
            sidecar,
            furniture,
            embedded,
            embedded_furniture,
            sidecar_textures,
            embedded_textures,
            embedded_results,
            read_ms,
            parse_ms,
            sidecar_ms,
            furn_ms,
            dwg_note,
        } = *payload;
        let n = doc.dobjects.len();
        let l = doc.layers.len();
        // Commit any live face-sketch FIRST. `self.doc` is the sketch while a session is open,
        // and the session parks the outgoing drawing in `saved_doc`. Installing over the top
        // would mean the eventual `factory_exit_sketch` restores the OLD drawing over the one
        // just opened, and files the newly opened drawing away inside a sketch slot.
        self.factory_exit_sketch();
        // A NEW OPEN REPLACES THE DOCUMENT, so any unanswered "which copy of the 3D project
        // data?" question from the PREVIOUS file is void — keeping it would let an answer
        // install the old file's project and storage mode onto the one just opened (and would
        // refuse every save on the new file until answered).
        self.extra_choice = None;
        // Re-emit the worker's stage timings to the recorder so a slow open still pins blame.
        self.stage_evt(
            "read file",
            0,
            std::time::Duration::from_millis(read_ms),
            "worker",
        );
        self.stage_evt(
            "parse doc",
            n,
            std::time::Duration::from_millis(parse_ms),
            "worker",
        );
        // Install the parsed doc + invalidate caches.
        self.doc = doc;
        // DROP THE HISTORY. It belongs to the document being replaced, and every entry in it is
        // a full snapshot of a DIFFERENT drawing — including the empty one the app starts with.
        // Left in place, Ctrl+Z after opening a file walks backwards out of that file and into
        // the previous document; the app then holds a pristine default while `current_file`
        // still points at the real project, and the next save — or the 3-minute autosave, which
        // needs no user action at all — writes that emptiness over it.
        //
        // This destroyed a user's project: a 213 KB drawing and a 304 MB model replaced by a
        // default document with zero entities and `features: []`.
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.selection.clear();
        self.selection_prev.clear();
        self.selected = None;
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
        // Frame the drawing, record where the file came from.
        self.fit_view_to_drawing();
        self.current_file = Some(std::path::PathBuf::from(path));
        // Apply the SIMLUX extra data (may rebuild the 3D model — the one recompute() we pay on
        // open). Furniture was decoded on the worker, so this no longer blocks on a huge mesh.
        //
        // THREE sources can exist: embedded in the file, the `.simlux.json` beside it — and
        // BOTH at once (an older sidecar next to a file saved "inside itself"). Both is a real
        // question for the user, asked once; one is applied straight away.
        self.stage_evt(
            "sidecar",
            n,
            std::time::Duration::from_millis(sidecar_ms),
            "worker parse",
        );
        self.stage_evt(
            "asset decode (meshes + textures)",
            furniture.len()
                + embedded_furniture.len()
                + sidecar_textures.len()
                + embedded_textures.len(),
            std::time::Duration::from_millis(furn_ms),
            "worker",
        );
        if sidecar.is_some() && embedded.is_some() {
            // Hold both until the user says which one this file's project is. The drawing and
            // its unit are installed below; the 3D install happens on the answer.
            self.extra_choice = Some(Box::new(PendingExtraChoice {
                path: path.to_string(),
                sidecar_cfg: sidecar,
                sidecar_furn: furniture,
                sidecar_tex: sidecar_textures,
                embedded_cfg: embedded,
                embedded_furn: embedded_furniture,
                embedded_tex: embedded_textures,
                embedded_results,
            }));
            self.history.push(format!(
                "  ⚠ extra data: the drawing embeds a 3D project AND '{}' sits beside it — \
                 which one loads?",
                crate::simlux_io::sidecar_path(std::path::Path::new(path))
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "simlux.json".into()),
            ));
        } else if let Some(cfg) = sidecar {
            self.extra_store = crate::simlux_io::ExtraDataStore::Sidecar;
            self.install_simlux_config(cfg, furniture, Some(sidecar_textures));
            // AFTER the extra data, never before: the fingerprint a stored result is checked
            // against is mostly a hash of the 3D model, and until `install_simlux_config` has
            // rebuilt it this project looks like an empty one — every saved result would read
            // as belonging to a different building and be refused.
            self.restore_light_results(std::path::Path::new(path));
        } else if let Some(cfg) = embedded {
            self.extra_store = crate::simlux_io::ExtraDataStore::Embedded;
            self.install_simlux_config(cfg, embedded_furniture, Some(embedded_textures));
            // The file's own embedded results first; the sidecar result file as the fallback
            // for a drawing that was embedded after its last calculation.
            let stored =
                embedded_results.or_else(|| crate::light_store::load(std::path::Path::new(path)));
            self.restore_light_results_from(stored);
        } else {
            // No project anywhere (a plain drawing) — the historic behaviour: the next save
            // writes a fresh sidecar beside it.
            self.extra_store = crate::simlux_io::ExtraDataStore::Sidecar;
            self.restore_light_results(std::path::Path::new(path));
        }
        self.unsaved = false; // freshly opened → matches the file on disk
        self.history.push(format!(
            "  opened '{}'  ({} dobject(s), {} layer(s))",
            path, n, l
        ));
        // HOW the DWG was read, and what it left behind -- see `LoadPayload::dwg_note`.
        if let Some(note) = dwg_note {
            self.history.push(note);
        }
        // Say so when the FILE declared its unit. Nothing was rescaled — the declaration only
        // changes how the 3D side reads the drawing from here on — but a scale that changed
        // without the user asking is exactly the kind of silence this whole problem came from.
        if self.doc.units.source == cad_kernel::UnitSource::Declared {
            self.history.push(format!(
                "  units: the file declares 1 drawing unit = {} — 3D builds use it from now on \
                 (nothing existing was rescaled; `units` to change)",
                self.doc.units.label()
            ));
        } else {
            // The file declares NOTHING — the common case for DXF, since most exporters omit
            // `$INSUNITS`. Adopt the Factory's working unit rather than assuming metres.
            //
            // Assuming metres was never neutral, it was a guess, and on a drawing that is really
            // in millimetres it fails silently and enormously: a 4400-unit outline becomes a
            // 4400-METRE footprint, and extruded to a 3 m storey that draws as a flat sheet 4.4 km
            // across with nothing on screen to explain it. This was reported from a real import.
            //
            // Adopted into the DOCUMENT, not applied as a hidden fallback at the 2D→3D boundary.
            // That distinction matters: the drawing now genuinely declares a scale, so every path
            // that reads it agrees — promotion, dimensions, the save, the DXF export — instead of
            // one boundary quietly disagreeing with the rest. It is also visible and reversible,
            // through the same `units` command that has always owned this, and it moves nothing.
            //
            // Only on IMPORT. Geometry built in memory is in whatever units its author meant, and
            // has no file to have declared anything.
            let u = self.factory.units.clone();
            self.doc.units = cad_kernel::Units::from_metres_per_unit(
                u.metres_per_unit,
                cad_kernel::UnitSource::User,
            );
            self.history.push(format!(
                "  units: the file declares none — read as {} (the 3D Factory working unit). \
                 Nothing was rescaled. If this drawing is in something else, `units <unit>` now, \
                 before building.",
                u.label()
            ));
        }
        // AND WHETHER THAT UNIT — declared by the file or adopted just above — CAN POSSIBLY BE
        // RIGHT. AFTER both branches, deliberately: the adopted case is at least as likely to be
        // wrong as the declared one, and an earlier draft of this put the check between them,
        // which silently attached the `else` to the wrong `if` and let a declared unit be
        // overwritten. The suite caught it; the shape is worth keeping obvious.
        if let Some(w) = self.unit_sanity_note() {
            self.history.push(w);
        }
    }

    /// Finalize a save completed by [`save_file_worker`] — record the file + log line.
    /// Fittings standing so far outside the model that they cannot be part of the scheme.
    ///
    /// THE MARGIN IS DELIBERATELY GENEROUS. An external fitting on a façade, a bollard by the door,
    /// a floodlight aimed back at the building — all legitimate, all outside the walls. What is not
    /// legitimate is a fitting a kilometre away, and the gap between those two cases is enormous,
    /// so the threshold does not need to be clever: the model's own extent, plus the larger of ten
    /// metres and a quarter of its span.
    ///
    /// Returns ids rather than indices — the caller removes them, and indices shift as it does.
    pub(super) fn stray_light_ids(&self) -> Vec<u32> {
        let Some((mn, mx)) = self.factory.features_aabb() else {
            return Vec::new(); // no model to be outside OF
        };
        let span = (mx.x - mn.x).max(mx.y - mn.y);
        let pad = 10.0_f32.max(span * 0.25);
        self.light
            .luminaires
            .iter()
            .filter(|l| {
                let p = l.position;
                p.x < mn.x - pad || p.x > mx.x + pad || p.y < mn.y - pad || p.y > mx.y + pad
            })
            .map(|l| l.id)
            .collect()
    }

    /// DOES THE DECLARED UNIT MAKE THIS DRAWING A PLAUSIBLE BUILDING? `None` when it does.
    ///
    /// A drawing carries two claims about its scale — the numbers in it, and the unit it says those
    /// numbers are in — and nothing ever checked one against the other. The owner's gym plan
    /// declared MILLIMETRES and contained METRES: a 3.4-unit wall, which is an ordinary wall in
    /// metres and 3.4 mm in millimetres. Under that declaration the fittings synced to 1/1000 scale
    /// and landed 3.5 km from the building, the furniture outlines drew 3.5 million units off
    /// screen, and every calculation returned 0 lx everywhere. It cost a session to find, and the
    /// contradiction was sitting in the file the whole time.
    ///
    /// The test is deliberately loose. Buildings run from about a metre to a few kilometres across;
    /// outside that is not a marginal call, it is a unit that cannot be right. A tighter bound
    /// would fire on legitimate work — a single detail, a site plan — and a warning that cries wolf
    /// is one nobody reads, which is how this was missed in the first place.
    pub(super) fn unit_sanity_note(&self) -> Option<String> {
        const PLAUSIBLE_MIN_M: f64 = 1.0;
        const PLAUSIBLE_MAX_M: f64 = 5_000.0;
        const CANDIDATES: [(&str, f64); 5] = [
            ("mm", 0.001),
            ("cm", 0.01),
            ("m", 1.0),
            ("in", 0.0254),
            ("ft", 0.3048),
        ];

        let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
        let mut any = false;
        for d in &self.doc.dobjects {
            let (bmin, bmax) = d.geom.bbox();
            if !bmin.x.is_finite() || !bmax.x.is_finite() {
                continue;
            }
            lo = (lo.0.min(bmin.x), lo.1.min(bmin.y));
            hi = (hi.0.max(bmax.x), hi.1.max(bmax.y));
            any = true;
        }
        // The SPAN, not the coordinates. A survey plan sits kilometres from the origin and that
        // says nothing about how big the building drawn on it is.
        let span = if any {
            (hi.0 - lo.0).max(hi.1 - lo.1)
        } else {
            0.0
        };
        if !any || span <= 0.0 {
            return None; // an empty drawing makes no claim to contradict
        }
        let k = self.doc.units.metres_per_unit;
        let as_declared = span * k;
        if (PLAUSIBLE_MIN_M..=PLAUSIBLE_MAX_M).contains(&as_declared) {
            return None;
        }
        // NAME THE UNIT THAT WOULD WORK. "Your unit is wrong" leaves the reader to guess which of
        // six it should have been, and the arithmetic is the same one they would have to do.
        let fits: Vec<(&str, f64)> = CANDIDATES
            .iter()
            .copied()
            .filter(|(_, kk)| (PLAUSIBLE_MIN_M..=PLAUSIBLE_MAX_M).contains(&(span * kk)))
            .collect();
        let advice = match fits.as_slice() {
            [] => {
                "no standard unit makes this a building-sized drawing — check the geometry itself"
                    .to_string()
            }
            [(n, kk)] => format!("`units {n}` would make it {:.2} m across", span * kk),
            // AS COMMANDS, not as bare unit names. The reader has to type one of these, and
            // "try m or ft" leaves them to work out that `units` is the word in front of it.
            many => format!(
                "try {}",
                many.iter()
                    .map(|(n, kk)| format!("`units {n}` ({:.2} m)", span * kk))
                    .collect::<Vec<_>>()
                    .join(" or "),
            ),
        };
        Some(format!(
            "  ⚠ units: this drawing is {span:.2} units across, which at 1 unit = {} is {as_declared:.3} m \
             — not a building. {advice}. (Setting the unit never moves geometry.)",
            self.doc.units.label(),
        ))
    }

    /// Install whichever copy of the 3D project data the user chose. Runs on the
    /// main thread, exactly like the equivalent branch of [`Self::apply_loaded`].
    pub(super) fn choose_extra_data(&mut self, pick: ExtraPick) {
        let Some(p) = self.extra_choice.take() else {
            return;
        };
        let p = *p;
        let path = std::path::PathBuf::from(&p.path);
        match pick {
            ExtraPick::Embedded => {
                self.extra_store = crate::simlux_io::ExtraDataStore::Embedded;
                if let Some(cfg) = p.embedded_cfg {
                    self.install_simlux_config(cfg, p.embedded_furn, Some(p.embedded_tex));
                    // The file's own results first; the sidecar result file as a
                    // fallback (a project embedded after its last calculation).
                    let stored = p
                        .embedded_results
                        .or_else(|| crate::light_store::load(&path));
                    self.restore_light_results_from(stored);
                }
                self.history.push(format!(
                    "  extra data: loaded the copy inside '{}' — further saves keep it inside \
                     the file",
                    p.path
                ));
            }
            ExtraPick::Sidecar => {
                self.extra_store = crate::simlux_io::ExtraDataStore::Sidecar;
                if let Some(cfg) = p.sidecar_cfg {
                    self.install_simlux_config(cfg, p.sidecar_furn, Some(p.sidecar_tex));
                    self.restore_light_results(&path);
                }
                self.history.push(format!(
                    "  extra data: loaded the .simlux.json copy beside '{}' — further saves keep \
                     using separate files",
                    p.path
                ));
            }
            ExtraPick::Neither => {
                self.extra_store = crate::simlux_io::ExtraDataStore::Sidecar;
                self.history.push(format!(
                    "  extra data: nothing loaded — drawing '{}' opened without its 3D project",
                    p.path
                ));
            }
        }
        self.touch_view();
    }

    /// A drawing that carries BOTH an embedded 3D project and a `.simlux.json`
    /// beside it — show the choice once, after the open; the drawing itself is
    /// already on screen. Nothing can proceed on the 3D side until it is answered
    /// (the model the fixtures and results belong to is whichever copy is chosen),
    /// so the dialog stays until one of the three options is clicked.
    pub(super) fn render_extra_choice_dialog(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.extra_choice else { return };
        let mut pick: Option<ExtraPick> = None;
        egui::Window::new("Which 3D project data should load?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.label(
                    "This drawing has TWO copies of its 3D project data (the model, furniture, \
                     materials and lighting):",
                );
                ui.add_space(6.0);
                for (label, cfg) in [
                    (
                        "Inside the drawing file".to_string(),
                        p.embedded_cfg.as_ref(),
                    ),
                    (
                        format!(
                            "Beside it — {}",
                            crate::simlux_io::sidecar_path(std::path::Path::new(&p.path))
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "simlux.json".into())
                        ),
                        p.sidecar_cfg.as_ref(),
                    ),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(label).strong());
                        if let Some(c) = cfg {
                            ui.weak(crate::simlux_io::cfg_summary(c));
                        }
                    });
                }
                ui.add_space(6.0);
                ui.label(
                    "Which copy should this session work with? Whichever you choose, the next \
                     Save stores the project the way you chose — the other copy is removed \
                     then. Save As can switch the storage mode any time.",
                );
                ui.add_space(8.0);
                if ui
                    .add_sized(
                        [460.0, 26.0],
                        egui::Button::new("Load the copy inside the file"),
                    )
                    .clicked()
                {
                    pick = Some(ExtraPick::Embedded);
                }
                if ui
                    .add_sized(
                        [460.0, 26.0],
                        egui::Button::new("Load the .simlux.json copy"),
                    )
                    .clicked()
                {
                    pick = Some(ExtraPick::Sidecar);
                }
                if ui
                    .add_sized(
                        [460.0, 26.0],
                        egui::Button::new("Load neither — drawing only"),
                    )
                    .clicked()
                {
                    pick = Some(ExtraPick::Neither);
                }
            });
        if let Some(pick) = pick {
            self.choose_extra_data(pick);
        }
    }

    /// THE SAVE THAT DID NOT SAVE, in front of the user until they say they have seen it.
    ///
    /// Everything below already existed except this window: the rename is retried for ~1.5 s, the
    /// temp is deliberately kept because it holds work that exists nowhere else, and the error names
    /// it and says what to do. All of that went into the command history, which scrolls — so on the
    /// owner's project the live sidecar ended up two weeks older than the temp beside it, while the
    /// app went on reporting "saved".
    ///
    /// Modal, and it cannot be dismissed by clicking past it: the alternative is losing a day's
    /// work to a line nobody read.
    pub(super) fn render_save_failure(&mut self, ctx: &egui::Context) {
        let Some(msg) = self.save_failure.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new("⚠  The project was NOT saved")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_max_width(560.0);
                ui.label(
                    egui::RichText::new(
                        "The drawing was written. The 3D model, furniture, fittings and results \
                         were not.",
                    )
                    .strong(),
                );
                ui.add_space(6.0);
                // THE PATH IS THE POINT. The work is on disk under another name, and this window
                // exists so somebody knows that before the window closes.
                ui.label(msg);
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "The file is usually held open by a sync tool — Dropbox, OneDrive, \
                         Google Drive. Pause it, or save somewhere local, and save again.",
                    )
                    .weak(),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    // Only offered where there is somewhere to write to. A project with no path
                    // has never been saved and this window cannot arise for it.
                    if let Some(p) = self.current_file.clone() {
                        if ui.button("Save again").clicked() {
                            self.save_failure = None;
                            let embedded_ok = crate::simlux_io::can_embed(self.extra_store, &p);
                            if embedded_ok {
                                // The retry must keep the project inside the file —
                                // `do_save_now` writes the drawing only, sidecar-style,
                                // and would drop the very payload that failed.
                                self.do_save(&p.to_string_lossy());
                            } else {
                                self.do_save_now(&p.to_string_lossy());
                            }
                        }
                    }
                    if ui.button("I understand — leave it unsaved").clicked() {
                        self.save_failure = None;
                    }
                });
                ui.add_space(4.0);
                // The project stays marked unsaved either way, so the close guard still fires.
                ui.label(
                    egui::RichText::new("The project stays marked unsaved until it writes.").weak(),
                );
            });
        if !open {
            self.save_failure = None;
        }
        let _ = &mut open;
    }

    pub(super) fn apply_saved(&mut self, path: &str, payload: SavePayload) {
        self.current_file = Some(std::path::PathBuf::from(path));
        // A HALF-SAVE IS NOT A SAVE. `unsaved = false` was unconditional, so a project whose SIMLUX
        // half never reached disk was marked as matching it: the close guard stayed quiet, the
        // window shut, and the only copy of the 3D model, the furniture, the fittings and the
        // results was a `.savetmp` nobody knew to look for. On the owner's project the live
        // sidecar was two weeks older than the temp sitting beside it.
        //
        // The warning existed the whole time. It was a line in the command history, which scrolls.
        self.unsaved = payload.simlux_failed.is_some();
        self.last_autosave = std::time::Instant::now(); // an explicit save resets the autosave clock
        self.history.push(payload.note);
        // AND IN FRONT OF THE USER, because losing a day's work to a line that scrolled past is
        // what this costs. Dismissed by acknowledging it, not by the next repaint.
        if let Some(e) = payload.simlux_failed {
            self.save_failure = Some(e);
        }
        // A result computed before the drawing had a name had nowhere to go; this is where the
        // project gets one. Also what carries the result along on a Save As, so the copy opens
        // showing what the original showed instead of an empty panel.
        self.save_light_results();
        // If this save was requested to complete a close, close now that the bytes are on disk.
        if self.close_after_save {
            self.close_after_save = false;
            self.pending_close = true;
        }
    }

    /// AUTOSAVE tick — run once per frame. Silently re-saves the current file on a background
    /// worker (no modal overlay) a few minutes after an edit, so a crash can't lose an afternoon's
    /// work. It NEVER blocks the UI, only writes an already-saved `.dxf`/`.rsm`, and writes
    /// atomically (see [`atomic_write`]) so an interrupted autosave can't corrupt the file. The
    /// `unsaved` flag is cleared only if no edit happened WHILE the save ran (via `edit_seq`), so
    /// the close-confirm guard still fires for changes the autosave didn't capture.
    pub(super) fn tick_autosave(&mut self) {
        const AUTOSAVE_SECS: u64 = 180; // ~3 minutes after the first unsaved edit

        // 1) Poll a running autosave.
        if self.autosave_rx.is_some() {
            use std::sync::mpsc::TryRecvError;
            match self.autosave_rx.as_ref().unwrap().try_recv() {
                Ok(BusyMsg::Saved(res)) => {
                    self.autosave_rx = None;
                    match res {
                        Ok(p) => {
                            // Clear `unsaved` only if nothing was edited during the save.
                            if self.edit_seq == self.autosave_seq {
                                self.unsaved = false;
                            }
                            self.history
                                .push(format!("  ⤓ autosaved ({} bytes)", p.bytes));
                        }
                        Err(e) => self.history.push(format!("  ! autosave: {}", e)),
                    }
                }
                Ok(_) => self.autosave_rx = None, // a stray Loaded message (never expected)
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.autosave_rx = None,
            }
            return; // never run two at once
        }

        // 2) Should we start one? Only for an already-saved drawing with pending edits, and not
        //    while a modal load/save or another background save is in flight — and never while
        //    the open-time "which copy of the 3D project data?" question is unanswered (saving
        //    then would write the empty factory over the copies the user is about to choose
        //    between). Two writers on one path must not overlap: the after-calculation embed
        //    save holds an older snapshot, and if it landed after this one it would silently
        //    revert everything since.
        if !self.autosave_on
            || self.busy.is_some()
            || !self.unsaved
            || self.extra_choice.is_some()
            || self.results_rx.is_some()
        {
            return;
        }
        let Some(path) = self
            .current_file
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
        else {
            return;
        };
        let lower = path.to_ascii_lowercase();
        if !(lower.ends_with(".dxf") || lower.ends_with(".rsm") || lower.ends_with(".dwg")) {
            return; // never autosave an untitled or non-drawing file to a surprise location
        }
        if self.last_autosave.elapsed().as_secs() < AUTOSAVE_SECS {
            return;
        }
        // Fire a silent background save of the state as of NOW; `unsaved` stays true until it
        // completes (so a close mid-autosave is still guarded).
        self.autosave_seq = self.edit_seq;
        self.autosave_rx = Some(self.spawn_save_thread(path));
        self.last_autosave = std::time::Instant::now();
    }

    // ---- Groups ---------------------------------------------------------

    /// Handles of the currently selected dobjects.
    fn selected_handles(&self) -> std::collections::HashSet<u64> {
        self.selection
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).map(|d| d.handle))
            .collect()
    }

    /// Ctrl+G / `group` — make the current selection into one group.
    pub(super) fn group_selection(&mut self) {
        let members: Vec<u64> = self
            .selection
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).map(|d| d.handle))
            .collect();
        if members.len() < 2 {
            self.history
                .push("  ! group: select 2+ objects first".into());
            return;
        }
        // Re-grouping: drop any existing group that shares these members, so a
        // member never lives in two groups.
        let set: std::collections::HashSet<u64> = members.iter().copied().collect();
        self.groups.retain(|g| !g.iter().any(|h| set.contains(h)));
        let n = members.len();
        self.groups.push(members);
        self.history.push(format!("  ⊞ grouped {} objects", n));
    }

    /// Add to Group — when the selection includes an existing group plus other
    /// entities, merge those entities (and any other touched groups) into one
    /// group. Needs at least one already-grouped object in the selection.
    pub(super) fn add_to_group(&mut self) {
        let sel = self.selected_handles();
        if sel.is_empty() {
            self.history
                .push("  ! add to group: nothing selected".into());
            return;
        }
        let touched: Vec<usize> = self
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| g.iter().any(|h| sel.contains(h)))
            .map(|(i, _)| i)
            .collect();
        if touched.is_empty() {
            self.history
                .push("  ! add to group: include an existing group in the selection".into());
            return;
        }
        // Merge every touched group's members + all selected handles into one.
        let mut set: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut members: Vec<u64> = Vec::new();
        for &gi in &touched {
            for h in &self.groups[gi] {
                if set.insert(*h) {
                    members.push(*h);
                }
            }
        }
        for h in &sel {
            if set.insert(*h) {
                members.push(*h);
            }
        }
        // Drop the touched groups (descending), then push the merged group.
        let mut t = touched.clone();
        t.sort_unstable();
        for &gi in t.iter().rev() {
            self.groups.remove(gi);
        }
        let n = members.len();
        self.groups.push(members);
        self.history
            .push(format!("  ⊞ added to group ({} objects total)", n));
    }

    /// Edit ▸ Ungroup / `ungroup` — dissolve every group that any selected
    /// object belongs to (objects themselves stay).
    pub(super) fn ungroup_selection(&mut self) {
        let sel = self.selected_handles();
        if sel.is_empty() {
            self.history.push("  ! ungroup: nothing selected".into());
            return;
        }
        let before = self.groups.len();
        self.groups.retain(|g| !g.iter().any(|h| sel.contains(h)));
        let removed = before - self.groups.len();
        if removed == 0 {
            self.history
                .push("  ungroup: selection isn't in a group".into());
        } else {
            self.history
                .push(format!("  ⊟ ungrouped {} group(s)", removed));
        }
    }

    /// Grow the selection so that picking any group member selects the whole
    /// group (groups select as a unit). Called after pointer-mode picks.
    pub(super) fn expand_selection_to_groups(&mut self) {
        if self.groups.is_empty() || self.selection.is_empty() {
            return;
        }
        let sel = self.selected_handles();
        let mut want = sel.clone();
        for g in &self.groups {
            if g.iter().any(|h| sel.contains(h)) {
                for h in g {
                    want.insert(*h);
                }
            }
        }
        if want.len() == sel.len() {
            return;
        } // nothing new
        self.selection = self
            .doc
            .dobjects
            .iter()
            .enumerate()
            .filter(|(_, d)| want.contains(&d.handle))
            .map(|(i, _)| i)
            .collect();
    }

    /// Ctrl+C — copy the current selection into the dobject clipboard.
    pub(super) fn copy_selection(&mut self) {
        if self.selection.is_empty() {
            self.history.push("  copy: nothing selected".into());
            return;
        }
        self.clipboard_dobjects = self
            .selection
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).cloned())
            .collect();
        // WHAT THE NUMBERS MEANT. Geometry is a pile of coordinates, and a coordinate is only a
        // length alongside the unit it was measured in — see `clipboard_unit_m`.
        self.clipboard_unit_m = self.doc.units.metres_per_unit;
        self.history.push(format!(
            "  ⎘ copied {} dobject(s)",
            self.clipboard_dobjects.len()
        ));
    }

    /// The clipboard, expressed in the ACTIVE document's units.
    ///
    /// The IDENTITY when the two agree — which is every copy and paste inside one document, so
    /// the ordinary case is unchanged to the bit. Only crossing between a plan and a face sketch
    /// needs anything done, and that is exactly the crossing nothing used to notice.
    fn clipboard_in_active_units(&self) -> Vec<DObject> {
        let k = self.clipboard_unit_m / self.doc.units.metres_per_unit;
        if !(k.is_finite() && k > 0.0) || (k - 1.0).abs() < 1e-12 {
            return self.clipboard_dobjects.clone();
        }
        // About the ORIGIN, so this is a pure change of scale. Where the objects then land is the
        // paste's own business, and it works that out from the converted geometry.
        self.clipboard_dobjects
            .iter()
            .map(|d| DObject::with_style(d.geom.scaled(Vec2::new(0.0, 0.0), k), d.style))
            .collect()
    }

    /// Edit ▸ Paste (Ctrl+V) — start the placement flow. The base point is
    /// assumed automatically (the clipboard's lower-left corner), so the user
    /// only clicks a DESTINATION; a live ghost preview follows the cursor.
    pub(super) fn start_paste(&mut self) {
        if self.clipboard_dobjects.is_empty() {
            self.history.push("  paste: clipboard is empty".into());
            return;
        }
        // IN THE ACTIVE DOCUMENT'S UNITS, so the base point is a place in THIS drawing rather
        // than a number that meant something in another one.
        let clip = self.clipboard_in_active_units();
        let mut base = clip[0].bbox().0;
        for d in &clip[1..] {
            let a = d.bbox().0;
            if a.x < base.x {
                base.x = a.x;
            }
            if a.y < base.y {
                base.y = a.y;
            }
        }
        self.paste_state = PasteState::WaitingForDest(base);
        self.set_prompt(format!(
            "paste: click DESTINATION for {} dobject(s)   [Esc=cancel]",
            self.clipboard_dobjects.len()
        ));
        self.refocus_cmd = true;
    }

    /// Commit the paste: add the clipboard clones translated by `dest - base`
    /// (fresh handles), select them, and end the flow.
    pub(super) fn commit_paste(&mut self, base: Vec2, dest: Vec2) {
        let v = dest - base;
        self.snapshot_doc();
        let start = self.doc.dobjects.len();
        let clones: Vec<DObject> = self
            .clipboard_in_active_units()
            .iter()
            .map(|d| DObject::with_style(d.geom.translated(v), d.style))
            .collect();
        let n = clones.len();
        for nd in clones {
            self.doc.push(nd);
        }
        self.selection = (start..self.doc.dobjects.len()).collect();
        self.selected = None;
        self.paste_state = PasteState::Off;
        self.clear_prompt();
        self.index_dirty = true;
        self.touch_view();
        self.history
            .push(format!("  ⎗ pasted {} dobject(s) (selected)", n));
    }

    /// `Save` — write to the current file if known, else fall back to Save As.
    pub(super) fn do_save_current(&mut self) {
        if let Some(p) = self.current_file.clone() {
            self.do_save(&p.to_string_lossy());
        } else {
            // No current file yet — open Save As (default to native .rsm).
            self.open_file_dialog(FileDialogMode::Save, ".rsm");
        }
    }

    /// Open the in-app file browser. Starting directory = last-used, else
    /// the current working dir, else `/`.
    /// (Re)build the Open-dialog preview for `path` if the selection changed.
    /// Parses the .dxf/.rsm into a Document and computes its bbox + a one-line
    /// summary. No-op when `path` is already the cached preview.
    fn update_file_preview(&mut self, path: &std::path::Path) {
        if self.file_preview.as_ref().map(|p| p.path.as_path()) == Some(path) {
            return;
        }
        let lower = path.to_string_lossy().to_ascii_lowercase();
        let parsed: Result<Document, String> = if lower.ends_with(".dxf") {
            std::fs::read_to_string(path)
                .map_err(|e| e.to_string())
                .and_then(|t| cad_io::dxf::read_dxf(&t))
        } else if lower.ends_with(".rsm") {
            std::fs::read(path)
                .map_err(|e| e.to_string())
                .and_then(|b| cad_io::rsm::read_rsm(&b))
        } else {
            Err("unsupported file type".into())
        };
        self.file_preview = Some(match parsed {
            Ok(mut doc) => {
                // The preview only draws the 2D plan; drop any embedded extra-data
                // payload (which can be hundreds of MB) the moment it is parsed,
                // or the open dialog would hold a whole project in RAM per peek.
                doc.extra_blobs.clear();
                let mut bb: Option<(Vec2, Vec2)> = None;
                for d in &doc.dobjects {
                    let (a, b) = d.bbox();
                    bb = Some(match bb {
                        None => (a, b),
                        Some((mn, mx)) => (
                            Vec2::new(mn.x.min(a.x), mn.y.min(a.y)),
                            Vec2::new(mx.x.max(b.x), mx.y.max(b.y)),
                        ),
                    });
                }
                let info = format!(
                    "{} object(s) · {} layer(s)",
                    doc.dobjects.len(),
                    doc.layers.len()
                );
                FilePreview {
                    path: path.to_path_buf(),
                    doc: Some(doc),
                    bbox: bb,
                    info,
                }
            }
            Err(e) => FilePreview {
                path: path.to_path_buf(),
                doc: None,
                bbox: None,
                info: format!("cannot preview: {}", e),
            },
        });
    }

    /// Draw the cached preview into `prect` as a fit-to-rect wireframe. Reuses
    /// the real renderer by temporarily swapping in the preview Document + a
    /// fit transform (so layers/blocks resolve correctly), then restoring.
    fn render_file_preview(&mut self, painter: &egui::Painter, prect: egui::Rect) {
        painter.rect_filled(prect, 2.0, egui::Color32::from_rgb(24, 28, 34));
        painter.rect_stroke(
            prect,
            2.0,
            egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
        );
        let Some(mut fp) = self.file_preview.take() else {
            return;
        };
        if let (Some(doc), Some((mn, mx))) = (fp.doc.take(), fp.bbox) {
            let w = (mx.x - mn.x).abs();
            let h = (mx.y - mn.y).abs();
            let pad = 10.0_f64;
            let aw = (prect.width() as f64 - 2.0 * pad).max(1.0);
            let ah = (prect.height() as f64 - 2.0 * pad).max(1.0);
            let sx = if w > 1e-9 { aw / w } else { f64::INFINITY };
            let sy = if h > 1e-9 { ah / h } else { f64::INFINITY };
            let mut s = sx.min(sy);
            if !s.is_finite() {
                s = 1.0;
            }
            let center = (mn + mx) * 0.5;
            // Swap transform + doc; draw; restore.
            let (saved_scale, saved_off) = (self.scale, self.world_offset);
            self.scale = s as f32;
            self.world_offset = egui::vec2(-center.x as f32, -center.y as f32);
            let mut pdoc = doc;
            std::mem::swap(&mut self.doc, &mut pdoc); // self.doc = preview
            let cp = painter.with_clip_rect(prect);
            let col = egui::Color32::from_rgb(200, 210, 220);
            // Clone the geom list so we don't alias self while passing &self.
            let geoms: Vec<Geom> = self.doc.dobjects.iter().map(|d| d.geom.clone()).collect();
            for g in &geoms {
                draw_dobject(&cp, prect, self, g, col);
            }
            std::mem::swap(&mut self.doc, &mut pdoc); // restore real doc
            self.scale = saved_scale;
            self.world_offset = saved_off;
            fp.doc = Some(pdoc);
        } else {
            painter.text(
                prect.center(),
                egui::Align2::CENTER_CENTER,
                "no preview",
                crate::theme::typ::data_code(),
                egui::Color32::from_gray(140),
            );
        }
        self.file_preview = Some(fp);
    }

    /// Does the browser list the file `name` in `mode`, under the active filter `ext`?
    ///
    /// Split out of the directory walk so it can be tested — it is the whole of "why can I not see
    /// my files", and until now it could only be checked by opening the app and looking.
    ///
    /// `PickFolder` is the interesting one. It used to be `false` outright, "folders only", which
    /// is defensible when you are choosing where to WRITE — the Radiance output folder — and wrong
    /// when you are choosing a folder BECAUSE OF WHAT IS IN IT. The Light Editor's photometry
    /// folder is the second kind: reported as "when i try to import an ies/ldt file in the block to
    /// fitting tab its not detecting the ies/ldt files in the folder", because the browser showed a
    /// directory holding thirty .ldt files as completely empty. So a folder pick may now carry a
    /// filter of its own, and files matching it are listed as CONTEXT — see the render, where they
    /// are deliberately not clickable, since the answer is still the folder.
    pub(super) fn dialog_lists(mode: FileDialogMode, ext: &str, name: &str) -> bool {
        let lname = name.to_ascii_lowercase();
        let any = |exts: &[&str]| exts.iter().any(|x| lname.ends_with(x));
        match mode {
            // Open shows every readable drawing type; Save filters to the chosen output extension.
            FileDialogMode::Open => any(&[".dxf", ".rsm", ".dwg", ".pst"]),
            FileDialogMode::ImportImage
            | FileDialogMode::ImportRaster
            | FileDialogMode::ImportTexture => {
                any(&[".png", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff"])
            }
            // HDR environments. `.hdr` is Radiance RGBE, `.exr` OpenEXR — the two formats HDRI
            // libraries actually ship.
            FileDialogMode::ImportHdri => any(&[".hdr", ".exr"]),
            FileDialogMode::ImportObj => any(&[".obj", ".3ds", ".fbx", ".glb", ".gltf"]),
            // Photometric files. `.ies` is LM-63, `.ldt` EULUMDAT — between them, what every
            // manufacturer publishes.
            FileDialogMode::ImportIes => any(&[".ies", ".ldt"]),
            FileDialogMode::Save => !ext.is_empty() && lname.ends_with(ext),
            // An EMPTY filter still means folders only, which is right for an output folder: the
            // files already there are none of the caller's business.
            FileDialogMode::PickFolder => {
                !ext.is_empty() && ext.split('|').any(|x| !x.is_empty() && lname.ends_with(x))
            }
        }
    }

    /// What to call the folder browser, which is shared by three callers that want different
    /// folders for different reasons.
    ///
    /// It was titled "Choose output folder · Radiance render" for all three, so asking the Light
    /// Editor for a photometry folder opened a window announcing a render nobody had started.
    pub(super) fn folder_dialog_title(&self) -> &'static str {
        if self.report_wants_dir {
            "Choose where the report goes"
        } else if self.photometry_wants_folder {
            "Choose the folder your .ies / .ldt files are in"
        } else if self.texset_target.is_some() {
            "Choose a PBR texture-set folder"
        } else {
            "Choose output folder  ·  Radiance render"
        }
    }

    pub(super) fn open_file_dialog(&mut self, mode: FileDialogMode, ext: &str) {
        if mode == FileDialogMode::Save {
            self.save_dialog_purpose = 0; // WBLOCK re-sets it after opening
        }
        let dir = self
            .file_dialog_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let mut dlg = FileDialog::new(mode, dir, ext);
        // Save-As shows the extra-data choice pre-set to the current mode, so a
        // plain "Save As" in the same format changes nothing unless asked.
        if mode == FileDialogMode::Save {
            dlg.embed_extra = self.extra_store == crate::simlux_io::ExtraDataStore::Embedded;
        }
        self.file_dialog = Some(dlg);
    }

    /// Render the file browser window. Pure `std::fs` — lists sub-dirs and
    /// .dxf/.rsm files, supports navigation (`..`, click a folder), name
    /// entry, and a format toggle for Save. Confirm routes to do_open /
    /// do_save.
    pub(super) fn render_file_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.file_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut do_confirm = false;
        let mut do_cancel = false;
        let mut navigate: Option<std::path::PathBuf> = None;
        // When the path bar points straight at a file, navigate to its folder
        // AND preselect the file name.
        let mut pending_filename: Option<String> = None;

        // List the current directory (dirs + files matching the active type).
        let mut dirs: Vec<String> = Vec::new();
        let mut files: Vec<String> = Vec::new();
        match std::fs::read_dir(&dlg.dir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    } // hide dotfiles
                      // Hide Windows hidden/system entries ($RECYCLE.BIN, System
                      // Volume Information, …) so the list matches Explorer.
                    #[cfg(windows)]
                    {
                        use std::os::windows::fs::MetadataExt;
                        const HIDDEN: u32 = 0x2;
                        const SYSTEM: u32 = 0x4;
                        if let Ok(md) = e.metadata() {
                            if md.file_attributes() & (HIDDEN | SYSTEM) != 0 {
                                continue;
                            }
                        }
                    }
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let shows = Self::dialog_lists(dlg.mode, &dlg.ext, &name);
                    if is_dir {
                        dirs.push(name);
                    } else if shows {
                        files.push(name);
                    }
                }
                dirs.sort_unstable();
                files.sort_unstable();
            }
            Err(e) => dlg.error = Some(format!("cannot read directory: {}", e)),
        }

        let title = match dlg.mode {
            FileDialogMode::Open if self.plotstyle_pst_io == Some(false) => {
                "Load Plot Style Table  ·  .pst"
            }
            FileDialogMode::Save if self.plotstyle_pst_io == Some(true) => {
                "Save Plot Style Table  ·  .pst"
            }
            FileDialogMode::Open => "Open  .dxf / .rsm / .dwg",
            FileDialogMode::ImportImage => "Open Image  ·  raster → vector",
            FileDialogMode::ImportRaster => "Open Image  ·  raster underlay",
            FileDialogMode::ImportObj => "Import furniture  ·  OBJ / 3DS / FBX / glTF",
            FileDialogMode::ImportTexture => "Load texture  ·  PNG / JPG image",
            FileDialogMode::ImportHdri => "Load environment  ·  HDR / EXR",
            FileDialogMode::ImportIes => "Import light file  ·  IES / EULUMDAT",
            FileDialogMode::Save => "Save As",
            FileDialogMode::PickFolder => self.folder_dialog_title(),
        };
        // Cap the window to the screen so the bottom controls (Type / File /
        // Open / Cancel) are never pushed off-screen.
        let screen = ctx.screen_rect();
        let max_h = (screen.height() - 80.0).max(320.0);
        let max_w = (screen.width() - 80.0).max(420.0);
        egui::Window::new(title)
            .id(egui::Id::new("file_dialog"))
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_size(egui::vec2(680.0_f32.min(max_w), 480.0_f32.min(max_h)))
            .max_height(max_h)
            .max_width(max_w)
            .default_pos(egui::pos2(160.0, 60.0))
            .show(ctx, |ui| {
                // Layout = pinned TOP (path bar) + pinned BOTTOM (type/file/
                // buttons) + filling CENTER (list + preview). Panels keep the
                // footer always visible and give the center a BOUNDED height
                // that tracks the window's (user-resized) size — so the window
                // fits the screen and its height is freely adjustable.

                // ---- TOP: path bar ------------------------------------------
                egui::TopBottomPanel::top("file_dialog_header")
                    .resizable(false)
                    .show_inside(ui, |ui| {
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            // Drive picker — click a drive root to jump there.
                            let cur_drive: String = {
                                #[cfg(windows)]
                                {
                                    dlg.dir.to_string_lossy().chars().take(2).collect()
                                }
                                #[cfg(not(windows))]
                                {
                                    "/".to_string()
                                }
                            };
                            egui::ComboBox::from_id_source("file_dialog_drive")
                                .selected_text(cur_drive)
                                .width(52.0)
                                .show_ui(ui, |ui| {
                                    for root in list_drive_roots() {
                                        let label = root.to_string_lossy().to_string();
                                        if ui.selectable_label(false, &label).clicked() {
                                            navigate = Some(root);
                                        }
                                    }
                                });
                            ui.label("Path");
                            let go = ui.button("Go").clicked();
                            let resp = ui.add_sized(
                                [ui.available_width().max(80.0), ui.spacing().interact_size.y],
                                egui::TextEdit::singleline(&mut dlg.path_buf)
                                    .hint_text("type or paste a path, then Enter"),
                            );
                            let enter =
                                resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if enter || go {
                                let p = std::path::PathBuf::from(dlg.path_buf.trim());
                                if p.is_dir() {
                                    navigate = Some(p);
                                } else if p.is_file() {
                                    if let Some(parent) = p.parent() {
                                        navigate = Some(parent.to_path_buf());
                                    }
                                    pending_filename =
                                        p.file_name().map(|n| n.to_string_lossy().to_string());
                                } else {
                                    dlg.error = Some(format!("not found: {}", dlg.path_buf.trim()));
                                }
                            }
                        });
                        ui.add_space(3.0);
                    });

                // ---- BOTTOM: type/format + filename + action buttons --------
                egui::TopBottomPanel::bottom("file_dialog_footer")
                    .resizable(false)
                    .show_inside(ui, |ui| {
                        ui.add_space(3.0);
                        if dlg.mode != FileDialogMode::PickFolder {
                            ui.horizontal(|ui| {
                                ui.label(match dlg.mode {
                                    FileDialogMode::Open => "Type  ",
                                    FileDialogMode::ImportImage
                                    | FileDialogMode::ImportRaster
                                    | FileDialogMode::ImportObj
                                    | FileDialogMode::ImportTexture => "Type  ",
                                    FileDialogMode::ImportHdri | FileDialogMode::ImportIes => {
                                        "Type  "
                                    }
                                    FileDialogMode::Save => "Format",
                                    FileDialogMode::PickFolder => unreachable!(),
                                });
                                let before = dlg.ext.clone();
                                ui.selectable_value(
                                    &mut dlg.ext,
                                    ".dxf".to_string(),
                                    "DXF (*.dxf)",
                                );
                                ui.selectable_value(
                                    &mut dlg.ext,
                                    ".rsm".to_string(),
                                    "Native (*.rsm)",
                                );
                                // DWG ON THE SAVE SIDE TOO. It was offered for opening and not for
                                // saving, which is the shape of "why cant i save as dwg?": the app
                                // plainly handled the format, so its absence read as an oversight
                                // rather than a decision. It needs AutoCAD present — the same
                                // dependency opening one already has — and says so if it is not.
                                ui.selectable_value(
                                    &mut dlg.ext,
                                    ".dwg".to_string(),
                                    "AutoCAD (*.dwg)",
                                );
                                if dlg.mode == FileDialogMode::Open
                                    && dlg.ext != before
                                    && !dlg.filename.to_ascii_lowercase().ends_with(&dlg.ext)
                                {
                                    dlg.filename.clear();
                                }
                            });
                            // WHERE THE 3D PROJECT DATA GOES — only a real drawing Save As, and
                            // only for the two formats that can carry it. WBLOCK writes a bare
                            // sub-document (no project data at all), and a .pst / plot-output
                            // save is not a drawing, so neither shows the row. A .dwg cannot
                            // embed reliably (see `save_file_worker`), so the choice is disabled
                            // and forced back to separate files on confirm.
                            let is_drawing_save = dlg.mode == FileDialogMode::Save
                                && self.save_dialog_purpose == 0
                                && self.plotstyle_pst_io.is_none()
                                && !self.plot_pdf_browse
                                && self.wblock_subdoc.is_none();
                            if is_drawing_save {
                                ui.horizontal(|ui| {
                                    ui.label("3D project data");
                                    let dwg = dlg.ext == ".dwg";
                                    ui.add_enabled_ui(!dwg, |ui| {
                                        ui.selectable_value(
                                            &mut dlg.embed_extra,
                                            true,
                                            "Inside this file",
                                        );
                                        ui.selectable_value(
                                            &mut dlg.embed_extra,
                                            false,
                                            "Separate JSON files",
                                        );
                                    });
                                    if dwg {
                                        ui.weak(
                                            "AutoCAD DWG cannot carry the extra data — always \
                                             separate files",
                                        );
                                    } else {
                                        ui.weak(
                                            "3D model, furniture, materials, lighting and saved \
                                             results",
                                        );
                                    }
                                });
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.label(if dlg.mode == FileDialogMode::PickFolder {
                                "Subfolder"
                            } else {
                                "File"
                            });
                            ui.add(
                                egui::TextEdit::singleline(&mut dlg.filename)
                                    .desired_width(260.0)
                                    .hint_text(match dlg.mode {
                                        FileDialogMode::Open => "pick a file above",
                                        FileDialogMode::ImportImage
                                        | FileDialogMode::ImportRaster
                                        | FileDialogMode::ImportTexture => "pick an image above",
                                        FileDialogMode::ImportObj => {
                                            "pick an .obj, .3ds, .fbx, .glb or .gltf above"
                                        }
                                        FileDialogMode::ImportHdri => "pick an .hdr or .exr above",
                                        FileDialogMode::ImportIes => "pick an .ies or .ldt above",
                                        FileDialogMode::Save => "drawing name",
                                        FileDialogMode::PickFolder => {
                                            "optional — new subfolder to create here"
                                        }
                                    }),
                            );
                        });
                        if let Some(err) = &dlg.error {
                            ui.colored_label(egui::Color32::from_rgb(240, 120, 120), err);
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Cancel").clicked() {
                                        do_cancel = true;
                                    }
                                    let label = match dlg.mode {
                                        FileDialogMode::Open => "Open",
                                        FileDialogMode::ImportImage
                                        | FileDialogMode::ImportRaster
                                        | FileDialogMode::ImportObj
                                        | FileDialogMode::ImportTexture
                                        | FileDialogMode::ImportHdri => "Open",
                                        FileDialogMode::ImportIes => "Import",
                                        FileDialogMode::Save => "Save",
                                        FileDialogMode::PickFolder => "Use this folder",
                                    };
                                    if ui.button(label).clicked() {
                                        do_confirm = true;
                                    }
                                },
                            );
                        });
                        ui.add_space(2.0);
                    });

                // ---- CENTER: file list (left) + preview (right) -------------
                egui::CentralPanel::default().show_inside(ui, |ui| {
                    let body_h = ui.available_height().max(80.0);
                    let list_w = 280.0_f32;
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(list_w, body_h),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                egui::ScrollArea::vertical()
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        // Up one level.
                                        if dlg.dir.parent().is_some()
                                            && ui.selectable_label(false, "📁 ..").clicked()
                                        {
                                            if let Some(p) = dlg.dir.parent() {
                                                navigate = Some(p.to_path_buf());
                                            }
                                        }
                                        for d in &dirs {
                                            if ui
                                                .selectable_label(false, format!("📁 {}", d))
                                                .clicked()
                                            {
                                                navigate = Some(dlg.dir.join(d));
                                            }
                                        }
                                        for f in &files {
                                            // IN A FOLDER PICK, CLICKING A FILE TAKES ITS FOLDER.
                                            //
                                            // Reported as "why cant i select it", looking at a
                                            // list with FONDO.ldt in it. The files were drawn as
                                            // read-only context on the reasoning that the answer
                                            // is the FOLDER — true, and beside the point: someone
                                            // pointing at the file they came for is saying exactly
                                            // which folder they mean, and a list that ignores a
                                            // click reads as broken rather than as informative.
                                            //
                                            // It must NOT go through `filename`, which is why it
                                            // was inert to begin with: a folder pick reads that as
                                            // the NEW SUBFOLDER TO CREATE, so clicking FONDO.ldt
                                            // would have offered to make a directory called
                                            // FONDO.ldt. Confirming directly skips it, and the
                                            // clear() is belt-and-braces on the same hazard.
                                            if dlg.mode == FileDialogMode::PickFolder {
                                                if ui
                                                    .selectable_label(
                                                        false,
                                                        egui::RichText::new(format!("📄 {}", f))
                                                            .weak(),
                                                    )
                                                    .on_hover_text("Use the folder this is in")
                                                    .clicked()
                                                {
                                                    dlg.filename.clear();
                                                    do_confirm = true;
                                                }
                                                continue;
                                            }
                                            let selected = dlg.filename.eq_ignore_ascii_case(f);
                                            let resp =
                                                ui.selectable_label(selected, format!("📄 {}", f));
                                            if resp.clicked() {
                                                dlg.filename = f.clone();
                                                let l = f.to_ascii_lowercase();
                                                if l.ends_with(".rsm") {
                                                    dlg.ext = ".rsm".into();
                                                } else if l.ends_with(".dxf") {
                                                    dlg.ext = ".dxf".into();
                                                }
                                            }
                                            if resp.double_clicked() {
                                                dlg.filename = f.clone();
                                                do_confirm = true;
                                            }
                                        }
                                        if dirs.is_empty() && files.is_empty() {
                                            // Naming the filter rather than printing it raw: the
                                            // old line rendered an empty `ext` as "*", so an empty
                                            // folder pick read "(no folders or * files here)" —
                                            // which looks like a broken dialog rather than an
                                            // empty directory.
                                            let what = if dlg.ext.is_empty() {
                                                "folders".to_string()
                                            } else {
                                                format!(
                                                    "folders or {} files",
                                                    dlg.ext.replace('|', " / ")
                                                )
                                            };
                                            ui.weak(format!("(no {what} here)"));
                                        }
                                    });
                            },
                        );
                        ui.separator();
                        // Preview pane — fills the remaining width; the preview
                        // box is squared to the available space.
                        let prev_w = ui.available_width().max(120.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(prev_w, body_h),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.label("Preview");
                                let side = (prev_w - 6.0).min(body_h - 24.0).max(60.0);
                                let (prect, _) = ui.allocate_exact_size(
                                    egui::vec2(side, side),
                                    egui::Sense::hover(),
                                );
                                let sel = if dlg.filename.trim().is_empty() {
                                    None
                                } else {
                                    let p = dlg.dir.join(dlg.filename.trim());
                                    if p.is_file() {
                                        Some(p)
                                    } else {
                                        None
                                    }
                                };
                                match sel {
                                    Some(p) => {
                                        self.update_file_preview(&p);
                                        self.render_file_preview(ui.painter(), prect);
                                        if let Some(fp) = &self.file_preview {
                                            ui.weak(&fp.info);
                                        }
                                    }
                                    None => {
                                        ui.painter().rect_filled(
                                            prect,
                                            2.0,
                                            egui::Color32::from_rgb(24, 28, 34),
                                        );
                                        ui.painter().rect_stroke(
                                            prect,
                                            2.0,
                                            egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
                                        );
                                        // SAY WHAT THIS MODE ACTUALLY WANTS. "(select a file)" is
                                        // right for an Open, and in a FOLDER pick it invites the
                                        // one action that does nothing — which is exactly how it
                                        // was reported: "why cant i select it".
                                        let hint = if dlg.mode == FileDialogMode::PickFolder {
                                            "the files are what is in this folder"
                                        } else {
                                            "(select a file)"
                                        };
                                        ui.painter().text(
                                            prect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            hint,
                                            crate::theme::typ::data_code(),
                                            egui::Color32::from_gray(140),
                                        );
                                    }
                                }
                            },
                        );
                    });
                });
            });

        // ---- resolve actions --------------------------------------------
        if let Some(target) = navigate {
            dlg.dir = target;
            dlg.path_buf = dlg.dir.to_string_lossy().to_string(); // keep bar in sync
            dlg.error = None;
            if let Some(name) = pending_filename {
                // Path bar pointed at a file: preselect it (and its type).
                let l = name.to_ascii_lowercase();
                if l.ends_with(".rsm") {
                    dlg.ext = ".rsm".into();
                } else if l.ends_with(".dxf") {
                    dlg.ext = ".dxf".into();
                }
                dlg.filename = name;
            }
            self.file_dialog = Some(dlg);
            return;
        }
        if !open || do_cancel {
            self.file_dialog_dir = Some(dlg.dir);
            self.file_preview = None; // drop cached preview doc
            self.plotstyle_pst_io = None; // clear any .pst targeting
            self.plot_pdf_browse = false; // clear any PDF-path targeting
                                          // A cancelled Save-As must not leave a pending "close after save" armed.
            self.close_after_save = false;
            self.history.push("  file: cancelled".into());
            return;
        }
        if do_confirm {
            self.file_dialog_dir = Some(dlg.dir.clone());
            let name = dlg.filename.trim();
            if name.is_empty() && dlg.mode != FileDialogMode::PickFolder {
                dlg.error = Some("enter or pick a file name".into());
                self.file_dialog = Some(dlg);
                return;
            }
            // Plot output path (PDF/SVG/PNG) chosen from the Plot dialog /
            // preview window — write the file now (and open it in the viewer).
            if self.plot_pdf_browse {
                self.plot_pdf_browse = false;
                // The dialog's own format choice wins (the user can switch
                // PDF/SVG/PNG right in the dialog); sync plot_format so
                // run_plot emits the same format the dialog settled on.
                let dlg_ext = dlg.ext.trim_start_matches('.').to_ascii_lowercase();
                self.plot_format = match dlg_ext.as_str() {
                    "svg" => "svg".into(),
                    "png" => "png".into(),
                    _ => "pdf".into(),
                };
                let ext = match self.plot_format.as_str() {
                    "svg" => "svg",
                    "png" => "png",
                    _ => "pdf",
                };
                let l = name.to_ascii_lowercase();
                let fname = if l.ends_with(&format!(".{}", ext)) {
                    name.to_string()
                } else {
                    format!("{}.{}", name, ext)
                };
                self.plot_pdf_path = dlg.dir.join(fname).to_string_lossy().to_string();
                self.file_preview = None;
                if self.plot_run_after_save {
                    self.plot_run_after_save = false;
                    self.run_plot(); // writes the output, then opens it in the viewer
                }
                return;
            }
            // Plot-style .pst save/load intercepts the normal drawing I/O.
            if let Some(save) = self.plotstyle_pst_io.take() {
                let l = name.to_ascii_lowercase();
                let fname = if l.ends_with(".pst") {
                    name.to_string()
                } else {
                    format!("{}.pst", name)
                };
                let path = dlg.dir.join(fname);
                if save {
                    match cad_io::save_plot_table(&path, &self.doc.plot_styles) {
                        Ok(()) => {
                            self.plotstyle_edit_file = Some(path.clone());
                            self.history
                                .push(format!("  plot table saved → {}", path.display()));
                        }
                        Err(e) => self
                            .history
                            .push(format!("  ! plot table save failed: {}", e)),
                    }
                } else {
                    match cad_io::load_plot_table(&path) {
                        Ok(t) => {
                            // A LOADED table is not bound to a CTB file — clear the
                            // binding so "Save changes" can't overwrite the file the
                            // editor was previously bound to with this table.
                            self.plotstyle_edit_file = None;
                            self.doc.plot_styles = t;
                            self.history
                                .push(format!("  plot table loaded ← {}", path.display()));
                        }
                        Err(e) => self
                            .history
                            .push(format!("  ! plot table load failed: {}", e)),
                    }
                }
                self.file_dialog_dir = Some(dlg.dir.clone());
                self.file_preview = None;
                // .pst launched from the editor → reopen the editor.
                if self.plotstyle_return_after_sub {
                    self.plotstyle_open = true;
                    self.plotstyle_return_after_sub = false;
                }
                return;
            }
            match dlg.mode {
                FileDialogMode::Open => {
                    let path = dlg.dir.join(name);
                    // The SAME browser, asked for by two callers. Illuminaire wants a file for
                    // its block table only — opening it as a project would throw away whatever
                    // the user is working on, which is not what "load block file" means.
                    if std::mem::take(&mut self.illuminaire_wants_blocks) {
                        self.load_illuminaire_blocks(&path.to_string_lossy());
                    } else {
                        self.do_open(&path.to_string_lossy());
                    }
                }
                FileDialogMode::ImportImage => {
                    let path = dlg.dir.join(name);
                    // The same browser, asked for by two callers — a raster to trace, or a render
                    // for the report.
                    match std::mem::take(&mut self.report_wants_images) {
                        Some(slot) => {
                            self.report_add_image(&path.to_string_lossy(), Some(ctx), slot)
                        }
                        None => self.do_import_image(&path.to_string_lossy()),
                    }
                }
                FileDialogMode::ImportRaster => {
                    let path = dlg.dir.join(name);
                    self.import_raster_underlay(&path.to_string_lossy());
                }
                FileDialogMode::ImportObj => {
                    let path = dlg.dir.join(name);
                    // The same picker serves ▼ Furniture and ▼ FBC scene import; the flag says
                    // which asked, and a scene is placed 1:1 at the origin rather than normalised
                    // to a 1.5 m prop at the cursor.
                    if std::mem::take(&mut self.scene_import_pending) {
                        self.import_scene_file(&path.to_string_lossy());
                    } else {
                        self.import_furniture_obj(&path.to_string_lossy());
                    }
                }
                FileDialogMode::ImportTexture => {
                    let path = dlg.dir.join(name);
                    self.load_texture_from_file(&path.to_string_lossy());
                }
                FileDialogMode::ImportIes => {
                    // A photometric file joins the FITTINGS library and becomes the chosen one,
                    // the same way an imported mesh joins the furniture library.
                    let path = dlg.dir.join(name);
                    self.light.load_photometry(&path.to_string_lossy());
                    self.history.push(format!("  {}", self.light.last_msg));
                }
                FileDialogMode::ImportHdri => {
                    // Loading does the SH projection and the GGX prefilter, once. Both are
                    // seconds-scale on a 4K map, so the busy overlay is what the user sees.
                    let path = dlg.dir.join(name);
                    match crate::env_map::EnvMap::load(&path) {
                        Ok(m) => {
                            let msg = self.factory.set_env_map(Some(m));
                            self.history.push(format!("  {msg}"));
                            self.factory.status = msg;
                            self.factory.open = true;
                        }
                        Err(e) => {
                            self.factory.status = format!("environment: {e}");
                            self.history.push(format!("  ! environment: {e}"));
                        }
                    }
                }
                FileDialogMode::Save => {
                    // Ensure the chosen extension; if the name already ends
                    // in .dxf/.rsm honour that, else append the toggle's ext.
                    let l = name.to_ascii_lowercase();
                    let fname = if l.ends_with(".dxf") || l.ends_with(".rsm") {
                        name.to_string()
                    } else {
                        format!("{}{}", name, dlg.ext)
                    };
                    let path = dlg.dir.join(fname);
                    let path = path.to_string_lossy().into_owned();
                    if self.save_dialog_purpose == 1 {
                        // WBLOCK confirm — write the pending sub-document.
                        if let Some(sub) = self.wblock_subdoc.take() {
                            let n = sub.dobjects.len();
                            if self.save_doc_to(&path, &sub) {
                                self.history.push(format!(
                                    "  wblock: wrote {} dobject(s) to '{}'",
                                    n, path
                                ));
                            }
                            self.clear_prompt();
                        } else {
                            self.do_save(&path);
                        }
                    } else {
                        // Any stale wblock sub-doc (cancelled dialog) must not
                        // hijack a normal Save As.
                        self.wblock_subdoc = None;
                        // COMMIT THE EXTRA-DATA CHOICE. It is the mode of every
                        // following plain Save / autosave too, until this dialog
                        // changes it again — the file the user chose to write with
                        // "inside the file" stays that way on Ctrl+S, exactly as a
                        // file opened from an embedded project does.
                        self.extra_store = if dlg.ext == ".dwg" {
                            crate::simlux_io::ExtraDataStore::Sidecar
                        } else if dlg.embed_extra {
                            crate::simlux_io::ExtraDataStore::Embedded
                        } else {
                            crate::simlux_io::ExtraDataStore::Sidecar
                        };
                        self.do_save(&path);
                    }
                }
                FileDialogMode::PickFolder => {
                    // The Radiance output folder: the browsed dir, or a new subfolder in it.
                    let dir = if name.is_empty() {
                        dlg.dir.clone()
                    } else {
                        dlg.dir.join(name)
                    };
                    // …or the Light Editor's photometry folder, scanned as soon as it is chosen so
                    // the right-hand list fills in without a second click.
                    if std::mem::take(&mut self.report_wants_dir) {
                        self.report_opts.out_dir = dir.to_string_lossy().into_owned();
                    } else if self.photometry_wants_folder {
                        self.photometry_wants_folder = false;
                        self.light.lib_folder = dir.to_string_lossy().into_owned();
                        self.light.lib_scanned =
                            crate::illuminaire::scan_folder(&self.light.lib_folder);
                        self.history.push(format!(
                            "  illuminaire: {} photometry file(s) found",
                            self.light.lib_scanned.len()
                        ));
                    // …or a PBR texture-set folder, if that is what asked for the picker.
                    } else if let Some(i) = self.texset_target.take() {
                        self.mf_load_texture_set(i, dir);
                    } else if self.rad_run_after_pick {
                        self.run_radiance_in(dir);
                    } else {
                        self.export_radiance_scene_to(&dir);
                    }
                }
            }
            self.file_preview = None; // drop cached preview doc
            return; // dialog closes on success
        }
        self.file_dialog = Some(dlg);
    }

    // ===================================================================
    // Slice J — Editing operations
    // ===================================================================
    //
    // Each operation snapshots the Document before mutating so `undo` can
    // roll back. The snapshot stack is bounded (UNDO_STACK_CAP) — oldest
    // snapshots fall off when the stack is full.

    /// Keep the undo history inside BOTH its limits: a count cap, and a memory budget.
    ///
    /// ONE FUNCTION, called after every push, because the two limits used to be one — a bare
    /// `if len >= CAP { remove(0) }` repeated at each snapshot site — and a count is the wrong
    /// unit for a stack whose steps are whole documents. Measured before this existed: 15.7 MB per
    /// step at 100k dobjects and 240.2 MB at 1.5M, i.e. 15.4 GB at the 64-step cap.
    ///
    /// Oldest first, and never below [`UNDO_MIN_STEPS`]: a single snapshot of a large enough
    /// document exceeds the budget on its own, and an undo that silently does nothing is worse
    /// than the memory it saves.
    pub(super) fn trim_undo_stack(&mut self) {
        while self.undo_stack.len() > UNDO_STACK_CAP {
            self.undo_stack.remove(0);
        }
        let mut total: usize = self.undo_stack.iter().map(|s| s.approx_bytes()).sum();
        while total > self.undo_budget_bytes && self.undo_stack.len() > UNDO_MIN_STEPS {
            total -= self.undo_stack[0].approx_bytes();
            self.undo_stack.remove(0);
        }
    }

    /// Snapshot for an edit that changes the FIXTURES only — a drag, a delete, an assignment.
    ///
    /// Uses the staged copy when there is one, because a method that has already mutated cannot
    /// hand back what it overwrote. See [`crate::light::LightState::stage_undo`].
    #[track_caller]
    pub(super) fn snapshot_lights(&mut self) {
        let luminaires = self
            .light
            .undo_pending
            .take()
            .unwrap_or_else(|| self.light.luminaires.clone());
        self.unsaved = true;
        self.undo_stack
            .push(UndoStep::Light(LightSnap { luminaires }));
        self.trim_undo_stack();
        self.redo_stack.clear();
    }

    /// Snapshot for an edit that changes the DRAWING and the FIXTURES as one act.
    ///
    /// One step, not two: placing a fitting puts a block on the plan and a light at the same
    /// point, and an Undo that took back half of it would leave a symbol with nothing behind it.
    #[track_caller]
    pub(super) fn snapshot_doc_and_lights(&mut self) {
        let luminaires = self
            .light
            .undo_pending
            .take()
            .unwrap_or_else(|| self.light.luminaires.clone());
        self.unsaved = true;
        self.edit_seq = self.edit_seq.wrapping_add(1);
        self.undo_stack
            .push(UndoStep::Both(self.doc.clone(), LightSnap { luminaires }));
        self.trim_undo_stack();
        self.redo_stack.clear();
    }

    /// Drop a bare fixture point on the plan, undoably.
    ///
    /// ONE PLACE THAT DOES BOTH HALVES, because a placement that is not committed to the undo
    /// stack looks exactly like one that is until someone presses Ctrl+Z and loses a wall instead.
    pub(super) fn place_fixture_point(&mut self, x: f32, y: f32) -> u32 {
        let id = self.light.place_point(x, y);
        self.commit_light_undo();
        id
    }

    /// RE-READ EVERY FIXTURE FROM THE SYMBOL IT WAS PLACED AS.
    ///
    /// Reported as: "i rotated a light and the result was still valid… does rotating the lights in
    /// the 2d actually rotate a light in simlux as well?" It did not. `apply_rotate`, `apply_move`,
    /// `apply_scale` and `apply_mirror` edit `doc.dobjects` and nothing else — verified by reading
    /// them — so the symbol turned on the plan and the luminaire behind it kept the aiming it was
    /// placed with. The calculation then answered a question about a layout the drawing no longer
    /// showed, and because nothing about the FIXTURE had changed, the result did not even go out of
    /// date. The staleness check was working; there was simply nothing for it to notice.
    ///
    /// ONE PLACE, DRIVEN BY "THE DRAWING CHANGED", rather than a call added to each editing
    /// command. There are a dozen of those and more will be written; every one of them would have
    /// to remember, and the day one does not, a light silently stops matching its symbol. Every
    /// edit takes an undo snapshot and every snapshot bumps `edit_seq`, so that is the signal — and
    /// it covers undo and redo too, which restore a whole document and would otherwise leave the
    /// fixtures behind.
    ///
    /// Returns how many fixtures actually moved, so the caller only invalidates when something did.
    fn sync_fixtures_from_symbols(&mut self) -> usize {
        if self.light.symbol_of.is_empty() {
            return 0;
        }
        let ku = self.doc.units.metres_per_unit;
        let k = if ku.is_finite() && ku > 0.0 { ku } else { 1.0 };
        // Handle → geometry, built once. A scan per fixture would be O(fixtures × dobjects), which
        // on a real plan is five hundred times fifty thousand.
        let by_handle: std::collections::HashMap<u64, (Vec2, f64)> = self
            .doc
            .dobjects
            .iter()
            .filter_map(|d| match &d.geom {
                cad_kernel::Geom::BlockRef(b) => Some((d.handle, (b.insert, b.rotation))),
                _ => None,
            })
            .collect();

        let mut moved = 0usize;
        for l in self.light.luminaires.iter_mut() {
            let Some(h) = self.light.symbol_of.get(&l.id) else {
                continue;
            };
            // A HANDLE THAT NO LONGER RESOLVES IS LEFT ALONE. The symbol may have been erased from
            // the drawing, and moving the light to the origin — or deleting it — because a block
            // went missing would be a far larger decision than this function is entitled to make.
            let Some((insert, rot)) = by_handle.get(h) else {
                continue;
            };
            let (wx, wy) = ((insert.x * k) as f32, (insert.y * k) as f32);
            let deg = rot.to_degrees() as f32;
            // A TOLERANCE, because the position makes a round trip through drawing units every
            // time a fixture is dragged. Writing back a value that differs in its last bit would
            // mark the result out of date on a frame where nothing happened.
            let dp = (l.position.x - wx).abs().max((l.position.y - wy).abs());
            let dr = (l.rotation_deg - deg).abs();
            if dp <= 1e-4 && dr <= 1e-3 {
                continue;
            }
            l.position.x = wx;
            l.position.y = wy;
            l.rotation_deg = deg;
            moved += 1;
        }
        moved
    }

    /// Keep the fixtures with their symbols, once per drawing edit.
    ///
    /// Cheap when nothing happened: an integer comparison. The work only runs on the frame after an
    /// edit actually took a snapshot.
    pub(super) fn tick_symbol_sync(&mut self) {
        if self.symbol_sync_seq == self.edit_seq {
            return;
        }
        self.symbol_sync_seq = self.edit_seq;
        // NOT DURING A SIMLUX DRAG. There the FIXTURE is what the pointer is moving and the symbol
        // is following it; pulling the other way in the same frame would have the two arguing, and
        // the drag would fight its own undo snapshot.
        if self.light.drag.is_some() {
            return;
        }
        let n = self.sync_fixtures_from_symbols();
        if n > 0 {
            self.history.push(format!(
                "  ↻ {n} fitting(s) followed their symbol — recalculate for the new layout"
            ));
            self.touch_view();
        }
    }

    /// One click of the aim tool: pick a fitting, then pick where it should point.
    ///
    /// Asked for as: *"in the luminaries tab i want a aim tool… when aim i selected the user can
    /// select a light and then click on a point where they would like to point it."*
    ///
    /// THE FITTING DOES NOT MOVE. "while aiming the light stays at the same height and at the same
    /// location, its place where its pointed downward is what we are changing" — so this writes the
    /// pose and never the position.
    ///
    /// THE TARGET IS ON THE WORKING PLANE. A click on a plan gives an x and a y and no height at
    /// all, and the working plane is the one surface such a click unambiguously names: it is the
    /// plane the whole calculation is about and the one the false colours on screen belong to.
    /// Aiming at a wall wants a height as well, which is a box to add when somebody needs it — not
    /// a number to guess at now.
    pub(super) fn aim_click(&mut self, x: f32, y: f32, tol: f32) {
        match self.light.aim_pick {
            // ---- second click: the target -------------------------------------------------
            Some(id) => {
                let z = self.light.plane_height;
                let Some(l) = self.light.luminaires.iter().find(|l| l.id == id) else {
                    self.light.aim_pick = None;
                    return;
                };
                // A fitting is aimed DOWNWARD, and a target at or above it has no such pose. Said
                // rather than clamped: a light silently left flat would look aimed and not be.
                if l.position.z <= z + 1e-3 {
                    self.light.last_msg = format!(
                        "Fitting #{id} is at {:.2} m, at or below the working plane at {z:.2} m — \
                         nothing below it to aim at.",
                        l.position.z,
                    );
                    self.light.aim_pick = None;
                    return;
                }
                self.snapshot_doc_and_lights();
                let target = glam::Vec3::new(x, y, z);
                let mut done = false;
                if let Some(l) = self.light.luminaires.iter_mut().find(|l| l.id == id) {
                    done = l.aim_at(target);
                }
                if done {
                    let l = self
                        .light
                        .luminaires
                        .iter()
                        .find(|l| l.id == id)
                        .expect("just aimed it");
                    self.light.last_msg = format!(
                        "Fitting #{id} aimed at ({x:.2}, {y:.2}) — {:.0}° from vertical, toward \
                         {:.0}°. Click another fitting, or Esc to stop.",
                        l.tilt_deg, l.rotation_deg,
                    );
                    self.touch_view();
                }
                // Ready for the next one, so a run of fittings can be aimed without re-arming.
                self.light.aim_pick = None;
            }
            // ---- first click: which fitting -------------------------------------------------
            None => match self.light.pick_at(x, y, tol) {
                Some(id) => {
                    self.light.aim_pick = Some(id);
                    // Selected as well, so the panel's parameters are about the one being aimed.
                    self.light.selected = vec![id];
                    self.light.last_msg =
                        format!("Fitting #{id} — now click the point it should light.");
                }
                None => {
                    self.light.last_msg =
                        "No fitting there. Click a marker to pick the light to aim.".into();
                }
            },
        }
    }
    /// Start dragging a fixture, and take its SYMBOL along.
    ///
    /// Reported as: dragging a fixture moves the light but not its symbol. It did — `from_block`
    /// names the block DEFINITION, which every instance of a fitting shares, so the drawing had no
    /// idea which of fifty identical downlights belonged to the marker under the pointer. The link
    /// is by POSITION, and the one moment the two are certainly still together is the press.
    ///
    /// So the pairing is made HERE, before anything moves, and held for the length of the gesture.
    /// Re-deriving it mid-drag would find nothing, because by then the fixture has moved away from
    /// the symbol it is supposed to be carrying — which is precisely the bug.
    pub(super) fn begin_fixture_drag(&mut self, id: u32, at: (f32, f32)) {
        self.light.begin_drag(id, at);
        let k = self.doc.units.metres_per_unit;
        let ids: Vec<u32> = self.light.selected.clone();
        let dragged: Vec<cad_light::Luminaire> = self
            .light
            .luminaires
            .iter()
            .filter(|l| ids.contains(&l.id))
            .cloned()
            .collect();
        self.light_drag_symbols = crate::illuminaire::claim_instances(&self.doc, dragged.iter(), k);
    }

    /// Put every claimed symbol back under its fixture.
    ///
    /// Called on each frame OF the drag rather than once at the end: a symbol that jumped to its
    /// new home only on release would leave the drag looking exactly as broken as it did before,
    /// for the whole time anybody is watching it.
    pub(super) fn drag_fixture_symbols(&mut self) {
        if self.light_drag_symbols.is_empty() {
            return;
        }
        let ku = self.doc.units.metres_per_unit;
        let k = if ku.is_finite() && ku > 0.0 { ku } else { 1.0 };
        let mut moved = false;
        for (id, index) in self.light_drag_symbols.clone() {
            let Some(l) = self.light.luminaires.iter().find(|l| l.id == id) else {
                continue;
            };
            let at = Vec2::new(l.position.x as f64 / k, l.position.y as f64 / k);
            moved |= crate::illuminaire::move_instance(&mut self.doc, index, at);
        }
        if moved {
            // The same three the placement path needs, and for the same reason: without them the
            // symbol is drawn from stale cached geometry and cannot be picked where it now is.
            self.intersections.clear();
            self.index_dirty = true;
            self.touch_view();
        }
    }

    /// Turn a snapshot staged inside the panel into an undo step, if one is still waiting.
    ///
    /// The panel's own edits — assigning a fitting, dropping one from the library — happen inside
    /// a closure that cannot reach `self.doc`, so they leave the fixtures they overwrote here and
    /// this collects them once the closure has gone. Everything driven from `CadApp` snapshots
    /// directly and drains the staging as it goes, so there is nothing left for this to find.
    pub(super) fn commit_light_undo(&mut self) {
        if self.light.undo_pending.is_some() {
            self.snapshot_lights();
        }
    }

    #[track_caller]
    pub(super) fn snapshot_doc(&mut self) {
        let t = std::time::Instant::now();
        self.unsaved = true; // an edit is about to happen → drawing diverges from disk
        self.edit_seq = self.edit_seq.wrapping_add(1);
        self.undo_stack.push(UndoStep::Doc(self.doc.clone()));
        self.trim_undo_stack();
        // A new editing op invalidates the redo branch — once you diverge
        // from the previously-redoable history you can't return to it.
        self.redo_stack.clear();
        // Recorder — undo snapshot is a major memory event (Document
        // clone). Capture both the new depth + a rough byte estimate
        // (per-dobject ~200B is the ballpark for our current variants).
        let elapsed_us = t.elapsed().as_micros() as u64;
        let bytes_estimate = self.doc.dobjects.len() * 200
            + self.doc.layers.len() * 64
            + self.doc.linetypes.len() * 64;
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::UndoSnapshotTaken {
                undo_depth_after: self.undo_stack.len(),
                bytes_estimate,
            }
        );
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::MemoryEvent {
                name: "snapshot_doc (Document clone)".into(),
                bytes: bytes_estimate,
                elapsed_us,
            }
        );
    }

    /// Snapshot the 3D Factory before a modelling operation — the 3D counterpart of
    /// [`Self::snapshot_doc`], pushing onto the SAME stack so one Ctrl+Z walks back
    /// through both views in the order the edits happened.
    ///
    /// Before this existed every solid operation was irreversible: `snapshot_doc` clones
    /// the `Document`, which does not contain the Factory model.
    /// The Factory model as a snapshot must record it — with the OPEN PLANE'S DRAWING IN IT.
    ///
    /// `factory_enter_sketch` does `mem::take` on `model.sketches[idx].doc` and installs it as
    /// `self.doc`, so while a session is live the MODEL's copy of the open plane is EMPTY. Any
    /// snapshot that clones the model naively records that hole, and restoring it after the sketch
    /// closed wipes everything drawn on that face.
    ///
    /// EXTRACTED BECAUSE THE TWO SITES HAD ALREADY DRIFTED. `snapshot_factory` patched this and
    /// claimed in its own comment that every snapshot was therefore self-consistent "whoever takes
    /// one" — while `counterpart_of`, which builds the opposite-stack entry on EVERY undo and EVERY
    /// redo, cloned the model bare. The invariant that comment asserts was false, and it was false
    /// on the path that runs most. One function now, so they cannot diverge again.
    fn factory_model_for_snapshot(&self) -> cad_solid::Model {
        let mut model = self.factory.model.clone();
        if let Some(s) = self.factory.session.as_ref() {
            if let Some(sk) = model.sketch_by_id_mut(s.plane) {
                sk.doc = self.doc.clone();
            }
        }
        model
    }

    #[track_caller]
    pub(super) fn snapshot_factory(&mut self) {
        self.unsaved = true; // a 3D edit is about to happen → drawing diverges from disk
        self.edit_seq = self.edit_seq.wrapping_add(1);
        // A SNAPSHOT TAKEN INSIDE A SKETCH MUST NOT RECORD THAT SKETCH AS EMPTY.
        //
        // `factory_enter_sketch` does `mem::take` on `model.sketches[idx].doc` and installs it as
        // `self.doc`, so while a session is live the MODEL's copy of the open plane is empty. A
        // Factory snapshot clones the model — hole and all — and undoing to it after the sketch
        // closed would restore that empty doc and wipe everything drawn on the face.
        //
        // Patched at the source, so every snapshot is self-consistent whoever takes one.
        let model = self.factory_model_for_snapshot();
        self.undo_stack.push(UndoStep::Factory(FactorySnap {
            model,
            walls: self.factory.walls.clone(),
            storeys: self.factory.storeys.clone(),
            active_storey: self.factory.active_storey,
            ceilings: self.factory.ceilings.clone(),
            furniture: self.factory.furniture.clone(),
            feature_color: self.factory.feature_color.clone(),
            surface_color: self.factory.surface_color.clone(),
            feature_texture: self.factory.feature_texture.clone(),
            surface_texture: self.factory.surface_texture.clone(),
            feature_group: self.factory.feature_group.clone(),
            rooms: self.factory.rooms.clone(),
            next_room_id: self.factory.next_room_id,
            texture_xforms: self
                .factory
                .textures
                .iter()
                .map(|t| (t.scale, t.offset, t.rot_deg, t.opacity, t.reflect))
                .collect(),
        }));
        self.trim_undo_stack();
        self.redo_stack.clear();
    }

    /// Roll back a 2D operation that failed after it had already snapshotted — pop the
    /// snapshot it pushed and restore it, so a failed command leaves NO undo step behind.
    ///
    /// Pops only when the top really is a `Doc` step. A `Factory` step on top is not this
    /// command's to consume, and discarding it would silently eat someone else's undo.
    pub(super) fn rollback_doc(&mut self) {
        if matches!(self.undo_stack.last(), Some(UndoStep::Doc(_))) {
            if let Some(UndoStep::Doc(prev)) = self.undo_stack.pop() {
                self.doc = prev;
            }
        }
    }

    /// Capture the CURRENT state in the same shape as the step being undone, so it can
    /// be pushed onto the opposite stack. Keeping this one function is what guarantees
    /// undo and redo stay exact inverses.

    /// The fixtures as they are now, for an undo step.
    fn light_snap(&self) -> LightSnap {
        LightSnap {
            luminaires: self.light.luminaires.clone(),
        }
    }

    /// Put the drawing back, and invalidate everything that was derived from it.
    fn restore_doc(&mut self, doc: Document) {
        // Any grip drag / paper-space drag in flight names indices (or
        // geometry states) of the OLD document — a release after this
        // restore would apply a stale vertex index to the new geometry.
        // Kill the drags so the next gesture re-grabs cleanly.
        self.grip_drag = None;
        self.grip_drag_peers.clear();
        self.layout_grip_drag = None;
        self.layout_move_last = None;
        self.viewport_draw_state = 0;
        self.viewport_draw_p1 = None;
        self.viewport_draw_p2 = None;
        self.viewport_draw_li = None;
        self.viewport_scale_dialog_open = false;
        self.vp_edit_dialog = None;
        self.vp_edit_init_for = None;
        self.doc = doc;
        self.selection.clear();
        self.selected = None;
        self.layout_selection.clear();
        self.intersections.clear();
        self.index_dirty = true;
        // The restored document carries the layer-table arrangement and
        // active space of the SNAPSHOT moment. Re-sync the on-screen camera
        // with the space the snapshot is in, or the canvas keeps the other
        // tab's zoom/pan (and the swap invariant doc.layers == active
        // space's table holds, but the camera lies).
        match self.doc.active_layout {
            Some(li) => {
                if let Some(l) = self.doc.layouts.get(li) {
                    self.scale = l.camera.zoom;
                    self.world_offset = egui::vec2(l.camera.pan_x, l.camera.pan_y);
                }
            }
            None => {
                if let (Some(s), Some(o)) = (
                    self.saved_model_scale.take(),
                    self.saved_model_offset.take(),
                ) {
                    self.scale = s;
                    self.world_offset = o;
                }
            }
        }
        self.touch_view();
    }

    /// Put the fixtures back.
    ///
    /// The SELECTION and any drag in flight go, for the same reason the drawing's selection does:
    /// both name fixtures by id, and the set of ids has just changed underneath them. A drag left
    /// pointing at a fixture that no longer exists would move nothing and never end.
    ///
    /// `next_id` is NOT rewound — see [`LightSnap`]. Nor is a result recalculated: the lux figures
    /// on screen were computed from a layout that no longer holds, and quietly leaving stale
    /// numbers under a restored layout would be worse than showing none, so the panel is told they
    /// are stale rather than silently kept.
    pub(super) fn restore_lights(&mut self, snap: LightSnap) {
        self.light.luminaires = snap.luminaires;
        self.light.selected.clear();
        self.light.drag = None;
        self.light.hover = None;
        self.light.undo_pending = None;
        self.light.invalidate_result();
    }

    pub(super) fn counterpart_of(&self, step: &UndoStep) -> UndoStep {
        match step {
            UndoStep::Doc(_) => UndoStep::Doc(self.doc.clone()),
            UndoStep::Light(_) => UndoStep::Light(self.light_snap()),
            UndoStep::Both(_, _) => UndoStep::Both(self.doc.clone(), self.light_snap()),
            UndoStep::Factory(_) => UndoStep::Factory(FactorySnap {
                // THE FIX: was , which records the open plane empty.
                model: self.factory_model_for_snapshot(),
                walls: self.factory.walls.clone(),
                storeys: self.factory.storeys.clone(),
                active_storey: self.factory.active_storey,
                ceilings: self.factory.ceilings.clone(),
                furniture: self.factory.furniture.clone(),
                feature_color: self.factory.feature_color.clone(),
                surface_color: self.factory.surface_color.clone(),
                feature_texture: self.factory.feature_texture.clone(),
                surface_texture: self.factory.surface_texture.clone(),
                feature_group: self.factory.feature_group.clone(),
                rooms: self.factory.rooms.clone(),
                next_room_id: self.factory.next_room_id,
                texture_xforms: self
                    .factory
                    .textures
                    .iter()
                    .map(|t| (t.scale, t.offset, t.rot_deg, t.opacity, t.reflect))
                    .collect(),
            }),
        }
    }

    /// Restore one step. 2D and 3D need different invalidation, and doing the wrong one
    /// leaves a stale render — hence one place that owns both.
    fn restore_step(&mut self, step: UndoStep) {
        match step {
            UndoStep::Doc(doc) => self.restore_doc(doc),
            UndoStep::Light(l) => self.restore_lights(l),
            UndoStep::Both(doc, l) => {
                self.restore_doc(doc);
                self.restore_lights(l);
            }
            UndoStep::Factory(snap) => {
                self.factory.model = snap.model;
                self.factory.walls = snap.walls;
                self.factory.storeys = snap.storeys;
                self.factory.active_storey = snap.active_storey;
                self.factory.ceilings = snap.ceilings;
                self.factory.furniture = snap.furniture;
                self.factory.feature_color = snap.feature_color;
                self.factory.surface_color = snap.surface_color;
                self.factory.feature_texture = snap.feature_texture;
                self.factory.surface_texture = snap.surface_texture;
                self.factory.feature_group = snap.feature_group;
                self.factory.rooms = snap.rooms;
                self.factory.next_room_id = snap.next_room_id;
                // Restore per-texture tiling/move/rotate onto the existing texture assets (their
                // pixels are unchanged, so only the transforms roll back).
                for (i, (s, o, r, op, rf)) in snap.texture_xforms.into_iter().enumerate() {
                    if let Some(t) = self.factory.textures.get_mut(i) {
                        t.scale = s;
                        t.offset = o;
                        t.rot_deg = r;
                        t.opacity = op;
                        t.reflect = rf;
                    }
                }
                // The restored furniture list may be shorter — drop any now-invalid index. Kept
                // per-entry rather than all-or-nothing so an undo that removes one piece does not
                // also silently deselect the others.
                let live = self.factory.furniture.len();
                self.factory.sel_furniture.retain(|&i| i < live);
                // The restored ids are not the ones that were selected.
                self.factory.clear_selection();
                // An in-flight modify would keep operating on features that no longer
                // exist — cancel it rather than let it mis-edit.
                self.factory.modify = None;
                self.factory.dirty = true;
                // Undo is a discrete user action, so paying one CSG evaluation here is
                // correct — the alternative is a viewport that still shows the undone
                // state until something else happens to trigger a rebuild.
                self.factory.recompute();
            }
        }
    }

    #[track_caller]
    pub(super) fn do_undo(&mut self) {
        let from_depth = self.undo_stack.len();
        match self.undo_stack.pop() {
            Some(prev) => {
                // Stash current state on the redo stack before restoring.
                if self.redo_stack.len() >= UNDO_STACK_CAP {
                    self.redo_stack.remove(0);
                }
                let cur = self.counterpart_of(&prev);
                self.redo_stack.push(cur);
                self.restore_step(prev);
                crate::dbg_event!(
                    self,
                    crate::dbg_recorder::DbgEvent::UndoFired {
                        from_depth,
                        to_depth: self.undo_stack.len(),
                    }
                );
                self.history.push(format!(
                    "  ↶ undo  (undo: {}  redo: {})",
                    self.undo_stack.len(),
                    self.redo_stack.len()
                ));
            }
            None => self.history.push("  ! nothing to undo".into()),
        }
    }

    #[track_caller]
    pub(super) fn do_redo(&mut self) {
        let from_depth = self.redo_stack.len();
        match self.redo_stack.pop() {
            Some(next) => {
                // Stash current state back on the undo stack — symmetric, and trimmed by the same
                // rule as any other push. A redo puts a full document back on the stack too.
                let cur = self.counterpart_of(&next);
                self.undo_stack.push(cur);
                self.trim_undo_stack();
                self.restore_step(next);
                crate::dbg_event!(
                    self,
                    crate::dbg_recorder::DbgEvent::RedoFired {
                        from_depth,
                        to_depth: self.redo_stack.len(),
                    }
                );
                self.history.push(format!(
                    "  ↷ redo  (undo: {}  redo: {})",
                    self.undo_stack.len(),
                    self.redo_stack.len()
                ));
            }
            None => self.history.push("  ! nothing to redo".into()),
        }
    }
}
