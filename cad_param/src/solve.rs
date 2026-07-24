//! The constraint solver — damped least squares (Levenberg–Marquardt) — plus
//! degrees-of-freedom analysis (the "fully defined / under-defined" diagnosis).
//!
//! Unknowns are the flat parameter vector `x = [p0.x, p0.y, p1.x, …, s0, s1, …]`
//! (point coordinates first, then scalars such as radii — see [`crate::model`]).
//! Each constraint contributes residual equations `r(x)`; we minimise `‖r‖²` by
//! Gauss–Newton with Marquardt damping:
//!
//! ```text
//!   (JᵀJ + λ·diag(JᵀJ)) Δx = −Jᵀr
//! ```
//!
//! The Jacobian `J` is numerical (central differences) — robust and enough for
//! sketch sizes. The dense linear solve is Gaussian elimination with partial
//! pivoting, implemented here (no external deps).
//!
//! [`dof_analysis`] computes the honest degrees of freedom by the RANK of `J`
//! (the same idea SolveSpace/planegcs use): `dof = free_params − rank(J)`, and
//! flags which individual parameters remain free so the UI can paint each entity
//! blue (under-defined) or black (fully defined).

use crate::model::{Constraint, Sketch};

/// Outcome of a solve.
#[derive(Clone, Copy, Debug)]
pub struct SolveReport {
    pub converged: bool,
    pub iterations: usize,
    /// RMS residual at the end (≈0 when satisfied).
    pub residual: f64,
    /// Naive degrees of freedom (see `Sketch::dof`).
    pub dof: i64,
}

/// Result of [`dof_analysis`] — the rank-honest "fully defined" diagnosis.
#[derive(Clone, Debug)]
pub struct DofReport {
    /// Free parameters minus Jacobian rank. 0 ⇒ fully defined.
    pub dof: i64,
    /// Rank of the constraint Jacobian at the current configuration.
    pub rank: usize,
    /// Number of free (non-anchored) parameters.
    pub free_params: usize,
    /// True when there are more constraint equations than rank — i.e. redundant
    /// (or conflicting) constraints.
    pub redundant: bool,
    /// True when `dof == 0` — every entity is locked down.
    pub fully_defined: bool,
    /// Per-parameter freedom flag, length = `sketch.param_count()`. `true` means
    /// that parameter still has freedom (drives the blue colouring of entities).
    pub param_free: Vec<bool>,
}

#[inline]
fn pt(x: &[f64], i: usize) -> (f64, f64) {
    (x[2 * i], x[2 * i + 1])
}

