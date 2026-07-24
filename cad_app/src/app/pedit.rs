//! PEDIT (polyline edit) flow — `pedit`/`pe` command + JOIN.
//!
//! Child module of `app`, so these methods can read `CadApp`'s private fields
//! and call its private helpers (`idx_of_handle`, `snapshot_doc`, `set_prompt`,
//! `begin_selection`, the free `explode_polyline`, …) via `use super::*`. The
//! methods themselves are `pub(crate)` so the parent `app` module can invoke
//! them from `run_command`, the queued-op dispatch, and the Esc handler.
//!
//! Pure code-movement out of `app.rs` (2026-06-23) — no behaviour change.

use super::*;

impl CadApp {
    pub(crate) fn pedit_menu_prompt(&self, h: u64) -> String {
        let closed = self.idx_of_handle(h)
            .and_then(|i| self.doc.dobjects.get(i))
            .map(|d| matches!(&d.geom, Geom::Polyline(p) if p.closed))
            .unwrap_or(false);
        format!("pedit: [{} / Join / Width / Undo / eXit]",
            if closed { "Open" } else { "Close" })
    }

    /// `pedit` — start editing the selected polyline. A single selected Line or
    /// Arc is converted to a polyline first (AutoCAD behavior).
    pub(crate) fn pedit_start(&mut self) {
        // Resolve a single target from the selection.
        let target: Option<usize> = if self.selection.len() == 1 {
            Some(self.selection[0])
        } else if self.selection.is_empty() {
            self.selected
        } else {
            None
        };
        let Some(i) = target.filter(|&i| i < self.doc.dobjects.len()) else {
            // Nothing chosen yet — AutoCAD-style "Select polyline": open a
            // one-object selection session and re-run pedit on Enter. Keeps the
            // command ACTIVE (the green prompt stays up) instead of aborting to
            // a fresh "command:" line. Picking nothing + Enter is caught by the
            // queued-op "nothing selected" guard and cancels cleanly.
            if self.selection.len() > 1 {
                self.history.push("  ! pedit: select only ONE object".into());
            }
            self.begin_selection(SelectMode::ForSelect);
            self.queued_op = QueuedOp::PeditStart;
            self.set_prompt(
                "pedit: select ONE polyline / line / arc / ellipse-arc / spline, then Enter  [Esc=cancel]");
            self.refocus_cmd = true;
            return;
        };
        // Convert Line / Arc → polyline in place.
        let converted: Option<Geom> = match &self.doc.dobjects[i].geom {
            Geom::Polyline(_) => None,
            Geom::Line(l) => Some(Geom::Polyline(Polyline {
                vertices: vec![
                    PolyVertex { pos: l.a, bulge: 0.0 },
                    PolyVertex { pos: l.b, bulge: 0.0 }],
                closed: false,
                widths: Vec::new(),
            })),
            Geom::Arc(a) => {
                let s = Vec2::new(a.center.x + a.radius * a.start_angle.cos(),
                                  a.center.y + a.radius * a.start_angle.sin());
                let e_ang = a.start_angle + a.sweep_angle;
                let e = Vec2::new(a.center.x + a.radius * e_ang.cos(),
                                  a.center.y + a.radius * e_ang.sin());
                // Sign the bulge by the centre side (not just sweep) so the
                // converted polyline keeps the arc's true curve direction.
                let bulge = cad_kernel::bulge_from_arc(s, e, a.center, a.sweep_angle);
                Some(Geom::Polyline(Polyline {
                    vertices: vec![
                        PolyVertex { pos: s, bulge },
                        PolyVertex { pos: e, bulge: 0.0 }],
                    closed: false,
                    widths: Vec::new(),
                }))
            }
            // Elliptical arc / spline can't be a polyline exactly (bulges are
            // circular). Tessellate to a faceted polyline so PEDIT can edit /
            // join them — matches how pedit_join treats secondary picks.
            g @ (Geom::EllipseArc(_) | Geom::Spline(_)) =>
                tessellated_polyline(g).map(Geom::Polyline),
            _ => {
                self.history.push(
                    "  ! pedit: that object isn't a polyline / line / arc / ellipse-arc / spline".into());
                return;
            }
        };
        if let Some(g) = converted {
            self.snapshot_doc();
            self.doc.dobjects[i].geom = g;
            self.history.push("  pedit: converted to polyline".into());
            self.index_dirty = true;
            self.gpu_dirty = true;
        }
        let h = self.doc.dobjects[i].handle;
        self.pedit_state = PeditState::Menu(h);
        let p = self.pedit_menu_prompt(h);
        self.set_prompt(p);
        self.refocus_cmd = true;
    }

