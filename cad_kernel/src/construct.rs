// Arc construction methods.
//
// Five well-known ways to specify an arc:
//   1. center + radius + start_angle + end_angle    (the canonical form, used by `Arc` directly)
//   2. center + start_point + end_point             (CCW from start to end)
//   3. three points on the arc                      (circumscribed circle of triangle)
//   4. chord (start, end) + radius                  (with major/minor selector)
//   5. chord (start, end) + arc length              (with side selector; needs a numerical solver)
//
// Each is a pure function returning Option<Arc>. None means the inputs were
// degenerate (collinear / chord longer than 2·radius / chord longer than arc length).

use crate::geom::{Arc, Ellipse, Line};
use crate::math::{norm_angle, Vec2, EPS};
use std::f64::consts::{PI, TAU};

// ---- Wall: two parallel lines from a centerline + thickness -----------------

/// Build the two side lines of a wall from its implicit CENTERLINE
/// (`start` → `end`) and `thickness`. The wall is symmetric about the
/// centerline: each side line is offset by `thickness/2` along the
/// perpendicular to the wall direction.
///
/// Returns `(left, right)` where "left" is on the CCW side of the
/// direction (`+perp` of (end - start)) and "right" is the opposite
/// (`-perp`). When `(end - start)` has near-zero length OR `thickness`
/// is non-positive, returns `None` (degenerate wall — caller should
/// treat as no-op).
///
/// Design rationale: keeping the centerline IMPLICIT means a wall is
/// just two normal `Line` dobjects on the canvas — every existing
/// trim/extend/offset/fillet/mirror operation works on the side lines
/// directly with no new infrastructure. The reverse direction (recover
/// the centerline from the two side lines) is trivial via midpoint of
/// the perpendicular between them; useful for a future wall-aware
/// fillet that fillets the centerlines first then re-derives the sides.
pub fn wall_sides(start: Vec2, end: Vec2, thickness: f64) -> Option<(Line, Line)> {
    let dir = end - start;
    let len = dir.len();
    if len < EPS || thickness <= EPS { return None; }
    let perp = (dir / len).perp();
    let off = perp * (thickness * 0.5);
    Some((
        Line { a: start + off, b: end + off },
        Line { a: start - off, b: end - off },
    ))
}

// ---- Ellipse from centre + end-of-major + minor length ---------------------

/// Build a full ellipse from its centre, the world-space END of its semi-major
/// axis (which encodes both rotation direction and semi-major length), and a
/// semi-minor length. Returns `None` for degenerate (zero-length) inputs.
pub fn ellipse_center_major_minor(
    center: Vec2,
    major_end: Vec2,
    semi_minor: f64,
) -> Option<Ellipse> {
    let major = major_end - center;
    let a = major.len();
    if a < EPS || semi_minor < EPS { return None; }
    Some(Ellipse { center, major, ratio: (semi_minor / a).min(1.0) })
}

// ---- 2. center + start + end (CCW) ----------------------------------------

/// Build an arc from a center, a start point, and an end point.
/// Radius is taken from the distance center→start. The end point only contributes its
/// *angle from the center* — its distance is ignored. CCW sweep.
pub fn arc_center_start_end(center: Vec2, start: Vec2, end: Vec2) -> Option<Arc> {
    let radius = center.dist(start);
    if radius < EPS { return None; }
    let start_angle = norm_angle((start - center).angle());
    let end_angle   = (end - center).angle();
    let sweep_raw   = norm_angle(end_angle - start_angle);
    let sweep = if sweep_raw < EPS { TAU } else { sweep_raw };
    Some(Arc { center, radius, start_angle, sweep_angle: sweep })
}

// ---- 3. three points on the arc -------------------------------------------

