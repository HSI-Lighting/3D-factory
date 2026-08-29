// 2D vector math with epsilon-aware comparisons.
// All geometry is f64; the UI converts to f32 only for screen pixels.

use std::ops::{Add, Sub, Mul, Div, Neg};

pub const EPS: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub fn new(x: f64, y: f64) -> Self { Self { x, y } }

    pub fn dot(self, o: Vec2) -> f64    { self.x * o.x + self.y * o.y }
    pub fn cross(self, o: Vec2) -> f64  { self.x * o.y - self.y * o.x }
    pub fn len_sq(self) -> f64          { self.dot(self) }
    pub fn len(self) -> f64             { self.len_sq().sqrt() }
    pub fn dist(self, o: Vec2) -> f64   { (self - o).len() }
    pub fn angle(self) -> f64           { self.y.atan2(self.x) }

    pub fn perp(self) -> Vec2           { Vec2::new(-self.y, self.x) }

    pub fn normalized(self) -> Vec2 {
        let l = self.len();
        if l < EPS { self } else { self / l }
    }
}

impl Add for Vec2     { type Output = Vec2; fn add(self, o: Vec2) -> Vec2 { Vec2::new(self.x + o.x, self.y + o.y) } }
impl Sub for Vec2     { type Output = Vec2; fn sub(self, o: Vec2) -> Vec2 { Vec2::new(self.x - o.x, self.y - o.y) } }
impl Neg for Vec2     { type Output = Vec2; fn neg(self) -> Vec2 { Vec2::new(-self.x, -self.y) } }
impl Mul<f64> for Vec2 { type Output = Vec2; fn mul(self, s: f64) -> Vec2 { Vec2::new(self.x * s, self.y * s) } }
impl Div<f64> for Vec2 { type Output = Vec2; fn div(self, s: f64) -> Vec2 { Vec2::new(self.x / s, self.y / s) } }

pub fn approx_eq(a: f64, b: f64) -> bool { (a - b).abs() < EPS }
pub fn approx_zero(a: f64) -> bool       { a.abs() < EPS }

/// The standard miter limit for stroked line joins (the PDF/SVG default): a
/// miter apex further than 4× the half stroke width from the corner falls
/// back to a bevel. Shared by the raster/preview join emulation so every
/// render path miters identically.
pub const JOIN_MITER_LIMIT: f64 = 4.0;

/// The outside-corner wedge geometry for a stroked join at vertex `v` between
/// unit directions `d1` (INTO the vertex) and `d2` (OUT of it), for a stroke
/// of half-width `half_w`. Two butt-joined segments leave a triangular notch
/// on the outside of the turn: `a` and `b` are the outside offset corners of
/// the meeting segments (the triangle `v→a→b` fills the notch — a bevel),
/// and `apex` is the miter point (edge intersection) when it stays within the
/// miter limit. `None` for degenerate corners (collinear or reversed).
pub struct JoinWedge {
    pub a:    Vec2,
    pub b:    Vec2,
    pub apex: Option<Vec2>,
}

pub fn join_wedge(v: Vec2, d1: Vec2, d2: Vec2, half_w: f64) -> Option<JoinWedge> {
    let cross = d1.cross(d2);
    if cross.abs() < EPS {
        return None;
    }
    // A LEFT turn (cross > 0) leaves the notch on the RIGHT of the travel
    // direction, and vice versa — the wedge is always on the OUTSIDE.
    let side = if cross > 0.0 { -1.0 } else { 1.0 };
    let a = v + d1.perp() * (side * half_w);
    let b = v + d2.perp() * (side * half_w);
    let t = (b - a).cross(d2) / cross;
    let apex = a + d1 * t;
    let apex = if (apex - v).len() <= JOIN_MITER_LIMIT * half_w {
        Some(apex)
    } else {
        None
    };
    Some(JoinWedge { a, b, apex })
}

/// Scale-relative geometry tolerance: `1e-6 · max(magnitude, 1)`.
///
/// A fixed absolute epsilon that works near the origin silently REJECTS real
/// intersections / tangents at large coordinates: a residual or half-chord that
/// is "0" geometrically carries floating-point noise proportional to the
/// coordinate magnitude, so at a survey scale (1e6) that noise dwarfs 1e-6 and
/// the gate mis-fires. Scale the tolerance by the geometry's characteristic
/// size (a radius, a semi-major axis) instead. `.max(1.0)` floors it at the
/// origin-scale tolerance for sub-unit geometry.
///
/// `intersect_line_circle` is the exemplar this generalizes (`1e-6 * r.max(1.0)`).
#[inline]
pub fn scaled_tol(magnitude: f64) -> f64 {
    1e-6 * magnitude.max(1.0)
}

