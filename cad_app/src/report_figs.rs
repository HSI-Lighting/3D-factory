//! Figures for the Phase 2–4 report, rendered **by the code they document**.
//!
//! A diagram of what a renderer is supposed to do proves nothing. Every image here comes out of the
//! actual modules — [`crate::color`]'s transforms, [`crate::env`]'s sky, [`crate::matball`]'s BRDF,
//! [`crate::pathtrace`]'s tracer and denoiser — so if a figure looks right, the code is right, and
//! if the code changes the figure changes with it.
//!
//! Run with:
//! ```text
//! cargo test -p cad_app --bin simlux report_figs -- --ignored --nocapture
//! ```
//! It writes a `figs.js` (plus the PNGs, for inspection) into the directory named by
//! `SIMLUX_FIG_DIR`, defaulting to the system temp folder.

use crate::color::{ColorPipeline, ViewTransform};
use crate::env::{self, Sky};
use crate::factory::{ProcDef, ProcPattern};
use glam::Vec3;

/// A simple RGBA8 canvas, so the plots do not need a drawing crate.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize, bg: [u8; 3]) -> Self {
        let mut px = vec![255u8; w * h * 4];
        for p in px.chunks_exact_mut(4) {
            p[0] = bg[0];
            p[1] = bg[1];
            p[2] = bg[2];
        }
        Self { w, h, px }
    }
    fn set(&mut self, x: isize, y: isize, c: [u8; 3]) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let o = (y as usize * self.w + x as usize) * 4;
        self.px[o] = c[0];
        self.px[o + 1] = c[1];
        self.px[o + 2] = c[2];
    }
    fn rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 3]) {
        for y in y0..y1.min(self.h) {
            for x in x0..x1.min(self.w) {
                self.set(x as isize, y as isize, c);
            }
        }
    }
    /// A 2-pixel-wide line, so it survives being scaled down in a browser.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [u8; 3]) {
        let n = (((b.0 - a.0).abs()).max((b.1 - a.1).abs()).ceil() as usize).max(1) * 2;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                self.set(x as isize + dx, y as isize + dy, c);
            }
        }
    }
}

fn png_data_uri(w: usize, h: usize, rgba: &[u8]) -> String {
    use base64::Engine;
    let mut buf: Vec<u8> = Vec::new();
    {
        let enc = image::codecs::png::PngEncoder::new(&mut buf);
        image::ImageEncoder::write_image(enc, rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8).expect("png encode");
    }
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&buf))
}

// ── the figures ──────────────────────────────────────────────────────────────────────────────

/// The four view transforms as response curves, over 0..8 scene-referred linear.
fn fig_tone_curves() -> (usize, usize, Vec<u8>) {
    let (w, h) = (760usize, 380usize);
    let mut c = Canvas::new(w, h, [24, 26, 32]);
    let (x0, y0, x1, y1) = (56.0f32, 24.0f32, w as f32 - 16.0, h as f32 - 36.0);
    // Grid at every stop of scene light, and every 0.25 of display value.
    for k in 0..=8 {
        let x = x0 + (x1 - x0) * (k as f32 / 8.0);
        c.line((x, y0), (x, y1), [44, 47, 56]);
    }
    for k in 0..=4 {
        let y = y1 - (y1 - y0) * (k as f32 / 4.0);
        c.line((x0, y), (x1, y), [44, 47, 56]);
    }
    // The transform this replaced, for comparison.
    let plot = |c: &mut Canvas, f: &dyn Fn(f32) -> f32, col: [u8; 3]| {
        let mut prev: Option<(f32, f32)> = None;
        for i in 0..=400 {
            let sx = i as f32 / 400.0 * 8.0;
            let v = f(sx).clamp(0.0, 1.0);
            let p = (x0 + (x1 - x0) * (sx / 8.0), y1 - (y1 - y0) * v);
            if let Some(q) = prev {
                c.line(q, p, col);
            }
            prev = Some(p);
        }
    };
    plot(&mut c, &|x| 1.0 - (-x).exp(), [150, 90, 90]); // the old in-shader tone-map
    for (view, col) in [
        (ViewTransform::Standard, [110, 130, 190]),
        (ViewTransform::PbrNeutral, [110, 190, 140]),
        (ViewTransform::AgX, [235, 180, 90]),
    ] {
        let p = ColorPipeline { view, ..Default::default() };
        plot(&mut c, &move |x| crate::color::tonemap(p, [x, x, x])[0], col);
    }
    c.rect(0, y1 as usize + 2, w, h, [24, 26, 32]);
    (w, h, c.px)
}

