//! 3D Factory — the `cad_solid` 3D solid layer, wired into the real app.
//!
//! This is the sandbox's core (`cad_solid/examples/sandbox.rs`) brought inside `cad_app`,
//! where all ~31.8k lines of 2D drafting + modify already work — so every plane can get the
//! FULL 2D toolset with nothing reimplemented. See `mentor MD/VENUE_DECISION_2D_ON_EVERY_PLANE.md`.
//!
//! What is deliberately NOT here: a renderer, a camera math fn, a command line, a cursor.
//! The app already has all of those. We reuse [`crate::light3d`]'s `Scene3dRenderer` + `mvp`
//! (the sandbox had duplicated both) and drive them with a `cad_solid::Model`.

use cad_solid::{BoolOp, Feature, Frame, Model, Placement, Plane, Primitive, SolidMesh};
use glam::{Mat4, Vec2, Vec3};

use crate::light3d::V3;

/// The standard camera orientations the nav gizmo snaps to — the six orthographic
/// faces plus an isometric, exactly the set every 3D solid app puts in its corner cube.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdView {
    Top, Bottom, Front, Back, Left, Right, Iso,
}

/// The 3D-Factory zoom mode — mirrors the 2D zoom command. Bare `z` → `Window` (the 2D
/// default: DRAG a box, or click two corners, with an amber "zoom window" rubber-band);
/// `z r` → `RealTime` (drag up/down dollies). `Off` = idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomMode {
    Off,
    Window,
    RealTime,
}

/// A promoted wall kept ALIVE — the Factory owns its **footprint** (the ground-plane
/// polyline) so the wall stays fully editable after promotion: change its height, or
/// move / add / delete a footprint vertex, and it re-derives.
///
/// The floor ring (`z = 0`) and the ceiling ring (`z = height`) are BOTH derived from the
/// SAME `footprint` points — so a vertex is a vertical edge present on *both* rings by
/// construction; they can never drift apart. This is why "add a vertex in Top view → it
/// lands on top AND bottom" is automatic (owner, 2026-07-22), not a special case: there is
/// only one set of points driving both rings.
///
/// Each consecutive footprint pair extrudes to one Box `Feature`; `segments[i]` is the
/// feature id of the i-th segment (`footprint.len() − 1` of them), in order. `rake`
/// (lean-from-vertical) is stored for the day the kernel gains a tilt DOF — today a
/// `Feature` is axis-aligned only, so it is not applied yet (and only then can top ≠
/// bottom, relaxing the "both rings" coupling).
#[derive(Clone, Debug)]
pub struct WallInst {
    /// Ground-plane footprint, ≥ 2 points. Shared by the floor and ceiling rings.
    pub footprint: Vec<Vec2>,
    /// One Box feature id per segment (`footprint.len() − 1` of them), in order.
    pub segments: Vec<u32>,
    pub thickness: f32,
    pub height: f32,
    pub rake_deg: f32,
    /// Z the wall STANDS ON — the base of the storey it was built on. Held on the wall
    /// rather than only in the feature's placement because `rederive_wall` drops and
    /// rebuilds the Boxes: without it, editing a vertex on the third floor would silently
    /// drop that wall to the ground.
    pub base_z: f32,
}

/// An open sketch-on-plane session.
///
/// **The core trick of 3D_Factory:** while this is live, the app's active `doc` IS the
/// sketch's `Document`. Every 2D tool in `cad_app` only ever knows `self.doc` — so draw,
/// fillet (with its R/T/M/P options), trim, extend, offset, chamfer, break, the command
/// line, snaps and layers ALL operate on the plane, **unchanged and complete**, with
/// nothing reimplemented. That is the whole thesis of this fork.
///
/// `undo_stack`/`redo_stack` are `Vec<Document>` (full snapshots), so they must be parked
/// alongside the model-space doc — otherwise an undo inside the sketch would restore a
/// model-space document over the sketch. The sketch gets a fresh, empty undo history.
pub struct SketchSession {
    /// Index into `Model::sketches`.
    pub idx: usize,
    pub saved_doc: cad_kernel::Document,
    /// The main drawing's undo/redo history, parked while the sketch owns `doc`. Holds
    /// `UndoStep`s (not bare Documents) since undo spans 2D and 3D in one stack.
    pub saved_undo: Vec<crate::app::UndoStep>,
    pub saved_redo: Vec<crate::app::UndoStep>,
}

/// Daylight settings the user edits (▼ Environment) — where/when the building is, so the sun can be
/// located with Radiance's model ([`crate::solar`]) and the scene lit accordingly. Not the render
/// output; the resolved directional light is pushed to [`set_sun_light`] each frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunEnv {
    /// Light the scene by the sun. Off → the original fixed studio key light (no visual change).
    pub enabled: bool,
    pub lat_deg: f32,
    /// Longitude, degrees, **positive east**.
    pub lon_deg: f32,
    /// UTC offset in hours, **positive east** (BST = +1, PST = −8).
    pub utc_offset: f32,
    pub month: u32,
    pub day: u32,
    /// Local standard time, hours 0..24.
    pub hour: f32,
    /// Rotate true north off world +Y (degrees) so the building can face any way.
    pub north_offset_deg: f32,
    /// Overall sun brightness multiplier.
    pub intensity: f32,
    /// Cast shadows from the sun (shadow-map pass). Independent of `enabled` lighting so it can be
    /// turned off if the shadow map misbehaves on a given scene.
    pub shadows: bool,
    /// Atmospheric turbidity for the analytic sky (see [`crate::env`]). ~2 = crisp alpine air,
    /// ~6 = hazy summer city, 10+ = fog. Drives both the sky's colour and its gradient.
    pub turbidity: f32,
    /// Draw the sky itself behind the model instead of the flat studio backdrop.
    pub sky_backdrop: bool,
    /// Ambient occlusion — the contact shading that keeps sky lighting from reading flat.
    pub ao: crate::env::AoSettings,
    /// One bounce of coloured light between visible surfaces. Off by default — see
    /// [`crate::env::GiSettings`] for why this one is opt-in when the others are not.
    pub gi: crate::env::GiSettings,
    /// Glass that BENDS what is behind it rather than only tinting it.
    pub refract: crate::env::RefractSettings,
    pub ssr: crate::env::SsrSettings,
    /// Strength of environment reflections, 0..1. What a glossy surface picks up from the sky.
    pub reflections: f32,
    /// Angular DIAMETER of the sun's disc, degrees — how soft its shadows are. The real sun is
    /// 0.53°; raising it stands in for haze or an overcast sky, which spread the source out. Only
    /// visible while the still-frame refinement is running (it is integrated over the samples).
    pub sun_angle_deg: f32,
    /// How many CASCADES the shadow map is split into. More means sharper shadows near the camera
    /// (each cascade covers less ground with the same 2048 texels) at the cost of one extra depth
    /// pass over the scene each. 1 = the single whole-scene map.
    pub shadow_cascades: u32,
    /// The air between the camera and the model — see [`crate::env::FogSettings`].
    pub fog: crate::env::FogSettings,
}

impl Default for SunEnv {
    fn default() -> Self {
        // London, midsummer noon — a pleasant high sun; OFF by default so nothing changes until
        // the user opts in.
        Self {
            enabled: false,
            lat_deg: 51.5,
            lon_deg: -0.13,
            utc_offset: 1.0,
            month: 6,
            day: 21,
            hour: 13.0,
            north_offset_deg: 0.0,
            intensity: 1.0,
            shadows: true,
            turbidity: crate::env::DEFAULT_TURBIDITY,
            sky_backdrop: true,
            ao: crate::env::AoSettings::default(),
            gi: crate::env::GiSettings::default(),
            refract: crate::env::RefractSettings::default(),
            ssr: crate::env::SsrSettings::default(),
            reflections: 1.0,
            sun_angle_deg: crate::env::SUN_ANGLE_DEG,
            // Three: the point at which a villa-sized site gets centimetre texels close to the
            // camera, and one more would cost a whole extra depth pass to sharpen ground that is
            // already too far away to look at.
            shadow_cascades: 3,
            fog: crate::env::FogSettings::default(),
        }
    }
}

impl SunEnv {
    /// Fold the settings into a render-cache signature so any change re-bakes the shaded buffers.
    fn hash_into(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        self.enabled.hash(h);
        for x in [self.lat_deg, self.lon_deg, self.utc_offset, self.hour, self.north_offset_deg, self.intensity, self.turbidity] {
            x.to_bits().hash(h);
        }
        self.month.hash(h);
        self.day.hash(h);
    }

    /// Resolve the whole environment for one frame: the sun as a **direct** light, and the analytic
    /// sky as everything else — diffuse ambient (via its SH projection) and glossy reflections.
    ///
    /// The sky is calibrated against [`Self::resolve`]'s old two-colour ambient, so this changes how
    /// light is *distributed* over the dome without changing how much of it there is. That keeps the
    /// daylight calibration that was matched to Blender intact. Returns
    /// `(enabled, sun direction, sun radiance, environment)`.
    pub fn resolve_env(&self) -> (bool, Vec3, [f32; 3], crate::env::EnvRender) {
        let (enabled, dir, sun, sky_col, ground_col) = self.resolve();
        let mut sky = crate::env::Sky::new(dir, self.turbidity);
        sky.calibrate(sky_col, ground_col, sun);
        let sh = if enabled && sky.valid {
            sky.sh9()
        } else {
            // No sun (night, or daylight switched off): fall back to the flat ambient rather than a
            // model whose normalisation has collapsed. Encoded as the l=0 term alone, which IS a
            // uniform environment — `sh_ambient` then returns `sky_col` for every normal.
            let mut sh = [[0.0f32; 3]; 9];
            for c in 0..3 {
                sh[0][c] = sky_col[c] * std::f32::consts::PI / 0.886_227;
            }
            sh
        };
        let env = crate::env::EnvRender {
            sky: if enabled { Some(sky) } else { None },
            sh,
            ao: self.ao,
            gi: self.gi,
            refract: self.refract,
            ssr: self.ssr,
            backdrop: if enabled && self.sky_backdrop { crate::env::Backdrop::Sky } else { crate::env::Backdrop::Studio },
            reflections: self.reflections,
            // Filled in by the caller that owns the loaded map — `SunEnv` describes the SUN, and an
            // HDRI belongs to the scene, not to a date and a latitude.
            hdri: None,
            sun_angle_deg: self.sun_angle_deg.max(0.0),
            fog: self.fog,
        };
        (enabled, dir, sun, env)
    }

    /// Resolve the current settings to a directional light: the sun direction (via [`crate::solar`])
    /// plus a warm direct term and a HEMISPHERIC ambient (cool sky from above + warm ground bounce
    /// from below), all dimming as the sun nears/goes below the horizon. Returns
    /// `(enabled, dir, sun_rgb, sky_rgb, ground_rgb)` ready for [`set_sun_light`].
    pub fn resolve(&self) -> (bool, Vec3, [f32; 3], [f32; 3], [f32; 3]) {
        let doy = crate::solar::day_of_year(self.month, self.day);
        let p = crate::solar::sun_position(self.lat_deg, self.lon_deg, self.utc_offset, doy, self.hour, self.north_offset_deg);
        // Daylight factor f = sin(altitude): 0 at the horizon, 1 with the sun overhead.
        let f = p.dir.z.max(0.0).clamp(0.0, 1.0);
        // Direct sun: amber + dim near the horizon, bright near-WHITE when high (a midday sun is not
        // yellow). These are HDR radiances — values exceed 1 on purpose; the shaders + CPU shade
        // tone-map (1 − e⁻ˣ) so highlights roll off to a photographic bright-day look instead of
        // clipping to flat white.
        // Whiten SLOWLY. This used to reach white by ~20° of altitude, so every daytime sun was a
        // neutral white light — and a white sun against a near-white sky ambient leaves an image
        // with no warm/cool separation at all, which is a large part of why our renders read as
        // flat where a reference render reads as sunlight. Calibrated against the villa scene's
        // own Blender sun, which is (1.0, 0.745, 0.48) at 34° of altitude; this ramp gives
        // (1.0, 0.75, 0.55) there, and still lands only slightly warm at noon — a midday sun is
        // about 5500 K, not 6500 K.
        let w = smoothstep(0.05, 0.90, f);
        let warm = [1.0, lerp(0.42, 0.93, w), lerp(0.12, 0.86, w)];
        let direct = self.intensity * (0.35 + 2.0 * f); // strong at noon
        let sun = [warm[0] * direct, warm[1] * direct, warm[2] * direct];
        // HEMISPHERIC ambient — the biggest thing missing before. Real daylight is not a flat fill:
        // the SKY lights everything from above and the sunlit GROUND bounces warm light back up (fake
        // GI). Blender's convincing render gets this from a bright sky dome + ray-traced bounce; we
        // approximate it with two colours the shaders/CPU blend by surface normal. Both scale with
        // `intensity`, so the brightness slider lifts the shadow side too — previously only the direct
        // term scaled, so a façade in shade stayed dusk-dark at any slider value.
        let sky_i = self.intensity * (0.35 + 0.95 * f); // cool skylight from ABOVE (was ~2× dimmer)
        let sky = [0.85 * sky_i + 0.04, 0.92 * sky_i + 0.05, 1.05 * sky_i + 0.07];
        let gnd_i = self.intensity * (0.15 + 0.80 * f); // warm ground BOUNCE from below
        let ground = [0.90 * gnd_i + 0.03, 0.85 * gnd_i + 0.03, 0.75 * gnd_i + 0.03];
        (self.enabled, p.dir, sun, sky, ground)
    }
}

/// The RESOLVED directional light that the CPU shaders read — recomputed from [`SunEnv`] by the app
/// each frame and pushed via [`set_sun_light`]. `Copy` so the thread-local read is a cheap value.
#[derive(Clone, Copy)]
struct SunLightRaw {
    enabled: bool,
    dir: Vec3,
    sun: [f32; 3], // direct term (already intensity-scaled), calibrated as irradiance/π
    /// The SKY's irradiance as 9 spherical-harmonic coefficients (see [`crate::env`]). This
    /// replaced a pair of colours lerped by `normal.z`: the ambient a surface receives now depends
    /// on where in the sky the light actually is, so two walls facing different ways differ.
    sh: [[f32; 3]; 9],
}

impl Default for SunLightRaw {
    fn default() -> Self {
        Self { enabled: false, dir: Vec3::new(0.35, 0.25, 0.9).normalize(), sun: [1.0, 0.96, 0.88], sh: [[0.0; 3]; 9] }
    }
}

thread_local! {
    /// The active sun light for shading on THIS (render) thread. Buffer building is single-threaded
    /// on the main thread, so a thread-local is both correct and lock-free per vertex.
    static SUN: std::cell::Cell<SunLightRaw> = const { std::cell::Cell::new(SunLightRaw { enabled: false, dir: Vec3::new(0.0, 0.0, 1.0), sun: [1.0, 0.96, 0.88], sh: [[0.0; 3]; 9] }) };
}

/// Push the resolved sun light (call before building any shaded buffers). `dir` points TO the sun,
/// `sh` is the sky's irradiance projection from [`crate::env::Sky::sh9`].
pub fn set_sun_light(enabled: bool, dir: Vec3, sun: [f32; 3], sh: [[f32; 3]; 9]) {
    SUN.with(|c| c.set(SunLightRaw { enabled, dir: dir.normalize_or_zero(), sun, sh }));
}

fn sun_light() -> SunLightRaw {
    SUN.with(|c| c.get())
}

thread_local! {
    /// CLAY mode: override every surface to a flat neutral grey (glass keeps its transparency) so
    /// light/shadow/bounce can be judged without material colour muddying it — the villa build's
    /// "light-meter". Set on the render thread before building buffers, like [`set_sun_light`].
    static CLAY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The neutral clay grey (matte).
pub const CLAY_GREY: [f32; 3] = [0.62, 0.62, 0.62];

/// Enable/disable clay mode for shading on this (render) thread.
pub fn set_clay(on: bool) {
    CLAY.with(|c| c.set(on));
}

fn clay_on() -> bool {
    CLAY.with(|c| c.get())
}

/// Linear interpolate a→b by t.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// GLSL-style smoothstep: 0 below `e0`, 1 above `e1`, a smooth Hermite ramp between.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One baked vertex colour: **scene-referred linear light**, plus the fraction of it that arrived
/// as ambient.
///
/// Both halves are new. The colour used to be display-referred — the material's sRGB bytes times a
/// shading factor, squashed by `1 − e⁻ˣ`. Once Phase 1 moved the view transform to the composite,
/// that made two tone-maps in series on every flat-shaded surface, which is exactly the mistake
/// Phase 1 removed from the *textured* path. Now the light stays linear all the way to the
/// composite, and the display transform runs once. The ambient fraction is what lets screen-space
/// occlusion darken only the part of the light that a crease actually blocks.
/// Shading modes the fragment shader reproduces. Carried per vertex so it can tell a lit surface
/// from a UI swatch, and a wall from a piece of furniture.
pub const SHADE_UI: f32 = 0.0;
pub const SHADE_SCENE: f32 = 1.0;
pub const SHADE_FURNITURE: f32 = 2.0;

#[derive(Clone, Copy, Debug)]
struct Baked {
    /// The lit colour. Still computed on the CPU, but no longer what reaches the GPU: it is the
    /// TWIN the fragment shader's lighting is tested against, and what the CPU-side consumers
    /// (the translucent `V3A` pass, the tests) read.
    col: [f32; 3],
    amb: f32,
    /// What the vertex buffer actually carries now — the surface's own colour and normal, with the
    /// lighting applied per FRAGMENT instead of baked in here.
    ///
    /// Baking meant a vertex buffer that had to be rebuilt whenever the light moved (the hour
    /// slider re-baking 1.86 M vertices was exactly this), and it made any per-frame variation in
    /// the lighting — jittered soft shadows, accumulation — impossible by construction. It also
    /// blocks instancing: one mesh drawn at twenty places cannot carry twenty bakes.
    albedo: [f32; 3],
    n: Vec3,
    mode: f32,
}

impl Baked {
    /// Split `ambient + direct` into a colour and the ambient's share of it, measured on luminance
    /// (a per-channel share would tint the occluded parts).
    fn split(ambient: [f32; 3], direct: [f32; 3]) -> Self {
        let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let col = [ambient[0] + direct[0], ambient[1] + direct[1], ambient[2] + direct[2]];
        let total = lum(col);
        Self {
            col,
            amb: if total > 1e-6 { (lum(ambient) / total).clamp(0.0, 1.0) } else { 1.0 },
            albedo: [0.0; 3],
            n: Vec3::ZERO,
            mode: SHADE_UI,
        }
    }

    /// Attach what the shader needs: the surface's authored colour, its normal, and which
    /// lighting response to use.
    fn surface(mut self, albedo: [f32; 3], n: Vec3, mode: f32) -> Self {
        self.albedo = albedo;
        self.n = n;
        self.mode = mode;
        self
    }
}

/// The sky's irradiance at a surface with normal `n`, divided by π — a direct multiplier on albedo.
/// This is the real integral of the [`crate::env`] sky over the hemisphere, so an underside sees the
/// ground, a north wall sees the dim half of the dome, and a surface facing the sun's side of the
/// sky sees the aureole. The two-colour `mix(ground, sky, n.z)` fill it replaced could express none
/// of that. Must match the shader's `sh_ambient`.
fn sky_ambient(s: &SunLightRaw, n: Vec3) -> [f32; 3] {
    crate::env::sh_ambient(&s.sh, n)
}

/// Fixed key light, matching `light3d`'s shading so the two 3D views look alike. When the sun is
/// enabled this instead lights by the real sun direction plus the sky's own irradiance, so the lit
/// and shadowed sides read as daylight and rotate as the sun moves.
fn shade(base: [f32; 3], n: Vec3) -> Baked {
    let base = if clay_on() { CLAY_GREY } else { base };
    // The AUTHORED colour, kept for the vertex buffer — the shader decodes sRGB itself, exactly as
    // the `SRGB8_ALPHA8` upload does for image textures.
    let authored = base;
    let tag = |b: Baked| b.surface(authored, n, SHADE_SCENE);
    // Authored colours are sRGB; light is linear. Decode ONCE, here — this is the CPU twin of the
    // `SRGB8_ALPHA8` upload that decodes image textures for free.
    let base = crate::color::srgb_to_linear3(base);
    let s = sun_light();
    if !s.enabled {
        // The studio key: a constant 0.35 fill plus a 0.65 directional term. The shader reproduces
        // exactly this split for textured surfaces (see `STUDIO_DIR` in TEX_FS).
        let dir = Vec3::new(0.35, 0.25, 0.9).normalize();
        let k = 0.65 * n.dot(dir).abs();
        return tag(Baked::split(
            [base[0] * 0.35, base[1] * 0.35, base[2] * 0.35],
            [base[0] * k, base[1] * k, base[2] * k],
        ));
    }
    let a = sky_ambient(&s, n);
    let lit = n.dot(s.dir).max(0.0);
    tag(Baked::split(
        [base[0] * a[0], base[1] * a[1], base[2] * a[2]],
        [base[0] * s.sun[0] * lit, base[1] * s.sun[1] * lit, base[2] * s.sun[2] * lit],
    ))
}

/// Brighter shading for imported furniture — a higher ambient floor (0.6) so meshes read
/// clearly instead of coming out murky-dark, which is how many imports looked.
fn shade_furniture(base: [f32; 3], n: Vec3) -> Baked {
    let base = if clay_on() { CLAY_GREY } else { base };
    let authored = base;
    let tag = |b: Baked| b.surface(authored, n, SHADE_FURNITURE);
    let base = crate::color::srgb_to_linear3(base);
    let s = sun_light();
    if !s.enabled {
        let dir = Vec3::new(0.35, 0.25, 0.9).normalize();
        let k = 0.4 * n.dot(dir).abs();
        return tag(Baked::split(
            [base[0] * 0.6, base[1] * 0.6, base[2] * 0.6],
            [base[0] * k, base[1] * k, base[2] * k],
        ));
    }
    // A touch more fill than the scene so furniture never reads murky.
    let a = sky_ambient(&s, n);
    let lit = n.dot(s.dir).max(0.0);
    let extra = 0.05;
    tag(Baked::split(
        [base[0] * (a[0] + extra), base[1] * (a[1] + extra), base[2] * (a[2] + extra)],
        [base[0] * s.sun[0] * lit, base[1] * s.sun[1] * lit, base[2] * s.sun[2] * lit],
    ))
}

/// Scalar lighting for TEXTURED surfaces — the `TexVtx.s` that multiplies the sampled image/procedural
/// colour. Sun-aware when enabled (the luminance of the directional response), else the fixed studio
/// scalar. `furniture` selects the brighter ambient floor. Keeps textured/procedural surfaces (e.g.
/// the cabin's oak) responding to the sun alongside flat-coloured ones.
///
/// Linear, and no tone-map: like [`shade`], this feeds a buffer the composite grades once.
fn shade_scalar(n: Vec3, furniture: bool) -> f32 {
    let s = sun_light();
    if !s.enabled {
        let dir = Vec3::new(0.35, 0.25, 0.9).normalize();
        return if furniture { 0.6 + 0.4 * n.dot(dir).abs() } else { 0.35 + 0.65 * n.dot(dir).abs() };
    }
    let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let lit = n.dot(s.dir).max(0.0);
    let amb = if furniture { 0.05 } else { 0.0 };
    lum(sky_ambient(&s, n)) + amb + lum(s.sun) * lit
}

fn v(p: Vec3, b: Baked) -> V3 {
    // The ALBEDO and the normal, not the lit colour — the fragment shader lights it. A UI swatch
    // has `mode = SHADE_UI` and a zero normal, and the shader passes its colour through untouched.
    V3 {
        x: p.x,
        y: p.y,
        z: p.z,
        r: b.albedo[0],
        g: b.albedo[1],
        b: b.albedo[2],
        nx: b.n.x,
        ny: b.n.y,
        nz: b.n.z,
        mode: b.mode,
    }
}

/// A UI colour for the overlay / line passes — a swatch, not a lit surface. It stays in **authored
/// sRGB** (those passes set `u_linearize = 1` and decode it in the shader) and carries no ambient
/// share, because a selection tint is not something ambient occlusion has any business dimming.
fn ui(c: [f32; 3]) -> Baked {
    Baked { col: c, amb: 0.0, albedo: c, n: Vec3::ZERO, mode: SHADE_UI }
}

/// Whether the triangle whose first vertex is `base` is see-through — any of its three
/// vertices carries below-opaque alpha. Used to sort furniture triangles into the solid
/// pass vs. the blended transparent pass.
fn tri_is_translucent(asset: &FurnitureAsset, base: usize) -> bool {
    asset.vertex_alpha(base) < ALPHA_OPAQUE
        || asset.vertex_alpha(base + 1) < ALPHA_OPAQUE
        || asset.vertex_alpha(base + 2) < ALPHA_OPAQUE
}

/// Even–odd ray-cast point-in-polygon test in XY. Used to tell whether a ceiling triangle
/// sits over the OPEN room interior (a floor footprint) — where it is hidden — or over the
/// surrounding wall, where it is kept.
fn point_in_poly(poly: &[Vec2], x: f32, y: f32) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i].x, poly[i].y);
        let (xj, yj) = (poly[j].x, poly[j].y);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The 8 corners of an AABB, bit order x=1, y=2, z=4 (same as the sandbox's `corners_of`).
fn corners_of(mn: Vec3, mx: Vec3) -> [Vec3; 8] {
    let mut o = [Vec3::ZERO; 8];
    for (i, slot) in o.iter_mut().enumerate() {
        *slot = Vec3::new(
            if i & 1 == 0 { mn.x } else { mx.x },
            if i & 2 == 0 { mn.y } else { mx.y },
            if i & 4 == 0 { mn.z } else { mx.z },
        );
    }
    o
}

fn seg(out: &mut Vec<V3>, a: Vec3, b: Vec3, c: [f32; 3]) {
    out.push(v(a, ui(c)));
    out.push(v(b, ui(c)));
}

/// The 12 edges of an AABB.
fn aabb_lines(out: &mut Vec<V3>, mn: Vec3, mx: Vec3, c: [f32; 3]) {
    let k = corners_of(mn, mx);
    // pairs differing by exactly one bit = the 12 edges
    for i in 0..8usize {
        for bit in [1usize, 2, 4] {
            let j = i | bit;
            if j != i {
                seg(out, k[i], k[j], c);
            }
        }
    }
}

/// Cache backing [`FactoryState::opaque_verts`]: the last-built opaque render buffer
/// (CSG solids + posed furniture) and a cheap signature of the inputs that produced it.
/// The buffer is an `Arc` so a paint callback can hold a reference-counted handle to the
/// exact same vertices with no per-frame copy.
#[derive(Default)]
struct RenderCache {
    sig: u64,
    ready: bool,
    verts: std::sync::Arc<Vec<V3>>,
}

/// The cached outline of a targeted face/piece, in the asset's LOCAL space.
struct FaceOutline {
    inst: usize,
    /// Which asset, and how many triangles it had — a re-import or a rebuild under the same
    /// instance must not leave the outline tracing geometry that no longer exists.
    asset: usize,
    tris: usize,
    groups: Vec<u32>,
    /// Boundary edges only — see [`FactoryState::furniture_face_highlight_segments`].
    edges: Vec<[Vec3; 2]>,
    /// True when the boundary hit [`MAX_OUTLINE_EDGES`] and was cut short.
    truncated: bool,
}

/// A hard ceiling on the outline, so a highlight can never again cost more than a few milliseconds
/// a frame no matter what is selected. A real coplanar face boundary is hundreds of edges; this is
/// two orders of magnitude above that, and exists only so the worst case stays bounded.
const MAX_OUTLINE_EDGES: usize = 20_000;

/// 3D Factory state — the model + its view. Lives on `CadApp` as one field.
pub struct FactoryState {
    pub open: bool,
    pub model: Model,
    /// Evaluated CSG mesh, rebuilt only when `dirty` (csgrs is not cheap).
    pub cached: SolidMesh,
    pub dirty: bool,
    pub selection: Vec<u32>,
    /// GROUPS of CSG features: `feature_id → group_id`. Members of a group select / move / delete
    /// as one entity; "Explode" clears the tag. A feature is in at most one group. Furniture is not
    /// grouped here (it is single-select). Persisted + snapshotted for undo.
    pub feature_group: std::collections::HashMap<u32, u32>,
    /// Next group id to hand out (monotonic; never reused so undo/redo stay unambiguous).
    pub next_group_id: u32,

    // orbit camera — `cam_target` is STORED, never recomputed from bounds each frame,
    // so the view does not jump when a solid is added or moved (sandbox lesson).
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    pub cam_dist: f32,
    pub cam_target: [f32; 3],
    /// Parallel (orthographic) projection — TRUE after a standard-view snap (Top/Front/…/
    /// Iso) so a cylinder reads as a true CIRCLE in Top (no perspective barrel); FALSE while
    /// free-orbiting (perspective depth). CAD convention: standard views are orthographic.
    pub ortho: bool,

    /// Live sketch-on-plane session (the app's `doc` is swapped while `Some`).
    pub session: Option<SketchSession>,
    /// Face picked by the last right-click — what the context menu acts on.
    pub pending_face: Option<Frame>,
    /// While sketching ON a face, the 3D object's feature edges projected onto the sketch
    /// plane (in that frame's u,v). Drawn faintly in the 2D canvas so it is a real 2D VIEW
    /// of the object instead of a blank void. Empty when not sketching on a face.
    pub sketch_ref: Vec<[Vec2; 2]>,

    pub box_w: f32,
    pub box_d: f32,
    pub box_h: f32,
    pub cyl_r: f32,
    pub cyl_h: f32,
    pub cyl_sides: u32,

    /// DRAW3D: the open primitive dialog (`None` = closed). The dialog OWNS the
    /// live parameters, so tweaking them costs nothing until Create is pressed —
    /// csgrs walks a BSP per boolean, so we never re-evaluate per keystroke.
    pub draw3d: Option<Draw3dDialog>,

    /// DRAW3D edit-binding: when exactly one solid is selected, the dialog's
    /// controllers edit THAT feature live. This holds the id currently bound, so the
    /// dialog reloads its fields only when the selection changes — not every frame,
    /// which would stomp the user's edits mid-drag.
    pub draw3d_edit: Option<u32>,
    /// A primitive built in the Draw3D dialog and awaiting a placement CLICK in the 3D
    /// view — created at the picked point (a Box's corner / everything else centred),
    /// not at the origin. `None` = nothing waiting to be placed.
    pub place_pending: Option<Primitive>,

    /// 3D wall extrusion height — the ONE thing a 2D wall lacks. A promoted wall keeps
    /// its own (per-wall) thickness and rises to this height. Kept in the 3D layer, NOT
    /// cad_kernel's `WallStyle` (that's CORE, shared with the 2D app / RUST_CAD).
    pub wall_height: f32,
    /// Thickness given to promoted geometry that carries NONE of its own — a line,
    /// polyline or arc, which is what an imported or traced plan consists of. A real
    /// `Geom::Wall` still uses its own thickness.
    ///
    /// Lives here beside `wall_height` so the pair is set in one place. It previously
    /// came from the 2D wall style, which meant the 3D view had no thickness control at
    /// all — you had to open the Wall Style Manager to change it.
    pub wall_thickness: f32,
    /// Live wall records — every promoted wall, so its height stays editable after the
    /// fact (the "walls are alive" requirement). Keyed to model features by `feature_id`.
    pub walls: Vec<WallInst>,

    /// The building's levels, bottom-up. NEVER empty — a building always has at least one
    /// storey, so `active_storey` always indexes something real.
    pub storeys: Vec<Storey>,
    /// Which storey new geometry is built on. Always a valid index into `storeys`.
    pub active_storey: usize,

    /// Vertex handle being dragged: `(wall index, vertex index)`. `None` = not dragging.
    /// Held across frames because a drag spans many, and the wall's feature ids change
    /// under it on every step (`rederive_wall`) — the WALL index is stable, the ids are not.
    pub wall_drag: Option<(usize, usize)>,

    /// Move-gizmo handle being dragged, plus the anchor for absolute (Free) dragging: the
    /// selection centre and the ground point grabbed when the drag began. `None` = idle.
    pub gizmo_drag: Option<GizmoHandle>,
    pub gizmo_grab_ground: Option<Vec3>,
    pub gizmo_start_center: Vec3,

    /// Which manipulation the gizmo performs (Move arms vs Rotate rings).
    pub gizmo_mode: GizmoMode,
    /// In-progress rotation-ring drag (`None` = idle).
    pub rot_drag: Option<RotDrag>,

    /// True while a dimension field in the properties panel is mid-interaction, so the
    /// whole drag/type is ONE undo step rather than one per keystroke.
    pub dim_edit_active: bool,

    /// Show the 2D drawing (the plan) as a ground-plane underlay in the 3D view — so you
    /// can see the plan you are building the 3D model from. Toggled from the panel toolbar.
    pub show_plan: bool,

    /// Feature ids that are ROOM CEILINGS — separate slab objects created by the room tool.
    /// Tracked so they can be hidden as a group without deleting them; the lighting model
    /// still contains them.
    pub ceilings: std::collections::HashSet<u32>,
    /// Feature ids DETECTED as ceiling/roof caps by GEOMETRY (a thin, horizontal slab that
    /// is the topmost cap of the model). Recomputed on every [`Self::recompute`]. This is
    /// the drift-proof backstop for [`Self::hide_ceilings`]: the hand-tracked `ceilings`
    /// set can go stale (feature ids are `max+1` and get reused across delete/undo), and a
    /// stale id hides NOTHING — the exact field failure. Geometry cannot drift, and it only
    /// ever matches a flat top cap, so walls are never sliced.
    pub ceiling_caps: std::collections::HashSet<u32>,
    /// SOLID building roofs to CLIP while hiding: `feature id → cut z`. The feature's
    /// triangles at/above the cut (its roof) are dropped; everything below (its walls) is
    /// kept, so a solid building you made over a room opens at the top instead of vanishing.
    /// Hide ceilings in the RENDER only, so you can see into rooms while the ceilings (and
    /// the lighting model) stay intact. Unlike a section cut, this hides ONLY the ceiling
    /// slabs — the surrounding roof and walls stay.
    pub hide_ceilings: bool,
    /// Cutaway (horizontal section) — hide everything above `cutaway_z` in the render.
    /// Geometric, so it ALWAYS works: it does not depend on any object being tagged a
    /// ceiling. VIEW ONLY; the model is untouched.
    pub cutaway: bool,
    pub cutaway_z: f32,
    /// Ceiling slab thickness, metres.
    pub ceiling_thickness: f32,

    /// Rubber-band box-select in progress: `(start, current)` screen points. `None` = idle.
    pub marquee: Option<(egui::Pos2, egui::Pos2)>,

    /// Imported furniture MESHES (the project library) + their PLACED instances. The
    /// library is stored in the project file so furniture can be reused later.
    pub furniture_lib: Vec<FurnitureAsset>,
    pub furniture: Vec<FurnitureInst>,
    /// The selected furniture instances, in pick order. Mutually exclusive with the CSG feature
    /// selection — selecting furniture clears the feature selection and vice versa.
    ///
    /// The FIRST entry is the PRIMARY: the one whose properties the panel edits and whose
    /// parameters the editors read. Operations that are meaningful on a set — move, rotate,
    /// delete, recolour — act on all of them; operations that need one set of numbers act on the
    /// primary. Use [`Self::sel_furn_one`] where a multi-selection should not be edited at all.
    pub sel_furniture: Vec<usize>,

    /// Guided PATH-SWEEP pick, if one is running (see [`SweepPick`]). While `Some`, a click in
    /// the 3D view designates the cross-section then the path instead of selecting.
    pub sweep_flow: Option<SweepFlow>,

    /// Cached opaque render buffer (solids + posed furniture) behind a cheap signature,
    /// so orbiting past a heavy imported mesh doesn't re-transform every vertex each frame
    /// (that was the post-import lag). See [`Self::opaque_verts`].
    render_cache: std::cell::RefCell<RenderCache>,
    /// Per-instance memo of [`Self::furniture_faceted`]: index → (cheap signature, built split).
    /// Rebuilding the per-surface TexVtx buckets of a heavy multi-material import (e.g. an 800k-tri
    /// glTF) every frame cost ~600 ms; this returns the cached `Arc` until the assignment or a
    /// referenced texture actually changes. Pose is irrelevant (the split is in LOCAL space).
    faceted_cache: std::cell::RefCell<
        std::collections::HashMap<usize, (u64, std::sync::Arc<FacetedFurniture>)>,
    >,
    /// A loaded HDR environment, and everything derived from it. `None` ⇒ the analytic sky.
    ///
    /// The derived parts are kept beside the map because they are expensive: the SH projection and
    /// the GGX prefilter run once when the file is loaded, never per frame. `env_version` changes
    /// whenever the map does, which is how the renderer tells "same environment" from "new one"
    /// without comparing megabytes of pixels.
    pub env_map: Option<std::sync::Arc<crate::env_map::EnvMap>>,
    pub env_chain: Vec<crate::env_map::EnvMip>,
    pub env_sh: [[f32; 3]; 9],
    /// Multiplies the environment's radiance; 1.0 = as photographed.
    pub env_strength: f32,
    /// Yaw about Z, degrees — turn the world without moving the model.
    pub env_rot_deg: f32,
    pub env_version: u64,

    /// Memo of the targeted face/piece OUTLINE, in the asset's LOCAL space — see
    /// [`Self::furniture_face_highlight_segments`], which explains why this exists. Local, so the
    /// pose is applied per frame and dragging the object never rebuilds it.
    face_outline: std::cell::RefCell<Option<FaceOutline>>,
    /// Bumped on every [`Self::recompute`]; lets the render cache notice a geometry change
    /// without hashing the (large) triangle buffer.
    pub geom_version: u64,

    /// Daylight (sun) settings — where/when the building is. Off by default (studio key light).
    /// When changed, the render caches re-bake so shading follows the sun (see [`Self::opaque_sig`]
    /// and [`Self::furniture_key`], which fold it in).
    pub sun: SunEnv,

    /// COLOUR MANAGEMENT — how scene-referred linear light becomes pixels (Blender's Color
    /// Management panel). Purely a display decision: it never touches the geometry or the render
    /// caches, so changing it costs one frame and nothing re-bakes.
    pub color: crate::color::ColorPipeline,

    /// TEMPORAL ACCUMULATION — how many sub-pixel-jittered samples a STILL frame is refined over
    /// before the viewport settles. 0 or 1 = off (one sample, exactly as before). It costs that
    /// many ordinary frames after every change and nothing at all once converged: the renderer
    /// then re-presents the finished buffer instead of redrawing the scene.
    pub taa_samples: u32,

    /// CLAY mode — flatten every material to neutral grey (glass keeps transparency) for a
    /// light-only study. Folded into the render-cache signatures so toggling re-bakes.
    pub clay_mode: bool,

    /// Per-feature colour (Textures menu): CSG feature id → linear RGB. A feature with no
    /// entry renders in the default neutral. Furniture carries its colour on the instance.
    pub feature_color: std::collections::HashMap<u32, [f32; 3]>,
    /// Per-SURFACE colour: a flat face (a body's feature id + its world plane) → RGB. Lets
    /// the user paint one wall face rather than the whole solid. Takes priority over
    /// `feature_color` when a triangle's surface has an entry.
    pub surface_color: std::collections::HashMap<SurfaceKey, [f32; 3]>,
    /// When on, clicking a face in the 3D view PAINTS that surface with the palette colour
    /// instead of selecting the object.
    pub paint_surface_mode: bool,
    /// Last colour chosen in the Textures picker, so it persists across opens of the menu.
    pub last_pick_color: [f32; 3],

    /// Bitmaps pasted from the clipboard, available to texture objects. Not persisted to
    /// the sidecar yet (image blobs are large) — a re-paste restores them. See
    /// [`TextureAsset`] and [`Self::add_texture`].
    pub textures: Vec<TextureAsset>,
    /// CSG feature id → index into [`Self::textures`]. Furniture carries its own texture
    /// index on the instance; this is the equivalent for built solids and walls.
    pub feature_texture: std::collections::HashMap<u32, usize>,
    /// Per-SURFACE texture: one flat face → texture index. Lets each wall face carry its OWN
    /// image instead of the whole solid sharing one. Takes priority over `feature_texture`.
    pub surface_texture: std::collections::HashMap<SurfaceKey, usize>,
    /// The texture "brush" for paint-single-surface mode: while set, clicking a face applies
    /// this texture to just that surface (set when a texture is applied with paint-mode on).
    pub surface_tex_brush: Option<usize>,
    /// How an applied texture lands on the SELECTED FURNITURE object: the whole object, one
    /// clicked flat face, or one clicked connected piece. See [`FurnPaintMode`].
    pub furn_paint_mode: FurnPaintMode,
    /// The armed furniture texture brush: while set (and `furn_paint_mode` is Face/Piece), the
    /// next click on the selected object textures the clicked face/piece with this texture.
    pub furn_tex_brush: Option<usize>,
    /// The face/piece the user clicked on the selected object (instance index + its face-group
    /// ids), in Face/Piece mode. The NEXT texture applied lands here — the "click a face, then pick
    /// a texture" order. Cleared when the mode returns to Whole object or another object is picked.
    pub furn_face_sel: Option<(usize, Vec<u32>)>,

    /// Copy/paste buffer for 3D objects (Ctrl+C / Ctrl+V) — one furniture instance or one
    /// CSG feature, captured with its colour/texture so a paste is a faithful clone.
    pub clip: Option<FactoryClip>,

    /// BUILDING section — the storey height the structure rises to, in metres. Held on
    /// the state (not in the dialog) because it is a property of the BUILDING, so it
    /// persists across elements: set it once and every element the section opens starts
    /// at that height, the way `wall_height` already works for promoted walls.
    pub building_height: f32,

    /// ROOM properties. `room_height` is the CLEAR interior height of a carved room;
    /// `room_floor` is the slab thickness left BELOW it. A room is carved on the active
    /// storey as `[base + room_floor, base + room_floor + room_height]`, so every storey
    /// keeps its own floor — no more "thin film on storey 1, no floor above".
    pub room_height: f32,
    pub room_floor: f32,
    /// Open the room to the sky (no ceiling). Default OFF — a room has a ceiling, which is
    /// what a lighting calculation needs; turn this on only for a court/atrium open above.
    pub room_open_top: bool,

    /// Height (m) a drawn face-sketch is extruded by for Room-elements / Furniture extrude,
    /// and the depth of a Cut RECESS (a cut that stops short of going all the way through).
    pub element_height: f32,
    /// Keep the drawn shape after Extrude / Cut instead of consuming it, so you can extrude
    /// and cut the SAME outline (e.g. a recessed frame around a hole) without redrawing.
    pub keep_sketch: bool,

    /// Zoom, mirroring the 2D command. `zoom`/`z` arms `RealTime` (drag to dolly) and
    /// shows the choice menu; typing `w` switches to `Window` (a left drag rubber-bands a
    /// box that reframes on release). `zoom_drag`/`zoom_cur` are the live box corners.
    pub zoom_mode: ZoomMode,
    pub zoom_drag: Option<egui::Pos2>,
    pub zoom_cur: Option<egui::Pos2>,
    /// Camera snapshot before the last zoom, for `zoom previous`: (yaw,pitch,dist,tx,ty,tz).
    pub cam_prev: Option<[f32; 6]>,
    /// Screen-zoom status captured at the start of a real-time drag, for the recorder.
    pub zoom_rt_before: Option<String>,

    /// An in-flight 3D modifier over `selection`. This is the SAME `move` command as
    /// 2D — only the objects and the algorithm differ ("check 2d or 3d, take the right
    /// move in the background"). `cad_solid::modify` is spec-conformant + unit-tested.
    pub modify: Option<cad_solid::modify::Modify>,
    /// A 3D op waiting on its selection — the 3D twin of the app's `queued_op`.
    /// `move` with nothing picked → queue it, gather, Enter dispatches into the picks.
    pub queued: Option<cad_solid::modify::ModifyOp>,
    /// Live prompt for the running/queued 3D op.
    pub status: String,
    /// True while a cutout is open for 2D reshape (via `factory_edit_cutout`). Drives the
    /// prominent "drag the points, then Apply" banner in the sketch panel and makes finishing
    /// the sketch re-cut the opening automatically. Cleared when the sketch is exited/applied.
    pub editing_cutout: bool,
    /// Cached library index of the default DOOR / WINDOW aperture mesh, once loaded from
    /// `assets/apertures/`. `[door, window]`. `None` until first used, so the bundled meshes are
    /// only parsed on demand and reused across every aperture placed.
    pub aperture_asset: [Option<usize>; 2],
    /// The selected features' own mesh + the selection it was built from (the cache
    /// key). Rebuilt only when the selection changes — never per frame.
    sel_mesh: SolidMesh,
    sel_key: Vec<u32>,
}

/// CARD cardinal lock on a WORLD delta: collapse the in-plane part to its dominant
/// axis, preserving the out-of-plane component (the 3D reading of the 2D H/V lock —
/// same rule `cad_solid::modify` applies internally).
fn card_lock_world(d: Vec3) -> Vec3 {
    if d.x.abs() >= d.y.abs() {
        Vec3::new(d.x, 0.0, d.z)
    } else {
        Vec3::new(0.0, d.y, d.z)
    }
}

/// Which primitive the Draw3D dialog is editing. One entry per menu item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Draw3dKind {
    Box,
    Sphere,
    Cylinder,
    Cone,
    Prism,
    Pyramid,
    Capsule,
    Torus,
    Tube,
    Ellipsoid,
}

impl Draw3dKind {
    /// Menu order — the owner's "basic 3D objects" list, minus the two that are
    /// NOT solids (Plane/Quad and Disk/Circle are 2D: that is what the sketch +
    /// plane system is for, not a CSG primitive).
    pub const ALL: [Draw3dKind; 10] = [
        Draw3dKind::Box,
        Draw3dKind::Sphere,
        Draw3dKind::Cylinder,
        Draw3dKind::Cone,
        Draw3dKind::Prism,
        Draw3dKind::Pyramid,
        Draw3dKind::Capsule,
        Draw3dKind::Torus,
        Draw3dKind::Tube,
        Draw3dKind::Ellipsoid,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Draw3dKind::Box => "Box / Cuboid",
            Draw3dKind::Sphere => "Sphere",
            Draw3dKind::Cylinder => "Cylinder",
            Draw3dKind::Cone => "Cone / Frustum",
            Draw3dKind::Prism => "Prism",
            Draw3dKind::Pyramid => "Pyramid",
            Draw3dKind::Capsule => "Capsule",
            Draw3dKind::Torus => "Torus",
            Draw3dKind::Tube => "Tube (hollow)",
            Draw3dKind::Ellipsoid => "Ellipsoid",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Draw3dKind::Box => "⬛",
            Draw3dKind::Sphere => "⬤",
            Draw3dKind::Cylinder => "⬮",
            Draw3dKind::Cone => "▲",
            Draw3dKind::Prism => "⬡",
            Draw3dKind::Pyramid => "◭",
            Draw3dKind::Capsule => "⬭",
            Draw3dKind::Torus => "◎",
            Draw3dKind::Tube => "◯",
            Draw3dKind::Ellipsoid => "⬯",
        }
    }
}

/// Render editable numeric fields for a primitive's DIMENSIONS (not its position — that is
/// the feature's placement). Returns `true` if any field changed. Type-in enabled: these
/// are `DragValue`s, so the user can click and type a number.
///
/// The Extrusion's outline is a stored profile, not a set of scalars, so only its HEIGHT
/// is editable here — the shape is fixed once drawn.
pub fn primitive_dim_fields(ui: &mut egui::Ui, p: &mut Primitive) -> bool {
    fn f(ui: &mut egui::Ui, label: &str, v: &mut f32, min: f32) -> bool {
        ui.horizontal(|ui| {
            ui.add_sized([64.0, 18.0], egui::Label::new(egui::RichText::new(label).small().weak()));
            ui.add(egui::DragValue::new(v).speed(0.02).range(min..=1e5).suffix(" m")).changed()
        })
        .inner
    }
    fn u(ui: &mut egui::Ui, label: &str, v: &mut u32, min: u32) -> bool {
        ui.horizontal(|ui| {
            ui.add_sized([64.0, 18.0], egui::Label::new(egui::RichText::new(label).small().weak()));
            ui.add(egui::DragValue::new(v).speed(1.0).range(min..=512)).changed()
        })
        .inner
    }
    let mut c = false;
    match p {
        Primitive::Box { w, d, h } => {
            c |= f(ui, "width", w, 0.001);
            c |= f(ui, "depth", d, 0.001);
            c |= f(ui, "height", h, 0.001);
        }
        Primitive::Cylinder { r, h, sides } => {
            c |= f(ui, "radius", r, 0.001);
            c |= f(ui, "height", h, 0.001);
            c |= u(ui, "sides", sides, 3);
        }
        Primitive::Sphere { r, segments, stacks } => {
            c |= f(ui, "radius", r, 0.001);
            c |= u(ui, "segments", segments, 3);
            c |= u(ui, "stacks", stacks, 2);
        }
        Primitive::Frustum { r_bottom, r_top, h, sides } => {
            c |= f(ui, "r bottom", r_bottom, 0.0);
            c |= f(ui, "r top", r_top, 0.0);
            c |= f(ui, "height", h, 0.001);
            c |= u(ui, "sides", sides, 3);
        }
        Primitive::Torus { major_r, minor_r, seg_major, seg_minor } => {
            c |= f(ui, "ring r", major_r, 0.001);
            c |= f(ui, "tube r", minor_r, 0.001);
            c |= u(ui, "seg ring", seg_major, 3);
            c |= u(ui, "seg tube", seg_minor, 3);
        }
        Primitive::Capsule { r, h, segments, stacks } => {
            c |= f(ui, "radius", r, 0.001);
            c |= f(ui, "length", h, 0.001);
            c |= u(ui, "segments", segments, 3);
            c |= u(ui, "stacks", stacks, 2);
        }
        Primitive::Tube { r_outer, r_inner, h, sides } => {
            c |= f(ui, "r outer", r_outer, 0.001);
            c |= f(ui, "r inner", r_inner, 0.0);
            c |= f(ui, "height", h, 0.001);
            c |= u(ui, "sides", sides, 3);
        }
        Primitive::Ellipsoid { rx, ry, rz, segments, stacks } => {
            c |= f(ui, "rx", rx, 0.001);
            c |= f(ui, "ry", ry, 0.001);
            c |= f(ui, "rz", rz, 0.001);
            c |= u(ui, "segments", segments, 3);
            c |= u(ui, "stacks", stacks, 2);
        }
        Primitive::Extrusion { h, .. } => {
            c |= f(ui, "height", h, 0.001);
            ui.label(egui::RichText::new("  outline shape is fixed").small().weak());
        }
        Primitive::Sweep { .. } => {
            ui.label(egui::RichText::new("  swept along a path — section & path are fixed").small().weak());
        }
    }
    c
}

/// Which manipulation the on-screen gizmo performs. Toggled from the 3D Factory bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GizmoMode {
    /// Translate arms + centre free-move (the original gizmo).
    #[default]
    Move,
    /// Three rotation rings (one per axis) — drag a ring to spin about that axis.
    Rotate,
}

/// One draggable handle of the gizmo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GizmoHandle {
    /// Constrain the move to the world X / Y / Z axis.
    X,
    Y,
    Z,
    /// Free move — the centre cube where the three arms meet; slides the object across the
    /// ground plane (a combination of X and Y at once).
    Free,
    /// Rotation ring about axis 0 / 1 / 2 (world axes for furniture, plane-local for a
    /// feature). Coloured red / green / blue like the move axes.
    RotX,
    RotY,
    RotZ,
}

impl GizmoHandle {
    /// World-axis direction, or `None` for Free.
    pub fn axis(self) -> Option<Vec3> {
        match self {
            GizmoHandle::X | GizmoHandle::RotX => Some(Vec3::X),
            GizmoHandle::Y | GizmoHandle::RotY => Some(Vec3::Y),
            GizmoHandle::Z | GizmoHandle::RotZ => Some(Vec3::Z),
            GizmoHandle::Free => None,
        }
    }

    /// Which of the three axes (0/1/2) this handle rotates about, if it's a ring.
    pub fn ring_axis(self) -> Option<usize> {
        match self {
            GizmoHandle::RotX => Some(0),
            GizmoHandle::RotY => Some(1),
            GizmoHandle::RotZ => Some(2),
            _ => None,
        }
    }

    /// Axis colour: X red, Y green, Z blue — the universal convention.
    pub fn color(self) -> egui::Color32 {
        match self {
            GizmoHandle::X | GizmoHandle::RotX => egui::Color32::from_rgb(235, 80, 80),
            GizmoHandle::Y | GizmoHandle::RotY => egui::Color32::from_rgb(90, 210, 90),
            GizmoHandle::Z | GizmoHandle::RotZ => egui::Color32::from_rgb(90, 150, 245),
            GizmoHandle::Free => egui::Color32::from_rgb(230, 230, 230),
        }
    }
}

/// In-progress rotation-ring drag. Captured on grab so the whole gesture is one undo step
/// and the rotation is measured relative to where you first grabbed the ring.
#[derive(Clone, Debug)]
pub struct RotDrag {
    pub handle: GizmoHandle,
    /// Unit rotation axis in WORLD space (a world axis for furniture; a plane-local axis for
    /// a feature).
    pub axis: Vec3,
    pub center: Vec3,
    /// Reference vector (in the rotation plane) where the grab started.
    pub r0: Vec3,
    /// Start rotation of the target: furniture Euler `[x,y,z]°`, or feature `[pitch,roll,spin]°`.
    pub start_rot: [f32; 3],
    /// For a feature, which placement angle (0=pitch,1=roll,2=spin) this ring drives.
    pub feat_axis: usize,
    pub is_furniture: bool,
    /// Every selected furniture instance as it stood when the grab began: `(index, pos, rot°)`.
    ///
    /// Captured up front, and every frame of the drag is computed from THESE rather than from the
    /// live values. Applying an incremental delta each frame would accumulate float error over a
    /// long drag and, worse, drift the pieces apart from each other — a row of chairs would come
    /// out of one rotation no longer a row.
    pub start_furn: Vec<(usize, [f32; 3], [f32; 3])>,
}

/// One projected arm of the gizmo.
pub struct GizmoArm {
    pub handle: GizmoHandle,
    pub dir: Vec3,
    pub tip_s: egui::Pos2,
}

/// The gizmo projected to screen space for one frame. Arms are `Option` because an arm
/// tip can fall behind the camera; a `None` arm is simply not drawn or picked.
pub struct GizmoView {
    pub center_w: Vec3,
    pub center_s: egui::Pos2,
    pub len_w: f32,
    pub arms: [Option<GizmoArm>; 3],
}

/// One rotation ring projected to screen: the axis it spins about + its screen polyline.
pub struct Ring {
    pub handle: GizmoHandle,
    pub axis: Vec3,           // world-space unit rotation axis
    pub pts: Vec<egui::Pos2>, // projected circle (screen space)
}

/// The rotation gizmo projected to screen for one frame — three rings + centre.
pub struct RingView {
    pub center: Vec3,
    pub center_s: egui::Pos2,
    pub radius: f32,
    pub rings: Vec<Ring>,
    pub is_furniture: bool,
}

/// Gizmo sizing / pick tolerances (pixels).
/// APX render mode: a placed furniture mesh with MORE triangles than this is drawn as a
/// 12-triangle bounding-box proxy instead of its full geometry, so heavy scenes stay smooth.
/// Light pieces (and everything in GPU / CPU mode) always draw in full.
const APX_FURNITURE_TRIS: usize = 5_000;

const GIZMO_MIN_PX: f32 = 65.0;   // shortest on-screen arm length, so tiny objects stay grabbable
const GIZMO_AXIS_PICK: f32 = 8.0;
const GIZMO_CUBE_PICK: f32 = 9.0;

/// Distance from a point to a line segment, in screen space. Public alias for the app's
/// gizmo hover test.
pub fn seg_dist(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    dist_point_segment(p, a, b)
}

/// Distance from a point to a line segment, in screen space.
fn dist_point_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    let proj = a + ab * t;
    p.distance(proj)
}

/// Radius a vertex handle is drawn at, and the two pick apertures. The edge aperture is
/// deliberately smaller: a midpoint lies between two vertex handles, so a close call must
/// go to the vertex — otherwise dragging a corner would insert a point instead.
pub const HANDLE_DRAW_R: f32 = 4.5;
const HANDLE_PICK_R: f32 = 10.0;
const EDGE_PICK_R: f32 = 7.0;

/// Project a world point to screen. `None` when it falls outside the depth range (behind
/// the camera), so nothing is drawn or picked where the user cannot see it.
pub fn world_to_screen(w: Vec3, rect: egui::Rect, mvp: &[f32; 16]) -> Option<egui::Pos2> {
    let ndc = Mat4::from_cols_array(mvp).project_point3(w);
    if !(-1.0..=1.0).contains(&ndc.z) {
        return None;
    }
    Some(egui::pos2(
        rect.left() + (ndc.x * 0.5 + 0.5) * rect.width(),
        rect.top() + (0.5 - ndc.y * 0.5) * rect.height(),
    ))
}

/// Nearest candidate within `aperture` pixels of `cursor`.
fn nearest_within(
    items: Vec<(usize, egui::Pos2)>, cursor: egui::Pos2, aperture: f32,
) -> Option<usize> {
    items
        .into_iter()
        .map(|(i, p)| (p.distance(cursor), i))
        .filter(|(d, _)| *d <= aperture)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i)| i)
}

/// Why a room could not be carved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoomError {
    /// No solid to cut from — make a building first.
    NoBuilding,
    /// The outline itself was invalid (too few points / no area / self-crossing).
    Profile(cad_solid::ProfileError),
}


/// Identifies a flat SURFACE for per-face colouring: the body's feature id plus its world
/// plane, quantised (normal ×50, offset ×100) so all coplanar triangles of one face share
/// a key. Stable while the object doesn't move; a moved object is simply re-painted.
pub type SurfaceKey = (u32, i32, i32, i32, i32);

/// Compute a triangle's [`SurfaceKey`] from its face id and its three world positions.
pub fn surface_key(face_id: u32, a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> SurfaceKey {
    let av = Vec3::from(a);
    let n = (Vec3::from(b) - av).cross(Vec3::from(c) - av).normalize_or_zero();
    let d = n.dot(av);
    (
        face_id,
        (n.x * 50.0).round() as i32,
        (n.y * 50.0).round() as i32,
        (n.z * 50.0).round() as i32,
        (d * 100.0).round() as i32,
    )
}

/// Tolerance for "is this feature on that storey" / "is this wall at that base". Floor
/// heights are summed f32s, so an exact `==` would miss by an ulp after a few levels.
const Z_EPS: f32 = 1e-4;

/// Floor-to-floor heights below this are rejected — a zero-height storey has no z band,
/// so nothing could ever be assigned to it.
const MIN_STOREY_H: f32 = 0.1;

/// An imported furniture MESH held in the project library. Stored once; placed as many
/// times as you like via [`FurnitureInst`]. Triangle soup (positions/normals, 3 per tri).
#[derive(Clone, Debug, Default)]
pub struct FurnitureAsset {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Default colour for new instances — the file's diffuse if it had one, else a neutral
    /// light grey (NOT the old tan). The user can recolour per instance afterward.
    pub color: [f32; 3],
    /// LOCAL-space bounding box (min, max) over `positions`, computed ONCE at construction.
    /// A placed instance's world AABB is the transform of these 8 corners — O(8), not a
    /// per-frame sweep of every vertex (which, at ~90k verts, tanked the framerate whenever
    /// a heavy piece was selected: the gizmo + highlight both asked for the AABB each frame).
    pub local_min: [f32; 3],
    pub local_max: [f32; 3],
    /// The factor [`FactoryState::add_furniture_asset`] applied to normalise the import (1.0 when
    /// it was left alone). A file whose longest side exceeds 20 m is shrunk toward a 1.5 m asset,
    /// which is right for a chair exported in millimetres and wrong for a building exported at
    /// real-world size. Recording it means a caller who KNOWS the file is already in metres can
    /// undo it exactly — `inst.scale = 1.0 / import_scale` — instead of guessing a multiplier.
    pub import_scale: f32,
    /// Per-vertex UVs (parallel to `positions`) from a glTF import. EMPTY when the source had
    /// no texture coords — then a textured instance falls back to box-projection UVs.
    pub uvs: Vec<[f32; 2]>,
    /// Per-vertex OPACITY (1.0 = opaque), parallel to `positions`, from the import's materials
    /// (OBJ/MTL `d`/`Tr`, glTF `alphaMode:BLEND`). EMPTY ⇒ fully opaque — the mesh takes the
    /// original single-pass draw. When present, translucent triangles are peeled into a
    /// separate blended pass so glass panes read as see-through. See [`Self::is_translucent`].
    pub alpha: Vec<f32>,
    /// Absolute path the mesh was imported from, when known — so a project saved by an older build
    /// (or awaiting a parser that learns a new transparency source) can RE-DERIVE per-vertex alpha
    /// on load without a manual re-import. `None` for meshes with no on-disk source (bundled/test).
    pub source_path: Option<String>,
    /// True once [`Self::alpha`] has been resolved from the source and is authoritative — set when
    /// freshly imported, and after a one-time on-load re-parse. Guards against re-parsing a heavy
    /// opaque mesh on every load just to re-confirm it has no glass.
    pub alpha_resolved: bool,
    /// Lazily-built DISPLAY level-of-detail (decimated positions + flat normals) for very heavy
    /// meshes. Drawing a 2M-triangle piece — let alone several copies — at full detail every
    /// frame is ~140 ms/frame; this coarse proxy keeps the shape but a fraction of the tris.
    /// Only built for heavy, UNTEXTURED assets (welding vertices would scramble real UVs).
    pub lod: std::cell::RefCell<Option<std::sync::Arc<(Vec<[f32; 3]>, Vec<[f32; 3]>)>>>,
    /// Lazily-built per-triangle grouping (coplanar faces + connected bodies) for per-surface
    /// texturing. Computed once from `positions`; see [`Self::group_geom`].
    pub groups: std::cell::RefCell<Option<std::sync::Arc<FurnGroups>>>,
    /// Optional per-triangle PART id (parallel to `positions`), set for GENERATED objects (e.g. a
    /// staircase, where each tread/riser/baluster is a distinct primitive). When present it drives
    /// the "piece" grouping so clicking a tread selects that ONE tread — not the whole welded run,
    /// which is what a geometry-only connected-component grouping would give. Empty for imported
    /// meshes (they fall back to welded connected components).
    pub part_ids: Vec<u32>,
}

/// How an applied texture lands on the selected furniture object.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FurnPaintMode {
    /// The texture covers the entire object (the classic behaviour).
    #[default]
    WholeObject,
    /// Arm a brush; the next click textures the single flat face clicked.
    Face,
    /// Arm a brush; the next click textures the whole connected piece clicked.
    Piece,
}

/// Per-triangle grouping of a furniture asset for per-surface texturing (one id per triangle).
/// `face[t]` = coplanar-connected flat surface id; `body[t]` = connected-component (welded sub-part)
/// id. Built once per asset by [`cad_solid::surface_groups`].
#[derive(Clone, Debug, Default)]
pub struct FurnGroups {
    pub face: Vec<u32>,
    pub body: Vec<u32>,
}

/// The split draw lists for a per-surface-textured furniture instance: `opaque` is one
/// `(texture_index, buffer_key, verts)` group per texture used across the piece's faces; `flat` is
/// the untextured remainder as flat-shaded verts (`None` when every face is textured). Built by
/// [`FactoryState::furniture_faceted`].
pub struct FacetedFurniture {
    pub opaque: Vec<(usize, u64, Vec<crate::light3d::TexVtx>)>,
    /// Texture groups whose surface is SEE-THROUGH (the texture's opacity < 1, or the mesh's own
    /// glass alpha) — drawn in the blended textured pass, back-to-front.
    pub translucent: Vec<(usize, u64, Vec<crate::light3d::TexVtx>)>,
    pub flat: Option<(u64, Vec<V3>)>,
}

/// Above this triangle count a furniture asset gets a decimated display proxy.
pub const LOD_TRI_THRESHOLD: usize = 200_000;

/// Above this triangle count, [`FurnitureAsset::group_geom`] stops running the coplanar flood fill
/// and uses the import's material PARTS as the face grouping instead. The flood fill is O(n) with a
/// large constant — 3.7 s in release for 1.86 M triangles, and it runs on the UI thread the first
/// time anything draws. See the note in `group_geom` for why parts are the better answer at that
/// size, not merely the cheaper one.
pub const COPLANAR_TRI_LIMIT: usize = 250_000;

/// At/above this opacity a vertex is treated as fully opaque (drawn in the solid pass). Below
/// it, the triangle is peeled into the blended transparent pass.
pub const ALPHA_OPAQUE: f32 = 0.996;

impl FurnitureAsset {
    /// Build an asset and cache its local AABB. All construction goes through here so the
    /// cached bounds can never be stale or forgotten.
    pub fn new(name: String, positions: Vec<[f32; 3]>, normals: Vec<[f32; 3]>, color: [f32; 3]) -> Self {
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for p in &positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        if !mn[0].is_finite() {
            mn = [0.0; 3];
            mx = [0.0; 3];
        }
        Self { name, positions, normals, color, local_min: mn, local_max: mx, import_scale: 1.0, uvs: Vec::new(), alpha: Vec::new(), source_path: None, alpha_resolved: false, lod: std::cell::RefCell::new(None), groups: std::cell::RefCell::new(None), part_ids: Vec::new() }
    }

    /// The per-triangle face/body grouping, built once and cached. Used for per-surface texturing.
    ///
    /// `body` is the PART id grouping when this is a generated object that carries `part_ids` (so
    /// "one piece" = one tread/baluster), otherwise the geometry-welded connected components.
    ///
    /// `face` is the coplanar-connected grouping — BUT, like Blender's per-polygon materials, a face
    /// must never span two primitives. Abutting boxes (a tread + its riser + the next tread) share
    /// the SAME plane along the stair's side and connect edge-to-edge, so a naive coplanar flood
    /// fill merges the whole side into ONE face; texturing it would paint the entire side. So when
    /// `part_ids` exist we REFINE each coplanar region by part id → a face is at most one flat side
    /// of one primitive.
    pub fn group_geom(&self) -> std::sync::Arc<FurnGroups> {
        if let Some(g) = self.groups.borrow().as_ref() {
            return g.clone();
        }
        let ntri = self.positions.len() / 3;
        let has_parts = self.part_ids.len() == ntri;

        // HEAVY IMPORTS SKIP THE FLOOD FILL. `surface_groups` welds every vertex and grows coplanar
        // regions across the whole mesh; on the villa scene (1.86 M triangles) it takes **3.7
        // seconds in release** and far longer in a debug build — on the UI thread, at first draw,
        // which is exactly the "the app is unresponsive after loading" symptom.
        //
        // What it buys is click-a-face texturing. On a mesh this size it produced 407,858 coplanar
        // regions, which is not a granularity anyone can work at anyway. A whole imported scene
        // already arrives with PART ids — one per source material — and that is the level a user
        // actually paints at: "the roof", "the walls". So for a heavy mesh the parts ARE the faces,
        // and the wait disappears entirely.
        if ntri > COPLANAR_TRI_LIMIT {
            let g = std::sync::Arc::new(if has_parts {
                FurnGroups { face: self.part_ids.clone(), body: self.part_ids.clone() }
            } else {
                // No parts either (a raw OBJ soup): one group. Per-face painting degrades to
                // whole-object painting, which is honest — better than a ten-second freeze for a
                // grouping nobody can use.
                FurnGroups { face: vec![0; ntri], body: vec![0; ntri] }
            });
            *self.groups.borrow_mut() = Some(g.clone());
            return g;
        }

        let (face_coplanar, welded_body) = cad_solid::surface_groups(&self.positions);
        let body = if has_parts { self.part_ids.clone() } else { welded_body };
        let face = if has_parts {
            // Split every coplanar region at part boundaries so a face stays within one primitive.
            let mut remap = std::collections::HashMap::new();
            let mut next = 0u32;
            (0..ntri)
                .map(|t| {
                    let key = (face_coplanar[t], self.part_ids[t]);
                    *remap.entry(key).or_insert_with(|| {
                        let id = next;
                        next += 1;
                        id
                    })
                })
                .collect()
        } else {
            face_coplanar
        };
        let g = std::sync::Arc::new(FurnGroups { face, body });
        *self.groups.borrow_mut() = Some(g.clone());
        g
    }

    /// This vertex's opacity (1.0 when the asset carries no alpha, or the index is past its end).
    #[inline]
    pub fn vertex_alpha(&self, i: usize) -> f32 {
        self.alpha.get(i).copied().unwrap_or(1.0)
    }

    /// True when any triangle is see-through, i.e. the mesh has a translucent material. Cheap:
    /// `alpha` is empty for the opaque common case, so this is usually a length check + scan.
    pub fn is_translucent(&self) -> bool {
        self.alpha.iter().any(|&a| a < ALPHA_OPAQUE)
    }

    /// True when this asset is heavy enough to warrant the decimated display proxy. Skipped for
    /// UV-mapped (glTF) assets — clustering welds vertices and would break their texture coords.
    pub fn needs_lod(&self) -> bool {
        // Translucent assets skip decimation: welding/dropping vertices would break the
        // per-vertex `alpha` correspondence used to split opaque vs. see-through triangles.
        self.uvs.is_empty() && self.alpha.is_empty() && self.positions.len() / 3 > LOD_TRI_THRESHOLD
    }

    /// The decimated (positions, normals), built once and cached.
    pub fn lod_geom(&self) -> std::sync::Arc<(Vec<[f32; 3]>, Vec<[f32; 3]>)> {
        {
            let c = self.lod.borrow();
            if let Some(a) = c.as_ref() {
                return a.clone();
            }
        }
        let built = std::sync::Arc::new(cluster_decimate(&self.positions, 64));
        *self.lod.borrow_mut() = Some(built.clone());
        built
    }
}

/// Vertex-cluster decimation: snap every vertex to a `grid`³ lattice over the mesh bounds, weld
/// coincident cells to their centroid, and keep only triangles whose three vertices land in
/// distinct cells. Flat per-face normals are recomputed. Deterministic and silhouette-
/// preserving — a fast display proxy of a multi-million-triangle import.
///
/// Uses a FLAT grid array (no per-vertex HashMap) so 6M vertices decimate in tens of ms, not
/// the ~half-second the hashed version cost (which showed up as spikes as heavy pieces loaded).
pub fn cluster_decimate(pos: &[[f32; 3]], grid: u32) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    if pos.len() < 3 {
        return (pos.to_vec(), vec![[0.0, 0.0, 1.0]; pos.len()]);
    }
    let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in pos {
        for k in 0..3 { mn[k] = mn[k].min(p[k]); mx[k] = mx[k].max(p[k]); }
    }
    let ext = [(mx[0] - mn[0]).max(1e-6), (mx[1] - mn[1]).max(1e-6), (mx[2] - mn[2]).max(1e-6)];
    let gi = grid.clamp(2, 128) as usize;
    let gf = gi as f32;
    let idx = |p: &[f32; 3]| -> usize {
        let c = |v: f32, mnk: f32, ek: f32| (((v - mnk) / ek * gf).floor() as i64).clamp(0, gi as i64 - 1) as usize;
        (c(p[0], mn[0], ext[0]) * gi + c(p[1], mn[1], ext[1])) * gi + c(p[2], mn[2], ext[2])
    };
    // Flat cell grid → running centroid (sum x,y,z,count). ~8 MB at grid=64 (transient).
    let mut acc = vec![[0.0f64; 4]; gi * gi * gi];
    for p in pos {
        let a = &mut acc[idx(p)];
        a[0] += p[0] as f64; a[1] += p[1] as f64; a[2] += p[2] as f64; a[3] += 1.0;
    }
    let centroid = |i: usize| -> [f32; 3] {
        let a = acc[i];
        [(a[0] / a[3]) as f32, (a[1] / a[3]) as f32, (a[2] / a[3]) as f32]
    };
    let mut out_p = Vec::new();
    let mut out_n = Vec::new();
    for t in pos.chunks_exact(3) {
        let (ia, ib, ic) = (idx(&t[0]), idx(&t[1]), idx(&t[2]));
        if ia == ib || ib == ic || ia == ic {
            continue; // collapses to a sliver/point after welding
        }
        let (a, b, c) = (centroid(ia), centroid(ib), centroid(ic));
        let n = (Vec3::from(b) - Vec3::from(a))
            .cross(Vec3::from(c) - Vec3::from(a))
            .normalize_or_zero();
        let n = [n.x, n.y, n.z];
        out_p.push(a); out_p.push(b); out_p.push(c);
        out_n.push(n); out_n.push(n); out_n.push(n);
    }
    (out_p, out_n)
}

/// Which step of the PATH-SWEEP flow is active.
#[derive(Clone, Debug, PartialEq)]
pub enum SweepStage {
    /// Drawing the cross-section on a picked face.
    Section,
    /// Section captured; the app is offering the two perpendicular views for the path.
    ChooseView,
    /// Drawing the path on the chosen perpendicular plane.
    Path,
}

/// A PATH-SWEEP in progress: cross-section on a face → pick a perpendicular view → draw the
/// path → build. `cut` selects Difference vs Union; `furniture` only tints a Union result.
#[derive(Clone, Debug)]
pub struct SweepFlow {
    pub cut: bool,
    pub furniture: bool,
    pub stage: SweepStage,
    /// The face the section was drawn on (captured when the section is finished).
    pub section_frame: Option<Frame>,
    /// The cross-section loop, in the section frame's (u,v).
    pub section_loop: Option<Vec<glam::Vec2>>,
    /// The two perpendicular drawing planes offered for the path: (view-name, frame).
    pub views: Vec<(String, Frame)>,
    /// The plane the user chose to draw the path on.
    pub path_frame: Option<Frame>,
}

/// One PLACED copy of a library asset.
#[derive(Clone, Debug)]
pub struct FurnitureInst {
    /// Index into [`FactoryState::furniture_lib`].
    pub asset: usize,
    pub pos: [f32; 3],
    pub scale: f32,
    /// Optional NON-UNIFORM scale `[sx, sy, sz]` in the asset's local axes. `None` → the piece
    /// scales uniformly by `scale` (the normal case). `Some` is used by APERTURES (doors/windows),
    /// which stretch to exactly fill a drawn opening. `scale` still multiplies on top, so the
    /// scale gizmo keeps working.
    pub fit: Option<[f32; 3]>,
    /// Euler rotation in DEGREES about the world X, Y, Z axes (applied X→Y→Z), pivoting
    /// on the instance's base-centre. `rot[2]` is the old yaw (Z), so upgrades are clean.
    pub rot: [f32; 3],
    /// Linear RGB, applied in the Textures menu (default a warm neutral).
    pub color: [f32; 3],
    /// Index into [`FactoryState::textures`] when a clipboard texture is applied. Phase 1
    /// only records the assignment (the visible effect is the texture's average colour
    /// baked into `color`); the UV-mapped render pass reads this in Phase 2.
    pub texture: Option<usize>,
    /// PER-SURFACE textures: `face_group_id → texture index`. A face-group id comes from the
    /// asset's [`FurnGroups::face`] grouping (a coplanar flat surface). When present for a
    /// triangle's group it overrides `texture` (the whole-object texture) for that surface — so
    /// one object can carry different textures on different faces/pieces. Empty ⇒ whole-object only.
    pub surface_texture: std::collections::HashMap<u32, usize>,

    /// CUTS drawn on this piece's faces — holes, rebates, service openings. See
    /// [`FactoryState::rebuild_cut_asset`] for how they become geometry.
    ///
    /// The list is the truth and the geometry is derived from it: every rebuild replays the whole
    /// list against the UNTOUCHED original, so a cut can be switched off, deleted or re-ordered and
    /// the piece returns to exactly what it was. Frames are stored in the asset's own LOCAL space,
    /// so moving or rotating the piece carries its holes with it.
    pub cuts: Vec<cad_solid::meshcut::MeshCut>,
    /// The uncut original in [`FactoryState::furniture_lib`] when `asset` points at a derived
    /// (cut) copy. `None` ⇒ `asset` IS the original and there is nothing to fall back to.
    pub base_asset: Option<usize>,
}

impl Default for FurnitureInst {
    fn default() -> Self {
        Self {
            asset: 0,
            pos: [0.0; 3],
            scale: 1.0,
            fit: None,
            rot: [0.0; 3],
            color: [0.8, 0.8, 0.8],
            texture: None,
            surface_texture: std::collections::HashMap::new(),
            cuts: Vec::new(),
            base_asset: None,
        }
    }
}

impl FurnitureInst {
    /// The library entry this instance's cuts are computed FROM — the uncut original.
    pub fn source_asset(&self) -> usize {
        self.base_asset.unwrap_or(self.asset)
    }
}

/// One copied 3D object held in the paste buffer — a furniture instance or a CSG feature
/// (with its colour/texture), so Ctrl+V reproduces it faithfully.
#[derive(Clone)]
pub enum FactoryClip {
    Furniture(FurnitureInst),
    Feature {
        op: BoolOp,
        plane: Plane,
        placement: Placement,
        primitive: Primitive,
        color: Option<[f32; 3]>,
        texture: Option<usize>,
        /// Per-FACE paint, captured as `(world normal, plane offset d, colour)` — the PRECISE
        /// plane, not the lossy `SurfaceKey`, so it can be re-keyed exactly after the paste's
        /// translation shifts each face's offset by `n·delta`.
        surface_colors: Vec<([f32; 3], f32, [f32; 3])>,
        /// Per-face texture assignments, same precise `(normal, d, texture_index)` form.
        surface_textures: Vec<([f32; 3], f32, usize)>,
    },
}

/// A PROCEDURAL texture pattern — evaluated in the shader from world position (no pixels). The
/// engine's answer to Blender's noise→ColorRamp materials. See [`crate::light3d::ProcParams`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcPattern {
    /// Anisotropic fbm straight into the ramp — the timber-grain recipe.
    Wood,
    /// fbm turbulence folded through a sine band — veined stone.
    Marble,
    /// Plain fractal noise between the two colours.
    Noise,
    /// Hard world-space cells between the two colours.
    Checker,
}

impl ProcPattern {
    pub fn label(self) -> &'static str {
        match self {
            ProcPattern::Wood => "Wood grain",
            ProcPattern::Marble => "Marble",
            ProcPattern::Noise => "Noise",
            ProcPattern::Checker => "Checker",
        }
    }
    /// The shader mode id (matches [`crate::light3d::ProcParams::mode`]).
    pub fn mode(self) -> i32 {
        match self {
            ProcPattern::Wood => 1,
            ProcPattern::Marble => 2,
            ProcPattern::Noise => 3,
            ProcPattern::Checker => 4,
        }
    }
    /// Stable persistence tag.
    pub fn tag(self) -> &'static str {
        match self {
            ProcPattern::Wood => "wood",
            ProcPattern::Marble => "marble",
            ProcPattern::Noise => "noise",
            ProcPattern::Checker => "checker",
        }
    }
    pub fn from_tag(s: &str) -> Self {
        match s {
            "marble" => ProcPattern::Marble,
            "noise" => ProcPattern::Noise,
            "checker" => ProcPattern::Checker,
            _ => ProcPattern::Wood,
        }
    }
    pub const ALL: [ProcPattern; 4] = [ProcPattern::Wood, ProcPattern::Marble, ProcPattern::Noise, ProcPattern::Checker];
}

/// A procedural material definition: two colours, an anisotropic world-space scale (tiles/m across,
/// along, through the grain) and the noise/ramp shaping. Grain runs continuously across every piece
/// because it reads WORLD position (Blender's object-coordinate trick, for free).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcDef {
    pub pattern: ProcPattern,
    pub col_a: [f32; 3],
    pub col_b: [f32; 3],
    pub scale: [f32; 3],
    pub detail: f32,
    pub rough: f32,
    pub contrast: f32,
    pub ramp: [f32; 2],
    /// SURFACE roughness at the two ends of the same pattern — not to be confused with `rough`
    /// above, which is the fBm's amplitude falloff. Dark grain rougher than light, mortar rougher
    /// than tile: a real surface changes its gloss wherever it changes its colour, and reading both
    /// off one field is what stops a procedural looking like a photo printed on plastic.
    pub surf_rough: [f32; 2],
    /// Relief from that same field, as a bump. 0 = flat.
    pub bump: f32,
}

impl ProcDef {
    /// A believable oak veneer — the cabin front default, ported from `cabin_v1.py`'s `make_wood`
    /// (anisotropic scale (110,26,2.6), dark→light ramp). Scaled to metres for our world coords.
    pub fn oak() -> Self {
        Self {
            pattern: ProcPattern::Wood,
            col_a: [0.30, 0.20, 0.11], // dark grain
            col_b: [0.66, 0.52, 0.34], // light oak
            scale: [42.0, 10.0, 2.0],
            detail: 7.0,
            rough: 0.62,
            contrast: 1.4,
            ramp: [0.40, 0.62],
            // Open-pored oak: the dark early-wood is noticeably duller than the pale late-wood,
            // and the pores are a real depression a raking light finds.
            surf_rough: [0.72, 0.48],
            bump: 0.35,
        }
    }
    /// A SOLID colour as a (degenerate) procedural: both ramp stops the same, so any pattern renders
    /// a flat colour. This lets the Materials Factory drive a plain base colour through the LIVE
    /// per-frame procedural uniforms (no GPU texture re-upload), so colour edits show instantly.
    pub fn solid(c: [f32; 3]) -> Self {
        Self { pattern: ProcPattern::Noise, col_a: c, col_b: c, scale: [1.0, 1.0, 1.0], detail: 1.0, rough: 0.5, contrast: 1.0, ramp: [0.0, 1.0], surf_rough: [0.5, 0.5], bump: 0.0 }
    }
    /// Whether this is a solid colour (both ramp stops equal) — the inverse of [`Self::solid`], so the
    /// node editor can show it as an RGB node rather than a pattern.
    pub fn is_solid(&self) -> bool {
        (0..3).all(|k| (self.col_a[k] - self.col_b[k]).abs() < 1e-4)
    }
    /// Its representative flat colour (ramp midpoint) — used for the 1×1 fallback swatch + avg tint.
    pub fn avg_color(&self) -> [f32; 3] {
        [
            (self.col_a[0] + self.col_b[0]) * 0.5,
            (self.col_a[1] + self.col_b[1]) * 0.5,
            (self.col_a[2] + self.col_b[2]) * 0.5,
        ]
    }
    /// Pack into the renderer's uniform bundle.
    pub fn params(&self) -> crate::light3d::ProcParams {
        crate::light3d::ProcParams {
            mode: self.pattern.mode(),
            col_a: self.col_a,
            col_b: self.col_b,
            scale: self.scale,
            detail: self.detail,
            rough: self.rough,
            contrast: self.contrast,
            ramp: self.ramp,
            rough_lo: self.surf_rough[0],
            rough_hi: self.surf_rough[1],
            bump: self.bump,
        }
    }

    /// True when this pattern varies its own surface finish (rather than being a uniform gloss).
    /// The renderer only lets the pattern drive roughness when there is no roughness MAP, so this
    /// is what decides whether the material's scalar roughness is the one that matters.
    pub fn varies_roughness(&self) -> bool {
        (self.surf_rough[0] - self.surf_rough[1]).abs() > 1e-3
    }
}

/// A ready-made material for the Materials Factory library — a named Principled + procedural recipe
/// (the subset of the classic material-type catalogues — Physical/PBR woods, stones, metals, glass,
/// paints, emitters — our engine represents). Applying one mints a normal [`TextureAsset`].
#[derive(Clone, Copy, Debug)]
pub struct MaterialPreset {
    pub category: &'static str,
    pub name: &'static str,
    /// The base-colour source: a real pattern, or [`ProcDef::solid`] for a plain colour.
    pub def: ProcDef,
    pub metallic: f32,
    pub roughness: f32,
    pub ior: f32,
    pub opacity: f32,
    /// This material is a MEDIUM light travels THROUGH — water, a solid glass block — rather than a
    /// surface with holes in it. See [`TextureAsset::transmission`]: it is what tells the renderer
    /// the surface belongs to a volume, so what is behind it gets FILTERED by the material's colour
    /// instead of merely uncovered, and so the volume's back faces are not drawn.
    ///
    /// 0 for almost everything, including the Glass presets: an architectural pane is modelled as a
    /// thin sheet, and coverage is the honest description of it. A body of water is not.
    pub transmission: f32,
    pub emission: [f32; 3],
    pub emission_strength: f32,
}

impl MaterialPreset {
    const fn flat(category: &'static str, name: &'static str, def: ProcDef) -> Self {
        Self { category, name, def, metallic: 0.0, roughness: 0.6, ior: 1.5, opacity: 1.0, transmission: 0.0, emission: [0.0; 3], emission_strength: 0.0 }
    }
}

/// The built-in material library, grouped by category (order = display order). Everything here is
/// expressible in the live renderer AND the path tracers: procedural/solid base colour + metallic /
/// roughness / IOR / opacity / emission.
/// The built-in material library.
///
/// Rebuilt in Phase 3 on **measured** numbers rather than eyeballed ones. Three things changed:
///
/// 1. **Albedos come from measured reflectance.** A "white" wall is not 0.88 — architectural white
///    paint measures around 0.75–0.80 diffuse reflectance, and fresh concrete about 0.35, not the
///    0.5 it was given. Too-bright albedos are why a bounced-light render goes milky: every bounce
///    multiplies the error. Values here are sRGB-encoded (what the colour picker shows); the
///    renderer decodes them.
/// 2. **Metals use their real spectral F0.** Copper is (0.95, 0.64, 0.54) and gold (1.00, 0.77,
///    0.34) at normal incidence — not a guess at "orange" and "yellow". A metal's base colour IS
///    its F0 once `metallic = 1`, so this is a physical constant, not a taste decision.
/// 3. **Roughness varies with the pattern.** Every procedural now carries a `surf_rough` range read
///    off the same field as its colour, plus a bump. This is the single biggest change: it is why
///    oak now catches light along the grain instead of glinting like a uniform sheet of plastic.
///
/// Everything is still expressible in BOTH renderers: procedural/solid base colour + metallic /
/// roughness / IOR / opacity / emission.
pub fn material_presets() -> Vec<MaterialPreset> {
    use ProcPattern::*;
    #[allow(clippy::too_many_arguments)]
    let pat = |pattern, col_a, col_b, scale: [f32; 3], detail, rough, contrast, ramp, surf_rough, bump| ProcDef {
        pattern, col_a, col_b, scale, detail, rough, contrast, ramp, surf_rough, bump,
    };
    let mut v: Vec<MaterialPreset> = Vec::new();
    let mut p = |m: MaterialPreset| v.push(m);

    // ---- Wood ---- (open-pored species: dark early-wood duller and lower than pale late-wood)
    p(MaterialPreset { roughness: 0.6, ..MaterialPreset::flat("Wood", "Oak", ProcDef::oak()) });
    p(MaterialPreset { roughness: 0.58, ..MaterialPreset::flat("Wood", "Walnut", pat(Wood, [0.14, 0.09, 0.05], [0.36, 0.23, 0.13], [42.0, 10.0, 2.0], 7.0, 0.62, 1.5, [0.38, 0.62], [0.68, 0.44], 0.30)) });
    p(MaterialPreset { roughness: 0.62, ..MaterialPreset::flat("Wood", "Pine", pat(Wood, [0.52, 0.40, 0.25], [0.74, 0.62, 0.42], [36.0, 9.0, 2.0], 6.0, 0.6, 1.3, [0.4, 0.62], [0.70, 0.52], 0.22)) });
    // Varnish is a clear dielectric layer over the timber: the grain still shows, the FINISH does
    // not vary with it, and it is not remotely metallic (the old preset used metallic 0.22 to fake
    // a sheen, which tinted the specular with the wood's own colour — the classic wrong-metal look).
    p(MaterialPreset { roughness: 0.14, ior: 1.52, ..MaterialPreset::flat("Wood", "Varnished oak", ProcDef { surf_rough: [0.14, 0.12], bump: 0.10, ..ProcDef::oak() }) });
    p(MaterialPreset { roughness: 0.52, ..MaterialPreset::flat("Wood", "Wenge (dark)", pat(Wood, [0.06, 0.045, 0.035], [0.18, 0.13, 0.09], [48.0, 12.0, 2.2], 7.0, 0.6, 1.6, [0.38, 0.6], [0.62, 0.40], 0.40)) });

    // ---- Stone & masonry ----
    // Polished marble is glossy where the calcite is and slightly duller along the veins.
    p(MaterialPreset { roughness: 0.13, ..MaterialPreset::flat("Stone", "White marble", pat(Marble, [0.88, 0.88, 0.86], [0.55, 0.56, 0.58], [3.0, 3.0, 3.0], 6.0, 0.55, 1.6, [0.35, 0.65], [0.10, 0.20], 0.05)) });
    p(MaterialPreset { roughness: 0.14, ..MaterialPreset::flat("Stone", "Black marble", pat(Marble, [0.05, 0.05, 0.06], [0.30, 0.30, 0.32], [3.0, 3.0, 3.0], 6.0, 0.55, 1.7, [0.35, 0.65], [0.11, 0.22], 0.05)) });
    // Concrete: ~0.35 reflectance, not 0.5. It is also genuinely rough at a millimetre scale.
    p(MaterialPreset { roughness: 0.88, ..MaterialPreset::flat("Stone", "Concrete", pat(Noise, [0.40, 0.395, 0.385], [0.30, 0.30, 0.29], [8.0, 8.0, 8.0], 7.0, 0.6, 1.1, [0.3, 0.7], [0.94, 0.80], 0.45)) });
    p(MaterialPreset { roughness: 0.45, ..MaterialPreset::flat("Stone", "Granite", pat(Noise, [0.32, 0.31, 0.30], [0.16, 0.155, 0.15], [60.0, 60.0, 60.0], 8.0, 0.7, 1.8, [0.35, 0.65], [0.30, 0.55], 0.25)) });
    p(MaterialPreset { roughness: 0.9, ..MaterialPreset::flat("Stone", "Sandstone", pat(Noise, [0.68, 0.60, 0.47], [0.54, 0.47, 0.36], [14.0, 14.0, 14.0], 6.0, 0.55, 1.2, [0.3, 0.7], [0.95, 0.82], 0.55)) });

    // ---- Metal ---- (base colour = the measured F0 at normal incidence, sRGB-encoded)
    p(MaterialPreset { metallic: 1.0, roughness: 0.04, ..MaterialPreset::flat("Metal", "Chrome", ProcDef::solid([0.95, 0.96, 0.97])) });
    p(MaterialPreset { metallic: 1.0, roughness: 0.28, ..MaterialPreset::flat("Metal", "Stainless steel", ProcDef::solid([0.77, 0.78, 0.78])) });
    // Brushed metal: an anisotropic streak that varies the FINISH, not the colour — which is what
    // brushing physically is. Hence a near-flat colour ramp and a wide roughness range.
    p(MaterialPreset { metallic: 1.0, roughness: 0.35, ..MaterialPreset::flat("Metal", "Brushed aluminium", pat(Wood, [0.89, 0.90, 0.91], [0.93, 0.94, 0.94], [220.0, 4.0, 2.0], 5.0, 0.55, 1.2, [0.3, 0.7], [0.46, 0.24], 0.05)) });
    p(MaterialPreset { metallic: 1.0, roughness: 0.22, ..MaterialPreset::flat("Metal", "Copper", ProcDef::solid([0.95, 0.64, 0.54])) });
    p(MaterialPreset { metallic: 1.0, roughness: 0.18, ..MaterialPreset::flat("Metal", "Gold", ProcDef::solid([1.00, 0.77, 0.34])) });
    p(MaterialPreset { metallic: 1.0, roughness: 0.45, ..MaterialPreset::flat("Metal", "Black steel", ProcDef::solid([0.16, 0.16, 0.17])) });
    p(MaterialPreset { metallic: 1.0, roughness: 0.12, ..MaterialPreset::flat("Metal", "Brass", ProcDef::solid([0.91, 0.79, 0.49])) });

    // ---- Glass ---- (IOR 1.52 = soda-lime, which every architectural pane is)
    p(MaterialPreset { roughness: 0.02, ior: 1.52, opacity: 0.06, ..MaterialPreset::flat("Glass", "Clear glass", ProcDef::solid([0.93, 0.96, 0.95])) });
    p(MaterialPreset { roughness: 0.45, ior: 1.52, opacity: 0.42, ..MaterialPreset::flat("Glass", "Frosted glass", ProcDef::solid([0.92, 0.94, 0.94])) });
    p(MaterialPreset { roughness: 0.05, ior: 1.52, opacity: 0.22, ..MaterialPreset::flat("Glass", "Bronze glass", ProcDef::solid([0.45, 0.34, 0.24])) });
    p(MaterialPreset { roughness: 0.04, ior: 1.52, opacity: 0.16, ..MaterialPreset::flat("Glass", "Blue glass", ProcDef::solid([0.6, 0.75, 0.85])) });

    // ---- Water ---- a MEDIUM, which is what separates these from the Glass presets above.
    //
    // `transmission` is the whole point. It tells the renderer this surface belongs to a volume, so
    // what lies behind gets FILTERED by the water's own colour rather than merely uncovered (the
    // difference between water and a hole in the ground), the reflection is not dimmed in step with
    // the transparency, and the faces of the volume that lie ON its container are not drawn — the
    // last of which is what stops a modelled pool z-fighting with its liner.
    //
    // Apply to ANY surface. Nothing here is specific to a pool: a fountain basin, a canal, a wet
    // road, a puddle drawn as a flat face all want the same material.
    //
    // IOR 1.333 is water at 20 °C. Roughness is the ONLY difference between the three: still water
    // is a mirror, and roughening it is what turns the reflection into a suggestion of one.
    p(MaterialPreset { roughness: 0.02, ior: 1.333, opacity: 0.55, transmission: 0.45,
        ..MaterialPreset::flat("Water", "Water (still)", ProcDef::solid([0.055, 0.30, 0.34])) });
    p(MaterialPreset { roughness: 0.12, ior: 1.333, opacity: 0.60, transmission: 0.40,
        ..MaterialPreset::flat("Water", "Water (rippled)", ProcDef::solid([0.055, 0.30, 0.34])) });
    // Deeper, greener and far less see-through — a lake or a canal rather than a swimming pool.
    p(MaterialPreset { roughness: 0.06, ior: 1.333, opacity: 0.82, transmission: 0.18,
        ..MaterialPreset::flat("Water", "Water (deep)", ProcDef::solid([0.018, 0.085, 0.075])) });

    // ---- Paint & plaster ---- (architectural white measures ~0.78, not ~0.9)
    p(MaterialPreset { roughness: 0.75, ..MaterialPreset::flat("Paint", "Matte white", ProcDef::solid([0.80, 0.79, 0.78])) });
    // Satin is a dielectric clearcoat. Metallic 0.12 (the old value) makes the highlight take the
    // paint's colour, which is what a metal does and a painted wall does not.
    p(MaterialPreset { roughness: 0.22, ior: 1.5, ..MaterialPreset::flat("Paint", "Satin white", ProcDef::solid([0.80, 0.79, 0.78])) });
    p(MaterialPreset { roughness: 0.55, ..MaterialPreset::flat("Paint", "Anthracite", ProcDef::solid([0.09, 0.095, 0.10])) });
    p(MaterialPreset { roughness: 0.82, ..MaterialPreset::flat("Paint", "Warm plaster", pat(Noise, [0.79, 0.75, 0.68], [0.72, 0.68, 0.61], [10.0, 10.0, 10.0], 5.0, 0.5, 1.0, [0.3, 0.7], [0.88, 0.76], 0.30)) });
    // Car paint IS a metallic-flake basecoat under a clearcoat, so this one keeps its metallic.
    p(MaterialPreset { metallic: 0.7, roughness: 0.12, ..MaterialPreset::flat("Paint", "Car paint red", ProcDef::solid([0.58, 0.05, 0.08])) });

    // ---- Fabric ---- (fully rough, with weave-scale relief)
    p(MaterialPreset { roughness: 0.95, ..MaterialPreset::flat("Fabric", "Grey fabric", pat(Noise, [0.44, 0.44, 0.46], [0.36, 0.36, 0.38], [90.0, 90.0, 90.0], 6.0, 0.6, 1.1, [0.3, 0.7], [1.0, 0.88], 0.6)) });
    p(MaterialPreset { roughness: 0.98, ..MaterialPreset::flat("Fabric", "Beige carpet", pat(Noise, [0.56, 0.50, 0.41], [0.45, 0.40, 0.32], [70.0, 70.0, 70.0], 7.0, 0.6, 1.1, [0.3, 0.7], [1.0, 0.92], 0.9)) });

    // ---- Floor ----
    // Tiles: glossy face, matte grout — the roughness range does the grout, no second map needed.
    p(MaterialPreset { roughness: 0.16, ..MaterialPreset::flat("Floor", "Checker tiles", pat(Checker, [0.78, 0.78, 0.77], [0.10, 0.10, 0.11], [2.0, 2.0, 2.0], 1.0, 0.5, 1.0, [0.0, 1.0], [0.14, 0.20], 0.06)) });
    p(MaterialPreset { roughness: 0.24, ..MaterialPreset::flat("Floor", "Terrazzo", pat(Noise, [0.74, 0.72, 0.69], [0.40, 0.39, 0.37], [90.0, 90.0, 90.0], 8.0, 0.75, 2.2, [0.42, 0.58], [0.20, 0.30], 0.10)) });

    // ---- Emission ---- (strength in the same scene-referred units the sun uses)
    p(MaterialPreset { roughness: 0.4, emission: [1.0, 0.85, 0.6], emission_strength: 5.0, ..MaterialPreset::flat("Emission", "LED warm", ProcDef::solid([1.0, 0.88, 0.7])) });
    p(MaterialPreset { roughness: 0.4, emission: [0.85, 0.9, 1.0], emission_strength: 5.0, ..MaterialPreset::flat("Emission", "LED cool", ProcDef::solid([0.88, 0.92, 1.0])) });
    p(MaterialPreset { roughness: 0.4, emission: [1.0, 0.08, 0.08], emission_strength: 8.0, ..MaterialPreset::flat("Emission", "Neon red", ProcDef::solid([1.0, 0.25, 0.25])) });

    v
}

impl FactoryState {
    /// Add a library [`MaterialPreset`] as a scene material, returning its texture index. The
    /// Materials Factory then behaves as with any material: node graph, live highlight, apply.
    pub fn add_preset_material(&mut self, p: &MaterialPreset) -> usize {
        let idx = self.add_procedural_texture(p.name.to_string(), p.def);
        let t = &mut self.textures[idx];
        t.metallic = p.metallic;
        t.roughness = p.roughness;
        t.ior = p.ior;
        t.opacity = p.opacity.clamp(0.01, 1.0);
        // A MEDIUM, not a surface with holes — see `TextureAsset::transmission`. Without this the
        // Water presets would import as ordinary alpha-blended paint: what lies beneath would show
        // through undyed rather than tinted, the reflection would be dimmed in proportion to the
        // transparency, and a modelled body of water would z-fight with whatever it sits in.
        t.transmission = p.transmission.clamp(0.0, 1.0);
        t.emission = p.emission;
        t.emission_strength = p.emission_strength;
        // `reflect` stays at its 1.0 default: metallic already reaches the specular through `f0`,
        // and multiplying by it again here is what left every dielectric preset matte.
        idx
    }
}

/// A bitmap captured from the OS clipboard and stored for texturing, OR a procedural pattern. For an
/// image, `rgba` is row-major RGBA8 (top row first) and `avg` its average linear-RGB colour (a flat
/// tint fallback); for a procedural material `proc` is `Some` and `rgba` is a 1×1 fallback swatch.
#[derive(Clone, Debug)]
pub struct TextureAsset {
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub avg: [f32; 3],
    /// Tiling factor: for a feature it's tiles-per-metre, for furniture it's the repeat count
    /// across the piece. 1.0 = the default (features tile once per metre, furniture wraps once).
    pub scale: f32,
    /// MOVE the texture: UV offset `[u, v]` applied after scale+rotation. Shifts the image across
    /// the surface (in tile units). `[0, 0]` = no shift.
    pub offset: [f32; 2],
    /// ROTATE the texture, degrees, about the tile centre. `0` = upright.
    pub rot_deg: f32,
    /// Surface OPACITY, `0.01..=1.0` (UI 1..100). 1.0 = fully opaque (the default); below it the
    /// surface is drawn see-through in the blended pass (multiplies the per-vertex alpha).
    pub opacity: f32,
    /// Surface REFLECTION, `0.01..=1.0` (UI 1..100) — how much of the PHYSICALLY CORRECT
    /// environment reflection this surface keeps. **1.0 (all of it) is the default.**
    ///
    /// It used to default to 0 and be computed as `metallic × (1 − 0.6·roughness)`, which meant
    /// every dielectric — water, glass, polished stone, varnished timber — had `metallic = 0` and
    /// so reflected nothing at all. That is backwards: water reflects ~2% head-on and ~100% at
    /// grazing incidence, and that Fresnel ramp is most of what makes water read as water. The
    /// shader already computes the right amount (`f0·ab.x + ab.y`, with `f0` carrying metallic),
    /// so gating it on metallic a second time both double-counted metals and zeroed everything
    /// else. This is now purely an artistic dial for dropping BELOW physical.
    pub reflect: f32,
    /// Cached PNG+base64 encoding for the sidecar. `rgba` never changes after creation, so the
    /// (expensive) encode is computed once on the first save and reused — otherwise EVERY save
    /// re-PNG-encoded every texture on the main thread (a visible save-time lag spike).
    pub png_cache: std::cell::RefCell<Option<String>>,
    /// `Some` when this is a PROCEDURAL material (evaluated in-shader from world position) rather
    /// than a pasted image. The `rgba` above is then just a 1×1 fallback swatch.
    pub proc: Option<ProcDef>,
    /// Texture Phase 2 — PBR maps. Index of another texture used as a tangent-space NORMAL map, and
    /// one used as a ROUGHNESS map (its red channel). `roughness` is the scalar fallback (0 = glossy,
    /// 1 = matte). All `None`/default → an ordinary albedo texture (unchanged).
    pub normal_map: Option<usize>,
    pub rough_map: Option<usize>,
    /// Phase 3 — the rest of a downloaded PBR texture set. [`load_texture_set`] fills all four from
    /// one folder by reading the filenames.
    pub metal_map: Option<usize>,
    pub ao_map: Option<usize>,
    /// Map this material in WORLD space at `tiles_per_m` instead of through the mesh's UVs. On for
    /// architecture, which is extruded from a 2D plan and has no UVs worth the name; off for
    /// imported furniture, which usually does.
    pub triplanar: bool,
    /// How many times the image repeats per metre of surface. This is what gives a texture a
    /// physical SIZE: 2.0 means a 0.5 m tile, whatever the geometry it lands on.
    pub tiles_per_m: f32,
    pub roughness: f32,
    /// TRANSMISSION — this material is a MEDIUM light travels through (water, solid glass), not a
    /// surface with holes in it. 0 = not a medium, which is everything else.
    ///
    /// The distinction is not pedantry, it is what tells the renderer the surface belongs to a
    /// VOLUME with an entry face and an exit face. Modelled water is a closed box sitting in a pool
    /// liner, so five of its six faces are exactly coplanar with the tiles — drawing those back
    /// faces put two coincident surfaces in a per-pixel z-fight that read as triangular wedges
    /// crawling over the water. Coverage transparency must NOT be treated this way: cull a leaf
    /// card's back face and the tree vanishes when you walk round it.
    pub transmission: f32,
    /// Principled BSDF parameters authored in the Materials Factory node editor. `metallic` drives the
    /// raster glossy sheen; all four are read by the path tracer. Defaults (0, 1.5, black, 0) reproduce
    /// the previous plastic look, so existing materials are unchanged until edited.
    pub metallic: f32,
    pub ior: f32,
    pub emission: [f32; 3],
    pub emission_strength: f32,
    /// CLEARCOAT — a thin varnish with its own smooth specular lobe, reflecting about the
    /// GEOMETRIC normal so it stays glassy over a grain or a bump map. This is the difference
    /// between oiled timber and lacquered timber, and it is not expressible by roughness alone:
    /// one surface is rough and one is smooth AT THE SAME TIME.
    pub clearcoat: f32,
    pub clearcoat_rough: f32,
    /// SHEEN — the pale grazing-angle rim of fabric. 0 = none.
    pub sheen: f32,
    /// The colour of the fuzz, usually near-white even on a dark cloth — which is most of why
    /// black velvet reads as velvet rather than as black plastic.
    pub sheen_tint: [f32; 3],
}

impl TextureAsset {
    /// Build from raw RGBA8 (as `arboard` hands it over), computing the average colour.
    pub fn new(name: String, w: u32, h: u32, rgba: Vec<u8>) -> Self {
        let mut sum = [0.0f64; 3];
        let mut n = 0.0f64;
        for px in rgba.chunks_exact(4) {
            // Weight by alpha so transparent padding doesn't wash the tint toward black.
            let a = px[3] as f64 / 255.0;
            sum[0] += px[0] as f64 * a;
            sum[1] += px[1] as f64 * a;
            sum[2] += px[2] as f64 * a;
            n += a;
        }
        let avg = if n > 0.0 {
            [
                (sum[0] / n / 255.0) as f32,
                (sum[1] / n / 255.0) as f32,
                (sum[2] / n / 255.0) as f32,
            ]
        } else {
            [0.8, 0.8, 0.82]
        };
        Self { name, w, h, rgba, avg, scale: 1.0, offset: [0.0, 0.0], rot_deg: 0.0, opacity: 1.0, reflect: 1.0, png_cache: std::cell::RefCell::new(None), proc: None, normal_map: None, rough_map: None, metal_map: None, ao_map: None, triplanar: false, tiles_per_m: 1.0, roughness: 0.5, transmission: 0.0, metallic: 0.0, ior: 1.5, emission: [0.0, 0.0, 0.0], emission_strength: 0.0, clearcoat: 0.0, clearcoat_rough: 0.1, sheen: 0.0, sheen_tint: [1.0; 3] }
    }

    /// Build a PROCEDURAL texture from a [`ProcDef`]. Carries a 1×1 fallback swatch (the ramp
    /// midpoint) so non-shader paths (avg tint, thumbnails, old renderers) still show a colour.
    pub fn procedural(name: String, def: ProcDef) -> Self {
        let c = def.avg_color();
        let px = [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, 255];
        Self {
            name,
            w: 1,
            h: 1,
            rgba: px.to_vec(),
            avg: c,
            scale: 1.0,
            offset: [0.0, 0.0],
            rot_deg: 0.0,
            opacity: 1.0,
            reflect: 1.0,
            png_cache: std::cell::RefCell::new(None),
            proc: Some(def),
            normal_map: None,
            rough_map: None,
            metal_map: None,
            ao_map: None,
            triplanar: false,
            tiles_per_m: 1.0,
            roughness: 0.5,
            transmission: 0.0,
            metallic: 0.0,
            ior: 1.5,
            emission: [0.0, 0.0, 0.0],
            emission_strength: 0.0,
            clearcoat: 0.0,
            clearcoat_rough: 0.1,
            sheen: 0.0,
            sheen_tint: [1.0; 3],
        }
    }

    /// Map a base UV `(uc, vc)` through this texture's tiling / rotation / offset — the ONE place
    /// the move+rotate+scale transform lives, called by both the furniture and feature textured
    /// meshes. Rotation is about the tile centre `(0.5, 0.5)` so it spins in place; then tiling
    /// (`scale`) multiplies, then `offset` shifts.
    pub fn map_uv(&self, uc: f32, vc: f32) -> [f32; 2] {
        let a = self.rot_deg.to_radians();
        let (ca, sa) = (a.cos(), a.sin());
        let (cu, cv) = (uc - 0.5, vc - 0.5); // centre so rotation spins in place
        let (ru, rv) = (ca * cu - sa * cv, sa * cu + ca * cv);
        [
            (ru + 0.5) * self.scale + self.offset[0],
            (rv + 0.5) * self.scale + self.offset[1],
        ]
    }

    /// The renderer's Principled bundle — normal/roughness map indices plus the scalars the raster
    /// shader needs. `metallic`/`ior` used to stop at the path tracer, so a metal authored in the
    /// Materials Factory rendered as grey plastic in the viewport; they ride along now.
    pub fn pbr_params(&self) -> crate::light3d::PbrParams {
        crate::light3d::PbrParams {
            normal_idx: self.normal_map,
            rough_idx: self.rough_map,
            metal_idx: self.metal_map,
            ao_idx: self.ao_map,
            triplanar: self.triplanar,
            tiles_per_m: self.tiles_per_m,
            roughness: self.roughness,
            transmission: self.transmission,
            metallic: self.metallic,
            ior: self.ior,
            // Pre-multiplied by strength and decoded to linear here, so the shader can simply add
            // it. An emissive material glowed only in the path tracer before this.
            emission: {
                let e = crate::color::srgb_to_linear3(self.emission);
                let k = self.emission_strength;
                [e[0] * k, e[1] * k, e[2] * k]
            },
            clearcoat: self.clearcoat,
            clearcoat_rough: self.clearcoat_rough,
            sheen: self.sheen,
            // Decoded here, like every other authored colour: the shader adds it to linear light.
            sheen_tint: crate::color::srgb_to_linear3(self.sheen_tint),
        }
    }
    /// Whether anything here differs from the plain-dielectric default (so the app only ships PBR
    /// params when they would change the picture).
    pub fn has_pbr(&self) -> bool {
        self.normal_map.is_some()
            || self.rough_map.is_some()
            || self.metal_map.is_some()
            || self.ao_map.is_some()
            || self.triplanar
            || (self.roughness - 0.5).abs() > 1e-3
            || self.transmission > 1e-3
            || self.metallic > 1e-3
            || (self.ior - 1.5).abs() > 1e-3
            || self.emission_strength > 1e-3
            || self.clearcoat > 1e-3
            || self.sheen > 1e-3
    }

    /// PNG+base64 for the sidecar, computed once and cached (the pixels are immutable).
    pub fn encoded_png(&self) -> String {
        let mut c = self.png_cache.borrow_mut();
        if c.is_none() {
            *c = Some(encode_texture_png_b64(self.w, self.h, &self.rgba));
        }
        c.clone().unwrap()
    }
}

/// Encode a flat `f32` slice as base64 of deflated little-endian bytes — compact, and (unlike a
/// JSON number array) near-instant to parse for multi-million-vertex furniture geometry.
pub fn encode_f32_blob(floats: &[f32]) -> String {
    use base64::Engine;
    let mut bytes = Vec::with_capacity(floats.len() * 4);
    for f in floats {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    // Level 1 — fast; the geometry is already float-dense, so higher levels barely shrink it.
    let comp = miniz_oxide::deflate::compress_to_vec(&bytes, 1);
    base64::engine::general_purpose::STANDARD.encode(comp)
}

/// Decode a blob written by [`encode_f32_blob`] back into `f32`s (empty on any error).
pub fn decode_f32_blob(s: &str) -> Vec<f32> {
    use base64::Engine;
    let Ok(comp) = base64::engine::general_purpose::STANDARD.decode(s.as_bytes()) else { return Vec::new() };
    let Ok(bytes) = miniz_oxide::inflate::decompress_to_vec(&comp) else { return Vec::new() };
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Flatten `[f32; 3]` vertices to a contiguous `f32` list (a plain memcpy, fast even for millions).
fn flat3(v: &[[f32; 3]]) -> Vec<f32> {
    let mut o = Vec::with_capacity(v.len() * 3);
    for p in v { o.extend_from_slice(p); }
    o
}
/// Flatten `[f32; 2]` UVs to a contiguous `f32` list.
fn flat2(v: &[[f32; 2]]) -> Vec<f32> {
    let mut o = Vec::with_capacity(v.len() * 2);
    for p in v { o.extend_from_slice(p); }
    o
}

/// One furniture mesh's geometry, FLATTENED but NOT yet compressed. Handed to a save worker so the
/// expensive deflate runs off the UI thread; see [`FactoryState::furniture_geom_flat`].
pub struct FurnitureGeomRaw {
    pub pos: Vec<f32>,
    pub nrm: Vec<f32>,
    pub uv: Vec<f32>,
    /// Per-vertex opacity (empty ⇒ opaque). Compressed off-thread like the rest of the geometry.
    pub alpha: Vec<f32>,
}

/// PNG-encode RGBA8 pixels and base64 the result, for compact sidecar storage.
pub fn encode_texture_png_b64(w: u32, h: u32, rgba: &[u8]) -> String {
    use base64::Engine;
    use image::ImageEncoder;
    let mut png = Vec::new();
    let enc = image::codecs::png::PngEncoder::new(&mut png);
    if enc.write_image(rgba, w, h, image::ExtendedColorType::Rgba8).is_err() {
        return String::new();
    }
    base64::engine::general_purpose::STANDARD.encode(&png)
}

/// Decode a persisted [`crate::simlux_io::TextureRec`] back into a [`TextureAsset`].
pub fn decode_texture_rec(r: &crate::simlux_io::TextureRec) -> Option<TextureAsset> {
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD.decode(r.png_b64.as_bytes()).ok()?;
    let img = image::load_from_memory(&png).ok()?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    let mut a = TextureAsset::new(r.name.clone(), w, h, img.into_raw());
    a.scale = if r.scale > 0.0 { r.scale } else { 1.0 };
    a.offset = r.offset;
    a.rot_deg = r.rot_deg;
    a.opacity = if r.opacity > 0.0 { r.opacity.clamp(0.01, 1.0) } else { 1.0 };
    // A stored 0 is an OLD sidecar's "matte", written when 0 was the default and nothing but a
    // metal ever got anything else. Read it as physical, or every project saved before this would
    // reload with its reflections switched off. The UI can no longer author 0 (it floors at 0.01),
    // so 0 unambiguously means "from before", not "the user asked for none".
    a.reflect = if r.reflect <= 0.0 { 1.0 } else { r.reflect.clamp(0.01, 1.0) };
    // The sidecar ALREADY holds this texture's PNG — reuse it as the cache so a re-save of a
    // loaded project doesn't re-encode every image (that was the save-time lag spike).
    *a.png_cache.borrow_mut() = Some(r.png_b64.clone());
    if let Some(p) = &r.proc {
        a.proc = Some(ProcDef {
            pattern: ProcPattern::from_tag(&p.pattern),
            col_a: p.col_a,
            col_b: p.col_b,
            scale: p.scale,
            detail: p.detail,
            rough: p.rough,
            contrast: p.contrast,
            ramp: p.ramp,
            // Phase 3 fields. A sidecar written before them loads as a UNIFORM finish, i.e. the
            // material renders exactly as it did — the serde defaults are 0, and 0/0 means "use the
            // material's scalar roughness", which is what it used to do.
            surf_rough: p.surf_rough,
            bump: p.bump,
        });
    }
    a.normal_map = r.normal_map;
    a.rough_map = r.rough_map;
    a.roughness = if r.roughness > 0.0 { r.roughness.clamp(0.0, 1.0) } else { 0.5 };
    a.metallic = r.metallic.clamp(0.0, 1.0);
    a.ior = if r.ior > 0.0 { r.ior.clamp(1.0, 4.0) } else { 1.5 };
    a.emission = r.emission;
    a.emission_strength = r.emission_strength.max(0.0);
    a.transmission = r.transmission.clamp(0.0, 1.0);
    Some(a)
}

impl FurnitureInst {
    /// World rotation matrix (X→Y→Z Euler). Applied to a scaled local point/normal.
    pub fn rot_mat(&self) -> glam::Mat3 {
        glam::Mat3::from_euler(
            glam::EulerRot::XYZ,
            self.rot[0].to_radians(),
            self.rot[1].to_radians(),
            self.rot[2].to_radians(),
        )
    }

    /// Per-axis scale actually applied to the local mesh: the non-uniform `fit` (apertures) if
    /// present, else uniform `scale`. `scale` always multiplies on top so the scale gizmo works
    /// for both. This is the ONE place scale is resolved — every pose site calls it.
    pub fn scale_vec(&self) -> glam::Vec3 {
        match self.fit {
            Some(f) => glam::Vec3::new(f[0], f[1], f[2]) * self.scale,
            None => glam::Vec3::splat(self.scale),
        }
    }
}

/// The two built-in aperture types placed into wall openings. Each maps to a bundled default
/// mesh under `assets/apertures/` and to a slot in [`FactoryState::aperture_asset`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ApertureKind {
    Door,
    Window,
}

impl ApertureKind {
    pub fn idx(self) -> usize {
        match self { ApertureKind::Door => 0, ApertureKind::Window => 1 }
    }
    pub fn label(self) -> &'static str {
        match self { ApertureKind::Door => "Door", ApertureKind::Window => "Window" }
    }
    /// Bundled default mesh, relative to the working dir. Both use the app's own OBJ/FBX readers.
    pub fn asset_path(self) -> &'static str {
        match self {
            ApertureKind::Door => "assets/apertures/door.fbx",
            ApertureKind::Window => "assets/apertures/window.obj",
        }
    }
}

/// One level of the building.
///
/// **`base_z` is deliberately NOT stored** — it is derived by summing the heights of the
/// storeys below (see [`FactoryState::storey_base_z`]). Storing it would let the stack
/// drift out of contiguity: change one height and every stored base below it becomes a
/// lie that nothing forces you to fix. Derived, the stack is contiguous by construction.
///
/// A storey likewise does not hold a list of the features on it. Membership is derived
/// from the z band (see [`FactoryState::features_on_storey`]) because `rederive_wall`
/// drops and rebuilds a wall's Boxes with FRESH ids — any stored id list would silently
/// rot on the first vertex edit.
#[derive(Clone, Debug, PartialEq)]
pub struct Storey {
    pub name: String,
    /// Floor-to-floor height, metres. Always > 0.
    pub height: f32,
}

/// The live parameter set for the Draw3D dialog.
///
/// ONE struct holds every primitive's controllers (rather than one per shape) so
/// switching kinds keeps what you already typed — set a radius on Cylinder, switch
/// to Cone, and the radius carries over. Fields are named for the CONTROLLER, and
/// several are deliberately shared across shapes (`r`, `h`, `segments`).
#[derive(Clone, Debug)]
pub struct Draw3dDialog {
    pub kind: Draw3dKind,
    // lengths
    pub w: f32,
    pub d: f32,
    pub h: f32,
    pub r: f32,
    pub r_top: f32,
    pub r_inner: f32,
    pub major_r: f32,
    pub minor_r: f32,
    pub rx: f32,
    pub ry: f32,
    pub rz: f32,
    // tessellation (accuracy controllers)
    pub segments: u32,
    pub stacks: u32,
    pub sides: u32,
    pub seg_major: u32,
    pub seg_minor: u32,
}

impl Default for Draw3dDialog {
    fn default() -> Self {
        Self {
            kind: Draw3dKind::Box,
            w: 2.0,
            d: 2.0,
            h: 1.0,
            r: 1.0,
            r_top: 0.0,
            r_inner: 0.6,
            major_r: 2.0,
            minor_r: 0.5,
            rx: 1.0,
            ry: 1.5,
            rz: 0.75,
            segments: 32,
            stacks: 16,
            sides: 6,
            seg_major: 32,
            seg_minor: 16,
        }
    }
}

impl Draw3dDialog {
    pub fn new(kind: Draw3dKind) -> Self {
        Self { kind, ..Default::default() }
    }

    /// Load the controllers FROM an existing primitive — the inverse of `build()` — so
    /// selecting a solid shows its real dimensions. The Frustum family (cone / prism /
    /// pyramid / frustum) is disambiguated the same way `Primitive::kind_label` does,
    /// by `r_top` and `sides`. Fields are set to match how `build()` reads them (e.g.
    /// cone/cylinder/tube take their facet count from `segments`, prism/pyramid from
    /// `sides`), so a load→build round-trip is stable.
    pub fn load_from(&mut self, p: &Primitive) {
        match *p {
            // An extrusion has no controllers here — its shape is a stored profile, not a
            // set of scalars. It is never bound for editing (see the edit-binding guard in
            // `app.rs`), so this arm exists only for exhaustiveness and must not touch the
            // dialog: writing a kind here would misreport the solid.
            Primitive::Extrusion { .. } => {}
            // A sweep is a profile + path in the shared tables, not a set of scalars —
            // nothing to load into the dialog (same as Extrusion).
            Primitive::Sweep { .. } => {}
            Primitive::Box { w, d, h } => {
                self.kind = Draw3dKind::Box;
                self.w = w; self.d = d; self.h = h;
            }
            Primitive::Sphere { r, segments, stacks } => {
                self.kind = Draw3dKind::Sphere;
                self.r = r; self.segments = segments; self.stacks = stacks;
            }
            Primitive::Cylinder { r, h, sides } => {
                self.kind = Draw3dKind::Cylinder;
                self.r = r; self.h = h; self.segments = sides;
            }
            Primitive::Frustum { r_bottom, r_top, h, sides } => {
                self.r = r_bottom; self.r_top = r_top; self.h = h;
                self.sides = sides; self.segments = sides;
                self.kind = if r_top <= 1e-6 {
                    if sides == 4 { Draw3dKind::Pyramid } else { Draw3dKind::Cone }
                } else if (r_top - r_bottom).abs() <= 1e-6 {
                    Draw3dKind::Prism
                } else {
                    Draw3dKind::Cone // a true frustum edits via the cone controllers (bottom/top/height)
                };
            }
            Primitive::Torus { major_r, minor_r, seg_major, seg_minor } => {
                self.kind = Draw3dKind::Torus;
                self.major_r = major_r; self.minor_r = minor_r;
                self.seg_major = seg_major; self.seg_minor = seg_minor;
            }
            Primitive::Capsule { r, h, segments, stacks } => {
                self.kind = Draw3dKind::Capsule;
                self.r = r; self.h = h; self.segments = segments; self.stacks = stacks;
            }
            Primitive::Tube { r_outer, r_inner, h, sides } => {
                self.kind = Draw3dKind::Tube;
                self.r = r_outer; self.r_inner = r_inner; self.h = h; self.segments = sides;
            }
            Primitive::Ellipsoid { rx, ry, rz, segments, stacks } => {
                self.kind = Draw3dKind::Ellipsoid;
                self.rx = rx; self.ry = ry; self.rz = rz;
                self.segments = segments; self.stacks = stacks;
            }
        }
    }

    /// Build the primitive from the current controllers.
    ///
    /// Cone / Prism / Pyramid all map onto ONE `Primitive::Frustum` — they are the
    /// same solid with different controllers (`r_top = 0` → cone; `r_top = r` →
    /// prism; 4 sides + `r_top = 0` → pyramid). Keeping them as separate MENU items
    /// but one primitive is why there is no duplicated meshing code.
    pub fn build(&self) -> Primitive {
        match self.kind {
            Draw3dKind::Box => Primitive::Box { w: self.w, d: self.d, h: self.h },
            Draw3dKind::Sphere => {
                Primitive::Sphere { r: self.r, segments: self.segments, stacks: self.stacks }
            }
            Draw3dKind::Cylinder => {
                Primitive::Cylinder { r: self.r, h: self.h, sides: self.segments }
            }
            Draw3dKind::Cone => Primitive::Frustum {
                r_bottom: self.r,
                r_top: self.r_top,
                h: self.h,
                sides: self.segments,
            },
            Draw3dKind::Prism => Primitive::Frustum {
                r_bottom: self.r,
                r_top: self.r, // equal radii ⇒ a prism
                h: self.h,
                sides: self.sides,
            },
            Draw3dKind::Pyramid => Primitive::Frustum {
                r_bottom: self.r,
                r_top: 0.0, // apex
                h: self.h,
                sides: self.sides,
            },
            Draw3dKind::Capsule => Primitive::Capsule {
                r: self.r,
                h: self.h,
                segments: self.segments,
                stacks: self.stacks,
            },
            Draw3dKind::Torus => Primitive::Torus {
                major_r: self.major_r,
                minor_r: self.minor_r,
                seg_major: self.seg_major,
                seg_minor: self.seg_minor,
            },
            Draw3dKind::Tube => Primitive::Tube {
                r_outer: self.r,
                r_inner: self.r_inner,
                h: self.h,
                sides: self.segments,
            },
            Draw3dKind::Ellipsoid => Primitive::Ellipsoid {
                rx: self.rx,
                ry: self.ry,
                rz: self.rz,
                segments: self.segments,
                stacks: self.stacks,
            },
        }
    }

    /// Validity + the reason, shown live in the dialog so Create is never a
    /// guess (e.g. a tube whose bore is wider than its wall isn't a tube).
    pub fn problem(&self) -> Option<&'static str> {
        match self.kind {
            Draw3dKind::Tube if self.r_inner >= self.r => {
                Some("inner radius must be smaller than outer")
            }
            Draw3dKind::Torus if self.minor_r >= self.major_r => {
                Some("minor radius must be smaller than major (else it self-intersects)")
            }
            Draw3dKind::Cone if self.r_top >= self.r => Some("top radius must be < bottom (0 = cone)"),
            _ => None,
        }
    }
}

impl Default for FactoryState {
    fn default() -> Self {
        Self {
            open: false,
            model: Model::default(),
            cached: SolidMesh::default(),
            dirty: false,
            selection: Vec::new(),
            feature_group: std::collections::HashMap::new(),
            next_group_id: 1,
            cam_yaw: 0.9,
            cam_pitch: 0.5,
            cam_dist: 12.0,
            cam_target: [0.0, 0.0, 0.0],
            ortho: false,
            session: None,
            pending_face: None,
            sketch_ref: Vec::new(),
            box_w: 2.0,
            box_d: 2.0,
            box_h: 1.0,
            cyl_r: 0.5,
            cyl_h: 2.0,
            cyl_sides: 24,
            draw3d: None,
            draw3d_edit: None,
            place_pending: None,
            wall_height: 2.7,
            wall_thickness: 0.2,
            walls: Vec::new(),
            building_height: 3.0,
            room_height: 2.7,
            room_floor: 0.2,
            room_open_top: false,
            element_height: 1.0,
            keep_sketch: false,
            // One storey at z = 0 — with a single level everything behaves exactly as it
            // did before storeys existed.
            storeys: vec![Storey { name: "Ground".into(), height: 3.0 }],
            active_storey: 0,
            wall_drag: None,
            gizmo_drag: None,
            gizmo_grab_ground: None,
            gizmo_start_center: Vec3::ZERO,
            gizmo_mode: GizmoMode::Move,
            rot_drag: None,
            dim_edit_active: false,
            show_plan: true,
            ceilings: std::collections::HashSet::new(),
            ceiling_caps: std::collections::HashSet::new(),
            hide_ceilings: false,
            cutaway: false,
            cutaway_z: 2.5,
            ceiling_thickness: 0.15,
            marquee: None,
            furniture_lib: Vec::new(),
            furniture: Vec::new(),
            sel_furniture: Vec::new(),
            sweep_flow: None,
            render_cache: std::cell::RefCell::new(RenderCache::default()),
            env_map: None,
            env_chain: Vec::new(),
            env_sh: [[0.0; 3]; 9],
            env_strength: 1.0,
            env_rot_deg: 0.0,
            env_version: 0,
            face_outline: std::cell::RefCell::new(None),
            faceted_cache: std::cell::RefCell::new(std::collections::HashMap::new()),
            geom_version: 0,
            sun: SunEnv::default(),
            color: crate::color::ColorPipeline::default(),
            // 16 samples: about a quarter of a second at 60 fps, and past the point where more
            // samples are visible on geometry edges. High enough to matter, short enough that the
            // refinement is over before you have finished letting go of the mouse.
            taa_samples: 16,
            clay_mode: false,
            feature_color: std::collections::HashMap::new(),
            surface_color: std::collections::HashMap::new(),
            paint_surface_mode: false,
            last_pick_color: [0.8, 0.8, 0.82],
            textures: Vec::new(),
            feature_texture: std::collections::HashMap::new(),
            surface_texture: std::collections::HashMap::new(),
            surface_tex_brush: None,
            furn_paint_mode: FurnPaintMode::WholeObject,
            furn_tex_brush: None,
            furn_face_sel: None,
            clip: None,
            zoom_mode: ZoomMode::Off,
            zoom_drag: None,
            zoom_cur: None,
            cam_prev: None,
            zoom_rt_before: None,
            modify: None,
            queued: None,
            status: String::new(),
            editing_cutout: false,
            aperture_asset: [None, None],
            sel_mesh: SolidMesh::default(),
            sel_key: Vec::new(),
        }
    }
}

impl FactoryState {
    pub fn add_box(&mut self) {
        let p = Primitive::Box { w: self.box_w, d: self.box_d, h: self.box_h };
        // Built on the ACTIVE storey, like every other new solid.
        let placement = Placement { lift: self.active_base_z(), ..Placement::default() };
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.selection = vec![id];
        self.dirty = true;
    }

    // ===================================================================
    // Storeys — the building's levels
    // ===================================================================

    /// Z the floor of storey `i` sits at: the sum of the heights below it. Derived, so
    /// the stack is contiguous by construction and cannot drift.
    pub fn storey_base_z(&self, i: usize) -> f32 {
        self.storeys.iter().take(i).map(|s| s.height).sum()
    }

    /// Z that new geometry is built on.
    pub fn active_base_z(&self) -> f32 {
        self.storey_base_z(self.active_storey.min(self.storeys.len().saturating_sub(1)))
    }

    /// Total height of the building.
    pub fn building_total_height(&self) -> f32 {
        self.storeys.iter().map(|s| s.height).sum()
    }

    /// Feature ids whose origin lies in storey `i`'s z band — `[base, base + height)`,
    /// with the TOP storey's band closed at the top so geometry standing exactly on the
    /// roof line still belongs to something.
    ///
    /// Derived rather than tracked: see [`Storey`].
    pub fn features_on_storey(&self, i: usize) -> Vec<u32> {
        if i >= self.storeys.len() {
            return Vec::new();
        }
        let base = self.storey_base_z(i);
        let top = base + self.storeys[i].height;
        let is_top = i + 1 == self.storeys.len();
        self.model
            .features
            .iter()
            .filter(|f| {
                let z = f.world_origin().z;
                z >= base - Z_EPS && (z < top - Z_EPS || (is_top && z <= top + Z_EPS))
            })
            .map(|f| f.id)
            .collect()
    }

    /// Move every feature whose origin is at or above `from_z` by `dz`, and carry the
    /// walls' `base_z` with them. Used when a storey is inserted, deleted or resized —
    /// everything above has to follow, or the stack tears apart.
    fn shift_above(&mut self, from_z: f32, dz: f32) {
        if dz == 0.0 {
            return;
        }
        let ids: Vec<u32> = self
            .model
            .features
            .iter()
            .filter(|f| f.world_origin().z >= from_z - Z_EPS)
            .map(|f| f.id)
            .collect();
        for id in ids {
            if let Some(f) = self.model.get_mut(id) {
                *f = f.translated(Vec3::new(0.0, 0.0, dz));
            }
        }
        for w in &mut self.walls {
            if w.base_z >= from_z - Z_EPS {
                w.base_z += dz;
            }
        }
        self.dirty = true;
    }

    /// Add a storey directly above storey `i` and make it active. Everything above `i`
    /// moves up by the new storey's height so nothing is left overlapping.
    pub fn insert_storey_above(&mut self, i: usize, name: String, height: f32) -> usize {
        let h = height.max(MIN_STOREY_H);
        let i = i.min(self.storeys.len().saturating_sub(1));
        let top_of_i = self.storey_base_z(i) + self.storeys[i].height;
        self.shift_above(top_of_i, h);
        self.storeys.insert(i + 1, Storey { name, height: h });
        self.active_storey = i + 1;
        i + 1
    }

    /// Append a storey on top of the building and make it active.
    pub fn add_storey_on_top(&mut self) -> usize {
        let n = self.storeys.len();
        let h = self.storeys.last().map_or(self.building_height, |s| s.height);
        self.storeys.push(Storey { name: format!("Level {n}"), height: h.max(MIN_STOREY_H) });
        self.active_storey = self.storeys.len() - 1;
        self.active_storey
    }

    /// Duplicate the ACTIVE storey's geometry onto a new level directly above it — "add a
    /// floor to my building". Copies the buildings, walls and solids standing on the level
    /// (not slabs, which belong to the level beneath) up by the storey height, and makes
    /// the new level active. Returns the new storey index, or `None` if the level is empty.
    pub fn duplicate_storey_up(&mut self) -> Option<usize> {
        let src = self.active_storey.min(self.storeys.len().saturating_sub(1));
        let base_src = self.storey_base_z(src);
        let feat_ids = self.features_on_storey(src);
        let src_walls: Vec<WallInst> = self
            .walls
            .iter()
            .filter(|w| (w.base_z - base_src).abs() < Z_EPS)
            .cloned()
            .collect();
        if feat_ids.is_empty() && src_walls.is_empty() {
            return None;
        }
        let dz = self.storeys[src].height;
        // A new level above src (shifts anything higher up to make room).
        let dst = self.insert_storey_above(src, format!("Level {}", self.storeys.len()), dz);
        let new_base = self.storey_base_z(dst);

        // Copy the solids up.
        let feats: Vec<Feature> = feat_ids
            .iter()
            .filter_map(|id| self.model.features.iter().find(|f| f.id == *id).cloned())
            .collect();
        for f in feats {
            let nf = f.translated(Vec3::new(0.0, 0.0, dz));
            self.model.push_feature(nf);
        }
        // Copy the walls up (fresh segment ids, new base).
        for w in src_walls {
            let mut segs = Vec::new();
            for win in w.footprint.windows(2) {
                if let Some(id) = self.push_wall_box(win[0], win[1], w.thickness, w.height, new_base) {
                    segs.push(id);
                }
            }
            if !segs.is_empty() {
                self.walls.push(WallInst {
                    footprint: w.footprint.clone(),
                    segments: segs,
                    thickness: w.thickness,
                    height: w.height,
                    rake_deg: w.rake_deg,
                    base_z: new_base,
                });
            }
        }
        self.active_storey = dst;
        self.dirty = true;
        Some(dst)
    }

    /// Delete storey `i`, ERASING the geometry standing on it and dropping everything
    /// above down by its height. Returns false — changing nothing — when this is the last
    /// storey: a building always has at least one level, and an empty `storeys` would
    /// make `active_storey` index nothing.
    pub fn delete_storey(&mut self, i: usize) -> bool {
        if self.storeys.len() <= 1 || i >= self.storeys.len() {
            return false;
        }
        for id in self.features_on_storey(i) {
            self.model.remove(id);
        }
        let base = self.storey_base_z(i);
        let h = self.storeys[i].height;
        // Walls that stood on the deleted level go with it. Walls above are left alone
        // here — `shift_above` below brings them down.
        self.walls.retain(|w| (w.base_z - base).abs() > Z_EPS);
        self.storeys.remove(i);
        self.shift_above(base + h, -h);
        self.active_storey = self.active_storey.min(self.storeys.len() - 1);
        self.clear_selection();
        self.dirty = true;
        true
    }

    /// Change storey `i`'s height. Everything ABOVE it moves by the difference so the
    /// stack stays contiguous; the geometry ON the storey keeps its own height (a taller
    /// level does not stretch its walls).
    pub fn set_storey_height(&mut self, i: usize, height: f32) {
        if i >= self.storeys.len() {
            return;
        }
        let h = height.max(MIN_STOREY_H);
        let old = self.storeys[i].height;
        if (h - old).abs() < f32::EPSILON {
            return;
        }
        let top = self.storey_base_z(i) + old;
        self.storeys[i].height = h;
        self.shift_above(top, h - old);
    }

    // ===================================================================
    // Furniture — imported OBJ meshes
    // ===================================================================

    /// Add a parsed OBJ mesh to the project library. Auto-normalises very large or very
    /// small meshes toward a ~1 m size (many OBJ exports use cm or mm), and re-seats it so
    /// its base sits on z = 0. Returns the library index.
    pub fn add_furniture_asset(&mut self, name: String, mesh: crate::mesh_io::ObjMesh) -> usize {
        let asset_color = mesh.color.unwrap_or([0.82, 0.82, 0.84]); // file diffuse, else neutral
        let mut positions = mesh.positions;
        let normals = mesh.normals;
        let alpha = mesh.alpha; // per-vertex opacity (empty ⇒ opaque); recentring below is xyz-only
        let bounds = {
            let mut mn = [f32::INFINITY; 3];
            let mut mx = [f32::NEG_INFINITY; 3];
            for p in &positions {
                for k in 0..3 { mn[k] = mn[k].min(p[k]); mx[k] = mx[k].max(p[k]); }
            }
            mn[0].is_finite().then_some((mn, mx))
        };
        let mut import_scale = 1.0f32;
        if let Some((mn, mx)) = bounds {
            let size = [(mx[0] - mn[0]), (mx[1] - mn[1]), (mx[2] - mn[2])];
            let longest = size[0].max(size[1]).max(size[2]).max(1e-4);
            // Scale toward ~1.5 m only for wildly off sizes (cm/mm exports, or giant units).
            let k = if longest > 20.0 || longest < 0.05 { 1.5 / longest } else { 1.0 };
            import_scale = k;
            for p in &mut positions {
                p[0] = (p[0] - (mn[0] + mx[0]) * 0.5) * k; // centre X
                p[1] = (p[1] - (mn[1] + mx[1]) * 0.5) * k; // centre Y
                p[2] = (p[2] - mn[2]) * k;                  // base on z = 0
            }
        }
        let mut fa = FurnitureAsset::new(name, positions, normals, asset_color);
        fa.import_scale = import_scale;
        fa.alpha = alpha;
        self.furniture_lib.push(fa);
        self.furniture_lib.len() - 1
    }

    /// A sensible world point to drop a new furniture copy: the CENTRE of the current
    /// model. Imported drawings live at their DXF coordinates (e.g. X≈3619, Y≈956), so
    /// placing at world origin puts the piece kilometres away, off-screen — it looks like
    /// nothing happened. Falls back to the origin when there is no model yet.
    pub fn default_place_at(&self) -> Vec3 {
        match self.cached.bounds() {
            Some((mn, mx)) => Vec3::new((mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, 0.0),
            None => Vec3::ZERO,
        }
    }

    /// Copy the current 3D selection (a furniture instance OR a single CSG feature) into the
    /// paste buffer, with its colour/texture. Returns true if something was captured.
    pub fn copy_selection(&mut self) -> bool {
        // The PRIMARY only: the paste buffer holds one object, and copying a multi-selection would
        // have to silently pick one of them anyway.
        if let Some(fi) = self.sel_furn_primary() {
            if let Some(inst) = self.furniture.get(fi).cloned() {
                self.clip = Some(FactoryClip::Furniture(inst));
                return true;
            }
        }
        if let Some(id) = self.selected_single() {
            if let Some(f) = self.model.features.iter().find(|f| f.id == id) {
                let (op, plane, placement, primitive) =
                    (f.op, f.plane.clone(), f.placement.clone(), f.primitive.clone());
                let (surface_colors, surface_textures) = self.capture_surface_paint(id);
                self.clip = Some(FactoryClip::Feature {
                    op,
                    plane,
                    placement,
                    primitive,
                    color: self.feature_color.get(&id).copied(),
                    texture: self.feature_texture.get(&id).copied(),
                    surface_colors,
                    surface_textures,
                });
                return true;
            }
        }
        false
    }

    /// Capture feature `id`'s PER-FACE paint as precise `(normal, plane-offset d, value)` tuples,
    /// one per painted face. Walks the cached geometry (which is why it uses the real triangle
    /// normal + offset, not the quantised `SurfaceKey`): storing the exact plane lets paste re-key
    /// each face after its offset shifts by `n·delta`, so per-face paint survives copy/paste.
    fn capture_surface_paint(
        &self,
        id: u32,
    ) -> (Vec<([f32; 3], f32, [f32; 3])>, Vec<([f32; 3], f32, usize)>) {
        if self.surface_color.is_empty() && self.surface_texture.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let mut cols: std::collections::HashMap<SurfaceKey, ([f32; 3], f32, [f32; 3])> =
            std::collections::HashMap::new();
        let mut texs: std::collections::HashMap<SurfaceKey, ([f32; 3], f32, usize)> =
            std::collections::HashMap::new();
        for (i, tri) in self.cached.positions.chunks_exact(3).enumerate() {
            let Some(fid) = self.cached.face_ids.get(i).copied() else { continue };
            if fid != id {
                continue;
            }
            let key = surface_key(fid, tri[0], tri[1], tri[2]);
            let av = Vec3::from(tri[0]);
            let n = (Vec3::from(tri[1]) - av).cross(Vec3::from(tri[2]) - av).normalize_or_zero();
            let d = n.dot(av);
            if let Some(&c) = self.surface_color.get(&key) {
                cols.entry(key).or_insert(([n.x, n.y, n.z], d, c));
            }
            if let Some(&t) = self.surface_texture.get(&key) {
                texs.entry(key).or_insert(([n.x, n.y, n.z], d, t));
            }
        }
        (cols.into_values().collect(), texs.into_values().collect())
    }

    /// Paste the buffer as a new object, offset so the copy is visible, and select it. Returns
    /// `Some(is_feature)` when a paste happened (`true` → caller must `recompute()`), else `None`.
    pub fn paste_clipboard(&mut self) -> Option<bool> {
        const OFF: f32 = 0.3; // metres, so the copy doesn't sit exactly on the original
        match self.clip.clone()? {
            FactoryClip::Furniture(mut inst) => {
                inst.pos[0] += OFF;
                inst.pos[1] += OFF;
                self.furniture.push(inst);
                self.select_furniture(self.furniture.len() - 1);
                Some(false)
            }
            FactoryClip::Feature {
                op, plane, mut placement, primitive, color, texture,
                surface_colors, surface_textures,
            } => {
                placement.u += OFF;
                placement.v += OFF;
                // World translation the paste applied, so each captured face's plane offset shifts
                // by n·delta and its re-keyed SurfaceKey matches the pasted geometry exactly.
                let (ua, va) = plane.axes();
                let delta = ua * OFF + va * OFF;
                let id = self.model.push(op, plane, placement, primitive);
                if let Some(c) = color { self.feature_color.insert(id, c); }
                if let Some(t) = texture { self.feature_texture.insert(id, t); }
                let rekey = |n: [f32; 3], d: f32| -> SurfaceKey {
                    let nv = Vec3::from(n);
                    let nd = d + nv.dot(delta);
                    (
                        id,
                        (n[0] * 50.0).round() as i32,
                        (n[1] * 50.0).round() as i32,
                        (n[2] * 50.0).round() as i32,
                        (nd * 100.0).round() as i32,
                    )
                };
                for (n, d, c) in surface_colors { self.surface_color.insert(rekey(n, d), c); }
                for (n, d, t) in surface_textures { self.surface_texture.insert(rekey(n, d), t); }
                self.sel_furniture.clear();
                self.selection = vec![id];
                self.dirty = true;
                Some(true)
            }
        }
    }

    /// Store a clipboard bitmap as a texture asset and return its index. Computes the
    /// average colour up front so callers can tint immediately.
    pub fn add_texture(&mut self, name: String, w: u32, h: u32, rgba: Vec<u8>) -> usize {
        self.textures.push(TextureAsset::new(name, w, h, rgba));
        self.textures.len() - 1
    }

    /// Register a PROCEDURAL texture (evaluated in-shader from world position) and return its index.
    pub fn add_procedural_texture(&mut self, name: String, def: ProcDef) -> usize {
        self.textures.push(TextureAsset::procedural(name, def));
        self.textures.len() - 1
    }

    /// Place a copy of library asset `asset` at world point `at` (seated on the plane),
    /// and select it so it can be moved/scaled immediately.
    pub fn place_furniture(&mut self, asset: usize, at: Vec3) {
        if asset >= self.furniture_lib.len() {
            return;
        }
        let color = self.furniture_lib[asset].color;
        self.furniture.push(FurnitureInst {
            asset,
            pos: [at.x, at.y, self.active_base_z()],
            scale: 1.0,
            fit: None,
            rot: [0.0, 0.0, 0.0],
            color,
            texture: None,
            surface_texture: std::collections::HashMap::new(),
            ..Default::default()
        });
        self.select_furniture(self.furniture.len() - 1);
    }

    /// Place aperture library asset `asset` (a door/window mesh) so it exactly FILLS an opening
    /// described in WORLD space: `center` is the opening's centre (at mid-wall-depth), `u_h` is
    /// the opening's horizontal in-plane axis (unit, level), and `width` / `height` / `depth` are
    /// its size (height is vertical, depth is the wall thickness). The mesh — authored with local
    /// X=width, Y=depth, Z=height — is stretched to fill and yawed so its face aligns with the
    /// wall. Returns the new instance index (also selected). Pure geometry, unit-tested.
    ///
    /// The transform is self-consistent with [`FurnitureInst::rot_mat`]/[`FurnitureInst::scale_vec`]:
    /// a local point `p` maps to `center + R·(fit·(p − localCentre))`, so the mesh's own centre
    /// lands on `center` and its extents span exactly the opening.
    pub fn place_aperture(
        &mut self,
        asset: usize,
        center: Vec3,
        u_h: Vec3,
        width: f32,
        height: f32,
        depth: f32,
    ) -> Option<usize> {
        let a = self.furniture_lib.get(asset)?;
        let (lmn, lmx) = (a.local_min, a.local_max);
        let sx = (lmx[0] - lmn[0]).max(1e-4);
        let sy = (lmx[1] - lmn[1]).max(1e-4);
        let sz = (lmx[2] - lmn[2]).max(1e-4);
        // Stretch each local axis to fill: X→width, Y→wall depth, Z→height.
        let fit = [width / sx, depth / sy, height / sz];
        let s = Vec3::new(fit[0], fit[1], fit[2]);
        // Yaw so local +X aligns to the horizontal opening axis; local +Z stays world-up.
        let theta = u_h.y.atan2(u_h.x); // radians
        let rm = glam::Mat3::from_rotation_z(theta);
        let lc = Vec3::new(
            (lmn[0] + lmx[0]) * 0.5,
            (lmn[1] + lmx[1]) * 0.5,
            (lmn[2] + lmx[2]) * 0.5,
        );
        // center = pos + R·(s·lc)  ⇒  pos = center − R·(s·lc), so the mesh centre lands on `center`.
        let pos = center - rm * (s * lc);
        let color = a.color;
        self.furniture.push(FurnitureInst {
            asset,
            pos: [pos.x, pos.y, pos.z],
            scale: 1.0,
            fit: Some(fit),
            rot: [0.0, 0.0, theta.to_degrees()],
            color,
            texture: None,
            surface_texture: std::collections::HashMap::new(),
            ..Default::default()
        });
        let i = self.furniture.len() - 1;
        self.select_furniture(i);
        Some(i)
    }

    /// Place an aperture asset at its NATIVE size (no stretch) — for a parametric door built to the
    /// opening's exact dimensions, so its mouldings and hardware aren't distorted. `anchor` is the
    /// mesh-local point (e.g. the door's structural-opening centre) that should land on `center`;
    /// the mesh is yawed so local +X → `u_h` and local +Z stays world-up (same frame as
    /// [`Self::place_aperture`], just fit = 1).
    pub fn place_aperture_native(
        &mut self,
        asset: usize,
        center: Vec3,
        u_h: Vec3,
        anchor: Vec3,
    ) -> Option<usize> {
        let a = self.furniture_lib.get(asset)?;
        let color = a.color;
        let theta = u_h.y.atan2(u_h.x);
        let rm = glam::Mat3::from_rotation_z(theta);
        let pos = center - rm * anchor; // land `anchor` on `center`
        self.furniture.push(FurnitureInst {
            asset,
            pos: [pos.x, pos.y, pos.z],
            scale: 1.0,
            fit: Some([1.0, 1.0, 1.0]),
            rot: [0.0, 0.0, theta.to_degrees()],
            color,
            texture: None,
            surface_texture: std::collections::HashMap::new(),
            ..Default::default()
        });
        let i = self.furniture.len() - 1;
        self.select_furniture(i);
        Some(i)
    }

    /// World-space vertex of instance `i`'s local mesh point — pose applied
    /// (scale → 3-axis rotate → translate).
    fn furniture_point(&self, inst: &FurnitureInst, p: [f32; 3]) -> Vec3 {
        let sv = inst.scale_vec();
        let lp = Vec3::new(p[0] * sv.x, p[1] * sv.y, p[2] * sv.z);
        inst.rot_mat() * lp + Vec3::from(inst.pos)
    }

    /// World AABB of a placed furniture instance.
    pub fn furniture_aabb(&self, i: usize) -> Option<(Vec3, Vec3)> {
        let inst = self.furniture.get(i)?;
        let asset = self.furniture_lib.get(inst.asset)?;
        // Transform the 8 corners of the asset's CACHED local box — O(8), not a per-frame
        // sweep of every vertex. The gizmo + selection highlight both call this each frame,
        // so at ~90k verts the old loop cost ~100 ms/frame while a heavy piece was selected.
        // `rot_mat()` is built ONCE here (it used to be rebuilt per vertex inside the loop).
        let rm = inst.rot_mat();
        let s = inst.scale_vec();
        let pos = Vec3::from(inst.pos);
        let (lmn, lmx) = (asset.local_min, asset.local_max);
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for cx in [lmn[0], lmx[0]] {
            for cy in [lmn[1], lmx[1]] {
                for cz in [lmn[2], lmx[2]] {
                    let w = rm * (Vec3::new(cx, cy, cz) * s) + pos;
                    mn = mn.min(w);
                    mx = mx.max(w);
                }
            }
        }
        mn.x.is_finite().then_some((mn, mx))
    }

    /// Ray-pick the front-most furniture instance under the cursor.
    pub fn pick_furniture(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<usize> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let mut best: Option<(f32, usize)> = None;
        for (i, inst) in self.furniture.iter().enumerate() {
            let Some(asset) = self.furniture_lib.get(inst.asset) else { continue };
            // CHEAP REJECT: skip furniture whose world AABB the ray misses. Without this a
            // pick ray-tested EVERY triangle of EVERY piece — a 2M-triangle mesh cost ~6M
            // vertex transforms per click (the select/drag lag spike). The AABB is O(8).
            match self.furniture_aabb(i) {
                Some((mn, mx)) if cad_solid::ray_aabb(orig, dir, mn, mx).is_some() => {}
                Some(_) => continue, // ray misses this piece entirely
                None => continue,
            }
            // Pick against the DISPLAY geometry — the decimated proxy for heavy pieces — so a
            // click on a 2M-triangle import doesn't ray-test millions of triangles per click.
            let lod = if asset.needs_lod() { Some(asset.lod_geom()) } else { None };
            let positions: &[[f32; 3]] = match &lod { Some(a) => &a.0, None => &asset.positions };
            let mut ft: Option<f32> = None;
            for tri in positions.chunks_exact(3) {
                let a = self.furniture_point(inst, tri[0]);
                let b = self.furniture_point(inst, tri[1]);
                let c = self.furniture_point(inst, tri[2]);
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                    if ft.map_or(true, |x| t < x) {
                        ft = Some(t);
                    }
                }
            }
            if let Some(t) = ft {
                if best.map_or(true, |(bt, _)| t < bt) {
                    best = Some((t, i));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// One pass over furniture returning BOTH the nearest hit of any kind AND the nearest APERTURE
    /// hit (door/window, `fit.is_some()`), each as `(index, ray_t)`. Tracking the aperture
    /// separately is what lets selection give it priority over the wall it sits in even when some
    /// other furniture happens to be the global-nearest along that ray — the single-nearest
    /// `pick_furniture` could not express that, which made aperture selection inconsistent.
    pub fn pick_furniture_ex(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> (Option<(usize, f32)>, Option<(usize, f32)>) {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let mut best: Option<(f32, usize)> = None;
        let mut best_ap: Option<(f32, usize)> = None;
        for (i, inst) in self.furniture.iter().enumerate() {
            let Some(asset) = self.furniture_lib.get(inst.asset) else { continue };
            match self.furniture_aabb(i) {
                Some((mn, mx)) if cad_solid::ray_aabb(orig, dir, mn, mx).is_some() => {}
                _ => continue,
            }
            let lod = if asset.needs_lod() { Some(asset.lod_geom()) } else { None };
            let positions: &[[f32; 3]] = match &lod { Some(a) => &a.0, None => &asset.positions };
            let mut ft: Option<f32> = None;
            for tri in positions.chunks_exact(3) {
                let a = self.furniture_point(inst, tri[0]);
                let b = self.furniture_point(inst, tri[1]);
                let c = self.furniture_point(inst, tri[2]);
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                    if ft.map_or(true, |x| t < x) { ft = Some(t); }
                }
            }
            if let Some(t) = ft {
                if best.map_or(true, |(bt, _)| t < bt) { best = Some((t, i)); }
                // Recognise apertures broadly (fit OR a door/window asset), not just `fit`, so a
                // free-standing/imported window flush in a wall still gets selection priority.
                if self.is_aperture(i) && best_ap.map_or(true, |(bt, _)| t < bt) { best_ap = Some((t, i)); }
            }
        }
        (best.map(|(t, i)| (i, t)), best_ap.map(|(t, i)| (i, t)))
    }

    /// Nearest Union feature under the cursor WITH its ray distance — the depth-aware counterpart
    /// of [`Self::pick_feature`] (which returns only the id).
    pub fn pick_feature_t(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<(u32, f32)> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let mut best: Option<(f32, f32, u32)> = None; // (t, aabb volume, id)
        for f in &self.model.features {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            if self.hide_ceilings && self.is_hidden_ceiling(f.id) {
                continue;
            }
            let tris = self.model.feature_world_positions(f);
            let mut ft: Option<f32> = None;
            for c in tris.chunks_exact(3) {
                let (a, b, cc) = (Vec3::from(c[0]), Vec3::from(c[1]), Vec3::from(c[2]));
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, cc) {
                    if ft.map_or(true, |x| t < x) { ft = Some(t); }
                }
            }
            if let Some(t) = ft {
                let (mn, mx) = f.world_aabb();
                let s = mx - mn;
                let vol = s.x.abs() * s.y.abs() * s.z.abs();
                let better = match best {
                    None => true,
                    Some((bt, bv, _)) => t < bt - 1e-3 || (t < bt + 1e-3 && vol < bv),
                };
                if better { best = Some((t, vol, f.id)); }
            }
        }
        best.map(|(t, _, id)| (id, t))
    }

    /// Depth tolerance (metres) by which an aperture `i` may sit BEHIND the nearest wall/other
    /// surface and still win the click. Scaled to the aperture's own thinnest dimension (≈ the
    /// wall thickness it fills), so a door in a thick wall is as grabbable as one in a thin wall.
    pub fn aperture_pick_tol(&self, i: usize) -> f32 {
        // Floor of 0.5 m: a free-standing window can sit recessed several cm-to-decimetres behind
        // the wall face (observed up to ~0.4 m in the pick diagnostic), and its own mesh may be
        // thin, so the tolerance can't be driven by thickness alone.
        match self.furniture_aabb(i) {
            Some((mn, mx)) => ((mx - mn).min_element() * 1.5 + 0.1).clamp(0.5, 3.0),
            None => 0.5,
        }
    }

    /// Is furniture instance `fi` in front of feature `id` along the pick ray? Used to
    /// break a tie when a click hits both. Exact depth compare (tolerance 0).
    pub fn furniture_nearer_than_feature(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16], fi: usize, id: u32,
    ) -> bool {
        self.furniture_beats_feature(cursor, rect, mvp, fi, id, 0.0)
    }

    /// Like [`Self::furniture_nearer_than_feature`] but the furniture wins as long as it is no more
    /// than `tol` metres BEHIND the feature. This is what makes an APERTURE (a door/window placed
    /// flush INSIDE a wall opening) selectable: viewed at an angle, the wall's opening reveal sits
    /// at nearly the same depth as the aperture's face and would otherwise steal every click. The
    /// caller passes a generous `tol` (≈ wall thickness) for apertures and a tiny one otherwise.
    pub fn furniture_beats_feature(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16], fi: usize, id: u32, tol: f32,
    ) -> bool {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let fur_t = self.furniture.get(fi).and_then(|inst| {
            let asset = self.furniture_lib.get(inst.asset)?;
            let lod = if asset.needs_lod() { Some(asset.lod_geom()) } else { None };
            let positions: &[[f32; 3]] = match &lod { Some(a) => &a.0, None => &asset.positions };
            let mut best: Option<f32> = None;
            for tri in positions.chunks_exact(3) {
                let a = self.furniture_point(inst, tri[0]);
                let b = self.furniture_point(inst, tri[1]);
                let c = self.furniture_point(inst, tri[2]);
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                    if best.map_or(true, |x| t < x) { best = Some(t); }
                }
            }
            best
        });
        let feat_t = self.model.features.iter().find(|f| f.id == id).and_then(|f| {
            let tris = self.model.feature_world_positions(f);
            let mut best: Option<f32> = None;
            for c in tris.chunks_exact(3) {
                let (a, b, cc) = (Vec3::from(c[0]), Vec3::from(c[1]), Vec3::from(c[2]));
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, cc) {
                    if best.map_or(true, |x| t < x) { best = Some(t); }
                }
            }
            best
        });
        match (fur_t, feat_t) {
            (Some(a), Some(b)) => a <= b + tol,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// True when furniture instance `i` is an APERTURE — a door or window that lives inside a wall
    /// and so gets selection priority over that wall. Recognised three ways, because an aperture
    /// can reach the scene by more than one route:
    ///  1. a non-uniform `fit` — set only by the "draw on a wall" flow (`place_aperture`);
    ///  2. its asset is a registered bundled door/window (`aperture_asset`);
    ///  3. its asset is NAMED "door"/"window" — covers a bundled aperture IMPORTED free-standing
    ///     (`factory_import_aperture` → `place_furniture`, no `fit`) then moved into an opening.
    /// Without (2)/(3), an imported/free-standing window sitting flush in a wall was invisible to
    /// the aperture priority and the wall stole the click (confirmed via the pick diagnostic).
    pub fn is_aperture(&self, i: usize) -> bool {
        let Some(inst) = self.furniture.get(i) else { return false };
        if inst.fit.is_some() {
            return true;
        }
        if self.aperture_asset.iter().any(|&a| a == Some(inst.asset)) {
            return true;
        }
        self.furniture_lib.get(inst.asset).map_or(false, |a| {
            let n = a.name.trim().to_ascii_lowercase();
            n == "door" || n == "window"
        })
    }

    /// Select a furniture instance, replacing any previous furniture selection — and clearing the
    /// CSG feature selection (the two are mutually exclusive, so one kind of thing is edited).
    pub fn select_furniture(&mut self, i: usize) {
        if i < self.furniture.len() {
            self.sel_furniture.clear();
            self.sel_furniture.push(i);
            self.selection.clear();
            self.sel_key.clear();
        }
    }

    /// ADD a furniture instance to the selection, or drop it if it was already in — Shift-click.
    ///
    /// Toggling rather than only adding is what makes a shift-click reversible: overshoot by one
    /// piece and you take it back the same way you added it, instead of starting the whole
    /// selection again.
    pub fn toggle_furniture(&mut self, i: usize) {
        if i >= self.furniture.len() {
            return;
        }
        match self.sel_furniture.iter().position(|&s| s == i) {
            Some(at) => {
                self.sel_furniture.remove(at);
            }
            None => self.sel_furniture.push(i),
        }
        self.selection.clear();
        self.sel_key.clear();
    }

    /// The PRIMARY selected furniture — the one whose properties the panel edits.
    pub fn sel_furn_primary(&self) -> Option<usize> {
        self.sel_furniture.first().copied()
    }

    /// The selected furniture only when there is EXACTLY one.
    ///
    /// For editors that write a single set of numbers back (dimensions, a parameter dialog): with
    /// several pieces selected there is no one set to show, and writing the primary's numbers to
    /// all of them would silently resize things the user never looked at.
    pub fn sel_furn_one(&self) -> Option<usize> {
        match self.sel_furniture.as_slice() {
            [i] => Some(*i),
            _ => None,
        }
    }

    /// True when anything is selected — a feature OR a furniture instance. The gizmo and
    /// properties panel key off this.
    pub fn has_any_selection(&self) -> bool {
        !self.selection.is_empty() || !self.sel_furniture.is_empty()
    }

    // ===================================================================
    // Selection: bounds, move, delete, per-object properties
    // ===================================================================

    /// GROUP the currently-selected features into one entity. All selected features (expanded to
    /// whole groups first, so re-grouping merges) get a fresh group id. Needs ≥ 2 features. Returns
    /// the count grouped, or 0 if there was nothing to group.
    pub fn group_selection(&mut self) -> usize {
        self.expand_selection_to_groups();
        let members: Vec<u32> = self
            .selection
            .iter()
            .copied()
            .filter(|id| self.model.features.iter().any(|f| f.id == *id))
            .collect();
        if members.len() < 2 {
            return 0;
        }
        let gid = self.next_group_id;
        self.next_group_id += 1;
        for id in &members {
            self.feature_group.insert(*id, gid);
        }
        members.len()
    }

    /// EXPLODE: dissolve the group(s) of the selected features so each piece is independent again.
    /// Returns the number of features released.
    pub fn ungroup_selection(&mut self) -> usize {
        let gids: std::collections::HashSet<u32> = self
            .selection
            .iter()
            .filter_map(|id| self.feature_group.get(id).copied())
            .collect();
        if gids.is_empty() {
            return 0;
        }
        let before = self.feature_group.len();
        self.feature_group.retain(|_, g| !gids.contains(g));
        before - self.feature_group.len()
    }

    /// Expand the current selection so that picking ONE member of a group selects the WHOLE group —
    /// the reason a group behaves as a single entity for move / delete / colour.
    pub fn expand_selection_to_groups(&mut self) {
        let gids: std::collections::HashSet<u32> = self
            .selection
            .iter()
            .filter_map(|id| self.feature_group.get(id).copied())
            .collect();
        if gids.is_empty() {
            return;
        }
        for (&fid, &gid) in &self.feature_group {
            if gids.contains(&gid) && !self.selection.contains(&fid) {
                self.selection.push(fid);
            }
        }
    }

    /// The group id of the current selection, if every selected feature shares ONE group (so the UI
    /// can show "Explode" instead of "Group").
    pub fn selection_group(&self) -> Option<u32> {
        let mut it = self.selection.iter().map(|id| self.feature_group.get(id).copied());
        let first = it.next()??;
        it.all(|g| g == Some(first)).then_some(first)
    }

    /// The single selected feature id, if exactly one solid is selected. Position and
    /// dimension editing act on ONE object; a multi-selection has no single set of
    /// dimensions.
    pub fn selected_single(&self) -> Option<u32> {
        match self.selection.as_slice() {
            [id] if self.model.features.iter().any(|f| f.id == *id) => Some(*id),
            _ => None,
        }
    }

    /// World AABB of the current selection — a furniture instance if one is selected,
    /// otherwise every selected feature. Drives the gizmo size and centre.
    pub fn selection_aabb(&self) -> Option<(Vec3, Vec3)> {
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        let mut any = false;
        for &i in &self.sel_furniture {
            if let Some((a, b)) = self.furniture_aabb(i) {
                mn = mn.min(a);
                mx = mx.max(b);
                any = true;
            }
        }
        for &id in &self.selection {
            if let Some(f) = self.model.features.iter().find(|f| f.id == id) {
                let (a, b) = f.world_aabb();
                mn = mn.min(a);
                mx = mx.max(b);
                any = true;
            }
        }
        any.then_some((mn, mx))
    }

    /// Geometric centre of the selection.
    pub fn selection_center(&self) -> Option<Vec3> {
        self.selection_aabb().map(|(mn, mx)| (mn + mx) * 0.5)
    }

    /// World AABB of the whole model (every feature). `None` when there are no features.
    /// Used by the room tool to size a void against the ACTUAL building, not a UI default.
    pub fn features_aabb(&self) -> Option<(Vec3, Vec3)> {
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for f in &self.model.features {
            let (a, b) = f.world_aabb();
            mn = mn.min(a);
            mx = mx.max(b);
        }
        (mn.x.is_finite()).then_some((mn, mx))
    }

    /// Select everything whose projected centre falls inside a screen rectangle — rubber-band
    /// box-select over BOTH the CSG features and the placed furniture. `additive` keeps the
    /// existing selection (Shift-drag).
    ///
    /// Centre-in-box is the intuitive rule for a box-select: an object counts as picked
    /// when its middle is inside the band, so a small overlap at the edge doesn't grab it.
    ///
    /// Features and furniture are normally mutually exclusive — but not here. A band dragged
    /// across a room is a statement about a REGION, and a user who draws one round a corner of the
    /// villa means the walls and the chairs standing in it. Silently dropping one kind would be
    /// the surprise; the delete and move paths handle both, so nothing downstream is confused by
    /// it. Returns `(features, furniture)` counts.
    pub fn select_in_marquee(
        &mut self, band: egui::Rect, viewport: egui::Rect, mvp: &[f32; 16], additive: bool,
    ) -> (usize, usize) {
        let mut hits = Vec::new();
        for f in &self.model.features {
            let (mn, mx) = f.world_aabb();
            if let Some(s) = world_to_screen((mn + mx) * 0.5, viewport, mvp) {
                if band.contains(s) {
                    hits.push(f.id);
                }
            }
        }
        let mut furn_hits = Vec::new();
        for i in 0..self.furniture.len() {
            if let Some((mn, mx)) = self.furniture_aabb(i) {
                if let Some(s) = world_to_screen((mn + mx) * 0.5, viewport, mvp) {
                    if band.contains(s) {
                        furn_hits.push(i);
                    }
                }
            }
        }
        if !additive {
            self.selection.clear();
            self.sel_furniture.clear();
        }
        for id in hits {
            if !self.selection.contains(&id) {
                self.selection.push(id);
            }
        }
        for i in furn_hits {
            if !self.sel_furniture.contains(&i) {
                self.sel_furniture.push(i);
            }
        }
        self.sel_key.clear();
        (self.selection.len(), self.sel_furniture.len())
    }

    /// Delete every selected furniture instance, highest index first.
    ///
    /// Descending order is not a detail: `Vec::remove` shifts everything after it down, so
    /// removing 2 then 5 deletes the wrong piece. Returns how many went.
    pub fn erase_selected_furniture(&mut self) -> usize {
        let mut idx = self.sel_furniture.clone();
        idx.sort_unstable();
        idx.dedup();
        let n = idx.len();
        for i in idx.into_iter().rev() {
            if i < self.furniture.len() {
                self.furniture.remove(i);
            }
        }
        self.sel_furniture.clear();
        n
    }

    /// Move the current selection by a world delta — the selected furniture instance, or
    /// every selected feature. In place, so the selection (and the gizmo) survives.
    pub fn move_selection(&mut self, delta: Vec3) {
        if delta.length_squared() < 1e-12 {
            return;
        }
        if !self.sel_furniture.is_empty() {
            for &i in &self.sel_furniture {
                if let Some(inst) = self.furniture.get_mut(i) {
                    inst.pos[0] += delta.x;
                    inst.pos[1] += delta.y;
                    inst.pos[2] += delta.z;
                }
            }
            if self.selection.is_empty() {
                return; // furniture is not part of the CSG model — nothing to re-eval
            }
        }
        for &id in &self.selection.clone() {
            if let Some(f) = self.model.get_mut(id) {
                *f = f.translated(delta);
            }
        }
        self.dirty = true;
    }

    /// Uniformly scale the current selection about its own centre — furniture instance or
    /// a single feature. `factor` multiplies the current size (1.0 = no change).
    pub fn scale_selection(&mut self, factor: f32) {
        let k = factor.clamp(0.02, 50.0);
        if (k - 1.0).abs() < 1e-4 {
            return;
        }
        if !self.sel_furniture.is_empty() {
            for &i in &self.sel_furniture {
                if let Some(inst) = self.furniture.get_mut(i) {
                    inst.scale = (inst.scale * k).clamp(0.001, 1000.0);
                }
            }
            return;
        }
        // A single feature: scale its primitive about its own centre.
        if let Some(id) = self.selected_single() {
            if let Some(f) = self.model.features.iter().find(|f| f.id == id).cloned() {
                let (mn, mx) = f.world_aabb();
                let pivot = (mn + mx) * 0.5;
                if let Some(fm) = self.model.get_mut(id) {
                    *fm = f.scaled(pivot, k);
                }
                self.dirty = true;
            }
        }
    }

    /// Paint the SURFACE (coplanar face) under the cursor with `color`. Ray-tests the
    /// cached mesh, finds the front-most triangle, and colours every triangle sharing its
    /// surface key. Returns true if a surface was hit.
    pub fn paint_surface(
        &mut self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16], color: [f32; 3],
    ) -> bool {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let mut best: Option<(f32, SurfaceKey)> = None;
        for (i, tri) in self.cached.positions.chunks_exact(3).enumerate() {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                if best.map_or(true, |(bt, _)| t < bt) {
                    let fid = self.cached.face_ids.get(i).copied().unwrap_or(0);
                    best = Some((t, surface_key(fid, tri[0], tri[1], tri[2])));
                }
            }
        }
        if let Some((_, key)) = best {
            self.surface_color.insert(key, color);
            return true;
        }
        false
    }

    /// Apply `tex_idx` to the SINGLE surface under the cursor (per-face texturing). Mirrors
    /// [`Self::paint_surface`] but writes `surface_texture`; clears any per-surface colour on
    /// that face so the image shows. Returns true if a surface was hit.
    pub fn paint_surface_texture(
        &mut self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16], tex_idx: usize,
    ) -> bool {
        if tex_idx >= self.textures.len() {
            return false;
        }
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let mut best: Option<(f32, SurfaceKey)> = None;
        for (i, tri) in self.cached.positions.chunks_exact(3).enumerate() {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                if best.map_or(true, |(bt, _)| t < bt) {
                    let fid = self.cached.face_ids.get(i).copied().unwrap_or(0);
                    best = Some((t, surface_key(fid, tri[0], tri[1], tri[2])));
                }
            }
        }
        if let Some((_, key)) = best {
            self.surface_color.remove(&key);
            self.surface_texture.insert(key, tex_idx);
            self.dirty = true;
            return true;
        }
        false
    }

    /// Resolve the cursor to the FACE-GROUP ids of FURNITURE instance `i` under it: the single flat
    /// face clicked, or (if `whole_piece`) every face-group of the connected body it belongs to.
    /// Ray-tests the asset's local triangles (the world ray is transformed into the instance's local
    /// space). Returns `None` if the object isn't under the cursor, or the asset is heavy
    /// (LOD)/translucent (per-surface unsupported there). See [`FurnGroups`].
    pub fn furniture_face_at(
        &self, i: usize, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16], whole_piece: bool,
    ) -> Option<Vec<u32>> {
        let asset_idx = self.furniture.get(i)?.asset;
        let model = self.furniture_model_matrix(i)?;
        let asset = self.furniture_lib.get(asset_idx)?;
        // NB: translucent assets are pickable — a mesh with glass (e.g. the villa, whose panes
        // carry per-vertex alpha since the FBX opacity import) must still take face/piece clicks;
        // the per-surface render path handles translucency. Only LOD-heavy meshes bail.
        if asset.needs_lod() {
            return None;
        }
        // World ray → the instance's LOCAL space (positions are stored local).
        let (ow, dw) = Self::ray(cursor, rect, mvp);
        let inv = glam::Mat4::from_cols_array(&model).inverse();
        let ol = inv.transform_point3(ow);
        let dl = inv.transform_vector3(dw).normalize_or_zero();
        let mut best: Option<(f32, usize)> = None;
        for (ti, tri) in asset.positions.chunks_exact(3).enumerate() {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            if let Some(t) = cad_solid::ray_triangle(ol, dl, a, b, c) {
                if best.map_or(true, |(bt, _)| t < bt) {
                    best = Some((t, ti));
                }
            }
        }
        let (_, ht) = best?;
        let groups = asset.group_geom();
        Some(if whole_piece {
            let bid = groups.body.get(ht).copied().unwrap_or(0);
            let mut set = std::collections::BTreeSet::new();
            for t in 0..groups.face.len() {
                if groups.body.get(t) == Some(&bid) {
                    set.insert(groups.face[t]);
                }
            }
            set.into_iter().collect()
        } else {
            vec![groups.face.get(ht).copied().unwrap_or(0)]
        })
    }

    /// Find (or create) a 1×1 solid-colour texture for `c`. A per-face COLOUR is stored as a tiny
    /// solid texture so it flows through the SAME per-surface machinery as an image — mirroring
    /// Blender, where a face's material can be a flat colour or a texture. Reuses an existing swatch
    /// so repeated colours don't bloat the library.
    pub fn ensure_solid_color_texture(&mut self, c: [f32; 3]) -> usize {
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let rgba = [to8(c[0]), to8(c[1]), to8(c[2]), 255];
        for (i, t) in self.textures.iter().enumerate() {
            if t.w == 1 && t.h == 1 && t.rgba == rgba {
                return i;
            }
        }
        self.add_texture(
            format!("colour #{:02x}{:02x}{:02x}", rgba[0], rgba[1], rgba[2]),
            1, 1, rgba.to_vec(),
        )
    }

    /// True when this furniture asset carries per-primitive part ids (a generated object). Used by
    /// diagnostics + to decide whether "piece"/"face" split cleanly.
    pub fn furniture_has_parts(&self, i: usize) -> bool {
        self.furniture.get(i)
            .and_then(|inst| self.furniture_lib.get(inst.asset))
            .map_or(false, |a| a.part_ids.len() == a.positions.len() / 3)
    }

    /// How many triangles of instance `i` currently carry a PER-FACE texture (for diagnostics —
    /// confirms an apply hit only the intended face, not the whole object).
    pub fn furniture_textured_tri_count(&self, i: usize) -> usize {
        let Some(inst) = self.furniture.get(i) else { return 0 };
        if inst.surface_texture.is_empty() {
            return 0;
        }
        let Some(asset) = self.furniture_lib.get(inst.asset) else { return 0 };
        let g = asset.group_geom();
        g.face.iter().filter(|fg| inst.surface_texture.contains_key(fg)).count()
    }

    /// Record `tex_idx` against `face_groups` of furniture instance `i` (per-surface texturing).
    /// Per-surface texturing is AUTHORITATIVE: applying to a face drops any WHOLE-OBJECT texture
    /// (and its baked-in avg-colour tint), so only the painted faces show a texture and the rest
    /// return to the asset's own colour — otherwise the whole-object texture would keep masking
    /// every un-painted face and it looks like "the texture went on the whole object".
    pub fn apply_face_texture(&mut self, i: usize, face_groups: &[u32], tex_idx: usize) {
        if tex_idx >= self.textures.len() {
            return;
        }
        let asset_col = self
            .furniture
            .get(i)
            .and_then(|inst| self.furniture_lib.get(inst.asset))
            .map(|a| a.color);
        if let Some(inst) = self.furniture.get_mut(i) {
            if inst.texture.take().is_some() {
                // was whole-object textured → reset the tinted colour to the asset default
                if let Some(c) = asset_col {
                    inst.color = c;
                }
            }
            for &fg in face_groups {
                inst.surface_texture.insert(fg, tex_idx);
            }
        }
    }

    /// The per-face texture bound to `face_groups` of instance `i`, if any — the material a
    /// face/piece "wears" (the first painted group's texture; `None` if none are painted).
    pub fn face_material(&self, i: usize, face_groups: &[u32]) -> Option<usize> {
        let inst = self.furniture.get(i)?;
        face_groups.iter().find_map(|fg| inst.surface_texture.get(fg).copied())
    }

    /// Is texture `ti` referenced ANYWHERE other than exactly `face_groups` of instance `i`? Drives
    /// copy-on-write: tuning one piece's opacity/reflection/tiling must not touch any OTHER surface
    /// that happens to share the same texture (including the same object's whole-object texture).
    pub fn texture_used_outside(&self, ti: usize, i: usize, face_groups: &[u32]) -> bool {
        let want: std::collections::HashSet<u32> = face_groups.iter().copied().collect();
        for (j, inst) in self.furniture.iter().enumerate() {
            if inst.texture == Some(ti) {
                return true; // a whole-object texture is "outside" the piece
            }
            for (&fg, &t) in &inst.surface_texture {
                if t == ti && !(j == i && want.contains(&fg)) {
                    return true;
                }
            }
        }
        self.feature_texture.values().any(|&t| t == ti)
            || self.surface_texture.values().any(|&t| t == ti)
    }

    /// Deep-copy texture `ti` into a new asset (own pixels + fresh PNG cache), keeping its
    /// tiling/opacity/reflection, and return the new index. Used to give a piece its OWN material.
    pub fn clone_texture(&mut self, ti: usize) -> usize {
        let Some(src) = self.textures.get(ti) else { return ti };
        let mut t = TextureAsset::new(format!("{} (copy)", src.name), src.w, src.h, src.rgba.clone());
        t.scale = src.scale;
        t.offset = src.offset;
        t.rot_deg = src.rot_deg;
        t.opacity = src.opacity;
        t.reflect = src.reflect;
        self.textures.push(t);
        self.textures.len() - 1
    }

    /// Give the selected face/piece a material that is EXCLUSIVE to it, so tuning its
    /// opacity/reflection/tiling changes ONLY that piece and nothing else. Seeds from the piece's
    /// own per-face texture, else the whole-object texture, else a solid swatch of the piece's
    /// colour; clones it copy-on-write when anything else shares it; binds it to `face_groups`
    /// (WITHOUT dropping the whole-object texture — the rest of the object keeps its look). Returns
    /// the exclusive texture index to write. Idempotent once the piece owns its material.
    pub fn private_piece_material(&mut self, i: usize, face_groups: &[u32]) -> usize {
        let seed = if let Some(t) = self.face_material(i, face_groups) {
            t
        } else if let Some(t) = self.furniture.get(i).and_then(|f| f.texture) {
            t
        } else {
            let col = self.furniture.get(i).map(|f| f.color).unwrap_or([0.8, 0.8, 0.82]);
            self.ensure_solid_color_texture(col)
        };
        let ti = if self.texture_used_outside(seed, i, face_groups) {
            self.clone_texture(seed)
        } else {
            seed
        };
        if let Some(inst) = self.furniture.get_mut(i) {
            for &fg in face_groups {
                inst.surface_texture.insert(fg, ti);
            }
        }
        ti
    }

    /// Is texture `ti` referenced by any surface OTHER than the given feature `ids`? The feature
    /// analog of [`Self::texture_used_outside`] — drives per-solid copy-on-write so tuning one
    /// CSG solid's opacity/reflection doesn't drag every solid that shares the texture.
    pub fn feature_texture_used_outside(&self, ti: usize, ids: &[u32]) -> bool {
        let want: std::collections::HashSet<u32> = ids.iter().copied().collect();
        for inst in &self.furniture {
            if inst.texture == Some(ti) || inst.surface_texture.values().any(|&t| t == ti) {
                return true;
            }
        }
        if self.surface_texture.values().any(|&t| t == ti) {
            return true;
        }
        self.feature_texture.iter().any(|(&id, &t)| t == ti && !want.contains(&id))
    }

    /// Give the selected CSG feature(s) a material EXCLUSIVE to them, so tuning its opacity /
    /// reflection / tiling changes ONLY those solids. Seeds from the first selected feature's
    /// texture, else a solid swatch of its colour; clones copy-on-write when anything else shares
    /// it; binds it to every selected feature. Recomputes ONCE if a previously-untextured feature
    /// was newly textured (its triangles must move to the textured pass). Returns the index to write.
    pub fn private_feature_material(&mut self, ids: &[u32]) -> usize {
        let seed = if let Some(t) = ids.iter().find_map(|id| self.feature_texture.get(id).copied()) {
            t
        } else {
            let col = ids
                .iter()
                .find_map(|id| self.feature_color.get(id).copied())
                .unwrap_or([0.8, 0.8, 0.82]);
            self.ensure_solid_color_texture(col)
        };
        let ti = if self.feature_texture_used_outside(seed, ids) {
            self.clone_texture(seed)
        } else {
            seed
        };
        let mut minted = false;
        for &id in ids {
            if self.feature_texture.insert(id, ti).is_none() {
                minted = true; // a newly-textured feature — its tris leave the flat batch
            }
        }
        if minted {
            self.recompute();
        }
        ti
    }

    /// Load an HDR environment (or drop the current one), doing all the expensive derivation once.
    ///
    /// The SH projection and the GGX prefilter both run here, at load, and never again — a frame
    /// must never wait on them. Returns a one-line summary for the status bar, including a warning
    /// when the file has no dynamic range in it, because an IBL lit by an 8-bit JPEG looks broken in
    /// a way that is very hard to attribute to the file.
    pub fn set_env_map(&mut self, map: Option<crate::env_map::EnvMap>) -> String {
        match map {
            None => {
                self.env_map = None;
                self.env_chain.clear();
                self.env_sh = [[0.0; 3]; 9];
                self.env_version = self.env_version.wrapping_add(1);
                self.dirty = true;
                "environment cleared — back to the analytic sky".into()
            }
            Some(m) => {
                let hdr = m.is_hdr();
                let peak = m.peak();
                let (w, h) = (m.w, m.h);
                let name = m.name.clone();
                self.env_sh = m.sh9();
                self.env_chain = m.prefilter();
                self.env_map = Some(std::sync::Arc::new(m));
                self.env_version = self.env_version.wrapping_add(1);
                self.dirty = true;
                if hdr {
                    format!("environment '{name}' loaded — {w}×{h}, peak {peak:.0}")
                } else {
                    format!(
                        "environment '{name}' loaded — {w}×{h}, but it peaks at {peak:.2}: \
                         a low-dynamic-range image has no sun in it and will light the scene flatly"
                    )
                }
            }
        }
    }

    /// The scene's environment for one frame: the sun's own resolution, with a loaded HDRI
    /// overriding the analytic sky's ambient and backdrop.
    ///
    /// The HDRI wins on the DIFFUSE term by replacing `sh` — the same nine coefficients the sky
    /// would have supplied, so nothing downstream needs to know which produced them. The sun stays
    /// whatever the Sun dialog says: an HDRI's own sun cannot cast a shadow (it is pixels, not a
    /// direction), so a daylight study keeps its directional light and gets the image's ambient.
    pub fn scene_env(&self) -> (bool, Vec3, [f32; 3], crate::env::EnvRender) {
        let (en, dir, sun_col, mut env) = self.sun.resolve_env();
        if self.env_map.is_some() {
            env.sh = self.env_sh;
            env.hdri = Some(crate::env::HdriUse {
                strength: self.env_strength.max(0.0),
                rot: self.env_rot_deg.to_radians(),
            });
            env.backdrop = crate::env::Backdrop::Sky;
        }
        (en, dir, sun_col, env)
    }

    /// Rebuild instance `fi`'s geometry from its UNCUT original plus its whole cut list.
    ///
    /// This is what makes the cuts editable. Nothing is ever baked: the source of truth is the
    /// list, and every change — adding, disabling, deleting, editing a depth — replays the lot
    /// against untouched geometry. Delete every cut and the piece is bit-for-bit what it was.
    ///
    /// The cut result lands in a DERIVED library asset the instance points at, rather than in some
    /// parallel per-instance mesh. That is deliberate: every consumer in the app — the render
    /// buffers, ray-picking, face grouping, LOD, per-surface texturing — already reads a
    /// `FurnitureAsset`, and they all keep working unchanged. Only this function knows a cut
    /// happened.
    ///
    /// Cutting one piece never touches its neighbours: a derived asset belongs to one instance, so
    /// three doors placed from the same library entry stay independent.
    pub fn rebuild_cut_asset(&mut self, fi: usize) -> Result<(), cad_solid::meshcut::CutError> {
        let Some(inst) = self.furniture.get(fi) else { return Ok(()) };
        let src = inst.source_asset();
        let Some(base) = self.furniture_lib.get(src) else { return Ok(()) };
        let cuts = inst.cuts.clone();

        // Nothing enabled → go back to the original outright, and drop the derived copy.
        let out = cad_solid::meshcut::apply(&base.positions, &base.normals, &base.part_ids, &cuts)?;
        let Some(mesh) = out else {
            if let Some(inst) = self.furniture.get_mut(fi) {
                inst.asset = src;
                inst.base_asset = None;
            }
            self.dirty = true;
            return Ok(());
        };

        // Per-part MATERIALS have to be carried across, and they cannot be carried by face-group
        // id: those are renumbered whenever the triangles change, so the ids the instance has bound
        // would land on different surfaces after a cut. Part ids are stable through the boolean
        // (that is why they go through it), so the binding is remapped part-wise: read what each
        // part is wearing now, then re-bind by part on the new geometry.
        let part_tex: std::collections::HashMap<u32, usize> = {
            let g = base.group_geom();
            let mut m = std::collections::HashMap::new();
            for t in 0..base.positions.len() / 3 {
                let part = base.part_ids.get(t).copied().unwrap_or(0);
                if let Some(&tex) = inst.surface_texture.get(&g.face[t]) {
                    m.entry(part).or_insert(tex);
                }
            }
            m
        };

        let name = format!("{} (cut)", base.name);
        let color = base.color;
        let import_scale = base.import_scale;
        let part_ids = mesh.face_ids;

        // Built DIRECTLY, never through `add_furniture_asset`. That routine RECENTRES an asset on
        // its own bounds and rescales a wildly-sized import toward prop size — right for an import,
        // wrong here. A cut changes the bounds, so recentring would shift the piece the first time
        // it was cut; and since a later rebuild goes through the in-place path, it would shift back
        // on the second edit. A derived asset is already in its original's units and its original's
        // frame, and has to stay in both.
        let mut asset = FurnitureAsset::new(name, mesh.positions, mesh.normals, color);
        asset.import_scale = import_scale;
        asset.alpha_resolved = true;
        if part_ids.len() == asset.positions.len() / 3 {
            asset.part_ids = part_ids;
        }

        // Reuse the derived slot when there is one, so repeated edits do not grow the library.
        let existing = self.furniture[fi].base_asset.map(|_| self.furniture[fi].asset);
        let derived = match existing {
            Some(d) if d != src && d < self.furniture_lib.len() => {
                self.furniture_lib[d] = asset;
                d
            }
            _ => {
                self.furniture_lib.push(asset);
                self.furniture_lib.len() - 1
            }
        };

        // Re-bind materials by part on the new face groups.
        let rebound = self.furniture_lib.get(derived).map(|a| {
            let g = a.group_geom();
            let mut m: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            for t in 0..a.positions.len() / 3 {
                let part = a.part_ids.get(t).copied().unwrap_or(0);
                if let Some(&tex) = part_tex.get(&part) {
                    m.insert(g.face[t], tex);
                }
            }
            m
        });
        if let Some(inst) = self.furniture.get_mut(fi) {
            inst.base_asset = Some(src);
            inst.asset = derived;
            if let Some(m) = rebound {
                inst.surface_texture = m;
            }
        }
        self.dirty = true;
        self.face_outline.borrow_mut().take();
        Ok(())
    }

    /// Overwrite an existing library asset's geometry in place, keeping its index valid.
    fn replace_furniture_asset(
        &mut self, idx: usize, name: String, mesh: crate::mesh_io::ObjMesh, import_scale: f32,
    ) {
        let Some(a) = self.furniture_lib.get_mut(idx) else { return };
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &mesh.positions {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        a.name = name;
        a.positions = mesh.positions;
        a.normals = mesh.normals;
        a.uvs.clear(); // the boolean does not carry UVs; per-part materials do the work instead
        a.alpha.clear();
        a.part_ids.clear();
        a.local_min = lo;
        a.local_max = hi;
        a.import_scale = import_scale;
        *a.groups.borrow_mut() = None;
        *a.lod.borrow_mut() = None;
    }

    /// Screen-space segments outlining the currently-targeted furniture face/piece
    /// (`furn_face_sel`) — so the user can SEE which surface a texture will land on. Empty when
    /// nothing is targeted.
    ///
    /// This used to walk every triangle in the asset each frame and emit all THREE edges of every
    /// selected one. On the villa (2.01 M triangles, a 15,296-triangle face) that was a 2 M-element
    /// scan producing **45,888 line segments per frame**, each of which egui tessellates into a
    /// quad — measured at 250–436 ms a frame, for as long as anything stayed selected. Selecting
    /// something made the app unusable, and deselecting fixed it.
    ///
    /// Both halves of that were wrong, and both fixes are here:
    ///
    /// - **Outline, not wireframe.** A face group is a surface; its three-edges-per-triangle form
    ///   is its whole triangulation, which is a dense mesh of lines nobody wants to look at. What
    ///   reads as "this face is selected" is the BOUNDARY — the edges used by exactly one selected
    ///   triangle. For that same villa face it is a few hundred segments instead of 45,888.
    /// - **Cached in LOCAL space.** The boundary depends on the selection, not on the camera or
    ///   the pose, so it is built once and then only transformed and projected. Orbiting the
    ///   camera and dragging the object both stay free.
    pub fn furniture_face_highlight_segments(
        &self, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Vec<[egui::Pos2; 2]> {
        let mut out = Vec::new();
        let Some((fi, groups)) = self.furn_face_sel.as_ref() else { return out };
        let Some(inst) = self.furniture.get(*fi) else { return out };
        let Some(asset) = self.furniture_lib.get(inst.asset) else { return out };
        if asset.needs_lod() {
            return out;
        }

        // Rebuild only when the SELECTION changed — not when the camera moved.
        {
            let tris = asset.positions.len() / 3;
            let stale = self.face_outline.borrow().as_ref().is_none_or(|c| {
                c.inst != *fi || c.asset != inst.asset || c.tris != tris || c.groups != *groups
            });
            if stale {
                *self.face_outline.borrow_mut() =
                    Some(Self::build_face_outline(asset, *fi, inst.asset, groups));
            }
        }
        let cache = self.face_outline.borrow();
        let Some(c) = cache.as_ref() else { return out };

        let model = glam::Mat4::from_cols_array(&self.furniture_model_matrix(*fi).unwrap_or([
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]));
        out.reserve(c.edges.len());
        for e in &c.edges {
            let a = world_to_screen(model.transform_point3(e[0]), rect, mvp);
            let b = world_to_screen(model.transform_point3(e[1]), rect, mvp);
            if let (Some(a), Some(b)) = (a, b) {
                out.push([a, b]);
            }
        }
        out
    }

    /// True when the last outline was cut short at [`MAX_OUTLINE_EDGES`] — the caller says so
    /// rather than quietly drawing a partial highlight, because a highlight that stops halfway
    /// round a face reads as a hole in the face.
    pub fn face_highlight_truncated(&self) -> bool {
        self.face_outline.borrow().as_ref().is_some_and(|c| c.truncated)
    }

    /// The boundary of the selected face-groups, in the asset's local space.
    ///
    /// Edges are welded by QUANTIZED position rather than by vertex index: this is triangle soup,
    /// so the two triangles sharing an edge carry two separate copies of its endpoints, and an
    /// index-based match would find no shared edges at all and hand back the wireframe it was
    /// meant to replace.
    fn build_face_outline(
        asset: &FurnitureAsset, inst: usize, asset_idx: usize, groups: &[u32],
    ) -> FaceOutline {
        use std::collections::hash_map::Entry;
        let fg = asset.group_geom();
        let want: std::collections::HashSet<u32> = groups.iter().copied().collect();
        // 0.1 mm — far below anything a user can see, far above f32 noise on a metre-scale model.
        let key = |p: [f32; 3]| {
            [
                (p[0] * 10_000.0).round() as i64,
                (p[1] * 10_000.0).round() as i64,
                (p[2] * 10_000.0).round() as i64,
            ]
        };
        let mut edges: std::collections::HashMap<([i64; 3], [i64; 3]), ([Vec3; 2], u32)> =
            std::collections::HashMap::new();
        for t in 0..fg.face.len() {
            if !want.contains(&fg.face[t]) {
                continue;
            }
            let base = t * 3;
            for e in 0..3 {
                let (p, q) = (asset.positions[base + e], asset.positions[base + (e + 1) % 3]);
                let (kp, kq) = (key(p), key(q));
                // Order-independent, so the same edge from the neighbouring triangle (which winds
                // it the other way) lands on the same key.
                let k = if kp <= kq { (kp, kq) } else { (kq, kp) };
                match edges.entry(k) {
                    Entry::Occupied(mut o) => o.get_mut().1 += 1,
                    Entry::Vacant(v) => {
                        v.insert(([Vec3::from(p), Vec3::from(q)], 1));
                    }
                }
            }
        }
        let mut out = Vec::new();
        let mut truncated = false;
        for (seg, n) in edges.into_values() {
            if n != 1 {
                continue; // interior: shared by two selected triangles
            }
            if out.len() >= MAX_OUTLINE_EDGES {
                truncated = true;
                break;
            }
            out.push(seg);
        }
        FaceOutline {
            inst,
            asset: asset_idx,
            tris: asset.positions.len() / 3,
            groups: groups.to_vec(),
            edges: out,
            truncated,
        }
    }

    /// Set a feature's world origin directly — the Position fields in the properties
    /// panel. `axis` 0/1/2 = x/y/z.
    pub fn set_feature_origin_axis(&mut self, id: u32, axis: usize, value: f32) {
        if let Some(f) = self.model.get_mut(id) {
            let mut o = f.world_origin();
            o[axis] = value;
            *f = f.with_world_origin(o);
            self.dirty = true;
        }
    }

    /// Replace a feature's primitive — the dimension fields in the properties panel.
    pub fn set_feature_primitive(&mut self, id: u32, p: Primitive) {
        if let Some(f) = self.model.get_mut(id) {
            f.primitive = p;
            self.dirty = true;
        }
    }

    /// Set one of a feature's rotation angles (degrees) about its plane-LOCAL axes:
    /// axis 0 = pitch (about plane u), 1 = roll (about plane v), 2 = spin (about the
    /// normal). Drives both the numeric fields and the rotation-ring gizmo.
    pub fn set_feature_rotation(&mut self, id: u32, axis: usize, deg: f32) {
        if let Some(f) = self.model.get_mut(id) {
            match axis {
                0 => f.placement.pitch_deg = deg,
                1 => f.placement.roll_deg = deg,
                _ => f.placement.spin_deg = deg,
            }
            self.dirty = true;
        }
    }

    /// A feature's current rotation `[pitch, roll, spin]` in degrees, for the panel/gizmo.
    pub fn feature_rotation(&self, id: u32) -> Option<[f32; 3]> {
        let f = self.model.features.iter().find(|f| f.id == id)?;
        Some([f.placement.pitch_deg, f.placement.roll_deg, f.placement.spin_deg])
    }

    /// The primitive of the single selected feature, for the properties panel.
    pub fn selected_primitive(&self) -> Option<(u32, Primitive, Vec3)> {
        let id = self.selected_single()?;
        let f = self.model.features.iter().find(|f| f.id == id)?;
        Some((id, f.primitive, f.world_origin()))
    }

    // ===================================================================
    // Move gizmo — a 3-axis translate handle at the selection centre
    // ===================================================================

    /// Screen geometry of the move gizmo, or `None` when nothing is selected / the centre
    /// is off-screen. The arm length scales with the object (so a big object gets a big
    /// gizmo) but has a screen-space floor (so a tiny one stays grabbable), and reaches
    /// PAST the object so the arms are not buried inside it.
    pub fn gizmo_view(&self, rect: egui::Rect, mvp: &[f32; 16]) -> Option<GizmoView> {
        let (mn, mx) = self.selection_aabb()?;
        let c = (mn + mx) * 0.5;
        let center_s = world_to_screen(c, rect, mvp)?;
        let half = ((mx - mn) * 0.5).max_element().max(1e-3);
        let ppw = self.px_per_world(c, rect, mvp);
        // Arms reach 1.4× the object's half-size, but never less than ~65 px on screen.
        let min_world = if ppw > 1e-6 { GIZMO_MIN_PX / ppw } else { half };
        let len = (half * 1.4).max(min_world);
        let mk = |h: GizmoHandle, d: Vec3| {
            let tip_w = c + d * len;
            world_to_screen(tip_w, rect, mvp).map(|tip_s| GizmoArm { handle: h, dir: d, tip_s })
        };
        Some(GizmoView {
            center_w: c,
            center_s,
            len_w: len,
            arms: [
                mk(GizmoHandle::X, Vec3::X),
                mk(GizmoHandle::Y, Vec3::Y),
                mk(GizmoHandle::Z, Vec3::Z),
            ],
        })
    }

    /// Approximate pixels-per-world at a point — the max screen speed over the three axes,
    /// so it stays non-zero even when one axis points at the camera.
    fn px_per_world(&self, c: Vec3, rect: egui::Rect, mvp: &[f32; 16]) -> f32 {
        let Some(cs) = world_to_screen(c, rect, mvp) else { return 0.0 };
        let probe = 0.5;
        let mut best = 0.0f32;
        for d in [Vec3::X, Vec3::Y, Vec3::Z] {
            if let Some(p) = world_to_screen(c + d * probe, rect, mvp) {
                best = best.max(cs.distance(p));
            }
        }
        best / probe
    }

    /// Which gizmo handle is under the cursor. The centre cube wins over the axes (it sits
    /// where all three arms meet), so a click there is always the free-move, never an axis.
    pub fn pick_gizmo(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<GizmoHandle> {
        let v = self.gizmo_view(rect, mvp)?;
        if v.center_s.distance(cursor) <= GIZMO_CUBE_PICK {
            return Some(GizmoHandle::Free);
        }
        let mut best: Option<(f32, GizmoHandle)> = None;
        for arm in v.arms.iter().flatten() {
            let d = dist_point_segment(cursor, v.center_s, arm.tip_s);
            if d <= GIZMO_AXIS_PICK && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, arm.handle));
            }
        }
        best.map(|(_, h)| h)
    }

    // ===================================================================
    // Rotation-ring gizmo
    // ===================================================================

    /// The rotation target: `(center_world, [axis0, axis1, axis2], is_furniture)`. Furniture
    /// rotates about WORLD axes; a single feature about its plane's LOCAL axes (u, v, n) — so
    /// ring 0→pitch(u), 1→roll(v), 2→spin(n), matching `set_feature_rotation`. `None` unless
    /// exactly one furniture OR one feature is selected.
    fn rot_target(&self) -> Option<(Vec3, [Vec3; 3], bool)> {
        let (mn, mx) = self.selection_aabb()?;
        let center = (mn + mx) * 0.5;
        if !self.sel_furniture.is_empty() {
            return Some((center, [Vec3::X, Vec3::Y, Vec3::Z], true));
        }
        let id = self.selected_single()?;
        let f = self.model.features.iter().find(|f| f.id == id)?;
        let (u, v) = f.plane.axes();
        let (u, v) = (u.normalize_or_zero(), v.normalize_or_zero());
        let n = u.cross(v).normalize_or_zero();
        Some((center, [u, v, n], false))
    }

    /// Rotation gizmo geometry for this frame — three rings sized like the move arms.
    pub fn rotation_rings(&self, rect: egui::Rect, mvp: &[f32; 16]) -> Option<RingView> {
        let (center, axes, is_furniture) = self.rot_target()?;
        let (mn, mx) = self.selection_aabb()?;
        let half = ((mx - mn) * 0.5).max_element().max(1e-3);
        let ppw = self.px_per_world(center, rect, mvp);
        let min_world = if ppw > 1e-6 { GIZMO_MIN_PX / ppw } else { half };
        let radius = (half * 1.3).max(min_world);
        let center_s = world_to_screen(center, rect, mvp)?;
        let handles = [GizmoHandle::RotX, GizmoHandle::RotY, GizmoHandle::RotZ];
        const SEG: usize = 48;
        let mut rings = Vec::with_capacity(3);
        for i in 0..3 {
            let (a, b) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
            let mut pts = Vec::with_capacity(SEG + 1);
            for k in 0..=SEG {
                let t = (k as f32) / (SEG as f32) * std::f32::consts::TAU;
                let w = center + (a * t.cos() + b * t.sin()) * radius;
                if let Some(s) = world_to_screen(w, rect, mvp) {
                    pts.push(s);
                }
            }
            rings.push(Ring { handle: handles[i], axis: axes[i], pts });
        }
        Some(RingView { center, center_s, radius, rings, is_furniture })
    }

    /// Which rotation ring is under the cursor (nearest ring polyline within tolerance).
    pub fn pick_ring(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<GizmoHandle> {
        let rv = self.rotation_rings(rect, mvp)?;
        let mut best: Option<(f32, GizmoHandle)> = None;
        for ring in &rv.rings {
            let mut d = f32::INFINITY;
            for seg in ring.pts.windows(2) {
                d = d.min(dist_point_segment(cursor, seg[0], seg[1]));
            }
            if d <= GIZMO_AXIS_PICK && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, ring.handle));
            }
        }
        best.map(|(_, h)| h)
    }

    /// The unit vector from the ring centre to where the cursor ray meets the ring's plane
    /// (axis-component removed). `None` if the ray is parallel to the plane or hits the centre.
    fn ray_to_ring_vec(&self, cursor: egui::Pos2, center: Vec3, axis: Vec3, rect: egui::Rect, mvp: &[f32; 16]) -> Option<Vec3> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let denom = dir.dot(axis);
        if denom.abs() < 1e-5 { return None; }
        let t = (center - orig).dot(axis) / denom;
        if t <= 0.0 { return None; }
        let p = orig + dir * t;
        let r = (p - center) - axis * (p - center).dot(axis);
        let len = r.length();
        if len < 1e-5 { return None; }
        Some(r / len)
    }

    /// Begin a rotation-ring drag on `handle`. Captures the grab reference + start rotation so
    /// the gesture is one undo step. Returns false if the grab can't be established.
    pub fn rot_begin(&mut self, handle: GizmoHandle, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> bool {
        let Some((center, axes, is_furniture)) = self.rot_target() else { return false };
        let Some(ai) = handle.ring_axis() else { return false };
        let axis = axes[ai];
        let Some(r0) = self.ray_to_ring_vec(cursor, center, axis, rect, mvp) else { return false };
        let start_furn: Vec<(usize, [f32; 3], [f32; 3])> = if is_furniture {
            self.sel_furniture
                .iter()
                .filter_map(|&fi| self.furniture.get(fi).map(|f| (fi, f.pos, f.rot)))
                .collect()
        } else {
            Vec::new()
        };
        let start_rot = if is_furniture {
            start_furn.first().map(|&(_, _, r)| r).unwrap_or([0.0; 3])
        } else {
            self.selected_single().and_then(|id| self.feature_rotation(id)).unwrap_or([0.0; 3])
        };
        self.rot_drag = Some(RotDrag {
            handle, axis, center, r0, start_rot,
            feat_axis: ai, is_furniture, start_furn,
        });
        true
    }

    /// Apply the current cursor position to the live rotation drag. Furniture composes a
    /// world-axis quaternion (kept as Euler for the numeric fields); a feature adds the swept
    /// angle to the ring's plane-local placement angle.
    pub fn rot_update(&mut self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) {
        let Some(d) = self.rot_drag.clone() else { return };
        let Some(r1) = self.ray_to_ring_vec(cursor, d.center, d.axis, rect, mvp) else { return };
        // Signed angle r0→r1 about the axis (radians).
        let angle = d.r0.cross(r1).dot(d.axis).atan2(d.r0.dot(r1));
        if d.is_furniture {
            let spin = glam::Quat::from_axis_angle(d.axis, angle);
            for &(fi, start_pos, start_rot) in &d.start_furn {
                let Some(inst) = self.furniture.get_mut(fi) else { continue };
                // ORBIT about the selection centre as well as spinning in place. With one piece
                // selected the centre IS the piece and this term vanishes; with several, turning
                // the group has to move them round each other or it is not a group rotation at
                // all — the pieces would each pirouette on the spot and the arrangement break up.
                let p = Vec3::from(start_pos);
                let moved = d.center + spin * (p - d.center);
                inst.pos = [moved.x, moved.y, moved.z];
                let q_start = glam::Quat::from_euler(
                    glam::EulerRot::XYZ,
                    start_rot[0].to_radians(), start_rot[1].to_radians(), start_rot[2].to_radians(),
                );
                let (x, y, z) = (spin * q_start).to_euler(glam::EulerRot::XYZ);
                inst.rot = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
            }
        } else if let Some(id) = self.selected_single() {
            let deg = d.start_rot[d.feat_axis] + angle.to_degrees();
            self.set_feature_rotation(id, d.feat_axis, deg);
        }
    }

    /// End a rotation drag.
    pub fn rot_end(&mut self) {
        self.rot_drag = None;
    }

    // ===================================================================
    // Wall vertex handles — reshaping an alive wall in the 3D view
    // ===================================================================

    /// The wall the current 3D selection belongs to. Handles are shown for THIS wall
    /// only: drawing them for every wall at once would bury the model in dots.
    pub fn selected_wall(&self) -> Option<usize> {
        self.selection.iter().find_map(|&id| self.wall_index(id))
    }

    /// Re-select a wall by its segments.
    ///
    /// Needed after every SHAPE edit: `rederive_wall` drops and rebuilds the Boxes, so the
    /// old ids are gone and the selection would be empty — the handles would vanish
    /// mid-gesture. (Height edits are different: they mutate in place and ids survive.)
    pub fn select_wall(&mut self, wi: usize) {
        if let Some(w) = self.walls.get(wi) {
            self.selection = w.segments.clone();
            self.sel_key.clear();
        }
    }

    /// World position of footprint vertex `vi` — on the wall's OWN storey, so handles on
    /// an upper floor appear up there rather than on the ground.
    fn wall_vertex_world(&self, wi: usize, vi: usize) -> Option<Vec3> {
        let w = self.walls.get(wi)?;
        let p = w.footprint.get(vi)?;
        Some(Vec3::new(p.x, p.y, w.base_z))
    }

    /// Screen positions of wall `wi`'s footprint vertices, as `(vertex index, position)`.
    /// Vertices behind the camera are omitted, so nothing is drawn or picked where the
    /// user cannot see it.
    pub fn wall_vertex_handles(
        &self, wi: usize, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Vec<(usize, egui::Pos2)> {
        let Some(w) = self.walls.get(wi) else { return Vec::new() };
        (0..w.footprint.len())
            .filter_map(|vi| {
                let world = self.wall_vertex_world(wi, vi)?;
                Some((vi, world_to_screen(world, rect, mvp)?))
            })
            .collect()
    }

    /// Screen positions of each EDGE's midpoint, as `(segment index, position)` — the
    /// click target for inserting a vertex.
    pub fn wall_edge_handles(
        &self, wi: usize, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Vec<(usize, egui::Pos2)> {
        let Some(w) = self.walls.get(wi) else { return Vec::new() };
        (0..w.footprint.len().saturating_sub(1))
            .filter_map(|si| {
                let a = self.wall_vertex_world(wi, si)?;
                let b = self.wall_vertex_world(wi, si + 1)?;
                Some((si, world_to_screen((a + b) * 0.5, rect, mvp)?))
            })
            .collect()
    }

    /// Vertex handle under the cursor, if any. Nearest wins, so overlapping handles
    /// resolve predictably.
    pub fn pick_wall_vertex(
        &self, wi: usize, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<usize> {
        nearest_within(self.wall_vertex_handles(wi, rect, mvp), cursor, HANDLE_PICK_R)
    }

    /// Edge midpoint under the cursor. A TIGHTER aperture than a vertex, because a
    /// midpoint sits between two vertex handles — the vertex must win a close call, or
    /// dragging a corner would insert a point instead.
    pub fn pick_wall_edge(
        &self, wi: usize, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<usize> {
        if self.pick_wall_vertex(wi, cursor, rect, mvp).is_some() {
            return None;
        }
        nearest_within(self.wall_edge_handles(wi, rect, mvp), cursor, EDGE_PICK_R)
    }

    // ===================================================================
    // Slabs — floors and ceilings
    // ===================================================================

    /// Add a horizontal slab spanning `footprint`, `thickness` thick, with its **top face
    /// at `top_z`**. Returns `(feature id, exact)`.
    ///
    /// `exact == false` means the outline is not a rectangle and the slab was built from
    /// its bounding box, so it OVER-COVERS (an L-shaped room gets a rectangular floor).
    /// The caller must report that — a silently wrong floor is worse than none, and it
    /// would hand the light calc a surface that is not the room.
    ///
    /// Why the limit exists: `Primitive::Box` is the only slab-shaped primitive
    /// `cad_solid` has, and an arbitrary profile needs the extrusion primitive that is
    /// still awaiting sign-off (`mentor MD/CAD_SOLID_EXTRUSION_PRIMITIVE_SPEC_2026-07-23.md`).
    /// A rotated rectangle IS exact — `Placement::spin_deg` carries the angle.
    /// Add a horizontal slab spanning `footprint`, `thickness` thick, top face at `top_z`.
    ///
    /// A slab is just an EXTRUSION of the outline by its thickness, so it is exact for ANY
    /// shape — L-rooms, circles, arbitrary polygons — not only rectangles. (It used to
    /// fall back to a bounding box for non-rectangles, which is why every non-rectangular
    /// floor came out a plain rectangle.) Returns `None` if the outline is not a valid
    /// closed profile (too few points / no area / self-crossing).
    pub fn add_slab(&mut self, footprint: &[Vec2], thickness: f32, top_z: f32) -> Option<u32> {
        let t = thickness.max(0.01);
        let (profile, centre, w, d) = self.model.add_profile(footprint).ok()?;
        // An extrusion rises +Z from its placement, so lift so the TOP face lands on
        // `top_z` — a floor's top is what you stand on, a ceiling's underside what you see.
        let placement = Placement { u: centre.x, v: centre.y, lift: top_z - t, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 };
        let p = Primitive::Extrusion { profile, h: t, w, d };
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.dirty = true;
        Some(id)
    }

    /// BUILDING OUTLINE: extrude a closed outline into one solid mass on the active
    /// storey, rising to `height`.
    ///
    /// This is what the greyed-out Building-outline row was waiting for. Unlike
    /// [`Self::add_slab`], an arbitrary shape is EXACT here — no bounding box — because
    /// `Primitive::Extrusion` carries the real profile.
    ///
    /// Returns the new feature id, or the reason the outline was refused.
    pub fn add_building_outline(
        &mut self, footprint: &[Vec2], height: f32,
    ) -> Result<u32, cad_solid::ProfileError> {
        let (profile, centre, w, d) = self.model.add_profile(footprint)?;
        let placement = Placement {
            u: centre.x, v: centre.y, lift: self.active_base_z(), spin_deg: 0.0,
            pitch_deg: 0.0, roll_deg: 0.0,
        };
        let id = self.model.push(
            BoolOp::Union,
            Plane::default(),
            placement,
            Primitive::Extrusion { profile, h: height.max(0.01), w, d },
        );
        self.selection = vec![id];
        self.dirty = true;
        Ok(id)
    }

    /// ROOM: carve an interior space out of the building solid from a closed outline.
    ///
    /// A building is a SOLID mass; a room is the void inside it. The outline is extruded
    /// and SUBTRACTED (`BoolOp::Difference`), and the void is inset vertically by a floor
    /// slab and a ceiling slab, so what remains around it reads as a real room — walls
    /// (the material between the outline and the building's edge), a floor below, and a
    /// ceiling above.
    ///
    /// Requires an existing solid to cut from: `csg::eval` treats the FIRST feature as the
    /// base regardless of its op, so a lone Difference would perversely render as a solid.
    /// Refused with [`RoomError::NoBuilding`] until a building exists.
    /// Carve the room's interior column out of an enclosing SOLID building, turning that
    /// building into a WALL (an annulus around the room). Returns true if a carve happened.
    ///
    /// "Enclosing building" = a THICK Union feature whose outline CONTAINS every point of the
    /// room footprint. The carve is the room footprint extruded from `base` up through the
    /// building's top, subtracted (`BoolOp::Difference`). The Difference feature is placed
    /// IMMEDIATELY AFTER the building so the group-based `eval` applies it to that body.
    fn carve_interior_from_building(&mut self, footprint: &[Vec2], base: f32) -> bool {
        // Find the enclosing building and its feature index + top height.
        let mut target: Option<(usize, f32)> = None;
        for (i, f) in self.model.features.iter().enumerate() {
            if f.op != BoolOp::Union {
                continue;
            }
            let (mn, mx) = f.world_aabb();
            if (mx.z - mn.z) <= 0.5 {
                continue; // a thin slab is a floor/ceiling, not a building mass
            }
            if let Some(outline) = self.feature_world_outline(f) {
                if outline.len() >= 3 && footprint.iter().all(|p| point_in_poly(&outline, p.x, p.y)) {
                    target = Some((i, mx.z)); // last (top-most in list) enclosing solid wins
                }
            }
        }
        let Some((idx, top)) = target else { return false };
        // Build the void, then move it to sit right after the building it cuts.
        let Ok((profile, centre, w, d)) = self.model.add_profile(footprint) else {
            return false;
        };
        let void_h = (top - base).max(0.1) + 0.02; // punch fully through the building
        let placement = Placement { u: centre.x, v: centre.y, lift: base, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 };
        self.model.push(
            BoolOp::Difference,
            Plane::default(),
            placement,
            Primitive::Extrusion { profile, h: void_h, w, d },
        );
        // `push` appended at the end; relocate it to just after the building feature so the
        // difference cuts the BUILDING body and nothing else.
        if let Some(void) = self.model.features.pop() {
            self.model.features.insert(idx + 1, void);
        }
        self.dirty = true;
        true
    }

    pub fn add_room(&mut self, footprint: &[Vec2]) -> Result<u32, RoomError> {
        // CONSTRUCTIVE room — built from an outline as a complete enclosed space, with NO
        // pre-existing building required:
        //
        //   floor slab   [base, base+floor]                       — always
        //   perimeter    walls on each edge, [base+floor, +height]— the room's walls
        //   walls
        //   ceiling slab [base+floor+height, +ceiling]            — unless open to sky
        //
        // This is what "draw a room, get a room" should mean. (The old behaviour carved a
        // void from a solid building — which left a hollow ring whenever there was no
        // matching building.)
        let base = self.active_base_z();
        let floor_t = self.room_floor.max(0.02);
        let h = self.room_height.max(0.05);
        let wall_t = self.wall_thickness.max(0.02);

        // If this room sits inside a SOLID building, carve its interior column out of that
        // building so the building becomes a WALL (an annulus around the room) rather than a
        // solid cap. Then hiding the room's ceiling reveals the floor while the surrounding
        // wall — its own solid, with its own top — stays. Without this, a solid building
        // over a room can never be "seen into".
        self.carve_interior_from_building(footprint, base);

        // Distinct default colours (≈ real reflectances) so floor / walls / ceiling are
        // TELLABLE APART from any angle — including straight down, where hiding the light
        // ceiling to reveal the dark floor is now an obvious change.
        const FLOOR_COL: [f32; 3] = [0.34, 0.31, 0.28];   // dark, ~0.2
        const WALL_COL: [f32; 3] = [0.62, 0.62, 0.64];    // mid, ~0.5
        const CEIL_COL: [f32; 3] = [0.90, 0.90, 0.93];    // light, ~0.7

        // FLOOR slab: top face at base + floor_t, so the walls stand on it.
        let floor_id = match self.add_slab(footprint, floor_t, base + floor_t) {
            Some(id) => id,
            None => return Err(RoomError::Profile(cad_solid::ProfileError::Degenerate)),
        };
        self.feature_color.insert(floor_id, FLOOR_COL);

        // WALLS: one box per outline edge, sitting on the floor slab.
        let wall_base = base + floor_t;
        for e in footprint.windows(2) {
            if let Some(id) = self.push_wall_box(e[0], e[1], wall_t, h, wall_base) {
                self.feature_color.insert(id, WALL_COL);
            }
        }
        // Close the loop if the outline wasn't already closed.
        if footprint.len() >= 3 {
            let (a, b) = (footprint[footprint.len() - 1], footprint[0]);
            if (a - b).length() > 1e-4 {
                if let Some(id) = self.push_wall_box(a, b, wall_t, h, wall_base) {
                    self.feature_color.insert(id, WALL_COL);
                }
            }
        }

        // CEILING slab on top of the walls, tracked so it can be hidden — unless open sky.
        if !self.room_open_top {
            let ct = self.ceiling_thickness.max(0.02);
            if let Some(cid) = self.add_slab(footprint, ct, wall_base + h + ct) {
                self.feature_color.insert(cid, CEIL_COL);
                self.ceilings.insert(cid);
            }
        }

        self.selection = vec![floor_id];
        self.dirty = true;
        Ok(floor_id)
    }

    /// Floor of the active storey — its top face is the level the walls stand on.
    ///
    /// Note the consequence: the slab's BODY lies below that base, so
    /// [`Self::features_on_storey`] records an upper floor on the storey BENEATH it. That
    /// is structurally what it is — level 1's floor and level 0's ceiling are one slab —
    /// and it is what decides which level `delete_storey` takes it with.
    pub fn add_floor(&mut self, footprint: &[Vec2], thickness: f32) -> Option<u32> {
        let z = self.active_base_z();
        self.add_slab(footprint, thickness, z)
    }

    /// Ceiling of the active storey — its top face is the floor level of the storey above,
    /// so a ceiling and the floor above it meet rather than overlap.
    pub fn add_ceiling(&mut self, footprint: &[Vec2], thickness: f32) -> Option<u32> {
        let i = self.active_storey.min(self.storeys.len().saturating_sub(1));
        let z = self.storey_base_z(i) + self.storeys[i].height;
        let id = self.add_slab(footprint, thickness, z)?;
        // Track it as a ceiling so "Hide ceilings" hides THIS one too — not just room
        // ceilings. Without this, a ceiling made with the Make-ceiling tool was unhideable.
        self.ceilings.insert(id);
        Some(id)
    }

    /// Drop the 3D selection AND the cached highlight mesh key. Both must go together:
    /// leaving `sel_key` set would make `sync_selection_mesh` think the (now empty)
    /// selection is already drawn, and the old highlight would linger.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.sel_furniture.clear();
        self.sel_key.clear();
    }

    /// PERSISTENCE: capture the 3D model for the sidecar. Camera, selection and any live
    /// sketch session are deliberately NOT captured — they are view state, not the
    /// building.
    /// Persist the full model with furniture geometry ENCODED inline (blobs). Used by the
    /// synchronous (dead) save path and by tests. The threaded save path uses [`Self::to_persist_lite`]
    /// + [`Self::furniture_geom_flat`] so the deflate happens on a worker.
    pub fn to_persist(&self) -> crate::simlux_io::FactoryDoc {
        self.build_persist(true)
    }

    /// Persist everything EXCEPT furniture geometry (blobs left empty). Pair with
    /// [`Self::furniture_geom_flat`] and encode the geometry on a worker thread.
    pub fn to_persist_lite(&self) -> crate::simlux_io::FactoryDoc {
        self.build_persist(false)
    }

    /// Each furniture asset's flattened geometry, for off-thread compression. Order matches
    /// `to_persist_lite().furniture_lib`, so a worker can fill blob `i` from raw `i`.
    pub fn furniture_geom_flat(&self) -> Vec<FurnitureGeomRaw> {
        self.furniture_lib
            .iter()
            .map(|a| FurnitureGeomRaw {
                pos: flat3(&a.positions),
                nrm: flat3(&a.normals),
                uv: if a.uvs.is_empty() { Vec::new() } else { flat2(&a.uvs) },
                alpha: a.alpha.clone(),
            })
            .collect()
    }

    fn build_persist(&self, encode_geom: bool) -> crate::simlux_io::FactoryDoc {
        crate::simlux_io::FactoryDoc {
            model: self.model.clone(),
            walls: self
                .walls
                .iter()
                .map(|w| crate::simlux_io::WallRec {
                    footprint: w.footprint.iter().map(|p| [p.x, p.y]).collect(),
                    segments: w.segments.clone(),
                    thickness: w.thickness,
                    height: w.height,
                    rake_deg: w.rake_deg,
                    base_z: w.base_z,
                })
                .collect(),
            wall_height: self.wall_height,
            building_height: self.building_height,
            storeys: self
                .storeys
                .iter()
                .map(|s| crate::simlux_io::StoreyRec { name: s.name.clone(), height: s.height })
                .collect(),
            active_storey: self.active_storey,
            ceilings: self.ceilings.iter().copied().collect(),
            furniture_lib: self
                .furniture_lib
                .iter()
                .map(|a| crate::simlux_io::FurnitureAssetRec {
                    name: a.name.clone(),
                    positions: Vec::new(), // geometry rides in the compact blobs below (or a worker)
                    normals: Vec::new(),
                    color: a.color,
                    uvs: Vec::new(),
                    // When `encode_geom` is false the blobs are left EMPTY and a save worker fills
                    // them from `furniture_geom_flat()` — keeping the deflate off the UI thread.
                    pos_b64: if encode_geom { encode_f32_blob(&flat3(&a.positions)) } else { String::new() },
                    nrm_b64: if encode_geom { encode_f32_blob(&flat3(&a.normals)) } else { String::new() },
                    uv_b64: if encode_geom && !a.uvs.is_empty() { encode_f32_blob(&flat2(&a.uvs)) } else { String::new() },
                    alpha_b64: if encode_geom && !a.alpha.is_empty() { encode_f32_blob(&a.alpha) } else { String::new() },
                    source_path: a.source_path.clone().unwrap_or_default(),
                    alpha_resolved: a.alpha_resolved,
                    part_ids: a.part_ids.clone(),
                })
                .collect(),
            furniture: self
                .furniture
                .iter()
                .map(|f| crate::simlux_io::FurnitureInstRec {
                    asset: f.asset,
                    pos: f.pos,
                    scale: f.scale,
                    rot_deg: f.rot[2],
                    rot_xy: [f.rot[0], f.rot[1]],
                    color: f.color,
                    texture: f.texture,
                    fit: f.fit,
                    surface_texture: f.surface_texture.iter().map(|(&g, &t)| (g, t)).collect(),
                    cuts: f.cuts.iter().map(crate::simlux_io::MeshCutRec::of).collect(),
                    base_asset: f.base_asset,
                })
                .collect(),
            feature_colors: self.feature_color.iter().map(|(&k, &v)| (k, v)).collect(),
            surface_colors: self
                .surface_color
                .iter()
                .map(|(&(f, a, b, c, d), &col)| (f, a, b, c, d, col))
                .collect(),
            textures: self
                .textures
                .iter()
                .map(|t| crate::simlux_io::TextureRec {
                    name: t.name.clone(),
                    w: t.w,
                    h: t.h,
                    scale: t.scale,
                    offset: t.offset,
                    rot_deg: t.rot_deg,
                    opacity: t.opacity,
                    reflect: t.reflect,
                    png_b64: t.encoded_png(),
                    proc: t.proc.map(|p| crate::simlux_io::ProcRec {
                        pattern: p.pattern.tag().to_string(),
                        col_a: p.col_a,
                        col_b: p.col_b,
                        scale: p.scale,
                        detail: p.detail,
                        rough: p.rough,
                        contrast: p.contrast,
                        ramp: p.ramp,
                        surf_rough: p.surf_rough,
                        bump: p.bump,
                    }),
                    normal_map: t.normal_map,
                    rough_map: t.rough_map,
                    roughness: t.roughness,
                    metallic: t.metallic,
                    ior: t.ior,
                    emission: t.emission,
                    emission_strength: t.emission_strength,
                    transmission: t.transmission,
                })
                .collect(),
            feature_textures: self.feature_texture.iter().map(|(&k, &v)| (k, v)).collect(),
            surface_textures: self
                .surface_texture
                .iter()
                .map(|(&(f, a, b, c, d), &ti)| (f, a, b, c, d, ti))
                .collect(),
            feature_groups: self.feature_group.iter().map(|(&f, &g)| (f, g)).collect(),
        }
    }

    /// PERSISTENCE: restore a model read from the sidecar. Returns the number of wall
    /// records DROPPED as unusable, so the caller can report it — a silently vanishing
    /// wall would look like data loss with no explanation.
    ///
    /// A wall is dropped when its footprint is too short to extrude (< 2 points) or when
    /// any segment id names a feature the model does not contain — that link is what
    /// makes a wall editable, and a dangling one would panic or mis-edit later.
    ///
    /// Leaves the model `dirty` rather than re-evaluating: `recompute()` walks a BSP per
    /// boolean, and the caller decides when to pay that.
    ///
    /// Decodes furniture geometry inline (blob → mesh + AABB). For a multi-million-vertex asset
    /// that decode is SECONDS of CPU — the threaded load path instead calls
    /// [`Self::decode_furniture_lib`] on a worker and hands the result to
    /// [`Self::apply_persist_prebuilt`], so the UI thread never blocks.
    pub fn apply_persist(&mut self, mut d: crate::simlux_io::FactoryDoc) -> usize {
        let lib = Self::decode_furniture_lib(std::mem::take(&mut d.furniture_lib));
        self.apply_persist_prebuilt(d, lib)
    }

    /// Decode furniture records (blob/JSON → `FurnitureAsset` with its cached AABB). Pure (no
    /// `self`), so a worker thread can run it off the UI thread — this is the heavy part of a load.
    pub fn decode_furniture_lib(recs: Vec<crate::simlux_io::FurnitureAssetRec>) -> Vec<FurnitureAsset> {
        recs.into_iter()
            .map(|a| {
                let color = if a.color == [0.0, 0.0, 0.0] { [0.82, 0.82, 0.84] } else { a.color };
                // Prefer the compact blobs; fall back to legacy JSON arrays for old sidecars.
                let un3 = |v: Vec<f32>| v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect::<Vec<[f32; 3]>>();
                let un2 = |v: Vec<f32>| v.chunks_exact(2).map(|c| [c[0], c[1]]).collect::<Vec<[f32; 2]>>();
                let positions = if !a.pos_b64.is_empty() { un3(decode_f32_blob(&a.pos_b64)) } else { a.positions };
                let normals = if !a.nrm_b64.is_empty() { un3(decode_f32_blob(&a.nrm_b64)) } else { a.normals };
                let uvs = if !a.uv_b64.is_empty() { un2(decode_f32_blob(&a.uv_b64)) } else { a.uvs };
                let alpha = if !a.alpha_b64.is_empty() { decode_f32_blob(&a.alpha_b64) } else { Vec::new() };
                let mut fa = FurnitureAsset::new(a.name, positions, normals, color);
                fa.uvs = uvs;
                fa.alpha = alpha;
                fa.source_path = (!a.source_path.is_empty()).then_some(a.source_path);
                fa.alpha_resolved = a.alpha_resolved;
                if a.part_ids.len() == fa.positions.len() / 3 {
                    fa.part_ids = a.part_ids; // keep per-piece grouping across a reload
                }
                fa
            })
            .collect()
    }

    /// Install a persisted model whose furniture library was ALREADY decoded (see
    /// [`Self::decode_furniture_lib`]). Everything here is cheap — it does NOT touch furniture
    /// geometry — so it is safe to run on the main thread after a worker did the heavy decode.
    pub fn apply_persist_prebuilt(
        &mut self,
        d: crate::simlux_io::FactoryDoc,
        furniture_lib: Vec<FurnitureAsset>,
    ) -> usize {
        let have: std::collections::HashSet<u32> =
            d.model.features.iter().map(|f| f.id).collect();
        let mut dropped = 0usize;
        let mut walls = Vec::with_capacity(d.walls.len());
        for w in d.walls {
            let usable = w.footprint.len() >= 2
                && !w.segments.is_empty()
                && w.segments.iter().all(|id| have.contains(id));
            if !usable {
                dropped += 1;
                continue;
            }
            walls.push(WallInst {
                footprint: w.footprint.iter().map(|p| Vec2::new(p[0], p[1])).collect(),
                segments: w.segments,
                thickness: w.thickness,
                height: w.height,
                rake_deg: w.rake_deg,
                base_z: w.base_z,
            });
        }
        self.model = d.model;
        self.walls = walls;
        // A zero height means the sidecar predates the field — keep the live default
        // rather than adopting a building of no height.
        if d.wall_height > 0.0 {
            self.wall_height = d.wall_height;
        }
        if d.building_height > 0.0 {
            self.building_height = d.building_height;
        }
        // A pre-storeys sidecar has no levels. Substitute the single ground storey rather
        // than leaving `storeys` empty, which would make `active_storey` index nothing.
        // Zero-height levels are dropped for the same reason (no z band ⇒ nothing can
        // ever belong to them).
        let levels: Vec<Storey> = d
            .storeys
            .into_iter()
            .filter(|s| s.height >= MIN_STOREY_H)
            .map(|s| Storey { name: s.name, height: s.height })
            .collect();
        self.storeys = if levels.is_empty() {
            vec![Storey { name: "Ground".into(), height: self.building_height.max(MIN_STOREY_H) }]
        } else {
            levels
        };
        self.active_storey = d.active_storey.min(self.storeys.len() - 1);
        // Ceilings that still exist in the restored model.
        let have: std::collections::HashSet<u32> =
            self.model.features.iter().map(|f| f.id).collect();
        self.ceilings = d.ceilings.into_iter().filter(|id| have.contains(id)).collect();
        // Furniture library was already decoded (off-thread on the live load path).
        self.furniture_lib = furniture_lib;
        // Textures first — furniture/feature assignments index into this list. Decode each;
        // a texture that fails to decode becomes a 1×1 placeholder so LATER indices stay valid.
        self.textures = d
            .textures
            .iter()
            .map(|r| decode_texture_rec(r).unwrap_or_else(|| TextureAsset::new(r.name.clone(), 1, 1, vec![200, 200, 200, 255])))
            .collect();
        let ntex = self.textures.len();
        // Keep only instances whose asset still exists.
        let nlib = self.furniture_lib.len();
        self.furniture = d
            .furniture
            .into_iter()
            .filter(|f| f.asset < nlib)
            .map(|f| FurnitureInst {
                asset: f.asset,
                pos: f.pos,
                scale: if f.scale > 0.0 { f.scale } else { 1.0 },
                fit: f.fit,
                rot: [f.rot_xy[0], f.rot_xy[1], f.rot_deg],
                color: f.color,
                texture: f.texture.filter(|&t| t < ntex), // drop a dangling index
                surface_texture: f
                    .surface_texture
                    .iter()
                    .filter(|(_, t)| *t < ntex)
                    .map(|(g, t)| (*g, *t))
                    .collect(),
                cuts: f.cuts.iter().map(|c| c.to_cut()).collect(),
                base_asset: f.base_asset.filter(|&b| b < nlib),
                ..Default::default()
            })
            .collect();
        self.feature_color = d.feature_colors.into_iter().collect();
        self.feature_texture = d
            .feature_textures
            .into_iter()
            .filter(|&(id, t)| have.contains(&id) && t < ntex)
            .collect();
        self.surface_color = d
            .surface_colors
            .into_iter()
            .map(|(f, a, b, c, dd, col)| ((f, a, b, c, dd), col))
            .collect();
        self.surface_texture = d
            .surface_textures
            .into_iter()
            .filter(|&(f, ..)| have.contains(&f))
            .filter(|&(.., ti)| ti < ntex)
            .map(|(f, a, b, c, dd, ti)| ((f, a, b, c, dd), ti))
            .collect();
        self.feature_group = d
            .feature_groups
            .into_iter()
            .filter(|&(id, _)| have.contains(&id))
            .collect();
        self.next_group_id = self.feature_group.values().copied().max().unwrap_or(0) + 1;
        // CUTS are stored as a list, not as geometry, so a loaded piece has to be re-cut before it
        // matches what was saved. Replaying them here (rather than trusting the saved mesh) is what
        // keeps them editable across a reload — and means a later fix to the boolean improves old
        // projects instead of leaving them frozen with their old result baked in.
        for fi in 0..self.furniture.len() {
            if self.furniture[fi].cuts.iter().any(|c| c.enabled) {
                let _ = self.rebuild_cut_asset(fi);
            }
        }
        // Ids are safe to carry across: `Model::push` mints `max(id) + 1`, so restored
        // ids are never reused. Selection, though, indexed the OLD model — drop it.
        self.clear_selection();
        self.dirty = true;
        dropped
    }

    /// DRAW3D: commit the dialog's primitive into the model (at the origin).
    pub fn add_primitive(&mut self, p: Primitive) {
        // Built on the ACTIVE storey, like every other new solid.
        let placement = Placement { lift: self.active_base_z(), ..Placement::default() };
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.selection = vec![id];
        self.dirty = true;
    }

    /// DRAW3D: place a dialog-built primitive at a picked point. The click is a CORNER for
    /// a Box (it extends +w,+d,+h from there) and the CENTRE for everything else.
    pub fn place_primitive(&mut self, p: Primitive, at: Vec3) {
        let plane = Plane::default();
        let uv = plane.to_uv(at);
        let (ox, oy) = match p {
            Primitive::Box { w, d, .. } => (w * 0.5, d * 0.5), // click = the near corner
            _ => (0.0, 0.0),                                   // click = the centre
        };
        // The click gives x,y; the ACTIVE storey gives z. Clicking the ground plane while
        // level 2 is active must build on level 2, not under it.
        let placement = Placement {
            u: uv.x + ox, v: uv.y + oy, lift: self.active_base_z(), spin_deg: 0.0,
            pitch_deg: 0.0, roll_deg: 0.0,
        };
        let id = self.model.push(BoolOp::Union, plane, placement, p);
        self.selection = vec![id];
        self.dirty = true;
    }

    pub fn add_cylinder(&mut self) {
        let p = Primitive::Cylinder { r: self.cyl_r, h: self.cyl_h, sides: self.cyl_sides.max(3) };
        // Built on the ACTIVE storey, like every other new solid.
        let placement = Placement { lift: self.active_base_z(), ..Placement::default() };
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.selection = vec![id];
        self.dirty = true;
    }

    // ── 2D → 3D wall promotion ──────────────────────────────────────────────────────
    // The practical journey (owner, 2026-07-17): draft the wall in 2D with the real
    // `wall` tool (snapping / ortho / corner-join), select it, right-click → Make 3D
    // wall. Each selected `Geom::Wall`'s centerline becomes placed Boxes here.

    /// Extrude ONE footprint edge `a→b` to a placed Box and push it, returning its feature
    /// id (or `None` if degenerate). `a`,`b` are ground-plane centerline points (a 2D
    /// wall's coords ARE the ground uv); the Box keeps `thickness` and rises to `height`.
    /// Pure Box + Placement (see `Plane::world_matrix`), so no `cad_solid` change is needed.
    fn push_wall_box(
        &mut self, a: Vec2, b: Vec2, thickness: f32, height: f32, base_z: f32,
    ) -> Option<u32> {
        let d = b - a;
        let len = d.length();
        if len < 1e-4 || thickness <= 0.0 || height <= 0.0 {
            return None; // ignore degenerate input
        }
        let mid = (a + b) * 0.5;
        let p = Primitive::Box { w: len, d: thickness, h: height };
        let placement = Placement {
            u: mid.x, v: mid.y, lift: base_z, spin_deg: d.y.atan2(d.x).to_degrees(),
            pitch_deg: 0.0, roll_deg: 0.0,
        };
        Some(self.model.push(BoolOp::Union, Plane::default(), placement, p))
    }

    /// Promote a **footprint** (≥ 2 ground-plane points) to a live wall: one Box per edge,
    /// all sharing `thickness` and `height`. The wall stays ALIVE — its footprint and
    /// height are remembered so vertices and rise can be edited later. Degenerate edges are
    /// skipped; returns the new wall's index, or `None` if every edge was degenerate.
    pub fn add_wall(&mut self, footprint: Vec<Vec2>, thickness: f32, height: f32) -> Option<usize> {
        if footprint.len() < 2 {
            return None;
        }
        // New geometry is built on the ACTIVE storey — that is what makes the storey
        // selector mean anything.
        let base_z = self.active_base_z();
        let mut segments = Vec::new();
        for w in footprint.windows(2) {
            if let Some(id) = self.push_wall_box(w[0], w[1], thickness, height, base_z) {
                segments.push(id);
            }
        }
        if segments.is_empty() {
            return None;
        }
        self.walls.push(WallInst {
            footprint, segments, thickness, height, rake_deg: 0.0, base_z,
        });
        self.dirty = true;
        Some(self.walls.len() - 1)
    }

    /// Back-compat + simplest promotion: a single centerline segment → a 2-point wall.
    pub fn add_wall_segment(&mut self, a: Vec2, b: Vec2, thickness: f32, height: f32) {
        self.add_wall(vec![a, b], thickness, height);
    }

    /// Index of the live-wall record OWNING `feature_id` (any of its segments), if any.
    pub fn wall_index(&self, feature_id: u32) -> Option<usize> {
        self.walls.iter().position(|w| w.segments.contains(&feature_id))
    }

    /// Rebuild every segment Box of wall `wi` from its current footprint + params. The old
    /// Boxes are dropped and fresh ones pushed (the segment count changes when a vertex is
    /// added or removed). Both rings follow the one footprint, so they stay coincident.
    /// Segment feature ids change — callers that track a selection must refresh it.
    fn rederive_wall(&mut self, wi: usize) {
        if wi >= self.walls.len() {
            return;
        }
        for id in std::mem::take(&mut self.walls[wi].segments) {
            self.model.remove(id);
        }
        let fp = self.walls[wi].footprint.clone();
        let (t, h) = (self.walls[wi].thickness, self.walls[wi].height);
        // Rebuild at the wall's OWN base, not the active storey's: editing a vertex on
        // the third floor must not drop the wall to the ground.
        let base_z = self.walls[wi].base_z;
        let mut segments = Vec::new();
        for w in fp.windows(2) {
            if let Some(id) = self.push_wall_box(w[0], w[1], t, h, base_z) {
                segments.push(id);
            }
        }
        self.walls[wi].segments = segments;
        self.dirty = true;
    }

    /// Change a live wall's height and re-derive — the "walls are alive" edit. Updates each
    /// segment Box IN PLACE (feature ids stay stable, so a selection survives), keeping each
    /// segment's length and thickness; only the rise changes.
    pub fn set_wall_height(&mut self, feature_id: u32, height: f32) {
        let h = height.max(0.01);
        if let Some(i) = self.wall_index(feature_id) {
            self.walls[i].height = h;
            let t = self.walls[i].thickness;
            let fp = self.walls[i].footprint.clone();
            let segs = self.walls[i].segments.clone();
            for (k, w) in fp.windows(2).enumerate() {
                if let Some(&fid) = segs.get(k) {
                    let len = (w[1] - w[0]).length();
                    if let Some(f) = self.model.get_mut(fid) {
                        f.primitive = Primitive::Box { w: len, d: t, h };
                    }
                }
            }
            self.dirty = true;
        }
    }

    /// Change a live wall's THICKNESS, the twin of [`Self::set_wall_height`]. Updates each
    /// segment Box in place (a Box's `d` IS the wall's thickness), so feature ids stay
    /// stable and a selection — and its handles — survive the edit.
    pub fn set_wall_thickness(&mut self, feature_id: u32, thickness: f32) {
        let t = thickness.max(0.01);
        if let Some(i) = self.wall_index(feature_id) {
            self.walls[i].thickness = t;
            let h = self.walls[i].height;
            let fp = self.walls[i].footprint.clone();
            let segs = self.walls[i].segments.clone();
            for (k, w) in fp.windows(2).enumerate() {
                if let Some(&fid) = segs.get(k) {
                    let len = (w[1] - w[0]).length();
                    if let Some(f) = self.model.get_mut(fid) {
                        f.primitive = Primitive::Box { w: len, d: t, h };
                    }
                }
            }
            self.dirty = true;
        }
    }

    /// Move footprint vertex `vi` of wall `wi` to `to`, then re-derive — this is how a 3D
    /// handle drag "shifts the surface". Because both rings share the footprint, the whole
    /// vertical edge moves together.
    pub fn wall_move_vertex(&mut self, wi: usize, vi: usize, to: Vec2) {
        let ok = matches!(self.walls.get(wi), Some(w) if vi < w.footprint.len());
        if !ok {
            return;
        }
        self.walls[wi].footprint[vi] = to;
        self.rederive_wall(wi);
    }

    /// Insert a vertex at `at` into wall `wi`, splitting the segment between
    /// `footprint[seg]` and `footprint[seg + 1]`. The new corner exists on BOTH the floor
    /// and ceiling rings by construction (they share the footprint). Returns the new
    /// vertex index, or `None` if `seg` is out of range.
    pub fn wall_insert_vertex(&mut self, wi: usize, seg: usize, at: Vec2) -> Option<usize> {
        let n = self.walls.get(wi)?.footprint.len();
        if seg + 1 >= n {
            return None;
        }
        self.walls[wi].footprint.insert(seg + 1, at);
        self.rederive_wall(wi);
        Some(seg + 1)
    }

    /// Delete footprint vertex `vi` of wall `wi`, then re-derive. A wall keeps a minimum of
    /// 2 points (one segment); returns `false` if the delete was rejected.
    pub fn wall_delete_vertex(&mut self, wi: usize, vi: usize) -> bool {
        match self.walls.get(wi) {
            Some(w) if w.footprint.len() > 2 && vi < w.footprint.len() => {}
            _ => return false,
        }
        self.walls[wi].footprint.remove(vi);
        self.rederive_wall(wi);
        true
    }

    pub fn erase_selection(&mut self) {
        for id in std::mem::take(&mut self.selection) {
            self.model.remove(id);
            self.ceilings.remove(&id); // keep the ceiling set in step with the model
            self.feature_group.remove(&id); // drop it from any group
        }
        self.dirty = true;
    }

    /// One representative feature id per CUTOUT OPENING. A through-cut makes one `Difference`
    /// per body it passes through (e.g. the building shell AND the room), all sharing a profile
    /// and placement — those are ONE opening, so the list shows one row, not one per body.
    pub fn cutout_ids(&self) -> Vec<u32> {
        let mut out = Vec::new();
        let mut seen: Vec<(u32, i32, i32)> = Vec::new(); // (profile, u·1e3, v·1e3)
        for f in &self.model.features {
            if f.op != cad_solid::BoolOp::Difference {
                continue;
            }
            // Extrusion cuts (the real openings) dedup by profile+placement; any other
            // Difference is listed on its own.
            if let cad_solid::Primitive::Extrusion { profile, .. } = f.primitive {
                let key = (profile, (f.placement.u * 1000.0) as i32, (f.placement.v * 1000.0) as i32);
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);
            }
            out.push(f.id);
        }
        out
    }

    /// Every `Difference` feature that belongs to the SAME opening as `id` (same profile +
    /// placement) — the sibling cuts in the shell, the room, etc. Moving or deleting them
    /// together keeps the opening coherent.
    pub fn cutout_siblings(&self, id: u32) -> Vec<u32> {
        let Some(f) = self.model.features.iter().find(|g| g.id == id) else { return vec![id] };
        let cad_solid::Primitive::Extrusion { profile, .. } = f.primitive else { return vec![id] };
        let (pu, pv) = (f.placement.u, f.placement.v);
        self.model.features.iter()
            .filter(|g| g.op == cad_solid::BoolOp::Difference)
            .filter(|g| matches!(g.primitive, cad_solid::Primitive::Extrusion { profile: pp, .. } if pp == profile))
            .filter(|g| (g.placement.u - pu).abs() < 1e-4 && (g.placement.v - pv).abs() < 1e-4)
            .map(|g| g.id)
            .collect()
    }

    /// A cutout's world size `[w, h, d]` (from its AABB), for labelling the list.
    pub fn cutout_size(&self, id: u32) -> Option<[f32; 3]> {
        let f = self.model.features.iter().find(|f| f.id == id)?;
        let (mn, mx) = f.world_aabb();
        Some([mx.x - mn.x, mx.y - mn.y, mx.z - mn.z])
    }

    /// Select an opening — ALL sibling cuts — so it highlights and its Position/Dimensions load
    /// into the panel, and a 3D move/nudge moves every body's hole together (not just one).
    pub fn select_cutout(&mut self, id: u32) {
        self.sel_furniture.clear();
        self.selection = self.cutout_siblings(id);
    }

    /// Remove an opening (all its sibling cuts) so the hole fills back in on the next re-eval.
    pub fn delete_cutout(&mut self, id: u32) {
        for sib in self.cutout_siblings(id) {
            self.model.remove(sib);
            self.selection.retain(|&s| s != sib);
            self.feature_color.remove(&sib);
            self.feature_texture.remove(&sib);
        }
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.ceilings.clear();
        self.model = Model::default();
        self.selection.clear();
        self.walls.clear();
        self.dirty = true;
    }

    /// Re-evaluate the CSG tree. Call ONLY when idle — csgrs walks a BSP per boolean.
    ///
    /// Hiding ceilings is NOT done here — it is a RENDER-time filter in [`Self::scene_verts`]
    /// (keyed on each triangle's feature id), so toggling it is instant and never depends
    /// on a re-evaluation. `cached` always holds the FULL model (undo / save / the light
    /// calc all see every ceiling).
    pub fn recompute(&mut self) {
        self.cached = self.model.eval();
        self.ceiling_caps = self.detect_ceiling_caps();
        self.sel_key.clear(); // the model changed → the selection's mesh is stale
        self.ensure_sel_mesh();
        self.dirty = false;
        // The solids changed → the cached opaque render buffer is stale.
        self.geom_version = self.geom_version.wrapping_add(1);
    }

    /// Is feature `id` hidden while "Hide ceilings" is on? True if it is a tracked room
    /// ceiling OR a geometrically-detected top cap. The geometry arm is what makes the
    /// toggle RELIABLE — it works even when the tracked id-set has drifted (the field bug).
    pub fn is_hidden_ceiling(&self, id: u32) -> bool {
        self.ceilings.contains(&id) || self.ceiling_caps.contains(&id)
    }

    /// Find the feature ids that are ceiling / roof CAPS purely from geometry: a thin,
    /// horizontal slab that sits at the TOP of the model. This is what "hide the ceiling"
    /// should target, and it cannot drift like the hand-maintained `ceilings` set.
    ///
    /// A cap must be (per its own world AABB):
    ///   * THIN in Z         — a slab, not a wall or a tall solid,
    ///   * FLAT              — far wider than it is thick,
    ///   * ELEVATED          — its underside is well above the ground (so a FLOOR at z≈0 is
    ///                         never mistaken for a ceiling),
    ///   * TOPMOST           — its top is level with the model's highest point (so an
    ///                         intermediate storey's slab is NOT hidden, only the roof/ceiling).
    /// World outline (XY polygon) of a slab/box feature, or `None` for a shape with no
    /// closed outline. Extrusions carry their real profile; a Box is its rotated rectangle.
    fn feature_world_outline(&self, f: &cad_solid::Feature) -> Option<Vec<Vec2>> {
        match &f.primitive {
            Primitive::Extrusion { profile, .. } => {
                let p = self.model.profile(*profile)?;
                // Stored pts are centred on the profile; placement (u,v) is that centre.
                Some(
                    p.pts
                        .iter()
                        .map(|q| Vec2::new(q[0] + f.placement.u, q[1] + f.placement.v))
                        .collect(),
                )
            }
            Primitive::Box { w, d, .. } => {
                let (hw, hd) = (w * 0.5, d * 0.5);
                let a = f.placement.spin_deg.to_radians();
                let (c, s) = (a.cos(), a.sin());
                Some(
                    [(-hw, -hd), (hw, -hd), (hw, hd), (-hw, hd)]
                        .iter()
                        .map(|(x, y)| {
                            Vec2::new(
                                f.placement.u + x * c - y * s,
                                f.placement.v + x * s + y * c,
                            )
                        })
                        .collect(),
                )
            }
            _ => None,
        }
    }

    fn detect_ceiling_caps(&self) -> std::collections::HashSet<u32> {
        let mut caps = std::collections::HashSet::new();
        let Some((_, world_mx)) = self.cached.bounds() else {
            return caps;
        };
        let world_top = world_mx[2];
        // Cache each Union feature's world AABB once — used twice below.
        let unions: Vec<(u32, Vec3, Vec3)> = self
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Union) // cutters (room voids) aren't surfaces
            .map(|f| {
                let (mn, mx) = f.world_aabb();
                (f.id, mn, mx)
            })
            .collect();

        // PASS 1 — thin flat ELEVATED TOPMOST slabs: these are ceiling / roof CAPS.
        for &(id, mn, mx) in &unions {
            let dz = mx.z - mn.z;
            let (dx, dy) = (mx.x - mn.x, mx.y - mn.y);
            let thin = dz <= 0.5;
            let flat = dx > 3.0 * dz && dy > 3.0 * dz;
            let elevated = mn.z > 0.5;
            let topmost = mx.z >= world_top - 0.05;
            if thin && flat && elevated && topmost {
                caps.insert(id);
            }
        }

        caps
    }

    /// Refresh the selection mesh if the selection moved on (cheap no-op otherwise).
    pub fn sync_selection_mesh(&mut self) {
        self.ensure_sel_mesh();
    }

    /// Upper bound for `cam_dist`, scaled to the model so you can always dolly back far
    /// enough to frame the WHOLE scene — a fixed cap (was 400) was too small for large
    /// imports (e.g. an architectural DXF in millimetres, span 100 000+). 20× the largest
    /// span, never below 400 so small/empty scenes keep the old generous headroom.
    pub fn max_cam_dist(&self) -> f32 {
        self.cached
            .bounds()
            .map(|(mn, mx)| {
                let span = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(mx[2] - mn[2]);
                (span * 20.0).max(400.0)
            })
            .unwrap_or(400.0)
    }

    /// Zoom-extents: the ONLY thing that moves `cam_target`.
    /// World-space AABB of everything drawn — the CSG scene UNION every furniture instance's posed
    /// bounding box. Used to frame the sun's shadow map. `None` when the scene is empty.
    pub fn render_bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut mn = Vec3::splat(f32::MAX);
        let mut mx = Vec3::splat(f32::MIN);
        let mut any = false;
        if let Some((a, b)) = self.cached.bounds() {
            mn = mn.min(Vec3::from(a));
            mx = mx.max(Vec3::from(b));
            any = true;
        }
        for (i, inst) in self.furniture.iter().enumerate() {
            let Some(asset) = self.furniture_lib.get(inst.asset) else { continue };
            let Some(model) = self.furniture_model_matrix(i) else { continue };
            let m = glam::Mat4::from_cols_array(&model);
            let (lo, hi) = (asset.local_min, asset.local_max);
            for cx in [lo[0], hi[0]] {
                for cy in [lo[1], hi[1]] {
                    for cz in [lo[2], hi[2]] {
                        let w = m.transform_point3(Vec3::new(cx, cy, cz));
                        mn = mn.min(w);
                        mx = mx.max(w);
                        any = true;
                    }
                }
            }
        }
        if any {
            Some((mn, mx))
        } else {
            None
        }
    }

    /// A texture's representative solid appearance for the offline render export: `(rgb, roughness,
    /// opacity)`. Procedural → its ramp midpoint; image → its average colour.
    fn export_tex_rep(&self, ti: usize) -> ([f32; 3], f32, f32) {
        match self.textures.get(ti) {
            Some(t) => {
                let rgb = t.proc.map(|p| p.avg_color()).unwrap_or(t.avg);
                (rgb, t.roughness, t.opacity)
            }
            None => ([0.72, 0.72, 0.72], 0.5, 1.0),
        }
    }

    /// Gather the whole drawn scene as world-space triangles with a resolved appearance each, for
    /// [`crate::radiance_export`]. CSG building + every furniture instance, each triangle's colour /
    /// roughness / opacity resolved from its per-surface or whole-object texture, else its colour.
    /// Geometry is rotated by `−north_offset` about Z so the model's north lines up with gensky's
    /// (true) north — so the exported `gensky` sun matches the viewport.
    /// The PROCEDURAL definition of every material, indexed by texture id — the table
    /// [`crate::radiance_export::ExportTri::material`] points into. Built alongside
    /// [`Self::export_render_tris`] so the path tracer can evaluate the same pattern the viewport
    /// evaluates in its shader, instead of being handed one averaged colour per surface.
    pub fn export_proc_table(&self) -> Vec<Option<ProcDef>> {
        if self.clay_mode {
            return Vec::new(); // a light study is deliberately material-free
        }
        self.textures.iter().map(|t| t.proc).collect()
    }

    /// The IMAGES the path tracer should sample, plus the material→image index parallel to
    /// [`Self::export_proc_table`]. A material with a procedural, or with only a 1×1 colour
    /// swatch, contributes nothing: the swatch IS the flat albedo the tracer already had, and
    /// paying for a texture fetch to read one texel back is pure cost.
    ///
    /// The pixel data is cloned once here, at the start of a render. For the villa that is ~62 MB
    /// copied once per render start — worth measuring if it ever shows up, but it is not per frame
    /// and not per sample.
    pub fn export_texture_table(&self) -> (Vec<crate::pathtrace::TexImage>, Vec<Option<u32>>) {
        if self.clay_mode {
            return (Vec::new(), Vec::new());
        }
        let mut pool = Vec::new();
        let mut index = Vec::with_capacity(self.textures.len());
        for t in &self.textures {
            if t.proc.is_some() || (t.w <= 1 && t.h <= 1) || t.rgba.len() < 4 {
                index.push(None);
                continue;
            }
            index.push(Some(pool.len() as u32));
            pool.push(crate::pathtrace::TexImage {
                w: t.w,
                h: t.h,
                rgba: std::sync::Arc::new(t.rgba.clone()),
                triplanar: t.triplanar,
                tiles_per_m: t.tiles_per_m,
            });
        }
        (pool, index)
    }

    pub fn export_render_tris(&self) -> Vec<crate::radiance_export::ExportTri> {
        const DEF: [f32; 3] = [0.72, 0.72, 0.72];
        let clay = self.clay_mode; // flat grey for a light study (glass keeps its transparency)
        let off = -self.sun.north_offset_deg.to_radians();
        let (c, s) = (off.cos(), off.sin());
        let rot = |p: [f32; 3]| [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]];
        let mut out = Vec::new();

        // Principled extras for a texture index — read by the path tracer; zeroed under clay so a
        // light study stays matte and unlit-by-materials.
        #[derive(Clone, Copy)]
        struct Extras {
            metallic: f32,
            ior: f32,
            emission: [f32; 3],
            clearcoat: f32,
            clearcoat_rough: f32,
            sheen: f32,
            sheen_tint: [f32; 3],
        }
        const BARE: Extras = Extras {
            metallic: 0.0, ior: 1.5, emission: [0.0; 3],
            clearcoat: 0.0, clearcoat_rough: 0.1, sheen: 0.0, sheen_tint: [1.0; 3],
        };
        let extras = |ti: Option<usize>| -> Extras {
            if clay {
                return BARE;
            }
            match ti.and_then(|t| self.textures.get(t)) {
                Some(t) => {
                    let e = t.emission;
                    let s = t.emission_strength;
                    Extras {
                        metallic: t.metallic,
                        ior: t.ior,
                        emission: [e[0] * s, e[1] * s, e[2] * s],
                        clearcoat: t.clearcoat,
                        clearcoat_rough: t.clearcoat_rough,
                        sheen: t.sheen,
                        // Linear, like every other colour crossing into the tracer.
                        sheen_tint: crate::color::srgb_to_linear3(t.sheen_tint),
                    }
                }
                None => BARE,
            }
        };

        // ── CSG building (cached is world-space). ──
        for (i, tri) in self.cached.positions.chunks_exact(3).enumerate() {
            let id = self.cached.face_ids.get(i).copied().unwrap_or(0);
            let sk = surface_key(id, tri[0], tri[1], tri[2]);
            let tex = self.surface_texture.get(&sk).or_else(|| self.feature_texture.get(&id)).copied();
            let (rgb, rough, op) = if let Some(ti) = tex {
                self.export_tex_rep(ti)
            } else {
                let col = self.surface_color.get(&sk).or_else(|| self.feature_color.get(&id)).copied().unwrap_or(DEF);
                (col, 0.5, 1.0)
            };
            let rgb = if clay { CLAY_GREY } else { rgb };
            let x = extras(tex);
            out.push(crate::radiance_export::ExportTri {
                verts: [rot(tri[0]), rot(tri[1]), rot(tri[2])],
                rgb,
                roughness: rough,
                opacity: op,
                metallic: x.metallic,
                ior: x.ior,
                emission: x.emission,
                clearcoat: x.clearcoat,
                clearcoat_rough: x.clearcoat_rough,
                sheen: x.sheen,
                sheen_tint: x.sheen_tint,
                material: if clay { None } else { tex.and_then(|t| u16::try_from(t).ok()) },
                // CSG faces carry no UV layer — the viewport projects their textures from world
                // space, and the tracer is told to do the same rather than invent a mapping.
                uv: [[0.0; 2]; 3],
                has_uv: false,
            });
        }

        // ── Furniture (local mesh → world via the model matrix). ──
        for (i, inst) in self.furniture.iter().enumerate() {
            let Some(asset) = self.furniture_lib.get(inst.asset) else { continue };
            let Some(model) = self.furniture_model_matrix(i) else { continue };
            let m = glam::Mat4::from_cols_array(&model);
            let groups = asset.group_geom();
            let ntri = asset.positions.len() / 3;
            for t in 0..ntri {
                let fg = groups.face.get(t).copied().unwrap_or(0);
                let eff = inst.surface_texture.get(&fg).copied().or(inst.texture);
                let (rgb, rough, op) = eff.map(|ti| self.export_tex_rep(ti)).unwrap_or((inst.color, 0.5, 1.0));
                let rgb = if clay { CLAY_GREY } else { rgb };
                let x = extras(eff);
                // Imported glass (per-vertex alpha, e.g. the villa panes) has no texture — carry the
                // mesh's own translucency so the tracer refracts it too.
                let op = if eff.is_none() && tri_is_translucent(asset, t * 3) {
                    op.min(asset.vertex_alpha(t * 3))
                } else {
                    op
                };
                let mut vs = [[0.0f32; 3]; 3];
                for (k, v) in vs.iter_mut().enumerate() {
                    let p = asset.positions[t * 3 + k];
                    let w = m.transform_point3(Vec3::new(p[0], p[1], p[2]));
                    *v = rot([w.x, w.y, w.z]);
                }
                // An imported mesh's OWN UVs, when it has them. This is what lets the tracer put
                // roof tiles on the roof instead of the tiles' average brown.
                let mut uv = [[0.0f32; 2]; 3];
                let has_uv = asset.uvs.len() >= t * 3 + 3;
                if has_uv {
                    for (k, slot) in uv.iter_mut().enumerate() {
                        *slot = asset.uvs[t * 3 + k];
                    }
                }
                out.push(crate::radiance_export::ExportTri {
                    verts: vs, rgb, roughness: rough, opacity: op,
                    metallic: x.metallic, ior: x.ior, emission: x.emission,
                    clearcoat: x.clearcoat, clearcoat_rough: x.clearcoat_rough,
                    sheen: x.sheen, sheen_tint: x.sheen_tint,
                    material: if clay { None } else { eff.and_then(|t| u16::try_from(t).ok()) },
                    uv,
                    has_uv,
                });
            }
        }
        out
    }

    /// The current viewport camera as `(eye, target)` in the EXPORT frame (rotated by −north_offset
    /// like [`Self::export_render_tris`]), so the Radiance view matches what's on screen.
    pub fn export_camera(&self) -> ([f32; 3], [f32; 3]) {
        let (cp, sp) = (self.cam_pitch.cos(), self.cam_pitch.sin());
        let (cy, sy) = (self.cam_yaw.cos(), self.cam_yaw.sin());
        let t = self.cam_target;
        let d = self.cam_dist;
        let eye = [t[0] + cp * cy * d, t[1] + cp * sy * d, t[2] + sp * d];
        let off = -self.sun.north_offset_deg.to_radians();
        let (c, s) = (off.cos(), off.sin());
        let rot = |p: [f32; 3]| [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]];
        (rot(eye), rot(t))
    }

    pub fn fit(&mut self) {
        if let Some((mn, mx)) = self.cached.bounds() {
            self.cam_target = [
                (mn[0] + mx[0]) * 0.5,
                (mn[1] + mx[1]) * 0.5,
                (mn[2] + mx[2]) * 0.5,
            ];
            let span = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(mx[2] - mn[2]);
            self.cam_dist = (span * 2.5).clamp(1.0, self.max_cam_dist());
        } else {
            self.cam_target = [0.0, 0.0, 0.0];
            self.cam_dist = 12.0;
        }
    }

    /// Frame the WHOLE scene — CSG building AND furniture (via [`Self::render_bounds`]) — so an
    /// imported furniture-only model (e.g. the villa) is centred in view, which [`Self::fit`] can't
    /// do (it only knows the CSG bounds).
    pub fn fit_all(&mut self) {
        if let Some((mn, mx)) = self.render_bounds() {
            self.cam_target = [(mn.x + mx.x) * 0.5, (mn.y + mx.y) * 0.5, (mn.z + mx.z) * 0.5];
            let span = (mx.x - mn.x).max(mx.y - mn.y).max(mx.z - mn.z);
            self.cam_dist = (span * 1.6).clamp(1.0, self.max_cam_dist());
        } else {
            self.fit();
        }
    }

    /// Pan the view by a screen drag `(dx, dy)` in pixels — slides the camera target in
    /// the camera's own right/up plane (right = screen →, up = screen ↑). Scaled by
    /// distance so a drag covers a consistent fraction of the view at any zoom.
    ///
    /// Only the TARGET moves, so orientation and zoom are untouched — exactly what pan
    /// should do.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let (cp, sp) = (self.cam_pitch.cos(), self.cam_pitch.sin());
        let (cy, sy) = (self.cam_yaw.cos(), self.cam_yaw.sin());
        let fwd = Vec3::new(cp * cy, cp * sy, sp);
        let right = {
            let x = fwd.cross(Vec3::Z);
            if x.length() < 1e-4 { Vec3::X } else { x.normalize() }
        };
        let up = right.cross(fwd).normalize();
        let k = self.cam_dist * 0.0018;
        let mut t = Vec3::from(self.cam_target);
        // Screen-right drag moves the world LEFT under a fixed camera → target goes right;
        // screen-down drag → target goes up. Signs chosen so content follows the cursor.
        t += right * (-dx * k) + up * (dy * k);
        self.cam_target = t.into();
    }

    /// Snap the orbit camera to a standard view — the nav-gizmo action. Sets `(yaw,
    /// pitch)`; `cam_target`/`cam_dist` are left alone (Zoom-extents is the only thing
    /// that moves the target). `mvp` flips its up-vector near ±90° so Top/Bottom are
    /// stable even though the free-orbit drag clamps pitch to ±1.45.
    pub fn set_view(&mut self, v: StdView) {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
        let (yaw, pitch) = match v {
            StdView::Top    => (-FRAC_PI_2,  FRAC_PI_2),
            StdView::Bottom => (-FRAC_PI_2, -FRAC_PI_2),
            StdView::Front  => (-FRAC_PI_2,  0.0),
            StdView::Back   => ( FRAC_PI_2,  0.0),
            StdView::Right  => ( 0.0,        0.0),
            StdView::Left   => ( PI,         0.0),
            StdView::Iso    => (-FRAC_PI_4,  0.6155), // 35.26° — the classic SE isometric
        };
        self.cam_yaw = yaw;
        self.cam_pitch = pitch;
        self.ortho = true; // standard views are orthographic (true CAD Top/Front/…)
    }

    /// Orbit the camera SQUARE-ON to a plane, centred on `at`.
    ///
    /// This is the prerequisite for drawing on a face rather than a convenience. On a face turned
    /// steeply away from the viewer, one pixel of cursor movement is metres of movement across the
    /// plane and the screen-ray/plane intersection goes ill-conditioned as the two approach
    /// parallel — so no amount of snapping makes an angled face workable. Squaring the view up is
    /// what makes it workable, and it costs one camera move.
    ///
    /// Unlike [`Self::set_view`] this MOVES the target. Putting that face in front of you is the
    /// entire point of the action, and leaving the target where it was would swing the face off
    /// screen instead. `cam_dist` is kept, so it reframes without also re-zooming.
    pub fn look_at_frame(&mut self, frame: &Frame, at: Vec3) {
        let n = frame.normal();
        if n.length_squared() < 1e-9 {
            return; // degenerate frame — nothing to square up to
        }
        let mut n = n.normalize();
        // Face the side the camera is ALREADY on. `pick_face` returns an OUTWARD normal, but
        // "outward" is a property of the solid and not of where you happen to be standing: taking
        // it on trust swings the camera through the wall and leaves you inside the building looking
        // at the back of the face you picked.
        let eye = Vec3::from(crate::light3d::cam_eye(
            self.cam_yaw, self.cam_pitch, self.cam_dist, self.cam_target,
        ));
        if n.dot(eye - at) < 0.0 {
            n = -n;
        }
        // The orbit camera puts the eye at `target + (cos p·cos y, cos p·sin y, sin p)·dist`, so
        // yaw and pitch have to encode the direction TO the eye — which is exactly `n` now.
        self.cam_pitch = n.z.clamp(-1.0, 1.0).asin();
        self.cam_yaw = n.y.atan2(n.x);
        self.cam_target = at.to_array();
        // ORTHOGRAPHIC — and not merely for CAD convention. Under perspective a plane's
        // pixels-per-metre varies across it, so a snap tolerance in pixels and a distance drawn in
        // metres mean different things at the two ends of the same wall. A parallel projection
        // makes that scale constant, which is what lets the 2D drafting tools behave on a face
        // exactly as they behave on the ground.
        self.ortho = true;
    }

    /// Dolly the camera by a factor: `<1` zooms in (closer), `>1` zooms out. The same
    /// clamp as the scroll wheel, so command / gizmo / wheel all agree.
    pub fn zoom_by(&mut self, factor: f32) {
        let max = self.max_cam_dist();
        self.cam_dist = (self.cam_dist * factor).clamp(0.4, max);
    }

    /// Reframe the camera to a screen rectangle — the 2D "zoom window", in 3D. Moves the
    /// target under the box centre (on the target's view plane) and dollies in so the box
    /// fills the viewport height. `vp` is the viewport rect; `p0`,`p1` the drag corners.
    /// Snapshot the camera so `zoom previous` can restore it.
    pub fn zoom_save_prev(&mut self) {
        self.cam_prev = Some([
            self.cam_yaw, self.cam_pitch, self.cam_dist,
            self.cam_target[0], self.cam_target[1], self.cam_target[2],
        ]);
    }

    /// Restore the camera saved before the last zoom (`zoom previous`). No-op if none.
    pub fn zoom_restore_previous(&mut self) {
        if let Some(p) = self.cam_prev.take() {
            self.cam_yaw = p[0];
            self.cam_pitch = p[1];
            self.cam_dist = p[2];
            self.cam_target = [p[3], p[4], p[5]];
        }
    }

    pub fn zoom_window(&mut self, vp: egui::Rect, p0: egui::Pos2, p1: egui::Pos2) {
        self.zoom_save_prev();
        let bh = (p1.y - p0.y).abs().max(1.0);
        let bc = egui::pos2((p0.x + p1.x) * 0.5, (p0.y + p1.y) * 0.5);
        // box centre → normalised device coords (y up)
        let ndc_x = (bc.x - vp.center().x) / (vp.width() * 0.5).max(1.0);
        let ndc_y = -(bc.y - vp.center().y) / (vp.height() * 0.5).max(1.0);
        // camera basis — matches `light3d::mvp`
        let (cp, sp) = (self.cam_pitch.cos(), self.cam_pitch.sin());
        let (cy, sy) = (self.cam_yaw.cos(), self.cam_yaw.sin());
        let fwd = -Vec3::new(cp * cy, cp * sy, sp); // eye → target
        let up_world = if sp.abs() > 0.999 { Vec3::Y } else { Vec3::Z };
        let right = fwd.cross(up_world).normalize();
        let up = right.cross(fwd).normalize();
        // world half-extents on the target's view plane (45° vertical FOV, as in mvp)
        let half_h = (45f32.to_radians() * 0.5).tan() * self.cam_dist;
        let half_w = half_h * (vp.width() / vp.height().max(1.0));
        let t = Vec3::from(self.cam_target) + right * (ndc_x * half_w) + up * (ndc_y * half_h);
        self.cam_target = [t.x, t.y, t.z];
        let factor = (bh / vp.height().max(1.0)).clamp(0.02, 1.0);
        let max = self.max_cam_dist();
        self.cam_dist = (self.cam_dist * factor).clamp(0.4, max);
    }

    /// One-line screen-zoom status for the session recorder: how zoomed-in the camera is
    /// (`dist`), what it is centred on (`target`), and the orbit angles. Comparing this
    /// before vs after a zoom is how we tell whether the zoom actually did anything.
    pub fn zoom_status(&self) -> String {
        format!(
            "dist={:.2} target=({:.1},{:.1},{:.1}) yaw={:.0}° pitch={:.0}°",
            self.cam_dist,
            self.cam_target[0], self.cam_target[1], self.cam_target[2],
            self.cam_yaw.to_degrees(), self.cam_pitch.to_degrees(),
        )
    }

    /// The SELECTED features' own geometry, as a mesh.
    ///
    /// `cached` is the fused CSG result — after booleans, individual features have no
    /// identity in it, so the selected solid's triangles cannot be picked back out.
    /// This evaluates just the selection into its own mesh, which is what both the
    /// selection SHADE and the modifier GHOST draw.
    ///
    /// **Cached on the selection**, because csgrs walks a BSP per boolean — doing this
    /// per frame is precisely the lag source the whole panel is careful to avoid.
    fn ensure_sel_mesh(&mut self) {
        if self.sel_key == self.selection {
            return;
        }
        let mut m = Model::default();
        for id in &self.selection {
            if let Some(f) = self.model.features.iter().find(|f| f.id == *id) {
                let mut f = *f;
                f.op = BoolOp::Union; // isolated: a lone Difference would erase itself
                m.push_feature(f);
            }
        }
        self.sel_mesh = m.eval();
        self.sel_key = self.selection.clone();
    }

    /// Selection SHADE — the selected solids tinted in place (§0.6's "selected
    /// dobjects get a shade"). Drawn in the translucent overlay pass, which uses
    /// `depth_func(LEQUAL)` so coincident geometry tints instead of z-fighting.
    pub fn shade_verts(&self) -> Vec<V3> {
        if self.selection.is_empty() || self.modify.as_ref().is_some_and(|m| m.has_base()) {
            return Vec::new(); // once the base is picked the GHOST is the feedback
        }
        let c = [0.0, 0.75, 0.95];
        self.sel_mesh.positions.iter().map(|p| v(Vec3::from(*p), ui(c))).collect()
    }

    /// GHOST — the selected solids under the op's LIVE transform, at the constrained
    /// cursor (spec §0.6: "while moving it shows the path").
    fn ghost_verts(&self, c: [f32; 3], xf: impl Fn(Vec3) -> Vec3) -> Vec<V3> {
        self.sel_mesh.positions.iter().map(|p| v(xf(Vec3::from(*p)), ui(c))).collect()
    }

    /// The live ghost for the running op. Colours per §0.6: Move accent(255,200,100) ·
    /// Copy green(150,230,170) · Rotate/Scale white · Mirror violet(200,160,255).
    pub fn modify_ghost(&self, cursor_world: Vec3, card: bool) -> Vec<V3> {
        use cad_solid::modify::{rot_about, scale_about, ModifyOp};
        let Some(md) = &self.modify else { return Vec::new() };
        let plane = Plane::default();
        let Some(base) = md.anchor_world(&plane) else { return Vec::new() };
        match md.op {
            ModifyOp::Move | ModifyOp::Copy => {
                let d = cursor_world - base;
                let d = if card { card_lock_world(d) } else { d };
                let c = if md.op == ModifyOp::Move { [1.0, 0.78, 0.39] } else { [0.59, 0.90, 0.67] };
                self.ghost_verts(c, |p| p + d)
            }
            ModifyOp::Rotate => {
                let a = md.preview_angle(&plane, cursor_world, card).unwrap_or(0.0);
                self.ghost_verts([0.92, 0.92, 0.98], |p| rot_about(p, base, Vec3::Z, a))
            }
            ModifyOp::Scale => {
                let k = md.preview_factor(&plane, cursor_world).unwrap_or(1.0);
                self.ghost_verts([0.80, 0.95, 0.82], |p| scale_about(p, base, k))
            }
            ModifyOp::Mirror => {
                let line = (cursor_world - base).normalize_or_zero();
                let n = Vec3::Z.cross(line).normalize_or_zero();
                if n.length_squared() < 1e-9 { return Vec::new(); }
                self.ghost_verts([0.78, 0.63, 1.0], |p| p - n * (2.0 * (p - base).dot(n)))
            }
        }
    }

    /// Cancel any queued/running 3D op.
    pub fn abort_op(&mut self) {
        self.modify = None;
        self.queued = None;
        self.status.clear();
    }

    /// Flat-shaded triangle soup for the evaluated solid.
    pub fn scene_verts(&self) -> Vec<V3> {
        let default_base = [0.62, 0.68, 0.78];
        let default_n = [0.0f32, 0.0, 1.0];
        let mut out = Vec::with_capacity(self.cached.positions.len());
        // Each triangle is coloured by, in priority order: its SURFACE (a painted face),
        // then its body's feature colour, then the neutral default. `face_ids` has one
        // entry per triangle.
        for (i, tri) in self.cached.positions.chunks_exact(3).enumerate() {
            let fid = self.cached.face_ids.get(i).copied();
            // HIDE CEILINGS: drop triangles that belong to a tracked ceiling. Done here (at
            // render time, keyed on the triangle's feature id) rather than by re-evaluating
            // the model — so the toggle is instant and cannot silently fail.
            if self.hide_ceilings {
                if let Some(id) = fid {
                    // Hide the whole ceiling slab. The surrounding WALL is a separate solid
                    // (an annulus, once the room is carved from its building) whose own top
                    // stays — so removing the ceiling opens the room but leaves the wall
                    // capped. No fragile per-triangle clipping of a disc cap.
                    if self.is_hidden_ceiling(id) {
                        continue;
                    }
                }
            }
            // CUTAWAY: drop any triangle lying ENTIRELY above the cut plane, so ceilings,
            // roofs and upper floors vanish and you can see into the structure. Walls and
            // floors that cross the plane stay whole.
            if self.cutaway && tri.iter().all(|p| p[2] >= self.cutaway_z - 1e-4) {
                continue;
            }
            // TEXTURED triangles (whole-feature OR a single painted surface) are drawn by the
            // image-mapped pass (see `feature_textured_meshes`) — keep them OUT of the flat
            // batch. A per-surface texture wins over a whole-feature one.
            if let Some(id) = fid {
                let skey = surface_key(id, tri[0], tri[1], tri[2]);
                if self.surface_texture.contains_key(&skey) || self.feature_texture.contains_key(&id) {
                    continue;
                }
            }
            let base = fid
                .and_then(|id| {
                    let key = surface_key(id, tri[0], tri[1], tri[2]);
                    self.surface_color.get(&key).copied()
                })
                .or_else(|| fid.and_then(|id| self.feature_color.get(&id).copied()))
                .unwrap_or(default_base);
            for (k, p) in tri.iter().enumerate() {
                let n = self.cached.normals.get(i * 3 + k).copied().unwrap_or(default_n);
                out.push(v(Vec3::from(*p), shade(base, Vec3::from(n))));
            }
        }
        out
    }

    /// World-space TEXTURED triangle soup for every feature that carries a pasted texture,
    /// grouped by texture index: `(texture_index, verts)`. UVs use WORLD box projection at
    /// ~1 tile / metre, so a wall or floor tiles the image at a real, consistent scale (the
    /// classic "wallpaper / floor tile" mapping). Honours hide-ceilings and cutaway exactly
    /// like [`Self::scene_verts`], and the triangles are the ones that method skips.
    pub fn feature_textured_meshes(&self) -> Vec<(usize, Vec<crate::light3d::TexVtx>)> {
        if self.feature_texture.is_empty() && self.surface_texture.is_empty() {
            return Vec::new();
        }
        let mut groups: std::collections::HashMap<usize, Vec<crate::light3d::TexVtx>> =
            std::collections::HashMap::new();
        for (i, tri) in self.cached.positions.chunks_exact(3).enumerate() {
            let Some(id) = self.cached.face_ids.get(i).copied() else { continue };
            // Per-surface texture wins over the whole-feature one.
            let tex_idx = self
                .surface_texture
                .get(&surface_key(id, tri[0], tri[1], tri[2]))
                .copied()
                .or_else(|| self.feature_texture.get(&id).copied());
            let Some(tex_idx) = tex_idx else { continue };
            if tex_idx >= self.textures.len() {
                continue;
            }
            if self.hide_ceilings && self.is_hidden_ceiling(id) {
                continue;
            }
            if self.cutaway && tri.iter().all(|p| p[2] >= self.cutaway_z - 1e-4) {
                continue;
            }
            let tex = &self.textures[tex_idx]; // tiling (tiles/metre) + move + rotate
            let g = groups.entry(tex_idx).or_default();
            for (k, p) in tri.iter().enumerate() {
                let n = Vec3::from(self.cached.normals.get(i * 3 + k).copied().unwrap_or([0.0, 0.0, 1.0]));
                let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
                let (uc, vc) = if ax >= ay && ax >= az {
                    (p[1], p[2]) // X-facing wall → YZ
                } else if ay >= az {
                    (p[0], p[2]) // Y-facing wall → XZ
                } else {
                    (p[0], p[1]) // floor / ceiling → XY
                };
                let s = shade_scalar(n, false); // sun-aware; matches `shade`
                let uv = tex.map_uv(uc, vc);
                g.push(crate::light3d::TexVtx {
                    x: p[0], y: p[1], z: p[2],
                    // Carry the texture's opacity so a see-through feature routes to the blended
                    // pass (opacity 1 = opaque, unchanged). Frag alpha = image.a · this.
                    u: uv[0], v: uv[1], s, a: tex.opacity,
                });
            }
        }
        groups.into_iter().collect()
    }

    /// Placed furniture as shaded triangles, ready to draw alongside the scene. Each
    /// instance's mesh is posed by its `pos` / `scale` / `rot` (3-axis) and tinted by its colour.
    ///
    /// `apx` is the APX render mode: a piece whose mesh exceeds [`APX_FURNITURE_TRIS`] is
    /// drawn as a cheap 12-triangle BOUNDING-BOX proxy instead of its full mesh, so a heavy
    /// scene stays smooth. GPU / CPU modes pass `apx = false` and always draw the full mesh.
    /// `skip` excludes one instance (the one being dragged) so the cached buffer stays stable
    /// during a move/rotate — the dragged piece is drawn live via [`Self::furniture_ghost_verts`]
    /// instead. Without this, dragging re-transformed EVERY furniture every frame (~70 ms for
    /// one 90k-tri couch, ~200 ms for three) — the move/rotate stutter.
    pub fn furniture_verts(&self, apx: bool, skip: Option<usize>) -> Vec<V3> {
        let mut out = Vec::new();
        for (idx, inst) in self.furniture.iter().enumerate() {
            if Some(idx) == skip { continue; }
            let Some(asset) = self.furniture_lib.get(inst.asset) else { continue };
            let s = inst.scale_vec();
            let rm = inst.rot_mat();
            let pos = Vec3::from(inst.pos);
            // APX proxy for a heavy mesh — a box (12 tris) spanning its cached local bounds.
            if apx && asset.positions.len() / 3 > APX_FURNITURE_TRIS {
                Self::push_furniture_box(
                    &mut out, asset.local_min, asset.local_max, s, rm, pos, inst.color,
                );
                continue;
            }
            for (i, p) in asset.positions.iter().enumerate() {
                // scale → 3-axis rotate → translate (normals rotate the same way)
                let lp = Vec3::new(p[0] * s.x, p[1] * s.y, p[2] * s.z);
                let wp = rm * lp + pos;
                let n = asset.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
                let wn = rm * Vec3::from(n);
                out.push(v(wp, shade_furniture(inst.color, wn)));
            }
        }
        out
    }

    /// Furniture instance `i`'s LOCAL mesh (asset positions, unposed), shaded by the instance
    /// colour + the LOCAL normal. Drawn via a GPU model matrix during a drag (see
    /// [`Self::furniture_model_matrix`]) so moving/rotating a heavy piece keeps its FULL form
    /// with no per-frame CPU vertex transform. Built once per drag and reused.
    pub fn furniture_local_mesh(&self, i: usize) -> Vec<V3> {
        let Some(inst) = self.furniture.get(i) else { return Vec::new() };
        let Some(asset) = self.furniture_lib.get(inst.asset) else { return Vec::new() };
        // Heavy pieces render from a decimated proxy (built once) so 8 imports don't crawl.
        if asset.needs_lod() {
            let lod = asset.lod_geom();
            return lod.0.iter().zip(lod.1.iter())
                .map(|(p, n)| v(Vec3::from(*p), shade_furniture(inst.color, Vec3::from(*n))))
                .collect();
        }
        // Opaque common case: emit every triangle (fast path, unchanged).
        if !asset.is_translucent() {
            return asset
                .positions
                .iter()
                .enumerate()
                .map(|(k, p)| {
                    let n = asset.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]);
                    v(Vec3::from(*p), shade_furniture(inst.color, Vec3::from(n)))
                })
                .collect();
        }
        // Mixed asset (glass + frame): the solid pass takes ONLY the opaque triangles; the
        // translucent ones are peeled into [`Self::furniture_translucent_mesh`].
        let mut out = Vec::with_capacity(asset.positions.len());
        for t in 0..asset.positions.len() / 3 {
            let base = t * 3;
            if tri_is_translucent(asset, base) {
                continue;
            }
            for k in base..base + 3 {
                let p = asset.positions[k];
                let n = asset.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]);
                out.push(v(Vec3::from(p), shade_furniture(inst.color, Vec3::from(n))));
            }
        }
        out
    }

    /// The TRANSLUCENT triangles of furniture instance `i` (glass panes etc.) as `V3A` with
    /// per-vertex opacity, plus a stable GPU-buffer key. `None` when the asset is fully opaque —
    /// then nothing is drawn in the blended pass. Coordinates are LOCAL (drawn with the
    /// instance's model matrix, exactly like [`Self::furniture_local_mesh`]).
    pub fn furniture_translucent_mesh(&self, i: usize) -> Option<(u64, Vec<crate::light3d::V3A>)> {
        use std::hash::{Hash, Hasher};
        let inst = self.furniture.get(i)?;
        let asset = self.furniture_lib.get(inst.asset)?;
        if !asset.is_translucent() {
            return None;
        }
        let mut out = Vec::new();
        for t in 0..asset.positions.len() / 3 {
            let base = t * 3;
            if !tri_is_translucent(asset, base) {
                continue;
            }
            for k in base..base + 3 {
                let p = asset.positions[k];
                let n = asset.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]);
                let c = shade_furniture(inst.color, Vec3::from(n));
                out.push(crate::light3d::V3A {
                    x: p[0], y: p[1], z: p[2],
                    r: c.col[0], g: c.col[1], b: c.col[2],
                    a: asset.vertex_alpha(k),
                });
            }
        }
        if out.is_empty() {
            return None;
        }
        // Key = asset + colour, tagged distinct from the opaque `furniture_key` so the two
        // never share a GPU buffer.
        let mut h = std::collections::hash_map::DefaultHasher::new();
        inst.asset.hash(&mut h);
        for x in inst.color { x.to_bits().hash(&mut h); }
        0xA1A1_A1A1u32.hash(&mut h);
        Some((h.finish(), out))
    }

    /// Build the full TEXTURED local mesh for instance `i` when it carries a pasted texture:
    /// `(texture_index, base_key, verts)` in LOCAL coords (drawn with the instance's model
    /// matrix, exactly like [`Self::furniture_local_mesh`]). UVs use BOX PROJECTION — each
    /// vertex is projected onto the plane it most faces and normalised to the asset's local
    /// bounding box, so the image wraps the piece once and reads correctly on axis-aligned
    /// faces (tables, cabinets, panels). `s` bakes the flat-shade lighting factor and `a` the
    /// per-vertex opacity. Callers ([`Self::furniture_textured_mesh`] /
    /// [`Self::furniture_textured_translucent_mesh`]) split this into the opaque + glass passes.
    fn textured_vtx(&self, i: usize) -> Option<(usize, u64, Vec<crate::light3d::TexVtx>)> {
        use std::hash::{Hash, Hasher};
        let inst = self.furniture.get(i)?;
        let tex_idx = inst.texture?;
        if tex_idx >= self.textures.len() {
            return None;
        }
        let asset = self.furniture_lib.get(inst.asset)?;
        let tex = &self.textures[tex_idx]; // tiling / move / rotate live here
        let (mn, mx) = (asset.local_min, asset.local_max);
        let ext = [
            (mx[0] - mn[0]).max(1e-4),
            (mx[1] - mn[1]).max(1e-4),
            (mx[2] - mn[2]).max(1e-4),
        ];
        // Real UVs (from a glTF import) map the texture as the artist intended; otherwise box
        // projection normalised to the local bbox.
        let has_uv = asset.uvs.len() == asset.positions.len();
        // Heavy untextured pieces render from the decimated proxy (needs_lod ⇒ no real UVs).
        let lod = if asset.needs_lod() { Some(asset.lod_geom()) } else { None };
        let (src_pos, src_nrm): (&[[f32; 3]], &[[f32; 3]]) = match &lod {
            Some(a) => (&a.0, &a.1),
            None => (&asset.positions, &asset.normals),
        };
        let mut out = Vec::with_capacity(src_pos.len());
        for (k, p) in src_pos.iter().enumerate() {
            let n = Vec3::from(src_nrm.get(k).copied().unwrap_or([0.0, 0.0, 1.0]));
            let (uc, vc) = if has_uv {
                let t = asset.uvs[k];
                (t[0], t[1])
            } else {
                let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
                if ax >= ay && ax >= az {
                    ((p[1] - mn[1]) / ext[1], (p[2] - mn[2]) / ext[2]) // X-facing → YZ
                } else if ay >= az {
                    ((p[0] - mn[0]) / ext[0], (p[2] - mn[2]) / ext[2]) // Y-facing → XZ
                } else {
                    ((p[0] - mn[0]) / ext[0], (p[1] - mn[1]) / ext[1]) // Z-facing → XY
                }
            };
            let s = shade_scalar(n, true); // sun-aware; matches `shade_furniture`
            let uv = tex.map_uv(uc, vc); // tiling + move + rotate
            out.push(crate::light3d::TexVtx {
                x: p[0], y: p[1], z: p[2], u: uv[0], v: uv[1], s, a: asset.vertex_alpha(k) * tex.opacity,
            });
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        inst.asset.hash(&mut h);
        tex_idx.hash(&mut h);
        // A tiling / offset / rotation change must yield a fresh GPU buffer.
        tex.scale.to_bits().hash(&mut h);
        tex.offset[0].to_bits().hash(&mut h);
        tex.offset[1].to_bits().hash(&mut h);
        tex.rot_deg.to_bits().hash(&mut h);
        tex.opacity.to_bits().hash(&mut h);
        Some((tex_idx, h.finish(), out))
    }

    /// The OPAQUE textured triangles of instance `i` (a textured piece minus any glass), keyed
    /// by asset+texture+tiling. For a fully-opaque textured asset this is the whole mesh (fast
    /// path); a mixed piece keeps only its solid faces here, glass going to
    /// [`Self::furniture_textured_translucent_mesh`]. `None` if it has no texture or no opaque tris.
    pub fn furniture_textured_mesh(&self, i: usize) -> Option<(usize, u64, Vec<crate::light3d::TexVtx>)> {
        let (tex_idx, key, all) = self.textured_vtx(i)?;
        // See-through if the mesh has glass OR the texture's opacity < 1 (baked into vertex `a`).
        if !all.iter().any(|v| v.a < ALPHA_OPAQUE) {
            return Some((tex_idx, key, all)); // every triangle opaque → draw as one mesh
        }
        let mut out = Vec::with_capacity(all.len());
        for t in 0..all.len() / 3 {
            let b = t * 3;
            let tr = all[b].a < ALPHA_OPAQUE || all[b + 1].a < ALPHA_OPAQUE || all[b + 2].a < ALPHA_OPAQUE;
            if !tr {
                out.extend_from_slice(&all[b..b + 3]);
            }
        }
        (!out.is_empty()).then_some((tex_idx, key, out))
    }

    /// The TRANSLUCENT textured triangles of instance `i` (textured glass), with per-vertex
    /// opacity, for the blended textured pass. Keyed distinctly from the opaque mesh so the two
    /// never share a GPU buffer. `None` when the asset is fully opaque or carries no texture.
    pub fn furniture_textured_translucent_mesh(&self, i: usize) -> Option<(usize, u64, Vec<crate::light3d::TexVtx>)> {
        let (tex_idx, key, all) = self.textured_vtx(i)?;
        let mut out = Vec::new();
        for t in 0..all.len() / 3 {
            let b = t * 3;
            let tr = all[b].a < ALPHA_OPAQUE || all[b + 1].a < ALPHA_OPAQUE || all[b + 2].a < ALPHA_OPAQUE;
            if tr {
                out.extend_from_slice(&all[b..b + 3]);
            }
        }
        if out.is_empty() {
            return None;
        }
        Some((tex_idx, key ^ 0x5151_5151_5151_5151, out))
    }

    /// Build the split draws for a PER-SURFACE-textured instance: one textured group per texture
    /// actually used across its faces, plus the flat (untextured) remainder. Returns `None` when
    /// the instance has no per-surface textures (the caller uses the ordinary whole-object path),
    /// or the asset is heavy/translucent (per-surface unsupported there). Each triangle's effective
    /// texture is its face-group's `surface_texture` entry if any, else the whole-object `texture`,
    /// else none (flat). Keys fold in the full assignment so any paint change yields fresh buffers.
    pub fn furniture_faceted(&self, i: usize) -> Option<std::sync::Arc<FacetedFurniture>> {
        use std::hash::{Hash, Hasher};
        let inst = self.furniture.get(i)?;
        if inst.surface_texture.is_empty() {
            return None;
        }
        let asset = self.furniture_lib.get(inst.asset)?;
        // NB: a translucent asset is NOT bailed here — the per-surface split below peels glass
        // face-groups into `translucent` buckets (the blended pass) while its solid materials stay
        // opaque, so a multi-material piece with glass (e.g. the villa) still renders every region.
        if asset.needs_lod() {
            return None;
        }

        // Cheap per-instance signature: everything the split depends on EXCEPT pose (the buckets are
        // in local space). If it matches the memo we skip the O(tri) rebuild entirely — the reason a
        // heavy multi-material glTF stopped pegging the frame at ~600 ms.
        let mut sig = std::collections::hash_map::DefaultHasher::new();
        inst.asset.hash(&mut sig);
        inst.texture.hash(&mut sig);
        for c in inst.color { c.to_bits().hash(&mut sig); }
        let mut sig_entries: Vec<(u32, usize)> =
            inst.surface_texture.iter().map(|(&g, &t)| (g, t)).collect();
        sig_entries.sort_unstable();
        sig_entries.hash(&mut sig);
        // Fold in the transform/opacity of every texture the assignment references (a slider tweak
        // must invalidate the memo). O(#faces mapped), not O(tris).
        for &t in inst.surface_texture.values().chain(inst.texture.iter()) {
            if let Some(tex) = self.textures.get(t) {
                tex.scale.to_bits().hash(&mut sig);
                tex.offset[0].to_bits().hash(&mut sig);
                tex.offset[1].to_bits().hash(&mut sig);
                tex.rot_deg.to_bits().hash(&mut sig);
                tex.opacity.to_bits().hash(&mut sig);
            }
        }
        // The baked `TexVtx.s` scalar is only READ by the shader in studio mode — when daylight is
        // on, the textured shader computes its own lighting from the sun and the sky's SH ambient
        // and ignores it entirely (see `u_sun_on` in light3d's TEX_FS). So only the sun's ON/OFF
        // state can change this buffer's contents; hashing the whole `SunEnv` meant every nudge of
        // the hour slider re-baked every textured vertex in the scene, which on the villa is 1.86 M
        // triangles rebuilt per frame while dragging.
        self.sun.enabled.hash(&mut sig);
        if !self.sun.enabled {
            self.sun.hash_into(&mut sig); // studio mode: the baked scalar is what lights the piece
        }
        self.clay_mode.hash(&mut sig); // clay toggle → re-bake
        let sig = sig.finish();
        if let Some((cached_sig, arc)) = self.faceted_cache.borrow().get(&i) {
            if *cached_sig == sig {
                return Some(arc.clone());
            }
        }

        let groups = asset.group_geom();
        let (mn, mx) = (asset.local_min, asset.local_max);
        let ext = [(mx[0] - mn[0]).max(1e-4), (mx[1] - mn[1]).max(1e-4), (mx[2] - mn[2]).max(1e-4)];
        let has_uv = asset.uvs.len() == asset.positions.len();
        let ntri = asset.positions.len() / 3;

        // Assignment fingerprint: whole-object texture + the sorted per-face map. Folded into every
        // buffer key so a paint (which changes this) rebuilds only that instance's GPU meshes.
        let mut ah = std::collections::hash_map::DefaultHasher::new();
        inst.texture.hash(&mut ah);
        let mut entries: Vec<(u32, usize)> = inst.surface_texture.iter().map(|(&g, &t)| (g, t)).collect();
        entries.sort_unstable();
        entries.hash(&mut ah);
        let assign_hash = ah.finish();

        let uv_of = |k: usize, p: &[f32; 3], n: Vec3| -> (f32, f32) {
            if has_uv {
                let t = asset.uvs[k];
                (t[0], t[1])
            } else {
                let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
                if ax >= ay && ax >= az {
                    ((p[1] - mn[1]) / ext[1], (p[2] - mn[2]) / ext[2])
                } else if ay >= az {
                    ((p[0] - mn[0]) / ext[0], (p[2] - mn[2]) / ext[2])
                } else {
                    ((p[0] - mn[0]) / ext[0], (p[1] - mn[1]) / ext[1])
                }
            }
        };

        use std::collections::BTreeMap;
        let mut buckets: BTreeMap<usize, Vec<crate::light3d::TexVtx>> = BTreeMap::new();
        let mut flat: Vec<V3> = Vec::new();
        for t in 0..ntri {
            let fg = groups.face.get(t).copied().unwrap_or(0);
            let eff = inst
                .surface_texture
                .get(&fg)
                .copied()
                .or(inst.texture)
                .filter(|&ti| ti < self.textures.len());
            let base = t * 3;
            match eff {
                Some(ti) => {
                    let tex = &self.textures[ti];
                    let buf = buckets.entry(ti).or_default();
                    for k in base..base + 3 {
                        let p = asset.positions[k];
                        let n = Vec3::from(asset.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]));
                        let (uc, vc) = uv_of(k, &p, n);
                        let s = shade_scalar(n, true);
                        let uv = tex.map_uv(uc, vc);
                        // Surface transparency multiplies the mesh's own per-vertex opacity.
                        let a = asset.vertex_alpha(k) * tex.opacity;
                        buf.push(crate::light3d::TexVtx { x: p[0], y: p[1], z: p[2], u: uv[0], v: uv[1], s, a });
                    }
                }
                None => {
                    for k in base..base + 3 {
                        let p = asset.positions[k];
                        let n = Vec3::from(asset.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]));
                        flat.push(v(Vec3::from(p), shade_furniture(inst.color, n)));
                    }
                }
            }
        }

        let mut opaque = Vec::new();
        let mut translucent = Vec::new();
        for (ti, verts) in buckets {
            let tex = &self.textures[ti];
            let mut h = std::collections::hash_map::DefaultHasher::new();
            inst.asset.hash(&mut h);
            ti.hash(&mut h);
            tex.scale.to_bits().hash(&mut h);
            tex.offset[0].to_bits().hash(&mut h);
            tex.offset[1].to_bits().hash(&mut h);
            tex.rot_deg.to_bits().hash(&mut h);
            tex.opacity.to_bits().hash(&mut h); // a transparency change → fresh buffer
            assign_hash.hash(&mut h);
            0xFACEu32.hash(&mut h);
            // A see-through surface (its texture opacity < 1, or any glass vertex) goes to the
            // blended pass; everything else stays in the fast opaque pass.
            let see_through = tex.opacity < ALPHA_OPAQUE || verts.iter().any(|v| v.a < ALPHA_OPAQUE);
            if see_through {
                translucent.push((ti, h.finish() ^ 0x5151_5151_5151_5151, verts));
            } else {
                opaque.push((ti, h.finish(), verts));
            }
        }
        let flat = if flat.is_empty() {
            None
        } else {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            inst.asset.hash(&mut h);
            for c in inst.color {
                c.to_bits().hash(&mut h);
            }
            assign_hash.hash(&mut h);
            0xF1A7u32.hash(&mut h);
            Some((h.finish(), flat))
        };
        let arc = std::sync::Arc::new(FacetedFurniture { opaque, translucent, flat });
        self.faceted_cache.borrow_mut().insert(i, (sig, arc.clone()));
        Some(arc)
    }

    /// Stable key for instance `i`'s GPU buffer: its asset + colour. Instances that share both
    /// share one uploaded mesh (the local geometry is identical); a recolour yields a new key.
    pub fn furniture_key(&self, i: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(inst) = self.furniture.get(i) {
            inst.asset.hash(&mut h);
            for x in inst.color { x.to_bits().hash(&mut h); }
        }
        self.sun.hash_into(&mut h); // sun change → re-shade furniture
        self.clay_mode.hash(&mut h); // clay toggle → re-bake
        h.finish()
    }

    /// World matrix (translate · rotate · scale) of furniture instance `i`, column-major for GL.
    /// Applied to the LOCAL mesh it reproduces `furniture_point`: `pos + rot·(scale·p)`.
    pub fn furniture_model_matrix(&self, i: usize) -> Option<[f32; 16]> {
        let inst = self.furniture.get(i)?;
        let m = glam::Mat4::from_scale_rotation_translation(
            inst.scale_vec(),
            glam::Quat::from_mat3(&inst.rot_mat()),
            glam::Vec3::from(inst.pos),
        );
        Some(m.to_cols_array())
    }

    /// The single furniture instance `i`, posed and shaded, for LIVE drawing during a drag
    /// (it is excluded from the cached opaque buffer while dragging). A heavy mesh draws as its
    /// bounding-box proxy so even a 90k-tri piece stays smooth while you move/rotate it.
    #[allow(dead_code)]
    pub fn furniture_ghost_verts(&self, i: usize) -> Vec<V3> {
        let Some(inst) = self.furniture.get(i) else { return Vec::new() };
        let Some(asset) = self.furniture_lib.get(inst.asset) else { return Vec::new() };
        let mut out = Vec::new();
        let s = inst.scale_vec();
        let rm = inst.rot_mat();
        let pos = Vec3::from(inst.pos);
        if asset.positions.len() / 3 > APX_FURNITURE_TRIS {
            Self::push_furniture_box(&mut out, asset.local_min, asset.local_max, s, rm, pos, inst.color);
        } else {
            for (k, p) in asset.positions.iter().enumerate() {
                let lp = Vec3::new(p[0] * s.x, p[1] * s.y, p[2] * s.z);
                let wp = rm * lp + pos;
                let n = asset.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]);
                let wn = rm * Vec3::from(n);
                out.push(v(wp, shade_furniture(inst.color, wn)));
            }
        }
        out
    }

    /// Emit a box (6 faces × 2 tris = 36 verts) spanning the local AABB `[lmn, lmx]`, posed
    /// by `scale`/`rot`/`pos` and tinted like the instance. This is the APX proxy for a heavy
    /// furniture mesh — 12 triangles instead of ~90 000.
    fn push_furniture_box(
        out: &mut Vec<V3>, lmn: [f32; 3], lmx: [f32; 3],
        s: Vec3, rm: glam::Mat3, pos: Vec3, color: [f32; 3],
    ) {
        let corner = |xi: usize, yi: usize, zi: usize| -> Vec3 {
            let lx = if xi == 0 { lmn[0] } else { lmx[0] };
            let ly = if yi == 0 { lmn[1] } else { lmx[1] };
            let lz = if zi == 0 { lmn[2] } else { lmx[2] };
            rm * (Vec3::new(lx, ly, lz) * s) + pos
        };
        // Six faces, each as a quad of corner picks (x,y,z ∈ {min=0, max=1}) + a local normal.
        let faces: [([(usize, usize, usize); 4], Vec3); 6] = [
            ([(0, 0, 0), (0, 1, 0), (0, 1, 1), (0, 0, 1)], Vec3::NEG_X),
            ([(1, 0, 0), (1, 0, 1), (1, 1, 1), (1, 1, 0)], Vec3::X),
            ([(0, 0, 0), (0, 0, 1), (1, 0, 1), (1, 0, 0)], Vec3::NEG_Y),
            ([(0, 1, 0), (1, 1, 0), (1, 1, 1), (0, 1, 1)], Vec3::Y),
            ([(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)], Vec3::NEG_Z),
            ([(0, 0, 1), (0, 1, 1), (1, 1, 1), (1, 0, 1)], Vec3::Z),
        ];
        for (quad, ln) in faces {
            let c = shade_furniture(color, rm * ln);
            let p = [
                corner(quad[0].0, quad[0].1, quad[0].2),
                corner(quad[1].0, quad[1].1, quad[1].2),
                corner(quad[2].0, quad[2].1, quad[2].2),
                corner(quad[3].0, quad[3].1, quad[3].2),
            ];
            for &(a, b, d) in &[(0usize, 1usize, 2usize), (0, 2, 3)] {
                out.push(v(p[a], c));
                out.push(v(p[b], c));
                out.push(v(p[d], c));
            }
        }
    }

    /// Triangle count of the single heaviest PLACED furniture mesh (0 if none). The usual
    /// culprit when the 3D view goes slow after an import — surfaced in the perf monitor.
    pub fn heaviest_furniture_tris(&self) -> usize {
        self.furniture
            .iter()
            .filter_map(|inst| self.furniture_lib.get(inst.asset))
            .map(|a| a.positions.len() / 3)
            .max()
            .unwrap_or(0)
    }

    /// Signature of the opaque CSG buffer's inputs. Furniture is NO LONGER part of this buffer
    /// (each furniture is a GPU-instanced draw with its own model matrix — see the app's
    /// furniture draw list), so the opaque buffer holds only the building solids and rebuilds
    /// only on a real geometry/colour/toggle change — never on a furniture import or move.
    fn opaque_sig(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.geom_version.hash(&mut h);
        self.hide_ceilings.hash(&mut h);
        self.cutaway.hash(&mut h);
        self.cutaway_z.to_bits().hash(&mut h);
        // The sun is DELIBERATELY not hashed. The buffer now carries albedo and normals, and the
        // fragment shader applies the light — so moving the sun changes no vertex. Re-baking here
        // was the hour slider rebuilding 1.86 M vertices on every nudge, and the whole point of
        // moving the lighting into the shader is that this cannot happen any more. Only the
        // sun's ENABLED state matters, because it selects the studio response.
        self.sun.enabled.hash(&mut h);
        self.clay_mode.hash(&mut h); // clay toggle → re-bake grey/coloured
        // Colour maps: combine per-entry hashes order-independently (XOR/add) so a recolour
        // of an existing key changes the signature even though the map length is unchanged.
        let mut fc: u64 = 0;
        for (k, c) in &self.feature_color {
            let mut e = std::collections::hash_map::DefaultHasher::new();
            k.hash(&mut e);
            for x in c { x.to_bits().hash(&mut e); }
            fc = fc.wrapping_add(e.finish());
        }
        fc.hash(&mut h);
        let mut sc: u64 = 0;
        for (k, c) in &self.surface_color {
            let mut e = std::collections::hash_map::DefaultHasher::new();
            k.hash(&mut e);
            for x in c { x.to_bits().hash(&mut e); }
            sc = sc.wrapping_add(e.finish());
        }
        sc.hash(&mut h);
        // Feature→texture map: applying/removing a texture changes which triangles the flat
        // batch emits (textured ones are skipped), so it must move the signature.
        let mut ft: u64 = 0;
        for (k, &t) in &self.feature_texture {
            let mut e = std::collections::hash_map::DefaultHasher::new();
            k.hash(&mut e);
            t.hash(&mut e);
            ft = ft.wrapping_add(e.finish());
        }
        ft.hash(&mut h);
        // Per-surface texture map: same reason — a painted face leaves the flat batch.
        let mut st: u64 = 0;
        for (k, &t) in &self.surface_texture {
            let mut e = std::collections::hash_map::DefaultHasher::new();
            k.hash(&mut e);
            t.hash(&mut e);
            st = st.wrapping_add(e.finish());
        }
        st.hash(&mut h);
        h.finish()
    }

    /// The opaque CSG scene (building solids only) as a shared, cached buffer, rebuilt only
    /// when [`Self::opaque_sig`] changes. Furniture is drawn separately as GPU instances, so
    /// this buffer stays tiny and a furniture import/move never touches it.
    pub fn opaque_verts(&self) -> std::sync::Arc<Vec<V3>> {
        let sig = self.opaque_sig();
        {
            let c = self.render_cache.borrow();
            if c.ready && c.sig == sig {
                return c.verts.clone();
            }
        }
        let scene = self.scene_verts();
        let arc = std::sync::Arc::new(scene);
        let mut c = self.render_cache.borrow_mut();
        c.verts = arc.clone();
        c.sig = sig;
        c.ready = true;
        arc
    }

    /// Grid on the construction plane + a cyan AABB around each selected feature.
    pub fn overlay_lines(&self) -> Vec<V3> {
        let mut out = Vec::new();
        let g = [0.22, 0.25, 0.30];
        let n = 10i32;
        let s = 1.0f32;
        for i in -n..=n {
            let t = i as f32 * s;
            let e = n as f32 * s;
            seg(&mut out, Vec3::new(t, -e, 0.0), Vec3::new(t, e, 0.0), g);
            seg(&mut out, Vec3::new(-e, t, 0.0), Vec3::new(e, t, 0.0), g);
        }
        for id in &self.selection {
            if let Some(f) = self.model.features.iter().find(|f| f.id == *id) {
                let (mn, mx) = f.world_aabb();
                aabb_lines(&mut out, mn, mx, [0.0, 0.9, 1.0]);
            }
        }
        // Highlight every selected furniture instance the same way.
        for &i in &self.sel_furniture {
            if let Some((mn, mx)) = self.furniture_aabb(i) {
                aabb_lines(&mut out, mn, mx, [1.0, 0.75, 0.2]);
            }
        }
        out
    }

    /// Screen cursor → world ray (origin, unit dir), by inverting the MVP.
    fn ray(cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> (Vec3, Vec3) {
        let ndc_x = 2.0 * (cursor.x - rect.left()) / rect.width().max(1.0) - 1.0;
        let ndc_y = 1.0 - 2.0 * (cursor.y - rect.top()) / rect.height().max(1.0);
        let inv = Mat4::from_cols_array(mvp).inverse();
        let near = inv.project_point3(Vec3::new(ndc_x, ndc_y, -1.0));
        let far = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        (near, (far - near).normalize_or_zero())
    }

    /// Ray-pick the front-most FEATURE (solid) under `cursor`, by world AABB.
    /// This is what the LEFT button does in the 3D view — selection, never camera.
    pub fn pick_feature(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<u32> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        // Ray-test the actual TRIANGLES of each visible body, not its bounding box — a big
        // building's AABB encloses a ceiling sitting on it, so an AABB pick could never
        // reach the ceiling. Skip Difference/Intersection features: those are cutters
        // (a room is a void), not clickable surfaces.
        //
        // Tiebreak on near-equal depth by the SMALLER body, so the specific object on top
        // (a ceiling slab) wins over the large solid it overlaps (the building).
        let mut best: Option<(f32, f32, u32)> = None; // (t, aabb volume, id)
        for f in &self.model.features {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            // A HIDDEN ceiling is not drawn, so it must not be clickable either — otherwise
            // you would select the invisible ceiling instead of what is behind it.
            if self.hide_ceilings && self.is_hidden_ceiling(f.id) {
                continue;
            }
            let tris = self.model.feature_world_positions(f);
            let mut ft: Option<f32> = None;
            for c in tris.chunks_exact(3) {
                let (a, b, cc) = (Vec3::from(c[0]), Vec3::from(c[1]), Vec3::from(c[2]));
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, cc) {
                    if ft.map_or(true, |x| t < x) {
                        ft = Some(t);
                    }
                }
            }
            if let Some(t) = ft {
                let (mn, mx) = f.world_aabb();
                let s = mx - mn;
                let vol = s.x.abs() * s.y.abs() * s.z.abs();
                let better = match best {
                    None => true,
                    Some((bt, bv, _)) => t < bt - 1e-3 || (t < bt + 1e-3 && vol < bv),
                };
                if better {
                    best = Some((t, vol, f.id));
                }
            }
        }
        best.map(|(_, _, id)| id)
    }

    /// Ray-pick the front-most solid FACE under `cursor` and return a sketch [`Frame`]
    /// sitting on it — the basis for sketch-on-face. `None` if the ray misses.
    pub fn pick_face(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<Frame> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        let mut consider = |t: f32, a: Vec3, b: Vec3, c: Vec3| {
            if best.map_or(true, |(bt, _, _)| t < bt) {
                let n = (b - a).cross(c - a).normalize_or_zero();
                best = Some((t, orig + dir * t, n));
            }
        };
        for tri in self.cached.positions.chunks_exact(3) {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                consider(t, a, b, c);
            }
        }
        // FURNITURE FACES TOO. Faces used to come from the evaluated CSG mesh alone, so a face on a
        // door, a cupboard, a staircase or an aperture could not be picked at all — and since
        // sketch-on-face is how a cut is aimed, none of those could be cut either. They are all
        // furniture instances, so testing them here opens the feature to all three at once.
        for (i, inst) in self.furniture.iter().enumerate() {
            let Some(asset) = self.furniture_lib.get(inst.asset) else { continue };
            match self.furniture_aabb(i) {
                Some((mn, mx)) if cad_solid::ray_aabb(orig, dir, mn, mx).is_some() => {}
                _ => continue,
            }
            // The FULL mesh, never the decimated LOD: a cut must be aimed at the surface that will
            // actually be cut, and a decimated proxy is a different surface.
            for tri in asset.positions.chunks_exact(3) {
                let a = self.furniture_point(inst, tri[0]);
                let b = self.furniture_point(inst, tri[1]);
                let c = self.furniture_point(inst, tri[2]);
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                    consider(t, a, b, c);
                }
            }
        }
        best.map(|(_, p, n)| Frame::from_point_normal(p, n))
    }

    /// Which furniture instance a sketch frame is sitting ON, if any.
    ///
    /// Resolved from the frame's own origin rather than remembered from the pick. A sketch can be
    /// started, left, re-entered and finished much later; a stashed instance index would go stale
    /// the moment anything else was deleted, and aiming a cut at the wrong object is worse than
    /// finding none. The origin is a point the pick placed exactly on a surface, so asking which
    /// surface it lies on gives the same answer however much later it is asked.
    pub fn furniture_at_face(&self, frame: &Frame) -> Option<usize> {
        let (o, n) = (frame.origin, frame.normal());
        let mut best: Option<(f32, usize)> = None;
        for (i, inst) in self.furniture.iter().enumerate() {
            let Some(asset) = self.furniture_lib.get(inst.asset) else { continue };
            match self.furniture_aabb(i) {
                // 5 mm of slack: the origin came OFF this surface, it is not a guess.
                Some((mn, mx))
                    if o.cmpge(mn - Vec3::splat(0.005)).all()
                        && o.cmple(mx + Vec3::splat(0.005)).all() => {}
                _ => continue,
            }
            for tri in asset.positions.chunks_exact(3) {
                let a = self.furniture_point(inst, tri[0]);
                let b = self.furniture_point(inst, tri[1]);
                let c = self.furniture_point(inst, tri[2]);
                let tn = (b - a).cross(c - a).normalize_or_zero();
                // Same plane AND the same way up: the two skins of a panel are a millimetre apart,
                // and only the one the user actually picked should claim the sketch.
                if tn.dot(n) < 0.99 {
                    continue;
                }
                let d = (o - a).dot(tn).abs();
                if d < 1e-3 && best.map_or(true, |(bd, _)| d < bd) {
                    best = Some((d, i));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Pull a WORLD-space frame back into instance `fi`'s own local space, where a cut is stored.
    ///
    /// A cut belongs to the object, not to the room: store the frame in world space and the hole
    /// stays behind the moment the piece is dragged or spun. The instance matrix can carry a
    /// non-uniform `fit` (apertures stretch to fill their opening), so the frame is rebuilt from
    /// the transformed normal rather than assuming the axes came through square.
    pub fn frame_to_local(&self, fi: usize, f: &Frame) -> Option<Frame> {
        let inv = Mat4::from_cols_array(&self.furniture_model_matrix(fi)?).inverse();
        let o = inv.transform_point3(f.origin);
        let n = inv.transform_point3(f.origin + f.normal()) - o;
        Some(Frame::from_point_normal(o, n.normalize_or_zero()))
    }

    /// Record a cut on instance `fi` and rebuild its geometry. The profile arrives in the WORLD
    /// frame the user drew on; both are converted to the asset's local space here, once.
    pub fn add_furniture_cut(
        &mut self,
        fi: usize,
        world: &Frame,
        loops: &[Vec<Vec2>],
        through: bool,
        depth: f32,
    ) -> Result<usize, cad_solid::meshcut::CutError> {
        let Some(local) = self.frame_to_local(fi, world) else { return Ok(0) };
        // Scale: the instance may be scaled, so a metre drawn on screen is not a metre in the
        // asset's own units. Measure it off the transform rather than assuming 1.
        let k = {
            let m = Mat4::from_cols_array(&self.furniture_model_matrix(fi).unwrap_or_default());
            let s = m.transform_vector3(local.u).length();
            if s > 1e-6 { 1.0 / s } else { 1.0 }
        };
        let before = self.furniture[fi].cuts.len();
        for pts in loops {
            let profile: Vec<[f32; 2]> = pts
                .iter()
                .map(|p| {
                    let w = world.from_uv(*p);
                    let uv = local.to_uv(
                        Mat4::from_cols_array(&self.furniture_model_matrix(fi).unwrap_or_default())
                            .inverse()
                            .transform_point3(w),
                    );
                    [uv.x, uv.y]
                })
                .collect();
            let label = format!("Cut {}", self.furniture[fi].cuts.len() + 1);
            self.furniture[fi].cuts.push(if through {
                cad_solid::meshcut::MeshCut::through(local, profile, label)
            } else {
                cad_solid::meshcut::MeshCut::pocket(local, profile, depth * k, label)
            });
        }
        // Rebuild, and put the list back exactly as it was if the mesh refuses — a failed cut must
        // leave no trace in the list, or the piece is stuck refusing forever.
        match self.rebuild_cut_asset(fi) {
            Ok(()) => Ok(self.furniture[fi].cuts.len() - before),
            Err(e) => {
                self.furniture[fi].cuts.truncate(before);
                Err(e)
            }
        }
    }

    /// Unproject `cursor` onto the active construction plane (XY at z=0) — the 3D
    /// analog of the 2D canvas's screen→world. `None` if the ray is parallel to it.
    pub fn cursor_on_plane(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<Vec3> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let n = Vec3::Z;
        let denom = dir.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = -orig.dot(n) / denom;
        (t >= 0.0).then(|| orig + dir * t)
    }

    /// OSNAP for 3D picks — the nearest solid mesh VERTEX whose screen projection is
    /// within the aperture. Mirrors the 2D pickbox: snapping to a real corner is what
    /// makes "move this corner to that corner" exact instead of eyeballed.
    pub fn snap_vertex(
        &self,
        cursor: egui::Pos2,
        rect: egui::Rect,
        mvp: &[f32; 16],
    ) -> Option<(Vec3, egui::Pos2)> {
        let m = Mat4::from_cols_array(mvp);
        let aperture = 12.0f32;
        let mut best: Option<(f32, Vec3, egui::Pos2)> = None;
        for p in &self.cached.positions {
            let w = Vec3::from(*p);
            let ndc = m.project_point3(w);
            if !(-1.0..=1.0).contains(&ndc.z) {
                continue;
            }
            let sx = rect.left() + (ndc.x * 0.5 + 0.5) * rect.width();
            let sy = rect.top() + (0.5 - ndc.y * 0.5) * rect.height();
            let sp = egui::pos2(sx, sy);
            let d = sp.distance(cursor);
            if d <= aperture && best.map_or(true, |(bd, _, _)| d < bd) {
                best = Some((d, w, sp));
            }
        }
        best.map(|(_, w, sp)| (w, sp))
    }

    /// The ground (XY) plane at the origin — the fallback sketch surface when the
    /// right-click misses a solid, so you can always start drawing.
    pub fn ground_frame() -> Frame {
        Frame::from_point_normal(Vec3::ZERO, Vec3::Z)
    }

    /// The solid's FEATURE EDGES projected onto `frame`'s (u,v) plane — a clean line
    /// drawing of the 3D object for use as a reference underlay when sketching on a face.
    ///
    /// An edge is a "feature" edge if it is shared by ONLY ONE triangle (a true boundary) or
    /// by two triangles whose normals differ by more than ~20° (a real crease). Interior
    /// tessellation edges — the diagonals that split a flat quad — are dropped, so what you
    /// get is the object's outline and its hard edges, not a triangle-soup mess.
    pub fn frame_reference_edges(&self, frame: &Frame) -> Vec<[Vec2; 2]> {
        use std::collections::HashMap;
        // Quantise a world position so the two triangles sharing an edge hash together.
        let q = |p: [f32; 3]| -> (i64, i64, i64) {
            const S: f32 = 1.0e4;
            ((p[0] * S).round() as i64, (p[1] * S).round() as i64, (p[2] * S).round() as i64)
        };
        // undirected edge key → (endpoint a, endpoint b, adjacent triangle normals)
        let mut map: HashMap<((i64, i64, i64), (i64, i64, i64)), ([f32; 3], [f32; 3], Vec<Vec3>)> =
            HashMap::new();
        for tri in self.cached.positions.chunks_exact(3) {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            let n = (b - a).cross(c - a).normalize_or_zero();
            for (p0, p1) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let (k0, k1) = (q(p0), q(p1));
                let key = if k0 <= k1 { (k0, k1) } else { (k1, k0) };
                map.entry(key).or_insert((p0, p1, Vec::new())).2.push(n);
            }
        }
        let cos_thresh = 20.0_f32.to_radians().cos();
        let mut out = Vec::new();
        for (_, (a, b, normals)) in map {
            let is_feature = normals.len() == 1
                || normals.iter().enumerate().any(|(i, na)| {
                    normals[i + 1..].iter().any(|nb| na.dot(*nb) < cos_thresh)
                });
            if is_feature {
                out.push([frame.to_uv(Vec3::from(a)), frame.to_uv(Vec3::from(b))]);
            }
        }
        out
    }

    /// Every sketch's geometry, lifted from its frame's `(u,v)` back into world space,
    /// as GL_LINES. This is what makes 2D work drawn on a plane visible in 3D.
    pub fn sketch_lines(&self) -> Vec<V3> {
        let mut out = Vec::new();
        for (i, sk) in self.model.sketches.iter().enumerate() {
            // the sketch being edited right now is drawn hot, the others cool
            let active = self.session.as_ref().is_some_and(|s| s.idx == i);
            let c = if active { [1.0, 0.62, 0.12] } else { [0.55, 0.62, 0.72] };
            for d in &sk.doc.dobjects {
                for poly in cad_solid::geom_outlines(&d.geom) {
                    for w in poly.windows(2) {
                        seg(
                            &mut out,
                            sk.frame.from_uv(Vec2::new(w[0].x, w[0].y)),
                            sk.frame.from_uv(Vec2::new(w[1].x, w[1].y)),
                            c,
                        );
                    }
                }
            }
            // frame axes, so an empty sketch plane is still visible
            if active {
                let o = sk.frame.origin;
                seg(&mut out, o, o + sk.frame.u * 1.5, [1.0, 0.3, 0.3]);
                seg(&mut out, o, o + sk.frame.v * 1.5, [0.3, 1.0, 0.3]);
            }
        }
        out
    }

    /// The ACTIVE sketch's LIVE geometry — which lives in the app's swapped-in document,
    /// passed in here — lifted onto its frame, so what you draw on a face appears in the 3D
    /// view immediately (2D↔3D linked). `sketch_lines` can't show it because the active
    /// sketch's own `doc` is empty while it is being edited.
    pub fn live_sketch_lines(&self, doc: &cad_kernel::Document) -> Vec<V3> {
        let mut out = Vec::new();
        let Some(session) = self.session.as_ref() else { return out };
        let Some(sk) = self.model.sketches.get(session.idx) else { return out };
        let c = [1.0, 0.62, 0.12]; // hot — the sketch you are drawing right now
        for d in &doc.dobjects {
            for poly in cad_solid::geom_outlines(&d.geom) {
                for w in poly.windows(2) {
                    seg(
                        &mut out,
                        sk.frame.from_uv(Vec2::new(w[0].x, w[0].y)),
                        sk.frame.from_uv(Vec2::new(w[1].x, w[1].y)),
                        c,
                    );
                }
            }
        }
        out
    }

    pub fn tri_count(&self) -> usize {
        self.cached.tri_count()
    }

    pub fn feature_count(&self) -> usize {
        self.model.features.len()
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    fn view(st: &FactoryState, rect: egui::Rect) -> [f32; 16] {
        let aspect = rect.width() / rect.height();
        crate::light3d::mvp(st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target, aspect, st.ortho)
    }

    /// The user reports "3D dobject not selecting". Picking is pure math (screen →
    /// ray → AABB), so it CAN be tested headlessly even though the click itself
    /// needs a live egui pointer. If this passes, selection math is sound and the
    /// fault is in reachability/routing, not geometry.
    #[test]
    fn clicking_the_centre_of_the_view_picks_the_solid_there() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        st.fit(); // aim the camera at the solid, as ⌖ Frame does
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = view(&st, rect);
        let hit = st.pick_feature(rect.center(), rect, &mvp);
        assert!(hit.is_some(), "a ray through the centre must hit the centred solid");
        assert_eq!(hit.unwrap(), st.model.features[0].id);
    }

    /// The face-sketch reference must be a clean OUTLINE of the object (its real edges),
    /// not a triangle-soup: a box projects to its 12 edges, with the per-face tessellation
    /// diagonals dropped.
    #[test]
    fn frame_reference_is_the_box_outline_not_triangle_soup() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        let edges = st.frame_reference_edges(&FactoryState::ground_frame());
        assert_eq!(edges.len(), 12, "a box projects to its 12 feature edges, got {}", edges.len());
    }

    /// …and a ray into empty space must MISS (else everything is always selected).
    #[test]
    fn clicking_far_from_the_solid_misses() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        st.fit();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = view(&st, rect);
        let corner = egui::pos2(rect.left() + 2.0, rect.top() + 2.0);
        assert!(st.pick_feature(corner, rect, &mvp).is_none(), "corner ray must miss");
    }

    /// Face-pick (the right-click → "Draw on this face" path) must land ON the solid.
    #[test]
    fn face_pick_returns_a_frame_on_the_solid() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        st.fit();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = view(&st, rect);
        let f = st.pick_face(rect.center(), rect, &mvp);
        assert!(f.is_some(), "centre ray must hit a face of the centred solid");
    }
}

#[cfg(test)]
mod outline_tests {
    use super::*;

    fn ell() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0), Vec2::new(6.0, 0.0), Vec2::new(6.0, 3.0),
            Vec2::new(3.0, 3.0), Vec2::new(3.0, 6.0), Vec2::new(0.0, 6.0),
        ]
    }

    /// The Building-outline tool now has a primitive behind it — the greyed row's reason
    /// for being disabled is gone.
    #[test]
    fn an_l_shaped_building_is_exact_not_a_bounding_box() {
        let mut st = FactoryState::default();
        st.add_building_outline(&ell(), 4.0).expect("an L is a valid outline");
        st.recompute();
        let (mn, mx) = st.cached.bounds().expect("the building must have geometry");
        assert!((mx[2] - mn[2] - 4.0).abs() < 1e-3, "it rises to the given height");
        // A bounding-box approximation would fill the whole 6×6 square. The L's area is
        // 27 of that 36, so an exact extrusion has strictly less volume.
        let tris = st.cached.tri_count();
        assert!(tris > 12, "an L has more faces than a box ({tris} triangles)");
    }

    /// A building is built on the ACTIVE storey, like every other new solid.
    #[test]
    fn a_building_rises_from_the_active_storey() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let base = st.active_base_z();
        let id = st.add_building_outline(&ell(), 3.0).unwrap();
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert!((f.placement.lift - base).abs() < 1e-4);
    }

    /// A bad outline is refused WITH ITS REASON and stores nothing — the app turns each
    /// variant into a message, so a silent failure would leave the user guessing.
    #[test]
    fn a_crossed_outline_is_refused_with_its_reason() {
        let mut st = FactoryState::default();
        let bowtie = vec![
            Vec2::new(0.0, 0.0), Vec2::new(4.0, 4.0),
            Vec2::new(4.0, 0.0), Vec2::new(0.0, 4.0),
        ];
        assert_eq!(
            st.add_building_outline(&bowtie, 3.0),
            Err(cad_solid::ProfileError::SelfIntersecting)
        );
        assert!(st.model.features.is_empty(), "nothing may be built from a bad outline");
        assert!(st.model.profiles.is_empty(), "and no profile may be left behind");
    }

    /// A building survives save/reopen — the profile table rides in the same `Model` the
    /// sidecar already stores.
    #[test]
    fn a_building_outline_round_trips_through_the_sidecar() {
        let mut st = FactoryState::default();
        st.add_building_outline(&ell(), 4.0).unwrap();
        let json = serde_json::to_string(&st.to_persist()).unwrap();
        let back: crate::simlux_io::FactoryDoc = serde_json::from_str(&json).unwrap();

        let mut re = FactoryState::default();
        re.apply_persist(back);
        assert_eq!(re.model.profiles.len(), 1, "the outline itself must survive");
        re.recompute();
        assert!(re.cached.tri_count() > 0, "and still build geometry after reload");
    }
}

#[cfg(test)]
mod pan_tests {
    use super::*;

    /// Pan moves ONLY the target — orientation and zoom must be untouched.
    #[test]
    fn pan_moves_the_target_not_the_orientation_or_zoom() {
        let mut st = FactoryState::default();
        let (yaw, pitch, dist) = (st.cam_yaw, st.cam_pitch, st.cam_dist);
        let t0 = st.cam_target;
        st.pan(40.0, -25.0);
        assert_ne!(st.cam_target, t0, "the target must move");
        assert_eq!(st.cam_yaw, yaw, "pan must not orbit");
        assert_eq!(st.cam_pitch, pitch);
        assert_eq!(st.cam_dist, dist, "pan must not zoom");
    }

    /// A zero drag is a no-op.
    #[test]
    fn a_zero_pan_changes_nothing() {
        let mut st = FactoryState::default();
        let t0 = st.cam_target;
        st.pan(0.0, 0.0);
        assert_eq!(st.cam_target, t0);
    }

    /// Panning right then left by the same amount returns to where it started.
    #[test]
    fn opposite_pans_cancel() {
        let mut st = FactoryState::default();
        let t0 = st.cam_target;
        st.pan(30.0, 15.0);
        st.pan(-30.0, -15.0);
        let d = glam::Vec3::from(st.cam_target) - glam::Vec3::from(t0);
        assert!(d.length() < 1e-4, "a pan and its inverse must cancel");
    }
}

#[cfg(test)]
mod furniture_and_color_tests {
    use super::*;

    fn tetra() -> crate::mesh_io::ObjMesh {
        crate::mesh_io::parse_obj(
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\nf 1 2 4\nf 1 3 4\nf 2 3 4\n",
        )
    }

    /// An imported asset enters the library; placing it adds an instance that renders.
    #[test]
    fn import_and_place_furniture_renders() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("chair".into(), tetra());
        assert_eq!(st.furniture_lib.len(), 1);
        st.place_furniture(idx, Vec3::new(2.0, 3.0, 0.0));
        assert_eq!(st.furniture.len(), 1);
        assert!(!st.furniture_verts(false, None).is_empty(), "placed furniture must produce geometry");
        assert!((st.furniture[0].pos[0] - 2.0).abs() < 1e-4, "placed at the given point");
    }

    /// The opaque CSG buffer is CACHED: repeated calls with no change hand back the SAME Arc.
    /// Furniture is NO LONGER part of it (furniture is GPU-instanced), so placing or moving
    /// furniture must NOT rebuild it — only a real geometry change (recompute) does. This is
    /// what keeps a furniture import/move cheap however heavy the mesh.
    #[test]
    fn opaque_render_buffer_is_cached_until_the_scene_changes() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();

        let a = st.opaque_verts();
        let b = st.opaque_verts();
        assert!(std::sync::Arc::ptr_eq(&a, &b), "unchanged scene reuses the cached buffer");

        // Placing / moving furniture must NOT touch the opaque buffer (it's GPU-instanced).
        let idx = st.add_furniture_asset("chair".into(), tetra());
        st.place_furniture(idx, Vec3::new(2.0, 3.0, 0.0));
        let c = st.opaque_verts();
        assert!(std::sync::Arc::ptr_eq(&a, &c), "furniture is not in the opaque buffer");
        st.furniture[0].pos[0] += 5.0;
        let d = st.opaque_verts();
        assert!(std::sync::Arc::ptr_eq(&a, &d), "moving furniture does not rebuild the opaque buffer");

        // A real geometry change (recompute) DOES invalidate it.
        st.add_box();
        st.recompute();
        let e = st.opaque_verts();
        assert!(!std::sync::Arc::ptr_eq(&a, &e), "a geometry change invalidates the cache");
    }

    /// APX render mode replaces a HEAVY furniture mesh with a 12-triangle box proxy, while
    /// GPU/CPU (apx=false) draw it in full and LIGHT furniture is unaffected by the mode.
    #[test]
    fn apx_mode_proxies_heavy_furniture_only() {
        let mut st = FactoryState::default();
        // Heavy mesh: 6000 tris (> APX_FURNITURE_TRIS = 5000).
        let heavy = st.add_furniture_asset(
            "couch".into(),
            crate::mesh_io::ObjMesh {
                positions: (0..18_000).map(|i| [(i % 7) as f32, (i % 5) as f32, (i % 3) as f32]).collect(),
                normals: vec![[0.0, 0.0, 1.0]; 18_000],
                color: None,
                alpha: Vec::new(),
            },
        );
        st.place_furniture(heavy, Vec3::ZERO);

        let full = st.furniture_verts(false, None).len();
        let proxy = st.furniture_verts(true, None).len();
        assert_eq!(full, 18_000, "GPU/CPU draws the full mesh");
        assert_eq!(proxy, 36, "APX draws a 12-triangle box (36 verts) for the heavy piece");
        assert!(proxy < full, "APX is lighter");

        // A LIGHT piece (4 tris) is drawn in full even in APX — no proxy.
        let mut st2 = FactoryState::default();
        let light = st2.add_furniture_asset("stool".into(), tetra());
        st2.place_furniture(light, Vec3::ZERO);
        assert_eq!(
            st2.furniture_verts(false, None).len(),
            st2.furniture_verts(true, None).len(),
            "light furniture is identical in both modes",
        );
    }

    /// The perf monitor's inputs are correct: the heaviest placed mesh is reported, and the
    /// FactoryPerf line surfaces the load + a SLOW flag when the build blows a frame budget.
    #[test]
    fn perf_monitor_reports_the_heaviest_mesh_and_flags_slow() {
        use crate::dbg_recorder::{format_event_oneline, DbgEvent};
        let mut st = FactoryState::default();
        assert_eq!(st.heaviest_furniture_tris(), 0, "no furniture → 0");
        let light = st.add_furniture_asset("stool".into(), tetra()); // 4 tris
        st.place_furniture(light, Vec3::ZERO);
        // A synthetic heavy mesh (300 tris) placed alongside the light one.
        let heavy = st.add_furniture_asset(
            "couch".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![[0.0, 0.0, 0.0]; 900],
                normals: vec![[0.0, 0.0, 1.0]; 900],
                color: None,
                alpha: Vec::new(),
            },
        );
        st.place_furniture(heavy, Vec3::ZERO);
        assert_eq!(st.heaviest_furniture_tris(), 300, "reports the heaviest, not the total");

        let line = format_event_oneline(&DbgEvent::FactoryPerf {
            phase: "buffer-rebuilt".into(),
            frame_us: 0,
            build_us: 20_000, // 20 ms — over one refresh
            scene_tris: 94_247,
            furniture_insts: 2,
            heaviest_tris: 94_247,
            upload_bytes: 7_000_000,
            cache_rebuilt: true,
        });
        assert!(line.contains("FACTORY PERF"), "labelled: {line}");
        assert!(line.contains("tris=94247") && line.contains("heaviest=94247"), "load shown: {line}");
        assert!(line.contains("⚠ SLOW"), "a 20 ms build is flagged slow: {line}");
    }

    /// `furniture_aabb` transforms the CACHED 8 local-box corners (O(8)) instead of sweeping
    /// every vertex each frame — the fix for the fps crater when a heavy piece is SELECTED
    /// (the gizmo + highlight both ask for the AABB per frame). It must still ENCLOSE every
    /// transformed vertex, and be EXACT under scale+translation with no rotation.
    #[test]
    fn furniture_aabb_encloses_all_verts_cheaply() {
        let mut st = FactoryState::default();
        // A skewed tetra so a rotation actually changes the bounds.
        let mesh = crate::mesh_io::ObjMesh {
            positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            color: None,
            alpha: Vec::new(),
        };
        let idx = st.add_furniture_asset("wedge".into(), mesh);
        st.place_furniture(idx, Vec3::new(5.0, -2.0, 0.0));
        st.furniture[0].scale = 1.5;
        st.furniture[0].rot = [15.0, 40.0, 25.0]; // full 3-axis rotation

        let (mn, mx) = st.furniture_aabb(0).expect("has bounds");
        // Brute-force truth: min/max over every transformed vertex.
        let asset = &st.furniture_lib[idx];
        let mut bmn = Vec3::splat(f32::INFINITY);
        let mut bmx = Vec3::splat(f32::NEG_INFINITY);
        for p in &asset.positions {
            let w = st.furniture_point(&st.furniture[0], *p);
            bmn = bmn.min(w);
            bmx = bmx.max(w);
        }
        // The 8-corner box must CONTAIN the true vertex bounds (a valid enclosing AABB).
        assert!(mn.x <= bmn.x + 1e-3 && mn.y <= bmn.y + 1e-3 && mn.z <= bmn.z + 1e-3, "encloses min");
        assert!(mx.x >= bmx.x - 1e-3 && mx.y >= bmx.y - 1e-3 && mx.z >= bmx.z - 1e-3, "encloses max");

        // With no rotation the corner box is EXACT.
        st.furniture[0].rot = [0.0, 0.0, 0.0];
        let (mn2, mx2) = st.furniture_aabb(0).unwrap();
        let mut emn = Vec3::splat(f32::INFINITY);
        let mut emx = Vec3::splat(f32::NEG_INFINITY);
        for p in &asset.positions {
            let w = st.furniture_point(&st.furniture[0], *p);
            emn = emn.min(w);
            emx = emx.max(w);
        }
        assert!((mn2 - emn).length() < 1e-3 && (mx2 - emx).length() < 1e-3, "exact without rotation");
    }

    /// The drag-time GPU model matrix must reproduce the CPU pose EXACTLY, so the dragged
    /// furniture doesn't jump when the drag ends and it rejoins the cached (CPU-posed) buffer.
    #[test]
    fn furniture_model_matrix_matches_cpu_pose() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("wedge".into(), crate::mesh_io::ObjMesh {
            positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            color: None,
            alpha: Vec::new(),
        });
        st.place_furniture(idx, Vec3::new(2.0, 3.0, 1.0));
        st.furniture[0].scale = 1.7;
        st.furniture[0].rot = [20.0, 35.0, 50.0];

        let m = glam::Mat4::from_cols_array(&st.furniture_model_matrix(0).unwrap());
        let asset = &st.furniture_lib[idx];
        for p in &asset.positions {
            let via_matrix = m.transform_point3(Vec3::from(*p));   // GPU path
            let via_point = st.furniture_point(&st.furniture[0], *p); // cached CPU path
            assert!((via_matrix - via_point).length() < 1e-4,
                "model matrix must match furniture_point (no jump on drag-end)");
        }
    }

    /// A copy placed from the library menu lands on the MODEL, not at world origin — the
    /// fix for "loading from the furniture library shows nothing" (it was km off-screen).
    #[test]
    fn default_place_at_is_the_model_centre() {
        let mut st = FactoryState::default();
        // No model yet → origin.
        assert_eq!(st.default_place_at(), Vec3::ZERO);
        // Build a box far from the origin (like a DXF-coordinate import) and re-check.
        let p = Primitive::Box { w: 4.0, d: 4.0, h: 3.0 };
        let placement = Placement { u: 3620.0, v: 958.0, ..Placement::default() };
        st.model.push(BoolOp::Union, Plane::default(), placement, p);
        st.recompute();
        let at = st.default_place_at();
        assert!(at.x > 3000.0 && at.y > 900.0, "lands where the building is, not at (0,0)");
    }

    /// The library persists across save/reload, and instances keep their asset/pose.
    #[test]
    fn furniture_round_trips_through_the_sidecar() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("lamp".into(), tetra());
        st.place_furniture(idx, Vec3::new(1.0, 1.0, 0.0));
        let json = serde_json::to_string(&st.to_persist()).unwrap();
        let back: crate::simlux_io::FactoryDoc = serde_json::from_str(&json).unwrap();

        let mut re = FactoryState::default();
        re.apply_persist(back);
        assert_eq!(re.furniture_lib.len(), 1, "the imported mesh is stored in the project");
        assert_eq!(re.furniture.len(), 1);
        assert!(!re.furniture_verts(false, None).is_empty(), "and still renders after reload");
    }

    /// Furniture is selectable, and selecting it clears the feature selection (they are
    /// mutually exclusive — the gizmo/properties act on one thing).
    #[test]
    fn furniture_selection_is_exclusive_with_features() {
        let mut st = FactoryState::default();
        st.add_box();                          // selects the feature
        assert!(!st.selection.is_empty());
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);   // selects the furniture
        assert_eq!(st.sel_furniture, vec![0]);
        assert!(st.selection.is_empty(), "selecting furniture clears the feature selection");
    }

    /// Furniture rotates about all three axes: yaw 90°/Z sends local +X→+Y; pitch 90°/X
    /// sends +Y→+Z. (Single-axis cases hold regardless of Euler order.)
    #[test]
    fn furniture_rotation_is_three_axis() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        st.furniture[0].rot = [0.0, 0.0, 90.0];
        let p = st.furniture_point(&st.furniture[0], [1.0, 0.0, 0.0]);
        assert!(p.x.abs() < 1e-4 && (p.y - 1.0).abs() < 1e-4, "yaw 90° sends +X→+Y, got {p:?}");
        st.furniture[0].rot = [90.0, 0.0, 0.0];
        let q = st.furniture_point(&st.furniture[0], [0.0, 1.0, 0.0]);
        assert!((q.z - 1.0).abs() < 1e-4, "pitch 90° sends +Y→+Z, got {q:?}");
    }

    /// The 3-axis furniture rotation survives the sidecar (Z via `rot_deg`, X/Y via `rot_xy`).
    #[test]
    fn furniture_three_axis_rotation_survives_the_sidecar() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        st.furniture[0].rot = [12.0, 34.0, 56.0];
        let json = serde_json::to_string(&st.to_persist()).unwrap();
        let back: crate::simlux_io::FactoryDoc = serde_json::from_str(&json).unwrap();
        let mut re = FactoryState::default();
        re.apply_persist(back);
        let r = re.furniture[0].rot;
        assert!((r[0] - 12.0).abs() < 1e-3 && (r[1] - 34.0).abs() < 1e-3 && (r[2] - 56.0).abs() < 1e-3,
            "3-axis rot round-trips, got {r:?}");
    }

    /// A feature's local rotation is settable, reads back, and the model still meshes.
    #[test]
    fn feature_rotation_setter_round_trips() {
        let mut st = FactoryState::default();
        st.add_box();
        let id = st.selected_single().unwrap();
        st.set_feature_rotation(id, 2, 45.0); // spin (about normal)
        st.set_feature_rotation(id, 0, 30.0); // pitch (about u)
        assert_eq!(st.feature_rotation(id), Some([30.0, 0.0, 45.0]));
        st.recompute();
        assert!(!st.cached.positions.is_empty(), "rotated feature still produces a mesh");
    }

    /// The gizmo drives furniture: move_selection shifts the selected instance's position,
    /// and its AABB (what the gizmo hangs off) follows.
    #[test]
    fn move_selection_moves_selected_furniture() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        let c0 = st.selection_center().unwrap();
        st.move_selection(Vec3::new(3.0, -2.0, 1.0));
        let c1 = st.selection_center().unwrap();
        assert!((c1 - c0 - Vec3::new(3.0, -2.0, 1.0)).length() < 1e-3);
    }

    /// Scaling furniture grows its instance scale (and its bounds).
    #[test]
    fn scale_selection_scales_furniture() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        let (mn0, mx0) = st.selection_aabb().unwrap();
        st.scale_selection(2.0);
        let (mn1, mx1) = st.selection_aabb().unwrap();
        let span0 = (mx0 - mn0).length();
        let span1 = (mx1 - mn1).length();
        assert!(span1 > span0 * 1.8, "the instance must grow when scaled up");
        assert!((st.furniture[0].scale - 2.0).abs() < 1e-4);
    }

    /// Scaling a selected SOLID grows its primitive about its centre.
    #[test]
    fn scale_selection_scales_a_solid() {
        let mut st = FactoryState::default();
        st.add_box();
        let (mn0, mx0) = st.selection_aabb().unwrap();
        st.scale_selection(2.0);
        let (mn1, mx1) = st.selection_aabb().unwrap();
        assert!((mx1 - mn1).length() > (mx0 - mn0).length() * 1.8);
    }

    /// Cutaway drops triangles ENTIRELY above the cut plane (top faces) and keeps those
    /// crossing it (walls) — a reliable "see inside" that needs no ceiling tagging. And it
    /// is view-only (the mesh itself is unchanged).
    #[test]
    fn cutaway_hides_geometry_above_the_plane() {
        let mut st = FactoryState::default();
        // A 2×2×4 box spanning z = 0..4.
        st.model.push(cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement::default(), Primitive::Box { w: 2.0, d: 2.0, h: 4.0 });
        st.recompute();
        let full = st.scene_verts().len();
        let mesh_tris = st.cached.tri_count();

        st.cutaway = true;
        st.cutaway_z = 2.0;
        assert!(st.scene_verts().len() < full, "the top cap above the plane is dropped");
        assert!(!st.scene_verts().is_empty(), "the walls crossing the plane remain");
        assert_eq!(st.cached.tri_count(), mesh_tris, "cutaway is view-only, mesh unchanged");
    }

    /// A HIDDEN ceiling must not be pickable — otherwise you select the invisible ceiling
    /// instead of what is behind/below it.
    #[test]
    fn a_hidden_ceiling_is_not_pickable() {
        let mut st = FactoryState::default();
        st.add_building_outline(&[
            Vec2::new(0.0, 0.0), Vec2::new(6.0, 0.0), Vec2::new(6.0, 6.0), Vec2::new(0.0, 6.0),
        ], 3.0).unwrap();
        st.add_room(&[
            Vec2::new(1.0, 1.0), Vec2::new(5.0, 1.0), Vec2::new(5.0, 5.0), Vec2::new(1.0, 5.0),
        ]).unwrap();
        let cid = *st.ceilings.iter().next().expect("a ceiling was made");
        st.recompute();

        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        st.set_view(StdView::Top);
        st.fit();
        let mvp = crate::light3d::mvp(st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target, 800.0/600.0, st.ortho);

        // Straight down the middle from the top: without hiding, the ceiling can be hit.
        st.hide_ceilings = true;
        st.recompute();
        let hit = st.pick_feature(rect.center(), rect, &mvp);
        assert_ne!(hit, Some(cid), "a hidden ceiling must never be the pick result");
    }

    /// Painting a surface stores a per-surface colour that scene_verts uses.
    #[test]
    fn painting_a_surface_colours_only_that_face() {
        let mut st = FactoryState::default();
        st.add_box();                       // 2×2×1 box, feature id 1
        st.recompute();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        st.set_view(StdView::Top);
        st.fit();
        let mvp = crate::light3d::mvp(st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target, 800.0/600.0, st.ortho);

        // From the top, the ray hits the top face — paint it red.
        assert!(st.paint_surface(rect.center(), rect, &mvp, [1.0, 0.0, 0.0]), "the top face must be hit");
        assert_eq!(st.surface_color.len(), 1, "one surface painted");
        // Some triangles now render red; not all (only the painted face).
        let verts = st.scene_verts();
        assert!(verts.iter().any(|v| v.r > 0.8 && v.g < 0.3), "the painted face is red");
        assert!(verts.iter().any(|v| !(v.r > 0.8 && v.g < 0.3)), "other faces are not");
    }

    /// A colour assigned to a feature tints that body's triangles (and only tints — it
    /// does not change the geometry).
    #[test]
    fn feature_colour_tints_only_that_body() {
        let mut st = FactoryState::default();
        st.add_box();                    // feature id 1
        st.recompute();
        let plain = st.scene_verts();
        let id = st.selected_single().unwrap();
        st.feature_color.insert(id, [1.0, 0.0, 0.0]);
        let tinted = st.scene_verts();
        assert_eq!(plain.len(), tinted.len(), "colour must not change triangle count");
        assert!(
            tinted.iter().any(|v| v.r > v.g && v.r > v.b),
            "the coloured body must render reddish"
        );
    }

    /// A feature with no assigned colour renders in the neutral (not blank).
    #[test]
    fn uncoloured_features_use_the_default() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        assert!(st.scene_verts().iter().all(|v| v.r > 0.0 || v.g > 0.0 || v.b > 0.0));
    }
}

#[cfg(test)]
mod gizmo_and_props_tests {
    use super::*;

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
    }
    fn view(st: &FactoryState) -> [f32; 16] {
        crate::light3d::mvp(
            st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target,
            rect().width() / rect().height(), st.ortho,
        )
    }
    fn one_box() -> FactoryState {
        let mut st = FactoryState::default();
        st.add_box();          // id 1, selected
        st.recompute();
        st.fit();
        st
    }
    /// The smallest thing that can be a furniture asset — four faces, no degenerate triangles.
    fn tetra() -> crate::mesh_io::ObjMesh {
        crate::mesh_io::parse_obj(
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\nf 1 2 4\nf 1 3 4\nf 2 3 4\n",
        )
    }

    /// The selection centre is the AABB centre — the gizmo hangs off it.
    #[test]
    fn selection_center_is_the_aabb_center() {
        let st = one_box();
        let (mn, mx) = st.selection_aabb().expect("a selected box has bounds");
        assert_eq!(st.selection_center().unwrap(), (mn + mx) * 0.5);
    }

    /// Moving the selection shifts its centre by exactly the delta, in place (id kept).
    #[test]
    fn move_selection_shifts_the_center_and_keeps_ids() {
        let mut st = one_box();
        let c0 = st.selection_center().unwrap();
        let ids0 = st.selection.clone();
        st.move_selection(Vec3::new(2.0, -1.0, 3.0));
        let c1 = st.selection_center().unwrap();
        assert!((c1 - c0 - Vec3::new(2.0, -1.0, 3.0)).length() < 1e-3);
        assert_eq!(st.selection, ids0, "a move must not renumber the selection");
    }

    /// A position field writes one axis of the world origin and leaves the others.
    #[test]
    fn setting_one_position_axis_leaves_the_others() {
        let mut st = one_box();
        let id = st.selected_single().unwrap();
        let o0 = st.model.features.iter().find(|f| f.id == id).unwrap().world_origin();
        st.set_feature_origin_axis(id, 2, 5.0);   // Z
        let o1 = st.model.features.iter().find(|f| f.id == id).unwrap().world_origin();
        assert!((o1.z - 5.0).abs() < 1e-4);
        assert!((o1.x - o0.x).abs() < 1e-4 && (o1.y - o0.y).abs() < 1e-4);
    }

    /// A dimension field replaces the primitive.
    #[test]
    fn setting_a_dimension_replaces_the_primitive() {
        let mut st = one_box();
        let (id, prim, _) = st.selected_primitive().unwrap();
        let Primitive::Box { w, d, .. } = prim else { panic!("default add_box is a Box") };
        st.set_feature_primitive(id, Primitive::Box { w, d, h: 9.0 });
        let (_, after, _) = st.selected_primitive().unwrap();
        match after {
            Primitive::Box { h, .. } => assert_eq!(h, 9.0),
            other => panic!("expected a Box, got {other:?}"),
        }
    }

    /// The gizmo projects, and clicking its centre returns the Free handle — the
    /// combination-move grab where all three arms meet.
    #[test]
    fn the_center_cube_picks_the_free_handle() {
        let st = one_box();
        let mvp = view(&st);
        let v = st.gizmo_view(rect(), &mvp).expect("a selected object has a gizmo");
        assert_eq!(st.pick_gizmo(v.center_s, rect(), &mvp), Some(GizmoHandle::Free));
    }

    /// Clicking partway along an arm picks that axis, not Free.
    #[test]
    fn clicking_an_arm_picks_that_axis() {
        let st = one_box();
        let mvp = view(&st);
        let v = st.gizmo_view(rect(), &mvp).unwrap();
        for arm in v.arms.iter().flatten() {
            // 70% along the arm — clear of the centre cube.
            let p = v.center_s + (arm.tip_s - v.center_s) * 0.7;
            assert_eq!(
                st.pick_gizmo(p, rect(), &mvp),
                Some(arm.handle),
                "a click along the {:?} arm must pick {:?}",
                arm.handle, arm.handle
            );
        }
    }

    /// The gizmo has a screen-space FLOOR: a tiny object still gets a grabbable gizmo
    /// (arms at least ~65 px), so it stays visible at any object size.
    #[test]
    fn a_tiny_object_still_gets_a_visible_gizmo() {
        let mut st = FactoryState::default();
        let id = st.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            Primitive::Box { w: 0.02, d: 0.02, h: 0.02 },
        );
        st.selection = vec![id];
        st.recompute();
        st.fit();
        let mvp = view(&st);
        let v = st.gizmo_view(rect(), &mvp).unwrap();
        let arm = v.arms.iter().flatten().next().unwrap();
        assert!(
            v.center_s.distance(arm.tip_s) >= 40.0,
            "even a 2 cm object needs a grabbable on-screen gizmo"
        );
    }

    /// Marquee box-select grabs every feature whose projected centre is inside the band,
    /// and leaves the rest alone.
    #[test]
    fn marquee_selects_features_inside_the_band() {
        let mut st = FactoryState::default();
        // Two boxes, far apart in X.
        let a = st.model.push(cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement { u: -3.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            Primitive::Box { w: 0.5, d: 0.5, h: 0.5 });
        let b = st.model.push(cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement { u: 3.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            Primitive::Box { w: 0.5, d: 0.5, h: 0.5 });
        st.selection.clear();
        st.recompute();
        st.fit();
        let mvp = view(&st);

        // A band around box A's screen centre only.
        let sa = crate::factory::world_to_screen(
            { let f = st.model.features.iter().find(|f| f.id == a).unwrap();
              let (mn, mx) = f.world_aabb(); (mn + mx) * 0.5 },
            rect(), &mvp,
        ).unwrap();
        let band = egui::Rect::from_center_size(sa, egui::vec2(30.0, 30.0));
        st.select_in_marquee(band, rect(), &mvp, false);
        assert!(st.selection.contains(&a), "A is inside the band");
        assert!(!st.selection.contains(&b), "B is outside it");
    }

    /// A band dragged across the view must pick up FURNITURE as well as solids.
    ///
    /// A marquee is a statement about a region: someone who draws one round a corner of the villa
    /// means the walls AND the chairs standing in it. Selecting only one kind is the surprise.
    #[test]
    fn the_marquee_takes_furniture_too() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("chair".into(), tetra());
        st.place_furniture(idx, Vec3::new(0.0, 0.0, 0.0));
        st.place_furniture(idx, Vec3::new(40.0, 0.0, 0.0)); // far outside any sane band
        st.clear_selection();
        st.fit();
        let mvp = view(&st);

        let near = st.furniture_aabb(0).map(|(a, b)| (a + b) * 0.5).unwrap();
        let s = crate::factory::world_to_screen(near, rect(), &mvp).unwrap();
        let band = egui::Rect::from_center_size(s, egui::vec2(60.0, 60.0));
        let (feat, furn) = st.select_in_marquee(band, rect(), &mvp, false);
        assert_eq!((feat, furn), (0, 1), "one piece of furniture, no solids");
        assert_eq!(st.sel_furniture, vec![0]);
    }

    /// Shift-click adds a second piece — and clicking it again takes it back out.
    ///
    /// Toggling rather than only adding is what makes the gesture reversible: overshoot by one and
    /// you undo it the same way you did it, instead of starting the selection over.
    #[test]
    fn shift_click_toggles_a_piece_in_and_out() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("chair".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        st.place_furniture(idx, Vec3::new(2.0, 0.0, 0.0));
        st.select_furniture(0);
        st.toggle_furniture(1);
        assert_eq!(st.sel_furniture, vec![0, 1], "both are selected");
        st.toggle_furniture(1);
        assert_eq!(st.sel_furniture, vec![0], "…and the second one came back out");
        assert_eq!(st.sel_furn_primary(), Some(0), "the primary is unchanged throughout");
    }

    /// Deleting several pieces must delete the ones that were selected.
    ///
    /// `Vec::remove` shifts everything after it down, so erasing ascending indices removes the
    /// wrong objects — 0 then 2 takes out the first and the fourth. This is the whole reason
    /// `erase_selected_furniture` walks the list backwards.
    #[test]
    fn erasing_several_pieces_removes_the_right_ones() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("chair".into(), tetra());
        for x in 0..4 {
            st.place_furniture(idx, Vec3::new(x as f32, 0.0, 0.0));
        }
        st.sel_furniture = vec![0, 2];
        assert_eq!(st.erase_selected_furniture(), 2);
        assert_eq!(st.furniture.len(), 2);
        let xs: Vec<f32> = st.furniture.iter().map(|f| f.pos[0]).collect();
        assert_eq!(xs, vec![1.0, 3.0], "the pieces at x=0 and x=2 went, not their neighbours");
        assert!(st.sel_furniture.is_empty(), "nothing is left selected");
    }

    /// Moving a multi-selection moves every piece by the same delta, keeping the arrangement.
    #[test]
    fn moving_several_pieces_keeps_them_in_formation() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("chair".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        st.place_furniture(idx, Vec3::new(2.0, 0.0, 0.0));
        st.sel_furniture = vec![0, 1];
        st.selection.clear();
        st.move_selection(Vec3::new(1.0, 5.0, 0.0));
        assert!((st.furniture[0].pos[0] - 1.0).abs() < 1e-4);
        assert!((st.furniture[1].pos[0] - 3.0).abs() < 1e-4);
        assert!((st.furniture[1].pos[1] - 5.0).abs() < 1e-4);
    }

    /// The selection bounds must cover EVERY selected piece, not just the first — the gizmo hangs
    /// off this, and anchored to one chair of a selected row it would sit off to one side.
    #[test]
    fn the_bounds_of_a_multi_selection_cover_all_of_it() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("chair".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        st.place_furniture(idx, Vec3::new(10.0, 0.0, 0.0));
        st.sel_furniture = vec![0, 1];
        st.selection.clear();
        let (mn, mx) = st.selection_aabb().expect("a multi-selection has bounds");
        assert!(mn.x <= 0.01 && mx.x >= 9.99, "bounds {mn:?}..{mx:?} miss a piece");
        let c = st.selection_center().unwrap();
        assert!((c.x - 5.0).abs() < 0.5, "the centre sits between them, not on one");
    }

    /// An editor that writes one set of numbers must refuse a multi-selection outright.
    ///
    /// With several pieces selected there is no single set of dimensions to show, and writing the
    /// primary's numbers to all of them would silently resize things nobody looked at.
    #[test]
    fn a_single_object_editor_sees_nothing_when_several_are_selected() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("chair".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        st.place_furniture(idx, Vec3::new(2.0, 0.0, 0.0));
        st.select_furniture(1);
        assert_eq!(st.sel_furn_one(), Some(1), "one selected — the editor opens");
        st.toggle_furniture(0);
        assert_eq!(st.sel_furn_one(), None, "two selected — the editor stands down");
        assert_eq!(st.sel_furn_primary(), Some(1), "…but the primary is still known");
    }

    /// Empty selection: no gizmo, nothing to pick.
    #[test]
    fn no_selection_no_gizmo() {
        let mut st = one_box();
        st.clear_selection();
        let mvp = view(&st);
        assert!(st.gizmo_view(rect(), &mvp).is_none());
    }

    /// Deleting the selection removes its features.
    #[test]
    fn erase_selection_removes_the_features() {
        let mut st = one_box();
        assert_eq!(st.model.features.len(), 1);
        st.erase_selection();
        assert!(st.model.features.is_empty());
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
    }

    fn view(st: &FactoryState) -> [f32; 16] {
        crate::light3d::mvp(
            st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target,
            rect().width() / rect().height(), st.ortho,
        )
    }

    fn wall_app() -> (FactoryState, usize) {
        let mut st = FactoryState::default();
        let wi = st
            .add_wall(
                vec![Vec2::new(-2.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(2.0, 3.0)],
                0.2,
                2.5,
            )
            .unwrap();
        st.recompute();
        st.fit();
        (st, wi)
    }

    /// Handles belong to the SELECTED wall. With nothing selected there are none — the
    /// model would be buried in dots if every wall showed them at once.
    #[test]
    fn handles_follow_the_selection() {
        let (mut st, wi) = wall_app();
        assert!(st.selected_wall().is_none(), "nothing selected ⇒ no wall");
        st.select_wall(wi);
        assert_eq!(st.selected_wall(), Some(wi));
    }

    /// One handle per footprint point, and one edge handle per segment.
    #[test]
    fn there_is_one_handle_per_vertex_and_one_per_edge() {
        let (st, wi) = wall_app();
        let mvp = view(&st);
        assert_eq!(st.wall_vertex_handles(wi, rect(), &mvp).len(), 3);
        assert_eq!(st.wall_edge_handles(wi, rect(), &mvp).len(), 2);
    }

    /// Clicking a drawn handle picks THAT vertex — the projection used for drawing and
    /// the one used for picking must agree, or handles would be un-grabbable.
    #[test]
    fn picking_at_a_drawn_handle_returns_that_vertex() {
        let (st, wi) = wall_app();
        let mvp = view(&st);
        for (vi, p) in st.wall_vertex_handles(wi, rect(), &mvp) {
            assert_eq!(st.pick_wall_vertex(wi, p, rect(), &mvp), Some(vi));
        }
    }

    /// A vertex must WIN a close call against an edge midpoint — otherwise dragging a
    /// corner would insert a point instead of moving it.
    #[test]
    fn a_vertex_beats_an_edge_midpoint_on_a_close_call() {
        let (st, wi) = wall_app();
        let mvp = view(&st);
        let (_, vp) = st.wall_vertex_handles(wi, rect(), &mvp)[0];
        assert!(st.pick_wall_vertex(wi, vp, rect(), &mvp).is_some());
        assert!(
            st.pick_wall_edge(wi, vp, rect(), &mvp).is_none(),
            "on a vertex, the edge pick must stand down"
        );
    }

    /// Clicking empty space grabs nothing.
    #[test]
    fn picking_away_from_every_handle_returns_nothing() {
        let (st, wi) = wall_app();
        let mvp = view(&st);
        let far = egui::pos2(5.0, 5.0);
        assert!(st.pick_wall_vertex(wi, far, rect(), &mvp).is_none());
        assert!(st.pick_wall_edge(wi, far, rect(), &mvp).is_none());
    }

    /// THE hazard of this slice: a shape edit calls `rederive_wall`, which drops the
    /// wall's Boxes and pushes new ones. `Model::push` mints `max(id) + 1`, so the new
    /// ids differ whenever the wall was NOT the highest-numbered feature — here, a solid
    /// added after the wall. The stale selection then resolves to no wall at all, and the
    /// handles would vanish mid-drag. `select_wall` is the refresh that prevents it.
    ///
    /// (With a lone wall the ids happen to be reused, because removing them empties the
    /// model and numbering restarts — which is exactly why this test adds the box.)
    #[test]
    fn reselecting_keeps_the_handles_alive_across_a_shape_edit() {
        let (mut st, wi) = wall_app();
        st.add_box();                 // now the wall is no longer the highest id
        st.select_wall(wi);
        let before = st.selection.clone();

        st.wall_move_vertex(wi, 1, Vec2::new(3.0, 1.0));
        assert_ne!(st.walls[wi].segments, before, "the rebuild really did mint new ids");
        assert!(
            st.selected_wall().is_none(),
            "the stale selection no longer resolves — the test is meaningful"
        );

        st.select_wall(wi);
        assert_eq!(st.selected_wall(), Some(wi), "handles must survive the edit");
        assert_ne!(st.selection, before, "and they track the NEW ids");
    }

    /// Handles sit on the wall's OWN storey, not the ground.
    #[test]
    fn handles_sit_on_the_walls_own_storey() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let base = st.active_base_z();
        let wi = st.add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(3.0, 0.0)], 0.2, 2.5).unwrap();
        assert_eq!(st.wall_vertex_world(wi, 0).unwrap().z, base);
    }
}

#[cfg(test)]
mod slab_tests {
    use super::*;

    fn square(s: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0), Vec2::new(s, 0.0), Vec2::new(s, s), Vec2::new(0.0, s),
            Vec2::new(0.0, 0.0),
        ]
    }

    /// A tessellated circle — exactly what "make room from a plan circle" feeds `add_room`.
    fn circle(r: f32, n: usize) -> Vec<Vec2> {
        let mut v: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec2::new(r * a.cos(), r * a.sin())
            })
            .collect();
        v.push(v[0]); // close it
        v
    }

    /// Highest rendered surface DIRECTLY ABOVE `(x, y)` — the z of the topmost triangle
    /// whose 2D projection contains the point. Robust to how the cap is triangulated (unlike
    /// a centroid-proximity test). Used to check the interior opens over the room centre:
    /// ceiling level before hiding, floor level after. Returns `f32::MIN` if nothing covers it.
    fn ceiling_z_at(st: &FactoryState, x: f32, y: f32) -> f32 {
        let mut best = f32::MIN;
        for t in st.scene_verts().chunks_exact(3) {
            let (ax, ay) = (t[0].x, t[0].y);
            let (bx, by) = (t[1].x, t[1].y);
            let (cx, cy) = (t[2].x, t[2].y);
            let d1 = (x - bx) * (ay - by) - (ax - bx) * (y - by);
            let d2 = (x - cx) * (by - cy) - (bx - cx) * (y - cy);
            let d3 = (x - ax) * (cy - ay) - (cx - ax) * (y - ay);
            let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
            let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
            if !(has_neg && has_pos) {
                let z = (t[0].z + t[1].z + t[2].z) / 3.0;
                if z > best {
                    best = z;
                }
            }
        }
        best
    }

    /// REPRODUCTION of the user's report: a room made from a circle in the plan, then
    /// Hide-ceilings does nothing. Dumps the tracked ceiling id against the face_ids that
    /// actually reach the renderer, so we can SEE whether they match.
    #[test]
    fn repro_hide_ceiling_on_a_circle_room() {
        let mut st = FactoryState::default();
        st.add_room(&circle(25.0, 32)).unwrap();
        st.recompute();

        let ceil_ids: Vec<u32> = st.ceilings.iter().copied().collect();
        let mut uniq: Vec<u32> = st.cached.face_ids.clone();
        uniq.sort_unstable();
        uniq.dedup();
        eprintln!("REPRO: ceilings set = {ceil_ids:?}");
        eprintln!("REPRO: unique rendered face_ids = {uniq:?}");
        let ceil_tris = st
            .cached
            .face_ids
            .iter()
            .filter(|id| st.ceilings.contains(id))
            .count();
        eprintln!("REPRO: rendered triangles tagged as a tracked ceiling = {ceil_tris}");

        // Over the room CENTRE the ceiling must open up (framed-opening look keeps a border
        // over the walls, so the global top may stay — the interior is what must clear).
        let center_shown = ceiling_z_at(&st, 0.0, 0.0);
        st.hide_ceilings = true;
        let center_hidden = ceiling_z_at(&st, 0.0, 0.0);
        eprintln!("REPRO: centre top shown = {center_shown:.3}, hidden = {center_hidden:.3}");
        assert!(ceil_tris > 0, "the ceiling must contribute triangles that hide can drop");
        assert!(center_shown > 2.5, "the ceiling covers the centre before hiding");
        assert!(
            center_hidden < 1.0,
            "the ceiling over the interior must open (hidden centre z {center_hidden:.2})"
        );
    }

    /// THE drift-proof guarantee: "Hide ceilings" must still hide the ceiling even when the
    /// tracked id-set is WRONG — which is exactly what fails in the field (a stale/empty set
    /// hides nothing). Here we deliberately clear `ceilings`; only the GEOMETRIC cap
    /// detection can save it. Hiding must still drop the ceiling and keep the floor + walls.
    #[test]
    fn hide_ceilings_works_even_with_a_broken_ceiling_set() {
        let mut st = FactoryState::default();
        st.add_room(&circle(25.0, 32)).unwrap();
        // Simulate the field bug: the tracked ceiling id no longer matches reality.
        st.ceilings.clear();
        st.recompute(); // recompute detects the cap by GEOMETRY into `ceiling_caps`

        assert!(
            !st.ceiling_caps.is_empty(),
            "geometry must detect the ceiling cap even with the tracked set cleared"
        );

        // Over the centre the ceiling opens even though the id-set was empty (geometry).
        let center_shown = ceiling_z_at(&st, 0.0, 0.0);
        st.hide_ceilings = true;
        let verts = st.scene_verts();
        let center_hidden = ceiling_z_at(&st, 0.0, 0.0);
        eprintln!("REPRO2: centre top shown = {center_shown:.3}, hidden = {center_hidden:.3}");
        assert!(center_shown > 2.5, "the ceiling covers the centre before hiding");
        assert!(
            center_hidden < 1.0,
            "the interior must open by geometry alone (hidden centre z {center_hidden:.2})"
        );
        // The floor is still there to look at (dark floor triangles near z≈0.2 survive).
        let floor_min = verts.iter().map(|v| v.z).fold(f32::MAX, f32::min);
        assert!(floor_min < 0.3, "the floor slab must remain visible, got min z {floor_min:.2}");
        // And the WALLS are NOT removed — plenty of geometry remains.
        assert!(verts.len() > 200, "walls + floor must survive, got {}", verts.len());
    }

    /// DIAGNOSTIC: what does a pure circle room actually contain? Compares against the
    /// user's live model (67 features / 1524 tris) to tell whether there is an extra solid.
    #[test]
    fn diag_circle_room_feature_and_tri_counts() {
        for n in [24usize, 32, 48, 64] {
            let mut st = FactoryState::default();
            st.add_room(&circle(23.0, n)).unwrap();
            st.recompute();
            eprintln!(
                "DIAG: circle({n} seg) room -> {} features, {} tris, ceilings={}, caps={}",
                st.model.features.len(),
                st.cached.tri_count(),
                st.ceilings.len(),
                st.ceiling_caps.len(),
            );
        }
        // Now: a BUILDING solid + a room on the same outline — the "made a building first"
        // path, which would leave a thick disc capping the view after the ceiling is hidden.
        // A building (outer, R=25) with a SMALLER room inside (R=18): the WALL is the ring
        // between them. Hiding must open the room interior and keep the wall ring capped.
        let mut st = FactoryState::default();
        st.add_building_outline(&circle(25.0, 48), 3.0).unwrap();
        st.add_room(&circle(18.0, 48)).unwrap();
        st.recompute();

        // Flat roof at z≈3.0 forming a BORDER over the walls, and the building's WALL tris.
        let roof_border = |st: &FactoryState| {
            st.scene_verts()
                .chunks_exact(3)
                .filter(|t| t.iter().all(|v| (v.z - 3.0).abs() < 0.05))
                .count()
        };
        let building_walls = |st: &FactoryState| {
            st.scene_verts()
                .chunks_exact(3)
                .filter(|t| {
                    t.iter().any(|v| (v.z - 3.0).abs() < 0.05) && t.iter().any(|v| v.z < 1.0)
                })
                .count()
        };
        let center_before = ceiling_z_at(&st, 0.0, 0.0); // over the room interior
        let wall_before = ceiling_z_at(&st, 21.5, 0.0); // over the annulus WALL (R 18→25)
        let walls_before = building_walls(&st);
        st.hide_ceilings = true;
        let center_after = ceiling_z_at(&st, 0.0, 0.0);
        let wall_after = ceiling_z_at(&st, 21.5, 0.0);
        let border_after = roof_border(&st);
        let walls_after = building_walls(&st);
        eprintln!(
            "DIAG: building+room -> {} features, {} tris; centre {center_before:.2}->{center_after:.2}, wall {wall_before:.2}->{wall_after:.2}, border={border_after}, walls {walls_before}->{walls_after}",
            st.model.features.len(),
            st.cached.tri_count(),
        );
        // THE FIX: the roof over the room INTERIOR opens (you see in) …
        assert!(center_before > 2.5, "roof caps the interior to begin with");
        assert!(center_after < 1.0, "hiding opens the interior — no roof over the centre");
        // … the roof over the WALL RING stays (the wall is capped, not opened) …
        assert!(wall_before > 2.5, "the wall ring is capped to begin with");
        assert!(wall_after > 2.5, "the wall ring MUST keep its cap after hiding");
        // … a border of roof remains, and the building's WALLS stay solid.
        assert!(border_after > 0, "a roof cap must remain over the wall ring");
        assert!(walls_before > 0, "the building has walls to begin with");
        assert_eq!(walls_after, walls_before, "the building walls must stay solid");
    }

    /// A plain solid building with NO room under it must NOT be touched by "Hide ceilings" —
    /// the roof-clip only applies to a solid that actually encloses a room.
    #[test]
    fn a_plain_building_is_not_touched_by_hide_ceilings() {
        let mut st = FactoryState::default();
        st.add_building_outline(&square(6.0), 3.0).unwrap();
        st.recompute();
        let shown = st.scene_verts().len();
        st.hide_ceilings = true;
        assert_eq!(
            st.scene_verts().len(),
            shown,
            "a lone building has no room cap, so nothing is hidden"
        );
    }

    /// The walls must NOT be treated as ceilings — hiding must never make a wall vanish.
    /// (Cap detection keys on flat/thin/elevated/topmost, none of which a wall satisfies.)
    #[test]
    fn hiding_ceilings_keeps_the_walls() {
        let mut st = FactoryState::default();
        st.add_room(&square(4.0)).unwrap();
        st.recompute();
        // A wall box is tall/vertical → never a cap.
        for f in &st.model.features {
            let (mn, mx) = f.world_aabb();
            let is_wall = (mx.z - mn.z) > 1.0; // walls span the room height
            if is_wall {
                assert!(!st.ceiling_caps.contains(&f.id), "a wall must not be a ceiling cap");
            }
        }
        st.hide_ceilings = true;
        // Vertical wall faces (normal ~horizontal) must survive hiding.
        let has_vertical = st
            .scene_verts()
            .chunks_exact(3)
            .any(|t| {
                let z = (t[0].z + t[1].z + t[2].z) / 3.0;
                z > 0.5 && z < 2.5 // mid-wall height band
            });
        assert!(has_vertical, "wall geometry at mid height must remain after hiding");
    }

    /// An L-shape — the case a Box genuinely cannot represent.
    fn ell() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 2.0),
            Vec2::new(2.0, 2.0), Vec2::new(2.0, 4.0), Vec2::new(0.0, 4.0),
            Vec2::new(0.0, 0.0),
        ]
    }

    /// A slab is now an EXTRUSION of the real outline, so an L-shaped room gets an
    /// L-shaped floor — not the bounding-box rectangle it used to get. This is THE bug
    /// this rewrite fixes.
    #[test]
    fn an_l_shaped_room_gets_an_l_shaped_floor() {
        let mut st = FactoryState::default();
        let id = st.add_floor(&ell(), 0.2).expect("an L must slab");
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert!(
            matches!(f.primitive, Primitive::Extrusion { .. }),
            "a non-rectangular slab must be an extrusion, not a Box"
        );
        // The extruded L has more triangles than a 6-face box would.
        st.recompute();
        assert!(st.cached.tri_count() > 12, "the L outline must be preserved in the mesh");
    }

    /// A room is a VOID carved from the building — a Difference feature, not a solid.
    #[test]
    fn a_room_is_built_constructively_with_walls() {
        let mut st = FactoryState::default();
        // No building needed — a room builds itself.
        let id = st.add_room(&square(4.0)).expect("a room builds from an outline");
        // The returned id is the floor slab, an extrusion.
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert_eq!(f.op, cad_solid::BoolOp::Union);
        // Walls were added as their own boxes (one per edge of the square = 4).
        let wall_boxes = st.model.features.iter()
            .filter(|x| matches!(x.primitive, Primitive::Box { .. })).count();
        assert!(wall_boxes >= 4, "a square room has at least 4 wall boxes, got {wall_boxes}");
        st.recompute();
        assert!(st.cached.tri_count() > 0, "the room renders");
    }

    /// Toggling hide_ceilings changes scene_verts IMMEDIATELY — no recompute needed. This
    /// is the mechanism the UI relies on; if it regresses, hide silently stops working.
    #[test]
    fn toggling_hide_changes_scene_verts_without_recompute() {
        let mut st = FactoryState::default();
        st.add_room(&square(4.0)).unwrap();
        st.recompute();
        let shown = st.scene_verts().len();
        st.hide_ceilings = true;         // NO recompute
        let hidden = st.scene_verts().len();
        st.hide_ceilings = false;        // NO recompute
        let shown_again = st.scene_verts().len();
        assert!(hidden < shown, "hiding drops triangles at render time");
        assert_eq!(shown, shown_again, "unhiding restores them");
    }

    /// THE actual goal: hiding the ceiling opens the room INTERIOR so from above you see in,
    /// while a border of ceiling is kept over the walls (the framed-opening look). Check the
    /// highest rendered point OVER THE CENTRE drops from the ceiling to the floor.
    #[test]
    fn hiding_removes_the_room_top() {
        let mut st = FactoryState::default();
        st.add_room(&square(4.0)).unwrap(); // ceiling top ≈ 3.05, centre at (2,2)
        st.recompute();
        let center_shown = ceiling_z_at(&st, 2.0, 2.0);
        st.hide_ceilings = true;
        let center_hidden = ceiling_z_at(&st, 2.0, 2.0);
        assert!(center_shown > 2.5, "the ceiling covers the centre before hiding");
        assert!(
            center_hidden < 1.0,
            "hiding must open the interior over the centre (hidden centre z {center_hidden:.2})"
        );
    }

    /// A default room must be TALL — floor + room_height walls + ceiling ≈ 3 m — not a
    /// flat pancake. If this fails, the walls aren't getting their height.
    #[test]
    fn a_default_room_is_full_height() {
        let mut st = FactoryState::default();
        // Defaults: floor 0.2, height 2.7, ceiling 0.15 → top ≈ 3.05 m.
        assert!((st.room_height - 2.7).abs() < 1e-4, "default room height is 2.7");
        st.add_room(&square(4.0)).unwrap();
        st.recompute();
        let (mn, mx) = st.cached.bounds().expect("the room has geometry");
        let tall = mx[2] - mn[2];
        assert!(tall > 2.5, "a default room must be ~3 m tall, got {tall:.2} m");
    }

    /// A room needs NO pre-existing building — it constructs its own floor, walls, ceiling.
    #[test]
    fn a_room_builds_standalone() {
        let mut st = FactoryState::default();
        assert!(st.add_room(&square(4.0)).is_ok(), "a room must build with no building");
        assert!(!st.model.features.is_empty());
        assert_eq!(st.ceilings.len(), 1, "and it has a ceiling");
    }

    /// A room has an explicit floor on the base and a tracked ceiling above the walls.
    #[test]
    fn a_room_has_a_separate_floor_and_ceiling_by_default() {
        let mut st = FactoryState::default();
        st.room_floor = 0.25;
        st.room_height = 2.5;   // floor→ceiling clear height
        st.ceiling_thickness = 0.15;
        st.add_room(&square(4.0)).unwrap();
        // The ceiling sits above the floor + walls: base(0) + floor(0.25) + height(2.5).
        assert_eq!(st.ceilings.len(), 1, "one separate ceiling object");
        let cid = *st.ceilings.iter().next().unwrap();
        let c = st.model.features.iter().find(|f| f.id == cid).unwrap();
        // add_slab lifts by (top_z - thickness): top at 0.25+2.5+0.15 = 2.9, lift = 2.75.
        assert!((c.placement.lift - 2.75).abs() < 1e-3, "ceiling underside at floor + height");
    }

    /// The open-to-sky toggle makes NO ceiling slab.
    #[test]
    fn open_top_room_has_no_ceiling() {
        let mut st = FactoryState::default();
        st.room_open_top = true;
        st.add_building_outline(&square(10.0), 3.0).unwrap();
        st.add_room(&square(4.0)).unwrap();
        assert!(st.ceilings.is_empty(), "an open room has no ceiling object");
    }

    /// A ceiling made with the Make-ceiling TOOL is tracked, so Hide-ceilings hides it too
    /// (previously only room ceilings were hideable).
    #[test]
    fn a_make_ceiling_ceiling_is_hideable() {
        let mut st = FactoryState::default();
        st.add_building_outline(&square(6.0), 3.0).unwrap();
        st.add_ceiling(&square(6.0), 0.2).expect("ceiling made");
        assert_eq!(st.ceilings.len(), 1, "the Make-ceiling result is tracked");
        st.recompute();
        let shown = st.scene_verts().len();
        st.hide_ceilings = true;
        // No recompute needed — hiding is a render-time filter.
        assert!(st.scene_verts().len() < shown, "hiding removes it from the render");
        assert_eq!(st.cached.tri_count(), st.model.eval().tri_count(), "the mesh itself is unchanged");
    }

    /// Hiding ceilings drops ONLY the ceiling slabs from the render — the model keeps them
    /// (for the lighting calc), and nothing else disappears.
    #[test]
    fn hide_ceilings_is_view_only() {
        let mut st = FactoryState::default();
        st.add_building_outline(&square(10.0), 3.0).unwrap();
        st.add_room(&square(4.0)).unwrap();
        st.recompute();
        let shown = st.scene_verts().len();
        let features = st.model.features.len();

        st.hide_ceilings = true;
        assert!(st.scene_verts().len() < shown, "the ceiling slab is not drawn");
        assert_eq!(st.model.features.len(), features, "but no feature is deleted");
        assert_eq!(st.ceilings.len(), 1, "the ceiling is still tracked");
    }

    /// On an UPPER storey the room is built on THAT storey's base — the void clears from
    /// the upper base and the floor slab sits there, not on the ground.
    #[test]
    fn an_upper_storey_room_sits_on_its_own_floor() {
        let mut st = FactoryState::default();
        st.room_floor = 0.2;
        st.room_height = 2.5;
        st.add_storey_on_top();     // storey 1 active
        let base = st.active_base_z();
        assert!(base > 0.0, "we are on an upper storey");
        st.add_room(&square(4.0)).unwrap();
        // The ceiling sits a storey up (base + floor + height, above the ground storey).
        let cid = *st.ceilings.iter().next().unwrap();
        let c = st.model.features.iter().find(|f| f.id == cid).unwrap();
        assert!(c.placement.lift > base, "the room is built on the upper storey");
    }

    /// A rectangle still slabs fine — the general path must not regress the simple case.
    #[test]
    fn a_rectangular_outline_still_slabs() {
        let mut st = FactoryState::default();
        assert!(st.add_floor(&square(4.0), 0.2).is_some());
    }

    /// A floor's TOP face is the level you stand on, so it sits below the storey base.
    #[test]
    fn a_floor_sits_below_the_level_it_serves() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let base = st.active_base_z();
        let id = st.add_floor(&square(3.0), 0.25).unwrap();
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert!(
            (f.placement.lift - (base - 0.25)).abs() < 1e-4,
            "the floor's top face must land on the storey base"
        );
    }

    /// A ceiling closes the storey at the level above, so a ceiling and the floor above
    /// it meet rather than overlap.
    #[test]
    fn a_ceiling_closes_the_storey_at_the_level_above() {
        let mut st = FactoryState::default();
        let top = st.storeys[0].height;
        let id = st.add_ceiling(&square(3.0), 0.2).unwrap();
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert!((f.placement.lift + 0.2 - top).abs() < 1e-4);
    }

    /// A degenerate outline has no slab — better nothing than a zero-volume solid.
    #[test]
    fn a_zero_area_outline_makes_no_slab() {
        let mut st = FactoryState::default();
        let flat = vec![
            Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(2.0, 0.0),
            Vec2::new(0.0, 0.0),
        ];
        assert!(st.add_slab(&flat, 0.2, 0.0).is_none());
    }

    /// Slabs are ordinary features, so the derived z-band rule assigns them like anything
    /// else — and a floor's BODY lies below the level it serves. So an upper floor is
    /// recorded on the storey beneath, which is structurally what it is: level 1's floor
    /// and level 0's ceiling are the same slab. Pinned here because it decides what
    /// `delete_storey` takes with it.
    #[test]
    fn an_upper_floor_belongs_to_the_storey_beneath_it() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        st.add_floor(&square(3.0), 0.2);
        assert!(
            st.features_on_storey(1).is_empty(),
            "the slab lies below level 1's base, so it is not level 1's own geometry"
        );
        assert!(!st.features_on_storey(0).is_empty(), "it caps level 0");
    }

    /// A CEILING, by contrast, lies inside its own storey's band.
    #[test]
    fn a_ceiling_belongs_to_its_own_storey() {
        let mut st = FactoryState::default();
        st.add_ceiling(&square(3.0), 0.2);
        assert!(!st.features_on_storey(0).is_empty());
    }
}

#[cfg(test)]
mod storey_tests {
    use super::*;

    fn fp() -> Vec<Vec2> {
        vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0)]
    }

    /// A building always has exactly one level to begin with, and it starts at zero — so
    /// everything behaves as it did before storeys existed.
    #[test]
    fn a_new_building_has_one_ground_storey() {
        let st = FactoryState::default();
        assert_eq!(st.storeys.len(), 1);
        assert_eq!(st.storey_base_z(0), 0.0);
        assert_eq!(st.active_base_z(), 0.0);
    }

    /// `base_z` is DERIVED, never stored — so the stack is contiguous by construction.
    #[test]
    fn bases_are_the_running_sum_of_the_heights_below() {
        let mut st = FactoryState::default();
        st.storeys = vec![
            Storey { name: "G".into(), height: 3.0 },
            Storey { name: "1".into(), height: 2.5 },
            Storey { name: "2".into(), height: 4.0 },
        ];
        assert_eq!(st.storey_base_z(0), 0.0);
        assert_eq!(st.storey_base_z(1), 3.0);
        assert_eq!(st.storey_base_z(2), 5.5);
        assert_eq!(st.building_total_height(), 9.5);
    }

    /// New geometry is built on the ACTIVE storey — otherwise the selector means nothing.
    #[test]
    fn new_geometry_lands_on_the_active_storey() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();                    // level 1, active
        let base = st.active_base_z();
        assert!(base > 0.0, "the second level cannot start at the ground");

        st.add_wall(fp(), 0.2, 2.5).expect("wall must promote");
        assert_eq!(st.walls[0].base_z, base);
        let z = st.model.features.last().unwrap().world_origin().z;
        assert!((z - base).abs() < 1e-3, "the solid must stand on the active level");
    }

    /// Membership is derived from the z band, so it survives a `rederive_wall` that mints
    /// brand-new feature ids — the failure a stored id list would have.
    #[test]
    fn storey_membership_survives_a_rederive() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let wi = st.add_wall(fp(), 0.2, 2.5).unwrap();
        let before = st.features_on_storey(1);
        assert!(!before.is_empty());

        st.wall_insert_vertex(wi, 0, Vec2::new(2.0, 0.0));   // rebuilds with fresh ids
        let after = st.features_on_storey(1);
        assert!(!after.is_empty(), "the wall must still belong to level 1");
        assert_ne!(before, after, "ids really did change — the test is meaningful");
    }

    /// Thickness is editable after the fact, like height — and IN PLACE, so feature ids
    /// (and therefore the selection and its handles) survive the edit.
    #[test]
    fn wall_thickness_is_editable_without_changing_ids() {
        let mut st = FactoryState::default();
        let wi = st.add_wall(fp(), 0.2, 2.5).unwrap();
        let fid = st.walls[wi].segments[0];
        let ids = st.walls[wi].segments.clone();

        st.set_wall_thickness(fid, 0.45);
        assert_eq!(st.walls[wi].thickness, 0.45);
        assert_eq!(st.walls[wi].segments, ids, "an in-place edit must not renumber");
        // The Box's depth IS the wall thickness.
        let f = st.model.features.iter().find(|f| f.id == fid).unwrap();
        match f.primitive {
            cad_solid::Primitive::Box { d, .. } => assert_eq!(d, 0.45),
            other => panic!("a wall segment must stay a Box, got {other:?}"),
        }
        assert_eq!(st.walls[wi].height, 2.5, "changing thickness must not touch height");
    }

    /// Promoted geometry with no thickness of its own takes the FACTORY setting — the one
    /// that is editable in the 3D panel.
    #[test]
    fn thickness_less_geometry_uses_the_factory_setting() {
        let mut st = FactoryState::default();
        st.wall_thickness = 0.33;
        let wi = st.add_wall(fp(), st.wall_thickness, 2.5).unwrap();
        assert_eq!(st.walls[wi].thickness, 0.33);
    }

    /// Editing a wall on an upper level must not drop it to the ground. This is why
    /// `base_z` lives on the wall and not only in the feature placement.
    #[test]
    fn editing_an_upper_wall_keeps_it_on_its_level() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let base = st.active_base_z();
        let wi = st.add_wall(fp(), 0.2, 2.5).unwrap();

        st.wall_move_vertex(wi, 1, Vec2::new(6.0, 0.0));
        assert_eq!(st.walls[wi].base_z, base);
        for id in &st.walls[wi].segments {
            let z = st.model.features.iter().find(|f| f.id == *id).unwrap().world_origin().z;
            assert!((z - base).abs() < 1e-3, "the wall fell off its storey");
        }
    }

    /// Changing a level's height moves everything ABOVE it, keeping the stack contiguous,
    /// without stretching the geometry that stands ON it.
    #[test]
    fn raising_a_storey_lifts_everything_above_it() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let upper = st.add_wall(fp(), 0.2, 2.5).unwrap();
        let upper_base = st.walls[upper].base_z;
        let wall_height = st.walls[upper].height;

        st.set_storey_height(0, st.storeys[0].height + 1.0);
        assert_eq!(st.walls[upper].base_z, upper_base + 1.0, "the upper level must rise");
        assert_eq!(st.walls[upper].height, wall_height, "its walls must not stretch");
        assert_eq!(st.storey_base_z(1), st.storeys[0].height, "stack stays contiguous");
    }

    /// "Duplicate floor up" copies the active level's geometry onto a new level above,
    /// stacked by the storey height — the visible "add a floor" the user expected.
    #[test]
    fn duplicate_storey_up_stacks_a_copy() {
        let sq = |s: f32| vec![
            Vec2::new(0.0, 0.0), Vec2::new(s, 0.0), Vec2::new(s, s), Vec2::new(0.0, s),
            Vec2::new(0.0, 0.0),
        ];
        let mut st = FactoryState::default();
        st.add_building_outline(&sq(6.0), 3.0).unwrap();  // a ground-floor building
        let before = st.model.features.len();
        let dst = st.duplicate_storey_up().expect("there is geometry to duplicate");
        assert_eq!(st.storeys.len(), 2, "a new level was added");
        assert_eq!(st.active_storey, dst, "the copy's level becomes active");
        assert!(st.model.features.len() > before, "the building was copied, not moved");
        let base = st.storey_base_z(dst);
        assert!(base > 0.0);
        assert!(
            st.model.features.iter().any(|f| (f.world_origin().z - base).abs() < 0.5),
            "the copy stands on the new level"
        );
    }

    /// Duplicating an empty level does nothing (and reports so via `None`).
    #[test]
    fn duplicating_an_empty_level_is_a_noop() {
        let mut st = FactoryState::default();
        assert!(st.duplicate_storey_up().is_none());
        assert_eq!(st.storeys.len(), 1, "no phantom level is created");
    }

    /// Deleting a level takes its geometry with it and closes the gap.
    #[test]
    fn deleting_a_storey_removes_its_geometry_and_closes_the_gap() {
        let mut st = FactoryState::default();
        st.add_wall(fp(), 0.2, 2.5);            // ground
        st.add_storey_on_top();
        st.add_wall(fp(), 0.2, 2.5);            // level 1
        assert_eq!(st.walls.len(), 2);

        assert!(st.delete_storey(0), "deleting the ground level must succeed");
        assert_eq!(st.storeys.len(), 1);
        assert_eq!(st.walls.len(), 1, "the ground wall went with its storey");
        assert_eq!(st.walls[0].base_z, 0.0, "the surviving level dropped to the ground");
    }

    /// A building must always have a level — otherwise `active_storey` indexes nothing.
    #[test]
    fn the_last_storey_cannot_be_deleted() {
        let mut st = FactoryState::default();
        assert!(!st.delete_storey(0));
        assert_eq!(st.storeys.len(), 1);
    }

    /// Levels survive save/reopen, and a pre-storeys sidecar loads as one ground level
    /// rather than a building with none.
    #[test]
    fn storeys_round_trip_and_old_files_get_a_ground_level() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        st.add_wall(fp(), 0.2, 2.5);
        let json = serde_json::to_string(&st.to_persist()).unwrap();
        let back: crate::simlux_io::FactoryDoc = serde_json::from_str(&json).unwrap();

        let mut re = FactoryState::default();
        re.apply_persist(back);
        assert_eq!(re.storeys.len(), 2);
        assert_eq!(re.active_storey, 1);
        assert_eq!(re.walls[0].base_z, st.walls[0].base_z, "the wall kept its level");

        // A sidecar written before storeys existed.
        let mut old = FactoryState::default();
        old.apply_persist(crate::simlux_io::FactoryDoc::default());
        assert_eq!(old.storeys.len(), 1, "an old file must still have a level");
        assert_eq!(old.active_storey, 0);
    }
}

#[cfg(test)]
mod persist_tests {
    use super::*;

    /// A building modelled in 3D must survive save → reopen. Before this, nothing wrote
    /// `factory.model` anywhere: you could model a building, close the app, and lose it.
    /// This proves the whole path INCLUDING the JSON hop, not just the struct copy.
    #[test]
    fn model_and_walls_survive_a_json_round_trip() {
        let mut st = FactoryState::default();
        st.wall_height = 3.4;
        st.building_height = 6.5;
        let fp = vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 3.0)];
        st.add_wall(fp.clone(), 0.25, 3.4).expect("wall must promote");
        st.add_box();
        let features_before = st.model.features.len();

        let json = serde_json::to_string(&st.to_persist()).expect("must serialize");
        let back: crate::simlux_io::FactoryDoc =
            serde_json::from_str(&json).expect("must deserialize");

        let mut re = FactoryState::default();
        assert_eq!(re.apply_persist(back), 0, "nothing should be dropped");
        assert_eq!(re.model.features.len(), features_before);
        assert_eq!(re.walls.len(), 1);
        assert_eq!(re.walls[0].footprint.len(), fp.len(), "footprint must survive intact");
        assert_eq!(re.walls[0].footprint[1], Vec2::new(4.0, 0.0));
        assert_eq!(re.walls[0].thickness, 0.25);
        assert_eq!(re.wall_height, 3.4);
        assert_eq!(re.building_height, 6.5);
        assert!(re.dirty, "a restored model must re-evaluate before it can be drawn");
    }

    /// A wall whose segments name features the model does not have is unusable — the
    /// link is what makes it editable. It must be dropped AND counted, never restored
    /// dangling and never dropped in silence.
    #[test]
    fn a_wall_with_dangling_feature_ids_is_dropped_and_counted() {
        let mut st = FactoryState::default();
        st.add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0)], 0.2, 2.5);
        let mut doc = st.to_persist();
        doc.walls[0].segments = vec![9999];       // no such feature

        let mut re = FactoryState::default();
        assert_eq!(re.apply_persist(doc), 1, "the bad wall must be counted as dropped");
        assert!(re.walls.is_empty());
        assert!(!re.model.features.is_empty(), "the solids themselves still load");
    }

    /// An older sidecar has no heights (serde fills 0.0). Adopting that would give a
    /// building of no height; the live defaults must win.
    #[test]
    fn absent_heights_do_not_flatten_the_building() {
        let mut re = FactoryState::default();
        let (wh, bh) = (re.wall_height, re.building_height);
        assert_eq!(re.apply_persist(crate::simlux_io::FactoryDoc::default()), 0);
        assert_eq!(re.wall_height, wh);
        assert_eq!(re.building_height, bh);
    }

    /// A drawing with no 3D model must not write a factory block.
    #[test]
    fn an_untouched_factory_persists_as_empty() {
        assert!(FactoryState::default().to_persist().is_empty());
    }
}

#[cfg(test)]
mod building_tests {
    use super::*;

    // NOTE (owner, 2026-07-23): the Building section must not re-expose a primitive the
    // Draw3D palette already offers — rectangular / circular / polygonal "elements" were
    // removed because they were Box / Cylinder / Prism under new labels. The section now
    // holds ACTIONS (Make building / walls / floor / ceiling), none of them shape-named,
    // so the `BuildingTool` enum that once guarded this is gone with it.

    /// Building height is a property of the BUILDING, so it lives on the state (like
    /// `wall_height`) and survives across operations rather than resetting per dialog.
    /// It is what the outline will rise to once the extrusion primitive lands.
    #[test]
    fn building_height_is_state_not_dialog() {
        let mut st = FactoryState::default();
        assert!(st.building_height > 0.0, "a building must have a usable default height");
        st.building_height = 4.25;
        st.add_box();   // an unrelated modelling op must not disturb it
        assert_eq!(st.building_height, 4.25);
    }
}

#[cfg(test)]
mod persist_perf_tests {
    use super::*;

    /// The f32 blob codec round-trips exactly (compact binary geometry, not JSON floats).
    #[test]
    fn f32_blob_round_trips() {
        let v: Vec<f32> = (0..300).map(|i| (i as f32) * 0.12345 - 7.0).collect();
        let dec = decode_f32_blob(&encode_f32_blob(&v));
        assert_eq!(dec.len(), v.len());
        assert!(v.iter().zip(&dec).all(|(a, b)| (a - b).abs() < 1e-6), "exact f32 round-trip");
    }

    /// A furniture mesh survives a sidecar round-trip through the compact blobs.
    #[test]
    fn furniture_geometry_persists_via_blob() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset(
            "m".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.5, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                color: None,
                alpha: Vec::new(),
            },
        );
        st.place_furniture(idx, Vec3::new(1.0, 1.0, 0.0));
        let before = st.furniture_lib[idx].positions.clone();

        let doc = st.to_persist();
        // The heavy JSON arrays must be empty — geometry rides in the blob.
        assert!(doc.furniture_lib[idx].positions.is_empty(), "no JSON float arrays written");
        assert!(!doc.furniture_lib[idx].pos_b64.is_empty(), "blob written");

        let mut st2 = FactoryState::default();
        st2.apply_persist(doc);
        let after = st2.furniture_lib[idx].positions.clone();
        assert_eq!(before.len(), after.len(), "vertex count preserved");
        assert!(before.iter().zip(&after).all(|(a, b)|
            (a[0]-b[0]).abs()<1e-4 && (a[1]-b[1]).abs()<1e-4 && (a[2]-b[2]).abs()<1e-4),
            "positions round-trip");
    }

    /// A mixed opaque+glass asset splits correctly and its per-vertex opacity survives a
    /// sidecar round-trip (so a reopened window keeps its see-through panes).
    #[test]
    fn translucent_furniture_splits_and_persists() {
        // Two triangles: tri 0 opaque (frame), tri 1 glass (alpha 0.2).
        let mesh = crate::mesh_io::ObjMesh {
            positions: vec![
                [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 6],
            color: None,
            alpha: vec![1.0, 1.0, 1.0, 0.2, 0.2, 0.2],
        };
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("window".into(), mesh);
        st.place_furniture(idx, Vec3::new(0.0, 0.0, 0.0));
        assert!(st.furniture_lib[idx].is_translucent(), "asset flagged translucent");

        // Solid pass gets ONLY the opaque triangle; the blended pass gets ONLY the glass one.
        assert_eq!(st.furniture_local_mesh(0).len(), 3, "one opaque tri in the solid pass");
        let (_key, glass) = st.furniture_translucent_mesh(0).expect("glass split out");
        assert_eq!(glass.len(), 3, "one glass tri in the transparent pass");
        assert!(glass.iter().all(|v| (v.a - 0.2).abs() < 1e-6), "glass carries its opacity");

        // Round-trip through the compact sidecar blob.
        let doc = st.to_persist();
        assert!(!doc.furniture_lib[idx].alpha_b64.is_empty(), "alpha blob written");
        let mut st2 = FactoryState::default();
        st2.apply_persist(doc);
        assert!(st2.furniture_lib[idx].is_translucent(), "still translucent after reload");
        assert_eq!(st2.furniture_lib[idx].alpha.len(), 6, "per-vertex opacity preserved");
    }

    /// REGRESSION: a TRANSLUCENT asset (glass panes — e.g. the villa after the FBX opacity import)
    /// must still take face/piece clicks. `furniture_face_at` used to bail on `is_translucent()`,
    /// which silently broke per-surface painting on any model containing glass.
    #[test]
    fn face_pick_works_on_translucent_asset() {
        // A 1×1 quad at z=0 (two tris), one vertex-run glass. Identity MVP ⇒ NDC == world, so a
        // click at the rect centre fires a +Z ray through (0,0) — square in the quad.
        let mesh = crate::mesh_io::ObjMesh {
            positions: vec![
                [-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.5, 0.5, 0.0],
                [-0.5, -0.5, 0.0], [0.5, 0.5, 0.0], [-0.5, 0.5, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 6],
            color: None,
            alpha: vec![0.1; 6], // all glass → asset is translucent
        };
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("pane".into(), mesh);
        st.place_furniture(idx, Vec3::ZERO);
        assert!(st.furniture_lib[idx].is_translucent());
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
        let mvp = glam::Mat4::IDENTITY.to_cols_array();
        let hit = st.furniture_face_at(0, egui::pos2(50.0, 50.0), rect, &mvp, false);
        assert!(hit.is_some(), "glass asset takes a face pick");
        let miss = st.furniture_face_at(0, egui::pos2(1.0, 1.0), rect, &mvp, false);
        assert!(miss.is_none(), "a click off the quad still misses");
    }

    /// The OFF-THREAD save/load path: `to_persist_lite` writes empty blobs + `furniture_geom_flat`
    /// supplies the raw geometry a worker compresses; `decode_furniture_lib` restores it. This
    /// mirrors exactly what the save/load workers do, so a mismatch here = corrupted furniture.
    #[test]
    fn furniture_geometry_persists_via_deferred_worker_path() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset(
            "m".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.5, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                color: None,
                alpha: Vec::new(),
            },
        );
        st.place_furniture(idx, Vec3::new(1.0, 1.0, 0.0));
        let before = st.furniture_lib[idx].positions.clone();

        // Main thread: config with EMPTY furniture blobs + the raw flattened geometry.
        let mut doc = st.to_persist_lite();
        let geom = st.furniture_geom_flat();
        assert!(doc.furniture_lib[idx].pos_b64.is_empty(), "lite leaves blobs empty");
        assert_eq!(geom.len(), doc.furniture_lib.len(), "one raw geom per asset");

        // Worker: compress the raw geometry into the blobs (what save_file_worker does).
        for (rec, g) in doc.furniture_lib.iter_mut().zip(geom.iter()) {
            rec.pos_b64 = encode_f32_blob(&g.pos);
            rec.nrm_b64 = encode_f32_blob(&g.nrm);
        }
        assert!(!doc.furniture_lib[idx].pos_b64.is_empty(), "worker filled the blob");

        // Load worker: decode furniture off-thread; main thread installs the prebuilt lib.
        let lib = FactoryState::decode_furniture_lib(std::mem::take(&mut doc.furniture_lib));
        let mut st2 = FactoryState::default();
        st2.apply_persist_prebuilt(doc, lib);
        let after = st2.furniture_lib[idx].positions.clone();
        assert_eq!(before.len(), after.len(), "vertex count preserved");
        assert!(before.iter().zip(&after).all(|(a, b)|
            (a[0]-b[0]).abs()<1e-4 && (a[1]-b[1]).abs()<1e-4 && (a[2]-b[2]).abs()<1e-4),
            "positions round-trip through the deferred worker path");
    }
}

#[cfg(test)]
mod aperture_tests {
    use super::*;

    /// A furniture asset whose LOCAL box is exactly width×depth×height (X,Y,Z), so the fit math
    /// is checkable. `add_furniture_asset` centres X/Y and seats the base at z=0; with the longest
    /// side < 20 it applies no auto-scale, so the local box is [-w/2,w/2]×[-d/2,d/2]×[0,h].
    fn box_asset(st: &mut FactoryState, w: f32, d: f32, h: f32) -> usize {
        let positions = vec![
            [-w/2.0, -d/2.0, 0.0], [w/2.0, -d/2.0, 0.0], [w/2.0, d/2.0, 0.0],
            [-w/2.0, d/2.0, h],    [w/2.0, d/2.0, h],    [-w/2.0, -d/2.0, h],
        ];
        let normals = vec![[0.0, 0.0, 1.0]; positions.len()];
        st.add_furniture_asset("box".into(), crate::mesh_io::ObjMesh { positions, normals, color: None, alpha: Vec::new() })
    }

    /// scale_vec resolves uniform vs non-uniform correctly, and `scale` multiplies on top.
    #[test]
    fn scale_vec_resolves_uniform_and_fit() {
        let mut inst = FurnitureInst { asset: 0, pos: [0.0;3], scale: 2.0, fit: None, rot: [0.0;3], color: [0.8;3], texture: None, surface_texture: std::collections::HashMap::new(), ..Default::default() };
        assert_eq!(inst.scale_vec(), Vec3::splat(2.0), "uniform");
        inst.scale = 1.0;
        inst.fit = Some([1.2, 3.0, 1.05]);
        assert_eq!(inst.scale_vec(), Vec3::new(1.2, 3.0, 1.05), "fit used verbatim at scale 1");
        inst.scale = 2.0;
        assert_eq!(inst.scale_vec(), Vec3::new(2.4, 6.0, 2.1), "scale multiplies fit");
    }

    /// place_aperture fills the opening exactly: the placed instance's world AABB matches the
    /// opening's width/depth/height and is centred on the opening centre (axis-aligned wall).
    #[test]
    fn aperture_fills_the_opening_axis_aligned() {
        let mut st = FactoryState::default();
        let a = box_asset(&mut st, 1.0, 0.1, 2.0);
        let center = Vec3::new(10.0, 5.0, 1.5);
        let (w, h, depth) = (1.2, 2.1, 0.3);
        let i = st.place_aperture(a, center, Vec3::X, w, h, depth).unwrap();
        let (mn, mx) = st.furniture_aabb(i).unwrap();
        let sz = mx - mn;
        assert!((sz.x - w).abs() < 1e-3, "width along X: {} vs {w}", sz.x);
        assert!((sz.y - depth).abs() < 1e-3, "depth along Y: {} vs {depth}", sz.y);
        assert!((sz.z - h).abs() < 1e-3, "height along Z: {} vs {h}", sz.z);
        let c = (mn + mx) * 0.5;
        assert!((c - center).length() < 1e-3, "centred on the opening: {c:?} vs {center:?}");
    }

    /// An aperture (placed with `fit`) is flagged `is_aperture`; ordinary uniform-scale furniture
    /// is not — this is the signal that gives a door/window selection priority over its wall.
    #[test]
    fn is_aperture_flags_only_fitted_pieces() {
        let mut st = FactoryState::default();
        let a = box_asset(&mut st, 1.0, 0.1, 2.0);
        let ap = st.place_aperture(a, Vec3::new(1.0, 0.0, 1.0), Vec3::X, 1.0, 2.0, 0.2).unwrap();
        assert!(st.is_aperture(ap), "a fitted door/window is an aperture");
        st.place_furniture(a, Vec3::new(5.0, 5.0, 0.0));
        let plain = st.furniture.len() - 1;
        assert!(!st.is_aperture(plain), "uniform-scale furniture is not an aperture");
    }

    /// A bundled door/window IMPORTED free-standing (so it has no `fit`) is still recognised as an
    /// aperture by its asset name — this is the case the pick diagnostic showed failing (ap=None).
    #[test]
    fn is_aperture_recognizes_named_window_without_fit() {
        let mut st = FactoryState::default();
        let win = {
            let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 2.0]];
            let normals = vec![[0.0, 1.0, 0.0]; 3];
            st.add_furniture_asset("Window".into(),
                crate::mesh_io::ObjMesh { positions, normals, color: None, alpha: Vec::new() })
        };
        st.place_furniture(win, Vec3::new(0.0, 0.0, 0.0)); // free-standing → no fit
        let i = st.furniture.len() - 1;
        assert!(st.furniture[i].fit.is_none(), "placed without a fit");
        assert!(st.is_aperture(i), "a Window-named asset is an aperture even without fit");
    }

    /// The aperture selection tolerance scales with the wall thickness it fills, so a door in a
    /// thick wall is as easy to click as one in a thin wall (and is clamped to a usable range).
    #[test]
    fn aperture_pick_tol_scales_with_thickness() {
        let mut st = FactoryState::default();
        let a = box_asset(&mut st, 1.0, 0.1, 2.0);
        let thin = st.place_aperture(a, Vec3::new(0.0, 0.0, 1.0), Vec3::X, 1.0, 2.0, 0.1).unwrap();
        let thick = st.place_aperture(a, Vec3::new(5.0, 0.0, 1.0), Vec3::X, 1.0, 2.0, 1.0).unwrap();
        let (t_thin, t_thick) = (st.aperture_pick_tol(thin), st.aperture_pick_tol(thick));
        assert!(t_thick > t_thin, "thicker wall → larger tolerance: {t_thin} vs {t_thick}");
        assert!((0.2..=3.0).contains(&t_thin), "clamped to a usable range: {t_thin}");
    }

    /// A wall running along world-Y (opening horizontal axis = Y): width/height/depth still match,
    /// just remapped to world axes, and the piece stays centred. Guards the yaw math.
    #[test]
    fn aperture_fills_the_opening_rotated_wall() {
        let mut st = FactoryState::default();
        let a = box_asset(&mut st, 1.0, 0.1, 2.0);
        let center = Vec3::new(-3.0, 7.0, 1.4);
        let (w, h, depth) = (0.9, 2.0, 0.25);
        let i = st.place_aperture(a, center, Vec3::Y, w, h, depth).unwrap();
        let (mn, mx) = st.furniture_aabb(i).unwrap();
        let sz = mx - mn;
        // u_h = +Y → width along Y, depth along X, height along Z.
        assert!((sz.y - w).abs() < 1e-3, "width along Y: {} vs {w}", sz.y);
        assert!((sz.x - depth).abs() < 1e-3, "depth along X: {} vs {depth}", sz.x);
        assert!((sz.z - h).abs() < 1e-3, "height along Z: {} vs {h}", sz.z);
        let c = (mn + mx) * 0.5;
        assert!((c - center).length() < 1e-3, "centred on the opening: {c:?}");
    }

    /// The non-uniform `fit` survives a sidecar round-trip (so a placed door/window keeps its
    /// stretched shape after save/reload).
    #[test]
    fn aperture_fit_persists() {
        let mut st = FactoryState::default();
        let a = box_asset(&mut st, 1.0, 0.1, 2.0);
        st.place_aperture(a, Vec3::new(2.0, 2.0, 1.0), Vec3::X, 1.1, 2.0, 0.2);
        let fit_before = st.furniture[0].fit;
        assert!(fit_before.is_some(), "aperture carries a non-uniform fit");

        let doc = st.to_persist();
        let mut st2 = FactoryState::default();
        st2.apply_persist(doc);
        assert_eq!(st2.furniture.len(), 1, "instance restored");
        let fit_after = st2.furniture[0].fit.expect("fit restored");
        let fb = fit_before.unwrap();
        assert!((0..3).all(|k| (fit_after[k] - fb[k]).abs() < 1e-5), "fit round-trips: {fit_after:?} vs {fb:?}");
    }
}

#[cfg(test)]
mod cutout_tests {
    use super::*;

    /// Cutouts (Difference features) are listed and can be deleted, filling the opening back in.
    #[test]
    fn cutout_list_and_delete() {
        let mut st = FactoryState::default();
        st.model.push(
            cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement::default(), Primitive::Box { w: 2.0, d: 2.0, h: 2.0 },
        );
        st.recompute();
        let plain = st.scene_verts().len();

        st.model.push(
            cad_solid::BoolOp::Difference, cad_solid::Plane::default(),
            cad_solid::Placement::default(), Primitive::Box { w: 0.5, d: 0.5, h: 3.0 },
        );
        st.recompute();
        assert_ne!(st.scene_verts().len(), plain, "the cut changed the geometry");

        let ids = st.cutout_ids();
        assert_eq!(ids.len(), 1, "one cutout listed");
        let id = ids[0];
        assert!(st.cutout_size(id).is_some(), "cutout has a size");

        st.select_cutout(id);
        assert_eq!(st.selected_single(), Some(id), "the cutout is selected");

        st.delete_cutout(id);
        st.recompute();
        assert!(st.cutout_ids().is_empty(), "cutout removed");
        assert_eq!(st.scene_verts().len(), plain, "the opening filled back to the plain solid");
    }
}

#[cfg(test)]
mod decimation_tests {
    use super::*;

    /// Vertex clustering drops sub-cell slivers and keeps genuine triangles (welded to cell
    /// centroids), so a dense mesh collapses toward its silhouette.
    #[test]
    fn cluster_decimate_drops_slivers_keeps_shape() {
        let pos = vec![
            [0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0], // spans distinct cells → kept
            [0.0, 0.0, 0.0], [0.001, 0.0, 0.0], [0.0, 0.001, 0.0], // sub-cell sliver → dropped
        ];
        let (dp, dn) = cluster_decimate(&pos, 8);
        assert_eq!(dp.len() / 3, 1, "one real triangle survives, the sliver is dropped");
        assert_eq!(dp.len(), dn.len(), "a normal per vertex");
    }

    /// A normal-sized asset is never decimated; the LOD is reserved for very heavy imports.
    #[test]
    fn small_asset_skips_lod() {
        let a = FurnitureAsset::new(
            "x".into(),
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            vec![[0.0, 0.0, 1.0]; 3],
            [0.8, 0.8, 0.8],
        );
        assert!(!a.needs_lod(), "3-triangle asset needs no proxy");
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    fn tri_mesh() -> crate::mesh_io::ObjMesh {
        crate::mesh_io::ObjMesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            color: None,
            alpha: Vec::new(),
        }
    }

    /// Ctrl+C / Ctrl+V on furniture: a paste adds an offset clone and selects the copy.
    #[test]
    fn copy_paste_furniture_clones_offset() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("chair".into(), tri_mesh());
        st.place_furniture(idx, Vec3::new(1.0, 2.0, 0.0));
        st.select_furniture(0);
        let orig = st.furniture[0].pos;

        assert!(st.copy_selection(), "furniture copied");
        assert_eq!(st.paste_clipboard(), Some(false), "furniture paste (not a feature)");
        assert_eq!(st.furniture.len(), 2, "one clone added");
        let copy = st.furniture[1].pos;
        assert!((copy[0] - orig[0] - 0.3).abs() < 1e-4 && (copy[1] - orig[1] - 0.3).abs() < 1e-4, "offset by 0.3 m");
        assert_eq!(st.sel_furniture, vec![1], "the copy is selected");
    }

    /// Ctrl+C / Ctrl+V on a CSG feature: clones it (new id), carries the colour, offsets it.
    #[test]
    fn copy_paste_feature_clones_with_colour() {
        let mut st = FactoryState::default();
        st.model.push(
            BoolOp::Union, Plane::default(), Placement::default(),
            Primitive::Box { w: 1.0, d: 1.0, h: 1.0 },
        );
        st.recompute();
        let id = st.model.features.last().unwrap().id;
        st.feature_color.insert(id, [1.0, 0.0, 0.0]);
        st.selection = vec![id];
        let n = st.model.features.len();

        assert!(st.copy_selection(), "feature copied");
        assert_eq!(st.paste_clipboard(), Some(true), "feature paste needs recompute");
        assert_eq!(st.model.features.len(), n + 1, "one feature added");
        let new_id = *st.selection.first().unwrap();
        assert_ne!(new_id, id, "the clone has a fresh id");
        assert_eq!(st.feature_color.get(&new_id).copied(), Some([1.0, 0.0, 0.0]), "colour carried");
        let f = st.model.features.iter().find(|f| f.id == new_id).unwrap();
        assert!((f.placement.u - 0.3).abs() < 1e-4 && (f.placement.v - 0.3).abs() < 1e-4, "placement offset");
    }

    /// Pasting with an empty buffer is a no-op.
    #[test]
    fn paste_empty_clipboard_is_none() {
        let mut st = FactoryState::default();
        assert_eq!(st.paste_clipboard(), None);
    }

    /// A drawn APERTURE (door/window) is a furniture instance with a non-uniform `fit`; copy/paste
    /// must clone it — stretched shape and all — exactly like an imported piece.
    #[test]
    fn copy_paste_aperture_preserves_fit() {
        let mut st = FactoryState::default();
        let a = st.add_furniture_asset("door".into(), crate::mesh_io::ObjMesh {
            positions: vec![
                [-0.5, -0.05, 0.0], [0.5, -0.05, 0.0], [0.5, 0.05, 0.0],
                [-0.5, 0.05, 2.0],  [0.5, 0.05, 2.0],  [-0.5, -0.05, 2.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 6],
            color: Some([0.7, 0.7, 0.7]),
            alpha: Vec::new(),
        });
        st.place_aperture(a, Vec3::new(5.0, 3.0, 1.0), Vec3::X, 1.2, 2.1, 0.3);
        let fit0 = st.furniture[0].fit;
        assert!(fit0.is_some(), "aperture carries a non-uniform fit");
        st.select_furniture(0);
        assert!(st.copy_selection(), "aperture furniture copied");
        assert_eq!(st.paste_clipboard(), Some(false), "furniture paste (not a feature)");
        assert_eq!(st.furniture.len(), 2, "one clone added");
        assert_eq!(st.furniture[1].fit, fit0, "the copy keeps the stretched fit");
        assert_eq!(st.sel_furniture, vec![1], "the copy is selected");
    }

    /// An EXTRUDED solid (the "As furniture" extrude → a Union Extrusion feature) copy/pastes as a
    /// cloned feature — new id, carried colour, and a valid re-evaluated mesh.
    #[test]
    fn copy_paste_extruded_solid_clones() {
        let mut st = FactoryState::default();
        let sq = [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)];
        let (profile, c, w, d) = st.model.add_profile(&sq).unwrap();
        let mut placement = Placement::default();
        placement.u = c.x;
        placement.v = c.y;
        let id = st.model.push(BoolOp::Union, Plane::default(), placement,
            Primitive::Extrusion { profile, h: 0.8, w, d });
        st.feature_color.insert(id, [0.6, 0.62, 0.70]);
        st.recompute();
        let n = st.model.features.len();
        st.sel_furniture.clear();
        st.selection = vec![id];

        assert!(st.copy_selection(), "extruded solid copied");
        assert_eq!(st.paste_clipboard(), Some(true), "feature paste needs recompute");
        st.recompute();
        assert_eq!(st.model.features.len(), n + 1, "one clone added");
        let new_id = *st.selection.first().unwrap();
        assert_ne!(new_id, id, "fresh id");
        assert_eq!(st.feature_color.get(&new_id).copied(), Some([0.6, 0.62, 0.70]), "colour carried");
        assert!(!st.scene_verts().is_empty(), "both solids render");
    }

    /// Pasting empty per-face vectors doesn't add spurious surface entries.
    #[test]
    fn paste_feature_without_paint_adds_no_surface_entries() {
        let mut st = FactoryState::default();
        st.model.push(BoolOp::Union, Plane::default(), Placement::default(),
            Primitive::Box { w: 1.0, d: 1.0, h: 1.0 });
        st.recompute();
        st.selection = vec![st.model.features[0].id];
        assert!(st.copy_selection());
        assert_eq!(st.paste_clipboard(), Some(true));
        assert!(st.surface_color.is_empty() && st.surface_texture.is_empty(), "no stray per-face entries");
    }
}

/// Property-style robustness for the 2D→3D bridge: thousands of RANDOM closed sketches are
/// extruded and the result is checked for the invariants a valid solid must hold (finite coords,
/// non-empty triangle soup, height bounded by the extrude depth, footprint within the sketch).
/// Deterministic (a seeded LCG — no external crate, no `Math::random`), so a failure reproduces.
/// This is the "does the bridge crash on a degenerate sketch?" guard the report can cite.
#[cfg(test)]
mod extrude_property_tests {
    use super::*;

    /// Tiny deterministic PRNG (a linear congruential generator) — reproducible random inputs.
    struct Lcg(u64);
    impl Lcg {
        fn u32(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
        fn f32(&mut self) -> f32 { self.u32() as f32 / u32::MAX as f32 }
        fn range(&mut self, a: f32, b: f32) -> f32 { a + (b - a) * self.f32() }
    }

    /// A simple (non-self-intersecting) polygon: random radii at ANGULARLY SORTED vertices around
    /// a centre — star-shaped, so it's always a valid closed outline `add_profile` accepts.
    fn simple_poly(rng: &mut Lcg, n: usize, cx: f32, cy: f32, r: f32) -> Vec<Vec2> {
        let mut angs: Vec<f32> = (0..n).map(|_| rng.range(0.0, std::f32::consts::TAU)).collect();
        angs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        angs.into_iter()
            .map(|a| {
                let rr = r * rng.range(0.55, 1.0);
                Vec2::new(cx + rr * a.cos(), cy + rr * a.sin())
            })
            .collect()
    }

    #[test]
    fn random_extrusions_are_finite_and_bounded() {
        let mut rng = Lcg(0x1234_5678_9abc_def0);
        let mut extruded = 0;
        for _ in 0..600 {
            let n = 3 + (rng.u32() % 9) as usize; // 3..=11 vertices
            let (cx, cy) = (rng.range(-6.0, 6.0), rng.range(-6.0, 6.0));
            let r = rng.range(0.2, 4.0);
            let h = rng.range(0.05, 3.0);
            let pts = simple_poly(&mut rng, n, cx, cy, r);

            let mut st = FactoryState::default();
            let Ok((profile, centre, w, d)) = st.model.add_profile(&pts) else { continue };
            st.model.push(
                BoolOp::Union, Plane::default(), Placement::default(),
                Primitive::Extrusion { profile, h, w, d },
            );
            st.recompute(); // MUST NOT panic on any random valid sketch

            let pos = &st.cached.positions;
            assert!(!pos.is_empty(), "a valid sketch extrudes to triangles");
            assert_eq!(pos.len() % 3, 0, "triangle soup (3 verts per tri)");
            assert!(pos.len() < 2_000_000, "triangle count stays sane");
            let (mut zmn, mut zmx) = (f32::MAX, f32::MIN);
            let (mut xmn, mut xmx, mut ymn, mut ymx) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for p in pos {
                assert!(p[0].is_finite() && p[1].is_finite() && p[2].is_finite(), "no NaN/Inf vertices");
                zmn = zmn.min(p[2]); zmx = zmx.max(p[2]);
                xmn = xmn.min(p[0]); xmx = xmx.max(p[0]);
                ymn = ymn.min(p[1]); ymx = ymx.max(p[1]);
            }
            assert!(zmx - zmn > 0.0 && zmx - zmn <= h + 1e-2, "solid height ≈ extrude depth");
            // The extruded footprint can't exceed the sketch's own bbox (+ a small margin).
            assert!(xmx - xmn <= w + 1e-2 && ymx - ymn <= d + 1e-2, "footprint within the sketch");
            let _ = centre;
            extruded += 1;
        }
        assert!(extruded > 500, "most random polygons extruded successfully ({extruded}/600)");
    }

    /// Degenerate / malformed sketches are REJECTED cleanly (a typed error), never a panic — the
    /// edge cases users actually hit: too few points, collinear, and a zero-height extrude.
    #[test]
    fn degenerate_sketches_are_rejected_without_panicking() {
        let mut st = FactoryState::default();
        // Fewer than 3 distinct points.
        assert!(st.model.add_profile(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0)]).is_err());
        // All duplicates → collapses below 3.
        assert!(st.model.add_profile(&[Vec2::splat(2.0); 5]).is_err());
        // Collinear → zero area.
        assert!(st.model
            .add_profile(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)])
            .is_err());
        // Bow-tie (self-intersecting).
        assert!(st.model
            .add_profile(&[Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)])
            .is_err());
        // A valid square extruded to ZERO height must not panic (may yield a flat/empty mesh).
        let sq = [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)];
        if let Ok((profile, _c, w, d)) = st.model.add_profile(&sq) {
            st.model.push(BoolOp::Union, Plane::default(), Placement::default(),
                Primitive::Extrusion { profile, h: 0.0, w, d });
            st.recompute(); // no panic
        }
    }

    /// PER-FACE paint survives copy/paste. Paint one face of a box, copy, paste; the pasted
    /// feature (translated + re-evaluated) must carry that paint on the CORRESPONDING face —
    /// which only holds if the surface key was re-derived with the paste's `n·delta` offset shift.
    #[test]
    fn per_face_paint_survives_copy_paste() {
        let mut st = FactoryState::default();
        st.model.push(
            BoolOp::Union, Plane::default(), Placement::default(),
            Primitive::Box { w: 2.0, d: 2.0, h: 2.0 },
        );
        st.recompute();
        let id = st.model.features[0].id;
        // Paint the box's first face.
        let (t0, t1, t2) = {
            let p = &st.cached.positions;
            (p[0], p[1], p[2])
        };
        let key = surface_key(id, t0, t1, t2);
        let paint = [0.9, 0.15, 0.15];
        st.surface_color.insert(key, paint);
        st.selection = vec![id];
        st.sel_furniture.clear();

        assert!(st.copy_selection(), "feature copied");
        assert_eq!(st.paste_clipboard(), Some(true));
        st.recompute();
        let new_id = *st.selection.first().unwrap();
        assert_ne!(new_id, id);

        // A face of the PASTED feature must resolve (via its real geometry) to the copied paint.
        let mut found = false;
        for (i, tri) in st.cached.positions.chunks_exact(3).enumerate() {
            if st.cached.face_ids.get(i).copied() != Some(new_id) { continue; }
            let k = surface_key(new_id, tri[0], tri[1], tri[2]);
            if st.surface_color.get(&k) == Some(&paint) { found = true; break; }
        }
        assert!(found, "per-face paint transferred to the matching face of the pasted feature");
    }
}

#[cfg(test)]
mod texture_tests {
    use super::*;

    /// A clipboard bitmap's average colour drives the immediate tint. A 2×1 image of one
    /// black and one white pixel must average to mid-grey (0.5), and alpha must weight the
    /// average so fully-transparent pixels don't drag it toward black.
    #[test]
    fn texture_average_is_alpha_weighted() {
        // Opaque black + opaque white → mid-grey.
        let rgba = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let t = TextureAsset::new("t".into(), 2, 1, rgba);
        assert!((t.avg[0] - 0.5).abs() < 1e-3, "avg R = {}", t.avg[0]);

        // Opaque red + fully-transparent green → the green must not count, so avg ≈ red.
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 0];
        let t = TextureAsset::new("t".into(), 2, 1, rgba);
        assert!(t.avg[0] > 0.99 && t.avg[1] < 0.01, "avg = {:?}", t.avg);
    }

    /// `add_texture` stores the bitmap and hands back its index.
    #[test]
    fn add_texture_appends_and_indexes() {
        let mut st = FactoryState::default();
        let i0 = st.add_texture("a".into(), 1, 1, vec![10, 20, 30, 255]);
        let i1 = st.add_texture("b".into(), 1, 1, vec![40, 50, 60, 255]);
        assert_eq!((i0, i1), (0, 1));
        assert_eq!(st.textures.len(), 2);
        assert_eq!(st.textures[1].w, 1);
    }

    /// `furniture_textured_mesh` is None until a texture is assigned, then yields one UV'd
    /// vertex per position with UVs normalised into [0,1] by box projection.
    #[test]
    fn furniture_textured_mesh_uvs_are_normalised() {
        let mut st = FactoryState::default();
        // A unit-ish triangle mesh spanning the local box [0,0,0]..[1,1,0] (Z-facing).
        let idx = st.add_furniture_asset(
            "t".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                color: None,
                alpha: Vec::new(),
            },
        );
        st.place_furniture(idx, Vec3::new(0.0, 0.0, 0.0));
        assert!(st.furniture_textured_mesh(0).is_none(), "no texture yet → None");

        let ti = st.add_texture("img".into(), 2, 2, vec![255; 16]);
        st.furniture[0].texture = Some(ti);
        let (tex_idx, _key, verts) = st.furniture_textured_mesh(0).expect("textured mesh");
        assert_eq!(tex_idx, ti);
        assert_eq!(verts.len(), 3, "one vertex per position");
        for v in &verts {
            assert!((0.0..=1.0).contains(&v.u) && (0.0..=1.0).contains(&v.v), "uv in [0,1]: {},{}", v.u, v.v);
            assert!(v.s > 0.0 && v.s <= 1.0, "shade in (0,1]: {}", v.s);
        }
        // Z-facing face → UV comes from XY: the (1,0,0) vertex maps to u=1, the (0,1,0) to v=1.
        assert!((verts[1].u - 1.0).abs() < 1e-4 && verts[2].v - 1.0 < 1e-4);
    }

    /// Build a triangle soup of one axis-aligned box at `o` (12 tris). Helper for the per-face tests.
    fn box_soup(o: [f32; 3]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
        let vtx = |x: f32, y: f32, z: f32| [o[0] + x, o[1] + y, o[2] + z];
        let c = [
            vtx(0.0, 0.0, 0.0), vtx(1.0, 0.0, 0.0), vtx(1.0, 1.0, 0.0), vtx(0.0, 1.0, 0.0),
            vtx(0.0, 0.0, 1.0), vtx(1.0, 0.0, 1.0), vtx(1.0, 1.0, 1.0), vtx(0.0, 1.0, 1.0),
        ];
        let mut pos = Vec::new();
        let mut nrm = Vec::new();
        let mut quad = |a: usize, b: usize, cc: usize, d: usize| {
            let (pa, pb, pcc, pd) = (Vec3::from(c[a]), Vec3::from(c[b]), Vec3::from(c[cc]), Vec3::from(c[d]));
            let n = (pb - pa).cross(pcc - pa).normalize_or_zero().to_array();
            for p in [c[a], c[b], c[cc], c[a], c[cc], c[d]] {
                pos.push(p);
                nrm.push(n);
            }
        };
        quad(0, 1, 2, 3);
        quad(4, 5, 6, 7);
        quad(0, 1, 5, 4);
        quad(3, 2, 6, 7);
        quad(0, 3, 7, 4);
        quad(1, 2, 6, 5);
        (pos, nrm)
    }

    /// Grouping: group ≥ 2 features → picking any member expands to the whole group; explode
    /// releases them; deleting a member drops it from the group.
    #[test]
    fn feature_grouping_selects_moves_and_explodes_as_one() {
        let mut st = FactoryState::default();
        let mut add_box = || {
            st.model.push(
                cad_solid::BoolOp::Union, cad_solid::Plane::default(), cad_solid::Placement::default(),
                Primitive::Box { w: 1.0, d: 1.0, h: 1.0 },
            )
        };
        let (a, b, c) = (add_box(), add_box(), add_box());

        // Group A + B.
        st.selection = vec![a, b];
        assert_eq!(st.group_selection(), 2);
        assert_eq!(st.selection_group(), st.feature_group.get(&a).copied());

        // Picking just A expands to {A, B} but not C.
        st.selection = vec![a];
        st.expand_selection_to_groups();
        assert!(st.selection.contains(&a) && st.selection.contains(&b));
        assert!(!st.selection.contains(&c));

        // Deleting B drops it from the group.
        st.selection = vec![b];
        st.erase_selection();
        assert!(!st.feature_group.contains_key(&b));
        assert!(st.feature_group.contains_key(&a));

        // Explode releases the rest.
        st.selection = vec![a];
        assert_eq!(st.ungroup_selection(), 1);
        assert!(st.feature_group.is_empty());
    }

    /// Per-surface texturing: painting one face-group textures only that flat face; the rest stay
    /// flat. Painting a whole body textures every face of that connected piece.
    #[test]
    fn per_surface_furniture_faceted_split() {
        let mut st = FactoryState::default();
        // Two disjoint boxes = one asset with two bodies, 6 flat faces each.
        let (mut pos, mut nrm) = box_soup([0.0, 0.0, 0.0]);
        let (p2, n2) = box_soup([5.0, 0.0, 0.0]);
        pos.extend(p2);
        nrm.extend(n2);
        let idx = st.add_furniture_asset(
            "twobox".into(),
            crate::mesh_io::ObjMesh { positions: pos, normals: nrm, color: None, alpha: Vec::new() },
        );
        st.place_furniture(idx, Vec3::ZERO);
        let ti = st.add_texture("img".into(), 1, 1, vec![255; 4]);

        let groups = st.furniture_lib[idx].group_geom();
        // No per-face textures yet → faceted returns None (whole-object path handles it).
        assert!(st.furniture_faceted(0).is_none());

        // Texture ONLY the face-group of triangle 0 (one flat face = 2 tris = 6 verts).
        let fg0 = groups.face[0];
        st.furniture[0].surface_texture.insert(fg0, ti);
        let fac = st.furniture_faceted(0).expect("faceted split");
        assert_eq!(fac.opaque.len(), 1, "one texture used");
        assert_eq!(fac.opaque[0].0, ti);
        assert_eq!(fac.opaque[0].2.len(), 6, "only that flat face's 2 tris are textured");
        let flat = fac.flat.as_ref().expect("flat remainder");
        assert_eq!(flat.1.len(), st.furniture_lib[idx].positions.len() - 6, "everything else stays flat");

        // Now paint the WHOLE first body: all 6 faces (12 tris = 36 verts) of box 0 textured.
        st.furniture[0].surface_texture.clear();
        let body0 = groups.body[0];
        for t in 0..groups.face.len() {
            if groups.body[t] == body0 {
                st.furniture[0].surface_texture.insert(groups.face[t], ti);
            }
        }
        let fac = st.furniture_faceted(0).expect("faceted split");
        assert_eq!(fac.opaque[0].2.len(), 36, "the whole first box (12 tris) is textured");
        assert_eq!(fac.flat.as_ref().expect("remainder").1.len(), 36, "the second box stays flat");
    }

    /// A GENERATED staircase carries per-primitive part ids, so "piece" grouping gives one tread
    /// per piece — NOT the whole welded run (which geometry-only connectivity would give).
    #[test]
    fn generated_stair_pieces_are_per_primitive_not_the_whole_run() {
        use cad_solid::architecture::{build_stairs, StairParams};
        let sp = StairParams { total_height: 2.0, desired_riser_height: 0.2, ..Default::default() };
        let m = build_stairs(&sp).unwrap();
        let part_ids = m.face_ids.clone();
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset(
            "stair".into(),
            crate::mesh_io::ObjMesh { positions: m.positions, normals: m.normals, color: None, alpha: Vec::new() },
        );
        // Mirror arch_build_and_place: tag the asset with its per-primitive part ids.
        assert_eq!(part_ids.len(), st.furniture_lib[idx].positions.len() / 3);
        st.furniture_lib[idx].part_ids = part_ids;

        let g = st.furniture_lib[idx].group_geom();
        let ntri = st.furniture_lib[idx].positions.len() / 3;
        let bodies = g.body.iter().copied().max().unwrap() + 1;
        assert!(bodies > 10, "many pieces (treads/risers/rails/balusters), got {bodies}");
        // No single piece is the whole object (the old welded-run bug).
        for b in 0..bodies {
            let cnt = g.body.iter().filter(|&&x| x == b).count();
            assert!(cnt * 3 < ntri, "piece {b} is not the whole run ({cnt} of {ntri} tris)");
        }
        // FACE groups must not span primitives: no face group covers a big chunk of the mesh
        // (the coplanar-sides-merge bug — a stair side is coplanar-connected across every tread).
        let faces = g.face.iter().copied().max().unwrap() + 1;
        let biggest = (0..faces)
            .map(|f| g.face.iter().filter(|&&x| x == f).count())
            .max()
            .unwrap();
        assert!(
            biggest < ntri / 8,
            "no face spans the object: biggest face has {biggest} of {ntri} tris ({faces} faces)"
        );
    }

    /// A GENERATED helical ramp is a SMOOTH swept deck (one continuous piece) plus a balustrade of
    /// separate rail/post pieces — the deck is deliberately one solid (a ramp, not steps), while
    /// rails and posts remain their own selectable components.
    #[test]
    fn generated_helical_ramp_deck_is_one_smooth_piece_plus_balustrade() {
        use cad_solid::architecture::{build_helical_ramp, HelicalRampParams};
        let m = build_helical_ramp(&HelicalRampParams { segments_per_turn: 48, ..Default::default() }).unwrap();
        let part_ids = m.face_ids.clone();
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset(
            "ramp".into(),
            crate::mesh_io::ObjMesh { positions: m.positions, normals: m.normals, color: None, alpha: Vec::new() },
        );
        assert_eq!(part_ids.len(), st.furniture_lib[idx].positions.len() / 3, "one part id per triangle");
        st.furniture_lib[idx].part_ids = part_ids;
        st.place_furniture(idx, Vec3::ZERO);
        assert!(st.furniture_has_parts(0), "the placed ramp carries part ids");

        let g = st.furniture_lib[idx].group_geom();
        // Many pieces overall — the deck is ONE, but each rail and post is its own piece.
        let bodies = g.body.iter().copied().max().unwrap() + 1;
        assert!(bodies > 20, "rails + posts are many separate pieces, got {bodies}");
    }

    /// Painting a face is AUTHORITATIVE: it drops any whole-object texture so the rest of the
    /// object doesn't stay masked (the "texture went on the whole object" bug).
    #[test]
    fn apply_face_texture_clears_whole_object_texture() {
        let mut st = FactoryState::default();
        let (pos, nrm) = box_soup([0.0, 0.0, 0.0]);
        let idx = st.add_furniture_asset(
            "box".into(),
            crate::mesh_io::ObjMesh { positions: pos, normals: nrm, color: None, alpha: Vec::new() },
        );
        st.place_furniture(idx, Vec3::ZERO);
        let ti = st.add_texture("img".into(), 1, 1, vec![255; 4]);
        // Start whole-object textured (as if applied in Whole-object mode first).
        st.furniture[0].texture = Some(ti);
        st.furniture[0].color = [0.3, 0.2, 0.1]; // a baked tint

        let groups = st.furniture_lib[idx].group_geom();
        let fg0 = groups.face[0];
        st.apply_face_texture(0, &[fg0], ti);

        assert_eq!(st.furniture[0].texture, None, "whole-object texture dropped");
        assert_eq!(st.furniture[0].surface_texture.get(&fg0).copied(), Some(ti), "face textured");
        // Only that one face is textured now; the rest are flat (no whole-object mask).
        let fac = st.furniture_faceted(0).expect("faceted");
        assert_eq!(fac.opaque.len(), 1);
        assert_eq!(fac.opaque[0].2.len(), 6, "just the clicked face");
        assert!(fac.flat.is_some(), "the rest is flat, not whole-object textured");
    }

    /// Tuning a PIECE's opacity/reflection must not touch the rest of the object: the piece gets
    /// its OWN material (copy-on-write) so the whole-object texture is left intact. Regression for
    /// "opacity/reflection get applied to the whole object instead of the selected piece".
    #[test]
    fn private_piece_material_isolates_from_the_whole_object() {
        let mut st = FactoryState::default();
        let (pos, nrm) = box_soup([0.0, 0.0, 0.0]);
        let idx = st.add_furniture_asset(
            "box".into(),
            crate::mesh_io::ObjMesh { positions: pos, normals: nrm, color: None, alpha: Vec::new() },
        );
        st.place_furniture(idx, Vec3::ZERO);
        let ti = st.add_texture("img".into(), 1, 1, vec![255; 4]);
        st.furniture[0].texture = Some(ti); // whole object textured with `ti`

        let groups = st.furniture_lib[idx].group_geom();
        let fg0 = groups.face[0];

        // Tuning a piece seeded from the whole-object texture must CLONE (ti is used outside).
        let piece_ti = st.private_piece_material(0, &[fg0]);
        assert_ne!(piece_ti, ti, "the piece got its OWN copy, not the shared whole-object texture");
        assert_eq!(st.furniture[0].surface_texture.get(&fg0).copied(), Some(piece_ti), "piece bound to its copy");
        assert_eq!(st.furniture[0].texture, Some(ti), "the whole-object texture is left intact");

        // Changing the piece's opacity leaves the whole-object texture's opacity unchanged.
        st.textures[piece_ti].opacity = 0.3;
        assert_eq!(st.textures[ti].opacity, 1.0, "whole-object opacity untouched");

        // Idempotent: the piece already owns an exclusive material → no further clone.
        let n = st.textures.len();
        let again = st.private_piece_material(0, &[fg0]);
        assert_eq!(again, piece_ti, "reuses the piece's own material");
        assert_eq!(st.textures.len(), n, "no extra texture minted");
    }

    /// Tuning ONE CSG feature-solid's opacity/reflection must isolate to it (copy-on-write), and a
    /// see-through feature routes its verts to the blended pass carrying the texture's opacity.
    /// Regression for "opacity/reflection applied to the whole stair instead of the selected solid".
    #[test]
    fn private_feature_material_isolates_and_routes_transparency() {
        let mut st = FactoryState::default();
        // Two boxes → two features sharing one texture (as if the whole stair were textured at once).
        let a = st.model.push(BoolOp::Union, Plane::default(), Placement::default(), Primitive::Box { w: 1.0, d: 1.0, h: 1.0 });
        let b = st.model.push(BoolOp::Union, Plane::default(), Placement { u: 3.0, ..Default::default() }, Primitive::Box { w: 1.0, d: 1.0, h: 1.0 });
        st.recompute();
        let ti = st.add_texture("shared".into(), 1, 1, vec![200, 200, 210, 255]);
        st.feature_texture.insert(a, ti);
        st.feature_texture.insert(b, ti);
        st.recompute();

        // Tuning solid `a` alone must clone (ti is shared with `b`) and rebind only `a`.
        let pa = st.private_feature_material(&[a]);
        assert_ne!(pa, ti, "solid A got its own copy");
        assert_eq!(st.feature_texture.get(&a).copied(), Some(pa), "A rebound to its copy");
        assert_eq!(st.feature_texture.get(&b).copied(), Some(ti), "B still on the shared texture");

        // Make A see-through; B stays opaque. The textured-feature verts split by opacity.
        st.textures[pa].opacity = 0.3;
        assert_eq!(st.textures[ti].opacity, 1.0, "B's texture opacity untouched");
        let groups = st.feature_textured_meshes();
        let a_verts = groups.iter().find(|(t, _)| *t == pa).map(|(_, v)| v).expect("A group");
        let b_verts = groups.iter().find(|(t, _)| *t == ti).map(|(_, v)| v).expect("B group");
        assert!(a_verts.iter().all(|v| (v.a - 0.3).abs() < 1e-3), "A carries its opacity → blended pass");
        assert!(b_verts.iter().all(|v| v.a >= ALPHA_OPAQUE), "B stays opaque");
    }

    /// A TEXTURED piece with glass splits between the opaque image pass and the blended textured
    /// pass: opaque faces keep their texture, glass faces carry per-vertex opacity and a distinct
    /// buffer key. This is the textured-glass path (e.g. a glTF window whose frame is textured).
    #[test]
    fn textured_glass_splits_into_opaque_and_blended() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset(
            "win".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![
                    [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], // frame (opaque)
                    [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0], // glass (0.25)
                ],
                normals: vec![[0.0, 0.0, 1.0]; 6],
                color: None,
                alpha: vec![1.0, 1.0, 1.0, 0.25, 0.25, 0.25],
            },
        );
        st.place_furniture(idx, Vec3::new(0.0, 0.0, 0.0));
        let ti = st.add_texture("frame".into(), 2, 2, vec![255; 16]);
        st.furniture[0].texture = Some(ti);

        let (_ti1, opaque_key, opaque) = st.furniture_textured_mesh(0).expect("opaque textured tris");
        assert_eq!(opaque.len(), 3, "only the frame triangle is opaque");
        assert!(opaque.iter().all(|v| v.a >= ALPHA_OPAQUE), "opaque verts carry full opacity");

        let (_ti2, glass_key, glass) = st.furniture_textured_translucent_mesh(0).expect("glass tris");
        assert_eq!(glass.len(), 3, "only the glass triangle is translucent");
        assert!(glass.iter().all(|v| (v.a - 0.25).abs() < 1e-6), "glass carries its opacity");
        assert_ne!(opaque_key, glass_key, "opaque and glass use distinct GPU buffers");
    }

    /// Texturing a FEATURE moves its triangles out of the flat batch and into the textured
    /// pass (same triangle count), so a wall/floor shows the image instead of a flat colour.
    #[test]
    fn feature_texture_moves_tris_to_the_textured_pass() {
        let mut st = FactoryState::default();
        st.model.push(
            cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement::default(), Primitive::Box { w: 2.0, d: 2.0, h: 2.0 },
        );
        st.recompute();
        let id = st.model.features.last().unwrap().id;
        let flat_before = st.scene_verts().len();
        assert!(flat_before > 0);

        let ti = st.add_texture("img".into(), 2, 2, vec![255; 16]);
        st.feature_texture.insert(id, ti);

        let flat_after = st.scene_verts().len();
        assert!(flat_after < flat_before, "textured feature leaves the flat batch");

        let groups = st.feature_textured_meshes();
        assert_eq!(groups.len(), 1, "one texture group");
        let (gi, verts) = &groups[0];
        assert_eq!(*gi, ti);
        assert_eq!(verts.len(), flat_before - flat_after, "moved tris == removed tris");
        assert!(verts.iter().all(|v| v.s > 0.0 && v.s <= 1.0), "shade in (0,1]");
    }

    /// Textures + their assignments + tiling survive a sidecar round-trip (PNG/base64), and a
    /// 2×2 image decodes back to the same pixels.
    #[test]
    fn textures_persist_through_the_sidecar() {
        let mut st = FactoryState::default();
        st.model.push(
            cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement::default(), Primitive::Box { w: 1.0, d: 1.0, h: 1.0 },
        );
        st.recompute();
        let id = st.model.features.last().unwrap().id;
        // A distinct 2×2 RGBA image so a decode error would be caught.
        let px: Vec<u8> = vec![
            255, 0, 0, 255,   0, 255, 0, 255,
            0, 0, 255, 255,   255, 255, 0, 255,
        ];
        let ti = st.add_texture("img".into(), 2, 2, px.clone());
        st.textures[ti].scale = 3.5;
        st.feature_texture.insert(id, ti);

        let doc = st.to_persist();
        let mut st2 = FactoryState::default();
        st2.apply_persist(doc);

        assert_eq!(st2.textures.len(), 1, "texture restored");
        assert_eq!(st2.textures[0].w, 2);
        assert_eq!(st2.textures[0].rgba, px, "pixels round-trip exactly");
        assert!((st2.textures[0].scale - 3.5).abs() < 1e-4, "tiling restored");
        assert_eq!(st2.feature_texture.get(&id).copied(), Some(0), "assignment restored");
    }

    /// A UNIFORM environment of radiance `l` as SH coefficients — only the `l = 0` band is
    /// non-zero, and `sh_ambient` then returns `l` for every normal. Handy for tests that want a
    /// controlled ambient rather than a real sky.
    fn flat_sh(l: f32) -> [[f32; 3]; 9] {
        let mut sh = [[0.0f32; 3]; 9];
        for c in 0..3 {
            sh[0][c] = l * std::f32::consts::PI / 0.886_227;
        }
        sh
    }

    /// The sun light drives the CPU shading: enabling it makes a surface facing the sun brighter
    /// than one facing away, and disabling it restores the fixed two-sided studio key light.
    #[test]
    fn sun_light_drives_directional_shading() {
        use glam::Vec3;
        // Disabled → the fixed two-sided key light (front and back shade equally under |n·dir|).
        set_sun_light(false, Vec3::new(0.0, 0.0, 1.0), [1.0, 1.0, 1.0], flat_sh(0.3));
        let up = shade([1.0, 1.0, 1.0], Vec3::Z);
        let down = shade([1.0, 1.0, 1.0], -Vec3::Z);
        assert!((up.col[0] - down.col[0]).abs() < 1e-6, "studio light is two-sided");

        // Enabled, sun straight up → an up-facing surface is lit, a down-facing one gets only ambient.
        set_sun_light(true, Vec3::new(0.0, 0.0, 1.0), [1.0, 0.95, 0.85], flat_sh(0.25));
        let lit = shade([1.0, 1.0, 1.0], Vec3::Z);
        let shadow = shade([1.0, 1.0, 1.0], -Vec3::Z);
        assert!(lit.col[0] > shadow.col[0] + 0.3, "sun-facing brighter than shadow side: {lit:?} vs {shadow:?}");
        // Reset so other tests see the default.
        set_sun_light(false, Vec3::new(0.35, 0.25, 0.9), [1.0, 0.96, 0.88], flat_sh(0.32));
    }

    /// The ambient SHARE is what ambient occlusion is allowed to darken, so it has to track where
    /// the light actually came from: everything on a face turned away from the sun, and only a
    /// minority on one facing it. Getting this backwards would darken sunlit creases and leave
    /// shadowed ones bright — plausible-looking, and exactly wrong.
    #[test]
    fn the_ambient_share_tracks_where_the_light_came_from() {
        use glam::Vec3;
        set_sun_light(true, Vec3::Z, [2.0, 2.0, 2.0], flat_sh(0.3));
        let lit = shade([0.8; 3], Vec3::Z);
        let away = shade([0.8; 3], -Vec3::Z);
        assert!(away.amb > 0.99, "a surface facing away from the sun is lit purely by the sky: {away:?}");
        assert!(lit.amb < 0.2, "a surface facing a 2.0 sun against a 0.3 sky is mostly direct: {lit:?}");
        // The split must be lossless: ambient + direct is the colour, whatever the share.
        assert!(lit.col[0] > away.col[0], "the sunlit face is still the brighter one");
        // Studio mode: the fixed key is 0.35 fill + 0.65 directional, so a face square to it is
        // 0.35/1.0 ambient and one edge-on is all ambient.
        set_sun_light(false, Vec3::Z, [1.0; 3], flat_sh(0.0));
        let d = Vec3::new(0.35, 0.25, 0.9).normalize();
        assert!((shade([0.8; 3], d).amb - 0.35).abs() < 0.02, "{:?}", shade([0.8; 3], d));
        let edge = d.cross(Vec3::Z).normalize();
        assert!(shade([0.8; 3], edge).amb > 0.99, "edge-on to the key light is pure fill");
    }

    /// Clay mode overrides the shaded base colour to neutral grey (same for any input colour),
    /// and turning it off restores the real colour.
    #[test]
    fn clay_mode_flattens_the_shade() {
        use glam::Vec3;
        set_sun_light(false, Vec3::Z, [1.0; 3], flat_sh(0.3));
        set_clay(true);
        let red = shade([0.9, 0.1, 0.1], Vec3::Z);
        let blue = shade([0.1, 0.1, 0.9], Vec3::Z);
        assert!((red.col[0] - blue.col[0]).abs() < 1e-6 && (red.col[2] - blue.col[2]).abs() < 1e-6, "clay greys any colour the same");
        set_clay(false);
        let red2 = shade([0.9, 0.1, 0.1], Vec3::Z);
        assert!(red2.col[0] > red2.col[2] + 0.2, "colour restored when clay off");
    }

    /// The baked colour must be **scene-referred linear** and must NOT be tone-mapped: Phase 1
    /// moved the display transform to the composite, so a `1 − e⁻ˣ` here would be the second one in
    /// series. The tell is that the output stays proportional to the light instead of saturating.
    #[test]
    fn the_flat_path_stays_linear_and_untonemapped() {
        use glam::Vec3;
        // White albedo, sun straight up, no sky: the colour IS the sun radiance.
        set_sun_light(true, Vec3::Z, [1.0; 3], flat_sh(0.0));
        let a = shade([1.0; 3], Vec3::Z).col[0];
        set_sun_light(true, Vec3::Z, [4.0; 3], flat_sh(0.0));
        let b = shade([1.0; 3], Vec3::Z).col[0];
        assert!(b > 3.0, "4x the light must give ~4x the value, not a saturated one: {a} -> {b}");
        assert!((b / a - 4.0).abs() < 0.01, "and exactly 4x: {a} -> {b}");
        // Albedo is decoded from sRGB, so a 0.5 swatch contributes its LINEAR 0.214.
        set_sun_light(true, Vec3::Z, [1.0; 3], flat_sh(0.0));
        let half = shade([0.5; 3], Vec3::Z).col[0];
        assert!((half - crate::color::srgb_to_linear(0.5)).abs() < 1e-4, "albedo must be decoded: {half}");
        set_sun_light(false, Vec3::new(0.35, 0.25, 0.9), [1.0, 0.96, 0.88], flat_sh(0.32));
    }

    /// [`SunEnv::resolve`] returns a normalized daytime sun direction and a dimmer light at night.
    #[test]
    fn sun_env_resolves_day_and_night() {
        let noon = SunEnv { enabled: true, hour: 13.0, ..Default::default() };
        let (en, dir, sun, _sky, _gnd) = noon.resolve();
        assert!(en && dir.z > 0.5, "midday sun is high");
        assert!((dir.length() - 1.0).abs() < 1e-3, "unit direction");
        let night = SunEnv { enabled: true, hour: 1.0, ..Default::default() };
        let (_e, ndir, nsun, _s, _g) = night.resolve();
        assert!(ndir.z < 0.0, "sun below horizon at 1 am");
        assert!(nsun[0] < sun[0], "night direct light dimmer than noon");
    }

    /// Every library preset mints a usable material: unique (category, name), fields carried onto
    /// the texture asset, glass translucent, metals metallic, emitters emitting.
    #[test]
    fn material_presets_build_into_assets() {
        let presets = material_presets();
        assert!(presets.len() >= 25, "a real library, not a stub: {}", presets.len());
        let mut seen = std::collections::HashSet::new();
        let mut st = FactoryState::default();
        for p in &presets {
            assert!(seen.insert((p.category, p.name)), "duplicate preset {}/{}", p.category, p.name);
            let idx = st.add_preset_material(p);
            let t = &st.textures[idx];
            assert_eq!(t.name, p.name);
            assert!((t.metallic - p.metallic).abs() < 1e-6 && (t.roughness - p.roughness).abs() < 1e-6);
            assert!(t.proc.is_some(), "every preset is procedural/solid (live-uniform driven)");
        }
        // Spot checks: glass see-through, chrome metallic-glossy, LED emits.
        let glass = presets.iter().find(|p| p.name == "Clear glass").unwrap();
        assert!(glass.opacity < 0.5);
        let chrome = presets.iter().find(|p| p.name == "Chrome").unwrap();
        assert!(chrome.metallic > 0.99 && chrome.roughness < 0.1);
        let led = presets.iter().find(|p| p.name == "LED warm").unwrap();
        assert!(led.emission_strength > 1.0);
    }

    /// PBR maps + roughness survive the sidecar round-trip and `has_pbr` reflects them.
    #[test]
    fn pbr_maps_persist() {
        let mut st = FactoryState::default();
        let base = st.add_texture("albedo".into(), 1, 1, vec![200, 180, 160, 255]);
        let nrm = st.add_texture("normal".into(), 1, 1, vec![128, 128, 255, 255]);
        st.textures[base].normal_map = Some(nrm);
        st.textures[base].roughness = 0.2;
        assert!(st.textures[base].has_pbr());

        let doc = st.to_persist();
        let mut st2 = FactoryState::default();
        st2.apply_persist(doc);
        assert_eq!(st2.textures[base].normal_map, Some(nrm), "normal map index restored");
        assert!((st2.textures[base].roughness - 0.2).abs() < 1e-4, "roughness restored");
    }

    /// A PROCEDURAL texture (wood grain) survives the sidecar round-trip — pattern + colours + grain
    /// scale come back, and its 1×1 fallback swatch reflects the ramp midpoint.
    #[test]
    fn procedural_texture_persists() {
        let mut st = FactoryState::default();
        let def = ProcDef::oak();
        let ti = st.add_procedural_texture("oak".into(), def);
        assert!(st.textures[ti].proc.is_some(), "created as procedural");
        assert_eq!(st.textures[ti].w, 1, "carries a 1×1 fallback swatch");
        assert_eq!(st.textures[ti].proc.unwrap().pattern.mode(), 1, "wood mode");

        let doc = st.to_persist();
        let mut st2 = FactoryState::default();
        st2.apply_persist(doc);

        let p = st2.textures[0].proc.expect("procedural restored");
        assert_eq!(p.pattern, ProcPattern::Wood);
        assert_eq!(p.col_a, def.col_a);
        assert_eq!(p.col_b, def.col_b);
        assert_eq!(p.scale, def.scale);
        assert!((p.detail - def.detail).abs() < 1e-4 && (p.contrast - def.contrast).abs() < 1e-4);
    }

    /// A furniture instance's PER-FACE textures survive the sidecar round-trip; a dangling texture
    /// index is dropped on load.
    #[test]
    fn per_face_furniture_textures_persist() {
        let mut st = FactoryState::default();
        let (pos, nrm) = box_soup([0.0, 0.0, 0.0]);
        let idx = st.add_furniture_asset(
            "box".into(),
            crate::mesh_io::ObjMesh { positions: pos, normals: nrm, color: None, alpha: Vec::new() },
        );
        st.place_furniture(idx, Vec3::ZERO);
        let ti = st.add_texture("img".into(), 1, 1, vec![255; 4]);
        st.furniture[0].surface_texture.insert(3, ti); // face-group 3 → texture 0
        st.furniture[0].surface_texture.insert(9, ti);

        let doc = st.to_persist();
        let mut st2 = FactoryState::default();
        st2.apply_persist(doc);
        let sm = &st2.furniture[0].surface_texture;
        assert_eq!(sm.len(), 2, "both per-face assignments restored");
        assert_eq!(sm.get(&3).copied(), Some(0));
        assert_eq!(sm.get(&9).copied(), Some(0));
    }

    /// Texture MOVE (offset) + ROTATE persist through the sidecar alongside tiling.
    #[test]
    fn texture_move_and_rotate_persist() {
        let mut st = FactoryState::default();
        let ti = st.add_texture("t".into(), 2, 2, vec![200; 16]);
        st.textures[ti].scale = 2.0;
        st.textures[ti].offset = [0.25, -0.5];
        st.textures[ti].rot_deg = 90.0;
        let doc = st.to_persist();
        let mut st2 = FactoryState::default();
        st2.apply_persist(doc);
        let t = &st2.textures[0];
        assert!((t.scale - 2.0).abs() < 1e-4, "tiling restored");
        assert!((t.offset[0] - 0.25).abs() < 1e-4 && (t.offset[1] + 0.5).abs() < 1e-4, "move restored");
        assert!((t.rot_deg - 90.0).abs() < 1e-3, "rotation restored");
    }

    /// `map_uv` — the one place tiling/move/rotate is applied. Identity at defaults; tiling scales
    /// about the origin; offset shifts; a 90° rotation about the tile centre maps right→top.
    #[test]
    fn map_uv_applies_tiling_move_rotate() {
        let mut t = TextureAsset::new("t".into(), 2, 2, vec![255; 16]);
        assert_eq!(t.map_uv(0.3, 0.7), [0.3, 0.7], "identity at defaults");
        t.scale = 2.0;
        let uv = t.map_uv(1.0, 0.0);
        assert!((uv[0] - 2.0).abs() < 1e-4 && uv[1].abs() < 1e-4, "tiling ×2: {uv:?}");
        t.scale = 1.0;
        t.offset = [0.1, 0.2];
        let uv = t.map_uv(0.3, 0.7);
        assert!((uv[0] - 0.4).abs() < 1e-4 && (uv[1] - 0.9).abs() < 1e-4, "offset shifts: {uv:?}");
        t.offset = [0.0, 0.0];
        t.rot_deg = 90.0;
        let uv = t.map_uv(1.0, 0.5); // right-centre → top-centre under a 90° spin about (0.5,0.5)
        assert!((uv[0] - 0.5).abs() < 1e-3 && (uv[1] - 1.0).abs() < 1e-3, "90° rotate: {uv:?}");
    }

    /// A per-SURFACE texture moves only that face's triangles out of the flat batch (the other
    /// faces stay flat), and they show up in the textured pass under that texture.
    #[test]
    fn per_surface_texture_moves_only_that_face() {
        let mut st = FactoryState::default();
        st.model.push(
            cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement::default(), Primitive::Box { w: 1.0, d: 1.0, h: 1.0 },
        );
        st.recompute();
        let flat_before = st.scene_verts().len();
        assert!(flat_before > 12, "a box has several faces of triangles");

        let ti = st.add_texture("img".into(), 2, 2, vec![255; 16]);
        // Texture just the first triangle's surface (its coplanar face).
        let fid = st.cached.face_ids[0];
        let p = &st.cached.positions;
        let key = surface_key(fid, p[0], p[1], p[2]);
        st.surface_texture.insert(key, ti);

        let flat_after = st.scene_verts().len();
        assert!(flat_after < flat_before, "the painted face leaves the flat batch");
        assert!(flat_after > 0, "the OTHER faces stay flat-shaded");

        let groups = st.feature_textured_meshes();
        assert_eq!(groups.len(), 1, "one texture group");
        assert_eq!(groups[0].0, ti);
        assert_eq!(groups[0].1.len(), flat_before - flat_after, "moved tris == removed tris");

        // Round-trips through the sidecar.
        let doc = st.to_persist();
        let mut st2 = FactoryState::default();
        st2.apply_persist(doc);
        assert_eq!(st2.surface_texture.get(&key).copied(), Some(0), "surface texture restored");
    }
}

#[cfg(test)]
mod draw3d_edit_tests {
    use super::*;

    /// EDIT-MODE invariant (owner, 2026-07-17: "if one 3d dobject selected, with these
    /// controllers we should be able to change its dimension"). Selecting a solid loads
    /// it into the dialog via `load_from`; editing then rebuilds via `build`. If the two
    /// are not inverses, tweaking one field would silently corrupt the others. This
    /// proves `load_from → build` reproduces the primitive for every shape (compared by
    /// Debug, since `Primitive` isn't `PartialEq`). The Frustum family is the tricky one:
    /// cone / prism / pyramid all share one variant but different controllers.
    #[test]
    fn load_from_then_build_round_trips() {
        let cases = [
            Primitive::Box { w: 3.0, d: 4.0, h: 2.5 },
            Primitive::Cylinder { r: 1.2, h: 5.0, sides: 20 },
            Primitive::Sphere { r: 2.0, segments: 40, stacks: 18 },
            Primitive::Frustum { r_bottom: 2.0, r_top: 0.0, h: 3.0, sides: 24 }, // cone
            Primitive::Frustum { r_bottom: 1.5, r_top: 1.5, h: 2.0, sides: 6 },  // prism
            Primitive::Frustum { r_bottom: 2.0, r_top: 0.0, h: 3.0, sides: 4 },  // pyramid
            Primitive::Torus { major_r: 3.0, minor_r: 0.8, seg_major: 36, seg_minor: 18 },
            Primitive::Capsule { r: 0.7, h: 2.0, segments: 24, stacks: 12 },
            Primitive::Tube { r_outer: 2.0, r_inner: 1.0, h: 3.0, sides: 28 },
            Primitive::Ellipsoid { rx: 1.0, ry: 2.0, rz: 0.5, segments: 32, stacks: 16 },
        ];
        for p in cases {
            let mut dlg = Draw3dDialog::new(Draw3dKind::Box);
            dlg.load_from(&p);
            let rebuilt = dlg.build();
            assert_eq!(
                format!("{rebuilt:?}"), format!("{p:?}"),
                "load_from → build must reproduce the primitive"
            );
        }
    }
}

#[cfg(test)]
mod wall_tests {
    use super::*;

    /// 2D→3D wall promotion (owner, 2026-07-17): a centerline segment → ONE wall solid,
    /// a Box of length × thickness × height placed at the midpoint, spun along the run.
    #[test]
    fn wall_segment_is_a_placed_box() {
        let mut st = FactoryState::default();
        st.add_wall_segment(Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), 0.3, 2.5); // 4 m along +X
        assert_eq!(st.model.features.len(), 1, "one segment → one solid");

        let f = &st.model.features[0];
        match f.primitive {
            Primitive::Box { w, d, h } => {
                assert!((w - 4.0).abs() < 1e-4, "length spans the centerline");
                assert!((d - 0.3).abs() < 1e-4, "depth = the wall's own thickness");
                assert!((h - 2.5).abs() < 1e-4, "height = the 3D wall height");
            }
            other => panic!("wall segment must be a Box, got {other:?}"),
        }
        assert!((f.placement.u - 2.0).abs() < 1e-4, "placed at the midpoint u");
        assert!(f.placement.v.abs() < 1e-4, "placed at the midpoint v");
        assert!(f.placement.spin_deg.abs() < 1e-4, "run along +X → spin 0°");
    }

    /// Orientation: a +Y run spins 90°; degenerate input is ignored.
    #[test]
    fn wall_segment_orientation_and_degenerate_guard() {
        let mut st = FactoryState::default();
        st.add_wall_segment(Vec2::new(0.0, 0.0), Vec2::new(0.0, 3.0), 0.2, 2.7); // +Y
        assert_eq!(st.model.features.len(), 1);
        assert!((st.model.features[0].placement.spin_deg - 90.0).abs() < 1e-3, "+Y run → spin 90°");

        st.add_wall_segment(Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0), 0.2, 2.7); // zero length
        st.add_wall_segment(Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), 0.0, 2.7); // zero thickness
        assert_eq!(st.model.features.len(), 1, "degenerate segments are ignored");
    }

    /// Walls stay ALIVE (owner, 2026-07-17): a promotion records a live wall whose height
    /// re-derives the Box on the fly, keeping its length and thickness.
    #[test]
    fn wall_stays_alive_height_re_derives() {
        let mut st = FactoryState::default();
        st.add_wall_segment(Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), 0.3, 2.5);
        assert_eq!(st.walls.len(), 1, "promotion records a live wall");
        let fid = st.walls[0].segments[0];

        st.set_wall_height(fid, 3.2);
        assert!((st.walls[0].height - 3.2).abs() < 1e-4, "registry height updated");
        match st.model.get_mut(fid).unwrap().primitive {
            Primitive::Box { w, d, h } => {
                assert!((h - 3.2).abs() < 1e-4, "box height re-derived");
                assert!((w - 4.0).abs() < 1e-4 && (d - 0.3).abs() < 1e-4, "length & thickness kept");
            }
            _ => panic!("a wall is a Box"),
        }
        st.clear();
        assert!(st.walls.is_empty(), "clear drops the live-wall records too");
    }

    /// Footprint editing (owner, 2026-07-22): a wall is driven by ONE ground-plane
    /// footprint, so N points → N−1 Box segments, and adding/moving/deleting a vertex
    /// re-derives. The new corner is on BOTH rings by construction: every segment Box
    /// rises the full height from z=0, so the vertex exists at the floor AND the ceiling.
    #[test]
    fn footprint_wall_add_vertex_couples_rings_and_reshapes() {
        let mut st = FactoryState::default();
        // An L-shaped footprint: (0,0)-(4,0)-(4,3) → 2 segments.
        let wi = st
            .add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 3.0)], 0.3, 2.7)
            .expect("L footprint promotes");
        assert_eq!(st.walls[wi].footprint.len(), 3);
        assert_eq!(st.walls[wi].segments.len(), 2, "N points → N−1 segments");
        assert_eq!(st.model.features.len(), 2);

        // Add a corner mid first edge, at (2,0): 4 points / 3 segments.
        let vi = st.wall_insert_vertex(wi, 0, Vec2::new(2.0, 0.0)).expect("split edge 0");
        assert_eq!(vi, 1);
        assert_eq!(st.walls[wi].footprint.len(), 4);
        assert_eq!(st.walls[wi].segments.len(), 3, "add vertex → +1 segment");

        // Both rings share the footprint: EVERY segment Box rises the full height from the
        // ground, so the new corner is present on both the floor (z=0) and ceiling (z=h).
        for &fid in &st.walls[wi].segments {
            match st.model.get_mut(fid).expect("segment feature").primitive {
                Primitive::Box { h, .. } => {
                    assert!((h - 2.7).abs() < 1e-4, "segment rises full height → vertex on floor & ceiling")
                }
                _ => panic!("a wall segment must be a Box"),
            }
        }

        // Drag the corner → the surface shifts; still 3 segments.
        st.wall_move_vertex(wi, 1, Vec2::new(2.0, 1.0));
        assert_eq!(st.walls[wi].segments.len(), 3);
        assert!((st.walls[wi].footprint[1] - Vec2::new(2.0, 1.0)).length() < 1e-6, "vertex moved");

        // Delete the corner → back to 3 points / 2 segments.
        assert!(st.wall_delete_vertex(wi, 1), "delete a corner");
        assert_eq!(st.walls[wi].footprint.len(), 3);
        assert_eq!(st.walls[wi].segments.len(), 2);
        // Delete down to the 2-point minimum (one segment), then reject any further delete.
        assert!(st.wall_delete_vertex(wi, 0), "delete down to a single segment");
        assert_eq!(st.walls[wi].footprint.len(), 2);
        assert_eq!(st.walls[wi].segments.len(), 1);
        assert!(!st.wall_delete_vertex(wi, 0), "a wall never drops below 2 points");
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::*;

    /// Zoom-window (owner, 2026-07-17: "we need zoom as it is in 2d"): a CENTERED box keeps
    /// the target where it is and dollies in by the box/viewport height ratio.
    #[test]
    fn zoom_window_centered_box_keeps_target_and_dollies_in() {
        let mut st = FactoryState::default();
        st.cam_target = [5.0, 5.0, 0.0];
        st.cam_dist = 20.0;
        let vp = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let c = vp.center();
        // a centered box, half the viewport height (300 px) → target unchanged, dist halved
        st.zoom_window(vp, egui::pos2(c.x - 100.0, c.y - 150.0), egui::pos2(c.x + 100.0, c.y + 150.0));
        assert!((st.cam_target[0] - 5.0).abs() < 1e-3 && (st.cam_target[1] - 5.0).abs() < 1e-3,
                "a centered box keeps the target");
        assert!((st.cam_dist - 10.0).abs() < 1e-2, "a half-height box halves the distance");
    }

    /// An off-centre box shifts the target toward it (here: box to the RIGHT of centre).
    #[test]
    fn zoom_window_offcentre_box_shifts_target() {
        let mut st = FactoryState::default();
        st.cam_dist = 20.0; // Iso-ish default yaw/pitch
        let vp = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let c = vp.center();
        let before = st.cam_target;
        st.zoom_window(vp, egui::pos2(c.x + 100.0, c.y - 50.0), egui::pos2(c.x + 300.0, c.y + 50.0));
        let moved = (st.cam_target[0] - before[0]).abs()
            + (st.cam_target[1] - before[1]).abs()
            + (st.cam_target[2] - before[2]).abs();
        assert!(moved > 1e-3, "an off-centre window must move the target");
    }

    /// `zoom previous` restores the camera saved before the last zoom.
    #[test]
    fn zoom_previous_restores_the_pre_zoom_camera() {
        let mut st = FactoryState::default();
        st.cam_dist = 20.0;
        st.cam_target = [1.0, 2.0, 3.0];
        st.zoom_save_prev();
        st.cam_dist = 5.0;
        st.cam_target = [9.0, 9.0, 9.0];
        st.zoom_restore_previous();
        assert!((st.cam_dist - 20.0).abs() < 1e-4, "distance restored");
        assert!((st.cam_target[0] - 1.0).abs() < 1e-4 && (st.cam_target[2] - 3.0).abs() < 1e-4,
                "target restored");
        // a second restore is a no-op (the snapshot was consumed)
        st.zoom_restore_previous();
        assert!((st.cam_dist - 20.0).abs() < 1e-4, "second restore is harmless");
    }
}

/// Cutting a furniture piece: the app-level half of `cad_solid::meshcut`, where a cut becomes an
/// EDITABLE property of an instance rather than a one-off boolean.
#[cfg(test)]
mod furniture_cuts {
    use super::*;

    /// A real parametric door, placed. Everything here is a generated piece, which is the scope
    /// cutting is offered for.
    fn placed_door() -> (FactoryState, usize) {
        let mut st = FactoryState::default();
        let (_m, mesh) = cad_solid::door::build(&cad_solid::door::DoorInput::default()).unwrap();
        let part_ids = mesh.face_ids.clone();
        let obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions,
            normals: mesh.normals,
            color: Some([0.62, 0.50, 0.38]),
            alpha: Vec::new(),
        };
        let a = st.add_furniture_asset("Door".into(), obj);
        if let Some(asset) = st.furniture_lib.get_mut(a) {
            asset.part_ids = part_ids;
        }
        st.place_furniture(a, Vec3::ZERO);
        (st, 0)
    }

    /// A 150 mm square on the piece's front-most +Y face, built the way a face PICK builds one:
    /// from a real triangle's centroid and its real normal. Inventing a plausible-looking frame
    /// instead would test the cut against a surface that is not quite there.
    fn slot(st: &FactoryState, fi: usize) -> (Frame, Vec<Vec<Vec2>>) {
        let inst = &st.furniture[fi];
        let asset = &st.furniture_lib[inst.asset];
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for tri in asset.positions.chunks_exact(3) {
            let a = st.furniture_point(inst, tri[0]);
            let b = st.furniture_point(inst, tri[1]);
            let c = st.furniture_point(inst, tri[2]);
            let n = (b - a).cross(c - a).normalize_or_zero();
            if n.dot(Vec3::Y) < 0.99 {
                continue; // want a face looking along +Y
            }
            let mid = (a + b + c) / 3.0;
            // The FRONT-most such face, so the cut starts outside the piece.
            if best.map_or(true, |(y, _, _)| mid.y > y) {
                best = Some((mid.y, mid, n));
            }
        }
        let (_, p, n) = best.expect("the piece has a +Y face");
        let r = 0.02;
        (
            Frame::from_point_normal(p, n),
            vec![vec![Vec2::new(-r, -r), Vec2::new(r, -r), Vec2::new(r, r), Vec2::new(-r, r)]],
        )
    }

    fn tris(st: &FactoryState, fi: usize) -> usize {
        st.furniture_lib[st.furniture[fi].asset].positions.len() / 3
    }

    /// THE round trip that makes the list editable: cut, and the piece changes; disable, and it is
    /// byte-for-byte what it was; re-enable, and the cut is back. Nothing is ever baked.
    #[test]
    fn a_cut_can_be_switched_off_and_the_piece_returns_exactly() {
        let (mut st, fi) = placed_door();
        let original: Vec<[f32; 3]> = st.furniture_lib[st.furniture[fi].asset].positions.clone();
        let (frame, loops) = slot(&st, fi);

        let made = st.add_furniture_cut(fi, &frame, &loops, true, 0.0).expect("the door cuts");
        assert_eq!(made, 1, "one loop, one cut");
        assert_eq!(st.furniture[fi].cuts.len(), 1);
        assert!(st.furniture[fi].base_asset.is_some(), "the instance now points at a derived copy");
        assert_ne!(tris(&st, fi), original.len() / 3, "the geometry changed");

        st.furniture[fi].cuts[0].enabled = false;
        st.rebuild_cut_asset(fi).unwrap();
        assert_eq!(st.furniture[fi].base_asset, None, "back to the original asset itself");
        assert_eq!(
            st.furniture_lib[st.furniture[fi].asset].positions, original,
            "and to the original geometry, exactly"
        );

        st.furniture[fi].cuts[0].enabled = true;
        st.rebuild_cut_asset(fi).unwrap();
        assert_ne!(st.furniture_lib[st.furniture[fi].asset].positions, original, "the cut is back");
    }

    /// Cutting one piece must not touch another placed from the same library entry. Three doors,
    /// one cut: the other two are untouched.
    #[test]
    fn cutting_one_instance_leaves_its_siblings_alone() {
        let (mut st, fi) = placed_door();
        let a = st.furniture[fi].asset;
        st.place_furniture(a, Vec3::new(2.0, 0.0, 0.0));
        st.place_furniture(a, Vec3::new(4.0, 0.0, 0.0));
        let before = tris(&st, 1);

        let (frame, loops) = slot(&st, fi);
        st.add_furniture_cut(fi, &frame, &loops, true, 0.0).unwrap();

        assert_ne!(tris(&st, 0), before, "the one that was cut changed");
        assert_eq!(tris(&st, 1), before, "its sibling did not");
        assert_eq!(tris(&st, 2), before, "nor did the third");
        assert!(st.furniture[1].cuts.is_empty() && st.furniture[2].cuts.is_empty());
    }

    /// Per-part materials must survive the boolean. Face-group ids are renumbered by a cut, so a
    /// binding kept by group id would land on the wrong surfaces; it is carried part-wise instead.
    #[test]
    fn per_part_materials_survive_a_cut() {
        let (mut st, fi) = placed_door();
        let tex = st.add_texture("glass".into(), 1, 1, vec![200, 210, 205, 255]);
        // Bind that texture to every face group belonging to the door's PANEL.
        let panel = cad_solid::door::Part::Panel as u32;
        let bound: std::collections::HashMap<u32, usize> = {
            let a = &st.furniture_lib[st.furniture[fi].asset];
            let g = a.group_geom();
            (0..a.positions.len() / 3)
                .filter(|&t| a.part_ids[t] == panel)
                .map(|t| (g.face[t], tex))
                .collect()
        };
        assert!(!bound.is_empty(), "the panel has face groups to bind");
        st.furniture[fi].surface_texture = bound;

        let (frame, loops) = slot(&st, fi);
        st.add_furniture_cut(fi, &frame, &loops, true, 0.0).unwrap();

        // After the cut, every group still wearing the texture must belong to the PANEL, and the
        // panel must still be wearing it somewhere.
        let a = &st.furniture_lib[st.furniture[fi].asset];
        let g = a.group_geom();
        let mut on_panel = 0;
        for t in 0..a.positions.len() / 3 {
            if st.furniture[fi].surface_texture.get(&g.face[t]) == Some(&tex) {
                assert_eq!(a.part_ids[t], panel, "the texture stayed on the panel");
                on_panel += 1;
            }
        }
        assert!(on_panel > 0, "…and is still on it after the cut");
    }

    /// An imported (open) mesh is refused, the piece is untouched, and — the part that matters for
    /// an editable list — NO cut is left behind. A recorded cut that always fails would leave the
    /// piece permanently unable to rebuild.
    #[test]
    fn a_refused_cut_leaves_no_trace() {
        let mut st = FactoryState::default();
        // An open shell: a box with one triangle removed, as an "import".
        let (_m, mesh) = cad_solid::door::build(&cad_solid::door::DoorInput::default()).unwrap();
        let obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions[3..].to_vec(),
            normals: mesh.normals[3..].to_vec(),
            color: None,
            alpha: Vec::new(),
        };
        let a = st.add_furniture_asset("Imported".into(), obj);
        st.place_furniture(a, Vec3::ZERO);
        let before = st.furniture_lib[a].positions.clone();

        let (frame, loops) = slot(&st, 0);
        let err = st.add_furniture_cut(0, &frame, &loops, true, 0.0).unwrap_err();
        assert!(matches!(err, cad_solid::meshcut::CutError::NotClosed { .. }), "{err}");
        assert!(st.furniture[0].cuts.is_empty(), "the failed cut was not recorded");
        assert_eq!(st.furniture[0].asset, a, "the piece still points at its own asset");
        assert_eq!(st.furniture_lib[a].positions, before, "and is geometrically untouched");
    }

    /// Cuts are stored in the piece's OWN space, so moving and spinning it carries its holes along.
    /// Stored in world space they would stay behind the instant it was dragged.
    #[test]
    fn a_cut_travels_with_the_piece() {
        let (mut st, fi) = placed_door();
        let (frame, loops) = slot(&st, fi);
        st.add_furniture_cut(fi, &frame, &loops, true, 0.0).unwrap();
        let cut_tris = tris(&st, fi);

        // Move and rotate, then rebuild: the geometry must be identical, because the cut is local.
        st.furniture[fi].pos = [5.0, -3.0, 0.0];
        st.furniture[fi].rot = [0.0, 0.0, 37.0];
        let before = st.furniture_lib[st.furniture[fi].asset].positions.clone();
        st.rebuild_cut_asset(fi).unwrap();
        assert_eq!(tris(&st, fi), cut_tris, "the same cut, after moving");
        assert_eq!(
            st.furniture_lib[st.furniture[fi].asset].positions, before,
            "local geometry is unchanged by the pose"
        );
    }

    /// A face on a furniture piece must be pickable at all — the gap that used to make this whole
    /// feature unreachable, since sketch-on-face is how a cut is aimed.
    #[test]
    fn a_furniture_face_can_be_picked_and_resolved_back_to_its_piece() {
        let (mut st, fi) = placed_door();
        st.recompute();
        let (frame, _) = slot(&st, fi);
        assert_eq!(st.furniture_at_face(&frame), Some(fi), "the frame resolves to its piece");

        // A frame floating in space belongs to nothing.
        let nowhere = Frame::from_point_normal(Vec3::new(9.0, 9.0, 9.0), Vec3::Z);
        assert_eq!(st.furniture_at_face(&nowhere), None);
    }
}

#[cfg(test)]
mod place_tests {
    use super::*;

    /// Point placement (owner, 2026-07-22): a Box places its NEAR CORNER at the click
    /// (extends +w,+d from there); every other primitive places its CENTRE at the click.
    #[test]
    fn box_corner_and_cylinder_centre() {
        let mut st = FactoryState::default();
        // Box 2×2×1, corner at (10, 20) → centre offset by half-extents (+1, +1).
        st.place_primitive(Primitive::Box { w: 2.0, d: 2.0, h: 1.0 }, Vec3::new(10.0, 20.0, 0.0));
        assert_eq!(st.model.features.len(), 1);
        let pl = st.model.features[0].placement;
        assert!((pl.u - 11.0).abs() < 1e-4 && (pl.v - 21.0).abs() < 1e-4,
                "box's near corner sits at the click");

        // Cylinder centred at the click.
        st.place_primitive(Primitive::Cylinder { r: 1.0, h: 2.0, sides: 24 }, Vec3::new(5.0, -5.0, 0.0));
        let pl2 = st.model.features[1].placement;
        assert!((pl2.u - 5.0).abs() < 1e-4 && (pl2.v + 5.0).abs() < 1e-4,
                "cylinder centre sits at the click");
    }
}

/// The face-selection highlight, which once made the whole app unusable for as long as anything
/// was selected: it walked all 2.01 M triangles of the villa each frame and handed egui 45,888 line
/// segments to tessellate, at 250–436 ms a frame. These tests pin BOTH halves of the fix.
#[cfg(test)]
mod face_highlight {
    use super::*;

    /// An `n × n` grid of quads in the z = 0 plane, spanning [-0.5, 0.5]², as triangle soup.
    /// One flat surface, so the coplanar grouping must see exactly one face.
    fn flat_grid(n: usize) -> crate::mesh_io::ObjMesh {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let step = 1.0 / n as f32;
        for j in 0..n {
            for i in 0..n {
                let (x0, y0) = (-0.5 + i as f32 * step, -0.5 + j as f32 * step);
                let (x1, y1) = (x0 + step, y0 + step);
                for tri in [
                    [[x0, y0, 0.0], [x1, y0, 0.0], [x1, y1, 0.0]],
                    [[x0, y0, 0.0], [x1, y1, 0.0], [x0, y1, 0.0]],
                ] {
                    for v in tri {
                        positions.push(v);
                        normals.push([0.0, 0.0, 1.0]);
                    }
                }
            }
        }
        crate::mesh_io::ObjMesh { positions, normals, color: None, alpha: Vec::new() }
    }

    fn one_selected_grid(n: usize) -> FactoryState {
        let mut st = FactoryState::default();
        let a = st.add_furniture_asset("grid".into(), flat_grid(n));
        st.place_furniture(a, Vec3::ZERO);
        // Every triangle is coplanar and connected, so the whole grid is one face group.
        let groups: Vec<u32> = {
            let fg = st.furniture_lib[a].group_geom();
            let mut g: Vec<u32> = fg.face.iter().copied().collect();
            g.sort_unstable();
            g.dedup();
            g
        };
        assert_eq!(groups.len(), 1, "a flat grid is ONE coplanar face");
        st.furn_face_sel = Some((0, groups));
        st
    }

    fn segs(st: &FactoryState) -> Vec<[egui::Pos2; 2]> {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        // Identity MVP: the grid lives in [-0.5, 0.5]², well inside the NDC cube.
        let mvp = Mat4::IDENTITY.to_cols_array();
        st.furniture_face_highlight_segments(rect, &mvp)
    }

    /// THE regression. The highlight must cost the face's BOUNDARY, not its triangulation — so
    /// the segment count tracks the perimeter and barely moves when the mesh gets 4× denser.
    #[test]
    fn the_highlight_outlines_the_face_instead_of_wireframing_it() {
        // 40 × 40 quads = 3,200 triangles; the old code emitted 3 segments for every one of them.
        let st = one_selected_grid(40);
        let n = segs(&st).len();
        assert_eq!(n, 160, "the perimeter of a 40×40 grid is 4 × 40 edges, got {n}");
        assert!(n * 20 < 3_200 * 3, "…which is a fraction of the 9,600-segment wireframe");

        // Quadrupling the triangles must only DOUBLE the outline (perimeter, not area). This is
        // the property that makes a 2 M-triangle mesh affordable.
        let dense = one_selected_grid(80);
        assert_eq!(segs(&dense).len(), 320, "6,400 more triangles cost 160 more segments");
    }

    /// The cache must survive a camera move — that was the other half of the cost, since the
    /// outline depends on the selection and the pose, never on where you are looking from.
    #[test]
    fn moving_the_camera_does_not_rebuild_the_outline() {
        let st = one_selected_grid(40);
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let a = st.furniture_face_highlight_segments(rect, &Mat4::IDENTITY.to_cols_array());
        let ptr = st.face_outline.borrow().as_ref().map(|c| c.edges.as_ptr() as usize);

        // A different view: same outline, different pixels, same allocation.
        let spun = Mat4::from_rotation_z(0.6) * Mat4::from_scale(Vec3::splat(0.8));
        let b = st.furniture_face_highlight_segments(rect, &spun.to_cols_array());
        assert_eq!(
            ptr,
            st.face_outline.borrow().as_ref().map(|c| c.edges.as_ptr() as usize),
            "the cached edges were not rebuilt"
        );
        assert_eq!(a.len(), b.len(), "the same edges are projected");
        assert!(a.iter().zip(&b).any(|(p, q)| p[0] != q[0]), "…to different places on screen");
    }

    /// Changing the selection MUST rebuild it — a cache that never invalidates is a bug that
    /// shows the previous face highlighted.
    #[test]
    fn changing_the_selection_rebuilds_the_outline() {
        let mut st = one_selected_grid(40);
        assert_eq!(segs(&st).len(), 160);
        st.furn_face_sel = Some((0, vec![999]));
        assert_eq!(segs(&st).len(), 0, "a group that selects nothing outlines nothing");
        assert!(!st.face_highlight_truncated());
    }

    /// Nothing targeted, nothing drawn.
    #[test]
    fn no_selection_means_no_segments() {
        let mut st = one_selected_grid(20);
        st.furn_face_sel = None;
        assert!(segs(&st).is_empty());
    }
}

/// The lighting moved out of the vertex bake and into the fragment shader. `shade` /
/// `shade_furniture` survive as the CPU TWIN — the thing the GLSL has to agree with — so these
/// tests are what stop the two drifting apart.
#[cfg(test)]
mod shader_twin {
    use super::*;

    /// Moving the sun must no longer invalidate the vertex buffer. This was the hour slider
    /// rebuilding 1.86 M vertices on every nudge; with the lighting in the shader, no vertex
    /// changes, and the cache must know that.
    #[test]
    fn moving_the_sun_does_not_rebuild_the_vertex_buffer() {
        let mut st = FactoryState::default();
        st.sun.enabled = true;
        st.sun.hour = 9.0;
        let a = st.opaque_sig();
        st.sun.hour = 17.0;
        st.sun.north_offset_deg = 40.0;
        st.sun.intensity = 1.6;
        assert_eq!(a, st.opaque_sig(), "a sun move must not invalidate the buffer");
        // …but switching daylight OFF selects the studio response, which the shader branches on.
        st.sun.enabled = false;
        assert_ne!(a, st.opaque_sig(), "toggling daylight must still invalidate");
    }

    /// Whatever else changes, a vertex must carry the surface's OWN colour and its normal. If the
    /// bake ever creeps back in, per-frame lighting and instancing both quietly become impossible
    /// again, and nothing else would notice.
    #[test]
    fn a_vertex_carries_albedo_and_normal_not_a_lit_colour() {
        set_sun_light(false, Vec3::Z, [1.0; 3], [[0.0; 3]; 9]);
        let n = Vec3::new(0.0, 0.0, 1.0);
        let red = [0.8, 0.1, 0.1];
        let vert = v(Vec3::ZERO, shade(red, n));
        assert!((vert.r - red[0]).abs() < 1e-6, "the vertex carries the authored colour, got {}", vert.r);
        assert!((vert.g - red[1]).abs() < 1e-6 && (vert.b - red[2]).abs() < 1e-6);
        assert!((vert.nz - 1.0).abs() < 1e-6, "and its normal, got ({},{},{})", vert.nx, vert.ny, vert.nz);
        assert_eq!(vert.mode, SHADE_SCENE);
        assert_eq!(v(Vec3::ZERO, shade_furniture(red, n)).mode, SHADE_FURNITURE);
        // A UI swatch is not a surface: zero normal, and the shader passes it through unlit.
        let sw = v(Vec3::ZERO, ui(red));
        assert_eq!(sw.mode, SHADE_UI);
        assert_eq!((sw.nx, sw.ny, sw.nz), (0.0, 0.0, 0.0));
    }

    /// A thin card seen from behind must still be lit. Foliage is one sheet of triangles, so half
    /// of every tree faces away from its authored normal; without this the villa's trees rendered
    /// black. The CPU twin has no viewer and cannot express it, so the check is on the shader
    /// source — and the tests below therefore cover the front-facing case only.
    #[test]
    fn the_flat_path_lights_both_sides_of_a_surface() {
        let src = crate::light3d::scene_fs_for_test();
        assert!(
            src.contains("if (!gl_FrontFacing) N = -N;"),
            "the flat path must face its normal to the viewer, as the textured path and the tracer do"
        );
    }

    /// The moved lighting must be the SAME lighting. This recomputes what the fragment shader does
    /// — from the shader's own source constants where it can — and checks it against the CPU twin
    /// for both modes, sun on and sun off. Front-facing normals only: see the two-sided test above.
    #[test]
    fn the_shader_lighting_matches_the_cpu_twin() {
        let src = crate::light3d::scene_fs_for_test();
        // The studio constants are written into the shader; read them back rather than trust that
        // the two copies were typed the same.
        for want in ["float fill = furniture ? 0.6 : 0.35;", "(furniture ? 0.4 : 0.65) * abs(dot(N, normalize(STUDIO_DIR)))"] {
            assert!(src.contains(want), "shader no longer contains `{want}`");
        }
        assert!(src.contains("vec3(0.35, 0.25, 0.9)"), "STUDIO_DIR changed in the shader");
        assert!(src.contains("float extra = furniture ? 0.05 : 0.0;"), "the furniture ambient floor changed");

        // The shader's own arithmetic, in Rust.
        let shader = |albedo: [f32; 3], n: Vec3, furniture: bool, sun: Option<&SunLightRaw>| -> ([f32; 3], [f32; 3]) {
            let a = crate::color::srgb_to_linear3(albedo);
            match sun {
                Some(s) => {
                    let extra = if furniture { 0.05 } else { 0.0 };
                    let sh = crate::env::sh_ambient(&s.sh, n);
                    let lit = n.dot(s.dir).max(0.0);
                    (
                        [a[0] * (sh[0] + extra), a[1] * (sh[1] + extra), a[2] * (sh[2] + extra)],
                        [a[0] * s.sun[0] * lit, a[1] * s.sun[1] * lit, a[2] * s.sun[2] * lit],
                    )
                }
                None => {
                    let fill = if furniture { 0.6 } else { 0.35 };
                    let k = (if furniture { 0.4 } else { 0.65 })
                        * n.dot(Vec3::new(0.35, 0.25, 0.9).normalize()).abs();
                    ([a[0] * fill, a[1] * fill, a[2] * fill], [a[0] * k, a[1] * k, a[2] * k])
                }
            }
        };

        let normals = [Vec3::Z, Vec3::X, Vec3::new(0.3, -0.6, 0.74).normalize(), -Vec3::Z];
        let colours = [[0.8, 0.1, 0.1], [0.2, 0.5, 0.9], [0.72; 3]];
        // A sky with real directional variation, so an SH mistake cannot hide behind a flat dome.
        let sky = crate::env::Sky::new(Vec3::new(0.4, 0.3, 0.86).normalize(), crate::env::DEFAULT_TURBIDITY);
        let lit_sun = SunLightRaw {
            enabled: true,
            dir: Vec3::new(0.4, 0.3, 0.86).normalize(),
            sun: [2.2, 2.0, 1.7],
            sh: sky.sh9(),
        };
        for sun_on in [false, true] {
            if sun_on {
                set_sun_light(true, lit_sun.dir, lit_sun.sun, lit_sun.sh);
            } else {
                set_sun_light(false, Vec3::Z, [1.0; 3], [[0.0; 3]; 9]);
            }
            for &n in &normals {
                for &c in &colours {
                    for furniture in [false, true] {
                        let cpu = if furniture { shade_furniture(c, n) } else { shade(c, n) };
                        let (amb, dir) = shader(c, n, furniture, sun_on.then_some(&lit_sun));
                        // The twin splits into ambient + direct; the CPU packs the same two into a
                        // colour and the ambient's share of it. Recombine and compare.
                        for k in 0..3 {
                            let total = amb[k] + dir[k];
                            assert!(
                                (total - cpu.col[k]).abs() < 1e-5,
                                "sun={sun_on} furn={furniture} n={n:?} c={c:?} channel {k}: shader {total} vs cpu {}",
                                cpu.col[k]
                            );
                        }
                    }
                }
            }
        }
        set_sun_light(false, Vec3::Z, [1.0; 3], [[0.0; 3]; 9]);
    }
}

/// The daylight the renders are judged against. Every number here was READ OUT of the villa
/// scene's own Blender file (`probe_look.py`), not chosen: sun energy 8.0, colour
/// (1.0, 0.745, 0.48), angular size 2.2°, at 34° of altitude; world a gradient sky from
/// (1.25, 1.19, 1.08) at the horizon to (0.75, 0.89, 1.08) at the zenith.
#[cfg(test)]
mod daylight_match {
    use super::*;

    /// Set the sun to a given ALTITUDE by searching the hour, so a claim about "a 34° sun" is
    /// about geometry rather than about a time of day at some latitude.
    fn sun_at_altitude(alt_deg: f32) -> SunEnv {
        let mut best = (f32::MAX, 12.0f32);
        let mut e = SunEnv { enabled: true, lat_deg: 15.3, lon_deg: 74.0, utc_offset: 5.5, month: 1, day: 15, ..Default::default() };
        for i in 0..(24 * 12) {
            e.hour = i as f32 / 12.0;
            let (_, dir, _, _, _) = e.resolve();
            let a = dir.z.clamp(-1.0, 1.0).asin().to_degrees();
            let d = (a - alt_deg).abs();
            if d < best.0 {
                best = (d, e.hour);
            }
        }
        e.hour = best.1;
        e
    }

    /// A low sun must be VISIBLY AMBER. It used to reach neutral white by ~20° of altitude, and a
    /// white sun lighting a scene under a near-white sky ambient is most of why our renders read
    /// flat: there is no warm/cool separation anywhere in the image.
    #[test]
    fn a_low_sun_is_warm_the_way_the_reference_render_is() {
        let e = sun_at_altitude(34.0);
        let (_, dir, sun, _, _) = e.resolve();
        let alt = dir.z.asin().to_degrees();
        assert!((alt - 34.0).abs() < 3.0, "test set up a {alt:.1}° sun, wanted 34°");
        // Blender's own sun for this scene is (1.0, 0.745, 0.48): a red:blue ratio of 2.08.
        let ratio = sun[0] / sun[2];
        assert!(ratio > 1.5, "a 34° sun should be amber (R/B {ratio:.2}, Blender's is 2.08)");
        let g = sun[1] / sun[0];
        assert!((0.65..0.85).contains(&g), "green should sit near Blender's 0.745, got {g:.3}");
    }

    /// …but noon must NOT be orange. Warming a low sun is only right if the ramp still resolves
    /// to near-neutral overhead, otherwise every render turns into a sunset.
    #[test]
    fn a_high_sun_is_close_to_neutral() {
        let e = sun_at_altitude(80.0);
        let (_, _, sun, _, _) = e.resolve();
        let ratio = sun[0] / sun[2];
        assert!((1.0..1.35).contains(&ratio), "an overhead sun should be near-neutral, R/B {ratio:.2}");
    }

    /// The direct-to-ambient BALANCE was already right, and must stay right — it is the one part
    /// of the lighting that already matched. Blender: direct irradiance 8.0 x (1, .745, .48)
    /// against a sky dome averaging ~1.03, so roughly 1.8 : 1 in favour of the sun. Ours is
    /// carried as irradiance/pi, so the comparison is `sun` against `sky` directly.
    #[test]
    fn direct_and_ambient_stay_in_the_ratio_the_reference_uses() {
        let e = sun_at_altitude(60.0);
        let (_, _, sun, sky, _) = e.resolve();
        let lum = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let r = lum(sun) / lum(sky);
        assert!((1.2..3.0).contains(&r), "direct:ambient is {r:.2}:1, reference is ~1.8:1");
    }
}

#[cfg(test)]
mod reflection_defaults {
    use super::*;

    /// A material nobody has said anything about reflects its surroundings by the physically
    /// correct amount. The old default of 0 meant "matte", and since the only code that ever set
    /// `reflect` was `metallic × (1 − 0.6·roughness)`, EVERY dielectric in every imported scene —
    /// water, glass, polished stone, varnished timber — kept it. The environment lobe was
    /// computed correctly and then multiplied by zero.
    #[test]
    fn a_new_material_reflects_its_surroundings() {
        let t = TextureAsset::new("swatch".into(), 1, 1, vec![128, 128, 128, 255]);
        assert_eq!(t.reflect, 1.0, "a plain swatch is not matte by fiat");
        assert_eq!(t.metallic, 0.0, "…and it is a dielectric, which is exactly the case that broke");

        let p = TextureAsset::procedural("wood".into(), ProcDef::oak());
        assert_eq!(p.reflect, 1.0, "a procedural material too");
    }

    /// Roughness is how a surface is made matte — not by switching its reflection off. A mirror
    /// and a chalk wall differ in `roughness`, and both keep `reflect` at 1.
    #[test]
    fn roughness_not_reflect_is_what_makes_something_matte() {
        let mut water = TextureAsset::new("pool_water".into(), 1, 1, vec![14, 77, 87, 255]);
        water.roughness = 0.035;
        let mut chalk = TextureAsset::new("stucco".into(), 1, 1, vec![230, 228, 220, 255]);
        chalk.roughness = 0.95;
        assert_eq!((water.reflect, chalk.reflect), (1.0, 1.0));
        assert!(water.roughness < chalk.roughness, "the difference lives in roughness alone");
    }

    /// Sidecars written while 0 was the default must not reload with their reflections switched
    /// off. The UI floors at 0.01, so a stored 0 can only have come from before.
    #[test]
    fn an_old_sidecars_zero_reflect_migrates_to_physical() {
        let png = TextureAsset::new("t".into(), 1, 1, vec![200, 200, 200, 255]).encoded_png();
        let rec = |reflect: f32| crate::simlux_io::TextureRec {
            name: "t".into(), w: 1, h: 1, scale: 1.0, offset: [0.0, 0.0], rot_deg: 0.0,
            opacity: 1.0, reflect, png_b64: png.clone(), ..Default::default()
        };
        assert_eq!(decode_texture_rec(&rec(0.0)).unwrap().reflect, 1.0, "0 = written before this worked");
        // …but a deliberate knock-down is still honoured.
        let dulled = decode_texture_rec(&rec(0.25)).unwrap().reflect;
        assert!((dulled - 0.25).abs() < 1e-6, "an authored value survives the round trip");
    }
}

#[cfg(test)]
mod face_on_view {
    use super::*;

    fn cam(st: &FactoryState) -> Vec3 {
        Vec3::from(crate::light3d::cam_eye(st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target))
    }

    /// Square-on means the camera sits along the face's normal, looking straight back down it.
    #[test]
    fn the_camera_ends_up_on_the_face_normal() {
        for n in [
            Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z,
            Vec3::new(1.0, 1.0, 0.0).normalize(),
            Vec3::new(-0.3, 0.7, 0.65).normalize(),
        ] {
            let at = Vec3::new(3.0, -4.0, 2.0);
            let frame = Frame::from_point_normal(at, n);
            let mut st = FactoryState::default();
            // Put the EYE well over on the side the face points at, so the side-flip has nothing
            // to do and this test is only about the ANGLE. Target far out along `n` with a short
            // dist: wherever the default yaw/pitch happens to point, the eye lands on the +n side.
            // (Setting only the TARGET is not enough — the eye sits `dist` away from it in some
            // unrelated direction, which is what made the first version of this test wrong.)
            st.cam_target = (at + n * 100.0).to_array();
            st.cam_dist = 1.0;
            assert!((cam(&st) - at).dot(n) > 0.0, "test setup: the eye must start on the +n side");
            st.look_at_frame(&frame, at);

            let to_eye = (cam(&st) - at).normalize();
            assert!(to_eye.dot(n) > 0.9999, "n={n:?} put the eye at {to_eye:?}");
            assert!((Vec3::from(st.cam_target) - at).length() < 1e-4, "the face is centred");
            assert!(st.ortho, "parallel projection — the scale across the face must be constant");
        }
    }

    /// The camera must not swing THROUGH the wall.
    ///
    /// `pick_face` hands back an outward normal, but outward is a property of the solid and not of
    /// where you are standing. Following it blindly on a face whose outward side faces away puts
    /// the camera inside the building, looking at the back of what you just picked.
    #[test]
    fn it_stays_on_the_side_the_camera_was_already_on() {
        let at = Vec3::ZERO;
        let frame = Frame::from_point_normal(at, Vec3::X); // "outward" is +X
        let mut st = FactoryState::default();
        st.cam_target = [-8.0, 0.0, 0.0]; // …but we are standing at −X
        st.cam_dist = 10.0;
        st.look_at_frame(&frame, at);
        assert!(cam(&st).x < 0.0, "the camera crossed the face instead of squaring up to it");
    }

    /// Distance is preserved: this reframes, it does not also re-zoom.
    #[test]
    fn the_zoom_is_left_alone() {
        let mut st = FactoryState::default();
        st.cam_dist = 37.5;
        let f = Frame::from_point_normal(Vec3::new(1.0, 2.0, 3.0), Vec3::Y);
        st.look_at_frame(&f, Vec3::new(1.0, 2.0, 3.0));
        assert!((st.cam_dist - 37.5).abs() < 1e-6);
    }

    /// A horizontal face asks for a straight-down view — the pitch the orbit drag itself clamps
    /// away from. `mvp` flips its up-vector past ±0.999 rad exactly so this case stays stable, and
    /// `set_view` already relies on it for Top/Bottom, so it must NOT be clamped here.
    #[test]
    fn a_floor_gives_a_true_plan_view() {
        let mut st = FactoryState::default();
        let f = Frame::from_point_normal(Vec3::ZERO, Vec3::Z);
        st.cam_target = [0.0, 0.0, 5.0];
        st.look_at_frame(&f, Vec3::ZERO);
        assert!((st.cam_pitch - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "pitch {} is not straight down", st.cam_pitch);
        let m = crate::light3d::mvp(st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target, 1.5, st.ortho);
        assert!(m.iter().all(|v| v.is_finite()), "the look-at degeneracy produced a NaN matrix");
    }

    /// A degenerate frame leaves the camera exactly where it was rather than producing NaNs.
    #[test]
    fn a_degenerate_face_is_ignored() {
        let mut st = FactoryState::default();
        let (y, p) = (st.cam_yaw, st.cam_pitch);
        let f = Frame { origin: Vec3::ZERO, u: Vec3::X, v: Vec3::X }; // u × v = 0
        st.look_at_frame(&f, Vec3::ZERO);
        assert_eq!((st.cam_yaw, st.cam_pitch), (y, p));
    }
}

#[cfg(test)]
mod water_material {
    use super::*;

    fn preset(name: &str) -> MaterialPreset {
        *material_presets().iter().find(|p| p.name == name).expect("preset in the library")
    }

    /// The library has water, and it is a MEDIUM — which is the property everything else hangs off.
    ///
    /// Nothing here is specific to the villa's pool. A fountain basin, a canal, a wet road or a
    /// puddle drawn as one flat face all take the same material and get the same treatment.
    #[test]
    fn water_is_in_the_library_and_transmits() {
        let waters: Vec<_> = material_presets().into_iter().filter(|p| p.category == "Water").collect();
        assert!(waters.len() >= 3, "still / rippled / deep");
        for p in &waters {
            assert!(p.transmission > 0.0, "{}: water is a medium, not coverage", p.name);
            assert!(p.opacity < 1.0, "{}: and it is see-through", p.name);
            assert!((p.ior - 1.333).abs() < 1e-3, "{}: water is IOR 1.333", p.name);
            assert_eq!(p.metallic, 0.0, "{}: water is a dielectric", p.name);
        }
        // Roughness is the ONLY thing separating them — still water is a mirror.
        let still = preset("Water (still)");
        let rippled = preset("Water (rippled)");
        assert!(still.roughness < rippled.roughness);
        assert!(still.roughness < 0.35,
            "still water must stay under the SSR roughness gate or it cannot reflect the scene");
    }

    /// GLASS is deliberately NOT transmissive. An architectural pane is modelled as a thin sheet
    /// and coverage is the honest description of it; treating it as a volume would drop the back
    /// face of every window. The distinction is the whole reason the two are separate fields.
    #[test]
    fn glass_is_coverage_and_water_is_a_medium() {
        for p in material_presets().iter().filter(|p| p.category == "Glass") {
            assert_eq!(p.transmission, 0.0, "{} is a sheet, not a volume", p.name);
            assert!(p.opacity < 1.0, "{} is still see-through", p.name);
        }
    }

    /// Adding it to a scene has to carry transmission all the way to the shader's uniforms —
    /// otherwise the preset is just tinted paint with a nice name.
    #[test]
    fn adding_water_reaches_the_shader() {
        let mut st = FactoryState::default();
        let i = st.add_preset_material(&preset("Water (still)"));
        let t = &st.textures[i];
        assert!(t.transmission > 0.0, "the material carries it");
        assert!(t.opacity < ALPHA_OPAQUE, "…so the surface routes to the blended pass");
        assert!(t.has_pbr(), "…and its PBR params are shipped to the renderer at all");
        assert!(t.pbr_params().transmission > 0.0, "…including the transmission the shader reads");
        assert_eq!(t.reflect, 1.0, "and it reflects its surroundings by the physical amount");
    }

    /// It has to survive a save and reload. `opacity` says how much light gets past; this says
    /// whether what gets past is FILTERED on the way. A water material that came back without it
    /// would reload as tinted glazing lying over the pool floor.
    #[test]
    fn transmission_survives_the_sidecar() {
        let mut st = FactoryState::default();
        let i = st.add_preset_material(&preset("Water (still)"));
        let src = &st.textures[i];
        let rec = crate::simlux_io::TextureRec {
            name: src.name.clone(), w: src.w, h: src.h, scale: src.scale, offset: src.offset,
            rot_deg: src.rot_deg, opacity: src.opacity, reflect: src.reflect,
            png_b64: src.encoded_png(), roughness: src.roughness, metallic: src.metallic,
            ior: src.ior, transmission: src.transmission, ..Default::default()
        };
        let back = decode_texture_rec(&rec).expect("round trip");
        assert!((back.transmission - src.transmission).abs() < 1e-6,
            "reloaded as {} instead of {}", back.transmission, src.transmission);
    }
}
