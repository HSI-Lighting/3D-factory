//! SIMLUX sidecar persistence — `<drawing>.simlux.json` beside the `.rsm`/`.dxf`.
//!
//! All SIMLUX-specific state (which layers extrude in 3D + their heights, the
//! load-once IES library, the LUX-block→IES map, materials, ray settings) lives
//! here, NOT in the (2D) `cad_kernel` document. Keyed by STABLE NAMES (layer
//! name, profile name, block-def name) so it survives save/reopen even though
//! layer/block ids are positional. `cad_kernel` / `cad_io` stay UNTOUCHED
//! (decision D5, SIMLUX_LUX_WORKFLOW.md).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cad_light::{IesProfile, Material, RaySettings};
use serde::{Deserialize, Serialize};

/// One promoted "alive wall" as it survives a save. Mirrors `factory::WallInst`, but
/// footprint points are `[f32; 2]` rather than `glam::Vec2`: glam is built WITHOUT its
/// `serde` feature here, and the on-disk shape should not track a maths library's
/// representation anyway. `segments` holds the owning Box feature ids, so a restored wall
/// stays linked to the features it built.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WallRec {
    pub footprint: Vec<[f32; 2]>,
    pub segments: Vec<u32>,
    pub thickness: f32,
    pub height: f32,
    pub rake_deg: f32,
    /// Z the wall stands on. `0.0` for a sidecar written before storeys existed, which is
    /// exactly right — everything back then stood on the ground.
    #[serde(default)]
    pub base_z: f32,
}

/// One building level. Mirrors `factory::Storey`. `base_z` is NOT stored — it is derived
/// by summing the heights below, so the stack cannot be loaded non-contiguous.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreyRec {
    pub name: String,
    pub height: f32,
}

/// The 3D Factory model as persisted. Without this a building could be modelled and then
/// lost on close — nothing else wrote `factory.model` anywhere.
///
/// `cad_solid::Model` is stored DIRECTLY (it already derives `Serialize`/`Deserialize`
/// for exactly this purpose, and its `sketches` field is `#[serde(skip)]`, so a live
/// sketch session is deliberately not persisted). No `Debug` derive — `Model` has none,
/// because `cad_kernel::Document` on `Sketch` is not `Debug`.
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct FactoryDoc {
    #[serde(default)]
    pub model: cad_solid::Model,
    #[serde(default)]
    pub walls: Vec<WallRec>,
    /// Height promoted walls rise to. `0.0` = absent (an older sidecar) — the loader
    /// keeps its current default rather than adopting a zero-height building.
    #[serde(default)]
    pub wall_height: f32,
    #[serde(default)]
    pub building_height: f32,
    /// The building's levels, bottom-up. EMPTY for a pre-storeys sidecar — the loader
    /// substitutes the single ground storey rather than a building with no levels.
    #[serde(default)]
    pub storeys: Vec<StoreyRec>,
    #[serde(default)]
    pub active_storey: usize,
    /// Feature ids that are room ceilings (separate slab objects). Persisted so the
    /// Hide-ceilings toggle still knows which features are ceilings after a reload.
    #[serde(default)]
    pub ceilings: Vec<u32>,
    /// Imported furniture MESHES — stored in the project so they can be reused.
    #[serde(default)]
    pub furniture_lib: Vec<FurnitureAssetRec>,
    /// Placed furniture instances.
    #[serde(default)]
    pub furniture: Vec<FurnitureInstRec>,
    /// Per-feature colour: `[feature_id, r, g, b]`.
    #[serde(default)]
    pub feature_colors: Vec<(u32, [f32; 3])>,
    /// Per-surface colour: `(feature_id, nx, ny, nz, offset, rgb)` — a flat list because
    /// the surface key is a tuple and JSON map keys must be strings.
    #[serde(default)]
    pub surface_colors: Vec<(u32, i32, i32, i32, i32, [f32; 3])>,
}

/// A furniture mesh as persisted (triangle soup). Mirrors `factory::FurnitureAsset`.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct FurnitureAssetRec {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    #[serde(default)]
    pub color: [f32; 3],
}

/// A placed furniture instance. Mirrors `factory::FurnitureInst`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default)]
pub struct FurnitureInstRec {
    pub asset: usize,
    pub pos: [f32; 3],
    pub scale: f32,
    /// Yaw about Z, degrees (kept for back-compat: old sidecars had only this).
    pub rot_deg: f32,
    /// Tilt about world X and Y, degrees. `#[serde(default)]` so old sidecars (which have
    /// only `rot_deg`) still load — they read back as [0,0] = upright.
    #[serde(default)]
    pub rot_xy: [f32; 2],
    pub color: [f32; 3],
}

impl FactoryDoc {
    /// Nothing worth writing — keeps an empty `factory` block out of the sidecar of a
    /// drawing that has no 3D model.
    pub fn is_empty(&self) -> bool {
        self.model.features.is_empty() && self.walls.is_empty()
    }
}

/// Everything SIMLUX persists next to a drawing.
///
/// (No `Debug` derive — it now carries a [`FactoryDoc`], and `cad_solid::Model` has no
/// `Debug`. Nothing formatted this struct.)
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct SimluxConfig {
    /// Layer NAME → extrude height (m). Presence ⇒ "use for 3D".
    #[serde(default)]
    pub layers_3d: BTreeMap<String, f32>,
    /// IES library — profile name → profile. Entered ONCE, referenced by name.
    #[serde(default)]
    pub ies_library: BTreeMap<String, IesProfile>,
    /// Selected / active IES profile name.
    #[serde(default)]
    pub active_profile: String,
    /// LUX block DEFINITION name → IES profile name (Slice 3; type-level D4).
    #[serde(default)]
    pub lux_block_ies: BTreeMap<String, String>,
    /// Surface materials [floor, wall, ceiling].
    #[serde(default)]
    pub materials: Vec<Material>,
    /// Ray-tracer controls.
    #[serde(default)]
    pub settings: RaySettings,
    /// Default room height + work-plane height + grid cell size (metres).
    #[serde(default)]
    pub room_height: f32,
    #[serde(default)]
    pub plane_height: f32,
    #[serde(default)]
    pub cell_size: f32,
    /// APP-LAYER wall-style extension: wall-style NAME → centerline linetype NAME.
    /// Keyed by name (like everything else here) so it survives save/reopen even though
    /// style/linetype ids are positional. Kept out of cad_kernel's `WallStyle` (D5).
    #[serde(default)]
    pub wall_centerline: BTreeMap<String, String>,
    /// The 3D Factory model (solids + alive walls). `#[serde(default)]` so every sidecar
    /// written before this existed still loads, as an empty factory.
    #[serde(default)]
    pub factory: FactoryDoc,
}

/// The sidecar path for a drawing: `foo.rsm` → `foo.simlux.json`.
pub fn sidecar_path(drawing: &Path) -> PathBuf {
    drawing.with_extension("simlux.json")
}

/// Read the sidecar for `drawing`, if present. `Ok(None)` = no sidecar there.
pub fn load(drawing: &Path) -> Result<Option<SimluxConfig>, String> {
    let p = sidecar_path(drawing);
    if !p.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let cfg: SimluxConfig = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(Some(cfg))
}

/// Write the sidecar for `drawing`. Returns the path written.
pub fn save(drawing: &Path, cfg: &SimluxConfig) -> Result<PathBuf, String> {
    let p = sidecar_path(drawing);
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| e.to_string())?;
    Ok(p)
}
