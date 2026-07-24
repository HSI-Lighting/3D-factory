//! 2D draw tools for sketches — the sandbox's drafting layer, mirroring simLUX's
//! `app.rs` Paradigm-A pipeline (a `tool` + a `pending: Vec<Vec2>` click accumulator
//! + a count-based finalize), enriched with per-shape CONSTRUCTION METHODS (Circle
//! center-radius/diameter/2P/3P; Arc 3-point/start-center-end/center-start-end;
//! Ellipse center-major-minor / axis-endpoints) and a POLYLINE that follows
//! `PLINE_GUIDE.md`: Line/Arc modes, tangent-continuous bulges, C=close, U=undo,
//! auto-close on the first vertex, commit-on-interrupt.
//!
//! Geometry MATH is reused verbatim from `cad_kernel` (primitive structs + the pure
//! constructors `arc_three_points` / `arc_center_start_end` /
//! `ellipse_center_major_minor` + the bulge form `PolyVertex{bulge}`). Only the
//! interaction is recreated here so a later merge with the app's draw layer is 1:1.
//! Picks arrive as glam `Vec2` in the sketch frame's `(u,v)`.

use cad_kernel::{
    arc_center_start_end, arc_three_points, bulge_from_arc, ellipse_center_major_minor, Circle, Geom,
    Line, Point, PolyVertex, Polyline, Vec2 as KVec2,
};
use glam::Vec2;

/// The active 2D draw tool. Mirrors the app's `Tool` (basic-drawing subset).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DrawTool {
    None,
    Line,
    Polyline,
    Rectangle,
    Circle,
    Arc,
    Ellipse,
    Point,
}

impl DrawTool {
    pub const ALL: [DrawTool; 7] = [
        DrawTool::Line,
        DrawTool::Polyline,
        DrawTool::Rectangle,
        DrawTool::Circle,
        DrawTool::Arc,
        DrawTool::Ellipse,
        DrawTool::Point,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DrawTool::None => "—",
            DrawTool::Line => "Line",
            DrawTool::Polyline => "Pline",
            DrawTool::Rectangle => "Rect",
            DrawTool::Circle => "Circle",
            DrawTool::Arc => "Arc",
            DrawTool::Ellipse => "Ellipse",
            DrawTool::Point => "Point",
        }
    }
}

/// Polyline segment mode — the PLINE `A`/`L` sub-commands (Arc = bulged, tangent-
/// continuous; Line = straight). Affects the NEXT segment. (PLINE_GUIDE §5/§7.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlineMode {
    Line,
    Arc,
}

/// The PLINE 3-point-arc sub-flow (`S`/second): pick an on-arc point, then the
/// endpoint → bulge from the three points (PLINE_GUIDE §4/§7).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PlineArcSub {
    Normal,
    AwaitingOnArc,
    AwaitingEnd(Vec2), // holds the on-arc point
}

/// Circle construction methods (mirrors the app's `CircleStep` variants).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CircleMethod {
    CenterRadius,
    Diameter,
    TwoPoint,
    ThreePoint,
}

impl CircleMethod {
    pub const ALL: [CircleMethod; 4] =
        [CircleMethod::CenterRadius, CircleMethod::Diameter, CircleMethod::TwoPoint, CircleMethod::ThreePoint];
    pub fn label(self) -> &'static str {
        match self {
            CircleMethod::CenterRadius => "Center, Radius",
            CircleMethod::Diameter => "Center, Diameter",
            CircleMethod::TwoPoint => "2 Point",
            CircleMethod::ThreePoint => "3 Point",
        }
    }
    pub fn count(self) -> usize {
        match self {
            CircleMethod::ThreePoint => 3,
            _ => 2,
        }
    }
}

/// Arc construction methods (the app's implemented `ArcMethod` set).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ArcMethod {
    ThreePoint,
    StartCenterEnd,
    CenterStartEnd,
}

impl ArcMethod {
    pub const ALL: [ArcMethod; 3] =
        [ArcMethod::ThreePoint, ArcMethod::StartCenterEnd, ArcMethod::CenterStartEnd];
    pub fn label(self) -> &'static str {
        match self {
            ArcMethod::ThreePoint => "3 Point",
            ArcMethod::StartCenterEnd => "Start, Center, End",
            ArcMethod::CenterStartEnd => "Center, Start, End",
        }
    }
}

