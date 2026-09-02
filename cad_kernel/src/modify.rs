//! modify.rs — OFFSET, FILLET, and CHAMFER commands.
//!
//! Split out of `geom.rs` verbatim (pure code-movement refactor). Contains
//! `Geom::offset`, the private offset helpers, and the fillet/chamfer line
//! operations.

use crate::math::{Vec2, EPS};
use crate::geom::{Arc, Circle, Donut, Ellipse, Geom, Line, PolyVertex, Polyline, Ray, Spline, Wall, Xline};
use crate::join::bulge_arc;

impl Geom {
    /// Return a parallel copy offset by `dist` to the side of `side` (a
    /// world point used to disambiguate which of the two parallel results
    /// to return — the one closer to `side` wins).
    ///
    /// Returns `Err` for Geom types we don't yet offset (Ellipse,
    /// EllipseArc — true offset of an ellipse isn't an ellipse;
    /// Polyline — corner intersection math TBD; Point — no meaningful
    /// offset).
    pub fn offset(&self, dist: f64, side: Vec2) -> Result<Geom, &'static str> {
        if dist.abs() < EPS { return Ok(self.clone()); }
        match self {
            Geom::Line(l) => {
                let d = l.b - l.a;
                let len_sq = d.len_sq();
                if len_sq < EPS { return Err("offset: zero-length line"); }
                let n = d.perp().normalized();
                // Project (side - midpoint) onto n; sign chooses direction.
                let mid = (l.a + l.b) * 0.5;
                let sgn = if (side - mid).dot(n) >= 0.0 { 1.0 } else { -1.0 };
                let shift = n * (dist * sgn);
                Ok(Geom::Line(Line { a: l.a + shift, b: l.b + shift }))
            }
            Geom::Circle(c) => {
                // Side-of-radius — inside or outside.
                let v = side - c.center;
                let outside = v.len() >= c.radius;
                let new_r = if outside { c.radius + dist.abs() } else { c.radius - dist.abs() };
                if new_r <= EPS { return Err("offset: would collapse to a point or smaller"); }
                Ok(Geom::Circle(Circle { center: c.center, radius: new_r }))
            }
            Geom::Arc(a) => {
                let v = side - a.center;
                let outside = v.len() >= a.radius;
                let new_r = if outside { a.radius + dist.abs() } else { a.radius - dist.abs() };
                if new_r <= EPS { return Err("offset: would collapse"); }
                Ok(Geom::Arc(Arc {
                    center: a.center, radius: new_r,
                    start_angle: a.start_angle, sweep_angle: a.sweep_angle,
                }))
            }
            Geom::Ellipse(el) => {
                // True offset of an ellipse is a quartic, NOT an ellipse —
                // so we return a Polyline approximation. Each sample point
                // is offset along its local outward normal; `side` picks
                // inside vs outside. Inward offsets beyond the minimum
                // radius of curvature are rejected (issue #20).
                let pts = offset_ellipse_samples(*el, 0.0, std::f64::consts::TAU,
                                                  dist, side, true)?;
                if pts.len() < 3 { return Err("offset: ellipse degenerate"); }
                Ok(Geom::Polyline(Polyline {
                    vertices: pts.into_iter()
                        .map(|p| PolyVertex { pos: p, bulge: 0.0 })
                        .collect(),
                    closed: true,
                    widths: Vec::new(),
                }))
            }
            Geom::EllipseArc(ea) => {
                // Same polyline approximation, but only over the swept range.
                let end_param = ea.start_param + ea.sweep_param;
                let pts = offset_ellipse_samples(ea.ellipse, ea.start_param,
                                                  end_param, dist, side, false)?;
                if pts.len() < 2 { return Err("offset: ellipse arc degenerate"); }
                Ok(Geom::Polyline(Polyline {
                    vertices: pts.into_iter()
                        .map(|p| PolyVertex { pos: p, bulge: 0.0 })
                        .collect(),
                    closed: false,
                    widths: Vec::new(),
                }))
            }
            Geom::Polyline(p) => offset_polyline(p, dist, side),
            Geom::Point(_) =>
                Err("offset on point is undefined"),
            Geom::Hatch(_) =>
                Err("offset on hatch is undefined (offset the boundary instead)"),
            Geom::Spline(s) => {
                // True parallel of a NURBS is not a NURBS, so (like the ellipse)
                // we sample the curve, shift each sample along its local normal,
                // and return a smooth Polyline approximation. `side` picks the
                // hand. A refit back to a spline is a future refinement.
                let pts = offset_spline_samples(s, dist, side);
                if pts.len() < 2 { return Err("offset: spline degenerate"); }
                Ok(Geom::Polyline(Polyline {
                    vertices: pts.into_iter()
                        .map(|p| PolyVertex { pos: p, bulge: 0.0 })
                        .collect(),
                    closed: false,
                    widths: Vec::new(),
                }))
            }
            Geom::Xline(x) => {
                // Parallel infinite line: shift the base along the normal.
                let n = x.dir.perp();
                let sgn = if (side - x.base).dot(n) >= 0.0 { 1.0 } else { -1.0 };
                Ok(Geom::Xline(Xline {
                    base: x.base + n * (dist * sgn),
                    dir: x.dir,
                }))
            }
            Geom::Ray(r) => {
                // Parallel ray: shift the base along the normal (dir stays).
                let n = r.dir.perp();
                let sgn = if (side - r.base).dot(n) >= 0.0 { 1.0 } else { -1.0 };
                Ok(Geom::Ray(Ray {
                    base: r.base + n * (dist * sgn),
                    dir: r.dir,
                }))
            }
            Geom::Donut(d) => {
                // Offset a donut = grow/shrink the ring (both radii move
                // by `dist`; the ring flips when dist exceeds the outer).
                let sgn = if (side - d.center).dot(Vec2::new(1.0, 0.0)) >= 0.0 { 1.0 } else { -1.0 };
                let delta = dist * sgn;
                let outer = (d.outer_radius + delta).max(1e-9);
                let inner = (d.inner_radius + delta).clamp(0.0, outer);
                Ok(Geom::Donut(Donut::new(d.center, inner, outer)))
            }
            Geom::Wipeout(_) =>
                Err("offset: a wipeout mask cannot be offset"),
            Geom::Region(_) =>
                Err("offset: a region cannot be offset — use a polyline"),
            Geom::Wall(w) => {
                // Offset the centerline; new Wall keeps the same
                // thickness on the offset centerline.
                let g = Geom::Line(w.centerline()).offset(dist, side)?;
                if let Geom::Line(l) = g {
                    Ok(Geom::Wall(Wall {
                        start: l.a, end: l.b, thickness: w.thickness,
                        style: w.style, bulge: 0.0,
                    }))
                } else { Err("offset wall: unexpected non-Line result") }
            }
            Geom::Table(_) =>
                Err("offset on a table is undefined"),
            Geom::Xref(_) =>
                Err("offset: explode the xref first"),
            Geom::Text(_) =>
                Err("offset on text is undefined"),
            Geom::Leader(_) =>
                Err("offset on leader is undefined"),
            Geom::CenterMark(_) =>
                Err("offset on a center mark is undefined"),
            Geom::AttrDef(_) =>
                Err("offset on attribute definition is undefined"),
            Geom::Dimension(_) =>
                Err("offset on dimension is undefined"),
            Geom::BlockRef(_) =>
                Err("offset: explode the block first"),
            Geom::Viewport(_) =>
                Err("offset: viewport is a paper-space entity"),
        }
    }
}

