//! Curved office luminaires — a lighting PROFILE swept along a PATH.
//!
//! Port of `SWEEPLIGHT_BUILD.md` (G:\blender dev\staircase\office lights): one 2D closed profile
//! polygon × one 3D path = the fixture — body, lens and suspension. The engineering is the sweep:
//! - **centripetal Catmull-Rom** (α = 0.5) for user splines — uniform CR overshoots on unevenly
//!   spaced points and produces cusps (the md's worst shipped-almost bug);
//! - **arc-length resampling** first — droppers, UVs and curvature all assume uniform spacing;
//! - **rotation-minimising frames by double reflection** (Wang et al. 2008) — Frenet frames are
//!   undefined on straights and flip 180° at inflections, and a serpentine is nothing but
//!   inflections;
//! - **twist closure** on closed paths (the transported frame doesn't return to itself; the
//!   residual roll is distributed linearly or the seam shows and the lens walks onto another face);
//! - **levelling with a fade** — the profile stays level like a luminaire, not banked like a
//!   rollercoaster; the correction fades out near vertical tangents instead of branching;
//! - **self-intersection validation** — a swept profile folds through itself where the path's
//!   curvature radius drops below the profile's inner reach; reported as an actionable number;
//! - **one independent quad strip per profile edge** — duplicated corner vertices keep the 3 mm
//!   chamfer arrises HARD across the profile and smooth along the sweep (a shared-vertex tube
//!   rounds every arris off and reads as plastic, not extruded aluminium).
//!
//! Conventions (md B9): metres; the CEILING is z = 0 and the fixture hangs at negative z;
//! `drop` = ceiling to the TOP of the profile (v = 0 is the top of the profile, the plane the
//! droppers meet). The app re-seats furniture to base-at-0 on import; metrics carry the height.

use crate::SolidMesh;
use glam::Vec3;

/// Per-part material tags, indexed by the mesh's `face_ids`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    /// Dark anodised extrusion.
    Body,
    /// The diffuser face — the part that glows.
    Lens,
    /// Rods + ceiling roses.
    Rod,
}

/// Path kinds. Every path carries its own drop(s); the sweep interpolates by arc length.
#[derive(Clone, Debug, PartialEq)]
pub enum PathKind {
    /// A closed circle of `radius` (plan), level at the fixture drop.
    Ring { radius: f32 },
    /// A closed rounded rectangle `w × d` (plan) with corner `fillet`, level.
    Racetrack { w: f32, d: f32, fillet: f32 },
    /// An OPEN 5-point centripetal spline shaped as a gentle S: `length` long, swinging `±width`,
    /// descending from the fixture drop to `drop_end` (equal drops = level).
    SCurve { length: f32, width: f32 },
    /// A USER-DRAWN plan curve (from the 2D drawing: polyline / arc / circle / ellipse sampling).
    /// Corners are filleted at `fillet` (clamped to what the adjacent segments allow — md B6),
    /// the path is re-centred on its plan centroid (md B9), and a closed path is re-oriented
    /// CLOCKWISE so the profile's +u faces outward.
    Custom { pts: Vec<glam::Vec2>, closed: bool, fillet: f32 },
}

/// Which face carries the lens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileKind {
    /// Lens on the INNER face (the md's reference product).
    RingInner,
    /// Lens on the bottom face.
    Downlight,
    /// Round tube; the lower arc is the lens.
    Round,
    /// Thin and deep; lens on the bottom.
    Blade,
}

impl ProfileKind {
    pub fn label(self) -> &'static str {
        match self {
            ProfileKind::RingInner => "Ring (lens inner)",
            ProfileKind::Downlight => "Downlight (lens under)",
            ProfileKind::Round => "Round tube",
            ProfileKind::Blade => "Blade",
        }
    }
    pub const ALL: [ProfileKind; 4] = [ProfileKind::RingInner, ProfileKind::Downlight, ProfileKind::Round, ProfileKind::Blade];
}

/// The three controls (md C9) plus the profile dimensions.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepInput {
    pub path: PathKind,
    pub profile: ProfileKind,
    /// Profile width (across the path), metres. Reference: 0.055.
    pub width: f32,
    /// Profile height, metres. Reference: 0.090.
    pub height: f32,
    /// Lens height on its face, metres. Reference: 0.055.
    pub lens: f32,
    /// Ceiling to the TOP of the profile, metres.
    pub drop: f32,
    /// Open paths: drop at the far end (level when equal to `drop`). Ignored for closed paths.
    pub drop_end: f32,
    /// Nominal distance between hanging points, metres.
    pub spacing: f32,

    // ---- PHOTOMETRY — what makes this a light rather than a glowing shape --------------------
    //
    // Until now a curved luminaire was FURNITURE with an emissive lens: it looked lit and
    // contributed nothing to any calculation. A linear fitting is specified by the two numbers
    // below, so they are what the dialog asks for.
    /// Connected load per metre of run, W/m. A typical architectural LED profile is 8–20.
    pub watts_per_m: f32,
    /// Luminous efficacy, lm/W — total flux is `path_len × watts_per_m × efficacy`.
    pub efficacy_lm_per_w: f32,
    /// Correlated colour temperature, kelvin.
    ///
    /// RECORDED AND REPORTED, NOT CALCULATED WITH. The engine is photometric, not spectral: it
    /// carries lux and candela, which are already weighted by the eye's luminous efficiency
    /// function, so 3000 K and 4000 K at the same lumens produce the same lux. CCT changes how a
    /// room LOOKS and belongs on the schedule; it does not change the numbers, and treating it as
    /// if it did would be inventing physics this engine does not do.
    pub cct_k: u32,
}

impl Default for SweepInput {
    fn default() -> Self {
        Self {
            path: PathKind::Ring { radius: 0.4 },
            profile: ProfileKind::RingInner,
            width: 0.055,
            height: 0.090,
            lens: 0.055,
            drop: 0.6,
            drop_end: 0.6,
            spacing: 1.0,
            // A mid-range architectural LED profile: 12 W/m at 110 lm/W is about 1300 lm/m, and
            // 3000 K is the usual specification for interiors.
            watts_per_m: 12.0,
            efficacy_lm_per_w: 110.0,
            cct_k: 3000,
        }
    }
}

