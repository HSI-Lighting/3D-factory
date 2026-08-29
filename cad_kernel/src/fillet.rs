//! fillet.rs — generalized FILLET / CHAMFER.
//!
//! The original `modify.rs` fillet/chamfer only handled LINE + LINE. This
//! module generalizes the operation to any combination of **line and arc
//! pieces**, which covers:
//!   * bare `Line` ↔ `Line` / `Arc`
//!   * `Arc`  ↔ `Line` / `Arc`
//!   * a `Polyline`'s END segment ↔ a separate Line/Arc/Polyline-end
//!   * two segments of the SAME polyline (the corner between them)
//!   * EVERY corner of one polyline at once (the AutoCAD `P` option)
//!
//! The math is a unified **offset-locus** solver: the centre of a radius-`r`
//! circle tangent to a line lies on one of two parallel offset lines; tangent
//! to a circle/arc it lies on a concentric circle (R±r). Intersecting the
//! loci of the two pieces yields candidate fillet centres; we pick the one
//! sitting INSIDE the corner (on the bisector side) nearest the corner. The
//! tangent points are the feet/projections from that centre onto each piece.
//!
//! Splines and ellipse-arcs are NOT handled here yet (they have no simple
//! offset locus — tessellate-to-polyline first, or add a numerical solver).

use std::f64::consts::{PI, TAU};

use crate::math::{scaled_tol, Vec2, EPS};
use crate::geom::{Arc, Geom, Line, PolyVertex, Polyline};
use crate::join::{bulge_arc, bulge_from_arc};
use crate::modify::{ChamferOut, FilletOut};

// ---------------------------------------------------------------------------
// Piece — a line segment or a circular arc. Every input (Line, Arc, polyline
// segment) reduces to this. Arc `sweep` is SIGNED (+ = CCW) so polyline arc
// segments keep their orientation; for a bare `Geom::Arc` the sweep is the
// stored positive (CCW) value.
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug)]
enum Piece {
    Seg { a: Vec2, b: Vec2 },
    Arc { c: Vec2, r: f64, a0: f64, sweep: f64 },
}

impl Piece {
    fn endpoints(&self) -> (Vec2, Vec2) {
        match *self {
            Piece::Seg { a, b } => (a, b),
            Piece::Arc { c, r, a0, sweep } => {
                let s = c + Vec2::new(r * a0.cos(), r * a0.sin());
                let e = c + Vec2::new(r * (a0 + sweep).cos(), r * (a0 + sweep).sin());
                (s, e)
            }
        }
    }

    /// Tangent point on this piece from a fillet centre `center` of radius
    /// `r`. For a segment that's the perpendicular foot; for an arc the
    /// radial projection (whichever of the two radial points is `r` away).
    fn tangent_point(&self, center: Vec2, r: f64) -> Option<Vec2> {
        match *self {
            Piece::Seg { a, b } => {
                let d = b - a;
                let dl = d.len();
                if dl < EPS { return None; }
                let u = d * (1.0 / dl);
                let t = (center - a).dot(u);
                Some(a + u * t)
            }
            Piece::Arc { c, r: rr, .. } => {
                let dir = center - c;
                let dl = dir.len();
                if dl < EPS { return None; }
                let u = dir * (1.0 / dl);
                let t1 = c + u * rr;
                let t2 = c - u * rr;
                let cand = if ((t1 - center).len() - r).abs()
                    <= ((t2 - center).len() - r).abs() { t1 } else { t2 };
                Some(cand)
            }
        }
    }

