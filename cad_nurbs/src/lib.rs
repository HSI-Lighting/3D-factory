// Pure-Rust 2D NURBS / B-spline curve math.
//
// Standard algorithms from "The NURBS Book" (Piegl & Tiller, 1997):
//   * Algorithm A2.1 — find knot span
//   * Algorithm A2.2 — basis function evaluation
//   * Algorithm A3.1 — B-spline curve point (De Boor via basis sum)
//   * Algorithm A4.1 — NURBS curve point (rational form)
//
// 2D today. Extension to 3D / N-D is mechanical (replace `Vec2` with a
// trait or a generic point type) — kept 2D for v1 to match the rest of
// RUST_CAD. Surfaces (NURBS patches) are out of scope; same algorithms
// in 2D parameter space + 3D control points, separate crate / module
// when the need arises.
//
// Reference test: a quadratic rational NURBS through control points
//   P0 = (1, 0), P1 = (1, 1), P2 = (0, 1)
// with weights [1, sqrt(2)/2, 1] and knots [0,0,0,1,1,1] is an EXACT
// quarter-circle of radius 1 — proves the rational path against a
// shape that B-splines alone (degree-2 polynomial) can only approximate.

/// Minimal 2D point used throughout the NURBS math. Defined locally so
/// the crate stays free of workspace-internal dependencies — that lets
/// `cad_kernel` take `cad_nurbs` as a dep (for `Geom::Spline`) without
/// a circular reference. Convert to/from `cad_kernel::Vec2` at the
/// call site (both are plain {x, y} structs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 { pub x: f64, pub y: f64 }

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self { Self { x, y } }
}

// ============================================================================
//   Knot vector
// ============================================================================
//
// A knot vector is a non-decreasing sequence of parameter values that
// partitions the curve's domain into segments. For a curve of degree p
// with n+1 control points, the knot vector has m+1 = n+p+2 entries.
//
// "Clamped / open uniform" knot vector — the standard choice for CAD —
// repeats the first and last knot value p+1 times so the curve passes
// THROUGH the first and last control points. Interior knots are evenly
// spaced. This is what AutoCAD / LibreCAD / most CAD apps use by
// default when the user just clicks a few points.

#[derive(Clone, Debug)]
pub struct KnotVector {
    /// Non-decreasing knot values. Length = n_control_points + degree + 1.
    knots: Vec<f64>,
    /// Degree of the curve this knot vector belongs to. Cached for
    /// span lookup; the math doesn't need it stored but it's a frequent
    /// argument so we keep it here.
    degree: usize,
}

impl KnotVector {
    /// Construct a clamped / open uniform knot vector for a curve of
    /// the given degree with `n_ctrl` control points. The parameter
    /// domain is [0, 1]; first p+1 knots are 0, last p+1 are 1, and
    /// the (n_ctrl - p - 1) interior knots are evenly spaced.
    ///
    /// Panics if `n_ctrl <= degree` — fewer control points than the
    /// curve's degree leaves no valid spans.
    pub fn clamped_uniform(degree: usize, n_ctrl: usize) -> Self {
        assert!(n_ctrl > degree,
            "NURBS: need n_ctrl > degree (got n_ctrl={} degree={})",
            n_ctrl, degree);
        let m = n_ctrl + degree;       // knot count is m+1
        let mut knots = Vec::with_capacity(m + 1);
        // First p+1 knots = 0.
        for _ in 0..=degree { knots.push(0.0); }
        // Interior knots — evenly spaced in (0, 1).
        // There are (m + 1) - 2 * (degree + 1) = n_ctrl - degree - 1 of them.
        let n_internal = n_ctrl.saturating_sub(degree + 1);
        for i in 1..=n_internal {
            knots.push(i as f64 / (n_internal + 1) as f64);
        }
        // Last p+1 knots = 1.
        for _ in 0..=degree { knots.push(1.0); }
        Self { knots, degree }
    }

