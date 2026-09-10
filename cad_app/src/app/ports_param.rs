use super::*;

// ============ parametric mode (from dokkandar/Auto_RASM) ============

/// Parametric DOF overlay — tints each constrainable entity by whether the
/// solver considers it fully defined (green, all params locked) or still free
/// (blue), the way SolidWorks turns sketch geometry black vs. blue. Reads the
/// per-handle map cached in `app.parametric.defined`. Drawn on top of the normal
/// geometry as a translucent halo.
pub(super) fn draw_param_overlay(painter: &egui::Painter, rect: egui::Rect, app: &CadApp) {
    let blue = egui::Color32::from_rgb(80, 160, 255).gamma_multiply(0.6);
    let green = egui::Color32::from_rgb(110, 220, 140).gamma_multiply(0.55);
    for d in &app.doc.dobjects {
        let col = match app.parametric.defined.get(&d.handle).copied() {
            Some(true) => green,
            Some(false) => blue,
            None => continue, // not part of the parametric sketch
        };
        match &d.geom {
            Geom::Line(l) => {
                let a = app.w2s(l.a, rect);
                let b = app.w2s(l.b, rect);
                painter.line_segment([a, b], egui::Stroke::new(3.0, col));
            }
            Geom::Circle(c) => {
                let center = app.w2s(c.center, rect);
                let r = (c.radius * app.scale as f64) as f32;
                if r.is_finite() && r > 0.0 {
                    painter.circle_stroke(center, r, egui::Stroke::new(3.0, col));
                }
            }
            // straight wall: tint its centerline (the constrained geometry)
            Geom::Wall(w) if w.bulge.abs() < 1e-9 => {
                let a = app.w2s(w.start, rect);
                let b = app.w2s(w.end, rect);
                painter.line_segment([a, b], egui::Stroke::new(3.0, col));
            }
            _ => {}
        }
    }
}

