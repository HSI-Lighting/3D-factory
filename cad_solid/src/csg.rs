//! The csgrs boundary — the ONLY place glam↔nalgebra conversion happens.
//!
//! `Model` (glam f32, UI-facing) → csgrs `Mesh<()>` (nalgebra f64, BSP CSG) →
//! [`SolidMesh`] (glam f32, render/wire-in). Keeping this in one file means the
//! rest of `cad_solid` never sees nalgebra or csgrs.

use csgrs::csg::CSG;
use csgrs::mesh::Mesh;
use csgrs::sketch::Sketch;
use nalgebra::{Matrix4, Vector3};

use crate::{BoolOp, Feature, Model, Primitive, SolidMesh};

/// csgrs mesh with no per-face metadata.
type CsgMesh = Mesh<()>;

/// glam `Mat4` (f32, column-major) → nalgebra `Matrix4<f64>` (also column-major).
fn to_na(m: glam::Mat4) -> Matrix4<f64> {
    let c = m.to_cols_array();
    Matrix4::from_column_slice(&c.map(|x| x as f64))
}

/// Build a primitive as a csgrs mesh in canonical LOCAL coords: footprint centred
/// on the local origin, resting on the plane (local z = 0) and rising +Z.
fn local_mesh(p: &Primitive, model: &Model) -> CsgMesh {
    match *p {
        // The ONE primitive that needs the Model: its outline lives in the profile table
        // (see `Primitive::Extrusion` for why it is referenced rather than inlined).
        // csgrs's own sketch module does the work — `earcut` is already enabled in
        // Cargo.toml, so there is no new dependency here.
        Primitive::Extrusion { profile, h, .. } => match model.profile(profile) {
            Some(pr) => {
                let pts: Vec<[f64; 2]> =
                    pr.pts.iter().map(|q| [q[0] as f64, q[1] as f64]).collect();
                Sketch::polygon(&pts, ()).extrude(h.max(1e-4) as f64)
            }
            // A stale id must not take the app down — an empty mesh is a visible
            // "nothing there", which is recoverable; a panic is not.
            None => CsgMesh::new(),
        },
        Primitive::Box { w, d, h } => {
            // csgrs cuboid is corner-anchored at the origin → recentre in u,v so it
            // sits centred on the placement point (base still on the plane).
            let m = CsgMesh::cuboid(w as f64, d as f64, h as f64, ());
            m.transform(&Matrix4::new_translation(&Vector3::new(
                -(w as f64) / 2.0,
                -(d as f64) / 2.0,
                0.0,
            )))
        }
        // csgrs cylinder already rises +Z from a base at z=0, centred on the axis.
        Primitive::Cylinder { r, h, sides } => {
            CsgMesh::cylinder(r as f64, h as f64, sides.max(3) as usize, ())
        }
        // csgrs sphere is CENTRED on the origin → lift by r so it RESTS on the plane
        // (this module's convention: footprint centred, sitting on local z = 0).
        Primitive::Sphere { r, segments, stacks } => {
            CsgMesh::sphere(r as f64, segments.max(3) as usize, stacks.max(2) as usize, ())
                .transform(&lift(r as f64))
        }
        // csgrs `frustum` delegates to `frustum_ptp(Point3::origin(), ..)` → base at
        // z=0, rising +Z. Matches our convention as-is. `sides` low + r_top=0 gives a
        // pyramid; r_top=r_bottom gives a prism — same primitive, no special case.
        Primitive::Frustum { r_bottom, r_top, h, sides } => CsgMesh::frustum(
            r_bottom as f64,
            r_top as f64,
            h as f64,
            sides.max(3) as usize,
            (),
        ),
        // csgrs torus = a revolved sketch; its axis convention is VERIFIED by
        // `local_aabb_matches_real_mesh` rather than assumed. `to_z_up` is the
        // measured correction (identity if it already lies in XY).
        Primitive::Torus { major_r, minor_r, seg_major, seg_minor } => CsgMesh::torus(
            major_r as f64,
            minor_r as f64,
            seg_major.max(3) as usize,
            seg_minor.max(3) as usize,
            (),
        )
        .transform(&torus_to_z_up(minor_r as f64)),
        // COMPOSED — csgrs has no capsule. Barrel from z=r to z=r+h, hemispherical
        // caps centred at each end. Whole thing rests on the plane; height = h + 2r.
        Primitive::Capsule { r, h, segments, stacks } => {
            let (rf, hf) = (r as f64, h as f64);
            let seg = segments.max(3) as usize;
            let st = stacks.max(2) as usize;
            let barrel = CsgMesh::cylinder(rf, hf, seg, ()).transform(&lift(rf));
            let bot = CsgMesh::sphere(rf, seg, st, ()).transform(&lift(rf));
            let top = CsgMesh::sphere(rf, seg, st, ()).transform(&lift(rf + hf));
            barrel.union(&bot).union(&top)
        }
        // COMPOSED — csgrs has no tube. Outer ∖ inner; the inner cylinder is
        // over-extended past both ends so the difference cuts cleanly instead of
        // leaving coplanar faces at z=0/z=h (a classic BSP artifact source).
        Primitive::Tube { r_outer, r_inner, h, sides } => {
            let (ro, ri, hf) = (r_outer as f64, r_inner as f64, h as f64);
            let n = sides.max(3) as usize;
            let outer = CsgMesh::cylinder(ro, hf, n, ());
            if ri <= 1e-6 || ri >= ro {
                return outer; // degenerate bore → a solid cylinder
            }
            let bore = CsgMesh::cylinder(ri, hf + 2.0 * EPS_CUT, n, ()).transform(&lift(-EPS_CUT));
            outer.difference(&bore)
        }
        Primitive::Ellipsoid { rx, ry, rz, segments, stacks } => CsgMesh::ellipsoid(
            rx as f64,
            ry as f64,
            rz as f64,
            segments.max(3) as usize,
            stacks.max(2) as usize,
            (),
        )
        .transform(&lift(rz as f64)),
    }
}

