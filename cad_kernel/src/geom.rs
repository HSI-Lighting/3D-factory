// Geometric primitives. Tight, Copy, no virtual dispatch.

use crate::math::{Vec2, EPS, norm_angle};
use crate::join::{bulge_arc, polyline_segments};

#[derive(Clone, Copy, Debug)]
pub struct Line { pub a: Vec2, pub b: Vec2 }

/// Architectural smart-dobject: a wall is its IMPLICIT centerline
/// (`start` → `end`) plus a perpendicular `thickness`. Renders as two
/// parallel side lines ±thickness/2 from the centerline. All editing
/// operations (translate / rotate / scale / mirror / lengthen / trim /
/// extend) modify the centerline; the two visible sides re-derive
/// automatically. This is the "smart" part — one user gesture, both
/// sides move.
///
/// Forms the foundation of the AEC-primitive family (future Window
/// and Door dobjects will reference a Wall by handle + a position
/// along its centerline).
#[derive(Clone, Copy, Debug)]
pub struct Wall {
    pub start:     Vec2,
    pub end:       Vec2,
    pub thickness: f64,
    /// WallStyle id (drywall / structural / …). 0 = STANDARD. Drives the
    /// poché fill + (optionally) thickness; see `WallStyleTable`.
    pub style:     u32,
    /// Centerline bulge (polyline/DXF convention = tan(sweep/4)): 0 = a
    /// straight segment; ≠ 0 = the centerline is a circular arc from
    /// start→end. Used for rounded wall corners — a fillet (r>0) on two
    /// straight walls spawns a curved corner wall.
    pub bulge:     f64,
}

impl Wall {
    /// True when the centerline is an arc (rounded corner wall).
    pub fn is_curved(&self) -> bool { self.bulge.abs() > 1e-9 }

    /// Tessellate the CENTERLINE into a polyline: 2 points for a straight
    /// wall, `n`+1 points along the arc for a curved one. Faces are derived
    /// by offsetting each point ±t/2 along the local normal (sign-robust).
    pub fn centerline_polyline(&self, n: usize) -> Vec<Vec2> {
        if !self.is_curved() {
            return vec![self.start, self.end];
        }
        match bulge_arc(self.start, self.end, self.bulge) {
            Some((center, r, a0, sweep)) => {
                let steps = n.max(2);
                (0..=steps).map(|i| {
                    let t = a0 + sweep * (i as f64 / steps as f64);
                    Vec2::new(center.x + r * t.cos(), center.y + r * t.sin())
                }).collect()
            }
            None => vec![self.start, self.end],
        }
    }

    /// Face polylines `(left, right)` derived from the centerline,
    /// UNJOINED (no neighbour miters — `cad_wall::solve_faces` adds those
    /// for straight walls). Straight wall → two 2-point lines (identical
    /// to `left_line`/`right_line`). Curved wall → EXACT concentric arc
    /// samples: every centerline sample is offset ±t/2 along the TRUE
    /// radial direction, so the end face points land exactly at
    /// `start/end ± perp(tangent)·t/2` and meet a tangent straight wall's
    /// face endpoints with no gap. (The old render path used
    /// finite-difference chord normals, which tilt the end normals by
    /// sweep/(2·steps) and left a zoom-visible gap ≈ (t/2)·sweep/(2·steps)
    /// at fillet joints.)
    pub fn face_polylines(&self, n: usize) -> Option<(Vec<Vec2>, Vec<Vec2>)> {
        let half = self.thickness * 0.5;
        if !self.is_curved() {
            let l = self.left_line()?;
            let r = self.right_line()?;
            return Some((vec![l.a, l.b], vec![r.a, r.b]));
        }
        let Some((center, radius, a0, sweep)) =
            bulge_arc(self.start, self.end, self.bulge)
        else {
            // Degenerate chord — fall back to the straight faces.
            let l = self.left_line()?;
            let r = self.right_line()?;
            return Some((vec![l.a, l.b], vec![r.a, r.b]));
        };
        // "Left" = CCW-perp of the travel direction. On a CCW arc
        // (sweep > 0) that perp points INWARD (−radial); on a CW arc it
        // points OUTWARD (+radial).
        let side = if sweep >= 0.0 { -1.0 } else { 1.0 };
        let steps = n.max(2);
        let mut left  = Vec::with_capacity(steps + 1);
        let mut right = Vec::with_capacity(steps + 1);
        for i in 0..=steps {
            let t = a0 + sweep * (i as f64 / steps as f64);
            let radial = Vec2::new(t.cos(), t.sin());
            let p = center + radial * radius;
            left.push(p + radial * (side * half));
            right.push(p - radial * (side * half));
        }
        Some((left, right))
    }

    /// CCW unit normal of the centerline direction. `None` if the
    /// centerline is degenerate (start ≈ end).
    pub fn normal(&self) -> Option<Vec2> {
        let d = self.end - self.start;
        let len = d.len();
        if len < EPS { return None; }
        Some((d / len).perp())
    }

    /// The "left" (CCW-side) face line of the wall.
    pub fn left_line(&self) -> Option<Line> {
        let n = self.normal()?;
        let off = n * (self.thickness * 0.5);
        Some(Line { a: self.start + off, b: self.end + off })
    }

    /// The "right" (CW-side) face line of the wall.
    pub fn right_line(&self) -> Option<Line> {
        let n = self.normal()?;
        let off = n * (self.thickness * 0.5);
        Some(Line { a: self.start - off, b: self.end - off })
    }

    /// The implicit centerline as a regular Line.
    pub fn centerline(&self) -> Line {
        Line { a: self.start, b: self.end }
    }

    /// Centerline length.
    pub fn length(&self) -> f64 {
        (self.end - self.start).len()
    }
}


#[derive(Clone, Copy, Debug)]
pub struct Circle { pub center: Vec2, pub radius: f64 }

#[derive(Clone, Copy, Debug)]
pub struct Arc {
    pub center: Vec2,
    pub radius: f64,
    pub start_angle: f64,   // radians, in [0, 2π)
    pub sweep_angle: f64,   // radians, in (0, 2π], positive = CCW from start
}

impl Arc {
    /// True if the given absolute angle lies on the arc's swept range.
    pub fn contains_angle(&self, abs_angle: f64) -> bool {
        let d = norm_angle(abs_angle - self.start_angle);
        d <= self.sweep_angle + EPS
    }

    pub fn endpoints(&self) -> (Vec2, Vec2) {
        let s = self.start_angle;
        let e = self.start_angle + self.sweep_angle;
        let p1 = self.center + Vec2::new(self.radius * s.cos(), self.radius * s.sin());
        let p2 = self.center + Vec2::new(self.radius * e.cos(), self.radius * e.sin());
        (p1, p2)
    }
}

/// Full ellipse, possibly rotated. The major-axis VECTOR stores both the
/// rotation (direction) and the semi-major length (magnitude); `ratio` is
/// the semi-minor / semi-major ratio in (0, 1]. ratio = 1 means circle.
///
/// Parametric form:
///   P(t) = center + a · cos(t) · û  +  b · sin(t) · v̂
/// where û = major̂, v̂ = û rotated 90° CCW, a = |major|, b = a·ratio.
#[derive(Clone, Copy, Debug)]
pub struct Ellipse {
    pub center: Vec2,
    pub major:  Vec2,
    pub ratio:  f64,
}

/// Partial ellipse — the elliptical analogue of `Arc`. `start_param` and
/// `sweep_param` are values of the parameter `t` (radians), NOT geometric
/// angles measured at the centre. For a circle they coincide; for a
/// stretched ellipse they don't.
#[derive(Clone, Copy, Debug)]
pub struct EllipseArc {
    pub ellipse:     Ellipse,
    pub start_param: f64,    // in [0, 2π)
    pub sweep_param: f64,    // (0, 2π], positive = CCW (in parameter space)
}

/// A 2D point primitive — AutoCAD POINT entity. Has a location and a
/// per-instance display style (PDMODE-like, 0..=99). The renderer maps
/// the integer to a glyph (cross / X / circle-with-cross / etc.).
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub location: Vec2,
    pub style:    u8,    // PDMODE — 0 = single pixel dot, 2 = +, 3 = ×, 4 = |, …
    pub size:     f32,   // PDSIZE — drawing units; 0.0 = use renderer default
}

/// A 2D polyline — AutoCAD LWPOLYLINE. Stores vertices with optional
/// per-vertex `bulge` (tan of one-quarter the arc segment's included
/// angle). Bulge = 0 means a straight segment to the next vertex; bulge
/// != 0 means an arc segment whose mid-deviation = `chord_len * bulge / 2`.
#[derive(Clone, Copy, Debug)]
pub struct PolyVertex {
    pub pos:   Vec2,
    pub bulge: f64,
}

#[derive(Clone, Debug)]
pub struct Polyline {
    pub vertices: Vec<PolyVertex>,
    pub closed:   bool,
    /// Per-segment (start_width, end_width) in drawing units, linear taper
    /// within each segment. EMPTY = no width (render as a thin stroke — current
    /// behaviour). When non-empty, length == segment count (vertices.len()-1
    /// for open, vertices.len() for closed); index i is the segment FROM
    /// vertex i.
    pub widths: Vec<(f64, f64)>,
}

impl Polyline {
    /// AABB of the polyline, INCLUDING arc-bulge extents. A bulged segment is
    /// expanded into its true Arc (via `polyline_segments`) so a segment that
    /// bows outside its chord — e.g. a major arc from a circle join — is fully
    /// covered. Without this the spatial index would cull clicks on the arc.
    pub fn bbox(&self) -> (Vec2, Vec2) {
        if self.vertices.is_empty() {
            return (Vec2::ZERO, Vec2::ZERO);
        }
        // Start from the vertices (covers the degenerate / all-straight case).
        let mut min = self.vertices[0].pos;
        let mut max = min;
        for v in &self.vertices[1..] {
            if v.pos.x < min.x { min.x = v.pos.x; }
            if v.pos.y < min.y { min.y = v.pos.y; }
            if v.pos.x > max.x { max.x = v.pos.x; }
            if v.pos.y > max.y { max.y = v.pos.y; }
        }
        // Union in each real segment's bbox so arc bulges are included.
        for seg in polyline_segments(self) {
            let (smin, smax) = seg.bbox();
            min.x = min.x.min(smin.x); min.y = min.y.min(smin.y);
            max.x = max.x.max(smax.x); max.y = max.y.max(smax.y);
        }
        let hw = self.widths.iter().flat_map(|&(a,b)| [a,b]).fold(0.0_f64, f64::max) * 0.5;
        if hw > 0.0 {
            min.x -= hw; min.y -= hw; max.x += hw; max.y += hw;
        }
        (min, max)
    }

    /// Distance from the polyline to a point, accounting for arc bulges.
    /// Each bulged segment is tested as its true Arc (via `polyline_segments`),
    /// not its straight chord — so picking works on the curved part too.
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        if self.vertices.is_empty() { return f64::INFINITY; }
        let mut best = f64::INFINITY;
        for seg in polyline_segments(self) {
            let d = seg.distance_to_point(p);
            if d < best { best = d; }
        }
        // Fallback for a 1-vertex / degenerate polyline (no segments).
        if best.is_infinite() {
            for v in &self.vertices { best = best.min(p.dist(v.pos)); }
        }
        best
    }

    /// Total length (sum of straight chords; arc bulges add the true arc
    /// length on top — TODO when bulge math lands).
    pub fn length(&self) -> f64 {
        if self.vertices.len() < 2 { return 0.0; }
        let n = self.vertices.len();
        let pairs = if self.closed { n } else { n - 1 };
        let mut sum = 0.0;
        for i in 0..pairs {
            sum += self.vertices[i].pos.dist(self.vertices[(i + 1) % n].pos);
        }
        sum
    }
}

/// Hatch pattern — the visual style applied INSIDE a hatch boundary.
/// `Solid` fills the polygon area. `Pattern { name, scale, angle_deg }`
/// references a named line-family pattern from `crate::patterns::lookup`
/// (ANSI31, BRICK, EARTH, NET, …); the renderer clips parallel lines
/// against the resolved boundary using even-odd.
///
/// `scale` and `angle_deg` are per-hatch transforms applied on top of
/// the catalog entry: spacing multiplies by `scale`, every line family's
/// angle gets `angle_deg` added. Defaults (1.0, 0.0) mean "render the
/// pattern as the catalog defines it". Negative scale, angle outside
/// [0, 360) are allowed and behave consistently.
#[derive(Clone, Debug)]
pub enum HatchPattern {
    /// Fill the entire boundary with the dobject's solid color.
    Solid,
    /// Named pattern from the built-in catalog.
    Pattern {
        /// Canonical name (`"ANSI31"`, `"BRICK"`, …) — case-insensitive
        /// lookup. Unknown names render as nothing (the hatch still
        /// exists in the doc; user can rename it later).
        name:      String,
        /// Multiplier applied to every family's spacing. 1.0 = catalog.
        scale:     f64,
        /// Degrees added to every family's angle. 0.0 = catalog.
        angle_deg: f64,
    },
}

/// An AutoCAD-style HATCH entity. Holds REFERENCES to its boundary
/// dobject(s) by handle rather than owning a copy of their vertices.
/// This matches the user's "smart hatch" requirement: moving / editing
/// a boundary dobject automatically updates the hatch fill because
/// rendering re-resolves the boundary every frame.
///
/// Multiple boundaries → multiple loops. Even-odd fill rule at render
/// time, so the second loop becomes a hole inside the first (and the
/// fourth a hole inside the third, etc.) — basic AutoCAD island support.
///
/// Geom-level methods (bbox / distance_to_point / transforms) can't
/// resolve handles on their own (no Document access from here), so
/// they return conservative defaults. Hit-test and bbox happen at the
/// app level where the Document is available.
///
/// Style of the hatch — fill color, layer, lineweight — still lives on
/// the outer DObject as for any other Geom variant.
#[derive(Clone, Debug)]
pub struct Hatch {
    /// Handles of boundary dobjects, in loop order. Outer loop first;
    /// successive loops alternate as holes via even-odd fill at render.
    /// A handle that no longer resolves is silently skipped — the
    /// hatch shrinks gracefully if the user deletes a boundary.
    pub boundary_handles: Vec<crate::dobject::Handle>,
    pub pattern:          HatchPattern,
}

impl Hatch {
    /// Conservative bbox — empty, since this geom can't resolve its
    /// own boundaries. Callers that need a real bbox (the spatial
    /// index, viewport culling) should resolve via the Document and
    /// union the boundary dobjects' bboxes there.
    pub fn bbox(&self) -> (Vec2, Vec2) {
        (Vec2::ZERO, Vec2::ZERO)
    }

    /// Geom-level distance — INFINITY, because point-in-polygon needs
    /// the resolved boundary geometry. The app-level hit-test does
    /// the actual check by resolving handles against the Document.
    pub fn distance_to_point(&self, _p: Vec2) -> f64 {
        f64::INFINITY
    }
}

/// 2D NURBS spline. Carries everything `cad_nurbs::NurbsCurve` needs to
/// reproduce the curve: degree, control points, per-control-point
/// weights. The knot vector is implicit (clamped/open uniform —
/// derived from degree + control-point count) for v1; user-supplied
/// knot vectors land when the kernel exposes more sophisticated
/// curve-fitting paths.
///
/// All weights equal to 1.0 reduces to a plain B-spline (non-rational).
/// Distinct weights enable EXACT conics — circles, ellipses, parabolas —
/// that polynomial B-splines can only approximate.
///
/// Bbox is the convex hull's bbox (i.e. the control points' bbox),
/// which is a valid superset of the curve since a NURBS lies entirely
/// inside its control polygon's convex hull. Transforms apply
/// element-wise to control points — that's the whole point of the
/// representation.
#[derive(Clone, Debug)]
pub struct Spline {
    pub degree:         usize,
    pub control_points: Vec<Vec2>,
    /// One weight per control point. Must satisfy
    /// `weights.len() == control_points.len()` and each weight > 0.
    /// Use `Spline::new_bspline` to build a non-rational (all-1)
    /// variant without constructing the weight vector by hand.
    pub weights:        Vec<f64>,
}

impl Spline {
    /// Build a rational NURBS spline. Panics if weight count mismatches
    /// control-point count or if the curve is degenerate
    /// (control_points <= degree).
    pub fn new(degree: usize, control_points: Vec<Vec2>, weights: Vec<f64>) -> Self {
        assert_eq!(weights.len(), control_points.len(),
            "Spline: weights count must match control_points count");
        assert!(control_points.len() > degree,
            "Spline: need more control points than degree");
        Self { degree, control_points, weights }
    }

    /// Non-rational B-spline (all weights = 1.0). Same constraints as
    /// `new`.
    pub fn new_bspline(degree: usize, control_points: Vec<Vec2>) -> Self {
        let n = control_points.len();
        Self::new(degree, control_points, vec![1.0; n])
    }

    /// Bounding box of the control polygon (= valid superset of the
    /// curve's bbox by the convex-hull property). Tighter bbox would
    /// need sample-based bounds; deferred until a profiler asks for it.
    pub fn bbox(&self) -> (Vec2, Vec2) {
        if self.control_points.is_empty() {
            return (Vec2::ZERO, Vec2::ZERO);
        }
        let mut min = self.control_points[0];
        let mut max = min;
        for v in &self.control_points[1..] {
            if v.x < min.x { min.x = v.x; }
            if v.y < min.y { min.y = v.y; }
            if v.x > max.x { max.x = v.x; }
            if v.y > max.y { max.y = v.y; }
        }
        (min, max)
    }

    /// Tessellate the curve to `n_samples` evenly-spaced points across
    /// its parameter domain. Convenience wrapper around the cad_nurbs
    /// rational evaluator — converts to/from the kernel's Vec2 type at
    /// the boundary.
    pub fn tessellate(&self, n_samples: usize) -> Vec<Vec2> {
        use cad_nurbs::{NurbsCurve, Vec2 as NV};
        let ctrls: Vec<NV> = self.control_points.iter()
            .map(|v| NV::new(v.x, v.y)).collect();
        let curve = NurbsCurve::new_clamped(self.degree, ctrls, self.weights.clone());
        curve.tessellate(n_samples).into_iter()
            .map(|p| Vec2::new(p.x, p.y))
            .collect()
    }

