//! Wall junction solver — smart-dobject category, member #1.
//!
//! A wall is the offset of its centerline by ±thickness/2 (`Geom::Wall`
//! stores the centerline as identity and derives the two face lines). When
//! two walls share an endpoint (a "node"), their derived faces are MITRED
//! at that node instead of overlapping. This is **Model A**: walls stay
//! independent dobjects; the join is recomputed every frame from endpoint
//! coincidence — no persistent node graph.
//!
//! **Scenario 1 (L-corner, sharp miter)** — extracted from a user session
//! dump: offset both centerlines ±t/2 → 4 faces, then fillet-radius-0 the
//! adjacent face pairs (= extend/trim to their intersection = the miter).
//! Here that's done analytically: at the shared node, intersect each wall's
//! face with the neighbour's facing face → corner vertex → trim.
//!
//! **Scenario 2a (X-crossing)** — two walls cross mid-span (both centerlines
//! interior): the slice of each face inside the other's footprint is removed
//! (`solve_face_segments`), leaving a clear opening at the junction.
//!
//! **Scenario 2b (T-junction)** — one wall's endpoint (the "stem") lands on
//! ANOTHER wall's span (the "through" wall) rather than on a shared node. Two
//! automatic clean-ups, keyed off `tee_contact`:
//!   • the stem's node-side faces are trimmed/extended to the through wall's
//!     NEAR face, so the stem butts the surface (no poke-through to the
//!     centerline) — done in [`solve_faces`];
//!   • the through wall's near face is OPENED across the stem's width (its far
//!     face stays whole), so the two interiors connect — done in
//!     [`solve_face_segments`].
//! Triggers within the through wall's thickness band, so it fires even when the
//! stem endpoint is placed a touch short or deep — "automatic connection".
//! See `Smart_Dobjects.md` (scenario 1b rounded corners still owed).

use cad_kernel::{Vec2, Wall};

/// World-unit tolerance for treating two wall endpoints as the same node.
pub const JOIN_TOL: f64 = 1e-4;

/// Miter limit. A sharp corner's OUTER (convex) miter runs to a point at
/// distance `half_thickness / sin(halfAngle)` from the node — unbounded as the
/// angle gets acute (the "spike"). When that distance exceeds `MITER_LIMIT *
/// half_thickness` the spike is replaced by a flat BEVEL cut, exactly like
/// AutoCAD's MITERLIMIT. 8.0 keeps a true, sharp miter for every normal corner
/// (down to ~7° between the walls — a 25° corner's tip is 4.6·half, well under
/// the limit) and bevels ONLY pathological near-hairpin folds. The concave
/// (inner) corner is NEVER bevelled — it is a valid sharp interior vertex.
pub const MITER_LIMIT: f64 = 8.0;

/// Derived (possibly mitred) faces of one wall — each a single segment, plus
/// any bevel connectors added where an over-limit acute corner was cut.
#[derive(Clone, Debug, PartialEq)]
pub struct WallFaces {
    pub left:  (Vec2, Vec2),
    pub right: (Vec2, Vec2),
    /// Flat bevel cut(s) across over-limit OUTER corners (empty for normal
    /// corners). Each replaces a runaway miter spike with a straight edge.
    pub bevels: Vec<(Vec2, Vec2)>,
}

/// Infinite-line intersection: line through `p1` dir `d1` vs `p2` dir `d2`.
/// `None` when parallel.
fn line_intersect(p1: Vec2, d1: Vec2, p2: Vec2, d2: Vec2) -> Option<Vec2> {
    let cross = d1.x * d2.y - d1.y * d2.x;
    if cross.abs() < 1e-12 { return None; }
    let dx = p2.x - p1.x;
    let dy = p2.y - p1.y;
    let t = (dx * d2.y - dy * d2.x) / cross;
    Some(p1 + d1 * t)
}

/// Two walls are "the same" (so a wall never joins to itself / an exact dup).
fn same_wall(a: &Wall, b: &Wall) -> bool {
    let close = |p: Vec2, q: Vec2| (p - q).len() < JOIN_TOL;
    (close(a.start, b.start) && close(a.end, b.end))
        || (close(a.start, b.end) && close(a.end, b.start))
}

/// True when two directions are (anti)parallel — no meaningful junction.
fn near_parallel(d1: Vec2, d2: Vec2) -> bool {
    let (l1, l2) = (d1.len(), d2.len());
    if l1 < 1e-12 || l2 < 1e-12 { return true; }
    ((d1.x * d2.y - d1.y * d2.x) / (l1 * l2)).abs() < 1e-6
}

/// T-junction test: does the point `p` (a stem wall's endpoint) land on
/// `host`'s centerline INTERIOR — not at a shared node, and within the host's
/// thickness band? Returns the along-parameter `u ∈ (0,1)` if so.
///
/// `T_BAND` = how far off the exact centerline the endpoint may sit, as a
/// multiple of the host's half-thickness. Users connect a wall to another by
/// snapping to its FACE (PER / NEA land on the face line, a full half-thickness
/// off the centerline), so the band MUST reach past the face or the clean-up
/// fires only intermittently. 1.75 covers a face-snapped endpoint (1.0·half)
/// with margin for snap drift and stems dropped a little short or deep, while
/// still excluding walls that merely pass nearby.
fn tee_contact(p: Vec2, host: &Wall) -> Option<f64> {
    const T_BAND: f64 = 1.75;
    let a = host.start;
    let d = host.end - a;
    let len = d.len();
    if len < JOIN_TOL { return None; }
    let u = (p - a).dot(d) / (len * len);         // param along host centerline
    let along = u * len;
    // INTERIOR only — a shared endpoint (along ≈ 0 or len) is an L-corner,
    // handled separately.
    if along <= JOIN_TOL || along >= len - JOIN_TOL { return None; }
    let foot = a + d * u;
    if (p - foot).len() > host.thickness * 0.5 * T_BAND + JOIN_TOL { return None; }
    Some(u)
}

