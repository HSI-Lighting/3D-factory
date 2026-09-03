//! trim.rs — TRIM command and trim-survivor re-joining.
//!
//! Split out of `geom.rs` verbatim (pure code-movement refactor). Contains
//! `Geom::trim_at`, the `join_trim_survivors` helper that re-merges the
//! touching fragments a trim leaves on a closed curve, and its private
//! support fn `same_ellipse`. (The angular-interval union `circular_union`
//! now lives in `math.rs`, shared with `join`.)

use crate::math::{Vec2, EPS};
use crate::geom::{Arc, Circle, Ellipse, EllipseArc, Geom, Line, PolyVertex, Polyline, Spline, Wall};
use crate::join::{arc_from_bulge, bulge_from_arc, polyline_segments, JOIN_EPS};

/// AutoCAD-correct TRIM survivors. `bounds` = sorted parameters
/// [target_start, …intersection_ts…, target_end]. The clicked interval is the
/// one containing `pick_t`; ONLY that interval is removed. Everything to the
/// LEFT stays as ONE continuous piece, everything to the RIGHT as one piece —
/// intersections outside the clicked interval do NOT split. Net survivors 0/1/2.
/// (Was a nested fn in `trim_at`; hoisted so the polyline trim can share it.)
fn surviving_segments(bounds: &[f64], pick_t: f64, eps: f64) -> Vec<(f64, f64)> {
    let n = bounds.len();
    if n < 2 { return Vec::new(); }
    let mut clicked: Option<(f64, f64)> = None;
    for i in 0..n - 1 {
        let (t1, t2) = (bounds[i], bounds[i + 1]);
        if (t2 - t1) <= eps { continue; }
        if pick_t >= t1 - eps && pick_t <= t2 + eps { clicked = Some((t1, t2)); break; }
    }
    let Some((left, right)) = clicked else {
        return vec![(bounds[0], bounds[n - 1])];
    };
    let mut out = Vec::new();
    if left - bounds[0] > eps { out.push((bounds[0], left)); }
    if bounds[n - 1] - right > eps { out.push((right, bounds[n - 1])); }
    out
}

/// Whole-curve TRIM for an OPEN, STRAIGHT (no-bulge), width-less polyline: the
/// polyline is treated as ONE continuous path whose NODES are its vertices AND
/// every crossing (external cutters + self-intersections). A click removes only
/// the one sub-edge between the two nearest nodes; the rest survives as up to two
/// still-connected runs. So clicking one arm of a self-touching shape removes
/// just that arm (to the neighbouring vertex/crossing), never the neighbour arm.
///
/// Global parameter `gp = i + t` (segment index `i`, fraction `t∈[0,1]`).
/// Returns `None` (caller falls back to the per-segment path) when the polyline
/// has bulges/widths, or no cutter crosses it at all.
fn trim_polyline_whole(
    p: &Polyline, cutters: &[Geom], pick: Vec2, edge_mode: bool,
) -> Option<Vec<Geom>> {
    use crate::intersect::intersect;
    if p.closed || !p.widths.is_empty() { return None; }
    if p.vertices.iter().any(|v| v.bulge.abs() > 1e-12) { return None; } // straight only
    let vs: Vec<Vec2> = p.vertices.iter().map(|v| v.pos).collect();
    let n = vs.len();
    if n < 2 { return None; }
    let nseg = n - 1;
    // Local param of point `q` on segment i (v[i]→v[i+1]).
    let t_on = |i: usize, q: Vec2| -> f64 {
        let (a, b) = (vs[i], vs[i + 1]);
        let d = b - a;
        let l2 = d.len_sq();
        if l2 < EPS { 0.0 } else { ((q - a).dot(d) / l2).clamp(0.0, 1.0) }
    };
    let point_at = |gp: f64| -> Vec2 {
        let i = (gp.floor() as usize).min(nseg - 1);
        let t = (gp - i as f64).clamp(0.0, 1.0);
        vs[i] + (vs[i + 1] - vs[i]) * t
    };
    // Cut points along the path (global params): external cutter crossings…
    let mut cut_gp: Vec<f64> = Vec::new();
    for i in 0..nseg {
        let seg = Geom::Line(Line { a: vs[i], b: vs[i + 1] });
        for c in cutters {
            let c_eff = if edge_mode { c.extended_for_edgemode() } else { c.clone() };
            for h in intersect(&seg, &c_eff) {
                let t = t_on(i, h);
                if t > 1e-9 && t < 1.0 - 1e-9 { cut_gp.push(i as f64 + t); }
            }
        }
    }
    // …AND the polyline's OWN self-intersections — a self-crossing polyline
    // divides at its crossings ("read the pline to its smallest parts"), so
    // trimming catches the real edge, not the whole vertex-to-vertex segment.
    // Adjacent segments share a vertex → skipped; on an OPEN polyline the first
    // and last segments are NOT adjacent, so they DO count.
    for i in 0..nseg {
        let si = Geom::Line(Line { a: vs[i], b: vs[i + 1] });
        for j in (i + 2)..nseg {
            let sj = Geom::Line(Line { a: vs[j], b: vs[j + 1] });
            for h in intersect(&si, &sj) {
                let (ti, tj) = (t_on(i, h), t_on(j, h));
                if ti > 1e-9 && ti < 1.0 - 1e-9 { cut_gp.push(i as f64 + ti); }
                if tj > 1e-9 && tj < 1.0 - 1e-9 { cut_gp.push(j as f64 + tj); }
            }
        }
    }
    // Every interior VERTEX is also a node. A click then removes ONLY the
    // sub-edge between the two nearest nodes (vertex OR crossing) — "cut just the
    // clicked part", never carrying a neighbour arm across a plain vertex.
    for i in 1..nseg { cut_gp.push(i as f64); }
    if cut_gp.is_empty() { return None; }   // single segment, no crossing → fallback
    cut_gp.sort_by(|a, b| a.partial_cmp(b).unwrap());
    cut_gp.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    // Pick's global param (nearest segment).
    let pick_gp = {
        let mut best = (f64::INFINITY, 0.0_f64);
        for i in 0..nseg {
            let seg = Geom::Line(Line { a: vs[i], b: vs[i + 1] });
            let d = seg.distance_to_point(pick);
            if d < best.0 { best = (d, i as f64 + t_on(i, pick)); }
        }
        best.1
    };
    let mut bounds = vec![0.0_f64];
    bounds.extend(&cut_gp);
    bounds.push(nseg as f64);
    // Build a polyline for each surviving [lo, hi] global-param interval.
    let mut out = Vec::new();
    for (lo, hi) in surviving_segments(&bounds, pick_gp, 1e-9) {
        let mut verts: Vec<PolyVertex> = vec![PolyVertex { pos: point_at(lo), bulge: 0.0 }];
        let mut k = lo.floor() as usize + 1;
        while (k as f64) < hi - 1e-9 { verts.push(PolyVertex { pos: vs[k], bulge: 0.0 }); k += 1; }
        let endp = point_at(hi);
        if verts.last().map_or(true, |v| (v.pos - endp).len() > 1e-9) {
            verts.push(PolyVertex { pos: endp, bulge: 0.0 });
        }
        if verts.len() >= 2 {
            out.push(Geom::Polyline(Polyline { vertices: verts, closed: false, widths: Vec::new() }));
        }
    }
    Some(out)
}