    /// Distance from the visible curve to a point. Uses a 64-sample
    /// tessellation then per-segment distance — accurate enough for
    /// pickbox hit-testing at typical zoom levels. Refine to a
    /// projection iteration when sub-pixel accuracy is needed.
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        let samples = self.tessellate(64);
        if samples.len() < 2 { return f64::INFINITY; }
        let mut best = f64::INFINITY;
        for w in samples.windows(2) {
            let a = w[0]; let b = w[1];
            let d = b - a;
            let len_sq = d.len_sq();
            let dist = if len_sq < EPS {
                p.dist(a)
            } else {
                let t = ((p - a).dot(d) / len_sq).clamp(0.0, 1.0);
                p.dist(a + d * t)
            };
            if dist < best { best = dist; }
        }
        best
    }
}

/// Pure geometry — the shape side of a `DObject`. Style / layer / handle
/// live on the outer `DObject` struct (see [`crate::dobject`]).
///
/// Future variants land here: Text, MText, BlockRef, Dim*,
/// Image, Wipeout, Viewport, Solid2D, Ray, Xline, Leader, MLeader, Tolerance,
/// Table. Each addition is a new arm + a new entry in every match below.
#[derive(Clone, Debug)]
pub enum Geom {
    Line(Line),
    Circle(Circle),
    Arc(Arc),
    Ellipse(Ellipse),
    EllipseArc(EllipseArc),
    Point(Point),
    Polyline(Polyline),
    Hatch(Hatch),
    Spline(Spline),
    /// Architectural wall — see `Wall` for the design. All transforms
    /// operate on the centerline; the visible side lines re-derive
    /// automatically.
    Wall(Wall),
    /// Single-line text. Stored as data (position + height + angle +
    /// string + alignment + style ref); rendered by the app via
    /// `egui::Painter::text`. MText and special escape codes are
    /// deferred. See `text::Text` for the data definition.
    Text(crate::text::Text),
    /// Dimension entity. The `Dim` carries a `DimKind` (Linear /
    /// Aligned / Radius / Diameter for slice 1) plus a style id. The
    /// renderer derives extension lines, dim line, arrows, and text
    /// from the def points + DimStyle every frame; the kernel stores
    /// only the inputs. See `dim::Dim` for the full data model.
    Dimension(crate::dim::Dim),
    /// Placed block instance — references `Document.blocks` by id and
    /// carries a similarity transform (insert + rotation + uniform
    /// scale). Like Hatch, it can't resolve its own contents without the
    /// Document, so kernel-level bbox/distance are placeholders and the
    /// app resolves through the block table. See `block::BlockRef`.
    BlockRef(crate::block::BlockRef),
}

impl Geom {
    /// Return a copy rotated by `angle` radians around `pivot` (CCW).
    /// Non-uniform scale about `pivot` by POSITIVE magnitudes (sx, sy). Signs
    /// (mirroring) are handled by the caller (block insert) via a reflection +
    /// rotation, so this never reflects. When sx≈sy it's a uniform scale
    /// (delegates to `scaled`, keeping circles/arcs). When they differ a
    /// circle/arc/ellipse becomes an ellipse / elliptical-arc — matching
    /// AutoCAD/LibreCAD block-insert behaviour for stretched blocks.
    pub fn scaled_xy(&self, pivot: Vec2, sx: f64, sy: f64) -> Geom {
        if (sx - sy).abs() < 1e-9 { return self.scaled(pivot, sx); }
        let sc  = |p: Vec2| Vec2::new(pivot.x + (p.x - pivot.x) * sx,
                                      pivot.y + (p.y - pivot.y) * sy);
        let scd = |v: Vec2| Vec2::new(v.x * sx, v.y * sy);   // linear part (no shift)
        // Two CONJUGATE semi-diameters u,v (point = cos t·u + sin t·v) → axis
        // form: (major vector, ratio, param phase). The image param t maps to
        // ellipse param (t − phase). sx,sy>0 ⇒ orientation preserved.
        let to_axes = |u: Vec2, v: Vec2| -> (Vec2, f64, f64) {
            let (a, b, c) = (u.dot(u), v.dot(v), u.dot(v));
            let sstar = 0.5 * (2.0 * c).atan2(a - b);
            let pp = |s: f64| u * s.cos() + v * s.sin();
            let (mut major, mut minor, mut phase) =
                (pp(sstar), pp(sstar + std::f64::consts::FRAC_PI_2), sstar);
            if minor.len() > major.len() {
                std::mem::swap(&mut major, &mut minor);
                phase += std::f64::consts::FRAC_PI_2;
            }
            let ratio = (minor.len() / major.len().max(1e-12)).clamp(1e-6, 1.0);
            (major, ratio, phase)
        };
        match self {
            Geom::Line(l) => Geom::Line(Line { a: sc(l.a), b: sc(l.b) }),
            Geom::Point(pt) => Geom::Point(Point { location: sc(pt.location),
                style: pt.style, size: pt.size }),
            Geom::Polyline(p) => {
                // Width is a perpendicular thickness with no single axis under
                // anisotropic scale → use the average of the two factors.
                let wf = 0.5 * (sx.abs() + sy.abs());
                Geom::Polyline(Polyline {
                    vertices: p.vertices.iter()
                        .map(|v| PolyVertex { pos: sc(v.pos), bulge: v.bulge }).collect(),
                    closed: p.closed,
                    widths: p.widths.iter().map(|&(a, b)| (a * wf, b * wf)).collect() })
            }
            Geom::Spline(s) => Geom::Spline(Spline { degree: s.degree,
                control_points: s.control_points.iter().map(|p| sc(*p)).collect(),
                weights: s.weights.clone() }),
            Geom::Circle(c) => {
                let (major, ratio, _) = to_axes(
                    Vec2::new(sx * c.radius, 0.0), Vec2::new(0.0, sy * c.radius));
                Geom::Ellipse(Ellipse { center: sc(c.center), major, ratio })
            }
            Geom::Arc(arc) => {
                let (major, ratio, phase) = to_axes(
                    Vec2::new(sx * arc.radius, 0.0), Vec2::new(0.0, sy * arc.radius));
                Geom::EllipseArc(EllipseArc {
                    ellipse: Ellipse { center: sc(arc.center), major, ratio },
                    start_param: arc.start_angle - phase, sweep_param: arc.sweep_angle })
            }
            Geom::Ellipse(e) => {
                let (major, ratio, _) = to_axes(scd(e.major), scd(e.major.perp() * e.ratio));
                Geom::Ellipse(Ellipse { center: sc(e.center), major, ratio })
            }
            Geom::EllipseArc(ea) => {
                let (major, ratio, phase) =
                    to_axes(scd(ea.ellipse.major), scd(ea.ellipse.major.perp() * ea.ellipse.ratio));
                Geom::EllipseArc(EllipseArc {
                    ellipse: Ellipse { center: sc(ea.ellipse.center), major, ratio },
                    start_param: ea.start_param - phase, sweep_param: ea.sweep_param })
            }
            Geom::Dimension(d) => Geom::Dimension(d.with_points_mapped(sc)),
            Geom::Hatch(h) => Geom::Hatch(h.clone()),
            // Best-effort for the rare cases — non-uniform on these is approximate.
            Geom::Wall(w) => Geom::Wall(Wall { start: sc(w.start), end: sc(w.end),
                thickness: w.thickness * 0.5 * (sx + sy), style: w.style, bulge: w.bulge }),
            Geom::Text(t) => { let mut nt = t.clone(); nt.position = sc(t.position);
                nt.height *= sy; Geom::Text(nt) }
            Geom::BlockRef(br) => Geom::BlockRef(crate::block::BlockRef {
                insert: sc(br.insert), scale: br.scale * sx, scale_y: br.scale_y * sy, ..*br }),
        }
    }

    pub fn rotated(&self, pivot: Vec2, angle: f64) -> Geom {
        let c = angle.cos();
        let s = angle.sin();
        let rot = |p: Vec2| -> Vec2 {
            let d = p - pivot;
            Vec2 { x: pivot.x + d.x * c - d.y * s, y: pivot.y + d.x * s + d.y * c }
        };
        let rot_dir = |v: Vec2| -> Vec2 {
            // direction vectors don't shift by pivot
            Vec2 { x: v.x * c - v.y * s, y: v.x * s + v.y * c }
        };
        match self {
            Geom::Line(l) => Geom::Line(Line { a: rot(l.a), b: rot(l.b) }),
            Geom::Circle(c) => Geom::Circle(Circle { center: rot(c.center), radius: c.radius }),
            Geom::Arc(a) => Geom::Arc(Arc {
                center: rot(a.center),
                radius: a.radius,
                start_angle: (a.start_angle + angle).rem_euclid(std::f64::consts::TAU),
                sweep_angle: a.sweep_angle,
            }),
            Geom::Ellipse(e) => Geom::Ellipse(Ellipse {
                center: rot(e.center),
                major:  rot_dir(e.major),
                ratio:  e.ratio,
            }),
            Geom::EllipseArc(ea) => Geom::EllipseArc(EllipseArc {
                ellipse: Ellipse {
                    center: rot(ea.ellipse.center),
                    major:  rot_dir(ea.ellipse.major),
                    ratio:  ea.ellipse.ratio,
                },
                // Parameter space is local to the ellipse's own frame, which
                // we rotated by `angle` (the major direction moved). The
                // start_param stays — it's measured against the (now rotated)
                // local axes.
                start_param: ea.start_param,
                sweep_param: ea.sweep_param,
            }),
            Geom::Point(pt) => Geom::Point(Point {
                location: rot(pt.location), style: pt.style, size: pt.size,
            }),
            Geom::Polyline(p) => Geom::Polyline(Polyline {
                vertices: p.vertices.iter()
                    .map(|v| PolyVertex { pos: rot(v.pos), bulge: v.bulge })
                    .collect(),
                closed: p.closed,
                widths: p.widths.clone(),
            }),
            // Hatch transforms are NO-OPS — the hatch follows its
            // boundary dobjects (which transform on their own) via the
            // handles in `boundary_handles`. To "move a hatch", select
            // its boundary too (or do that automatically at the app
            // level — TODO follow-up).
            Geom::Hatch(h) => Geom::Hatch(h.clone()),
            // Splines transform their control points — the curve
            // follows because NURBS evaluation is linear in control
            // points (Σ N_i(u) * P_i / Σ N_i(u) * w_i; rotating each
            // P_i rotates the whole curve by the same R). Weights
            // and degree are invariant.
            Geom::Spline(s) => Geom::Spline(Spline {
                degree:         s.degree,
                control_points: s.control_points.iter().map(|p| rot(*p)).collect(),
                weights:        s.weights.clone(),
            }),
            // Wall — rotate the centerline; thickness is direction-
            // invariant. Side lines re-derive from the new endpoints.
            Geom::Wall(w) => Geom::Wall(Wall {
                start:     rot(w.start),
                end:       rot(w.end),
                thickness: w.thickness,
                style:     w.style,
                bulge:     w.bulge,        // rotation preserves arc winding
            }),
            // Text — rotate the anchor + bump the text's own angle.
            Geom::Text(t) => {
                let mut nt = t.clone();
                nt.position = rot(t.position);
                nt.angle    = t.angle + angle;
                Geom::Text(nt)
            }
            // Dimension — rotate every def point. Text orientation
            // re-derives at render time from the dim-line direction.
            Geom::Dimension(d) => Geom::Dimension(d.with_points_mapped(rot)),
            // BlockRef — rotate the insertion point about the pivot and
            // add the angle to the instance rotation. Exact (similarity
            // transforms compose).
            Geom::BlockRef(br) => Geom::BlockRef(crate::block::BlockRef {
                insert:   rot(br.insert),
                rotation: br.rotation + angle,
                ..*br
            }),
        }
    }

    /// Return a copy scaled uniformly by `factor` around `pivot`.
    /// Non-uniform scale (different x/y) isn't supported because it
    /// turns circles into ellipses — a separate refactor.
    pub fn scaled(&self, pivot: Vec2, factor: f64) -> Geom {
        let sc = |p: Vec2| -> Vec2 {
            pivot + (p - pivot) * factor
        };
        let sc_dir = |v: Vec2| -> Vec2 { v * factor };
        let f_abs = factor.abs();
        match self {
            Geom::Line(l) => Geom::Line(Line { a: sc(l.a), b: sc(l.b) }),
            Geom::Circle(c) => Geom::Circle(Circle {
                center: sc(c.center), radius: c.radius * f_abs,
            }),
            Geom::Arc(a) => Geom::Arc(Arc {
                center: sc(a.center), radius: a.radius * f_abs,
                start_angle: a.start_angle, sweep_angle: a.sweep_angle,
            }),
            Geom::Ellipse(e) => Geom::Ellipse(Ellipse {
                center: sc(e.center), major: sc_dir(e.major), ratio: e.ratio,
            }),
            Geom::EllipseArc(ea) => Geom::EllipseArc(EllipseArc {
                ellipse: Ellipse {
                    center: sc(ea.ellipse.center),
                    major:  sc_dir(ea.ellipse.major),
                    ratio:  ea.ellipse.ratio,
                },
                start_param: ea.start_param, sweep_param: ea.sweep_param,
            }),
            Geom::Point(pt) => Geom::Point(Point {
                location: sc(pt.location), style: pt.style, size: pt.size * factor as f32,
            }),
            Geom::Polyline(p) => Geom::Polyline(Polyline {
                vertices: p.vertices.iter()
                    .map(|v| PolyVertex { pos: sc(v.pos), bulge: v.bulge })
                    .collect(),
                closed: p.closed,
                widths: p.widths.iter().map(|&(a,b)| (a * f_abs, b * f_abs)).collect(),
            }),
            // No-op for the same reason as `rotated`.
            Geom::Hatch(h) => Geom::Hatch(h.clone()),
            Geom::Spline(s) => Geom::Spline(Spline {
                degree:         s.degree,
                control_points: s.control_points.iter().map(|p| sc(*p)).collect(),
                weights:        s.weights.clone(),
            }),
            // Wall — scale the centerline + thickness uniformly so
            // the wall stays geometrically similar.
            Geom::Wall(w) => Geom::Wall(Wall {
                start:     sc(w.start),
                end:       sc(w.end),
                thickness: w.thickness * f_abs,
                style:     w.style,
                bulge:     w.bulge,        // tan(sweep/4) is scale-invariant
            }),
            // Text — scale anchor + height (angle invariant under
            // uniform scale).
            Geom::Text(t) => {
                let mut nt = t.clone();
                nt.position = sc(t.position);
                nt.height   = t.height * f_abs;
                Geom::Text(nt)
            }
            // Dimension — scale every def point. Renderer uses the
            // current DimStyle's text_height + arrow_size; those are
            // already in world units so they scale with the camera.
            Geom::Dimension(d) => Geom::Dimension(d.with_points_mapped(sc)),
            // BlockRef — scale the insertion point about the pivot and
            // multiply the uniform instance scale (|factor| like Wall
            // thickness; negative factors don't reflect a block in v1).
            Geom::BlockRef(br) => Geom::BlockRef(crate::block::BlockRef {
                insert: sc(br.insert),
                scale:   br.scale   * f_abs,
                scale_y: br.scale_y * f_abs,
                ..*br
            }),
        }
    }

    /// Return a copy mirrored across the line through `a` and `b`.
    pub fn mirrored(&self, a: Vec2, b: Vec2) -> Geom {
        let dir = b - a;
        let len_sq = dir.len_sq();
        if len_sq < EPS {
            return self.clone();   // degenerate axis — no-op
        }
        let mirror = |p: Vec2| -> Vec2 {
            let d = p - a;
            let t = d.dot(dir) / len_sq;
            let foot = a + dir * t;
            foot * 2.0 - p
        };
        let mirror_dir = |v: Vec2| -> Vec2 {
            // direction reflection is the same formula without the `a` shift.
            let t = v.dot(dir) / len_sq;
            let foot = dir * t;
            foot * 2.0 - v
        };
        match self {
            Geom::Line(l) => Geom::Line(Line { a: mirror(l.a), b: mirror(l.b) }),
            Geom::Circle(c) => Geom::Circle(Circle {
                center: mirror(c.center), radius: c.radius,
            }),
            Geom::Arc(arc) => {
                // Mirroring flips CCW → CW; we keep CCW convention by starting
                // from the OTHER endpoint and sweeping the same magnitude.
                let (_e1, e2) = arc.endpoints();
                let m2 = mirror(e2);
                let new_center = mirror(arc.center);
                let new_start = (m2 - new_center).angle();
                Geom::Arc(Arc {
                    center: new_center, radius: arc.radius,
                    start_angle: new_start.rem_euclid(std::f64::consts::TAU),
                    sweep_angle: arc.sweep_angle,
                })
            }
            Geom::Ellipse(e) => Geom::Ellipse(Ellipse {
                center: mirror(e.center), major: mirror_dir(e.major), ratio: e.ratio,
            }),
            Geom::EllipseArc(ea) => Geom::EllipseArc(EllipseArc {
                ellipse: Ellipse {
                    center: mirror(ea.ellipse.center),
                    major:  mirror_dir(ea.ellipse.major),
                    ratio:  ea.ellipse.ratio,
                },
                start_param: ea.start_param, sweep_param: ea.sweep_param,
            }),
            Geom::Point(pt) => Geom::Point(Point {
                location: mirror(pt.location), style: pt.style, size: pt.size,
            }),
            Geom::Polyline(p) => Geom::Polyline(Polyline {
                vertices: p.vertices.iter()
                    .map(|v| PolyVertex { pos: mirror(v.pos), bulge: v.bulge })
                    .collect(),
                closed: p.closed,
                widths: p.widths.clone(),
            }),
            // No-op for the same reason as `rotated`.
            Geom::Hatch(h) => Geom::Hatch(h.clone()),
            Geom::Spline(s) => Geom::Spline(Spline {
                degree:         s.degree,
                control_points: s.control_points.iter().map(|p| mirror(*p)).collect(),
                weights:        s.weights.clone(),
            }),
            // Wall — mirror the centerline; thickness unchanged.
            Geom::Wall(w) => Geom::Wall(Wall {
                start:     mirror(w.start),
                end:       mirror(w.end),
                thickness: w.thickness,
                style:     w.style,
                bulge:     -w.bulge,       // reflection flips arc winding
            }),
            // Text — mirror the anchor; reflect the text angle by the
            // mirror axis. Text content remains readable (would appear
            // BACKWARDS without the `TextGeneration::Backward` flag we
            // haven't modelled yet — defer to MText slice).
            Geom::Text(t) => {
                let mut nt = t.clone();
                nt.position = mirror(t.position);
                // Mirror axis angle: atan2(b-a).y, x). Reflected angle
                // = 2*axis_angle - angle.
                let axis_angle = (b - a).angle();
                nt.angle = 2.0 * axis_angle - t.angle;
                Geom::Text(nt)
            }
            // Dimension — mirror every def point. Text reads naturally
            // because the dim line direction is recomputed; no need to
            // flip the text angle separately.
            Geom::Dimension(d) => Geom::Dimension(d.with_points_mapped(mirror)),
            // BlockRef — v1 LIMITATION (documented in block.rs): the
            // insertion point and rotation are reflected, but the content
            // keeps its handedness (no `mirrored` flag yet). Symmetric
            // blocks look right; mirror asymmetric blocks by exploding
            // first. Reflected rotation = 2·axis_angle − rotation.
            Geom::BlockRef(br) => {
                let axis_angle = (b - a).angle();
                Geom::BlockRef(crate::block::BlockRef {
                    insert:   mirror(br.insert),
                    rotation: 2.0 * axis_angle - br.rotation,
                    mirror_x: !br.mirror_x,   // reflecting flips the parity
                    ..*br
                })
            }
        }
    }