/// An over-range grey ramp through the old tone-map and through AgX — the same numbers, side by
/// side, so the difference is a picture rather than a claim.
fn fig_tone_strip() -> (usize, usize, Vec<u8>) {
    let (w, h) = (760usize, 210usize);
    let mut c = Canvas::new(w, h, [24, 26, 32]);
    const MAX: f32 = 16.0;
    let rows: [(&str, Box<dyn Fn([f32; 3]) -> [u8; 3]>); 3] = [
        // What actually shipped: 1 − e⁻ˣ per channel, written straight into an 8-bit buffer with
        // no display encode at all.
        ("old", Box::new(|l: [f32; 3]| {
            [
                ((1.0 - (-l[0]).exp()) * 255.0) as u8,
                ((1.0 - (-l[1]).exp()) * 255.0) as u8,
                ((1.0 - (-l[2]).exp()) * 255.0) as u8,
            ]
        })),
        ("standard", Box::new(|l| crate::color::tonemap8(ColorPipeline { view: ViewTransform::Standard, ..Default::default() }, l))),
        ("agx", Box::new(|l| crate::color::tonemap8(ColorPipeline::default(), l))),
    ];
    for (r, (_, f)) in rows.iter().enumerate() {
        let y0 = 10 + r * 66;
        for x in 0..w {
            let s = x as f32 / w as f32 * MAX;
            let px = f([s, s * 0.72, s * 0.42]); // a warm cast, so desaturation is visible
            for y in y0..y0 + 52 {
                c.set(x as isize, y as isize, px);
            }
        }
    }
    // Stop marks at 1, 2, 4, 8 scene-referred — the range where each transform gives up.
    for stop in [1.0f32, 2.0, 4.0, 8.0] {
        let x = (stop / MAX * w as f32) as usize;
        for r in 0..3 {
            let y0 = 10 + r * 66;
            for y in y0 + 52..y0 + 60 {
                c.set(x as isize, y as isize, [200, 200, 210]);
                c.set(x as isize + 1, y as isize, [200, 200, 210]);
            }
        }
    }
    (w, h, c.px)
}

/// The sky itself: a 360°×90° panorama at three turbidities.
fn fig_sky(turbidity: f32, sun_alt_deg: f32) -> (usize, usize, Vec<u8>) {
    let (w, h) = (600usize, 150usize);
    let a = sun_alt_deg.to_radians();
    let sun = Vec3::new(0.0, a.cos(), a.sin());
    let mut sky = Sky::new(sun, turbidity);
    sky.calibrate([0.9, 0.98, 1.15], [0.25, 0.24, 0.21], [3.0, 2.9, 2.7]);
    let p = ColorPipeline::default();
    let mut px = vec![255u8; w * h * 4];
    for y in 0..h {
        // Elevation 90° at the top down to 0° at the horizon.
        let el = (1.0 - y as f32 / h as f32) * std::f32::consts::FRAC_PI_2;
        for x in 0..w {
            let az = x as f32 / w as f32 * std::f32::consts::TAU - std::f32::consts::PI;
            let d = Vec3::new(el.cos() * az.sin(), el.cos() * az.cos(), el.sin());
            let [r, g, b] = crate::color::tonemap8(p, sky.radiance_with_sun(d));
            let o = (y * w + x) * 4;
            px[o] = r;
            px[o + 1] = g;
            px[o + 2] = b;
        }
    }
    (w, h, px)
}

/// A neutral sphere lit ONLY by ambient: the old two-colour hemispheric lerp on the left, the sky's
/// real SH irradiance on the right. Same total energy — the calibration guarantees it — so any
/// difference is distribution, which is the whole point.
fn fig_ambient_compare() -> (usize, usize, Vec<u8>) {
    let (side, w, h) = (220usize, 460usize, 220usize);
    // A fairly low sun (~28°): the old fill is a function of `normal.z` alone, so it can only ever
    // make a vertical gradient. A real sky at this hour is far brighter on the sun's side than
    // opposite it, and that horizontal difference is precisely what the SH ambient can express.
    let sun = Vec3::new(-0.78, -0.40, 0.48).normalize();
    let mut sky = Sky::new(sun, env::DEFAULT_TURBIDITY);
    let (sky_col, ground_col) = ([0.85, 0.92, 1.10], [0.24, 0.23, 0.20]);
    sky.calibrate(sky_col, ground_col, [2.6, 2.5, 2.35]);
    let sh = sky.sh9();
    let p = ColorPipeline { exposure: 0.6, ..Default::default() };
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let panel = x / side;
            let lx = x % side;
            if panel > 1 || x >= side * 2 {
                let o = (y * w + x) * 4;
                px[o + 3] = 255;
                continue;
            }
            // Orthographic sphere.
            let u = (lx as f32 / side as f32) * 2.0 - 1.0;
            let v = 1.0 - (y as f32 / h as f32) * 2.0;
            let r2 = u * u + v * v;
            let lin = if r2 <= 1.0 {
                let n = Vec3::new(u, -(1.0 - r2).max(0.0).sqrt(), v).normalize();
                let amb = if panel == 0 {
                    // The old fill: two colours lerped by the normal's up-component.
                    let t = (0.5 + 0.5 * n.z).clamp(0.0, 1.0);
                    [
                        ground_col[0] + (sky_col[0] - ground_col[0]) * t,
                        ground_col[1] + (sky_col[1] - ground_col[1]) * t,
                        ground_col[2] + (sky_col[2] - ground_col[2]) * t,
                    ]
                } else {
                    env::sh_ambient(&sh, n)
                };
                let a = crate::color::srgb_to_linear(0.8);
                [amb[0] * a, amb[1] * a, amb[2] * a]
            } else {
                [0.02, 0.022, 0.028]
            };
            let [r, g, b] = crate::color::tonemap8(p, lin);
            let o = (y * w + x) * 4;
            px[o] = r;
            px[o + 1] = g;
            px[o + 2] = b;
            px[o + 3] = 255;
        }
    }
    // A hairline between the panels.
    for y in 0..h {
        let o = (y * w + side) * 4;
        px[o] = 40;
        px[o + 1] = 42;
        px[o + 2] = 50;
    }
    (w, h, px)
}