/// Ellipse construction methods.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EllipseMethod {
    CenterMajorMinor,
    AxisEnds,
}

impl EllipseMethod {
    pub const ALL: [EllipseMethod; 2] = [EllipseMethod::CenterMajorMinor, EllipseMethod::AxisEnds];
    pub fn label(self) -> &'static str {
        match self {
            EllipseMethod::CenterMajorMinor => "Center, Major, Minor",
            EllipseMethod::AxisEnds => "Axis ends, Minor",
        }
    }
}

/// Outcome of feeding a typed OPTION (not a coordinate) to an active draw.
pub enum CmdOutcome {
    Committed(Geom),
    Consumed,
}

fn circle_method_from(t: &str) -> Option<CircleMethod> {
    match t {
        "2p" => Some(CircleMethod::TwoPoint),
        "3p" => Some(CircleMethod::ThreePoint),
        "d" | "dia" | "diameter" => Some(CircleMethod::Diameter),
        "r" | "radius" | "cr" => Some(CircleMethod::CenterRadius),
        _ => None,
    }
}

fn arc_method_from(t: &str) -> Option<ArcMethod> {
    match t {
        "3p" => Some(ArcMethod::ThreePoint),
        "sce" | "s" => Some(ArcMethod::StartCenterEnd),
        "cse" | "ce" => Some(ArcMethod::CenterStartEnd),
        _ => None,
    }
}

fn ellipse_method_from(t: &str) -> Option<EllipseMethod> {
    match t {
        "c" | "center" | "cmm" => Some(EllipseMethod::CenterMajorMinor),
        "a" | "axis" | "ax" => Some(EllipseMethod::AxisEnds),
        _ => None,
    }
}

#[inline]
fn kv(p: Vec2) -> KVec2 {
    KVec2::new(p.x as f64, p.y as f64)
}

