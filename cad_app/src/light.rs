//! SIMLUX lighting integration for the CAD app.
//!
//! [`LightState`] holds the lighting scene (IES profiles, surface materials,
//! luminaires, room height, ray settings) and the last computed lux grid, and
//! draws the **Light** panel. It drives the pure-Rust `cad_light` engine on the
//! shared `cad_kernel::Document`; the app paints the resulting grid as a 2D
//! false-colour overlay on the plan (see `CadApp::paint_lux_overlay`).

use std::collections::HashMap;

use cad_light::{
    MATERIAL_FURNITURE,
    bbox, calculate_maintained as calc_lux, default_materials, extrude, extrude_handles,
    installation_summary, parse_ies, parse_ldt, CalcPlane, IesProfile, Installation, LuxGrid,
    Luminaire, Maintenance, Material, Mesh, PhotometryType, RaySettings, Vertex,
};
use cad_kernel::Document;

/// Key for the always-available synthetic luminaire (works before any IES import).
pub const BUILTIN: &str = "Built-in downlight (1000 cd)";

/// A placed point that has no fitting on it yet.
///
/// The workflow is deliberately two-step — mark WHERE the lights go, then say WHICH fitting goes
/// in each spot — because that is the order the decisions are actually made: the layout comes from
/// the room and the fitting comes from a catalogue, often after the layout is agreed. An empty
/// profile name is the honest representation of "not chosen yet": the engine skips it (an unknown
/// profile contributes nothing), the marker is drawn hollow, and the toolbar says how many are
/// still waiting rather than quietly substituting a light the user never picked.
pub const UNASSIGNED: &str = "";

/// Pick radius for a luminaire marker on the 2D plan, in SCREEN pixels.
///
/// Screen-space, not world-space: a fixture must be as easy to grab zoomed out as zoomed in, and
/// this is the same reasoning the grip pick radius (`GrpHvR`) already follows.
pub const PICK_PX: f32 = 11.0;

/// A drag in progress on the plan: which fixtures, and where they were when it started.
///
/// Positions are captured at PRESS so the drag is always measured from the original pose. Applying
/// per-frame deltas instead accumulates rounding, and a drag that is nudged back to where it began
/// would not land back on the same coordinates.
#[derive(Clone, Debug)]
pub struct LumDrag {
    /// `(id, x, y)` at press time — every selected fixture moves together.
    pub start: Vec<(u32, f32, f32)>,
    /// Plan point (metres) the drag began at.
    pub from: (f32, f32),
    /// Set once the pointer has actually moved, so a press-and-release stays a click.
    pub moved: bool,
}

/// Turn the 3D Factory's evaluated solid into lighting geometry.
///
/// SIMLUX used to build its scene by EXTRUDING THE 2D DOCUMENT — every closed outline pulled up to
/// one room height. That is a fair stand-in for a bare plan and completely wrong once a building
/// exists in the Factory: the extrusion has no window or door openings, no floor slabs at their
/// real levels, no curved or sloped surfaces, and no storeys. A lighting result is only as good as
/// the room it was given, so the calculation was solving a shoebox that merely shared a footprint
/// with the model on screen.
///
/// Triangles are bucketed by ORIENTATION into the engine's three standing materials — up-facing is
/// floor (0.20), down-facing is ceiling (0.70), the rest are walls (0.50), which is what
/// `default_materials()` already defines and what a designer would assume. Reading reflectance
/// from each surface's own colour is the obvious next step; orientation is the honest starting
/// point, because a number guessed from an albedo texture is not more truthful, only more precise.
///
/// Returns empty when the model is empty, so the caller falls back to the extrusion and a
/// 2D-only project keeps working exactly as before.
pub fn meshes_from_factory(f: &crate::factory::FactoryState) -> Vec<Mesh> {
    let pos = &f.cached.positions;
    if pos.len() < 3 && f.furniture.is_empty() {
        return Vec::new();
    }
    // One bucket per material, so the engine sees four meshes and not thousands.
    let mut buckets: [Vec<Vertex>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for tri in pos.chunks_exact(3) {
        let (a, b, c) = (
            glam::Vec3::from(tri[0]),
            glam::Vec3::from(tri[1]),
            glam::Vec3::from(tri[2]),
        );
        let n = (b - a).cross(c - a).normalize_or_zero();
        if n.length_squared() < 0.5 {
            continue; // degenerate sliver: it can only add noise to the trace
        }
        // 0.7 ≈ 45°, so a surface is floor or ceiling only when it is nearer flat than upright.
        // A sloped ceiling therefore reads as a wall, which is the conservative way round.
        let id = if n.z > 0.7 { 0 } else if n.z < -0.7 { 2 } else { 1 };
        for p in [a, b, c] {
            buckets[id].push(Vertex::new(p.x, p.y, p.z));
        }
    }
    // ---- FURNITURE ---------------------------------------------------------------------------
    //
    // Until now the light engine could not see furniture AT ALL: this function read
    // `cached.positions`, which is the CSG solid mesh, and furniture lives separately as instanced
    // assets. So every cupboard, kitchen and desk placed in the Factory was invisible to the
    // calculation, and every room was computed as an empty box.
    //
    // That is the whole of the +48 % against DIALux on the DISTRICT PEOPLE project. The engine's
    // interreflection is verified correct against the radiosity closed form, and an empty box at
    // the reported 0.70 / 0.82 / 0.72 really does produce that much light — a real shop full of
    // racks and stock does not, and its measured uniformity (U₀ 0.17 against our 0.59) says so.
    //
    // Furniture goes in under its OWN material rather than being bucketed by orientation like the
    // building: a desk top is not a floor and a cupboard side is not a wall, and giving a shop's
    // stock the ceiling's 0.70 would recreate the very error this fixes.
    for (i, inst) in f.furniture.iter().enumerate() {
        let Some(asset) = f.furniture_lib.get(inst.asset) else { continue };
        let Some(m) = f.furniture_model_matrix(i) else { continue };
        let m = glam::Mat4::from_cols_array(&m);
        for p in &asset.positions {
            let w = m.transform_point3(glam::Vec3::from(*p));
            buckets[MATERIAL_FURNITURE as usize].push(Vertex::new(w.x, w.y, w.z));
        }
    }

    let mut out = Vec::new();
    for (id, verts) in buckets.into_iter().enumerate() {
        if verts.is_empty() {
            continue;
        }
        // Already a per-triangle soup, so the indices are just 0,1,2,3,… Welding would save a
        // little memory and cost the sharp edges that the BVH is perfectly happy to keep.
        let triangles = (0..verts.len() as u32 / 3)
            .map(|t| cad_light::Triangle { a: t * 3, b: t * 3 + 1, c: t * 3 + 2 })
            .collect();
        out.push(Mesh { vertices: verts, triangles, material: id as u32 });
    }
    out
}

/// Plan-view extent `(min_x, min_y, max_x, max_y)` of lighting geometry, or `None` if empty.
///
/// The counterpart to `cad_light::bbox`, which measures the 2D DOCUMENT. Once the room comes from
/// the 3D model those two answer different questions: a drawing contains dimensions, notes and a
/// title block that are not part of the building, and a survey plan puts the whole thing
/// kilometres from the origin.
pub fn mesh_bbox(meshes: &[Mesh]) -> Option<(f32, f32, f32, f32)> {
    let mut b: Option<(f32, f32, f32, f32)> = None;
    for m in meshes {
        for v in &m.vertices {
            b = Some(match b {
                None => (v.x, v.y, v.x, v.y),
                Some((x0, y0, x1, y1)) => (x0.min(v.x), y0.min(v.y), x1.max(v.x), y1.max(v.y)),
            });
        }
    }
    b
}

/// Height of the CEILING over `(x, y)`, searching upward from `from_z`.
///
/// A luminaire hangs from whatever is above it, and that is not one number. The array mounted
/// everything at a single `mount_height`, which is only right in a box: a real building has a
/// lower ceiling over the entrance than over the hall, soffits round the perimeter, and slopes.
/// One height for all of them buries some fixtures in the slab above and leaves others floating a
/// metre below the ceiling — and the lux result then describes that, not the design.
///
/// "Ceiling" means the nearest DOWN-FACING surface above the point: the underside of something. An
/// up-facing triangle at the same height is the TOP of that slab, seen from the floor above, and
/// fixing a luminaire to it would put it inside the structure.
///
/// `None` when nothing is overhead — an outdoor area, or a point outside the footprint — so the
/// caller keeps its own default instead of inventing a height.
pub fn ceiling_above(meshes: &[Mesh], x: f32, y: f32, from_z: f32) -> Option<f32> {
    let origin = glam::Vec3::new(x, y, from_z);
    let up = glam::Vec3::Z;
    let mut best: Option<f32> = None;
    for m in meshes {
        for t in &m.triangles {
            let (a, b, c) = (
                m.vertices[t.a as usize].to_vec3(),
                m.vertices[t.b as usize].to_vec3(),
                m.vertices[t.c as usize].to_vec3(),
            );
            // Down-facing only: a ray straight up hits both faces of a slab, and the underside is
            // the one a luminaire can be fixed to.
            let n = (b - a).cross(c - a);
            if n.z >= 0.0 {
                continue;
            }
            if let Some(d) = cad_solid::ray_triangle(origin, up, a, b, c) {
                if d > 1e-3 && best.is_none_or(|z| d < z) {
                    best = Some(d);
                }
            }
        }
    }
    best.map(|d| from_z + d)
}

/// Height of the lighting geometry (max z − min z), or `None` if empty.
pub fn mesh_height(meshes: &[Mesh]) -> Option<f32> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for m in meshes {
        for v in &m.vertices {
            lo = lo.min(v.z);
            hi = hi.max(v.z);
        }
    }
    (hi > lo).then(|| hi - lo)
}

/// Bring a fitting's emitting points down to at most [`MAX_EMITTERS_PER_FIXTURE`], merging
/// consecutive runs of them into one point each.
///
/// WHY THIS EXISTS AT DERIVE TIME AND NOT ONLY AT BUILD TIME. The count is the path length divided
/// by a spacing, and a curved light swept along a drawn 2D curve has no bound on its path: a ring
/// of 30 m radius is 188 m around, which at 0.25 m is 753 point sources for ONE fitting. Every
/// calculation point, every cylindrical sample and every surface sample then fires a shadow ray at
/// each of them, and Calculate stops responding. Capping at build time fixes fittings built from
/// now on; a project already carrying 753-point assets would still freeze on open, so the cap has
/// to hold here, where the luminaire list is actually made.
///
/// FLUX IS CONSERVED EXACTLY — the merged point carries the SUM of the points it replaces, at their
/// centroid. A longer run is therefore sampled more coarsely, not dimmed. What that costs is
/// accuracy close to the fitting, within about one spacing of it, which is the same approximation
/// the sampling was always making, just at a larger step.
fn merge_emitters(src: &[crate::factory::FurnEmitter]) -> Vec<crate::factory::FurnEmitter> {
    let max = crate::app::MAX_EMITTERS_PER_FIXTURE;
    if src.len() <= max || src.is_empty() {
        return src.to_vec();
    }
    let stride = src.len().div_ceil(max);
    src.chunks(stride)
        .map(|c| {
            let n = c.len() as f32;
            let mut pos = [0.0f32; 3];
            for e in c {
                for k in 0..3 {
                    pos[k] += e.pos[k] / n;
                }
            }
            crate::factory::FurnEmitter {
                pos,
                lumens: c.iter().map(|e| e.lumens).sum(),
                watts: c.iter().map(|e| e.watts).sum(),
            }
        })
        .collect()
}

/// Photometry for one emitting point of a diffused linear fitting, carrying `lumens` and `watts`.
///
/// LAMBERTIAN, and that is a decision worth stating rather than a default that fell out. A curved
/// light is an extrusion behind an opal diffuser, and a diffuser is the textbook Lambertian
/// emitter: its luminance is the same from every direction, which is exactly `I(γ) = I₀ cos γ`.
/// For that distribution `Φ = π·I₀`, so `I₀ = Φ/π` is forced — there is no free constant to pick.
///
/// It is an APPROXIMATION, and it is the honest one to make in the absence of a measurement. A real
/// product's LDT will differ, most where the fitting is deep and its side walls cut the beam off
/// below some angle. When one is available, import the LDT and use it; this is what the geometry
/// alone can justify, not a stand-in for having measured.
fn lambertian_profile(name: &str, lumens: f64, watts: f64) -> IesProfile {
    let vertical_angles: Vec<f64> = (0..=18).map(|i| i as f64 * 5.0).collect();
    let peak = lumens / std::f64::consts::PI;
    let candela: Vec<f64> =
        vertical_angles.iter().map(|g| peak * g.to_radians().cos().max(0.0)).collect();
    IesProfile {
        name: name.to_string(),
        photometry: PhotometryType::C,
        lumens,
        multiplier: 1.0,
        vertical_angles,
        horizontal_angles: vec![0.0],
        candela: vec![candela],
        watts,
        width: 0.0,
        length: 0.0,
        height: 0.0,
        // No aperture: one sampling point of a continuous run has no meaningful area of its own,
        // and UGR from a line source is not the sum of UGRs from the points it was sliced into.
        // Declaring zero excludes it from the glare figure — see `UgrResult::skipped_no_area` —
        // which is right, because a fabricated area here would produce a fabricated UGR.
        luminous_length: 0.0,
        luminous_width: 0.0,
    }
}

/// A cosine (Lambertian) downlight: I(γ) = 1000·cos γ cd, axially symmetric.
fn builtin_downlight() -> IesProfile {
    let vertical_angles: Vec<f64> = (0..=18).map(|i| i as f64 * 5.0).collect();
    let candela: Vec<f64> = vertical_angles
        .iter()
        .map(|g| 1000.0 * g.to_radians().cos().max(0.0))
        .collect();
    IesProfile {
        name: BUILTIN.to_string(),
        photometry: PhotometryType::C,
        lumens: -1.0,
        multiplier: 1.0,
        vertical_angles,
        horizontal_angles: vec![0.0],
        candela: vec![candela],
        watts: 0.0,
        width: 0.0,
        length: 0.0,
        height: 0.0,
        // No aperture declared, so this fitting is excluded from UGR rather than counted with a
        // made-up area — and `UgrResult::skipped_no_area` says so. The built-in is a placeholder
        // distribution for a point that has no real photometry yet; inventing a size for it would
        // put a fabricated glare figure next to the real ones.
        luminous_length: 0.0,
        luminous_width: 0.0,
    }
}

/// Side effects the panel asks the app to run (they need `&Document`).
#[derive(Default)]
pub struct LightAction {
    pub calculate: bool,
    /// Import every dobject on this source-layer id into the room (Phase B).
    pub import_layer: Option<u32>,
    /// Drop this imported room layer.
    pub remove_layer: Option<u32>,
    /// Move the current selection onto the dedicated SIMLUX layer + use it for 3D.
    pub shift_to_simlux: bool,
    /// Open the file browser to import a photometric file — the same gesture as importing
    /// furniture, because it is the same kind of act: bringing a manufacturer's product in.
    pub import_photometry: bool,
    /// Write the calculation out as a standalone HTML report. A result that lives only in a panel
    /// cannot be sent to a client or filed against a project.
    pub export_report: bool,
}

/// One imported source layer of the room: the drafted dobjects on `layer_id`,
/// extruded to a per-layer `height` (SIMLUX layer-grouped room model — D1/D2).
/// Handle-based so the set survives redraws / re-ordering of the document.
#[derive(Clone)]
pub struct RoomLayer {
    pub layer_id: u32,
    pub name: String,
    pub height: f32,
    pub handles: Vec<u64>,
}

/// All lighting UI + engine state, owned by `CadApp`.
pub struct LightState {
    /// Toggles the Light window (Tools ▸ SIMLUX Light).
    pub window_open: bool,
    /// Loaded IES profiles, keyed by name; always contains [`BUILTIN`].
    pub profiles: HashMap<String, IesProfile>,
    /// Profile used for auto-placed / new luminaires.
    pub active_profile: String,
    /// Surface materials [floor, wall, ceiling] — reflectances are editable.
    pub materials: Vec<Material>,
    /// Room (extrusion) height, metres — default height for newly imported
    /// layers and the fallback when no layer has been imported yet.
    pub room_height: f32,
    /// SIMLUX room (Phase B/C): imported source layers, each extruded to its
    /// own `height`. Empty ⇒ `calculate` falls back to extruding the whole doc.
    pub room: Vec<RoomLayer>,
    /// Work-plane height above the floor, metres (typ. 0.8 m desk height).
    pub plane_height: f32,
    /// Target grid cell size, metres (clamped to 8..64 cells per axis).
    pub cell_size: f32,
    /// Ray-tracer controls.
    pub settings: RaySettings,
    /// The maintenance factor the result is quoted at (EN 12464-1 / CIE 97).
    ///
    /// SIMLUX used to compute INITIAL illuminance and present it as the answer, which overstates a
    /// design by the whole of this factor — around 20% — and can turn a scheme that fails into one
    /// that appears to pass. Every lux figure the app now reports is maintained.
    pub maintenance: Maintenance,
    /// Connected load of the last calculation — filled in by [`LightState::calculate`].
    pub installation: Option<Installation>,
    /// Height the cylindrical illuminance is measured at — eye level. 1.2 m is the seated figure
    /// EN 12464-1 uses; 1.6 m is standing.
    pub eye_height: f32,
    /// Mean CYLINDRICAL illuminance at eye height, from the last calculation.
    ///
    /// The measure of whether a space renders faces and solid objects. A room can hold 500 lx on
    /// the desks and still read as flat and cave-like, and this is the only number that says so —
    /// EN 12464-1 asks for at least 50 lx in most occupied spaces, and more where faces matter.
    pub cylindrical_avg: Option<f64>,
    /// How many luminaires the MODEL is carrying — curved lights, counted for the status strip.
    ///
    /// Kept as a number rather than derived on the spot because the strip is drawn inside the
    /// panel closure, which already holds `self` and cannot reach the factory. Refreshed by
    /// [`Self::refresh_model_fixtures`] each frame the panel is shown.
    pub model_fixtures: usize,
    /// Per-surface illuminance and luminance from the last calculation — walls and ceiling, which
    /// EN 12464-1 sets levels for and the work plane says nothing about.
    pub surfaces: Vec<cad_light::SurfaceResult>,
    /// Placed luminaires (P4); empty ⇒ auto-place one at room centre.
    pub luminaires: Vec<Luminaire>,
    pub auto_center_light: bool,
    /// When set, canvas clicks drop a luminaire (P4 placement mode).
    pub place_mode: bool,
    /// Ids of the selected fixtures. Selection is what "assign a fitting", "delete" and "drag"
    /// all act on, so it is the one piece of state the whole editing flow shares.
    pub selected: Vec<u32>,
    /// Fixture under the pointer, refreshed each frame by the canvas handler — the marker lights
    /// up before it is pressed, so it is clear WHAT will be grabbed.
    pub hover: Option<u32>,
    /// A drag in progress, if any.
    pub drag: Option<LumDrag>,
    /// Monotonic id source for placed luminaires.
    pub next_id: u32,
    /// Rows/columns for the ▼ Luminaires grid array — the usual way a room is lit.
    pub array_rows: u32,
    pub array_cols: u32,
    /// Mount fixtures to the ceiling found ABOVE each point, rather than one fixed height.
    /// A real building has soffits, steps and slopes; one height suits only a box.
    pub mount_to_ceiling: bool,
    /// Drop below that ceiling — 0 is surface-mounted, 0.3 a short pendant.
    pub ceiling_drop: f32,
    /// Mounting height for newly placed fixtures (defaults to room height).
    pub mount_height: f32,
    /// Last computed grid + its plane + extruded scene.
    pub grid: Option<LuxGrid>,
    pub plane: Option<CalcPlane>,
    pub meshes: Vec<Mesh>,
    /// Paint the false-colour overlay on the 2D plan.
    pub show_overlay: bool,
    /// Fixed scale ceiling for the colour map (None ⇒ auto = grid max).
    pub scale_max: Option<f64>,
    /// IES file path typed into the panel.
    pub ies_path: String,
    /// Status / result line.
    pub last_msg: String,

