//! BPOLY-style boundary tracing for the Hatch pick-point flow.
//!
//! The classical AutoCAD BPOLY/BHATCH algorithm:
//!  1. Gather every visible dobject that could form a boundary.
//!  2. Cast a horizontal ray from the seed point. Sort intersections by t.
//!  3. The first hit lies on the outer boundary.
//!  4. Walk along the geometry, always picking the next edge that keeps
//!     the seed on the same side (smallest CCW turn → inside on the left).
//!  5. Trace returns to the start → closed loop. Repeat from hits 3, 5, …
//!     to find islands.
//!  6. Materialise as a polyline / region.
//!
//! Implementation strategy here:
//!  * Tessellate every dobject into short straight segments tagged with
//!    the source dobject index. Curves all become polylines, so the
//!    algorithm works uniformly across Line / Arc / Circle / Ellipse /
//!    EllipseArc / Polyline (bulge-aware) / Spline.
//!  * Cluster segment endpoints within `JOIN_EPS` so segments meeting at
//!    a shared corner share a graph node.
//!  * Build the adjacency: cluster → list of (segment, other_cluster,
//!    leaving angle).
//!  * Ray cast in +X: only consider segments whose endpoints straddle
//!    the seed's Y. Sort by x-intercept.
//!  * Trace each candidate loop with the smallest-CCW-turn rule. Cap
//!    walk length to avoid runaway on broken geometry.
//!  * Classify all traced loops: the outer is the largest one containing
//!    the seed; islands are the smaller ones inside the outer's bbox
//!    that do NOT contain the seed.
//!
//! Limitations of this v1 slice:
//!  * Pairwise intersections between drawn dobjects are NOT split — two
//!    crossing lines won't form a face at their crossing. The common
//!    "shapes connected at endpoints" case (rectangles, polygons with
//!    arc corners) works; "lens between two overlapping circles" does
//!    not. Splitting at intersections is a v2c follow-up.
//!
//! Cancellation / scaling roadmap (for the "1M dobjects" scenario the
//! user flagged):
//!  * Each heavy phase has a `_cancellable` variant taking
//!    `&AtomicBool`. They poll the flag every CANCEL_CHECK_STRIDE
//!    iterations and return whatever they've built so far when set.
//!  * Caller must verify the flag after each phase and discard the
//!    partial result on cancel.
//!  * `split_at_intersections_cancellable` is O(N²) and is THE
//!    bottleneck at scale. The next slice should:
//!     1. Replace pairwise scan with a spatial-index (UniformGrid)
//!        broad-phase so pairs to test become O(N·k) where k is the
//!        avg overlap count.
//!     2. Run the whole `trace_boundary_at_cancellable` on a worker
//!        thread (std::thread + mpsc), so the UI thread keeps
//!        spinning and Esc presses are seen mid-op. The cancel flag
//!        becomes the bridge between the UI Esc handler and the
//!        worker.
//!  * Today (synchronous): mid-op Esc is NOT honoured because the
//!    egui input snapshot is frozen for the duration of the
//!    `update` call. The cancel infrastructure is in place so
//!    that the work for (2) is a localised change, not a rewrite.

use std::sync::atomic::{AtomicBool, Ordering};

use cad_kernel::{DObject, Document, Geom, Vec2, EPS};

/// Cooperative-cancellation shim. The hatch trace pipeline reads
/// this between phases (and inside the heavy O(N²) loop) and bails
/// out early when set. Pass `&NEVER_CANCELLED` if you don't have one.
///
/// The caller — typically `apply_pick_point_hatch` — controls the
/// lifetime: it resets the flag before starting and is responsible
/// for sharing the same Arc with whatever sets the cancel (the
/// global Esc handler today; a future background-thread driver when
/// the trace gets moved off the UI thread).
pub fn never_cancelled() -> AtomicBool { AtomicBool::new(false) }

#[inline]
fn cancelled(c: &AtomicBool) -> bool {
    c.load(Ordering::Relaxed)
}

/// Stride between cancel checks inside the O(N²) split pass. Picked
/// so the check cost is negligible (one atomic load per ~256 inner
/// iterations) while still bailing within a few milliseconds even
/// on very large drawings.
const CANCEL_CHECK_STRIDE: usize = 256;

/// Tolerance for treating two segment endpoints as the same graph node.
/// Tessellation noise + user-drawn corners typically fall well within
/// 1e-4 world units; raise if your project's typical units are extreme.
pub const JOIN_EPS: f64 = 1e-4;

/// Cap on a single boundary walk's length, in segments. Prevents runaway
/// on broken or self-intersecting geometry. A real CAD drawing's
/// boundary should never need this many segments at our tessellation
/// density (32-64 per curve), so hitting the cap = malformed input.
const MAX_TRACE_STEPS: usize = 8192;

/// One short straight segment from the doc-wide tessellation. Carries
/// the source dobject's index so future refinements (intersection-
/// splitting, reuse-existing-boundary mode) can attribute each segment
/// back to the dobject it came from.
#[derive(Clone, Copy, Debug)]
pub struct TessSeg {
    pub a:   Vec2,
    pub b:   Vec2,
    #[allow(dead_code)]
    pub src: usize,
}

/// Per-cluster adjacency: list of (segment_index, other_cluster_id,
/// outgoing_angle). `outgoing_angle` is the direction vector pointing
/// from THIS cluster toward the OTHER cluster (i.e. the direction along
/// the segment when leaving this cluster).
type AdjEntry = (usize, usize, f64);

/// Tessellate every visible, non-hatch dobject into short straight
/// segments. Closed geometries (Circle / Ellipse / closed Polyline)
/// produce closed segment chains; open geometries produce open chains.
/// Hatch dobjects are skipped — they're an output of this pipeline, not
/// an input.
pub fn tessellate_doc(doc: &Document) -> Vec<TessSeg> {
    let never = never_cancelled();
    tessellate_doc_cancellable(doc, &never)
}

/// Cancellable variant. Checks the cancel flag every
/// `CANCEL_CHECK_STRIDE` dobjects and bails out, returning whatever
/// has been collected so far. Callers must verify the cancel flag
/// and treat a partial list as invalid.
pub fn tessellate_doc_cancellable(doc: &Document, cancel: &AtomicBool) -> Vec<TessSeg> {
    let mut out = Vec::new();
    for (i, d) in doc.dobjects.iter().enumerate() {
        if i & (CANCEL_CHECK_STRIDE - 1) == 0 && cancelled(cancel) {
            return out;
        }
        if !d.style.visible { continue; }
        if matches!(d.geom, Geom::Hatch(_) | Geom::Point(_)) { continue; }
        tessellate_one(d, i, &mut out);
    }
    out
}

