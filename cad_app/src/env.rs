//! Environment lighting — the sky as an actual light source.
//!
//! Phase 1 fixed how light becomes pixels. This fixes what the light *is*. Before this module the
//! ambient term was two colours lerped by `normal.z`: a flat cool value above, a flat warm one
//! below. That is why every surface facing away from the sun read as one dead tone — there was no
//! sky, only a fill, so nothing could vary across a wall and nothing could be reflected.
//!
//! Here the sky is the **Preetham** analytic daylight model (Preetham, Shirley & Smits 1999, the
//! model `gensky`-style tools and most renderers' "physical sky" descend from). One closed-form
//! function gives the radiance arriving from any direction, so the same sky can be:
//!
//! - **drawn** as the background,
//! - **integrated** into 9 spherical-harmonic coefficients for the diffuse ambient — a real
//!   irradiance field instead of a lerp, so a north wall and a south wall differ,
//! - **mirrored** by a glossy surface, which is what finally gives metal something to reflect.
//!
//! ## What is physical here and what is calibrated
//!
//! The *shape* and *colour* of the sky are the published model. The *absolute level* is not: it is
//! scaled so the irradiance on an up-facing surface equals what [`crate::factory::SunEnv::resolve`]
//! already produced. That deliberately preserves the daylight calibration that was tuned against
//! Blender — this change alters how light is distributed over the sky, not how much of it there is.
//! (A consequence: Preetham's zenith luminance `Yz` cancels out of every result, so it is not
//! computed. Only the zenith *chromaticity* survives, and it is what makes a hazy sky go white.)
//!
//! Like [`crate::color`], everything exists twice — Rust for the path tracer, SH baking and tests,
//! GLSL in [`SKY_GLSL`] for the shaders. The Perez coefficients are computed **once in Rust** and
//! passed to the shader as uniforms, so the turbidity fits below cannot drift out of sync; only the
//! ~15 lines of the Perez formula itself are duplicated, and `sky_glsl_matches_rust` pins them.

use glam::Vec3;

// ── the Perez / Preetham sky ─────────────────────────────────────────────────────────────────

/// The five Perez coefficients `A..E` for each of the three Yxy channels, i.e. `perez[coef][chan]`.
/// Fitted to turbidity by Preetham's published tables. `chan` is 0 = luminance, 1 = x, 2 = y.
///
/// Preetham's table, verbatim (`A..E` down, `Y/x/y` across):
/// ```text
///     Y                      x                       y
/// A   0.1787·T − 1.4630     −0.0193·T − 0.2592      −0.0167·T − 0.2608
/// B  −0.3554·T + 0.4275     −0.0665·T + 0.0008      −0.0950·T + 0.0092
/// C  −0.0227·T + 5.3251     −0.0004·T + 0.2125      −0.0079·T + 0.2102
/// D   0.1206·T − 2.5771     −0.0641·T − 0.8989      −0.0441·T − 1.6537
/// E  −0.0670·T + 0.3703     −0.0033·T + 0.0452      −0.0109·T + 0.0529
/// ```
#[rustfmt::skip]
fn perez_coefficients(t: f32) -> [[f32; 3]; 5] {
    [
        [ 0.1787 * t - 1.4630, -0.0193 * t - 0.2592, -0.0167 * t - 0.2608],
        [-0.3554 * t + 0.4275, -0.0665 * t + 0.0008, -0.0950 * t + 0.0092],
        [-0.0227 * t + 5.3251, -0.0004 * t + 0.2125, -0.0079 * t + 0.2102],
        [ 0.1206 * t - 2.5771, -0.0641 * t - 0.8989, -0.0441 * t - 1.6537],
        [-0.0670 * t + 0.3703, -0.0033 * t + 0.0452, -0.0109 * t + 0.0529],
    ]
}

/// Preetham's zenith **chromaticity** for turbidity `t` and solar zenith angle `ts` (radians).
/// Two cubics in `ts` weighted by `T²`, `T` and `1` — this is what turns a clear blue zenith white
/// as haze rises, and it is the only part of the zenith fit that survives the normalisation.
#[rustfmt::skip]
fn zenith_chroma(t: f32, ts: f32) -> [f32; 2] {
    let (t2, ts2, ts3) = (t * t, ts * ts, ts * ts * ts);
    let x = t2 * ( 0.00166 * ts3 - 0.00375 * ts2 + 0.00209 * ts)
          + t  * (-0.02903 * ts3 + 0.06377 * ts2 - 0.03202 * ts + 0.00394)
          +      ( 0.11693 * ts3 - 0.21196 * ts2 + 0.06052 * ts + 0.25886);
    let y = t2 * ( 0.00275 * ts3 - 0.00610 * ts2 + 0.00317 * ts)
          + t  * (-0.04214 * ts3 + 0.08970 * ts2 - 0.04153 * ts + 0.00516)
          +      ( 0.15346 * ts3 - 0.26756 * ts2 + 0.06670 * ts + 0.26688);
    [x, y]
}

/// CIE XYZ → **linear** sRGB (Rec. 709 primaries, D65). Stored as ROWS, like the AgX matrices in
/// [`crate::color`], and for the same reason: `mat3()` in GLSL takes columns, so the shader copy is
/// written transposed.
#[rustfmt::skip]
const XYZ_TO_RGB: [[f32; 3]; 3] = [
    [ 3.2406, -1.5372, -0.4986],
    [-0.9689,  1.8758,  0.0415],
    [ 0.0557, -0.2040,  1.0570],
];

