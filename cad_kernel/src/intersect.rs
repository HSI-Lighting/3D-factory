// Pure-function intersection math. Returns a Vec<Vec2> of hits.
// Empty vec  = no intersection.
// 1 element  = tangent / single hit.
// 2 elements = two-point intersection.

use crate::geom::*;
use crate::math::*;

pub fn intersect(a: &Geom, b: &Geom) -> Vec<Vec2> {
    use Geom::*;
    match (a, b) {
        (Line(l1),   Line(l2))   => intersect_line_line(*l1, *l2),
        (Line(l),    Circle(c))  | (Circle(c), Line(l))  => intersect_line_circle(*l, *c),
        (Line(l),    Arc(ar))    | (Arc(ar),   Line(l))  => intersect_line_arc(*l, *ar),
        (Circle(c1), Circle(c2)) => intersect_circle_circle(*c1, *c2),
        (Circle(c),  Arc(ar))    | (Arc(ar),   Circle(c)) => intersect_arc_circle(*ar, *c),
        (Arc(a1),    Arc(a2))    => intersect_arc_arc(*a1, *a2),

        // ---- Ellipse pairs ----
        (Line(l),   Ellipse(e)) | (Ellipse(e), Line(l))  => intersect_line_ellipse(*l, *e),
        (Circle(c), Ellipse(e)) | (Ellipse(e), Circle(c)) => intersect_circle_ellipse(*c, *e),
        (Arc(ar),   Ellipse(e)) | (Ellipse(e), Arc(ar))  => intersect_arc_ellipse(*ar, *e),
        (Ellipse(e1), Ellipse(e2)) => intersect_ellipse_ellipse(*e1, *e2),

        // ---- EllipseArc pairs: each reduces to the full-ellipse case + sweep filter
        (Line(l),    EllipseArc(ea)) | (EllipseArc(ea), Line(l))  =>
            filter_by_ellipse_arc(intersect_line_ellipse(*l, ea.ellipse), ea),
        (Circle(c),  EllipseArc(ea)) | (EllipseArc(ea), Circle(c)) =>
            filter_by_ellipse_arc(intersect_circle_ellipse(*c, ea.ellipse), ea),
        (Arc(ar),    EllipseArc(ea)) | (EllipseArc(ea), Arc(ar))  =>
            filter_by_arc(filter_by_ellipse_arc(
                intersect_arc_ellipse(*ar, ea.ellipse), ea), ar),
        (Ellipse(e), EllipseArc(ea)) | (EllipseArc(ea), Ellipse(e)) =>
            filter_by_ellipse_arc(intersect_ellipse_ellipse(*e, ea.ellipse), ea),
        (EllipseArc(ea1), EllipseArc(ea2)) =>
            filter_by_ellipse_arc(
                filter_by_ellipse_arc(
                    intersect_ellipse_ellipse(ea1.ellipse, ea2.ellipse), ea1),
                ea2),

        // Polyline ∩ X — per-segment dispatch. Each Polyline segment is
        // either a Line (bulge == 0) or an Arc (bulge != 0). Intersect each
        // surviving segment vs the other geom and concatenate hits.
        // Polyline ∩ Polyline — both sides iterate.
        (Polyline(p), other) => intersect_polyline_other(p, other),
        (other, Polyline(p)) => intersect_polyline_other(p, other),

        // Point ∩ anything: degenerates to "is the point on the curve?" —
        // not used by any tool today. Return empty until needed.
        (Point(_), _) | (_, Point(_)) => Vec::new(),

        // Hatch ∩ anything: the boundary of a hatch is its own polyline
        // dobject and intersection with that is what the user wants. The
        // Hatch entity itself contributes no intersections.
        (Hatch(_), _) | (_, Hatch(_)) => Vec::new(),

        // Spline ∩ anything: not implemented yet — NURBS curve
        // intersection is a non-trivial numerical problem (typically
        // Bézier-subdivision + Newton refinement). Return empty for
        // v1; trim/extend against splines is also gated upstream.
        (Spline(_), _) | (_, Spline(_)) => Vec::new(),

        // Wall ∩ anything — intersect the WALL's CENTERLINE with the
        // other geom. The centerline is the smart-dobject's identity;
        // the visible side lines are derived. fillet/trim/etc. on
        // walls operate through the centerline (per the design), so
        // intersect must too.
        //
        // Wall ∩ Wall = intersect the two centerlines.
        (Wall(w), other) | (other, Wall(w)) => {
            let cl = Geom::Line(w.centerline());
            match other {
                Wall(w2) => intersect(&cl, &Geom::Line(w2.centerline())),
                _        => intersect(&cl, other),
            }
        }

        // Text ∩ anything: text has no curve; trim/extend never need
        // to compute intersections against it. Empty.
        (Text(_), _) | (_, Text(_)) => Vec::new(),

        // Dimension ∩ anything: dimensions are annotations, not
        // boundary curves; no meaningful intersection contribution.
        (Dimension(_), _) | (_, Dimension(_)) => Vec::new(),

        // BlockRef ∩ anything: contents resolve through the Document,
        // which intersect() can't reach. Empty (explode to intersect).
        (BlockRef(_), _) | (_, BlockRef(_)) => Vec::new(),
    }
}

