//! **Image-based lighting** — a real HDR environment, in place of the analytic sky.
//!
//! [`crate::env`] can synthesise a Preetham sky, which is right for a daylight study where the sun
//! position is the answer. It cannot be a photograph of a real place, and that is what a renderer
//! needs to look like a renderer: a blank Blender scene already has an HDRI behind it, and most of
//! the difference between "CG" and "photographed" is that one image doing the lighting.
//!
//! What was here before approximated the specular environment by mixing a mirror sample of the sky
//! with the SH ambient, weighted by roughness (`env_sample` in `light3d`). That is a decent cheat
//! and costs nothing, but it cannot show a rough metal the shape of the window that lit it. This
//! module produces the real thing:
//!
//! - **[`EnvMap::sh9`]** — the 9 spherical-harmonic coefficients of the environment's diffuse
//!   irradiance, which is the same representation the analytic sky already feeds the shaders, so
//!   every diffuse consumer picks an HDRI up with no further change.
//! - **[`EnvMap::prefilter`]** — a GGX-convolved chain, one level per roughness. Level 0 is the
//!   environment itself (a mirror); each level after it is blurrier by exactly the lobe a surface
//!   of that roughness would gather.
//!
//! **Equirectangular, not a cubemap.** The textbook answer prefilters into cubemap mips, which on
//! GL 3.3 without compute means rendering six faces per level, per frame, through a geometry shader
//! or six draw calls. A lat-long map is one ordinary 2D texture with ordinary mips, samples in four
//! instructions, and is the format every HDRI is distributed in anyway — so nothing has to be
//! converted, and the shader side stays a `textureLod`. The distortion at the poles is real and is
//! irrelevant here, because it only affects the blurriest levels, which are blur.
//!
//! Coordinates are the app's: **Z up**, `u` running anticlockwise from +X, `v` from the zenith.

use glam::Vec3;

/// An equirectangular HDR environment, in LINEAR radiance.
#[derive(Clone, Debug)]
pub struct EnvMap {
    pub w: usize,
    pub h: usize,
    /// Row-major, row 0 at the zenith (+Z). Linear, unbounded — a sun here is genuinely thousands.
    pub px: Vec<[f32; 3]>,
    pub name: String,
}

/// One level of the prefiltered specular chain: the environment as a surface of `roughness` sees it.
#[derive(Clone, Debug)]
pub struct EnvMip {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 3]>,
    pub roughness: f32,
}

/// Width of level 0 — the MIRROR. A polished surface reflects the environment pixel-for-pixel, so
/// this level carries real detail and has to stay sharp; the levels below it are blur by definition
/// and halve from here.
const CHAIN_BASE_W: usize = 1024;

/// How many prefiltered levels. Level 0 is the mirror; the rest span roughness 0→1.
///
/// Each level is exactly half the one above, which makes the chain a **real GL mip chain** — one
/// 2D texture, `lod = roughness × 5`, one texture unit, and the hardware's trilinear filter doing
/// the interpolation between roughnesses for free. Two separate textures (a sharp one and a blurry
/// one, blended in the shader) would cost a second sampler and a hand-written blend for exactly the
/// same picture.
pub const PREFILTER_LEVELS: usize = 6;

/// Samples per texel, per level. The narrow lobes near the mirror end need the most rays; by the
/// time the lobe covers a hemisphere the answer barely moves. Falls off with roughness exactly as
/// the lobe widens.
const LEVEL_SAMPLES: [usize; PREFILTER_LEVELS] = [0, 192, 128, 96, 64, 48];

