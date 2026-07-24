//! trim.rs — TRIM command and trim-survivor re-joining.
//!
//! Split out of `geom.rs` verbatim (pure code-movement refactor). Contains
//! `Geom::trim_at`, the `join_trim_survivors` helper that re-merges the
//! touching fragments a trim leaves on a closed curve, and its private
//! support fns `same_ellipse` / `circular_union`.

use crate::math::{Vec2, EPS};
use crate::geom::{Arc, Circle, Ellipse, EllipseArc, Geom, Line, PolyVertex, Polyline, Wall};
use crate::join::{bulge_from_arc, polyline_segments, JOIN_EPS};

/// Trim an OPEN, width-carrying polyline while keeping CONNECTED runs (so
/// mitred corners and per-segment widths survive). The clicked segment is
/// trimmed; the polyline splits into a "before" run (vertices up to the clicked
/// segment + its surviving start piece) and an "after" run (its surviving end
/// piece + the remaining vertices). Either run with ≥ 2 vertices is emitted.
fn trim_polyline_connected(
    p: &Polyline,
    segs: &[Geom],
    best_i: usize,
    cutters: &[Geom],
    pick: Vec2,
    edge_mode: bool,
) -> Vec<Geom> {
    let n = p.vertices.len();
    let a_pt = p.vertices[best_i].pos;        // start of clicked segment
    let b_pt = p.vertices[best_i + 1].pos;    // end of clicked segment
    let w_clicked = p.widths.get(best_i).copied().unwrap_or((0.0, 0.0));
    // Intersects a cutter → trim normally. No intersection → REMOVE the whole
    // clicked segment (empty pieces): the polyline splits into a "before" run
    // ending at v[best_i] and an "after" run starting at v[best_i+1].
    let pieces = match segs[best_i].trim_at(cutters, pick, edge_mode) {
        Ok(ps) => ps,
        Err(_) => Vec::new(),
    };
    let near = |x: Vec2, y: Vec2| (x - y).len() < 1e-6;
    let ep = |g: &Geom| -> (Vec2, Vec2) {
        match g {
            Geom::Line(l) => (l.a, l.b),
            Geom::Arc(ar) => ar.endpoints(),
            _ => (Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0)),
        }
    };
    let bulge_of = |g: &Geom, from: Vec2, to: Vec2| match g {
        Geom::Arc(ar) => bulge_from_arc(from, to, ar.center, ar.sweep_angle),
        _ => 0.0,
    };
    // Classify the surviving pieces of the clicked segment by which original
    // endpoint they still touch.
    let mut start_piece: Option<&Geom> = None;   // touches a_pt
    let mut end_piece: Option<&Geom> = None;      // touches b_pt
    for pc in &pieces {
        let (pa, pb) = ep(pc);
        if near(pa, a_pt) || near(pb, a_pt) { start_piece = Some(pc); }
        else if near(pa, b_pt) || near(pb, b_pt) { end_piece = Some(pc); }
    }
    // Non-width polylines keep EMPTY widths in their runs (so they stay plain).
    let has_w = !p.widths.is_empty();
    let mut out: Vec<Geom> = Vec::new();
    // --- before run: v[0..=best_i] (+ start piece's far end) ---
    {
        let mut vb: Vec<PolyVertex> = (0..=best_i).map(|i| p.vertices[i]).collect();
        let mut wb: Vec<(f64, f64)> =
            (0..best_i).map(|i| p.widths.get(i).copied().unwrap_or((0.0, 0.0))).collect();
        if let Some(pc) = start_piece {
            let (pa, pb) = ep(pc);
            let far = if near(pa, a_pt) { pb } else { pa };
            let bl = bulge_of(pc, a_pt, far);
            if let Some(last) = vb.last_mut() { last.bulge = bl; }
            vb.push(PolyVertex { pos: far, bulge: 0.0 });
            wb.push(w_clicked);
        }
        if vb.len() >= 2 {
            out.push(Geom::Polyline(Polyline {
                vertices: vb, closed: false,
                widths: if has_w { wb } else { Vec::new() },
            }));
        }
    }
    // --- after run: (end piece's cut end +) v[best_i+1..] ---
    {
        let mut va: Vec<PolyVertex> = Vec::new();
        let mut wa: Vec<(f64, f64)> = Vec::new();
        if let Some(pc) = end_piece {
            let (pa, pb) = ep(pc);
            let cut = if near(pa, b_pt) { pb } else { pa };   // the new free start
            let bl = bulge_of(pc, cut, b_pt);
            va.push(PolyVertex { pos: cut, bulge: bl });
            wa.push(w_clicked);
        }
        for i in (best_i + 1)..n {
            va.push(p.vertices[i]);
            if i < n - 1 { wa.push(p.widths.get(i).copied().unwrap_or((0.0, 0.0))); }
        }
        if va.len() >= 2 {
            out.push(Geom::Polyline(Polyline {
                vertices: va, closed: false,
                widths: if has_w { wa } else { Vec::new() },
            }));
        }
    }
    // Fallback: nothing chained into a run (e.g. a 2-vertex polyline) — keep the
    // surviving pieces individually so width isn't lost.
    if out.is_empty() {
        for pc in pieces { out.push(wrap_with_width(pc, w_clicked)); }
    }
    out
}