    pub(crate) fn pedit_set_closed(&mut self, h: u64, closed: bool) {
        let Some(i) = self.idx_of_handle(h) else { return; };
        if let Geom::Polyline(p) = &self.doc.dobjects[i].geom {
            if p.closed == closed {
                self.history.push(format!(
                    "  pedit: already {}", if closed { "closed" } else { "open" }));
                return;
            }
        } else { return; }
        self.snapshot_doc();
        if let Geom::Polyline(p) = &mut self.doc.dobjects[i].geom {
            p.closed = closed;
        }
        self.history.push(format!("  pedit: polyline {}",
            if closed { "closed" } else { "opened" }));
        self.index_dirty = true;
        self.gpu_dirty = true;
    }

    pub(crate) fn pedit_set_width(&mut self, h: u64, mm: f64) {
        let Some(i) = self.idx_of_handle(h) else { return; };
        self.snapshot_doc();
        self.doc.dobjects[i].style.lineweight =
            cad_kernel::Lineweight::Custom(mm as f32);
        self.history.push(format!("  pedit: width → {} mm", mm));
        self.gpu_dirty = true;
    }

    /// PEDIT Join — merge the user-PICKED objects (lines, arcs, open polylines,
    /// splines) into the target polyline. Splines are tessellated to polyline
    /// segments first. Called from the queued-op dispatch after the selection
    /// session ends. Re-enters the PEDIT menu on the merged result.
    pub(crate) fn pedit_join_selected(&mut self, h: u64) {
        let Some(ti) = self.idx_of_handle(h) else { self.pedit_exit(); return; };
        // Target + the picked objects (deduped).
        let mut idxs: Vec<usize> = vec![ti];
        for &i in &self.selection {
            if i != ti && i < self.doc.dobjects.len() { idxs.push(i); }
        }
        idxs.sort_unstable();
        idxs.dedup();
        if idxs.len() < 2 {
            self.history.push("  pedit join: pick at least one object to join".into());
            self.pedit_state = PeditState::Menu(h);
            self.pedit_reprompt(h);
            return;
        }
        let style = self.doc.dobjects[ti].style;
        // EXPLODE every input into chainable Line/Arc primitives, so the join
        // only ever sees true segments (join_geoms mishandles a whole polyline
        // as one straight segment, flattening interiors & arcs). Splines are
        // tessellated; polylines/rectangles become their Line/Arc segments.
        let mut prims: Vec<Geom> = Vec::new();
        for &i in &idxs {
            let Some(d) = self.doc.dobjects.get(i) else { continue; };
            match &d.geom {
                Geom::Polyline(p) => prims.extend(explode_polyline(p)),
                // Ellipse arc / spline → tessellated to straight segments
                // (they can't be circular-bulge segments). Same helper as
                // pedit_start so a target and a pick tessellate identically.
                g @ (Geom::EllipseArc(_) | Geom::Spline(_)) => {
                    if let Some(pl) = tessellated_polyline(g) {
                        prims.extend(explode_polyline(&pl));
                    }
                }
                Geom::Line(_) | Geom::Arc(_) => prims.push(d.geom.clone()),
                _ => {}   // non-chainable (full circle/ellipse, text, …) → skipped
            }
        }
        if prims.len() < 2 {
            self.history.push("  pedit join: nothing chainable was picked".into());
            self.pedit_state = PeditState::Menu(h);
            self.pedit_reprompt(h);
            return;
        }
        let items: Vec<(usize, Geom)> = prims.iter().cloned().enumerate().collect();
        let out = cad_kernel::join_geoms(&items);
        if out.merged.is_empty() {
            self.history.push(
                "  pedit join: picked objects don't connect end-to-end".into());
            // DIAGNOSTIC: print every exploded segment's true endpoints so we can
            // see whether (and by how much) the chain ends actually miss.
            for (k, g) in prims.iter().enumerate() {
                let ends = match g {
                    Geom::Line(l) => Some((l.a, l.b)),
                    Geom::Arc(ar) => {
                        let s = Vec2::new(
                            ar.center.x + ar.radius * ar.start_angle.cos(),
                            ar.center.y + ar.radius * ar.start_angle.sin());
                        let ea = ar.start_angle + ar.sweep_angle;
                        let e = Vec2::new(
                            ar.center.x + ar.radius * ea.cos(),
                            ar.center.y + ar.radius * ea.sin());
                        Some((s, e))
                    }
                    _ => None,
                };
                if let Some((a, b)) = ends {
                    let kind = match g {
                        Geom::Line(_) => "line",
                        Geom::Arc(_) => "arc ",
                        _ => "?   ",
                    };
                    self.history.push(format!(
                        "    seg[{}] {} ({:.4},{:.4}) → ({:.4},{:.4})",
                        k, kind, a.x, a.y, b.x, b.y));
                }
            }
            self.pedit_state = PeditState::Menu(h);
            self.pedit_reprompt(h);
            return;
        }
        // Result = merged piece(s) + any primitive that didn't chain (so the
        // original geometry is never lost).
        let consumed: std::collections::HashSet<usize> =
            out.consumed_indices.iter().copied().collect();
        let mut result: Vec<Geom> = out.merged.clone();
        for (k, g) in prims.iter().enumerate() {
            if !consumed.contains(&k) { result.push(g.clone()); }
        }
        // DIAGNOSTIC: dump source arcs and the resulting polyline bulges so we
        // can verify the stored curvature matches the original arc direction.
        for g in &prims {
            if let Geom::Arc(ar) = g {
                let (s, e) = ar.endpoints();
                self.history.push(format!(
                    "    src arc: c=({:.3},{:.3}) r={:.3} start={:.1}° sweep={:.1}° s=({:.3},{:.3}) e=({:.3},{:.3})",
                    ar.center.x, ar.center.y, ar.radius,
                    ar.start_angle.to_degrees(), ar.sweep_angle.to_degrees(),
                    s.x, s.y, e.x, e.y));
            }
        }
        for g in &out.merged {
            if let Geom::Polyline(pl) = g {
                for (vi, v) in pl.vertices.iter().enumerate() {
                    self.history.push(format!(
                        "    pl v[{}] ({:.3},{:.3}) bulge={:.4}",
                        vi, v.pos.x, v.pos.y, v.bulge));
                }
            }
        }
        self.snapshot_doc();
        // Remove the originals (target + picked), descending.
        let mut rem = idxs.clone();
        rem.sort_unstable_by(|a, b| b.cmp(a));
        for i in rem {
            if i < self.doc.dobjects.len() { self.doc.dobjects.remove(i); }
        }
        let mut new_handle = None;
        for g in result {
            let is_poly = matches!(g, Geom::Polyline(_));
            let d = DObject::with_style(g, style);
            let nh = d.handle;
            self.doc.push(d);
            if is_poly && new_handle.is_none() { new_handle = Some(nh); }
        }
        self.selection.clear();
        self.index_dirty = true;
        self.gpu_dirty = true;
        self.history.push("  pedit join: merged into polyline".into());
        let nh = new_handle.unwrap_or(h);
        self.pedit_state = PeditState::Menu(nh);
        self.pedit_reprompt(nh);
    }

