//! The **Rust twin** of the procedural material evaluated in `light3d`'s fragment shader.
//!
//! Procedurals are how most materials in this app are actually defined — the Materials Factory
//! compiles even a plain colour to a (degenerate) procedural so it can be driven by live uniforms.
//! Until now they existed *only* as GLSL, which meant the path tracer could not see them: an offline
//! render of an oak cabinet came out as one flat brown, because the tracer was handed the ramp's
//! midpoint colour and nothing else. The material preview had the same problem, for the same reason.
//!
//! So the evaluation lives here as well, and both consumers read it:
//!
//! - [`crate::pathtrace`] — so ⏺ Render shows the grain the viewport shows.
//! - [`crate::matball`] — so the Materials Factory can show a real sphere of the material.
//!
//! The two copies must agree exactly or a render will not match its viewport. The hash and the
//! value-noise lattice are therefore transcribed literally from the shader, `float` for `float`,
//! and `matches_the_shader_source` reads the GLSL back out of `light3d` to check the constants
//! still line up.

use crate::factory::{ProcDef, ProcPattern};
use glam::Vec3;

/// GLSL `fract` — the positive fractional part. Rust's `%` and `fract()` keep the sign, GLSL's does
/// not, and the difference silently changes the noise lattice for any negative coordinate — which
/// is most of a building, since plans are rarely all in the +x+y quadrant.
#[inline]
fn fract(x: f32) -> f32 {
    x - x.floor()
}

#[inline]
fn fract3(p: Vec3) -> Vec3 {
    Vec3::new(fract(p.x), fract(p.y), fract(p.z))
}

/// `vhash` from the shader.
#[inline]
fn vhash(p: Vec3) -> f32 {
    let mut p = fract3(p * 0.318_309_9 + 0.1);
    p *= 17.0;
    fract(p.x * p.y * p.z * (p.x + p.y + p.z))
}

/// `vnoise` — trilinear value noise on the integer lattice, smoothstepped.
fn vnoise(x: Vec3) -> f32 {
    let i = x.floor();
    let mut f = x - i;
    f = f * f * (Vec3::splat(3.0) - 2.0 * f);
    let h = |dx: f32, dy: f32, dz: f32| vhash(i + Vec3::new(dx, dy, dz));
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    lerp(
        lerp(
            lerp(h(0., 0., 0.), h(1., 0., 0.), f.x),
            lerp(h(0., 1., 0.), h(1., 1., 0.), f.x),
            f.y,
        ),
        lerp(
            lerp(h(0., 0., 1.), h(1., 0., 1.), f.x),
            lerp(h(0., 1., 1.), h(1., 1., 1.), f.x),
            f.y,
        ),
        f.z,
    )
}

/// `fbm` — octaves of value noise, amplitude falling by `rough` each time.
fn fbm(mut p: Vec3, detail: f32, rough: f32) -> f32 {
    let (mut a, mut s, mut norm) = (0.5f32, 0.0f32, 0.0f32);
    for i in 0..8 {
        if i as f32 >= detail {
            break;
        }
        s += a * vnoise(p);
        norm += a;
        p *= 2.0;
        a *= rough;
    }
    if norm > 0.0 {
        s / norm
    } else {
        s
    }
}

/// GLSL `smoothstep`.
#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Everything one point of a procedural surface is: its linear albedo, its roughness, and the
/// normal after the pattern's own relief has tilted it.
#[derive(Clone, Copy, Debug)]
pub struct ProcSample {
    pub albedo: [f32; 3],
    pub roughness: f32,
    pub normal: Vec3,
}

/// The raw pattern **field** at a world point, 0..1 — the shader's `proc_field`.
pub fn field(def: &ProcDef, wp: Vec3) -> f32 {
    let p = wp * Vec3::from(def.scale);
    match def.pattern {
        ProcPattern::Marble => 0.5 + 0.5 * ((p.x + p.y) * 0.6 + fbm(p, def.detail, def.rough) * std::f32::consts::TAU).sin(),
        ProcPattern::Checker => {
            let c = p.floor();
            (c.x + c.y + c.z).rem_euclid(2.0)
        }
        // Wood's anisotropy is entirely in `scale`; the field itself is plain fBm, as in the shader.
        ProcPattern::Wood | ProcPattern::Noise => fbm(p, def.detail, def.rough),
    }
}

