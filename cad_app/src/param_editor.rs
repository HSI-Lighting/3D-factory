//! Parametric MODE glue — lets the app's normal tools draw into a drawing that
//! the `cad_param` constraint solver then governs. We deliberately reuse ALL the
//! existing tools/modifiers (Line, Circle, trim, snaps, …); this module only
//! adds a constraint LAYER + a Solve step + the "fully defined" diagnosis.
//! `cad_param` stays the isolated, swappable solver backend (kept separate for
//! safety / dev / library-testing); the core kernel `Document`/`Geom` are not
//! modified.
//!
//! Constraints reference drawing geometry by stable HANDLE, resolved to sketch
//! ids at solve time. LINE and CIRCLE dobjects are constrainable; coincident
//! line endpoints are auto-merged into one sketch point so connected lines move
//! together. Point-level constraints (Coincident/Symmetric/PointOnLine/
//! PointOnCircle) exist in the solver but need endpoint-picking UI (a later
//! slice), so they are not yet exposed here.

use cad_kernel::{dobject::Handle, Document, Geom, Vec2};
use cad_param::{dof_analysis, solve, Constraint, DofReport, Sketch, VarTable};
use std::collections::{HashMap, HashSet};

/// A user constraint, referencing drawing geometry by stable handle. Mixed
/// (line/circle) refs like `Tangent` are disambiguated by geometry kind at solve
/// time.
#[derive(Clone, Copy, Debug)]
pub enum CRef {
    // line / direction
    Horizontal(Handle),
    Vertical(Handle),
    Parallel(Handle, Handle),
    Perpendicular(Handle, Handle),
    Collinear(Handle, Handle),
    Equal(Handle, Handle),
    Angle(Handle, Handle, f64),
    Length(Handle, f64),
    // circle
    Radius(Handle, f64),
    Concentric(Handle, Handle),
    EqualRadius(Handle, Handle),
    Tangent(Handle, Handle), // line↔circle or circle↔circle
}

impl CRef {
    pub fn label(&self) -> String {
        match self {
            CRef::Horizontal(_) => "horizontal".into(),
            CRef::Vertical(_) => "vertical".into(),
            CRef::Parallel(..) => "parallel".into(),
            CRef::Perpendicular(..) => "perpendicular".into(),
            CRef::Collinear(..) => "collinear".into(),
            CRef::Equal(..) => "equal length".into(),
            CRef::Angle(_, _, d) => format!("angle = {:.3}°", d.to_degrees()),
            CRef::Length(_, d) => format!("length = {d}"),
            CRef::Radius(_, r) => format!("radius = {r}"),
            CRef::Concentric(..) => "concentric".into(),
            CRef::EqualRadius(..) => "equal radius".into(),
            CRef::Tangent(..) => "tangent".into(),
        }
    }

    /// Every drawing handle this constraint references (for pruning constraints
    /// whose geometry has been deleted).
    pub fn handles(&self) -> Vec<Handle> {
        match *self {
            CRef::Horizontal(h) | CRef::Vertical(h) | CRef::Length(h, _) | CRef::Radius(h, _) => vec![h],
            CRef::Parallel(a, b)
            | CRef::Perpendicular(a, b)
            | CRef::Collinear(a, b)
            | CRef::Equal(a, b)
            | CRef::Angle(a, b, _)
            | CRef::Concentric(a, b)
            | CRef::EqualRadius(a, b)
            | CRef::Tangent(a, b) => vec![a, b],
        }
    }
}

/// A human-readable trace of one solve — the "session recorder inside the
/// parametric window": the selected geometry, the math (per-constraint residual,
/// rms before/after, iterations), and where it went. Shown in the diagnostics
/// panel and logged to the global recorder.
#[derive(Clone, Default)]
pub struct SolveTrace {
    pub lines: Vec<String>,
}

/// Outcome of [`solve_doc`] — message, convergence (the UI rolls a just-added
/// constraint back when it does not), and the diagnostic trace.
pub struct SolveOutcome {
    pub msg: String,
    pub converged: bool,
    pub trace: SolveTrace,
}

/// Remove constraints that reference geometry no longer in the drawing (handles
/// from deleted/replaced dobjects). Without this, constraints from earlier edits
/// linger forever and make every solve look like it "applies to everything".
/// Returns how many were dropped.
pub fn prune_constraints(doc: &Document, constraints: &mut Vec<CRef>) -> usize {
    let present: HashSet<Handle> = doc.dobjects.iter().map(|d| d.handle).collect();
    let before = constraints.len();
    constraints.retain(|c| c.handles().iter().all(|h| present.contains(h)));
    before - constraints.len()
}

/// A two-entity constraint that's been started with a REFERENCE entity and is
/// waiting for the user to pick the TARGET (the "select one, choose Parallel,
/// then pick the other" flow). The kind decides how the pair becomes a `CRef`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PendingKind {
    Parallel,
    Perpendicular,
    Collinear,
    Equal,
    Concentric,
    EqualRadius,
    Tangent,
}

impl PendingKind {
    pub fn label(&self) -> &'static str {
        match self {
            PendingKind::Parallel => "Parallel",
            PendingKind::Perpendicular => "Perpendicular",
            PendingKind::Collinear => "Collinear",
            PendingKind::Equal => "Equal length",
            PendingKind::Concentric => "Concentric",
            PendingKind::EqualRadius => "Equal radius",
            PendingKind::Tangent => "Tangent",
        }
    }
    /// True for kinds whose target is a circle (concentric / equal-radius).
    pub fn target_is_circle(&self) -> bool {
        matches!(self, PendingKind::Concentric | PendingKind::EqualRadius)
    }
    /// Build the constraint from the reference + target handles.
    pub fn to_cref(&self, first: Handle, second: Handle) -> CRef {
        match self {
            PendingKind::Parallel => CRef::Parallel(first, second),
            PendingKind::Perpendicular => CRef::Perpendicular(first, second),
            PendingKind::Collinear => CRef::Collinear(first, second),
            PendingKind::Equal => CRef::Equal(first, second),
            PendingKind::Concentric => CRef::Concentric(first, second),
            PendingKind::EqualRadius => CRef::EqualRadius(first, second),
            PendingKind::Tangent => CRef::Tangent(first, second),
        }
    }
}

