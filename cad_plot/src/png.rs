// PNG export — CPU rasterise a plot Scene to a PNG image.
//
// Reuses the existing `build_scene` pipeline for color/width/CTB/transform
// resolution, then rasterises Scene primitives to a pixel buffer and
// encodes as PNG via the `image` crate.
//
// Caps apply to the thick-stroke path only: Butt = no extension, Square =
// extended by half-width at each endpoint, Round/Diamond = round caps. Joins
// honour the pen: Miter (with the standard 4× miter limit, falling back to a
// bevel), Bevel, Round/Diamond (filled disc). The thin anti-aliased path
// always draws round-ish endpoints and overlapping joints (documented).
// Dither is a 2×2 checker stipple on the prim's pixels (approximation of the
// CTB raster dither — documented).

use crate::scene::{Prim, Scene};
use cad_kernel::math::Vec2;
use cad_kernel::plotstyle::{EndStyle, JoinStyle};
use image::{ImageEncoder, Rgba, RgbaImage};
use std::io::Cursor;

/// Rasterise a plot Scene to PNG bytes at the given DPI (e.g. 300).
pub fn scene_to_png_bytes(scene: &Scene, dpi: f32) -> Result<Vec<u8>, String> {
    let scale = dpi as f64 / 25.4;
    let iw = (scene.page_w_mm * scale).ceil() as u32;
    let ih = (scene.page_h_mm * scale).ceil() as u32;
    let w = iw.max(1);
    let h = ih.max(1);

    let mut img = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));

    // Fills first, strokes on top.
    for prim in &scene.prims {
        if let Prim::Fill { loops, rgb, dither, .. } = prim {
            for lp in loops {
                draw_filled_polygon(&mut img, lp, *rgb, *dither, scale, h);
            }
        }
    }
    for prim in &scene.prims {
        match prim {
            Prim::Stroke {
                pts, closed, width_mm, rgb, dash_mm, dash_offset_mm, cap, join, dither, ..
            } => {
                if dash_mm.is_empty() {
                    draw_stroke(&mut img, pts, *closed, *width_mm, *rgb, *cap, *join, *dither, scale, h);
                    continue;
                }
                // Linetype dashes: walk the pattern along the polyline (both in
                // paper mm — no scaling) starting at the dash-phase offset, and
                // stroke each dash run separately.
                for run in dash_runs(pts, *closed, dash_mm, *dash_offset_mm as f64) {
                    if run.len() < 2 || dist2(run[0], run[run.len() - 1]) < 1e-6 {
                        // A dot (zero-length dash): a filled round cap.
                        let (cx, cy) = to_px(run[0].0, run[0].1, scale, h);
                        let r = ((*width_mm as f64 * scale) * 0.5).max(0.6) as i32;
                        draw_filled_circle(&mut img, cx, cy, r,
                            Rgba([rgb.0, rgb.1, rgb.2, 255]), *dither);
                    } else {
                        // Dash runs inherit the pen's cap AND join — matching
                        // PDF (per-combo layer caps) and SVG (stroke-linecap /
                        // stroke-linejoin). Interior corners inside a run get
                        // the join style; the run's two ends get the cap.
                        draw_stroke(&mut img, &run, false, *width_mm, *rgb, *cap, *join, *dither, scale, h);
                    }
                }
            }
            Prim::Tris { tris, rgb } => {
                for tri in tris {
                    let pts_mm: Vec<(f64, f64)> = tri.iter().copied().collect();
                    draw_filled_polygon(&mut img, &pts_mm, *rgb, false, scale, h);
                }
            }
            _ => {}
        }
    }

    let raw = img.into_raw();
    let mut buf = Cursor::new(Vec::new());
    let encoder = image::codecs::png::PngEncoder::new(&mut buf);
    encoder.write_image(&raw, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("PNG encode: {e}"))?;
    Ok(buf.into_inner())
}

// ---------------------------------------------------------------------------
// Coordinate helpers
// ---------------------------------------------------------------------------

#[inline]
fn to_px(x: f64, y: f64, scale: f64, img_h: u32) -> (f32, f32) {
    ((x * scale) as f32, (img_h as f64 - y * scale) as f32)
}

// ---------------------------------------------------------------------------
// Stroke rasterisation
// ---------------------------------------------------------------------------