/// Over-cut for boolean subtraction, so the cutter pokes through both faces and
/// never leaves a coplanar pair for the BSP to argue about.
const EPS_CUT: f64 = 1e-3;

/// Translate along +Z.
fn lift(dz: f64) -> Matrix4<f64> {
    Matrix4::new_translation(&Vector3::new(0.0, 0.0, dz))
}

/// Correction that puts a csgrs torus flat in XY, resting on the plane.
///
/// csgrs builds it as `Sketch::circle(minor_r).translate(major_r,0,0).revolve(360)`
/// and **revolves about Y** — so the raw ring stands UP in the XZ plane
/// (MEASURED, not assumed: raw bounds were x=±2.5, y=±0.5, z=±2.5 for
/// major=2, minor=0.5). Every other primitive here lies flat and rises +Z, so the
/// torus alone needs a 90° roll about X to match: (x,y,z) → (x,−z,y). Then lift by
/// `minor_r` so it rests on the plane like the rest.
///
/// `local_aabb_matches_real_mesh` measures this; if csgrs ever changes its revolve
/// axis the test fails loudly instead of shipping a silently mis-oriented torus.
fn torus_to_z_up(minor_r: f64) -> Matrix4<f64> {
    let roll_x_90 = Matrix4::from_euler_angles(std::f64::consts::FRAC_PI_2, 0.0, 0.0);
    lift(minor_r) * roll_x_90
}

/// A feature's primitive, placed into world coords on its plane.
fn world_mesh(f: &Feature, model: &Model) -> CsgMesh {
    let local = local_mesh(&f.primitive, model);
    local.transform(&to_na(f.plane.world_matrix(&f.placement)))
}

/// Evaluate the feature list into a render mesh with **GROUP** semantics:
///
/// - a **Union** feature starts a NEW independent body,
/// - a **Difference** / **Intersection** feature modifies the CURRENT body,
/// - bodies are **concatenated**, never booleaned with each other.
///
/// This is what keeps distinct objects distinct. A ceiling (its own Union body) can never
/// weld into the building or fill a room carved into it — it just sits alongside as its
/// own body. A room (Difference) cuts only the building it was added onto. It also matches
/// how the tools read to a user: "building" is one thing, "ceiling" is another.
///
/// A leading Difference with no body to cut is simply dropped (there is nothing to
/// subtract from) — which is why `add_room` requires a building first.
pub fn eval(model: &Model) -> SolidMesh {
    let mut out = SolidMesh::default();
    // The leading feature id of the body currently accumulating — every triangle it
    // produces is tagged with it, so the app can colour a body by its feature.
    let mut current: Option<(CsgMesh, u32)> = None;
    for f in &model.features {
        let m = world_mesh(f, model);
        match f.op {
            BoolOp::Union => {
                if let Some((c, id)) = current.take() {
                    append_solid(&c, id, &mut out); // flush the finished body
                }
                current = Some((m, f.id));
            }
            BoolOp::Difference => {
                if let Some((c, id)) = current.take() {
                    current = Some((c.difference(&m), id));
                }
            }
            BoolOp::Intersection => {
                if let Some((c, id)) = current.take() {
                    current = Some((c.intersection(&m), id));
                }
            }
        }
    }
    if let Some((c, id)) = current {
        append_solid(&c, id, &mut out);
    }
    out
}

