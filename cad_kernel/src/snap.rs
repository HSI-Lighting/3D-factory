// Object-snap framework.
//
// Two flows feed into `find_snap`:
//
//  1. Passive hover-snap: the UI calls `find_snap` every frame with the cursor
//     position, the user's enabled `SnapSet` from the floating settings panel,
//     and `forced = None`. The function returns the highest-priority snap
//     candidate within the screen-space search radius, if any.
//  2. Typed override: when the user types a snap keyword on the command line
//     (PER, END, MID, …) the next click goes through `find_snap` with
//     `forced = Some(kind)` — only that kind is considered, and it bypasses
//     the persistent enable bitmask.
//
// Priority (when multiple kinds match within radius):
//     END > MID > CEN > INT > PER > TAN > NEA
// — same convention every CAD user already has in their muscle memory.
//
// All snap kinds are local computations on a single dobject, except INT which
// looks at intersection points of nearby dobject pairs.

use crate::dobject::DObject;
use crate::geom::{Arc, Circle, Ellipse, EllipseArc, Geom, Line};
use crate::intersect::intersect;
use crate::math::{newton_roots_periodic, scaled_tol, Vec2, EPS};
use crate::spatial::UniformGrid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapKind {
    End,    // line/arc endpoints
    Mid,    // midpoint of line / arc
    Cen,    // centre of circle / arc
    Qua,    // quadrant of circle / arc (the four cardinal compass points)
    Int,    // intersection of two nearby dobjects
    Per,    // perpendicular from an anchor point — requires `from`
    Tan,    // tangent from an anchor point — requires `from`
    Nea,    // nearest point on the curve
}

impl SnapKind {
    /// Canonical lowercase three-letter command name.
    pub fn name(self) -> &'static str {
        match self {
            SnapKind::End => "END",
            SnapKind::Mid => "MID",
            SnapKind::Cen => "CEN",
            SnapKind::Qua => "QUA",
            SnapKind::Int => "INT",
            SnapKind::Per => "PER",
            SnapKind::Tan => "TAN",
            SnapKind::Nea => "NEA",
        }
    }

    /// Parse a token typed in the command line. Case-insensitive.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "end" | "endpoint" => Some(SnapKind::End),
            "mid" | "midpoint" => Some(SnapKind::Mid),
            "cen" | "center" | "centre" => Some(SnapKind::Cen),
            "qua" | "quadrant" => Some(SnapKind::Qua),
            "int" | "intersect" | "intersection" => Some(SnapKind::Int),
            "per" | "perp" | "perpendicular" => Some(SnapKind::Per),
            "tan" | "tangent" => Some(SnapKind::Tan),
            "nea" | "near" | "nearest" => Some(SnapKind::Nea),
            _ => None,
        }
    }

    /// PER and TAN need an anchor point (the last clicked pending point); the
    /// rest snap to features intrinsic to the hovered dobject and ignore `from`.
    pub fn requires_from(self) -> bool {
        matches!(self, SnapKind::Per | SnapKind::Tan)
    }

    /// Lower number = higher priority. Matches the conventional AutoCAD order.
    pub fn priority(self) -> u8 {
        match self {
            SnapKind::End => 0,
            SnapKind::Mid => 1,
            SnapKind::Cen => 2,
            SnapKind::Qua => 3,
            SnapKind::Int => 4,
            SnapKind::Per => 5,
            SnapKind::Tan => 6,
            SnapKind::Nea => 7,
        }
    }

    pub const ALL: [SnapKind; 8] = [
        SnapKind::End, SnapKind::Mid, SnapKind::Cen, SnapKind::Qua,
        SnapKind::Int, SnapKind::Per, SnapKind::Tan, SnapKind::Nea,
    ];
}

/// Persistent enable bits for each snap kind — mirrors AutoCAD's running-osnap
/// settings panel. The UI keeps one of these on `CadApp` and mutates it
/// through the floating settings window.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SnapSet {
    pub end: bool,
    pub mid: bool,
    pub cen: bool,
    pub qua: bool,
    pub int: bool,
    pub per: bool,
    pub tan: bool,
    pub nea: bool,
}

impl SnapSet {
    pub fn is_enabled(&self, k: SnapKind) -> bool {
        match k {
            SnapKind::End => self.end,
            SnapKind::Mid => self.mid,
            SnapKind::Cen => self.cen,
            SnapKind::Qua => self.qua,
            SnapKind::Int => self.int,
            SnapKind::Per => self.per,
            SnapKind::Tan => self.tan,
            SnapKind::Nea => self.nea,
        }
    }

    pub fn set(&mut self, k: SnapKind, v: bool) {
        match k {
            SnapKind::End => self.end = v,
            SnapKind::Mid => self.mid = v,
            SnapKind::Cen => self.cen = v,
            SnapKind::Qua => self.qua = v,
            SnapKind::Int => self.int = v,
            SnapKind::Per => self.per = v,
            SnapKind::Tan => self.tan = v,
            SnapKind::Nea => self.nea = v,
        }
    }

    pub fn any(&self) -> bool {
        self.end || self.mid || self.cen || self.qua || self.int
            || self.per || self.tan || self.nea
    }

    /// Sensible first-launch defaults — the snaps every CAD user expects on
    /// out of the box. Matches AutoCAD's typical default running osnaps.
    pub fn defaults() -> Self {
        SnapSet { end: true, mid: true, cen: true, qua: true,
                  int: false, per: false, tan: false, nea: false }
    }
}

/// Result of a successful snap lookup.
#[derive(Clone, Copy, Debug)]
pub struct SnapHit {
    pub kind:  SnapKind,
    pub point: Vec2,
    /// Index of the dobject the snap point lies on. For INT it's the index of
    /// one of the two intersecting dobjects (the renderer doesn't actually
    /// care which one for a marker glyph).
    pub dobject: Option<usize>,
    /// Set when the snap point lies OUTSIDE the dobject's visible range — e.g.
    /// PER foot on the infinite-line extension beyond a segment endpoint, or
    /// PER foot on the full-circle extension of an arc. The point itself is
    /// always returned; this carries the on-curve anchor so the UI can draw
    /// a dashed "imaginary extension" line from anchor → point.
    pub extension_anchor: Option<Vec2>,
}

/// Single entry point for both passive hover and typed override.
///
/// - `cursor` is in world coordinates.
/// - `world_radius` is the screen-pixel search radius converted to world units
///   (i.e. `pixel_radius / scale`). All candidates must fall inside this disk.
/// - `enabled` is the user's persistent toggle set; ignored when `forced` is
///   set.
/// - `forced` is the one-shot typed override; when `Some(k)` only that kind is
///   considered and the priority loop short-circuits.
/// - `from` is the anchor point (e.g. last clicked point of an in-progress
///   draw). Required for PER / TAN; ignored otherwise.
/// - `grid` is the spatial index; if absent we fall back to scanning all
///   dobjects (fine for small drawings).
pub fn find_snap(
    cursor: Vec2,
    world_radius: f64,
    enabled: SnapSet,
    forced: Option<SnapKind>,
    from: Option<Vec2>,
    dobjects: &[DObject],
    grid: Option<&UniformGrid>,
) -> Option<SnapHit> {
    find_all_snaps(cursor, world_radius, enabled, forced, from, dobjects, grid)
        .into_iter().next()
}

