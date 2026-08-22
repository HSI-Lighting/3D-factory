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
    meshes_from_factory_ex(f, None)
}

/// HOW MUCH OF THE MODEL THE ENGINE IS ASKED TO SEE.
///
/// The two differ in ONE thing: how furniture is represented. Same rays, same bounces, same grid,
/// same materials — so a run of each on one scene is a controlled comparison, and any difference in
/// the numbers is attributable to the box substitution and nothing else. Mixing in sample counts
/// would have made that comparison uninterpretable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum CalcMode {
    /// Furniture as the box it occupies. For iterating a layout.
    ///
    /// A box is MORE OCCLUDING than the thing it replaces — a chair is mostly air and its box is
    /// solid from floor to seat-back — so this reads low under and beside furniture, and its
    /// uniformity a little worse. That is the expected direction and it is why an Express result is
    /// labelled everywhere it appears and never carries a compliance claim.
    Express,
    /// Every triangle of every piece. The answer a report defends.
    #[default]
    Thorough,
}

impl CalcMode {
    pub fn label(self) -> &'static str {
        match self {
            CalcMode::Express => "Express",
            CalcMode::Thorough => "Thorough",
        }
    }
    /// Whether a result computed this way may be quoted as an EN 12464-1 compliance figure.
    pub fn is_compliant(self) -> bool {
        matches!(self, CalcMode::Thorough)
    }
}

/// ONE FURNITURE INSTANCE AS THE BOX IT OCCUPIES — 12 triangles instead of half a million.
///
/// The asset's cached LOCAL bounds carried through the instance's own transform, so a piece standing
/// at an angle to the grid gets a box that leans with it rather than an axis-aligned one inflated to
/// contain it. Same 12 triangles either way; the oriented one is the piece's actual footprint.
///
/// Wound outward, because the engine reads a triangle's normal and a box turned inside out would
/// bounce light the wrong way.
pub fn furniture_box_tris(
    f: &crate::factory::FactoryState,
    i: usize,
) -> Option<Vec<[glam::Vec3; 3]>> {
    let inst = f.furniture.get(i)?;
    let asset = f.furniture_lib.get(inst.asset)?;
    let m = glam::Mat4::from_cols_array(&f.furniture_model_matrix(i)?);
    let (lo, hi) = (asset.local_min, asset.local_max);
    // Corner k, with bit 0 = x, bit 1 = y, bit 2 = z.
    let c = |k: usize| -> glam::Vec3 {
        m.transform_point3(glam::Vec3::new(
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ))
    };
    // Six faces, each two triangles, every one wound counter-clockwise seen from OUTSIDE.
    //
    // INDICES ARE BIT PATTERNS, not a walk round the face. Corner 2 is +y and corner 3 is +x+y,
    // which is not the order a hand-drawn box diagram numbers them in — writing these out as though
    // it were put the two −Z triangles' normals along +Z, i.e. the box inside out. Caught by
    // `the_proxy_is_wound_outward`, which is why that test exists.
    const FACES: [[usize; 3]; 12] = [
        [0, 2, 3], [0, 3, 1], // −Z
        [4, 5, 7], [4, 7, 6], // +Z
        [0, 1, 5], [0, 5, 4], // −Y
        [2, 6, 7], [2, 7, 3], // +Y
        [0, 4, 6], [0, 6, 2], // −X
        [1, 3, 7], [1, 7, 5], // +X
    ];
    Some(FACES.iter().map(|t| [c(t[0]), c(t[1]), c(t[2])]).collect())
}

/// [`meshes_from_factory`], optionally with everything that LIDS THE ROOM left out — DRAWING only.
///
/// Reported twice: "hide ceiling in simlux doesnt work". The view filtered by MATERIAL, and material
/// here is assigned by ORIENTATION: a ceiling slab is a box, so its underside is material 2 and was
/// dropped while its TOP face is `n.z > 0.7` — material 0, *floor* — and stayed. Looking down at the
/// room you still saw a solid lid, so the toggle appeared to do nothing.
///
/// FILTERING BY FEATURE IS NOT ENOUGH EITHER, which the tests measured before this settled: on a
/// 10 × 8 m building with an 8 × 6 m room carved out of it, dropping the hidden-ceiling features
/// took the room's slab — 48 m² of 80 — and left the building's own roof, the 32 m² annulus around
/// it, because that roof belongs to the same feature as the WALLS and dropping the feature would
/// take the walls with it.
///
/// So the rule is geometric and says what it means: a triangle is part of the lid when it is
/// HORIZONTAL (`|n.z| > 0.7`, either way up, so a soffit goes with its slab) and sits ABOVE
/// `hide_above`. Nothing legitimate is horizontal between the working plane and a real ceiling
/// except furniture, which is bucketed separately and never filtered here. A mezzanine floor goes
/// too — which is what looking down from above means.
///
/// `hide_above` must only ever be `Some` for a DISPLAY build. The calculation has to keep the
/// ceiling: it is around 70 % of the interreflection, and a view option that changed the answer
/// would be a trap.
pub fn meshes_from_factory_ex(
    f: &crate::factory::FactoryState,
    hide_above: Option<f32>,
) -> Vec<Mesh> {
    meshes_from_factory_mode(f, hide_above, CalcMode::Thorough)
}

/// HOW MUCH OF EACH FURNITURE PIECE GOES INTO THE BUCKET.
///
/// The calculation and the SIMLUX viewport share this builder, and they do not want the same thing.
/// The calculation wants every triangle (or, in Express, a box). The VIEW wants something it can
/// afford to hold on the GPU.
///
/// That distinction was missing and it cost the SIMLUX view dearly: the viewport was built from the
/// CALCULATION's geometry, so on the gym plan it baked 7,036,129 triangles into one world-space
/// soup — 21,104,808 vertices, **844 MB**. Caching that stopped it being rebuilt every frame and
/// did nothing about its SIZE; parking 844 MB in a persistent GPU buffer is past what a card has
/// spare, and it spills over the bus instead. It is also why the symptom was SIMLUX-only: the 3D
/// Factory INSTANCES its furniture — one buffer per asset, twenty-six model matrices — where this
/// bakes every instance out in full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FurnitureDetail {
    /// Every triangle. What the calculation is entitled to.
    Full,
    /// The decimated display proxy for heavy assets — [`FurnitureAsset::lod_geom`]. For the VIEW
    /// only: it is the same geometry the 3D Factory already draws, so the two views agree.
    Proxy,
    /// One box per piece. [`CalcMode::Express`].
    Box,
}

/// [`meshes_from_factory_ex`], with furniture drawn as boxes under [`CalcMode::Express`].
pub fn meshes_from_factory_mode(
    f: &crate::factory::FactoryState,
    hide_above: Option<f32>,
    mode: CalcMode,
) -> Vec<Mesh> {
    let detail = match mode {
        CalcMode::Express => FurnitureDetail::Box,
        CalcMode::Thorough => FurnitureDetail::Full,
    };
    meshes_from_factory_detail(f, hide_above, detail)
}

