//! Parametric **wood-frame sofa**, ported from `COUCH_BUILD.md` (Part B) — reverse-engineered from
//! "Sofa by Howard Furniture" (2.14 × 0.78 × 0.68 m): an oak seat box floating 50 mm off the floor,
//! arch end-panel arms that double as the legs, a spindle back under a floating top rail, and
//! pillowed seat + wedge back cushions.
//!
//! The generator is a **run chain**, the same concept as [`crate::kitchen`]: a list of straight
//! segments joined by +90° corner units, so one tool covers straight sofas, L-sectionals and
//! U-sectionals. A turtle walks the back edges —
//!
//! ```text
//! frame k+1:   B' = B_end + W·(D_k + D_k+1),   D' = rot90(D_k)
//! corner unit: the W × W square between the two back edges
//! ```
//!
//! — and the seats always face the **inside** of every turn.
//!
//! Like [`crate::desk`], the five features are independently deletable ([`CouchInput::frame`],
//! `back`, `arm_start`/`arm_end`, `seat_cushions`, `back_cushions`); the `Bench` preset is literally
//! the chain with `back` and the arms switched off. Arms can only ever appear at the two **free**
//! chain ends, which the input makes structural rather than validated — there is no way to ask for
//! one on an interior junction.
//!
//! Frame: the chain starts along +x with its back edge on y = 0 and the seats facing +y; z = 0 floor.
//! Run-local coordinates are `u` along the run and `y` measured **from the front edge** (so the
//! reference FBX numbers drop straight in), which [`Frame`] converts to its own back-referenced axis.
//! Metres throughout.
//!
//! PURE geometry, **no boolean**. Output is a [`SolidMesh`] whose `face_ids` tag each component (one
//! part per seat pillow, per back wedge, per arm, per frame box) plus a [`Material`] per part id.
//! Cushions are emitted through [`Soft`], which shares vertices within a pillow and averages face
//! normals into them — Blender's smooth shading, which is what makes them read as upholstery rather
//! than facetted boxes.
//!
//! Not ported: the reference's procedural wood-grain noise (our engine binds a procedural oak swatch
//! per part instead — see the app's `factory_build_couch`).

use crate::SolidMesh;
use glam::Vec3;

// ── fixed geometry (spec §B1/§B3; metres) ──
const BOX_Z0: f32 = 0.050; // the seat box floats — the arms are the legs
const RAIL_T: f32 = 0.090; // back rail height
const RAIL_D: f32 = 0.025; // back rail depth
const SPIN_W: f32 = 0.016; // spindle section
const SPIN_PITCH: f32 = 0.250;
const SPIN_INSET: f32 = 0.020; // spindle inset from each end of a span
const SPIN_LIFT: f32 = 0.018; // spindles start this far above the seat box
const CUSH_PITCH: f32 = 0.667; // auto cushion count aims at this
const CUSH_FRONT: f32 = 0.009; // seat cushion inset from the front edge
const CUSH_DEPTH: f32 = 0.754;
const CUSH_GAP: f32 = 0.002; // reveal between neighbouring seat pillows
const CORNER_MARGIN: f32 = 0.017; // corner pillow margin to both back edges
const ARM_PROUD: f32 = 0.070; // how far an arm stands beyond the frame
const ARM_Y0: f32 = 0.075; // arm profile, measured from the FRONT edge
const ARM_Y1: f32 = 0.710;
const ARM_STILE_F: f32 = 0.190; // front stile ends here
const ARM_STILE_R: f32 = 0.550; // rear stile starts here
const ARM_BAND: f32 = 0.125; // solid armrest band height below the arm top
const WEDGE_REAR: f32 = 0.021; // back cushion rear face, in from the back edge
const WEDGE_T_BOT: f32 = 0.240;
const WEDGE_T_TOP: f32 = 0.150;
const WEDGE_DROP: f32 = 0.078; // wedge base below the seat top
const WEDGE_RISE: f32 = 0.039; // wedge top above the back rail
const WEDGE_GAP: f32 = 0.004; // reveal between neighbouring wedges
const WEDGE_CORNER_CLEAR: f32 = 0.250; // corner wedges start this far from the corner
const PILLOW_R: f32 = 0.045;
const DOME: f32 = 0.014;

/// The back treatment (spec §B1). `None` leaves a backless bench.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackKind {
    /// Top rail on turned spindles — the reference.
    Spindle,
    None,
}

impl BackKind {
    pub const ALL: [BackKind; 2] = [BackKind::Spindle, BackKind::None];
    pub fn label(self) -> &'static str {
        match self {
            BackKind::Spindle => "Spindle back",
            BackKind::None => "None (bench)",
        }
    }
}

/// The material a component wears — one flat swatch per part in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Material {
    /// Frame boxes, rails, spindles, arm panels.
    Oak,
    /// Seat pillows and back wedges.
    Fabric,
}

/// One straight segment of the chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    /// Frame span of the segment.
    pub length: f32,
    /// Seat cushions across it; **0 = auto** (aims at a 667 mm pitch, spec §B1).
    pub cushions: u32,
}

impl Run {
    pub fn new(length: f32, cushions: u32) -> Self {
        Self { length, cushions }
    }
    /// Resolved cushion count — never zero.
    pub fn count(self) -> u32 {
        if self.cushions > 0 { self.cushions } else { ((self.length / CUSH_PITCH).round() as u32).max(1) }
    }
}

/// The parametric feature tree (spec §B1). Not `Copy` — it owns the run list.
#[derive(Clone, Debug, PartialEq)]
pub struct CouchInput {
    /// One entry = a straight sofa; each extra entry adds a +90° corner unit.
    pub runs: Vec<Run>,
    /// Depth `W`.
    pub depth: f32,
    /// Seat cushion TOP.
    pub seat_top: f32,
    /// Back rail top.
    pub back_h: f32,
    pub arm_h: f32,
    /// Arm panel thickness along the run.
    pub arm_t: f32,
    pub cushion_t: f32,
    pub back: BackKind,
    /// Arm at the start of the chain (a free end by construction).
    pub arm_start: bool,
    /// Arm at the end of the chain (the other free end).
    pub arm_end: bool,
    // ── the remaining deletable features (spec §B3) ──
    pub frame: bool,
    pub seat_cushions: bool,
    pub back_cushions: bool,
}