/// Derive `this` wall's faces, mitring each end whose node coincides with a
/// different wall's end. `all` is every wall in scope (may include `this`;
/// identical walls are skipped). `None` only for a degenerate wall.
///
/// Miter rule (symmetric, order-independent): at a node, relative to each
/// wall's OUTGOING direction (away from the node),
///   miter_inner = this.leftOut  ∩ neighbour.rightOut
///   miter_outer = this.rightOut ∩ neighbour.leftOut
/// and the node-side endpoint of each face is moved to the matching miter.
pub fn solve_faces(this: &Wall, all: &[Wall]) -> Option<WallFaces> {
    let ll = this.left_line()?;
    let rl = this.right_line()?;
    let mut left  = (ll.a, ll.b);
    let mut right = (rl.a, rl.b);
    let mut bevels: Vec<(Vec2, Vec2)> = Vec::new();
    let dt = this.end - this.start;           // face direction (shared by L & R)
    let cap = MITER_LIMIT * this.thickness * 0.5;   // max spike length from node

    for (node, at_start) in [(this.start, true), (this.end, false)] {
        // ---- Scenario 1: shared-node corner (L, or a Y where 3+ walls meet).
        // "left-out" / "right-out" = this wall's faces relative to its OUTGOING
        // direction (away from the node).
        //   node == start: stored left is left-out  (node endpoint = .a)
        //   node == end:   stored right is left-out (node endpoint = .b)
        let this_lo = if at_start { ll } else { rl };
        let this_ro = if at_start { rl } else { ll };
        let this_out = if at_start { dt } else { -dt };   // dir away from node
        let this_ang = this_out.y.atan2(this_out.x);

        // Every OTHER straight wall that shares this node, with its own
        // outgoing dir + faces, keyed by CCW angle from this wall. The face
        // that closes the wedge on this wall's LEFT is the CCW-adjacent wall's
        // RIGHT-out face; on the RIGHT it's the CW-adjacent wall's LEFT-out.
        // With one neighbour (a plain L) CCW == CW == that wall — identical to
        // the old pairwise miter. With several, each face meets the correct
        // angular neighbour, so a wall dropped onto the joint of two others
        // (a Y) cleans up too.
        let mut others: Vec<(f64, Vec2, cad_kernel::Line, cad_kernel::Line)> = Vec::new();
        for n in all {
            if same_wall(this, n) || n.is_curved() { continue; }
            let at_s = (n.start - node).len() < JOIN_TOL;
            let at_e = (n.end - node).len() < JOIN_TOL;
            if at_s == at_e { continue; }   // not incident, or degenerate both-ends
            let (Some(nl), Some(nr)) = (n.left_line(), n.right_line()) else { continue };
            let out = if at_s { n.end - n.start } else { n.start - n.end };
            if out.len() < 1e-12 { continue; }
            let lo = if at_s { nl } else { nr };   // n's left-out
            let ro = if at_s { nr } else { nl };   // n's right-out
            let mut delta = out.y.atan2(out.x) - this_ang;
            while delta <= 1e-9 { delta += std::f64::consts::TAU; }   // (0, 2π]
            others.push((delta, out, lo, ro));
        }

        if !others.is_empty() {
            // CCW-adjacent = smallest positive angle; CW-adjacent = largest.
            let ccw = *others.iter()
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
            let cw = *others.iter()
                .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)).unwrap();
            let (ccw_out, ccw_ro) = (ccw.1, ccw.3);
            let (cw_out,  cw_lo)  = (cw.1,  cw.2);

            // mA rides this_lo (meets the CCW neighbour's right-out); mB rides
            // this_ro (meets the CW neighbour's left-out). One is the CONVEX
            // (outer) miter, the other CONCAVE (inner). Only the convex corner
            // is ever bevelled; the concave one is a valid sharp interior vertex
            // (bevelling it produced the little triangle artefacts at acute tips).
            let mA = line_intersect(this_lo.a, dt, ccw_ro.a, ccw_ro.b - ccw_ro.a);
            let mB = line_intersect(this_ro.a, dt, cw_lo.a,  cw_lo.b  - cw_lo.a);

            // Bisector INTO the wedge interior (concave side) for each pairing.
            let bis = |a: Vec2, b: Vec2| {
                let (la, lb) = (a.len(), b.len());
                if la < 1e-12 || lb < 1e-12 { Vec2::new(0.0, 0.0) } else { a / la + b / lb }
            };
            let concave_a = bis(this_out, ccw_out);
            let concave_b = bis(this_out, cw_out);

            // Bevel a face's node-side point only if it is the CONVEX miter AND
            // runs past the limit. Returns the (possibly bevelled) point; pushes
            // the bevel connector when it cuts.
            let place = |m: Option<Vec2>, concave: Vec2,
                         this_face: &cad_kernel::Line, n_face: &cad_kernel::Line,
                         bevels: &mut Vec<(Vec2, Vec2)>| -> Option<Vec2> {
                let m = m?;
                let d = (m - node).len();
                let is_convex = (m - node).dot(concave) <= 1e-9;   // away from interior
                if is_convex && d > cap && d > 1e-9 {
                    let u = (m - node) / d;                 // outward bisector
                    let p = node + u * cap;                 // bevel line origin
                    let bdir = u.perp();                    // bevel line dir
                    let tb = line_intersect(this_face.a, dt, p, bdir);
                    let nb = line_intersect(n_face.a, n_face.b - n_face.a, p, bdir);
                    if let (Some(tb), Some(nb)) = (tb, nb) {
                        bevels.push((tb, nb));
                        return Some(tb);
                    }
                }
                Some(m)
            };
            let pA = place(mA, concave_a, &this_lo, &ccw_ro, &mut bevels);
            let pB = place(mB, concave_b, &this_ro, &cw_lo,  &mut bevels);

            if at_start {
                if let Some(m) = pA { left.0  = m; }
                if let Some(m) = pB { right.0 = m; }
            } else {
                if let Some(m) = pA { right.1 = m; }
                if let Some(m) = pB { left.1  = m; }
            }

            // Compound case: this shared node ALSO sits on ANOTHER wall's SPAN
            // (a corner dropped mid-wall via nea/per). Keep the mitred faces
            // from poking THROUGH that host — trim any node-side end that landed
            // INSIDE the host back to its near face, so the corner butts it.
            if let Some(h) = all.iter().find(|h| {
                !same_wall(this, h) && !h.is_curved()
                    && (h.start - node).len() >= JOIN_TOL && (h.end - node).len() >= JOIN_TOL
                    && !near_parallel(this_out, h.end - h.start)
                    && tee_contact(node, h).is_some()
            }) {
                let hlen = (h.end - h.start).len();
                if hlen >= JOIN_TOL {
                    let hn = ((h.end - h.start) / hlen).perp();
                    let side = if this_out.dot(hn) >= 0.0 { 1.0 } else { -1.0 };
                    let foff = hn * side * (h.thickness * 0.5);
                    let (fa, fb) = (h.start + foff, h.end + foff);   // near-face line
                    let inside = |p: Vec2| (p - fa).dot(hn) * (-side) > 1e-9; // past near face → in host
                    let clip = |p: Vec2, face_pt: Vec2| -> Vec2 {
                        if inside(p) { line_intersect(face_pt, dt, fa, fb - fa).unwrap_or(p) } else { p }
                    };
                    if at_start {
                        left.0  = clip(left.0,  ll.a);
                        right.0 = clip(right.0, rl.a);
                    } else {
                        left.1  = clip(left.1,  ll.a);
                        right.1 = clip(right.1, rl.a);
                    }
                }
            }
            continue;   // corner handled — never also T-trim the same node
        }

        // ---- Scenario 2b: T-junction — THIS node lands on a wall's span --
        // Trim (or extend) this wall's node-side faces to the through wall's
        // NEAR face, so the stem butts cleanly against the surface instead of
        // poking half-thickness into it. If the endpoint lands where SEVERAL
        // walls meet (e.g. on the X-crossing of two others), each face butts the
        // NEAREST host near-face — so the stem never pokes through any of them.
        let hosts: Vec<&Wall> = all.iter().filter(|h| {
            !same_wall(this, h) && !h.is_curved()
                && !near_parallel(dt, h.end - h.start)
                && tee_contact(node, h).is_some()
        }).collect();
        if !hosts.is_empty() {
            let body = if at_start { this.end - this.start } else { this.start - this.end };
            // Best trim of one face line (through `fp`, dir `dt`): the near-face
            // hit CLOSEST to the node among all hosts.
            let best = |fp: Vec2| -> Option<Vec2> {
                let mut best: Option<(f64, Vec2)> = None;
                for h in &hosts {
                    let hlen = (h.end - h.start).len();
                    if hlen < JOIN_TOL { continue; }
                    let hn = ((h.end - h.start) / hlen).perp();      // host unit normal
                    let side = if body.dot(hn) >= 0.0 { 1.0 } else { -1.0 };  // near = stem side
                    let foff = hn * side * (h.thickness * 0.5);
                    let (fa, fb) = (h.start + foff, h.end + foff);   // near-face line
                    if let Some(m) = line_intersect(fp, dt, fa, fb - fa) {
                        let d = (m - node).len();
                        if best.map_or(true, |(bd, _)| d < bd) { best = Some((d, m)); }
                    }
                }
                best.map(|(_, m)| m)
            };
            let ml = best(ll.a);
            let mr = best(rl.a);
            if at_start {
                if let Some(m) = ml { left.0  = m; }
                if let Some(m) = mr { right.0 = m; }
            } else {
                if let Some(m) = ml { left.1  = m; }
                if let Some(m) = mr { right.1 = m; }
            }
        }
    }
    Some(WallFaces { left, right, bevels })
}