/// CIE Yxy → linear sRGB. Negative components are clamped: the sky's chromaticity can stray just
/// outside the sRGB gamut near the horizon, and a negative channel would darken a *neighbouring*
/// one once it is multiplied into an albedo.
fn yxy_to_rgb(yy: f32, x: f32, y: f32) -> [f32; 3] {
    if y <= 1e-5 {
        return [0.0; 3];
    }
    let xyz = [x / y * yy, yy, (1.0 - x - y) / y * yy];
    let mut out = [0.0f32; 3];
    for (k, o) in out.iter_mut().enumerate() {
        *o = (XYZ_TO_RGB[k][0] * xyz[0] + XYZ_TO_RGB[k][1] * xyz[1] + XYZ_TO_RGB[k][2] * xyz[2]).max(0.0);
    }
    out
}

/// A resolved sky: everything the shaders, the SH bake and the path tracer need, with no turbidity
/// fits left to redo. Built by [`Sky::new`] and then normalised by [`Sky::calibrate`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sky {
    /// Unit direction **to** the sun (world axes, Z up).
    pub sun_dir: Vec3,
    /// Perez `A..E` per Yxy channel — see [`perez_coefficients`].
    pub perez: [[f32; 3]; 5],
    /// Zenith chromaticity `(x, y)`. Zenith luminance is 1 by construction (see the module note).
    pub zenith_xy: [f32; 2],
    /// `F(0, θs)` per channel — the normalisation divisor, cached because [`Self::sh9`] evaluates
    /// the model a thousand times and would otherwise recompute it a thousand times.
    norm: [f32; 3],
    /// Overall linear scale applied after the model, set by [`Sky::calibrate`].
    pub scale: f32,
    /// Radiance of the lower hemisphere — the ground, treated as a uniform diffuse reflector.
    pub ground: [f32; 3],
    /// Radiance of the sun's own disc, for the background and for mirror reflections.
    pub sun_col: [f32; 3],
    /// `false` ⇒ the sun is below the horizon and the model is meaningless; callers fall back to a
    /// flat ambient rather than dividing by a normalisation that has collapsed.
    pub valid: bool,
}

impl Default for Sky {
    fn default() -> Self {
        Sky::new(Vec3::new(0.0, 0.0, 1.0), 2.5)
    }
}

/// Clear, believable daylight — the turbidity default. 2 is an exceptionally clear alpine day, 6 is
/// hazy summer city air, 10+ is fog.
pub const DEFAULT_TURBIDITY: f32 = 2.8;

impl Sky {
    /// Build the sky for a sun direction and turbidity. The result still has `scale = 1`; call
    /// [`Self::calibrate`] to put it on the scene's own light level.
    pub fn new(sun_dir: Vec3, turbidity: f32) -> Self {
        let sun_dir = sun_dir.normalize_or(Vec3::Z);
        let t = turbidity.clamp(1.7, 12.0);
        // Solar ZENITH angle: 0 = overhead. Clamped just off the horizon — the Perez normalisation
        // divides by F(0, ts), which stays finite, but the model's colours become nonsense below it.
        let ts = sun_dir.z.clamp(-1.0, 1.0).acos().min(std::f32::consts::FRAC_PI_2 - 0.02);
        let mut s = Self {
            sun_dir,
            perez: perez_coefficients(t),
            zenith_xy: zenith_chroma(t, ts),
            norm: [1.0; 3],
            scale: 1.0,
            ground: [0.0; 3],
            sun_col: [0.0; 3],
            valid: sun_dir.z > 0.0,
        };
        let theta_s = sun_dir.z.clamp(-1.0, 1.0).acos();
        for ch in 0..3 {
            s.norm[ch] = s.perez_f(ch, 1.0, theta_s).max(1e-6);
        }
        s
    }

    /// The Perez function `F(θ, γ) = (1 + A·e^(B/cos θ))·(1 + C·e^(D·γ) + E·cos²γ)` for channel
    /// `ch`, where `cos_theta` is measured from the zenith and `gamma` is the angle to the sun.
    fn perez_f(&self, ch: usize, cos_theta: f32, gamma: f32) -> f32 {
        let [a, b, c, d, e] = [self.perez[0][ch], self.perez[1][ch], self.perez[2][ch], self.perez[3][ch], self.perez[4][ch]];
        // cos θ is floored, not clamped to 0: `e^(B/cos θ)` with B < 0 tends to 1 as cos θ → 0⁺, but
        // in floating point it is a division by zero on the way there.
        let ct = cos_theta.max(0.01);
        let cg = gamma.cos();
        (1.0 + a * (b / ct).exp()) * (1.0 + c * (d * gamma).exp() + e * cg * cg)
    }

    /// Sky radiance arriving from `dir` (unit, world axes), **without** the sun's disc — the
    /// continuous dome only. Below the horizon this is the ground.
    pub fn radiance(&self, dir: Vec3) -> [f32; 3] {
        if dir.z <= 0.0 || !self.valid {
            return self.ground;
        }
        let cos_theta = dir.z;
        let gamma = dir.dot(self.sun_dir).clamp(-1.0, 1.0).acos();
        // Normalised against `F(0, θs)` — the zenith with the sun where it is — exactly as Preetham
        // publishes it. Cached in `norm` at construction.
        let mut yxy = [0.0f32; 3];
        for (ch, v) in yxy.iter_mut().enumerate() {
            *v = self.perez_f(ch, cos_theta, gamma) / self.norm[ch];
        }
        // Luminance is relative to a zenith of 1; chromaticity is the zenith chromaticity carried
        // through the same ratio, which is exactly how Preetham applies it.
        let rgb = yxy_to_rgb(yxy[0], self.zenith_xy[0] * yxy[1], self.zenith_xy[1] * yxy[2]);
        [rgb[0] * self.scale, rgb[1] * self.scale, rgb[2] * self.scale]
    }