    pub(crate) fn pedit_exit(&mut self) {
        if self.pedit_state != PeditState::Off {
            self.pedit_state = PeditState::Off;
            self.clear_prompt();
            self.history.push("  pedit: done".into());
        }
    }

    /// Typed input while a PEDIT flow is active.
    pub(crate) fn pedit_input(&mut self, raw: &str) {
        let s = raw.trim().to_ascii_lowercase();
        match self.pedit_state {
            PeditState::Off => {}
            PeditState::Menu(h) => {
                match s.as_str() {
                    "c" | "close" => { self.pedit_set_closed(h, true); self.pedit_reprompt(h); }
                    "o" | "open"  => { self.pedit_set_closed(h, false); self.pedit_reprompt(h); }
                    "j" | "join"  => {
                        // Interactive: open a selection session to pick the
                        // objects to join; the queued PeditJoin fires on Enter.
                        self.pedit_state = PeditState::Off;
                        self.begin_selection(SelectMode::ForSelect);
                        self.queued_op = QueuedOp::PeditJoin(h);
                        self.set_prompt(
                            "pedit join: select lines/arcs/plines/splines, Enter to join  [Esc=cancel]");
                        self.refocus_cmd = true;
                    }
                    "w" | "width" => {
                        self.pedit_state = PeditState::Width(h);
                        self.set_prompt("pedit width: enter width in mm  [Esc=cancel]");
                        self.refocus_cmd = true;
                    }
                    "u" | "undo"  => { self.do_undo(); self.pedit_reprompt(h); }
                    "x" | "exit" | "" => self.pedit_exit(),
                    _ => self.history.push(
                        "  ! pedit: type C/O, J, W, U, or X".into()),
                }
            }
            PeditState::Width(h) => {
                if s.is_empty() { self.pedit_state = PeditState::Menu(h); self.pedit_reprompt(h); }
                else if let Ok(mm) = s.parse::<f64>() {
                    if mm < 0.0 {
                        self.history.push("  ! pedit width: must be ≥ 0".into());
                    } else {
                        self.pedit_set_width(h, mm);
                    }
                    self.pedit_state = PeditState::Menu(h);
                    self.pedit_reprompt(h);
                } else {
                    self.history.push("  ! pedit width: enter a number (mm)".into());
                }
            }
        }
    }

