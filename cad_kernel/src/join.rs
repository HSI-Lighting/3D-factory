//! join.rs — segment joining, polyline explosion, and bulge-arc helpers.
//!
//! Split out of `geom.rs` verbatim (pure code-movement refactor). Contains
//! the JOIN command (collinear-line / concentric-arc / touching-chain merge),
//! the `polyline_segments` exploder, and the DXF bulge<->arc conversions.

use crate::math::{Vec2, EPS};
use crate::geom::{Arc, Geom, Line, PolyVertex, Polyline};

/// Circular arc through `a`→`b` with DXF `bulge` (= tan(sweep/4)). Returns
/// `(center, radius, start_angle, signed_sweep)`; signed sweep is
/// `4·atan(bulge)` so positive = CCW, negative = CW. `None` if degenerate.
pub fn bulge_arc(a: Vec2, b: Vec2, bulge: f64) -> Option<(Vec2, f64, f64, f64)> {
    let chord = b - a;
    let l = chord.len();
    if l < EPS || bulge.abs() < 1e-12 { return None; }
    let r = l * (1.0 + bulge * bulge) / (4.0 * bulge.abs());
    let mid = (a + b) * 0.5;
    let perp = chord.perp() / l;
    let d = r * (1.0 - bulge * bulge) / (1.0 + bulge * bulge);
    let center = mid + perp * (d * bulge.signum());
    let start_angle = (a - center).angle();
    let sweep = 4.0 * bulge.atan();
    Some((center, r, start_angle, sweep))
}

/// The DXF bulge for traversing the arc start→end about `center`.
/// Magnitude = tan(sweep/4); sign = + when start→end runs CCW about the centre,
/// − when CW. The direction is decided by comparing the CCW angle (start→end)
/// against the actual swept magnitude — NOT by which side of the chord the
/// centre lies on. The chord-side test is only valid for MINOR arcs (sweep <
/// π); for a MAJOR arc it flips while tan(sweep/4) already encodes the wider
/// span, double-counting and inverting the curvature.
pub fn bulge_from_arc(start: Vec2, end: Vec2, center: Vec2, sweep_abs: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let a0 = (start - center).angle();
    let a1 = (end - center).angle();
    let ccw = (a1 - a0).rem_euclid(tau);   // CCW angle start→end, [0, 2π)
    let mag = sweep_abs.abs();
    // Circular distance helper (shortest angular gap, accounting for wrap).
    let circ = |x: f64, y: f64| { let d = (x - y).abs() % tau; d.min(tau - d) };
    // CCW if the CCW angle matches the swept magnitude better than its CW
    // complement (τ − mag) does.
    let sign = if circ(ccw, mag) <= circ(ccw, tau - mag) { 1.0 } else { -1.0 };
    sign * (mag * 0.25).tan()
}

/// The arc a polyline segment with `bulge` sweeps from `a` to `b`: center,
/// radius, start angle and SIGNED sweep (positive = CCW, like a positive
/// bulge). None for a zero/near-zero bulge, a degenerate chord, or a
/// full-circle bulge (whose center can't be derived from a chord alone).
/// Inverse of [`bulge_from_arc`].
pub fn arc_from_bulge(a: Vec2, b: Vec2, bulge: f64) -> Option<(Vec2, f64, f64, f64)> {
    use crate::math::EPS;
    if bulge.abs() < 1e-12 { return None; }
    let chord = b - a;
    let chord_len = chord.len();
    if chord_len < EPS { return None; }
    let theta = 4.0 * bulge.atan();
    if theta.abs() >= std::f64::consts::TAU - 1e-9 { return None; }   // full circle
    let half = theta * 0.5;
    let sin_half = half.sin();
    if sin_half.abs() < EPS { return None; }
    let r = chord_len / (2.0 * sin_half.abs());
    let chord_hat = chord / chord_len;
    let perp = Vec2::new(-chord_hat.y, chord_hat.x);
    let mid = (a + b) * 0.5;
    let centre_off = r * half.cos();
    let center = mid + perp * (if bulge > 0.0 { centre_off } else { -centre_off });
    let start_ang = (a - center).angle();
    let end_ang = (b - center).angle();
    let sweep = if bulge > 0.0 {
        (end_ang - start_ang).rem_euclid(std::f64::consts::TAU)
    } else {
        -((start_ang - end_ang).rem_euclid(std::f64::consts::TAU))
    };
    Some((center, r, start_ang, sweep))
}

