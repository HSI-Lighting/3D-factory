//! The csgrs boundary — the ONLY place glam↔nalgebra conversion happens.
//!
//! `Model` (glam f32, UI-facing) → csgrs `Mesh<()>` (nalgebra f64, BSP CSG) →
//! [`SolidMesh`] (glam f32, render/wire-in). Keeping this in one file means the
//! rest of `cad_solid` never sees nalgebra or csgrs.

use csgrs::csg::CSG;
use csgrs::mesh::Mesh;
use csgrs::sketch::Sketch;
use nalgebra::{Matrix4, Point3, Vector3};

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
        // Sweep the centred cross-section along the open path — csgrs keeps the section
        // perpendicular to the path (aims sketch +Z at the tangent) and caps the ends.
        Primitive::Sweep { profile, path, .. } => match (model.profile(profile), model.path(path)) {
            (Some(pr), Some(pa)) if pa.pts.len() >= 2 => {
                let pts: Vec<[f64; 2]> =
                    pr.pts.iter().map(|q| [q[0] as f64, q[1] as f64]).collect();
                let path3: Vec<Point3<f64>> = pa
                    .pts
                    .iter()
                    .map(|q| Point3::new(q[0] as f64, q[1] as f64, q[2] as f64))
                    .collect();
                Sketch::polygon(&pts, ()).sweep(&path3)
            }
            _ => CsgMesh::new(),
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
    eval_inner(model, None)
}

/// What ONE feature cost to fold in. See [`EvalProfile`].
#[derive(Clone, Debug)]
pub struct FeatureCost {
    pub id: u32,
    pub op: BoolOp,
    /// Primitive kind, for reading the table without cross-referencing the model.
    pub kind: &'static str,
    /// Wall-clock for this feature's own boolean, in milliseconds.
    pub ms: f64,
    /// Polygons in this feature's OWN mesh — the second operand of the boolean, and the thing
    /// a bounding-box pre-partition gets to shrink.
    pub polys_operand: usize,
    /// Polygons in the running body AFTER this step. A Union starts a new body, so this is its
    /// own count; a Difference's is the body it just cut.
    pub polys_body: usize,
}

/// Where the time in an evaluation actually goes.
///
/// EVERY DOWNSTREAM THRESHOLD IN THE PLAN WAS ESTIMATED, and two of them were estimated against a
/// BSP that was collapsing because the boolean tolerance was wrong. This is the measurement that
/// replaces the estimates: per-feature cost against a real project, not a synthetic one.
///
/// `deepest_operand` is the recursion bound that matters. csgrs's BSP degenerates to a linked list
/// for a convex body, and `Node::build`, `clip_polygons`, `clip_to` and `Drop` all recurse, so the
/// largest single mesh fed to a boolean is what decides how close an evaluation comes to the
/// stack. It is the number `Model::eval`'s 64 MB thread was sized against.
#[derive(Clone, Debug, Default)]
pub struct EvalProfile {
    pub total_ms: f64,
    pub features: Vec<FeatureCost>,
    pub bodies: usize,
    pub tris: usize,
    pub deepest_operand: usize,
    /// Features skipped because [`crate::Feature::enabled`] is clear.
    pub disabled: usize,
}

impl EvalProfile {
    /// The `n` most expensive features, worst first — the only part of a 4,000-feature table
    /// anyone reads.
    pub fn worst(&self, n: usize) -> Vec<&FeatureCost> {
        let mut v: Vec<&FeatureCost> = self.features.iter().collect();
        v.sort_by(|a, b| b.ms.total_cmp(&a.ms));
        v.truncate(n);
        v
    }
}

/// [`eval`], with a per-feature cost table. Same fold, same result — deliberately the SAME
/// FUNCTION with the profiler switched on, because a profiler that walks its own copy of the loop
/// measures a code path the app does not run.
///
/// Runs on the caller's stack, unlike [`Model::eval`]: this is a diagnostic, and a diagnostic that
/// silently used a different stack size would not be measuring the thing being diagnosed. Call it
/// from a big-stack thread yourself if the model is deep.
pub fn eval_profiled(model: &Model) -> (SolidMesh, EvalProfile) {
    let mut p = EvalProfile::default();
    let mesh = eval_inner(model, Some(&mut p));
    p.tris = mesh.tri_count();
    (mesh, p)
}