    // ---- 3D viewport (P2) -------------------------------------------------
    /// Show the docked 3D viewport panel.
    pub view3d_open: bool,
    /// SIMLUX workspace mode — a persistent half-screen 2D | 3D split. The 3D
    /// panel is force-shown at ~half the window width and tracks the 2D drawing
    /// LIVE (extrudes the current room every frame, no Calculate needed).
    pub simlux_mode: bool,
    /// One-shot: fit the orbit camera the next time live meshes rebuild (set
    /// when the workspace is entered so the drawing is framed on arrival).
    pub simlux_fit_pending: bool,
    /// Orbit camera: yaw + pitch (radians), distance (m), target (world, Z-up).
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    pub cam_dist: f32,
    pub cam_target: [f32; 3],
    /// Paint the lux heatmap on the 3D floor (P3) rather than the floor material.
    pub floor_heatmap: bool,
    /// Which false-colour palette the scale is read through.
    pub ramp: LuxRamp,
    /// Drop the ceiling out of the SIMLUX 3D view, so the room can be seen into from above.
    ///
    /// The same need the 3D Factory's own hide-ceilings answers, and for a stronger reason here:
    /// the result being looked at is painted on the FLOOR, and a closed box hides exactly the
    /// surface the view exists to show.
    pub hide_ceilings: bool,
}

impl Default for LightState {
    fn default() -> Self {
        Self::new()
    }
}