    /// True if point `p` (assumed on the piece's host line/circle) lies
    /// within the piece's swept extent (with a tiny tolerance).
    fn contains(&self, p: Vec2) -> bool {
        match *self {
            Piece::Seg { a, b } => {
                let d = b - a;
                let l2 = d.len_sq();
                if l2 < EPS { return false; }
                let t = (p - a).dot(d) / l2;
                t >= -1e-6 && t <= 1.0 + 1e-6
            }
            Piece::Arc { c, a0, sweep, .. } => {
                let s = if sweep >= 0.0 { 1.0 } else { -1.0 };
                let dd = (((p - c).angle() - a0) * s).rem_euclid(TAU);
                dd <= sweep.abs() + 1e-6
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Offset loci & intersections.
// ---------------------------------------------------------------------------
#[derive(Clone, Copy)]
enum Locus {
    Line { p: Vec2, d: Vec2 },   // d unit
    Circle { c: Vec2, r: f64 },
}

fn piece_loci(pc: &Piece, r: f64) -> Vec<Locus> {
    match *pc {
        Piece::Seg { a, b } => {
            let d = b - a;
            let dl = d.len();
            if dl < EPS { return Vec::new(); }
            let u = d * (1.0 / dl);
            let n = u.perp();
            vec![
                Locus::Line { p: a + n * r, d: u },
                Locus::Line { p: a - n * r, d: u },
            ]
        }
        Piece::Arc { c, r: rr, .. } => {
            let mut v = vec![Locus::Circle { c, r: rr + r }];
            let inner = (rr - r).abs();
            if inner > EPS { v.push(Locus::Circle { c, r: inner }); }
            v
        }
    }
}

fn loci_intersect(a: &Locus, b: &Locus) -> Vec<Vec2> {
    match (a, b) {
        (Locus::Line { p: p0, d: d0 }, Locus::Line { p: p1, d: d1 }) => {
            line_line(*p0, *d0, *p1, *d1).into_iter().collect()
        }
        (Locus::Line { p, d }, Locus::Circle { c, r }) |
        (Locus::Circle { c, r }, Locus::Line { p, d }) => line_circle(*p, *d, *c, *r),
        (Locus::Circle { c: c0, r: r0 }, Locus::Circle { c: c1, r: r1 }) =>
            circle_circle(*c0, *r0, *c1, *r1),
    }
}

fn line_line(p0: Vec2, d0: Vec2, p1: Vec2, d1: Vec2) -> Option<Vec2> {
    let denom = d0.cross(d1);
    if denom.abs() < 1e-12 { return None; }
    let t = (p1 - p0).cross(d1) / denom;
    Some(p0 + d0 * t)
}

fn line_circle(p: Vec2, d: Vec2, c: Vec2, r: f64) -> Vec<Vec2> {
    // d is unit. Foot of perpendicular from c, then ± along d.
    let f = p + d * ((c - p).dot(d));
    let h2 = r * r - (f - c).len_sq();
    if h2 < -1e-9 { return Vec::new(); }
    let h = h2.max(0.0).sqrt();
    if h < 1e-9 { return vec![f]; }
    vec![f + d * h, f - d * h]
}

fn circle_circle(c0: Vec2, r0: f64, c1: Vec2, r1: f64) -> Vec<Vec2> {
    let dv = c1 - c0;
    let dist = dv.len();
    if dist < 1e-9 { return Vec::new(); }
    if dist > r0 + r1 + 1e-9 || dist < (r0 - r1).abs() - 1e-9 { return Vec::new(); }
    let a = (r0 * r0 - r1 * r1 + dist * dist) / (2.0 * dist);
    let h2 = r0 * r0 - a * a;
    let mid = c0 + dv * (a / dist);
    if h2 <= 1e-12 { return vec![mid]; }
    let h = h2.sqrt();
    let perp = dv.perp() * (1.0 / dist);
    vec![mid + perp * h, mid - perp * h]
}

/// Geometric intersection of two pieces' host line/circle (ignoring extent),
/// returning all candidate points.
fn piece_intersect(p1: &Piece, p2: &Piece) -> Vec<Vec2> {
    match (p1, p2) {
        (Piece::Seg { a, b }, Piece::Seg { a: a2, b: b2 }) => {
            let d0 = *b - *a; let d1 = *b2 - *a2;
            line_line(*a, d0, *a2, d1).into_iter().collect()
        }
        (Piece::Seg { a, b }, Piece::Arc { c, r, .. }) |
        (Piece::Arc { c, r, .. }, Piece::Seg { a, b }) => {
            let d = (*b - *a).normalized();
            line_circle(*a, d, *c, *r)
        }
        (Piece::Arc { c: c0, r: r0, .. }, Piece::Arc { c: c1, r: r1, .. }) =>
            circle_circle(*c0, *r0, *c1, *r1),
    }
}

// ---------------------------------------------------------------------------
// Core solver.
// ---------------------------------------------------------------------------

/// Solve for the fillet centre + the two tangent points. `corner` is the
/// junction vertex (used to pick the right fillet centre); `keep1`/`keep2`
/// are points on the side of each piece that should survive (used only to
/// orient the corner bisector).
fn solve_fillet(
    p1: &Piece, keep1: Vec2,
    p2: &Piece, keep2: Vec2,
    r: f64, corner: Vec2,
) -> Option<(Vec2, Vec2, Vec2)> {
    let l1 = piece_loci(p1, r);
    let l2 = piece_loci(p2, r);
    let bis_raw = (keep1 - corner).normalized() + (keep2 - corner).normalized();
    let bis = if bis_raw.len() > EPS { bis_raw.normalized() } else { bis_raw };

    let mut best: Option<(f64, Vec2, Vec2, Vec2)> = None;
    let mut best_any: Option<(f64, Vec2, Vec2, Vec2)> = None;
    for la in &l1 {
        for lb in &l2 {
            for center in loci_intersect(la, lb) {
                let (Some(tp1), Some(tp2)) =
                    (p1.tangent_point(center, r), p2.tangent_point(center, r))
                else { continue };
                // G4: accept the tangent points with a radius-relative tolerance
                // — `(tp - center).len()` carries noise ~ r·1e-16, so a fixed
                // 1e-6 rejected valid fillets at large radius.
                if ((tp1 - center).len() - r).abs() > scaled_tol(r) { continue; }
                if ((tp2 - center).len() - r).abs() > scaled_tol(r) { continue; }
                // Reject a DEGENERATE fillet whose circle coincides with one of
                // the input ARCS. When the fillet radius ≈ an arc's radius, that
                // arc's inner offset locus |rr−r| collapses onto the arc's own
                // centre, so a candidate centre lands on the arc centre and the
                // "fillet" is just the arc itself (owner dump: filleting an arc
                // of r=6.841 with fillet r=6.8415 returned the arc unchanged).
                let coincides_with_arc = |pc: &Piece| -> bool {
                    if let Piece::Arc { c, r: rr, .. } = *pc {
                        let tol = 1e-3 * r.max(1.0);
                        (center - c).len() < tol && (r - rr).abs() < tol
                    } else { false }
                };
                if coincides_with_arc(p1) || coincides_with_arc(p2) { continue; }
                // Score = how close each TANGENT POINT lands to the point the
                // user wants to keep (`keep1`/`keep2` — the pick clicks for two
                // separate objects, the far segment ends for a polyline corner).
                // This is what makes the fillet land on the CLICKED side, instead
                // of the old `center.dist(corner)` (degenerate for a line↔arc pair:
                // the clicked-side and far-side centres are near-equidistant from
                // the corner). `corner` still drives the bisector fallback below.
                //
                // THE LINE PICK DECIDES WHICH CORNER — weight the SEGMENT 3×. Owner:
                // "I pick the line's LEFT end, so the fillet happens at that end."
                // A line spans both candidate corners, so its pick is the reliable
                // "which corner" signal. (WHICH END of the arc grows to reach that
                // corner is a SEPARATE decision, made from the ARC pick in
                // `arc_keep` / `rebuild_side`.)
                let w = |pc: &Piece| if matches!(pc, Piece::Seg { .. }) { 3.0 } else { 1.0 };
                let score = w(p1) * tp1.dist(keep1) + w(p2) * tp2.dist(keep2);
                if best_any.map_or(true, |x| score < x.0) {
                    best_any = Some((score, center, tp1, tp2));
                }
                // `best` additionally requires the centre to sit on the inside-
                // of-corner (bisector) side. Kept only as a FALLBACK — the pick-
                // distance `score` above is now the primary selector, so `best_any`
                // (global min score) wins. The bisector must NOT veto a better
                // pick-honouring solution (that veto was what let a fillet flip to
                // the wrong side of an arc).
                if bis.len() > EPS && (center - corner).dot(bis) <= 1e-9 { continue; }
                if best.map_or(true, |x| score < x.0) {
                    best = Some((score, center, tp1, tp2));
                }
            }
        }
    }
    best_any.or(best).map(|(_, c, t1, t2)| (c, t1, t2))
}

/// The minor fillet arc between two tangent points about `center`, as a
/// `Geom::Arc` (stored CCW, positive sweep).
fn fillet_arc_geom(center: Vec2, r: f64, tp1: Vec2, tp2: Vec2) -> Geom {
    let a1 = (tp1 - center).angle();
    let a2 = (tp2 - center).angle();
    let d_ccw = (a2 - a1).rem_euclid(TAU);
    let (start, sweep) = if d_ccw <= PI { (a1, d_ccw) } else { (a2, TAU - d_ccw) };
    Geom::Arc(Arc {
        center,
        radius: r,
        start_angle: start.rem_euclid(TAU),
        sweep_angle: sweep,
    })
}

/// The bulge for a polyline segment tp1 → tp2 following the minor fillet arc.
fn fillet_arc_bulge(center: Vec2, tp1: Vec2, tp2: Vec2) -> f64 {
    let a1 = (tp1 - center).angle();
    let a2 = (tp2 - center).angle();
    let d_ccw = (a2 - a1).rem_euclid(TAU);
    let signed = if d_ccw <= PI { d_ccw } else { -(TAU - d_ccw) };
    (signed / 4.0).tan()
}

/// Rebuild an arc after a fillet/chamfer trims OR extends it to tangent point
/// `tp`. If `tp` lies ON the arc → TRIM to `tp`, keeping the side that contains
/// `pick`. If `tp` lies OFF the arc (the fillet is beyond one end) → EXTEND the
/// arc, growing the NEAR end to `tp` the SHORT way and keeping the whole arc.
/// Owner rule: the end nearest the click extends toward the line; the old code
/// wrapped the long way round (a near-full arc) when the tangent sat just past
/// an end.
fn arc_keep(center: Vec2, r: f64, a0: f64, sweep: f64, tp: Vec2, pick: Vec2) -> Geom {
    let ta = (tp - center).angle();
    let tp_delta = (ta - a0).rem_euclid(TAU);        // CCW from a0, 0..TAU
    let (start, sw) = if tp_delta <= sweep + 1e-9 {
        // TRIM — tangent is on the arc; keep the pick side.
        let pick_delta = ((pick - center).angle() - a0).rem_euclid(TAU);
        if pick_delta <= tp_delta {
            (a0, tp_delta)                           // keep [a0 .. tp]
        } else {
            (ta, sweep - tp_delta)                   // keep [tp .. end]
        }
    } else {
        // EXTEND — tangent is beyond an end. Grow the end nearest the CLICK (`pick`)
        // to reach `tp`, keeping the whole arc. Owner rule: the arc's clicked end is
        // the one that grows toward the line — click the arc's right end → the right
        // (a0) end grows; click the left → the far end grows. (This can wrap most of
        // the way round when the corner is on the far side from the clicked end —
        // that IS the intended "grow the clicked end all the way to the line".)
        let pick_delta = ((pick - center).angle() - a0).rem_euclid(TAU);  // ∈ [0, sweep]
        if pick_delta <= sweep * 0.5 {
            let gap_before = TAU - tp_delta;         // CW from a0 to tp
            (ta, sweep + gap_before)                 // grow the a0 (start) end to tp
        } else {
            let gap_after = tp_delta - sweep;        // CCW past the far end to tp
            (a0, sweep + gap_after)                  // grow the far end to tp
        }
    };
    Geom::Arc(Arc {
        center,
        radius: r,
        start_angle: start.rem_euclid(TAU),
        sweep_angle: sw.max(0.0).min(TAU),
    })
}

/// Recompute a polyline segment's bulge after its endpoints moved, keeping
/// the original arc's circle. Straight stays straight.
fn recompute_bulge(orig_bulge: f64, a: Vec2, b: Vec2, new_a: Vec2, new_b: Vec2) -> f64 {
    if orig_bulge.abs() < 1e-12 { return 0.0; }
    let Some((center, _r, _sa, sweep)) = bulge_arc(a, b, orig_bulge) else { return 0.0; };
    if new_a.dist(new_b) < EPS { return 0.0; }
    let s = if sweep >= 0.0 { 1.0 } else { -1.0 };
    let new_sweep_abs = (((new_b - center).angle() - (new_a - center).angle()) * s)
        .rem_euclid(TAU);
    bulge_from_arc(new_a, new_b, center, new_sweep_abs)
}

// ---------------------------------------------------------------------------
// Geom ↔ Piece helpers.
// ---------------------------------------------------------------------------
fn polyseg_piece(pl: &Polyline, i: usize) -> Option<Piece> {
    let n = pl.vertices.len();
    if n < 2 { return None; }
    let a = pl.vertices[i].pos;
    let b = pl.vertices[(i + 1) % n].pos;
    let bulge = pl.vertices[i].bulge;
    if bulge.abs() < 1e-9 {
        Some(Piece::Seg { a, b })
    } else {
        let (c, r, a0, sweep) = bulge_arc(a, b, bulge)?;
        Some(Piece::Arc { c, r, a0, sweep })
    }
}

/// Nearest polyline segment index to a world point.
pub fn nearest_polyline_segment(pl: &Polyline, p: Vec2) -> Option<usize> {
    let n = pl.vertices.len();
    if n < 2 { return None; }
    let seg_count = if pl.closed { n } else { n - 1 };
    let mut best = (f64::INFINITY, 0usize);
    for i in 0..seg_count {
        let Some(pc) = polyseg_piece(pl, i) else { continue };
        let d = match pc {
            Piece::Seg { a, b } => {
                let dv = b - a; let l2 = dv.len_sq();
                let t = if l2 < EPS { 0.0 } else { ((p - a).dot(dv) / l2).clamp(0.0, 1.0) };
                p.dist(a + dv * t)
            }
            Piece::Arc { c, r, .. } => {
                // distance to the circle, clamped to the arc if the radial
                // projection is on it, else to the nearest endpoint.
                let proj = c + (p - c).normalized() * r;
                if pc.contains(proj) {
                    (p.dist(c) - r).abs()
                } else {
                    let (s, e) = pc.endpoints();
                    p.dist(s).min(p.dist(e))
                }
            }
        };
        if d < best.0 { best = (d, i); }
    }
    Some(best.1)
}

// ---------------------------------------------------------------------------
// Public: fillet/chamfer two SEPARATE objects (Line / Arc / Polyline-end).
// ---------------------------------------------------------------------------

/// Which kind of input a Geom contributed, with enough context to rebuild it.
enum Ctx {
    Line,
    Arc { center: Vec2, r: f64, a0: f64, sweep: f64 },
    /// A polyline whose clicked segment `seg` (vertices seg, seg+1) is being
    /// filleted against a separate object. The vertex that gets trimmed to the
    /// tangent point is the one FARTHER from the pick (the corner side); it
    /// must be a free end of an open polyline, else we refuse (see poly_move_ok).
    Poly { pl: Polyline, seg: usize },
}

fn geom_piece_ctx(g: &Geom, pick: Vec2) -> Result<(Piece, Ctx), String> {
    match g {
        Geom::Line(l) => Ok((Piece::Seg { a: l.a, b: l.b }, Ctx::Line)),
        Geom::Arc(a) => Ok((
            Piece::Arc { c: a.center, r: a.radius, a0: a.start_angle, sweep: a.sweep_angle },
            Ctx::Arc { center: a.center, r: a.radius, a0: a.start_angle, sweep: a.sweep_angle },
        )),
        Geom::Polyline(pl) => {
            if pl.closed {
                return Err("fillet: pick two segments of a closed polyline, or use the P option".into());
            }
            let n = pl.vertices.len();
            if n < 2 { return Err("fillet: polyline has no segments".into()); }
            let seg = nearest_polyline_segment(pl, pick)
                .ok_or_else(|| "fillet: could not locate the polyline segment".to_string())?;
            let pc = polyseg_piece(pl, seg)
                .ok_or_else(|| "fillet: degenerate polyline segment".to_string())?;
            Ok((pc, Ctx::Poly { pl: pl.clone(), seg }))
        }
        _ => Err("fillet: supports Line, Arc and Polyline (Walls use the Line path)".into()),
    }
}

/// The polyline vertex that a fillet trims to the tangent point — the clicked
/// segment's endpoint FARTHER from the pick (the corner side) — and whether it
/// is a FREE end of an open polyline (so moving it keeps the polyline valid).
fn poly_moved_vertex(pl: &Polyline, seg: usize, pick: Vec2) -> (usize, bool) {
    let n = pl.vertices.len();
    let va = pl.vertices[seg].pos;
    let vb = pl.vertices[seg + 1].pos;
    let move_i = if va.dist(pick) >= vb.dist(pick) { seg } else { seg + 1 };
    let ok = !pl.closed && (move_i == 0 || move_i == n - 1);
    (move_i, ok)
}

/// True unless this side is a polyline whose corner-side vertex is INTERIOR
/// (shared) — in that case the fillet can't trim it in place without breaking
/// the polyline, and the caller errors instead of producing garbage.
fn poly_move_ok(ctx: &Ctx, pick: Vec2) -> bool {
    match ctx {
        Ctx::Poly { pl, seg } => poly_moved_vertex(pl, *seg, pick).1,
        _ => true,
    }
}

/// Rebuild one side's Geom after trimming it to tangent point `tp`, keeping
/// the side toward `pick`.
fn rebuild_side(ctx: &Ctx, piece: &Piece, tp: Vec2, pick: Vec2) -> Geom {
    match ctx {
        Ctx::Line => {
            let (a, b) = piece.endpoints();
            // Keep the endpoint on the pick side of tp.
            let dir = pick - tp;
            let keep = if (a - tp).dot(dir) >= (b - tp).dot(dir) { a } else { b };
            Geom::Line(Line { a: keep, b: tp })
        }
        Ctx::Arc { center, r, a0, sweep } => arc_keep(*center, *r, *a0, *sweep, tp, pick),
        Ctx::Poly { pl, seg } => {
            // Keep the vertex on the SAME side of the tangent point as the PICK,
            // measured ALONG the segment (so a bulged/arc segment is handled
            // right), and move the OTHER vertex to tp. Recompute the clicked
            // segment's bulge so an arc segment stays on its circle.
            // (The old rule moved the vertex farther from the pick by straight-line
            // distance, which kept the WRONG side when the pick sat PAST tp along a
            // curved segment — owner report: it trimmed the picked side.)
            let t_tp = along_param(piece, tp);
            let t_pick = along_param(piece, pick);
            let move_i = if t_pick <= t_tp { *seg + 1 } else { *seg };
            let va = pl.vertices[*seg].pos;
            let vb = pl.vertices[*seg + 1].pos;
            let mut np = pl.clone();
            np.vertices[move_i].pos = tp;
            let new_a = np.vertices[*seg].pos;
            let new_b = np.vertices[*seg + 1].pos;
            np.vertices[*seg].bulge = recompute_bulge(pl.vertices[*seg].bulge, va, vb, new_a, new_b);
            Geom::Polyline(np)
        }
    }
}

pub fn fillet_geoms(
    g1: &Geom, p1: Vec2,
    g2: &Geom, p2: Vec2,
    radius: f64,
) -> Result<FilletOut, String> {
    if radius < 0.0 { return Err("fillet: radius must be ≥ 0".into()); }
    let (pc1, ctx1) = geom_piece_ctx(g1, p1)?;
    let (pc2, ctx2) = geom_piece_ctx(g2, p2)?;
    if !poly_move_ok(&ctx1, p1) || !poly_move_ok(&ctx2, p2) {
        return Err("fillet: can't fillet that polyline segment to a separate object in place — its corner-side end isn't free. Explode the polyline first, or pick the polyline so the end nearest the other object is its loose tip.".into());
    }

    if radius < EPS {
        // Sharp corner: trim/extend both to their intersection nearest the picks.
        let mid = (p1 + p2) * 0.5;
        let corner = nearest_point(&piece_intersect(&pc1, &pc2), mid)
            .or_else(|| infinite_corner(&pc1, &pc2))
            .ok_or_else(|| "fillet: objects do not meet".to_string())?;
        return Ok(FilletOut {
            g1_new: rebuild_side(&ctx1, &pc1, corner, p1),
            g2_new: rebuild_side(&ctx2, &pc2, corner, p2),
            arc: None,
        });
    }

    let mid = (p1 + p2) * 0.5;
    let corner = nearest_point(&piece_intersect(&pc1, &pc2), mid)
        .or_else(|| infinite_corner(&pc1, &pc2))
        .unwrap_or(mid);
    let (center, tp1, tp2) = solve_fillet(&pc1, p1, &pc2, p2, radius, corner)
        .ok_or_else(|| "fillet: no radius-r arc fits between these objects".to_string())?;

    let arc = fillet_arc_geom(center, radius, tp1, tp2);
    Ok(FilletOut {
        g1_new: rebuild_side(&ctx1, &pc1, tp1, p1),
        g2_new: rebuild_side(&ctx2, &pc2, tp2, p2),
        arc: Some(arc),
    })
}

pub fn chamfer_geoms(
    g1: &Geom, p1: Vec2,
    g2: &Geom, p2: Vec2,
    d1: f64, d2: f64,
) -> Result<ChamferOut, String> {
    if d1 < 0.0 || d2 < 0.0 { return Err("chamfer: distances must be ≥ 0".into()); }
    let (pc1, ctx1) = geom_piece_ctx(g1, p1)?;
    let (pc2, ctx2) = geom_piece_ctx(g2, p2)?;
    if !poly_move_ok(&ctx1, p1) || !poly_move_ok(&ctx2, p2) {
        return Err("chamfer: can't chamfer that polyline segment to a separate object in place — its corner-side end isn't free. Explode the polyline first.".into());
    }
    // THE LINE PICK DECIDES WHICH CORNER (mirror of the fillet rule): choose the
    // crossing nearest the SEGMENT (line) pick — a line spans both corners, so its
    // pick is the reliable "which corner" signal. (Chamfer walks a fixed distance
    // from the corner, so unlike fillet there's no separate "grow the arc end".)
    let seg_pick = match (matches!(pc1, Piece::Seg { .. }), matches!(pc2, Piece::Seg { .. })) {
        (true, false) => p1,   // pc1 is the segment (line)
        (false, true) => p2,   // pc2 is the segment (line)
        _ => (p1 + p2) * 0.5,
    };
    let corner = nearest_point(&piece_intersect(&pc1, &pc2), seg_pick)
        .or_else(|| infinite_corner(&pc1, &pc2))
        .ok_or_else(|| "chamfer: objects do not meet".to_string())?;

    let tp1 = walk_from_corner(&pc1, corner, p1, d1)
        .ok_or_else(|| "chamfer: distance exceeds object 1".to_string())?;
    let tp2 = walk_from_corner(&pc2, corner, p2, d2)
        .ok_or_else(|| "chamfer: distance exceeds object 2".to_string())?;

    Ok(ChamferOut {
        g1_new: rebuild_side(&ctx1, &pc1, tp1, p1),
        g2_new: rebuild_side(&ctx2, &pc2, tp2, p2),
        bridge: Geom::Line(Line { a: tp1, b: tp2 }),
    })
}

/// Point on `piece` at distance `d` from `corner`, walking toward the `keep`
/// side.
fn walk_from_corner(piece: &Piece, corner: Vec2, keep: Vec2, d: f64) -> Option<Vec2> {
    match *piece {
        Piece::Seg { a, b } => {
            let (a, b) = (a, b);
            // Direction along the segment toward the keep side.
            let dir = if (a - corner).dot(keep - corner) >= (b - corner).dot(keep - corner) {
                (a - corner).normalized()
            } else {
                (b - corner).normalized()
            };
            let p = corner + dir * d;
            let pc = Piece::Seg { a, b };
            if pc.contains(p) { Some(p) } else { None }
        }
        Piece::Arc { c, r, .. } => {
            let ang0 = (corner - c).angle();
            // CCW or CW toward keep?
            let keep_delta = ((keep - c).angle() - ang0).rem_euclid(TAU);
            let s = if keep_delta <= PI { 1.0 } else { -1.0 };
            let dang = d / r;
            let ang = ang0 + s * dang;
            let p = c + Vec2::new(r * ang.cos(), r * ang.sin());
            if piece.contains(p) { Some(p) } else { None }
        }
    }
}

fn nearest_point(pts: &[Vec2], to: Vec2) -> Option<Vec2> {
    pts.iter().copied().min_by(|a, b| {
        a.dist(to).partial_cmp(&b.dist(to)).unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Infinite (host-line / host-circle) corner for pieces that don't intersect
/// within extent — used so we can still extend lines to a virtual corner.
fn infinite_corner(p1: &Piece, p2: &Piece) -> Option<Vec2> {
    let pts = piece_intersect(p1, p2);
    pts.into_iter().next()
}

// ---------------------------------------------------------------------------
// Public: fillet/chamfer the CORNER between two segments of ONE polyline.
// ---------------------------------------------------------------------------

/// Result of a single-corner solve: the two tangent points (on the incoming
/// and outgoing segment respectively) and the connecting fillet bulge.
struct CornerSolve {
    tp_in: Vec2,
    tp_out: Vec2,
    bulge: f64,
}

/// Solve a fillet at the vertex shared by segment `seg_in` (…→V) and segment
/// `seg_out` (V→…). Returns None when the radius doesn't fit the segments.
fn solve_corner_fillet(pl: &Polyline, seg_in: usize, seg_out: usize, vtx: usize, radius: f64)
    -> Option<CornerSolve>
{
    let n = pl.vertices.len();
    let v = pl.vertices[vtx].pos;
    let far_in = pl.vertices[seg_in].pos;            // start of incoming seg
    let far_out = pl.vertices[(seg_out + 1) % n].pos; // end of outgoing seg
    let p_in = polyseg_piece(pl, seg_in)?;
    let p_out = polyseg_piece(pl, seg_out)?;
    let (center, tp_in, tp_out) = solve_fillet(&p_in, far_in, &p_out, far_out, radius, v)?;
    // Tangent points must lie within their own segments.
    if !p_in.contains(tp_in) || !p_out.contains(tp_out) { return None; }
    let bulge = fillet_arc_bulge(center, tp_in, tp_out);
    Some(CornerSolve { tp_in, tp_out, bulge })
}

/// Solve a chamfer at the shared vertex.
fn solve_corner_chamfer(pl: &Polyline, seg_in: usize, seg_out: usize, vtx: usize, d1: f64, d2: f64)
    -> Option<CornerSolve>
{
    let n = pl.vertices.len();
    let v = pl.vertices[vtx].pos;
    let far_in = pl.vertices[seg_in].pos;
    let far_out = pl.vertices[(seg_out + 1) % n].pos;
    let p_in = polyseg_piece(pl, seg_in)?;
    let p_out = polyseg_piece(pl, seg_out)?;
    let tp_in = walk_from_corner(&p_in, v, far_in, d1)?;
    let tp_out = walk_from_corner(&p_out, v, far_out, d2)?;
    Some(CornerSolve { tp_in, tp_out, bulge: 0.0 })
}

/// Fillet the corner between two segments of one polyline. The segments must
/// be adjacent (share a vertex).
pub fn fillet_polyline_corner(pl: &Polyline, seg_a: usize, seg_b: usize, radius: f64)
    -> Result<Polyline, String>
{
    let (seg_in, seg_out, vtx) = adjacency(pl, seg_a, seg_b)
        .ok_or_else(|| "fillet: the two polyline segments must be adjacent".to_string())?;
    let cs = solve_corner_fillet(pl, seg_in, seg_out, vtx, radius)
        .ok_or_else(|| "fillet: radius too large for these segments".to_string())?;
    Ok(apply_corner(pl, seg_in, seg_out, vtx, &cs))
}

/// Chamfer the corner between two adjacent segments of one polyline.
pub fn chamfer_polyline_corner(pl: &Polyline, seg_a: usize, seg_b: usize, d1: f64, d2: f64)
    -> Result<Polyline, String>
{
    let (seg_in, seg_out, vtx) = adjacency(pl, seg_a, seg_b)
        .ok_or_else(|| "chamfer: the two polyline segments must be adjacent".to_string())?;
    // d1 applies to the incoming segment, d2 to the outgoing — but the user
    // picked seg_a/seg_b which may be reversed; map by which is seg_in.
    let (dd1, dd2) = if seg_a == seg_in { (d1, d2) } else { (d2, d1) };
    let cs = solve_corner_chamfer(pl, seg_in, seg_out, vtx, dd1, dd2)
        .ok_or_else(|| "chamfer: distance exceeds segment length".to_string())?;
    Ok(apply_corner(pl, seg_in, seg_out, vtx, &cs))
}

/// Map two segment indices to (incoming, outgoing, shared-vertex) if adjacent.
fn adjacency(pl: &Polyline, a: usize, b: usize) -> Option<(usize, usize, usize)> {
    let n = pl.vertices.len();
    let seg_count = if pl.closed { n } else { n - 1 };
    if a >= seg_count || b >= seg_count || a == b { return None; }
    // segment i spans vertex i → (i+1)%n. Two segments are adjacent if one's
    // end vertex == the other's start vertex.
    let end = |s: usize| (s + 1) % n;
    if end(a) == b { return Some((a, b, end(a))); }       // a then b
    if end(b) == a { return Some((b, a, end(b))); }       // b then a
    None
}

/// Rebuild the polyline replacing the shared corner vertex `vtx` with the two
/// tangent points (and the connecting bulge on tp_in).
fn apply_corner(pl: &Polyline, seg_in: usize, seg_out: usize, vtx: usize, cs: &CornerSolve)
    -> Polyline
{
    let n = pl.vertices.len();
    let a_in = pl.vertices[seg_in].pos;
    let v = pl.vertices[vtx].pos;
    let bulge_in = pl.vertices[seg_in].bulge;
    let b_out = pl.vertices[(seg_out + 1) % n].pos;
    let bulge_out = pl.vertices[seg_out].bulge;

    let new_in_bulge = recompute_bulge(bulge_in, a_in, v, a_in, cs.tp_in);
    let new_out_bulge = recompute_bulge(bulge_out, v, b_out, cs.tp_out, b_out);

    let mut out: Vec<PolyVertex> = Vec::with_capacity(n + 1);
    for (i, pv) in pl.vertices.iter().enumerate() {
        if i == vtx {
            // Replace V by tp_in (carries the fillet bulge) then tp_out.
            out.push(PolyVertex { pos: cs.tp_in, bulge: cs.bulge });
            out.push(PolyVertex { pos: cs.tp_out, bulge: new_out_bulge });
        } else {
            out.push(*pv);
        }
    }
    // Fix the incoming segment's bulge (vertex seg_in now ends at tp_in).
    // After insertion, seg_in's index is unchanged if seg_in < vtx, else +0
    // (vtx is the only inserted slot and seg_in != vtx). Its position in
    // `out` equals its original index when seg_in < vtx, original+0 when
    // seg_in is before vtx in the vector. Since seg_in is the vertex BEFORE
    // vtx along the ring, for an interior corner seg_in == vtx-1 (< vtx) so
    // index is unchanged; for the wrap corner (vtx==0) seg_in == n-1 which
    // shifts by +1 due to the insertion at index 0.
    let in_idx = if seg_in < vtx { seg_in } else { seg_in + 1 };
    if let Some(pvv) = out.get_mut(in_idx) { pvv.bulge = new_in_bulge; }

    Polyline { vertices: out, closed: pl.closed, widths: Vec::new() }
}

// ---------------------------------------------------------------------------
// Public: fillet/chamfer EVERY corner of one polyline (the `P` option).
// ---------------------------------------------------------------------------

/// Collapse existing fillet arcs back to their sharp corners. A fillet is a
/// line → arc → line run where the arc is TANGENT to both straight neighbours
/// (its defining property). Each such arc + its two tangent vertices is
/// replaced by the single recovered corner (the intersection of the two
/// straight sides). This lets `fillet P` be re-run with a new radius to UPDATE
/// the rounding instead of filleting the already-rounded corners. Genuine
/// (non-tangent) arc segments, and arcs whose neighbours aren't straight, are
/// left untouched.
fn defillet_polyline(pl: &Polyline) -> Polyline {
    let n = pl.vertices.len();
    if n < 4 { return pl.clone(); }
    let seg_count = if pl.closed { n } else { n - 1 };
    let seg_straight = |i: usize| pl.vertices[i].bulge.abs() < 1e-9;
    let seg_dir = |i: usize| -> Vec2 {
        (pl.vertices[(i + 1) % n].pos - pl.vertices[i].pos).normalized()
    };

    let mut fillet = vec![false; n];
    let mut corner = vec![Vec2::ZERO; n];
    for a in 0..seg_count {
        if seg_straight(a) { continue; }              // need an arc segment
        if !pl.closed && (a == 0 || a >= seg_count - 1) { continue; } // needs both sides
        let prev = (a + n - 1) % n;
        let next = (a + 1) % n;
        if prev >= seg_count || next >= seg_count { continue; }
        if !seg_straight(prev) || !seg_straight(next) { continue; }
        let va = pl.vertices[a].pos;
        let vb = pl.vertices[next].pos;
        let Some((center, _r, _sa, _sw)) = bulge_arc(va, vb, pl.vertices[a].bulge) else { continue };
        // Tangency: arc tangent (⊥ radius) ∥ the straight neighbour at each end.
        let ta = (va - center).perp().normalized();
        let tb = (vb - center).perp().normalized();
        if ta.cross(seg_dir(prev)).abs() > 1e-3 { continue; }
        if tb.cross(seg_dir(next)).abs() > 1e-3 { continue; }
        // Recover the corner = intersection of the two straight sides.
        let Some(c) = line_line(pl.vertices[prev].pos, seg_dir(prev),
                                pl.vertices[next].pos, seg_dir(next)) else { continue };
        fillet[a] = true;
        corner[a] = c;
    }
    if !fillet.iter().any(|&f| f) { return pl.clone(); }

    // Rebuild: an arc's start vertex (tp_in) → the corner; its end vertex
    // (tp_out) is dropped (merged into that corner). Wrap-safe for closed.
    let mut out: Vec<PolyVertex> = Vec::with_capacity(n);
    for v in 0..n {
        let starts = fillet[v];                    // seg v is a fillet arc → v = tp_in
        let ends = fillet[(v + n - 1) % n];        // seg v-1 is a fillet arc → v = tp_out
        if starts {
            out.push(PolyVertex { pos: corner[v], bulge: 0.0 });
        } else if ends {
            // tp_out — merged into the corner already emitted at tp_in.
        } else {
            out.push(pl.vertices[v]);
        }
    }
    Polyline { vertices: out, closed: pl.closed, widths: Vec::new() }
}

/// Monotonic parameter of a point along a piece's host line/circle, measured
/// from the piece's start: a fraction for a segment, the swept-angle delta for
/// an arc. Used only to compare ordering of two points on the same segment.
fn along_param(orig: &Piece, p: Vec2) -> f64 {
    match *orig {
        Piece::Seg { a, b } => {
            let d = b - a;
            let l2 = d.len_sq();
            if l2 < EPS { 0.0 } else { (p - a).dot(d) / l2 }
        }
        Piece::Arc { c, a0, sweep, .. } => {
            let s = if sweep >= 0.0 { 1.0 } else { -1.0 };
            (((p - c).angle() - a0) * s).rem_euclid(TAU)
        }
    }
}

enum AllOp { Fillet(f64), Chamfer(f64, f64) }

fn polyline_all(pl: &Polyline, op: AllOp) -> Result<(Polyline, usize), String> {
    // Re-running fillet P should UPDATE existing fillets to the new radius, not
    // round the already-rounded corners. Collapse tangent fillet arcs back to
    // sharp corners first, then fillet the sharpened outline.
    let sharpened = if matches!(op, AllOp::Fillet(_)) {
        Some(defillet_polyline(pl))
    } else {
        None
    };
    let pl = sharpened.as_ref().unwrap_or(pl);

    let n = pl.vertices.len();
    if n < 3 { return Err("need at least 3 vertices".into()); }
    let seg_count = if pl.closed { n } else { n - 1 };

    // For each corner vertex k, solve and remember the two tangent points.
    // corner_at[k] = Some((tp_in, tp_out, corner_bulge)) when filleted.
    let mut corner_at: Vec<Option<(Vec2, Vec2, f64)>> = vec![None; n];
    let corner_vertices: Vec<usize> = if pl.closed {
        (0..n).collect()
    } else {
        (1..n - 1).collect()
    };
    let mut count = 0usize;
    for &k in &corner_vertices {
        let seg_out = k;                       // segment k → k+1
        let seg_in = (k + n - 1) % n;          // segment k-1 → k
        if seg_in >= seg_count || seg_out >= seg_count { continue; }
        let solved = match op {
            AllOp::Fillet(r) => solve_corner_fillet(pl, seg_in, seg_out, k, r),
            AllOp::Chamfer(d1, d2) => solve_corner_chamfer(pl, seg_in, seg_out, k, d1, d2),
        };
        if let Some(cs) = solved {
            corner_at[k] = Some((cs.tp_in, cs.tp_out, cs.bulge));
            count += 1;
        }
    }
    if count == 0 { return Err("radius too large for any corner".into()); }

    // Assemble: for each segment, its start/end may be a tangent point.
    let start_of = |k: usize| corner_at[k].map(|c| c.1).unwrap_or(pl.vertices[k].pos);
    let end_of = |k: usize| {
        let ev = (k + 1) % n;
        corner_at[ev].map(|c| c.0).unwrap_or(pl.vertices[ev].pos)
    };

    // OVERLAP CHECK: two fillets on the SAME segment (one at each end) must
    // not eat past each other. Each individual corner only validated against
    // its own full segment; here we require the trimmed span to keep its
    // direction (start param ≤ end param along the original segment).
    for i in 0..seg_count {
        let Some(orig) = polyseg_piece(pl, i) else { continue };
        let ts = along_param(&orig, start_of(i));
        let te = along_param(&orig, end_of(i));
        if ts > te + 1e-6 {
            return Err("radius too large — rounded corners would overlap".into());
        }
    }

    let mut out: Vec<PolyVertex> = Vec::with_capacity(n * 2);
    for i in 0..seg_count {
        let a = pl.vertices[i].pos;
        let b = pl.vertices[(i + 1) % n].pos;
        let orig_bulge = pl.vertices[i].bulge;
        let s = start_of(i);
        let e = end_of(i);
        let seg_bulge = recompute_bulge(orig_bulge, a, b, s, e);
        out.push(PolyVertex { pos: s, bulge: seg_bulge });
        // If the segment ends at a filleted corner, emit the corner tangent
        // point with the fillet bulge.
        let ev = (i + 1) % n;
        if let Some((tp_in, _tp_out, cbulge)) = corner_at[ev] {
            out.push(PolyVertex { pos: tp_in, bulge: cbulge });
        }
    }
    if !pl.closed {
        // open polyline: append the final endpoint (last segment's true end).
        out.push(PolyVertex { pos: pl.vertices[n - 1].pos, bulge: 0.0 });
    }

    // Drop consecutive coincident vertices (un-filleted corners produce a
    // duplicate). Keep the later one's bulge.
    let mut dedup: Vec<PolyVertex> = Vec::with_capacity(out.len());
    for pv in out.into_iter() {
        if let Some(last) = dedup.last() {
            if last.pos.dist(pv.pos) < 1e-9 {
                // collapse: keep this one's bulge (outgoing).
                let li = dedup.len() - 1;
                dedup[li].bulge = pv.bulge;
                continue;
            }
        }
        dedup.push(pv);
    }

    Ok((Polyline { vertices: dedup, closed: pl.closed, widths: Vec::new() }, count))
}

/// Fillet every corner of a polyline with `radius`. Returns the new polyline
/// and how many corners were rounded.
pub fn fillet_polyline_all(pl: &Polyline, radius: f64) -> Result<(Polyline, usize), String> {
    polyline_all(pl, AllOp::Fillet(radius)).map_err(|e| format!("fillet P: {e}"))
}

/// Chamfer every corner of a polyline with distances `d1`/`d2`.
pub fn chamfer_polyline_all(pl: &Polyline, d1: f64, d2: f64) -> Result<(Polyline, usize), String> {
    polyline_all(pl, AllOp::Chamfer(d1, d2)).map_err(|e| format!("chamfer P: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Geom {
        Geom::Line(Line { a: Vec2::new(ax, ay), b: Vec2::new(bx, by) })
    }

    #[test]
    fn fillet_two_perpendicular_lines() {
        // L1 along +x from origin, L2 along +y from origin. Corner at (0,0).
        let l1 = line(0.0, 0.0, 10.0, 0.0);
        let l2 = line(0.0, 0.0, 0.0, 10.0);
        // Picks far from the corner so the kept side is the far end.
        let out = fillet_geoms(&l1, Vec2::new(8.0, 0.0),
                               &l2, Vec2::new(0.0, 8.0), 2.0).unwrap();
        let arc = out.arc.expect("expected a fillet arc");
        if let Geom::Arc(a) = arc {
            assert!((a.radius - 2.0).abs() < 1e-9);
            // Centre of a fillet on this corner is (2,2).
            assert!((a.center - Vec2::new(2.0, 2.0)).len() < 1e-6,
                "center was {:?}", a.center);
        } else { panic!("not an arc"); }
        // Trimmed lines should end at the tangent points (2,0) and (0,2).
        if let Geom::Line(l) = out.g1_new {
            assert!(l.a.dist(Vec2::new(2.0, 0.0)).min(l.b.dist(Vec2::new(2.0, 0.0))) < 1e-6);
        } else { panic!(); }
    }

    #[test]
    fn fillet_line_arc_lands_on_the_clicked_side_not_the_far_side() {
        // Regression (owner dump 2026-07-29): a line crossing a big arc has TWO
        // valid same-radius fillet solutions — one on each side of the arc. The
        // fillet must land where the user CLICKED, not flip to the far side.
        // Line a=(-40,-20)→(40,20); Arc = upper half of circle c=(0,0) r=45.
        let l = Geom::Line(Line { a: Vec2::new(-40.0, -20.0), b: Vec2::new(40.0, 20.0) });
        let arc = Geom::Arc(Arc {
            center: Vec2::new(0.0, 0.0), radius: 45.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::PI,
        });
        // Picks (from the dump) sit on the lower-right pocket.
        let p_line = Vec2::new(33.657, 16.849);
        let p_arc  = Vec2::new(43.053, 12.152);
        let out = fillet_geoms(&l, p_line, &arc, p_arc, 8.498).unwrap();
        let Geom::Arc(a) = out.arc.expect("expected a fillet arc") else { panic!("not an arc") };
        assert!((a.radius - 8.498).abs() < 1e-3, "radius {}", a.radius);
        // The CLICKED-side solution (a same-radius circle tangent to both near the
        // picks) is centred ~(35.56, 8.29); the WRONG far side is ~(27.95, 23.48).
        let clicked = Vec2::new(35.557, 8.286);
        let far     = Vec2::new(27.950, 23.477);
        assert!(a.center.dist(clicked) < a.center.dist(far),
            "fillet flipped to the far side of the arc: center {:?}", a.center);
        assert!(a.center.dist(clicked) < 3.0, "center {:?} not near the clicked pocket", a.center);
    }

    #[test]
    fn chamfer_line_pick_decides_the_corner() {
        // Chamfer corner follows the LINE pick (same as the fillet CORNER rule).
        // Horizontal line y=30 crosses the 180° arc at ±33.54. Click the LINE's
        // right half → bevel at the RIGHT crossing; left half → LEFT.
        let arc = Geom::Arc(Arc { center: Vec2::new(0.0, 0.0), radius: 45.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::PI });
        let line = Geom::Line(Line { a: Vec2::new(-50.0, 30.0), b: Vec2::new(50.0, 30.0) });
        let arc_pick = Vec2::new(0.0, 45.0);   // arc top — neutral
        let right = chamfer_geoms(&arc, arc_pick, &line, Vec2::new(40.0, 30.0), 5.0, 5.0).unwrap();
        if let Geom::Line(b) = right.bridge {
            assert!((b.a + b.b).x * 0.5 > 0.0, "line clicked right → bridge on the right, got {:?}", b);
        }
        let left = chamfer_geoms(&arc, arc_pick, &line, Vec2::new(-40.0, 30.0), 5.0, 5.0).unwrap();
        if let Geom::Line(b) = left.bridge {
            assert!((b.a + b.b).x * 0.5 < 0.0, "line clicked left → bridge on the left, got {:?}", b);
        }
    }

    #[test]
    fn fillet_arc_extends_the_near_end_not_the_long_way() {
        // Owner: hovering the arc's RIGHT end must extend the RIGHT end a few
        // degrees toward the line — NOT wrap the arc the long way into a near-full
        // circle. Line y=-5 crosses the circle just below both ends; arc=upper 180°.
        let line = Geom::Line(Line { a: Vec2::new(-50.0, -15.0), b: Vec2::new(50.0, -15.0) });
        let arc = Geom::Arc(Arc { center: Vec2::new(0.0, 0.0), radius: 45.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::PI });
        // First pick = line (neutral); SECOND = arc on the RIGHT (~19°).
        let out = fillet_geoms(&line, Vec2::new(0.0, -15.0), &arc, Vec2::new(42.4, 15.0), 5.0).unwrap();
        let Geom::Arc(kept) = out.g2_new else { panic!("g2 should stay an arc") };
        let sw = kept.sweep_angle.to_degrees();
        assert!(sw > 180.0 && sw < 220.0,
            "arc should extend the near end a little (>180°, ≪360°), got {sw:.0}° (long-way wrap = bug)");
    }

    #[test]
    fn fillet_arc_hover_side_decides_the_corner() {
        // Owner image 1/2: line through the arc's centre, ARC picked SECOND. Hover
        // the arc's LEFT → fillet at the left corner; the RIGHT → right corner. The
        // 2nd pick (the arc) must decide, so the two sides differ.
        let line = Geom::Line(Line { a: Vec2::new(-40.0, -20.0), b: Vec2::new(40.0, 20.0) });
        let arc = Geom::Arc(Arc { center: Vec2::new(0.0, 0.0), radius: 45.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::PI });
        let line_pick = Vec2::new(0.0, 0.0);   // neutral centre of the line
        // Arc points: LEFT ~170° and RIGHT ~10° (both on the 0..180° arc).
        let l = 170f64.to_radians(); let r = 10f64.to_radians();
        let left = fillet_geoms(&line, line_pick, &arc, Vec2::new(45.0*l.cos(), 45.0*l.sin()), 5.0).unwrap();
        let right = fillet_geoms(&line, line_pick, &arc, Vec2::new(45.0*r.cos(), 45.0*r.sin()), 5.0).unwrap();
        let cl = if let Some(Geom::Arc(a)) = left.arc { a.center } else { panic!() };
        let cr = if let Some(Geom::Arc(a)) = right.arc { a.center } else { panic!() };
        assert!(cl.x < 0.0, "arc hovered LEFT → fillet on the left, got {:?}", cl);
        assert!(cr.x > 0.0, "arc hovered RIGHT → fillet on the right, got {:?}", cr);
    }

    #[test]
    fn fillet_line_sets_corner_arc_pick_sets_which_end_grows() {
        // Owner 2026-07-29 (detailed): TWO separate decisions. The LINE pick sets
        // WHICH CORNER (line clicked LEFT → left corner). The ARC pick sets WHICH
        // END of the arc grows to reach it: arc clicked RIGHT → the right end grows
        // all the way round (big arc); arc clicked LEFT → the near end grows a
        // little. Both cases fillet at the SAME (line-left) corner.
        let line = Geom::Line(Line { a: Vec2::new(-40.0, -20.0), b: Vec2::new(40.0, 20.0) });
        let arc = Geom::Arc(Arc { center: Vec2::new(0.0, 0.0), radius: 45.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::PI });
        let line_left = Vec2::new(-31.167, -15.21);
        let ar = fillet_geoms(&line, line_left, &arc, Vec2::new(44.167, 3.79), 5.0).unwrap();   // arc RIGHT
        let al = fillet_geoms(&line, line_left, &arc, Vec2::new(-43.5, 10.79), 5.0).unwrap();    // arc LEFT
        // Both fillets land at the LEFT (line-pick) corner.
        let cr = if let Some(Geom::Arc(a)) = ar.arc { a.center } else { panic!() };
        let cl = if let Some(Geom::Arc(a)) = al.arc { a.center } else { panic!() };
        assert!(cr.x < 0.0 && cl.x < 0.0, "both should be the LEFT corner: {cr:?} {cl:?}");
        // Arc-RIGHT click grows the far (right) end round → big arc; arc-LEFT → small.
        let sw_r = if let Geom::Arc(a) = ar.g2_new { a.sweep_angle.to_degrees() } else { panic!() };
        let sw_l = if let Geom::Arc(a) = al.g2_new { a.sweep_angle.to_degrees() } else { panic!() };
        assert!(sw_r > 300.0, "arc clicked RIGHT → right end grows round (big), got {sw_r:.0}°");
        assert!(sw_l < 220.0, "arc clicked LEFT → near end grows a little, got {sw_l:.0}°");
    }

    #[test]
    fn fillet_short_arc_tangent_lands_on_the_arc_extent() {
        // Owner report 2026-07-29: "the short arc doesn't fillet with the line".
        // A short arc must fillet where it ACTUALLY is. Two arcs of the same
        // circle (c=0, r=45): a LONG one (0..180°) and a SHORT one (190..220°,
        // the lower-left corner where the line y=0.5x re-crosses the circle at
        // ~206.6°). Filleting the line to the SHORT arc must place the arc-side
        // tangent within/near the short arc's extent, not up at ~26° (the OTHER,
        // upper-right crossing) which the short arc doesn't cover.
        let line = Geom::Line(Line { a: Vec2::new(-60.0, -30.0), b: Vec2::new(60.0, 30.0) });
        let short = Geom::Arc(Arc { center: Vec2::new(0.0, 0.0), radius: 45.0,
            start_angle: 190.0_f64.to_radians(), sweep_angle: 30.0_f64.to_radians() });
        // Pick the arc in the lower-left (~206°) and the line just outside there.
        let arc_pick = Vec2::new(45.0 * 206f64.to_radians().cos(), 45.0 * 206f64.to_radians().sin());
        let line_pick = Vec2::new(-42.0, -21.0);
        let out = fillet_geoms(&line, line_pick, &short, arc_pick, 6.0).unwrap();
        let Geom::Arc(fa) = out.arc.expect("fillet arc") else { panic!("not an arc") };
        // The fillet arc's endpoint that sits on the r=45 circle = the arc-side
        // tangent. Its angle must be in the lower-left (near 190..220°), NOT ~26°.
        let e1 = fa.center + Vec2::new(fa.radius * fa.start_angle.cos(), fa.radius * fa.start_angle.sin());
        let e2 = fa.center + Vec2::new(fa.radius * (fa.start_angle + fa.sweep_angle).cos(),
                                       fa.radius * (fa.start_angle + fa.sweep_angle).sin());
        let tp = if (e1.len() - 45.0).abs() < (e2.len() - 45.0).abs() { e1 } else { e2 };
        let ang = tp.angle().to_degrees().rem_euclid(360.0);
        assert!((150.0..=260.0).contains(&ang),
            "short-arc fillet tangent landed at {ang:.1}°, off the 190..220° arc");
    }

    #[test]
    fn fillet_radius_equal_to_arc_radius_does_not_return_the_arc_itself() {
        // Owner dump 2026-07-29 #3: filleting an arc of r≈6.841 to another arc
        // with fillet radius 6.8415 (≈ the arc's own radius) returned a "fillet"
        // arc IDENTICAL to the input arc (its inner offset locus |rr−r|≈0 collapsed
        // onto the arc centre). The result must NOT coincide with either input arc.
        let arc7 = Geom::Arc(Arc { center: Vec2::new(10.813, 1.262), radius: 14.368,
            start_angle: 253.92_f64.to_radians(), sweep_angle: 108.04_f64.to_radians() });
        let arc8 = Geom::Arc(Arc { center: Vec2::new(18.335, 1.519), radius: 6.841,
            start_angle: 1.95_f64.to_radians(), sweep_angle: 114.61_f64.to_radians() });
        let r = 6.841459993376525;   // ≈ arc8.radius
        if let Ok(out) = fillet_geoms(&arc8, Vec2::new(19.333, 7.79),
                                      &arc7, Vec2::new(24.5, -2.71), r) {
            if let Some(Geom::Arc(a)) = out.arc {
                // Not coincident with arc8 (the degenerate result).
                let same_as_arc8 = a.center.dist(Vec2::new(18.335, 1.519)) < 0.1
                    && (a.radius - 6.841).abs() < 0.1;
                assert!(!same_as_arc8,
                    "fillet returned a copy of the input arc: center {:?} r {}", a.center, a.radius);
            }
        }
    }

    #[test]
    fn fillet_line_inside_arc_circle_stays_on_clicked_side() {
        // Owner dump 2026-07-29 #2: the whole LINE sits INSIDE the arc's circle
        // (both ends radius < 45) and the arc is a partial 193° sweep. Fillet
        // still flipped to the far side (result centre ~(-35.54,-8.25)).
        let l = Geom::Line(Line { a: Vec2::new(40.0, 20.0), b: Vec2::new(-31.734, -15.867) });
        let arc = Geom::Arc(Arc {
            center: Vec2::new(0.0, 0.0), radius: 45.0,
            start_angle: 0.0, sweep_angle: 193.07_f64.to_radians(),
        });
        let out = fillet_geoms(&l, Vec2::new(33.167, 16.457),
                               &arc, Vec2::new(43.667, 11.623), 8.513).unwrap();
        let Geom::Arc(a) = out.arc.expect("expected a fillet arc") else { panic!("not an arc") };
        // Clicked pocket is Q1 near ~(35.55, 8.26); the flipped far side is
        // ~(-35.54, -8.25).
        assert!(a.center.x > 0.0 && a.center.y > 0.0,
            "fillet flipped off the clicked (Q1) side: center {:?}", a.center);
        assert!(a.center.dist(Vec2::new(35.55, 8.26)) < 3.0,
            "center {:?} not in the clicked pocket", a.center);
    }

    #[test]
    fn fillet_line_to_polyline_end_segment_moves_free_tip() {
        // Open polyline: free tip (10,0) → interior (0,0) → free (0,10).
        // Vertical line at x=20. Fillet the horizontal end segment (whose free
        // tip (10,0) faces the line) — picking the polyline on the BODY side.
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },   // free tip (seg 0 start)
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },    // interior
                PolyVertex { pos: Vec2::new(0.0, 10.0), bulge: 0.0 },   // free end
            ],
            closed: false,
            widths: Vec::new(),
        };
        let line = Geom::Line(Line { a: Vec2::new(20.0, -5.0), b: Vec2::new(20.0, 5.0) });
        let out = fillet_geoms(&line, Vec2::new(20.0, 4.0),
                               &Geom::Polyline(pl), Vec2::new(3.0, 0.0), 2.0).unwrap();
        assert!(out.arc.is_some(), "expected a fillet arc");
        let Geom::Polyline(np) = out.g2_new else { panic!("polyline side should stay a polyline") };
        // Free tip extended to the tangent point (18,0); interior + far end intact.
        assert!(np.vertices[0].pos.dist(Vec2::new(18.0, 0.0)) < 1e-6,
            "tip moved to {:?}", np.vertices[0].pos);
        assert!(np.vertices[1].pos.dist(Vec2::new(0.0, 0.0)) < 1e-9, "interior moved!");
        assert!(np.vertices[2].pos.dist(Vec2::new(0.0, 10.0)) < 1e-9, "far end moved!");
    }

    #[test]
    fn fillet_bulged_polyline_keeps_the_picked_side_when_pick_is_past_tp() {
        // Owner dump 2026-07-29: a 2-vertex BULGED polyline (an arc stored as a
        // polyline) filleted with a line. The pick is nearest v00 by straight line,
        // but ALONG the arc it sits PAST the tangent point (on the v01 side). The
        // fillet must keep the v01 (picked) side — i.e. move v00 to tp, leave v01.
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(185.174, -92.434), bulge: -0.3655 },
                PolyVertex { pos: Vec2::new(408.011, 95.594), bulge: 0.0 },
            ],
            closed: false, widths: Vec::new(),
        };
        let line = Geom::Line(Line { a: Vec2::new(160.359, -102.118), b: Vec2::new(236.160, -72.537) });
        let out = fillet_geoms(&Geom::Polyline(pl), Vec2::new(205.938, -26.606),
                               &line, Vec2::new(216.361, -78.719), 5.0).unwrap();
        let Geom::Polyline(np) = out.g1_new else { panic!("poly side stays a polyline") };
        // v01 (the PICKED side) is untouched; v00 moved to the tangent point.
        assert!(np.vertices[1].pos.dist(Vec2::new(408.011, 95.594)) < 1e-6,
            "picked-side vertex v01 was moved: {:?}", np.vertices[1].pos);
        assert!(np.vertices[0].pos.dist(Vec2::new(185.174, -92.434)) > 1e-3,
            "v00 should have moved to tp (kept the picked v01 side)");
    }

    #[test]
    fn fillet_line_to_polyline_interior_side_is_refused_not_garbage() {
        // Same polyline, but pick near the FREE TIP (10,0): the corner-side
        // vertex would then be the INTERIOR (0,0), which can't be moved in
        // place — must error (suggesting explode), NOT mangle the polyline.
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 10.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        };
        let line = Geom::Line(Line { a: Vec2::new(20.0, -5.0), b: Vec2::new(20.0, 5.0) });
        let err = fillet_geoms(&line, Vec2::new(20.0, 4.0),
                               &Geom::Polyline(pl), Vec2::new(9.0, 0.0), 2.0).unwrap_err();
        assert!(err.contains("Explode") || err.contains("free"), "msg was: {err}");
    }