fn filter_by_ellipse_arc(hits: Vec<Vec2>, ea: &EllipseArc) -> Vec<Vec2> {
    hits.into_iter()
        .filter(|p| {
            let t = ea.ellipse.nearest_param(*p);
            ea.contains_param(t)
        })
        .collect()
}

fn filter_by_arc(hits: Vec<Vec2>, arc: &Arc) -> Vec<Vec2> {
    hits.into_iter()
        .filter(|p| arc.contains_angle((*p - arc.center).angle()))
        .collect()
}

/// Polyline ∩ any-other-Geom — iterate the polyline's segments, dispatch
/// each as a Line (bulge == 0) or an Arc (bulge != 0), and concatenate
/// every intersection. Closed polylines also test the closing segment
/// between the last and first vertex.
fn intersect_polyline_other(p: &Polyline, other: &Geom) -> Vec<Vec2> {
    let n = p.vertices.len();
    if n < 2 { return Vec::new(); }
    let mut out: Vec<Vec2> = Vec::new();
    let seg_count = if p.closed { n } else { n - 1 };
    for i in 0..seg_count {
        let v_i  = p.vertices[i];
        let v_n  = p.vertices[(i + 1) % n];
        // bulge of segment i lives on v_i per DXF convention.
        if v_i.bulge.abs() < EPS {
            let seg = Geom::Line(Line { a: v_i.pos, b: v_n.pos });
            out.extend(intersect(&seg, other));
        } else {
            // Bulge → Arc. Math: chord length L, sagitta s = L·bulge/2,
            // radius r = L·(1 + bulge²) / (4·|bulge|); sign(bulge) ⇒ CCW/CW.
            let chord = v_n.pos - v_i.pos;
            let l = chord.len();
            if l < EPS { continue; }
            let b = v_i.bulge;
            let r = l * (1.0 + b * b) / (4.0 * b.abs());
            // Center is perpendicular to chord midpoint, distance d from
            // midpoint where d = r·(1 - bulge²)/(1 + bulge²) along the
            // perpendicular. Sign: bulge > 0 → centre on the LEFT of the
            // chord (CCW arc); bulge < 0 → centre on the RIGHT (CW).
            let mid = (v_i.pos + v_n.pos) * 0.5;
            let perp = chord.perp() / l;
            let d = r * (1.0 - b * b) / (1.0 + b * b);
            let center = mid + perp * (d * b.signum());
            let start_angle = (v_i.pos - center).angle().rem_euclid(std::f64::consts::TAU);
            let end_angle   = (v_n.pos - center).angle().rem_euclid(std::f64::consts::TAU);
            // Sweep is always positive (CCW). For bulge < 0, the SHORTER
            // path is CW from v_i to v_n; reparameterise so the Arc still
            // represents the same swept curve in our CCW convention.
            let raw_sweep = (end_angle - start_angle).rem_euclid(std::f64::consts::TAU);
            let arc = if b > 0.0 {
                Arc { center, radius: r, start_angle,
                      sweep_angle: raw_sweep }
            } else {
                let rev_sweep = std::f64::consts::TAU - raw_sweep;
                Arc { center, radius: r, start_angle: end_angle,
                      sweep_angle: rev_sweep }
            };
            out.extend(intersect(&Geom::Arc(arc), other));
        }
    }
    out
}