impl EnvMap {
    /// Read an `.hdr`, `.exr`, or ordinary 8-bit image.
    ///
    /// An 8-bit file is accepted and decoded from sRGB, because someone will try it — but it cannot
    /// carry the dynamic range that makes IBL work, so [`Self::is_hdr`] reports the difference and
    /// the caller can say so.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "environment".into());
        let rgb = img.to_rgb32f();
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);
        if w < 2 || h < 2 {
            return Err(format!("{}: too small to be an environment", path.display()));
        }
        // `to_rgb32f` already linearises an 8-bit source and passes float data through untouched,
        // so both paths arrive as linear radiance and nothing is decoded twice.
        let px: Vec<[f32; 3]> = rgb.pixels().map(|p| [p[0], p[1], p[2]]).collect();
        Ok(Self { w, h, px, name })
    }

    /// Build one directly — for tests, and for turning an analytic sky into a map.
    pub fn from_fn(w: usize, h: usize, name: &str, f: impl Fn(Vec3) -> [f32; 3]) -> Self {
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                px.push(f(Self::texel_dir(x, y, w, h)));
            }
        }
        Self { w, h, px, name: name.to_string() }
    }

    /// Does this environment actually carry high dynamic range? An IBL lit by a map whose brightest
    /// pixel is 1.0 has no sun in it, and will look flat however good the shading is.
    pub fn is_hdr(&self) -> bool {
        self.peak() > 1.05
    }

    /// The brightest channel anywhere — the sun, in an outdoor map.
    pub fn peak(&self) -> f32 {
        self.px.iter().flatten().copied().fold(0.0f32, f32::max)
    }

    /// The direction a texel centre looks along. Z up, `v = 0` at the zenith.
    fn texel_dir(x: usize, y: usize, w: usize, h: usize) -> Vec3 {
        let phi = ((x as f32 + 0.5) / w as f32) * std::f32::consts::TAU;
        let theta = ((y as f32 + 0.5) / h as f32) * std::f32::consts::PI;
        let (st, ct) = theta.sin_cos();
        Vec3::new(st * phi.cos(), st * phi.sin(), ct)
    }

    /// Bilinear sample along `dir`, wrapping in `u` and clamping in `v`.
    pub fn sample(&self, dir: Vec3) -> [f32; 3] {
        sample_px(&self.px, self.w, self.h, dir)
    }

    /// The 9 spherical-harmonic coefficients of this environment, the representation
    /// [`crate::env::sh_ambient`] already consumes — so a diffuse surface picks an HDRI up with no
    /// other change anywhere.
    ///
    /// Projected off a DOWNSAMPLED copy. The irradiance of a 9-coefficient basis cannot hold detail
    /// finer than a hemisphere anyway, so integrating four million pixels to produce nine numbers is
    /// four million pixels of wasted work; and a lit scene must not wait seconds for its lighting.
    pub fn sh9(&self) -> [[f32; 3]; 9] {
        let small = self.resized(64.min(self.w), 32.min(self.h).max(2));
        let mut sh = [[0.0f32; 3]; 9];
        let mut total = 0.0f32;
        for y in 0..small.h {
            for x in 0..small.w {
                let d = Self::texel_dir(x, y, small.w, small.h);
                // Solid angle of the texel: sinθ dθ dφ. Without it the poles — which are a sliver
                // of sky spread across a whole row of pixels — would count as much as the horizon.
                let theta = ((y as f32 + 0.5) / small.h as f32) * std::f32::consts::PI;
                let dw = theta.sin() * (std::f32::consts::PI / small.h as f32)
                    * (std::f32::consts::TAU / small.w as f32);
                let c = small.px[y * small.w + x];
                let b = sh_basis(d);
                for i in 0..9 {
                    for k in 0..3 {
                        sh[i][k] += c[k] * b[i] * dw;
                    }
                }
                total += dw;
            }
        }
        // The projection is complete as it stands — `Σ L·Y·dω` IS the coefficient, and
        // `env::sh_ambient` carries the convolution constants that turn it into irradiance. The
        // ONLY correction wanted here is for the quadrature: sampling at row centres does not sum
        // to exactly 4π, so scale by however far off it came, which is a factor of ~1. Dividing by
        // 4π as well (the easy slip, since 4π appears in every derivation of this) darkens every
        // surface in the scene by 12.6× and reads as a broken tone map rather than a broken sum.
        let k = if total > 1e-6 { 4.0 * std::f32::consts::PI / total } else { 1.0 };
        for c in &mut sh {
            for v in c.iter_mut() {
                *v *= k;
            }
        }
        sh
    }

    /// The GGX-convolved chain. Level 0 is a mirror (this map, downsampled for upload); each later
    /// level is the environment as a surface of that roughness gathers it.
    pub fn prefilter(&self) -> Vec<EnvMip> {
        let mut out = Vec::with_capacity(PREFILTER_LEVELS);
        for level in 0..PREFILTER_LEVELS {
            let rough = level as f32 / (PREFILTER_LEVELS - 1) as f32;
            // Never finer than the source: convolving up to 1024 from a 64-wide map would invent
            // detail it does not have.
            let base = CHAIN_BASE_W.min(self.w.next_power_of_two());
            if level == 0 {
                // THE MIRROR, and it keeps its resolution. A polished surface reflects the
                // environment pixel-for-pixel; putting this level on the same footing as the blurry
                // ones would make chrome reflect a 128-pixel-wide smear of the world, which is
                // precisely the "specular is approximated" failure this module exists to remove.
                // There is nothing to convolve — a mirror gathers exactly one direction.
                let m = self.resized(base, base / 2);
                out.push(EnvMip { w: m.w, h: m.h, px: m.px, roughness: 0.0 });
                continue;
            }
            let w = (base >> level).max(8);
            let h = (w / 2).max(4);
            // Convolve from a source only a little finer than the target. Importance-sampling a 4K
            // map directly leaves fireflies — a sun one ray hits and its neighbour misses — so the
            // source is pre-averaged instead, which is what a mip chain is for.
            let src = self.resized((w * 2).min(self.w).max(8), (h * 2).min(self.h).max(4));
            let n_samples = LEVEL_SAMPLES[level];
            let mut px = vec![[0.0f32; 3]; w * h];
            // Rows in parallel: a narrow lobe at low roughness costs 192 rays a texel, and the
            // whole chain has to be ready before the first frame draws.
            let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16);
            let band = h.div_ceil(threads);
            std::thread::scope(|s| {
                for (bi, rows) in px.chunks_mut(band * w).enumerate() {
                    let src = &src;
                    s.spawn(move || {
                        for (ry, row) in rows.chunks_mut(w).enumerate() {
                            let y = bi * band + ry;
                            for (x, texel) in row.iter_mut().enumerate() {
                                let n = Self::texel_dir(x, y, w, h);
                                *texel = ggx_convolve(src, n, rough, n_samples);
                            }
                        }
                    });
                }
            });
            out.push(EnvMip { w, h, px, roughness: rough });
        }
        out
    }

    /// Box-downsample to `w × h`. Averaging every source texel that lands in a target one, rather
    /// than point-sampling, is what keeps a one-pixel sun from disappearing between samples.
    pub fn resized(&self, w: usize, h: usize) -> EnvMap {
        let (w, h) = (w.max(2), h.max(1));
        if w >= self.w && h >= self.h {
            return self.clone();
        }
        let mut px = vec![[0.0f32; 3]; w * h];
        let mut n = vec![0.0f32; w * h];
        for sy in 0..self.h {
            let ty = (sy * h / self.h).min(h - 1);
            for sx in 0..self.w {
                let tx = (sx * w / self.w).min(w - 1);
                let o = ty * w + tx;
                let c = self.px[sy * self.w + sx];
                for k in 0..3 {
                    px[o][k] += c[k];
                }
                n[o] += 1.0;
            }
        }
        for (p, c) in px.iter_mut().zip(&n) {
            if *c > 0.0 {
                for k in 0..3 {
                    p[k] /= c;
                }
            }
        }
        EnvMap { w, h, px, name: self.name.clone() }
    }

    /// The whole chain flattened for GPU upload: every level's texels, back to back, plus each
    /// level's size. One 2D texture with explicit mips is all GL 3.3 needs.
    pub fn upload_chain(&self) -> (Vec<EnvMip>, [[f32; 3]; 9]) {
        (self.prefilter(), self.sh9())
    }
}