/// Viewport-scoped + cancellable. Only tessellates the dobjects whose
/// indices are in `scope` (typically a spatial-index query result for
/// the visible world bbox). For a 400k-dobject doc with ~50 visible,
/// this is ~10000x cheaper than tessellating everything.
///
/// The check loop runs over `scope` instead of `doc.dobjects` so the
/// cost is bounded by `scope.len()`, not `doc.dobjects.len()`.
pub fn tessellate_doc_in_view_cancellable(
    doc:    &Document,
    scope:  &[usize],
    cancel: &AtomicBool,
) -> Vec<TessSeg> {
    let mut out = Vec::new();
    for (k, &i) in scope.iter().enumerate() {
        if k & (CANCEL_CHECK_STRIDE - 1) == 0 && cancelled(cancel) {
            return out;
        }
        let Some(d) = doc.dobjects.get(i) else { continue };
        if !d.style.visible { continue; }
        if matches!(d.geom, Geom::Hatch(_) | Geom::Point(_)) { continue; }
        tessellate_one(d, i, &mut out);
    }
    out
}

fn tessellate_one(d: &DObject, src: usize, out: &mut Vec<TessSeg>) {
    match &d.geom {
        Geom::Line(l) => {
            out.push(TessSeg { a: l.a, b: l.b, src });
        }
        Geom::Arc(a) => {
            push_arc(a.center, a.radius, a.start_angle, a.sweep_angle, 32, src, out);
        }
        Geom::Circle(c) => {
            push_arc(c.center, c.radius, 0.0, std::f64::consts::TAU, 64, src, out);
        }
        Geom::Ellipse(e) => {
            let mut last = e.point_at(0.0);
            let n = 64;
            for k in 1..=n {
                let t = (k as f64) / (n as f64) * std::f64::consts::TAU;
                let p = e.point_at(t);
                out.push(TessSeg { a: last, b: p, src });
                last = p;
            }
        }
        Geom::EllipseArc(ea) => {
            let n = 32;
            let mut last = ea.ellipse.point_at(ea.start_param);
            for k in 1..=n {
                let t = (k as f64) / (n as f64);
                let p = ea.ellipse.point_at(ea.start_param + ea.sweep_param * t);
                out.push(TessSeg { a: last, b: p, src });
                last = p;
            }
        }
        Geom::Polyline(p) => {
            let n = p.vertices.len();
            if n < 2 { return; }
            let end = if p.closed { n } else { n - 1 };
            for k in 0..end {
                let a = p.vertices[k].pos;
                let b = p.vertices[(k + 1) % n].pos;
                push_bulged(a, b, p.vertices[k].bulge, src, out);
            }
        }
        Geom::Spline(s) => {
            let samples = s.tessellate(64);
            for w in samples.windows(2) {
                out.push(TessSeg { a: w[0], b: w[1], src });
            }
        }
        Geom::Wall(w) => {
            // Hatch boundary tracing — emit the two visible side lines
            // so the wall contributes as a real boundary candidate.
            if let Some(l) = w.left_line() {
                out.push(TessSeg { a: l.a, b: l.b, src });
            }
            if let Some(r) = w.right_line() {
                out.push(TessSeg { a: r.a, b: r.b, src });
            }
        }
        // BlockRef: instances don't contribute hatch-boundary segments in
        // v1 (contents resolve through the Document, which the
        // tessellator doesn't carry). Explode to hatch against contents.
        Geom::Point(_) | Geom::Hatch(_) | Geom::Text(_) | Geom::Dimension(_)
            | Geom::BlockRef(_) => {}
    }
}

fn push_arc(centre: Vec2, r: f64, start: f64, sweep: f64,
            n: usize, src: usize, out: &mut Vec<TessSeg>)
{
    let mut last = Vec2::new(
        centre.x + r * start.cos(),
        centre.y + r * start.sin());
    for k in 1..=n {
        let t = (k as f64) / (n as f64);
        let ang = start + sweep * t;
        let p = Vec2::new(centre.x + r * ang.cos(), centre.y + r * ang.sin());
        out.push(TessSeg { a: last, b: p, src });
        last = p;
    }
}

fn push_bulged(a: Vec2, b: Vec2, bulge: f64, src: usize, out: &mut Vec<TessSeg>) {
    if bulge.abs() < 1e-9 {
        out.push(TessSeg { a, b, src });
        return;
    }
    let chord = b - a;
    let chord_len = chord.len();
    if chord_len < EPS {
        out.push(TessSeg { a, b, src });
        return;
    }
    let theta = 4.0 * bulge.atan();
    let half = theta * 0.5;
    let sin_half = half.sin();
    if sin_half.abs() < EPS {
        out.push(TessSeg { a, b, src });
        return;
    }
    let r = chord_len / (2.0 * sin_half.abs());
    let chord_hat = chord / chord_len;
    let perp = Vec2::new(-chord_hat.y, chord_hat.x);
    let mid  = (a + b) * 0.5;
    let centre_off = r * half.cos();
    let centre = mid + perp * (if bulge > 0.0 { centre_off } else { -centre_off });
    let start_ang = (a - centre).angle();
    let end_ang   = (b - centre).angle();
    let sweep = if bulge > 0.0 {
        (end_ang - start_ang).rem_euclid(std::f64::consts::TAU)
    } else {
        -((start_ang - end_ang).rem_euclid(std::f64::consts::TAU))
    };
    let n = 24_usize;
    let mut last = a;
    for k in 1..=n {
        let t = (k as f64) / (n as f64);
        let ang = start_ang + sweep * t;
        let p = centre + Vec2::new(r * ang.cos(), r * ang.sin());
        out.push(TessSeg { a: last, b: p, src });
        last = p;
    }
}

/// Cluster segment endpoints within `JOIN_EPS`. Returns
///   * `endpoint_clusters[i] = (cluster_a, cluster_b)` for each segment
///   * `cluster_pos[k]` = representative position of cluster `k`
///
/// Naive O(N·C) — fine for typical doc sizes; swap for a grid hash when
/// profiling shows it matters.
pub fn cluster_endpoints(segs: &[TessSeg])
    -> (Vec<(usize, usize)>, Vec<Vec2>)
{
    let never = never_cancelled();
    cluster_endpoints_cancellable(segs, &never)
}