fn draw_stroke(
    img: &mut RgbaImage,
    pts: &[(f64, f64)],
    closed: bool,
    width_mm: f32,
    rgb: (u8, u8, u8),
    cap: EndStyle,
    join: JoinStyle,
    dither: bool,
    scale: f64,
    img_h: u32,
) {
    if pts.len() < 2 || width_mm <= 0.0 {
        return;
    }
    let (rr, gg, bb) = rgb;
    let color = |a: f32| -> Rgba<u8> {
        Rgba([rr, gg, bb, (a.clamp(0.0, 1.0) * 255.0).round() as u8])
    };
    let width_px = (width_mm as f64 * scale).max(0.5);

    if width_px <= 1.2 {
        // Thin anti-aliased line — caps/joins stay round at this width (documented).
        for i in 0..pts.len() - 1 {
            let (x0, y0) = to_px(pts[i].0, pts[i].1, scale, img_h);
            let (x1, y1) = to_px(pts[i + 1].0, pts[i + 1].1, scale, img_h);
            draw_wu_line(img, x0, y0, x1, y1, &color, dither);
        }
        if closed {
            let (x0, y0) = to_px(pts[pts.len() - 1].0, pts[pts.len() - 1].1, scale, img_h);
            let (x1, y1) = to_px(pts[0].0, pts[0].1, scale, img_h);
            draw_wu_line(img, x0, y0, x1, y1, &color, dither);
        }
        return;
    }

    // Quad offsets are in PAPER mm (the pts are paper mm); circle radii are
    // in px. Keep the two separate — mixing them rendered thick strokes
    // ~dpi/25.4× too fat (a px value used as mm).
    let half_mm = (width_mm as f64) * 0.5;
    let r_px = ((width_px * 0.5).max(0.75)) as i32;
    let n = pts.len();
    let seg_count = if closed { n } else { n - 1 };
    // Cap treatment applies to OPEN endpoints only; interior vertices keep
    // their join treatment (see below).
    let square_ext = cap == EndStyle::Square && !closed;
    let round_caps = !closed && matches!(cap, EndStyle::Round | EndStyle::Diamond);
    for i in 0..seg_count {
        let p0 = &pts[i];
        let p1 = &pts[(i + 1) % n];
        let dx = p1.0 - p0.0;
        let dy = p1.1 - p0.1;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            continue;
        }
        let nx = -dy / len;
        let ny = dx / len;
        // Square caps: extend the polyline's true endpoints by half-width
        // along their direction (interior vertices are re-covered by the next
        // segment's quad, so only the first/last segment ends extend).
        let (mut a0, mut a1) = (p0.0, p0.1);
        let (mut b0, mut b1) = (p1.0, p1.1);
        if square_ext {
            if i == 0 {
                a0 -= dx / len * half_mm;
                a1 -= dy / len * half_mm;
            }
            if i == seg_count - 1 {
                b0 += dx / len * half_mm;
                b1 += dy / len * half_mm;
            }
        }
        let quad = [
            (a0 + nx * half_mm, a1 + ny * half_mm),
            (b0 + nx * half_mm, b1 + ny * half_mm),
            (b0 - nx * half_mm, b1 - ny * half_mm),
            (a0 - nx * half_mm, a1 - ny * half_mm),
        ];
        draw_filled_polygon(img, &quad, rgb, dither, scale, img_h);
    }
    // Joins at interior vertices (and the closed seam) — the pen's join style.
    let join_round = matches!(join, JoinStyle::Round | JoinStyle::Diamond);
    if join_round {
        let idxs: Vec<usize> = if closed { (0..n).collect() } else { (1..n - 1).collect() };
        for &i in &idxs {
            let (cx, cy) = to_px(pts[i].0, pts[i].1, scale, img_h);
            if r_px > 0 {
                draw_filled_circle(img, cx, cy, r_px, color(1.0), dither);
            }
        }
    } else if join == JoinStyle::Bevel || join == JoinStyle::Miter {
        // Wedge fill per corner. The geometry is the SHARED
        // `cad_kernel::math::join_wedge` (also used by the app's preview), so
        // the raster and the on-screen emulation cannot drift apart. A bevel
        // fills the butt notch with a straight edge; a miter extends to the
        // edge intersection, capped by the shared miter limit.
        let want_miter = join == JoinStyle::Miter;
        let idxs: Vec<usize> = if closed { (0..n).collect() } else { (1..n - 1).collect() };
        for &i in &idxs {
            let prev = if closed { (i + n - 1) % n } else { i - 1 };
            let next = if closed { (i + 1) % n } else { i + 1 };
            let v = Vec2::new(pts[i].0, pts[i].1);
            let d1 = (v - Vec2::new(pts[prev].0, pts[prev].1)).normalized();
            let d2 = (Vec2::new(pts[next].0, pts[next].1) - v).normalized();
            let Some(w) = cad_kernel::math::join_wedge(v, d1, d2, half_mm) else { continue };
            let mm = |p: Vec2| (p.x, p.y);
            if want_miter {
                if let Some(apex) = w.apex {
                    draw_filled_polygon(img, &[mm(v), mm(w.a), mm(apex), mm(w.b)],
                        rgb, dither, scale, img_h);
                    continue;
                }
            }
            draw_filled_polygon(img, &[mm(v), mm(w.a), mm(w.b)], rgb, dither, scale, img_h);
        }
    }
    // Open endpoints: round caps for Round/Diamond (Butt/Square are none).
    if round_caps {
        for &(px, py) in [pts[0], pts[n - 1]].iter() {
            let (cx, cy) = to_px(px, py, scale, img_h);
            if r_px > 0 {
                draw_filled_circle(img, cx, cy, r_px, color(1.0), dither);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Linetype dash runs (paper mm)
// ---------------------------------------------------------------------------

/// Squared distance between two points.
fn dist2(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)
}

/// Split a polyline (paper mm) into dash runs along a linetype pattern
/// (dash/gap alternating; each entry is an absolute length — a zero-length
/// dash is a dot). `closed` appends the closing segment so the pattern wraps
/// across the seam. `phase_mm` starts the pattern walk that far INTO the
/// pattern (the adaptive-linetype dash offset from the scene).
fn dash_runs(
    pts: &[(f64, f64)],
    closed: bool,
    pattern: &[f32],
    phase_mm: f64,
) -> Vec<Vec<(f64, f64)>> {
    if pattern.is_empty() || pts.len() < 2 {
        return vec![pts.to_vec()];
    }
    // Segment chain: straight chords between consecutive points.
    let mut segs: Vec<((f64, f64), (f64, f64), f64)> = Vec::new();
    let mut add_seg = |a: (f64, f64), b: (f64, f64)| {
        let len = dist2(a, b).sqrt();
        if len > 1e-9 {
            segs.push((a, b, len));
        }
    };
    for i in 0..pts.len().saturating_sub(1) {
        add_seg(pts[i], pts[i + 1]);
    }
    if closed && pts.len() > 2 {
        add_seg(pts[pts.len() - 1], pts[0]);
    }
    if segs.is_empty() {
        return Vec::new();
    }
    // Start the walk at the dash phase: consume `phase_mm` from the pattern
    // before the first segment so the pattern position at the path start
    // matches the scene's offset.
    let mut pi = 0usize;
    let mut remaining = (pattern[0] as f64).abs();
    let mut phase = phase_mm.abs();
    let total: f64 = pattern.iter().map(|&v| v.abs() as f64).sum();
    phase = if total > 1e-9 { phase % total } else { 0.0 };
    while phase > 1e-9 {
        let seg = (pattern[pi] as f64).abs();
        if phase >= seg - 1e-9 {
            phase -= seg;
            pi = (pi + 1) % pattern.len();
            // The new entry starts fresh — without this, a phase landing on an
            // entry boundary keeps the stale `pattern[0]` length and the walk
            // misplaces every dash.
            remaining = (pattern[pi] as f64).abs();
        } else {
            remaining = seg - phase;
            phase = 0.0;
        }
    }
    let mut runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let mut on = pi % 2 == 0;
    let mut seg_i = 0usize;
    let mut carry = 0.0f64; // distance already walked in the current segment
    while seg_i < segs.len() {
        let (a, b, len) = segs[seg_i];
        let avail = len - carry;
        if avail <= 1e-9 {
            carry = 0.0;
            seg_i += 1;
            continue;
        }
        let take = avail.min(remaining);
        let t0 = carry / len;
        let t1 = (carry + take) / len;
        let p0 = (a.0 + (b.0 - a.0) * t0, a.1 + (b.1 - a.1) * t0);
        let p1 = (a.0 + (b.0 - a.0) * t1, a.1 + (b.1 - a.1) * t1);
        if on {
            if cur.is_empty() {
                cur.push(p0);
            }
            cur.push(p1);
        }
        carry += take;
        remaining -= take;
        if remaining <= 1e-9 {
            if on && cur.len() >= 2 {
                runs.push(std::mem::take(&mut cur));
            }
            on = !on;
            pi = (pi + 1) % pattern.len();
            remaining = (pattern[pi] as f64).abs();
        }
        if carry >= len - 1e-9 {
            carry = 0.0;
            seg_i += 1;
        }
    }
    if on && cur.len() >= 2 {
        runs.push(cur);
    }
    runs
}

// ---------------------------------------------------------------------------
// Xiaolin Wu anti-aliased line (all f32 arithmetic)
// ---------------------------------------------------------------------------

fn draw_wu_line(
    img: &mut RgbaImage,
    x0: f32, y0: f32,
    x1: f32, y1: f32,
    color: &dyn Fn(f32) -> Rgba<u8>,
    dither: bool,
) {
    let mut x0 = x0;
    let mut y0 = y0;
    let mut x1 = x1;
    let mut y1 = y1;
    let steep = (y1 - y0).abs() > (x1 - x0).abs();
    if steep {
        std::mem::swap(&mut x0, &mut y0);
        std::mem::swap(&mut x1, &mut y1);
    }
    if x0 > x1 {
        std::mem::swap(&mut x0, &mut x1);
        std::mem::swap(&mut y0, &mut y1);
    }
    let dx = x1 - x0;
    let dy = y1 - y0;
    let gradient = if dx.abs() < 1e-6 { 1.0 } else { dy / dx };
    let w = img.width() as i32;
    let h = img.height() as i32;

    let mut plot = |x: i32, y: i32, alpha: f32| {
        if dither && (x + y).rem_euclid(2) != 0 {
            return;
        }
        if x >= 0 && x < w && y >= 0 && y < h {
            blend_pixel(img, x as u32, y as u32, color(alpha));
        }
    };

    // First endpoint
    let xend = x0.round();
    let yend = y0 + gradient * (xend - x0);
    let xgap = 1.0 - x0.fract();
    let xpxl1 = xend as i32;
    let ypxl1 = yend.floor() as i32;
    let fy = (1.0 - yend.fract()) * xgap;
    if steep {
        plot(ypxl1, xpxl1, fy);
        plot(ypxl1 + 1, xpxl1, yend.fract() * xgap);
    } else {
        plot(xpxl1, ypxl1, fy);
        plot(xpxl1, ypxl1 + 1, yend.fract() * xgap);
    }
    let mut intery = yend + gradient;

    // Second endpoint
    let xend = x1.round();
    let yend = y1 + gradient * (xend - x1);
    let xgap = x1.fract();
    let xpxl2 = xend as i32;
    let ypxl2 = yend.floor() as i32;
    let fy = (1.0 - yend.fract()) * xgap;
    if steep {
        plot(ypxl2, xpxl2, fy);
        plot(ypxl2 + 1, xpxl2, yend.fract() * xgap);
    } else {
        plot(xpxl2, ypxl2, fy);
        plot(xpxl2, ypxl2 + 1, yend.fract() * xgap);
    }

    // Main loop
    if steep {
        for x in (xpxl1 + 1)..xpxl2 {
            let yf = intery.fract();
            plot(intery.floor() as i32, x, 1.0 - yf);
            plot(intery.floor() as i32 + 1, x, yf);
            intery += gradient;
        }
    } else {
        for x in (xpxl1 + 1)..xpxl2 {
            let yf = intery.fract();
            plot(x, intery.floor() as i32, 1.0 - yf);
            plot(x, intery.floor() as i32 + 1, yf);
            intery += gradient;
        }
    }
}

// ---------------------------------------------------------------------------
// Filled polygon — integer scanline even-odd
// ---------------------------------------------------------------------------

fn draw_filled_polygon(
    img: &mut RgbaImage,
    pts: &[(f64, f64)],
    rgb: (u8, u8, u8),
    dither: bool,
    scale: f64,
    img_h: u32,
) {
    if pts.len() < 3 {
        return;
    }
    let (rr, gg, bb) = rgb;
    let color = Rgba([rr, gg, bb, 255]);

    let pxs: Vec<(f32, f32)> = pts.iter()
        .map(|&(x, y)| to_px(x, y, scale, img_h))
        .collect();

    let mut ymin = f32::INFINITY;
    let mut ymax = f32::NEG_INFINITY;
    for &(_, y) in &pxs {
        ymin = ymin.min(y);
        ymax = ymax.max(y);
    }
    let y0 = ymin.floor() as i32;
    let y1 = ymax.ceil() as i32;
    let w = img.width() as i32;
    let hm1 = img.height() as i32 - 1;

    for y in y0..=y1 {
        let mut xs: Vec<f32> = Vec::new();
        let n = pxs.len();
        let fy = y as f32;
        for i in 0..n {
            let (x0, py0) = pxs[i];
            let (x1, py1) = pxs[(i + 1) % n];
            if (py0 <= fy && py1 > fy) || (py1 <= fy && py0 > fy) {
                let t = (fy - py0) / (py1 - py0);
                xs.push(x0 + t * (x1 - x0));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for k in (0..xs.len()).step_by(2) {
            if k + 1 >= xs.len() {
                break;
            }
            let xa = xs[k].round() as i32;
            let xb = xs[k + 1].round() as i32;
            let x_start = xa.max(0);
            let x_end = xb.min(w - 1);
            for x in x_start..=x_end {
                if y >= 0 && y <= hm1 && (!dither || (x + y).rem_euclid(2) == 0) {
                    img.put_pixel(x as u32, y as u32, color);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Filled circle
// ---------------------------------------------------------------------------

fn draw_filled_circle(img: &mut RgbaImage, cx: f32, cy: f32, r: i32, color: Rgba<u8>, dither: bool) {
    if r < 1 {
        return;
    }
    let w = img.width() as i32;
    let hm1 = img.height() as i32 - 1;
    let cx_i = cx.round() as i32;
    let cy_i = cy.round() as i32;
    for dy in -r..=r {
        let y = cy_i + dy;
        if y < 0 || y > hm1 {
            continue;
        }
        let dx_max = ((r * r - dy * dy) as f32).sqrt().round() as i32;
        for dx in -dx_max..=dx_max {
            let x = cx_i + dx;
            if x >= 0 && x < w && (!dither || (x + y).rem_euclid(2) == 0) {
                img.put_pixel(x as u32, y as u32, color);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Alpha-blend onto a pixel
// ---------------------------------------------------------------------------

fn blend_pixel(img: &mut RgbaImage, x: u32, y: u32, src: Rgba<u8>) {
    let dst = img.get_pixel(x, y);
    let sa = src[3] as f32 / 255.0;
    if sa >= 1.0 {
        img.put_pixel(x, y, src);
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a < 1.0 / 255.0 {
        return;
    }
    let r = (src[0] as f32 * sa + dst[0] as f32 * da * (1.0 - sa)) / out_a;
    let g = (src[1] as f32 * sa + dst[1] as f32 * da * (1.0 - sa)) / out_a;
    let b = (src[2] as f32 * sa + dst[2] as f32 * da * (1.0 - sa)) / out_a;
    img.put_pixel(x, y, Rgba([
        r.round() as u8,
        g.round() as u8,
        b.round() as u8,
        (out_a * 255.0).round() as u8,
    ]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Scene;
    use cad_kernel::plotstyle::{EndStyle, JoinStyle};

    fn stroke(
        pts: Vec<(f64, f64)>,
        width_mm: f32,
        rgb: (u8, u8, u8),
        dash_mm: Vec<f32>,
    ) -> Prim {
        Prim::Stroke {
            pts,
            closed: false,
            width_mm,
            rgb,
            dash_mm,
            dash_offset_mm: 0.0,
            cap: EndStyle::Round,
            join: JoinStyle::Round,
            dither: false,
            smooth: false,
        }
    }

    #[test]
    fn thick_stroke_width_is_physical() {
        // Regression: `half` was once computed in PIXELS but used as paper mm,
        // so a 2 mm stroke at 300 dpi rendered ~11.8× too wide. A 2 mm
        // vertical line must come out ≈ 23.6 px wide.
        let dpi = 300.0;
        let scale = dpi as f64 / 25.4;
        let scene = Scene {
            page_w_mm: 50.0, page_h_mm: 50.0,
            prims: vec![stroke(vec![(35.0, 10.0), (35.0, 40.0)], 2.0, (0, 255, 0), Vec::new())],
            skipped_dims: 0,
        };
        let bytes = scene_to_png_bytes(&scene, dpi).unwrap();
        let img = image::load_from_memory(&bytes).unwrap().to_rgba8();
        let (w, h) = (img.width(), img.height());
        let green_at = |x: f64, y: f64| {
            let xi = (x * scale) as u32;
            let yi = h - 1 - (y * scale) as u32;
            let p = img.get_pixel(xi.min(w - 1), yi.min(h - 1));
            p[1] > 150 && p[0] < 100
        };
        // The stroke centre row: walk right from the centre until white.
        let y = 25.0;
        let mut x_max = 35.0;
        while x_max < 45.0 && green_at(x_max, y) { x_max += 0.1; }
        let mut x_min = 35.0;
        while x_min > 30.0 && green_at(x_min, y) { x_min -= 0.1; }
        let w_mm = x_max - x_min;
        assert!((w_mm - 2.0).abs() < 0.5,
            "2 mm stroke must rasterise ~2 mm wide, got {w_mm:.2} mm");
    }

    #[test]
    fn empty_scene_produces_png() {
        let scene = Scene {
            page_w_mm: 50.0, page_h_mm: 50.0,
            prims: Vec::new(), skipped_dims: 0,
        };
        let bytes = scene_to_png_bytes(&scene, 72.0).unwrap();
        assert!(bytes.len() > 100);
        // PNG magic bytes
        assert_eq!(&bytes[1..4], b"PNG");
    }

    #[test]
    fn stroke_produces_nonempty_png() {
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![stroke(vec![(10.0, 10.0), (90.0, 50.0)], 0.5, (255, 0, 0), Vec::new())],
            skipped_dims: 0,
        };
        let bytes = scene_to_png_bytes(&scene, 300.0).unwrap();
        assert!(bytes.len() > 200);
        assert_eq!(&bytes[1..4], b"PNG");
    }

    #[test]
    fn dash_runs_split_a_line_along_the_pattern() {
        let pts = vec![(0.0, 0.0), (10.0, 0.0)];
        let runs = dash_runs(&pts, false, &[4.0, 2.0], 0.0);
        assert_eq!(runs.len(), 2, "dash-gap-dash: {:?}", runs);
        assert_eq!(runs[0], vec![(0.0, 0.0), (4.0, 0.0)]);
        assert_eq!(runs[1], vec![(6.0, 0.0), (10.0, 0.0)]);
    }

    #[test]
    fn dash_runs_honour_the_phase_offset() {
        // Phase 1.0 starts the walk 1 mm into pattern [4, 2]: the first dash is
        // shortened to 3 mm, then gap 3-5, dash 5-9, gap 9-10.
        let pts = vec![(0.0, 0.0), (10.0, 0.0)];
        let runs = dash_runs(&pts, false, &[4.0, 2.0], 1.0);
        assert_eq!(runs.len(), 2, "dash-gap-dash after phase: {:?}", runs);
        assert_eq!(runs[0], vec![(0.0, 0.0), (3.0, 0.0)]);
        assert_eq!(runs[1], vec![(5.0, 0.0), (9.0, 0.0)]);
    }

    #[test]
    fn dash_runs_phase_on_entry_boundary_uses_the_next_entry_length() {
        // Phase 4.0 = exactly the end of the first dash of [4, 2]: the walk
        // starts in the GAP with the gap's own length (2), not a stale dash
        // length: gap 0-2, dash 2-6, gap 6-8, dash 8-10.
        let pts = vec![(0.0, 0.0), (10.0, 0.0)];
        let runs = dash_runs(&pts, false, &[4.0, 2.0], 4.0);
        assert_eq!(runs.len(), 2, "two dashes after a boundary phase: {:?}", runs);
        assert_eq!(runs[0], vec![(2.0, 0.0), (6.0, 0.0)]);
        assert_eq!(runs[1], vec![(8.0, 0.0), (10.0, 0.0)]);
    }

    #[test]
    fn dash_runs_wrap_around_closed_polylines() {
        // A square of perimeter 8 with pattern [3, 1]: dash 0-3, gap 3-4,
        // dash 4-7, gap 7-8 — the pattern wraps across the closing seam.
        let pts = vec![(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)];
        let runs = dash_runs(&pts, true, &[3.0, 1.0], 0.0);
        assert_eq!(runs.len(), 2, "two dashes over the perimeter: {:?}", runs);
        assert_eq!(runs[0], vec![(0.0, 0.0), (2.0, 0.0), (2.0, 1.0)]);
        assert_eq!(runs[1], vec![(2.0, 2.0), (0.0, 2.0), (0.0, 1.0)]);
    }

    #[test]
    fn dashed_stroke_produces_nonempty_png() {
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![stroke(vec![(10.0, 50.0), (90.0, 50.0)], 0.5, (0, 0, 255), vec![4.0, 2.0])],
            skipped_dims: 0,
        };
        let bytes = scene_to_png_bytes(&scene, 300.0).unwrap();
        assert!(bytes.len() > 200);
        assert_eq!(&bytes[1..4], b"PNG");
    }

    #[test]
    fn dithered_fill_produces_nonempty_png() {
        // Dither smoke: a dithered fill + stroke still rasterise to a valid PNG.
        let scene = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![
                Prim::Fill {
                    loops: vec![vec![(10.0, 10.0), (90.0, 10.0), (90.0, 90.0), (10.0, 90.0)]],
                    rgb: (200, 0, 0),
                    dither: true,
                },
                {
                    let mut s = stroke(vec![(10.0, 50.0), (90.0, 50.0)], 0.5, (0, 0, 255), Vec::new());
                    if let Prim::Stroke { dither, .. } = &mut s { *dither = true; }
                    s
                },
            ],
            skipped_dims: 0,
        };
        let bytes = scene_to_png_bytes(&scene, 300.0).unwrap();
        assert!(bytes.len() > 200);
        assert_eq!(&bytes[1..4], b"PNG");
    }

    #[test]
    fn square_caps_extend_butt_do_not() {
        // A single thick segment, width 2 mm at 72 dpi (≈5.67 px wide). The
        // square-capped stroke must cover more pixels than the butt-capped one
        // (it extends by half-width at each endpoint).
        let pts = vec![(10.0, 50.0), (30.0, 50.0)];
        let mut sq = stroke(pts.clone(), 2.0, (0, 0, 0), Vec::new());
        let mut bt = stroke(pts.clone(), 2.0, (0, 0, 0), Vec::new());
        if let Prim::Stroke { cap, .. } = &mut sq { *cap = EndStyle::Square; }
        if let Prim::Stroke { cap, .. } = &mut bt { *cap = EndStyle::Butt; }

        let scene_sq = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![sq], skipped_dims: 0,
        };
        let scene_bt = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![bt], skipped_dims: 0,
        };
        let px_sq = scene_to_png_bytes(&scene_sq, 72.0).unwrap();
        let px_bt = scene_to_png_bytes(&scene_bt, 72.0).unwrap();
        let img_sq = image::load_from_memory(&px_sq).unwrap().to_rgba8();
        let img_bt = image::load_from_memory(&px_bt).unwrap().to_rgba8();
        let dark = |img: &RgbaImage| {
            img.pixels().filter(|p| p[0] < 200).count()
        };
        assert!(dark(&img_sq) > dark(&img_bt),
            "square caps must extend past butt caps: sq={} bt={}",
            dark(&img_sq), dark(&img_bt));
    }

    #[test]
    fn miter_join_extends_further_than_bevel() {
        // A 90° corner, 2 mm wide at 72 dpi. A miter join's wedge reaches the
        // edge intersection (~1.41× half width past the corner) while a bevel
        // stops at the butt corners — the miter must paint more pixels.
        let pts = vec![(0.0, 40.0), (30.0, 40.0), (30.0, 60.0)];
        let mut mt = stroke(pts.clone(), 2.0, (0, 0, 0), Vec::new());
        let mut bv = stroke(pts.clone(), 2.0, (0, 0, 0), Vec::new());
        if let Prim::Stroke { join, .. } = &mut mt { *join = JoinStyle::Miter; }
        if let Prim::Stroke { join, .. } = &mut bv { *join = JoinStyle::Bevel; }

        let scene_mt = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![mt], skipped_dims: 0,
        };
        let scene_bv = Scene {
            page_w_mm: 100.0, page_h_mm: 100.0,
            prims: vec![bv], skipped_dims: 0,
        };
        let img_mt = image::load_from_memory(&scene_to_png_bytes(&scene_mt, 72.0).unwrap())
            .unwrap().to_rgba8();
        let img_bv = image::load_from_memory(&scene_to_png_bytes(&scene_bv, 72.0).unwrap())
            .unwrap().to_rgba8();
        let dark = |img: &RgbaImage| img.pixels().filter(|p| p[0] < 200).count();
        assert!(dark(&img_mt) > dark(&img_bv),
            "miter join must reach further than bevel: miter={} bevel={}",
            dark(&img_mt), dark(&img_bv));
    }
}