// ---------------------------------------------------------------------------
// Ellipse offset helper.
//
// True parallel offset of an ellipse is a quartic, NOT an ellipse, so we
// approximate by sampling the curve and offsetting each sample along its
// local outward normal. Sign chosen from `side` projected onto the normal
// at the first sample.
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Polyline offset — segment-offset + corner-intersection (AutoCAD OFFSET).
//
// Each segment is offset to ONE consistent hand (left/right of the directed
// polyline, decided from where the user clicked). Straight segments shift
// along their normal; arc (bulge) segments become CONCENTRIC arcs (radius
// ±dist, same swept angle → same bulge). Adjacent straight offsets are
// joined at their true line-line intersection (miter); joints touching an
// arc fall back to the midpoint of the two offset ends (exact for tangent
// joints, close otherwise). Self-intersection trimming is NOT done (matches
// a plain OFFSET — the user trims afterwards if needed).
// ---------------------------------------------------------------------------

/// Point-to-segment distance — used only to find the nearest segment when
/// resolving which hand the click is on (and by the spline trim/split arms
/// in `trim.rs` for parameter projection).
pub(crate) fn point_seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.len_sq();
    if l2 < EPS { return p.dist(a); }
    let t = ((p - a).dot(d) / l2).clamp(0.0, 1.0);
    p.dist(a + d * t)
}