/// Dimensionless sine tolerance for parallel / collinear tests. Two directions
/// are parallel (three points collinear) when
/// `|sinθ| = |cross| / (|a|·|b|)` falls below this — compared as
/// `cross.abs() <= PARALLEL_SIN_EPS * |a| * |b|`, a RELATIVE test with NO absolute
/// floor. It is scale-free for both tiny and huge operands.
///
/// ⚠️ Do NOT use `scaled_tol` here: its `.max(1.0)` floor makes the threshold
/// absolute (`1e-6`) for sub-unit length products, which wrongly declares SHORT
/// segments / SMALL triangles at clean angles "degenerate" (B16 FIX 4 regression,
/// mentor-corrected). `1e-9` matches the historical absolute `EPS` at unit scale.
pub const PARALLEL_SIN_EPS: f64 = 1e-9;

/// Wrap angle to [0, 2π). Snaps results within ~1e-12 of 2π back to 0
/// — without this, an angle of `-1e-17` (a typical rounding wobble) wraps
/// to TAU and then displays as 360°, which is mathematically the same as 0°
/// but breaks `contains_angle` and other == 0 comparisons.
pub fn norm_angle(a: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let r = a.rem_euclid(tau);
    if r >= tau - 1e-12 { 0.0 } else { r }
}