/// Per-session parametric state held by the app. `active` gates the panel +
/// behaviour. Constraints and variables live here, NOT in the core Document.
pub struct ParamSession {
    pub active: bool,
    pub constraints: Vec<CRef>,
    /// A reference→target constraint waiting for its second pick: `(kind, first)`.
    pub pending: Option<(PendingKind, Handle)>,
    pub vars: VarTable,
    pub status: String,
    pub length_input: String,
    pub value_input: String,
    pub angle_input: String,
    pub new_var_name: String,
    pub new_var_expr: String,
    /// Show the blue/black under-defined overlay on the canvas.
    pub show_dof: bool,
    /// When true, edits auto-re-solve so constraint links are always kept live
    /// ("rotate one → the linked one follows"). See `geom_signature`.
    pub keep_link: bool,
    /// Geometry signature at the last solve — an edit that changes it triggers a
    /// driven re-solve.
    pub last_solved_sig: u64,
    /// Cached per-handle "fully defined" flags (true = black/locked). Recomputed
    /// by [`analyze_doc`] each frame the panel runs; the canvas overlay reads it.
    pub defined: HashMap<Handle, bool>,
    /// Cached degrees-of-freedom for the readout.
    pub dof: i64,
    pub fully_defined: bool,
    pub redundant: bool,
    /// Last solve's diagnostic trace (shown in the "Solver diagnostics" section).
    pub last_trace: SolveTrace,
    /// Whether the diagnostics section is expanded.
    pub show_trace: bool,
    /// Whether the per-entity math inspector is expanded.
    pub show_inspect: bool,
}

impl ParamSession {
    pub fn new() -> Self {
        Self {
            active: false,
            constraints: Vec::new(),
            pending: None,
            vars: VarTable::new(),
            status: String::new(),
            length_input: "100".into(),
            value_input: "50".into(),
            angle_input: "90".into(),
            new_var_name: String::new(),
            new_var_expr: String::new(),
            show_dof: true,
            keep_link: true,
            last_solved_sig: 0,
            defined: HashMap::new(),
            dof: 0,
            fully_defined: false,
            redundant: false,
            last_trace: SolveTrace::default(),
            show_trace: false,
            show_inspect: false,
        }
    }

    /// Evaluate a numeric field that may be a literal or an expression
    /// (`=W/2 + 3`, leading `=` optional) against the variable table.
    pub fn eval_field(&self, text: &str) -> Result<f64, String> {
        let env = self.vars.resolve()?;
        let s = text.trim().strip_prefix('=').unwrap_or(text);
        cad_param::eval(s, &env)
    }
}

fn intern(pts: &mut Vec<Vec2>, p: Vec2) -> usize {
    const EPS: f64 = 1e-6;
    for (i, q) in pts.iter().enumerate() {
        if (*q - p).len() < EPS {
            return i;
        }
    }
    pts.push(p);
    pts.len() - 1
}

/// Everything needed to build the sketch from the drawing, resolve handle-based
/// constraints, write solved geometry back, and colour each entity by DOF.
struct DocMap {
    sk: Sketch,
    /// per collected line: (dobject idx, point a id, point b id)
    lines: Vec<(usize, usize, usize)>,
    /// per collected STRAIGHT wall: (dobject idx, start point id, end point id).
    /// A wall's centerline is treated exactly like a line for constraints.
    walls: Vec<(usize, usize, usize)>,
    /// per collected circle: (dobject idx, center point id, radius scalar id)
    circles: Vec<(usize, usize, usize)>,
    line_id: HashMap<Handle, usize>,   // handle → sketch line id (lines AND walls)
    circle_id: HashMap<Handle, usize>, // handle → sketch circle id
    /// handle → its parameter indices in the flat unknown vector (for colouring)
    handle_params: HashMap<Handle, Vec<usize>>,
}

/// True for a straight (non-curved) wall — its centerline is a plain segment.
fn straight_wall(g: &Geom) -> Option<(Vec2, Vec2)> {
    match g {
        Geom::Wall(w) if w.bulge.abs() < 1e-9 => Some((w.start, w.end)),
        _ => None,
    }
}

