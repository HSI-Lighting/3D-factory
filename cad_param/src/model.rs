//! The parametric sketch data model — points, scalars, lines, circles, constraints.
//!
//! A `Sketch` is parameterised by a FLAT unknown vector laid out as
//! `[p0.x, p0.y, p1.x, p1.y, … , s0, s1, …]`: every point contributes two
//! unknowns (x/y) and every scalar contributes one (a radius, etc.). Lines and
//! circles reference points/scalars by id; the solver moves the unknowns so
//! every constraint's residual goes to zero.
//!
//! This generalisation (scalars as first-class unknowns, like SolveSpace /
//! planegcs treat all parameters in one vector) is what lets us constrain a
//! circle's radius, tangency, etc. — not just point coordinates. This is
//! `cad_param`'s OWN structure; it is not the kernel `Document`.

use cad_kernel::Vec2;

pub type PointId = usize;
pub type LineId = usize;
pub type CircleId = usize;
pub type ScalarId = usize;

/// A line segment defined by two point ids (a `cad_param` line, not a kernel one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub a: PointId,
    pub b: PointId,
}

/// A circle: a center point and a radius scalar (both are solver unknowns).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub center: PointId,
    pub radius: ScalarId,
}

/// A geometric constraint. Each contributes one or more residual equations the
/// solver drives to zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Constraint {
    // ---- point / position ----
    /// Pin a point to a fixed world location (anchor). 2 residuals.
    Fixed { p: PointId, x: f64, y: f64 },
    /// Two points coincide. 2 residuals.
    Coincident { p: PointId, q: PointId },
    /// Distance between two points equals `d`. 1 residual.
    Distance { p: PointId, q: PointId, d: f64 },
    /// A point lies on a line (infinite line through the segment). 1 residual.
    PointOnLine { p: PointId, line: LineId },
    /// Two points are symmetric about a line. 2 residuals.
    Symmetric { p: PointId, q: PointId, line: LineId },

    // ---- line / direction ----
    /// A line is horizontal (endpoints share y). 1 residual.
    Horizontal { line: LineId },
    /// A line is vertical (endpoints share x). 1 residual.
    Vertical { line: LineId },
    /// Two lines are parallel (direction cross-product = 0). 1 residual.
    Parallel { a: LineId, b: LineId },
    /// Two lines are perpendicular (direction dot-product = 0). 1 residual.
    Perpendicular { a: LineId, b: LineId },
    /// Two lines lie on the same infinite line (parallel + offset 0). 2 residuals.
    Collinear { a: LineId, b: LineId },
    /// Two lines have equal length. 1 residual.
    EqualLength { a: LineId, b: LineId },
    /// Signed angle from line `a` to line `b` equals `radians`. 1 residual.
    Angle { a: LineId, b: LineId, radians: f64 },

    // ---- circle ----
    /// A circle's radius equals `r`. 1 residual.
    Radius { circle: CircleId, r: f64 },
    /// Two circles share a center. 2 residuals.
    Concentric { a: CircleId, b: CircleId },
    /// Two circles have equal radius. 1 residual.
    EqualRadius { a: CircleId, b: CircleId },
    /// A point lies on a circle. 1 residual.
    PointOnCircle { p: PointId, circle: CircleId },
    /// A line is tangent to a circle (center-to-line distance = radius). 1 residual.
    TangentLineCircle { line: LineId, circle: CircleId },
    /// Two circles are tangent. `internal` = inner tangency (|r₁−r₂|) vs outer
    /// (r₁+r₂). 1 residual.
    TangentCircleCircle { a: CircleId, b: CircleId, internal: bool },
}

impl Constraint {
    /// How many residual equations this constraint contributes.
    pub fn residual_count(&self) -> usize {
        match self {
            Constraint::Fixed { .. }
            | Constraint::Coincident { .. }
            | Constraint::Symmetric { .. }
            | Constraint::Collinear { .. }
            | Constraint::Concentric { .. } => 2,
            _ => 1,
        }
    }
}

/// A parametric sketch: points + scalars (the unknowns), lines + circles, and
/// constraints.
#[derive(Clone, Debug, Default)]
pub struct Sketch {
    pub points: Vec<Vec2>,
    pub scalars: Vec<f64>,
    pub lines: Vec<Line>,
    pub circles: Vec<Circle>,
    pub constraints: Vec<Constraint>,
}

impl Sketch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_point(&mut self, x: f64, y: f64) -> PointId {
        self.points.push(Vec2::new(x, y));
        self.points.len() - 1
    }

    pub fn add_scalar(&mut self, v: f64) -> ScalarId {
        self.scalars.push(v);
        self.scalars.len() - 1
    }

    pub fn add_line(&mut self, a: PointId, b: PointId) -> LineId {
        self.lines.push(Line { a, b });
        self.lines.len() - 1
    }

    /// Add a circle from a center point id and a radius scalar id.
    pub fn add_circle(&mut self, center: PointId, radius: ScalarId) -> CircleId {
        self.circles.push(Circle { center, radius });
        self.circles.len() - 1
    }

    /// Convenience: add a circle from raw center coords + radius, creating the
    /// backing point and scalar. Returns the circle id.
    pub fn add_circle_xy(&mut self, cx: f64, cy: f64, r: f64) -> CircleId {
        let c = self.add_point(cx, cy);
        let s = self.add_scalar(r);
        self.add_circle(c, s)
    }

    pub fn add(&mut self, c: Constraint) {
        self.constraints.push(c);
    }

    /// Number of solver unknowns = 2·points + scalars.
    pub fn param_count(&self) -> usize {
        2 * self.points.len() + self.scalars.len()
    }

    /// Flat index of a point's x coordinate in the unknown vector.
    #[inline]
    pub fn point_x_index(&self, p: PointId) -> usize {
        2 * p
    }

    /// Flat index of a scalar in the unknown vector (scalars follow all points).
    #[inline]
    pub fn scalar_index(&self, s: ScalarId) -> usize {
        2 * self.points.len() + s
    }

    /// Total residual equations (the height of the system the solver builds).
    pub fn residual_dim(&self) -> usize {
        self.constraints.iter().map(|c| c.residual_count()).sum()
    }

    /// Naive degrees of freedom = unknowns − residual equations. This OVER-counts
    /// when constraints are redundant; use [`crate::solve::dof_analysis`] for the
    /// rank-honest figure that drives the blue/black "fully defined" indicator.
    pub fn dof(&self) -> i64 {
        self.param_count() as i64 - self.residual_dim() as i64
    }
}