/// Wrap a single trimmed Line/Arc segment back into a 1-segment Polyline that
/// carries the segment's `(start,end)` width — so trimming a WIDE polyline
/// keeps its width (bare Line/Arc have no width field). Other geoms pass
/// through unchanged.
fn wrap_with_width(g: Geom, w: (f64, f64)) -> Geom {
    match g {
        Geom::Line(l) => Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: l.a, bulge: 0.0 },
                PolyVertex { pos: l.b, bulge: 0.0 }],
            closed: false,
            widths: vec![w],
        }),
        Geom::Arc(a) => {
            let (s, e) = a.endpoints();
            let bulge = bulge_from_arc(s, e, a.center, a.sweep_angle);
            Geom::Polyline(Polyline {
                vertices: vec![
                    PolyVertex { pos: s, bulge },
                    PolyVertex { pos: e, bulge: 0.0 }],
                closed: false,
                widths: vec![w],
            })
        }
        other => other,
    }
}

impl Geom {
    /// Trim this geometry by the given cutting edges.
    ///
    /// **Semantics (matches AutoCAD's TRIM):** the target is broken at
    /// EVERY intersection with the cutters into `N+1` separate segments;
    /// the segment containing the click is REMOVED; every other segment
    /// is returned as its OWN piece. The caller wraps each piece in a
    /// fresh `DObject` with the target's preserved style.
    ///
    /// Returns `Vec<Geom>` of the surviving sub-segments. For a target
    /// with N cuts, the user clicks one segment; you get back exactly N
    /// surviving pieces.
    ///
    /// `edge_mode` ON treats cutters as their infinite extensions for
    /// the intersection step (see `extended_for_edgemode`).
    ///
    /// Supported targets in v1: Line, Arc, EllipseArc. Other variants
    /// return an `Err` so the caller can leave them untouched.
    pub fn trim_at(
        &self,
        cutters: &[Geom],
        pick: Vec2,
        edge_mode: bool,
    ) -> Result<Vec<Geom>, &'static str> {
        use crate::intersect::intersect;

        // Gather intersection points with every cutter.
        let mut hits: Vec<Vec2> = Vec::new();
        for c in cutters {
            let c_eff = if edge_mode { c.extended_for_edgemode() } else { c.clone() };
            hits.extend(intersect(self, &c_eff));
        }
        // A POLYLINE is handled per-segment below: a clicked segment that meets
        // no cutter is REMOVED, so an all-miss polyline is still valid (it just
        // deletes that segment). Only non-polyline targets need a real hit.
        if hits.is_empty() && !matches!(self, Geom::Polyline(_)) {
            return Err("trim: target has no intersection with the cutting edges");
        }

        /// AutoCAD-correct TRIM survivors. `bounds` = sorted parameters
        /// [target_start, …intersection_ts…, target_end]. The clicked
        /// interval is the one containing `pick_t`; that interval is the
        /// only thing removed. Everything to the LEFT of the clicked
        /// interval stays as ONE continuous piece (target_start → left
        /// boundary of clicked interval), and everything to the RIGHT
        /// stays as ONE continuous piece (right boundary → target_end).
        /// Cutter intersections that lie OUTSIDE the clicked interval do
        /// NOT cause splits — the line passes through them uninterrupted.
        ///
        /// Net survivors: 0, 1, or 2 pieces.
        ///   0 → clicked interval spans the whole target (degenerate cut).
        ///   1 → click is in the FIRST or LAST interval; the other end
        ///       survives as one continuous piece (typical case for a
        ///       line crossing a closed cutter from outside).
        ///   2 → click is in a MIDDLE interval; removing it disconnects
        ///       the target into two pieces.
        ///
        /// Earlier the algorithm kept every non-clicked interval as its
        /// own piece (over-split — the trim docs in
        /// `feedback_rust_cad_trim_breaks_into_all_segments` reflect the
        /// old rule). Confirmed bug 2026-06-08: trimming a line outside
        /// an ellipse produced 2 separate dobjects (Inside + Outside-B)
        /// when only Outside-A should have been removed.
        fn surviving_segments(bounds: &[f64], pick_t: f64, eps: f64) -> Vec<(f64, f64)> {
            let n = bounds.len();
            if n < 2 { return Vec::new(); }
            // Find the interval containing `pick_t`.
            let mut clicked: Option<(f64, f64)> = None;
            for i in 0..n - 1 {
                let t1 = bounds[i];
                let t2 = bounds[i + 1];
                if (t2 - t1) <= eps { continue; }   // skip empty intervals
                if pick_t >= t1 - eps && pick_t <= t2 + eps {
                    clicked = Some((t1, t2));
                    break;
                }
            }
            let Some((left, right)) = clicked else {
                // Pick fell outside all intervals (shouldn't happen for
                // clamped pick_t) — defensively keep the whole target.
                return vec![(bounds[0], bounds[n - 1])];
            };
            let mut out = Vec::new();
            // Left survivor: target_start → left edge of clicked interval.
            if left - bounds[0] > eps {
                out.push((bounds[0], left));
            }
            // Right survivor: right edge of clicked interval → target_end.
            if bounds[n - 1] - right > eps {
                out.push((right, bounds[n - 1]));
            }
            out
        }