    pub(crate) fn pedit_reprompt(&mut self, h: u64) {
        if self.idx_of_handle(h).is_none() {
            // Target vanished (e.g. undo removed it) — leave pedit.
            self.pedit_exit();
            return;
        }
        let p = self.pedit_menu_prompt(h);
        self.set_prompt(p);
        self.refocus_cmd = true;
    }
}

/// Tessellate an elliptical arc or spline into a straight-segment (bulge-0)
/// polyline so it can become — or join into — a PEDIT polyline. Circular arcs
/// are NOT handled here (they keep their exact bulge). Returns `None` for any
/// other geom. Used by both `pedit_start` (target) and `pedit_join_selected`
/// (pick) so the same curve tessellates identically either way.
///
/// EllipseArc density scales with the swept fraction (~96 segments for a full
/// ellipse, min 12); splines use the kernel's 64-sample tessellation. The
/// result APPROXIMATES the curve (faceted at very high zoom) because a polyline
/// can only encode straight + circular-arc spans.
fn tessellated_polyline(g: &Geom) -> Option<Polyline> {
    match g {
        Geom::EllipseArc(ea) => {
            let frac = (ea.sweep_param.abs()
                / std::f64::consts::TAU).clamp(0.0, 1.0);
            let n = (96.0 * frac).ceil().max(12.0) as usize;
            let vertices = (0..=n).map(|i| {
                let t = ea.start_param + ea.sweep_param * (i as f64 / n as f64);
                PolyVertex { pos: ea.ellipse.point_at(t), bulge: 0.0 }
            }).collect();
            Some(Polyline { vertices, closed: false, widths: Vec::new() })
        }
        Geom::Spline(s) => Some(Polyline {
            vertices: s.tessellate(64).into_iter()
                .map(|p| PolyVertex { pos: p, bulge: 0.0 }).collect(),
            closed: false,
            widths: Vec::new(),
        }),
        _ => None,
    }
}