fn eval_inner(model: &Model, mut prof: Option<&mut EvalProfile>) -> SolidMesh {
    let t_all = std::time::Instant::now();
    let mut out = SolidMesh::default();
    // The leading feature id of the body currently accumulating — every triangle it
    // produces is tagged with it, so the app can colour a body by its feature.
    let mut current: Option<(CsgMesh, u32)> = None;
    for f in &model.features {
        // A DISABLED FEATURE IS SKIPPED — and skipping a disabled UNION also ENDS its body,
        // rather than letting the cuts that follow it fall through onto the previous one.
        //
        // Dropping the body and keeping its cuts would re-bind them, which is the exact
        // corruption `Feature::enabled` was added to refuse. So the body is flushed and
        // `current` cleared: the trailing Differences then meet no body and are dropped, the
        // same way `eval` already drops a leading Difference. Disabling a body disables what it
        // was opened by; it never moves those openings onto a neighbour.
        if !f.enabled {
            if f.op == BoolOp::Union {
                if let Some((c, id)) = current.take() {
                    append_solid(&c, id, &mut out);
                }
            }
            if let Some(p) = prof.as_deref_mut() {
                p.disabled += 1;
            }
            continue;
        }
        let t = std::time::Instant::now();
        let m = world_mesh(f, model);
        let operand = m.polygons.len();
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
        if let Some(p) = prof.as_deref_mut() {
            // The BODY count is read AFTER the boolean, so a Difference reports what the cut
            // left behind rather than what it started with — that is the number that grows.
            // THE LARGEST MESH EVER FED TO A BOOLEAN, not the last one — that is the figure the
            // eval stack has to survive, and a model rarely ends on its biggest solid.
            p.deepest_operand = p.deepest_operand.max(operand);
            let body = current.as_ref().map_or(0, |(c, _)| c.polygons.len());
            p.deepest_operand = p.deepest_operand.max(body);
            p.features.push(FeatureCost {
                id: f.id,
                op: f.op,
                kind: primitive_kind(&f.primitive),
                ms: t.elapsed().as_secs_f64() * 1000.0,
                polys_operand: operand,
                polys_body: body,
            });
        }
    }
    if let Some((c, id)) = current {
        append_solid(&c, id, &mut out);
    }
    if let Some(p) = prof.as_deref_mut() {
        p.total_ms = t_all.elapsed().as_secs_f64() * 1000.0;
        p.bodies = out.face_ids.iter().collect::<std::collections::HashSet<_>>().len();
    }
    out
}