    /// Extend the length by signed `delta` at the end nearer to `near`.
    /// Negative delta shortens.
    /// - Line: move whichever endpoint is closer to `near` further along
    ///   the segment's direction by `delta`.
    /// - Arc / EllipseArc: extend the start or end angle/param so the
    ///   total arc length changes by `delta`.
    /// Other variants return Err.
    pub fn lengthened(&self, delta: f64, near: Vec2) -> Result<Geom, &'static str> {
        if delta.abs() < EPS { return Ok(self.clone()); }
        match self {
            Geom::Line(l) => {
                let dir = l.b - l.a;
                let len = dir.len();
                if len < EPS { return Err("lengthen: zero-length line"); }
                let u = dir / len;
                let at_b = near.dist(l.b) < near.dist(l.a);
                if at_b {
                    Ok(Geom::Line(Line { a: l.a, b: l.b + u * delta }))
                } else {
                    Ok(Geom::Line(Line { a: l.a - u * delta, b: l.b }))
                }
            }
            Geom::Arc(a) => {
                if a.radius < EPS { return Err("lengthen: zero-radius arc"); }
                let d_angle = delta / a.radius;
                let (e1, e2) = a.endpoints();
                let at_end = near.dist(e2) < near.dist(e1);
                let new_sweep = a.sweep_angle + d_angle;
                if new_sweep <= 0.0 || new_sweep >= std::f64::consts::TAU {
                    return Err("lengthen: would close arc or invert");
                }
                if at_end {
                    Ok(Geom::Arc(Arc {
                        center: a.center, radius: a.radius,
                        start_angle: a.start_angle, sweep_angle: new_sweep,
                    }))
                } else {
                    let new_start = (a.start_angle - d_angle)
                        .rem_euclid(std::f64::consts::TAU);
                    Ok(Geom::Arc(Arc {
                        center: a.center, radius: a.radius,
                        start_angle: new_start, sweep_angle: new_sweep,
                    }))
                }
            }
            Geom::EllipseArc(ea) => {
                // For an ellipse, "arc length" varies with parameter — we
                // approximate by scaling sweep_param by (delta / current
                // chord), which is correct only for near-circular ellipses
                // but acceptable for v1 (refine when needed).
                let (e1, e2) = ea.endpoints();
                let approx_len = e1.dist(e2).max(EPS);
                let dp = ea.sweep_param * (delta / approx_len);
                let at_end = near.dist(e2) < near.dist(e1);
                let new_sweep = ea.sweep_param + dp;
                if new_sweep <= 0.0 || new_sweep >= std::f64::consts::TAU {
                    return Err("lengthen: would close ellipse-arc or invert");
                }
                if at_end {
                    Ok(Geom::EllipseArc(EllipseArc {
                        ellipse: ea.ellipse,
                        start_param: ea.start_param, sweep_param: new_sweep,
                    }))
                } else {
                    let new_start = (ea.start_param - dp)
                        .rem_euclid(std::f64::consts::TAU);
                    Ok(Geom::EllipseArc(EllipseArc {
                        ellipse: ea.ellipse,
                        start_param: new_start, sweep_param: new_sweep,
                    }))
                }
            }
            Geom::Wall(w) => {
                // Lengthen the centerline; thickness unchanged.
                let line = w.centerline();
                let g = Geom::Line(line).lengthened(delta, near)?;
                if let Geom::Line(new_line) = g {
                    Ok(Geom::Wall(Wall {
                        start: new_line.a,
                        end:   new_line.b,
                        thickness: w.thickness,
                        style: w.style,
                        bulge: 0.0,        // lengthen yields a straight wall
                    }))
                } else { Err("lengthen wall: unexpected non-Line result") }
            }
            _ => Err("lengthen: only Line / Arc / EllipseArc / Wall are supported"),
        }
    }

    /// Return an "infinite" / "extended" copy of this geometry for use as a
    /// cutting / boundary edge in trim / extend with `EdgMod` ON. The point
    /// is to make intersections fire for "imaginary" geometry that the
    /// visible curve alone would miss.
    /// - Line: extended HUGE in both directions (still a Line, just very
    ///   long — keeps the segment-based intersect math working)
    /// - Arc → full Circle of the same center+radius
    /// - EllipseArc → full Ellipse
    /// - Already-closed (Circle, Ellipse) or dimensionless (Point) → clone
    /// - Polyline: per-segment "extend" doesn't generalise; clone for now
    pub fn extended_for_edgemode(&self) -> Geom {
        const EXT: f64 = 1.0e6;     // big enough to clear any drawing
        match self {
            Geom::Line(l) => {
                let d = l.b - l.a;
                let len = d.len();
                if len < EPS { return Geom::Line(*l); }
                let u = d / len;
                Geom::Line(Line { a: l.a - u * EXT, b: l.b + u * EXT })
            }
            Geom::Arc(a) => Geom::Circle(Circle { center: a.center, radius: a.radius }),
            Geom::EllipseArc(ea) => Geom::Ellipse(ea.ellipse),
            // A Wall's "infinite form" is its centerline lengthened past the
            // drawing — same as a Line. WITHOUT this, a Wall TARGET (or an
            // edge-mode Wall boundary) fails the `hits.is_empty()` guard in
            // `extend_to` before the `Geom::Wall` arm can extend the centerline,
            // so "extend a wall to a boundary" silently no-ops.
            Geom::Wall(w) => {
                let cl = w.centerline();
                let d = cl.b - cl.a;
                let len = d.len();
                if len < EPS { return self.clone(); }
                let u = d / len;
                Geom::Line(Line { a: cl.a - u * EXT, b: cl.b + u * EXT })
            }
            other => other.clone(),
        }
    }



    /// Return a copy with direction flipped, where direction is defined.
    /// - Line: swap a/b
    /// - Arc / EllipseArc: start at the OTHER end, same sweep magnitude
    ///   (still CCW in our convention — the swept curve is identical, just
    ///   parameterised from the opposite endpoint)
    /// - Polyline: reverse vertex order, flip every per-vertex bulge sign
    ///   (bulge is signed by direction)
    /// - Circle / Ellipse / Point: no observable direction; returns a clone
    pub fn reversed(&self) -> Geom {
        match self {
            Geom::Line(l) => Geom::Line(Line { a: l.b, b: l.a }),
            // An Arc stores only a POSITIVE CCW sweep, so it cannot encode a
            // traversal direction — the reversed arc occupies the IDENTICAL
            // set of points. Return it unchanged. (The old code set
            // start = start+sweep while KEEPING the positive sweep, which is a
            // DIFFERENT arc, P(start+2·sweep) — it relocated the arc and broke
            // both the `reverse` command and polyline joining.)
            Geom::Arc(a) => Geom::Arc(*a),
            Geom::EllipseArc(ea) => Geom::EllipseArc(*ea),
            Geom::Polyline(p) => {
                // Bulge belongs to the segment FROM this vertex to the next.
                // After reversing the vertex order, new segment `i` (verts
                // [i] → [i+1]) corresponds to the OLD segment between
                // (n-1-i) and (n-2-i) traversed in reverse — so its bulge
                // is the OLD segment's bulge with sign flipped.
                let n = p.vertices.len();
                let mut new_verts: Vec<PolyVertex> = p.vertices.iter().rev()
                    .map(|v| PolyVertex { pos: v.pos, bulge: 0.0 })
                    .collect();
                for i in 0..n {
                    let old_seg = if p.closed {
                        // closed has n segments; new segment i = reverse of
                        // old segment (n-1-i) % n. bulge[k] = segment k→k+1.
                        (n - 1 - i) % n
                    } else {
                        // open has n-1 segments; vertex (n-1)'s bulge slot
                        // is unused, so skip when i+1 >= n.
                        if i + 1 >= n { continue; }
                        n - 2 - i
                    };
                    new_verts[i].bulge = -p.vertices[old_seg].bulge;
                }
                Geom::Polyline(Polyline {
                    vertices: new_verts,
                    closed: p.closed,
                    widths: { let mut w: Vec<(f64,f64)> = p.widths.iter().rev().map(|&(s,e)| (e,s)).collect(); w },
                })
            }
            // Direction-agnostic — return a deep copy.
            // Spline: reversing a NURBS curve = reverse control points
            //         + reverse weights + reverse knot vector. Since
            //         we use an implicit clamped/open uniform knot
            //         vector that's symmetric, reversing control
            //         points + weights is enough.
            Geom::Spline(s) => Geom::Spline(Spline {
                degree:         s.degree,
                control_points: s.control_points.iter().rev().copied().collect(),
                weights:        s.weights.iter().rev().copied().collect(),
            }),
            Geom::Circle(_) | Geom::Ellipse(_) | Geom::Point(_) | Geom::Hatch(_) => self.clone(),
            // Wall — reverse the centerline; thickness unchanged. The
            // visible side-line naming (left/right) swaps because the
            // CCW normal flips, but the geometry is identical.
            Geom::Wall(w) => Geom::Wall(Wall {
                start: w.end, end: w.start, thickness: w.thickness,
                style: w.style, bulge: -w.bulge,   // reversing flips winding
            }),
            // Text — reversal has no geometric meaning. Clone.
            Geom::Text(t) => Geom::Text(t.clone()),
            // Dimension — direction-agnostic; clone.
            Geom::Dimension(d) => Geom::Dimension(d.clone()),
            // BlockRef — direction-agnostic; clone.
            Geom::BlockRef(br) => Geom::BlockRef(*br),
        }
    }

    /// Return a copy of this geometry translated by `off`.
    pub fn translated(&self, off: Vec2) -> Geom {
        match self {
            Geom::Line(l) => Geom::Line(Line {
                a: l.a + off, b: l.b + off,
            }),
            Geom::Circle(c) => Geom::Circle(Circle {
                center: c.center + off, radius: c.radius,
            }),
            Geom::Arc(a) => Geom::Arc(Arc {
                center: a.center + off,
                radius: a.radius,
                start_angle: a.start_angle,
                sweep_angle: a.sweep_angle,
            }),
            Geom::Ellipse(e) => Geom::Ellipse(Ellipse {
                center: e.center + off,
                major:  e.major,
                ratio:  e.ratio,
            }),
            Geom::EllipseArc(ea) => Geom::EllipseArc(EllipseArc {
                ellipse:     Ellipse {
                    center: ea.ellipse.center + off,
                    major:  ea.ellipse.major,
                    ratio:  ea.ellipse.ratio,
                },
                start_param: ea.start_param,
                sweep_param: ea.sweep_param,
            }),
            Geom::Point(pt) => Geom::Point(Point {
                location: pt.location + off,
                style:    pt.style,
                size:     pt.size,
            }),
            Geom::Polyline(p) => Geom::Polyline(Polyline {
                vertices: p.vertices.iter()
                    .map(|v| PolyVertex { pos: v.pos + off, bulge: v.bulge })
                    .collect(),
                closed:   p.closed,
                widths:   p.widths.clone(),
            }),
            // No-op for the same reason as `rotated`.
            Geom::Hatch(h) => Geom::Hatch(h.clone()),
            Geom::Spline(s) => Geom::Spline(Spline {
                degree:         s.degree,
                control_points: s.control_points.iter().map(|p| *p + off).collect(),
                weights:        s.weights.clone(),
            }),
            Geom::Wall(w) => Geom::Wall(Wall {
                start: w.start + off, end: w.end + off, thickness: w.thickness,
                style: w.style, bulge: w.bulge,
            }),
            Geom::Text(t) => {
                let mut nt = t.clone();
                nt.position = t.position + off;
                Geom::Text(nt)
            }
            Geom::Dimension(d) => Geom::Dimension(d.with_points_mapped(|p| p + off)),
            Geom::BlockRef(br) => Geom::BlockRef(crate::block::BlockRef {
                insert: br.insert + off,
                ..*br
            }),
        }
    }

    /// Minimum distance from the dobject (its visible curve) to a point.
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        match self {
            Geom::Line(l)        => l.distance_to_point(p),
            Geom::Circle(c)      => c.distance_to_point(p),
            Geom::Arc(a)         => a.distance_to_point(p),
            Geom::Ellipse(e)     => e.distance_to_point(p),
            Geom::EllipseArc(ea) => ea.distance_to_point(p),
            Geom::Point(pt)      => pt.location.dist(p),
            Geom::Polyline(pl)   => pl.distance_to_point(p),
            Geom::Hatch(h)       => h.distance_to_point(p),
            Geom::Spline(s)      => s.distance_to_point(p),
            // Wall — min distance to either visible side line. The
            // centerline ITSELF is invisible (a debug overlay) so it
            // doesn't participate in pick-test.
            Geom::Wall(w) => {
                let l = w.left_line();
                let r = w.right_line();
                match (l, r) {
                    (Some(l), Some(r)) =>
                        l.distance_to_point(p).min(r.distance_to_point(p)),
                    _ => f64::INFINITY,
                }
            }
            // Text — distance to the anchor point. Good enough for
            // click-pick; refine to bbox-distance when text starts
            // occupying significant screen area.
            Geom::Text(t) => t.position.dist(p),
            // Dimension — distance to the VISIBLE outline (extension lines +
            // dim line, or leader for radius/diameter) so the user can click
            // ON the line, plus the def/grip points (text anchor + def pts)
            // as a fallback.
            Geom::Dimension(d) => {
                let mut best = f64::INFINITY;
                for (a, b) in d.outline_segments() {
                    let dist = Line { a, b }.distance_to_point(p);
                    if dist < best { best = dist; }
                }
                for gp in d.grip_points() {
                    let dist = gp.dist(p);
                    if dist < best { best = dist; }
                }
                best
            }
            // BlockRef — contents live in Document.blocks, which the
            // kernel can't reach from here (same situation as Hatch).
            // INFINITY keeps the generic pick loop from matching; the
            // app resolves block contents in its pick fallback.
            Geom::BlockRef(_) => f64::INFINITY,
        }
    }
}

impl Line {
    /// Distance from this segment to a point (perpendicular if the foot is on
    /// the segment, otherwise the nearer endpoint).
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        let d = self.b - self.a;
        let len_sq = d.len_sq();
        if len_sq < EPS { return p.dist(self.a); }
        let t = ((p - self.a).dot(d) / len_sq).clamp(0.0, 1.0);
        let foot = self.a + d * t;
        p.dist(foot)
    }
}

impl Circle {
    /// Distance from this circle's curve to a point (always positive).
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        (p.dist(self.center) - self.radius).abs()
    }
}

impl Arc {
    /// Distance from this arc's visible curve to a point. If the point's angle
    /// from the centre is within the swept range, this is the radial distance;
    /// otherwise it's the distance to the nearer endpoint.
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        let v = p - self.center;
        let ang = v.angle();
        if self.contains_angle(ang) {
            (v.len() - self.radius).abs()
        } else {
            let (e1, e2) = self.endpoints();
            p.dist(e1).min(p.dist(e2))
        }
    }
}