/// Collect every viable snap candidate at this cursor position, sorted by
/// (kind priority, distance to cursor). The first element is what `find_snap`
/// would return; subsequent elements are the alternatives the user can
/// Tab-cycle through.
///
/// Same parameters as [`find_snap`]; see that doc.
pub fn find_all_snaps(
    cursor: Vec2,
    world_radius: f64,
    enabled: SnapSet,
    forced: Option<SnapKind>,
    from: Option<Vec2>,
    dobjects: &[DObject],
    grid: Option<&UniformGrid>,
) -> Vec<SnapHit> {
    if world_radius <= 0.0 || dobjects.is_empty() { return Vec::new(); }

    let kinds: Vec<SnapKind> = if let Some(k) = forced {
        vec![k]
    } else if !enabled.any() {
        return Vec::new();
    } else {
        let mut ks: Vec<SnapKind> = SnapKind::ALL
            .iter().copied().filter(|k| enabled.is_enabled(*k)).collect();
        ks.sort_by_key(|k| k.priority());
        ks
    };

    let r2 = world_radius * world_radius;
    // (hit, sort_key) — sort_key is the per-kind tiebreaker (distance² to
    // cursor for mouse-priority kinds, dobject distance for cursor-on-dobject
    // kinds). Smaller = better.
    let mut hits: Vec<(SnapHit, f64)> = Vec::new();

    for k in kinds {
        if k.requires_from() && from.is_none() { continue; }

        // PER/TAN can land on the extension — widen the dobject search.
        let dobject_search_r = if k.requires_from() {
            world_radius * 20.0
        } else {
            world_radius
        };
        let mut cand_ents: Vec<usize> = match grid {
            Some(g) => g.query_near(cursor, dobject_search_r)
                        .into_iter().map(|u| u as usize).collect(),
            None    => (0..dobjects.len()).collect(),
        };
        // P2: the grid's indices are from its BUILD-time count. If the doc shrank
        // (delete-then-hover) the grid is stale, so drop any index past the
        // current slice — else `dobjects[..]` panics in the INT + main loops below.
        cand_ents.retain(|&i| i < dobjects.len());
        if cand_ents.is_empty() { continue; }

        if k == SnapKind::Int {
            for i in 0..cand_ents.len() {
                for j in (i + 1)..cand_ents.len() {
                    let pts = intersect(
                        &dobjects[cand_ents[i]].geom,
                        &dobjects[cand_ents[j]].geom,
                    );
                    for p in pts {
                        let d2 = (p - cursor).len_sq();
                        if d2 < r2 {
                            hits.push((SnapHit {
                                kind: k, point: p,
                                dobject: Some(cand_ents[i]),
                                extension_anchor: None,
                            }, d2));
                        }
                    }
                }
            }
        } else {
            // Cursor-on-dobject activation:
            //   CEN, NEA — natural cursor-on-curve snaps.
            //   PER, TAN — the foot/tangent location is determined entirely
            //              by anchor geometry; the user can't possibly guess
            //              where it lands in empty space. Hovering the
            //              dobject is the only sane way to invoke them.
            // For PER/TAN with multiple feet (close and far on a circle/arc),
            // candidates are sorted by cursor distance so Tab cycles from
            // the nearest foot outward.
            let dobject_priority = matches!(k,
                SnapKind::Cen | SnapKind::Nea | SnapKind::Per | SnapKind::Tan
            );

            for &ei in &cand_ents {
                let e = &dobjects[ei];

                if dobject_priority {
                    let d_ent = e.distance_to_point(cursor);
                    let mut local: Vec<(Vec2, Option<Vec2>, f64)> =
                        candidate_points(k, &e.geom, cursor, from).into_iter()
                            .map(|(p, a)| (p, a, (p - cursor).len_sq()))
                            .collect();
                    if local.is_empty() { continue; }

                    // Activation: cursor is near the dobject's visible curve
                    // OR near one of its candidate points (the latter is the
                    // PER/TAN "imaginary extension" case where the foot
                    // lives in empty space far from the dobject itself).
                    let nearest_d2 = local.iter().map(|(_, _, d2)| *d2)
                        .fold(f64::INFINITY, f64::min);
                    if d_ent > world_radius && nearest_d2 > r2 { continue; }

                    // Closest-to-cursor candidate first — that's the default;
                    // Tab walks the rest in order of cursor proximity.
                    local.sort_by(|a, b|
                        a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
                    // Effective sort key across dobjects: whichever measure
                    // (dobject-distance or nearest-candidate distance) is
                    // smaller. Picks the dobject the user is "most clearly"
                    // pointing at.
                    let sort_key = d_ent.min(nearest_d2.sqrt());
                    for (p, anchor, _) in local {
                        hits.push((SnapHit {
                            kind: k, point: p, dobject: Some(ei),
                            extension_anchor: anchor,
                        }, sort_key));
                    }
                } else {
                    for (p, anchor) in candidate_points(k, &e.geom, cursor, from) {
                        let d2 = (p - cursor).len_sq();
                        if d2 < r2 {
                            hits.push((SnapHit {
                                kind: k, point: p, dobject: Some(ei),
                                extension_anchor: anchor,
                            }, d2));
                        }
                    }
                }
            }
        }
    }

    // Sort by (kind priority, sort_key). Iteration order already preserved
    // priority, but explicit sort makes the contract obvious to callers.
    hits.sort_by(|a, b| {
        a.0.kind.priority().cmp(&b.0.kind.priority())
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });

    hits.into_iter().map(|(h, _)| h).collect()
}

/// All candidate snap points of a given kind that the dobject can offer.
/// The second element of each tuple is the optional on-dobject anchor used to
/// draw the "imaginary extension" dashed line (Some only when the snap target
/// lies outside the dobject's visible range — see [`SnapHit::extension_anchor`]).
fn candidate_points(
    k: SnapKind, e: &Geom, cursor: Vec2, from: Option<Vec2>,
) -> Vec<(Vec2, Option<Vec2>)> {
    fn plain<T: IntoIterator<Item = Vec2>>(i: T) -> Vec<(Vec2, Option<Vec2>)> {
        i.into_iter().map(|p| (p, None)).collect()
    }
    match k {
        SnapKind::End => match e {
            Geom::Line(l)         => plain([l.a, l.b]),
            Geom::Arc(a)          => { let (p1, p2) = a.endpoints(); plain([p1, p2]) }
            Geom::EllipseArc(ea)  => { let (p1, p2) = ea.endpoints(); plain([p1, p2]) }
            // Point's location IS its endpoint; useful to snap to.
            Geom::Point(pt)       => plain([pt.location]),
            // Polyline END = EVERY vertex. Each vertex is the end of a
            // segment, so AutoCAD's ENDpoint osnap lands on all of them
            // (open or closed) — not just the two extreme ends. This is
            // also what makes a polyline-in-progress snap to each already
            // placed vertex during the ghost preview, not only the start.
            Geom::Polyline(p) => {
                if p.vertices.is_empty() { Vec::new() }
                else { plain(p.vertices.iter().map(|v| v.pos).collect::<Vec<_>>()) }
            }
            // Spline endpoints = first and last control point (clamped
            // curves interpolate their endpoint control points exactly).
            Geom::Spline(s) => {
                if s.control_points.is_empty() { Vec::new() }
                else {
                    plain([s.control_points[0],
                           s.control_points[s.control_points.len() - 1]])
                }
            }
            Geom::Circle(_) | Geom::Ellipse(_) | Geom::Hatch(_) => Vec::new(),
            // Wall END = the two CENTERLINE endpoints (the wall's TRUE
            // endpoints, and the exact nodes `cad_wall::solve_faces` mitres
            // on). Snapping a new wall's endpoint here makes the two
            // centerlines coincide, so the corner auto-cleans. The face
            // corners are deliberately NOT offered: they drift as neighbours
            // mitre (unstable target) and snapping there left walls unjoined.
            Geom::Wall(w) => plain([w.start, w.end]),
            // Text — anchor point is the only snap "endpoint".
            Geom::Text(t) => plain([t.position]),
            // Dimension — its three def points act as snap endpoints.
            Geom::Dimension(d) => plain(d.grip_points()),
            // BlockRef — insertion point only (v1; snap-through to the
            // contained geometry needs Document access — deferred).
            Geom::BlockRef(br) => plain([br.insert]),
            // Leader END = EVERY chain vertex (like Polyline — each
            // vertex is a segment end). The arrow tip + landing included.
            Geom::Leader(l) => {
                if l.pts.is_empty() { Vec::new() }
                else { plain(l.pts.clone()) }
            }
            // AttrDef — its position is the only "endpoint".
            Geom::AttrDef(a) => plain([a.position]),
            // CenterMark END — all four arm tips.
            Geom::CenterMark(cm) => plain(cm.tips().to_vec()),
            // Xline — infinite: no endpoints.
            Geom::Xline(_) => Vec::new(),
            // Ray — the base is the one real endpoint.
            Geom::Ray(r) => plain([r.base]),
            // Wipeout / Region END — every loop vertex.
            Geom::Wipeout(w) => plain(w.pts.clone()),
            Geom::Region(rg) => plain(rg.loop_pts.clone()),
            // Donut — no endpoints.
            Geom::Donut(_) => Vec::new(),
            // Table — annotation, no endpoints.
            Geom::Table(_) => Vec::new(),
            // Xref — resolved by exploding; no direct endpoints.
            Geom::Xref(_) => Vec::new(),
            Geom::Viewport(_) => Vec::new(),
        },
        SnapKind::Mid => match e {
            Geom::Line(l) => plain([(l.a + l.b) * 0.5]),
            Geom::Arc(a)  => {
                let m = a.start_angle + a.sweep_angle * 0.5;
                plain([a.center + Vec2::new(a.radius * m.cos(), a.radius * m.sin())])
            }
            Geom::EllipseArc(ea) => {
                let m = ea.start_param + ea.sweep_param * 0.5;
                plain([ea.ellipse.point_at(m)])
            }
            // Polyline MID = midpoint of every segment.
            Geom::Polyline(p) => {
                if p.vertices.len() < 2 { return Vec::new(); }
                let n = p.vertices.len();
                let pairs = if p.closed { n } else { n - 1 };
                let pts: Vec<Vec2> = (0..pairs).map(|i| {
                    let a = p.vertices[i].pos;
                    let b = p.vertices[(i + 1) % n].pos;
                    (a + b) * 0.5
                }).collect();
                plain(pts)
            }
            Geom::Circle(_) | Geom::Ellipse(_) | Geom::Point(_) | Geom::Hatch(_) | Geom::Spline(_) => Vec::new(),
            // Wall MID — midpoint of each visible side line.
            Geom::Wall(w) => {
                let mut out: Vec<Vec2> = Vec::new();
                if let Some(l) = w.left_line()  { out.push((l.a + l.b) * 0.5); }
                if let Some(r) = w.right_line() { out.push((r.a + r.b) * 0.5); }
                plain(out)
            }
            // Text has no MID concept (no segment); empty.
            Geom::Text(_) => Vec::new(),
            // Leader MID — midpoint of every chain segment.
            Geom::Leader(l) => {
                if l.pts.len() < 2 { return Vec::new(); }
                let pts: Vec<Vec2> = l.pts.windows(2)
                    .map(|w| (w[0] + w[1]) * 0.5).collect();
                plain(pts)
            }
            // AttrDef has no segments; empty.
            Geom::AttrDef(_) => Vec::new(),
            // CenterMark MID — midpoints of the two arms (the center twice).
            Geom::CenterMark(cm) => {
                let [t0, _t1, t2, _t3] = cm.tips();
                plain([(t0 + t2) * 0.5])
            }
            // Dimension MID — midpoint between the first two def points.
            Geom::Dimension(d) => {
                let g = d.grip_points();
                if g.len() >= 2 { plain([(g[0] + g[1]) * 0.5]) } else { Vec::new() }
            }
            Geom::BlockRef(_) => Vec::new(),
            Geom::Xline(_) => Vec::new(),
            Geom::Ray(_) => Vec::new(),
            Geom::Donut(_) => Vec::new(),
            Geom::Wipeout(_) => Vec::new(),
            Geom::Region(_) => Vec::new(),
            Geom::Table(_) => Vec::new(),
            Geom::Xref(_) => Vec::new(),
            Geom::Viewport(_) => Vec::new(),
        },
        SnapKind::Cen => match e {
            Geom::Line(_)        => Vec::new(),
            Geom::Arc(a)         => plain([a.center]),
            Geom::Circle(c)      => plain([c.center]),
            Geom::Ellipse(e)     => plain([e.center]),
            Geom::EllipseArc(ea) => plain([ea.ellipse.center]),
            Geom::Point(_) | Geom::Polyline(_) | Geom::Hatch(_) | Geom::Spline(_) => Vec::new(),
            // Wall has no canonical centre.
            Geom::Wall(_) => Vec::new(),
            // Text — anchor doubles as its "centre" for snapping.
            Geom::Text(t) => plain([t.position]),
            // Dimension CEN — for radius/diameter, the circle's center
            // (first grip); for linear, no canonical centre.
            Geom::Dimension(d) => {
                use crate::dim::DimKind;
                match &d.kind {
                    DimKind::Radius { center, .. } |
                    DimKind::Diameter { center, .. } => plain([*center]),
                    DimKind::Linear { .. } => Vec::new(),
                    // Angular CEN — the vertex (the angle's apex).
                    DimKind::Angular { vertex, .. } => plain([*vertex]),
                    // ArcLen CEN — the arc center.
                    DimKind::ArcLen { center, .. } => plain([*center]),
                    // Ordinate CEN — the datum doubles as the centre.
                    DimKind::Ordinate { datum, .. } => plain([*datum]),
                    // JoggedRadius CEN — the circle center.
                    DimKind::JoggedRadius { center, .. } => plain([*center]),
                }
            }
            // BlockRef CEN — the insertion point (mirrors AutoCAD's INS
            // snap until a dedicated SnapKind::Ins exists).
            Geom::BlockRef(br) => plain([br.insert]),
            // Leader has no canonical centre; AttrDef — position doubles
            // as its "centre" like Text.
            Geom::Leader(_) => Vec::new(),
            Geom::AttrDef(a) => plain([a.position]),
            // CenterMark CEN — the cross center.
             Geom::CenterMark(cm) => plain([cm.center]),
             Geom::Xline(_) => Vec::new(),
             Geom::Ray(_) => Vec::new(),
             Geom::Donut(d) => plain([d.center]),
             Geom::Wipeout(w) => {
                 let mut c = Vec2::ZERO;
                 for p in &w.pts { c = c + *p; }
                 if w.pts.is_empty() { Vec::new() } else { plain([c / w.pts.len() as f64]) }
             }
             Geom::Region(rg) => {
                 let mut c = Vec2::ZERO;
                 for p in &rg.loop_pts { c = c + *p; }
                 if rg.loop_pts.is_empty() { Vec::new() } else { plain([c / rg.loop_pts.len() as f64]) }
             }
             Geom::Table(_) => Vec::new(),
             Geom::Xref(_) => Vec::new(),
             Geom::Viewport(_) => Vec::new(),
         },
        // QUA — for circles & arcs, four cardinal compass points; for
        // ellipses & elliptical arcs, the FOUR AXIS-END POINTS (ends of
        // the semi-major axis × 2 and the semi-minor axis × 2). These
        // ROTATE with the ellipse — they are NOT compass E/N/W/S.
        SnapKind::Qua => match e {
            Geom::Line(_) | Geom::Xline(_) | Geom::Ray(_) | Geom::Donut(_) | Geom::Wipeout(_) | Geom::Region(_) | Geom::Table(_) | Geom::Xref(_) | Geom::Point(_) | Geom::Polyline(_) | Geom::Hatch(_) | Geom::Spline(_) | Geom::Wall(_) | Geom::Text(_) | Geom::Dimension(_) | Geom::BlockRef(_) | Geom::Leader(_) | Geom::AttrDef(_) | Geom::CenterMark(_) => Vec::new(),
            Geom::Circle(c) => plain([
                c.center + Vec2::new( c.radius, 0.0),    //   0°  east
                c.center + Vec2::new(0.0,  c.radius),    //  90°  north
                c.center + Vec2::new(-c.radius, 0.0),    // 180°  west
                c.center + Vec2::new(0.0, -c.radius),    // 270°  south
            ]),
            Geom::Arc(a) => {
                let pts = [
                    (0.0_f64,                         Vec2::new( a.radius, 0.0)),
                    (std::f64::consts::FRAC_PI_2,     Vec2::new(0.0,  a.radius)),
                    (std::f64::consts::PI,            Vec2::new(-a.radius, 0.0)),
                    (3.0 * std::f64::consts::FRAC_PI_2, Vec2::new(0.0, -a.radius)),
                ];
                pts.iter()
                    .filter(|(ang, _)| a.contains_angle(*ang))
                    .map(|(_, off)| (a.center + *off, None))
                    .collect()
            }
            Geom::Ellipse(el) => plain([
                el.point_at(0.0),                             // +major
                el.point_at(std::f64::consts::FRAC_PI_2),     // +minor
                el.point_at(std::f64::consts::PI),            // -major
                el.point_at(3.0 * std::f64::consts::FRAC_PI_2), // -minor
            ]),
            Geom::EllipseArc(ea) => {
                let params = [
                    0.0_f64,
                    std::f64::consts::FRAC_PI_2,
                    std::f64::consts::PI,
                    3.0 * std::f64::consts::FRAC_PI_2,
                ];
                params.iter()
                    .filter(|t| ea.contains_param(**t))
                    .map(|t| (ea.ellipse.point_at(*t), None))
                    .collect()
            }
            Geom::Viewport(_) => Vec::new(),
        },
        SnapKind::Nea => match nearest_point_on(e, cursor) {
            Some(p) => plain([p]),
            None    => Vec::new(),
        },
        // PER / TAN emit ONLY geometric feet (perpendicular feet for PER,
        // tangent points for TAN). For lines that's one foot; for circles
        // and arcs there are two (close and far). Each foot is either
        // "real" (anchor = None, on the dobject's visible range) or
        // "imaginary" (anchor = Some(on-dobject endpoint), on the extension —
        // UI draws the dashed line from anchor to foot).
        //
        // The on-dobject endpoint is NOT emitted as a PER/TAN candidate.
        // If the user wants to snap to an endpoint, that's what END is for.
        // Mixing them was the cause of "wrong PER" snapping to endpoints
        // instead of the perpendicular foot.
        SnapKind::Per => from
            .map(|f| perpendicular_extended(f, e))
            .unwrap_or_default(),
        SnapKind::Tan => from
            .map(|f| tangent_points_extended(f, e, cursor))
            .unwrap_or_default(),
        SnapKind::Int => Vec::new(),   // handled by the caller's pairwise loop
    }
}

// ---- one-dobject helpers ----------------------------------------------------

/// Point on the visible curve of `e` closest to `p`. Returns None only for
/// degenerate inputs (zero-length line, zero-radius circle, etc).
pub fn nearest_point_on(e: &Geom, p: Vec2) -> Option<Vec2> {
    match e {
        Geom::Line(l)        => nearest_on_line(p, l),
        Geom::Xline(x)       => {
            let d = x.dir;
            if d.len_sq() < EPS { return None; }
            Some(x.base + d * (p - x.base).dot(d) / d.len_sq())
        }
        Geom::Ray(r)         => {
            let d = r.dir;
            if d.len_sq() < EPS { return None; }
            let t = (p - r.base).dot(d) / d.len_sq();
            if t < 0.0 { Some(r.base) } else { Some(r.base + d * t) }
        }
        Geom::Donut(d)       => {
            // Nearest on the ring's two circles.
            let dv = p - d.center;
            let dir = if dv.len_sq() < EPS { Vec2::new(1.0, 0.0) } else { dv.normalized() };
            let on_outer = d.center + dir * d.outer_radius;
            if d.inner_radius <= 1e-9 { return Some(on_outer); }
            let dist_p = p.dist(d.center);
            let on_inner = d.center + dir * d.inner_radius;
            if dist_p >= d.inner_radius { Some(on_outer) } else { Some(on_inner) }
        }
        Geom::Wipeout(w)     => nearest_on_loop(p, &w.pts),
        Geom::Region(rg)     => nearest_on_loop(p, &rg.loop_pts),
        // Xref — nearest resolved child.
        Geom::Xref(x) => {
            let mut best: Option<Vec2> = None;
            let mut bd = f64::INFINITY;
            for d in &x.cached {
                if let Some(q) = nearest_point_on(&x.transform_geom(&d.geom), p) {
                    let dd = q.dist(p);
                    if dd < bd { bd = dd; best = Some(q); }
                }
            }
            best
        }
        // Table — nearest grid rule or cell text (annotation pick).
        Geom::Table(t) => {
            let mut best: Option<Vec2> = None;
            let mut bd = f64::INFINITY;
            for (a, b) in t.grid_lines() {
                if let Some(q) = nearest_on_line(p, &crate::geom::Line { a, b }) {
                    let d = q.dist(p);
                    if d < bd { bd = d; best = Some(q); }
                }
            }
            best
        }
        Geom::Circle(c)      => nearest_on_circle(p, c),
        Geom::Arc(a)         => nearest_on_arc(p, a),
        Geom::Ellipse(el)    => {
            let t = el.nearest_param(p);
            Some(el.point_at(t))
        }
        Geom::EllipseArc(ea) => {
            let t = ea.ellipse.nearest_param(p);
            if ea.contains_param(t) {
                Some(ea.ellipse.point_at(t))
            } else {
                let (e1, e2) = ea.endpoints();
                Some(if p.dist(e1) < p.dist(e2) { e1 } else { e2 })
            }
        }
        Geom::Point(pt) => Some(pt.location),
        Geom::Polyline(pl) => {
            // Project onto every segment, keep the closest foot.
            if pl.vertices.len() < 2 { return pl.vertices.first().map(|v| v.pos); }
            let n = pl.vertices.len();
            let pairs = if pl.closed { n } else { n - 1 };
            let mut best: Option<(Vec2, f64)> = None;
            for i in 0..pairs {
                let a = pl.vertices[i].pos;
                let b = pl.vertices[(i + 1) % n].pos;
                let l = Line { a, b };
                if let Some(foot) = nearest_on_line(p, &l) {
                    let d = p.dist(foot);
                    if best.map_or(true, |(_, bd)| d < bd) {
                        best = Some((foot, d));
                    }
                }
            }
            best.map(|(pt, _)| pt)
        }
        // Hatch boundary edges are the snap target via the boundary's own
        // polyline dobject. The Hatch entity itself has no curve to snap to.
        Geom::Hatch(_) => None,
        // Spline NEA = nearest point on a tessellation of the curve.
        // Good enough at typical zooms; an iterative parameter-space
        // projection (Newton on |C(u) - p|) would give sub-pixel
        // precision when we need it.
        Geom::Spline(s) => {
            let samples = s.tessellate(64);
            if samples.is_empty() { return None; }
            let mut best: Option<(Vec2, f64)> = None;
            for w in samples.windows(2) {
                let a = w[0]; let b = w[1];
                let d = b - a;
                let len_sq = d.len_sq();
                let foot = if len_sq < EPS { a }
                    else {
                        let t = ((p - a).dot(d) / len_sq).clamp(0.0, 1.0);
                        a + d * t
                    };
                let dist = p.dist(foot);
                if best.map_or(true, |(_, bd)| dist < bd) {
                    best = Some((foot, dist));
                }
            }
            best.map(|(pt, _)| pt)
        }
        // Wall — closest point on the NEARER of the two visible side lines.
        Geom::Wall(w) => {
            let l = w.left_line();
            let r = w.right_line();
            let cand_l = l.and_then(|line| nearest_on_line(p, &line)
                .map(|q| (q, q.dist(p))));
            let cand_r = r.and_then(|line| nearest_on_line(p, &line)
                .map(|q| (q, q.dist(p))));
            match (cand_l, cand_r) {
                (Some((ql, dl)), Some((qr, dr))) =>
                    Some(if dl <= dr { ql } else { qr }),
                (Some((q, _)), None) | (None, Some((q, _))) => Some(q),
                (None, None) => None,
            }
        }
        // Text — nearest point on the entity = its anchor.
        Geom::Text(t) => Some(t.position),
        // Leader — nearest point on the chain (like Polyline).
        Geom::Leader(l) => {
            if l.pts.is_empty() { return None; }
            if l.pts.len() == 1 { return Some(l.pts[0]); }
            let mut best: Option<(Vec2, f64)> = None;
            for w in l.pts.windows(2) {
                let a = w[0]; let b = w[1];
                let l2 = Line { a, b };
                if let Some(foot) = nearest_on_line(p, &l2) {
                    let d = p.dist(foot);
                    if best.map_or(true, |(_, bd)| d < bd) {
                        best = Some((foot, d));
                    }
                }
            }
            best.map(|(pt, _)| pt)
        }
        // AttrDef — nearest point = its position.
        Geom::AttrDef(a) => Some(a.position),
        // CenterMark — nearest point on the two arms.
        Geom::CenterMark(cm) => {
            let mut best: Option<(Vec2, f64)> = None;
            for (a, b) in cm.segments() {
                let l2 = Line { a, b };
                if let Some(foot) = nearest_on_line(p, &l2) {
                    let d = p.dist(foot);
                    if best.map_or(true, |(_, bd)| d < bd) {
                        best = Some((foot, d));
                    }
                }
            }
            best.map(|(pt, _)| pt)
        }
        // Dimension — nearest def point.
        Geom::Dimension(d) => {
            let mut best: Option<(Vec2, f64)> = None;
            for gp in d.grip_points() {
                let dist = gp.dist(p);
                if best.map_or(true, |(_, bd)| dist < bd) {
                    best = Some((gp, dist));
                }
            }
            best.map(|(pt, _)| pt)
        }
        // BlockRef — insertion point (contents need Document access).
        Geom::BlockRef(br) => Some(br.insert),
        Geom::Viewport(_) => None,
    }
}

fn nearest_on_line(p: Vec2, l: &Line) -> Option<Vec2> {
    let d = l.b - l.a;
    let len_sq = d.len_sq();
    if len_sq < EPS { return None; }
    let t = ((p - l.a).dot(d) / len_sq).clamp(0.0, 1.0);
    Some(l.a + d * t)
}

fn nearest_on_circle(p: Vec2, c: &Circle) -> Option<Vec2> {
    if c.radius < EPS { return None; }
    let v = p - c.center;
    let d = v.len();
    if d < EPS { return None; }   // cursor at centre — direction undefined
    Some(c.center + v * (c.radius / d))
}

fn nearest_on_arc(p: Vec2, a: &Arc) -> Option<Vec2> {
    if a.radius < EPS { return None; }
    let v = p - a.center;
    let d = v.len();
    if d < EPS {
        // cursor at centre — return arc's nearer endpoint
        let (e1, e2) = a.endpoints();
        return Some(if p.dist(e1) < p.dist(e2) { e1 } else { e2 });
    }
    let candidate = a.center + v * (a.radius / d);
    let ang = (candidate - a.center).angle();
    if a.contains_angle(ang) {
        Some(candidate)
    } else {
        // outside the sweep — clamp to nearer endpoint
        let (e1, e2) = a.endpoints();
        Some(if p.dist(e1) < p.dist(e2) { e1 } else { e2 })
    }
}

/// First in-range perpendicular foot from `from` onto the dobject. Kept only
/// for the `snap_to` compatibility wrapper. New code should iterate
/// [`perpendicular_extended`] directly to consider all geometric feet.
pub fn perpendicular_from(from: Vec2, geom: &Geom) -> Option<Vec2> {
    perpendicular_extended(from, geom).into_iter()
        .find(|(_, anchor)| anchor.is_none())
        .or_else(|| perpendicular_extended(from, geom).into_iter().next())
        .map(|(p, _)| p)
}

/// All perpendicular feet from `from` onto the dobject. For lines there is
/// exactly one (the foot on the infinite line); for circles and arcs there
/// are exactly two — the `close` foot on the side of `from` and the `far`
/// foot on the opposite side. Each foot carries an optional extension
/// anchor:
///
///   - `None` means the foot lies on the dobject's visible range — a "real"
///     snap point. The marker is drawn there without any dashed line.
///   - `Some(p)` means the foot lies on the imaginary extension — past a
///     segment endpoint, or outside an arc's swept range. The UI draws a
///     dashed extension from `p` (the on-dobject anchor) to the foot.
///
/// Emitting all feet lets the snap finder compete them on cursor distance
/// (mouse-priority). The result: the user can hover near whichever foot they
/// actually want — the in-range one or the extension one — and the snap
/// fires at that one. No more "which mode am I in?" confusion.
pub fn perpendicular_extended(from: Vec2, geom: &Geom)
    -> Vec<(Vec2, Option<Vec2>)>
{
    match geom {
        Geom::Line(l)        => per_to_line(from, l).into_iter().collect(),
        Geom::Xline(x)       => {
            let d = x.dir;
            if d.len_sq() < EPS { return Vec::new(); }
            let foot = x.base + d * (from - x.base).dot(d) / d.len_sq();
            vec![(foot, None)]
        }
        Geom::Ray(r)         => {
            let d = r.dir;
            if d.len_sq() < EPS { return Vec::new(); }
            let t = (from - r.base).dot(d) / d.len_sq();
            let foot = if t < 0.0 { r.base } else { r.base + d * t };
            vec![(foot, None)]
        }
        Geom::Donut(d)       => {
            let dv = from - d.center;
            let dir = if dv.len_sq() < EPS { Vec2::new(1.0, 0.0) } else { dv.normalized() };
            vec![(d.center + dir * d.outer_radius, None)]
        }
        Geom::Wipeout(w)     => per_to_loop(from, &w.pts),
        Geom::Region(rg)     => per_to_loop(from, &rg.loop_pts),
        // Xref — feet onto the resolved children.
        Geom::Xref(x) => {
            let mut out = Vec::new();
            for d in &x.cached {
                out.extend(perpendicular_extended(from, &x.transform_geom(&d.geom)));
            }
            out
        }
        // Table — perpendicular feet onto the nearest grid rule.
        Geom::Table(t) => {
            let mut out = Vec::new();
            for (a, b) in t.grid_lines() {
                if let Some(f) = per_to_line(from, &crate::geom::Line { a, b }) {
                    out.push(f);
                }
            }
            out
        }
        Geom::Circle(c)      => per_to_circle(from, c),
        Geom::Arc(a)         => per_to_arc(from, a),
        Geom::Ellipse(e)     => per_to_ellipse(from, e),
        Geom::EllipseArc(ea) => per_to_ellipse_arc(from, ea),
        // PER from a point to a "Point" is the point itself; conventional.
        Geom::Point(pt)      => vec![(pt.location, None)],
        // PER to a polyline = perpendicular foot on every segment;
        // candidate set, cursor distance sorts which wins.
        Geom::Polyline(p) => {
            if p.vertices.len() < 2 { return Vec::new(); }
            let n = p.vertices.len();
            let pairs = if p.closed { n } else { n - 1 };
            let mut out = Vec::new();
            for i in 0..pairs {
                let l = Line { a: p.vertices[i].pos, b: p.vertices[(i + 1) % n].pos };
                if let Some(hit) = per_to_line(from, &l) { out.push(hit); }
            }
            out
        }
        // Snap to the hatch's boundary via its own polyline dobject instead.
        Geom::Hatch(_) => Vec::new(),
        // Spline PER: per-segment perpendicular foot on a 64-sample
        // tessellation. Like the polyline branch above — good enough
        // for typical pickbox sizes; per_to_line filters by segment
        // bounds via its anchor.
        Geom::Spline(s) => {
            let samples = s.tessellate(64);
            if samples.len() < 2 { return Vec::new(); }
            let mut out = Vec::new();
            for w in samples.windows(2) {
                let l = Line { a: w[0], b: w[1] };
                if let Some(hit) = per_to_line(from, &l) { out.push(hit); }
            }
            out
        }
        // Wall PER — perpendicular feet onto BOTH visible side lines.
        Geom::Wall(w) => {
            let mut out = Vec::new();
            if let Some(l) = w.left_line() {
                if let Some(hit) = per_to_line(from, &l) { out.push(hit); }
            }
            if let Some(r) = w.right_line() {
                if let Some(hit) = per_to_line(from, &r) { out.push(hit); }
            }
            out
        }
        // Text — perpendicular foot is just the anchor.
        Geom::Text(t) => vec![(t.position, None)],
        // Leader PER — feet onto each chain segment.
        Geom::Leader(l) => {
            let mut out = Vec::new();
            for w in l.pts.windows(2) {
                let seg = Line { a: w[0], b: w[1] };
                if let Some(hit) = per_to_line(from, &seg) { out.push(hit); }
            }
            out
        }
        // AttrDef — foot is just the position.
        Geom::AttrDef(a) => vec![(a.position, None)],
        // CenterMark PER — feet onto the two arms.
        Geom::CenterMark(cm) => {
            let mut out = Vec::new();
            for (a, b) in cm.segments() {
                let seg = Line { a, b };
                if let Some(hit) = per_to_line(from, &seg) { out.push(hit); }
            }
            out
        }
        // Dimension PER — def points (anchor-less, since they're discrete).
        Geom::Dimension(d) => d.grip_points().into_iter().map(|p| (p, None)).collect(),
        // BlockRef PER — insertion point only (v1).
        Geom::BlockRef(br) => vec![(br.insert, None)],
        Geom::Viewport(_) => Vec::new(),
    }
}

/// Single perpendicular foot onto the infinite line; anchor set when the
/// foot lies past one of the segment endpoints.
fn per_to_line(from: Vec2, l: &Line) -> Option<(Vec2, Option<Vec2>)> {
    let d = l.b - l.a;
    let len_sq = d.len_sq();
    if len_sq < EPS { return None; }
    let t = (from - l.a).dot(d) / len_sq;
    let foot = l.a + d * t;
    let anchor = if t < 0.0       { Some(l.a) }
                 else if t > 1.0  { Some(l.b) }
                 else             { None };
    Some((foot, anchor))
}

/// Both perpendicular feet on the circle: `close` (same side as `from`) and
/// `far` (opposite side). Neither carries an extension anchor — every point
/// on a circle is part of the dobject, so nothing is on an "extension".
fn per_to_circle(from: Vec2, c: &Circle) -> Vec<(Vec2, Option<Vec2>)> {
    if c.radius < EPS { return Vec::new(); }
    let to_center = c.center - from;
    let dist = to_center.len();
    if dist < EPS { return Vec::new(); }
    let dir = to_center / dist;
    vec![
        (c.center - dir * c.radius, None),  // close — toward `from`
        (c.center + dir * c.radius, None),  // far  — away from `from`
    ]
}

/// Both perpendicular feet on the arc's underlying circle. Each foot carries
/// the nearer arc endpoint as its extension anchor when it falls outside the
/// swept range.
fn per_to_arc(from: Vec2, a: &Arc) -> Vec<(Vec2, Option<Vec2>)> {
    if a.radius < EPS { return Vec::new(); }
    let to_center = a.center - from;
    let dist = to_center.len();
    if dist < EPS { return Vec::new(); }
    let dir = to_center / dist;
    let close = a.center - dir * a.radius;
    let far   = a.center + dir * a.radius;
    let (e1, e2) = a.endpoints();
    let anchor_for = |p: Vec2| -> Option<Vec2> {
        if a.contains_angle((p - a.center).angle()) {
            None        // foot is on the visible arc — real
        } else {
            Some(if from.dist(e1) < from.dist(e2) { e1 } else { e2 })
        }
    };
    vec![
        (close, anchor_for(close)),
        (far,   anchor_for(far)),
    ]
}

/// Tangent points on an dobject from an external anchor point, mirroring the
/// extension semantics of [`perpendicular_extended`]: when the natural
/// tangent point falls outside the dobject's visible range, return it anyway
/// and emit the nearer on-curve anchor for the UI's dashed extension line.
///
/// - Line: a line is its own tangent — fall back to perpendicular foot.
/// - Circle: from external point P (|PC| > r), two tangent points exist;
///   from on-circle, one (= P itself); from inside, none.
/// - Arc: tangent against the underlying circle, anchored on the nearer arc
///   endpoint if all candidates fall outside the swept range.
pub fn tangent_points_extended(from: Vec2, e: &Geom, _cursor: Vec2)
    -> Vec<(Vec2, Option<Vec2>)>
{
    match e {
        Geom::Line(l) => per_to_line(from, l).into_iter().collect(),
        Geom::Xline(x) => {
            let d = x.dir;
            if d.len_sq() < EPS { return Vec::new(); }
            let foot = x.base + d * (from - x.base).dot(d) / d.len_sq();
            vec![(foot, None)]
        }
        Geom::Ray(r) => {
            let d = r.dir;
            if d.len_sq() < EPS { return Vec::new(); }
            let t = (from - r.base).dot(d) / d.len_sq();
            let foot = if t < 0.0 { r.base } else { r.base + d * t };
            vec![(foot, None)]
        }
        Geom::Donut(d) => {
            let dv = from - d.center;
            let dir = if dv.len_sq() < EPS { Vec2::new(1.0, 0.0) } else { dv.normalized() };
            vec![(d.center + dir * d.outer_radius, None)]
        }
        Geom::Wipeout(w) => per_to_loop(from, &w.pts),
        Geom::Region(rg) => per_to_loop(from, &rg.loop_pts),
        // Xref — tangents onto the resolved children.
        Geom::Xref(x) => {
            let mut out = Vec::new();
            for d in &x.cached {
                out.extend(tangent_points_extended(from, &x.transform_geom(&d.geom), _cursor));
            }
            out
        }
        // Table — line-like: perpendicular foot on the nearest rule.
        Geom::Table(t) => {
            let mut out = Vec::new();
            for (a, b) in t.grid_lines() {
                if let Some(f) = per_to_line(from, &crate::geom::Line { a, b }) {
                    out.push(f);
                }
            }
            out
        }
        Geom::Circle(c) => tangent_to_circle(from, c.center, c.radius)
            .into_iter().map(|p| (p, None)).collect(),
        Geom::Arc(a) => {
            // Emit BOTH tangent points always. Each carries the nearer arc
            // endpoint as its extension anchor when it falls outside the
            // swept range. Mouse-priority disambiguates among them.
            let tangs = tangent_to_circle(from, a.center, a.radius);
            let (e1, e2) = a.endpoints();
            tangs.into_iter().map(|p| {
                if a.contains_angle((p - a.center).angle()) {
                    (p, None)
                } else {
                    let anchor = if from.dist(e1) < from.dist(e2) { e1 } else { e2 };
                    (p, Some(anchor))
                }
            }).collect()
        }
        Geom::Ellipse(e)     => tan_to_ellipse(from, e),
        Geom::EllipseArc(ea) => tan_to_ellipse_arc(from, ea),
        // No tangent concept for Point / Polyline straight segments —
        // fall back to perpendicular for polyline segments (same as Line).
        Geom::Point(pt) => vec![(pt.location, None)],
        Geom::Polyline(p) => {
            if p.vertices.len() < 2 { return Vec::new(); }
            let n = p.vertices.len();
            let pairs = if p.closed { n } else { n - 1 };
            let mut out = Vec::new();
            for i in 0..pairs {
                let l = Line { a: p.vertices[i].pos, b: p.vertices[(i + 1) % n].pos };
                if let Some(hit) = per_to_line(from, &l) { out.push(hit); }
            }
            out
        }
        // TAN to a hatch isn't defined — use the boundary polyline directly.
        Geom::Hatch(_) => Vec::new(),
        // TAN to a spline: same polyline fallback (per-segment
        // perpendicular foot). True NURBS tangent solver lands when
        // someone needs it.
        Geom::Spline(s) => {
            let samples = s.tessellate(64);
            if samples.len() < 2 { return Vec::new(); }
            let mut out = Vec::new();
            for w in samples.windows(2) {
                let l = Line { a: w[0], b: w[1] };
                if let Some(hit) = per_to_line(from, &l) { out.push(hit); }
            }
            out
        }
        // Wall TAN — same as line fallback on each side line (a line
        // is its own tangent; this gives the perpendicular foot).
        Geom::Wall(w) => {
            let mut out = Vec::new();
            if let Some(l) = w.left_line() {
                if let Some(hit) = per_to_line(from, &l) { out.push(hit); }
            }
            if let Some(r) = w.right_line() {
                if let Some(hit) = per_to_line(from, &r) { out.push(hit); }
            }
            out
        }
        // Text — no tangent concept; anchor as the only candidate.
        Geom::Text(t) => vec![(t.position, None)],
        // Leader — no tangent concept; feet onto each chain segment.
        Geom::Leader(l) => {
            let mut out = Vec::new();
            for w in l.pts.windows(2) {
                let seg = Line { a: w[0], b: w[1] };
                if let Some(hit) = per_to_line(from, &seg) { out.push(hit); }
            }
            out
        }
        // AttrDef — no tangent concept; position as candidate.
        Geom::AttrDef(a) => vec![(a.position, None)],
        // CenterMark — no tangent concept; feet onto the arms as candidates.
        Geom::CenterMark(cm) => {
            let mut out = Vec::new();
            for (a, b) in cm.segments() {
                let seg = Line { a, b };
                if let Some(hit) = per_to_line(from, &seg) { out.push(hit); }
            }
            out
        }
        // Dimension — no tangent concept; def points as candidates.
        Geom::Dimension(d) => d.grip_points().into_iter().map(|p| (p, None)).collect(),
        // BlockRef — no tangent concept; insertion point as candidate.
        Geom::BlockRef(br) => vec![(br.insert, None)],
        Geom::Viewport(_) => Vec::new(),
    }
}

// ---- Ellipse PER / TAN -----------------------------------------------------
//
// Both reduce to "find all t in [0, 2π) such that g(t) = 0" on the ellipse's
// parameter circle. For PER, g(t) = (E(t) - from) · T(t). For TAN,
// g(t) = (from - E(t)) × T(t) (the 2D cross product). Each can have up to
// four roots; we seed Newton from 8 equally-spaced starts and dedupe.

/// All perpendicular feet from `from` onto the full ellipse curve. Each is a
/// "real" snap target (anchor = None) because the entire ellipse is in range.
fn per_to_ellipse(from: Vec2, el: &Ellipse) -> Vec<(Vec2, Option<Vec2>)> {
    if el.semi_major() < EPS { return Vec::new(); }
    let a = el.semi_major();
    let b = el.semi_minor();
    let u = el.u_hat();
    let v = el.v_hat();
    // f(t)  = (E(t) - from) · E'(t)
    // f'(t) = (E(t) - from) · E''(t) + E'(t) · E'(t)
    let f = |t: f64| {
        let pt = el.point_at(t);
        let dp = el.tangent_at(t);
        (pt - from).dot(dp)
    };
    let fd = |t: f64| {
        let pt = el.point_at(t);
        let dp = el.tangent_at(t);
        // E''(t) = -a·cos(t)·û − b·sin(t)·v̂
        let d2 = u * (-a * t.cos()) + v * (-b * t.sin());
        (pt - from).dot(d2) + dp.dot(dp)
    };
    // FIX 1 (B16, G1 family): the residual f = (E(t) − from) · E'(t) is a DOT of
    // two length vectors → length², so it scales with coordinate² and a fixed 1e-6
    // rejects real feet at large scale. `char` must bound BOTH factors of the
    // product: |E'| scales with the ellipse (≈ a), but |E(t) − from| scales with
    // how far the ANCHOR sits from the curve — and `from` is a user click, which
    // can be arbitrarily distant. So char = a.max(from.dist(center)): a
    // semi_major-ONLY char under-scales for a distant anchor on a small ellipse
    // and rejects real feet — the G1 failure reached from the other side. The
    // residual stays length²-scaled — do NOT "simplify" it back to a constant.
    let char = a.max(from.dist(el.center));
    let residual_tol = 1e-6 * (char * char).max(1.0);
    let dedup_tol = scaled_tol(a) / a;   // G7: param-space dedup, a = |E'| char (guarded > EPS)
    newton_roots_periodic(f, fd, 16, residual_tol, dedup_tol).into_iter()
        .map(|t| (el.point_at(t), None))
        .collect()
}

/// Perpendicular feet on an elliptical arc: same feet as the underlying
/// ellipse, but feet that fall outside the swept range carry the nearer arc
/// endpoint as their extension anchor (so the UI draws the dashed cue).
fn per_to_ellipse_arc(from: Vec2, ea: &EllipseArc) -> Vec<(Vec2, Option<Vec2>)> {
    let (e1, e2) = ea.endpoints();
    per_to_ellipse(from, &ea.ellipse).into_iter()
        .map(|(p, _)| {
            let t = ea.ellipse.nearest_param(p);
            if ea.contains_param(t) {
                (p, None)
            } else {
                let anchor = if from.dist(e1) < from.dist(e2) { e1 } else { e2 };
                (p, Some(anchor))
            }
        })
        .collect()
}

/// All tangent points on the full ellipse from an external anchor.
/// Geometry: at a tangent point T, the chord (T - from) is parallel to the
/// tangent vector. The 2D cross product is zero:
///     (from - E(t)) × E'(t) = 0
/// — a polynomial of degree ≤ 4 in (cos t, sin t). Up to 2 roots for points
/// outside the ellipse, 0 for points inside, 1 (degenerate) on the curve.
fn tan_to_ellipse(from: Vec2, el: &Ellipse) -> Vec<(Vec2, Option<Vec2>)> {
    if el.semi_major() < EPS { return Vec::new(); }
    let a = el.semi_major();
    let b = el.semi_minor();
    let u = el.u_hat();
    let v = el.v_hat();
    // f(t)  = (from - E(t)) × E'(t)
    //       = (from.x - E.x) · E'.y - (from.y - E.y) · E'.x
    // f'(t) = -E'(t) × E'(t) + (from - E(t)) × E''(t)
    //       =       0       + (from - E(t)) × E''(t)
    let cross = |p: Vec2, q: Vec2| p.x * q.y - p.y * q.x;
    let f = |t: f64| {
        let pt = el.point_at(t);
        let dp = el.tangent_at(t);
        cross(from - pt, dp)
    };
    let fd = |t: f64| {
        let pt = el.point_at(t);
        let d2 = u * (-a * t.cos()) + v * (-b * t.sin());
        cross(from - pt, d2)
    };
    // FIX 1 (B16, G1 family): the residual f = (from − E(t)) × E'(t) is a CROSS of
    // two length vectors → length², the same scaling as `per_to_ellipse`. `char`
    // must bound both |from − E(t)| (grows with the anchor distance — `from` is a
    // possibly-distant user click) and |E'| (≈ a), so char = a.max(from.dist(
    // center)). Length²-scaled — do NOT revert it to a constant.
    let char = a.max(from.dist(el.center));
    let residual_tol = 1e-6 * (char * char).max(1.0);
    let dedup_tol = scaled_tol(a) / a;   // G7: param-space dedup, a = |E'| char (guarded > EPS)
    newton_roots_periodic(f, fd, 16, residual_tol, dedup_tol).into_iter()
        .map(|t| (el.point_at(t), None))
        .collect()
}

fn tan_to_ellipse_arc(from: Vec2, ea: &EllipseArc) -> Vec<(Vec2, Option<Vec2>)> {
    let (e1, e2) = ea.endpoints();
    tan_to_ellipse(from, &ea.ellipse).into_iter()
        .map(|(p, _)| {
            let t = ea.ellipse.nearest_param(p);
            if ea.contains_param(t) {
                (p, None)
            } else {
                let anchor = if from.dist(e1) < from.dist(e2) { e1 } else { e2 };
                (p, Some(anchor))
            }
        })
        .collect()
}

/// Tangent point(s) from `from` to a circle (centre, radius).
/// Geometry: tangent point T satisfies (T - centre) · (T - from) = 0, |T -
/// centre| = r. Equivalently, T lies on the circle of diameter
/// [centre, from] intersected with the original circle.
fn tangent_to_circle(from: Vec2, centre: Vec2, r: f64) -> Vec<Vec2> {
    if r < EPS { return Vec::new(); }
    let v = from - centre;
    let d2 = v.len_sq();
    let r2 = r * r;
    if d2 < r2 - EPS { return Vec::new(); }     // inside — no tangent
    if (d2 - r2).abs() < EPS {
        // on the circle — single tangent at `from` itself
        return vec![from];
    }
    let d = d2.sqrt();
    // angle between the line (centre→from) and (centre→T)
    let a = (r / d).acos();
    let base = v.angle();        // centre→from
    let t1 = base + a;
    let t2 = base - a;
    vec![
        centre + Vec2::new(r * t1.cos(), r * t1.sin()),
        centre + Vec2::new(r * t2.cos(), r * t2.sin()),
    ]
}

// ---- public single-snap entry point (kept for typed-override convenience)

/// Snap from a known anchor point to the dobject using the given snap kind.
/// Kept as a thin compatibility wrapper for callers that already have the
/// dobject in hand. The returned point may lie on an extension of the dobject
/// (e.g. PER foot on the infinite line beyond a segment endpoint).
pub fn snap_to(kind: SnapKind, from: Vec2, geom: &Geom) -> Option<Vec2> {
    candidate_points(kind, geom, from, Some(from))
        .into_iter().next().map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::approx_eq;

    fn close(p: Vec2, x: f64, y: f64) -> bool {
        approx_eq(p.x, x) && approx_eq(p.y, y)
    }

    // P2: a STALE spatial grid (built before the doc shrank — delete-then-hover)
    // yields candidate indices past the current slice. `find_all_snaps`/`find_snap`
    // must filter them, not index OOB and panic.
    #[test]
    fn stale_grid_indices_do_not_panic() {
        let mut dobjects: Vec<DObject> = Vec::new();
        for i in 0..6 {
            dobjects.push(DObject::new(Geom::Circle(Circle {
                center: Vec2::new(i as f64 * 0.1, 0.0), radius: 1.0,
            })));
        }
        let grid = UniformGrid::build(&dobjects, 1.0);   // knows all 6
        let shorter = &dobjects[..dobjects.len() - 2];   // doc "shrank" by 2
        let all = SnapSet { end: true, mid: true, cen: true, qua: true,
                            int: true, per: true, tan: true, nea: true };
        // No panic on the OOB indices the stale grid returns.
        let _ = find_all_snaps(Vec2::ZERO, 5.0, all, None, None, shorter, Some(&grid));
        let _ = find_snap(Vec2::ZERO, 5.0, all, None, None, shorter, Some(&grid));
    }

    #[test]
    fn per_to_horizontal_line_in_segment() {        let l = Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) };
        let (p, anchor) = per_to_line(Vec2::new(3.0, 4.0), &l).unwrap();
        assert!(close(p, 3.0, 0.0));
        assert!(anchor.is_none(), "foot is inside the segment, no extension");
    }

    #[test]
    fn per_to_line_extends_beyond_endpoint_with_anchor() {
        let l = Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) };
        let (p, anchor) = per_to_line(Vec2::new(-5.0, 4.0), &l).unwrap();
        assert!(close(p, -5.0, 0.0), "foot is on the INFINITE line, not clamped");
        assert!(close(anchor.unwrap(), 0.0, 0.0), "anchor is the nearer endpoint");
    }

    #[test]
    fn per_to_ellipse_axis_aligned_returns_axis_feet() {
        // Axis-aligned ellipse a=5, b=2. From (10, 0) on the +x axis, the
        // perpendicular feet on the ellipse are the two x-axis intersections
        // — (5, 0) and (-5, 0).
        let el = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 };
        let pts = per_to_ellipse(Vec2::new(10.0, 0.0), &el);
        assert!(pts.len() >= 2, "got {}", pts.len());
        let coords: Vec<(f64, f64)> = pts.iter().map(|(p, _)| (p.x, p.y)).collect();
        assert!(coords.iter().any(|&(x, y)| (x - 5.0).abs() < 1e-6 && y.abs() < 1e-6));
        assert!(coords.iter().any(|&(x, y)| (x + 5.0).abs() < 1e-6 && y.abs() < 1e-6));
    }

    #[test]
    fn tan_to_ellipse_from_outside_returns_two_tangents() {
        // From (10, 0), two tangents touch the axis-aligned a=5, b=2 ellipse
        // symmetrically about the x-axis.
        let el = Ellipse { center: Vec2::ZERO, major: Vec2::new(5.0, 0.0), ratio: 0.4 };
        let pts = tan_to_ellipse(Vec2::new(10.0, 0.0), &el);
        assert!(pts.len() >= 2, "got {}", pts.len());
        // Each tangent point T should satisfy: (P - T) parallel to E'(T).
        for (p, _) in &pts {
            let t = el.nearest_param(*p);
            let tangent = el.tangent_at(t);
            let chord = Vec2::new(10.0, 0.0) - *p;
            // 2D cross product zero means parallel.
            assert!((chord.x * tangent.y - chord.y * tangent.x).abs() < 1e-4,
                "tangent point {:?} not collinear with tangent line", p);
        }
    }

    // ---- FIX 1 (B16): snap residuals are length²-scaled ---------------------
    // MEASURED: the residual only binds when the ELLIPSE is large (a big residual
    // derivative). For a small ellipse the Newton residual stays < 1e-6 at any
    // scale, so those cases are vacuous. Discriminator below uses a=1e6 at 1e10:
    // fixed-1e-6 → 0 feet, char² → 2. (`a.max(dist)` vs `a` alone never differed
    // observably in testing — it's belt-and-braces; see the report.)

    // Scale-free perpendicularity: cos(angle between (foot−from) and tangent) ≈ 0.
    fn is_perp_foot(from: Vec2, foot: Vec2, el: &Ellipse) -> bool {
        let t = el.nearest_param(foot);
        let tan = el.tangent_at(t);
        let chord = foot - from;
        (chord.dot(tan) / (chord.len() * tan.len())).abs() < 1e-6
    }

    #[test]
    fn per_feet_found_on_large_ellipse_at_scale() {
        // a=1e6 ellipse at 1e10 coords. The residual at the true feet exceeds
        // 1e-6, so fixed-1e-6 returns EMPTY; the char²-scaled threshold finds both
        // feet. (Fails-before verified: revert residual_tol → 1e-6 → empty.)
        let el = Ellipse { center: Vec2::new(1.0e10, 1.0e10),
                           major: Vec2::new(1.0e6, 0.0), ratio: 0.6 };
        let from = Vec2::new(1.0e10 + 3.0e6, 1.0e10 + 1.7e6);
        let pts = per_to_ellipse(from, &el);
        assert!(!pts.is_empty(),
            "perpendicular feet rejected on a large ellipse at scale (residual > 1e-6)");
        for (foot, _) in &pts {
            assert!(is_perp_foot(from, *foot, &el),
                "foot {foot:?} is not a true perpendicular foot");
        }
    }

    #[test]
    fn tan_points_found_on_large_ellipse_at_scale() {
        // Same regime for the tangent cross residual.
        let el = Ellipse { center: Vec2::new(1.0e10, 1.0e10),
                           major: Vec2::new(1.0e6, 0.0), ratio: 0.6 };
        let from = Vec2::new(1.0e10 + 5.0e6, 1.0e10 + 3.0e6);   // outside → tangents
        let pts = tan_to_ellipse(from, &el);
        assert!(!pts.is_empty(),
            "tangent points rejected on a large ellipse at scale (residual > 1e-6)");
        for (tp, _) in &pts {
            let t = el.nearest_param(*tp);
            let tan = el.tangent_at(t);
            let chord = from - *tp;
            let s = (chord.x * tan.y - chord.y * tan.x) / (chord.len() * tan.len());
            assert!(s.abs() < 1e-6, "tangent point {tp:?} not parallel (sin={s})");
        }
    }

    #[test]
    fn per_to_circle_returns_both_feet() {
        let c = Circle { center: Vec2::new(0.0, 0.0), radius: 5.0 };
        let pts = per_to_circle(Vec2::new(20.0, 0.0), &c);
        assert_eq!(pts.len(), 2);
        // Close foot is on the same side as `from`: (5, 0).
        // Far foot is on the opposite side: (-5, 0).
        assert!(close(pts[0].0,  5.0, 0.0) && pts[0].1.is_none());
        assert!(close(pts[1].0, -5.0, 0.0) && pts[1].1.is_none());
    }

    #[test]
    fn per_to_arc_emits_both_feet_with_anchors() {
        let a = Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_2,    // 0..90° (NE quadrant)
        };
        // From (20, 0): dir = (-1, 0). close = (5, 0) on arc (angle 0°);
        // far = (-5, 0) NOT on arc (angle 180°). Far's anchor is the nearer
        // endpoint to from — (5,0).
        let pts = per_to_arc(Vec2::new(20.0, 0.0), &a);
        assert_eq!(pts.len(), 2);
        let close_pt = pts.iter().find(|(p, _)| p.x > 0.0).unwrap();
        let far_pt   = pts.iter().find(|(p, _)| p.x < 0.0).unwrap();
        assert!(close(close_pt.0, 5.0, 0.0));
        assert!(close_pt.1.is_none(), "close foot is on the arc — real");
        assert!(close(far_pt.0, -5.0, 0.0));
        assert!(far_pt.1.is_some(), "far foot is off the arc — imaginary");
        assert!(close(far_pt.1.unwrap(), 5.0, 0.0));
    }

    #[test]
    fn nearest_on_circle_projects_radially() {
        let c = Circle { center: Vec2::ZERO, radius: 5.0 };
        let p = nearest_on_circle(Vec2::new(8.0, 0.0), &c).unwrap();
        assert!(close(p, 5.0, 0.0));
        let p = nearest_on_circle(Vec2::new(3.0, 0.0), &c).unwrap();
        assert!(close(p, 5.0, 0.0));     // inside also snaps OUT to circle
    }

    #[test]
    fn tangent_from_external_point_to_circle() {
        // circle r=3 at origin, external point at (5,0); tangents at angles
        // ±acos(3/5) ≈ ±53.13° from the centre→point direction.
        let pts = tangent_to_circle(Vec2::new(5.0, 0.0), Vec2::ZERO, 3.0);
        assert_eq!(pts.len(), 2);
        for p in pts {
            // tangent length squared = d² - r² = 25 - 9 = 16
            assert!(approx_eq(p.len(), 3.0));    // on circle
            let v = Vec2::new(5.0, 0.0) - p;
            assert!(approx_eq(v.dot(p), 0.0));   // perpendicular to radius
        }
    }

    #[test]
    fn snapkind_parse_case_insensitive() {
        assert_eq!(SnapKind::parse("per"),  Some(SnapKind::Per));
        assert_eq!(SnapKind::parse("PER"),  Some(SnapKind::Per));
        assert_eq!(SnapKind::parse("Perp"), Some(SnapKind::Per));
        assert_eq!(SnapKind::parse("end"),  Some(SnapKind::End));
        assert_eq!(SnapKind::parse("Endpoint"), Some(SnapKind::End));
        assert_eq!(SnapKind::parse("zzz"),  None);
    }

    #[test]
    fn per_on_line_emits_only_the_perpendicular_foot() {
        // Segment [0,0]→[10,0]; anchor at (15, 5). Foot on the infinite
        // line is at (15, 0). PER must emit ONLY this foot — not the
        // endpoint. (Endpoint snapping is END's job.)
        let g = Geom::Line(Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        let anchor = Vec2::new(15.0, 5.0);
        let pts = candidate_points(SnapKind::Per, &g, anchor, Some(anchor));
        assert_eq!(pts.len(), 1);
        assert!(close(pts[0].0, 15.0, 0.0));
        assert!(pts[0].1.is_some(), "foot is past the endpoint — extension anchor present");
        assert!(close(pts[0].1.unwrap(), 10.0, 0.0));
    }

    #[test]
    fn per_fires_when_cursor_is_on_segment_even_if_foot_is_far() {
        // Object-priority: hovering ANYWHERE on the segment is enough to
        // invoke PER. The snap point lands at the geometric foot, wherever
        // it is — here past the right endpoint, so the dashed extension
        // line will be drawn.
        let ents: Vec<DObject> = vec![Line {
            a: Vec2::ZERO, b: Vec2::new(10.0, 0.0),
        }.into()];
        let mut set = SnapSet::default();
        set.per = true;
        let from   = Vec2::new(15.0, 5.0);
        let cursor = Vec2::new(5.0, 0.1);   // mid-segment, far from foot
        let hit = find_snap(cursor, 1.0, set, None, Some(from), &ents, None).unwrap();
        assert_eq!(hit.kind, SnapKind::Per);
        assert!(close(hit.point, 15.0, 0.0), "snap at the geometric foot");
        assert!(hit.extension_anchor.is_some());
    }

    #[test]
    fn per_imaginary_fires_when_cursor_is_at_the_extension_foot() {
        // Same line + anchor, cursor at the imaginary foot (15.1, 0.1).
        let ents: Vec<DObject> = vec![Line {
            a: Vec2::ZERO, b: Vec2::new(10.0, 0.0),
        }.into()];
        let mut set = SnapSet::default();
        set.per = true;
        let from   = Vec2::new(15.0, 5.0);
        let cursor = Vec2::new(15.1, 0.1);
        let hit = find_snap(cursor, 1.0, set, None, Some(from), &ents, None).unwrap();
        assert_eq!(hit.kind, SnapKind::Per);
        assert!(close(hit.point, 15.0, 0.0));
        assert!(hit.extension_anchor.is_some());
        assert!(close(hit.extension_anchor.unwrap(), 10.0, 0.0));
    }

    #[test]
    fn cen_fires_when_cursor_is_on_the_curve_far_from_centre() {
        // A circle r=10 at origin. Cursor at (9.9, 0.5) — hovering the curve
        // near the right side, NOT near the centre. CEN must still fire and
        // return the centre (0, 0).
        let ents: Vec<DObject> = vec![Circle {
            center: Vec2::ZERO, radius: 10.0,
        }.into()];
        let mut set = SnapSet::default();
        set.cen = true;
        let cursor = Vec2::new(9.9, 0.5);
        let hit = find_snap(cursor, 1.0, set, None, None, &ents, None).unwrap();
        assert_eq!(hit.kind, SnapKind::Cen);
        assert!(close(hit.point, 0.0, 0.0), "CEN snaps to the centre, not to cursor");
    }

    #[test]
    fn find_all_snaps_offers_cen_and_nea_on_arc_for_tab_cycling() {
        // Arc 0..180° at origin r=5. All "intrinsic" snaps enabled. Cursor
        // sits ON the arc at angle 45° — far from every endpoint, midpoint,
        // and quadrant. The user's frustrating scenario: only CEN and NEA
        // are reachable, so without Tab they'd always get CEN.
        let ents: Vec<DObject> = vec![Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::PI,
        }.into()];
        let mut set = SnapSet::default();
        set.end = true; set.mid = true; set.cen = true;
        set.qua = true; set.nea = true;
        let r45 = 5.0 / std::f64::consts::SQRT_2;
        let hits = find_all_snaps(
            Vec2::new(r45, r45), 1.0, set, None, None, &ents, None,
        );
        // Default snap (hits[0]) is CEN (priority 2), Tab gives NEA (priority 7).
        assert_eq!(hits.len(), 2, "expected exactly CEN + NEA, got {:?}",
            hits.iter().map(|h| h.kind).collect::<Vec<_>>());
        assert_eq!(hits[0].kind, SnapKind::Cen);
        assert_eq!(hits[1].kind, SnapKind::Nea);
        // CEN snaps to the centre (far from cursor); NEA snaps to the
        // projection-on-curve which equals the cursor itself.
        assert!(close(hits[0].point, 0.0, 0.0));
        assert!(close(hits[1].point, r45, r45));
    }

    #[test]
    fn qua_offers_all_four_compass_points_on_a_circle() {
        let g = Geom::Circle(Circle { center: Vec2::ZERO, radius: 5.0 });
        let pts = candidate_points(SnapKind::Qua, &g, Vec2::ZERO, None);
        assert_eq!(pts.len(), 4);
        let coords: Vec<(f64, f64)> = pts.iter().map(|(p, _)| (p.x, p.y)).collect();
        assert!(coords.contains(&( 5.0,  0.0)));   // east
        assert!(coords.contains(&( 0.0,  5.0)));   // north
        assert!(coords.contains(&(-5.0,  0.0)));   // west
        assert!(coords.contains(&( 0.0, -5.0)));   // south
    }

    #[test]
    fn qua_filters_by_arc_swept_range() {
        // arc 0..90° — only east (0°) and north (90°) are visible
        let g = Geom::Arc(Arc {
            center: Vec2::ZERO, radius: 5.0,
            start_angle: 0.0, sweep_angle: std::f64::consts::FRAC_PI_2,
        });
        let pts = candidate_points(SnapKind::Qua, &g, Vec2::ZERO, None);
        // 0° and 90° both in [0, π/2] (with EPS slack inside contains_angle)
        assert!(pts.len() >= 2, "east and north should be visible");
        let coords: Vec<(f64, f64)> = pts.iter().map(|(p, _)| (p.x, p.y)).collect();
        assert!(coords.iter().any(|&(x, y)| close(Vec2::new(x, y), 5.0, 0.0)));
        assert!(coords.iter().any(|&(x, y)| close(Vec2::new(x, y), 0.0, 5.0)));
        assert!(!coords.iter().any(|&(x, y)| close(Vec2::new(x, y), -5.0, 0.0)));
        assert!(!coords.iter().any(|&(x, y)| close(Vec2::new(x, y), 0.0, -5.0)));
    }

    #[test]
    fn cen_does_not_fire_when_cursor_is_far_from_curve() {
        // Cursor inside the circle but >1 unit from the curve → CEN must NOT
        // fire (would otherwise trigger from anywhere inside the dobject).
        let ents: Vec<DObject> = vec![Circle {
            center: Vec2::ZERO, radius: 10.0,
        }.into()];
        let mut set = SnapSet::default();
        set.cen = true;
        let cursor = Vec2::new(3.0, 0.0);   // 7 units from curve
        let hit = find_snap(cursor, 1.0, set, None, None, &ents, None);
        assert!(hit.is_none());
    }

    #[test]
    fn find_snap_picks_endpoint_over_midpoint() {
        // Two enabled: end + mid. Cursor very close to endpoint of a line.
        let ents: Vec<DObject> = vec![
            Line { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) }.into(),
        ];
        let mut set = SnapSet::default();
        set.end = true; set.mid = true;
        let hit = find_snap(
            Vec2::new(0.1, 0.1), 1.0, set, None, None, &ents, None,
        ).unwrap();
        assert_eq!(hit.kind, SnapKind::End);
        assert!(close(hit.point, 0.0, 0.0));
    }

    #[test]
    fn open_polyline_end_snap_fires_on_a_middle_vertex() {
        // An OPEN polyline's ENDpoint osnap must land on EVERY vertex, not
        // just the first/last — so a pline-in-progress snaps to each already
        // placed vertex during the ghost preview. Cursor near the MIDDLE
        // vertex (10,0) of a 3-vertex open polyline.
        use crate::geom::{Polyline, PolyVertex};
        let ents: Vec<DObject> = vec![Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0),  bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(10.0, 10.0), bulge: 0.0 },
            ],
            closed: false,
            widths: Vec::new(),
        }.into()];
        let mut set = SnapSet::default();
        set.end = true;
        let hit = find_snap(
            Vec2::new(10.05, 0.05), 1.0, set, None, None, &ents, None,
        ).expect("END snap should fire on the middle vertex");
        assert_eq!(hit.kind, SnapKind::End);
        assert!(close(hit.point, 10.0, 0.0));
    }

    #[test]
    fn leader_end_mid_nea_and_per_snaps() {
        // MLEADER chain (1,1)→(6,4)→(12,5): END at every vertex, MID on
        // every segment, NEA on the curve, PER foot on the segments.
        let l = crate::text::Leader {
            pts: vec![
                Vec2::new(1.0, 1.0),
                Vec2::new(6.0, 4.0),
                Vec2::new(12.0, 5.0),
            ],
            label: crate::text::Text::empty(),
            arrow: true,
        };
        let g = Geom::Leader(l);
        // END — all three vertices.
        let ends = candidate_points(SnapKind::End, &g, Vec2::ZERO, None);
        assert_eq!(ends.len(), 3);
        assert!(ends.iter().any(|(p, _)| close(*p, 6.0, 4.0)));
        // MID — two segment midpoints.
        let mids = candidate_points(SnapKind::Mid, &g, Vec2::ZERO, None);
        assert_eq!(mids.len(), 2);
        assert!(mids.iter().any(|(p, _)| close(*p, 3.5, 2.5)));
        // NEA — nearest point on the chain (a foot slightly past (6,4)
        // on the second segment).
        let nea = nearest_point_on(&g, Vec2::new(6.1, 3.9));
        assert!(nea.is_some());
        let n = nea.unwrap();
        assert!((n - Vec2::new(6.0, 4.0)).len() < 0.1);
        assert!(n.x > 6.0 - 1e-9 && n.y > 4.0 - 1e-9);
        // PER — a foot onto the first segment.
        let pers = candidate_points(SnapKind::Per, &g, Vec2::new(1.0, 4.0), Some(Vec2::ZERO));
        assert!(!pers.is_empty());
        // CEN — none (a leader has no canonical centre).
        assert!(candidate_points(SnapKind::Cen, &g, Vec2::ZERO, None).is_empty());
    }

    #[test]
    fn attdef_snaps_like_text() {
        let ad = crate::block::AttrDef {
            tag: "TAG".into(), prompt: String::new(), default: String::new(),
            position: Vec2::new(3.0, 3.0), height: 0.5, angle: 0.0,
            style: 0, visible: true,
        };
        let g = Geom::AttrDef(ad);
        let ends = candidate_points(SnapKind::End, &g, Vec2::ZERO, None);
        assert_eq!(ends.len(), 1);
        assert!(close(ends[0].0, 3.0, 3.0));
        assert!(candidate_points(SnapKind::Mid, &g, Vec2::ZERO, None).is_empty());
        assert!(candidate_points(SnapKind::Qua, &g, Vec2::ZERO, None).is_empty());
        assert_eq!(nearest_point_on(&g, Vec2::new(3.2, 3.1)),
            Some(Vec2::new(3.0, 3.0)));
    }

    #[test]
    fn text_snaps_at_anchor() {
        // Text has no curve geometry — its ANCHOR is the END/CEN candidate,
        // the NEA hit, and the PER foot.
        let t = crate::text::Text {
            position: Vec2::new(4.0, 5.0),
            ..crate::text::Text::empty()
        };
        let g = Geom::Text(t);
        let ends = candidate_points(SnapKind::End, &g, Vec2::ZERO, None);
        assert_eq!(ends.len(), 1);
        assert!(close(ends[0].0, 4.0, 5.0));
        let cens = candidate_points(SnapKind::Cen, &g, Vec2::ZERO, None);
        assert_eq!(cens.len(), 1);
        assert!(close(cens[0].0, 4.0, 5.0));
        assert_eq!(nearest_point_on(&g, Vec2::new(4.2, 5.1)),
            Some(Vec2::new(4.0, 5.0)));
        let pers = candidate_points(SnapKind::Per, &g, Vec2::new(0.0, 0.0), Some(Vec2::ZERO));
        assert_eq!(pers.len(), 1);
        assert!(close(pers[0].0, 4.0, 5.0));
        assert!(candidate_points(SnapKind::Mid, &g, Vec2::ZERO, None).is_empty());
        assert!(candidate_points(SnapKind::Qua, &g, Vec2::ZERO, None).is_empty());
    }
}

/// Nearest point ON a closed vertex loop (edge-projection, boundary counts).
fn nearest_on_loop(p: Vec2, pts: &[Vec2]) -> Option<Vec2> {
    let n = pts.len();
    if n == 0 { return None; }
    if n == 1 { return Some(pts[0]); }
    let mut best: Option<(Vec2, f64)> = None;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let d = b - a;
        let l2 = d.len_sq();
        if l2 < EPS { continue; }
        let t = ((p - a).dot(d) / l2).clamp(0.0, 1.0);
        let foot = a + d * t;
        let dist = foot.dist(p);
        if best.map_or(true, |(_, bd)| dist < bd) { best = Some((foot, dist)); }
    }
    best.map(|(f, _)| f)
}

/// Perpendicular feet from `from` onto a closed loop's edges.
fn per_to_loop(from: Vec2, pts: &[Vec2]) -> Vec<(Vec2, Option<Vec2>)> {
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let d = b - a;
        let l2 = d.len_sq();
        if l2 < EPS { continue; }
        let t = ((from - a).dot(d) / l2).clamp(0.0, 1.0);
        out.push((a + d * t, None));
    }
    out
}