// ---------------------------------------------------------------------------
// Polyline segments — explode into independent Line / Arc geoms.
//
// Each vertex `i` owns the bulge for segment `i → (i+1)`. Straight when
// bulge == 0; otherwise an Arc derived from chord + DXF bulge formula.
// Closed polylines also produce the closing segment (last → first).
// ---------------------------------------------------------------------------
pub fn polyline_segments(p: &Polyline) -> Vec<Geom> {
    let n = p.vertices.len();
    if n < 2 { return Vec::new(); }
    let seg_count = if p.closed { n } else { n - 1 };
    let mut out = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let v_i = p.vertices[i];
        let v_n = p.vertices[(i + 1) % n];
        if v_i.bulge.abs() < EPS {
            out.push(Geom::Line(Line { a: v_i.pos, b: v_n.pos }));
        } else {
            let chord = v_n.pos - v_i.pos;
            let l = chord.len();
            if l < EPS { continue; }
            let b = v_i.bulge;
            let r = l * (1.0 + b * b) / (4.0 * b.abs());
            let mid = (v_i.pos + v_n.pos) * 0.5;
            let perp = chord.perp() / l;
            let d = r * (1.0 - b * b) / (1.0 + b * b);
            let center = mid + perp * (d * b.signum());
            let start_angle = (v_i.pos - center).angle().rem_euclid(std::f64::consts::TAU);
            let end_angle   = (v_n.pos - center).angle().rem_euclid(std::f64::consts::TAU);
            let raw_sweep = (end_angle - start_angle).rem_euclid(std::f64::consts::TAU);
            let arc = if b > 0.0 {
                Arc { center, radius: r, start_angle, sweep_angle: raw_sweep }
            } else {
                let rev_sweep = std::f64::consts::TAU - raw_sweep;
                Arc { center, radius: r, start_angle: end_angle, sweep_angle: rev_sweep }
            };
            out.push(Geom::Arc(arc));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Join — Slice M.5.
//
// Three merge classes, applied in order:
//   1. Collinear Lines  → one Line covering the union extent.
//   2. Concentric Arcs of equal radius with touching sweeps → one Arc.
//   3. Any chain of touching segments (Lines + Arcs, end-to-end) → Polyline.
//      Open chain → open polyline; closed chain → closed polyline.
// Unjoinable inputs come back unchanged in `unmodified`.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct JoinOut {
    /// The merged Geoms produced this run. Each replaces one or more inputs.
    pub merged: Vec<Geom>,
    /// Indices INTO the input slice that participated in some merge.
    /// Callers remove these from the doc before appending `merged`.
    pub consumed_indices: Vec<usize>,
}

/// Try to merge the given geoms (referenced by `indices` into the doc) into
/// one or more bigger geoms. Each input index is paired with its `Geom`.
pub fn join_geoms(geoms: &[(usize, Geom)]) -> JoinOut {
    let mut merged       = Vec::new();
    let mut consumed     = Vec::new();
    let mut available: Vec<(usize, Geom)> = geoms.iter().cloned().collect();

    // -- pass 1: collinear lines ------------------------------------------------
    loop {
        let group = find_collinear_line_group(&available);
        if group.len() < 2 { break; }
        let line_geoms: Vec<Line> = group.iter()
            .filter_map(|&i| match &available[i].1 { Geom::Line(l) => Some(*l), _ => None })
            .collect();
        if let Some(merged_line) = merge_collinear_lines(&line_geoms) {
            for &local_i in &group { consumed.push(available[local_i].0); }
            // remove from `available` in descending index order
            let mut sorted = group.clone();
            sorted.sort_unstable_by(|a, b| b.cmp(a));
            for li in sorted { available.remove(li); }
            merged.push(Geom::Line(merged_line));
        } else { break; }
    }

    // -- pass 2: concentric arcs (same center, same radius, touching sweeps) ----
    loop {
        let group = find_concentric_arc_group(&available);
        if group.len() < 2 { break; }
        let arcs: Vec<Arc> = group.iter()
            .filter_map(|&i| match &available[i].1 { Geom::Arc(a) => Some(*a), _ => None })
            .collect();
        if let Some(merged_arc) = merge_concentric_arcs(&arcs) {
            for &local_i in &group { consumed.push(available[local_i].0); }
            let mut sorted = group.clone();
            sorted.sort_unstable_by(|a, b| b.cmp(a));
            for li in sorted { available.remove(li); }
            merged.push(Geom::Arc(merged_arc));
        } else { break; }
    }

    // -- pass 3: chain of touching Lines + Arcs → Polyline ---------------------
    loop {
        let chain = find_touching_chain(&available);
        if chain.len() < 2 { break; }
        if let Some(pl) = chain_to_polyline(&chain.iter().map(|&i| available[i].1.clone()).collect::<Vec<_>>()) {
            for &local_i in &chain { consumed.push(available[local_i].0); }
            let mut sorted = chain.clone();
            sorted.sort_unstable_by(|a, b| b.cmp(a));
            for li in sorted { available.remove(li); }
            merged.push(Geom::Polyline(pl));
        } else { break; }
    }

    JoinOut { merged, consumed_indices: consumed }
}

pub(crate) const JOIN_EPS: f64 = 1e-6;
/// Fuzzy endpoint-coincidence tolerance for CHAINING segments into a polyline.
/// Looser than JOIN_EPS because an arc's endpoint is reconstructed via trig
/// (`center + r·(cosθ,sinθ)`), so even a perfectly-filleted arc end can sit a
/// few ×1e-6 off the touching line end. JOIN_EPS stays tight for the
/// collinear/concentric precision tests.
const CHAIN_EPS: f64 = 1e-3;

fn find_collinear_line_group(items: &[(usize, Geom)]) -> Vec<usize> {
    // Returns local indices of a run of >= 2 collinear Lines that actually
    // TOUCH end-to-end (a contiguous, gap-free overlap). Collinear lines with a
    // GAP between them (e.g. the two stubs a trim leaves when it cuts a line at
    // a crossing) must NOT merge — bridging that gap would redraw the removed
    // middle piece. Only touching collinear runs collapse into one clean Line.
    for i in 0..items.len() {
        let li = if let Geom::Line(l) = &items[i].1 { *l } else { continue };
        let dir_i = li.b - li.a;
        let len_i = dir_i.len();
        if len_i < JOIN_EPS { continue; }
        let u_i = dir_i / len_i;
        let perp = u_i.perp();
        let proj = |p: Vec2| (p - li.a).dot(u_i);
        // Collect every collinear line with its projected [tmin, tmax] span.
        let mut members: Vec<(usize, f64, f64)> = Vec::new();
        for j in 0..items.len() {
            let lj = if let Geom::Line(l) = &items[j].1 { *l } else { continue };
            let dir_j = lj.b - lj.a;
            let len_j = dir_j.len();
            if len_j < JOIN_EPS { continue; }
            // Same infinite line: parallel + lj.a lies on li's infinite line.
            let cross = u_i.x * dir_j.y - u_i.y * dir_j.x;
            if cross.abs() > JOIN_EPS * len_j { continue; }
            if (lj.a - li.a).dot(perp).abs() > JOIN_EPS { continue; }
            let (mut ta, mut tb) = (proj(lj.a), proj(lj.b));
            if ta > tb { std::mem::swap(&mut ta, &mut tb); }
            members.push((j, ta, tb));
        }
        if members.len() < 2 { continue; }
        // Sort along the line, then split into contiguous runs — a gap larger
        // than CHAIN_EPS between consecutive spans starts a new run. Return the
        // first gap-free run of >= 2 that contains the seed `i`.
        members.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let mut run: Vec<usize> = Vec::new();
        let mut run_max = f64::NEG_INFINITY;
        for (idx, ta, tb) in members {
            if run.is_empty() || ta <= run_max + CHAIN_EPS {
                run.push(idx);
                if tb > run_max { run_max = tb; }
            } else {
                if run.len() >= 2 && run.contains(&i) { return run; }
                run = vec![idx];
                run_max = tb;
            }
        }
        if run.len() >= 2 && run.contains(&i) { return run; }
    }
    Vec::new()
}

fn merge_collinear_lines(lines: &[Line]) -> Option<Line> {
    if lines.is_empty() { return None; }
    let l0 = lines[0];
    let u = (l0.b - l0.a).normalized();
    // Project every endpoint onto u; take min/max.
    let project = |p: Vec2| (p - l0.a).dot(u);
    let mut t_min = f64::INFINITY;
    let mut t_max = f64::NEG_INFINITY;
    for l in lines {
        for p in [l.a, l.b] {
            let t = project(p);
            if t < t_min { t_min = t; }
            if t > t_max { t_max = t; }
        }
    }
    Some(Line { a: l0.a + u * t_min, b: l0.a + u * t_max })
}

fn find_concentric_arc_group(items: &[(usize, Geom)]) -> Vec<usize> {
    for i in 0..items.len() {
        let ai = if let Geom::Arc(a) = &items[i].1 { *a } else { continue };
        let mut group = vec![i];
        for j in (i + 1)..items.len() {
            let aj = if let Geom::Arc(a) = &items[j].1 { *a } else { continue };
            if (aj.center - ai.center).len() > JOIN_EPS { continue; }
            if (aj.radius - ai.radius).abs() > JOIN_EPS { continue; }
            group.push(j);
        }
        // Only return the group if at least two of them have sweeps that
        // CONNECT (one's end is another's start, within EPS).
        if group.len() >= 2 && arcs_form_a_chain(&group.iter()
            .filter_map(|&k| if let Geom::Arc(a) = &items[k].1 { Some(*a) } else { None })
            .collect::<Vec<_>>())
        {
            return group;
        }
    }
    Vec::new()
}

/// True iff the arcs' angular sweeps union into a SINGLE contiguous range (or
/// the whole circle). G6: routed through `circular_union` (gap-rotation), so a
/// chain that crosses the 0°/360° SEAM is recognised — the old sort-low→high
/// sweep treated 0° as a hard wall and rejected any wrapping chain. Two-or-more
/// merged intervals ⇒ a genuine gap ⇒ not a chain. Strict superset of the old
/// behaviour: every non-wrapping chain it accepted still collapses to one
/// interval. (`circular_union`'s internal eps is `1e-6`, same as `JOIN_EPS`, so
/// the touching threshold is unchanged.)
fn arcs_form_a_chain(arcs: &[Arc]) -> bool {
    if arcs.len() < 2 { return false; }
    let ivs = arc_intervals(arcs);
    let (merged, full) = crate::math::circular_union(&ivs);
    full || merged.len() == 1
}

fn merge_concentric_arcs(arcs: &[Arc]) -> Option<Arc> {
    if arcs.is_empty() { return None; }
    let ivs = arc_intervals(arcs);
    let (merged, full) = crate::math::circular_union(&ivs);
    // Concentric ⇒ centre/radius are shared; take them from arcs[0] (as before).
    let (center, radius) = (arcs[0].center, arcs[0].radius);
    if full {
        return Some(Arc { center, radius,
            start_angle: 0.0, sweep_angle: std::f64::consts::TAU });
    }
    match merged.as_slice() {
        // Single contiguous range (incl. across the seam) → that IS the arc.
        [(s, len)] => Some(Arc { center, radius, start_angle: *s, sweep_angle: *len }),
        // A gap remains: merge is only called after `arcs_form_a_chain`, so this
        // shouldn't happen — return None rather than fabricate a span.
        _ => None,
    }
}

/// `(start.rem_euclid(τ), sweep)` intervals — exactly `circular_union`'s
/// `(start, LENGTH)` form.
fn arc_intervals(arcs: &[Arc]) -> Vec<(f64, f64)> {
    arcs.iter()
        .map(|a| (a.start_angle.rem_euclid(std::f64::consts::TAU), a.sweep_angle))
        .collect()
}

fn endpoints_of(g: &Geom) -> Option<(Vec2, Vec2)> {
    match g {
        Geom::Line(l)       => Some((l.a, l.b)),
        Geom::Arc(a)        => Some(a.endpoints()),
        Geom::EllipseArc(e) => Some(e.endpoints()),
        Geom::Polyline(p) if !p.closed && !p.vertices.is_empty() =>
            Some((p.vertices.first()?.pos, p.vertices.last()?.pos)),
        _ => None,
    }
}

fn find_touching_chain(items: &[(usize, Geom)]) -> Vec<usize> {
    // BFS from each item; connect via shared endpoints (within EPS).
    if items.is_empty() { return Vec::new(); }
    for start in 0..items.len() {
        if endpoints_of(&items[start].1).is_none() { continue; }
        let mut seen = vec![false; items.len()];
        seen[start] = true;
        let mut queue = vec![start];
        let mut group = vec![start];
        while let Some(cur) = queue.pop() {
            let Some((ca, cb)) = endpoints_of(&items[cur].1) else { continue };
            for j in 0..items.len() {
                if seen[j] { continue; }
                let Some((ja, jb)) = endpoints_of(&items[j].1) else { continue };
                let touches = ca.dist(ja) < CHAIN_EPS || ca.dist(jb) < CHAIN_EPS
                           || cb.dist(ja) < CHAIN_EPS || cb.dist(jb) < CHAIN_EPS;
                if touches {
                    seen[j] = true;
                    queue.push(j);
                    group.push(j);
                }
            }
        }
        if group.len() >= 2 { return group; }
    }
    Vec::new()
}

fn chain_to_polyline(geoms: &[Geom]) -> Option<Polyline> {
    // Walk the chain endpoint-to-endpoint, emitting one PolyVertex per
    // vertex along the way. Arcs contribute a `bulge` to the vertex BEFORE
    // them (DXF convention: bulge belongs to the segment FROM that vertex).
    if geoms.len() < 2 { return None; }
    // Build an unordered set, then traverse.
    let mut remaining: Vec<Geom> = geoms.to_vec();
    // Pick a starting endpoint: any vertex that's only touched ONCE among
    // the unordered set is an open-chain end. If every vertex is touched
    // twice, the chain is closed.
    let mut endpoint_count: Vec<(Vec2, usize)> = Vec::new();
    let bump = |list: &mut Vec<(Vec2, usize)>, p: Vec2| {
        for (q, c) in list.iter_mut() {
            if q.dist(p) < CHAIN_EPS { *c += 1; return; }
        }
        list.push((p, 1));
    };
    for g in &remaining {
        let Some((a, b)) = endpoints_of(g) else { return None; };
        bump(&mut endpoint_count, a);
        bump(&mut endpoint_count, b);
    }
    let chain_closed = endpoint_count.iter().all(|&(_, c)| c == 2);
    let start_pt: Vec2 = if chain_closed {
        endpoint_count[0].0
    } else {
        endpoint_count.iter().find(|&&(_, c)| c == 1).map(|&(p, _)| p)?
    };

    let mut current = start_pt;
    let mut verts: Vec<PolyVertex> = vec![PolyVertex { pos: current, bulge: 0.0 }];
    while !remaining.is_empty() {
        // Find a segment that touches `current`.
        let mut found: Option<usize> = None;
        let mut reverse = false;
        for (i, g) in remaining.iter().enumerate() {
            let Some((a, b)) = endpoints_of(g) else { continue };
            if a.dist(current) < CHAIN_EPS { found = Some(i); reverse = false; break; }
            if b.dist(current) < CHAIN_EPS { found = Some(i); reverse = true;  break; }
        }
        let i = found?;
        let seg = remaining.remove(i);
        // Far endpoint relative to the entry point `current`. Compute it
        // DIRECTLY from the segment's two endpoints and the side we entered on
        // — do NOT route through `Geom::reversed()`. `Arc::reversed()` cannot
        // encode a CW traversal under the positive-sweep invariant, so it
        // returns the wrong far endpoint (P(start+2·sweep)) and stalls the
        // walk. `endpoints_of` + the entry side is unambiguous for every type.
        let (sa, sb) = endpoints_of(&seg)?;
        let next = if reverse { sa } else { sb };
        // Bulge for the segment OUT of `current` toward `next`. DXF bulge =
        // tan(included_angle / 4), signed by which side of the chord
        // (current→next) the centre lies. Because the chord direction follows
        // the actual traversal, bulge_from_arc gets the sign right for EITHER
        // direction — no reversal needed.
        let bulge_for_last = match &seg {
            Geom::Arc(a) => bulge_from_arc(current, next, a.center, a.sweep_angle),
            _ => 0.0,
        };
        if let Some(last) = verts.last_mut() { last.bulge = bulge_for_last; }
        if remaining.is_empty() && chain_closed {
            // Don't push the closing vertex — `closed: true` carries it.
            break;
        }
        verts.push(PolyVertex { pos: next, bulge: 0.0 });
        current = next;
    }

    Some(Polyline { vertices: verts, closed: chain_closed, widths: Vec::new() })
}

#[cfg(test)]
mod concentric_arc_join_tests {
    use super::*;
    use std::f64::consts::TAU;

    fn arc(start_deg: f64, sweep_deg: f64) -> Arc {
        Arc { center: Vec2::ZERO, radius: 5.0,
              start_angle: start_deg.to_radians(),
              sweep_angle: sweep_deg.to_radians() }
    }
    fn deg(r: f64) -> f64 { r.to_degrees() }

    // G6 — the fix: a chain that CROSSES the 0°/360° seam. A covers 350°→10°,
    // B covers 10°→30°; together a contiguous 350°→30° arc. The old low→high
    // sweep treated 0° as a wall and called this "not a chain".
    #[test]
    fn wrap_around_chain_is_recognised_and_merged() {
        let arcs = [arc(350.0, 20.0), arc(10.0, 20.0)];
        assert!(arcs_form_a_chain(&arcs), "wrap chain must be a chain");
        let m = merge_concentric_arcs(&arcs).expect("wrap chain merges");
        assert!((deg(m.start_angle) - 350.0).abs() < 1e-4, "start {}", deg(m.start_angle));
        assert!((deg(m.sweep_angle) - 40.0).abs() < 1e-4, "sweep {}", deg(m.sweep_angle));
    }

    // Two half-arcs cover the whole circle → full.
    #[test]
    fn two_half_arcs_are_a_full_circle() {
        let arcs = [arc(0.0, 180.0), arc(180.0, 180.0)];
        assert!(arcs_form_a_chain(&arcs));
        let m = merge_concentric_arcs(&arcs).unwrap();
        assert!((m.sweep_angle - TAU).abs() < 1e-6, "sweep {}", deg(m.sweep_angle));
    }

    // ⚠️ THE TRAP GUARD: overlapping arcs whose sweeps SUM past τ (400°) but only
    // COVER 300°. Must merge to 300°, NOT a full circle. Fails if the disjoint-
    // only `total >= τ` shortcut is reintroduced.
    #[test]
    fn overlapping_arcs_summing_past_tau_are_not_a_full_circle() {
        let arcs = [arc(0.0, 200.0), arc(100.0, 200.0)];
        assert!(arcs_form_a_chain(&arcs));
        let m = merge_concentric_arcs(&arcs).unwrap();
        assert!((deg(m.sweep_angle) - 300.0).abs() < 1e-4,
            "overlap covers 300°, got {}", deg(m.sweep_angle));
        assert!(m.sweep_angle < TAU - 1e-6, "must NOT be a full circle");
    }

    // A genuine gap → not a chain, no merge.
    #[test]
    fn disjoint_arcs_with_a_gap_are_not_a_chain() {
        let arcs = [arc(0.0, 30.0), arc(90.0, 30.0)];
        assert!(!arcs_form_a_chain(&arcs), "a real gap is not a chain");
        assert!(merge_concentric_arcs(&arcs).is_none(),
            "two disjoint intervals → no single merged arc");
    }

    // Non-wrapping regression: the case that already worked must still work.
    #[test]
    fn adjacent_non_wrapping_arcs_still_chain() {
        let arcs = [arc(0.0, 30.0), arc(30.0, 30.0)];
        assert!(arcs_form_a_chain(&arcs));
        let m = merge_concentric_arcs(&arcs).unwrap();
        assert!((deg(m.start_angle) - 0.0).abs() < 1e-4, "start {}", deg(m.start_angle));
        assert!((deg(m.sweep_angle) - 60.0).abs() < 1e-4, "sweep {}", deg(m.sweep_angle));
    }

    // NESTED overlap (B entirely inside A): a nested input still merges to A's
    // span, not a false gap/circle.
    #[test]
    fn nested_overlap_merges_to_the_outer_span() {
        let arcs = [arc(0.0, 300.0), arc(90.0, 30.0)];
        assert!(arcs_form_a_chain(&arcs));
        let m = merge_concentric_arcs(&arcs).unwrap();
        assert!((deg(m.sweep_angle) - 300.0).abs() < 1e-4,
            "nested union is the outer 300° span, got {}", deg(m.sweep_angle));
    }

    // ⚠️ NESTED TRIPLE (mentor-caught B16b bug): outer 0°→300° with TWO small
    // inners at 10°→15° and 20°→25°. The shipped per-interval gap-finder found a
    // FALSE gap between the two inners (inside the outer) and stranded one inner
    // as a spurious 2nd interval → chain FALSE. The running-`cover_end` finder
    // sees the real gap at 330° → one [0,300] cover → chain true. (Fails-before
    // against the per-interval finder.)
    #[test]
    fn nested_triple_with_two_inners_is_a_single_cover() {
        let arcs = [arc(0.0, 300.0), arc(10.0, 5.0), arc(20.0, 5.0)];
        assert!(arcs_form_a_chain(&arcs),
            "outer + two nested inners is a single 300° cover, not a gap");
        let m = merge_concentric_arcs(&arcs).unwrap();
        assert!((deg(m.start_angle) - 0.0).abs() < 1e-4, "start {}", deg(m.start_angle));
        assert!((deg(m.sweep_angle) - 300.0).abs() < 1e-4,
            "merged span is the outer 300°, got {}", deg(m.sweep_angle));
    }
}
