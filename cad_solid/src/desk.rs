//! Parametric **office workstation desk**, ported from `DESK_BUILD.md` (Part B) — reverse-engineered
//! from a BlenderKit "workstation office desk" (1.36 × 1.60 × 1.03 m): white chamfered top, curved
//! fabric privacy screen standing ON the top, splayed Λ "compass" legs at one end and a drawer
//! pedestal with a fingerprint lock at the other.
//!
//! The brief's headline is that the desk is a **feature tree**: every feature is an independent node
//! the user can turn off, and deleting one takes its subtractions with it (a grommet takes its hole).
//! Here that tree is the [`DeskInput`] itself — `partition`, `rail`, each [`Support`] (including
//! [`Support::None`]) and the `grommets` list switch features in and out, and the build re-emits
//! without them. A removed support only *warns* about the cantilever, per spec §B3: the user asked to
//! be able to delete anything.
//!
//! ```text
//! desk
//! |- top          chamfered slab; one rounded hole per grommet (the ONLY subtractive geometry)
//! |- partition    rounded shell + dark trim ring + proud fabric panel per face + organiser band
//! |- support_L    Legs | Drawers | Panel | None
//! |- support_R    Legs | Drawers | Panel | None
//! |- rail         under-top tube between the two supports
//! |- grommets     alu collar + dark liner
//! ```
//!
//! Frame (spec §B0): length `L` along **x**, depth `W` along **y** (the user sits at −y), `z = 0`
//! floor, `H` = top **surface** height. Centred on x/y. Metres throughout.
//!
//! PURE geometry — **no boolean**. The grommet holes are cut by decomposing the slab's caps into
//! rectangle-minus-rectangles ([`rect_minus_holes`]) and walling the openings, which is exact for the
//! axis-aligned lozenges and costs no CSG. Output is a [`SolidMesh`] whose `face_ids` tag each
//! component (every drawer front is its own selectable part) plus a [`Material`] per part id, so the
//! app paints one flat swatch per surface — the same contract as [`crate::cabin`].
//!
//! Not ported: the reference's rounded grommet *corners* (the alu collar covers the opening edge, so
//! the hole is cut square and nothing shows), and the screen's woven-fabric texture (our engine paints
//! flat swatches; the app binds a fabric-grey swatch to the `Fabric` parts).

use crate::SolidMesh;
use glam::Vec3;

// ── fixed geometry (spec §B2; metres) ──
const EDGE_CH: f32 = 0.008; // chamfer around the top's lower edge
const GR_L: f32 = 0.160; // grommet lozenge length
const GR_D: f32 = 0.034; // grommet lozenge depth
const GR_REAR: f32 = 0.105; // rear grommets sit this far in from the back edge
const PART_T: f32 = 0.012; // screen shell thickness
const PART_R: f32 = 0.140; // screen top-corner radius
const PART_INSET: f32 = 0.045; // fabric panel border
const BAND_H: f32 = 0.130; // organiser band height
const LEG_R: f32 = 0.013; // Λ tube radius
const LEG_STANCE: f32 = 0.040; // foot kick BEYOND the top's depth edge
const LEG_KICK_X: f32 = 0.070; // foot kick outward along x from the hub
const LEG_INSET: f32 = 0.100; // hub inset from the end of the top
const HUB_R: f32 = 0.022; // hub sphere radius
const FOOT_H: f32 = 0.095; // sleeve cap height
const FILLET_R: f32 = 0.065; // elbow radius near the floor
const PLINTH_Z: f32 = 0.040; // pedestal plinth height
const RAIL_R: f32 = 0.014;
const RAIL_SINK: f32 = 0.016; // rail centre below the top's underside

/// What holds up one end of the top (spec §B1 — set independently per end; this is how "legs on the
/// left, drawers on the right" works). [`Support::None`] deletes the support entirely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// The signature splayed Λ compass frame.
    Legs,
    /// A drawer pedestal (its top IS the support — no legs needed at that end).
    Drawers,
    /// A plain slab end panel.
    Panel,
    /// Deleted — the top cantilevers from the other end.
    None,
}

impl Support {
    pub const ALL: [Support; 4] = [Support::Legs, Support::Drawers, Support::Panel, Support::None];
    pub fn label(self) -> &'static str {
        match self {
            Support::Legs => "Λ legs",
            Support::Drawers => "Drawer pedestal",
            Support::Panel => "End panel",
            Support::None => "None (deleted)",
        }
    }
}

/// Which face the drawer fronts open on (spec §B2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PedFace {
    /// Fronts at −y — a single desk you sit at.
    Front,
    /// Fronts on the outer x face — a bench, like the reference.
    End,
}

impl PedFace {
    pub const ALL: [PedFace; 2] = [PedFace::Front, PedFace::End];
    pub fn label(self) -> &'static str {
        match self {
            PedFace::Front => "Front (user side)",
            PedFace::End => "End (bench)",
        }
    }
}

/// Where the screen stands (spec §B2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartPos {
    /// A privacy screen along the back edge.
    Rear,
    /// On the centre line — two users face to face (wants depth ≥ 1.10 m).
    Center,
}

impl PartPos {
    pub const ALL: [PartPos; 2] = [PartPos::Rear, PartPos::Center];
    pub fn label(self) -> &'static str {
        match self {
            PartPos::Rear => "Rear (privacy)",
            PartPos::Center => "Centre (bench)",
        }
    }
}

/// One cable port. `rear` derives `y` from the depth (spec §B2 default `W/2 − 105`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grommet {
    pub x: f32,
    /// Only used when `rear` is false.
    pub y: f32,
    pub rear: bool,
}

impl Grommet {
    pub fn rear_at(x: f32) -> Self {
        Self { x, y: 0.0, rear: true }
    }
    fn xy(self, w: f32) -> (f32, f32) {
        (self.x, if self.rear { w / 2.0 - GR_REAR } else { self.y })
    }
}

/// The material a component wears — one flat swatch per part in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Material {
    /// Top, carcass, plinth, screen shell, end panel.
    White,
    /// Drawer fronts.
    Oak,
    /// Screen panels.
    Fabric,
    /// Λ tubes, hubs, rail.
    Metal,
    /// Groove handles, trim ring, feet blocks, liner, fingerprint disc.
    Dark,
    /// Organiser band, grommet collar.
    Alu,
    /// Foot sleeve caps.
    Cap,
}

/// The parametric feature tree (spec §B1). Not `Copy` — it owns the grommet list.
#[derive(Clone, Debug, PartialEq)]
pub struct DeskInput {
    /// Length along x.
    pub length: f32,
    /// Depth along y.
    pub width: f32,
    /// Top SURFACE height.
    pub height: f32,
    pub top_t: f32,
    /// Feature: the privacy screen.
    pub partition: bool,
    pub part_pos: PartPos,
    /// Screen span along x.
    pub part_w: f32,
    /// Screen height ABOVE the top surface.
    pub part_h: f32,
    /// Organiser band at the screen base.
    pub part_band: bool,
    pub sup_l: Support,
    pub sup_r: Support,
    pub ped_w: f32,
    pub ped_n: u8,
    pub ped_face: PedFace,
    /// Feature: the under-top stiffener tube.
    pub rail: bool,
    /// Feature: cable ports. Each takes its hole with it.
    pub grommets: Vec<Grommet>,
}