impl LightState {
    pub fn new() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(BUILTIN.to_string(), builtin_downlight());
        Self {
            window_open: false,
            profiles,
            // NOT the built-in. Starting with a fitting already chosen makes the second step of
            // the workflow invisible: every point silently becomes a generic downlight and the
            // user never learns that a fitting is something they pick. Empty means "not chosen",
            // which is the truth on a fresh project.
            active_profile: UNASSIGNED.to_string(),
            materials: default_materials(),
            room_height: 3.0,
            room: Vec::new(),
            plane_height: 0.8,
            cell_size: 0.25,
            settings: RaySettings::default(),
            maintenance: Maintenance::default(),
            installation: None,
            eye_height: 1.2,
            cylindrical_avg: None,
            model_fixtures: 0,
            surfaces: Vec::new(),
            luminaires: Vec::new(),
            auto_center_light: true,
            place_mode: false,
            selected: Vec::new(),
            hover: None,
            drag: None,
            next_id: 1,
            array_rows: 3,
            array_cols: 4,
            mount_to_ceiling: true,
            ceiling_drop: 0.0,
            mount_height: 3.0,
            grid: None,
            plane: None,
            meshes: Vec::new(),
            show_overlay: true,
            scale_max: None,
            ies_path: String::new(),
            last_msg: "① Import your light files · ② click the plan to mark where they go · ③ pick a fitting for them."
                .to_string(),
            view3d_open: false,
            simlux_mode: false,
            simlux_fit_pending: false,
            cam_yaw: 0.7,
            cam_pitch: 0.6,
            cam_dist: 10.0,
            cam_target: [0.0, 0.0, 1.5],
            floor_heatmap: true,
            ramp: LuxRamp::default(),
            hide_ceilings: true,
        }
    }

    /// Colour-map ceiling: user override, else the current grid's max.
    pub fn scale_ceiling(&self) -> f64 {
        self.scale_max
            .or_else(|| self.grid.as_ref().map(|g| g.max))
            .unwrap_or(1.0)
            .max(1e-3)
    }

    /// The fitting a NEW point should get: the chosen one, or nothing.
    fn default_profile(&self) -> String {
        if self.profiles.contains_key(&self.active_profile) {
            self.active_profile.clone()
        } else {
            UNASSIGNED.to_string()
        }
    }

    /// Fixtures that still have no fitting on them.
    pub fn unassigned_count(&self) -> usize {
        self.luminaires
            .iter()
            .filter(|l| !self.profiles.contains_key(&l.profile))
            .count()
    }

    /// True when this fixture has a real fitting behind it — the marker is drawn solid, and the
    /// engine will actually emit from it.
    pub fn is_assigned(&self, l: &Luminaire) -> bool {
        self.profiles.contains_key(&l.profile)
    }

    /// Mounting height for a point on the plan: the ceiling above it, less the drop.
    ///
    /// Shared by every placement path — single click, grid array, and the re-mount after a drag —
    /// so a fixture moved under a lower soffit ends up exactly where the array would have put it.
    /// The search starts at the WORK PLANE, which is inside the room by definition; starting at
    /// the floor would catch the floor slab's own underside from the storey below.
    pub fn mount_z_at(&self, x: f32, y: f32) -> (f32, bool) {
        match (self.mount_to_ceiling, ceiling_above(&self.meshes, x, y, self.plane_height)) {
            (true, Some(zc)) => (zc - self.ceiling_drop, true),
            _ => (self.mount_height, false),
        }
    }

    /// The fixture nearest `(x, y)` within `tol` metres, or `None`.
    ///
    /// Nearest rather than first-within-tolerance: on a tight pitch two markers overlap, and
    /// grabbing whichever happens to be earlier in the list is how a drag moves the wrong light.
    pub fn pick_at(&self, x: f32, y: f32, tol: f32) -> Option<u32> {
        let mut best: Option<(f32, u32)> = None;
        for l in &self.luminaires {
            let (dx, dy) = (l.position.x - x, l.position.y - y);
            let d2 = dx * dx + dy * dy;
            if d2 <= tol * tol && best.is_none_or(|(b, _)| d2 < b) {
                best = Some((d2, l.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Drop a light POINT at `(x, y)` — step ② of the workflow. Returns its id.
    ///
    /// The point carries whatever fitting is currently chosen, which is usually nothing: marking
    /// out a layout does not require having decided on a product yet.
    pub fn place_point(&mut self, x: f32, y: f32) -> u32 {
        let (z, on_ceiling) = self.mount_z_at(x, y);
        let id = self.next_id;
        self.next_id += 1;
        let profile = self.default_profile();
        self.luminaires.push(Luminaire {
            id,
            profile: profile.clone(),
            position: Vertex::new(x, y, z),
            rotation_deg: 0.0,
            dimming: 1.0,
        });
        self.selected = vec![id];
        let what = if profile.is_empty() {
            "no fitting yet".to_string()
        } else {
            profile.clone()
        };
        self.last_msg = format!(
            "Point #{id} at ({x:.2}, {y:.2}) · {z:.2} m{} · {what} — {} point(s) placed.",
            if on_ceiling { " (ceiling)" } else { "" },
            self.luminaires.len(),
        );
        id
    }

    /// Select one fixture. `additive` (Shift/Ctrl) toggles it into the existing selection.
    pub fn select(&mut self, id: u32, additive: bool) {
        if additive {
            if let Some(i) = self.selected.iter().position(|&s| s == id) {
                self.selected.remove(i);
            } else {
                self.selected.push(id);
            }
        } else {
            self.selected = vec![id];
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    pub fn select_all(&mut self) {
        self.selected = self.luminaires.iter().map(|l| l.id).collect();
    }

    /// Start dragging. A press on an UNSELECTED fixture selects it first, so a drag always moves
    /// what is under the pointer rather than a selection made earlier and forgotten.
    pub fn begin_drag(&mut self, id: u32, at: (f32, f32)) {
        if !self.selected.contains(&id) {
            self.selected = vec![id];
        }
        let start = self
            .luminaires
            .iter()
            .filter(|l| self.selected.contains(&l.id))
            .map(|l| (l.id, l.position.x, l.position.y))
            .collect();
        self.drag = Some(LumDrag { start, from: at, moved: false });
    }

    /// Move the dragged fixtures so the grabbed one follows the pointer.
    pub fn drag_to(&mut self, at: (f32, f32)) {
        let Some(d) = self.drag.as_mut() else { return };
        let (dx, dy) = (at.0 - d.from.0, at.1 - d.from.1);
        if dx.abs() > 1e-4 || dy.abs() > 1e-4 {
            d.moved = true;
        }
        let start = d.start.clone();
        for (id, x0, y0) in start {
            if let Some(l) = self.luminaires.iter_mut().find(|l| l.id == id) {
                l.position.x = x0 + dx;
                l.position.y = y0 + dy;
            }
        }
    }

    /// Finish a drag: re-mount every fixture that moved, and report. Returns whether anything
    /// actually moved (a press-and-release that never moved is a click, and stays a selection).
    ///
    /// The re-mount is the point of doing this at the END rather than per frame: `ceiling_above`
    /// walks the whole model, so dragging across a 500k-triangle building would cost that on every
    /// frame — and the height is only interesting once the fixture has landed somewhere.
    pub fn end_drag(&mut self) -> bool {
        let Some(d) = self.drag.take() else { return false };
        if !d.moved {
            return false;
        }
        let ids: Vec<u32> = d.start.iter().map(|(id, _, _)| *id).collect();
        let mut zlo = f32::INFINITY;
        let mut zhi = f32::NEG_INFINITY;
        for id in &ids {
            let Some((x, y)) = self
                .luminaires
                .iter()
                .find(|l| l.id == *id)
                .map(|l| (l.position.x, l.position.y))
            else {
                continue;
            };
            let (z, _) = self.mount_z_at(x, y);
            if let Some(l) = self.luminaires.iter_mut().find(|l| l.id == *id) {
                l.position.z = z;
            }
            zlo = zlo.min(z);
            zhi = zhi.max(z);
        }
        let height = if (zhi - zlo).abs() < 1e-3 {
            format!("{zlo:.2} m")
        } else {
            format!("{zlo:.2}–{zhi:.2} m")
        };
        self.last_msg = format!(
            "Moved {} fixture(s) — now at {height}. Re-run Calculate to update the result.",
            ids.len()
        );
        true
    }

    /// Delete the selected fixtures. Returns how many went.
    pub fn delete_selected(&mut self) -> usize {
        let before = self.luminaires.len();
        let sel = std::mem::take(&mut self.selected);
        self.luminaires.retain(|l| !sel.contains(&l.id));
        let n = before - self.luminaires.len();
        if n > 0 {
            self.last_msg = format!("Deleted {n} fixture(s) — {} left.", self.luminaires.len());
        }
        self.drag = None;
        n
    }

    /// Put `name` on the fixtures that should get it — step ③.
    ///
    /// Targets, in order: the SELECTION if there is one, else every point still waiting for a
    /// fitting, else nothing (the fitting simply becomes the default for the next point placed).
    /// That order is what makes one click do the obvious thing in each of the three situations a
    /// user is actually in — some points picked out, a fresh layout to fill, or setting up before
    /// placing anything.
    pub fn assign_profile(&mut self, name: &str) -> usize {
        self.active_profile = name.to_string();
        let known: Vec<String> = self.profiles.keys().cloned().collect();
        let targets: Vec<u32> = if !self.selected.is_empty() {
            self.selected.clone()
        } else {
            self.luminaires
                .iter()
                .filter(|l| !known.contains(&l.profile))
                .map(|l| l.id)
                .collect()
        };
        for l in self.luminaires.iter_mut() {
            if targets.contains(&l.id) {
                l.profile = name.to_string();
            }
        }
        let n = targets.len();
        self.last_msg = if n == 0 {
            format!("'{name}' is now the fitting for new points — click the plan to place them.")
        } else if !self.selected.is_empty() {
            format!("Assigned '{name}' to {n} selected fixture(s) — press Calculate.")
        } else {
            format!("Assigned '{name}' to {n} point(s) that had none — press Calculate.")
        };
        n
    }

    /// Forget an imported fitting. Fixtures that used it fall back to unassigned rather than
    /// silently pointing at a profile that no longer exists.
    pub fn remove_profile(&mut self, name: &str) {
        if name == BUILTIN {
            return; // the built-in is generated, not imported — there is nothing to remove
        }
        self.profiles.remove(name);
        let mut orphaned = 0;
        for l in self.luminaires.iter_mut() {
            if l.profile == name {
                l.profile = UNASSIGNED.to_string();
                orphaned += 1;
            }
        }
        if self.active_profile == name {
            self.active_profile = UNASSIGNED.to_string();
        }
        self.last_msg = if orphaned > 0 {
            format!("Removed '{name}' — {orphaned} fixture(s) now need a fitting.")
        } else {
            format!("Removed '{name}'.")
        };
    }

    /// Load a photometric file — IES (`.ies`) or EULUMDAT (`.ldt`).
    ///
    /// The FORMAT IS CHOSEN BY CONTENT, with the extension only as a tie-break. Manufacturers
    /// rename these files constantly, and a `.ies` that is really EULUMDAT should still load
    /// rather than produce a parse error the user cannot act on.
    ///
    /// Manufacturer files are also routinely Latin-1, not UTF-8 — degree signs in luminaire names
    /// are near-universal ("PULSE MG - 14°"). `read_to_string` rejects those outright, so the
    /// bytes are read raw and mapped, which is exact for the printable range either format uses.
    fn import_photometry(&mut self) {
        let path = self.ies_path.trim().trim_matches('"').to_string();
        self.load_photometry(&path);
    }

    /// Import the photometric file at `path` into the library, and make it the chosen fitting.
    ///
    /// Public because the file browser calls it: photometry is imported the way furniture is,
    /// through the same picker, rather than by typing a path into a box.
    pub fn load_photometry(&mut self, path: &str) -> bool {
        let path = path.trim().trim_matches('"').to_string();
        if path.is_empty() {
            self.last_msg = "Enter a .ies or .ldt file path first.".to_string();
            return false;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.last_msg = format!("Read error: {e}");
                return false;
            }
        };
        let text: String = match String::from_utf8(bytes.clone()) {
            Ok(s) => s,
            Err(_) => bytes.iter().map(|&b| b as char).collect(),
        };

        // IES announces itself: every LM-63 file carries a TILT= line. EULUMDAT has no marker at
        // all, being a bare list of values, so it is what remains.
        let looks_ies = text.lines().take(60).any(|l| l.trim_start().starts_with("TILT="));
        let ext_ldt = std::path::Path::new(&path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ldt"));
        let (parsed, kind) = if looks_ies && !ext_ldt {
            (parse_ies(&text), "IES")
        } else {
            match parse_ldt(&text) {
                // A file that is neither still deserves the better of the two errors, so try the
                // other reader before giving up.
                Err(e_ldt) => match parse_ies(&text) {
                    Ok(p) => (Ok(p), "IES"),
                    Err(_) => (Err(e_ldt), "EULUMDAT"),
                },
                ok => (ok, "EULUMDAT"),
            }
        };

        match parsed {
            Ok(mut prof) => {
                if prof.name.trim().is_empty() {
                    prof.name = std::path::Path::new(&path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| kind.to_string());
                }
                let key = prof.name.clone();
                // Report the photometry, not just the name: a wrong flux or a peak in the wrong
                // place is visible here and nowhere else until the whole calculation looks odd.
                let detail = format!(
                    "{:.0} lm, {:.0} W, peak {:.0} cd",
                    prof.lumens.max(0.0),
                    prof.watts,
                    prof.peak_candela(),
                );
                self.profiles.insert(key.clone(), prof);
                self.ies_path = path.clone();
                // An import makes it the chosen fitting AND fills in any points already marked
                // out — importing a file right after laying out a grid is the common order, and
                // having to click the fitting again afterwards is a step with no decision in it.
                let n = self.assign_profile(&key);
                self.last_msg = if n > 0 {
                    format!("Loaded {kind} '{key}' ({detail}) → {n} fixture(s).")
                } else {
                    format!("Loaded {kind} '{key}' — {detail}. Click the plan to place it.")
                };
                return true;
            }
            Err(e) => self.last_msg = format!("{kind} parse error: {e}"),
        }
        false
    }

    /// Drop a luminaire at plan position (x, y) on the mounting plane.
    /// Lay out a regular grid of luminaires over `bounds`, inset from the walls.
    ///
    /// The way lighting is actually designed, and the thing that was missing. Placing fixtures one
    /// click at a time is fine for a feature light and hopeless for a room: a gym wants a 6x4 array
    /// on a regular pitch, and getting there by hand is twenty-four clicks that will not be evenly
    /// spaced. The spacing convention is the standard one — fixtures sit at the CENTRE of each
    /// cell, so the gap to the wall is half the gap between fixtures, which is what gives an even
    /// wash rather than hot edges.
    ///
    /// Returns how many were placed.
    pub fn add_luminaire_grid(
        &mut self,
        bounds: (f32, f32, f32, f32),
        rows: u32,
        cols: u32,
    ) -> usize {
        let (x0, y0, x1, y1) = bounds;
        let (rows, cols) = (rows.max(1), cols.max(1));
        let (w, d) = (x1 - x0, y1 - y0);
        if w <= 0.0 || d <= 0.0 {
            self.last_msg = "No room bounds yet — build or import geometry first.".into();
            return 0;
        }
        let (dx, dy) = (w / cols as f32, d / rows as f32);
        let mut n = 0;
        let mut found_ceiling = 0usize;
        let (mut zlo, mut zhi) = (f32::INFINITY, f32::NEG_INFINITY);
        let profile = self.default_profile();
        let mut placed = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                let x = x0 + dx * (c as f32 + 0.5);
                let y = y0 + dy * (r as f32 + 0.5);
                // Each fixture finds ITS OWN ceiling — see `mount_z_at`.
                let (z, on_ceiling) = self.mount_z_at(x, y);
                if on_ceiling {
                    found_ceiling += 1;
                }
                zlo = zlo.min(z);
                zhi = zhi.max(z);
                let id = self.next_id;
                self.next_id += 1;
                self.luminaires.push(Luminaire {
                    id,
                    profile: profile.clone(),
                    position: Vertex::new(x, y, z),
                    rotation_deg: 0.0,
                    dimming: 1.0,
                });
                placed.push(id);
                n += 1;
            }
        }
        // The new array IS the selection, so the next act — choosing a fitting for it, nudging it,
        // deleting it because the pitch was wrong — needs no further picking.
        self.selected = placed;
        // Report the SPREAD of mounting heights, not just one number: on a stepped ceiling that
        // spread is the useful fact, and it is the only sign that some fixtures found no ceiling
        // and fell back.
        let height = if (zhi - zlo).abs() < 1e-3 {
            format!("{zlo:.2} m")
        } else {
            format!("{zlo:.2}–{zhi:.2} m")
        };
        let missed = n - found_ceiling;
        self.last_msg = format!(
            "Placed {n} points ({rows}×{cols}) at {height}, {dx:.2} × {dy:.2} m pitch{}{}",
            if self.mount_to_ceiling && missed > 0 {
                format!(" · {missed} found no ceiling and used {:.2} m", self.mount_height)
            } else {
                String::new()
            },
            if profile.is_empty() {
                " — now pick a fitting for them in ▼ Fittings."
            } else {
                " — press Calculate."
            },
        );
        n
    }

    /// Plan-view bounds of the current lighting geometry, for laying out an array.
    pub fn room_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        mesh_bbox(&self.meshes)
    }

    /// Import (Phase B) every drafted dobject on `layer_id` into the room, at
    /// the current default height. Re-importing the same layer refreshes its
    /// handle set and keeps its chosen height.
    pub fn import_layer(&mut self, doc: &Document, layer_id: u32) {
        let handles: Vec<u64> = doc.dobjects.iter()
            .filter(|d| d.style.layer == layer_id)
            .map(|d| d.handle)
            .collect();
        let name = doc.layers.get(layer_id)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("layer {layer_id}"));
        let n = handles.len();
        if let Some(g) = self.room.iter_mut().find(|g| g.layer_id == layer_id) {
            g.handles = handles;
            g.name = name.clone();
        } else {
            self.room.push(RoomLayer { layer_id, name: name.clone(), height: self.room_height, handles });
        }
        self.last_msg =
            format!("Imported {n} object(s) from layer '{name}' — set height, then Calculate.");
    }

    /// Drop one imported room layer (Phase B).
    pub fn remove_room_layer(&mut self, layer_id: u32) {
        self.room.retain(|g| g.layer_id != layer_id);
    }

    /// Every handle across all imported room layers (for plan highlight / count).
    pub fn room_handles(&self) -> Vec<u64> {
        self.room.iter().flat_map(|g| g.handles.iter().copied()).collect()
    }

    /// Run the lux engine on `doc` and store the grid + plane + scene.
    /// The ONE geometry source, shared by the 3D view and the calculation.
    ///
    /// These were two separate expressions that happened to agree — until the view learned about
    /// the Factory model and the calculation did not, at which point the picture would have shown
    /// the real building while the numbers described an extruded footprint. A lighting result that
    /// disagrees with the room on screen is worse than no result, because nothing about it looks
    /// wrong.
    fn scene_meshes(
        &self,
        doc: &Document,
        factory: Option<&crate::factory::FactoryState>,
    ) -> Vec<Mesh> {
        let from_3d = factory.map(meshes_from_factory).unwrap_or_default();
        if !from_3d.is_empty() {
            return from_3d;
        }
        if self.room.is_empty() {
            extrude(doc, self.room_height)
        } else {
            let mut m = Vec::new();
            for g in &self.room {
                m.extend(extrude_handles(doc, &g.handles, g.height));
            }
            m
        }
    }

    /// Luminaires the MODEL carries: every placed fitting that was generated with emitting points.
    ///
    /// DERIVED, never stored. The emitters live on the asset in its own local frame, so the
    /// instance transform puts them where the fixture actually is — move it, copy it, rotate it or
    /// delete it and its light does the same thing for free. A luminaire list written once at build
    /// time strands behind the fixture the first time anybody drags it, and nothing on screen says
    /// so; that failure is silent and produces a plausible wrong answer, which is the worst kind.
    ///
    /// Registers a synthesised photometry per asset as a side effect, which is why this takes
    /// `&mut self`.
    /// Slide the view: move the camera TARGET across the screen plane by a drag of `(dx, dy)`
    /// pixels.
    ///
    /// Orbit and zoom alone cannot get you to a corner of a large plan — the pivot stays put and
    /// the room swings around it. Scaled by distance so the scene keeps up with the cursor at any
    /// zoom, which is what makes a pan feel like dragging the model rather than nudging it.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let (cy, sy) = (self.cam_yaw.cos(), self.cam_yaw.sin());
        let (cp, sp) = (self.cam_pitch.cos(), self.cam_pitch.sin());
        // The camera's own basis: forward is the eye→target direction used by `light3d::mvp`.
        let fwd = glam::Vec3::new(-cp * cy, -cp * sy, -sp);
        let right = fwd.cross(glam::Vec3::Z).normalize_or_zero();
        let up = right.cross(fwd).normalize_or_zero();
        let k = self.cam_dist * 0.0022;
        let t = glam::Vec3::from(self.cam_target) - right * dx * k + up * dy * k;
        self.cam_target = [t.x, t.y, t.z];
    }

    /// Count the model-carried luminaires for the status strip, without building them.
    ///
    /// The MERGED count, so the strip agrees with what Calculate will actually run.
    pub fn refresh_model_fixtures(&mut self, f: &crate::factory::FactoryState) {
        self.model_fixtures = f
            .furniture
            .iter()
            .filter_map(|inst| f.furniture_lib.get(inst.asset))
            .map(|a| merge_emitters(&a.emitters).len())
            .sum();
    }

    fn generated_luminaires(&mut self, f: &crate::factory::FactoryState) -> Vec<Luminaire> {
        let mut out = Vec::new();
        // A range placed lights never reach, so a generated id can never collide with a user's.
        let mut id = 1_000_000_u32;
        for (i, inst) in f.furniture.iter().enumerate() {
            let Some(asset) = f.furniture_lib.get(inst.asset) else { continue };
            if asset.emitters.is_empty() {
                continue;
            }
            let Some(m) = f.furniture_model_matrix(i) else { continue };
            let m = glam::Mat4::from_cols_array(&m);
            let groups = merge_emitters(&asset.emitters);
            // One profile per ASSET: every point on a run carries the same share of its flux, so
            // they share a distribution, while two different fittings do not.
            let profile = format!("{} · {} K", asset.name, asset.cct_k);
            if !self.profiles.contains_key(&profile) {
                let per_lm = groups[0].lumens;
                let per_w = groups[0].watts;
                self.profiles.insert(profile.clone(), lambertian_profile(&profile, per_lm, per_w));
            }
            for e in &groups {
                let p = m.transform_point3(glam::Vec3::from(e.pos));
                out.push(Luminaire {
                    id,
                    profile: profile.clone(),
                    position: Vertex::new(p.x, p.y, p.z),
                    rotation_deg: 0.0,
                    dimming: 1.0,
                });
                id += 1;
            }
        }
        out
    }

    pub fn calculate(&mut self, doc: &Document, factory: Option<&crate::factory::FactoryState>) {
        let meshes = self.scene_meshes(doc, factory);
        // The calculation plane must cover whatever is actually being lit. With a 3D model that is
        // the MODEL's footprint, which need not match the 2D drawing's at all — a plan carries
        // dimensions, notes and title blocks that are not part of the building, and a building can
        // sit anywhere relative to them.
        let bounds = if meshes.is_empty() { None } else { mesh_bbox(&meshes) };
        let Some((min_x, min_y, max_x, max_y)) = bounds.or_else(|| bbox(doc)) else {
            self.grid = None;
            self.plane = None;
            self.last_msg = "No geometry — draw a closed room, or build one in the 3D Factory."
                .to_string();
            return;
        };
        let (w, d) = ((max_x - min_x).max(1e-3), (max_y - min_y).max(1e-3));
        let cols = ((w / self.cell_size).round() as u32).clamp(8, 64);
        let rows = ((d / self.cell_size).round() as u32).clamp(8, 64);
        let plane = CalcPlane {
            origin: Vertex::new(min_x, min_y, self.plane_height),
            width: w,
            depth: d,
            cols,
            rows,
        };
        // Lights the MODEL carries — a curved light is a real fitting, not a glowing texture.
        // Derived here rather than stored, so moving or deleting the fixture takes its light along.
        let generated = match factory {
            Some(f) => self.generated_luminaires(f),
            None => Vec::new(),
        };
        let lums = if self.luminaires.is_empty() && generated.is_empty() && self.auto_center_light {
            vec![Luminaire {
                id: 1,
                // A stand-in light needs a real profile behind it or the "first look" it exists to
                // give is a black room; the built-in is what that means.
                profile: if self.profiles.contains_key(&self.active_profile) {
                    self.active_profile.clone()
                } else {
                    BUILTIN.to_string()
                },
                position: Vertex::new(0.5 * (min_x + max_x), 0.5 * (min_y + max_y), self.room_height),
                rotation_deg: 0.0,
                dimming: 1.0,
            }]
        } else {
            let mut v = self.luminaires.clone();
            v.extend(generated);
            v
        };
        let grid = calc_lux(
            &meshes,
            &lums,
            &self.profiles,
            &self.materials,
            &plane,
            &self.settings,
            self.maintenance,
        );
        // MEAN CYLINDRICAL ILLUMINANCE at eye height — how well the space renders faces and solid
        // objects, which the horizontal grid cannot report at any resolution.
        //
        // On a COARSE sub-grid deliberately: every point costs 24 azimuth evaluations, so measuring
        // it at the work plane's resolution would multiply the whole calculation by twenty-four to
        // refine a single room-average figure that a 12 × 12 sample already settles.
        self.cylindrical_avg = {
            let ev = cad_light::Evaluator::new(
                &meshes,
                &lums,
                &self.profiles,
                &self.materials,
                self.settings,
                self.maintenance,
            );
            const N: u32 = 12;
            let mut sum = 0.0;
            for r in 0..N {
                for c in 0..N {
                    let x = min_x + w * (c as f32 + 0.5) / N as f32;
                    let y = min_y + d * (r as f32 + 0.5) / N as f32;
                    sum += ev.cylindrical(glam::Vec3::new(x, y, self.eye_height));
                }
            }
            Some(sum / (N * N) as f64)
        };
        // ROOM SURFACES — walls and ceiling, which EN 12464-1 sets levels for and which the work
        // plane says nothing about.
        //
        // 1 sample/m² and the same ray settings: this is a room-average figure per surface, and the
        // cost scales with the room's whole surface area rather than the grid, so a fine sample
        // here would dominate the calculation to refine a number quoted to the nearest lux.
        self.surfaces = cad_light::surface_report(
            &meshes,
            &lums,
            &self.profiles,
            &self.materials,
            &self.settings,
            self.maintenance,
            1.0,
        );
        // What the scheme costs to run, over the area actually assessed. The calculation plane's
        // extent, not the true floor area — for an L-shaped room those differ, and the honest thing
        // is to say which one the density is per, which the UI does.
        self.installation =
            Some(installation_summary(&lums, &self.profiles, (w * d) as f64));
        // A point with no fitting emits nothing, and a result computed from half a layout looks
        // exactly like a result computed from all of it. Say so, on the same line as the numbers.
        let waiting = self.unassigned_count();
        self.last_msg = format!(
            "{}×{} grid · avg {:.0} · min {:.0} · max {:.0} lx maintained (MF {:.2}) · U₀ {:.2}{}",
            cols,
            rows,
            grid.avg,
            grid.min,
            grid.max,
            grid.maintenance,
            grid.u0(),
            match waiting {
                0 => String::new(),
                n => format!("  ⚠ {n} point(s) have no fitting and emit nothing — pick one in ▼ Fittings"),
            },
        );
        self.grid = Some(grid);
        self.plane = Some(plane);
        self.meshes = meshes;
        self.show_overlay = true;

        // Fit the orbit camera to the room.
        self.cam_target = [0.5 * (min_x + max_x), 0.5 * (min_y + max_y), 0.5 * self.room_height];
        let diag = (w * w + d * d + self.room_height * self.room_height).sqrt();
        self.cam_dist = (diag * 1.3).max(3.0);
    }

    /// SIMLUX workspace live sync: extrude the current room (imported per-layer
    /// groups, else the whole document) into `meshes` WITHOUT running the lux
    /// calc, so the right-hand 3D view tracks whatever is drawn/imported on the
    /// left 2D plan. Cheap (geometry only). Fits the orbit camera ONCE, the
    /// first frame after the workspace is entered (`simlux_fit_pending`).
    /// Rebuild the lighting geometry. `factory` is the 3D model, when there is one.
    ///
    /// The Factory model WINS whenever it holds anything: it is the real building, with its
    /// openings, slabs and storeys, and the 2D extrusion is a footprint pulled to a single height.
    /// The extrusion stays as the fallback so a plan-only project is unaffected — that is still a
    /// perfectly good way to get a first lux figure before any 3D work exists.
    pub fn rebuild_live_meshes_with(
        &mut self,
        doc: &Document,
        factory: Option<&crate::factory::FactoryState>,
    ) {
        self.meshes = self.scene_meshes(doc, factory);
        // Frame what is actually THERE. Framing from the 2D drawing pointed the camera at the
        // plan's extent — title block, dimensions and all — which with a 3D model loaded is not
        // where the building is, and on a survey plan sited kilometres from the origin is not
        // even close.
        if self.simlux_fit_pending {
            let scene = mesh_bbox(&self.meshes).or_else(|| bbox(doc));
            if let Some((min_x, min_y, max_x, max_y)) = scene {
                let (w, d) = ((max_x - min_x).max(1e-3), (max_y - min_y).max(1e-3));
                let h = mesh_height(&self.meshes).unwrap_or(self.room_height);
                self.cam_target = [0.5 * (min_x + max_x), 0.5 * (min_y + max_y), 0.5 * h];
                let diag = (w * w + d * d + h * h).sqrt();
                self.cam_dist = (diag * 1.3).max(3.0);
                self.simlux_fit_pending = false;
            }
        }
    }

    /// Snapshot the SIMLUX-side state into a serialisable sidecar config,
    /// keyed by STABLE NAMES (layer name, profile name) so it round-trips a
    /// save/reopen. The built-in synthetic downlight is NOT persisted (it is
    /// regenerated in `new`).
    pub fn to_config(&self, doc: &Document) -> crate::simlux_io::SimluxConfig {
        use std::collections::BTreeMap;
        let mut layers_3d = BTreeMap::new();
        for g in &self.room {
            let name = doc
                .layers
                .get(g.layer_id)
                .map(|l| l.name.clone())
                .unwrap_or_else(|| g.name.clone());
            layers_3d.insert(name, g.height);
        }
        let mut ies_library = BTreeMap::new();
        for (k, v) in &self.profiles {
            if k != BUILTIN {
                ies_library.insert(k.clone(), v.clone());
            }
        }
        crate::simlux_io::SimluxConfig {
            layers_3d,
            ies_library,
            active_profile: self.active_profile.clone(),
            lux_block_ies: BTreeMap::new(),
            materials: self.materials.clone(),
            settings: self.settings,
            room_height: self.room_height,
            plane_height: self.plane_height,
            cell_size: self.cell_size,
            // App-layer wall centerline linetypes are filled in by the caller
            // (write_simlux_sidecar) — `light` doesn't own that map.
            wall_centerline: BTreeMap::new(),
            // Likewise the 3D Factory model: `light` doesn't own it, so it stays empty
            // here and the caller fills it from `factory.to_persist()`.
            factory: Default::default(),
            luminaires: self.luminaires.clone(),
            next_luminaire_id: self.next_id,
            maintenance: Some(self.maintenance),
        }
    }

    /// Apply a loaded sidecar config onto the current document — merge the IES
    /// library, restore materials/settings/defaults, and rebuild the room by
    /// resolving persisted layer NAMES back to ids + their current handles.
    pub fn apply_config(&mut self, cfg: crate::simlux_io::SimluxConfig, doc: &Document) {
        for (k, v) in cfg.ies_library {
            self.profiles.insert(k, v);
        }
        // An EMPTY active profile is a real state — "no fitting chosen yet" — so it restores as
        // written. Anything else has to name a fitting that is actually in the library.
        if cfg.active_profile.is_empty() || self.profiles.contains_key(&cfg.active_profile) {
            self.active_profile = cfg.active_profile;
        }
        // The placed layout. A fixture whose fitting did not come back with the library is left
        // unassigned — visible as a hollow marker and counted in the toolbar — rather than kept
        // pointing at a name that resolves to nothing and silently emits no light.
        if !cfg.luminaires.is_empty() {
            self.luminaires = cfg.luminaires;
            for l in self.luminaires.iter_mut() {
                if !self.profiles.contains_key(&l.profile) {
                    l.profile = UNASSIGNED.to_string();
                }
            }
            self.selected.clear();
            self.drag = None;
            let highest = self.luminaires.iter().map(|l| l.id).max().unwrap_or(0);
            self.next_id = cfg.next_luminaire_id.max(highest + 1);
        }
        if !cfg.materials.is_empty() {
            self.materials = cfg.materials;
            // A project saved before furniture was traced carries only floor, wall and ceiling.
            // Furniture triangles would then reference a material that is not there and be traced
            // as a PERFECT ABSORBER — every piece a black hole, which is a worse answer than the
            // empty box it replaced. Add the default rather than leave the gap.
            if !self.materials.iter().any(|m| m.id == MATERIAL_FURNITURE) {
                if let Some(f) =
                    default_materials().into_iter().find(|m| m.id == MATERIAL_FURNITURE)
                {
                    self.materials.push(f);
                }
            }
        }
        self.settings = cfg.settings;
        // A project saved before maintenance existed was quoted at the INITIAL condition. Restore
        // it that way: adopting today's default would silently change every number in a result the
        // user has already read, reported, or issued.
        self.maintenance = cfg.maintenance.unwrap_or(Maintenance::INITIAL);
        if cfg.room_height > 0.0 {
            self.room_height = cfg.room_height;
        }
        if cfg.plane_height > 0.0 {
            self.plane_height = cfg.plane_height;
        }
        if cfg.cell_size > 0.0 {
            self.cell_size = cfg.cell_size;
        }
        self.room.clear();
        for (name, height) in cfg.layers_3d {
            if let Some(lid) = doc.layers.find(&name) {
                let handles: Vec<u64> = doc
                    .dobjects
                    .iter()
                    .filter(|d| d.style.layer == lid)
                    .map(|d| d.handle)
                    .collect();
                self.room.push(RoomLayer { layer_id: lid, name, height, handles });
            }
        }
    }

    /// Draw the panel body. Returns actions the app must run (they need `&Document`).
    /// The SIMLUX toolbar — the same shape as the 3D Factory's, and for the same reason.
    ///
    /// The lighting controls were a tall stack of numbered sections in a side panel, so getting a
    /// fixture into a room meant reading the whole column to find out which step you were on. The
    /// Factory solved this already: grouped `▼` menus on one wrapped row, with the state that
    /// matters on a line underneath. Matching it means one thing to learn, not two.
    ///
    /// Grouped by the QUESTION being answered, not by the code behind it, and ordered by the
    /// order the questions come up:
    ///   Fittings — which real products are available, imported from the manufacturer's files
    ///   Luminaires — where they go, and which fitting is in each spot
    ///   Calculation — how the answer is worked out
    ///   Surfaces — what the room is made of
    ///   Display — how the result is drawn
    pub fn toolbar_ui(&mut self, ui: &mut egui::Ui) -> LightAction {
        let mut action = LightAction::default();
        ui.horizontal_wrapped(|ui| {
            // ---- ① the LIBRARY of imported fittings -------------------------------------
            //
            // First on the bar because it is first in the workflow, and because a photometric
            // file is a product brought into the project exactly as a piece of furniture is.
            let waiting = self.unassigned_count();
            let fittings = self.profiles.len();
            ui.menu_button("▼ Fittings", |ui| {
                if ui
                    .button("📂  Import light file…")
                    .on_hover_text("IES (.ies) or EULUMDAT (.ldt) from the manufacturer — the same picker furniture uses")
                    .clicked()
                {
                    action.import_photometry = true;
                    ui.close_menu();
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(if waiting > 0 {
                        format!("click a fitting → the {waiting} point(s) with none")
                    } else if !self.selected.is_empty() {
                        format!("click a fitting → the {} selected", self.selected.len())
                    } else {
                        "click a fitting → use it for new points".to_string()
                    })
                    .small()
                    .weak(),
                );
                // Sorted, because a HashMap would reorder the list on every repaint and the entry
                // under the cursor would not be the one that gets clicked.
                let mut names: Vec<String> = self.profiles.keys().cloned().collect();
                names.sort();
                let mut assign: Option<String> = None;
                let mut drop: Option<String> = None;
                for n in &names {
                    let active = *n == self.active_profile;
                    let used = self.luminaires.iter().filter(|l| l.profile == *n).count();
                    let detail = self.profiles.get(n).map(|p| {
                        if p.lumens > 0.0 {
                            format!("{:.0} lm · {:.0} W · peak {:.0} cd", p.lumens, p.watts, p.peak_candela())
                        } else {
                            format!("peak {:.0} cd", p.peak_candela())
                        }
                    });
                    ui.horizontal(|ui| {
                        let label = if used > 0 { format!("{n}   ({used})") } else { n.clone() };
                        if ui
                            .selectable_label(active, label)
                            .on_hover_text(detail.unwrap_or_default())
                            .clicked()
                        {
                            assign = Some(n.clone());
                        }
                        if *n != BUILTIN && ui.small_button("✕").on_hover_text("Remove from the library").clicked() {
                            drop = Some(n.clone());
                        }
                    });
                }
                if let Some(n) = assign {
                    self.assign_profile(&n);
                    ui.close_menu();
                }
                if let Some(n) = drop {
                    self.remove_profile(&n);
                }
                if names.len() <= 1 {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Only the built-in is loaded.\nImport a manufacturer file to light\nthe real product.")
                            .small()
                            .weak(),
                    );
                }
            })
            .response
            .on_hover_text(format!("{fittings} fitting(s) in the library"));

            // ---- ② where the lights go, ③ what goes in each spot -----------------------
            ui.menu_button("▼ Luminaires", |ui| {
                ui.label(egui::RichText::new("place").small().weak());
                let placing = self.place_mode;
                if ui
                    .selectable_label(placing, if placing { "◉ Placing — click the plan (Esc to stop)" } else { "＋ Place points on the plan" })
                    .on_hover_text("Click the 2D plan to mark each spot. Points stay editable: drag to move, click to select, Del to delete.")
                    .clicked()
                {
                    self.place_mode = !placing;
                    if self.place_mode {
                        self.last_msg = "Click the plan to mark each light position · drag a marker to move it · Esc to stop.".into();
                    }
                    ui.close_menu();
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("selection — {} of {}", self.selected.len(), self.luminaires.len()))
                        .small()
                        .weak(),
                );
                ui.horizontal(|ui| {
                    if ui.button("All").clicked() {
                        self.select_all();
                    }
                    if ui.button("None").clicked() {
                        self.clear_selection();
                    }
                    let can_del = !self.selected.is_empty();
                    if ui
                        .add_enabled(can_del, egui::Button::new("🗑 Delete"))
                        .on_hover_text("Del also does this while the plan has focus")
                        .clicked()
                    {
                        self.delete_selected();
                    }
                });
                ui.separator();
                ui.label(egui::RichText::new("array — the usual way to light a room").small().weak());
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.array_rows).range(1..=40).prefix("rows "));
                    ui.add(egui::DragValue::new(&mut self.array_cols).range(1..=40).prefix("cols "));
                });
                if ui
                    .button("⊞  Lay out grid over the room")
                    .on_hover_text("Fixtures at the centre of each cell, so the gap to the wall is half the gap between fixtures")
                    .clicked()
                {
                    if let Some(b) = self.room_bounds() {
                        let (rows, cols) = (self.array_rows, self.array_cols);
                        self.add_luminaire_grid(b, rows, cols);
                    } else {
                        self.last_msg = "No room yet — build one in the 3D Factory, or draw a closed outline.".into();
                    }
                    ui.close_menu();
                }
                ui.separator();
                ui.label(egui::RichText::new("mounting").small().weak());
                ui.checkbox(&mut self.mount_to_ceiling, "follow the ceiling")
                    .on_hover_text("Each fixture finds the ceiling above its own position — soffits, steps and slopes all mount correctly. Off: everything at one height.");
                ui.horizontal(|ui| {
                    if self.mount_to_ceiling {
                        ui.label("drop below it");
                        ui.add(egui::DragValue::new(&mut self.ceiling_drop).speed(0.02).suffix(" m").range(0.0..=5.0))
                            .on_hover_text("0 = surface-mounted · 0.3 = a short pendant");
                    } else {
                        ui.label("mount at");
                        ui.add(egui::DragValue::new(&mut self.mount_height).speed(0.05).suffix(" m").range(0.1..=30.0));
                    }
                });
                ui.checkbox(&mut self.auto_center_light, "auto-place one at the centre if none")
                    .on_hover_text("A convenience for a first look; turn it off once you place fixtures yourself");
                ui.separator();
                if ui.button("🗑  Remove all fixtures").clicked() {
                    let n = self.luminaires.len();
                    self.luminaires.clear();
                    self.selected.clear();
                    self.drag = None;
                    self.last_msg = format!("Removed {n} fixture(s).");
                    ui.close_menu();
                }
            });

            ui.menu_button("▼ Calculation", |ui| {
                egui::Grid::new("simlux_calc_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    ui.label("work plane").on_hover_text("Height above the floor the lux is measured at — 0.8 m is the usual desk height");
                    ui.add(egui::DragValue::new(&mut self.plane_height).speed(0.05).suffix(" m").range(0.0..=10.0));
                    ui.end_row();
                    ui.label("grid cell").on_hover_text("Target spacing of the measurement grid; finer is slower");
                    ui.add(egui::DragValue::new(&mut self.cell_size).speed(0.05).suffix(" m").range(0.05..=5.0));
                    ui.end_row();
                    ui.label("eye height").on_hover_text("Height the cylindrical illuminance Ez is measured at — 1.2 m seated, 1.6 m standing");
                    ui.add(egui::DragValue::new(&mut self.eye_height).speed(0.05).suffix(" m").range(0.3..=2.5));
                    ui.end_row();
                    ui.label("bounces").on_hover_text("Indirect light: 0 is direct only, which under-reads a bright room badly");
                    ui.add(egui::DragValue::new(&mut self.settings.max_bounces).range(0..=8));
                    ui.end_row();
                    ui.label("rays").on_hover_text("Samples per point for the indirect term — more is smoother and slower");
                    ui.add(egui::DragValue::new(&mut self.settings.rays_per_point).range(1..=4096));
                    ui.end_row();
                });
                ui.separator();
                // MAINTENANCE. Every illuminance a designer quotes is the maintained one — what
                // the scheme still delivers at the end of the cleaning cycle, not on day one.
                ui.label(
                    egui::RichText::new(format!("maintenance factor — MF {:.2}", self.maintenance.factor()))
                        .small()
                        .strong(),
                );
                egui::Grid::new("simlux_mf_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    let mut row = |ui: &mut egui::Ui, label: &str, tip: &str, v: &mut f64| {
                        ui.label(label).on_hover_text(tip);
                        ui.add(egui::DragValue::new(v).speed(0.005).range(0.1..=1.0).fixed_decimals(2));
                        ui.end_row();
                    };
                    row(ui, "LLMF", "Lamp lumen maintenance — output left after the operating interval, from the luminaire's data sheet", &mut self.maintenance.llmf);
                    row(ui, "LSF", "Lamp survival — the fraction still lit. 1.00 for LED with spot replacement", &mut self.maintenance.lsf);
                    row(ui, "LMF", "Luminaire maintenance — dirt on the optic, set by the cleaning interval and room cleanliness", &mut self.maintenance.lmf);
                    row(ui, "RSMF", "Room surface maintenance — the room's own surfaces darkening", &mut self.maintenance.rsmf);
                });
                ui.horizontal(|ui| {
                    if ui.button("Clean office (0.80)").clicked() {
                        self.maintenance = Maintenance::default();
                    }
                    if ui
                        .button("Initial (1.00)")
                        .on_hover_text("Day-one condition. Useful for comparison — NOT what a scheme is submitted at.")
                        .clicked()
                    {
                        self.maintenance = Maintenance::INITIAL;
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Defaults are a clean interior on a 3-year cycle.\nSet all four from the data sheet + CIE 97 for a submission.",
                    )
                    .small()
                    .weak(),
                );
            });

            // EXPORT. A result that lives only in a panel cannot be sent to a client, checked by a
            // colleague, or filed against a project. Enabled only once there IS a result — an empty
            // report is worse than none, because it looks like a finished one.
            if ui
                .add_enabled(self.grid.is_some(), egui::Button::new("📄 Report"))
                .on_hover_text(
                    "Write this calculation out as a standalone HTML report — conditions, results, \
                     the full grid, room surfaces and connected load.",
                )
                .on_disabled_hover_text("Press Calculate first")
                .clicked()
            {
                action.export_report = true;
            }

            ui.menu_button("▼ Surfaces", |ui| {
                ui.label(egui::RichText::new("reflectance — how much light a surface returns").small().weak());
                egui::Grid::new("simlux_mat_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    for m in &mut self.materials {
                        ui.label(&m.name);
                        ui.add(egui::DragValue::new(&mut m.reflectance).speed(0.01).range(0.0..=1.0));
                        ui.end_row();
                    }
                });
                ui.label(
                    egui::RichText::new("Surfaces are sorted by which way they face:\nup = floor, down = ceiling, upright = wall.")
                        .small().weak(),
                );
            });

            ui.menu_button("▼ Display", |ui| {
                ui.checkbox(&mut self.floor_heatmap, "false-colour on the floor")
                    .on_hover_text("Paint the calculated illuminance onto the floor of the 3D view.");
                ui.checkbox(&mut self.hide_ceilings, "hide ceilings")
                    .on_hover_text(
                        "Drop the ceiling so the room can be seen into from above. The result is \
                         painted on the FLOOR, and a closed box hides the surface this view exists \
                         to show.",
                    );
                ui.separator();
                // ---- THE SCALE ITSELF ----------------------------------------------------
                // A false-colour picture is unreadable without knowing what it is scaled to, and
                // two rooms cannot be compared at all unless they are on the SAME scale — which is
                // what the manual setting is for. Auto is per-room and re-scales every calculation.
                ui.label(egui::RichText::new("Scale").small().weak());
                let mut auto = self.scale_max.is_none();
                if ui
                    .checkbox(&mut auto, "auto (to this room's maximum)")
                    .on_hover_text("Off: pin the top of the scale, so two rooms can be compared.")
                    .changed()
                {
                    self.scale_max = if auto {
                        None
                    } else {
                        Some(self.grid.as_ref().map(|g| g.max).unwrap_or(500.0).max(1.0))
                    };
                }
                if let Some(m) = &mut self.scale_max {
                    ui.add(
                        egui::DragValue::new(m)
                            .speed(10.0)
                            .prefix("top of scale  ")
                            .suffix(" lx")
                            .range(1.0..=100_000.0),
                    );
                }
                ui.separator();
                ui.label(egui::RichText::new("Colours").small().weak());
                let cur = self.ramp;
                egui::ComboBox::from_id_salt("lux_ramp")
                    .width(190.0)
                    .selected_text(cur.label())
                    .show_ui(ui, |ui| {
                        for r in LuxRamp::ALL {
                            ui.selectable_value(&mut self.ramp, r, r.label());
                        }
                    });
                // A live sample of the chosen ramp, so the choice can be made by looking rather
                // than by reading four names.
                legend_bar_with(ui, self.scale_ceiling(), self.ramp);
            });

            ui.separator();
            if ui
                .button("⚡ Calculate")
                .on_hover_text("Trace the room and compute the lux grid")
                .clicked()
            {
                action.calculate = true;
            }
        });

        // The state line, exactly as the Factory reports features/tris/selection: what is loaded,
        // and what the last answer was.
        ui.horizontal_wrapped(|ui| {
            let small = |t: String| egui::RichText::new(t).small().weak();
            ui.label(
                egui::RichText::new(format!("{} fixture(s)", self.luminaires.len()))
                    .small()
                    .strong(),
            );
            // …AND THE LIGHTS THE MODEL CARRIES. A curved light is a real fitting now, but it is
            // DERIVED from the placed object at calculation time rather than living in
            // `luminaires` — so a room holding two of them and no hand-placed points read
            // "0 fixture(s)", which says the scheme is empty when it is not.
            if self.model_fixtures > 0 {
                ui.label(
                    egui::RichText::new(format!("+ {} from the model", self.model_fixtures))
                        .small()
                        .strong()
                        .color(egui::Color32::from_rgb(120, 190, 255)),
                )
                .on_hover_text(
                    "Luminaires built into the 3D model — curved lights. They carry their own \
                     photometry and are included in Calculate; move or delete the fitting and its \
                     light goes with it.",
                );
            }
            if !self.selected.is_empty() {
                ui.label(
                    egui::RichText::new(format!("· {} selected", self.selected.len()))
                        .small()
                        .color(egui::Color32::from_rgb(120, 190, 255)),
                );
            }
            // The one number that explains a dark result, kept on screen rather than only in the
            // message that the next status line overwrites.
            let waiting = self.unassigned_count();
            if waiting > 0 {
                ui.label(
                    egui::RichText::new(format!("· {waiting} need a fitting"))
                        .small()
                        .color(egui::Color32::from_rgb(230, 170, 90)),
                )
                .on_hover_text("Points with no fitting emit nothing. ▼ Fittings → click one.");
            }
            ui.label(small(format!(
                "· {}",
                if self.active_profile.is_empty() { "no fitting chosen" } else { &self.active_profile }
            )));
            if self.place_mode {
                ui.label(
                    egui::RichText::new("· PLACING — click the plan")
                        .small()
                        .strong()
                        .color(egui::Color32::from_rgb(255, 214, 90)),
                );
            }
            if let Some(g) = self.grid.as_ref() {
                ui.label(small(format!("· avg {:.0} lx", g.avg)));
                ui.label(small(format!("· min {:.0}", g.min)));
                ui.label(small(format!("· max {:.0}", g.max)));
                // Which condition the figures are for. A lux number without this is ambiguous, and
                // the ambiguity always flatters the design.
                ui.label(
                    egui::RichText::new(if g.maintenance < 0.999 {
                        format!("· maintained MF {:.2}", g.maintenance)
                    } else {
                        "· INITIAL (MF 1.00)".to_string()
                    })
                    .small()
                    .color(if g.maintenance < 0.999 {
                        egui::Color32::from_rgb(150, 200, 150)
                    } else {
                        egui::Color32::from_rgb(230, 170, 90)
                    }),
                )
                .on_hover_text(
                    "EN 12464 limits are on MAINTAINED illuminance. ▼ Calculation → maintenance factor.",
                );
                // UNIFORMITY. EN 12464 specifies U0 = Emin/Eavg, and a scheme is judged on it as
                // much as on the average — 500 lx at U0 = 0.2 is a room with dark patches, and
                // nothing in min/avg/max on its own says that.
                if g.avg > 0.0 {
                    let u0 = g.min / g.avg;
                    ui.label(
                        egui::RichText::new(format!("· U₀ {u0:.2}"))
                            .small()
                            .color(if u0 >= 0.6 {
                                egui::Color32::from_rgb(120, 200, 120)
                            } else if u0 >= 0.4 {
                                egui::Color32::from_rgb(220, 190, 100)
                            } else {
                                egui::Color32::from_rgb(220, 130, 120)
                            }),
                    )
                    .on_hover_text("Uniformity Emin/Eavg. EN 12464 asks 0.60 for most work areas, 0.40 for circulation.");
                }
            } else {
                ui.label(small("· not calculated".into()));
            }
        });
        action
    }

    pub fn panel_ui(&mut self, ui: &mut egui::Ui, layers: &[(u32, String)]) -> LightAction {
        let mut action = LightAction::default();
        ui.set_min_width(260.0);

        // ---- ① Room — mark layers "use for 3D"; each extrudes to its height ----
        ui.label(egui::RichText::new("① Room  ·  use layers for 3D").strong());
        ui.label(
            egui::RichText::new("Tick the layers that form the room.")
                .small()
                .weak(),
        );
        if ui
            .button("⬚  Move selection → SIMLUX layer")
            .on_hover_text("Put the selected geometry on a dedicated SIMLUX layer and use it for 3D")
            .clicked()
        {
            action.shift_to_simlux = true;
        }
        egui::Grid::new("simlux_layer_use3d")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for (id, name) in layers {
                    let group = self.room.iter().find(|g| g.layer_id == *id);
                    let mut on = group.is_some();
                    let n = group.map(|g| g.handles.len()).unwrap_or(0);
                    if ui
                        .checkbox(&mut on, name.as_str())
                        .on_hover_text("Use this layer's geometry in the 3D model / lux calc")
                        .changed()
                    {
                        if on {
                            action.import_layer = Some(*id);
                        } else {
                            action.remove_layer = Some(*id);
                        }
                    }
                    ui.label(
                        egui::RichText::new(if on { format!("{n} obj") } else { String::new() })
                            .small()
                            .weak(),
                    );
                    ui.end_row();
                }
            });
        if self.room.is_empty() {
            ui.label(
                egui::RichText::new("No layers imported → Calculate extrudes the whole drawing.")
                    .small()
                    .weak(),
            );
        } else {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("② Extrude  ·  per-layer height (m)").strong());
            egui::Grid::new("simlux_room_groups")
                .num_columns(4)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for g in &mut self.room {
                        ui.label(egui::RichText::new(&g.name).strong());
                        ui.label(
                            egui::RichText::new(format!("{} obj", g.handles.len()))
                                .small()
                                .weak(),
                        );
                        ui.add(
                            egui::DragValue::new(&mut g.height)
                                .speed(0.05)
                                .suffix(" m")
                                .range(0.1..=20.0),
                        );
                        if ui.button("✕").on_hover_text("Remove from room").clicked() {
                            action.remove_layer = Some(g.layer_id);
                        }
                        ui.end_row();
                    }
                });
        }
        ui.separator();

        // ---- Luminaire / IES --------------------------------------------
        ui.label(egui::RichText::new("Luminaire").strong());
        let mut keys: Vec<String> = self.profiles.keys().cloned().collect();
        keys.sort();
        egui::ComboBox::from_label("Photometry")
            .selected_text(self.active_profile.clone())
            .show_ui(ui, |ui| {
                for k in &keys {
                    ui.selectable_value(&mut self.active_profile, k.clone(), k.as_str());
                }
            });
        ui.horizontal(|ui| {
            if ui
                .button("📂  Import light file…")
                .on_hover_text("IES (.ies) or EULUMDAT (.ldt)")
                .clicked()
            {
                action.import_photometry = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("path:");
            ui.add(
                egui::TextEdit::singleline(&mut self.ies_path)
                    .desired_width(150.0)
                    .hint_text(r"C:\path\to\file.ies"),
            );
            if ui.button("Load").clicked() {
                self.import_photometry();
            }
        });
        ui.checkbox(&mut self.auto_center_light, "Auto-place one at room centre if none placed");

        ui.separator();

        // ---- Fixtures (P4 placement) ------------------------------------
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Fixtures").strong());
            ui.label(egui::RichText::new(format!("({})", self.luminaires.len())).weak());
        });
        let place_label = if self.place_mode { "◉ Placing… click the plan" } else { "＋ Place on plan" };
        if ui.selectable_label(self.place_mode, place_label)
            .on_hover_text("Toggle, then click points on the 2D plan to drop fixtures. Drag a marker to move it. Esc / untoggle to stop.")
            .clicked()
        {
            self.place_mode = !self.place_mode;
        }
        ui.add(egui::Slider::new(&mut self.mount_height, 0.0..=8.0).text("Mount height (m)"));
        if !self.luminaires.is_empty() {
            // The list is a SELECTION view, not a read-out: clicking a row selects that fixture on
            // the plan, which is how you find #17 in a grid of forty identical markers.
            let mut remove: Option<u32> = None;
            let mut click: Option<u32> = None;
            let known: Vec<String> = self.profiles.keys().cloned().collect();
            let selected = self.selected.clone();
            egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                for l in self.luminaires.iter_mut() {
                    let sel = selected.contains(&l.id);
                    let fitted = known.contains(&l.profile);
                    ui.horizontal(|ui| {
                        let label = format!(
                            "#{}  ({:.1}, {:.1}, {:.1})  {}",
                            l.id,
                            l.position.x,
                            l.position.y,
                            l.position.z,
                            if fitted { l.profile.as_str() } else { "— no fitting —" },
                        );
                        let text = if fitted {
                            egui::RichText::new(label)
                        } else {
                            egui::RichText::new(label).color(egui::Color32::from_rgb(230, 170, 90))
                        };
                        if ui.selectable_label(sel, text).clicked() {
                            click = Some(l.id);
                        }
                        if ui.small_button("✕").clicked() {
                            remove = Some(l.id);
                        }
                        ui.add(egui::Slider::new(&mut l.dimming, 0.0..=1.0).text("dim"));
                    });
                }
            });
            if let Some(id) = click {
                let additive = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                self.select(id, additive);
            }
            if let Some(id) = remove {
                self.luminaires.retain(|l| l.id != id);
                self.selected.retain(|&s| s != id);
            }
            if ui.button("Clear all fixtures").clicked() {
                self.luminaires.clear();
                self.selected.clear();
                self.drag = None;
            }
        }

        ui.separator();

        // ---- Room -------------------------------------------------------
        ui.label(egui::RichText::new("Room").strong());
        ui.add(egui::Slider::new(&mut self.room_height, 2.0..=8.0).text("Height (m)"));
        ui.add(egui::Slider::new(&mut self.plane_height, 0.0..=2.0).text("Work plane (m)"));
        ui.add(egui::Slider::new(&mut self.cell_size, 0.1..=1.0).text("Grid cell (m)"));

        ui.separator();

        // ---- Materials --------------------------------------------------
        ui.label(egui::RichText::new("Reflectances").strong());
        for m in &mut self.materials {
            let name = m.name.clone();
            ui.add(egui::Slider::new(&mut m.reflectance, 0.0..=1.0).text(name));
        }

        ui.separator();

        // ---- Quality ----------------------------------------------------
        ui.collapsing("Quality", |ui| {
            ui.add(egui::Slider::new(&mut self.settings.max_bounces, 0..=3).text("Indirect bounces"));
            let mut rays = self.settings.rays_per_point as i32;
            if ui.add(egui::Slider::new(&mut rays, 8..=256).text("Rays / point")).changed() {
                self.settings.rays_per_point = rays.max(1) as u32;
            }
            ui.checkbox(&mut self.settings.shadows, "Cast shadows");
        });

        ui.separator();

        // ---- Calculate --------------------------------------------------
        if ui
            .add(egui::Button::new(egui::RichText::new("  Calculate  ").strong()))
            .clicked()
        {
            action.calculate = true;
        }
        ui.checkbox(&mut self.show_overlay, "Show lux overlay on 2D plan");
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.view3d_open, "3D view");
            ui.checkbox(&mut self.floor_heatmap, "Heatmap floor");
        });

        // ---- Colour scale -----------------------------------------------
        ui.horizontal(|ui| {
            let mut auto = self.scale_max.is_none();
            if ui.checkbox(&mut auto, "Auto scale").changed() {
                self.scale_max = if auto {
                    None
                } else {
                    Some(self.grid.as_ref().map(|g| g.max).unwrap_or(500.0).max(1.0))
                };
            }
            if let Some(m) = &mut self.scale_max {
                ui.add(
                    egui::DragValue::new(m)
                        .speed(10.0)
                        .suffix(" lx")
                        .range(1.0..=100_000.0),
                );
            }
        });

        // ---- Results ----------------------------------------------------
        if let Some(g) = &self.grid {
            ui.separator();
            ui.label(
                egui::RichText::new(if g.maintenance < 0.999 {
                    format!("Results — MAINTAINED (MF {:.2})", g.maintenance)
                } else {
                    "Results — INITIAL (no maintenance allowance)".to_string()
                })
                .strong(),
            );
            egui::Grid::new("simlux_results").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
                let mut row = |ui: &mut egui::Ui, k: &str, v: String| {
                    ui.label(egui::RichText::new(k).small().weak());
                    ui.label(v);
                    ui.end_row();
                };
                row(ui, "Average  Eavg", format!("{:.0} lx", g.avg));
                row(ui, "Minimum  Emin", format!("{:.0} lx", g.min));
                row(ui, "Maximum  Emax", format!("{:.0} lx", g.max));
                row(ui, "Median", format!("{:.0} lx", g.median()));
                // Percentiles say what the average cannot: 500 lx average is a different room
                // when a tenth of it sits at 450 than when it sits at 150.
                row(ui, "10th / 90th pct", format!("{:.0} / {:.0} lx", g.percentile(10.0), g.percentile(90.0)));
                row(ui, "Uniformity  U₀ = Emin/Eavg", format!("{:.2}", g.u0()));
                row(ui, "Diversity  U₁ = Emin/Emax", format!("{:.2}", g.u1()));
                // WHICH GRID THE UNIFORMITY IS ON.
                //
                // U₀ is not a property of a room — it is a property of a room AND the grid it was
                // sampled on, and a coarse grid always reports it too HIGH. Comparing against
                // DIALux on three fully specified rooms showed the averages agreeing to 0.5 % while
                // U₀ differed by a third, entirely from where the minimum was taken; and their
                // figure could not be reproduced because the grid behind it is stated nowhere in
                // their report. A uniformity quoted without its grid is not reproducible, so this
                // says it — and flags a grid coarser than EN 12464-1 asks for, which is exactly the
                // case where U₀ flatters the design.
                if let Some(p) = self.plane.as_ref() {
                    let (wc, wr) = cad_light::en12464_cells(p.width, p.depth);
                    let note = if p.cols < wc || p.rows < wr {
                        format!("{}  ⚠ EN 12464-1 asks {wc} × {wr}", p.grid_note())
                    } else {
                        p.grid_note()
                    };
                    row(ui, "…measured on", note);
                }
                if let Some(f) = g.direct_fraction() {
                    row(ui, "Direct / indirect", format!("{:.0}% / {:.0}%", f * 100.0, (1.0 - f) * 100.0));
                }
                if let Some(ez) = self.cylindrical_avg {
                    row(
                        ui,
                        &format!("Cylindrical  Ez @ {:.1} m", self.eye_height),
                        format!("{ez:.0} lx"),
                    );
                }
                // ROOM SURFACES. EN 12464-1 does not stop at the work plane — it sets maintained
                // levels for walls and ceilings too (an office wants roughly 50 lx on walls and
                // 30 lx on the ceiling, each at U₀ ≥ 0.10), and a scheme that passes on the desk
                // can still fail on those. Luminance is the quantity the appearance clauses are
                // written in, and for a diffuse surface it is ρE/π — so a bright ceiling and a
                // dark floor can receive the same light and look nothing alike.
                if !self.surfaces.is_empty() {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("room surfaces").small().weak());
                    for s in &self.surfaces {
                        row(
                            ui,
                            &format!("{}  ({:.0} m²)", s.name, s.area_m2),
                            format!(
                                "{:.0} lx   {:.0} cd/m²   U₀ {:.2}",
                                s.e_avg, s.l_avg, s.u0
                            ),
                        );
                    }
                }
            });
            // Ez is the one number that says whether the space renders faces. A room can hold its
            // average on the desks and still read as flat, and nothing else on this panel shows it.
            if let Some(ez) = self.cylindrical_avg {
                let (verdict, col) = if ez >= 150.0 {
                    ("good modelling — faces read well", egui::Color32::from_rgb(120, 200, 120))
                } else if ez >= 50.0 {
                    ("meets the usual 50 lx minimum", egui::Color32::from_rgb(220, 190, 100))
                } else {
                    ("below 50 lx — the space will read flat", egui::Color32::from_rgb(220, 130, 120))
                };
                ui.label(egui::RichText::new(format!("Ez {ez:.0} lx · {verdict}")).small().color(col));
            }
            // EN 12464-1 judges a workplace on U₀, and a scheme can meet its average and still
            // fail here — so say which it is rather than leaving the reader to compare.
            let u0 = g.u0();
            let (verdict, col) = if u0 >= 0.60 {
                ("meets 0.60 (work areas)", egui::Color32::from_rgb(120, 200, 120))
            } else if u0 >= 0.40 {
                ("meets 0.40 (circulation) — below 0.60 for work areas", egui::Color32::from_rgb(220, 190, 100))
            } else {
                ("below 0.40 — fails EN 12464 uniformity", egui::Color32::from_rgb(220, 130, 120))
            };
            ui.label(egui::RichText::new(format!("U₀ {u0:.2} · {verdict}")).small().color(col));

            if let Some(i) = &self.installation {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Installation").strong());
                egui::Grid::new("simlux_energy").num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
                    let mut row = |ui: &mut egui::Ui, k: &str, v: String| {
                        ui.label(egui::RichText::new(k).small().weak());
                        ui.label(v);
                        ui.end_row();
                    };
                    row(ui, "Fixtures", format!("{}", i.count));
                    row(ui, "Connected load", format!("{:.0} W", i.total_watts));
                    row(ui, "Power density", format!("{:.2} W/m²", i.power_density));
                    row(ui, "Installed flux", format!("{:.0} lm", i.total_lumens));
                    if i.efficacy > 0.0 {
                        row(ui, "Efficacy", format!("{:.0} lm/W", i.efficacy));
                    }
                    row(ui, "Assessed area", format!("{:.1} m²", i.area_m2));
                });
                // A density computed from half the fixtures looks exactly like one computed from
                // all of them, so an incomplete file has to announce itself.
                if i.missing_watts > 0 || i.missing_lumens > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "⚠ {} fitting(s) declare no wattage, {} no flux — the figures above exclude them.",
                            i.missing_watts, i.missing_lumens
                        ))
                        .small()
                        .color(egui::Color32::from_rgb(230, 170, 90)),
                    );
                }
            }
            legend_bar(ui, self.scale_ceiling());
        }

        ui.add_space(4.0);
        ui.label(egui::RichText::new(&self.last_msg).small().italics());
        action
    }
}