/// What the build actually produced — achieved numbers next to requested ones (md C8).
#[derive(Clone, Debug)]
pub struct SweepMetrics {
    pub path_len: f32,
    pub droppers: usize,
    pub achieved_spacing: f32,
    /// Smallest curvature radius found on the path.
    pub min_radius: f32,
    pub tris: usize,
    /// Total height of the fixture (ceiling to lowest point).
    pub total_drop: f32,
    pub warnings: Vec<String>,
}

/// Arc-length resample step, metres (md B8: right for architectural work).
const STEP: f32 = 0.02;
/// Chamfer on each arris (md B4: geometry, not shading).
const CHAMFER: f32 = 0.003;
/// Rod sink into the profile (md: a flush cap z-fights with the top strip).
const ROD_SINK: f32 = 0.014;
const ROD_R: f32 = 0.004;
const ROSE_R: f32 = 0.035;
const ROSE_H: f32 = 0.012;
/// Open-path end margin for the end-pinned hangers (md C8).
const END_MARGIN: f32 = 0.09;

// ============================ profile ============================

/// A closed CCW profile polygon with a material tag per EDGE. `+u` is OUTWARD (away from the
/// centre of a turn), `v` up, `v = 0` at the TOP of the profile (md B2).
struct Profile {
    pts: Vec<[f32; 2]>,
    mats: Vec<Material>,
}

impl Profile {
    /// How far the profile reaches on the INSIDE (−u) — the self-intersection limit driver.
    fn inner_reach(&self) -> f32 {
        self.pts.iter().map(|p| -p[0]).fold(0.0, f32::max)
    }
    fn min_v(&self) -> f32 {
        self.pts.iter().map(|p| p[1]).fold(0.0, f32::min)
    }
}

/// Build the tagged profile polygon (md B2/B4): a chamfered rectangle with the lens edge on the
/// chosen face, or a 12-gon tube for `Round`.
fn make_profile(kind: ProfileKind, width: f32, height: f32, lens: f32) -> Profile {
    let c = CHAMFER.min(width * 0.2).min(height * 0.2);
    let (w, h) = (width.max(0.01), height.max(0.01));
    let (uo, ui) = (w * 0.5, -w * 0.5); // outer / inner u
    match kind {
        ProfileKind::Round => {
            // 12-gon of diameter `w`, centred u=0, top at v=0. Lower-third edges are the lens.
            let n = 12usize;
            let r = w * 0.5;
            let mut pts = Vec::with_capacity(n);
            let mut mats = Vec::with_capacity(n);
            for k in 0..n {
                // CCW in (u, v), starting at the top.
                let a = std::f32::consts::TAU * (k as f32 / n as f32) + std::f32::consts::FRAC_PI_2;
                pts.push([r * a.cos(), -r + r * a.sin()]);
            }
            for k in 0..n {
                let mid_v = (pts[k][1] + pts[(k + 1) % n][1]) * 0.5;
                mats.push(if mid_v < -1.55 * r { Material::Lens } else { Material::Body });
            }
            Profile { pts, mats }
        }
        _ => {
            let lens_on_bottom = matches!(kind, ProfileKind::Downlight | ProfileKind::Blade);
            let lens_h = lens.clamp(0.005, h - 2.0 * c - 0.002);
            let g = ((h - lens_h) * 0.5).max(c); // top gap on the inner face
            let mut pts: Vec<[f32; 2]> = Vec::new();
            let mut mats: Vec<Material> = Vec::new();
            let mut push = |p: [f32; 2], m: Material| {
                pts.push(p);
                mats.push(m); // material of the edge STARTING at this point
            };
            if lens_on_bottom {
                let lw = lens.clamp(0.005, w - 2.0 * c - 0.002);
                let (l0, l1) = (-lw * 0.5, lw * 0.5);
                // CCW from top-outer: top → inner face → bottom (with lens span) → outer face.
                push([uo - c, 0.0], Material::Body); // top edge →
                push([ui + c, 0.0], Material::Body); // chamfer
                push([ui, -c], Material::Body); // inner face
                push([ui, -h + c], Material::Body); // chamfer
                push([ui + c, -h], Material::Body); // bottom: body strip to the lens
                push([l0, -h], Material::Lens); // LENS
                push([l1, -h], Material::Body); // bottom: body strip to the chamfer
                push([uo - c, -h], Material::Body); // chamfer
                push([uo, -h + c], Material::Body); // outer face
                push([uo, -c], Material::Body); // chamfer back to start
            } else {
                // Lens on the INNER face (−u), vertically centred.
                push([uo - c, 0.0], Material::Body); // top edge →
                push([ui + c, 0.0], Material::Body); // chamfer
                push([ui, -c], Material::Body); // inner face upper
                push([ui, -g], Material::Lens); // LENS
                push([ui, -g - lens_h], Material::Body); // inner face lower
                push([ui, -h + c], Material::Body); // chamfer
                push([ui + c, -h], Material::Body); // bottom
                push([uo - c, -h], Material::Body); // chamfer
                push([uo, -h + c], Material::Body); // outer face
                push([uo, -c], Material::Body); // chamfer back to start
            }
            Profile { pts, mats }
        }
    }
}

// ============================ path sampling ============================

/// Centripetal Catmull-Rom (α = 0.5) through `ctrl`, `samples` per segment (md C3). Open curve;
/// end segments use doubled endpoints.
fn catmull_rom(ctrl: &[Vec3], samples: usize) -> Vec<Vec3> {
    let n = ctrl.len();
    if n < 2 {
        return ctrl.to_vec();
    }
    let get = |i: i64| ctrl[i.clamp(0, n as i64 - 1) as usize];
    let mut out = Vec::new();
    for seg in 0..n - 1 {
        let (p0, p1, p2, p3) = (get(seg as i64 - 1), get(seg as i64), get(seg as i64 + 1), get(seg as i64 + 2));
        let a = 0.5f32;
        let t01 = p0.distance(p1).max(1e-6).powf(a);
        let t12 = p1.distance(p2).max(1e-6).powf(a);
        let t23 = p2.distance(p3).max(1e-6).powf(a);
        let m1 = (p2 - p1) + ((p1 - p0) / t01 - (p2 - p0) / (t01 + t12)) * t12;
        let m2 = (p2 - p1) + ((p3 - p2) / t23 - (p3 - p1) / (t12 + t23)) * t12;
        let last = seg == n - 2;
        let count = if last { samples + 1 } else { samples };
        for k in 0..count {
            let s = k as f32 / samples as f32;
            let (s2, s3) = (s * s, s * s * s);
            out.push(
                p1 * (2.0 * s3 - 3.0 * s2 + 1.0)
                    + m1 * (s3 - 2.0 * s2 + s)
                    + p2 * (-2.0 * s3 + 3.0 * s2)
                    + m2 * (s3 - s2),
            );
        }
    }
    out
}

