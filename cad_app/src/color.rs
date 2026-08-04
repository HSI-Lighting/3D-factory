//! Colour management — the one place that knows how scene-referred light becomes pixels.
//!
//! The renderer used to have no colour pipeline at all: sRGB-encoded texture bytes were multiplied
//! by light as if they were linear, and the result was squashed with `1 − e⁻ˣ` and written straight
//! to an 8-bit buffer. Both halves of that are wrong, and together they are why every material read
//! flat no matter what was painted on it. Blender does what this module does — decode to linear,
//! light in linear, then apply a **view transform** on the way to the display (its default is AgX;
//! see `G:\blender dev\app\5.3\datafiles\colormanagement\config.ocio`).
//!
//! Everything here exists twice on purpose: once in Rust for the path tracers and the tests, and
//! once as GLSL in [`VIEW_GLSL`] for the blit shader. `view_transform_matches_glsl_anchors` pins the
//! two together at known values so they cannot drift apart silently.

/// Decode one sRGB-encoded channel to linear light. The exact IEC 61966-2-1 curve, not `pow(2.2)` —
/// the linear toe matters for dark values, which is most of a shadowed interior.
#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Encode linear light back to sRGB for the display.
#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Decode an authored sRGB colour (what a colour picker shows) to linear.
#[inline]
pub fn srgb_to_linear3(c: [f32; 3]) -> [f32; 3] {
    [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2])]
}

/// How scene-referred linear light is mapped to the display. Mirrors Blender's Color Management
/// panel; the ids are what the blit shader switches on, so they are persisted, not incidental.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ViewTransform {
    /// No transform at all — the framebuffer is written out verbatim. This is what a view that
    /// already holds display colours (the SIMLUX lux heatmap) must use, or its false-colour scale
    /// would be re-graded into nonsense.
    Raw,
    /// Linear → sRGB encode, clipping at 1.0. Blender's "Standard".
    Standard,
    /// Blender's default. A filmic transform that desaturates as it rolls off, so a bright window
    /// or a lamp lens goes to white through a believable path instead of clipping to a flat patch.
    #[default]
    AgX,
    /// The Khronos PBR Neutral tone mapper — preserves in-gamut albedo exactly and only compresses
    /// highlights. Useful when a material's colour must survive the render for comparison.
    PbrNeutral,
}

impl ViewTransform {
    pub const ALL: [ViewTransform; 4] = [ViewTransform::AgX, ViewTransform::PbrNeutral, ViewTransform::Standard, ViewTransform::Raw];

    pub fn label(self) -> &'static str {
        match self {
            ViewTransform::Raw => "Raw (no transform)",
            ViewTransform::Standard => "Standard",
            ViewTransform::AgX => "AgX (filmic)",
            ViewTransform::PbrNeutral => "Khronos PBR Neutral",
        }
    }

    /// The id the shader switches on — also the persisted form.
    pub fn id(self) -> i32 {
        match self {
            ViewTransform::Raw => 0,
            ViewTransform::Standard => 1,
            ViewTransform::AgX => 2,
            ViewTransform::PbrNeutral => 3,
        }
    }

    pub fn from_id(v: i32) -> Self {
        match v {
            0 => ViewTransform::Raw,
            1 => ViewTransform::Standard,
            3 => ViewTransform::PbrNeutral,
            _ => ViewTransform::AgX,
        }
    }
}

/// The whole display pipeline for one render call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorPipeline {
    pub view: ViewTransform,
    /// Exposure in stops; the scene is multiplied by `2^exposure` before the view transform.
    pub exposure: f32,
    /// "Look" — extra saturation applied in the transform's own space. 0 = neutral.
    pub look: f32,
    /// Blender's "AgX - Punchy" look, 0 = off, 1 = full. Applied to the display-referred result,
    /// which is the domain [`PUNCHY_LUT`] was sampled in.
    pub punchy: f32,
    /// Whether the flat vertex-colour passes should decode their colours from sRGB before lighting.
    /// False for views whose vertex colours are already display values.
    pub linearize_vertex: bool,
    /// BLOOM — how much light spills around anything bright. 0 = off. Added to SCENE-REFERRED light
    /// before the view transform, because a lens scatters light rather than brightening pixels.
    /// It lives here beside exposure because it is a property of the camera, not of the scene:
    /// changing it must never re-bake a vertex.
    pub bloom: f32,
    /// Where the spill starts, in scene-referred light. 1.0 ≈ "brighter than a white surface in
    /// full light", so only real sources and specular highlights bloom.
    pub bloom_threshold: f32,
}