impl Default for DeskInput {
    /// The brief's example — `combo`: legs left, drawers right.
    fn default() -> Self {
        Preset::Combo.input()
    }
}

/// The five reference configurations (`DESK_BUILD.md`, "Variants").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Replica of the reference FBX — bench, centre screen, pedestal opening on the end.
    Bench,
    Single,
    /// Legs left, drawers right — the brief's example.
    Combo,
    /// Two pedestals, no screen.
    Exec,
    /// A bare table.
    Minimal,
}

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::Bench, Preset::Single, Preset::Combo, Preset::Exec, Preset::Minimal];
    pub fn label(self) -> &'static str {
        match self {
            Preset::Bench => "Bench (reference)",
            Preset::Single => "Single desk",
            Preset::Combo => "Combo (legs + drawers)",
            Preset::Exec => "Executive (two pedestals)",
            Preset::Minimal => "Minimal (bare table)",
        }
    }

    pub fn input(self) -> DeskInput {
        let base = DeskInput {
            length: 1.80,
            width: 0.90,
            height: 0.75,
            top_t: 0.030,
            partition: true,
            part_pos: PartPos::Rear,
            part_w: 1.50,
            part_h: 0.40,
            part_band: true,
            sup_l: Support::Legs,
            sup_r: Support::Legs,
            ped_w: 0.42,
            ped_n: 3,
            ped_face: PedFace::Front,
            rail: true,
            grommets: vec![Grommet::rear_at(0.0)],
        };
        match self {
            Preset::Bench => DeskInput {
                length: 1.60,
                width: 1.24,
                height: 0.78,
                part_pos: PartPos::Center,
                part_w: 1.14,
                part_h: 0.25,
                sup_r: Support::Drawers,
                ped_face: PedFace::End,
                grommets: vec![Grommet { x: 0.70, y: 0.0, rear: false }],
                ..base
            },
            Preset::Single => DeskInput { length: 1.60, width: 0.80, part_w: 1.40, part_h: 0.35, ..base },
            Preset::Combo => DeskInput { sup_r: Support::Drawers, grommets: vec![Grommet::rear_at(0.45)], ..base },
            Preset::Exec => DeskInput {
                length: 2.00,
                width: 1.00,
                partition: false,
                sup_l: Support::Drawers,
                sup_r: Support::Drawers,
                ped_w: 0.45,
                ped_n: 4,
                grommets: vec![Grommet::rear_at(-0.55), Grommet::rear_at(0.55)],
                ..base
            },
            Preset::Minimal => DeskInput { length: 1.40, width: 0.70, height: 0.74, partition: false, grommets: Vec::new(), ..base },
        }
    }
}

/// What the build actually produced — achieved numbers next to the requested ones.
#[derive(Clone, Debug, PartialEq)]
pub struct DeskMetrics {
    pub length: f32,
    pub width: f32,
    pub height: f32,
    /// Absolute z of the screen's top edge (`0` with no screen).
    pub partition_top: f32,
    /// Outer-to-outer foot span of the Λ legs (`0` when neither end has legs) — wider than the top.
    pub leg_stance: f32,
    /// Clear height of one drawer front (`0` with no pedestal).
    pub drawer_front_h: f32,
    pub grommets: usize,
    /// The feature nodes that were actually emitted, in build order.
    pub features: Vec<String>,
    pub tris: usize,
    pub warnings: Vec<String>,
}

// ============================ mesh emitters ============================

fn alloc(mats: &mut Vec<Material>, m: Material) -> u32 {
    mats.push(m);
    (mats.len() - 1) as u32
}

fn push_tri(mesh: &mut SolidMesh, part: u32, a: [f32; 3], b: [f32; 3], c: [f32; 3], n: [f32; 3]) {
    for v in [a, b, c] {
        mesh.positions.push(v);
        mesh.normals.push(n);
    }
    mesh.face_ids.push(part);
}

/// One flat quad `a→b→c→d` (already wound to match `n`).
fn push_quad(mesh: &mut SolidMesh, part: u32, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3], n: [f32; 3]) {
    push_tri(mesh, part, a, b, c, n);
    push_tri(mesh, part, a, c, d, n);
}

/// An axis-aligned box tagged with `part`, outward flat normals.
fn push_box(mesh: &mut SolidMesh, part: u32, x: [f32; 2], y: [f32; 2], z: [f32; 2]) {
    let (x0, x1) = (x[0].min(x[1]), x[0].max(x[1]));
    let (y0, y1) = (y[0].min(y[1]), y[0].max(y[1]));
    let (z0, z1) = (z[0].min(z[1]), z[0].max(z[1]));
    if (x1 - x0) < 1e-6 || (y1 - y0) < 1e-6 || (z1 - z0) < 1e-6 {
        return;
    }
    let c = [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0], [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]];
    let quads: [([usize; 4], [f32; 3]); 6] = [
        ([0, 3, 2, 1], [0.0, 0.0, -1.0]),
        ([4, 5, 6, 7], [0.0, 0.0, 1.0]),
        ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
        ([3, 7, 6, 2], [0.0, 1.0, 0.0]),
        ([0, 4, 7, 3], [-1.0, 0.0, 0.0]),
        ([1, 2, 6, 5], [1.0, 0.0, 0.0]),
    ];
    for (q, n) in quads {
        push_quad(mesh, part, c[q[0]], c[q[1]], c[q[2]], c[q[3]], n);
    }
}

/// Which pair of axes a 2D profile lives in; the third is the extrusion axis.
#[derive(Clone, Copy)]
enum Plane {
    /// poly in (x, z); extrude along y.
    Xz,
    /// poly in (x, y); extrude along z.
    Xy,
}