    /// Radiance including the **sun's disc** — what a camera ray or a mirror sees, as opposed to
    /// what the diffuse integral wants (the sun is already accounted for there as a direct light,
    /// so including it twice would double-count it).
    ///
    /// The disc is 0.53° across in reality; it is widened here to about 1.4° so that a single
    /// reflected ray still lands on it often enough to read as a highlight rather than as noise.
    pub fn radiance_with_sun(&self, dir: Vec3) -> [f32; 3] {
        let mut c = self.radiance(dir);
        if !self.valid {
            return c;
        }
        let d = dir.dot(self.sun_dir);
        const COS_DISC: f32 = 0.999_7; // ≈ 1.4°
        if d > COS_DISC {
            let k = ((d - COS_DISC) / (1.0 - COS_DISC)).clamp(0.0, 1.0);
            // Smooth limb so the disc has no aliased edge at any resolution.
            let k = k * k * (3.0 - 2.0 * k);
            for i in 0..3 {
                c[i] += self.sun_col[i] * k;
            }
        }
        c
    }

    /// Project the dome (no sun disc) onto 9 spherical-harmonic coefficients and pre-convolve with
    /// the clamped cosine lobe, so [`sh_ambient`] is a direct multiplier on albedo.
    ///
    /// Sampled on a **Fibonacci sphere**: deterministic, near-uniform, and free of the clumping a
    /// random set has at this sample count — a stochastic bake would make the ambient shimmer as
    /// the sun moves, which is far more visible than a small bias.
    pub fn sh9(&self) -> [[f32; 3]; 9] {
        const N: usize = 1024;
        let mut sh = [[0.0f32; 3]; 9];
        let ga = std::f32::consts::PI * (3.0 - 5.0f32.sqrt()); // golden angle
        for i in 0..N {
            let z = 1.0 - 2.0 * (i as f32 + 0.5) / N as f32;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let phi = i as f32 * ga;
            let d = Vec3::new(r * phi.cos(), r * phi.sin(), z);
            let l = self.radiance(d);
            let b = sh_basis(d);
            for (k, bk) in b.iter().enumerate() {
                for c in 0..3 {
                    sh[k][c] += l[c] * bk;
                }
            }
        }
        // Each sample carries an equal solid angle 4π/N.
        let w = 4.0 * std::f32::consts::PI / N as f32;
        for coef in sh.iter_mut() {
            for c in coef.iter_mut() {
                *c *= w;
            }
        }
        sh
    }

    /// Scale the sky so an up-facing Lambertian surface receives exactly `target` — the ambient
    /// the app already produced. See the module note on why the absolute level is inherited rather
    /// than derived. `ground` becomes the lower hemisphere's radiance verbatim.
    ///
    /// Order matters: `ground` is set FIRST because it contributes to the up-facing irradiance
    /// through the lower half of the hemisphere integral, and the solve has to account for it.
    pub fn calibrate(&mut self, target: [f32; 3], ground: [f32; 3], sun_col: [f32; 3]) {
        self.ground = ground;
        self.sun_col = sun_col;
        self.scale = 1.0;
        let up = sh_ambient(&self.sh9(), Vec3::Z);
        // Solve on luminance rather than per channel: scaling the channels independently would
        // rewrite the sky's colour, which is the one thing this model is here to get right.
        let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let (want, have) = (lum(target), lum(up));
        self.scale = if have > 1e-6 { (want / have).max(0.0) } else { 0.0 };
        if !self.scale.is_finite() {
            self.scale = 0.0;
        }
    }

    /// Pack the Perez coefficients for the shader: five `vec3`s, one per coefficient, laid out
    /// `(Y, x, y)` — the same order [`SKY_GLSL`] indexes them.
    pub fn perez_uniform(&self) -> [f32; 15] {
        let mut o = [0.0f32; 15];
        for c in 0..5 {
            for ch in 0..3 {
                o[c * 3 + ch] = self.perez[c][ch];
            }
        }
        o
    }

    /// `F(0, θs)` per channel — the normalisation divisor, computed once on the CPU so the shader
    /// does not repeat it per fragment.
    pub fn norm_uniform(&self) -> [f32; 3] {
        self.norm
    }
}

/// The nine real spherical-harmonic basis functions up to `l = 2`, evaluated at a unit direction.
fn sh_basis(d: Vec3) -> [f32; 9] {
    let (x, y, z) = (d.x, d.y, d.z);
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

/// Irradiance from the SH coefficients, divided by π — i.e. the value that multiplies an albedo to
/// give outgoing diffuse radiance. Ramamoorthi & Hanrahan's closed form; the `c1..c5` constants
/// already fold in the cosine-lobe convolution `Â₀ = π, Â₁ = 2π/3, Â₂ = π/4`.
pub fn sh_ambient(sh: &[[f32; 3]; 9], n: Vec3) -> [f32; 3] {
    const C1: f32 = 0.429_043;
    const C2: f32 = 0.511_664;
    const C3: f32 = 0.743_125;
    const C4: f32 = 0.886_227;
    const C5: f32 = 0.247_708;
    let (x, y, z) = (n.x, n.y, n.z);
    let mut out = [0.0f32; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let e = C1 * sh[8][c] * (x * x - y * y) + C3 * sh[6][c] * z * z + C4 * sh[0][c] - C5 * sh[6][c]
            + 2.0 * C1 * (sh[4][c] * x * y + sh[7][c] * x * z + sh[5][c] * y * z)
            + 2.0 * C2 * (sh[3][c] * x + sh[1][c] * y + sh[2][c] * z);
        *o = (e / std::f32::consts::PI).max(0.0);
    }
    out
}

// ── screen-space ambient occlusion settings ──────────────────────────────────────────────────

/// Ambient occlusion parameters. AO does not model a light — it models the fact that a crease sees
/// less of the sky than a flat wall does, which is precisely the information a per-normal ambient
/// term throws away. Without it the new sky ambient would make the render *flatter*, not rounder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AoSettings {
    pub enabled: bool,
    /// World-space sampling radius, metres. Roughly "how far a crease reaches".
    pub radius: f32,
    /// Strength, 0..2. 1 = the geometric estimate; above that is artistic.
    pub strength: f32,
}