// ---------- Line–Line (both treated as segments) ----------------------------
//
// Parametric: P1 + t*(P2-P1) = P3 + s*(P4-P3)
// Solve with 2D cross product (Cramer).

pub fn intersect_line_line(a: Line, b: Line) -> Vec<Vec2> {
    let d1 = a.b - a.a;
    let d2 = b.b - b.a;
    let denom = d1.cross(d2);
    if approx_zero(denom) {
        return vec![];                        // parallel or collinear
    }
    let diff = b.a - a.a;
    let t = diff.cross(d2) / denom;
    let s = diff.cross(d1) / denom;
    if t < -EPS || t > 1.0 + EPS || s < -EPS || s > 1.0 + EPS {
        return vec![];
    }
    vec![a.a + d1 * t]
}

// ---------- Line–Circle -----------------------------------------------------
//
// Substitute P(t) = A + t*D into |P-C|² = r², solve quadratic in t.
// Keep solutions with t ∈ [0,1] (segment, not infinite line).

pub fn intersect_line_circle(line: Line, c: Circle) -> Vec<Vec2> {
    // Perpendicular-distance form. The earlier quadratic-discriminant form used
    // an ABSOLUTE epsilon (`disc < -EPS`); for a TANGENT line the discriminant
    // is ~0 but its magnitude scales with |d|⁴, so on large coordinates — or a
    // line lengthened to 1e6 for edge-mode extend — the float error dwarfs EPS
    // and the tangent was wrongly rejected (a TTR-tangent line wouldn't extend
    // to its circle). Working from the perpendicular distance is stable
    // regardless of the line's length.
    let d  = line.b - line.a;
    let aa = d.dot(d);
    if approx_zero(aa) { return vec![]; }
    let len = aa.sqrt();
    let t0   = (c.center - line.a).dot(d) / aa;   // param of the perpendicular foot
    let foot = line.a + d * t0;
    let pd   = foot.dist(c.center);               // perpendicular distance to centre
    let r    = c.radius;
    // Tangent tolerance relative to the radius (geometric, length-independent).
    let tol  = 1e-6 * r.max(1.0);
    if pd > r + tol { return vec![]; }            // genuine miss
    let half = (r * r - pd * pd).max(0.0).sqrt(); // half-chord length
    let ts: [f64; 2] = if half <= tol {
        [t0, f64::NAN]                            // tangent — single point
    } else {
        let dt = half / len;
        [t0 - dt, t0 + dt]
    };
    let mut out = Vec::with_capacity(2);
    for t in ts {
        if t.is_nan() { continue; }
        if t >= -EPS && t <= 1.0 + EPS {
            out.push(line.a + d * t);
        }
    }
    out
}

// ---------- Circle–Circle ---------------------------------------------------
//
// Classic d/a/h decomposition:
//   d = |C2-C1|
//   a = (r1² - r2² + d²) / (2d)
//   h = sqrt(r1² - a²)
//   midpoint = C1 + a*(C2-C1)/d
//   intersections = midpoint ± h * perp((C2-C1)/d)