/// Bilinear equirectangular lookup, wrapping in longitude and clamping in latitude.
pub fn sample_px(px: &[[f32; 3]], w: usize, h: usize, dir: Vec3) -> [f32; 3] {
    if px.is_empty() {
        return [0.0; 3];
    }
    let d = dir.normalize_or(Vec3::Z);
    let u = (d.y.atan2(d.x) / std::f32::consts::TAU + 1.0).fract();
    let v = (d.z.clamp(-1.0, 1.0).acos()) / std::f32::consts::PI;
    let fx = u * w as f32 - 0.5;
    let fy = (v * h as f32 - 0.5).clamp(0.0, h as f32 - 1.0);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let xi = |x: f32| ((x as i64).rem_euclid(w as i64)) as usize;
    let yi = |y: f32| (y.clamp(0.0, h as f32 - 1.0) as usize).min(h - 1);
    let (x0i, x1i) = (xi(x0), xi(x0 + 1.0));
    let (y0i, y1i) = (yi(y0), yi(y0 + 1.0));
    let g = |x: usize, y: usize| px[y * w + x];
    let (a, b, c, d2) = (g(x0i, y0i), g(x1i, y0i), g(x0i, y1i), g(x1i, y1i));
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bot = c[k] + (d2[k] - c[k]) * tx;
        out[k] = top + (bot - top) * ty;
    }
    out
}