/// Build an arc passing through p1, p2, p3, in that order. Center is the
/// circumcenter of the triangle. Returns None if the three points are collinear.
pub fn arc_three_points(p1: Vec2, p2: Vec2, p3: Vec2) -> Option<Arc> {
    // Twice the signed area of the triangle.
    let d = 2.0 * (p1.x * (p2.y - p3.y)
                 + p2.x * (p3.y - p1.y)
                 + p3.x * (p1.y - p2.y));
    if d.abs() < EPS { return None; }

    let p1_sq = p1.x * p1.x + p1.y * p1.y;
    let p2_sq = p2.x * p2.x + p2.y * p2.y;
    let p3_sq = p3.x * p3.x + p3.y * p3.y;

    let ux = (p1_sq * (p2.y - p3.y)
            + p2_sq * (p3.y - p1.y)
            + p3_sq * (p1.y - p2.y)) / d;
    let uy = (p1_sq * (p3.x - p2.x)
            + p2_sq * (p1.x - p3.x)
            + p3_sq * (p2.x - p1.x)) / d;
    let center = Vec2::new(ux, uy);
    let radius = center.dist(p1);

    let a1 = (p1 - center).angle();
    let a2 = (p2 - center).angle();
    let a3 = (p3 - center).angle();

    let ccw_total  = norm_angle(a3 - a1);
    let ccw_to_mid = norm_angle(a2 - a1);

    if ccw_to_mid <= ccw_total + EPS {
        // CCW from p1 through p2 to p3 works.
        Some(Arc {
            center,
            radius,
            start_angle: norm_angle(a1),
            sweep_angle: if ccw_total < EPS { TAU } else { ccw_total },
        })
    } else {
        // CCW from p1 misses p2 → the natural arc is CW.
        // Represent it as CCW from p3 back to p1 (sweeps through p2 going the other way).
        Some(Arc {
            center,
            radius,
            start_angle: norm_angle(a3),
            sweep_angle: TAU - ccw_total,
        })
    }
}

// ---- 4. chord + radius ----------------------------------------------------

/// Build an arc with the given start and end points and the given radius.
/// Two arcs satisfy these constraints (one minor, one major); `major = true`
/// returns the one with sweep > π, `false` returns sweep < π.
/// Returns None if the chord is longer than 2·radius or the radius is zero.
pub fn arc_chord_radius(start: Vec2, end: Vec2, radius: f64, major: bool) -> Option<Arc> {
    if radius < EPS { return None; }
    let chord = end - start;
    let chord_len = chord.len();
    if chord_len < EPS { return None; }
    let half_chord = chord_len * 0.5;
    if half_chord > radius + EPS { return None; }

    let mid  = (start + end) * 0.5;
    let h_sq = (radius * radius - half_chord * half_chord).max(0.0);
    let h    = h_sq.sqrt();
    let perp = chord.normalized().perp();

    let from_center = |center: Vec2| -> Arc {
        let sa = (start - center).angle();
        let ea = (end - center).angle();
        let sweep = norm_angle(ea - sa);
        Arc {
            center,
            radius,
            start_angle: norm_angle(sa),
            sweep_angle: if sweep < EPS { TAU } else { sweep },
        }
    };

    let arc_a = from_center(mid + perp * h);
    let arc_b = from_center(mid - perp * h);

    // Exactly one of the two has sweep > π (the "major" arc).
    let arc_a_is_major = arc_a.sweep_angle > PI;
    Some(if arc_a_is_major == major { arc_a } else { arc_b })
}

// ---- 5. chord + arc length (numerical) ------------------------------------