/// Cancellable variant. Naive matching is O(N·C) — for many-cluster
/// drawings this is the second-most-expensive phase after split.
/// Checks the cancel flag every `CANCEL_CHECK_STRIDE` segments.
/// Returns a partial result on cancel; caller must verify.
pub fn cluster_endpoints_cancellable(
    segs: &[TessSeg],
    cancel: &AtomicBool,
) -> (Vec<(usize, usize)>, Vec<Vec2>) {
    let mut centres: Vec<Vec2> = Vec::new();
    let mut endpoints: Vec<(usize, usize)> = Vec::with_capacity(segs.len());
    let find_or_add = |p: Vec2, centres: &mut Vec<Vec2>| -> usize {
        for (i, c) in centres.iter().enumerate() {
            if (*c - p).len() < JOIN_EPS { return i; }
        }
        centres.push(p);
        centres.len() - 1
    };
    for (i, s) in segs.iter().enumerate() {
        if i & (CANCEL_CHECK_STRIDE - 1) == 0 && cancelled(cancel) {
            return (endpoints, centres);
        }
        let a_id = find_or_add(s.a, &mut centres);
        let b_id = find_or_add(s.b, &mut centres);
        endpoints.push((a_id, b_id));
    }
    (endpoints, centres)
}

/// Build the adjacency table: for each cluster, list every segment that
/// has it as an endpoint, along with the OTHER cluster the segment
/// connects to and the outgoing angle (direction LEAVING this cluster).
pub fn build_adjacency(
    segs: &[TessSeg],
    endpoints: &[(usize, usize)],
    n_clusters: usize,
) -> Vec<Vec<AdjEntry>> {
    let mut adj: Vec<Vec<AdjEntry>> = vec![Vec::new(); n_clusters];
    for (i, s) in segs.iter().enumerate() {
        let (a_id, b_id) = endpoints[i];
        if a_id == b_id { continue; }       // degenerate, skip
        let dir_from_a = (s.b - s.a).angle();
        let dir_from_b = (s.a - s.b).angle();
        adj[a_id].push((i, b_id, dir_from_a));
        adj[b_id].push((i, a_id, dir_from_b));
    }
    adj
}

/// Segment-segment intersection in parametric form. Returns `(t, u, pt)`
/// where `pt = p.a + t*(p.b - p.a) = q.a + u*(q.b - q.a)` and both
/// `t, u ∈ [0, 1]`. Returns `None` for parallel segments or for
/// intersections lying outside either segment's parameter range.
pub fn seg_seg_intersect_params(p: &TessSeg, q: &TessSeg) -> Option<(f64, f64, Vec2)> {
    let r = p.b - p.a;
    let s = q.b - q.a;
    let rxs = r.x * s.y - r.y * s.x;
    if rxs.abs() < 1e-12 { return None; }   // parallel
    let qp = q.a - p.a;
    let t = (qp.x * s.y - qp.y * s.x) / rxs;
    let u = (qp.x * r.y - qp.y * r.x) / rxs;
    if !(0.0..=1.0).contains(&t) || !(0.0..=1.0).contains(&u) { return None; }
    Some((t, u, p.a + r * t))
}

/// Pairwise intersection splitting pass — the missing primitive that
/// makes the trace path work for partial overlaps.
///
/// Walks every segment pair where the two segments come from DIFFERENT
/// source dobjects (same-source pairs already share endpoints at chain
/// joints — no split needed there). For each crossing that lies
/// strictly in the interior of BOTH segments, both segments get a new
/// vertex inserted at the crossing point and are split into two
/// fragments.
///
/// After this pass, the endpoint-clustering step naturally treats every
/// crossing as a graph node, and the CCW trace walks through the
/// planar subdivision correctly — partial overlaps trace the
/// intersection region instead of either source dobject's whole
/// boundary.
///
/// Complexity: O(N²) over segments, with each pair being a fast 2D
/// cross-product check. At 200 segs (typical interactive drawing)
/// that's 20k checks → sub-millisecond. Spatial-index acceleration
/// is a future optimisation if profiling demands it.
pub fn split_at_intersections(segs: &[TessSeg]) -> Vec<TessSeg> {
    let never = never_cancelled();
    split_at_intersections_cancellable(segs, &never)
}

/// Cancellable variant. Checks the cancel flag every
/// `CANCEL_CHECK_STRIDE` inner-loop iterations and bails out
/// returning whatever fragments have been collected so far. The
/// returned segment list may be incomplete in that case — callers
/// must verify the cancel flag and discard the result on cancel.
pub fn split_at_intersections_cancellable(
    segs: &[TessSeg],
    cancel: &AtomicBool,
) -> Vec<TessSeg> {
    let n = segs.len();
    if n < 2 { return segs.to_vec(); }
    let mut cuts: Vec<Vec<(f64, Vec2)>> = vec![Vec::new(); n];
    let mut counter: usize = 0;
    for i in 0..n {
        if cancelled(cancel) { return segs.to_vec(); }
        for j in (i + 1)..n {
            counter += 1;
            if counter & (CANCEL_CHECK_STRIDE - 1) == 0 && cancelled(cancel) {
                return segs.to_vec();
            }
            if segs[i].src == segs[j].src { continue; }
            let Some((ti, tj, pt)) = seg_seg_intersect_params(&segs[i], &segs[j]) else { continue; };
            // Endpoint-touching intersections are already handled by
            // cluster_endpoints; only interior crossings need a split.
            if ti > 1e-6 && ti < 1.0 - 1e-6 {
                cuts[i].push((ti, pt));
            }
            if tj > 1e-6 && tj < 1.0 - 1e-6 {
                cuts[j].push((tj, pt));
            }
        }
    }
    let mut out: Vec<TessSeg> = Vec::with_capacity(n);
    for (i, s) in segs.iter().enumerate() {
        if cuts[i].is_empty() {
            out.push(*s);
            continue;
        }
        cuts[i].sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut prev = s.a;
        for (_, pt) in &cuts[i] {
            // Skip near-duplicate splits (two intersections at almost
            // the same point — would create a zero-length fragment).
            if (*pt - prev).len() < 1e-7 { continue; }
            out.push(TessSeg { a: prev, b: *pt, src: s.src });
            prev = *pt;
        }
        // Tail fragment — only emit if non-degenerate.
        if (s.b - prev).len() > 1e-7 {
            out.push(TessSeg { a: prev, b: s.b, src: s.src });
        }
    }
    out
}

