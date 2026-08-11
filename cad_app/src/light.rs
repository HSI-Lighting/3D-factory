//! SIMLUX lighting integration for the CAD app.
//!
//! [`LightState`] holds the lighting scene (IES profiles, surface materials,
//! luminaires, room height, ray settings) and the last computed lux grid, and
//! draws the **Light** panel. It drives the pure-Rust `cad_light` engine on the
//! shared `cad_kernel::Document`; the app paints the resulting grid as a 2D
//! false-colour overlay on the plan (see `CadApp::paint_lux_overlay`).

use std::collections::HashMap;

use cad_light::{
    bbox, calculate as calc_lux, default_materials, extrude, extrude_handles, parse_ies, parse_ldt,
    CalcPlane,
    IesProfile, LuxGrid, Luminaire, Material, Mesh, PhotometryType, RaySettings, Vertex,
};
use cad_kernel::Document;

/// Key for the always-available synthetic luminaire (works before any IES import).
pub const BUILTIN: &str = "Built-in downlight (1000 cd)";

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
    if pos.len() < 3 {
        return Vec::new();
    }
    // One bucket per material, so the engine sees three meshes and not thousands.
    let mut buckets: [Vec<Vertex>; 3] = [Vec::new(), Vec::new(), Vec::new()];
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
    /// Placed luminaires (P4); empty ⇒ auto-place one at room centre.
    pub luminaires: Vec<Luminaire>,
    pub auto_center_light: bool,
    /// When set, canvas clicks drop a luminaire (P4 placement mode).
    pub place_mode: bool,
    /// Monotonic id source for placed luminaires.
    pub next_id: u32,
    /// Rows/columns for the ▼ Luminaires grid array — the usual way a room is lit.
    pub array_rows: u32,
    pub array_cols: u32,
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
            active_profile: BUILTIN.to_string(),
            materials: default_materials(),
            room_height: 3.0,
            room: Vec::new(),
            plane_height: 0.8,
            cell_size: 0.25,
            settings: RaySettings::default(),
            luminaires: Vec::new(),
            auto_center_light: true,
            place_mode: false,
            next_id: 1,
            array_rows: 3,
            array_cols: 4,
            mount_height: 3.0,
            grid: None,
            plane: None,
            meshes: Vec::new(),
            show_overlay: true,
            scale_max: None,
            ies_path: String::new(),
            last_msg: "Draw a room, set the height, then Calculate.".to_string(),
            view3d_open: false,
            simlux_mode: false,
            simlux_fit_pending: false,
            cam_yaw: 0.7,
            cam_pitch: 0.6,
            cam_dist: 10.0,
            cam_target: [0.0, 0.0, 1.5],
            floor_heatmap: true,
        }
    }

    /// Colour-map ceiling: user override, else the current grid's max.
    pub fn scale_ceiling(&self) -> f64 {
        self.scale_max
            .or_else(|| self.grid.as_ref().map(|g| g.max))
            .unwrap_or(1.0)
            .max(1e-3)
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
        if path.is_empty() {
            self.last_msg = "Enter a .ies or .ldt file path first.".to_string();
            return;
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.last_msg = format!("Read error: {e}");
                return;
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
                self.last_msg = format!(
                    "Loaded {kind} '{key}' — {:.0} lm, {:.0} W, peak {:.0} cd",
                    prof.lumens.max(0.0),
                    prof.watts,
                    prof.peak_candela(),
                );
                self.active_profile = key.clone();
                self.profiles.insert(key, prof);
            }
            Err(e) => self.last_msg = format!("{kind} parse error: {e}"),
        }
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
        for r in 0..rows {
            for c in 0..cols {
                let x = x0 + dx * (c as f32 + 0.5);
                let y = y0 + dy * (r as f32 + 0.5);
                let id = self.next_id;
                self.next_id += 1;
                self.luminaires.push(Luminaire {
                    id,
                    profile: self.active_profile.clone(),
                    position: Vertex::new(x, y, self.mount_height),
                    rotation_deg: 0.0,
                    dimming: 1.0,
                });
                n += 1;
            }
        }
        self.last_msg = format!(
            "Placed {n} fixtures ({rows}x{cols}) at {:.2} m, {:.2} x {:.2} m pitch — press Calculate.",
            self.mount_height, dx, dy
        );
        n
    }

    /// Plan-view bounds of the current lighting geometry, for laying out an array.
    pub fn room_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        mesh_bbox(&self.meshes)
    }

    pub fn add_luminaire_at(&mut self, x: f32, y: f32) {
        let id = self.next_id;
        self.next_id += 1;
        self.luminaires.push(Luminaire {
            id,
            profile: self.active_profile.clone(),
            position: Vertex::new(x, y, self.mount_height),
            rotation_deg: 0.0,
            dimming: 1.0,
        });
        self.last_msg = format!("Placed fixture #{id} at ({x:.2}, {y:.2}) — press Calculate.");
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
        let lums = if self.luminaires.is_empty() && self.auto_center_light {
            vec![Luminaire {
                id: 1,
                profile: self.active_profile.clone(),
                position: Vertex::new(0.5 * (min_x + max_x), 0.5 * (min_y + max_y), self.room_height),
                rotation_deg: 0.0,
                dimming: 1.0,
            }]
        } else {
            self.luminaires.clone()
        };
        let grid = calc_lux(&meshes, &lums, &self.profiles, &self.materials, &plane, &self.settings);
        self.last_msg = format!(
            "{}×{} grid · avg {:.0} · min {:.0} · max {:.0} lx",
            cols, rows, grid.avg, grid.min, grid.max
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
        }
    }

    /// Apply a loaded sidecar config onto the current document — merge the IES
    /// library, restore materials/settings/defaults, and rebuild the room by
    /// resolving persisted layer NAMES back to ids + their current handles.
    pub fn apply_config(&mut self, cfg: crate::simlux_io::SimluxConfig, doc: &Document) {
        for (k, v) in cfg.ies_library {
            self.profiles.insert(k, v);
        }
        if self.profiles.contains_key(&cfg.active_profile) {
            self.active_profile = cfg.active_profile;
        }
        if !cfg.materials.is_empty() {
            self.materials = cfg.materials;
        }
        self.settings = cfg.settings;
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
    /// Grouped by the QUESTION being answered, not by the code behind it:
    ///   Luminaires — what is emitting, and where
    ///   Photometry — which real fitting it is
    ///   Calculation — how the answer is worked out
    ///   Surfaces — what the room is made of
    ///   Display — how the result is drawn
    pub fn toolbar_ui(&mut self, ui: &mut egui::Ui) -> LightAction {
        let mut action = LightAction::default();
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("▼ Luminaires", |ui| {
                ui.label(egui::RichText::new("place").small().weak());
                let placing = self.place_mode;
                if ui
                    .selectable_label(placing, if placing { "◉ Placing — click the plan" } else { "＋ Place one (click the plan)" })
                    .on_hover_text("Then click in the 2D plan to drop a fixture at the mounting height")
                    .clicked()
                {
                    self.place_mode = !placing;
                    ui.close_menu();
                }
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
                ui.horizontal(|ui| {
                    ui.label("mount at");
                    ui.add(egui::DragValue::new(&mut self.mount_height).speed(0.05).suffix(" m").range(0.1..=30.0));
                });
                ui.checkbox(&mut self.auto_center_light, "auto-place one at the centre if none")
                    .on_hover_text("A convenience for a first look; turn it off once you place fixtures yourself");
                ui.separator();
                if ui.button("🗑  Remove all fixtures").clicked() {
                    let n = self.luminaires.len();
                    self.luminaires.clear();
                    self.last_msg = format!("Removed {n} fixture(s).");
                    ui.close_menu();
                }
            });

            ui.menu_button("▼ Photometry", |ui| {
                ui.label(egui::RichText::new("IES (.ies) or EULUMDAT (.ldt)").small().weak());
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.ies_path)
                            .desired_width(240.0)
                            .hint_text(r"C:\path\to\fitting.ldt"),
                    );
                    if ui.button("Load").clicked() {
                        self.import_photometry();
                    }
                });
                ui.separator();
                ui.label(egui::RichText::new("loaded — click to make active").small().weak());
                // Sorted, because a HashMap would reorder the list on every repaint and the entry
                // under the cursor would not be the one that gets clicked.
                let mut names: Vec<String> = self.profiles.keys().cloned().collect();
                names.sort();
                for n in names {
                    let active = n == self.active_profile;
                    let detail = self.profiles.get(&n).map(|p| {
                        if p.lumens > 0.0 {
                            format!("{:.0} lm · {:.0} W · peak {:.0} cd", p.lumens, p.watts, p.peak_candela())
                        } else {
                            format!("peak {:.0} cd", p.peak_candela())
                        }
                    });
                    if ui.selectable_label(active, &n).on_hover_text(detail.unwrap_or_default()).clicked() {
                        self.active_profile = n.clone();
                        ui.close_menu();
                    }
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
                    ui.label("bounces").on_hover_text("Indirect light: 0 is direct only, which under-reads a bright room badly");
                    ui.add(egui::DragValue::new(&mut self.settings.max_bounces).range(0..=8));
                    ui.end_row();
                    ui.label("rays").on_hover_text("Samples per point for the indirect term — more is smoother and slower");
                    ui.add(egui::DragValue::new(&mut self.settings.rays_per_point).range(1..=4096));
                    ui.end_row();
                });
            });

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
                ui.checkbox(&mut self.floor_heatmap, "false-colour on the floor");
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
            ui.label(small(format!("· {}", self.active_profile)));
            if let Some(g) = self.grid.as_ref() {
                ui.label(small(format!("· avg {:.0} lx", g.avg)));
                ui.label(small(format!("· min {:.0}", g.min)));
                ui.label(small(format!("· max {:.0}", g.max)));
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
            ui.label("IES:");
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
            .on_hover_text("Toggle, then click points on the 2D plan to drop fixtures. Esc / untoggle to stop.")
            .clicked()
        {
            self.place_mode = !self.place_mode;
        }
        ui.add(egui::Slider::new(&mut self.mount_height, 0.0..=8.0).text("Mount height (m)"));
        if !self.luminaires.is_empty() {
            let mut remove: Option<usize> = None;
            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for (i, l) in self.luminaires.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("#{}  ({:.1}, {:.1}, {:.1})", l.id, l.position.x, l.position.y, l.position.z));
                        if ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                        ui.add(egui::Slider::new(&mut l.dimming, 0.0..=1.0).text("dim"));
                    });
                }
            });
            if let Some(i) = remove {
                self.luminaires.remove(i);
            }
            if ui.button("Clear all fixtures").clicked() {
                self.luminaires.clear();
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
            let uo = if g.avg > 0.0 { g.min / g.avg } else { 0.0 };
            ui.label(format!("Average   {:.0} lx", g.avg));
            ui.label(format!("Min / Max   {:.0} / {:.0} lx", g.min, g.max));
            ui.label(format!("Uniformity Uo (min/avg)   {:.2}", uo));
            legend_bar(ui, self.scale_ceiling());
        }

        ui.add_space(4.0);
        ui.label(egui::RichText::new(&self.last_msg).small().italics());
        action
    }
}

/// Five-stop false-colour ramp (low→high). `t` is clamped to 0..1.
pub fn lux_color(t: f32) -> egui::Color32 {
    const STOPS: [(f32, [u8; 3]); 5] = [
        (0.00, [20, 24, 82]),    // deep blue
        (0.25, [34, 116, 204]),  // blue
        (0.50, [40, 190, 120]),  // green
        (0.75, [240, 214, 72]),  // yellow
        (1.00, [226, 72, 46]),   // red
    ];
    let t = t.clamp(0.0, 1.0);
    let (mut lo, mut hi) = (STOPS[0], STOPS[STOPS.len() - 1]);
    for w in STOPS.windows(2) {
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

/// The same false-colour ramp as [`lux_color`], as float RGB (0..1) for the
/// 3D floor heatmap. `fn(f32) -> (f32, f32, f32)` so it can be passed as a
/// plain function pointer into the 3D vertex builder.
pub fn lux_rgb(t: f32) -> (f32, f32, f32) {
    let c = lux_color(t);
    (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0)
}

/// A horizontal gradient legend from 0 to `max` lux.
pub fn legend_bar(ui: &mut egui::Ui, max: f64) {
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
            lux_color(t),
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
