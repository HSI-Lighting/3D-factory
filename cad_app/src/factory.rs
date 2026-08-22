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

/// WHERE a newly added 3D object, furniture piece or architecture element lands.
///
/// Reported as: "when a 3d object, furniture, or room element etc … [is] placed the user has no
/// control, it gets added at the origin. the user needs the control. the user can choose whether it
/// gets added at the origin, or at a distance from the origin and a place the user can click on the
/// 3d or 2d window."
///
/// `Centre` is what every add path used to do unconditionally — and it is not the world origin on
/// purpose: an imported drawing sits at its DXF coordinates (X≈3619, Y≈956), so a piece dropped at
/// (0,0) is kilometres off-screen and looks like nothing happened. That behaviour is kept, as an
/// option rather than the only possibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum PlaceMode {
    /// Click the 2D or 3D window to say where it goes. THE DEFAULT.
    #[default]
    Click,
    /// The centre of the current model — where a piece is certain to be visible.
    Centre,
    /// World (0, 0), whatever the model is doing.
    Origin,
    /// A fixed distance from the world origin — [`FactoryState::place_offset`].
    Offset,
}

impl PlaceMode {
    pub const ALL: [PlaceMode; 4] =
        [PlaceMode::Click, PlaceMode::Centre, PlaceMode::Origin, PlaceMode::Offset];

    pub fn label(self) -> &'static str {
        match self {
            PlaceMode::Click => "Click to place",
            PlaceMode::Centre => "Model centre",
            PlaceMode::Origin => "World origin",
            PlaceMode::Offset => "Offset from origin",
        }
    }

    /// The word the 3D command line accepts for this mode.
    pub fn keyword(self) -> &'static str {
        match self {
            PlaceMode::Click => "click",
            PlaceMode::Centre => "centre",
            PlaceMode::Origin => "origin",
            PlaceMode::Offset => "offset",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            PlaceMode::Click => {
                "New objects wait for a click — in either window — before they settle."
            }
            PlaceMode::Centre => "New objects land at the middle of the model, always in view.",
            PlaceMode::Origin => "New objects land at world (0, 0).",
            PlaceMode::Offset => "New objects land at a fixed distance from world (0, 0).",
        }
    }
}

/// The object that has just been added and is waiting on a click to say where it goes.
///
/// It ALREADY EXISTS at the position [`PlaceMode::Centre`] would have chosen. Building it first and
/// moving it second is what keeps this from touching the nine add paths: each one still creates its
/// object, binds its textures and writes its status exactly as before, and none of them has to know
/// a click is coming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AwaitingPlace {
    /// Index into [`FactoryState::furniture`] — furniture, apertures, and every parametric
    /// generator, all of which are furniture instances.
    Furniture(usize),
    /// A CSG feature id — boxes, cylinders, and the rest of the solid primitives.
    Feature(u32),
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

/// A ROOM as an editable object, rather than the loose features it happened to create.
///
/// `add_room` used to build a floor slab, a ring of wall boxes and a ceiling slab, then forget
/// which was which. Nothing tied them together, so a room could not be renamed, re-heighted or even
/// listed — the only way to change one was to delete every piece and draw it again. It also meant
/// an opening could not say which room it was in, because no room existed to be in.
///
/// The record owns the PARAMETERS and the feature ids they produced, and that is what makes an edit
/// possible in place: changing the clear height resizes each wall box and lifts the ceiling slab
/// while leaving every feature id alone — so a window already cut into a wall survives its room
/// getting taller.
#[derive(Clone, Debug)]
pub struct RoomInst {
    pub id: u32,
    /// What the room is called. Shown on the plan, and what openings are grouped under.
    pub name: String,
    /// Ground-plane outline, closed.
    pub footprint: Vec<Vec2>,
    /// Z the room stands on — the storey it was built on.
    pub base_z: f32,
    /// CLEAR height: floor top to ceiling underside. The slabs are additional, so the structure is
    /// always `floor_t + height + ceiling_t` tall. See the `room_height_meaning` tests.
    pub height: f32,
    pub floor_t: f32,
    pub ceiling_t: f32,
    pub wall_t: f32,
    pub open_top: bool,
    /// The features this room owns, so an edit can find them and a delete can take them all.
    pub floor: Option<u32>,
    /// Perimeter walls — EMPTY when the room was carved out of an enclosing building.
    ///
    /// A carved room's wall IS the material left around the void. Building a second ring of walls
    /// inside that annulus is what made rooms look lopsided: two concentric rings at different
    /// heights, the building's and the room's, stepping against one another.
    pub walls: Vec<u32>,
    pub ceiling: Option<u32>,
    /// The Difference feature that punched this room's void out of the enclosing building.
    ///
    /// Owned by the room so that deleting the room removes it and the building returns to solid.
    /// Without this the void outlived the room and left a permanent hole — the area could never be
    /// built in again.
    pub carve: Option<u32>,
}

impl RoomInst {
    /// Overall height of the built structure — what actually occupies space.
    pub fn overall_height(&self) -> f32 {
        self.floor_t + self.height + if self.open_top { 0.0 } else { self.ceiling_t }
    }

    /// Where to put the room's NAME on the plan — a point guaranteed to be inside the outline.
    ///
    /// The area centroid is the obvious choice and it is not enough: for a concave outline it can
    /// land in the notch. An L-shaped room 6 × 6 with a 4 × 4 bite out of it centroids at
    /// (2.2, 2.2), which is in the missing corner — so the name would print over whichever room is
    /// really there. A test pins that exact case.
    ///
    /// So: use the centroid when it genuinely is inside, and otherwise take the midpoint of the
    /// widest horizontal span the room has. That is inside by construction, and for an L or a
    /// corridor it puts the label along the fat part, which is where a person would write it.
    pub fn label_point(&self) -> Vec2 {
        let c = self.centroid();
        if self.contains(c) {
            return c;
        }
        let n = self.footprint.len();
        if n < 3 {
            return c;
        }
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in &self.footprint {
            lo = lo.min(p.y);
            hi = hi.max(p.y);
        }
        let mut best: Option<(f32, Vec2)> = None;
        const SCANS: u32 = 32;
        for k in 1..SCANS {
            let y = lo + (hi - lo) * k as f32 / SCANS as f32;
            // Every crossing of this scan line, sorted; the inside runs are consecutive pairs.
            let mut xs: Vec<f32> = Vec::new();
            let mut j = n - 1;
            for i in 0..n {
                let (a, b) = (self.footprint[i], self.footprint[j]);
                if (a.y > y) != (b.y > y) {
                    let t = (y - a.y) / (b.y - a.y);
                    xs.push(a.x + t * (b.x - a.x));
                }
                j = i;
            }
            xs.sort_by(f32::total_cmp);
            for pair in xs.chunks_exact(2) {
                let w = pair[1] - pair[0];
                if best.is_none_or(|(bw, _)| w > bw) {
                    best = Some((w, Vec2::new((pair[0] + pair[1]) * 0.5, y)));
                }
            }
        }
        best.map(|(_, p)| p).unwrap_or(c)
    }

    /// Area-weighted centroid of the footprint. Correct as a centre of mass, and NOT necessarily
    /// inside a concave outline — [`Self::label_point`] is the one that is.
    pub fn centroid(&self) -> Vec2 {
        let n = self.footprint.len();
        if n == 0 {
            return Vec2::ZERO;
        }
        let (mut a2, mut cx, mut cy) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            let p = self.footprint[i];
            let q = self.footprint[(i + 1) % n];
            let cross = p.x * q.y - q.x * p.y;
            a2 += cross;
            cx += (p.x + q.x) * cross;
            cy += (p.y + q.y) * cross;
        }
        if a2.abs() < 1e-9 {
            let s: Vec2 = self.footprint.iter().copied().fold(Vec2::ZERO, |a, b| a + b);
            return s / n as f32;
        }
        Vec2::new(cx / (3.0 * a2), cy / (3.0 * a2))
    }

    /// Is `p` inside this room's outline? Even-odd ray cast — the test that lets an opening say
    /// which room it belongs to.
    pub fn contains(&self, p: Vec2) -> bool {
        let n = self.footprint.len();
        if n < 3 {
            return false;
        }
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (self.footprint[i], self.footprint[j]);
            if (a.y > p.y) != (b.y > p.y) {
                let t = (p.y - a.y) / (b.y - a.y);
                if p.x < a.x + t * (b.x - a.x) {
                    inside = !inside;
                }
            }
            j = i;
        }
        inside
    }
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
    /// WHICH PLANE is open, by [`cad_solid::Sketch::id`] — not by index.
    ///
    /// It used to be an index, and `factory_delete_plane` carried a hand-written fixup to slide
    /// it down when an earlier plane was removed. That fixup was correct, and it was also the
    /// tell: an index that has to be patched after every edit is not an identity. The rename
    /// dialog held one too and never got the same treatment, so deleting a plane while a rename
    /// was open renamed a different plane.
    pub plane: u32,
    pub saved_doc: cad_kernel::Document,
    /// The main drawing's undo/redo history, parked while the sketch owns `doc`. Holds
    /// `UndoStep`s (not bare Documents) since undo spans 2D and 3D in one stack.
    pub saved_undo: Vec<crate::app::UndoStep>,
    pub saved_redo: Vec<crate::app::UndoStep>,
    /// The drawing's PARAMETRIC CONSTRAINTS, parked for the same reason as the document they
    /// describe.
    ///
    /// A constraint names its geometry by `Handle`, and handles come from a process-global
    /// counter — so a fresh sketch document shares NO handle with the plan. `prune_constraints`
    /// drops every constraint whose handles are absent from the document it is handed, and the
    /// parametric panel runs it against `self.doc` each frame. With a sketch installed that is
    /// the WHOLE drawing's constraint set, on the FIRST FRAME, silently: the DOF readout fell to
    /// 0 and nothing said why. Constraints live only in memory — no `UndoStep` variant carries
    /// them and `simlux_io` does not write them — so there was nothing to recover them from.
    ///
    /// Parking them gives the sketch a constraint space of its own, which is also the honest
    /// model: its handles are its own.
    pub saved_constraints: Vec<crate::param_editor::CRef>,
    /// A half-picked two-entity constraint ("select one, choose Parallel, now pick the other").
    /// Parked with the constraints: its `Handle` refers to the parked document, so leaving it
    /// live would let the next pick inside the sketch complete a pair spanning two documents.
    pub saved_pending: Option<(crate::param_editor::PendingKind, cad_kernel::Handle)>,
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
pub fn point_in_poly(poly: &[Vec2], x: f32, y: f32) -> bool {
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

/// Round `want` UP to the next 1-2-5 decade step: 0.1, 0.2, 0.5, 1, 2, 5, 10, 20 …
///
/// The ladder every ruler and every CAD grid uses, and for the same reason: the steps are numbers a
/// person can count in, and rounding UP means the resulting line count is never more than asked
/// for — which is what bounds the grid's cost at any zoom.
fn nice_step(want: f32) -> f32 {
    if !(want.is_finite() && want > 0.0) {
        return 1.0;
    }
    let decade = 10.0_f32.powf(want.log10().floor());
    let m = want / decade; // 1.0 ..< 10.0
    let mult = if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    };
    decade * mult
}

fn seg(out: &mut Vec<V3>, a: Vec3, b: Vec3, c: [f32; 3]) {
    out.push(v(a, ui(c)));
    out.push(v(b, ui(c)));
}