/// Build an arc with the given start and end points and the given total arc length.
/// The radius is determined numerically by solving  sin(θ/2)/(θ/2) = chord / length
/// for the sweep angle θ ∈ (0, 2π), then radius = length / θ.
///
/// Two arcs satisfy these constraints (mirror images across the chord); `flip = false`
/// places the center to the LEFT of the chord direction (start→end), `flip = true` to the right.
///
/// Returns None if the chord is longer than the arc length (impossible), or zero length.
pub fn arc_chord_length(start: Vec2, end: Vec2, arc_length: f64, flip: bool) -> Option<Arc> {
    if arc_length < EPS { return None; }
    let chord_len = (end - start).len();
    if chord_len < EPS { return None; }
    if chord_len > arc_length + EPS { return None; }

    // ratio = chord / arc_length = sin(θ/2) / (θ/2)  ∈ (0, 1]
    let ratio = chord_len / arc_length;

    // sinc(θ/2) is strictly decreasing on (0, 2π), so bisection has a unique root.
    // f(θ) = sin(θ/2)/(θ/2) - ratio  →  positive at θ → 0, negative at θ → 2π.
    let f = |theta: f64| -> f64 {
        let x = theta * 0.5;
        if x < EPS { 1.0 - ratio } else { x.sin() / x - ratio }
    };

    let mut lo = 1e-9_f64;
    let mut hi = TAU - 1e-9_f64;
    // If chord ≈ length the answer is θ → 0, which is a degenerate straight line.
    // Treat it as no arc to construct.
    if f(lo) < 0.0 { return None; }
    for _ in 0..100 {
        let mid = 0.5 * (lo + hi);
        if f(mid) > 0.0 { lo = mid; } else { hi = mid; }
        if (hi - lo).abs() < 1e-13 { break; }
    }
    let theta = 0.5 * (lo + hi);
    let radius = arc_length / theta;

    // Now we have radius + chord → two candidate arcs (major flag matters).
    // Because θ ∈ (0, 2π) is the *actual* sweep, we know whether this is major (θ > π) or minor.
    let want_major = theta > PI;
    // The `flip` flag mirrors across the chord:
    //   - flip = false → center on the LEFT of (start→end) direction
    //   - flip = true  → center on the RIGHT
    // arc_chord_radius's center selection (mid + perp*h vs mid - perp*h) corresponds to the
    // same left/right convention via `major` matching the sweep, so we combine the two.
    arc_chord_radius(start, end, radius, want_major ^ flip)
}