/// Build a `cad_param::Sketch` from the drawing's LINE, straight WALL, and CIRCLE
/// dobjects. A wall's centerline is modelled as a sketch line, so every line
/// constraint (H/V/∥/⊥/collinear/equal/length/angle) works on walls too.
/// Coincident endpoints / centers are merged into shared points (so a wall that
/// meets a line at a corner moves with it).
fn build_doc_map(doc: &Document) -> DocMap {
    // 1. collect geometry
    let mut raw_lines: Vec<(Handle, usize, Vec2, Vec2)> = Vec::new();
    let mut raw_walls: Vec<(Handle, usize, Vec2, Vec2)> = Vec::new();
    let mut raw_circles: Vec<(Handle, usize, Vec2, f64)> = Vec::new();
    for (idx, d) in doc.dobjects.iter().enumerate() {
        match &d.geom {
            Geom::Line(l) => raw_lines.push((d.handle, idx, l.a, l.b)),
            Geom::Circle(c) => raw_circles.push((d.handle, idx, c.center, c.radius)),
            _ => {
                if let Some((s, e)) = straight_wall(&d.geom) {
                    raw_walls.push((d.handle, idx, s, e));
                }
            }
        }
    }

    // 2. intern all points (line + wall endpoints + circle centers) FIRST, so
    //    scalar indices (which follow all points) are stable.
    let mut pts: Vec<Vec2> = Vec::new();
    let line_pt_ids: Vec<(usize, usize)> = raw_lines
        .iter()
        .map(|(_, _, a, b)| (intern(&mut pts, *a), intern(&mut pts, *b)))
        .collect();
    let wall_pt_ids: Vec<(usize, usize)> = raw_walls
        .iter()
        .map(|(_, _, a, b)| (intern(&mut pts, *a), intern(&mut pts, *b)))
        .collect();
    let circ_center_ids: Vec<usize> = raw_circles
        .iter()
        .map(|(_, _, c, _)| intern(&mut pts, *c))
        .collect();

    let mut sk = Sketch::new();
    for p in &pts {
        sk.add_point(p.x, p.y);
    }
    let np = sk.points.len();

    // 3. circle radii become scalars (after all points).
    let circ_scalar_ids: Vec<usize> = raw_circles
        .iter()
        .map(|(_, _, _, r)| sk.add_scalar(*r))
        .collect();

    // 4. build line + wall + circle entities and the lookup maps.
    let mut line_id = HashMap::new();
    let mut circle_id = HashMap::new();
    let mut handle_params: HashMap<Handle, Vec<usize>> = HashMap::new();
    let mut lines = Vec::with_capacity(raw_lines.len());
    let mut walls = Vec::with_capacity(raw_walls.len());
    let mut circles = Vec::with_capacity(raw_circles.len());

    for (k, (h, dobj, _, _)) in raw_lines.iter().enumerate() {
        let (pa, pb) = line_pt_ids[k];
        line_id.insert(*h, sk.add_line(pa, pb));
        lines.push((*dobj, pa, pb));
        handle_params.insert(*h, vec![2 * pa, 2 * pa + 1, 2 * pb, 2 * pb + 1]);
    }
    for (k, (h, dobj, _, _)) in raw_walls.iter().enumerate() {
        let (pa, pb) = wall_pt_ids[k];
        line_id.insert(*h, sk.add_line(pa, pb)); // a wall centerline IS a sketch line
        walls.push((*dobj, pa, pb));
        handle_params.insert(*h, vec![2 * pa, 2 * pa + 1, 2 * pb, 2 * pb + 1]);
    }
    for (k, (h, dobj, _, _)) in raw_circles.iter().enumerate() {
        let c = circ_center_ids[k];
        let s = circ_scalar_ids[k];
        circle_id.insert(*h, sk.add_circle(c, s));
        circles.push((*dobj, c, s));
        handle_params.insert(*h, vec![2 * c, 2 * c + 1, 2 * np + s]);
    }

    DocMap { sk, lines, walls, circles, line_id, circle_id, handle_params }
}

/// Translate the session's handle-based constraints into the sketch. Returns how
/// many resolved (an unresolved one means its geometry is gone). Anchors the
/// first point so the sketch can't float, and HARD-PINS every `driver` entity at
/// its current position so an edited/dragged entity stays put while the linked
/// geometry moves to satisfy the constraints ("keep the link").
fn apply_constraints(
    map: &mut DocMap,
    doc: &Document,
    session: &ParamSession,
    drivers: &HashSet<Handle>,
) -> usize {
    if let Some(p0) = map.sk.points.first().copied() {
        map.sk.add(Constraint::Fixed { p: 0, x: p0.x, y: p0.y });
    }
    // Pin driver entities (the ones the user just moved) — their points become
    // hard-fixed at their current spots, so the solver adjusts everything else.
    for h in drivers {
        if let Some(&l) = map.line_id.get(h) {
            let ln = map.sk.lines[l];
            let (pa, pb) = (map.sk.points[ln.a], map.sk.points[ln.b]);
            map.sk.add(Constraint::Fixed { p: ln.a, x: pa.x, y: pa.y });
            map.sk.add(Constraint::Fixed { p: ln.b, x: pb.x, y: pb.y });
        } else if let Some(&c) = map.circle_id.get(h) {
            let circ = map.sk.circles[c];
            let cen = map.sk.points[circ.center];
            map.sk.add(Constraint::Fixed { p: circ.center, x: cen.x, y: cen.y });
        }
    }
    let _ = doc; // doc kept for future kind lookups beyond the maps
    let mut resolved = 0usize;
    for c in &session.constraints {
        let added = match *c {
            CRef::Horizontal(h) => map.line_id.get(&h).map(|&l| Constraint::Horizontal { line: l }),
            CRef::Vertical(h) => map.line_id.get(&h).map(|&l| Constraint::Vertical { line: l }),
            CRef::Parallel(a, b) => pair(&map.line_id, a, b).map(|(la, lb)| Constraint::Parallel { a: la, b: lb }),
            CRef::Perpendicular(a, b) => pair(&map.line_id, a, b).map(|(la, lb)| Constraint::Perpendicular { a: la, b: lb }),
            CRef::Collinear(a, b) => pair(&map.line_id, a, b).map(|(la, lb)| Constraint::Collinear { a: la, b: lb }),
            CRef::Equal(a, b) => pair(&map.line_id, a, b).map(|(la, lb)| Constraint::EqualLength { a: la, b: lb }),
            CRef::Angle(a, b, d) => pair(&map.line_id, a, b).map(|(la, lb)| Constraint::Angle { a: la, b: lb, radians: d }),
            CRef::Length(h, d) => map.line_id.get(&h).map(|&l| {
                let ln = map.sk.lines[l];
                Constraint::Distance { p: ln.a, q: ln.b, d }
            }),
            CRef::Radius(h, r) => map.circle_id.get(&h).map(|&c| Constraint::Radius { circle: c, r }),
            CRef::Concentric(a, b) => pair(&map.circle_id, a, b).map(|(ca, cb)| Constraint::Concentric { a: ca, b: cb }),
            CRef::EqualRadius(a, b) => pair(&map.circle_id, a, b).map(|(ca, cb)| Constraint::EqualRadius { a: ca, b: cb }),
            CRef::Tangent(a, b) => {
                let (la, ca) = (map.line_id.get(&a).copied(), map.circle_id.get(&a).copied());
                let (lb, cb) = (map.line_id.get(&b).copied(), map.circle_id.get(&b).copied());
                match (la, ca, lb, cb) {
                    (Some(l), _, _, Some(c)) => Some(Constraint::TangentLineCircle { line: l, circle: c }),
                    (_, Some(c), Some(l), _) => Some(Constraint::TangentLineCircle { line: l, circle: c }),
                    (_, Some(x), _, Some(y)) => Some(Constraint::TangentCircleCircle { a: x, b: y, internal: false }),
                    _ => None,
                }
            }
        };
        if let Some(con) = added {
            map.sk.add(con);
            resolved += 1;
        }
    }
    resolved
}