impl CadApp {
    /// Keep the constraint LINK live: if the drawing's geometry changed since the
    /// last solve (the user moved/rotated/stretched/grip-edited something), re-run
    /// the solver with the current SELECTION pinned as the driver — so the edited
    /// entity stays where the user put it and every linked entity follows. Runs
    /// each frame while parametric mode is active (skipped while a drawing tool is
    /// mid-stroke, to not perturb new geometry being placed).
    fn param_auto_resolve(&mut self) {
        if !self.parametric.active || !self.parametric.keep_link {
            return;
        }
        if self.parametric.constraints.is_empty() {
            return;
        }
        if self.tool != Tool::None {
            return;
        } // don't tug geometry while drawing
        let sig = crate::param_editor::geom_signature(&self.doc);
        if sig == self.parametric.last_solved_sig {
            return;
        } // nothing edited
        let drivers: std::collections::HashSet<Handle> = self
            .selection
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).map(|d| d.handle))
            .collect();
        let out = crate::param_editor::solve_doc_driven(&mut self.doc, &self.parametric, &drivers);
        self.parametric.last_trace = out.trace;
        self.parametric.last_solved_sig = crate::param_editor::geom_signature(&self.doc);
        self.index_dirty = true;
        self.touch_view();
    }

    /// Parametric MODE panel — add geometric/dimensional/variable constraints to
    /// the selected geometry, see the degrees-of-freedom diagnosis, and Solve.
    /// The drawing is built with the NORMAL tools; this only adds the constraint
    /// layer (the cad_param solver moves the geometry on Solve).
    pub(crate) fn render_param_panel(&mut self, ctx: &egui::Context) {
        if !self.parametric.active {
            return;
        }
        use crate::param_editor::{CRef, PendingKind};

        // Keep the link live: re-solve if the user edited geometry this frame.
        self.param_auto_resolve();

        // Drop constraints whose geometry has been deleted (handles gone) so old
        // edits don't linger and make every solve "apply to everything".
        crate::param_editor::prune_constraints(&self.doc, &mut self.parametric.constraints);

        // ---- recompute the DOF diagnosis for this frame (drives the readout +
        //      the blue/green canvas overlay). Cheap for small sketches. ----
        let (rep, defined) = crate::param_editor::analyze_doc(&self.doc, &self.parametric);
        self.parametric.defined = defined;
        self.parametric.dof = rep.dof;
        self.parametric.fully_defined = rep.fully_defined;
        self.parametric.redundant = rep.redundant;

        let lines = self.selected_linear_handles();
        let circles = self.selected_circle_handles();
        // resolve variables once for value display + field evaluation
        let var_env = self.parametric.vars.resolve();

        let mut to_add: Vec<CRef> = Vec::new();
        let mut arm: Option<(PendingKind, Handle)> = None;
        let mut cancel_pending = false;
        let mut remove_var: Option<String> = None;
        let mut remove_constraint: Option<usize> = None;
        let mut add_var = false;
        let (mut do_solve, mut clear_c, mut close) = (false, false, false);

        // ---- reference→target: if a constraint was armed with a first entity,
        //      and the user has now picked a different target, complete it. ----
        if let Some((kind, first)) = self.parametric.pending {
            let target = if kind.target_is_circle() {
                circles.iter().copied().find(|&h| h != first)
            } else if kind == PendingKind::Tangent {
                lines
                    .iter()
                    .chain(circles.iter())
                    .copied()
                    .find(|&h| h != first)
            } else {
                lines.iter().copied().find(|&h| h != first)
            };
            if let Some(t) = target {
                to_add.push(kind.to_cref(first, t));
                self.parametric.pending = None;
            }
        }

        let blue = egui::Color32::from_rgb(90, 160, 255);
        let green = egui::Color32::from_rgb(120, 220, 140);

        egui::Window::new("Parametric  ·  constraints")
            .id(egui::Id::new("param_panel"))
            .resizable(true)
            .default_width(300.0)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 64.0))
            .show(ctx, |ui| {
                // ---- DOF status banner ("fully defined" / "N DOF") ----
                if self.parametric.fully_defined {
                    ui.colored_label(green, egui::RichText::new("✔ Fully defined").strong());
                } else {
                    ui.colored_label(blue, egui::RichText::new(
                        format!("◇ Under-defined — {} DOF", self.parametric.dof)).strong());
                }
                if self.parametric.redundant {
                    ui.colored_label(egui::Color32::from_rgb(240, 180, 80),
                        "⚠ redundant / conflicting constraints");
                }
                // pending reference→target prompt
                if let Some((kind, _)) = self.parametric.pending {
                    ui.horizontal(|ui| {
                        ui.colored_label(egui::Color32::from_rgb(255, 210, 120),
                            format!("⏳ {} — now pick the target", kind.label()));
                        if ui.small_button("cancel").clicked() { cancel_pending = true; }
                    });
                }
                ui.checkbox(&mut self.parametric.show_dof, "colour geometry by DOF (blue=free, green=locked)");
                ui.checkbox(&mut self.parametric.keep_link,
                    "keep link live (move one → linked geometry follows)");
                ui.separator();
                ui.small("Draw with the normal tools (Line, Circle, snaps…), select \
                          geometry, then add constraints. Adding auto-solves.");

                egui::ScrollArea::vertical().max_height(440.0).id_salt("param_scroll").show(ui, |ui| {
                    ui.label(format!("selected: {} line/wall(s), {} circle(s)", lines.len(), circles.len()));

                    // ===== Per-entity math inspector =====
                    ui.horizontal(|ui| {
                        let hdr = if self.parametric.show_inspect { "🔍 Inspect selected ▼" } else { "🔍 Inspect selected ▶" };
                        if ui.selectable_label(false, hdr).clicked() {
                            self.parametric.show_inspect = !self.parametric.show_inspect;
                        }
                    });
                    if self.parametric.show_inspect {
                        let target = if lines.len() == 1 && circles.is_empty() {
                            Some(lines[0])
                        } else if circles.len() == 1 && lines.is_empty() {
                            Some(circles[0])
                        } else {
                            None
                        };
                        match target {
                            Some(h) => match crate::param_editor::inspect_handle(&self.doc, &self.parametric, h) {
                                Some(ls) => for ln in &ls { ui.small(egui::RichText::new(ln).monospace()); },
                                None => { ui.small("(selected entity is not a constrainable line/wall/circle)"); }
                            },
                            None => { ui.small("select exactly ONE line/wall or circle to inspect its math"); }
                        }
                    }

                    // ===== Geometric relations (lines & straight walls) =====
                    // Binary relations work two ways: select BOTH then click, OR
                    // select ONE, click, then pick the target ("reference→target").
                    ui.add_space(2.0);
                    ui.strong("Lines & walls");
                    ui.horizontal_wrapped(|ui| {
                        // Horizontal/Vertical apply to EVERY selected line.
                        if ui.add_enabled(!lines.is_empty(), egui::Button::new("Horizontal")).clicked() {
                            for &h in &lines { to_add.push(CRef::Horizontal(h)); }
                        }
                        if ui.add_enabled(!lines.is_empty(), egui::Button::new("Vertical")).clicked() {
                            for &h in &lines { to_add.push(CRef::Vertical(h)); }
                        }
                        // Binary relations: select 2 then click, OR select 1 then
                        // pick a target. Parallel/Collinear/Equal CHAIN across the
                        // whole selection (all become equal/parallel/collinear);
                        // Perpendicular is pairwise (>2 has no meaning in 2D).
                        for (kind, lbl, chain) in [
                            (PendingKind::Parallel, "Parallel", true),
                            (PendingKind::Perpendicular, "Perpendicular", false),
                            (PendingKind::Collinear, "Collinear", true),
                            (PendingKind::Equal, "Equal len", true),
                        ] {
                            if ui.add_enabled(!lines.is_empty(), egui::Button::new(lbl)).clicked() {
                                if lines.len() == 1 {
                                    arm = Some((kind, lines[0]));
                                } else if chain {
                                    for i in 1..lines.len() { to_add.push(kind.to_cref(lines[0], lines[i])); }
                                } else {
                                    to_add.push(kind.to_cref(lines[0], lines[1]));
                                }
                            }
                        }
                    });
                    // length dimension (driving) on one line
                    ui.horizontal(|ui| {
                        ui.label("Length");
                        ui.add(egui::TextEdit::singleline(&mut self.parametric.length_input)
                            .desired_width(70.0).hint_text("100 or =W/2"));
                        let ok = lines.len() == 1;
                        if ui.add_enabled(ok, egui::Button::new("set")).clicked() {
                            if let Ok(d) = self.parametric.eval_field(&self.parametric.length_input) {
                                to_add.push(CRef::Length(lines[0], d));
                            }
                        }
                    });
                    // angle dimension between two lines
                    ui.horizontal(|ui| {
                        ui.label("Angle°");
                        ui.add(egui::TextEdit::singleline(&mut self.parametric.angle_input)
                            .desired_width(70.0).hint_text("90 or =A"));
                        let ok = lines.len() == 2;
                        if ui.add_enabled(ok, egui::Button::new("set")).clicked() {
                            if let Ok(deg) = self.parametric.eval_field(&self.parametric.angle_input) {
                                to_add.push(CRef::Angle(lines[0], lines[1], deg.to_radians()));
                            }
                        }
                    });

                    // ===== Circles =====
                    ui.add_space(4.0);
                    ui.strong("Circles");
                    ui.horizontal_wrapped(|ui| {
                        // Concentric / Equal radius CHAIN across all selected circles.
                        for (kind, lbl) in [
                            (PendingKind::Concentric, "Concentric"),
                            (PendingKind::EqualRadius, "Equal radius"),
                        ] {
                            if ui.add_enabled(!circles.is_empty(), egui::Button::new(lbl)).clicked() {
                                if circles.len() == 1 {
                                    arm = Some((kind, circles[0]));
                                } else {
                                    for i in 1..circles.len() { to_add.push(kind.to_cref(circles[0], circles[i])); }
                                }
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Value");
                        ui.add(egui::TextEdit::singleline(&mut self.parametric.value_input)
                            .desired_width(70.0).hint_text("50 or =R"));
                        // Radius/Diameter apply to EVERY selected circle.
                        let ok = !circles.is_empty();
                        if ui.add_enabled(ok, egui::Button::new("Radius")).clicked() {
                            if let Ok(r) = self.parametric.eval_field(&self.parametric.value_input) {
                                for &h in &circles { to_add.push(CRef::Radius(h, r)); }
                            }
                        }
                        if ui.add_enabled(ok, egui::Button::new("Diameter")).clicked() {
                            if let Ok(d) = self.parametric.eval_field(&self.parametric.value_input) {
                                for &h in &circles { to_add.push(CRef::Radius(h, d * 0.5)); }
                            }
                        }
                    });

                    // ===== Tangent (line+circle or circle+circle) =====
                    ui.add_space(4.0);
                    ui.strong("Tangent");
                    let tan_lc = lines.len() == 1 && circles.len() == 1;
                    let tan_cc = lines.is_empty() && circles.len() == 2;
                    // also armable from a single entity (line OR circle) → pick target
                    let tan_one = (lines.len() == 1 && circles.is_empty())
                        || (circles.len() == 1 && lines.is_empty());
                    if ui.add_enabled(tan_lc || tan_cc || tan_one, egui::Button::new("Tangent")).clicked() {
                        if tan_lc {
                            to_add.push(CRef::Tangent(lines[0], circles[0]));
                        } else if tan_cc {
                            to_add.push(CRef::Tangent(circles[0], circles[1]));
                        } else if tan_one {
                            let first = if lines.len() == 1 { lines[0] } else { circles[0] };
                            arm = Some((PendingKind::Tangent, first));
                        }
                    }

                    // ===== Reference (driven) measurement =====
                    ui.add_space(4.0);
                    ui.strong("Reference (read-only)");
                    if lines.len() == 1 {
                        let ends = self.doc.dobjects.iter().find(|d| d.handle == lines[0])
                            .and_then(|d| match &d.geom {
                                Geom::Line(l) => Some((l.a, l.b)),
                                Geom::Wall(w) => Some((w.start, w.end)),
                                _ => None,
                            });
                        if let Some((a, b)) = ends {
                            ui.small(format!("length = ({:.4})", (b - a).len()));
                        }
                    } else if circles.len() == 1 {
                        if let Some(Geom::Circle(c)) =
                            self.doc.dobjects.iter().find(|d| d.handle == circles[0]).map(|d| &d.geom) {
                            ui.small(format!("radius = ({:.4})   diameter = ({:.4})", c.radius, c.radius * 2.0));
                        }
                    } else {
                        ui.small("select 1 line or 1 circle to measure");
                    }

                    // ===== Global variables / equations =====
                    ui.add_space(6.0);
                    ui.separator();
                    ui.strong("Global variables  (use as  =name  in fields)");
                    let mut idx_to_remove = None;
                    for (i, v) in self.parametric.vars.vars.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(70.0).hint_text("name"));
                            ui.label("=");
                            ui.add(egui::TextEdit::singleline(&mut v.expr).desired_width(90.0).hint_text("expr"));
                            match &var_env {
                                Ok(env) => match env.get(&v.name) {
                                    Some(val) => { ui.small(format!("= {val:.4}")); }
                                    None => { ui.small(""); }
                                },
                                Err(_) => { ui.colored_label(egui::Color32::from_rgb(240,120,120), "err"); }
                            }
                            if ui.small_button("✖").clicked() { idx_to_remove = Some(i); }
                        });
                    }
                    if let Some(i) = idx_to_remove {
                        if let Some(v) = self.parametric.vars.vars.get(i) { remove_var = Some(v.name.clone()); }
                    }
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.parametric.new_var_name)
                            .desired_width(70.0).hint_text("name"));
                        ui.label("=");
                        ui.add(egui::TextEdit::singleline(&mut self.parametric.new_var_expr)
                            .desired_width(90.0).hint_text("expr"));
                        if ui.button("＋ add").clicked() { add_var = true; }
                    });
                    if let Err(e) = &var_env {
                        ui.colored_label(egui::Color32::from_rgb(240,120,120), e);
                    }

                    // ===== constraint list =====
                    ui.add_space(6.0);
                    ui.separator();
                    ui.label(format!("constraints ({}) — only these are applied", self.parametric.constraints.len()));
                    for (i, c) in self.parametric.constraints.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("✖").clicked() { remove_constraint = Some(i); }
                            ui.small(format!("{}. {}", i + 1, c.label()));
                        });
                    }

                    // ===== Solver diagnostics (the math recorder) =====
                    ui.add_space(6.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        let hdr = if self.parametric.show_trace { "🔬 Solver diagnostics ▼" } else { "🔬 Solver diagnostics ▶" };
                        if ui.selectable_label(false, hdr).clicked() {
                            self.parametric.show_trace = !self.parametric.show_trace;
                        }
                        if !self.parametric.last_trace.lines.is_empty()
                            && ui.small_button("📋 copy log").clicked() {
                            ui.ctx().copy_text(self.parametric.last_trace.lines.join("\n"));
                            self.parametric.status = "solve log copied to clipboard".into();
                        }
                    });
                    if self.parametric.show_trace {
                        if self.parametric.last_trace.lines.is_empty() {
                            ui.small("(apply or re-solve a constraint to record a trace)");
                        } else {
                            for ln in &self.parametric.last_trace.lines {
                                ui.small(egui::RichText::new(ln).monospace());
                            }
                        }
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(egui::RichText::new("Solve").strong())).clicked() {
                        do_solve = true;
                    }
                    if ui.button("Clear constraints").clicked() { clear_c = true; }
                    if ui.button("Exit").clicked() { close = true; }
                });
                ui.label(egui::RichText::new(&self.parametric.status).weak());
            });

        // ---- arm / cancel a reference→target pick ----
        if cancel_pending {
            self.parametric.pending = None;
            self.parametric.status = "pick cancelled".into();
        }
        if let Some((kind, first)) = arm {
            self.parametric.pending = Some((kind, first));
            self.parametric.status = format!("⏳ {}: now select the target entity", kind.label());
            crate::dbg_event!(
                self,
                crate::dbg_recorder::DbgEvent::Note {
                    message: format!("param: armed {} (ref handle picked)", kind.label())
                }
            );
        }

        // ---- apply variable edits ----
        if add_var {
            let name = self.parametric.new_var_name.trim().to_string();
            if !name.is_empty() {
                let expr = self.parametric.new_var_expr.clone();
                self.parametric.vars.set(&name, &expr);
                self.parametric.new_var_name.clear();
                self.parametric.new_var_expr.clear();
            }
        }
        if let Some(name) = remove_var {
            self.parametric.vars.remove(&name);
        }

        // Removing a single constraint re-solves with the rest.
        if let Some(i) = remove_constraint {
            if i < self.parametric.constraints.len() {
                let lbl = self.parametric.constraints[i].label();
                self.parametric.constraints.remove(i);
                self.parametric.status = format!("removed: {lbl}");
                do_solve = true;
            }
        }

        // `just_added` tracks how many constraints THIS frame added — only those
        // get rolled back on non-convergence (a fresh constraint that conflicts).
        let mut just_added: usize = 0;
        let mut just_added_label: Option<String> = None;
        if !to_add.is_empty() {
            just_added = to_add.len();
            just_added_label = Some(if to_add.len() == 1 {
                to_add[0].label()
            } else {
                format!("{} ×{}", to_add[0].label(), to_add.len())
            });
            for c in to_add {
                crate::dbg_event!(
                    self,
                    crate::dbg_recorder::DbgEvent::Note {
                        message: format!(
                            "param: + {} (lines={}, circles={})",
                            c.label(),
                            lines.len(),
                            circles.len()
                        )
                    }
                );
                self.parametric.constraints.push(c);
            }
            self.parametric.pending = None; // the pair is complete
            do_solve = true; // auto-solve so the constraint takes effect immediately
        }
        if clear_c {
            self.parametric.constraints.clear();
            self.parametric.pending = None;
            self.parametric.status = "constraints cleared".into();
        }
        if do_solve {
            // keep a clean copy so a conflicting constraint can be rolled back
            // WITHOUT leaving the geometry half-solved ("half is gone").
            let before = self.doc.clone();
            self.snapshot_doc();
            let out = crate::param_editor::solve_doc(&mut self.doc, &self.parametric);
            let full_trace = out.trace.lines.join("\n");
            let converged = out.converged;
            let msg = out.msg;
            let trace = out.trace;

            if let (Some(lbl), false) = (just_added_label.as_ref(), converged) {
                if just_added > 0 {
                    // The just-added constraint(s) conflict / over-define the
                    // sketch. Restore geometry, drop them, and undo the snapshot
                    // (SolidWorks: "would over-define the sketch").
                    self.doc = before;
                    self.undo_stack.pop();
                    let keep = self.parametric.constraints.len().saturating_sub(just_added);
                    self.parametric.constraints.truncate(keep);
                }
                self.parametric.status =
                    format!("⚠ '{lbl}' conflicts / over-defines — NOT applied (use ✖ to free up constraints first)");
            } else {
                self.history.push(format!("  parametric: {}", msg));
                self.parametric.status = msg;
            }
            self.parametric.last_trace = trace;
            // Record the FULL math trace (geometry + sketch + residuals in/out)
            // into the session recorder so a Start/Stop dump captures everything.
            crate::dbg_event!(
                self,
                crate::dbg_recorder::DbgEvent::Note {
                    message: full_trace
                }
            );
            self.index_dirty = true;
            self.touch_view();
            // record the post-solve geometry so keep-link doesn't re-fire on it
            self.parametric.last_solved_sig = crate::param_editor::geom_signature(&self.doc);
        }
        if close {
            self.parametric.active = false;
            self.parametric.pending = None;
            self.parametric.status = "exited parametric mode".into();
        }
    }
}

impl CadApp {
    /// Handles of the currently-selected LINEAR dobjects — lines and straight
    /// wall centerlines, which the solver treats identically. (Curved walls have
    /// an arc centerline and aren't constrainable yet.)
    fn selected_linear_handles(&self) -> Vec<Handle> {
        self.selection
            .iter()
            .filter_map(|&i| {
                self.doc
                    .dobjects
                    .get(i)
                    .filter(|d| {
                        matches!(&d.geom, Geom::Line(_))
                            || matches!(&d.geom, Geom::Wall(w) if w.bulge.abs() < 1e-9)
                    })
                    .map(|d| d.handle)
            })
            .collect()
    }

    /// Handles of the currently-selected CIRCLE dobjects (for radius / concentric
    /// / tangent constraints).
    fn selected_circle_handles(&self) -> Vec<Handle> {
        self.selection
            .iter()
            .filter_map(|&i| {
                self.doc
                    .dobjects
                    .get(i)
                    .filter(|d| matches!(d.geom, Geom::Circle(_)))
                    .map(|d| d.handle)
            })
            .collect()
    }
}
