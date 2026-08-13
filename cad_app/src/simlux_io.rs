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

/// One ROOM as it survives a save. Mirrors `factory::RoomInst`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoomRec {
    pub id: u32,
    pub name: String,
    pub footprint: Vec<[f32; 2]>,
    pub base_z: f32,
    /// CLEAR height — floor top to ceiling underside. The slabs are additional.
    pub height: f32,
    pub floor_t: f32,
    pub ceiling_t: f32,
    pub wall_t: f32,
    pub open_top: bool,
    /// Feature ids the room owns. A feature that no longer exists is dropped on load rather than
    /// leaving the room pointing at geometry that is not there.
    pub floor: Option<u32>,
    pub walls: Vec<u32>,
    pub ceiling: Option<u32>,
    /// The Difference feature that carved this room out of a building, when it was carved from
    /// one. Removing it on delete is what returns the building to solid.
    #[serde(default)]
    pub carve: Option<u32>,
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
    /// Clipboard TEXTURES pasted onto objects. Stored PNG-compressed + base64 so a screenshot
    /// doesn't bloat the sidecar into megabytes of raw pixels.
    #[serde(default)]
    pub textures: Vec<TextureRec>,
    /// Per-feature texture assignment: `(feature_id, texture_index)`.
    #[serde(default)]
    pub feature_textures: Vec<(u32, usize)>,
    /// Per-surface texture: `(feature_id, nx, ny, nz, offset, texture_index)` — flat because
    /// the surface key is a tuple. Lets each wall face carry its own image.
    #[serde(default)]
    pub surface_textures: Vec<(u32, i32, i32, i32, i32, usize)>,
    /// GROUPS of CSG features: `(feature_id, group_id)` — members select/move/delete as one.
    #[serde(default)]
    pub feature_groups: Vec<(u32, u32)>,
    /// Every ROOM, as an editable record. Mirrors `factory::RoomInst`; footprint points are
    /// `[f32; 2]` for the same reason `WallRec` uses them — glam is built without serde here, and
    /// the on-disk shape should not track a maths library's representation.
    ///
    /// Without this a reopened project has rooms as loose geometry again: no names, no heights to
    /// adjust, and openings that cannot say which room they are in.
    #[serde(default)]
    pub rooms: Vec<RoomRec>,
    /// Next free room id, so reopening does not restart numbering and hand two rooms the same id.
    #[serde(default)]
    pub next_room_id: u32,
    /// The Factory's WORKING UNIT — metres per unit, e.g. `0.001` for millimetres.
    ///
    /// A display and input preference, not a property of the geometry: the model is stored in
    /// metres whatever this says. Persisted so a project reopens showing the numbers it was
    /// authored with. `0.0` (an older sidecar, or a missing field) means "not recorded" and the
    /// loader keeps its own default rather than reinterpreting anything.
    #[serde(default)]
    pub working_unit_m: f64,
    /// WHERE newly added objects land — see `factory::PlaceMode`. A working preference like the
    /// unit above, persisted so a project reopens behaving the way it was left. `None` (an older
    /// file) means "not recorded" and the loader keeps its own default.
    #[serde(default)]
    pub place_mode: Option<crate::factory::PlaceMode>,
    /// The distance from the origin used by `PlaceMode::Offset`, in METRES.
    #[serde(default)]
    pub place_offset: Option<[f32; 3]>,
    /// FACE PLANES and the drawings on them — see [`SketchRec`].
    #[serde(default)]
    pub sketches: Vec<SketchRec>,
    /// Has this project been asked what unit it is built in? Absent in a file written before the
    /// question existed — and such a file already declares `working_unit_m`, which IS an answer,
    /// so the loader treats a recorded unit as consent and does not re-ask.
    #[serde(default)]
    pub unit_asked: Option<bool>,
}

/// One face plane as persisted: where it stands, what it is called, and the drawing on it.
///
/// `cad_solid::Model::sketches` is `#[serde(skip)]` and always was, so face sketches lived only as
/// long as the app was open. That was survivable while a plane was something you re-picked off the
/// model every time; it is not survivable now that planes are a NAMED LIST you go back to — a view
/// called "Kitchen elevation" that is gone after a save is worse than no list at all.
///
/// The drawing is stored as RSM, the app's own 2D format, rather than a bespoke serialization of
/// `cad_kernel::Document` (which derives no serde at all). That reuses the reader and writer the
/// file menu already depends on, so a sketch cannot drift into a format nothing else can read.
#[derive(Clone, Serialize, Deserialize, Default)]
pub struct SketchRec {
    /// What the plane is called in the 2D view list.
    pub name: String,
    /// The frame: origin and the two in-plane axes. The normal is `u × v` and is not stored.
    pub origin: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    /// The drawing, as base64 of the RSM bytes.
    pub rsm_b64: String,
}