/// Intersection of the infinite lines (p0 + t·d0) and (p1 + s·d1). `None`
/// when (near-)parallel.
fn line_line_inf(p0: Vec2, d0: Vec2, p1: Vec2, d1: Vec2) -> Option<Vec2> {
    let denom = d0.x * d1.y - d0.y * d1.x;
    if denom.abs() < 1e-12 { return None; }
    let dp = p1 - p0;
    let t = (dp.x * d1.y - dp.y * d1.x) / denom;
    Some(p0 + d0 * t)
}

/// True when segments a0→a1 and b0→b1 cross at an interior point of BOTH
/// (a "proper" crossing). Shared/touching endpoints and collinear overlaps
/// return false — only a genuine X counts. Used to catch a self-intersecting
/// (bow-tie) offset polyline.
fn segs_cross(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> bool {
    let d1 = a1 - a0;
    let d2 = b1 - b0;
    let denom = d1.x * d2.y - d1.y * d2.x;
    if denom.abs() < 1e-12 { return false; } // parallel / collinear
    let dp = b0 - a0;
    let t = (dp.x * d2.y - dp.y * d2.x) / denom;
    let u = (dp.x * d1.y - dp.y * d1.x) / denom;
    const E: f64 = 1e-9;
    t > E && t < 1.0 - E && u > E && u < 1.0 - E
}

fn offset_polyline(p: &Polyline, dist: f64, side: Vec2) -> Result<Geom, &'static str> {
    let v = &p.vertices;
    let n = v.len();
    if n < 2 { return Err("offset: polyline needs ≥ 2 vertices"); }
    let seg_count = if p.closed { n } else { n - 1 };
    let amt = dist.abs();

    // --- 1. global hand from the nearest segment's chord ---
    let mut best = (f64::INFINITY, 0usize);
    for i in 0..seg_count {
        let a = v[i].pos;
        let b = v[(i + 1) % n].pos;
        let d = point_seg_dist(side, a, b);
        if d < best.0 { best = (d, i); }
    }
    let na = v[best.1].pos;
    let nb = v[(best.1 + 1) % n].pos;
    if (nb - na).len() < EPS { return Err("offset: zero-length polyline segment"); }
    let left = (nb - na).perp().normalized();
    let mid  = (na + nb) * 0.5;
    let hand = if (side - mid).dot(left) >= 0.0 { 1.0 } else { -1.0 };
    let off  = amt * hand;   // signed offset toward the clicked hand (+ = left)

    // --- 2. per-segment offset geometry ---
    struct OffSeg { a: Vec2, b: Vec2, bulge: f64, line: bool, dir: Vec2 }
    let mut segs: Vec<OffSeg> = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let a = v[i].pos;
        let b = v[(i + 1) % n].pos;
        let bulge = v[i].bulge;
        if bulge.abs() < 1e-9 {
            let dir = b - a;
            if dir.len() < EPS { return Err("offset: zero-length polyline segment"); }
            let shift = dir.perp().normalized() * off;
            segs.push(OffSeg { a: a + shift, b: b + shift, bulge: 0.0,
                               line: true, dir: dir.normalized() });
        } else {
            let Some((c, r, _sa, sweep)) = bulge_arc(a, b, bulge) else {
                return Err("offset: degenerate arc segment");
            };
            let s_arc = if sweep >= 0.0 { 1.0 } else { -1.0 };
            let rp = r - hand * s_arc * amt;   // concentric radius
            if rp <= EPS { return Err("offset: arc segment collapses"); }
            let scale = rp / r;
            segs.push(OffSeg {
                a: c + (a - c) * scale,
                b: c + (b - c) * scale,
                bulge,                 // same swept angle → same bulge
                line: false, dir: Vec2::ZERO,
            });
        }
    }

    // --- 3. join into a vertex list ---
    let join = |s0: &OffSeg, s1: &OffSeg| -> Vec2 {
        if s0.line && s1.line {
            if let Some(p) = line_line_inf(s0.a, s0.dir, s1.a, s1.dir) { return p; }
        }
        (s0.b + s1.a) * 0.5
    };
    let mut out: Vec<PolyVertex> = Vec::with_capacity(n);
    if p.closed {
        for i in 0..seg_count {
            let prev = (i + seg_count - 1) % seg_count;
            out.push(PolyVertex { pos: join(&segs[prev], &segs[i]), bulge: segs[i].bulge });
        }
    } else {
        out.push(PolyVertex { pos: segs[0].a, bulge: segs[0].bulge });
        for i in 1..seg_count {
            out.push(PolyVertex { pos: join(&segs[i - 1], &segs[i]), bulge: segs[i].bulge });
        }
        out.push(PolyVertex { pos: segs[seg_count - 1].b, bulge: 0.0 });
    }

    // --- 4. reject a self-crossing / collapsed CLOSED offset -----------
    // When an inward offset is larger than the shape can absorb, adjacent
    // segment joins cross over: the polygon inverts (edges reverse) or
    // self-intersects (a bow-tie). Either way the result is garbage, so
    // refuse it and tell the user to reduce the value — instead of silently
    // emitting a tangled shape (owner: "edges cross, reduce the value").
    // Open polylines can't fold onto themselves this way, so only closed
    // ones are checked. `out` has one vertex per segment here.
    if p.closed && out.len() == seg_count && seg_count >= 3 {
        // (a) Direction reversal — each output edge must still run the SAME
        //     way as its source edge. A collapsed segment flips 180°. This
        //     is the common convex case (a pentagon over-offset inward).
        for i in 0..seg_count {
            let src_dir = v[(i + 1) % n].pos - v[i].pos;
            let out_dir = out[(i + 1) % seg_count].pos - out[i].pos;
            if src_dir.len() > EPS && out_dir.len() > EPS
                && out_dir.dot(src_dir) < 0.0
            {
                return Err("edges cross — reduce the offset value");
            }
        }
        // (b) Genuine self-intersection among the STRAIGHT edges — catches
        //     concave bow-ties that reverse no single edge. Arc edges are
        //     skipped (their over-collapse is already caught by the rp≤EPS
        //     guard above). Bounded so it stays cheap on large polylines.
        if seg_count <= 512 {
            let line_edge = |i: usize| -> Option<(Vec2, Vec2)> {
                if segs[i].line {
                    Some((out[i].pos, out[(i + 1) % seg_count].pos))
                } else { None }
            };
            for i in 0..seg_count {
                let Some((a0, a1)) = line_edge(i) else { continue };
                for j in (i + 2)..seg_count {
                    // Skip the adjacent pair that wraps around the closure.
                    if i == 0 && j == seg_count - 1 { continue; }
                    let Some((b0, b1)) = line_edge(j) else { continue };
                    if segs_cross(a0, a1, b0, b1) {
                        return Err("edges cross — reduce the offset value");
                    }
                }
            }
        }
    }

    Ok(Geom::Polyline(Polyline { vertices: out, closed: p.closed, widths: Vec::new() }))
}

