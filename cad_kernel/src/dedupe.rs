//! OVERKILL — duplicate-entity removal.
//!
//! Removes dobjects that coincide within `tol` (drawing units): identical
//! lines (either endpoint order), circles/arcs/ellipses/points/polylines
//! (same defining geometry), and same-position text. Keeps the FIRST
//! occurrence (its handle + style win); style differences are ignored —
//! duplicate GEOMETRY is what OVERKILL hunts.

use crate::geom::{Geom, Polyline};
use crate::math::Vec2;

/// True when two geometries coincide within `tol` (defensive: any mismatch
/// in the defining data → not duplicates).
pub fn geoms_equal(a: &Geom, b: &Geom, tol: f64) -> bool {
    use Geom::*;
    let close = |x: f64, y: f64| (x - y).abs() <= tol;
    let pt = |p: Vec2, q: Vec2| (p - q).len() <= tol;
    match (a, b) {
        (Line(l1), Line(l2)) => {
            (pt(l1.a, l2.a) && pt(l1.b, l2.b))
                || (pt(l1.a, l2.b) && pt(l1.b, l2.a))
        }
        (Circle(c1), Circle(c2)) => pt(c1.center, c2.center) && close(c1.radius, c2.radius),
        (Arc(a1), Arc(a2)) => {
            pt(a1.center, a2.center)
                && close(a1.radius, a2.radius)
                && close(a1.start_angle, a2.start_angle)
                && close(a1.sweep_angle, a2.sweep_angle)
        }
        (Ellipse(e1), Ellipse(e2)) => {
            pt(e1.center, e2.center)
                && pt(e1.major, e2.major)
                && close(e1.ratio, e2.ratio)
        }
        (EllipseArc(x1), EllipseArc(x2)) => {
            pt(x1.ellipse.center, x2.ellipse.center)
                && pt(x1.ellipse.major, x2.ellipse.major)
                && close(x1.ellipse.ratio, x2.ellipse.ratio)
                && close(x1.start_param, x2.start_param)
                && close(x1.sweep_param, x2.sweep_param)
        }
        (Point(p1), Point(p2)) => pt(p1.location, p2.location),
        (Polyline(p1), Polyline(p2)) => polylines_equal(p1, p2, tol),
        (Text(t1), Text(t2)) => {
            pt(t1.position, t2.position)
                && t1.text == t2.text
                && close(t1.height, t2.height)
        }
        // Wall — same centerline + thickness.
        (Wall(w1), Wall(w2)) => {
            pt(w1.start, w2.start) && pt(w1.end, w2.end)
                && close(w1.thickness, w2.thickness)
                && close(w1.bulge, w2.bulge)
        }
        (Xline(x1), Xline(x2)) => {
            pt(x1.base, x2.base)
                && (pt(x1.dir, x2.dir) || pt(x1.dir, -x2.dir))
        }
        _ => false,
    }
}

/// Polyline equivalence: same vertex count, positions, bulges (in order; for
/// closed polylines also the reversed winding), same closed flag.
fn polylines_equal(a: &Polyline, b: &Polyline, tol: f64) -> bool {
    if a.closed != b.closed || a.vertices.len() != b.vertices.len() {
        return false;
    }
    let n = a.vertices.len();
    if n == 0 { return true; }
    let chain = |p: &Polyline, start: usize, rev: bool| -> bool {
        let mut ok = true;
        for i in 0..n {
            let j = if rev { n - 1 - i } else { i };
            let vi = p.vertices[(start + j) % n];
            let vj = b.vertices[i];
            if (vi.pos - vj.pos).len() > tol || (vi.bulge - vj.bulge).abs() > tol {
                ok = false;
                break;
            }
        }
        ok
    };
    for start in 0..n {
        if chain(a, start, false) { return true; }
        if chain(a, start, true) { return true; }
    }
    false
}

/// Remove duplicates from `dobjs` in place (keeping the FIRST occurrence).
/// Returns the original indices that were dropped.
pub fn dedupe(dobjs: &mut Vec<crate::dobject::DObject>, tol: f64)
    -> Vec<usize>
{
    let n = dobjs.len();
    // Mark-then-compact: indices stay stable while marking (a live-compaction
    // loop shifts indices and breaks the "duplicate of an earlier keeper"
    // test).
    let mut drop = vec![false; n];
    for i in 0..n {
        if drop[i] { continue; }
        for j in i + 1..n {
            if !drop[j] && geoms_equal(&dobjs[i].geom, &dobjs[j].geom, tol) {
                drop[j] = true;
            }
        }
    }
    let removed: Vec<usize> = (0..n).filter(|&i| drop[i]).collect();
    let mut k = 0;
    dobjs.retain(|_| {
        let keep = !drop[k];
        k += 1;
        keep
    });
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dobject::DObject;
    use crate::geom::{Circle, Line, PolyVertex};

    fn doc() -> Vec<DObject> {
        vec![
            DObject::new(Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) })),
            DObject::new(Geom::Line(Line { a: Vec2::new(10.0, 0.0), b: Vec2::new(0.0, 0.0) })),
            DObject::new(Geom::Circle(Circle { center: Vec2::new(5.0, 5.0), radius: 2.0 })),
            DObject::new(Geom::Circle(Circle { center: Vec2::new(5.0, 5.0), radius: 2.0 })),
            DObject::new(Geom::Circle(Circle { center: Vec2::new(5.0, 5.0), radius: 2.5 })),
        ]
    }

    #[test]
    fn removes_exact_and_reversed_duplicates() {
        let mut d = doc();
        let removed = dedupe(&mut d, 1e-9);
        assert_eq!(removed.len(), 2);
        assert_eq!(d.len(), 3);
        // Keeper is the FIRST line (a→b), not the reversed one.
        if let Geom::Line(l) = &d[0].geom {
            assert_eq!((l.a.x, l.b.x), (0.0, 10.0));
        } else { panic!(); }
    }

    #[test]
    fn near_duplicates_within_tolerance() {
        let mut d = vec![
            DObject::new(Geom::Line(Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) })),
            DObject::new(Geom::Line(Line { a: Vec2::new(0.001, 0.0), b: Vec2::new(10.0, 0.0) })),
        ];
        assert_eq!(dedupe(&mut d, 0.01).len(), 1);
    }

    #[test]
    fn closed_polyline_reversed_winding_is_duplicate() {
        let pl = |order: [Vec2; 4]| Geom::Polyline(Polyline {
            vertices: order.iter().map(|p| PolyVertex { pos: *p, bulge: 0.0 }).collect(),
            closed: true,
            widths: Vec::new(),
        });
        let mut d = vec![
            DObject::new(pl([Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0),
                             Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)])),
            DObject::new(pl([Vec2::new(0.0, 1.0), Vec2::new(1.0, 1.0),
                             Vec2::new(1.0, 0.0), Vec2::new(0.0, 0.0)])),
        ];
        assert_eq!(dedupe(&mut d, 1e-9).len(), 1);
    }

    #[test]
    fn different_geometry_is_kept() {
        let mut d = doc();
        assert_eq!(dedupe(&mut d, 1e-9).len(), 2);
        // Different radius circle survives.
        assert!(d.iter().any(|x| matches!(x.geom, Geom::Circle(c) if (c.radius - 2.5).abs() < 1e-9)));
    }
}