/// The false-colour palettes the lux scale can be read through.
///
/// A false-colour scale is a READING INSTRUMENT, not decoration, and which palette it uses changes
/// what a person can see in it. The classic blue→red ramp is what lighting reports have always
/// used and is the one to hand a client. It is also not perceptually uniform, and is close to
/// unusable for the ~8 % of men with red-green colour blindness — which is why Viridis is here.
/// Greyscale prints and photocopies without the reader having to guess which grey was which colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum LuxRamp {
    /// Blue → green → yellow → red. The lighting-industry convention.
    #[default]
    Classic,
    /// Perceptually uniform and colour-blind safe: equal steps in lux look like equal steps.
    Viridis,
    /// Black → red → orange → white. High contrast at the top of the scale.
    Fire,
    /// Greyscale, for print.
    Grey,
}

impl LuxRamp {
    pub const ALL: [LuxRamp; 4] = [LuxRamp::Classic, LuxRamp::Viridis, LuxRamp::Fire, LuxRamp::Grey];

    pub fn label(self) -> &'static str {
        match self {
            LuxRamp::Classic => "Classic (blue→red)",
            LuxRamp::Viridis => "Viridis (colour-blind safe)",
            LuxRamp::Fire => "Fire",
            LuxRamp::Grey => "Greyscale (for print)",
        }
    }

    /// Its stops, low→high. The first is at 0.0 and the last at 1.0.
    fn stops(self) -> &'static [(f32, [u8; 3])] {
        match self {
            LuxRamp::Classic => &[
                (0.00, [20, 24, 82]),   // deep blue
                (0.25, [34, 116, 204]), // blue
                (0.50, [40, 190, 120]), // green
                (0.75, [240, 214, 72]), // yellow
                (1.00, [226, 72, 46]),  // red
            ],
            LuxRamp::Viridis => &[
                (0.00, [68, 1, 84]),
                (0.25, [59, 82, 139]),
                (0.50, [33, 145, 140]),
                (0.75, [94, 201, 98]),
                (1.00, [253, 231, 37]),
            ],
            LuxRamp::Fire => &[
                (0.00, [0, 0, 0]),
                (0.33, [153, 26, 12]),
                (0.66, [237, 139, 22]),
                (1.00, [255, 255, 224]),
            ],
            LuxRamp::Grey => &[(0.00, [12, 12, 12]), (1.00, [245, 245, 245])],
        }
    }

    /// The colour at `t`, clamped to 0..1.
    pub fn color(self, t: f32) -> egui::Color32 {
        let stops = self.stops();
        let t = t.clamp(0.0, 1.0);
        let (mut lo, mut hi) = (stops[0], stops[stops.len() - 1]);
        for w in stops.windows(2) {
            if t >= w[0].0 && t <= w[1].0 {
                lo = w[0];
                hi = w[1];
                break;
            }
        }
        let span = (hi.0 - lo.0).max(1e-6);
        let f = (t - lo.0) / span;
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * f).round() as u8;
        egui::Color32::from_rgb(lerp(lo.1[0], hi.1[0]), lerp(lo.1[1], hi.1[1]), lerp(lo.1[2], hi.1[2]))
    }

    /// The same, as float RGB (0..1) for the 3D floor heatmap.
    ///
    /// Handed back as a plain `fn` POINTER because the vertex builder takes one — it cannot carry
    /// a closure that borrows `self`.
    pub fn rgb_fn(self) -> fn(f32) -> (f32, f32, f32) {
        fn conv(r: LuxRamp, t: f32) -> (f32, f32, f32) {
            let c = r.color(t);
            (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0)
        }
        match self {
            LuxRamp::Classic => |t| conv(LuxRamp::Classic, t),
            LuxRamp::Viridis => |t| conv(LuxRamp::Viridis, t),
            LuxRamp::Fire => |t| conv(LuxRamp::Fire, t),
            LuxRamp::Grey => |t| conv(LuxRamp::Grey, t),
        }
    }
}