impl Geom {
    /// Axis-aligned bounding box (min, max). For arcs / elliptical arcs this
    /// is the conservative bbox of the full underlying curve, not the tight
    /// per-quadrant one — good enough for viewport culling and fast to compute.
    pub fn bbox(&self) -> (Vec2, Vec2) {
        match self {
            Geom::Line(l) => (
                Vec2::new(l.a.x.min(l.b.x), l.a.y.min(l.b.y)),
                Vec2::new(l.a.x.max(l.b.x), l.a.y.max(l.b.y)),
            ),
            Geom::Circle(c) => (
                Vec2::new(c.center.x - c.radius, c.center.y - c.radius),
                Vec2::new(c.center.x + c.radius, c.center.y + c.radius),
            ),
            // TIGHT arc bbox: the two endpoints, plus any cardinal extreme
            // (+x/+y/-x/-y) the arc actually sweeps through. The old
            // full-circle bbox broke window selection — an arc visually
            // inside a window was rejected because its parent circle poked
            // out of the box.
            Geom::Arc(a) => {
                let (e1, e2) = a.endpoints();
                let mut min = Vec2::new(e1.x.min(e2.x), e1.y.min(e2.y));
                let mut max = Vec2::new(e1.x.max(e2.x), e1.y.max(e2.y));
                for k in 0..4 {
                    let ang = k as f64 * std::f64::consts::FRAC_PI_2;
                    let rel = (ang - a.start_angle).rem_euclid(std::f64::consts::TAU);
                    if rel <= a.sweep_angle + 1e-12 {
                        let p = Vec2::new(
                            a.center.x + a.radius * ang.cos(),
                            a.center.y + a.radius * ang.sin());
                        min.x = min.x.min(p.x); min.y = min.y.min(p.y);
                        max.x = max.x.max(p.x); max.y = max.y.max(p.y);
                    }
                }
                (min, max)
            }
            Geom::Ellipse(e)     => e.bbox(),
            // TIGHT elliptical-arc bbox by sampling — the full-ellipse bbox
            // had the same window-selection bug as Arc.
            Geom::EllipseArc(ea) => {
                let n = 48;
                let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
                let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
                for i in 0..=n {
                    let t = ea.start_param + ea.sweep_param * (i as f64 / n as f64);
                    let p = ea.ellipse.point_at(t);
                    min.x = min.x.min(p.x); min.y = min.y.min(p.y);
                    max.x = max.x.max(p.x); max.y = max.y.max(p.y);
                }
                (min, max)
            }
            Geom::Point(pt) => (pt.location, pt.location),
            Geom::Polyline(pl) => pl.bbox(),
            Geom::Hatch(h)  => h.bbox(),
            Geom::Spline(s) => s.bbox(),
            // Wall bbox = centerline bbox EXPANDED by thickness/2 in
            // both axes (loose but cheap; the rotated side-line corners
            // are always within this box).
            Geom::Wall(w) => {
                let h = w.thickness * 0.5;
                let min = Vec2::new(
                    w.start.x.min(w.end.x) - h,
                    w.start.y.min(w.end.y) - h,
                );
                let max = Vec2::new(
                    w.start.x.max(w.end.x) + h,
                    w.start.y.max(w.end.y) + h,
                );
                (min, max)
            }
            // Text — unrotated bbox (loose; ignores `angle`). Same
            // approximation Wall uses for its rotated side-line corners.
            Geom::Text(t) => t.bbox_unrotated(),
            // Dimension — bbox of the def points. The renderer's text
            // and extension lines can fall slightly outside this; v1
            // accepts the loose bbox.
            Geom::Dimension(d) => d.bbox(),
            // BlockRef — placeholder (can't resolve contents without the
            // Document). `is_view_independent_bbox()` returns true so
            // spatial indexes never cull on this; the app computes the
            // real resolved bbox where it matters (window selection).
            Geom::BlockRef(br) => (br.insert, br.insert),
        }
    }

    /// True iff this dobject's bbox is NOT a reliable view-culling key.
    ///
    /// Hatches reference their boundary by handle, so the kernel can't
    /// compute the bbox without the Document. Bbox returns (0, 0) as a
    /// placeholder — which means a spatial-index bbox-query may filter
    /// the hatch out unless the camera happens to include the origin.
    ///
    /// Spatial indexes (e.g. `UniformGrid`) MUST treat
    /// view-independent dobjects as ALWAYS in the candidate set, so
    /// the resolved-by-handle render path still gets a chance to draw
    /// them. App-level renderers MUST short-circuit them BEFORE
    /// viewport-bbox culls.
    ///
    /// Future variants (Region, BlockRef, Xref) that store references
    /// rather than vertices will override this the same way Hatch
    /// does today.
    pub fn is_view_independent_bbox(&self) -> bool {
        // Hatch and BlockRef both resolve their real extent through the
        // Document (boundary handles / block table), so their kernel
        // bboxes are placeholders and spatial indexes must keep them in
        // every candidate set.
        matches!(self, Geom::Hatch(_) | Geom::BlockRef(_))
    }
}

// ---- Ellipse / EllipseArc geometry ----------------------------------------

impl Ellipse {
    /// Semi-major axis length, `a`.
    pub fn semi_major(&self) -> f64 { self.major.len() }
    /// Semi-minor axis length, `b = a · ratio`.
    pub fn semi_minor(&self) -> f64 { self.semi_major() * self.ratio }

    /// Unit vector along the major axis (the "u" direction).
    pub fn u_hat(&self) -> Vec2 { self.major.normalized() }
    /// Unit vector along the minor axis (u rotated 90° CCW).
    pub fn v_hat(&self) -> Vec2 { self.u_hat().perp() }

    /// Point on the ellipse curve at parameter t (radians).
    /// P(t) = center + a·cos(t)·û + b·sin(t)·v̂
    pub fn point_at(&self, t: f64) -> Vec2 {
        let a = self.semi_major();
        let b = self.semi_minor();
        self.center + self.u_hat() * (a * t.cos()) + self.v_hat() * (b * t.sin())
    }

    /// Tangent vector (un-normalized) at parameter t. dP/dt.
    pub fn tangent_at(&self, t: f64) -> Vec2 {
        let a = self.semi_major();
        let b = self.semi_minor();
        self.u_hat() * (-a * t.sin()) + self.v_hat() * (b * t.cos())
    }

    /// Axis-aligned bbox of the FULL ellipse (regardless of any swept range).
    /// Derived from the rotated parametric form:
    ///   x_half = sqrt(a²·cos²θ + b²·sin²θ)
    ///   y_half = sqrt(a²·sin²θ + b²·cos²θ)
    /// In our representation (major = a · û, ratio = b/a) this simplifies to:
    ///   x_half = sqrt(major.x² + (ratio · major.y)²)
    ///   y_half = sqrt(major.y² + (ratio · major.x)²)
    pub fn bbox(&self) -> (Vec2, Vec2) {
        let mx = self.major.x;
        let my = self.major.y;
        let r2 = self.ratio * self.ratio;
        let hx = (mx * mx + r2 * my * my).sqrt();
        let hy = (my * my + r2 * mx * mx).sqrt();
        (Vec2::new(self.center.x - hx, self.center.y - hy),
         Vec2::new(self.center.x + hx, self.center.y + hy))
    }

    /// Closest parameter t to a world point `p`, found by Newton iteration on
    /// `f(t) = (P(t) - p) · P'(t) = 0`. Closed-form solving requires a
    /// quartic; this is fast, robust, and accurate enough for snap / hit-test.
    /// The initial guess is the angle of `p - center` in the ellipse's local
    /// frame, which is usually 1–2 iterations from the true root.
    pub fn nearest_param(&self, p: Vec2) -> f64 {
        let a = self.semi_major();
        if a < EPS { return 0.0; }
        let b = self.semi_minor();
        // Initial guess: rotate `p - center` into local frame, then take atan2
        // using the scaled coordinates so the angle matches PARAMETER space.
        let d = p - self.center;
        let lx = d.dot(self.u_hat());
        let ly = d.dot(self.v_hat());
        let mut t = (ly * a).atan2(lx * b);
        // 5 Newton iterations is more than enough for 1e-9 convergence in
        // double precision for any reasonable ratio.
        for _ in 0..5 {
            let pt = self.point_at(t);
            let dp = self.tangent_at(t);
            let d2 = -self.u_hat() * (a * t.cos()) - self.v_hat() * (b * t.sin());
            let f  = (pt - p).dot(dp);
            let fd = (pt - p).dot(d2) + dp.dot(dp);
            if fd.abs() < EPS { break; }
            t -= f / fd;
        }
        t.rem_euclid(std::f64::consts::TAU)
    }

    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        let t = self.nearest_param(p);
        self.point_at(t).dist(p)
    }
}

#[cfg(test)]
mod wall_explode_tests {
    use super::*;

    #[test]
    fn straight_wall_faces_are_offset_by_half_thickness() {
        // A horizontal wall of thickness 4 → two faces at y = ±2, both 2-point
        // (lines), which is exactly what `explode` turns into face Lines + caps.
        let w = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0),
                       thickness: 4.0, style: 0, bulge: 0.0 };
        let (left, right) = w.face_polylines(48).expect("faces");
        assert_eq!(left.len(), 2);
        assert_eq!(right.len(), 2);
        // faces sit ±2 off the centerline
        assert!((left[0].y.abs() - 2.0).abs() < 1e-9 && (right[0].y.abs() - 2.0).abs() < 1e-9);
        assert!((left[0].y * right[0].y) < 0.0, "faces must be on opposite sides");
        // end caps span the full thickness (4)
        let start_cap = (left[0] - right[0]).len();
        assert!((start_cap - 4.0).abs() < 1e-9, "cap width {start_cap}");
    }

    #[test]
    fn curved_wall_faces_are_sampled_polylines() {
        let w = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0),
                       thickness: 2.0, style: 0, bulge: 0.5 };
        let (left, right) = w.face_polylines(16).expect("faces");
        assert!(left.len() > 2 && right.len() > 2, "curved faces should sample many points");
    }
}

#[cfg(test)]
mod transform_tests {
    use super::*;
    use crate::join::*;
    use crate::trim::*;
    use crate::modify::*;
    use crate::math::approx_eq;

    #[test]
    fn line_rotated_90_around_origin() {
        let g = Geom::Line(Line { a: Vec2::new(1.0, 0.0), b: Vec2::new(2.0, 0.0) });
        let r = g.rotated(Vec2::ZERO, std::f64::consts::FRAC_PI_2);
        if let Geom::Line(l) = r {
            assert!(approx_eq(l.a.x, 0.0)); assert!(approx_eq(l.a.y, 1.0));
            assert!(approx_eq(l.b.x, 0.0)); assert!(approx_eq(l.b.y, 2.0));
        } else { panic!(); }
    }

    #[test]
    fn circle_scaled_2x_around_origin() {
        let g = Geom::Circle(Circle { center: Vec2::new(5.0, 0.0), radius: 3.0 });
        let s = g.scaled(Vec2::ZERO, 2.0);
        if let Geom::Circle(c) = s {
            assert!(approx_eq(c.center.x, 10.0));
            assert!(approx_eq(c.radius, 6.0));
        } else { panic!(); }
    }

    #[test]
    fn line_mirrored_across_x_axis() {
        // Axis from (-10,0) to (10,0). Point (3,4) mirrors to (3,-4).
        let g = Geom::Line(Line { a: Vec2::new(3.0, 4.0), b: Vec2::new(8.0, 2.0) });
        let m = g.mirrored(Vec2::new(-10.0, 0.0), Vec2::new(10.0, 0.0));
        if let Geom::Line(l) = m {
            assert!(approx_eq(l.a.x, 3.0)); assert!(approx_eq(l.a.y, -4.0));
            assert!(approx_eq(l.b.x, 8.0)); assert!(approx_eq(l.b.y, -2.0));
        } else { panic!(); }
    }

    #[test]
    fn line_reversed_swaps_endpoints() {
        let g = Geom::Line(Line { a: Vec2::new(1.0, 2.0), b: Vec2::new(7.0, 9.0) });
        if let Geom::Line(l) = g.reversed() {
            assert!(approx_eq(l.a.x, 7.0)); assert!(approx_eq(l.a.y, 9.0));
            assert!(approx_eq(l.b.x, 1.0)); assert!(approx_eq(l.b.y, 2.0));
        } else { panic!(); }
    }