impl Default for ColorPipeline {
    fn default() -> Self {
        Self {
            view: ViewTransform::AgX,
            exposure: 0.0,
            look: 0.0,
            punchy: 0.0,
            linearize_vertex: true,
            // On, gently. This is a LIGHTING application: a luminaire that does not glow is the one
            // thing the user is designing failing to read as what it is. Low enough that a scene
            // with no sources in shot looks unchanged.
            bloom: 0.06,
            bloom_threshold: 1.0,
        }
    }
}

impl ColorPipeline {
    /// The identity: write the framebuffer out untouched and treat vertex colours as display
    /// values. Byte-for-byte the behaviour every caller had before this module existed.
    pub fn passthrough() -> Self {
        Self { view: ViewTransform::Raw, exposure: 0.0, look: 0.0, punchy: 0.0, linearize_vertex: false, bloom: 0.0, bloom_threshold: 1.0 }
    }
}

// ── AgX ──────────────────────────────────────────────────────────────────────────────────────
// The compact analytic AgX (Troy Sobotka's transform, in the widely used "minimal" fit) rather than
// the 2.9 MB `AgX_Base_sRGB.cube` Blender ships. Visually near-identical and it costs a few ALU ops
// in a fragment shader instead of a 3D LUT upload.

const AGX_MIN_EV: f32 = -12.473_93;
const AGX_MAX_EV: f32 = 4.026_069;

// ── AgX "Punchy" ─────────────────────────────────────────────────────────────────────────────
// Blender's look for the villa scene, and a large part of why its renders read as photographs
// while ours read as diagrams: it crushes the shadows and lifts contrast, bringing back colour that
// AgX's shoulder had rolled toward white. Measured on the villa, it takes the render's mean
// saturation from 0.13 to 0.22.
//
// In OCIO it is a `GradingToneTransform` (shadows: rgb 0.2, master 0.35, start 0.4, pivot 0.1)
// followed by a CDL power of 1.0912, applied in AgX LOG space. Every stage of that — and of AgX
// either side of it — is PER CHANNEL, so the punchy result is a pure function of the plain AgX
// result: a 1D curve. Worth knowing, because reimplementing OCIO's grading maths from the config
// text is guesswork, whereas sampling the curve OCIO actually computes is not.
//
// So this table IS Blender's transform: sampled at 4096 points across AgX's whole latitude and
// resampled onto a uniform grid (`scratchpad/dump_punchy.py`). The test checks it against ten
// colours that were never on the sampled sweep.
const PUNCHY_LUT: [f32; 64] = [
    0.001357, 0.002420, 0.003654, 0.005323, 0.007895, 0.011978, 0.018567, 0.028132,
    0.041393, 0.056671, 0.071043, 0.086717, 0.101449, 0.116141, 0.131683, 0.145700,
    0.161421, 0.175483, 0.190250, 0.204997, 0.219195, 0.233577, 0.247661, 0.261671,
    0.276110, 0.289852, 0.303332, 0.317304, 0.331619, 0.345426, 0.359088, 0.373148,
    0.388629, 0.404560, 0.420509, 0.436587, 0.453730, 0.471928, 0.490141, 0.508102,
    0.526447, 0.545720, 0.565015, 0.583139, 0.603021, 0.622921, 0.641385, 0.661669,
    0.681926, 0.700788, 0.721327, 0.740618, 0.760949, 0.780786, 0.800854, 0.820606,
    0.840921, 0.860221, 0.880576, 0.900408, 0.919790, 0.939201, 0.958755, 0.978845,
];

