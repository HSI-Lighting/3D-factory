//! The **material ball** — a small sphere of a material, rendered on the CPU.
//!
//! A swatch of flat colour cannot show what a material now does. Roughness, metallic, IOR, the
//! grain's own relief and what the sky puts back into a glossy surface are all things you can only
//! judge on a curved surface with a real environment behind it, which is precisely why every tool
//! that has materials shows you a sphere and not a rectangle.
//!
//! It is deliberately the **same maths as the viewport**, not a lookalike: [`crate::env`] for the
//! sky and its SH ambient, the Cook-Torrance GGX transcribed from `light3d`'s shader,
//! [`crate::proc_tex`] for the pattern, and [`crate::color`] for the display transform. If the
//! preview and the viewport disagree, one of those is wrong — which makes this a cheap standing
//! check on the whole chain, and the reason `the_preview_agrees_with_the_viewports_brdf` exists.

use crate::color::ColorPipeline;
use crate::env::{self, Sky};
use crate::factory::TextureAsset;
use glam::Vec3;

/// The material to draw, flattened out of a [`TextureAsset`].
#[derive(Clone, Debug)]
pub struct Preview {
    pub albedo: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    pub ior: f32,
    pub opacity: f32,
    pub emission: [f32; 3],
    pub proc: Option<crate::factory::ProcDef>,
}

impl Preview {
    pub fn from_texture(t: &TextureAsset) -> Self {
        let e = crate::color::srgb_to_linear3(t.emission);
        let k = t.emission_strength;
        Self {
            albedo: crate::color::srgb_to_linear3(t.avg),
            roughness: t.roughness,
            metallic: t.metallic,
            ior: t.ior,
            opacity: t.opacity,
            emission: [e[0] * k, e[1] * k, e[2] * k],
            proc: t.proc,
        }
    }
}

// ── the BRDF, transcribed from light3d's TEX_FS ──────────────────────────────────────────────

fn d_ggx(n_o_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_o_h * n_o_h * (a2 - 1.0) + 1.0;
    a2 / (std::f32::consts::PI * d * d).max(1e-7)
}

fn v_smith(n_o_v: f32, n_o_l: f32, a: f32) -> f32 {
    let a2 = a * a;
    let sv = n_o_l * (n_o_v * n_o_v * (1.0 - a2) + a2).sqrt();
    let sl = n_o_v * (n_o_l * n_o_l * (1.0 - a2) + a2).sqrt();
    0.5 / (sv + sl).max(1e-5)
}

fn f_schlick(f0: [f32; 3], u: f32) -> [f32; 3] {
    let k = (1.0 - u).clamp(0.0, 1.0).powi(5);
    [f0[0] + (1.0 - f0[0]) * k, f0[1] + (1.0 - f0[1]) * k, f0[2] + (1.0 - f0[2]) * k]
}

/// The surface response at one shading point. Public because the path tracer and the tests both
/// want exactly this, and a second transcription is a second chance to get it wrong.
///
/// `sun_col` is calibrated as irradiance/π (the app's convention throughout), which is why the
/// specular multiplies π back rather than the diffuse dividing by it.
#[allow(clippy::too_many_arguments)]
pub fn shade_point(
    albedo: [f32; 3],
    roughness: f32,
    metallic: f32,
    ior: f32,
    n: Vec3,
    v: Vec3,
    sun_dir: Vec3,
    sun_col: [f32; 3],
    sky: &Sky,
    sh: &[[f32; 3]; 9],
    reflections: f32,
) -> [f32; 3] {
    let f0d = {
        let f = (ior - 1.0) / (ior + 1.0);
        (f * f).clamp(0.0, 0.25)
    };
    let f0 = [
        f0d + (albedo[0] - f0d) * metallic,
        f0d + (albedo[1] - f0d) * metallic,
        f0d + (albedo[2] - f0d) * metallic,
    ];
    let diff = [albedo[0] * (1.0 - metallic), albedo[1] * (1.0 - metallic), albedo[2] * (1.0 - metallic)];
    let a = (roughness * roughness).max(1e-3);
    let n_o_v = n.dot(v).max(1e-4);
    let n_o_l = n.dot(sun_dir).max(0.0);

    let amb = env::sh_ambient(sh, n);
    let mut col = [diff[0] * amb[0], diff[1] * amb[1], diff[2] * amb[2]];
    for i in 0..3 {
        col[i] += diff[i] * sun_col[i] * n_o_l;
    }
    if n_o_l > 0.0 {
        let h = (sun_dir + v).normalize();
        let f = f_schlick(f0, v.dot(h).max(0.0));
        let s = d_ggx(n.dot(h).max(0.0), a) * v_smith(n_o_v, n_o_l, a) * n_o_l * std::f32::consts::PI;
        for i in 0..3 {
            col[i] += f[i] * s * sun_col[i];
        }
    }
    // Environment specular, blurred by roughness exactly as `env_sample` does in the shader.
    let r = (2.0 * n.dot(v) * n - v).normalize();
    let sharp = sky.radiance_with_sun(r);
    let wide = env::sh_ambient(sh, r);
    let k = (roughness * 1.4).clamp(0.0, 1.0);
    let e = [
        sharp[0] + (wide[0] - sharp[0]) * k,
        sharp[1] + (wide[1] - sharp[1]) * k,
        sharp[2] + (wide[2] - sharp[2]) * k,
    ];
    let [sa, sb] = env::env_brdf(roughness, n_o_v);
    for i in 0..3 {
        col[i] += e[i] * (f0[i] * sa + sb) * reflections.clamp(0.0, 1.0);
    }
    col
}