/// The ramp position for a field value — the shader's `proc_ramp_t`.
pub fn ramp_t(def: &ProcDef, val: f32) -> f32 {
    let t = smoothstep(def.ramp[0], def.ramp[1], val);
    ((t - 0.5) * def.contrast + 0.5).clamp(0.0, 1.0)
}

/// Sample the whole material at a world point. `fallback_rough` is the material's scalar roughness,
/// used when the pattern does not vary its own finish.
pub fn sample(def: &ProcDef, wp: Vec3, n: Vec3, fallback_rough: f32) -> ProcSample {
    sample_with(def, wp, n, fallback_rough, true)
}

/// The same, without the RELIEF — colour and roughness only, leaving the normal alone.
///
/// [`bump`] costs three more [`field`] evaluations than everything else here put together (a
/// gradient by finite difference), so a caller that has to be fast and can live without the
/// surface's own tilt — an interactive preview mid-drag, say — pays a quarter of the price. The
/// pattern itself is identical; only the lighting response of its relief is missing.
pub fn sample_flat(def: &ProcDef, wp: Vec3, n: Vec3, fallback_rough: f32) -> ProcSample {
    sample_with(def, wp, n, fallback_rough, false)
}

fn sample_with(def: &ProcDef, wp: Vec3, n: Vec3, fallback_rough: f32, relief: bool) -> ProcSample {
    let f = field(def, wp);
    let t = ramp_t(def, f);
    // Ramp colours are authored sRGB — the same decode the shader does before lighting.
    let a = crate::color::srgb_to_linear3(def.col_a);
    let b = crate::color::srgb_to_linear3(def.col_b);
    let albedo = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    let roughness = if def.varies_roughness() {
        def.surf_rough[0] + (def.surf_rough[1] - def.surf_rough[0]) * t
    } else {
        fallback_rough
    }
    .clamp(0.03, 1.0);
    let normal = if relief { bump(def, wp, n, f) } else { n };
    ProcSample { albedo, roughness, normal }
}