/// Whole-curve TRIM for a CLOSED, STRAIGHT (no-bulge), width-less polyline —
/// the closed counterpart of `trim_polyline_whole`. The ring's NODES are its
/// vertices AND every crossing (external cutters + self-intersections); ONLY the
/// clicked sub-arc — the span between the two nearest nodes bracketing the click
/// around the ring — is removed. The rest of the loop survives as ONE open
/// polyline (the complementary arc, wrapping through the closing segment). This
/// is what lets a self-overlapping closed polyline trim edge-by-edge instead of
/// exploding into raw vertex-to-vertex segments on the first click.
///
/// Global parameter `gp = i + t` on a ring of `n` segments (segment `n-1` is the
/// closing segment `v[n-1]→v[0]`); `gp` wraps in `[0, n)`.
/// Returns `None` (caller falls back to the explode path) for bulge/width
/// polylines, `< 3` vertices, or fewer than 2 distinct crossings (nothing to
/// bracket a sub-arc on a closed loop).
fn trim_polyline_whole_closed(
    p: &Polyline, cutters: &[Geom], pick: Vec2, edge_mode: bool,
) -> Option<Vec<Geom>> {
    use crate::intersect::intersect;
    use crate::join::polyline_segments;
    if !p.closed { return None; }
    let n = p.vertices.len();
    if n < 3 { return None; }
    let vs: Vec<Vec2> = p.vertices.iter().map(|v| v.pos).collect();
    let nseg = n;                       // includes the closing segment (n-1 → 0)
    // Per-segment geometry (Line or Arc) so bulge segments stay on the ring.
    let segs = polyline_segments(p);
    if segs.len() != n { return None; } // degenerate (skipped zero-chord arcs)
    let seg_a = |i: usize| vs[i];
    // Fraction along segment i of point `q` (0..1). Lines project onto the
    // chord; arcs map by sweep angle (exact for points ON the arc, which all
    // intersection hits are).
    let t_on = |i: usize, q: Vec2| -> f64 {
        match &segs[i] {
            Geom::Line(l) => {
                let d = l.b - l.a;
                let l2 = d.len_sq();
                if l2 < EPS { 0.0 } else { ((q - l.a).dot(d) / l2).clamp(0.0, 1.0) }
            }
            Geom::Arc(a) => {
                let ccw = ((q - a.center).angle() - a.start_angle)
                    .rem_euclid(std::f64::consts::TAU);
                let t = if a.sweep_angle > 0.0 {
                    ccw / a.sweep_angle
                } else {
                    (std::f64::consts::TAU - ccw) / (-a.sweep_angle)
                };
                t.clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    };
    // World position at global param `gp` (i + t): line lerp or arc sweep.
    let point_at = |gp: f64| -> Vec2 {
        let g = gp.rem_euclid(nseg as f64);
        let i = (g.floor() as usize) % n;
        let t = g - g.floor();
        match &segs[i] {
            Geom::Line(l) => l.a + (l.b - l.a) * t,
            Geom::Arc(a) => a.center + Vec2::new(
                (a.start_angle + a.sweep_angle * t).cos(),
                (a.start_angle + a.sweep_angle * t).sin()) * a.radius,
            _ => seg_a(i),
        }
    };
    // Cut points along the ring (global params): external cutter crossings…
    let mut cut_gp: Vec<f64> = Vec::new();
    for i in 0..nseg {
        for c in cutters {
            let c_eff = if edge_mode { c.extended_for_edgemode() } else { c.clone() };
            for h in intersect(&segs[i], &c_eff) {
                let t = t_on(i, h);
                if t > 1e-9 && t < 1.0 - 1e-9 { cut_gp.push(i as f64 + t); }
            }
        }
    }
    // …AND the loop's OWN self-intersections. Segments i and j are adjacent
    // (share a vertex, never a real crossing) when j == i+1 OR the wrap pair
    // {0, n-1}; those are skipped. A crossing lands on BOTH segments, so both
    // global params are recorded (the loop passes that world point twice).
    for i in 0..nseg {
        for j in (i + 1)..nseg {
            if j == i + 1 || (i == 0 && j == nseg - 1) { continue; } // adjacent
            for h in intersect(&segs[i], &segs[j]) {
                let (ti, tj) = (t_on(i, h), t_on(j, h));
                if ti > 1e-9 && ti < 1.0 - 1e-9 { cut_gp.push(i as f64 + ti); }
                if tj > 1e-9 && tj < 1.0 - 1e-9 { cut_gp.push(j as f64 + tj); }
            }
        }
    }
    // Every VERTEX is also a node (ring params 0..nseg), so a click removes ONLY
    // the sub-arc between the two nearest nodes (vertex OR crossing) — never a
    // neighbour arm across a plain vertex.
    for i in 0..nseg { cut_gp.push(i as f64); }
    cut_gp.sort_by(|a, b| a.partial_cmp(b).unwrap());
    cut_gp.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    if cut_gp.len() < 2 { return None; }    // can't bracket a sub-arc → explode path
    // Pick's global param (nearest segment around the ring).
    let pick_gp = {
        let mut best = (f64::INFINITY, 0.0_f64);
        for i in 0..nseg {
            let d = segs[i].distance_to_point(pick);
            if d < best.0 { best = (d, i as f64 + t_on(i, pick)); }
        }
        best.1
    };
    // Circular bracket: `hi` = first cut ≥ pick, `lo` = the cut just before it.
    // Both wrap. The removed arc is (lo → hi) containing the click; the survivor
    // is the complementary arc hi → (wrap) → lo, emitted as one open polyline.
    let k = cut_gp.len();
    let (lo, hi) = match cut_gp.iter().position(|&c| c >= pick_gp - 1e-9) {
        Some(0)   => (cut_gp[k - 1], cut_gp[0]),
        Some(idx) => (cut_gp[idx - 1], cut_gp[idx]),
        None      => (cut_gp[k - 1], cut_gp[0]),   // pick past last cut → wrap
    };
    // Walk the survivor forward from `hi` around to `lo` (wrapping past the seam).
    let end = if lo > hi { lo } else { lo + nseg as f64 };
    let span = end - hi;
    // The survivor's vertices: [c0 = point_at(hi), v_{sh+1} .. v_sl, c1].
    // Vertex i's bulge describes segment i → i+1 (DXF convention).
    let sh = (hi.floor() as usize) % n;          // segment containing the cut start
    let mut verts: Vec<PolyVertex> = vec![PolyVertex { pos: point_at(hi), bulge: 0.0 }];
    let mut wout: Vec<(f64, f64)> = Vec::new();
    // Entering partial segment (c0 → v_{sh+1}): keep the original width of
    // segment sh; its bulge is the SUB-ARC from the cut to the next vertex
    // (the included angle of a shorter chord is smaller, so the full
    // segment's bulge would over-bulge the partial).
    wout.push(p.widths.get(sh).copied().unwrap_or((0.0, 0.0)));
    let mut kk = hi.floor() as usize + 1;          // next integer vertex after hi
    while (kk as f64) - hi < span - 1e-9 {
        let pos = vs[kk % n];
        if verts.last().map_or(true, |v| (v.pos - pos).len() > 1e-9) {
            // The just-completed segment runs from the PREVIOUS vertex to
            // this one. If it is an ORIGINAL full segment (any push after the
            // first), keep the original vertex's own bulge (owned by the
            // segment's start vertex). For the FIRST push the previous vertex
            // is c0 — its partial-segment bulge is fixed below.
            let owner = (kk - 1) % n;
            if verts.len() > 1 {
                if let Some(last) = verts.last_mut() {
                    last.bulge = p.vertices[owner].bulge;
                }
            }
            verts.push(PolyVertex { pos, bulge: 0.0 });
            // Width of the NEXT segment (this vertex → following) = original.
            wout.push(p.widths.get(kk % n).copied().unwrap_or((0.0, 0.0)));
        }
        kk += 1;
    }
    // c0's outgoing segment is a partial of segment sh — recompute its bulge
    // from the sub-arc (line → 0). When the cut lands exactly ON a vertex the
    // segment is the FULL original one and keeps its own bulge.
    let hi_frac = hi.rem_euclid(nseg as f64);
    let hi_is_vertex = (hi_frac - hi_frac.floor()).abs() < 1e-9;
    if verts.len() >= 2 {
        let next_v = vs[(sh + 1) % n];
        if let Some(c0) = verts.first_mut() {
            if hi_is_vertex {
                c0.bulge = p.vertices[sh].bulge;
            } else {
                match &segs[sh] {
                    Geom::Arc(a) => {
                        c0.bulge = sub_arc_bulge(a, c0.pos, next_v);
                    }
                    _ => { c0.bulge = 0.0; }
                }
            }
        }
    }
    // Closing partial: the segment from the last whole vertex to point_at(lo)
    // is a sub-arc of segment sl — recompute the bulge. If the walk ended
    // exactly ON a vertex there is no partial: the last vertex keeps its own
    // (full original) outgoing bulge.
    let endp = point_at(end);
    let end_frac = end.rem_euclid(nseg as f64);
    let end_is_vertex = (end_frac - end_frac.floor()).abs() < 1e-9;
    if end_is_vertex {
        let last_vi = (end_frac.floor() as usize + nseg - 1) % n;
        if let Some(last) = verts.last_mut() {
            last.bulge = p.vertices[last_vi].bulge;
        }
        if verts.last().map_or(true, |v| (v.pos - endp).len() > 1e-9) {
            verts.push(PolyVertex { pos: endp, bulge: 0.0 });
        }
    } else {
        let sl = end_frac.floor() as usize % n;
        let last_v = vs[(end_frac.floor() as usize + nseg - 1) % n];
        if let Some(last) = verts.last_mut() {
            match &segs[sl] {
                Geom::Arc(a) => {
                    last.bulge = sub_arc_bulge(a, last_v, endp);
                }
                _ => { last.bulge = 0.0; }
            }
        }
        if verts.last().map_or(true, |v| (v.pos - endp).len() > 1e-9) {
            verts.push(PolyVertex { pos: endp, bulge: 0.0 });
        }
    }
    if verts.len() < 2 { return Some(Vec::new()); }
    let widths = if p.widths.is_empty() { Vec::new() } else { wout };
    Some(vec![Geom::Polyline(Polyline { vertices: verts, closed: false, widths })])
}

/// The DXF bulge of the SUB-ARC from `from` to `to` along `arc` (both
/// points lie ON the arc). The included angle of a shorter chord is smaller
/// than the full segment's, so tan(full_sweep/4) would over-bulge it.
fn sub_arc_bulge(arc: &Arc, from: Vec2, to: Vec2) -> f64 {
    let tau = std::f64::consts::TAU;
    let t1 = ((from - arc.center).angle() - arc.start_angle).rem_euclid(tau);
    let t2 = ((to - arc.center).angle() - arc.start_angle).rem_euclid(tau);
    let d = (t2 - t1).rem_euclid(tau);
    let sweep = if arc.sweep_angle >= 0.0 { d } else { d - tau };
    (sweep * 0.25).tan()
}

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
                    // Straight, width-less open polyline: trim the WHOLE path at
                    // the cutter crossings (a cutting edge cuts it like any other
                    // dobject). Bulge/width polylines fall back to per-segment.
                    if let Some(pieces) = trim_polyline_whole(p, cutters, pick, edge_mode) {
                        return Ok(pieces);
                    }
                    return Ok(trim_polyline_connected(p, &segs, best_i, cutters, pick, edge_mode));
                }
                // CLOSED polyline: whole-ring trim — removes only the clicked
                // sub-arc (bracketed by the nearest crossings — cutter OR
                // self-intersection) and keeps the rest as ONE open polyline
                // with original bulges + widths preserved (arc segments stay
                // arcs, rects with pen width stay single polylines — #18).
                // Degenerate rings (skipped zero-chord arcs) fall back to the
                // per-segment path below.
                if let Some(pieces) =
                    trim_polyline_whole_closed(p, cutters, pick, edge_mode)
                {
                    return Ok(pieces);
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
            // Issue #21 — parameter-space spline trim: the nearest cutter
            // crossing (in parameter space, via the tessellated curve) splits
            // the spline with knot insertion; the half containing the pick
            // is kept, the other is discarded.
            Geom::Spline(s) => {
                if s.control_points.len() <= s.degree {
                    return Err("trim: degenerate spline");
                }
                let samples = s.tessellate(64);
                if samples.len() < 2 {
                    return Err("trim: degenerate spline");
                }
                let to_param = |p: Vec2| -> f64 {
                    let mut best = (f64::INFINITY, 0usize);
                    for (i, w) in samples.windows(2).enumerate() {
                        let d = crate::modify::point_seg_dist(p, w[0], w[1]);
                        if d < best.0 { best = (d, i); }
                    }
                    let (a, b) = (samples[best.1], samples[best.1 + 1]);
                    let l2 = (b - a).len_sq();
                    let t = if l2 < EPS { 0.0 }
                        else { ((p - a).dot(b - a) / l2).clamp(0.0, 1.0) };
                    (best.1 as f64 + t) / (samples.len() - 1) as f64
                };
                let mut params: Vec<f64> = hits.iter().map(|&p| to_param(p))
                    .filter(|&t| t > 1e-9 && t < 1.0 - 1e-9).collect();
                params.sort_by(|a, b| a.partial_cmp(b).unwrap());
                params.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
                if params.is_empty() {
                    return Err("trim: target has no interior intersection");
                }
                let pick_t = to_param(pick);
                let cut = params.iter().cloned()
                    .min_by(|a, b| (a - pick_t).abs()
                        .partial_cmp(&(b - pick_t).abs())
                        .unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap();
                let (left, right) = s.split_at(cut);
                if pick_t < cut {
                    Ok(vec![Geom::Spline(left)])
                } else {
                    Ok(vec![Geom::Spline(right)])
                }
            }
            Geom::Wall(w) => {
                // Trim the wall's CENTERLINE and wrap each surviving
                // sub-segment as a new Wall with the same thickness. A
                // CURVED wall (bulge ≠ 0) trims along its ARC centerline —
                // the straight chord would cut at the wrong point and lose
                // the curvature (#18).
                let center_geom = if w.is_curved() {
                    match crate::join::arc_from_bulge(w.start, w.end, w.bulge) {
                        Some((center, r, a0, sweep)) => Geom::Arc(Arc {
                            center, radius: r, start_angle: a0, sweep_angle: sweep,
                        }),
                        None => Geom::Line(w.centerline()),
                    }
                } else {
                    Geom::Line(w.centerline())
                };
                let pieces = center_geom.trim_at(cutters, pick, edge_mode)?;
                Ok(pieces.into_iter().filter_map(|g| match g {
                    Geom::Line(seg) => Some(Geom::Wall(Wall {
                        start: seg.a, end: seg.b, thickness: w.thickness,
                        style: w.style, bulge: 0.0,
                    })),
                    Geom::Arc(a) => {
                        // The surviving arc piece keeps its own curvature —
                        // encode it back as a wall bulge.
                        let (s, e) = a.endpoints();
                        let bl = crate::join::bulge_from_arc(
                            s, e, a.center, a.sweep_angle);
                        Some(Geom::Wall(Wall {
                            start: s, end: e, thickness: w.thickness,
                            style: w.style, bulge: bl,
                        }))
                    }
                    _ => None,
                }).collect())
            }
            Geom::Xline(_) =>
                Err("trim: an xline is infinite and has no curve to cut"),
            Geom::Ray(_) =>
                Err("trim: a ray is infinite and has no curve to cut"),
            Geom::Donut(_) =>
                Err("trim: a donut is a filled ring with no curve to cut"),
            Geom::Wipeout(_) =>
                Err("trim: a wipeout is a mask with no curve to cut"),
            Geom::Region(_) =>
                Err("trim: a region is a filled area with no curve to cut"),
            Geom::Table(_) =>
                Err("trim: a table has no curve to cut"),
            Geom::Xref(_) =>
                Err("trim: explode the xref first"),
            Geom::Text(_) =>
                Err("trim: text entities have no curve to cut"),
            Geom::Leader(_) =>
                Err("trim: explode the leader first"),
            Geom::CenterMark(_) =>
                Err("trim: a center mark has no curve to cut"),
            Geom::AttrDef(_) =>
                Err("trim: attribute definitions have no curve to cut"),
            Geom::Dimension(_) =>
                Err("trim: dimensions have no curve to cut"),
            Geom::BlockRef(_) =>
                Err("trim: explode the block first"),
            Geom::Viewport(_) =>
                Err("trim: viewport is a paper-space entity"),
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
            // Issue #23 — extend the segment NEAREST the click, interior
            // segments included. The old code only ever touched the first or
            // last segment, so interior vertices couldn't be extended.
            let (seg_i, _) = segs.iter().enumerate()
                .map(|(i, g)| (i, g.distance_to_point(pick)))
                .min_by(|a, b| a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or((0, f64::INFINITY));
            let extended = segs[seg_i].extend_to(boundaries, pick, edge_mode)?;
            let (ea, eb) = match &extended {
                Geom::Line(l) => (l.a, l.b),
                Geom::Arc(a)  => a.endpoints(),
                _ => return Err("extend: unsupported polyline end segment"),
            };
            // The extended segment keeps one of its ORIGINAL endpoints; the
            // other endpoint is the new free one (which replaces the segment
            // vertex it moved from).
            let next = |i: usize| if i + 1 < n { i + 1 } else { 0 };   // closed wrap
            let (seg_a, seg_b) = (p.vertices[seg_i].pos, p.vertices[next(seg_i)].pos);
            let is_original = |pt: Vec2| pt.dist(seg_a) < 1e-6 || pt.dist(seg_b) < 1e-6;
            let new_free = if is_original(ea) { eb } else { ea };
            let mut verts = p.vertices.clone();
            // The moved vertex is the segment endpoint nearest the new free
            // endpoint; the other endpoint of the segment stays put.
            let (free_idx, fixed_idx) =
                if new_free.dist(seg_a) <= new_free.dist(seg_b) { (seg_i, next(seg_i)) }
                else { (next(seg_i), seg_i) };
            verts[free_idx].pos = new_free;
            // Recompute the affected segment's bulge if it's an arc (the
            // segment's leading bulge lives at `verts[seg_i].bulge`).
            verts[seg_i].bulge = match &extended {
                Geom::Arc(a) => bulge_from_arc(
                    verts[fixed_idx].pos, verts[free_idx].pos, a.center, a.sweep_angle),
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
        // Issue #21 — a spline target reaches no boundary on its own (the
        // point of extending); its arm computes the tangent-ray crossing
        // itself, so exempt it from the pre-gate. A WALL resolves its own
        // centerline (straight OR arc for curved walls — #18) inside its
        // arm, and the pre-gate's straight-chord intersect would miss arc
        // crossings, so exempt it too.
        if hits.is_empty() && !matches!(self, Geom::Spline(_) | Geom::Wall(_)) {
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
                // Pick the end by the click's PARAMETER along the arc, not raw
                // endpoint distance: a click near the midpoint must split the
                // sweep in half, and clicks on either extension must pick the
                // extension's own end (raw distance flips arbitrarily near the
                // middle and misreads the far wrap side of a full-ish arc).
                let local = to_local(pick);
                let at_end = if local <= arc.sweep_angle {
                    local >= arc.sweep_angle * 0.5
                } else {
                    // past the end forward — nearer to the end, or to the
                    // start across the wrap (click just before the start)?
                    (local - arc.sweep_angle) < (std::f64::consts::TAU - local)
                };
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
                // Extend the CENTERLINE (arc for curved walls) to the
                // nearest boundary; wrap the result as a Wall keeping the
                // curvature.
                let center_geom = if w.is_curved() {
                    match crate::join::arc_from_bulge(w.start, w.end, w.bulge) {
                        Some((center, r, a0, sweep)) => Geom::Arc(Arc {
                            center, radius: r, start_angle: a0, sweep_angle: sweep,
                        }),
                        None => Geom::Line(w.centerline()),
                    }
                } else {
                    Geom::Line(w.centerline())
                };
                let g = center_geom.extend_to(boundaries, pick, edge_mode)?;
                match g {
                    Geom::Line(new_line) => {
                        Ok(Geom::Wall(Wall {
                            start: new_line.a, end: new_line.b,
                            thickness: w.thickness,
                            style: w.style, bulge: 0.0,
                        }))
                    }
                    Geom::Arc(a) => {
                        let (s, e) = a.endpoints();
                        let bl = crate::join::bulge_from_arc(
                            s, e, a.center, a.sweep_angle);
                        Ok(Geom::Wall(Wall {
                            start: s, end: e,
                            thickness: w.thickness,
                            style: w.style, bulge: bl,
                        }))
                    }
                    _ => Err("extend wall: unexpected non-Line/Arc result"),
                }
            }
            // FIX 1: elliptical arc — the elliptical analogue of the Arc arm, but
            // worked in PARAMETER space (start_param/sweep_param), not geometric
            // angle. `target_infinite` is the full underlying ellipse (see
            // `extended_for_edgemode`), so `hits` are points on that ellipse; map
            // each to its ellipse parameter to grow the swept range to the nearest
            // boundary past the picked end. All ellipse∩{line,arc,circle,ellipse}
            // intersections already exist, so no new intersection code is needed.
            Geom::EllipseArc(ea) => {
                if ea.ellipse.semi_major() < EPS { return Err("extend: degenerate ellipse arc"); }
                let tau = std::f64::consts::TAU;
                // Parameter of a hit (a point ON the ellipse), relative to the
                // arc's start_param, wrapped to [0, TAU).
                let to_local = |p: Vec2| -> f64 {
                    (ea.ellipse.nearest_param(p) - ea.start_param).rem_euclid(tau)
                };
                // Same parameter-side rule as the Arc arm: split the swept
                // range at its midpoint, and route extension-side clicks to
                // the extension's own end.
                let local = to_local(pick);
                let at_end = if local <= ea.sweep_param {
                    local >= ea.sweep_param * 0.5
                } else {
                    (local - ea.sweep_param) < (tau - local)
                };
                if at_end {
                    // Grow the sweep forward: smallest param past the current end.
                    let candidate = hits.iter().map(|&p| to_local(p))
                        .filter(|&t| t > ea.sweep_param + EPS).fold(f64::INFINITY, f64::min);
                    if candidate.is_infinite() || candidate >= tau {
                        return Err("extend: no boundary intersection past the ellipse-arc end");
                    }
                    Ok(Geom::EllipseArc(EllipseArc {
                        ellipse: ea.ellipse,
                        start_param: ea.start_param,
                        sweep_param: candidate,
                    }))
                } else {
                    // Grow the start backward: largest param < 0 (wrap the far side).
                    let candidate = hits.iter().map(|&p| {
                        let raw = to_local(p);
                        if raw > ea.sweep_param + EPS { raw - tau } else { raw }
                    }).filter(|&t| t < -EPS).fold(f64::NEG_INFINITY, f64::max);
                    if candidate.is_infinite() {
                        return Err("extend: no boundary intersection before the ellipse-arc start");
                    }
                    let new_start = (ea.start_param + candidate).rem_euclid(tau);
                    Ok(Geom::EllipseArc(EllipseArc {
                        ellipse: ea.ellipse,
                        start_param: new_start,
                        sweep_param: ea.sweep_param - candidate,
                    }))
                }
            }
            // FIX 2: closed curves are genuinely unextendable — say so precisely
            // instead of the old generic "only Line/Arc/Wall" message.
            Geom::Circle(_)  => Err("extend: can't extend a closed circle"),
            Geom::Ellipse(_) => Err("extend: can't extend a closed ellipse"),
            // Issue #21 — spline extension: extend the end nearest the pick
            // ALONG ITS TANGENT to the nearest boundary crossing. The free
            // endpoint becomes the boundary hit (the end control point moves
            // along the end tangent, so G1 continuity is preserved).
            Geom::Spline(s) => {
                if s.control_points.len() <= s.degree {
                    return Err("extend: degenerate spline");
                }
                let samples = s.tessellate(64);
                if samples.len() < 2 {
                    return Err("extend: degenerate spline");
                }
                let n = samples.len();
                let at_start = pick.dist(samples[0]) < pick.dist(samples[n - 1]);
                let (end_pt, tang) = if at_start {
                    (samples[0], samples[0] - samples[1])
                } else {
                    (samples[n - 1], samples[n - 1] - samples[n - 2])
                };
                if tang.len() < EPS {
                    return Err("extend: degenerate spline end tangent");
                }
                // Nearest boundary crossing PAST the free end along the end
                // tangent (measured from the endpoint; t > 0 = outward).
                let dir = tang.normalized();
                let ray = Geom::Line(Line { a: end_pt, b: end_pt + dir * 1e9 });
                let mut hits: Vec<Vec2> = Vec::new();
                for b in boundaries {
                    let b_eff = if edge_mode { b.extended_for_edgemode() } else { b.clone() };
                    hits.extend(intersect(&ray, &b_eff));
                }
                let candidate = hits.iter().map(|&p| (p - end_pt).dot(dir))
                    .filter(|&t| t > EPS).fold(f64::INFINITY, f64::min);
                if candidate.is_infinite() {
                    return Err("extend: no boundary intersection past the spline end");
                }
                let new_end = end_pt + dir * candidate;
                let mut cp = s.control_points.clone();
                if at_start {
                    cp[0] = new_end;
                } else {
                    *cp.last_mut().unwrap() = new_end;
                }
                Ok(Geom::Spline(Spline {
                    degree: s.degree,
                    control_points: cp,
                    weights: s.weights.clone(),
                    knots: s.knots.clone(),
                    width: s.width,  // lengthen/trim preserves the ribbon width
                }))
            }
            _ => Err("extend: unsupported target type (hatch extend not yet supported)"),
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
                // G3: `f64::clamp` PANICS when min > max. For a near-zero sweep,
                // `sweep_angle - EPS < EPS`, so bail before clamping.
                if a.sweep_angle < 2.0 * EPS { return Err("split: arc too small to split"); }
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
                // G3: same near-zero-sweep clamp panic as the Arc arm.
                if ea.sweep_param < 2.0 * EPS { return Err("split: ellipse-arc too small to split"); }
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
                // Find the segment closest to `at`; split that one. Arc
                // segments (bulge ≠ 0) are measured against the ARC itself
                // and split into TWO ARCS that rejoin to the original curve
                // (the old code flattened the split arc into straight
                // segments — the "breaking a polyline at an arc gives an
                // incorrect result" bug).
                let n = p.vertices.len();
                let pairs = if p.closed { n } else { n - 1 };
                let tau = std::f64::consts::TAU;
                let mut best: Option<(usize, f64, Vec2, (f64, f64))> = None;
                for i in 0..pairs {
                    let a = p.vertices[i].pos;
                    let b = p.vertices[(i + 1) % n].pos;
                    let bulge = p.vertices[i].bulge;
                    let d = b - a;
                    let len_sq = d.len_sq();
                    if len_sq < EPS { continue; }
                    let (foot, dist, half_bulges) =
                        if let Some((center, r, start_ang, sweep)) = arc_from_bulge(a, b, bulge) {
                            // Project the click onto the arc (not the chord).
                            let foot_ang = (at - center).angle();
                            let foot = center + Vec2::new(r * foot_ang.cos(), r * foot_ang.sin());
                            let dist = foot.dist(at);
                            // Fraction of the SIGNED sweep to the foot.
                            let diff = (foot_ang - start_ang).rem_euclid(tau);
                            let raw = if sweep > 0.0 { diff } else { diff - tau };
                            let t = (raw / sweep).clamp(EPS, 1.0 - EPS);
                            let s1 = t * sweep;
                            let s2 = (1.0 - t) * sweep;
                            let b1 = bulge_from_arc(a, foot, center, s1.abs());
                            let b2 = bulge_from_arc(foot, b, center, s2.abs());
                            (foot, dist, (b1, b2))
                        } else {
                            // Straight segment: project onto the chord.
                            let t = ((at - a).dot(d) / len_sq).clamp(0.0, 1.0);
                            let foot = a + d * t;
                            (foot, foot.dist(at), (0.0, 0.0))
                        };
                    if best.map_or(true, |(_, bd, _, _)| dist < bd) {
                        best = Some((i, dist, foot, half_bulges));
                    }
                }
                let (seg, _, foot, (bulge_first, bulge_second)) =
                    best.ok_or("split: degenerate polyline")?;
                // Build first piece: vertices[0..=seg] + foot. The seg→foot
                // segment now carries `bulge_first` (its own leading bulge).
                let mut first: Vec<PolyVertex> = p.vertices[..=seg].iter().cloned().collect();
                first.last_mut().unwrap().bulge = bulge_first;
                first.push(PolyVertex { pos: foot, bulge: 0.0 });
                // Build second piece: foot + vertices[seg+1..] (or wrap for
                // closed). foot's leading bulge is `bulge_second` (foot→seg+1).
                let mut second: Vec<PolyVertex> =
                    vec![PolyVertex { pos: foot, bulge: bulge_second }];
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
            // Issue #21 — split a spline at the projection of `at` onto the
            // curve: nearest tessellated segment → normalized parameter →
            // knot-insertion split (both halves stay exact splines).
            Geom::Spline(s) => {
                if s.control_points.len() <= s.degree {
                    return Err("split: degenerate spline");
                }
                let samples = s.tessellate(64);
                if samples.len() < 2 {
                    return Err("split: degenerate spline");
                }
                let mut best = (f64::INFINITY, 0usize);
                for (i, w) in samples.windows(2).enumerate() {
                    let d = crate::modify::point_seg_dist(at, w[0], w[1]);
                    if d < best.0 { best = (d, i); }
                }
                let (a, b) = (samples[best.1], samples[best.1 + 1]);
                let l2 = (b - a).len_sq();
                let t = if l2 < EPS { 0.0 }
                    else { ((at - a).dot(b - a) / l2).clamp(0.0, 1.0) };
                let u = ((best.1 as f64 + t) / (samples.len() - 1) as f64)
                    .clamp(1e-6, 1.0 - 1e-6);
                let (left, right) = s.split_at(u);
                Ok((Geom::Spline(left), Geom::Spline(right)))
            }
            Geom::Wall(w) => {
                // Split the CENTERLINE at `at` (arc for curved walls); wrap
                // each piece as a Wall with the same thickness.
                let center_geom = if w.is_curved() {
                    match crate::join::arc_from_bulge(w.start, w.end, w.bulge) {
                        Some((center, r, a0, sweep)) => Geom::Arc(Arc {
                            center, radius: r, start_angle: a0, sweep_angle: sweep,
                        }),
                        None => Geom::Line(w.centerline()),
                    }
                } else {
                    Geom::Line(w.centerline())
                };
                let (g1, g2) = center_geom.split_at(at)?;
                match (g1, g2) {
                    (Geom::Line(l1), Geom::Line(l2)) => Ok((
                        Geom::Wall(Wall { start: l1.a, end: l1.b, thickness: w.thickness, style: w.style, bulge: 0.0 }),
                        Geom::Wall(Wall { start: l2.a, end: l2.b, thickness: w.thickness, style: w.style, bulge: 0.0 }),
                    )),
                    (Geom::Arc(a1), Geom::Arc(a2)) => {
                        let (s1, e1) = a1.endpoints();
                        let (s2, e2) = a2.endpoints();
                        let b1 = crate::join::bulge_from_arc(s1, e1, a1.center, a1.sweep_angle);
                        let b2 = crate::join::bulge_from_arc(s2, e2, a2.center, a2.sweep_angle);
                        Ok((
                            Geom::Wall(Wall { start: s1, end: e1, thickness: w.thickness, style: w.style, bulge: b1 }),
                            Geom::Wall(Wall { start: s2, end: e2, thickness: w.thickness, style: w.style, bulge: b2 }),
                        ))
                    }
                    _ => Err("split wall: unexpected non-Line/Arc result"),
                }
            }
            Geom::Xline(_) =>
                Err("split: cannot split an xline"),
            Geom::Ray(_) =>
                Err("split: cannot split a ray"),
            Geom::Donut(_) =>
                Err("split: cannot split a donut"),
            Geom::Wipeout(_) =>
                Err("split: cannot split a wipeout"),
            Geom::Region(_) =>
                Err("split: cannot split a region"),
            Geom::Table(_) =>
                Err("split: cannot split a table"),
            Geom::Xref(_) =>
                Err("split: cannot split an xref"),
            Geom::Text(_) =>
                Err("split: cannot split a text entity"),
            Geom::Leader(_) =>
                Err("split: cannot split a leader entity"),
            Geom::CenterMark(_) =>
                Err("split: cannot split a center mark"),
            Geom::AttrDef(_) =>
                Err("split: cannot split an attribute definition"),
            Geom::Dimension(_) =>
                Err("split: cannot split a dimension entity"),
            Geom::BlockRef(_) =>
                Err("split: explode the block first"),
            Geom::Viewport(_) =>
                Err("split: viewport is a paper-space entity"),
        }
    }

    /// Issue #19 — AutoCAD-style TWO-point break for closed curves: the arc
    /// from `p1` COUNTERCLOCKWISE to `p2` is removed, leaving the
    /// complementary arc running from `p2` CCW back to `p1` (a circle → one
    /// Arc, an ellipse → one EllipseArc). Everything else returns Err.
    pub fn break_two(&self, p1: Vec2, p2: Vec2) -> Result<Geom, &'static str> {
        let tau = std::f64::consts::TAU;
        match self {
            Geom::Circle(c) => {
                let a1 = (p1 - c.center).angle();
                let a2 = (p2 - c.center).angle();
                let sweep = (a1 - a2).rem_euclid(tau);
                if sweep < EPS || sweep >= tau - EPS {
                    return Err("break: the two break points coincide");
                }
                Ok(Geom::Arc(Arc {
                    center: c.center, radius: c.radius,
                    start_angle: a2, sweep_angle: sweep,
                }))
            }
            Geom::Ellipse(e) => {
                let t1 = e.nearest_param(p1);
                let t2 = e.nearest_param(p2);
                let sweep = (t1 - t2).rem_euclid(tau);
                if sweep < EPS || sweep >= tau - EPS {
                    return Err("break: the two break points coincide");
                }
                Ok(Geom::EllipseArc(EllipseArc {
                    ellipse: *e, start_param: t2, sweep_param: sweep,
                }))
            }
            _ => Err("break: two-point break only applies to circles and ellipses"),
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
        let (merged, full) = crate::math::circular_union(&ivs);
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
        let (merged, full) = crate::math::circular_union(&ivs);
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

// `circular_union` moved to `math.rs` (`crate::math::circular_union`) so `join`
// can share it without depending on trim internals (B16b/G6). Same gap-rotation
// algorithm; the disjoint-only `total >= TAU` shortcut was dropped there (it
// falsely reports "full" for join's overlapping inputs; disjoint trim inputs are
// unaffected — no gap ⟺ full either way).

#[cfg(test)]
mod extend_end_side_tests {
    use super::*;
    use std::f64::consts::PI;

    /// The 0..π upper half of the unit-ish circle (r=5, center origin).
    fn upper_half_arc() -> Geom {
        Geom::Arc(Arc {
            center: Vec2::new(0.0, 0.0), radius: 5.0,
            start_angle: 0.0, sweep_angle: PI,
        })
    }

    /// Horizontal boundary y = -2: the full circle crosses it at angle
    /// -0.411516846 (backward extension of the START) and at angle
    /// π + 0.411516846 (forward extension of the END).
    fn boundary_line() -> Geom {
        Geom::Line(Line { a: Vec2::new(-10.0, -2.0), b: Vec2::new(10.0, -2.0) })
    }

    fn on_arc(angle: f64) -> Vec2 {
        Vec2::new(5.0 * angle.cos(), 5.0 * angle.sin())
    }

    fn assert_end_extended(g: &Geom) {
        let Geom::Arc(a) = g else { panic!("expected an arc") };
        assert_eq!(a.start_angle, 0.0, "start angle untouched");
        assert!((a.sweep_angle - (PI + 0.411516846)).abs() < 1e-6,
            "sweep must grow forward to the boundary: got {}", a.sweep_angle);
    }

    fn assert_start_extended(g: &Geom) {
        let Geom::Arc(a) = g else { panic!("expected an arc") };
        // The start angle moves backward and is stored wrapped into [0, TAU):
        // 0 + (-0.411516846) → TAU - 0.411516846.
        let expected_start = std::f64::consts::TAU - 0.411516846;
        assert!((a.start_angle - expected_start).abs() < 1e-6,
            "start angle must grow backward: got {}", a.start_angle);
        assert!((a.sweep_angle - (PI + 0.411516846)).abs() < 1e-6);
    }

    #[test]
    fn click_at_midpoint_extends_end() {
        // Exactly at the midpoint the sweep splits at half — end side wins
        // (the tie-break), so the sweep grows forward.
        let out = upper_half_arc()
            .extend_to(&[boundary_line()], on_arc(PI / 2.0), false).expect("extend");
        assert_end_extended(&out);
    }

    #[test]
    fn click_just_before_midpoint_extends_start() {
        // Slightly before the midpoint (raw distance would still call it
        // "closer to the start end" — but the parameter rule must split at
        // the sweep midpoint, so the START side is extended).
        let out = upper_half_arc()
            .extend_to(&[boundary_line()], on_arc(PI / 2.0 - 0.1), false).expect("extend");
        assert_start_extended(&out);
    }

    #[test]
    fn click_past_end_extends_end() {
        let out = upper_half_arc()
            .extend_to(&[boundary_line()], on_arc(PI + 0.2), false).expect("extend");
        assert_end_extended(&out);
    }

    #[test]
    fn click_before_start_wraps_to_start_side() {
        // The click just before the start wraps to ~TAU in local coords; the
        // rule must route it to the START extension, not the END.
        let out = upper_half_arc()
            .extend_to(&[boundary_line()], on_arc(-0.2), false).expect("extend");
        assert_start_extended(&out);
    }

    #[test]
    fn polyline_extend_targets_interior_segment() {
        // Issue #23: clicking the MIDDLE segment extends it — the old code
        // only ever grew the first or last segment (and this click would
        // have failed: the last, horizontal segment never meets the
        // horizontal boundary).
        let g = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(20.0, 10.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        });
        let boundary = Geom::Line(Line {
            a: Vec2::new(-5.0, 15.0), b: Vec2::new(25.0, 15.0),
        });
        // Click on the middle (vertical x=10) segment, near its top end.
        let out = g.extend_to(&[boundary], Vec2::new(10.0, 8.0), false).expect("extend");
        let Geom::Polyline(p) = out else { panic!("expected polyline") };
        assert_eq!(p.vertices.len(), 4);
        assert_eq!(p.vertices[0].pos, Vec2::new(0.0, 0.0), "first vertex stays");
        assert_eq!(p.vertices[1].pos, Vec2::new(10.0, 0.0), "segment bottom stays");
        assert_eq!(p.vertices[2].pos, Vec2::new(10.0, 15.0),
            "middle segment must extend up to the boundary");
        assert_eq!(p.vertices[3].pos, Vec2::new(20.0, 10.0), "last vertex stays");
    }

    #[test]
    fn polyline_extend_interior_segment_keeps_other_segments() {
        // A 4-vertex polyline with an interior ARC segment: extending the
        // arc segment's end keeps both neighbours untouched.
        let g = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                // Arc segment from (10,0) to (10,10) bulging to the right.
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.5 },
                PolyVertex { pos: Vec2::new(20.0, 10.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        });
        // Boundary crossing the arc segment's forward extension (a vertical
        // line to the right of the arc's peak).
        let boundary = Geom::Line(Line {
            a: Vec2::new(15.0, -5.0), b: Vec2::new(15.0, 20.0),
        });
        let out = g.extend_to(&[boundary], Vec2::new(12.0, 8.0), false)
            .expect("arc segment extends");
        let Geom::Polyline(p) = out else { panic!("expected polyline") };
        assert_eq!(p.vertices.len(), 4);
        assert_eq!(p.vertices[0].pos, Vec2::new(0.0, 0.0));
        assert_eq!(p.vertices[1].pos, Vec2::new(10.0, 0.0));
        assert!(p.vertices[2].pos.x > 10.5,
            "arc free end must move toward the boundary: {:?}", p.vertices[2].pos);
        assert_eq!(p.vertices[3].pos, Vec2::new(20.0, 10.0), "last vertex stays");
    }

    #[test]
    fn ellipse_arc_midpoint_split_uses_parameter_not_endpoint_distance() {
        let g = Geom::EllipseArc(EllipseArc {
            ellipse: Ellipse {
                center: Vec2::new(0.0, 0.0), major: Vec2::new(5.0, 0.0), ratio: 0.5,
            },
            start_param: 0.0, sweep_param: PI,
        });
        // Boundary: a circle of radius 4.9 crosses the full ellipse near both
        // the backward start extension and the forward end extension.
        let b = Geom::Circle(Circle { center: Vec2::new(0.0, 0.0), radius: 4.9 });
        let Geom::EllipseArc(ea0) = &g else { unreachable!() };
        let point_at = |t: f64| ea0.ellipse.point_at(ea0.start_param + t);
        // Click just BEFORE the ellipse-arc midpoint: start side must extend.
        let pick = point_at(PI / 2.0 - 0.1);
        let out = g.extend_to(&[b], pick, false).expect("extend");
        let Geom::EllipseArc(ea) = out else { panic!("expected ellipse arc") };
        // Backward extension wraps the start param into [0, TAU): it must sit
        // just before the wrap and the sweep must grow past the original π.
        // (The ellipse crosses the r=4.9 circle at param ≈ ±0.2318 of its
        // own origin, so the start lands near TAU − 0.2318.)
        assert!(ea.start_param > PI, "start must extend backward: {}", ea.start_param);
        assert!((ea.start_param - (std::f64::consts::TAU - 0.2318)).abs() < 1e-3,
            "start must land on the backward boundary hit: {}", ea.start_param);
        assert!(ea.sweep_param > PI + 0.2, "sweep must grow backward: {}", ea.sweep_param);
        // Click just AFTER the midpoint: end side must extend.
        let pick2 = point_at(PI / 2.0 + 0.1);
        let out2 = g.extend_to(&[Geom::Circle(Circle { center: Vec2::new(0.0, 0.0), radius: 4.9 })], pick2, false).expect("extend");
        let Geom::EllipseArc(ea2) = out2 else { panic!("expected ellipse arc") };
        assert_eq!(ea2.start_param, 0.0, "start untouched");
        assert!(ea2.sweep_param > PI + 0.2, "sweep must grow forward: {}", ea2.sweep_param);
    }
}

#[cfg(test)]
mod split_at_g3_tests {
    use super::*;

    // G3: `f64::clamp` panics when min > max. A near-zero-sweep arc / ellipse-arc
    // must return Err, not panic (reachable by splitting a degenerate curve).
    #[test]
    fn tiny_arc_split_errs_no_panic() {
        let g = Geom::Arc(Arc {
            center: Vec2::new(0.0, 0.0), radius: 5.0,
            start_angle: 0.0, sweep_angle: 1e-10,
        });
        assert!(g.split_at(Vec2::new(5.0, 0.0)).is_err());
    }

    #[test]
    fn tiny_ellipse_arc_split_errs_no_panic() {
        let g = Geom::EllipseArc(EllipseArc {
            ellipse: Ellipse {
                center: Vec2::new(0.0, 0.0), major: Vec2::new(5.0, 0.0), ratio: 0.5,
            },
            start_param: 0.0, sweep_param: 1e-10,
        });
        assert!(g.split_at(Vec2::new(5.0, 0.0)).is_err());
    }

    #[test]
    fn normal_arc_still_splits_into_two_summing_to_original() {
        use std::f64::consts::PI;
        let g = Geom::Arc(Arc {
            center: Vec2::new(0.0, 0.0), radius: 5.0,
            start_angle: 0.0, sweep_angle: PI,
        });
        let (a, b) = g.split_at(Vec2::new(0.0, 5.0)).expect("a PI arc must split");
        let (sa, sb) = match (a, b) {
            (Geom::Arc(a), Geom::Arc(b)) => (a.sweep_angle, b.sweep_angle),
            _ => panic!("expected two arcs"),
        };
        assert!(sa > EPS && sb > EPS, "both halves must be non-degenerate");
        assert!((sa + sb - PI).abs() < 1e-6, "sweeps must sum to the original");
    }
}


#[cfg(test)]
mod break_tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn polyline_split_on_arc_segment_keeps_curvature() {
        // Issue #19 — a 4-vertex polyline whose FIRST segment is a bulge=1
        // semicircle (center (5,0), r=5). Splitting exactly at the arc's
        // top (5,5) must produce two pieces whose leading segments are still
        // ARCS (bulge ≈ tan(π/8) each), not flattened straight lines.
        let g = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 1.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(20.0, 10.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        });
        // The positive-bulge semicircle runs CCW from (0,0) to (10,0) about
        // center (5,0) — through (5,-5), NOT (5,5). Split at its bottom.
        let (p1, p2) = g.split_at(Vec2::new(5.0, -5.0)).expect("arc split");
        let (Geom::Polyline(a), Geom::Polyline(b)) = (p1, p2) else {
            panic!("expected two polylines") };
        // Piece 1: (0,0) → (5,-5); its only segment is a quarter arc.
        assert_eq!(a.vertices.len(), 2);
        let expect_bulge = (PI / 8.0).tan();
        assert!((a.vertices[0].bulge - expect_bulge).abs() < 1e-9,
            "first half must stay an arc: {}", a.vertices[0].bulge);
        assert_eq!(a.vertices[1].pos, Vec2::new(5.0, -5.0));
        // Piece 2: (5,-5) → (10,0) → (10,10) → (20,10); foot's leading
        // bulge is the other quarter arc; the rest stay straight.
        assert_eq!(b.vertices.len(), 4);
        assert_eq!(b.vertices[0].pos, Vec2::new(5.0, -5.0));
        assert!((b.vertices[0].bulge - expect_bulge).abs() < 1e-9,
            "second half must stay an arc: {}", b.vertices[0].bulge);
        assert_eq!(b.vertices[1].pos, Vec2::new(10.0, 0.0));
        assert_eq!(b.vertices[2].bulge, 0.0);
        assert_eq!(b.vertices[3].bulge, 0.0);
        // The two quarter arcs must lie ON the original semicircle: the
        // midpoints sit at angles 5π/4 and 7π/4 — radius 5 from (5,0).
        let mid1 = Vec2::new(5.0 + 5.0 * (5.0 * PI / 4.0).cos(),
                             5.0 * (5.0 * PI / 4.0).sin());
        let mid2 = Vec2::new(5.0 + 5.0 * (7.0 * PI / 4.0).cos(),
                             5.0 * (7.0 * PI / 4.0).sin());
        assert!((mid1.dist(Vec2::new(5.0, 0.0)) - 5.0).abs() < 1e-9);
        assert!((mid2.dist(Vec2::new(5.0, 0.0)) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn two_point_break_on_circle_removes_ccw_arc() {
        let g = Geom::Circle(Circle { center: Vec2::new(0.0, 0.0), radius: 5.0 });
        // p1 at angle 0, p2 at angle π/2: the CCW arc 0→π/2 is removed, so
        // the result is the arc from π/2 CCW back to 0 (sweep 3π/2).
        let out = g.break_two(Vec2::new(5.0, 0.0), Vec2::new(0.0, 5.0)).expect("break");
        let Geom::Arc(a) = out else { panic!("circle break must yield an arc") };
        assert!((a.start_angle - PI / 2.0).abs() < 1e-9);
        assert!((a.sweep_angle - 3.0 * PI / 2.0).abs() < 1e-9);
        assert_eq!(a.radius, 5.0);
        assert_eq!(a.center, Vec2::new(0.0, 0.0));
        // A full-circle removal (coincident points) is rejected.
        assert!(g.break_two(Vec2::new(5.0, 0.0), Vec2::new(5.0, 0.0)).is_err());
    }

    #[test]
    fn two_point_break_on_ellipse_yields_ellipse_arc() {
        let g = Geom::Ellipse(Ellipse {
            center: Vec2::new(0.0, 0.0), major: Vec2::new(5.0, 0.0), ratio: 0.5,
        });
        let out = g.break_two(Vec2::new(5.0, 0.0), Vec2::new(0.0, 2.5)).expect("break");
        let Geom::EllipseArc(ea) = out else { panic!("ellipse break must yield an ellipse arc") };
        assert!((ea.start_param - PI / 2.0).abs() < 1e-9);
        assert!((ea.sweep_param - 3.0 * PI / 2.0).abs() < 1e-9);
        // Non-closed geoms reject two-point breaks.
        let line = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        assert!(line.break_two(Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)).is_err());
    }

    /// Issue #21 — spline trim via knot insertion: a line cutter crossing the
    /// spline trims it at the parameter-space intersection; the kept half
    /// still passes through the cutter crossing and matches the original
    /// curve on its side.
    #[test]
    fn spline_trim_as_target_via_knot_insertion() {
        // A cubic S-curve crossing a vertical line cutter twice.
        let sp = Geom::Spline(crate::geom::Spline::new_bspline(3, vec![
            Vec2::new(0.0, 0.0), Vec2::new(3.0, 6.0),
            Vec2::new(7.0, -4.0), Vec2::new(10.0, 2.0),
        ]));
        let cutter = Geom::Line(Line { a: Vec2::new(5.0, -10.0), b: Vec2::new(5.0, 10.0) });
        // Click on the LEFT part of the curve (x < 5).
        let pieces = sp.trim_at(&[cutter], Vec2::new(1.0, 1.0), false)
            .expect("spline trims against a line cutter");
        assert_eq!(pieces.len(), 1, "one kept half");
        let Geom::Spline(kept) = &pieces[0] else { panic!("kept piece must stay a spline") };
        assert!(kept.knots.is_some(), "trimmed splines carry explicit knots");
        // The kept half is the LEFT side: its far end sits at the cutter
        // crossing (x ≈ 5) and its near end at the original start.
        let samples = kept.tessellate(64);
        let (start, end) = (samples[0], samples[samples.len() - 1]);
        assert!((start - Vec2::new(0.0, 0.0)).len() < 1e-6,
            "kept half starts at the original start");
        assert!((end.x - 5.0).abs() < 0.2, "kept half ends at the cutter (x={})", end.x);
        // Every sample of the kept half lies on the ORIGINAL curve (left of
        // the cut).
        let Geom::Spline(orig_sp) = &sp else { unreachable!() };
        let orig = orig_sp.tessellate(128);
        for s in &samples {
            let mut best = f64::INFINITY;
            for o in orig.iter() { best = best.min((*o - *s).len()); }
            assert!(best < 0.1, "kept-half point off the original curve");
        }
    }

    /// Issue #21 — spline split_at: the two halves re-join into the original
    /// curve (endpoint equality at the split + sampling agreement).
    #[test]
    fn spline_split_at_halves_match_original() {
        let sp = Geom::Spline(crate::geom::Spline::new_bspline(3, vec![
            Vec2::new(0.0, 0.0), Vec2::new(3.0, 6.0),
            Vec2::new(7.0, -4.0), Vec2::new(10.0, 2.0),
        ]));
        let split_at = Vec2::new(4.2, 0.5);
        let (a, b) = sp.split_at(split_at).expect("spline splits");
        let Geom::Spline(sa) = a else { panic!("left half") };
        let Geom::Spline(sb) = b else { panic!("right half") };
        let aa = sa.tessellate(48);
        let bb = sb.tessellate(48);
        // Halves meet: last left sample ≈ first right sample ≈ C(u).
        assert!((aa[aa.len() - 1] - bb[0]).len() < 1e-6, "halves meet at the split");
        // The split point is the curve point nearest the click.
        let Geom::Spline(orig_sp) = &sp else { unreachable!() };
        let orig = orig_sp.tessellate(128);
        let mut best = f64::INFINITY;
        for o in orig.iter() { best = best.min((*o - split_at).len()); }
        assert!((aa[aa.len() - 1] - split_at).len() < best + 1.0,
            "split lands on the curve");
        // Left half starts at the original start; right ends at the original end.
        assert!((aa[0] - Vec2::new(0.0, 0.0)).len() < 1e-6);
        assert!((bb[bb.len() - 1] - Vec2::new(10.0, 2.0)).len() < 1e-6);
    }

    /// Issue #21 — spline extend: the picked end extends along its tangent to
    /// the boundary; the endpoint moves outward, the near end stays put.
    #[test]
    fn spline_extend_to_boundary_along_tangent() {
        let sp = Geom::Spline(crate::geom::Spline::new_bspline(3, vec![
            Vec2::new(0.0, 0.0), Vec2::new(2.0, 2.0),
            Vec2::new(4.0, -1.0), Vec2::new(6.0, 1.0),
        ]));
        let boundary = Geom::Line(Line { a: Vec2::new(10.0, -5.0), b: Vec2::new(10.0, 5.0) });
        let out = sp.extend_to(&[boundary], Vec2::new(6.0, 1.0), false)
            .expect("spline extends to the boundary");
        let Geom::Spline(s2) = out else { panic!("extended spline stays a spline") };
        let samples = s2.tessellate(64);
        let end = samples[samples.len() - 1];
        assert!((end.x - 10.0).abs() < 0.5,
            "extended end reaches the boundary (x={})", end.x);
        assert!((samples[0] - Vec2::new(0.0, 0.0)).len() < 1e-6,
            "the near end does not move");
    }
}

#[cfg(test)]
mod spline_trim_tests {
    use super::*;
}






#[cfg(test)]
mod issue18_tests {
    use super::*;
    use crate::geom::PolyVertex;
    use crate::join::{arc_from_bulge, bulge_from_arc};

    /// A closed 4-segment square: (0,0) (10,0) (10,10) (0,10). The RIGHT
    /// edge (10,0)→(10,10) carries a bulge arc (bulge on its start vertex).
    fn closed_square_with_arc() -> Polyline {
        Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.5 },
                PolyVertex { pos: Vec2::new(0.0, 10.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        }
    }

    // #18: a CLOSED polyline with a bulge ARC segment trims to ONE open
    // polyline that keeps the arc — never standalone Line/Arc pieces.
    #[test]
    fn closed_bulge_polyline_stays_one_polyline() {
        let pl = Geom::Polyline(closed_square_with_arc());
        // Vertical cutter through the middle, pick on the right (arc) side.
        let cutters = vec![Geom::Line(Line {
            a: Vec2::new(5.0, -2.0), b: Vec2::new(5.0, 12.0),
        })];
        let res = pl.trim_at(&cutters, Vec2::new(9.0, 5.0), false).expect("trim");
        // ONE survivor polyline.
        assert_eq!(res.len(), 1, "closed bulge trim must stay one polyline: {res:?}");
        let Geom::Polyline(out) = &res[0] else { panic!("expected polyline") };
        assert!(!out.closed);
        // The arc bulge (0.5) survives on the right edge's start vertex.
        assert!(out.vertices.iter().any(|v| v.bulge.abs() > 0.1),
            "arc bulge must survive: {:?}",
            out.vertices.iter().map(|v| v.bulge).collect::<Vec<_>>());
        // And the geometry still traces through the arc: the survivor's
        // polyline_segments include an Arc that matches the original edge.
        let segs = crate::join::polyline_segments(out);
        assert!(segs.iter().any(|s| matches!(s, Geom::Arc(_))),
            "survivor must keep an arc segment");
    }

    // #18: a CLOSED polyline with per-segment WIDTHS (rectangle with pen
    // width) trims to ONE polyline that keeps its widths.
    #[test]
    fn closed_width_polyline_stays_one_polyline() {
        let pl = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 10.0), bulge: 0.0 },
            ],
            closed: true,
            widths: vec![(0.2, 0.2), (0.2, 0.2), (0.2, 0.2), (0.2, 0.2)],
        });
        let cutters = vec![Geom::Line(Line {
            a: Vec2::new(5.0, -2.0), b: Vec2::new(5.0, 12.0),
        })];
        let res = pl.trim_at(&cutters, Vec2::new(9.0, 5.0), false).expect("trim");
        assert_eq!(res.len(), 1, "closed width trim must stay one polyline: {res:?}");
        let Geom::Polyline(out) = &res[0] else { panic!("expected polyline") };
        assert!(!out.closed);
        assert!(!out.widths.is_empty(), "widths survive");
        assert_eq!(out.widths.len(), out.vertices.len() - 1);
    }

    // #18: trimming a closed ring keeps the OTHER side intact — the arc
    // segment that was NOT clicked stays exactly as drawn.
    #[test]
    fn closed_ring_unclicked_arc_keeps_geometry() {
        let pl = Geom::Polyline(closed_square_with_arc());
        let cutters = vec![Geom::Line(Line {
            a: Vec2::new(5.0, -2.0), b: Vec2::new(5.0, 12.0),
        })];
        // Click the LEFT side — the arc on the right must remain whole.
        let res = pl.trim_at(&cutters, Vec2::new(2.0, 5.0), false).expect("trim");
        let Geom::Polyline(out) = &res[0] else { panic!() };
        // The arc's full 0.5 bulge is present exactly once.
        let bulges: Vec<f64> = out.vertices.iter().map(|v| v.bulge).collect();
        let full = bulges.iter().filter(|&&b| (b - 0.5).abs() < 1e-9).count();
        assert_eq!(full, 1, "the unclicked arc edge keeps its full bulge: {bulges:?}");
    }

    // #18: a CURVED wall (bulge ≠ 0) trims along its ARC centerline — the
    // surviving piece keeps the curvature (bulge ≠ 0) and the cut lands ON
    // the arc, not the straight chord.
    #[test]
    fn curved_wall_trims_along_arc_centerline() {
        let w = Geom::Wall(Wall {
            start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0),
            thickness: 1.0, style: 0, bulge: 0.5,
        });
        let (c, r, a0, sweep) = arc_from_bulge(
            Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), 0.5).unwrap();
        let on_arc = c + Vec2::new(
            (a0 + sweep * 0.5).cos(), (a0 + sweep * 0.5).sin()) * r;
        let cutters = vec![Geom::Line(Line {
            a: Vec2::new(on_arc.x, on_arc.y - 4.0),
            b: Vec2::new(on_arc.x, on_arc.y + 4.0),
        })];
        let res = w.trim_at(&cutters, on_arc, false).expect("trim");
        // The clicked half is removed; the survivor is a CURVED wall whose
        // arc still passes through the same circle.
        assert_eq!(res.len(), 1);
        let Geom::Wall(out) = &res[0] else { panic!("expected wall") };
        assert!(out.is_curved(), "curvature survives: bulge={}", out.bulge);
        // Its sub-arc is a proper part of the ORIGINAL arc: same center,
        // radius, sweep direction, smaller sweep.
        let (c2, r2, _a, sw2) = arc_from_bulge(out.start, out.end, out.bulge)
            .expect("survivor arc");
        assert!((c2 - c).len() < 1e-6, "same center: {c2:?} vs {c:?}");
        assert!((r2 - r).abs() < 1e-6, "same radius: {r2} vs {r}");
        assert!((sw2.abs() - sweep.abs() * 0.5).abs() < 1e-6,
            "half sweep after midpoint cut: {sw2} vs {}", sweep);
        // start sits on x = on_arc.x (the cutter's x).
        assert!((out.start.x - on_arc.x).abs() < 1e-6,
            "cut lands on the cutter: {:?}", out.start);
    }

    // #18: a CURVED wall EXTENDS along its arc — the endpoint walks the
    // circle, not the chord.
    #[test]
    fn curved_wall_extends_along_arc() {
        let w = Geom::Wall(Wall {
            start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0),
            thickness: 1.0, style: 0, bulge: 0.5,
        });
        // Boundary crossing the arc PAST the start end.
        let (c, r, a0, sweep) = arc_from_bulge(
            Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), 0.5).unwrap();
        let past_start = c + Vec2::new(
            (a0 - 0.4).cos(), (a0 - 0.4).sin()) * r;
        let bounds = vec![Geom::Line(Line {
            a: past_start - Vec2::new(1.0, 0.0),
            b: past_start + Vec2::new(1.0, 0.0),
        })];
        // Pick near the START end (extend that side).
        let pick = Vec2::new(0.1, 0.0);
        let res = w.extend_to(&bounds, pick, false);
        let Geom::Wall(out) = res.expect("extend") else { panic!("expected wall") };
        assert!(out.is_curved(), "extended wall stays curved: bulge={}", out.bulge);
        // The extended START moved OUT along the arc — clearly OFF the chord
        // line (y=0), so it walked the circle, not the chord.
        assert!(out.start.y.abs() > 0.1,
            "start walked the arc: {:?}", out.start);
        // ...and the wall's own chord got LONGER (start x < 0).
        assert!(out.start.x < -0.5, "start extended past x=0: {:?}", out.start);
    }

    // #18: a CURVED wall SPLITS into two curved walls at the click.
    #[test]
    fn curved_wall_splits_into_two_curved_pieces() {
        let w = Geom::Wall(Wall {
            start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0),
            thickness: 1.0, style: 0, bulge: 0.5,
        });
        let (c, r, a0, sweep) = arc_from_bulge(
            Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), 0.5).unwrap();
        let mid = c + Vec2::new(
            (a0 + sweep * 0.5).cos(), (a0 + sweep * 0.5).sin()) * r;
        let (g1, g2) = w.split_at(mid).expect("split");
        let Geom::Wall(w1) = g1 else { panic!("wall 1") };
        let Geom::Wall(w2) = g2 else { panic!("wall 2") };
        assert!(w1.is_curved() && w2.is_curved(),
            "both pieces curved: {} / {}", w1.bulge, w2.bulge);
        // The two pieces join at `mid` and together span the original arc.
        assert!((w1.end - w2.start).len() < 1e-6);
        assert!((w1.start - Vec2::new(0.0, 0.0)).len() < 1e-6);
        assert!((w2.end - Vec2::new(10.0, 0.0)).len() < 1e-6);
    }

    // #18 sanity: the arc-bulge helper round-trips a partial arc.
    #[test]
    fn sub_arc_bulge_matches_geometry() {
        let (c, r, a0, sweep) = arc_from_bulge(
            Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), 0.5).unwrap();
        // Take the first QUARTER of the arc: a0 → a0 + sweep/4.
        let p1 = c + Vec2::new((a0 + sweep * 0.25).cos(),
                               (a0 + sweep * 0.25).sin()) * r;
        // The bulge of the sub-chord (0,0)→p1 encodes the SUB sweep.
        let sub = Arc { center: c, radius: r, start_angle: a0, sweep_angle: sweep };
        let bl = sub_arc_bulge(&sub, Vec2::new(0.0, 0.0), p1);
        let (c2, r2, _a2, sw2) = arc_from_bulge(Vec2::new(0.0, 0.0), p1, bl).unwrap();
        assert!((c2 - c).len() < 1e-9);
        assert!((r2 - r).abs() < 1e-9);
        assert!((sw2 - sweep * 0.25).abs() < 1e-9, "sub sweep {sw2} vs {}", sweep * 0.25);
    }
}