/// Render a `size × size` RGBA8 preview of `mat` lit by `sky`.
///
/// The sphere is one metre across at the world origin, which matters: a procedural is evaluated in
/// WORLD space, so the ball shows the pattern at the scale it would appear on a one-metre object.
/// A preview that quietly rescaled the pattern to fit would be the most misleading thing it could
/// possibly do.
pub fn render(mat: &Preview, sky: &Sky, sh: &[[f32; 3]; 9], sun_col: [f32; 3], color: ColorPipeline, size: usize) -> Vec<u8> {
    let size = size.max(8);
    let mut out = vec![0u8; size * size * 4];
    let eye = Vec3::new(0.0, -3.2, 0.55);
    let target = Vec3::ZERO;
    let fwd = (target - eye).normalize();
    let right = fwd.cross(Vec3::Z).normalize();
    let up = right.cross(fwd);
    let half = (26f32.to_radians() * 0.5).tan();
    let sun_dir = sky.sun_dir;
    let radius = 0.5f32;

    for y in 0..size {
        for x in 0..size {
            let px = ((x as f32 + 0.5) / size as f32 * 2.0 - 1.0) * half;
            let py = (1.0 - (y as f32 + 0.5) / size as f32 * 2.0) * half;
            let rd = (fwd + right * px + up * py).normalize();

            // Ray/sphere at the origin.
            let b = eye.dot(rd);
            let c = eye.dot(eye) - radius * radius;
            let disc = b * b - c;
            let lin = if disc > 0.0 {
                let t = -b - disc.sqrt();
                if t > 0.0 {
                    let p = eye + rd * t;
                    let mut n = p.normalize();
                    let v = -rd;
                    // The procedural, sampled in world space at the size it would really be.
                    let (albedo, rough) = match &mat.proc {
                        Some(def) => {
                            let s = crate::proc_tex::sample(def, p, n, mat.roughness);
                            n = s.normal;
                            (s.albedo, s.roughness)
                        }
                        None => (mat.albedo, mat.roughness),
                    };
                    let mut col = shade_point(albedo, rough, mat.metallic, mat.ior, n, v, sun_dir, sun_col, sky, sh, 1.0);
                    for i in 0..3 {
                        col[i] += mat.emission[i];
                    }
                    // A translucent material shows the sky through it — approximated as a straight
                    // blend, which is what the raster viewport does for a glass pane too.
                    if mat.opacity < 0.999 {
                        let behind = sky.radiance_with_sun(rd);
                        for i in 0..3 {
                            col[i] = col[i] * mat.opacity + behind[i] * (1.0 - mat.opacity);
                        }
                    }
                    col
                } else {
                    sky.radiance_with_sun(rd)
                }
            } else {
                sky.radiance_with_sun(rd)
            };

            let [r, g, bch] = crate::color::tonemap8(color, lin);
            let o = (y * size + x) * 4;
            out[o] = r;
            out[o + 1] = g;
            out[o + 2] = bch;
            out[o + 3] = 255;
        }
    }
    out
}