/// Primitive kind as a short word, for a diagnostic table.
fn primitive_kind(p: &Primitive) -> &'static str {
    match p {
        Primitive::Box { .. } => "Box",
        Primitive::Cylinder { .. } => "Cylinder",
        Primitive::Sphere { .. } => "Sphere",
        Primitive::Frustum { .. } => "Frustum",
        Primitive::Torus { .. } => "Torus",
        Primitive::Capsule { .. } => "Capsule",
        Primitive::Tube { .. } => "Tube",
        Primitive::Ellipsoid { .. } => "Ellipsoid",
        Primitive::Extrusion { .. } => "Extrusion",
        Primitive::Sweep { .. } => "Sweep",
    }
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

    /// A DISABLED CUTTER DOES NOT CUT — the flag has to reach the geometry, or "kept and flagged"
    /// is just "kept", and an orphaned opening goes on quietly making a hole in the wrong wall.
    ///
    /// Asserted on the BOUNDS rather than the triangle count: a cut that removes the whole +X end
    /// of the body shortens the mesh, and that is a fact about the solid rather than about how
    /// csgrs happened to tessellate it.
    #[test]
    fn a_disabled_difference_does_not_cut() {
        // A 4 × 1 × 1 bar centred on the origin, so it spans x ∈ [-2, 2].
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(4.0, 1.0, 1.0));
        // A cutter that swallows everything past x = 1: centred at x = 2, 2 wide.
        let cut = m.push(
            BoolOp::Difference,
            Plane::default(),
            Placement { u: 2.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            boxf(2.0, 2.0, 2.0),
        );

        let (_, mx) = m.eval().bounds().expect("the bar has bounds");
        assert!(
            (mx[0] - 1.0).abs() < 1e-3,
            "with the cutter enabled the bar must end at x = 1, got {}",
            mx[0]
        );

        assert_eq!(m.set_enabled(cut, false), Some(true), "the cutter was enabled before");
        let (_, mx) = m.eval().bounds().expect("the bar still has bounds");
        assert!(
            (mx[0] - 2.0).abs() < 1e-3,
            "a DISABLED cutter still cut: the bar should reach x = 2 again, got {}",
            mx[0]
        );
    }

    /// DISABLING A BODY ALSO ENDS IT — its openings must not fall through onto the previous body.
    ///
    /// `eval` folds each Difference onto the most recent Union. If a disabled Union were merely
    /// skipped, the cutters behind it would meet the body BEFORE it and start cutting that
    /// instead — re-binding an opening to a neighbour, which is the exact corruption the flag
    /// exists to refuse. So the body is flushed and its cuts meet nothing.
    #[test]
    fn disabling_a_body_does_not_hand_its_cuts_to_the_previous_body() {
        let mut m = Model::default();
        // Body A: a 4 × 1 × 1 bar at the origin. Untouched by anything that follows.
        m.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(4.0, 1.0, 1.0));
        // Body B, well clear of A, with a cutter of its own sitting behind it.
        let b = m.push(
            BoolOp::Union,
            Plane::default(),
            Placement { u: 20.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            boxf(4.0, 1.0, 1.0),
        );
        m.push(
            BoolOp::Difference,
            Plane::default(),
            // Overlaps A, NOT B — so if this cut ever reaches A it is unmistakable.
            Placement { u: 2.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            boxf(2.0, 2.0, 2.0),
        );

        m.set_enabled(b, false);
        let (mn, mx) = m.eval().bounds().expect("body A survives");
        assert!(
            (mn[0] + 2.0).abs() < 1e-3 && (mx[0] - 2.0).abs() < 1e-3,
            "body B's cutter reached body A: A should still span x ∈ [-2, 2], got [{}, {}]",
            mn[0], mx[0]
        );
    }


    /// THE PROFILER MUST MEASURE THE EVALUATION THE APP ACTUALLY RUNS.
    ///
    /// `eval` and `eval_profiled` are the same function with the profiler switched on, and that
    /// is the point: a profiler that walks its own copy of the fold measures a code path nobody
    /// ships. This pins the two together on the thing that would diverge first — the mesh.
    #[test]
    fn profiling_an_evaluation_does_not_change_it() {
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(4.0, 2.0, 2.0));
        m.push(
            BoolOp::Difference,
            Plane::default(),
            Placement { u: 1.0, v: 0.0, lift: 0.5, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            boxf(1.0, 4.0, 1.0),
        );
        // A DISABLED FEATURE IS IN THE FIXTURE ON PURPOSE. It is the cheapest thing for a
        // second copy of the fold to get wrong, and the difference would be invisible on any
        // model that has none.
        let off = m.push(
            BoolOp::Difference,
            Plane::default(),
            Placement { u: -1.0, v: 0.0, lift: 0.5, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            boxf(1.0, 4.0, 1.0),
        );
        m.set_enabled(off, false);

        let plain = eval(&m);
        let (profiled, p) = eval_profiled(&m);
        assert_eq!(plain.tri_count(), profiled.tri_count(), "profiling changed the mesh");
        assert_eq!(plain.positions, profiled.positions, "profiling moved a vertex");
        assert_eq!(p.tris, plain.tri_count(), "the profile disagrees with its own mesh");
        assert_eq!(p.disabled, 1, "the disabled feature was not seen as skipped");
    }

    /// EVERY APPLIED FEATURE IS ACCOUNTED FOR, and every skipped one is counted rather than
    /// silently missing — otherwise a model with disabled features would report a total that
    /// does not add up and nobody could tell which of the two numbers was wrong.
    #[test]
    fn the_profile_accounts_for_every_feature() {
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(4.0, 2.0, 2.0));
        let cut = m.push(
            BoolOp::Difference,
            Plane::default(),
            Placement { u: 1.0, v: 0.0, lift: 0.5, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            boxf(1.0, 4.0, 1.0),
        );
        m.push(
            BoolOp::Union,
            Plane::default(),
            Placement { u: 30.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            boxf(1.0, 1.0, 1.0),
        );

        let (_, p) = eval_profiled(&m);
        assert_eq!(p.features.len(), 3, "a feature went unmeasured");
        assert_eq!(p.disabled, 0, "nothing was disabled");
        assert_eq!(p.bodies, 2, "two Unions are two bodies");

        m.set_enabled(cut, false);
        let (_, p) = eval_profiled(&m);
        assert_eq!(p.features.len(), 2, "a disabled feature was still measured as work done");
        assert_eq!(p.disabled, 1, "the skipped feature was not counted");
    }

    /// `deepest_operand` IS THE STACK NUMBER. csgrs's BSP degenerates to a linked list on a convex
    /// body and recurses about once per polygon, so the largest single mesh in an evaluation is
    /// what decides how close it comes to overflowing — the figure `Model::eval`'s 64 MB thread
    /// was sized against, and until now the one nobody had measured on a real model.
    #[test]
    fn the_profile_reports_the_largest_mesh_it_fed_to_a_boolean() {
        let mut m = Model::default();
        // A 64-sided cylinder is far more polygons than the box that cuts it, so the deepest
        // operand must be the cylinder's — not the last thing evaluated.
        m.push(
            BoolOp::Union, Plane::default(), Placement::default(),
            Primitive::Cylinder { r: 3.0, h: 2.0, sides: 64 },
        );
        m.push(BoolOp::Difference, Plane::default(), Placement::default(), boxf(1.0, 1.0, 4.0));
        // A TINY BODY LAST, and this is the part that makes the assertion mean anything: it
        // starts a new body, so whatever is "current" at the end is six polygons. A profiler
        // that reported the LAST body instead of the LARGEST mesh would look correct on any
        // model that happens to end on its biggest solid, and would understate the stack risk
        // on every model that does not.
        m.push(
            BoolOp::Union,
            Plane::default(),
            Placement { u: 40.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            boxf(0.5, 0.5, 0.5),
        );

        let (_, p) = eval_profiled(&m);
        let cyl = p.features.iter().find(|f| f.kind == "Cylinder").expect("the cylinder was measured");
        assert!(cyl.polys_operand > 60, "a 64-sided cylinder should be > 60 polygons, got {}", cyl.polys_operand);
        let last = p.features.last().expect("a last feature");
        assert!(
            last.polys_body < cyl.polys_operand,
            "the fixture must END on a small body, or this proves nothing: {} vs {}",
            last.polys_body, cyl.polys_operand,
        );
        assert!(
            p.deepest_operand >= cyl.polys_operand,
            "the deepest operand ({}) is smaller than a mesh that was actually evaluated ({}) — \
             the stack figure is being read off the last body rather than the biggest one",
            p.deepest_operand, cyl.polys_operand,
        );
    }

    /// The worst-N view is what a 4,000-feature table is actually read through, so it has to be
    /// sorted the way it claims.
    #[test]
    fn the_worst_features_come_back_worst_first() {
        let mut m = Model::default();
        m.push(
            BoolOp::Union, Plane::default(), Placement::default(),
            Primitive::Cylinder { r: 3.0, h: 2.0, sides: 64 },
        );
        for i in 0..4 {
            m.push(
                BoolOp::Difference, Plane::default(),
                Placement { u: i as f32 * 0.5, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
                boxf(0.4, 0.4, 4.0),
            );
        }
        let (_, p) = eval_profiled(&m);
        let worst = p.worst(3);
        assert_eq!(worst.len(), 3, "asked for three");
        assert!(worst[0].ms >= worst[1].ms && worst[1].ms >= worst[2].ms, "not sorted worst-first");
        assert!(p.worst(999).len() == p.features.len(), "asking for more than there are is not an error");
    }
    #[test]
    fn single_box_has_12_triangles() {
        let mut m = Model::default();
        m.push(BoolOp::Union, Plane::default(), Placement::default(), boxf(2.0, 2.0, 1.0));
        let mesh = m.eval();
        // A box = 6 quad faces = 12 triangles.
        assert_eq!(mesh.tri_count(), 12, "a plain box should tessellate to 12 tris");
    }

    /// A `Sweep` extrudes a closed cross-section along an open path — the swept solid must
    /// have geometry and span the path length.
    #[test]
    fn sweep_along_a_straight_path_makes_a_solid() {
        let mut m = Model::default();
        let sq = [
            glam::Vec2::new(-0.2, -0.2), glam::Vec2::new(0.2, -0.2),
            glam::Vec2::new(0.2, 0.2), glam::Vec2::new(-0.2, 0.2),
        ];
        let (profile, _c, _w, _d) = m.add_profile(&sq).unwrap();
        // A straight 5 m path along +X, on the plane (z = 0).
        let (path, _mn, _mx) = m
            .add_path(&[glam::Vec3::new(0.0, 0.0, 0.0), glam::Vec3::new(5.0, 0.0, 0.0)])
            .unwrap();
        m.push(
            BoolOp::Union, Plane::default(), Placement::default(),
            Primitive::Sweep { profile, path, bmin: [-0.5, -0.5, -0.5], bmax: [5.5, 0.5, 0.5] },
        );
        let mesh = m.eval();
        assert!(mesh.tri_count() > 0, "a swept solid must produce geometry");
        let (mn, mx) = mesh.bounds().expect("swept solid has bounds");
        assert!(mx[0] - mn[0] > 4.5, "swept bar should span the ~5 m path, got {}", mx[0] - mn[0]);
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
