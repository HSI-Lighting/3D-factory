//! **Cutting a mesh** with a profile drawn on one of its faces.
//!
//! The app's parametric pieces — doors, cupboards, kitchens, desks, stairs, ramps, apertures — are
//! not CSG feature trees. They are finished triangle meshes, so [`crate::csg`]'s `BoolOp::Difference`
//! (which subtracts one *feature* from another) cannot reach them. This module is the missing half:
//! it subtracts a drawn prism from a triangle soup, using the same csgrs BSP the feature evaluator
//! uses, and hands back a [`SolidMesh`].
//!
//! Three things make it a tool rather than a boolean call.
//!
//! **It refuses meshes it cannot cut.** A BSP subtraction is only meaningful on a CLOSED solid. Hand
//! it an open shell — most downloaded models, and anything exported as a surface — and it returns
//! something that looks plausible in one view and is nonsense in another: faces that vanish at
//! grazing angles, interiors that light as if they were outside. [`closure`] measures the mesh
//! first and [`apply`] refuses on that evidence, with the number of open edges, rather than
//! producing a wrong answer confidently.
//!
//! **It carries part ids through the boolean.** csgrs polygons hold arbitrary metadata, so the mesh
//! goes in as `Mesh<u32>` with each triangle tagged by the component it belongs to. Without that a
//! cut door would come back as one anonymous blob and lose every per-part material the user had
//! chosen. The cutter carries its OWN id, so the exposed inner surface of the cut is a part in its
//! own right — selectable and paintable, which is what you want for a rebate or a service hole.
//!
//! **Cuts are a LIST, evaluated from the original.** Nothing is baked. `apply` always starts from
//! the asset's untouched geometry and replays every enabled cut, so a cut can be disabled, deleted
//! or re-ordered later and the mesh returns exactly to what it was.

use crate::{Frame, SolidMesh};
use csgrs::csg::CSG;
use csgrs::mesh::Mesh;
use csgrs::polygon::Polygon;
use csgrs::sketch::Sketch;
use csgrs::vertex::Vertex;
use nalgebra::{Matrix4, Point3, Vector3};

/// Mesh carrying a part id per polygon, so a boolean cannot erase which component a triangle
/// belongs to.
type PartMesh = Mesh<u32>;

/// Over-cut, metres. The prism starts this far OUTSIDE the face it was drawn on, so the boolean
/// never has to decide about two coplanar surfaces — the case BSP trees get wrong.
const OVERCUT: f64 = 1e-3;

/// One cut: a closed profile on a plane, swept along that plane's inward normal.
///
/// `frame` is in the mesh's **own local space**, never world space. A cut is a property of the
/// object, so moving or rotating the piece has to carry its holes with it; storing a world frame
/// would leave the holes behind the moment the piece was dragged.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshCut {
    /// Closed loop in the frame's `(u, v)`, metres. First and last point need not coincide.
    pub profile: Vec<[f32; 2]>,
    pub frame: Frame,
    /// How far in, along `−frame.normal()`. Ignored when `through`.
    pub depth: f32,
    /// Punch all the way out the far side instead of stopping at `depth`.
    pub through: bool,
    /// Off keeps the cut in the list without applying it — the point of an editable list.
    pub enabled: bool,
    /// What the user sees in the cut list.
    pub label: String,
}

impl MeshCut {
    /// A through-cut from a drawn loop.
    pub fn through(frame: Frame, profile: Vec<[f32; 2]>, label: impl Into<String>) -> Self {
        Self { profile, frame, depth: 0.0, through: true, enabled: true, label: label.into() }
    }

    /// A blind pocket `depth` metres deep.
    pub fn pocket(frame: Frame, profile: Vec<[f32; 2]>, depth: f32, label: impl Into<String>) -> Self {
        Self { profile, frame, depth: depth.max(1e-4), through: false, enabled: true, label: label.into() }
    }
}

/// Why a cut could not be made. Every variant carries the measurement behind it, because "it
/// didn't work" sends someone hunting and "417 open edges" tells them what to fix.
#[derive(Clone, Debug, PartialEq)]
pub enum CutError {
    /// The mesh is not a closed solid. `open` counts edges used by other than exactly two
    /// triangles — the holes and the seams.
    NotClosed { open: usize, tris: usize },
    /// Nothing to cut.
    EmptyMesh,
    /// The profile has fewer than three usable points, or encloses no area.
    DegenerateProfile,
    /// The boolean ran and removed nothing — the prism missed the body entirely.
    NothingRemoved,
    /// The boolean ran and removed everything.
    EverythingRemoved,
}

impl std::fmt::Display for CutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CutError::NotClosed { open, tris } => write!(
                f,
                "this mesh is not a closed solid ({open} open edges in {tris} triangles) — \
                 only generated pieces can be cut, not imported models"
            ),
            CutError::EmptyMesh => write!(f, "there is no geometry to cut"),
            CutError::DegenerateProfile => write!(f, "the drawn shape encloses no area"),
            CutError::NothingRemoved => write!(f, "the cut missed the body — nothing was removed"),
            CutError::EverythingRemoved => write!(f, "the cut would remove the whole piece"),
        }
    }
}