/// The 9 real SH basis functions, in the order [`crate::env::sh_ambient`] expects.
fn sh_basis(d: Vec3) -> [f32; 9] {
    [
        0.282_095,
        0.488_603 * d.y,
        0.488_603 * d.z,
        0.488_603 * d.x,
        1.092_548 * d.x * d.y,
        1.092_548 * d.y * d.z,
        0.315_392 * (3.0 * d.z * d.z - 1.0),
        1.092_548 * d.x * d.z,
        0.546_274 * (d.x * d.x - d.y * d.y),
    ]
}

/// GGX importance-sampled convolution of `src` about `n` at `roughness`.
///
/// The split-sum approximation's first half: assume the view direction equals the normal, which is
/// what lets the result be stored per-roughness instead of per-view. The second half is the BRDF
/// term, which [`crate::env::env_brdf`] already provides.
fn ggx_convolve(src: &EnvMap, n: Vec3, roughness: f32, samples: usize) -> [f32; 3] {
    let n_samples = samples.max(16);
    let a = (roughness * roughness).max(1e-3);
    // An orthonormal basis about n.
    let up = if n.z.abs() < 0.999 { Vec3::Z } else { Vec3::X };
    let tx = up.cross(n).normalize_or(Vec3::X);
    let ty = n.cross(tx);

    let mut acc = [0.0f32; 3];
    let mut wsum = 0.0f32;
    for i in 0..n_samples {
        // Hammersley: deterministic, evenly spread, and identical every run — a prefilter that
        // changed slightly each launch would make renders unreproducible.
        let u1 = (i as f32 + 0.5) / n_samples as f32;
        let u2 = radical_inverse(i as u32);
        // GGX half-vector.
        let phi = std::f32::consts::TAU * u1;
        let cos_t = ((1.0 - u2) / (1.0 + (a * a - 1.0) * u2)).max(0.0).sqrt();
        let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
        let hv = tx * (sin_t * phi.cos()) + ty * (sin_t * phi.sin()) + n * cos_t;
        // Reflect the view (= n) about it.
        let l = (hv * (2.0 * n.dot(hv)) - n).normalize_or(n);
        let n_o_l = n.dot(l);
        if n_o_l <= 0.0 {
            continue;
        }
        let c = src.sample(l);
        for k in 0..3 {
            acc[k] += c[k] * n_o_l;
        }
        wsum += n_o_l;
    }
    if wsum > 1e-6 {
        for v in &mut acc {
            *v /= wsum;
        }
        acc
    } else {
        src.sample(n)
    }
}