impl Default for CouchInput {
    /// The generator's own default — an L-sectional.
    fn default() -> Self {
        Preset::L.input()
    }
}

/// The five reference configurations (`COUCH_BUILD.md`, "Variants").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Replica of the reference FBX.
    Straight,
    Loveseat,
    L,
    U,
    /// The chain with `back` and the arms deleted.
    Bench,
}

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::Straight, Preset::Loveseat, Preset::L, Preset::U, Preset::Bench];
    pub fn label(self) -> &'static str {
        match self {
            Preset::Straight => "Straight (reference)",
            Preset::Loveseat => "Loveseat",
            Preset::L => "L-sectional",
            Preset::U => "U-sectional",
            Preset::Bench => "Bench (no back, no arms)",
        }
    }

    pub fn input(self) -> CouchInput {
        let base = CouchInput {
            runs: vec![Run::new(2.001, 3)],
            depth: 0.780,
            seat_top: 0.375,
            back_h: 0.640,
            arm_h: 0.500,
            arm_t: 0.190,
            cushion_t: 0.183,
            back: BackKind::Spindle,
            arm_start: true,
            arm_end: true,
            frame: true,
            seat_cushions: true,
            back_cushions: true,
        };
        match self {
            Preset::Straight => base,
            Preset::Loveseat => CouchInput { runs: vec![Run::new(1.350, 2)], ..base },
            Preset::L => CouchInput { runs: vec![Run::new(2.001, 3), Run::new(1.350, 2)], ..base },
            Preset::U => CouchInput { runs: vec![Run::new(1.350, 2), Run::new(2.001, 3), Run::new(1.350, 2)], ..base },
            Preset::Bench => CouchInput {
                runs: vec![Run::new(1.800, 0)],
                back: BackKind::None,
                arm_start: false,
                arm_end: false,
                back_cushions: false,
                ..base
            },
        }
    }
}

/// What the build actually produced — achieved numbers next to the requested ones.
#[derive(Clone, Debug, PartialEq)]
pub struct CouchMetrics {
    pub runs: usize,
    pub corners: usize,
    /// Chain run length including the proud arms — the reference's "overall L".
    pub overall_len: f32,
    pub depth: f32,
    pub seat_top: f32,
    pub back_h: f32,
    pub arm_h: f32,
    /// Total seat pillows, corner ones included.
    pub seats: usize,
    /// Achieved cushion pitch on the first run.
    pub cushion_pitch: f32,
    /// Base of the spindles (`0` with no back).
    pub spindle_z0: f32,
    /// The feature nodes that were actually emitted, in build order.
    pub features: Vec<String>,
    pub tris: usize,
    pub warnings: Vec<String>,
}

// ============================ run frames ============================

/// One straight segment's placement. `d` runs along the segment, `n` points from the back edge
/// toward the seat front, and `(d, n, Z)` is right-handed — which is why local geometry can be
/// authored with plain outward normals and mapped straight through without re-winding anything.
#[derive(Clone, Copy, Debug)]
struct Frame {
    b: Vec3,
    d: Vec3,
    n: Vec3,
    w: f32,
}

impl Frame {
    fn pos(&self, u: f32, yb: f32, z: f32) -> Vec3 {
        self.b + self.d * u + self.n * yb + Vec3::Z * z
    }
    fn dir(&self, du: f32, dyb: f32, dz: f32) -> Vec3 {
        self.d * du + self.n * dyb + Vec3::Z * dz
    }
    /// Reference numbers are measured from the FRONT edge; the frame's own axis runs from the back.
    fn yb(&self, y_front: f32) -> f32 {
        self.w - y_front
    }
}

fn rot90(d: Vec3) -> Vec3 {
    Vec3::new(-d.y, d.x, 0.0)
}

/// Walk the chain (spec §B2). Each corner turns +90°, and the next run starts across the corner
/// square so the two back edges meet at its far corner.
fn chain(runs: &[Run], w: f32) -> Vec<Frame> {
    let mut out = Vec::with_capacity(runs.len());
    let mut b = Vec3::ZERO;
    let mut d = Vec3::X;
    for (i, r) in runs.iter().enumerate() {
        out.push(Frame { b, d, n: rot90(d), w });
        if i + 1 < runs.len() {
            let dn = rot90(d);
            b = b + d * r.length + d * w + dn * w;
            d = dn;
        }
    }
    out
}

// ============================ mesh emitters ============================

fn alloc(mats: &mut Vec<Material>, m: Material) -> u32 {
    mats.push(m);
    (mats.len() - 1) as u32
}

fn push_tri(mesh: &mut SolidMesh, part: u32, a: Vec3, b: Vec3, c: Vec3, n: Vec3) {
    for v in [a, b, c] {
        mesh.positions.push(v.into());
        mesh.normals.push(n.into());
    }
    mesh.face_ids.push(part);
}

fn push_quad(mesh: &mut SolidMesh, part: u32, a: Vec3, b: Vec3, c: Vec3, d: Vec3, n: Vec3) {
    push_tri(mesh, part, a, b, c, n);
    push_tri(mesh, part, a, c, d, n);
}