/// Cast a +X horizontal ray from `seed` against every segment. Returns
/// `(t, seg_idx, hit_pos)` sorted by `t`, with `t > 0` (strictly east of
/// the seed). Horizontal segments are ignored — they lie ALONG the ray
/// and contribute no clean crossing.
pub fn ray_cast_horiz(seed: Vec2, segs: &[TessSeg])
    -> Vec<(f64, usize, Vec2)>
{
    let mut hits = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        let ay = s.a.y - seed.y;
        let by = s.b.y - seed.y;
        // Skip horizontal-ish segments (both endpoints near the ray).
        if ay.abs() < EPS && by.abs() < EPS { continue; }
        // Half-open Y-test — one endpoint above, one below.
        let above_a = ay > 0.0;
        let above_b = by > 0.0;
        if above_a == above_b { continue; }
        // Solve y = seed.y → a + u*(b - a), u ∈ [0,1].
        let u = -ay / (by - ay);
        if !(u >= 0.0 && u <= 1.0) { continue; }
        let x = s.a.x + u * (s.b.x - s.a.x);
        let t = x - seed.x;
        if t > EPS {
            hits.push((t, i, Vec2::new(x, seed.y)));
        }
    }
    hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    hits
}

/// Trace a closed loop starting from segment `start_seg`, leaving
/// `start_cluster`. At each junction, picks the smallest CCW turn —
/// which traces the face on the LEFT of the walk direction.
///
/// Returns vertex positions including the closing repeat of the start;
/// `None` if the walk hits a dead-end (cluster with no other edge) or
/// the trace exceeds `MAX_TRACE_STEPS`.
pub fn trace_loop(
    start_seg: usize,
    start_cluster: usize,
    segs: &[TessSeg],
    endpoints: &[(usize, usize)],
    adj: &[Vec<AdjEntry>],
    cluster_pos: &[Vec2],
) -> Option<Vec<Vec2>> {
    let _ = segs; // unused — kept for future intersection-split refinement
    let mut verts: Vec<Vec2> = Vec::new();
    let mut cur_seg = start_seg;
    let mut cur_cluster = start_cluster;
    verts.push(cluster_pos[cur_cluster]);
    for _step in 0..MAX_TRACE_STEPS {
        let (a_id, b_id) = endpoints[cur_seg];
        let next_cluster = if cur_cluster == a_id { b_id } else if cur_cluster == b_id { a_id }
            else { return None };
        verts.push(cluster_pos[next_cluster]);
        if next_cluster == start_cluster {
            return Some(verts);
        }
        // At each junction, the CCW turn from "arrive_back" to each
        // outgoing edge tells us how far around (CCW) we'd rotate to
        // face that edge. To bound the face on our LEFT (the face that
        // contains the seed, which is what we want to hatch), we pick
        // the LARGEST valid CCW turn — that's the sharpest LEFT turn
        // = stay hugging the left face's boundary. Picking smallest
        // CCW would bound the RIGHT face instead.
        //
        // We start the walk with the seed on our LEFT (the +X ray hits
        // the east boundary of the seed's region; we begin walking
        // from the south endpoint, putting the west — and the seed —
        // on our left), so LEFT-face = seed's face = the region to
        // hatch.
        let in_dir = (cluster_pos[next_cluster] - cluster_pos[cur_cluster]).angle();
        let arrive_back = (in_dir + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU);
        let mut best: Option<(usize, f64)> = None;
        for &(sidx, _other, out_dir) in &adj[next_cluster] {
            if sidx == cur_seg { continue; }
            let turn = (out_dir - arrive_back).rem_euclid(std::f64::consts::TAU);
            // Reject ~0 and ~2π (coincident reverse / back-edge — would
            // turn around and walk back the way we came).
            if turn < 1e-6 || turn > std::f64::consts::TAU - 1e-6 { continue; }
            if best.map_or(true, |(_, t)| turn > t) {
                best = Some((sidx, turn));
            }
        }
        let (next_seg, _) = best?;
        cur_seg = next_seg;
        cur_cluster = next_cluster;
    }
    None
}

/// Polygon area, signed. Positive = CCW, negative = CW.
fn polygon_signed_area(verts: &[Vec2]) -> f64 {
    let n = verts.len();
    if n < 3 { return 0.0; }
    let mut a = 0.0;
    for i in 0..n {
        let p = verts[i];
        let q = verts[(i + 1) % n];
        a += p.x * q.y - q.x * p.y;
    }
    a * 0.5
}

/// Standard even-odd ray-cast PIP — duplicated here so the module has
/// no dependency on app-level helpers.
fn point_in_polygon(p: Vec2, verts: &[Vec2]) -> bool {
    let n = verts.len();
    if n < 3 { return false; }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let pi = verts[i];
        let pj = verts[j];
        if (pi.y > p.y) != (pj.y > p.y) {
            let x_int = pi.x + (p.y - pi.y) * (pj.x - pi.x) / (pj.y - pi.y);
            if p.x < x_int { inside = !inside; }
        }
        j = i;
    }
    inside
}

fn polygon_bbox(verts: &[Vec2]) -> (Vec2, Vec2) {
    let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
    let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in verts {
        if v.x < min.x { min.x = v.x; }
        if v.y < min.y { min.y = v.y; }
        if v.x > max.x { max.x = v.x; }
        if v.y > max.y { max.y = v.y; }
    }
    (min, max)
}

/// Two polygons "the same" if their bbox centres and absolute areas
/// match within tolerance. Cheap dedup — robust enough for the
/// "different hits, same traced loop" case.
fn polygons_equivalent(a: &[Vec2], b: &[Vec2]) -> bool {
    let area_a = polygon_signed_area(a).abs();
    let area_b = polygon_signed_area(b).abs();
    if (area_a - area_b).abs() > area_a.max(area_b) * 1e-3 + 1e-6 { return false; }
    let (amin, amax) = polygon_bbox(a);
    let (bmin, bmax) = polygon_bbox(b);
    let acx = (amin.x + amax.x) * 0.5;
    let acy = (amin.y + amax.y) * 0.5;
    let bcx = (bmin.x + bmax.x) * 0.5;
    let bcy = (bmin.y + bmax.y) * 0.5;
    ((acx - bcx).abs() + (acy - bcy).abs()) < 1e-4 * (area_a.sqrt() + 1.0)
}