/// Van der Corput radical inverse — the second Hammersley coordinate.
fn radical_inverse(mut bits: u32) -> f32 {
    bits = (bits << 16) | (bits >> 16);
    bits = ((bits & 0x5555_5555) << 1) | ((bits & 0xAAAA_AAAA) >> 1);
    bits = ((bits & 0x3333_3333) << 2) | ((bits & 0xCCCC_CCCC) >> 2);
    bits = ((bits & 0x0F0F_0F0F) << 4) | ((bits & 0xF0F0_F0F0) >> 4);
    bits = ((bits & 0x00FF_00FF) << 8) | ((bits & 0xFF00_FF00) >> 8);
    bits as f32 * 2.328_306_4e-10
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(v: f32) -> EnvMap {
        EnvMap::from_fn(64, 32, "grey", |_| [v, v, v])
    }

    /// The direction convention, pinned. Every other number in this file is meaningless if a
    /// texel's direction and the sampler's direction disagree — and the failure looks like
    /// "lighting comes from the wrong side", which is easy to blame on anything else.
    #[test]
    fn a_texel_samples_back_along_its_own_direction() {
        let m = EnvMap::from_fn(64, 32, "dirs", |d| [d.x, d.y, d.z]);
        for d in [
            Vec3::Z, -Vec3::Z, Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y,
            Vec3::new(0.5, -0.3, 0.8).normalize(),
        ] {
            let s = m.sample(d);
            let got = Vec3::from(s);
            assert!(
                (got - d).length() < 0.08,
                "sampling {d:?} returned {got:?} — the equirect mapping disagrees with itself"
            );
        }
    }

    /// A uniform environment must give uniform irradiance: the SH ambient of a grey sky is that
    /// grey, from every direction. This is the ENERGY check — if the normalisation is wrong the
    /// whole scene is uniformly too bright or too dark, which reads as "the tone map is off" and
    /// sends you to the wrong file.
    #[test]
    fn a_uniform_environment_has_uniform_irradiance() {
        for v in [0.25f32, 1.0, 8.0] {
            let sh = grey(v).sh9();
            for n in [Vec3::Z, -Vec3::Z, Vec3::X, Vec3::new(0.3, 0.7, -0.6).normalize()] {
                let a = crate::env::sh_ambient(&sh, n);
                for k in 0..3 {
                    assert!(
                        (a[k] - v).abs() < 0.03 * v.max(1.0),
                        "grey {v} lit {n:?} as {a:?}"
                    );
                }
            }
        }
    }

    /// Irradiance follows the environment: a sky bright overhead and dark below must light an
    /// upward normal more than a downward one.
    #[test]
    fn irradiance_follows_where_the_light_is() {
        let m = EnvMap::from_fn(64, 32, "split", |d| if d.z > 0.0 { [4.0; 3] } else { [0.1; 3] });
        let sh = m.sh9();
        let up = crate::env::sh_ambient(&sh, Vec3::Z)[0];
        let down = crate::env::sh_ambient(&sh, -Vec3::Z)[0];
        let side = crate::env::sh_ambient(&sh, Vec3::X)[0];
        assert!(up > side, "up ({up}) is brighter than sideways ({side})");
        assert!(side > down, "sideways ({side}) is brighter than down ({down})");
        assert!(up > down * 3.0, "and up is much brighter than down");
    }

    /// The prefilter chain must CONSERVE ENERGY: convolving a uniform environment cannot change
    /// it, at any roughness. A convolution that loses energy makes rough metals mysteriously dark.
    #[test]
    fn prefiltering_a_uniform_environment_changes_nothing() {
        let chain = grey(2.0).prefilter();
        assert_eq!(chain.len(), PREFILTER_LEVELS);
        for mip in &chain {
            for p in &mip.px {
                for k in 0..3 {
                    assert!(
                        (p[k] - 2.0).abs() < 0.02,
                        "roughness {}: {} should still be 2.0", mip.roughness, p[k]
                    );
                }
            }
        }
    }

    /// …and it must actually BLUR. A single bright spot spreads as roughness rises, so the peak
    /// falls monotonically. Without this the chain could conserve energy by doing nothing at all.
    #[test]
    fn a_bright_spot_spreads_as_roughness_rises() {
        let sun = Vec3::new(0.3, 0.2, 0.93).normalize();
        let m = EnvMap::from_fn(256, 128, "sun", |d| {
            if d.dot(sun) > 0.995 { [400.0; 3] } else { [0.4, 0.5, 0.7] }
        });
        let chain = m.prefilter();
        let peaks: Vec<f32> = chain
            .iter()
            .map(|l| l.px.iter().flatten().copied().fold(0.0f32, f32::max))
            .collect();
        // Monotonic within a fifth. The blurriest levels are 8×4 texels, where a sun landing on a
        // texel centre rather than a corner shifts the peak by more than rounding — the trend is
        // the claim, not the individual step.
        for w in peaks.windows(2) {
            assert!(w[1] <= w[0] * 1.2, "peak must not grow with roughness: {peaks:?}");
        }
        assert!(peaks[0] > peaks[PREFILTER_LEVELS - 1] * 5.0, "and it spreads a lot: {peaks:?}");
        // The blurriest level is still brighter toward the sun than away from it.
        let last = &chain[PREFILTER_LEVELS - 1];
        let toward = sample_px(&last.px, last.w, last.h, sun)[0];
        let away = sample_px(&last.px, last.w, last.h, -sun)[0];
        assert!(toward > away, "the blur keeps the sun's direction ({toward} vs {away})");
    }

    /// Levels get smaller, and level 0 is the mirror.
    #[test]
    fn the_chain_is_a_chain() {
        let chain = grey(1.0).prefilter();
        assert_eq!(chain[0].roughness, 0.0, "level 0 is a mirror");
        assert_eq!(chain[PREFILTER_LEVELS - 1].roughness, 1.0, "the last is fully rough");
        for w in chain.windows(2) {
            assert!(w[1].w <= w[0].w, "sizes do not grow");
            assert!(w[1].roughness > w[0].roughness, "roughness does");
            assert_eq!(w[1].px.len(), w[1].w * w[1].h, "each level is fully populated");
        }
    }

    /// THE MIRROR LEVEL KEEPS ITS DETAIL. Chrome reflects the world pixel-for-pixel, so level 0 has
    /// to stay near the source resolution — putting it on the same shrinking chain as the blurry
    /// levels made a polished surface reflect a 128-pixel smear of the world, which is exactly the
    /// approximation this module was written to remove.
    #[test]
    fn the_mirror_level_stays_sharp() {
        let src = EnvMap::from_fn(1024, 512, "detail", |d| {
            // A fine checker: only a high-resolution level can still resolve it.
            let f = (d.x * 40.0).sin() * (d.y * 40.0).sin() * (d.z * 40.0).sin();
            if f > 0.0 { [4.0; 3] } else { [0.05; 3] }
        });
        let chain = src.prefilter();
        assert_eq!(chain[0].w, 1024, "the mirror keeps its resolution");
        assert_eq!(chain[1].w, 512, "and each level halves 2014 a real GL mip chain");

        // Contrast survives at level 0 and is gone by the end — detail, then blur.
        let spread = |m: &EnvMip| {
            let mx = m.px.iter().map(|p| p[0]).fold(0.0f32, f32::max);
            let mn = m.px.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
            mx - mn
        };
        assert!(spread(&chain[0]) > 3.0, "the mirror still resolves the pattern");
        assert!(
            spread(&chain[PREFILTER_LEVELS - 1]) < spread(&chain[0]) * 0.5,
            "and the roughest level has averaged it away"
        );
    }

    /// THE TWIN CHECK. `env_uv` in the shader and [`sample_px`] here must agree about which pixel a
    /// direction lands on, or the viewport and every CPU consumer light the scene from different
    /// sides of the world. The GLSL is transcribed here and compared over the whole sphere.
    ///
    /// It also guards the shader text itself: if someone edits `env_uv` in `SKY_GLSL`, the literals
    /// below stop matching and this fails, rather than the difference showing up months later as
    /// "the render doesn't match the viewport".
    #[test]
    fn the_shader_and_the_cpu_agree_on_where_a_direction_lands() {
        let glsl = crate::env::SKY_GLSL;
        assert!(glsl.contains("vec2 env_uv(vec3 d)"), "the shader still has env_uv");
        assert!(glsl.contains("atan(d.y, d.x)"), "…using atan2(y, x), as the CPU does");
        assert!(glsl.contains("6.28318530718"), "…over TAU");
        assert!(glsl.contains("acos(clamp(d.z, -1.0, 1.0))"), "…and acos(z) for latitude");
        assert!(glsl.contains("roughness * 5") || glsl.contains("* 5.0"), "lod = roughness × 5");

        // The GLSL, line for line.
        let shader_uv = |d: Vec3| {
            let d = d.normalize();
            let u = d.y.atan2(d.x) / std::f32::consts::TAU;
            let v = d.z.clamp(-1.0, 1.0).acos() / std::f32::consts::PI;
            // GL_REPEAT in S is what makes a negative u legal; fold it the way the sampler does.
            (u.rem_euclid(1.0), v)
        };

        // A map whose value IS its position, so a disagreement shows up as a wrong colour.
        let m = EnvMap::from_fn(256, 128, "twin", |d| [d.x, d.y, d.z]);
        for (i, d) in [
            Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z,
            Vec3::new(1.0, 1.0, 0.0), Vec3::new(-1.0, 0.3, 0.5), Vec3::new(0.2, -0.9, -0.4),
            Vec3::new(-0.6, -0.6, 0.5),
        ]
        .into_iter()
        .enumerate()
        {
            let d = d.normalize();
            let (su, sv) = shader_uv(d);
            // What the CPU sampler reads at that direction…
            let cpu = m.sample(d);
            // …and what the shader's uv would fetch, resolved through the same texels.
            let x = ((su * m.w as f32 - 0.5).round() as i64).rem_euclid(m.w as i64) as usize;
            let y = ((sv * m.h as f32 - 0.5).round().clamp(0.0, m.h as f32 - 1.0)) as usize;
            let gpu = m.px[y * m.w + x];
            for k in 0..3 {
                assert!(
                    (cpu[k] - gpu[k]).abs() < 0.05,
                    "case {i} dir {d:?}: cpu {cpu:?} vs shader-uv {gpu:?}"
                );
            }
        }
    }

    /// The PATH TRACER and the viewport must be lit by the same environment, including its
    /// rotation. ⏺ Render is sold as a preview of the scene; an offline image lit by a differently
    /// turned sky is a preview of something else.
    /// The rotation is the part that goes wrong silently. A sign error costs nothing at rot = 0
    /// and is invisible until someone turns the sky and compares a render with the viewport side
    /// by side — at which point the sun is coming from the other side of the building.
    ///
    /// So the expectation here is DERIVED from the shader's rule (`u = (atan2(y, x) + rot) / τ`)
    /// rather than restated as a rotation of its own, which is how the old version of this test
    /// managed to pin the wrong convention: the tracer turned the panorama the opposite way from
    /// the viewport, and the test agreed with the tracer.
    #[test]
    fn the_path_tracer_sees_the_same_environment_as_the_viewport() {
        use std::f32::consts::TAU;
        let glsl = crate::env::SKY_GLSL;
        assert!(
            glsl.contains("(atan(d.y, d.x) + u_env_rot)"),
            "the shader ADDS the rotation to the azimuth — the tracer must do the same"
        );

        // A map whose red channel IS its own azimuth, in turns. Whatever the tracer returns names
        // the exact part of the panorama it looked at, so a wrong rotation cannot hide.
        let m = std::sync::Arc::new(EnvMap::from_fn(2048, 1024, "azimuth", |d| {
            [d.y.atan2(d.x).rem_euclid(TAU) / TAU, 0.0, 0.0]
        }));
        let rot = 0.7f32;
        let mut env = crate::env::EnvRender::default();
        env.hdri = Some(crate::env::HdriUse { strength: 1.0, rot });
        let sky = crate::pathtrace::Sky::from_env(Vec3::Z, [1.0; 3], &env).with_env(Some(m));

        for d in [
            Vec3::new(1.0, 0.2, 0.3),
            Vec3::new(0.1, 1.0, -0.2),
            Vec3::new(-0.7, 0.4, 0.5),
            Vec3::new(0.3, -0.9, 0.1),
        ] {
            let d = d.normalize();
            let want = (d.y.atan2(d.x) + rot).rem_euclid(TAU) / TAU; // exactly `env_uv`'s u
            let got = crate::pathtrace::sky_radiance_for_test(&sky, d, true)[0];
            // The seam is a real discontinuity in this map (u wraps 1 → 0) and bilinear filtering
            // smears across it — compare the way a circle does, so a sample beside the seam is not
            // read as being half a world away.
            let err = (got - want).abs().min(1.0 - (got - want).abs());
            assert!(err < 0.01, "dir {d:?}: tracer looked at u={got:.4}, viewport at u={want:.4}");
        }

        // Strength still multiplies, and nothing has been lost from the plain unrotated case.
        env.hdri = Some(crate::env::HdriUse { strength: 3.0, rot: 0.0 });
        let bright = std::sync::Arc::new(EnvMap::from_fn(64, 32, "flat", |_| [0.25; 3]));
        let sky2 = crate::pathtrace::Sky::from_env(Vec3::Z, [1.0; 3], &env).with_env(Some(bright));
        let r = crate::pathtrace::sky_radiance_for_test(&sky2, Vec3::X, true);
        assert!((r[0] - 0.75).abs() < 1e-3, "strength 3 on a 0.25 map is 0.75, got {}", r[0]);
    }

    /// An HDRI must draw its own backdrop, with or without the sun.
    ///
    /// The backdrop pass used to require a VALID ANALYTIC DOME, so switching daylight off — or
    /// simply picking an hour when the sun is down — left an HDRI drawing nothing. In an empty
    /// scene the backdrop is the whole image, so the environment looked as if it had failed to
    /// load. Both halves of the fix are pinned here because neither can be reached without a GL
    /// context: the shader must consult the environment BEFORE the analytic sky, and the draw gate
    /// must accept an HDRI as a sky in its own right.
    #[test]
    fn an_hdri_draws_the_backdrop_without_needing_the_sun() {
        let glsl = crate::env::SKY_GLSL;
        let at_env = glsl.find("u_env_on == 1").expect("sky_with_sun asks about the HDRI");
        let at_sky = glsl.find("u_sky_on == 0) return c").expect("…and about the analytic sky");
        assert!(at_env < at_sky, "the HDRI is consulted first, so the sun's state cannot veto it");

        let src = include_str!("light3d.rs");
        assert!(
            src.contains("env.hdri.is_some() || env.sky.map(|s| s.valid)"),
            "the backdrop gate treats a loaded HDRI as a sky"
        );
        // …and the full-resolution copy has a fallback, so a failed upload is soft rather than black.
        assert!(src.contains("self.env_bg_tex.or(self.env_tex)"), "the backdrop sampler always has a texture");
    }

    /// An 8-bit image is loadable but is NOT an HDR environment, and the difference is reportable —
    /// someone will point this at a JPEG and wonder why nothing looks lit.
    #[test]
    fn low_dynamic_range_is_recognised_as_such() {
        assert!(!grey(1.0).is_hdr(), "a map that peaks at 1.0 has no sun in it");
        assert!(grey(50.0).is_hdr());
        let sunny = EnvMap::from_fn(32, 16, "s", |d| if d.z > 0.99 { [900.0; 3] } else { [0.3; 3] });
        assert!(sunny.is_hdr());
        assert!(sunny.peak() > 800.0);
    }

    /// Downsampling must PRESERVE THE SUN. Point-sampling would drop a one-pixel sun between
    /// samples and the environment would quietly stop lighting anything, which is the exact bug
    /// that makes an HDRI look like a flat grey card.
    #[test]
    fn downsampling_keeps_the_energy_of_a_tiny_sun() {
        let m = EnvMap::from_fn(512, 256, "sun", |d| {
            if d.dot(Vec3::Z) > 0.9995 { [10_000.0; 3] } else { [0.2; 3] }
        });
        let mean = |e: &EnvMap| {
            e.px.iter().map(|p| p[0] as f64).sum::<f64>() / e.px.len() as f64
        };
        let before = mean(&m);
        let after = mean(&m.resized(64, 32));
        // Equirect rows are not equal-area, so the means differ a little; an order of magnitude
        // apart would mean the sun was lost.
        assert!(after > before * 0.3, "the sun survived downsampling ({before} → {after})");
        assert!(m.resized(64, 32).peak() > 100.0, "and is still much brighter than the sky");
    }
}
