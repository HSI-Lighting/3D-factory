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

/// Wrap angle to [0, 2π). Snaps results within ~1e-12 of 2π back to 0
/// — without this, an angle of `-1e-17` (a typical rounding wobble) wraps
/// to TAU and then displays as 360°, which is mathematically the same as 0°
/// but breaks `contains_angle` and other == 0 comparisons.
pub fn norm_angle(a: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let r = a.rem_euclid(tau);
    if r >= tau - 1e-12 { 0.0 } else { r }
}

/// Find all roots of a 2π-periodic function `f` in `[0, 2π)` via Newton
/// iteration from `n_seeds` equally-spaced starting points. Returns roots
/// deduplicated within `1e-4` of each other (handles wrap-around at TAU).
///
/// Used for ellipse-specific snap and intersection queries that boil down to
/// "find all t such that g(t) = 0 on the ellipse's parameter circle." For up
/// to k expected roots, use `n_seeds = 2k` (more is robust, costs nothing
/// meaningful — each seed is ≤ 30 Newton steps in the worst case).
pub fn newton_roots_periodic<F, FD>(f: F, fd: FD, n_seeds: usize) -> Vec<f64>
where
    F:  Fn(f64) -> f64,
    FD: Fn(f64) -> f64,
{
    let mut roots: Vec<f64> = Vec::new();
    let tau = std::f64::consts::TAU;
    for i in 0..n_seeds {
        let mut t = (i as f64 / n_seeds as f64) * tau;
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
        // "converge" to a saddle or local extreme that isn't a root of f.
        if !converged || f(t).abs() > 1e-6 { continue; }
        let t = t.rem_euclid(tau);
        if !roots.iter().any(|&r| {
            let d = (t - r).abs();
            d < 1e-4 || (tau - d) < 1e-4
        }) {
            roots.push(t);
        }
    }
    roots
}