fn offset_ellipse_samples(
    el: Ellipse,
    t_start: f64,
    t_end: f64,
    dist: f64,
    side: Vec2,
    closed: bool,
) -> Result<Vec<Vec2>, &'static str> {
    let a = el.semi_major();
    if a < EPS { return Ok(Vec::new()); }
    let b = (a * el.ratio).max(EPS);
    // Sample density scales with size; minimum 64 for visual smoothness.
    let n = (64.0_f64 + a.log10().max(0.0) * 32.0).round().max(48.0) as usize;
    // Sign: compare `side` direction to the CCW-perp tangent at t_start.
    let p0 = el.point_at(t_start);
    let tg0 = el.tangent_at(t_start);
    let nrm0 = tg0.perp();
    let nl   = nrm0.len();
    if nl < EPS { return Ok(Vec::new()); }
    let n0u  = nrm0 / nl;
    let sgn  = if (side - p0).dot(n0u) >= 0.0 { 1.0 } else { -1.0 };
    let amt  = dist.abs();
    // The CCW-perp of the tangent points toward the loop's INSIDE (the
    // existing convention: a click outside the ellipse yields sgn = -1 and
    // the samples push outward). Inward = pushing along the perp.
    let inward = sgn > 0.0;
    let span = t_end - t_start;
    let count = if closed { n } else { n + 1 };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = t_start + (i as f64 / n as f64) * span;
        // Issue #20 — an INWARD offset beyond the local radius of curvature
        // flips the offset normal: the parallel crosses itself (the "weird
        // self-intersecting shapes" report). Radius of curvature of an
        // ellipse at parameter t: ρ = (a²sin²t + b²cos²t)^(3/2) / (ab);
        // the minimum over the FULL ellipse is b²/a at the minor-axis ends.
        // Checking per sample lets arcs that never reach the high-curvature
        // region offset further than a full ellipse would allow.
        if inward {
            let s2 = t.sin().powi(2);
            let c2 = t.cos().powi(2);
            let rho = (a * a * s2 + b * b * c2).powf(1.5) / (a * b);
            if amt > rho - 1e-9 {
                return Err(
                    "offset beyond the curve's radius of curvature — reduce the value");
            }
        }
        let p = el.point_at(t);
        let tg = el.tangent_at(t);
        let nm = tg.perp();
        let nl = nm.len();
        if nl < EPS { continue; }
        out.push(p + (nm / nl) * amt * sgn);
    }
    Ok(out)
}