impl Default for AoSettings {
    fn default() -> Self {
        // 0.5 m suits interiors and furniture — big enough to darken a wall/floor junction, small
        // enough that a doorway does not smear onto the room behind it.
        Self { enabled: true, radius: 0.5, strength: 1.0 }
    }
}

/// SCREEN-SPACE GLOBAL ILLUMINATION — one bounce of coloured light between visible surfaces.
///
/// OFF by default, and deliberately so. It can only gather from surfaces the camera already sees,
/// which makes it the one lighting term here whose error depends on where you are standing: pan a
/// bounce source off screen and its contribution fades out. That is a fair trade for a viewport
/// and a poor one for a measurement, so it is something you switch on when you want a room to feel
/// connected — not something that quietly happens to every scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GiSettings {
    pub enabled: bool,
    /// How far a bounce reaches, in metres.
    pub radius: f32,
    /// 0..2. 1 is the geometric estimate; above that is artistic licence.
    pub strength: f32,
}

impl Default for GiSettings {
    fn default() -> Self {
        // 1.5 m: far enough that a wall lights the floor beside it and a rug tints what stands on
        // it, short enough that the gather stays inside what one screen can actually show.
        Self { enabled: false, radius: 1.5, strength: 1.0 }
    }
}

/// What the sky should be drawn as behind the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// The flat dark studio grey the viewport has always had.
    #[default]
    Studio,
    /// The physical sky itself, with the sun's disc — so the render is lit by what you can see.
    Sky,
}

/// Everything the renderer needs to light and draw the environment for one frame. Bundled because
/// `render` already takes twenty-odd arguments and these travel together.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvRender {
    /// `None` ⇒ no sky lighting; the studio key light and flat ambient apply (unchanged behaviour).
    pub sky: Option<Sky>,
    /// The baked ambient, computed once per sun change rather than per frame.
    pub sh: [[f32; 3]; 9],
    pub ao: AoSettings,
    /// One bounce of coloured light between visible surfaces — see [`GiSettings`].
    pub gi: GiSettings,
    pub backdrop: Backdrop,
    /// Strength of environment reflections, 0..1 — the old "reflection" slider's new home.
    pub reflections: f32,
    /// An HDR environment is loaded and should be used in place of the analytic sky. The pixels
    /// live on the renderer as a texture; this only says whether to look at it, and how.
    pub hdri: Option<HdriUse>,
    /// Angular DIAMETER of the sun's disc, degrees. The real sun is 0.53°.
    ///
    /// This is what makes a shadow edge crisp under a table leg and soft under a roof eave: the
    /// penumbra is the sun's disc projected past the occluder, so it widens with the distance the
    /// shadow has to travel. 0 gives the mathematically perfect point source — a razor edge
    /// everywhere, which never happens outdoors and is one of the things that most gives a
    /// daylight render away. Only has an effect while temporal accumulation is running.
    pub sun_angle_deg: f32,
    /// The air between the camera and the model — see [`FogSettings`].
    pub fog: FogSettings,
}

/// How a loaded HDR environment is being used this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HdriUse {
    /// Multiplies the environment's radiance. 1.0 = as photographed.
    pub strength: f32,
    /// Yaw about Z, radians — turn the world without moving the model.
    pub rot: f32,
}

impl Default for HdriUse {
    fn default() -> Self {
        Self { strength: 1.0, rot: 0.0 }
    }
}

/// The sun's true angular diameter, degrees — 1.39 million km at 150 million km away.
pub const SUN_ANGLE_DEG: f32 = 0.53;

/// HEIGHT FOG — the air between the camera and the model.
///
/// Distance in a render reads almost entirely from how much contrast the air takes out of things.
/// Without it, a wall 200 m away is exactly as saturated and exactly as dark in its shadows as one
/// at 2 m, and the eye reads the whole scene as a model on a table rather than as a place.
///
/// Height fog rather than plain distance fog because air really does thin out with altitude: fog
/// pools in a valley and the hills above it stay clear, and looking DOWN through it from a roof
/// terrace picks up more haze than looking level. Constant-density fog gets that backwards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogSettings {
    pub enabled: bool,
    /// Inscattered radiance — scene-referred and LINEAR, like everything else the composite adds.
    /// This is what distance fades TOWARDS, so it wants to be roughly the sky's own brightness in
    /// that direction; too dark and the horizon reads as smoke rather than air.
    pub color: [f32; 3],
    /// Extinction per metre at `base_z`. 0.002 ⇒ half the light is gone by ~350 m.
    pub density: f32,
    /// The world Z at which `density` applies — usually the ground.
    pub base_z: f32,
    /// How fast density falls off with height, per metre. 0 = uniform fog at every altitude.
    pub falloff: f32,
}

impl Default for FogSettings {
    fn default() -> Self {
        // OFF, and gentle when switched on: at 0.0015/m an object is barely touched at 20 m and
        // clearly softened at 400 m, which is the range a building site actually spans.
        Self { enabled: false, color: [0.55, 0.62, 0.75], density: 0.0015, base_z: 0.0, falloff: 0.02 }
    }
}