/// Sample the path into raw 3D points (z = −drop along it) + closed flag.
fn sample_path(inp: &SweepInput) -> (Vec<Vec3>, bool) {
    match inp.path.clone() {
        PathKind::Ring { radius } => {
            // CLOCKWISE in plan: with the up (world-Z) bitangent of a levelled frame, CW travel
            // puts the frame normal N = B×T OUTWARD — the md's "+u is outward" convention. (A CCW
            // path with an up bitangent necessarily has N inward, which mirrors the profile.)
            let r = radius.max(0.02);
            let n = ((std::f32::consts::TAU * r / STEP).round() as usize).max(24);
            let pts = (0..n)
                .map(|k| {
                    let a = -(std::f32::consts::TAU * k as f32 / n as f32);
                    Vec3::new(r * a.cos(), r * a.sin(), -inp.drop)
                })
                .collect();
            (pts, true)
        }
        PathKind::Racetrack { w, d, fillet } => {
            let (hw, hd) = (w.max(0.2) * 0.5, d.max(0.2) * 0.5);
            let f = fillet.clamp(0.02, hw.min(hd) - 0.01);
            let mut pts: Vec<Vec3> = Vec::new();
            // Four straights + four 90° corner arcs, CCW, sampled at ~STEP.
            let corners = [
                (Vec3::new(hw - f, hd - f, 0.0), 0.0f32),
                (Vec3::new(-hw + f, hd - f, 0.0), 90.0),
                (Vec3::new(-hw + f, -hd + f, 0.0), 180.0),
                (Vec3::new(hw - f, -hd + f, 0.0), 270.0),
            ];
            for ci in 0..4 {
                let (centre, a0) = corners[ci];
                let arc_n = ((std::f32::consts::FRAC_PI_2 * f / STEP).round() as usize).max(4);
                for k in 0..=arc_n {
                    let a = (a0 + 90.0 * k as f32 / arc_n as f32).to_radians();
                    pts.push(Vec3::new(centre.x + f * a.cos(), centre.y + f * a.sin(), -inp.drop));
                }
                // The straight to the next corner start falls out of the resample.
            }
            pts.reverse(); // clockwise — same reason as the ring: +u outward under a level frame
            (pts, true)
        }
        PathKind::SCurve { length, width } => {
            let l = length.max(0.5);
            let w2 = width.max(0.0);
            let ctrl = [
                Vec3::new(0.0, 0.0, -inp.drop),
                Vec3::new(l * 0.25, w2, -(inp.drop * 0.75 + inp.drop_end * 0.25)),
                Vec3::new(l * 0.5, 0.0, -(inp.drop + inp.drop_end) * 0.5),
                Vec3::new(l * 0.75, -w2, -(inp.drop * 0.25 + inp.drop_end * 0.75)),
                Vec3::new(l, 0.0, -inp.drop_end),
            ];
            (catmull_rom(&ctrl, 24), false)
        }
        PathKind::Custom { pts, closed, fillet } => {
            // Clean: drop consecutive near-duplicates and a duplicated closing point.
            let mut p: Vec<glam::Vec2> = Vec::with_capacity(pts.len());
            for &q in &pts {
                if p.last().map_or(true, |l| l.distance(q) > 2e-3) {
                    p.push(q);
                }
            }
            if closed && p.len() > 2 && p[0].distance(*p.last().unwrap()) < 2e-3 {
                p.pop();
            }
            // Re-centre on the plan centroid (md B9) so the fixture is a local object.
            let c = p.iter().copied().reduce(|a, b| a + b).unwrap_or(glam::Vec2::ZERO) / p.len().max(1) as f32;
            for q in &mut p {
                *q -= c;
            }
            // A closed plan loop must run CLOCKWISE for +u outward (same as Ring/Racetrack).
            if closed {
                let area2: f32 = (0..p.len())
                    .map(|i| {
                        let (a, b) = (p[i], p[(i + 1) % p.len()]);
                        a.x * b.y - b.x * a.y
                    })
                    .sum();
                if area2 > 0.0 {
                    p.reverse(); // was counter-clockwise
                }
            }
            let filleted = fillet_polyline(&p, closed, fillet.max(0.0));
            // Descend open custom runs from `drop` to `drop_end` by arc length; closed = level.
            let mut cum = vec![0.0f32];
            for i in 1..filleted.len() {
                cum.push(cum[i - 1] + filleted[i].distance(filleted[i - 1]));
            }
            let total = *cum.last().unwrap();
            let out: Vec<Vec3> = filleted
                .iter()
                .enumerate()
                .map(|(i, q)| {
                    let t = if closed || total < 1e-6 { 0.0 } else { cum[i] / total };
                    Vec3::new(q.x, q.y, -(inp.drop * (1.0 - t) + inp.drop_end * t))
                })
                .collect();
            (out, closed)
        }
    }
}