/// Offset a NURBS spline by sampling it and shifting each sample along its
/// local normal (perp of the finite-difference tangent). The true parallel of
/// a NURBS is not a NURBS, so — like the ellipse — the result is a Polyline
/// approximation. `side` picks inside/outside via the normal at the first
/// sample, so the whole offset stays on one hand.
fn offset_spline_samples(sp: &Spline, dist: f64, side: Vec2) -> Vec<Vec2> {
    // Sample density scales with the control-polygon span; min 96 for a smooth
    // result at typical zoom, capped so huge splines stay cheap.
    let (mn, mx) = sp.bbox();
    let span = (mx - mn).len().max(1.0);
    let n = (96.0 + span.log10().max(0.0) * 48.0).round().clamp(96.0, 512.0) as usize;
    let pts = sp.tessellate(n);
    if pts.len() < 2 { return Vec::new(); }
    // Central-difference tangent (clamped at the ends).
    let tangent = |i: usize| -> Vec2 {
        let a = if i == 0 { pts[0] } else { pts[i - 1] };
        let b = if i + 1 >= pts.len() { pts[pts.len() - 1] } else { pts[i + 1] };
        b - a
    };
    // Sign from the FIRST sample so every point offsets to the same hand.
    let n0  = tangent(0).perp();
    let nl0 = n0.len();
    if nl0 < EPS { return Vec::new(); }
    let sgn = if (side - pts[0]).dot(n0 / nl0) >= 0.0 { 1.0 } else { -1.0 };
    let mut out = Vec::with_capacity(pts.len());
    for i in 0..pts.len() {
        let nm = tangent(i).perp();
        let nl = nm.len();
        if nl < EPS { continue; }
        out.push(pts[i] + (nm / nl) * (dist.abs() * sgn));
    }
    out
}

// ---------------------------------------------------------------------------
// Fillet / Chamfer — Slices M.3 + M.4.
//
// v1 supports LINE + LINE only. Other combinations return Err; line-arc and
// arc-arc are deferred to v2 per the user's scoping decision.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct FilletOut {
    /// L1 trimmed back to the tangent point on its kept side.
    pub g1_new: Geom,
    /// L2 trimmed back to the tangent point on its kept side.
    pub g2_new: Geom,
    /// The fillet arc itself. `None` for radius 0 — the two lines just meet
    /// at the corner intersection.
    pub arc: Option<Geom>,
}