/// The fog integral, as GLSL. Shared by the composite and the accumulation resolve, which have to
/// compose a frame identically or turning accumulation on would change what the picture is.
///
/// Expects `u_fog_col`, `u_fog_density`, `u_fog_base` and `u_fog_falloff` to be in scope.
pub const FOG_GLSL: &str = r#"
    // Transmittance from `cam` to `p` through fog whose density falls off exponentially with Z.
    //
    // Closed form, not a march: for density ρ(z) = ρ₀·e^(−k(z−z₀)) the optical depth along a
    // straight segment integrates exactly to ρ(cam)·L·(1 − e^(−k·Δz))/(k·Δz). One exp, no loop,
    // and correct at every distance — a fixed-step march would band visibly across a long view.
    float fog_transmittance(vec3 cam, vec3 p) {
        if (u_fog_density <= 0.0) return 1.0;
        vec3 seg = p - cam;
        float L = length(seg);
        if (L < 1e-4) return 1.0;
        float rho = u_fog_density * exp(-u_fog_falloff * (cam.z - u_fog_base));
        float kdz = u_fog_falloff * seg.z;
        // The integral has a removable singularity at Δz = 0 (a level look). Take the limit rather
        // than dividing by it, or the horizon grows a bright seam exactly where the eye is.
        float tau = (abs(kdz) < 1e-4) ? rho * L : rho * L * (1.0 - exp(-kdz)) / kdz;
        return exp(-max(tau, 0.0));
    }
"#;

/// Where a direction lands in an equirectangular environment map, as GLSL.
///
/// ONE definition, substituted into every shader that reads an environment: the viewport's sky and
/// the GPU path tracer both need it, and if the two ever disagreed by so much as a sign the render
/// would come back with the world turned round from what the viewport had shown. Expects a
/// `uniform float u_env_rot` to be in scope.
pub const ENV_UV_GLSL: &str = r#"
    vec2 env_uv(vec3 d) {
        d = normalize(d);
        // Matches `env_map::sample_px` exactly: +X is u = 0, v runs down from the zenith. The
        // sampler wraps in u (GL_REPEAT), so the seam needs no fract() here.
        float u = (atan(d.y, d.x) + u_env_rot) / 6.28318530718;
        float v = acos(clamp(d.z, -1.0, 1.0)) / 3.14159265359;
        return vec2(u, v);
    }
"#;

impl Default for EnvRender {
    fn default() -> Self {
        Self { sky: None, sh: [[0.0; 3]; 9], ao: AoSettings::default(), gi: GiSettings::default(), backdrop: Backdrop::Studio, reflections: 1.0, hdri: None, sun_angle_deg: SUN_ANGLE_DEG, fog: FogSettings::default() }
    }
}

impl EnvRender {
    /// No environment at all — for the SIMLUX lux view, which must not be relit.
    pub fn none() -> Self {
        // GI off too: the lux view is a MEASUREMENT, and a term whose value depends on where the
        // camera is standing has no business anywhere near a false-colour illuminance scale.
        Self { sky: None, sh: [[0.0; 3]; 9], ao: AoSettings { enabled: false, ..Default::default() }, gi: GiSettings { enabled: false, ..Default::default() }, backdrop: Backdrop::Studio, reflections: 0.0, hdri: None, sun_angle_deg: 0.0, fog: FogSettings { enabled: false, ..FogSettings::default() } }
    }
}

// ── the GLSL twin ────────────────────────────────────────────────────────────────────────────