/// Tilt `n` by the field's gradient — the shader's `proc_bump`, same step size and same strength.
pub fn bump(def: &ProcDef, wp: Vec3, n: Vec3, f0: f32) -> Vec3 {
    if def.bump <= 0.0 {
        return n;
    }
    let e = 0.35 / def.scale[0].max(def.scale[1]).max(def.scale[2]).max(1e-3);
    let mut g = Vec3::new(
        field(def, wp + Vec3::new(e, 0.0, 0.0)) - f0,
        field(def, wp + Vec3::new(0.0, e, 0.0)) - f0,
        field(def, wp + Vec3::new(0.0, 0.0, e)) - f0,
    );
    g -= n * n.dot(g);
    (n - g * (def.bump * 4.0)).normalize_or(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The noise must be bounded, deterministic and — the part that actually bites — **continuous
    /// across zero**. Rust's `fract` keeps the sign of its input and GLSL's does not; transcribing
    /// the hash without noticing puts a visible seam on every wall that crosses the origin, which
    /// is most of them, since a plan is drawn wherever the user drew it.
    #[test]
    fn the_noise_lattice_survives_negative_coordinates() {
        for i in 0..400 {
            let x = -2.0 + i as f32 * 0.01;
            let v = vnoise(Vec3::new(x, 0.37, -1.21));
            assert!((0.0..=1.0).contains(&v), "noise out of range at {x}: {v}");
        }
        // No discontinuity as the lattice cell index flips sign: consecutive samples either side of
        // an integer boundary must differ by no more than the general local variation.
        let step = |x: f32| vnoise(Vec3::new(x, 0.37, -1.21));
        let across = (step(-0.001) - step(0.001)).abs();
        let typical: f32 = (0..50).map(|i| (step(0.3 + i as f32 * 0.002) - step(0.3 + (i + 1) as f32 * 0.002)).abs()).fold(0.0, f32::max);
        assert!(across <= typical * 4.0 + 0.02, "seam at the origin: {across} vs typical {typical}");
    }

    /// Every pattern must produce a bounded field, or the ramp maps it somewhere meaningless.
    #[test]
    fn every_pattern_field_is_bounded() {
        for pattern in ProcPattern::ALL {
            let def = ProcDef { pattern, ..ProcDef::oak() };
            for i in 0..500 {
                let p = Vec3::new(i as f32 * 0.031 - 7.0, i as f32 * -0.017 + 3.0, i as f32 * 0.007);
                let f = field(&def, p);
                assert!(f.is_finite() && (-0.001..=1.001).contains(&f), "{pattern:?} at {p}: {f}");
            }
        }
    }

    /// A pattern that varies its finish must actually deliver different roughness at different
    /// points, and one that does not must hand back the material's own scalar untouched. This is
    /// the switch that decides which control the user is holding.
    #[test]
    fn roughness_comes_from_the_pattern_only_when_the_pattern_varies_it() {
        let oak = ProcDef::oak();
        assert!(oak.varies_roughness());
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in 0..600 {
            let p = Vec3::new(i as f32 * 0.013, i as f32 * 0.0071, 0.4);
            let r = sample(&oak, p, Vec3::Z, 0.99).roughness;
            lo = lo.min(r);
            hi = hi.max(r);
        }
        assert!(hi - lo > 0.1, "oak's finish must vary with its grain: {lo}..{hi}");
        assert!(lo >= 0.03 && hi <= 1.0, "and stay in range: {lo}..{hi}");
        // A flat colour does not vary anything, so the material's scalar wins.
        let flat = ProcDef::solid([0.5, 0.4, 0.3]);
        assert!(!flat.varies_roughness());
        assert!((sample(&flat, Vec3::ONE, Vec3::Z, 0.17).roughness - 0.17).abs() < 1e-6);
    }

    /// Albedo must come back LINEAR — the same decode the sampler does for an image texture. If it
    /// did not, a procedural and a bitmap of the same colour would render differently.
    #[test]
    fn albedo_is_linear() {
        let d = ProcDef::solid([0.5, 0.5, 0.5]);
        let s = sample(&d, Vec3::ZERO, Vec3::Z, 0.5);
        for c in s.albedo {
            assert!((c - crate::color::srgb_to_linear(0.5)).abs() < 1e-4, "{:?}", s.albedo);
        }
    }

    /// Bump tilts the normal, keeps it a unit vector, and never flips it through the surface — a
    /// normal that crosses the tangent plane makes a lit face go black.
    #[test]
    fn bump_tilts_without_flipping() {
        let oak = ProcDef { bump: 1.0, ..ProcDef::oak() };
        let mut max_tilt: f32 = 0.0;
        for i in 0..400 {
            let p = Vec3::new(i as f32 * 0.011, i as f32 * 0.006, 0.2);
            let n = sample(&oak, p, Vec3::Z, 0.5).normal;
            assert!((n.length() - 1.0).abs() < 1e-4, "not unit at {p}: {n}");
            assert!(n.z > 0.0, "bump flipped the normal through the surface at {p}: {n}");
            max_tilt = max_tilt.max(n.dot(Vec3::Z).clamp(-1.0, 1.0).acos().to_degrees());
        }
        assert!(max_tilt > 2.0, "bump should actually do something: max tilt {max_tilt}°");
        // …and zero bump is exactly the identity.
        let flat = ProcDef { bump: 0.0, ..ProcDef::oak() };
        assert_eq!(sample(&flat, Vec3::new(0.3, 0.2, 0.1), Vec3::Z, 0.5).normal, Vec3::Z);
    }

    /// The two implementations are separate texts. Pin the constants that would drift: the hash
    /// multipliers, the fBm octave cap, and the bump step — read back out of the shader source.
    #[test]
    fn matches_the_shader_source() {
        let glsl = crate::light3d::tex_fs_for_test();
        for c in ["0.3183099", "17.0", "i < 8", "0.35 /", "u_bump * 4.0", "3.0 - 2.0 * f"] {
            assert!(glsl.contains(c), "the shader no longer contains `{c}` — this copy has drifted");
        }
        // The ramp is the same shape in both: contrast about the midpoint after a smoothstep.
        assert!(glsl.contains("clamp((t - 0.5) * u_pcontrast + 0.5, 0.0, 1.0)"));
        let d = ProcDef { ramp: [0.2, 0.8], contrast: 2.0, ..ProcDef::oak() };
        assert!((ramp_t(&d, 0.5) - 0.5).abs() < 1e-6, "the midpoint is a fixed point of the contrast");
        assert_eq!(ramp_t(&d, 0.0), 0.0);
        assert_eq!(ramp_t(&d, 1.0), 1.0);
    }
}
