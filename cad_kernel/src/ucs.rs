//! UCS — user coordinate systems (AutoCAD UCS).
//!
//! A UCS is an origin + rotation (CCW) that reinterprets coordinates for
//! INPUT and DISPLAY: a typed `x,y` is understood in UCS space and converted
//! to world space; the cursor readout shows UCS coordinates. Clicking still
//! places at the clicked world point (the UCS changes how the user thinks,
//! not where the mouse lands). The World coordinate system is implicit
//! (index 0 of `Document.current_ucs`); named UCSs live in
//! `Document.ucs_list`.

use crate::math::Vec2;

/// A named user coordinate system.
#[derive(Clone, Debug, PartialEq)]
pub struct Ucs {
    pub name:     String,
    pub origin:   Vec2,
    /// Rotation in radians, CCW (the UCS x-axis direction).
    pub rotation: f64,
}

impl Ucs {
    /// World → UCS (inverse rotation about the origin, then translate).
    pub fn to_ucs(&self, p: Vec2) -> Vec2 {
        let d = p - self.origin;
        let (s, c) = self.rotation.sin_cos();
        Vec2::new(d.x * c + d.y * s, -d.x * s + d.y * c)
    }

    /// UCS → world (rotate, then translate).
    pub fn to_world(&self, p: Vec2) -> Vec2 {
        let (s, c) = self.rotation.sin_cos();
        self.origin + Vec2::new(p.x * c - p.y * s, p.x * s + p.y * c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_round_trips() {
        let u = Ucs {
            name: "R45".into(),
            origin: Vec2::new(10.0, -5.0),
            rotation: std::f64::consts::FRAC_PI_4,
        };
        let world = Vec2::new(15.0, 2.0);
        let back = u.to_world(u.to_ucs(world));
        assert!((back - world).len() < 1e-9);
    }

    #[test]
    fn origin_only_translates() {
        let u = Ucs { name: "O".into(), origin: Vec2::new(100.0, 200.0), rotation: 0.0 };
        assert!((u.to_world(Vec2::new(1.0, 2.0)) - Vec2::new(101.0, 202.0)).len() < 1e-9);
        assert!((u.to_ucs(Vec2::new(101.0, 202.0)) - Vec2::new(1.0, 2.0)).len() < 1e-9);
    }

    #[test]
    fn identity_ucs_is_world() {
        let u = Ucs { name: "W".into(), origin: Vec2::ZERO, rotation: 0.0 };
        assert!((u.to_world(Vec2::new(3.0, 4.0)) - Vec2::new(3.0, 4.0)).len() < 1e-12);
    }

    #[test]
    fn ninety_degree_rotation_swaps_axes() {
        // UCS x-axis pointing +Y: UCS (1,0) lands at world (0,1).
        let u = Ucs { name: "R90".into(), origin: Vec2::ZERO, rotation: std::f64::consts::FRAC_PI_2 };
        let w = u.to_world(Vec2::new(1.0, 0.0));
        assert!((w - Vec2::new(0.0, 1.0)).len() < 1e-9);
    }
}