    #[test]
    fn arc_reversed_preserves_geometry() {
        // An Arc stores only positive CCW sweep, so it can't encode direction.
        // Reversing must leave the SAME geometric arc (identical endpoints),
        // NOT relocate it. Arc from 0°→90°, radius 5: endpoints (5,0)→(0,5).
        let g = Geom::Arc(Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_2,
        });
        if let Geom::Arc(a) = g.reversed() {
            assert!(approx_eq(a.start_angle, 0.0));
            assert!(approx_eq(a.sweep_angle, std::f64::consts::FRAC_PI_2));
            let (p1, p2) = a.endpoints();
            assert!(approx_eq(p1.x, 5.0) && approx_eq(p1.y, 0.0));
            assert!(approx_eq(p2.x, 0.0) && approx_eq(p2.y, 5.0));
        } else { panic!(); }
    }

    #[test]
    fn polyline_reversed_flips_vertex_order_and_bulges() {
        let g = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.2 },
                PolyVertex { pos: Vec2::new(1.0, 0.0), bulge: -0.4 },
                PolyVertex { pos: Vec2::new(1.0, 1.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        });
        if let Geom::Polyline(p) = g.reversed() {
            assert_eq!(p.vertices[0].pos, Vec2::new(1.0, 1.0));
            assert_eq!(p.vertices[2].pos, Vec2::new(0.0, 0.0));
            // bulge[0] of reversed = -original bulge[1] (shifted+sign-flipped)
            assert!(approx_eq(p.vertices[0].bulge, 0.4));
            assert!(approx_eq(p.vertices[1].bulge, -0.2));
        } else { panic!(); }
    }

    #[test]
    fn offset_line_picks_side_by_hint() {
        // Horizontal line from (0,0) to (10,0). Side hint above → offset up.
        let g = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let up = g.offset(2.0, Vec2::new(5.0, 5.0)).unwrap();
        if let Geom::Line(l) = up {
            assert!(approx_eq(l.a.y, 2.0)); assert!(approx_eq(l.b.y, 2.0));
        } else { panic!(); }
        let dn = g.offset(2.0, Vec2::new(5.0, -5.0)).unwrap();
        if let Geom::Line(l) = dn {
            assert!(approx_eq(l.a.y, -2.0)); assert!(approx_eq(l.b.y, -2.0));
        } else { panic!(); }
    }

    #[test]
    fn offset_circle_outward_grows_radius() {
        let g = Geom::Circle(Circle { center: Vec2::ZERO, radius: 5.0 });
        let outside_hint = Vec2::new(10.0, 0.0);
        let out = g.offset(2.0, outside_hint).unwrap();
        if let Geom::Circle(c) = out { assert!(approx_eq(c.radius, 7.0)); } else { panic!(); }
        let inside_hint = Vec2::new(1.0, 0.0);
        let inn = g.offset(2.0, inside_hint).unwrap();
        if let Geom::Circle(c) = inn { assert!(approx_eq(c.radius, 3.0)); } else { panic!(); }
    }

    #[test]
    fn offset_closed_rectangle_inward_shrinks() {
        // 10×6 rectangle, CCW. Click INSIDE → offset shrinks it by `dist`
        // on every edge (8×4, same 4 corners pulled in).
        let g = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 6.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 6.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        });
        let out = g.offset(1.0, Vec2::new(5.0, 3.0)).expect("offset ok");
        let Geom::Polyline(pl) = out else { panic!("not a polyline") };
        assert!(pl.closed);
        assert_eq!(pl.vertices.len(), 4);
        let corners: Vec<Vec2> = pl.vertices.iter().map(|x| x.pos).collect();
        // Expect the inset rectangle (1,1)-(9,5).
        let want = [
            Vec2::new(1.0, 1.0), Vec2::new(9.0, 1.0),
            Vec2::new(9.0, 5.0), Vec2::new(1.0, 5.0),
        ];
        for w in &want {
            assert!(corners.iter().any(|c| approx_eq(c.x, w.x) && approx_eq(c.y, w.y)),
                "missing inset corner {:?} in {:?}", w, corners);
        }
    }

    #[test]
    fn offset_closed_rectangle_outward_grows() {
        let g = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 6.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 6.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        });
        // Click OUTSIDE (below the bottom edge) → grow.
        let out = g.offset(1.0, Vec2::new(5.0, -3.0)).expect("offset ok");
        let Geom::Polyline(pl) = out else { panic!("not a polyline") };
        let corners: Vec<Vec2> = pl.vertices.iter().map(|x| x.pos).collect();
        let want = [
            Vec2::new(-1.0, -1.0), Vec2::new(11.0, -1.0),
            Vec2::new(11.0, 7.0), Vec2::new(-1.0, 7.0),
        ];
        for w in &want {
            assert!(corners.iter().any(|c| approx_eq(c.x, w.x) && approx_eq(c.y, w.y)),
                "missing grown corner {:?} in {:?}", w, corners);
        }
    }

    #[test]
    fn offset_polyline_arc_segment_concentric() {
        // Single arc segment: quarter circle (1,0)→(0,1), centre origin,
        // CCW (bulge = tan(22.5°)). Offset OUTWARD → concentric radius 2,
        // same bulge.
        let b = (std::f64::consts::FRAC_PI_8).tan();   // tan(22.5°)
        let g = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(1.0, 0.0), bulge: b },
                PolyVertex { pos: Vec2::new(0.0, 1.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        });
        let out = g.offset(1.0, Vec2::new(1.0, 1.0)).expect("offset ok");
        let Geom::Polyline(pl) = out else { panic!("not a polyline") };
        assert_eq!(pl.vertices.len(), 2);
        assert!(approx_eq(pl.vertices[0].pos.x, 2.0) && approx_eq(pl.vertices[0].pos.y, 0.0),
            "arc start {:?} should scale to (2,0)", pl.vertices[0].pos);
        assert!(approx_eq(pl.vertices[1].pos.x, 0.0) && approx_eq(pl.vertices[1].pos.y, 2.0),
            "arc end {:?} should scale to (0,2)", pl.vertices[1].pos);
        assert!(approx_eq(pl.vertices[0].bulge, b), "bulge preserved (concentric)");
    }

    #[test]
    fn offset_open_polyline_two_segments() {
        // L-shape (open): (0,0)-(10,0)-(10,10). Offset to the +Y/-X inside.
        let g = Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        });
        let out = g.offset(1.0, Vec2::new(5.0, 1.0)).expect("offset ok");
        let Geom::Polyline(pl) = out else { panic!("not a polyline") };
        assert!(!pl.closed);
        assert_eq!(pl.vertices.len(), 3);
        // The miter corner moves from (10,0) to (9,1).
        let mid = pl.vertices[1].pos;
        assert!(approx_eq(mid.x, 9.0) && approx_eq(mid.y, 1.0),
            "miter corner {:?} should be (9,1)", mid);
    }

    #[test]
    fn lengthen_line_extends_at_clicked_end() {
        let g = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        // Click near (10, 0) — the b-end — to extend it.
        let longer = g.lengthened(3.0, Vec2::new(10.0, 1.0)).unwrap();
        if let Geom::Line(l) = longer { assert!(approx_eq(l.b.x, 13.0)); } else { panic!(); }
        // Click near (0, 0) — the a-end — to extend backwards.
        let earlier = g.lengthened(3.0, Vec2::new(-1.0, 0.0)).unwrap();
        if let Geom::Line(l) = earlier { assert!(approx_eq(l.a.x, -3.0)); } else { panic!(); }
    }

    #[test]
    fn trim_line_at_single_cutter_keeps_other_side() {
        // Horizontal line 0→10. Vertical cutter at x=5. Click at x=7 (right
        // of the cut) → that side is removed; we keep 0→5.
        let target  = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let cutter  = Geom::Line(Line { a: Vec2::new(5.0, -5.0), b: Vec2::new(5.0, 5.0) });
        let out = target.trim_at(&[cutter], Vec2::new(7.0, 0.0), false).unwrap();
        assert_eq!(out.len(), 1);
        if let Geom::Line(l) = &out[0] {
            assert!(approx_eq(l.a.x, 0.0)); assert!(approx_eq(l.b.x, 5.0));
        } else { panic!(); }
    }

    #[test]
    fn trim_line_between_two_cutters_keeps_outer_pieces() {
        // 0→10 cut at x=3 and x=7; click at x=5 (middle) removes 3..7.
        // Two cuts = three segments; click middle → 2 outer pieces survive.
        let target = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let c1 = Geom::Line(Line { a: Vec2::new(3.0, -5.0), b: Vec2::new(3.0, 5.0) });
        let c2 = Geom::Line(Line { a: Vec2::new(7.0, -5.0), b: Vec2::new(7.0, 5.0) });
        let out = target.trim_at(&[c1, c2], Vec2::new(5.0, 0.0), false).unwrap();
        assert_eq!(out.len(), 2);
        let mut xs: Vec<(f64, f64)> = out.iter().map(|g| {
            if let Geom::Line(l) = g { (l.a.x, l.b.x) } else { panic!() }
        }).collect();
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert!(approx_eq(xs[0].0, 0.0)); assert!(approx_eq(xs[0].1, 3.0));
        assert!(approx_eq(xs[1].0, 7.0)); assert!(approx_eq(xs[1].1, 10.0));
    }

    #[test]
    fn trim_line_with_three_cutters_two_outer_pieces() {
        // AutoCAD-correct: line 0→10 with cuts at x=2, 5, 8. Click in
        // interval (2..5) → ONLY that interval is removed. Surviving
        // pieces (NOT further split at x=8): (0..2) and (5..10).
        // Updated from the prior N+1 over-split rule (2026-06-08 — user
        // bug report: trimming outside a closed cutter incorrectly split
        // the line at the far intersection too).
        let target = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let cs: Vec<Geom> = [2.0, 5.0, 8.0].iter().map(|&x| {
            Geom::Line(Line { a: Vec2::new(x, -5.0), b: Vec2::new(x, 5.0) })
        }).collect();
        let out = target.trim_at(&cs, Vec2::new(3.5, 0.0), false).unwrap();
        assert_eq!(out.len(), 2,
            "expected 2 surviving pieces (AutoCAD-style), got {}", out.len());
        let mut xs: Vec<(f64, f64)> = out.iter().map(|g| {
            if let Geom::Line(l) = g { (l.a.x, l.b.x) } else { panic!() }
        }).collect();
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert!(approx_eq(xs[0].0, 0.0) && approx_eq(xs[0].1, 2.0));
        assert!(approx_eq(xs[1].0, 5.0) && approx_eq(xs[1].1, 10.0));   // not split at 8!
    }

    #[test]
    fn trim_line_with_five_cutters_two_pieces_on_middle_click() {
        // AutoCAD-correct: 5 cuts at x=2,4,6,8,10; click in interval
        // (4..6) → 2 surviving pieces: (0..4) and (6..12). Non-bounding
        // intersections at x=2, 8, 10 do NOT cause splits.
        let target = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(12.0, 0.0) });
        let cs: Vec<Geom> = [2.0, 4.0, 6.0, 8.0, 10.0].iter().map(|&x| {
            Geom::Line(Line { a: Vec2::new(x, -5.0), b: Vec2::new(x, 5.0) })
        }).collect();
        let out = target.trim_at(&cs, Vec2::new(5.0, 0.0), false).unwrap();
        assert_eq!(out.len(), 2,
            "expected 2 surviving pieces (AutoCAD-style), got {}", out.len());
        let mut xs: Vec<(f64, f64)> = out.iter().map(|g| {
            if let Geom::Line(l) = g { (l.a.x, l.b.x) } else { panic!() }
        }).collect();
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert!(approx_eq(xs[0].0, 0.0) && approx_eq(xs[0].1, 4.0));
        assert!(approx_eq(xs[1].0, 6.0) && approx_eq(xs[1].1, 12.0));
    }

    #[test]
    fn trim_line_outside_closed_cutter_keeps_one_continuous_piece() {
        // User-reported bug 2026-06-08: line crosses an ellipse at TWO
        // points; click on the OUTSIDE-A portion (before first
        // intersection). Survivor should be ONE continuous piece from
        // intersection1 → line.end (passing through the inside AND
        // outside-B without further splits).
        let target = Geom::Line(Line {
            a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0),
        });
        // Two perpendicular cutters at x=3 and x=7 (simulate ellipse's
        // two intersections with the line).
        let c1 = Geom::Line(Line { a: Vec2::new(3.0, -2.0), b: Vec2::new(3.0, 2.0) });
        let c2 = Geom::Line(Line { a: Vec2::new(7.0, -2.0), b: Vec2::new(7.0, 2.0) });
        // Click before first intersection — on the OUTSIDE-A portion.
        let out = target.trim_at(&[c1, c2], Vec2::new(1.5, 0.0), false).unwrap();
        assert_eq!(out.len(), 1,
            "expected ONE continuous piece (3 → 10), got {}", out.len());
        if let Geom::Line(l) = &out[0] {
            assert!(approx_eq(l.a.x, 3.0));
            assert!(approx_eq(l.b.x, 10.0));
        }
    }

    #[test]
    fn trim_arc_with_two_cutters_breaks_into_two_subarcs_on_middle_click() {
        use std::f64::consts::FRAC_PI_2;
        // Half-arc 0°→180° at radius 5. Cuts at 60° and 120°. Click at 90°
        // (between the cuts) → segments (0..60) and (120..180) survive.
        let arc = Geom::Arc(Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::PI,
        });
        let cx_60 = 5.0 * (60.0_f64).to_radians().cos();
        let cy_60 = 5.0 * (60.0_f64).to_radians().sin();
        let cx_120 = 5.0 * (120.0_f64).to_radians().cos();
        let cy_120 = 5.0 * (120.0_f64).to_radians().sin();
        // Radial cut lines from origin through each angle, long enough.
        let c1 = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(cx_60 * 2.0, cy_60 * 2.0) });
        let c2 = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(cx_120 * 2.0, cy_120 * 2.0) });
        // Click at 90° (top of arc): (0, 5)
        let out = arc.trim_at(&[c1, c2], Vec2::new(0.0, 5.0), false).unwrap();
        assert_eq!(out.len(), 2, "expected 2 sub-arcs, got {}", out.len());
        for g in &out {
            if let Geom::Arc(a) = g {
                let sweep_deg = a.sweep_angle.to_degrees();
                assert!((sweep_deg - 60.0).abs() < 1.0,
                    "each surviving sub-arc should sweep ~60°, got {}°", sweep_deg);
            } else { panic!() }
        }
        let _ = FRAC_PI_2;
    }

    #[test]
    fn trim_uses_edge_mode_for_imaginary_intersection() {
        // Target line 0→10 at y=0. Cutter is a SHORT segment that, in its
        // visible form, does NOT cross the target. With EdgMod ON it
        // extends to a full line that DOES cross at x=5.
        let target = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let short_cutter = Geom::Line(Line {
            a: Vec2::new(5.0, 3.0), b: Vec2::new(5.0, 4.0),    // y=3..4 only
        });
        // EdgMod OFF — no intersection; trim fails.
        assert!(target.trim_at(&[short_cutter.clone()], Vec2::new(7.0, 0.0), false).is_err());
        // EdgMod ON — imaginary intersection at (5,0); right side trimmed.
        let out = target.trim_at(&[short_cutter], Vec2::new(7.0, 0.0), true).unwrap();
        assert_eq!(out.len(), 1);
        if let Geom::Line(l) = &out[0] {
            assert!(approx_eq(l.b.x, 5.0));
        } else { panic!(); }
    }

    #[test]
    fn trim_arc_at_single_radial_cutter_keeps_other_side() {
        // Quarter-arc 0°→90° at origin, radius 5. Cutter is a vertical line
        // at x = 5·cos(45°) ≈ 3.536, which intersects the arc at 45°.
        // Click at angle 22° (lower-left half) → removes that side, keeps 45°→90°.
        let arc = Geom::Arc(Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::FRAC_PI_2,
        });
        let cut_x = 5.0 * std::f64::consts::FRAC_1_SQRT_2;
        let cutter = Geom::Line(Line {
            a: Vec2::new(cut_x, -10.0), b: Vec2::new(cut_x, 10.0),
        });
        // Click below the cutter (small angle on the arc)
        let pick = Vec2::new(4.5, 1.0);   // ≈12° on the arc
        let out = arc.trim_at(&[cutter], pick, false).unwrap();
        assert_eq!(out.len(), 1);
        if let Geom::Arc(a) = &out[0] {
            // Should start at ≈45° and sweep to ≈90°
            assert!(a.start_angle > 0.5 && a.start_angle < 1.0,
                "start_angle = {} rad ({}°)", a.start_angle, a.start_angle.to_degrees());
            assert!((a.sweep_angle - std::f64::consts::FRAC_PI_4).abs() < 0.05,
                "sweep_angle = {} rad", a.sweep_angle);
        } else { panic!(); }
    }

    #[test]
    fn trim_target_with_disjoint_cutters_errors_when_pick_is_outside() {
        // Line 0→10. Cutter at x=15 (no intersection on the visible line).
        // EdgMod OFF — no imaginary intersection either. trim should fail.
        let target = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let cutter = Geom::Line(Line {
            a: Vec2::new(15.0, -1.0), b: Vec2::new(15.0, 1.0),
        });
        let r = target.trim_at(&[cutter], Vec2::new(5.0, 0.0), false);
        assert!(r.is_err());
    }

    #[test]
    fn trim_with_pick_on_endpoint_side_of_intersection() {
        // Line 0→10 cut at x=5. Click at x=2 (left of cut) → trims toward a;
        // result keeps the right half (5..10).
        let target = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let cutter = Geom::Line(Line { a: Vec2::new(5.0, -1.0), b: Vec2::new(5.0, 1.0) });
        let out = target.trim_at(&[cutter], Vec2::new(2.0, 0.0), false).unwrap();
        assert_eq!(out.len(), 1);
        if let Geom::Line(l) = &out[0] {
            assert!(approx_eq(l.a.x, 5.0)); assert!(approx_eq(l.b.x, 10.0));
        } else { panic!(); }
    }

    #[test]
    fn trim_uses_visible_intersection_first_with_edgemode_off() {
        // Cutter VISIBLY intersects target. EdgMod OFF: should use the real
        // intersection (no need to extend). Confirms we don't accidentally
        // always extend.
        let target = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let cutter = Geom::Line(Line { a: Vec2::new(5.0, -5.0), b: Vec2::new(5.0, 5.0) });
        let out = target.trim_at(&[cutter.clone()], Vec2::new(7.0, 0.0), false).unwrap();
        // Same result as the EdgMod-ON case for this geometry; assert we
        // got the trimmed left half.
        if let Geom::Line(l) = &out[0] {
            assert!(approx_eq(l.b.x, 5.0));
        } else { panic!(); }
    }

    #[test]
    fn extend_arc_lengthens_sweep_to_boundary() {
        // Quarter-arc 0°→90° at radius 5. Boundary line at y=5 (already
        // tangent-ish to top of arc at 90°). Pick a point past the 90°
        // endpoint to extend forward; expect sweep grows past 90°.
        // Simpler boundary: vertical line at x = -2 (cuts arc extension
        // at angle just past 90°).
        let arc = Geom::Arc(Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::FRAC_PI_2,
        });
        let boundary = Geom::Line(Line {
            a: Vec2::new(-2.0, -10.0), b: Vec2::new(-2.0, 10.0),
        });
        // End of arc is at (0, 5). Click slightly past it (above and left).
        let pick = Vec2::new(-0.5, 5.5);
        let out = arc.extend_to(&[boundary], pick, true).unwrap();
        if let Geom::Arc(a) = out {
            // Sweep should be > original PI/2 but < PI (since x=-2 → angle ≈113°)
            assert!(a.sweep_angle > std::f64::consts::FRAC_PI_2,
                "sweep didn't grow: {}", a.sweep_angle);
            assert!(a.sweep_angle < std::f64::consts::PI,
                "sweep overshot: {}", a.sweep_angle);
        } else { panic!(); }
    }

    #[test]
    fn document_level_apply_trim_pick_shape() {
        // Simulate what apply_trim_pick does in cad_app: trim target,
        // remove it from the Document, push the surviving pieces. Confirms
        // the index-shift logic produces a coherent Document.
        use crate::Document;
        let mut doc = Document::default();
        // Cutter at index 0, target at index 1.
        let cutter_i = doc.push(Line {
            a: Vec2::new(5.0, -5.0), b: Vec2::new(5.0, 5.0),
        }.into());
        let target_i = doc.push(Line {
            a: Vec2::ZERO, b: Vec2::new(10.0, 0.0),
        }.into());
        assert_eq!(cutter_i, 0);
        assert_eq!(target_i, 1);

        // Mirror apply_trim_pick:
        let cutter_geom = doc.dobjects[cutter_i].geom.clone();
        let target_style = doc.dobjects[target_i].style;
        let pieces = doc.dobjects[target_i].geom
            .trim_at(&[cutter_geom], Vec2::new(7.0, 0.0), true)
            .unwrap();
        doc.dobjects.remove(target_i);
        for g in pieces {
            let mut d = crate::DObject::new(g);
            d.style = target_style;
            doc.push(d);
        }
        // Now doc.dobjects = [cutter, piece]; total 2, both Lines.
        assert_eq!(doc.dobjects.len(), 2);
        // Cutter still at index 0 (unchanged).
        if let Geom::Line(l) = &doc.dobjects[0].geom {
            assert!(approx_eq(l.a.x, 5.0));
        } else { panic!("cutter shifted or mutated"); }
        // Piece is the original LEFT half (since pick was on the right at x=7).
        if let Geom::Line(l) = &doc.dobjects[1].geom {
            assert!(approx_eq(l.a.x, 0.0));
            assert!(approx_eq(l.b.x, 5.0));
        } else { panic!(); }
    }

    #[test]
    fn extend_line_grows_toward_boundary() {
        // Target line 0→4 at y=0. Boundary at x=10.
        let target = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(4.0, 0.0) });
        let boundary = Geom::Line(Line { a: Vec2::new(10.0, -5.0), b: Vec2::new(10.0, 5.0) });
        // Click near the right end (b) — extend that side toward x=10.
        let out = target.extend_to(&[boundary], Vec2::new(4.0, 0.5), false).unwrap();
        if let Geom::Line(l) = out {
            assert!(approx_eq(l.a.x, 0.0));
            assert!(approx_eq(l.b.x, 10.0));
        } else { panic!(); }
    }

    #[test]
    fn extend_wall_target_reaches_boundary() {
        // Regression: a WALL target used to fail extend because
        // `extended_for_edgemode` didn't lengthen walls, so the early
        // hits-empty guard bailed before the Wall arm could run. The wall's
        // top end (y=4) must extend up to the boundary at y=10.
        let target = Geom::Wall(Wall {
            start: Vec2::new(2.0, 0.0), end: Vec2::new(2.0, 4.0),
            thickness: 0.3, style: 0, bulge: 0.0,
        });
        let boundary = Geom::Line(Line { a: Vec2::new(-5.0, 10.0), b: Vec2::new(5.0, 10.0) });
        // Click near the top end (y=4) → extend that side up to y=10.
        let out = target.extend_to(&[boundary], Vec2::new(2.0, 3.5), false).unwrap();
        if let Geom::Wall(w) = out {
            assert!(approx_eq(w.start.y, 0.0));
            assert!(approx_eq(w.end.y, 10.0));
            assert!(approx_eq(w.thickness, 0.3));
        } else {
            panic!("expected a Wall back");
        }
    }

    #[test]
    fn break_line_at_midpoint_makes_two() {
        let g = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let (l1, l2) = g.split_at(Vec2::new(5.0, 0.0)).unwrap();
        if let (Geom::Line(a), Geom::Line(b)) = (l1, l2) {
            assert!(approx_eq(a.b.x, 5.0));
            assert!(approx_eq(b.a.x, 5.0));
            assert!(approx_eq(b.b.x, 10.0));
        } else { panic!(); }
    }

    #[test]
    fn break_circle_errors() {
        let g = Geom::Circle(Circle { center: Vec2::ZERO, radius: 5.0 });
        assert!(g.split_at(Vec2::new(5.0, 0.0)).is_err());
    }

    #[test]
    fn translate_then_rotate_then_translate_back() {
        // Translation invariance under rotation around the SAME pivot
        let g = Geom::Circle(Circle { center: Vec2::new(7.0, 3.0), radius: 2.0 });
        let g2 = g.translated(Vec2::new(10.0, 0.0));
        let g3 = g2.rotated(Vec2::new(17.0, 3.0), std::f64::consts::PI);
        let g4 = g3.translated(Vec2::new(-10.0, 0.0));
        if let Geom::Circle(c) = g4 {
            // Should be rotated 180° about (7,3) which moves (7,3)+r=2 to the same spot
            assert!(approx_eq(c.center.x, 7.0));
            assert!(approx_eq(c.center.y, 3.0));
            assert!(approx_eq(c.radius, 2.0));
        } else { panic!(); }
    }
}