    /// Construct from raw values. Caller must ensure non-decreasing
    /// order; this constructor doesn't sort or validate.
    pub fn from_raw(knots: Vec<f64>, degree: usize) -> Self {
        Self { knots, degree }
    }

    pub fn knots(&self) -> &[f64] { &self.knots }
    pub fn degree(&self) -> usize { self.degree }
    pub fn len(&self) -> usize { self.knots.len() }
    pub fn is_empty(&self) -> bool { self.knots.is_empty() }

    /// Parameter domain — `[knots[degree], knots[n - degree - 1]]`. For
    /// a clamped knot vector this is the range over which the curve is
    /// actually defined; values outside return endpoints when clamped.
    pub fn domain(&self) -> (f64, f64) {
        (self.knots[self.degree],
         self.knots[self.knots.len() - self.degree - 1])
    }

    /// Algorithm A2.1 — find the knot span index `i` such that
    /// `knots[i] <= u < knots[i+1]`. Handles the right-endpoint edge
    /// case (u == knots[n - degree]) by returning the last interior
    /// span, so curve evaluation at u_max gives the last control point
    /// as expected for a clamped curve.
    ///
    /// Linear scan is fine for v1 (knot vectors are small). Binary
    /// search drop-in when n_ctrl > ~1000.
    pub fn find_span(&self, u: f64, n_ctrl: usize) -> usize {
        let p = self.degree;
        // High-end edge case — return the last meaningful span.
        if u >= self.knots[n_ctrl] {
            return n_ctrl - 1;
        }
        if u <= self.knots[p] {
            return p;
        }
        // Linear scan; replace with binary search if profiling shows it.
        let mut i = p;
        while i + 1 < self.knots.len() && self.knots[i + 1] <= u {
            i += 1;
        }
        i
    }
}

// ============================================================================
//   B-spline basis functions  (Algorithm A2.2 in The NURBS Book)
// ============================================================================