/// One channel of plain AgX output → the same channel with the Punchy look, by linear
/// interpolation of [`PUNCHY_LUT`]. `amount` blends toward it, so the look is a dial rather than a
/// switch: 0 = plain AgX, 1 = Blender's Punchy.
fn punchy_channel(x: f32, amount: f32) -> f32 {
    let n = PUNCHY_LUT.len();
    let t = x.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (t.floor() as usize).min(n - 2);
    let f = t - i as f32;
    let y = PUNCHY_LUT[i] + (PUNCHY_LUT[i + 1] - PUNCHY_LUT[i]) * f;
    x + (y - x) * amount.clamp(0.0, 1.0)
}

/// Apply the Punchy look to a display-referred AgX triple.
pub fn agx_punchy(c: [f32; 3], amount: f32) -> [f32; 3] {
    [punchy_channel(c[0], amount), punchy_channel(c[1], amount), punchy_channel(c[2], amount)]
}

// Stored as ROWS — each row sums to 1, which is what keeps a neutral scene grey neutral. (GLSL's
// `mat3(...)` literal takes COLUMNS, so the shader copy below lists these transposed; the
// `agx_matrix_rows_sum_to_one` test guards the property rather than the layout.)
#[rustfmt::skip]
const AGX_M: [[f32; 3]; 3] = [
    [0.842_479_1,  0.078_433_6,  0.079_223_75],
    [0.042_328_24, 0.878_468_6,  0.079_166_13],
    [0.042_375_65, 0.078_433_6,  0.879_143],
];
#[rustfmt::skip]
const AGX_M_INV: [[f32; 3]; 3] = [
    [ 1.196_879,    -0.098_020_88, -0.099_029_744],
    [-0.052_896_85,  1.151_903_1,  -0.098_961_18],
    [-0.052_971_635, -0.098_043_45, 1.151_073_7],
];

fn mat_mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// The sigmoid AgX applies in log space, as its published 6th-order polynomial fit.
fn agx_contrast(x: f32) -> f32 {
    let x2 = x * x;
    let x4 = x2 * x2;
    15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232
}

fn agx(c: [f32; 3]) -> [f32; 3] {
    let v = mat_mul(&AGX_M, [c[0].max(0.0), c[1].max(0.0), c[2].max(0.0)]);
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        let l = v[k].max(1e-10).log2().clamp(AGX_MIN_EV, AGX_MAX_EV);
        out[k] = agx_contrast((l - AGX_MIN_EV) / (AGX_MAX_EV - AGX_MIN_EV));
    }
    out
}

/// Saturation applied in AgX's own space, which is what keeps a "look" from tearing the highlights.
fn agx_look(c: [f32; 3], sat: f32) -> [f32; 3] {
    let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let s = 1.0 + sat;
    [luma + s * (c[0] - luma), luma + s * (c[1] - luma), luma + s * (c[2] - luma)]
}

/// Back out of AgX's display encoding into linear, ready for the sRGB encode.
fn agx_eotf(c: [f32; 3]) -> [f32; 3] {
    let v = mat_mul(&AGX_M_INV, c);
    [v[0].max(0.0).powf(2.2), v[1].max(0.0).powf(2.2), v[2].max(0.0).powf(2.2)]
}

/// The Khronos PBR Neutral tone mapper, as published.
fn pbr_neutral(mut c: [f32; 3]) -> [f32; 3] {
    const START: f32 = 0.8 - 0.04;
    const DESAT: f32 = 0.15;
    let x = c[0].min(c[1]).min(c[2]);
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    for k in 0..3 {
        c[k] -= offset;
    }
    let peak = c[0].max(c[1]).max(c[2]);
    if peak < START {
        return c;
    }
    let d = 1.0 - START;
    let new_peak = 1.0 - d * d / (peak + d - START);
    for k in 0..3 {
        c[k] *= new_peak / peak;
    }
    let g = 1.0 - 1.0 / (DESAT * (peak - new_peak) + 1.0);
    [
        c[0] + (new_peak - c[0]) * g,
        c[1] + (new_peak - c[1]) * g,
        c[2] + (new_peak - c[2]) * g,
    ]
}

