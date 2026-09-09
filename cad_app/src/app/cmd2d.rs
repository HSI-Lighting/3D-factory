use super::*;

// ============ 2D command machinery: Slice K .. array ============
// Command apply flows and geometry machinery for the 2D drafting
// workspace: matchprop/reverse/chlayer (Slice K), Blocks
// (create/insert/explode), prompt-driven command flow (CIRCLE), PEDIT
// shims, Slice L / Slice M apply methods, trim/extend, fillet/chamfer/
// join, coordinate transforms, interactive-draw finalisation, the array
// generator. Child module of `app`; entry methods are pub(super)/
// pub(crate) so the shell, panel and tests can reach them.

impl CadApp {
    // ---- Slice K: matchprop / reverse / chlayer apply methods ----

    /// Paint one target with the source's properties: general style (layer /
    /// color / linetype via `d.style`) always, plus the geom-embedded style
    /// (wall / dim / text) when both are the same kind. Returns true on apply.
    pub(super) fn apply_matchprop_one(&mut self, src_idx: usize, target_idx: usize) -> bool {
        if src_idx == target_idx {
            return false;
        } // self-match = no-op
        let (src_style, src_geom) = match self.doc.dobjects.get(src_idx) {
            Some(s) => (s.style, s.geom.clone()),
            None => return false,
        };
        let Some(t) = self.doc.dobjects.get_mut(target_idx) else {
            return false;
        };
        t.style = src_style;
        // Geom-embedded style — only when source and target are the same kind.
        match (&src_geom, &mut t.geom) {
            (Geom::Wall(s), Geom::Wall(d)) => d.style = s.style,
            (Geom::Dimension(s), Geom::Dimension(d)) => d.style = s.style,
            (Geom::Text(s), Geom::Text(d)) => d.style = s.style,
            _ => {}
        }
        self.touch_view();
        true
    }

    // ---- Blocks: create / insert / explode ----