/// Five-stop false-colour ramp (low→high). `t` is clamped to 0..1. The industry-standard palette,
/// kept as a free function for callers that have no `LightState` to ask.
pub fn lux_color(t: f32) -> egui::Color32 {
    LuxRamp::Classic.color(t)
}

/// The same false-colour ramp as [`lux_color`], as float RGB (0..1) for the
/// 3D floor heatmap. `fn(f32) -> (f32, f32, f32)` so it can be passed as a
/// plain function pointer into the 3D vertex builder.
pub fn lux_rgb(t: f32) -> (f32, f32, f32) {
    let c = lux_color(t);
    (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0)
}

/// A horizontal gradient legend from 0 to `max` lux, in the standard palette.
pub fn legend_bar(ui: &mut egui::Ui, max: f64) {
    legend_bar_with(ui, max, LuxRamp::Classic)
}

/// A horizontal gradient legend from 0 to `max` lux, in `ramp`.
pub fn legend_bar_with(ui: &mut egui::Ui, max: f64, ramp: LuxRamp) {
    let (resp, painter) = ui.allocate_painter(egui::vec2(240.0, 16.0), egui::Sense::hover());
    let rect = resp.rect;
    let n = 64;
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32;
        let x0 = rect.left() + rect.width() * (i as f32 / n as f32);
        let x1 = rect.left() + rect.width() * ((i + 1) as f32 / n as f32);
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            ramp.color(t),
        );
    }
    ui.horizontal(|ui| {
        ui.label("0");
        ui.add_space(180.0);
        ui.label(format!("{max:.0} lx"));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box built in the Factory becomes lighting geometry, split by orientation.
    ///
    /// The SIMLUX scene used to be the 2D document extruded to one height — a footprint with no
    /// openings, no slabs at their real levels and no storeys. With a building in the Factory that
    /// is the wrong room, and a lighting result is only as good as the room it was given.
    #[test]
    fn the_factory_model_becomes_lighting_geometry() {
        let mut f = crate::factory::FactoryState::default();
        f.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box { w: 6.0, d: 4.0, h: 3.0 },
        );
        f.recompute();

        let meshes = meshes_from_factory(&f);
        assert!(!meshes.is_empty(), "a solid box must produce lighting geometry");

        // A closed box has all three orientations, and each must land in its own material so the
        // engine's floor/wall/ceiling reflectances (0.20 / 0.50 / 0.70) actually apply.
        let mats: std::collections::HashSet<u32> = meshes.iter().map(|m| m.material).collect();
        assert!(mats.contains(&0), "an up-facing surface must be FLOOR");
        assert!(mats.contains(&1), "the sides must be WALL");
        assert!(mats.contains(&2), "a down-facing surface must be CEILING");

        // Every triangle index must be in range, or the ray tracer walks off the end of a vertex
        // list — a crash rather than a wrong answer, but only on someone else's model.
        for m in &meshes {
            for t in &m.triangles {
                for i in [t.a, t.b, t.c] {
                    assert!((i as usize) < m.vertices.len(),
                        "index {i} is past the {} vertices of material {}", m.vertices.len(), m.material);
                }
            }
        }
    }

    /// An EMPTY model falls back to the 2D extrusion, so a plan-only project is untouched.
    #[test]
    fn an_empty_factory_leaves_the_2d_workflow_alone() {
        let f = crate::factory::FactoryState::default();
        assert!(meshes_from_factory(&f).is_empty(),
            "nothing modelled means nothing to hand over — the extrusion must stay in charge");
    }

    /// The bounds come from the GEOMETRY, not the drawing.
    ///
    /// The calculation plane and the camera used to be framed from the 2D document's extent,
    /// which includes dimensions, notes and a title block, and on a survey plan sits kilometres
    /// from the building. With the room coming from the model, those are different questions.
    #[test]
    fn scene_bounds_measure_the_model_not_the_drawing() {
        let mut f = crate::factory::FactoryState::default();
        f.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::from_basis(
                glam::Vec3::new(3500.0, -6850.0, 0.0), glam::Vec3::X, glam::Vec3::Y),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box { w: 6.0, d: 4.0, h: 3.0 },
        );
        f.recompute();
        let meshes = meshes_from_factory(&f);

        let (x0, y0, x1, y1) = mesh_bbox(&meshes).expect("a box has bounds");
        assert!((x1 - x0 - 6.0).abs() < 0.01, "width should be 6 m, got {}", x1 - x0);
        assert!((y1 - y0 - 4.0).abs() < 0.01, "depth should be 4 m, got {}", y1 - y0);
        assert!(x0 > 3000.0, "…and it must be found where the building actually is, not at the origin");
        let h = mesh_height(&meshes).expect("a box has height");
        assert!((h - 3.0).abs() < 0.01, "height should be 3 m, got {h}");
    }
}