/// Fillet two lines with `radius`. `p1` / `p2` are the user's click points
/// on each line — they determine which SIDE of each line is kept (the side
/// nearest the click). Radius 0 produces a sharp corner.
pub fn fillet_lines(
    l1: &Line, p1: Vec2,
    l2: &Line, p2: Vec2,
    radius: f64,
) -> Result<FilletOut, &'static str> {
    if radius < 0.0 { return Err("fillet: radius must be ≥ 0"); }
    let d1v = l1.b - l1.a;
    let d2v = l2.b - l2.a;
    let len1 = d1v.len();
    let len2 = d2v.len();
    if len1 < EPS || len2 < EPS { return Err("fillet: zero-length line"); }

    // Infinite-line intersection.
    let cross = d1v.x * d2v.y - d1v.y * d2v.x;
    if cross.abs() < EPS { return Err("fillet: lines are parallel"); }
    let dx = l2.a.x - l1.a.x;
    let dy = l2.a.y - l1.a.y;
    let t1 = (dx * d2v.y - dy * d2v.x) / cross;
    let i_pt = l1.a + d1v * t1;

    // Unit directions from I toward each pick.
    let u1 = d1v / len1;
    let u2 = d2v / len2;
    let dir1 = if (p1 - i_pt).dot(u1) >= 0.0 { u1 } else { -u1 };
    let dir2 = if (p2 - i_pt).dot(u2) >= 0.0 { u2 } else { -u2 };

    // Corner angle at I (between the two kept-side outgoing rays).
    let cos_th = dir1.dot(dir2).clamp(-1.0, 1.0);
    let theta = cos_th.acos();
    if theta < 1e-6 || (std::f64::consts::PI - theta) < 1e-6 {
        return Err("fillet: lines are collinear at the corner");
    }

    // Endpoint of each line to KEEP — the one on the same side of I as the
    // click. We compare projections onto dirN; whichever endpoint projects
    // further along dirN from I is the kept endpoint.
    let endpoint_of_l1 = if (l1.a - i_pt).dot(dir1) > (l1.b - i_pt).dot(dir1) { l1.a } else { l1.b };
    let endpoint_of_l2 = if (l2.a - i_pt).dot(dir2) > (l2.b - i_pt).dot(dir2) { l2.a } else { l2.b };

    if radius < EPS {
        // Sharp corner: both lines run from their kept endpoint to I.
        return Ok(FilletOut {
            g1_new: Geom::Line(Line { a: endpoint_of_l1, b: i_pt }),
            g2_new: Geom::Line(Line { a: i_pt, b: endpoint_of_l2 }),
            arc: None,
        });
    }

    // Tangent-point distance along each kept-side ray: t = r / tan(θ/2).
    let tan_half = (theta / 2.0).tan();
    let t = radius / tan_half;
    // The kept segment must be long enough to reach the tangent point.
    let kept_len1 = (endpoint_of_l1 - i_pt).len();
    let kept_len2 = (endpoint_of_l2 - i_pt).len();
    if kept_len1 < t || kept_len2 < t {
        return Err("fillet: radius too large for these lines");
    }

    let tp1 = i_pt + dir1 * t;
    let tp2 = i_pt + dir2 * t;

    // Arc center: bisector direction from I, distance r/sin(θ/2).
    let bis_raw = dir1 + dir2;
    let bis = bis_raw / bis_raw.len();
    let center = i_pt + bis * (radius / (theta / 2.0).sin());

    // The fillet arc is the MINOR arc between the two tangent points
    // (central angle π − θ). Because I lies OUTSIDE the circle (its distance
    // r/sin(θ/2) > r), the minor arc is the one whose chord faces I — i.e.
    // it always bulges toward the corner vertex, giving the rounded inside
    // corner. We render arcs CCW (positive sweep), so the only decision is
    // which tangent point is the START such that a CCW sweep of π − θ stays
    // on that minor arc:
    //   d_ccw = CCW angle from tp1 to tp2. If d_ccw ≤ π the minor arc runs
    //   CCW from tp1; otherwise it runs CCW from tp2 (CW from tp1), so start
    //   there. sweep is always the minor magnitude π − θ.
    //
    // (The previous heuristic rotated v1 toward the I-direction and accepted
    // on `dot > 0` — a 90°-wide window that mis-fired for non-right corners,
    // e.g. θ = 120°, rendering the arc sweeping the wrong way out of the
    // corner. It only happened to be correct at θ = 90°.)
    let arc_angle = std::f64::consts::PI - theta;
    let v1 = tp1 - center;
    let v2 = tp2 - center;
    let a1 = v1.angle().rem_euclid(std::f64::consts::TAU);
    let a2 = v2.angle().rem_euclid(std::f64::consts::TAU);
    let d_ccw = (a2 - a1).rem_euclid(std::f64::consts::TAU);
    let start_angle = if d_ccw <= std::f64::consts::PI { a1 } else { a2 };
    let arc = Geom::Arc(Arc {
        center,
        radius,
        start_angle,
        sweep_angle: arc_angle,
    });

    Ok(FilletOut {
        g1_new: Geom::Line(Line { a: endpoint_of_l1, b: tp1 }),
        g2_new: Geom::Line(Line { a: tp2, b: endpoint_of_l2 }),
        arc: Some(arc),
    })
}