/// How closed a triangle soup is: the number of BOUNDARY edges, and the triangle count they were
/// measured over.
///
/// Two subtleties, both learned the hard way on this crate's own geometry.
///
/// Vertices are welded by QUANTIZED POSITION, not by index. This is triangle soup — the two
/// triangles either side of an edge carry their own copies of its endpoints, and matching by index
/// would report every edge as open on a perfectly closed mesh.
///
/// Edges are counted with DIRECTION, and an edge is a boundary when its two directions do not
/// balance. Counting "edges used by other than two triangles" is the obvious test and it is wrong
/// here: these pieces are unions of closed boxes, and where two boxes touch exactly, their shared
/// edge is used FOUR times while the surface is perfectly closed. Direction handles that by
/// construction — the two boxes traverse the shared edge opposite ways, so it cancels — while a
/// genuine hole still leaves an unmatched traversal behind.
pub fn closure(positions: &[[f32; 3]]) -> (usize, usize) {
    let tris = positions.len() / 3;
    // 0.1 mm — below anything a user can see, above f32 noise at building scale.
    let key = |p: [f32; 3]| {
        [
            (p[0] * 10_000.0).round() as i64,
            (p[1] * 10_000.0).round() as i64,
            (p[2] * 10_000.0).round() as i64,
        ]
    };
    let mut net: std::collections::HashMap<([i64; 3], [i64; 3]), i32> =
        std::collections::HashMap::with_capacity(tris * 3);
    for t in 0..tris {
        let v = [key(positions[t * 3]), key(positions[t * 3 + 1]), key(positions[t * 3 + 2])];
        for e in 0..3 {
            let (a, b) = (v[e], v[(e + 1) % 3]);
            if a == b {
                continue; // a degenerate edge is not an opening
            }
            // One slot per unordered pair, signed by which way this triangle walks it.
            let (k, step) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
            *net.entry(k).or_insert(0) += step;
        }
    }
    let unbalanced: Vec<([i64; 3], [i64; 3], i32)> =
        net.into_iter().filter(|(_, n)| *n != 0).map(|((a, b), n)| (a, b, n)).collect();
    (genuine_openings(&unbalanced), tris)
}

/// Of the edges whose two directions do not balance, how many bound a REAL hole.
///
/// Not all of them do, and this is the distinction that decides whether a piece can be cut. The
/// desk's top is tiled into rectangles around its cable ports, so a long tile meets two short ones
/// and its edge is matched by two half-edges rather than one whole one — a **T-junction**. The
/// surface is watertight; no light gets through it; a BSP classifies points against it perfectly
/// well, because a BSP works on planes and not on edge adjacency. Only the *bookkeeping* is
/// uneven, and rejecting the desk for that would be rejecting it for a technicality no user could
/// see or act on.
///
/// So unbalanced edges are gathered onto the LINE they lie on and their coverage is summed there.
/// A T-junction's long edge and its two halves cover the same interval opposite ways and cancel to
/// nothing. A genuine hole leaves interval left over, and that is what gets counted.
fn genuine_openings(unbalanced: &[([i64; 3], [i64; 3], i32)]) -> usize {
    // line key → the intervals laid down on it, as (start, end, signed count)
    let mut lines: std::collections::HashMap<([i64; 3], [i64; 3]), Vec<(f64, f64, i32)>> =
        std::collections::HashMap::new();
    for &(a, b, n) in unbalanced {
        let pa = glam::DVec3::new(a[0] as f64, a[1] as f64, a[2] as f64);
        let pb = glam::DVec3::new(b[0] as f64, b[1] as f64, b[2] as f64);
        let d = pb - pa;
        let len = d.length();
        if len < 1e-9 {
            continue;
        }
        let mut dir = d / len;
        // Canonical direction, so the same line reached from either end lands in one bucket.
        let flip = (dir.x, dir.y, dir.z) < (0.0, 0.0, 0.0);
        if flip {
            dir = -dir;
        }
        // Anchor: the point of the line nearest the origin — independent of which edge we came in
        // on, so collinear edges share a key.
        let anchor = pa - dir * pa.dot(dir);
        // 0.01 of the 0.1 mm grid: tight enough that two different lines never merge, loose enough
        // that the same line always does.
        let q = |v: f64| (v * 100.0).round() as i64;
        let key = ([q(anchor.x), q(anchor.y), q(anchor.z)], [q(dir.x * 1e4), q(dir.y * 1e4), q(dir.z * 1e4)]);
        let (ta, tb) = (pa.dot(dir), pb.dot(dir));
        // `n` counts a→b traversals; on the canonical direction that sign may invert.
        let sign = if (tb > ta) != flip { n } else { -n };
        let (lo, hi) = if ta <= tb { (ta, tb) } else { (tb, ta) };
        lines.entry(key).or_default().push((lo, hi, if ta <= tb { sign } else { -sign }));
    }

    let mut open = 0usize;
    for spans in lines.values() {
        // Sweep the breakpoints; any sub-interval with a non-zero running sum is unmatched.
        let mut marks: Vec<f64> = spans.iter().flat_map(|s| [s.0, s.1]).collect();
        marks.sort_by(|a, b| a.partial_cmp(b).unwrap());
        marks.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        for w in marks.windows(2) {
            let mid = (w[0] + w[1]) * 0.5;
            let sum: i32 = spans.iter().filter(|s| s.0 < mid && mid < s.1).map(|s| s.2).sum();
            if sum != 0 {
                open += 1;
            }
        }
    }
    open
}