#[cfg(test)]
mod array_tests {
    use super::*;

    /// A grid array lays fixtures at CELL CENTRES, so the gap to the wall is half the gap between
    /// fixtures.
    ///
    /// This is the standard spacing convention and the reason the tool exists: placing 24 fittings
    /// by hand is 24 clicks that will not be evenly spaced, and putting them at cell CORNERS
    /// instead pushes the outer row against the wall, which over-lights the perimeter and leaves
    /// the middle short.
    #[test]
    fn a_grid_array_centres_fixtures_in_their_cells() {
        let mut s = LightState::new();
        s.mount_height = 3.0;
        s.luminaires.clear();

        // A 12 x 8 m room, 2 rows x 3 cols => 4 m x 4 m pitch, 2 m to each wall.
        let n = s.add_luminaire_grid((0.0, 0.0, 12.0, 8.0), 2, 3);
        assert_eq!(n, 6, "2 rows x 3 cols is 6 fixtures");
        assert_eq!(s.luminaires.len(), 6);

        let xs: Vec<f32> = {
            let mut v: Vec<f32> = s.luminaires.iter().map(|l| l.position.x).collect();
            v.sort_by(f32::total_cmp);
            v.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
            v
        };
        assert_eq!(xs.len(), 3, "three distinct columns, got {xs:?}");
        assert!((xs[0] - 2.0).abs() < 1e-3, "first column at half a pitch from the wall, got {}", xs[0]);
        assert!((xs[1] - 6.0).abs() < 1e-3);
        assert!((xs[2] - 10.0).abs() < 1e-3, "last column symmetric to the first, got {}", xs[2]);

        // Every one is at the mounting height, not the floor.
        assert!(s.luminaires.iter().all(|l| (l.position.z - 3.0).abs() < 1e-6),
            "fixtures must sit at the mount height");
        // …and they all carry the active photometry, or the array would be lit by nothing.
        assert!(s.luminaires.iter().all(|l| l.profile == s.active_profile));
    }

    /// A degenerate room is refused rather than filled with fixtures stacked on one spot.
    #[test]
    fn an_empty_room_places_nothing() {
        let mut s = LightState::new();
        s.luminaires.clear();
        assert_eq!(s.add_luminaire_grid((5.0, 5.0, 5.0, 5.0), 3, 3), 0);
        assert!(s.luminaires.is_empty(), "no room means no array");
        assert!(s.last_msg.contains("bounds"), "and it should say why: {}", s.last_msg);
    }
}

#[cfg(test)]
mod mounting_tests {
    use super::*;

    /// Two bays at different ceiling heights, sharing a floor. Bay A spans x 0..6 with its ceiling
    /// at 4 m; bay B spans x 6..12 at 2.5 m — the shape of an entrance soffit beside a hall.
    pub(super) fn stepped_room() -> Vec<Mesh> {
        let quad = |z: f32, x0: f32, x1: f32, down: bool| -> Mesh {
            // Wound so the normal points DOWN when `down`, which is what marks an underside.
            let v = |x: f32, y: f32| Vertex::new(x, y, z);
            let (a, b, c, d) = if down {
                (v(x0, 0.0), v(x0, 8.0), v(x1, 8.0), v(x1, 0.0))
            } else {
                (v(x0, 0.0), v(x1, 0.0), v(x1, 8.0), v(x0, 8.0))
            };
            Mesh {
                vertices: vec![a, b, c, d],
                triangles: vec![
                    cad_light::Triangle { a: 0, b: 1, c: 2 },
                    cad_light::Triangle { a: 0, b: 2, c: 3 },
                ],
                material: if down { 2 } else { 0 },
            }
        };
        vec![
            quad(0.0, 0.0, 12.0, false), // floor, up-facing
            quad(4.0, 0.0, 6.0, true),   // high ceiling over bay A
            quad(2.5, 6.0, 12.0, true),  // low ceiling over bay B
        ]
    }

    /// The ceiling is found PER POINT, and it is the underside — not the floor, and not the top of
    /// the slab seen from above.
    #[test]
    fn each_point_finds_the_ceiling_above_it() {
        let m = stepped_room();
        let a = ceiling_above(&m, 3.0, 4.0, 0.8).expect("bay A has a ceiling");
        let b = ceiling_above(&m, 9.0, 4.0, 0.8).expect("bay B has a ceiling");
        assert!((a - 4.0).abs() < 1e-3, "bay A ceiling is 4 m, got {a}");
        assert!((b - 2.5).abs() < 1e-3, "bay B ceiling is 2.5 m, got {b}");
        // Outside the footprint there is nothing overhead, and inventing a height would be worse
        // than saying so.
        assert!(ceiling_above(&m, 50.0, 4.0, 0.8).is_none(), "nothing overhead outside the room");
    }

    /// An UP-facing surface is not a ceiling. A ray cast upward crosses both faces of a slab, and
    /// hanging a luminaire from the top one would put it inside the structure.
    #[test]
    fn the_top_of_a_slab_is_not_a_ceiling() {
        let up_only = vec![Mesh {
            vertices: vec![
                Vertex::new(0.0, 0.0, 3.0), Vertex::new(6.0, 0.0, 3.0),
                Vertex::new(6.0, 6.0, 3.0), Vertex::new(0.0, 6.0, 3.0),
            ],
            triangles: vec![
                cad_light::Triangle { a: 0, b: 1, c: 2 },
                cad_light::Triangle { a: 0, b: 2, c: 3 },
            ],
            material: 0,
        }];
        assert!(ceiling_above(&up_only, 3.0, 3.0, 0.8).is_none(),
            "an up-facing surface overhead is the TOP of a slab, not a ceiling");
    }

    /// The array follows the stepped ceiling instead of putting everything at one height.
    ///
    /// One mounting height is right only in a box. Across a step it buries some fixtures in the
    /// slab above and leaves the rest hanging a metre low — and the lux figures then describe
    /// that, not the design, with nothing on screen to say so.
    #[test]
    fn the_array_mounts_each_fixture_to_its_own_ceiling() {
        let mut s = LightState::new();
        s.luminaires.clear();
        s.meshes = stepped_room();
        s.mount_to_ceiling = true;
        s.ceiling_drop = 0.0;
        s.plane_height = 0.8;

        // 1 row x 4 cols over 0..12 => x at 1.5, 4.5, 7.5, 10.5: two under each bay.
        assert_eq!(s.add_luminaire_grid((0.0, 0.0, 12.0, 8.0), 1, 4), 4);
        let mut by_x: Vec<(f32, f32)> =
            s.luminaires.iter().map(|l| (l.position.x, l.position.z)).collect();
        by_x.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!((by_x[0].1 - 4.0).abs() < 1e-3, "x=1.5 is under the high bay, got z={}", by_x[0].1);
        assert!((by_x[1].1 - 4.0).abs() < 1e-3, "x=4.5 is under the high bay, got z={}", by_x[1].1);
        assert!((by_x[2].1 - 2.5).abs() < 1e-3, "x=7.5 is under the low bay, got z={}", by_x[2].1);
        assert!((by_x[3].1 - 2.5).abs() < 1e-3, "x=10.5 is under the low bay, got z={}", by_x[3].1);
        assert!(s.last_msg.contains("2.50–4.00 m"),
            "the message should report the SPREAD on a stepped ceiling: {}", s.last_msg);
    }

    /// A pendant drop hangs below whatever it is fixed to, per fixture.
    #[test]
    fn a_pendant_drop_is_measured_from_each_ceiling() {
        let mut s = LightState::new();
        s.luminaires.clear();
        s.meshes = stepped_room();
        s.mount_to_ceiling = true;
        s.ceiling_drop = 0.5;
        s.plane_height = 0.8;
        s.add_luminaire_grid((0.0, 0.0, 12.0, 8.0), 1, 2);
        let mut z: Vec<f32> = s.luminaires.iter().map(|l| l.position.z).collect();
        z.sort_by(f32::total_cmp);
        assert!((z[0] - 2.0).abs() < 1e-3, "0.5 m below the 2.5 m ceiling, got {}", z[0]);
        assert!((z[1] - 3.5).abs() < 1e-3, "0.5 m below the 4 m ceiling, got {}", z[1]);
    }

    /// Turning it OFF restores one fixed height — the old behaviour, kept for a designer who wants
    /// a uniform mounting plane regardless of what the ceiling does.
    #[test]
    fn a_fixed_height_is_still_available() {
        let mut s = LightState::new();
        s.luminaires.clear();
        s.meshes = stepped_room();
        s.mount_to_ceiling = false;
        s.mount_height = 3.2;
        s.add_luminaire_grid((0.0, 0.0, 12.0, 8.0), 1, 4);
        assert!(s.luminaires.iter().all(|l| (l.position.z - 3.2).abs() < 1e-6),
            "with the toggle off every fixture sits at the set height");
    }
}

/// Placing, picking, moving and fitting out the light points.
///
/// The workflow these cover is the one the user asked for: mark the spots first, choose the
/// product afterwards, and be able to change your mind about either. Before this, "place" was a
/// checkbox no click handler read, and a placed fixture could not be moved at all.
#[cfg(test)]
mod placement_tests {
    use super::*;

    fn room() -> LightState {
        let mut s = LightState::new();
        s.luminaires.clear();
        s.mount_to_ceiling = false;
        s.mount_height = 3.0;
        s
    }

    /// A real fitting, so "assigned" can be told from "not".
    fn fitting(name: &str) -> IesProfile {
        let mut p = builtin_downlight();
        p.name = name.to_string();
        p
    }

    /// A fresh project has NO fitting chosen, so a placed point is a mark on the plan and nothing
    /// more. Starting with the built-in already active would make step ③ invisible: every point
    /// would silently become a generic downlight the user never picked.
    #[test]
    fn a_new_point_starts_without_a_fitting() {
        let mut s = room();
        assert_eq!(s.active_profile, UNASSIGNED, "nothing is chosen on a fresh project");
        let id = s.place_point(2.0, 3.0);
        assert_eq!(s.luminaires.len(), 1);
        assert_eq!(s.unassigned_count(), 1, "the point is waiting for a fitting");
        assert_eq!(s.selected, vec![id], "and it is selected, ready to be fitted out");
        assert!(!s.is_assigned(&s.luminaires[0]));
    }

    /// Step ③: choosing a fitting fills in the points that have none.
    #[test]
    fn choosing_a_fitting_fills_in_the_points_that_have_none() {
        let mut s = room();
        s.profiles.insert("Downlight 3000K".into(), fitting("Downlight 3000K"));
        s.place_point(1.0, 1.0);
        s.place_point(2.0, 1.0);
        s.place_point(3.0, 1.0);
        s.clear_selection(); // nothing picked out — so it should reach every waiting point
        assert_eq!(s.assign_profile("Downlight 3000K"), 3);
        assert_eq!(s.unassigned_count(), 0);
        assert!(s.luminaires.iter().all(|l| l.profile == "Downlight 3000K"));
    }

    /// …but a SELECTION wins over "everything unassigned". Re-fitting part of a layout is the
    /// normal second act of a design, and it must not touch the rest.
    #[test]
    fn a_selection_narrows_the_assignment_to_it() {
        let mut s = room();
        s.profiles.insert("A".into(), fitting("A"));
        s.profiles.insert("B".into(), fitting("B"));
        let a = s.place_point(1.0, 1.0);
        let b = s.place_point(2.0, 1.0);
        let c = s.place_point(3.0, 1.0);
        s.clear_selection();
        s.assign_profile("A");
        s.select(b, false);
        s.select(c, true);
        assert_eq!(s.assign_profile("B"), 2);
        let by = |id: u32| s.luminaires.iter().find(|l| l.id == id).unwrap().profile.clone();
        assert_eq!(by(a), "A", "the unselected fixture keeps its fitting");
        assert_eq!(by(b), "B");
        assert_eq!(by(c), "B");
    }

    /// Picking takes the NEAREST marker, not the first one within reach. On a tight pitch two
    /// markers overlap, and grabbing whichever came first in the list moves the wrong light.
    #[test]
    fn picking_takes_the_nearest_marker() {
        let mut s = room();
        let far = s.place_point(0.0, 0.0);
        let near = s.place_point(0.30, 0.0);
        assert_eq!(s.pick_at(0.25, 0.0, 0.5), Some(near), "0.25 is nearer the 0.30 marker");
        assert_eq!(s.pick_at(0.05, 0.0, 0.5), Some(far));
        assert_eq!(s.pick_at(5.0, 5.0, 0.5), None, "nothing within reach");
    }

    /// A fixture can be MOVED — the thing that was impossible before. The drag carries the whole
    /// selection, so a grid can be nudged as one.
    #[test]
    fn a_drag_moves_every_selected_fixture_together() {
        let mut s = room();
        let a = s.place_point(1.0, 1.0);
        let b = s.place_point(3.0, 1.0);
        s.select(a, false);
        s.select(b, true);
        s.begin_drag(a, (1.0, 1.0));
        s.drag_to((1.5, 2.0));
        assert!(s.end_drag(), "the drag moved something");
        let pos = |id: u32| {
            let l = s.luminaires.iter().find(|l| l.id == id).unwrap();
            (l.position.x, l.position.y)
        };
        assert_eq!(pos(a), (1.5, 2.0));
        assert_eq!(pos(b), (3.5, 2.0), "the other selected fixture moved by the same delta");
    }

    /// Pressing on an UNSELECTED marker grabs that one alone — a drag always moves what is under
    /// the pointer, not a selection made earlier and forgotten about.
    #[test]
    fn pressing_an_unselected_marker_grabs_only_it() {
        let mut s = room();
        let a = s.place_point(1.0, 1.0);
        let b = s.place_point(3.0, 1.0);
        s.select(a, false);
        s.begin_drag(b, (3.0, 1.0));
        s.drag_to((4.0, 1.0));
        s.end_drag();
        assert_eq!(s.selected, vec![b]);
        let by = |id: u32| s.luminaires.iter().find(|l| l.id == id).unwrap().position.x;
        assert_eq!(by(a), 1.0, "the previously selected fixture stayed put");
        assert_eq!(by(b), 4.0);
    }

