//! Centerline TRACE + fit — turns a buffer layer's carved line-work into
//! `cad_kernel` geometry. Pipeline:
//!
//! 1. **ink-within-mask** — keep dark pixels of the source that fall under the
//!    layer's carve mask (the mask only SCOPES; the ink is the real line-work).
//! 2. **skeletonize** — Zhang-Suen thinning to a 1-px centerline.
//! 3. **trace** — walk the skeleton into pixel polylines (collinear-preferring
//!    greedy walk, endpoints first).
//! 4. **simplify** — Douglas-Peucker.
//! 5. **fit** — per the layer's geometry kind: straight `Line`s, a fitted `Arc`
//!    (falls back to lines), or a `Spline` (NURBS) through the simplified path.
//!
//! Output is in WORLD coordinates (image y-down is flipped to y-up via
//! `img_height`). This is the first real engine; the per-type detectors (OCR
//! text, dimensions, walls) layer on top later — see `RASTER_TO_VECTOR.md`.

use cad_kernel::{Arc, Geom, Line, Spline, Vec2};
use image::{DynamicImage, GrayImage};

/// Which geometry a buffer layer fits its line-work into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitKind { Lines, Arcs, Nurbs }

/// Tuning + placement for a trace.
#[derive(Clone, Copy, Debug)]
pub struct TraceParams {
    /// Luminance (0..255) below which a masked pixel counts as ink.
    pub ink_threshold: u8,
    /// World units per image pixel.
    pub scale: f64,
    /// Image height in px — used to flip y (image is y-down, world is y-up).
    pub img_height: u32,
    /// Douglas-Peucker tolerance in px.
    pub simplify_eps: f64,
    /// Drop traced polylines with fewer than this many skeleton pixels.
    pub min_run: usize,
}

impl Default for TraceParams {
    fn default() -> Self {
        Self { ink_threshold: 128, scale: 1.0, img_height: 0,
               simplify_eps: 1.6, min_run: 8 }
    }
}

/// Trace one carve `mask` over the adjusted `working` source into geometry.
pub fn trace_layer(mask: &GrayImage, working: &DynamicImage, fit: FitKind,
                   p: &TraceParams) -> Vec<Geom> {
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    if w == 0 || h == 0 { return Vec::new(); }
    let luma = working.to_luma8();
    if luma.width() as usize != w || luma.height() as usize != h { return Vec::new(); }

    // 1. ink-within-mask -------------------------------------------------
    let mut bits = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if mask.get_pixel(x as u32, y as u32).0[0] == 0 { continue; }
            if luma.get_pixel(x as u32, y as u32).0[0] <= p.ink_threshold {
                bits[y * w + x] = true;
            }
        }
    }

    // 2. skeletonize -----------------------------------------------------
    skeletonize(&mut bits, w, h);

    // 3. trace -> pixel polylines ---------------------------------------
    let paths = trace_polylines(&bits, w, h, p.min_run);

    // 4 + 5. simplify + fit ---------------------------------------------
    let to_world = |x: f64, y: f64| Vec2::new(x * p.scale,
                                              (p.img_height as f64 - y) * p.scale);
    let mut out = Vec::new();
    for path in paths {
        let pts: Vec<(f64, f64)> = path.iter().map(|&(x, y)| (x as f64, y as f64)).collect();
        let simp = douglas_peucker(&pts, p.simplify_eps);
        if simp.len() < 2 { continue; }
        match fit {
            FitKind::Lines => emit_lines(&simp, &to_world, &mut out),
            FitKind::Nurbs => emit_spline(&simp, &to_world, &mut out),
            FitKind::Arcs  => {
                if !emit_arc(&pts, &simp, &to_world, &mut out) {
                    emit_lines(&simp, &to_world, &mut out);   // fall back
                }
            }
        }
    }
    out
}

fn emit_lines(simp: &[(f64, f64)], to_world: &impl Fn(f64, f64) -> Vec2, out: &mut Vec<Geom>) {
    for w2 in simp.windows(2) {
        let a = to_world(w2[0].0, w2[0].1);
        let b = to_world(w2[1].0, w2[1].1);
        if (a - b).len() > 1e-6 { out.push(Geom::Line(Line { a, b })); }
    }
}

fn emit_spline(simp: &[(f64, f64)], to_world: &impl Fn(f64, f64) -> Vec2, out: &mut Vec<Geom>) {
    let cps: Vec<Vec2> = simp.iter().map(|&(x, y)| to_world(x, y)).collect();
    if cps.len() < 2 { return; }
    let degree = (cps.len() - 1).min(3).max(1);
    out.push(Geom::Spline(Spline::new_bspline(degree, cps)));
}