pub fn intersect_circle_circle(c1: Circle, c2: Circle) -> Vec<Vec2> {
    let d = c1.center.dist(c2.center);

    if approx_zero(d) {
        return vec![];                        // concentric: ignore (coincident or none)
    }
    if d > c1.radius + c2.radius + EPS {
        return vec![];                        // too far apart
    }
    if d < (c1.radius - c2.radius).abs() - EPS {
        return vec![];                        // one inside the other
    }

    let a   = (c1.radius * c1.radius - c2.radius * c2.radius + d * d) / (2.0 * d);
    let h2  = (c1.radius * c1.radius - a * a).max(0.0);
    let h   = h2.sqrt();
    let dir = (c2.center - c1.center) / d;
    let mid = c1.center + dir * a;

    if h < EPS {
        return vec![mid];                     // tangent
    }
    let off = dir.perp() * h;
    vec![mid + off, mid - off]
}

// ---------- Line–Arc, Arc–Circle, Arc–Arc -----------------------------------
//
// All three reduce to the corresponding circle-based test, then filter the
// hit points by whether their angle falls in each arc's swept range.

pub fn intersect_line_arc(line: Line, arc: Arc) -> Vec<Vec2> {
    let c = Circle { center: arc.center, radius: arc.radius };
    intersect_line_circle(line, c)
        .into_iter()
        .filter(|p| arc.contains_angle((*p - arc.center).angle()))
        .collect()
}

pub fn intersect_arc_circle(arc: Arc, circle: Circle) -> Vec<Vec2> {
    let ac = Circle { center: arc.center, radius: arc.radius };
    intersect_circle_circle(ac, circle)
        .into_iter()
        .filter(|p| arc.contains_angle((*p - arc.center).angle()))
        .collect()
}

pub fn intersect_arc_arc(a: Arc, b: Arc) -> Vec<Vec2> {
    let ca = Circle { center: a.center, radius: a.radius };
    let cb = Circle { center: b.center, radius: b.radius };
    intersect_circle_circle(ca, cb)
        .into_iter()
        .filter(|p| {
            let ang_a = (*p - a.center).angle();
            let ang_b = (*p - b.center).angle();
            a.contains_angle(ang_a) && b.contains_angle(ang_b)
        })
        .collect()
}

// ---------- Ellipse intersections -------------------------------------------
//
// We work in the ellipse's local frame (centre at origin, major along x,
// scaled so the implicit form is x² + y² = 1 — i.e. a unit circle). In that
// frame the other dobject becomes simpler:
//   - a line is still a line (rotated + scaled)
//   - a circle becomes an ellipse (scaled inversely)
//   - another ellipse becomes a rotated/scaled ellipse
// All algorithms then solve a polynomial in t (parameter of the local
// dobject) and emit world-space hits via the inverse transform.

/// Implicit value: `((P-c)·û)² / a² + ((P-c)·v̂)² / b²`. Equals 1 on the
/// ellipse, < 1 inside, > 1 outside.
fn ellipse_implicit(el: &Ellipse, p: Vec2) -> f64 {
    let a = el.semi_major();
    let b = el.semi_minor();
    let q = p - el.center;
    let qu = q.dot(el.u_hat()) / a;
    let qv = q.dot(el.v_hat()) / b;
    qu * qu + qv * qv
}

/// Gradient of `ellipse_implicit` at `p`.
fn ellipse_implicit_grad(el: &Ellipse, p: Vec2) -> Vec2 {
    let a = el.semi_major();
    let b = el.semi_minor();
    let q = p - el.center;
    el.u_hat() * (2.0 * q.dot(el.u_hat()) / (a * a))
        + el.v_hat() * (2.0 * q.dot(el.v_hat()) / (b * b))
}