/// Parametric crossing of two centerlines `a0→a1` and `b0→b1`. Returns
/// `(u, v)` where the lines cross — `u` along a, `v` along b. `None` if parallel.
fn centerline_cross(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> Option<(f64, f64)> {
    let da = a1 - a0;
    let db = b1 - b0;
    let denom = da.x * db.y - da.y * db.x;
    if denom.abs() < 1e-12 { return None; }
    let dx = b0.x - a0.x;
    let dy = b0.y - a0.y;
    let u = (dx * db.y - dy * db.x) / denom;
    let v = (dx * da.y - dy * da.x) / denom;
    Some((u, v))
}

/// Interval `(t_in, t_out)` of segment `p0→p1` that lies INSIDE the convex
/// polygon `poly` (CCW). `None` if the segment misses the polygon. Liang–Barsky
/// against each edge's inward (left) half-plane.
fn clip_segment_convex(p0: Vec2, p1: Vec2, poly: &[Vec2]) -> Option<(f64, f64)> {
    let d = p1 - p0;
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let edge = b - a;
        let nrm = Vec2::new(-edge.y, edge.x); // inward normal for a CCW polygon
        let c0 = nrm.x * (p0.x - a.x) + nrm.y * (p0.y - a.y);
        let den = nrm.x * d.x + nrm.y * d.y;
        if den.abs() < 1e-12 {
            if c0 < 0.0 { return None; } // parallel to edge and outside
        } else {
            let t = -c0 / den;
            if den > 0.0 { if t > t0 { t0 = t; } } else if t < t1 { t1 = t; }
            if t0 > t1 { return None; }
        }
    }
    Some((t0, t1))
}