/// Fillet every corner of a polyline (md B6/C3): `t = r / tan(θ/2)`, clamped to 0.48× of each
/// adjacent segment, blending circularly between the tangent points. Corners flatter than ~8° are
/// left alone (already smooth — e.g. a sampled arc). Open ends are untouched.
fn fillet_polyline(p: &[glam::Vec2], closed: bool, radius: f32) -> Vec<glam::Vec2> {
    let n = p.len();
    if n < 3 || radius < 1e-4 {
        return p.to_vec();
    }
    let mut out: Vec<glam::Vec2> = Vec::new();
    let corner_range = if closed { 0..n } else { 1..n - 1 };
    if !closed {
        out.push(p[0]);
    }
    for i in corner_range {
        let prev = p[(i + n - 1) % n];
        let cur = p[i];
        let next = p[(i + 1) % n];
        let a = (cur - prev).normalize_or_zero();
        let b = (next - cur).normalize_or_zero();
        let turn = a.dot(b).clamp(-1.0, 1.0).acos();
        if turn < 0.14 {
            out.push(cur); // ≈ < 8°: already smooth
            continue;
        }
        let half = (std::f32::consts::PI - turn) * 0.5; // interior half-angle
        let mut t = radius / half.tan().max(1e-4);
        t = t.min(0.48 * cur.distance(prev)).min(0.48 * cur.distance(next));
        let r_eff = t * half.tan();
        let p0 = cur - a * t;
        let p1 = cur + b * t;
        // Circular blend between the tangent points, sampled FINER than the arc-length resample
        // step — coarser chords put all the turning at kink vertices and the curvature validator
        // reads a far smaller radius than the true fillet.
        let arc_len = r_eff.max(1e-4) * turn;
        let steps = ((arc_len / (STEP * 0.5)).ceil() as usize).clamp(4, 400);
        for k in 0..=steps {
            let s = k as f32 / steps as f32;
            // Rational quadratic Bézier through the corner control point with w = cos(turn/2) —
            // an EXACT circular arc between the tangent points.
            let w = (turn * 0.5).cos();
            let one = (1.0 - s) * (1.0 - s);
            let two = 2.0 * s * (1.0 - s) * w;
            let three = s * s;
            let denom = one + two + three;
            out.push((p0 * one + cur * two + p1 * three) / denom);
        }
    }
    if !closed {
        out.push(p[n - 1]);
    }
    out
}

/// Arc-length resample at ~[`STEP`] (md C4): `n = round(total/step)`, actual step = `total/n`, so
/// a closed path closes exactly. Closed: n points (wrap); open: n+1 points including both ends.
fn resample(raw: &[Vec3], closed: bool) -> (Vec<Vec3>, f32) {
    let mut pts = raw.to_vec();
    if closed {
        pts.push(raw[0]);
    }
    let mut cum = vec![0.0f32];
    for i in 1..pts.len() {
        cum.push(cum[i - 1] + pts[i].distance(pts[i - 1]));
    }
    let total = *cum.last().unwrap();
    let n = ((total / STEP).round() as usize).max(8);
    let step = total / n as f32;
    let count = if closed { n } else { n + 1 };
    let mut out = Vec::with_capacity(count);
    let mut seg = 0usize;
    for k in 0..count {
        let target = step * k as f32;
        while seg + 1 < cum.len() - 1 && cum[seg + 1] < target {
            seg += 1;
        }
        let span = (cum[seg + 1] - cum[seg]).max(1e-9);
        let t = ((target - cum[seg]) / span).clamp(0.0, 1.0);
        out.push(pts[seg].lerp(pts[seg + 1], t));
    }
    (out, total)
}

// ============================ frames ============================

struct Frames {
    t: Vec<Vec3>,
    n: Vec<Vec3>,
    b: Vec<Vec3>,
}

/// Tangents by central differences; RMF by double reflection; twist closure on closed paths;
/// levelled with a fade near vertical (md C5). Returns the worst `|T·Z|` for reporting.
fn make_frames(p: &[Vec3], closed: bool) -> (Frames, f32) {
    let n = p.len();
    let at = |i: i64| -> Vec3 {
        if closed {
            p[i.rem_euclid(n as i64) as usize]
        } else {
            p[i.clamp(0, n as i64 - 1) as usize]
        }
    };
    let mut t: Vec<Vec3> = (0..n as i64).map(|i| (at(i + 1) - at(i - 1)).normalize_or_zero()).collect();
    for i in 0..n {
        if t[i].length_squared() < 0.5 {
            t[i] = Vec3::X; // degenerate guard
        }
    }
    // Seed normal so the bitangent B = T×N starts UP: N = Z×T (or X×T when the tangent is
    // vertical). Starting B-down would make the levelling step roll every frame 180°.
    let seed_up = if t[0].z.abs() > 0.9 { Vec3::X } else { Vec3::Z };
    let mut nrm = vec![Vec3::ZERO; n];
    nrm[0] = (seed_up.cross(t[0])).normalize_or_zero();
    if nrm[0].length_squared() < 0.5 {
        nrm[0] = Vec3::Y;
    }
    // Double reflection (Wang et al. 2008).
    for i in 0..n - 1 {
        let v1 = p[i + 1] - p[i];
        let c1 = v1.length_squared().max(1e-12);
        let nl = nrm[i] - v1 * (2.0 / c1) * v1.dot(nrm[i]);
        let tl = t[i] - v1 * (2.0 / c1) * v1.dot(t[i]);
        let v2 = t[i + 1] - tl;
        let c2 = v2.length_squared().max(1e-12);
        let mut ni = nl - v2 * (2.0 / c2) * v2.dot(nl);
        ni = (ni - t[i + 1] * ni.dot(t[i + 1])).normalize_or_zero();
        nrm[i + 1] = ni;
    }
    let mut b: Vec<Vec3> = (0..n).map(|i| t[i].cross(nrm[i]).normalize_or_zero()).collect();

    // Twist closure (closed): distribute the residual roll linearly (md C5).
    if closed {
        // Transport one more step to see where the frame lands relative to the start.
        let v1 = p[0] - p[n - 1];
        let c1 = v1.length_squared().max(1e-12);
        let nl = nrm[n - 1] - v1 * (2.0 / c1) * v1.dot(nrm[n - 1]);
        let tl = t[n - 1] - v1 * (2.0 / c1) * v1.dot(t[n - 1]);
        let v2 = t[0] - tl;
        let c2 = v2.length_squared().max(1e-12);
        let mut n_last = nl - v2 * (2.0 / c2) * v2.dot(nl);
        n_last = (n_last - t[0] * n_last.dot(t[0])).normalize_or_zero();
        let b_first = t[0].cross(nrm[0]).normalize_or_zero();
        let ang = n_last.dot(b_first).atan2(n_last.dot(nrm[0]));
        for i in 0..n {
            let roll = -ang * (i as f32 / n as f32);
            let (s, c) = roll.sin_cos();
            let (ni, bi) = (nrm[i], b[i]);
            nrm[i] = ni * c + bi * s;
            b[i] = bi * c - ni * s;
        }
    }

    // Level the frames with a fade near vertical (md C5): bring B toward world +Z.
    let mut worst_tz = 0.0f32;
    for i in 0..n {
        let tz = t[i].z.abs();
        worst_tz = worst_tz.max(tz);
        let d = (Vec3::Z - t[i] * t[i].z).normalize_or_zero();
        if d.length_squared() < 0.5 {
            continue;
        }
        let ang = d.dot(nrm[i]).atan2(d.dot(b[i]));
        // Fade the correction to zero over |T·Z| ∈ [0.835, 0.985] — never branch hard.
        let k = 1.0 - ((tz - 0.835) / (0.985 - 0.835)).clamp(0.0, 1.0);
        let roll = ang * k;
        let (s, c) = roll.sin_cos();
        let (ni, bi) = (nrm[i], b[i]);
        nrm[i] = ni * c + bi * s;
        b[i] = bi * c - ni * s;
    }
    (Frames { t, n: nrm, b }, worst_tz)
}