/// Fit a circular arc through `raw` (the full pixel path). Returns false (and
/// emits nothing) if the fit is poor, so the caller can fall back to lines.
fn emit_arc(raw: &[(f64, f64)], simp: &[(f64, f64)],
            to_world: &impl Fn(f64, f64) -> Vec2, out: &mut Vec<Geom>) -> bool {
    if raw.len() < 5 { return false; }
    let Some((cx, cy, r)) = fit_circle(raw) else { return false; };
    // span of the path, to reject near-straight runs masquerading as huge arcs
    let span = {
        let (a, b) = (raw[0], raw[raw.len() - 1]);
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    };
    if r < 2.0 || r > span * 12.0 { return false; }
    // residual: max deviation from the fitted circle
    let mut resid: f64 = 0.0;
    for &(x, y) in raw {
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        resid = resid.max((d - r).abs());
    }
    if resid > 2.5 { return false; }

    // start angle + signed sweep, by unwrapping angles along the path
    let ang = |x: f64, y: f64| (y - cy).atan2(x - cx);
    let a0 = ang(raw[0].0, raw[0].1);
    let mut prev = a0;
    let mut sweep = 0.0;
    for &(x, y) in &raw[1..] {
        let a = ang(x, y);
        let mut d = a - prev;
        while d >  std::f64::consts::PI { d -= std::f64::consts::TAU; }
        while d < -std::f64::consts::PI { d += std::f64::consts::TAU; }
        sweep += d;
        prev = a;
    }
    if sweep.abs() < 0.15 { return false; }   // too flat — let lines handle it

    // World mapping flips y, which flips angle sign and the sweep direction.
    let c = to_world(cx, cy);
    let start = to_world(raw[0].0, raw[0].1);
    let start_angle = (start - c).y.atan2((start - c).x);
    out.push(Geom::Arc(Arc {
        center: c, radius: r * sweep_scale(to_world),
        start_angle, sweep_angle: -sweep,
    }));
    let _ = simp;
    true
}

/// Pixel→world is uniform scale, so |dr| scales by the same factor; recover it
/// from two unit-apart world points.
fn sweep_scale(to_world: &impl Fn(f64, f64) -> Vec2) -> f64 {
    (to_world(1.0, 0.0) - to_world(0.0, 0.0)).len()
}