fn map3(plane: Plane, p: f32, q: f32, a: f32) -> [f32; 3] {
    match plane {
        Plane::Xz => [p, a, q],
        Plane::Xy => [p, q, a],
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

/// Ear-clip a simple polygon into triangles of ORIGINAL indices. The profiles here (rounded rects)
/// are convex, so this always terminates on the first sweep.
fn earclip(poly: &[[f32; 2]]) -> Vec<[usize; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    let mut ring: Vec<usize> = if signed_area(poly) < 0.0 { (0..n).rev().collect() } else { (0..n).collect() };
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let in_tri = |p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
        let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(neg && pos)
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
                continue;
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

/// A closed 2D profile extruded along the third axis, tagged `part`. Every triangle is oriented
/// outward from the solid's centroid, so winding never has to be tracked by hand.
fn push_prism(mesh: &mut SolidMesh, part: u32, poly: &[[f32; 2]], plane: Plane, a0: f32, a1: f32) {
    let n = poly.len();
    if n < 3 || (a1 - a0).abs() < 1e-7 {
        return;
    }
    let (mut cp, mut cq) = (0.0f32, 0.0f32);
    for v in poly {
        cp += v[0];
        cq += v[1];
    }
    let centroid = Vec3::from(map3(plane, cp / n as f32, cq / n as f32, (a0 + a1) / 2.0));
    let mut tri = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
        let (va, vb, vc) = (Vec3::from(a), Vec3::from(b), Vec3::from(c));
        let out = (va + vb + vc) / 3.0 - centroid;
        let (p, q, r) = if (vb - va).cross(vc - va).dot(out) < 0.0 { (va, vc, vb) } else { (va, vb, vc) };
        let nrm = (q - p).cross(r - p).normalize_or_zero();
        push_tri(mesh, part, p.into(), q.into(), r.into(), nrm.into());
    };
    for t in earclip(poly) {
        for a in [a1, a0] {
            tri(
                map3(plane, poly[t[0]][0], poly[t[0]][1], a),
                map3(plane, poly[t[1]][0], poly[t[1]][1], a),
                map3(plane, poly[t[2]][0], poly[t[2]][1], a),
            );
        }
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let p0 = map3(plane, poly[i][0], poly[i][1], a0);
        let p1 = map3(plane, poly[j][0], poly[j][1], a0);
        let p2 = map3(plane, poly[j][0], poly[j][1], a1);
        let p3 = map3(plane, poly[i][0], poly[i][1], a1);
        tri(p0, p1, p2);
        tri(p0, p2, p3);
    }
}

/// A 2D rounded rectangle centred on the origin, CCW from the bottom-left corner. Radii are given
/// per corner (bottom-left, bottom-right, top-right, top-left) — the screen shell is square at the
/// bottom and rounded on top, which is exactly what makes it read as a screen rather than a board.
fn rrect(w: f32, h: f32, r: [f32; 4], seg: usize) -> Vec<[f32; 2]> {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let corners = [(-hw, -hh, r[0], 180.0f32), (hw, -hh, r[1], 270.0), (hw, hh, r[2], 0.0), (-hw, hh, r[3], 90.0)];
    let mut out = Vec::with_capacity(4 * (seg + 1));
    for (cx, cy, rad, a0) in corners {
        let rad = rad.min(hw).min(hh).max(0.0);
        if rad <= 1e-6 {
            out.push([cx, cy]);
            continue;
        }
        let ccx = cx + if cx < 0.0 { rad } else { -rad };
        let ccy = cy + if cy < 0.0 { rad } else { -rad };
        for i in 0..=seg {
            let a = (a0 + 90.0 * i as f32 / seg as f32).to_radians();
            out.push([ccx + rad * a.cos(), ccy + rad * a.sin()]);
        }
    }
    // A radius equal to the half-height makes neighbouring arcs MEET (the grommet lozenge is a pure
    // stadium), which leaves a duplicated vertex — and a duplicated vertex is a zero-area side quad
    // with a zero-length normal. Collapse them here, at the one place that can create them.
    out.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6);
    if out.len() > 1 {
        let (f, l) = (out[0], *out.last().unwrap());
        if (f[0] - l[0]).abs() < 1e-6 && (f[1] - l[1]).abs() < 1e-6 {
            out.pop();
        }
    }
    out
}

/// A frame for a circular cross-section perpendicular to `t`.
fn frame(t: Vec3) -> (Vec3, Vec3) {
    let up = if t.z.abs() < 0.99 { Vec3::Z } else { Vec3::X };
    let u = up.cross(t).normalize_or_zero();
    (u, t.cross(u))
}

fn ring_at(p: Vec3, u: Vec3, v: Vec3, r: f32, n: usize) -> Vec<Vec3> {
    (0..n)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / n as f32;
            p + r * (u * a.cos() + v * a.sin())
        })
        .collect()
}

/// A cap fan closing a ring, facing `n`.
fn push_cap(mesh: &mut SolidMesh, part: u32, c: Vec3, ring: &[Vec3], n: Vec3) {
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        let (a, b) = if (b - a).cross(c - a).dot(n) < 0.0 { (b, a) } else { (a, b) };
        push_tri(mesh, part, c.into(), a.into(), b.into(), n.into());
    }
}

/// A capped cylinder from `p0` to `p1`, smooth-shaded around its axis.
fn push_cyl(mesh: &mut SolidMesh, part: u32, p0: Vec3, p1: Vec3, r: f32, n: usize) {
    let d = p1 - p0;
    if d.length() < 1e-7 || r < 1e-7 {
        return;
    }
    let t = d.normalize();
    let (u, v) = frame(t);
    let r0 = ring_at(p0, u, v, r, n);
    let r1: Vec<Vec3> = r0.iter().map(|p| *p + d).collect();
    push_ring_strip(mesh, part, &r0, &r1, p0, p1);
    push_cap(mesh, part, p0, &r0, -t);
    push_cap(mesh, part, p1, &r1, t);
}

/// A quad strip between two rings, with normals radial from the ring centres (smooth along the tube).
fn push_ring_strip(mesh: &mut SolidMesh, part: u32, a: &[Vec3], b: &[Vec3], ca: Vec3, cb: Vec3) {
    let n = a.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (na, nb) = ((a[i] - ca).normalize_or_zero(), (a[j] - ca).normalize_or_zero());
        let (ma, mb) = ((b[i] - cb).normalize_or_zero(), (b[j] - cb).normalize_or_zero());
        for v in [a[i], a[j], b[j]] {
            mesh.positions.push(v.into());
        }
        for nn in [na, nb, mb] {
            mesh.normals.push(nn.into());
        }
        mesh.face_ids.push(part);
        for v in [a[i], b[j], b[i]] {
            mesh.positions.push(v.into());
        }
        for nn in [na, mb, ma] {
            mesh.normals.push(nn.into());
        }
        mesh.face_ids.push(part);
    }
}

/// A UV sphere with radial (smooth) normals — the leg hub.
fn push_sphere(mesh: &mut SolidMesh, part: u32, c: Vec3, r: f32, nu: usize, nv: usize) {
    let mut rings: Vec<Vec<Vec3>> = Vec::new();
    for j in 1..nv {
        let ph = std::f32::consts::PI * j as f32 / nv as f32;
        rings.push(
            (0..nu)
                .map(|i| {
                    let th = std::f32::consts::TAU * i as f32 / nu as f32;
                    c + r * Vec3::new(ph.sin() * th.cos(), ph.sin() * th.sin(), ph.cos())
                })
                .collect(),
        );
    }
    let (top, bot) = (c + Vec3::Z * r, c - Vec3::Z * r);
    for i in 0..nu {
        let j = (i + 1) % nu;
        for (a, b, d) in [(top, rings[0][i], rings[0][j]), (bot, rings[nv - 2][j], rings[nv - 2][i])] {
            for v in [a, b, d] {
                mesh.positions.push(v.into());
                mesh.normals.push((v - c).normalize_or_zero().into());
            }
            mesh.face_ids.push(part);
        }
    }
    // Lower ring FIRST: the rings run top-down (θ increasing about +z), so a strip advancing with
    // them advances along −z and comes out inside-out. Feeding them bottom-up puts the advance and
    // the ring's handedness back in agreement.
    for k in 0..rings.len() - 1 {
        let (a, b) = (rings[k + 1].clone(), rings[k].clone());
        push_ring_strip(mesh, part, &a, &b, c, c);
    }
}