/// World-space triangle positions (3 per triangle) of ONE feature's raw primitive mesh,
/// before any boolean. Ray-pick uses this: testing real triangles lets a small body that
/// sits on top win over the big body that merely encloses it in its bounding box.
pub fn feature_world_positions(model: &Model, f: &Feature) -> Vec<[f32; 3]> {
    let m = world_mesh(f, model);
    let mut out = Vec::new();
    for poly in &m.polygons {
        for tri in poly.triangulate() {
            for v in tri {
                let p = v.position.coords;
                out.push([p.x as f32, p.y as f32, p.z as f32]);
            }
        }
    }
    out
}

/// Append a csgrs mesh's triangles (fan-triangulated) to a [`SolidMesh`], tagging each
/// triangle with the body's leading `feature_id`.
fn append_solid(m: &CsgMesh, feature_id: u32, out: &mut SolidMesh) {
    for poly in &m.polygons {
        for tri in poly.triangulate() {
            for v in tri {
                let p = v.position.coords;
                let n = v.normal;
                out.positions.push([p.x as f32, p.y as f32, p.z as f32]);
                out.normals.push([n.x as f32, n.y as f32, n.z as f32]);
            }
            out.face_ids.push(feature_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Placement, Plane, PlaneKind};

    fn boxf(w: f32, d: f32, h: f32) -> Primitive {
        Primitive::Box { w, d, h }
    }

    #[test]
    fn single_box_has_12_triangles() {
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(2.0, 2.0, 1.0));
        let mesh = m.eval();
        // A box = 6 quad faces = 12 triangles.
        assert_eq!(mesh.tri_count(), 12, "a plain box should tessellate to 12 tris");
    }

    /// GROUP semantics: two Union features are two INDEPENDENT bodies, concatenated —
    /// never booleaned together. Two separate boxes = 24 triangles, not a merged solid.
    /// This is what stops a ceiling from welding into a building.
    #[test]
    fn two_union_features_are_independent_bodies() {
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(2.0, 2.0, 1.0));
        m.push(BoolOp::Union, Plane::default(),
            Placement { u: 10.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 }, boxf(2.0, 2.0, 1.0));
        assert_eq!(m.eval().tri_count(), 24, "two Union bodies concatenate, not merge");
    }

    /// A Difference cuts only the CURRENT body — the building it was added onto — and a
    /// later Union body is untouched by it.
    #[test]
    fn a_later_union_is_not_cut_by_an_earlier_difference() {
        let mut m = Model::default();
        // Body 1: a box with a smaller box subtracted (a "room").
        m.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(4.0, 4.0, 3.0));
        m.push(BoolOp::Difference, Plane::default(),
            Placement { u: 0.0, v: 0.0, lift: 0.5, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 }, boxf(2.0, 2.0, 3.0));
        let carved = m.eval().tri_count();
        // Body 2: an independent "ceiling" added afterward.
        m.push(BoolOp::Union, Plane::default(),
            Placement { u: 0.0, v: 0.0, lift: 3.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 }, boxf(4.0, 4.0, 0.2));
        let with_ceiling = m.eval().tri_count();
        // The ceiling ADDS its own triangles; it does not re-fill (reduce) the carved body.
        assert!(with_ceiling > carved, "the ceiling is added, not merged into the carve");
    }

    #[test]
    fn difference_adds_geometry_and_stays_bounded() {
        // 2×2×2 box minus a 1×1 cylinder punched through the top.
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(2.0, 2.0, 2.0));
        m.push(
            BoolOp::Difference,
            Plane::default(),
            Placement { u: 0.0, v: 0.0, lift: 0.5, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            Primitive::Cylinder { r: 0.5, h: 2.0, sides: 24 },
        );
        let cut = m.eval();
        let plain = {
            let mut b = Model::default();
            b.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(2.0, 2.0, 2.0));
            b.eval()
        };
        // Subtracting a hole cannot yield fewer triangles than the plain box.
        assert!(cut.tri_count() > plain.tri_count(), "difference should add cut geometry");
        // Result stays within the original box footprint (±small epsilon).
        let (mn, mx) = cut.bounds().expect("non-empty");
        assert!(mn[0] >= -1.01 && mx[0] <= 1.01, "x within box");
        assert!(mn[1] >= -1.01 && mx[1] <= 1.01, "y within box");
    }

    #[test]
    fn eval_is_deterministic() {
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane { kind: PlaneKind::XZ, offset: 0.3, custom: None }, Placement::default(), boxf(1.0, 1.0, 1.0));
        let a = m.eval();
        let b = m.eval();
        assert_eq!(a.positions, b.positions, "same model must yield the same mesh");
    }
}

#[cfg(test)]
mod aabb_truth_tests {
    use super::*;
    use crate::{Placement, Plane};