/// Evaluate all p+1 non-zero B-spline basis functions at parameter `u`
/// for the span index `span` returned by `KnotVector::find_span`.
/// Returns `N[0..=p]` where `N[i]` corresponds to the (span - p + i)-th
/// control point. Algorithm A2.2 — the iterative Cox-de Boor recursion
/// avoiding the obvious O(p²) overhead of the naïve recursive form.
fn basis_funs(span: usize, u: f64, degree: usize, knots: &[f64]) -> Vec<f64> {
    let p = degree;
    let mut n = vec![0.0_f64; p + 1];
    n[0] = 1.0;
    let mut left  = vec![0.0_f64; p + 1];
    let mut right = vec![0.0_f64; p + 1];
    for j in 1..=p {
        left[j]  = u - knots[span + 1 - j];
        right[j] = knots[span + j] - u;
        let mut saved = 0.0;
        for r in 0..j {
            let denom = right[r + 1] + left[j - r];
            // Knots can coincide at the endpoints of a clamped curve; the
            // basis is well-defined in the limit (0/0 → 0) so we just
            // skip the term.
            let temp = if denom.abs() < 1e-18 { 0.0 } else { n[r] / denom };
            n[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        n[j] = saved;
    }
    n
}

// ============================================================================
//   Non-rational B-spline curve  (Algorithm A3.1)
// ============================================================================

/// 2D B-spline curve (non-rational — all weights implicitly 1.0).
/// Adding rational weights gives a `NurbsCurve` which delegates here
/// in 3D homogeneous space then projects back.
#[derive(Clone, Debug)]
pub struct BSplineCurve {
    pub degree:         usize,
    pub control_points: Vec<Vec2>,
    pub knots:          KnotVector,
}

impl BSplineCurve {
    /// Build a clamped/open uniform B-spline from the given control
    /// points and degree. The knot vector is derived automatically —
    /// most common case for CAD input.
    pub fn new_clamped(degree: usize, control_points: Vec<Vec2>) -> Self {
        let n_ctrl = control_points.len();
        let knots = KnotVector::clamped_uniform(degree, n_ctrl);
        Self { degree, control_points, knots }
    }

    /// Build with a caller-supplied knot vector. Use this for advanced
    /// cases (non-uniform spacing, knot insertion results, etc.).
    /// Panics if the knot vector length doesn't match n_ctrl + p + 1.
    pub fn new(degree: usize, control_points: Vec<Vec2>, knots: KnotVector) -> Self {
        assert_eq!(knots.len(), control_points.len() + degree + 1,
            "NURBS: knot vector length mismatch");
        Self { degree, control_points, knots }
    }

    /// Curve domain (start..=end parameter values).
    pub fn domain(&self) -> (f64, f64) { self.knots.domain() }

    /// Evaluate the curve at parameter `u` — Algorithm A3.1. `u` is
    /// clamped to the curve's domain so callers don't have to worry
    /// about off-end queries during tessellation.
    pub fn evaluate(&self, u: f64) -> Vec2 {
        let (u_min, u_max) = self.domain();
        let u = u.clamp(u_min, u_max);
        let n_ctrl = self.control_points.len();
        let span = self.knots.find_span(u, n_ctrl);
        let n = basis_funs(span, u, self.degree, self.knots.knots());
        let mut sum = Vec2::new(0.0, 0.0);
        for i in 0..=self.degree {
            let ctrl = self.control_points[span - self.degree + i];
            sum.x += n[i] * ctrl.x;
            sum.y += n[i] * ctrl.y;
        }
        sum
    }

    /// Tessellate to `n_samples` evenly-spaced points across the
    /// parameter domain (inclusive on both ends). 0 or 1 samples
    /// return a single-point or empty Vec respectively.
    pub fn tessellate(&self, n_samples: usize) -> Vec<Vec2> {
        if n_samples == 0 { return Vec::new(); }
        if n_samples == 1 {
            let (u_min, _) = self.domain();
            return vec![self.evaluate(u_min)];
        }
        let (u_min, u_max) = self.domain();
        let mut out = Vec::with_capacity(n_samples);
        for i in 0..n_samples {
            let t = i as f64 / (n_samples - 1) as f64;
            out.push(self.evaluate(u_min + (u_max - u_min) * t));
        }
        out
    }
}

// ============================================================================
//   Rational NURBS curve  (Algorithm A4.1)
// ============================================================================

/// 2D NURBS — a B-spline curve with one positive weight per control
/// point. Evaluation is the rational form
///   C(u) = Σ Ni(u) * wi * Pi   /   Σ Ni(u) * wi
/// which is mathematically equivalent to evaluating a B-spline in
/// homogeneous 3D space (wi*Pi.x, wi*Pi.y, wi) then projecting by
/// dividing through by the homogeneous w coordinate.
///
/// Rational form is what lets NURBS represent EXACT conics (circles,
/// ellipses, parabolas, hyperbolas) that plain polynomial B-splines
/// can only approximate.
#[derive(Clone, Debug)]
pub struct NurbsCurve {
    pub bspline: BSplineCurve,
    /// One positive weight per control point. weights.len() must equal
    /// bspline.control_points.len(). A weight of 1.0 everywhere
    /// reduces to a plain B-spline.
    pub weights: Vec<f64>,
}

impl NurbsCurve {
    pub fn new(bspline: BSplineCurve, weights: Vec<f64>) -> Self {
        assert_eq!(weights.len(), bspline.control_points.len(),
            "NURBS: weight count must match control-point count");
        Self { bspline, weights }
    }

    /// Convenience: clamped/open uniform NURBS with the given degree,
    /// control points, and weights. Same shape as
    /// `BSplineCurve::new_clamped` with an extra weight per point.
    pub fn new_clamped(degree: usize, control_points: Vec<Vec2>, weights: Vec<f64>) -> Self {
        Self::new(BSplineCurve::new_clamped(degree, control_points), weights)
    }

    pub fn domain(&self) -> (f64, f64) { self.bspline.domain() }

    /// Evaluate the NURBS curve at parameter `u` — Algorithm A4.1.
    pub fn evaluate(&self, u: f64) -> Vec2 {
        let (u_min, u_max) = self.domain();
        let u = u.clamp(u_min, u_max);
        let p = self.bspline.degree;
        let n_ctrl = self.bspline.control_points.len();
        let span = self.bspline.knots.find_span(u, n_ctrl);
        let n = basis_funs(span, u, p, self.bspline.knots.knots());
        let mut num = Vec2::new(0.0, 0.0);
        let mut den = 0.0_f64;
        for i in 0..=p {
            let idx  = span - p + i;
            let wi   = self.weights[idx];
            let ctrl = self.bspline.control_points[idx];
            let w_ni = n[i] * wi;
            num.x += w_ni * ctrl.x;
            num.y += w_ni * ctrl.y;
            den   += w_ni;
        }
        if den.abs() < 1e-18 {
            // Pathological — all basis * weight contributions sum to zero.
            // Shouldn't happen with positive weights and a valid knot
            // vector; return the first contributing control point as
            // a sensible fallback.
            return self.bspline.control_points[span - p];
        }
        Vec2::new(num.x / den, num.y / den)
    }

    pub fn tessellate(&self, n_samples: usize) -> Vec<Vec2> {
        if n_samples == 0 { return Vec::new(); }
        if n_samples == 1 {
            let (u_min, _) = self.domain();
            return vec![self.evaluate(u_min)];
        }
        let (u_min, u_max) = self.domain();
        let mut out = Vec::with_capacity(n_samples);
        for i in 0..n_samples {
            let t = i as f64 / (n_samples - 1) as f64;
            out.push(self.evaluate(u_min + (u_max - u_min) * t));
        }
        out
    }
}

// ============================================================================
//   Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool { (a - b).abs() < tol }
    fn approx_v(a: Vec2, b: Vec2, tol: f64) -> bool {
        approx(a.x, b.x, tol) && approx(a.y, b.y, tol)
    }

    #[test]
    fn clamped_knot_vector_shape() {
        // Cubic (degree 3) with 5 control points → knot vector length 9
        // = [0,0,0,0, 0.5, 1,1,1,1].
        let kv = KnotVector::clamped_uniform(3, 5);
        assert_eq!(kv.len(), 9);
        assert_eq!(&kv.knots()[..4], &[0.0, 0.0, 0.0, 0.0]);
        assert!(approx(kv.knots()[4], 0.5, 1e-12));
        assert_eq!(&kv.knots()[5..], &[1.0, 1.0, 1.0, 1.0]);
        let (u_min, u_max) = kv.domain();
        assert!(approx(u_min, 0.0, 1e-12));
        assert!(approx(u_max, 1.0, 1e-12));
    }

    #[test]
    fn degree_1_bspline_is_a_polyline() {
        // Degree 1 with control polygon = the polyline itself.
        let cps = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        let c = BSplineCurve::new_clamped(1, cps.clone());
        // Endpoints hit the polygon ends.
        assert!(approx_v(c.evaluate(0.0), cps[0], 1e-12));
        assert!(approx_v(c.evaluate(1.0), cps[3], 1e-12));
        // Parameter values are the clamped uniform knot values.
        // Interior knots are at 1/3 and 2/3 for 4 control points,
        // degree 1 → those must land exactly on the middle two ctrls.
        assert!(approx_v(c.evaluate(1.0 / 3.0), cps[1], 1e-12));
        assert!(approx_v(c.evaluate(2.0 / 3.0), cps[2], 1e-12));
    }

    #[test]
    fn endpoint_interpolation_clamped() {
        // Clamped curves pass through first and last control points
        // for ANY degree.
        for &p in &[2_usize, 3, 4] {
            let cps: Vec<Vec2> = (0..=p+1).map(|i| Vec2::new(i as f64, (i as f64).sin())).collect();
            let c = BSplineCurve::new_clamped(p, cps.clone());
            assert!(approx_v(c.evaluate(c.domain().0), cps[0], 1e-12),
                "degree {} start endpoint", p);
            assert!(approx_v(c.evaluate(c.domain().1), *cps.last().unwrap(), 1e-12),
                "degree {} end endpoint", p);
        }
    }

    #[test]
    fn basis_partition_of_unity() {
        // For any valid u in domain, the p+1 non-zero basis functions
        // sum to exactly 1.0 (partition of unity — a defining property
        // of B-spline basis functions).
        let p = 3;
        let n_ctrl = 6;
        let kv = KnotVector::clamped_uniform(p, n_ctrl);
        for &u in &[0.0, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9, 1.0] {
            let span = kv.find_span(u, n_ctrl);
            let n = basis_funs(span, u, p, kv.knots());
            let sum: f64 = n.iter().sum();
            assert!(approx(sum, 1.0, 1e-12),
                "basis at u={} sums to {}", u, sum);
        }
    }

    #[test]
    fn nurbs_quarter_circle_is_exact() {
        // Classic test: quadratic rational NURBS quarter-circle.
        //   P0 = (1, 0)
        //   P1 = (1, 1)
        //   P2 = (0, 1)
        //   weights = [1, sqrt(2)/2, 1]
        //   knots   = [0,0,0, 1,1,1]
        // The curve traces the unit circle from (1,0) to (0,1).
        // Every evaluated point must satisfy x² + y² = 1.
        let cps = vec![
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        let w = vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0];
        let c = NurbsCurve::new_clamped(2, cps.clone(), w);

        // Endpoints exact.
        assert!(approx_v(c.evaluate(0.0), cps[0], 1e-12));
        assert!(approx_v(c.evaluate(1.0), cps[2], 1e-12));

        // Mid-parameter point lies on the circle and bisects the arc.
        let mid = c.evaluate(0.5);
        let r2  = mid.x * mid.x + mid.y * mid.y;
        assert!(approx(r2, 1.0, 1e-12), "midpoint not on circle: r²={}", r2);
        // Quarter-circle midpoint by symmetry = (cos 45°, sin 45°)
        // = (√2/2, √2/2).
        let s = std::f64::consts::FRAC_1_SQRT_2;
        assert!(approx_v(mid, Vec2::new(s, s), 1e-12),
            "midpoint expected (√½, √½), got ({}, {})", mid.x, mid.y);

        // Every sample on the curve must lie on the unit circle.
        for sample in c.tessellate(33) {
            let r2 = sample.x * sample.x + sample.y * sample.y;
            assert!(approx(r2, 1.0, 1e-10),
                "off-circle sample: ({}, {}) r²={}", sample.x, sample.y, r2);
        }
    }

    #[test]
    fn tessellate_count_and_endpoints() {
        let cps = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(3.0, 2.0),
            Vec2::new(4.0, 0.0),
        ];
        let c = BSplineCurve::new_clamped(3, cps.clone());
        let pts = c.tessellate(50);
        assert_eq!(pts.len(), 50);
        assert!(approx_v(pts[0], cps[0], 1e-12));
        assert!(approx_v(*pts.last().unwrap(), *cps.last().unwrap(), 1e-12));
    }

    #[test]
    fn nurbs_with_unit_weights_matches_bspline() {
        // Weight = 1 everywhere reduces NURBS to plain B-spline.
        let cps = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 3.0),
            Vec2::new(3.0, 3.0),
            Vec2::new(4.0, 0.0),
        ];
        let b = BSplineCurve::new_clamped(3, cps.clone());
        let n = NurbsCurve::new_clamped(3, cps, vec![1.0; 4]);
        for k in 0..=20 {
            let u = k as f64 / 20.0;
            let pb = b.evaluate(u);
            let pn = n.evaluate(u);
            assert!(approx_v(pb, pn, 1e-12),
                "B-spline vs unit-weight NURBS diverge at u={}: ({},{}) vs ({},{})",
                u, pb.x, pb.y, pn.x, pn.y);
        }
    }
}