/// Smallest circumradius over consecutive point triples (md C6).
fn min_curve_radius(p: &[Vec3], closed: bool) -> f32 {
    let n = p.len();
    let mut best = f32::INFINITY;
    let range = if closed { 0..n } else { 1..n - 1 };
    for i in range {
        let (a, b, c) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
        let (ab, bc, ca) = (a.distance(b), b.distance(c), c.distance(a));
        let cross = (b - a).cross(c - a).length();
        if cross > 1e-9 {
            best = best.min(ab * bc * ca / (2.0 * cross));
        }
    }
    best
}

// ============================ mesh emit ============================

fn push_tri(mesh: &mut SolidMesh, part: u32, a: Vec3, b: Vec3, c: Vec3, na: Vec3, nb: Vec3, nc: Vec3) {
    mesh.positions.push(a.into());
    mesh.positions.push(b.into());
    mesh.positions.push(c.into());
    mesh.normals.push(na.into());
    mesh.normals.push(nb.into());
    mesh.normals.push(nc.into());
    mesh.face_ids.push(part);
}

/// A vertical 8-sided cylinder from `z0` up to `z1` at `(x, y)`, with a top+bottom cap.
fn push_cylinder(mesh: &mut SolidMesh, part: u32, x: f32, y: f32, r: f32, z0: f32, z1: f32) {
    let n = 8;
    let ring = |z: f32| -> Vec<Vec3> {
        (0..n)
            .map(|k| {
                let a = std::f32::consts::TAU * k as f32 / n as f32;
                Vec3::new(x + r * a.cos(), y + r * a.sin(), z)
            })
            .collect()
    };
    let (lo, hi) = (ring(z0), ring(z1));
    for k in 0..n {
        let k1 = (k + 1) % n;
        let nrm_k = (lo[k] - Vec3::new(x, y, z0)).normalize_or_zero();
        let nrm_k1 = (lo[k1] - Vec3::new(x, y, z0)).normalize_or_zero();
        push_tri(mesh, part, lo[k], lo[k1], hi[k1], nrm_k, nrm_k1, nrm_k1);
        push_tri(mesh, part, lo[k], hi[k1], hi[k], nrm_k, nrm_k1, nrm_k);
    }
    let (cd, cu) = (Vec3::new(x, y, z0), Vec3::new(x, y, z1));
    for k in 0..n {
        let k1 = (k + 1) % n;
        push_tri(mesh, part, cd, lo[k1], lo[k], -Vec3::Z, -Vec3::Z, -Vec3::Z);
        push_tri(mesh, part, cu, hi[k], hi[k1], Vec3::Z, Vec3::Z, Vec3::Z);
    }
}

// ============================ build ============================

const PART_BODY: u32 = 0;
const PART_LENS: u32 = 1;
const PART_ROD: u32 = 2;

/// Build the fixture. Errors are ACTIONABLE (md C6): the self-intersection limit names both
/// numbers and the fix.
/// One emitting point on the run: where it is, and how much flux it carries.
///
/// A linear fitting is a LINE of light, and the lighting engine's luminaire is a point with a
/// distribution — so the run is represented by points spaced along it, each carrying its share of
/// the flux. That is the standard treatment and it converges quickly: at a spacing well under the
/// mounting height the field is indistinguishable from a true line source, and the error only shows
/// directly beneath, closer than the spacing itself.
#[derive(Clone, Copy, Debug)]
pub struct Emitter {
    /// Position in the fixture's own frame — ceiling at z = 0, fixture hanging below.
    pub pos: [f32; 3],
    /// Luminous flux this point carries, lumens.
    pub lumens: f64,
    /// Its share of the connected load, watts.
    pub watts: f64,
}

/// The run's emitting points, at roughly `spacing_m` apart along the path.
///
/// Total flux is `path_len × watts_per_m × efficacy`, split evenly — the run is uniform, so each
/// point carries the same share. Returns empty when the path is unusable or the fitting is
/// specified with no output, rather than a list of zero-flux lights that would look like a working
/// installation contributing nothing.
///
/// The points sit on the LENS, not the path centreline: the path is the top of the profile (v = 0,
/// where the droppers meet), and the light leaves from the diffuser below it. Emitting from the
/// centreline would place every source `height` too high — small, but wrong in a way that grows as
/// the fitting gets deeper.
pub fn emitters(inp: &SweepInput, spacing_m: f32) -> Vec<Emitter> {
    let (raw, closed) = sample_path(inp);
    if raw.len() < 3 {
        return Vec::new();
    }
    let (pts, total_len) = resample(&raw, closed);
    if pts.is_empty() || !(total_len.is_finite() && total_len > 1e-4) {
        return Vec::new();
    }
    let total_w = total_len as f64 * inp.watts_per_m.max(0.0) as f64;
    let total_lm = total_w * inp.efficacy_lm_per_w.max(0.0) as f64;
    if total_lm <= 0.0 {
        return Vec::new();
    }

    let step = spacing_m.max(0.02);
    let n = ((total_len / step).round() as usize).max(1);
    let per_lm = total_lm / n as f64;
    let per_w = total_w / n as f64;
    // The lens hangs below the path by the profile height (its face is on the underside for every
    // profile that points light into the room).
    let drop_to_lens = inp.height;

    (0..n)
        .map(|k| {
            // Sample the resampled path, which is already uniform in arc length.
            let t = (k as f32 + 0.5) / n as f32;
            let i = ((t * pts.len() as f32) as usize).min(pts.len() - 1);
            let p = pts[i];
            Emitter {
                pos: [p.x, p.y, p.z - drop_to_lens],
                lumens: per_lm,
                watts: per_w,
            }
        })
        .collect()
}