/// Quantized vertex key — the weld these routines all share. 0.1 mm: below anything a user can
/// see, above f32 noise at building scale.
#[inline]
fn vkey(p: [f32; 3]) -> [i64; 3] {
    [
        (p[0] * 10_000.0).round() as i64,
        (p[1] * 10_000.0).round() as i64,
        (p[2] * 10_000.0).round() as i64,
    ]
}

/// Group triangles into connected components by welded vertex, returning a component index per
/// triangle and the component count.
fn components(positions: &[[f32; 3]]) -> (Vec<usize>, usize) {
    let tris = positions.len() / 3;
    let mut parent: Vec<usize> = (0..tris).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    // First triangle seen at each vertex; every later one unions with it.
    let mut at: std::collections::HashMap<[i64; 3], usize> = std::collections::HashMap::new();
    for t in 0..tris {
        for k in 0..3 {
            match at.entry(vkey(positions[t * 3 + k])) {
                std::collections::hash_map::Entry::Occupied(o) => {
                    let (a, b) = (find(&mut parent, t), find(&mut parent, *o.get()));
                    parent[a] = b;
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(t);
                }
            }
        }
    }
    let mut label: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let mut out = vec![0usize; tris];
    for t in 0..tris {
        let r = find(&mut parent, t);
        let n = label.len();
        out[t] = *label.entry(r).or_insert(n);
    }
    let n = label.len();
    (out, n)
}

/// Which triangles are flat DECALS rather than parts of a solid.
///
/// Generators use decals for detail that would be absurd to model as geometry: the cabin stamps a
/// shelf-pin dimple as an octagon standing 0.2 mm proud of its panel, ninety-six times. Each is an
/// open fan, so a whole-mesh closure test condemns the cabin at 768 open edges while every actual
/// panel in it is a perfectly closed box.
///
/// The test is per CONNECTED COMPONENT, and it has to be. Part id is too coarse — all ninety-six
/// dimples share one id, spread over six different panel faces. And volume by the divergence
/// theorem is only meaningful on a CLOSED surface: on an open fan it measures the cone from
/// wherever the origin happens to be, so a disc 18 mm off the origin reports a healthy volume and
/// passes for a solid. Measured about the component's OWN centroid, a flat fan's apex lies in its
/// own plane and the volume is zero — which is the property being tested for.
fn decal_triangles(positions: &[[f32; 3]]) -> Vec<bool> {
    let tris = positions.len() / 3;
    let (comp, n) = components(positions);
    let mut sum = vec![glam::DVec3::ZERO; n];
    let mut count = vec![0.0f64; n];
    let p = |i: usize| {
        let q = positions[i];
        glam::DVec3::new(q[0] as f64, q[1] as f64, q[2] as f64)
    };
    for t in 0..tris {
        for k in 0..3 {
            sum[comp[t]] += p(t * 3 + k);
            count[comp[t]] += 1.0;
        }
    }
    let centroid: Vec<glam::DVec3> =
        (0..n).map(|i| sum[i] / count[i].max(1.0)).collect();

    let mut vol = vec![0.0f64; n];
    let mut area = vec![0.0f64; n];
    let mut open = vec![false; n];
    for t in 0..tris {
        let c = comp[t];
        let (a, b, d) = (p(t * 3) - centroid[c], p(t * 3 + 1) - centroid[c], p(t * 3 + 2) - centroid[c]);
        vol[c] += a.dot(b.cross(d)) / 6.0;
        area[c] += (b - a).cross(d - a).length() / 2.0;
    }
    // A component that is already closed is a solid whatever its volume says.
    for c in 0..n {
        open[c] = true;
    }
    let mut tri_of: Vec<Vec<usize>> = vec![Vec::new(); n];
    for t in 0..tris {
        tri_of[comp[t]].push(t);
    }
    for (c, list) in tri_of.iter().enumerate() {
        let mut pos = Vec::with_capacity(list.len() * 3);
        for &t in list {
            pos.extend_from_slice(&positions[t * 3..t * 3 + 3]);
        }
        open[c] = closure(&pos).0 > 0;
    }

    let mut out = vec![false; tris];
    for t in 0..tris {
        let c = comp[t];
        // Flat AND open. A closed shell is never a decal; an open shell with real volume is a
        // BROKEN solid and must still be reported, not quietly waved through.
        out[t] = open[c] && vol[c].abs() < 1e-3 * area[c].max(1e-12).powf(1.5);
    }
    out
}

/// [`closure`], ignoring flat decals — the measurement [`apply`] actually refuses on.
pub fn closure_for_cutting(positions: &[[f32; 3]]) -> (usize, usize) {
    let decal = decal_triangles(positions);
    if !decal.iter().any(|&d| d) {
        return closure(positions);
    }
    let mut solid = Vec::with_capacity(positions.len());
    for t in 0..positions.len() / 3 {
        if decal[t] {
            continue;
        }
        solid.extend_from_slice(&positions[t * 3..t * 3 + 3]);
    }
    let (open, _) = closure(&solid);
    (open, positions.len() / 3)
}