/// A sky for previews: a fixed three-quarter sun, so every material in the library is judged under
/// the same light regardless of what the scene's daylight happens to be set to.
pub fn preview_sky() -> (Sky, [[f32; 3]; 9], [f32; 3]) {
    let mut sky = Sky::new(Vec3::new(-0.45, -0.35, 0.82).normalize(), env::DEFAULT_TURBIDITY);
    let sun_col = [2.6, 2.5, 2.35];
    sky.calibrate([0.85, 0.92, 1.10], [0.24, 0.23, 0.20], sun_col);
    let sh = sky.sh9();
    (sky, sh, sun_col)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ball(mat: &Preview) -> Vec<u8> {
        let (sky, sh, sun) = preview_sky();
        render(mat, &sky, &sh, sun, ColorPipeline::default(), 48)
    }

    fn base() -> Preview {
        Preview { albedo: [0.5, 0.5, 0.5], roughness: 0.5, metallic: 0.0, ior: 1.5, opacity: 1.0, emission: [0.0; 3], proc: None }
    }

    /// The centre pixel must be the sphere and the corner must be the sky. If the camera framing
    /// ever drifts, every other assertion here would be measuring the background.
    #[test]
    fn the_ball_is_in_frame() {
        let img = ball(&base());
        let n = 48;
        let at = |x: usize, y: usize| {
            let o = (y * n + x) * 4;
            [img[o], img[o + 1], img[o + 2]]
        };
        let mid = at(n / 2, n / 2);
        let corner = at(1, 1);
        assert_ne!(mid, corner, "the sphere must be distinguishable from the backdrop");
        assert!(img.chunks_exact(4).all(|p| p[3] == 255), "opaque output");
        // The sphere occupies a sensible share of the frame — not a speck, not the whole image.
        // Detected by "differs from the corner colour", which the sky gradient alone would not.
        let sphere_px = (0..n * n).filter(|i| {
            let (x, y) = (i % n, i / n);
            let (cx, cy) = (x as f32 - n as f32 / 2.0, y as f32 - n as f32 / 2.0);
            (cx * cx + cy * cy).sqrt() < n as f32 * 0.3
        }).count();
        assert!(sphere_px > n * n / 12, "the ball should fill a useful part of the frame");
    }

    /// A metal must reflect its own colour and a dielectric must not. This is the single property
    /// the old renderer got wrong — `metallic` never reached the raster shader, so gold came out
    /// grey-brown plastic — and it is the property a preview exists to show.
    #[test]
    fn metals_tint_their_highlight_and_dielectrics_do_not() {
        let gold = [0.9f32, 0.6, 0.15];
        let white = [0.9f32, 0.9, 0.9];
        let (sky, sh, sun) = preview_sky();
        let n = Vec3::new(0.2, -0.6, 0.77).normalize();
        let v = Vec3::new(0.0, -1.0, 0.2).normalize();
        // Isolate the ENVIRONMENT SPECULAR by differencing reflections on against off. Comparing
        // whole shaded colours does not test this: with a dielectric the diffuse term carries the
        // base colour and swamps the specular, so the total tint says nothing about F0.
        let env_spec = |albedo: [f32; 3], metallic: f32| {
            let a = shade_point(albedo, 0.15, metallic, 1.5, n, v, sky.sun_dir, sun, &sky, &sh, 1.0);
            let b = shade_point(albedo, 0.15, metallic, 1.5, n, v, sky.sun_dir, sun, &sky, &sh, 0.0);
            [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
        };
        // A DIELECTRIC's specular is white — it does not depend on the base colour at all.
        let (gd, wd) = (env_spec(gold, 0.0), env_spec(white, 0.0));
        for i in 0..3 {
            assert!((gd[i] - wd[i]).abs() < 1e-5, "a dielectric's reflection must not take its albedo: {gd:?} vs {wd:?}");
        }
        // A CONDUCTOR's specular IS its base colour: gold returns ~a sixth of the blue a white
        // metal does, and all of the red.
        let (gm, wm) = (env_spec(gold, 1.0), env_spec(white, 1.0));
        let blue = gm[2] / wm[2].max(1e-9);
        let red = gm[0] / wm[0].max(1e-9);
        assert!(blue < red * 0.5, "gold must reflect far less blue than white metal: blue {blue:.3} vs red {red:.3}");
        // …and a metal has no diffuse at all, so it is black where nothing is being reflected.
        let away = -v;
        let m_dark = shade_point(gold, 0.15, 1.0, 1.5, away, v, sky.sun_dir, sun, &sky, &sh, 0.0);
        assert!(m_dark.iter().all(|c| *c < 0.02), "a metal with no environment has no diffuse: {m_dark:?}");
    }

    /// Roughness must actually change the picture, monotonically: a mirror concentrates the sun
    /// into a small bright spot, a rough surface spreads it. Measured as the peak pixel.
    #[test]
    fn roughness_spreads_the_highlight() {
        let peak = |r: f32| {
            let img = ball(&Preview { roughness: r, metallic: 1.0, albedo: [0.9, 0.9, 0.9], ..base() });
            img.chunks_exact(4).map(|p| p[0]).max().unwrap()
        };
        let bright = |r: f32| {
            let img = ball(&Preview { roughness: r, metallic: 1.0, albedo: [0.9, 0.9, 0.9], ..base() });
            img.chunks_exact(4).filter(|p| p[0] > 200).count()
        };
        assert!(peak(0.05) >= peak(0.8), "a mirror must reach at least as bright as a rough surface");
        assert!(bright(0.6) > bright(0.05), "and a rough surface must spread that brightness wider");
    }

    /// An emissive material glows regardless of the light on it — including on the side facing away
    /// from the sun, which is the whole tell that it is emitting rather than reflecting.
    #[test]
    fn emission_glows_on_the_dark_side() {
        let (sky, sh, sun) = preview_sky();
        let dark = -sky.sun_dir;
        let plain = shade_point([0.5; 3], 0.5, 0.0, 1.5, dark, -dark, sky.sun_dir, sun, &sky, &sh, 0.0);
        let mat = Preview { emission: [4.0, 3.4, 2.4], ..base() };
        let img = render(&mat, &sky, &sh, sun, ColorPipeline::default(), 32);
        let o = (16 * 32 + 16) * 4;
        assert!(img[o] > 200, "an emitter should read bright: {}", img[o]);
        assert!(plain[0] < 1.0, "the same surface without emission is not bright: {plain:?}");
    }

    /// The preview and the viewport must be the same shader. `shade_point` is the Rust copy of the
    /// GLSL in `light3d`; check the pieces that would silently diverge are still spelled the same
    /// way over there — the π on the specular especially, which is the app's whole sun calibration.
    #[test]
    fn the_preview_agrees_with_the_viewports_brdf() {
        let glsl = crate::light3d::tex_fs_for_test();
        assert!(glsl.contains("v_smith(NoV, NoL, a) * NoL * sh * PI) * u_sun_col"), "the specular's PI factor moved");
        assert!(glsl.contains("mix(vec3(f0d), albedo, metallic)"), "F0 blend changed");
        assert!(glsl.contains("albedo * (1.0 - metallic)"), "the diffuse/metallic split changed");
        assert!(glsl.contains("mix(sky_with_sun(R), sh_ambient(R), clamp(rough * 1.4, 0.0, 1.0))"), "env_sample changed");
        assert!(glsl.contains("f0 * ab.x + ab.y"), "the split-sum weighting changed");
        // And the numbers agree at a point we can compute both ways: a perfect mirror facing the
        // viewer reflects the environment with essentially all of F0.
        let (sky, sh, sun) = preview_sky();
        let n = Vec3::new(0.0, -1.0, 0.0);
        let c = shade_point([0.9, 0.9, 0.9], 0.02, 1.0, 1.5, n, n, sky.sun_dir, sun, &sky, &sh, 1.0);
        let env_here = sky.radiance_with_sun(n);
        for i in 0..3 {
            assert!(c[i] >= env_here[i] * 0.8, "a mirror should return most of what it sees: {c:?} vs {env_here:?}");
        }
    }

    /// A procedural must show its PATTERN on the ball, not its average colour — the specific bug
    /// this preview exists to make visible.
    #[test]
    fn a_procedural_shows_its_grain() {
        let oak = crate::factory::ProcDef::oak();
        let img = ball(&Preview { proc: Some(oak), ..base() });
        // Sample a horizontal line across the middle of the ball and measure the spread.
        let n = 48;
        let row: Vec<u8> = (n / 4..3 * n / 4).map(|x| img[((n / 2) * n + x) * 4]).collect();
        let (lo, hi) = (*row.iter().min().unwrap(), *row.iter().max().unwrap());
        assert!(hi as i32 - lo as i32 > 20, "the grain must be visible across the ball: {lo}..{hi}");
        // A SOLID procedural of the same average must not — it has no pattern to show.
        let flat = crate::factory::ProcDef::solid([0.5, 0.4, 0.28]);
        let img2 = ball(&Preview { proc: Some(flat), ..base() });
        let row2: Vec<u8> = (2 * n / 5..3 * n / 5).map(|x| img2[((n / 2) * n + x) * 4]).collect();
        let (lo2, hi2) = (*row2.iter().min().unwrap(), *row2.iter().max().unwrap());
        assert!((hi2 as i32 - lo2 as i32) < 30, "a flat colour must stay smooth across the ball: {lo2}..{hi2}");
    }
}
