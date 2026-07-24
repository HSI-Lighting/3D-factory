//! Block-diff parametric rule extraction (programming by demonstration).
//!
//! Given two near-identical block definitions A and B — e.g. the SAME
//! door drawn at opening 1000 and at 1150 — this matches their dobjects,
//! finds which defining points moved, and clusters the moved points into
//! candidate PARAMETERS. Each cluster is one displacement: a window (the
//! bbox of the points that moved), a unit direction, and a magnitude
//! (the A→B difference). Two samples define a LINEAR rule per cluster.
//!
//! Output = the raw material for a parametric ("semi-smart") block: which
//! geometry moves, in what direction, by how much, between the examples.
//! The user then names/merges the clusters into inputs (`width`, `frame`).
//!
//! NO solver, NO external kernel — pure arithmetic over the existing
//! `Geom` types. See Smart_Dobjects.md "Parametric Block (block diff)".
//! Slice 1: detect point displacements (line/arc-endpoint/polyline-vertex/
//! circle-centre/point). Bulge-only and topology changes are NOT clustered
//! yet (the dobject is still reported as `changed`).

use crate::dobject::DObject;
use crate::geom::Geom;
use crate::math::Vec2;

/// A cluster of moved points sharing one displacement vector — a
/// candidate parameter (one stretch step). `dir * magnitude` is the A→B
/// displacement; `win_min/win_max` is the bbox (in base-relative space) of
/// the points that moved by it.
#[derive(Clone, Debug)]
pub struct ParamCluster {
    pub win_min:     Vec2,
    pub win_max:     Vec2,
    pub dir:         Vec2,    // unit direction of the displacement
    pub magnitude:   f64,     // |A→B displacement|
    pub point_count: usize,   // how many moved points fell in this cluster
    /// Base-relative BEFORE positions of every point in this cluster —
    /// for highlighting the parametric points on a block instance.
    pub points:      Vec<Vec2>,
}

/// Result of diffing two block definitions.
#[derive(Clone, Debug, Default)]
pub struct BlockDiff {
    pub matched:   usize,   // dobjects paired A↔B
    pub unchanged: usize,   // paired AND identical (the fixed majority)
    pub changed:   usize,   // paired but geometry differs
    pub added:     usize,   // present in B, no match in A
    pub removed:   usize,   // present in A, no match in B
    pub clusters:  Vec<ParamCluster>,   // candidate parameters
}

/// Matching feature for one dobject, in base-relative coordinates.
/// `pts` are the defining points (used for matching AND delta); `scal`
/// are shape scalars (radius, bulge, closed-flag) used only to decide
/// "identical vs changed".
struct Feat { tag: u8, pts: Vec<Vec2>, scal: Vec<f64> }

fn feature(g: &Geom, base: Vec2) -> Feat {
    let rel = |p: Vec2| p - base;
    match g {
        Geom::Line(l) => Feat { tag: 0, pts: vec![rel(l.a), rel(l.b)], scal: vec![] },
        Geom::Polyline(p) => {
            let pts = p.vertices.iter().map(|v| rel(v.pos)).collect();
            let mut scal: Vec<f64> = p.vertices.iter().map(|v| v.bulge).collect();
            scal.push(if p.closed { 1.0 } else { 0.0 });
            Feat { tag: 1, pts, scal }
        }
        Geom::Circle(c) => Feat { tag: 2, pts: vec![rel(c.center)], scal: vec![c.radius] },
        Geom::Arc(a) => {
            let (s, e) = a.endpoints();
            // endpoints carry the stretch; bulge captures curvature/dir.
            Feat { tag: 3, pts: vec![rel(s), rel(e)], scal: vec![(a.sweep_angle * 0.25).tan()] }
        }
        Geom::Point(pt) => Feat { tag: 4, pts: vec![rel(pt.location)], scal: vec![] },
        // Coarse fallback for types not modelled this slice (ellipse,
        // text, dim, hatch, spline, wall, blockref): bbox corners, so
        // they still match/translate and a gross move is detected.
        other => {
            let (mn, mx) = other.bbox();
            Feat { tag: 5, pts: vec![rel(mn), rel(mx)], scal: vec![] }
        }
    }
}

fn same_shape(a: &Feat, b: &Feat) -> bool {
    a.tag == b.tag && a.pts.len() == b.pts.len() && a.scal.len() == b.scal.len()
}

fn scalars_equal(a: &Feat, b: &Feat, eps: f64) -> bool {
    a.scal.iter().zip(&b.scal).all(|(x, y)| (x - y).abs() <= eps)
}

fn points_equal(a: &Feat, b: &Feat, eps: f64) -> bool {
    a.pts.iter().zip(&b.pts).all(|(p, q)| (*p - *q).len() <= eps)
}