fn ball_of(p: &crate::matball::Preview, size: usize) -> (usize, usize, Vec<u8>) {
    let (sky, sh, sun) = crate::matball::preview_sky();
    (size, size, crate::matball::render(p, &sky, &sh, sun, ColorPipeline::default(), size))
}

fn preview(albedo: [f32; 3], rough: f32, metallic: f32, proc: Option<ProcDef>) -> crate::matball::Preview {
    crate::matball::Preview {
        albedo: crate::color::srgb_to_linear3(albedo),
        roughness: rough,
        metallic,
        ior: 1.5,
        opacity: 1.0,
        emission: [0.0; 3],
        proc,
    }
}

/// A noisy path-traced frame and the same frame denoised — a room corner with a sphere in it.
fn fig_denoise() -> ((usize, usize, Vec<u8>), (usize, usize, Vec<u8>)) {
    use crate::pathtrace::{Camera, Scene, Settings, Sky as PtSky};
    use crate::radiance_export::ExportTri;

    let mut tris: Vec<ExportTri> = Vec::new();
    let mut quad = |a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3], rgb: [f32; 3], rough: f32| {
        tris.push(ExportTri::plain([a, b, c], rgb, rough, 1.0));
        tris.push(ExportTri::plain([a, c, d], rgb, rough, 1.0));
    };
    // Floor, back wall, side wall — a corner, which is where indirect light gets interesting.
    quad([-3.0, -3.0, 0.0], [3.0, -3.0, 0.0], [3.0, 3.0, 0.0], [-3.0, 3.0, 0.0], [0.55, 0.53, 0.5], 0.7);
    quad([-3.0, 3.0, 0.0], [3.0, 3.0, 0.0], [3.0, 3.0, 3.0], [-3.0, 3.0, 3.0], [0.7, 0.35, 0.28], 0.9);
    quad([-3.0, -3.0, 0.0], [-3.0, 3.0, 0.0], [-3.0, 3.0, 3.0], [-3.0, -3.0, 3.0], [0.35, 0.5, 0.4], 0.9);
    // A faceted sphere (icosphere-ish by lat/long) sitting on the floor.
    let (cx, cy, cz, r) = (0.3f32, 0.2f32, 0.75f32, 0.75f32);
    let pt = |i: usize, j: usize| {
        let th = (i as f32 / 16.0) * std::f32::consts::PI;
        let ph = (j as f32 / 24.0) * std::f32::consts::TAU;
        [cx + r * th.sin() * ph.cos(), cy + r * th.sin() * ph.sin(), cz + r * th.cos()]
    };
    for i in 0..16 {
        for j in 0..24 {
            let (a, b, c, d) = (pt(i, j), pt(i + 1, j), pt(i + 1, j + 1), pt(i, j + 1));
            let mut t1 = ExportTri::plain([a, b, c], [0.8, 0.75, 0.6], 0.25, 1.0);
            let mut t2 = ExportTri::plain([a, c, d], [0.8, 0.75, 0.6], 0.25, 1.0);
            t1.metallic = 1.0;
            t2.metallic = 1.0;
            tris.push(t1);
            tris.push(t2);
        }
    }
    let scene = Scene::build(&tris);
    let sun = Vec3::new(-0.4, -0.5, 0.77).normalize();
    let mut dome = Sky::new(sun, env::DEFAULT_TURBIDITY);
    dome.calibrate([0.55, 0.60, 0.72], [0.16, 0.15, 0.13], [2.4, 2.3, 2.15]);
    let sky = PtSky { sun_dir: sun, sun_col: [2.4, 2.3, 2.15], sky_col: dome.radiance(Vec3::Z), ground_col: dome.ground, dome: Some(dome), env: None, env_strength: 1.0, env_rot: 0.0 };
    let cam = Camera { eye: Vec3::new(2.6, -3.4, 2.0), target: Vec3::new(0.0, 0.3, 0.8), fov_deg: 45.0 };
    let (w, h) = (400usize, 300usize);
    let set = Settings { w, h, passes: 8, max_depth: 5, color: ColorPipeline { exposure: 0.4, ..Default::default() } };
    let job = crate::pathtrace::RenderJob::start(scene, cam, sky, set, crate::pathtrace::Device::Cpu);
    let t0 = std::time::Instant::now();
    while !job.is_done() && t0.elapsed().as_secs() < 300 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let raw = job.snapshot_rgba_opt(false).expect("noisy frame");
    let den = job.snapshot_rgba_opt(true).expect("denoised frame");
    (raw, den)
}

