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

fn vtx(p: Vec2, z: f32) -> Vertex {
    Vertex::new(p.x as f32, p.y as f32, z)
}

fn surface(a: Vec2, b: Vec2, height: f32, material: MaterialId) -> Mesh {
    Mesh {
        vertices: vec![vtx(a, 0.0), vtx(b, 0.0), vtx(b, height), vtx(a, height)],
        triangles: vec![Triangle { a: 0, b: 1, c: 2 }, Triangle { a: 0, b: 2, c: 3 }],
        material,
    }
}

fn cap(poly: &[Vec2], z: f32, material: MaterialId, out: &mut Vec<Mesh>) {
    let p2: Vec<[f32; 2]> = poly.iter().map(|v| [v.x as f32, v.y as f32]).collect();
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

fn extrude_path(pts: &[Vec2], closed: bool, height: f32, out: &mut Vec<Mesh>) {
    for w in pts.windows(2) {
        out.push(surface(w[0], w[1], height, WALL));
    }
    if closed && pts.len() >= 3 {
        out.push(surface(pts[pts.len() - 1], pts[0], height, WALL));
        cap(pts, 0.0, FLOOR, out);
        cap(pts, height, CEILING, out);
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
fn extrude_geom(geom: &Geom, height: f32, out: &mut Vec<Mesh>) {
    match geom {
        Geom::Line(l) => extrude_path(&[l.a, l.b], false, height, out),
        Geom::Wall(w) => extrude_path(&[w.start, w.end], false, height, out),
        Geom::Polyline(p) => {
            let v: Vec<Vec2> = p.vertices.iter().map(|x| x.pos).collect();
            extrude_path(&v, p.closed, height, out);
        }
        Geom::Circle(c) => extrude_path(&circle_pts(c.center, c.radius), true, height, out),
        Geom::Arc(a) => extrude_path(&arc_pts(a), false, height, out),
        _ => {}
    }
}

/// Extrude every drafted entity to surfaces (closed paths also get floor + ceiling).
pub fn extrude(doc: &Document, height: f32) -> Vec<Mesh> {
    let mut out = Vec::new();
    for d in &doc.dobjects {
        extrude_geom(&d.geom, height, &mut out);
    }
    out
}

/// Extrude ONLY the dobjects named by `handles`, at `height` — the SIMLUX
/// per-layer room build (each imported layer extrudes to its own height).
/// Handles that no longer exist in `doc` are silently skipped.
pub fn extrude_handles(doc: &Document, handles: &[u64], height: f32) -> Vec<Mesh> {
    let mut out = Vec::new();
    for &h in handles {
        if let Some(d) = doc.find_by_handle(h) {
            extrude_geom(&d.geom, height, &mut out);
        }
    }
    out
}

/// Bounding box of all drafted geometry (x-min, y-min, x-max, y-max).
pub fn bbox(doc: &Document) -> Option<(f32, f32, f32, f32)> {
    let (mut mnx, mut mny, mut mxx, mut mxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut any = false;
    let mut add = |v: Vec2| {
        any = true;
        mnx = mnx.min(v.x as f32);
        mny = mny.min(v.y as f32);
        mxx = mxx.max(v.x as f32);
        mxy = mxy.max(v.y as f32);
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
}