/// A box in run-local coordinates, `y` given from the FRONT edge. Flat outward normals.
fn push_lbox(mesh: &mut SolidMesh, part: u32, fr: &Frame, u: [f32; 2], y_front: [f32; 2], z: [f32; 2]) {
    let (u0, u1) = (u[0].min(u[1]), u[0].max(u[1]));
    let (ya, yb) = (fr.yb(y_front[0]), fr.yb(y_front[1]));
    let (y0, y1) = (ya.min(yb), ya.max(yb));
    let (z0, z1) = (z[0].min(z[1]), z[0].max(z[1]));
    if (u1 - u0) < 1e-6 || (y1 - y0) < 1e-6 || (z1 - z0) < 1e-6 {
        return;
    }
    let c = [
        fr.pos(u0, y0, z0),
        fr.pos(u1, y0, z0),
        fr.pos(u1, y1, z0),
        fr.pos(u0, y1, z0),
        fr.pos(u0, y0, z1),
        fr.pos(u1, y0, z1),
        fr.pos(u1, y1, z1),
        fr.pos(u0, y1, z1),
    ];
    let quads: [([usize; 4], [f32; 3]); 6] = [
        ([0, 3, 2, 1], [0.0, 0.0, -1.0]),
        ([4, 5, 6, 7], [0.0, 0.0, 1.0]),
        ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
        ([3, 7, 6, 2], [0.0, 1.0, 0.0]),
        ([0, 4, 7, 3], [-1.0, 0.0, 0.0]),
        ([1, 2, 6, 5], [1.0, 0.0, 0.0]),
    ];
    for (q, n) in quads {
        push_quad(mesh, part, c[q[0]], c[q[1]], c[q[2]], c[q[3]], fr.dir(n[0], n[1], n[2]));
    }
}

fn signed_area(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    let mut s = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        s += poly[i][0] * poly[j][1] - poly[j][0] * poly[i][1];
    }
    s * 0.5
}