    /// A press that never moves is a CLICK: it selects, and reports that nothing moved, so the
    /// same gesture serves both "pick this one" and "move this one".
    #[test]
    fn a_press_without_motion_is_a_selection_not_a_move() {
        let mut s = room();
        let a = s.place_point(1.0, 1.0);
        s.begin_drag(a, (1.0, 1.0));
        assert!(!s.end_drag(), "nothing moved");
        assert_eq!(s.selected, vec![a], "but it is now selected");
        let l = &s.luminaires[0];
        assert_eq!((l.position.x, l.position.y), (1.0, 1.0));
    }

    /// Dropping a fixture under a different ceiling RE-MOUNTS it. A light dragged from the hall to
    /// under the soffit belongs to the soffit; keeping the old height would bury it in the slab.
    #[test]
    fn a_dropped_fixture_re_mounts_to_the_ceiling_it_landed_under() {
        let mut s = room();
        s.meshes = super::mounting_tests::stepped_room();
        s.mount_to_ceiling = true;
        s.plane_height = 0.8;
        let id = s.place_point(3.0, 4.0); // under the 4 m bay
        assert!((s.luminaires[0].position.z - 4.0).abs() < 1e-3);
        s.begin_drag(id, (3.0, 4.0));
        s.drag_to((9.0, 4.0)); // over into the 2.5 m bay
        assert!(s.end_drag());
        assert!((s.luminaires[0].position.z - 2.5).abs() < 1e-3,
            "it should hang from the low ceiling it was dropped under, got {}",
            s.luminaires[0].position.z);
    }

    /// Deleting removes exactly the selection and nothing else.
    #[test]
    fn delete_removes_the_selection_only() {
        let mut s = room();
        let a = s.place_point(1.0, 1.0);
        let b = s.place_point(2.0, 1.0);
        let c = s.place_point(3.0, 1.0);
        s.select(a, false);
        s.select(c, true);
        assert_eq!(s.delete_selected(), 2);
        assert_eq!(s.luminaires.len(), 1);
        assert_eq!(s.luminaires[0].id, b);
        assert!(s.selected.is_empty());
    }

    /// Removing a fitting from the library leaves its fixtures UNASSIGNED — visible and counted —
    /// rather than pointing at a name that resolves to nothing and silently emits no light.
    #[test]
    fn removing_a_fitting_leaves_its_fixtures_needing_one() {
        let mut s = room();
        s.profiles.insert("A".into(), fitting("A"));
        s.place_point(1.0, 1.0);
        s.place_point(2.0, 1.0);
        s.clear_selection();
        s.assign_profile("A");
        assert_eq!(s.unassigned_count(), 0);
        s.remove_profile("A");
        assert_eq!(s.unassigned_count(), 2, "both fixtures now need a fitting");
        assert_eq!(s.active_profile, UNASSIGNED);
    }

    /// The built-in is generated rather than imported, so it cannot be removed — the library is
    /// never empty and there is always something to light a room with.
    #[test]
    fn the_builtin_fitting_cannot_be_removed() {
        let mut s = room();
        s.remove_profile(BUILTIN);
        assert!(s.profiles.contains_key(BUILTIN));
    }

    /// Ids keep counting up across a save/reopen. Restarting at #1 would hand two fixtures the
    /// same id, and every id-keyed operation — select, drag, delete — would then hit both.
    #[test]
    fn reopening_a_project_keeps_the_layout_and_the_id_sequence() {
        let mut s = room();
        s.profiles.insert("A".into(), fitting("A"));
        s.place_point(1.0, 1.0);
        s.place_point(2.0, 1.0);
        s.clear_selection();
        s.assign_profile("A");
        let doc = Document::default();
        let cfg = s.to_config(&doc);
        assert_eq!(cfg.luminaires.len(), 2, "the layout is written to the sidecar");

        let mut reopened = LightState::new();
        reopened.luminaires.clear();
        reopened.apply_config(cfg, &doc);
        assert_eq!(reopened.luminaires.len(), 2);
        assert_eq!(reopened.unassigned_count(), 0, "the fitting came back with the library");
        let next = reopened.place_point(9.0, 9.0);
        assert!(next > 2, "a new point gets a fresh id, not one already in use");
    }

    /// A NEW project computes maintained illuminance. This is the setting that decides whether
    /// every lux figure the app reports is submittable or 20% optimistic, so it is pinned.
    #[test]
    fn a_new_project_is_quoted_at_a_maintenance_factor() {
        let s = LightState::new();
        let mf = s.maintenance.factor();
        assert!(mf < 1.0, "a fresh project must not report INITIAL lux as the answer, got {mf}");
        assert!((0.78..=0.82).contains(&mf), "the shipped default is about 0.80, got {mf}");
    }

    /// …but a project saved BEFORE maintenance existed comes back at the initial condition.
    ///
    /// Adopting today's default on load would silently restate every number in a result the user
    /// has already read and possibly issued — a 20% change to a document they believe they are
    /// merely reopening.
    #[test]
    fn an_older_project_reopens_at_the_condition_it_was_calculated_at() {
        let doc = Document::default();
        let mut s = LightState::new();
        let mut cfg = s.to_config(&doc);
        cfg.maintenance = None; // as written by a build that predates the factor
        s.apply_config(cfg, &doc);
        assert_eq!(s.maintenance.factor(), 1.0, "restored as INITIAL, not silently maintained");
    }

    /// A maintenance factor set by the user round-trips a save.
    #[test]
    fn the_maintenance_factor_survives_a_save() {
        let doc = Document::default();
        let mut s = LightState::new();
        s.maintenance = Maintenance { llmf: 0.88, lsf: 0.99, lmf: 0.85, rsmf: 0.92 };
        let want = s.maintenance.factor();
        let cfg = s.to_config(&doc);
        let mut reopened = LightState::new();
        reopened.apply_config(cfg, &doc);
        assert!((reopened.maintenance.factor() - want).abs() < 1e-12);
        assert!((reopened.maintenance.llmf - 0.88).abs() < 1e-12, "the sub-factors, not just the product");
    }

    /// A fixture whose fitting did NOT come back comes in unassigned, so the toolbar can say so.
    #[test]
    fn a_missing_fitting_comes_back_as_unassigned() {
        let mut s = room();
        s.profiles.insert("Gone".into(), fitting("Gone"));
        s.place_point(1.0, 1.0);
        s.clear_selection();
        s.assign_profile("Gone");
        let doc = Document::default();
        let mut cfg = s.to_config(&doc);
        cfg.ies_library.clear(); // the library entry went missing
        let mut reopened = LightState::new();
        reopened.apply_config(cfg, &doc);
        assert_eq!(reopened.unassigned_count(), 1);
    }
}

/// FURNITURE IS PART OF THE LIGHTING SCENE.
///
/// It was not. `meshes_from_factory` read `cached.positions` — the CSG solid mesh — and furniture
/// lives separately as instanced assets, so every cupboard, kitchen and desk placed in the Factory
/// was INVISIBLE to the calculation and every room was computed as an empty box.
///
/// That is the whole of the +48 % against DIALux on the DISTRICT PEOPLE project: the engine's
/// interreflection is verified correct against the radiosity closed form, and an empty box at the
/// reported 0.70 / 0.82 / 0.72 really does produce that much light. A shop full of racks does not,
/// and its measured uniformity — U₀ 0.17 against our 0.59 — says so.
#[cfg(test)]
mod furniture_in_the_light_scene {
    use super::*;

    /// A slab asset: one square metre of horizontal surface, `n` metres up in its own local space.
    fn slab_asset(f: &mut crate::factory::FactoryState, half: f32, z: f32) -> usize {
        let v = |x: f32, y: f32| [x, y, z];
        let positions = vec![
            v(-half, -half),
            v(half, -half),
            v(half, half),
            v(-half, -half),
            v(half, half),
            v(-half, half),
        ];
        let normals = vec![[0.0, 0.0, 1.0]; 6];
        f.add_furniture_asset(
            "slab".into(),
            crate::mesh_io::ObjMesh { positions, normals, color: None, alpha: Vec::new() },
        )
    }

    fn a_room() -> crate::factory::FactoryState {
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(
            &vec![
                glam::Vec2::new(0.0, 0.0),
                glam::Vec2::new(6.0, 0.0),
                glam::Vec2::new(6.0, 6.0),
                glam::Vec2::new(0.0, 6.0),
                glam::Vec2::new(0.0, 0.0),
            ],
            3.0,
        )
        .expect("building");
        f.recompute();
        f
    }

    /// THE BUG. A room with furniture in it must hand the engine more geometry than the same room
    /// without — that is the whole of what was missing.
    #[test]
    fn furniture_reaches_the_engine_at_all() {
        let mut f = a_room();
        let bare = meshes_from_factory(&f).iter().map(|m| m.triangles.len()).sum::<usize>();
        assert!(bare > 0, "the building itself should be there");

        let a = slab_asset(&mut f, 0.5, 0.0);
        f.place_mode = crate::factory::PlaceMode::Centre;
        f.place_furniture(a, glam::Vec3::new(3.0, 3.0, 0.0));
        let with = meshes_from_factory(&f).iter().map(|m| m.triangles.len()).sum::<usize>();
        assert_eq!(with, bare + 2, "the slab's two triangles must reach the engine");
    }

    /// …under its OWN material, not bucketed by orientation with the building. A desk top is not a
    /// floor; giving a shop's stock the ceiling's 0.70 would recreate the error this fixes.
    #[test]
    fn furniture_gets_its_own_material() {
        let mut f = a_room();
        let a = slab_asset(&mut f, 0.5, 0.0);
        f.place_mode = crate::factory::PlaceMode::Centre;
        f.place_furniture(a, glam::Vec3::new(3.0, 3.0, 0.0));
        let meshes = meshes_from_factory(&f);
        let furn = meshes.iter().find(|m| m.material == MATERIAL_FURNITURE);
        assert!(furn.is_some(), "furniture must be its own mesh");
        assert_eq!(furn.unwrap().triangles.len(), 2);
        // The slab faces UP, so a bucket-by-orientation pass would have filed it as floor.
        let floor = meshes.iter().find(|m| m.material == 0).map(|m| m.triangles.len()).unwrap_or(0);
        let bare_floor = {
            let mut g = a_room();
            let _ = &mut g;
            meshes_from_factory(&g).iter().find(|m| m.material == 0).map(|m| m.triangles.len()).unwrap_or(0)
        };
        assert_eq!(floor, bare_floor, "the slab was filed as floor instead of furniture");
    }

    /// Its POSE is applied. A piece is placed somewhere, and the engine has to see it there —
    /// geometry delivered at the asset's local origin would shade the wrong part of the room.
    #[test]
    fn the_instance_transform_is_applied() {
        let mut f = a_room();
        let a = slab_asset(&mut f, 0.5, 0.0);
        f.place_mode = crate::factory::PlaceMode::Centre;
        f.place_furniture(a, glam::Vec3::new(4.5, 1.5, 0.0));
        let meshes = meshes_from_factory(&f);
        let m = meshes.iter().find(|m| m.material == MATERIAL_FURNITURE).unwrap();
        let cx = m.vertices.iter().map(|v| v.x).sum::<f32>() / m.vertices.len() as f32;
        let cy = m.vertices.iter().map(|v| v.y).sum::<f32>() / m.vertices.len() as f32;
        assert!((cx - 4.5).abs() < 1e-3, "x = {cx}, expected 4.5");
        assert!((cy - 1.5).abs() < 1e-3, "y = {cy}, expected 1.5");
    }

    /// AND IT ACTUALLY SHADES. The point of all of it: a slab between the fitting and the work
    /// plane must darken the point beneath it.
    #[test]
    fn furniture_casts_a_shadow_on_the_work_plane() {
        use cad_light::{calculate, CalcPlane, Luminaire, RaySettings, Vertex};
        use std::collections::HashMap;

        let mut f = a_room();
        // A 2 m square panel, hung at 2 m by its INSTANCE rather than by its geometry: assets are
        // rebased to z = 0 on import (`add_furniture_asset`), so a slab authored at z = 2 lands on
        // the floor and shades nothing. Height belongs to the placement.
        let a = slab_asset(&mut f, 1.0, 0.0);

        let mut profiles = HashMap::new();
        profiles.insert("p".to_string(), builtin_downlight());
        let lums = vec![Luminaire {
            id: 1,
            profile: "p".into(),
            position: Vertex::new(3.0, 3.0, 2.9),
            rotation_deg: 0.0,
            dimming: 1.0,
        }];
        // One cell, directly under the fitting.
        let plane = CalcPlane {
            origin: Vertex::new(2.9, 2.9, 0.8),
            width: 0.2,
            depth: 0.2,
            cols: 1,
            rows: 1,
        };
        // DIRECT ONLY: the shadow is the thing under test, and bounced light would fill it in and
        // blur exactly the effect being measured.
        let settings = RaySettings { rays_per_point: 1, max_bounces: 0, shadows: true };
        let materials = cad_light::default_materials();

        let open = calculate(
            &meshes_from_factory(&f),
            &lums,
            &profiles,
            &materials,
            &plane,
            &settings,
        );
        f.place_mode = crate::factory::PlaceMode::Centre;
        f.place_furniture(a, glam::Vec3::new(3.0, 3.0, 2.0));
        let shaded = calculate(
            &meshes_from_factory(&f),
            &lums,
            &profiles,
            &materials,
            &plane,
            &settings,
        );

        assert!(open.avg > 1.0, "precondition: the point is lit with nothing in the way");
        assert!(
            shaded.avg < open.avg * 0.05,
            "the panel should block the fitting: {:.1} lx open, {:.1} lx shaded",
            open.avg,
            shaded.avg,
        );
    }
}

/// A CURVED LIGHT IS A LIGHT.
///
/// It was not. `factory_build_sweeplight` produced furniture with an emissive lens texture: it
/// glowed in the raytraced render and contributed exactly nothing to a calculation. That is the
/// most misleading state a lighting tool can be in — the picture is lit and the numbers are dark,
/// and neither one says the other is wrong.
#[cfg(test)]
mod curved_lights_are_real_lights {
    use super::*;

    /// A fitting whose emitters are attached, placed somewhere specific.
    fn a_room_with_a_curved_light(at: glam::Vec3) -> crate::factory::FactoryState {
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(
            &vec![
                glam::Vec2::new(0.0, 0.0),
                glam::Vec2::new(6.0, 0.0),
                glam::Vec2::new(6.0, 6.0),
                glam::Vec2::new(0.0, 6.0),
                glam::Vec2::new(0.0, 0.0),
            ],
            3.0,
        )
        .expect("building");
        // A minimal body for the fixture: the asset needs geometry, and the rebase is applied to
        // the emitters through the SAME bounds, so this stands in for the real extrusion.
        let v = |x: f32, z: f32| [x, 0.0, z];
        let positions = vec![v(-0.5, 0.0), v(0.5, 0.0), v(0.5, 0.1), v(-0.5, 0.0), v(0.5, 0.1), v(-0.5, 0.1)];
        let idx = f.add_furniture_asset(
            "Curved light 1".into(),
            crate::mesh_io::ObjMesh { positions, normals: vec![[0.0, -1.0, 0.0]; 6], color: None, alpha: Vec::new() },
        );
        if let Some(a) = f.furniture_lib.get_mut(idx) {
            a.cct_k = 3000;
            a.emitters = vec![
                crate::factory::FurnEmitter { pos: [-0.25, 0.0, 0.0], lumens: 1000.0, watts: 10.0 },
                crate::factory::FurnEmitter { pos: [0.25, 0.0, 0.0], lumens: 1000.0, watts: 10.0 },
            ];
        }
        f.place_mode = crate::factory::PlaceMode::Centre;
        f.place_furniture(idx, at);
        f.recompute();
        f
    }

    /// THE BUG: a placed curved light must appear in the luminaire list the calculation runs on.
    #[test]
    fn its_emitters_become_luminaires() {
        let f = a_room_with_a_curved_light(glam::Vec3::new(3.0, 3.0, 2.5));
        let mut s = LightState::new();
        assert!(s.luminaires.is_empty(), "nothing was placed by hand");
        let lums = s.generated_luminaires(&f);
        assert_eq!(lums.len(), 2, "both emitting points must reach the engine");
    }

    /// …with photometry behind them. A luminaire naming a profile that is not in the table
    /// contributes nothing, and would look exactly like the bug being fixed.
    #[test]
    fn a_photometry_is_registered_for_them() {
        let f = a_room_with_a_curved_light(glam::Vec3::new(3.0, 3.0, 2.5));
        let mut s = LightState::new();
        let lums = s.generated_luminaires(&f);
        let p = s.profiles.get(&lums[0].profile).expect("its profile must be in the table");
        // Lambertian: Phi = pi * I0, so a 1000 lm point peaks at 1000/pi cd straight down.
        assert!((p.candela[0][0] - 1000.0 / std::f64::consts::PI).abs() < 1e-6, "I0 = {}", p.candela[0][0]);
        assert!(p.candela[0][18] < 1e-9, "and nothing at the horizon");
        assert_eq!(p.watts, 10.0, "its share of the connected load, for the power density");
    }

    /// THE POINT OF DERIVING THEM. Move the fixture and its light moves — this is why the emitters
    /// live on the asset rather than being written into the luminaire list once at build time.
    #[test]
    fn the_light_follows_the_fixture() {
        let mut s = LightState::new();
        let here = s.generated_luminaires(&a_room_with_a_curved_light(glam::Vec3::new(1.0, 1.0, 2.5)));
        let there = s.generated_luminaires(&a_room_with_a_curved_light(glam::Vec3::new(4.0, 2.0, 2.0)));
        let mid = |v: &[Luminaire]| {
            let n = v.len() as f32;
            (v.iter().map(|l| l.position.x).sum::<f32>() / n, v.iter().map(|l| l.position.y).sum::<f32>() / n,
             v.iter().map(|l| l.position.z).sum::<f32>() / n)
        };
        let (ax, ay, az) = mid(&here);
        let (bx, by, bz) = mid(&there);
        assert!((ax - 1.0).abs() < 1e-3 && (ay - 1.0).abs() < 1e-3, "first at ({ax}, {ay})");
        assert!((bx - 4.0).abs() < 1e-3 && (by - 2.0).abs() < 1e-3, "second at ({bx}, {by})");
        assert!((az - bz).abs() > 0.4, "and it carried its mounting height with it: {az} vs {bz}");
    }