fn pair(m: &HashMap<Handle, usize>, a: Handle, b: Handle) -> Option<(usize, usize)> {
    match (m.get(&a), m.get(&b)) {
        (Some(&x), Some(&y)) => Some((x, y)),
        _ => None,
    }
}

/// A stable hash of all constrainable geometry's coordinates — used to detect
/// when the user has edited the drawing (so the link can be re-enforced).
pub fn geom_signature(doc: &Document) -> u64 {
    fn mix(acc: &mut u64, x: f64) {
        *acc ^= x.to_bits();
        *acc = acc.wrapping_mul(0x100000001b3);
    }
    let mut acc: u64 = 0xcbf29ce484222325;
    for d in &doc.dobjects {
        match &d.geom {
            Geom::Line(l) => { mix(&mut acc, l.a.x); mix(&mut acc, l.a.y); mix(&mut acc, l.b.x); mix(&mut acc, l.b.y); }
            Geom::Circle(c) => { mix(&mut acc, c.center.x); mix(&mut acc, c.center.y); mix(&mut acc, c.radius); }
            _ => if let Some((s, e)) = straight_wall(&d.geom) {
                mix(&mut acc, s.x); mix(&mut acc, s.y); mix(&mut acc, e.x); mix(&mut acc, e.y);
            },
        }
    }
    acc
}

fn handle_line_ends(doc: &Document, h: Handle) -> Option<(Vec2, Vec2)> {
    doc.dobjects.iter().find(|d| d.handle == h).and_then(|d| match &d.geom {
        Geom::Line(l) => Some((l.a, l.b)),
        Geom::Wall(w) => Some((w.start, w.end)),
        _ => None,
    })
}
fn handle_circle(doc: &Document, h: Handle) -> Option<(Vec2, f64)> {
    doc.dobjects.iter().find(|d| d.handle == h).and_then(|d| match &d.geom {
        Geom::Circle(c) => Some((c.center, c.radius)),
        _ => None,
    })
}

/// Geometric error of one constraint on the CURRENT doc geometry — the "math"
/// readout for the diagnostics panel (so you can see which constraint is off).
fn cref_report(doc: &Document, c: &CRef) -> String {
    // signed angle a→b vs c→d in degrees
    let ang = |h1: Handle, h2: Handle| -> Option<f64> {
        let (a, b) = handle_line_ends(doc, h1)?;
        let (p, q) = handle_line_ends(doc, h2)?;
        let (u, v) = (b - a, q - p);
        Some((u.x * v.y - u.y * v.x).atan2(u.x * v.x + u.y * v.y).to_degrees())
    };
    match *c {
        CRef::Horizontal(h) => match handle_line_ends(doc, h) {
            Some((a, b)) => format!("horizontal: Δy = {:.4}", (a.y - b.y).abs()),
            None => "horizontal: (geometry gone)".into(),
        },
        CRef::Vertical(h) => match handle_line_ends(doc, h) {
            Some((a, b)) => format!("vertical: Δx = {:.4}", (a.x - b.x).abs()),
            None => "vertical: (geometry gone)".into(),
        },
        CRef::Parallel(a, b) => match ang(a, b) {
            Some(d) => { let dev = { let x = d.abs(); x.min(180.0 - x) }; format!("parallel: {:.4}° off", dev) }
            None => "parallel: (geometry gone)".into(),
        },
        CRef::Perpendicular(a, b) => match ang(a, b) {
            Some(d) => format!("perpendicular: {:.4}° off", (d.abs() - 90.0).abs()),
            None => "perpendicular: (geometry gone)".into(),
        },
        CRef::Collinear(a, b) => match ang(a, b) {
            Some(d) => { let dev = { let x = d.abs(); x.min(180.0 - x) }; format!("collinear: {:.4}° off", dev) }
            None => "collinear: (geometry gone)".into(),
        },
        CRef::Equal(a, b) => match (handle_line_ends(doc, a), handle_line_ends(doc, b)) {
            (Some((a0, a1)), Some((b0, b1))) => format!("equal len: Δ = {:.4}", ((a1 - a0).len() - (b1 - b0).len()).abs()),
            _ => "equal len: (geometry gone)".into(),
        },
        CRef::Angle(a, b, want) => match ang(a, b) {
            Some(d) => format!("angle: {:.4}° (want {:.2}°)", d, want.to_degrees()),
            None => "angle: (geometry gone)".into(),
        },
        CRef::Length(h, want) => match handle_line_ends(doc, h) {
            Some((a, b)) => format!("length: {:.3} (want {:.3})", (b - a).len(), want),
            None => "length: (geometry gone)".into(),
        },
        CRef::Radius(h, want) => match handle_circle(doc, h) {
            Some((_, r)) => format!("radius: {:.3} (want {:.3})", r, want),
            None => "radius: (geometry gone)".into(),
        },
        CRef::Concentric(a, b) => match (handle_circle(doc, a), handle_circle(doc, b)) {
            (Some((ca, _)), Some((cb, _))) => format!("concentric: centres {:.4} apart", (ca - cb).len()),
            _ => "concentric: (geometry gone)".into(),
        },
        CRef::EqualRadius(a, b) => match (handle_circle(doc, a), handle_circle(doc, b)) {
            (Some((_, ra)), Some((_, rb))) => format!("equal radius: Δ = {:.4}", (ra - rb).abs()),
            _ => "equal radius: (geometry gone)".into(),
        },
        CRef::Tangent(..) => "tangent".into(),
    }
}