pub fn build(inp: &SweepInput) -> Result<(SweepMetrics, SolidMesh, Vec<Material>), String> {
    let profile = make_profile(inp.profile, inp.width, inp.height, inp.lens);
    let (raw, closed) = sample_path(inp);
    if raw.len() < 3 {
        return Err("path too short".into());
    }
    let (pts, total_len) = resample(&raw, closed);
    let n = pts.len();

    // C6 — the self-intersection limit, before any geometry is emitted.
    let reach = profile.inner_reach();
    let min_r = min_curve_radius(&pts, closed);
    if min_r < reach * 1.15 {
        return Err(format!(
            "min curve radius {:.0} mm against a profile reaching {:.0} mm inboard — the swept \
             surface folds through itself. Increase the radius/fillet to ≥ {:.0} mm or narrow the profile.",
            min_r * 1000.0,
            reach * 1000.0,
            reach * 1.15 * 1000.0
        ));
    }

    let (frames, worst_tz) = make_frames(&pts, closed);
    let mut warnings = Vec::new();
    if worst_tz > 0.90 {
        warnings.push(format!("path reaches |T·Z| = {worst_tz:.2} — near-vertical; the profile may bank there"));
    }

    let mut mesh = SolidMesh::default();

    // C7 — the sweep: one independent strip per profile EDGE, duplicated corner vertices.
    let np = profile.pts.len();
    let world = |i: usize, u: f32, v: f32| -> Vec3 { pts[i] + frames.n[i] * u + frames.b[i] * v };
    let steps = if closed { n } else { n - 1 };
    for j in 0..np {
        let (p0, p1) = (profile.pts[j], profile.pts[(j + 1) % np]);
        let part = match profile.mats[j] {
            Material::Lens => PART_LENS,
            _ => PART_BODY,
        };
        // 2D outward edge normal for a CCW polygon: (dv, -du).
        let (du, dv) = (p1[0] - p0[0], p1[1] - p0[1]);
        let el = (du * du + dv * dv).sqrt().max(1e-9);
        let (nu, nv) = (dv / el, -du / el);
        for i in 0..steps {
            let i1 = (i + 1) % n;
            let a = world(i, p0[0], p0[1]);
            let b = world(i, p1[0], p1[1]);
            let c = world(i1, p1[0], p1[1]);
            let d = world(i1, p0[0], p0[1]);
            // Smooth ALONG the sweep (per-station frame normal), hard ACROSS the profile.
            let n_i = frames.n[i] * nu + frames.b[i] * nv;
            let n_i1 = frames.n[i1] * nu + frames.b[i1] * nv;
            push_tri(&mut mesh, part, a, b, c, n_i, n_i, n_i1);
            push_tri(&mut mesh, part, a, c, d, n_i, n_i1, n_i1);
        }
    }

    // C7 — end caps on open paths (body material; a real end cap covers the lens end too).
    if !closed {
        let centroid = |i: usize| -> Vec3 {
            let mut s = Vec3::ZERO;
            for p in &profile.pts {
                s += world(i, p[0], p[1]);
            }
            s / np as f32
        };
        for (i, flip) in [(0usize, true), (n - 1, false)] {
            let c = centroid(i);
            let nrm = if flip { -frames.t[i] } else { frames.t[i] };
            for j in 0..np {
                let a = world(i, profile.pts[j][0], profile.pts[j][1]);
                let b = world(i, profile.pts[(j + 1) % np][0], profile.pts[(j + 1) % np][1]);
                if flip {
                    push_tri(&mut mesh, PART_BODY, c, b, a, nrm, nrm, nrm);
                } else {
                    push_tri(&mut mesh, PART_BODY, c, a, b, nrm, nrm, nrm);
                }
            }
        }
    }

    // C8 — the hanging system: evenly re-spaced count, never a leftover last gap.
    let stations: Vec<usize> = if closed {
        let count = ((total_len / inp.spacing.max(0.2)).round() as usize).max(3);
        (0..count).map(|k| (k * n) / count % n).collect()
    } else {
        let usable = (total_len - 2.0 * END_MARGIN).max(0.0);
        let count = ((usable / inp.spacing.max(0.2)).round() as usize + 1).max(2);
        let step = total_len / (n - 1) as f32;
        (0..count)
            .map(|k| {
                let s = END_MARGIN + usable * k as f32 / (count - 1) as f32;
                ((s / step).round() as usize).min(n - 1)
            })
            .collect()
    };
    for &i in &stations {
        let p = pts[i];
        push_cylinder(&mut mesh, PART_ROD, p.x, p.y, ROD_R, p.z + ROD_SINK - inp.height.min(0.05), 0.0);
        push_cylinder(&mut mesh, PART_ROD, p.x, p.y, ROSE_R, -ROSE_H, 0.0);
    }
    let droppers = stations.len();
    let achieved = if closed {
        total_len / droppers as f32
    } else if droppers > 1 {
        (total_len - 2.0 * END_MARGIN).max(0.0) / (droppers - 1) as f32
    } else {
        0.0
    };
    if achieved > 2.0 {
        warnings.push(format!("hanger spacing {achieved:.2} m exceeds 2.0 m — the profile may sag"));
    }
    if total_len > 60.0 {
        warnings.push(format!(
            "path is {total_len:.0} m long — if the 2D drawing is in millimetres, scale the curve down (the sweep works in metres)"
        ));
    }

    let total_drop = inp.drop.max(inp.drop_end) - profile.min_v();
    let metrics = SweepMetrics {
        path_len: total_len,
        droppers,
        achieved_spacing: achieved,
        min_radius: min_r,
        tris: mesh.tri_count(),
        total_drop,
        warnings,
    };
    Ok((metrics, mesh, vec![Material::Body, Material::Lens, Material::Rod]))
}