/// The sky and the SH ambient as GLSL. Expects `u_perez[5]`, `u_perez_norm`, `u_zenith_xy`,
/// `u_sky_scale`, `u_sky_sun`, `u_sky_ground`, `u_sky_sun_col`, `u_sky_on` and `u_sh[9]`, and
/// exposes `sky_radiance(vec3)`, `sky_with_sun(vec3)` and `sh_ambient(vec3)`.
///
/// Only the Perez formula is duplicated here — every coefficient is computed in Rust and uploaded,
/// so the turbidity fits and the zenith cubics exist in exactly one place.
pub const SKY_GLSL: &str = r#"
    uniform vec3  u_perez[5];      // A..E, each (Y, x, y)
    uniform vec3  u_perez_norm;    // F(0, theta_s) per channel
    uniform vec2  u_zenith_xy;
    uniform float u_sky_scale;
    uniform vec3  u_sky_sun;       // unit direction TO the sun
    uniform vec3  u_sky_ground;    // lower-hemisphere radiance
    uniform vec3  u_sky_sun_col;   // the sun disc's radiance
    uniform int   u_sky_on;        // 0 = no physical sky (studio ambient)
    uniform vec3  u_sh[9];         // pre-convolved irradiance SH

    // ── IMAGE-BASED LIGHTING ────────────────────────────────────────────────────────────────
    // An equirectangular HDR environment as ONE 2D texture whose mip chain IS the roughness
    // chain: level 0 is the mirror, level 5 fully rough (see `crate::env_map`). A glossy lookup
    // is therefore `textureLod(u_env, uv, roughness * 5)`, and the hardware's trilinear filter
    // interpolates between roughnesses for nothing.
    //
    // DIFFUSE needs nothing here. `u_sh` above is already the interface, so an HDRI simply
    // supplies those nine coefficients where the analytic sky used to.
    // TWO textures, for two different jobs.
    //
    //   u_env    the GGX-convolved chain, for GLOSSY reflection. Its level 0 is capped so the
    //            chain stays a clean 6-level mip chain, which is fine: everything it is used for
    //            is blur.
    //   u_env_bg the environment at its FULL resolution, for the BACKDROP and for near-mirror
    //            reflection. This one is looked at directly and magnified — a 60° field of view
    //            across a 1400-pixel window magnifies a 1024-wide panorama roughly eight times,
    //            and a 4K HDRI drawn from a 1024 copy is visibly, obviously soft. That was the
    //            first thing anyone noticed on loading one.
    uniform sampler2D u_env;
    uniform sampler2D u_env_bg;
    uniform int   u_env_on;        // 0 = analytic sky, 1 = HDR environment
    uniform float u_env_rot;       // yaw, radians — turn the world without moving the model
    uniform float u_env_strength;

    // NOTE: this function is duplicated VERBATIM in `ENV_UV_GLSL`, which the GPU path tracer
    // includes. `the_tracer_and_the_viewport_read_the_environment_the_same_way` fails if they ever
    // drift — a sign flip here would come back as a render with the world turned round.
    vec2 env_uv(vec3 d) {
        d = normalize(d);
        // Matches `env_map::sample_px` exactly: +X is u = 0, v runs down from the zenith. The
        // sampler wraps in u (GL_REPEAT), so the seam needs no fract() here.
        float u = (atan(d.y, d.x) + u_env_rot) / 6.28318530718;
        float v = acos(clamp(d.z, -1.0, 1.0)) / 3.14159265359;
        return vec2(u, v);
    }
    // What you SEE: full resolution, no roughness, no compromise.
    vec3 env_radiance(vec3 d) {
        return texture(u_env_bg, env_uv(d)).rgb * u_env_strength;
    }
    // What a SURFACE sees. Near-mirror comes off the full-resolution map too, because a polished
    // surface reflects detail the convolved chain has already thrown away; from a fifth of the way
    // up the roughness range the GGX chain takes over entirely.
    vec3 env_glossy(vec3 d, float rough) {
        float r = clamp(rough, 0.0, 1.0);
        vec3 sharp = texture(u_env_bg, env_uv(d)).rgb;
        vec3 blur = textureLod(u_env, env_uv(d), r * 5.0).rgb;
        return mix(sharp, blur, smoothstep(0.0, 0.2, r)) * u_env_strength;
    }

    // CIE XYZ -> linear sRGB. mat3() takes COLUMNS, so this is the transpose of the row-wise
    // matrix in env.rs (same convention as the AgX matrices in color.rs).
    const mat3 XYZ_TO_RGB = mat3(
         3.2406, -0.9689,  0.0557,
        -1.5372,  1.8758, -0.2040,
        -0.4986,  0.0415,  1.0570);

    vec3 yxy_to_rgb(float Y, float x, float y) {
        if (y <= 1e-5) return vec3(0.0);
        vec3 xyz = vec3(x / y * Y, Y, (1.0 - x - y) / y * Y);
        return max(XYZ_TO_RGB * xyz, 0.0);
    }

    // (1 + A e^(B/cos t)) (1 + C e^(D g) + E cos^2 g), per channel, all three at once.
    vec3 perez_f(float cos_theta, float gamma) {
        float ct = max(cos_theta, 0.01);
        float cg = cos(gamma);
        return (1.0 + u_perez[0] * exp(u_perez[1] / ct))
             * (1.0 + u_perez[2] * exp(u_perez[3] * gamma) + u_perez[4] * cg * cg);
    }

    vec3 sky_radiance(vec3 dir) {
        if (u_sky_on == 0 || dir.z <= 0.0) return u_sky_ground;
        float gamma = acos(clamp(dot(dir, u_sky_sun), -1.0, 1.0));
        vec3 yxy = perez_f(dir.z, gamma) / max(u_perez_norm, 1e-6);
        return yxy_to_rgb(yxy.x, u_zenith_xy.x * yxy.y, u_zenith_xy.y * yxy.z) * u_sky_scale;
    }

    // With the sun's disc — for the background and for mirror reflections. The diffuse ambient must
    // NOT use this: the sun is already a direct light there.
    vec3 sky_with_sun(vec3 dir) {
        // An HDR environment IS the sky — its sun is a bright patch of the image, not an added
        // disc, so this returns before any of the analytic machinery below runs.
        if (u_env_on == 1) return env_radiance(dir);
        vec3 c = sky_radiance(dir);
        if (u_sky_on == 0) return c;
        float d = dot(dir, u_sky_sun);
        const float cos_disc = 0.9997;
        if (d > cos_disc) {
            float k = clamp((d - cos_disc) / (1.0 - cos_disc), 0.0, 1.0);
            c += u_sky_sun_col * (k * k * (3.0 - 2.0 * k));
        }
        return c;
    }

    // Ramamoorthi & Hanrahan, divided by PI so it multiplies an albedo directly.
    vec3 sh_ambient(vec3 n) {
        const float c1 = 0.429043, c2 = 0.511664, c3 = 0.743125, c4 = 0.886227, c5 = 0.247708;
        vec3 e = c1 * u_sh[8] * (n.x * n.x - n.y * n.y) + c3 * u_sh[6] * n.z * n.z
               + c4 * u_sh[0] - c5 * u_sh[6]
               + 2.0 * c1 * (u_sh[4] * n.x * n.y + u_sh[7] * n.x * n.z + u_sh[5] * n.y * n.z)
               + 2.0 * c2 * (u_sh[3] * n.x + u_sh[1] * n.y + u_sh[2] * n.z);
        return max(e / 3.14159265359, 0.0);
    }
"#;

/// The split-sum environment BRDF as GLSL — Karis's analytic fit to the usual 2D lookup table.
/// A LUT would be marginally more accurate and would cost a texture upload, a texture unit and a
/// bind on every draw; the fit is within about a percent over the whole roughness range.
pub const ENV_BRDF_GLSL: &str = r#"
    // Returns the scale and bias to apply to F0 for a prefiltered environment reflection.
    vec2 env_brdf(float rough, float NoV) {
        const vec4 c0 = vec4(-1.0, -0.0275, -0.572,  0.022);
        const vec4 c1 = vec4( 1.0,  0.0425,  1.040, -0.040);
        vec4 r = rough * c0 + c1;
        float a004 = min(r.x * r.x, exp2(-9.28 * NoV)) * r.x + r.y;
        return vec2(-1.04, 1.04) * a004 + r.zw;
    }