#[test]
#[ignore = "writes report figures; run explicitly"]
fn generate_report_figures() {
    let dir = std::env::var("SIMLUX_FIG_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    std::fs::create_dir_all(&dir).expect("figure dir");
    let mut figs: Vec<(String, String)> = Vec::new();
    let pngdir = dir.clone();
    let mut add = |name: &str, (w, h, px): (usize, usize, Vec<u8>)| {
        let uri = png_data_uri(w, h, &px);
        // Also drop the PNG on disk, so a figure can be eyeballed without opening the report.
        let _ = image::save_buffer(pngdir.join(format!("{name}.png")), &px, w as u32, h as u32, image::ExtendedColorType::Rgba8);
        figs.push((name.to_string(), uri));
        eprintln!("  {name}: {w}×{h}");
    };

    add("tone_curves", fig_tone_curves());
    add("tone_strip", fig_tone_strip());
    add("sky_t2", fig_sky(2.0, 55.0));
    add("sky_t4", fig_sky(4.0, 55.0));
    add("sky_t9", fig_sky(9.0, 55.0));
    add("sky_low", fig_sky(3.0, 6.0));
    add("ambient_compare", fig_ambient_compare());

    // The rebuilt library, on measured values.
    for p in crate::factory::material_presets() {
        let name = format!("mat_{}", p.name.to_lowercase().replace([' ', '(', ')', '—'], "_"));
        let prev = crate::matball::Preview {
            albedo: crate::color::srgb_to_linear3(p.def.avg_color()),
            roughness: p.roughness,
            metallic: p.metallic,
            ior: p.ior,
            opacity: p.opacity,
            emission: {
                let e = crate::color::srgb_to_linear3(p.emission);
                [e[0] * p.emission_strength, e[1] * p.emission_strength, e[2] * p.emission_strength]
            },
            proc: Some(p.def),
        };
        add(&name, ball_of(&prev, 128));
    }

    // Procedural PBR: the same oak with and without the finish/relief the pattern now drives.
    let plain_oak = ProcDef { surf_rough: [0.5, 0.5], bump: 0.0, ..ProcDef::oak() };
    add("proc_before", ball_of(&preview([0.5; 3], 0.6, 0.0, Some(plain_oak)), 200));
    add("proc_after", ball_of(&preview([0.5; 3], 0.6, 0.0, Some(ProcDef::oak())), 200));

    // Roughness sweep on a metal — what the split-sum environment term buys.
    for (k, r) in [0.05f32, 0.2, 0.45, 0.8].iter().enumerate() {
        add(&format!("rough_{k}"), ball_of(&preview([0.95, 0.96, 0.97], *r, 1.0, None), 128));
    }
    // …and a checker, which is the clearest demonstration that roughness follows the pattern.
    let tiles = ProcDef {
        pattern: ProcPattern::Checker,
        col_a: [0.78, 0.78, 0.77],
        col_b: [0.10, 0.10, 0.11],
        scale: [6.0, 6.0, 6.0],
        detail: 1.0,
        rough: 0.5,
        contrast: 1.0,
        ramp: [0.0, 1.0],
        surf_rough: [0.08, 0.75],
        bump: 0.0,
    };
    add("proc_tiles", ball_of(&preview([0.5; 3], 0.3, 0.0, Some(tiles)), 200));

    let (raw, den) = fig_denoise();
    add("pt_noisy", raw);
    add("pt_denoised", den);

    // Emit as a script the report can inline verbatim.
    let mut js = String::from("<script>\nwindow.FIGS = {\n");
    for (n, uri) in &figs {
        js.push_str(&format!("  {n:?}: {uri:?},\n"));
    }
    js.push_str("};\n</script>\n");
    let out = dir.join("figs.js");
    std::fs::write(&out, &js).expect("write figs.js");
    eprintln!("\n{} figures -> {} ({} KB)", figs.len(), out.display(), js.len() / 1024);
}