        match self {
            Geom::Line(l) => {
                let d = l.b - l.a;
                let len_sq = d.len_sq();
                if len_sq < EPS { return Err("trim: zero-length line"); }
                let to_t = |p: Vec2| -> f64 { (p - l.a).dot(d) / len_sq };
                let pick_t = to_t(pick).clamp(0.0, 1.0);
                let mut params: Vec<f64> = hits.iter().map(|&p| to_t(p))
                    .filter(|&t| t > 1e-9 && t < 1.0 - 1e-9).collect();
                params.sort_by(|a, b| a.partial_cmp(b).unwrap());
                params.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
                // Endpoint-only hits → this is a stray fragment between two
                // cutters; the user wants it removed entirely. See memo
                // `feedback_rust_cad_trim_fragment_endpoint_only_deletes`.
                if params.is_empty() {
                    return Ok(Vec::new());
                }
                let mut bounds = vec![0.0_f64];
                bounds.extend(&params);
                bounds.push(1.0);
                Ok(surviving_segments(&bounds, pick_t, 1e-9).into_iter()
                    .map(|(t1, t2)| Geom::Line(Line {
                        a: l.a + d * t1,
                        b: l.a + d * t2,
                    })).collect())
            }
            Geom::Arc(arc) => {
                if arc.radius < EPS { return Err("trim: zero-radius arc"); }
                let to_local = |p: Vec2| -> f64 {
                    ((p - arc.center).angle() - arc.start_angle)
                        .rem_euclid(std::f64::consts::TAU)
                };
                let pick_t = to_local(pick).clamp(0.0, arc.sweep_angle);
                let mut params: Vec<f64> = hits.iter().map(|&p| to_local(p))
                    .filter(|&t| t > EPS && t < arc.sweep_angle - EPS).collect();
                params.sort_by(|a, b| a.partial_cmp(b).unwrap());
                params.dedup_by(|a, b| (*a - *b).abs() < EPS);
                if params.is_empty() {
                    return Ok(Vec::new());
                }
                let mut bounds = vec![0.0_f64];
                bounds.extend(&params);
                bounds.push(arc.sweep_angle);
                Ok(surviving_segments(&bounds, pick_t, EPS).into_iter()
                    .map(|(t1, t2)| Geom::Arc(Arc {
                        center: arc.center,
                        radius: arc.radius,
                        start_angle: (arc.start_angle + t1).rem_euclid(std::f64::consts::TAU),
                        sweep_angle: t2 - t1,
                    })).collect())
            }
            Geom::EllipseArc(ea) => {
                let to_local = |p: Vec2| -> f64 {
                    (ea.ellipse.nearest_param(p) - ea.start_param)
                        .rem_euclid(std::f64::consts::TAU)
                };
                let pick_t = to_local(pick).clamp(0.0, ea.sweep_param);
                let mut params: Vec<f64> = hits.iter().map(|&p| to_local(p))
                    .filter(|&t| t > EPS && t < ea.sweep_param - EPS).collect();
                params.sort_by(|a, b| a.partial_cmp(b).unwrap());
                params.dedup_by(|a, b| (*a - *b).abs() < EPS);
                if params.is_empty() {
                    return Ok(Vec::new());
                }
                let mut bounds = vec![0.0_f64];
                bounds.extend(&params);
                bounds.push(ea.sweep_param);
                Ok(surviving_segments(&bounds, pick_t, EPS).into_iter()
                    .map(|(t1, t2)| Geom::EllipseArc(EllipseArc {
                        ellipse: ea.ellipse,
                        start_param: (ea.start_param + t1).rem_euclid(std::f64::consts::TAU),
                        sweep_param: t2 - t1,
                    })).collect())
            }
            Geom::Circle(c) => {
                // Closed loop: 2+ cuts break it into N arcs.
                // Find all intersection angles (relative to angle 0); sort;
                // build segments; drop the one containing pick_angle.
                if c.radius < EPS { return Err("trim: zero-radius circle"); }
                let to_ang = |p: Vec2| (p - c.center).angle().rem_euclid(std::f64::consts::TAU);
                let pick_t = to_ang(pick);
                let mut params: Vec<f64> = hits.iter().map(|&p| to_ang(p)).collect();
                params.sort_by(|a, b| a.partial_cmp(b).unwrap());
                params.dedup_by(|a, b| (*a - *b).abs() < EPS);
                if params.len() < 2 {
                    return Err("trim: circle needs at least 2 intersections to break");
                }
                // Wrap segments end-to-end around the circle.
                let mut out = Vec::new();
                let n = params.len();
                for i in 0..n {
                    let t1 = params[i];
                    let t2 = params[(i + 1) % n];
                    let sweep = (t2 - t1).rem_euclid(std::f64::consts::TAU);
                    // Pick-angle in this arc iff (t1 → pick_t → t2) in CCW order.
                    let pick_offset = (pick_t - t1).rem_euclid(std::f64::consts::TAU);
                    let click_inside = pick_offset > EPS && pick_offset < sweep - EPS;
                    if click_inside { continue; }
                    out.push(Geom::Arc(Arc {
                        center: c.center, radius: c.radius,
                        start_angle: t1, sweep_angle: sweep,
                    }));
                }
                Ok(out)
            }
            Geom::Ellipse(el) => {
                // Closed loop, same shape as the Circle case but in ellipse
                // parameter space. Each intersection point maps to its t via
                // `nearest_param` (exact for points on the curve).
                if el.semi_major() < EPS {
                    return Err("trim: degenerate ellipse");
                }
                let to_t = |p: Vec2| el.nearest_param(p).rem_euclid(std::f64::consts::TAU);
                let pick_t = to_t(pick);
                let mut params: Vec<f64> = hits.iter().map(|&p| to_t(p)).collect();
                params.sort_by(|a, b| a.partial_cmp(b).unwrap());
                params.dedup_by(|a, b| (*a - *b).abs() < EPS);
                if params.len() < 2 {
                    return Err("trim: ellipse needs at least 2 intersections to break");
                }
                let mut out = Vec::new();
                let n = params.len();
                for i in 0..n {
                    let t1 = params[i];
                    let t2 = params[(i + 1) % n];
                    let sweep = (t2 - t1).rem_euclid(std::f64::consts::TAU);
                    let pick_offset = (pick_t - t1).rem_euclid(std::f64::consts::TAU);
                    let click_inside = pick_offset > EPS && pick_offset < sweep - EPS;
                    if click_inside { continue; }
                    out.push(Geom::EllipseArc(EllipseArc {
                        ellipse:     *el,
                        start_param: t1,
                        sweep_param: sweep,
                    }));
                }
                Ok(out)
            }
            Geom::Polyline(p) => {
                // v1 semantic: EXPLODE the polyline into independent Line
                // / Arc segments, trim the one nearest the click, leave
                // every other segment intact. The polyline structure
                // dissolves — user can `join` them back if needed.
                let segs = polyline_segments(p);
                if segs.is_empty() {
                    return Err("trim: polyline has no segments");
                }
                // Nearest-segment-to-pick.
                let mut best_i = 0usize;
                let mut best_d = f64::INFINITY;
                for (i, s) in segs.iter().enumerate() {
                    let d = s.distance_to_point(pick);
                    if d < best_d { best_d = d; best_i = i; }
                }
                let has_w = !p.widths.is_empty();
                // OPEN polyline: keep CONNECTED runs so the rest stays a single
                // polyline (mitred corners + widths preserved). Trimming the
                // clicked segment splits it into a "before" run and an "after"
                // run at the cut; a segment that meets no cutter is removed.
                if !p.closed {
                    return Ok(trim_polyline_connected(p, &segs, best_i, cutters, pick, edge_mode));
                }
                // CLOSED polyline: EXPLODE into independent Line/Arc segments (v1).
                let mut out = Vec::new();
                for (i, s) in segs.into_iter().enumerate() {
                    let w = p.widths.get(i).copied().unwrap_or((0.0, 0.0));
                    if i == best_i {
                        match s.trim_at(cutters, pick, edge_mode) {
                            // Intersects a cutter → normal trim (keep the pieces).
                            Ok(pieces) => {
                                for piece in pieces {
                                    out.push(if has_w { wrap_with_width(piece, w) } else { piece });
                                }
                            }
                            // No intersection with any boundary → REMOVE the
                            // whole clicked segment (push nothing).
                            Err(_) => {}
                        }
                    } else {
                        out.push(if has_w { wrap_with_width(s, w) } else { s });
                    }
                }
                Ok(out)
            }
            Geom::Point(_) =>
                Err("trim: Point has nothing to trim"),
            Geom::Hatch(_) =>
                Err("trim: hatch entities cannot be trimmed"),
            Geom::Spline(_) =>
                Err("trim: spline entities cannot be trimmed in v1 (knot insertion + split + reparametrise pending)"),
            Geom::Wall(w) => {
                // Trim the centerline; wrap each surviving sub-segment
                // as a new Wall with the same thickness. Side lines
                // re-derive on render.
                let line = Geom::Line(w.centerline());
                let pieces = line.trim_at(cutters, pick, edge_mode)?;
                Ok(pieces.into_iter().filter_map(|g| {
                    if let Geom::Line(seg) = g {
                        Some(Geom::Wall(Wall {
                            start: seg.a, end: seg.b, thickness: w.thickness,
                            style: w.style, bulge: 0.0,
                        }))
                    } else { None }
                }).collect())
            }
            Geom::Text(_) =>
                Err("trim: text entities have no curve to cut"),
            Geom::Dimension(_) =>
                Err("trim: dimensions have no curve to cut"),
            Geom::BlockRef(_) =>
                Err("trim: explode the block first"),
        }
    }

    /// Extend this geometry toward the nearest boundary intersection on the
    /// side indicated by `pick`. Symmetric to `trim_at`. Supported targets
    /// in v1: Line and Arc (extend at whichever endpoint the click is closer to).
    pub fn extend_to(
        &self,
        boundaries: &[Geom],
        pick: Vec2,
        edge_mode: bool,
    ) -> Result<Geom, &'static str> {
        use crate::intersect::intersect;
        // Polyline: extend the END SEGMENT nearest the pick toward the boundary
        // and move that free endpoint — handled here BEFORE the whole-target
        // intersection test (a polyline doesn't itself reach the boundary).
        if let Geom::Polyline(p) = self {
            let n = p.vertices.len();
            if n < 2 { return Err("extend: polyline has no segments"); }
            let segs = polyline_segments(p);
            if segs.is_empty() { return Err("extend: polyline has no segments"); }
            let start_pt = p.vertices[0].pos;
            let end_pt = p.vertices[n - 1].pos;
            let at_end = pick.dist(end_pt) <= pick.dist(start_pt);
            let seg_i = if at_end { segs.len() - 1 } else { 0 };
            // The vertex that STAYS put (the inner end of the extended segment).
            let fixed_pt = if at_end { p.vertices[n - 2].pos } else { p.vertices[1].pos };
            let extended = segs[seg_i].extend_to(boundaries, pick, edge_mode)?;
            let (ea, eb) = match &extended {
                Geom::Line(l) => (l.a, l.b),
                Geom::Arc(a)  => a.endpoints(),
                _ => return Err("extend: unsupported polyline end segment"),
            };
            // The free end is whichever extended endpoint is farther from the
            // fixed vertex.
            let new_free = if ea.dist(fixed_pt) > eb.dist(fixed_pt) { ea } else { eb };
            let mut verts = p.vertices.clone();
            let free_idx  = if at_end { n - 1 } else { 0 };
            let bulge_idx = if at_end { n - 2 } else { 0 };
            verts[free_idx].pos = new_free;
            // Recompute the affected segment's bulge if it's an arc.
            verts[bulge_idx].bulge = match &extended {
                Geom::Arc(a) => bulge_from_arc(
                    verts[bulge_idx].pos, verts[bulge_idx + 1].pos, a.center, a.sweep_angle),
                _ => 0.0,
            };
            return Ok(Geom::Polyline(Polyline {
                vertices: verts, closed: p.closed, widths: p.widths.clone(),
            }));
        }
        // Build intersections of the target's INFINITE form with each
        // (possibly extended) boundary — extension is the whole point.
        let target_infinite = self.extended_for_edgemode();
        let mut hits: Vec<Vec2> = Vec::new();
        for b in boundaries {
            let b_eff = if edge_mode { b.extended_for_edgemode() } else { b.clone() };
            hits.extend(intersect(&target_infinite, &b_eff));
        }
        if hits.is_empty() {
            return Err("extend: target has no intersection with the boundary");
        }
        match self {
            Geom::Line(l) => {
                let d = l.b - l.a;
                let len_sq = d.len_sq();
                if len_sq < EPS { return Err("extend: zero-length line"); }
                let to_t = |p: Vec2| -> f64 { (p - l.a).dot(d) / len_sq };
                let at_b = pick.dist(l.b) < pick.dist(l.a);
                if at_b {
                    // Extend forward: smallest t > 1
                    let candidate = hits.iter().map(|&p| to_t(p))
                        .filter(|&t| t > 1.0 + EPS).fold(f64::INFINITY, f64::min);
                    if candidate.is_infinite() {
                        return Err("extend: no boundary intersection past the end of the line");
                    }
                    Ok(Geom::Line(Line { a: l.a, b: l.a + d * candidate }))
                } else {
                    // Extend backward: largest t < 0
                    let candidate = hits.iter().map(|&p| to_t(p))
                        .filter(|&t| t < -EPS).fold(f64::NEG_INFINITY, f64::max);
                    if candidate.is_infinite() {
                        return Err("extend: no boundary intersection before the start of the line");
                    }
                    Ok(Geom::Line(Line { a: l.a + d * candidate, b: l.b }))
                }
            }
            Geom::Arc(arc) => {
                if arc.radius < EPS { return Err("extend: zero-radius arc"); }
                let to_local = |p: Vec2| -> f64 {
                    ((p - arc.center).angle() - arc.start_angle)
                        .rem_euclid(std::f64::consts::TAU)
                };
                let (e1, e2) = arc.endpoints();
                let at_end = pick.dist(e2) < pick.dist(e1);
                if at_end {
                    // Extend sweep: smallest t > sweep_angle
                    let candidate = hits.iter().map(|&p| to_local(p))
                        .filter(|&t| t > arc.sweep_angle + EPS).fold(f64::INFINITY, f64::min);
                    if candidate.is_infinite() || candidate >= std::f64::consts::TAU {
                        return Err("extend: no boundary intersection past the arc end");
                    }
                    Ok(Geom::Arc(Arc {
                        center: arc.center, radius: arc.radius,
                        start_angle: arc.start_angle, sweep_angle: candidate,
                    }))
                } else {
                    // Extend start backward: largest t < 0 (or equivalently t > sweep going CCW past TAU)
                    let candidate = hits.iter().map(|&p| {
                        let raw = to_local(p);
                        if raw > arc.sweep_angle + EPS { raw - std::f64::consts::TAU } else { raw }
                    }).filter(|&t| t < -EPS).fold(f64::NEG_INFINITY, f64::max);
                    if candidate.is_infinite() {
                        return Err("extend: no boundary intersection before the arc start");
                    }
                    let new_start = (arc.start_angle + candidate)
                        .rem_euclid(std::f64::consts::TAU);
                    Ok(Geom::Arc(Arc {
                        center: arc.center, radius: arc.radius,
                        start_angle: new_start,
                        sweep_angle: arc.sweep_angle - candidate,
                    }))
                }
            }
            Geom::Wall(w) => {
                let line = Geom::Line(w.centerline());
                let g = line.extend_to(boundaries, pick, edge_mode)?;
                if let Geom::Line(new_line) = g {
                    Ok(Geom::Wall(Wall {
                        start: new_line.a, end: new_line.b,
                        thickness: w.thickness,
                        style: w.style, bulge: 0.0,
                    }))
                } else { Err("extend wall: unexpected non-Line result") }
            }
            _ => Err("extend: only Line / Arc / Wall are supported in v1"),
        }
    }

    /// Split into two pieces at the projection of `at` onto the curve.
    /// Both pieces inherit nothing from style — the caller wraps them in
    /// DObjects with the original's style.
    /// Returns Err for Circle (single click can't define which side to keep)
    /// and Point (nothing to split). Closed polylines split into two open
    /// polylines.
    pub fn split_at(&self, at: Vec2) -> Result<(Geom, Geom), &'static str> {
        match self {
            Geom::Line(l) => {
                let d = l.b - l.a;
                let len_sq = d.len_sq();
                if len_sq < EPS { return Err("split: zero-length line"); }
                let t = ((at - l.a).dot(d) / len_sq).clamp(EPS, 1.0 - EPS);
                let mid = l.a + d * t;
                Ok((Geom::Line(Line { a: l.a, b: mid }),
                    Geom::Line(Line { a: mid, b: l.b })))
            }
            Geom::Arc(a) => {
                if a.radius < EPS { return Err("split: zero-radius arc"); }
                let ang = ((at - a.center).angle() - a.start_angle)
                    .rem_euclid(std::f64::consts::TAU);
                let split = ang.clamp(EPS, a.sweep_angle - EPS);
                Ok((Geom::Arc(Arc {
                    center: a.center, radius: a.radius,
                    start_angle: a.start_angle, sweep_angle: split,
                }), Geom::Arc(Arc {
                    center: a.center, radius: a.radius,
                    start_angle: (a.start_angle + split).rem_euclid(std::f64::consts::TAU),
                    sweep_angle: a.sweep_angle - split,
                })))
            }
            Geom::EllipseArc(ea) => {
                let t = ea.ellipse.nearest_param(at);
                let local = (t - ea.start_param).rem_euclid(std::f64::consts::TAU);
                let split = local.clamp(EPS, ea.sweep_param - EPS);
                Ok((Geom::EllipseArc(EllipseArc {
                    ellipse: ea.ellipse,
                    start_param: ea.start_param, sweep_param: split,
                }), Geom::EllipseArc(EllipseArc {
                    ellipse: ea.ellipse,
                    start_param: (ea.start_param + split).rem_euclid(std::f64::consts::TAU),
                    sweep_param: ea.sweep_param - split,
                })))
            }
            Geom::Polyline(p) => {
                if p.vertices.len() < 2 { return Err("split: polyline needs 2+ vertices"); }
                // Find the segment closest to `at`; split that one.
                let n = p.vertices.len();
                let pairs = if p.closed { n } else { n - 1 };
                let mut best: Option<(usize, f64, Vec2)> = None;
                for i in 0..pairs {
                    let a = p.vertices[i].pos;
                    let b = p.vertices[(i + 1) % n].pos;
                    let d = b - a;
                    let len_sq = d.len_sq();
                    if len_sq < EPS { continue; }
                    let t = ((at - a).dot(d) / len_sq).clamp(0.0, 1.0);
                    let foot = a + d * t;
                    let dist = foot.dist(at);
                    if best.map_or(true, |(_, bd, _)| dist < bd) {
                        best = Some((i, dist, foot));
                    }
                }
                let (seg, _, foot) = best.ok_or("split: degenerate polyline")?;
                // Build first piece: vertices[0..=seg] + foot
                let mut first: Vec<PolyVertex> = p.vertices[..=seg].iter().cloned().collect();
                first.push(PolyVertex { pos: foot, bulge: 0.0 });
                // Build second piece: foot + vertices[seg+1..] (or wrap for closed)
                let mut second: Vec<PolyVertex> =
                    vec![PolyVertex { pos: foot, bulge: 0.0 }];
                if p.closed {
                    for i in 0..n {
                        let idx = (seg + 1 + i) % n;
                        second.push(p.vertices[idx].clone());
                        if idx == seg { break; }
                    }
                } else {
                    for v in &p.vertices[seg + 1..] {
                        second.push(v.clone());
                    }
                }
                Ok((Geom::Polyline(Polyline { vertices: first,  closed: false, widths: Vec::new() }),
                    Geom::Polyline(Polyline { vertices: second, closed: false, widths: Vec::new() })))
            }
            Geom::Circle(_) =>
                Err("split: circle needs TWO break points (1-click break not allowed)"),
            Geom::Ellipse(_) =>
                Err("split: closed ellipse needs TWO break points"),
            Geom::Point(_) =>
                Err("split: cannot split a point"),
            Geom::Hatch(_) =>
                Err("split: hatch entities cannot be split"),
            Geom::Spline(_) =>
                Err("split: spline entities cannot be split in v1 (knot insertion pending)"),
            Geom::Wall(w) => {
                // Split the centerline at `at`; wrap each piece as a
                // Wall with the same thickness.
                let line = Geom::Line(w.centerline());
                let (g1, g2) = line.split_at(at)?;
                match (g1, g2) {
                    (Geom::Line(l1), Geom::Line(l2)) => Ok((
                        Geom::Wall(Wall { start: l1.a, end: l1.b, thickness: w.thickness, style: w.style, bulge: 0.0 }),
                        Geom::Wall(Wall { start: l2.a, end: l2.b, thickness: w.thickness, style: w.style, bulge: 0.0 }),
                    )),
                    _ => Err("split wall: unexpected non-Line result"),
                }
            }
            Geom::Text(_) =>
                Err("split: cannot split a text entity"),
            Geom::Dimension(_) =>
                Err("split: cannot split a dimension entity"),
            Geom::BlockRef(_) =>
                Err("split: explode the block first"),
        }
    }
}