/// Result of a successful boundary trace.
#[derive(Debug)]
pub struct TracedBoundary {
    /// Outer loop, CCW orientation, last vertex repeats the first.
    pub outer: Vec<Vec2>,
    /// Island loops, contained inside `outer`. Each closed (first == last).
    pub islands: Vec<Vec<Vec2>>,
}

/// Full BPOLY pipeline. Returns `None` if no closed boundary surrounds
/// the seed, or if every trace attempt failed.
pub fn trace_boundary_at(doc: &Document, seed: Vec2) -> Option<TracedBoundary> {
    let raw = tessellate_doc(doc);
    let mut tb = trace_boundary_from_raw(raw, seed)?;
    let scope: Vec<usize> = (0..doc.dobjects.len()).collect();
    augment_islands_from_closed_dobjects(doc, &scope, seed, &mut tb);
    Some(tb)
}

/// Viewport-scoped variant — tessellates ONLY dobjects whose indices
/// are in `scope` (typically a spatial-index query result for the
/// visible world bbox). At 400k+ dobjects with ~50 visible, this
/// makes the trace tractable; without it the trace tessellates all
/// 400k and the worker thread runs effectively forever.
pub fn trace_boundary_at_in_view(
    doc:   &Document,
    scope: &[usize],
    seed:  Vec2,
) -> Option<TracedBoundary> {
    let never = never_cancelled();
    let raw = tessellate_doc_in_view_cancellable(doc, scope, &never);
    let mut tb = trace_boundary_from_raw(raw, seed)?;
    augment_islands_from_closed_dobjects(doc, scope, seed, &mut tb);
    Some(tb)
}

/// Cancellable variant of `trace_boundary_at_in_view`. Used by the
/// async worker thread — checks the cancel flag between phases so
/// the user's Esc actually fires mid-op.
pub fn trace_boundary_at_in_view_cancellable(
    doc:    &Document,
    scope:  &[usize],
    seed:   Vec2,
    cancel: &AtomicBool,
) -> Option<TracedBoundary> {
    let raw = tessellate_doc_in_view_cancellable(doc, scope, cancel);
    if cancelled(cancel) { return None; }
    let segs = split_at_intersections_cancellable(&raw, cancel);
    if cancelled(cancel) { return None; }
    let mut tb = trace_boundary_from_segs(segs, seed)?;
    // Ray-cast tracing only discovers loops that the +X ray actually
    // crosses. Closed primitives that sit above/below the seed's
    // horizontal ray (and therefore aren't hit) get silently missed
    // as islands. Sweep the doc once more for closed dobjects fully
    // contained in the traced outer and add them.
    augment_islands_from_closed_dobjects(doc, scope, seed, &mut tb);
    Some(tb)
}

/// Approximate a single closed kernel dobject as a polygon. Used for
/// island detection — we need to know which closed primitives sit
/// fully inside the traced outer so they can be cut out. Returns
/// `None` for dobjects whose boundary isn't a single closed loop
/// (lines, arcs, open polylines, etc.).
fn closed_geom_to_polygon(d: &DObject) -> Option<Vec<Vec2>> {
    use std::f64::consts::TAU;
    match &d.geom {
        Geom::Circle(c) => {
            let n = 64;
            let mut verts = Vec::with_capacity(n + 1);
            for k in 0..=n {
                let t = (k as f64) / (n as f64) * TAU;
                verts.push(Vec2::new(
                    c.center.x + c.radius * t.cos(),
                    c.center.y + c.radius * t.sin()));
            }
            Some(verts)
        }
        Geom::Ellipse(e) => {
            let n = 64;
            let mut verts = Vec::with_capacity(n + 1);
            for k in 0..=n {
                let t = (k as f64) / (n as f64) * TAU;
                verts.push(e.point_at(t));
            }
            Some(verts)
        }
        Geom::Polyline(p) if p.closed && p.vertices.len() >= 3 => {
            let mut verts: Vec<Vec2> = Vec::new();
            let n = p.vertices.len();
            for k in 0..n {
                let a = p.vertices[k].pos;
                let b = p.vertices[(k + 1) % n].pos;
                // Reuse the same bulge tessellation the segs path uses
                // so the polygon matches the trace pipeline's idea of
                // this boundary. push_bulged is private so duplicate
                // the straight-edge case (most polylines) and accept
                // bulged segments as straight chords here — good
                // enough for island containment tests.
                if p.vertices[k].bulge.abs() < 1e-9 {
                    verts.push(a);
                } else {
                    // Approximate the arc with a few vertices.
                    let bulge = p.vertices[k].bulge;
                    let chord = b - a;
                    let chord_len = chord.len();
                    if chord_len < 1e-9 { verts.push(a); continue; }
                    let sagitta = bulge * chord_len * 0.5;
                    let radius = (chord_len * chord_len / 4.0 + sagitta * sagitta)
                        / (2.0 * sagitta.abs());
                    let _ = radius;  // not needed; sample by angle steps
                    let steps = 12;
                    for s in 0..steps {
                        let t = (s as f64) / (steps as f64);
                        verts.push(a + chord * t);
                    }
                }
            }
            // Close the loop
            if let Some(first) = verts.first().copied() {
                verts.push(first);
            }
            Some(verts)
        }
        _ => None,
    }
}