/// A polyline with a circular fillet at each interior corner, swept with a circle using
/// parallel-transport frames (spec §B2: "enough — the paths are near-planar").
fn push_tube(mesh: &mut SolidMesh, part: u32, pts: &[Vec3], fillet: f32, r: f32, n: usize, seg: usize) {
    if pts.len() < 2 {
        return;
    }
    let mut path = vec![pts[0]];
    for i in 1..pts.len() - 1 {
        let (a, k, b) = (pts[i - 1], pts[i], pts[i + 1]);
        let (t1, t2) = ((k - a).normalize_or_zero(), (b - k).normalize_or_zero());
        let ang = t1.dot(t2).clamp(-1.0, 1.0).acos();
        if ang < 1e-3 {
            path.push(k);
            continue;
        }
        let d = (fillet * (ang / 2.0).tan()).min((k - a).length() * 0.5).min((b - k).length() * 0.5);
        let (p1, p2) = (k - t1 * d, k + t2 * d);
        path.push(p1);
        // Quadratic Bézier through the corner — the de Casteljau form, so the blend is tangent to
        // both legs at P1/P2 and no kink survives the join.
        for s in 1..seg {
            let tt = s as f32 / seg as f32;
            path.push(p1.lerp(k, tt).lerp(k.lerp(p2, tt), tt));
        }
        path.push(p2);
    }
    path.push(*pts.last().unwrap());

    let m = path.len();
    let tans: Vec<Vec3> = (0..m)
        .map(|i| (path[(i + 1).min(m - 1)] - path[i.saturating_sub(1)]).normalize_or_zero())
        .collect();
    let (mut u, _) = frame(tans[0]);
    let mut rings: Vec<Vec<Vec3>> = Vec::with_capacity(m);
    for i in 0..m {
        let t = tans[i];
        u = (u - t * u.dot(t)).normalize_or_zero(); // parallel transport
        let v = t.cross(u);
        rings.push(ring_at(path[i], u, v, r, n));
    }
    push_cap(mesh, part, path[0], &rings[0], -tans[0]);
    push_cap(mesh, part, path[m - 1], &rings[m - 1], tans[m - 1]);
    for i in 0..m - 1 {
        let (a, b) = (rings[i].clone(), rings[i + 1].clone());
        push_ring_strip(mesh, part, &a, &b, path[i], path[i + 1]);
    }
}

/// Decompose `[x0,x1] × [y0,y1]` **minus** a set of axis-aligned holes into disjoint sub-rectangles,
/// by sweeping x-slabs at every hole edge and splitting each slab on the holes that span it. Exact
/// for axis-aligned holes and needs no boolean — this is what cuts the grommet openings.
///
/// Holes are clipped to the outer rectangle; a hole that swallows it returns nothing.
fn rect_minus_holes(x0: f32, x1: f32, y0: f32, y1: f32, holes: &[[f32; 4]]) -> Vec<[f32; 4]> {
    const EPS: f32 = 1e-6;
    if x1 - x0 <= EPS || y1 - y0 <= EPS {
        return Vec::new();
    }
    let clipped: Vec<[f32; 4]> = holes
        .iter()
        .map(|h| [h[0].max(x0), h[1].min(x1), h[2].max(y0), h[3].min(y1)])
        .filter(|h| h[1] - h[0] > EPS && h[3] - h[2] > EPS)
        .collect();
    if clipped.is_empty() {
        return vec![[x0, x1, y0, y1]];
    }
    let mut xs = vec![x0, x1];
    for h in &clipped {
        xs.push(h[0]);
        xs.push(h[1]);
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs.dedup_by(|a, b| (*a - *b).abs() <= EPS);

    let mut out = Vec::new();
    for w in xs.windows(2) {
        let (xa, xb) = (w[0], w[1]);
        if xb - xa <= EPS {
            continue;
        }
        // Because every hole edge is a breakpoint, a hole either spans this slab fully or misses it.
        let mut bands: Vec<[f32; 2]> = clipped
            .iter()
            .filter(|h| h[0] <= xa + EPS && h[1] >= xb - EPS)
            .map(|h| [h[2], h[3]])
            .collect();
        if bands.is_empty() {
            out.push([xa, xb, y0, y1]);
            continue;
        }
        bands.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
        let mut merged: Vec<[f32; 2]> = Vec::new();
        for b in bands {
            match merged.last_mut() {
                Some(m) if b[0] <= m[1] + EPS => m[1] = m[1].max(b[1]),
                _ => merged.push(b),
            }
        }
        let mut cy = y0;
        for m in merged {
            if m[0] - cy > EPS {
                out.push([xa, xb, cy, m[0]]);
            }
            cy = cy.max(m[1]);
        }
        if y1 - cy > EPS {
            out.push([xa, xb, cy, y1]);
        }
    }
    out
}

// ============================ features ============================

/// The chamfered slab with one square opening per grommet. Caps are tiled by [`rect_minus_holes`];
/// the openings get four vertical walls each. Spec §B2 — the only subtractive geometry in the desk.
fn build_top(mesh: &mut SolidMesh, part: u32, l: f32, w: f32, h: f32, t: f32, holes: &[[f32; 4]]) {
    let under = h - t;
    let ch = EDGE_CH.min(t * 0.4);
    let (hx, hy) = (l / 2.0, w / 2.0);
    let (ix, iy) = (hx - ch, hy - ch);

    // caps
    for r in rect_minus_holes(-hx, hx, -hy, hy, holes) {
        push_quad(
            mesh,
            part,
            [r[0], r[2], h],
            [r[1], r[2], h],
            [r[1], r[3], h],
            [r[0], r[3], h],
            [0.0, 0.0, 1.0],
        );
    }
    for r in rect_minus_holes(-ix, ix, -iy, iy, holes) {
        push_quad(
            mesh,
            part,
            [r[0], r[2], under],
            [r[0], r[3], under],
            [r[1], r[3], under],
            [r[1], r[2], under],
            [0.0, 0.0, -1.0],
        );
    }
    // outer wall (full size, h → under+ch) then the chamfer band (→ the inset underside)
    let outer = [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)];
    let inner = [(-ix, -iy), (ix, -iy), (ix, iy), (-ix, iy)];
    for i in 0..4 {
        let j = (i + 1) % 4;
        let (a, b) = (outer[i], outer[j]);
        let n = Vec3::new(b.1 - a.1, a.0 - b.0, 0.0).normalize_or_zero();
        push_quad(mesh, part, [a.0, a.1, under + ch], [b.0, b.1, under + ch], [b.0, b.1, h], [a.0, a.1, h], n.into());
        let (c, d) = (inner[i], inner[j]);
        let e0 = Vec3::new(b.0 - a.0, b.1 - a.1, 0.0);
        let e1 = Vec3::new(c.0 - a.0, c.1 - a.1, -ch);
        let cn = e0.cross(e1).normalize_or_zero();
        push_quad(mesh, part, [a.0, a.1, under + ch], [c.0, c.1, under], [d.0, d.1, under], [b.0, b.1, under + ch], (-cn).into());
    }
    // Opening walls. The solid lies OUTSIDE the hole, so each wall's outward normal points INTO the
    // opening — and the winding has to run the other way round from the slab's outer perimeter.
    for hle in holes {
        let (a, b, c, d) = (hle[0], hle[1], hle[2], hle[3]);
        for (p, q, n) in [
            ([b, c], [a, c], [0.0, 1.0, 0.0]),
            ([a, d], [b, d], [0.0, -1.0, 0.0]),
            ([b, d], [b, c], [-1.0, 0.0, 0.0]),
            ([a, c], [a, d], [1.0, 0.0, 0.0]),
        ] {
            push_quad(mesh, part, [p[0], p[1], under], [q[0], q[1], under], [q[0], q[1], h], [p[0], p[1], h], n);
        }
    }
}