// ---------------------------------------------------------------------------
// Zhang-Suen thinning
// ---------------------------------------------------------------------------
fn skeletonize(img: &mut [bool], w: usize, h: usize) {
    if w < 3 || h < 3 { return; }
    let idx = |x: usize, y: usize| y * w + x;
    let mut changed = true;
    let mut to_remove: Vec<usize> = Vec::new();
    while changed {
        changed = false;
        for step in 0..2 {
            to_remove.clear();
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    if !img[idx(x, y)] { continue; }
                    // P2..P9 clockwise from north
                    let p = [
                        img[idx(x, y - 1)],     // P2 N
                        img[idx(x + 1, y - 1)], // P3 NE
                        img[idx(x + 1, y)],     // P4 E
                        img[idx(x + 1, y + 1)], // P5 SE
                        img[idx(x, y + 1)],     // P6 S
                        img[idx(x - 1, y + 1)], // P7 SW
                        img[idx(x - 1, y)],     // P8 W
                        img[idx(x - 1, y - 1)], // P9 NW
                    ];
                    let b = p.iter().filter(|&&v| v).count();
                    if b < 2 || b > 6 { continue; }
                    // A = number of 0->1 transitions in the ordered sequence
                    let mut a = 0;
                    for i in 0..8 {
                        if !p[i] && p[(i + 1) % 8] { a += 1; }
                    }
                    if a != 1 { continue; }
                    let (p2, p4, p6, p8) = (p[0], p[2], p[4], p[6]);
                    if step == 0 {
                        if p2 && p4 && p6 { continue; }
                        if p4 && p6 && p8 { continue; }
                    } else {
                        if p2 && p4 && p8 { continue; }
                        if p2 && p6 && p8 { continue; }
                    }
                    to_remove.push(idx(x, y));
                }
            }
            if !to_remove.is_empty() {
                changed = true;
                for &i in &to_remove { img[i] = false; }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Skeleton -> pixel polylines (greedy, collinearity-preferring)
// ---------------------------------------------------------------------------
fn trace_polylines(bits: &[bool], w: usize, h: usize, min_run: usize) -> Vec<Vec<(usize, usize)>> {
    const N8: [(i32, i32); 8] = [(-1,-1),(0,-1),(1,-1),(-1,0),(1,0),(-1,1),(0,1),(1,1)];
    let at = |x: i32, y: i32| -> bool {
        x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && bits[y as usize * w + x as usize]
    };
    let degree = |x: usize, y: usize| -> usize {
        N8.iter().filter(|(dx, dy)| at(x as i32 + dx, y as i32 + dy)).count()
    };
    let mut visited = vec![false; w * h];
    let mut paths = Vec::new();

    let walk = |sx: usize, sy: usize, visited: &mut Vec<bool>| -> Vec<(usize, usize)> {
        let mut path = vec![(sx, sy)];
        visited[sy * w + sx] = true;
        let (mut cx, mut cy) = (sx as i32, sy as i32);
        let (mut px, mut py) = (cx, cy);
        loop {
            // candidate skeleton neighbours not yet consumed
            let mut best: Option<(i32, i32)> = None;
            let mut best_score = -2.0_f64;
            let dirx = (cx - px) as f64;
            let diry = (cy - py) as f64;
            let dlen = (dirx * dirx + diry * diry).sqrt().max(1e-9);
            for (dx, dy) in N8 {
                let (nx, ny) = (cx + dx, cy + dy);
                if !at(nx, ny) { continue; }
                if visited[ny as usize * w + nx as usize] { continue; }
                // prefer the most collinear continuation
                let (sx2, sy2) = (dx as f64, dy as f64);
                let slen = (sx2 * sx2 + sy2 * sy2).sqrt();
                let score = if px == cx && py == cy { 0.0 }
                            else { (dirx * sx2 + diry * sy2) / (dlen * slen) };
                if score > best_score { best_score = score; best = Some((nx, ny)); }
            }
            match best {
                Some((nx, ny)) => {
                    visited[ny as usize * w + nx as usize] = true;
                    path.push((nx as usize, ny as usize));
                    px = cx; py = cy; cx = nx; cy = ny;
                }
                None => break,
            }
        }
        path
    };

    // endpoints first (cleanest open runs), then anything left (loops/junctions)
    for &endpoints_first in &[true, false] {
        for y in 0..h {
            for x in 0..w {
                if !bits[y * w + x] || visited[y * w + x] { continue; }
                let is_end = degree(x, y) <= 1;
                if endpoints_first && !is_end { continue; }
                let path = walk(x, y, &mut visited);
                if path.len() >= min_run { paths.push(path); }
            }
        }
    }
    paths
}

// ---------------------------------------------------------------------------
// Douglas-Peucker
// ---------------------------------------------------------------------------
fn douglas_peucker(pts: &[(f64, f64)], eps: f64) -> Vec<(f64, f64)> {
    if pts.len() < 3 { return pts.to_vec(); }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    *keep.last_mut().unwrap() = true;
    dp_rec(pts, 0, pts.len() - 1, eps, &mut keep);
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

fn dp_rec(pts: &[(f64, f64)], i: usize, j: usize, eps: f64, keep: &mut [bool]) {
    if j <= i + 1 { return; }
    let (ax, ay) = pts[i];
    let (bx, by) = pts[j];
    let (dx, dy) = (bx - ax, by - ay);
    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
    let mut best = eps;
    let mut split = 0;
    for k in i + 1..j {
        let (px, py) = pts[k];
        let d = ((px - ax) * dy - (py - ay) * dx).abs() / len;
        if d > best { best = d; split = k; }
    }
    if split > 0 {
        keep[split] = true;
        dp_rec(pts, i, split, eps, keep);
        dp_rec(pts, split, j, eps, keep);
    }
}

// ---------------------------------------------------------------------------
// Kasa algebraic circle fit
// ---------------------------------------------------------------------------
fn fit_circle(pts: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    let n = pts.len() as f64;
    if pts.len() < 3 { return None; }
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut sxz, mut syz, mut sz) = (0.0, 0.0, 0.0);
    for &(x, y) in pts {
        let z = x * x + y * y;
        sx += x; sy += y; sxx += x * x; syy += y * y; sxy += x * y;
        sxz += x * z; syz += y * z; sz += z;
    }
    // Solve [sxx sxy sx; sxy syy sy; sx sy n] [D;E;F] = [-sxz;-syz;-sz]
    let m = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]];
    let rhs = [-sxz, -syz, -sz];
    let det = det3(&m);
    if det.abs() < 1e-9 { return None; }
    let d = det3(&replace_col(&m, 0, &rhs)) / det;
    let e = det3(&replace_col(&m, 1, &rhs)) / det;
    let f = det3(&replace_col(&m, 2, &rhs)) / det;
    let cx = -d / 2.0;
    let cy = -e / 2.0;
    let r2 = cx * cx + cy * cy - f;
    if r2 <= 0.0 { return None; }
    Some((cx, cy, r2.sqrt()))
}

fn det3(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn replace_col(m: &[[f64; 3]; 3], col: usize, v: &[f64; 3]) -> [[f64; 3]; 3] {
    let mut r = *m;
    for i in 0..3 { r[i][col] = v[i]; }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, Luma, RgbImage, Rgb};

    #[test]
    fn straight_ink_run_makes_a_line() {
        let (w, h) = (40u32, 20u32);
        // white background, one black horizontal stroke (3 px thick)
        let mut rgb = RgbImage::from_pixel(w, h, Rgb([255, 255, 255]));
        for x in 4..36 {
            for y in 9..12 { rgb.put_pixel(x, y, Rgb([0, 0, 0])); }
        }
        let working = DynamicImage::ImageRgb8(rgb);
        let mask = GrayImage::from_pixel(w, h, Luma([255])); // whole image carved
        let p = TraceParams { img_height: h, min_run: 4, ..Default::default() };
        let g = trace_layer(&mask, &working, FitKind::Lines, &p);
        assert!(!g.is_empty(), "expected at least one line from a straight stroke");
        assert!(g.iter().all(|x| matches!(x, Geom::Line(_))));
    }
}