/// One line per constrainable dobject — handle + geometry. Used to record the
/// INPUT and OUTPUT geometry in the solve trace.
fn doc_geom_lines(doc: &Document) -> Vec<String> {
    let fv = |p: Vec2| format!("({:.3}, {:.3})", p.x, p.y);
    let mut out = Vec::new();
    for d in &doc.dobjects {
        match &d.geom {
            Geom::Line(l) => out.push(format!("  h={:?} line   {} → {}  len={:.3}", d.handle, fv(l.a), fv(l.b), (l.b - l.a).len())),
            Geom::Circle(c) => out.push(format!("  h={:?} circle c={} r={:.3}", d.handle, fv(c.center), c.radius)),
            _ => if let Some((s, e)) = straight_wall(&d.geom) {
                out.push(format!("  h={:?} wall   {} → {}  len={:.3}", d.handle, fv(s), fv(e), (e - s).len()));
            },
        }
    }
    out
}

/// Build the sketch, apply constraints, solve, and write solved geometry back
/// into the drawing (lines AND circles). Returns the outcome (message +
/// convergence) so the caller can roll a conflicting constraint back.
pub fn solve_doc(doc: &mut Document, session: &ParamSession) -> SolveOutcome {
    solve_doc_driven(doc, session, &HashSet::new())
}

/// Like [`solve_doc`], but hard-pins the `drivers` entities at their current
/// positions (the ones the user just moved/dragged), so the constraint link is
/// kept by moving everything ELSE. This is what makes "rotate one → the linked
/// one follows" work.
pub fn solve_doc_driven(
    doc: &mut Document,
    session: &ParamSession,
    drivers: &HashSet<Handle>,
) -> SolveOutcome {
    let mut map = build_doc_map(doc);
    if map.sk.lines.is_empty() && map.sk.circles.is_empty() {
        return SolveOutcome {
            msg: "parametric: no line/circle geometry to solve".into(),
            converged: true,
            trace: SolveTrace::default(),
        };
    }
    let resolved = apply_constraints(&mut map, doc, session, drivers);

    // Pin every point that NO constraint touches, so a solve can't drift
    // unrelated geometry (this is why an isolated edit stays local). Driver and
    // constrained points are "involved" and stay free.
    let mut involved: HashSet<usize> = HashSet::new();
    let note_involved = |map: &DocMap, h: Handle, set: &mut HashSet<usize>| {
        if let Some(&l) = map.line_id.get(&h) {
            let ln = map.sk.lines[l];
            set.insert(ln.a);
            set.insert(ln.b);
        }
        if let Some(&ci) = map.circle_id.get(&h) {
            set.insert(map.sk.circles[ci].center);
        }
    };
    for c in &session.constraints {
        for h in c.handles() { note_involved(&map, h, &mut involved); }
    }
    for h in drivers { note_involved(&map, *h, &mut involved); }
    let to_pin: Vec<(usize, Vec2)> = map.sk.points.iter().enumerate()
        .filter(|(i, _)| !involved.contains(i))
        .map(|(i, p)| (i, *p))
        .collect();
    let pinned = to_pin.len();
    for (i, p) in to_pin {
        map.sk.add(Constraint::Fixed { p: i, x: p.x, y: p.y });
    }

    // ---- record MATH INPUT (the exact cad_param sketch the solver sees) ----
    let fmt_v = |p: Vec2| format!("({:.3}, {:.3})", p.x, p.y);
    let input_geom = doc_geom_lines(doc); // doc still original until write-back
    let mut math_in: Vec<String> = Vec::new();
    math_in.push(format!("points ({}):", map.sk.points.len()));
    for (i, p) in map.sk.points.iter().enumerate() {
        math_in.push(format!("  p{i} = {}", fmt_v(*p)));
    }
    if !map.sk.scalars.is_empty() {
        math_in.push(format!("scalars ({}): {:?}", map.sk.scalars.len(), map.sk.scalars));
    }
    for (i, l) in map.sk.lines.iter().enumerate() {
        math_in.push(format!("  L{i} = p{}→p{}", l.a, l.b));
    }
    for (i, c) in map.sk.circles.iter().enumerate() {
        math_in.push(format!("  C{i} = centre p{} radius s{}", c.center, c.radius));
    }
    let res_in = cad_param::residual_breakdown(&map.sk);

    // ---- solve (record before/after for the diagnostics trace) ----
    let init_pts = map.sk.points.clone();
    let init_rms = cad_param::current_rms(&map.sk);
    let rep = solve(&mut map.sk);
    let res_out = cad_param::residual_breakdown(&map.sk);
    let max_disp = init_pts.iter().zip(&map.sk.points)
        .map(|(a, b)| (*b - *a).len())
        .fold(0.0_f64, f64::max);

    // write back lines
    for &(idx, pa, pb) in &map.lines {
        if let Some(d) = doc.dobjects.get_mut(idx) {
            if let Geom::Line(l) = &mut d.geom {
                l.a = map.sk.points[pa];
                l.b = map.sk.points[pb];
            }
        }
    }
    // write back wall centerlines (start/end)
    for &(idx, pa, pb) in &map.walls {
        if let Some(d) = doc.dobjects.get_mut(idx) {
            if let Geom::Wall(w) = &mut d.geom {
                w.start = map.sk.points[pa];
                w.end = map.sk.points[pb];
            }
        }
    }
    // write back circles
    for &(idx, c, s) in &map.circles {
        if let Some(d) = doc.dobjects.get_mut(idx) {
            if let Geom::Circle(circ) = &mut d.geom {
                circ.center = map.sk.points[c];
                circ.radius = map.sk.scalars[s].abs();
            }
        }
    }

    let n_geom = map.sk.lines.len() + map.sk.circles.len();
    let total = session.constraints.len();
    let msg = format!(
        "solved {} entit{}: {}/{} constraints applied · rms={:.2e}{}",
        n_geom,
        if n_geom == 1 { "y" } else { "ies" },
        resolved,
        total,
        rep.residual,
        if rep.converged { "" } else { "  (NOT converged)" }
    );

    let output_geom = doc_geom_lines(doc); // doc now holds solved geometry

    // ---- assemble the full solve recorder (geometry + math, in & out) ----
    let mut t: Vec<String> = Vec::new();
    t.push("════════ PARAMETRIC SOLVE ════════".into());
    let drv: Vec<String> = drivers.iter().map(|h| format!("{h:?}")).collect();
    t.push(format!("drivers (pinned as moved): [{}]", drv.join(", ")));
    t.push(format!("user constraints: {} ({} resolved)", total, resolved));
    t.push("── INPUT geometry ──".into());
    t.extend(input_geom);
    t.push("── MATH INPUT (cad_param sketch) ──".into());
    t.extend(math_in);
    t.push(format!("── CONSTRAINTS (sketch, {} eqs) · residual in → out ──", map.sk.constraints.len()));
    let mut nfixed = 0usize;
    for (i, c) in map.sk.constraints.iter().enumerate() {
        if matches!(c, Constraint::Fixed { .. }) { nfixed += 1; continue; }
        t.push(format!(
            "  {:?}   r {:.2e} → {:.2e}",
            c, res_in.get(i).copied().unwrap_or(0.0), res_out.get(i).copied().unwrap_or(0.0)
        ));
    }
    t.push(format!("  (+ {nfixed} Fixed anchors/pins)"));
    t.push("── SOLVE ──".into());
    t.push(format!(
        "  rms {:.3e} → {:.3e}   {} iters   {}",
        init_rms, rep.residual, rep.iterations,
        if rep.converged { "✓ CONVERGED" } else { "✗ DIVERGED / not converged" }
    ));
    t.push(format!("  unrelated points pinned: {pinned}   largest point move: {max_disp:.3}"));
    t.push("── OUTPUT geometry ──".into());
    t.extend(output_geom);
    if !session.constraints.is_empty() {
        t.push("── per-constraint geometric error (solved) ──".into());
        for c in &session.constraints {
            t.push(format!("  • {}", cref_report(doc, c)));
        }
    }
    t.push("══════════════════════════════════".into());

    SolveOutcome { msg, converged: rep.converged, trace: SolveTrace { lines: t } }
}