    /// Shared entry for block creation (typed `block <name>` AND the
    /// Block dialog's OK). Universal selection model: empty basket opens
    /// a select session with the op queued; otherwise straight to the
    /// base-point click.
    /// Lower-left (min) corner of the current selection's combined bbox,
    /// in world coords. (0,0) when nothing is selected. Used as the Block
    /// dialog's default insertion point.
    fn selection_min_corner(&self) -> Vec2 {
        let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut any = false;
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get(i) {
                let (mn, _mx) = d.geom.bbox();
                if mn.x < min.x {
                    min.x = mn.x;
                }
                if mn.y < min.y {
                    min.y = mn.y;
                }
                any = true;
            }
        }
        if any {
            min
        } else {
            Vec2::ZERO
        }
    }

    /// Centre of the selection's combined bounding box — the "gravity centre"
    /// used as the DEFAULT block base/insertion point (user rule 2026-07-09:
    /// if the user doesn't set one, the block's centre is the insertion point).
    pub(super) fn selection_centroid(&self) -> Vec2 {
        let (mut mnx, mut mny) = (f64::INFINITY, f64::INFINITY);
        let (mut mxx, mut mxy) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut any = false;
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get(i) {
                let (mn, mx) = d.geom.bbox();
                mnx = mnx.min(mn.x);
                mny = mny.min(mn.y);
                mxx = mxx.max(mx.x);
                mxy = mxy.max(mx.y);
                any = true;
            }
        }
        if any {
            Vec2::new(0.5 * (mnx + mxx), 0.5 * (mny + mxy))
        } else {
            Vec2::ZERO
        }
    }

    /// Combined bbox (min,max) of the current selection. Degenerate
    /// (0,0)-(0,0) when empty. Used as the stretch test region for a
    /// pickfirst selection that has no crossing window.
    pub(super) fn selection_bbox(&self) -> (Vec2, Vec2) {
        let mut mn = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut mx = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut any = false;
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get(i) {
                // BlockRef.bbox() is a degenerate placeholder (insert point)
                // in the kernel — it can't resolve the definition. Resolve
                // it here so a select-first stretch/move uses the block's
                // REAL extents (otherwise the test box is a single point and
                // nothing moves). See `resolved_blockref_bbox`.
                let (a, b) = match &d.geom {
                    Geom::BlockRef(br) => self.resolved_blockref_bbox(br),
                    g => g.bbox(),
                };
                mn.x = mn.x.min(a.x);
                mn.y = mn.y.min(a.y);
                mx.x = mx.x.max(b.x);
                mx.y = mx.y.max(b.y);
                any = true;
            }
        }
        if any {
            (mn, mx)
        } else {
            (Vec2::ZERO, Vec2::ZERO)
        }
    }

    pub(super) fn start_block_def(&mut self, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() {
            self.history.push("  ! block: name cannot be empty".into());
            return;
        }
        if self.doc.blocks.find(&name).is_some() {
            self.history.push(format!(
                "  ! block: '{}' already exists (no redefinition yet)",
                name
            ));
            return;
        }
        if self.selection.is_empty() {
            self.begin_selection(SelectMode::ForSelect);
            self.queued_op = QueuedOp::BlockDef(name.clone());
            self.set_prompt(format!(
                "block '{}': select dobjects, Enter to continue  [Esc=cancel]",
                name
            ));
        } else {
            self.set_prompt(format!(
                "block '{}' ({} dobject(s)): click BASE point  [Esc=cancel]",
                name,
                self.selection.len()
            ));
            self.block_def_state = BlockDefState::WaitingForBase { name };
        }
    }

    /// World-space wireframe polylines approximating a geom, for the Block
    /// dialog preview. Curves sample to short chords; Walls use their exact
    /// face polylines; BlockRefs recurse into their (transformed) contents;
    /// Text/Dimension fall back to their bbox rectangle. Hatch contributes
    /// its boundary loops. Good enough for a thumbnail, NOT a pick source.
    pub(super) fn preview_world_polylines(&self, g: &Geom) -> Vec<Vec<Vec2>> {
        use std::f64::consts::TAU;
        let n = 48usize;
        match g {
            Geom::Line(l) => vec![vec![l.a, l.b]],
            Geom::Polyline(p) => {
                let mut v: Vec<Vec2> = p.vertices.iter().map(|x| x.pos).collect();
                if p.closed {
                    if let Some(&f) = v.first() {
                        v.push(f);
                    }
                }
                vec![v]
            }
            Geom::Circle(c) => {
                let mut v = Vec::with_capacity(n + 1);
                for i in 0..=n {
                    let t = i as f64 / n as f64 * TAU;
                    v.push(Vec2::new(
                        c.center.x + c.radius * t.cos(),
                        c.center.y + c.radius * t.sin(),
                    ));
                }
                vec![v]
            }
            Geom::Arc(a) => {
                let mut v = Vec::with_capacity(n + 1);
                for i in 0..=n {
                    let t = a.start_angle + (i as f64 / n as f64) * a.sweep_angle;
                    v.push(Vec2::new(
                        a.center.x + a.radius * t.cos(),
                        a.center.y + a.radius * t.sin(),
                    ));
                }
                vec![v]
            }
            Geom::Ellipse(el) => {
                let mut v = Vec::with_capacity(n + 1);
                for i in 0..=n {
                    v.push(el.point_at(i as f64 / n as f64 * TAU));
                }
                vec![v]
            }
            Geom::EllipseArc(ea) => {
                let mut v = Vec::with_capacity(n + 1);
                for i in 0..=n {
                    let t = ea.start_param + (i as f64 / n as f64) * ea.sweep_param;
                    v.push(ea.ellipse.point_at(t));
                }
                vec![v]
            }
            Geom::Point(pt) => {
                let s = 0.5;
                vec![
                    vec![
                        Vec2::new(pt.location.x - s, pt.location.y),
                        Vec2::new(pt.location.x + s, pt.location.y),
                    ],
                    vec![
                        Vec2::new(pt.location.x, pt.location.y - s),
                        Vec2::new(pt.location.x, pt.location.y + s),
                    ],
                ]
            }
            Geom::Spline(s) => vec![s.tessellate(64)],
            Geom::Wall(w) => {
                if let Some((l, r)) = w.face_polylines(24) {
                    vec![l, r]
                } else {
                    vec![vec![w.start, w.end]]
                }
            }
            Geom::Hatch(h) => self.resolve_hatch_loops(h),
            Geom::Text(_) | Geom::Dimension(_) => {
                let (mn, mx) = g.bbox();
                vec![vec![
                    Vec2::new(mn.x, mn.y),
                    Vec2::new(mx.x, mn.y),
                    Vec2::new(mx.x, mx.y),
                    Vec2::new(mn.x, mx.y),
                    Vec2::new(mn.x, mn.y),
                ]]
            }
            Geom::BlockRef(br) => {
                let mut out = Vec::new();
                if let Some(blk) = self.doc.blocks.get(br.block) {
                    for cd in &blk.dobjects {
                        let wg = br.transform_geom(&cd.geom, blk.base);
                        out.extend(self.preview_world_polylines(&wg));
                    }
                }
                out
            }
            // Merged-in RUST-AutoRASM variants: render their bbox rectangle.
            _ => {
                let (mn, mx) = g.bbox();
                vec![vec![
                    Vec2::new(mn.x, mn.y),
                    Vec2::new(mx.x, mn.y),
                    Vec2::new(mx.x, mx.y),
                    Vec2::new(mn.x, mx.y),
                    Vec2::new(mn.x, mn.y),
                ]]
            }
        }
    }

    /// "Set parameters on block" dialog — opened after picking source +
    /// target blocks. Name each modifier vector + set its source/target
    /// value; Save makes the source block parametric.
    pub(super) fn render_param_name_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.param_name_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut do_save = false;
        let mut do_cancel = false;
        egui::Window::new("Set parameters on block")
            .id(egui::Id::new("param_name_dialog"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "base '{}'  →  variant '{}'",
                        dlg.source_name, dlg.target_name
                    ))
                    .strong(),
                );
                ui.small(
                    "Name each modifier vector (P) and set what value the \
                          source / target represent. Highlighted on the base block. \
                          Leave a name blank to skip a vector. Give two rows the \
                          SAME name to LINK them into one variable (asked once at insert).",
                );
                ui.add_space(4.0);
                egui::Grid::new("param_rows_grid")
                    .num_columns(4)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("P").strong());
                        ui.label(egui::RichText::new("Δx, Δy").strong());
                        ui.label(egui::RichText::new("name (variable)").strong());
                        ui.label(egui::RichText::new("original").strong());
                        ui.end_row();
                        for (i, r) in dlg.rows.iter_mut().enumerate() {
                            ui.label(format!("P{}", i + 1));
                            ui.monospace(format!("{:.1}, {:.1}", r.dx, r.dy));
                            ui.add(
                                egui::TextEdit::singleline(&mut r.name)
                                    .desired_width(110.0)
                                    .hint_text("width / thickness"),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut r.original)
                                    .desired_width(64.0)
                                    .hint_text("e.g. 1000"),
                            );
                            ui.end_row();
                        }
                    });
                ui.small(
                    "At insert the app asks \"set <name>\"; displacement = \
                          (entered value − original) along the vector.",
                );
                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            do_cancel = true;
                        }
                        if ui
                            .button(egui::RichText::new("Save parametric block").strong())
                            .clicked()
                        {
                            do_save = true;
                        }
                    });
                });
            });
        if do_save {
            self.save_block_params(dlg.source_id, &dlg.rows);
            self.blockdiff_overlay = None;
            return; // dialog closes
        }
        if !open || do_cancel {
            self.history.push("  set parameters: cancelled".into());
            return;
        }
        self.param_name_dialog = Some(dlg); // keep open
    }

    /// Open the isolated Block Editor on a block DEFINITION. Starts with no
    /// recorded parameters — the user demonstrates stretches inside the
    /// editor's own canvas (Save replaces the block's params). All editing is
    /// self-contained, so nothing touches the main canvas.
    pub(super) fn open_block_editor(&mut self, block_id: u32) {
        let Some(blk) = self.doc.blocks.get(block_id) else {
            self.history.push("  ! Block Editor: no such block".into());
            return;
        };
        let name = blk.name.clone();
        let base = blk.base;
        // Seed the editor with the block's EXISTING parameters so re-opening
        // ADDS to them. Save replaces the whole param set, so without this a
        // second editing session would silently drop the prior tasks (e.g.
        // adding 'frame' would forget 'opening'). Windows are stored base-
        // relative in the editor (`save_block_params` re-adds the base).
        let mut rows: Vec<ParamRow> = Vec::new();
        for p in &blk.params {
            for v in &p.vectors {
                let diag = (v.win_max - v.win_min).len();
                let mag = if diag > 1e-6 { diag * 0.5 } else { 100.0 };
                rows.push(ParamRow {
                    win_min: v.win_min - base,
                    win_max: v.win_max - base,
                    dir: v.dir,
                    magnitude: mag,
                    dx: v.dir.x * mag,
                    dy: v.dir.y * mag,
                    points: Vec::new(),
                    name: p.name.clone(),
                    original: format!("{}", p.original),
                    gain: v.gain,
                });
            }
        }
        let next_pid = rows.len() as u32 + 1;
        let seeded = rows.len();
        let cut_edges = blk.cut_edges.clone();
        self.block_editor = Some(BlockEditor {
            block_id,
            name: name.clone(),
            view_center: Vec2::ZERO,
            view_scale: 1.0,
            fitted: false,
            phase: EdPhase::Window,
            drag_start: None,
            rows,
            next_pid,
            active_name: String::new(),
            active_original: "0".into(),
            active_gain: 1.0,
            card: true,
            cut_edges,
            mark_cut: false,
        });
        self.history.push(format!(
            "  ◉ Block Editor: '{}' ({} existing vector(s)) — name a Task, Measure original, \
             drag a window then the direction; Save when done",
            name, seeded
        ));
    }

    /// Render the isolated **Block Editor** window: an embedded canvas showing
    /// the block (read-only) where the user demonstrates stretch gestures that
    /// become named parameters. Reads input ONLY from the editor canvas's
    /// response — the main canvas interaction is gated off while this is open
    /// (see `block_editor.is_some()` checks in the CentralPanel).
    pub(super) fn render_block_editor(&mut self, ctx: &egui::Context) {
        let Some(mut ed) = self.block_editor.take() else {
            return;
        };

        // Block gone (deleted elsewhere)? Bail.
        let Some(blk) = self.doc.blocks.get(ed.block_id) else {
            self.history
                .push("  Block Editor closed — block no longer exists".into());
            return;
        };
        let base = blk.base;
        // Pre-extract the block's geometry as world (definition-space)
        // polylines while &self is free, plus the bbox (seeded with base).
        let geoms: Vec<Geom> = blk.dobjects.iter().map(|d| d.geom.clone()).collect();
        // Definition-space dobjects for OBJECT SNAP inside the editor (END /
        // MID / CEN / QUA / INT on the block's own geometry).
        let snap_dobjects: Vec<DObject> = blk.dobjects.clone();
        let snap_set = {
            let mut s = SnapSet::defaults();
            s.int = true;
            s
        };
        // Per-dobject polylines (index = dobject index) for hit-testing the
        // cut-edge marker + drawing marked edges; plus a flat copy for the
        // read-only geometry + affected-vertex tests.
        let geom_polys: Vec<Vec<Vec<Vec2>>> = geoms
            .iter()
            .map(|g| self.preview_world_polylines(g))
            .collect();
        let polylines: Vec<Vec<Vec2>> = geom_polys.iter().flatten().cloned().collect();
        let mut bmin = base;
        let mut bmax = base;
        for pl in &polylines {
            for p in pl {
                bmin.x = bmin.x.min(p.x);
                bmin.y = bmin.y.min(p.y);
                bmax.x = bmax.x.max(p.x);
                bmax.y = bmax.y.max(p.y);
            }
        }

        let mut open = true;
        let mut do_save = false;
        let mut do_cancel = false;

        egui::Window::new(format!("Block Editor — {}", ed.name))
            .id(egui::Id::new("block_editor"))
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_size(egui::vec2(780.0, 660.0))
            .default_pos(egui::pos2(120.0, 60.0))
            .show(ctx, |ui| {
                let hint = match ed.phase {
                    EdPhase::Window => {
                        "① Drag a CROSSING WINDOW over the vertices that should move."
                    }
                    EdPhase::Displace { .. } => {
                        "② Now drag the DIRECTION it moves (amount is value-driven, not this drag)."
                    }
                    EdPhase::Measure => {
                        "⟷ Click TWO points to MEASURE this task's original distance."
                    }
                };
                ui.label(egui::RichText::new(hint).strong().size(14.0));
                ui.small(
                    "left-drag = gesture · right/middle-drag = pan · scroll = zoom · \
                          Esc = cancel current step",
                );

                // ---- task toolbar --------------------------------------
                // The active TASK new stretches join. Rows sharing a name are
                // ONE correlated variable (e.g. all parts of a door opening).
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label("Task:");
                    ui.add(
                        egui::TextEdit::singleline(&mut ed.active_name)
                            .desired_width(140.0)
                            .hint_text("e.g. opening / frame"),
                    );
                    if ui
                        .add(egui::Button::new("Measure original ⟷"))
                        .on_hover_text(
                            "Click two points; their distance becomes \
                                        this task's baseline value (e.g. 900).",
                        )
                        .clicked()
                    {
                        ed.phase = EdPhase::Measure;
                        ed.drag_start = None;
                    }
                    ui.label("orig:");
                    ui.add(egui::TextEdit::singleline(&mut ed.active_original).desired_width(56.0));
                    ui.label("gain:");
                    ui.add(
                        egui::DragValue::new(&mut ed.active_gain)
                            .update_while_editing(false)
                            .speed(0.05)
                            .range(-10.0..=10.0),
                    )
                    .on_hover_text(
                        "How much each new stretch moves per unit of \
                                        value. 1.0 = full; 0.5 = half (centered).",
                    );
                    ui.toggle_value(&mut ed.card, "CARD")
                        .on_hover_text("Lock stretch/measure to the nearest H or V axis.");
                    ui.toggle_value(
                        &mut ed.mark_cut,
                        format!("Mark cut edges ✂ ({})", ed.cut_edges.len()),
                    )
                    .on_hover_text(
                        "Click the block edges that BOUND the opening \
                                        (e.g. the two jambs). On insert, host geometry \
                                        between them is trimmed away.",
                    );
                });

                // ---- embedded canvas -----------------------------------
                let avail = ui.available_size();
                let canvas_h = (avail.y - 200.0).max(240.0);
                let (cresp, p) = ui
                    .allocate_painter(egui::vec2(avail.x, canvas_h), egui::Sense::click_and_drag());
                let rect = cresp.rect;
                p.rect_filled(rect, 4.0, egui::Color32::from_rgb(16, 20, 26));
                p.rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 95)),
                );

                // First-open fit-to-view (and the "Fit view" button resets it).
                if !ed.fitted {
                    let size = bmax - bmin;
                    let sx = (rect.width() as f64 - 48.0) / size.x.max(1e-9);
                    let sy = (rect.height() as f64 - 48.0) / size.y.max(1e-9);
                    ed.view_scale = sx.min(sy).clamp(1e-6, 1e6);
                    ed.view_center = (bmin + bmax) * 0.5;
                    ed.fitted = true;
                }

                // Pan (right/middle drag) — raw delta, no transform needed.
                if cresp.dragged_by(egui::PointerButton::Middle)
                    || cresp.dragged_by(egui::PointerButton::Secondary)
                {
                    let d = cresp.drag_delta();
                    ed.view_center.x -= d.x as f64 / ed.view_scale;
                    ed.view_center.y += d.y as f64 / ed.view_scale;
                }
                // Zoom about the cursor.
                let ctr = rect.center();
                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll != 0.0 {
                    if let Some(c) = cresp.hover_pos() {
                        let vc0 = ed.view_center;
                        let vs0 = ed.view_scale;
                        let before = Vec2::new(
                            vc0.x + (c.x - ctr.x) as f64 / vs0,
                            vc0.y - (c.y - ctr.y) as f64 / vs0,
                        );
                        let f = (scroll as f64 * 0.0015).exp();
                        ed.view_scale = (vs0 * f).clamp(1e-6, 1e6);
                        let vs1 = ed.view_scale;
                        let after = Vec2::new(
                            vc0.x + (c.x - ctr.x) as f64 / vs1,
                            vc0.y - (c.y - ctr.y) as f64 / vs1,
                        );
                        ed.view_center.x += before.x - after.x;
                        ed.view_center.y += before.y - after.y;
                    }
                }

                // Final transform for this frame (after pan/zoom).
                let vc = ed.view_center;
                let vs = ed.view_scale;
                let w2s = |w: Vec2| {
                    egui::pos2(
                        ctr.x + ((w.x - vc.x) * vs) as f32,
                        ctr.y - ((w.y - vc.y) * vs) as f32,
                    )
                };
                let s2w = |s: egui::Pos2| {
                    Vec2::new(
                        vc.x + (s.x - ctr.x) as f64 / vs,
                        vc.y - (s.y - ctr.y) as f64 / vs,
                    )
                };

                // Object snap on the block's own geometry. Returns the snapped
                // world point (or the raw one) + whether a snap was hit.
                let tol_world = 12.0 / vs;
                let snap = |raw: Vec2| -> (Vec2, bool) {
                    match find_snap(raw, tol_world, snap_set, None, None, &snap_dobjects, None) {
                        Some(h) => (h.point, true),
                        None => (raw, false),
                    }
                };

                let pc = p.with_clip_rect(rect);

                // Block geometry (read-only).
                let geo_col = egui::Color32::from_rgb(150, 200, 235);
                for pl in &polylines {
                    if pl.len() >= 2 {
                        let pts: Vec<egui::Pos2> = pl.iter().map(|w| w2s(*w)).collect();
                        pc.add(egui::Shape::line(pts, egui::Stroke::new(1.4, geo_col)));
                    }
                }
                // Marked CUT EDGES (door/window jambs) — drawn thick orange.
                let cut_col = egui::Color32::from_rgb(255, 140, 40);
                for &ci in &ed.cut_edges {
                    if let Some(polys) = geom_polys.get(ci) {
                        for pl in polys {
                            if pl.len() >= 2 {
                                let pts: Vec<egui::Pos2> = pl.iter().map(|w| w2s(*w)).collect();
                                pc.add(egui::Shape::line(pts, egui::Stroke::new(3.0, cut_col)));
                            }
                        }
                    }
                }
                // Base point marker.
                let bs = w2s(base);
                let bcol = egui::Color32::from_rgb(255, 180, 60);
                pc.line_segment(
                    [egui::pos2(bs.x - 8.0, bs.y), egui::pos2(bs.x + 8.0, bs.y)],
                    egui::Stroke::new(1.2, bcol),
                );
                pc.line_segment(
                    [egui::pos2(bs.x, bs.y - 8.0), egui::pos2(bs.x, bs.y + 8.0)],
                    egui::Stroke::new(1.2, bcol),
                );
                pc.circle_stroke(bs, 4.0, egui::Stroke::new(1.2, bcol));

                // Already-recorded parameters (window + displacement arrow).
                for (k, r) in ed.rows.iter().enumerate() {
                    let col = editor_param_color(k);
                    let wmin = r.win_min + base;
                    let wmax = r.win_max + base;
                    let a = w2s(Vec2::new(wmin.x.min(wmax.x), wmin.y.min(wmax.y)));
                    let b = w2s(Vec2::new(wmin.x.max(wmax.x), wmin.y.max(wmax.y)));
                    pc.rect_stroke(
                        egui::Rect::from_two_pos(a, b),
                        0.0,
                        egui::Stroke::new(1.0, col),
                    );
                    let c = (wmin + wmax) * 0.5;
                    let tip = c + r.dir * r.magnitude;
                    draw_editor_arrow(&pc, w2s(c), w2s(tip), col);
                }

                // Snap marker at the hovered geometry point.
                if let Some(hp) = cresp.hover_pos() {
                    let (sp, hit) = snap(s2w(hp));
                    if hit {
                        let m = w2s(sp);
                        pc.rect_stroke(
                            egui::Rect::from_center_size(m, egui::vec2(11.0, 11.0)),
                            0.0,
                            egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 255, 120)),
                        );
                    }
                }

                // ---- gesture handling (canvas-local input only) --------
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    ed.phase = EdPhase::Window;
                    ed.drag_start = None;
                    ed.mark_cut = false;
                }
                // MARK-CUT-EDGES mode: a click toggles the nearest edge's mark
                // (no stretch gesture while marking).
                if ed.mark_cut {
                    if cresp.clicked() {
                        if let Some(pp) = cresp.interact_pointer_pos() {
                            let w = s2w(pp);
                            let mut best: Option<(usize, f64)> = None;
                            for (i, polys) in geom_polys.iter().enumerate() {
                                let mut dmin = f64::INFINITY;
                                for pl in polys {
                                    for seg in pl.windows(2) {
                                        dmin = dmin.min(point_seg_dist(w, seg[0], seg[1]));
                                    }
                                }
                                if best.map_or(true, |(_, bd)| dmin < bd) {
                                    best = Some((i, dmin));
                                }
                            }
                            if let Some((i, d)) = best {
                                if d <= (15.0 / vs) {
                                    if let Some(pos) = ed.cut_edges.iter().position(|&x| x == i) {
                                        ed.cut_edges.remove(pos);
                                    } else {
                                        ed.cut_edges.push(i);
                                    }
                                }
                            }
                        }
                    }
                }
                let gesturing = !ed.mark_cut;
                if gesturing && cresp.drag_started_by(egui::PointerButton::Primary) {
                    if let Some(pp) = cresp.interact_pointer_pos() {
                        ed.drag_start = Some(snap(s2w(pp)).0);
                    }
                }
                let cur = cresp.interact_pointer_pos().map(|pp| snap(s2w(pp)).0);
                let prim_drag = gesturing && cresp.dragged_by(egui::PointerButton::Primary);
                let prim_stop = gesturing && cresp.drag_stopped_by(egui::PointerButton::Primary);

                match ed.phase {
                    EdPhase::Window => {
                        if prim_drag {
                            if let (Some(st), Some(cu)) = (ed.drag_start, cur) {
                                let a = w2s(st);
                                let b = w2s(cu);
                                pc.rect_stroke(
                                    egui::Rect::from_two_pos(a, b),
                                    0.0,
                                    egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 220, 140)),
                                );
                            }
                        }
                        if prim_stop {
                            if let (Some(st), Some(cu)) = (ed.drag_start, cur) {
                                let wmin = Vec2::new(st.x.min(cu.x), st.y.min(cu.y));
                                let wmax = Vec2::new(st.x.max(cu.x), st.y.max(cu.y));
                                if (wmax.x - wmin.x) > 1e-6 && (wmax.y - wmin.y) > 1e-6 {
                                    ed.phase = EdPhase::Displace {
                                        win_min: wmin,
                                        win_max: wmax,
                                    };
                                }
                            }
                            ed.drag_start = None;
                        }
                    }
                    EdPhase::Displace { win_min, win_max } => {
                        let inside = |q: Vec2| {
                            q.x >= win_min.x
                                && q.x <= win_max.x
                                && q.y >= win_min.y
                                && q.y <= win_max.y
                        };
                        // Chosen window + affected vertices.
                        let a = w2s(win_min);
                        let b = w2s(win_max);
                        pc.rect_stroke(
                            egui::Rect::from_two_pos(a, b),
                            0.0,
                            egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 220, 140)),
                        );
                        for pl in &polylines {
                            for q in pl {
                                if inside(*q) {
                                    pc.circle_filled(
                                        w2s(*q),
                                        3.0,
                                        egui::Color32::from_rgb(255, 120, 120),
                                    );
                                }
                            }
                        }
                        // Live displacement preview (CARD-locked if on).
                        if prim_drag {
                            if let (Some(st), Some(cu)) = (ed.drag_start, cur) {
                                let mut v = cu - st;
                                if ed.card {
                                    v = card_lock_vec(v);
                                }
                                pc.line_segment(
                                    [w2s(st), w2s(st + v)],
                                    egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 220, 120)),
                                );
                                for pl in &polylines {
                                    let pts: Vec<egui::Pos2> = pl
                                        .iter()
                                        .map(|q| {
                                            let nq = if inside(*q) { *q + v } else { *q };
                                            w2s(nq)
                                        })
                                        .collect();
                                    if pts.len() >= 2 {
                                        pc.add(egui::Shape::line(
                                            pts,
                                            egui::Stroke::new(
                                                1.0,
                                                egui::Color32::from_rgb(255, 220, 120),
                                            ),
                                        ));
                                    }
                                }
                            }
                        }
                        if prim_stop {
                            if let (Some(st), Some(cu)) = (ed.drag_start, cur) {
                                let mut v = cu - st;
                                if ed.card {
                                    v = card_lock_vec(v);
                                }
                                if v.len() > 1e-6 {
                                    let dir = v / v.len();
                                    let mut pts = Vec::new();
                                    for pl in &polylines {
                                        for q in pl {
                                            if inside(*q) {
                                                pts.push(*q - base);
                                            }
                                        }
                                    }
                                    // Join the ACTIVE task (shared name = linked,
                                    // correlated vectors). Empty name → own P{n}.
                                    let name = if ed.active_name.trim().is_empty() {
                                        let pid = ed.next_pid;
                                        ed.next_pid += 1;
                                        format!("P{}", pid)
                                    } else {
                                        ed.active_name.trim().to_string()
                                    };
                                    ed.rows.push(ParamRow {
                                        win_min: win_min - base,
                                        win_max: win_max - base,
                                        dir,
                                        magnitude: v.len(),
                                        dx: v.x,
                                        dy: v.y,
                                        points: pts,
                                        name,
                                        original: ed.active_original.clone(),
                                        gain: ed.active_gain,
                                    });
                                }
                            }
                            ed.phase = EdPhase::Window;
                            ed.drag_start = None;
                        }
                    }
                    EdPhase::Measure => {
                        // Two CLICKS → distance becomes the active task's
                        // original. First click sets A (held in drag_start),
                        // second sets B. CARD locks the measured direction.
                        if let Some(a0) = ed.drag_start {
                            let asp = w2s(a0);
                            let mcol = egui::Color32::from_rgb(120, 220, 255);
                            pc.circle_filled(asp, 4.0, mcol);
                            if let Some(cu) = cur {
                                let b = if ed.card {
                                    a0 + card_lock_vec(cu - a0)
                                } else {
                                    cu
                                };
                                pc.line_segment([asp, w2s(b)], egui::Stroke::new(1.2, mcol));
                                pc.text(
                                    w2s(b) + egui::vec2(8.0, -8.0),
                                    egui::Align2::LEFT_BOTTOM,
                                    format!("{:.1}", (b - a0).len()),
                                    crate::theme::typ::data_value(),
                                    mcol,
                                );
                            }
                        }
                        if gesturing && cresp.clicked() {
                            if let Some(pp) = cresp.interact_pointer_pos() {
                                let p = snap(s2w(pp)).0;
                                match ed.drag_start {
                                    None => ed.drag_start = Some(p),
                                    Some(a0) => {
                                        let b = if ed.card {
                                            a0 + card_lock_vec(p - a0)
                                        } else {
                                            p
                                        };
                                        let dist = (b - a0).len();
                                        ed.active_original = format!("{:.1}", dist);
                                        // Propagate to existing rows of this task.
                                        let nm = ed.active_name.trim().to_string();
                                        if !nm.is_empty() {
                                            let val = ed.active_original.clone();
                                            for r in ed.rows.iter_mut().filter(|r| r.name == nm) {
                                                r.original = val.clone();
                                            }
                                        }
                                        ed.drag_start = None;
                                        ed.phase = EdPhase::Window;
                                    }
                                }
                            }
                        }
                    }
                }

                // ---- parameters list -----------------------------------
                // Rows sharing a NAME are one task (correlated vectors). The
                // colour swatch is keyed by name so a task's vectors match.
                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    let tasks = {
                        let mut names: Vec<&str> =
                            ed.rows.iter().map(|r| r.name.as_str()).collect();
                        names.sort_unstable();
                        names.dedup();
                        names.len()
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "Tasks: {} · vectors: {}",
                            tasks,
                            ed.rows.len()
                        ))
                        .strong(),
                    );
                    if ui.button("Fit view").clicked() {
                        ed.fitted = false;
                    }
                });
                let mut del: Option<usize> = None;
                // Stable per-NAME colour index so a task's vectors share a hue.
                let mut name_order: Vec<String> = Vec::new();
                for r in &ed.rows {
                    if !name_order.iter().any(|n| n == &r.name) {
                        name_order.push(r.name.clone());
                    }
                }
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for (k, r) in ed.rows.iter_mut().enumerate() {
                            let ci = name_order.iter().position(|n| n == &r.name).unwrap_or(k);
                            ui.horizontal(|ui| {
                                ui.colored_label(editor_param_color(ci), "■");
                                ui.add(
                                    egui::TextEdit::singleline(&mut r.name)
                                        .desired_width(110.0)
                                        .hint_text("task e.g. opening"),
                                );
                                ui.label(format!("Δ({:.1},{:.1})", r.dx, r.dy));
                                ui.label("orig:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut r.original).desired_width(52.0),
                                );
                                ui.label("gain:");
                                ui.add(
                                    egui::DragValue::new(&mut r.gain)
                                        .update_while_editing(false)
                                        .speed(0.05)
                                        .range(-10.0..=10.0),
                                );
                                if ui.small_button("✕").clicked() {
                                    del = Some(k);
                                }
                            });
                        }
                    });
                if let Some(k) = del {
                    ed.rows.remove(k);
                }
                if ed.rows.is_empty() {
                    ui.small(
                        "No parameters yet — drag a window over the geometry, \
                              then drag where it should move.",
                    );
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            do_cancel = true;
                        }
                        if ui
                            .add(egui::Button::new(egui::RichText::new("Save").strong()))
                            .clicked()
                        {
                            do_save = true;
                        }
                        ui.label(
                            egui::RichText::new(
                                "name each parameter, set its original value, then",
                            )
                            .weak(),
                        );
                    });
                });
            });

        // ---- post-frame -------------------------------------------------
        if do_save {
            // Persist the opening CUT EDGES (door/window jambs) onto the block
            // first — these save even when there are no parametric vectors.
            if let Some(blk) = self.doc.blocks.get_mut(ed.block_id) {
                blk.cut_edges = ed.cut_edges.clone();
            }
            let n_cut = ed.cut_edges.len();
            let rows = std::mem::take(&mut ed.rows);
            self.save_block_params(ed.block_id, &rows);
            if n_cut > 0 {
                self.history.push(format!(
                    "  ✔ block '{}' has {} cut edge(s) — host geometry is trimmed on insert",
                    ed.name, n_cut
                ));
            }
            return; // editor closes
        }
        if do_cancel || !open {
            self.history
                .push("  Block Editor closed (no changes saved)".into());
            return;
        }
        self.block_editor = Some(ed); // keep open
    }

    /// Block dialog — bare `block` / Draw menu. Full define-a-block form:
    /// name, live PREVIEW of the selection, "Select objects" (round-trips
    /// to canvas), insertion point (typed X/Y or "Pick ⊕"), instance COLOR
    /// (ACI wheel), and the SMART block flag. Plus an Existing list with
    /// one-click Insert.
    pub(super) fn render_block_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.block_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut do_create = false;
        let mut do_cancel = false;
        let mut do_select = false; // Select objects (round-trip)
        let mut do_pick_base = false; // Pick insertion point (round-trip)
        let mut do_pick_color = false; // open ACI wheel
        let mut clear_color = false; // reset to inherit/ByLayer
        let mut insert_id: Option<u32> = None;
        let mut do_compare = false; // diff the two chosen blocks
        let mut edit_id: Option<u32> = None; // open Block Editor for this block

        let existing: Vec<(u32, String, usize, bool)> = self
            .doc
            .blocks
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (i as u32, b.name.clone(), b.dobjects.len(), b.smart))
            .collect();
        let sel_count = self.selection.len();

        // Pre-extract preview wireframe (world polylines + resolved color)
        // and its combined bbox — done here where `&self` is available.
        let mut preview: Vec<(Vec<Vec2>, egui::Color32)> = Vec::new();
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get(i) {
                let (r, g, b) = resolve_color(
                    d.style.color,
                    d.style.layer,
                    &self.doc.layers,
                    &self.doc.truecolors,
                );
                let col = egui::Color32::from_rgb(r, g, b);
                for pl in self.preview_world_polylines(&d.geom) {
                    preview.push((pl, col));
                }
            }
        }
        let mut pmin = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut pmax = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (pl, _) in &preview {
            for w in pl {
                pmin.x = pmin.x.min(w.x);
                pmin.y = pmin.y.min(w.y);
                pmax.x = pmax.x.max(w.x);
                pmax.y = pmax.y.max(w.y);
            }
        }
        let has_preview = pmax.x >= pmin.x && pmax.y >= pmin.y;

        let swatch = match dialog.color_aci {
            Some(aci) => {
                let (r, g, b) = aci_palette(aci);
                egui::Color32::from_rgb(r, g, b)
            }
            None => egui::Color32::from_rgb(90, 100, 115),
        };

        egui::Window::new("Block")
            .id(egui::Id::new("block_dialog"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_size(egui::vec2(360.0, 520.0))
            .default_pos(egui::pos2(240.0, 90.0))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Create new block").strong());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Name");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut dialog.name)
                            .desired_width(220.0)
                            .hint_text("CHAIR, DOOR-90, …"),
                    );
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        do_create = true;
                    }
                });

                // ---- Preview --------------------------------------------
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Preview").strong());
                let (resp, painter) =
                    ui.allocate_painter(egui::vec2(320.0, 130.0), egui::Sense::hover());
                let pr = resp.rect;
                painter.rect_filled(pr, 4.0, egui::Color32::from_rgb(18, 22, 28));
                painter.rect_stroke(
                    pr,
                    4.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 95)),
                );
                if has_preview && !preview.is_empty() {
                    let size = pmax - pmin;
                    let sx = (pr.width() as f64 - 16.0) / size.x.max(1e-9);
                    let sy = (pr.height() as f64 - 16.0) / size.y.max(1e-9);
                    let s = sx.min(sy); // uniform fit, padded
                    let cw = (pmin + pmax) * 0.5;
                    let cx = pr.center().x;
                    let cy = pr.center().y;
                    let map = |w: Vec2| {
                        egui::pos2(
                            cx + ((w.x - cw.x) * s) as f32,
                            cy - ((w.y - cw.y) * s) as f32,
                        )
                    };
                    let clip = painter.with_clip_rect(pr);
                    for (pl, col) in &preview {
                        if pl.len() >= 2 {
                            let pts: Vec<egui::Pos2> = pl.iter().map(|w| map(*w)).collect();
                            clip.add(egui::Shape::line(pts, egui::Stroke::new(1.2, *col)));
                        }
                    }
                } else {
                    painter.text(
                        pr.center(),
                        egui::Align2::CENTER_CENTER,
                        "(no objects selected)",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_rgb(130, 140, 155),
                    );
                }

                // ---- Select objects -------------------------------------
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Select objects…").clicked() {
                        do_select = true;
                    }
                    ui.label(format!("{} selected", sel_count));
                });

                // ---- Insertion point ------------------------------------
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Insertion point").strong());
                ui.horizontal(|ui| {
                    ui.label("X");
                    ui.add(egui::TextEdit::singleline(&mut dialog.base_x).desired_width(80.0));
                    ui.label("Y");
                    ui.add(egui::TextEdit::singleline(&mut dialog.base_y).desired_width(80.0));
                    if ui.button("Pick ⊕").clicked() {
                        do_pick_base = true;
                    }
                });

                // ---- Color ----------------------------------------------
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Color").strong());
                    let (rect, sresp) =
                        ui.allocate_exact_size(egui::vec2(28.0, 16.0), egui::Sense::click());
                    ui.painter().rect_filled(rect, 2.0, swatch);
                    ui.painter().rect_stroke(
                        rect,
                        2.0,
                        egui::Stroke::new(1.0, egui::Color32::from_gray(160)),
                    );
                    if sresp.clicked() {
                        do_pick_color = true;
                    }
                    ui.label(match dialog.color_aci {
                        Some(aci) => format!("ACI {}", aci),
                        None => "inherit (ByLayer)".to_string(),
                    });
                    if ui.button("ACI…").clicked() {
                        do_pick_color = true;
                    }
                    if dialog.color_aci.is_some() && ui.button("inherit").clicked() {
                        clear_color = true;
                    }
                });

                // ---- Smart block ----------------------------------------
                ui.add_space(4.0);
                ui.checkbox(&mut dialog.smart, "Smart block").on_hover_text(
                    "Mark this definition as a smart block \
                        (re-derive algorithm wired later).",
                );

                // ---- OK / Cancel ----------------------------------------
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            do_cancel = true;
                        }
                        if ui.button("OK").clicked() {
                            do_create = true;
                        }
                    });
                });

                if !existing.is_empty() {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(egui::RichText::new("Existing blocks").strong());
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (id, name, n, smart) in &existing {
                                ui.horizontal(|ui| {
                                    ui.label(format!(
                                        "{}{}  ({} dobj)",
                                        name,
                                        if *smart { " ⚙" } else { "" },
                                        n
                                    ));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("Insert").clicked() {
                                                insert_id = Some(*id);
                                            }
                                            if ui
                                                .small_button("Edit ▶")
                                                .on_hover_text(
                                                    "Open the Block Editor — \
                                                turn this block parametric WITHOUT \
                                                touching the main screen.",
                                                )
                                                .clicked()
                                            {
                                                edit_id = Some(*id);
                                            }
                                        },
                                    );
                                });
                            }
                        });
                }

                // ---- Set parameters on block ----------------------------
                // Parametrizing now happens entirely inside the isolated Block
                // Editor (the "Edit ▶" button per block above) — no exploding
                // to the main canvas. COMPARE still needs two distinct block
                // definitions, so it stays gated below.
                if !existing.is_empty() {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(egui::RichText::new("Set parameters on block").strong());
                    ui.small(
                        "Click ‘Edit ▶’ next to a block above to open the \
                              Block Editor — demonstrate each stretch in its own canvas, \
                              name it, then Save. All editing stays inside the dialog.",
                    );
                    if existing.len() >= 2 {
                        ui.add_space(2.0);
                        ui.small("…or COMPARE two finished blocks (lossy for arcs/rotations):");
                        if ui
                            .add(egui::Button::new("Compare source & target on screen ▶"))
                            .on_hover_text(
                                "Click the SOURCE block then the TARGET block; the \
                                            diff is extracted (can include side-effect vectors).",
                            )
                            .clicked()
                        {
                            do_compare = true;
                        }
                    }
                }
            });

        // ---- post-frame actions ------------------------------------------
        if let Some(id) = edit_id {
            // Close the dialog and open the isolated Block Editor on this
            // block definition — all parametric editing happens in there.
            self.open_block_editor(id);
            return;
        }
        if do_compare {
            // Close the dialog and start the on-screen 2-block pick.
            self.begin_block_diff_pick();
            return;
        }
        if let Some(id) = insert_id {
            let name = existing
                .iter()
                .find(|(i, _, _, _)| *i == id)
                .map(|(_, n, _, _)| n.clone())
                .unwrap_or_default();
            self.insert_state = InsertState::WaitingForPoint { block: id };
            self.set_prompt(format!(
                "insert '{}': click insertion point  [Esc=cancel]",
                name
            ));
            return; // dialog closes; insert click takes over
        }
        if clear_color {
            dialog.color_aci = None;
        }
        if do_pick_color {
            // Open the ACI wheel; it writes back into block_dialog.color_aci,
            // so the dialog MUST be restored first.
            self.aci_pick_request = Some(AciPickRequest::BlockForm);
            self.block_dialog = Some(dialog);
            return;
        }
        if do_select {
            // Park the dialog, run a fresh selection session; BlockReopen
            // restores the dialog when the user presses Enter.
            self.block_dialog_stash = Some(dialog);
            self.begin_selection(SelectMode::ForSelect);
            self.queued_op = QueuedOp::BlockReopen;
            self.set_prompt("block: select objects, Enter to return to the dialog  [Esc=cancel]");
            return;
        }
        if do_pick_base {
            // Park the dialog, capture one canvas click as the base point.
            self.block_dialog_stash = Some(dialog);
            self.block_dialog_pick_base = true;
            self.set_prompt("block: click the insertion point  [Esc=cancel]");
            return;
        }
        if !open || do_cancel {
            self.history.push("  block: cancelled".into());
            return;
        }
        if do_create {
            let name = dialog.name.trim().to_string();
            if name.is_empty() {
                self.history.push("  ! block: name cannot be empty".into());
                self.block_dialog = Some(dialog);
            } else if self.doc.blocks.find(&name).is_some() {
                self.history.push(format!(
                    "  ! block: '{}' already exists (no redefinition yet)",
                    name
                ));
                self.block_dialog = Some(dialog);
            } else if self.selection.is_empty() {
                self.history
                    .push("  ! block: select objects first (Select objects…)".into());
                self.block_dialog = Some(dialog);
            } else {
                let base = match dialog.base_point(&self.calc) {
                    Ok(p) => p,
                    Err(e) => {
                        // Expression error — keep the dialog open, keep the
                        // typed text, show why.
                        self.history.push(format!("  ! {e}"));
                        self.block_dialog = Some(dialog);
                        return;
                    }
                };
                let smart = dialog.smart;
                self.apply_block_create(&name, base, dialog.color_aci, smart);
                // Smart block → jump STRAIGHT into the isolated Block Editor to
                // define the parametric modifier vectors (user request): ticking
                // "Smart block" + OK opens the editor on the new definition.
                if smart {
                    if let Some(id) = self.doc.blocks.find(&name) {
                        self.open_block_editor(id);
                    }
                }
                // success → dialog closes (not restored)
            }
            return;
        }
        self.block_dialog = Some(dialog);
    }

    /// The **Insert Block** dialog: pick a block, set scale + rotation +
    /// insertion point (typed or picked on screen), then place it. Bare
    /// `insert` opens it. Parked (not drawn) while "Pick ⊕" waits for a click.
    pub(super) fn render_insert_dialog(&mut self, ctx: &egui::Context) {
        // Parked while an on-canvas pick (insertion point or a param distance) runs.
        if self.insert_dialog_pick || self.insert_param_pick.is_some() {
            return;
        }
        let Some(mut dlg) = self.insert_dialog.take() else {
            return;
        };
        let blocks: Vec<(u32, String)> = self
            .doc
            .blocks
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (i as u32, b.name.clone()))
            .collect();
        if blocks.is_empty() {
            return;
        }
        // Sync the parametric value fields to the SELECTED block's definition:
        // a smart block with N params shows N value boxes; a plain block none.
        let want: Vec<(String, f64)> = dlg
            .block
            .and_then(|id| self.doc.blocks.get(id))
            .map(|b| {
                b.params
                    .iter()
                    .map(|p| (p.name.clone(), p.original))
                    .collect()
            })
            .unwrap_or_default();
        let same_names = dlg.params.len() == want.len()
            && dlg
                .params
                .iter()
                .zip(&want)
                .all(|((n, _), (wn, _))| n == wn);
        if !same_names {
            dlg.params = want
                .iter()
                .map(|(n, v)| (n.clone(), format!("{v}")))
                .collect();
        }
        let mut open = true;
        let mut do_insert = false;
        let mut do_cancel = false;
        let mut pick_param: Option<usize> = None;
        egui::Window::new("Insert Block")
            .id(egui::Id::new("insert_dialog"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(300.0);
                ui.horizontal(|ui| {
                    ui.label("Block");
                    let cur = dlg
                        .block
                        .and_then(|id| blocks.iter().find(|(i, _)| *i == id))
                        .map(|(_, n)| n.clone())
                        .unwrap_or_else(|| "(pick)".into());
                    egui::ComboBox::from_id_source("insert_block_pick")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for (id, name) in &blocks {
                                ui.selectable_value(&mut dlg.block, Some(*id), name);
                            }
                        });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Scale");
                    ui.add(egui::TextEdit::singleline(&mut dlg.scale).desired_width(70.0));
                    ui.add_space(8.0);
                    ui.label("Rotation°");
                    ui.add(egui::TextEdit::singleline(&mut dlg.rotation).desired_width(70.0));
                });
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new("Insertion point: click on the drawing after Insert.")
                        .small()
                        .weak(),
                );
                // ---- Smart-block parameters (the "vector" values) ----------
                if !dlg.params.is_empty() {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.label(egui::RichText::new("Parameters  (smart block)").strong());
                    ui.label(
                        egui::RichText::new("Set each vector value; the block deforms on insert.")
                            .small()
                            .weak(),
                    );
                    for (k, (name, val)) in dlg.params.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(name.as_str()).monospace());
                            ui.add(egui::TextEdit::singleline(val).desired_width(90.0));
                            if ui
                                .button("↔")
                                .on_hover_text(
                                    "Pick two points on the drawing — \
                                    their distance becomes this value",
                                )
                                .clicked()
                            {
                                pick_param = Some(k);
                            }
                        });
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            do_cancel = true;
                        }
                        if ui
                            .add_enabled(dlg.block.is_some(), egui::Button::new("Insert"))
                            .clicked()
                        {
                            do_insert = true;
                        }
                    });
                });
            });
        if let Some(k) = pick_param {
            // Park the dialog (hidden) and pick two points; their distance fills
            // this parameter's value box.
            let name = dlg
                .params
                .get(k)
                .map(|(n, _)| n.clone())
                .unwrap_or_default();
            self.insert_dialog = Some(dlg);
            self.insert_param_pick = Some((k, None));
            self.set_prompt(format!(
                "insert {name}: click the FIRST point of the distance  [Esc=cancel]"
            ));
            return;
        }
        if do_insert {
            if let Some(id) = dlg.block {
                let mut pv = [0.0; cad_kernel::MAX_BLOCK_PARAMS];
                for (k, (_, txt)) in dlg.params.iter().enumerate() {
                    if k < cad_kernel::MAX_BLOCK_PARAMS {
                        match self.eval_field(txt) {
                            Ok(v) => pv[k] = v,
                            Err(e) => {
                                // Expression error — keep the dialog open.
                                self.history.push(format!("  ! {e}"));
                                self.insert_dialog = Some(dlg);
                                return;
                            }
                        }
                    }
                }
                let scale = match dlg.scale_f(&self.calc) {
                    Ok(v) => v,
                    Err(e) => {
                        self.history.push(format!("  ! {e}"));
                        self.insert_dialog = Some(dlg);
                        return;
                    }
                };
                let rotation = match dlg.rot_rad(&self.calc) {
                    Ok(v) => v,
                    Err(e) => {
                        self.history.push(format!("  ! {e}"));
                        self.insert_dialog = Some(dlg);
                        return;
                    }
                };
                // Do NOT place yet — insert ALWAYS follows a click. Arm the
                // configured block; the next canvas click sets its insertion
                // point (with a live shade preview following the cursor).
                self.pending_insert = Some(PendingInsert {
                    block: id,
                    scale,
                    rotation,
                    param_values: pv,
                });
                self.insert_state = InsertState::WaitingForPoint { block: id };
                self.set_prompt("insert: click the insertion point on the drawing  [Esc=cancel]");
            }
            return; // dialog closes; the click places it
        }
        if !open || do_cancel {
            return; // Cancel / × — dialog closes
        }
        self.insert_dialog = Some(dlg); // keep open across frames
    }

    /// Place a block instance at `insert` with uniform `scale`, `rotation`
    /// (radians) and explicit `param_values` — the Insert-dialog placement path.
    /// ALWAYS places (unlike `apply_insert`, whose parametric branch waits for a
    /// command-line prompt), so a smart block appears immediately with its
    /// vector values applied.
    pub(super) fn place_block_full(
        &mut self,
        block: u32,
        insert: Vec2,
        scale: f64,
        rotation: f64,
        param_values: [f64; cad_kernel::MAX_BLOCK_PARAMS],
    ) {
        self.snapshot_doc();
        let s = if scale.abs() < 1e-6 { 1.0 } else { scale.abs() };
        let br = cad_kernel::BlockRef {
            block,
            insert,
            scale: s,
            scale_y: s,
            rotation,
            mirror_x: false,
            param_values,
            attr_values: Vec::new(),
        };
        self.add_dobject(Geom::BlockRef(br.clone()), "insert");
        self.apply_block_cut(br);
        self.index_dirty = true;
        self.touch_view();
        self.history.push(format!(
            "  + inserted block #{} at ({:.2}, {:.2})  s={:.3}  rot={:.1}°",
            block,
            insert.x,
            insert.y,
            s,
            rotation.to_degrees()
        ));
    }

    /// `block <name>` final step — store the definition (contents = the
    /// selected dobjects, cloned verbatim; coordinates stay as-is and
    /// `base` is what the instance transform carries), then replace the
    /// originals with ONE instance at insert == base so the drawing is
    /// visually unchanged (AutoCAD BLOCK behavior).
    /// `color_aci`: `Some(aci)` sets the instance color explicitly;
    /// `None` inherits the first selected dobject's style (layer/color).
    /// `smart` marks the definition as a smart block.
    pub(super) fn apply_block_create(&mut self, name: &str, base: Vec2, color_aci: Option<u8>, smart: bool) {
        if self.selection.is_empty() {
            self.history.push("  ! block: empty selection".into());
            return;
        }
        self.snapshot_doc();
        let mut sel: Vec<usize> = self.selection.clone();
        sel.sort_unstable();
        let mut contents: Vec<DObject> = Vec::new();
        for &i in &sel {
            if let Some(d) = self.doc.dobjects.get(i) {
                contents.push(d.clone());
            }
        }
        let n = contents.len();
        // Instance inherits the FIRST selected dobject's style so the
        // blockref lands on a sensible layer/color — then the dialog's
        // explicit color (if any) overrides just the color.
        let inst_style = contents.first().map(|d| d.style);
        let id = self.doc.blocks.add(cad_kernel::Block {
            name: name.to_string(),
            base,
            dobjects: contents,
            smart,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        for &i in sel.iter().rev() {
            self.doc.dobjects.remove(i);
        }
        self.selection.clear();
        let geom = Geom::BlockRef(cad_kernel::BlockRef {
            block: id,
            insert: base,
            scale: 1.0,
            scale_y: 1.0,
            rotation: 0.0,
            mirror_x: false,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
            attr_values: Vec::new(),
        });
        let mut d = match inst_style {
            Some(st) => DObject::with_style(geom, st),
            None => DObject::new(geom),
        };
        if let Some(aci) = color_aci {
            d.style.color = Color::Aci(aci);
        }
        self.doc.push(d);
        self.history.push(format!(
            "  + block '{}'{} defined (#{}, {} dobject(s)) and instanced at base",
            name,
            if smart { " [smart]" } else { "" },
            id,
            n
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// Start the pick-two-blocks-on-screen compare flow (bare `blockdiff`
    /// or the dialog's Compare button). Click the BASE block, then the
    /// variant.
    pub(super) fn begin_block_diff_pick(&mut self) {
        if self.doc.blocks.is_empty() {
            self.history
                .push("  ! compare: no blocks defined yet".into());
            return;
        }
        self.blockdiff_pick = BlockDiffPick::WaitingForA;
        self.set_prompt("compare: click the BASE block instance  [Esc=cancel]");
    }

    /// Handle a canvas click while picking blocks to compare. Returns true
    /// if the click was consumed by the pick flow.
    pub(super) fn block_diff_pick_click(&mut self, world: Vec2) -> bool {
        let state = self.blockdiff_pick;
        if state == BlockDiffPick::Off {
            return false;
        }
        let tol = 10.0 / self.scale as f64;
        let hit = self.nearest_entity_under(world, tol).and_then(|i| {
            match &self.doc.dobjects.get(i)?.geom {
                Geom::BlockRef(br) => Some(br.block),
                _ => None,
            }
        });
        let Some(block_id) = hit else {
            self.history.push(
                "  compare: that's not a block — click a block instance  [Esc=cancel]".into(),
            );
            return true;
        };
        match state {
            BlockDiffPick::WaitingForA => {
                let name = self
                    .doc
                    .blocks
                    .get(block_id)
                    .map(|b| b.name.clone())
                    .unwrap_or_default();
                self.blockdiff_pick = BlockDiffPick::WaitingForB(block_id);
                self.set_prompt(format!(
                    "compare: base = '{}' — click the SECOND (variant) block  [Esc=cancel]",
                    name
                ));
            }
            BlockDiffPick::WaitingForB(a) => {
                self.blockdiff_pick = BlockDiffPick::Off;
                self.clear_prompt();
                if block_id == a {
                    self.history
                        .push("  ! compare: pick a DIFFERENT block for the variant".into());
                    self.blockdiff_pick = BlockDiffPick::WaitingForA;
                    self.set_prompt("compare: click the BASE block instance  [Esc=cancel]");
                } else {
                    // Extract the modifier vectors, highlight them, and open
                    // the "Set parameters on block" dialog to name them.
                    self.open_param_name_dialog(a, block_id);
                }
            }
            BlockDiffPick::Off => {}
        }
        true
    }

    /// Coordinate tolerance for diffing two blocks — a fraction of the
    /// bigger block's largest-dobject extent, floored.
    fn block_diff_eps(&self, ia: u32, ib: u32) -> f64 {
        let span = |id: u32| {
            self.doc
                .blocks
                .get(id)
                .map(|blk| {
                    blk.dobjects.iter().fold(0.0_f64, |m, d| {
                        let (mn, mx) = d.bbox();
                        m.max((mx - mn).len())
                    })
                })
                .unwrap_or(0.0)
        };
        (span(ia).max(span(ib)) * 1e-4).max(1e-6)
    }

    /// Extract the modifier vectors between two blocks, highlight them on
    /// the BASE block, and open the "Set parameters on block" dialog so the
    /// user can name each + set its source/target values, then Save.
    fn open_param_name_dialog(&mut self, ia: u32, ib: u32) {
        if ia == ib {
            self.history
                .push("  ! compare: pick TWO different blocks".into());
            return;
        }
        let (Some(ba), Some(bb)) = (self.doc.blocks.get(ia), self.doc.blocks.get(ib)) else {
            return;
        };
        let source_name = ba.name.clone();
        let target_name = bb.name.clone();
        let eps = self.block_diff_eps(ia, ib);
        let diff = cad_kernel::diff_blocks(&ba.dobjects, ba.base, &bb.dobjects, bb.base, eps);
        let rows: Vec<ParamRow> = diff
            .clusters
            .iter()
            .filter(|c| c.point_count >= 2)
            .map(|c| ParamRow {
                win_min: c.win_min,
                win_max: c.win_max,
                dir: c.dir,
                magnitude: c.magnitude,
                dx: c.dir.x * c.magnitude,
                dy: c.dir.y * c.magnitude,
                points: c.points.clone(),
                name: String::new(),
                original: "0".into(),
                gain: 1.0,
            })
            .collect();
        // Highlight on the base block.
        let oc: Vec<(Vec<Vec2>, Vec2)> = rows.iter().map(|r| (r.points.clone(), r.dir)).collect();
        self.blockdiff_overlay = if oc.is_empty() {
            None
        } else {
            Some(BlockDiffOverlay {
                block_a: ia,
                clusters: oc,
            })
        };
        if rows.is_empty() {
            self.history.push(format!(
                "  compare '{}' → '{}': no strong modifier vectors (try the ✂ recorder)",
                source_name, target_name
            ));
            return;
        }
        self.history.push(format!(
            "  ◉ {} modifier vector(s) — name them in 'Set parameters on block'",
            rows.len()
        ));
        self.param_name_dialog = Some(ParamNameDialog {
            source_id: ia,
            source_name,
            target_name,
            rows,
        });
    }

    /// Save the named modifier vectors as the SOURCE block's parametric
    /// params (making it a parametric block). Existing instances are reset
    /// to the params' source values so they look unchanged.
    fn save_block_params(&mut self, source_id: u32, rows: &[ParamRow]) {
        // Rows carry BASE-RELATIVE windows; the derive applies them to the
        // block's DEFINITION-space geometry, so add the base back here.
        let base = self
            .doc
            .blocks
            .get(source_id)
            .map(|b| b.base)
            .unwrap_or(Vec2::ZERO);
        // Group rows by name → one VARIABLE per distinct name; rows sharing
        // a name are LINKED (their vectors move together). First-seen order
        // preserved (= the param_values slot order).
        let mut params: Vec<cad_kernel::BlockParam> = Vec::new();
        for r in rows.iter().filter(|r| !r.name.trim().is_empty()) {
            let name = r.name.trim().to_string();
            let vec = cad_kernel::ParamVector {
                win_min: r.win_min + base,
                win_max: r.win_max + base,
                dir: r.dir,
                gain: r.gain,
            };
            if let Some(p) = params.iter_mut().find(|p| p.name == name) {
                p.vectors.push(vec); // link into the existing variable
            } else if params.len() < cad_kernel::MAX_BLOCK_PARAMS {
                let original = r.original.trim().parse().unwrap_or(0.0);
                params.push(cad_kernel::BlockParam {
                    name,
                    original,
                    vectors: vec![vec],
                });
            }
        }
        if params.is_empty() {
            self.history
                .push("  ! set parameters: name at least one vector first".into());
            return;
        }
        let n = params.len();
        let src_defaults: Vec<f64> = params.iter().map(|p| p.original).collect();
        let name = self
            .doc
            .blocks
            .get(source_id)
            .map(|b| b.name.clone())
            .unwrap_or_default();
        if let Some(blk) = self.doc.blocks.get_mut(source_id) {
            blk.params = params;
            blk.smart = true;
        }
        // Keep existing instances looking unchanged: set their values to the
        // source defaults.
        for d in &mut self.doc.dobjects {
            if let Geom::BlockRef(br) = &mut d.geom {
                if br.block == source_id {
                    for (k, v) in src_defaults.iter().enumerate() {
                        if k < cad_kernel::MAX_BLOCK_PARAMS {
                            br.param_values[k] = *v;
                        }
                    }
                }
            }
        }
        self.touch_view();
        self.index_dirty = true;
        self.history.push(format!(
            "  ✔ block '{}' is now parametric — {} param(s). Insert it, then set values in the Inspector.",
            name, n));
    }

    /// Start a Block Task Recorder on the selected block instance — explode
    /// it into a temp sandbox the user stretches to demonstrate parameters.
    /// SUPERSEDED by the isolated Block Editor (`open_block_editor`); kept for
    /// reference. The `btr` command and dialog now open the editor instead.
    #[allow(dead_code)]
    fn begin_block_task_recorder(&mut self) {
        // Find a selected block instance (basket or single).
        let br = self
            .selection
            .iter()
            .copied()
            .chain(self.selected)
            .filter_map(|i| match self.doc.dobjects.get(i).map(|d| &d.geom) {
                Some(Geom::BlockRef(b)) => Some(b.clone()),
                _ => None,
            })
            .next();
        let Some(br) = br else {
            self.history
                .push("  ! block task recorder: select a BLOCK instance first, then `btr`".into());
            return;
        };
        let Some(blk) = self.doc.blocks.get(br.block) else {
            return;
        };
        let source_block = br.block;
        let source_name = blk.name.clone();
        let off = br.insert - blk.base; // place at the instance
        let temps: Vec<DObject> = blk
            .dobjects
            .iter()
            .map(|cd| DObject::with_style(cd.geom.translated(off), cd.style))
            .collect();
        // (blk borrow ends here — `temps` owns clones.)
        self.snapshot_doc();
        // Remove the block's OWN instances for the session so they don't
        // overlap the sandbox (and get caught by the stretch window).
        let mut removed_instances: Vec<DObject> = Vec::new();
        self.doc.dobjects.retain(|d| {
            if matches!(&d.geom, Geom::BlockRef(b) if b.block == source_block) {
                removed_instances.push(d.clone());
                false
            } else {
                true
            }
        });
        let mut temp_handles = Vec::new();
        for t in temps {
            temp_handles.push(t.handle);
            self.doc.push(t);
        }
        self.selection.clear();
        self.selected = None;
        self.block_task_rec = Some(BlockTaskRec {
            source_block,
            source_name: source_name.clone(),
            world_offset: br.insert,
            temp_handles,
            removed_instances,
            recorded: Vec::new(),
        });
        self.index_dirty = true;
        self.touch_view();
        self.history.push(format!(
            "  ◉ Block Task Recorder: '{}' exploded to a sandbox — crossing-window to STRETCH each parameter, then `finish`",
            source_name));
        // Drop the user straight into a stretch selection session so the
        // first crossing-window IS the stretch (no need to type `stretch`).
        self.start_btr_stretch();
    }

    /// Enter a fresh STRETCH selection session for the active Block Task
    /// Recorder so the user can crossing-window straight away instead of
    /// having to type `stretch` first. Mirrors the empty-selection arm of
    /// `Command::Stretch`, with a recorder-specific prompt; deliberately
    /// leaves `block_task_rec` intact (begin_selection only touches the
    /// selection/draw state, not the recorder).
    pub(super) fn start_btr_stretch(&mut self) {
        self.tool = Tool::None;
        self.pending.clear();
        self.stretch_window_box = None;
        self.begin_selection(SelectMode::ForSelect); // clears selection
        self.queued_op = QueuedOp::Stretch; // set AFTER begin_selection
        self.set_prompt(
            "Block Task Recorder ▸ STRETCH: crossing-window the vertices, Enter, \
             then click/​type base+dest  ·  `done` or empty Enter = finish  [Esc=cancel]",
        );
    }

    /// Finish the Block Task Recorder: delete the sandbox and open the
    /// "Set parameters on block" dialog with the recorded tasks (or just
    /// clean up if none were recorded). Esc-cancel calls the cleanup half.
    pub(super) fn finish_block_task_recorder(&mut self) {
        self.btr_awaiting_name = false;
        let Some(rec) = self.block_task_rec.take() else {
            self.history
                .push("  ! finish: no Block Task Recorder active".into());
            return;
        };
        // Delete the sandbox; restore the block's own instances.
        self.doc
            .dobjects
            .retain(|d| !rec.temp_handles.contains(&d.handle));
        for d in &rec.removed_instances {
            self.doc.push(d.clone());
        }
        self.selection.clear();
        self.selected = None;
        // `finish` is usually typed while the auto-started stretch selection
        // session is live — tear it down so no dangling select/stretch state
        // survives into the dialog.
        self.select_mode = SelectMode::Off;
        self.queued_op = QueuedOp::None;
        self.stretch_state = StretchState::Off;
        self.window_first = None;
        self.stretch_window_box = None;
        self.index_dirty = true;
        self.touch_view();
        self.clear_prompt();
        if rec.recorded.is_empty() {
            self.history
                .push("  Block Task Recorder: no stretches recorded — nothing to save".into());
            return;
        }
        let oc: Vec<(Vec<Vec2>, Vec2)> = rec
            .recorded
            .iter()
            .map(|r| (r.points.clone(), r.dir))
            .collect();
        self.blockdiff_overlay = Some(BlockDiffOverlay {
            block_a: rec.source_block,
            clusters: oc,
        });
        self.history.push(format!(
            "  ◉ {} task(s) recorded — name them in 'Set parameters on block'",
            rec.recorded.len()
        ));
        self.param_name_dialog = Some(ParamNameDialog {
            source_id: rec.source_block,
            source_name: rec.source_name,
            target_name: "(recorded)".into(),
            rows: rec.recorded,
        });
    }

    /// `blockdiff <A> <B>` — resolve two block names, then diff. The
    /// pick-on-screen flow calls `run_block_diff_ids` directly.
    pub(super) fn run_block_diff(&mut self, name_a: &str, name_b: &str) {
        let (Some(ia), Some(ib)) = (self.doc.blocks.find(name_a), self.doc.blocks.find(name_b))
        else {
            let names: Vec<String> = self
                .doc
                .blocks
                .blocks
                .iter()
                .map(|b| b.name.clone())
                .collect();
            self.history.push(format!(
                "  ! blockdiff: need two existing blocks — have: {}",
                if names.is_empty() {
                    "none".into()
                } else {
                    names.join(", ")
                }
            ));
            return;
        };
        self.run_block_diff_ids(ia, ib);
    }

    /// Compare two block definitions by id (`ia` = BASE block) and report
    /// the parametric rule: a base block + one MODIFIER VECTOR per cluster,
    /// each shown as (Δx, Δy). Highlights the moved points on the BASE
    /// block's instances. Objective 2, Slice 1 (extraction only).
    fn run_block_diff_ids(&mut self, ia: u32, ib: u32) {
        if ia == ib {
            self.history
                .push("  ! blockdiff: pick TWO different blocks".into());
            return;
        }
        let (Some(ba), Some(bb)) = (self.doc.blocks.get(ia), self.doc.blocks.get(ib)) else {
            self.history.push("  ! blockdiff: block not found".into());
            return;
        };
        let name_a = ba.name.clone();
        let name_b = bb.name.clone();
        // Tolerance ~ a fraction of the bigger block's extent (robust to
        // unit scale); floor so tiny drawings still work.
        let eps = {
            let span = |blk: &cad_kernel::Block| {
                blk.dobjects.iter().fold(0.0_f64, |m, d| {
                    let (mn, mx) = d.bbox();
                    m.max((mx - mn).len())
                })
            };
            (span(ba).max(span(bb)) * 1e-4).max(1e-6)
        };
        let diff = cad_kernel::diff_blocks(&ba.dobjects, ba.base, &bb.dobjects, bb.base, eps);

        let mut lines: Vec<String> = Vec::new();
        lines.push(format!(
            "  blockdiff: base '{}' → '{}':  {} matched ({} unchanged, {} changed), {} added, {} removed  (eps={:.4})",
            name_a, name_b, diff.matched, diff.unchanged, diff.changed,
            diff.added, diff.removed, eps));
        // Split STRONG modifier vectors (≥2 moved points, sorted first)
        // from 1-point OUTLIERS (arc reshape / rotation / mis-match noise).
        let strong: Vec<_> = diff
            .clusters
            .iter()
            .filter(|c| c.point_count >= 2)
            .collect();
        let outliers = diff.clusters.len() - strong.len();
        if strong.is_empty() && outliers == 0 {
            lines.push(
                "    → no point displacements found (identical, or only bulge/topology changed)"
                    .into(),
            );
        } else {
            if strong.is_empty() {
                lines.push(
                    "    → 0 strong modifier vectors; only 1-point outliers (see below)".into(),
                );
            } else {
                lines.push(format!(
                    "    → base block '{}' + {} modifier vector(s):",
                    name_a,
                    strong.len()
                ));
            }
            for (k, c) in strong.iter().enumerate() {
                // Report the displacement as Δx, Δy components.
                let dx = c.dir.x * c.magnitude;
                let dy = c.dir.y * c.magnitude;
                lines.push(format!(
                    "       [P{}] Δx={:.3}  Δy={:.3}   window=({:.3},{:.3})→({:.3},{:.3})  {} point(s)",
                    k + 1, dx, dy,
                    c.win_min.x, c.win_min.y, c.win_max.x, c.win_max.y,
                    c.point_count));
            }
            if outliers > 0 {
                lines.push(format!(
                    "    ⚠ {} single-point outlier(s) ignored — usually the swing arc reshaping, \
                     rotated parts, or a mis-match. If a block has many, the two blocks aren't a \
                     clean copy+stretch — use the ✂ stretch recorder instead.",
                    outliers
                ));
            }
            if !strong.is_empty() {
                lines.push(
                    "    (next: name each modifier vector — typed or pick a point on screen)"
                        .into(),
                );
            }
        }
        for l in &lines {
            self.history.push(l.clone());
        }
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::Note {
                message: lines.join("\n").trim_start().to_string(),
            }
        );

        // Highlight overlay: the STRONG modifier vectors' points, drawn on
        // the BASE block's instances only (block A). Lets the user SEE the
        // parametric points where they actually live.
        let overlay_clusters: Vec<(Vec<Vec2>, Vec2)> = diff
            .clusters
            .iter()
            .filter(|c| c.point_count >= 2)
            .map(|c| (c.points.clone(), c.dir))
            .collect();
        self.blockdiff_overlay = if overlay_clusters.is_empty() {
            None
        } else {
            self.history
                .push("    ◉ parametric points highlighted on the BASE block (Esc clears)".into());
            Some(BlockDiffOverlay {
                block_a: ia,
                clusters: overlay_clusters,
            })
        };
    }

    /// `insert <name>` final step. A PLAIN block is placed immediately. A
    /// PARAMETRIC block instead starts the insert-time value prompt — the
    /// command line asks for each variable ("set width [1000]:") and the
    /// instance is placed once every value is entered.
    // ---- Prompt-driven command flow (CIRCLE; Slice 1) -------------------
    // The command line becomes a sequence of prompts; each (prompt, reply) is
    // recorded in `self.transcript`. See COMMAND_LINE.md.

    /// Start the CIRCLE flow at the center step.
    pub(super) fn circle_flow_start(&mut self) {
        // Ribbon/menu/typed all land here — record it so an empty Enter
        // repeats the command (icon commands ARE app commands).
        self.last_command = Some("circle".into());
        self.cmd_flow = Some(CmdFlow {
            name: "circle",
            circle: CircleStep::Center,
        });
        self.history.push("  command: circle".into());
        self.flow_show_prompt();
    }

    /// Per-step GHOST geometry for the active flow + the optional anchor for a
    /// hint line to the cursor. Drafting commands preview by default — this is
    /// where each step declares what to show. See COMMAND_LINE.md.
    pub(super) fn flow_preview(&self, cursor: Vec2) -> (Vec<Geom>, Option<Vec2>) {
        let Some(f) = &self.cmd_flow else {
            return (Vec::new(), None);
        };
        match f.circle {
            CircleStep::Center => (Vec::new(), None),
            CircleStep::Radius(c) => (
                vec![Geom::Circle(Circle {
                    center: c,
                    radius: (cursor - c).len(),
                })],
                Some(c),
            ),
            CircleStep::Diameter(c) => (
                vec![Geom::Circle(Circle {
                    center: c,
                    radius: (cursor - c).len() * 0.5,
                })],
                Some(c),
            ),
            CircleStep::P3a => (Vec::new(), None),
            CircleStep::P3b(p1) => (Vec::new(), Some(p1)),
            CircleStep::P3c(p1, p2) => match arc_three_points(p1, p2, cursor) {
                Some(a) => (
                    vec![Geom::Circle(Circle {
                        center: a.center,
                        radius: a.radius,
                    })],
                    None,
                ),
                None => (Vec::new(), None),
            },
            CircleStep::P2a => (Vec::new(), None),
            CircleStep::P2b(p1) => {
                let c = (p1 + cursor) * 0.5;
                (
                    vec![Geom::Circle(Circle {
                        center: c,
                        radius: (cursor - p1).len() * 0.5,
                    })],
                    Some(p1),
                )
            }
            // Ttr — highlight the hovered/picked objects, and at the radius
            // step show the live tangent circle for a cursor-derived radius.
            CircleStep::TtrObj1 => {
                let tol = 10.0 / self.scale as f64;
                let mut g = Vec::new();
                if let Some(i) = self.nearest_entity_under(cursor, tol) {
                    if let Some(d) = self.doc.dobjects.get(i) {
                        g.push(d.geom.clone());
                    }
                }
                (g, None)
            }
            CircleStep::TtrObj2(o1, _) => {
                let tol = 10.0 / self.scale as f64;
                let mut g = Vec::new();
                if let Some(d) = self.doc.dobjects.get(o1) {
                    g.push(d.geom.clone());
                }
                if let Some(i) = self.nearest_entity_under(cursor, tol) {
                    if i != o1 {
                        if let Some(d) = self.doc.dobjects.get(i) {
                            g.push(d.geom.clone());
                        }
                    }
                }
                (g, None)
            }
            CircleStep::TtrRadius(o1, pk1, o2, pk2) => {
                let mut g = Vec::new();
                if let Some(d) = self.doc.dobjects.get(o1) {
                    g.push(d.geom.clone());
                }
                if let Some(d) = self.doc.dobjects.get(o2) {
                    g.push(d.geom.clone());
                }
                // Cursor-derived radius = nearest distance from cursor to obj1.
                let r = self
                    .doc
                    .dobjects
                    .get(o1)
                    .map(|d| (cursor - ttr_foot(&d.geom, cursor)).len())
                    .unwrap_or(0.0);
                if r > 1e-6 {
                    if let Some(center) = self.solve_ttr(o1, pk1, o2, pk2, r) {
                        g.push(Geom::Circle(Circle { center, radius: r }));
                    }
                }
                (g, None)
            }
        }
    }

    /// Exact prompt text for the current flow step (empty if no flow).
    fn flow_prompt(&self) -> String {
        let Some(f) = &self.cmd_flow else {
            return String::new();
        };
        match f.circle {
            CircleStep::Center => {
                "Specify center point for circle or [3P/2P/Ttr (tan tan radius)]:".into()
            }
            CircleStep::Radius(_) => "Specify radius of circle or [Diameter]:".into(),
            CircleStep::Diameter(_) => "Specify diameter of circle:".into(),
            CircleStep::P3a => "Specify first point on circle:".into(),
            CircleStep::P3b(_) => "Specify second point on circle:".into(),
            CircleStep::P3c(_, _) => "Specify third point on circle:".into(),
            CircleStep::P2a => "Specify first endpoint of circle's diameter:".into(),
            CircleStep::P2b(_) => "Specify second endpoint of circle's diameter:".into(),
            CircleStep::TtrObj1 => "Specify point on object for first tangent of circle:".into(),
            CircleStep::TtrObj2(..) => {
                "Specify point on object for second tangent of circle:".into()
            }
            CircleStep::TtrRadius(..) => "Specify radius of circle:".into(),
        }
    }

    /// Echo the current step's prompt into the command line + status line.
    pub(super) fn flow_show_prompt(&mut self) {
        let p = self.flow_prompt();
        self.history.push(format!("  {}", p));
        self.set_prompt(p);
        self.refocus_cmd = true;
    }

    /// All current CIRCLE steps accept a point/coordinate.
    pub(super) fn flow_wants_point(&self) -> bool {
        self.cmd_flow.is_some()
    }

    /// True while a command flow is asking the user to PICK AN OBJECT (an
    /// entity), not a point — currently the circle Ttr "first/second tangent"
    /// steps. Object snap must be suppressed there so a click anywhere on the
    /// object selects the entity, instead of snapping the cursor to the
    /// object's centre/endpoint (which then hit-tests as a miss on the curve).
    pub(super) fn flow_picks_object(&self) -> bool {
        matches!(
            self.cmd_flow.as_ref().map(|f| f.circle),
            Some(CircleStep::TtrObj1) | Some(CircleStep::TtrObj2(..))
        )
    }

    /// Pending-based draw tools that capture their first point with a click.
    fn draw_tool_wants_first_point(&self) -> bool {
        matches!(
            self.tool,
            Tool::Line
                | Tool::Arc
                | Tool::Ellipse
                | Tool::EllipseArc
                | Tool::Polyline
                | Tool::Spline
                | Tool::Rectangle
                | Tool::Point
        )
    }

    /// Enter/Space at a draw tool's FIRST-point prompt: feed the remembered
    /// `last_point` as that point so the new command continues from where the
    /// previous one ended (AutoCAD "last point"). Returns true if consumed.
    pub(super) fn feed_first_point_from_last(&mut self) -> bool {
        let Some(p) = self.last_point else {
            return false;
        };
        // CIRCLE / ARC / cmd-flow waiting for a point.
        if self.cmd_flow.is_some() && self.flow_wants_point() {
            self.flow_input_point(p);
            self.refocus_cmd = true;
            return true;
        }
        // Pending-based draw tool with no point captured yet (first point).
        if self.pending.is_empty() && self.draw_tool_wants_first_point() {
            self.pending.push(p);
            self.last_point = Some(p);
            self.try_finalise();
            self.refocus_cmd = true;
            return true;
        }
        false
    }

    /// Record (current prompt, reply) into the transcript + log. Call BEFORE
    /// advancing the step.
    fn flow_record(&mut self, reply: impl Into<String>) {
        let cmd = self.cmd_flow.as_ref().map(|f| f.name).unwrap_or("");
        let prompt = self.flow_prompt();
        let reply = reply.into();
        self.history.push(format!("    {}  →  {}", prompt, reply));
        self.transcript.push(PromptReply { cmd, prompt, reply });
    }

    /// Record the reply that caused the transition, then advance to `next`.
    fn flow_to(&mut self, reply: impl Into<String>, next: CircleStep) {
        self.flow_record(reply);
        self.cmd_flow = Some(CmdFlow {
            name: "circle",
            circle: next,
        });
        self.flow_show_prompt();
    }

    /// Record the reply, draw the circle, end the flow.
    fn flow_finish_circle(&mut self, reply: impl Into<String>, center: Vec2, radius: f64) {
        self.flow_record(reply);
        self.cmd_flow = None;
        self.clear_prompt();
        if radius <= 1e-9 {
            self.history
                .push("  ! circle: zero radius — cancelled".into());
            return;
        }
        self.snapshot_doc();
        self.add_dobject(Geom::Circle(Circle { center, radius }), "circle");
        self.history.push(format!(
            "  ✔ circle — center ({:.3},{:.3})  r={:.3}",
            center.x, center.y, radius
        ));
    }

    /// Cancel the active flow (Esc).
    pub(super) fn flow_cancel(&mut self) {
        if self.cmd_flow.take().is_some() {
            self.clear_prompt();
            self.history.push("  circle: cancelled".into());
        }
    }

    /// A POINT answer — a canvas click (already snapped) or a typed `x,y`.
    pub(super) fn flow_input_point(&mut self, p: Vec2) {
        let step = match self.cmd_flow.as_ref() {
            Some(f) => f.circle,
            None => return,
        };
        self.last_point = Some(p); // remember for Enter-continues-from-last-point
        let r = format!("{:.3},{:.3}", p.x, p.y);
        match step {
            CircleStep::Center => self.flow_to(r, CircleStep::Radius(p)),
            CircleStep::Radius(c) => self.flow_finish_circle(r, c, (p - c).len()),
            CircleStep::Diameter(c) => self.flow_finish_circle(r, c, (p - c).len() * 0.5),
            CircleStep::P3a => self.flow_to(r, CircleStep::P3b(p)),
            CircleStep::P3b(p1) => self.flow_to(r, CircleStep::P3c(p1, p)),
            CircleStep::P3c(p1, p2) => match arc_three_points(p1, p2, p) {
                Some(a) => self.flow_finish_circle(r, a.center, a.radius),
                None => {
                    self.flow_record(r);
                    self.history
                        .push("  ! circle: three points are collinear — pick again".into());
                    self.cmd_flow = Some(CmdFlow {
                        name: "circle",
                        circle: CircleStep::P3a,
                    });
                    self.flow_show_prompt();
                }
            },
            CircleStep::P2a => self.flow_to(r, CircleStep::P2b(p)),
            CircleStep::P2b(p1) => {
                let c = (p1 + p) * 0.5;
                self.flow_finish_circle(r, c, (p - p1).len() * 0.5);
            }
            // Ttr: a click PICKS an object (not a point). Hit-test the nearest
            // dobject; the click point is kept to disambiguate the solution.
            CircleStep::TtrObj1 => {
                let tol = 10.0 / self.scale as f64;
                match self.nearest_entity_under(p, tol) {
                    Some(i) => self.flow_to(format!("object #{}", i), CircleStep::TtrObj2(i, p)),
                    None => self
                        .history
                        .push("  ! no object there — click on a line/circle/arc".into()),
                }
            }
            CircleStep::TtrObj2(o1, pk1) => {
                let tol = 10.0 / self.scale as f64;
                match self.nearest_entity_under(p, tol) {
                    Some(i) if i != o1 => self.flow_to(
                        format!("object #{}", i),
                        CircleStep::TtrRadius(o1, pk1, i, p),
                    ),
                    Some(_) => self
                        .history
                        .push("  ! pick a DIFFERENT object for the second tangent".into()),
                    None => self
                        .history
                        .push("  ! no object there — click on a line/circle/arc".into()),
                }
            }
            // Click at the radius step = pick the radius on screen (nearest
            // distance from the click to object 1). Typing a number also works.
            CircleStep::TtrRadius(o1, pk1, o2, pk2) => {
                let r = match self.doc.dobjects.get(o1) {
                    Some(d1) => (p - ttr_foot(&d1.geom, p)).len(),
                    None => 0.0,
                };
                if r > 1e-9 {
                    if let Some(center) = self.solve_ttr(o1, pk1, o2, pk2, r) {
                        self.flow_finish_circle(format!("{:.3}", r), center, r);
                        return;
                    }
                }
                self.history.push(
                    "  ! couldn't fit a tangent circle there — move out, or type a radius".into(),
                );
            }
        }
    }

    /// A TYPED answer (keyword / number / coordinate) at the current step.
    pub(super) fn flow_input_text(&mut self, raw: &str) {
        let step = match self.cmd_flow.as_ref() {
            Some(f) => f.circle,
            None => return,
        };
        let low = raw.trim().to_ascii_lowercase();
        if low.is_empty() {
            return;
        }
        // Inline object-snap override (END/MID/CEN/PER/TAN/NEA/INT/QUA) — arms a
        // one-shot snap for the NEXT click, exactly as during any other point
        // pick. See feedback_rust_cad_inline_snap_override_supersedes.
        if let Some(kind) = cad_kernel::SnapKind::parse(&low) {
            if kind.requires_from() && self.card_anchor().is_none() {
                self.history.push(format!(
                    "  ! {} needs an anchor — pick the center first",
                    kind.name()
                ));
            } else {
                self.snap_override = Some(kind);
                self.history.push(format!(
                    "  ↳ {} armed — hover the target and click",
                    kind.name()
                ));
            }
            return;
        }
        // A typed coordinate `x,y` is a POINT answer in every step.
        if let Some(p) = parse_xy(&self.calc, raw) {
            self.flow_input_point(p);
            return;
        }
        match step {
            CircleStep::Center => match low.as_str() {
                "3p" => self.flow_to("3P", CircleStep::P3a),
                "2p" => self.flow_to("2P", CircleStep::P2a),
                "t" | "ttr" => self.flow_to("Ttr", CircleStep::TtrObj1),
                _ if low.parse::<f64>().is_ok() => self.history.push(
                    "  ! a single number here is a COORDINATE, not a radius — type x,y or click  [3P/2P/Ttr]".into()),
                _ => self.history.push(
                    "  ! Invalid option keyword — valid: 3P / 2P / Ttr, or pick a point".into()),
            },
            CircleStep::Radius(c) => {
                if low == "d" || low == "diameter" {
                    self.flow_to("D", CircleStep::Diameter(c));
                } else if let Ok(rad) = self.eval_number(raw) {
                    self.flow_finish_circle(format!("{}", rad), c, rad);
                } else {
                    self.history.push("  ! need a radius number, D, or a point".into());
                }
            }
            CircleStep::Diameter(c) => {
                if let Ok(d) = self.eval_number(raw) {
                    self.flow_finish_circle(format!("{}", d), c, d * 0.5);
                } else {
                    self.history.push("  ! need a diameter number".into());
                }
            }
            CircleStep::TtrRadius(o1, pk1, o2, pk2) => {
                if let Ok(rad) = self.eval_number(raw) {
                    if rad <= 1e-9 {
                        self.history.push("  ! radius must be > 0".into());
                        return;
                    }
                    match self.solve_ttr(o1, pk1, o2, pk2, rad) {
                        Some(center) => self.flow_finish_circle(format!("{}", rad), center, rad),
                        None => {
                            self.flow_record(format!("{}", rad));
                            self.cmd_flow = None;
                            self.clear_prompt();
                            self.history.push(
                                "  ! circle: no tangent circle of that radius touches both objects".into());
                        }
                    }
                } else {
                    self.history.push("  ! need a radius number".into());
                }
            }
            _ => self.history.push("  click an object (or type x,y)".into()),
        }
    }

    /// Solve Ttr (tangent-tangent-radius): find the centre of a circle of
    /// radius `r` tangent to dobjects `o1` and `o2`. Candidate centres are the
    /// intersections of each object's ±r offset curves (reusing the kernel's
    /// `intersect`); the one whose tangent feet land nearest the two pick
    /// points (`pk1`,`pk2`) wins — matching AutoCAD's pick-driven choice.
    fn solve_ttr(&self, o1: usize, pk1: Vec2, o2: usize, pk2: Vec2, r: f64) -> Option<Vec2> {
        let ga = self.doc.dobjects.get(o1)?.geom.clone();
        let gb = self.doc.dobjects.get(o2)?.geom.clone();
        let offs_a = ttr_offsets(&ga, r);
        let offs_b = ttr_offsets(&gb, r);
        let mut best: Option<(f64, Vec2)> = None;
        for oa in &offs_a {
            for ob in &offs_b {
                for c in cad_kernel::intersect(oa, ob) {
                    let fa = ttr_foot(&ga, c);
                    let fb = ttr_foot(&gb, c);
                    // Verify true tangency (filters invalid offset branches).
                    if ((fa - c).len() - r).abs() > 1e-6 {
                        continue;
                    }
                    if ((fb - c).len() - r).abs() > 1e-6 {
                        continue;
                    }
                    let score = (fa - pk1).len() + (fb - pk2).len();
                    if best.map_or(true, |(bs, _)| score < bs) {
                        best = Some((score, c));
                    }
                }
            }
        }
        best.map(|(_, c)| c)
    }

    // ===================================================================
    // ZOOM command — AutoCAD-style sub-option flow.
    //
    // Entry: `zoom` / `z` (optionally with an inline argument) → zoom_start().
    // While `zoom_state != Off` the command line is captured (zoom_input_text)
    // and canvas clicks at point steps route to zoom_input_point(). Empty Enter
    // is handled in the update() Enter cascade (RealTime exit / Object apply /
    // Center default). The view is driven purely through `scale` + `world_offset`
    // (w2s: screen = center + (world + world_offset) * scale).
    // ===================================================================

    /// World-space bbox of every dobject (None when the drawing is empty).
    pub(super) fn doc_extents(&self) -> Option<(Vec2, Vec2)> {
        let mut it = self.doc.dobjects.iter();
        let (mut min, mut max) = it.next()?.bbox();
        for d in it {
            let (a, b) = d.bbox();
            if a.x < min.x {
                min.x = a.x;
            }
            if a.y < min.y {
                min.y = a.y;
            }
            if b.x > max.x {
                max.x = b.x;
            }
            if b.y > max.y {
                max.y = b.y;
            }
        }
        Some((min, max))
    }

    /// World-space bbox of the current selection (None when nothing selected).
    fn selection_extents(&self) -> Option<(Vec2, Vec2)> {
        let mut acc: Option<(Vec2, Vec2)> = None;
        for &i in &self.selection {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            let (a, b) = d.bbox();
            acc = Some(match acc {
                None => (a, b),
                Some((mn, mx)) => (
                    Vec2::new(mn.x.min(a.x), mn.y.min(a.y)),
                    Vec2::new(mx.x.max(b.x), mx.y.max(b.y)),
                ),
            });
        }
        acc
    }

    /// The canvas viewport rect (stashed each frame). Falls back to a nominal
    /// size only if a zoom is somehow requested before the first canvas layout.
    fn view_rect_or(&self) -> egui::Rect {
        self.canvas_screen_rect.unwrap_or_else(|| {
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
        })
    }

    /// Push the current view onto the history stack (cap 10) so ZOOM Previous
    /// can step back. Call BEFORE mutating `scale` / `world_offset`.
    pub(super) fn view_push_history(&mut self) {
        self.view_history.push((self.scale, self.world_offset));
        let n = self.view_history.len();
        if n > 10 {
            self.view_history.drain(0..n - 10);
        }
    }

    /// Set scale + offset so `min..max` fills `frac` of the viewport, centered.
    /// `frac` < 1.0 leaves a margin (0.9 for extents, 1.0 for a tight window).
    pub(super) fn zoom_fit_bbox(&mut self, min: Vec2, max: Vec2, frac: f32) {
        self.view_push_history();
        let center = (min + max) * 0.5;
        let w = (max.x - min.x).abs();
        let h = (max.y - min.y).abs();
        let rect = self.view_rect_or();
        // Fit the LIMITING axis so the whole bbox is visible at this aspect.
        let sx = if w > 1e-9 {
            rect.width() as f64 / w
        } else {
            f64::INFINITY
        };
        let sy = if h > 1e-9 {
            rect.height() as f64 / h
        } else {
            f64::INFINITY
        };
        let mut s = sx.min(sy);
        if !s.is_finite() {
            s = self.scale as f64;
        } // degenerate (point) bbox
        self.scale = ((s * frac as f64) as f32).clamp(0.01, 5000.0);
        self.world_offset = egui::vec2(-center.x as f32, -center.y as f32);
    }

    /// Fit the view to the drawing on OPEN — no manual ZOOM needed. Uses
    /// RESOLVED block bboxes (so block-heavy drawings frame correctly, unlike
    /// raw `doc_extents`, which sees only block insert points). If a few stray
    /// entities sit far from the bulk (making the real content tiny / the view
    /// look blank), it falls back to a robust, outlier-trimmed extent so we zoom
    /// to where the drawing actually is.
    pub(super) fn fit_view_to_drawing(&mut self) {
        let fin = |v: Vec2| v.x.is_finite() && v.y.is_finite();
        let boxes: Vec<(Vec2, Vec2)> = self
            .doc
            .dobjects
            .iter()
            .filter_map(|d| {
                let (a, b) = match &d.geom {
                    Geom::BlockRef(br) => self.resolved_blockref_bbox(br),
                    _ => d.bbox(),
                };
                if fin(a) && fin(b) {
                    Some((a, b))
                } else {
                    None
                }
            })
            .collect();
        if boxes.is_empty() {
            return;
        } // nothing to display → leave the view

        let union = |bs: &[(Vec2, Vec2)]| -> (Vec2, Vec2) {
            let (mut mn, mut mx) = (bs[0].0, bs[0].1);
            for (a, b) in bs {
                mn.x = mn.x.min(a.x);
                mn.y = mn.y.min(a.y);
                mx.x = mx.x.max(b.x);
                mx.y = mx.y.max(b.y);
            }
            (mn, mx)
        };
        let full = union(&boxes);

        // Robust extent: central 90% of entity centres, padded by a typical
        // entity size — trims far-away strays.
        let mut xs: Vec<f64> = boxes.iter().map(|(a, b)| (a.x + b.x) * 0.5).collect();
        let mut ys: Vec<f64> = boxes.iter().map(|(a, b)| (a.y + b.y) * 0.5).collect();
        let mut sz: Vec<f64> = boxes
            .iter()
            .map(|(a, b)| (b.x - a.x).abs().max((b.y - a.y).abs()))
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        sz.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |v: &[f64], p: f64| v[(((v.len() - 1) as f64) * p).round() as usize];
        let pad = sz[sz.len() / 2].max(1e-6);
        let robust = (
            Vec2::new(pct(&xs, 0.05) - pad, pct(&ys, 0.05) - pad),
            Vec2::new(pct(&xs, 0.95) + pad, pct(&ys, 0.95) + pad),
        );

        // Use the robust box only when strays blow the full extent up (≫16×).
        let area =
            |bb: (Vec2, Vec2)| ((bb.1.x - bb.0.x).abs() + 1e-9) * ((bb.1.y - bb.0.y).abs() + 1e-9);
        let pick = if boxes.len() >= 8 && area(full) > area(robust) * 16.0 {
            robust
        } else {
            full
        };
        self.zoom_fit_bbox(pick.0, pick.1, 0.9);
    }

    /// ZOOM Extents — fit all objects.
    pub(super) fn zoom_extents(&mut self) {
        match self.doc_extents() {
            Some((mn, mx)) => {
                self.zoom_fit_bbox(mn, mx, 0.9);
                self.history.push("  ✔ zoom: extents".into());
            }
            None => self
                .history
                .push("  zoom: nothing to fit (drawing is empty)".into()),
        }
    }

    /// Frame the demo plan a fresh app ships with — ONCE, on the first real
    /// frame ([`Self::demo_view_set`]). The demo geometry is drawn at real
    /// room sizes (thousands of drawing-units in the millimetre document), so
    /// the launch camera (origin ± a couple of hundred units) would show an
    /// empty canvas. Guarded so it never overrides the user: only while no
    /// file is open and the view history is still empty (no zoom has
    /// happened), and once done it never runs again.
    pub(super) fn maybe_frame_demo_plan(&mut self) {
        if !self.demo_view_set
            && self.canvas_screen_rect.is_some()
            && self.current_file.is_none()
            && self.view_history.is_empty()
        {
            self.demo_view_set = true;
            self.fit_view_to_drawing();
        }
    }

    /// ZOOM All — limits-or-extents. This app has no drawing-limits concept, so
    /// All == Extents; an empty drawing resets to a default view.
    fn zoom_all(&mut self) {
        match self.doc_extents() {
            Some((mn, mx)) => {
                self.zoom_fit_bbox(mn, mx, 0.9);
                self.history.push("  ✔ zoom: all (= extents)".into());
            }
            None => {
                self.view_push_history();
                self.scale = 20.0;
                self.world_offset = egui::Vec2::ZERO;
                self.history
                    .push("  zoom: all — empty drawing, default view".into());
            }
        }
    }

    /// ZOOM Object — fit the current selection.
    pub(super) fn zoom_object(&mut self) {
        match self.selection_extents() {
            Some((mn, mx)) => {
                let n = self.selection.len();
                self.zoom_fit_bbox(mn, mx, 0.85);
                self.history
                    .push(format!("  ✔ zoom: object(s) — {} selected", n));
            }
            None => self
                .history
                .push("  ! zoom object: nothing selected".into()),
        }
    }

    /// ZOOM Previous — restore the last saved view (up to 10 deep).
    pub(super) fn zoom_previous(&mut self) {
        match self.view_history.pop() {
            Some((s, off)) => {
                self.scale = s;
                self.world_offset = off;
                self.history.push(format!(
                    "  ✔ zoom: previous  ({} left)",
                    self.view_history.len()
                ));
            }
            None => self.history.push("  zoom: no previous view".into()),
        }
    }

    /// ZOOM Scale `nX` — multiply the current view scale (objects appear n×).
    fn zoom_scale_factor(&mut self, factor: f64) {
        if factor <= 1e-9 {
            self.history
                .push("  ! zoom scale: factor must be > 0".into());
            return;
        }
        self.view_push_history();
        self.scale = ((self.scale as f64 * factor) as f32).clamp(0.01, 5000.0);
        self.history.push(format!("  ✔ zoom: scale {}×", factor));
    }

    /// ZOOM Window — fit the rectangle defined by two opposite corners.
    fn zoom_window(&mut self, a: Vec2, b: Vec2) {
        let mn = Vec2::new(a.x.min(b.x), a.y.min(b.y));
        let mx = Vec2::new(a.x.max(b.x), a.y.max(b.y));
        if (mx.x - mn.x) < 1e-9 || (mx.y - mn.y) < 1e-9 {
            self.history
                .push("  ! zoom window: zero-area box — cancelled".into());
            return;
        }
        self.zoom_fit_bbox(mn, mx, 1.0);
        self.history.push("  ✔ zoom: window".into());
    }

    /// ZOOM Center — recenter on `c`. Magnification is `Some(factor)` for an
    /// `nX` entry, or `height` (new view height in drawing units) for a plain
    /// number; both `None` = recenter only, keep the current zoom.
    pub(super) fn zoom_center(&mut self, c: Vec2, height: Option<f64>, factor: Option<f64>) {
        self.view_push_history();
        self.world_offset = egui::vec2(-c.x as f32, -c.y as f32);
        if let Some(f) = factor.filter(|f| *f > 1e-9) {
            self.scale = ((self.scale as f64 * f) as f32).clamp(0.01, 5000.0);
        } else if let Some(h) = height.filter(|h| *h > 1e-9) {
            let rect = self.view_rect_or();
            self.scale = ((rect.height() as f64 / h) as f32).clamp(0.01, 5000.0);
        }
        self.history
            .push(format!("  ✔ zoom: center ({:.3},{:.3})", c.x, c.y));
    }

    /// The top-level ZOOM prompt line.
    fn zoom_menu_prompt() -> &'static str {
        "ZOOM — window corner, scale (nX), or \
         [All/Center/Dynamic/Extents/Previous/Scale/Window/Object] <real time>:"
    }

    /// Transition to a ZOOM sub-state and show its prompt.
    pub(super) fn zoom_set_state(&mut self, st: ZoomState, prompt: impl Into<String>) {
        self.zoom_state = st;
        let p = prompt.into();
        self.history.push(format!("  {}", p));
        self.set_prompt(p);
        self.refocus_cmd = true;
    }

    /// End the flow and return the command line to idle.
    pub(super) fn zoom_finish(&mut self) {
        self.zoom_state = ZoomState::Off;
        self.clear_prompt();
    }

    /// Cancel an active ZOOM flow (called from the global Esc handler).
    pub(super) fn zoom_cancel(&mut self) {
        if self.zoom_state != ZoomState::Off {
            self.zoom_state = ZoomState::Off;
            self.clear_prompt();
            self.history.push("  zoom: cancelled".into());
        }
    }

    /// Entry point: `zoom` / `z`, optionally with an inline argument
    /// (`zoom e`, `z 2x`, `zoom 10,10`, …).
    pub(super) fn zoom_start(&mut self, arg: &str) {
        self.last_command = Some("zoom".into());
        let arg = arg.trim();
        if arg.is_empty() {
            self.history.push("  command: zoom".into());
            self.zoom_set_state(ZoomState::Menu, Self::zoom_menu_prompt());
        } else {
            self.zoom_state = ZoomState::Menu;
            self.history.push(format!("  command: zoom {}", arg));
            self.zoom_input_text(arg);
        }
    }

    /// A TYPED answer (option letter / number / coordinate) while ZOOM is live.
    pub(super) fn zoom_input_text(&mut self, raw: &str) {
        let s = raw.trim();
        let low = s.to_ascii_lowercase();
        match self.zoom_state {
            ZoomState::Off => {}
            ZoomState::Menu => {
                // (empty Enter → real-time is handled in the update() cascade)
                if let Some(p) = parse_xy(&self.calc, s) {
                    // typed coordinate = first corner of an implicit Window
                    self.zoom_set_state(
                        ZoomState::WinSecond(p),
                        "zoom window: specify OPPOSITE corner",
                    );
                } else if let Some(f) = parse_scale_x(&low) {
                    self.zoom_scale_factor(f);
                    self.zoom_finish();
                } else {
                    match low.as_str() {
                        "a" | "all"      => { self.zoom_all();      self.zoom_finish(); }
                        "e" | "extents"  => { self.zoom_extents();  self.zoom_finish(); }
                        "p" | "previous" => { self.zoom_previous(); self.zoom_finish(); }
                        "c" | "center"   => self.zoom_set_state(ZoomState::CenterPoint,
                            "zoom center: specify center point"),
                        "w" | "window"   => self.zoom_set_state(ZoomState::WinFirst,
                            "zoom window: specify FIRST corner"),
                        "o" | "object"   => self.zoom_set_state(ZoomState::ObjectSel,
                            "zoom object: select objects, then Enter  [Esc=cancel]"),
                        "s" | "scale"    => {
                            self.history.push(
                                "  zoom scale: enter a factor as nX (e.g. 2X)".into());
                            self.set_prompt("zoom scale: enter factor (nX, e.g. 2X)");
                            self.refocus_cmd = true;
                        }
                        "r" | "realtime" | "real" => self.zoom_set_state(ZoomState::RealTime,
                            "real-time zoom: drag UP = in, DOWN = out  [Enter/Esc = exit]"),
                        "d" | "dynamic"  => {
                            self.history.push(
                                "  zoom: Dynamic is not implemented — use Window or the scroll wheel".into());
                            self.zoom_finish();
                        }
                        _ => self.history.push(format!(
                            "  ! zoom: '{}' — type A/C/E/P/S/W/O, an nX factor, x,y, or Enter for real-time", s)),
                    }
                }
            }
            ZoomState::CenterPoint => {
                if let Some(p) = parse_xy(&self.calc, s) {
                    self.zoom_set_state(
                        ZoomState::CenterMag(p),
                        "zoom center: enter magnification (nX) or height <Enter=keep>",
                    );
                } else {
                    self.history
                        .push("  ! zoom center: pick a point or type x,y".into());
                }
            }
            ZoomState::CenterMag(c) => {
                // (empty Enter → recenter only is handled in the update() cascade)
                if let Some(f) = parse_scale_x(&low) {
                    self.zoom_center(c, None, Some(f));
                    self.zoom_finish();
                } else if let Ok(h) = self.eval_number(s) {
                    self.zoom_center(c, Some(h), None);
                    self.zoom_finish();
                } else {
                    self.history
                        .push("  ! zoom center: enter a height number or nX".into());
                }
            }
            ZoomState::WinFirst => {
                if let Some(p) = parse_xy(&self.calc, s) {
                    self.zoom_set_state(
                        ZoomState::WinSecond(p),
                        "zoom window: specify OPPOSITE corner",
                    );
                } else {
                    self.history
                        .push("  ! zoom window: pick the first corner or type x,y".into());
                }
            }
            ZoomState::WinSecond(a) => {
                if let Some(p) = parse_xy(&self.calc, s) {
                    self.zoom_window(a, p);
                    self.zoom_finish();
                } else {
                    self.history
                        .push("  ! zoom window: pick the opposite corner or type x,y".into());
                }
            }
            ZoomState::ObjectSel => {
                // (empty Enter → apply is handled in the update() cascade)
                self.history
                    .push("  zoom object: click objects on the canvas, then Enter".into());
            }
            ZoomState::RealTime => {
                // Any typed input ends real-time mode.
                self.zoom_finish();
                self.history.push("  zoom: real-time done".into());
            }
        }
    }

    /// A canvas POINT (already snap-applied) during a ZOOM flow.
    pub(super) fn zoom_input_point(&mut self, p: Vec2) {
        match self.zoom_state {
            // A click at the top-level prompt OR at WinFirst = first corner.
            ZoomState::Menu | ZoomState::WinFirst => self.zoom_set_state(
                ZoomState::WinSecond(p),
                "zoom window: specify OPPOSITE corner",
            ),
            ZoomState::WinSecond(a) => {
                self.zoom_window(a, p);
                self.zoom_finish();
            }
            ZoomState::CenterPoint => self.zoom_set_state(
                ZoomState::CenterMag(p),
                "zoom center: enter magnification (nX) or height <Enter=keep>",
            ),
            // CenterMag waits for a TYPED value; a stray click is ignored.
            _ => {}
        }
    }

    pub(super) fn apply_insert(&mut self, block: u32, insert: Vec2, rotation: f64) {
        let (names, defaults): (Vec<String>, Vec<f64>) = match self.doc.blocks.get(block) {
            Some(blk) if !blk.params.is_empty() => (
                blk.params.iter().map(|p| p.name.clone()).collect(),
                blk.params.iter().map(|p| p.original).collect(),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        if names.is_empty() {
            // Plain block — place now, then cut the host opening (if any).
            self.snapshot_doc();
            let br = cad_kernel::BlockRef {
                block,
                insert,
                scale: 1.0,
                scale_y: 1.0,
                rotation,
                mirror_x: false,
                param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
                attr_values: Vec::new(),
            };
            self.add_dobject(Geom::BlockRef(br.clone()), "insert");
            self.apply_block_cut(br);
            return;
        }
        // Parametric — prompt for each variable, starting with the first.
        self.set_prompt(format!(
            "insert: set {} [{}]  (Enter=default)  [Esc=cancel]",
            names[0], defaults[0]
        ));
        self.insert_param_prompt = Some(InsertParamPrompt {
            block,
            insert,
            rotation,
            names,
            defaults,
            values: Vec::new(),
        });
        self.refocus_cmd = true;
    }

    /// Place the parametric instance once all variable values are collected,
    /// then cut the host opening (if the block has cut edges).
    pub(super) fn place_parametric_insert(&mut self, p: InsertParamPrompt) {
        self.snapshot_doc();
        let mut param_values = [0.0; cad_kernel::MAX_BLOCK_PARAMS];
        for (k, v) in p.values.iter().enumerate() {
            if k < cad_kernel::MAX_BLOCK_PARAMS {
                param_values[k] = *v;
            }
        }
        let br = cad_kernel::BlockRef {
            block: p.block,
            insert: p.insert,
            scale: 1.0,
            scale_y: 1.0,
            rotation: p.rotation,
            mirror_x: false,
            param_values,
            attr_values: Vec::new(),
        };
        self.add_dobject(Geom::BlockRef(br.clone()), "insert");
        self.apply_block_cut(br);
        self.clear_prompt();
    }

    /// Cut the host opening: if the inserted block carries CUT EDGES (jamb
    /// lines), build the region they enclose (derived + transformed to world)
    /// and trim out the portion of every host Line/Polyline that falls inside
    /// it. The door/window leaves a clean gap in the wall. Runs as part of the
    /// insert op (no extra undo snapshot — one undo reverts insert + cut).
    fn apply_block_cut(&mut self, br: cad_kernel::BlockRef) {
        let region: Vec<Vec2> = {
            let Some(blk) = self.doc.blocks.get(br.block) else {
                return;
            };
            if blk.cut_edges.is_empty() {
                return;
            }
            let derived = self.block_derived_geoms(blk, &br.param_values);
            let mut pts: Vec<Vec2> = Vec::new();
            for &i in &blk.cut_edges {
                if let Some(g) = derived.get(i) {
                    let wg = br.transform_geom(g, blk.base);
                    edge_endpoints(&wg, &mut pts);
                }
            }
            convex_hull(&pts)
        };
        if region.len() < 3 {
            return;
        }
        let mut changed = false;
        let old = std::mem::take(&mut self.doc.dobjects);
        let mut out: Vec<DObject> = Vec::with_capacity(old.len());
        for d in old {
            match &d.geom {
                Geom::Line(l) => {
                    let parts = clip_line_outside(l.a, l.b, &region);
                    let whole = parts.len() == 1
                        && (parts[0].0 - l.a).len() < 1e-6
                        && (parts[0].1 - l.b).len() < 1e-6;
                    if whole {
                        out.push(d);
                    } else {
                        changed = true;
                        for (a, b) in parts {
                            if (a - b).len() > 1e-9 {
                                out.push(DObject::with_style(Geom::Line(Line { a, b }), d.style));
                            }
                        }
                    }
                }
                Geom::Polyline(pl) => {
                    let verts: Vec<Vec2> = pl.vertices.iter().map(|v| v.pos).collect();
                    let mut segs: Vec<(Vec2, Vec2)> =
                        verts.windows(2).map(|w| (w[0], w[1])).collect();
                    if pl.closed && verts.len() >= 2 {
                        segs.push((verts[verts.len() - 1], verts[0]));
                    }
                    let mut survivors: Vec<(Vec2, Vec2)> = Vec::new();
                    let mut any_cut = false;
                    for (a, b) in &segs {
                        let parts = clip_line_outside(*a, *b, &region);
                        let whole = parts.len() == 1
                            && (parts[0].0 - *a).len() < 1e-6
                            && (parts[0].1 - *b).len() < 1e-6;
                        if !whole {
                            any_cut = true;
                        }
                        survivors.extend(parts);
                    }
                    if !any_cut {
                        out.push(d);
                    } else {
                        changed = true;
                        for (a, b) in survivors {
                            if (a - b).len() > 1e-9 {
                                out.push(DObject::with_style(Geom::Line(Line { a, b }), d.style));
                            }
                        }
                    }
                }
                _ => out.push(d), // blocks / walls / arcs untouched (v1)
            }
        }
        self.doc.dobjects = out;
        if changed {
            self.index_dirty = true;
            self.touch_view();
            self.history
                .push("  ✔ opening cut into host geometry".into());
        }
    }

    /// Explode: replace each selected BlockRef with transformed copies of
    /// its contents (ONE level — nested blockrefs stay instances, like
    /// AutoCAD). `Color::ByBlock` contents take the instance's color.
    /// Fresh handles via `DObject::with_style` (clones must not reuse the
    /// definition's handles).
    // ---- PEDIT (polyline edit) -----------------------------------------

    /// Find a dobject's current index by handle.
    pub(super) fn idx_of_handle(&self, h: u64) -> Option<usize> {
        self.doc.dobjects.iter().position(|d| d.handle == h)
    }

    // PEDIT (polyline edit) methods moved to `src/app/pedit.rs` (child module
    // `pedit`, declared below). They remain inherent `CadApp` methods, so call
    // sites here are unchanged.

    pub(super) fn apply_explode(&mut self) {
        if self.selection.is_empty() {
            self.history.push("  ! explode: empty selection".into());
            return;
        }
        self.snapshot_doc();
        let mut sel: Vec<usize> = self.selection.clone();
        sel.sort_unstable();
        let mut to_remove: Vec<usize> = Vec::new();
        let mut to_add: Vec<DObject> = Vec::new();
        let mut skipped = 0_usize;
        for &i in &sel {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            let dstyle = d.style;
            match &d.geom {
                Geom::BlockRef(br) => {
                    if let Some(blk) = self.doc.blocks.get(br.block) {
                        // Explode the DERIVED geometry (params applied), so a
                        // parametric instance explodes to what's drawn.
                        let derived = self.block_derived_geoms(blk, &br.param_values);
                        for (cd, dg) in blk.dobjects.iter().zip(&derived) {
                            let mut style = cd.style;
                            if matches!(style.color, Color::ByBlock) {
                                style.color = dstyle.color;
                            }
                            to_add
                                .push(DObject::with_style(br.transform_geom(dg, blk.base), style));
                        }
                        to_remove.push(i);
                    } else {
                        skipped += 1; // dangling reference
                    }
                }
                // Polyline / rectangle → individual Line + Arc segments.
                Geom::Polyline(p) => {
                    let segs = explode_polyline(p);
                    if segs.is_empty() {
                        skipped += 1;
                    } else {
                        for g in segs {
                            to_add.push(DObject::with_style(g, dstyle));
                        }
                        to_remove.push(i);
                    }
                }
                // Wall → its boundary "particles": the two faces (lines for a
                // straight wall, arc-sampled polylines for a curved one) plus
                // the two end caps — a closed outline inheriting the wall style.
                Geom::Wall(w) => {
                    let mk_face = |pts: &Vec<Vec2>| -> Geom {
                        if pts.len() == 2 {
                            Geom::Line(Line {
                                a: pts[0],
                                b: pts[1],
                            })
                        } else {
                            Geom::Polyline(cad_kernel::Polyline {
                                vertices: pts
                                    .iter()
                                    .map(|p| cad_kernel::PolyVertex {
                                        pos: *p,
                                        bulge: 0.0,
                                    })
                                    .collect(),
                                closed: false,
                                widths: Vec::new(),
                            })
                        }
                    };
                    if let Some((left, right)) = w.face_polylines(48) {
                        to_add.push(DObject::with_style(mk_face(&left), dstyle));
                        to_add.push(DObject::with_style(mk_face(&right), dstyle));
                        if let (Some(&l0), Some(&r0), Some(&l1), Some(&r1)) =
                            (left.first(), right.first(), left.last(), right.last())
                        {
                            to_add.push(DObject::with_style(
                                Geom::Line(Line { a: l0, b: r0 }),
                                dstyle,
                            )); // start cap
                            to_add.push(DObject::with_style(
                                Geom::Line(Line { a: l1, b: r1 }),
                                dstyle,
                            )); // end cap
                        }
                        to_remove.push(i);
                    } else {
                        skipped += 1; // degenerate wall (start ≈ end)
                    }
                }
                // TXTEXP — text explodes into CLOSED glyph-outline polylines
                // (one per contour; holes as separate closed loops). Hatch
                // island resolution treats the loops even-odd, so a letter
                // counter keeps working as a hatch island.
                Geom::Text(t) => {
                    let font = self.resolve_text_font(t);
                    let req = cad_text::TextRequest {
                        text: &t.text,
                        font_name: &font,
                        position: t.position,
                        height: t.height,
                        angle: t.angle,
                        h_align: t.h_align,
                        v_align: t.v_align,
                        fill_mode: cad_text::FillMode::Fill,
                        slant: t.oblique,
                        x_scale: t.width_factor,
                    };
                    let glyphs = self.font_manager.borrow_mut().render_explode(&req);
                    let mk_poly = |pts: &Vec<Vec2>| -> Geom {
                        Geom::Polyline(cad_kernel::Polyline {
                            vertices: pts
                                .iter()
                                .map(|p| cad_kernel::PolyVertex {
                                    pos: *p,
                                    bulge: 0.0,
                                })
                                .collect(),
                            closed: true,
                            widths: Vec::new(),
                        })
                    };
                    let mut n_polys = 0usize;
                    for (outer, holes) in &glyphs.glyph_polygons {
                        if outer.len() >= 3 {
                            to_add.push(DObject::with_style(mk_poly(outer), dstyle));
                            n_polys += 1;
                        }
                        for h in holes {
                            if h.len() >= 3 {
                                to_add.push(DObject::with_style(mk_poly(h), dstyle));
                                n_polys += 1;
                            }
                        }
                    }
                    if n_polys == 0 {
                        skipped += 1; // empty / unshapeable text
                    } else {
                        to_remove.push(i);
                    }
                }
                _ => skipped += 1, // not explodable (line/circle/arc/…)
            }
        }
        let exploded = to_remove.len();
        let added = to_add.len();
        for &i in to_remove.iter().rev() {
            self.doc.dobjects.remove(i);
        }
        for nd in to_add {
            self.doc.push(nd);
        }
        self.selection.clear();
        self.history.push(format!(
            "  ✸ explode: {} object(s) → {} dobject(s){}",
            exploded,
            added,
            if skipped > 0 {
                format!("  ({} skipped — not explodable)", skipped)
            } else {
                String::new()
            }
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    pub(super) fn apply_reverse(&mut self) {
        if self.selection.is_empty() {
            self.history.push("  ! reverse: empty basket".into());
            return;
        }
        self.snapshot_doc();
        let mut flipped = 0_usize;
        let mut noop = 0_usize;
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                let direction_aware = matches!(
                    d.geom,
                    Geom::Line(_) | Geom::Arc(_) | Geom::EllipseArc(_) | Geom::Polyline(_)
                );
                if direction_aware {
                    d.geom = d.geom.reversed();
                    flipped += 1;
                } else {
                    noop += 1;
                }
            }
        }
        self.history.push(format!(
            "  ⇋ reverse: {} flipped, {} no-op (direction-agnostic)",
            flipped, noop
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    // ---- Slice L apply methods ----

    /// Refresh the offset prompt with the current mode (Distance/Through),
    /// erase + layer mode flags, and applied count. Called after every
    /// sub-option toggle or phase transition so the visible prompt is
    /// always current.
    pub(super) fn refresh_offset_prompt(&mut self) {
        let mode_str = match self.offset_state {
            OffsetState::Off => return,
            OffsetState::WaitingForObject(m) | OffsetState::WaitingForSide(m, _) => match m {
                OffsetMode::Distance(d) => format!("d={}", d),
                OffsetMode::Through => "THROUGH".to_string(),
            },
        };
        let phase = match self.offset_state {
            OffsetState::WaitingForObject(_) => "select object to offset",
            OffsetState::WaitingForSide(OffsetMode::Through, _) => "click THROUGH-point",
            OffsetState::WaitingForSide(_, _) => "click SIDE",
            OffsetState::Off => return,
        };
        let badges = format!(
            "{}{}{}",
            mode_str,
            if self.offset_erase { ", erase" } else { "" },
            if self.offset_layer_src {
                ", lyr=src"
            } else {
                ""
            },
        );
        self.set_prompt(format!(
            "offset ({}): {}  [t=through, e=erase, l=layer, u=undo, Esc]",
            badges, phase
        ));
    }

    /// Apply a single offset of one source dobject by one side/through
    /// click. Honors the current mode (Distance or Through), the
    /// erase flag (delete source after), and the layer flag (Current
    /// vs Source layer for the result). Pushes the result and bumps
    /// the in-command undo counter.
    pub(super) fn apply_offset_single(&mut self, mode: OffsetMode, src_idx: usize, click: Vec2) {
        let Some(src) = self.doc.dobjects.get(src_idx) else {
            return;
        };
        let src_clone = src.clone();
        // Resolve distance + side hint based on mode.
        let (dist, side_hint) = match mode {
            OffsetMode::Distance(d) => (d, click),
            OffsetMode::Through => {
                let d = distance_world_to_geom(&src_clone.geom, click);
                if !d.is_finite() || d.abs() < 1e-9 {
                    self.history.push(
                        "  ! offset (through): point is on the source dobject — skipped".into(),
                    );
                    return;
                }
                (d, click)
            }
        };
        self.snapshot_doc();
        match src_clone.offset(dist, side_hint) {
            Ok(mut new_d) => {
                // Layer flag: Current (default) — keep new_d's layer
                // as whatever the kernel set (which is typically
                // src_clone's). Source — explicitly copy src style
                // (layer + color + linetype). When the kernel preserves
                // source style by default we just override layer for
                // "Current" mode.
                if self.offset_layer_src {
                    new_d.style.layer = src_clone.style.layer;
                } else {
                    new_d.style.layer = self.doc.layers.active;
                }
                let new_idx = self.doc.push(new_d);
                self.offset_applied_count += 1;
                if self.offset_erase {
                    // Delete the source. The new dobject was just
                    // pushed at new_idx; deleting src_idx shifts new_idx
                    // down by 1 if new_idx > src_idx — bookkeeping for
                    // the log only.
                    if src_idx < self.doc.dobjects.len() {
                        self.doc.dobjects.remove(src_idx);
                    }
                    let final_idx = if new_idx > src_idx {
                        new_idx - 1
                    } else {
                        new_idx
                    };
                    self.history.push(format!(
                        "  ⇉ offset {:.4} → #{} (source #{} erased)",
                        dist, final_idx, src_idx
                    ));
                } else {
                    self.history.push(format!(
                        "  ⇉ offset {:.4} → #{} (source #{} kept)",
                        dist, new_idx, src_idx
                    ));
                }
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
            }
            Err(msg) => {
                self.rollback_doc();
                self.history.push(format!("  ! offset: {}", msg));
            }
        }
    }

    fn apply_offset(&mut self, dist: f64, side: Vec2) {
        if self.selection.is_empty() {
            return;
        }
        self.snapshot_doc();
        let mut ok = 0usize;
        let mut errs: Vec<String> = Vec::new();
        let mut new_dobjects: Vec<DObject> = Vec::new();
        for &i in &self.selection {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            match d.offset(dist, side) {
                Ok(new_d) => {
                    new_dobjects.push(new_d);
                    ok += 1;
                }
                Err(msg) => errs.push(format!("#{}: {}", i, msg)),
            }
        }
        for nd in new_dobjects {
            self.doc.push(nd);
        }
        self.history.push(format!(
            "  ⇉ offset {:.3} → {} new dobject(s); {} skipped",
            dist,
            ok,
            errs.len()
        ));
        for e in errs.iter().take(3) {
            self.history.push(format!("    {}", e));
        }
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    pub(super) fn apply_lengthen(&mut self, delta: f64, near: Vec2) {
        if self.selection.is_empty() {
            return;
        }
        self.snapshot_doc();
        let mut ok = 0usize;
        let mut errs: Vec<String> = Vec::new();
        for &i in &self.selection {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            match d.geom.lengthened(delta, near) {
                Ok(new_geom) => {
                    if let Some(d_mut) = self.doc.dobjects.get_mut(i) {
                        d_mut.geom = new_geom;
                        ok += 1;
                    }
                }
                Err(msg) => errs.push(format!("#{}: {}", i, msg)),
            }
        }
        self.history.push(format!(
            "  ⟼ lengthen {:+.3} → {} ok, {} skipped",
            delta,
            ok,
            errs.len()
        ));
        for e in errs.iter().take(3) {
            self.history.push(format!("    {}", e));
        }
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    pub(super) fn apply_break(&mut self, at: Vec2) {
        if self.selection.is_empty() {
            return;
        }
        self.snapshot_doc();
        // Process in reverse-index order so removals don't shift later indices.
        let mut sel = self.selection.clone();
        sel.sort_unstable();
        sel.dedup();
        let mut ok = 0usize;
        let mut errs: Vec<String> = Vec::new();
        let mut adds: Vec<(usize, DObject, DObject, cad_kernel::Style)> = Vec::new();
        for &i in sel.iter().rev() {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            match d.geom.split_at(at) {
                Ok((g1, g2)) => {
                    adds.push((i, DObject::new(g1), DObject::new(g2), d.style));
                    ok += 1;
                }
                Err(msg) => errs.push(format!("#{}: {}", i, msg)),
            }
        }
        // Apply: remove original at i, push both halves with preserved style.
        for (i, mut h1, mut h2, style) in adds {
            if i < self.doc.dobjects.len() {
                self.doc.dobjects.remove(i);
            }
            h1.style = style;
            h2.style = style;
            self.doc.dobjects.push(h1);
            self.doc.dobjects.push(h2);
        }
        self.selection.clear();
        self.selected = None;
        self.history
            .push(format!("  ✂ break: {} split, {} skipped", ok, errs.len()));
        for e in errs.iter().take(3) {
            self.history.push(format!("    {}", e));
        }
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    pub(super) fn apply_align(&mut self, s1: Vec2, s2: Vec2, t1: Vec2, t2: Vec2) {
        if self.selection.is_empty() {
            return;
        }
        let src_len = s1.dist(s2);
        let tgt_len = t1.dist(t2);
        if src_len < EPS {
            self.history
                .push("  ! align: source points coincide".into());
            return;
        }
        if tgt_len < EPS {
            self.history
                .push("  ! align: target points coincide".into());
            return;
        }
        self.snapshot_doc();
        // Three-stage affine: translate s1→t1, rotate around t1 so the
        // (s1→s2) direction aligns with (t1→t2), then uniformly scale
        // around t1 so the source segment maps onto the target segment.
        // AutoCAD's ALIGN with two ref pairs does exactly this.
        let v = t1 - s1;
        let src_dir = (s2 - s1).angle();
        let tgt_dir = (t2 - t1).angle();
        let dtheta = (tgt_dir - src_dir).rem_euclid(std::f64::consts::TAU);
        let dtheta = if dtheta > std::f64::consts::PI {
            dtheta - std::f64::consts::TAU
        } else {
            dtheta
        };
        let scale = tgt_len / src_len;
        let n = self.selection.len();
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                let translated = d.geom.translated(v);
                let rotated = translated.rotated(t1, dtheta);
                d.geom = rotated.scaled(t1, scale);
            }
        }
        self.history.push(format!(
            "  ⇲ align: {} dobject(s)  shifted ({:.2},{:.2})  rotated {:.2}°  scaled ×{:.3}  around ({:.2},{:.2})",
            n, v.x, v.y, dtheta.to_degrees(), scale, t1.x, t1.y));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    pub(super) fn apply_stretch(&mut self, win_min: Vec2, win_max: Vec2, base: Vec2, dest: Vec2) {
        let v = dest - base;
        if v.len() < EPS || self.selection.is_empty() {
            return;
        }
        self.snapshot_doc();
        // Only the SELECTED dobjects are stretched (the crossing window +
        // Shift-exclude during the selection phase decided the set); each
        // one's vertices inside the box move by `v`, the rest stay.
        let sel: Vec<usize> = self.selection.clone();
        // Smart-block authoring: while the Session Recorder runs, capture
        // each stretch with FULL detail — box + vector + every CHANGED
        // dobject's before→after coordinates — so the parametric rule
        // (which dobjects move, by what vector, inside which box) can be
        // extracted from the dump.
        let recording = self.dbg.recording;
        let mut affected: Vec<String> = Vec::new();
        // Block Task Recorder: collect the affected points (BEFORE the
        // stretch, base-relative) so this demonstrated stretch becomes a
        // recorded parametric task.
        let btr_off = self.block_task_rec.as_ref().map(|r| r.world_offset);
        let btr_pts: Vec<Vec2> = if let Some(off) = btr_off {
            let inside = |p: Vec2| {
                p.x >= win_min.x && p.x <= win_max.x && p.y >= win_min.y && p.y <= win_max.y
            };
            sel.iter()
                .filter_map(|&i| self.doc.dobjects.get(i))
                .flat_map(|d| geom_def_points(&d.geom))
                .filter(|p| inside(*p))
                .map(|p| p - off)
                .collect()
        } else {
            Vec::new()
        };
        for &i in &sel {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                let before = d.geom.clone();
                let after = stretch_one(&before, win_min, win_max, v);
                if recording {
                    let b = describe_verbose(&before);
                    let a = describe_verbose(&after);
                    if b != a {
                        affected.push(format!(
                            "#{} h=0x{:X} {}\n              before: {}\n              after : {}",
                            i,
                            d.handle,
                            dobject_kind_name(&before),
                            b,
                            a
                        ));
                    }
                }
                d.geom = after;
            }
        }
        if recording {
            crate::dbg_event!(
                self,
                crate::dbg_recorder::DbgEvent::StretchRecord {
                    box_min: (win_min.x, win_min.y),
                    box_max: (win_max.x, win_max.y),
                    base: (base.x, base.y),
                    dest: (dest.x, dest.y),
                    vector: (v.x, v.y),
                    total_selected: sel.len(),
                    affected,
                }
            );
        }
        // Block Task Recorder: record this stretch as a parametric task,
        // then return control to the recorder by asking for its NAME (the
        // amount stays dynamic — entered at insert).
        if let Some(off) = btr_off {
            let dir = if v.len() > 1e-9 { v / v.len() } else { v };
            let n_pts = btr_pts.len();
            if let Some(rec) = self.block_task_rec.as_mut() {
                rec.recorded.push(ParamRow {
                    win_min: win_min - off,
                    win_max: win_max - off,
                    dir,
                    magnitude: v.len(),
                    dx: v.x,
                    dy: v.y,
                    points: btr_pts,
                    name: String::new(),
                    original: "0".into(),
                    gain: 1.0,
                });
            }
            self.history.push(format!(
                "  ◉ stretch recorded: dir=({:.2},{:.2})  {} point(s)",
                dir.x, dir.y, n_pts
            ));
            self.btr_awaiting_name = true;
            self.set_prompt(
                "Block Task Recorder: type a NAME for this function (e.g. width) — \
                 it auto-continues to the next stretch  ·  `done` or empty Enter = finish  [Esc=cancel]");
            // Don't fall through to the generic stretch summary/clear below.
            self.selection.clear();
            self.intersections.clear();
            self.index_dirty = true;
            self.touch_view();
            return;
        }
        self.history.push(format!(
            "  ↔ stretch by ({:.2},{:.2})  {} dobject(s){}",
            v.x,
            v.y,
            sel.len(),
            if recording { "  [recorded]" } else { "" }
        ));
        self.selection.clear();
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// Recorder "📐 Capture smart dobject" — snapshot the FULL geometry of
    /// the current selection into the timeline. Works on an EXPLODED set
    /// (each dobject's coords) OR a BLOCK (its definition-space contents
    /// are dumped too). With the `✂REC` stretch steps, this is everything
    /// needed to convert the selection into a parametric smart block.
    pub(super) fn capture_smart_geometry(&mut self) {
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
        if targets.is_empty() {
            self.history.push(
                "  ! capture: select the smart dobject(s) first — exploded set or a block".into(),
            );
            return;
        }
        if !self.dbg.recording {
            self.history
                .push("  ! capture: press ▶ Start in the Session Recorder first".into());
            return;
        }
        let mut entries: Vec<String> = Vec::new();
        for &i in &targets {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            match &d.geom {
                Geom::BlockRef(br) => {
                    let bname = self
                        .doc
                        .blocks
                        .get(br.block)
                        .map(|b| b.name.clone())
                        .unwrap_or_else(|| "?".into());
                    let mut s =
                        format!(
                        "#{} h=0x{:X} BlockRef '{}'  insert=({:.3},{:.3}) scale={:.4} rot={:.2}°",
                        i, d.handle, bname,
                        br.insert.x, br.insert.y, br.scale, br.rotation.to_degrees());
                    if let Some(blk) = self.doc.blocks.get(br.block) {
                        s.push_str(&format!(
                            "\n              base=({:.3},{:.3})  {} content dobject(s) [DEFINITION space]:",
                            blk.base.x, blk.base.y, blk.dobjects.len()));
                        for (k, cd) in blk.dobjects.iter().enumerate() {
                            s.push_str(&format!(
                                "\n                [{}] h=0x{:X} {} | {}",
                                k,
                                cd.handle,
                                dobject_kind_name(&cd.geom),
                                describe_verbose(&cd.geom)
                            ));
                        }
                    }
                    entries.push(s);
                }
                other => {
                    entries.push(format!(
                        "#{} h=0x{:X} {} | {}",
                        i,
                        d.handle,
                        dobject_kind_name(other),
                        describe_verbose(other)
                    ));
                }
            }
        }
        let label = format!("{} dobject(s) selected", targets.len());
        let count = entries.len();
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::GeometryCapture { label, entries }
        );
        self.history.push(format!(
            "  📐 captured geometry of {} dobject(s) → recorder timeline",
            count
        ));
    }

    // ---- Trim debug instrumentation ----

    /// Append one line to the trim debug log. Bounded so a runaway
    /// session can't blow memory; oldest entries drop first.
    pub(super) fn trim_dbg<S: Into<String>>(&mut self, msg: S) {
        const CAP: usize = 1000;
        if self.trim_debug_log.len() >= CAP {
            self.trim_debug_log.drain(..200);
        }
        self.trim_debug_log
            .push(format!("[{:>5}] {}", self.trim_debug_frame, msg.into()));
    }

    /// Append a Hatch Debug Log entry.
    ///
    /// ALWAYS RECORDS (HATCH_DEBUG §11). This used to early-return unless the
    /// debug window happened to be open, which meant the ~60 log sites in the
    /// hatch pipeline produced nothing in the one situation they exist for:
    /// a user hits a bug, THEN opens the log — and finds it empty, because
    /// recording only started at that moment. The window is now purely a
    /// viewer over a buffer that has been filling all along. The 1000-line cap
    /// keeps the cost bounded (it is a plain `Vec<String>`, drained in blocks).
    pub(super) fn hatch_dbg<S: Into<String>>(&mut self, msg: S) {
        const CAP: usize = 1000;
        if self.hatch_debug_log.len() >= CAP {
            self.hatch_debug_log.drain(..200);
        }
        self.hatch_debug_log
            .push(format!("[{:>5}] {}", self.trim_debug_frame, msg.into()));
    }

    /// Stamp a phase banner into the hatch log. `n` is the phase number from
    /// the AutoCAD HATCH workflow (1 Init, 2 Pattern, 3 Boundary detection,
    /// 4 Boundary processing, 5 Pattern generation, 6 Preview, 7 Object
    /// creation, 8 Edge cases, 9 Errors, 10 Post-creation) so a log read
    /// top-to-bottom maps onto that reference workflow.
    pub(super) fn hatch_phase(&mut self, n: u8, title: &str) {
        self.hatch_dbg(format!("--- [{}] {} ---", n, title));
    }

    /// Record that a workflow phase AutoCAD performs has no counterpart here,
    /// so a log reader can tell "we skipped this" apart from "this failed".
    pub(super) fn hatch_phase_absent(&mut self, n: u8, what: &str) {
        self.hatch_dbg(format!("    [{}] not implemented: {}", n, what));
    }

    /// Auto-open the Hatch Debug window at the start of a fresh `hatch`
    /// command and stamp a session-start marker. Mirrors
    /// `trim_dbg_session_start`. Does NOT clear the prior log — the
    /// user can hit 🗑 Clear themselves if they want a fresh slate.
    pub(super) fn hatch_dbg_session_start(&mut self) {
        self.hatch_debug_open = true;
        self.hatch_dbg(format!(
            "=== HATCH session START ===  doc.dobjects.len() = {}, selection.len() = {}",
            self.doc.dobjects.len(),
            self.selection.len()
        ));

        // Phase 1 — command initialisation. AutoCAD captures the current
        // drafting settings here; record the same set so a report says which
        // layer/colour/units the hatch was created under.
        self.hatch_phase(1, "command init — drafting settings captured");
        let layer_name = self
            .doc
            .layers
            .get(self.doc.layers.active)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| "?".into());
        let (vis, frozen, locked) = self
            .doc
            .layers
            .get(self.doc.layers.active)
            .map(|l| (l.visible, l.frozen, l.locked))
            .unwrap_or((true, false, false));
        self.hatch_dbg(format!(
            "    layer='{}' (visible={}, frozen={}, locked={}), color={:?}, units='{}'",
            layer_name, vis, frozen, locked, self.doc.current_color, self.doc.units.name
        ));
        let n_pat = cad_kernel::patterns::PATTERN_NAMES.len();
        self.hatch_dbg(format!(
            "    pattern catalog: {} built-in name(s), hardcoded (no .pat file loaded)",
            n_pat
        ));
        self.hatch_phase_absent(
            1,
            "external .pat loading (acad.pat / acadiso.pat) \
                                    — cad_io::pat parser exists but is not wired",
        );
        self.hatch_phase_absent(1, "UCS / elevation (2D app — world coordinates only)");
    }

    /// Wipe + open the debug log at the start of a fresh trim/extend
    /// session. Called from the command handlers.
    pub(super) fn trim_dbg_session_start(&mut self, op: &str) {
        self.trim_debug_log.clear();
        self.trim_debug_open = true;
        self.trim_dbg(format!("=== {} session START ===", op));
        self.trim_dbg(format!("  pre_op_selection = {:?}", self.pre_op_selection));
        self.trim_dbg(format!(
            "  doc.dobjects.len() = {}  EdgMod = {}",
            self.doc.dobjects.len(),
            self.env.EdgMod
        ));
        // FULL DOC SNAPSHOT — every dobject with its index + full
        // geometry. Lets the user reproduce a trim bug exactly: the
        // log says "trim of #5 with cutters [0, 5] @ click X" and
        // here are the EXACT coordinates of every dobject involved.
        // Auto-trimmed lists are useless without the inputs.
        let dump: Vec<String> = self
            .doc
            .dobjects
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let vis = if d.style.visible { "vis" } else { "HID" };
                format!(
                    "    #{:02} [{}] L{} {}",
                    i,
                    vis,
                    d.style.layer,
                    describe_verbose(&d.geom)
                )
            })
            .collect();
        self.trim_dbg("  --- doc snapshot at session start ---".to_string());
        for line in dump {
            self.trim_dbg(line);
        }
    }

    /// Dump one dobject's full state to the trim log. Used at
    /// cutter-capture and target-click points so a bug report has
    /// the EXACT coordinates of every input — no need to re-derive
    /// from screen pixels or re-open the file.
    pub(super) fn trim_dbg_dobject(&mut self, idx: usize, role: &str) {
        if let Some(d) = self.doc.dobjects.get(idx) {
            let vis = if d.style.visible { "vis" } else { "HID" };
            let line = format!(
                "    {} #{} [{}] L{} {}",
                role,
                idx,
                vis,
                d.style.layer,
                describe_verbose(&d.geom)
            );
            self.trim_dbg(line);
        } else {
            self.trim_dbg(format!("    {} #{} <out of range>", role, idx));
        }
    }

    /// Format a Vec2 compactly for log entries.
    pub(super) fn fmt_v(v: Vec2) -> String {
        format!("({:.3},{:.3})", v.x, v.y)
    }

    // ---- Slice M.1 / M.2: trim / extend apply methods ----

    /// Returns true iff the trim actually mutated the document. The
    /// caller uses this to gate cutter-list index patching — patching
    /// when the doc didn't change corrupts the list silently (the bug
    /// the user caught in commit ae54eef's debug log).
    #[track_caller]
    /// Flatten a dobject's geom into the EFFECTIVE cutting/boundary
    /// geometry the intersection math can actually use. A `BlockRef` has
    /// an empty `intersect`, so a block picked as a cutter/boundary would
    /// never cut anything; instead we explode it IN MEMORY ONLY —
    /// transform each contained dobject into world space (recursively for
    /// nested blocks) and push the real lines/arcs/… The document is never
    /// modified; this is the user's "background explode → temp memo → do
    /// the math" algorithm. Everything else passes through as itself.
    fn expand_cutter_geoms(&self, g: &Geom, out: &mut Vec<Geom>, depth: u8) {
        if depth > 8 {
            return;
        } // matches draw_blockref's cycle/depth cap
        match g {
            Geom::BlockRef(br) => {
                if let Some(blk) = self.doc.blocks.get(br.block) {
                    // Parametric blocks: derive the contents at this
                    // instance's values BEFORE transforming, so trim/snap
                    // see the actual (stretched) geometry.
                    let derived = self.block_derived_geoms(blk, &br.param_values);
                    for dg in &derived {
                        let tg = br.transform_geom(dg, blk.base);
                        self.expand_cutter_geoms(&tg, out, depth + 1);
                    }
                }
            }
            other => out.push(other.clone()),
        }
    }

    /// Definition-space geoms of a block with its parametric `params`
    /// applied for the given per-instance `param_values` — parallel to
    /// `blk.dobjects` (same order/len). Each param is a `stretch_one` over
    /// its window by `dir·displacement(value)`. Non-parametric blocks
    /// (empty `params`) return the geoms unchanged. This is the single
    /// derive used by render / bbox / trim / snap / explode.
    /// Cursor → value for the smart parameter `idx` during LIVE insertion:
    /// project (cursor − base) onto the parameter's first vector direction
    /// (rotated by the instance rotation, de-scaled), added to `original`. So
    /// dragging away from the base along the parameter's direction grows it.
    pub(super) fn live_param_value(&self, live: &InsertLive, cursor: Vec2) -> f64 {
        let Some(blk) = self.doc.blocks.get(live.block) else {
            return 0.0;
        };
        let Some(p) = blk.params.get(live.idx) else {
            return 0.0;
        };
        let dir = p
            .vectors
            .first()
            .map(|v| v.dir)
            .unwrap_or(Vec2::new(1.0, 0.0));
        let (c, s) = (live.rotation.cos(), live.rotation.sin());
        let dw = Vec2::new(dir.x * c - dir.y * s, dir.x * s + dir.y * c);
        let l = dw.len();
        if l < 1e-9 {
            return p.original;
        }
        let u = dw / l;
        p.original + (cursor - live.insert).dot(u) / live.scale.max(1e-9)
    }

    pub(super) fn block_derived_geoms(
        &self,
        blk: &cad_kernel::Block,
        param_values: &[f64; cad_kernel::MAX_BLOCK_PARAMS],
    ) -> Vec<Geom> {
        blk.dobjects
            .iter()
            .map(|cd| {
                let mut g = cd.geom.clone();
                for (k, p) in blk.params.iter().enumerate() {
                    let value = param_values.get(k).copied().unwrap_or(p.original);
                    // displacement = dir * (value - original); all linked
                    // vectors move by the same (value - original) amount.
                    let amount = value - p.original;
                    for v in &p.vectors {
                        // `gain` lets linked vectors share one value with
                        // different magnitudes (e.g. 0.5 each for a centered
                        // opening). See `ParamVector::gain`.
                        let disp = v.dir * (v.gain * amount);
                        if disp.len() > 1e-12 {
                            g = stretch_one(&g, v.win_min, v.win_max, disp);
                        }
                    }
                }
                g
            })
            .collect()
    }

    /// Temporary DObjects for the geometry INSIDE every visible block
    /// instance, so object-snap finds END / MID / CEN / … on block
    /// contents (snap-through). Same in-memory explode as trim's
    /// `expand_cutter_geoms`; the document is never touched. Expands all
    /// visible blocks each hover frame — fine for normal drawings;
    /// viewport-gate if it ever shows up in a profile.
    pub(super) fn block_snap_phantoms(&self) -> Vec<DObject> {
        let mut geoms: Vec<Geom> = Vec::new();
        for d in &self.doc.dobjects {
            if matches!(d.geom, Geom::BlockRef(_))
                && d.style.visible
                && self.doc.layers.renders(d.style.layer)
            {
                self.expand_cutter_geoms(&d.geom, &mut geoms, 0);
            }
        }
        geoms.into_iter().map(DObject::new).collect()
    }

    /// Paint the translucent "shade" of the block currently being inserted —
    /// it follows the cursor during `WaitingForPoint`, then pivots about the
    /// chosen insertion point during `WaitingForAngle`. Purely visual (no state
    /// change): resolves the block definition at the live pose via
    /// `expand_cutter_geoms` and strokes it dashed/ghosted. (HSI's insert had
    /// no preview at all — the block only appeared after the final click.)
    pub(super) fn paint_insert_preview(&self, painter: &egui::Painter, rect: egui::Rect) {
        let (block, insert, rotation, scale, pv) = match self.insert_state {
            InsertState::WaitingForPoint { block } => {
                let Some(p) = painter.ctx().input(|i| i.pointer.hover_pos()) else {
                    return;
                };
                // Reflect the dialog's scale/rotation/params in the ghost.
                let (scale, rot, pv) = self
                    .pending_insert
                    .as_ref()
                    .map(|pd| (pd.scale, pd.rotation, pd.param_values))
                    .unwrap_or((1.0, 0.0, [0.0; cad_kernel::MAX_BLOCK_PARAMS]));
                (block, self.s2w(p, rect), rot, scale, pv)
            }
            InsertState::WaitingForAngle { block, insert } => {
                let rot = painter
                    .ctx()
                    .input(|i| i.pointer.hover_pos())
                    .map(|p| (self.s2w(p, rect) - insert).angle())
                    .unwrap_or(0.0);
                (block, insert, rot, 1.0, [0.0; cad_kernel::MAX_BLOCK_PARAMS])
            }
            InsertState::Off => return,
        };
        let s = if scale.abs() < 1e-6 { 1.0 } else { scale.abs() };
        let preview = Geom::BlockRef(cad_kernel::BlockRef {
            block,
            insert,
            attr_values: Vec::new(),
            scale: s,
            scale_y: s,
            rotation,
            mirror_x: false,
            param_values: pv,
        });
        let mut geoms: Vec<Geom> = Vec::new();
        self.expand_cutter_geoms(&preview, &mut geoms, 0);
        let ghost = egui::Color32::from_rgba_unmultiplied(150, 200, 255, 140);
        for g in &geoms {
            draw_dobject_dashed(painter, rect, self, g, ghost, 6.0, 4.0);
        }
        let ip = self.w2s(insert, rect);
        painter.circle_stroke(ip, 4.0, egui::Stroke::new(1.0, ghost));
    }

    /// Draw the live-deforming ghost during parametric insertion: the parameter
    /// being set takes its value from the cursor, the block is RE-DERIVED
    /// (`block_derived_geoms`) at that value, transformed by the insert pose, and
    /// stroked dashed — so a door frame grows under the cursor.
    pub(super) fn paint_insert_live(&self, painter: &egui::Painter, rect: egui::Rect) {
        let Some(live) = &self.insert_live else {
            return;
        };
        let Some(cur) = painter.ctx().input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let cursor = self.s2w(cur, rect);
        let mut values = live.values.clone();
        if live.idx < values.len() {
            values[live.idx] = self.live_param_value(live, cursor);
        }
        let mut pv = [0.0; cad_kernel::MAX_BLOCK_PARAMS];
        for (k, v) in values.iter().enumerate() {
            if k < cad_kernel::MAX_BLOCK_PARAMS {
                pv[k] = *v;
            }
        }
        let Some(blk) = self.doc.blocks.get(live.block) else {
            return;
        };
        let base = blk.base;
        let derived = self.block_derived_geoms(blk, &pv);
        let br = cad_kernel::BlockRef {
            block: live.block,
            insert: live.insert,
            scale: live.scale,
            scale_y: live.scale,
            rotation: live.rotation,
            mirror_x: false,
            param_values: pv,
            attr_values: Vec::new(),
        };
        let ghost = egui::Color32::from_rgba_unmultiplied(150, 200, 255, 180);
        for g in &derived {
            let gw = br.transform_geom(g, base);
            draw_dobject_dashed(painter, rect, self, &gw, ghost, 6.0, 4.0);
        }
        let ip = self.w2s(live.insert, rect);
        painter.circle_stroke(ip, 4.0, egui::Stroke::new(1.0, ghost));
        if live.idx < values.len() {
            painter.text(
                cur + egui::vec2(10.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{:.3}", values[live.idx]),
                egui::FontId::proportional(12.0),
                ghost,
            );
        }
    }

    /// Trim-FENCE: trim every dobject the fence segment p→q crosses, at the
    /// crossing point (the piece the fence passes through is removed). Reuses
    /// `apply_trim_pick` per crossing and re-remaps the stored cutter list
    /// after each cut (same patch the single-click target flow does), so
    /// stored cutter indices stay valid across several trims.
    pub(super) fn apply_trim_fence(&mut self, p: Vec2, q: Vec2) {
        let all_mode = matches!(self.trim_state, TrimState::PickingTargetsAll);
        let fence = Geom::Line(cad_kernel::Line { a: p, b: q });
        // Crossing points of the fence segment with the CURRENT geometry.
        let mut picks: Vec<Vec2> = Vec::new();
        for i in 0..self.doc.dobjects.len() {
            for ip in intersect(&self.doc.dobjects[i].geom, &fence) {
                picks.push(ip);
            }
        }
        let tol = 8.0 / self.scale as f64;
        let mut cuts = 0usize;
        for pick in picks {
            // cutters: dynamic in all-mode; the (remapped) stored list otherwise.
            let cutters: Vec<usize> = if all_mode {
                (0..self.doc.dobjects.len()).collect()
            } else if let TrimState::PickingTargets(c) = &self.trim_state {
                c.clone()
            } else {
                break;
            };
            // Re-resolve the target by position — indices shift as we trim.
            let Some(tgt) = self.nearest_entity_under(pick, tol) else {
                continue;
            };
            let n_before = self.doc.dobjects.len();
            let tgt_was_cutter = cutters.contains(&tgt);
            // A HATCH trim only APPENDS a hidden hole boundary (no removal /
            // reorder), so the cutter indices stay valid — skip the patch.
            let tgt_was_hatch = matches!(
                self.doc.dobjects.get(tgt).map(|d| &d.geom),
                Some(Geom::Hatch(_))
            );
            let did = self.apply_trim_pick(&cutters, tgt, pick);
            let n_after = self.doc.dobjects.len();
            if did && !all_mode && !tgt_was_hatch {
                let n_pieces = n_after + 1 - n_before;
                let first_new = n_after - n_pieces;
                if let TrimState::PickingTargets(c) = &mut self.trim_state {
                    c.retain(|&i| i != tgt);
                    for c_i in c.iter_mut() {
                        if *c_i > tgt {
                            *c_i -= 1;
                        }
                    }
                    if tgt_was_cutter && n_pieces > 0 {
                        c.extend(first_new..n_after);
                    }
                }
            }
            if did {
                cuts += 1;
            }
        }
        self.history
            .push(format!("  trim (fence): {} cut(s)", cuts));
    }

    /// TRIM by a WINDOW/CROSSING drag in the target phase: the drag box acts as
    /// a rectangular fence — every object crossing any of its FOUR edges is
    /// trimmed at the crossing, reusing `apply_trim_fence` per edge. Window
    /// (L→R) vs crossing (R→L) doesn't change what's removed (owner choice: the
    /// part the box crosses); only the rubber-band colour differs.
    pub(super) fn apply_trim_window(&mut self, p1: Vec2, p2: Vec2) {
        let (x0, x1) = (p1.x.min(p2.x), p1.x.max(p2.x));
        let (y0, y1) = (p1.y.min(p2.y), p1.y.max(p2.y));
        let bl = Vec2::new(x0, y0);
        let br = Vec2::new(x1, y0);
        let tr = Vec2::new(x1, y1);
        let tl = Vec2::new(x0, y1);
        for (a, b) in [(bl, br), (br, tr), (tr, tl), (tl, bl)] {
            self.apply_trim_fence(a, b);
        }
    }

    /// EXTEND-FENCE: extend every dobject the fence segment p→q crosses, growing
    /// the end nearest the crossing toward the boundary edges. Mirrors
    /// `apply_trim_fence`, but extend mutates dobjects IN PLACE (no
    /// removal/append), so there's no cutter-index bookkeeping — the boundary
    /// list stays valid across every extend.
    pub(super) fn apply_extend_fence(&mut self, p: Vec2, q: Vec2) {
        let all_mode = matches!(self.extend_state, ExtendState::PickingTargetsAll);
        let fence = Geom::Line(cad_kernel::Line { a: p, b: q });
        // Crossing points of the fence segment with the CURRENT geometry.
        let mut picks: Vec<Vec2> = Vec::new();
        for i in 0..self.doc.dobjects.len() {
            for ip in intersect(&self.doc.dobjects[i].geom, &fence) {
                picks.push(ip);
            }
        }
        let tol = 8.0 / self.scale as f64;
        let mut n = 0usize;
        for pick in picks {
            let bounds: Vec<usize> = if all_mode {
                (0..self.doc.dobjects.len()).collect()
            } else if let ExtendState::PickingTargets(b) = &self.extend_state {
                b.clone()
            } else {
                break;
            };
            // Re-resolve by position (extend never shifts indices, but a target
            // may fall out of tolerance after growing — keep it defensive).
            let Some(tgt) = self.nearest_entity_under(pick, tol) else {
                continue;
            };
            if self.apply_extend_pick(&bounds, tgt, pick) {
                n += 1;
            }
        }
        self.history
            .push(format!("  extend (fence): {} extended", n));
    }

    /// EXTEND by a WINDOW/CROSSING drag box in the target phase: the box edges
    /// act as a rectangular fence, extending every object the box crosses.
    /// Mirrors `apply_trim_window`; window (L→R) vs crossing (R→L) only changes
    /// the rubber-band colour, not what gets extended.
    pub(super) fn apply_extend_window(&mut self, p1: Vec2, p2: Vec2) {
        let (x0, x1) = (p1.x.min(p2.x), p1.x.max(p2.x));
        let (y0, y1) = (p1.y.min(p2.y), p1.y.max(p2.y));
        let bl = Vec2::new(x0, y0);
        let br = Vec2::new(x1, y0);
        let tr = Vec2::new(x1, y1);
        let tl = Vec2::new(x0, y1);
        for (a, b) in [(bl, br), (br, tr), (tr, tl), (tl, bl)] {
            self.apply_extend_fence(a, b);
        }
    }

    pub(super) fn apply_trim_pick(&mut self, cutters: &[usize], target_idx: usize, pick: Vec2) -> bool {
        let before_dobj_count = self.doc.dobjects.len();
        let _trim_pick_dbg = (cutters.to_vec(), target_idx, pick);
        // Snapshot ONCE per click so undo rolls back this single trim.
        self.snapshot_doc();
        let edge_mode = self.env.EdgMod;
        // Build cutter geoms, EXCLUDING the target itself — a dobject
        // never cuts itself (self-intersection = 0). This is what allows
        // trimming a cutter dobject in the basket: it's still a valid
        // target, just doesn't intersect with itself for cut math.
        // BlockRef cutters are exploded in-memory into their real contents
        // (see `expand_cutter_geoms`) so geometry INSIDE a block can cut.
        let mut cutter_geoms: Vec<Geom> = Vec::new();
        for &i in cutters.iter().filter(|&&i| i != target_idx) {
            if let Some(d) = self.doc.dobjects.get(i) {
                self.expand_cutter_geoms(&d.geom, &mut cutter_geoms, 0);
            }
        }
        // A POLYLINE target with no other cutters is still valid: the clicked
        // segment meets no boundary, so it should be REMOVED (the kernel splits
        // the polyline at that segment). Only non-polyline targets need a real
        // cutter — for a Line/Arc "no cutters" means nothing to do.
        let target_is_polyline = matches!(
            self.doc.dobjects.get(target_idx).map(|d| &d.geom),
            Some(Geom::Polyline(_))
        );
        if cutter_geoms.is_empty() && !target_is_polyline {
            // No OTHER dobjects to cut against → roll back, fail.
            self.rollback_doc();
            self.history.push(format!(
                "  ! trim #{}: no other cutters available (target is the only candidate)",
                target_idx
            ));
            return false;
        }
        let Some(target) = self.doc.dobjects.get(target_idx) else {
            self.rollback_doc();
            return false;
        };
        let target_style = target.style;
        match target.geom.trim_at(&cutter_geoms, pick, edge_mode) {
            Ok(pieces) => {
                // Verify + JOIN the break parts: a closed circle/ellipse is
                // over-split at every cut point; re-merge the consecutive
                // survivors that still TOUCH so we keep the natural run(s)
                // (gaps from removed arcs are preserved). See join_trim_survivors.
                let pieces = cad_kernel::join_trim_survivors(pieces);
                let n_pieces = pieces.len();
                self.doc.dobjects.remove(target_idx);
                for g in pieces {
                    let mut d = DObject::new(g);
                    d.style = target_style;
                    self.doc.push(d);
                }
                self.history.push(format!(
                    "  ✂ trim: #{} cut → {} piece(s) survive (EdgMod {})",
                    target_idx,
                    n_pieces,
                    if edge_mode { "ON" } else { "OFF" }
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
                crate::dbg_event!(
                    self,
                    crate::dbg_recorder::DbgEvent::ApplyOp {
                        name: "apply_trim_pick".into(),
                        before_dobj_count,
                        after_dobj_count: self.doc.dobjects.len(),
                        success: true,
                        detail: format!(
                            "target #{}, cutters={:?}, pick=({:.3},{:.3}), n_pieces={}, EdgMod={}",
                            target_idx, cutters, pick.x, pick.y, n_pieces, edge_mode
                        ),
                    }
                );
                true
            }
            Err(msg) => {
                self.rollback_doc();
                // Surface the kernel error in BOTH history and the trim
                // debug log — the user needs the actual reason to diagnose
                // "trim silently fails" (the bug your 268-cutter log
                // exposed: every retry returned Err but the log only said
                // 'success=false', not which Err).
                let kind = match self.doc.dobjects.get(target_idx).map(|d| &d.geom) {
                    Some(Geom::Line(_)) => "Line",
                    Some(Geom::Circle(_)) => "Circle",
                    Some(Geom::Arc(_)) => "Arc",
                    Some(Geom::Ellipse(_)) => "Ellipse",
                    Some(Geom::EllipseArc(_)) => "EllipseArc",
                    Some(Geom::Polyline(_)) => "Polyline",
                    Some(Geom::Point(_)) => "Point",
                    Some(Geom::Hatch(_)) => "Hatch",
                    Some(Geom::Spline(_)) => "Spline",
                    Some(Geom::Wall(_)) => "Wall",
                    Some(Geom::Text(_)) => "Text",
                    Some(Geom::Dimension(_)) => "Dimension",
                    Some(Geom::BlockRef(_)) => "BlockRef",
                    Some(_) => "Other",
                    None => "<gone>",
                };
                self.history
                    .push(format!("  ! trim #{}: {}", target_idx, msg));
                self.trim_dbg(format!(
                    "  ! trim_at Err on #{} ({}): {}",
                    target_idx, kind, msg
                ));
                crate::dbg_event!(
                    self,
                    crate::dbg_recorder::DbgEvent::ApplyOp {
                        name: "apply_trim_pick".into(),
                        before_dobj_count,
                        after_dobj_count: self.doc.dobjects.len(),
                        success: false,
                        detail: format!("target #{} ({}) Err: {}", target_idx, kind, msg),
                    }
                );
                false
            }
        }
    }

    /// Returns true iff the extend actually mutated the document.
    pub(super) fn apply_extend_pick(&mut self, bounds: &[usize], target_idx: usize, pick: Vec2) -> bool {
        self.snapshot_doc();
        let edge_mode = self.env.EdgMod;
        // Same self-exclusion rule as trim. BlockRef boundaries are
        // exploded in-memory so geometry INSIDE a block can be a boundary.
        let mut boundary_geoms: Vec<Geom> = Vec::new();
        for &i in bounds.iter().filter(|&&i| i != target_idx) {
            if let Some(d) = self.doc.dobjects.get(i) {
                self.expand_cutter_geoms(&d.geom, &mut boundary_geoms, 0);
            }
        }
        if boundary_geoms.is_empty() {
            self.rollback_doc();
            self.history.push(format!(
                "  ! extend #{}: no other boundaries available",
                target_idx
            ));
            return false;
        }
        let Some(target) = self.doc.dobjects.get(target_idx) else {
            self.rollback_doc();
            return false;
        };
        match target.geom.extend_to(&boundary_geoms, pick, edge_mode) {
            Ok(new_geom) => {
                if let Some(d) = self.doc.dobjects.get_mut(target_idx) {
                    d.geom = new_geom;
                }
                self.history.push(format!(
                    "  ⟼ extend: #{} extended to boundary (EdgMod {})",
                    target_idx,
                    if edge_mode { "ON" } else { "OFF" }
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
                true
            }
            Err(msg) => {
                self.rollback_doc();
                self.history
                    .push(format!("  ! extend #{}: {}", target_idx, msg));
                false
            }
        }
    }

    /// The tall logo column at the left of the top bar — spans the Quick
    /// Access row + the menu-category row. Shows the brand PNG if present,
    /// else a painted placeholder.
    pub(super) fn draw_logo_column(&mut self, ui: &mut egui::Ui) {
        let size = egui::vec2(60.0, 56.0);
        let (resp, painter) = ui.allocate_painter(size, egui::Sense::hover());
        let rect = resp.rect;
        // No custom fill or divider — the logo sits flush on the toolbar
        // panel background.
        // Lazy-load the logo texture once.
        if self.logo_tex.is_none() && !self.logo_load_tried {
            self.logo_tex = load_logo_texture(ui.ctx());
            self.logo_load_tried = true;
        }
        if let Some(tex) = &self.logo_tex {
            let avail = rect.shrink(5.0);
            let img = tex.size_vec2();
            let scale = (avail.width() / img.x).min(avail.height() / img.y);
            let draw = img * scale;
            let img_rect = egui::Rect::from_center_size(rect.center(), draw);
            painter.image(
                tex.id(),
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            draw_logo_placeholder(&painter, rect);
        }
    }

    /// Run a Quick Access Toolbar shortcut.
    pub(super) fn run_qat_action(&mut self, act: QatAction) {
        match act {
            QatAction::New => self.run_command("clear"),
            QatAction::Open => self.open_file_dialog(FileDialogMode::Open, ".rsm"),
            QatAction::Save => self.do_save_current(),
            QatAction::SaveAs => self.open_file_dialog(FileDialogMode::Save, ".rsm"),
            QatAction::Undo => self.run_command("undo"),
            QatAction::Redo => self.run_command("redo"),
        }
    }

    /// The "Quick access" drop window — a narrow, neat list of actions, each
    /// with a leading check when it's on the toolbar. Clicking a row toggles
    /// it. No close button: ESC or a click outside dismisses it.
    pub(super) fn qat_customize_window(&mut self, ctx: &egui::Context) {
        if !self.qat_customize_open {
            return;
        }
        let item_size = 13.0;
        let item_font = egui::FontId::proportional(item_size);
        let accent = egui::Color32::from_rgb(120, 180, 235);
        let text_c = egui::Color32::from_rgb(205, 216, 228);
        // Width = check column + the longest label (or the title) + padding.
        let check_col = 22.0;
        let label_w = |s: &str, sz: f32| {
            ctx.fonts(|f| {
                f.layout_no_wrap(s.to_string(), egui::FontId::proportional(sz), text_c)
                    .size()
                    .x
            })
        };
        let longest = QatAction::all()
            .iter()
            .map(|a| label_w(a.label(), item_size))
            .fold(0.0_f32, f32::max);
        let title_w = label_w("Quick access", item_size * 1.2);
        let content_w = (check_col + longest + 6.0).max(title_w);

        let bg = egui::Color32::from_rgb(45, 54, 66);
        let frame = egui::Frame::popup(&ctx.style())
            .fill(bg)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 80, 95)))
            .rounding(0.0) // right-angle corners
            .shadow(egui::epaint::Shadow {
                // drop shadow to bottom-right
                offset: egui::vec2(4.0, 4.0),
                blur: 10.0,
                spread: 0.0,
                color: egui::Color32::from_black_alpha(120),
            })
            .inner_margin(egui::Margin::symmetric(8.0, 6.0));
        // Open directly beneath the chevron.
        let pos = self
            .qat_chevron_rect
            .map(|r| egui::pos2(r.left(), r.bottom() + 2.0))
            .unwrap_or(egui::pos2(60.0, 44.0));
        let win = egui::Window::new("qat_quick_access")
            .title_bar(false)
            .resizable(false)
            .frame(frame)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui.set_width(content_w);
                // Title — 20% bigger than the list items.
                ui.label(
                    egui::RichText::new("Quick access")
                        .size(item_size * 1.2)
                        .color(egui::Color32::from_rgb(210, 220, 232)),
                );
                ui.add_space(3.0);
                ui.separator(); // single separation line
                ui.add_space(3.0);
                for act in QatAction::all() {
                    let on = self.qat_actions.contains(&act);
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 20.0),
                        egui::Sense::click(),
                    );
                    if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, 3.0, egui::Color32::from_rgb(58, 70, 86));
                    }
                    // Painted checkmark (the font lacks a ✓ glyph).
                    if on {
                        let cy = rect.center().y;
                        let cx = rect.left() + 7.0;
                        let s = egui::Stroke::new(1.8, accent);
                        ui.painter().line_segment(
                            [egui::pos2(cx, cy + 1.0), egui::pos2(cx + 3.0, cy + 4.0)],
                            s,
                        );
                        ui.painter().line_segment(
                            [
                                egui::pos2(cx + 3.0, cy + 4.0),
                                egui::pos2(cx + 8.0, cy - 4.0),
                            ],
                            s,
                        );
                    }
                    ui.painter().text(
                        egui::pos2(rect.left() + check_col, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        act.label(),
                        item_font.clone(),
                        text_c,
                    );
                    if resp.clicked() {
                        if on {
                            self.qat_actions.retain(|a| *a != act);
                        } else {
                            // Re-insert keeping canonical order.
                            self.qat_actions = QatAction::all()
                                .into_iter()
                                .filter(|a| *a == act || self.qat_actions.contains(a))
                                .collect();
                        }
                    }
                }
            });
        // Dismiss on ESC, or on a click outside the drop window. Skip the
        // outside-click on the very frame it opened (that press was the
        // chevron itself, which lives outside the window rect).
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.qat_customize_open = false;
        } else if self.qat_just_opened {
            self.qat_just_opened = false;
        } else if let Some(win) = win {
            let rect = win.response.rect;
            let clicked_outside = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
            });
            if clicked_outside {
                self.qat_customize_open = false;
            }
        }
    }

    // ---------------------------------------------------------------------
    // USER-ENVIRONMENT SETTINGS window — registry-driven (varreg::VARS).
    // Left sidebar = sections; right panel = typed rows with status badges.
    // Every variable is shown; WIRED ones are editable (through the shared
    // validated varreg::env_set), the rest are disabled (Option A).
    // ---------------------------------------------------------------------
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let sections = crate::varreg::sections();
        if self.settings_section.is_empty() {
            if let Some(first) = sections.first() {
                self.settings_section = first.to_string();
            }
        }
        let mut keep = true;
        egui::Window::new("USER-ENVIRONMENT SETTINGS")
            .open(&mut keep)
            .resizable(true)
            .default_size([840.0, 600.0])
            .default_pos(egui::pos2(40.0, 70.0))
            .show(ctx, |ui| {
                // Reserve the footer row at the BOTTOM, then let the body
                // (sidebar + rows) fill ALL the remaining height — so the
                // lists grow/shrink with the window, there's no blank gap, and
                // the window resizes freely (no fixed inner heights forcing a
                // minimum). Clamp guards against egui's first-frame huge value.
                let footer_h = 34.0;
                let body_h = (ui.available_height() - footer_h).clamp(140.0, 4000.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), body_h), |ui| {
                    ui.horizontal_top(|ui| {
                        // ---- left sidebar: section list ---------------------
                        ui.vertical(|ui| {
                            ui.set_width(210.0);
                            egui::ScrollArea::vertical().id_salt("set_sections")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for sec in &sections {
                                        let count = crate::varreg::VARS.iter()
                                            .filter(|v| v.section == *sec).count();
                                        let selected = self.settings_section == *sec;
                                        if settings_section_item(ui, sec, count, selected) {
                                            self.settings_section = sec.to_string();
                                        }
                                    }
                                });
                        });
                        ui.separator();
                        // ---- right panel: rows for the selected section -----
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.heading(egui::RichText::new(&self.settings_section).size(15.0));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    settings_legend(ui);
                                });
                            });
                            ui.label(egui::RichText::new(format!(
                                "{} variables  ·  edits to Active/Planned vars persist to ~/.config/rust_cad/user_env.txt",
                                crate::varreg::VARS.iter().filter(|v| v.section == self.settings_section).count()))
                                .font(crate::theme::typ::hint()).color(egui::Color32::from_rgb(140, 150, 165)));
                            ui.separator();
                            let sec = self.settings_section.clone();
                            egui::ScrollArea::vertical().id_salt("set_rows")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for v in crate::varreg::VARS.iter().filter(|v| v.section == sec) {
                                        self.settings_row(ui, v);
                                    }
                                });
                        });
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Save now").clicked() {
                        match self.env.save() {
                            Ok(_)  => self.history.push("  settings saved".into()),
                            Err(e) => self.history.push(format!("  ! settings save failed: {}", e)),
                        }
                    }
                    if ui.button("Reload from disk").clicked() { self.env = UserEnv::load(); }
                    if ui.button("Reset to defaults").clicked() {
                        self.env = UserEnv::default();
                        let _ = self.env.save();
                    }
                });
            });
        if !keep {
            self.settings_open = false;
        }
    }

    /// One settings row: name · description · status badge · typed input.
    fn settings_row(&mut self, ui: &mut egui::Ui, v: &crate::varreg::Var) {
        let frame = egui::Frame::none()
            .fill(egui::Color32::from_rgb(28, 31, 40))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(38, 42, 51)))
            .rounding(4.0)
            .inner_margin(egui::Margin::symmetric(8.0, 5.0));
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                // name (mono, fixed width)
                ui.add_sized(
                    [86.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new(v.name)
                            .monospace()
                            .strong()
                            .color(egui::Color32::from_rgb(220, 226, 234)),
                    )
                    .truncate(),
                );
                // description (flex)
                ui.add_sized(
                    [300.0, 18.0],
                    egui::Label::new(
                        egui::RichText::new(v.desc)
                            .size(11.5)
                            .color(egui::Color32::from_rgb(155, 162, 174)),
                    )
                    .truncate(),
                );
                settings_status_badge(ui, v.status);
                // input — right aligned
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(v.wired, |ui| {
                        self.settings_input(ui, v);
                    });
                });
            });
        });
        ui.add_space(3.0);
    }

    /// The typed control for one variable. WIRED → reads/writes the live value
    /// through varreg::env_get / env_set (validated). UNWIRED → shows the
    /// default (the enclosing add_enabled_ui(false) greys it out).
    fn settings_input(&mut self, ui: &mut egui::Ui, v: &crate::varreg::Var) {
        use crate::varreg::Kind;
        let cur = if v.wired {
            crate::varreg::env_get(&self.env, v.name).unwrap_or_default()
        } else {
            v.default.to_string()
        };
        let mut changed: Option<String> = None;
        match v.kind {
            Kind::Bool => {
                let mut b = matches!(cur.as_str(), "on" | "true" | "1");
                let label = if b { "On" } else { "Off" };
                if ui.checkbox(&mut b, label).changed() {
                    changed = Some(if b { "true" } else { "false" }.into());
                }
            }
            Kind::U8 { min, max } => {
                let mut n: u8 = cur.parse().unwrap_or(min);
                // Expressions commit too, but an integer setting must come
                // out WHOLE — no silent rounding of `2.5*2`.
                let calc = &self.calc;
                if ui
                    .add(
                        egui::DragValue::new(&mut n)
                            .update_while_editing(false)
                            .range(min..=max)
                            .custom_parser(move |s| {
                                crate::calc::parse_drag_int(calc, s, min as i64, max as i64)
                                    .map(|n| n as f64)
                            }),
                    )
                    .changed()
                {
                    changed = Some(n.to_string());
                }
            }
            Kind::Int { min, max } => {
                let mut n: i64 = cur.parse().unwrap_or(min);
                let calc = &self.calc;
                if ui
                    .add(
                        egui::DragValue::new(&mut n)
                            .update_while_editing(false)
                            .range(min..=max)
                            .custom_parser(move |s| {
                                crate::calc::parse_drag_int(calc, s, min, max).map(|n| n as f64)
                            }),
                    )
                    .changed()
                {
                    changed = Some(n.to_string());
                }
            }
            Kind::Float { min, max } => {
                let mut f: f64 = cur.parse().unwrap_or(0.0);
                let speed = ((max - min) / 500.0).clamp(0.001, 1.0);
                let calc = &self.calc;
                if ui
                    .add(
                        egui::DragValue::new(&mut f)
                            .update_while_editing(false)
                            .range(min..=max)
                            .speed(speed)
                            .custom_parser(move |s| crate::calc::parse_drag(calc, s)),
                    )
                    .changed()
                {
                    changed = Some(crate::calc::fmt_value(f));
                }
            }
            Kind::Choice(names) => {
                let cur_idx = names
                    .iter()
                    .position(|n| *n == cur)
                    .unwrap_or_else(|| cur.parse().unwrap_or(0));
                let mut sel = cur_idx;
                egui::ComboBox::from_id_salt(v.name)
                    .selected_text(names.get(sel).copied().unwrap_or(""))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for (i, name) in names.iter().enumerate() {
                            ui.selectable_value(&mut sel, i, *name);
                        }
                    });
                if sel != cur_idx {
                    changed = Some(sel.to_string());
                }
            }
            Kind::Color => {
                let rgb =
                    u32::from_str_radix(cur.trim_start_matches("0x").trim_start_matches("0X"), 16)
                        .unwrap_or(0);
                let mut c = egui::Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
                if ui.color_edit_button_srgba(&mut c).changed() {
                    changed = Some(format!("0x{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()));
                }
            }
            Kind::Text => {
                let mut s = cur.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut s).desired_width(150.0))
                    .changed()
                {
                    changed = Some(s);
                }
            }
        }
        if let (Some(nv), true) = (changed, v.wired) {
            match crate::varreg::env_set(&mut self.env, v.name, &nv) {
                Ok(_) => {
                    let _ = self.env.save();
                }
                Err(e) => self.history.push(format!("  ! {}: {}", v.name, e)),
            }
        }
    }

    /// `setvar` / bare-variable-name handler. `name=None` (or "?") lists the
    /// editable variables; `name` + non-empty `value` sets it inline; `name`
    /// alone queries + arms the value prompt (for wired vars) or reports the
    /// read-only status (for unwired ones).
    pub(super) fn handle_setvar(&mut self, name: Option<&str>, value: &str) {
        let Some(name) = name else {
            self.list_setvars();
            return;
        };
        if name == "?" {
            self.list_setvars();
            return;
        }
        let Some(var) = crate::varreg::find(name) else {
            self.history
                .push(format!("  ! unknown variable '{}'", name));
            return;
        };
        let canon = var.name;
        if !value.trim().is_empty() {
            // Numeric SYSVARs go through the calculator — `setvar TxHt 0.3*2`
            // sets 0.6, same as the two-step `setvar TxHt` → reply form.
            let vstr = self.sysvar_value_string(canon, value.trim());
            match crate::varreg::env_set(&mut self.env, canon, &vstr) {
                Ok(_) => {
                    let _ = self.env.save();
                    let nv = crate::varreg::env_get(&self.env, canon).unwrap_or_default();
                    self.history.push(format!("  {} = {}", canon, nv));
                }
                Err(e) => self.history.push(format!("  ! {}", e)),
            }
            return;
        }
        if var.wired {
            let cur = crate::varreg::env_get(&self.env, canon).unwrap_or_default();
            self.var_set_pending = Some(canon.to_string());
            self.set_prompt(format!(
                "Enter new value for {} <{}>:  [Esc=cancel]",
                canon, cur
            ));
        } else {
            self.history.push(format!(
                "  {} = {} ({:?} — not editable yet)",
                canon, var.default, var.status
            ));
        }
    }

    /// List the editable (wired) variables with their current values.
    fn list_setvars(&mut self) {
        self.history
            .push("  Editable variables — `setvar NAME VALUE` or type the name:".into());
        for v in crate::varreg::VARS.iter().filter(|v| v.wired) {
            let cur = crate::varreg::env_get(&self.env, v.name).unwrap_or_default();
            self.history
                .push(format!("    {:<8} = {:<14} {}", v.name, cur, v.desc));
        }
    }

    /// Re-issue the fillet prompt with the current radius, trim mode,
    /// and multiple-mode badges. Called after any sub-option toggle.
    pub(super) fn refresh_fillet_prompt(&mut self) {
        let r = self.env.FltRad;
        let tm = if self.env.TrmMd { "trim" } else { "no-trim" };
        let mm = if self.fillet_multiple { ", multi" } else { "" };
        let pm = if self.fillet_poly_all { ", POLY" } else { "" };
        let phase = if self.fillet_poly_all {
            "pick a POLYLINE to round ALL corners"
        } else {
            match self.fillet_state {
                FilletState::WaitingForFirst(_) => {
                    "click FIRST object (or two segments of a polyline)"
                }
                FilletState::WaitingForSecond(..) => "click SECOND object",
                FilletState::Off => return,
            }
        };
        self.set_prompt(format!(
            "fillet (r={}, {}{}{}): {}  [t=trim, m=multi, p=poly, r=radius, Esc]",
            r, tm, mm, pm, phase
        ));
    }

    /// A fillet failed because the radius was too large. Stay IN the fillet
    /// command and ask for a smaller radius (arms `fillet_waiting_radius` so
    /// the next typed number becomes the new radius). Returns true when it
    /// armed the re-prompt, so the caller knows not to exit / overwrite it.
    fn fillet_radius_too_big(&mut self, msg: &str) -> bool {
        if !msg.contains("too large") {
            return false;
        }
        self.fillet_waiting_radius = true;
        self.set_prompt(format!(
            "fillet: {} — type a SMALLER radius <{}> then re-pick  [Esc=cancel]",
            msg, self.env.FltRad
        ));
        true
    }

    /// Mirror of `fillet_radius_too_big` for Chamfer — re-prompt for distances.
    fn chamfer_dist_too_big(&mut self, msg: &str) -> bool {
        if !(msg.contains("too large") || msg.contains("exceeds")) {
            return false;
        }
        self.chamfer_dist_wait = ChamferDistWait::WaitingD1;
        self.set_prompt(format!(
            "chamfer: {} — type a SMALLER first distance <{}> then re-pick  [Esc=cancel]",
            msg, self.env.ChmDs1
        ));
        true
    }

    /// Re-issue the chamfer prompt — mirror of `refresh_fillet_prompt`.
    pub(super) fn refresh_chamfer_prompt(&mut self) {
        let d1 = self.env.ChmDs1;
        let d2 = self.env.ChmDs2;
        let tm = if self.env.TrmMd { "trim" } else { "no-trim" };
        let mm = if self.chamfer_multiple { ", multi" } else { "" };
        let pm = if self.chamfer_poly_all { ", POLY" } else { "" };
        let phase = if self.chamfer_poly_all {
            "pick a POLYLINE to bevel ALL corners"
        } else {
            match self.chamfer_state {
                ChamferState::WaitingForFirst(..) => {
                    "click FIRST object (or two segments of a polyline)"
                }
                ChamferState::WaitingForSecond(..) => "click SECOND object",
                ChamferState::Off => return,
            }
        };
        self.set_prompt(format!(
            "chamfer (d1={}, d2={}, {}{}{}): {}  [t=trim, m=multi, p=poly, d=distance, Esc]",
            d1, d2, tm, mm, pm, phase
        ));
    }

    // ---------------------------------------------------------------------
    // Fillet dispatcher. Routes the two picks:
    //   * same polyline, two different segments → corner fillet (insert arc)
    //   * Line/Wall pair                        → centerline path (curved walls)
    //   * anything with Arc / Polyline-end      → generalized kernel solver
    // ---------------------------------------------------------------------
    pub(super) fn apply_fillet(&mut self, r: f64, idx1: usize, pick1: Vec2, idx2: usize, pick2: Vec2) {
        if idx1 == idx2 {
            // Two segments of the SAME polyline → round that corner in place.
            self.apply_fillet_corner(r, idx1, pick1, pick2);
            return;
        }
        let is_lw = |g: &Geom| matches!(g, Geom::Line(_) | Geom::Wall(_));
        let lw = self
            .doc
            .dobjects
            .get(idx1)
            .map(|d| is_lw(&d.geom))
            .unwrap_or(false)
            && self
                .doc
                .dobjects
                .get(idx2)
                .map(|d| is_lw(&d.geom))
                .unwrap_or(false);
        if lw {
            self.apply_fillet_lines(r, idx1, pick1, idx2, pick2);
        } else {
            self.apply_fillet_general(r, idx1, pick1, idx2, pick2);
        }
    }

    /// Generalized fillet for any Line/Arc/Polyline-end pair (kernel solver).
    fn apply_fillet_general(&mut self, r: f64, idx1: usize, pick1: Vec2, idx2: usize, pick2: Vec2) {
        let (Some(d1), Some(d2)) = (self.doc.dobjects.get(idx1), self.doc.dobjects.get(idx2))
        else {
            return;
        };
        let g1 = d1.geom.clone();
        let g2 = d2.geom.clone();
        let style1 = d1.style;
        self.snapshot_doc();
        let trim = self.env.TrmMd;
        match cad_kernel::fillet_geoms(&g1, pick1, &g2, pick2, r) {
            Ok(out) => {
                if trim {
                    if let Some(d) = self.doc.dobjects.get_mut(idx1) {
                        d.geom = out.g1_new;
                    }
                    if let Some(d) = self.doc.dobjects.get_mut(idx2) {
                        d.geom = out.g2_new;
                    }
                }
                if let Some(arc_geom) = out.arc {
                    let mut d = DObject::new(arc_geom);
                    d.style = style1;
                    self.doc.push(d);
                }
                self.history.push(format!(
                    "  ⌐ fillet ✓ r={} between #{} and #{} ({})",
                    r,
                    idx1,
                    idx2,
                    if trim { "trim" } else { "no-trim" }
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
            }
            Err(msg) => {
                self.rollback_doc();
                self.history.push(format!("  ! {}", msg));
                self.fillet_radius_too_big(&msg);
            }
        }
    }

    /// Fillet the corner between two segments of one polyline (in place).
    fn apply_fillet_corner(&mut self, r: f64, idx: usize, pick1: Vec2, pick2: Vec2) {
        let Some(d) = self.doc.dobjects.get(idx) else {
            return;
        };
        let Geom::Polyline(pl) = d.geom.clone() else {
            self.history
                .push("  ! fillet: same object clicked twice".into());
            return;
        };
        let (Some(sa), Some(sb)) = (
            cad_kernel::nearest_polyline_segment(&pl, pick1),
            cad_kernel::nearest_polyline_segment(&pl, pick2),
        ) else {
            self.history
                .push("  ! fillet: couldn't locate the polyline segments".into());
            return;
        };
        if sa == sb {
            self.history
                .push("  ! fillet: click two DIFFERENT segments of the polyline".into());
            return;
        }
        self.snapshot_doc();
        match cad_kernel::fillet_polyline_corner(&pl, sa, sb, r) {
            Ok(np) => {
                if let Some(d) = self.doc.dobjects.get_mut(idx) {
                    d.geom = Geom::Polyline(np);
                }
                self.history.push(format!(
                    "  ⌐ fillet ✓ r={} corner of polyline #{} (segments {}+{})",
                    r, idx, sa, sb
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
            }
            Err(msg) => {
                self.rollback_doc();
                self.history.push(format!("  ! {}", msg));
                self.fillet_radius_too_big(&msg);
            }
        }
    }

    /// Fillet EVERY corner of one polyline (the `P` option).
    pub(super) fn apply_fillet_poly_all(&mut self, r: f64, idx: usize) {
        let Some(d) = self.doc.dobjects.get(idx) else {
            return;
        };
        let Geom::Polyline(pl) = d.geom.clone() else {
            self.history.push("  ! fillet P: pick a POLYLINE".into());
            return;
        };
        self.snapshot_doc();
        match cad_kernel::fillet_polyline_all(&pl, r) {
            Ok((np, count)) => {
                if let Some(d) = self.doc.dobjects.get_mut(idx) {
                    d.geom = Geom::Polyline(np);
                }
                self.history.push(format!(
                    "  ⌐ fillet ✓ r={} on polyline #{} — {} corner(s) rounded",
                    r, idx, count
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
            }
            Err(msg) => {
                self.rollback_doc();
                self.history.push(format!("  ! {}", msg));
                self.fillet_radius_too_big(&msg);
            }
        }
    }

    // ---------------------------------------------------------------------
    // Slice M.3 — Fillet (line-line / walls). Two clicks; second commits.
    // ---------------------------------------------------------------------
    fn apply_fillet_lines(&mut self, r: f64, idx1: usize, pick1: Vec2, idx2: usize, pick2: Vec2) {
        if idx1 == idx2 {
            self.history
                .push("  ! fillet: same dobject clicked twice".into());
            return;
        }
        // Fillet operates on a CENTERLINE: a Line is its own centerline; a
        // Wall contributes its centerline and keeps its thickness (the
        // wall's identity). This is how two separate walls "reach and
        // corner" — fillet extends/trims the centerlines to their
        // intersection, then the wall faces re-derive (mitre) from the
        // moved endpoints via `wall::solve_faces`.
        let Some(d1) = self.doc.dobjects.get(idx1) else {
            return;
        };
        let Some(d2) = self.doc.dobjects.get(idx2) else {
            return;
        };
        // (Line, Some((thickness, wall_style_id)) if it's a Wall).
        let wall_info = |g: &Geom| -> Option<(Line, Option<(f64, u32)>)> {
            match g {
                Geom::Line(a) => Some((*a, None)),
                Geom::Wall(w) => Some((w.centerline(), Some((w.thickness, w.style)))),
                _ => None,
            }
        };
        let (Some((l1, w1)), Some((l2, w2))) = (wall_info(&d1.geom), wall_info(&d2.geom)) else {
            self.history
                .push("  ! fillet: supports Line and Wall (its centerline) only".into());
            return;
        };
        let style1 = d1.style;
        let style2 = d2.style;
        self.snapshot_doc();
        let trim = self.env.TrmMd;
        match cad_kernel::fillet_lines(&l1, pick1, &l2, pick2, r) {
            Ok(out) => {
                // Trim mode → replace originals with the kernel's shortened
                // centerlines, re-wrapped as Walls so they keep their
                // identity (smart dobject), not become bare lines.
                if trim {
                    let rebuild = |g: Geom, w: Option<(f64, u32)>| -> Geom {
                        if let (Some((t, st)), Geom::Line(l)) = (w, &g) {
                            Geom::Wall(cad_kernel::Wall {
                                start: l.a,
                                end: l.b,
                                thickness: t,
                                style: st,
                                bulge: 0.0,
                            })
                        } else {
                            g
                        }
                    };
                    let new1 = rebuild(out.g1_new, w1);
                    let new2 = rebuild(out.g2_new, w2);
                    if let Some(d) = self.doc.dobjects.get_mut(idx1) {
                        d.geom = new1;
                    }
                    if let Some(d) = self.doc.dobjects.get_mut(idx2) {
                        d.geom = new2;
                    }
                }
                if let Some(arc_geom) = out.arc {
                    // r>0 corner arc. If either side is a WALL, the rounded
                    // corner becomes a CURVED WALL (scenario 1b): its
                    // centerline is the arc (stored as bulge), faces derive
                    // as concentric arcs. Otherwise it's a plain Arc.
                    let wall_side = w1.or(w2);
                    let g = if let (Some((thk, st)), Geom::Arc(arc)) = (wall_side, &arc_geom) {
                        let (sp, ep) = arc.endpoints();
                        let bulge = cad_kernel::bulge_from_arc(sp, ep, arc.center, arc.sweep_angle);
                        Geom::Wall(cad_kernel::Wall {
                            start: sp,
                            end: ep,
                            thickness: thk,
                            style: st,
                            bulge,
                        })
                    } else {
                        arc_geom
                    };
                    let mut d = DObject::new(g);
                    d.style = style1;
                    let _ = style2;
                    self.doc.push(d);
                }
                self.history.push(format!(
                    "  ⌐ fillet ✓ r={} between #{} and #{} ({})",
                    r,
                    idx1,
                    idx2,
                    if trim { "trim" } else { "no-trim" }
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
            }
            Err(msg) => {
                self.rollback_doc();
                self.history.push(format!("  ! fillet: {}", msg));
                self.fillet_radius_too_big(msg);
            }
        }
    }

    // ---------------------------------------------------------------------
    // Chamfer dispatcher — mirror of apply_fillet's routing.
    // ---------------------------------------------------------------------
    pub(super) fn apply_chamfer(
        &mut self,
        d1_dist: f64,
        d2_dist: f64,
        idx1: usize,
        pick1: Vec2,
        idx2: usize,
        pick2: Vec2,
    ) {
        if idx1 == idx2 {
            self.apply_chamfer_corner(d1_dist, d2_dist, idx1, pick1, pick2);
            return;
        }
        let is_l = |g: &Geom| matches!(g, Geom::Line(_));
        let ll = self
            .doc
            .dobjects
            .get(idx1)
            .map(|d| is_l(&d.geom))
            .unwrap_or(false)
            && self
                .doc
                .dobjects
                .get(idx2)
                .map(|d| is_l(&d.geom))
                .unwrap_or(false);
        if ll {
            self.apply_chamfer_lines(d1_dist, d2_dist, idx1, pick1, idx2, pick2);
        } else {
            self.apply_chamfer_general(d1_dist, d2_dist, idx1, pick1, idx2, pick2);
        }
    }

    /// Generalized chamfer for any Line/Arc/Polyline-end pair.
    fn apply_chamfer_general(
        &mut self,
        d1_dist: f64,
        d2_dist: f64,
        idx1: usize,
        pick1: Vec2,
        idx2: usize,
        pick2: Vec2,
    ) {
        let (Some(da), Some(db)) = (self.doc.dobjects.get(idx1), self.doc.dobjects.get(idx2))
        else {
            return;
        };
        let g1 = da.geom.clone();
        let g2 = db.geom.clone();
        let style1 = da.style;
        self.snapshot_doc();
        let trim = self.env.TrmMd || (d1_dist < cad_kernel::EPS && d2_dist < cad_kernel::EPS);
        match cad_kernel::chamfer_geoms(&g1, pick1, &g2, pick2, d1_dist, d2_dist) {
            Ok(out) => {
                if trim {
                    if let Some(d) = self.doc.dobjects.get_mut(idx1) {
                        d.geom = out.g1_new;
                    }
                    if let Some(d) = self.doc.dobjects.get_mut(idx2) {
                        d.geom = out.g2_new;
                    }
                }
                // Skip a None bridge — a d1=d2=0 chamfer is a clean sharp corner
                // (g1_new/g2_new already trimmed), so there's no segment to add.
                if let Some(bridge_geom) = out.bridge {
                    let mut bridge = DObject::new(bridge_geom);
                    bridge.style = style1;
                    self.doc.push(bridge);
                }
                self.history.push(format!(
                    "  ⌐ chamfer ✓ d=({}, {}) between #{} and #{} ({})",
                    d1_dist,
                    d2_dist,
                    idx1,
                    idx2,
                    if trim { "trim" } else { "no-trim" }
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
            }
            Err(msg) => {
                self.rollback_doc();
                self.history.push(format!("  ! {}", msg));
                self.chamfer_dist_too_big(&msg);
            }
        }
    }

    /// Chamfer the corner between two segments of one polyline (in place).
    fn apply_chamfer_corner(
        &mut self,
        d1_dist: f64,
        d2_dist: f64,
        idx: usize,
        pick1: Vec2,
        pick2: Vec2,
    ) {
        let Some(d) = self.doc.dobjects.get(idx) else {
            return;
        };
        let Geom::Polyline(pl) = d.geom.clone() else {
            self.history
                .push("  ! chamfer: same object clicked twice".into());
            return;
        };
        let (Some(sa), Some(sb)) = (
            cad_kernel::nearest_polyline_segment(&pl, pick1),
            cad_kernel::nearest_polyline_segment(&pl, pick2),
        ) else {
            self.history
                .push("  ! chamfer: couldn't locate the polyline segments".into());
            return;
        };
        if sa == sb {
            self.history
                .push("  ! chamfer: click two DIFFERENT segments of the polyline".into());
            return;
        }
        self.snapshot_doc();
        match cad_kernel::chamfer_polyline_corner(&pl, sa, sb, d1_dist, d2_dist) {
            Ok(np) => {
                if let Some(d) = self.doc.dobjects.get_mut(idx) {
                    d.geom = Geom::Polyline(np);
                }
                self.history.push(format!(
                    "  ⌐ chamfer ✓ d=({}, {}) corner of polyline #{} (segments {}+{})",
                    d1_dist, d2_dist, idx, sa, sb
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
            }
            Err(msg) => {
                self.rollback_doc();
                self.history.push(format!("  ! {}", msg));
                self.chamfer_dist_too_big(&msg);
            }
        }
    }

    /// Chamfer EVERY corner of one polyline (the `P` option).
    pub(super) fn apply_chamfer_poly_all(&mut self, d1_dist: f64, d2_dist: f64, idx: usize) {
        let Some(d) = self.doc.dobjects.get(idx) else {
            return;
        };
        let Geom::Polyline(pl) = d.geom.clone() else {
            self.history.push("  ! chamfer P: pick a POLYLINE".into());
            return;
        };
        self.snapshot_doc();
        match cad_kernel::chamfer_polyline_all(&pl, d1_dist, d2_dist) {
            Ok((np, count)) => {
                if let Some(d) = self.doc.dobjects.get_mut(idx) {
                    d.geom = Geom::Polyline(np);
                }
                self.history.push(format!(
                    "  ⌐ chamfer ✓ d=({}, {}) on polyline #{} — {} corner(s) beveled",
                    d1_dist, d2_dist, idx, count
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
            }
            Err(msg) => {
                self.rollback_doc();
                self.history.push(format!("  ! {}", msg));
                self.chamfer_dist_too_big(&msg);
            }
        }
    }

    // ---------------------------------------------------------------------
    // Slice M.4 — Chamfer (line-line).
    // ---------------------------------------------------------------------
    fn apply_chamfer_lines(
        &mut self,
        d1_dist: f64,
        d2_dist: f64,
        idx1: usize,
        pick1: Vec2,
        idx2: usize,
        pick2: Vec2,
    ) {
        if idx1 == idx2 {
            self.history
                .push("  ! chamfer: same dobject clicked twice".into());
            return;
        }
        let Some(da) = self.doc.dobjects.get(idx1) else {
            return;
        };
        let Some(db) = self.doc.dobjects.get(idx2) else {
            return;
        };
        let (l1, l2) = match (&da.geom, &db.geom) {
            (Geom::Line(a), Geom::Line(b)) => (*a, *b),
            _ => {
                self.history
                    .push("  ! chamfer: v1 supports LINE + LINE only".into());
                return;
            }
        };
        let style1 = da.style;
        self.snapshot_doc();
        let trim = self.env.TrmMd || (d1_dist < cad_kernel::EPS && d2_dist < cad_kernel::EPS);
        match cad_kernel::chamfer_lines(&l1, pick1, &l2, pick2, d1_dist, d2_dist) {
            Ok(out) => {
                if trim {
                    if let Some(d) = self.doc.dobjects.get_mut(idx1) {
                        d.geom = out.g1_new;
                    }
                    if let Some(d) = self.doc.dobjects.get_mut(idx2) {
                        d.geom = out.g2_new;
                    }
                }
                // Skip a None bridge — a d1=d2=0 chamfer is a clean sharp corner
                // (g1_new/g2_new already trimmed), so there's no segment to add.
                if let Some(bridge_geom) = out.bridge {
                    let mut bridge = DObject::new(bridge_geom);
                    bridge.style = style1;
                    self.doc.push(bridge);
                }
                self.history.push(format!(
                    "  ⌐ chamfer ✓ d=({}, {}) between #{} and #{} ({})",
                    d1_dist,
                    d2_dist,
                    idx1,
                    idx2,
                    if trim { "trim" } else { "no-trim" }
                ));
                self.intersections.clear();
                self.index_dirty = true;
                self.touch_view();
            }
            Err(msg) => {
                self.rollback_doc();
                self.history.push(format!("  ! chamfer: {}", msg));
                self.chamfer_dist_too_big(msg);
            }
        }
    }

    // ---------------------------------------------------------------------
    // Slice M.5 — Join. Operates on `self.selection`. Three-pass merge in
    // the kernel; we remove consumed dobjects (descending) and append the
    // merged ones at the doc's tail, inheriting style from the lowest-
    // indexed contributor.
    // ---------------------------------------------------------------------
    pub(super) fn apply_join(&mut self) {
        if self.selection.is_empty() {
            self.history.push("  ! join: empty basket".into());
            return;
        }
        let items: Vec<(usize, Geom)> = self
            .selection
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).map(|d| (i, d.geom.clone())))
            .collect();
        if items.len() < 2 {
            self.history.push("  ! join: need ≥ 2 dobjects".into());
            return;
        }
        let out = cad_kernel::join_geoms(&items);
        if out.merged.is_empty() || out.consumed_indices.is_empty() {
            self.history.push(
                "  ! join: nothing in the basket could be merged (need collinear lines, concentric arcs, or a touching chain)".into());
            return;
        }
        self.snapshot_doc();
        // Inherit style from the LOWEST-indexed consumed dobject in each
        // merged piece — simple, deterministic, matches AutoCAD's "first
        // selected wins" convention.
        let inherit_style = out
            .consumed_indices
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).map(|d| d.style))
            .next();
        // Remove consumed dobjects in DESCENDING index order so shifts
        // don't invalidate later removals.
        let mut to_remove: Vec<usize> = out.consumed_indices.clone();
        to_remove.sort_unstable_by(|a, b| b.cmp(a));
        for idx in &to_remove {
            if *idx < self.doc.dobjects.len() {
                self.doc.dobjects.remove(*idx);
            }
        }
        // Append merged geoms.
        for g in out.merged {
            let mut d = DObject::new(g);
            if let Some(s) = inherit_style {
                d.style = s;
            }
            self.doc.push(d);
        }
        // The selection's old indices are now stale — clear it.
        self.selection.clear();
        self.history.push(format!(
            "  ⧙ join ✓ {} dobject(s) merged → {} new piece(s)",
            to_remove.len(),
            self.doc.dobjects.len()
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// UCS — dispatch `ucs origin X,Y [rot]` / `save NAME` / `NAME` /
    /// `delete NAME` / `rename OLD NEW` / `world` / bare (list). The
    /// current UCS lives on the Document (persisted in RSM v27+).
    pub(super) fn apply_ucs(&mut self, args: &[String]) {
        use cad_kernel::ucs::Ucs;
        if args.is_empty() {
            let cur = self.ucs_name();
            let names: Vec<String> = self.doc.ucs_list.iter().map(|u| u.name.clone()).collect();
            if names.is_empty() {
                self.history.push(format!(
                    "  ucs: current = {cur}  [origin X,Y, save NAME, NAME, world]"
                ));
            } else {
                self.history.push(format!(
                    "  ucs: current = {cur}  list: {}",
                    names.join(", ")
                ));
            }
            return;
        }
        let first = args[0].to_ascii_lowercase();
        match first.as_str() {
            "world" | "w" => {
                self.doc.current_ucs = 0;
                self.history.push("  ucs: current = World".into());
            }
            "save" | "s" => {
                let Some(name) = args.get(1) else {
                    self.history.push("  ! ucs: save needs a name".into());
                    return;
                };
                let Some(u) = self.current_ucs().cloned() else {
                    self.history.push(
                        "  ! ucs: nothing to save — define an origin first (`ucs origin X,Y`)"
                            .into(),
                    );
                    return;
                };
                if self
                    .doc
                    .ucs_list
                    .iter()
                    .any(|x| x.name.eq_ignore_ascii_case(name))
                {
                    self.history.push(format!(
                        "  ! ucs: '{name}' already exists (delete it first)"
                    ));
                    return;
                }
                let mut nu = u.clone();
                nu.name = name.clone();
                self.doc.ucs_list.push(nu);
                self.doc.current_ucs = self.doc.ucs_list.len(); // new entry is current
                self.history
                    .push(format!("  ucs: saved '{name}' and made it current"));
            }
            "delete" | "del" | "d" => {
                let Some(name) = args.get(1) else {
                    self.history.push("  ! ucs: delete needs a name".into());
                    return;
                };
                let Some(idx) = self
                    .doc
                    .ucs_list
                    .iter()
                    .position(|x| x.name.eq_ignore_ascii_case(name))
                else {
                    self.history
                        .push(format!("  ! ucs: no system named '{name}'"));
                    return;
                };
                self.doc.ucs_list.remove(idx);
                if self.doc.current_ucs == idx + 1 {
                    self.doc.current_ucs = 0;
                } else if self.doc.current_ucs > idx + 1 {
                    self.doc.current_ucs -= 1;
                }
                self.history.push(format!("  ucs: deleted '{name}'"));
            }
            "rename" | "r" => {
                let (Some(from), Some(to)) = (args.get(1), args.get(2)) else {
                    self.history
                        .push("  ! ucs: rename needs OLD and NEW names".into());
                    return;
                };
                let Some(idx) = self
                    .doc
                    .ucs_list
                    .iter()
                    .position(|x| x.name.eq_ignore_ascii_case(from))
                else {
                    self.history
                        .push(format!("  ! ucs: no system named '{from}'"));
                    return;
                };
                if self
                    .doc
                    .ucs_list
                    .iter()
                    .any(|x| x.name.eq_ignore_ascii_case(to))
                {
                    self.history.push(format!("  ! ucs: '{to}' already exists"));
                    return;
                }
                self.doc.ucs_list[idx].name = to.clone();
                self.history
                    .push(format!("  ucs: renamed '{from}' → '{to}'"));
            }
            "origin" | "o" => {
                let Some(xy) = args.get(1) else {
                    self.history
                        .push("  ! ucs: origin needs X,Y  [e.g. `ucs origin 10,20 45`]".into());
                    return;
                };
                let Some(p) = parse_xy(&self.calc, xy) else {
                    self.history
                        .push(format!("  ! ucs: bad origin '{xy}' — use X,Y"));
                    return;
                };
                let rot = args
                    .get(2)
                    .and_then(|t| t.trim().parse::<f64>().ok())
                    .unwrap_or(0.0)
                    .to_radians();
                // Set the (unnamed) CURRENT system; `save` names it.
                self.doc.ucs_list.push(Ucs {
                    name: "Unnamed".into(),
                    origin: p,
                    rotation: rot,
                });
                self.doc.current_ucs = self.doc.ucs_list.len();
                self.history.push(format!(
                    "  ucs: origin=({:.3},{:.3}) rotation={:.2}° (unnamed — `ucs save NAME`)",
                    p.x,
                    p.y,
                    rot.to_degrees()
                ));
            }
            _ => {
                // A bare name = make that system current.
                let Some(idx) = self
                    .doc
                    .ucs_list
                    .iter()
                    .position(|x| x.name.eq_ignore_ascii_case(&args[0]))
                else {
                    self.history.push(format!(
                        "  ! ucs: no system named '{}'  [origin X,Y, save NAME, world]",
                        args[0]
                    ));
                    return;
                };
                self.doc.current_ucs = idx + 1;
                self.history
                    .push(format!("  ucs: current = {}", self.doc.ucs_list[idx].name));
            }
        }
    }

    /// The current UCS (World when `current_ucs == 0`).
    pub(super) fn current_ucs(&self) -> Option<&cad_kernel::ucs::Ucs> {
        if self.doc.current_ucs == 0 {
            return None;
        }
        self.doc.ucs_list.get(self.doc.current_ucs - 1)
    }

    /// Convert a typed UCS-space point to world space (no-op in World).
    pub(super) fn ucs_to_world(&self, p: Vec2) -> Vec2 {
        match self.current_ucs() {
            Some(u) => u.to_world(p),
            None => p,
        }
    }

    /// Convert a world point to the current UCS (for display/readout).
    pub(super) fn world_to_ucs(&self, p: Vec2) -> Vec2 {
        match self.current_ucs() {
            Some(u) => u.to_ucs(p),
            None => p,
        }
    }

    /// Name of the current UCS ("World" when none is active).
    pub(super) fn ucs_name(&self) -> String {
        self.current_ucs()
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "World".into())
    }

    /// LAYERSTATE — save / restore / delete / rename named layer states
    /// (kernel `laystate` module). Bare name = restore; `?` lists.
    pub(super) fn apply_layerstate(&mut self, args: &[String]) {
        use cad_kernel::laystate as ls;
        if args.is_empty() {
            let names = ls::names(&self.doc);
            if names.is_empty() {
                self.history
                    .push("  layerstate: no saved states  [save NAME to create one]".into());
            } else {
                self.history
                    .push(format!("  layerstate: {}", names.join(", ")));
            }
            return;
        }
        let first = args[0].to_ascii_lowercase();
        match first.as_str() {
            "save" | "s" => {
                let Some(name) = args.get(1) else {
                    self.history.push(
                        "  ! layerstate: save needs a name, e.g. `layerstate save Plan`".into(),
                    );
                    return;
                };
                match ls::save(&mut self.doc, name) {
                    Ok(()) => {
                        self.index_dirty = true;
                        self.history.push(format!(
                            "  layerstate: saved '{name}' ({} layers)",
                            self.doc.layers.layers.len()
                        ));
                    }
                    Err(e) => self.history.push(format!("  ! layerstate: {}", e)),
                }
            }
            "delete" | "del" | "d" => {
                let Some(name) = args.get(1) else {
                    self.history
                        .push("  ! layerstate: delete needs a name".into());
                    return;
                };
                if ls::delete(&mut self.doc, name) {
                    self.history.push(format!("  layerstate: deleted '{name}'"));
                } else {
                    self.history
                        .push(format!("  ! layerstate: no state named '{name}'"));
                }
            }
            "rename" | "r" => {
                let (Some(from), Some(to)) = (args.get(1), args.get(2)) else {
                    self.history
                        .push("  ! layerstate: rename needs OLD and NEW names".into());
                    return;
                };
                if ls::rename(&mut self.doc, from, to) {
                    self.history
                        .push(format!("  layerstate: renamed '{from}' → '{to}'"));
                } else {
                    self.history
                        .push(format!("  ! layerstate: cannot rename '{from}' → '{to}'"));
                }
            }
            "?" | "list" | "ls" => {
                let names = ls::names(&self.doc);
                if names.is_empty() {
                    self.history.push("  ! layerstate: no saved states".into());
                } else {
                    self.history
                        .push(format!("  layerstate: {}", names.join(", ")));
                }
            }
            _ => {
                // Bare name = restore.
                self.snapshot_doc();
                if ls::restore(&mut self.doc, &args[0]) {
                    self.index_dirty = true;
                    self.gpu_dirty = true;
                    self.history
                        .push(format!("  layerstate: restored '{}'", args[0]));
                } else {
                    self.rollback_doc();
                    self.history.push(format!(
                        "  ! layerstate: no state named '{}'  [save NAME to create one]",
                        args[0]
                    ));
                }
            }
        }
    }

    /// PURGE — remove every unreferenced named item (layers, linetypes,
    /// text/dim/wall styles, blocks). One undo entry; nothing to purge
    /// fails visibly.
    pub(super) fn commit_purge(&mut self) {
        self.snapshot_doc();
        let report = cad_kernel::purge::purge(&mut self.doc);
        if report.is_empty() {
            self.rollback_doc();
            self.history.push("  ! purge: nothing to purge".into());
            return;
        }
        // Clamp current-style ids (removals may have shifted them).
        let n_ts = self.doc.text_styles.styles.len() as u32;
        let n_ds = self.doc.dim_styles.styles.len() as u32;
        let n_ws = self.doc.wall_styles.styles.len() as u32;
        if self.text_input_dialog_style_id >= n_ts {
            self.text_input_dialog_style_id = cad_kernel::TextStyleTable::STANDARD;
        }
        if self.current_dim_style >= n_ds {
            self.current_dim_style = 0;
        }
        if self.current_wall_style >= n_ws {
            self.current_wall_style = 0;
        }
        // Same clamp for the ACTIVE layer: purging the layer it pointed at
        // left `layers.active` dangling (index past the end), which made
        // every dobject's `is_visible`/`is_selectable` check fail (nothing
        // could be picked or drawn until the user switched layers).
        if self.doc.layers.active >= self.doc.layers.layers.len() as u32 {
            self.doc.layers.active = cad_kernel::LayerTable::LAYER_ZERO;
        }
        for (label, v) in [
            ("layer", &report.layers),
            ("linetype", &report.linetypes),
            ("text style", &report.text_styles),
            ("dim style", &report.dim_styles),
            ("wall style", &report.wall_styles),
            ("block", &report.blocks),
        ] {
            if !v.is_empty() {
                self.history.push(format!(
                    "  ✓ purge: removed {} {}: {}",
                    v.len(),
                    label,
                    v.join(", ")
                ));
            }
        }
        self.index_dirty = true;
        self.gpu_dirty = true;
        self.touch_view();
    }

    /// OVERKILL — dedupe `subset` (None = the whole drawing). One undo
    /// entry; reports removed count; nothing to do fails visibly.
    pub(super) fn commit_overkill(&mut self, subset: Option<Vec<usize>>) {
        let tol = 1e-6; // drawing units — exact-coincidence hunt
        let n_before = self.doc.dobjects.len();
        self.snapshot_doc();
        match subset {
            None => {
                let removed = cad_kernel::dedupe::dedupe(&mut self.doc.dobjects, tol);
                let n = removed.len();
                self.selection.clear();
                self.selected = None;
                if n == 0 {
                    self.rollback_doc();
                    self.history
                        .push("  ! overkill: no duplicates found".into());
                    return;
                }
                self.history.push(format!(
                    "  ✓ overkill: removed {n} duplicate(s) from {n_before} dobject(s)"
                ));
            }
            Some(indices) => {
                // Dedupe only within the selected set (indices stay valid
                // because removal happens at the end, highest first).
                let mut sorted: Vec<usize> = indices.clone();
                sorted.sort_unstable();
                sorted.dedup();
                let mut drop: Vec<usize> = Vec::new();
                for (k, &i) in sorted.iter().enumerate() {
                    for &j in &sorted[k + 1..] {
                        let same = match (
                            self.doc.dobjects.get(i).map(|d| d.geom.clone()),
                            self.doc.dobjects.get(j).map(|d| d.geom.clone()),
                        ) {
                            (Some(a), Some(b)) => cad_kernel::dedupe::geoms_equal(&a, &b, tol),
                            _ => false,
                        };
                        if same {
                            drop.push(j);
                        }
                    }
                }
                drop.sort_unstable();
                drop.dedup();
                let n = drop.len();
                if n == 0 {
                    self.rollback_doc();
                    self.history
                        .push("  ! overkill: no duplicates found in selection".into());
                    return;
                }
                for &idx in drop.iter().rev() {
                    if idx < self.doc.dobjects.len() {
                        self.doc.dobjects.remove(idx);
                    }
                }
                self.selection.clear();
                self.selected = None;
                self.history.push(format!(
                    "  ✓ overkill: removed {n} duplicate(s) from selection"
                ));
            }
        }
        self.index_dirty = true;
        self.gpu_dirty = true;
        self.touch_view();
    }

    /// Report a measured area+perimeter into the history, and fold it into
    /// the running total (sign ±1 for add/subtract mode).
    fn report_area(&mut self, label: &str, area: f64, perim: Option<f64>) {
        let sign_txt = if self
            .area_state
            .as_ref()
            .map(|a| a.sign < 0.0)
            .unwrap_or(false)
        {
            " (subtract)"
        } else {
            ""
        };
        let p_txt = match perim {
            Some(p) => format!("  perimeter={p:.4}"),
            None => String::new(),
        };
        self.history
            .push(format!("  area{sign_txt}: {label} area={area:.4}{p_txt}"));
        if let Some(st) = self.area_state.as_mut() {
            st.total += area * st.sign;
            let t = st.total.abs();
            self.history.push(format!(
                "  area: total ({})= {t:.4}",
                if st.total >= 0.0 { "+" } else { "-" }
            ));
        }
    }

    /// One area click: a closed object under the cursor is measured; an
    /// empty click starts/extends the point polygon.
    pub(super) fn area_click(&mut self, click_world: Vec2) {
        if self.area_state.is_none() {
            return;
        }
        let tol = (self.env.PkBxSz.max(8) as f64) / (self.scale as f64).max(1e-6);
        let hit = self
            .nearest_entity_under(click_world, tol)
            .and_then(|i| self.doc.dobjects.get(i))
            .map(|d| d.geom.clone());
        if let Some(g) = hit {
            match g.measured_area() {
                Some(a) => {
                    let name = dobject_kind_name(&g).to_lowercase();
                    self.report_area(&name, a, g.measured_perimeter());
                    return;
                }
                None => {
                    self.history.push(
                        "  ! area: that object is not closed — pick a circle/ellipse/closed polyline/closed spline, or pick points".into());
                    return;
                }
            }
        }
        // Empty click → point-polygon mode.
        let starting = self
            .area_state
            .as_ref()
            .map(|st| st.pts.is_empty())
            .unwrap_or(false);
        if let Some(st) = self.area_state.as_mut() {
            st.pts.push(click_world);
            if st.pts.len() >= 3 {
                let a = polygon_shoelace(&st.pts).abs() * 0.5;
                self.history
                    .push(format!("  area: running polygon area = {a:.4}"));
            }
        }
        if starting {
            self.set_prompt("area: pick polygon points  [Enter=finish  Esc]".to_string());
        }
    }

    /// Finish the point polygon (Enter while picking) and fold it in.
    pub(super) fn area_finish_polygon(&mut self) -> bool {
        let Some(st) = self.area_state.as_ref() else {
            return false;
        };
        if st.pts.len() < 3 {
            self.history.push("  ! area: need at least 3 points".into());
            if let Some(st) = self.area_state.as_mut() {
                st.pts.clear();
            }
            return true;
        }
        let pts = st.pts.clone();
        let a = polygon_shoelace(&pts).abs() * 0.5;
        let perim = pts.windows(2).fold(0.0, |acc, w| acc + (w[1] - w[0]).len())
            + (pts[0] - pts[pts.len() - 1]).len();
        if let Some(st) = self.area_state.as_mut() {
            st.pts.clear();
        }
        self.report_area("point polygon", a, Some(perim));
        self.set_prompt(
            "area: pick a closed object or pick points  [A=add  S=subtract  Enter=finish  Esc]"
                .to_string(),
        );
        true
    }

    /// `a`/`s` sub-options toggle add/subtract mode. Returns true when the
    /// input was consumed by an area mode toggle.
    pub(super) fn area_suboption(&mut self, raw: &str) -> bool {
        if self.area_state.is_none() {
            return false;
        }
        match raw.trim().to_ascii_lowercase().as_str() {
            "a" | "add" => {
                if let Some(st) = self.area_state.as_mut() {
                    st.sign = 1.0;
                }
                self.history
                    .push("  area: ADD mode — areas are added to the total".into());
                true
            }
            "s" | "sub" | "subtract" => {
                if let Some(st) = self.area_state.as_mut() {
                    st.sign = -1.0;
                }
                self.history
                    .push("  area: SUBTRACT mode — areas are subtracted from the total".into());
                true
            }
            _ => false,
        }
    }

    /// DIVIDE / MEASURE commit: place POINT marks along the picked curve —
    /// `count_or_dist` is a whole segment COUNT (divide) or a positive
    /// segment LENGTH (measure). One undo entry; one history report.
    pub(super) fn apply_divmeasure_marks(&mut self, obj_idx: usize, count_or_dist: f64, is_measure: bool) {
        let Some(geom) = self.doc.dobjects.get(obj_idx).map(|d| d.geom.clone()) else {
            self.history
                .push("  ! divide/measure: curve is gone".into());
            return;
        };
        let positions = divmeasure_positions_for(&geom, count_or_dist, is_measure);
        if positions.is_empty() {
            self.history.push(format!(
                "  ! {}: no marks to place (check the value)",
                if is_measure { "measure" } else { "divide" }
            ));
            return;
        }
        let (color, layer) = (self.doc.current_color, self.doc.layers.active);
        self.snapshot_doc();
        for p in &positions {
            let mut d = DObject::new(Geom::Point(cad_kernel::Point {
                location: *p,
                style: 0,
                size: 0.0,
            }));
            d.style.color = color;
            d.style.layer = layer;
            self.doc.push(d);
        }
        self.selection.clear();
        self.selected = None;
        self.index_dirty = true;
        self.gpu_dirty = true;
        self.history.push(format!(
            "  {}: {} mark(s) placed",
            if is_measure { "measure" } else { "divide" },
            positions.len()
        ));
    }

    /// LAYISO / LAYFRZ / LAYOFF — apply the armed op to a layer id. One
    /// undo entry per pick; a no-op rolls the entry back and reports
    /// visibly.
    pub(super) fn handle_layer_pick(&mut self, lid: u32) {
        if self.layer_pick == LayerPickState::Off {
            return;
        }
        self.snapshot_doc();
        let layers = &mut self.doc.layers.layers;
        let changed = match self.layer_pick {
            LayerPickState::Iso => {
                let mut n = 0usize;
                for (i, l) in layers.iter_mut().enumerate() {
                    if i as u32 != lid && !l.frozen {
                        l.frozen = true;
                        n += 1;
                    }
                }
                n
            }
            LayerPickState::Frz => {
                let already = layers.get(lid as usize).map(|l| l.frozen).unwrap_or(false);
                if !already {
                    if let Some(l) = layers.get_mut(lid as usize) {
                        l.frozen = true;
                    }
                }
                if already {
                    0
                } else {
                    1
                }
            }
            LayerPickState::OffLayer => {
                let already = layers
                    .get(lid as usize)
                    .map(|l| !l.visible)
                    .unwrap_or(false);
                if !already {
                    if let Some(l) = layers.get_mut(lid as usize) {
                        l.visible = false;
                    }
                }
                if already {
                    0
                } else {
                    1
                }
            }
            LayerPickState::Off => 0,
        };
        if changed == 0 {
            self.rollback_doc();
            self.history.push(format!(
                "  ! {}: nothing changed (layer '{}' not found or already in that state)",
                match self.layer_pick {
                    LayerPickState::Iso => "layiso",
                    LayerPickState::Frz => "layfrz",
                    LayerPickState::OffLayer => "layoff",
                    LayerPickState::Off => unreachable!(),
                },
                self.doc
                    .layers
                    .get(lid)
                    .map(|l| l.name.clone())
                    .unwrap_or_default()
            ));
            return;
        }
        self.gpu_dirty = true;
        self.index_dirty = true;
        let name = self
            .doc
            .layers
            .get(lid)
            .map(|l| l.name.clone())
            .unwrap_or_default();
        match self.layer_pick {
            LayerPickState::Iso => self.history.push(format!(
                "  layiso: {} layer(s) frozen — '{}' stays",
                changed, name
            )),
            LayerPickState::Frz => self.history.push(format!("  layfrz: '{}' frozen", name)),
            LayerPickState::OffLayer => self.history.push(format!("  layoff: '{}' off", name)),
            LayerPickState::Off => {}
        }
    }

    pub(super) fn apply_chlayer(&mut self) {
        if self.selection.is_empty() {
            self.history.push("  ! chlayer: empty basket".into());
            return;
        }
        let target = self.doc.layers.active;
        let name = self
            .doc
            .layers
            .get(target)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| "?".into());
        self.snapshot_doc();
        let n = self.selection.len();
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                d.style.layer = target;
            }
        }
        self.history.push(format!(
            "  → chlayer: {} dobject(s) moved to active layer '{}'",
            n, name
        ));
        self.touch_view();
    }

    /// Append translated copies of the current selection (`copy` op).
    pub(super) fn apply_copy(&mut self, v: Vec2) {
        if v.x.abs() < EPS && v.y.abs() < EPS {
            return;
        }
        self.snapshot_doc();
        let copies: Vec<DObject> = self
            .selection
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i))
            .map(|d| {
                let g = d.geom.translated(v);
                let mut new = DObject::new(g);
                new.style = d.style;
                new
            })
            .collect();
        let n = copies.len();
        for c in copies {
            self.doc.push(c);
        }
        self.selection_prev = self.selection.clone();
        self.selection.clear();
        self.history
            .push(format!("  + copy: {} dobject(s) duplicated", n));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// Rotate the current selection in place by `angle` around `pivot`.
    pub(super) fn apply_rotate(&mut self, pivot: Vec2, angle: f64) {
        if angle.abs() < EPS {
            return;
        }
        self.snapshot_doc();
        // A selected hatch also rotates its boundary dobjects — a hatch has no
        // geometry of its own, so `Geom::Hatch::rotated` is a kernel no-op and
        // the fill only follows if the boundary does.
        let targets = self.transform_targets_with_hatch_boundaries();
        let n = targets.len();
        for &i in &targets {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                d.geom = d.geom.rotated(pivot, angle);
            }
        }
        self.history.push(format!(
            "  ⟳ rotate: {} dobject(s) by {:.2}° around ({:.2}, {:.2})",
            n,
            angle.to_degrees(),
            pivot.x,
            pivot.y
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// Dispatch the rotate session's commit step: if `rotate_copy` is on,
    /// produce rotated COPIES (originals untouched) instead of modifying
    /// the selection in place. AutoCAD's `C` sub-command.
    pub(super) fn apply_rotate_or_copy(&mut self, pivot: Vec2, angle: f64) {
        if angle.abs() < EPS {
            return;
        }
        if !self.rotate_copy {
            self.apply_rotate(pivot, angle);
            return;
        }
        self.snapshot_doc();
        let copies: Vec<DObject> = self
            .selection
            .iter()
            .filter_map(|&i| {
                self.doc.dobjects.get(i).map(|d| {
                    let mut copy = d.clone();
                    copy.geom = copy.geom.rotated(pivot, angle);
                    copy.handle = cad_kernel::next_handle();
                    copy
                })
            })
            .collect();
        let n = copies.len();
        for c in copies {
            self.doc.push(c);
        }
        self.history.push(format!(
            "  ⟳+ rotate-copy: {} new dobject(s) at {:.2}° around ({:.2}, {:.2})",
            n,
            angle.to_degrees(),
            pivot.x,
            pivot.y
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// Scale the current selection in place by `factor` around `pivot`.
    pub(super) fn apply_scale(&mut self, pivot: Vec2, factor: f64) {
        if (factor - 1.0).abs() < EPS || factor.abs() < EPS {
            return;
        }
        self.snapshot_doc();
        // A selected hatch also scales its boundary dobjects (see apply_rotate).
        let targets = self.transform_targets_with_hatch_boundaries();
        let n = targets.len();
        for &i in &targets {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                d.geom = d.geom.scaled(pivot, factor);
            }
        }
        self.history.push(format!(
            "  ⊕ scale: {} dobject(s) by {:.3}× around ({:.2}, {:.2})",
            n, factor, pivot.x, pivot.y
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// Dispatch the scale session's commit step: if `scale_copy` is on,
    /// produce scaled COPIES (originals untouched). AutoCAD `C` sub-cmd.
    pub(super) fn apply_scale_or_copy(&mut self, pivot: Vec2, factor: f64) {
        if (factor - 1.0).abs() < EPS || factor.abs() < EPS {
            return;
        }
        if !self.scale_copy {
            self.apply_scale(pivot, factor);
            return;
        }
        self.snapshot_doc();
        let copies: Vec<DObject> = self
            .selection
            .iter()
            .filter_map(|&i| {
                self.doc.dobjects.get(i).map(|d| {
                    let mut copy = d.clone();
                    copy.geom = copy.geom.scaled(pivot, factor);
                    copy.handle = cad_kernel::next_handle();
                    copy
                })
            })
            .collect();
        let n = copies.len();
        for c in copies {
            self.doc.push(c);
        }
        self.history.push(format!(
            "  ⊕+ scale-copy: {} new dobject(s) by {:.3}× around ({:.2}, {:.2})",
            n, factor, pivot.x, pivot.y
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// Mirror the current selection in place across the axis A→B.
    /// Mirror the selection across the axis (a, b). `keep_original = true`
    /// appends mirrored COPIES (originals stay — fresh handles, like copy);
    /// `false` flips the originals in place.
    pub(super) fn apply_mirror(&mut self, a: Vec2, b: Vec2, keep_original: bool) {
        if a.dist(b) < EPS {
            return;
        }
        self.snapshot_doc();
        // Counts differ per branch: copies come from the selection alone,
        // the in-place flip touches the hatch-widened set.
        let n;
        if keep_original {
            let copies: Vec<DObject> = self
                .selection
                .iter()
                .filter_map(|&i| self.doc.dobjects.get(i))
                .map(|d| {
                    let mut new = DObject::new(d.geom.mirrored(a, b));
                    new.style = d.style;
                    new
                })
                .collect();
            n = copies.len();
            for c in copies {
                self.doc.push(c);
            }
            self.selection_prev = self.selection.clone();
            self.selection.clear();
        } else {
            // Like the other in-place transforms, carry a selected hatch's
            // boundary with it. The keep_original COPY branch deliberately does
            // not (a hatch copied alone still references the original boundary).
            let targets = self.transform_targets_with_hatch_boundaries();
            n = targets.len();
            for &i in &targets {
                if let Some(d) = self.doc.dobjects.get_mut(i) {
                    d.geom = d.geom.mirrored(a, b);
                }
            }
        }
        self.history.push(format!(
            "  ⇄ mirror: {} dobject(s) across ({:.2},{:.2})–({:.2},{:.2}) [{}]",
            n,
            a.x,
            a.y,
            b.x,
            b.y,
            if keep_original {
                "kept original"
            } else {
                "erased original"
            }
        ));
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// Commit a Text dobject at `pos` with the given string + height.
    /// Defaults: angle=0, h_align=Left, v_align=Baseline, style=STANDARD.
    pub(super) fn commit_text_at(&mut self, pos: Vec2, string: &str, height: f64) {
        self.commit_text_at_with_style(pos, string, height, cad_kernel::TextStyleTable::STANDARD);
    }

    /// Smart-dim click handler. Drives the 2- or 3-click flow
    /// based on `self.dim_draft`.
    ///
    ///   1. `WaitingForP1` + click on Circle/Arc → Radius (jumps to
    ///                                              WaitingForDimLinePos)
    ///   1. `WaitingForP1` + click on empty/other → store as p1,
    ///                                              transition to
    ///                                              WaitingForP2
    ///   2. `WaitingForP2 { p1 }` → store as p2, ortho = Aligned,
    ///                              transition to WaitingForDimLinePos
    ///   3. `WaitingForDimLinePos { kind }` → fill leader/dimline pos,
    ///                                        commit Dimension, exit
    ///
    /// Ortho selection (Linear): auto-detected from the dimline_pos
    /// click — if the perpendicular offset from p1→p2 is mostly Y,
    /// the dim line is HORIZONTAL (measures Δx); mostly X →
    /// VERTICAL (measures Δy); ALIGNED is the fallback when the click
    /// sits closer to the chord-perp than to either axis. v1: just
    /// always use Aligned and let the user re-pick via grips later.
    pub(super) fn handle_dim_click(&mut self, click: Vec2) {
        use cad_kernel::{Dim, DimKind, LinearOrtho};
        let tol = (self.env.PkBxSz.max(8) as f64) / (self.scale as f64).max(1e-6);
        match self.dim_draft.clone() {
            DimDraftState::Off => {
                // Defensive — shouldn't reach here unless Tool::Dim
                // got armed without the command. Reset.
                self.tool = Tool::None;
            }
            DimDraftState::WaitingForP1 => {
                // Check the click against existing dobjects. If it
                // hits a Circle or Arc, jump straight to Radius
                // drafting; otherwise treat the click as P1 of a
                // linear dim.
                let hit_circ = self
                    .nearest_entity_under(click, tol)
                    .and_then(|i| self.doc.dobjects.get(i))
                    .and_then(|d| match &d.geom {
                        Geom::Circle(c) => Some((
                            c.center,
                            c.center + (click - c.center).normalized() * c.radius,
                        )),
                        Geom::Arc(a) => Some((
                            a.center,
                            a.center + (click - a.center).normalized() * a.radius,
                        )),
                        _ => None,
                    });
                if let Some((center, on_circle)) = hit_circ {
                    self.dim_draft = DimDraftState::WaitingForDimLinePos {
                        kind: DimDraftKind::Radius { center, on_circle },
                    };
                    self.set_prompt(
                        "dim: click leader position  (D toggles to Diameter, Esc cancels)"
                            .to_string(),
                    );
                } else {
                    self.dim_draft = DimDraftState::WaitingForP2 { p1: click };
                    self.set_prompt("dim: click second point  [Esc cancels]".to_string());
                }
            }
            DimDraftState::WaitingForP2 { p1 } => {
                self.dim_draft = DimDraftState::WaitingForDimLinePos {
                    kind: DimDraftKind::Linear {
                        p1,
                        p2: click,
                        ortho: LinearOrtho::Aligned,
                    },
                };
                self.set_prompt("dim: click dim line position  [Esc cancels]".to_string());
            }
            DimDraftState::WaitingForDimLinePos { kind } => {
                let dim_kind = match kind {
                    DimDraftKind::Linear { p1, p2, ortho } => DimKind::Linear {
                        p1,
                        p2,
                        dimline_pos: click,
                        ortho,
                    },
                    DimDraftKind::Radius { center, on_circle } => DimKind::Radius {
                        center,
                        on_circle,
                        leader_end: click,
                    },
                    DimDraftKind::Diameter { center, on_circle } => DimKind::Diameter {
                        center,
                        on_circle,
                        leader_end: click,
                    },
                };
                self.add_dobject(
                    Geom::Dimension(Dim {
                        kind: dim_kind,
                        style: self.current_dim_style,
                        text_override: None,
                    }),
                    "canvas",
                );
                self.dim_draft = DimDraftState::Off;
                self.tool = Tool::None;
                self.clear_prompt();
            }
        }
    }

    fn commit_text_at_with_style(&mut self, pos: Vec2, string: &str, height: f64, style_id: u32) {
        if string.is_empty() {
            self.history.push("  ! text: empty string skipped".into());
            return;
        }
        if height <= 1e-9 {
            self.history
                .push("  ! text: non-positive height (set TxHt)".into());
            return;
        }
        // Clamp style id to the table — guard against stale dialog ids
        // after a style was deleted between open + commit.
        let style = if (style_id as usize) < self.doc.text_styles.len() {
            style_id
        } else {
            cad_kernel::TextStyleTable::STANDARD
        };
        self.add_dobject(
            Geom::Text(cad_kernel::Text {
                position: pos,
                height,
                angle: 0.0,
                text: string.into(),
                h_align: cad_kernel::TextHAlign::Left,
                v_align: cad_kernel::TextVAlign::Baseline,
                style,
                font_name: String::new(),
                bold: false,
                oblique: 0.0,
                width_factor: 1.0,
                outline_only: false,
                outline_width: 0.0,
                underline: false,
                list_mode: cad_kernel::TextListKind::None,
                line_spacing: 1.0,
            }),
            "canvas",
        );
        self.history.push(format!(
            "  + text: \"{}\" @ ({:.3},{:.3}) h={}",
            string, pos.x, pos.y, height
        ));
        self.clear_prompt();
    }

    #[track_caller]
    /// §5 / WP6.1 (DOKKANDAR_MERGE_SPEC §4): apply the current Specs-rail defaults
    /// (color / linetype / lineweight) + the active layer to a BRAND-NEW dobject's
    /// style. This moved OUT of `Document::push` (which mis-stamped default-styled
    /// COPIES) into the app layer, where "fresh interactive draw" intent is known.
    /// Called ONLY from the fresh-draw commits: `add_dobject` + the hatch commit.
    /// Copies / paste / array / mirror / load push straight through the now-dumb
    /// `Document::push` and are never stamped. Only a still-fully-default style is
    /// stamped (never clobbers an explicitly-styled object).
    pub(super) fn stamp_fresh_style(&self, style: &mut cad_kernel::Style) {
        let fresh_default = style.layer == cad_kernel::LayerTable::LAYER_ZERO
            && style.color == cad_kernel::Color::ByLayer
            && style.linetype == cad_kernel::LinetypeTable::CONTINUOUS
            && style.linetype_scale == 1.0
            && style.lineweight == cad_kernel::Lineweight::ByLayer
            && style.visible;
        if fresh_default {
            style.color = self.doc.current_color;
            style.linetype = self.doc.current_linetype;
            style.lineweight = self.doc.current_lineweight;
        }
        if style.layer == cad_kernel::LayerTable::LAYER_ZERO
            && self.doc.layers.active != cad_kernel::LayerTable::LAYER_ZERO
        {
            style.layer = self.doc.layers.active;
        }
    }

    pub(super) fn add_dobject(&mut self, geom: Geom, origin: &str) {
        let d = describe(&geom);
        let kind = dobject_kind_name(&geom).to_string();
        let mut dobj = DObject::new(geom);
        self.stamp_fresh_style(&mut dobj.style); // §5/WP6.1 fresh-draw stamp
                                                 // Layout tabs (upstream parity): while a layout is active, fresh
                                                 // geometry lands in that layout's PAPER-SPACE entity list (mm coords),
                                                 // not the model. All save/load, undo snapshots and the tab's paper
                                                 // render read the same per-layout store.
        let (i, handle, went_paper) = if let Some(li) = self.doc.active_layout {
            if let Some(layout) = self.doc.layouts.get_mut(li) {
                let handle = dobj.handle;
                layout.entities.push(dobj);
                (layout.entities.len() - 1, handle, true)
            } else {
                let i = self.doc.push(dobj);
                let h = self.doc.dobjects[i].handle;
                (i, h, false)
            }
        } else {
            let i = self.doc.push(dobj);
            let h = self.doc.dobjects[i].handle;
            (i, h, false)
        };
        if went_paper {
            self.touch_view();
        }
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::DocPush {
                index: i,
                geom_kind: kind,
                handle,
                summary: format!("{}  [{}]", d, origin),
            }
        );
        self.history.push(format!("  + #{} {}  [{}]", i, d, origin));
        // No auto-recompute. Intersections are only computed when the user
        // presses an ∩ button — otherwise modifying dobjects silently
        // invalidates them.
        self.intersections.clear();
        // Paper-space appends don't touch the model index (they live in the
        // layout's entity list; the paper lane draws them directly).
        if self.doc.active_layout.is_some() {
            return;
        }
        // THE INDEX ABSORBS THE ONE NEW OBJECT INSTEAD OF BEING REBUILT FROM SCRATCH.
        //
        // This used to set `index_dirty`, which costs a full O(n) rebuild on the next query:
        // measured 13.4 ms at 100k dobjects and 247.9 ms at 1.5M, to record the arrival of one
        // line. An APPEND shifts no existing index, so the grid can take it in O(1) — and when
        // it cannot (the new object lands outside the grid's fixed bounds), it says so and we
        // fall back to exactly the old behaviour.
        if !self.index_dirty
            && self
                .index
                .as_mut()
                .is_some_and(|g| g.insert_appended(&self.doc.dobjects))
        {
            // absorbed — the grid is current, nothing to rebuild
        } else {
            self.index_dirty = true;
        }
        self.touch_view();
    }

    /// Signature of everything the CACHED 3D line builders read.
    ///
    /// Modelled on `FactoryState::opaque_sig`, including the habit of saying what is deliberately
    /// absent. Two builders are cached — `sketch_lines` (every finished plane's drawing) and
    /// `plan_lines` (the 2D plan on the ground) — because they are the expensive ones and neither
    /// depends on the camera.
    ///
    /// NOT CACHED, and each for its own reason:
    ///   `overlay_lines`      READS THE CAMERA. `grid_lines` sizes the ground grid by camera
    ///                        distance and 1/sin(pitch), so it genuinely changes on every orbit.
    ///                        Measured at 0.02 ms and 172 verts, so there is nothing to win.
    ///   `picked_face_lines`  a handful of segments, and it tracks a live pick.
    ///   `live_sketch_lines`  the plane being drawn on RIGHT NOW; it changes as the mouse moves,
    ///                        which is the one thing a cache must never be between.
    fn lines_sig(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // The drawing, via the counter every mutation site advances (see `touch_view`).
        self.view_seq.hash(&mut h);
        // The 3D model, via the counter `recompute` advances. `sketch_lines` resolves colours
        // through the layer table, and the plan's z comes off the active storey, so both of the
        // model's own version signals belong here.
        self.factory.geom_version.hash(&mut h);
        // Which plane is open, because entering one moves its document out of `sketch_lines`
        // and into `live_sketch_lines` — the same drawing, drawn by a different builder.
        self.factory.session.as_ref().map(|s| s.plane).hash(&mut h);
        self.factory.model.sketches.len().hash(&mut h);
        // Toggles and placement that change WHAT IS EMITTED without changing any geometry.
        self.factory.show_plan.hash(&mut h);
        self.factory.plan_xray.hash(&mut h);
        self.factory.active_base_z().to_bits().hash(&mut h);
        // The document the plan is read FROM, and the scale it is read at. A unit change moves
        // every line without touching a single coordinate.
        self.doc.units.metres_per_unit.to_bits().hash(&mut h);
        self.doc.dobjects.len().hash(&mut h);
        h.finish()
    }

    /// Sort `lines` (GL_LINES vertex pairs) into world-space cells and return the cell index.
    ///
    /// Segments are grouped by the cell their MIDPOINT falls in, and each cell's recorded bounds
    /// are the union of its segments' real extents — not the cell's nominal square. A long segment
    /// whose midpoint sits in one cell still reaches into its neighbours, and culling on the
    /// nominal square would drop it the moment its midpoint went off screen while most of it was
    /// still visible.
    fn bucket_lines(
        lines: &mut Vec<crate::light3d::V3>,
    ) -> Vec<(glam::Vec2, glam::Vec2, usize, usize)> {
        if lines.len() < 2 {
            return Vec::new();
        }
        let (mut mn, mut mx) = (
            glam::Vec2::splat(f32::INFINITY),
            glam::Vec2::splat(f32::NEG_INFINITY),
        );
        for v in lines.iter() {
            mn = mn.min(glam::Vec2::new(v.x, v.y));
            mx = mx.max(glam::Vec2::new(v.x, v.y));
        }
        let span = (mx - mn).max(glam::Vec2::splat(1e-3));
        // ~32 x 32, so a zoomed-in view copies a few cells out of a thousand, and the index itself
        // stays small enough to scan per frame without thinking about it.
        const N: usize = 32;
        let cell = glam::Vec2::new(span.x / N as f32, span.y / N as f32);
        let idx_of = |p: glam::Vec2| -> usize {
            let cx = (((p.x - mn.x) / cell.x) as usize).min(N - 1);
            let cy = (((p.y - mn.y) / cell.y) as usize).min(N - 1);
            cy * N + cx
        };

        // Segment -> cell, then a counting sort into one contiguous buffer per cell.
        let segs = lines.len() / 2;
        let mut key: Vec<usize> = Vec::with_capacity(segs);
        for s in 0..segs {
            let (a, b) = (&lines[s * 2], &lines[s * 2 + 1]);
            key.push(idx_of(glam::Vec2::new(
                (a.x + b.x) * 0.5,
                (a.y + b.y) * 0.5,
            )));
        }
        let mut order: Vec<usize> = (0..segs).collect();
        order.sort_unstable_by_key(|s| key[*s]);

        let mut sorted: Vec<crate::light3d::V3> = Vec::with_capacity(lines.len());
        let mut cells: Vec<(glam::Vec2, glam::Vec2, usize, usize)> = Vec::new();
        let mut run: Option<(usize, usize, glam::Vec2, glam::Vec2)> = None; // (key, start, mn, mx)
        for s in order {
            let k = key[s];
            let (a, b) = (lines[s * 2], lines[s * 2 + 1]);
            let (smn, smx) = (
                glam::Vec2::new(a.x.min(b.x), a.y.min(b.y)),
                glam::Vec2::new(a.x.max(b.x), a.y.max(b.y)),
            );
            match &mut run {
                Some((rk, _, rmn, rmx)) if *rk == k => {
                    *rmn = rmn.min(smn);
                    *rmx = rmx.max(smx);
                }
                _ => {
                    if let Some((_, start, rmn, rmx)) = run.take() {
                        cells.push((rmn, rmx, start, sorted.len()));
                    }
                    run = Some((k, sorted.len(), smn, smx));
                }
            }
            sorted.push(a);
            sorted.push(b);
        }
        if let Some((_, start, rmn, rmx)) = run.take() {
            cells.push((rmn, rmx, start, sorted.len()));
        }
        *lines = sorted;
        cells
    }

    /// Append the cached plan lines that can be SEEN into `out`.
    ///
    /// `view` is the world XY rectangle the camera covers on the plan's plane, or `None` when that
    /// cannot be bounded — a camera tilted toward the horizon sees an unbounded region, and there
    /// the honest answer is to draw everything rather than to invent a limit and cull geometry off
    /// the screen edge.
    ///
    /// Returns how many vertices were skipped, which is what the perf report shows.
    pub(super) fn emit_plan_lines(
        &self,
        out: &mut Vec<crate::light3d::V3>,
        view: Option<(glam::Vec2, glam::Vec2)>,
    ) -> usize {
        let Some((vmn, vmx)) = view else {
            out.extend_from_slice(&self.cached_plan_lines);
            return 0;
        };
        // No index (an empty or tiny plan) — copying it whole costs less than deciding not to.
        if self.cached_plan_cells.is_empty() {
            out.extend_from_slice(&self.cached_plan_lines);
            return 0;
        }
        let mut drawn = 0usize;
        for (cmn, cmx, start, end) in &self.cached_plan_cells {
            if cmx.x < vmn.x || cmn.x > vmx.x || cmx.y < vmn.y || cmn.y > vmx.y {
                continue;
            }
            out.extend_from_slice(&self.cached_plan_lines[*start..*end]);
            drawn += end - start;
        }
        self.cached_plan_lines.len() - drawn
    }

    /// Rebuild the cached line buffers if anything they read has changed. Returns whether it
    /// actually rebuilt — which is what the tests assert on.
    ///
    /// The plan's warning about this work was that caching some of the builders "makes the version
    /// check inert". It does not here, and the reason is the measurement rather than the design:
    /// the two cached builders are 15.7 ms of a 15.7 ms total, and the three left live cost
    /// 0.02 ms between them. The check covers what costs.
    pub(super) fn refresh_cached_lines(&mut self) -> bool {
        let sig = self.lines_sig();
        if sig == self.cached_lines_sig {
            return false;
        }
        self.cached_sketch_lines = self.factory.sketch_lines(&self.doc);
        let plan = Self::plan_doc_of(self.factory.session.as_ref(), &self.doc);
        self.cached_plan_lines =
            if self.factory.show_plan && !self.factory.plan_xray && !plan.dobjects.is_empty() {
                let z = self.factory.active_base_z();
                self.factory.plan_lines(plan, z)
            } else {
                Vec::new()
            };
        // Bucket for view culling — part of the CACHED, geometry-keyed data, so the per-frame work
        // is only choosing which cells to copy and the signature keeps meaning what it meant.
        self.cached_plan_cells = Self::bucket_lines(&mut self.cached_plan_lines);
        self.cached_lines_sig = sig;
        true
    }

    /// THE DRAWING CHANGED. Marks the GPU vertex cache stale AND advances [`Self::view_seq`],
    /// which is what every cross-frame cache keys off.
    ///
    /// ONE function rather than two assignments, so the two cannot drift apart. This replaced
    /// sixty-odd bare `gpu_dirty = true` sites, and the point of doing so is that a future edit
    /// site cannot invalidate one and forget the other — there is nothing left to forget.
    #[inline]
    pub(super) fn touch_view(&mut self) {
        self.gpu_dirty = true;
        self.view_seq = self.view_seq.wrapping_add(1);
    }

    /// Rebuild the spatial index if it's missing or stale. Returns the build
    /// duration in milliseconds.
    /// Try to absorb an edit that changed exactly `changed` dobjects, WITHOUT the
    /// O(n) rebuild. Falls back silently (leaves `index_dirty`) when the grid can't
    /// take it — always correct, just slow.
    ///
    /// Why: a full rebuild is ~163 ms at 1.5M dobjects (auto_cell_size 23 ms + build
    /// 140 ms) and fires from ~58 `index_dirty` sites. Moving 3,150 of 1.5M should
    /// re-bucket 3,150 — not 1,500,000. Measured by
    /// `perf_investigation::where_does_the_time_go`.
    pub(super) fn index_absorb(&mut self, changed: &[usize]) {
        if self.index_dirty || changed.is_empty() {
            return; // already stale for another reason → let the rebuild handle it
        }
        let ok = match self.index.as_mut() {
            Some(g) => g.update(&self.doc.dobjects, changed),
            None => false,
        };
        if !ok {
            self.index_dirty = true; // grid can't absorb it → full rebuild next query
        }
    }

    pub(super) fn ensure_index(&mut self) -> f64 {
        if !self.index_dirty && self.index.is_some() {
            return 0.0;
        }
        let t = std::time::Instant::now();
        // ONE bbox() sweep, not three. On the owner's real 1.5M drawing the old
        // `auto_cell_size(..) + build(..)` pair measured 417 ms, of which 71% was
        // bbox() swept three times; build_auto is 233 ms (1.76×) for a provably
        // identical grid (`build_auto_matches_auto_cell_size_plus_build`).
        let g = UniformGrid::build_auto(&self.doc.dobjects, 10.0);
        let (cells_total, idx_entries, cell_size) = g.stats();
        self.index = Some(g);
        self.index_dirty = false;
        // B24: the hatch-fill cache is a function of boundary-dobject GEOMETRY,
        // which is exactly what dirties the spatial index. Invalidate it here —
        // in lockstep with the index rebuild — so it clears on geometry change
        // but NOT on a pure selection/highlight/camera change (which sets
        // `gpu_dirty` only). See the `hatch_cache` field comment.
        self.hatch_cache.clear();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let avg = if !self.doc.dobjects.is_empty() {
            idx_entries as f64 / self.doc.dobjects.len() as f64
        } else {
            0.0
        };
        self.index_label = format!(
            "{} ents · {}×{} cells (size {:.2}) · avg {:.1} cells/ent · built {:.1} ms",
            self.doc.dobjects.len(),
            (cells_total as f64).sqrt() as usize,
            (cells_total as f64).sqrt() as usize,
            cell_size,
            avg,
            ms,
        );
        self.history.push(format!("  index: {}", self.index_label));
        // Into the RECORDER too, not just the history — this is ~half the per-edit
        // cost at scale and a dump could not see it.
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::IndexRebuild {
                dobjects: self.doc.dobjects.len(),
                elapsed_us: (ms * 1000.0) as u64,
            }
        );
        ms
    }

    pub(super) fn recompute(&mut self) {
        let all: Vec<usize> = (0..self.doc.dobjects.len()).collect();
        self.intersect_indices(&all);
    }

    /// Run intersections on a chosen subset of dobject indices.
    fn intersect_indices(&mut self, idx: &[usize]) {
        self.intersections.clear();
        for a in 0..idx.len() {
            for b in (a + 1)..idx.len() {
                self.intersections.extend(intersect(
                    &self.doc.dobjects[idx[a]].geom,
                    &self.doc.dobjects[idx[b]].geom,
                ));
            }
        }
    }

    /// "Intersect view" — uses the spatial index to fetch candidate dobjects
    /// whose bbox intersects the viewport, then runs O(k²) pairwise on those.
    pub(super) fn intersect_in_bbox(&mut self, v_min: Vec2, v_max: Vec2) {
        let build_ms = self.ensure_index();
        let t = std::time::Instant::now();

        let cands: Vec<usize> = self
            .index
            .as_ref()
            .map(|g| {
                g.query_bbox(v_min, v_max)
                    .into_iter()
                    .map(|u| u as usize)
                    .collect()
            })
            .unwrap_or_default();

        // Tight bbox cull on the candidates (grid gives loose cells).
        let mut filtered: Vec<usize> = cands
            .into_iter()
            .filter(|&i| {
                let (emin, emax) = self.doc.dobjects[i].bbox();
                !(emax.x < v_min.x || emin.x > v_max.x || emax.y < v_min.y || emin.y > v_max.y)
            })
            .collect();
        let n = filtered.len();
        let pairs_est = n.saturating_mul(n.saturating_sub(1)) / 2;

        if pairs_est > PAIR_LIMIT {
            self.last_intersect_label = format!(
                "view: {} dobjects ({} pairs) > pair cap {} — zoom in",
                n, pairs_est, PAIR_LIMIT
            );
            self.history
                .push(format!("  intersect  {}", self.last_intersect_label));
            return;
        }
        filtered.sort_unstable();
        self.intersect_indices(&filtered);
        let calc_ms = t.elapsed().as_secs_f64() * 1000.0;
        self.last_intersect_label = format!(
            "view: {} ents · {} pairs · {} hits · {:.1} ms{}",
            n,
            pairs_est,
            self.intersections.len(),
            calc_ms,
            if build_ms > 0.0 {
                format!(" (+{:.1} ms idx rebuild)", build_ms)
            } else {
                String::new()
            }
        );
        self.history
            .push(format!("  intersect  {}", self.last_intersect_label));
    }

    /// "Intersect near click" — uses the spatial index to fetch candidates
    /// inside the bbox of (click ± radius), then runs O(k²) pairwise on those.
    pub(super) fn intersect_near(&mut self, click: Vec2, world_radius: f64) {
        let build_ms = self.ensure_index();
        let t = std::time::Instant::now();
        let r2 = world_radius * world_radius;

        let cands: Vec<usize> = self
            .index
            .as_ref()
            .map(|g| {
                g.query_near(click, world_radius)
                    .into_iter()
                    .map(|u| u as usize)
                    .collect()
            })
            .unwrap_or_default();

        let mut filtered: Vec<usize> = cands
            .into_iter()
            .filter(|&i| {
                let (emin, emax) = self.doc.dobjects[i].bbox();
                let cx = click.x.clamp(emin.x, emax.x);
                let cy = click.y.clamp(emin.y, emax.y);
                let dx = click.x - cx;
                let dy = click.y - cy;
                dx * dx + dy * dy <= r2
            })
            .collect();
        let n = filtered.len();
        let pairs_est = n.saturating_mul(n.saturating_sub(1)) / 2;

        if pairs_est > PAIR_LIMIT {
            self.last_intersect_label = format!(
                "click: {} dobjects ({} pairs) > pair cap {} — shrink radius / zoom in",
                n, pairs_est, PAIR_LIMIT
            );
            self.history
                .push(format!("  intersect  {}", self.last_intersect_label));
            return;
        }
        filtered.sort_unstable();
        // Compute all hits in the candidate set, but keep only the single one
        // closest to the click point — that's what the user is actually
        // pointing at.
        let mut all_hits: Vec<Vec2> = Vec::new();
        for a in 0..filtered.len() {
            for b in (a + 1)..filtered.len() {
                all_hits.extend(intersect(
                    &self.doc.dobjects[filtered[a]].geom,
                    &self.doc.dobjects[filtered[b]].geom,
                ));
            }
        }
        self.intersections.clear();
        let total_hits = all_hits.len();
        if let Some(closest) = all_hits.into_iter().min_by(|p1, p2| {
            let d1 = (*p1 - click).len_sq();
            let d2 = (*p2 - click).len_sq();
            d1.partial_cmp(&d2).unwrap_or(std::cmp::Ordering::Equal)
        }) {
            self.intersections.push(closest);
        }
        let calc_ms = t.elapsed().as_secs_f64() * 1000.0;
        self.last_intersect_label = format!(
            "click ({:.1},{:.1}) r={:.1}: {} ents · {} pairs · {} hits (kept nearest) · {:.1} ms{}",
            click.x,
            click.y,
            world_radius,
            n,
            pairs_est,
            total_hits,
            calc_ms,
            if build_ms > 0.0 {
                format!(" (+{:.1} ms idx rebuild)", build_ms)
            } else {
                String::new()
            }
        );
        self.history
            .push(format!("  intersect  {}", self.last_intersect_label));
    }

    // ---- coordinate transforms ----------------------------------------

    pub(super) fn w2s(&self, w: Vec2, rect: egui::Rect) -> egui::Pos2 {
        let c = rect.center();
        egui::pos2(
            c.x + (w.x as f32 + self.world_offset.x) * self.scale,
            c.y - (w.y as f32 + self.world_offset.y) * self.scale,
        )
    }

    /// Project a WORLD (metre) XY point onto the 2D canvas.
    ///
    /// `w2s` maps DRAWING UNITS to screen, but everything the Factory and the lux engine hold
    /// is metres. Anything painting 3D data onto the plan must come through here, or it draws
    /// at the wrong scale the moment a drawing declares a unit — on a millimetre plan a
    /// metre-space overlay lands 1000x too small, clustered near the origin.
    ///
    /// Uses the ACTIVE document's unit, matching `w2s`: with a face sketch open the canvas IS
    /// the sketch (metre-space, k = 1), where this is the identity — which is correct.
    pub(super) fn w2s_m(&self, world_m: Vec2, rect: egui::Rect) -> egui::Pos2 {
        let u = self.doc.units.clone();
        self.w2s(
            Vec2::new(u.from_metres(world_m.x), u.from_metres(world_m.y)),
            rect,
        )
    }

    /// The visible part of an infinite xline, clipped to the canvas's world
    /// bounds (world-space segment). `None` when the line misses the view.
    pub(super) fn xline_visible_segment(
        &self,
        x: &cad_kernel::Xline,
        rect: egui::Rect,
    ) -> Option<cad_kernel::Line> {
        let lo = self.s2w(rect.min, rect);
        let hi = self.s2w(rect.max, rect);
        let grow = 2.0 / (self.scale as f64).max(1e-9);
        x.clip_to_rect(
            Vec2::new(lo.x.min(hi.x), lo.y.min(hi.y)) - Vec2::new(grow, grow),
            Vec2::new(lo.x.max(hi.x), lo.y.max(hi.y)) + Vec2::new(grow, grow),
        )
    }

    /// The visible part of a forward ray, clipped to the canvas's world
    /// bounds (world-space segment). `None` when the ray misses the view.
    pub(super) fn ray_visible_segment(
        &self,
        r: &cad_kernel::Ray,
        rect: egui::Rect,
    ) -> Option<cad_kernel::Line> {
        let lo = self.s2w(rect.min, rect);
        let hi = self.s2w(rect.max, rect);
        let grow = 2.0 / (self.scale as f64).max(1e-9);
        r.clip_to_rect(
            Vec2::new(lo.x.min(hi.x), lo.y.min(hi.y)) - Vec2::new(grow, grow),
            Vec2::new(lo.x.max(hi.x), lo.y.max(hi.y)) + Vec2::new(grow, grow),
        )
    }

    pub(super) fn s2w(&self, s: egui::Pos2, rect: egui::Rect) -> Vec2 {
        let c = rect.center();
        Vec2::new(
            ((s.x - c.x) / self.scale - self.world_offset.x) as f64,
            (-(s.y - c.y) / self.scale - self.world_offset.y) as f64,
        )
    }

    /// Inverse of [`Self::w2s_m`]: a canvas click back to WORLD metres. For click-placement of
    /// anything the 3D side or the lux engine owns.
    #[allow(dead_code)]
    pub(super) fn s2w_m(&self, s: egui::Pos2, rect: egui::Rect) -> Vec2 {
        let d = self.s2w(s, rect);
        let u = self.doc.units.clone();
        Vec2::new(u.to_metres(d.x), u.to_metres(d.y))
    }

    /// Live PLINE prompt — reflects current Line/Arc sub-mode + vertex
    /// count. AutoCAD bracket-list of options matches the spec the user
    /// provided. Phase-2 sub-options appear in the prompt so the user
    /// knows they're recognised even though they print "not yet wired"
    /// when typed.
    pub(super) fn update_pline_prompt(&mut self) {
        let n = self.pending.len();
        let prompt = match (self.pline_mode, n) {
            (PlineMode::Line, 0) =>
                "pline: click FIRST vertex  [Esc=cancel]".to_string(),
            (PlineMode::Line, _) => format!(
                "pline ({} vert): click next  \
                [Arc / Halfwidth / Length / Undo / Width  |  Enter=finish, 'c' Enter=close]",
                n),
            (PlineMode::Arc,  _) => match self.pline_arc_sub {
                PlineArcSub::Normal => format!(
                    "pline·ARC ({} vert): click endpoint  \
                    [Angle / CEnter / Direction / Halfwidth / Line / Radius / Second / Undo / Width  |  Enter=finish, 'c' Enter=close]",
                    n),
                PlineArcSub::AwaitingSecondPt =>
                    "pline·ARC·SECOND: click a point ON the arc  [U=cancel sub-flow]".to_string(),
                PlineArcSub::AwaitingSecondPtEnd(_) =>
                    "pline·ARC·SECOND: click ENDPOINT  [U=cancel sub-flow]".to_string(),
                PlineArcSub::AwaitingDirection =>
                    "pline·ARC·DIRECTION: click a point for the start tangent, or type an angle°  [U=cancel]".to_string(),
            },
        };
        self.set_prompt(prompt);
    }

    /// Snap-only "phantom" DObject built from the in-progress polyline
    /// (vertices + bulges currently in `self.pending`). Returned when
    /// the pline tool is mid-flow with at least 2 vertices so that the
    /// snap engine can offer END/MID/CEN/etc snaps against vertices the
    /// user has just clicked but hasn't committed yet. NOT inserted in
    /// `self.doc.dobjects` — it's a fresh allocation per snap frame.
    pub(super) fn pline_phantom_dobject(&self) -> Option<DObject> {
        if self.tool != Tool::Polyline {
            return None;
        }
        if self.pending.len() < 2 {
            return None;
        }
        let n = self.pending.len();
        let verts: Vec<PolyVertex> = (0..n)
            .map(|i| {
                let bulge = if i + 1 < n {
                    self.pending_bulges.get(i).copied().unwrap_or(0.0)
                } else {
                    0.0
                };
                PolyVertex {
                    pos: self.pending[i],
                    bulge,
                }
            })
            .collect();
        Some(
            Polyline {
                vertices: verts,
                closed: false,
                widths: Vec::new(),
            }
            .into(),
        )
    }

    /// PLINE arc-mode helper: the exit tangent of the most recently
    /// captured segment, used to make the next arc tangent-continuous.
    /// Returns None if there's no previous segment (just the very first
    /// vertex captured) — callers fall back to a default direction.
    fn pline_previous_exit_tangent(&self) -> Option<Vec2> {
        let n = self.pending.len();
        if n < 2 {
            return None;
        }
        let a = self.pending[n - 2];
        let b = self.pending[n - 1];
        let chord = b - a;
        if chord.len() < EPS {
            return None;
        }
        // Bulge of the segment ENDING at the previous vertex.
        let bulge = self.pending_bulges.get(n - 2).copied().unwrap_or(0.0);
        let alpha = 2.0 * bulge.atan();
        // Rotate the chord by -alpha (CW by alpha) to get the exit
        // tangent at the end of the segment. For a straight segment
        // (bulge = 0) this is the chord direction unchanged.
        let c = (-alpha).cos();
        let s = (-alpha).sin();
        let rotated = Vec2::new(chord.x * c - chord.y * s, chord.x * s + chord.y * c);
        let len = rotated.len();
        if len < EPS {
            None
        } else {
            Some(rotated / len)
        }
    }

    /// Pack the in-progress pline positions + per-segment bulges into a
    /// `Vec<PolyVertex>` ready for the kernel, and reset all transient
    /// pline state. `closed` selects whether the last bulge slot
    /// (segment from final vertex back to first) is set; it stays 0 in
    /// the MVP since `c`/close was a Line-mode action.
    /// Empty-Enter while entering a PLINE width: accept the default. In
    /// AwaitingStart it keeps the current start and moves to the ending-width
    /// prompt; in AwaitingEnd it sets end = start. Called from the empty-Enter
    /// handler so accepting a default width does NOT finish the polyline.
    pub(super) fn pline_width_accept_default(&mut self) {
        match self.pline_width_cap {
            PlineWidthCap::AwaitingStart { half } => {
                let start = self.pline_next_width.0;
                self.pline_width_cap = PlineWidthCap::AwaitingEnd { half, start };
                self.set_prompt(format!(
                    "pline {}: ending width <{:.4}>  [Enter = same]",
                    if half { "halfwidth" } else { "width" },
                    if half { start * 0.5 } else { start }
                ));
                self.refocus_cmd = true;
            }
            PlineWidthCap::AwaitingEnd { start, .. } => {
                self.pline_next_width = (start, start);
                self.pline_width_cap = PlineWidthCap::None;
                self.history.push(format!(
                    "  pline: width start={:.4} end={:.4}",
                    start, start
                ));
                self.update_pline_prompt();
            }
            PlineWidthCap::None => {}
        }
    }

    pub(super) fn drain_pline_pending(&mut self, closed: bool) -> (Vec<PolyVertex>, Vec<(f64, f64)>) {
        let n = self.pending.len();
        // Per-segment widths (parallel to bulges). seg_count = n-1 open, n
        // closed. Empty result when every segment is thin, so plain polylines
        // stay width-free.
        let seg_count = if closed { n } else { n.saturating_sub(1) };
        let mut widths: Vec<(f64, f64)> = (0..seg_count)
            .map(|i| self.pending_widths.get(i).copied().unwrap_or((0.0, 0.0)))
            .collect();
        if widths
            .iter()
            .all(|&(s, e)| s.abs() < 1e-9 && e.abs() < 1e-9)
        {
            widths.clear();
        }
        let mut verts = Vec::with_capacity(n);
        for (i, p) in self.pending.drain(..).enumerate() {
            // bulge[i] is the segment from vertex i to vertex i+1. For
            // an open polyline only i < n-1 are meaningful; for closed
            // the (n-1)-th slot is the closing segment, defaulting to 0.
            let bulge = if i + 1 < n {
                self.pending_bulges.get(i).copied().unwrap_or(0.0)
            } else if closed {
                0.0 // closing segment — Line mode for now
            } else {
                0.0
            };
            verts.push(PolyVertex { pos: p, bulge });
        }
        self.pending_bulges.clear();
        self.pending_widths.clear();
        self.pline_mode = PlineMode::Line;
        self.pline_arc_sub = PlineArcSub::Normal;
        self.pline_dir_override = None;
        self.pline_width_cap = PlineWidthCap::None;
        // pline_next_width PERSISTS as the sticky default width for the next
        // polyline (AutoCAD PLINEWID) — only an explicit `w` changes it.
        (verts, widths)
    }

    /// Tessellate one polyline-preview segment from `a` to `b` with the
    /// given bulge and paint it. Straight segment (bulge ≈ 0) renders as
    /// a single line_segment; non-zero bulge tessellates the arc into
    /// short chords. Used both for committed-segment preview and for
    /// the rubber-band to the cursor while in Arc mode.
    pub(super) fn draw_pline_preview_segment(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        a: Vec2,
        b: Vec2,
        bulge: f64,
        stroke: egui::Stroke,
    ) {
        if bulge.abs() < 1e-9 {
            painter.line_segment([self.w2s(a, rect), self.w2s(b, rect)], stroke);
            return;
        }
        let chord = b - a;
        let chord_len = chord.len();
        if chord_len < EPS {
            painter.line_segment([self.w2s(a, rect), self.w2s(b, rect)], stroke);
            return;
        }
        // theta = 4 * atan(bulge) is the (signed) included angle.
        let theta = 4.0 * bulge.atan();
        let half = theta * 0.5;
        // radius r = chord / (2 * sin(half)); sin(half) shares sign with bulge.
        let r = chord_len / (2.0 * half.sin().abs());
        // Centre offset from chord midpoint along the chord's perpendicular.
        // The sagitta (mid-deviation) is r - r*cos(half) = r * (1 - cos(half)),
        // signed by bulge so positive-bulge arcs sit on the LEFT of the chord
        // when travelling from a to b.
        let chord_hat = chord / chord_len;
        let perp = Vec2::new(-chord_hat.y, chord_hat.x); // CCW perp
        let mid = (a + b) * 0.5;
        let centre_off = r * half.cos();
        // half positive (bulge > 0): centre is to the LEFT of the chord
        //                            (in the +perp direction)
        // half negative (bulge < 0): centre is to the RIGHT (-perp)
        let centre = mid + perp * (if bulge > 0.0 { centre_off } else { -centre_off });
        // Angles from centre to a, b. CCW sweep selected by sign of bulge.
        let start_ang = (a - centre).angle();
        let end_ang = (b - centre).angle();
        let sweep = if bulge > 0.0 {
            (end_ang - start_ang).rem_euclid(std::f64::consts::TAU)
        } else {
            -((start_ang - end_ang).rem_euclid(std::f64::consts::TAU))
        };
        // Tessellation density grows with screen size of the arc.
        let arc_len_px = (r as f32 * self.scale) * sweep.abs() as f32;
        let n = (arc_len_px * 0.4).clamp(6.0, 256.0) as usize;
        let mut pts = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let ang = start_ang + sweep * t;
            let p = centre + Vec2::new(r * ang.cos(), r * ang.sin());
            pts.push(self.w2s(p, rect));
        }
        painter.add(egui::Shape::line(pts, stroke));
    }

    /// AutoCAD bulge for a tangent-continuous arc from the current last
    /// pending vertex to `end`. `bulge = tan(alpha / 2)` where alpha is
    /// the signed (CCW positive) angle from the chord direction to the
    /// start tangent.
    pub(super) fn pline_arc_bulge_to(&self, end: Vec2) -> f64 {
        let n = self.pending.len();
        if n == 0 {
            return 0.0;
        }
        let start = self.pending[n - 1];
        let chord = end - start;
        if chord.len() < EPS {
            return 0.0;
        }
        // Tangent priority: an explicit `d`/Direction override (this segment
        // only) → the previous-segment exit tangent (G1-continuous) → X+ when
        // there's only the start vertex.
        let tangent = self
            .pline_dir_override
            .or_else(|| self.pline_previous_exit_tangent())
            .unwrap_or(Vec2::new(1.0, 0.0));
        // Signed angle from chord to tangent, CCW positive.
        let cross = chord.x * tangent.y - chord.y * tangent.x;
        let dot = chord.x * tangent.x + chord.y * tangent.y;
        let alpha = cross.atan2(dot);
        (alpha / 2.0).tan()
    }

    /// The "from" point that CARD (cardinal-directions drafting lock)
    /// constrains the cursor against — the most recent locked-in point in
    /// whatever command is active. None means CARD has no anchor and
    /// therefore no effect this frame (e.g. waiting for the first
    /// endpoint of a line, or no active command at all).
    pub(super) fn card_anchor(&self) -> Option<Vec2> {
        // Prompt-flow (CIRCLE): the center/last-point is the anchor for CARD +
        // PER/TAN at the radius / next-point steps.
        if let Some(f) = &self.cmd_flow {
            match f.circle {
                CircleStep::Radius(c) | CircleStep::Diameter(c) => return Some(c),
                CircleStep::P3b(p1) | CircleStep::P2b(p1) => return Some(p1),
                CircleStep::P3c(_, p2) => return Some(p2),
                _ => {}
            }
        }
        if let MoveState::WaitingForDest(base) = self.move_state {
            return Some(base);
        }
        if let CopyState::WaitingForDest(base) = self.copy_state {
            return Some(base);
        }
        if let MirrorState::WaitingForB(a) = self.mirror_state {
            return Some(a);
        }
        if let StretchState::WaitingForDest(_, _, base) = self.stretch_state {
            return Some(base);
        }
        // Dist — the second point measures from the first, so CARD locks
        // the measurement to H/V and PER/TAN anchor on P1.
        if let DistState::WaitingForP2(p1) = self.dist_state {
            return Some(p1);
        }
        // Rotate — the angle / reference picks pivot about the pivot point,
        // so CARD snaps the rotation to the cardinal directions (0/90/…).
        match self.rotate_state {
            RotateState::WaitingForAngle(pivot)
            | RotateState::WaitingForRefSrc1(pivot)
            | RotateState::WaitingForRefSrc2(pivot, _)
            | RotateState::WaitingForRefTgt(pivot, _) => return Some(pivot),
            _ => {}
        }
        // Scale — the factor / new-length clicks measure distance from the
        // pivot, and the reference end measures from the reference start;
        // CARD locks each to H/V from its measurement anchor.
        match self.scale_state {
            ScaleState::WaitingForFactor(pivot) | ScaleState::WaitingForNewLength(pivot, _) => {
                return Some(pivot)
            }
            ScaleState::WaitingForRefEnd(_, ref_start) => return Some(ref_start),
            _ => {}
        }
        // Polyline / Line / Arc draw tools: the last captured point is
        // the CARD anchor for the next click.
        if matches!(
            self.tool,
            Tool::Line
                | Tool::Polyline
                | Tool::Spline
                | Tool::Arc
                | Tool::Ellipse
                | Tool::EllipseArc
                | Tool::Wall
                | Tool::Text
                | Tool::Rectangle
        ) {
            if let Some(p) = self.pending.last().copied() {
                return Some(p);
            }
        }
        None
    }

    /// Apply CARD + grid-snap constraints to a raw world position. Order:
    ///   1. CARD first if enabled AND an anchor exists — projects onto
    ///      whichever axis from the anchor is closer (ONLY horizontal or
    ///      vertical).
    ///   2. Grid-snap second if enabled — rounds to the nearest
    ///      `GrdSpc` multiple in both axes.
    /// Object-snap is NOT handled here; callers apply it BEFORE this so
    /// osnap always wins over both CARD and grid.
    pub(super) fn apply_constraints(&self, raw: Vec2) -> Vec2 {
        let mut p = raw;
        if self.env.CrdEnb {
            if let Some(a) = self.card_anchor() {
                let dx = (p.x - a.x).abs();
                let dy = (p.y - a.y).abs();
                if dx >= dy {
                    p.y = a.y;
                } else {
                    p.x = a.x;
                }
            }
        }
        let grid_s = self.grid_spacing();
        if self.env.GrdSnp && grid_s > 0.0 {
            let s = grid_s;
            p.x = (p.x / s).round() * s;
            p.y = (p.y / s).round() * s;
        }
        p
    }

    /// World position of the cursor with all current constraints applied
    /// in priority order: osnap > CARD > grid-snap > raw. Used both
    /// for click-capture and for live-preview rendering so the user
    /// sees exactly what the click will produce.
    pub(super) fn cursor_world_constrained(
        &self,
        screen_pos: Option<egui::Pos2>,
        rect: egui::Rect,
        snap_hit: Option<Vec2>,
    ) -> Option<Vec2> {
        if let Some(w) = snap_hit {
            return Some(w);
        } // osnap wins
        screen_pos.map(|p| self.apply_constraints(self.s2w(p, rect)))
    }

    /// Find the dobject nearest to a world point, within `tol_world`. Uses the
    /// spatial index when available so it's cheap even at millions of dobjects.
    pub(super) fn nearest_entity_under(&self, w: Vec2, tol_world: f64) -> Option<usize> {
        let cands: Vec<usize> = if let (Some(g), false) = (self.index.as_ref(), self.index_dirty) {
            g.query_near(w, tol_world)
                .into_iter()
                .map(|u| u as usize)
                .collect()
        } else {
            (0..self.doc.dobjects.len()).collect()
        };
        let mut best: Option<(usize, f64)> = None;
        for i in &cands {
            // LOCKED AND HIDDEN OBJECTS ARE NOT UNDER THE CURSOR.
            //
            // Every click-pick in the app comes through here, which is why the check belongs here
            // and not at each of the eighteen call sites. A locked object is therefore clicked
            // THROUGH, exactly as in AutoCAD: the click reads as a click on empty space, and what
            // is behind the locked object can be picked instead.
            //
            // `is_selectable` is "not locked AND it renders", so this also stops a hidden or
            // frozen layer being selected by clicking where it used to be — which the renderer
            // already refuses to draw.
            if !self.doc.is_selectable(*i) {
                continue;
            }
            let d = self.doc.dobjects[*i].distance_to_point(w);
            if d < tol_world {
                if best.map_or(true, |(_, bd)| d < bd) {
                    best = Some((*i, d));
                }
            }
        }
        if best.is_some() {
            return best.map(|(i, _)| i);
        }
        // Fallback for Hatch dobjects: kernel-level distance is INFINITY
        // (Hatch can't resolve its own boundary without the Document), so
        // a click inside the fill never gets picked by the loop above.
        // Walk all hatches whose resolved interior contains `w` and pick
        // the SMALLEST by bbox area (mirrors hatch pick-point's
        // smallest-containing rule — clicking the inner island wins over
        // the outer ring). Boundary-edge clicks already won above, so
        // this fallback only kicks in for clicks in the empty fill.
        // OVER THE CANDIDATES, NOT THE WHOLE DOCUMENT. Both fallbacks below used to walk every
        // dobject on every MISSED click — measured floor 0.95 ms against 5.8 µs for the indexed
        // query they are backing up, i.e. 160× the cost of the thing they exist to supplement, and
        // it fires on every click that lands on empty space.
        //
        // `cands` loses nothing here. `UniformGrid` classes Hatch and BlockRef as
        // view-independent and appends them to the result of EVERY query regardless of the
        // rectangle, precisely because their true extent cannot be known without the Document. So
        // the candidate list already contains every one of them; when the index is dirty it is the
        // whole document anyway. Same answer, bounded work.
        let mut hatch_best: Option<(usize, f64)> = None;
        for &i in &cands {
            if !self.doc.is_selectable(i) {
                continue;
            } // the fallbacks obey the lock too
            let Some(dob) = self.doc.dobjects.get(i) else {
                continue;
            };
            let Geom::Hatch(h) = &dob.geom else {
                continue;
            };
            let loops = self.resolve_hatch_loops(h);
            if loops.is_empty() {
                continue;
            }
            let inside = loops
                .iter()
                .fold(false, |acc, l| acc ^ point_in_polygon(w, l.iter().copied()));
            if !inside {
                continue;
            }
            // Loop bbox area as a smallest-containing tiebreak.
            let mut mn = Vec2::new(f64::INFINITY, f64::INFINITY);
            let mut mx = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
            for l in &loops {
                for v in l {
                    if v.x < mn.x {
                        mn.x = v.x;
                    }
                    if v.y < mn.y {
                        mn.y = v.y;
                    }
                    if v.x > mx.x {
                        mx.x = v.x;
                    }
                    if v.y > mx.y {
                        mx.y = v.y;
                    }
                }
            }
            let area = (mx.x - mn.x).max(0.0) * (mx.y - mn.y).max(0.0);
            if hatch_best.map_or(true, |(_, ba)| area < ba) {
                hatch_best = Some((i, area));
            }
        }
        if hatch_best.is_some() {
            return hatch_best.map(|(i, _)| i);
        }
        // BlockRef fallback — kernel distance_to_point is INFINITY for
        // instances (contents resolve through the Document), so the main
        // loop above never matches them. Test the transformed contents
        // directly (one nesting level; nested instances contribute their
        // insertion point only — full recursion when pick perf matters).
        // Same reasoning as the hatch fallback above — block references are view-independent too,
        // so the candidate list already holds every one of them.
        let mut blk_best: Option<(usize, f64)> = None;
        for &i in &cands {
            if !self.doc.is_selectable(i) {
                continue;
            } // the fallbacks obey the lock too
            let Some(dob) = self.doc.dobjects.get(i) else {
                continue;
            };
            let Geom::BlockRef(br) = &dob.geom else {
                continue;
            };
            let Some(blk) = self.doc.blocks.get(br.block) else {
                continue;
            };
            for cd in &blk.dobjects {
                let g = br.transform_geom(&cd.geom, blk.base);
                let dist = match &g {
                    Geom::BlockRef(nb) => nb.insert.dist(w),
                    _ => g.distance_to_point(w),
                };
                if dist < tol_world && blk_best.map_or(true, |(_, bd)| dist < bd) {
                    blk_best = Some((i, dist));
                }
            }
        }
        blk_best.map(|(i, _)| i)
    }

    /// World bbox of a block instance, resolved through the block table:
    /// union of the transformed contents' bboxes (nested instances
    /// contribute their insertion point — one level, consistent with the
    /// pick fallback). Falls back to the insertion point when dangling.
    pub(super) fn resolved_blockref_bbox(&self, br: &cad_kernel::BlockRef) -> (Vec2, Vec2) {
        let mut min = br.insert;
        let mut max = br.insert;
        if let Some(blk) = self.doc.blocks.get(br.block) {
            let derived = self.block_derived_geoms(blk, &br.param_values);
            for dg in &derived {
                let g = br.transform_geom(dg, blk.base);
                let (gmin, gmax) = match &g {
                    Geom::BlockRef(nb) => (nb.insert, nb.insert),
                    _ => g.bbox(),
                };
                min.x = min.x.min(gmin.x);
                min.y = min.y.min(gmin.y);
                max.x = max.x.max(gmax.x);
                max.y = max.y.max(gmax.y);
            }
        }
        (min, max)
    }

    /// Commit an in-progress PLINE/SPLINE as a FINISHED open dobject when the
    /// draw is ENDED or INTERRUPTED (Enter, switching tools, or starting another
    /// command). Placed vertices are real geometry and are NEVER discarded — a
    /// polyline with ≥2 verts becomes an open polyline, a spline with ≥3 control
    /// points a curve. Clears the pending draw state. Returns true if it
    /// committed. (Esc is the ONE exception — it drops only the last segment.)
    pub(super) fn commit_active_draw(&mut self) -> bool {
        if self.tool == Tool::Polyline && self.pending.len() >= 2 {
            let (verts, widths) = self.drain_pline_pending(false);
            self.add_dobject(
                Geom::Polyline(Polyline {
                    vertices: verts,
                    closed: false,
                    widths,
                }),
                "canvas",
            );
            true
        } else if self.tool == Tool::Spline && self.pending.len() >= 3 {
            // Degree-3 (cubic) clamped/open uniform B-spline through the control
            // points; lower degree for <4 controls so the curve stays well-formed.
            // `qb`/QuadBezier overrides the degree to 2 (quadratic — three
            // control clicks P0..P2, AutoCAD QB).
            let n = self.pending.len();
            let degree = self.spline_degree_override.unwrap_or(3).min(n - 1);
            let was_qb = self.spline_degree_override == Some(2);
            self.spline_degree_override = None;
            let ctrls: Vec<Vec2> = self.pending.drain(..).collect();
            self.pending_bulges.clear();
            let spline =
                cad_kernel::Spline::new_bspline(degree, ctrls).with_width(self.spline_width);
            self.add_dobject(Geom::Spline(spline), "canvas");
            if was_qb {
                self.history.push("  ✓ quadbezier committed".into());
            }
            true
        } else {
            false
        }
    }

    // ---- interactive draw: finalise dobject from clicked points ---------

    pub(super) fn try_finalise(&mut self) {
        match (self.tool, self.pending.len()) {
            (Tool::Line, 2) => {
                let g = Geom::Line(Line {
                    a: self.pending[0],
                    b: self.pending[1],
                });
                let last = self.pending[1];
                self.pending.clear();
                self.add_dobject(g, "canvas");
                // Continue the chain: the last endpoint becomes the next
                // segment's start, so successive clicks draw CONNECTED lines
                // (AutoCAD LINE). Esc ends the chain + exits the tool; re-run
                // `line` (or empty Enter) for a fresh, separate line.
                self.pending.push(last);
            }
            (Tool::Rectangle, 2) => {
                // Two opposite corners → an axis-aligned closed polyline.
                let a = self.pending[0];
                let b = self.pending[1];
                self.pending.clear();
                if (b.x - a.x).abs() < EPS || (b.y - a.y).abs() < EPS {
                    self.history
                        .push("  ! rectangle: corners are collinear (zero width or height)".into());
                } else {
                    self.add_dobject(rect_polyline(a, b), "canvas");
                }
            }
            (Tool::Wall, 2) => {
                // SMART DOBJECT: one Geom::Wall, not two Lines. The kernel
                // renders it as two side lines; `wall::solve_faces` mitres
                // the corner wherever endpoints coincide.
                let start = self.pending[0];
                let end = self.pending[1];
                // `WlThk` is an honest METRE value (settings.rs documents it as "0.20 (200mm)"),
                // but a Wall's `thickness` is a DRAWING-UNIT length. Convert on the way in, so
                // `dlen_m` converts it back to 0.2 m at promotion whatever the document's unit
                // is — and so the two arms of the promotion match (a drawn wall and an imported
                // centerline must not come out 1000x apart).
                let thk = self.doc.units.from_metres(self.env.WlThk);
                if (end - start).len() < EPS || thk <= 1e-9 {
                    // Degenerate click (no motion / bad thickness): drop the
                    // duplicate point but keep the chain anchored.
                    self.pending = vec![start];
                } else {
                    self.add_dobject(
                        Geom::Wall(cad_kernel::Wall {
                            start,
                            end,
                            thickness: thk,
                            style: self.current_wall_style,
                            bulge: 0.0,
                        }),
                        "canvas",
                    );
                    self.history.push(format!("  + wall: thickness={}", thk));
                    // CHAINED drawing: retain the end as the start of the
                    // next segment, so a run of walls shares endpoints and
                    // auto-mitres. Enter/Esc ends the run.
                    self.pending = vec![end];
                }
            }
            (Tool::Text, 1) => {
                // Position captured. If `pending_text` is set, commit
                // Text immediately. Otherwise enter WaitingForString —
                // the next cmd-line input is consumed as the body.
                let pos = self.pending[0];
                self.pending.clear();
                let height = self.env.TxHt;
                if let Some(string) = self.pending_text.take() {
                    // Scripted quick path (`text "Hello"`) — commit at the click.
                    self.commit_text_at(pos, &string, height);
                    self.text_draft = TextDraftState::Off;
                    self.tool = Tool::None;
                } else {
                    // Interactive path: the dockable Text panel is already open
                    // (opened when the command started). This click just sets the
                    // anchor; the user types in the panel and presses Apply/Enter.
                    self.text_draft = TextDraftState::WaitingForString(pos);
                    // Drop any armed-but-unentered Height/Angle so it can't eat
                    // the next command-line input.
                    self.text_waiting_height = false;
                    self.text_waiting_angle = false;
                    self.text_input_dialog_anchor = Some(pos);
                    self.text_input_dialog_focus = true; // focus the content box
                    if !self.text_input_dialog_open {
                        // Robustness: if the panel was closed, re-open + seed it.
                        if self.hatch_dialog_open {
                            self.hatch_dialog_open = false;
                        }
                        self.text_input_dialog_open = true;
                        self.text_input_dialog_buf.clear();
                        self.text_input_dialog_height = height.max(1e-3);
                        if (self.text_input_dialog_style_id as usize) >= self.doc.text_styles.len()
                        {
                            self.text_input_dialog_style_id = cad_kernel::TextStyleTable::STANDARD;
                        }
                        let sid = self.text_input_dialog_style_id;
                        self.seed_text_dialog_from_style(sid);
                    }
                    self.set_prompt("text: type in the panel, then Apply".to_string());
                }
            }
            (Tool::Point, 1) => {
                let loc = self.pending[0];
                self.pending.clear();
                self.add_dobject(
                    Geom::Point(Point {
                        location: loc,
                        style: self.current_point_style,
                        size: self.current_point_size,
                    }),
                    "canvas",
                );
            }
            // Polyline never finalises via the click-count path — it
            // accumulates clicks forever and finalises when the user
            // presses Enter (see `finish_polyline`).
            (Tool::Circle, 2) => {
                let c = self.pending[0];
                let p = self.pending[1];
                let r = c.dist(p);
                self.pending.clear();
                if r > EPS {
                    self.add_dobject(
                        Geom::Circle(Circle {
                            center: c,
                            radius: r,
                        }),
                        "canvas",
                    );
                } else {
                    self.history.push("  ! circle has zero radius".into());
                }
            }
            (Tool::Ellipse, 3) => {
                // 1) centre  2) end of major axis  3) any point on the minor
                // side; semi-minor is the perpendicular distance from the
                // major-axis line to that third click.
                let c = self.pending[0];
                let me = self.pending[1];
                let mp = self.pending[2];
                self.pending.clear();
                let major = me - c;
                if major.len() < EPS {
                    self.history
                        .push("  ! ellipse: zero-length major axis".into());
                    return;
                }
                // Project (mp - c) onto the minor-axis direction.
                let v_hat = major.normalized().perp();
                let semi_minor = (mp - c).dot(v_hat).abs();
                match ellipse_center_major_minor(c, me, semi_minor) {
                    Some(el) => self.add_dobject(Geom::Ellipse(el), "canvas"),
                    None => self
                        .history
                        .push("  ! ellipse: degenerate inputs (zero major or minor)".into()),
                }
            }
            (Tool::EllipseArc, 5) => {
                // 1)centre  2)major_end  3)minor side  4)start point  5)end point
                let c = self.pending[0];
                let me = self.pending[1];
                let mp = self.pending[2];
                let sp = self.pending[3];
                let ep = self.pending[4];
                self.pending.clear();
                let major = me - c;
                if major.len() < EPS {
                    self.history
                        .push("  ! ellipse arc: zero-length major axis".into());
                    return;
                }
                let v_hat = major.normalized().perp();
                let semi_minor = (mp - c).dot(v_hat).abs();
                let Some(el) = ellipse_center_major_minor(c, me, semi_minor) else {
                    self.history
                        .push("  ! ellipse arc: degenerate inputs".into());
                    return;
                };
                // Convert start/end click points to parameters on the ellipse
                // (nearest_param projects them onto the curve, so the user
                // can click roughly near the ellipse and the system snaps the
                // bounds to it).
                let t_start = el.nearest_param(sp);
                let t_end = el.nearest_param(ep);
                let sweep_raw = (t_end - t_start).rem_euclid(std::f64::consts::TAU);
                let sweep = if sweep_raw < 1e-6 {
                    std::f64::consts::TAU
                } else {
                    sweep_raw
                };
                self.add_dobject(
                    Geom::EllipseArc(EllipseArc {
                        ellipse: el,
                        start_param: t_start.rem_euclid(std::f64::consts::TAU),
                        sweep_param: sweep,
                    }),
                    "canvas",
                );
            }
            (Tool::Arc, n) if n >= self.arc_method.click_count() => {
                let needed = self.arc_method.click_count();
                let pts: Vec<Vec2> = self.pending.drain(..needed).collect();
                let arc_opt = match self.arc_method {
                    ArcMethod::ThreePoints => arc_three_points(pts[0], pts[1], pts[2]),
                    // S,C,E: 1st = start, 2nd = center, 3rd = end → reorder for kernel
                    ArcMethod::StartCenterEnd => arc_center_start_end(pts[1], pts[0], pts[2]),
                    // C,S,E: 1st = center, 2nd = start, 3rd = end → kernel signature
                    ArcMethod::CenterStartEnd => arc_center_start_end(pts[0], pts[1], pts[2]),
                    _ => {
                        self.history.push(format!(
                            "  ! arc method '{}' not implemented yet",
                            self.arc_method.name()
                        ));
                        return;
                    }
                };
                let tag = format!("canvas ({})", self.arc_method.name());
                match arc_opt {
                    Some(arc) => self.add_dobject(Geom::Arc(arc), &tag),
                    None => self
                        .history
                        .push("  ! could not build arc (collinear / zero radius)".into()),
                }
            }
            _ => {}
        }
    }

    // ---- array generator -----------------------------------------------

    /// POLAR array — `array_count` copies of every source stepped evenly
    /// around `array_center` over `array_fill_deg` total. The first copy is
    /// the source itself (skipped). `array_rotate_items` spins each copy
    /// about the pole (AutoCAD default); off = revolve the bbox anchor
    /// without spinning.
    pub(super) fn generate_polar_array(&mut self) {
        let sources: Vec<DObject> = self
            .selection
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).cloned())
            .collect();
        if sources.is_empty() {
            self.history.push("  ! array: no sources selected".into());
            return;
        }
        let count = self.array_count.max(1);
        let center = self.array_center;
        let new_dobjects = count.saturating_sub(1) * sources.len();
        if new_dobjects == 0 {
            self.history
                .push("  ! array (polar): count must be ≥ 2".into());
            return;
        }
        // A full 360° fill must NOT double up item 0 and item `count` on the
        // same spot — the step divides the fill by `count` for a full circle
        // and by `count-1` for a partial fan (AutoCAD convention).
        let full = (self.array_fill_deg.abs() - 360.0).abs() < 1e-6;
        let divisor = if full { count } else { (count - 1).max(1) };
        let step = (self.array_fill_deg / divisor as f64).to_radians();
        let rotate = self.array_rotate_items;

        self.doc.dobjects.reserve(new_dobjects);
        for k in 1..count {
            let ang = step * k as f64;
            let (sn, cs) = ang.sin_cos();
            for d in duplicate_dobjects(&sources, |g| {
                if rotate {
                    g.rotated(center, ang)
                } else {
                    let (mn, mx) = g.bbox();
                    let anchor = (mn + mx) * 0.5;
                    let rel = anchor - center;
                    let moved =
                        center + Vec2::new(rel.x * cs - rel.y * sn, rel.x * sn + rel.y * cs);
                    g.translated(moved - anchor)
                }
            }) {
                self.doc.dobjects.push(d);
            }
        }
        let new_total = self.doc.dobjects.len();
        self.intersections.clear();
        self.index_dirty = true;
        self.gpu_dirty = true;
        self.history.push(format!(
            "  + array (polar): {} items × {} source(s) over {}° = {} new → {} total",
            count,
            sources.len(),
            self.array_fill_deg,
            new_dobjects,
            new_total,
        ));
        self.ensure_index();
    }

    /// The (point, tangent-angle) placements for a PATH array along
    /// `path_geom`, honouring divide/measure spacing + the L/M/R anchor.
    /// Empty if the path can't be sampled.
    pub(super) fn path_array_placements(&self, path_geom: &Geom) -> Vec<(Vec2, f64)> {
        let pts = sample_path_points(path_geom, 600);
        if pts.len() < 2 {
            return Vec::new();
        }
        let mut cum = vec![0.0_f64; pts.len()];
        for i in 1..pts.len() {
            cum[i] = cum[i - 1] + pts[i].dist(pts[i - 1]);
        }
        let total = cum[pts.len() - 1];
        if total < 1e-9 {
            return Vec::new();
        }
        let at = |s: f64| -> (Vec2, f64) {
            let s = s.clamp(0.0, total);
            let mut i = 1;
            while i < pts.len() && cum[i] < s {
                i += 1;
            }
            let i = i.min(pts.len() - 1);
            let seg = cum[i] - cum[i - 1];
            let f = if seg > 1e-12 {
                (s - cum[i - 1]) / seg
            } else {
                0.0
            };
            let p = pts[i - 1] + (pts[i] - pts[i - 1]) * f;
            let dir = pts[i] - pts[i - 1];
            (p, dir.y.atan2(dir.x))
        };
        let ss: Vec<f64> = if self.array_path_measure {
            let dist = self.array_path_dist.abs().max(1e-6);
            let count = ((total / dist).floor() as usize + 1).max(1);
            let span = count.saturating_sub(1) as f64 * dist;
            let base = match self.array_path_anchor {
                PathAnchor::Start => 0.0,
                PathAnchor::End => total - span,
                PathAnchor::Middle => (total - span) * 0.5,
            };
            (0..count).map(|k| base + k as f64 * dist).collect()
        } else {
            let count = self.array_count.max(2);
            (0..count)
                .map(|k| total * k as f64 / (count - 1) as f64)
                .collect()
        };
        ss.into_iter().map(at).collect()
    }

    /// PATH array — copies of every source at each path placement, rotated
    /// onto the tangent when `array_path_align`. Sources are always kept.
    pub(super) fn generate_path_array(&mut self) {
        let sources: Vec<DObject> = self
            .selection
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).cloned())
            .collect();
        if sources.is_empty() {
            self.history.push("  ! array: no sources selected".into());
            return;
        }
        let Some(pi) = self.array_path_idx else {
            self.history
                .push("  ! array (path): pick a path curve first".into());
            return;
        };
        let Some(path_geom) = self.doc.dobjects.get(pi).map(|d| d.geom.clone()) else {
            self.history
                .push("  ! array (path): the path dobject is gone — re-pick".into());
            return;
        };
        let placements = self.path_array_placements(&path_geom);
        if placements.len() < 2 {
            self.history
                .push("  ! array (path): that dobject isn't a usable path curve".into());
            return;
        }
        let base_ang = placements[0].1;
        let align = self.array_path_align;
        self.doc.dobjects.reserve(placements.len() * sources.len());
        for (pt, ang) in &placements {
            let (pt, ang) = (*pt, *ang);
            let rot = if align { ang - base_ang } else { 0.0 };
            for d in duplicate_dobjects(&sources, |g| {
                let (mn, mx) = g.bbox();
                let anchor = (mn + mx) * 0.5;
                let g = g.translated(pt - anchor);
                if align {
                    g.rotated(pt, rot)
                } else {
                    g
                }
            }) {
                self.doc.dobjects.push(d);
            }
        }
        let new_total = self.doc.dobjects.len();
        self.intersections.clear();
        self.index_dirty = true;
        self.gpu_dirty = true;
        self.history.push(format!(
            "  + array (path): {} items × {} source(s) along #{} = {} new → {} total",
            placements.len(),
            sources.len(),
            pi,
            placements.len() * sources.len(),
            new_total,
        ));
        self.ensure_index();
    }

    pub(super) fn generate_array(&mut self) {
        // Multi-source: iterate `self.selection` (the standard basket).
        // Every grid cell instantiates a copy of every source, offset
        // by that cell's (c·dx, r·dy). The cell at (0, 0) is the
        // source itself — skipped so we don't duplicate the originals.
        let sources: Vec<DObject> = self
            .selection
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).cloned())
            .collect();
        if sources.is_empty() {
            self.history.push("  ! array: no sources selected".into());
            return;
        }
        let cols = self.array_cols.max(1);
        let rows = self.array_rows.max(1);
        let dx = self.array_dx;
        let dy = self.array_dy;
        let cells = cols * rows;
        let new_dobjects = cells.saturating_sub(1) * sources.len();

        self.doc.dobjects.reserve(new_dobjects);
        for r in 0..rows {
            for c in 0..cols {
                if r == 0 && c == 0 {
                    continue;
                } // skip the source cell
                let off = Vec2::new(c as f64 * dx, r as f64 * dy);
                for s in &sources {
                    self.doc.dobjects.push(s.translated(off));
                }
            }
        }
        let new_total = self.doc.dobjects.len();
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
        self.history.push(format!(
            "  + array: {} cells × {} source(s) = {} new → {} total dobjects",
            cells,
            sources.len(),
            new_dobjects,
            new_total,
        ));
        self.ensure_index();
    }
}