/// The screen: a rounded shell standing ON the top (spec §A1 — it does *not* pierce the slab), with a
/// dark trim ring and a proud fabric panel per face, plus the optional organiser band.
fn build_partition(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &DeskInput, pw: f32) {
    let (h, ph, pt) = (inp.height, inp.part_h, PART_T);
    let py = match inp.part_pos {
        PartPos::Center => 0.0,
        PartPos::Rear => inp.width / 2.0 - 0.080,
    };
    let cz = h + ph / 2.0;
    let pr = PART_R.min(ph * 0.45).min(pw * 0.45);

    let shell = alloc(mats, Material::White);
    // Profile is authored centred on the origin, then mapped to (x, z) with the extrusion along y —
    // so the polygon's own y is the desk's z. `cz` re-centres it above the top surface.
    let poly: Vec<[f32; 2]> = rrect(pw, ph, [0.0, 0.0, pr, pr], 10).iter().map(|p| [p[0], p[1] + cz]).collect();
    push_prism(mesh, shell, &poly, Plane::Xz, py - pt / 2.0, py + pt / 2.0);

    let ins = PART_INSET.min(pw * 0.25).min(ph * 0.25);
    let r_in = (pr - ins).max(0.02);
    let shift = |p: &[[f32; 2]]| -> Vec<[f32; 2]> { p.iter().map(|q| [q[0], q[1] + cz]).collect() };
    let pan = shift(&rrect(pw - 2.0 * ins, ph - 2.0 * ins, [r_in * 0.4, r_in * 0.4, r_in, r_in], 10));
    let trim = shift(&rrect(pw - 2.0 * ins + 0.006, ph - 2.0 * ins + 0.006, [r_in * 0.4, r_in * 0.4, r_in, r_in], 10));
    let trim_p = alloc(mats, Material::Dark);
    let fab_p = alloc(mats, Material::Fabric);
    for s in [-1.0f32, 1.0] {
        push_prism(mesh, trim_p, &trim, Plane::Xz, py + s * (pt / 2.0 + 0.001), py + s * (pt / 2.0 + 0.004));
        push_prism(mesh, fab_p, &pan, Plane::Xz, py + s * (pt / 2.0 + 0.002), py + s * (pt / 2.0 + 0.007));
    }
    if inp.part_band {
        let band = alloc(mats, Material::Alu);
        let bw = (pw - 0.14).min(0.90);
        if bw > 0.05 {
            let bz = h + 0.010 + BAND_H / 2.0;
            let poly: Vec<[f32; 2]> = rrect(bw, BAND_H, [0.01; 4], 4).iter().map(|p| [p[0], p[1] + bz]).collect();
            for s in [-1.0f32, 1.0] {
                push_prism(mesh, band, &poly, Plane::Xz, py + s * (pt / 2.0 + 0.007), py + s * (pt / 2.0 + 0.012));
            }
        }
    }
}

/// The Λ compass frame at end `s` (spec §B2). Returns the rail attachment `(x, z)`.
///
/// The stance is deliberately **wider than the top** — that is the product's signature, and per §A1
/// it only shows in a front ortho, never from the side.
fn build_legs(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &DeskInput, s: f32) -> (f32, f32) {
    let (l, w) = (inp.length, inp.width);
    let under = inp.height - inp.top_t;
    let xe = s * (l / 2.0 - LEG_INSET);
    let xf = s * (l / 2.0 - LEG_INSET + LEG_KICK_X);
    let hub_z = under - RAIL_SINK;
    let metal = alloc(mats, Material::Metal);
    let cap = alloc(mats, Material::Cap);
    for sy in [-1.0f32, 1.0] {
        let yb = sy * 0.033;
        let yf = sy * (w / 2.0 + LEG_STANCE) * 0.985;
        push_sphere(mesh, metal, Vec3::new(xe, yb, hub_z), HUB_R, 12, 8);
        push_tube(
            mesh,
            metal,
            &[Vec3::new(xe, yb, hub_z), Vec3::new(xf, yf, 0.115), Vec3::new(xf, yf, 0.010)],
            FILLET_R,
            LEG_R,
            12,
            8,
        );
        push_cyl(mesh, cap, Vec3::new(xf, yf, 0.0), Vec3::new(xf, yf, FOOT_H), LEG_R + 0.002, 12);
    }
    (xe, hub_z)
}

/// The drawer pedestal at end `s` (spec §B2). Every drawer front is its own part so a single drawer
/// can be selected and re-painted in the app. Returns the rail attachment `(x, z)`.
fn build_pedestal(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &DeskInput, s: f32) -> (f32, f32) {
    let (l, w) = (inp.length, inp.width);
    let under = inp.height - inp.top_t;
    let n = inp.ped_n.max(1) as usize;
    // 'end' opens on the outer x face (bench, like the reference); 'front' opens at −y.
    let end_face = inp.ped_face == PedFace::End;
    let x_out = s * (l / 2.0 - if end_face { 0.010 } else { 0.020 });
    let x_in = x_out - s * inp.ped_w;
    let (x0, x1) = (x_out.min(x_in), x_out.max(x_in));
    let y0 = -(w / 2.0 - 0.020);
    let y1 = if end_face { w / 2.0 - 0.020 } else { w / 2.0 - 0.060 };

    let dark = alloc(mats, Material::Dark);
    let white = alloc(mats, Material::White);
    let ins = 0.045;
    for fx in [x0 + ins, x1 - ins - 0.06] {
        for fy in [y0 + ins, y1 - ins - 0.06] {
            push_box(mesh, dark, [fx, fx + 0.06], [fy, fy + 0.06], [0.0, PLINTH_Z]);
        }
    }
    push_box(mesh, white, [x0, x1], [y0, y1], [PLINTH_Z, PLINTH_Z + 0.020]);
    let c0 = PLINTH_Z + 0.020;
    push_box(mesh, white, [x0, x1], [y0, y1], [c0, under - 0.002]);

    let rev = 0.003;
    let fh = (under - 0.002 - c0) / n as f32;
    for i in 0..n {
        let front = alloc(mats, Material::Oak);
        let handle = alloc(mats, Material::Dark);
        let z0 = c0 + i as f32 * fh + rev;
        let z1 = c0 + (i + 1) as f32 * fh - rev;
        let gz = z1 - 0.020;
        if end_face {
            let fx0 = if s > 0.0 { x1 } else { x0 - 0.004 };
            let fx1 = fx0 + 0.004;
            push_box(mesh, front, [fx0, fx1], [y0 + 0.004, y1 - 0.004], [z0, z1]);
            push_box(mesh, handle, [fx0 - 0.0005, fx1 + 0.0008], [y0 + 0.030, y1 - 0.030], [gz, z1 - 0.004]);
        } else {
            push_box(mesh, front, [x0 + 0.004, x1 - 0.004], [y0 - 0.004, y0], [z0, z1]);
            push_box(mesh, handle, [x0 + 0.030, x1 - 0.030], [y0 - 0.0048, y0 + 0.0005], [gz, z1 - 0.004]);
        }
    }
    // fingerprint lock on the top drawer
    let lock = alloc(mats, Material::Dark);
    let zc = c0 + n as f32 * fh - 0.045;
    if end_face {
        let fx = if s > 0.0 { x1 + 0.004 } else { x0 - 0.004 };
        push_cyl(mesh, lock, Vec3::new(fx, y0 + 0.050, zc), Vec3::new(fx + s * 0.002, y0 + 0.050, zc), 0.009, 12);
    } else {
        push_cyl(mesh, lock, Vec3::new(x1 - 0.050, y0 - 0.004, zc), Vec3::new(x1 - 0.050, y0 - 0.006, zc), 0.009, 12);
    }
    (x_in, under - RAIL_SINK)
}