#[cfg(test)]
mod ellipse_tests {
    use super::*;
    use crate::join::*;
    use crate::trim::*;
    use crate::modify::*;
    use crate::math::approx_eq;

    fn close(p: Vec2, x: f64, y: f64) -> bool {
        approx_eq(p.x, x) && approx_eq(p.y, y)
    }

    #[test]
    fn axis_aligned_ellipse_point_at() {
        // a = 5, b = 2, no rotation
        let e = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 };
        assert!(close(e.point_at(0.0), 5.0, 0.0));
        assert!(close(e.point_at(std::f64::consts::FRAC_PI_2), 0.0, 2.0));
        assert!(close(e.point_at(std::f64::consts::PI), -5.0, 0.0));
    }

    #[test]
    fn rotated_ellipse_bbox() {
        // Rotate 90°: major now points up. Bbox half-extents swap.
        let e = Ellipse { center: Vec2::ZERO, major: Vec2::new(0.0, 5.0), ratio: 0.4 };
        let (mn, mx) = e.bbox();
        assert!(approx_eq(mx.x, 2.0));   // semi-minor along x
        assert!(approx_eq(mx.y, 5.0));   // semi-major along y
        assert!(approx_eq(mn.x, -2.0));
        assert!(approx_eq(mn.y, -5.0));
    }

    #[test]
    fn nearest_param_on_circle_is_atan2() {
        // ratio=1.0 means circle — nearest point on a circle is the radial.
        let e = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 1.0 };
        let p = Vec2::new(10.0, 10.0);
        let t = e.nearest_param(p);
        let pt = e.point_at(t);
        // pt should be on the circle of r=5 in the same direction as p.
        assert!(approx_eq(pt.len(), 5.0));
        assert!(approx_eq((pt.y / pt.x).atan(), (10.0_f64 / 10.0_f64).atan()));
    }

    #[test]
    fn ellipse_arc_endpoints_and_contains() {
        let e = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 };
        let ea = EllipseArc {
            ellipse: e,
            start_param: 0.0,
            sweep_param: std::f64::consts::FRAC_PI_2,
        };
        let (p1, p2) = ea.endpoints();
        assert!(close(p1, 5.0, 0.0));
        assert!(close(p2, 0.0, 2.0));
        assert!(ea.contains_param(0.0));
        assert!(ea.contains_param(std::f64::consts::FRAC_PI_4));
        assert!(!ea.contains_param(std::f64::consts::PI));
    }
}

impl EllipseArc {
    /// True if parameter `t` lies in the swept range, mod TAU.
    pub fn contains_param(&self, t: f64) -> bool {
        let d = (t - self.start_param).rem_euclid(std::f64::consts::TAU);
        d <= self.sweep_param + EPS
    }

    pub fn endpoints(&self) -> (Vec2, Vec2) {
        (self.ellipse.point_at(self.start_param),
         self.ellipse.point_at(self.start_param + self.sweep_param))
    }

    /// Distance from the visible arc to a point. If the nearest-on-full-
    /// ellipse parameter lies in the swept range, that's the foot; otherwise
    /// the answer is whichever endpoint is closer.
    pub fn distance_to_point(&self, p: Vec2) -> f64 {
        let t = self.ellipse.nearest_param(p);
        if self.contains_param(t) {
            self.ellipse.point_at(t).dist(p)
        } else {
            let (e1, e2) = self.endpoints();
            p.dist(e1).min(p.dist(e2))
        }
    }
}

/// Characteristic role of a grip point — drives `Geom::with_grip_moved`.
/// Two opposing quadrant/axis tips (e.g. circle +X and -X) share the same
/// role because their drag semantic is symmetric (radius change / axis
/// reflection); the renderer still draws all four squares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GripRole {
    LineEndA,
    LineEndB,
    LineMid,
    CircleCenter,
    CircleQuadrant,
    ArcEndStart,
    ArcEndEnd,
    ArcMid,
    ArcCenter,
    EllipseCenter,
    EllipseMajorTip,        // either +major or -major tip
    EllipseMinorTip,        // either +minor or -minor tip
    EllipseArcEndStart,
    EllipseArcEndEnd,
    EllipseArcCenter,
    PolyVertex(usize),
    PointLoc,
    /// Block instance insertion point — dragging it translates the
    /// whole instance.
    BlockInsert,
    /// Control point of a NURBS spline at the given index. Dragging
    /// reshapes the curve via control-point edit (the dragged point
    /// stays put; the curve bends towards it).
    SplineCtrlPt(usize),
    /// Dimension def-point grips. Three per Dim:
    ///   * `DimP1`     — Linear.p1, or Radius/Diameter.center
    ///   * `DimP2`     — Linear.p2, or Radius/Diameter.on_circle
    ///   * `DimLeader` — Linear.dimline_pos, or Radius/Diameter.leader_end
    /// `with_grip_moved` interprets the role against the current
    /// DimKind.
    DimP1,
    DimP2,
    DimLeader,
}

impl Geom {
    /// Characteristic grip points + their roles. Renderer draws a square
    /// at each position; `with_grip_moved` knows how to edit the geometry
    /// based on the role.
    pub fn grip_points(&self) -> Vec<(Vec2, GripRole)> {
        match self {
            Geom::Line(l) => vec![
                (l.a, GripRole::LineEndA),
                (l.b, GripRole::LineEndB),
                ((l.a + l.b) * 0.5, GripRole::LineMid),
            ],
            Geom::Circle(c) => {
                let r = c.radius;
                vec![
                    (c.center, GripRole::CircleCenter),
                    (c.center + Vec2::new( r, 0.0), GripRole::CircleQuadrant),
                    (c.center + Vec2::new( 0.0,  r), GripRole::CircleQuadrant),
                    (c.center + Vec2::new(-r, 0.0), GripRole::CircleQuadrant),
                    (c.center + Vec2::new( 0.0, -r), GripRole::CircleQuadrant),
                ]
            }
            Geom::Arc(a) => {
                let (e1, e2) = a.endpoints();
                let mid_t = a.start_angle + a.sweep_angle * 0.5;
                let mid   = a.center + Vec2::new(
                    a.radius * mid_t.cos(),
                    a.radius * mid_t.sin(),
                );
                vec![
                    (e1,        GripRole::ArcEndStart),
                    (e2,        GripRole::ArcEndEnd),
                    (mid,       GripRole::ArcMid),
                    (a.center,  GripRole::ArcCenter),
                ]
            }
            Geom::Ellipse(el) => {
                let half = std::f64::consts::FRAC_PI_2;
                vec![
                    (el.center,                                GripRole::EllipseCenter),
                    (el.point_at(0.0),                          GripRole::EllipseMajorTip),
                    (el.point_at(half),                         GripRole::EllipseMinorTip),
                    (el.point_at(std::f64::consts::PI),         GripRole::EllipseMajorTip),
                    (el.point_at(std::f64::consts::PI + half),  GripRole::EllipseMinorTip),
                ]
            }
            Geom::EllipseArc(ea) => {
                let (e1, e2) = ea.endpoints();
                vec![
                    (e1, GripRole::EllipseArcEndStart),
                    (e2, GripRole::EllipseArcEndEnd),
                    (ea.ellipse.center, GripRole::EllipseArcCenter),
                ]
            }
            Geom::Polyline(p) => p.vertices.iter().enumerate()
                .map(|(i, v)| (v.pos, GripRole::PolyVertex(i)))
                .collect(),
            Geom::Point(p) => vec![(p.location, GripRole::PointLoc)],
            // Hatch MVP exposes no grips. The boundary vertices could
            // become PolyVertex grips later; for now editing happens by
            // recreating the hatch from a different boundary.
            Geom::Hatch(_) => Vec::new(),
            // Spline grips = every control point. Dragging one
            // reshapes the curve locally (NURBS basis support is
            // narrow — drag at index i only affects a span of degree+1
            // segments around i).
            Geom::Spline(s) => s.control_points.iter().enumerate()
                .map(|(i, p)| (*p, GripRole::SplineCtrlPt(i)))
                .collect(),
            // Wall — grip at each centerline endpoint + midpoint.
            // Re-uses Line grip roles so the existing renderer + apply
            // path "just work"; `with_grip_moved` below maps them back.
            Geom::Wall(w) => vec![
                (w.start, GripRole::LineEndA),
                (w.end,   GripRole::LineEndB),
                ((w.start + w.end) * 0.5, GripRole::LineMid),
            ],
            // Text — one grip at the anchor position. Re-uses PointLoc
            // role so `with_grip_moved` repositions text the same way
            // it repositions Points.
            Geom::Text(t) => vec![(t.position, GripRole::PointLoc)],
            // Dimension — three role-tagged grips matching the def
            // points. Roles are uniform across kinds so the renderer
            // can treat them generically.
            Geom::Dimension(d) => {
                use crate::dim::DimKind;
                match &d.kind {
                    DimKind::Linear { p1, p2, dimline_pos, .. } => vec![
                        (*p1,          GripRole::DimP1),
                        (*p2,          GripRole::DimP2),
                        (*dimline_pos, GripRole::DimLeader),
                    ],
                    DimKind::Radius { center, on_circle, leader_end } |
                    DimKind::Diameter { center, on_circle, leader_end } => vec![
                        (*center,     GripRole::DimP1),
                        (*on_circle,  GripRole::DimP2),
                        (*leader_end, GripRole::DimLeader),
                    ],
                }
            }
            // BlockRef — one grip at the insertion point; dragging it
            // translates the whole instance.
            Geom::BlockRef(br) => vec![(br.insert, GripRole::BlockInsert)],
        }
    }

    /// Produce a new geometry with the given grip moved to `new_pos`.
    /// All math reuses kernel methods already in this file — this is
    /// just per-role dispatch.
    pub fn with_grip_moved(&self, role: GripRole, new_pos: Vec2) -> Geom {
        match (self, role) {
            // ---- Line --------------------------------------------------
            (Geom::Line(l), GripRole::LineEndA) =>
                Geom::Line(Line { a: new_pos, b: l.b }),
            (Geom::Line(l), GripRole::LineEndB) =>
                Geom::Line(Line { a: l.a, b: new_pos }),
            (Geom::Line(l), GripRole::LineMid) => {
                let delta = new_pos - (l.a + l.b) * 0.5;
                Geom::Line(Line { a: l.a + delta, b: l.b + delta })
            }
            // ---- Circle ------------------------------------------------
            (Geom::Circle(c), GripRole::CircleCenter) =>
                Geom::Circle(Circle { center: new_pos, radius: c.radius }),
            (Geom::Circle(c), GripRole::CircleQuadrant) => {
                let new_r = (new_pos - c.center).len().max(EPS);
                Geom::Circle(Circle { center: c.center, radius: new_r })
            }
            // ---- Arc ---------------------------------------------------
            (Geom::Arc(a), GripRole::ArcCenter) => {
                let delta = new_pos - a.center;
                Geom::Arc(Arc {
                    center: a.center + delta,
                    radius: a.radius,
                    start_angle: a.start_angle,
                    sweep_angle: a.sweep_angle,
                })
            }
            (Geom::Arc(a), GripRole::ArcEndStart) => {
                // Slide start endpoint along the radial direction at the
                // new angle; sweep adjusts so the END endpoint stays put.
                let new_start = (new_pos - a.center).angle()
                    .rem_euclid(std::f64::consts::TAU);
                let old_end_abs = (a.start_angle + a.sweep_angle)
                    .rem_euclid(std::f64::consts::TAU);
                let new_sweep = (old_end_abs - new_start)
                    .rem_euclid(std::f64::consts::TAU);
                Geom::Arc(Arc {
                    center: a.center,
                    radius: a.radius,
                    start_angle: new_start,
                    sweep_angle: new_sweep.max(EPS),
                })
            }
            (Geom::Arc(a), GripRole::ArcEndEnd) => {
                let new_end_abs = (new_pos - a.center).angle()
                    .rem_euclid(std::f64::consts::TAU);
                let new_sweep = (new_end_abs - a.start_angle)
                    .rem_euclid(std::f64::consts::TAU);
                Geom::Arc(Arc {
                    center: a.center,
                    radius: a.radius,
                    start_angle: a.start_angle,
                    sweep_angle: new_sweep.max(EPS),
                })
            }
            (Geom::Arc(a), GripRole::ArcMid) => {
                // v1: translate whole arc so the midpoint lands at new_pos.
                let mid_t = a.start_angle + a.sweep_angle * 0.5;
                let mid = a.center + Vec2::new(
                    a.radius * mid_t.cos(),
                    a.radius * mid_t.sin(),
                );
                let delta = new_pos - mid;
                Geom::Arc(Arc {
                    center: a.center + delta,
                    radius: a.radius,
                    start_angle: a.start_angle,
                    sweep_angle: a.sweep_angle,
                })
            }
            // ---- Ellipse -----------------------------------------------
            (Geom::Ellipse(el), GripRole::EllipseCenter) =>
                Geom::Ellipse(Ellipse {
                    center: new_pos,
                    major:  el.major,
                    ratio:  el.ratio,
                }),
            (Geom::Ellipse(el), GripRole::EllipseMajorTip) => {
                // Major axis becomes (new_pos - center). Ratio (b/a) stays
                // so b scales proportionally. Direction of the ellipse
                // also rotates to match.
                let new_major = new_pos - el.center;
                if new_major.len() < EPS {
                    return Geom::Ellipse(*el);
                }
                Geom::Ellipse(Ellipse {
                    center: el.center,
                    major:  new_major,
                    ratio:  el.ratio,
                })
            }
            (Geom::Ellipse(el), GripRole::EllipseMinorTip) => {
                // Hold major direction + length; change ratio so the
                // minor tip lands at the projection of new_pos onto v̂.
                let new_b = (new_pos - el.center).dot(el.v_hat()).abs().max(EPS);
                let new_ratio = (new_b / el.semi_major()).max(1e-6);
                Geom::Ellipse(Ellipse {
                    center: el.center,
                    major:  el.major,
                    ratio:  new_ratio,
                })
            }
            // ---- EllipseArc --------------------------------------------
            (Geom::EllipseArc(ea), GripRole::EllipseArcCenter) => {
                let delta = new_pos - ea.ellipse.center;
                Geom::EllipseArc(EllipseArc {
                    ellipse: Ellipse {
                        center: ea.ellipse.center + delta,
                        ..ea.ellipse
                    },
                    start_param: ea.start_param,
                    sweep_param: ea.sweep_param,
                })
            }
            (Geom::EllipseArc(ea), GripRole::EllipseArcEndStart) => {
                // Re-project new_pos to the ellipse parameter; new start_param
                // = that t; sweep_param adjusts so the END endpoint stays.
                let new_start = ea.ellipse.nearest_param(new_pos)
                    .rem_euclid(std::f64::consts::TAU);
                let old_end = (ea.start_param + ea.sweep_param)
                    .rem_euclid(std::f64::consts::TAU);
                let new_sweep = (old_end - new_start)
                    .rem_euclid(std::f64::consts::TAU);
                Geom::EllipseArc(EllipseArc {
                    ellipse: ea.ellipse,
                    start_param: new_start,
                    sweep_param: new_sweep.max(EPS),
                })
            }
            (Geom::EllipseArc(ea), GripRole::EllipseArcEndEnd) => {
                let new_end = ea.ellipse.nearest_param(new_pos)
                    .rem_euclid(std::f64::consts::TAU);
                let new_sweep = (new_end - ea.start_param)
                    .rem_euclid(std::f64::consts::TAU);
                Geom::EllipseArc(EllipseArc {
                    ellipse: ea.ellipse,
                    start_param: ea.start_param,
                    sweep_param: new_sweep.max(EPS),
                })
            }
            // ---- Polyline ----------------------------------------------
            (Geom::Polyline(p), GripRole::PolyVertex(i)) => {
                let mut new_verts = p.vertices.clone();
                if let Some(v) = new_verts.get_mut(i) { v.pos = new_pos; }
                Geom::Polyline(Polyline { vertices: new_verts, closed: p.closed, widths: p.widths.clone() })
            }
            // ---- Spline (control-point edit) ---------------------------
            (Geom::Spline(s), GripRole::SplineCtrlPt(i)) => {
                let mut new_ctrls = s.control_points.clone();
                if let Some(c) = new_ctrls.get_mut(i) { *c = new_pos; }
                Geom::Spline(Spline {
                    degree:         s.degree,
                    control_points: new_ctrls,
                    weights:        s.weights.clone(),
                })
            }
            // ---- Point -------------------------------------------------
            (Geom::Point(p), GripRole::PointLoc) =>
                Geom::Point(Point { location: new_pos, style: p.style, size: p.size }),
            // ---- Text — re-uses PointLoc role for its single anchor grip.
            (Geom::Text(t), GripRole::PointLoc) => {
                let mut nt = t.clone();
                nt.position = new_pos;
                Geom::Text(nt)
            }
            // ---- Wall — re-uses Line grip roles --------------------------
            // Grip drags reshape the centerline; both side lines re-derive
            // on render so the wall moves coherently as one entity.
            (Geom::Wall(w), GripRole::LineEndA) => Geom::Wall(Wall {
                start: new_pos, end: w.end, thickness: w.thickness,
                style: w.style, bulge: w.bulge,
            }),
            (Geom::Wall(w), GripRole::LineEndB) => Geom::Wall(Wall {
                start: w.start, end: new_pos, thickness: w.thickness,
                style: w.style, bulge: w.bulge,
            }),
            (Geom::Wall(w), GripRole::LineMid) => {
                // Move the whole wall — translate by (new_pos - current mid).
                let mid = (w.start + w.end) * 0.5;
                let off = new_pos - mid;
                Geom::Wall(Wall {
                    start: w.start + off, end: w.end + off,
                    thickness: w.thickness,
                    style: w.style, bulge: w.bulge,
                })
            }
            // ---- Dimension --------------------------------------------------
            // DimP1/P2/Leader move whichever def point corresponds to
            // the role for the current DimKind. Renderer re-derives
            // extension lines, dim line, and arrows on the next frame.
            (Geom::Dimension(d), GripRole::DimP1) => {
                use crate::dim::DimKind;
                let new_kind = match &d.kind {
                    DimKind::Linear { p2, dimline_pos, ortho, .. } => DimKind::Linear {
                        p1: new_pos, p2: *p2, dimline_pos: *dimline_pos, ortho: *ortho,
                    },
                    DimKind::Radius { on_circle, leader_end, .. } => DimKind::Radius {
                        center: new_pos, on_circle: *on_circle, leader_end: *leader_end,
                    },
                    DimKind::Diameter { on_circle, leader_end, .. } => DimKind::Diameter {
                        center: new_pos, on_circle: *on_circle, leader_end: *leader_end,
                    },
                };
                Geom::Dimension(crate::dim::Dim {
                    kind: new_kind, style: d.style, text_override: d.text_override.clone(),
                })
            }
            (Geom::Dimension(d), GripRole::DimP2) => {
                use crate::dim::DimKind;
                let new_kind = match &d.kind {
                    DimKind::Linear { p1, dimline_pos, ortho, .. } => DimKind::Linear {
                        p1: *p1, p2: new_pos, dimline_pos: *dimline_pos, ortho: *ortho,
                    },
                    DimKind::Radius { center, leader_end, .. } => DimKind::Radius {
                        center: *center, on_circle: new_pos, leader_end: *leader_end,
                    },
                    DimKind::Diameter { center, leader_end, .. } => DimKind::Diameter {
                        center: *center, on_circle: new_pos, leader_end: *leader_end,
                    },
                };
                Geom::Dimension(crate::dim::Dim {
                    kind: new_kind, style: d.style, text_override: d.text_override.clone(),
                })
            }
            (Geom::Dimension(d), GripRole::DimLeader) => {
                use crate::dim::DimKind;
                let new_kind = match &d.kind {
                    DimKind::Linear { p1, p2, ortho, .. } => DimKind::Linear {
                        p1: *p1, p2: *p2, dimline_pos: new_pos, ortho: *ortho,
                    },
                    DimKind::Radius { center, on_circle, .. } => DimKind::Radius {
                        center: *center, on_circle: *on_circle, leader_end: new_pos,
                    },
                    DimKind::Diameter { center, on_circle, .. } => DimKind::Diameter {
                        center: *center, on_circle: *on_circle, leader_end: new_pos,
                    },
                };
                Geom::Dimension(crate::dim::Dim {
                    kind: new_kind, style: d.style, text_override: d.text_override.clone(),
                })
            }
            // BlockRef — the insertion grip carries the whole instance.
            (Geom::BlockRef(br), GripRole::BlockInsert) => {
                Geom::BlockRef(crate::block::BlockRef {
                    insert: new_pos, ..*br
                })
            }
            // Mismatched (role, geom) — return unchanged.
            (g, _) => g.clone(),
        }
    }
}