/// The colour a 2D entity draws in, as authored **sRGB** 0..1 — the same answer the 2D canvas
/// gets from `cad_kernel::resolve_color`, which is the whole point: a circle on a green layer is
/// green in both views.
///
/// Reported as: "see how the color is not getting carried. why is that" — two circles, one on a
/// white layer and one on a green one, both drawn ORANGE in 3D. Nothing was missing from the data
/// model. The line builders below simply never looked at `d.style` at all: they picked a literal
/// before entering the dobject loop, so every object in a sketch was the same colour by
/// construction. The layer table has been inside sketch documents since 752b77d, `V3` has carried
/// per-vertex RGB all along, and the 2D canvas has always resolved through this same function.
///
/// TWO documents, because an entity and the layer table that answers for it do not always live in
/// the same one. A FINISHED sketch keeps its own `Document` — its entities, its `truecolors` —
/// while the table that decides what "green" MEANS belongs to the DRAWING and is edited from the
/// Layers panel. `own` answers an entity-level override; `layer_src` answers ByLayer/ByBlock.
///
/// Exactly ONE document reaches any single `resolve_color` call, so a `TrueColorRef` is never
/// dereferenced against a table that does not index it. Be precise about what that does NOT
/// promise: `factory_enter_sketch` copies `layers` into a sketch and NOT `truecolors`, so a
/// `TrueColorRef` layer resolves to the white fallback while a sketch is open. The 2D canvas
/// reads that same broken pair, so the two views agree — it is a 2D gap, and not this one's to
/// close. Unreachable today in any case: every live path writes `Color::Aci`.
///
/// A layer id past the end of the table is `.get()` → `None` → the ACI-7 white fallback. A short
/// or drifted table is a wrong-ish colour, never a panic.
fn dobject_srgb(
    d: &cad_kernel::DObject,
    own: &cad_kernel::Document,
    layer_src: &cad_kernel::Document,
) -> [f32; 3] {
    let src = match d.style.color {
        cad_kernel::Color::ByLayer | cad_kernel::Color::ByBlock => layer_src,
        // Aci / TrueColorRef never consult the layer table; the entity's own document owns the
        // truecolor index.
        _ => own,
    };
    let (r, g, b) =
        cad_kernel::resolve_color(d.style.color, d.style.layer, &src.layers, &src.truecolors);
    // AUTHORED sRGB, NOT LINEAR. `seg` → `ui` marks the vertex `SHADE_UI`, and the line pass sets
    // `u_linearize = 1` so the FRAGMENT shader decodes it. Decoding on the CPU here would decode
    // it twice and every layer colour would come out roughly half-bright and muddy.
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

/// How far a FINISHED plane's drawing sits back from the one being drawn on, as a multiply on the
/// authored sRGB triple.
///
/// Colour used to carry "which plane is live" — hot orange against cool grey-blue — and it cannot
/// any more, because colour now carries the LAYER. VALUE carries it instead: same hue, dimmer. A
/// green layer still reads green on a plane you are not on; it just steps back. Deliberately a
/// multiply in sRGB rather than in light, because this is a perceptual step back and sRGB is the
/// space that steps back evenly across white, green and grey.
///
/// Set this to 1.0 for maximum colour fidelity — "the same white on every plane" — and leave the
/// live plane to be marked by its frame axes, the drafting banner and the yellow face outline.
const FINISHED_DIM: f32 = 0.65;

/// A tiny, STABLE outward nudge for body `fid`, in metres.
///
/// Two solids can legitimately meet face to face — a slab resting exactly on another, a wall
/// flush against a floor — and there is no depth bias anywhere in this renderer, so such faces
/// have nothing to break the tie. Which one draws is then decided by floating-point noise and
/// changes with the camera: the surface crawls, flickers, and reads as a pattern painted on it.
///
/// Keying the nudge to the body id makes the winner CONSTANT instead of correct-but-arbitrary,
/// which is what stops the crawl.
///
/// SIZING THIS IS THE WHOLE PROBLEM, and the first attempt got it wrong by choosing a value
/// that was merely invisible. A nudge below the depth buffer's own resolution rounds to the
/// same stored depth and does exactly nothing. Resolution at distance z is about
/// `z² / (near · 2²⁴)`; across a building at ordinary viewing range that is a few tenths of a
/// millimetre once `near` is sane (see the projection in `light3d`), so the nudge has to be
/// comfortably above that — roughly a millimetre, not a hundredth of one.
///
/// A millimetre is still nothing at architectural scale: it is a tenth the thickness of a
/// sheet of plasterboard, and it is applied along the surface normal, so a face moves within
/// its own plane's tolerance rather than changing shape.
///
/// This makes coincident faces STOP MOVING; it does not make overlapping solids correct. A
/// model with a solid mass swallowing its own floors is still wrong — `diag` reports that.
#[inline]
pub fn depth_tiebreak(fid: u32) -> f32 {
    (fid % 4) as f32 * 4e-4
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

/// How many decimals a length deserves in a given unit.
///
/// A millimetre field showing `2700.00` is noise — nobody dimensions a building to a hundredth of a
/// millimetre. A metre field showing `2` instead of `2.700` has lost the dimension entirely. The
/// right number of decimals is a property of the unit, not a constant.
pub fn length_decimals(u: cad_kernel::DocUnits) -> usize {
    match u.metres_per_unit {
        m if m <= 0.0015 => 0, // mm — whole millimetres
        m if m <= 0.015 => 1,  // cm
        m if m <= 0.03 => 2,   // inch
        _ => 3,                // m, ft — millimetre resolution needs three
    }
}

/// A DragValue over a length **stored in metres**, typed and displayed in `u`.
///
/// The one place metres become user-facing numbers. `speed_m`, `min_m` and `max_m` are given in
/// METRES by the caller, so the call sites keep reading as the physical limits they are and the
/// conversion happens once, here.
///
/// Converting the RANGE matters as much as converting the value: a field clamped to `0.02..=2.0`
/// would refuse every legal millimetre entry, and a drag speed left at `0.01` would move a
/// millimetre field by a hundredth of a millimetre per pixel — which reads as a field that has
/// stopped responding.
///
/// `update_while_editing(false)` is on every numeric field in the app, and it is not a detail.
/// Reported as: "when entering the parameters for ceiling, it doesnt allow to delete the already
/// existing values — when the user enters a new value it automatically has a 2 already there."
///
/// egui's `DragValue` clamps to its range on EVERY KEYSTROKE by default. A ceiling slab's minimum
/// is 0.02 m, which in millimetres is 20 — so clearing the box and typing anything smaller snapped
/// the text to "20" mid-word, and the digits that followed landed after a "2" the user never typed.
/// The range is right; enforcing it on half-finished input is not. With the flag off, the text is
/// left alone while typing and the value commits — still clamped — on Enter or on leaving the
/// field. 105 fields carried the same behaviour; all of them are fixed, not just the reported one.
pub fn length_ui(
    ui: &mut egui::Ui,
    u: cad_kernel::DocUnits,
    v: &mut f32,
    speed_m: f64,
    min_m: f64,
    max_m: f64,
) -> egui::Response {
    let mut shown = u.from_metres(*v as f64);
    let r = ui.add(
        egui::DragValue::new(&mut shown).update_while_editing(false)
            .speed(u.from_metres(speed_m))
            .range(u.from_metres(min_m)..=u.from_metres(max_m))
            .max_decimals(length_decimals(u))
            .suffix(format!(" {}", u.label())),
    );
    if r.changed() {
        *v = u.to_metres(shown) as f32;
    }
    r
}

/// [`length_ui`] with a label printed inside the field — `x`, `radius`, `reach` and so on.
pub fn length_ui_pre(
    ui: &mut egui::Ui,
    u: cad_kernel::DocUnits,
    prefix: &str,
    v: &mut f32,
    speed_m: f64,
    min_m: f64,
    max_m: f64,
) -> egui::Response {
    let mut shown = u.from_metres(*v as f64);
    let r = ui.add(
        egui::DragValue::new(&mut shown).update_while_editing(false)
            .speed(u.from_metres(speed_m))
            .range(u.from_metres(min_m)..=u.from_metres(max_m))
            .max_decimals(length_decimals(u))
            .prefix(prefix)
            .suffix(format!(" {}", u.label())),
    );
    if r.changed() {
        *v = u.to_metres(shown) as f32;
    }
    r
}

/// Format a length held in metres for display in `u`, with its unit.
pub fn length_str(u: cad_kernel::DocUnits, metres: f32) -> String {
    format!("{:.*} {}", length_decimals(u), u.from_metres(metres as f64), u.label())
}

/// An opening lifted out of the model for 2D reshaping — everything the sketch session has to
/// carry so the edit can be finished OR abandoned without losing the opening.
///
/// `factory_edit_cutout` removes the baked cutters before handing the outline to the 2D canvas,
/// which means that between entering and applying, the opening exists nowhere but here.
#[derive(Clone, Debug)]
pub struct CutoutEdit {
    /// The removed cutters, each with the body it was cutting and the index it sat at.
    ///
    /// THE HOST ID IS THE POINT, not the index. `csg::eval` binds a Difference to the nearest
    /// Union above it, so restoring by pushing onto the end would re-bind every cutter to
    /// whichever body happens to be last — the same silent defect that `rederive_wall` was fixed
    /// for. The index is the fallback for when the host body has itself been deleted meanwhile,
    /// which is the one case where there is no right answer and the user has to be told.
    pub stash: Vec<StashedCut>,
    /// Whether this opening went THROUGH its wall, as opposed to being a blind recess.
    ///
    /// Measured from the cutter at edit time (see `CadApp::cut_coverage`) rather than stored with
    /// the opening, so it works on openings cut by earlier versions and openings loaded from a
    /// file. The honest limit: a THROUGH cut that never actually reached through is remembered as
    /// the recess it visibly is. That is the safe direction — it never turns a working through
    /// hole into a pocket, and never turns a pocket into a hole.
    pub through: bool,
    /// The pocket depth to re-cut with when `!through`, in metres. Taken off the stashed cutter,
    /// so a reshape that only moves a corner reproduces the same depth exactly.
    pub depth: f32,
}

/// One cutter held out of the model during a cutout edit. See [`CutoutEdit::stash`].
#[derive(Clone, Copy, Debug)]
pub struct StashedCut {
    pub feature: cad_solid::Feature,
    /// The Union this cutter was bound to — the body it opens.
    pub host: Option<u32>,
    /// Where it sat in `model.features`, used only when `host` no longer exists.
    pub at: usize,
}

/// 3D Factory state — the model + its view. Lives on `CadApp` as one field.
pub struct FactoryState {
    pub open: bool,
    /// The unit lengths are TYPED AND SHOWN in, throughout the Factory.
    ///
    /// Everything is STORED in metres and always will be: the lux engine measures in metres, the
    /// sun's position is computed for a building in metres, imported meshes are normalised to
    /// metres, and every physical constant in the renderer assumes them. This setting changes only
    /// what a typed number means and what is printed beside it — so switching it can never move a
    /// wall, which is the same guarantee the 2D `units` command makes, for the same reason.
    ///
    /// Independent of the drawing's own unit (`Document::units`). A drawing states what its
    /// coordinates mean, which is a fact about that file; this states what someone building in 3D
    /// would rather type, which is a preference. Promotion from 2D still converts through the
    /// drawing's unit exactly as before — this setting is not in that path at all.
    ///
    /// Millimetres by default, because that is what building drawings are dimensioned in.
    pub units: cad_kernel::DocUnits,
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

    /// WHERE a newly added object lands. See [`PlaceMode`] and [`FactoryState::place_at`].
    /// Has this project been asked what unit it is built in? Persisted, so the question is asked
    /// ONCE — a modal on every visit gets dismissed unread, which is how a unit warning stops
    /// warning anybody.
    /// Draw the ground grid in the 3D view. Toggled by the GRID badge on the drafting bar.
    pub show_grid: bool,
    /// Draw the three-axis gizmo at the world origin. OFF by default — see `origin_gizmo_lines`.
    pub show_origin: bool,
    /// 3D object snap: a click in the 3D view lands on the nearest solid VERTEX within the
    /// aperture. On by default; the SNAP3D badge turns it off. See `snap_vertex`.
    pub snap_3d: bool,
    pub unit_asked: bool,
    /// The unit question is on screen right now.
    pub ask_unit: bool,
    pub place_mode: PlaceMode,
    /// A face plane being renamed: its [`cad_solid::Sketch::id`] and the text being typed.
    /// `None` = no dialog open.
    ///
    /// BY ID, because the dialog is a plain window and the view list behind it stays live: open a
    /// rename, delete an earlier plane from that list, press Rename, and an index would land on
    /// whichever plane had slid into the slot.
    pub rename_plane: Option<(u32, String)>,
    /// The distance from the world origin used by [`PlaceMode::Offset`], in metres.
    pub place_offset: [f32; 3],
    /// The typed coordinate applies to the NEXT object only, then the mode goes back to
    /// [`Self::place_mode_before_offset`]. Not persisted: a one-shot answer to a prompt is not
    /// project state, and restoring it on load would re-arm a placement nobody asked for.
    pub place_offset_once: bool,
    /// What [`Self::place_mode`] was before a typed coordinate borrowed it.
    pub place_mode_before_offset: PlaceMode,
    /// The object that was just added and is waiting for a click — in EITHER window — to say
    /// where it goes. See [`FactoryState::place_awaiting_at`].
    pub awaiting_place: Option<AwaitingPlace>,

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
    /// While drawing ON a plane, also show what is drawn on the OTHER planes, projected onto this
    /// canvas.
    ///
    /// Asked for as: "when i chose a plane to on i can see the sketch made on another plane i need
    /// a toggle to turn it off so i can only see the what ever is on the plane i am drawing."
    ///
    /// OFF by default, which is the answer to that sentence: a face sketch is its own drawing, and
    /// another plane's work projected onto it is reference at best and unselectable clutter at
    /// worst. Kept available because lining one plane's work up against another's is a real thing
    /// to want — just not the default.
    pub show_other_planes: bool,

    /// Feature ids that are ROOM CEILINGS — separate slab objects created by the room tool.
    /// Tracked so they can be hidden as a group without deleting them; the lighting model
    /// still contains them.
    pub ceilings: std::collections::HashSet<u32>,
    /// Which plane the 2D canvas is drawing on. See [`PlanView`].
    /// Every ROOM built, as an editable record. See [`RoomInst`].
    pub rooms: Vec<RoomInst>,
    /// Monotonic room id. Never reused, so a rename or a delete cannot be confused with the room
    /// that happens to occupy the same slot afterwards.
    pub next_room_id: u32,
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
    /// Show placed furniture's top-down footprint as an outline on the 2D plan — a tracing
    /// reference while drawing new furniture shapes. VIEW ONLY; toggled by the FURN badge.
    pub show_furniture_outlines_2d: bool,
    /// Draw the 2D plan THROUGH the model instead of letting solids hide it.
    ///
    /// Off by default: on a full architectural drawing the x-ray reads as texture painted on
    /// the near wall, because every line on the far side of the building projects onto it.
    /// Worth turning on when the plan is a bare outline you are tracing and the building
    /// would otherwise cover it.
    pub plan_xray: bool,

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
    ///
    /// ON BY DEFAULT. Reported as: "when i try to apply a texture of colour to wall of a room it
    /// applied for the entire building except the floor." That is exactly what per-FEATURE paint
    /// does — a building is ONE extrusion, so every wall of it is one feature, and only the floor
    /// slab (a separate feature) escaped. Per-face painting was already implemented and sat behind
    /// a checkbox nobody had reason to find.
    ///
    /// A surface is what a person means by "this wall". Painting the whole solid is still there and
    /// is now the explicit act, which is the right way round: the narrow, obvious result by
    /// default; the sweeping one on request.
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
    /// The opening currently open for 2D reshape (via `factory_edit_cutout`), and everything
    /// needed to put it back or re-cut it as itself. `None` when no cutout is being edited.
    ///
    /// This was a bare `bool`, which is what made both of its defects possible: the edit deleted
    /// the opening's cutters and then carried nothing about them, so leaving without Apply had
    /// nothing to restore and Apply had nothing to re-cut FROM — it just re-cut everything
    /// through. Holding the state in an `Option` makes the invariant structural: you cannot be
    /// editing a cutout without also holding what it takes to undo that.
    pub cutout_edit: Option<CutoutEdit>,
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
/// Every length here is in the Factory's WORKING UNIT.
///
/// This whole family was missed by the unit sweep: it lives in this file, and the sweep counted
/// metre-suffixed fields in `app.rs` only. The result was a properties panel reading
/// `height 4.00 m` with millimetre fields directly above it — so typing 1000 for a metre-tall
/// solid produced one a KILOMETRE tall, which is the "1000 m above the building" that was
/// reported. It covers every primitive's dimensions, not just the extrusion that was noticed.
pub fn primitive_dim_fields(
    ui: &mut egui::Ui,
    units: cad_kernel::DocUnits,
    p: &mut Primitive,
) -> bool {
    let f = |ui: &mut egui::Ui, label: &str, v: &mut f32, min: f32| -> bool {
        ui.horizontal(|ui| {
            ui.add_sized([64.0, 18.0], egui::Label::new(egui::RichText::new(label).small().weak()));
            length_ui(ui, units, v, 0.02, min as f64, 1e5).changed()
        })
        .inner
    };
    fn u(ui: &mut egui::Ui, label: &str, v: &mut u32, min: u32) -> bool {
        ui.horizontal(|ui| {
            ui.add_sized([64.0, 18.0], egui::Label::new(egui::RichText::new(label).small().weak()));
            ui.add(egui::DragValue::new(v).update_while_editing(false).speed(1.0).range(min..=512)).changed()
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
    pub lod: std::cell::RefCell<Option<std::sync::Arc<LodMesh>>>,
    /// Lazily-built per-triangle grouping (coplanar faces + connected bodies) for per-surface
    /// texturing. Computed once from `positions`; see [`Self::group_geom`].
    pub groups: std::cell::RefCell<Option<std::sync::Arc<FurnGroups>>>,
    /// Optional per-triangle PART id (parallel to `positions`), set for GENERATED objects (e.g. a
    /// staircase, where each tread/riser/baluster is a distinct primitive). When present it drives
    /// the "piece" grouping so clicking a tread selects that ONE tread — not the whole welded run,
    /// which is what a geometry-only connected-component grouping would give. Empty for imported
    /// meshes (they fall back to welded connected components).
    pub part_ids: Vec<u32>,
    /// EMITTING POINTS, in this asset's own local frame — set for a generated LUMINAIRE (a curved
    /// light), empty for everything else.
    ///
    /// Kept on the ASSET rather than as standalone luminaires so that the light follows the object:
    /// move the fitting and its emitters move with it, copy it and the copy lights, delete it and
    /// the light goes. A luminaire list written once at build time strands behind the fixture the
    /// first time anybody drags it, and nothing on screen says so.
    pub emitters: Vec<FurnEmitter>,
    /// Correlated colour temperature of those emitters, kelvin (0 = not a luminaire).
    ///
    /// It does NOT enter the lux calculation and must not: lux and candela are already V(λ)-
    /// weighted, so 3000 K and 6000 K at the same lumen output give the same illuminance. It is
    /// carried because the lens tint is derived from it and because EN 12464-1 asks for it on the
    /// report beside Ra.
    pub cct_k: u32,
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

/// A blackbody colour temperature as a LINEAR RGB tint, normalised so the brightest channel is 1.
///
/// For DISPLAY only — the lens glow in the render and the swatch beside it. It must never reach the
/// lux calculation: photometric units are already V(λ)-weighted, so tinting flux by colour would
/// double-count the eye's response and make a warm fitting compute dimmer than a cool one of the
/// same output, which is simply false.
///
/// Kim et al.'s piecewise cubic for the Planckian locus in CIE 1931 xy, then xy → XYZ → linear
/// sRGB. It lands D65 on (0.3135, 0.3238) against the true (0.3127, 0.3290) and 2700 K on
/// (0.4593, 0.4106) against the tabulated (0.4593, 0.4107) — far finer than a tint needs.
pub fn cct_to_linear_rgb(cct_k: u32) -> [f32; 3] {
    let t = (cct_k as f64).clamp(1667.0, 25000.0);
    let (i, i2, i3) = (1.0 / t, 1.0 / (t * t), 1.0 / (t * t * t));
    let x = if t < 4000.0 {
        -0.2661239e9 * i3 - 0.2343589e6 * i2 + 0.8776956e3 * i + 0.179910
    } else {
        -3.0258469e9 * i3 + 2.1070379e6 * i2 + 0.2226347e3 * i + 0.240390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t < 2222.0 {
        -1.1063814 * x3 - 1.34811020 * x2 + 2.18555832 * x - 0.20219683
    } else if t < 4000.0 {
        -0.9549476 * x3 - 1.37418593 * x2 + 2.09137015 * x - 0.16748867
    } else {
        3.0817580 * x3 - 5.87338670 * x2 + 3.75112997 * x - 0.37001483
    };
    let (xx, yy, zz) = (x / y, 1.0, (1.0 - x - y) / y);
    // CIE XYZ → linear sRGB (sRGB primaries, D65).
    let r = 3.2406 * xx - 1.5372 * yy - 0.4986 * zz;
    let g = -0.9689 * xx + 1.8758 * yy + 0.0415 * zz;
    let b = 0.0557 * xx - 0.2040 * yy + 1.0570 * zz;
    let m = r.max(g).max(b).max(1e-6);
    [(r / m).max(0.0) as f32, (g / m).max(0.0) as f32, (b / m).max(0.0) as f32]
}

/// One emitting point of a GENERATED LUMINAIRE, in the asset's own local frame.
///
/// Mirrors `cad_solid::sweeplight::Emitter`, so `factory` does not have to know how the fixture was
/// generated — a curved light today, anything else that emits tomorrow.
#[derive(Clone, Copy, Debug, Default)]
pub struct FurnEmitter {
    pub pos: [f32; 3],
    pub lumens: f64,
    pub watts: f64,
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
        Self { name, positions, normals, color, local_min: mn, local_max: mx, import_scale: 1.0, uvs: Vec::new(), alpha: Vec::new(), source_path: None, alpha_resolved: false, lod: std::cell::RefCell::new(None), groups: std::cell::RefCell::new(None), part_ids: Vec::new(), emitters: Vec::new(), cct_k: 0 }
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

    /// True when this asset is heavy enough to warrant the decimated display proxy.
    ///
    /// USED TO READ `uvs.is_empty() && alpha.is_empty() && …`, and that was the whole LOD system
    /// switched off. Every real import carries UVs, and anything with glass carries per-vertex
    /// alpha, so no asset above the threshold was ever decimated — on the reference gym plan all
    /// five heavy machines (467k–497k triangles each) declined it, and the view drew 7,030,514
    /// triangles a frame with a decimator sitting right there. The guard was not wrong about the
    /// hazard: clustering welds vertices, and welding across a UV seam does not blur a texture, it
    /// smears a different part of the atlas across the face.
    ///
    /// The answer is to weld with the attributes rather than in spite of them — see
    /// [`cluster_decimate_attr`], which refuses to merge across a seam, a face group or a material
    /// part, and carries UV, alpha and face id through to the proxy.
    pub fn needs_lod(&self) -> bool {
        self.positions.len() / 3 > LOD_TRI_THRESHOLD
    }

    /// The decimated proxy — positions, normals AND the attributes — built once and cached.
    ///
    /// `face` is the source face-group id per proxy triangle, which is what lets everything keyed
    /// on face groups keep working against the proxy: the per-surface texture split, face picking
    /// and the selected-face outline all read it instead of `group_geom()`.
    pub fn lod_geom(&self) -> std::sync::Arc<LodMesh> {
        {
            let c = self.lod.borrow();
            if let Some(a) = c.as_ref() {
                return a.clone();
            }
        }
        // The face grouping first — it is what stops the decimator welding one material into its
        // neighbour. `group_geom` is itself cached and, above `COPLANAR_TRI_LIMIT`, is the cheap
        // material-part grouping rather than the flood fill.
        let groups = self.group_geom();
        let built = std::sync::Arc::new(cluster_decimate_attr(
            &self.positions,
            &self.normals,
            &self.uvs,
            &self.alpha,
            &groups.face,
            64,
        ));
        *self.lod.borrow_mut() = Some(built.clone());
        built
    }
}

/// A decimated display proxy that still knows what it is made of.
///
/// The old proxy was positions and normals only, which is why it could only ever be used on assets
/// that had nothing else — and those are exactly the assets that never needed it.
#[derive(Debug, Clone, Default)]
pub struct LodMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Per vertex, and EMPTY when the source had none — callers test
    /// `uvs.len() == positions.len()` exactly as they do on the full mesh.
    pub uvs: Vec<[f32; 2]>,
    /// Per vertex, empty when the source was fully opaque.
    pub alpha: Vec<f32>,
    /// Per TRIANGLE: the face-group id of the source triangle this one came from.
    pub face: Vec<u32>,
}

impl LodMesh {
    pub fn tri_count(&self) -> usize {
        self.positions.len() / 3
    }
    /// Mirrors `FurnitureAsset::vertex_alpha` so the two can be swapped without a special case.
    pub fn vertex_alpha(&self, i: usize) -> f32 {
        self.alpha.get(i).copied().unwrap_or(1.0)
    }
}

/// How far apart in the atlas two vertices may be and still be welded together.
///
/// This is the seam-preserving half of [`cluster_decimate_attr`]. Two vertices in the same position
/// cell merge only when their texture coordinates are within this of each other, so the two sides
/// of a seam — touching in space, far apart in the atlas — stay separate.
///
/// A DISTANCE, NOT A LATTICE CELL, and the difference is worth both bugs it avoids. Quantising UV
/// space puts arbitrary boundaries through it: two coordinates a thousandth apart land either side
/// of one and are needlessly split, while two a whole cell apart can share it and be wrongly
/// merged. Measured on the reference gym plan, quantising at 1/256 gave 2,627,375 proxy triangles
/// and at 1/64 gave 1,306,456 — a 2× swing driven entirely by where the boundaries happened to
/// fall, not by any property of the meshes.
///
/// 0.01 is about ten texels on a 1024 map: comfortably tighter than the gap an unwrapper leaves
/// between islands, comfortably looser than the UV drift across one 3 cm position cell. The other
/// half of the protection is that the cluster key already separates face groups, and a UV island
/// boundary usually IS a material boundary.
pub const UV_WELD_TOL: f32 = 0.01;

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

/// SEAM-PRESERVING vertex-cluster decimation — the same idea as [`cluster_decimate`], but it
/// carries the attributes instead of refusing to run when they exist.
///
/// The plain decimator snaps every vertex to a lattice cell and welds the cell to its centroid.
/// That is fine for bare geometry and destroys anything keyed to a vertex, which is why
/// `needs_lod` used to decline every asset with UVs or per-vertex alpha — i.e. every real import.
///
/// THE CLUSTER KEY IS NOT THE POSITION CELL ALONE. It is `(position cell, UV cell, face group)`,
/// and each of the three is load-bearing:
///
/// * **UV cell** — the two sides of a texture seam touch in space and are far apart in the atlas.
///   Welding them does not blur the texture, it stretches one triangle across whatever else the
///   atlas holds between the two islands. Quantising by [`UV_WELD_GRID`] keeps them apart.
/// * **Face group** — a face group is what a texture is assigned to. Welding across the boundary
///   would drag one material's vertices into another's draw call, so the two would fight over
///   which texture the triangle belongs to. Splitting on it also means the proxy can carry the
///   source face id per triangle, which is what keeps per-surface texturing, face picking and the
///   face outline working against the proxy at all.
/// * **Position cell** — the actual decimation. Everything else only refuses to merge.
///
/// A triangle survives when its three vertices land in three distinct clusters, as before. Normals
/// are recomputed flat per surviving face; UV and alpha are the cluster's mean, which is exact for
/// a cluster that came from one island and never runs across a seam because a seam cannot be in
/// one cluster.
///
/// COST. `cluster_decimate` uses a flat `grid³` array because a per-vertex HashMap over 6 M
/// vertices measured ≈ 0.5 s and showed up as a spike the first time a heavy piece drew. That trick
/// cannot survive extra key dimensions — the array would be `grid³ × uv_grid² × groups`. Instead
/// the flat array holds a SMALL LIST per cell, scanned linearly: a position cell holds one entry in
/// the ordinary case and a handful at a seam, so the lookup keeps the flat array's speed without
/// its dimensionality.
#[allow(clippy::too_many_arguments)]
pub fn cluster_decimate_attr(
    pos: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    alpha: &[f32],
    face: &[u32],
    grid: u32,
) -> LodMesh {
    let _ = normals; // flat normals are recomputed per surviving face, as in `cluster_decimate`
    let keep_uv = uvs.len() == pos.len();
    let keep_alpha = alpha.len() == pos.len();
    if pos.len() < 3 {
        return LodMesh {
            positions: pos.to_vec(),
            normals: vec![[0.0, 0.0, 1.0]; pos.len()],
            uvs: if keep_uv { uvs.to_vec() } else { Vec::new() },
            alpha: if keep_alpha { alpha.to_vec() } else { Vec::new() },
            face: Vec::new(),
        };
    }
    let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in pos {
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    }
    let ext = [
        (mx[0] - mn[0]).max(1e-6),
        (mx[1] - mn[1]).max(1e-6),
        (mx[2] - mn[2]).max(1e-6),
    ];
    let gi = grid.clamp(2, 128) as usize;
    let gf = gi as f32;
    let cell_of = |p: &[f32; 3]| -> usize {
        let c = |v: f32, mnk: f32, ek: f32| {
            (((v - mnk) / ek * gf).floor() as i64).clamp(0, gi as i64 - 1) as usize
        };
        (c(p[0], mn[0], ext[0]) * gi + c(p[1], mn[1], ext[1])) * gi + c(p[2], mn[2], ext[2])
    };

    // Per position cell, the clusters occupying it: `(face group, cluster index)`. One entry in the
    // ordinary case; more only where a seam or a material boundary crosses the cell.
    let mut buckets: Vec<Vec<(u32, u32)>> = vec![Vec::new(); gi * gi * gi];
    // Running sums: x, y, z, u, v, alpha, count.
    let mut acc: Vec<[f64; 7]> = Vec::new();
    let mut of_vertex: Vec<u32> = Vec::with_capacity(pos.len());

    for (i, p) in pos.iter().enumerate() {
        // A NaN UV can never be within tolerance of anything, itself included, so it would split
        // every vertex from every other and defeat the decimation entirely. Folded to the origin.
        let (u, vv) = match uvs.get(i) {
            Some(t) if keep_uv && t[0].is_finite() && t[1].is_finite() => (t[0], t[1]),
            _ => (0.0, 0.0),
        };
        let fg = face.get(i / 3).copied().unwrap_or(0);
        let cell = cell_of(p);
        // Same face group, and near enough in the atlas to be the same piece of surface. The UV is
        // compared against the cluster's RUNNING MEAN, which is the centre of the coordinates
        // already merged into it — so a cluster cannot creep across a seam one vertex at a time.
        let slot = buckets[cell]
            .iter()
            .find(|(g, c)| {
                if *g != fg {
                    return false;
                }
                if !keep_uv {
                    return true;
                }
                let a = acc[*c as usize];
                let n = a[6].max(1.0);
                ((a[3] / n) as f32 - u).abs() <= UV_WELD_TOL
                    && ((a[4] / n) as f32 - vv).abs() <= UV_WELD_TOL
            })
            .map(|(_, c)| *c);
        let ci = match slot {
            Some(c) => c,
            None => {
                let c = acc.len() as u32;
                acc.push([0.0; 7]);
                buckets[cell].push((fg, c));
                c
            }
        };
        let a = &mut acc[ci as usize];
        a[0] += p[0] as f64;
        a[1] += p[1] as f64;
        a[2] += p[2] as f64;
        a[3] += u as f64;
        a[4] += vv as f64;
        a[5] += if keep_alpha { alpha[i] as f64 } else { 1.0 };
        a[6] += 1.0;
        of_vertex.push(ci);
    }

    let centre = |c: u32| -> ([f32; 3], [f32; 2], f32) {
        let a = acc[c as usize];
        let n = a[6].max(1.0);
        (
            [(a[0] / n) as f32, (a[1] / n) as f32, (a[2] / n) as f32],
            [(a[3] / n) as f32, (a[4] / n) as f32],
            (a[5] / n) as f32,
        )
    };

    let mut out = LodMesh {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        alpha: Vec::new(),
        face: Vec::new(),
    };
    for t in 0..pos.len() / 3 {
        let (i0, i1, i2) = (of_vertex[t * 3], of_vertex[t * 3 + 1], of_vertex[t * 3 + 2]);
        // Two corners in one cluster means the triangle collapsed to a sliver — drop it, exactly
        // as the plain decimator does.
        if i0 == i1 || i1 == i2 || i0 == i2 {
            continue;
        }
        let (pa, ua, aa) = centre(i0);
        let (pb, ub, ab) = centre(i1);
        let (pc, uc, ac) = centre(i2);
        let n = (Vec3::from(pb) - Vec3::from(pa))
            .cross(Vec3::from(pc) - Vec3::from(pa))
            .normalize_or_zero();
        let n = if n.length_squared() < 0.5 { [0.0, 0.0, 1.0] } else { n.to_array() };
        out.positions.extend_from_slice(&[pa, pb, pc]);
        out.normals.extend_from_slice(&[n, n, n]);
        // ABSENT IN, ABSENT OUT. Callers decide between real texture coordinates and box projection
        // by testing `uvs.len() == positions.len()`, so a proxy that emitted zeroes for a mesh that
        // had none would not fail that test — it would pass it, with garbage.
        if keep_uv {
            out.uvs.extend_from_slice(&[ua, ub, uc]);
        }
        if keep_alpha {
            out.alpha.extend_from_slice(&[aa, ab, ac]);
        }
        out.face.push(face.get(t).copied().unwrap_or(0));
    }
    out
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
    /// The bundled mesh's name WITHIN `assets/` — a stable identifier, not a place to open.
    pub fn asset_path(self) -> &'static str {
        match self {
            ApertureKind::Door => "assets/apertures/door.fbx",
            ApertureKind::Window => "assets/apertures/window.obj",
        }
    }

    /// …and where it actually is on disk. Resolved against the EXECUTABLE, because a bare
    /// relative path only finds anything when the app was launched from the repo root — see
    /// [`crate::assets`]. Both use the app's own OBJ/FBX readers.
    pub fn asset_file(self) -> std::path::PathBuf {
        crate::assets::path(self.asset_path())
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
            // Millimetres — what building drawings are dimensioned in. Storage stays metres.
            units: cad_kernel::DocUnits::new(cad_kernel::DocUnits::MM, cad_kernel::UnitSource::User),
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
            // CLICK by default. The complaint was that everything landed at the origin with no say
            // in it; the fix is not a better default position, it is being ASKED. Switch it to
            // "Model centre" on the Factory toolbar to get the old drop-and-go behaviour back.
            show_grid: true,
            show_origin: false,
            snap_3d: true,
            unit_asked: false,
            ask_unit: false,
            place_mode: PlaceMode::Click,
            rename_plane: None,
            place_offset: [0.0, 0.0, 0.0],
            place_offset_once: false,
            place_mode_before_offset: PlaceMode::Click,
            awaiting_place: None,
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
            show_other_planes: false,
            ceilings: std::collections::HashSet::new(),
            rooms: Vec::new(),
            next_room_id: 1,
            ceiling_caps: std::collections::HashSet::new(),
            hide_ceilings: false,
            cutaway: false,
            cutaway_z: 2.5,
            ceiling_thickness: 0.15,
            show_furniture_outlines_2d: true,
            plan_xray: false,
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
            paint_surface_mode: true,
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
            cutout_edit: None,
            aperture_asset: [None, None],
            sel_mesh: SolidMesh::default(),
            sel_key: Vec::new(),
        }
    }
}

impl FactoryState {
    pub fn add_box(&mut self) {
        let p = Primitive::Box { w: self.box_w, d: self.box_d, h: self.box_h };
        let placement = self.placement_for(&p);
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.selection = vec![id];
        self.arm_placement(AwaitingPlace::Feature(id));
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
    /// The rebasing [`Self::add_furniture_asset`] applies to an incoming mesh, as `(offset, k)`:
    /// every point becomes `(p - offset) * k`, which centres it in x/y, sets its base on z = 0 and
    /// rescales a wildly-off unit system toward a ~1.5 m object.
    ///
    /// Exposed as its own function because a mesh can arrive carrying data in the SAME frame that
    /// is not part of the mesh — a generated luminaire's emitting points — and that data has to
    /// move with it. Two copies of this rule would silently drift apart and put a curved light's
    /// light somewhere its lens is not.
    pub fn asset_rebase(positions: &[[f32; 3]]) -> Option<([f32; 3], f32)> {
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for p in positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        if !mn[0].is_finite() {
            return None;
        }
        let size = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
        let longest = size[0].max(size[1]).max(size[2]).max(1e-4);
        // Scale toward ~1.5 m only for wildly off sizes (cm/mm exports, or giant units).
        let k = if longest > 20.0 || longest < 0.05 { 1.5 / longest } else { 1.0 };
        Some(([(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, mn[2]], k))
    }

    pub fn add_furniture_asset(&mut self, name: String, mesh: crate::mesh_io::ObjMesh) -> usize {
        let asset_color = mesh.color.unwrap_or([0.82, 0.82, 0.84]); // file diffuse, else neutral
        let mut positions = mesh.positions;
        let normals = mesh.normals;
        let alpha = mesh.alpha; // per-vertex opacity (empty ⇒ opaque); recentring below is xyz-only
        let mut import_scale = 1.0f32;
        if let Some((off, k)) = Self::asset_rebase(&positions) {
            import_scale = k;
            for p in &mut positions {
                for c in 0..3 {
                    p[c] = (p[c] - off[c]) * k;
                }
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

    /// WHERE a newly added object lands, per [`PlaceMode`].
    ///
    /// Reported as: "when a 3d object, furniture, or room element etc … [is] placed the user has no
    /// control, it gets added at the origin". Every add path called `default_place_at` and there was
    /// no way to say otherwise. This is the one place that decides, so every add path obeys the same
    /// setting.
    ///
    /// [`PlaceMode::Click`] resolves to the same point as `Centre` — the object has to EXIST before
    /// it can be moved, and it lands somewhere visible while it waits for the click. See
    /// [`Self::awaiting_place`].
    pub fn place_at(&self) -> Vec3 {
        match self.place_mode {
            PlaceMode::Centre | PlaceMode::Click => self.default_place_at(),
            PlaceMode::Origin => Vec3::new(0.0, 0.0, 0.0),
            PlaceMode::Offset => Vec3::from(self.place_offset),
        }
    }

    /// Hand the just-added object to the placing click, if the user asked for one.
    ///
    /// Called at the END of every add — the object is already built and positioned, so nothing
    /// downstream (texture binding, part ids, status) has to know about placement at all. If the
    /// click never comes the object simply stays where it landed; Esc says so explicitly.
    fn arm_placement(&mut self, what: AwaitingPlace) {
        if self.place_mode == PlaceMode::Click {
            self.awaiting_place = Some(what);
        }
        // A TYPED COORDINATE IS ONE PLACEMENT, NOT A MODE.
        //
        // Reported as: "placing the objects with coordinates — once i enter a coordinate it keeps
        // inserting it in the same place again and again." Answering the coordinate prompt set
        // `place_mode = Offset` and left it there, so every later add landed on the same point,
        // stacked inside the last one, with nothing on screen saying a mode was still in force.
        //
        // A coordinate typed at a prompt answers THAT prompt — it is a point, the way a point is
        // in any drafting command. Choosing "Offset from origin" from the placement menu is a
        // different act and stays sticky, because that one IS a mode and the menu shows it.
        //
        // Consumed here because this runs at the END of every add, after the object has taken its
        // position — so nothing upstream needs to know the offset was single-use.
        if self.place_offset_once {
            self.place_offset_once = false;
            self.place_mode = self.place_mode_before_offset;
        }
    }

    /// Move whatever is waiting on a placing click to `at`, and disarm. Returns what moved, so the
    /// caller can name it in the status line.
    ///
    /// Only X and Y come from the click: Z is the storey the object was built on. A click on the
    /// ground plane is a plan position, not an instruction to drop a first-floor slab to the ground.
    pub fn place_awaiting_at(&mut self, at: Vec3) -> Option<AwaitingPlace> {
        let what = self.awaiting_place.take()?;
        match what {
            AwaitingPlace::Furniture(i) => {
                let inst = self.furniture.get_mut(i)?;
                inst.pos[0] = at.x;
                inst.pos[1] = at.y;
            }
            AwaitingPlace::Feature(id) => {
                let f = self.model.features.iter_mut().find(|f| f.id == id)?;
                // A Box's click point is its NEAR CORNER, matching `place_primitive` — the same
                // click must not mean two different things depending on how the box was created.
                let (ox, oy) = match f.primitive {
                    Primitive::Box { w, d, .. } => (w * 0.5, d * 0.5),
                    _ => (0.0, 0.0),
                };
                f.placement.u = at.x + ox;
                f.placement.v = at.y + oy;
            }
        }
        self.dirty = true;
        Some(what)
    }

    /// The [`Placement`] a NEW SOLID gets — [`Self::place_at`] applied, on the active storey.
    ///
    /// 3D solids used to be built at `Placement::default()`, i.e. always (0, 0), no matter what the
    /// placement mode said. Click worked (the object is created and then moved by the click) and
    /// the other three modes did nothing at all for a solid, which is not "the same placing modes"
    /// by any reading.
    ///
    /// A Box's point is its NEAR CORNER and everything else's is its CENTRE — the same convention
    /// `place_primitive` and `place_awaiting_at` use, so a box lands in the same spot however it
    /// got there.
    pub fn placement_for(&self, p: &Primitive) -> Placement {
        let at = self.place_at();
        let (ox, oy) = match p {
            Primitive::Box { w, d, .. } => (w * 0.5, d * 0.5),
            _ => (0.0, 0.0),
        };
        Placement {
            u: at.x + ox,
            v: at.y + oy,
            // Z is a HEIGHT ABOVE THE STOREY, not a replacement for it: `@0,0,2400` means "2.4 m
            // up", and on the first floor that is 2.4 m above the first floor.
            lift: self.active_base_z() + at.z,
            ..Placement::default()
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
            // Z is a HEIGHT ABOVE THE STOREY, not a replacement for it — the same reading
            // `placement_for` gives a solid, so `@0,0,2400` means the same thing to both.
            pos: [at.x, at.y, self.active_base_z() + at.z],
            scale: 1.0,
            fit: None,
            rot: [0.0, 0.0, 0.0],
            color,
            texture: None,
            surface_texture: std::collections::HashMap::new(),
            ..Default::default()
        });
        let i = self.furniture.len() - 1;
        self.select_furniture(i);
        // EVERY furniture add funnels through here — imports, apertures, and all nine parametric
        // generators — so arming once here gives the placing click to all of them without any of
        // them knowing about it. `place_aperture` deliberately does NOT come through this function:
        // an aperture fitted into an opening the user drew is already exactly where it belongs.
        self.arm_placement(AwaitingPlace::Furniture(i));
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
            let positions: &[[f32; 3]] = match &lod { Some(a) => &a.positions, None => &asset.positions };
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
            let positions: &[[f32; 3]] = match &lod { Some(a) => &a.positions, None => &asset.positions };
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
            let positions: &[[f32; 3]] = match &lod { Some(a) => &a.positions, None => &asset.positions };
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

    /// Tag `ids` as one group, so they select, move and delete together.
    ///
    /// Unlike [`Self::group_selection`] this does not touch the selection — it is for the BUILD
    /// paths, which know what they just made and must not disturb what the user had picked.
    ///
    /// MERGES rather than skips. A building with two rooms carved out of it calls this twice, and
    /// the second call shares the shell with the first; making a fresh group for it would leave
    /// one building in two groups, so moving the shell would take room A along and abandon room B.
    /// Any group the ids already belong to is absorbed into one.
    pub fn group_features(&mut self, ids: &[u32]) {
        if ids.len() < 2 {
            return;
        }
        let existing: Vec<u32> =
            ids.iter().filter_map(|id| self.feature_group.get(id).copied()).collect();
        let gid = existing.first().copied().unwrap_or_else(|| {
            let g = self.next_group_id;
            self.next_group_id += 1;
            g
        });
        let absorb: std::collections::HashSet<u32> =
            existing.into_iter().filter(|g| *g != gid).collect();
        if !absorb.is_empty() {
            for g in self.feature_group.values_mut() {
                if absorb.contains(g) {
                    *g = gid;
                }
            }
        }
        for &id in ids {
            self.feature_group.insert(id, gid);
        }
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
        self.translate_surface_textures(&self.selection.clone(), delta);
        self.dirty = true;
    }

    /// RE-KEY THE PER-FACE PAINT ON `ids` BY MAPPING THE WORLD PLANE EACH KEY NAMES.
    ///
    /// A [`SurfaceKey`] is `(feature id, quantised normal, quantised plane offset d)` — it names a
    /// PLANE, not a triangle, which is what lets one key paint a whole flat face however it is
    /// tessellated. The cost is that the plane moves with the geometry: change a feature's pose or
    /// size and every painted face on it is looked up under a key that no longer exists, so the
    /// paint silently vanishes. "A painted face keeps its paint" is exactly this function.
    ///
    /// `map` takes the old plane `(n, d)` and returns the new one. Every caller can state that in
    /// closed form — a translation, a pose change, a scale about a pivot — so nothing here
    /// re-derives a plane from the evaluated mesh and there is no tolerance anywhere.
    ///
    /// The normal is NORMALISED on the way out of the key. It was quantised at ×50 going in, so
    /// an oblique face comes back up to a hundredth off unit, and `n · Δ` over a long move would
    /// then miss the 1 cm offset quantisation and lose the paint anyway. An axis-aligned face —
    /// which is nearly all of them — is unaffected either way.
    fn remap_surface_keys(&mut self, ids: &[u32], map: impl Fn(Vec3, f32) -> (Vec3, f32)) {
        if self.surface_texture.is_empty() && self.surface_color.is_empty() {
            return;
        }
        let moved: std::collections::HashSet<u32> = ids.iter().copied().collect();
        let shift = |k: &SurfaceKey| -> SurfaceKey {
            let n = Vec3::new(k.1 as f32 / 50.0, k.2 as f32 / 50.0, k.3 as f32 / 50.0)
                .normalize_or_zero();
            let (n2, d2) = map(n, k.4 as f32 / 100.0);
            (
                k.0,
                (n2.x * 50.0).round() as i32,
                (n2.y * 50.0).round() as i32,
                (n2.z * 50.0).round() as i32,
                (d2 * 100.0).round() as i32,
            )
        };
        let remap = |m: &mut std::collections::HashMap<SurfaceKey, usize>| {
            let (mut keep, mut move_): (Vec<_>, Vec<_>) =
                m.drain().partition(|(k, _)| !moved.contains(&k.0));
            for (k, v) in move_.drain(..) {
                keep.push((shift(&k), v));
            }
            *m = keep.into_iter().collect();
        };
        remap(&mut self.surface_texture);
        let (mut keep, mut move_): (Vec<_>, Vec<_>) =
            self.surface_color.drain().partition(|(k, _)| !moved.contains(&k.0));
        for (k, v) in move_.drain(..) {
            keep.push((shift(&k), v));
        }
        self.surface_color = keep.into_iter().collect();
    }

    /// Carry per-face paint along when its feature TRANSLATES.
    ///
    /// It matters more now that a building moves as one object — "make sure you dont break the
    /// texture application while fixing this". `d' = n · (p + Δ) = d + n · Δ`, normal unchanged.
    fn translate_surface_textures(&mut self, ids: &[u32], delta: Vec3) {
        self.remap_surface_keys(ids, |n, d| (n, d + n.dot(delta)));
    }

    /// …and when its POSE changes: a rotation ring, a typed angle, anything that rewrites the
    /// feature's plane or placement.
    ///
    /// The transform that took the old pose to the new one is `after · before⁻¹`, whatever the
    /// edit was — so this needs no case per kind of edit, and a new one gets it for free.
    /// `Plane::world_matrix` is an orthonormal basis times a rotation, with no scale in it, which
    /// is what makes transforming the normal as a direction correct.
    fn repose_surface_keys(&mut self, id: u32, before: glam::Mat4, after: glam::Mat4) {
        if before.abs_diff_eq(after, 1e-6) {
            return;
        }
        let m = after * before.inverse();
        self.remap_surface_keys(&[id], |n, d| {
            // `n * d` is the point on the plane nearest the origin — any point on it will do.
            let p = m.transform_point3(n * d);
            let n2 = m.transform_vector3(n).normalize_or_zero();
            (n2, n2.dot(p))
        });
    }

    /// …and when it is SCALED about a pivot. `world_matrix` carries no scale, so the pose is
    /// unchanged and the faces move because the primitive itself grew: the normal stays, and a
    /// point on the plane goes to `pivot + k(p − pivot)`.
    fn rescale_surface_keys(&mut self, id: u32, pivot: Vec3, k: f32) {
        self.remap_surface_keys(&[id], |n, d| (n, n.dot(pivot + (n * d - pivot) * k)));
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
                self.rescale_surface_keys(id, pivot, k);
                self.dirty = true;
            }
        }
    }

    /// Paint the SURFACE (coplanar face) under the cursor with `color`. Ray-tests the
    /// cached mesh, finds the front-most triangle, and colours every triangle sharing its
    /// surface key. Returns true if a surface was hit.
    /// The SURFACE under the cursor, as its [`SurfaceKey`] — without painting anything.
    ///
    /// Split out of [`Self::paint_surface`] so the Materials Factory can ask "which face did they
    /// click?" and then edit or create that face's material. One implementation, so the window and
    /// the brush can never disagree about which face was meant.
    pub fn pick_surface_key(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<SurfaceKey> {
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
        best.map(|(_, key)| key)
    }

    pub fn paint_surface(
        &mut self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16], color: [f32; 3],
    ) -> bool {
        if let Some(key) = self.pick_surface_key(cursor, rect, mvp) {
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
        // the per-surface render path handles translucency.
        //
        // A HEAVY PIECE IS PICKED AGAINST THE PROXY, not turned away. This returned `None` for
        // anything LOD'd — which cost nothing while `needs_lod` refused every textured asset, and
        // would have made every real import un-paintable the moment it stopped. The proxy carries
        // the source face id per triangle, so a hit on it names the same face group a hit on the
        // full mesh would, at a fraction of the ray tests. It also picks what is actually ON
        // SCREEN, which is the more defensible answer for a click.
        let lod = if asset.needs_lod() { Some(asset.lod_geom()) } else { None };
        // World ray → the instance's LOCAL space (positions are stored local).
        let (ow, dw) = Self::ray(cursor, rect, mvp);
        let inv = glam::Mat4::from_cols_array(&model).inverse();
        let ol = inv.transform_point3(ow);
        let dl = inv.transform_vector3(dw).normalize_or_zero();
        let hit_pos: &[[f32; 3]] = match &lod {
            Some(a) => &a.positions,
            None => &asset.positions,
        };
        let mut best: Option<(f32, usize)> = None;
        for (ti, tri) in hit_pos.chunks_exact(3).enumerate() {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            if let Some(t) = cad_solid::ray_triangle(ol, dl, a, b, c) {
                if best.map_or(true, |(bt, _)| t < bt) {
                    best = Some((t, ti));
                }
            }
        }
        let (_, ht) = best?;
        // Which face group was hit. Through the proxy, its own per-triangle face id IS the source
        // group; the body grouping below is still read off the full mesh, which is right — it is a
        // property of the asset, not of whichever proxy triangle happened to be in the way.
        let groups = asset.group_geom();
        let ht = match &lod {
            Some(a) => {
                let fg = a.face.get(ht).copied().unwrap_or(0);
                groups.face.iter().position(|&g| g == fg).unwrap_or(0)
            }
            None => ht,
        };
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
        // TRACED ON WHATEVER IS ON SCREEN. This returned nothing at all for a LOD'd asset, which
        // cost nothing while `needs_lod` refused every textured import and would have silently
        // removed the selection outline from all of them once it stopped.
        let lod = if asset.needs_lod() { Some(asset.lod_geom()) } else { None };
        let fg = asset.group_geom();
        let (src_pos, src_face): (&[[f32; 3]], &[u32]) = match &lod {
            Some(a) => (&a.positions, &a.face),
            None => (&asset.positions, &fg.face),
        };

        // Rebuild only when the SELECTION changed — not when the camera moved.
        {
            let tris = src_pos.len() / 3;
            let stale = self.face_outline.borrow().as_ref().is_none_or(|c| {
                c.inst != *fi || c.asset != inst.asset || c.tris != tris || c.groups != *groups
            });
            if stale {
                *self.face_outline.borrow_mut() =
                    Some(Self::build_face_outline(src_pos, src_face, *fi, inst.asset, groups));
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
    /// Takes the POSITIONS AND FACE IDS rather than the asset, so it can be traced on the decimated
    /// proxy when that is what is on screen. An outline built from the full mesh would float a
    /// little off the proxy the user is actually looking at.
    fn build_face_outline(
        positions: &[[f32; 3]], face: &[u32], inst: usize, asset_idx: usize, groups: &[u32],
    ) -> FaceOutline {
        use std::collections::hash_map::Entry;
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
        for t in 0..face.len() {
            if !want.contains(&face[t]) {
                continue;
            }
            let base = t * 3;
            for e in 0..3 {
                let (p, q) = (positions[base + e], positions[base + (e + 1) % 3]);
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
            tris: positions.len() / 3,
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
        let Some(before) = self.model.get(id).map(|f| f.plane.world_matrix(&f.placement)) else {
            return;
        };
        let mut after = before;
        if let Some(f) = self.model.get_mut(id) {
            match axis {
                0 => f.placement.pitch_deg = deg,
                1 => f.placement.roll_deg = deg,
                _ => f.placement.spin_deg = deg,
            }
            after = f.plane.world_matrix(&f.placement);
            self.dirty = true;
        }
        // TURNING AN OBJECT DOES NOT REPAINT IT. Every face's world plane changed, and a
        // `SurfaceKey` names a world plane — so without this the paint on a rotated wall is
        // looked up under a plane that is no longer anywhere and quietly reverts to the default.
        self.repose_surface_keys(id, before, after);
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
    /// Returns `(void feature id, the BUILDING it was cut from)`.
    ///
    /// The building's id comes back because a room carved out of a building is PART of that
    /// building: the void, the slabs, and the mass around them are one object, and a Move that
    /// takes only one of them is exactly "when i move a building its floor and ceiling stay in
    /// place". The enclosing solid is identified here and nowhere else, so this is the only place
    /// that can say which one it was.
    fn carve_interior_from_building(&mut self, footprint: &[Vec2], base: f32) -> Option<(u32, u32)> {
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
        let Some((idx, top)) = target else { return None };
        let building_id = self.model.features[idx].id;
        // Build the void, then move it to sit right after the building it cuts.
        let Ok((profile, centre, w, d)) = self.model.add_profile(footprint) else {
            return None;
        };
        let void_h = (top - base).max(0.1) + 0.02; // punch fully through the building
        let placement = Placement { u: centre.x, v: centre.y, lift: base, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 };
        self.model.push(
            BoolOp::Difference,
            Plane::default(),
            placement,
            Primitive::Extrusion { profile, h: void_h, w, d },
        );
        // `push` appended at the end; bind it to the building and move it in behind, so the
        // difference cuts the BUILDING body and nothing else — and goes on cutting that same
        // body if anything later reorders the list.
        let mut void_id = None;
        if let Some(void) = self.model.features.pop() {
            void_id = Some(void.id);
            // CALL FIRST, ASSERT SECOND. `debug_assert!(expr)` does not EVALUATE `expr` in a
            // release build — it compiles to nothing — so wrapping the insert in one would make
            // the void never get inserted in the shipped binary. `idx` was read out of `features`
            // a few lines up, so the host really is present and the assert is a guard, not a path.
            let placed = self.model.insert_after(building_id, void);
            debug_assert!(placed, "the building was read from the feature list a moment ago");
        }
        self.dirty = true;
        void_id.map(|v| (v, building_id))
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
        let (carve, carved_from) = match self.carve_interior_from_building(footprint, base) {
            Some((void, building)) => (Some(void), Some(building)),
            None => (None, None),
        };

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
        //
        // ONLY when the room was NOT carved from a building. If it was, the material left around
        // the void already IS the wall, and adding a second ring inside it produced two concentric
        // walls at different heights — the building's full height against the room's floor + clear
        // — which is the step that made rooms look lopsided.
        let wall_base = base + floor_t;
        let mut wall_ids = Vec::new();
        if carve.is_none() {
            for e in footprint.windows(2) {
                if let Some(id) = self.push_wall_box(e[0], e[1], wall_t, h, wall_base) {
                    self.feature_color.insert(id, WALL_COL);
                    wall_ids.push(id);
                }
            }
            // Close the loop if the outline wasn't already closed.
            if footprint.len() >= 3 {
                let (a, b) = (footprint[footprint.len() - 1], footprint[0]);
                if (a - b).length() > 1e-4 {
                    if let Some(id) = self.push_wall_box(a, b, wall_t, h, wall_base) {
                        self.feature_color.insert(id, WALL_COL);
                        wall_ids.push(id);
                    }
                }
            }
        }

        // CEILING slab on top of the walls, tracked so it can be hidden — unless open sky.
        let mut ceiling_id = None;
        if !self.room_open_top {
            let ct = self.ceiling_thickness.max(0.02);
            if let Some(cid) = self.add_slab(footprint, ct, wall_base + h + ct) {
                self.feature_color.insert(cid, CEIL_COL);
                self.ceilings.insert(cid);
                ceiling_id = Some(cid);
            }
        }

        // REGISTER the room, so it can be found, named, re-heighted and deleted as one thing.
        let rid = self.next_room_id;
        self.next_room_id += 1;
        self.rooms.push(RoomInst {
            id: rid,
            name: format!("Room {rid}"),
            footprint: footprint.to_vec(),
            base_z: base,
            height: h,
            floor_t,
            ceiling_t: self.ceiling_thickness.max(0.02),
            wall_t,
            open_top: self.room_open_top,
            floor: Some(floor_id),
            walls: wall_ids.clone(),
            ceiling: ceiling_id,
            carve,
        });

        // A ROOM IS ONE OBJECT.
        //
        // Reported as: "when i move a building its floor and ceiling stay in place — the ceiling
        // and floor are part of the building so they should stay attached to it." They were built
        // as separate CSG features and nothing tied them together, so Move took whichever one the
        // click had landed on and left the rest behind. The dump shows the cost exactly: a 20 m
        // building whose model AABB had stretched to 69.31 m after two moves, because the parts
        // had walked away from each other.
        //
        // Grouping is the mechanism that already exists for this — members "select / move /
        // delete as one entity" — and using it means no new movement code, no second rule about
        // what moves with what, and Explode still works for anyone who wants the parts apart.
        // …INCLUDING THE BUILDING IT WAS CARVED OUT OF, which is what the first attempt at this
        // missed. The user's dump picked `feature#1` — the building SHELL — and moved it, and the
        // shell was not in the room's group because only the room's own features were. A room cut
        // from a building and the mass around it are one object; grouping half of them fixed
        // nothing for the half that gets clicked.
        let mut parts = vec![floor_id];
        parts.extend(wall_ids);
        parts.extend(ceiling_id);
        parts.extend(carve);
        parts.extend(carved_from);
        self.group_features(&parts);

        self.selection = vec![floor_id];
        self.dirty = true;
        // What the room actually occupies, versus the number that was typed.
        //
        // `room_height` is the CLEAR height, so the structure is always taller than it by the two
        // slabs. When that overruns the building the ceiling stands proud of the top, and the only
        // evidence is a picture that looks wrong — which is exactly how it was reported. Stated
        // here so the answer is in the status line and the history, not only in a menu that has
        // since been closed.
        let ct = if self.room_open_top { 0.0 } else { self.ceiling_thickness.max(0.02) };
        let overall = floor_t + h + ct;
        self.status = format!(
            "Room: {} clear, {} overall ({} floor + {} clear{}).{}",
            length_str(self.units, h),
            length_str(self.units, overall),
            length_str(self.units, floor_t),
            length_str(self.units, h),
            if ct > 0.0 { format!(" + {} ceiling", length_str(self.units, ct)) } else { String::new() },
            if overall > self.effective_building_height() + 1e-4 {
                format!(
                    "  ⚠ {} taller than the {} building — the room stands proud of the top. \
                     Room height is the CLEAR height; the slabs are extra.",
                    length_str(self.units, overall - self.effective_building_height()),
                    length_str(self.units, self.effective_building_height()),
                )
            } else {
                String::new()
            },
        );
        Ok(floor_id)
    }

    /// Index of a room by id.
    pub fn room_index(&self, id: u32) -> Option<usize> {
        self.rooms.iter().position(|r| r.id == id)
    }

    /// Rename a room. The name is what appears on the plan and what openings are grouped under.
    pub fn rename_room(&mut self, id: u32, name: &str) {
        if let Some(i) = self.room_index(id) {
            let n = name.trim();
            // An empty name would leave a room unlabelled on the plan and unfindable in the
            // openings list, so it falls back to something addressable rather than to nothing.
            self.rooms[i].name = if n.is_empty() { format!("Room {id}") } else { n.to_string() };
        }
    }

    /// Change a built room's CLEAR height, in place.
    ///
    /// The thing that was impossible: a room's height was fixed the moment it was drawn, and the
    /// only way to change it was to delete every piece and draw it again — losing any window
    /// already cut into its walls.
    ///
    /// Each wall is a Box whose `h` is its height, placed at the wall base, so resizing grows it
    /// upward from the floor. The ceiling is an Extrusion lifted so its TOP lands at a given z, so
    /// it moves by its lift alone. Neither touches a feature id, and that is what lets cuts and
    /// apertures already in those walls survive the edit.
    pub fn set_room_height(&mut self, id: u32, height: f32) {
        let Some(i) = self.room_index(id) else { return };
        let h = height.max(0.05);
        if (self.rooms[i].height - h).abs() < 1e-6 {
            return;
        }
        self.rooms[i].height = h;
        let (walls, ceiling, carve, base_z, floor_t, ct, wall_t) = {
            let r = &self.rooms[i];
            (r.walls.clone(), r.ceiling, r.carve, r.base_z, r.floor_t, r.ceiling_t, r.wall_t)
        };
        for fid in walls {
            if let Some(f) = self.model.get_mut(fid) {
                if let Primitive::Box { w, .. } = f.primitive {
                    f.primitive = Primitive::Box { w, d: wall_t, h };
                }
            }
        }
        // A CARVED room's height is the void's height — the building around it is the wall, so
        // raising the room means cutting further up through that building.
        if let Some(cid) = carve {
            if let Some(f) = self.model.get_mut(cid) {
                if let Primitive::Extrusion { profile, w, d, h: was } = f.primitive {
                    // GROW ONLY. The void was cut to punch clear THROUGH the building, which is
                    // what turns the building into a wall ring rather than a cap sitting over the
                    // room. Re-sizing it to the room's own height would put that cap back — a test
                    // caught exactly that. So it only ever reaches further, when the room outgrows
                    // the void it was given.
                    let need = floor_t + h + ct + 0.02;
                    f.primitive = Primitive::Extrusion { profile, h: was.max(need), w, d };
                }
            }
        }
        if let Some(cid) = ceiling {
            // The ceiling's TOP sits at wall_base + h + ct, and an extrusion rises +Z from its
            // lift — so the lift is that top less its own thickness.
            let top = base_z + floor_t + h + ct;
            if let Some(f) = self.model.get_mut(cid) {
                f.placement.lift = top - ct;
            }
        }
        self.dirty = true;
        self.status = format!(
            "{}: clear height {} · {} overall.",
            self.rooms[i].name,
            length_str(self.units, h),
            length_str(self.units, self.rooms[i].overall_height()),
        );
    }

    /// Change a built room's FLOOR thickness. The walls stand on the slab, so everything above it
    /// moves with it.
    pub fn set_room_floor(&mut self, id: u32, thickness: f32) {
        let Some(i) = self.room_index(id) else { return };
        let t = thickness.max(0.02);
        if (self.rooms[i].floor_t - t).abs() < 1e-6 {
            return;
        }
        self.rooms[i].floor_t = t;
        self.rebuild_room_levels(id);
    }

    /// Change a built room's CEILING thickness.
    pub fn set_room_ceiling(&mut self, id: u32, thickness: f32) {
        let Some(i) = self.room_index(id) else { return };
        let t = thickness.max(0.02);
        if (self.rooms[i].ceiling_t - t).abs() < 1e-6 {
            return;
        }
        self.rooms[i].ceiling_t = t;
        self.rebuild_room_levels(id);
    }

    /// Re-seat a room's slabs and walls after a thickness change.
    ///
    /// The floor's TOP is the level everything else is measured from, so changing its thickness
    /// moves the wall bases and the ceiling with it — otherwise a thicker floor would swallow the
    /// bottom of the walls and leave the ceiling where it was.
    fn rebuild_room_levels(&mut self, id: u32) {
        let Some(i) = self.room_index(id) else { return };
        let (floor, walls, ceiling, carve, base_z, floor_t, h, ct) = {
            let r = &self.rooms[i];
            (r.floor, r.walls.clone(), r.ceiling, r.carve, r.base_z, r.floor_t, r.height, r.ceiling_t)
        };
        if let Some(fid) = floor {
            if let Some(f) = self.model.get_mut(fid) {
                if let Primitive::Extrusion { profile, w, d, .. } = f.primitive {
                    f.primitive = Primitive::Extrusion { profile, h: floor_t, w, d };
                    f.placement.lift = base_z; // top lands at base + floor_t
                }
            }
        }
        let wall_base = base_z + floor_t;
        for fid in walls {
            if let Some(f) = self.model.get_mut(fid) {
                f.placement.lift = wall_base;
            }
        }
        if let Some(cid) = ceiling {
            if let Some(f) = self.model.get_mut(cid) {
                if let Primitive::Extrusion { profile, w, d, .. } = f.primitive {
                    f.primitive = Primitive::Extrusion { profile, h: ct, w, d };
                    f.placement.lift = wall_base + h;
                }
            }
        }
        if let Some(cid) = carve {
            if let Some(f) = self.model.get_mut(cid) {
                if let Primitive::Extrusion { profile, w, d, .. } = f.primitive {
                    f.primitive =
                        Primitive::Extrusion { profile, h: (floor_t + h + ct + 0.02).max(0.1), w, d };
                    f.placement.lift = base_z;
                }
            }
        }
        self.dirty = true;
    }

    /// A clear height that FITS the building — what to suggest rather than making the user work it
    /// out from three numbers that interact.
    ///
    /// The structure is `floor + clear + ceiling`, so the clear height a building can take is what
    /// is left of it after the two slabs. Never below a usable room, so a building too short for
    /// one says so through the warning rather than by silently proposing a crawlspace.
    pub fn suggested_room_height(&self) -> f32 {
        let ct = if self.room_open_top { 0.0 } else { self.ceiling_thickness.max(0.02) };
        (self.effective_building_height() - self.room_floor.max(0.02) - ct).max(2.1)
    }

    /// Top of the building that is ACTUALLY STANDING, measured from the model.
    ///
    /// `building_height` is a TEMPLATE — the height the NEXT building is raised to — and it keeps
    /// no connection to one already built. Resize a building through its own height field and the
    /// geometry changes while the template does not, so every check written against the template
    /// compares a room to a building that no longer exists: a 4 m building reported as 3 m, and a
    /// room that fits it warned as standing 1000 mm proud of the top.
    ///
    /// The same test the carve uses to find its target: a Union solid too tall to be a slab.
    /// `None` when nothing is built, so the caller falls back to the template — which at that
    /// point is the only statement about the building there is.
    pub fn building_top(&self) -> Option<f32> {
        // A ROOM'S OWN WALLS ARE NOT THE BUILDING. Without this a free-standing room measures
        // itself: its wall boxes are Union solids well over the slab threshold, so a room with no
        // building around it reported its own wall top as the height it had to fit inside — and
        // then warned that it did not. A test caught exactly that.
        let owned: std::collections::HashSet<u32> =
            self.rooms.iter().flat_map(|r| self.room_features(r.id)).collect();
        let mut top: Option<f32> = None;
        for f in &self.model.features {
            if f.op != BoolOp::Union || owned.contains(&f.id) {
                continue;
            }
            let (mn, mx) = f.world_aabb();
            if (mx.z - mn.z) <= 0.5 {
                continue; // a thin slab is a floor or a ceiling, not a mass
            }
            top = Some(top.map_or(mx.z, |t: f32| t.max(mx.z)));
        }
        top
    }

    /// The height a room is judged against: what is standing, or the template when nothing is.
    pub fn effective_building_height(&self) -> f32 {
        self.building_top().unwrap_or(self.building_height)
    }

    /// Raise or lower the building ALREADY BUILT, from the template field.
    ///
    /// The counterpart to [`Self::set_room_height`], and missing for the same reason: the height
    /// was fixed at the moment of creation. Any room carved out of the building has its void grown
    /// to match, so a building made taller cannot seal itself back over its own rooms.
    ///
    /// Returns how many masses were resized.
    pub fn set_building_height(&mut self, height: f32) -> usize {
        let h = height.max(0.05);
        let ids: Vec<u32> = self
            .model
            .features
            .iter()
            .filter(|f| f.op == BoolOp::Union)
            .filter(|f| {
                let (mn, mx) = f.world_aabb();
                (mx.z - mn.z) > 0.5
            })
            .map(|f| f.id)
            .collect();
        for id in &ids {
            if let Some(f) = self.model.get_mut(*id) {
                match f.primitive {
                    Primitive::Extrusion { profile, w, d, .. } => {
                        f.primitive = Primitive::Extrusion { profile, h, w, d };
                    }
                    Primitive::Box { w, d, .. } => {
                        f.primitive = Primitive::Box { w, d, h };
                    }
                    _ => {}
                }
            }
        }
        let rooms: Vec<(Option<u32>, f32)> = self.rooms.iter().map(|r| (r.carve, r.base_z)).collect();
        for (carve, base_z) in rooms {
            if let Some(cid) = carve {
                if let Some(f) = self.model.get_mut(cid) {
                    if let Primitive::Extrusion { profile, w, d, h: was } = f.primitive {
                        let need = (h - base_z).max(0.1) + 0.02;
                        f.primitive = Primitive::Extrusion { profile, h: was.max(need), w, d };
                    }
                }
            }
        }
        self.building_height = h;
        self.dirty = true;
        self.status = format!(
            "Building height {} — {} mass(es) resized.",
            length_str(self.units, h),
            ids.len(),
        );
        ids.len()
    }

    /// Every feature a room owns — what a delete has to take, and what selecting it should cover.
    pub fn room_features(&self, id: u32) -> Vec<u32> {
        let Some(i) = self.room_index(id) else { return Vec::new() };
        let r = &self.rooms[i];
        r.floor.iter().chain(r.ceiling.iter()).copied().chain(r.walls.iter().copied()).collect()
    }

    /// The room whose outline contains `p` — how an opening finds the room it is in.
    ///
    /// Smallest-first, so a room inside a larger space (a store within a hall) wins over the space
    /// enclosing it. Without that the outer room would claim every opening in the building.
    pub fn room_at(&self, p: Vec2) -> Option<u32> {
        let mut best: Option<(f32, u32)> = None;
        for r in &self.rooms {
            if !r.contains(p) {
                continue;
            }
            let n = r.footprint.len();
            let mut a2 = 0.0f32;
            for k in 0..n {
                let (u, v) = (r.footprint[k], r.footprint[(k + 1) % n]);
                a2 += u.x * v.y - v.x * u.y;
            }
            let area = a2.abs() * 0.5;
            if best.is_none_or(|(b, _)| area < b) {
                best = Some((area, r.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Drop a room: its geometry, its void, and its record, as one act.
    ///
    /// Removing the CARVE is what makes the area buildable again. Deleting only the room's own
    /// pieces left the Difference feature still cutting the building, so the floor plate kept a
    /// permanent hole where the room had been and nothing could be put back there.
    pub fn delete_room(&mut self, id: u32) {
        let carve = self.room_index(id).and_then(|i| self.rooms[i].carve);
        for f in self.room_features(id).into_iter().chain(carve) {
            self.model.remove(f);
            self.feature_color.remove(&f);
            self.ceilings.remove(&f);
            self.feature_group.remove(&f);
        }
        let name = self
            .room_index(id)
            .map(|i| self.rooms[i].name.clone())
            .unwrap_or_else(|| format!("Room {id}"));
        self.rooms.retain(|r| r.id != id);
        self.selection.clear();
        self.dirty = true;
        self.status = format!("Deleted {name}.");
    }

    /// The OPENINGS inside a room — indices into `furniture`, for apertures whose position falls
    /// within that room's outline.
    ///
    /// What makes an openings list say which room each one belongs to. An aperture is placed in a
    /// wall, and a wall is shared between two rooms, so this attributes it by where the piece
    /// actually sits — the smaller room wins, via [`Self::room_at`].
    pub fn openings_in_room(&self, id: u32) -> Vec<usize> {
        let Some(i) = self.room_index(id) else { return Vec::new() };
        let r = &self.rooms[i];
        (0..self.furniture.len())
            .filter(|&k| self.is_aperture(k))
            .filter(|&k| {
                let p = self.furniture[k].pos;
                r.contains(Vec2::new(p[0], p[1]))
            })
            .collect()
    }

    /// Every aperture that falls in no room at all — in an external wall, or outside the model.
    ///
    /// Listed rather than dropped: an opening the app cannot place is exactly the one a user needs
    /// to be told about, and silently omitting it from a grouped list would read as "there are
    /// none".
    pub fn openings_without_a_room(&self) -> Vec<usize> {
        (0..self.furniture.len())
            .filter(|&k| self.is_aperture(k))
            .filter(|&k| {
                let p = self.furniture[k].pos;
                self.room_at(Vec2::new(p[0], p[1])).is_none()
            })
            .collect()
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

    /// Select everything in the model — every solid feature and every placed piece.
    pub fn select_all(&mut self) {
        self.selection = self.model.features.iter().map(|f| f.id).collect();
        self.sel_furniture = (0..self.furniture.len()).collect();
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
                    emitters: a
                        .emitters
                        .iter()
                        .map(|e| [e.pos[0] as f64, e.pos[1] as f64, e.pos[2] as f64, e.lumens, e.watts])
                        .collect(),
                    cct_k: a.cct_k,
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
            rooms: self
                .rooms
                .iter()
                .map(|r| crate::simlux_io::RoomRec {
                    id: r.id,
                    name: r.name.clone(),
                    footprint: r.footprint.iter().map(|p| [p.x, p.y]).collect(),
                    base_z: r.base_z,
                    height: r.height,
                    floor_t: r.floor_t,
                    ceiling_t: r.ceiling_t,
                    wall_t: r.wall_t,
                    open_top: r.open_top,
                    floor: r.floor,
                    walls: r.walls.clone(),
                    ceiling: r.ceiling,
                    carve: r.carve,
                })
                .collect(),
            next_room_id: self.next_room_id,
            working_unit_m: self.units.metres_per_unit,
            place_mode: Some(self.place_mode),
            place_offset: Some(self.place_offset),
            unit_asked: Some(self.unit_asked),
            // FACE PLANES. `Model::sketches` is `#[serde(skip)]`, so without this a named view and
            // everything drawn on it is gone the moment the project is reopened — which would make
            // a list you go back to worse than no list at all.
            //
            // The drawing goes out as RSM, the app's own 2D format, rather than a bespoke
            // serialization of `cad_kernel::Document` (which derives no serde at all). That reuses
            // the reader and writer the File menu already depends on.
            sketches: self
                .model
                .sketches
                .iter()
                .map(|s| crate::simlux_io::SketchRec {
                    name: s.name.clone(),
                    origin: s.frame.origin.to_array(),
                    u: s.frame.u.to_array(),
                    v: s.frame.v.to_array(),
                    rsm_b64: {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD
                            .encode(cad_io::rsm::write_rsm(&s.doc))
                    },
                })
                .collect(),
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
                fa.emitters = a
                    .emitters
                    .iter()
                    .map(|e| FurnEmitter { pos: [e[0] as f32, e[1] as f32, e[2] as f32], lumens: e[3], watts: e[4] })
                    .collect();
                fa.cct_k = a.cct_k;
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
        // The id counters are `#[serde(default)]`, so every project saved before they existed
        // loads with them at 0 — and the next feature drawn would be handed id 1, which the loaded
        // model already uses. That new object would inherit the existing one's colour, texture and
        // group, because all of those are keyed by feature id. Raise the floor before anything can
        // allocate.
        self.model.reserve_ids_above_loaded();
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
        // The working unit, if the sidecar recorded one. A project reopens showing the numbers it
        // was authored with; a sidecar written before this existed says nothing, and the default
        // stands rather than the geometry being reinterpreted.
        if d.working_unit_m > 0.0 {
            self.units = cad_kernel::DocUnits::new(d.working_unit_m, cad_kernel::UnitSource::User);
        }
        // Placement preference. Absent from a file written before it existed, and then the default
        // stands — the same rule as the working unit above.
        if let Some(m) = d.place_mode {
            self.place_mode = m;
        }
        // A project that RECORDS a unit has answered the question, whether or not the flag exists:
        // every file written before the flag did carries `working_unit_m`, and re-asking someone who
        // already told us is exactly the "shows every time" the dialog is meant to avoid.
        self.unit_asked = d.unit_asked.unwrap_or(d.working_unit_m > 0.0);
        if let Some(o) = d.place_offset {
            self.place_offset = o;
        }
        // FACE PLANES and their drawings. A file written before these were saved carries none, and
        // then the model simply has no planes yet — which is exactly what it had before.
        //
        // IDS ARE MINTED FRESH, through `push_sketch`. Nothing persisted names a plane — the open
        // sketch and the rename dialog are both session state — so an id only has to be unique
        // within this run. Assigning the vector directly would leave every plane holding
        // `Sketch::new`'s id 0, and a reference to plane 0 would resolve to whichever of them the
        // search reached first.
        self.model.sketches.clear();
        self.model.next_sketch_id = 0;
        for r in &d.sketches {
            let mut sk = cad_solid::Sketch::new(Frame {
                origin: Vec3::from(r.origin),
                u: Vec3::from(r.u),
                v: Vec3::from(r.v),
            });
            sk.name = r.name.clone();
            // An unreadable drawing costs the DRAWING, not the plane: a named face with nothing
            // on it can be drawn on again, where a dropped plane is a view that silently went
            // missing.
            use base64::Engine;
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&r.rsm_b64) {
                if let Ok(doc) = cad_io::rsm::read_rsm(&bytes) {
                    sk.doc = doc;
                }
            }
            self.model.push_sketch(sk);
        }
        // ROOMS. A feature the model no longer holds is dropped from the room rather than left
        // dangling — a room pointing at geometry that is not there would resize nothing and
        // delete nothing, which is worse than a room that knows it has lost a wall.
        self.rooms = d
            .rooms
            .into_iter()
            .map(|r| RoomInst {
                id: r.id,
                name: r.name,
                footprint: r.footprint.iter().map(|p| Vec2::new(p[0], p[1])).collect(),
                base_z: r.base_z,
                height: r.height,
                floor_t: r.floor_t,
                ceiling_t: r.ceiling_t,
                wall_t: r.wall_t,
                open_top: r.open_top,
                floor: r.floor.filter(|f| have.contains(f)),
                walls: r.walls.into_iter().filter(|f| have.contains(f)).collect(),
                ceiling: r.ceiling.filter(|f| have.contains(f)),
                carve: r.carve.filter(|f| have.contains(f)),
            })
            .collect();
        let highest = self.rooms.iter().map(|r| r.id).max().unwrap_or(0);
        self.next_room_id = d.next_room_id.max(highest + 1);
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
        let placement = self.placement_for(&p);
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.selection = vec![id];
        self.arm_placement(AwaitingPlace::Feature(id));
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
        let placement = self.placement_for(&p);
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.selection = vec![id];
        self.arm_placement(AwaitingPlace::Feature(id));
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
    ///
    /// AN OPENING NEVER CHANGES WALLS HERE, and that is the whole difficulty of the function.
    ///
    /// MECHANISM. This removes the wall's segment features and rebuilds them, and the rebuild
    /// APPENDS (`Model::push` adds at the end). Openings are `Difference` features that must sit
    /// directly behind the body they cut — `csg::eval` folds each Difference onto the most recent
    /// Union, and `factory_cut_sketch` relies on that explicitly. So moving a wall out from under
    /// its cutters would re-bind them to whichever Union now precedes them: the NEIGHBOURING
    /// wall. Silently, on an ordinary handle drag.
    ///
    /// TWO CASES, BECAUSE THE MAPPING IS NOT ALWAYS 1:1.
    ///   `wall_move_vertex`   segment COUNT is unchanged, so the rebuilt features go back at the
    ///                        indices the removed ones held and the arrangement is restored
    ///                        exactly. Nothing is lifted, nothing is re-homed.
    ///   `wall_insert_vertex` / `wall_delete_vertex`   the count CHANGES — a segment splits in
    ///                        two, or two merge — so there is no index mapping at all. The
    ///                        openings are lifted out and re-homed GEOMETRICALLY: each one goes
    ///                        behind whichever rebuilt segment still contains it.
    ///
    /// AN OPENING THAT FITS NO SEGMENT IS KEPT AND FLAGGED (owner's decision, 2026-08-15): it
    /// stays in the model with [`cad_solid::Feature::enabled`] cleared, so it is not applied and
    /// not re-bound, and [`Self::orphaned_cutouts`] surfaces it — as a red box in the 3D overlay
    /// and a ⚠ row in the Openings menu. Deleting it would destroy the user's window; re-binding
    /// it would put that window in the wrong wall. A missing window is invisible and an
    /// intact-looking wall is not, so it must be said out loud rather than shown.
    ///
    /// DO NOT take the cheap fix of "removing a Union also removes the Differences after it".
    /// That upgrades a mis-binding into DELETING EVERY WINDOW IN THE WALL on a drag.
    fn rederive_wall(&mut self, wi: usize) {
        if wi >= self.walls.len() {
            return;
        }
        // WHERE THE OLD SEGMENTS SAT, so the rebuilt ones can go back there. Ascending, because
        // re-inserting in ascending order at the original indices reconstructs the original
        // arrangement exactly: each insert shifts only what follows it, so by the time the k-th
        // index is used every earlier hole has already been refilled.
        let old: Vec<u32> = std::mem::take(&mut self.walls[wi].segments);
        let mut slots: Vec<usize> = old
            .iter()
            .filter_map(|id| self.model.features.iter().position(|f| f.id == *id))
            .collect();
        slots.sort_unstable();

        // THE OPENINGS THIS WALL HOSTS, PER SEGMENT, read off BEFORE anything moves — because
        // after the removal there is nothing left in the model that says which body a cutter
        // belonged to. A cutter is hosted by the nearest Union above it, so a segment's openings
        // are the unbroken run of non-Union features that follows it.
        //
        // Kept per segment rather than flattened, because the rebuilt segments carry NEW ids
        // (`push_wall_box` mints one each) and every opening has to be told which of them it now
        // belongs to. Same ascending order as `slots`, so index k means the same segment in both.
        let hosted_by: Vec<Vec<u32>> = slots
            .iter()
            .map(|&i| {
                self.model.features[i + 1..]
                    .iter()
                    .take_while(|f| f.op != cad_solid::BoolOp::Union)
                    .map(|f| f.id)
                    .collect::<Vec<_>>()
            })
            .collect();
        let hosted: Vec<u32> = hosted_by.iter().flatten().copied().collect();

        for id in old {
            self.model.remove(id);
        }
        let fp = self.walls[wi].footprint.clone();
        let (t, h) = (self.walls[wi].thickness, self.walls[wi].height);
        // Rebuild at the wall's OWN base, not the active storey's: editing a vertex on
        // the third floor must not drop the wall to the ground.
        let base_z = self.walls[wi].base_z;
        let mut segments = Vec::new();
        // The rebuilt segments WITH THE EDGE EACH ONE SPANS, which is what re-homing tests
        // against. A degenerate edge is skipped, so a footprint index is not a segment index —
        // the span has to be carried, not recomputed from the footprint later.
        let mut spans: Vec<(Vec2, Vec2)> = Vec::new();
        for w in fp.windows(2) {
            if let Some(id) = self.push_wall_box(w[0], w[1], t, h, base_z) {
                segments.push(id);
                spans.push((w[0], w[1]));
            }
        }

        if !segments.is_empty() && slots.len() == segments.len() {
            // PUT THEM BACK WHERE THEY WERE. The count matched, so the mapping is exact and the
            // openings never had to move at all — restoring the segments to their own indices
            // restores every binding with them.
            let tail = self.model.features.len() - segments.len();
            let rebuilt: Vec<_> = self.model.features.drain(tail..).collect();
            // AND EACH OPENING NOW NAMES ITS NEW SEGMENT. Restoring the positions restores the
            // positional binding, but a cutter that names a body names it by ID — and every
            // rebuilt segment has a fresh one, so an opening still naming the segment it was cut
            // in would open nothing and the wall would come back solid. The binding has to be
            // carried across the rebuild exactly as the position is.
            for (k, ids) in hosted_by.iter().enumerate() {
                let Some(&seg) = segments.get(k) else { continue };
                for &c in ids {
                    self.model.set_target(c, Some(seg));
                }
            }
            for (slot, feat) in slots.into_iter().zip(rebuilt) {
                self.model.insert_at(slot, feat);
            }
        } else if !hosted.is_empty() {
            // THE COUNT CHANGED, so re-home by geometry — the only thing still true about an
            // opening once the segment it named no longer exists is WHERE IT IS.
            //
            // Lifting first is what makes this safe. The cutters are pulled out of the list
            // before the wall is reassembled, so at no point does a cutter sit behind a body
            // that is not its own: it is either lifted, or placed behind a segment that contains
            // it, or flagged. There is no intermediate state that a re-entrant caller could
            // observe as a wrong binding.
            let mut lifted: Vec<cad_solid::Feature> = Vec::with_capacity(hosted.len());
            for id in &hosted {
                if let Some(i) = self.model.features.iter().position(|f| f.id == *id) {
                    lifted.push(self.model.features.remove(i));
                }
            }
            // The rebuilt segments are the tail: `push_wall_box` appended them and the lift only
            // removed features from before them.
            let tail = self.model.features.len() - segments.len();
            let rebuilt: Vec<cad_solid::Feature> = self.model.features.drain(tail..).collect();

            let mut per_seg: Vec<Vec<cad_solid::Feature>> = vec![Vec::new(); rebuilt.len()];
            let mut orphans: Vec<cad_solid::Feature> = Vec::new();
            for mut cut in lifted {
                let (mn, mx) = cut.world_aabb();
                match Self::segment_containing(&spans, (mn + mx) * 0.5, t, h, base_z) {
                    Some(k) => {
                        // Re-homed BY NAME as well as by position — the segment it used to name
                        // no longer exists, so leaving the old target would turn a re-homed
                        // opening into no opening at all.
                        cut.target = rebuilt.get(k).map(|f| f.id);
                        per_seg[k].push(cut);
                    }
                    None => {
                        cut.enabled = false;
                        // NO OPINION, rather than a wrong one. It named a segment that is gone;
                        // naming a different one would be a claim nothing supports, so it goes
                        // back to the positional rule — which, resting at the end of this wall's
                        // own block, is what the comment below describes.
                        cut.target = None;
                        orphans.push(cut);
                    }
                }
            }
            let orphaned = orphans.len();

            // Reassemble the wall's block: every segment immediately followed by its own
            // openings, in the order they were made.
            for (k, seg) in rebuilt.into_iter().enumerate() {
                self.model.features.push(seg);
                self.model.features.append(&mut per_seg[k]);
            }
            // The orphans rest at the END OF THIS WALL'S BLOCK, not the end of the model. They
            // are disabled, so today they bind to nothing — but if one is ever re-enabled the
            // worst it can do is cut the wrong segment of its OWN wall, rather than a stranger's.
            self.model.features.append(&mut orphans);

            if orphaned > 0 {
                self.status = format!(
                    "{orphaned} opening(s) lost their wall segment — kept and flagged, not applied"
                );
            }
        }

        self.walls[wi].segments = segments;
        self.dirty = true;
    }

    /// Which rebuilt segment, if any, still contains the world point `p` — the re-homing test.
    ///
    /// `p` is an opening's world-AABB centre, which sits INSIDE the wall for both kinds of cut: a
    /// through-cut spans the full thickness so its centre is on the centreline, and a blind
    /// recess runs from one face inward so its centre is between that face and the centreline.
    /// Either way it is within half a thickness of the centreline, which is the test.
    ///
    /// Returns the CLOSEST qualifying segment. At a corner two segments both contain the point,
    /// and the one the opening is actually cut into is the one it is squarely inside.
    fn segment_containing(
        spans: &[(Vec2, Vec2)], p: Vec3, thickness: f32, height: f32, base_z: f32,
    ) -> Option<usize> {
        const TOL: f32 = 1e-3; // a millimetre, in a model measured in metres
        // Height first: a wall directly above this one, on the next storey, lines up perfectly in
        // plan. Only the z band tells the two apart.
        if p.z < base_z - TOL || p.z > base_z + height + TOL {
            return None;
        }
        let q = Vec2::new(p.x, p.y);
        let half = thickness * 0.5 + TOL;
        let mut best: Option<(f32, usize)> = None;
        for (k, (a, b)) in spans.iter().enumerate() {
            let d = *b - *a;
            let len = d.length();
            if len < 1e-4 {
                continue;
            }
            // Parametric position along the edge, with the tolerance expressed in metres rather
            // than as a fraction — otherwise a short segment gets a slacker test than a long one.
            let t = (q - *a).dot(d) / (len * len);
            let slack = TOL / len;
            if t < -slack || t > 1.0 + slack {
                continue;
            }
            let perp = ((q - *a) - d * t).length();
            if perp > half {
                continue;
            }
            if best.is_none_or(|(bp, _)| perp < bp) {
                best = Some((perp, k));
            }
        }
        best.map(|(_, k)| k)
    }

    /// True while an opening is open for 2D reshape — what the sketch panel's banner and its
    /// ✔ Apply / ✖ Cancel buttons key off. The state itself is in [`Self::cutout_edit`].
    pub fn editing_cutout(&self) -> bool {
        self.cutout_edit.is_some()
    }

    /// Every opening that lost its host wall segment: kept in the model, NOT applied, and needing
    /// the user's decision. See [`Self::rederive_wall`] — these are the flagged ones.
    ///
    /// A disabled `Difference` is not drawn as a hole, so nothing in the scene would otherwise say
    /// it exists. This is what the 3D overlay marks and the Openings menu flags.
    pub fn orphaned_cutouts(&self) -> Vec<u32> {
        self.model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Difference && !f.enabled)
            .map(|f| f.id)
            .collect()
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
            // EVERY SIDE MAP KEYED BY THIS ID, or whatever is given the id next inherits what was
            // left behind. Four of the six were missing, and under the old `max + 1` allocator the
            // id WAS handed out again the moment you deleted the newest feature — so deleting a
            // coloured object and drawing another made the new one wear the dead one's paint.
            //
            // The allocator no longer recycles ids, which removes the symptom. These stay because
            // a leak that only shows when something else regresses is the kind that comes back:
            // without them the maps also grow for the whole session, and `SurfaceKey`'s first
            // field is the feature id, so a stale surface entry outlives the surface itself.
            self.ceilings.remove(&id); // keep the ceiling set in step with the model
            self.feature_group.remove(&id); // drop it from any group
            self.feature_color.remove(&id);
            self.feature_texture.remove(&id);
            self.surface_color.retain(|k, _| k.0 != id);
            self.surface_texture.retain(|k, _| k.0 != id);
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

    /// Scale the ENTIRE 3D scene uniformly about the world origin.
    ///
    /// This is what makes a declared unit useful on a project that was BUILT before the unit
    /// was known: promotion has been fixed for new work, but solids already in the model were
    /// built reading millimetres as metres and are 1000x too large. Nothing does this
    /// automatically — a file that opens must never silently move — so it is an explicit,
    /// undoable action the user asks for.
    ///
    /// Scales the CSG model, wall footprints/heights/thicknesses, storey heights and furniture
    /// placement. Furniture SIZE scales too: the point is a similarity transform of the whole
    /// scene, and a piece that was enlarged to match an oversized building has to come back
    /// down with it.
    ///
    /// Re-keys the per-face paint. `surface_key` quantises `d = n·a`, the plane's WORLD offset,
    /// so every colour and texture assignment would orphan the moment the geometry moved.
    /// (`paste_clipboard` does the same re-keying for the translation a paste applies.)
    pub fn rescale_world(&mut self, k: f32) {
        if !(k.is_finite() && k > 0.0) || (k - 1.0).abs() < f32::EPSILON {
            return;
        }
        self.model.rescale(k);
        for w in &mut self.walls {
            for p in &mut w.footprint {
                *p *= k;
            }
            w.thickness *= k;
            w.height *= k;
            w.base_z *= k;
        }
        for s in &mut self.storeys {
            s.height *= k;
        }
        for f in &mut self.furniture {
            f.pos = [f.pos[0] * k, f.pos[1] * k, f.pos[2] * k];
            f.scale *= k;
        }
        // Re-key the per-face paint: only the plane-offset component of the key moves, and it
        // moves by exactly `k` (the normal is a unit direction, so its quantised components
        // are unchanged). Rebuilt into fresh maps because two distinct old keys can quantise
        // onto the same new one after scaling down.
        fn rekey<V: Clone>(
            m: &std::collections::HashMap<SurfaceKey, V>,
            k: f32,
        ) -> std::collections::HashMap<SurfaceKey, V> {
            m.iter()
                .map(|(&(fid, nx, ny, nz, d), v)| {
                    let d_world = d as f32 / 100.0;
                    ((fid, nx, ny, nz, (d_world * k * 100.0).round() as i32), v.clone())
                })
                .collect()
        }
        self.surface_color = rekey(&self.surface_color, k);
        self.surface_texture = rekey(&self.surface_texture, k);
        self.recompute();
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
        // …AND SO HAS ANY OPEN SKETCH'S REFERENCE OUTLINE.
        //
        // Reported as: "when i move a building the building outline moves in the global view but
        // not in any other view — if i pick a face to draw and move the building while doing it,
        // the outline in 2D stays in the same place. Only after finishing the sketch and opening
        // it again does it fix itself." The outline was projected ONCE, when the sketch was
        // entered, and never again: a photograph of where the face used to be. Everything drawn
        // after that was aligned and osnapped to a ghost.
        //
        // Rebuilt here rather than at the call sites because this is the one place that already
        // means "the model changed" — a caller that forgot would leave the drawing on the ghost.
        if let Some(idx) = self.session.as_ref().map(|s| s.plane) {
            if let Some(frame) = self.model.sketch_by_id(idx).map(|sk| sk.frame) {
                self.sketch_ref = self.frame_face_edges(&frame);
            }
        }
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
    pub fn feature_world_outline(&self, f: &cad_solid::Feature) -> Option<Vec<Vec2>> {
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

    /// PLAN FOOTPRINT of any feature — what it covers looked at from above.
    ///
    /// [`Self::feature_world_outline`] is deliberately narrow: it answers "what closed outline does
    /// this slab have" for the carve logic, and says `None` for everything round. That is right for
    /// carving and wrong for DRAWING — a cylinder that returns `None` is simply absent from the
    /// plan, and a column you cannot see is indistinguishable from a column that is not there.
    ///
    /// So this never returns `None`. Exact where the shape allows, bounding rectangle where it does
    /// not, but always SOMETHING.
    pub fn feature_plan_footprint(&self, f: &cad_solid::Feature) -> Vec<Vec2> {
        let p = &f.placement;
        // A tilted shape's plan outline is not its upright one — fall through to the world AABB,
        // which is computed from the real transform and is honest about a tilt.
        let upright = p.pitch_deg.abs() < 1e-3 && p.roll_deg.abs() < 1e-3;

        let ring = |n: u32, rx: f32, ry: f32, phase: f32| -> Vec<Vec2> {
            let n = n.max(3);
            (0..n)
                .map(|i| {
                    let a = phase + std::f32::consts::TAU * i as f32 / n as f32;
                    Vec2::new(p.u + rx * a.cos(), p.v + ry * a.sin())
                })
                .collect()
        };
        let spin = p.spin_deg.to_radians();

        if upright {
            if let Some(o) = self.feature_world_outline(f) {
                return o;
            }
            match f.primitive {
                // Facetted around the vertical axis: the n-gon IS the footprint, exactly. A
                // 4-sided Frustum is a pyramid, and drawing it as a circle would be a lie.
                Primitive::Cylinder { r, sides, .. } => return ring(sides, r, r, spin),
                Primitive::Tube { r_outer, sides, .. } => return ring(sides, r_outer, r_outer, spin),
                Primitive::Frustum { r_bottom, r_top, sides, .. } => {
                    // The wider of the two ends is what the shape covers.
                    let r = r_bottom.max(r_top);
                    return ring(sides, r, r, spin);
                }
                Primitive::Sphere { r, segments, .. } => return ring(segments, r, r, spin),
                Primitive::Capsule { r, segments, .. } => return ring(segments, r, r, spin),
                Primitive::Torus { major_r, minor_r, seg_major, .. } => {
                    return ring(seg_major, major_r + minor_r, major_r + minor_r, spin)
                }
                Primitive::Ellipsoid { rx, ry, segments, .. } => {
                    // Only axis-aligned radii are meaningful here; a spun ellipse needs the corner
                    // rotation, so hand a spun one to the AABB below rather than draw it wrong.
                    if spin.abs() < 1e-3 {
                        return ring(segments, rx, ry, 0.0);
                    }
                }
                _ => {}
            }
        }

        let (mn, mx) = f.world_aabb();
        vec![
            Vec2::new(mn.x, mn.y),
            Vec2::new(mx.x, mn.y),
            Vec2::new(mx.x, mx.y),
            Vec2::new(mn.x, mx.y),
        ]
    }

    /// The feature a point on a face belongs to — the Union whose world AABB it sits on or in.
    ///
    /// Used to name a sketch plane after the thing it was drawn on. Deliberately AABB-based rather
    /// than an exact surface test: a name is a label, and the nearest enclosing box is right often
    /// enough that being wrong costs a rename rather than a wrong drawing.
    pub fn feature_at(&self, p: Vec3) -> Option<u32> {
        const SLACK: f32 = 1e-2; // a face point sits exactly ON the bound, so allow for rounding
        let mut best: Option<(u32, f32)> = None;
        for f in &self.model.features {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            let (mn, mx) = f.world_aabb();
            let inside = p.x >= mn.x - SLACK
                && p.x <= mx.x + SLACK
                && p.y >= mn.y - SLACK
                && p.y <= mx.y + SLACK
                && p.z >= mn.z - SLACK
                && p.z <= mx.z + SLACK;
            if !inside {
                continue;
            }
            // The SMALLEST box wins: a cupboard standing inside a building is inside both, and the
            // useful name is the cupboard.
            let vol = (mx.x - mn.x) * (mx.y - mn.y) * (mx.z - mn.z);
            if best.is_none_or(|(_, b)| vol < b) {
                best = Some((f.id, vol));
            }
        }
        best.map(|(id, _)| id)
    }

    /// A human name for one feature — "Building 2", "Box 1" — numbered within its own kind so the
    /// number means something to look for.
    pub fn feature_display_name(&self, id: u32) -> String {
        let Some(f) = self.model.features.iter().find(|f| f.id == id) else {
            return "Object".into();
        };
        let kind = f.primitive.kind_label();
        // An Extrusion is what a building outline is, and "Extrusion 1" is not what anyone calls it.
        let kind = if kind == "Extrusion" { "Building" } else { kind };
        let n = self
            .model
            .features
            .iter()
            .filter(|o| o.op == cad_solid::BoolOp::Union)
            .filter(|o| {
                let k = o.primitive.kind_label();
                (if k == "Extrusion" { "Building" } else { k }) == kind
            })
            .position(|o| o.id == id)
            .map(|i| i + 1)
            .unwrap_or(1);
        format!("{kind} {n}")
    }

    /// The name a NEW sketch plane gets: the object the face belongs to, and which face of it.
    ///
    /// Asked for as "named after the object the face belongs to". A room wins over the solid it was
    /// carved from — the room is the thing with a name the user chose, and that is what they are
    /// looking for in the list.
    pub fn sketch_auto_name(&self, frame: &Frame) -> String {
        let o = frame.origin;
        // The ground plan is not a face of anything.
        if frame.normal().z.abs() > 0.999 && o.z.abs() < 1e-3 {
            return "Global view".into();
        }
        let owner = self
            .rooms
            .iter()
            .find(|r| r.contains(Vec2::new(o.x, o.y)))
            .map(|r| r.name.clone())
            .or_else(|| self.feature_at(o).map(|id| self.feature_display_name(id)))
            .unwrap_or_else(|| "Plane".into());
        // "Cupboard 1 — face 2": the face number counts only the planes already on THIS object, so
        // it stays meaningful when other objects come and go.
        let n = self
            .model
            .sketches
            .iter()
            .filter(|s| s.name.starts_with(&format!("{owner} — face ")))
            .count()
            + 1;
        format!("{owner} — face {n}")
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
            // Stable tie-break so faces coincident with another body's stop flickering.
            let nudge = fid.map_or(0.0, depth_tiebreak);
            for (k, p) in tri.iter().enumerate() {
                let n = self.cached.normals.get(i * 3 + k).copied().unwrap_or(default_n);
                let p = Vec3::from(*p) + Vec3::from(n) * nudge;
                out.push(v(p, shade(base, Vec3::from(n))));
            }
        }
        out
    }

    /// World-space TEXTURED triangle soup for every feature that carries a pasted texture,
    /// grouped by texture index: `(texture_index, verts)`. UVs use WORLD box projection at
    /// ~1 tile / metre, so a wall or floor tiles the image at a real, consistent scale (the
    /// classic "wallpaper / floor tile" mapping). Honours hide-ceilings and cutaway exactly
    /// like [`Self::scene_verts`], and the triangles are the ones that method skips.
    /// The origin texture lookups are measured FROM, in world metres.
    ///
    /// A plan imported from a survey sits at coordinates in the thousands, and a texture
    /// coordinate that large is a precision disaster: the texel is chosen by the FRACTIONAL
    /// part, so deriving it from a huge number — then interpolating that in f32 across a
    /// triangle — discards most of the bits that actually select the texel. It shows as moiré
    /// banding that crawls with the camera, worst at grazing angles, and it is invisible on a
    /// model built near the origin, which is why it survives every synthetic test.
    ///
    /// Rounded to whole units so the tiling PHASE does not shift as the model grows, and taken
    /// once for the WHOLE model so neighbouring bodies stay aligned rather than breaking the
    /// pattern at every seam. Shared with the shader's triplanar path so the two cannot drift.
    pub fn uv_rebase_origin(&self) -> [f32; 3] {
        self.cached
            .bounds()
            .map_or([0.0; 3], |(mn, _)| [mn[0].round(), mn[1].round(), mn[2].round()])
    }

    pub fn feature_textured_meshes(&self) -> Vec<(usize, Vec<crate::light3d::TexVtx>)> {
        if self.feature_texture.is_empty() && self.surface_texture.is_empty() {
            return Vec::new();
        }
        let mut groups: std::collections::HashMap<usize, Vec<crate::light3d::TexVtx>> =
            std::collections::HashMap::new();
        // REBASE the UV origin to the model, because these coordinates are WORLD ones and a
        // plan imported from a survey sits at X≈3500, Y≈−6850. Texture coordinates in the
        // thousands are a precision disaster: the texel is chosen by the FRACTIONAL part of
        // the coordinate, and computing that from a large number — then interpolating it in
        // f32 across a triangle — loses most of the bits that actually select the texel. The
        // result is moiré banding that crawls as the camera moves, worst at grazing angles.
        //
        // Rounded to whole world units so the tiling PHASE is unchanged (the pattern does not
        // jump when the model grows), and taken from the model rather than per feature so
        // neighbouring bodies stay aligned instead of breaking the pattern at every seam.
        let uv_org = self.uv_rebase_origin();
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
            // The SAME tie-break `scene_verts` applies. A body can have flat and textured
            // triangles at once, and nudging only one of them would split it along the seam.
            let nudge = depth_tiebreak(id);
            for (k, p) in tri.iter().enumerate() {
                let n = Vec3::from(self.cached.normals.get(i * 3 + k).copied().unwrap_or([0.0, 0.0, 1.0]));
                let np = Vec3::from(*p) + n * nudge;
                let p = &[np.x, np.y, np.z];
                // UV from the REBASED position — see `uv_org`. The vertex itself keeps its
                // true world coordinates; only the texture lookup is moved near the origin.
                let q = [p[0] - uv_org[0], p[1] - uv_org[1], p[2] - uv_org[2]];
                let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
                let (uc, vc) = if ax >= ay && ax >= az {
                    (q[1], q[2]) // X-facing wall → YZ
                } else if ay >= az {
                    (q[0], q[2]) // Y-facing wall → XZ
                } else {
                    (q[0], q[1]) // floor / ceiling → XY
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
        // SORTED, and this is load-bearing rather than tidiness. `HashMap` seeds a fresh random
        // state per instance and this map is rebuilt every frame, so its iteration order differs
        // from frame to frame. The renderer's temporal accumulation hashes these groups IN ORDER
        // to decide whether anything changed; an unstable order hashes identical content
        // differently every frame, so the accumulator restarted forever and `n` never left 1/16.
        //
        // That is what kept screen-space noise visible: the GI gather gets its smoothness from
        // averaging ~16 jittered samples, and it never got past the first. A smooth material
        // then reads as speckled dots that change as the camera moves.
        let mut out: Vec<(usize, Vec<crate::light3d::TexVtx>)> = groups.into_iter().collect();
        out.sort_by_key(|(ti, _)| *ti);
        out
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
        //
        // THE PROXY PEELS ITS GLASS TOO, now that it carries alpha. This branch used to emit every
        // proxy triangle into the solid pass, which was correct only because `needs_lod` refused
        // any asset that had alpha at all — a heavy piece with glass in it would otherwise have had
        // its panes drawn opaque here AND blended again in the transparent pass.
        if asset.needs_lod() {
            let lod = asset.lod_geom();
            let glassy = lod.alpha.iter().any(|&a| a < ALPHA_OPAQUE);
            let mut out = Vec::with_capacity(lod.positions.len());
            for t in 0..lod.tri_count() {
                let base = t * 3;
                // ANY vertex, matching `tri_is_translucent` on the full mesh — the peel here and
                // the one in `furniture_translucent_mesh` have to agree exactly or a pane is
                // drawn twice or lost.
                if glassy && (base..base + 3).any(|k| lod.vertex_alpha(k) < ALPHA_OPAQUE) {
                    continue;
                }
                for k in base..base + 3 {
                    let n = lod.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]);
                    out.push(v(
                        Vec3::from(lod.positions[k]),
                        shade_furniture(inst.color, Vec3::from(n)),
                    ));
                }
            }
            return out;
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
        // FROM THE SAME MESH THE SOLID PASS DRAWS. `furniture_local_mesh` peels this piece's glass
        // OUT of the proxy, so if the panes came back from the full mesh the frame would be the
        // proxy's and the glass the original's — two different meshes in one object.
        let lod = if asset.needs_lod() { Some(asset.lod_geom()) } else { None };
        let (src_pos, src_nrm, src_alpha): (&[[f32; 3]], &[[f32; 3]], &[f32]) = match &lod {
            Some(a) => (&a.positions, &a.normals, &a.alpha),
            None => (&asset.positions, &asset.normals, &asset.alpha),
        };
        let mut out = Vec::new();
        for t in 0..src_pos.len() / 3 {
            let base = t * 3;
            // The proxy's own alpha, and ANY vertex — the same rule `tri_is_translucent` applies to
            // the full mesh. `all` instead would leave a triangle that is part glass in the solid
            // pass here and out of it there, so a pane would be drawn twice or not at all.
            if !(base..base + 3).any(|k| src_alpha.get(k).copied().unwrap_or(1.0) < ALPHA_OPAQUE) {
                continue;
            }
            for k in base..base + 3 {
                let p = src_pos[k];
                let n = src_nrm.get(k).copied().unwrap_or([0.0, 0.0, 1.0]);
                let c = shade_furniture(inst.color, Vec3::from(n));
                out.push(crate::light3d::V3A {
                    x: p[0], y: p[1], z: p[2],
                    r: c.col[0], g: c.col[1], b: c.col[2],
                    a: src_alpha.get(k).copied().unwrap_or(1.0),
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
        // Heavy pieces render from the decimated proxy.
        //
        // EVERY ATTRIBUTE COMES FROM THE SAME SOURCE AS THE POSITIONS. This used to read positions
        // from the proxy and UVs from `asset.uvs` by the same index `k`, which is not a
        // correspondence at all — the proxy has its own, shorter vertex list. It was safe only
        // because `needs_lod` guaranteed a proxied asset had no UVs (the comment here said so
        // outright). With the proxy carrying UVs, mixing the two would texture a mesh with another
        // mesh's coordinates, so `src_uv` follows `src_pos`.
        let lod = if asset.needs_lod() { Some(asset.lod_geom()) } else { None };
        let (src_pos, src_nrm, src_uv, src_alpha): (
            &[[f32; 3]],
            &[[f32; 3]],
            &[[f32; 2]],
            &[f32],
        ) = match &lod {
            Some(a) => (&a.positions, &a.normals, &a.uvs, &a.alpha),
            None => (&asset.positions, &asset.normals, &asset.uvs, &asset.alpha),
        };
        // Real UVs (from a glTF import) map the texture as the artist intended; otherwise box
        // projection normalised to the local bbox.
        let has_uv = src_uv.len() == src_pos.len();
        let mut out = Vec::with_capacity(src_pos.len());
        for (k, p) in src_pos.iter().enumerate() {
            let n = Vec3::from(src_nrm.get(k).copied().unwrap_or([0.0, 0.0, 1.0]));
            let (uc, vc) = if has_uv {
                let t = src_uv[k];
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
                // From `src_alpha`, for the same reason as `src_uv`: the proxy's vertex k is not
                // the full mesh's vertex k.
                x: p[0], y: p[1], z: p[2], u: uv[0], v: uv[1], s,
                a: src_alpha.get(k).copied().unwrap_or(1.0) * tex.opacity,
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
        // NO BAIL FOR HEAVY PIECES ANY MORE. This returned `None`, which sent a multi-material
        // import down the whole-object texture path and dropped its per-surface assignment — and
        // that was invisible only because `needs_lod` refused any asset with UVs, i.e. every
        // multi-material import there is. The split reads the proxy's own per-triangle face ids
        // below, so it produces the same buckets from fewer triangles.

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
        // Positions, normals, UVs, alpha AND the per-triangle face id all from ONE source. The
        // proxy's face ids are the source's, which is what makes the same texture assignment land
        // on the same surfaces at a fraction of the triangles.
        let lod = if asset.needs_lod() { Some(asset.lod_geom()) } else { None };
        let (src_pos, src_nrm, src_uv, src_alpha, src_face): (
            &[[f32; 3]],
            &[[f32; 3]],
            &[[f32; 2]],
            &[f32],
            &[u32],
        ) = match &lod {
            Some(a) => (&a.positions, &a.normals, &a.uvs, &a.alpha, &a.face),
            None => (&asset.positions, &asset.normals, &asset.uvs, &asset.alpha, &groups.face),
        };
        let has_uv = src_uv.len() == src_pos.len();
        let ntri = src_pos.len() / 3;

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
        // EVERY INDEX BELOW IS INTO THE SAME MESH. `ntri`, the positions, the normals, the UVs, the
        // alpha and the face ids all come from `src_*` — which is the proxy when there is one and
        // the full mesh when there is not.
        //
        // This read `ntri` from the proxy and `asset.positions[k]` from the full mesh, which is not
        // a correspondence: it drew the FIRST N triangles of the real mesh wearing the proxy's
        // texture coordinates. It shipped in build 44 and was caught by the compiler's own
        // `unused variable: src_nrm` / `src_face` — the two the loop had quietly stopped consulting.
        for t in 0..ntri {
            let fg = src_face.get(t).copied().unwrap_or(0);
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
                        let p = src_pos[k];
                        let n = Vec3::from(src_nrm.get(k).copied().unwrap_or([0.0, 0.0, 1.0]));
                        let (uc, vc) = uv_of(k, &p, n);
                        let s = shade_scalar(n, true);
                        let uv = tex.map_uv(uc, vc);
                        // Surface transparency multiplies the mesh's own per-vertex opacity.
                        let a = src_alpha.get(k).copied().unwrap_or(1.0) * tex.opacity;
                        buf.push(crate::light3d::TexVtx { x: p[0], y: p[1], z: p[2], u: uv[0], v: uv[1], s, a });
                    }
                }
                None => {
                    for k in base..base + 3 {
                        let p = src_pos[k];
                        let n = Vec3::from(src_nrm.get(k).copied().unwrap_or([0.0, 0.0, 1.0]));
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
    /// THE GROUND GRID — as far as you can see, at a spacing you can read.
    ///
    /// Reported as: "the grid now just shows for a small area on the canvas. lets have it for the
    /// whole visible canvas on 3d factory."
    ///
    /// It was a fixed ±10 m patch of 1 m squares sitting at the world origin. Zoom out past it, or
    /// pan away from the origin — which an imported drawing does immediately, since a DXF puts a
    /// building at coordinates like (−232, −58) — and the grid is a small rug on an empty floor,
    /// nowhere near the model.
    ///
    /// Two things fix that, and both are needed:
    ///
    ///   * it FOLLOWS THE CAMERA, sized from `cam_dist` so it always reaches past the edges of the
    ///     view, and snapped to its own spacing so the lines do not crawl while you pan;
    ///   * the SPACING STEPS with the zoom, on a 1-2-5 decade ladder. A fixed 1 m spacing is a
    ///     solid wall of lines across a 145 m building and invisibly fine on a door handle. The
    ///     ladder keeps the count bounded — about 80 lines per axis at any zoom, so this stays a
    ///     few hundred vertices whether you are looking at a city or a screw.
    ///
    /// Majors every tenth line, brighter, so the scale is readable without a ruler.
    pub fn grid_lines(&self) -> Vec<V3> {
        let mut out = Vec::new();
        if !self.show_grid {
            return out;
        }
        // How far the ground plane is visible.
        //
        // Straight down, that is about the camera distance and 1.6× covers a wide viewport's
        // corners. LOOKING ALONG THE GROUND it is far more: the plane runs away to the horizon, and
        // a grid sized for the overhead case reads as a small rug in the middle of the screen —
        // which is what the first version of this did in a perspective view.
        //
        // 1/sin(pitch) is the honest factor, floored so a dead-level camera does not ask for an
        // infinite grid, and capped at 10× so a near-horizontal view does not spend its whole
        // budget on ground that is two pixels tall.
        let spread = if self.ortho {
            1.6
        } else {
            (1.6 / self.cam_pitch.abs().sin().max(0.16)).min(10.0)
        };
        let reach = (self.cam_dist * spread).clamp(1.0, 1.0e6);
        // ~40 divisions each side of centre. `nice_step` rounds UP the ladder, so the real count is
        // between 16 and 40 — never more, which is what bounds the cost.
        let step = nice_step(reach / 40.0);
        let major = step * 10.0;

        const MINOR: [f32; 3] = [0.17, 0.20, 0.25];
        const MAJOR: [f32; 3] = [0.28, 0.33, 0.40];
        // The world axes stay legible whatever the spacing — they are the only two lines that say
        // where the origin is once the grid has walked off with the camera.
        const AXIS_X: [f32; 3] = [0.45, 0.25, 0.25];
        const AXIS_Y: [f32; 3] = [0.25, 0.45, 0.28];

        // SNAP THE CENTRE to the spacing. Without this every line moves with the camera and the
        // grid shimmers instead of standing still under the model.
        let cx = (self.cam_target[0] / step).round() * step;
        let cy = (self.cam_target[1] / step).round() * step;
        let n = (reach / step).ceil() as i32;
        // The EXTENT is snapped too, not only the line positions. `reach` is rarely a whole number
        // of steps, so using it directly put every line's two ENDPOINTS off the grid — a ragged
        // edge on all four sides, and a "snapped to the spacing" claim that was only true of the
        // coordinate each line happens to run along. `half` makes it a clean rectangle of whole
        // cells. Caught by the test, not by looking at it.
        let half = n as f32 * step;

        for i in -n..=n {
            let t = i as f32 * step;
            let (x, y) = (cx + t, cy + t);
            let is_major = |v: f32| (v / major - (v / major).round()).abs() < 1.0e-4;

            let cxl = if x.abs() < step * 0.5 { AXIS_Y } // the line x = 0 runs along +Y
            else if is_major(x) { MAJOR } else { MINOR };
            let cyl = if y.abs() < step * 0.5 { AXIS_X }
            else if is_major(y) { MAJOR } else { MINOR };

            seg(&mut out, Vec3::new(x, cy - half, 0.0), Vec3::new(x, cy + half, 0.0), cxl);
            seg(&mut out, Vec3::new(cx - half, y, 0.0), Vec3::new(cx + half, y, 0.0), cyl);
        }
        out
    }

    /// THE ORIGIN — three axes standing at (0, 0, 0), so world zero is a place you can see.
    ///
    /// Asked for as: "have a gizmo at the origin so the user know where the origin is. it should
    /// have x, y and z axis."
    ///
    /// It matters more here than in most 3D apps, because the origin is where three of the four
    /// placement modes measure from: `origin` puts an object exactly here, and `@X,Y,Z` measures
    /// from here. Being unable to see the thing your coordinates are relative to is most of why
    /// "the placement is still confusing".
    ///
    /// Sized off the camera so it is a readable size at any zoom rather than a dot from far away
    /// and a wall from close up. RGB = XYZ, the convention every 3D tool uses.
    pub fn origin_gizmo_lines(&self) -> Vec<V3> {
        let mut out = Vec::new();
        // OFF BY DEFAULT. Asked for, then "the origin gizmo needs to turned off" — it sits in the
        // middle of the model and there is rarely anything at the world origin worth looking at.
        // Kept behind a toggle rather than deleted: it is the reference `origin` and `@X,Y,Z`
        // measure from, so it earns its place the moment those are being used.
        if !self.show_origin {
            return out;
        }
        // A fixed fraction of the view, so it reads the same at 2 m and at 2 km.
        let len = (self.cam_dist * 0.12).clamp(0.05, 1.0e5);
        const RED: [f32; 3] = [0.95, 0.25, 0.25]; // +X
        const GREEN: [f32; 3] = [0.30, 0.90, 0.35]; // +Y
        const BLUE: [f32; 3] = [0.35, 0.55, 1.00]; // +Z
        let o = Vec3::ZERO;
        for (dir, c) in
            [(Vec3::X, RED), (Vec3::Y, GREEN), (Vec3::Z, BLUE)]
        {
            let tip = dir * len;
            seg(&mut out, o, tip, c);
            // An ARROWHEAD, so +X and −X are not the same line. Two barbs in the plane the axis is
            // least aligned with, which keeps them visible from any camera angle.
            let side = if dir.z.abs() > 0.5 { Vec3::X } else { Vec3::Z };
            let barb = len * 0.18;
            let back = tip - dir * barb;
            seg(&mut out, tip, back + side * barb * 0.5, c);
            seg(&mut out, tip, back - side * barb * 0.5, c);
        }
        out
    }

    pub fn overlay_lines(&self) -> Vec<V3> {
        let mut out = Vec::new();
        out.extend(self.grid_lines());
        // Each behind its own switch — the grid is a working surface, the origin is a reference
        // point, and wanting one is no reason to be given the other.
        out.extend(self.origin_gizmo_lines());
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
        // EVERY ORPHANED OPENING, IN RED, WHETHER OR NOT ANYTHING IS SELECTED — this is the one
        // overlay that is not a response to the user pointing at something.
        //
        // An opening that lost its wall segment is kept but not applied (see `rederive_wall`), so
        // the wall it belonged to renders whole. There is nothing to notice: a window that has
        // stopped existing looks exactly like a wall that never had one. The box is drawn where
        // the cut would be, so the absence has a location.
        for id in self.orphaned_cutouts() {
            if let Some(f) = self.model.features.iter().find(|f| f.id == id) {
                let (mn, mx) = f.world_aabb();
                aabb_lines(&mut out, mn, mx, [1.0, 0.25, 0.2]);
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

    /// The world-space XY rectangle the camera can currently see ON the plane `z`, or `None` when
    /// that cannot be bounded.
    ///
    /// Cast a ray through each of the four viewport corners and intersect it with the plane. When
    /// every corner lands, their bbox contains everything visible on that plane and nothing
    /// outside it needs drawing.
    ///
    /// `None` IS THE HONEST ANSWER WHEN A CORNER MISSES — a camera tilted toward the horizon has
    /// corners whose rays run parallel to the plane or away from it, and the visible region is
    /// then unbounded. Returning a bbox of the corners that did land would cull geometry that is
    /// genuinely on screen, which is a rendering bug that looks like missing data. The caller
    /// draws everything instead, which is merely slow.
    pub fn ground_view_bounds(rect: egui::Rect, mvp: &[f32; 16], z: f32) -> Option<(Vec2, Vec2)> {
        let corners = [
            rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom(),
        ];
        let (mut mn, mut mx) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
        for c in corners {
            let (o, d) = Self::ray(c, rect, mvp);
            // Parallel to the plane, or pointing away from it — unbounded either way.
            if d.z.abs() < 1e-6 {
                return None;
            }
            let t = (z - o.z) / d.z;
            if t < 0.0 || !t.is_finite() {
                return None;
            }
            let p = o + d * t;
            mn = mn.min(Vec2::new(p.x, p.y));
            mx = mx.max(Vec2::new(p.x, p.y));
        }
        (mn.x.is_finite() && mx.x.is_finite()).then_some((mn, mx))
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
        if !self.snap_3d {
            return None;
        }
        let m = Mat4::from_cols_array(mvp);
        let aperture = 12.0f32;
        let mut best: Option<(f32, Vec3, egui::Pos2)> = None;
        for (i, p) in self.cached.positions.iter().enumerate() {
            // NEVER SNAP TO SOMETHING YOU CANNOT SEE. With "hide ceilings" on — the normal way to
            // work inside a building — the roof slab is still in the mesh, so a click in the middle
            // of a room could jump to a ceiling corner floating above it with nothing on screen to
            // explain why. `face_ids` is per TRIANGLE; three positions share one.
            if self.hide_ceilings {
                if let Some(&fid) = self.cached.face_ids.get(i / 3) {
                    if self.is_hidden_ceiling(fid) {
                        continue;
                    }
                }
            }
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

    /// THE FACE ITSELF — only the edges that LIE IN `frame`'s plane, in its (u,v).
    ///
    /// Reported as: "when click draw on this face. show me only that face on the 2d cad. because
    /// now it looks confusing for the user."
    ///
    /// [`Self::frame_reference_edges`] projects the WHOLE model onto the plane, which on a wall face
    /// of a 139 m building draws every wall, room and opening in the model flattened on top of each
    /// other — a band of overlapping lines you cannot read, let alone draw on. An edge that lies in
    /// the plane is a boundary of a face standing ON that plane, and that is what "this face" means.
    ///
    /// Coplanar geometry belonging to OTHER objects is kept on purpose: if something else stands in
    /// exactly this plane it is genuinely part of the surface being drawn on, and hiding it would
    /// mean drawing blind against it.
    pub fn frame_face_edges(&self, frame: &Frame) -> Vec<[Vec2; 2]> {
        // Half a millimetre. Tight enough to reject a wall 200 mm behind this one, loose enough to
        // survive f32 at the 130 m coordinates an imported DXF puts a building at (the session dump
        // measures 0.016 mm per ULP out there, so this is ~30 ULP of headroom).
        const ON_PLANE: f32 = 5.0e-4;
        let n = frame.normal();
        let plane_d = n.dot(frame.origin);
        self.frame_reference_edges_filtered(frame, |a, b| {
            (n.dot(a) - plane_d).abs() <= ON_PLANE && (n.dot(b) - plane_d).abs() <= ON_PLANE
        })
    }

    /// The solid's FEATURE EDGES projected onto `frame`'s (u,v) plane — a clean line
    /// drawing of the 3D object for use as a reference underlay when sketching on a face.
    ///
    /// An edge is a "feature" edge if it is shared by ONLY ONE triangle (a true boundary) or
    /// by two triangles whose normals differ by more than ~20° (a real crease). Interior
    /// tessellation edges — the diagonals that split a flat quad — are dropped, so what you
    /// get is the object's outline and its hard edges, not a triangle-soup mess.
    pub fn frame_reference_edges(&self, frame: &Frame) -> Vec<[Vec2; 2]> {
        self.frame_reference_edges_filtered(frame, |_, _| true)
    }

    /// The shared body of [`Self::frame_reference_edges`] and [`Self::frame_face_edges`]: the same
    /// feature-edge extraction, with `keep` deciding which world-space edges survive. One
    /// implementation, so "the whole model" and "just this face" cannot drift apart.
    fn frame_reference_edges_filtered(
        &self,
        frame: &Frame,
        keep: impl Fn(Vec3, Vec3) -> bool,
    ) -> Vec<[Vec2; 2]> {
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
            if is_feature && keep(Vec3::from(a), Vec3::from(b)) {
                out.push([frame.to_uv(Vec3::from(a)), frame.to_uv(Vec3::from(b))]);
            }
        }
        out
    }

    /// THE PICKED FACE, outlined in YELLOW in the 3D view.
    ///
    /// Asked for as: "when a face is selected to sketch, have a yellow outline show up in the 3d
    /// view so the user can know if he selected the right face in 3d."
    ///
    /// The 2D canvas swings onto the plane the moment a face is picked, and until now the 3D view
    /// said nothing about WHICH face that was. On a building with a dozen similar walls the only
    /// way to find out was to draw something and see where it landed.
    ///
    /// The outline is [`Self::sketch_ref`] — already exactly this face's in-plane edges, see
    /// `frame_face_edges` — lifted back out of (u,v) into world space. Same source as the 2D
    /// underlay ON PURPOSE: the yellow line in 3D and the outline being drawn against in 2D are
    /// then the same edges by construction, and cannot disagree about which face is open.
    pub fn picked_face_lines(&self) -> Vec<V3> {
        let mut out = Vec::new();
        let Some(sk) = self.session.as_ref().and_then(|s| self.model.sketch_by_id(s.plane)) else {
            return out;
        };
        // Yellow, and brighter than anything else in the overlay — this answers "did I pick the
        // right one?", which is only useful if it is the first thing you see.
        const YELLOW: [f32; 3] = [1.0, 0.85, 0.10];
        for [a, b] in &self.sketch_ref {
            seg(&mut out, sk.frame.from_uv(*a), sk.frame.from_uv(*b), YELLOW);
        }
        out
    }

    /// Every sketch's geometry, lifted from its frame's `(u,v)` back into world space,
    /// as GL_LINES. This is what makes 2D work drawn on a plane visible in 3D.
    ///
    /// `doc` is the DRAWING, for the layer table — the one the Layers panel edits, so recolouring
    /// a layer recolours the work on every plane at once. A finished sketch keeps its own snapshot
    /// of the table from when it closed, and resolving against that would ship stale colours.
    pub fn sketch_lines(&self, doc: &cad_kernel::Document) -> Vec<V3> {
        let mut out = Vec::new();
        for (i, sk) in self.model.sketches.iter().enumerate() {
            let active = self.session.as_ref().is_some_and(|s| s.plane == sk.id);
            // THE ACTIVE PLANE IS NOT DRAWN FROM HERE. `factory_enter_sketch` `mem::take`s its
            // document, so this loop would emit nothing for it anyway — `live_sketch_lines` is
            // what shows it, at full strength. Skipping it explicitly means the dim below applies
            // to everything this function ever emits, so nothing pops brighter on Finish.
            if active {
                let o = sk.frame.origin;
                seg(&mut out, o, o + sk.frame.u * 1.5, [1.0, 0.3, 0.3]);
                seg(&mut out, o, o + sk.frame.v * 1.5, [0.3, 1.0, 0.3]);
                continue;
            }
            for d in &sk.doc.dobjects {
                // The entity's own document answers an entity-level colour; the DRAWING answers
                // ByLayer, which is what almost everything is. Stepped back in value, not in hue.
                let c = dobject_srgb(d, &sk.doc, doc).map(|x| x * FINISHED_DIM);
                // By the SKETCH's own unit — `from_uv` lifts (u,v) into world METRES.
                for poly in cad_solid::geom_display_outlines_scaled(&d.geom, &sk.doc, sk.doc.units.metres_per_unit) {
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
        }
        out
    }

    /// The ACTIVE sketch's LIVE geometry — which lives in the app's swapped-in document,
    /// passed in here — lifted onto its frame, so what you draw on a face appears in the 3D
    /// view immediately (2D↔3D linked). `sketch_lines` can't show it because the active
    /// sketch's own `doc` is empty while it is being edited.
    /// The 2D plan as DEPTH-TESTED world segments on the ground plane — so a wall in front of
    /// it hides it, the way any other geometry would.
    ///
    /// The alternative (and the original behaviour) is [`crate::app::CadApp::paint_plan_underlay`],
    /// which paints the same lines on an egui FOREGROUND layer so they read through the model.
    /// That is genuinely useful when the plan is a bare outline you are tracing, and actively
    /// confusing when it is a full drawing: every annotation block and hatch on the far side of
    /// the building lands on the near wall and reads as a texture painted on it.
    pub fn plan_lines(&self, doc: &cad_kernel::Document, z: f32) -> Vec<V3> {
        let mut out = Vec::new();
        let c = [0.47, 0.69, 0.88]; // muted blue — reference, not built geometry
        let k = doc.units.metres_per_unit;
        // Lifted a hair off the plane. A floor slab sits exactly ON `z`, and there is no
        // polygon offset in this renderer, so drawing the plan at the same height makes the
        // two fight for the depth buffer and the lines stipple in and out as the camera moves.
        let z = z + 0.002;
        for d in &doc.dobjects {
            for path in cad_solid::geom_display_outlines_scaled(&d.geom, doc, k) {
                for w in path.windows(2) {
                    seg(
                        &mut out,
                        Vec3::new(w[0].x, w[0].y, z),
                        Vec3::new(w[1].x, w[1].y, z),
                        c,
                    );
                }
            }
        }
        out
    }

    pub fn live_sketch_lines(&self, doc: &cad_kernel::Document) -> Vec<V3> {
        let mut out = Vec::new();
        let Some(session) = self.session.as_ref() else { return out };
        let Some(sk) = self.model.sketch_by_id(session.plane) else { return out };
        for d in &doc.dobjects {
            // THE LIVE SKETCH RESOLVES AGAINST ITSELF, at full strength — this is the plane being
            // drawn on. While a session is open `doc` IS the sketch: it carries the clone of the
            // drawing's layer table taken on the way in, and the Layers panel edits THAT table, so
            // it is the freshest there is and the one `factory_exit_sketch` copies back out.
            let c = dobject_srgb(d, doc, doc);
            // By the live sketch document's own unit — `from_uv` lifts into world METRES.
            for poly in cad_solid::geom_display_outlines_scaled(&d.geom, doc, doc.units.metres_per_unit) {
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

    /// AN ID IS AN IDENTITY AND MUST NOT BE RECYCLED.
    ///
    /// The allocator was `features.iter().map(|f| f.id).max() + 1`, so deleting the newest feature
    /// handed its id straight to the next one. Every per-object property the app keeps — colour,
    /// texture, group, ceiling flag — lives in a side map keyed by that id, and `erase_selection`
    /// purged only two of them. So: paint a box, delete it, draw another, and the new one came up
    /// wearing the dead one's paint.
    #[test]
    fn a_new_object_never_inherits_a_deleted_ones_identity() {
        let mut st = one_box();
        let first = st.model.features[0].id;
        st.feature_color.insert(first, [1.0, 0.0, 0.0]);
        st.feature_texture.insert(first, 7);

        st.selection = vec![first];
        st.erase_selection();
        st.add_box();
        let second = st.model.features.last().expect("a second box").id;

        assert_ne!(second, first, "the deleted feature's id was reused");
        assert!(
            !st.feature_color.contains_key(&second) && !st.feature_texture.contains_key(&second),
            "the new object inherited the deleted object's paint",
        );
        assert!(
            st.feature_color.is_empty() && st.feature_texture.is_empty(),
            "erase left stale per-feature state behind",
        );
    }

    /// A project saved before the counters existed loads with them at zero. Without raising the
    /// floor, the next object drawn is handed id 1 — which the loaded model is already using.
    #[test]
    fn loading_an_older_model_raises_the_id_floor() {
        let mut st = one_box();
        st.add_box();
        let used: Vec<u32> = st.model.features.iter().map(|f| f.id).collect();

        // Exactly the shape serde produces for a file that predates the counters.
        st.model.next_feature_id = 0;
        st.model.reserve_ids_above_loaded();
        st.add_box();

        let fresh = st.model.features.last().unwrap().id;
        assert!(
            !used.contains(&fresh),
            "id {fresh} collides with the loaded model's {used:?}",
        );
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

/// The Factory's working unit — an input and display preference, never a change to the model.
#[cfg(test)]
mod working_unit {
    use super::*;
    use cad_kernel::{DocUnits, UnitSource};

    fn mm() -> DocUnits {
        DocUnits::new(DocUnits::MM, UnitSource::User)
    }

    /// The default is millimetres, because that is what building drawings are dimensioned in and
    /// what the 2D side of this app is usually working in.
    #[test]
    fn a_new_factory_works_in_millimetres() {
        let f = FactoryState::default();
        assert!((f.units.metres_per_unit - DocUnits::MM).abs() < 1e-12);
        assert_eq!(f.units.label(), "mm");
    }

    /// **Switching the unit must never move geometry.**
    ///
    /// The whole design rests on this: lengths are stored in metres and the unit is a lens over
    /// them. If a switch moved anything, the setting would be unusable — nobody could change their
    /// mind about how they prefer to type, and a project opened by someone with a different
    /// preference would be a different building.
    #[test]
    fn changing_the_unit_moves_nothing() {
        let mut f = FactoryState::default();
        f.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box { w: 6.0, d: 4.0, h: 3.0 },
        );
        f.wall_height = 2.7;
        f.wall_thickness = 0.2;
        let before = format!("{:?}", f.model.features[0].primitive);

        for m in [DocUnits::M, DocUnits::INCH, DocUnits::FOOT, DocUnits::CM, DocUnits::MM] {
            f.units = DocUnits::new(m, UnitSource::User);
            assert_eq!(
                format!("{:?}", f.model.features[0].primitive),
                before,
                "the solid changed when the working unit became {}",
                f.units.label()
            );
            assert_eq!(f.wall_height, 2.7, "a stored length is metres whatever the unit says");
            assert_eq!(f.wall_thickness, 0.2);
        }
    }

    /// A metre value is SHOWN as the same physical length in whatever unit is chosen.
    #[test]
    fn the_same_length_reads_correctly_in_every_unit() {
        let cases = [
            (DocUnits::MM, 2700.0, "mm"),
            (DocUnits::CM, 270.0, "cm"),
            (DocUnits::M, 2.7, "m"),
        ];
        for (m, want, label) in cases {
            let u = DocUnits::new(m, UnitSource::User);
            assert_eq!(u.label(), label);
            assert!(
                (u.from_metres(2.7) - want).abs() < 1e-6,
                "2.7 m should read {want} {label}, got {}",
                u.from_metres(2.7)
            );
            // …and typing that number back gives the metre value again.
            assert!((u.to_metres(want) - 2.7).abs() < 1e-9);
        }
    }

    /// Decimals follow the unit. Whole millimetres, three decimals in metres — a mm field showing
    /// hundredths is noise, and a metre field showing none has thrown the dimension away.
    #[test]
    fn a_length_is_shown_to_a_useful_precision() {
        assert_eq!(length_decimals(DocUnits::new(DocUnits::MM, UnitSource::User)), 0);
        assert_eq!(length_decimals(DocUnits::new(DocUnits::CM, UnitSource::User)), 1);
        assert_eq!(length_decimals(DocUnits::new(DocUnits::M, UnitSource::User)), 3);
        assert_eq!(length_str(mm(), 2.7), "2700 mm");
        assert_eq!(length_str(DocUnits::new(DocUnits::M, UnitSource::User), 2.7), "2.700 m");
    }

    /// The unit survives a save, so a project reopens showing the numbers it was authored with.
    #[test]
    fn the_working_unit_round_trips_through_the_sidecar() {
        let mut f = FactoryState::default();
        f.units = cad_kernel::DocUnits::new(cad_kernel::DocUnits::INCH, UnitSource::User);
        let doc = f.to_persist();
        assert!((doc.working_unit_m - DocUnits::INCH).abs() < 1e-12);

        let mut reopened = FactoryState::default();
        reopened.apply_persist(doc);
        assert!((reopened.units.metres_per_unit - DocUnits::INCH).abs() < 1e-12);
        assert_eq!(reopened.units.label(), "in");
    }

    /// A sidecar written BEFORE the working unit existed records nothing, and the default stands.
    /// Reinterpreting an old project's numbers would be the one way this feature could do harm.
    #[test]
    fn an_older_project_keeps_the_default_unit() {
        let mut f = FactoryState::default();
        let mut doc = f.to_persist();
        doc.working_unit_m = 0.0; // as written by a build that predates the field
        f.units = cad_kernel::DocUnits::new(DocUnits::FOOT, UnitSource::User);
        f.apply_persist(doc);
        assert!(
            (f.units.metres_per_unit - DocUnits::FOOT).abs() < 1e-12,
            "an unrecorded unit must not silently reset the one in use"
        );
    }
}

/// What a room's height number actually buys you.
///
/// Reported as "I made the building 4 m tall and gave the room 3900 mm — why is the room taller?".
/// It is not an arithmetic error: `room_height` is the CLEAR height, floor top to ceiling
/// underside, which is what the term means on a drawing. The two slabs are additional, so the
/// structure always stands taller than the number typed. The failure was that nothing said so.
#[cfg(test)]
mod room_height_meaning {
    use super::*;

    fn square(m: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(m, 0.0),
            Vec2::new(m, m),
            Vec2::new(0.0, m),
            Vec2::new(0.0, 0.0),
        ]
    }

    /// The reported numbers, to the millimetre: 200 floor + 3900 clear + 150 ceiling = 4250.
    #[test]
    fn a_room_is_taller_than_its_clear_height_by_both_slabs() {
        let mut f = FactoryState::default();
        f.room_floor = 0.20;
        f.room_height = 3.90;
        f.ceiling_thickness = 0.15;
        f.room_open_top = false;
        f.building_height = 4.0;
        f.add_room(&square(4.4)).expect("the room builds");
        f.recompute();

        let (mn, mx) = f.cached.bounds().expect("the room has bounds");
        let overall = mx[2] - mn[2];
        assert!(
            (overall - 4.25).abs() < 1e-3,
            "0.20 + 3.90 + 0.15 = 4.25 m overall, got {overall}",
        );
        assert!(overall > f.building_height, "which is taller than the 4 m building");
    }

    /// …and the app SAYS so, with the arithmetic and the overrun. The geometry was always right;
    /// the silence was the bug.
    #[test]
    fn the_overrun_is_reported_rather_than_left_to_be_noticed() {
        let mut f = FactoryState::default();
        f.room_floor = 0.20;
        f.room_height = 3.90;
        f.ceiling_thickness = 0.15;
        f.room_open_top = false;
        f.building_height = 4.0;
        f.add_room(&square(4.4)).expect("the room builds");

        let s = &f.status;
        assert!(s.contains("4250 mm"), "the overall height must be stated: {s}");
        assert!(s.contains("3900 mm"), "beside the clear height that was typed: {s}");
        assert!(s.contains("taller than"), "and the overrun called out: {s}");
        assert!(s.contains("250 mm"), "by how much: {s}");
    }

    /// A room that FITS says so without crying wolf — a warning that appears every time is one
    /// nobody reads.
    #[test]
    fn a_room_that_fits_carries_no_warning() {
        let mut f = FactoryState::default();
        f.room_floor = 0.20;
        f.room_height = 3.60;
        f.ceiling_thickness = 0.15;
        f.room_open_top = false;
        f.building_height = 4.0; // 3.95 overall — fits
        f.add_room(&square(4.4)).expect("the room builds");
        assert!(!f.status.contains("taller than"), "no warning when it fits: {}", f.status);
        assert!(f.status.contains("3950 mm"), "but the overall is still stated: {}", f.status);
    }

    /// Open to sky: no ceiling slab, so only the floor is added to the clear height.
    #[test]
    fn an_open_topped_room_only_adds_its_floor() {
        let mut f = FactoryState::default();
        f.room_floor = 0.20;
        f.room_height = 3.90;
        f.room_open_top = true;
        f.building_height = 4.2;
        f.add_room(&square(4.4)).expect("the room builds");
        f.recompute();
        let (mn, mx) = f.cached.bounds().expect("bounds");
        assert!(
            (mx[2] - mn[2] - 4.10).abs() < 1e-3,
            "0.20 + 3.90 with no ceiling = 4.10 m, got {}",
            mx[2] - mn[2],
        );
        assert!(!f.status.contains("ceiling"), "and no ceiling in the breakdown: {}", f.status);
    }
}

/// Rooms as editable objects: named, re-heightable, and able to say what is inside them.
///
/// Reported as "once a room is made there's no way of adjusting its height". There was not — a
/// room was a floor slab, a ring of wall boxes and a ceiling slab with nothing tying them together,
/// so the only way to change one was to delete every piece and draw it again.
#[cfg(test)]
mod rooms {
    use super::*;

    fn square(m: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(m, 0.0),
            Vec2::new(m, m),
            Vec2::new(0.0, m),
            Vec2::new(0.0, 0.0),
        ]
    }

    fn a_room() -> FactoryState {
        let mut f = FactoryState::default();
        f.room_floor = 0.20;
        f.room_height = 3.00;
        f.ceiling_thickness = 0.15;
        f.room_open_top = false;
        f.building_height = 4.0;
        f.add_room(&square(5.0)).expect("the room builds");
        f
    }

    /// Building a room REGISTERS it, with the pieces it owns.
    #[test]
    fn a_built_room_is_an_object_not_just_geometry() {
        let f = a_room();
        assert_eq!(f.rooms.len(), 1);
        let r = &f.rooms[0];
        assert!(r.floor.is_some(), "it owns its floor");
        assert!(r.ceiling.is_some(), "and its ceiling");
        assert_eq!(r.walls.len(), 4, "and one wall per edge of the square");
        assert_eq!(f.room_features(r.id).len(), 6, "6 features in total");
    }

    /// **The reported failure.** A room's clear height can be changed after it is built, and the
    /// structure follows.
    #[test]
    fn a_rooms_height_can_be_changed_after_it_is_built() {
        let mut f = a_room();
        let id = f.rooms[0].id;
        f.recompute();
        let before = f.cached.bounds().expect("bounds").1[2];
        assert!((before - 3.35).abs() < 1e-3, "0.20 + 3.00 + 0.15 = 3.35 m, got {before}");

        f.set_room_height(id, 3.9);
        f.recompute();
        let after = f.cached.bounds().expect("bounds").1[2];
        assert!((after - 4.25).abs() < 1e-3, "0.20 + 3.90 + 0.15 = 4.25 m, got {after}");
        assert!((f.rooms[0].height - 3.9).abs() < 1e-6, "and the record agrees");
    }

    /// The edit happens IN PLACE — every feature id survives.
    ///
    /// This is the point of editing rather than rebuilding: a window cut into a wall references
    /// that wall's feature. Delete and recreate the walls and the cut is orphaned, so raising a
    /// room's height would silently destroy its openings.
    #[test]
    fn changing_the_height_keeps_every_feature_id() {
        let mut f = a_room();
        let id = f.rooms[0].id;
        let before = f.room_features(id);
        f.set_room_height(id, 4.5);
        let after = f.room_features(id);
        assert_eq!(before, after, "the same features, resized — not new ones");
    }

    /// A room can be renamed, and an empty name falls back to something addressable rather than
    /// leaving it unlabelled on the plan and unfindable in the openings list.
    #[test]
    fn a_room_can_be_renamed_but_never_to_nothing() {
        let mut f = a_room();
        let id = f.rooms[0].id;
        f.rename_room(id, "  Reception  ");
        assert_eq!(f.rooms[0].name, "Reception", "trimmed");
        f.rename_room(id, "   ");
        assert_eq!(f.rooms[0].name, format!("Room {id}"), "empty falls back");
    }

    /// A point inside the outline finds the room; one outside finds nothing.
    #[test]
    fn a_point_finds_the_room_it_is_in() {
        let f = a_room();
        let id = f.rooms[0].id;
        assert_eq!(f.room_at(Vec2::new(2.5, 2.5)), Some(id));
        assert_eq!(f.room_at(Vec2::new(50.0, 50.0)), None);
    }

    /// A room INSIDE another wins — the smaller outline is the one a piece is really in.
    ///
    /// Without this the enclosing space claims every opening in the building, which makes the
    /// grouping useless exactly where it is needed most.
    #[test]
    fn the_smaller_room_wins_when_one_contains_another() {
        let mut f = FactoryState::default();
        f.add_room(&square(20.0)).expect("hall");
        let hall = f.rooms[0].id;
        // A store in the corner of the hall.
        f.add_room(&vec![
            Vec2::new(1.0, 1.0),
            Vec2::new(4.0, 1.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(1.0, 4.0),
            Vec2::new(1.0, 1.0),
        ])
        .expect("store");
        let store = f.rooms[1].id;
        assert_eq!(f.room_at(Vec2::new(2.5, 2.5)), Some(store), "inside the store");
        assert_eq!(f.room_at(Vec2::new(10.0, 10.0)), Some(hall), "out in the hall");
    }

    /// The area-weighted centroid lands INSIDE an L-shaped room. The mean of the corners does not,
    /// which would print the name over a different room.
    #[test]
    fn an_l_shaped_room_labels_inside_itself() {
        let mut f = FactoryState::default();
        let l = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(6.0, 0.0),
            Vec2::new(6.0, 2.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(2.0, 6.0),
            Vec2::new(0.0, 6.0),
            Vec2::new(0.0, 0.0),
        ];
        f.add_room(&l).expect("L room");
        let r = &f.rooms[0];
        assert!(
            !r.contains(r.centroid()),
            "this L is the case where the AREA centroid falls in the notch — that is why \n             label_point exists",
        );
        assert!(
            r.contains(r.label_point()),
            "the label itself must sit inside the room, got {:?}",
            r.label_point(),
        );
    }

    /// Deleting a room takes its geometry AND its record.
    #[test]
    fn deleting_a_room_takes_everything_with_it() {
        let mut f = a_room();
        let id = f.rooms[0].id;
        let feats = f.room_features(id);
        f.delete_room(id);
        assert!(f.rooms.is_empty(), "the record is gone");
        for fid in feats {
            assert!(!f.model.features.iter().any(|x| x.id == fid), "feature {fid} should be gone too");
        }
    }

    /// Rooms survive a save, with their names, heights and the pieces they own.
    #[test]
    fn rooms_round_trip_through_the_sidecar() {
        let mut f = a_room();
        let id = f.rooms[0].id;
        f.rename_room(id, "Reception");
        f.set_room_height(id, 3.6);
        let doc = f.to_persist();
        assert_eq!(doc.rooms.len(), 1);

        let mut reopened = FactoryState::default();
        reopened.apply_persist(doc);
        assert_eq!(reopened.rooms.len(), 1);
        let r = &reopened.rooms[0];
        assert_eq!(r.name, "Reception");
        assert!((r.height - 3.6).abs() < 1e-6);
        assert_eq!(r.walls.len(), 4, "and it still owns its walls");
        assert!(reopened.next_room_id > id, "ids keep counting up");
    }
}

/// A room carved out of a building: one set of walls, and a hole that closes when it is deleted.
///
/// Reported together — "the rooms are being built lopsided" and "when a room is deleted that area
/// becomes hollow". Both come from the carve: making a room inside a building punched a void AND
/// built a second ring of walls inside it, and deleting the room removed the walls but left the
/// void cutting the building for good.
#[cfg(test)]
mod carved_rooms {
    use super::*;

    fn square(m: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(m, 0.0),
            Vec2::new(m, m),
            Vec2::new(0.0, m),
            Vec2::new(0.0, 0.0),
        ]
    }

    fn inner(lo: f32, hi: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(lo, lo),
            Vec2::new(hi, lo),
            Vec2::new(hi, hi),
            Vec2::new(lo, hi),
            Vec2::new(lo, lo),
        ]
    }

    /// A building with a room carved into it.
    fn building_with_room() -> FactoryState {
        let mut f = FactoryState::default();
        f.building_height = 4.0;
        f.room_floor = 0.2;
        f.room_height = 3.0;
        f.ceiling_thickness = 0.15;
        f.add_building_outline(&square(10.0), 4.0).expect("building");
        f.add_room(&inner(1.0, 9.0)).expect("room");
        f
    }

    /// **Lopsided.** A carved room does NOT build its own ring of walls — the building material
    /// left around the void is the wall. Two rings at different heights is what the step was.
    #[test]
    fn a_carved_room_does_not_add_a_second_ring_of_walls() {
        let f = building_with_room();
        let r = &f.rooms[0];
        assert!(r.carve.is_some(), "it was carved from the building");
        assert!(
            r.walls.is_empty(),
            "the building is the wall — {} extra walls would step against it",
            r.walls.len(),
        );
    }

    /// A FREE-STANDING room, with no building to carve, still builds its own walls.
    #[test]
    fn a_free_standing_room_still_builds_its_walls() {
        let mut f = FactoryState::default();
        f.add_room(&square(5.0)).expect("room");
        let r = &f.rooms[0];
        assert!(r.carve.is_none(), "nothing to carve");
        assert_eq!(r.walls.len(), 4, "so it provides its own walls");
    }

    /// **The hollow.** Deleting a carved room removes the void too, so the building is solid again
    /// and the area can be built in a second time.
    #[test]
    fn deleting_a_carved_room_makes_the_building_whole_again() {
        let mut f = building_with_room();
        let id = f.rooms[0].id;
        let carve = f.rooms[0].carve.expect("carved");
        f.recompute();
        let hollow = f.cached.positions.len();

        f.delete_room(id);
        assert!(
            !f.model.features.iter().any(|x| x.id == carve),
            "the void must go with the room, or the hole is permanent",
        );
        f.recompute();
        // A solid building has FEWER triangles than one with a room-sized void cut through it.
        assert!(
            f.cached.positions.len() < hollow,
            "the building should be solid again ({} tris vs {hollow} hollow)",
            f.cached.positions.len(),
        );

        // …and the same area accepts a new room.
        f.add_room(&inner(1.0, 9.0)).expect("the area can be built in again");
        assert_eq!(f.rooms.len(), 1);
    }

    /// Raising a carved room's height cuts further up through the building, rather than leaving a
    /// lip of material over the room.
    #[test]
    fn raising_a_carved_room_takes_the_void_with_it() {
        let mut f = building_with_room();
        let id = f.rooms[0].id;
        let carve = f.rooms[0].carve.expect("carved");
        let before = match f.model.features.iter().find(|x| x.id == carve).unwrap().primitive {
            Primitive::Extrusion { h, .. } => h,
            _ => panic!("the void is an extrusion"),
        };
        // A MODEST raise must not shrink the void — that would leave a cap of building
        // material sitting over the room.
        f.set_room_height(id, 3.6);
        let modest = match f.model.features.iter().find(|x| x.id == carve).unwrap().primitive {
            Primitive::Extrusion { h, .. } => h,
            _ => panic!("the void is an extrusion"),
        };
        assert!(
            (modest - before).abs() < 1e-6,
            "still punching clear through the building ({before} → {modest})",
        );
        // Raising it PAST the building does grow the void, so the room is never capped.
        f.set_room_height(id, 6.0);
        let tall = match f.model.features.iter().find(|x| x.id == carve).unwrap().primitive {
            Primitive::Extrusion { h, .. } => h,
            _ => panic!("the void is an extrusion"),
        };
        assert!(tall > before, "the void grew with the room ({before} → {tall})");
    }

    /// The suggested height is the building less both slabs — the sum users were getting wrong.
    #[test]
    fn the_suggested_height_fills_the_building_exactly() {
        let mut f = FactoryState::default();
        f.building_height = 4.0;
        f.room_floor = 0.2;
        f.ceiling_thickness = 0.15;
        f.room_open_top = false;
        let want = f.suggested_room_height();
        assert!((want - 3.65).abs() < 1e-6, "4.00 − 0.20 − 0.15 = 3.65, got {want}");

        f.room_height = want;
        f.add_room(&square(5.0)).expect("room");
        let over = f.rooms[0].overall_height();
        assert!((over - 4.0).abs() < 1e-6, "which builds to exactly the building height, got {over}");
    }

    /// Open to sky: no ceiling slab, so the suggestion has more room to give.
    #[test]
    fn an_open_topped_room_is_offered_the_ceiling_back() {
        let mut f = FactoryState::default();
        f.building_height = 4.0;
        f.room_floor = 0.2;
        f.ceiling_thickness = 0.15;
        f.room_open_top = true;
        assert!((f.suggested_room_height() - 3.8).abs() < 1e-6, "4.00 − 0.20 = 3.80");
    }

    /// Floor and ceiling thickness are adjustable after the fact, and everything above the floor
    /// moves with it — otherwise a thicker slab would swallow the bottom of the walls.
    #[test]
    fn floor_and_ceiling_thickness_can_be_changed_after_building() {
        let mut f = FactoryState::default();
        f.room_floor = 0.2;
        f.room_height = 3.0;
        f.ceiling_thickness = 0.15;
        f.add_room(&square(5.0)).expect("room");
        let id = f.rooms[0].id;
        f.recompute();
        let before = f.cached.bounds().expect("bounds").1[2];
        assert!((before - 3.35).abs() < 1e-3);

        f.set_room_floor(id, 0.4);
        f.recompute();
        let after = f.cached.bounds().expect("bounds").1[2];
        assert!((after - 3.55).abs() < 1e-3, "0.40 + 3.00 + 0.15 = 3.55, got {after}");

        f.set_room_ceiling(id, 0.3);
        f.recompute();
        let last = f.cached.bounds().expect("bounds").1[2];
        assert!((last - 3.70).abs() < 1e-3, "0.40 + 3.00 + 0.30 = 3.70, got {last}");
    }
}

/// No length field in the 3D Factory may be hard-coded to metres.
#[cfg(test)]
mod working_unit_coverage {
    /// **A source-level guard for the whole class of bug.**
    ///
    /// The unit sweep counted metre-suffixed fields in `app.rs` and missed `primitive_dim_fields`
    /// here — so the properties panel showed `height 4.00 m` with millimetre fields directly above
    /// it, and typing 1000 for a metre-tall solid produced one a KILOMETRE tall.
    ///
    /// A grep is a blunt test and it is the right one here: the failure was not a wrong
    /// calculation but a field that never went through the conversion at all, and nothing about
    /// its behaviour distinguishes it until someone types into it. Checking the source catches the
    /// next one at compile-of-the-test time rather than in a screenshot.
    ///
    /// `light.rs` is deliberately exempt: SIMLUX quotes mounting height and work plane in metres
    /// because every lighting standard and report does, and matching the Factory there would put
    /// the panel at odds with the documents it exists to produce.
    #[test]
    fn no_factory_length_field_is_hard_coded_to_metres() {
        for (file, src) in [
            ("factory.rs", include_str!("factory.rs")),
            ("app.rs", include_str!("app.rs")),
        ] {
            // Assembled at RUNTIME so this line does not contain the pattern it hunts for — a
            // literal here would make the guard fail on itself, permanently.
            let needle = format!("suffix(\" {}\")", "m");
            let bad: Vec<usize> = src
                .lines()
                .enumerate()
                .filter(|(_, l)| l.contains(&needle))
                .map(|(i, _)| i + 1)
                .collect();
            assert!(
                bad.is_empty(),
                "{file}: length fields at lines {bad:?} are hard-coded to metres. Use \
                 `crate::factory::length_ui`, which shows the Factory working unit — a millimetre \
                 project must not have a metre field hiding among them.",
            );
        }
    }
}

/// The building a room is judged against is the one that is standing, not a stale template.
///
/// Reported as "why does it still say the building is 3000 mm even though I changed it to 4000 mm?
/// it builds it correctly though" — the geometry was right and the number beside it was three
/// versions out of date.
#[cfg(test)]
mod building_height_truth {
    use super::*;

    fn square(m: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(m, 0.0),
            Vec2::new(m, m),
            Vec2::new(0.0, m),
            Vec2::new(0.0, 0.0),
        ]
    }

    /// Resizing the building through its own feature — which is what the properties panel does —
    /// changes what a room is measured against, even though the template field is untouched.
    #[test]
    fn the_building_is_measured_not_remembered() {
        let mut f = FactoryState::default();
        assert!((f.building_height - 3.0).abs() < 1e-6, "the template starts at 3 m");
        let id = f.add_building_outline(&square(10.0), 3.0).expect("building");
        assert!((f.effective_building_height() - 3.0).abs() < 1e-3);

        // Raise it the way the properties panel does: edit the primitive directly.
        if let Some(feat) = f.model.get_mut(id) {
            if let Primitive::Extrusion { profile, w, d, .. } = feat.primitive {
                feat.primitive = Primitive::Extrusion { profile, h: 4.0, w, d };
            }
        }
        assert!(
            (f.effective_building_height() - 4.0).abs() < 1e-3,
            "the standing building is 4 m, whatever the template still says",
        );
        assert!((f.building_height - 3.0).abs() < 1e-6, "and the template really is untouched");
    }

    /// A 4 m room in a 4 m building raises NO warning. Against the stale 3 m template it claimed
    /// the room stood 1000 mm proud of the top — the report exactly.
    #[test]
    fn a_room_that_fits_the_real_building_is_not_warned_about() {
        let mut f = FactoryState::default();
        let id = f.add_building_outline(&square(10.0), 3.0).expect("building");
        if let Some(feat) = f.model.get_mut(id) {
            if let Primitive::Extrusion { profile, w, d, .. } = feat.primitive {
                feat.primitive = Primitive::Extrusion { profile, h: 4.0, w, d };
            }
        }
        f.room_floor = 0.05;
        f.room_height = 3.9;
        f.ceiling_thickness = 0.05;
        f.add_room(&vec![
            Vec2::new(1.0, 1.0),
            Vec2::new(9.0, 1.0),
            Vec2::new(9.0, 9.0),
            Vec2::new(1.0, 9.0),
            Vec2::new(1.0, 1.0),
        ])
        .expect("room");
        assert!(
            (f.rooms[0].overall_height() - 4.0).abs() < 1e-3,
            "50 + 3900 + 50 = 4000 mm overall",
        );
        assert!(
            !f.status.contains("taller than"),
            "it fits the 4 m building that is standing: {}",
            f.status,
        );
    }

    /// The suggestion follows the real building too.
    #[test]
    fn the_suggested_height_follows_the_standing_building() {
        let mut f = FactoryState::default();
        f.room_floor = 0.05;
        f.ceiling_thickness = 0.05;
        let id = f.add_building_outline(&square(10.0), 3.0).expect("building");
        if let Some(feat) = f.model.get_mut(id) {
            if let Primitive::Extrusion { profile, w, d, .. } = feat.primitive {
                feat.primitive = Primitive::Extrusion { profile, h: 4.0, w, d };
            }
        }
        assert!(
            (f.suggested_room_height() - 3.9).abs() < 1e-3,
            "4.00 − 0.05 − 0.05 = 3.90, got {}",
            f.suggested_room_height(),
        );
    }

    /// And the template field now RESIZES what is standing, rather than describing nothing.
    #[test]
    fn setting_the_height_resizes_the_building() {
        let mut f = FactoryState::default();
        f.add_building_outline(&square(10.0), 3.0).expect("building");
        assert_eq!(f.set_building_height(4.5), 1, "one mass resized");
        f.recompute();
        let top = f.cached.bounds().expect("bounds").1[2];
        assert!((top - 4.5).abs() < 1e-3, "the geometry followed, got {top}");
        assert!((f.effective_building_height() - 4.5).abs() < 1e-3);
    }

    /// Raising the building must not seal it back over a room carved out of it.
    #[test]
    fn a_taller_building_does_not_close_over_its_rooms() {
        let mut f = FactoryState::default();
        f.room_floor = 0.1;
        f.room_height = 2.7;
        f.ceiling_thickness = 0.1;
        f.add_building_outline(&square(10.0), 3.0).expect("building");
        f.add_room(&vec![
            Vec2::new(1.0, 1.0),
            Vec2::new(9.0, 1.0),
            Vec2::new(9.0, 9.0),
            Vec2::new(1.0, 9.0),
            Vec2::new(1.0, 1.0),
        ])
        .expect("room");
        let carve = f.rooms[0].carve.expect("carved");
        f.set_building_height(6.0);
        let void_h = match f.model.features.iter().find(|x| x.id == carve).unwrap().primitive {
            Primitive::Extrusion { h, .. } => h,
            _ => panic!("the void is an extrusion"),
        };
        assert!(
            void_h >= 6.0,
            "the void must still punch clear through a 6 m building, got {void_h}",
        );
    }
}

#[cfg(test)]
mod plan_footprint_tests {
    use super::*;

    fn feat(p: Primitive) -> cad_solid::Feature {
        cad_solid::Feature {
            id: 1,
            op: cad_solid::BoolOp::Union,
            plane: cad_solid::Plane::default(),
            placement: cad_solid::Placement { u: 0.0, v: 0.0, ..Default::default() },
            primitive: p,
            enabled: true,
            target: None,
            through: None,
        }
    }

    /// THE GUARANTEE. The 2D overlay drew only what `feature_world_outline` could describe, which
    /// is boxes and extrusions — so a column, a dome or a ramp built from anything round was simply
    /// absent from the plan, and absent is indistinguishable from "not there". Every primitive the
    /// app can build must put SOMETHING on the plan.
    #[test]
    fn every_primitive_has_a_plan_footprint() {
        let st = FactoryState::default();
        let all = [
            Primitive::Box { w: 2.0, d: 3.0, h: 1.0 },
            Primitive::Cylinder { r: 1.0, h: 2.0, sides: 24 },
            Primitive::Sphere { r: 1.0, segments: 16, stacks: 8 },
            Primitive::Frustum { r_bottom: 1.0, r_top: 0.5, h: 2.0, sides: 6 },
            Primitive::Torus { major_r: 2.0, minor_r: 0.3, seg_major: 24, seg_minor: 8 },
            Primitive::Capsule { r: 0.5, h: 2.0, segments: 16, stacks: 8 },
            Primitive::Tube { r_outer: 1.0, r_inner: 0.6, h: 2.0, sides: 24 },
            Primitive::Ellipsoid { rx: 2.0, ry: 1.0, rz: 0.5, segments: 16, stacks: 8 },
        ];
        for p in all {
            let fp = st.feature_plan_footprint(&feat(p));
            assert!(
                fp.len() >= 3,
                "{p:?} produced {} points — it would be invisible on the plan",
                fp.len(),
            );
            // And it must have real extent, not a degenerate dot.
            let (mut mn, mut mx) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
            for q in &fp {
                mn = mn.min(*q);
                mx = mx.max(*q);
            }
            assert!(
                mx.x - mn.x > 1e-3 && mx.y - mn.y > 1e-3,
                "{p:?} produced a degenerate footprint {mn:?}..{mx:?}",
            );
        }
    }

    /// A cylinder is a CIRCLE in plan, not the square of its bounding box. If this ever falls back
    /// to the AABB, a round column reads as a square one.
    #[test]
    fn a_cylinder_is_round_in_plan() {
        let st = FactoryState::default();
        let fp = st.feature_plan_footprint(&feat(Primitive::Cylinder { r: 1.5, h: 2.0, sides: 32 }));
        assert_eq!(fp.len(), 32);
        for q in &fp {
            let r = (q.x * q.x + q.y * q.y).sqrt();
            assert!((r - 1.5).abs() < 1e-3, "vertex at radius {r}, expected 1.5");
        }
    }

    /// A 4-sided Frustum is a PYRAMID. Drawing every round-ish primitive as a circle would put a
    /// disc where a square base is.
    #[test]
    fn a_four_sided_frustum_is_a_quadrilateral_not_a_disc() {
        let st = FactoryState::default();
        let fp = st.feature_plan_footprint(&feat(Primitive::Frustum {
            r_bottom: 1.0,
            r_top: 0.0,
            h: 2.0,
            sides: 4,
        }));
        assert_eq!(fp.len(), 4, "a pyramid has four corners in plan");
    }

    /// A tilted shape's upright outline is the WRONG outline, so it falls back to the world AABB —
    /// which must still be big enough to contain the tilted shape.
    #[test]
    fn a_tilted_solid_falls_back_to_a_bound_that_contains_it() {
        let st = FactoryState::default();
        let mut f = feat(Primitive::Box { w: 4.0, d: 1.0, h: 1.0 });
        f.placement.pitch_deg = 45.0;
        let fp = st.feature_plan_footprint(&f);
        let (mn, mx) = f.world_aabb();
        for q in &fp {
            assert!(
                q.x >= mn.x - 1e-3 && q.x <= mx.x + 1e-3 && q.y >= mn.y - 1e-3 && q.y <= mx.y + 1e-3,
                "footprint point {q:?} escapes the solid's own bounds",
            );
        }
        let w = fp.iter().fold(f32::NEG_INFINITY, |a, q| a.max(q.x))
            - fp.iter().fold(f32::INFINITY, |a, q| a.min(q.x));
        assert!((w - (mx.x - mn.x)).abs() < 1e-3, "the fallback must be the full bound");
    }
}

/// WHERE a newly added object lands.
///
/// Reported as: "now when a 3d object, furniture, or room element etc except the drawn furnitures
/// are being placed the user has no control it gets added at the origin. the user needs the
/// control." Every add path called `default_place_at()` and nothing could say otherwise.
#[cfg(test)]
mod placement_tests {
    use super::*;

    fn with_a_model() -> FactoryState {
        let mut st = FactoryState::default();
        // A building well away from the world origin — the DXF-coordinate case that is the whole
        // reason `Centre` exists and is not simply (0,0).
        let placement = Placement { u: 100.0, v: 50.0, ..Placement::default() };
        st.model.push(
            BoolOp::Union,
            Plane::default(),
            placement,
            Primitive::Box { w: 10.0, d: 10.0, h: 3.0 },
        );
        st.recompute();
        st
    }

    fn tri_mesh() -> crate::mesh_io::ObjMesh {
        crate::mesh_io::ObjMesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            color: None,
            alpha: Vec::new(),
        }
    }

    #[test]
    fn each_mode_resolves_to_its_own_point() {
        let mut st = with_a_model();

        st.place_mode = PlaceMode::Origin;
        let o = st.place_at();
        assert!(o.x.abs() < 1e-6 && o.y.abs() < 1e-6, "World origin means (0,0), got {o:?}");

        st.place_mode = PlaceMode::Centre;
        let c = st.place_at();
        assert!(
            (c.x - 100.0).abs() < 1e-3 && (c.y - 50.0).abs() < 1e-3,
            "Centre must follow the model out to its DXF coordinates, got {c:?}",
        );

        st.place_mode = PlaceMode::Offset;
        st.place_offset = [3.0, 4.0, 0.0];
        let f = st.place_at();
        assert!((f.x - 3.0).abs() < 1e-6 && (f.y - 4.0).abs() < 1e-6, "got {f:?}");

        // Click has to land somewhere VISIBLE while it waits, so it borrows Centre's answer. A piece
        // parked at (0,0) while the building is 100 m away is off-screen, and off-screen reads as
        // "nothing happened" — the bug this replaced.
        st.place_mode = PlaceMode::Click;
        let k = st.place_at();
        assert!((k.x - 100.0).abs() < 1e-3, "Click must park it in view, got {k:?}");
    }

    /// The default. The complaint was never that the origin is the wrong point — it is that nobody
    /// was asked.
    #[test]
    fn the_default_is_to_ask() {
        assert_eq!(FactoryState::default().place_mode, PlaceMode::Click);
    }

    /// EVERY furniture add funnels through `place_furniture`, so arming there covers imports,
    /// apertures and all nine parametric generators at once.
    #[test]
    fn adding_furniture_arms_the_placing_click() {
        let mut st = with_a_model();
        let idx = st.add_furniture_asset("chair".into(), tri_mesh());
        let at = st.place_at();
        st.place_furniture(idx, at);
        assert_eq!(
            st.awaiting_place,
            Some(AwaitingPlace::Furniture(0)),
            "a new piece must wait to be told where it goes",
        );

        // …and the click moves it, in plan only.
        let z_before = st.furniture[0].pos[2];
        let what = st.place_awaiting_at(Vec3::new(7.0, 8.0, 0.0));
        assert_eq!(what, Some(AwaitingPlace::Furniture(0)));
        assert!((st.furniture[0].pos[0] - 7.0).abs() < 1e-6);
        assert!((st.furniture[0].pos[1] - 8.0).abs() < 1e-6);
        assert!(
            (st.furniture[0].pos[2] - z_before).abs() < 1e-6,
            "Z is the storey it was built on — a ground-plane click must not drop it a floor",
        );
        assert!(st.awaiting_place.is_none(), "and the wait is over");
    }

    #[test]
    fn adding_a_solid_arms_the_placing_click() {
        let mut st = with_a_model();
        st.place_mode = PlaceMode::Click;
        st.add_box();
        let id = *st.selection.first().expect("the new box is selected");
        assert_eq!(st.awaiting_place, Some(AwaitingPlace::Feature(id)));

        st.place_awaiting_at(Vec3::new(20.0, 30.0, 0.0));
        let f = st.model.features.iter().find(|f| f.id == id).expect("still there");
        // A Box's click point is its NEAR CORNER — the meaning `place_primitive` already gives it.
        // The same click must not mean two different things depending on how the box was made.
        let (w, d) = match f.primitive {
            Primitive::Box { w, d, .. } => (w, d),
            _ => panic!("a box"),
        };
        assert!((f.placement.u - (20.0 + w * 0.5)).abs() < 1e-6, "u = {}", f.placement.u);
        assert!((f.placement.v - (30.0 + d * 0.5)).abs() < 1e-6, "v = {}", f.placement.v);
    }

    /// In any mode but Click, nothing waits — the object is finished the moment it is added.
    #[test]
    fn the_other_modes_do_not_wait() {
        for m in [PlaceMode::Centre, PlaceMode::Origin, PlaceMode::Offset] {
            let mut st = with_a_model();
            st.place_mode = m;
            st.add_box();
            assert!(st.awaiting_place.is_none(), "{m:?} must not arm a click");
            st.awaiting_place = None;
            st.add_cylinder();
            assert!(st.awaiting_place.is_none(), "{m:?} must not arm a click for a cylinder");
        }
    }

    /// An aperture FITTED into an opening the user drew is already exactly where it belongs. Asking
    /// for a click would be asking twice — "except the drawn furnitures".
    #[test]
    fn a_fitted_aperture_does_not_wait() {
        let mut st = with_a_model();
        let idx = st.add_furniture_asset("window".into(), tri_mesh());
        st.awaiting_place = None;
        st.place_aperture(idx, Vec3::new(5.0, 0.0, 1.2), Vec3::X, 1.2, 1.4, 0.2);
        assert!(
            st.awaiting_place.is_none(),
            "a drawn aperture is already in its opening — it must not ask to be placed",
        );
    }

    /// The object can go away underneath the wait — undone, or deleted from the panel. Placing then
    /// has nothing to move, and must say so rather than panic on a stale index.
    #[test]
    fn placing_something_that_is_gone_is_harmless() {
        let mut st = with_a_model();
        st.awaiting_place = Some(AwaitingPlace::Furniture(7)); // never existed
        assert_eq!(st.place_awaiting_at(Vec3::new(1.0, 1.0, 0.0)), None);
        st.awaiting_place = Some(AwaitingPlace::Feature(4242)); // no such id
        assert_eq!(st.place_awaiting_at(Vec3::new(1.0, 1.0, 0.0)), None);
    }

    /// The preference survives save/reload, like the working unit beside it.
    #[test]
    fn the_placement_preference_round_trips() {
        let mut st = with_a_model();
        st.place_mode = PlaceMode::Offset;
        st.place_offset = [1.5, -2.5, 0.75];
        let doc = st.to_persist();

        let mut back = FactoryState::default();
        back.apply_persist(doc);
        assert_eq!(back.place_mode, PlaceMode::Offset);
        assert!((back.place_offset[0] - 1.5).abs() < 1e-6);
        assert!((back.place_offset[2] - 0.75).abs() < 1e-6);
    }

    /// A project written before this existed says nothing about placement, and then the default
    /// stands — the same rule the working unit follows.
    #[test]
    fn an_older_project_keeps_the_default() {
        let st = with_a_model();
        let mut doc = st.to_persist();
        doc.place_mode = None;
        doc.place_offset = None;
        let mut back = FactoryState::default();
        back.place_mode = PlaceMode::Centre;
        back.apply_persist(doc);
        assert_eq!(back.place_mode, PlaceMode::Centre, "an absent field must not overwrite");
    }
}

/// "SHOW ME ONLY THAT FACE."
///
/// Reported as: "when click draw on this face. show me only that face on the 2d cad. because now it
/// looks confusing for the user." The underlay projected the WHOLE model onto the sketch plane, so
/// drawing on one wall of a 139 m building put every wall, room and opening in it flattened on top
/// of each other — a band of overlapping lines you cannot read, let alone draw against.
#[cfg(test)]
mod face_only_underlay {
    use super::*;

    /// Two boxes, the second offset DIAGONALLY. Drawing on the first one's -X face must not bring
    /// the second along.
    ///
    /// Diagonally on purpose. Offsetting only along the frame's normal proves nothing: projection
    /// collapses that direction, so the two boxes land on top of each other in (u,v) and "the far
    /// one is not drawn" would pass whatever the filter did — which is exactly how the first
    /// version of this fixture fooled itself. The in-plane component is what makes the far box land
    /// somewhere visibly different.
    fn two_boxes() -> FactoryState {
        let mut st = FactoryState::default();
        for k in [0.0_f32, 5.0] {
            st.model.push(
                BoolOp::Union,
                Plane::default(),
                Placement { u: k, v: k, ..Placement::default() },
                Primitive::Box { w: 2.0, d: 2.0, h: 2.0 },
            );
        }
        st.recompute();
        st
    }

    /// The near box spans x = -1..1 (w=2 centred on u=0), so its -X face is the plane x = -1.
    fn near_face() -> Frame {
        Frame::from_point_normal(Vec3::new(-1.0, 0.0, 1.0), -Vec3::X)
    }

    #[test]
    fn the_underlay_is_only_the_edges_in_the_plane() {
        let st = two_boxes();
        let frame = near_face();
        let whole = st.frame_reference_edges(&frame);
        let face = st.frame_face_edges(&frame);

        assert!(!face.is_empty(), "the face itself must still be drawn");
        assert!(
            face.len() < whole.len(),
            "the whole model projected {} edges and the face {} — the filter did nothing",
            whole.len(),
            face.len(),
        );

        // Every surviving edge must lie IN the plane: that is what makes it part of this face.
        let n = frame.normal();
        let d = n.dot(frame.origin);
        for e in &face {
            for uv in e {
                let w = frame.from_uv(*uv);
                assert!(
                    (n.dot(w) - d).abs() < 1e-3,
                    "an edge {:.4} m off the plane survived",
                    (n.dot(w) - d).abs(),
                );
            }
        }
    }

    /// The specific confusion: the OTHER object, 5 m away, used to be drawn on top of this face.
    #[test]
    fn a_solid_somewhere_else_is_not_drawn_on_this_face() {
        let st = two_boxes();
        let frame = near_face();
        // The far box spans u 4..6 in world x; on this frame that is a big |u|. The near box's own
        // face spans 2 m, so anything beyond that came from somewhere else.
        let face = st.frame_face_edges(&frame);
        let widest = face
            .iter()
            .flatten()
            .map(|p| p.x.abs().max(p.y.abs()))
            .fold(0.0_f32, f32::max);
        assert!(
            widest < 3.0,
            "the underlay reaches {widest:.2} m from the face centre — the far box is in it",
        );
    }

    /// A face 200 mm behind this one — the far side of a wall — is a DIFFERENT face. The tolerance
    /// has to be tight enough to tell them apart, or drawing on a wall shows both of its sides.
    #[test]
    fn the_far_side_of_a_wall_is_a_different_face() {
        let mut st = FactoryState::default();
        st.model.push(
            BoolOp::Union,
            Plane::default(),
            Placement::default(),
            // A 200 mm-thick wall: faces at y = -0.1 and y = +0.1.
            Primitive::Box { w: 4.0, d: 0.2, h: 3.0 },
        );
        st.recompute();
        let front = Frame::from_point_normal(Vec3::new(0.0, -0.1, 1.5), -Vec3::Y);
        let face = st.frame_face_edges(&front);
        let whole = st.frame_reference_edges(&front);
        assert!(!face.is_empty(), "the near side must be drawn");
        // COUNTS, not positions. `from_uv` reconstructs a point ON the plane by definition, so
        // asserting that the result is on the plane asserts nothing — the first version of this
        // test did exactly that and passed with the filter disabled. The wall's two faces are
        // identical rectangles landing on top of each other in (u,v), so the only thing that tells
        // them apart is how many edges came through.
        assert!(
            face.len() < whole.len(),
            "the far side, 200 mm away, came through: {} of {} edges",
            face.len(),
            whole.len(),
        );
    }

    /// And the whole-model projection is still available — the sweep flow and the face-pick preview
    /// want it, and this must not have quietly changed what they get.
    #[test]
    fn the_whole_model_projection_still_exists() {
        let st = two_boxes();
        let whole = st.frame_reference_edges(&near_face());
        let widest = whole
            .iter()
            .flatten()
            .map(|p| p.x.abs().max(p.y.abs()))
            .fold(0.0_f32, f32::max);
        assert!(widest > 3.0, "it must still reach the far box, got {widest:.2} m");
    }

    /// The global view is what it is called now — "the ground plan is going to be renamed as global
    /// view" — and it is not a face of anything.
    #[test]
    fn the_ground_plane_is_called_the_global_view() {
        let st = FactoryState::default();
        assert_eq!(st.sketch_auto_name(&FactoryState::ground_frame()), "Global view");
    }
}

/// SOLIDS OBEY THE PLACEMENT MODE TOO.
///
/// Reported as: "the 3d solid also should follow the same placing modes. make sure you include them
/// as well." They did not. `add_box` / `add_cylinder` / `add_primitive` armed the placing CLICK, so
/// Click mode appeared to work — but they all built at `Placement::default()`, i.e. always (0, 0),
/// so Centre, Origin and Offset did nothing at all to a solid.
#[cfg(test)]
mod solids_follow_the_mode {
    use super::*;

    /// A model far from the world origin, so Centre and Origin cannot be confused for each other.
    fn far_model() -> FactoryState {
        let mut st = FactoryState::default();
        st.model.push(
            BoolOp::Union,
            Plane::default(),
            Placement { u: 100.0, v: 50.0, ..Placement::default() },
            Primitive::Box { w: 10.0, d: 10.0, h: 3.0 },
        );
        st.recompute();
        st
    }

    fn newest(st: &FactoryState) -> &Feature {
        let id = *st.selection.first().expect("the new solid is selected");
        st.model.features.iter().find(|f| f.id == id).expect("still there")
    }

    /// A cylinder's point is its CENTRE, so its placement lands on the point exactly.
    #[test]
    fn a_cylinder_lands_where_each_mode_says() {
        for (mode, want) in [
            (PlaceMode::Origin, (0.0_f32, 0.0_f32)),
            (PlaceMode::Centre, (100.0, 50.0)),
            (PlaceMode::Offset, (3.0, -4.0)),
        ] {
            let mut st = far_model();
            st.place_mode = mode;
            st.place_offset = [3.0, -4.0, 0.0];
            st.add_cylinder();
            let f = newest(&st);
            assert!(
                (f.placement.u - want.0).abs() < 1e-3 && (f.placement.v - want.1).abs() < 1e-3,
                "{mode:?}: got ({}, {}), want {want:?}",
                f.placement.u,
                f.placement.v,
            );
        }
    }

    /// A Box's point is its NEAR CORNER — the convention `place_primitive` and the placing click
    /// both use. The same box must land in the same spot however it got there.
    #[test]
    fn a_box_uses_the_same_corner_rule_as_the_click() {
        let mut st = far_model();
        st.place_mode = PlaceMode::Origin;
        st.add_box();
        let f = newest(&st);
        let (w, d) = match f.primitive {
            Primitive::Box { w, d, .. } => (w, d),
            _ => panic!("a box"),
        };
        assert!((f.placement.u - w * 0.5).abs() < 1e-4, "u = {}", f.placement.u);
        assert!((f.placement.v - d * 0.5).abs() < 1e-4, "v = {}", f.placement.v);

        // …and a click to the SAME point puts it in the SAME place.
        let mut click = far_model();
        click.place_mode = PlaceMode::Click;
        click.add_box();
        click.place_awaiting_at(Vec3::ZERO);
        let g = newest(&click);
        assert!((g.placement.u - f.placement.u).abs() < 1e-4);
        assert!((g.placement.v - f.placement.v).abs() < 1e-4);
    }

    /// The dialog-built primitives go through `add_primitive`, and they are solids too.
    #[test]
    fn a_dialog_primitive_follows_the_mode() {
        let mut st = far_model();
        st.place_mode = PlaceMode::Offset;
        st.place_offset = [7.0, 8.0, 0.0];
        st.add_primitive(Primitive::Sphere { r: 1.0, segments: 16, stacks: 8 });
        let f = newest(&st);
        assert!((f.placement.u - 7.0).abs() < 1e-4, "u = {}", f.placement.u);
        assert!((f.placement.v - 8.0).abs() < 1e-4, "v = {}", f.placement.v);
    }

    /// The offset's Z is a HEIGHT ABOVE THE STOREY, not a replacement for it: on the first floor,
    /// `@0,0,2400` means 2.4 m above the first floor, not 2.4 m above the ground.
    #[test]
    fn the_offset_z_lifts_above_the_storey() {
        let mut st = far_model();
        st.storeys.push(Storey { name: "First".into(), height: 3.0 });
        st.active_storey = 1;
        let base = st.active_base_z();
        assert!(base > 0.0, "precondition: the active storey is off the ground");

        st.place_mode = PlaceMode::Offset;
        st.place_offset = [0.0, 0.0, 2.4];
        st.add_cylinder();
        assert!(
            (newest(&st).placement.lift - (base + 2.4)).abs() < 1e-4,
            "lift = {}, want {}",
            newest(&st).placement.lift,
            base + 2.4,
        );
    }

    /// Furniture reads the same Z the same way, or `@0,0,2400` would mean two different things
    /// depending on what you happened to be placing.
    #[test]
    fn furniture_reads_the_offset_z_the_same_way() {
        let mut st = far_model();
        let mesh = crate::mesh_io::ObjMesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            color: None,
            alpha: Vec::new(),
        };
        let idx = st.add_furniture_asset("lamp".into(), mesh);
        st.place_mode = PlaceMode::Offset;
        st.place_offset = [0.0, 0.0, 2.4];
        let at = st.place_at();
        st.place_furniture(idx, at);
        assert!(
            (st.furniture[0].pos[2] - (st.active_base_z() + 2.4)).abs() < 1e-4,
            "z = {}",
            st.furniture[0].pos[2],
        );
    }
}

/// THE GROUND GRID — as far as you can see, at a spacing you can read.
///
/// Reported as: "the grid now just shows for a small area on the canvas. lets have it for the whole
/// visible canvas on 3d factory, see if the zoom is infinite. there should be an option to turn the
/// grid on or off."
///
/// It was a fixed ±10 m patch of 1 m squares nailed to the world origin.
#[cfg(test)]
mod ground_grid {
    use super::*;

    /// The grid's reach on each axis, and how many lines it took to get there.
    fn extent(st: &FactoryState) -> (f32, usize) {
        let g = st.grid_lines();
        let mut reach = 0.0_f32;
        for v in &g {
            reach = reach
                .max((v.x - st.cam_target[0]).abs())
                .max((v.y - st.cam_target[1]).abs());
        }
        (reach, g.len() / 2)
    }

    /// THE BUG. Zoomed out, the grid has to still reach the edges of the view.
    #[test]
    fn the_grid_covers_the_view_at_any_zoom() {
        let mut st = FactoryState::default();
        for dist in [2.0_f32, 20.0, 200.0, 2000.0, 50_000.0] {
            st.cam_dist = dist;
            let (reach, _) = extent(&st);
            assert!(
                reach >= dist,
                "at cam_dist {dist} the grid reaches only {reach:.1} m — the view runs off it",
            );
        }
    }

    /// …without becoming a solid block of lines doing it. A fixed 1 m spacing at 50 km would be
    /// 100 000 lines; the 1-2-5 ladder keeps it in the low hundreds at every zoom.
    #[test]
    fn the_line_count_stays_bounded_at_any_zoom() {
        let mut st = FactoryState::default();
        for dist in [0.5_f32, 2.0, 20.0, 200.0, 2000.0, 50_000.0] {
            st.cam_dist = dist;
            let (_, segs) = extent(&st);
            assert!(
                (20..=260).contains(&segs),
                "cam_dist {dist} drew {segs} segments — the spacing ladder is not holding",
            );
        }
    }

    /// It FOLLOWS THE CAMERA. An imported DXF puts a building at coordinates like (−232, −58), and
    /// a grid pinned to the world origin is a rug on an empty floor nowhere near the model.
    #[test]
    fn the_grid_follows_the_camera() {
        let mut st = FactoryState::default();
        st.cam_dist = 50.0;
        st.cam_target = [-232.0, -58.0, 0.0];
        let g = st.grid_lines();
        assert!(!g.is_empty());
        // The grid's BOUNDING BOX has to be centred on the camera. Counting vertices near the
        // centre would find none however well this worked — every vertex is at the END of a line,
        // and the ends sit at the extremes by construction. The box is the honest measure.
        let (mut mnx, mut mxx) = (f32::MAX, f32::MIN);
        let (mut mny, mut mxy) = (f32::MAX, f32::MIN);
        for v in &g {
            mnx = mnx.min(v.x);
            mxx = mxx.max(v.x);
            mny = mny.min(v.y);
            mxy = mxy.max(v.y);
        }
        let (cx, cy) = ((mnx + mxx) * 0.5, (mny + mxy) * 0.5);
        assert!(
            (cx + 232.0).abs() < 5.0 && (cy + 58.0).abs() < 5.0,
            "the grid is centred on ({cx:.1}, {cy:.1}), not on the camera at (-232, -58)",
        );
    }

    /// The spacing STEPS with the zoom rather than being fixed: 1 m squares are a wall of lines
    /// across a 145 m building and invisibly fine on a door handle.
    #[test]
    fn the_spacing_steps_with_the_zoom() {
        let spacing = |dist: f32| {
            let mut st = FactoryState::default();
            st.cam_dist = dist;
            // The two smallest distinct x values apart is one step.
            let mut xs: Vec<f32> = st.grid_lines().iter().map(|v| v.x).collect();
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            xs.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
            xs.windows(2).map(|w| w[1] - w[0]).fold(f32::MAX, f32::min)
        };
        let close = spacing(2.0);
        let far = spacing(2000.0);
        assert!(far > close * 50.0, "close {close}, far {far} — the spacing did not step");
    }

    /// Snapped to its own spacing, so the lines stand still under the model instead of crawling
    /// with the camera.
    #[test]
    fn the_lines_are_snapped_to_the_spacing() {
        let mut st = FactoryState::default();
        st.cam_dist = 20.0;
        st.cam_target = [3.7, -8.3, 0.0]; // deliberately off-grid
        let step = {
            let mut xs: Vec<f32> = st.grid_lines().iter().map(|v| v.x).collect();
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            xs.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
            xs.windows(2).map(|w| w[1] - w[0]).fold(f32::MAX, f32::min)
        };
        for v in st.grid_lines() {
            let off = v.x / step - (v.x / step).round();
            assert!(off.abs() < 1e-3, "a line at x = {} is not on the {step} m grid", v.x);
        }
    }

    /// The toggle.
    #[test]
    fn the_grid_can_be_turned_off() {
        let mut st = FactoryState::default();
        assert!(st.show_grid, "on by default");
        assert!(!st.grid_lines().is_empty());
        st.show_grid = false;
        assert!(st.grid_lines().is_empty(), "off must mean off");
        // …and the rest of the overlay (selection boxes, axes) is untouched by it.
        st.add_box();
        st.recompute();
        assert!(!st.overlay_lines().is_empty(), "turning the grid off must not blank the overlay");
    }

    /// The 1-2-5 ladder itself: rounds UP, which is what bounds the line count.
    #[test]
    fn nice_step_climbs_the_1_2_5_ladder() {
        for (want, expect) in [
            (0.09_f32, 0.1_f32),
            (0.15, 0.2),
            (0.3, 0.5),
            (0.7, 1.0),
            (1.0, 1.0),
            (1.5, 2.0),
            (4.9, 5.0),
            (6.0, 10.0),
            (250.0, 500.0),
        ] {
            let got = nice_step(want);
            assert!((got - expect).abs() < 1e-4, "nice_step({want}) = {got}, want {expect}");
            assert!(got >= want, "it must round UP, or the line count is not bounded");
        }
        // Nonsense in, something usable out — a zero or NaN reach must not divide by zero.
        assert_eq!(nice_step(0.0), 1.0);
        assert_eq!(nice_step(f32::NAN), 1.0);
        assert_eq!(nice_step(-5.0), 1.0);
    }

    /// ZOOM RANGE — asked as "see if the zoom is infinite". It is NOT: `max_cam_dist` caps the
    /// dolly at 20× the model's largest span, never below 400 m. That is deliberate (an unbounded
    /// ortho zoom runs out of f32 and the depth range collapses), and this records the actual
    /// numbers so the answer is not a guess.
    #[test]
    fn the_zoom_out_limit_scales_with_the_model() {
        let mut st = FactoryState::default();
        assert!((st.max_cam_dist() - 400.0).abs() < 1e-3, "an empty scene gets the 400 m floor");

        st.model.push(
            BoolOp::Union,
            Plane::default(),
            Placement::default(),
            Primitive::Box { w: 1000.0, d: 10.0, h: 10.0 },
        );
        st.recompute();
        assert!(
            (st.max_cam_dist() - 20_000.0).abs() < 1.0,
            "a 1 km model gets 20 km of dolly, got {}",
            st.max_cam_dist(),
        );

        // …and the grid keeps up all the way out to that limit.
        st.cam_dist = st.max_cam_dist();
        let (reach, segs) = extent(&st);
        assert!(reach >= st.cam_dist, "the grid stops short at full zoom-out");
        assert!(segs < 300, "…and does not explode getting there: {segs} segments");
    }
}

/// THE GRID AT A SHALLOW ANGLE.
///
/// Looking straight down, the visible ground is about the camera distance. Looking ALONG it, the
/// plane runs away to the horizon — and a grid sized for the overhead case reads as a small rug in
/// the middle of the screen, which is what the first version of this did in a perspective view.
#[cfg(test)]
mod grid_pitch {
    use super::*;

    fn reach_of(st: &FactoryState) -> f32 {
        st.grid_lines()
            .iter()
            .fold(0.0_f32, |a, v| {
                a.max((v.x - st.cam_target[0]).abs()).max((v.y - st.cam_target[1]).abs())
            })
    }

    #[test]
    fn a_shallow_perspective_view_gets_a_wider_grid() {
        let mut st = FactoryState::default();
        st.ortho = false;
        st.cam_dist = 50.0;

        st.cam_pitch = std::f32::consts::FRAC_PI_2; // straight down
        let overhead = reach_of(&st);

        st.cam_pitch = 0.15; // nearly along the ground
        let shallow = reach_of(&st);

        assert!(
            shallow > overhead * 3.0,
            "overhead {overhead:.0} m vs shallow {shallow:.0} m — the grid did not open out",
        );
    }

    /// …but not without bound. A near-horizontal view must not spend the whole budget on ground
    /// that is two pixels tall.
    #[test]
    fn the_widening_is_capped() {
        let mut st = FactoryState::default();
        st.ortho = false;
        st.cam_dist = 50.0;
        for pitch in [0.0_f32, 0.001, 0.05, 0.16] {
            st.cam_pitch = pitch;
            let r = reach_of(&st);
            assert!(r <= 50.0 * 10.0 + 1.0, "pitch {pitch}: reach {r:.0} m is past the cap");
            assert!(r.is_finite(), "pitch {pitch} produced a non-finite reach");
        }
    }

    /// A LEVEL camera must not ask for an infinite grid — sin(0) is zero and the floor is what
    /// stops the division.
    #[test]
    fn a_level_camera_is_survivable() {
        let mut st = FactoryState::default();
        st.ortho = false;
        st.cam_dist = 50.0;
        st.cam_pitch = 0.0;
        let g = st.grid_lines();
        assert!(!g.is_empty(), "a level camera must still get a grid");
        assert!(g.len() / 2 <= 260, "…and a bounded one: {} segments", g.len() / 2);
        for v in &g {
            assert!(v.x.is_finite() && v.y.is_finite(), "non-finite vertex");
        }
    }

    /// Orthographic has no horizon, so it keeps the tight overhead sizing whatever the pitch says.
    #[test]
    fn orthographic_ignores_the_pitch() {
        let mut st = FactoryState::default();
        st.ortho = true;
        st.cam_dist = 50.0;
        st.cam_pitch = std::f32::consts::FRAC_PI_2;
        let a = reach_of(&st);
        st.cam_pitch = 0.05;
        let b = reach_of(&st);
        assert!((a - b).abs() < 1e-3, "ortho reach moved with pitch: {a} vs {b}");
    }

    /// And the count stays bounded through all of it — the spacing ladder scales with the reach, so
    /// a 10× wider grid is not 10× the lines.
    #[test]
    fn the_count_stays_bounded_however_wide_it_opens() {
        let mut st = FactoryState::default();
        st.ortho = false;
        for dist in [5.0_f32, 500.0, 20_000.0] {
            for pitch in [0.0_f32, 0.3, 1.2, std::f32::consts::FRAC_PI_2] {
                st.cam_dist = dist;
                st.cam_pitch = pitch;
                let segs = st.grid_lines().len() / 2;
                assert!(
                    (20..=260).contains(&segs),
                    "dist {dist} pitch {pitch}: {segs} segments",
                );
            }
        }
    }
}

/// A TYPED COORDINATE IS ONE PLACEMENT, NOT A MODE.
///
/// Reported as: "placing the objects with coordinates — once i enter a coordinate it keeps
/// inserting it in the same place again and again." Answering the coordinate prompt set
/// `place_mode = Offset` and left it there, so every later add landed on the same point, stacked
/// inside the last one, with nothing on screen saying a mode was still in force.
#[cfg(test)]
mod a_typed_coordinate_places_once {
    use super::*;

    fn a_state() -> FactoryState {
        let mut f = FactoryState::default();
        f.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box { w: 1.0, d: 1.0, h: 1.0 },
        );
        f
    }

    /// Arm a one-shot coordinate the way the command line does.
    fn type_a_coordinate(f: &mut FactoryState, at: [f32; 3]) {
        f.place_offset = at;
        if f.place_mode != PlaceMode::Offset {
            f.place_mode_before_offset = f.place_mode;
        }
        f.place_offset_once = true;
        f.place_mode = PlaceMode::Offset;
    }

    /// THE BUG: the second object must not land on the first.
    #[test]
    fn the_mode_reverts_after_one_object() {
        let mut f = a_state();
        f.place_mode = PlaceMode::Centre;
        type_a_coordinate(&mut f, [3.0, 4.0, 0.0]);

        assert_eq!(f.place_at(), Vec3::new(3.0, 4.0, 0.0), "the object being placed uses it");
        f.arm_placement(AwaitingPlace::Feature(1)); // end of the add
        assert_eq!(f.place_mode, PlaceMode::Centre, "and the mode goes back");
        assert!(!f.place_offset_once);
        assert_ne!(f.place_at(), Vec3::new(3.0, 4.0, 0.0), "the NEXT object does not stack on it");
    }

    /// It goes back to whatever was in force, not to a hardcoded default — Click is the common
    /// case and reverting to Centre would silently change where every later object lands.
    #[test]
    fn it_reverts_to_the_mode_that_was_in_force() {
        for before in [PlaceMode::Click, PlaceMode::Centre, PlaceMode::Origin] {
            let mut f = a_state();
            f.place_mode = before;
            type_a_coordinate(&mut f, [1.0, 2.0, 3.0]);
            f.arm_placement(AwaitingPlace::Feature(1));
            assert_eq!(f.place_mode, before, "reverted to the wrong mode from {before:?}");
        }
    }

    /// The STICKY mode is still available and still sticky — it is chosen from the placement menu,
    /// where it is visible, rather than implied by having typed a number once.
    #[test]
    fn choosing_offset_from_the_menu_stays_on() {
        let mut f = a_state();
        f.place_mode = PlaceMode::Offset; // as the menu sets it — no one-shot flag
        f.place_offset = [5.0, 6.0, 0.0];
        f.arm_placement(AwaitingPlace::Feature(1));
        assert_eq!(f.place_mode, PlaceMode::Offset, "an explicitly chosen mode must persist");
        assert_eq!(f.place_at(), Vec3::new(5.0, 6.0, 0.0));
    }

    /// Typing a coordinate twice in a row must not lose the original mode — the second entry must
    /// not record "Offset" as the thing to revert to.
    #[test]
    fn two_coordinates_in_a_row_still_revert_to_the_original_mode() {
        let mut f = a_state();
        f.place_mode = PlaceMode::Click;
        type_a_coordinate(&mut f, [1.0, 0.0, 0.0]);
        type_a_coordinate(&mut f, [2.0, 0.0, 0.0]); // before the first was consumed
        f.arm_placement(AwaitingPlace::Feature(1));
        assert_eq!(f.place_mode, PlaceMode::Click, "the original mode was overwritten by Offset");
    }

    /// A one-shot placement is not project state. Reloading a file must not re-arm it.
    #[test]
    fn it_does_not_survive_a_save() {
        let mut f = a_state();
        type_a_coordinate(&mut f, [7.0, 8.0, 0.0]);
        let doc = f.to_persist();
        let mut g = FactoryState::default();
        g.apply_persist(doc);
        assert!(!g.place_offset_once, "a reopened project must not be holding a typed coordinate");
    }
}

/// A ROOM MOVES AS ONE OBJECT, AND ITS PAINT GOES WITH IT.
///
/// Reported as: "when i move a building its floor and ceiling stay in place — the ceiling and
/// floor are part of building so they should stay attached to it. but make sure you dont break the
/// texture application while fixing this."
///
/// The parts were separate CSG features with nothing tying them together, so Move took whichever
/// one the click landed on. The session dump measures it: a 20 m building whose model AABB had
/// stretched to 69.31 m after two moves.
#[cfg(test)]
mod a_room_moves_as_one_object {
    use super::*;

    fn a_room() -> FactoryState {
        let mut f = FactoryState::default();
        f.add_room(&[
            Vec2::new(0.0, 0.0),
            Vec2::new(6.0, 0.0),
            Vec2::new(6.0, 4.0),
            Vec2::new(0.0, 4.0),
            Vec2::new(0.0, 0.0),
        ])
        .expect("room");
        f.recompute();
        f
    }

    /// Picking ONE part selects the whole room — the mechanism that makes Move take all of it.
    #[test]
    fn picking_one_part_selects_the_room() {
        let mut f = a_room();
        let parts = f.model.features.len();
        assert!(parts >= 3, "a room is built from several features, got {parts}");

        let one = f.model.features[0].id;
        f.selection = vec![one];
        f.expand_selection_to_groups();
        assert_eq!(
            f.selection.len(),
            parts,
            "one click must select all {parts} parts, got {:?}",
            f.selection,
        );
    }

    /// THE BUG, measured the way the dump measured it: the model's own extent must not grow when
    /// the room is moved. It grows exactly when a part is left behind.
    #[test]
    fn moving_it_does_not_stretch_the_model() {
        let mut f = a_room();
        let size = |f: &FactoryState| {
            let (mn, mx) = f.cached.bounds().expect("geometry");
            [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]]
        };
        let before = size(&f);

        f.selection = vec![f.model.features[0].id];
        f.expand_selection_to_groups();
        f.move_selection(Vec3::new(25.0, 0.0, 0.0));
        f.recompute();

        let after = size(&f);
        for k in 0..3 {
            assert!(
                (after[k] - before[k]).abs() < 1e-3,
                "the model stretched on axis {k}: {:?} → {:?} — a part stayed behind",
                before,
                after,
            );
        }
    }

    /// …and it actually WENT somewhere. A test that only checks the size would pass on a move
    /// that did nothing at all.
    #[test]
    fn all_of_it_actually_moved() {
        let mut f = a_room();
        let min_x = |f: &FactoryState| f.cached.bounds().unwrap().0[0];
        let before = min_x(&f);
        f.selection = vec![f.model.features[0].id];
        f.expand_selection_to_groups();
        f.move_selection(Vec3::new(25.0, 0.0, 0.0));
        f.recompute();
        assert!((min_x(&f) - before - 25.0).abs() < 1e-3, "the whole room must travel 25 m");
    }

    /// THE TEXTURE WARNING. A face painted before the move must still be painted after it — the
    /// key names a plane, and the plane moved.
    #[test]
    fn a_painted_face_keeps_its_paint_through_a_move() {
        let mut f = a_room();
        let id = f.model.features[0].id;
        // A face on the plane z = 0 with an upward normal, as `surface_key` would produce it.
        let key = surface_key(id, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        f.surface_texture.insert(key, 7);

        f.selection = vec![id];
        f.move_selection(Vec3::new(0.0, 0.0, 2.5)); // straight up: d changes by n·Δ = 2.5

        assert!(!f.surface_texture.contains_key(&key), "the old key must not survive");
        let want = surface_key(id, [0.0, 0.0, 2.5], [1.0, 0.0, 2.5], [0.0, 1.0, 2.5]);
        assert_eq!(
            f.surface_texture.get(&want).copied(),
            Some(7),
            "the paint must move with the face; keys are {:?}",
            f.surface_texture.keys().collect::<Vec<_>>(),
        );
    }

    /// A move ALONG a face's plane must not disturb its key at all — `n · Δ` is zero there, and a
    /// rounding slip would drop the paint on every horizontal drag.
    #[test]
    fn sliding_along_a_face_leaves_its_key_alone() {
        let mut f = a_room();
        let id = f.model.features[0].id;
        let key = surface_key(id, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        f.surface_texture.insert(key, 3);
        f.selection = vec![id];
        f.move_selection(Vec3::new(9.0, -4.0, 0.0)); // in the plane z = 0
        assert_eq!(f.surface_texture.get(&key).copied(), Some(3), "an in-plane slide changes nothing");
    }

    /// Paint on a feature that did NOT move must be left exactly where it is.
    #[test]
    fn paint_on_an_unmoved_feature_is_untouched() {
        let mut f = a_room();
        let moved = f.model.features[0].id;
        let still = f.model.features[1].id;
        let k_still = surface_key(still, [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        f.surface_texture.insert(k_still, 11);
        f.selection = vec![moved];
        f.move_selection(Vec3::new(0.0, 0.0, 2.5));
        assert_eq!(f.surface_texture.get(&k_still).copied(), Some(11));
    }

    /// Explode still works — grouping must not take away the ability to move one slab.
    #[test]
    fn a_room_can_still_be_taken_apart() {
        let mut f = a_room();
        f.selection = vec![f.model.features[0].id];
        f.expand_selection_to_groups();
        assert!(f.ungroup_selection() > 0, "Explode must release the parts");
        let one = f.model.features[0].id;
        f.selection = vec![one];
        f.expand_selection_to_groups();
        assert_eq!(f.selection, vec![one], "after Explode a part is its own object again");
    }
}

/// THE BUILDING SHELL IS PART OF THE BUILDING TOO.
///
/// The first attempt at "a room moves as one object" grouped only the features the ROOM owns —
/// floor, walls, ceiling, carve. The user's dump then picked `feature#1`, the building SHELL that
/// the room was carved out of, moved it, and everything else stayed put: "the building moving
/// thing i reported is still the same. seems like the fix isnt working."
///
/// These tests pick the shell, the way the dump did.
#[cfg(test)]
mod a_carved_building_moves_as_one_object {
    use super::*;

    /// A building with a room carved out of it — the shape the dump had: 4 features, 3 bodies,
    /// 1 cut.
    fn a_building_with_a_room() -> FactoryState {
        let mut f = FactoryState::default();
        f.add_building_outline(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(40.0, 0.0),
                Vec2::new(40.0, 20.0),
                Vec2::new(0.0, 20.0),
                Vec2::new(0.0, 0.0),
            ],
            3.0,
        )
        .expect("building");
        f.add_room(&[
            Vec2::new(1.0, 1.0),
            Vec2::new(39.0, 1.0),
            Vec2::new(39.0, 19.0),
            Vec2::new(1.0, 19.0),
            Vec2::new(1.0, 1.0),
        ])
        .expect("room");
        f.recompute();
        f
    }

    /// The shell is feature #1 and it must be in the group.
    #[test]
    fn picking_the_shell_selects_the_whole_building() {
        let mut f = a_building_with_a_room();
        let n = f.model.features.len();
        assert!(n >= 4, "expected the dump's shape, got {n} features");

        let shell = f.model.features[0].id;
        f.selection = vec![shell];
        f.expand_selection_to_groups();
        assert_eq!(
            f.selection.len(),
            n,
            "picking the shell must select all {n} parts, got {:?}",
            f.selection,
        );
    }

    /// THE REPORTED BUG, measured as the dump measured it: moving the shell must not stretch the
    /// model. It stretches by exactly the distance moved when parts stay behind.
    #[test]
    fn moving_the_shell_takes_everything_with_it() {
        let mut f = a_building_with_a_room();
        let extent = |f: &FactoryState| {
            let (mn, mx) = f.cached.bounds().expect("geometry");
            [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]]
        };
        let before = extent(&f);

        f.selection = vec![f.model.features[0].id]; // the SHELL, as the dump picked it
        f.expand_selection_to_groups();
        f.move_selection(Vec3::new(30.0, 0.0, 0.0));
        f.recompute();

        let after = extent(&f);
        for k in 0..3 {
            assert!(
                (after[k] - before[k]).abs() < 1e-3,
                "the model stretched on axis {k}: {before:?} → {after:?} — a part stayed behind",
            );
        }
        let min_x = f.cached.bounds().unwrap().0[0];
        assert!((min_x - 30.0).abs() < 1e-3, "and all of it travelled 30 m, got min x {min_x}");
    }

    /// The room is still a void in the moved building — the carve has to travel too, or the
    /// building arrives solid and the room is left as a hole in empty space.
    #[test]
    fn the_room_is_still_hollow_after_the_move() {
        let mut f = a_building_with_a_room();
        let tris = f.cached.positions.len();
        f.selection = vec![f.model.features[0].id];
        f.expand_selection_to_groups();
        f.move_selection(Vec3::new(30.0, 0.0, 0.0));
        f.recompute();
        assert_eq!(
            f.cached.positions.len(),
            tris,
            "the evaluated solid changed shape — the cut did not travel with the mass",
        );
    }

    /// TWO rooms in one building are ONE object, not two groups sharing a shell. Grouping that
    /// skipped already-grouped ids would leave the second room behind.
    #[test]
    fn a_second_room_joins_the_same_building() {
        let mut f = FactoryState::default();
        f.add_building_outline(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(40.0, 0.0),
                Vec2::new(40.0, 20.0),
                Vec2::new(0.0, 20.0),
                Vec2::new(0.0, 0.0),
            ],
            3.0,
        )
        .expect("building");
        for x in [(1.0, 18.0), (21.0, 38.0)] {
            f.add_room(&[
                Vec2::new(x.0, 1.0),
                Vec2::new(x.1, 1.0),
                Vec2::new(x.1, 19.0),
                Vec2::new(x.0, 19.0),
                Vec2::new(x.0, 1.0),
            ])
            .expect("room");
        }
        f.recompute();

        let n = f.model.features.len();
        f.selection = vec![f.model.features[0].id]; // the shell both rooms were cut from
        f.expand_selection_to_groups();
        assert_eq!(
            f.selection.len(),
            n,
            "both rooms must move with the building, got {:?} of {n}",
            f.selection,
        );
    }
}

/// A HALF-TYPED NUMBER IS NOT A VALUE YET.
///
/// Reported as: "when entering the parameters for ceiling, it doesnt allow to delete the already
/// existing values — when the user enters a new value it automatically has a 2 already there."
/// egui's `DragValue` clamps to its range on every keystroke, and a ceiling slab's minimum of
/// 0.02 m is 20 in millimetres, so clearing the field and typing snapped it to "20" mid-word.
#[cfg(test)]
mod numeric_fields_do_not_clamp_mid_keystroke {
    use super::*;

    /// Every `DragValue` in the app must carry the flag. Asserted on the SOURCE because the
    /// behaviour lives inside egui's text-edit state, which a unit test cannot drive.
    #[test]
    fn every_numeric_field_defers_its_clamp() {
        for (name, src) in [
            ("factory.rs", include_str!("factory.rs")),
            ("app.rs", include_str!("app.rs")),
            ("light.rs", include_str!("light.rs")),
        ] {
            // Cut this module off first: it names both literals, so a whole-file count counts the
            // assertions themselves. That is how the first version of this test failed.
            let src = src.split("mod numeric_fields_do_not_clamp_mid_keystroke").next().unwrap();
            let fields = src.matches("DragValue::new(").count();
            let deferred = src.matches(".update_while_editing(false)").count();
            assert_eq!(
                fields, deferred,
                "{name}: {fields} numeric fields but {deferred} defer their clamp — one that does \
                 not will snap the text to its minimum while the user is still typing",
            );
        }
    }

    /// The RANGE itself must survive — deferring the clamp must not mean abandoning it, or a
    /// ceiling could be committed at zero thickness.
    #[test]
    fn the_range_is_still_enforced() {
        let src = include_str!("factory.rs");
        let a = src.find("pub fn length_ui(").expect("the helper");
        let b = src[a..].find("\n}").map(|e| a + e).unwrap();
        let f = &src[a..b];
        assert!(f.contains(".range("), "the field must still declare its limits");
        assert!(f.contains(".update_while_editing(false)"), "…and defer them until commit");
    }
}

#[cfg(test)]
mod wall_opening_tests {
    use super::*;

    /// A WALL EDIT MUST NOT MOVE ANOTHER WALL'S OPENINGS.
    ///
    /// `rederive_wall` removes a wall's segments and rebuilds them, and the rebuild APPENDS. An
    /// opening is a Difference that must sit directly behind the body it cuts — `csg::eval` folds
    /// each Difference onto the most recent Union. So rebuilding at the END of the feature list
    /// left every cutter that used to sit behind that wall bound to whichever Union now precedes
    /// it: the neighbouring wall. Silently, on an ordinary corner drag.
    ///
    /// The assertion is on ORDER, not on geometry, because order IS the binding. A cutter that is
    /// no longer immediately after its host wall is cutting something else.
    #[test]
    fn dragging_a_corner_leaves_this_walls_opening_on_this_wall() {
        let mut st = FactoryState::default();

        // Two walls, so there is a neighbour for an opening to defect to.
        let a = st.add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0)], 0.2, 2.7)
            .expect("wall a");
        st.add_wall(vec![Vec2::new(0.0, 5.0), Vec2::new(4.0, 5.0)], 0.2, 2.7)
            .expect("wall b");

        // An opening in wall A's segment, placed the way a cut places one: directly behind it.
        let host = *st.walls[a].segments.first().expect("a segment");
        let host_at = st.model.features.iter().position(|f| f.id == host).expect("host present");
        let cutter = cad_solid::Feature {
            id: 9_000,
            op: cad_solid::BoolOp::Difference,
            plane: cad_solid::Plane::default(),
            placement: cad_solid::Placement::default(),
            primitive: cad_solid::Primitive::Box { w: 0.9, d: 1.0, h: 1.2 },
            enabled: true,
            target: None,
            through: None,
        };
        st.model.insert_at(host_at + 1, cutter);

        // Drag a corner of wall A. Segment count is unchanged.
        st.wall_move_vertex(a, 1, Vec2::new(4.5, 0.3));

        let new_host = *st.walls[a].segments.first().expect("a segment after the edit");
        let host_now = st.model.features.iter().position(|f| f.id == new_host)
            .expect("the rebuilt segment is in the model");
        let cutter_now = st.model.features.iter().position(|f| f.id == 9_000)
            .expect("the cutter is still in the model");

        assert_eq!(
            cutter_now, host_now + 1,
            "the opening is no longer directly behind its own wall — it is cutting a neighbour",
        );
    }

    // ── Vertex INSERT / DELETE: the count changes, so there is no index mapping ──────────────
    //
    // These are the cases `wall_move_vertex`'s slot restore cannot reach. An opening is re-homed
    // by geometry — behind whichever rebuilt segment still contains it — and one that fits no
    // segment is kept with `enabled` cleared rather than deleted or re-bound.

    /// Put a cutter of `size` centred on world `at`, directly behind `host`, the way a real cut
    /// places one. Returns its id.
    fn cut_on(st: &mut FactoryState, host: u32, at: Vec3, size: [f32; 3], id: u32) -> u32 {
        let cutter = cad_solid::Feature {
            id,
            op: cad_solid::BoolOp::Difference,
            plane: cad_solid::Plane::default(),
            placement: cad_solid::Placement {
                u: at.x, v: at.y, lift: at.z, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0,
            },
            primitive: cad_solid::Primitive::Box { w: size[0], d: size[1], h: size[2] },
            enabled: true,
            target: None,
            through: None,
        };
        assert!(st.model.insert_after(host, cutter), "the host body must exist");
        id
    }

    fn index_of(st: &FactoryState, id: u32) -> usize {
        st.model.features.iter().position(|f| f.id == id).expect("feature is still in the model")
    }

    /// The wall index each feature id belongs to, or `None` — "did this opening change walls?"
    /// asked directly, since order is only a proxy for it.
    fn owning_wall(st: &FactoryState, cutter: u32) -> Option<usize> {
        let at = index_of(st, cutter);
        let host = st.model.features[..at]
            .iter()
            .rev()
            .find(|f| f.op == cad_solid::BoolOp::Union)?;
        st.wall_index(host.id)
    }

    /// DELETING A VERTEX MERGES TWO SEGMENTS, and an opening on either of them belongs to the
    /// merged one. Nothing is orphaned here: the wall still runs through both openings.
    #[test]
    fn deleting_a_vertex_re_homes_the_openings_onto_the_merged_segment() {
        let mut st = FactoryState::default();
        // Wall A is COLLINEAR in three points, so deleting the middle one merges its two
        // segments into a single segment covering exactly the same ground.
        let a = st
            .add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(8.0, 0.0)], 0.2, 2.7)
            .expect("wall a");
        // A neighbour AFTER wall A, so an appended cutter would land behind it — that is the
        // defect this whole area exists to prevent, and it must have somewhere to defect to.
        let b = st
            .add_wall(vec![Vec2::new(0.0, 9.0), Vec2::new(8.0, 9.0)], 0.2, 2.7)
            .expect("wall b");

        let (s0, s1) = (st.walls[a].segments[0], st.walls[a].segments[1]);
        let w0 = cut_on(&mut st, s0, Vec3::new(2.0, 0.0, 1.2), [0.9, 1.0, 1.2], 9_001);
        let w1 = cut_on(&mut st, s1, Vec3::new(6.0, 0.0, 1.2), [0.9, 1.0, 1.2], 9_002);

        assert!(st.wall_delete_vertex(a, 1), "the middle vertex is deletable");

        assert_eq!(st.walls[a].segments.len(), 1, "the two segments merged into one");
        let merged = st.walls[a].segments[0];
        assert!(st.orphaned_cutouts().is_empty(), "both openings are still on the wall");
        for (w, name) in [(w0, "the first opening"), (w1, "the second opening")] {
            assert_eq!(owning_wall(&st, w), Some(a), "{name} changed walls");
            assert!(
                index_of(&st, w) > index_of(&st, merged),
                "{name} must sit behind the merged segment, not in front of it",
            );
            assert_ne!(owning_wall(&st, w), Some(b), "{name} defected to the neighbour");
            assert_eq!(st.model.is_enabled(w), Some(true), "{name} is still applied");
        }
    }

    /// INSERTING A VERTEX SPLITS ONE SEGMENT IN TWO, and each opening goes behind the HALF it is
    /// actually in. Both halves are the same wall, so binding to either would look right in the
    /// list and be wrong in the geometry — the assertion is on which half.
    #[test]
    fn inserting_a_vertex_re_homes_each_opening_onto_the_half_that_contains_it() {
        let mut st = FactoryState::default();
        let a = st
            .add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(8.0, 0.0)], 0.2, 2.7)
            .expect("wall a");
        let s0 = st.walls[a].segments[0];
        let near = cut_on(&mut st, s0, Vec3::new(1.0, 0.0, 1.2), [0.9, 1.0, 1.2], 9_010);
        let far = cut_on(&mut st, s0, Vec3::new(7.0, 0.0, 1.2), [0.9, 1.0, 1.2], 9_011);

        assert_eq!(st.wall_insert_vertex(a, 0, Vec2::new(4.0, 0.0)), Some(1), "vertex inserted");
        assert_eq!(st.walls[a].segments.len(), 2, "one segment became two");
        let (left, right) = (st.walls[a].segments[0], st.walls[a].segments[1]);

        assert!(st.orphaned_cutouts().is_empty(), "both openings still sit on the wall");
        // Directly behind, not merely after: `right` follows `left`, so "after left" is also
        // true of an opening that has drifted onto the far half.
        assert_eq!(
            index_of(&st, near), index_of(&st, left) + 1,
            "the opening at x = 1 must cut the 0–4 half",
        );
        assert_eq!(
            index_of(&st, far), index_of(&st, right) + 1,
            "the opening at x = 7 must cut the 4–8 half",
        );
    }

    /// AN OPENING WITH NOWHERE TO GO IS KEPT AND FLAGGED — the decision this milestone item is.
    ///
    /// Deleting the corner of an L turns two perpendicular segments into one diagonal, which runs
    /// nowhere near either original opening. There is no honest host for them. The two silent
    /// answers are both destructive: deleting them throws away the user's windows, re-binding
    /// them cuts holes in a wall they were never drawn on.
    #[test]
    fn an_opening_that_fits_no_rebuilt_segment_is_kept_and_flagged() {
        let mut st = FactoryState::default();
        let a = st
            .add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 4.0)], 0.2, 2.7)
            .expect("wall a");
        let b = st
            .add_wall(vec![Vec2::new(0.0, 9.0), Vec2::new(8.0, 9.0)], 0.2, 2.7)
            .expect("wall b");

        let (s0, s1) = (st.walls[a].segments[0], st.walls[a].segments[1]);
        let along = cut_on(&mut st, s0, Vec3::new(2.0, 0.0, 1.2), [0.9, 1.0, 1.2], 9_020);
        let up = cut_on(&mut st, s1, Vec3::new(4.0, 2.0, 1.2), [0.9, 1.0, 1.2], 9_021);

        // Delete the corner: (0,0)–(4,0)–(4,4) becomes the diagonal (0,0)–(4,4). Both openings
        // are >1.4 m off that line, and the wall is 0.2 m thick.
        assert!(st.wall_delete_vertex(a, 1), "the corner vertex is deletable");

        let both = [(along, "the opening along the base"), (up, "the opening up the side")];

        // KEPT — asserted FIRST, and deliberately. An implementation that simply drops what it
        // cannot place satisfies every flag assertion below by leaving nothing to flag, so
        // presence in the model has to be established before anything is said about the flag.
        for (w, name) in both {
            assert!(
                st.model.features.iter().any(|f| f.id == w),
                "{name} was DELETED — the user's window is gone",
            );
        }

        // FLAGGED — and not applied, which is what stops it cutting something it was never
        // drawn on.
        let orphans = st.orphaned_cutouts();
        assert_eq!(orphans.len(), 2, "both openings lost their segment, got {orphans:?}");
        for (w, name) in both {
            assert!(orphans.contains(&w), "{name} was not flagged");
            assert_eq!(st.model.is_enabled(w), Some(false), "{name} is still being applied");
            // NOT RE-BOUND — and least of all to the neighbouring wall.
            assert_ne!(owning_wall(&st, w), Some(b), "{name} was re-bound to the neighbour");
        }

        // And it is SAID. A disabled cutter makes no hole, so the wall renders whole and there is
        // nothing in the scene to notice.
        assert!(
            st.status.contains("lost their wall segment"),
            "the user was not told; status was {:?}",
            st.status,
        );
        let overlay = st.overlay_lines();
        let red = overlay.iter().filter(|v| v.r > 0.9 && v.g < 0.4 && v.b < 0.4).count();
        assert!(red > 0, "no marker was drawn for an opening that has silently stopped existing");
    }

    /// A CUT ON THE STOREY ABOVE IS NOT THIS WALL'S. The re-homing test is geometric, and in plan
    /// a wall on the next floor lines up perfectly with the one below it — only the z band tells
    /// them apart. Without that check, editing a ground-floor wall would adopt the first-floor
    /// windows sitting directly above it.
    #[test]
    fn re_homing_does_not_adopt_an_opening_from_the_storey_above() {
        let mut st = FactoryState::default();
        let a = st
            .add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(8.0, 0.0)], 0.2, 2.7)
            .expect("wall a");
        let s0 = st.walls[a].segments[0];
        // Directly above the wall in plan, but on the floor above: z = 4.0 is clear of the
        // ground-floor band [0, 2.7].
        let upstairs = cut_on(&mut st, s0, Vec3::new(2.0, 0.0, 4.0), [0.9, 1.0, 1.2], 9_030);

        assert!(st.wall_delete_vertex(a, 1), "the middle vertex is deletable");

        assert_eq!(
            st.model.is_enabled(upstairs), Some(false),
            "a cut 1.3 m above the wall's own head height was adopted as one of its openings",
        );
    }

    // ── AND THE OPENING IS STILL AN OPENING ────────────────────────────────────────────────
    //
    // Every test above asks WHERE the cutter sits, because until now position WAS the binding.
    // It is not any more: a cutter names the body it opens, and `rederive_wall` mints fresh ids
    // for every rebuilt segment — so an opening left naming the segment it was cut in names
    // nothing, cuts nothing, and the wall comes back SOLID with the cutter still sitting in
    // exactly the right place. Position assertions cannot see that. These two ask the geometry.

    /// How far along X the whole model reaches — the cheapest observable that says "the hole is
    /// still there", since each fixture puts its opening at the far end of the wall.
    fn model_max_x(st: &FactoryState) -> f32 {
        st.model.eval().bounds().expect("the model has bounds").1[0]
    }

    /// COUNT UNCHANGED (`wall_move_vertex`). The segments go back to their own slots, so the
    /// positional binding is restored for free — and the NAMED binding is not, unless it is
    /// carried across deliberately.
    #[test]
    fn an_opening_still_opens_after_its_wall_corner_is_dragged() {
        let mut st = FactoryState::default();
        let a = st.add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0)], 0.2, 2.7)
            .expect("wall a");
        let s0 = st.walls[a].segments[0];
        // Takes everything past x = 3 away, so the hole is visible in the model's extent.
        let cut = cut_on(&mut st, s0, Vec3::new(5.0, 0.0, 0.0), [4.0, 2.0, 4.0], 9_100);
        assert!((model_max_x(&st) - 3.0).abs() < 0.05, "the fixture must cut before the edit");

        // Drag the NEAR corner, so the far end — where the opening is — does not move.
        st.wall_move_vertex(a, 0, Vec2::new(-1.0, 0.0));

        let seg = st.walls[a].segments[0];
        assert_eq!(
            st.model.get(cut).and_then(|f| f.target), Some(seg),
            "the opening still names the segment it was cut in, which no longer exists",
        );
        assert!(
            (model_max_x(&st) - 3.0).abs() < 0.05,
            "the wall came back SOLID: the opening is in the right place and bound to nothing \
             (model reaches x = {})",
            model_max_x(&st),
        );
    }

    /// COUNT CHANGED (`wall_insert_vertex`). The opening is lifted out and re-homed by geometry
    /// onto the half that contains it — and has to be re-named onto that half too.
    #[test]
    fn a_re_homed_opening_still_opens_the_half_it_landed_on() {
        let mut st = FactoryState::default();
        let a = st.add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(8.0, 0.0)], 0.2, 2.7)
            .expect("wall a");
        let s0 = st.walls[a].segments[0];
        // Takes everything past x = 6 away — squarely inside the right-hand half after the split.
        let cut = cut_on(&mut st, s0, Vec3::new(8.0, 0.0, 0.0), [4.0, 2.0, 4.0], 9_101);
        assert!((model_max_x(&st) - 6.0).abs() < 0.05, "the fixture must cut before the edit");

        assert_eq!(st.wall_insert_vertex(a, 0, Vec2::new(4.0, 0.0)), Some(1), "vertex inserted");
        let right = st.walls[a].segments[1];

        assert_eq!(
            st.model.get(cut).and_then(|f| f.target), Some(right),
            "the re-homed opening does not name the half it was re-homed onto",
        );
        assert!(
            (model_max_x(&st) - 6.0).abs() < 0.05,
            "the split wall came back SOLID (model reaches x = {})",
            model_max_x(&st),
        );
    }
}

/// A PAINTED FACE KEEPS ITS PAINT.
///
/// Per-face paint is keyed by the face's WORLD PLANE, which is what lets one entry colour a whole
/// flat face however csgrs happens to tessellate it — and what makes the key move with the object.
/// A translate already carried the keys along; a rotation and a scale did not, so turning or
/// resizing a wall you had just painted reverted it to the default with nothing said.
///
/// Each test paints EVERY face of the body, transforms it, and asks whether every face of the
/// result is still painted. Asserting on the whole set rather than on one face is deliberate: a
/// remap that is right for the two faces perpendicular to the motion and wrong for the four that
/// turn would pass any single-face test written by someone who had just fixed the easy case.
#[cfg(test)]
mod a_painted_face_keeps_its_paint {
    use super::*;

    fn one_box() -> FactoryState {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        st
    }

    /// Every distinct surface in the evaluated mesh.
    fn surfaces(st: &FactoryState) -> std::collections::HashSet<SurfaceKey> {
        let p = &st.cached.positions;
        (0..p.len() / 3)
            .map(|t| surface_key(st.cached.face_ids[t], p[t * 3], p[t * 3 + 1], p[t * 3 + 2]))
            .collect()
    }

    /// Paint every face a distinguishable colour; returns how many there were.
    fn paint_every_face(st: &mut FactoryState) -> usize {
        let keys = surfaces(st);
        for (i, k) in keys.iter().enumerate() {
            st.surface_color.insert(*k, [i as f32 * 0.05, 0.5, 0.5]);
        }
        assert!(keys.len() >= 6, "a box has at least six faces, found {}", keys.len());
        keys.len()
    }

    /// After the edit, every face of the body must still resolve to a colour, and no entry may
    /// have been lost along the way.
    fn assert_every_face_still_painted(st: &FactoryState, before: usize, what: &str) {
        assert_eq!(
            st.surface_color.len(), before,
            "{what}: the paint table changed size — an entry was dropped or duplicated",
        );
        let now = surfaces(st);
        let unpainted: Vec<_> = now.iter().filter(|k| !st.surface_color.contains_key(k)).collect();
        assert!(
            unpainted.is_empty(),
            "{what}: {} of {} faces came back unpainted — the paint is keyed to a plane that is \
             no longer anywhere",
            unpainted.len(), now.len(),
        );
    }

    /// The control, and the one case that already worked.
    #[test]
    fn a_moved_body_keeps_its_paint() {
        let mut st = one_box();
        let n = paint_every_face(&mut st);
        st.move_selection(Vec3::new(3.0, -2.0, 1.5));
        st.recompute();
        assert_every_face_still_painted(&st, n, "after a move");
    }

    /// A ROTATION. Every side face's normal turns, so every one of their keys changes — the case
    /// a translate-only remap gets completely wrong.
    #[test]
    fn a_rotated_body_keeps_its_paint() {
        let mut st = one_box();
        let n = paint_every_face(&mut st);
        let id = st.selected_single().expect("the new box is selected");
        st.set_feature_rotation(id, 2, 30.0); // spin about the plane normal
        st.recompute();
        assert_every_face_still_painted(&st, n, "after a 30° spin");
    }

    /// AND A ROTATION OUT OF THE PLANE, which turns the top and bottom faces as well — a remap
    /// that only spun the sides would leave those two behind.
    #[test]
    fn a_body_tipped_out_of_its_plane_keeps_its_paint() {
        let mut st = one_box();
        let n = paint_every_face(&mut st);
        let id = st.selected_single().expect("selected");
        st.set_feature_rotation(id, 0, 25.0); // pitch
        st.recompute();
        assert_every_face_still_painted(&st, n, "after a 25° pitch");
    }

    /// A SCALE. The pose does not change at all here — `world_matrix` carries no scale — so the
    /// faces move because the primitive itself grew, and only the offsets change.
    #[test]
    fn a_scaled_body_keeps_its_paint() {
        let mut st = one_box();
        let n = paint_every_face(&mut st);
        st.scale_selection(1.7);
        st.recompute();
        assert_every_face_still_painted(&st, n, "after a 1.7x scale");
    }

    /// AND SOMEBODY ELSE'S PAINT IS LEFT ALONE. The remap partitions on the feature id, and a
    /// remap that moved every key would repaint the building next door on every drag.
    #[test]
    fn paint_on_another_body_is_untouched() {
        let mut st = one_box();
        let moving = st.selected_single().expect("selected");
        // A key belonging to a feature that is NOT selected, with a plane nothing else shares.
        let stranger: SurfaceKey = (moving + 1_000, 50, 0, 0, 777);
        st.surface_color.insert(stranger, [1.0, 0.0, 0.0]);

        st.move_selection(Vec3::new(3.0, 0.0, 0.0));
        st.set_feature_rotation(moving, 2, 40.0);
        st.scale_selection(2.0);

        assert_eq!(
            st.surface_color.get(&stranger), Some(&[1.0, 0.0, 0.0]),
            "another body's paint moved when this one did",
        );
    }
}

/// SAVING KEEPS THE FURNITURE AND THE TEXTURES.
///
/// Reported as "its not saving it properly i saved this file a number of times yet when i load it,
/// it loads an older version". The file on disk says exactly what happened: a model carrying one
/// 481,738-triangle asset and one 2048×2048 texture wrote a 30 KB sidecar with
///
///     "furniture_lib": [],  "furniture": [],  "textures": []
///
/// so reopening it gives the model MINUS everything imported into it — which is indistinguishable
/// from an older version of the file.
#[cfg(test)]
mod a_save_keeps_the_furniture {
    use super::*;

    /// A state with one imported asset placed once, and one pasted texture — built through the
    /// app's own constructors, so the fixture is what an import really produces.
    fn furnished() -> FactoryState {
        let mut st = FactoryState::default();
        let mesh = crate::mesh_io::parse_obj(
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\nf 1 2 4\nf 1 3 4\nf 2 3 4\n",
        );
        let a = st.add_furniture_asset("model_20260805-163358".into(), mesh);
        st.place_furniture(a, Vec3::new(2.99, 0.42, 0.68));
        st.add_texture("mat0".into(), 4, 4, vec![255; 4 * 4 * 4]);
        assert!(!st.furniture_lib.is_empty() && !st.furniture.is_empty() && !st.textures.is_empty());
        st
    }

    /// THE FULL SAVE keeps all three. This is the path a plain Save As takes.
    #[test]
    fn a_full_save_keeps_the_furniture_and_textures() {
        let st = furnished();
        let d = st.to_persist();
        assert_eq!(d.furniture_lib.len(), 1, "the imported asset was dropped");
        assert_eq!(d.furniture.len(), 1, "the placed instance was dropped");
        assert_eq!(d.textures.len(), 1, "the texture was dropped");
        assert!(
            !d.furniture_lib[0].pos_b64.is_empty(),
            "the asset is listed but its geometry is empty — it reopens as nothing",
        );
    }

    /// AND SO DOES THE LITE SAVE, which is the one the app actually uses: the UI thread builds a
    /// config with the geometry LEFT OUT and a worker fills the blobs, to keep the deflate off the
    /// frame. Leaving the blobs empty is the point of it — dropping the ENTRIES is not.
    #[test]
    fn the_lite_save_still_lists_the_furniture_and_textures() {
        let st = furnished();
        let d = st.to_persist_lite();
        assert_eq!(
            d.furniture_lib.len(), 1,
            "the lite path dropped the asset itself, not just its geometry — so the worker has \
             nothing to fill in and the file reopens without it",
        );
        assert_eq!(d.furniture.len(), 1, "the placed instance was dropped");
        assert_eq!(d.textures.len(), 1, "the texture was dropped");
    }

    /// AND THE WORKER HAS SOMETHING TO FILL FROM. The two lists are paired by INDEX — blob `i` is
    /// filled from raw `i` — so a mismatch in length silently pairs one asset's geometry with
    /// another's name.
    #[test]
    fn the_raw_geometry_lines_up_with_the_lite_list() {
        let st = furnished();
        let d = st.to_persist_lite();
        let raw = st.furniture_geom_flat();
        assert_eq!(
            raw.len(), d.furniture_lib.len(),
            "{} asset(s) listed but {} set(s) of geometry — the worker pairs these by index",
            d.furniture_lib.len(), raw.len(),
        );
        assert!(!raw.is_empty() && !raw[0].pos.is_empty(), "the geometry handed over is empty");
    }
}

/// THE LOD WAS SWITCHED OFF FOR EVERY ASSET IT WAS BUILT FOR.
///
/// `needs_lod` read `uvs.is_empty() && alpha.is_empty() && tris > 200_000`. Every real import
/// carries UVs and anything with glass carries per-vertex alpha, so nothing above the threshold was
/// ever decimated: on the reference gym plan all five heavy machines (467k–497k triangles) declined
/// it and the view drew 7,030,514 triangles a frame.
///
/// The guard was right about the hazard and wrong about the remedy. Welding across a texture seam
/// does not blur a texture — it stretches a triangle across whatever else the atlas holds between
/// the two islands. So the decimator now refuses to weld across a seam, a face group or a material
/// part, and carries the attributes through instead of declining the work.
#[cfg(test)]
mod seam_preserving_lod {
    use super::*;

    /// A flat strip of quads along +X, subdivided `n` times, with a TEXTURE SEAM at x = 0.44.
    ///
    /// Left of the seam the atlas coordinate is ~0.05, right of it ~0.95: the two sides touch in
    /// space and sit at opposite ends of the image, which is exactly what a seam is. Nothing in the
    /// source has a `u` anywhere near the middle — that is what makes the assertion below sharp.
    fn seamed_strip(n: usize) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>) {
        let (mut pos, mut uv, mut face) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..n {
            for j in 0..n {
                let (x0, x1) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
                let (y0, y1) = (j as f32 / n as f32, (j + 1) as f32 / n as f32);
                let u = |x: f32| if x < 0.44 { 0.05 } else { 0.95 };
                for (a, b, c) in [((x0, y0), (x1, y0), (x1, y1)), ((x0, y0), (x1, y1), (x0, y1))] {
                    for (x, y) in [a, b, c] {
                        pos.push([x, y, 0.0]);
                        uv.push([u(x), y]);
                    }
                    face.push(0);
                }
            }
        }
        (pos, uv, face)
    }

    /// THE SEAM SURVIVES. Every output UV must be a value the source could have produced.
    ///
    /// If a cluster had swallowed both sides of the seam its mean `u` would be 0.5, and no source
    /// vertex is anywhere near 0.5 — so a UV in the middle of the range is proof of a weld across
    /// the atlas, which is the exact failure the old `uvs.is_empty()` guard existed to avoid.
    #[test]
    fn a_texture_seam_is_never_welded_across() {
        let (pos, uv, face) = seamed_strip(40);
        let out = cluster_decimate_attr(&pos, &[], &uv, &[], &face, 8);
        assert!(!out.positions.is_empty(), "it must produce a proxy at all");
        assert_eq!(out.uvs.len(), out.positions.len(), "UVs stay per-vertex");
        for t in &out.uvs {
            assert!(
                t[0] <= 0.2 || t[0] >= 0.8,
                "u = {:.3} is between the two islands, so a cluster spanned the seam",
                t[0],
            );
        }
    }

    /// …AND IT STILL DECIMATES. A seam-aware decimator that simply refused to weld anything would
    /// pass the test above and be useless.
    #[test]
    fn it_still_removes_most_of_the_triangles() {
        let (pos, uv, face) = seamed_strip(40);
        let before = pos.len() / 3;
        let out = cluster_decimate_attr(&pos, &[], &uv, &[], &face, 8);
        assert!(
            out.tri_count() * 4 < before,
            "3,200 triangles should collapse hard on an 8³ grid; got {} from {before}",
            out.tri_count(),
        );
    }

    /// TWO MATERIALS ALMOST IN THE SAME PLACE MUST NOT MERGE.
    ///
    /// A face group is what a texture is ASSIGNED to, so welding across the boundary drags one
    /// material's vertices into another's draw call and the two fight over which texture the
    /// triangle carries.
    ///
    /// The two sheets are 1 mm apart — comfortably inside one lattice cell — with identical UVs, so
    /// the face id is the only thing keeping them apart. Merged, both would be pulled to the
    /// half-way plane at z = 0.0005; separate, each keeps its own z exactly.
    ///
    /// ASSERTED ON THE GEOMETRY, NOT ON THE ID LIST. Checking merely that both ids appear in the
    /// output proves nothing at all — the id is copied from the source triangle, so it survives a
    /// merge untouched. It has to be the positions that show the two never met.
    #[test]
    fn two_face_groups_in_the_same_place_stay_apart() {
        let (mut pos, mut uv, mut face) = (Vec::new(), Vec::new(), Vec::new());
        for (g, z) in [(0u32, 0.0f32), (1, 0.001)] {
            for i in 0..20 {
                for (x, y) in [(0.0, 0.0), (1.0, 0.0), (0.5, 1.0)] {
                    pos.push([x + i as f32, y, z]);
                    uv.push([0.5, 0.5]);
                }
                face.push(g);
            }
        }
        // A SPACER FAR AWAY IN Z, and it is load-bearing. The lattice is sized to the mesh's own
        // bounds, so with only the two sheets in it the whole z extent IS the 1 mm gap and the
        // lattice separates them on its own — the test then passed with the face id deleted from
        // the key, proving nothing. With the model a metre deep, one z cell is 125 mm and the two
        // sheets sit squarely inside the same one, which is the situation the face id has to
        // resolve by itself.
        for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.5, 1.0)] {
            pos.push([x, y, 1.0]);
            uv.push([0.5, 0.5]);
        }
        face.push(2);

        let out = cluster_decimate_attr(&pos, &[], &uv, &[], &face, 8);
        assert!(!out.face.is_empty(), "the proxy must not be empty");
        assert_eq!(out.face.len(), out.tri_count(), "one face id per proxy triangle");
        assert!(out.face.contains(&0) && out.face.contains(&1), "both groups must survive");
        for t in 0..out.tri_count() {
            if out.face[t] == 2 {
                continue; // the spacer
            }
            let want = if out.face[t] == 0 { 0.0 } else { 0.001 };
            for k in t * 3..t * 3 + 3 {
                assert!(
                    (out.positions[k][2] - want).abs() < 1e-6,
                    "face {} vertex at z = {} should be exactly {want}; halfway means the two \
                     materials were welded into one cluster",
                    out.face[t],
                    out.positions[k][2],
                );
            }
        }
    }

    /// PER-VERTEX ALPHA COMES THROUGH. Glass is split from frame by it, so a proxy that dropped it
    /// would draw every pane opaque.
    #[test]
    fn per_vertex_alpha_is_carried_and_stays_per_vertex() {
        let (pos, uv, face) = seamed_strip(20);
        let alpha: Vec<f32> = pos.iter().map(|p| if p[1] < 0.5 { 0.3 } else { 1.0 }).collect();
        let out = cluster_decimate_attr(&pos, &[], &uv, &alpha, &face, 8);
        assert_eq!(out.alpha.len(), out.positions.len(), "alpha stays per-vertex");
        assert!(out.alpha.iter().any(|&a| a < ALPHA_OPAQUE), "the see-through half must survive");
        assert!(out.alpha.iter().any(|&a| a >= ALPHA_OPAQUE), "and so must the solid half");
    }

    /// A SOURCE WITH NO ATTRIBUTES PRODUCES A PROXY WITH NONE — callers test
    /// `uvs.len() == positions.len()`, and a proxy that invented UVs would fail that test in the
    /// worst way, by passing it.
    #[test]
    fn absent_attributes_stay_absent() {
        let (pos, _, face) = seamed_strip(10);
        let out = cluster_decimate_attr(&pos, &[], &[], &[], &face, 8);
        assert!(out.uvs.is_empty(), "no UVs in, no UVs out");
        assert!(out.alpha.is_empty(), "no alpha in, no alpha out");
        assert!(!out.positions.is_empty());
    }

    /// THE FIX ITSELF: a heavy asset WITH UVs must now want a proxy. This is the assertion that
    /// fails against the old `needs_lod`, and the reason the gym plan drew 7 M triangles a frame.
    #[test]
    fn a_heavy_textured_asset_now_wants_a_proxy() {
        let n = (LOD_TRI_THRESHOLD + 1) * 3;
        let mut a = FurnitureAsset::new(
            "heavy".into(),
            vec![[0.0, 0.0, 0.0]; n],
            vec![[0.0, 0.0, 1.0]; n],
            [1.0, 1.0, 1.0],
        );
        assert!(a.needs_lod(), "a bare heavy asset always did");
        a.uvs = vec![[0.0, 0.0]; n];
        assert!(a.needs_lod(), "and now it does WITH texture coordinates — this was the bug");
        a.alpha = vec![1.0; n];
        assert!(a.needs_lod(), "…and with per-vertex alpha");
    }

    /// The threshold still governs: a small asset is drawn whole, proxy or not.
    #[test]
    fn a_light_asset_is_left_alone() {
        let a = FurnitureAsset::new(
            "light".into(),
            vec![[0.0, 0.0, 0.0]; 300],
            vec![[0.0, 0.0, 1.0]; 300],
            [1.0, 1.0, 1.0],
        );
        assert!(!a.needs_lod());
    }
}

/// EVERY DRAW PATH MUST READ ONE MESH, PROXY OR FULL — never half of each.
///
/// The tests beside `cluster_decimate_attr` prove the PROXY is right. They say nothing about the
/// five functions that consume it, and that is where the real bug was: `furniture_faceted` took its
/// triangle COUNT from the proxy and its POSITIONS from the full mesh, so it drew the first N
/// triangles of the real mesh wearing the proxy's texture coordinates. It compiled, it did not
/// panic — the proxy is shorter, so every index was in range — and it shipped.
///
/// The signal was the compiler's own `unused variable: src_nrm` / `src_face`: the loop had quietly
/// stopped consulting two of the five things it destructured. These tests are the version that does
/// not depend on anyone reading a warning.
#[cfg(test)]
mod lod_consumers_read_one_mesh {
    use super::*;

    /// An asset over the LOD threshold, with UVs, two material parts and some glass — so every
    /// consumer has something of each to get wrong.
    fn a_heavy_textured_asset() -> FactoryState {
        let mut f = FactoryState::default();
        let n = LOD_TRI_THRESHOLD + 50;
        let (mut pos, mut uv, mut alpha) = (Vec::new(), Vec::new(), Vec::new());
        for t in 0..n {
            let a = t as f32 * 0.0013;
            let z = if t % 2 == 0 { 0.0 } else { 0.7 };
            for (dx, dy) in [(0.0f32, 0.0f32), (0.01, 0.0), (0.0, 0.01)] {
                pos.push([a.cos() + dx, a.sin() + dy, z]);
                uv.push([(a * 0.1).fract().abs(), (a * 0.07).fract().abs()]);
                alpha.push(if t % 5 == 0 { 0.3 } else { 1.0 });
            }
        }
        let normals = vec![[0.0, 0.0, 1.0]; pos.len()];
        let idx = f.add_furniture_asset(
            "heavy".into(),
            crate::mesh_io::ObjMesh {
                positions: pos,
                normals,
                color: Some([0.7, 0.7, 0.7]),
                alpha,
            },
        );
        f.place_furniture(idx, Vec3::new(0.0, 0.0, 0.0));
        f
    }

    /// THE FACETED SPLIT — the one that was wrong. Every emitted vertex must be a vertex of the
    /// proxy, because that is where its triangle count came from.
    #[test]
    fn the_faceted_split_emits_only_proxy_vertices() {
        let mut f = a_heavy_textured_asset();
        let asset = &f.furniture_lib[0];
        assert!(asset.needs_lod(), "the fixture must actually be proxied");
        let lod = asset.lod_geom();
        let proxy: std::collections::HashSet<[u32; 3]> =
            lod.positions.iter().map(|p| [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]).collect();
        let full_only = f.furniture_lib[0]
            .positions
            .iter()
            .filter(|p| !proxy.contains(&[p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]))
            .count();
        assert!(full_only > 0, "the two meshes must differ, or this test cannot fail");

        // Give it a per-surface assignment so `furniture_faceted` actually runs.
        let tex = f.add_texture("t".into(), 2, 2, vec![255u8; 16]);
        let groups = f.furniture_lib[0].group_geom();
        let some_group = groups.face.first().copied().unwrap_or(0);
        f.furniture[0].surface_texture.insert(some_group, tex);

        let fac = f.furniture_faceted(0).expect("a faceted split");
        let mut seen = 0usize;
        for (_, _, verts) in &fac.opaque {
            for v in verts {
                seen += 1;
                assert!(
                    proxy.contains(&[v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]),
                    "a vertex at ({}, {}, {}) is not in the proxy — the split mixed two meshes",
                    v.x, v.y, v.z,
                );
            }
        }
        for (_, _, verts) in &fac.translucent {
            for v in verts {
                seen += 1;
                assert!(proxy.contains(&[v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]));
            }
        }
        if let Some((_, verts)) = &fac.flat {
            for v in verts {
                seen += 1;
                assert!(proxy.contains(&[v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]));
            }
        }
        assert!(seen > 0, "the split produced nothing to check");
    }

    /// THE WHOLE-OBJECT TEXTURED PATH, same property.
    #[test]
    fn the_textured_mesh_emits_only_proxy_vertices() {
        let mut f = a_heavy_textured_asset();
        let tex = f.add_texture("t".into(), 2, 2, vec![255u8; 16]);
        f.furniture[0].texture = Some(tex);
        let lod = f.furniture_lib[0].lod_geom();
        let proxy: std::collections::HashSet<[u32; 3]> =
            lod.positions.iter().map(|p| [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]).collect();

        let (_, _, verts) = f.furniture_textured_mesh(0).expect("a textured mesh");
        assert!(!verts.is_empty());
        for v in &verts {
            assert!(
                proxy.contains(&[v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]),
                "a vertex at ({}, {}, {}) is not in the proxy",
                v.x, v.y, v.z,
            );
        }
    }

    /// THE SOLID AND GLASS PASSES MUST PARTITION THE PROXY, not overlap and not lose triangles.
    /// Both peel on the same rule, so every proxy triangle belongs to exactly one of them.
    #[test]
    fn the_solid_and_glass_passes_partition_the_proxy() {
        let f = a_heavy_textured_asset();
        let lod = f.furniture_lib[0].lod_geom();
        let total = lod.tri_count();

        let solid = f.furniture_local_mesh(0).len() / 3;
        let glass = f.furniture_translucent_mesh(0).map(|(_, v)| v.len() / 3).unwrap_or(0);
        assert!(glass > 0, "the fixture has glass in it, so the peel must find some");
        assert_eq!(
            solid + glass,
            total,
            "every proxy triangle belongs to exactly one pass: {solid} solid + {glass} glass \
             against {total}",
        );
    }
}