/// Rotate a 2D vector by `ang` radians.
fn rot2(v: Vec2, ang: f32) -> Vec2 {
    let (s, c) = ang.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// The polyline bulge (`tan(sweep/4)`) for a segment `a→b` given the desired start
/// TANGENT: `alpha` = signed angle(tangent → chord), `bulge = tan(alpha/2)`. So arcs
/// are G1 tangent-continuous. (PLINE_GUIDE §7.)
fn pline_bulge(a: Vec2, b: Vec2, tangent: Vec2) -> f64 {
    let chord = (b - a).normalize_or_zero();
    let t = tangent.normalize_or_zero();
    if chord.length_squared() < 1e-12 || t.length_squared() < 1e-12 {
        return 0.0;
    }
    let alpha = (t.x * chord.y - t.y * chord.x).atan2(t.x * chord.x + t.y * chord.y);
    (alpha as f64 * 0.5).tan()
}

/// Polyline bulge for a 3-point arc (start `a`, on-arc `m`, end `b`) — via the kernel
/// circle-through-3-points then `bulge_from_arc` (sign-correct for major arcs).
fn bulge_from_three_points(a: Vec2, m: Vec2, b: Vec2) -> f64 {
    match arc_three_points(kv(a), kv(m), kv(b)) {
        Some(arc) => bulge_from_arc(kv(a), kv(b), arc.center, arc.sweep_angle.abs()),
        None => 0.0,
    }
}

/// An in-progress draw command: a tool + method selectors + the picked points.
#[derive(Clone)]
pub struct Draw {
    pub tool: DrawTool,
    pub pending: Vec<Vec2>,
    /// Polyline bulge per segment i→i+1 (`len == pending.len()-1`). 0 = straight.
    pub pl_bulges: Vec<f64>,
    pub pline_mode: PlineMode,
    pub pl_arc_sub: PlineArcSub,
    pub circle_method: CircleMethod,
    pub arc_method: ArcMethod,
    pub ellipse_method: EllipseMethod,
}

impl Default for Draw {
    fn default() -> Self {
        Self {
            tool: DrawTool::None,
            pending: Vec::new(),
            pl_bulges: Vec::new(),
            pline_mode: PlineMode::Line,
            pl_arc_sub: PlineArcSub::Normal,
            circle_method: CircleMethod::CenterRadius,
            arc_method: ArcMethod::ThreePoint,
            ellipse_method: EllipseMethod::CenterMajorMinor,
        }
    }
}

impl Draw {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active(&self) -> bool {
        self.tool != DrawTool::None
    }

    /// Switch tool (clears any half-drawn entity), like picking a draw command.
    pub fn set_tool(&mut self, t: DrawTool) {
        self.tool = t;
        self.pending.clear();
        self.pl_bulges.clear();
        self.pline_mode = PlineMode::Line;
        self.pl_arc_sub = PlineArcSub::Normal;
    }

    /// The first picked point (base) — for a "you started here" marker.
    pub fn first_point(&self) -> Option<Vec2> {
        self.pending.first().copied()
    }

    fn needed(&self) -> usize {
        match self.tool {
            DrawTool::Line | DrawTool::Rectangle => 2,
            DrawTool::Circle => self.circle_method.count(),
            DrawTool::Arc => 3,
            DrawTool::Ellipse => 3,
            DrawTool::Point => 1,
            DrawTool::Polyline | DrawTool::None => usize::MAX,
        }
    }

    /// The exit tangent at the last placed vertex — for tangent-continuous arcs.
    /// From the previous segment's bulge; `+X` if there is no previous segment.
    fn pline_prev_tangent(&self) -> Vec2 {
        let n = self.pending.len();
        if n >= 2 {
            let chord = (self.pending[n - 1] - self.pending[n - 2]).normalize_or_zero();
            let prev_bulge = self.pl_bulges.last().copied().unwrap_or(0.0);
            if prev_bulge.abs() < 1e-9 {
                chord
            } else {
                rot2(chord, 2.0 * (prev_bulge as f32).atan())
            }
        } else {
            Vec2::new(1.0, 0.0)
        }
    }

    /// Feed a pick (frame `u,v`). Returns a completed `Geom` to commit, or `None`
    /// while the entity still needs more points. Chains where the app chains.
    pub fn feed(&mut self, uv: Vec2) -> Option<Geom> {
        // POLYLINE: accumulate vertices; compute a bulge per segment (arc mode + the
        // 3-point-arc `S` sub-flow).
        if self.tool == DrawTool::Polyline {
            match self.pl_arc_sub {
                PlineArcSub::AwaitingOnArc => {
                    self.pl_arc_sub = PlineArcSub::AwaitingEnd(uv); // stored on-arc point
                    return None;
                }
                PlineArcSub::AwaitingEnd(mid) => {
                    if let Some(&start) = self.pending.last() {
                        self.pl_bulges.push(bulge_from_three_points(start, mid, uv));
                    }
                    self.pending.push(uv);
                    self.pl_arc_sub = PlineArcSub::Normal;
                    return None;
                }
                PlineArcSub::Normal => {
                    if let Some(&last) = self.pending.last() {
                        let bulge = match self.pline_mode {
                            PlineMode::Arc => pline_bulge(last, uv, self.pline_prev_tangent()),
                            PlineMode::Line => 0.0,
                        };
                        self.pl_bulges.push(bulge);
                    }
                    self.pending.push(uv);
                    return None; // Enter / C / interrupt commits
                }
            }
        }
        // LINE: continuous — each 2nd point commits a segment and re-arms from it.
        if self.tool == DrawTool::Line && self.pending.is_empty() {
            self.pending.push(uv);
            return None;
        }
        if self.tool == DrawTool::Line {
            let a = self.pending[0];
            self.pending = vec![uv];
            return Some(Geom::Line(Line { a: kv(a), b: kv(uv) }));
        }
        if self.tool == DrawTool::None {
            return None;
        }
        self.pending.push(uv);
        if self.pending.len() >= self.needed() {
            let g = build(self.tool, self.circle_method, self.arc_method, self.ellipse_method, &self.pending);
            self.pending.clear();
            return g;
        }
        None
    }

    /// Provisional geometry for the live rubber-band: the shape as if `cursor` were
    /// the next click (polyline previews the arc rubber-band in arc mode).
    pub fn preview(&self, cursor: Vec2) -> Vec<Geom> {
        match self.tool {
            DrawTool::None | DrawTool::Point => Vec::new(),
            DrawTool::Polyline => {
                if self.pending.is_empty() {
                    return Vec::new();
                }
                let last = *self.pending.last().unwrap();
                let cursor_bulge = match self.pl_arc_sub {
                    PlineArcSub::AwaitingEnd(mid) => bulge_from_three_points(last, mid, cursor),
                    PlineArcSub::AwaitingOnArc => 0.0,
                    PlineArcSub::Normal => match self.pline_mode {
                        PlineMode::Arc => pline_bulge(last, cursor, self.pline_prev_tangent()),
                        PlineMode::Line => 0.0,
                    },
                };
                let mut vertices: Vec<PolyVertex> = self
                    .pending
                    .iter()
                    .enumerate()
                    .map(|(i, p)| PolyVertex {
                        pos: kv(*p),
                        bulge: self.pl_bulges.get(i).copied().unwrap_or(cursor_bulge),
                    })
                    .collect();
                vertices.push(PolyVertex { pos: kv(cursor), bulge: 0.0 });
                vec![Geom::Polyline(Polyline { vertices, closed: false, widths: Vec::new() })]
            }
            _ => {
                let mut p = self.pending.clone();
                p.push(cursor);
                if p.len() >= self.needed() {
                    let start = p.len() - self.needed();
                    build(self.tool, self.circle_method, self.arc_method, self.ellipse_method, &p[start..])
                        .into_iter()
                        .collect()
                } else {
                    p.windows(2).map(|w| Geom::Line(Line { a: kv(w[0]), b: kv(w[1]) })).collect()
                }
            }
        }
    }

    /// Enter: commit an in-progress polyline (≥2 pts) as an OPEN polyline.
    pub fn finish(&mut self) -> Option<Geom> {
        let out = self.polyline_geom(false);
        self.reset_pending();
        out
    }

    /// C: commit the polyline as a CLOSED loop (≥3 pts).
    pub fn close(&mut self) -> Option<Geom> {
        let out = if self.tool == DrawTool::Polyline && self.pending.len() >= 3 {
            self.polyline_geom(true)
        } else {
            None
        };
        self.reset_pending();
        out
    }

    /// Build the polyline geom from the picked vertices + their per-segment bulges.
    fn polyline_geom(&self, closed: bool) -> Option<Geom> {
        if self.tool != DrawTool::Polyline || self.pending.len() < 2 {
            return None;
        }
        let vertices = self
            .pending
            .iter()
            .enumerate()
            .map(|(i, p)| PolyVertex { pos: kv(*p), bulge: self.pl_bulges.get(i).copied().unwrap_or(0.0) })
            .collect();
        Some(Geom::Polyline(Polyline { vertices, closed, widths: Vec::new() }))
    }

    fn reset_pending(&mut self) {
        self.pending.clear();
        self.pl_bulges.clear();
        self.pline_mode = PlineMode::Line;
        self.pl_arc_sub = PlineArcSub::Normal;
    }

    /// U: drop the last placed vertex (and its segment bulge).
    pub fn undo_point(&mut self) {
        self.pending.pop();
        self.pl_bulges.pop();
    }

    /// Esc: remove the LAST placed vertex (one per press); when none remain, exit the
    /// tool (returns true = handled, stay in the sketch). Returns false only when
    /// there is nothing to cancel (no tool, no vertices) so the caller leaves the sketch.
    pub fn cancel(&mut self) -> bool {
        // an active 3-point-arc sub-flow cancels first (keeps the vertices)
        if self.pl_arc_sub != PlineArcSub::Normal {
            self.pl_arc_sub = PlineArcSub::Normal;
            return true;
        }
        if !self.pending.is_empty() {
            self.pending.pop();
            self.pl_bulges.pop();
            return true;
        }
        if self.tool != DrawTool::None {
            self.tool = DrawTool::None;
            return true;
        }
        false
    }

    /// Start a draw tool from a typed verb (+ optional method token).
    pub fn start_verb(&mut self, raw: &str) -> bool {
        let low = raw.trim().to_lowercase();
        let mut parts = low.split_whitespace();
        let verb = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("");
        let tool = match verb {
            "line" | "l" => DrawTool::Line,
            "pline" | "pl" | "polyline" | "pline2d" => DrawTool::Polyline,
            "rect" | "rec" | "rectangle" => DrawTool::Rectangle,
            "circle" | "c" | "ci" => DrawTool::Circle,
            "arc" | "a" => DrawTool::Arc,
            "ellipse" | "el" | "ellip" => DrawTool::Ellipse,
            "point" | "po" => DrawTool::Point,
            _ => return false,
        };
        self.set_tool(tool);
        match tool {
            DrawTool::Circle => {
                if let Some(m) = circle_method_from(arg) {
                    self.circle_method = m;
                }
            }
            DrawTool::Arc => {
                if let Some(m) = arc_method_from(arg) {
                    self.arc_method = m;
                }
            }
            DrawTool::Ellipse => {
                if let Some(m) = ellipse_method_from(arg) {
                    self.ellipse_method = m;
                }
            }
            _ => {}
        }
        true
    }

    /// Consume a typed OPTION for the active tool (letters, not coordinates). For a
    /// POLYLINE these are the PLINE sub-commands — intercepted BEFORE the verb parser
    /// so `a`/`l`/`c`/`u` mean arc-mode / line-mode / close / undo, NOT the global
    /// Arc/Line/Copy/Undo (PLINE_GUIDE §5). Returns `None` if not an option.
    pub fn option(&mut self, text: &str) -> Option<CmdOutcome> {
        let t = text.trim().to_lowercase();
        match self.tool {
            DrawTool::Polyline => match t.as_str() {
                "a" | "arc" => {
                    self.pline_mode = PlineMode::Arc;
                    Some(CmdOutcome::Consumed)
                }
                "l" | "line" => {
                    self.pline_mode = PlineMode::Line;
                    self.pl_arc_sub = PlineArcSub::Normal;
                    Some(CmdOutcome::Consumed)
                }
                // S = 3-point arc: next two picks are the on-arc point and the endpoint.
                "s" | "second" => {
                    if !self.pending.is_empty() {
                        self.pline_mode = PlineMode::Arc;
                        self.pl_arc_sub = PlineArcSub::AwaitingOnArc;
                    }
                    Some(CmdOutcome::Consumed)
                }
                "c" | "close" => Some(self.close().map(CmdOutcome::Committed).unwrap_or(CmdOutcome::Consumed)),
                "u" | "undo" => {
                    // cancel an active arc sub-flow first, else drop the last vertex
                    if self.pl_arc_sub != PlineArcSub::Normal {
                        self.pl_arc_sub = PlineArcSub::Normal;
                    } else {
                        self.undo_point();
                    }
                    Some(CmdOutcome::Consumed)
                }
                _ => None,
            },
            DrawTool::Circle => circle_method_from(&t).map(|m| {
                self.circle_method = m;
                self.pending.clear();
                CmdOutcome::Consumed
            }),
            DrawTool::Arc => arc_method_from(&t).map(|m| {
                self.arc_method = m;
                self.pending.clear();
                CmdOutcome::Consumed
            }),
            DrawTool::Ellipse => ellipse_method_from(&t).map(|m| {
                self.ellipse_method = m;
                self.pending.clear();
                CmdOutcome::Consumed
            }),
            _ => None,
        }
    }

    /// A short prompt for the current step (tool + method + how many picks so far).
    pub fn prompt(&self) -> String {
        let n = self.pending.len();
        match self.tool {
            DrawTool::None => "pick a draw tool".into(),
            DrawTool::Line if n == 0 => "line: pick start".into(),
            DrawTool::Line => "line: pick next point · Enter/Esc ends".into(),
            DrawTool::Polyline => match self.pl_arc_sub {
                PlineArcSub::AwaitingOnArc => "polyline arc: click POINT ON ARC".into(),
                PlineArcSub::AwaitingEnd(_) => "polyline arc: click ARC END".into(),
                PlineArcSub::Normal => {
                    let mode = if self.pline_mode == PlineMode::Arc { "Arc" } else { "Line" };
                    if n == 0 {
                        format!("polyline [{mode}]: pick start")
                    } else {
                        format!("polyline [{mode}]: pick next · A=arc L=line S=3pt-arc C=close U=undo · Enter=finish")
                    }
                }
            },
            DrawTool::Rectangle if n == 0 => "rect: pick first corner".into(),
            DrawTool::Rectangle => "rect: pick opposite corner".into(),
            DrawTool::Point => "point: pick location".into(),
            DrawTool::Circle => circle_prompt(self.circle_method, n),
            DrawTool::Arc => arc_prompt(self.arc_method, n),
            DrawTool::Ellipse => ellipse_prompt(self.ellipse_method, n),
        }
    }
}

// ── builders (shared by feed + preview) ──────────────────────────────────────

fn build(
    tool: DrawTool,
    cm: CircleMethod,
    am: ArcMethod,
    em: EllipseMethod,
    p: &[Vec2],
) -> Option<Geom> {
    match tool {
        DrawTool::Line => Some(Geom::Line(Line { a: kv(p[0]), b: kv(p[1]) })),
        DrawTool::Rectangle => Some(rect(p[0], p[1])),
        DrawTool::Point => Some(Geom::Point(Point { location: kv(p[0]), style: 0, size: 0.0 })),
        DrawTool::Circle => build_circle(cm, p),
        DrawTool::Arc => build_arc(am, p),
        DrawTool::Ellipse => build_ellipse(em, p),
        _ => None,
    }
}

fn build_circle(cm: CircleMethod, p: &[Vec2]) -> Option<Geom> {
    let c = match cm {
        CircleMethod::CenterRadius => Circle { center: kv(p[0]), radius: (p[1] - p[0]).length() as f64 },
        CircleMethod::Diameter => Circle { center: kv(p[0]), radius: ((p[1] - p[0]).length() * 0.5) as f64 },
        CircleMethod::TwoPoint => {
            let center = (p[0] + p[1]) * 0.5;
            Circle { center: kv(center), radius: ((p[1] - p[0]).length() * 0.5) as f64 }
        }
        CircleMethod::ThreePoint => {
            let a = arc_three_points(kv(p[0]), kv(p[1]), kv(p[2]))?;
            Circle { center: a.center, radius: a.radius }
        }
    };
    if c.radius < 1e-6 {
        return None;
    }
    Some(Geom::Circle(c))
}

fn build_arc(am: ArcMethod, p: &[Vec2]) -> Option<Geom> {
    let a = match am {
        ArcMethod::ThreePoint => arc_three_points(kv(p[0]), kv(p[1]), kv(p[2])),
        ArcMethod::StartCenterEnd => arc_center_start_end(kv(p[1]), kv(p[0]), kv(p[2])),
        ArcMethod::CenterStartEnd => arc_center_start_end(kv(p[0]), kv(p[1]), kv(p[2])),
    };
    a.map(Geom::Arc)
}

fn build_ellipse(em: EllipseMethod, p: &[Vec2]) -> Option<Geom> {
    let (center, major_end, side) = match em {
        EllipseMethod::CenterMajorMinor => (p[0], p[1], p[2]),
        EllipseMethod::AxisEnds => ((p[0] + p[1]) * 0.5, p[1], p[2]),
    };
    let major = major_end - center;
    if major.length() < 1e-6 {
        return None;
    }
    let dir = major.normalize();
    let d = side - center;
    let semi_minor = (d - dir * d.dot(dir)).length();
    ellipse_center_major_minor(kv(center), kv(major_end), semi_minor as f64).map(Geom::Ellipse)
}

/// Rectangle → a CLOSED 4-vertex polyline (the app's `rect_polyline`).
fn rect(a: Vec2, b: Vec2) -> Geom {
    let v = |x: f32, y: f32| PolyVertex { pos: KVec2::new(x as f64, y as f64), bulge: 0.0 };
    Geom::Polyline(Polyline {
        vertices: vec![v(a.x, a.y), v(b.x, a.y), v(b.x, b.y), v(a.x, b.y)],
        closed: true,
        widths: Vec::new(),
    })
}

fn circle_prompt(cm: CircleMethod, n: usize) -> String {
    match (cm, n) {
        (CircleMethod::CenterRadius, 0) => "circle: pick centre",
        (CircleMethod::CenterRadius, _) => "circle: pick radius",
        (CircleMethod::Diameter, 0) => "circle: pick centre",
        (CircleMethod::Diameter, _) => "circle: pick diameter point",
        (CircleMethod::TwoPoint, 0) => "circle 2P: first diameter point",
        (CircleMethod::TwoPoint, _) => "circle 2P: opposite diameter point",
        (CircleMethod::ThreePoint, 0) => "circle 3P: first point",
        (CircleMethod::ThreePoint, 1) => "circle 3P: second point",
        (CircleMethod::ThreePoint, _) => "circle 3P: third point",
    }
    .to_string()
}

fn arc_prompt(am: ArcMethod, n: usize) -> String {
    match (am, n) {
        (ArcMethod::ThreePoint, 0) => "arc 3P: start",
        (ArcMethod::ThreePoint, 1) => "arc 3P: point on arc",
        (ArcMethod::ThreePoint, _) => "arc 3P: end",
        (ArcMethod::StartCenterEnd, 0) => "arc: start point",
        (ArcMethod::StartCenterEnd, 1) => "arc: centre",
        (ArcMethod::StartCenterEnd, _) => "arc: end point",
        (ArcMethod::CenterStartEnd, 0) => "arc: centre",
        (ArcMethod::CenterStartEnd, 1) => "arc: start point",
        (ArcMethod::CenterStartEnd, _) => "arc: end point",
    }
    .to_string()
}

fn ellipse_prompt(em: EllipseMethod, n: usize) -> String {
    match (em, n) {
        (EllipseMethod::CenterMajorMinor, 0) => "ellipse: centre",
        (EllipseMethod::CenterMajorMinor, 1) => "ellipse: major-axis end",
        (EllipseMethod::CenterMajorMinor, _) => "ellipse: minor distance",
        (EllipseMethod::AxisEnds, 0) => "ellipse: first axis end",
        (EllipseMethod::AxisEnds, 1) => "ellipse: second axis end",
        (EllipseMethod::AxisEnds, _) => "ellipse: minor distance",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_chains_from_last_point() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Line);
        assert!(d.feed(Vec2::new(0.0, 0.0)).is_none());
        let g = d.feed(Vec2::new(1.0, 0.0)).expect("segment committed");
        assert!(matches!(g, Geom::Line(_)));
        assert_eq!(d.pending.len(), 1);
        assert!((d.pending[0] - Vec2::new(1.0, 0.0)).length() < 1e-6);
    }

    #[test]
    fn circle_center_radius() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Circle);
        d.circle_method = CircleMethod::CenterRadius;
        d.feed(Vec2::ZERO);
        match d.feed(Vec2::new(3.0, 4.0)).unwrap() {
            Geom::Circle(c) => assert!((c.radius - 5.0).abs() < 1e-6),
            _ => panic!("expected circle"),
        }
    }

    #[test]
    fn circle_two_point_diameter() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Circle);
        d.circle_method = CircleMethod::TwoPoint;
        d.feed(Vec2::new(-2.0, 0.0));
        match d.feed(Vec2::new(2.0, 0.0)).unwrap() {
            Geom::Circle(c) => {
                assert!((c.radius - 2.0).abs() < 1e-6, "r={}", c.radius);
                assert!((c.center.x).abs() < 1e-6 && (c.center.y).abs() < 1e-6);
            }
            _ => panic!("expected circle"),
        }
    }

    #[test]
    fn circle_three_point() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Circle);
        d.circle_method = CircleMethod::ThreePoint;
        d.feed(Vec2::new(1.0, 0.0));
        d.feed(Vec2::new(0.0, 1.0));
        match d.feed(Vec2::new(-1.0, 0.0)).unwrap() {
            Geom::Circle(c) => {
                assert!((c.radius - 1.0).abs() < 1e-6);
                assert!(c.center.x.abs() < 1e-6 && c.center.y.abs() < 1e-6);
            }
            _ => panic!("expected circle"),
        }
    }

    #[test]
    fn arc_start_center_end() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Arc);
        d.arc_method = ArcMethod::StartCenterEnd;
        d.feed(Vec2::new(1.0, 0.0));
        d.feed(Vec2::ZERO);
        let g = d.feed(Vec2::new(0.0, 1.0));
        assert!(matches!(g, Some(Geom::Arc(_))));
    }

    #[test]
    fn ellipse_axis_ends() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Ellipse);
        d.ellipse_method = EllipseMethod::AxisEnds;
        d.feed(Vec2::new(-2.0, 0.0));
        d.feed(Vec2::new(2.0, 0.0));
        let g = d.feed(Vec2::new(0.0, 1.0));
        assert!(matches!(g, Some(Geom::Ellipse(_))));
    }

    #[test]
    fn polyline_close_makes_closed_loop() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Polyline);
        d.feed(Vec2::ZERO);
        d.feed(Vec2::new(2.0, 0.0));
        d.feed(Vec2::new(2.0, 2.0));
        match d.close().unwrap() {
            Geom::Polyline(p) => {
                assert_eq!(p.vertices.len(), 3);
                assert!(p.closed);
            }
            _ => panic!("expected closed polyline"),
        }
    }

    #[test]
    fn polyline_arc_mode_gives_bulged_segment() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Polyline);
        d.feed(Vec2::ZERO);
        d.feed(Vec2::new(2.0, 0.0)); // first straight segment (bulge 0)
        // switch to arc mode; the NEXT segment should bulge (tangent-continuous)
        assert!(matches!(d.option("a"), Some(CmdOutcome::Consumed)));
        assert_eq!(d.pline_mode, PlineMode::Arc);
        d.feed(Vec2::new(4.0, 2.0));
        match d.finish().unwrap() {
            Geom::Polyline(p) => {
                assert_eq!(p.vertices.len(), 3);
                // segment[1] (index 1) leaves vertex 1 as an arc → non-zero bulge
                assert!(p.vertices[1].bulge.abs() > 1e-6, "arc segment should bulge, got {}", p.vertices[1].bulge);
                assert!(p.vertices[0].bulge.abs() < 1e-9, "first segment stays straight");
            }
            _ => panic!("expected polyline"),
        }
    }

    #[test]
    fn polyline_undo_drops_last_vertex() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Polyline);
        d.feed(Vec2::ZERO);
        d.feed(Vec2::new(1.0, 0.0));
        d.feed(Vec2::new(2.0, 0.0));
        assert_eq!(d.pending.len(), 3);
        d.undo_point();
        assert_eq!(d.pending.len(), 2);
        assert_eq!(d.pl_bulges.len(), 1);
    }

    #[test]
    fn escape_removes_last_vertex_then_exits_tool() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Polyline);
        d.feed(Vec2::ZERO);
        d.feed(Vec2::new(1.0, 0.0));
        assert!(d.cancel()); // remove (1,0)
        assert_eq!(d.pending.len(), 1);
        assert!(d.cancel()); // remove (0,0)
        assert_eq!(d.pending.len(), 0);
        assert!(d.cancel()); // exit tool
        assert_eq!(d.tool, DrawTool::None);
        assert!(!d.cancel()); // nothing left → caller leaves sketch
    }

    #[test]
    fn command_line_starts_tool_with_method() {
        let mut d = Draw::new();
        assert!(d.start_verb("circle 3p"));
        assert!(matches!(d.tool, DrawTool::Circle));
        assert!(matches!(d.circle_method, CircleMethod::ThreePoint));
        assert!(d.start_verb("arc sce"));
        assert!(matches!(d.arc_method, ArcMethod::StartCenterEnd));
        assert!(!d.start_verb("frobnicate"));
    }

    #[test]
    fn pline_s_three_point_arc() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Polyline);
        d.feed(Vec2::ZERO); // start vertex
        assert!(matches!(d.option("s"), Some(CmdOutcome::Consumed)));
        assert_eq!(d.pl_arc_sub, PlineArcSub::AwaitingOnArc);
        d.feed(Vec2::new(1.0, 1.0)); // on-arc point → AwaitingEnd (NOT a vertex)
        assert!(matches!(d.pl_arc_sub, PlineArcSub::AwaitingEnd(_)));
        assert_eq!(d.pending.len(), 1);
        d.feed(Vec2::new(2.0, 0.0)); // endpoint → vertex pushed with 3-pt bulge
        assert_eq!(d.pl_arc_sub, PlineArcSub::Normal);
        assert_eq!(d.pending.len(), 2);
        match d.finish().unwrap() {
            Geom::Polyline(p) => assert!(p.vertices[0].bulge.abs() > 1e-6, "3pt arc bulge, got {}", p.vertices[0].bulge),
            _ => panic!("expected polyline"),
        }
    }

    #[test]
    fn pline_a_is_arc_mode_not_arc_tool() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Polyline);
        d.feed(Vec2::ZERO);
        // 'a' while drawing a pline must NOT start the Arc tool — it's arc MODE.
        assert!(matches!(d.option("a"), Some(CmdOutcome::Consumed)));
        assert_eq!(d.tool, DrawTool::Polyline);
        assert_eq!(d.pline_mode, PlineMode::Arc);
    }

    #[test]
    fn rectangle_is_a_closed_quad() {
        let mut d = Draw::new();
        d.set_tool(DrawTool::Rectangle);
        d.feed(Vec2::ZERO);
        match d.feed(Vec2::new(2.0, 1.0)).unwrap() {
            Geom::Polyline(p) => {
                assert_eq!(p.vertices.len(), 4);
                assert!(p.closed);
            }
            _ => panic!("expected polyline"),
        }
    }
}