#[cfg(test)]
mod fillet_chamfer_join_tests {
    use super::*;
    use crate::join::*;
    use crate::trim::*;
    use crate::modify::*;
    use crate::math::approx_eq;

    fn ln(ax: f64, ay: f64, bx: f64, by: f64) -> Line {
        Line { a: Vec2::new(ax, ay), b: Vec2::new(bx, by) }
    }

    // --- fillet -----------------------------------------------------------

    #[test]
    fn fillet_right_angle_radius_1() {
        let l1 = ln(0.0, 0.0, 5.0, 0.0);  // along +X
        let l2 = ln(0.0, 0.0, 0.0, 5.0);  // along +Y
        let p1 = Vec2::new(3.0, 0.0);
        let p2 = Vec2::new(0.0, 3.0);
        let out = fillet_lines(&l1, p1, &l2, p2, 1.0).unwrap();
        if let Geom::Line(l) = out.g1_new {
            assert!(approx_eq(l.a.x, 5.0));
            assert!(approx_eq(l.b.x, 1.0));
            assert!(approx_eq(l.b.y, 0.0));
        } else { panic!("g1_new not a Line") }
        if let Geom::Line(l) = out.g2_new {
            assert!(approx_eq(l.b.y, 5.0));
            assert!(approx_eq(l.a.x, 0.0));
            assert!(approx_eq(l.a.y, 1.0));
        } else { panic!("g2_new not a Line") }
        let arc = out.arc.expect("expected an arc for r>0");
        if let Geom::Arc(a) = arc {
            assert!(approx_eq(a.radius, 1.0));
            assert!(approx_eq(a.center.x, 1.0));
            assert!(approx_eq(a.center.y, 1.0));
            assert!(approx_eq(a.sweep_angle, std::f64::consts::FRAC_PI_2));
        } else { panic!("arc not an Arc") }
    }

    #[test]
    fn fillet_obtuse_120deg_arc_bulges_toward_corner() {
        // L1 along +X, L2 along 120° — an obtuse (θ = 120°) corner at the
        // origin. Regression for the arc-DIRECTION bug: the rounded corner
        // must bulge TOWARD the corner vertex I, not sweep out the far side.
        // The old `dot > 0` start-angle heuristic mis-fired here and rendered
        // the arc on the wrong side (only θ = 90° happened to be correct).
        use std::f64::consts::PI;
        let theta = 2.0 * PI / 3.0;                       // 120°
        let dir2  = Vec2::new(theta.cos(), theta.sin());
        let l1 = ln(0.0, 0.0, 10.0, 0.0);
        let l2 = Line { a: Vec2::new(0.0, 0.0), b: dir2 * 10.0 };
        let p1 = Vec2::new(8.0, 0.0);
        let p2 = dir2 * 8.0;
        let r  = 2.0;
        let out = fillet_lines(&l1, p1, &l2, p2, r).unwrap();

        // Expected geometry from first principles.
        let t      = r / (theta / 2.0).tan();
        let tp1    = Vec2::new(t, 0.0);
        let tp2    = dir2 * t;
        let bis    = Vec2::new(1.0, 0.0) + dir2;
        let center = bis / bis.len() * (r / (theta / 2.0).sin());
        let i_pt   = Vec2::new(0.0, 0.0);
        let mid_exp = center + (i_pt - center) / (i_pt - center).len() * r;

        let Geom::Arc(a) = out.arc.expect("expected an arc") else { panic!("not an arc") };
        let pt = |ang: f64| Vec2::new(
            a.center.x + a.radius * ang.cos(),
            a.center.y + a.radius * ang.sin());
        let start = pt(a.start_angle);
        let end   = pt(a.start_angle + a.sweep_angle);
        let mid   = pt(a.start_angle + a.sweep_angle * 0.5);
        let close = |p: Vec2, q: Vec2| approx_eq(p.x, q.x) && approx_eq(p.y, q.y);

        assert!(approx_eq(a.sweep_angle, PI - theta), "sweep should be π−θ (60°)");
        assert!(close(a.center, center), "center {:?} != {:?}", a.center, center);
        // Endpoints land on the two tangent points (either traversal order).
        assert!(
            (close(start, tp1) && close(end, tp2)) ||
            (close(start, tp2) && close(end, tp1)),
            "arc endpoints {:?},{:?} should be tangent pts {:?},{:?}", start, end, tp1, tp2);
        // Midpoint bulges toward the corner vertex I (the bug put it on the
        // opposite side of the circle).
        assert!(close(mid, mid_exp), "arc mid {:?} should bulge toward I ({:?})", mid, mid_exp);
    }

    #[test]
    fn fillet_radius_zero_makes_sharp_corner() {
        let l1 = ln(0.0, 0.0, 5.0, 0.0);
        let l2 = ln(2.0, -3.0, 2.0, 4.0);    // intersects l1 at (2, 0)
        let p1 = Vec2::new(4.5, 0.0);
        let p2 = Vec2::new(2.0, 3.0);
        let out = fillet_lines(&l1, p1, &l2, p2, 0.0).unwrap();
        assert!(out.arc.is_none());
        if let Geom::Line(l) = out.g1_new {
            assert!(approx_eq(l.b.x, 2.0));
            assert!(approx_eq(l.b.y, 0.0));
        } else { panic!() }
    }

    #[test]
    fn fillet_parallel_lines_errs() {
        let l1 = ln(0.0, 0.0, 5.0, 0.0);
        let l2 = ln(0.0, 1.0, 5.0, 1.0);
        let p1 = Vec2::new(2.0, 0.0);
        let p2 = Vec2::new(2.0, 1.0);
        assert!(fillet_lines(&l1, p1, &l2, p2, 1.0).is_err());
    }

    #[test]
    fn fillet_radius_too_large_errs() {
        let l1 = ln(0.0, 0.0, 1.0, 0.0);  // 1-long
        let l2 = ln(0.0, 0.0, 0.0, 1.0);
        let p1 = Vec2::new(0.5, 0.0);
        let p2 = Vec2::new(0.0, 0.5);
        // radius 10 needs tangent point at (10, 0) — way past line end.
        assert!(fillet_lines(&l1, p1, &l2, p2, 10.0).is_err());
    }

    // --- chamfer ----------------------------------------------------------

    #[test]
    fn chamfer_right_angle_d1_d2() {
        let l1 = ln(0.0, 0.0, 5.0, 0.0);
        let l2 = ln(0.0, 0.0, 0.0, 5.0);
        let p1 = Vec2::new(3.0, 0.0);
        let p2 = Vec2::new(0.0, 3.0);
        let out = chamfer_lines(&l1, p1, &l2, p2, 1.0, 2.0).unwrap();
        if let Geom::Line(l) = out.g1_new {
            assert!(approx_eq(l.b.x, 1.0));
            assert!(approx_eq(l.b.y, 0.0));
        } else { panic!() }
        if let Geom::Line(l) = out.g2_new {
            assert!(approx_eq(l.a.x, 0.0));
            assert!(approx_eq(l.a.y, 2.0));
        } else { panic!() }
        if let Geom::Line(l) = out.bridge {
            assert!(approx_eq(l.a.x, 1.0)); assert!(approx_eq(l.a.y, 0.0));
            assert!(approx_eq(l.b.x, 0.0)); assert!(approx_eq(l.b.y, 2.0));
        } else { panic!() }
    }

    // --- join: collinear lines -------------------------------------------

    #[test]
    fn join_two_touching_collinear_lines() {
        let items = vec![
            (3usize, Geom::Line(ln(0.0, 0.0, 2.0, 0.0))),
            (7usize, Geom::Line(ln(2.0, 0.0, 5.0, 0.0))),
        ];
        let out = join_geoms(&items);
        assert_eq!(out.consumed_indices.len(), 2);
        assert_eq!(out.merged.len(), 1);
        if let Geom::Line(l) = &out.merged[0] {
            assert!(approx_eq(l.a.x, 0.0));
            assert!(approx_eq(l.b.x, 5.0));
        } else { panic!() }
    }

    #[test]
    fn join_overlapping_collinear_lines() {
        let items = vec![
            (0usize, Geom::Line(ln(0.0, 0.0, 3.0, 0.0))),
            (1usize, Geom::Line(ln(2.0, 0.0, 5.0, 0.0))),
        ];
        let out = join_geoms(&items);
        assert_eq!(out.merged.len(), 1);
        if let Geom::Line(l) = &out.merged[0] {
            assert!(approx_eq(l.a.x, 0.0));
            assert!(approx_eq(l.b.x, 5.0));
        } else { panic!() }
    }

    #[test]
    fn polyline_bbox_and_distance_account_for_bulge() {
        // Two vertices with a semicircle bulge (bulge=1, CCW) from (0,0) to
        // (2,0): the arc apex reaches (1,-1). bbox must include y≈-1, and a
        // click on the apex must be near the polyline (not ~1 unit away at the
        // chord).
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 1.0 },
                PolyVertex { pos: Vec2::new(2.0, 0.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        };
        let (min, max) = pl.bbox();
        assert!(min.y <= -0.99, "bbox must include the arc apex, got min.y={}", min.y);
        assert!(max.y >= -0.01);
        // A point right on the arc apex (1,-1) should be ~0 from the polyline.
        let d = pl.distance_to_point(Vec2::new(1.0, -1.0));
        assert!(d < 1e-6, "apex should lie on the polyline, got dist={}", d);
        // The chord midpoint (1,0) is ~1 unit from the arc (NOT on it).
        let d_chord = pl.distance_to_point(Vec2::new(1.0, 0.0));
        assert!(d_chord > 0.9, "chord midpoint should be off the arc, got {}", d_chord);
    }

    #[test]
    fn join_does_not_bridge_collinear_gap() {
        // Two collinear stubs with a GAP between them (like a line trimmed at a
        // crossing). They must NOT merge into one line — that would redraw the
        // removed middle piece. With no other geometry, nothing chains.
        let items = vec![
            (0usize, Geom::Line(ln(0.0, 0.0, 2.0, 0.0))),
            (1usize, Geom::Line(ln(5.0, 0.0, 8.0, 0.0))),
        ];
        let out = join_geoms(&items);
        assert!(out.merged.is_empty(), "gapped collinear lines must not merge");
        assert!(out.consumed_indices.is_empty());
    }

    #[test]
    fn join_line_arc_line_with_collinear_stubs_makes_one_polyline() {
        // The trim-then-join case: two collinear line stubs that DON'T touch
        // each other, but each touches an arc that bridges the gap. Must yield
        // a single polyline (line→arc→line), NOT a straight line across.
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(2.0, 0.0);          // stub-left end / arc start
        let c = Vec2::new(4.0, 0.0);          // arc end / stub-right start
        let d = Vec2::new(6.0, 0.0);
        // Semicircle bulging up from b to c (center (3,0), r=1).
        let (center, r, start, sweep) = bulge_arc(b, c, 1.0).unwrap();
        let items = vec![
            (0usize, Geom::Line(Line { a, b })),
            (1usize, Geom::Line(Line { a: c, b: d })),
            (2usize, Geom::Arc(Arc { center, radius: r,
                                     start_angle: start, sweep_angle: sweep })),
        ];
        let out = join_geoms(&items);
        assert_eq!(out.consumed_indices.len(), 3, "all three pieces should chain");
        assert_eq!(out.merged.len(), 1);
        assert!(matches!(out.merged[0], Geom::Polyline(_)),
                "result must be one polyline, not a bridged straight line");
    }

    #[test]
    fn bulge_from_arc_sign_major_and_minor() {
        let c = Vec2::ZERO;
        let q = std::f64::consts::FRAC_PI_2;          // 90°
        let three_q = 3.0 * q;                        // 270°
        let s = Vec2::new(1.0, 0.0);
        // Minor CCW: start (1,0) → end (0,1), 90° → positive bulge tan(22.5°).
        let b_minor = bulge_from_arc(s, Vec2::new(0.0, 1.0), c, q);
        assert!(b_minor > 0.0 && approx_eq(b_minor, (q * 0.25).tan()));
        // Major CCW: start (1,0) → end (0,-1), 270° → STILL positive (CCW),
        // magnitude tan(67.5°). The old chord-side rule got this NEGATIVE.
        let b_major = bulge_from_arc(s, Vec2::new(0.0, -1.0), c, three_q);
        assert!(b_major > 0.0 && approx_eq(b_major, (three_q * 0.25).tan()));
        // Reverse traversal of the minor arc is CW → negative.
        let b_rev = bulge_from_arc(Vec2::new(0.0, 1.0), s, c, q);
        assert!(b_rev < 0.0);
    }

    // --- join: concentric arcs -------------------------------------------

    #[test]
    fn join_two_touching_arcs_same_center_radius() {
        let a1 = Arc { center: Vec2::ZERO, radius: 1.0,
                       start_angle: 0.0, sweep_angle: std::f64::consts::FRAC_PI_2 };
        let a2 = Arc { center: Vec2::ZERO, radius: 1.0,
                       start_angle: std::f64::consts::FRAC_PI_2,
                       sweep_angle: std::f64::consts::FRAC_PI_2 };
        let items = vec![(0usize, Geom::Arc(a1)), (1usize, Geom::Arc(a2))];
        let out = join_geoms(&items);
        assert_eq!(out.merged.len(), 1);
        if let Geom::Arc(a) = &out.merged[0] {
            assert!(approx_eq(a.sweep_angle, std::f64::consts::PI));
            assert!(approx_eq(a.radius, 1.0));
        } else { panic!() }
    }

    // --- join: chain → polyline ------------------------------------------

    #[test]
    fn join_line_arc_line_chain_to_polyline() {
        let items = vec![
            (0, Geom::Line(ln(0.0, 0.0, 1.0, 0.0))),
            (1, Geom::Arc(Arc {
                center: Vec2::new(1.0, 1.0), radius: 1.0,
                start_angle: -std::f64::consts::FRAC_PI_2,
                sweep_angle: std::f64::consts::FRAC_PI_2,
            })),
            (2, Geom::Line(ln(2.0, 1.0, 2.0, 3.0))),
        ];
        let out = join_geoms(&items);
        // The collinear / arc-group passes shouldn't fire (only 1 line
        // direction; 1 arc). Chain pass should produce a polyline.
        assert!(out.merged.iter().any(|g| matches!(g, Geom::Polyline(_))));
    }

    // --- trim: closed Ellipse -------------------------------------------