/// One pasted texture as persisted: PNG bytes, base64-encoded. Mirrors `factory::TextureAsset`.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct TextureRec {
    pub name: String,
    pub w: u32,
    pub h: u32,
    #[serde(default = "one")]
    pub scale: f32,
    /// Texture MOVE (UV offset) and ROTATE (degrees). `#[serde(default)]` so older sidecars load.
    #[serde(default)]
    pub offset: [f32; 2],
    #[serde(default)]
    pub rot_deg: f32,
    /// Surface OPACITY (1.0 = opaque) and REFLECTION (1.0 = the full physical amount).
    /// `#[serde(default = "one")]` on both, so a sidecar predating either field loads as an
    /// ordinary opaque surface that reflects its surroundings — which is what a material with
    /// nothing said about it should do. (`decode_texture_rec` also reads a stored 0 as 1.0, for
    /// the sidecars written while 0 was still the default.)
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default = "one")]
    pub reflect: f32,
    /// PNG-encoded pixels, base64. Decoded back to RGBA8 on load. For a procedural texture this is
    /// just a 1×1 fallback swatch; the real definition lives in `proc`.
    pub png_b64: String,
    /// PROCEDURAL material definition — `Some` for a shader-evaluated pattern (wood/marble/…),
    /// `None` (the default, so older sidecars load) for a plain pasted image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc: Option<ProcRec>,
    /// Texture Phase 2 — PBR maps: normal/roughness map texture indices + scalar roughness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal_map: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rough_map: Option<usize>,
    #[serde(default = "half")]
    pub roughness: f32,
    /// Principled BSDF params from the Materials Factory node editor. `#[serde(default*)]` so older
    /// sidecars load as non-metallic, IOR 1.5, no emission (the previous plastic look).
    #[serde(default)]
    pub metallic: f32,
    #[serde(default = "ior_default")]
    pub ior: f32,
    #[serde(default)]
    pub emission: [f32; 3],
    #[serde(default)]
    pub emission_strength: f32,
    /// This material is a MEDIUM light travels through (water, a glass block) rather than a surface
    /// with holes. `#[serde(default)]` = 0, which is what every material predating it was.
    ///
    /// Persisted because it is not derivable from anything else here: opacity says HOW MUCH light
    /// gets past, and this says whether what gets past is filtered by the material on its way. A
    /// water material that reloaded without it would come back as tinted glazing over the pool
    /// floor instead of water in a pool.
    #[serde(default)]
    pub transmission: f32,
}

fn half() -> f32 {
    0.5
}

fn ior_default() -> f32 {
    1.5
}

/// A procedural material as persisted. `pattern` is a stable tag ("wood"|"marble"|"noise"|"checker").
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ProcRec {
    pub pattern: String,
    pub col_a: [f32; 3],
    pub col_b: [f32; 3],
    pub scale: [f32; 3],
    pub detail: f32,
    pub rough: f32,
    pub contrast: f32,
    pub ramp: [f32; 2],
    /// Phase 3 — the pattern's own SURFACE finish (roughness at its two ends) and its relief.
    /// `#[serde(default*)]` so older sidecars load with an EQUAL pair, which the renderer reads as
    /// "this pattern does not vary its finish, use the material's scalar roughness" — i.e. exactly
    /// the look those projects already had.
    #[serde(default = "half2")]
    pub surf_rough: [f32; 2],
    #[serde(default)]
    pub bump: f32,
}

fn one() -> f32 { 1.0 }

fn half2() -> [f32; 2] {
    [0.5, 0.5]
}

/// A furniture mesh as persisted (triangle soup). Mirrors `factory::FurnitureAsset`.
///
/// Geometry is stored COMPACTLY as base64 of deflated little-endian f32 bytes (`*_b64`). The
/// old JSON `Vec<[f32;3]>` fields remain only to READ pre-existing sidecars — writing millions
/// of vertices as JSON numbers made a heavy mesh take ~20s to load/save.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct FurnitureAssetRec {
    pub name: String,
    /// LEGACY JSON geometry (old sidecars). New writes leave these empty and use `*_b64`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positions: Vec<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub normals: Vec<[f32; 3]>,
    #[serde(default)]
    pub color: [f32; 3],
    /// LEGACY JSON UVs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uvs: Vec<[f32; 2]>,
    /// Compact geometry: base64(deflate(LE f32 bytes)). `pos`/`nrm` are xyz triples, `uv` pairs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pos_b64: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nrm_b64: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uv_b64: String,
    /// Compact PER-VERTEX OPACITY: base64(deflate(LE f32)). EMPTY for a fully-opaque asset (the
    /// common case) and for sidecars written before transparency existed — both load as opaque.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alpha_b64: String,
    /// Absolute path the mesh came from, so a stale project can re-derive transparency on load.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_path: String,
    /// True once alpha was resolved from the source (avoids a repeat on-load re-parse).
    #[serde(default)]
    pub alpha_resolved: bool,
    /// Per-TRIANGLE part id for GENERATED objects (each tread/riser/baluster) so a reloaded
    /// staircase keeps its per-piece selection. Empty for imported meshes. Small (one u32/tri).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub part_ids: Vec<u32>,
}

