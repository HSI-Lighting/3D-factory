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
        //   Spline — a genuine gap, not a decision. `Spline::tessellate` already exists; the arm
        //     is simply unwritten, and a curved wall drafted with it lights as open air today.
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
}