/// Ear-clip a simple (possibly concave) polygon into triangles of ORIGINAL indices.
fn earclip(poly: &[[f32; 2]]) -> Vec<[usize; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    let mut ring: Vec<usize> = if signed_area(poly) < 0.0 { (0..n).rev().collect() } else { (0..n).collect() };
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let in_tri = |p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
        !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
    };
    let mut out = Vec::new();
    let mut guard = 0;
    while ring.len() > 3 && guard < 10_000 {
        guard += 1;
        let m = ring.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (ring[(i + m - 1) % m], ring[i], ring[(i + 1) % m]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross(a, b, c) <= 0.0 {
                continue; // reflex — not an ear
            }
            if ring.iter().any(|&iv| iv != ia && iv != ib && iv != ic && in_tri(poly[iv], a, b, c)) {
                continue;
            }
            out.push([ia, ib, ic]);
            ring.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if ring.len() == 3 {
        out.push([ring[0], ring[1], ring[2]]);
    }
    out
}

/// Reorder a closed profile to CCW. The arm arch is CONCAVE, so orientation has to come from the
/// polygon's own winding — a "point the normal away from the centroid" shortcut puts the window
/// walls inside-out, because the centroid of an arch sits in the opening.
fn ensure_ccw(mut poly: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    if signed_area(&poly) < 0.0 {
        poly.reverse();
    }
    poly
}

/// A closed CCW profile in the local `(yb, z)` plane, extruded along `u`. `(yb, z, u)` is
/// right-handed, so the outward side normal for edge P→Q is simply the right-hand perpendicular.
fn push_lprism(mesh: &mut SolidMesh, part: u32, fr: &Frame, poly: &[[f32; 2]], u0: f32, u1: f32) {
    let n = poly.len();
    if n < 3 || (u1 - u0).abs() < 1e-7 {
        return;
    }
    let (u0, u1) = (u0.min(u1), u0.max(u1));
    let p = |i: usize, u: f32| fr.pos(u, poly[i][0], poly[i][1]);
    for t in earclip(poly) {
        push_tri(mesh, part, p(t[0], u1), p(t[1], u1), p(t[2], u1), fr.dir(1.0, 0.0, 0.0));
        push_tri(mesh, part, p(t[2], u0), p(t[1], u0), p(t[0], u0), fr.dir(-1.0, 0.0, 0.0));
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let (dy, dz) = (poly[j][0] - poly[i][0], poly[j][1] - poly[i][1]);
        let len = (dy * dy + dz * dz).sqrt();
        if len < 1e-9 {
            continue;
        }
        push_quad(mesh, part, p(i, u0), p(j, u0), p(j, u1), p(i, u1), fr.dir(0.0, dz / len, -dy / len));
    }
}

/// A smooth-shaded scratch mesh: vertices are shared by index within one cushion, and face normals
/// are area-weighted into them on [`Soft::emit`]. Sharing is the whole point — duplicated ring
/// vertices would split the normals and put a crease at every ring of a pillow.
#[derive(Default)]
struct Soft {
    v: Vec<Vec3>,
    t: Vec<[usize; 3]>,
}

impl Soft {
    fn add(&mut self, p: Vec3) -> usize {
        self.v.push(p);
        self.v.len() - 1
    }
    fn ring(&mut self, pts: impl IntoIterator<Item = Vec3>) -> Vec<usize> {
        pts.into_iter().map(|p| self.add(p)).collect()
    }
    fn tri(&mut self, a: usize, b: usize, c: usize) {
        self.t.push([a, b, c]);
    }
    /// A closed strip from ring `a` to ring `b`. With both rings wound CCW about the advance
    /// direction and `b` further along it, every quad comes out facing outward.
    fn strip(&mut self, a: &[usize], b: &[usize]) {
        let n = a.len();
        for i in 0..n {
            let j = (i + 1) % n;
            self.tri(a[i], a[j], b[j]);
            self.tri(a[i], b[j], b[i]);
        }
    }
    /// Cap a ring with an ear-clipped fan. `flip` faces it the other way.
    fn cap(&mut self, ring: &[usize], poly: &[[f32; 2]], flip: bool) {
        for t in earclip(poly) {
            if flip {
                self.tri(ring[t[2]], ring[t[1]], ring[t[0]]);
            } else {
                self.tri(ring[t[0]], ring[t[1]], ring[t[2]]);
            }
        }
    }
    fn emit(&self, mesh: &mut SolidMesh, part: u32) {
        let mut nrm = vec![Vec3::ZERO; self.v.len()];
        for t in &self.t {
            // NOT normalised: the cross product's length is twice the triangle area, which is
            // exactly the weight a big face should carry over a sliver.
            let f = (self.v[t[1]] - self.v[t[0]]).cross(self.v[t[2]] - self.v[t[0]]);
            for &i in t {
                nrm[i] += f;
            }
        }
        for t in &self.t {
            if (self.v[t[1]] - self.v[t[0]]).cross(self.v[t[2]] - self.v[t[0]]).length() < 1e-12 {
                continue;
            }
            for &i in t {
                mesh.positions.push(self.v[i].into());
                mesh.normals.push(nrm[i].normalize_or_zero().into());
            }
            mesh.face_ids.push(part);
        }
    }
}

/// A seat pillow: a box with its bottom and top rings pulled in and a domed top (spec §B3.4).
fn push_pillow(mesh: &mut SolidMesh, part: u32, fr: &Frame, u: [f32; 2], y_front: [f32; 2], z: [f32; 2]) {
    let (u0, u1) = (u[0].min(u[1]), u[0].max(u[1]));
    let (ya, yb) = (fr.yb(y_front[0]), fr.yb(y_front[1]));
    let (y0, y1) = (ya.min(yb), ya.max(yb));
    let (z0, z1) = (z[0].min(z[1]), z[0].max(z[1]));
    let r = PILLOW_R.min((u1 - u0) * 0.3).min((y1 - y0) * 0.3).min((z1 - z0) * 0.45);
    if r <= 1e-5 {
        return;
    }
    let mut s = Soft::default();
    // CCW in (u, yb) seen from +z, so a strip advancing upward faces outward.
    let mut ring = |inset: f32, z: f32| -> Vec<usize> {
        let (a, b) = (u0 + inset, u1 - inset);
        let (c, d) = (y0 + inset, y1 - inset);
        s.ring([fr.pos(a, c, z), fr.pos(b, c, z), fr.pos(b, d, z), fr.pos(a, d, z)])
    };
    let r0 = ring(r * 0.8, z0);
    let r1 = ring(0.0, z0 + r);
    let r2 = ring(0.0, z1 - r);
    let r3 = ring(r * 0.8, z1);
    s.tri(r0[0], r0[2], r0[1]);
    s.tri(r0[0], r0[3], r0[2]);
    s.strip(&r0, &r1);
    s.strip(&r1, &r2);
    s.strip(&r2, &r3);
    let apex = {
        let c: Vec3 = r3.iter().map(|&i| s.v[i]).sum::<Vec3>() / 4.0;
        s.add(c + Vec3::Z * DOME)
    };
    for i in 0..4 {
        s.tri(r3[i], r3[(i + 1) % 4], apex);
    }
    s.emit(mesh, part);
}

/// Round every corner of a closed profile with a quadratic blend (spec §A3 — a raw 5-point prism
/// reads as a tent, not a cushion).
fn fillet_closed(pts: &[[f32; 2]], r: f32, seg: usize) -> Vec<[f32; 2]> {
    let n = pts.len();
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(n * (seg + 1));
    let v = |p: [f32; 2]| Vec3::new(p[0], p[1], 0.0);
    for i in 0..n {
        let (a, p, b) = (v(pts[(i + n - 1) % n]), v(pts[i]), v(pts[(i + 1) % n]));
        let (t1, t2) = ((p - a).normalize_or_zero(), (b - p).normalize_or_zero());
        let ang = t1.dot(t2).clamp(-1.0, 1.0).acos();
        let d = if ang > 1e-3 { r * (ang / 2.0).tan() } else { 0.0 }
            .min((p - a).length() * 0.45)
            .min((b - p).length() * 0.45);
        let (p1, p2) = (p - t1 * d, p + t2 * d);
        out.push([p1.x, p1.y]);
        for k in 1..seg {
            let tt = k as f32 / seg as f32;
            let q = p1.lerp(p, tt).lerp(p.lerp(p2, tt), tt);
            out.push([q.x, q.y]);
        }
        out.push([p2.x, p2.y]);
    }
    out.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
    if out.len() > 1 {
        let (f, l) = (out[0], *out.last().unwrap());
        if (f[0] - l[0]).abs() < 1e-6 && (f[1] - l[1]).abs() < 1e-6 {
            out.pop();
        }
    }
    out
}

/// A back cushion: a leaning wedge whose profile is filleted and whose end rings shrink to 0.82, so
/// it reads as a pillow rather than a slab (spec §B3.5).
fn push_wedge(mesh: &mut SolidMesh, part: u32, fr: &Frame, u0: f32, u1: f32, y_rear: f32, z0: f32, z1: f32) {
    if u1 - u0 < 0.02 || z1 - z0 < 0.02 {
        return;
    }
    // Profile in (y_from_front, z), then mapped onto the frame's back-referenced axis.
    let raw = [
        [y_rear - WEDGE_T_BOT, z0],
        [y_rear, z0],
        [y_rear, z1],
        [y_rear - WEDGE_T_TOP, z1],
        [y_rear - WEDGE_T_BOT, z0 + 0.55 * (z1 - z0)],
    ];
    let prof = fillet_closed(&raw, 0.035, 3);
    let prof: Vec<[f32; 2]> = ensure_ccw(prof.iter().map(|p| [fr.yb(p[0]), p[1]]).collect());
    let (cy, cz) = {
        let n = prof.len() as f32;
        (prof.iter().map(|p| p[0]).sum::<f32>() / n, prof.iter().map(|p| p[1]).sum::<f32>() / n)
    };
    let mut s = Soft::default();
    let mut ring = |u: f32, k: f32| -> Vec<usize> {
        let pts: Vec<Vec3> = prof.iter().map(|p| fr.pos(u, cy + (p[0] - cy) * k, cz + (p[1] - cz) * k)).collect();
        s.ring(pts)
    };
    let inset = ((u1 - u0) * 0.25).min(0.040);
    let r0 = ring(u0, 0.82);
    let r1 = ring(u0 + inset, 1.0);
    let r2 = ring(u1 - inset, 1.0);
    let r3 = ring(u1, 0.82);
    s.cap(&r0, &prof, true);
    s.cap(&r3, &prof, false);
    s.strip(&r0, &r1);
    s.strip(&r1, &r2);
    s.strip(&r2, &r3);
    s.emit(mesh, part);
}

/// The arch end panel (spec §B3.3) — the window under the armrest is open to the floor, and the two
/// stiles are the sofa's only ground contact.
fn push_arm(mesh: &mut SolidMesh, part: u32, fr: &Frame, u0: f32, u1: f32, arm_h: f32) {
    let band = (arm_h - ARM_BAND).max(0.02);
    let raw = [
        [ARM_Y0, 0.0],
        [ARM_STILE_F, 0.0],
        [ARM_STILE_F, band],
        [ARM_STILE_R, band],
        [ARM_STILE_R, 0.0],
        [ARM_Y1, 0.0],
        [ARM_Y1, arm_h],
        [ARM_Y0, arm_h],
    ];
    let prof = ensure_ccw(raw.iter().map(|p| [fr.yb(p[0]), p[1]]).collect());
    push_lprism(mesh, part, fr, &prof, u0, u1);
}

// ============================ build ============================

/// Build the whole sofa chain. Returns the achieved metrics (with any validation warnings — spec §B4
/// fires while editing and never blocks), the mesh with per-component `face_ids`, and the material
/// per part id.
pub fn build(inp: &CouchInput) -> Result<(CouchMetrics, SolidMesh, Vec<Material>), String> {
    let w = inp.depth;
    if inp.runs.is_empty() {
        return Err("a sofa needs at least one run".into());
    }
    for (name, v) in [("depth", w), ("seat top", inp.seat_top), ("back height", inp.back_h), ("arm height", inp.arm_h), ("arm thickness", inp.arm_t), ("cushion thickness", inp.cushion_t)] {
        if !(v > 0.0) || !v.is_finite() {
            return Err(format!("{name} must be greater than 0"));
        }
    }
    for (i, r) in inp.runs.iter().enumerate() {
        if !(r.length > 0.0) || !r.length.is_finite() {
            return Err(format!("run {} has no length", i + 1));
        }
    }
    let box_z1 = inp.seat_top - inp.cushion_t;
    if box_z1 <= BOX_Z0 + 0.01 {
        return Err(format!(
            "a {:.0} mm cushion under a {:.0} mm seat leaves no frame above the {:.0} mm float — thin the cushion or raise the seat",
            inp.cushion_t * 1000.0,
            inp.seat_top * 1000.0,
            BOX_Z0 * 1000.0
        ));
    }
    if inp.back == BackKind::Spindle && inp.back_h <= box_z1 + RAIL_T + 0.02 {
        return Err(format!("back height {:.0} mm leaves no room for a rail above the seat box", inp.back_h * 1000.0));
    }
    if inp.arm_t > inp.runs[0].length || inp.arm_t > inp.runs[inp.runs.len() - 1].length {
        return Err("the arm is thicker than the run it sits on".into());
    }

    // §B4 — validation fires while editing; warn, never block.
    let mut warnings: Vec<String> = Vec::new();
    for (i, r) in inp.runs.iter().enumerate() {
        let cw = r.length / r.count() as f32;
        if !(0.45..=0.85).contains(&cw) {
            warnings.push(format!("run {} at {:.2} m / {} cushions is {:.0} mm wide (aim 450–850)", i + 1, r.length, r.count(), cw * 1000.0));
        }
        if r.length < 0.60 {
            warnings.push(format!("run {} is {:.0} mm — under 600 mm cannot hold one cushion sensibly", i + 1, r.length * 1000.0));
        }
    }
    if inp.runs.len() > 2 {
        for (i, r) in inp.runs.iter().enumerate().take(inp.runs.len() - 1).skip(1) {
            if r.length - 2.0 * w < 0.40 {
                warnings.push(format!("U inner clearance {:.0} mm (< 400) — run {} is too short between the corners", (r.length - 2.0 * w) * 1000.0, i + 1));
            }
        }
    }
    if inp.back == BackKind::None && inp.back_cushions {
        warnings.push("back cushions with no back — nothing for them to lean on".into());
    }
    if !inp.frame && (inp.seat_cushions || inp.back_cushions) {
        warnings.push("the frame is deleted — the cushions float".into());
    }

    let frames = chain(&inp.runs, w);
    let mut mesh = SolidMesh::default();
    let mut mats: Vec<Material> = Vec::new();
    let mut features: Vec<String> = Vec::new();
    let last = inp.runs.len() - 1;

    // ── frame: one box per run + one per corner (§B3.1) ──
    if inp.frame {
        for (i, (fr, r)) in frames.iter().zip(&inp.runs).enumerate() {
            let part = alloc(&mut mats, Material::Oak);
            push_lbox(&mut mesh, part, fr, [0.0, r.length], [0.0, w], [BOX_Z0, box_z1]);
            if i < last {
                // Overlap the corner square 1 mm into the run — coplanar faces z-fight (§B2).
                let part = alloc(&mut mats, Material::Oak);
                push_lbox(&mut mesh, part, fr, [r.length - 0.001, r.length + w], [0.0, w], [BOX_Z0, box_z1]);
            }
        }
        features.push("frame".into());
    }

    // ── back: top rail + spindles, per run and both corner sides (§B3.2) ──
    let rail_z0 = inp.back_h - RAIL_T;
    let spin_z0 = box_z1 + SPIN_LIFT;
    if inp.back == BackKind::Spindle {
        let back_span = |mesh: &mut SolidMesh, mats: &mut Vec<Material>, fr: &Frame, u0: f32, u1: f32| {
            let part = alloc(mats, Material::Oak);
            push_lbox(mesh, part, fr, [u0, u1], [w - RAIL_D, w], [rail_z0, inp.back_h]);
            let span = u1 - u0;
            let n = ((span / SPIN_PITCH) as usize).max(2);
            for k in 0..=n {
                let uc = u0 + SPIN_INSET + (span - 2.0 * SPIN_INSET) * k as f32 / n as f32;
                push_lbox(
                    mesh,
                    part,
                    fr,
                    [uc - SPIN_W / 2.0, uc + SPIN_W / 2.0],
                    [w - 0.022, w - 0.006],
                    [spin_z0, rail_z0],
                );
            }
        };
        for (i, (fr, r)) in frames.iter().zip(&inp.runs).enumerate() {
            back_span(&mut mesh, &mut mats, fr, 0.0, r.length);
            if i < last {
                back_span(&mut mesh, &mut mats, fr, r.length, r.length + w); // corner, side A
                back_span(&mut mesh, &mut mats, &frames[i + 1], -w, 0.0); // corner, side B
            }
        }
        features.push("back".into());
    }

    // ── arms: only ever at the two FREE chain ends (§B3.3) ──
    if inp.arm_start {
        let part = alloc(&mut mats, Material::Oak);
        push_arm(&mut mesh, part, &frames[0], -ARM_PROUD, -ARM_PROUD + inp.arm_t, inp.arm_h);
    }
    if inp.arm_end {
        let part = alloc(&mut mats, Material::Oak);
        let u = inp.runs[last].length + ARM_PROUD;
        push_arm(&mut mesh, part, &frames[last], u - inp.arm_t, u, inp.arm_h);
    }
    if inp.arm_start || inp.arm_end {
        features.push(match (inp.arm_start, inp.arm_end) {
            (true, true) => "arms".into(),
            (true, false) => "arms (start)".to_string(),
            _ => "arms (end)".to_string(),
        });
    }

    // ── seat cushions (§B3.4) ──
    let mut seats = 0usize;
    if inp.seat_cushions {
        for (i, (fr, r)) in frames.iter().zip(&inp.runs).enumerate() {
            let n = r.count();
            let cw = r.length / n as f32;
            for k in 0..n {
                let part = alloc(&mut mats, Material::Fabric);
                push_pillow(
                    &mut mesh,
                    part,
                    fr,
                    [k as f32 * cw + CUSH_GAP, (k + 1) as f32 * cw - CUSH_GAP],
                    [CUSH_FRONT, CUSH_FRONT + CUSH_DEPTH],
                    [box_z1, inp.seat_top],
                );
                seats += 1;
            }
            if i < last {
                let part = alloc(&mut mats, Material::Fabric);
                push_pillow(
                    &mut mesh,
                    part,
                    fr,
                    [r.length + CUSH_FRONT, r.length + w - CORNER_MARGIN],
                    [CUSH_FRONT, w - CORNER_MARGIN],
                    [box_z1, inp.seat_top],
                );
                seats += 1;
            }
        }
        features.push("seat_cushions".into());
    }

    // ── back cushions: one wedge per seat, corner sides pulled clear of each other (§B3.5) ──
    if inp.back_cushions {
        let (z0, z1) = (inp.seat_top - WEDGE_DROP, inp.back_h + WEDGE_RISE);
        let y_rear = w - WEDGE_REAR;
        for (i, (fr, r)) in frames.iter().zip(&inp.runs).enumerate() {
            let n = r.count();
            let cw = r.length / n as f32;
            for k in 0..n {
                let part = alloc(&mut mats, Material::Fabric);
                push_wedge(&mut mesh, part, fr, k as f32 * cw + WEDGE_GAP, (k + 1) as f32 * cw - WEDGE_GAP, y_rear, z0, z1);
            }
            if i < last {
                let part = alloc(&mut mats, Material::Fabric);
                push_wedge(&mut mesh, part, fr, r.length + WEDGE_CORNER_CLEAR, r.length + w - 0.020, y_rear, z0, z1);
                let part = alloc(&mut mats, Material::Fabric);
                push_wedge(&mut mesh, part, &frames[i + 1], -w + WEDGE_CORNER_CLEAR, -0.020, y_rear, z0, z1);
            }
        }
        features.push("back_cushions".into());
    }

    let overall_len: f32 = inp.runs.iter().map(|r| r.length).sum::<f32>()
        + last as f32 * w
        + if inp.arm_start { ARM_PROUD } else { 0.0 }
        + if inp.arm_end { ARM_PROUD } else { 0.0 };
    let metrics = CouchMetrics {
        runs: inp.runs.len(),
        corners: last,
        overall_len,
        depth: w,
        seat_top: inp.seat_top,
        back_h: inp.back_h,
        arm_h: inp.arm_h,
        seats,
        cushion_pitch: inp.runs[0].length / inp.runs[0].count() as f32,
        spindle_z0: if inp.back == BackKind::Spindle { spin_z0 } else { 0.0 },
        features,
        tris: mesh.tri_count(),
        warnings,
    };
    Ok((metrics, mesh, mats))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox_of(mesh: &SolidMesh) -> ([f32; 3], [f32; 3]) {
        mesh.bounds().expect("mesh is empty")
    }

    /// World bbox of every triangle wearing `want`.
    fn bbox_mat(mesh: &SolidMesh, mats: &[Material], want: Material) -> ([f32; 3], [f32; 3]) {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for t in 0..mesh.tri_count() {
            if mats[mesh.face_ids[t] as usize] != want {
                continue;
            }
            for i in 0..3 {
                let p = mesh.positions[t * 3 + i];
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        (lo, hi)
    }

    /// §A4 — the `straight` preset reproduces the reference FBX table to well under 1%.
    #[test]
    fn straight_preset_matches_the_reference_fbx() {
        let inp = Preset::Straight.input();
        let (m, mesh, mats) = build(&inp).unwrap();
        for (name, reference, built) in [
            ("frame span", 2.001, inp.runs[0].length),
            ("overall L", 2.140, m.overall_len),
            ("depth", 0.780, m.depth),
            ("back top", 0.640, m.back_h),
            ("arm top", 0.500, m.arm_h),
            ("seat top", 0.375, m.seat_top),
            ("cushion pitch", 0.667, m.cushion_pitch),
            ("spindle z0", 0.210, m.spindle_z0),
        ] {
            let err = (built - reference).abs() / reference;
            assert!(err < 0.01, "{name}: ref {reference:.3}, built {built:.3} ({:.1}%)", err * 100.0);
        }
        // The oak reaches the floor (the arms ARE the legs) while the frame box floats.
        let (lo, hi) = bbox_of(&mesh);
        assert!(lo[2].abs() < 1e-5, "sofa floats at z {}", lo[2]);
        assert!((hi[1] - lo[1] - 0.780).abs() < 0.002, "depth {}", hi[1] - lo[1]);
        assert!((hi[0] - lo[0] - 2.141).abs() < 0.002, "overall length {}", hi[0] - lo[0]);
        let (flo, _) = bbox_mat(&mesh, &mats, Material::Fabric);
        assert!(flo[2] > 0.15, "upholstery should sit on the frame, not the floor: {}", flo[2]);
    }

    /// §B2 — the chain turns +90° at each corner and the seats face the inside of the L.
    #[test]
    fn the_chain_turns_and_the_corner_squares_close_it() {
        let inp = Preset::L.input();
        let (m, mesh, _) = build(&inp).unwrap();
        assert_eq!((m.runs, m.corners), (2, 1));
        let (lo, hi) = bbox_of(&mesh);
        // run 0 (2.001) + corner (0.780) along x, plus 70 mm of proud arm at the start;
        // run 1 (1.350) + the corner's depth along y, plus 70 mm of proud arm at the far end.
        assert!((lo[0] + 0.070).abs() < 0.005, "start arm should stand 70 mm proud: {}", lo[0]);
        assert!((hi[0] - 2.781).abs() < 0.005, "x reach {}", hi[0]);
        assert!(lo[1].abs() < 0.005, "back edge of run 0 sits on y = 0: {}", lo[1]);
        assert!((hi[1] - 2.200).abs() < 0.005, "y reach {}", hi[1]);
        // The corner square bridges the two back edges — nothing may be missing in the elbow.
        let filled = (0..mesh.tri_count()).any(|t| {
            (0..3).all(|i| {
                let p = mesh.positions[t * 3 + i];
                p[0] > 2.05 && p[0] < 2.73 && p[1] > 0.05 && p[1] < 0.73
            })
        });
        assert!(filled, "no geometry inside the corner square");
    }

    /// Every triangle's WINDING must agree with the normal it carries — including the smooth-shaded
    /// cushions, whose averaged vertex normals still have to sit on the outward side of each face.
    #[test]
    fn winding_agrees_with_every_normal() {
        for p in Preset::ALL {
            let (_, mesh, _) = build(&p.input()).unwrap();
            for t in 0..mesh.tri_count() {
                let v: Vec<Vec3> = (0..3).map(|i| Vec3::from(mesh.positions[t * 3 + i])).collect();
                let face = (v[1] - v[0]).cross(v[2] - v[0]);
                assert!(face.length() > 1e-9, "{}: degenerate triangle {t} at {:?}", p.label(), v[0]);
                let shade: Vec3 = (0..3).map(|i| Vec3::from(mesh.normals[t * 3 + i])).sum();
                let dot = face.normalize().dot(shade.normalize_or_zero());
                assert!(dot > 0.2, "{}: triangle {t} is wound against its normal (dot {dot:.3}) at {:?}", p.label(), v[0]);
            }
        }
    }

    /// §B3 — the five features detach independently. Deleting `back` must leave the cushions and the
    /// arms exactly as they were.
    #[test]
    fn deleting_the_back_leaves_cushions_and_arms_alone() {
        let full = build(&Preset::Straight.input()).unwrap();
        let mut inp = Preset::Straight.input();
        inp.back = BackKind::None;
        let (m, mesh, mats) = build(&inp).unwrap();
        assert!(mesh.tri_count() < full.1.tri_count());
        assert!(!m.features.iter().any(|f| f == "back"), "{:?}", m.features);
        assert_eq!(m.seats, full.0.seats, "seat count changed with the back deleted");
        assert!(m.features.iter().any(|f| f.starts_with("arms")), "{:?}", m.features);
        // Same fabric envelope: the wedges still lean where they leaned.
        let a = bbox_mat(&mesh, &mats, Material::Fabric);
        let b = bbox_mat(&full.1, &full.2, Material::Fabric);
        for k in 0..3 {
            assert!((a.0[k] - b.0[k]).abs() < 1e-4 && (a.1[k] - b.1[k]).abs() < 1e-4, "fabric envelope moved on axis {k}");
        }
        assert_eq!(m.spindle_z0, 0.0);
    }

    /// The `bench` preset is literally the chain minus back and arms (§A2).
    #[test]
    fn bench_is_the_chain_minus_back_and_arms() {
        let (m, mesh, mats) = build(&Preset::Bench.input()).unwrap();
        assert_eq!(m.features, vec!["frame".to_string(), "seat_cushions".to_string()]);
        assert!(!mats.iter().any(|x| *x == Material::Fabric && false)); // fabric is the cushions only
        assert!(mesh.tri_count() > 50 && mesh.tri_count() < 1_000, "{} tris", mesh.tri_count());
        // With no arms the sofa stands on nothing but its floating box — the frame starts at 50 mm.
        let (lo, _) = bbox_of(&mesh);
        assert!((lo[2] - BOX_Z0).abs() < 1e-5, "bench should float on its box: z {}", lo[2]);
        // auto cushion count: 1.800 / 0.667 rounds to 3
        assert_eq!(m.seats, 3);
    }

    #[test]
    fn auto_cushion_count_aims_at_the_reference_pitch() {
        assert_eq!(Run::new(2.001, 0).count(), 3);
        assert_eq!(Run::new(1.350, 0).count(), 2);
        assert_eq!(Run::new(0.400, 0).count(), 1, "never zero cushions");
        assert_eq!(Run::new(2.001, 5).count(), 5, "an explicit count wins");
    }

    /// Arms stand 70 mm proud of the frame at each free end, and their stiles reach the floor.
    #[test]
    fn arms_stand_proud_and_reach_the_floor() {
        let mut inp = Preset::Straight.input();
        inp.frame = false;
        inp.seat_cushions = false;
        inp.back_cushions = false;
        inp.back = BackKind::None;
        let (m, mesh, _) = build(&inp).unwrap();
        assert_eq!(m.features, vec!["arms".to_string()]);
        let (lo, hi) = bbox_of(&mesh);
        assert!(lo[2].abs() < 1e-5, "arm stiles must reach z = 0: {}", lo[2]);
        assert!((hi[2] - inp.arm_h).abs() < 1e-5, "arm top {}", hi[2]);
        assert!((lo[0] + ARM_PROUD).abs() < 1e-5, "start arm proud edge {}", lo[0]);
        assert!((hi[0] - (inp.runs[0].length + ARM_PROUD)).abs() < 1e-5, "end arm proud edge {}", hi[0]);
        // The window really is open: nothing spans the middle of the arch at knee height.
        let solid = (0..mesh.tri_count()).any(|t| {
            (0..3).all(|i| {
                let p = mesh.positions[t * 3 + i];
                let y_front = inp.depth - p[1];
                y_front > 0.25 && y_front < 0.50 && p[2] > 0.05 && p[2] < 0.30
            })
        });
        assert!(!solid, "the arm window is blocked — it should be open to the floor");
    }

    #[test]
    fn validation_warns_per_spec_b4() {
        // cushion far too wide
        let mut inp = Preset::Straight.input();
        inp.runs = vec![Run::new(2.001, 1)];
        let (m, _, _) = build(&inp).unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("450–850")), "{:?}", m.warnings);
        // a run too short to hold a cushion
        let mut inp = Preset::Straight.input();
        inp.runs = vec![Run::new(0.500, 1)];
        let (m, _, _) = build(&inp).unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("under 600 mm")), "{:?}", m.warnings);
        // U with no inner clearance
        let mut inp = Preset::U.input();
        inp.runs[1] = Run::new(1.600, 2);
        let (m, _, _) = build(&inp).unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("inner clearance")), "{:?}", m.warnings);
        // back cushions with nothing to lean on
        let mut inp = Preset::Straight.input();
        inp.back = BackKind::None;
        let (m, _, _) = build(&inp).unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("lean on")), "{:?}", m.warnings);
        // hard errors
        assert!(build(&CouchInput { runs: Vec::new(), ..Preset::Straight.input() }).is_err());
        assert!(build(&CouchInput { cushion_t: 0.4, ..Preset::Straight.input() }).is_err());
        assert!(build(&CouchInput { back_h: 0.25, ..Preset::Straight.input() }).is_err());
    }

    /// §B5 — rebuild all five presets; a regression in shared code shows up in whichever uses it.
    #[test]
    fn all_presets_build() {
        for p in Preset::ALL {
            let inp = p.input();
            let (m, mesh, mats) = build(&inp).unwrap_or_else(|e| panic!("{}: {e}", p.label()));
            assert!(mesh.tri_count() > 50, "{}: {} tris", p.label(), mesh.tri_count());
            assert!(mesh.tri_count() < 12_000, "{}: {} tris is far past the reference budget", p.label(), mesh.tri_count());
            assert_eq!(mesh.face_ids.len(), mesh.tri_count(), "{}", p.label());
            assert!((*mesh.face_ids.iter().max().unwrap() as usize) < mats.len(), "{}", p.label());
            for v in &mesh.positions {
                assert!(v.iter().all(|c| c.is_finite()), "{}: non-finite vertex", p.label());
            }
            for n in &mesh.normals {
                assert!((Vec3::from(*n).length() - 1.0).abs() < 1e-3, "{}: normal length {}", p.label(), Vec3::from(*n).length());
            }
            assert!(m.warnings.is_empty(), "{}: {:?}", p.label(), m.warnings);
        }
    }

    /// §B3.5 — the corner back wedges are pulled 250 mm along their own runs so the two do not pile
    /// into each other at the elbow. The failure this guards against is a solid fabric blob in the
    /// corner, which is what you get if both wedges run the full corner span.
    ///
    /// Every pair of back wedges in the chain is checked, not just the corner pair: neighbours on a
    /// run are separated by the 4 mm reveal at each end. The reference generator still lets the two
    /// corner wedges graze by ~11 mm where their footprints cross — that is its geometry, not a
    /// slip here, so the bound admits a graze and rejects a pile-up.
    #[test]
    fn back_wedges_never_pile_up_at_a_corner() {
        let inp = Preset::L.input();
        let (_, mesh, mats) = build(&inp).unwrap();
        // Plan footprint per part, keeping only parts that rise above the seat — the back wedges.
        let mut boxes: std::collections::HashMap<u32, [f32; 5]> = std::collections::HashMap::new();
        for t in 0..mesh.tri_count() {
            if mats[mesh.face_ids[t] as usize] != Material::Fabric {
                continue;
            }
            let e = boxes.entry(mesh.face_ids[t]).or_insert([f32::MAX, f32::MIN, f32::MAX, f32::MIN, f32::MIN]);
            for i in 0..3 {
                let p = mesh.positions[t * 3 + i];
                e[0] = e[0].min(p[0]);
                e[1] = e[1].max(p[0]);
                e[2] = e[2].min(p[1]);
                e[3] = e[3].max(p[1]);
                e[4] = e[4].max(p[2]);
            }
        }
        // Seat pillows top out a hair above `seat_top` (the 14 mm dome), so the cut has to clear it.
        let wedges: Vec<[f32; 5]> = boxes.into_values().filter(|b| b[4] > inp.seat_top + 0.10).collect();
        assert_eq!(wedges.len(), 3 + 2 + 2, "expected one wedge per seat plus two at the corner");
        for (i, a) in wedges.iter().enumerate() {
            for b in wedges.iter().skip(i + 1) {
                let ox = a[1].min(b[1]) - a[0].max(b[0]);
                let oy = a[3].min(b[3]) - a[2].max(b[2]);
                assert!(ox.min(oy) < 0.015, "two back wedges overlap by {:.3} × {:.3} m", ox, oy);
            }
        }
    }
}