// ============================ tests ============================

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(radius: f32) -> SweepInput {
        SweepInput { path: PathKind::Ring { radius }, ..Default::default() }
    }

    /// The md's key visual property: on the reference ring, the LENS faces the centre.
    #[test]
    fn ring_lens_faces_inward() {
        let (m, mesh, mats) = build(&ring(0.4)).expect("ring builds");
        assert_eq!(mats, vec![Material::Body, Material::Lens, Material::Rod]);
        assert!(m.tris > 500);
        // Every lens vertex must sit CLOSER to the ring axis than the outer body face does.
        let mut lens_r = (f32::INFINITY, 0.0f32); // (min, max) radial distance of lens verts
        let mut body_max_r = 0.0f32;
        for t in 0..mesh.tri_count() {
            for k in 0..3 {
                let p = mesh.positions[t * 3 + k];
                let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
                if mesh.face_ids[t] == PART_LENS {
                    lens_r.0 = lens_r.0.min(r);
                    lens_r.1 = lens_r.1.max(r);
                } else if mesh.face_ids[t] == PART_BODY {
                    body_max_r = body_max_r.max(r);
                }
            }
        }
        assert!(lens_r.1 < 0.4, "lens sits inboard of the path radius: {lens_r:?}");
        assert!(body_max_r > 0.4, "body reaches outboard of the path radius");
    }

    /// Closed loops: ≥ 3 hangers, evenly re-spaced (achieved ≈ length / count) — md C8.
    #[test]
    fn closed_ring_hangs_on_at_least_three_evenly() {
        let (m, _, _) = build(&ring(0.3)).unwrap();
        assert!(m.droppers >= 3, "{} droppers", m.droppers);
        assert!((m.achieved_spacing - m.path_len / m.droppers as f32).abs() < 1e-4);
        // The md's reference: a 5.03 m ring at 1.0 m spacing hangs on 5.
        let big = SweepInput { path: PathKind::Ring { radius: 0.8 }, ..Default::default() };
        let (m2, _, _) = build(&big).unwrap();
        assert_eq!(m2.droppers, 5, "5.03 m ring at 1.0 m spacing → 5 droppers");
    }

    /// Open paths get end caps and end-pinned hangers inside the margin — md C7/C8.
    #[test]
    fn open_scurve_caps_and_end_pins() {
        let inp = SweepInput {
            path: PathKind::SCurve { length: 4.0, width: 0.5 },
            drop_end: 1.0,
            ..Default::default()
        };
        let (m, mesh, _) = build(&inp).unwrap();
        assert!(m.droppers >= 2);
        // Rod geometry must reach the ceiling plane z = 0 exactly (roses sit on it).
        let mut top = f32::NEG_INFINITY;
        for t in 0..mesh.tri_count() {
            if mesh.face_ids[t] == PART_ROD {
                for k in 0..3 {
                    top = top.max(mesh.positions[t * 3 + k][2]);
                }
            }
        }
        assert!((top - 0.0).abs() < 1e-5, "suspension reaches the ceiling: {top}");
        // Descends: lowest fixture point near the far-end drop.
        let (mn, _) = mesh.bounds().unwrap();
        assert!(mn[2] < -1.0, "descends to the far drop: {}", mn[2]);
    }

    /// The C6 validator fires with an ACTIONABLE message (both numbers + the fix).
    #[test]
    fn too_tight_ring_is_rejected_with_numbers() {
        let e = build(&ring(0.02)).unwrap_err();
        assert!(e.contains("folds through itself"), "{e}");
        assert!(e.contains("mm"), "actionable numbers: {e}");
    }

    /// Arc-length resample is uniform (md C4) — max deviation of step lengths is tiny.
    #[test]
    fn resample_is_uniform() {
        let inp = SweepInput { path: PathKind::SCurve { length: 3.0, width: 0.4 }, ..Default::default() };
        let (raw, closed) = sample_path(&inp);
        let (pts, total) = resample(&raw, closed);
        let step = total / (pts.len() - 1) as f32;
        for i in 1..pts.len() {
            let d = pts[i].distance(pts[i - 1]);
            assert!((d - step).abs() < step * 0.25, "station {i}: {d} vs {step}");
        }
    }

    /// RMF frames never flip: consecutive normals stay on the same side (md C5).
    #[test]
    fn frames_do_not_flip_on_the_s_curve() {
        let inp = SweepInput { path: PathKind::SCurve { length: 4.0, width: 0.8 }, ..Default::default() };
        let (raw, closed) = sample_path(&inp);
        let (pts, _) = resample(&raw, closed);
        let (f, _) = make_frames(&pts, closed);
        for i in 1..pts.len() {
            assert!(f.n[i].dot(f.n[i - 1]) > 0.0, "frame flip at station {i}");
            assert!(f.b[i].z > 0.0, "levelled frame keeps up up at {i}");
        }
    }

    /// A user-drawn OPEN polyline with a right-angle corner: the fillet turns the corner into a
    /// smooth arc the profile can follow — without it the C6 validator would reject the sweep.
    #[test]
    fn custom_polyline_corner_is_filleted() {
        let inp = SweepInput {
            path: PathKind::Custom {
                pts: vec![glam::Vec2::new(0.0, 0.0), glam::Vec2::new(2.0, 0.0), glam::Vec2::new(2.0, 1.5)],
                closed: false,
                fillet: 0.25,
            },
            ..Default::default()
        };
        let (m, _, _) = build(&inp).expect("filleted corner sweeps");
        assert!(m.min_radius > 0.15, "corner rounded near the fillet radius: {}", m.min_radius);
        assert!(m.path_len > 3.0 && m.path_len < 3.6, "≈ leg lengths minus the corner cut: {}", m.path_len);
        // The same polyline with NO fillet folds through itself and must be rejected.
        let sharp = SweepInput {
            path: PathKind::Custom {
                pts: vec![glam::Vec2::new(0.0, 0.0), glam::Vec2::new(2.0, 0.0), glam::Vec2::new(2.0, 1.5)],
                closed: false,
                fillet: 0.0,
            },
            ..Default::default()
        };
        assert!(build(&sharp).is_err(), "an unfilleted right angle folds the profile");
    }

    /// A CLOSED user loop drawn counter-clockwise is re-oriented so the lens still faces inward,
    /// and the path is re-centred on its centroid.
    #[test]
    fn custom_closed_loop_reorients_and_recentres() {
        // A CCW square around (5, 5) — off-centre on purpose.
        let sq = vec![
            glam::Vec2::new(4.0, 4.0),
            glam::Vec2::new(6.0, 4.0),
            glam::Vec2::new(6.0, 6.0),
            glam::Vec2::new(4.0, 6.0),
        ];
        let inp = SweepInput {
            path: PathKind::Custom { pts: sq, closed: true, fillet: 0.3 },
            ..Default::default()
        };
        let (m, mesh, _) = build(&inp).expect("closed custom loop builds");
        assert!(m.droppers >= 3);
        let (mn, mx) = mesh.bounds().unwrap();
        // Re-centred: the fixture straddles the origin, not (5, 5).
        assert!(mn[0] < 0.0 && mx[0] > 0.0 && mn[1] < 0.0 && mx[1] > 0.0, "recentred: {mn:?} {mx:?}");
        // Lens inboard (same radial check as the ring — orientation was fixed to CW).
        let mut lens_max = 0.0f32;
        let mut body_max = 0.0f32;
        for t in 0..mesh.tri_count() {
            for k in 0..3 {
                let p = mesh.positions[t * 3 + k];
                let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
                if mesh.face_ids[t] == PART_LENS {
                    lens_max = lens_max.max(r);
                } else if mesh.face_ids[t] == PART_BODY {
                    body_max = body_max.max(r);
                }
            }
        }
        assert!(lens_max < body_max, "lens sits inboard of the body: {lens_max} vs {body_max}");
    }

    /// Racetrack: closed, valid, and its fillets respect the profile reach.
    #[test]
    fn racetrack_builds_closed() {
        let inp = SweepInput {
            path: PathKind::Racetrack { w: 1.8, d: 0.9, fillet: 0.25 },
            ..Default::default()
        };
        let (m, _, _) = build(&inp).unwrap();
        assert!(m.path_len > 4.0 && m.min_radius > 0.2, "len {} minR {}", m.path_len, m.min_radius);
        assert!(m.droppers >= 3);
    }
}