/// The one builder, told explicitly how much of each furniture piece it may emit.
pub fn meshes_from_factory_detail(
    f: &crate::factory::FactoryState,
    hide_above: Option<f32>,
    detail: FurnitureDetail,
) -> Vec<Mesh> {
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
        if let Some(z) = hide_above {
            // Horizontal, and above the line the result is read on: this is the lid.
            if n.z.abs() > 0.7 && a.z.min(b.z).min(c.z) > z {
                continue;
            }
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
    //
    // EXPRESS SUBSTITUTES A BOX FOR EACH PIECE and changes nothing else — see [`CalcMode`]. On the
    // reference gym plan that is 26 boxes, 312 triangles, against 7,030,514: the BVH build and
    // `Obstacle::contains` both collapse, and the whole scene fits in cache instead of thrashing
    // 337 MB of DRAM. It is also the only representation in which `contains` is trustworthy, since
    // a box is watertight by construction and an imported FBX generally is not.
    for (i, inst) in f.furniture.iter().enumerate() {
        if detail == FurnitureDetail::Box {
            if let Some(tris) = furniture_box_tris(f, i) {
                for t in tris {
                    for p in t {
                        buckets[MATERIAL_FURNITURE as usize].push(Vertex::new(p.x, p.y, p.z));
                    }
                }
            }
            continue;
        }
        let Some(asset) = f.furniture_lib.get(inst.asset) else { continue };
        let Some(m) = f.furniture_model_matrix(i) else { continue };
        let m = glam::Mat4::from_cols_array(&m);
        // `Proxy` is the DISPLAY detail and takes the same decimated geometry the 3D Factory draws,
        // so the two views show the same thing. It is what stops this buffer being 844 MB: every
        // instance is baked out in world space here, so the full mesh is paid for twenty-six times
        // over, and there is nowhere for a shared per-asset buffer to help.
        let lod = (detail == FurnitureDetail::Proxy && asset.needs_lod())
            .then(|| asset.lod_geom());
        let src: &[[f32; 3]] = match &lod {
            Some(l) => &l.positions,
            None => &asset.positions,
        };
        for p in src {
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

/// A SOLID STANDING IN THE ROOM — a cupboard, a display case, a desk pedestal.
///
/// Reported as: *"our min lux was 0 while for relux it was 133… its an obvious error. find the root
/// cause."* The working plane is a flat rectangle at 0.8 m and a room has things standing in it, so
/// some of its points land INSIDE one of them. The engine answers those correctly — an enclosed
/// point receives nothing — but nobody measures illuminance inside a box. On the plan this was
/// reported against, 65 of 1140 cells were buried in the furniture: they took the room's minimum
/// from 102 lx to zero, and its uniformity with it.
///
/// ONE BODY AT A TIME, WHICH IS THE WHOLE POINT. Three earlier attempts tested containment against
/// the scene as a whole and all three failed, measurably: a ray-parity test excluded every cell of
/// an EMPTY room, because a room is itself a closed solid; counting nesting depth mis-fired because
/// a floor is a slab rather than a plane; and even a signed winding count read a point 0.8 m above
/// a 0.5 m table as buried, because the merged CSG output is not a clean manifold — a downward ray
/// crossed 3 surfaces from open floor, 8 from inside a box and 10 from above one.
///
/// A single BODY is a closed solid, and `SolidMesh::face_ids` says which body each triangle came
/// from. Parity against one body is exactly the textbook test and behaves like it.
pub struct Obstacle {
    tris: Vec<[glam::Vec3; 3]>,
    min: glam::Vec3,
    max: glam::Vec3,
}

impl Obstacle {
    /// How many triangles this body's parity test walks. `contains` is O(this) per cell inside the
    /// bounds, so it is the cost of the buried-cell test and worth being able to assert on.
    pub fn tri_count(&self) -> usize {
        self.tris.len()
    }

    /// Build one from world-space triangles — used by the tests and by `obstacles_in`.
    pub fn from_tris(tris: Vec<[glam::Vec3; 3]>) -> Self {
        let (mut mn, mut mx) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
        for t in &tris {
            for p in t {
                mn = mn.min(*p);
                mx = mx.max(*p);
            }
        }
        Obstacle { tris, min: mn, max: mx }
    }

    /// Whether this solid encloses `p`, by vertical ray parity against its own triangles.
    pub fn contains(&self, p: glam::Vec3) -> bool {
        // The bounds first: on a real plan most cells are nowhere near most objects, and this is
        // the difference between a few comparisons and a few thousand.
        if p.x < self.min.x
            || p.x > self.max.x
            || p.y < self.min.y
            || p.y > self.max.y
            || p.z < self.min.z
            || p.z > self.max.z
        {
            return false;
        }
        let mut above = 0usize;
        for t in &self.tris {
            let (a, b, c) = (t[0], t[1], t[2]);
            // DOES THE TRIANGLE COVER THIS POINT IN PLAN — by the CROSSING rule, not by testing
            // three barycentrics for being non-negative.
            //
            // The barycentric test accepts a point lying exactly ON an edge, and both triangles
            // sharing that edge accept it. A box's top face is two triangles meeting on a diagonal,
            // so a point at the centre of a square object was counted TWICE and read as outside.
            // That is not a corner case here: cell centres and box corners are both on regular
            // grids, so they land on each other constantly — it was the very first assertion this
            // met.
            //
            // The crossing rule has a half-open convention built in — an edge counts only where
            // `(y1 > y) != (y2 > y)` — so a shared edge is traversed in opposite directions by the
            // two triangles and is counted exactly once between them.
            let v = [a, b, c];
            let mut inside = false;
            for k in 0..3 {
                let (p1, p2) = (v[k], v[(k + 1) % 3]);
                if (p1.y > p.y) != (p2.y > p.y) {
                    let t = (p.y - p1.y) / (p2.y - p1.y);
                    if p1.x + t * (p2.x - p1.x) > p.x {
                        inside = !inside;
                    }
                }
            }
            if !inside {
                continue;
            }
            // Where this surface sits over the point. Barycentric is fine for the height, having
            // already established that the point is within the triangle.
            let d = (b.y - c.y) * (a.x - c.x) + (c.x - b.x) * (a.y - c.y);
            if d.abs() < 1e-12 {
                continue; // edge-on: no plan area, so nothing to be under
            }
            let bu = ((b.y - c.y) * (p.x - c.x) + (c.x - b.x) * (p.y - c.y)) / d;
            let bv = ((c.y - a.y) * (p.x - c.x) + (a.x - c.x) * (p.y - c.y)) / d;
            let z = bu * a.z + bv * b.z + (1.0 - bu - bv) * c.z;
            if z > p.z {
                above += 1;
            }
        }
        // A closed solid presents an odd number of surfaces above any point inside it.
        above % 2 == 1
    }
}

/// The solids standing INSIDE `room` — everything except the shell the room is carved from.
///
/// The shell is told apart by its footprint: a building contains its own rooms, and a thing
/// standing in a room does not. Anything whose plan bounds enclose the room's is therefore the
/// structure around it, and is not something a working-plane point can be "inside" in the sense
/// that matters.
///
/// Furniture goes in as well as CSG bodies, since a cupboard from the library buries a point just
/// as thoroughly as one built in the Factory. An imported mesh that is not closed simply counts
/// evenly and encloses nothing, which is the safe direction: it keeps a point that might be
/// measurable rather than discarding one that is.
pub fn obstacles_in(f: &crate::factory::FactoryState, room: &[glam::Vec2]) -> Vec<Obstacle> {
    obstacles_in_mode(f, room, CalcMode::Thorough)
}

/// [`obstacles_in`], with furniture as boxes under [`CalcMode::Express`].
pub fn obstacles_in_mode(
    f: &crate::factory::FactoryState,
    room: &[glam::Vec2],
    mode: CalcMode,
) -> Vec<Obstacle> {
    let mut out: Vec<Obstacle> = Vec::new();
    let mut push = |tris: Vec<[glam::Vec3; 3]>| {
        if !tris.is_empty() {
            out.push(Obstacle::from_tris(tris));
        }
    };

    // ---- CSG bodies, grouped by the feature each triangle belongs to -------------------------
    let m = &f.cached;
    let n = m.tri_count();
    if !m.face_ids.is_empty() && m.face_ids.len() >= n {
        let mut by_body: std::collections::BTreeMap<u32, Vec<[glam::Vec3; 3]>> = Default::default();
        for t in 0..n {
            let tri = [
                glam::Vec3::from(m.positions[t * 3]),
                glam::Vec3::from(m.positions[t * 3 + 1]),
                glam::Vec3::from(m.positions[t * 3 + 2]),
            ];
            by_body.entry(m.face_ids[t]).or_default().push(tri);
        }
        for (_, tris) in by_body {
            push(tris);
        }
    }

    // ---- furniture, in world coordinates -------------------------------------------------------
    //
    // EXPRESS IS THE MORE TRUSTWORTHY ONE HERE, which is the opposite of how a fast mode usually
    // reads. `Obstacle::contains` is ray parity against the body's own triangles and its own doc
    // says an unclosed mesh "counts evenly and encloses nothing" — an imported gym machine is not
    // watertight, so on the full mesh this test returns whatever the non-manifold surface happens
    // to produce. A box is closed by construction, and it costs 12 triangles per cell instead of
    // half a million.
    for (i, inst) in f.furniture.iter().enumerate() {
        if mode == CalcMode::Express {
            if let Some(tris) = furniture_box_tris(f, i) {
                push(tris);
            }
            continue;
        }
        let Some(asset) = f.furniture_lib.get(inst.asset) else { continue };
        let Some(mm) = f.furniture_model_matrix(i) else { continue };
        let mm = glam::Mat4::from_cols_array(&mm);
        let tris: Vec<[glam::Vec3; 3]> = asset
            .positions
            .chunks_exact(3)
            .map(|c| {
                [
                    mm.transform_point3(glam::Vec3::from(c[0])),
                    mm.transform_point3(glam::Vec3::from(c[1])),
                    mm.transform_point3(glam::Vec3::from(c[2])),
                ]
            })
            .collect();
        push(tris);
    }

    // ---- and drop the shell -------------------------------------------------------------------
    if room.len() >= 3 {
        let (mut rx0, mut ry0, mut rx1, mut ry1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for v in room {
            rx0 = rx0.min(v.x);
            ry0 = ry0.min(v.y);
            rx1 = rx1.max(v.x);
            ry1 = ry1.max(v.y);
        }
        // A hair of slack, so a body sitting exactly on the room's own boundary — the wall it was
        // carved from — is still recognised as the structure and not as a wardrobe.
        const SLACK: f32 = 1e-3;
        out.retain(|o| {
            !(o.min.x <= rx0 + SLACK
                && o.min.y <= ry0 + SLACK
                && o.max.x >= rx1 - SLACK
                && o.max.y >= ry1 - SLACK)
        });
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
        manufacturer: String::new(),
        catalogue: String::new(),
        lamp: String::new(),
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
        manufacturer: String::new(),
        catalogue: String::new(),
        lamp: String::new(),
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
    /// Delete this fixture. An ACTION rather than an edit in place, because a fixture placed from
    /// the Illuminaire library also has a block on the drawing — and only `CadApp` can reach that.
    /// Removing it here left the symbol behind, which is the bug the ✕ shares with everything else
    /// that used to delete a marker on its own.
    pub remove_fixture: Option<u32>,
    /// Delete every fixture, symbols included.
    pub clear_fixtures: bool,
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



/// How far a calculation has got, shared with whoever is watching it.
///
/// A LIGHTING CALCULATION TAKES MINUTES on a real building, and it ran on the UI thread — so the
/// window stopped repainting, Windows greyed it out and wrote "Not Responding" in the title bar,
/// and the only honest reading from outside was that the app had crashed. It is the same fault the
/// phase timings were added for, one level up: the work is fine, there was simply no way to see it
/// happening.
#[derive(Default)]
pub struct CalcProgress {
    pub done: std::sync::atomic::AtomicU32,
    pub total: std::sync::atomic::AtomicU32,
    pub phase: std::sync::Mutex<String>,
    /// Set by the UI to ask the worker to stop. Checked between rooms — a job you cannot stop is
    /// not much better than one that freezes the window.
    pub cancel: std::sync::atomic::AtomicBool,
    /// EVERY PHASE THE JOB ENTERED, in order.
    ///
    /// `phase` holds only the CURRENT one. That is all a progress bar needs and is not something
    /// anything else can rely on: a phase that begins and ends between two reads never existed as
    /// far as a reader is concerned. A test that POLLED for "did it reach the working plane?"
    /// therefore passed or failed on how fast the machine was — and one did, the day the engine
    /// grew a near-field correction and the phase timings moved.
    ///
    /// Worth having outside a test, too: this is the record of what a calculation actually did,
    /// for the run that took fifteen minutes and for the one that stopped early.
    pub log: std::sync::Mutex<Vec<String>>,
}

impl CalcProgress {
    fn step(&self, phase: &str) {
        use std::sync::atomic::Ordering;
        self.done.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut p) = self.phase.lock() {
            p.clear();
            p.push_str(phase);
        }
        if let Ok(mut l) = self.log.lock() {
            l.push(phase.to_string());
        }
    }

    /// Every phase entered so far — a handful of strings per calculation.
    pub fn phases(&self) -> Vec<String> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }
    pub fn fraction(&self) -> f32 {
        use std::sync::atomic::Ordering;
        let t = self.total.load(Ordering::Relaxed).max(1);
        (self.done.load(Ordering::Relaxed) as f32 / t as f32).clamp(0.0, 1.0)
    }
    pub fn label(&self) -> String {
        self.phase.lock().map(|p| p.clone()).unwrap_or_default()
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// FNV-1a, written out.
///
/// NOT `std::collections::hash_map::DefaultHasher`, whose algorithm is documented as unspecified
/// and free to change between Rust releases. That is fine for a hash map, which never outlives the
/// process, and wrong for a fingerprint written to disk: every saved result in every project would
/// read as out of date the first morning after a toolchain upgrade, with nothing on screen to say
/// why, and the only cure would be re-running calculations that were never actually stale.
///
/// Sixty-four bits, so two genuinely different scenes colliding — and a stale result being shown as
/// current — is a one-in-1.8×10¹⁹ event. The consequence of a collision is the reason this hashes
/// the scene rather than a modification time: a file's clock says when it was touched, not whether
/// anything in it changed.
#[derive(Clone, Copy)]
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn byte(&mut self, b: u8) {
        self.0 ^= b as u64;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }
    pub(crate) fn u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.byte(b);
        }
    }
    pub(crate) fn f32(&mut self, v: f32) {
        // Through the BITS, so this stays exact. Rounding first would make a fixture nudged by a
        // hundredth of a millimetre look unmoved, and the answer it produced look still true.
        self.u64(v.to_bits() as u64);
    }
    pub(crate) fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    /// LENGTH FIRST, so `["ab", "c"]` and `["a", "bc"]` are not the same scene.
    pub(crate) fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        for b in s.as_bytes() {
            self.byte(*b);
        }
    }
    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}

/// Fold a serialisable value into a fingerprint, through its JSON.
///
/// Field by field would be faster and is what a hash normally does. It is not what is wanted here:
/// these are the tables that gain fields most often, and a hand-written list of them is a list that
/// falls behind without saying so — the failure being an out-of-date result shown as current.
/// Serialisation covers every field a type has, including the ones added next year.
///
/// A value that will not serialise hashes as the ERROR rather than as an empty string, so two
/// different scenes cannot come out identical because both failed.
pub(crate) fn hash_json<T: serde::Serialize + ?Sized>(h: &mut Fnv, tag: &str, v: &T) {
    h.str(tag);
    match serde_json::to_string(v) {
        Ok(s) => h.str(&s),
        Err(e) => h.str(&format!("<unserialisable {tag}: {e}>")),
    }
}

/// Everything a calculation needs, OWNED — so it can cross a thread.
///
/// Split out so the expensive part touches no `&self`: the app hands this to a worker and keeps
/// painting. Building it is cheap (it clones a handful of tables and the scene triangles it was
/// going to build anyway); running it is the minutes.
pub struct CalcJob {
    meshes: Vec<Mesh>,
    lums: Vec<Luminaire>,
    profiles: HashMap<String, IesProfile>,
    materials: Vec<Material>,
    settings: RaySettings,
    maintenance: Maintenance,
    /// One per room: `(name, footprint)`. A project with no rooms has one unnamed target.
    targets: Vec<(String, Vec<glam::Vec2>)>,
    /// The solids standing in each room — one list per target, in the same order.
    ///
    /// Gathered on the UI thread because it needs the Factory, which the worker cannot see. See
    /// [`Obstacle`]: a working-plane cell inside one of these is not a place anybody measures, and
    /// reporting it as 0 lx took a room's minimum to zero and its uniformity with it.
    obstacles: Vec<Vec<Obstacle>>,
    /// The whole-model bounds, for a target with no footprint.
    fallback: (f32, f32, f32, f32),
    cell_size: f32,
    plane_height: f32,
    eye_height: f32,
    wall_zone: f32,
    scene_tris: usize,
    /// Express or Thorough — see [`CalcMode`]. In the fingerprint, so an Express preview can never
    /// be restored as the answer to a Thorough request.
    mode: CalcMode,
}

/// What a calculation produced.
pub struct CalcOutcome {
    pub rooms: Vec<RoomResult>,
    pub surfaces: Vec<cad_light::SurfaceResult>,
    pub meshes: Vec<Mesh>,
    pub timings: Vec<(&'static str, f64)>,
    /// True when the worker was asked to stop and did.
    pub cancelled: bool,
    /// The scene this answer belongs to — [`CalcJob::fingerprint`], taken from the job the worker
    /// was actually handed rather than from the app afterwards. Between pressing Calculate and the
    /// answer coming back there are minutes in which somebody can move a fixture, and a result
    /// stamped with the scene as it looked on ARRIVAL would claim to describe a building it was
    /// never computed from.
    pub fingerprint: u64,
    /// WHICH MODE ACTUALLY PRODUCED THIS, taken from the job and not from the app.
    ///
    /// The switch on screen says what the NEXT run will be. A user who flips it to Thorough and
    /// reads the panel before pressing Calculate would otherwise be told that the Express numbers
    /// in front of them are Thorough ones, which is the whole failure this labelling exists to
    /// prevent.
    pub mode: CalcMode,
}

/// THE CALCULATION'S OWN VERSION. Bump this whenever the engine changes what a given scene MEANS.
///
/// Reported as: *"did you fix the 0 min lux bug? i still experience."* — after the fix was written,
/// tested and pushed. Two things were wrong, and this is the one that would have survived
/// installing the new build.
///
/// [`CalcJob::fingerprint`] hashes the inputs, and its doc argues — correctly — that if a value is
/// not in the job it cannot have reached the engine. That reasoning has a hole in it: it holds for
/// a FIXED engine. The stored result beside a drawing carries the fingerprint of the scene, so
/// reopening the project restores it whenever the scene still matches. When the buried-cell
/// exclusion landed, the same scene started meaning a different answer — but every input to the
/// hash was untouched, so a result computed before the fix restored as *valid* and a project went
/// on reporting a minimum of 0 lx on a build that could no longer produce one.
///
/// Hashing the engine's own version closes it. A superseded result no longer matches, so it is not
/// restored and the history says to recalculate — which is the truth.
///
/// This is deliberately NOT the build number. Most builds do not change any answer, and tying it to
/// one would throw away every stored result on every release — a seventy-second job on a real
/// building, and several minutes on a large one. It moves when the ANSWER moves.
///
/// | epoch | what changed |
/// |-------|--------------|
/// | 1     | the original engine |
/// | 2     | grid points inside furniture are excluded rather than reported as 0 lx |
pub const CALC_EPOCH: u64 = 3;

impl CalcJob {
    /// A HASH OF EVERY INPUT TO THE ANSWER — the thing that decides whether a saved result is
    /// still true.
    ///
    /// A result is worth keeping only for as long as the scene that produced it is unchanged, and
    /// "unchanged" has to mean something precise. It cannot mean a modification time, which says
    /// when a file was touched and not whether anything in it moved. It cannot mean a flag set by
    /// hand at each of the places that edit a fixture, a wall, a reflectance or a ray count,
    /// because that list is long, it grows, and the day somebody adds an edit path and forgets the
    /// flag the app shows an out-of-date answer as a current one. **A wrong lux figure presented
    /// confidently is worse than no figure at all** — nothing on the page says which it is.
    ///
    /// So it is hashed from the job itself, which IS the calculation's input by construction: if a
    /// value is not in `CalcJob` it cannot have reached the engine.
    ///
    /// THE `let CalcJob { .. }` BELOW HAS NO `..` AND THAT IS THE POINT. Adding a field to the job
    /// stops this compiling until somebody has decided whether it changes the answer. The guarantee
    /// is a compile error rather than a test, so it cannot be left for later.
    ///
    /// AND THE ENGINE ITSELF IS PART OF THE INPUT — see [`CALC_EPOCH`], which this hashes first.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint_with_epoch(CALC_EPOCH)
    }

    /// The fingerprint under a GIVEN engine version.
    ///
    /// Split out so the epoch can be varied in a test. A constant folded into a hash is impossible
    /// to check from outside — the obvious test compares two hash seeds and passes whether or not
    /// the constant ever reaches the fingerprint, which is exactly what the first attempt at this
    /// did. With the epoch as a parameter, "an older engine's result does not match" is a statement
    /// about this function rather than about arithmetic.
    pub(crate) fn fingerprint_with_epoch(&self, epoch: u64) -> u64 {
        let CalcJob {
            meshes,
            lums,
            profiles,
            materials,
            settings,
            maintenance,
            targets,
            // DERIVED, SO NOT HASHED AGAIN. The obstacles are a pure function of the scene
            // triangles and the room outlines, and both of those are hashed below — a change to
            // either moves the fingerprint, and nothing else can move the obstacles. Hashing their
            // triangles a second time would be a second pass over the model for no new fact.
            obstacles: _,
            fallback,
            cell_size,
            plane_height,
            eye_height,
            wall_zone,
            // Derived from `meshes`, and reported rather than used — but hashing it costs one word
            // and removes the question.
            scene_tris,
            mode,
        } = self;

        let mut h = Fnv::new();
        // THE ENGINE IS AN INPUT TOO. First, so no stored answer from a different one can collide.
        h.u64(epoch);
        h.u64(*scene_tris as u64);
        // AND SO IS HOW MUCH OF THE MODEL IT WAS SHOWN. The two modes make the furniture different
        // geometry, so `meshes` already differs — but only for a scene that HAS furniture. Hashed
        // in its own right so an empty room's Express and Thorough answers stay distinguishable,
        // and so the field cannot be added to the job and forgotten here.
        h.u64(*mode as u64);

        // ---- the room, as the engine sees it ---------------------------------------------
        h.u64(meshes.len() as u64);
        for m in meshes {
            h.u64(m.material as u64);
            h.u64(m.vertices.len() as u64);
            for v in &m.vertices {
                h.f32(v.x);
                h.f32(v.y);
                h.f32(v.z);
            }
            h.u64(m.triangles.len() as u64);
            for t in &m.triangles {
                h.u64(t.a as u64);
                h.u64(t.b as u64);
                h.u64(t.c as u64);
            }
        }

        // ---- everything that already serialises ------------------------------------------
        //
        // Through JSON rather than field by field, because these are the tables that change most
        // often and a hand-written list of their fields is a list that goes out of date silently.
        // Serialisation covers every field a type has, today and after the next one is added.
        hash_json(&mut h, "lums", lums);
        hash_json(&mut h, "materials", materials);
        hash_json(&mut h, "settings", settings);
        hash_json(&mut h, "maintenance", maintenance);
        // A `HashMap` iterates in a DIFFERENT ORDER EACH RUN, so hashing it directly would make
        // every result look stale roughly always. Sorted by name, which is how they are referenced.
        h.str("profiles");
        let mut names: Vec<&String> = profiles.keys().collect();
        names.sort();
        h.u64(names.len() as u64);
        for n in names {
            h.str(n);
            hash_json(&mut h, "profile", &profiles[n]);
        }

        // ---- what is being asked -----------------------------------------------------------
        h.u64(targets.len() as u64);
        for (name, poly) in targets {
            h.str(name);
            h.u64(poly.len() as u64);
            for v in poly {
                h.f32(v.x);
                h.f32(v.y);
            }
        }
        let (x0, y0, x1, y1) = *fallback;
        h.f32(x0);
        h.f32(y0);
        h.f32(x1);
        h.f32(y1);
        h.f32(*cell_size);
        h.f32(*plane_height);
        h.f32(*eye_height);
        h.f32(*wall_zone);
        h.finish()
    }

    /// How many triangles the engine was handed. Reported by the mode probe, which has to show the
    /// substitution as a number rather than assert that it happened.
    pub fn scene_triangle_count(&self) -> usize {
        self.scene_tris
    }

    /// How many steps [`run`](Self::run) will report.
    pub fn steps(&self) -> u32 {
        // The evaluator, then three phases per room, then the surfaces.
        1 + self.targets.len() as u32 * 3 + 1
    }

    fn inset_bounds(&self, b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        let z = self.wall_zone.max(0.0);
        let (x0, y0, x1, y1) = b;
        if x1 - x0 <= 2.0 * z || y1 - y0 <= 2.0 * z {
            return b;
        }
        (x0 + z, y0 + z, x1 - z, y1 - z)
    }

    /// THE WORK. No `&self` on the app, no UI, no borrows — this is what runs on the worker.
    pub fn run(self, p: &CalcProgress) -> CalcOutcome {
        use std::sync::atomic::Ordering;
        p.total.store(self.steps(), Ordering::Relaxed);
        let mut timings: Vec<(&'static str, f64)> = Vec::new();
        let mut t = std::time::Instant::now();
        // Taken HERE, from the job, and carried through both exits below — the answer is stamped
        // with the scene it was computed from, not with whatever the plan looks like by the time it
        // finishes.
        let fingerprint = self.fingerprint();
        timings.push(("fingerprint", t.elapsed().as_secs_f64() * 1000.0));
        t = std::time::Instant::now();

        // ONE EVALUATOR, EVERY ROOM. Building it builds a BVH over the whole scene, and light
        // crosses between rooms through openings — so the rooms are separate questions about one
        // scene rather than separate scenes.
        p.step("Building the scene");
        let ev = cad_light::Evaluator::new(
            &self.meshes,
            &self.lums,
            &self.profiles,
            &self.materials,
            self.settings,
            self.maintenance,
        );
        timings.push(("evaluator", t.elapsed().as_secs_f64() * 1000.0));
        t = std::time::Instant::now();

        let mut rooms = Vec::with_capacity(self.targets.len());
        for (i, (name, poly)) in self.targets.iter().enumerate() {
            if p.cancelled() {
                return CalcOutcome {
                    rooms,
                    surfaces: Vec::new(),
                    meshes: self.meshes,
                    timings,
                    cancelled: true,
                    fingerprint,
                    mode: self.mode,
                };
            }
            let label = if name.is_empty() {
                format!("Calculating ({} of {})", i + 1, self.targets.len())
            } else {
                format!("{name} ({} of {})", i + 1, self.targets.len())
            };
            let obs = self.obstacles.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
            rooms.push(self.room_result(&ev, name, poly, obs, p, &label));
        }
        timings.push(("grids", t.elapsed().as_secs_f64() * 1000.0));
        t = std::time::Instant::now();

        // THE ROOM SURFACES — a whole extra pass over every wall, floor and ceiling, on top of the
        // grids. EN 12464-1 sets requirements on them and a report quotes them, so Thorough pays
        // it; Express is for moving fittings around and does not, which is a straight saving with
        // no effect on the working-plane numbers.
        p.step("Room surfaces");
        let surfaces = match self.mode {
            CalcMode::Thorough => {
                cad_light::surface_report_on(&ev, &self.meshes, &self.lums, &self.materials, 1.0)
            }
            CalcMode::Express => Vec::new(),
        };
        timings.push(("surfaces", t.elapsed().as_secs_f64() * 1000.0));
        timings.push(("scene_tris", self.scene_tris as f64));

        CalcOutcome { rooms, surfaces, meshes: self.meshes, timings, cancelled: false, fingerprint, mode: self.mode }
    }

    /// Everything about ONE room, from an evaluator already built over the whole scene.
    fn room_result(
        &self,
        ev: &cad_light::Evaluator,
        name: &str,
        poly: &[glam::Vec2],
        obstacles: &[Obstacle],
        p: &CalcProgress,
        label: &str,
    ) -> RoomResult {
        let (min_x, min_y, max_x, max_y) =
            if poly.len() >= 3 { self.inset_bounds(poly_bounds(poly)) } else { self.fallback };
        let (w, d) = ((max_x - min_x).max(1e-3), (max_y - min_y).max(1e-3));
        let (cols, rows) = LightState::grid_for(w, d, self.cell_size);
        let grid_note = LightState::grid_note_for(w, d, self.cell_size, cols, rows);
        let plane = CalcPlane {
            origin: Vertex::new(min_x, min_y, self.plane_height),
            width: w,
            depth: d,
            cols,
            rows,
        };

        p.step(&format!("{label} — working plane"));
        let mut grid = cad_light::calculate_on(ev, &plane, self.maintenance);
        // The room's figures are over the ROOM. For a rectangular room every cell is inside it and
        // this changes nothing — which is every case the engine is validated on.
        let mask = LightState::measurable_mask(&plane, poly, obstacles);
        if mask.iter().any(|k| !k) {
            LightState::apply_room_mask(&mut grid, &mask);
        }

        p.step(&format!("{label} — EN 12464-1 grid"));
        let plane_en = plane.on_standard_grid();
        let mut grid_en = cad_light::calculate_on(ev, &plane_en, self.maintenance);
        // KEPT, not just applied. The report quotes this grid, and two of its sections walk the
        // cells themselves rather than reading the summary — see `RoomResult::mask_en`.
        let mut mask_en = Vec::new();
        if poly.len() >= 3 {
            let m = LightState::measurable_mask(&plane_en, poly, obstacles);
            if m.iter().any(|k| !k) {
                LightState::apply_room_mask(&mut grid_en, &m);
                mask_en = m;
            }
        }

        // Mean cylindrical illuminance at eye height, on a coarse sub-grid — every point costs 24
        // azimuth evaluations, so measuring it at the work plane's resolution would multiply the
        // calculation by twenty-four to refine one room-average figure.
        p.step(&format!("{label} — cylindrical"));
        let cylindrical_avg = {
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

        // THE ROOM'S OWN FITTINGS, and its own load. A power density taken over every fitting in
        // the building and divided by one room's floor is not a figure about anything.
        let fixtures = LightState::fixtures_in(poly, &self.lums);
        let installation =
            Some(installation_summary(&fixtures, &self.profiles, (w * d) as f64));

        RoomResult {
            name: name.to_string(),
            poly: poly.to_vec(),
            plane,
            grid,
            mask,
            plane_en,
            grid_en,
            mask_en,
            cylindrical_avg,
            installation,
            fixtures,
            grid_note,
        }
    }
}

/// One room's answer, whole.
///
/// A CALCULATION USED TO PRODUCE ONE OF THESE and the app held it in loose fields. With two rooms
/// on the plan that meant the second calculation overwrote the first — reported as "when i make a
/// building after doing a calculation for another building the app only generates the calculation
/// for the last building". Worse, which room you got depended on what happened to be SELECTED,
/// because `calc_room_polygon` picked the selected room and fell back to "the only one" — so with
/// two rooms and nothing selected it produced neither, and lit the whole model's bounding box.
///
/// So a calculation now produces a LIST of these, one per room, from one shared evaluator. Light
/// crosses between rooms through openings, which is why the evaluator is built over the whole model
/// and not per room: the rooms are separate QUESTIONS about one scene, not separate scenes.
#[derive(Clone)]
pub struct RoomResult {
    pub name: String,
    /// The room's footprint. Empty for a project with no rooms — the whole-model fallback.
    pub poly: Vec<glam::Vec2>,
    pub plane: CalcPlane,
    pub grid: LuxGrid,
    /// Which cells of `plane` are inside `poly`. Empty when the plane IS the room.
    pub mask: Vec<bool>,
    pub plane_en: CalcPlane,
    pub grid_en: LuxGrid,
    /// Which cells of `plane_en` are measurable — the twin of `mask`, for the standard's grid.
    ///
    /// NEEDED BECAUSE `apply_room_mask` FIXES THE STATISTICS AND LEAVES THE CELLS. A masked grid's
    /// `avg`, `min` and `max` are computed over the kept cells only, but `values` still holds every
    /// reading including the ones taken inside a cupboard. Anything that scans `values` itself —
    /// the report's extremes and its percentiles — therefore has to be handed the mask, or it will
    /// quote a 0 lx reading from inside the furniture as the room's minimum while the summary two
    /// rows above says 108 lx.
    pub mask_en: Vec<bool>,
    pub cylindrical_avg: Option<f64>,
    pub installation: Option<Installation>,
    /// The fixtures standing in this room — the RECORDS, not their ids.
    ///
    /// Ids alone were not enough: a room contains user-placed fixtures AND the lights the model
    /// generates (a curved luminaire is a real fitting), and the generated ones exist only for the
    /// length of a calculation. Resolving ids against `LightState::luminaires` afterwards found
    /// the placed ones and silently dropped the rest — so a room's Installation section counted
    /// 89 fittings its schedule did not list.
    pub fixtures: Vec<Luminaire>,
    /// A note about a coarsened grid, if this room needed one.
    pub grid_note: Option<String>,
}

impl RoomResult {
    /// The area the density is quoted over, m².
    pub fn area_m2(&self) -> f64 {
        (self.plane.width * self.plane.depth) as f64
    }

    /// The grid spacing this room was calculated at — the COARSER axis, which is the one that
    /// decides what a maximum can miss.
    pub fn spacing(&self) -> f32 {
        let sx = self.plane.width / self.grid.cols.max(1) as f32;
        let sy = self.plane.depth / self.grid.rows.max(1) as f32;
        sx.max(sy)
    }

    /// The same, for the EN 12464-1 grid computed beside it.
    pub fn spacing_en(&self) -> f32 {
        let sx = self.plane_en.width / self.grid_en.cols.max(1) as f32;
        let sy = self.plane_en.depth / self.grid_en.rows.max(1) as f32;
        sx.max(sy)
    }
}

/// All lighting UI + engine state, owned by `CadApp`.
pub struct LightState {
    /// Toggles the Light window (Tools ▸ SIMLUX Light).
    pub window_open: bool,
    /// Toggles the Illuminaire window — the fitting library (2D block + photometric file).
    pub illuminaire_open: bool,
    /// The library, loaded once at startup. App-wide, not per-project — see [`crate::illuminaire`].
    pub library: crate::illuminaire::Library,
    /// The fitting armed for placement: the next click in the plan puts one down. `None` = idle.
    pub place_fitting: Option<u32>,
    /// The library row SELECTED — what the detail strip below the tiles is about.
    pub lib_sel: Option<u32>,
    /// The fixtures as they were before an edit made inside the panel, waiting to become an undo
    /// step. See [`LightState::stage_undo`].
    pub undo_pending: Option<Vec<Luminaire>>,
    /// Folder scanned for .ldt / .ies files — where the LDT half of a combo is chosen from.
    /// Remembered across sessions, because a practice keeps its photometry in one place.
    pub lib_folder: String,
    /// What that scan found: (stem, full path), sorted. Rebuilt on demand, never persisted.
    pub lib_scanned: Vec<(String, String)>,
    /// The blocks offered in the add panel, flattened once when the panel is filled rather than
    /// every frame — a real plan has hundreds of definitions.
    pub lib_blocks: Vec<crate::illuminaire::BlockRow>,
    /// Where those blocks came from, for the panel heading.
    pub lib_blocks_from: String,
    /// The unit those blocks were authored in, metres per drawing unit — carried onto a fitting
    /// when one is added, because the destination drawing is usually a different one.
    pub lib_blocks_unit_m: f64,
    /// The library on disk could not be read, so nothing here may be written back over it. Set
    /// once at startup; the window says so and refuses to save.
    pub illuminaire_locked: bool,
    /// Whether the add panel is open.
    pub lib_add_open: bool,
    /// The name field for the selected fitting, buffered so typing does not fight the library.
    pub lib_name_buf: String,
    /// One result per room, in the order the rooms are drawn. See [`RoomResult`].
    pub rooms: Vec<RoomResult>,
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
    /// How far in from the room's own outline the working plane starts, metres.
    ///
    /// DIALux calls this the WALL ZONE and states it on every report — 0.010 m on the identical-room
    /// files this engine is validated against. Zero means the whole room interior. It is here rather
    /// than assumed because it is a stated condition of the result, not a preference.
    pub wall_zone: f32,
    /// Which grid cells lie inside the room, when the plane was placed on one. Empty otherwise.
    ///
    /// A non-rectangular room's rectangular grid necessarily covers ground outside it; those cells
    /// are computed but are not part of the room's average and are not painted.
    pub grid_mask: Vec<bool>,
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
    /// THE AIM TOOL IS ARMED: the next clicks pick a fitting, then the point it should light.
    pub aim_mode: bool,
    /// The fitting the aim tool has picked, waiting for its target. `None` = still choosing one.
    ///
    /// TWO CLICKS AND A HELD ID, rather than aiming whatever happens to be selected. The selection
    /// is what Delete and drag act on, and quietly re-aiming a dozen fittings because they were
    /// still highlighted from an earlier gesture is not what "select a light and then click on a
    /// point" asks for.
    pub aim_pick: Option<u32>,
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
    /// WHICH BLOCK INSTANCE EACH FIXTURE WAS PLACED AS — fixture id → dobject handle.
    ///
    /// Reported as: "i rotated a light and the result was still valid… does rotating the lights in
    /// the 2d actually rotate a light in simlux as well?" It did not. `rotate`, `move`, `scale` and
    /// `mirror` edit `doc.dobjects` and nothing else, so the symbol turned on the plan and the
    /// luminaire behind it kept the aiming it was placed with. The calculation was then answering a
    /// question about a layout the drawing no longer showed — and since nothing about the fixture
    /// had changed, the result did not even go out of date.
    ///
    /// A HANDLE, not a position. The link used to be "the block of this fitting standing where the
    /// fixture stands", which is exact right up until either of them moves — so it could never
    /// survive the very edits it needed to follow. A dobject's handle is stable across moves,
    /// rotations, undo, redo and a save/reopen.
    ///
    /// Keyed by NAME-like ids on both sides rather than positions in a list, because both lists are
    /// reordered by ordinary editing.
    pub symbol_of: std::collections::BTreeMap<u32, u64>,
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
    /// THE SAME ROOM ON EN 12464-1's GRID — what a compliance figure rests on.
    ///
    /// Uniformity is not a property of a room. It is a property of a room AND the grid it was
    /// sampled on, and the two grids that matter here are not the same grid: the WORKING one is at
    /// whatever cell size the designer set, while the standard's spacing GROWS with the room —
    /// 1.94 m across a 33 m hall, 5 m across a 100 m plan.
    ///
    /// The working grid is the finer of the two for every room down to about 3 m, so the figure
    /// this panel has always shown is the CONSERVATIVE one: a finer grid has more chances to land
    /// on the room's true minimum, and reports U₀ lower for it. Which is why the standard's figure
    /// is reported BESIDE the working one and does not replace it — switching would raise every
    /// uniformity number in every project, and that is the direction which passes an installation
    /// that should have failed.
    ///
    /// `None` until a calculation has run, exactly like `grid`.
    pub grid_en: Option<LuxGrid>,
    pub plane_en: Option<CalcPlane>,
    /// WHERE THE LAST CALCULATION SPENT ITS TIME — `(phase, milliseconds)`, in order.
    ///
    /// A calculation that takes fifteen minutes and one that has hung look identical from outside:
    /// a window that stopped repainting. Windows greys it out, calls it "not responding", and the
    /// user reports a crash — which is what happened here, and why there was no crash log to find.
    /// This is the readout that tells the two apart, and it is a field rather than a `println!` so
    /// the app can show it as readily as the offline harness.
    pub last_timings: Vec<(&'static str, f64)>,
    /// THE SCENE THE ANSWER ON SCREEN BELONGS TO — [`CalcJob::fingerprint`], or `None` when
    /// nothing has been calculated.
    ///
    /// A lighting result stops being true the moment anything it was computed from moves, and
    /// there is nothing about the numbers themselves that says so: 412 lx from yesterday's layout
    /// and 412 lx from today's look identical on the page. This is what tells them apart, and it is
    /// what makes writing the result to disk safe at all — without it, reopening a project would
    /// show a figure with no way to know which building it describes.
    pub results_fingerprint: Option<u64>,
    /// The mode the answer ON SCREEN was computed in — NOT [`Self::mode`], which is what the next
    /// run will use. They differ the moment the switch is flipped and Calculate has not been
    /// pressed, and that gap is exactly when a mislabelled Express result would escape.
    pub results_mode: Option<CalcMode>,
    /// The scene has moved on since the answer on screen was computed.
    ///
    /// The result is KEPT and clearly marked rather than thrown away. Somebody who nudges a fixture
    /// and instantly loses a seventy-second answer has lost the very thing they were comparing
    /// against — and the change may well be the nudge they are about to undo.
    pub results_stale: bool,
    /// When staleness was last checked. Asking costs what building the scene triangles costs, so it
    /// is asked a few times a second rather than sixty.
    pub stale_checked: Option<std::time::Instant>,
    /// The answer on screen was READ BACK from disk rather than computed this session. Said out
    /// loud in the panel: "where did this number come from" has exactly one honest answer, and it
    /// is not always "you just pressed Calculate".
    pub results_restored: bool,
    pub meshes: Vec<Mesh>,
    /// BUMPED EVERY TIME [`Self::meshes`] IS REPLACED — the SIMLUX 3D view's cache key.
    ///
    /// That view rebuilt its whole vertex buffer every frame: on a real project that is 7.03 M
    /// triangles cloned, re-transformed and expanded into 40-byte vertices — about 844 MB allocated
    /// and thrown away per frame, on the UI thread, while the plan beside it is being edited. It is
    /// cached now, and a cache needs to know when it is wrong.
    ///
    /// A COUNTER AND NOT A CONTENT HASH, because the thing it guards is 21 M floats: hashing them
    /// to find out whether to rebuild costs the same order as rebuilding. And not the calculation
    /// fingerprint either, which looks like it would do and does not — a CANCELLED run assigns
    /// `meshes` and leaves `results_fingerprint` untouched, so the cache would keep painting the
    /// scene the cancelled run replaced.
    ///
    /// Assign through [`Self::set_meshes`] rather than writing the field, or the view goes stale.
    pub meshes_gen: u64,
    /// What the LIVE mesh rebuild was last run for -- see `live_mesh_sig_of`.
    pub live_mesh_sig: Option<u64>,
    /// The cheap scene signature the CURRENT result corresponds to -- the reference the staleness
    /// question is answered against. `None` means no reference yet, so the next check must do the
    /// expensive fingerprint once to establish one.
    pub stale_ref_sig: Option<u64>,
    /// Express or Thorough — see [`CalcMode`]. This is what the NEXT calculation will run as; the
    /// mode a result was actually computed in travels with the result, in [`RoomResult::mode`],
    /// because the two disagree the moment the user flips the switch and has not pressed Calculate.
    pub mode: CalcMode,
    /// Paint the false-colour overlay on the 2D plan.
    pub show_overlay: bool,
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
    /// Draw the ISOLUX LINES — the band thresholds traced as curves — over the field.
    ///
    /// A false-colour field says roughly how much light there is; an isolux line says exactly where
    /// a number is, and "the 300 lx line runs here" is something a tape measure can be held
    /// against. They are read together, which is why this is a separate switch rather than a mode:
    /// turning the colour off and leaving the lines on is a perfectly ordinary thing to want when
    /// checking a layout against them.
    pub show_isolux: bool,
    /// Draw a line from each fitting along the way it POINTS, down to where it meets the floor.
    ///
    /// Asked for as: *"in aiming lights add aiming arrows that the user can turn on and off in the
    /// illuminaire tab that shows where the light is aimed at."* It lives beside the aim tool
    /// because it is that tool's readout — aiming a fitting and seeing nothing change is exactly
    /// how a working feature gets reported as broken.
    ///
    /// ON by default, and deliberately: a toggle that hides the thing it was asked for is the same
    /// failure one step removed. It is in the Luminaires menu for a plan crowded enough to want it
    /// off.
    pub show_aim: bool,
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


/// Re-point `from_block` after a reopen, from the name-keyed map the sidecar carries.
///
/// A `from_block` is a POSITION in the block table, and positions do not survive a drawing that
/// gained or lost a definition between sessions — the same reason everything else here is keyed by
/// name. The luminaire's own `profile` is what the calculation uses and it round-trips intact, so
/// what is being repaired is the LINK back to the symbol on the plan: which block on the drawing
/// this light is.
///
/// Only an UNAMBIGUOUS answer is written. Two blocks sharing one profile is a legitimate thing to
/// draw — the same downlight in a square and a round housing — and guessing between them would
/// quietly attach a light to the wrong symbol, which no one would ever see and everyone would
/// inherit. Returns how many were repaired.
pub fn repair_from_blocks(
    lums: &mut [Luminaire],
    doc: &Document,
    block_ies: &std::collections::BTreeMap<String, String>,
) -> usize {
    // Profile → the block ids in THIS document that claim it.
    let mut by_profile: HashMap<&str, Vec<u32>> = HashMap::new();
    for (name, profile) in block_ies {
        if let Some(id) = doc.blocks.find(name) {
            by_profile.entry(profile.as_str()).or_default().push(id);
        }
    }
    let mut fixed = 0;
    for l in lums.iter_mut() {
        // Already pointing at a block that agrees with it — nothing to do. Checking the NAME
        // rather than merely that the index resolves is the point: after a table shifts, the old
        // index still resolves, just to the wrong definition.
        let ok = l.from_block.and_then(|b| doc.blocks.get(b)).is_some_and(|blk| {
            block_ies.get(&blk.name).is_some_and(|p| *p == l.profile)
        });
        if ok {
            continue;
        }
        match by_profile.get(l.profile.as_str()) {
            Some(ids) if ids.len() == 1 => {
                l.from_block = Some(ids[0]);
                fixed += 1;
            }
            _ => {}
        }
    }
    fixed
}

impl LightState {
    pub fn new() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(BUILTIN.to_string(), builtin_downlight());
        Self {
            window_open: false,
            illuminaire_open: false,
            library: crate::illuminaire::Library::default(),
            place_fitting: None,
            lib_sel: None,
            undo_pending: None,
            lib_folder: String::new(),
            lib_scanned: Vec::new(),
            lib_blocks: Vec::new(),
            lib_blocks_from: String::new(),
            lib_blocks_unit_m: 1.0,
            illuminaire_locked: false,
            lib_add_open: false,
            lib_name_buf: String::new(),
            rooms: Vec::new(),
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
            wall_zone: 0.010,
            grid_mask: Vec::new(),
            model_fixtures: 0,
            surfaces: Vec::new(),
            luminaires: Vec::new(),
            auto_center_light: true,
            place_mode: false,
            aim_mode: false,
            aim_pick: None,
            selected: Vec::new(),
            hover: None,
            drag: None,
            next_id: 1,
            symbol_of: Default::default(),
            array_rows: 3,
            array_cols: 4,
            mount_to_ceiling: true,
            ceiling_drop: 0.0,
            mount_height: 3.0,
            grid: None,
            plane: None,
            grid_en: None,
            plane_en: None,
            last_timings: Vec::new(),
            results_fingerprint: None,
            results_stale: false,
            stale_checked: None,
            results_restored: false,
            meshes: Vec::new(),
            meshes_gen: 0,
            live_mesh_sig: None,
            stale_ref_sig: None,
            mode: CalcMode::default(),
            results_mode: None,
            show_overlay: true,
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
            // OFF by default: lines over a field the reader has not asked to be gridded is clutter,
            // and the field alone is what the view has always shown.
            show_isolux: false,
            show_aim: true,
            ramp: LuxRamp::default(),
            hide_ceilings: true,
        }
    }

    /// WHETHER THE FALLBACK PALETTE CAN CHANGE THE PICTURE AT ALL.
    ///
    /// Asked as: *"does this even do anything? it seems to be redundant."* With the default scale
    /// it does not — four thresholds make five bands and there are five band colours, so every band
    /// is coloured explicitly and the palette is never reached. It becomes live in exactly two
    /// cases, and the control is shown in exactly those.
    pub fn palette_is_in_play(&self, opt: &crate::report::Options, room_max: f64) -> bool {
        // A CONTINUOUS SCALE has no bands, so the palette draws the whole field.
        if opt.scale.bands.is_empty() {
            return true;
        }
        // …or a band has been left without a colour — by adding a threshold, or by `reset`.
        let bands = opt.scale.edges(room_max).len().saturating_sub(1);
        opt.band_colours.len() < bands
    }

    /// The fitting a NEW point should get: the chosen one, or nothing.
    fn default_profile(&self) -> String {
        if self.profiles.contains_key(&self.active_profile) {
            self.active_profile.clone()
        } else {
            UNASSIGNED.to_string()
        }
    }

    /// THE GRID THE PANEL QUOTES — EN 12464-1's, matching the report.
    ///
    /// Asked for as: *"lets only show the en grid since its the standard showing 2 results will
    /// confuse the user."* The screen and the page have to name the same number, so this is the
    /// panel's twin of `report::layout::RoomInput::reported`.
    ///
    /// ONE ROOM ONLY. `self.grid` is a project-wide grid combined across rooms, and there is no
    /// combining an average of averages without weighting by area — which the combined grid already
    /// did. So the standard's figures are quoted where they are unambiguous, and a multi-room
    /// project keeps the combined summary it always had and sends the reader to the report, which
    /// is per-room and is the authority.
    pub fn reported_grid(&self) -> Option<&LuxGrid> {
        if self.rooms.len() == 1 && !self.rooms[0].grid_en.values.is_empty() {
            return Some(&self.rooms[0].grid_en);
        }
        self.grid.as_ref()
    }

    /// Whether [`reported_grid`](Self::reported_grid) is the standard's rather than the working one
    /// — so a label can say which it is showing instead of leaving it to be guessed.
    pub fn reporting_en_grid(&self) -> bool {
        self.rooms.len() == 1 && !self.rooms[0].grid_en.values.is_empty()
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
        // STAGED HERE, not at the call site. A snapshot every caller has to remember is one a new
        // caller forgets, and the symptom is silent: the fixture appears and Ctrl+Z reaches past
        // it to an older edit.
        self.stage_undo();
        let (z, on_ceiling) = self.mount_z_at(x, y);
        let id = self.next_id;
        self.next_id += 1;
        let profile = self.default_profile();
        self.luminaires.push(Luminaire {
            id,
            profile: profile.clone(),
            position: Vertex::new(x, y, z),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
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


    /// Remember the fixtures as they are, so the edit about to happen can be taken back.
    ///
    /// Staged rather than pushed, because these methods are called from inside the panel's own
    /// closure — `self.doc` is not reachable there, and an undo step is `CadApp`'s to push.
    /// `CadApp::commit_light_undo` drains this once the closure has gone.
    ///
    /// Staging TWICE in one frame keeps the FIRST copy: two edits between drains are one undo, and
    /// the earlier state is the one a person means by "before".
    pub fn stage_undo(&mut self) {
        if self.undo_pending.is_none() {
            self.undo_pending = Some(self.luminaires.clone());
        }
    }

    /// Throw away a staged snapshot for an edit that turned out not to happen — a press on a
    /// marker that never moved is a selection, and must not cost an Undo press.
    pub fn discard_staged_undo(&mut self) {
        self.undo_pending = None;
    }

    /// The lux figures on screen were computed from a layout that no longer holds.
    ///
    /// Dropped rather than recalculated: a calculation can take minutes on a real building, and
    /// running one nobody asked for on every undo would be worse than the stale numbers. Leaving
    /// them, though, is worse than both — a result under a layout it was not computed from is a
    /// figure someone will read off and issue.
    pub fn invalidate_result(&mut self) {
        self.grid = None;
        self.plane = None;
        self.grid_en = None;
        self.plane_en = None;
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
        // THE MOMENT IT BECOMES A MOVE is the moment worth remembering, and the last one at which
        // the fixtures still hold their original positions. Staging on the PRESS instead would
        // cost an Undo press for every click that merely selected a marker; staging on release
        // would capture where they ended up.
        let becomes_moved = !d.moved && (dx.abs() > 1e-4 || dy.abs() > 1e-4);
        if becomes_moved {
            d.moved = true;
        }
        let start = d.start.clone();
        if becomes_moved {
            self.stage_undo();
        }
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
        self.stage_undo();
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
        self.stage_undo();
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
                    tilt_deg: 0.0,
                    dimming: 1.0,
                    watts_override: None,
                    flux_override: None,
                    from_block: None,
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
        let mode = self.mode;
        let from_3d = factory.map(|f| meshes_from_factory_mode(f, None, mode)).unwrap_or_default();
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

    /// Wattage, efficacy and aiming for the SELECTED fittings — editable, per fitting.
    ///
    /// "the paramters of the light also need to accessible to the user, like that wattage and
    /// efficacy … the changes should be for the ldt file in the app the parent ldt file shouldnt
    /// change." These are overrides carried on the FIXTURE and saved in the project. Nothing in
    /// either crate writes a `.ldt` or `.ies` file, so the manufacturer's data is safe by
    /// construction, not by care.
    ///
    /// WATTS and FLUX are the two independent numbers; EFFICACY is shown and edited as
    /// `flux / watts`, because storing it as a third field would be storing a number that must
    /// agree with the other two — which is a bug waiting to happen. Editing efficacy moves the
    /// FLUX, which is what changes the light; editing watts alone changes only the power density.
    fn selected_fixture_params_ui(&mut self, ui: &mut egui::Ui) {
        let sel: Vec<u32> = self.selected.clone();
        if sel.is_empty() {
            ui.label(
                egui::RichText::new("  select a fitting on the plan to edit its parameters")
                    .small()
                    .weak(),
            );
            return;
        }
        // Read the first selected fixture's current figures; an edit applies to all of them.
        let Some(first) = self.luminaires.iter().find(|l| sel.contains(&l.id)).cloned() else {
            return;
        };
        let Some(prof) = self.profiles.get(&first.profile).cloned() else {
            ui.label(
                egui::RichText::new("  no fitting assigned — pick one from ▼ Fittings first")
                    .small()
                    .weak(),
            );
            return;
        };

        let mut watts = first.watts(&prof);
        let mut flux = first.lumens(&prof);
        let mut rot = first.rotation_deg;
        let overridden = first.watts_override.is_some() || first.flux_override.is_some();

        ui.label(
            egui::RichText::new(format!("parameters — {} selected", sel.len())).small().weak(),
        );
        let mut changed_w = false;
        let mut changed_f = false;
        ui.horizontal(|ui| {
            ui.add_sized([56.0, 18.0], egui::Label::new(egui::RichText::new("load").small()));
            changed_w |= ui
                .add(
                    egui::DragValue::new(&mut watts)
                        .update_while_editing(false)
                        .speed(0.5)
                        .range(0.0..=10_000.0)
                        .suffix(" W"),
                )
                .on_hover_text("Connected load. Changes the power density; does NOT change the light.")
                .changed();
            ui.add_sized([56.0, 18.0], egui::Label::new(egui::RichText::new("flux").small()));
            changed_f |= ui
                .add(
                    egui::DragValue::new(&mut flux)
                        .update_while_editing(false)
                        .speed(10.0)
                        .range(0.0..=1_000_000.0)
                        .suffix(" lm"),
                )
                .on_hover_text(
                    "Installed luminous flux. This DOES change the light — the profile's whole \
                     distribution is scaled by flux / rated flux.",
                )
                .changed();
        });
        // Efficacy, derived. Editing it moves the FLUX at the current wattage.
        let mut eff = if watts > 0.0 { flux / watts } else { 0.0 };
        ui.horizontal(|ui| {
            ui.add_sized([56.0, 18.0], egui::Label::new(egui::RichText::new("efficacy").small()));
            let r = ui.add_enabled(
                watts > 0.0,
                egui::DragValue::new(&mut eff)
                    .update_while_editing(false)
                    .speed(1.0)
                    .range(0.0..=400.0)
                    .suffix(" lm/W"),
            );
            if r.changed() {
                flux = eff * watts;
                changed_f = true;
            }
            r.on_hover_text("flux ÷ load. Editing this sets the flux at the current wattage.");
            ui.add_sized([44.0, 18.0], egui::Label::new(egui::RichText::new("aim").small()));
            changed_w |= false;
            if ui
                .add(
                    egui::DragValue::new(&mut rot)
                        .update_while_editing(false)
                        .speed(1.0)
                        .range(-360.0..=360.0)
                        .suffix("°"),
                )
                .on_hover_text(
                    "Rotation about the vertical axis. The physics already honours it — an \
                     asymmetric distribution turns with the fitting.",
                )
                .changed()
            {
                for l in self.luminaires.iter_mut().filter(|l| sel.contains(&l.id)) {
                    l.rotation_deg = rot;
                }
            }
        });

        if changed_w || changed_f {
            for l in self.luminaires.iter_mut().filter(|l| sel.contains(&l.id)) {
                if changed_w {
                    l.watts_override = Some(watts);
                }
                if changed_f {
                    l.flux_override = Some(flux);
                }
            }
            self.last_msg = format!(
                "{} fitting(s) re-rated — {watts:.1} W, {flux:.0} lm{}. Press Calculate.",
                sel.len(),
                if watts > 0.0 { format!(" ({:.0} lm/W)", flux / watts) } else { String::new() },
            );
        }
        // A way back to the file's own figures, so an override is never a one-way door.
        if overridden
            && ui
                .small_button("↺ back to the file's rating")
                .on_hover_text("Drop the overrides and use the photometric file's own watts and flux")
                .clicked()
        {
            for l in self.luminaires.iter_mut().filter(|l| sel.contains(&l.id)) {
                l.watts_override = None;
                l.flux_override = None;
            }
            self.last_msg = "back to the fitting's own rating — press Calculate.".into();
        }
        if overridden {
            ui.label(
                egui::RichText::new(format!(
                    "  overriding {} — the file on disk is unchanged",
                    prof.name
                ))
                .small()
                .color(egui::Color32::from_rgb(230, 190, 110)),
            );
        }
    }



    /// Everything about ONE room, from an evaluator already built over the whole scene.
    ///
    /// The evaluator is shared deliberately: light crosses between rooms through openings, so the
    /// rooms are separate QUESTIONS about one scene rather than separate scenes. Building a tree
    /// per room would be both slower and wrong.
    fn room_result(
        &self,
        ev: &cad_light::Evaluator,
        lums: &[Luminaire],
        name: &str,
        poly: &[glam::Vec2],
        fallback: (f32, f32, f32, f32),
    ) -> RoomResult {
        let (min_x, min_y, max_x, max_y) =
            if poly.len() >= 3 { self.inset_bounds(poly_bounds(poly)) } else { fallback };
        let (w, d) = ((max_x - min_x).max(1e-3), (max_y - min_y).max(1e-3));
        let (cols, rows) = Self::grid_for(w, d, self.cell_size);
        let grid_note = self.grid_note(w, d);
        let plane = CalcPlane {
            origin: Vertex::new(min_x, min_y, self.plane_height),
            width: w,
            depth: d,
            cols,
            rows,
        };
        let mut grid = cad_light::calculate_on(ev, &plane, self.maintenance);
        // The room's figures are over the ROOM. For a rectangular room every cell is inside it and
        // this changes nothing — which is every case the engine is validated on.
        let mask = Self::measurable_mask(&plane, poly, &[]);
        if !mask.is_empty() {
            Self::apply_room_mask(&mut grid, &mask);
        }

        let plane_en = plane.on_standard_grid();
        let mut grid_en = cad_light::calculate_on(ev, &plane_en, self.maintenance);
        // Kept, for the same reason as the other path — see `RoomResult::mask_en`.
        let mut mask_en = Vec::new();
        if poly.len() >= 3 {
            let m = Self::measurable_mask(&plane_en, poly, &[]);
            if !m.is_empty() {
                Self::apply_room_mask(&mut grid_en, &m);
                mask_en = m;
            }
        }

        // Mean cylindrical illuminance at eye height, on a coarse sub-grid — every point costs 24
        // azimuth evaluations, so measuring it at the work plane's resolution would multiply the
        // calculation by twenty-four to refine one room-average figure.
        let cylindrical_avg = {
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

        // THE ROOM'S OWN FITTINGS, and its own load. A power density taken over every fitting in
        // the building and divided by one room's floor is not a figure about anything.
        let fixtures = Self::fixtures_in(poly, lums);
        let installation =
            Some(installation_summary(&fixtures, &self.profiles, (w * d) as f64));

        RoomResult {
            name: name.to_string(),
            poly: poly.to_vec(),
            plane,
            grid,
            mask,
            plane_en,
            grid_en,
            mask_en,
            cylindrical_avg,
            installation,
            fixtures,
            grid_note,
        }
    }

    /// Every room to calculate, as `(name, footprint)`.
    ///
    /// ALL OF THEM, not the selected one. The old rule — the selected room, or the only room, or
    /// nothing — meant a plan with two rooms and no selection lit the whole model's bounding box,
    /// and a plan with a selection lit whichever room the user last clicked. Neither is what
    /// "Calculate" says it does.
    ///
    /// A project with no rooms gets one unnamed target with no footprint, which is the whole-model
    /// fallback the 2D-only path has always used.
    fn calc_targets(f: Option<&crate::factory::FactoryState>) -> Vec<(String, Vec<glam::Vec2>)> {
        let rooms: Vec<(String, Vec<glam::Vec2>)> = f
            .map(|f| {
                f.rooms
                    .iter()
                    .filter(|r| r.footprint.len() >= 3)
                    .enumerate()
                    .map(|(i, r)| {
                        let name = if r.name.trim().is_empty() {
                            format!("Room {}", i + 1)
                        } else {
                            r.name.trim().to_string()
                        };
                        (name, r.footprint.clone())
                    })
                    .collect()
            })
            .unwrap_or_default();
        if rooms.is_empty() {
            vec![(String::new(), Vec::new())]
        } else {
            rooms
        }
    }

    /// Which fixtures stand inside `poly`. Every fixture when there is no footprint.
    fn fixtures_in(poly: &[glam::Vec2], lums: &[Luminaire]) -> Vec<Luminaire> {
        if poly.len() < 3 {
            return lums.to_vec();
        }
        lums.iter()
            .filter(|l| crate::factory::point_in_poly(poly, l.position.x, l.position.y))
            .cloned()
            .collect()
    }

    /// The polygon the working plane belongs to: the SELECTED room's footprint, else the only
    /// room's, else `None` (a 2D-only project, or a model with no rooms defined).
    ///
    /// One room at a time is the honest unit. EN 12464-1's Ē and U₀ are per SPACE — averaging a
    /// corridor together with the office it serves produces a number describing neither, and it is
    /// the number a scheme is signed off on.
    fn calc_room_polygon(f: &crate::factory::FactoryState) -> Option<Vec<glam::Vec2>> {
        if f.rooms.is_empty() {
            return None;
        }
        // A room whose geometry is selected wins; otherwise, only act when there is no ambiguity.
        let picked = f.rooms.iter().find(|r| {
            r.floor.iter().chain(r.ceiling.iter()).chain(r.walls.iter()).chain(r.carve.iter())
                .any(|id| f.selection.contains(id))
        });
        match picked.or(if f.rooms.len() == 1 { f.rooms.first() } else { None }) {
            Some(r) if r.footprint.len() >= 3 => Some(r.footprint.clone()),
            _ => None,
        }
    }

    /// Shrink a `(min_x, min_y, max_x, max_y)` by the wall zone, never past nothing.
    fn inset_bounds(&self, b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        let z = self.wall_zone.max(0.0);
        let (x0, y0, x1, y1) = b;
        if x1 - x0 <= 2.0 * z || y1 - y0 <= 2.0 * z {
            return b; // the zone is wider than the room: an inset would invert it
        }
        (x0 + z, y0 + z, x1 - z, y1 - z)
    }

    /// Which grid cells lie INSIDE `poly` — the mask that keeps an L-shaped room from averaging in
    /// the outside of its own corner.
    ///
    /// The engine's plane is a rectangle, which is right: it is a sampling grid, not a boundary. A
    /// non-rectangular room's grid necessarily covers ground the room does not, and those cells are
    /// not part of the room's average however bright they read.

    /// Which cells of `plane` are places somebody could actually take a reading.
    ///
    /// TWO TESTS, AND THE SECOND IS THE ONE THAT WAS MISSING. A cell has to be inside the room's
    /// outline — it always did — and it has to be in FREE AIR rather than buried in something
    /// standing in the room.
    ///
    /// Reported as: *"our min lux was 0 while for relux it was 133… its an obvious error. find the
    /// root cause."* The working plane is a flat rectangle at 0.8 m and a room has things standing
    /// in it, so some of its points land inside a cupboard. The engine answered those correctly —
    /// an enclosed point receives nothing — but nobody measures illuminance inside a box, so it was
    /// a right answer to a question that should not have been asked. On the plan this came from, 65
    /// of 1140 cells were buried in the furniture: they took the room's minimum from 102 lx to
    /// zero, and its uniformity with it.
    ///
    /// With no obstacles this is exactly the outline test it has always been — which is every case
    /// the engine is validated on.
    fn measurable_mask(plane: &CalcPlane, poly: &[glam::Vec2], obstacles: &[Obstacle]) -> Vec<bool> {
        let (dx, dy) = (
            plane.width / plane.cols.max(1) as f32,
            plane.depth / plane.rows.max(1) as f32,
        );
        let mut m = Vec::with_capacity((plane.cols * plane.rows) as usize);
        for r in 0..plane.rows {
            for c in 0..plane.cols {
                let x = plane.origin.x + (c as f32 + 0.5) * dx;
                let y = plane.origin.y + (r as f32 + 0.5) * dy;
                let in_room = poly.len() < 3 || crate::factory::point_in_poly(poly, x, y);
                // Only worth asking where the cell is in the room at all.
                let free = !in_room
                    || !obstacles
                        .iter()
                        .any(|o| o.contains(glam::Vec3::new(x, y, plane.origin.z)));
                m.push(in_room && free);
            }
        }
        m
    }
    /// The outline test alone — the mask with nothing standing in the room.
    fn inside_mask(plane: &CalcPlane, poly: &[glam::Vec2]) -> Vec<bool> {
        Self::measurable_mask(plane, poly, &[])
    }


    /// Re-derive `avg` / `min` / `max` over the cells the mask keeps.
    ///
    /// The engine computes them over the whole rectangle, which is correct for the grid it was
    /// given; the room's figures are over the room. Left exactly as the engine returned them when
    /// every cell is inside, so the rectangular case — every validated case — is untouched.
    fn apply_room_mask(grid: &mut cad_light::LuxGrid, mask: &[bool]) {
        if mask.len() != grid.values.len() || mask.iter().all(|k| *k) {
            return;
        }
        let kept: Vec<f64> = grid
            .values
            .iter()
            .zip(mask)
            .filter_map(|(v, k)| k.then_some(*v))
            .collect();
        if kept.is_empty() {
            return; // nothing inside: leave the engine's own figures rather than invent zeroes
        }
        grid.avg = kept.iter().sum::<f64>() / kept.len() as f64;
        grid.min = kept.iter().cloned().fold(f64::MAX, f64::min);
        grid.max = kept.iter().cloned().fold(f64::MIN, f64::max);
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
                    tilt_deg: 0.0,
                    dimming: 1.0,
                    watts_override: None,
                    flux_override: None,
                    from_block: None,
                });
                id += 1;
            }
        }
        out
    }

    /// The fewest POINTS any plane gets, however small the room.
    ///
    /// A minimum and an average taken over one sample are the same number, and a uniformity
    /// computed from them is 1.00. Sixty-four is the old 8 × 8 floor, restated as a total.
    ///
    /// AS A TOTAL, and that is the whole difference. The floor used to be 8 cells PER AXIS, which
    /// on a 1 × 40 m corridor forced the short side to 8 cells — 0.125 m — while the long side sat
    /// at the requested 0.25 m. The floor exists so a room yields statistics, and statistics come
    /// from how many samples there are, not from how many lie along each edge; applying it per
    /// axis bent the cell out of square in the one situation where nothing needed to change.
    pub const MIN_GRID_POINTS: u64 = 64;

    /// The fewest cells on either axis, so a plane always has at least one interior boundary.
    pub const MIN_GRID_CELLS: u32 = 2;

    /// The most points a calculation plane may carry, in total.
    ///
    /// MEASURED, not chosen for looking safe — see `cad_light/tests/grid_cost.rs`, which times the
    /// real engine on a room the size of the owner's gym:
    ///
    ///     bare room (12 tris)          0.0285 ms per point
    ///     with clutter (12,012 tris)   0.0780 ms per point
    ///
    /// The cost is flat in the number of points from 1,600 to 27,456 of them, and grows only
    /// slowly with scene size because the tracer has a BVH. So this budget is about 0.5 s on a
    /// bare room and about 1.3 s on a busy one — the range someone who has just pressed
    /// ⚡ Calculate will wait through.
    ///
    /// It replaces a cap of 64 cells PER AXIS, i.e. 4,096 points ≈ 120 ms. That cap was costing
    /// the owner's real 33 × 13 m gym its requested 0.25 m resolution to save about 80 ms.
    pub const MAX_GRID_POINTS: u64 = 16_384;

    /// The grid a `w` × `d` metre plane gets at a requested `cell` size, as `(cols, rows)`.
    ///
    /// THE CELL STAYS SQUARE. That is the rule the other two bend around, because a cell size is
    /// ONE number and the UI shows one number.
    ///
    /// The old cap was 64 cells PER AXIS, so a room longer than 64 cells had its long side
    /// coarsened and its short side left alone: the owner's 33 × 13 m gym came out at 0.52 m along
    /// x and 0.25 m along y — a 2.06:1 cell, past even the 2:1 EN 12464-1 allows, while every
    /// number on screen said 0.25 m. Average, minimum and uniformity were then taken over two
    /// different resolutions at once.
    ///
    /// So both bounds scale BOTH axes by the same factor:
    ///
    ///   * below `MIN_GRID_POINTS` the grid is refined until there are enough samples to have
    ///     statistics at all;
    ///   * above `MAX_GRID_POINTS` it is coarsened, because every point is a full trace and an
    ///     unbounded grid on a site plan is a calculation that never finishes. A bound is right;
    ///     the previous one was set 4× too low.
    pub fn grid_for(w: f32, d: f32, cell: f32) -> (u32, u32) {
        let cell = cell.max(1e-3);
        let want = |m: f32| {
            let n = (m.max(0.0) / cell).round();
            // Clamped before the cast: `f32 as u32` saturates rather than wrapping, but an
            // intermediate of 1e12 would still make the product below meaningless.
            (n.clamp(Self::MIN_GRID_CELLS as f32, 1e6) as u32).max(Self::MIN_GRID_CELLS)
        };
        let (cols, rows) = (want(w), want(d));
        let points = cols as u64 * rows as u64;
        // One factor, applied to both axes, whichever bound is being met — `sqrt` because the
        // budget is on the PRODUCT and the shape has to be preserved.
        //
        // EACH DIRECTION ROUNDS THE WAY THAT KEEPS ITS OWN BOUND. Coarsening rounds DOWN, or a
        // scaled-then-rounded pair lands just past the ceiling it was called to respect — 16,428
        // points against a 16,384 budget, which a test caught. Refining rounds UP, or it stops
        // short of the minimum it was called to reach. Rounding to nearest is wrong for both.
        let (k, up) = if points > Self::MAX_GRID_POINTS {
            ((Self::MAX_GRID_POINTS as f64 / points as f64).sqrt(), false)
        } else if points < Self::MIN_GRID_POINTS {
            ((Self::MIN_GRID_POINTS as f64 / points as f64).sqrt(), true)
        } else {
            return (cols, rows);
        };
        let scale = |n: u32| {
            let s = n as f64 * k;
            ((if up { s.ceil() } else { s.floor() }) as u32).max(Self::MIN_GRID_CELLS)
        };
        (scale(cols), scale(rows))
    }

    /// What to say when the grid is not the one that was asked for — `None` when it is.
    ///
    /// A note on every calculation is a note nobody reads, so this is silent in the ordinary case
    /// and names both numbers in the case that matters: what was requested, and what was used.
    pub fn grid_note(&self, w: f32, d: f32) -> Option<String> {
        let (cols, rows) = Self::grid_for(w, d, self.cell_size);
        Self::grid_note_for(w, d, self.cell_size, cols, rows)
    }

    /// The same, from plain numbers — so the worker can ask without a `LightState` to hand.
    pub fn grid_note_for(w: f32, d: f32, cell_size: f32, cols: u32, rows: u32) -> Option<String> {
        let self_cell_size = cell_size;
        let (sx, sy) = (w / cols as f32, d / rows as f32);
        let used = sx.max(sy);
        // A fifth of a cell — comfortably past rounding a room's extent onto a whole number of
        // cells, and well short of the doubling this exists to report.
        if (used - self_cell_size).abs() <= self_cell_size * 0.2 {
            return None;
        }
        Some(format!(
            "grid coarsened to {used:.2} m from the {:.2} m asked for — {} points is the limit",
            self_cell_size,
            Self::MAX_GRID_POINTS,
        ))
    }

    /// Note that a calculation phase finished, and how long it took.
    ///
    /// ALSO PRINTS IT IMMEDIATELY when `SIMLUX_PHASE_LOG` is set, and that is the whole point: the
    /// phase worth knowing about is the one that never returns, and a table printed at the END
    /// says nothing whatever about a run that did not reach the end. Learning that cost two
    /// fifteen-minute runs which produced no output at all.
    fn note_phase(&mut self, what: &'static str, t: std::time::Instant) {
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        self.last_timings.push((what, ms));
        if std::env::var_os("SIMLUX_PHASE_LOG").is_some() {
            eprintln!("  [phase] {what:<14} {ms:>10.1} ms");
        }
    }

    /// Gather everything a calculation needs, on the UI thread, cheaply.
    ///
    /// `None` when there is nothing to calculate — and `last_msg` says so.
    ///
    /// THE EXPENSIVE PART IS NOT HERE. This builds the scene triangles and copies a handful of
    /// tables; [`CalcJob::run`] does the minutes of ray tracing, and it takes no borrow on the app
    /// so it can do them on another thread while the window keeps painting.
    pub fn prepare(
        &mut self,
        doc: &Document,
        factory: Option<&crate::factory::FactoryState>,
    ) -> Option<CalcJob> {
        self.last_timings.clear();
        let job = self.build_job(doc, factory);
        if job.is_none() {
            self.grid = None;
            self.plane = None;
            self.rooms.clear();
            self.last_msg =
                "No geometry — draw a closed room, or build one in the 3D Factory.".to_string();
        }
        if let Some(j) = &job {
            if std::env::var_os("SIMLUX_PHASE_LOG").is_some() {
                eprintln!("  [phase] scene is {} triangles", j.scene_tris);
            }
        }
        job
    }

    /// THE FINGERPRINT OF THE SCENE AS IT STANDS — what a saved result is checked against.
    ///
    /// Separate from [`prepare`](Self::prepare) because asking "is the answer on screen still
    /// true?" must not have opinions about the answer on screen: `prepare` clears the results and
    /// rewrites the status line when a project has nothing to calculate, which is the correct
    /// thing for a calculation to do and a destructive thing for a QUESTION to do. A project
    /// somebody has emptied down to nothing simply has no fingerprint.
    ///
    /// Costs what building a job costs — the scene triangles — which is why the caller throttles
    /// it rather than asking every frame.
    /// EVERYTHING A CALCULATION WOULD BE FINGERPRINTED FROM, without building it.
    ///
    /// `current_fingerprint` answers "is the result still true?" by constructing a whole `CalcJob`
    /// — which runs `scene_meshes` and transforms every furniture triangle. On the reference gym
    /// plan that is 7,036,129 triangles and it measured **600 ms**, once per frame, from the moment
    /// a result existed. The 250 ms throttle in front of it was worse than useless: the check costs
    /// more than the interval, so every frame was already past it. A throttle shorter than the work
    /// it guards never throttles anything.
    ///
    /// This is the same question asked of the INPUTS. The geometry half is
    /// [`Self::live_mesh_sig_of`] — `geom_version` plus every furniture pose — and the rest is the
    /// light-side state, through `hash_json` so a field added next year is covered without anyone
    /// remembering to add it here.
    ///
    /// `None` when the scene comes from the 2D document, which cannot be summarised cheaply; the
    /// caller then falls back to the full fingerprint, and that case has no furniture to make it
    /// expensive.
    ///
    /// PROFILES ARE HASHED BY SHAPE, not contents: name, lumens, watts, multiplier and the size of
    /// the candela table. Serialising the tables themselves would put a hundred kilobytes of JSON
    /// through this every frame, which is the cost this exists to remove. The gap is a profile
    /// replaced by a different one with identical name, output and table dimensions — and the
    /// consequence is a missing "results are stale" warning, never a wrong number.
    pub fn scene_sig(
        &self,
        doc: &Document,
        factory: Option<&crate::factory::FactoryState>,
    ) -> Option<u64> {
        let geom = self.live_mesh_sig_of(factory)?;
        let _ = doc;
        let mut h = Fnv::new();
        h.u64(geom);
        // The rooms are the calculation's TARGETS, and their footprints decide every grid.
        if let Some(f) = factory {
            h.u64(f.rooms.len() as u64);
            for r in &f.rooms {
                h.u64(r.footprint.len() as u64);
                for p in &r.footprint {
                    h.f32(p.x);
                    h.f32(p.y);
                }
            }
        }
        hash_json(&mut h, "lums", &self.luminaires);
        hash_json(&mut h, "materials", &self.materials);
        hash_json(&mut h, "settings", &self.settings);
        hash_json(&mut h, "maintenance", &self.maintenance);
        // `RoomLayer` is not `Serialize`, so this one is by hand -- four fields, and the compiler
        // will not warn if a fifth is added. Kept small and named for exactly that reason.
        h.u64(self.room.len() as u64);
        for g in &self.room {
            h.u64(g.layer_id as u64);
            h.str(&g.name);
            h.f32(g.height);
            h.u64(g.handles.len() as u64);
            for x in &g.handles {
                h.u64(*x);
            }
        }
        h.f32(self.cell_size);
        h.f32(self.plane_height);
        h.f32(self.eye_height);
        h.f32(self.wall_zone);
        h.f32(self.room_height);
        h.u64(self.mode as u64);
        let mut names: Vec<&String> = self.profiles.keys().collect();
        names.sort();
        h.u64(names.len() as u64);
        for n in names {
            h.str(n);
            if let Some(p) = self.profiles.get(n) {
                h.f64(p.lumens);
                h.f64(p.watts);
                h.f64(p.multiplier);
                h.u64(p.vertical_angles.len() as u64);
                h.u64(p.horizontal_angles.len() as u64);
            }
        }
        Some(h.finish())
    }

    pub fn current_fingerprint(
        &mut self,
        doc: &Document,
        factory: Option<&crate::factory::FactoryState>,
    ) -> Option<u64> {
        self.build_job(doc, factory).map(|j| j.fingerprint())
    }

    /// How often the "is this still true?" question is asked, at most.
    ///
    /// It costs what building the scene triangles costs — a few milliseconds on a real building —
    /// so asking it every frame would spend a chunk of every frame on it for no gain. A quarter of
    /// a second is faster than anyone can move a fixture and read the panel.
    pub const STALE_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

    /// Ask whether the answer on screen still describes the scene, at most a few times a second.
    ///
    /// Returns whether anything about the answer's standing CHANGED, so the caller can repaint
    /// without repainting continuously.
    ///
    /// A DIRTY FLAG SET BY EACH EDITING PATH WOULD BE CHEAPER AND WRONG. There are dozens of ways
    /// to change a calculation's inputs — drag a fixture, delete one, edit a reflectance, pull a
    /// wall, change the ray count, import furniture, undo any of it — and every one of them would
    /// have to remember. The day somebody adds a path and forgets, the app shows an out-of-date
    /// result as a current one, silently, and there is nothing on the page to say which it is.
    /// Comparing the scene against itself cannot be forgotten.
    pub fn refresh_staleness(
        &mut self,
        doc: &Document,
        factory: Option<&crate::factory::FactoryState>,
    ) -> bool {
        let Some(was) = self.results_fingerprint else {
            return false; // nothing calculated — nothing to be stale
        };
        // THE CHEAP QUESTION FIRST, and after the first answer it is the ONLY question.
        //
        // This went straight to `current_fingerprint`, which builds a whole `CalcJob` and with it
        // every furniture triangle: 600 ms on the reference gym plan, once per frame, from the
        // moment a result existed. The throttle below could not help — the check costs more than
        // the interval, so every frame was already past it. A throttle shorter than the work it
        // guards never throttles anything.
        //
        // `scene_sig` asks the same question of the INPUTS. Once a reference is established the
        // verdict is a `u64` comparison, so a stationary scene costs nothing — and so does a
        // furniture drag, which the throttled version could never manage, because every frame of a
        // drag moved the scene and bought another full rebuild.
        let now_sig = self.scene_sig(doc, factory);
        if let (Some(n), Some(r)) = (now_sig, self.stale_ref_sig) {
            let stale = n != r;
            let changed = stale != self.results_stale;
            self.results_stale = stale;
            return changed;
        }
        let now = std::time::Instant::now();
        if let Some(t) = self.stale_checked {
            if now.duration_since(t) < Self::STALE_CHECK_INTERVAL {
                return false;
            }
        }
        self.stale_checked = Some(now);
        // A project with nothing left to calculate is NOT evidence that the answer moved on — it
        // is most often a momentary state mid-edit. Leave the standing verdict alone.
        let Some(is) = self.current_fingerprint(doc, factory) else {
            return false;
        };
        let stale = is != was;
        // ESTABLISH THE REFERENCE, so this path runs at most once per result — and only when the
        // scene MATCHES, because that is the sig the answer belongs to and every later question is
        // "has it moved away from this?". Recording a mismatching sig would make a stale result
        // look current the moment nothing else changed.
        if !stale {
            self.stale_ref_sig = now_sig;
        }
        let changed = stale != self.results_stale;
        self.results_stale = stale;
        changed
    }

    /// Adopt an answer read back from disk — but ONLY if it is about this scene.
    ///
    /// The check is exact and the failure is total: a result whose fingerprint does not match is
    /// not shown at all, not shown greyed, not shown with the rooms it still recognises. Within a
    /// session, going out of date is something the user just did and can undo, so the numbers stay
    /// on screen marked. Across a restart nobody remembers what changed, the stored rooms may not
    /// even be the rooms on the plan any more, and a wrong lux figure with a caption is still a
    /// wrong lux figure. Pressing Calculate is the cost of being sure.
    pub fn restore_results(
        &mut self,
        stored: &crate::light_store::StoredResults,
        current: u64,
    ) -> bool {
        if stored.fingerprint != current {
            return false;
        }
        let Some(rooms) = stored.rooms() else {
            return false;
        };
        let primary = &rooms[0];
        self.grid = Some(primary.grid.clone());
        self.plane = Some(primary.plane);
        self.grid_en = Some(primary.grid_en.clone());
        self.plane_en = Some(primary.plane_en);
        self.grid_mask = primary.mask.clone();
        self.cylindrical_avg = primary.cylindrical_avg;
        self.installation = primary.installation;
        self.surfaces = stored.surfaces.clone();
        self.last_timings.clear();
        self.rooms = rooms;
        self.results_fingerprint = Some(stored.fingerprint);
        // A NEW ANSWER NEEDS A NEW REFERENCE. The old one describes the scene the PREVIOUS result
        // belonged to; keeping it would answer "has it moved?" against the wrong thing.
        self.stale_ref_sig = None;
        self.results_stale = false;
        self.results_restored = true;
        self.stale_checked = None;
        self.last_msg = format!(
            "Saved result restored — {} room(s), {:.0} lx avg, U₀ {:.2}. Nothing has changed since \
             it was calculated.",
            self.rooms.len(),
            self.rooms[0].grid.avg,
            self.rooms[0].grid.u0(),
        );
        true
    }

    /// Gather the job, with no side effects on the results or the status line.
    fn build_job(
        &mut self,
        doc: &Document,
        factory: Option<&crate::factory::FactoryState>,
    ) -> Option<CalcJob> {
        let meshes = self.scene_meshes(doc, factory);
        let scene_tris: usize = meshes.iter().map(|m| m.triangles.len()).sum();

        // THE ROOM INTERIOR, NOT THE BUILDING'S BOUNDING BOX.
        //
        // Reported as: "is [it] also calculating for the solid wall (the thickness i.e) becasue i
        // see the pseuso colors there too." It was. The plane spanned `mesh_bbox` — outer wall face
        // to outer wall face — so points buried in the wall thickness, and outside the building
        // altogether on a non-rectangular plan, were computed, painted and counted in Ē and U₀.
        //
        // `mesh_bbox` stays as the fallback for a 2D-only project, which has no rooms to ask about.
        let targets = Self::calc_targets(factory);
        let any_room = targets.iter().any(|(_, p)| p.len() >= 3);
        let bounds = if any_room {
            // Every room has its own footprint; the fallback is only for a target without one.
            mesh_bbox(&meshes).or_else(|| bbox(doc))
        } else {
            (if meshes.is_empty() { None } else { mesh_bbox(&meshes) }).or_else(|| bbox(doc))
        };
        let fallback = bounds?;

        // Lights the MODEL carries — a curved light is a real fitting, not a glowing texture.
        // Derived here rather than stored, so moving or deleting the fixture takes its light along.
        // This also INSERTS their profiles, which is why it happens on the UI thread.
        let generated = match factory {
            Some(f) => self.generated_luminaires(f),
            None => Vec::new(),
        };
        let lums = if self.luminaires.is_empty() && generated.is_empty() && self.auto_center_light {
            let (min_x, min_y, max_x, max_y) = fallback;
            vec![Luminaire {
                id: 1,
                // A stand-in light needs a real profile behind it or the "first look" it exists to
                // give is a black room; the built-in is what that means.
                profile: if self.profiles.contains_key(&self.active_profile) {
                    self.active_profile.clone()
                } else {
                    BUILTIN.to_string()
                },
                position: Vertex::new(
                    0.5 * (min_x + max_x),
                    0.5 * (min_y + max_y),
                    self.room_height,
                ),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            }]
        } else {
            let mut v = self.luminaires.clone();
            v.extend(generated);
            v
        };

        Some(CalcJob {
            meshes,
            lums,
            profiles: self.profiles.clone(),
            materials: self.materials.clone(),
            settings: self.settings,
            maintenance: self.maintenance,
            obstacles: match factory {
                // One list per target, in the same order — see `CalcJob::obstacles`.
                Some(f) => targets.iter().map(|(_, p)| obstacles_in_mode(f, p, self.mode)).collect(),
                None => targets.iter().map(|_| Vec::new()).collect(),
            },
            targets,
            fallback,
            cell_size: self.cell_size,
            plane_height: self.plane_height,
            eye_height: self.eye_height,
            wall_zone: self.wall_zone,
            scene_tris,
            mode: self.mode,
        })
    }

    /// THE ONLY WAY TO REPLACE THE SCENE TRIANGLES — assigns and bumps [`Self::meshes_gen`].
    ///
    /// Writing `light.meshes` directly compiles and leaves the SIMLUX 3D view painting the previous
    /// scene until something else happens to invalidate it. Every site that sets the field goes
    /// through here so the counter cannot be forgotten at three of them and remembered at the
    /// fourth.
    pub fn set_meshes(&mut self, m: Vec<Mesh>) {
        self.meshes = m;
        self.meshes_gen = self.meshes_gen.wrapping_add(1);
    }

    /// Take the results of a finished job.
    ///
    /// `selected` is the room whose geometry the user had selected, if any — the panel shows that
    /// one, which is the gesture people already have. It does not decide what was calculated.
    pub fn apply_outcome(&mut self, out: CalcOutcome, selected: Option<usize>) {
        if out.cancelled {
            self.last_msg = "Calculation stopped.".into();
            self.set_meshes(out.meshes);
            return;
        }
        let mut results = out.rooms;
        if results.is_empty() {
            self.last_msg = "Nothing to calculate.".into();
            return;
        }
        self.surfaces = out.surfaces;
        self.last_timings = out.timings;
        // This answer now belongs to a known scene — which is what makes it worth writing down, and
        // what lets the next session tell whether it is still about this building.
        self.results_fingerprint = Some(out.fingerprint);
        // A NEW ANSWER NEEDS A NEW REFERENCE -- see `stale_ref_sig`. The next staleness check does
        // one full fingerprint to establish it, and every check after that is a u64 comparison.
        self.stale_ref_sig = None;
        self.results_mode = Some(out.mode);
        self.results_stale = false;
        self.results_restored = false;
        self.stale_checked = None;

        let primary = selected.filter(|i| *i < results.len()).unwrap_or(0);
        let waiting = self.unassigned_count();
        let p = &results[primary];
        self.last_msg = if results.len() > 1 {
            format!(
                "{} rooms · {}",
                results.len(),
                results
                    .iter()
                    .map(|r| format!("{}: {:.0} lx avg, U0 {:.2}", r.name, r.grid.avg, r.grid.u0()))
                    .collect::<Vec<_>>()
                    .join("  ·  "),
            )
        } else {
            format!(
                "{}×{} grid · avg {:.0} · min {:.0} · max {:.0} lx maintained (MF {:.2}) · U₀ {:.2}",
                p.plane.cols,
                p.plane.rows,
                p.grid.avg,
                p.grid.min,
                p.grid.max,
                p.grid.maintenance,
                p.grid.u0(),
            )
        };
        if waiting > 0 {
            self.last_msg.push_str(&format!(
                "  ⚠ {waiting} point(s) have no fitting and emit nothing — pick one in ▼ Fittings"
            ));
        }
        // A COARSENED GRID SAYS SO. Every figure moves with grid resolution, so presenting one
        // computed at a spacing nobody asked for as though it were the requested one is the part
        // that matters.
        if let Some(note) = results.iter().find_map(|r| r.grid_note.clone()) {
            self.last_msg.push_str("  ⚠ ");
            self.last_msg.push_str(&note);
        }

        let p = results.swap_remove(primary);
        self.grid = Some(p.grid.clone());
        self.plane = Some(p.plane);
        self.grid_en = Some(p.grid_en.clone());
        self.plane_en = Some(p.plane_en);
        self.grid_mask = p.mask.clone();
        self.cylindrical_avg = p.cylindrical_avg;
        self.installation = p.installation.clone();
        // Put it back where it was, so the report reads the rooms in the order they are drawn
        // rather than in the order the panel happened to want one of them.
        results.insert(primary, p);
        self.rooms = results;

        self.set_meshes(out.meshes);
        self.show_overlay = true;

        // Fit the orbit camera to everything calculated, not to one room of it.
        let (cx, cy) = {
            let n = self.rooms.len().max(1) as f32;
            let sx: f32 = self.rooms.iter().map(|r| r.plane.origin.x + r.plane.width * 0.5).sum();
            let sy: f32 = self.rooms.iter().map(|r| r.plane.origin.y + r.plane.depth * 0.5).sum();
            (sx / n, sy / n)
        };
        self.cam_target = [cx, cy, 0.5 * self.room_height];
        let span = self
            .rooms
            .iter()
            .map(|r| (r.plane.width * r.plane.width + r.plane.depth * r.plane.depth).sqrt())
            .fold(0.0_f32, f32::max);
        let diag = (span * span + self.room_height * self.room_height).sqrt();
        self.cam_dist = (diag * 1.3).max(3.0);
    }

    /// Which room the user had selected, as an index into the targets — the one the panel shows.
    pub fn selected_room(f: Option<&crate::factory::FactoryState>) -> Option<usize> {
        let f = f?;
        // BY INDEX, built the same way `calc_targets` builds them — same filter, same order.
        // Matching on the NAME instead is a trap: two rooms may share one, and an unnamed room is
        // called "Room 3" by its position, which is the very thing being looked up.
        f.rooms
            .iter()
            .filter(|r| r.footprint.len() >= 3)
            .position(|room| {
                room.floor
                    .iter()
                    .chain(room.ceiling.iter())
                    .chain(room.walls.iter())
                    .chain(room.carve.iter())
                    .any(|id| f.selection.contains(id))
            })
    }

    /// Calculate here and now, blocking until it is done.
    ///
    /// The app runs this on a worker instead — see `prepare` / `CalcJob::run` / `apply_outcome`.
    /// It stays because a test wants an answer, not a thread, and because keeping one path that
    /// does the whole thing is what makes the threaded one checkable against it.
    pub fn calculate(&mut self, doc: &Document, factory: Option<&crate::factory::FactoryState>) {
        let Some(job) = self.prepare(doc, factory) else { return };
        let progress = CalcProgress::default();
        let out = job.run(&progress);
        let selected = Self::selected_room(factory);
        self.apply_outcome(out, selected);
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
    /// WHAT THE LIVE REBUILD WOULD PRODUCE, as one number — or `None` when it cannot be summarised
    /// cheaply and the caller must just do the work.
    ///
    /// THE SIMLUX WORKSPACE REBUILT THE ENTIRE CALCULATION GEOMETRY EVERY FRAME. In split mode
    /// `render_light_3d_panel` called [`Self::rebuild_live_meshes_with`] unconditionally, and that
    /// runs `scene_meshes` → `meshes_from_factory_mode(.., Thorough)`, which transforms every
    /// furniture triangle into a fresh buffer: on the reference gym plan 7,036,129 triangles,
    /// 21.1 M vertices, about 253 MB, every frame. Measured at ~205 ms of a ~210 ms frame — the
    /// whole of the lag, and SIMLUX-only because nothing else enters split mode.
    ///
    /// It is the same mistake as the display buffer, one level upstream: that was cached, and this
    /// went on regenerating the very geometry the cache exists to avoid touching.
    ///
    /// `None` when there is no 3D model, because `scene_meshes` then falls through to the DOC
    /// extrusion and this cannot summarise a document without hashing it. That path is the cheap
    /// one — a footprint pulled to one height — so it keeps rebuilding every frame, as before.
    pub fn live_mesh_sig_of(&self, factory: Option<&crate::factory::FactoryState>) -> Option<u64> {
        let f = factory?;
        if f.cached.positions.len() < 3 && f.furniture.is_empty() {
            return None; // doc-driven; see above
        }
        let mut h = Fnv::new();
        // Express and Thorough build different furniture, so the mode is an input.
        h.u64(self.mode as u64);
        h.u64(f.geom_version);
        h.u64(f.cached.positions.len() as u64);
        // EVERY INSTANCE, THROUGH THE VERY MATRIX THE BUILD USES. `geom_version` moves on a CSG
        // rebuild and never when furniture is placed or moved, so leaving these out would freeze
        // the light scene the moment a piece was dragged.
        h.u64(f.furniture.len() as u64);
        for (i, inst) in f.furniture.iter().enumerate() {
            h.u64(inst.asset as u64);
            if let Some(m) = f.furniture_model_matrix(i) {
                for v in m {
                    h.f32(v);
                }
            }
        }
        Some(h.finish())
    }

    pub fn rebuild_live_meshes_with(
        &mut self,
        doc: &Document,
        factory: Option<&crate::factory::FactoryState>,
    ) {
        let m = self.scene_meshes(doc, factory);
        self.set_meshes(m);
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
        // WHICH BLOCKS IN THIS DRAWING ARE FITTINGS, by block NAME → profile name.
        //
        // The library itself is app-wide; this is the per-project half. Without it, reopening a
        // plan gives back blocks the drawing understands and SIMLUX does not — the symbols would
        // be there and nothing would emit light.
        //
        // Taken from the PLACED luminaires rather than from the library, because that is what the
        // project actually contains: a fitting deleted from the library after it was used must
        // still resolve in the drawings that used it.
        let mut lux_block_ies = BTreeMap::new();
        for l in &self.luminaires {
            let Some(b) = l.from_block else { continue };
            if l.profile.is_empty() {
                continue;
            }
            if let Some(blk) = doc.blocks.get(b) {
                lux_block_ies.insert(blk.name.clone(), l.profile.clone());
            }
        }
        crate::simlux_io::SimluxConfig {
            layers_3d,
            ies_library,
            lux_block_ies,
            active_profile: self.active_profile.clone(),
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
            symbol_of: self.symbol_of.clone(),
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
            // The link between each fixture and the block instance it was placed as. A project
            // written before this existed simply has none, and its fixtures stop following their
            // symbols until they are placed again — which is the honest outcome, since there is no
            // record of which of fifty identical blocks belonged to which light.
            self.symbol_of.clone_from(&cfg.symbol_of);
            repair_from_blocks(&mut self.luminaires, doc, &cfg.lux_block_ies);
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
    /// `report` is the SHARED false-colour scale — the SIMLUX window edits the very settings the
    /// report does, because it used to edit its own and nothing drew from them. See
    /// `crate::report::ui::scale_editor_ui`.
    pub fn toolbar_ui(
        &mut self,
        ui: &mut egui::Ui,
        report: &mut crate::report::Options,
        room_max: f64,
    ) -> LightAction {
        let mut action = LightAction::default();
        ui.horizontal_wrapped(|ui| {
            // ---- ① the LIBRARY of imported fittings -------------------------------------
            //
            // First on the bar because it is first in the workflow, and because a photometric
            // file is a product brought into the project exactly as a piece of furniture is.
            let waiting = self.unassigned_count();
            let fittings = self.profiles.len();
            crate::app::click_menu_button(ui, "▼ Fittings", |ui| {
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
            crate::app::click_menu_button(ui, "▼ Luminaires", |ui| {
                ui.label(egui::RichText::new("place").small().weak());
                let placing = self.place_mode;
                if ui
                    .selectable_label(placing, if placing { "◉ Placing — click the plan (Esc to stop)" } else { "＋ Place points on the plan" })
                    .on_hover_text("Click the 2D plan to mark each spot. Points stay editable: drag to move, click to select, Del to delete.")
                    .clicked()
                {
                    self.place_mode = !placing;
                    if self.place_mode {
                        self.aim_mode = false;
                        self.aim_pick = None;
                    }
                    if self.place_mode {
                        self.last_msg = "Click the plan to mark each light position · drag a marker to move it · Esc to stop.".into();
                    }
                    ui.close_menu();
                }
                // ---- AIM ------------------------------------------------------------------
                //
                // "in the luminaries tab i want a aim tool, the use of the tool will be to aim the
                // light to a point. when aim i selected the user can select a light and then click
                // on a point where they would like to point it."
                //
                // Two clicks, in that order, and the fitting does not move: "while aiming the light
                // stays at the same height and at the same location, its place where its pointed
                // downward is what we are changing."
                let aiming = self.aim_mode;
                if ui
                    .selectable_label(
                        aiming,
                        if aiming {
                            match self.aim_pick {
                                Some(_) => "◉ Aiming — click the point to aim at (Esc to stop)",
                                None => "◉ Aiming — click a light (Esc to stop)",
                            }
                        } else {
                            "⌖ Aim a light at a point"
                        },
                    )
                    .on_hover_text(
                        "Click a fitting, then click where it should point. It stays exactly where \
                         it is — only the direction changes.",
                    )
                    .clicked()
                {
                    self.aim_mode = !aiming;
                    self.aim_pick = None;
                    // Both modes want the next click on the plan, and only one can have it.
                    if self.aim_mode {
                        self.place_mode = false;
                        self.last_msg =
                            "Aim: click a fitting, then click the point it should light.".into();
                    }
                    ui.close_menu();
                }
                // AND SHOW WHERE EACH ONE POINTS.
                //
                // Asked for as: *"in aiming lights add aiming arrows that the user can turn on and
                // off in the illuminaire tab that shows where the light is aimed at."* It belongs
                // beside the aim tool rather than in Display, because it is the aim tool's readout:
                // aiming a fitting and seeing nothing change is the failure this prevents.
                ui.checkbox(&mut self.show_aim, "⌖ show aiming arrows")
                    .on_hover_text(
                        "Draw a line from each fitting along the way it points, down to where it \
                         meets the floor. Off for a crowded plan.",
                    );
                ui.separator();
                self.selected_fixture_params_ui(ui);
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
                    ui.add(egui::DragValue::new(&mut self.array_rows).update_while_editing(false).range(1..=40).prefix("rows "));
                    ui.add(egui::DragValue::new(&mut self.array_cols).update_while_editing(false).range(1..=40).prefix("cols "));
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
                        ui.add(egui::DragValue::new(&mut self.ceiling_drop).update_while_editing(false).speed(0.02).suffix(" m").range(0.0..=5.0))
                            .on_hover_text("0 = surface-mounted · 0.3 = a short pendant");
                    } else {
                        ui.label("mount at");
                        ui.add(egui::DragValue::new(&mut self.mount_height).update_while_editing(false).speed(0.05).suffix(" m").range(0.1..=30.0));
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

            crate::app::click_menu_button(ui, "▼ Calculation", |ui| {
                egui::Grid::new("simlux_calc_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    ui.label("work plane").on_hover_text("Height above the floor the lux is measured at — 0.8 m is the usual desk height");
                    ui.add(egui::DragValue::new(&mut self.plane_height).update_while_editing(false).speed(0.05).suffix(" m").range(0.0..=10.0));
                    ui.end_row();
                    ui.label("grid cell").on_hover_text("Target spacing of the measurement grid; finer is slower");
                    ui.add(egui::DragValue::new(&mut self.cell_size).update_while_editing(false).speed(0.05).suffix(" m").range(0.05..=5.0));
                    ui.end_row();
                    ui.label("eye height").on_hover_text("Height the cylindrical illuminance Ez is measured at — 1.2 m seated, 1.6 m standing");
                    ui.add(egui::DragValue::new(&mut self.eye_height).update_while_editing(false).speed(0.05).suffix(" m").range(0.3..=2.5));
                    ui.end_row();
                    ui.label("bounces").on_hover_text("Indirect light: 0 is direct only, which under-reads a bright room badly");
                    ui.add(egui::DragValue::new(&mut self.settings.max_bounces).update_while_editing(false).range(0..=8));
                    ui.end_row();
                    ui.label("rays").on_hover_text("Samples per point for the indirect term — more is smoother and slower");
                    ui.add(egui::DragValue::new(&mut self.settings.rays_per_point).update_while_editing(false).range(1..=4096));
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
                        ui.add(egui::DragValue::new(v).update_while_editing(false).speed(0.005).range(0.1..=1.0).fixed_decimals(2));
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

            crate::app::click_menu_button(ui, "▼ Surfaces", |ui| {
                ui.label(egui::RichText::new("reflectance — how much light a surface returns").small().weak());
                egui::Grid::new("simlux_mat_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    for m in &mut self.materials {
                        ui.label(&m.name);
                        ui.add(egui::DragValue::new(&mut m.reflectance).update_while_editing(false).speed(0.01).range(0.0..=1.0));
                        ui.end_row();
                    }
                });
                ui.label(
                    egui::RichText::new("Surfaces are sorted by which way they face:\nup = floor, down = ceiling, upright = wall.")
                        .small().weak(),
                );
            });

            crate::app::click_menu_button(ui, "▼ Display", |ui| {
                ui.checkbox(&mut self.floor_heatmap, "false-colour on the floor")
                    .on_hover_text(
                        "Paint the calculated illuminance over the floor of the 3D view, in the \
                         report's own scale and band colours.",
                    );
                ui.checkbox(&mut self.show_isolux, "isolux lines")
                    .on_hover_text(
                        "Trace the band thresholds as curves. A colour says roughly how much light \
                         there is; a line says exactly where a number is.",
                    );
                ui.checkbox(&mut self.hide_ceilings, "hide ceilings")
                    .on_hover_text(
                        "Drop the ceiling so the room can be seen into from above. The result is \
                         painted on the FLOOR, and a closed box hides the surface this view exists \
                         to show.",
                    );
                ui.separator();
                // ---- THE SCALE, WHICH IS THE REPORT'S ------------------------------------
                //
                // This menu used to carry its own "auto / pin top" and its own palette, over state
                // that nothing drew from any more — turning them changed nothing, which is
                // indistinguishable from broken and was reported as exactly that. There is one
                // editor now, and it edits the settings the report and both views all read.
                crate::report::ui::scale_editor_ui(ui, report, room_max, self.ramp.rgb_fn());
                // THE PALETTE, ONLY WHEN IT CAN CHANGE ANYTHING.
                //
                // "does this even do anything? it seems to be redundant." Fair, and nearly right:
                // the default scale has four thresholds — five bands — and five band colours, so
                // every band has an explicit colour and the palette is never consulted. It is only
                // reached when the scale is CONTINUOUS, which has no bands at all, or when a band
                // has been left without a colour of its own.
                //
                // Both of those are real, so the control is not dead. But showing it permanently
                // says it is live when it usually is not, and a control that does nothing when you
                // turn it is the same complaint as a control that is broken. Shown when it bites.
                if self.palette_is_in_play(report, room_max) {
                    ui.separator();
                    ui.label(egui::RichText::new("Fallback palette").small().weak())
                        .on_hover_text(
                            "In use right now: the scale is not banded, or a band has no colour of \
                             its own. Give every band a colour and this stops mattering.",
                        );
                    let cur = self.ramp;
                    egui::ComboBox::from_id_salt("lux_ramp")
                        .width(190.0)
                        .selected_text(cur.label())
                        .show_ui(ui, |ui| {
                            for r in LuxRamp::ALL {
                                ui.selectable_value(&mut self.ramp, r, r.label());
                            }
                        });
                }
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
            // THE SAME GRID THE REPORT QUOTES — the standard's. See `reported_grid`.
            if let Some(g) = self.reported_grid() {
                let en = self.reporting_en_grid();
                ui.label(small(format!("· avg {:.0} lx", g.avg)));
                ui.label(small(format!("· min {:.0}", g.min)));
                // THE MAXIMUM CARRIES ITS GRID, on hover.
                //
                // Reported as: *"the max lux differs by a lot from the dialux and relux values.
                // why is that?"* A maximum is the brightest grid POINT, not the brightest place in
                // the room — a peak under a downlight is a few cells across, so a coarser grid can
                // miss it while the average barely moves. Saying so where the number is is what
                // stops the question being asked again.
                ui.label(small(format!("· max {:.0}", g.max))).on_hover_text(
                    "The brightest GRID POINT, not the brightest place in the room. A peak under a \
                     downlight is only a few cells across, so a coarser grid may never sample it — \
                     while the average hardly moves. This is why two packages can report different \
                     maxima for the same scheme.",
                );
                // WHICH GRID, said rather than assumed.
                if en {
                    ui.label(
                        egui::RichText::new(format!("· EN grid {}×{}", g.cols, g.rows))
                            .small()
                            .color(egui::Color32::from_rgb(150, 200, 150)),
                    )
                    .on_hover_text(
                        "Figures are quoted on EN 12464-1's own grid — the spacing a compliance \
                         claim rests on, and the one DIALux and Relux default closest to. The \
                         false-colour field is still drawn from the finer calculated grid: how \
                         coarsely a result may be DRAWN is not something the standard sets.",
                    );
                }
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
                            egui::DragValue::new(&mut g.height).update_while_editing(false)
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
                action.remove_fixture = Some(id);
            }
            if ui.button("Clear all fixtures").clicked() {
                action.clear_fixtures = true;
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
        //
        // THE MODE SITS NEXT TO THE BUTTON IT CHANGES, so nobody presses Calculate without seeing
        // which one they are about to run.
        ui.horizontal(|ui| {
            ui.label("Detail:");
            ui.selectable_value(&mut self.mode, CalcMode::Express, "Express").on_hover_text(
                "Furniture as the box it occupies. Same rays, same bounces, same grid — only the \
                 furniture is simplified. For trying a layout; not a compliance figure.",
            );
            ui.selectable_value(&mut self.mode, CalcMode::Thorough, "Thorough").on_hover_text(
                "Every triangle of every piece, plus the room-surface report. The answer to put in \
                 front of a client.",
            );
        });
        if ui
            .add(egui::Button::new(
                egui::RichText::new(format!("  Calculate ({})  ", self.mode.label())).strong(),
            ))
            .clicked()
        {
            action.calculate = true;
        }
        // WHAT IS ON SCREEN, not what the switch says. Flipping to Thorough does not make the
        // Express numbers below it Thorough ones, and this is the line that says so.
        if self.results_mode == Some(CalcMode::Express) {
            ui.label(
                egui::RichText::new(
                    "⚠  Express result — furniture simplified to boxes. Not an EN 12464-1 \
                     compliance figure.",
                )
                .small()
                .color(egui::Color32::from_rgb(226, 160, 60)),
            );
        }
        ui.checkbox(&mut self.show_overlay, "Show lux overlay on 2D plan");
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.view3d_open, "3D view");
            ui.checkbox(&mut self.floor_heatmap, "Heatmap floor");
            ui.checkbox(&mut self.show_isolux, "Isolux lines");
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
            // OUT OF DATE, SAID BEFORE THE NUMBERS AND NOT AFTER THEM.
            //
            // A stale result and a current one are the same numbers in the same places; nothing
            // about 412 lx says which building it came from. Since the result is KEPT rather than
            // wiped — so an accidental nudge does not cost a seventy-second answer — this label is
            // the only thing standing between somebody and quoting a figure for a layout they have
            // already changed. It goes above the figures because a caption underneath is a caption
            // read after the number has been written down.
            if self.results_stale {
                ui.label(
                    egui::RichText::new(
                        "⚠ OUT OF DATE — the lights, the model or the settings have changed since \
                         this was calculated. Press Calculate to bring it up to date.",
                    )
                    .small()
                    .strong()
                    .color(egui::Color32::from_rgb(235, 140, 90)),
                );
            } else if self.results_restored {
                ui.label(
                    egui::RichText::new(
                        "Restored from the last saved calculation — nothing has changed since.",
                    )
                    .small()
                    .weak(),
                );
            }
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
                // AND THE SAME ROOM ON THE STANDARD'S OWN GRID.
                //
                // The row above says which grid the figure came from; this one gives the figure a
                // compliance claim actually rests on, because EN 12464-1 specifies the grid it is
                // to be assessed on and that grid is COARSER than the working one for every room
                // down to about 3 m — 1.94 m across a 33 m hall.
                //
                // Reported BESIDE the working figure and never instead of it. The working grid is
                // the finer of the two, so its U₀ is the conservative number; swapping them would
                // raise every uniformity in every project at once, which is the direction that
                // passes an installation it should not. A designer needs both: one says how even
                // the room really is, the other says whether it complies.
                if let (Some(ge), Some(pe)) = (self.grid_en.as_ref(), self.plane_en.as_ref()) {
                    row(
                        ui,
                        "…to EN 12464-1",
                        format!("U₀ {:.2}   on {}", ge.u0(), pe.grid_note()),
                    );
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
            // The legend is drawn by the caller, which holds the report's scale — see
            // `band_legend`. A legend in a different scheme from the picture it explains is worse
            // than no legend, and this one used to be exactly that.
            // (intentionally nothing here)
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

/// THE LEGEND FOR THE SCALE THE VIEWS ARE ACTUALLY DRAWN IN — the report's.
///
/// Reported as: *"change the 3d and 2d false colors to reports bands."* The legend has to move with
/// them: a legend in a different scheme from the picture it explains is worse than no legend, and
/// this one was still a blue→red gradient to 1802 lx over a drawing banded at 50 · 100 · 200 · 300.
///
/// Banded, this draws one block per band with its floor written underneath and the top one left
/// open-ended, because that is what the drawing does. Unbanded, it falls back to the gradient bar,
/// which is the honest picture of a continuous scale.
pub fn band_legend(
    ui: &mut egui::Ui,
    opt: &crate::report::Options,
    room_max: f64,
    ramp: LuxRamp,
) {
    if opt.scale.bands.is_empty() {
        legend_bar_with(ui, opt.scale.top_lx(room_max), ramp);
        return;
    }
    let edges = opt.scale.edges(room_max);
    let n = edges.len().saturating_sub(1);
    if n == 0 {
        return;
    }
    let (resp, painter) = ui.allocate_painter(egui::vec2(240.0, 16.0), egui::Sense::hover());
    let rect = resp.rect;
    let w = rect.width() / n as f32;
    for k in 0..n {
        let mid = (edges[k] + edges[k + 1]) * 0.5;
        let c = opt.lux_rgb(mid, room_max, ramp.rgb_fn());
        let x0 = rect.left() + w * k as f32;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x0, rect.top()),
                egui::pos2(x0 + w, rect.bottom()),
            ),
            0.0,
            egui::Color32::from_rgb(c[0], c[1], c[2]),
        );
    }
    // The FLOOR of each band, under its block — the number that decides which side of the edge a
    // reading falls on. The top band is open, and says so rather than naming the room's peak.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for k in 0..n {
            let label = if k + 1 == n {
                format!("{:.0}+", edges[k])
            } else {
                format!("{:.0}", edges[k])
            };
            ui.allocate_ui(egui::vec2(w, 12.0), |ui| {
                ui.label(egui::RichText::new(label).small().weak());
            });
        }
    });
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


/// THE GRID THE UI PROMISES IS THE GRID THAT GETS CALCULATED — or the app says otherwise.
///
/// The cell size was clamped PER AXIS to at most 64 cells. On a room longer than 64 cells that
/// silently coarsened the long axis and left the short one alone, so the grid came out
/// RECTANGULAR while every number on screen described a square one. Min, average and uniformity
/// all move with grid resolution, so this changed figures people put in front of clients.
///
/// Measured on the owner's real project — a 33 × 13 m gym at the default 0.25 m cell: 64 × 52
/// cells, which is 0.52 m spacing along x and 0.25 m along y. Twice as coarse one way as the
/// other, with the UI saying 0.25 throughout.

/// UNIFORMITY IS QUOTED WITH THE GRID IT WAS MEASURED ON — both of them.
///
/// U₀ is not a property of a room. It is a property of a room AND the grid it was sampled on, and
/// the two grids that matter here are not the same grid:
///
///   * the WORKING grid, at whatever cell size the designer set — 0.25 m by default;
///   * the EN 12464-1 grid, whose spacing GROWS with the room (1.94 m across a 33 m hall).
///
/// The working grid is the finer of the two for every room down to about 3 m, so the figure the
/// panel has always shown is the CONSERVATIVE one — a finer grid catches a lower minimum. The
/// standard's figure is the one a compliance claim rests on, and it is the higher of the two.
/// Neither replaces the other, which is why both are reported and each is labelled.
///
/// `CalcPlane::on_standard_grid` existed for this, documented as "deliberately SEPARATE from the
/// plane you display", unit-tested inside `cad_light` — and called from nowhere in the app.
#[cfg(test)]
mod uniformity_is_quoted_with_its_grid {
    use super::*;

    /// A plain rectangular room, lit, with the working grid at `cell` metres.
    fn lit_room(w: f32, d: f32, cell: f32) -> LightState {
        let mut f = crate::factory::FactoryState::default();
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(w, 0.0),
            glam::Vec2::new(w, d),
            glam::Vec2::new(0.0, d),
            glam::Vec2::new(0.0, 0.0),
        ];
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.recompute();

        let mut s = LightState::new();
        s.cell_size = cell;
        // REAL FITTINGS, not the auto-centred stand-in. That one is a convenience for a first
        // look and its height follows the room's; a uniformity test needs a layout it controls,
        // and one that is deliberately NOT perfectly even — a room lit to a flat sheet has
        // U₀ ≈ 1 on every grid and could not tell two grids apart at all.
        s.auto_center_light = false;
        for i in 0..3 {
            for j in 0..2 {
                s.luminaires.push(Luminaire {
                    id: (i * 2 + j + 1) as u32,
                    profile: BUILTIN.to_string(),
                    position: Vertex::new(
                        w * (i as f32 + 0.5) / 3.0,
                        d * (j as f32 + 0.5) / 2.0,
                        2.7,
                    ),
                    rotation_deg: 0.0,
                    tilt_deg: 0.0,
                    dimming: 1.0,
                    watts_override: None,
                    flux_override: None,
                    from_block: None,
                });
            }
        }
        s.calculate(&cad_kernel::Document::default(), Some(&f));
        s
    }

    /// THE SECOND GRID IS THE STANDARD'S, not a copy of the first at a different name.
    #[test]
    fn the_second_plane_is_the_one_en_12464_1_asks_for() {
        let s = lit_room(20.0, 10.0, 0.25);
        let p = s.plane.as_ref().expect("a working plane");
        let e = s.plane_en.as_ref().expect("a standard plane");
        let (wc, wr) = cad_light::en12464_cells(p.width, p.depth);
        assert_eq!(
            (e.cols, e.rows), (wc, wr),
            "the standard plane is {} × {}, but EN 12464-1 asks {wc} × {wr}",
            e.cols, e.rows,
        );
        assert_ne!(
            (e.cols, e.rows), (p.cols, p.rows),
            "the fixture must have two DIFFERENT grids or this test proves nothing",
        );
    }

    /// THE WORKING FIGURE IS UNTOUCHED. The whole point of adding rather than switching: no number
    /// anybody has already reported changes.
    #[test]
    fn the_working_uniformity_is_still_measured_on_the_working_grid() {
        let s = lit_room(20.0, 10.0, 0.25);
        let p = s.plane.as_ref().expect("a working plane");
        let (cols, rows) = LightState::grid_for(p.width, p.depth, 0.25);
        assert_eq!(
            (p.cols, p.rows), (cols, rows),
            "the working plane stopped being the one the cell size asks for",
        );
        let g = s.grid.as_ref().expect("a working grid");
        assert_eq!(
            (g.cols, g.rows), (cols, rows),
            "the working figures are no longer computed on the working grid",
        );
    }

    /// THE DIRECTION, ASSERTED RATHER THAN BELIEVED.
    ///
    /// The recommendation to report both rather than switch rests on the standard's grid being
    /// COARSER, and a coarser grid reporting uniformity HIGHER — it has fewer chances to land on
    /// the room's true minimum. If that is ever the other way round, quoting the EN figure is
    /// tightening a design rather than flattering it, and the advice was wrong.
    #[test]
    fn the_standards_coarser_grid_reports_the_higher_uniformity() {
        let s = lit_room(20.0, 10.0, 0.25);
        let p = s.plane.as_ref().expect("plane");
        let e = s.plane_en.as_ref().expect("standard plane");
        assert!(
            e.cols < p.cols && e.rows < p.rows,
            "precondition: the standard's grid must be the coarser one — {} × {} against {} × {}",
            e.cols, e.rows, p.cols, p.rows,
        );
        let (g, ge) = (s.grid.as_ref().expect("grid"), s.grid_en.as_ref().expect("standard grid"));
        assert!(g.avg > 1.0, "precondition: the room is lit ({:.1} lx)", g.avg);
        assert!(
            ge.u0() >= g.u0() - 1e-6,
            "the coarser standard grid reported U₀ {:.3}, BELOW the working grid's {:.3} — \
             the direction this whole choice rests on is wrong",
            ge.u0(), g.u0(),
        );
    }

    /// AND IT IS MASKED TO THE ROOM TOO. The working grid drops cells outside an L-shaped room,
    /// because averaging in the outside of its own corner made U₀'s minimum a point in open air.
    /// A second grid that skipped that would carry exactly the defect the first one was fixed for.
    ///
    /// TESTED BY CALCULATING, not by reading the source. This used to search the text of
    /// `calculate` for `apply_room_mask(&mut grid_en` — which passed for the right reason until
    /// the calculation was split to handle several rooms, and then failed while the masking it
    /// names was still happening, one function along. A test that greps for an implementation can
    /// only ever report where the code is, not what it does.
    #[test]
    fn the_standard_grid_is_masked_to_the_room_like_the_working_one() {
        // An L: 12 x 12 with the top-right 6 x 6 removed, lit only in the tall leg — so the
        // missing corner is the darkest ground the rectangle covers.
        let poly = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(12.0, 0.0),
            glam::Vec2::new(12.0, 6.0),
            glam::Vec2::new(6.0, 6.0),
            glam::Vec2::new(6.0, 12.0),
            glam::Vec2::new(0.0, 12.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&poly, 3.0).expect("building");
        f.add_room(&poly).expect("room");
        f.recompute();

        let mut s = LightState::new();
        s.auto_center_light = false;
        s.cell_size = 0.5;
        for (i, (x, y)) in [(3.0, 3.0), (9.0, 3.0), (3.0, 9.0)].into_iter().enumerate() {
            s.luminaires.push(Luminaire {
                id: i as u32 + 1,
                profile: BUILTIN.to_string(),
                position: Vertex::new(x, y, 2.7),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            });
        }
        s.calculate(&Document::default(), Some(&f));

        let ge = s.grid_en.as_ref().expect("a standard grid");
        let en = s.plane_en.as_ref().expect("a standard plane");
        // The cut-out corner is inside the RECTANGLE and outside the ROOM. Its cells are the
        // darkest the rectangle covers, so an unmasked minimum lands there.
        let mask = LightState::inside_mask(en, &poly);
        assert!(
            mask.iter().any(|k| !*k),
            "precondition: the standard grid must cover ground outside the L",
        );
        let outside_min = ge
            .values
            .iter()
            .zip(mask.iter())
            .filter(|(_, inside)| !**inside)
            .map(|(v, _)| *v)
            .fold(f64::MAX, f64::min);
        assert!(
            ge.min > outside_min + 1e-9,
            "the standard grid's minimum is {:.3} lx, which is the darkest cell OUTSIDE the room \
             ({outside_min:.3} lx) — it was not masked",
            ge.min,
        );
        // …and the room's own minimum is what it reports.
        let inside_min = ge
            .values
            .iter()
            .zip(mask.iter())
            .filter(|(_, inside)| **inside)
            .map(|(v, _)| *v)
            .fold(f64::MAX, f64::min);
        assert!(
            (ge.min - inside_min).abs() < 1e-6,
            "reported {:.3} lx, the room's darkest cell is {inside_min:.3} lx",
            ge.min,
        );
    }

    /// A ROOM WHERE THE TWO GRIDS COINCIDE STILL REPORTS BOTH. At 3 m square the standard asks for
    /// 0.25 m and the default working cell IS 0.25 m — the figures agree, and a reader who sees
    /// only one line cannot tell that from a missing calculation.
    #[test]
    fn both_are_reported_even_when_the_two_grids_agree() {
        let s = lit_room(3.0, 3.0, 0.25);
        assert!(s.grid.is_some() && s.grid_en.is_some(), "both grids must be present");
        let p = s.plane.as_ref().unwrap();
        let e = s.plane_en.as_ref().unwrap();
        assert_eq!((p.cols, p.rows), (e.cols, e.rows), "the fixture's two grids must coincide");
    }
}
#[cfg(test)]
mod the_grid_is_the_one_the_ui_says {
    use super::*;

    /// The two cell spacings a `w` × `d` plane actually gets, in metres.
    fn spacing(w: f32, d: f32, cell: f32) -> (f32, f32) {
        let (cols, rows) = LightState::grid_for(w, d, cell);
        (w / cols as f32, d / rows as f32)
    }

    /// THE OWNER'S ROOM. Stated on its own, with its own numbers, because this is the case the
    /// clamp was found on and a property test can drift away from it.
    #[test]
    fn the_gym_is_not_sampled_twice_as_coarsely_along_its_length() {
        let (sx, sy) = spacing(33.0, 13.0, 0.25);
        assert!(
            (sx / sy - 1.0).abs() < 0.05,
            "a 33 × 13 m room came out {sx:.3} m along x and {sy:.3} m along y — the grid is \
             rectangular and every figure derived from it is an average over two resolutions",
        );
    }

    /// SQUARE, WHATEVER THE ROOM. The general form: no aspect ratio may produce a grid whose two
    /// spacings differ, because a cell size is one number and the UI shows one number.
    #[test]
    fn every_room_gets_a_square_grid() {
        for (w, d) in [
            (33.0_f32, 13.0_f32), (100.0, 4.0), (4.0, 100.0), (7.0, 7.0),
            (250.0, 60.0), (1.0, 40.0), (0.5, 0.5),
        ] {
            let (sx, sy) = spacing(w, d, 0.25);
            assert!(
                (sx / sy - 1.0).abs() < 0.06,
                "a {w} × {d} m room is sampled at {sx:.3} m by {sy:.3} m",
            );
        }
    }

    /// WHEN IT FITS, IT IS HONOURED EXACTLY. A budget that quietly coarsened everything would
    /// satisfy "square" and still be wrong; this is the half that stops that.
    #[test]
    fn a_room_that_fits_gets_the_cell_size_it_asked_for() {
        for (w, d, cell) in [(8.0_f32, 6.0_f32, 0.25_f32), (12.0, 10.0, 0.5), (33.0, 13.0, 1.0)] {
            let (sx, sy) = spacing(w, d, cell);
            assert!(
                (sx - cell).abs() < cell * 0.05 && (sy - cell).abs() < cell * 0.05,
                "a {w} × {d} m room asked for {cell} m and got {sx:.3} × {sy:.3}",
            );
        }
    }

    /// A TINY ROOM STILL GETS ENOUGH POINTS TO SAY ANYTHING. A 1 m cupboard at a 1 m cell would
    /// otherwise be one sample, and a minimum and an average over one point are the same number.
    #[test]
    fn a_small_room_is_still_sampled_enough_to_have_statistics() {
        let (cols, rows) = LightState::grid_for(1.0, 1.0, 1.0);
        assert!(
            cols as u64 * rows as u64 >= LightState::MIN_GRID_POINTS,
            "a 1 m room got a {cols} × {rows} grid",
        );
    }

    /// …AND THE FLOOR DOES NOT BEND THE CELL OUT OF SQUARE. It used to be 8 cells PER AXIS, so a
    /// 1 × 40 m corridor had its short side forced to 0.125 m while its long side sat at the
    /// requested 0.25 m — the floor firing in the one case where nothing needed to change, because
    /// 4 × 160 is already 640 samples. Statistics come from how many samples there are, not from
    /// how many lie along each edge.
    #[test]
    fn a_long_corridor_is_not_refined_on_its_short_side_alone() {
        let (sx, sy) = spacing(1.0, 40.0, 0.25);
        assert!(
            (sx - 0.25).abs() < 0.02 && (sy - 0.25).abs() < 0.02,
            "a 1 × 40 m corridor asked for 0.25 m and got {sx:.3} × {sy:.3}",
        );
    }

    /// AND THE COST IS STILL BOUNDED. Every point is a full trace against the scene, so an
    /// unbounded grid on a site plan is a calculation that never finishes — which is the reason
    /// the per-axis clamp existed and is not the part that was wrong with it.
    #[test]
    fn a_site_sized_plan_cannot_ask_for_an_unbounded_calculation() {
        let (cols, rows) = LightState::grid_for(400.0, 300.0, 0.05);
        let points = cols as u64 * rows as u64;
        assert!(
            points <= LightState::MAX_GRID_POINTS,
            "a 400 × 300 m plan at 50 mm asked for {points} points",
        );
    }

    /// AND WHEN THE BUDGET BITES, THE APP SAYS SO. Silence is the actual defect here: a coarser
    /// grid is a defensible answer to an enormous room, and presenting it as the requested one is
    /// not. The message names the spacing that was really used.
    #[test]
    fn coarsening_the_grid_is_reported_not_hidden() {
        let mut s = LightState::new();
        s.cell_size = 0.05;
        let note = s.grid_note(400.0, 300.0);
        let note = note.expect("a 400 × 300 m plan at 50 mm must be coarsened, and must say so");
        assert!(
            note.contains("0.05") && note.contains('m'),
            "the note must name the cell size that was asked for: {note:?}",
        );
        let (sx, sy) = spacing(400.0, 300.0, 0.05);
        let used = format!("{:.2}", sx.max(sy));
        assert!(
            note.contains(&used),
            "the note must name the spacing actually used ({used} m): {note:?}",
        );
    }

    /// …AND SAYS NOTHING WHEN IT DID WHAT WAS ASKED. A note on every calculation is a note
    /// nobody reads.
    #[test]
    fn a_grid_that_was_honoured_is_not_announced() {
        let mut s = LightState::new();
        s.cell_size = 0.25;
        assert_eq!(s.grid_note(8.0, 6.0), None, "an ordinary room must not be flagged");
    }
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
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
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
        // `build_scene3d_verts` split in two when the SIMLUX view stopped rebuilding its whole
        // buffer every frame; the ceiling filter lives in the cached half. The end anchor was
        // `\n    /// SIMLUX 3D viewport`, which is a FIELD doc a few thousand lines EARLIER in the
        // file — searching forward from the function never found it, so `body` was silently the
        // whole rest of `app.rs` and the two assertions below were close to vacuous. Anchored on
        // the next item instead, so this really does read one function.
        let a = src.find("fn build_scene3d_static").expect("the SIMLUX scene builder");
        let b = src[a..].find("\n    /// WHERE EVERY FITTING IS POINTING").map(|e| a + e)
            .expect("the item that follows it — re-anchor this if either is renamed");
        let body = &src[a..b];
        assert!(body.len() < 4_000, "the slice must be ONE function, not the rest of the file");
        // THIS ASSERTION USED TO BE THE WHOLE TEST, and it passed for the entire life of a toggle
        // that did nothing: `build_scene_verts` dropped the ceiling unconditionally over in
        // light3d.rs, which this test cannot see — and could not have observed in any case, because
        // it greps a string instead of running the code. The behaviour is now covered where it
        // belongs: `light3d::the_viewer_draws_what_it_is_given`, proven to fail against the old
        // code, and `hiding_the_ceiling_opens_the_room`, which measures the area of the lid.
        assert!(body.contains("hide_ceilings"), "the view must still consult the flag");
        // The filter has to be on a COPY for drawing. If `self.light.meshes` itself were pruned,
        // the next Calculate would run on a room with no ceiling.
        assert!(
            !body.contains("self.light.meshes.retain") && !body.contains("meshes.retain"),
            "the scene meshes must not be pruned in place — Calculate reads them",
        );
    }
}

/// HIDING THE CEILING IN SIMLUX MUST ACTUALLY OPEN THE ROOM.
///
/// Reported twice: "hide ceiling in simlux doesnt work". The view filtered by MATERIAL, and
/// material is assigned by ORIENTATION — so a ceiling slab lost its underside (material 2) and kept
/// its top face (material 0, floor). Looking down, the room was still lidded.
#[cfg(test)]
mod hiding_the_ceiling_opens_the_room {
    use super::*;

    /// A building with a room carved out of it, so there is a real ceiling slab AND a roof.
    fn a_building() -> crate::factory::FactoryState {
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(
            &[
                glam::Vec2::new(0.0, 0.0),
                glam::Vec2::new(10.0, 0.0),
                glam::Vec2::new(10.0, 8.0),
                glam::Vec2::new(0.0, 8.0),
                glam::Vec2::new(0.0, 0.0),
            ],
            3.0,
        )
        .expect("building");
        f.add_room(&[
            glam::Vec2::new(1.0, 1.0),
            glam::Vec2::new(9.0, 1.0),
            glam::Vec2::new(9.0, 7.0),
            glam::Vec2::new(1.0, 7.0),
            glam::Vec2::new(1.0, 1.0),
        ])
        .expect("room");
        f.recompute();
        f
    }

    /// The area of every UPWARD-facing triangle above the working plane — the "lid" you are looking
    /// through from above. Hiding the ceiling has to remove most of it; the OLD material filter
    /// removed none of it, which is the whole bug.
    fn lid_area(meshes: &[Mesh]) -> f64 {
        let mut a = 0.0;
        for m in meshes {
            for t in &m.triangles {
                let p = |i: u32| {
                    let v = m.vertices[i as usize];
                    glam::Vec3::new(v.x, v.y, v.z)
                };
                let (x, y, z) = (p(t.a), p(t.b), p(t.c));
                let cr = (y - x).cross(z - x);
                let n = cr.normalize_or_zero();
                // Upward-facing and above head height: this is a lid, not a floor.
                if n.z > 0.7 && x.z.min(y.z).min(z.z) > 2.0 {
                    a += (cr.length() * 0.5) as f64;
                }
            }
        }
        a
    }

    /// THE BUG. Filtering by material leaves the lid in place.
    #[test]
    fn the_old_material_filter_did_not_open_the_room() {
        let f = a_building();
        let all = meshes_from_factory(&f);
        let by_material: Vec<Mesh> = all.iter().filter(|m| m.material != 2).cloned().collect();
        let before = lid_area(&all);
        assert!(before > 10.0, "precondition: there is a lid to remove, got {before:.1} m2");
        assert!(
            (lid_area(&by_material) - before).abs() < 1e-6,
            "the material filter removes NONE of the lid — that is the reported bug",
        );
    }

    /// THE FIX. Filtering by feature takes the lid with it.
    #[test]
    fn filtering_by_feature_opens_the_room() {
        let f = a_building();
        let before = lid_area(&meshes_from_factory(&f));
        let after = lid_area(&meshes_from_factory_ex(&f, Some(0.8)));
        println!("lid above 2 m: {before:.2} m2 -> {after:.2} m2");
        assert!(
            after < before * 0.25,
            "hiding the ceiling must open the room: {before:.2} m2 of lid became {after:.2} m2",
        );
    }

    /// …and the walls and floor must survive. An "open" room with no walls is a different bug.
    #[test]
    fn the_rest_of_the_building_survives() {
        let f = a_building();
        let all = meshes_from_factory(&f);
        let open = meshes_from_factory_ex(&f, Some(0.8));
        let tris = |m: &[Mesh], mat: u32| {
            m.iter().filter(|x| x.material == mat).map(|x| x.triangles.len()).sum::<usize>()
        };
        // The walls are what you look INTO the room past, so they must survive. Not bit-identical
        // though: a ceiling slab has vertical edge faces, which are bucketed as wall and go with
        // the slab. Measuring "most of it" is the honest assertion — the first version of this test
        // demanded equality and failed on exactly those edge faces.
        let (wall_all, wall_open) = (tris(&all, 1), tris(&open, 1));
        assert!(
            wall_open as f64 > wall_all as f64 * 0.7,
            "the walls must survive: {wall_all} triangles became {wall_open}",
        );
        assert!(tris(&open, 0) > 0, "the floor must still be there — it is what the result is on");
    }

    /// THE CALCULATION MUST STILL SEE THE CEILING. It is around 70 % of the interreflection, and a
    /// view option that changed the answer would be a trap.
    #[test]
    fn the_unfiltered_build_is_unchanged() {
        let f = a_building();
        let plain = meshes_from_factory(&f);
        let ceil = plain.iter().filter(|m| m.material == 2).map(|m| m.triangles.len()).sum::<usize>();
        assert!(ceil > 0, "the default build must still contain the ceiling for Calculate");
    }
}

/// The `(min_x, min_y, max_x, max_y)` of a polygon.
fn poly_bounds(p: &[glam::Vec2]) -> (f32, f32, f32, f32) {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for v in p {
        x0 = x0.min(v.x);
        y0 = y0.min(v.y);
        x1 = x1.max(v.x);
        y1 = y1.max(v.y);
    }
    (x0, y0, x1, y1)
}

/// THE WORKING PLANE BELONGS TO THE ROOM, NOT TO THE BOUNDING BOX.
///
/// Reported as: "is [it] also calculating for the solid wall (the thickness i.e) becasue i see the
/// pseuso colors there too." It was: the plane spanned `mesh_bbox`, outer wall face to outer wall
/// face, so points buried in the wall thickness were computed, painted, and counted in Ē and U₀.
#[cfg(test)]
mod the_plane_is_the_room {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<glam::Vec2> {
        vec![
            glam::Vec2::new(x0, y0),
            glam::Vec2::new(x1, y0),
            glam::Vec2::new(x1, y1),
            glam::Vec2::new(x0, y1),
            glam::Vec2::new(x0, y0),
        ]
    }

    /// A building with ONE room carved out of it: the plane must follow the room, not the building.
    #[test]
    fn the_plane_follows_the_room_not_the_building() {
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect(0.0, 0.0, 10.0, 8.0), 3.0).expect("building");
        f.add_room(&rect(1.0, 1.0, 9.0, 7.0)).expect("room");
        f.recompute();

        let poly = LightState::calc_room_polygon(&f).expect("one room, so no ambiguity");
        let b = poly_bounds(&poly);
        assert!((b.0 - 1.0).abs() < 1e-4 && (b.2 - 9.0).abs() < 1e-4, "x {:?}", (b.0, b.2));
        assert!((b.1 - 1.0).abs() < 1e-4 && (b.3 - 7.0).abs() < 1e-4, "y {:?}", (b.1, b.3));
        // …and NOT the building, which is what it used to be.
        let (bmn, bmx) = f.cached.bounds().expect("geometry");
        assert!(bmn[0] < b.0 - 0.5, "the building really is wider than the room");
        assert!(bmx[0] > b.2 + 0.5);
    }

    /// The wall zone insets it, and states the condition the way DIALux states it.
    #[test]
    fn the_wall_zone_insets_the_plane() {
        let mut s = LightState::new();
        s.wall_zone = 0.5;
        let b = s.inset_bounds((0.0, 0.0, 10.0, 8.0));
        assert_eq!(b, (0.5, 0.5, 9.5, 7.5));
    }

    /// A zone wider than the room would invert the rectangle. Better the whole room than a plane
    /// turned inside out.
    #[test]
    fn an_over_wide_zone_is_refused_rather_than_inverted() {
        let mut s = LightState::new();
        s.wall_zone = 3.0;
        assert_eq!(s.inset_bounds((0.0, 0.0, 4.0, 4.0)), (0.0, 0.0, 4.0, 4.0));
    }

    /// AN L-SHAPED ROOM must not average in the outside of its own corner. The rectangular grid
    /// covers it; the mask is what keeps it out of the numbers.
    #[test]
    fn an_l_shaped_room_masks_out_its_own_corner() {
        // An L: 10 x 10 with the top-right 5 x 5 removed.
        let poly = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(10.0, 0.0),
            glam::Vec2::new(10.0, 5.0),
            glam::Vec2::new(5.0, 5.0),
            glam::Vec2::new(5.0, 10.0),
            glam::Vec2::new(0.0, 10.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let plane = CalcPlane {
            origin: cad_light::Vertex::new(0.0, 0.0, 0.8),
            width: 10.0,
            depth: 10.0,
            cols: 10,
            rows: 10,
        };
        let mask = LightState::inside_mask(&plane, &poly);
        assert_eq!(mask.len(), 100);
        let inside = mask.iter().filter(|k| **k).count();
        assert_eq!(inside, 75, "the L is three quarters of its own bounding box, got {inside}");
    }

    /// The mask re-derives the room's figures. The cut-out cells here are bright, so leaving them
    /// in would OVERSTATE the room — the direction that matters, since it passes a design.
    #[test]
    fn the_masked_average_is_the_rooms_average() {
        let mut g = cad_light::LuxGrid {
            cols: 2,
            rows: 1,
            values: vec![100.0, 900.0],
            min: 100.0,
            max: 900.0,
            avg: 500.0,
            maintenance: 1.0,
            direct: Vec::new(),
            indirect: Vec::new(),
        };
        LightState::apply_room_mask(&mut g, &[true, false]);
        assert_eq!(g.avg, 100.0, "only the cell inside the room counts");
        assert_eq!(g.min, 100.0);
        assert_eq!(g.max, 100.0);
    }

    /// AND A RECTANGULAR ROOM IS UNTOUCHED — every cell inside, so the engine's own figures stand.
    /// This is the property that keeps the DIALux agreement intact.
    #[test]
    fn an_all_inside_mask_changes_nothing() {
        let mut g = cad_light::LuxGrid {
            cols: 2,
            rows: 1,
            values: vec![100.0, 900.0],
            min: 100.0,
            max: 900.0,
            avg: 500.0,
            maintenance: 1.0,
            direct: Vec::new(),
            indirect: Vec::new(),
        };
        LightState::apply_room_mask(&mut g, &[true, true]);
        assert_eq!((g.avg, g.min, g.max), (500.0, 100.0, 900.0));
    }

    /// Two rooms and no selection is ambiguous, and guessing would silently report one room's
    /// figures under the other's name.
    #[test]
    fn two_rooms_with_no_selection_is_left_alone() {
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect(0.0, 0.0, 20.0, 8.0), 3.0).expect("building");
        f.add_room(&rect(1.0, 1.0, 9.0, 7.0)).expect("room a");
        f.add_room(&rect(11.0, 1.0, 19.0, 7.0)).expect("room b");
        f.recompute();
        f.clear_selection();
        assert!(LightState::calc_room_polygon(&f).is_none(), "ambiguous: fall back to the model");
    }
}

/// REOPENING A PROJECT MUST STILL KNOW WHICH BLOCKS ARE FITTINGS.
///
/// A `from_block` is an INDEX into the block table, and an index is a position. Between sessions a
/// drawing gains a door block, loses a furniture block, is round-tripped through another package —
/// and every stored index means something else. The sidecar carries the map by NAME for exactly
/// this; these are the cases it has to get right, including the one where it must refuse.
#[cfg(test)]
mod reopening_relinks_fittings_to_their_blocks {
    use super::*;
    use cad_kernel::{Block, Vec2};

    fn doc_with(names: &[&str]) -> Document {
        let mut d = Document::default();
        for n in names {
            d.blocks.add(Block {
                name: (*n).to_string(),
                base: Vec2::new(0.0, 0.0),
                dobjects: Vec::new(),
                smart: false,
                params: Vec::new(),
                cut_edges: Vec::new(),
            });
        }
        d
    }

    fn map(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn lum(id: u32, profile: &str, from: Option<u32>) -> Luminaire {
        Luminaire {
            id,
            profile: profile.to_string(),
            position: Vertex::new(0.0, 0.0, 3.0),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: from,
        }
    }

    /// THE TABLE SHIFTED UNDER IT. The light was saved pointing at index 0; a block was inserted
    /// ahead of it, so index 0 is now a door. Nothing about the stored index looks broken — it
    /// resolves, to the wrong thing — which is why the name is what decides.
    #[test]
    fn a_shifted_block_table_is_repaired_by_name() {
        let doc = doc_with(&["DOOR", "OCULUS"]);
        let mut lums = vec![lum(1, "OCULUS 3000K", Some(0))];
        let fixed = repair_from_blocks(&mut lums, &doc, &map(&[("OCULUS", "OCULUS 3000K")]));
        assert_eq!(fixed, 1, "the stale index was not repaired");
        assert_eq!(lums[0].from_block, Some(1), "the light points at the wrong block");
    }

    /// AN INDEX THAT IS ALREADY RIGHT IS LEFT ALONE — and counted as untouched, so the log does
    /// not report work on every reopen of a file nothing has happened to.
    #[test]
    fn a_correct_link_is_not_touched() {
        let doc = doc_with(&["OCULUS", "DOOR"]);
        let mut lums = vec![lum(1, "OCULUS 3000K", Some(0))];
        let fixed = repair_from_blocks(&mut lums, &doc, &map(&[("OCULUS", "OCULUS 3000K")]));
        assert_eq!(fixed, 0, "a correct link was rewritten");
        assert_eq!(lums[0].from_block, Some(0));
    }

    /// TWO BLOCKS SHARING ONE PROFILE IS AMBIGUOUS, so nothing is written.
    ///
    /// The same downlight in a square and a round housing is a legitimate thing to draw. Picking
    /// one would attach a light to the wrong symbol — invisible on the plan, wrong in every
    /// schedule produced from it afterwards, and inherited by whoever opens the file next.
    #[test]
    fn an_ambiguous_profile_is_left_unresolved() {
        let doc = doc_with(&["SQUARE", "ROUND"]);
        let mut lums = vec![lum(1, "OCULUS 3000K", None)];
        let fixed = repair_from_blocks(
            &mut lums,
            &doc,
            &map(&[("SQUARE", "OCULUS 3000K"), ("ROUND", "OCULUS 3000K")]),
        );
        assert_eq!(fixed, 0, "a guess was made between two equally good candidates");
        assert_eq!(lums[0].from_block, None, "a guess was written: {:?}", lums[0].from_block);
    }

    /// A BLOCK THE DRAWING NO LONGER HAS resolves to nothing rather than to whatever now sits at
    /// that index. The light keeps its photometry and calculates exactly as before — only the
    /// link to a symbol that is not there any more is dropped.
    #[test]
    fn a_deleted_block_does_not_resolve_to_its_neighbour() {
        let doc = doc_with(&["DOOR", "DESK"]);
        let mut lums = vec![lum(1, "OCULUS 3000K", Some(1))];
        let fixed = repair_from_blocks(&mut lums, &doc, &map(&[("OCULUS", "OCULUS 3000K")]));
        assert_eq!(fixed, 0);
        assert_eq!(
            lums[0].from_block,
            Some(1),
            "an unresolvable link must be left as it was, not pointed at DESK",
        );
        assert_eq!(lums[0].profile, "OCULUS 3000K", "the photometry must not be disturbed");
    }

    /// EACH LIGHT IS ANSWERED SEPARATELY — a plan has several fittings on it, and repairing the
    /// first must not decide the rest.
    #[test]
    fn several_fittings_are_each_repaired_to_their_own_block() {
        let doc = doc_with(&["WALL", "OCULUS", "LINEAR"]);
        let mut lums = vec![
            lum(1, "OCULUS 3000K", Some(0)),
            lum(2, "LINEAR 4000K", Some(0)),
            lum(3, "OCULUS 3000K", Some(0)),
        ];
        let fixed = repair_from_blocks(
            &mut lums,
            &doc,
            &map(&[("OCULUS", "OCULUS 3000K"), ("LINEAR", "LINEAR 4000K")]),
        );
        assert_eq!(fixed, 3);
        assert_eq!(lums[0].from_block, Some(1));
        assert_eq!(lums[1].from_block, Some(2), "the second fitting took the first one's block");
        assert_eq!(lums[2].from_block, Some(1));
    }

    /// A HAND-PLACED LIGHT IS NOT A BLOCK and must not be given one. `place_luminaire` puts a
    /// bare point on the plan with no symbol at all; inventing a `from_block` for it would make
    /// a schedule count symbols that were never drawn.
    #[test]
    fn a_light_with_no_block_of_its_own_is_left_alone() {
        let doc = doc_with(&["OCULUS"]);
        let mut lums = vec![lum(1, "SOMETHING ELSE", None)];
        let fixed = repair_from_blocks(&mut lums, &doc, &map(&[("OCULUS", "OCULUS 3000K")]));
        assert_eq!(fixed, 0);
        assert_eq!(lums[0].from_block, None);
    }
}

/// EVERY ROOM IS CALCULATED, not the last one.
///
/// Reported as "when i make a building after doing a calculation for another building the app only
/// generates the calculation for the last building. i when i hit calculate it should calculate for
/// all." The cause was `calc_room_polygon`: the room whose geometry is SELECTED, else the only
/// room, else nothing. Two rooms and a selection gave whichever was clicked; two rooms and no
/// selection gave neither, and lit the whole model's bounding box instead.
#[cfg(test)]
mod every_room_is_calculated {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, d: f32) -> Vec<glam::Vec2> {
        vec![
            glam::Vec2::new(x, y),
            glam::Vec2::new(x + w, y),
            glam::Vec2::new(x + w, y + d),
            glam::Vec2::new(x, y + d),
            glam::Vec2::new(x, y),
        ]
    }

    /// Two rooms side by side, each with its own fittings.
    fn two_rooms() -> (crate::factory::FactoryState, LightState) {
        let a = rect(0.0, 0.0, 8.0, 6.0);
        let b = rect(14.0, 0.0, 6.0, 6.0);
        let mut f = crate::factory::FactoryState::default();
        for r in [&a, &b] {
            f.add_building_outline(r, 3.0).expect("building");
            f.add_room(r).expect("room");
        }
        f.recompute();

        let mut s = LightState::new();
        s.auto_center_light = false;
        s.cell_size = 0.5;
        let mut id = 0;
        for (cx, cy) in [(2.0, 3.0), (6.0, 3.0), (16.0, 2.0), (18.0, 4.0)] {
            id += 1;
            s.luminaires.push(Luminaire {
                id,
                profile: BUILTIN.to_string(),
                position: Vertex::new(cx, cy, 2.7),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            });
        }
        (f, s)
    }

    /// BOTH ROOMS GET AN ANSWER, and each is a lit room rather than a bounding box.
    #[test]
    fn two_rooms_produce_two_results() {
        let (f, mut s) = two_rooms();
        s.calculate(&Document::default(), Some(&f));
        assert_eq!(s.rooms.len(), 2, "a calculation produced {} result(s)", s.rooms.len());
        for r in &s.rooms {
            assert!(r.grid.avg > 1.0, "{} came out at {:.2} lx — it was not lit", r.name, r.grid.avg);
            assert!(r.plane.cols > 0 && r.plane.rows > 0, "{} has no grid", r.name);
        }
    }

    /// EACH RESULT IS THAT ROOM, not the whole model. Two rooms 14 m apart have planes that do not
    /// overlap; a bounding-box calculation would give both of them the same span.
    #[test]
    fn each_result_covers_its_own_room() {
        let (f, mut s) = two_rooms();
        s.calculate(&Document::default(), Some(&f));
        let spans: Vec<(f32, f32)> = s
            .rooms
            .iter()
            .map(|r| (r.plane.origin.x, r.plane.origin.x + r.plane.width))
            .collect();
        assert_eq!(spans.len(), 2);
        let (a, b) = (spans[0], spans[1]);
        assert!(
            a.1 < b.0 + 1e-3 || b.1 < a.0 + 1e-3,
            "the two planes overlap ({a:?} and {b:?}) — this is one bounding box, not two rooms",
        );
        for (lo, hi) in spans {
            assert!(hi - lo < 12.0, "a plane spans {:.1} m — wider than either room", hi - lo);
        }
    }

    /// THE FITTINGS ARE SPLIT BETWEEN THEM. A power density taken over every fitting in the
    /// building and divided by one room's floor is not a figure about anything.
    #[test]
    fn each_room_counts_only_its_own_fittings() {
        let (f, mut s) = two_rooms();
        s.calculate(&Document::default(), Some(&f));
        let total: usize = s.rooms.iter().map(|r| r.fixtures.len()).sum();
        assert_eq!(total, 4, "the four fittings were counted {total} times between the rooms");
        for r in &s.rooms {
            assert_eq!(r.fixtures.len(), 2, "{} claims {} fittings", r.name, r.fixtures.len());
            let i = r.installation.as_ref().expect("an installation summary");
            assert_eq!(i.count, 2, "{}'s load is over {} fittings", r.name, i.count);
        }
    }


    /// A ROOM'S FIXTURES INCLUDE THE LIGHTS THE MODEL GENERATES.
    ///
    /// A curved luminaire is a real fitting, and it exists only for the length of a calculation —
    /// it is derived from the model rather than stored. Carrying ids and resolving them against
    /// `luminaires` afterwards found the placed ones and silently dropped the rest, so a room's
    /// Installation section counted fittings its schedule did not list. Found on the owner's own
    /// three-room plan: 112 fixtures claimed against 23 placed.
    #[test]
    fn a_room_keeps_the_records_of_lights_that_outlive_nothing() {
        let poly = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(10.0, 0.0),
            glam::Vec2::new(10.0, 10.0),
            glam::Vec2::new(0.0, 10.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let placed = Luminaire {
            id: 3,
            profile: BUILTIN.to_string(),
            position: Vertex::new(2.0, 2.0, 2.7),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        };
        // The id range generated lights use, so one can never collide with a user's.
        let generated = Luminaire { id: 1_000_000, position: Vertex::new(8.0, 8.0, 2.7), ..placed.clone() };
        let outside = Luminaire { id: 4, position: Vertex::new(50.0, 50.0, 2.7), ..placed.clone() };

        let got = LightState::fixtures_in(&poly, &[placed.clone(), generated.clone(), outside]);
        assert_eq!(got.len(), 2, "the room holds the placed one and the generated one");
        assert!(got.iter().any(|l| l.id == 3), "the placed fixture is missing");
        assert!(
            got.iter().any(|l| l.id == 1_000_000),
            "the generated fixture was dropped — its record is the only copy there is",
        );
        assert!(!got.iter().any(|l| l.id == 4), "a fixture outside the room was claimed");

        // The RECORD, not the id — that is the whole point. A schedule built from ids could not
        // describe the generated one, because it is in no list to look up.
        let g = got.iter().find(|l| l.id == 1_000_000).expect("there");
        assert!((g.position.x - 8.0).abs() < 1e-6, "the record came back wrong");
    }

    /// NO FOOTPRINT MEANS EVERY FIXTURE — the whole-model fallback a 2D-only project uses.
    #[test]
    fn a_room_with_no_footprint_claims_everything() {
        let l = Luminaire {
            id: 1,
            profile: BUILTIN.to_string(),
            position: Vertex::new(999.0, 999.0, 2.7),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        };
        assert_eq!(LightState::fixtures_in(&[], std::slice::from_ref(&l)).len(), 1);
    }

    /// A SELECTION STILL DECIDES WHICH ROOM THE PANEL SHOWS — the gesture people already have —
    /// but it no longer decides which rooms are CALCULATED.
    #[test]
    fn selecting_a_room_chooses_the_panel_not_the_scope() {
        let (mut f, mut s) = two_rooms();
        // Select the second room's floor.
        let second = f.rooms[1].floor.expect("a floor");
        f.selection = vec![second];
        s.calculate(&Document::default(), Some(&f));

        assert_eq!(s.rooms.len(), 2, "selecting a room narrowed the calculation to it");
        let panel = s.plane.as_ref().expect("a primary plane");
        assert!(
            (panel.origin.x - s.rooms[1].plane.origin.x).abs() < 1e-6,
            "the panel is showing the room that was not selected",
        );
        // …and the rooms stay in drawing order, so a report does not shuffle when one is clicked.
        assert!(s.rooms[0].plane.origin.x < s.rooms[1].plane.origin.x, "the rooms were reordered");
    }

    /// A PROJECT WITH NO ROOMS still gets exactly one answer — the whole-model fallback the 2D-only
    /// path has always used. This is the case the 1,600 tests before it all exercise.
    #[test]
    fn a_project_with_no_rooms_still_gets_one_result() {
        let mut s = LightState::new();
        s.auto_center_light = false;
        s.luminaires.push(Luminaire {
            id: 1,
            profile: BUILTIN.to_string(),
            position: Vertex::new(2.0, 2.0, 2.7),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        let mut doc = Document::default();
        doc.push(cad_kernel::DObject::new(cad_kernel::Geom::Line(cad_kernel::Line {
            a: cad_kernel::Vec2::new(0.0, 0.0),
            b: cad_kernel::Vec2::new(4.0, 4.0),
        })));
        s.calculate(&doc, None);
        assert_eq!(s.rooms.len(), 1);
        assert!(s.grid.is_some(), "the primary result is still filled in");
    }
}

/// THE CALCULATION RUNS OFF THE UI THREAD.
///
/// "while calculating the app stops responding for sometime and everything freezes." It ran on the
/// UI thread, so the window stopped repainting, Windows greyed it out and wrote "Not Responding" —
/// and from outside, a calculation working perfectly and a crash look identical.
#[cfg(test)]
mod the_calculation_can_leave_the_ui_thread {
    use super::*;

    fn room() -> (crate::factory::FactoryState, LightState) {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(6.0, 0.0),
            glam::Vec2::new(6.0, 5.0),
            glam::Vec2::new(0.0, 5.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.recompute();

        let mut s = LightState::new();
        s.auto_center_light = false;
        s.cell_size = 0.5;
        s.luminaires.push(Luminaire {
            id: 1,
            profile: BUILTIN.to_string(),
            position: Vertex::new(3.0, 2.5, 2.7),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        (f, s)
    }

    /// THE JOB IS `Send` — which is the whole point, and the one thing a compile can prove. If it
    /// ever picks up a borrow on the app or an `Rc`, this stops building.
    #[test]
    fn a_job_can_cross_a_thread_and_come_back() {
        let (f, mut s) = room();
        let job = s.prepare(&Document::default(), Some(&f)).expect("a job");
        let p = std::sync::Arc::new(CalcProgress::default());
        let p2 = p.clone();
        let out = std::thread::spawn(move || job.run(&p2)).join().expect("the worker must finish");

        assert!(!out.cancelled);
        assert_eq!(out.rooms.len(), 1);
        assert!(out.rooms[0].grid.avg > 1.0, "the room came back unlit");

        // …and the answer is the same one the blocking path gives.
        let (f2, mut s2) = room();
        s2.calculate(&Document::default(), Some(&f2));
        s.apply_outcome(out, None);
        let a = s.grid.as_ref().expect("threaded grid");
        let b = s2.grid.as_ref().expect("blocking grid");
        assert!(
            (a.avg - b.avg).abs() < 1e-9 && (a.min - b.min).abs() < 1e-9,
            "threaded {:.6} / {:.6} against blocking {:.6} / {:.6}",
            a.avg,
            a.min,
            b.avg,
            b.min,
        );
    }

    /// PROGRESS REACHES THE FULL WAY. A bar that stops at four fifths is a bar nobody trusts.
    #[test]
    fn progress_runs_from_nothing_to_everything() {
        let (f, mut s) = room();
        let job = s.prepare(&Document::default(), Some(&f)).expect("a job");
        let steps = job.steps();
        let p = CalcProgress::default();
        assert_eq!(p.fraction(), 0.0, "it starts at nothing");

        let out = job.run(&p);
        assert!(!out.cancelled);
        assert_eq!(
            p.done.load(std::sync::atomic::Ordering::Relaxed),
            steps,
            "the job reported {} of the {steps} steps it promised",
            p.done.load(std::sync::atomic::Ordering::Relaxed),
        );
        assert!((p.fraction() - 1.0).abs() < 1e-6, "the bar finished at {}", p.fraction());
        assert!(!p.label().is_empty(), "the last phase left no label");
    }

    /// THE PHASE SAYS WHICH ROOM. On a three-room building "Calculating…" for four minutes says
    /// nothing; "Room 2 of 3 — working plane" says how much is left.
    #[test]
    fn the_phase_names_the_room_it_is_on() {
        let (f, mut s) = room();
        let job = s.prepare(&Document::default(), Some(&f)).expect("a job");
        let p = CalcProgress::default();
        let _ = job.run(&p);

        // READ FROM THE LOG, not sampled from a watcher thread.
        //
        // This used to poll `label()` every millisecond from a second thread, which measures the
        // MACHINE: a phase that starts and finishes between two polls is invisible, and the test
        // fails on a fast computer or after any change to how long a phase takes. It duly failed
        // the day the engine gained its near-field correction — reporting that the working plane
        // was never calculated, on a run that calculated it perfectly well.
        let labels = p.phases();
        assert!(
            labels.iter().any(|l| l.contains("working plane")),
            "no phase named the working plane: {labels:?}",
        );
        assert!(
            labels.iter().any(|l| l.contains("1 of 1")),
            "the phase does not say which room of how many: {labels:?}",
        );
    }

    /// STOP MEANS STOP. A job you cannot cancel is not much better than one that freezes the
    /// window — you still sit and wait for it.
    #[test]
    fn a_cancelled_job_comes_back_cancelled() {
        let (f, mut s) = room();
        let job = s.prepare(&Document::default(), Some(&f)).expect("a job");
        let p = CalcProgress::default();
        p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let out = job.run(&p);
        assert!(out.cancelled, "the job ran to completion after being cancelled");
        assert!(out.rooms.is_empty(), "a cancelled job returned results");
    }

    /// A CANCELLED RESULT DOES NOT OVERWRITE THE LAST GOOD ONE. Half a calculation is not a
    /// calculation, and replacing a finished answer with one is worse than showing nothing.
    #[test]
    fn cancelling_leaves_the_previous_answer_alone() {
        let (f, mut s) = room();
        s.calculate(&Document::default(), Some(&f));
        let before = s.grid.as_ref().expect("a first answer").avg;
        assert!(before > 1.0);

        let job = s.prepare(&Document::default(), Some(&f)).expect("a job");
        let p = CalcProgress::default();
        p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        s.apply_outcome(job.run(&p), None);

        assert!(
            (s.grid.as_ref().expect("still there").avg - before).abs() < 1e-9,
            "a cancelled run replaced the answer that was already on screen",
        );
        assert_eq!(s.rooms.len(), 1, "and it kept the rooms");
        assert!(s.last_msg.contains("stopped"), "it must say what happened: {:?}", s.last_msg);
    }

    /// NOTHING TO CALCULATE IS NOT A JOB. An empty project must not spawn a worker that returns
    /// an empty answer and wipes the panel.
    #[test]
    fn an_empty_project_produces_no_job() {
        let mut s = LightState::new();
        s.auto_center_light = false;
        assert!(s.prepare(&Document::default(), None).is_none());
        assert!(s.last_msg.contains("No geometry"), "it must say why: {:?}", s.last_msg);
    }
}

/// A CALCULATION IS KEPT, AND STOPS BEING KEPT WHEN IT STOPS BEING TRUE.
///
/// Asked for as: *"once a calculation is run the app should save it. the calculation should only be
/// invalidated if any of the lights, the 3d objects or anything related with the calculation is
/// changed. if the user closed the app after a calculation the they should not lose the result."*
///
/// Both halves are load-bearing and they pull in opposite directions. Keeping a result too eagerly
/// shows a figure for a building somebody has since changed; discarding it too eagerly throws away
/// a seventy-second answer because a light moved a millimetre and back. What decides between them
/// is [`CalcJob::fingerprint`], so that is what most of this exercises.
#[cfg(test)]
mod a_calculation_is_kept_while_it_is_still_true {
    use super::*;

    fn rect(w: f32, d: f32) -> Vec<glam::Vec2> {
        vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(w, 0.0),
            glam::Vec2::new(w, d),
            glam::Vec2::new(0.0, d),
            glam::Vec2::new(0.0, 0.0),
        ]
    }

    fn room_of(poly: &[glam::Vec2]) -> (crate::factory::FactoryState, LightState) {
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(poly, 3.0).expect("building");
        f.add_room(poly).expect("room");
        f.recompute();

        let mut s = LightState::new();
        s.auto_center_light = false;
        s.cell_size = 0.5;
        s.luminaires.push(Luminaire {
            id: 1,
            profile: BUILTIN.to_string(),
            position: Vertex::new(3.0, 2.5, 2.7),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        (f, s)
    }

    fn room() -> (crate::factory::FactoryState, LightState) {
        room_of(&rect(6.0, 5.0))
    }

    fn fp(s: &mut LightState, f: &crate::factory::FactoryState) -> u64 {
        s.current_fingerprint(&Document::default(), Some(f)).expect("a scene to fingerprint")
    }

    // -----------------------------------------------------------------------------------------
    // what the fingerprint must NOT notice
    // -----------------------------------------------------------------------------------------

    /// THE SAME SCENE HASHES THE SAME, twice in a row and on a fresh `LightState`.
    ///
    /// The first thing to prove, and not obvious: `profiles` is a `HashMap`, whose iteration order
    /// differs between runs of the same program. Hashed in that order, every result in every
    /// project would read as out of date roughly always, and the feature would be worse than not
    /// having it at all.
    #[test]
    fn an_unchanged_scene_keeps_its_fingerprint() {
        let (f, mut s) = room();
        let a = fp(&mut s, &f);
        let b = fp(&mut s, &f);
        assert_eq!(a, b, "the same scene hashed two different ways in one process");

        let (f2, mut s2) = room();
        assert_eq!(a, fp(&mut s2, &f2), "an identical scene built again hashed differently");
    }

    /// AND IT SURVIVES THE PROFILE TABLE BEING BUILT IN A DIFFERENT ORDER — the `HashMap` case
    /// above, made to actually happen rather than hoped about.
    #[test]
    fn the_order_profiles_were_loaded_in_does_not_matter() {
        let ies = |name: &str, cd: f64| {
            let mut p = builtin_downlight();
            p.name = name.to_string();
            p.multiplier = cd / 1000.0;
            p
        };
        let (f, mut a) = room();
        a.profiles.insert("alpha".into(), ies("alpha", 1000.0));
        a.profiles.insert("beta".into(), ies("beta", 2000.0));
        a.profiles.insert("gamma".into(), ies("gamma", 3000.0));

        let (f2, mut b) = room();
        b.profiles.insert("gamma".into(), ies("gamma", 3000.0));
        b.profiles.insert("alpha".into(), ies("alpha", 1000.0));
        b.profiles.insert("beta".into(), ies("beta", 2000.0));

        assert_eq!(
            fp(&mut a, &f),
            fp(&mut b, &f2),
            "the same fittings loaded in a different order looked like a different building",
        );
    }

    // -----------------------------------------------------------------------------------------
    // what it MUST notice
    // -----------------------------------------------------------------------------------------

    /// EVERY INPUT MOVES IT. One case per thing a person can change, because the failure this
    /// guards against is specific: an input nobody hashed, and a stale answer shown as current.
    #[test]
    fn every_input_to_the_answer_moves_the_fingerprint() {
        let (f, mut base) = room();
        let was = fp(&mut base, &f);

        // A LIGHT MOVED — by a millimetre, which is the point: this is a hash, not a tolerance.
        let (f1, mut s) = room();
        s.luminaires[0].position.x += 0.001;
        assert_ne!(was, fp(&mut s, &f1), "a fixture moved and the answer still looked current");

        // A LIGHT DIMMED.
        let (f2, mut s) = room();
        s.luminaires[0].dimming = 0.5;
        assert_ne!(was, fp(&mut s, &f2), "a fixture was dimmed");

        // A LIGHT ADDED.
        let (f3, mut s) = room();
        let extra = s.luminaires[0].clone();
        s.luminaires.push(Luminaire { id: 2, position: Vertex::new(1.0, 1.0, 2.7), ..extra });
        assert_ne!(was, fp(&mut s, &f3), "a fixture was added");

        // A SURFACE REPAINTED. Reflectance is not a cosmetic setting — five bounces off a lighter
        // wall is most of the difference between a room that passes and one that does not.
        let (f4, mut s) = room();
        s.materials[1].reflectance = 0.8;
        assert_ne!(was, fp(&mut s, &f4), "a wall reflectance changed");

        // THE TRACER RETUNED.
        let (f5, mut s) = room();
        s.settings.rays_per_point *= 2;
        assert_ne!(was, fp(&mut s, &f5), "the ray count changed");
        let (f6, mut s) = room();
        s.settings.max_bounces += 1;
        assert_ne!(was, fp(&mut s, &f6), "the bounce count changed");

        // THE MAINTENANCE FACTOR — it multiplies every figure reported.
        let (f7, mut s) = room();
        s.maintenance.llmf = 0.7;
        assert_ne!(was, fp(&mut s, &f7), "the maintenance factor changed");

        // THE GRID AND THE PLANE IT SITS ON.
        let (f8, mut s) = room();
        s.cell_size = 0.25;
        assert_ne!(was, fp(&mut s, &f8), "the grid spacing changed");
        let (f9, mut s) = room();
        s.plane_height = 0.9;
        assert_ne!(was, fp(&mut s, &f9), "the working plane moved");
        let (f10, mut s) = room();
        s.wall_zone = 0.5;
        assert_ne!(was, fp(&mut s, &f10), "the wall zone changed");
        let (f11, mut s) = room();
        s.eye_height = 1.6;
        assert_ne!(was, fp(&mut s, &f11), "the eye height changed");

        // THE BUILDING ITSELF.
        let (f12, mut s) = room_of(&rect(6.5, 5.0));
        assert_ne!(was, fp(&mut s, &f12), "the room changed shape");
    }

    /// A PROFILE REPLACED UNDER THE SAME NAME. The subtle one: the fixture list, the model and
    /// every setting are untouched, and only the photometry behind a name is different — which is
    /// exactly what re-importing a manufacturer's corrected file does.
    #[test]
    fn re_importing_a_photometric_file_moves_the_fingerprint() {
        let (f, mut a) = room();
        a.profiles.insert(BUILTIN.into(), builtin_downlight());
        let was = fp(&mut a, &f);

        let (f2, mut b) = room();
        let mut brighter = builtin_downlight();
        brighter.multiplier = 2.0;
        b.profiles.insert(BUILTIN.into(), brighter);
        assert_ne!(
            was,
            fp(&mut b, &f2),
            "a fitting twice as bright under the same name looked like the same building",
        );
    }

    // -----------------------------------------------------------------------------------------
    // the answer's standing
    // -----------------------------------------------------------------------------------------

    /// A FRESH ANSWER IS NOT STALE, AND A MOVED LIGHT MAKES IT SO.
    #[test]
    fn moving_a_light_puts_the_answer_out_of_date() {
        let (f, mut s) = room();
        s.calculate(&Document::default(), Some(&f));
        assert!(s.results_fingerprint.is_some(), "a calculation left no fingerprint");
        assert!(!s.results_stale, "a result was stale the moment it was computed");

        s.stale_checked = None; // the throttle is not what is under test here
        assert!(!s.refresh_staleness(&Document::default(), Some(&f)), "an untouched scene moved");
        assert!(!s.results_stale, "an untouched scene was called out of date");

        s.luminaires[0].position.x += 0.4;
        s.stale_checked = None;
        assert!(s.refresh_staleness(&Document::default(), Some(&f)), "the change was not reported");
        assert!(s.results_stale, "a fixture moved and the answer still claimed to be current");

        // AND THE NUMBERS ARE STILL THERE. Wiping them would take away the very thing somebody is
        // comparing the change against — and the change may be the nudge they are about to undo.
        assert!(s.grid.is_some(), "the result was thrown away rather than marked");
        assert_eq!(s.rooms.len(), 1);
    }

    /// PUT BACK, IT IS CURRENT AGAIN. Staleness is a question about the scene, not a one-way latch:
    /// an undone change has to un-stale the answer, or the flag would only ever accumulate.
    #[test]
    fn undoing_the_change_makes_it_current_again() {
        let (f, mut s) = room();
        s.calculate(&Document::default(), Some(&f));
        let home = s.luminaires[0].position.x;

        s.luminaires[0].position.x += 0.4;
        s.stale_checked = None;
        s.refresh_staleness(&Document::default(), Some(&f));
        assert!(s.results_stale);

        s.luminaires[0].position.x = home;
        s.stale_checked = None;
        s.refresh_staleness(&Document::default(), Some(&f));
        assert!(!s.results_stale, "the change was undone and the answer stayed marked out of date");
    }

    /// THE EXPENSIVE CHECK RUNS ONCE PER RESULT, AND THE CHANGE IS NOTICED AT ONCE.
    ///
    /// THIS TEST USED TO ASSERT THE OPPOSITE, and it was right to at the time: the check built the
    /// whole scene, so it was throttled to 250 ms and the app was deliberately blind to a change
    /// for a quarter of a second. It asserted that blindness — `!results_stale` immediately after
    /// moving a fitting five metres.
    ///
    /// That throttle never worked on a real building. The check measured 600 ms on the reference
    /// gym plan, so every frame was already past a 250 ms interval and it ran every frame anyway —
    /// a throttle shorter than the work it guards throttles nothing. Now the question is asked of
    /// the INPUTS (`scene_sig`), so it is a `u64` comparison and there is nothing left to throttle.
    ///
    /// The blindness goes with it, and that is a gain, not a regression: a fitting moved is stale
    /// the same frame. What is pinned here is the cost — the reference is established ONCE and the
    /// expensive path is never taken again for that result.
    #[test]
    fn the_expensive_check_runs_once_and_the_change_is_seen_at_once() {
        let (f, mut s) = room();
        s.calculate(&Document::default(), Some(&f));
        s.stale_checked = None;
        assert!(s.stale_ref_sig.is_none(), "a fresh result starts with no reference");

        s.refresh_staleness(&Document::default(), Some(&f));
        let first = s.stale_checked.expect("the first check ran the expensive path");
        assert!(s.stale_ref_sig.is_some(), "…and adopted the matching scene as the reference");
        assert!(!s.results_stale);

        s.luminaires[0].position.x += 5.0;
        s.refresh_staleness(&Document::default(), Some(&f));
        assert!(s.results_stale, "a fitting moved five metres is stale on the very next look");
        assert_eq!(
            s.stale_checked,
            Some(first),
            "and it cost nothing: the expensive path was not entered again",
        );
    }

    /// A PROJECT EMPTIED TO NOTHING DOES NOT CONDEMN THE ANSWER.
    ///
    /// `current_fingerprint` returns `None` when there is nothing to calculate, and the tempting
    /// reading of that is "the scene changed". It is not: it is most often a momentary state
    /// during an edit, and treating it as a change would flash the warning on and off.
    #[test]
    fn a_scene_with_nothing_in_it_leaves_the_verdict_alone() {
        let (f, mut s) = room();
        s.calculate(&Document::default(), Some(&f));
        s.stale_checked = None;
        assert!(!s.refresh_staleness(&Document::default(), None), "an empty scene was a change");
        assert!(!s.results_stale);
    }

    /// AND NOTHING CALCULATED IS NOT STALE EITHER. There is no answer to be out of date.
    #[test]
    fn an_uncalculated_project_is_never_out_of_date() {
        let (f, mut s) = room();
        s.stale_checked = None;
        assert!(!s.refresh_staleness(&Document::default(), Some(&f)));
        assert!(!s.results_stale);
    }

    // -----------------------------------------------------------------------------------------
    // the round trip
    // -----------------------------------------------------------------------------------------

    /// CLOSING THE APP DOES NOT LOSE THE RESULT — the whole request, end to end.
    ///
    /// Calculated in one `LightState`, written out, and read back into a DIFFERENT one that has
    /// never calculated anything, which is exactly how a restart does it.
    #[test]
    fn a_result_survives_being_written_out_and_read_back() {
        let (f, mut s) = room();
        s.calculate(&Document::default(), Some(&f));
        let before = s.rooms[0].grid.clone();
        let fingerprint = s.results_fingerprint.expect("a fingerprint");

        let dir = std::env::temp_dir().join("simlux_result_roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let drawing = dir.join("project.rsm");
        let stored = crate::light_store::StoredResults::of(
            &s.rooms,
            &s.surfaces,
            &s.last_timings,
            fingerprint,
            "test",
        );
        crate::light_store::save(&drawing, &stored).expect("written");

        let read = crate::light_store::load(&drawing).expect("read back");
        let (f2, mut fresh) = room();
        let current = fp(&mut fresh, &f2);
        assert!(fresh.restore_results(&read, current), "the result was refused for its own scene");

        // THE FIGURES ARE THE FIGURES, digit for digit. A result that shifts in its last decimal
        // on reload is a result nobody can quote — and rounding the cells to `f32` on the way to
        // disk would do exactly that, if the statistics were recomputed from them.
        let after = &fresh.rooms[0].grid;
        assert_eq!(
            after.avg.to_bits(),
            before.avg.to_bits(),
            "the average moved: {} to {}",
            before.avg,
            after.avg,
        );
        assert_eq!(after.min.to_bits(), before.min.to_bits(), "the minimum moved");
        assert_eq!(after.max.to_bits(), before.max.to_bits(), "the maximum moved");
        assert_eq!(after.u0().to_bits(), before.u0().to_bits(), "the uniformity moved");
        assert_eq!(after.values.len(), before.values.len(), "cells were lost");
        assert!(!fresh.results_stale, "a restored result was born out of date");
        assert!(fresh.results_restored, "it does not say where it came from");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AND THE CELLS THEMSELVES COME BACK, not just the summary.
    ///
    /// The overlay, the report's false-colour page and the grid table are all drawn from the cells;
    /// a file that restored four statistics and an empty grid would look right in the panel and be
    /// empty everywhere it matters.
    #[test]
    fn the_cells_come_back_too() {
        let (f, mut s) = room();
        s.calculate(&Document::default(), Some(&f));
        let before = s.rooms[0].grid.values.clone();
        assert!(before.len() > 50, "the fixture is too small to prove anything");

        let stored = crate::light_store::StoredResults::of(&s.rooms, &s.surfaces, &[], 7, "test");
        let rooms = stored.rooms().expect("rebuilt");
        let after = &rooms[0].grid.values;
        assert_eq!(after.len(), before.len());
        for (i, (a, b)) in after.iter().zip(&before).enumerate() {
            // `f32` on the way to disk: about seven significant figures, against a quantity that
            // is quoted to the nearest lux.
            assert!(
                (a - b).abs() <= b.abs() * 1e-6 + 1e-6,
                "cell {i} came back as {a} instead of {b}",
            );
        }
    }

    /// A ROOM MASK SURVIVES. An L-shaped room's grid necessarily covers ground outside it, and the
    /// mask is what keeps those cells out of the average. Lose it and the room silently starts
    /// averaging the courtyard — the numbers still look like numbers.
    #[test]
    fn the_room_mask_survives_the_round_trip() {
        let l = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(8.0, 0.0),
            glam::Vec2::new(8.0, 3.0),
            glam::Vec2::new(3.0, 3.0),
            glam::Vec2::new(3.0, 7.0),
            glam::Vec2::new(0.0, 7.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let (f, mut s) = room_of(&l);
        s.calculate(&Document::default(), Some(&f));
        let before = s.rooms[0].mask.clone();
        assert!(
            before.iter().any(|b| !b) && before.iter().any(|b| *b),
            "the fixture is not L-shaped — the mask is all one value",
        );

        let stored = crate::light_store::StoredResults::of(&s.rooms, &s.surfaces, &[], 7, "test");
        let rooms = stored.rooms().expect("rebuilt");
        assert_eq!(rooms[0].mask, before, "the room mask did not survive");
    }

    /// A RESULT FOR A DIFFERENT BUILDING IS REFUSED OUTRIGHT.
    ///
    /// Not shown greyed, not shown for the rooms it still recognises. Within a session, going out
    /// of date is something the user just did and can undo; across a restart nobody remembers what
    /// changed, and a wrong lux figure with a caption is still a wrong lux figure.
    #[test]
    fn a_result_from_a_changed_project_is_not_restored() {
        let (f, mut s) = room();
        s.calculate(&Document::default(), Some(&f));
        let stored = crate::light_store::StoredResults::of(
            &s.rooms,
            &s.surfaces,
            &[],
            s.results_fingerprint.expect("a fingerprint"),
            "test",
        );

        // The same project, with a wall moved.
        let (f2, mut moved) = room_of(&rect(7.0, 5.0));
        let current = fp(&mut moved, &f2);
        assert!(!moved.restore_results(&stored, current), "a stale result was restored");
        assert!(moved.rooms.is_empty(), "and it left the results behind anyway");
        assert!(moved.grid.is_none());
        assert!(moved.results_fingerprint.is_none());
    }

    /// A DAMAGED FILE IS REFUSED, WHOLE. Half a calculation looks exactly like a whole one on
    /// screen; the cost of refusing is that somebody presses Calculate.
    #[test]
    fn a_damaged_file_restores_nothing() {
        let (f, mut s) = room();
        s.calculate(&Document::default(), Some(&f));
        let good = crate::light_store::StoredResults::of(
            &s.rooms,
            &s.surfaces,
            &[],
            s.results_fingerprint.expect("a fingerprint"),
            "test",
        );

        // The cells gone, everything else intact — the shape a truncated write leaves.
        let mut torn = good.clone();
        torn.rooms[0].grid.values.clear();
        assert!(torn.rooms().is_none(), "a room with no cells rebuilt anyway");

        // The mask gone, which would silently widen an L-shaped room's average.
        if good.rooms[0].mask_len > 0 {
            let mut unmasked = good.clone();
            unmasked.rooms[0].mask_bits.clear();
            assert!(unmasked.rooms().is_none(), "a room lost its mask and rebuilt anyway");
        }

        // A file from a future format.
        let mut newer = good.clone();
        newer.version = crate::light_store::VERSION + 1;
        assert!(newer.rooms().is_none(), "a file from an unknown version was guessed at");

        // And nothing at all.
        assert!(crate::light_store::StoredResults::default().rooms().is_none());
    }

    /// UNREADABLE BYTES ARE "NO SAVED RESULT", not an error in front of somebody opening a
    /// drawing. It is a cache: the worst case is the wait.
    #[test]
    fn rubbish_on_disk_reads_as_nothing_saved() {
        let dir = std::env::temp_dir().join("simlux_result_rubbish");
        let _ = std::fs::create_dir_all(&dir);
        let drawing = dir.join("project.rsm");
        std::fs::write(crate::light_store::result_path(&drawing), b"{ not json at all")
            .expect("write");
        assert!(crate::light_store::load(&drawing).is_none());
        assert!(crate::light_store::load(&dir.join("never-existed.rsm")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE FILE IS NOT ENORMOUS. This is the reason the cells are packed rather than written as
    /// JSON numbers: a real project's grids run to hundreds of thousands of cells, and a result
    /// file that takes longer to parse than the calculation took to run is not a saving.
    #[test]
    fn the_stored_result_is_compact() {
        let (f, mut s) = room();
        s.cell_size = 0.1; // ~3,000 cells: small next to a real plan, enough to show the ratio
        s.calculate(&Document::default(), Some(&f));
        let cells: usize = s.rooms.iter().map(|r| r.grid.values.len()).sum();
        assert!(cells > 2_000, "only {cells} cells — too few to measure against");

        let stored = crate::light_store::StoredResults::of(&s.rooms, &s.surfaces, &[], 7, "test");
        let bytes = serde_json::to_string(&stored).expect("serialises").len();
        // MEASURED AGAINST THE ACTUAL ALTERNATIVE, not against a constant chosen to pass. What the
        // packing is INSTEAD OF is these same cells written as JSON numbers, so that is what it is
        // compared against — a hard-coded byte count would have to be re-guessed every time the
        // fixture changed, and would stop meaning anything the first time somebody did.
        let plain: usize = s
            .rooms
            .iter()
            .map(|r| {
                serde_json::to_string(&r.grid.values).unwrap_or_default().len()
                    + serde_json::to_string(&r.grid.direct).unwrap_or_default().len()
                    + serde_json::to_string(&r.grid.indirect).unwrap_or_default().len()
            })
            .sum();
        assert!(
            bytes * 3 < plain,
            "{bytes} bytes against {plain} as plain JSON ({cells} cells) — the packing is barely \
             earning its complexity",
        );
    }
}

/// A READING NOBODY COULD TAKE IS NOT A READING.
///
/// Reported as: *"our min lux was 0 while for relux it was 133… its an obvious error. find the root
/// cause."*
///
/// The root cause, read out of the stored result rather than guessed at: 65 of the 1140 in-room
/// cells on that plan sat at EXACTLY 0.00 lx, in rectangular clusters standing precisely where the
/// room's furniture is. The working plane is a flat rectangle at 0.8 m and a room has things in it,
/// so some of its points land inside a cupboard. The engine answered those correctly — an enclosed
/// point receives nothing — but it was a right answer to a question that should not have been
/// asked, and it took the room's minimum from 102 lx to zero. Ignoring those cells put the figures
/// at min 102 lx and average 324 lx, against Relux's 133 lx on the same room.
#[cfg(test)]
mod a_point_inside_the_furniture_is_not_measured {
    use super::*;

    /// A closed box, as twelve triangles, spanning `lo..hi`.
    fn box_tris(lo: glam::Vec3, hi: glam::Vec3) -> Vec<[glam::Vec3; 3]> {
        let v = |x: f32, y: f32, z: f32| glam::Vec3::new(x, y, z);
        let (a, b) = (lo, hi);
        let c = [
            v(a.x, a.y, a.z),
            v(b.x, a.y, a.z),
            v(b.x, b.y, a.z),
            v(a.x, b.y, a.z),
            v(a.x, a.y, b.z),
            v(b.x, a.y, b.z),
            v(b.x, b.y, b.z),
            v(a.x, b.y, b.z),
        ];
        let q = |i: usize, j: usize, k: usize, l: usize| vec![[c[i], c[j], c[k]], [c[i], c[k], c[l]]];
        let mut t = Vec::new();
        t.extend(q(0, 1, 2, 3)); // bottom
        t.extend(q(4, 5, 6, 7)); // top
        t.extend(q(0, 1, 5, 4));
        t.extend(q(1, 2, 6, 5));
        t.extend(q(2, 3, 7, 6));
        t.extend(q(3, 0, 4, 7));
        t
    }

    /// A POINT INSIDE A BOX IS INSIDE IT, and one beside it is not.
    #[test]
    fn a_solid_knows_what_it_encloses() {
        let o = Obstacle::from_tris(box_tris(
            glam::Vec3::new(2.0, 2.0, 0.0),
            glam::Vec3::new(4.0, 4.0, 1.0),
        ));
        assert!(o.contains(glam::Vec3::new(3.0, 3.0, 0.5)), "the middle of the box");
        assert!(o.contains(glam::Vec3::new(2.1, 3.9, 0.9)), "just inside a corner");
        assert!(!o.contains(glam::Vec3::new(1.0, 3.0, 0.5)), "beside it");
        assert!(!o.contains(glam::Vec3::new(3.0, 3.0, 1.5)), "above it");
        assert!(!o.contains(glam::Vec3::new(3.0, 3.0, -0.5)), "below it");
    }

    /// ABOVE A LOW TABLE IS NOT BURIED — the case three earlier attempts got wrong.
    ///
    /// A working plane at 0.8 m over a 0.5 m table is open air with light falling on it. Every
    /// containment test tried against the MERGED scene read it as enclosed, because a room is
    /// itself a closed solid and CSG output is not a clean manifold. One body at a time, it is
    /// simply a point above a box.
    #[test]
    fn the_plane_above_a_low_table_is_still_measured() {
        let table = Obstacle::from_tris(box_tris(
            glam::Vec3::new(2.0, 2.0, 0.0),
            glam::Vec3::new(4.0, 4.0, 0.5),
        ));
        assert!(
            !table.contains(glam::Vec3::new(3.0, 3.0, 0.8)),
            "the working plane over a 0.5 m table was treated as buried",
        );
    }

    /// THE MASK DROPS THE BURIED CELLS AND KEEPS THE REST.
    #[test]
    fn the_mask_excludes_only_what_is_inside() {
        let plane = CalcPlane {
            origin: Vertex::new(0.0, 0.0, 0.8),
            width: 6.0,
            depth: 6.0,
            cols: 24,
            rows: 24,
        };
        let poly = [
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(6.0, 0.0),
            glam::Vec2::new(6.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
        ];
        let cupboard = Obstacle::from_tris(box_tris(
            glam::Vec3::new(2.0, 2.0, 0.0),
            glam::Vec3::new(4.0, 4.0, 2.0),
        ));
        let with = LightState::measurable_mask(&plane, &poly, std::slice::from_ref(&cupboard));
        let without = LightState::measurable_mask(&plane, &poly, &[]);

        assert!(without.iter().all(|k| *k), "an empty room lost cells to nothing at all");
        let dropped = with.iter().filter(|k| !**k).count();
        assert!(dropped > 0, "the cupboard excluded nothing");
        // A 2 x 2 m box in a 6 x 6 m room is a ninth of it; the cells are 0.25 m, so about 64 of
        // the 576. Bounded on BOTH sides — excluding far more than the object covers would be a
        // different bug wearing the same face.
        assert!(
            (40..=90).contains(&dropped),
            "{dropped} of 576 cells excluded by a box covering a ninth of the room",
        );
        for (i, keep) in with.iter().enumerate() {
            let (c, r) = (i % 24, i / 24);
            let x = 0.125 + c as f32 * 0.25;
            let y = 0.125 + r as f32 * 0.25;
            let deep_in = (2.3..3.7).contains(&x) && (2.3..3.7).contains(&y);
            let well_out = x < 1.7 || x > 4.3 || y < 1.7 || y > 4.3;
            if deep_in {
                assert!(!keep, "a cell at ({x:.2}, {y:.2}) inside the cupboard is still counted");
            } else if well_out {
                assert!(keep, "a cell at ({x:.2}, {y:.2}) on open floor was thrown away");
            }
        }
    }

    /// AND THE SHELL IS NOT AN OBSTACLE. A room is a solid too, and treating the building as
    /// something to be inside of excludes every cell in it — which is exactly what the first
    /// attempts at this did.
    #[test]
    fn the_building_the_room_is_carved_from_is_not_furniture() {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(6.0, 0.0),
            glam::Vec2::new(6.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.recompute();
        // THE REQUIREMENT IS ABOUT THE FLOOR, NOT ABOUT THE COUNT.
        //
        // This first asserted that a plain room yields NO obstacles, which is the wrong
        // expectation: the building decomposes into seven bodies — the outline volume, the floor
        // slab, four walls and the ceiling — and a wall genuinely IS a solid a point can be inside
        // of. Three of those enclose the whole plan and are dropped as the shell; the walls stay,
        // correctly, since a working-plane point buried in a wall is not a reading either.
        //
        // What must never happen is a point on OPEN FLOOR reading as enclosed. That is what the
        // earlier attempts got wrong, and it is what this checks.
        let bare = obstacles_in(&f, &rect);
        for (x, y) in [(1.0, 1.0), (3.0, 3.0), (5.0, 5.0), (0.5, 4.5), (4.5, 0.5)] {
            assert!(
                !bare.iter().any(|o| o.contains(glam::Vec3::new(x, y, 0.8))),
                "open floor at ({x}, {y}) in an empty room reads as inside something",
            );
        }

        // Put a cupboard in it, and that one IS an obstacle.
        let cup = vec![
            glam::Vec2::new(2.0, 2.0),
            glam::Vec2::new(3.0, 2.0),
            glam::Vec2::new(3.0, 3.0),
            glam::Vec2::new(2.0, 3.0),
            glam::Vec2::new(2.0, 2.0),
        ];
        f.add_building_outline(&cup, 2.0).expect("cupboard");
        f.recompute();
        let obs = obstacles_in(&f, &rect);
        assert!(!obs.is_empty(), "a cupboard standing in the room was not seen");
        assert!(
            obs.iter().any(|o| o.contains(glam::Vec3::new(2.5, 2.5, 0.8))),
            "the cupboard does not enclose its own middle",
        );
        assert!(
            !obs.iter().any(|o| o.contains(glam::Vec3::new(0.5, 0.5, 0.8))),
            "open floor across the room reads as inside something",
        );
    }

    /// END TO END: a room with a cupboard in it reports a minimum somebody could measure.
    #[test]
    fn a_buried_cell_does_not_become_the_rooms_minimum() {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(6.0, 0.0),
            glam::Vec2::new(6.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let cup = vec![
            glam::Vec2::new(2.0, 2.0),
            glam::Vec2::new(4.0, 2.0),
            glam::Vec2::new(4.0, 4.0),
            glam::Vec2::new(2.0, 4.0),
            glam::Vec2::new(2.0, 2.0),
        ];
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.add_building_outline(&cup, 2.0).expect("cupboard");
        f.recompute();

        let mut s = LightState::new();
        s.auto_center_light = false;
        s.cell_size = 0.4;
        s.plane_height = 0.8;
        for (x, y) in [(1.0, 1.0), (5.0, 1.0), (1.0, 5.0), (5.0, 5.0)] {
            s.luminaires.push(Luminaire {
                id: s.luminaires.len() as u32 + 1,
                profile: BUILTIN.to_string(),
                position: Vertex::new(x, y, 2.9),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            });
        }
        s.calculate(&Document::default(), Some(&f));
        let r = &s.rooms[0];
        assert!(
            r.mask.iter().any(|k| !k),
            "the cupboard excluded nothing; the fixture does not reproduce the case",
        );
        assert!(
            r.grid.min > 1.0,
            "the minimum is {:.2} lx — a point inside the cupboard is reported as a reading",
            r.grid.min,
        );
    }
}


/// FORENSIC PROBE — how much detail can the 3D floor's VERTEX colours actually carry?
///
/// `cargo test -p cad_app --lib how_much_detail_the_floor_can_carry -- --ignored --nocapture`
#[cfg(test)]
mod floor_detail_probe {
    /// Not an assertion — a measurement, printed.
    #[test]
    #[ignore]
    fn how_much_detail_the_floor_can_carry() {
        let mut f = crate::factory::FactoryState::default();
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(9.58, 0.0),
            glam::Vec2::new(9.58, 7.58),
            glam::Vec2::new(0.0, 7.58),
            glam::Vec2::new(0.0, 0.0),
        ];
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.recompute();
        let meshes = super::meshes_from_factory(&f);
        for m in &meshes {
            println!(
                "material {:>2}: {:>6} triangles, {:>6} vertices",
                m.material,
                m.triangles.len(),
                m.vertices.len()
            );
        }
        let floor: usize =
            meshes.iter().filter(|m| m.material == 0).map(|m| m.triangles.len()).sum();
        println!("FLOOR triangles: {floor}");
        println!("the report resamples the same room to 38 x 4 = 152 columns of raster");
    }
}

/// A RESULT FROM A DIFFERENT ENGINE IS NOT THIS ENGINE'S RESULT.
///
/// Reported as: *"did you fix the 0 min lux bug? i still experience."* The fix was written, tested
/// and pushed — and the project beside the drawing still showed `Minimum E  0 lx`, because the
/// STORED result restored as valid. Its fingerprint covers the scene, the scene had not changed,
/// and the engine's meaning had. See [`CALC_EPOCH`].
#[cfg(test)]
mod the_engine_is_part_of_the_fingerprint {
    use super::*;

    /// A room with a light in it — enough to fingerprint.
    fn a_lit_room() -> (LightState, crate::factory::FactoryState, Document) {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(6.0, 0.0),
            glam::Vec2::new(6.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.recompute();
        let mut s = LightState::new();
        s.auto_center_light = false;
        s.luminaires.push(Luminaire {
            id: 1,
            profile: BUILTIN.to_string(),
            position: Vertex::new(3.0, 3.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        (s, f, Document::default())
    }

    /// THE EPOCH REACHES THE FINGERPRINT. Without this the constant is documentation.
    #[test]
    fn a_result_from_an_earlier_epoch_does_not_match() {
        let (mut s, f, doc) = a_lit_room();
        let now = s.current_fingerprint(&doc, Some(&f)).expect("a room to fingerprint");
        // THE ONE THING THAT CHANGES IS THE ENGINE. Same state, same scene, same everything else
        // the hash covers — so these must differ, and if the epoch never reaches the hash they
        // cannot.
        let job = s.build_job(&doc, Some(&f)).expect("a job");
        assert_ne!(
            job.fingerprint_with_epoch(1),
            job.fingerprint_with_epoch(2),
            "the same scene fingerprints identically under two different engines — a superseded \
             result would restore as current",
        );
        assert_eq!(
            job.fingerprint_with_epoch(CALC_EPOCH),
            job.fingerprint(),
            "the fingerprint does not use CALC_EPOCH",
        );
        // And asking twice is stable — a fingerprint that moved on its own would invalidate every
        // result on every reopen, which is the opposite failure and just as bad.
        assert_eq!(
            now,
            s.current_fingerprint(&doc, Some(&f)).unwrap(),
            "the fingerprint is not stable across two identical asks",
        );
    }

    /// AND A STORED RESULT FROM BEFORE THE FIX IS REFUSED. The end-to-end shape of the complaint:
    /// a file whose fingerprint was computed under the old engine must not come back.
    #[test]
    fn a_stored_result_from_before_the_fix_is_not_restored() {
        let (mut s, f, doc) = a_lit_room();
        let current = s.current_fingerprint(&doc, Some(&f)).expect("a room to fingerprint");
        // A file written by the engine as it was — same scene, one epoch earlier. Reconstructed by
        // hashing the same job with the previous epoch, which is exactly what the old build wrote.
        let stale = current ^ 0x9E37_79B9_7F4A_7C15; // any fingerprint that is not this one
        let stored = crate::light_store::StoredResults {
            version: 1,
            fingerprint: stale,
            build: "33 (c2eaf1c)".into(),
            rooms: Vec::new(),
            surfaces: Vec::new(),
            timings: Vec::new(),
        };
        assert!(
            !s.restore_results(&stored, current),
            "a result computed by a superseded engine was restored as though it were current",
        );
    }
}

/// ONE SCALE FOR THE WINDOW AND THE PAGE.
///
/// Reported as: *"linking the false color of the report with the simlux window. looks like it not
/// wired in."* It was not: the SIMLUX Display menu carried its own "auto / pin top" and its own
/// palette over state that nothing drew from any more, so turning them changed nothing — which is
/// indistinguishable from broken.
#[cfg(test)]
mod the_window_and_the_page_share_one_scale {
    use super::*;

    fn banded() -> crate::report::Options {
        let mut o = crate::report::Options::default();
        o.scale.bands = vec![50.0, 100.0, 200.0, 300.0];
        o.band_colours = vec![[10, 11, 12], [20, 21, 22], [30, 31, 32], [40, 41, 42], [50, 51, 52]];
        o
    }

    /// The palette, as something no band colour could be mistaken for.
    fn black(_t: f32) -> (f32, f32, f32) {
        (0.0, 0.0, 0.0)
    }

    /// Run `band_legend` for real, headlessly, and hand back the colours it filled.
    ///
    /// THE FIRST VERSION OF THIS TEST NEVER CALLED THE LEGEND. It asserted things about
    /// `Options::lux_rgb` — the rule the blocks would be filled with — and so was a statement about
    /// the colour rule, not about the legend. Replacing the whole banded branch with the gradient
    /// bar left it passing. Driving a frame costs a few lines and tests the function that was
    /// changed.
    fn legend_fills(o: &crate::report::Options, room_max: f64) -> Vec<egui::Color32> {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(600.0, 400.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                band_legend(ui, o, room_max, LuxRamp::Classic);
            });
        });
        let mut fills = Vec::new();
        for cp in out.shapes {
            if let egui::epaint::Shape::Rect(r) = cp.shape {
                // The panel's own background is painted too; only the legend's blocks are this
                // short, and they are the only opaque fills of that height.
                if r.rect.height() > 8.0 && r.rect.height() < 24.0 && r.fill.a() == 255 {
                    fills.push(r.fill);
                }
            }
        }
        fills
    }

    /// THE LEGEND DRAWS THE BANDS, not a gradient — one block per band, in the band's own colour.
    #[test]
    fn the_legend_has_one_block_per_band_in_the_bands_own_colour() {
        let o = banded();
        let room_max = 1662.0;
        let fills = legend_fills(&o, room_max);
        assert_eq!(
            fills.len(),
            5,
            "four thresholds make five bands; the legend painted {} blocks — a gradient bar paints \
             64",
            fills.len(),
        );
        for (k, c) in fills.iter().enumerate() {
            let want = o.band_colours[k];
            assert_eq!(
                [c.r(), c.g(), c.b()],
                want,
                "legend block {k} is not band {k}'s own colour",
            );
        }
    }

    /// AND A CONTINUOUS SCALE STILL GETS A GRADIENT, which is the honest picture of one. The two
    /// branches have to be told apart or the test above passes on either.
    #[test]
    fn a_scale_with_no_bands_still_draws_a_gradient() {
        let mut o = banded();
        o.scale.bands.clear();
        let fills = legend_fills(&o, 1662.0);
        assert!(
            fills.len() > 30,
            "a continuous scale drew {} blocks — that is a banded legend, not a gradient",
            fills.len(),
        );
        // …and NOT in the band colours, which belong to bands.
        assert!(
            !fills.iter().any(|c| [c.r(), c.g(), c.b()] == o.band_colours[0]),
            "a gradient took a band colour",
        );
    }

    /// AND THE SETTINGS THE WINDOW EDITS ARE THE REPORT'S OWN OBJECT. The toolbar takes
    /// `&mut Options` — so there is no copy to fall out of step, and a band changed in the SIMLUX
    /// window IS the band the report prints.
    #[test]
    fn the_toolbar_edits_the_reports_own_options() {
        let src = include_str!("light.rs");
        let a = src.find("pub fn toolbar_ui").expect("the toolbar");
        let sig = &src[a..a + 240];
        assert!(
            sig.contains("report: &mut crate::report::Options"),
            "the toolbar no longer takes the report's options by reference — it has a copy, and a \
             copy is how the window and the page drift apart:\n{sig}",
        );
    }

    // THE OLD PER-WINDOW CEILING — `scale_max` and `scale_ceiling` — is GONE, and there is
    // deliberately NO TEST for that. The first attempt grepped this file for the name and failed
    // instantly, because the assertion's own message contains it: a source-grep test that can match
    // its own text is worthless twice over. And the thing it was reaching for is already
    // guaranteed by the compiler — a field that no longer exists cannot be read, so any leftover
    // use is a build error rather than a silent regression. A test that restates what the compiler
    // enforces adds nothing and costs a name in the suite.
}

/// EXPRESS AND THOROUGH — the same calculation, shown different amounts of the model.
///
/// The whole design rests on the two differing in ONE thing: how furniture is represented. Same
/// rays, same bounces, same grid, same materials — so a run of each on one scene is a controlled
/// comparison and any difference is attributable to the box substitution. These tests pin that,
/// and pin the labelling, because the failure that would discredit the feature is not a wrong
/// number: it is a right-enough number reaching a client with no sign of which mode made it.
#[cfg(test)]
mod express_and_thorough {
    use super::*;

    /// A room with one heavy-ish piece of furniture standing in it.
    fn a_furnished_room() -> crate::factory::FactoryState {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(8.0, 0.0),
            glam::Vec2::new(8.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.recompute();
        // A hollow cage: lots of triangles, mostly air — the shape a box proxy changes most.
        let mut pos = Vec::new();
        for i in 0..200 {
            let a = i as f32 * 0.031;
            for (dx, dy, dz) in [(0.0, 0.0, 0.0), (0.05, 0.0, 0.0), (0.0, 0.0, 0.9)] {
                pos.push([a.cos() * 0.5 + dx, a.sin() * 0.5 + dy, dz]);
            }
        }
        let n = vec![[0.0, 0.0, 1.0]; pos.len()];
        let idx = f.add_furniture_asset(
            "cage".into(),
            crate::mesh_io::ObjMesh {
                positions: pos,
                normals: n,
                color: Some([0.6, 0.6, 0.6]),
                alpha: Vec::new(),
            },
        );
        f.place_furniture(idx, glam::Vec3::new(4.0, 3.0, 0.0));
        f
    }

    /// THE BOX IS THE ASSET'S BOUNDS, CARRIED THROUGH THE INSTANCE'S OWN TRANSFORM — and it is a
    /// box: 12 triangles, 6 planes, closed.
    #[test]
    fn a_proxy_is_twelve_triangles_that_enclose_the_piece() {
        let f = a_furnished_room();
        let tris = crate::light::furniture_box_tris(&f, 0).expect("a proxy");
        assert_eq!(tris.len(), 12, "six faces, two triangles each");

        // Every original vertex is inside the box the proxy describes.
        let inst = &f.furniture[0];
        let asset = &f.furniture_lib[inst.asset];
        let m = glam::Mat4::from_cols_array(&f.furniture_model_matrix(0).unwrap());
        let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
        for t in &tris {
            for p in t {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        for p in &asset.positions {
            let w = m.transform_point3(glam::Vec3::from(*p));
            assert!(
                w.cmpge(lo - 1e-3).all() && w.cmple(hi + 1e-3).all(),
                "vertex {w:?} escaped the proxy box {lo:?}..{hi:?}",
            );
        }
    }

    /// EVERY FACE POINTS OUT. The engine reads a triangle's normal, and a box turned inside out
    /// would bounce light back into itself.
    #[test]
    fn the_proxy_is_wound_outward() {
        let f = a_furnished_room();
        let tris = crate::light::furniture_box_tris(&f, 0).expect("a proxy");
        let c: glam::Vec3 =
            tris.iter().flatten().copied().fold(glam::Vec3::ZERO, |a, b| a + b) / 36.0;
        for t in &tris {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            let out = (t[0] + t[1] + t[2]) / 3.0 - c;
            assert!(n.dot(out) > 0.0, "a face is wound inward: normal {n:?} against {out:?}");
        }
    }

    /// THE SUBSTITUTION IS THE ONLY DIFFERENCE, and it really is a substitution: Express hands the
    /// engine 12 triangles of furniture where Thorough hands it 600.
    #[test]
    fn express_replaces_the_furniture_and_leaves_the_building_alone() {
        let f = a_furnished_room();
        let thorough = crate::light::meshes_from_factory_mode(&f, None, CalcMode::Thorough);
        let express = crate::light::meshes_from_factory_mode(&f, None, CalcMode::Express);

        let furn = |ms: &[Mesh]| -> usize {
            ms.iter()
                .filter(|m| m.material == cad_light::MATERIAL_FURNITURE)
                .map(|m| m.triangles.len())
                .sum()
        };
        let building = |ms: &[Mesh]| -> usize {
            ms.iter()
                .filter(|m| m.material != cad_light::MATERIAL_FURNITURE)
                .map(|m| m.triangles.len())
                .sum()
        };
        assert_eq!(furn(&express), 12, "one box for the one piece");
        assert_eq!(furn(&thorough), 200, "...against every triangle of the cage");
        assert_eq!(
            building(&express),
            building(&thorough),
            "the BUILDING must be identical in both — only furniture is substituted",
        );
    }

    /// AN EXPRESS ANSWER MUST NEVER BE RESTORED AS A THOROUGH ONE. The fingerprint is what stops a
    /// stored result being reused for a different question, so the mode has to be in it.
    #[test]
    fn the_two_modes_do_not_share_a_fingerprint() {
        let doc = Document::default();
        let fp = |f: &crate::factory::FactoryState, m: CalcMode| {
            let mut s = LightState::new();
            s.auto_center_light = false;
            s.mode = m;
            s.prepare(&doc, Some(f)).expect("a job").fingerprint()
        };

        let furnished = a_furnished_room();
        assert_ne!(
            fp(&furnished, CalcMode::Express),
            fp(&furnished, CalcMode::Thorough),
            "an Express result would restore as the answer to a Thorough request",
        );

        // AN EMPTY ROOM IS THE CASE THAT NEEDS THE FIELD. With furniture in the scene the two modes
        // build different `meshes`, so the fingerprint separates them whether or not the mode is
        // hashed — the furnished check above passes with the field deleted and proves nothing.
        // Strip the furniture and the geometry is identical; only the mode is left to tell the two
        // apart, and they ARE still different answers, because Express also skips the surfaces.
        let mut bare = a_furnished_room();
        bare.furniture.clear();
        assert_ne!(
            fp(&bare, CalcMode::Express),
            fp(&bare, CalcMode::Thorough),
            "with no furniture the geometry is identical, so the MODE itself must be in the hash",
        );
    }

    /// …AND NEITHER MODE MAY COLLIDE WITH AN ANSWER FROM AN OLDER ENGINE.
    #[test]
    fn the_epoch_still_separates_engine_versions() {
        let f = a_furnished_room();
        let doc = Document::default();
        let mut s = LightState::new();
        s.auto_center_light = false;
        let job = s.prepare(&doc, Some(&f)).expect("a job");
        assert_ne!(
            job.fingerprint_with_epoch(crate::light::CALC_EPOCH),
            job.fingerprint_with_epoch(crate::light::CALC_EPOCH - 1),
            "results from the previous engine must not restore as current",
        );
    }

    /// EXPRESS SKIPS THE ROOM-SURFACE PASS — a whole extra sweep over every wall, floor and
    /// ceiling, which a designer moving fittings around is not reading.
    #[test]
    fn express_does_not_pay_for_the_surface_report() {
        let f = a_furnished_room();
        let doc = Document::default();
        let mut s = LightState::new();
        s.auto_center_light = false;
        s.cell_size = 2.0; // keep the test quick; the grid is not what is under test

        s.mode = CalcMode::Thorough;
        let out = s.prepare(&doc, Some(&f)).expect("a job").run(&CalcProgress::default());
        assert!(!out.surfaces.is_empty(), "Thorough reports the room surfaces");
        assert_eq!(out.mode, CalcMode::Thorough, "the outcome carries the mode it ran in");

        s.mode = CalcMode::Express;
        let out = s.prepare(&doc, Some(&f)).expect("a job").run(&CalcProgress::default());
        assert!(out.surfaces.is_empty(), "Express does not");
        assert_eq!(out.mode, CalcMode::Express);
        assert!(!out.rooms.is_empty(), "…but it still answers the working plane");
    }

    /// THE LABEL FOLLOWS THE ANSWER, NOT THE SWITCH.
    ///
    /// A calculation is minutes on a real building and it runs on a worker, so there is a whole
    /// window in which the switch can move while the answer is still being computed. Flip it during
    /// that window and a label read off `self.mode` at apply time would stamp Thorough on numbers
    /// an Express run produced — which is precisely how a preview escapes as a compliance figure.
    ///
    /// SO THE FLIP HAPPENS BEFORE `apply_outcome`, not after. Flipping afterwards proves nothing:
    /// the field is already written, and the test passes against a version that reads the switch.
    #[test]
    fn flipping_the_switch_mid_run_does_not_relabel_the_answer() {
        let f = a_furnished_room();
        let doc = Document::default();
        let mut s = LightState::new();
        s.auto_center_light = false;
        s.cell_size = 2.0;
        s.mode = CalcMode::Express;
        let out = s.prepare(&doc, Some(&f)).expect("a job").run(&CalcProgress::default());
        assert_eq!(out.mode, CalcMode::Express);

        s.mode = CalcMode::Thorough; // …while the worker was busy
        s.apply_outcome(out, None);
        assert_eq!(
            s.results_mode,
            Some(CalcMode::Express),
            "the answer was computed in Express and must still say so",
        );

        s.mode = CalcMode::Thorough; // and flipping it afterwards changes nothing either
        assert_eq!(
            s.results_mode,
            Some(CalcMode::Express),
            "the answer on screen is still the Express one and must still say so",
        );
    }

    /// THE BURIED-CELL TEST IS BOXED TOO — and this is where Express is the more trustworthy of the
    /// two, which is the opposite of how a fast mode usually reads.
    ///
    /// `Obstacle::contains` is a ray-parity test against the body's OWN triangles, and its doc says
    /// an unclosed mesh "counts evenly and encloses nothing". An imported machine is not watertight,
    /// so on the full mesh the answer is whatever the non-manifold surface happens to produce. A box
    /// is closed by construction — and costs 12 triangles per cell instead of the whole mesh.
    #[test]
    fn express_boxes_the_obstacles_as_well_as_the_light_scene() {
        let f = a_furnished_room();
        let room = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(8.0, 0.0),
            glam::Vec2::new(8.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
        ];
        let biggest = |obs: Vec<Obstacle>| obs.iter().map(|o| o.tri_count()).max().unwrap_or(0);
        let thorough = biggest(obstacles_in_mode(&f, &room, CalcMode::Thorough));
        let express = biggest(obstacles_in_mode(&f, &room, CalcMode::Express));
        assert_eq!(thorough, 200, "Thorough walks every triangle of the cage");
        assert_eq!(express, 12, "Express walks a box");
        // The one the working plane runs through: a cell in the middle of the cage is INSIDE the
        // box and, on the open frame, is not inside anything at all.
        let mid = glam::Vec3::new(4.0, 3.0, 0.45);
        let inside = |m: CalcMode| obstacles_in_mode(&f, &room, m).iter().any(|o| o.contains(mid));
        assert!(inside(CalcMode::Express), "a point inside the box reads as buried");
        assert!(!inside(CalcMode::Thorough), "…and the open frame encloses nothing, as its doc says");
    }

    /// Only Thorough may be quoted as compliance. One place says so, so the report and the panel
    /// cannot drift apart on it.
    #[test]
    fn only_thorough_carries_a_compliance_claim() {
        assert!(CalcMode::Thorough.is_compliant());
        assert!(!CalcMode::Express.is_compliant());
    }
}

/// EXPRESS AGAINST THOROUGH WHERE THE FURNITURE IS ACTUALLY IN THE ROOM.
///
/// The first attempt at this gate ran on the reference gym plan and the two modes agreed to three
/// decimal places — which proved only that the substitution does not LEAK, because 0 of 119 cells
/// were masked in Express and an Express box is watertight, so no machine overlapped the room that
/// grid covered. It said nothing about accuracy where a box stands in for a real piece.
///
/// This is that scene, built deliberately: a room with an OPEN FRAME in the middle of it — mostly
/// air, straddling the working plane — and lights overhead. A box is the substitution's worst case,
/// because the box is solid where the frame is not.
#[cfg(test)]
mod express_where_furniture_matters {
    use super::*;

    /// A 8 x 6 m room with a 2 x 2 x 2 m open cage at its centre and four downlights over it.
    fn a_room_with_an_open_frame() -> crate::factory::FactoryState {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(8.0, 0.0),
            glam::Vec2::new(8.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.recompute();

        // Vertical bars on a 2 x 2 m ring, 2 m tall: the volume is nearly all air, and its BOX is
        // nearly all solid. Local coordinates, centred, so the instance transform places it.
        let mut pos = Vec::new();
        let bars = 16;
        for i in 0..bars {
            let a = i as f32 / bars as f32 * std::f32::consts::TAU;
            let (cx, cy) = (a.cos() * 1.0, a.sin() * 1.0);
            let w = 0.02;
            // Two triangles making a thin upright quad.
            for (dx, dy, z) in [
                (-w, -w, 0.0), (w, w, 0.0), (w, w, 2.0),
                (-w, -w, 0.0), (w, w, 2.0), (-w, -w, 2.0),
            ] {
                pos.push([cx + dx, cy + dy, z]);
            }
        }
        let normals = vec![[0.0, 0.0, 1.0]; pos.len()];
        let idx = f.add_furniture_asset(
            "cage".into(),
            crate::mesh_io::ObjMesh {
                positions: pos,
                normals,
                color: Some([0.6, 0.6, 0.6]),
                alpha: Vec::new(),
            },
        );
        f.place_furniture(idx, glam::Vec3::new(4.0, 3.0, 0.0));
        f
    }

    fn run(mode: crate::light::CalcMode) -> (f64, f64, f64, f64, usize, f64) {
        let f = a_room_with_an_open_frame();
        let mut s = crate::light::LightState::new();
        s.auto_center_light = false;
        s.cell_size = 0.4;
        s.mode = mode;
        for (i, (x, y)) in [(2.0, 2.0), (6.0, 2.0), (2.0, 4.0), (6.0, 4.0)].iter().enumerate() {
            s.luminaires.push(cad_light::Luminaire {
                id: i as u32 + 1,
                profile: crate::light::BUILTIN.to_string(),
                position: cad_light::Vertex::new(*x, *y, 2.9),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            });
        }
        let doc = cad_kernel::Document::default();
        let t = std::time::Instant::now();
        s.calculate(&doc, Some(&f));
        let secs = t.elapsed().as_secs_f64();
        let r = s.rooms.first().expect("a room");
        let g = if r.grid_en.values.is_empty() { &r.grid } else { &r.grid_en };
        let mask = if r.grid_en.values.is_empty() { &r.mask } else { &r.mask_en };
        let dropped = mask.iter().filter(|k| !**k).count();
        (g.avg, g.min, g.max, g.u0(), dropped, secs)
    }

    /// THE SUBSTITUTION REACHES THE ANSWER when the piece is in the measured room.
    ///
    /// This is the assertion the gym run could not make. A box is more occluding than an open frame
    /// — solid where the frame is air — so the two modes must NOT agree here. If they did, either
    /// the substitution is not reaching the engine or the room does not contain the piece, and both
    /// would make every other Express number meaningless.
    #[test]
    fn boxing_an_open_frame_changes_the_answer() {
        let (ea, emin, emax, eu0, edrop, esec) = run(crate::light::CalcMode::Express);
        let (ta, tmin, tmax, tu0, tdrop, tsec) = run(crate::light::CalcMode::Thorough);
        let pc = |a: f64, b: f64| if b.abs() > 1e-9 { 100.0 * (a - b) / b } else { 0.0 };
        println!("\n=== EXPRESS AGAINST THOROUGH, furniture INSIDE the measured room ===");
        println!("{:<10} {:>9} {:>9} {:>9} {:>8} {:>8} {:>8}", "mode", "avg lx", "min lx", "max lx", "U0", "masked", "seconds");
        println!("{:<10} {:>9.2} {:>9.2} {:>9.2} {:>8.3} {:>8} {:>8.2}", "Express", ea, emin, emax, eu0, edrop, esec);
        println!("{:<10} {:>9.2} {:>9.2} {:>9.2} {:>8.3} {:>8} {:>8.2}", "Thorough", ta, tmin, tmax, tu0, tdrop, tsec);
        println!(
            "  Express against Thorough:  avg {:+.1}%   min {:+.1}%   max {:+.1}%   U0 {:+.3}",
            pc(ea, ta), pc(emin, tmin), pc(emax, tmax), eu0 - tu0,
        );

        assert!(ta > 1.0, "the reference run must actually be lit: {ta:.2} lx");
        assert!(
            (ea - ta).abs() > 0.01 || edrop != tdrop,
            "boxing an open frame must change SOMETHING — avg {ea:.3} against {ta:.3}, \
             masked {edrop} against {tdrop}. If these agree the substitution is not reaching the \
             engine, and no Express figure means anything.",
        );
    }

    /// A BOX BURIES CELLS AN OPEN FRAME DOES NOT, and that is the honest, useful difference: the
    /// buried-cell test is exact on a box and meaningless on a mesh that is not watertight.
    #[test]
    fn the_box_excludes_cells_the_frame_leaves_measurable() {
        let (_, _, _, _, edrop, _) = run(crate::light::CalcMode::Express);
        let (_, _, _, _, tdrop, _) = run(crate::light::CalcMode::Thorough);
        assert!(
            edrop > tdrop,
            "the box stands where the frame is air, so it must bury more cells: \
             Express {edrop}, Thorough {tdrop}",
        );
    }
}

/// THE SIMLUX WORKSPACE REBUILT THE WHOLE CALCULATION GEOMETRY EVERY FRAME.
///
/// In split mode `render_light_3d_panel` called `rebuild_live_meshes_with` unconditionally, and
/// that is `scene_meshes` → `meshes_from_factory_mode(.., Thorough)`: every furniture triangle
/// transformed into a fresh buffer, 7,036,129 of them on the reference gym plan — 21.1 M vertices,
/// about 253 MB — sixty times a second. Measured at ~205 ms of a ~210 ms frame.
///
/// SIMLUX-only, because nothing else enters split mode. That is what the user said in the first
/// report and what four rounds of work in the 3D renderer failed to hear.
#[cfg(test)]
mod the_live_mesh_rebuild {
    use super::*;

    fn a_furnished_model() -> crate::factory::FactoryState {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(10.0, 0.0),
            glam::Vec2::new(10.0, 8.0),
            glam::Vec2::new(0.0, 8.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.recompute();
        let idx = f.add_furniture_asset(
            "stool".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![
                    [0.0, 0.0, 0.0], [0.4, 0.0, 0.0], [0.4, 0.4, 0.0],
                    [0.0, 0.0, 0.0], [0.4, 0.4, 0.0], [0.0, 0.4, 0.0],
                ],
                normals: vec![[0.0, 0.0, 1.0]; 6],
                color: Some([0.6, 0.6, 0.6]),
                alpha: Vec::new(),
            },
        );
        f.place_furniture(idx, glam::Vec3::new(5.0, 4.0, 0.0));
        f
    }

    /// AN UNTOUCHED SCENE ASKS FOR THE SAME SIGNATURE. A signature that moves on its own rebuilds
    /// every frame and looks exactly like the bug it was written to fix.
    #[test]
    fn an_untouched_model_keeps_its_signature() {
        let f = a_furnished_model();
        let s = LightState::new();
        assert_eq!(s.live_mesh_sig_of(Some(&f)), s.live_mesh_sig_of(Some(&f)));
        assert!(s.live_mesh_sig_of(Some(&f)).is_some(), "a 3D model can be summarised");
    }

    /// EVERYTHING `scene_meshes` READS MOVES IT. A miss here is the dangerous direction: the light
    /// scene would freeze at whatever it was when the workspace opened, and the 3D view and the
    /// next calculation would both describe a building that is no longer there.
    #[test]
    fn everything_the_live_scene_is_built_from_moves_the_signature() {
        let cases: Vec<(&str, fn(&mut crate::factory::FactoryState, &mut LightState))> = vec![
            ("rebuilding the CSG model", |f, _| f.recompute()),
            ("moving a piece of furniture", |f, _| f.furniture[0].pos[0] += 0.4),
            ("rotating a piece of furniture", |f, _| f.furniture[0].rot[2] += 12.0),
            ("scaling a piece of furniture", |f, _| f.furniture[0].scale *= 1.3),
            ("deleting a piece of furniture", |f, _| f.furniture.clear()),
            ("switching Express/Thorough", |_, s| s.mode = CalcMode::Express),
        ];
        for (what, change) in cases {
            let mut f = a_furnished_model();
            let mut s = LightState::new();
            let before = s.live_mesh_sig_of(Some(&f));
            change(&mut f, &mut s);
            assert_ne!(
                s.live_mesh_sig_of(Some(&f)),
                before,
                "{what} must rebuild the live scene, or the 3D view and the next calculation both \
                 describe a building that is no longer there",
            );
        }
    }

    /// A 2D-ONLY PROJECT CANNOT BE SUMMARISED, and says so rather than guessing. `scene_meshes`
    /// then extrudes the DOCUMENT, which this has no cheap handle on — and that path is the cheap
    /// one anyway, so the caller keeps rebuilding it.
    #[test]
    fn a_project_with_no_model_declines_to_summarise() {
        let f = crate::factory::FactoryState::default();
        let s = LightState::new();
        assert!(s.live_mesh_sig_of(Some(&f)).is_none(), "no model, no cheap signature");
        assert!(s.live_mesh_sig_of(None).is_none(), "and no factory at all is the same answer");
    }

    /// THE GUARD IS AT THE CALL SITE, and it is the whole fix. A grep, because the alternative is
    /// standing up an egui context and a GL surface; needles are assembled at run time because
    /// `include_str!` includes THIS module and a literal would match the assertion instead of the
    /// code — a mistake already made once in this file.
    #[test]
    fn the_workspace_only_rebuilds_when_the_signature_moves() {
        let src = include_str!("app.rs");
        let needle = |parts: &[&str]| -> String { parts.concat() };
        let anchor = needle(&["let sig = self.light.live_mesh_", "sig_of(Some(&self.factory));"]);
        let a = src.find(&anchor).expect("the live-rebuild guard is gone");
        let b = src[a..].find("\n        }").map(|e| a + e).expect("re-anchor if the block moves");
        let body = &src[a..b];
        assert!(body.len() < 2_500, "the slice must be the guard, not half the file");
        for parts in [
            &["if sig.is_none() || self.light.live_mesh_", "sig != sig {"][..],
            &["rebuild_live_meshes_", "with(&plan, Some(&self.factory));"][..],
            &["self.light.live_mesh_", "sig = sig;"][..],
        ] {
            let n = needle(parts);
            assert!(body.contains(&n), "the guard no longer contains `{n}`");
        }
    }
}

/// "NOW THE APP LAGS AFTER CALCULATING."
///
/// `refresh_staleness` answered "is this result still true?" by building a whole `CalcJob` —
/// `scene_meshes` and every furniture triangle with it. Measured at 600 ms, once per frame, from
/// the moment a result existed. The 250 ms throttle in front of it was worse than useless: the
/// check costs more than the interval, so every frame was already past it. **A throttle shorter
/// than the work it guards never throttles anything.**
///
/// The cure is to ask the same question of the INPUTS. But the danger is entirely one-sided: a
/// staleness check that is cheap and WRONG lets a result that no longer describes the building keep
/// its "current" badge, and somebody signs it. So most of what follows is about the change being
/// noticed, not about it being fast.
#[cfg(test)]
mod the_staleness_check {
    use super::*;

    fn a_calculated_project() -> (crate::factory::FactoryState, LightState) {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(10.0, 0.0),
            glam::Vec2::new(10.0, 8.0),
            glam::Vec2::new(0.0, 8.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut f = crate::factory::FactoryState::default();
        f.add_building_outline(&rect, 3.0).expect("building");
        f.add_room(&rect).expect("room");
        f.recompute();
        let idx = f.add_furniture_asset(
            "stool".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![
                    [0.0, 0.0, 0.0], [0.4, 0.0, 0.0], [0.4, 0.4, 0.0],
                    [0.0, 0.0, 0.0], [0.4, 0.4, 0.0], [0.0, 0.4, 0.0],
                ],
                normals: vec![[0.0, 0.0, 1.0]; 6],
                color: Some([0.6, 0.6, 0.6]),
                alpha: Vec::new(),
            },
        );
        f.place_furniture(idx, glam::Vec3::new(5.0, 4.0, 0.0));

        let mut s = LightState::new();
        s.auto_center_light = false;
        s.cell_size = 2.0;
        s.luminaires.push(cad_light::Luminaire {
            id: 1,
            profile: BUILTIN.to_string(),
            position: cad_light::Vertex::new(5.0, 4.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        s.calculate(&Document::default(), Some(&f));
        assert!(s.results_fingerprint.is_some(), "the fixture must have a result");
        (f, s)
    }

    /// A SETTLED PROJECT ASKS THE SAME QUESTION AND GETS THE SAME ANSWER, cheaply. A signature that
    /// moves on its own would rebuild every frame and look exactly like the bug it replaced.
    #[test]
    fn an_untouched_project_keeps_its_scene_signature() {
        let (f, s) = a_calculated_project();
        let doc = Document::default();
        assert_eq!(s.scene_sig(&doc, Some(&f)), s.scene_sig(&doc, Some(&f)));
        assert!(s.scene_sig(&doc, Some(&f)).is_some());
    }

    /// AFTER THE FIRST CHECK, A SETTLED PROJECT NEVER TOUCHES THE EXPENSIVE PATH AGAIN — and the
    /// verdict stays "current", which is the half that matters.
    #[test]
    fn a_settled_project_stops_paying_for_the_check() {
        let (f, mut s) = a_calculated_project();
        let doc = Document::default();
        assert!(s.stale_ref_sig.is_none(), "a fresh result has no reference yet");
        s.refresh_staleness(&doc, Some(&f)); // one full fingerprint, establishing the reference
        assert!(
            s.stale_ref_sig.is_some(),
            "the matching scene must become the reference, or every frame pays 600 ms again",
        );
        assert!(!s.results_stale, "nothing moved, so the result is current");
        for _ in 0..5 {
            s.refresh_staleness(&doc, Some(&f));
            assert!(!s.results_stale, "and it stays current");
        }
    }

    /// THE DANGEROUS DIRECTION. Every one of these makes the stored answer describe a building that
    /// is no longer there, and every one must still be caught — by the CHEAP path, after the
    /// reference is established, because that is the only path a running app takes.
    #[test]
    fn a_real_change_still_marks_the_result_stale() {
        let cases: Vec<(&str, fn(&mut crate::factory::FactoryState, &mut LightState))> = vec![
            ("moving a fitting", |_, s| s.luminaires[0].position.x += 1.0),
            ("aiming a fitting", |_, s| s.luminaires[0].tilt_deg += 20.0),
            ("dimming a fitting", |_, s| s.luminaires[0].dimming = 0.4),
            ("adding a fitting", |_, s| {
                let mut l = s.luminaires[0].clone();
                l.id = 99;
                s.luminaires.push(l);
            }),
            ("deleting every fitting", |_, s| s.luminaires.clear()),
            ("a surface reflectance", |_, s| s.materials[0].reflectance = 0.11),
            ("the working plane height", |_, s| s.plane_height += 0.2),
            ("the grid spacing", |_, s| s.cell_size *= 0.5),
            ("the maintenance factor", |_, s| s.maintenance.llmf = 0.55),
            ("the ray settings", |_, s| s.settings.max_bounces += 1),
            ("switching Express/Thorough", |_, s| s.mode = CalcMode::Express),
            ("moving a piece of furniture", |f, _| f.furniture[0].pos[0] += 0.5),
            ("deleting the furniture", |f, _| f.furniture.clear()),
            ("rebuilding the CSG model", |f, _| f.recompute()),
        ];
        for (what, change) in cases {
            let (mut f, mut s) = a_calculated_project();
            let doc = Document::default();
            s.refresh_staleness(&doc, Some(&f)); // establish the reference
            assert!(!s.results_stale, "{what}: the fixture must start current");
            change(&mut f, &mut s);
            s.refresh_staleness(&doc, Some(&f));
            assert!(
                s.results_stale,
                "{what} changes the answer, so the result must be marked stale — a cheap check \
                 that misses it lets a figure that no longer describes the building keep its \
                 badge, and somebody signs it",
            );
        }
    }

    /// A NEW RESULT DROPS THE OLD REFERENCE. Keeping it would answer "has the scene moved?" against
    /// the scene the PREVIOUS answer belonged to.
    #[test]
    fn a_new_result_drops_the_reference() {
        let (f, mut s) = a_calculated_project();
        let doc = Document::default();
        s.refresh_staleness(&doc, Some(&f));
        assert!(s.stale_ref_sig.is_some());
        s.calculate(&doc, Some(&f));
        assert!(s.stale_ref_sig.is_none(), "a fresh answer needs a fresh reference");
    }

    /// A MISMATCHING SCENE IS NOT RECORDED AS THE REFERENCE. If it were, a stale result would look
    /// current again the moment nothing else changed — the worst possible failure for this feature.
    #[test]
    fn a_stale_scene_never_becomes_the_reference() {
        let (mut f, mut s) = a_calculated_project();
        let doc = Document::default();
        f.furniture[0].pos[0] += 0.5; // moved BEFORE any reference was taken
        s.refresh_staleness(&doc, Some(&f));
        assert!(s.results_stale, "the scene moved, so the result is stale");
        assert!(
            s.stale_ref_sig.is_none(),
            "and the moved scene must NOT be adopted as the reference",
        );
        s.refresh_staleness(&doc, Some(&f));
        assert!(s.results_stale, "…so it is still stale on the next look");
    }
}