/// Re-join the TOUCHING fragments a trim leaves on a CLOSED curve. The trim
/// over-splits a circle / ellipse at EVERY cut point; after the clicked arc is
/// removed, the remaining consecutive arcs share their cut points and should
/// merge back into the natural run(s). Only fragments that actually TOUCH are
/// merged — the removed gap is preserved (so removing a middle arc still leaves
/// two parts). Lines and everything else pass through untouched (no collinear-
/// across-a-gap merge, which would undo the trim). Used right after a trim pick.
pub fn join_trim_survivors(pieces: Vec<Geom>) -> Vec<Geom> {
    let mut out: Vec<Geom> = Vec::new();
    let mut arcs:  Vec<Arc> = Vec::new();
    let mut earcs: Vec<EllipseArc> = Vec::new();
    for g in pieces {
        match g {
            Geom::Arc(a)        => arcs.push(a),
            Geom::EllipseArc(e) => earcs.push(e),
            other               => out.push(other),
        }
    }
    // Arcs grouped by (center, radius).
    while let Some(first) = arcs.first().copied() {
        let same = |a: &Arc| (a.center - first.center).len() < JOIN_EPS
            && (a.radius - first.radius).abs() < JOIN_EPS;
        let group: Vec<Arc> = arcs.iter().copied().filter(|a| same(a)).collect();
        arcs.retain(|a| !same(a));
        let ivs: Vec<(f64, f64)> = group.iter().map(|a| (a.start_angle, a.sweep_angle)).collect();
        let (merged, full) = circular_union(&ivs);
        if full {
            out.push(Geom::Circle(Circle { center: first.center, radius: first.radius }));
        } else {
            for (s, sw) in merged {
                out.push(Geom::Arc(Arc {
                    center: first.center, radius: first.radius,
                    start_angle: s, sweep_angle: sw }));
            }
        }
    }
    // Ellipse arcs grouped by underlying ellipse.
    while let Some(first) = earcs.first().copied() {
        let same = |e: &EllipseArc| same_ellipse(&e.ellipse, &first.ellipse);
        let group: Vec<EllipseArc> = earcs.iter().copied().filter(|e| same(e)).collect();
        earcs.retain(|e| !same(e));
        let ivs: Vec<(f64, f64)> = group.iter().map(|e| (e.start_param, e.sweep_param)).collect();
        let (merged, full) = circular_union(&ivs);
        if full {
            out.push(Geom::Ellipse(first.ellipse));
        } else {
            for (s, sw) in merged {
                out.push(Geom::EllipseArc(EllipseArc {
                    ellipse: first.ellipse, start_param: s, sweep_param: sw }));
            }
        }
    }
    out
}

