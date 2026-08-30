//! Turn a 2D `cad_kernel::Document` into the 3D surfaces the lux engine lights.
//!
//! Rule: one drafted line/wall → one vertical surface (no solid boxes). A closed
//! path (or a circle) also gets a floor + ceiling. Engine world is Z-up.
use cad_kernel::{Arc as KArc, Document, Geom, Vec2};

use crate::types::{MaterialId, Mesh, Triangle, Vertex};

const FLOOR: MaterialId = 0;
const WALL: MaterialId = 1;
const CEILING: MaterialId = 2;
const CURVE_SEGMENTS: usize = 48;

/// `k` = metres per drawing unit. The engine's world is METRES (see `types::Vertex`), but the
/// document's X/Y are drawing units while `z`/`height` already arrive in metres — so without
/// this scale a wall quad pairs millimetre X/Y with a metre Z in the same vertex, and the
/// inverse-square law in `calc` is evaluated on nonsense.
fn vtx(p: Vec2, z: f32, k: f64) -> Vertex {
    Vertex::new((p.x * k) as f32, (p.y * k) as f32, z)
}

fn surface(a: Vec2, b: Vec2, height: f32, material: MaterialId, k: f64) -> Mesh {
    Mesh {
        vertices: vec![vtx(a, 0.0, k), vtx(b, 0.0, k), vtx(b, height, k), vtx(a, height, k)],
        triangles: vec![Triangle { a: 0, b: 1, c: 2 }, Triangle { a: 0, b: 2, c: 3 }],
        material,
    }
}

fn cap(poly: &[Vec2], z: f32, material: MaterialId, out: &mut Vec<Mesh>, k: f64) {
    let p2: Vec<[f32; 2]> = poly.iter().map(|v| [(v.x * k) as f32, (v.y * k) as f32]).collect();
    let tris = triangulate(&p2);
    if tris.is_empty() {
        return;
    }
    out.push(Mesh {
        vertices: p2.iter().map(|p| Vertex::new(p[0], p[1], z)).collect(),
        triangles: tris.iter().map(|t| Triangle { a: t[0] as u32, b: t[1] as u32, c: t[2] as u32 }).collect(),
        material,
    });
}

fn extrude_path(pts: &[Vec2], closed: bool, height: f32, out: &mut Vec<Mesh>, k: f64) {
    for w in pts.windows(2) {
        out.push(surface(w[0], w[1], height, WALL, k));
    }
    if closed && pts.len() >= 3 {
        out.push(surface(pts[pts.len() - 1], pts[0], height, WALL, k));
        cap(pts, 0.0, FLOOR, out, k);
        cap(pts, height, CEILING, out, k);
    }
}

