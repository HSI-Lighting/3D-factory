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
//! See `Smart_Dobjects.md` (scenarios 1b rounded and 2 T-junction are owed).

use cad_kernel::{Vec2, Wall};

/// World-unit tolerance for treating two wall endpoints as the same node.
pub const JOIN_TOL: f64 = 1e-4;

/// Derived (possibly mitred) faces of one wall — each a single segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallFaces {
    pub left:  (Vec2, Vec2),
    pub right: (Vec2, Vec2),
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

    for (node, at_start) in [(this.start, true), (this.end, false)] {
        let neighbor = all.iter().find(|n| {
            !same_wall(this, n)
                && !n.is_curved()   // curved corner walls meet tangentially — no miter
                && ((n.start - node).len() < JOIN_TOL || (n.end - node).len() < JOIN_TOL)
        });
        let Some(n) = neighbor else { continue };
        let (Some(nl), Some(nr)) = (n.left_line(), n.right_line()) else { continue };
        let n_at_start = (n.start - node).len() < JOIN_TOL;

        // "left-out" / "right-out" = faces relative to the outgoing dir.
        //   node == start: stored left is left-out  (node endpoint = .a)
        //   node == end:   stored right is left-out (node endpoint = .b)
        let this_lo = if at_start { ll } else { rl };
        let this_ro = if at_start { rl } else { ll };
        let n_lo    = if n_at_start { nl } else { nr };
        let n_ro    = if n_at_start { nr } else { nl };

        let dt = this_lo.b - this_lo.a;       // both this-faces share this dir
        let m1 = line_intersect(this_lo.a, dt, n_ro.a, n_ro.b - n_ro.a);
        let m2 = line_intersect(this_ro.a, dt, n_lo.a, n_lo.b - n_lo.a);

        if at_start {
            if let Some(m) = m1 { left.0  = m; }
            if let Some(m) = m2 { right.0 = m; }
        } else {
            if let Some(m) = m1 { right.1 = m; }
            if let Some(m) = m2 { left.1  = m; }
        }
    }
    Some(WallFaces { left, right })
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
    let faces = solve_faces(this, all)?; // L-corner mitered single segments

    // Walls that this one CROSSES through (pure X: both params interior).
    let crossers: Vec<&Wall> = all.iter().filter(|n| {
        !same_wall(this, n) && !n.is_curved()
            && centerline_cross(this.start, this.end, n.start, n.end)
                .map(|(u, v)| u > 1e-6 && u < 1.0 - 1e-6 && v > 1e-6 && v < 1.0 - 1e-6)
                .unwrap_or(false)
    }).collect();

    let trim = |seg: (Vec2, Vec2)| -> Vec<(Vec2, Vec2)> {
        let (s0, s1) = seg;
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
        let d = s1 - s0;
        subtract_intervals(removed).into_iter()
            .map(|(a, b)| (s0 + d * a, s0 + d * b))
            .collect()
    };

    Some((trim(faces.left), trim(faces.right)))
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
}
