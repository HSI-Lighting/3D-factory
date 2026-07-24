// DObject — the user-facing drafting object.
//
// A Dobject is geometry + style + identity. The internal geometry enum is
// `Geom` (Line, Circle, Arc, Ellipse, EllipseArc, plus future variants);
// this struct wraps it with the common properties every entity carries
// (layer, color, linetype, lineweight, visibility) and a stable handle.
//
// API design choice: `&[DObject]` is the canonical kernel slice type for
// snap / spatial / intersect dispatch — callers can index the slice to map
// hits back to their storage. Pure-geometry math (intersect pairs, distance)
// operates on `&Geom` internally; the struct exposes `.geom` for that.

use crate::geom::{Arc, Circle, Ellipse, EllipseArc, Geom, Line, Point, Polyline};
use crate::math::Vec2;
use crate::style::Style;

pub type Handle = u64;

#[derive(Clone, Debug)]
pub struct DObject {
    pub geom:   Geom,
    pub style:  Style,
    pub handle: Handle,
}

impl DObject {
    /// New Dobject with default style and a freshly allocated handle.
    pub fn new(geom: Geom) -> Self {
        Self { geom, style: Style::default(), handle: next_handle() }
    }

    /// New Dobject with an explicit style and a freshly allocated handle.
    pub fn with_style(geom: Geom, style: Style) -> Self {
        Self { geom, style, handle: next_handle() }
    }

    /// AABB of the contained geometry. Delegated for caller convenience —
    /// the spatial index doesn't have to crack the struct on every iter.
    pub fn bbox(&self) -> (Vec2, Vec2) { self.geom.bbox() }

    /// Distance from the visible geometry to a world point. Style does NOT
    /// affect this — invisible/locked layers are an enforcement concern at
    /// the call site, not a geometry one.
    pub fn distance_to_point(&self, p: Vec2) -> f64 { self.geom.distance_to_point(p) }

    /// Return a copy of this Dobject with its geometry translated by `off`.
    /// Style and handle are preserved.
    pub fn translated(&self, off: Vec2) -> DObject {
        DObject {
            geom:   self.geom.translated(off),
            style:  self.style,
            handle: self.handle,
        }
    }

    /// Rotate around `pivot` by `angle` radians (CCW). Style + handle preserved.
    pub fn rotated(&self, pivot: Vec2, angle: f64) -> DObject {
        DObject { geom: self.geom.rotated(pivot, angle),
                  style: self.style, handle: self.handle }
    }

    /// Scale uniformly by `factor` around `pivot`. Style + handle preserved.
    pub fn scaled(&self, pivot: Vec2, factor: f64) -> DObject {
        DObject { geom: self.geom.scaled(pivot, factor),
                  style: self.style, handle: self.handle }
    }

    /// Mirror across the line through `a` and `b`. Style + handle preserved.
    pub fn mirrored(&self, a: Vec2, b: Vec2) -> DObject {
        DObject { geom: self.geom.mirrored(a, b),
                  style: self.style, handle: self.handle }
    }

    /// Flip direction (Line, Arc, EllipseArc, Polyline). Style + handle preserved.
    pub fn reversed(&self) -> DObject {
        DObject { geom: self.geom.reversed(),
                  style: self.style, handle: self.handle }
    }

    /// Offset by `dist` on the side of `side_hint`. Style preserved; new
    /// handle (offset is a duplicate, not a transform).
    pub fn offset(&self, dist: f64, side_hint: Vec2) -> Result<DObject, &'static str> {
        let g = self.geom.offset(dist, side_hint)?;
        Ok(DObject { geom: g, style: self.style, handle: next_handle() })
    }
}

// ---- ergonomic constructors ------------------------------------------------
//
// `Line { … }.into()` builds a default-styled DObject. Keeps cad_snap-style
// quick examples one line.

impl From<Geom>       for DObject { fn from(g: Geom) -> Self        { Self::new(g) } }
impl From<Line>       for DObject { fn from(l: Line) -> Self        { Self::new(Geom::Line(l)) } }
impl From<Circle>     for DObject { fn from(c: Circle) -> Self      { Self::new(Geom::Circle(c)) } }
impl From<Arc>        for DObject { fn from(a: Arc) -> Self         { Self::new(Geom::Arc(a)) } }
impl From<Ellipse>    for DObject { fn from(e: Ellipse) -> Self     { Self::new(Geom::Ellipse(e)) } }
impl From<EllipseArc> for DObject { fn from(ea: EllipseArc) -> Self { Self::new(Geom::EllipseArc(ea)) } }
impl From<Point>      for DObject { fn from(p: Point) -> Self       { Self::new(Geom::Point(p)) } }
impl From<Polyline>   for DObject { fn from(p: Polyline) -> Self    { Self::new(Geom::Polyline(p)) } }
impl From<crate::geom::Hatch> for DObject {
    fn from(h: crate::geom::Hatch) -> Self { Self::new(Geom::Hatch(h)) }
}
impl From<crate::geom::Spline> for DObject {
    fn from(s: crate::geom::Spline) -> Self { Self::new(Geom::Spline(s)) }
}

// ---- handle allocation -----------------------------------------------------
//
// Global counter is fine for now — handles are unique per process. When DXF
// import lands we'll need handle preservation (load files keep their hex
// handles), so this becomes per-Document. Today it's simple and sufficient.

use std::sync::atomic::{AtomicU64, Ordering};
static HANDLE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn next_handle() -> Handle {
    HANDLE_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec2;

    #[test]
    fn line_into_dobject() {
        let d: DObject = Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }.into();
        assert!(matches!(d.geom, Geom::Line(_)));
        assert!(d.handle > 0);
    }

    #[test]
    fn handles_are_distinct() {
        let a: DObject = Circle { center: Vec2::ZERO, radius: 1.0 }.into();
        let b: DObject = Circle { center: Vec2::ZERO, radius: 2.0 }.into();
        assert_ne!(a.handle, b.handle);
    }

    #[test]
    fn translated_preserves_style_and_handle() {
        let d: DObject = Circle { center: Vec2::ZERO, radius: 5.0 }.into();
        let h = d.handle;
        let t = d.translated(Vec2::new(10.0, 0.0));
        assert_eq!(t.handle, h);
        if let Geom::Circle(c) = t.geom {
            assert_eq!(c.center, Vec2::new(10.0, 0.0));
        } else { panic!(); }
    }
}