#[cfg(test)]
mod issue18_ellipse_offset_test {
    use super::*;
    use crate::geom::Ellipse;

    #[test]
    fn ellipse_offset_polyline_trims_as_one() {
        // The polyline RESULT of an ellipse offset (dense straight samples,
        // closed) must trim to ONE open polyline.
        let el = Ellipse {
            center: Vec2::new(0.0, 0.0),
            major: Vec2::new(10.0, 0.0),
            ratio: 0.5,
        };
        let g = Geom::Ellipse(el).offset(1.0, Vec2::new(15.0, 0.0))
            .expect("offset");
        let Geom::Polyline(pl) = &g else { panic!("expected polyline: {g:?}") };
        assert!(pl.closed);
        assert!(pl.vertices.len() >= 40);
        let cutters = vec![Geom::Line(Line {
            a: Vec2::new(0.0, -12.0), b: Vec2::new(0.0, 12.0),
        })];
        let res = g.trim_at(&cutters, Vec2::new(8.0, 0.0), false).expect("trim");
        assert_eq!(res.len(), 1, "ellipse-offset trim stays one polyline: {res:?}");
        let Geom::Polyline(out) = &res[0] else { panic!("expected polyline") };
        assert!(!out.closed);
        assert!(out.vertices.len() >= 20);
    }
}