"#;

/// Rust twin of `env_brdf` — used by the CPU material preview and the path tracer.
pub fn env_brdf(rough: f32, n_o_v: f32) -> [f32; 2] {
    let c0 = [-1.0f32, -0.0275, -0.572, 0.022];
    let c1 = [1.0f32, 0.0425, 1.040, -0.040];
    let r: Vec<f32> = (0..4).map(|i| rough * c0[i] + c1[i]).collect();
    let a004 = (r[0] * r[0]).min((-9.28 * n_o_v).exp2()) * r[0] + r[1];
    [-1.04 * a004 + r[2], 1.04 * a004 + r[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noon() -> Sky {
        let mut s = Sky::new(Vec3::new(0.2, -0.3, 0.93).normalize(), DEFAULT_TURBIDITY);
        s.calibrate([0.9, 0.98, 1.15], [0.25, 0.24, 0.21], [3.0, 2.9, 2.7]);
        s
    }

    /// The whole point of a sky model rather than a lerp: it is not the same everywhere. Two
    /// independent gradients have to be there.
    ///
    /// 1. **Around the sun** — forward-scattering makes the sky near the sun far brighter than the
    ///    sky opposite it at the same elevation. This is what puts a bright side and a dim side on
    ///    a building, which the old two-colour ambient could not do at all.
    /// 2. **Down toward the horizon** — along a fixed bearing *away* from the sun, radiance rises
    ///    as elevation falls, because the line of sight passes through more atmosphere. (Note this
    ///    is not "horizon brighter than zenith": with a high sun the zenith sits near the solar
    ///    aureole and wins outright. Testing it as a same-bearing gradient is the claim the model
    ///    actually makes.)
    #[test]
    fn the_sky_is_not_uniform() {
        let s = noon();
        let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let sun_h = Vec3::new(s.sun_dir.x, s.sun_dir.y, 0.0).normalize();
        // `bearing` = +1 toward the sun's compass bearing, −1 away from it.
        let at = |bearing: f32, elev_deg: f32| {
            let e = elev_deg.to_radians();
            (sun_h * bearing * e.cos() + Vec3::Z * e.sin()).normalize()
        };
        let (near, far) = (lum(s.radiance(at(1.0, 30.0))), lum(s.radiance(at(-1.0, 30.0))));
        assert!(near > far * 1.2, "the sun's side of the sky must be brighter: {near} vs {far}");

        let (high, low) = (lum(s.radiance(at(-1.0, 60.0))), lum(s.radiance(at(-1.0, 3.0))));
        assert!(low > high * 1.05, "radiance must rise toward the horizon: {low} at 3° vs {high} at 60°");
    }

    /// A clear sky is BLUE — the chromaticity fit has to survive the Yxy → RGB trip. (This catches
    /// a transposed matrix, which is exactly the bug that bit the AgX conversion in Phase 1.)
    #[test]
    fn a_clear_zenith_is_blue() {
        let s = noon();
        let z = s.radiance(Vec3::Z);
        assert!(z[2] > z[1] && z[1] > z[0], "clear zenith should run B > G > R: {z:?}");
        // …and a very hazy sky is much closer to neutral.
        let mut hazy = Sky::new(s.sun_dir, 9.0);
        hazy.calibrate([1.0; 3], [0.2; 3], [2.0; 3]);
        let hz = hazy.radiance(Vec3::Z);
        let ratio = |c: [f32; 3]| c[2] / c[0].max(1e-6);
        assert!(ratio(hz) < ratio(z), "turbidity must wash the blue out: {} vs {}", ratio(hz), ratio(z));
    }

    /// Every direction must give finite, non-negative radiance — including straight at the horizon,
    /// where `e^(B/cos θ)` is one divide away from infinity.
    #[test]
    fn radiance_is_finite_everywhere() {
        for alt in [-1.0f32, -0.001, 0.0, 0.0001, 0.01, 0.5, 0.999, 1.0] {
            for az in 0..16 {
                let a = az as f32 / 16.0 * std::f32::consts::TAU;
                let r = (1.0 - alt * alt).max(0.0).sqrt();
                let d = Vec3::new(r * a.cos(), r * a.sin(), alt);
                for s in [noon(), Sky::new(Vec3::new(1.0, 0.0, 0.02).normalize(), 6.0)] {
                    let c = s.radiance_with_sun(d.normalize_or(Vec3::Z));
                    assert!(c.iter().all(|v| v.is_finite() && *v >= 0.0), "{c:?} at {d:?}");
                }
            }
        }
    }

    /// The SH bake must reproduce the irradiance a brute-force hemisphere integral gives. This is
    /// the load-bearing approximation of the whole diffuse path: if it is wrong, every surface is
    /// wrong, and it is wrong in a way that looks plausible.
    ///
    /// Order-2 SH cannot represent a sharp sky, so the tolerance is 12% — that is the *model's*
    /// error, not a slack test. What it pins is that there is no scale factor or missing π.
    #[test]
    fn sh_reconstructs_the_hemisphere_integral() {
        let s = noon();
        let sh = s.sh9();
        for n in [Vec3::Z, Vec3::X, Vec3::Y, -Vec3::X, Vec3::new(0.6, 0.5, 0.62).normalize(), -Vec3::Z] {
            let n = n.normalize();
            // Brute force: ∫ L(ω) max(n·ω, 0) dω / π, over a Fibonacci sphere.
            const N: usize = 20000;
            let ga = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
            let mut acc = [0.0f64; 3];
            for i in 0..N {
                let z = 1.0 - 2.0 * (i as f32 + 0.5) / N as f32;
                let r = (1.0 - z * z).max(0.0).sqrt();
                let phi = i as f32 * ga;
                let d = Vec3::new(r * phi.cos(), r * phi.sin(), z);
                let c = n.dot(d).max(0.0);
                if c <= 0.0 {
                    continue;
                }
                let l = s.radiance(d);
                for k in 0..3 {
                    acc[k] += (l[k] * c) as f64;
                }
            }
            let w = 4.0 * std::f64::consts::PI / N as f64 / std::f64::consts::PI;
            let exact = [(acc[0] * w) as f32, (acc[1] * w) as f32, (acc[2] * w) as f32];
            let got = sh_ambient(&sh, n);
            for k in 0..3 {
                let rel = (got[k] - exact[k]).abs() / exact[k].max(1e-4);
                assert!(rel < 0.12, "n={n:?} ch{k}: SH {got:?} vs exact {exact:?} ({:.1}%)", rel * 100.0);
            }
        }
    }

    /// `calibrate` must hit its target on an up-facing surface — this is the contract that keeps
    /// the daylight calibration from Phase 1 intact while the sky's *shape* changes underneath it.
    #[test]
    fn calibration_preserves_the_up_facing_ambient() {
        for target in [[0.9, 0.98, 1.15], [0.2, 0.22, 0.3], [2.5, 2.4, 2.6]] {
            let mut s = Sky::new(Vec3::new(0.3, 0.1, 0.8).normalize(), DEFAULT_TURBIDITY);
            s.calibrate(target, [0.2, 0.19, 0.17], [3.0; 3]);
            let got = sh_ambient(&s.sh9(), Vec3::Z);
            let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            let rel = (lum(got) - lum(target)).abs() / lum(target);
            assert!(rel < 0.02, "target {target:?} got {got:?} ({:.1}%)", rel * 100.0);
        }
        // A sun below the horizon must not divide by a collapsed normalisation.
        let mut night = Sky::new(Vec3::new(0.3, 0.1, -0.4).normalize(), DEFAULT_TURBIDITY);
        night.calibrate([0.05; 3], [0.02; 3], [0.0; 3]);
        assert!(!night.valid);
        assert!(night.radiance(Vec3::Z).iter().all(|v| v.is_finite()));
    }

    /// The sun's disc belongs in the reflection/background path and NOWHERE else — counting it in
    /// the diffuse SH as well would double the direct sun, which the shader already applies.
    #[test]
    fn the_sun_disc_is_only_in_the_visible_sky() {
        let s = noon();
        let on_sun = s.radiance_with_sun(s.sun_dir);
        let dome = s.radiance(s.sun_dir);
        assert!(on_sun[0] > dome[0] + 1.0, "the disc must be visible: {on_sun:?} vs {dome:?}");
        // 3° off the sun there is no disc left.
        let off = Vec3::new(s.sun_dir.x + 0.06, s.sun_dir.y, s.sun_dir.z).normalize();
        let c = s.radiance_with_sun(off);
        assert!((c[0] - s.radiance(off)[0]).abs() < 1e-4, "disc must be tight: {c:?}");
    }

    /// The GLSL copy is a separate text. Pin the pieces that would silently diverge: the XYZ→RGB
    /// matrix (compared numerically — the two texts round differently), the SH constants, and the
    /// disc width. The Perez coefficients need no such check: they are uploaded, not duplicated.
    #[test]
    fn sky_glsl_matches_rust() {
        let lits: Vec<f32> = SKY_GLSL
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter_map(|t| t.parse::<f32>().ok())
            .collect();
        for row in XYZ_TO_RGB.iter() {
            for v in row {
                assert!(lits.iter().any(|l| (l - v).abs() < 1e-4), "{v} missing from SKY_GLSL");
            }
        }
        for c in ["0.429043", "0.511664", "0.743125", "0.886227", "0.247708"] {
            assert!(SKY_GLSL.contains(c), "SH constant {c} missing from the shader");
        }
        assert!(SKY_GLSL.contains("0.9997"), "the sun-disc width must match radiance_with_sun");
        // Uniform names the renderer looks up must exist in the source it is linked from.
        for u in ["u_perez", "u_perez_norm", "u_zenith_xy", "u_sky_scale", "u_sky_sun", "u_sky_ground", "u_sky_sun_col", "u_sky_on", "u_sh"] {
            assert!(SKY_GLSL.contains(u), "SKY_GLSL has no `{u}`");
        }
        assert!(ENV_BRDF_GLSL.contains("env_brdf"));
    }

    /// The split-sum fit has to behave: a mirror keeps nearly all of F0, a fully rough surface
    /// keeps much less, and the result never leaves 0..1 (an energy gain would compound per bounce).
    #[test]
    fn env_brdf_is_sane() {
        let mirror = env_brdf(0.0, 1.0);
        let matte = env_brdf(1.0, 1.0);
        assert!(mirror[0] > 0.9, "a smooth surface keeps its F0: {mirror:?}");
        assert!(matte[0] < mirror[0], "roughness must cost energy: {matte:?} vs {mirror:?}");
        for ri in 0..=10 {
            for vi in 1..=10 {
                let ab = env_brdf(ri as f32 / 10.0, vi as f32 / 10.0);
                assert!(ab[0] >= -0.01 && ab[0] <= 1.01 && ab[1] >= -0.01 && ab[1] <= 1.01, "{ab:?}");
            }
        }
    }
}