/// Subtract a set of `removed` (t_in, t_out) intervals from `[0,1]`, returning
/// the surviving sub-intervals.
fn subtract_intervals(mut removed: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    removed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut kept = Vec::new();
    let mut cursor = 0.0_f64;
    for (a, b) in removed {
        let a = a.clamp(0.0, 1.0);
        let b = b.clamp(0.0, 1.0);
        if a > cursor + 1e-9 { kept.push((cursor, a)); }
        if b > cursor { cursor = b; }
    }
    if cursor < 1.0 - 1e-9 { kept.push((cursor, 1.0)); }
    kept
}

/// Face footprint quad of a wall (CCW): left.a → right.a → right.b → left.b.
fn wall_quad(w: &Wall) -> Option<[Vec2; 4]> {
    let l = w.left_line()?;
    let r = w.right_line()?;
    Some([l.a, r.a, r.b, l.b])
}

/// Wall faces broken into SEGMENTS, with X-crossings cleaned: where this wall
/// passes straight through another straight wall (both centerlines cross in
/// each other's interior), the part of each face inside the other wall's
/// footprint is removed — leaving a clear opening at the junction. L-corner
/// miters from [`solve_faces`] are applied first. Returns `(left, right)` lists
/// of face pieces. Straight walls only.
pub fn solve_face_segments(this: &Wall, all: &[Wall]) -> Option<(Vec<(Vec2, Vec2)>, Vec<(Vec2, Vec2)>)> {
    if this.is_curved() { return None; }
    let faces = solve_faces(this, all)?; // L-corner / T-stem single segments

    // Walls that this one CROSSES through (pure X: both params interior) — the
    // slice of BOTH faces inside such a wall's footprint is removed.
    let crossers: Vec<&Wall> = all.iter().filter(|n| {
        !same_wall(this, n) && !n.is_curved()
            && centerline_cross(this.start, this.end, n.start, n.end)
                .map(|(u, v)| u > 1e-6 && u < 1.0 - 1e-6 && v > 1e-6 && v < 1.0 - 1e-6)
                .unwrap_or(false)
    }).collect();

    // Scenario 2b (through side): walls whose ENDPOINT lands on THIS wall's
    // span (T-stems). Each opens THIS wall's NEAR face only (the face on the
    // stem's side); the far face stays whole.
    //
    // The opening is the span where the STEM'S TWO FACE LINES cross this wall's
    // near-face line — NOT a clip of the stem's raw footprint quad. When the
    // stem is snapped to the face (the normal case) its quad only grazes the
    // near face, so a quad-clip fired only intermittently (the "first two OK,
    // last one messed up" report). The face-line crossings are deterministic
    // and coincide exactly with the stem's trimmed ends. Stored as
    // `(near_is_left, q_left, q_right)`.
    let this_dir = this.end - this.start;
    let tn = {
        let l = this_dir.len();
        if l < JOIN_TOL { return None; }
        (this_dir / l).perp()
    };
    let this_ll = this.left_line();
    let this_rl = this.right_line();
    let mut tee_open: Vec<(bool, Vec2, Vec2)> = Vec::new();
    for n in all {
        if same_wall(this, n) || n.is_curved() || near_parallel(this_dir, n.end - n.start) {
            continue;
        }
        // whichever endpoint of n (if any) lands on this wall's interior
        let contact = if tee_contact(n.start, this).is_some() {
            Some((n.start, n.end))
        } else if tee_contact(n.end, this).is_some() {
            Some((n.end, n.start))
        } else {
            None
        };
        let Some((ep, far)) = contact else { continue };
        let near_is_left = (far - ep).dot(tn) >= 0.0;               // stem body side
        let near = if near_is_left { this_ll } else { this_rl };
        let (Some(near), Some(nl), Some(nr)) = (near, n.left_line(), n.right_line())
            else { continue };
        let ndir = n.end - n.start;
        // where the stem's two faces cross this wall's near-face line
        let q_l = line_intersect(nl.a, ndir, near.a, near.b - near.a);
        let q_r = line_intersect(nr.a, ndir, near.a, near.b - near.a);
        if let (Some(q_l), Some(q_r)) = (q_l, q_r) {
            tee_open.push((near_is_left, q_l, q_r));
        }
    }

    let trim = |seg: (Vec2, Vec2), is_left: bool| -> Vec<(Vec2, Vec2)> {
        let (s0, s1) = seg;
        let len2 = (s1 - s0).dot(s1 - s0).max(1e-18);
        let proj = |q: Vec2| ((q - s0).dot(s1 - s0)) / len2;   // param of q on seg
        let mut removed: Vec<(f64, f64)> = Vec::new();
        for n in &crossers {
            if let Some(quad) = wall_quad(n) {
                if let Some((ti, to)) = clip_segment_convex(s0, s1, &quad) {
                    // only an INTERIOR bite (a through-crossing), never an end
                    if ti > 1e-6 && to < 1.0 - 1e-6 && to - ti > 1e-9 {
                        removed.push((ti, to));
                    }
                }
            }
        }
        for (near_is_left, q_l, q_r) in &tee_open {
            if *near_is_left != is_left { continue; }   // near face only
            let (a, b) = (proj(*q_l), proj(*q_r));
            let (lo, hi) = (a.min(b), a.max(b));
            if hi - lo > 1e-9 {
                removed.push((lo.clamp(0.0, 1.0), hi.clamp(0.0, 1.0)));
            }
        }
        let d = s1 - s0;
        subtract_intervals(removed).into_iter()
            .map(|(a, b)| (s0 + d * a, s0 + d * b))
            .collect()
    };

    let mut left_pieces = trim(faces.left, true);
    let right_pieces = trim(faces.right, false);
    // Bevel connectors are corner features (not a left/right face); append them
    // so the renderer strokes them. They ride the left list purely for
    // transport — both this wall and its neighbour emit an identical copy, so a
    // tiny harmless overdraw, never a gap.
    left_pieces.extend(faces.bevels.iter().copied());
    Some((left_pieces, right_pieces))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(p: Vec2, q: Vec2) -> bool { (p - q).len() < 1e-6 }

    #[test]
    fn lone_wall_keeps_full_faces() {
        let w = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let f = solve_faces(&w, &[w]).unwrap();
        assert!(close(f.left.0, Vec2::new(0.0, 1.0)));
        assert!(close(f.left.1, Vec2::new(10.0, 1.0)));
        assert!(close(f.right.0, Vec2::new(0.0, -1.0)));
        assert!(close(f.right.1, Vec2::new(10.0, -1.0)));
    }

    #[test]
    fn l_corner_90deg_miters_both_faces() {
        // A: (0,0)->(10,0)  B: (0,0)->(0,10), thickness 2, shared node (0,0).
        let a = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let b = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(0.0, 10.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![a, b];
        let fa = solve_faces(&a, &all).unwrap();
        // A's start-side faces miter to the inner (1,1) and outer (-1,-1).
        assert!(close(fa.left.0,  Vec2::new(1.0, 1.0)),  "inner miter, got {:?}", fa.left.0);
        assert!(close(fa.right.0, Vec2::new(-1.0, -1.0)), "outer miter, got {:?}", fa.right.0);
        // Far end untouched.
        assert!(close(fa.left.1,  Vec2::new(10.0, 1.0)));
        assert!(close(fa.right.1, Vec2::new(10.0, -1.0)));
    }

    #[test]
    fn l_corner_any_angle_meets_at_a_point() {
        // 45° corner: A east, B north-east. Faces must still meet (no gap):
        // the two inner faces share the inner miter, the two outer share outer.
        let a = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let b = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(7.07, 7.07), thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![a, b];
        let fa = solve_faces(&a, &all).unwrap();
        let fb = solve_faces(&b, &all).unwrap();
        // A.start-left (inner) should coincide with B's matching inner face end.
        // Both inner faces meet at the same point; both outer faces meet too.
        let a_inner = fa.left.0;
        let a_outer = fa.right.0;
        let b_ends = [fb.left.0, fb.right.0];
        assert!(b_ends.iter().any(|p| close(*p, a_inner)),
            "A inner {:?} not shared by B {:?}", a_inner, b_ends);
        assert!(b_ends.iter().any(|p| close(*p, a_outer)),
            "A outer {:?} not shared by B {:?}", a_outer, b_ends);
    }

    #[test]
    fn lone_wall_faces_are_single_segments() {
        let w = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let (l, r) = solve_face_segments(&w, &[w]).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(r.len(), 1);
        assert!(close(l[0].0, Vec2::new(0.0, 1.0)) && close(l[0].1, Vec2::new(10.0, 1.0)));
    }

    #[test]
    fn x_crossing_breaks_each_face_into_two_with_a_gap() {
        // Horizontal wall A (thickness 2) crossed mid-span by vertical wall B
        // (thickness 4, spanning y=-10..10 at x=5). Each of A's faces must be
        // cut into two pieces, with the gap = B's width (x = 5±2).
        let a = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let b = Wall { start: Vec2::new(5.0, -10.0), end: Vec2::new(5.0, 10.0), thickness: 4.0, style: 0, bulge: 0.0 };
        let all = vec![a, b];
        let (l, r) = solve_face_segments(&a, &all).unwrap();
        assert_eq!(l.len(), 2, "left face should split in two, got {l:?}");
        assert_eq!(r.len(), 2, "right face should split in two");
        // first piece ends at x≈3, second starts at x≈7 (B half-width = 2)
        assert!((l[0].1.x - 3.0).abs() < 1e-6, "gap start {:?}", l[0].1);
        assert!((l[1].0.x - 7.0).abs() < 1e-6, "gap end {:?}", l[1].0);
    }

    #[test]
    fn parallel_neighbour_does_not_trim() {
        // A second wall running parallel and apart must NOT bite the faces.
        let a = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let b = Wall { start: Vec2::new(0.0, 20.0), end: Vec2::new(10.0, 20.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let (l, r) = solve_face_segments(&a, &vec![a, b]).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn t_junction_stem_trims_to_host_near_face() {
        // Through wall HOST (thickness 4) along the X-axis → faces at y=±2.
        // STEM (thickness 2) rises from the host centerline at x=5 up to y=5.
        // The stem's start-side faces must trim back to the near face (y=+2),
        // not run down to the centerline (y=0).
        let host = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 4.0, style: 0, bulge: 0.0 };
        let stem = Wall { start: Vec2::new(5.0, 0.0), end: Vec2::new(5.0, 5.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![host, stem];
        let f = solve_faces(&stem, &all).unwrap();
        // stem faces are the vertical lines x=4 (left) and x=6 (right).
        assert!(close(f.left.0,  Vec2::new(4.0, 2.0)), "left start trimmed to near face, got {:?}", f.left.0);
        assert!(close(f.right.0, Vec2::new(6.0, 2.0)), "right start trimmed to near face, got {:?}", f.right.0);
        // far (free) end untouched
        assert!(close(f.left.1,  Vec2::new(4.0, 5.0)));
        assert!(close(f.right.1, Vec2::new(6.0, 5.0)));
    }

    #[test]
    fn t_junction_opens_host_near_face_only() {
        // Same T. The host's NEAR face (y=+2) must split in two with a gap over
        // the stem's width (x=4..6); the FAR face (y=-2) stays a single piece.
        let host = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 4.0, style: 0, bulge: 0.0 };
        let stem = Wall { start: Vec2::new(5.0, 0.0), end: Vec2::new(5.0, 5.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![host, stem];
        let (l, r) = solve_face_segments(&host, &all).unwrap();
        // host left_line is the +normal face = y=+2 → the NEAR face → split.
        assert_eq!(l.len(), 2, "near face should open, got {l:?}");
        assert_eq!(r.len(), 1, "far face should stay whole, got {r:?}");
        assert!((l[0].1.x - 4.0).abs() < 1e-6, "gap start {:?}", l[0].1);
        assert!((l[1].0.x - 6.0).abs() < 1e-6, "gap end {:?}", l[1].0);
    }

    #[test]
    fn t_junction_opening_is_stable_short_exact_and_deep() {
        // The SAME opening must appear whether the stem stops a little SHORT of
        // the near face, exactly ON it, or pokes a little PAST — the flaky
        // "first two OK, last messed up" case. Opening = where the stem's faces
        // (x=9 and x=11) cross the near face (y=+2), regardless of stem depth.
        let host = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(20.0, 0.0), thickness: 4.0, style: 0, bulge: 0.0 };
        for end_y in [3.0_f64, 2.0, 1.0] {   // short of / on / past the near face
            let stem = Wall { start: Vec2::new(10.0, 8.0), end: Vec2::new(10.0, end_y), thickness: 2.0, style: 0, bulge: 0.0 };
            let (l, r) = solve_face_segments(&host, &vec![host, stem]).unwrap();
            assert_eq!(l.len(), 2, "near face must open (stem end y={end_y}), got {l:?}");
            assert_eq!(r.len(), 1, "far face stays whole (stem end y={end_y})");
            assert!((l[0].1.x - 9.0).abs() < 1e-6 && (l[1].0.x - 11.0).abs() < 1e-6,
                "opening not at stem faces (stem end y={end_y}): {l:?}");
        }
    }

    #[test]
    fn stem_ending_on_a_crossing_of_two_walls_butts_the_nearest() {
        // A wall ends where two OTHER walls cross. Its node-side faces must butt
        // the nearest host near-face — never shoot half-thickness past the
        // junction (the reported "wall to one joint of two others" case).
        let h1 = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(40.0, 0.0), thickness: 4.0, style: 0, bulge: 0.0 };
        let h2 = Wall { start: Vec2::new(10.0, -20.0), end: Vec2::new(30.0, 20.0), thickness: 4.0, style: 0, bulge: 0.0 };
        // both pass through (20,0); the stem comes up from the lower-left to it.
        let stem = Wall { start: Vec2::new(5.0, -25.0), end: Vec2::new(20.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![h1, h2, stem];
        let f = solve_faces(&stem, &all).unwrap();
        let node = Vec2::new(20.0, 0.0);
        for p in [f.left.1, f.right.1] {
            assert!((p - node).len() <= 6.0, "stem face pokes far past the junction: {:?}", p);
        }
    }

    #[test]
    fn t_junction_stem_landing_on_the_face_still_fires() {
        // Users snap the stem end to the through wall's FACE (PER/NEA), not its
        // centerline — so the endpoint sits a full half-thickness off-centre.
        // The clean-up must still fire (was flaky when the band == half-thk).
        let host = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(20.0, 0.0), thickness: 5.0, style: 0, bulge: 0.0 };
        // near face is y = +2.5; drop the stem end right on it.
        let stem = Wall { start: Vec2::new(10.0, 2.5), end: Vec2::new(10.0, 12.0), thickness: 3.0, style: 0, bulge: 0.0 };
        let all = vec![host, stem];
        let f = solve_faces(&stem, &all).unwrap();
        // both stem faces trim onto the near face (y = 2.5)
        assert!((f.left.0.y  - 2.5).abs() < 1e-6, "left not on face: {:?}", f.left.0);
        assert!((f.right.0.y - 2.5).abs() < 1e-6, "right not on face: {:?}", f.right.0);
        // near face opens, far face whole
        let (l, r) = solve_face_segments(&host, &all).unwrap();
        assert_eq!(l.len(), 2, "near face should open, got {l:?}");
        assert_eq!(r.len(), 1, "far face should stay whole, got {r:?}");
    }

    #[test]
    fn t_junction_non_right_angle_trims_both_stem_faces() {
        // A SLANTED stem into a horizontal wall. Both of the stem's faces must
        // trim to the near face (the "only one side trims at non-90°" report),
        // and the through wall opens its near face only.
        let host = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(20.0, 0.0), thickness: 4.0, style: 0, bulge: 0.0 };
        // near face y = +2; stem comes in at ~45° and ends on it at (10,2).
        let stem = Wall { start: Vec2::new(2.0, 10.0), end: Vec2::new(10.0, 2.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![host, stem];
        let f = solve_faces(&stem, &all).unwrap();
        // node-side endpoint of BOTH faces lands on the near-face line y = 2.
        assert!((f.left.1.y  - 2.0).abs() < 1e-6, "left face not trimmed to face: {:?}", f.left.1);
        assert!((f.right.1.y - 2.0).abs() < 1e-6, "right face not trimmed to face: {:?}", f.right.1);
        let (l, r) = solve_face_segments(&host, &all).unwrap();
        assert_eq!(l.len(), 2, "near face should open at a slanted T, got {l:?}");
        assert_eq!(r.len(), 1, "far face should stay whole, got {r:?}");
    }

    #[test]
    fn right_angle_corner_is_not_bevelled() {
        // 90° L — the miter tip (1.41·half) is well under the limit, so NO
        // bevel and the faces meet at the sharp mitre (regression guard).
        let a = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let b = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(0.0, 10.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let fa = solve_faces(&a, &[a, b]).unwrap();
        assert!(fa.bevels.is_empty(), "90° must not bevel, got {:?}", fa.bevels);
        assert!(close(fa.left.0, Vec2::new(1.0, 1.0)));
    }

    #[test]
    fn corner_dropped_on_a_wall_span_butts_the_host() {
        // Two walls meet in a V whose shared node sits ON a horizontal host
        // wall's near face (a corner attached mid-wall via nea). The corner's
        // node-side faces must NOT dip below the near face (y=+2) into the host
        // — they butt it. The host still opens under them (T-stem path).
        let host = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(40.0, 0.0), thickness: 4.0, style: 0, bulge: 0.0 };
        let w13 = Wall { start: Vec2::new(35.0, 20.0), end: Vec2::new(20.0, 2.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let w14 = Wall { start: Vec2::new(20.0, 2.0), end: Vec2::new(5.0, 20.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![host, w13, w14];
        let f13 = solve_faces(&w13, &all).unwrap();   // node is w13.end → .1 ends
        let f14 = solve_faces(&w14, &all).unwrap();   // node is w14.start → .0 ends
        for y in [f13.left.1.y, f13.right.1.y, f14.left.0.y, f14.right.0.y] {
            assert!(y >= 2.0 - 1e-6, "corner face poked into host (y={y})");
        }
        // host opens its near face where the corner attaches
        let (l, _r) = solve_face_segments(&host, &all).unwrap();
        assert!(l.len() >= 2, "host near face should open under the attached corner, got {l:?}");
    }

    #[test]
    fn y_junction_three_walls_share_a_node_cleanly() {
        // Three walls meeting at the origin, 120° apart (a wall dropped onto the
        // joint of two others). Each wall's two node-side face ends must meet
        // exactly one neighbour's face end — three shared corners, no floating
        // faces, no bevels (was messy: only ONE neighbour got mitred).
        let a = Vec2::new(10.0, 0.0);
        let b = Vec2::new(-5.0, 8.66);
        let c = Vec2::new(-5.0, -8.66);
        let w0 = Wall { start: Vec2::new(0.0, 0.0), end: a, thickness: 2.0, style: 0, bulge: 0.0 };
        let w1 = Wall { start: Vec2::new(0.0, 0.0), end: b, thickness: 2.0, style: 0, bulge: 0.0 };
        let w2 = Wall { start: Vec2::new(0.0, 0.0), end: c, thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![w0, w1, w2];
        let mut pts = Vec::new();
        for w in &all {
            let f = solve_faces(w, &all).unwrap();
            assert!(f.bevels.is_empty(), "120° Y must not bevel: {:?}", f.bevels);
            pts.push(f.left.0);   // all three start at the node → node-side is .0
            pts.push(f.right.0);
        }
        for (i, p) in pts.iter().enumerate() {
            let shared = pts.iter().enumerate()
                .filter(|(j, q)| *j != i && close(*p, **q)).count();
            assert_eq!(shared, 1, "face end {i} {:?} not shared by exactly one other", p);
        }
    }

    #[test]
    fn moderately_sharp_corner_stays_a_true_miter() {
        // ~30° corner (like the reported zig-zag). It must NOT bevel — a proper
        // sharp miter, same as a wide corner. A's end-side faces meet B's
        // start-side faces at shared points, and no bevel connector appears.
        let a = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let b = Wall { start: Vec2::new(10.0, 0.0), end: Vec2::new(-7.3, -10.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![a, b];
        let fa = solve_faces(&a, &all).unwrap();
        let fb = solve_faces(&b, &all).unwrap();
        assert!(fa.bevels.is_empty() && fb.bevels.is_empty(),
            "30° corner must not bevel: {:?} {:?}", fa.bevels, fb.bevels);
        let b_ends = [fb.left.0, fb.right.0];
        for ae in [fa.left.1, fa.right.1] {
            assert!(b_ends.iter().any(|be| close(*be, ae)),
                "A end {:?} not shared by B {:?}", ae, b_ends);
        }
    }

    #[test]
    fn sharp_fold_bevels_instead_of_spiking() {
        // A sharp fold (~12° between the wall lines): the outer miter would
        // otherwise spike ~9.6·half out from the node. It must bevel instead —
        // a connector appears and BOTH node-side face ends stay within the
        // miter cap (MITER_LIMIT·half) of the node.
        let a = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let b = Wall { start: Vec2::new(10.0, 0.0), end: Vec2::new(0.0, 2.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let node = Vec2::new(10.0, 0.0);
        let cap = MITER_LIMIT * 1.0;   // half-thickness = 1
        // The un-bevelled miter would spike ~10 units out; the bevel bounds the
        // node-side ends to roughly cap (the flat cut crosses the face a little
        // beyond cap laterally, but nothing like a spike).
        let bound = 2.0 * cap;         // = 6, vs the ~10-unit raw spike
        let fa = solve_faces(&a, &[a, b]).unwrap();
        assert!(!fa.bevels.is_empty(), "sharp fold should bevel");
        assert!((fa.left.1  - node).len() <= bound, "left end still spikes: {:?}", fa.left.1);
        assert!((fa.right.1 - node).len() <= bound, "right end still spikes: {:?}", fa.right.1);
        for (p, q) in &fa.bevels {
            assert!((*p - node).len() <= bound && (*q - node).len() <= bound,
                "bevel far from node: {:?}", (p, q));
        }
    }

    #[test]
    fn t_junction_short_stem_extends_to_face() {
        // Stem dropped a little SHORT (ends at y=0.5, still inside the band):
        // the clean-up must EXTEND the faces down to the near face y=+2, same
        // as the exact case — "automatic connection".
        let host = Wall { start: Vec2::new(0.0, 0.0), end: Vec2::new(10.0, 0.0), thickness: 4.0, style: 0, bulge: 0.0 };
        let stem = Wall { start: Vec2::new(5.0, 0.5), end: Vec2::new(5.0, 5.0), thickness: 2.0, style: 0, bulge: 0.0 };
        let all = vec![host, stem];
        let f = solve_faces(&stem, &all).unwrap();
        assert!(close(f.left.0,  Vec2::new(4.0, 2.0)), "short stem left, got {:?}", f.left.0);
        assert!(close(f.right.0, Vec2::new(6.0, 2.0)), "short stem right, got {:?}", f.right.0);
    }
}

/// Issue #16 — join wall CENTERLINES whose ends were drawn near each other
/// ("drawn individually, ends don't miter"): once endpoints coincide,
/// [`solve_faces`] miters the corner at render. Two passes:
///   1. end→end — each wall end within `tol` of another wall's end is moved
///      (with its partner) to the midpoint of the two, joining the pair;
///   2. end→span (T) — a wall end within `tol` of another wall's centerline
///      interior is projected onto it, so the render's T-junction clean-up
///      (stem butt + through-face opening) takes over.
/// Returns the number of endpoints moved. Curved walls keep their bulge —
/// only the endpoint moves.
pub fn join_wall_endpoints(walls: &mut [Wall], tol: f64) -> usize {
    let mut moved = 0;
    let n = walls.len();
    if n < 2 || tol <= 0.0 { return 0; }
    // ---- Pass 1: end→end joins (symmetric midpoint). ----
    for i in 0..n {
        let p = walls[i].end;
        let mut best: Option<(usize, bool, Vec2)> = None;   // (wall j, at_j_start, target)
        for j in 0..n {
            if i == j { continue; }
            for (at_start, q) in [(true, walls[j].start), (false, walls[j].end)] {
                let d = (p - q).len();
                if d < tol && best.map_or(true, |(_, _, bp)| d < (p - bp).len()) {
                    best = Some((j, at_start, q));
                }
            }
        }
        if let Some((j, at_start, q)) = best {
            let mid = (p + q) * 0.5;
            walls[i].end = mid;
            if at_start { walls[j].start = mid; } else { walls[j].end = mid; }
            moved += 1;
        }
    }
    // ---- Pass 2: end→span T-joins (project the stem end onto the host). ----
    for i in 0..n {
        for j in 0..n {
            if i == j { continue; }
            let d = walls[j].end - walls[j].start;
            let len_sq = d.len_sq();
            if len_sq < 1e-12 { continue; }
            for (at_start, p) in [(true, walls[i].start), (false, walls[i].end)] {
                let t = ((p - walls[j].start).dot(d) / len_sq).clamp(0.0, 1.0);
                // Only a genuine T: the foot must be interior to j's span,
                // not its own end (those were handled in pass 1).
                if t < 1e-3 || t > 1.0 - 1e-3 { continue; }
                let foot = walls[j].start + d * t;
                let dist = (foot - p).len();
                if dist < tol && dist > 1e-9 {
                    if at_start { walls[i].start = foot; } else { walls[i].end = foot; }
                    moved += 1;
                }
            }
        }
    }
    moved
}

#[cfg(test)]
mod join_tests {
    use super::*;

    fn w(start: Vec2, end: Vec2) -> Wall {
        Wall { start, end, thickness: 2.0, style: 0, bulge: 0.0 }
    }

    #[test]
    fn near_end_to_end_joins_at_midpoint() {
        // Two walls drawn individually, ends 0.5 apart: the ends move to
        // the midpoint so the render's miter solver can corner them.
        let mut walls = vec![
            w(Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)),
            w(Vec2::new(10.4, 0.0), Vec2::new(20.0, 0.0)),
        ];
        let moved = join_wall_endpoints(&mut walls, 1.0);
        assert_eq!(moved, 1);
        assert_eq!(walls[0].end, Vec2::new(10.2, 0.0));
        assert_eq!(walls[1].start, Vec2::new(10.2, 0.0));
    }

    #[test]
    fn far_endpoints_are_left_alone() {
        let mut walls = vec![
            w(Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)),
            w(Vec2::new(15.0, 0.0), Vec2::new(25.0, 0.0)),
        ];
        let moved = join_wall_endpoints(&mut walls, 1.0);
        assert_eq!(moved, 0);
        assert_eq!(walls[0].end, Vec2::new(10.0, 0.0));
    }

    #[test]
    fn t_junction_end_projects_onto_span() {
        // Stem's end is 0.3 off the through wall's centerline interior →
        // it moves onto the span so the render's T-cleanup takes over.
        let mut walls = vec![
            w(Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0)),   // through, horizontal
            w(Vec2::new(10.0, -5.0), Vec2::new(10.3, 0.3)), // stem, 0.3 off the span
        ];
        let moved = join_wall_endpoints(&mut walls, 1.0);
        assert_eq!(moved, 1);
        assert_eq!(walls[1].end, Vec2::new(10.3, 0.0));
        // The through wall itself is untouched.
        assert_eq!(walls[0].start, Vec2::new(0.0, 0.0));
        assert_eq!(walls[0].end, Vec2::new(20.0, 0.0));
    }
}