/// Compute the degrees-of-freedom diagnosis WITHOUT moving geometry, plus a
/// per-handle "fully defined" map for the canvas overlay. Cheap for the small
/// sketches parametric mode targets.
pub fn analyze_doc(doc: &Document, session: &ParamSession) -> (DofReport, HashMap<Handle, bool>) {
    let mut map = build_doc_map(doc);
    let _ = apply_constraints(&mut map, doc, session, &HashSet::new());
    let rep = dof_analysis(&map.sk);
    let mut defined = HashMap::new();
    for (h, params) in &map.handle_params {
        let all_locked = params.iter().all(|&i| !rep.param_free.get(i).copied().unwrap_or(true));
        defined.insert(*h, all_locked);
    }
    (rep, defined)
}

/// Per-entity math inspection — "deeply rooted in the math library". For one
/// drawing handle it reports the cad_param sketch ids it maps to, each
/// parameter's value and whether the solver considers it FREE or locked (from
/// the Jacobian-rank DOF analysis), the entity's local degrees of freedom, and
/// every constraint that references it with its current geometric error.
pub fn inspect_handle(doc: &Document, session: &ParamSession, h: Handle) -> Option<Vec<String>> {
    let mut map = build_doc_map(doc);
    let _ = apply_constraints(&mut map, doc, session, &HashSet::new());
    let rep = dof_analysis(&map.sk);
    let np = map.sk.points.len();
    let free = |i: usize| rep.param_free.get(i).copied().unwrap_or(true);
    let yn = |b: bool| if b { "FREE" } else { "locked" };

    let mut out: Vec<String> = Vec::new();
    let mut local_free = 0usize;
    let mut local_total = 0usize;

    if let Some(&lid) = map.line_id.get(&h) {
        let ln = map.sk.lines[lid];
        let is_wall = matches!(doc.dobjects.iter().find(|d| d.handle == h).map(|d| &d.geom), Some(Geom::Wall(_)));
        out.push(format!("handle {:?}  ({}, sketch L{lid})", h, if is_wall { "wall" } else { "line" }));
        for pid in [ln.a, ln.b] {
            let (fx, fy) = (free(2 * pid), free(2 * pid + 1));
            let p = map.sk.points[pid];
            out.push(format!("  p{pid} = ({:.3}, {:.3})    x:{}  y:{}", p.x, p.y, yn(fx), yn(fy)));
            local_total += 2;
            local_free += fx as usize + fy as usize;
        }
    } else if let Some(&cid) = map.circle_id.get(&h) {
        let c = map.sk.circles[cid];
        out.push(format!("handle {:?}  (circle, sketch C{cid})", h));
        let (fx, fy) = (free(2 * c.center), free(2 * c.center + 1));
        let p = map.sk.points[c.center];
        out.push(format!("  centre p{} = ({:.3}, {:.3})    x:{}  y:{}", c.center, p.x, p.y, yn(fx), yn(fy)));
        let sidx = 2 * np + c.radius;
        let fr = free(sidx);
        out.push(format!("  radius s{} = {:.3}    {}", c.radius, map.sk.scalars[c.radius], yn(fr)));
        local_total += 3;
        local_free += fx as usize + fy as usize + fr as usize;
    } else {
        return None;
    }

    out.push(format!("  local DOF: {local_free}/{local_total} params free  ·  sketch total {} DOF", rep.dof));
    let touching: Vec<&CRef> = session.constraints.iter().filter(|c| c.handles().contains(&h)).collect();
    if touching.is_empty() {
        out.push("  ⚠ no constraint references this entity".into());
    } else {
        out.push(format!("  constraints touching it ({}):", touching.len()));
        for c in touching {
            out.push(format!("    • {}", cref_report(doc, c)));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_kernel::{Circle, DObject, Line};

    fn add_line(doc: &mut Document, a: Vec2, b: Vec2) -> Handle {
        let d = DObject::new(Geom::Line(Line { a, b }));
        let h = d.handle;
        doc.dobjects.push(d);
        h
    }
    fn add_circle(doc: &mut Document, c: Vec2, r: f64) -> Handle {
        let d = DObject::new(Geom::Circle(Circle { center: c, radius: r }));
        let h = d.handle;
        doc.dobjects.push(d);
        h
    }
    fn add_wall(doc: &mut Document, s: Vec2, e: Vec2) -> Handle {
        let d = DObject::new(Geom::Wall(cad_kernel::Wall {
            start: s, end: e, thickness: 4.0, style: 0, bulge: 0.0 }));
        let h = d.handle;
        doc.dobjects.push(d);
        h
    }

    #[test]
    fn solve_doc_makes_a_wall_horizontal() {
        let mut doc = Document::default();
        let h0 = add_wall(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 5.0));
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Horizontal(h0));
        let out = solve_doc(&mut doc, &sess);
        let Geom::Wall(w) = &doc.dobjects[0].geom else { panic!() };
        assert!((w.start.y - w.end.y).abs() < 1e-6, "wall not horizontal ({})", out.msg);
    }

    #[test]
    fn wall_and_line_can_be_made_perpendicular() {
        // a wall and a line sharing a corner, made perpendicular — proves walls
        // are first-class linear entities in the solver.
        let mut doc = Document::default();
        let hw = add_wall(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        let hl = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(8.0, 3.0));
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Perpendicular(hw, hl));
        let _ = solve_doc(&mut doc, &sess);
        let Geom::Wall(w) = &doc.dobjects[0].geom else { panic!() };
        let Geom::Line(l) = &doc.dobjects[1].geom else { panic!() };
        let u = w.end - w.start;
        let v = l.b - l.a;
        assert!(u.dot(v).abs() < 1e-5, "dot={}", u.dot(v));
    }

    #[test]
    fn solve_doc_makes_a_line_horizontal() {
        let mut doc = Document::default();
        let h0 = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 5.0));
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Horizontal(h0));
        let out = solve_doc(&mut doc, &sess);
        let Geom::Line(l) = &doc.dobjects[0].geom else { panic!() };
        assert!((l.a.y - l.b.y).abs() < 1e-6, "not horizontal ({})", out.msg);
    }

    #[test]
    fn prune_drops_constraints_for_deleted_geometry() {
        let mut doc = Document::default();
        let h0 = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        let h1 = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(0.0, 10.0));
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Horizontal(h0));
        sess.constraints.push(CRef::Vertical(h1));
        // delete the second line
        doc.dobjects.retain(|d| d.handle == h0);
        let dropped = prune_constraints(&doc, &mut sess.constraints);
        assert_eq!(dropped, 1);
        assert_eq!(sess.constraints.len(), 1);
    }

    #[test]
    fn conflicting_h_v_does_not_converge() {
        // horizontal + vertical on the SAME line is impossible (non-degenerate)
        // — solve_doc must report non-convergence so the UI can roll it back.
        let mut doc = Document::default();
        let h0 = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Horizontal(h0));
        sess.constraints.push(CRef::Length(h0, 120.0));
        sess.constraints.push(CRef::Vertical(h0));
        let out = solve_doc(&mut doc, &sess);
        assert!(!out.converged, "should NOT converge: {}", out.msg);
    }

    #[test]
    fn solve_doc_makes_two_lines_perpendicular() {
        let mut doc = Document::default();
        let h0 = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        let h1 = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(8.0, 3.0));
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Perpendicular(h0, h1));
        let _ = solve_doc(&mut doc, &sess);
        let Geom::Line(l0) = &doc.dobjects[0].geom else { panic!() };
        let Geom::Line(l1) = &doc.dobjects[1].geom else { panic!() };
        let u = l0.b - l0.a;
        let v = l1.b - l1.a;
        assert!(u.dot(v).abs() < 1e-5, "dot={}", u.dot(v));
    }

    #[test]
    fn solve_doc_sets_circle_radius() {
        let mut doc = Document::default();
        let h = add_circle(&mut doc, Vec2::new(2.0, 2.0), 3.0);
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Radius(h, 9.0));
        let _ = solve_doc(&mut doc, &sess);
        let Geom::Circle(c) = &doc.dobjects[0].geom else { panic!() };
        assert!((c.radius - 9.0).abs() < 1e-6, "r={}", c.radius);
    }

    #[test]
    fn solve_doc_makes_circles_concentric_and_equal() {
        let mut doc = Document::default();
        let h0 = add_circle(&mut doc, Vec2::new(0.0, 0.0), 5.0);
        let h1 = add_circle(&mut doc, Vec2::new(4.0, 1.0), 2.0);
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Concentric(h0, h1));
        sess.constraints.push(CRef::EqualRadius(h0, h1));
        let _ = solve_doc(&mut doc, &sess);
        let Geom::Circle(c0) = &doc.dobjects[0].geom else { panic!() };
        let Geom::Circle(c1) = &doc.dobjects[1].geom else { panic!() };
        assert!((c0.center - c1.center).len() < 1e-5, "centers differ");
        assert!((c0.radius - c1.radius).abs() < 1e-5, "radii differ");
    }

    #[test]
    fn analyze_reports_underdefined_then_defined() {
        let mut doc = Document::default();
        let h0 = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 1.0));
        let sess = ParamSession::new();
        let (rep, defined) = analyze_doc(&doc, &sess);
        // anchored first point removes 2 DOF; a free line still has DOF left
        assert!(rep.dof > 0, "expected under-defined, dof={}", rep.dof);
        assert_eq!(defined.get(&h0), Some(&false));
    }

    #[test]
    fn pentagon_parallel_does_not_explode() {
        // The reported bug: a closed pentagon at real coordinates (~thousands),
        // select two adjacent edges, apply Parallel → geometry exploded to
        // ±15000. With normalized residuals + pinning untouched points it must
        // stay bounded, make those two edges parallel, and leave the FAR edge
        // (#6, touched by no constraint) exactly where it was.
        let mut doc = Document::default();
        let a = Vec2::new(3248.7959, 4004.4858);
        let b = Vec2::new(5316.1538, 5652.0);
        let c = Vec2::new(7888.0684, 3823.5750);
        let d = Vec2::new(6548.9917, 1475.9070);
        let e = Vec2::new(3653.9062, 2058.4360);
        let h3 = add_line(&mut doc, a, b);
        let h4 = add_line(&mut doc, b, c);
        let _h5 = add_line(&mut doc, c, d);
        let h6 = add_line(&mut doc, d, e);
        let _h7 = add_line(&mut doc, e, a);
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Parallel(h3, h4));
        let out = solve_doc(&mut doc, &sess);
        assert!(out.converged, "did not converge: {}", out.msg);
        // nothing flew off
        for dobj in &doc.dobjects {
            if let Geom::Line(l) = &dobj.geom {
                for p in [l.a, l.b] {
                    assert!(p.x.abs() < 20_000.0 && p.y.abs() < 20_000.0, "exploded: {:?}", p);
                }
            }
        }
        // #6 (untouched by any constraint) stayed put
        let Geom::Line(l6) = &doc.dobjects[3].geom else { panic!() };
        assert!((l6.a - d).len() < 1e-6 && (l6.b - e).len() < 1e-6, "untouched edge moved");
        // #3 ∥ #4
        let Geom::Line(l3) = &doc.dobjects[0].geom else { panic!() };
        let Geom::Line(l4) = &doc.dobjects[1].geom else { panic!() };
        let (u, v) = (l3.b - l3.a, l4.b - l4.a);
        assert!((u.x * v.y - u.y * v.x).abs() / (u.len() * v.len()) < 1e-5, "not parallel");
    }

    #[test]
    fn driven_solve_keeps_link_other_follows() {
        // two parallel lines; the user tilts line A; a driven re-solve with A
        // pinned must keep A put and rotate B to stay parallel ("keep the link").
        let mut doc = Document::default();
        let ha = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        let hb = add_line(&mut doc, Vec2::new(0.0, 5.0), Vec2::new(10.0, 5.0));
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Parallel(ha, hb));
        let _ = solve_doc(&mut doc, &sess);
        // user tilts A's far endpoint
        if let Geom::Line(l) = &mut doc.dobjects[0].geom { l.b = Vec2::new(10.0, 4.0); }
        let drivers: HashSet<Handle> = [ha].into_iter().collect();
        let _ = solve_doc_driven(&mut doc, &sess, &drivers);
        let Geom::Line(a) = &doc.dobjects[0].geom else { panic!() };
        let Geom::Line(b) = &doc.dobjects[1].geom else { panic!() };
        // A stayed where the user put it
        assert!((a.b - Vec2::new(10.0, 4.0)).len() < 1e-6, "driver moved: {:?}", a.b);
        // B followed — now parallel to A
        let (u, v) = (a.b - a.a, b.b - b.a);
        assert!((u.x * v.y - u.y * v.x).abs() < 1e-5, "B not parallel after drag");
    }

    #[test]
    fn chained_equal_length_makes_all_selected_equal() {
        // Mirrors what the UI now does for a multi-selection: chain Equal from
        // the first edge to every other → ALL become equal length.
        let mut doc = Document::default();
        let h0 = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)); // 10
        let h1 = add_line(&mut doc, Vec2::new(0.0, 5.0), Vec2::new(30.0, 5.0)); // 30
        let h2 = add_line(&mut doc, Vec2::new(0.0, 9.0), Vec2::new(20.0, 9.0)); // 20
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Equal(h0, h1));
        sess.constraints.push(CRef::Equal(h0, h2));
        let out = solve_doc(&mut doc, &sess);
        assert!(out.converged, "{}", out.msg);
        let len = |i: usize| { let Geom::Line(l) = &doc.dobjects[i].geom else { panic!() }; (l.b - l.a).len() };
        let (a, b, c) = (len(0), len(1), len(2));
        assert!((a - b).abs() < 1e-5 && (a - c).abs() < 1e-5, "not all equal: {a}, {b}, {c}");
    }

    #[test]
    fn inspect_handle_reports_free_and_locked_params() {
        let mut doc = Document::default();
        let h = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(10.0, 3.0));
        let mut sess = ParamSession::new();
        sess.constraints.push(CRef::Horizontal(h));
        let report = inspect_handle(&doc, &sess, h).expect("inspect");
        // mentions the constraint touching it and the cad_param sketch line id
        assert!(report.iter().any(|l| l.contains("horizontal")));
        assert!(report.iter().any(|l| l.contains("L0") || l.contains("line")));
        // a non-existent handle returns None
        assert!(inspect_handle(&doc, &sess, add_circle(&mut Document::default(), Vec2::new(0.0,0.0), 1.0)).is_none());
    }

    #[test]
    fn pending_kind_builds_the_right_pair() {
        let mut doc = Document::default();
        let a = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0));
        let b = add_line(&mut doc, Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0));
        assert!(matches!(PendingKind::Parallel.to_cref(a, b), CRef::Parallel(x, y) if x == a && y == b));
        assert!(matches!(PendingKind::Tangent.to_cref(a, b), CRef::Tangent(..)));
        assert!(!PendingKind::Parallel.target_is_circle());
        assert!(PendingKind::Concentric.target_is_circle());
        assert!(PendingKind::EqualRadius.target_is_circle());
    }

    #[test]
    fn eval_field_uses_variables() {
        let mut sess = ParamSession::new();
        sess.vars.set("W", "120");
        assert!((sess.eval_field("=W/2").unwrap() - 60.0).abs() < 1e-9);
        assert!((sess.eval_field("25").unwrap() - 25.0).abs() < 1e-9);
    }
}