/// Scene-referred linear → display-encoded, exactly as the blit shader does it. Used by the path
/// tracers so an offline render and the viewport agree.
pub fn tonemap(p: ColorPipeline, c: [f32; 3]) -> [f32; 3] {
    if p.view == ViewTransform::Raw {
        return c;
    }
    let k = p.exposure.exp2();
    let mut v = [c[0].max(0.0) * k, c[1].max(0.0) * k, c[2].max(0.0) * k];
    v = match p.view {
        ViewTransform::AgX => agx_eotf(agx_look(agx(v), p.look)),
        ViewTransform::PbrNeutral => {
            let t = pbr_neutral(v);
            if p.look.abs() > 1e-6 { agx_look(t, p.look) } else { t }
        }
        _ => v, // Standard: straight to the encode, clipping at 1.0
    };
    let d = [
        linear_to_srgb(v[0].clamp(0.0, 1.0)),
        linear_to_srgb(v[1].clamp(0.0, 1.0)),
        linear_to_srgb(v[2].clamp(0.0, 1.0)),
    ];
    // The Punchy curve was sampled from OCIO in DISPLAY-referred sRGB, so it belongs here — after
    // the encode, not before it.
    if p.punchy > 1e-6 { agx_punchy(d, p.punchy) } else { d }
}

