//! XREF — external reference (AutoCAD XREF, v1).
//!
//! A reference to an external drawing file: `name` (display), `path` (the
//! source file), an instance transform (insert/scale/rotation), and a
//! SNAPSHOT of the file's dobjects (`cached`, resolved by the app at attach
//! time — the kernel cannot read files). The instance transform maps file
//! coordinates to world: `world = insert + R(rot)·(scale·p)`. `cached`
//! content is re-resolved by `xref reload` / on document open, so a missing
//! file degrades to a broken link (the snapshot stays) instead of vanishing.

use crate::dobject::DObject;
use crate::geom::Geom;
use crate::math::Vec2;

#[derive(Clone, Debug)]
pub struct Xref {
    pub name: String,
    pub path: String,
    pub insert: Vec2,
    pub scale: f64,
    pub rotation: f64,
    /// Snapshot of the referenced file's dobjects (file coordinates).
    pub cached: Vec<DObject>,
}

impl Xref {
    /// Map one file-space geometry into world space via the instance
    /// transform (uniform scale about the file origin, rotate, translate).
    pub fn transform_geom(&self, g: &Geom) -> Geom {
        g.scaled(Vec2::ZERO, self.scale)
            .rotated(Vec2::ZERO, self.rotation)
            .translated(self.insert)
    }

    /// Bbox of the resolved content (transformed), in world.
    pub fn bbox(&self) -> (Vec2, Vec2) {
        let mut mn = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut mx = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for d in &self.cached {
            let (a, b) = self.transform_geom(&d.geom).bbox();
            if b.x < a.x || b.y < a.y { continue; }
            mn.x = mn.x.min(a.x); mn.y = mn.y.min(a.y);
            mx.x = mx.x.max(b.x); mx.y = mx.y.max(b.y);
        }
        if !self.cached.is_empty() && mx.x < mn.x {
            (self.insert, self.insert)
        } else if self.cached.is_empty() {
            (self.insert, self.insert)
        } else {
            (mn, mx)
        }
    }

    /// Distance to the resolved content (min over transformed children).
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        let mut best = f64::INFINITY;
        for d in &self.cached {
            best = best.min(self.transform_geom(&d.geom).distance_to_point(p));
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle, Line};

    #[test]
    fn transform_maps_file_to_world() {
        let x = Xref {
            name: "r".into(),
            path: "r.rsm".into(),
            insert: Vec2::new(100.0, 0.0),
            scale: 2.0,
            rotation: std::f64::consts::FRAC_PI_2,
            cached: Vec::new(),
        };
        // File point (1,0) → scale 2 → (2,0) → rotate 90° → (0,2) → +insert.
        let g = Geom::Point(crate::geom::Point {
            location: Vec2::new(1.0, 0.0),
            style: 0,
            size: 1.0,
        });
        let w = x.transform_geom(&g);
        if let Geom::Point(p) = w {
            assert!((p.location - Vec2::new(100.0, 2.0)).len() < 1e-9);
        } else { panic!(); }
    }

    #[test]
    fn bbox_and_distance_follow_cached_content() {
        let x = Xref {
            name: "r".into(),
            path: "r.rsm".into(),
            insert: Vec2::ZERO,
            scale: 1.0,
            rotation: 0.0,
            cached: vec![
                DObject::new(Geom::Line(Line {
                    a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) })),
                DObject::new(Geom::Circle(Circle {
                    center: Vec2::new(5.0, 3.0), radius: 1.0 })),
            ],
        };
        let (mn, mx) = x.bbox();
        assert_eq!((mn.x, mn.y, mx.x, mx.y), (0.0, 0.0, 10.0, 4.0));
        assert!(x.distance_to_point(Vec2::new(5.0, 0.0)) < 1e-9);
        assert!(x.distance_to_point(Vec2::new(50.0, 50.0)) > 40.0);
        // Empty cache → degenerate bbox at the insert.
        let mut e = x.clone();
        e.cached.clear();
        let (a, b) = e.bbox();
        assert!((a - e.insert).len() < 1e-9 && (b - e.insert).len() < 1e-9);
    }
}