/// A CURVED LUMINAIRE IS A LIGHT, not a glowing shape.
///
/// "they have a glow which is texture based and its not real light but just a furniture. i want to
/// make it a real light we can use in calculation… the user can select its efficacy (watt/meter),
/// and cct."
#[cfg(test)]
mod emitter_tests {
    use super::*;

    fn ring(r: f32) -> SweepInput {
        SweepInput { path: PathKind::Ring { radius: r }, ..SweepInput::default() }
    }

    /// THE SPECIFICATION. Flux is `length × W/m × lm/W`, and the emitters must carry all of it —
    /// splitting a run into points must not lose or invent light.
    #[test]
    fn the_run_carries_exactly_the_flux_it_was_specified() {
        let mut inp = ring(1.0);
        inp.watts_per_m = 10.0;
        inp.efficacy_lm_per_w = 100.0;
        let circumference = 2.0 * std::f32::consts::PI * 1.0;
        let expect_lm = circumference as f64 * 10.0 * 100.0;

        let em = emitters(&inp, 0.25);
        assert!(!em.is_empty());
        let got: f64 = em.iter().map(|e| e.lumens).sum();
        assert!(
            (got - expect_lm).abs() / expect_lm < 0.02,
            "{got:.0} lm against the specified {expect_lm:.0}",
        );
        let watts: f64 = em.iter().map(|e| e.watts).sum();
        assert!((watts - circumference as f64 * 10.0).abs() / (circumference as f64 * 10.0) < 0.02);
    }

    /// The total does not depend on how finely the run is divided — only the point count does.
    /// A result that changed with the sampling would be a knob that silently rescales a design.
    #[test]
    fn the_total_is_independent_of_the_sampling() {
        let inp = ring(1.5);
        let coarse: f64 = emitters(&inp, 0.5).iter().map(|e| e.lumens).sum();
        let fine: f64 = emitters(&inp, 0.05).iter().map(|e| e.lumens).sum();
        assert!(
            (coarse - fine).abs() / fine < 0.02,
            "coarse {coarse:.0} lm vs fine {fine:.0} lm — the sampling is changing the answer",
        );
        assert!(emitters(&inp, 0.05).len() > emitters(&inp, 0.5).len(), "…but the counts differ");
    }

    /// Longer run, more light — at a fixed W/m that is the whole meaning of the unit.
    #[test]
    fn a_longer_run_emits_proportionally_more() {
        let small: f64 = emitters(&ring(1.0), 0.2).iter().map(|e| e.lumens).sum();
        let big: f64 = emitters(&ring(2.0), 0.2).iter().map(|e| e.lumens).sum();
        assert!((big / small - 2.0).abs() < 0.05, "double the radius should double the flux");
    }

    /// A fitting specified with no output produces NO emitters — not a set of zero-flux lights,
    /// which would look like a working installation contributing nothing.
    #[test]
    fn a_fitting_with_no_output_emits_nothing() {
        let mut inp = ring(1.0);
        inp.watts_per_m = 0.0;
        assert!(emitters(&inp, 0.2).is_empty());
        inp.watts_per_m = 12.0;
        inp.efficacy_lm_per_w = 0.0;
        assert!(emitters(&inp, 0.2).is_empty());
    }

    /// The emitters sit on the LENS, below the path. The path is the top of the profile — where
    /// the droppers meet — so emitting from it would put every source a profile-height too high.
    #[test]
    fn the_light_leaves_from_the_lens_not_the_centreline() {
        let mut inp = ring(1.0);
        inp.height = 0.20;
        let em = emitters(&inp, 0.25);
        let (raw, closed) = sample_path(&inp);
        let (pts, _) = resample(&raw, closed);
        let path_z = pts[0].z;
        for e in &em {
            assert!(
                (e.pos[2] - (path_z - 0.20)).abs() < 1e-3,
                "emitter at z {} — expected {}, one profile height below the path",
                e.pos[2],
                path_z - 0.20,
            );
        }
    }

    /// They follow the path, so a ring of light is a ring: every emitter on the circle, none at
    /// its centre.
    #[test]
    fn the_emitters_follow_the_path() {
        let r = 1.2_f32;
        let em = emitters(&ring(r), 0.15);
        assert!(em.len() > 20);
        for e in &em {
            let d = (e.pos[0] * e.pos[0] + e.pos[1] * e.pos[1]).sqrt();
            assert!((d - r).abs() < 0.05, "emitter {d:.3} m from centre, ring is {r}");
        }
    }
}