    /// Ordinary furniture is not a light. A chair with an emissive-looking texture must not start
    /// emitting because this path exists.
    #[test]
    fn ordinary_furniture_emits_nothing() {
        let mut f = a_room_with_a_curved_light(glam::Vec3::new(3.0, 3.0, 2.5));
        for a in &mut f.furniture_lib {
            a.emitters.clear();
        }
        let mut s = LightState::new();
        assert!(s.generated_luminaires(&f).is_empty());
    }

    /// AND IT ACTUALLY LIGHTS THE ROOM. Everything above could pass with the luminaires assembled
    /// correctly and still handed to nothing.
    #[test]
    fn the_room_is_brighter_with_it_than_without() {
        let doc = cad_kernel::Document::default();
        let lit = a_room_with_a_curved_light(glam::Vec3::new(3.0, 3.0, 2.9));
        let mut dark = a_room_with_a_curved_light(glam::Vec3::new(3.0, 3.0, 2.9));
        for a in &mut dark.furniture_lib {
            a.emitters.clear();
        }

        let avg = |f: &crate::factory::FactoryState| {
            let mut s = LightState::new();
            s.auto_center_light = false; // or the stand-in light would supply the difference
            s.calculate(&doc, Some(f));
            s.grid.as_ref().map(|g| g.avg).unwrap_or(0.0)
        };
        let (on, off) = (avg(&lit), avg(&dark));
        assert!(off < 1e-6, "precondition: with no emitters the room is dark, got {off:.3} lx");
        assert!(on > 1.0, "the curved light must light the room: {on:.1} lx");
    }

    /// COLOUR TEMPERATURE MUST NOT CHANGE THE LUX. Photometric units are already V(lambda)-
    /// weighted, so a 2700 K and a 6500 K fitting of the same output give the same illuminance.
    /// If CCT ever leaks into the flux path — as a tint multiplying lumens, say — this fails.
    #[test]
    fn colour_temperature_does_not_change_the_illuminance() {
        let doc = cad_kernel::Document::default();
        let avg_at = |cct: u32| {
            let mut f = a_room_with_a_curved_light(glam::Vec3::new(3.0, 3.0, 2.9));
            for a in &mut f.furniture_lib {
                a.cct_k = cct;
            }
            let mut s = LightState::new();
            s.auto_center_light = false;
            s.calculate(&doc, Some(&f));
            s.grid.as_ref().map(|g| g.avg).unwrap_or(0.0)
        };
        let (warm, cool) = (avg_at(2700), avg_at(6500));
        assert!(warm > 1.0, "precondition: it is lit");
        assert!((warm - cool).abs() < 1e-9, "2700 K gave {warm:.4} lx, 6500 K gave {cool:.4} lx");
    }

    /// The tint it DOES drive has to be the right way round: warm is redder than cool.
    #[test]
    fn the_lens_tint_follows_the_colour_temperature() {
        let warm = crate::factory::cct_to_linear_rgb(2700);
        let cool = crate::factory::cct_to_linear_rgb(6500);
        assert!(warm[0] > warm[1] && warm[1] > warm[2], "2700 K must run red > green > blue: {warm:?}");
        assert!(cool[2] > warm[2], "6500 K must be bluer than 2700 K: {cool:?} vs {warm:?}");
        // 6500 K is essentially the sRGB white point, so it should come out near neutral.
        assert!((cool[0] - cool[2]).abs() < 0.05, "6500 K should be close to white: {cool:?}");
        // A halogen fitting is warmer than a sodium one is not; the ordering has to be monotone.
        let mid = crate::factory::cct_to_linear_rgb(4000);
        assert!(warm[2] < mid[2] && mid[2] < cool[2], "blue must rise with CCT: {warm:?} {mid:?} {cool:?}");
    }
}

/// THE REBUILT PROJECT FILES, END TO END.
///
/// `identical_dialux_furniture.rs` proves the ENGINE against DIALux by assembling the scene in
/// code. That leaves a gap wide enough to drive a project through: the app does not assemble
/// scenes in code, it loads them from a `.simlux.json` and derives the meshes from a CSG feature
/// tree. A room that is right in the test and wrong in the file — a ceiling slab at the wrong
/// height, furniture floating, a fitting 150 mm low — reads as a correct engine and a wrong answer.
///
/// The user's own `testfiles.simlux.json` was exactly that: its ceiling slab sat at 0.52 m, so the
/// room's clear height was 0.37 m rather than 4.000, and the bike stood on top of the misplaced
/// slab. This loads the corrected files through the app's REAL loader and checks the room they
/// describe against DIALux, so the geometry is verified rather than asserted.
#[cfg(test)]
mod the_project_file_describes_the_dialux_room {
    use super::*;

    /// Where the files are. Skipped, loudly, when it is not set.
    fn dir() -> Option<String> {
        std::env::var("IDENTICAL_PROJECTS").ok()
    }

    struct Case {
        file: &'static str,
        /// Ē as the matching DIALux report states it (t3's summary is stale, so it has none).
        dialux: Option<f64>,
    }
    const CASES: [Case; 3] = [
        Case { file: "t1 with furniture.simlux.json", dialux: Some(199.0) },
        Case { file: "t2 with furniture.simlux.json", dialux: Some(336.0) },
        Case { file: "t3 with furniture.simlux.json", dialux: None },
    ];

    #[test]
    #[ignore = "needs IDENTICAL_PROJECTS=<folder of rebuilt .simlux.json files>"]
    fn the_rebuilt_files_reproduce_dialux() {
        let Some(dir) = dir() else {
            println!("set IDENTICAL_PROJECTS to the folder holding the rebuilt project files");
            return;
        };
        for case in &CASES {
            let path = format!("{dir}/{}", case.file);
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
            let cfg: crate::simlux_io::SimluxConfig =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"));

            // Through the app's own loader, not a hand-built FactoryState.
            let mut f = crate::factory::FactoryState::default();
            f.apply_persist(cfg.factory.clone());
            f.recompute();
            let meshes = meshes_from_factory(&f);
            assert!(!meshes.is_empty(), "{}: the file produced no geometry at all", case.file);

            // THE GEOMETRY THE FILE DESCRIBES. Check it before checking the light, so a wrong
            // answer says WHICH thing is wrong.
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for m in &meshes {
                for v in &m.vertices {
                    for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                        lo[k] = lo[k].min(c);
                        hi[k] = hi[k].max(c);
                    }
                }
            }
            println!("\n=== {} ===", case.file);
            println!("  model z {:.3} .. {:.3} m   ({} meshes)", lo[2], hi[2], meshes.len());
            let furn = meshes.iter().find(|m| m.material == cad_light::MATERIAL_FURNITURE);
            let fz = furn.map(|m| m.vertices.iter().fold(f32::MAX, |a, v| a.min(v.z)));
            println!("  furniture base z {:?}   luminaires {}", fz, cfg.luminaires.len());
            assert!(
                fz.is_some_and(|z| z.abs() < 0.02),
                "{}: the furniture must stand ON the floor, base at z = {:?}",
                case.file,
                fz,
            );
            assert!(
                cfg.luminaires.iter().all(|l| (l.position.z - 4.0).abs() < 1e-3),
                "{}: DIALux mounts at 4.000 m",
                case.file,
            );

            // The room's own footprint, so the grid can be laid on it the way DIALux lays it.
            let room = f.rooms.first().expect("the file carries a room");
            let (rx, ry) = (
                room.footprint.iter().fold(f32::MAX, |a, p| a.min(p[0])),
                room.footprint.iter().fold(f32::MAX, |a, p| a.min(p[1])),
            );
            const WALL_ZONE: f32 = 0.010;
            let plane = cad_light::CalcPlane {
                origin: cad_light::Vertex::new(rx + WALL_ZONE, ry + WALL_ZONE, cfg.plane_height),
                width: 4.0 - 2.0 * WALL_ZONE,
                depth: 4.0 - 2.0 * WALL_ZONE,
                cols: 8,
                rows: 8,
            };
            let profiles: HashMap<String, IesProfile> =
                cfg.ies_library.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let settings = RaySettings { rays_per_point: 4096, max_bounces: 8, shadows: true };
            let grid = cad_light::calculate_maintained(
                &meshes,
                &cfg.luminaires,
                &profiles,
                &cfg.materials,
                &plane,
                &settings,
                cfg.maintenance.expect("the file states its maintenance factor"),
            );
            let avg = grid.values.iter().sum::<f64>() / grid.values.len() as f64;
            for r in 0..8 {
                println!(
                    "  r{r}  {}",
                    (0..8).map(|c| format!("{:>6.0}", grid.values[r * 8 + c])).collect::<Vec<_>>().join(""),
                );
            }
            match case.dialux {
                Some(want) => {
                    let err = (avg - want) / want * 100.0;
                    println!("  E average {avg:>8.1} lx   DIALux {want:>6.0}   {err:>+6.2}%");
                    assert!(err.abs() < 3.0, "{}: {avg:.1} lx against DIALux's {want:.0}", case.file);
                }
                None => println!("  E average {avg:>8.1} lx   (that report's summary is stale)"),
            }
        }
    }
}

/// A FITTING CANNOT BE SAMPLED INTO UNBOUNDED WORK.
///
/// Reported as: "why is the app frozen? it froze after i gave calculate."
///
/// The emitter count is the PATH LENGTH divided by a spacing, and a curved light swept along a
/// drawn 2D curve has no bound on its path. The user's own session snapshot carries a circle of
/// 30 m radius — 188 m around, 753 point sources at 0.25 m for ONE fitting, and their scene had
/// three. Every calculation point, every cylindrical sample and every surface sample then fires a
/// shadow ray at each of ~2 250 luminaires, on the UI thread, with no progress and no way out.
#[cfg(test)]
mod a_fitting_is_bounded_work {
    use super::*;
    use crate::factory::FurnEmitter;

    fn run(n: usize, lm_each: f64) -> Vec<FurnEmitter> {
        (0..n)
            .map(|i| FurnEmitter {
                pos: [i as f32 * 0.25, 0.0, 0.0],
                lumens: lm_each,
                watts: lm_each / 100.0,
            })
            .collect()
    }

    /// A 188 m ring must not put 753 luminaires into the calculation.
    #[test]
    fn a_long_run_is_capped() {
        let merged = merge_emitters(&run(753, 10.0));
        assert!(
            merged.len() <= crate::app::MAX_EMITTERS_PER_FIXTURE,
            "753 points came through as {}",
            merged.len(),
        );
        assert!(merged.len() > 1, "…but it is still sampled as a line, not collapsed to a point");
    }

    /// AND THE LIGHT IS ALL STILL THERE. Capping the count must not dim the fitting — the whole
    /// difference between sampling a line more coarsely and throwing part of it away.
    #[test]
    fn the_flux_is_conserved_exactly() {
        for n in [1usize, 119, 120, 121, 753, 2000] {
            let src = run(n, 10.0);
            let want: f64 = src.iter().map(|e| e.lumens).sum();
            let got: f64 = merge_emitters(&src).iter().map(|e| e.lumens).sum();
            assert!(
                (got - want).abs() < 1e-9,
                "n = {n}: {got} lm out of {want} lm — a cap that dims the fitting is not a cap",
            );
            let ww: f64 = src.iter().map(|e| e.watts).sum();
            let gw: f64 = merge_emitters(&src).iter().map(|e| e.watts).sum();
            assert!((gw - ww).abs() < 1e-9, "n = {n}: the connected load moved too");
        }
    }

    /// The merged points must still lie ALONG the run, not pile up at one end — a merged point
    /// sits at the centroid of the ones it replaces.
    #[test]
    fn the_merged_points_still_span_the_run() {
        let src = run(753, 10.0);
        let merged = merge_emitters(&src);
        let (lo, hi) = (merged[0].pos[0], merged[merged.len() - 1].pos[0]);
        let span = src[src.len() - 1].pos[0];
        assert!(lo < span * 0.02, "the first merged point is not near the start: {lo}");
        assert!(hi > span * 0.98, "the last is not near the end: {hi} of {span}");
    }

    /// A short run is left completely alone — no merging, no repositioning.
    #[test]
    fn a_short_run_is_untouched() {
        let src = run(40, 10.0);
        let merged = merge_emitters(&src);
        assert_eq!(merged.len(), 40);
        for (a, b) in src.iter().zip(&merged) {
            assert_eq!(a.pos, b.pos);
            assert_eq!(a.lumens, b.lumens);
        }
    }

    /// The count on screen must be the count that will be CALCULATED, or the strip promises one
    /// cost and Calculate pays another.
    #[test]
    fn the_strip_counts_what_calculate_will_run() {
        let mut f = crate::factory::FactoryState::default();
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let idx = f.add_furniture_asset(
            "Curved light 1".into(),
            crate::mesh_io::ObjMesh { positions, normals: vec![[0.0, 0.0, 1.0]; 3], color: None, alpha: Vec::new() },
        );
        if let Some(a) = f.furniture_lib.get_mut(idx) {
            a.cct_k = 3000;
            a.emitters = run(753, 10.0);
        }
        f.place_mode = crate::factory::PlaceMode::Centre;
        f.place_furniture(idx, glam::Vec3::new(0.0, 0.0, 0.0));

        let mut s = LightState::new();
        s.refresh_model_fixtures(&f);
        let built = s.generated_luminaires(&f).len();
        assert_eq!(s.model_fixtures, built, "the strip said {} and Calculate runs {built}", s.model_fixtures);
        assert!(built <= crate::app::MAX_EMITTERS_PER_FIXTURE);
    }
}

/// THE SIMLUX VIEW HAS TO BE USABLE ON A REAL PLAN.
///
/// Orbit and zoom alone cannot reach the corner of a large building: the pivot stays put and the
/// room swings around it. And the result is painted on the FLOOR, so a closed box hides the one
/// surface the view exists to show.
#[cfg(test)]
mod the_simlux_view_can_be_read {
    use super::*;

    /// Panning moves the camera TARGET across the screen plane, not along one world axis — a pan
    /// that ignored the yaw would slide sideways in the wrong direction as soon as you orbited.
    #[test]
    fn a_pan_follows_the_camera_not_the_world() {
        let mut s = LightState::new();
        s.cam_target = [0.0, 0.0, 0.0];
        s.cam_pitch = 0.0;
        s.cam_dist = 10.0;

        s.cam_yaw = 0.0;
        s.pan(100.0, 0.0);
        let a = s.cam_target;
        assert!(a[0].abs() < 1e-4, "at yaw 0 a horizontal drag must not move x: {a:?}");
        assert!(a[1].abs() > 1e-3, "…it must move y: {a:?}");

        // Turn a quarter turn and the SAME drag has to move the other axis.
        let mut s = LightState::new();
        s.cam_target = [0.0, 0.0, 0.0];
        s.cam_pitch = 0.0;
        s.cam_dist = 10.0;
        s.cam_yaw = std::f32::consts::FRAC_PI_2;
        s.pan(100.0, 0.0);
        let b = s.cam_target;
        assert!(b[1].abs() < 1e-4, "at yaw 90° the same drag must not move y: {b:?}");
        assert!(b[0].abs() > 1e-3, "…it must move x: {b:?}");
    }

    /// It has to keep up with the cursor: the same drag covers more ground zoomed out.
    #[test]
    fn a_pan_scales_with_the_zoom() {
        let far = {
            let mut s = LightState::new();
            s.cam_target = [0.0; 3];
            s.cam_dist = 100.0;
            s.pan(50.0, 0.0);
            glam::Vec3::from(s.cam_target).length()
        };
        let near = {
            let mut s = LightState::new();
            s.cam_target = [0.0; 3];
            s.cam_dist = 5.0;
            s.pan(50.0, 0.0);
            glam::Vec3::from(s.cam_target).length()
        };
        assert!(far > near * 10.0, "zoomed out: {far}, zoomed in: {near}");
    }

    /// Every palette must span its full range and stay inside it.
    #[test]
    fn every_ramp_is_a_complete_scale() {
        for r in LuxRamp::ALL {
            let lo = r.color(0.0);
            let hi = r.color(1.0);
            assert_ne!(lo, hi, "{:?} has no range at all", r);
            // Clamped, not wrapped: out-of-range readings must not come back as a mid-scale colour.
            assert_eq!(r.color(-5.0), lo, "{:?} does not clamp below", r);
            assert_eq!(r.color(9.0), hi, "{:?} does not clamp above", r);
            // …and monotone in brightness, or "brighter patch" stops meaning "more light".
            let lum = |c: egui::Color32| c.r() as f32 * 0.299 + c.g() as f32 * 0.587 + c.b() as f32 * 0.114;
            assert!(lum(hi) > lum(lo), "{:?} runs dark at the top of the scale", r);
        }
    }

    /// The `fn` pointer form must agree with the colour form — they are two routes to one scale,
    /// and the 3D floor uses one while the legend beside it uses the other.
    #[test]
    fn the_heatmap_and_the_legend_read_the_same_scale() {
        for r in LuxRamp::ALL {
            let f = r.rgb_fn();
            for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let (a, b, c) = f(t);
                let want = r.color(t);
                assert!((a - want.r() as f32 / 255.0).abs() < 1e-6, "{r:?} at {t}");
                assert!((b - want.g() as f32 / 255.0).abs() < 1e-6, "{r:?} at {t}");
                assert!((c - want.b() as f32 / 255.0).abs() < 1e-6, "{r:?} at {t}");
            }
        }
    }

    /// HIDING THE CEILING MUST NOT CHANGE THE ANSWER. It is a view option, and the ceiling is
    /// 70 % of the interreflection — if the calculation stopped seeing it, the room would read
    /// far darker and nothing on screen would say why.
    #[test]
    fn hiding_the_ceiling_is_a_view_option_only() {
        let src = include_str!("app.rs");
        let a = src.find("fn build_scene3d_verts").expect("the SIMLUX vertex builder");
        let b = src[a..].find("\n    /// SIMLUX 3D viewport").map(|e| a + e).unwrap_or(src.len());
        let body = &src[a..b];
        assert!(body.contains("hide_ceilings"), "the view filters the ceiling out");
        // The filter has to be on a COPY for drawing. If `self.light.meshes` itself were pruned,
        // the next Calculate would run on a room with no ceiling.
        assert!(
            !body.contains("self.light.meshes.retain") && !body.contains("meshes.retain"),
            "the scene meshes must not be pruned in place — Calculate reads them",
        );
    }
}