    #[test]
    fn trim_ellipse_horizontal_cut_keeps_lower_half() {
        // a = 2, b = 1; cut by y = 0 → intersections at (±2, 0) (t = 0, π).
        // Click on upper half → upper EllipseArc dropped; lower survives.
        let el = Ellipse {
            center: Vec2::ZERO,
            major:  Vec2::new(2.0, 0.0),
            ratio:  0.5,
        };
        let g      = Geom::Ellipse(el);
        let cutter = Geom::Line(Line {
            a: Vec2::new(-3.0, 0.0),
            b: Vec2::new( 3.0, 0.0),
        });
        let pieces = g.trim_at(&[cutter], Vec2::new(0.0, 0.5), false).unwrap();
        assert_eq!(pieces.len(), 1, "expected one surviving EllipseArc");
        if let Geom::EllipseArc(ea) = &pieces[0] {
            assert!((ea.start_param - std::f64::consts::PI).abs() < 1e-4);
            assert!((ea.sweep_param - std::f64::consts::PI).abs() < 1e-4);
        } else {
            panic!("expected EllipseArc, got {:?}", pieces[0]);
        }
    }

    #[test]
    fn trim_ellipse_three_cuts_drops_clicked_arc() {
        // Three cuts → 3 arcs. Click inside one → 2 survive.
        let el = Ellipse {
            center: Vec2::ZERO,
            major:  Vec2::new(3.0, 0.0),
            ratio:  0.5,
        };
        let g = Geom::Ellipse(el);
        // Three horizontal/diagonal lines through the ellipse, picked so
        // each crosses the curve twice; total 6 intersections, dedup'd by
        // sort+dedup gives 6 distinct params → 6 arcs of small sweep,
        // not 3. Adjust: pick lines that share a common pair of points.
        // Easier: cut twice by parallel lines y=±0.4 — 4 intersections,
        // four sub-arcs.
        let c1 = Geom::Line(Line { a: Vec2::new(-4.0,  0.4), b: Vec2::new(4.0,  0.4) });
        let c2 = Geom::Line(Line { a: Vec2::new(-4.0, -0.4), b: Vec2::new(4.0, -0.4) });
        let pieces = g.trim_at(&[c1, c2], Vec2::new(0.0, 1.0), false).unwrap();
        // Click is on the top arc (between y=0.4 cuts at the top); 3 survive.
        assert_eq!(pieces.len(), 3);
        for p in &pieces {
            assert!(matches!(p, Geom::EllipseArc(_)));
        }
    }

    // --- offset: ellipse / EllipseArc → Polyline approximation --------

    // --- trim: Polyline (explode + trim) ---------------------------------

    // --- grips: per-role semantics ---------------------------------------

    #[test]
    fn grip_line_endpoint_moves_only_that_end() {
        let g = Geom::Line(ln(0.0, 0.0, 10.0, 0.0));
        let out = g.with_grip_moved(GripRole::LineEndA, Vec2::new(2.0, 5.0));
        if let Geom::Line(l) = out {
            assert!(approx_eq(l.a.x, 2.0)); assert!(approx_eq(l.a.y, 5.0));
            assert!(approx_eq(l.b.x, 10.0)); assert!(approx_eq(l.b.y, 0.0));
        } else { panic!(); }
    }

    #[test]
    fn grip_line_midpoint_translates_whole_line() {
        let g = Geom::Line(ln(0.0, 0.0, 10.0, 0.0));
        // Original midpoint = (5,0). Move to (5,3) → delta (0,3).
        let out = g.with_grip_moved(GripRole::LineMid, Vec2::new(5.0, 3.0));
        if let Geom::Line(l) = out {
            assert!(approx_eq(l.a.y, 3.0));
            assert!(approx_eq(l.b.y, 3.0));
            assert!(approx_eq(l.a.x, 0.0));
            assert!(approx_eq(l.b.x, 10.0));
        } else { panic!(); }
    }

    #[test]
    fn grip_circle_quadrant_changes_radius() {
        let g = Geom::Circle(Circle { center: Vec2::ZERO, radius: 1.0 });
        // Drag a quadrant to (3, 4) → new radius = 5.
        let out = g.with_grip_moved(GripRole::CircleQuadrant, Vec2::new(3.0, 4.0));
        if let Geom::Circle(c) = out {
            assert!(approx_eq(c.center.x, 0.0));
            assert!(approx_eq(c.center.y, 0.0));
            assert!(approx_eq(c.radius, 5.0));
        } else { panic!(); }
    }

    #[test]
    fn grip_circle_center_translates() {
        let g = Geom::Circle(Circle { center: Vec2::ZERO, radius: 2.0 });
        let out = g.with_grip_moved(GripRole::CircleCenter, Vec2::new(7.0, 8.0));
        if let Geom::Circle(c) = out {
            assert!(approx_eq(c.center.x, 7.0));
            assert!(approx_eq(c.center.y, 8.0));
            assert!(approx_eq(c.radius, 2.0));
        } else { panic!(); }
    }

    #[test]
    fn grip_polyline_vertex_moves_only_that_vertex() {
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 4.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        };
        let g = Geom::Polyline(pl);
        let out = g.with_grip_moved(GripRole::PolyVertex(1), Vec2::new(4.0, -2.0));
        if let Geom::Polyline(p) = out {
            assert!(approx_eq(p.vertices[0].pos.y, 0.0));
            assert!(approx_eq(p.vertices[1].pos.y, -2.0));
            assert!(approx_eq(p.vertices[2].pos.y, 4.0));
        } else { panic!(); }
    }

    #[test]
    fn trim_polyline_segment_drops_clicked_sub_segment() {
        // Open polyline: (0,0) → (4,0) → (4,4). A vertical line at x=2
        // crosses segment 0 (horizontal) at (2,0). Click between the cut
        // and (4,0) → that sub-segment is dropped. Other segments survive.
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 4.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        };
        let g = Geom::Polyline(pl);
        let cutter = Geom::Line(Line {
            a: Vec2::new(2.0, -1.0), b: Vec2::new(2.0, 5.0),
        });
        let pieces = g.trim_at(&[cutter], Vec2::new(3.0, 0.0), false).unwrap();
        // Open polylines now keep CONNECTED runs: the cut splits this into two
        // polylines — (0,0)→(2,0) and (4,0)→(4,4) — instead of exploding to
        // bare Lines, so the surviving structure stays a polyline.
        assert_eq!(pieces.len(), 2);
        for p in &pieces { assert!(matches!(p, Geom::Polyline(_))); }
        // First run starts at (0,0) and ends at the cut (2,0); second is the
        // untouched vertical leg.
        let ends: Vec<(Vec2, Vec2)> = pieces.iter().map(|g| {
            if let Geom::Polyline(pl) = g {
                (pl.vertices.first().unwrap().pos, pl.vertices.last().unwrap().pos)
            } else { unreachable!() }
        }).collect();
        assert!(ends.iter().any(|&(a, b)|
            approx_eq(a.x, 0.0) && approx_eq(a.y, 0.0) && approx_eq(b.x, 2.0) && approx_eq(b.y, 0.0)));
        assert!(ends.iter().any(|&(a, b)|
            approx_eq(a.x, 4.0) && approx_eq(a.y, 0.0) && approx_eq(b.x, 4.0) && approx_eq(b.y, 4.0)));
    }

    #[test]
    fn dimension_picks_on_the_dim_line_not_only_def_points() {
        // Aligned linear dim: def points (0,0)→(10,0), dim line offset down to
        // y=-3. The visible dim line runs (0,-3)→(10,-3). A click at its midpoint
        // (5,-3) — far from every def point — must hit-test as ~0 distance.
        use crate::dim::{Dim, DimKind, LinearOrtho};
        let d = Geom::Dimension(Dim {
            kind: DimKind::Linear {
                p1: Vec2::new(0.0, 0.0),
                p2: Vec2::new(10.0, 0.0),
                dimline_pos: Vec2::new(0.0, -3.0),
                ortho: LinearOrtho::Aligned,
            },
            style: 0,
            text_override: None,
        });
        // On the dim line, mid-span — previously INFINITY-ish (nearest def
        // point is (0,0)/(10,0) ~5.8 away); now must be ~0.
        assert!(d.distance_to_point(Vec2::new(5.0, -3.0)) < 1e-6);
        // On an extension line (x=10 from y=0 to y=-3) at its midpoint.
        assert!(d.distance_to_point(Vec2::new(10.0, -1.5)) < 1e-6);
        // Well away from everything → large distance (still selectable only by
        // window). Sanity that we didn't make everything "hit".
        assert!(d.distance_to_point(Vec2::new(5.0, 20.0)) > 5.0);
    }

    #[test]
    fn intersect_line_polyline_returns_per_segment_hits() {
        use crate::intersect::intersect;
        let pl = Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(4.0, 4.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        };
        let g = Geom::Polyline(pl);
        // Diagonal line crosses both segments.
        let line = Geom::Line(Line {
            a: Vec2::new(-1.0, -1.0), b: Vec2::new(5.0, 5.0),
        });
        let hits = intersect(&g, &line);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn offset_ellipse_outward_grows_bbox() {
        // Outward offset: pick `side` outside the ellipse. The resulting
        // polyline should be a closed loop whose bbox is wider than the
        // original.
        let el = Ellipse {
            center: Vec2::ZERO,
            major:  Vec2::new(2.0, 0.0),
            ratio:  0.5,
        };
        let g = Geom::Ellipse(el);
        let out = g.offset(0.5, Vec2::new(10.0, 0.0)).unwrap();
        if let Geom::Polyline(p) = out {
            assert!(p.closed, "outward ellipse offset must be a closed polyline");
            assert!(p.vertices.len() >= 48);
            // Every offset vertex should sit outside the original ellipse —
            // the parametric form satisfies x²/a² + y²/b² >= 1.
            for v in &p.vertices {
                let val = (v.pos.x / 2.0).powi(2) + (v.pos.y / 1.0).powi(2);
                assert!(val > 1.0,
                    "offset vertex {:?} lies inside original ellipse (val={})",
                    v.pos, val);
            }
        } else {
            panic!("expected Polyline, got {:?}", out);
        }
    }

    #[test]
    fn offset_ellipse_inward_shrinks_inside_bbox() {
        // Inward offset: pick `side` inside. Every vertex sits inside the
        // original ellipse.
        let el = Ellipse {
            center: Vec2::ZERO,
            major:  Vec2::new(2.0, 0.0),
            ratio:  0.5,
        };
        let g = Geom::Ellipse(el);
        let out = g.offset(0.3, Vec2::new(0.0, 0.0)).unwrap();
        if let Geom::Polyline(p) = out {
            assert!(p.closed);
            for v in &p.vertices {
                let val = (v.pos.x / 2.0).powi(2) + (v.pos.y / 1.0).powi(2);
                assert!(val < 1.0,
                    "offset vertex {:?} lies outside original ellipse (val={})",
                    v.pos, val);
            }
        } else {
            panic!("expected Polyline, got {:?}", out);
        }
    }

    #[test]
    fn offset_ellipse_arc_returns_open_polyline() {
        // Half-ellipse arc (top): start_param=0, sweep=π.
        let ea = EllipseArc {
            ellipse: Ellipse {
                center: Vec2::ZERO,
                major:  Vec2::new(2.0, 0.0),
                ratio:  0.5,
            },
            start_param: 0.0,
            sweep_param: std::f64::consts::PI,
        };
        let g = Geom::EllipseArc(ea);
        let out = g.offset(0.2, Vec2::new(0.0, 5.0)).unwrap();
        if let Geom::Polyline(p) = out {
            assert!(!p.closed, "ellipse arc offset must be an OPEN polyline");
            assert!(p.vertices.len() >= 49);  // n+1 samples
        } else {
            panic!("expected Polyline, got {:?}", out);
        }
    }

    #[test]
    fn trim_ellipse_single_tangent_intersection_errs() {
        let el = Ellipse {
            center: Vec2::ZERO,
            major:  Vec2::new(2.0, 0.0),
            ratio:  0.5,
        };
        let g = Geom::Ellipse(el);
        // Tangent at the top: y = 1. Touches at one point (0, 1).
        let tangent = Geom::Line(Line {
            a: Vec2::new(-3.0, 1.0),
            b: Vec2::new( 3.0, 1.0),
        });
        let err = g.trim_at(&[tangent], Vec2::new(0.0, 0.5), false).unwrap_err();
        assert!(err.contains("at least 2 intersections")
             || err.contains("no intersection"));
    }

    // ---- Wall ----------------------------------------------------------
    #[test]
    fn wall_translated_moves_centerline_keeps_thickness() {
        let w = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0),
                       thickness: 2.0, style: 0, bulge: 0.0 };
        let g = Geom::Wall(w).translated(Vec2::new(5.0, 3.0));
        if let Geom::Wall(w2) = g {
            assert_eq!(w2.start, Vec2::new(5.0, 3.0));
            assert_eq!(w2.end,   Vec2::new(15.0, 3.0));
            assert_eq!(w2.thickness, 2.0);
        } else { panic!("translated lost variant"); }
    }

    #[test]
    fn wall_scaled_scales_thickness() {
        let w = Wall { start: Vec2::ZERO, end: Vec2::new(4.0, 0.0), thickness: 1.0, style: 0, bulge: 0.0 };
        let g = Geom::Wall(w).scaled(Vec2::ZERO, 2.5);
        if let Geom::Wall(w2) = g {
            assert!((w2.end.x - 10.0).abs() < 1e-12);
            assert!((w2.thickness - 2.5).abs() < 1e-12);
        } else { panic!("scaled lost variant"); }
    }

    #[test]
    fn wall_rotated_90_swaps_axes() {
        let w = Wall { start: Vec2::ZERO, end: Vec2::new(5.0, 0.0), thickness: 1.0, style: 0, bulge: 0.0 };
        let g = Geom::Wall(w).rotated(Vec2::ZERO, std::f64::consts::FRAC_PI_2);
        if let Geom::Wall(w2) = g {
            assert!((w2.end - Vec2::new(0.0, 5.0)).len() < 1e-9);
            assert_eq!(w2.thickness, 1.0);
        } else { panic!("rotated lost variant"); }
    }

    #[test]
    fn wall_distance_to_point_picks_nearer_side() {
        // Horizontal wall along the X-axis, thickness 2 → sides at y=±1.
        // Point at (5, 0.3) is distance 0.7 from the upper side and 1.3 from the lower.
        let w = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0),
                       thickness: 2.0, style: 0, bulge: 0.0 };
        let d = Geom::Wall(w).distance_to_point(Vec2::new(5.0, 0.3));
        assert!((d - 0.7).abs() < 1e-9);
    }

    #[test]
    fn wall_bbox_includes_thickness() {
        let w = Wall { start: Vec2::ZERO, end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let (min, max) = Geom::Wall(w).bbox();
        // Loose bbox: expanded by thk/2 = 1.0 in both axes.
        assert!((min.y + 1.0).abs() < 1e-9);
        assert!((max.y - 1.0).abs() < 1e-9);
    }

    #[test]
    fn arc_bbox_is_tight_not_full_circle() {
        // 45°→135° top-cap arc, r=5 at origin. Tight bbox must hug the cap,
        // NOT return the full circle (that broke window selection).
        let a = Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: std::f64::consts::FRAC_PI_4,
            sweep_angle: std::f64::consts::FRAC_PI_2,
        };
        let (min, max) = Geom::Arc(a).bbox();
        assert!((max.y - 5.0).abs() < 1e-6, "top cardinal swept → max.y=5, got {}", max.y);
        assert!(min.y > 3.0, "bottom NOT swept → min.y ~3.54 (not -5), got {}", min.y);
        assert!((max.x - 3.5355).abs() < 1e-3, "max.x from endpoint, got {}", max.x);
        assert!((min.x + 3.5355).abs() < 1e-3, "min.x from endpoint, got {}", min.x);
    }

    #[test]
    fn curved_wall_faces_meet_tangent_straight_wall_exactly() {
        // Straight wall along +x ending at (10,0); tangent CCW quarter-arc
        // wall (10,0) → (15,5), centre (10,5), r=5 (bulge = tan(90°/4)).
        // This is exactly the configuration `fillet r>0` on two walls
        // produces. The curved wall's FIRST face points must coincide with
        // the straight wall's END face points — the old chord-normal
        // offsetting left a ≈(t/2)·sweep/(2·steps) gap here.
        let b = (std::f64::consts::FRAC_PI_2 / 4.0).tan();
        let s = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0),
                       thickness: 0.3, style: 0, bulge: 0.0 };
        let c = Wall { start: Vec2::new(10.0, 0.0), end: Vec2::new(15.0, 5.0),
                       thickness: 0.3, style: 0, bulge: b };
        let (sl, sr) = s.face_polylines(1).unwrap();
        let (cl, cr) = c.face_polylines(28).unwrap();
        let gap_l = (sl[1] - cl[0]).len();
        let gap_r = (sr[1] - cr[0]).len();
        assert!(gap_l < 1e-9, "left-face gap at tangent joint: {}", gap_l);
        assert!(gap_r < 1e-9, "right-face gap at tangent joint: {}", gap_r);
        // Faces must be true concentric arcs: inner radius 5−0.15,
        // outer 5+0.15, for every sample.
        let centre = Vec2::new(10.0, 5.0);
        for p in &cl { assert!(((  *p - centre).len() - 4.85).abs() < 1e-9); }
        for p in &cr { assert!(((  *p - centre).len() - 5.15).abs() < 1e-9); }
    }

    #[test]
    fn bulge_roundtrip_reconstructs_arc() {
        // CCW quarter arc, radius 5, centre origin: (5,0) → (0,5), sweep 90°.
        let center = Vec2::new(0.0, 0.0);
        let (start, end) = (Vec2::new(5.0, 0.0), Vec2::new(0.0, 5.0));
        let sweep = std::f64::consts::FRAC_PI_2;
        let b = bulge_from_arc(start, end, center, sweep);
        let (c2, r2, _a0, sw) = bulge_arc(start, end, b).expect("arc");
        assert!((c2 - center).len() < 1e-6, "centre {:?}", c2);
        assert!((r2 - 5.0).abs() < 1e-6, "radius {}", r2);
        assert!((sw - sweep).abs() < 1e-6, "sweep {}", sw);
    }
}