    #[test]
    fn chamfer_two_perpendicular_lines() {
        let l1 = line(0.0, 0.0, 10.0, 0.0);
        let l2 = line(0.0, 0.0, 0.0, 10.0);
        let out = chamfer_geoms(&l1, Vec2::new(8.0, 0.0),
                                &l2, Vec2::new(0.0, 8.0), 2.0, 3.0).unwrap();
        if let Geom::Line(br) = out.bridge {
            // bridge connects (2,0) and (0,3) in some order.
            let ok = (br.a.dist(Vec2::new(2.0, 0.0)) < 1e-6 && br.b.dist(Vec2::new(0.0, 3.0)) < 1e-6)
                  || (br.b.dist(Vec2::new(2.0, 0.0)) < 1e-6 && br.a.dist(Vec2::new(0.0, 3.0)) < 1e-6);
            assert!(ok, "bridge was {:?}", br);
        } else { panic!(); }
    }

    #[test]
    fn fillet_all_corners_of_square() {
        // Unit-ish closed square 0..4 CCW.
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 4.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 4.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        };
        let (np, count) = fillet_polyline_all(&pl, 1.0).unwrap();
        assert_eq!(count, 4);
        // 4 corners → 8 vertices, each a fillet (non-zero bulge on every
        // other vertex).
        assert_eq!(np.vertices.len(), 8, "verts: {:?}", np.vertices);
        let fillet_bulges = np.vertices.iter().filter(|v| v.bulge.abs() > 1e-6).count();
        assert_eq!(fillet_bulges, 4);
    }

    #[test]
    fn fillet_corner_of_open_L() {
        // L-shape: (0,0)->(4,0)->(4,4). Corner at vertex 1 between seg0,seg1.
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 4.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        };
        let np = fillet_polyline_corner(&pl, 0, 1, 1.0).unwrap();
        assert_eq!(np.vertices.len(), 4);
        // tangent points: (3,0) and (4,1).
        assert!(np.vertices[1].pos.dist(Vec2::new(3.0, 0.0)) < 1e-6,
            "v1 = {:?}", np.vertices[1].pos);
        assert!(np.vertices[2].pos.dist(Vec2::new(4.0, 1.0)) < 1e-6,
            "v2 = {:?}", np.vertices[2].pos);
        assert!(np.vertices[1].bulge.abs() > 1e-6);
    }

    #[test]
    fn refillet_updates_radius_does_not_stack() {
        // Square → fillet r=1 → re-fillet r=0.5. Must still be 8 vertices
        // (4 rounded corners), NOT 16, and the new fillet radius must be ~0.5.
        let sq = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 10.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        };
        let (round1, _) = fillet_polyline_all(&sq, 1.0).unwrap();
        assert_eq!(round1.vertices.len(), 8);
        let (round2, count) = fillet_polyline_all(&round1, 0.5).unwrap();
        assert_eq!(count, 4);
        assert_eq!(round2.vertices.len(), 8, "re-fillet stacked: {:?}", round2.vertices);
        // Reconstruct an arc radius from one fillet vertex.
        let arc_v = round2.vertices.iter().enumerate()
            .find(|(_, v)| v.bulge.abs() > 1e-6).unwrap();
        let i = arc_v.0;
        let a = round2.vertices[i].pos;
        let b = round2.vertices[(i + 1) % round2.vertices.len()].pos;
        let (_, r, _, _) = bulge_arc(a, b, round2.vertices[i].bulge).unwrap();
        assert!((r - 0.5).abs() < 1e-6, "radius after re-fillet was {r}, want 0.5");
    }

    #[test]
    fn defillet_recovers_original_square_corners() {
        let sq = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 10.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        };
        let (rounded, _) = fillet_polyline_all(&sq, 2.0).unwrap();
        let back = defillet_polyline(&rounded);
        assert_eq!(back.vertices.len(), 4);
        // Each recovered corner should coincide with an original corner.
        for orig in &sq.vertices {
            assert!(back.vertices.iter().any(|v| v.pos.dist(orig.pos) < 1e-6),
                "missing corner {:?} in {:?}", orig.pos, back.vertices);
        }
    }

    #[test]
    fn fillet_all_radius_too_large_overlaps_errs() {
        // 4×4 square; r=3 → each corner eats 3 of every 4-long side, so the
        // two fillets on one side overlap. Must error (not silently produce a
        // self-intersecting blob).
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 4.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 4.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        };
        let err = fillet_polyline_all(&pl, 3.0).unwrap_err();
        assert!(err.contains("too large"), "msg was: {err}");
    }

    #[test]
    fn fillet_corner_radius_too_large_errs() {
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 4.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        };
        // tangent distance = r = 10 ≫ 4-long segments.
        let err = fillet_polyline_corner(&pl, 0, 1, 10.0).unwrap_err();
        assert!(err.contains("too large"), "msg was: {err}");
    }

    #[test]
    fn chamfer_all_corners_of_square_stays_straight() {
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 4.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 4.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        };
        let (np, count) = chamfer_polyline_all(&pl, 1.0, 1.0).unwrap();
        assert_eq!(count, 4);
        assert_eq!(np.vertices.len(), 8);
        assert!(np.vertices.iter().all(|v| v.bulge.abs() < 1e-9));
    }
}