/// Union of param intervals `(start, LENGTH)` on a CLOSED curve (period TAU).
/// Merges overlapping/touching intervals and preserves gaps. Returns the merged
/// `(start, len)` list and a `full` flag (whole curve covered). Correct
/// regardless of WRAP: it finds a point inside a GAP, rotates the origin there so
/// nothing wraps, merges in the rotated domain, then rotates back — so a chain
/// or overlap crossing the 0/τ seam is handled like any other (G6).
///
/// Shared by trim (splitting a curve into disjoint pieces) and join (merging
/// user arcs, which may OVERLAP). Fullness is decided SOLELY by "is there a gap
/// anywhere?" — never by a sum-of-lengths shortcut. Two arcs summing past TAU
/// while covering < TAU (e.g. 0°→200° + 100°→300° = 300°) are NOT a full circle.
///
/// ⚠️ Do NOT reintroduce a `total >= TAU` early-return: it is valid only for
/// DISJOINT inputs (trim's) and falsely reports "full" for overlapping inputs
/// (join's). The gap path below is correct for both — with disjoint inputs
/// `sum ≥ τ ⟺ no gap`, so it returns the identical answer trim relied on.
pub(crate) fn circular_union(intervals: &[(f64, f64)]) -> (Vec<(f64, f64)>, bool) {
    let tau = std::f64::consts::TAU;
    let eps = 1e-6;
    if intervals.is_empty() { return (Vec::new(), false); }
    // Normalise starts into [0, TAU); find a point inside a GAP to use as origin
    // so nothing wraps in the rotated domain.
    let mut a: Vec<(f64, f64)> = intervals.iter()
        .map(|&(s, l)| (s.rem_euclid(tau), l)).collect();
    a.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
    let n = a.len();
    let mut origin = 0.0_f64;
    let mut found = false;
    // `cover_end` = the far edge of everything covered so far, a RUNNING MAX — NOT
    // the current interval's own end. With overlapping/NESTED inputs (join's user
    // arcs) a later interval can end BEFORE an earlier one, so per-interval `end_i`
    // reports a FALSE gap inside a covered region; the rotation then strands a
    // nested piece as a spurious interval (B16b bug, mentor-caught). For DISJOINT
    // inputs (trim's) `cover_end == end_i` at every step, so this is identical to
    // before — trim's suite is the guard.
    let mut cover_end = a[0].0 + a[0].1;
    for i in 0..n {
        cover_end = cover_end.max(a[i].0 + a[i].1);
        let next_start = if i + 1 < n { a[i + 1].0 } else { a[0].0 + tau };
        if next_start - cover_end > eps {
            origin = (cover_end + (next_start - cover_end) * 0.5).rem_euclid(tau);
            found = true;
            break;
        }
    }
    if !found { return (Vec::new(), true); }   // no gap anywhere ⇒ full curve
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

/// Find all roots of a 2π-periodic function `f` in `[0, 2π)` via Newton
/// iteration from `n_seeds` equally-spaced starting points. Returns roots
/// deduplicated within `1e-4` of each other (handles wrap-around at TAU).
///
/// Used for ellipse-specific snap and intersection queries that boil down to
/// "find all t such that g(t) = 0 on the ellipse's parameter circle." For up
/// to k expected roots, use `n_seeds = 2k` (more is robust, costs nothing
/// meaningful — each seed is ≤ 30 Newton steps in the worst case).
///
/// `residual_tol` is the accept threshold on `|f(t)|` after convergence (a
/// converged step can still sit on a saddle that isn't a root). It MUST match
/// the UNITS/SCALE of `f`: for a squared-distance residual `|P−c|²−r²` pass
/// `1e-6 · char²` (the residual scales with coordinate²); for a DIMENSIONLESS
/// residual (e.g. an ellipse implicit `F(p)−1`) pass a fixed `1e-6` — scaling a
/// dimensionless residual by char² would over-loosen it and admit false roots.
/// The parameter-space convergence test (1e-12 on the Newton step) is separate
/// and stays absolute (t is an angle in radians, always O(1)).
///
/// `dedup_tol` is the PARAMETER-SPACE distance below which two roots are merged
/// as one. G7: an absolute param tolerance is a scale-DEPENDENT world tolerance,
/// because world separation ≈ `|f-curve derivative| · Δt`. On an ellipse that is
/// `≈ semi_major · Δt`, so a fixed `1e-4` rad merges roots `1e-4 · a` apart in
/// world units — 100 units at `a = 1e6`, silently collapsing two distinct roots.
/// So the caller converts a small WORLD tolerance to param space by dividing by
/// the curve's characteristic derivative magnitude (`≈ a`): `scaled_tol(a) / a`.
/// It is INDEPENDENT of `residual_tol` (different quantity, different units) —
/// even when the residual is a fixed dimensionless `1e-6`, `dedup_tol` must still
/// be a real param-space value. Do NOT collapse it to float precision (`1e-12`):
/// two seeds legitimately converging on ONE root land ~1e-9 apart, and a
/// precision-level dedup would emit both — a duplicate-root bug traded for a
/// merge bug. It must track world scale, not shrink.
pub fn newton_roots_periodic<F, FD>(
    f: F, fd: FD, n_seeds: usize, residual_tol: f64, dedup_tol: f64,
) -> Vec<f64>
where
    F:  Fn(f64) -> f64,
    FD: Fn(f64) -> f64,
{
    let mut roots: Vec<f64> = Vec::new();
    let tau = std::f64::consts::TAU;
    for i in 0..n_seeds {
        // G7 seeds: phase-shifted by HALF a step so seeds do NOT sit on the exact
        // `i/n · τ` grid. Symmetric geometry places stationary points (`f=f'=0`,
        // where Newton stalls at `fd≈0 → break`) exactly on that grid — precisely
        // where a tangent/double root hides — so a seed landing on one is the
        // worst case. Off-grid seeds approach such roots from the side instead.
        let mut t = ((i as f64 + 0.5) / n_seeds as f64) * tau;
        let mut converged = false;
        for _ in 0..30 {
            let val = f(t);
            let deriv = fd(t);
            if deriv.abs() < EPS { break; }
            let step = val / deriv;
            t -= step;
            if step.abs() < 1e-12 {
                converged = true;
                break;
            }
        }
        // Require both convergence and a residual close to zero — Newton can
        // "converge" to a saddle or local extreme that isn't a root of f. The
        // threshold is scale-relative (caller-supplied) so a squared-distance
        // residual isn't rejected at large coordinates. The parameter-space
        // step test above (1e-12) stays absolute.
        if !converged || f(t).abs() > residual_tol { continue; }
        let t = t.rem_euclid(tau);
        // Dedup in param space with the caller's scale-aware tolerance (see the
        // `dedup_tol` doc above), handling wrap-around at τ.
        if !roots.iter().any(|&r| {
            let d = (t - r).abs();
            d < dedup_tol || (tau - d) < dedup_tol
        }) {
            roots.push(t);
        }
    }
    roots
}

#[cfg(test)]
mod newton_dedup_tests {
    use super::*;

    // G7: `dedup_tol` is threaded through and CONTROLS param-space merging — the
    // pre-fix hardcoded `1e-4` could not be scale-tuned by the caller at all. Two
    // roots π apart survive a small dedup and merge under a large one.
    #[test]
    fn dedup_tol_controls_root_merging() {
        // f = sin t: simple roots at 0 and π, both reliably found from 16 seeds.
        let f  = |t: f64| t.sin();
        let fd = |t: f64| t.cos();
        let two = newton_roots_periodic(f, fd, 16, 1e-6, 0.1);
        assert_eq!(two.len(), 2, "roots π apart must survive a 0.1 dedup: {two:?}");
        let one = newton_roots_periodic(f, fd, 16, 1e-6, 4.0);
        assert_eq!(one.len(), 1, "roots π apart must merge under a 4.0 dedup: {one:?}");
    }

    // The duplicate-root guard the task warns about: many seeds converge onto the
    // same handful of roots, and dedup must collapse them to EXACTLY the distinct
    // set — not emit a copy per seed. A float-precision dedup would break this
    // (seeds land ~1e-9 apart on one root); the scale-aware tol must still merge.
    #[test]
    fn many_seeds_dedup_to_the_distinct_roots() {
        let f  = |t: f64| t.sin();
        let fd = |t: f64| t.cos();
        let roots = newton_roots_periodic(f, fd, 64, 1e-6, 1e-6);
        assert_eq!(roots.len(), 2, "64 seeds, 2 real roots → exactly 2, got {roots:?}");
    }
}

#[cfg(test)]
mod join_wedge_tests {
    use super::*;

    // 90° LEFT turn at the origin, half-width 1: path goes +x then +y. The
    // butt notch is on the right of the travel direction (below +x / right of
    // +y), so the outside corners are (0,-1) and (1,0) and the miter apex is
    // (1,-1) at distance √2 ≤ 4.
    #[test]
    fn left_turn_wedge_sits_on_the_outside() {
        let w = join_wedge(Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), 1.0)
            .expect("a 90° corner must produce a wedge");
        assert!((w.a - Vec2::new(0.0, -1.0)).len() < 1e-9, "a = {:?}", w.a);
        assert!((w.b - Vec2::new(1.0, 0.0)).len() < 1e-9, "b = {:?}", w.b);
        let apex = w.apex.expect("90° miter stays within the limit");
        assert!((apex - Vec2::new(1.0, -1.0)).len() < 1e-9, "apex = {apex:?}");
    }

    // The mirror-image RIGHT turn: the notch is above +x / left of -y.
    #[test]
    fn right_turn_wedge_sits_on_the_other_side() {
        let w = join_wedge(Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(0.0, -1.0), 1.0)
            .expect("a -90° corner must produce a wedge");
        assert!((w.a - Vec2::new(0.0, 1.0)).len() < 1e-9, "a = {:?}", w.a);
        assert!((w.b - Vec2::new(1.0, 0.0)).len() < 1e-9, "b = {:?}", w.b);
        let apex = w.apex.expect("90° miter stays within the limit");
        assert!((apex - Vec2::new(1.0, 1.0)).len() < 1e-9, "apex = {apex:?}");
    }

    // A nearly-REVERSING corner (turn ≈ 175°): the miter apex distance is
    // half_w / sin(θ/2) ≈ 22.9× half-width — far beyond the 4× limit → apex
    // is None (bevel fallback).
    #[test]
    fn sharp_corner_falls_back_to_bevel() {
        let d = Vec2::new((175.0_f64).to_radians().cos(), (175.0_f64).to_radians().sin());
        let w = join_wedge(Vec2::ZERO, Vec2::new(1.0, 0.0), d, 1.0)
            .expect("a 175° reflex corner must still produce a wedge");
        assert!(w.apex.is_none(), "nearly-reversing corners must bevel, got {:?}", w.apex);
    }

    // Collinear segments have no notch.
    #[test]
    fn collinear_corner_has_no_wedge() {
        assert!(join_wedge(Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(1.0, 0.0), 1.0).is_none());
        assert!(join_wedge(Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0), 1.0).is_none());
    }
}