fn centroid(f: &Feat) -> Vec2 {
    if f.pts.is_empty() { return Vec2::ZERO; }
    let mut s = Vec2::ZERO;
    for p in &f.pts { s = s + *p; }
    s / (f.pts.len() as f64)
}

/// Diff two block definitions. `*_base` are the blocks' base points;
/// everything is compared base-relative so a translated copy reads as
/// unchanged. `eps` is the coordinate tolerance for "same point".
pub fn diff_blocks(
    a: &[DObject], a_base: Vec2,
    b: &[DObject], b_base: Vec2,
    eps: f64,
) -> BlockDiff {
    let fa: Vec<Feat> = a.iter().map(|d| feature(&d.geom, a_base)).collect();
    let fb: Vec<Feat> = b.iter().map(|d| feature(&d.geom, b_base)).collect();
    let mut used_b   = vec![false; fb.len()];
    let mut matched_a = vec![false; fa.len()];

    let mut out = BlockDiff::default();
    let mut moved: Vec<(Vec2, Vec2)> = Vec::new();   // (before, delta)
    let mut pairs: Vec<(usize, usize)> = Vec::new();

    // ---- Correspondence ---------------------------------------------------
    // INDEX-FIRST: when the two blocks line up 1:1 in order with matching
    // shapes AND most pairs are identical (the copy-then-edit case), pair by
    // index — this is exact and avoids the mis-matching that nearest-centroid
    // produces when the blocks aren't a clean stretch. Otherwise fall back to
    // exact-then-nearest greedy matching with a distance gate.
    let index_ok = !fa.is_empty()
        && fa.len() == fb.len()
        && fa.iter().zip(&fb).all(|(x, y)| same_shape(x, y))
        && {
            let unchanged = fa.iter().zip(&fb)
                .filter(|(x, y)| points_equal(x, y, eps) && scalars_equal(x, y, eps))
                .count();
            unchanged * 2 >= fa.len()   // ≥50% identical → a genuine variant pair
        };

    if index_ok {
        for i in 0..fa.len() {
            used_b[i] = true; matched_a[i] = true;
            pairs.push((i, i));
        }
    } else {
        // Pass 1 — exact unchanged matches (the fixed majority).
        for i in 0..fa.len() {
            for j in 0..fb.len() {
                if used_b[j] || !same_shape(&fa[i], &fb[j]) { continue; }
                if points_equal(&fa[i], &fb[j], eps) && scalars_equal(&fa[i], &fb[j], eps) {
                    used_b[j] = true; matched_a[i] = true;
                    pairs.push((i, j));
                    break;
                }
            }
        }
        // Pass 2 — changed matches: same shape, nearest centroid. (No
        // distance gate: a mis-pair is always to another dobject INSIDE
        // the block, so a distance gate can't tell it from a legitimate
        // large stretch — it only ever rejected real moves. Noise from
        // imperfect pairs is handled downstream by the 1-point outlier
        // split, and the clean case is handled by index-first above.)
        for i in 0..fa.len() {
            if matched_a[i] { continue; }
            let mut best: Option<usize> = None;
            let mut best_d = f64::INFINITY;
            for j in 0..fb.len() {
                if used_b[j] || !same_shape(&fa[i], &fb[j]) { continue; }
                let d = (centroid(&fa[i]) - centroid(&fb[j])).len();
                if d < best_d { best_d = d; best = Some(j); }
            }
            if let Some(j) = best {
                used_b[j] = true; matched_a[i] = true;
                pairs.push((i, j));
            }
        }
    }

    // ---- Tally + per-point deltas from the chosen pairs -------------------
    for &(i, j) in &pairs {
        out.matched += 1;
        let same = points_equal(&fa[i], &fb[j], eps) && scalars_equal(&fa[i], &fb[j], eps);
        if same {
            out.unchanged += 1;
        } else {
            out.changed += 1;
            for (p, q) in fa[i].pts.iter().zip(&fb[j].pts) {
                let delta = *q - *p;
                if delta.len() > eps { moved.push((*p, delta)); }
            }
        }
    }
    out.removed = matched_a.iter().filter(|m| !**m).count();
    out.added   = used_b.iter().filter(|u| !**u).count();

    // Cluster moved points by displacement vector — same vector = one
    // parameter (one stretch step). Symmetric moves (±x by equal amount)
    // form TWO clusters with one shared magnitude; the user merges them
    // into one named input later.
    for (before, delta) in moved {
        let mag = delta.len();
        if mag <= eps { continue; }
        let tol = (mag * 0.02).max(eps * 4.0);
        if let Some(c) = out.clusters.iter_mut()
            .find(|c| (c.dir * c.magnitude - delta).len() <= tol)
        {
            c.win_min.x = c.win_min.x.min(before.x);
            c.win_min.y = c.win_min.y.min(before.y);
            c.win_max.x = c.win_max.x.max(before.x);
            c.win_max.y = c.win_max.y.max(before.y);
            c.point_count += 1;
            c.points.push(before);
        } else {
            out.clusters.push(ParamCluster {
                win_min: before, win_max: before,
                dir: delta / mag, magnitude: mag, point_count: 1,
                points: vec![before],
            });
        }
    }
    // Strongest (most points) first — real parameters lead, 1-point
    // outliers (arc reshape / mis-match) sink to the bottom.
    out.clusters.sort_by(|x, y| y.point_count.cmp(&x.point_count)
        .then(y.magnitude.partial_cmp(&x.magnitude)
            .unwrap_or(std::cmp::Ordering::Equal)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Line, Polyline, PolyVertex};

    fn line(ax: f64, ay: f64, bx: f64, by: f64) -> DObject {
        DObject::new(Geom::Line(Line {
            a: Vec2::new(ax, ay), b: Vec2::new(bx, by),
        }))
    }

    #[test]
    fn identical_blocks_have_no_clusters() {
        let a = vec![line(0.0, 0.0, 10.0, 0.0), line(10.0, 0.0, 10.0, 10.0)];
        let b = a.clone();
        let d = diff_blocks(&a, Vec2::ZERO, &b, Vec2::ZERO, 1e-6);
        assert_eq!(d.matched, 2);
        assert_eq!(d.unchanged, 2);
        assert_eq!(d.changed, 0);
        assert!(d.clusters.is_empty());
    }

    #[test]
    fn one_moved_endpoint_is_one_cluster() {
        let a = vec![line(0.0, 0.0, 10.0, 0.0), line(10.0, 0.0, 10.0, 10.0)];
        // B: the shared right edge moved +x by 50 (both lines' right pts).
        let b = vec![line(0.0, 0.0, 60.0, 0.0), line(60.0, 0.0, 60.0, 10.0)];
        let d = diff_blocks(&a, Vec2::ZERO, &b, Vec2::ZERO, 1e-6);
        assert_eq!(d.changed, 2);
        assert_eq!(d.clusters.len(), 1, "all +x-by-50 moves cluster together");
        let c = &d.clusters[0];
        assert!((c.magnitude - 50.0).abs() < 1e-6);
        assert!((c.dir - Vec2::new(1.0, 0.0)).len() < 1e-6);
        assert_eq!(c.point_count, 3); // line1.b, line2.a, line2.b
    }

    #[test]
    fn index_pairing_clean_when_mostly_unchanged() {
        // 3 lines in the same order; only the last moved → index pairing
        // pairs i↔i exactly (no nearest-centroid scramble).
        let a = vec![
            line(0.0, 0.0, 10.0, 0.0),
            line(0.0, 5.0, 10.0, 5.0),
            line(0.0, 10.0, 10.0, 10.0),
        ];
        let b = vec![
            line(0.0, 0.0, 10.0, 0.0),
            line(0.0, 5.0, 10.0, 5.0),
            line(0.0, 10.0, 10.0, 40.0),   // last endpoint +30 in y
        ];
        let d = diff_blocks(&a, Vec2::ZERO, &b, Vec2::ZERO, 1e-6);
        assert_eq!(d.unchanged, 2);
        assert_eq!(d.changed, 1);
        assert_eq!(d.clusters.len(), 1);
        assert!((d.clusters[0].magnitude - 30.0).abs() < 1e-6);
    }

    #[test]
    fn base_relative_translation_reads_as_unchanged() {
        let a = vec![line(0.0, 0.0, 10.0, 0.0)];
        // Same geometry, drawn 1000 to the right, with base shifted too.
        let b = vec![line(1000.0, 0.0, 1010.0, 0.0)];
        let d = diff_blocks(&a, Vec2::ZERO, &b, Vec2::new(1000.0, 0.0), 1e-6);
        assert_eq!(d.unchanged, 1);
        assert!(d.clusters.is_empty());
    }

    #[test]
    fn closed_rectangle_right_edge_stretch() {
        let rect = |w: f64| DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex { pos: Vec2::new(0.0, 0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(w,   0.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(w,  10.0), bulge: 0.0 },
                PolyVertex { pos: Vec2::new(0.0, 10.0), bulge: 0.0 },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        let d = diff_blocks(&[rect(100.0)], Vec2::ZERO,
                            &[rect(130.0)], Vec2::ZERO, 1e-6);
        assert_eq!(d.changed, 1);
        assert_eq!(d.clusters.len(), 1);
        let c = &d.clusters[0];
        assert!((c.magnitude - 30.0).abs() < 1e-6);
        assert_eq!(c.point_count, 2); // the two right vertices
    }
}