/// True when every edge is shared by exactly two triangles.
///
/// This is a test for the INPUT, not for the output. A BSP boolean leaves T-junctions — where a
/// polygon was split along an edge its untouched neighbour was not — so a perfectly watertight
/// result still reports open edges here. Cutting an already-cut mesh is never needed anyway:
/// [`apply`] always replays the whole cut list from the original geometry.
pub fn is_closed(positions: &[[f32; 3]]) -> bool {
    closure(positions).0 == 0
}

/// Apply every enabled cut to a mesh, starting from the ORIGINAL geometry each time.
///
/// `part_ids` is one id per triangle (empty ⇒ all zero). The returned mesh's `face_ids` are those
/// same ids, plus one fresh id per cut for the surface that cut exposed.
///
/// Returns `Ok(None)` when there is nothing enabled to do, so the caller can keep using the
/// original mesh rather than a needless copy of it.
pub fn apply(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    part_ids: &[u32],
    cuts: &[MeshCut],
) -> Result<Option<SolidMesh>, CutError> {
    let tris = positions.len() / 3;
    if tris == 0 {
        return Err(CutError::EmptyMesh);
    }
    if !cuts.iter().any(|c| c.enabled) {
        return Ok(None);
    }
    // Refuse on evidence, before doing any work — see the module note on why an open shell must
    // not be cut rather than cut badly.
    let (open, _) = closure_for_cutting(positions);
    if open > 0 {
        return Err(CutError::NotClosed { open, tris });
    }

    let mut body = to_csg(positions, normals, part_ids);
    let mut next_part = part_ids.iter().copied().max().map(|m| m + 1).unwrap_or(1);
    let (lo, hi) = bounds(positions);

    for cut in cuts.iter().filter(|c| c.enabled) {
        let prism = prism(cut, lo, hi, next_part)?;
        next_part += 1;
        let after = body.difference(&prism);
        if after.polygons.is_empty() {
            return Err(CutError::EverythingRemoved);
        }
        body = after;
    }

    let out = from_csg(&body);
    if out.tri_count() == 0 {
        return Err(CutError::EverythingRemoved);
    }
    Ok(Some(out))
}

/// The cutting prism for one cut, in the mesh's local space.
fn prism(cut: &MeshCut, lo: [f32; 3], hi: [f32; 3], part: u32) -> Result<PartMesh, CutError> {
    let pts: Vec<[f64; 2]> = cut.profile.iter().map(|p| [p[0] as f64, p[1] as f64]).collect();
    if pts.len() < 3 || shoelace(&pts).abs() < 1e-9 {
        return Err(CutError::DegenerateProfile);
    }
    let n = cut.frame.normal();
    if n.length_squared() < 0.5 {
        return Err(CutError::DegenerateProfile);
    }

    // How deep. THROUGH means past the far side of the body whatever its shape, so measure the
    // body's own extent along the cut direction rather than trusting a fixed number.
    let depth = if cut.through {
        let mut far = 0.0f32;
        for i in 0..8 {
            let c = glam::Vec3::new(
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            );
            far = far.max((cut.frame.origin - c).dot(n));
        }
        (far as f64).max(0.0) + 4.0 * OVERCUT
    } else {
        (cut.depth.max(1e-4) as f64) + OVERCUT
    };

    // csgrs extrudes a sketch from z = 0 to z = h along +Z. Sink it so the solid spans
    // z ∈ [−depth, +OVERCUT], then map sketch (x, y, z) → local (u, v, n): the prism then starts
    // just OUTSIDE the drawn face and runs inward, which is the direction a cut goes.
    let solid = Sketch::polygon(&pts, part).extrude(depth + OVERCUT);
    let sink = Matrix4::new_translation(&Vector3::new(0.0, 0.0, -depth));
    let (u, v) = (cut.frame.u, cut.frame.v);
    let o = cut.frame.origin;
    // (u, v, n) is right-handed by construction (`Frame::from_point_normal`), so this transform
    // does not mirror — a mirrored cutter would invert the winding and subtract inside-out.
    #[rustfmt::skip]
    let place = Matrix4::new(
        u.x as f64, v.x as f64, n.x as f64, o.x as f64,
        u.y as f64, v.y as f64, n.y as f64, o.y as f64,
        u.z as f64, v.z as f64, n.z as f64, o.z as f64,
        0.0,        0.0,        0.0,        1.0,
    );
    Ok(Mesh::from(solid).transform(&(place * sink)))
}

/// Twice the signed area of a closed loop — zero for a degenerate or self-cancelling profile.
fn shoelace(pts: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for i in 0..pts.len() {
        let p = pts[i];
        let q = pts[(i + 1) % pts.len()];
        a += p[0] * q[1] - q[0] * p[1];
    }
    a
}

fn bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    (lo, hi)
}