fn circle_pts(center: Vec2, radius: f64) -> Vec<Vec2> {
    (0..CURVE_SEGMENTS)
        .map(|i| {
            let a = std::f64::consts::TAU * (i as f64 / CURVE_SEGMENTS as f64);
            Vec2::new(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect()
}

/// An ellipse as a closed ring, sampled in PARAMETER space.
///
/// `point_at` takes the parameter `t`, not a geometric angle at the centre — for a stretched
/// ellipse those differ, and sampling the angle uniformly would bunch the points at the ends of
/// the major axis and thin them at the sides, which is where a room's wall is longest.
fn ellipse_pts(e: &cad_kernel::Ellipse) -> Vec<Vec2> {
    (0..CURVE_SEGMENTS)
        .map(|i| e.point_at(std::f64::consts::TAU * (i as f64 / CURVE_SEGMENTS as f64)))
        .collect()
}

fn ellipse_arc_pts(ea: &cad_kernel::EllipseArc) -> Vec<Vec2> {
    (0..=CURVE_SEGMENTS)
        .map(|i| {
            let t = i as f64 / CURVE_SEGMENTS as f64;
            ea.ellipse.point_at(ea.start_param + ea.sweep_param * t)
        })
        .collect()
}

/// A spline as a point run, plus whether it closes back on itself.
///
/// A clamped NURBS interpolates its first and last control points, so a room traced as a closed
/// spline comes back to where it started and this reports `true`. When it does, the duplicated end
/// point is DROPPED: `extrude_path` closes a ring by joining the last point to the first, and
/// leaving the duplicate in place would hand it a zero-length wall to build a surface from.
///
/// The closure tolerance is RELATIVE to the curve's own size rather than absolute. This crate is
/// fed documents in metres and in millimetres, and a fixed epsilon that is generous at 1 m/unit is
/// a thousand times too strict at 0.001 — the same room would close in one unit and stand open in
/// the other, which is exactly the class of unit bug this file already carries tests against.
fn spline_pts(s: &cad_kernel::Spline) -> (Vec<Vec2>, bool) {
    let mut pts = s.tessellate(CURVE_SEGMENTS + 1);
    if pts.len() < 2 {
        return (pts, false);
    }
    let (first, last) = (pts[0], pts[pts.len() - 1]);
    let span = pts.iter().fold(0.0_f64, |acc, p| acc.max(p.dist(first)));
    let closed = span > 0.0 && first.dist(last) <= 1e-4 * span;
    if closed {
        pts.pop();
    }
    let closed = closed && pts.len() >= 3;
    (pts, closed)
}

fn arc_pts(a: &KArc) -> Vec<Vec2> {
    (0..=CURVE_SEGMENTS)
        .map(|i| {
            let t = i as f64 / CURVE_SEGMENTS as f64;
            let ang = a.start_angle + a.sweep_angle * t;
            Vec2::new(a.center.x + a.radius * ang.cos(), a.center.y + a.radius * ang.sin())
        })
        .collect()
}

/// Extrude ONE geometry to surfaces at `height` (closed paths also get
/// floor + ceiling). Shared by `extrude` (whole doc) and `extrude_handles`
/// (SIMLUX per-layer room build), so both stay in lock-step.
fn extrude_geom(geom: &Geom, height: f32, out: &mut Vec<Mesh>, k: f64) {
    match geom {
        Geom::Line(l) => extrude_path(&[l.a, l.b], false, height, out, k),
        Geom::Wall(w) => extrude_path(&[w.start, w.end], false, height, out, k),
        Geom::Polyline(p) => {
            let v: Vec<Vec2> = p.vertices.iter().map(|x| x.pos).collect();
            extrude_path(&v, p.closed, height, out, k);
        }
        Geom::Circle(c) => extrude_path(&circle_pts(c.center, c.radius), true, height, out, k),
        Geom::Arc(a) => extrude_path(&arc_pts(a), false, height, out, k),
        Geom::Ellipse(e) => extrude_path(&ellipse_pts(e), true, height, out, k),
        Geom::EllipseArc(ea) => extrude_path(&ellipse_arc_pts(ea), false, height, out, k),
        Geom::Spline(s) => {
            let (pts, closed) = spline_pts(s);
            extrude_path(&pts, closed, height, out, k);
        }
        // EVERYTHING BELOW IS DROPPED ON PURPOSE, and saying so is the point — this arm used to
        // swallow the two above it as well, so a room traced with an ellipse was lit AS IF IT HAD
        // NO WALLS: no error, no warning, a plausible lux figure somebody sizes an installation
        // from.
        //
        //   Text, Dimension — annotation, not building fabric. A drawing full of dimension
        //     strings must not light like a maze.
        //   Hatch — a fill. Its boundary is other objects, which extrude on their own account;
        //     extruding it too would double every wall it touches.
        //   BlockRef — deliberately NOT resolved here. It would turn every piece of imported
        //     furniture into a wall. Whether a block's contents should light is a real question,
        //     and it deserves an answer rather than a side effect.
        _ => {}
    }
}

/// Extrude every drafted entity to surfaces (closed paths also get floor + ceiling).
pub fn extrude(doc: &Document, height: f32) -> Vec<Mesh> {
    let mut out = Vec::new();
    let k = doc.units.metres_per_unit;
    for d in &doc.dobjects {
        extrude_geom(&d.geom, height, &mut out, k);
    }
    out
}

/// Extrude ONLY the dobjects named by `handles`, at `height` — the SIMLUX
/// per-layer room build (each imported layer extrudes to its own height).
/// Handles that no longer exist in `doc` are silently skipped.
pub fn extrude_handles(doc: &Document, handles: &[u64], height: f32) -> Vec<Mesh> {
    let mut out = Vec::new();
    let k = doc.units.metres_per_unit;
    for &h in handles {
        if let Some(d) = doc.find_by_handle(h) {
            extrude_geom(&d.geom, height, &mut out, k);
        }
    }
    out
}

/// Bounding box of all drafted geometry (x-min, y-min, x-max, y-max), in METRES — it sizes the
/// calc plane, which the engine treats as metres.
pub fn bbox(doc: &Document) -> Option<(f32, f32, f32, f32)> {
    let (mut mnx, mut mny, mut mxx, mut mxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut any = false;
    let k = doc.units.metres_per_unit;
    let mut add = |v: Vec2| {
        any = true;
        mnx = mnx.min((v.x * k) as f32);
        mny = mny.min((v.y * k) as f32);
        mxx = mxx.max((v.x * k) as f32);
        mxy = mxy.max((v.y * k) as f32);
    };
    for d in &doc.dobjects {
        match &d.geom {
            Geom::Line(l) => { add(l.a); add(l.b); }
            Geom::Wall(w) => { add(w.start); add(w.end); }
            Geom::Polyline(p) => p.vertices.iter().for_each(|x| add(x.pos)),
            Geom::Circle(c) => {
                add(Vec2::new(c.center.x - c.radius, c.center.y - c.radius));
                add(Vec2::new(c.center.x + c.radius, c.center.y + c.radius));
            }
            Geom::Arc(a) => arc_pts(a).into_iter().for_each(&mut add),
            // MUST MATCH `extrude_geom` ABOVE. This sizes the calculation plane, so a shape that
            // extrudes to walls but is missing here gets a plane that does not cover it — the
            // room is built and then only partly measured, which is the same wrong answer arriving
            // by a different route.
            Geom::Ellipse(e) => ellipse_pts(e).into_iter().for_each(&mut add),
            Geom::EllipseArc(ea) => ellipse_arc_pts(ea).into_iter().for_each(&mut add),
            Geom::Spline(s) => spline_pts(s).0.into_iter().for_each(&mut add),
            _ => {}
        }
    }
    any.then_some((mnx, mny, mxx, mxy))
}

/// A closed rectangular room (floor + 4 walls + ceiling) — a demo/test stand-in.
pub fn box_room(width: f32, depth: f32, height: f32) -> Vec<Mesh> {
    let (w, d, h) = (width, depth, height);
    let quad = |p0: Vertex, p1: Vertex, p2: Vertex, p3: Vertex, material: MaterialId| Mesh {
        vertices: vec![p0, p1, p2, p3],
        triangles: vec![Triangle { a: 0, b: 1, c: 2 }, Triangle { a: 0, b: 2, c: 3 }],
        material,
    };
    let v = Vertex::new;
    vec![
        quad(v(0.0, 0.0, 0.0), v(w, 0.0, 0.0), v(w, d, 0.0), v(0.0, d, 0.0), FLOOR),
        quad(v(0.0, 0.0, h), v(0.0, d, h), v(w, d, h), v(w, 0.0, h), CEILING),
        quad(v(0.0, 0.0, 0.0), v(0.0, d, 0.0), v(0.0, d, h), v(0.0, 0.0, h), WALL),
        quad(v(w, 0.0, 0.0), v(w, 0.0, h), v(w, d, h), v(w, d, 0.0), WALL),
        quad(v(0.0, 0.0, 0.0), v(0.0, 0.0, h), v(w, 0.0, h), v(w, 0.0, 0.0), WALL),
        quad(v(0.0, d, 0.0), v(w, d, 0.0), v(w, d, h), v(0.0, d, h), WALL),
    ]
}

// --- ear-clipping triangulation for a simple polygon (no holes) ---

fn signed_area(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum::<f32>()
        * 0.5
}

fn cross(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn in_tri(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Ear-clip a simple polygon into triangle index triples.
pub fn triangulate(poly: &[[f32; 2]]) -> Vec<[usize; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    let mut idx: Vec<usize> = (0..n).collect();
    if signed_area(poly) < 0.0 {
        idx.reverse();
    }
    let mut tris = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            let mut ear = true;
            for &j in &idx {
                if j != ia && j != ib && j != ic && in_tri(poly[j], a, b, c) {
                    ear = false;
                    break;
                }
            }
            if ear {
                tris.push([ia, ib, ic]);
                idx.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_kernel::{DObject, Polyline, PolyVertex};

    #[test]
    fn closed_polyline_extrudes_with_caps() {
        let mut doc = Document::default();
        let verts: Vec<PolyVertex> = [(0.0, 0.0), (4.0, 0.0), (4.0, 3.0), (0.0, 3.0)]
            .iter()
            .map(|&(x, y)| PolyVertex { pos: Vec2::new(x, y), bulge: 0.0 })
            .collect();
        doc.push(DObject::new(Geom::Polyline(Polyline { vertices: verts, closed: true, widths: Vec::new() })));
        let m = extrude(&doc, 3.0);
        // 4 wall surfaces + floor + ceiling.
        assert_eq!(m.len(), 6);
        assert!(m.iter().any(|x| x.material == FLOOR));
        assert!(m.iter().any(|x| x.material == CEILING));
    }

    #[test]
    fn triangulate_square() {
        assert_eq!(triangulate(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]).len(), 2);
    }

    // ── THE SAME ROOM IN DIFFERENT UNITS IS THE SAME ROOM ──────────────────────────────────
    //
    // Every function here scales X and Y by `doc.units.metres_per_unit`, because the engine's
    // world is METRES while the document's coordinates are drawing units. Get that factor wrong
    // and the inverse-square law in `calc` is evaluated on nonsense — a millimetre plan lit as
    // though it were a kilometre across.
    //
    // THERE WAS NO TEST FOR IT. Coverage pinned units at the IO layer only; nothing exercised
    // this crate at a non-default unit, so the factor could be dropped, doubled or inverted and
    // the suite would stay green. That is the same shape as the "1000 m solid" defect, in the one
    // crate whose numbers a lighting designer acts on.
    //
    // The test is an EQUIVALENCE, which is the only form that cannot be satisfied by writing down
    // whatever the code currently does: the same physical room, drafted in metres and in
    // millimetres, must extrude to the same world geometry.

    fn room_doc(unit_m: f64, side: f64) -> Document {
        let mut d = Document::default();
        d.units = cad_kernel::Units::from_metres_per_unit(unit_m, cad_kernel::UnitSource::Declared);
        let verts: Vec<PolyVertex> = [(0.0, 0.0), (side, 0.0), (side, side), (0.0, side)]
            .iter()
            .map(|&(x, y)| PolyVertex { pos: Vec2::new(x, y), bulge: 0.0 })
            .collect();
        d.push(DObject::new(Geom::Polyline(Polyline {
            vertices: verts, closed: true, widths: Vec::new(),
        })));
        d
    }

    /// A 4 m room drawn in metres and the SAME 4 m room drawn in millimetres must produce
    /// identical world geometry. If the scale factor is ever dropped, the mm version comes out
    /// 1000× too big and this fails immediately.
    #[test]
    fn a_room_extrudes_the_same_whatever_unit_it_was_drafted_in() {
        let in_metres = extrude(&room_doc(1.0, 4.0), 2.7);
        let in_mm = extrude(&room_doc(0.001, 4000.0), 2.7);

        assert_eq!(in_metres.len(), in_mm.len(), "same room, different mesh count");
        for (a, b) in in_metres.iter().zip(in_mm.iter()) {
            assert_eq!(a.material, b.material);
            assert_eq!(a.vertices.len(), b.vertices.len());
            for (va, vb) in a.vertices.iter().zip(b.vertices.iter()) {
                assert!(
                    (va.x - vb.x).abs() < 1e-3 && (va.y - vb.y).abs() < 1e-3
                        && (va.z - vb.z).abs() < 1e-3,
                    "unit scaling lost: metres gave ({:.4}, {:.4}, {:.4}), \
                     millimetres gave ({:.4}, {:.4}, {:.4})",
                    va.x, va.y, va.z, vb.x, vb.y, vb.z,
                );
            }
        }
    }

    /// And the absolute value, not just the agreement — two wrong answers can agree with each
    /// other. A 4 m room's far corner is at 4 m in the engine's world, whatever it was drafted in.
    #[test]
    fn drafting_units_reach_the_engine_as_metres() {
        for (unit_m, side) in [(1.0, 4.0), (0.001, 4000.0), (0.01, 400.0)] {
            let meshes = extrude(&room_doc(unit_m, side), 2.7);
            let max_x = meshes.iter()
                .flat_map(|m| m.vertices.iter())
                .fold(f32::MIN, |acc, v| acc.max(v.x));
            assert!(
                (max_x - 4.0).abs() < 1e-3,
                "a 4 m room at {unit_m} m/unit reached the engine {max_x} m across",
            );
        }
    }

    /// `bbox` sizes the CALCULATION PLANE, so it has to scale identically. A plane that does not
    /// cover the room measures part of it and reports the average over the rest — a wrong lux
    /// figure with no symptom.
    #[test]
    fn the_calc_plane_bbox_is_in_metres_too() {
        for (unit_m, side) in [(1.0, 4.0), (0.001, 4000.0), (0.01, 400.0)] {
            let (mnx, mny, mxx, mxy) = bbox(&room_doc(unit_m, side)).expect("a room has a bbox");
            assert!(
                mnx.abs() < 1e-3 && mny.abs() < 1e-3
                    && (mxx - 4.0).abs() < 1e-3 && (mxy - 4.0).abs() < 1e-3,
                "bbox at {unit_m} m/unit came out ({mnx}, {mny})..({mxx}, {mxy}), not 0..4 metres",
            );
        }
    }

    /// The per-layer room build takes the same path and must scale the same way — it is the one
    /// SIMLUX actually calls, via the layer heights.
    #[test]
    fn the_per_layer_room_build_scales_too() {
        let doc = room_doc(0.001, 4000.0);
        let handles: Vec<u64> = doc.dobjects.iter().map(|d| d.handle).collect();
        let meshes = extrude_handles(&doc, &handles, 2.7);
        let max_x = meshes.iter()
            .flat_map(|m| m.vertices.iter())
            .fold(f32::MIN, |acc, v| acc.max(v.x));
        assert!((max_x - 4.0).abs() < 1e-3, "extrude_handles ignored the unit: {max_x} m");
    }

    // ── A ROOM IS A ROOM WHATEVER IT WAS DRAWN WITH ────────────────────────────────────────
    //
    // `extrude_geom`'s `_ => {}` arm silently discarded every curved entity, so a room traced
    // with an ellipse or a spline was LIT AS THOUGH IT HAD NO WALLS. No error, no warning —
    // a plausible-looking lux figure that someone sizes an installation from.
    //
    // Deliberately still dropped, and these are correct rather than missing: Text and Dimension
    // are annotation, not building fabric; Hatch is a fill whose boundary is other objects that
    // extrude on their own account; BlockRef is left alone on purpose, because resolving it here
    // would turn every imported furniture block into a wall.

    fn doc_with(geom: cad_kernel::Geom) -> Document {
        let mut d = Document::default();
        // The fixture geometries are written in METRE numbers (a 4 m spline room),
        // but a default document is now millimetre space — declare metres or the
        // bbox tests measure the room 1000x too small.
        d.units = cad_kernel::Units::from_metres_per_unit(
            1.0, cad_kernel::UnitSource::Declared);
        d.push(DObject::new(geom));
        d
    }

    /// The control. A circle already extruded, and it is the same shape as an ellipse of ratio 1 —
    /// without this pair the ellipse assertion could pass for the wrong reason.
    #[test]
    fn a_circular_room_has_walls() {
        let c = cad_kernel::Circle { center: Vec2::new(0.0, 0.0), radius: 3.0 };
        assert!(!extrude(&doc_with(cad_kernel::Geom::Circle(c)), 2.7).is_empty());
    }

    #[test]
    fn an_elliptical_room_has_walls() {
        let e = cad_kernel::Ellipse {
            center: Vec2::new(0.0, 0.0),
            major: Vec2::new(4.0, 0.0),
            ratio: 0.6,
        };
        assert!(
            !extrude(&doc_with(cad_kernel::Geom::Ellipse(e)), 2.7).is_empty(),
            "an ellipse produced no wall surfaces — the room would be lit as if open to the sky",
        );
    }

    #[test]
    fn an_elliptical_arc_wall_is_extruded() {
        let e = cad_kernel::Ellipse {
            center: Vec2::new(0.0, 0.0),
            major: Vec2::new(4.0, 0.0),
            ratio: 0.6,
        };
        let ea = cad_kernel::EllipseArc { ellipse: e, start_param: 0.0, sweep_param: 1.2 };
        assert!(!extrude(&doc_with(cad_kernel::Geom::EllipseArc(ea)), 2.7).is_empty());
    }

    /// The other half of the contract: annotation must NOT become fabric. A drawing full of
    /// dimension strings should not light like a maze.
    #[test]
    fn annotation_is_not_building_fabric() {
        let t = cad_kernel::Text::empty();
        assert!(
            extrude(&doc_with(cad_kernel::Geom::Text(t)), 2.7).is_empty(),
            "text was extruded into walls",
        );
    }

    // ── A CURVED WALL DRAFTED WITH A SPLINE ────────────────────────────────────────────────
    //
    // The arm above used to name Spline as "a genuine gap, not a decision" — the tessellator
    // existed, the arm was simply unwritten, and a room traced with a spline lit as OPEN AIR.
    // M5 then made splines visible in 3D, so the viewport showed walls the calculation could not
    // see: the worst version of this defect, because the picture agrees with you.

    /// Control points that trace a closed quadrilateral room, the first point repeated at the end
    /// so the clamped curve comes back to where it started.
    ///
    /// NOT repeated EXACTLY. The end is offset by a ten-thousandth of the room — 0.04 mm on a 4 m
    /// room — because that is what a closed spline looks like after a round trip through a DXF
    /// written by another package, and a fixture that closes to the last bit would let an absolute
    /// tolerance, or no tolerance at all, pass this file's unit tests for the wrong reason.
    fn closed_spline(side: f64) -> cad_kernel::Spline {
        let nudge = side * 1e-5;
        let pts = [
            (0.0, 0.0), (side, 0.0), (side, side), (0.0, side), (nudge, nudge),
        ];
        let ctrl: Vec<Vec2> = pts.iter().map(|&(x, y)| Vec2::new(x, y)).collect();
        let w = vec![1.0; ctrl.len()];
        cad_kernel::Spline::new(2, ctrl, w)
    }

    fn open_spline() -> cad_kernel::Spline {
        let ctrl = vec![
            Vec2::new(0.0, 0.0), Vec2::new(2.0, 3.0), Vec2::new(5.0, 1.0), Vec2::new(8.0, 4.0),
        ];
        cad_kernel::Spline::new(3, ctrl, vec![1.0; 4])
    }

    /// THE GAP ITSELF. A spline produced no surfaces at all, so the room had no walls.
    #[test]
    fn a_spline_traced_room_has_walls() {
        let m = extrude(&doc_with(cad_kernel::Geom::Spline(closed_spline(4.0))), 2.7);
        assert!(
            !m.is_empty(),
            "a spline produced no wall surfaces — the room would be lit as if open to the sky",
        );
    }

    /// AND IT IS A ROOM, not a fence. A closed spline must get its floor and ceiling, or the
    /// light escapes upward and the working-plane figure comes out low with nothing to show for
    /// it. This is the assertion the bare `!is_empty()` above cannot make.
    #[test]
    fn a_closed_spline_room_is_capped() {
        let m = extrude(&doc_with(cad_kernel::Geom::Spline(closed_spline(4.0))), 2.7);
        assert!(m.iter().any(|x| x.material == FLOOR), "no floor");
        assert!(m.iter().any(|x| x.material == CEILING), "no ceiling");
        assert!(m.iter().any(|x| x.material == WALL), "no walls");
    }

    /// AN OPEN SPLINE IS A WALL, NOT A ROOM. Capping it would seal a curve the drafter left open
    /// — a lid over an area they meant to leave connected to the space next door.
    #[test]
    fn an_open_spline_is_a_wall_and_is_not_capped() {
        let m = extrude(&doc_with(cad_kernel::Geom::Spline(open_spline())), 2.7);
        assert!(!m.is_empty(), "an open spline wall vanished");
        assert!(
            !m.iter().any(|x| x.material == FLOOR || x.material == CEILING),
            "an open spline was capped — it sealed a space the drafter left open",
        );
    }

    /// THE CALCULATION PLANE COVERS IT. `bbox` sizes the grid the lux figures are sampled on, and
    /// it matched the same five types the extruder did. A spline that builds walls but is missing
    /// here gets a plane that does not reach them: the room is modelled and then only partly
    /// measured, which is the same wrong answer arriving by a different route.
    #[test]
    fn the_calc_plane_reaches_a_spline_room() {
        let (mnx, mny, mxx, mxy) =
            bbox(&doc_with(cad_kernel::Geom::Spline(closed_spline(4.0)))).expect("a room has a bbox");
        assert!(
            mnx.abs() < 0.05 && mny.abs() < 0.05 && (mxx - 4.0).abs() < 0.05 && (mxy - 4.0).abs() < 0.05,
            "the calc plane came out ({mnx}, {mny})..({mxx}, {mxy}) for a 4 m spline room",
        );
    }

    /// AND IT SCALES. The closure test is a distance between two points, so it has a tolerance,
    /// and a tolerance in a crate fed both metres and millimetres is where a unit bug hides: an
    /// absolute epsilon generous at 1 m/unit is 1000x too strict at 0.001. The same room drafted
    /// either way must close either way.
    #[test]
    fn a_spline_room_closes_whatever_unit_it_was_drafted_in() {
        for (unit_m, side) in [(1.0, 4.0), (0.001, 4000.0), (0.01, 400.0)] {
            let mut d = Document::default();
            d.units = cad_kernel::Units::from_metres_per_unit(unit_m, cad_kernel::UnitSource::Declared);
            d.push(DObject::new(cad_kernel::Geom::Spline(closed_spline(side))));
            let m = extrude(&d, 2.7);
            assert!(
                m.iter().any(|x| x.material == FLOOR),
                "a spline room drafted at {unit_m} m/unit did not close",
            );
            let max_x = m.iter().flat_map(|x| x.vertices.iter()).fold(f32::MIN, |a, v| a.max(v.x));
            assert!(
                (max_x - 4.0).abs() < 1e-2,
                "a 4 m spline room at {unit_m} m/unit reached the engine {max_x} m across",
            );
        }
    }
}