#[cfg(test)]
mod issue18_open_cases {
    use super::*;
    use crate::geom::PolyVertex;

    // #18: an OPEN polyline with BOTH widths and a bulge arc segment trims
    // via the connected-run path, keeping the arc bulge + widths.
    #[test]
    fn open_width_bulge_polyline_keeps_both() {
        let pl = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.5 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
            ],
            closed: false,
            widths: vec![(0.3, 0.3), (0.3, 0.3)],
        });
        // The cutter crosses the arc (bulge 0.5 from (0,0)->(10,0) dips to
        // about y=-2.5 at x=5, so the cutter must start below -3).
        let cutters = vec![Geom::Line(Line {
            a: Vec2::new(5.0, -5.0), b: Vec2::new(5.0, 12.0),
        })];
        // Click on the arc's LEFT half: the clicked half is removed; the
        // survivor starts at the cut (5,-2.5) and keeps the arc's SUB-bulge
        // (half sweep ≈ 0.236) + widths.
        let res = pl.trim_at(&cutters, Vec2::new(5.0, -2.0), false).expect("trim");
        assert_eq!(res.len(), 1, "one survivor run: {res:?}");
        let Geom::Polyline(p0) = &res[0] else { panic!("run") };
        assert_eq!(p0.widths.len(), p0.vertices.len() - 1, "widths preserved");
        assert!((p0.vertices[0].pos - Vec2::new(5.0, -2.5)).len() < 1e-6,
            "survivor starts at the cut: {:?}", p0.vertices[0]);
        // Sub-arc bulge: tan(half_sweep/4) where half_sweep ≈ 0.927.
        assert!((p0.vertices[0].bulge - 0.236).abs() < 1e-3,
            "sub-arc bulge: {}", p0.vertices[0].bulge);
    }

    // #18: EXTENDING an open polyline's bulge segment walks the arc — the
    // moved vertex's segment keeps a bulge (recomputed for the longer chord).
    #[test]
    fn extend_open_polyline_bulge_segment() {
        let pl = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.5 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        });
        // Boundary past the arc's free end, INSIDE the arc's circle
        // (center (5,3.75), r=6.25 → max x ≈ 11.25).
        let bounds = vec![Geom::Line(Line {
            a: Vec2::new(11.0, -4.0), b: Vec2::new(11.0, 4.0),
        })];
        // Pick NEAR THE ARC's free end, BELOW the chord so the arc segment
        // wins the nearest-segment race (the straight (10,0)->(10,10) side
        // is parallel to the boundary and can't extend to it).
        let res = pl.extend_to(&bounds, Vec2::new(9.8, -0.3), false);
        let Geom::Polyline(out) = res.expect("extend") else { panic!("polyline") };
        // The free endpoint moved: the first vertex's segment is still an arc
        // (bulge ≠ 0), and the endpoint is no longer (10,0).
        assert!(out.vertices[0].bulge.abs() > 0.1,
            "arc bulge survives extension: {:?}", out.vertices[0].bulge);
        assert!((out.vertices[1].pos - Vec2::new(10.0, 0.0)).len() > 1e-3,
            "endpoint moved: {:?}", out.vertices[1].pos);
        // The moved endpoint sits on the boundary line (x=11).
        assert!((out.vertices[1].pos.x - 11.0).abs() < 1e-6,
            "extended to the boundary: {:?}", out.vertices[1].pos);
    }
}