/// Line ∩ Ellipse — analytical quadratic. Substitute the parametric line
/// `P(s) = A + s·D` into the implicit ellipse equation and solve in `s`,
/// then keep solutions with s ∈ [0, 1] (segment).
pub fn intersect_line_ellipse(line: Line, el: Ellipse) -> Vec<Vec2> {
    let a = el.semi_major();
    let b = el.semi_minor();
    if a < EPS || b < EPS { return Vec::new(); }
    // Project both A and D onto the ellipse axes.
    let u = el.u_hat();
    let v = el.v_hat();
    let a0 = line.a - el.center;
    let d  = line.b - line.a;
    let au = a0.dot(u);
    let av = a0.dot(v);
    let du = d.dot(u);
    let dv = d.dot(v);
    // ((au + s·du) / a)² + ((av + s·dv) / b)² = 1
    let aa = (du * du) / (a * a) + (dv * dv) / (b * b);
    let bb = 2.0 * (au * du / (a * a) + av * dv / (b * b));
    let cc = (au * au) / (a * a) + (av * av) / (b * b) - 1.0;
    if aa.abs() < EPS { return Vec::new(); }
    let disc = bb * bb - 4.0 * aa * cc;
    if disc < -EPS { return Vec::new(); }
    let disc = disc.max(0.0);
    let sq = disc.sqrt();
    let mut out = Vec::with_capacity(2);
    for s in [(-bb - sq) / (2.0 * aa), (-bb + sq) / (2.0 * aa)] {
        if s >= -EPS && s <= 1.0 + EPS {
            out.push(line.a + d * s);
        }
    }
    if out.len() == 2 && out[0].dist(out[1]) < EPS { out.pop(); }
    out
}

/// Circle ∩ Ellipse — find all `t ∈ [0, 2π)` such that the ellipse point
/// `E(t)` is at distance `r` from the circle's centre. Up to 4 hits.
pub fn intersect_circle_ellipse(circle: Circle, el: Ellipse) -> Vec<Vec2> {
    if el.semi_major() < EPS { return Vec::new(); }
    let f = |t: f64| {
        let p = el.point_at(t);
        let d = p - circle.center;
        d.dot(d) - circle.radius * circle.radius
    };
    let fd = |t: f64| {
        let p = el.point_at(t);
        2.0 * (p - circle.center).dot(el.tangent_at(t))
    };
    crate::math::newton_roots_periodic(f, fd, 8)
        .into_iter().map(|t| el.point_at(t)).collect()
}

/// Arc ∩ Ellipse — circle ∩ ellipse, filtered by the arc's swept range.
pub fn intersect_arc_ellipse(arc: Arc, el: Ellipse) -> Vec<Vec2> {
    let c = Circle { center: arc.center, radius: arc.radius };
    intersect_circle_ellipse(c, el).into_iter()
        .filter(|p| arc.contains_angle((*p - arc.center).angle()))
        .collect()
}

/// Ellipse ∩ Ellipse — parametrize one ellipse and find all `t` where the
/// implicit form of the other vanishes. Up to 4 hits.
pub fn intersect_ellipse_ellipse(a: Ellipse, b: Ellipse) -> Vec<Vec2> {
    if a.semi_major() < EPS || b.semi_major() < EPS { return Vec::new(); }
    // f(t)  = F_b(E_a(t)) - 1
    // f'(t) = ∇F_b(E_a(t)) · E_a'(t)
    let f = |t: f64| ellipse_implicit(&b, a.point_at(t)) - 1.0;
    let fd = |t: f64| ellipse_implicit_grad(&b, a.point_at(t)).dot(a.tangent_at(t));
    crate::math::newton_roots_periodic(f, fd, 8)
        .into_iter().map(|t| a.point_at(t)).collect()
}