/// Residual vector for the sketch at parameter vector `x`
/// (len = `2·points + scalars`).
pub fn residuals(s: &Sketch, x: &[f64]) -> Vec<f64> {
    let np = s.points.len();
    let sc = |j: usize| x[2 * np + j];
    let line_pts = |l: crate::model::Line| (pt(x, l.a), pt(x, l.b));
    let circ = |c: crate::model::Circle| (pt(x, c.center), sc(c.radius));
    let mut r = Vec::with_capacity(s.residual_dim());
    for c in &s.constraints {
        match *c {
            Constraint::Fixed { p, x: fx, y: fy } => {
                let (px, py) = pt(x, p);
                r.push(px - fx);
                r.push(py - fy);
            }
            Constraint::Coincident { p, q } => {
                let (px, py) = pt(x, p);
                let (qx, qy) = pt(x, q);
                r.push(px - qx);
                r.push(py - qy);
            }
            Constraint::Distance { p, q, d } => {
                let (px, py) = pt(x, p);
                let (qx, qy) = pt(x, q);
                r.push(((px - qx).powi(2) + (py - qy).powi(2)).sqrt() - d);
            }
            Constraint::PointOnLine { p, line } => {
                let (px, py) = pt(x, p);
                let ((ax, ay), (bx, by)) = line_pts(s.lines[line]);
                let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt().max(1e-9);
                // perpendicular distance of p from the infinite line (length units,
                // NOT area — so it scales linearly and stays well-conditioned)
                r.push(((bx - ax) * (py - ay) - (by - ay) * (px - ax)) / len);
            }
            Constraint::Symmetric { p, q, line } => {
                let (px, py) = pt(x, p);
                let (qx, qy) = pt(x, q);
                let ((ax, ay), (bx, by)) = line_pts(s.lines[line]);
                let (dx, dy) = (bx - ax, by - ay);
                let len = (dx * dx + dy * dy).sqrt().max(1e-9);
                // midpoint lies on the line (distance, normalized)
                let (mx, my) = ((px + qx) * 0.5, (py + qy) * 0.5);
                r.push((dx * (my - ay) - dy * (mx - ax)) / len);
                // p→q is perpendicular to the line direction (projection, normalized)
                r.push((dx * (qx - px) + dy * (qy - py)) / len);
            }
            Constraint::Horizontal { line } => {
                let ((_, ay), (_, by)) = line_pts(s.lines[line]);
                r.push(ay - by);
            }
            Constraint::Vertical { line } => {
                let ((ax, _), (bx, _)) = line_pts(s.lines[line]);
                r.push(ax - bx);
            }
            Constraint::Parallel { a, b } => {
                let ((ax, ay), (bx, by)) = line_pts(s.lines[a]);
                let ((cx, cy), (dx, dy)) = line_pts(s.lines[b]);
                let (ux, uy) = (bx - ax, by - ay);
                let (vx, vy) = (dx - cx, dy - cy);
                let nrm = (ux * ux + uy * uy).sqrt().max(1e-9) * (vx * vx + vy * vy).sqrt().max(1e-9);
                // sin(angle) — dimensionless, so it stays well-scaled at any
                // coordinate magnitude (the raw cross product is area-scale and
                // makes the solver diverge on large drawings).
                r.push((ux * vy - uy * vx) / nrm);
            }
            Constraint::Perpendicular { a, b } => {
                let ((ax, ay), (bx, by)) = line_pts(s.lines[a]);
                let ((cx, cy), (dx, dy)) = line_pts(s.lines[b]);
                let (ux, uy) = (bx - ax, by - ay);
                let (vx, vy) = (dx - cx, dy - cy);
                let nrm = (ux * ux + uy * uy).sqrt().max(1e-9) * (vx * vx + vy * vy).sqrt().max(1e-9);
                r.push((ux * vx + uy * vy) / nrm); // cos(angle), dimensionless
            }
            Constraint::Collinear { a, b } => {
                let ((ax, ay), (bx, by)) = line_pts(s.lines[a]);
                let ((cx, cy), (dx, dy)) = line_pts(s.lines[b]);
                let (ux, uy) = (bx - ax, by - ay);
                let (vx, vy) = (dx - cx, dy - cy);
                let lu = (ux * ux + uy * uy).sqrt().max(1e-9);
                let lv = (vx * vx + vy * vy).sqrt().max(1e-9);
                // parallel (sin) …
                r.push((ux * vy - uy * vx) / (lu * lv));
                // … and b's first endpoint lies on infinite line a (distance)
                r.push((ux * (cy - ay) - uy * (cx - ax)) / lu);
            }
            Constraint::EqualLength { a, b } => {
                let ((ax, ay), (bx, by)) = line_pts(s.lines[a]);
                let ((cx, cy), (dx, dy)) = line_pts(s.lines[b]);
                let la = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
                let lb = ((dx - cx).powi(2) + (dy - cy).powi(2)).sqrt();
                r.push(la - lb);
            }
            Constraint::Angle { a, b, radians } => {
                let ((ax, ay), (bx, by)) = line_pts(s.lines[a]);
                let ((cx, cy), (dx, dy)) = line_pts(s.lines[b]);
                let (ux, uy) = (bx - ax, by - ay);
                let (vx, vy) = (dx - cx, dy - cy);
                let cross = ux * vy - uy * vx;
                let dot = ux * vx + uy * vy;
                // wrap the angle error into (−π, π] so e.g. 179° vs −179° is small
                let mut e = cross.atan2(dot) - radians;
                while e > std::f64::consts::PI { e -= 2.0 * std::f64::consts::PI; }
                while e < -std::f64::consts::PI { e += 2.0 * std::f64::consts::PI; }
                r.push(e);
            }
            Constraint::Radius { circle, r: rr } => {
                let (_, rad) = circ(s.circles[circle]);
                r.push(rad - rr);
            }
            Constraint::Concentric { a, b } => {
                let ((ax, ay), _) = circ(s.circles[a]);
                let ((bx, by), _) = circ(s.circles[b]);
                r.push(ax - bx);
                r.push(ay - by);
            }
            Constraint::EqualRadius { a, b } => {
                let (_, ra) = circ(s.circles[a]);
                let (_, rb) = circ(s.circles[b]);
                r.push(ra - rb);
            }
            Constraint::PointOnCircle { p, circle } => {
                let (px, py) = pt(x, p);
                let ((cx, cy), rad) = circ(s.circles[circle]);
                r.push(((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - rad);
            }
            Constraint::TangentLineCircle { line, circle } => {
                let ((ax, ay), (bx, by)) = line_pts(s.lines[line]);
                let ((cx, cy), rad) = circ(s.circles[circle]);
                let (dx, dy) = (bx - ax, by - ay);
                let len = (dx * dx + dy * dy).sqrt().max(1e-12);
                let dist = (dx * (cy - ay) - dy * (cx - ax)) / len; // signed
                r.push(dist.abs() - rad);
            }
            Constraint::TangentCircleCircle { a, b, internal } => {
                let ((ax, ay), ra) = circ(s.circles[a]);
                let ((bx, by), rb) = circ(s.circles[b]);
                let d = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
                let target = if internal { (ra - rb).abs() } else { ra + rb };
                r.push(d - target);
            }
        }
    }
    r
}

fn sum_sq(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum()
}

/// Build the flat unknown vector from the sketch (points then scalars), and the
/// `locked` mask (point coords pinned by `Fixed` are hard-eliminated).
fn pack(s: &Sketch) -> (Vec<f64>, Vec<bool>) {
    let np = s.points.len();
    let n = s.param_count();
    let mut x = vec![0.0; n];
    for (i, p) in s.points.iter().enumerate() {
        x[2 * i] = p.x;
        x[2 * i + 1] = p.y;
    }
    for (j, v) in s.scalars.iter().enumerate() {
        x[2 * np + j] = *v;
    }
    let mut locked = vec![false; n];
    for c in &s.constraints {
        if let Constraint::Fixed { p, x: fx, y: fy } = *c {
            if 2 * p + 1 < 2 * np {
                x[2 * p] = fx;
                x[2 * p + 1] = fy;
                locked[2 * p] = true;
                locked[2 * p + 1] = true;
            }
        }
    }
    (x, locked)
}

/// Write a solved unknown vector back into the sketch's points and scalars.
fn unpack(s: &mut Sketch, x: &[f64]) {
    let np = s.points.len();
    for (i, p) in s.points.iter_mut().enumerate() {
        p.x = x[2 * i];
        p.y = x[2 * i + 1];
    }
    for (j, v) in s.scalars.iter_mut().enumerate() {
        *v = x[2 * np + j];
    }
}

/// Numerical Jacobian (m×nf, row-major) over the free columns, central differences.
fn jacobian(s: &Sketch, x: &mut [f64], free: &[usize], m: usize) -> Vec<f64> {
    let nf = free.len();
    let h = 1e-6;
    let mut jac = vec![0.0; m * nf];
    for (k, &j) in free.iter().enumerate() {
        let old = x[j];
        x[j] = old + h;
        let rp = residuals(s, x);
        x[j] = old - h;
        let rm = residuals(s, x);
        x[j] = old;
        for i in 0..m {
            jac[i * nf + k] = (rp[i] - rm[i]) / (2.0 * h);
        }
    }
    jac
}

/// Solve the sketch in place: move its points/scalars to satisfy the constraints
/// as closely as possible. Returns convergence info.
pub fn solve(s: &mut Sketch) -> SolveReport {
    let dof = s.dof();
    let (mut x, locked) = pack(s);
    let n = x.len();

    // HARD-fix: the coordinates named by `Fixed` are pinned and removed from the
    // unknowns. Soft-penalising a Fixed lets the anchor tilt by a hair, which an
    // under-constrained DOF then exploits (sliding off to infinity).
    let free: Vec<usize> = (0..n).filter(|i| !locked[*i]).collect();
    let nf = free.len();

    let mut r = residuals(s, &x);
    let m = r.len();
    if m == 0 || nf == 0 {
        unpack(s, &x);
        let rms = if m == 0 { 0.0 } else { (sum_sq(&r) / m as f64).sqrt() };
        return SolveReport { converged: rms < 1e-6, iterations: 0, residual: rms, dof };
    }

    let mut cost = sum_sq(&r);
    let mut lambda = 1e-3_f64;
    const MAX_ITER: usize = 200;
    const TOL: f64 = 1e-10; // RMS residual

    let mut iters = 0;
    while iters < MAX_ITER {
        iters += 1;
        if (cost / m as f64).sqrt() < TOL {
            break;
        }
        let jac = jacobian(s, &mut x, &free, m);
        // JᵀJ (nf×nf) and Jᵀr (nf) over the FREE coords.
        let mut jtj = vec![0.0; nf * nf];
        let mut jtr = vec![0.0; nf];
        for i in 0..m {
            for a in 0..nf {
                let jia = jac[i * nf + a];
                if jia == 0.0 {
                    continue;
                }
                jtr[a] += jia * r[i];
                for b in 0..nf {
                    jtj[a * nf + b] += jia * jac[i * nf + b];
                }
            }
        }
        // LM step with adaptive damping.
        let mut accepted = false;
        for _try in 0..10 {
            let mut a_mat = jtj.clone();
            for d in 0..nf {
                a_mat[d * nf + d] += lambda * jtj[d * nf + d] + 1e-12; // Marquardt + floor
            }
            let rhs: Vec<f64> = jtr.iter().map(|v| -v).collect();
            if let Some(dx) = solve_linear(a_mat, rhs, nf) {
                let mut x_new = x.clone();
                for (k, &j) in free.iter().enumerate() {
                    x_new[j] = x[j] + dx[k];
                }
                let r_new = residuals(s, &x_new);
                let cost_new = sum_sq(&r_new);
                if cost_new < cost {
                    x = x_new;
                    r = r_new;
                    cost = cost_new;
                    lambda = (lambda * 0.5).max(1e-12);
                    accepted = true;
                    break;
                } else {
                    lambda = (lambda * 4.0).min(1e12);
                }
            } else {
                lambda = (lambda * 4.0).min(1e12);
            }
        }
        if !accepted {
            break; // stuck (singular or no improvement)
        }
    }

    unpack(s, &x);
    let rms = (cost / m as f64).sqrt();
    SolveReport { converged: rms < 1e-6, iterations: iters, residual: rms, dof }
}

/// RMS of all constraint residuals at the sketch's CURRENT positions (no solve).
/// Used by the diagnostics panel to show how far the sketch is from satisfied.
pub fn current_rms(s: &Sketch) -> f64 {
    let (x, _) = pack(s);
    let r = residuals(s, &x);
    if r.is_empty() { 0.0 } else { (sum_sq(&r) / r.len() as f64).sqrt() }
}

/// Per-constraint residual MAGNITUDE at the sketch's current positions, aligned
/// 1:1 with `sketch.constraints`. Lets the recorder show exactly which equation
/// is satisfied (≈0) and which is fighting.
pub fn residual_breakdown(s: &Sketch) -> Vec<f64> {
    let (x, _) = pack(s);
    let r = residuals(s, &x);
    let mut out = Vec::with_capacity(s.constraints.len());
    let mut k = 0;
    for c in &s.constraints {
        let n = c.residual_count();
        let mut ss = 0.0;
        for _ in 0..n {
            if k < r.len() { ss += r[k] * r[k]; k += 1; }
        }
        out.push(ss.sqrt());
    }
    out
}

/// Diagnose degrees of freedom by the RANK of the constraint Jacobian at the
/// sketch's current configuration. `dof = free_params − rank`. Also reports which
/// individual parameters remain free (non-pivot columns) so the UI can colour
/// each entity blue (under-defined) or black (fully defined).
pub fn dof_analysis(s: &Sketch) -> DofReport {
    let (mut x, locked) = pack(s);
    let n = x.len();
    let free: Vec<usize> = (0..n).filter(|i| !locked[*i]).collect();
    let nf = free.len();
    let mut param_free = vec![false; n]; // locked params count as defined
    let m = s.residual_dim();

    if m == 0 || nf == 0 {
        // No constraints (or everything anchored): every free param is free.
        for &j in &free {
            param_free[j] = true;
        }
        return DofReport {
            dof: nf as i64,
            rank: 0,
            free_params: nf,
            redundant: false,
            fully_defined: nf == 0,
            param_free,
        };
    }

    let jac = jacobian(s, &mut x, &free, m);
    // Row-reduce `jac` (m×nf) to find pivot columns; rank = number of pivots.
    let mut a = jac.clone();
    let maxabs = a.iter().fold(0.0_f64, |acc, v| acc.max(v.abs()));
    let tol = 1e-9_f64.max(maxabs * 1e-9);
    // Count rows that actually depend on a FREE param. Rows that are all-zero
    // (e.g. a Fixed/anchor whose coords are locked out) must NOT count toward
    // "redundant", or an anchored-but-unconstrained sketch falsely warns.
    let active_rows = (0..m)
        .filter(|&i| (0..nf).any(|k| jac[i * nf + k].abs() > tol))
        .count();
    let mut pivot_col = vec![false; nf];
    let mut row = 0usize;
    for col in 0..nf {
        if row >= m {
            break;
        }
        // pick the row ≥ `row` with the largest magnitude in this column
        let mut piv = row;
        let mut best = a[row * nf + col].abs();
        for rr in (row + 1)..m {
            let v = a[rr * nf + col].abs();
            if v > best {
                best = v;
                piv = rr;
            }
        }
        if best <= tol {
            continue; // free column (no pivot) — a degree of freedom
        }
        if piv != row {
            for k in 0..nf {
                a.swap(row * nf + k, piv * nf + k);
            }
        }
        let d = a[row * nf + col];
        for rr in 0..m {
            if rr == row {
                continue;
            }
            let f = a[rr * nf + col] / d;
            if f == 0.0 {
                continue;
            }
            for k in col..nf {
                a[rr * nf + k] -= f * a[row * nf + k];
            }
        }
        pivot_col[col] = true;
        row += 1;
    }
    let rank = row;
    for (col, &is_pivot) in pivot_col.iter().enumerate() {
        if !is_pivot {
            param_free[free[col]] = true; // non-pivot free column ⇒ a DOF
        }
    }
    let dof = nf as i64 - rank as i64;
    DofReport {
        dof,
        rank,
        free_params: nf,
        redundant: rank < active_rows,
        fully_defined: dof <= 0,
        param_free,
    }
}

/// Solve a dense `n×n` system `A·x = b` by Gaussian elimination with partial
/// pivoting. `a` is row-major (consumed). Returns None if singular.
fn solve_linear(mut a: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for col in 0..n {
        // partial pivot
        let mut piv = col;
        let mut best = a[col * n + col].abs();
        for r in (col + 1)..n {
            let v = a[r * n + col].abs();
            if v > best {
                best = v;
                piv = r;
            }
        }
        if best < 1e-14 {
            return None;
        }
        if piv != col {
            for k in 0..n {
                a.swap(col * n + k, piv * n + k);
            }
            b.swap(col, piv);
        }
        let d = a[col * n + col];
        for r in (col + 1)..n {
            let f = a[r * n + col] / d;
            if f == 0.0 {
                continue;
            }
            for k in col..n {
                a[r * n + k] -= f * a[col * n + k];
            }
            b[r] -= f * b[col];
        }
    }
    // back-substitution
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for k in (r + 1)..n {
            s -= a[r * n + k] * x[k];
        }
        x[r] = s / a[r * n + r];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Constraint, Sketch};

    fn dist(s: &Sketch, p: usize, q: usize) -> f64 {
        (s.points[p] - s.points[q]).len()
    }

    #[test]
    fn distance_constraint_scales_segment() {
        let mut s = Sketch::new();
        let p0 = s.add_point(0.0, 0.0);
        let p1 = s.add_point(3.0, 4.0); // currently length 5
        s.add(Constraint::Fixed { p: p0, x: 0.0, y: 0.0 });
        s.add(Constraint::Distance { p: p0, q: p1, d: 10.0 });
        let rep = solve(&mut s);
        assert!(rep.converged, "rms={}", rep.residual);
        assert!((dist(&s, p0, p1) - 10.0).abs() < 1e-6);
        assert!(s.points[p0].len() < 1e-6);
    }

    #[test]
    fn perpendicular_makes_right_angle() {
        let mut s = Sketch::new();
        let a = s.add_point(0.0, 0.0);
        let b = s.add_point(10.0, 0.0);
        let c = s.add_point(2.0, 1.0); // skewed
        let l0 = s.add_line(a, b);
        let l1 = s.add_line(a, c);
        s.add(Constraint::Fixed { p: a, x: 0.0, y: 0.0 });
        s.add(Constraint::Fixed { p: b, x: 10.0, y: 0.0 });
        s.add(Constraint::Perpendicular { a: l0, b: l1 });
        let rep = solve(&mut s);
        assert!(rep.converged, "rms={}", rep.residual);
        let u = s.points[b] - s.points[a];
        let v = s.points[c] - s.points[a];
        assert!(u.dot(v).abs() < 1e-6, "dot={}", u.dot(v));
    }

    #[test]
    fn solves_a_rectangle() {
        let mut s = Sketch::new();
        let p0 = s.add_point(0.0, 0.0);
        let p1 = s.add_point(10.0, 0.3);
        let p2 = s.add_point(9.8, 5.0);
        let p3 = s.add_point(0.2, 4.9);
        let l0 = s.add_line(p0, p1);
        let l1 = s.add_line(p1, p2);
        let l2 = s.add_line(p2, p3);
        let l3 = s.add_line(p3, p0);
        s.add(Constraint::Fixed { p: p0, x: 0.0, y: 0.0 });
        s.add(Constraint::Horizontal { line: l0 });
        s.add(Constraint::Vertical { line: l1 });
        s.add(Constraint::Horizontal { line: l2 });
        s.add(Constraint::Vertical { line: l3 });
        s.add(Constraint::Distance { p: p0, q: p1, d: 10.0 });
        s.add(Constraint::Distance { p: p1, q: p2, d: 5.0 });
        let rep = solve(&mut s);
        assert!(rep.converged, "rms={}", rep.residual);
        let close = |a: cad_kernel::Vec2, x: f64, y: f64| (a.x - x).abs() < 1e-5 && (a.y - y).abs() < 1e-5;
        assert!(close(s.points[p0], 0.0, 0.0));
        assert!(close(s.points[p1], 10.0, 0.0));
        assert!(close(s.points[p2], 10.0, 5.0));
        assert!(close(s.points[p3], 0.0, 5.0));
        // fully defined: rank-honest dof is 0
        let d = dof_analysis(&s);
        assert_eq!(d.dof, 0, "{:?}", d);
        assert!(d.fully_defined);
    }

    #[test]
    fn parallel_stays_bounded_at_large_coordinates() {
        // Regression: real drawings use coordinates in the thousands. With an
        // UNNORMALIZED (area-scale) parallel residual the solver diverged and
        // points flew to ±15000. The normalized sin(angle) residual must keep
        // the solve bounded and actually parallel.
        let mut s = Sketch::new();
        let a = s.add_point(3248.0, 4004.0);
        let b = s.add_point(5316.0, 5652.0);
        let c = s.add_point(7888.0, 3823.0);
        let d = s.add_point(6548.0, 1475.0);
        let l0 = s.add_line(a, b);
        let l1 = s.add_line(c, d);
        s.add(Constraint::Fixed { p: a, x: 3248.0, y: 4004.0 });
        s.add(Constraint::Fixed { p: b, x: 5316.0, y: 5652.0 });
        s.add(Constraint::Parallel { a: l0, b: l1 });
        let rep = solve(&mut s);
        assert!(rep.converged, "rms={}", rep.residual);
        // nothing flew off — every point stays within the original bbox + margin
        for p in &s.points {
            assert!(p.x.abs() < 20_000.0 && p.y.abs() < 20_000.0, "exploded: {:?}", p);
        }
        let u = s.points[b] - s.points[a];
        let v = s.points[d] - s.points[c];
        let cross = u.x * v.y - u.y * v.x;
        assert!((cross / (u.len() * v.len())).abs() < 1e-6, "not parallel: sin={}", cross / (u.len() * v.len()));
    }

    #[test]
    fn parallel_constraint() {
        let mut s = Sketch::new();
        let a = s.add_point(0.0, 0.0);
        let b = s.add_point(10.0, 0.0);
        let c = s.add_point(0.0, 5.0);
        let d = s.add_point(9.0, 6.0);
        let l0 = s.add_line(a, b);
        let l1 = s.add_line(c, d);
        s.add(Constraint::Fixed { p: a, x: 0.0, y: 0.0 });
        s.add(Constraint::Fixed { p: b, x: 10.0, y: 0.0 });
        s.add(Constraint::Fixed { p: c, x: 0.0, y: 5.0 });
        s.add(Constraint::Parallel { a: l0, b: l1 });
        let rep = solve(&mut s);
        assert!(rep.converged, "rms={}", rep.residual);
        let u = s.points[b] - s.points[a];
        let v = s.points[d] - s.points[c];
        assert!((u.x * v.y - u.y * v.x).abs() < 1e-6);
    }

    #[test]
    fn radius_constraint_sizes_circle() {
        let mut s = Sketch::new();
        let c = s.add_circle_xy(1.0, 2.0, 3.0);
        s.add(Constraint::Radius { circle: c, r: 7.5 });
        let rep = solve(&mut s);
        assert!(rep.converged, "rms={}", rep.residual);
        let circ = s.circles[c];
        assert!((s.scalars[circ.radius] - 7.5).abs() < 1e-6, "r={}", s.scalars[circ.radius]);
    }

    #[test]
    fn concentric_and_equal_radius() {
        let mut s = Sketch::new();
        let c0 = s.add_circle_xy(0.0, 0.0, 5.0);
        let c1 = s.add_circle_xy(3.0, 4.0, 2.0);
        // anchor c0's center
        let center0 = s.circles[c0].center;
        s.add(Constraint::Fixed { p: center0, x: 0.0, y: 0.0 });
        s.add(Constraint::Concentric { a: c0, b: c1 });
        s.add(Constraint::EqualRadius { a: c0, b: c1 });
        let rep = solve(&mut s);
        assert!(rep.converged, "rms={}", rep.residual);
        let (cc0, cc1) = (s.circles[c0], s.circles[c1]);
        assert!((s.points[cc1.center] - s.points[cc0.center]).len() < 1e-6);
        assert!((s.scalars[cc0.radius] - s.scalars[cc1.radius]).abs() < 1e-6);
    }

    #[test]
    fn line_tangent_to_circle() {
        // horizontal line y = 0 (both endpoints anchored), circle radius pinned
        // to 2, center starts at (0, 5). Tangency forces center-to-line distance
        // = radius, i.e. |center.y| = 2.
        let mut s = Sketch::new();
        let a = s.add_point(-5.0, 0.0);
        let b = s.add_point(5.0, 0.0);
        let l = s.add_line(a, b);
        let c = s.add_circle_xy(0.0, 5.0, 2.0);
        s.add(Constraint::Fixed { p: a, x: -5.0, y: 0.0 });
        s.add(Constraint::Fixed { p: b, x: 5.0, y: 0.0 });
        s.add(Constraint::Radius { circle: c, r: 2.0 });
        s.add(Constraint::TangentLineCircle { line: l, circle: c });
        let rep = solve(&mut s);
        assert!(rep.converged, "rms={}", rep.residual);
        let cc = s.circles[c];
        let dist = s.points[cc.center].y.abs(); // distance from line y=0
        assert!((dist - 2.0).abs() < 1e-5, "dist={dist}");
    }

    #[test]
    fn dof_detects_underconstrained() {
        // a lone line has 4 free params, no constraints ⇒ dof 4, all params free
        let mut s = Sketch::new();
        let a = s.add_point(0.0, 0.0);
        let b = s.add_point(10.0, 1.0);
        let _l = s.add_line(a, b);
        let d = dof_analysis(&s);
        assert_eq!(d.dof, 4);
        assert!(!d.fully_defined);
        assert!(d.param_free.iter().all(|&f| f));
    }

    #[test]
    fn dof_zero_when_fully_pinned() {
        let mut s = Sketch::new();
        let a = s.add_point(0.0, 0.0);
        let b = s.add_point(10.0, 0.0);
        let _l = s.add_line(a, b);
        s.add(Constraint::Fixed { p: a, x: 0.0, y: 0.0 });
        s.add(Constraint::Fixed { p: b, x: 10.0, y: 0.0 });
        let d = dof_analysis(&s);
        assert_eq!(d.dof, 0);
        assert!(d.fully_defined);
        assert!(d.param_free.iter().all(|&f| !f));
    }

    #[test]
    fn anchor_alone_is_not_redundant() {
        // a single anchor on an otherwise-free line must NOT report "redundant"
        let mut s = Sketch::new();
        let a = s.add_point(0.0, 0.0);
        let b = s.add_point(10.0, 2.0);
        let _l = s.add_line(a, b);
        s.add(Constraint::Fixed { p: a, x: 0.0, y: 0.0 });
        assert!(!dof_analysis(&s).redundant);
    }

    #[test]
    fn duplicate_constraint_is_redundant() {
        let mut s = Sketch::new();
        let a = s.add_point(0.0, 0.0);
        let b = s.add_point(10.0, 2.0);
        let l = s.add_line(a, b);
        s.add(Constraint::Fixed { p: a, x: 0.0, y: 0.0 });
        s.add(Constraint::Horizontal { line: l });
        s.add(Constraint::Horizontal { line: l }); // duplicate ⇒ redundant
        assert!(dof_analysis(&s).redundant);
    }

    #[test]
    fn angle_constraint_sets_45_degrees() {
        let mut s = Sketch::new();
        let a = s.add_point(0.0, 0.0);
        let b = s.add_point(10.0, 0.0);
        let c = s.add_point(0.0, 0.0);
        let d = s.add_point(8.0, 1.0);
        let l0 = s.add_line(a, b);
        let l1 = s.add_line(c, d);
        s.add(Constraint::Fixed { p: a, x: 0.0, y: 0.0 });
        s.add(Constraint::Fixed { p: b, x: 10.0, y: 0.0 });
        s.add(Constraint::Fixed { p: c, x: 0.0, y: 0.0 });
        s.add(Constraint::Angle { a: l0, b: l1, radians: std::f64::consts::FRAC_PI_4 });
        let rep = solve(&mut s);
        assert!(rep.converged, "rms={}", rep.residual);
        let u = s.points[b] - s.points[a];
        let v = s.points[d] - s.points[c];
        let ang = (u.x * v.y - u.y * v.x).atan2(u.x * v.x + u.y * v.y);
        assert!((ang - std::f64::consts::FRAC_PI_4).abs() < 1e-5, "ang={ang}");
    }
}