/// Triangle soup → csgrs, one polygon per triangle, tagged with its part id.
fn to_csg(positions: &[[f32; 3]], normals: &[[f32; 3]], part_ids: &[u32]) -> PartMesh {
    let tris = positions.len() / 3;
    let mut polys = Vec::with_capacity(tris);
    for t in 0..tris {
        let mut vs = Vec::with_capacity(3);
        for k in 0..3 {
            let p = positions[t * 3 + k];
            let n = normals.get(t * 3 + k).copied().unwrap_or([0.0, 0.0, 1.0]);
            vs.push(Vertex::new(
                Point3::new(p[0] as f64, p[1] as f64, p[2] as f64),
                Vector3::new(n[0] as f64, n[1] as f64, n[2] as f64),
            ));
        }
        // `Polygon::new` ASSERTS three vertices and a real plane; a zero-area triangle (common in
        // generated geometry, harmless in a render buffer) would take the app down.
        if degenerate(&vs) {
            continue;
        }
        polys.push(Polygon::new(vs, part_ids.get(t).copied().unwrap_or(0)));
    }
    Mesh::from_polygons(&polys, 0)
}

fn degenerate(vs: &[Vertex]) -> bool {
    let (a, b, c) = (vs[0].position, vs[1].position, vs[2].position);
    (b - a).cross(&(c - a)).norm() < 1e-12
}