    fn mesh_bounds(p: Primitive) -> ([f32; 3], [f32; 3]) {
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane::default(), Placement::default(), p);
        m.eval().bounds().expect("non-empty mesh")
    }

    /// The declared `local_aabb()` must match the mesh csgrs ACTUALLY builds.
    /// This is the test that verifies origin conventions (sphere centred vs resting,
    /// the torus revolve axis) by MEASURING instead of assuming — `world_aabb` drives
    /// picking, selection boxes and zoom-extents, so a wrong AABB means you cannot
    /// click the thing you can see.
    #[test]
    fn local_aabb_matches_real_mesh() {
        let cases: Vec<(&str, Primitive)> = vec![
            ("box", Primitive::Box { w: 2.0, d: 3.0, h: 1.0 }),
            ("cylinder", Primitive::Cylinder { r: 1.0, h: 2.0, sides: 32 }),
            ("sphere", Primitive::Sphere { r: 1.0, segments: 24, stacks: 12 }),
            ("cone", Primitive::Frustum { r_bottom: 1.0, r_top: 0.0, h: 2.0, sides: 32 }),
            ("prism", Primitive::Frustum { r_bottom: 1.0, r_top: 1.0, h: 2.0, sides: 6 }),
            ("torus", Primitive::Torus { major_r: 2.0, minor_r: 0.5, seg_major: 24, seg_minor: 12 }),
            ("capsule", Primitive::Capsule { r: 0.5, h: 2.0, segments: 24, stacks: 8 }),
            ("tube", Primitive::Tube { r_outer: 1.0, r_inner: 0.6, h: 2.0, sides: 32 }),
            ("ellipsoid", Primitive::Ellipsoid { rx: 1.0, ry: 2.0, rz: 0.5, segments: 24, stacks: 12 }),
        ];
        let mut bad = Vec::new();
        for (name, p) in cases {
            let (dmn, dmx) = p.local_aabb();
            let (amn, amx) = mesh_bounds(p);
            // An AABB owes CONTAINMENT, not tightness: it must never clip the real
            // mesh (that would make geometry unpickable — the dangerous direction).
            // Being slightly loose is fine and expected, because a faceted n-gon is
            // strictly inside its circumradius (a hexagon of r=1 only reaches
            // y = sin60° = 0.866). So: must contain, and must not be absurdly loose.
            let eps = 1e-3;   // float slack
            let slack = 0.30; // a wrong AXIS is off by ~radius and still caught
            let contains = (0..3).all(|k| dmn[k] <= amn[k] + eps && dmx[k] >= amx[k] - eps);
            let tight = (0..3).all(|k| (amn[k] - dmn[k]).abs() < slack && (dmx[k] - amx[k]).abs() < slack);
            if !contains || !tight {
                let why = if !contains { "CLIPS the mesh" } else { "far too loose" };
                bad.push(format!(
                    "  {name} — {why}\n     declared min={:?} max={:?}\n     ACTUAL   min={amn:?} max={amx:?}",
                    dmn.to_array(), dmx.to_array()
                ));
            }
        }
        assert!(bad.is_empty(), "local_aabb disagrees with the real csgrs mesh:\n{}", bad.join("\n"));
    }

    /// A tube must be HOLLOW — the bore has to actually remove geometry.
    #[test]
    fn tube_is_hollow() {
        let solid = mesh_bounds(Primitive::Cylinder { r: 1.0, h: 2.0, sides: 32 });
        let tube = mesh_bounds(Primitive::Tube { r_outer: 1.0, r_inner: 0.6, h: 2.0, sides: 32 });
        // same outer envelope…
        assert!((solid.1[0] - tube.1[0]).abs() < 0.12, "tube keeps the outer radius");
        // …but the difference added the bore wall, so it has strictly more triangles
        let mut a = Model::default();
        a.push(BoolOp::Union, Plane::default(), Placement::default(),
               Primitive::Cylinder { r: 1.0, h: 2.0, sides: 32 });
        let mut b = Model::default();
        b.push(BoolOp::Union, Plane::default(), Placement::default(),
               Primitive::Tube { r_outer: 1.0, r_inner: 0.6, h: 2.0, sides: 32 });
        assert!(b.eval().tri_count() > a.eval().tri_count(), "the bore must add geometry");
    }

    /// A capsule is taller than its barrel by exactly its two caps (h + 2r).
    #[test]
    fn capsule_has_hemispherical_caps() {
        let (mn, mx) = mesh_bounds(Primitive::Capsule { r: 0.5, h: 2.0, segments: 24, stacks: 8 });
        assert!((mn[2] - 0.0).abs() < 0.05, "rests on the plane, got z-min {}", mn[2]);
        assert!((mx[2] - 3.0).abs() < 0.05, "h + 2r = 3.0, got z-max {}", mx[2]);
    }
}