/// A placed furniture instance. Mirrors `factory::FurnitureInst`.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
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
    /// Index into `FactoryDoc::textures` when a clipboard texture is applied to this piece.
    #[serde(default)]
    pub texture: Option<usize>,
    /// Non-uniform local scale for an APERTURE (door/window stretched to fill an opening).
    /// `None`/absent for ordinary furniture. `#[serde(default)]` so old sidecars still load.
    #[serde(default)]
    pub fit: Option<[f32; 3]>,
    /// PER-SURFACE textures: `(face_group_id, texture_index)` pairs (flattened `FurnitureInst::
    /// surface_texture`). `#[serde(default)]` so old sidecars load as whole-object only.
    #[serde(default)]
    pub surface_texture: Vec<(u32, usize)>,
    /// CUTS drawn on this piece's faces. The LIST is saved, never the cut geometry — the mesh is
    /// rebuilt from the original on load, so a reopened project keeps every cut editable and
    /// removable exactly as it was, and a later fix to the boolean improves old files too.
    /// `#[serde(default)]` so older sidecars load with none.
    #[serde(default)]
    pub cuts: Vec<MeshCutRec>,
    /// The UNCUT original when `asset` points at a derived (cut) copy. Saved so a reload reuses
    /// that derived slot instead of minting a new one, which would otherwise leave a dead "(cut)"
    /// entry in the library on every save/open cycle.
    #[serde(default)]
    pub base_asset: Option<usize>,
}

/// One cut, flattened for the sidecar. Mirrors `cad_solid::meshcut::MeshCut`; the frame is written
/// out as its origin and two axes, plainly, so a migrated or hand-edited file stays readable.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct MeshCutRec {
    pub profile: Vec<[f32; 2]>,
    pub origin: [f32; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    pub depth: f32,
    pub through: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub label: String,
}

/// `true` — so a cut written before `enabled` existed reads back as applied, which is what it was.
fn yes() -> bool {
    true
}

impl MeshCutRec {
    pub fn of(c: &cad_solid::meshcut::MeshCut) -> Self {
        Self {
            profile: c.profile.clone(),
            origin: c.frame.origin.to_array(),
            u: c.frame.u.to_array(),
            v: c.frame.v.to_array(),
            depth: c.depth,
            through: c.through,
            enabled: c.enabled,
            label: c.label.clone(),
        }
    }

    pub fn to_cut(&self) -> cad_solid::meshcut::MeshCut {
        cad_solid::meshcut::MeshCut {
            profile: self.profile.clone(),
            frame: cad_solid::Frame {
                origin: glam::Vec3::from(self.origin),
                u: glam::Vec3::from(self.u),
                v: glam::Vec3::from(self.v),
            },
            depth: self.depth,
            through: self.through,
            enabled: self.enabled,
            label: self.label.clone(),
        }
    }
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
    /// PLACED LUMINAIRES — the lighting layout itself.
    ///
    /// These were not persisted at all, so a scheme laid out over a whole building was gone the
    /// moment the project was reopened, with nothing on screen to say it had happened. `profile`
    /// is a library key, so the fittings above resolve them by name on load; a fixture whose
    /// fitting is missing comes back UNASSIGNED rather than pointing at nothing.
    #[serde(default)]
    pub luminaires: Vec<cad_light::Luminaire>,
    /// Next free fixture id, so reopening a project does not restart numbering at #1 and hand two
    /// fixtures the same id.
    #[serde(default)]
    pub next_luminaire_id: u32,
    /// The MAINTENANCE FACTOR the results are quoted at. `None` in a sidecar written before this
    /// existed — those projects were calculated at the initial condition, and are restored that
    /// way rather than silently acquiring a factor that would change every number in them.
    #[serde(default)]
    pub maintenance: Option<cad_light::Maintenance>,
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

/// Write the sidecar for `drawing`. Returns the path written. Written ATOMICALLY (temp + rename)
/// so an interrupted write (crash / close during autosave) can't corrupt the previous sidecar.
pub fn save(drawing: &Path, cfg: &SimluxConfig) -> Result<PathBuf, String> {
    let p = sidecar_path(drawing);
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.savetmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(p)
}