// ---------- tests -----------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn approx_pt(p: Vec2, x: f64, y: f64) -> bool {
        approx_eq(p.x, x) && approx_eq(p.y, y)
    }

    #[test]
    fn line_line_cross() {
        let pts = intersect_line_line(
            Line { a: Vec2::new(0.0, 0.0),  b: Vec2::new(10.0, 0.0) },
            Line { a: Vec2::new(5.0, -5.0), b: Vec2::new(5.0,  5.0) },
        );
        assert_eq!(pts.len(), 1);
        assert!(approx_pt(pts[0], 5.0, 0.0));
    }

    #[test]
    fn line_line_parallel() {
        let pts = intersect_line_line(
            Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) },
            Line { a: Vec2::new(0.0, 1.0), b: Vec2::new(10.0, 1.0) },
        );
        assert!(pts.is_empty());
    }

    #[test]
    fn line_line_outside_segment() {
        let pts = intersect_line_line(
            Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(2.0, 0.0) },
            Line { a: Vec2::new(5.0, -1.0), b: Vec2::new(5.0, 1.0) },
        );
        assert!(pts.is_empty());
    }

    #[test]
    fn line_circle_two_points() {
        let pts = intersect_line_circle(
            Line { a: Vec2::new(-10.0, 0.0), b: Vec2::new(10.0, 0.0) },
            Circle { center: Vec2::new(0.0, 0.0), radius: 5.0 },
        );
        assert_eq!(pts.len(), 2);
        assert!(approx_pt(pts[0], -5.0, 0.0) || approx_pt(pts[0], 5.0, 0.0));
    }

    #[test]
    fn line_circle_tangent() {
        let pts = intersect_line_circle(
            Line { a: Vec2::new(-10.0, 5.0), b: Vec2::new(10.0, 5.0) },
            Circle { center: Vec2::new(0.0, 0.0), radius: 5.0 },
        );
        assert_eq!(pts.len(), 1);
        assert!(approx_pt(pts[0], 0.0, 5.0));
    }

    #[test]
    fn line_circle_tangent_large_coords_and_long_line() {
        // Regression: a TTR-tangent line lengthened to ~1e6 (edge-mode extend)
        // on large coordinates must still report its tangent point. The old
        // absolute-epsilon discriminant test rejected it.
        let cx = 450.0;
        let cy = -38.0;
        let r = 60.0;
        // Horizontal line tangent to the bottom of the circle (y = cy - r),
        // lengthened far past the drawing like extended_for_edgemode does.
        let y = cy - r;
        let line = Line { a: Vec2::new(cx - 1.0e6, y), b: Vec2::new(cx + 1.0e6, y) };
        let pts = intersect_line_circle(line, Circle { center: Vec2::new(cx, cy), radius: r });
        assert_eq!(pts.len(), 1, "tangent must yield exactly one point, got {pts:?}");
        assert!(approx_pt(pts[0], cx, y), "tangent point was {:?}", pts[0]);
    }

    #[test]
    fn line_circle_miss() {
        let pts = intersect_line_circle(
            Line { a: Vec2::new(-10.0, 10.0), b: Vec2::new(10.0, 10.0) },
            Circle { center: Vec2::new(0.0, 0.0), radius: 5.0 },
        );
        assert!(pts.is_empty());
    }

    #[test]
    fn circle_circle_two_points() {
        let pts = intersect_circle_circle(
            Circle { center: Vec2::new(0.0, 0.0), radius: 5.0 },
            Circle { center: Vec2::new(8.0, 0.0), radius: 5.0 },
        );
        assert_eq!(pts.len(), 2);
        assert!(approx_eq(pts[0].x, 4.0) && approx_eq(pts[1].x, 4.0));
        assert!((pts[0].y - pts[1].y).abs() > EPS);
    }

    #[test]
    fn circle_circle_tangent_external() {
        let pts = intersect_circle_circle(
            Circle { center: Vec2::new(0.0, 0.0), radius: 5.0 },
            Circle { center: Vec2::new(10.0, 0.0), radius: 5.0 },
        );
        assert_eq!(pts.len(), 1);
        assert!(approx_pt(pts[0], 5.0, 0.0));
    }

    #[test]
    fn arc_line_filters_by_angle() {
        // Quarter arc 0°→90°, line crosses the full circle but only
        // the upper-right intersection should survive.
        let arc = Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.0, sweep_angle: TAU / 4.0,
        };
        let line = Line { a: Vec2::new(-10.0, 3.0), b: Vec2::new(10.0, 3.0) };
        let pts = intersect_line_arc(line, arc);
        assert_eq!(pts.len(), 1);
        assert!(pts[0].x > 0.0 && approx_eq(pts[0].y, 3.0));
    }

    #[test]
    fn arc_contains_angle_wrap() {
        // Arc from 350° to 10° (sweep 20°, crosses 0)
        let arc = Arc {
            center: Vec2::ZERO, radius: 1.0,
            start_angle: (350.0_f64).to_radians(),
            sweep_angle: (20.0_f64).to_radians(),
        };
        assert!( arc.contains_angle((0.0_f64).to_radians()));
        assert!( arc.contains_angle((355.0_f64).to_radians()));
        assert!( arc.contains_angle((5.0_f64).to_radians()));
        assert!(!arc.contains_angle((90.0_f64).to_radians()));
    }

    // ---- Ellipse intersection tests -------------------------------------

    #[test]
    fn line_ellipse_two_points_on_major_axis() {
        // Axis-aligned ellipse a=5, b=2. Horizontal line through centre at
        // y=0 must cross at (±5, 0).
        let el = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 };
        let line = Line { a: Vec2::new(-10.0, 0.0), b: Vec2::new(10.0, 0.0) };
        let pts = intersect_line_ellipse(line, el);
        assert_eq!(pts.len(), 2);
        let xs: Vec<f64> = pts.iter().map(|p| p.x).collect();
        assert!(xs.iter().any(|&x| (x - 5.0).abs() < 1e-9));
        assert!(xs.iter().any(|&x| (x + 5.0).abs() < 1e-9));
    }

    #[test]
    fn line_ellipse_tangent() {
        // Line y=2 is tangent to the same ellipse at (0, 2).
        let el = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 };
        let line = Line { a: Vec2::new(-10.0, 2.0), b: Vec2::new(10.0, 2.0) };
        let pts = intersect_line_ellipse(line, el);
        assert_eq!(pts.len(), 1);
        assert!(approx_eq(pts[0].x, 0.0));
        assert!(approx_eq(pts[0].y, 2.0));
    }

    #[test]
    fn line_ellipse_miss() {
        let el = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 };
        let line = Line { a: Vec2::new(-10.0, 5.0), b: Vec2::new(10.0, 5.0) };
        assert!(intersect_line_ellipse(line, el).is_empty());
    }

    #[test]
    fn circle_ellipse_four_points() {
        // Circle of radius 3 centred at origin intersects the same ellipse
        // (a=5, b=2) at exactly 4 points (symmetric across both axes).
        let el = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 };
        let c  = Circle { center: Vec2::ZERO, radius: 3.0 };
        let pts = intersect_circle_ellipse(c, el);
        assert_eq!(pts.len(), 4);
        // Each must satisfy both x² + y² = 9 and x²/25 + y²/4 = 1.
        for p in &pts {
            assert!(approx_eq(p.x * p.x + p.y * p.y, 9.0));
            assert!((p.x * p.x / 25.0 + p.y * p.y / 4.0 - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn ellipse_ellipse_four_points_rotated() {
        // Same axis-aligned ellipse, plus a 90°-rotated copy of itself —
        // they intersect at four symmetric points.
        let a = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 };
        let b = Ellipse { center: Vec2::ZERO, major: Vec2::new(0.0, 5.0), ratio: 0.4 };
        let pts = intersect_ellipse_ellipse(a, b);
        assert_eq!(pts.len(), 4, "got {} hits", pts.len());
        // Each must lie on both ellipses (implicit value = 1).
        for p in &pts {
            assert!((ellipse_implicit(&a, *p) - 1.0).abs() < 1e-6);
            assert!((ellipse_implicit(&b, *p) - 1.0).abs() < 1e-6);
        }
    }
}