#[derive(Clone, Debug)]
pub struct ChamferOut {
    pub g1_new: Geom,
    pub g2_new: Geom,
    /// The chamfer line connecting the two tangent points. `None` when both
    /// distances are ~0 — the two objects just meet at the sharp corner and the
    /// bridge would be a zero-length line (mirrors `FilletOut::arc == None` for
    /// radius 0). Consumers must skip a `None` bridge.
    pub bridge: Option<Geom>,
}

/// Chamfer two lines with distances `d1` along L1 and `d2` along L2 from the
/// intersection. Same click-side convention as fillet.
pub fn chamfer_lines(
    l1: &Line, p1: Vec2,
    l2: &Line, p2: Vec2,
    d1: f64, d2: f64,
) -> Result<ChamferOut, &'static str> {
    if d1 < 0.0 || d2 < 0.0 { return Err("chamfer: distances must be ≥ 0"); }
    let d1v = l1.b - l1.a;
    let d2v = l2.b - l2.a;
    let len1 = d1v.len();
    let len2 = d2v.len();
    if len1 < EPS || len2 < EPS { return Err("chamfer: zero-length line"); }

    let cross = d1v.x * d2v.y - d1v.y * d2v.x;
    if cross.abs() < EPS { return Err("chamfer: lines are parallel"); }
    let dx = l2.a.x - l1.a.x;
    let dy = l2.a.y - l1.a.y;
    let t1 = (dx * d2v.y - dy * d2v.x) / cross;
    let i_pt = l1.a + d1v * t1;

    let u1 = d1v / len1;
    let u2 = d2v / len2;
    let dir1 = if (p1 - i_pt).dot(u1) >= 0.0 { u1 } else { -u1 };
    let dir2 = if (p2 - i_pt).dot(u2) >= 0.0 { u2 } else { -u2 };

    let endpoint_of_l1 = if (l1.a - i_pt).dot(dir1) > (l1.b - i_pt).dot(dir1) { l1.a } else { l1.b };
    let endpoint_of_l2 = if (l2.a - i_pt).dot(dir2) > (l2.b - i_pt).dot(dir2) { l2.a } else { l2.b };

    let kept_len1 = (endpoint_of_l1 - i_pt).len();
    let kept_len2 = (endpoint_of_l2 - i_pt).len();
    if kept_len1 < d1 || kept_len2 < d2 {
        return Err("chamfer: distance exceeds available line length");
    }

    let tp1 = i_pt + dir1 * d1;
    let tp2 = i_pt + dir2 * d2;
    // A zero-length bridge (both distances ~0 → tp1 == tp2 == corner) is a
    // clean sharp corner, not a chamfer — emit no bridge segment.
    let bridge = if (tp1 - tp2).len() < EPS {
        None
    } else {
        Some(Geom::Line(Line { a: tp1, b: tp2 }))
    };
    Ok(ChamferOut {
        g1_new: Geom::Line(Line { a: endpoint_of_l1, b: tp1 }),
        g2_new: Geom::Line(Line { a: tp2, b: endpoint_of_l2 }),
        bridge,
    })
}