/// After the main trace succeeds, scan `scope` for closed kernel
/// dobjects that:
///   * are fully inside the traced outer (every vertex inside),
///   * do NOT contain the seed,
///   * are not already represented in `tb.islands`.
/// Each match becomes an additional island. Fixes the "+X ray missed
/// this island" failure mode without changing the ray-based trace.
fn augment_islands_from_closed_dobjects(
    doc:   &Document,
    scope: &[usize],
    seed:  Vec2,
    tb:    &mut TracedBoundary,
) {
    let (omin, omax) = polygon_bbox(&tb.outer);
    for &idx in scope {
        let Some(d) = doc.dobjects.get(idx) else { continue; };
        let Some(poly) = closed_geom_to_polygon(d) else { continue; };
        let (pmin, pmax) = polygon_bbox(&poly);
        // Quick bbox reject before the expensive PIP loop.
        if pmin.x < omin.x - JOIN_EPS || pmin.y < omin.y - JOIN_EPS
            || pmax.x > omax.x + JOIN_EPS || pmax.y > omax.y + JOIN_EPS
        { continue; }
        // Seed must NOT be inside this poly — otherwise it would be a
        // candidate for the outer, not an island.
        if point_in_polygon(seed, &poly) { continue; }
        // Don't duplicate islands already found via the ray trace.
        if tb.islands.iter().any(|i| polygons_equivalent(i, &poly)) { continue; }
        // Don't duplicate the outer.
        if polygons_equivalent(&tb.outer, &poly) { continue; }
        // Every vertex of the polygon must lie inside the outer.
        // Even ONE vertex outside means the dobject crosses the outer
        // boundary (probably split by chord lines) — let the ray-
        // based trace handle that case rather than over-cutting.
        if !poly.iter().all(|v| point_in_polygon(*v, &tb.outer)) { continue; }
        tb.islands.push(poly);
    }
}

/// Shared trace pipeline given the raw segment soup. Used by both the
/// full-doc and viewport-scoped entry points.
fn trace_boundary_from_raw(raw: Vec<TessSeg>, seed: Vec2) -> Option<TracedBoundary> {
    if raw.is_empty() { return None; }
    let segs = split_at_intersections(&raw);
    trace_boundary_from_segs(segs, seed)
}

/// Iteratively remove "dangle" segments — segments whose far endpoint
/// is a graph node of degree 1. These are tree branches (open ends)
/// that no closed walk can include; leaving them in causes
/// `trace_loop` to walk INTO the dangle and fail when it can't
/// continue past the free endpoint.
///
/// Repeat until stable: each removed segment may expose a new dangle
/// at the OTHER endpoint, which we then also prune. After convergence,
/// every surviving segment is on at least one cycle.
///
/// Operates by mutating `adj` and returning a `dead[seg]` mask. The
/// segs / endpoints / cluster_pos arrays stay untouched — the trace
/// walker reads adj exclusively for "what's next from this cluster",
/// and ray_cast hits get filtered through `dead` so we never start a
/// walk from a pruned segment either.
fn prune_dangles(adj: &mut [Vec<AdjEntry>], n_segs: usize) -> Vec<bool> {
    let mut dead = vec![false; n_segs];
    loop {
        let mut changed = false;
        for c in 0..adj.len() {
            // Filter previously-dead segs out of this cluster's adj.
            let before = adj[c].len();
            adj[c].retain(|(s, _, _)| !dead[*s]);
            if adj[c].len() != before { changed = true; }
            // If this leaves the cluster with exactly one outgoing
            // edge, that edge is a dangle — kill it.
            if adj[c].len() == 1 {
                let (sidx, other, _) = adj[c][0];
                dead[sidx] = true;
                adj[c].clear();
                adj[other].retain(|(s, _, _)| *s != sidx);
                changed = true;
            }
        }
        if !changed { break; }
    }
    dead
}