#[cfg(test)]
mod issue18_rect_tests {
    use super::*;
    use crate::geom::PolyVertex;

    fn rect(w: f64) -> Polyline {
        Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 10.0), bulge: 0.0 },
            ],
            closed: true,
            widths: if w > 0.0 {
                vec![(w, w), (w, w), (w, w), (w, w)]
            } else { Vec::new() },
        }
    }

    // #18: trimming a rectangle (closed polyline, with OR without pen width)
    // keeps ONE polyline — never four separate segments.
    #[test]
    fn rect_trim_stays_one_polyline_with_and_without_width() {
        for w in [0.0, 0.25] {
            let pl = Geom::Polyline(rect(w));
            let cutters = vec![Geom::Line(Line {
                a: Vec2::new(5.0, -2.0), b: Vec2::new(5.0, 12.0),
            })];
            // Click the LEFT edge → the right part (with the arc-free
            // corners) survives as one open polyline.
            let res = pl.trim_at(&cutters, Vec2::new(2.0, 5.0), false)
                .expect("trim");
            assert_eq!(res.len(), 1,
                "rect (w={w}) trim must stay one polyline: {res:?}");
            let Geom::Polyline(out) = &res[0] else { panic!("polyline") };
            assert!(!out.closed);
            assert!(out.vertices.len() >= 4, "full ring except the click side");
            if w > 0.0 {
                assert_eq!(out.widths.len(), out.vertices.len() - 1);
            }
        }
    }

    // #18: trimming a rectangle's clicked CORNER removes only that corner
    // sub-arc — the two adjacent edges keep their vertices.
    #[test]
    fn rect_corner_trim_removes_just_the_corner() {
        let pl = Geom::Polyline(rect(0.0));
        // Two cutters bracketing the bottom-right corner: vertical at x=8,
        // horizontal at y=2. Click the corner region between them.
        let cutters = vec![
            Geom::Line(Line { a: Vec2::new(8.0, -2.0), b: Vec2::new(8.0, 12.0) }),
            Geom::Line(Line { a: Vec2::new(-2.0, 2.0), b: Vec2::new(12.0, 2.0) }),
        ];
        let res = pl.trim_at(&cutters, Vec2::new(9.9, 0.9), false).expect("trim");
        // One open polyline. Vertices are NODES (by design) so the click
        // removes ONLY the sub-edge between the two nearest nodes around
        // the pick: here the bottom part of the right edge (10,0)→(10,2).
        // The survivor keeps the rest — and never contains the REMOVED
        // sub-edge's interior point (10,1).
        assert_eq!(res.len(), 1, "corner trim keeps one polyline: {res:?}");
        let Geom::Polyline(out) = &res[0] else { panic!() };
        assert!(!out.vertices.iter().any(|v|
            (v.pos - Vec2::new(10.0, 1.0)).len() < 1e-6),
            "removed sub-edge interior gone: {:?}",
            out.vertices.iter().map(|v| v.pos).collect::<Vec<_>>());
        // The rest of the ring is intact: bottom from (0,0), top edge, left.
        assert!(out.vertices.iter().any(|v|
            (v.pos - Vec2::new(0.0, 10.0)).len() < 1e-6));
        assert!(out.vertices.iter().any(|v|
            (v.pos - Vec2::new(0.0, 0.0)).len() < 1e-6));
    }
}