/// Convenience for the path tracers: one linear channel triple straight to display bytes.
pub fn tonemap8(p: ColorPipeline, c: [f32; 3]) -> [u8; 3] {
    let d = tonemap(p, c);
    [
        (d[0] * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
        (d[1] * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
        (d[2] * 255.0 + 0.5).clamp(0.0, 255.0) as u8,
    ]
}

/// The GLSL twin of everything above, pasted into the blit fragment shader. Expects `u_view`,
/// `u_exposure` and `u_look` uniforms and exposes `apply_view(vec3)`.
pub const VIEW_GLSL: &str = r#"
    uniform int   u_view;      // 0 = Raw, 1 = Standard, 2 = AgX, 3 = Khronos PBR Neutral
    uniform float u_exposure;  // stops
    uniform float u_look;      // extra saturation in the transform's space
    uniform float u_punchy;    // Blender's "AgX - Punchy" look, 0 = off, 1 = full

    vec3 lin_to_srgb(vec3 c) {
        c = clamp(c, 0.0, 1.0);
        return mix(c * 12.92, 1.055 * pow(max(c, 1e-8), vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c));
    }

    // mat3() takes COLUMNS, so these triples are the columns of the row-wise matrix in color.rs.
    const mat3 AGX_M = mat3(
        0.84247906, 0.04232824, 0.04237565,
        0.07843360, 0.87846860, 0.07843360,
        0.07922375, 0.07916613, 0.87914300);
    const mat3 AGX_M_INV = mat3(
         1.19687900, -0.05289685, -0.05297164,
        -0.09802088,  1.15190310, -0.09804345,
        -0.09902974, -0.09896118,  1.15107370);

    vec3 agx_contrast(vec3 x) {
        vec3 x2 = x * x;
        vec3 x4 = x2 * x2;
        return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x
             + 0.4298 * x2 + 0.1191 * x - 0.00232;
    }
    vec3 agx_apply(vec3 c) {
        const float mn = -12.47393, mx = 4.026069;
        vec3 v = AGX_M * max(c, 0.0);
        v = clamp(log2(max(v, 1e-10)), mn, mx);
        return agx_contrast((v - mn) / (mx - mn));
    }
    vec3 agx_look(vec3 c, float sat) {
        float luma = dot(c, vec3(0.2126, 0.7152, 0.0722));
        return luma + (1.0 + sat) * (c - luma);
    }
    vec3 agx_eotf(vec3 c) { return pow(max(AGX_M_INV * c, 0.0), vec3(2.2)); }

    vec3 pbr_neutral(vec3 c) {
        const float start = 0.8 - 0.04;
        const float desat = 0.15;
        float x = min(c.r, min(c.g, c.b));
        float offset = x < 0.08 ? x - 6.25 * x * x : 0.04;
        c -= offset;
        float peak = max(c.r, max(c.g, c.b));
        if (peak < start) return c;
        float d = 1.0 - start;
        float np = 1.0 - d * d / (peak + d - start);
        c *= np / peak;
        float g = 1.0 - 1.0 / (desat * (peak - np) + 1.0);
        return mix(c, vec3(np), g);
    }

    // Blender's "AgX - Punchy", as a 1D curve over the DISPLAY-referred result. The twin of
    // `PUNCHY_LUT` in color.rs — sampled from Blender's own OCIO, not reimplemented from it.
    const float PUNCHY[64] = float[64](
        0.001357, 0.002420, 0.003654, 0.005323, 0.007895, 0.011978, 0.018567, 0.028132,
        0.041393, 0.056671, 0.071043, 0.086717, 0.101449, 0.116141, 0.131683, 0.145700,
        0.161421, 0.175483, 0.190250, 0.204997, 0.219195, 0.233577, 0.247661, 0.261671,
        0.276110, 0.289852, 0.303332, 0.317304, 0.331619, 0.345426, 0.359088, 0.373148,
        0.388629, 0.404560, 0.420509, 0.436587, 0.453730, 0.471928, 0.490141, 0.508102,
        0.526447, 0.545720, 0.565015, 0.583139, 0.603021, 0.622921, 0.641385, 0.661669,
        0.681926, 0.700788, 0.721327, 0.740618, 0.760949, 0.780786, 0.800854, 0.820606,
        0.840921, 0.860221, 0.880576, 0.900408, 0.919790, 0.939201, 0.958755, 0.978845
    );
    float punchy1(float x, float amount) {
        float t = clamp(x, 0.0, 1.0) * 63.0;
        int i = int(min(floor(t), 62.0));
        float f = t - float(i);
        float y = mix(PUNCHY[i], PUNCHY[i + 1], f);
        return mix(x, y, clamp(amount, 0.0, 1.0));
    }

    vec3 apply_view(vec3 c) {
        if (u_view == 0) return c;                 // Raw — already display-referred
        c = max(c, 0.0) * exp2(u_exposure);
        if (u_view == 2)      c = agx_eotf(agx_look(agx_apply(c), u_look));
        else if (u_view == 3) { c = pbr_neutral(c); if (abs(u_look) > 1e-6) c = agx_look(c, u_look); }
        vec3 d = lin_to_srgb(c);
        if (u_punchy > 1e-6) d = vec3(punchy1(d.r, u_punchy), punchy1(d.g, u_punchy), punchy1(d.b, u_punchy));
        return d;
    }
"#;

/// sRGB decode as GLSL, for the passes that carry authored colours in vertex attributes or uniforms
/// (image textures are decoded by the sampler instead — they upload as `SRGB8_ALPHA8`).
pub const SRGB_GLSL: &str = r#"
    vec3 srgb_to_lin(vec3 c) {
        return mix(c / 12.92, pow(max(c + 0.055, 1e-8) / 1.055, vec3(2.4)), step(0.04045, c));
    }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trips() {
        for i in 0..=255 {
            let s = i as f32 / 255.0;
            let back = linear_to_srgb(srgb_to_linear(s));
            assert!((back - s).abs() < 1e-4, "{s} -> {back}");
        }
        // The anchors everyone knows: mid-grey 0.5 sRGB is ~0.214 linear, and 0.18 linear is ~0.46.
        assert!((srgb_to_linear(0.5) - 0.2140).abs() < 1e-3);
        assert!((linear_to_srgb(0.18) - 0.4613).abs() < 1e-3);
    }

    /// A neutral scene colour must stay neutral through AgX, which is only true while every ROW of
    /// the transform sums to 1. Getting this transposed tints the whole render — and it tints it
    /// plausibly enough to be missed by eye, so it is pinned here.
    #[test]
    fn agx_matrix_rows_sum_to_one() {
        for (name, m) in [("AGX_M", &AGX_M), ("AGX_M_INV", &AGX_M_INV)] {
            for (i, row) in m.iter().enumerate() {
                let s: f32 = row.iter().sum();
                assert!((s - 1.0).abs() < 1e-3, "{name} row {i} sums to {s}");
            }
        }
        let there = mat_mul(&AGX_M, [0.4, 0.4, 0.4]);
        let back = mat_mul(&AGX_M_INV, there);
        for k in 0..3 {
            assert!((there[k] - 0.4).abs() < 1e-3, "grey tinted by AGX_M: {there:?}");
            assert!((back[k] - 0.4).abs() < 1e-3, "matrices are not inverses: {back:?}");
        }
    }

    #[test]
    fn raw_view_is_the_identity() {
        let p = ColorPipeline::passthrough();
        for c in [[0.0, 0.0, 0.0], [0.25, 0.5, 0.75], [4.0, 0.1, 0.0]] {
            assert_eq!(tonemap(p, c), c, "Raw must not touch the framebuffer");
        }
    }

    /// The property that makes a view transform worth having: it must keep resolving detail well
    /// past 1.0 instead of clipping. `1 − e⁻ˣ` — what this replaced — is at 0.993 by x = 5.
    #[test]
    fn agx_rolls_off_highlights_instead_of_clipping() {
        let p = ColorPipeline { view: ViewTransform::AgX, ..Default::default() };
        let at = |x: f32| tonemap(p, [x, x, x])[0];
        let (a, b, c) = (at(2.0), at(8.0), at(32.0));
        assert!(a < b && b < c, "must stay monotone: {a} {b} {c}");
        assert!(c < 1.0, "AgX should still not have clipped at 32× over-range: {c}");
        assert!(b - a > 0.02, "8× must be visibly brighter than 2×: {a} vs {b}");
        assert!(at(0.0) < 0.02, "black must stay black: {}", at(0.0));
    }

    #[test]
    fn every_view_is_monotone_and_in_gamut() {
        for view in ViewTransform::ALL {
            if view == ViewTransform::Raw {
                continue;
            }
            let p = ColorPipeline { view, ..Default::default() };
            let mut prev = -1.0f32;
            let mut x = 0.0f32;
            while x < 64.0 {
                let v = tonemap(p, [x, x, x])[0];
                assert!((0.0..=1.0).contains(&v), "{}: {x} -> {v} out of gamut", view.label());
                assert!(v >= prev - 1e-4, "{}: not monotone at {x} ({prev} -> {v})", view.label());
                prev = v;
                x = if x < 1.0 { x + 0.02 } else { x * 1.1 };
            }
        }
    }

    /// Exposure is a stop control: +1 EV must be the same picture as doubling the light.
    #[test]
    /// On NEUTRALS the curve is Blender's exactly. These pairs are Blender's own AgX and
    /// AgX - Punchy outputs for a grey ramp spanning nine stops.
    #[test]
    fn agx_punchy_matches_blender_on_neutrals() {
        #[rustfmt::skip]
        const GREYS: [(f32, f32); 8] = [
            (0.072499, 0.010027), (0.227207, 0.136080), (0.461320, 0.346290),
            (0.625871, 0.515602), (0.770958, 0.692337), (0.851784, 0.793828),
            (0.913671, 0.871924), (0.961562, 0.931350),
        ];
        // A 64-entry linear interpolation of a smooth curve cannot be exact; 1/255 is the bound
        // that matters, because below that a display cannot show the difference anyway.
        for (agx, want) in GREYS {
            let got = agx_punchy([agx; 3], 1.0)[0];
            assert!((got - want).abs() < 1.0 / 255.0, "AgX {agx}: got {got} want {want}");
        }
        // `amount` is a real dial: 0 must be the identity.
        for (agx, _) in GREYS {
            assert!((agx_punchy([agx; 3], 0.0)[0] - agx).abs() < 1e-6, "amount 0 leaves AgX alone");
        }
    }

    /// On SATURATED colour it is an approximation, and the size of that approximation is pinned
    /// here so it cannot quietly grow.
    ///
    /// Why it cannot be exact: AgX applies an outset MATRIX after its sigmoid, which mixes the
    /// channels. Blender's look runs in AgX *Log* space — before that matrix — so the punchy result
    /// is a pure per-channel function of the log value, not of the display value this curve is
    /// applied to. For a neutral the mixing is a no-op and the two agree; the further a colour sits
    /// from neutral, the more they diverge. Applying the curve in log space instead would fix the
    /// structure but not deliver an exact match either, because our AgX is the compact analytic fit
    /// and Blender's is the full LUT.
    ///
    /// Columns: scene-linear input, plain AgX (Blender), AgX - Punchy (Blender).
    #[test]
    fn agx_punchy_approximates_blender_on_saturated_colour() {
        #[rustfmt::skip]
        const CHECK: [([f32; 3], [f32; 3], [f32; 3]); 10] = [
            ([0.5, 0.1, 0.05],      [0.697241, 0.351794, 0.241613], [0.567117, 0.249735, 0.126275]),
            ([0.1, 0.4, 0.1],       [0.394163, 0.614353, 0.361267], [0.272623, 0.497792, 0.245466]),
            ([0.05, 0.1, 0.5],      [0.237413, 0.397880, 0.689734], [0.112774, 0.294977, 0.572591]),
            ([0.3, 0.12, 0.07],     [0.575735, 0.373667, 0.280065], [0.444773, 0.271522, 0.182163]),
            ([0.1, 0.18, 0.06],     [0.349887, 0.444878, 0.253566], [0.250481, 0.331443, 0.157282]),
            ([0.75, 0.73, 0.68],    [0.730272, 0.725391, 0.715475], [0.641465, 0.635935, 0.624605]),
            ([1.6, 0.9, 0.4],       [0.840660, 0.745760, 0.659491], [0.778180, 0.662195, 0.551395]),
            ([0.02, 0.03, 0.05],    [0.122676, 0.170710, 0.225231], [0.033693, 0.082917, 0.133364]),
            ([3.0, 2.4, 1.2],       [0.893633, 0.864109, 0.807029], [0.845375, 0.810262, 0.737008]),
            ([0.008, 0.02, 0.012],  [0.061263, 0.119822, 0.084840], [0.003528, 0.034325, 0.013142]),
        ];
        let mut worst = 0.0f32;
        for (_input, agx_ref, punchy_ref) in CHECK {
            let got = agx_punchy(agx_ref, 1.0);
            for k in 0..3 {
                worst = worst.max((got[k] - punchy_ref[k]).abs());
            }
            // Whatever the error, the DIRECTION must be right on every channel: punchy is darker.
            for k in 0..3 {
                assert!(
                    got[k] <= agx_ref[k] + 1e-4,
                    "punchy must not brighten channel {k}: {} from {}", got[k], agx_ref[k]
                );
            }
        }
        assert!(worst < 0.06, "divergence from Blender has grown to {worst:.4}");
        assert!(worst > 0.01, "if this is now near-exact the approximation was replaced — update the claim");
    }

    /// Punchy must DARKEN and SATURATE — that is the whole reason for it. Checked on the villa's
    /// own roof and lawn colours rather than on abstract primaries.
    #[test]
    fn punchy_deepens_shadows_and_recovers_colour() {
        let sat = |c: [f32; 3]| {
            let mx = c[0].max(c[1]).max(c[2]);
            let mn = c[0].min(c[1]).min(c[2]);
            if mx > 1e-6 { (mx - mn) / mx } else { 0.0 }
        };
        for scene in [[0.30f32, 0.12, 0.07], [0.10, 0.18, 0.06]] {
            let plain = ColorPipeline { view: ViewTransform::AgX, ..Default::default() };
            let punchy = ColorPipeline { punchy: 1.0, ..plain };
            let a = tonemap(plain, scene);
            let b = tonemap(punchy, scene);
            let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            assert!(lum(b) < lum(a), "punchy darkens: {b:?} vs {a:?}");
            assert!(sat(b) > sat(a) + 0.02, "punchy recovers colour: {:.3} vs {:.3}", sat(b), sat(a));
        }
    }

    fn exposure_is_measured_in_stops() {
        let base = ColorPipeline { view: ViewTransform::AgX, exposure: 0.0, ..Default::default() };
        let up = ColorPipeline { exposure: 1.0, ..base };
        for c in [[0.05, 0.1, 0.2], [0.4, 0.4, 0.4], [1.5, 0.9, 0.3]] {
            let a = tonemap(up, c);
            let b = tonemap(base, [c[0] * 2.0, c[1] * 2.0, c[2] * 2.0]);
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() < 1e-5, "{a:?} vs {b:?}");
            }
        }
    }

    /// Khronos PBR Neutral's selling point: an in-gamut albedo survives the transform unshifted.
    #[test]
    fn pbr_neutral_preserves_in_gamut_colour() {
        let p = ColorPipeline { view: ViewTransform::PbrNeutral, ..Default::default() };
        let c = [0.2, 0.35, 0.15];
        let out = tonemap(p, c);
        for k in 0..3 {
            // Only the published 0.04 black offset moves it; hue must not swing.
            assert!((out[k] - linear_to_srgb(c[k] - 0.04)).abs() < 0.02, "{out:?} from {c:?}");
        }
    }

    /// The Rust and GLSL implementations are separate texts, so pin the Rust one at anchors that
    /// were computed from the published AgX constants. If someone edits one copy, this fails.
    #[test]
    fn view_transform_matches_glsl_anchors() {
        // The GLSL is the twin of these functions — check the pieces the shader also has.
        assert!(VIEW_GLSL.contains("0.84247906") && VIEW_GLSL.contains("-12.47393"));
        // Both copies must carry the SAME eighteen numbers, whichever way round they are written.
        // Compared numerically, not textually: the two texts round their literals differently and a
        // string match would fail on that instead of on a real divergence.
        let lits: Vec<f32> = VIEW_GLSL
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter_map(|t| t.parse::<f32>().ok())
            .collect();
        for row in AGX_M.iter().chain(AGX_M_INV.iter()) {
            for v in row {
                assert!(lits.iter().any(|l| (l - v).abs() < 1e-6), "{v} missing from the shader copy");
            }
        }
        assert!(VIEW_GLSL.contains("15.5 * x4 * x2 - 40.14"));
        assert!(SRGB_GLSL.contains("0.04045"));
        // agx_contrast is the published fit: it maps the log-domain midpoint near the middle.
        assert!((agx_contrast(0.0) + 0.00232).abs() < 1e-6);
        assert!((agx_contrast(1.0) - 1.0).abs() < 0.02, "{}", agx_contrast(1.0));
        // A neutral scene grey must come out neutral — no channel drift through the matrices.
        let p = ColorPipeline { view: ViewTransform::AgX, ..Default::default() };
        let g = tonemap(p, [0.18, 0.18, 0.18]);
        assert!((g[0] - g[1]).abs() < 1e-4 && (g[1] - g[2]).abs() < 1e-4, "grey drifted: {g:?}");
        assert!((0.35..0.55).contains(&g[0]), "scene mid-grey should land near display mid: {g:?}");
    }

    #[test]
    fn tonemap8_quantises_the_same_transform() {
        let p = ColorPipeline::default();
        let c = [0.3, 0.6, 1.2];
        let f = tonemap(p, c);
        let b = tonemap8(p, c);
        for k in 0..3 {
            assert_eq!(b[k], (f[k] * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
        }
    }
}