/// A plain end panel (spec §B2). Returns the rail attachment `(x, z)`.
fn build_panel(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &DeskInput, s: f32) -> (f32, f32) {
    let (l, w) = (inp.length, inp.width);
    let under = inp.height - inp.top_t;
    let xe = s * (l / 2.0 - 0.020);
    let white = alloc(mats, Material::White);
    push_box(mesh, white, [xe, xe - s * 0.025], [-(w / 2.0 - 0.050), w / 2.0 - 0.050], [0.008, under - 0.001]);
    (xe - s * 0.025, under - RAIL_SINK)
}

// ============================ build ============================

/// Build the whole desk. Returns the achieved metrics (with any validation warnings — spec §B3
/// clamps and warns rather than blocking), the mesh with per-component `face_ids`, and the material
/// per part id.
pub fn build(inp: &DeskInput) -> Result<(DeskMetrics, SolidMesh, Vec<Material>), String> {
    let (l, w, h, t) = (inp.length, inp.width, inp.height, inp.top_t);
    for (name, v) in [("length", l), ("depth", w), ("height", h), ("top thickness", t)] {
        if !(v > 0.0) || !v.is_finite() {
            return Err(format!("{name} must be greater than 0"));
        }
    }
    if t >= h {
        return Err(format!("top thickness {:.0} mm cannot reach the {:.0} mm surface height", t * 1000.0, h * 1000.0));
    }

    let mut warnings: Vec<String> = Vec::new();

    // §B3 — validation fires while editing: clamp what can be clamped, warn about the rest.
    let mut pw = inp.part_w;
    if inp.partition {
        if pw > l - 0.10 {
            warnings.push(format!("screen span {:.0} mm clamped to the top length − 100 mm", pw * 1000.0));
            pw = l - 0.10;
        }
        if pw <= 0.05 {
            return Err("screen span is too small — widen the screen or the top".into());
        }
        if inp.part_h <= 0.02 {
            return Err("screen height must be at least 20 mm above the top".into());
        }
        if inp.part_pos == PartPos::Center && w < 1.10 {
            warnings.push(format!("a centre screen wants a depth ≥ 1100 mm for two users face to face (this is {:.0} mm)", w * 1000.0));
        }
    }
    if !(0.66..=0.82).contains(&h) {
        warnings.push(format!("top surface {:.0} mm is outside the ergonomic 660–820 mm band", h * 1000.0));
    }
    let peds = [inp.sup_l, inp.sup_r].iter().filter(|s| **s == Support::Drawers).count();
    if peds > 0 {
        if inp.ped_w <= 0.10 || inp.ped_w >= l / 2.0 {
            return Err(format!(
                "pedestal width {:.0} mm does not fit a {:.0} mm top — it must be between 100 mm and half the length",
                inp.ped_w * 1000.0,
                l * 1000.0
            ));
        }
        if peds == 2 && 2.0 * inp.ped_w > l - 0.45 {
            warnings.push(format!("two pedestals leave {:.0} mm of knee space — under 450 mm", (l - 2.0 * inp.ped_w) * 1000.0));
        }
    }
    let front_h = if peds > 0 { (h - t - 0.060) / inp.ped_n.max(1) as f32 } else { 0.0 };
    if peds > 0 && front_h < 0.130 {
        warnings.push(format!("drawer fronts are {:.0} mm tall — under 130 mm is unusable", front_h * 1000.0));
    }
    match (inp.sup_l, inp.sup_r) {
        (Support::None, Support::None) => warnings.push("both supports deleted — the top has nothing to stand on".into()),
        (Support::None, _) | (_, Support::None) => {
            warnings.push("one support deleted — the top cantilevers from the other end".into())
        }
        _ => {}
    }

    // Grommet openings, in top-plate coordinates. A port that would breach the edge is refused
    // rather than silently clipped — a half-cut hole is a hole in the desk.
    let mut holes: Vec<[f32; 4]> = Vec::new();
    let margin = EDGE_CH + 0.010;
    for (i, g) in inp.grommets.iter().enumerate() {
        let (gx, gy) = g.xy(w);
        let hle = [gx - GR_L / 2.0, gx + GR_L / 2.0, gy - GR_D / 2.0, gy + GR_D / 2.0];
        if hle[0] < -l / 2.0 + margin || hle[1] > l / 2.0 - margin || hle[2] < -w / 2.0 + margin || hle[3] > w / 2.0 - margin {
            return Err(format!(
                "cable port {} at ({:.0}, {:.0}) mm breaks the edge of the top — keep it {:.0} mm clear",
                i + 1,
                gx * 1000.0,
                gy * 1000.0,
                margin * 1000.0
            ));
        }
        holes.push(hle);
    }

    let mut mesh = SolidMesh::default();
    let mut mats: Vec<Material> = Vec::new();
    let mut features: Vec<String> = Vec::new();

    let top = alloc(&mut mats, Material::White);
    build_top(&mut mesh, top, l, w, h, t, &holes);
    features.push("top".into());

    if inp.partition {
        build_partition(&mut mesh, &mut mats, inp, pw);
        features.push("partition".into());
    }

    let mut rail_ends: Vec<(f32, f32)> = Vec::new();
    let mut leg_stance = 0.0f32;
    for (side, kind, s) in [("support_L", inp.sup_l, -1.0f32), ("support_R", inp.sup_r, 1.0)] {
        let end = match kind {
            Support::Legs => {
                leg_stance = 2.0 * ((w / 2.0 + LEG_STANCE) * 0.985 + LEG_R + 0.002);
                features.push(format!("{side} (legs)"));
                build_legs(&mut mesh, &mut mats, inp, s)
            }
            Support::Drawers => {
                features.push(format!("{side} (drawers)"));
                build_pedestal(&mut mesh, &mut mats, inp, s)
            }
            Support::Panel => {
                features.push(format!("{side} (panel)"));
                build_panel(&mut mesh, &mut mats, inp, s)
            }
            Support::None => (s * (l / 2.0 - LEG_INSET), h - t - RAIL_SINK),
        };
        rail_ends.push(end);
    }

    if inp.rail {
        let rail = alloc(&mut mats, Material::Metal);
        let ((xa, za), (xb, zb)) = (rail_ends[0], rail_ends[1]);
        push_cyl(&mut mesh, rail, Vec3::new(xa, 0.0, za), Vec3::new(xb, 0.0, zb), RAIL_R, 12);
        features.push("rail".into());
    }

    if !inp.grommets.is_empty() {
        let collar = alloc(&mut mats, Material::Alu);
        let liner = alloc(&mut mats, Material::Dark);
        for g in &inp.grommets {
            let (gx, gy) = g.xy(w);
            let shift = |p: Vec<[f32; 2]>| -> Vec<[f32; 2]> { p.into_iter().map(|q| [q[0] + gx, q[1] + gy]).collect() };
            let ring = shift(rrect(GR_L + 0.012, GR_D + 0.012, [(GR_D + 0.012) / 2.0; 4], 8));
            push_prism(&mut mesh, collar, &ring, Plane::Xy, h - 0.004, h + 0.003);
            let sleeve = shift(rrect(GR_L - 0.006, GR_D - 0.006, [(GR_D - 0.006) / 2.0; 4], 8));
            push_prism(&mut mesh, liner, &sleeve, Plane::Xy, h - 0.030, h - 0.006);
        }
        features.push(format!("grommets ×{}", inp.grommets.len()));
    }

    let metrics = DeskMetrics {
        length: l,
        width: w,
        height: h,
        partition_top: if inp.partition { h + inp.part_h } else { 0.0 },
        leg_stance,
        drawer_front_h: front_h,
        grommets: inp.grommets.len(),
        features,
        tris: mesh.tri_count(),
        warnings,
    };
    Ok((metrics, mesh, mats))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(mesh: &SolidMesh) -> ([f32; 3], [f32; 3]) {
        mesh.bounds().expect("mesh is empty")
    }

    /// Summed area of the up-facing triangles that lie exactly on the top surface — the honest way
    /// to prove the grommet openings are really cut, with no boolean anywhere in the build.
    fn top_surface_area(mesh: &SolidMesh, h: f32) -> f32 {
        let mut a = 0.0;
        for t in 0..mesh.tri_count() {
            let p: Vec<Vec3> = (0..3).map(|i| Vec3::from(mesh.positions[t * 3 + i])).collect();
            if p.iter().any(|v| (v.z - h).abs() > 1e-5) || mesh.normals[t * 3][2] < 0.99 {
                continue;
            }
            a += (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
        }
        a
    }

    #[test]
    fn default_desk_builds_and_is_well_formed() {
        let (m, mesh, mats) = build(&DeskInput::default()).unwrap();
        assert!(mesh.tri_count() > 500, "only {} tris", mesh.tri_count());
        assert_eq!(mesh.face_ids.len(), mesh.tri_count());
        assert_eq!(mesh.positions.len(), mesh.normals.len());
        assert!((*mesh.face_ids.iter().max().unwrap() as usize) < mats.len());
        for p in &mesh.positions {
            assert!(p.iter().all(|v| v.is_finite()), "non-finite vertex {p:?}");
        }
        let (lo, hi) = bbox(&mesh);
        assert!((hi[0] - lo[0] - m.length).abs() < 0.02, "length {}", hi[0] - lo[0]);
        assert!(lo[2].abs() < 1e-4, "desk floats: z0 = {}", lo[2]);
        // The screen top is the highest thing in the model.
        assert!((hi[2] - m.partition_top).abs() < 0.02, "screen top {} vs bbox {}", m.partition_top, hi[2]);
        assert!(m.warnings.is_empty(), "unexpected warnings: {:?}", m.warnings);
    }

    /// §A4 — the `bench` preset reproduces the reference FBX. Same tolerances the generator prints.
    #[test]
    fn bench_preset_matches_the_reference_fbx() {
        let inp = Preset::Bench.input();
        let (m, mesh, _) = build(&inp).unwrap();
        let (lo, hi) = bbox(&mesh);
        assert!((m.length - 1.600).abs() < 0.001);
        assert!((m.width - 1.240).abs() < 0.001);
        assert!((m.height - 0.780).abs() < 0.001);
        assert!((m.partition_top - 1.028).abs() < 0.005, "screen top {}", m.partition_top);
        // The Λ stance is WIDER than the top (§A1) — that is the whole point of the compass leg.
        // NB the reference generator *reports* 1.350 here but *builds* 1.330: its printed formula
        // omits the 0.985 the feet are actually pulled in by. We port the built geometry and report
        // what was built, so this sits ~2% under the reference's 1.359 rather than the quoted 0.7%.
        assert!((m.leg_stance - 1.359).abs() < 0.04, "stance {}", m.leg_stance);
        assert!(m.leg_stance > m.width, "stance {} should exceed depth {}", m.leg_stance, m.width);
        assert!((hi[1] - lo[1] - m.leg_stance).abs() < 0.02, "bbox depth {} vs stance {}", hi[1] - lo[1], m.leg_stance);
    }

    /// The grommet openings are genuinely missing from the slab, not merely covered by the collar.
    #[test]
    fn grommet_openings_are_cut_out_of_the_top() {
        let mut inp = Preset::Minimal.input();
        inp.grommets.clear();
        let (_, solid, _) = build(&inp).unwrap();
        let full = top_surface_area(&solid, inp.height);
        assert!((full - inp.length * inp.width).abs() < 1e-4, "uncut top area {full}");

        inp.grommets = vec![Grommet::rear_at(-0.3), Grommet::rear_at(0.3)];
        let (m, holed, _) = build(&inp).unwrap();
        let cut = top_surface_area(&holed, inp.height);
        let expect = full - 2.0 * GR_L * GR_D;
        assert!((cut - expect).abs() < 1e-4, "cut area {cut}, expected {expect}");
        assert_eq!(m.grommets, 2);
        // Deleting the grommets takes their holes with them (§B0).
        inp.grommets.clear();
        let (_, back, _) = build(&inp).unwrap();
        assert!((top_surface_area(&back, inp.height) - full).abs() < 1e-4);
    }

    #[test]
    fn overlapping_and_edge_holes_are_handled() {
        // Two ports close enough to merge into one opening — the slab decomposition must not emit
        // overlapping cap rectangles (which would show as a doubled, z-fighting surface).
        let mut inp = Preset::Minimal.input();
        inp.grommets = vec![Grommet::rear_at(0.0), Grommet::rear_at(0.05)];
        let (_, mesh, _) = build(&inp).unwrap();
        let cut = top_surface_area(&mesh, inp.height);
        let union = (GR_L + 0.05) * GR_D; // the two lozenges overlap in x
        assert!((cut - (inp.length * inp.width - union)).abs() < 1e-4, "area {cut}");
        // A port hanging off the edge is refused outright.
        inp.grommets = vec![Grommet { x: inp.length / 2.0, y: 0.0, rear: false }];
        assert!(build(&inp).is_err());
    }

    #[test]
    fn rect_minus_holes_tiles_without_overlap() {
        let rs = rect_minus_holes(-1.0, 1.0, -0.5, 0.5, &[[-0.2, 0.2, -0.1, 0.1], [0.5, 0.7, 0.0, 0.2]]);
        let area: f32 = rs.iter().map(|r| (r[1] - r[0]) * (r[3] - r[2])).sum();
        assert!((area - (2.0 * 1.0 - 0.4 * 0.2 - 0.2 * 0.2)).abs() < 1e-5, "area {area}");
        for (i, a) in rs.iter().enumerate() {
            for b in rs.iter().skip(i + 1) {
                let ox = a[1].min(b[1]) - a[0].max(b[0]);
                let oy = a[3].min(b[3]) - a[2].max(b[2]);
                assert!(ox <= 1e-6 || oy <= 1e-6, "{a:?} overlaps {b:?}");
            }
        }
        // A hole that swallows the rectangle leaves nothing behind.
        assert!(rect_minus_holes(0.0, 1.0, 0.0, 1.0, &[[-1.0, 2.0, -1.0, 2.0]]).is_empty());
    }

    /// §B0/§B3 — every feature is deletable, and a missing support warns instead of blocking.
    #[test]
    fn features_detach_cleanly() {
        let full = build(&Preset::Combo.input()).unwrap();
        let mut inp = Preset::Combo.input();
        inp.partition = false;
        inp.rail = false;
        inp.sup_r = Support::None;
        inp.grommets.clear();
        let (m, mesh, mats) = build(&inp).unwrap();
        assert!(mesh.tri_count() < full.1.tri_count(), "stripping features should shrink the mesh");
        assert!(m.features.iter().any(|f| f == "top"), "the top survived: {:?}", m.features);
        assert!(!m.features.iter().any(|f| f.contains("partition") || f.contains("rail") || f.contains("grommet")));
        assert!(m.warnings.iter().any(|w| w.contains("cantilever")), "{:?}", m.warnings);
        // Still a valid, paintable mesh after the deletions.
        assert_eq!(mesh.face_ids.len(), mesh.tri_count());
        assert!((*mesh.face_ids.iter().max().unwrap() as usize) < mats.len());
    }

    /// Each drawer front is its own part, so one drawer can be selected and re-painted.
    #[test]
    fn every_drawer_front_is_a_separate_part() {
        let mut inp = Preset::Combo.input();
        inp.ped_n = 4;
        let (_, mesh, mats) = build(&inp).unwrap();
        assert_eq!(mats.iter().filter(|m| **m == Material::Oak).count(), 4);
        let oak_tris = (0..mesh.tri_count()).filter(|t| mats[mesh.face_ids[*t] as usize] == Material::Oak).count();
        assert!(oak_tris >= 4 * 12, "{oak_tris} oak triangles for 4 fronts");
    }

    /// `face = end` puts the fronts on the outer x face (the reference bench); `front` at −y.
    #[test]
    fn pedestal_face_controls_which_way_the_drawers_open() {
        let extent = |inp: &DeskInput| -> ([f32; 3], [f32; 3]) {
            let (_, mesh, mats) = build(inp).unwrap();
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for t in 0..mesh.tri_count() {
                if mats[mesh.face_ids[t] as usize] != Material::Oak {
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
        };
        let mut inp = Preset::Combo.input();
        inp.ped_face = PedFace::End;
        let (_, hi_end) = extent(&inp);
        assert!(hi_end[0] > inp.length / 2.0 - 0.011, "end fronts should reach the outer x face: {}", hi_end[0]);
        inp.ped_face = PedFace::Front;
        let (lo_front, _) = extent(&inp);
        assert!(lo_front[1] < -(inp.width / 2.0 - 0.021), "front fronts should sit proud at −y: {}", lo_front[1]);
    }

    #[test]
    fn validation_clamps_and_warns_per_spec_b3() {
        // screen wider than the top → clamped, not rejected
        let mut inp = Preset::Single.input();
        inp.part_w = 5.0;
        let (m, _, _) = build(&inp).unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("clamped")), "{:?}", m.warnings);
        // two pedestals with no knee space
        let mut inp = Preset::Exec.input();
        inp.ped_w = 0.80;
        let (m, _, _) = build(&inp).unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("knee space")), "{:?}", m.warnings);
        // ergonomics + unusable drawer fronts
        let mut inp = Preset::Combo.input();
        inp.height = 0.55;
        inp.ped_n = 4;
        let (m, _, _) = build(&inp).unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("ergonomic")), "{:?}", m.warnings);
        assert!(m.warnings.iter().any(|w| w.contains("unusable")), "{:?}", m.warnings);
        // a centre screen on a shallow top
        let mut inp = Preset::Single.input();
        inp.part_pos = PartPos::Center;
        let (m, _, _) = build(&inp).unwrap();
        assert!(m.warnings.iter().any(|w| w.contains("face to face")), "{:?}", m.warnings);
        // hard errors
        assert!(build(&DeskInput { length: 0.0, ..Preset::Combo.input() }).is_err());
        assert!(build(&DeskInput { top_t: 1.0, ..Preset::Combo.input() }).is_err());
        assert!(build(&DeskInput { ped_w: 1.9, ..Preset::Combo.input() }).is_err());
    }

    /// §B4 — rebuild all five presets; a regression in shared code shows up in whichever uses it.
    #[test]
    fn all_presets_build() {
        for p in Preset::ALL {
            let inp = p.input();
            let (m, mesh, mats) = build(&inp).unwrap_or_else(|e| panic!("{}: {e}", p.label()));
            assert!(mesh.tri_count() > 200, "{}: {} tris", p.label(), mesh.tri_count());
            assert!(mesh.tri_count() < 20_000, "{}: {} tris is far past the reference budget", p.label(), mesh.tri_count());
            assert_eq!(mesh.face_ids.len(), mesh.tri_count(), "{}", p.label());
            assert!((*mesh.face_ids.iter().max().unwrap() as usize) < mats.len(), "{}", p.label());
            let (lo, hi) = bbox(&mesh);
            assert!(lo[2].abs() < 1e-4 && hi[2] >= m.height - 1e-4, "{}: z {}..{}", p.label(), lo[2], hi[2]);
            for n in &mesh.normals {
                let len = Vec3::from(*n).length();
                assert!((len - 1.0).abs() < 1e-3, "{}: normal length {len}", p.label());
            }
        }
    }

    /// Every triangle's WINDING must agree with the normal it carries. A face wound the wrong way
    /// round is invisible under backface culling and lit inside-out without it — and it is exactly
    /// the mistake that hides in a hand-built solid, because the shading normal still looks right in
    /// the data. Smooth-shaded parts (hubs, tubes) are covered too: on a 12-gon the face normal is
    /// within 15° of its vertex normals, nowhere near the sign flip this catches.
    #[test]
    fn winding_agrees_with_every_normal() {
        for p in Preset::ALL {
            let (_, mesh, _) = build(&p.input()).unwrap();
            for t in 0..mesh.tri_count() {
                let v: Vec<Vec3> = (0..3).map(|i| Vec3::from(mesh.positions[t * 3 + i])).collect();
                let face = (v[1] - v[0]).cross(v[2] - v[0]);
                if face.length() < 1e-9 {
                    panic!("{}: degenerate triangle {t} at {:?}", p.label(), v[0]);
                }
                let shade: Vec3 = (0..3).map(|i| Vec3::from(mesh.normals[t * 3 + i])).sum();
                assert!(
                    face.normalize().dot(shade.normalize()) > 0.2,
                    "{}: triangle {t} is wound against its normal (dot {:.3}) at {:?}",
                    p.label(),
                    face.normalize().dot(shade.normalize()),
                    v[0],
                );
            }
        }
    }

    /// The screen stands ON the top — it does not pierce the slab (§A1's forensic win).
    #[test]
    fn the_screen_does_not_pierce_the_top() {
        let inp = Preset::Combo.input();
        let (_, mesh, mats) = build(&inp).unwrap();
        let mut lowest = f32::MAX;
        for t in 0..mesh.tri_count() {
            if !matches!(mats[mesh.face_ids[t] as usize], Material::Fabric) {
                continue;
            }
            for i in 0..3 {
                lowest = lowest.min(mesh.positions[t * 3 + i][2]);
            }
        }
        assert!(lowest >= inp.height - 1e-4, "screen panel dips to {lowest}, below the {} surface", inp.height);
    }
}