/// csgrs → triangle soup, keeping each polygon's part id on every triangle it fans into.
fn from_csg(m: &PartMesh) -> SolidMesh {
    let mut out = SolidMesh::default();
    for poly in &m.polygons {
        for tri in poly.triangulate() {
            for v in tri {
                let p = v.position.coords;
                let n = v.normal;
                out.positions.push([p.x as f32, p.y as f32, p.z as f32]);
                out.normals.push([n.x as f32, n.y as f32, n.z as f32]);
            }
            out.face_ids.push(poly.metadata);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec2, Vec3};

    /// An axis-aligned box as closed triangle soup, tagged `part`.
    fn boxsoup(lo: [f32; 3], hi: [f32; 3], part: u32) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>) {
        let c = [
            [lo[0], lo[1], lo[2]], [hi[0], lo[1], lo[2]], [hi[0], hi[1], lo[2]], [lo[0], hi[1], lo[2]],
            [lo[0], lo[1], hi[2]], [hi[0], lo[1], hi[2]], [hi[0], hi[1], hi[2]], [lo[0], hi[1], hi[2]],
        ];
        let quads: [([usize; 4], [f32; 3]); 6] = [
            ([0, 3, 2, 1], [0.0, 0.0, -1.0]), ([4, 5, 6, 7], [0.0, 0.0, 1.0]),
            ([0, 1, 5, 4], [0.0, -1.0, 0.0]), ([3, 7, 6, 2], [0.0, 1.0, 0.0]),
            ([0, 4, 7, 3], [-1.0, 0.0, 0.0]), ([1, 2, 6, 5], [1.0, 0.0, 0.0]),
        ];
        let (mut p, mut n, mut ids) = (Vec::new(), Vec::new(), Vec::new());
        for (q, nn) in quads {
            for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                for &vi in &tri {
                    p.push(c[vi]);
                    n.push(nn);
                }
                ids.push(part);
            }
        }
        (p, n, ids)
    }

    fn square(half: f32) -> Vec<[f32; 2]> {
        vec![[-half, -half], [half, -half], [half, half], [-half, half]]
    }

    fn volume(m: &SolidMesh) -> f32 {
        // Signed volume by the divergence theorem — the honest way to ask "did material leave?".
        let mut v = 0.0;
        for t in 0..m.tri_count() {
            let a = Vec3::from(m.positions[t * 3]);
            let b = Vec3::from(m.positions[t * 3 + 1]);
            let c = Vec3::from(m.positions[t * 3 + 2]);
            v += a.dot(b.cross(c)) / 6.0;
        }
        v.abs()
    }

    /// The measurement the refusal rests on: a real solid reads closed, and one triangle removed
    /// from it does not.
    #[test]
    fn closure_tells_a_solid_from_a_shell() {
        let (p, _, _) = boxsoup([0.0; 3], [1.0; 3], 1);
        assert_eq!(closure(&p), (0, 12), "a box is closed");
        assert!(is_closed(&p));

        // Drop one triangle: its three edges are now used once each.
        let open: Vec<[f32; 3]> = p[3..].to_vec();
        let (n_open, tris) = closure(&open);
        assert_eq!(tris, 11);
        assert_eq!(n_open, 3, "a missing triangle leaves exactly its three edges open");
        assert!(!is_closed(&open));
    }

    /// A through-cut must punch a real hole: volume falls by the prism's share and the mesh comes
    /// back CLOSED, which is what proves the cut was capped rather than just opened.
    #[test]
    fn a_through_cut_removes_a_column_and_leaves_a_solid() {
        let (p, n, ids) = boxsoup([-0.5, -0.5, 0.0], [0.5, 0.5, 0.2], 3);
        let before = volume(&SolidMesh {
            positions: p.clone(), normals: n.clone(), face_ids: ids.clone(),
        });
        assert!((before - 0.2).abs() < 1e-4, "1 × 1 × 0.2 box, got {before}");

        // Draw on the top face (normal +Z) and cut down through the slab.
        let frame = Frame::from_point_normal(Vec3::new(0.0, 0.0, 0.2), Vec3::Z);
        let cut = MeshCut::through(frame, square(0.1), "hole");
        let out = apply(&p, &n, &ids, &[cut]).unwrap().expect("a cut was applied");

        let after = volume(&out);
        let want = 0.2 - 0.2 * 0.2 * 0.2; // slab minus a 0.2 × 0.2 column
        assert!((after - want).abs() < 2e-3, "volume {after}, want {want}");

        // The hole goes all the way THROUGH: the surface the cut exposed spans the full 0.2 m
        // thickness. Volume alone cannot tell a through-hole from a deep pocket.
        let (lo, hi) = cut_face_z(&out, 3);
        assert!(lo < 1e-3 && hi > 0.2 - 1e-3, "the cut surface spans the slab ({lo}..{hi})");
    }

    /// The z range of the triangles that are NOT part of the original body — the surface the cut
    /// exposed.
    fn cut_face_z(m: &SolidMesh, body_part: u32) -> (f32, f32) {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for t in 0..m.tri_count() {
            if m.face_ids[t] == body_part {
                continue;
            }
            for k in 0..3 {
                lo = lo.min(m.positions[t * 3 + k][2]);
                hi = hi.max(m.positions[t * 3 + k][2]);
            }
        }
        (lo, hi)
    }

    /// A blind pocket removes less than a through-cut, and leaves a floor.
    #[test]
    fn a_pocket_stops_at_its_depth() {
        let (p, n, ids) = boxsoup([-0.5, -0.5, 0.0], [0.5, 0.5, 0.2], 3);
        let frame = Frame::from_point_normal(Vec3::new(0.0, 0.0, 0.2), Vec3::Z);
        let shallow = apply(&p, &n, &ids, &[MeshCut::pocket(frame, square(0.1), 0.05, "pocket")])
            .unwrap().unwrap();
        let deep = apply(&p, &n, &ids, &[MeshCut::through(frame, square(0.1), "hole")])
            .unwrap().unwrap();
        assert!(volume(&shallow) > volume(&deep), "a pocket keeps more material than a hole");
        let want = 0.2 - 0.2 * 0.2 * 0.05;
        assert!((volume(&shallow) - want).abs() < 2e-3, "pocket volume {}", volume(&shallow));
        // THE difference from a through-cut: the pocket has a FLOOR. Its exposed surface stops
        // 0.05 m down from the top face and never reaches the base.
        let (lo, hi) = cut_face_z(&shallow, 3);
        assert!(hi > 0.2 - 1e-3, "the pocket is open at the top ({hi})");
        assert!(lo > 0.15 - 2e-3, "…and floored 50 mm down, not open to the base ({lo})");
    }

    /// Part ids survive the boolean, and the cut's own surface arrives as a NEW part — so a door
    /// keeps its per-component materials and the inside of the hole can be painted separately.
    #[test]
    fn part_ids_survive_and_the_cut_face_is_its_own_part() {
        let (p, n, ids) = boxsoup([-0.5, -0.5, 0.0], [0.5, 0.5, 0.2], 7);
        let frame = Frame::from_point_normal(Vec3::new(0.0, 0.0, 0.2), Vec3::Z);
        let out = apply(&p, &n, &ids, &[MeshCut::through(frame, square(0.1), "hole")])
            .unwrap().unwrap();
        let parts: std::collections::HashSet<u32> = out.face_ids.iter().copied().collect();
        assert!(parts.contains(&7), "the body kept its part id");
        assert_eq!(parts.len(), 2, "body + cut surface, got {parts:?}");
        let cut_part = *parts.iter().find(|&&x| x != 7).unwrap();
        assert!(cut_part > 7, "the cut's part id continues past the body's");
        assert_eq!(out.face_ids.len(), out.tri_count(), "one id per triangle");
    }

    /// An OPEN mesh is refused, with the count, and left alone. This is the whole of the
    /// "only generated pieces" promise.
    #[test]
    fn an_open_shell_is_refused_with_its_measurement() {
        let (p, n, ids) = boxsoup([0.0; 3], [1.0; 3], 1);
        let (p, n, ids) = (p[3..].to_vec(), n[3..].to_vec(), ids[1..].to_vec());
        let frame = Frame::from_point_normal(Vec3::new(0.5, 0.5, 1.0), Vec3::Z);
        let err = apply(&p, &n, &ids, &[MeshCut::through(frame, square(0.2), "x")]).unwrap_err();
        assert_eq!(err, CutError::NotClosed { open: 3, tris: 11 });
        assert!(err.to_string().contains("3 open edges"), "says how open: {err}");
        assert!(err.to_string().contains("imported"), "says which meshes can be cut: {err}");
    }

    /// Disabling a cut must restore the mesh exactly — the point of an editable list. `None` means
    /// "use the original", so nothing is copied when nothing is enabled.
    #[test]
    fn a_disabled_cut_leaves_the_mesh_untouched() {
        let (p, n, ids) = boxsoup([-0.5, -0.5, 0.0], [0.5, 0.5, 0.2], 3);
        let frame = Frame::from_point_normal(Vec3::new(0.0, 0.0, 0.2), Vec3::Z);
        let mut cut = MeshCut::through(frame, square(0.1), "hole");
        assert!(apply(&p, &n, &ids, &[cut.clone()]).unwrap().is_some(), "enabled cuts");
        cut.enabled = false;
        assert!(apply(&p, &n, &ids, &[cut]).unwrap().is_none(), "disabled does nothing at all");
        assert!(apply(&p, &n, &ids, &[]).unwrap().is_none(), "and an empty list does nothing");
    }

    /// Two cuts compose, and the order they are listed in does not change the result — a
    /// difference is commutative, and a user reordering the list must not see the mesh change.
    #[test]
    fn cuts_compose_and_do_not_depend_on_their_order() {
        let (p, n, ids) = boxsoup([-0.5, -0.5, 0.0], [0.5, 0.5, 0.2], 3);
        let top = Frame::from_point_normal(Vec3::new(-0.25, 0.0, 0.2), Vec3::Z);
        let side = Frame::from_point_normal(Vec3::new(0.5, 0.0, 0.1), Vec3::X);
        let a = MeshCut::through(top, square(0.08), "a");
        let b = MeshCut::pocket(side, square(0.05), 0.3, "b");

        let one = apply(&p, &n, &ids, &[a.clone(), b.clone()]).unwrap().unwrap();
        let two = apply(&p, &n, &ids, &[b, a]).unwrap().unwrap();
        assert!((volume(&one) - volume(&two)).abs() < 1e-4, "order does not matter");
        // …and both removed more than either alone.
        let solo = apply(&p, &n, &ids, &[MeshCut::through(top, square(0.08), "a")])
            .unwrap().unwrap();
        assert!(volume(&one) < volume(&solo), "the second cut removed more");
    }

    /// A cut aimed away from the body must not silently mangle it.
    #[test]
    fn a_profile_with_no_area_is_refused() {
        let (p, n, ids) = boxsoup([0.0; 3], [1.0; 3], 1);
        let frame = Frame::from_point_normal(Vec3::new(0.5, 0.5, 1.0), Vec3::Z);
        for bad in [vec![], vec![[0.0, 0.0]], vec![[0.0, 0.0], [0.1, 0.0]],
                    vec![[0.0, 0.0], [0.1, 0.0], [0.2, 0.0]]] {
            let err = apply(&p, &n, &ids, &[MeshCut::through(frame, bad, "x")]).unwrap_err();
            assert_eq!(err, CutError::DegenerateProfile);
        }
    }

    /// The cut goes INTO the body, never out of the face it was drawn on. Get this sign wrong and
    /// the prism sits in mid-air, the volume is unchanged, and nothing looks broken until someone
    /// checks — so check.
    #[test]
    fn the_cut_goes_inward_from_the_drawn_face() {
        let (p, n, ids) = boxsoup([-0.5, -0.5, 0.0], [0.5, 0.5, 0.2], 3);
        let before = 0.2f32;
        // Every face of the box, cut with the same small pocket.
        for (o, nrm) in [
            (Vec3::new(0.0, 0.0, 0.2), Vec3::Z),
            (Vec3::new(0.0, 0.0, 0.0), -Vec3::Z),
            (Vec3::new(0.5, 0.0, 0.1), Vec3::X),
            (Vec3::new(-0.5, 0.0, 0.1), -Vec3::X),
            (Vec3::new(0.0, 0.5, 0.1), Vec3::Y),
            (Vec3::new(0.0, -0.5, 0.1), -Vec3::Y),
        ] {
            let f = Frame::from_point_normal(o, nrm);
            let out = apply(&p, &n, &ids, &[MeshCut::pocket(f, square(0.04), 0.03, "p")])
                .unwrap().unwrap();
            let v = volume(&out);
            assert!(v < before - 1e-5, "cutting at {o:?}/{nrm:?} removed material ({v} < {before})");
            assert!(v > before * 0.9, "…and only a little of it ({v})");
        }
    }

    /// THE promise: "generated pieces can be cut". Every generator in this crate must produce
    /// geometry that passes the precondition, or the feature quietly does not exist for that piece.
    ///
    /// They are built as overlapping BOXES rather than one welded manifold, which is fine — each
    /// box is closed on its own, so every edge still has exactly two triangles, and a BSP is happy
    /// with a union of closed shells. This test is what stops a future generator shipping an open
    /// shell (a stray triangle fan, a missing cap) and taking cutting away from that piece.
    #[test]
    fn every_generated_piece_is_cuttable() {
        let door = crate::door::build(&crate::door::DoorInput::default()).unwrap().1;
        let cupboard = crate::cupboard::build(&crate::cupboard::CupboardInput::default()).unwrap().1;
        let kitchen = crate::kitchen::build(&crate::kitchen::KitchenInput::default()).unwrap().1;
        let cabin = crate::cabin::build(&crate::cabin::CabinInput::default()).unwrap().1;
        let desk = crate::desk::build(&crate::desk::DeskInput::default()).unwrap().1;
        let couch = crate::couch::build(&crate::couch::CouchInput::default()).unwrap().1;
        let stair = crate::architecture::build_stairs(&crate::architecture::StairParams::default()).unwrap();
        let spiral = crate::architecture::build_spiral(&crate::architecture::SpiralParams::default()).unwrap();
        let ramp = crate::architecture::build_ramp(&crate::architecture::RampParams::default()).unwrap();

        for (name, m) in [
            ("door", &door), ("cupboard", &cupboard), ("kitchen", &kitchen), ("cabin", &cabin),
            ("desk", &desk), ("couch", &couch), ("stair", &stair), ("spiral", &spiral),
            ("ramp", &ramp),
        ] {
            let (open, tris) = closure_for_cutting(&m.positions);
            assert_eq!(open, 0, "{name}: {open} open edges in {tris} triangles — not cuttable");
        }
    }

    /// The cabin is the reason [`decal_triangles`] exists: 96 shelf-pin dimples, each an open
    /// octagon fan, on a carcass of perfectly closed panels. Whole-mesh closure condemns it; its
    /// solids alone are clean. If a future generator makes its PANELS open, this still catches it.
    #[test]
    fn flat_decals_do_not_condemn_a_solid_piece() {
        let cabin = crate::cabin::build(&crate::cabin::CabinInput::default()).unwrap().1;
        let (raw, _) = closure(&cabin.positions);
        assert!(raw > 0, "the raw mesh really does have open edges ({raw})");
        assert_eq!(closure_for_cutting(&cabin.positions).0, 0, "its solids do not");
        let decal = decal_triangles(&cabin.positions);
        let n = decal.iter().filter(|&&d| d).count();
        assert!(n > 0, "the dimples were identified as decals");
        // 96 dimples × 8 triangles = 768 of the cabin's 864; the carcass is the small remainder.
        assert!(n < decal.len(), "…and not the whole mesh ({n} of {})", decal.len());

        // A solid box must never be mistaken for a decal — that would exempt a genuine hole.
        let (p, _, _) = boxsoup([0.0; 3], [1.0; 3], 4);
        assert!(!decal_triangles(&p).iter().any(|&d| d), "a box is not a decal");

        // …and a genuinely BROKEN solid — open, but enclosing real volume — is still condemned.
        let broken: Vec<[f32; 3]> = p[3..].to_vec();
        assert!(!decal_triangles(&broken).iter().any(|&d| d), "a box with a hole is not a decal");
        assert!(closure_for_cutting(&broken).0 > 0, "and is still refused");
    }

    /// …and a real door really does cut, keeping its component parts. The unit box above proves the
    /// maths; this proves it survives contact with the geometry the app actually makes.
    #[test]
    fn a_real_door_takes_a_letterbox_slot() {
        let inp = crate::door::DoorInput::default();
        let (_m, mesh) = crate::door::build(&inp).unwrap();
        let before = volume(&mesh);

        // A letterbox through the leaf. The leaf spans y ∈ [−thickness, 0], so its FRONT face is
        // the one at y = 0 and its outward normal is +Y — which is the normal a face-pick would
        // hand back. Get that sign wrong and the prism sits in the air in front of the door,
        // removes nothing, and looks like a boolean failure rather than an aiming error.
        let frame = Frame::from_point_normal(Vec3::new(0.0, 0.0, 0.9), Vec3::Y);
        let slot = vec![[-0.13, -0.04], [0.13, -0.04], [0.13, 0.04], [-0.13, 0.04]];
        let out = apply(&mesh.positions, &mesh.normals, &mesh.face_ids, &[MeshCut::through(frame, slot, "letterbox")])
            .unwrap()
            .expect("the door was cut");

        assert!(volume(&out) < before, "material came out ({} < {before})", volume(&out));
        let parts: std::collections::HashSet<u32> = out.face_ids.iter().copied().collect();
        for keep in [crate::door::Part::Leaf, crate::door::Part::Lining, crate::door::Part::ArchFront] {
            assert!(parts.contains(&(keep as u32)), "{keep:?} survived the cut with its id");
        }
        assert!(
            parts.iter().any(|&p| p > crate::door::Part::Handle as u32),
            "the slot's own surface is a new part: {parts:?}"
        );
    }

    /// The frame's `to_uv`/`from_uv` round-trip is what the app relies on to turn a drawn point
    /// into a profile coordinate. If it did not, cuts would land offset from where they were drawn.
    #[test]
    fn a_point_drawn_on_a_face_maps_back_to_itself() {
        let f = Frame::from_point_normal(Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.3, -0.5, 0.8));
        for w in [Vec3::new(1.2, 2.1, 3.05), Vec3::new(0.4, 1.7, 3.4)] {
            // Project onto the plane first — `to_uv` drops the normal component by construction.
            let n = f.normal();
            let on_plane = w - n * (w - f.origin).dot(n);
            let uv: Vec2 = f.to_uv(on_plane);
            let back = f.from_uv(uv);
            assert!((back - on_plane).length() < 1e-5, "{on_plane:?} → {uv:?} → {back:?}");
        }
    }
}