/// Shared trace pipeline given POST-SPLIT segments. Caller is
/// responsible for any cancel checks during the upstream phases.
fn trace_boundary_from_segs(segs: Vec<TessSeg>, seed: Vec2) -> Option<TracedBoundary> {
    if segs.is_empty() { return None; }
    // (no further split — segs are already split)
    let (endpoints, cluster_pos) = cluster_endpoints(&segs);
    let n_clusters = cluster_pos.len();
    let mut adj = build_adjacency(&segs, &endpoints, n_clusters);
    // Prune tree-branch dangles before tracing. Without this, lines
    // crossing a closed boundary (e.g. a line through a circle) leave
    // outside-the-circle stubs whose far endpoints are degree-1, and
    // the CCW walker walks INTO them and fails. After pruning, every
    // surviving edge is on at least one cycle and the walker closes.
    let dead = prune_dangles(&mut adj, segs.len());
    let hits = ray_cast_horiz(seed, &segs);
    // Hits on pruned (dangle) segments aren't valid starting points
    // either — filter them out before iterating.
    let hits: Vec<(f64, usize, Vec2)> = hits.into_iter()
        .filter(|(_, sidx, _)| !dead[*sidx])
        .collect();
    if hits.is_empty() { return None; }

    // Try to trace a loop from EACH hit. Each starting hit is a
    // segment that crosses the +X ray. We start the walk at the SOUTH
    // endpoint of that segment so the walk direction is northward,
    // which puts the WEST side (where the seed is) on our LEFT.
    //
    // Dedupe hits that land on the same world point — when several
    // dobjects share a vertex AND the +X ray happens to pass through
    // it, the same (x, y) shows up once per dobject. Each duplicate
    // gives the same trace outcome, so iterating them wastes work
    // and clutters the debug log.
    let mut seen_hits: Vec<Vec2> = Vec::new();
    let hits_deduped: Vec<&(f64, usize, Vec2)> = hits.iter().filter(|(_, _, p)| {
        if seen_hits.iter().any(|s| (*s - *p).len() < 1e-6) {
            false
        } else {
            seen_hits.push(*p);
            true
        }
    }).collect();
    let mut traced: Vec<Vec<Vec2>> = Vec::new();
    for &&(_, seg_idx, _) in &hits_deduped {
        let (a_id, b_id) = endpoints[seg_idx];
        let pa = cluster_pos[a_id];
        let pb = cluster_pos[b_id];
        let start_cluster = if pa.y < pb.y { a_id } else { b_id };
        let Some(loop_verts) = trace_loop(
            seg_idx, start_cluster, &segs, &endpoints, &adj, &cluster_pos)
        else { continue; };
        if loop_verts.len() < 3 { continue; }
        if traced.iter().any(|t| polygons_equivalent(t, &loop_verts)) { continue; }
        traced.push(loop_verts);
    }
    if traced.is_empty() { return None; }

    // Classify: outer = SMALLEST loop containing the seed (AutoCAD
    // BPOLY semantics — click in a nested region, hatch the innermost
    // bounded one). Islands = strictly-smaller loops inside the outer
    // that do NOT contain the seed.
    let mut outer_idx: Option<usize> = None;
    let mut outer_area = f64::INFINITY;
    for (i, t) in traced.iter().enumerate() {
        if !point_in_polygon(seed, t) { continue; }
        let area = polygon_signed_area(t).abs();
        if area < outer_area {
            outer_area = area;
            outer_idx = Some(i);
        }
    }
    let outer_idx = outer_idx?;
    let outer = traced[outer_idx].clone();
    let (omin, omax) = polygon_bbox(&outer);
    let mut islands: Vec<Vec<Vec2>> = Vec::new();
    for (i, t) in traced.iter().enumerate() {
        if i == outer_idx { continue; }
        let (tmin, tmax) = polygon_bbox(t);
        let inside_bbox = tmin.x >= omin.x - JOIN_EPS && tmin.y >= omin.y - JOIN_EPS
                       && tmax.x <= omax.x + JOIN_EPS && tmax.y <= omax.y + JOIN_EPS;
        if !inside_bbox { continue; }
        if point_in_polygon(seed, t) { continue; }   // would not be an island
        // Need at least one vertex of the candidate inside `outer` to
        // count as a real island (filters out unrelated loops far away).
        if !t.iter().any(|v| point_in_polygon(*v, &outer)) { continue; }
        if islands.iter().any(|x| polygons_equivalent(x, t)) { continue; }
        islands.push(t.clone());
    }
    Some(TracedBoundary { outer, islands })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_kernel::{Circle, DObject, Document, Line, Polyline, PolyVertex};

    fn doc_from(dobjects: Vec<DObject>) -> Document {
        let mut doc = Document::default();
        for d in dobjects {
            doc.push(d);
        }
        doc
    }

    /// 4 lines forming a 10×10 square at the origin — no closed
    /// polyline involved. Click at (5,5) should trace the square.
    #[test]
    fn square_from_four_lines() {
        let doc = doc_from(vec![
            Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }.into(),
            Line { a: Vec2::new(10.0, 0.0), b: Vec2::new(10.0, 10.0) }.into(),
            Line { a: Vec2::new(10.0, 10.0), b: Vec2::new(0.0, 10.0) }.into(),
            Line { a: Vec2::new(0.0, 10.0), b: Vec2::new(0.0, 0.0) }.into(),
        ]);
        let tb = trace_boundary_at(&doc, Vec2::new(5.0, 5.0))
            .expect("must trace 4-line square");
        let area = polygon_signed_area(&tb.outer).abs();
        assert!((area - 100.0).abs() < 1e-3, "expected ~100, got {}", area);
        assert!(tb.islands.is_empty());
    }

    /// Outer square + inner circle island. Click inside square but
    /// outside circle. Expected: outer = square, islands = [circle].
    #[test]
    fn square_with_circle_island() {
        let doc = doc_from(vec![
            // Outer closed polyline rectangle
            Polyline {
                vertices: vec![
                    PolyVertex { pos: Vec2::new(0.0, 0.0),  bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(20.0, 0.0), bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(20.0, 20.0),bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(0.0, 20.0), bulge: 0.0 },
                ],
                closed: true,
                widths: Vec::new(),
            }.into(),
            // Inner circle
            Circle { center: Vec2::new(10.0, 10.0), radius: 3.0 }.into(),
        ]);
        // Seed inside square but to the LEFT of the circle
        let tb = trace_boundary_at(&doc, Vec2::new(3.0, 10.0))
            .expect("must trace outer + island");
        let outer_area = polygon_signed_area(&tb.outer).abs();
        assert!((outer_area - 400.0).abs() < 1e-2,
            "expected ~400, got {}", outer_area);
        assert_eq!(tb.islands.len(), 1, "expected exactly one island");
    }

    /// Click inside the circle, NOT the square: outer should be the
    /// circle (the smallest containing loop).
    #[test]
    fn click_inside_island_makes_island_the_outer() {
        let doc = doc_from(vec![
            Polyline {
                vertices: vec![
                    PolyVertex { pos: Vec2::new(0.0, 0.0),  bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(20.0, 0.0), bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(20.0, 20.0),bulge: 0.0 },
                    PolyVertex { pos: Vec2::new(0.0, 20.0), bulge: 0.0 },
                ],
                closed: true,
                widths: Vec::new(),
            }.into(),
            Circle { center: Vec2::new(10.0, 10.0), radius: 3.0 }.into(),
        ]);
        // Seed at circle centre
        let tb = trace_boundary_at(&doc, Vec2::new(10.0, 10.0))
            .expect("must trace circle as outer");
        let area = polygon_signed_area(&tb.outer).abs();
        let circle_area_approx = std::f64::consts::PI * 3.0 * 3.0;
        let rel = (area - circle_area_approx).abs() / circle_area_approx;
        // 64-segment polygon → < 0.5% area error vs analytic
        assert!(rel < 0.01,
            "expected ~{}, got {} (rel err {})", circle_area_approx, area, rel);
    }

    /// Seed in empty space → ray finds no hits → None.
    #[test]
    fn no_boundary_returns_none() {
        let doc = doc_from(vec![
            Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) }.into(),
        ]);
        assert!(trace_boundary_at(&doc, Vec2::new(5.0, 100.0)).is_none());
    }

    /// The partial-overlap case the user's log surfaced: two circles
    /// overlap forming a lens. Click in the lens. The trace should
    /// find a region SMALLER than either circle alone — proof that
    /// the split-at-intersections pass is doing its job.
    #[test]
    fn lens_between_two_overlapping_circles() {
        let doc = doc_from(vec![
            Circle { center: Vec2::new(0.0, 0.0), radius: 10.0 }.into(),
            Circle { center: Vec2::new(8.0, 0.0), radius: 10.0 }.into(),
        ]);
        // Seed in the lens (between the two centres, on the x-axis
        // both circles cover this point)
        let tb = trace_boundary_at(&doc, Vec2::new(4.0, 0.0))
            .expect("must trace the lens region between two overlapping circles");
        let area = polygon_signed_area(&tb.outer).abs();
        let single_circle_area = std::f64::consts::PI * 100.0;
        // The lens is strictly smaller than either single circle.
        assert!(area > 0.0 && area < single_circle_area,
            "lens area {} should be > 0 and < single-circle area {}",
            area, single_circle_area);
    }

    /// Three overlapping circles with a common region in the middle.
    /// Click in the common region — trace must walk a boundary made of
    /// THREE different circles' arc fragments, only possible after
    /// pairwise intersection splitting.
    #[test]
    fn common_region_of_three_overlapping_circles() {
        let doc = doc_from(vec![
            Circle { center: Vec2::new(-3.0, -2.0), radius: 6.0 }.into(),
            Circle { center: Vec2::new( 3.0, -2.0), radius: 6.0 }.into(),
            Circle { center: Vec2::new( 0.0,  3.0), radius: 6.0 }.into(),
        ]);
        // Seed at the centroid — inside all three circles
        let tb = trace_boundary_at(&doc, Vec2::new(0.0, 0.0))
            .expect("must trace the 3-circle common region");
        let area = polygon_signed_area(&tb.outer).abs();
        let single_circle_area = std::f64::consts::PI * 36.0;
        assert!(area > 0.0 && area < single_circle_area * 0.5,
            "common-region area {} should be < half of one circle's area {}",
            area, single_circle_area);
    }

    /// REGRESSION: circle with a line crossing through it — the
    /// line's two outside-the-circle stubs are dangles (their outer
    /// endpoints are degree-1 graph nodes). Before the dangle-prune
    /// pass, the CCW walker walked INTO one of those stubs and the
    /// whole trace returned None. The chord splits the disk into two
    /// half-discs; a seed in either half should trace a closed loop
    /// strictly smaller than the full disc.
    #[test]
    fn circle_chord_with_outside_stubs_traces_half_disc() {
        let doc = doc_from(vec![
            Circle { center: Vec2::new(0.0, 0.0), radius: 10.0 }.into(),
            // Horizontal chord — endpoints OUTSIDE the circle so the
            // line has dangles on both sides.
            Line { a: Vec2::new(-15.0, 0.0), b: Vec2::new(15.0, 0.0) }.into(),
        ]);
        // Seed in the lower half.
        let tb = trace_boundary_at(&doc, Vec2::new(0.0, -5.0))
            .expect("must trace lower half-disc despite outside-the-circle line stubs");
        let area = polygon_signed_area(&tb.outer).abs();
        let full_disc = std::f64::consts::PI * 100.0;
        assert!(area > full_disc * 0.3 && area < full_disc * 0.7,
            "half-disc area {} should be ~half of full disc {}", area, full_disc);
    }

    /// REGRESSION: circle with MULTIPLE crossing lines — every line
    /// has both endpoints outside the circle, so all 8 line stubs are
    /// dangles. Picking inside a sub-region bounded by an arc + line
    /// chords must succeed.
    #[test]
    fn circle_with_many_crossing_lines() {
        let doc = doc_from(vec![
            Circle { center: Vec2::new(0.0, 0.0), radius: 10.0 }.into(),
            Line { a: Vec2::new(-15.0, -3.0), b: Vec2::new(15.0,  3.0) }.into(),
            Line { a: Vec2::new(-15.0,  3.0), b: Vec2::new(15.0, -3.0) }.into(),
        ]);
        // Seed in the small region just above the X crossing.
        let tb = trace_boundary_at(&doc, Vec2::new(0.0, 5.0))
            .expect("must trace a chord-bounded region of the disc");
        let area = polygon_signed_area(&tb.outer).abs();
        let full_disc = std::f64::consts::PI * 100.0;
        assert!(area > 0.0 && area < full_disc,
            "region area {} must be between 0 and full disc {}", area, full_disc);
    }

    /// REGRESSION: an island that the +X ray from the seed never
    /// touches must still be detected. The ray-based trace only finds
    /// loops the ray crosses, so a closed primitive lying entirely
    /// above the seed's y is invisible to it. The post-trace doc
    /// scan catches it.
    #[test]
    fn island_above_seed_is_detected_by_doc_scan() {
        let doc = doc_from(vec![
            // Outer container: big circle at origin r=20.
            Circle { center: Vec2::new(0.0, 0.0), radius: 20.0 }.into(),
            // Island fully above the seed (y range [8..12], r=2).
            Circle { center: Vec2::new(0.0, 10.0), radius: 2.0 }.into(),
        ]);
        // Seed in the lower half — its +X ray (y=-5) never reaches
        // the island at y∈[8,12].
        let tb = trace_boundary_at(&doc, Vec2::new(0.0, -5.0))
            .expect("must trace outer disc");
        assert!(!tb.islands.is_empty(),
            "island above the seed's ray must still appear in tb.islands; got {} islands",
            tb.islands.len());
    }

    /// Splitting a single seg by itself is a no-op.
    #[test]
    fn split_at_intersections_noop_when_no_crossings() {
        let segs = vec![
            TessSeg { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0), src: 0 },
            TessSeg { a: Vec2::new(0.0, 5.0), b: Vec2::new(10.0, 5.0), src: 0 },
        ];
        let out = split_at_intersections(&segs);
        assert_eq!(out.len(), 2);
    }

    /// Two perpendicular segments crossing at their midpoints split
    /// each one in half — 2 input segs → 4 output segs.
    #[test]
    fn split_at_intersections_basic_cross() {
        let segs = vec![
            TessSeg { a: Vec2::new(-5.0, 0.0), b: Vec2::new(5.0, 0.0),  src: 0 },
            TessSeg { a: Vec2::new(0.0, -5.0), b: Vec2::new(0.0, 5.0),  src: 1 },
        ];
        let out = split_at_intersections(&segs);
        assert_eq!(out.len(), 4, "expected 4 sub-segments, got {}", out.len());
        // All sub-segments touch the origin
        let near_origin = out.iter().filter(|s|
            (s.a - Vec2::new(0.0, 0.0)).len() < 1e-9
            || (s.b - Vec2::new(0.0, 0.0)).len() < 1e-9
        ).count();
        assert_eq!(near_origin, 4, "every fragment should touch the cross point");
    }

    /// Same-source crossings are NOT split (a self-intersecting open
    /// polyline's tessellation fragments shouldn't generate spurious
    /// splits at the self-intersection — that's a degenerate input).
    #[test]
    fn split_at_intersections_skips_same_source() {
        let segs = vec![
            TessSeg { a: Vec2::new(-5.0, 0.0), b: Vec2::new(5.0, 0.0),  src: 0 },
            TessSeg { a: Vec2::new(0.0, -5.0), b: Vec2::new(0.0, 5.0),  src: 0 },
        ];
        let out = split_at_intersections(&segs);
        assert_eq!(out.len(), 2, "same-source segs should NOT be split");
    }
}