fn same_ellipse(a: &Ellipse, b: &Ellipse) -> bool {
    (a.center - b.center).len() < JOIN_EPS
        && (a.major - b.major).len() < JOIN_EPS
        && (a.ratio - b.ratio).abs() < JOIN_EPS
}

/// Union of param intervals `(start, sweep)` on a CLOSED curve (period TAU).
/// Merges overlapping/touching intervals, preserves gaps. Returns the merged
/// `(start, sweep)` list and a `full` flag (total coverage ≈ TAU → whole curve).
/// Works regardless of wrap by rotating the origin into a gap first.
fn circular_union(intervals: &[(f64, f64)]) -> (Vec<(f64, f64)>, bool) {
    let tau = std::f64::consts::TAU;
    let eps = 1e-6;
    if intervals.is_empty() { return (Vec::new(), false); }
    let total: f64 = intervals.iter().map(|(_, l)| *l).sum();
    if total >= tau - eps { return (Vec::new(), true); }
    // Normalise starts into [0, TAU); find a point inside a GAP to use as origin
    // so nothing wraps in the rotated domain.
    let mut a: Vec<(f64, f64)> = intervals.iter()
        .map(|&(s, l)| (s.rem_euclid(tau), l)).collect();
    a.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
    let n = a.len();
    let mut origin = 0.0_f64;
    let mut found = false;
    for i in 0..n {
        let end_i = a[i].0 + a[i].1;
        let next_start = if i + 1 < n { a[i + 1].0 } else { a[0].0 + tau };
        if next_start - end_i > eps {
            origin = (end_i + (next_start - end_i) * 0.5).rem_euclid(tau);
            found = true;
            break;
        }
    }
    if !found { return (Vec::new(), true); }
    let mut rel: Vec<(f64, f64)> = a.iter()
        .map(|&(s, l)| ((s - origin).rem_euclid(tau), l)).collect();
    rel.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (s, l) in rel {
        if let Some(last) = merged.last_mut() {
            let last_end = last.0 + last.1;
            if s <= last_end + eps {
                last.1 = (last_end.max(s + l)) - last.0;
                continue;
            }
        }
        merged.push((s, l));
    }
    let abs: Vec<(f64, f64)> = merged.into_iter()
        .map(|(rs, l)| ((origin + rs).rem_euclid(tau), l)).collect();
    (abs, false)
}