// ---- tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::approx_eq;

    fn approx_pt(p: Vec2, x: f64, y: f64) -> bool {
        approx_eq(p.x, x) && approx_eq(p.y, y)
    }

    #[test]
    fn center_start_end_quarter_arc() {
        let a = arc_center_start_end(
            Vec2::new(0.0, 0.0),
            Vec2::new(5.0, 0.0),      // start at 0°
            Vec2::new(0.0, 5.0),      // end   at 90°
        ).unwrap();
        assert!(approx_pt(a.center, 0.0, 0.0));
        assert!(approx_eq(a.radius, 5.0));
        assert!(approx_eq(a.start_angle, 0.0));
        assert!(approx_eq(a.sweep_angle, PI * 0.5));
    }

    #[test]
    fn three_points_quarter_arc() {
        // p1=(5,0), p2=(5/√2, 5/√2)=(3.536...), p3=(0,5) on a circle of radius 5
        let s2 = 5.0 / 2.0_f64.sqrt();
        let a = arc_three_points(
            Vec2::new(5.0, 0.0),
            Vec2::new(s2, s2),
            Vec2::new(0.0, 5.0),
        ).unwrap();
        assert!(approx_pt(a.center, 0.0, 0.0));
        assert!(approx_eq(a.radius, 5.0));
        assert!(approx_eq(a.sweep_angle, PI * 0.5));
    }

    #[test]
    fn three_points_collinear_returns_none() {
        let r = arc_three_points(
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
        );
        assert!(r.is_none());
    }

    #[test]
    fn three_points_picks_long_way_when_needed() {
        // p1=(5,0), p2=(-5,0)  is 180° — both arcs equal,
        // pick p2 below: p2=(0,-5) → arc goes CW (270° from (5,0) through (0,-5) to (-5,0)? no, 180°)
        // Actually p1=(5,0) at 0°, p3=(-5,0) at 180°. p2=(0,-5) at 270°.
        // Going CCW from p1 to p3: sweep 180° passes through 90° = (0,5). p2=(0,-5) is NOT on this path.
        // So the constructor must flip and represent the arc as CCW from p3 to p1 (sweep 180°,
        // passes through 270° = p2). Both sweeps are 180° in this symmetric case — still valid.
        let a = arc_three_points(
            Vec2::new(5.0, 0.0),
            Vec2::new(0.0, -5.0),
            Vec2::new(-5.0, 0.0),
        ).unwrap();
        assert!(approx_eq(a.radius, 5.0));
        assert!(approx_pt(a.center, 0.0, 0.0));
        // p2 must lie within the swept arc
        let p2_angle = (Vec2::new(0.0, -5.0) - a.center).angle();
        let to_p2 = norm_angle(p2_angle - a.start_angle);
        assert!(to_p2 <= a.sweep_angle + EPS);
    }

    #[test]
    fn chord_radius_minor() {
        // chord from (-3,0) to (3,0), radius 5 → minor arc with sweep < π
        let a = arc_chord_radius(
            Vec2::new(-3.0, 0.0),
            Vec2::new( 3.0, 0.0),
            5.0,
            false,
        ).unwrap();
        assert!(approx_eq(a.radius, 5.0));
        assert!(a.sweep_angle < PI);
    }

    #[test]
    fn chord_radius_major() {
        let a = arc_chord_radius(
            Vec2::new(-3.0, 0.0),
            Vec2::new( 3.0, 0.0),
            5.0,
            true,
        ).unwrap();
        assert!(a.sweep_angle > PI);
    }

    #[test]
    fn chord_radius_chord_too_long_is_none() {
        let r = arc_chord_radius(
            Vec2::new(-10.0, 0.0),
            Vec2::new( 10.0, 0.0),
            5.0,         // chord 20 > 2·5
            false,
        );
        assert!(r.is_none());
    }

    #[test]
    fn chord_length_quarter_arc_round_trip() {
        // A quarter-circle of radius 5: arc length = 5 * π/2 ≈ 7.854
        // chord = 5·√2 ≈ 7.071, from (5,0) to (0,5)
        let arc_len = 5.0 * PI * 0.5;
        let a = arc_chord_length(
            Vec2::new(5.0, 0.0),
            Vec2::new(0.0, 5.0),
            arc_len,
            false,
        ).unwrap();
        // numerical, so allow a looser tolerance
        assert!((a.radius - 5.0).abs() < 1e-8,
            "radius {} ≠ 5.0", a.radius);
        assert!((a.sweep_angle - PI * 0.5).abs() < 1e-8,
            "sweep {} ≠ π/2", a.sweep_angle);
    }

    #[test]
    fn chord_length_impossible_is_none() {
        // chord 10, arc length 5 — impossible (arc ≥ chord)
        let r = arc_chord_length(
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            5.0,
            false,
        );
        assert!(r.is_none());
    }

    #[test]
    fn wall_sides_horizontal() {
        // Horizontal centerline (0,0)→(10,0), thickness 2 → sides at y=±1.
        let (l, r) = wall_sides(
            Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), 2.0,
        ).expect("non-degenerate");
        // dir = (1,0); perp = (0,1) (CCW); off = (0,1).
        // left  = (0,1) → (10,1); right = (0,-1) → (10,-1).
        assert!((l.a - Vec2::new(0.0,  1.0)).len() < 1e-12);
        assert!((l.b - Vec2::new(10.0, 1.0)).len() < 1e-12);
        assert!((r.a - Vec2::new(0.0, -1.0)).len() < 1e-12);
        assert!((r.b - Vec2::new(10.0,-1.0)).len() < 1e-12);
    }

    #[test]
    fn wall_sides_diagonal_preserves_spacing() {
        // 45° wall; verify the two side lines are still `thickness` apart
        // when measured perpendicular to the centerline.
        let s = Vec2::new(0.0, 0.0);
        let e = Vec2::new(7.0, 7.0);
        let thk = 3.0;
        let (l, r) = wall_sides(s, e, thk).expect("non-degenerate");
        // Distance between corresponding endpoints = thickness.
        assert!(((l.a - r.a).len() - thk).abs() < 1e-12);
        assert!(((l.b - r.b).len() - thk).abs() < 1e-12);
    }

    #[test]
    fn wall_sides_degenerate_returns_none() {
        // Zero-length centerline.
        assert!(wall_sides(
            Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0), 1.0).is_none());
        // Non-positive thickness.
        assert!(wall_sides(
            Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), 0.0).is_none());
        assert!(wall_sides(
            Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), -1.0).is_none());
    }
}
