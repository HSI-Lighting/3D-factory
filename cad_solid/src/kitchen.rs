//! Parametric KITCHEN cabinet run, ported from `KITCHEN_BUILD.md` (Part B). Builds a **straight**,
//! **L** or **U** run out of two independent lanes of modules laid out along each leg's length:
//!
//! - **base** (lower) — floor-standing carcasses; a continuous worktop is generated over them, a
//!   recessed plinth + legs beneath.
//! - **wall** (upper) — hung carcasses at the splashback height. **Optional** — [`KitchenInput`]'s
//!   `include_wall` builds the run with or without the upper cabinets (the user-requested toggle).
//!
//! A `Tall` module lives in the base lane, rises to the wall-unit top, and carries no worktop.
//!
//! For **L / U** the extra legs are laid against the return walls as their own straight runs
//! (auto-filled to length), placed by a rigid quarter-turn about Z, and a blind **corner filler**
//! cabinet + wrapped worktop bridges each inside corner. Only the main leg's modules are editable;
//! the return legs are auto-populated to their length (spec §B0 box-engine subset — corner units are
//! blind fillers, not the spec's carousel/diagonal variants; curved end units are still a follow-up).
//!
//! Like [`crate::cupboard`]/[`crate::door`] this is PURE geometry (boxes + prisms, **no boolean**)
//! returning a [`SolidMesh`] whose `face_ids` tag each component, plus a [`Material`] per part id so
//! the app paints carcass / front / stone / plinth / chrome. Frame (spec §B1): `u` along a leg
//! (→ world X for the main leg), `v` out of the wall into the room (→ Y; carcass at `v = 0..depth`),
//! `z` up, floor at `z = 0`.

use crate::architecture::ArchError;
use crate::SolidMesh;

// ── levels (spec §B3; metres) — these MUST close, asserted in tests ──
const PLINTH_H: f32 = 0.150;
const BASE_H: f32 = 0.720;
const WT_T: f32 = 0.040;
const SPLASH_H: f32 = 0.500;
const WALL_H: f32 = 0.720;
const BASE_Z0: f32 = PLINTH_H; // 0.150
const BASE_Z1: f32 = BASE_Z0 + BASE_H; // 0.870
const WT_TOP: f32 = BASE_Z1 + WT_T; // 0.910
const WALL_Z0: f32 = WT_TOP + SPLASH_H; // 1.410
const WALL_Z1: f32 = WALL_Z0 + WALL_H; // 2.130
const TALL_TOP: f32 = WALL_Z1; // tall units top-align with wall units

// ── depths ──
const BASE_D: f32 = 0.560;
const WALL_D: f32 = 0.330;
const TALL_D: f32 = 0.560;
const WT_BACK: f32 = 0.005;
const WT_FRONT: f32 = 0.030;
const WT_EDGE: f32 = 0.010;
const WT_RIM: f32 = 0.008;

// ── construction ──
const PANEL_T: f32 = 0.018;
const BACK_T: f32 = 0.008;
const SHELF_T: f32 = 0.018;
const CARC_GAP: f32 = 0.0003; // air between neighbours — NEVER 0 (coplanar faces z-fight)
const DOOR_T: f32 = 0.019;
const DOOR_GAP: f32 = 0.004;
const FRONT_INSET: f32 = 0.002;

// ── plinth + legs ──
const PLINTH_SETBACK: f32 = 0.055;
const PLINTH_T: f32 = 0.016;
const LEG_R: f32 = 0.014;
const LEG_SPACING: f32 = 0.500;
const LEG_STANDOFF: f32 = 0.045;

// ── handles ──
const HANDLE_L: f32 = 0.110; // pull length
const HANDLE_PROJ: f32 = 0.030; // standoff off the front
const HANDLE_D: f32 = 0.010; // bar section
const HANDLE_INSET: f32 = 0.045; // in from the door's opening edge

const BASE_SHELVES: usize = 1;
const WALL_SHELVES: usize = 1;
const OVERLAP: f32 = 0.0002;

/// The tall unit's bottom-first stack: (height in m, is a door). `None` height flexes to fill.
const TALL_STACK: [(Option<f32>, bool); 3] = [
    (Some(0.720), true),  // bottom door
    (Some(0.540), false), // open appliance niche
    (None, true),         // top door (flexes)
];

/// Plan shape (spec §B0). `L`/`U` add return legs against the side walls with corner fillers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KitchenShape {
    /// One straight run.
    Straight,
    /// The main run plus one return leg (one inside corner).
    L,
    /// The main run plus a return leg at each end (two inside corners).
    U,
}

impl KitchenShape {
    pub fn label(self) -> &'static str {
        match self {
            KitchenShape::Straight => "Straight",
            KitchenShape::L => "L-shape",
            KitchenShape::U => "U-shape",
        }
    }
}

/// Base-lane module kind (spec §B2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseKind {
    /// A cabinet with `count` door leaves.
    Door,
    /// A stack of `count` drawer fronts.
    Drawers,
    /// An open appliance space (dishwasher / washer) — worktop bears over it, no front.
    Void,
    /// A full-height unit (door / niche / door), no worktop over it.
    Tall,
    /// Nothing.
    Gap,
}

/// Wall-lane (upper) module kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallKind {
    /// A hung cabinet with `count` door leaves.
    Door,
    /// An open shelf unit with `count` shelves, no front.
    Open,
    /// Nothing (e.g. the clear space over a tall unit).
    Gap,
}

impl BaseKind {
    pub fn label(self) -> &'static str {
        match self {
            BaseKind::Door => "Door",
            BaseKind::Drawers => "Drawers",
            BaseKind::Void => "Appliance void",
            BaseKind::Tall => "Tall unit",
            BaseKind::Gap => "Gap",
        }
    }
    /// Whether a worktop bears over this module.
    fn bears_worktop(self) -> bool {
        matches!(self, BaseKind::Door | BaseKind::Drawers | BaseKind::Void)
    }
    /// Whether a plinth runs under this module (tall units stand on plinth too).
    fn bears_plinth(self) -> bool {
        !matches!(self, BaseKind::Gap)
    }
}

impl WallKind {
    pub fn label(self) -> &'static str {
        match self {
            WallKind::Door => "Door",
            WallKind::Open => "Open shelves",
            WallKind::Gap => "Gap",
        }
    }
}

/// One base-lane module. `width` in metres; **0 = flex** (share the run's slack). `count` is the
/// door-leaf count (Door) or drawer count (Drawers); ignored otherwise.
#[derive(Clone, Copy, Debug)]
pub struct BaseModule {
    pub kind: BaseKind,
    pub width: f32,
    pub count: u8,
}

/// One wall-lane (upper) module. `count` = door leaves (Door) or shelves (Open).
#[derive(Clone, Copy, Debug)]
pub struct WallModule {
    pub kind: WallKind,
    pub width: f32,
    pub count: u8,
}

/// The material a component wears — so the app paints one flat colour per piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Material {
    Carcass,
    Front,
    Stone,
    Edge,
    Plinth,
    Metal,
}

/// Inputs. Not `Copy` (module lists). Metres throughout.
#[derive(Clone, Debug)]
pub struct KitchenInput {
    /// Plan shape — straight, L or U.
    pub shape: KitchenShape,
    /// Main-leg length along `u` (m).
    pub length: f32,
    /// First return-leg length (L and U). Auto-filled cabinets.
    pub length_b: f32,
    /// Second return-leg length (U only). Auto-filled cabinets.
    pub length_c: f32,
    pub base: Vec<BaseModule>,
    pub wall: Vec<WallModule>,
    /// Build the UPPER (hung wall) cabinets. `false` → base run only (worktop, plinth, tall stay).
    pub include_wall: bool,
    pub handles: bool,
}

impl Default for KitchenInput {
    /// The spec's `straight` variant, minus the curved end unit (box-engine subset).
    fn default() -> Self {
        use BaseKind::*;
        let b = |kind, width, count| BaseModule { kind, width, count };
        let w = |kind, width, count| WallModule { kind, width, count };
        Self {
            shape: KitchenShape::Straight,
            length: 3.600,
            length_b: 2.400,
            length_c: 2.400,
            base: vec![
                b(Door, 0.500, 1),
                b(Drawers, 0.600, 3),
                b(Void, 0.600, 0),
                b(Door, 0.800, 2),
                b(Door, 0.500, 1),
                b(Tall, 0.600, 0),
            ],
            wall: vec![
                w(WallKind::Door, 0.500, 1),
                w(WallKind::Open, 0.600, 2),
                w(WallKind::Door, 0.800, 2),
                w(WallKind::Gap, 0.0, 0), // flex — clears the tall unit
            ],
            include_wall: true,
            handles: true,
        }
    }
}

/// Derived sizes + the DERIVED counts (spec §B9 — counts are outputs, never inputs). Totals span
/// every leg + corner filler for L/U.
#[derive(Clone, Debug, PartialEq)]
pub struct KitchenMetrics {
    pub length: f32,
    pub worktop_top: f32,
    pub wall_top: f32,
    pub base_modules: usize,
    pub wall_modules: usize,
    pub door_leaves: usize,
    pub drawers: usize,
    pub voids: usize,
    pub talls: usize,
    pub shelves: usize,
    pub handles: usize,
    pub legs: usize,
    pub corners: usize,
    pub worktop_len: f32,
}

/// A placed module: index + span `[u0, u1]` along the run.
struct Placed {
    idx: usize,
    u0: f32,
    u1: f32,
}

/// A quarter-turn about Z used to seat a leg against a return wall (rigid → keeps winding/normals).
#[derive(Clone, Copy)]
enum Rot {
    D0,
    D90,
    Dm90,
}

impl Rot {
    fn apply(self, x: f32, y: f32) -> (f32, f32) {
        match self {
            Rot::D0 => (x, y),
            Rot::D90 => (-y, x),
            Rot::Dm90 => (y, -x),
        }
    }
}

/// One leg's owned module lists + where it seats in the world.
struct Leg {
    base: Vec<BaseModule>,
    wall: Vec<WallModule>,
    length: f32,
    rot: Rot,
    tx: f32,
    ty: f32,
}

/// Fill a return leg of `length` with ~600 mm auto cabinets (a drawer bank every third bay), all
/// flex-width so they share the leg exactly. Wall lane mirrors it with door cabinets when enabled.
fn auto_leg(length: f32, include_wall: bool) -> (Vec<BaseModule>, Vec<WallModule>) {
    let n = ((length / 0.6).round() as usize).max(1);
    let base = (0..n)
        .map(|i| {
            if i % 3 == 1 {
                BaseModule { kind: BaseKind::Drawers, width: 0.0, count: 3 }
            } else {
                BaseModule { kind: BaseKind::Door, width: 0.0, count: 1 }
            }
        })
        .collect();
    let wall = if include_wall {
        (0..n).map(|_| WallModule { kind: WallKind::Door, width: 0.0, count: 1 }).collect()
    } else {
        Vec::new()
    };
    (base, wall)
}

/// The legs (+ corner-filler origins) for this shape. The main leg carries the user's modules; L/U
/// add auto-filled return legs seated by a quarter-turn, and one corner square per inside corner.
fn legs_of(inp: &KitchenInput) -> (Vec<Leg>, Vec<[f32; 2]>) {
    let d = BASE_D;
    let main = |rot, tx, ty| Leg { base: inp.base.clone(), wall: inp.wall.clone(), length: inp.length, rot, tx, ty };
    match inp.shape {
        KitchenShape::Straight => (vec![main(Rot::D0, 0.0, 0.0)], Vec::new()),
        KitchenShape::L => {
            let (bb, bw) = auto_leg(inp.length_b, inp.include_wall);
            (
                vec![
                    main(Rot::D0, d, 0.0),
                    Leg { base: bb, wall: bw, length: inp.length_b, rot: Rot::Dm90, tx: 0.0, ty: d + inp.length_b },
                ],
                vec![[0.0, 0.0]],
            )
        }
        KitchenShape::U => {
            let (bb, bw) = auto_leg(inp.length_b, inp.include_wall);
            let (cb, cw) = auto_leg(inp.length_c, inp.include_wall);
            (
                vec![
                    main(Rot::D0, d, 0.0),
                    Leg { base: bb, wall: bw, length: inp.length_b, rot: Rot::Dm90, tx: 0.0, ty: d + inp.length_b },
                    Leg { base: cb, wall: cw, length: inp.length_c, rot: Rot::D90, tx: 2.0 * d + inp.length, ty: d },
                ],
                vec![[0.0, 0.0], [d + inp.length, 0.0]],
            )
        }
    }
}

/// Lay a lane out along `[0, length]` (spec §B5): fixed widths sum, flex (width 0) shares the rest.
fn place<T>(mods: &[T], length: f32, width_of: impl Fn(&T) -> f32) -> Result<Vec<Placed>, ArchError> {
    let fixed: f32 = mods.iter().map(&width_of).filter(|w| *w > 0.0).sum();
    let flex = mods.iter().filter(|m| width_of(m) <= 0.0).count();
    let flex_w = if flex > 0 {
        let w = (length - fixed) / flex as f32;
        if w <= 0.0 {
            return Err(ArchError::Invalid("fixed module widths exceed the run length"));
        }
        w
    } else {
        0.0
    };
    let mut out = Vec::new();
    let mut u = 0.0;
    for (idx, m) in mods.iter().enumerate() {
        let mw = if width_of(m) > 0.0 { width_of(m) } else { flex_w };
        out.push(Placed { idx, u0: u, u1: u + mw });
        u += mw;
    }
    if u - length > 1e-6 {
        return Err(ArchError::Invalid("module widths overflow the run length"));
    }
    Ok(out)
}

/// Validate + derive counts + soft warnings (spec §B9 / §B10). Counts total across every leg.
pub fn plan(inp: &KitchenInput) -> Result<(KitchenMetrics, Vec<String>), ArchError> {
    if inp.length <= 0.0 {
        return Err(ArchError::NonPositive("run length"));
    }
    if inp.base.is_empty() {
        return Err(ArchError::Invalid("the base lane needs at least one module"));
    }
    if matches!(inp.shape, KitchenShape::L | KitchenShape::U) && inp.length_b <= 0.0 {
        return Err(ArchError::NonPositive("return-leg B length"));
    }
    if matches!(inp.shape, KitchenShape::U) && inp.length_c <= 0.0 {
        return Err(ArchError::NonPositive("return-leg C length"));
    }

    let (legs, corners) = legs_of(inp);
    let corner_ct = corners.len();

    let (mut door_leaves, mut drawers, mut voids, mut talls, mut shelves) = (0usize, 0, 0, 0, 0);
    let (mut base_modules, mut wall_modules) = (corner_ct, 0usize);
    let (mut legs_feet, mut worktop_len) = (4 * corner_ct, BASE_D * corner_ct as f32);

    for leg in &legs {
        let placed = place(&leg.base, leg.length, |m| m.width)?;
        if inp.include_wall {
            place(&leg.wall, leg.length, |m| m.width)?; // validate the wall lane fits
        }
        base_modules += leg.base.len();
        if inp.include_wall {
            wall_modules += leg.wall.len();
        }
        for m in &leg.base {
            match m.kind {
                BaseKind::Door => door_leaves += m.count.max(1) as usize,
                BaseKind::Drawers => drawers += m.count.max(1) as usize,
                BaseKind::Void => voids += 1,
                BaseKind::Tall => {
                    talls += 1;
                    door_leaves += TALL_STACK.iter().filter(|(_, d)| *d).count();
                }
                BaseKind::Gap => {}
            }
        }
        if inp.include_wall {
            for m in &leg.wall {
                match m.kind {
                    WallKind::Door => door_leaves += m.count.max(1) as usize,
                    WallKind::Open => shelves += m.count.max(1) as usize,
                    WallKind::Gap => {}
                }
            }
        }
        for p in &placed {
            let k = leg.base[p.idx].kind;
            if k != BaseKind::Gap {
                legs_feet += (((p.u1 - p.u0) / LEG_SPACING).round() as usize + 1).max(2);
            }
            if k.bears_worktop() {
                worktop_len += p.u1 - p.u0;
            }
        }
    }

    let handles = if inp.handles { door_leaves + drawers } else { 0 };

    let m = KitchenMetrics {
        length: inp.length,
        worktop_top: WT_TOP,
        wall_top: WALL_Z1,
        base_modules,
        wall_modules,
        door_leaves,
        drawers,
        voids,
        talls,
        shelves,
        handles,
        legs: legs_feet,
        corners: corner_ct,
        worktop_len,
    };

    // ── warnings (spec §B10) — evaluated on the editable MAIN leg. ──
    let mut warn = Vec::new();
    if !inp.include_wall {
        warn.push("upper (wall) cabinets are OFF — base run, worktop, plinth and tall units only".into());
    }
    if corner_ct > 0 {
        warn.push("corner units are blind fillers and return legs are auto-filled — tune the main leg here".into());
    }
    let base = place(&inp.base, inp.length, |m| m.width)?;
    let tall_spans: Vec<(f32, f32)> = base
        .iter()
        .filter(|p| inp.base[p.idx].kind == BaseKind::Tall)
        .map(|p| (p.u0, p.u1))
        .collect();
    for p in &base {
        let m = inp.base[p.idx];
        match m.kind {
            BaseKind::Door => {
                let leaf = (p.u1 - p.u0) / m.count.max(1) as f32;
                if leaf > 0.620 {
                    warn.push(format!("base door leaf {:.0} mm is over 620 mm — will sag and foul; split the bay", leaf * 1000.0));
                }
            }
            BaseKind::Drawers => {
                if (BASE_H / m.count.max(1) as f32) < 0.100 {
                    warn.push(format!("{} drawers gives {:.0} mm fronts — too shallow", m.count.max(1), BASE_H / m.count.max(1) as f32 * 1000.0));
                }
            }
            _ => {}
        }
    }
    if inp.include_wall {
        let wall = place(&inp.wall, inp.length, |m| m.width)?;
        for p in &wall {
            if inp.wall[p.idx].kind == WallKind::Gap {
                continue;
            }
            for (t0, t1) in &tall_spans {
                if p.u0 < t1 - 1e-6 && p.u1 > t0 + 1e-6 {
                    warn.push(format!(
                        "wall module '{}' at {:.2}–{:.2} m runs into the tall unit — put a Gap there",
                        inp.wall[p.idx].kind.label(), p.u0, p.u1,
                    ));
                }
            }
        }
    }
    Ok((m, warn))
}

// ── mesh emitters ───────────────────────────────────────────────────────────────────────────

fn alloc(mats: &mut Vec<Material>, m: Material) -> u32 {
    mats.push(m);
    (mats.len() - 1) as u32
}

/// An axis-aligned box tagged with `part`, outward flat normals (as [`crate::cupboard`]).
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
        for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            for &vi in &tri {
                mesh.positions.push(c[vi]);
                mesh.normals.push(n);
            }
            mesh.face_ids.push(part);
        }
    }
}

/// A convex polygon in XY extruded along Z, tagged with `part` (legs). Outward normals via a
/// centroid test — robust for the convex leg cross-section.
fn push_prism_z(mesh: &mut SolidMesh, part: u32, poly: &[[f32; 2]], z0: f32, z1: f32) {
    let n = poly.len();
    if n < 3 || (z1 - z0).abs() < 1e-6 {
        return;
    }
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    for p in poly {
        cx += p[0];
        cy += p[1];
    }
    let centroid = [cx / n as f32, cy / n as f32, (z0 + z1) / 2.0];
    let vtx = |i: usize, z: f32| [poly[i][0], poly[i][1], z];
    let mut tri = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
        let sub = |p: [f32; 3], q: [f32; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
        let cross = |u: [f32; 3], w: [f32; 3]| [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
        let dot = |u: [f32; 3], w: [f32; 3]| u[0] * w[0] + u[1] * w[1] + u[2] * w[2];
        let tc = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
        let out = sub(tc, centroid);
        let (p, q, r) = if dot(cross(sub(b, a), sub(c, a)), out) < 0.0 { (a, c, b) } else { (a, b, c) };
        let mut nrm = cross(sub(q, p), sub(r, p));
        let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
        if len > 1e-9 {
            nrm = [nrm[0] / len, nrm[1] / len, nrm[2] / len];
        }
        for v in [p, q, r] {
            mesh.positions.push(v);
            mesh.normals.push(nrm);
        }
        mesh.face_ids.push(part);
    };
    for i in 1..n - 1 {
        tri(vtx(0, z1), vtx(i, z1), vtx(i + 1, z1));
        tri(vtx(0, z0), vtx(i, z0), vtx(i + 1, z0));
    }
    for i in 0..n {
        let j = (i + 1) % n;
        tri(vtx(i, z0), vtx(j, z0), vtx(j, z1));
        tri(vtx(i, z0), vtx(j, z1), vtx(i, z1));
    }
}

/// An 8-sided ring for a chrome foot.
fn octagon(cx: f32, cy: f32, r: f32) -> Vec<[f32; 2]> {
    (0..8)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / 8.0;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect()
}

/// A chrome bar pull on two standoffs (proud in +v). `vertical` = along Z (door); else along U
/// (drawer). Spec §B6.1 wants a bow crescent; a straight bar is the box-engine simplification.
fn push_handle(mesh: &mut SolidMesh, part: u32, cu: f32, cz: f32, v_face: f32, vertical: bool) {
    let (d, l, proj) = (HANDLE_D, HANDLE_L, HANDLE_PROJ);
    let y_bar = [v_face + proj - d, v_face + proj];
    let y_post = [v_face - OVERLAP, v_face + proj];
    if vertical {
        push_box(mesh, part, [cu - d / 2.0, cu + d / 2.0], y_bar, [cz - l / 2.0, cz + l / 2.0]);
        push_box(mesh, part, [cu - d / 2.0, cu + d / 2.0], y_post, [cz - l / 2.0, cz - l / 2.0 + d]);
        push_box(mesh, part, [cu - d / 2.0, cu + d / 2.0], y_post, [cz + l / 2.0 - d, cz + l / 2.0]);
    } else {
        push_box(mesh, part, [cu - l / 2.0, cu + l / 2.0], y_bar, [cz - d / 2.0, cz + d / 2.0]);
        push_box(mesh, part, [cu - l / 2.0, cu - l / 2.0 + d], y_post, [cz - d / 2.0, cz + d / 2.0]);
        push_box(mesh, part, [cu + l / 2.0 - d, cu + l / 2.0], y_post, [cz - d / 2.0, cz + d / 2.0]);
    }
}

/// Whether the carcass gets a full top, front/back rails, or nothing.
#[derive(Clone, Copy, PartialEq)]
enum Top {
    Full,
    Rails,
    None,
}

/// Two sides, a deck, a back, a top (full / rails / none) and shelves — all `Carcass` (spec §B6).
/// Sides inset by `CARC_GAP/2` so neighbours never share a face.
#[allow(clippy::too_many_arguments)]
fn carcass(mesh: &mut SolidMesh, part: u32, u0: f32, u1: f32, v0: f32, v1: f32, z0: f32, z1: f32, top: Top, shelves: usize) {
    let g = CARC_GAP / 2.0;
    let (a, b) = (u0 + g, u1 - g);
    push_box(mesh, part, [a, a + PANEL_T], [v0, v1], [z0, z1]); // side L
    push_box(mesh, part, [b - PANEL_T, b], [v0, v1], [z0, z1]); // side R
    push_box(mesh, part, [a + PANEL_T, b - PANEL_T], [v0, v1], [z0, z0 + PANEL_T]); // deck
    push_box(mesh, part, [a + PANEL_T, b - PANEL_T], [v0, v0 + BACK_T], [z0 + PANEL_T, z1]); // back
    match top {
        Top::Full => {
            push_box(mesh, part, [a + PANEL_T, b - PANEL_T], [v0, v1], [z1 - PANEL_T, z1]);
        }
        Top::Rails => {
            push_box(mesh, part, [a + PANEL_T, b - PANEL_T], [v1 - 0.080, v1], [z1 - PANEL_T, z1]);
            push_box(mesh, part, [a + PANEL_T, b - PANEL_T], [v0 + BACK_T, v0 + BACK_T + 0.080], [z1 - PANEL_T, z1]);
        }
        Top::None => {}
    }
    for k in 0..shelves {
        let zz = z0 + (z1 - z0) * (k + 1) as f32 / (shelves + 1) as f32;
        push_box(mesh, part, [a + PANEL_T, b - PANEL_T], [v0 + BACK_T, v1 - 0.020], [zz, zz + SHELF_T]);
    }
}

/// `n` overlay door leaves across `[u0,u1]` + their vertical pulls (hinges mirror about centre).
#[allow(clippy::too_many_arguments)]
fn doors_in(mesh: &mut SolidMesh, front: u32, handle: Option<u32>, u0: f32, u1: f32, z0: f32, z1: f32, v_face: f32, n: usize) {
    let g = DOOR_GAP / 2.0;
    let w = (u1 - u0) / n as f32;
    for k in 0..n {
        let (a, b) = (u0 + k as f32 * w + g, u0 + (k + 1) as f32 * w - g);
        push_box(mesh, front, [a, b], [v_face, v_face + DOOR_T], [z0 + g, z1 - g]);
        if let Some(h) = handle {
            let hinge_left = (k as f32 + 0.5) < n as f32 / 2.0;
            let hu = if hinge_left { b - HANDLE_INSET } else { a + HANDLE_INSET };
            push_handle(mesh, h, hu, (z0 + z1) / 2.0, v_face + DOOR_T, true);
        }
    }
}

/// `n` drawer fronts stacked over `[z0,z1]` + horizontal pulls.
#[allow(clippy::too_many_arguments)]
fn drawers_in(mesh: &mut SolidMesh, front: u32, handle: Option<u32>, u0: f32, u1: f32, z0: f32, z1: f32, v_face: f32, n: usize) {
    let g = DOOR_GAP / 2.0;
    let h = (z1 - z0) / n as f32;
    for k in 0..n {
        let (a, b) = (z0 + k as f32 * h, z0 + (k + 1) as f32 * h);
        push_box(mesh, front, [u0 + g, u1 - g], [v_face, v_face + DOOR_T], [a + g, b - g]);
        if let Some(hn) = handle {
            push_handle(mesh, hn, (u0 + u1) / 2.0, (a + b) / 2.0, v_face + DOOR_T, false);
        }
    }
}

/// A full-height tall unit (spec §B7): carcass + bottom-first stack (door / niche / door).
fn build_tall(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &KitchenInput, u0: f32, u1: f32) {
    let carc = alloc(mats, Material::Carcass);
    let (z0, z1) = (BASE_Z0, TALL_TOP);
    let g = CARC_GAP / 2.0;
    let (a, b) = (u0 + g, u1 - g);
    push_box(mesh, carc, [a, a + PANEL_T], [0.0, TALL_D], [z0, z1]);
    push_box(mesh, carc, [b - PANEL_T, b], [0.0, TALL_D], [z0, z1]);
    push_box(mesh, carc, [a + PANEL_T, b - PANEL_T], [0.0, BACK_T], [z0, z1]);
    push_box(mesh, carc, [a + PANEL_T, b - PANEL_T], [0.0, TALL_D], [z0, z0 + PANEL_T]);
    push_box(mesh, carc, [a + PANEL_T, b - PANEL_T], [0.0, TALL_D], [z1 - PANEL_T, z1]);

    let avail = (z1 - z0) - 2.0 * PANEL_T;
    let fixed: f32 = TALL_STACK.iter().filter_map(|(h, _)| *h).sum();
    let flex = TALL_STACK.iter().filter(|(h, _)| h.is_none()).count();
    let fh = if flex > 0 { (avail - fixed) / flex as f32 } else { 0.0 };

    let front = alloc(mats, Material::Front);
    let handle = if inp.handles { Some(alloc(mats, Material::Metal)) } else { None };
    let mut z = z0 + PANEL_T;
    for (k, (h, is_door)) in TALL_STACK.iter().enumerate() {
        let h = h.unwrap_or(fh);
        if k > 0 {
            push_box(mesh, carc, [a + PANEL_T, b - PANEL_T], [0.0, TALL_D], [z, z + SHELF_T]); // divider
        }
        if *is_door {
            doors_in(mesh, front, handle, a + PANEL_T + FRONT_INSET, b - PANEL_T - FRONT_INSET, z, z + h, TALL_D, 1);
        }
        z += h;
    }
}

/// One base module. Returns whether it emitted a carcass (for leg placement it doesn't matter).
fn build_base_module(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &KitchenInput, m: BaseModule, u0: f32, u1: f32) {
    let (z0, z1) = (BASE_Z0, BASE_Z1);
    let fi = FRONT_INSET;
    match m.kind {
        BaseKind::Gap => {}
        BaseKind::Void => carcass(mesh, alloc(mats, Material::Carcass), u0, u1, 0.0, BASE_D, z0, z1, Top::None, 0),
        BaseKind::Tall => build_tall(mesh, mats, inp, u0, u1),
        BaseKind::Door | BaseKind::Drawers => {
            carcass(mesh, alloc(mats, Material::Carcass), u0, u1, 0.0, BASE_D, z0, z1, Top::Rails, BASE_SHELVES);
            let front = alloc(mats, Material::Front);
            let handle = if inp.handles { Some(alloc(mats, Material::Metal)) } else { None };
            if m.kind == BaseKind::Door {
                doors_in(mesh, front, handle, u0 + fi, u1 - fi, z0 + fi, z1, BASE_D, m.count.max(1) as usize);
            } else {
                drawers_in(mesh, front, handle, u0 + fi, u1 - fi, z0 + fi, z1, BASE_D, m.count.max(1) as usize);
            }
        }
    }
}

/// One wall (upper) module.
fn build_wall_module(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &KitchenInput, m: WallModule, u0: f32, u1: f32) {
    let (z0, z1) = (WALL_Z0, WALL_Z1);
    let fi = FRONT_INSET;
    match m.kind {
        WallKind::Gap => {}
        WallKind::Open => {
            let carc = alloc(mats, Material::Carcass);
            let g = CARC_GAP / 2.0;
            let (a, b) = (u0 + g, u1 - g);
            push_box(mesh, carc, [a, a + PANEL_T], [0.0, WALL_D], [z0, z1]);
            push_box(mesh, carc, [b - PANEL_T, b], [0.0, WALL_D], [z0, z1]);
            push_box(mesh, carc, [a + PANEL_T, b - PANEL_T], [0.0, WALL_D], [z1 - PANEL_T, z1]); // top
            push_box(mesh, carc, [a + PANEL_T, b - PANEL_T], [0.0, WALL_D], [z0, z0 + PANEL_T]); // deck
            push_box(mesh, carc, [a + PANEL_T, b - PANEL_T], [0.0, BACK_T], [z0, z1]); // back
            let n = m.count.max(1) as usize;
            for k in 0..n {
                let zz = z0 + (z1 - z0) * (k + 1) as f32 / (n + 1) as f32;
                push_box(mesh, carc, [a + PANEL_T, b - PANEL_T], [BACK_T, WALL_D], [zz, zz + SHELF_T]);
            }
        }
        WallKind::Door => {
            carcass(mesh, alloc(mats, Material::Carcass), u0, u1, 0.0, WALL_D, z0, z1, Top::Full, WALL_SHELVES);
            let front = alloc(mats, Material::Front);
            let handle = if inp.handles { Some(alloc(mats, Material::Metal)) } else { None };
            doors_in(mesh, front, handle, u0 + fi, u1 - fi, z0 + fi, z1 - fi, WALL_D, m.count.max(1) as usize);
        }
    }
}

/// Maximal contiguous spans of base modules matching `pred`.
fn spans(placed: &[Placed], base_mods: &[BaseModule], pred: impl Fn(BaseKind) -> bool) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::new();
    for p in placed {
        if pred(base_mods[p.idx].kind) {
            if let Some(last) = out.last_mut() {
                if (last.1 - p.u0).abs() < 1e-6 {
                    last.1 = p.u1;
                    continue;
                }
            }
            out.push((p.u0, p.u1));
        }
    }
    out
}

/// Build ONE straight run (base + optional wall lanes, worktop, plinth, legs) in its local frame
/// `[0, length]` along `u` (→ x), returning the mesh + material-per-part. Composed by [`build`].
fn build_run(inp: &KitchenInput, base_mods: &[BaseModule], wall_mods: &[WallModule], length: f32) -> Result<(SolidMesh, Vec<Material>), ArchError> {
    let base = place(base_mods, length, |m| m.width)?;
    let wall = if inp.include_wall { place(wall_mods, length, |m| m.width)? } else { Vec::new() };

    let mut mesh = SolidMesh::default();
    let mut mats: Vec<Material> = Vec::new();

    for p in &base {
        build_base_module(&mut mesh, &mut mats, inp, base_mods[p.idx], p.u0, p.u1);
    }
    for p in &wall {
        build_wall_module(&mut mesh, &mut mats, inp, wall_mods[p.idx], p.u0, p.u1);
    }

    // ── Worktop over the bearing spans — three slices (stone body / dark rim / stone top). ──
    let body = alloc(&mut mats, Material::Stone);
    let rim = alloc(&mut mats, Material::Edge);
    let face = alloc(&mut mats, Material::Stone);
    for (u0, u1) in spans(&base, base_mods, |k| k.bears_worktop()) {
        let (vb, vf) = (-WT_BACK, BASE_D + WT_FRONT);
        push_box(&mut mesh, body, [u0, u1], [vb, vf], [BASE_Z1, WT_TOP - WT_RIM]);
        push_box(&mut mesh, rim, [u0, u1], [vb, vf], [WT_TOP - WT_RIM - OVERLAP, WT_TOP]);
        push_box(&mut mesh, face, [u0, u1], [vb, vf - WT_EDGE], [WT_TOP - WT_RIM, WT_TOP + OVERLAP]);
    }

    // ── Plinth — recessed, under every non-gap base module (tall units too). ──
    let plinth = alloc(&mut mats, Material::Plinth);
    let front = BASE_D - PLINTH_SETBACK;
    for (u0, u1) in spans(&base, base_mods, |k| k.bears_plinth()) {
        push_box(&mut mesh, plinth, [u0, u1], [front - PLINTH_T, front], [0.0, PLINTH_H + OVERLAP]);
    }

    // ── Legs — octagonal chrome feet in front of the plinth. ──
    let leg = alloc(&mut mats, Material::Metal);
    for p in &base {
        if base_mods[p.idx].kind == BaseKind::Gap {
            continue;
        }
        let cnt = (((p.u1 - p.u0) / LEG_SPACING).round() as usize + 1).max(2);
        for k in 0..cnt {
            let u = p.u0 + 0.060 + (p.u1 - p.u0 - 0.120) * k as f32 / (cnt - 1) as f32;
            let cy = BASE_D - LEG_STANDOFF;
            push_prism_z(&mut mesh, leg, &octagon(u, cy, LEG_R), 0.0, PLINTH_H);
            push_prism_z(&mut mesh, leg, &octagon(u, cy, LEG_R * 1.8), 0.0, 0.008); // foot
        }
    }

    Ok((mesh, mats))
}

/// A blind corner-filler base unit bridging an inside corner: carcass shell in the square
/// `[cx,cx+D] × [cy,cy+D]`, a wrapped worktop, recessed plinth and four feet. It's a solid filler
/// (no doors) — the box-engine stand-in for the spec's carousel/diagonal corner cabinet.
fn corner_unit(mesh: &mut SolidMesh, mats: &mut Vec<Material>, cx: f32, cy: f32) {
    let d = BASE_D;
    let (x0, x1) = (cx, cx + d);
    let (y0, y1) = (cy, cy + d);
    let g = CARC_GAP / 2.0;
    let (a0, a1) = (x0 + g, x1 - g);
    let (b0, b1) = (y0 + g, y1 - g);

    let carc = alloc(mats, Material::Carcass);
    push_box(mesh, carc, [a0, a0 + PANEL_T], [b0, b1], [BASE_Z0, BASE_Z1]); // side -x
    push_box(mesh, carc, [a1 - PANEL_T, a1], [b0, b1], [BASE_Z0, BASE_Z1]); // side +x
    push_box(mesh, carc, [a0, a1], [b0, b0 + PANEL_T], [BASE_Z0, BASE_Z1]); // side -y
    push_box(mesh, carc, [a0, a1], [b1 - PANEL_T, b1], [BASE_Z0, BASE_Z1]); // side +y
    push_box(mesh, carc, [a0, a1], [b0, b1], [BASE_Z0, BASE_Z0 + PANEL_T]); // deck
    push_box(mesh, carc, [a0, a1], [b0, b1], [BASE_Z1 - PANEL_T, BASE_Z1]); // top

    // Worktop — cover the square + a lip all round so it meets both legs' tops (overlap is fine).
    let m = WT_FRONT;
    let body = alloc(mats, Material::Stone);
    let rim = alloc(mats, Material::Edge);
    push_box(mesh, body, [x0 - m, x1 + m], [y0 - m, y1 + m], [BASE_Z1, WT_TOP - WT_RIM]);
    push_box(mesh, rim, [x0 - m, x1 + m], [y0 - m, y1 + m], [WT_TOP - WT_RIM - OVERLAP, WT_TOP]);

    // Recessed plinth block (mostly hidden in the corner).
    let plinth = alloc(mats, Material::Plinth);
    let s = PLINTH_SETBACK;
    push_box(mesh, plinth, [x0 + s, x1 - s], [y0 + s, y1 - s], [0.0, PLINTH_H + OVERLAP]);

    // Four feet.
    let leg = alloc(mats, Material::Metal);
    for (fx, fy) in [(x0 + 0.08, y0 + 0.08), (x1 - 0.08, y0 + 0.08), (x0 + 0.08, y1 - 0.08), (x1 - 0.08, y1 - 0.08)] {
        push_prism_z(mesh, leg, &octagon(fx, fy, LEG_R), 0.0, PLINTH_H);
        push_prism_z(mesh, leg, &octagon(fx, fy, LEG_R * 1.8), 0.0, 0.008);
    }
}

/// Append `src` (built in its own local frame) into `dst`, rotated a quarter-turn about Z and
/// translated to `(tx, ty)`, remapping every part id past `dst`'s current materials.
fn append_transformed(dst: &mut SolidMesh, dmats: &mut Vec<Material>, src: &SolidMesh, smats: &[Material], rot: Rot, tx: f32, ty: f32) {
    let base = dmats.len() as u32;
    dmats.extend_from_slice(smats);
    for p in &src.positions {
        let (x, y) = rot.apply(p[0], p[1]);
        dst.positions.push([x + tx, y + ty, p[2]]);
    }
    for n in &src.normals {
        let (x, y) = rot.apply(n[0], n[1]);
        dst.normals.push([x, y, n[2]]);
    }
    for f in &src.face_ids {
        dst.face_ids.push(f + base);
    }
}

/// Build the whole plan (straight / L / U): every leg's straight run seated by its quarter-turn,
/// plus a corner filler at each inside corner. Returns the mesh (per-component `face_ids`) + the
/// material per part id.
pub fn build(inp: &KitchenInput) -> Result<(KitchenMetrics, SolidMesh, Vec<Material>), ArchError> {
    let (metrics, _w) = plan(inp)?;
    let (legs, corners) = legs_of(inp);

    let mut mesh = SolidMesh::default();
    let mut mats: Vec<Material> = Vec::new();

    for leg in &legs {
        let (rm, rmat) = build_run(inp, &leg.base, &leg.wall, leg.length)?;
        append_transformed(&mut mesh, &mut mats, &rm, &rmat, leg.rot, leg.tx, leg.ty);
    }
    for c in &corners {
        corner_unit(&mut mesh, &mut mats, c[0], c[1]);
    }

    Ok((metrics, mesh, mats))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec §B3 / §B11.3 — the level arithmetic MUST close, or tall units stop top-aligning.
    #[test]
    fn levels_close() {
        assert!((BASE_Z0 + BASE_H + WT_T + SPLASH_H + WALL_H - WALL_Z1).abs() < 1e-6);
        let tall_carcass = TALL_TOP - BASE_Z0;
        assert!((BASE_Z0 + tall_carcass - WALL_Z1).abs() < 1e-6, "tall top aligns with wall top");
        assert!((WT_TOP - 0.910).abs() < 1e-6 && (WALL_Z1 - 2.130).abs() < 1e-6);
    }

    /// Spec §B9 — counts are DERIVED from the layout (reference straight run).
    #[test]
    fn reference_counts() {
        let (m, _w) = plan(&KitchenInput::default()).unwrap();
        // base doors: 1 + 2 + 1 = 4, tall stack has 2 doors; wall doors: 1 + 2 = 3 → 9 leaves.
        assert_eq!(m.door_leaves, 9, "door leaves {}", m.door_leaves);
        assert_eq!(m.drawers, 3);
        assert_eq!(m.voids, 1);
        assert_eq!(m.talls, 1);
        assert_eq!(m.shelves, 2);
        assert_eq!(m.corners, 0);
        assert!(m.legs >= 12, "legs {}", m.legs);
        assert!((m.worktop_top - 0.910).abs() < 1e-6);
    }

    /// The upper-part toggle: without wall units the mesh is smaller and has NO geometry in the
    /// wall band (z ∈ [WALL_Z0, WALL_Z1] except the tall unit, which still rises there).
    #[test]
    fn include_wall_toggle_removes_the_upper_cabinets() {
        let with = build(&KitchenInput { include_wall: true, ..Default::default() }).unwrap();
        let without = build(&KitchenInput { include_wall: false, ..Default::default() }).unwrap();
        assert!(without.1.tri_count() < with.1.tri_count(), "fewer tris without the upper cabinets");
        assert_eq!(without.0.wall_modules, 0);
        // Count vertices sitting in the wall band but away from the tall unit's u-range (≥3.0 m).
        let in_wall_band = |m: &SolidMesh| {
            m.positions.iter().filter(|p| p[2] > WALL_Z0 + 0.05 && p[2] < WALL_Z1 - 0.05 && p[0] < 3.0).count()
        };
        assert!(in_wall_band(&with.1) > 0, "wall cabinets present when on");
        assert_eq!(in_wall_band(&without.1), 0, "no wall cabinets when off");
    }

    /// The build yields a non-empty mesh, one material per part id, and every material present.
    #[test]
    fn build_yields_a_tagged_mesh() {
        let (_m, mesh, mats) = build(&KitchenInput::default()).unwrap();
        assert!(mesh.tri_count() > 0);
        assert_eq!(mesh.face_ids.len(), mesh.tri_count());
        assert!((*mesh.face_ids.iter().max().unwrap() as usize) < mats.len());
        for p in &mesh.positions {
            for v in p {
                assert!(v.is_finite());
            }
        }
        let used: std::collections::HashSet<Material> = mesh.face_ids.iter().map(|&id| mats[id as usize]).collect();
        for want in [Material::Carcass, Material::Front, Material::Stone, Material::Edge, Material::Plinth, Material::Metal] {
            assert!(used.contains(&want), "mesh carries {want:?}");
        }
    }

    /// L adds one corner + a return leg: more tris than straight, one corner, and cabinets that
    /// extend in +Y past the main leg's depth (the return arm).
    #[test]
    fn l_shape_adds_a_leg_and_corner() {
        let straight = build(&KitchenInput { shape: KitchenShape::Straight, ..Default::default() }).unwrap();
        let (m, mesh, mats) = build(&KitchenInput { shape: KitchenShape::L, ..Default::default() }).unwrap();
        assert_eq!(m.corners, 1);
        assert!(mesh.tri_count() > straight.1.tri_count(), "L has more geometry than straight");
        assert_eq!(mesh.face_ids.len(), mesh.tri_count());
        assert!((*mesh.face_ids.iter().max().unwrap() as usize) < mats.len());
        // The return arm runs up +Y beyond the main leg (whose depth is only BASE_D).
        let up_the_arm = mesh.positions.iter().filter(|p| p[1] > BASE_D + 0.5).count();
        assert!(up_the_arm > 0, "return leg extends up +Y");
        for p in &mesh.positions {
            for v in p {
                assert!(v.is_finite());
            }
        }
    }

    /// U adds two corners and two return arms (both ends).
    #[test]
    fn u_shape_has_two_corners_and_two_arms() {
        let (m, mesh, _mats) = build(&KitchenInput { shape: KitchenShape::U, ..Default::default() }).unwrap();
        assert_eq!(m.corners, 2);
        let d = BASE_D;
        // Left arm sits near x≈0; right arm near x ≈ 2*D + length. Both should carry geometry.
        let left_arm = mesh.positions.iter().filter(|p| p[0] < d - 0.05 && p[1] > d + 0.5).count();
        let right_arm = mesh.positions.iter().filter(|p| p[0] > d + KitchenInput::default().length + 0.05 && p[1] > d + 0.5).count();
        assert!(left_arm > 0, "left return arm present");
        assert!(right_arm > 0, "right return arm present");
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(plan(&KitchenInput { length: 0.0, ..Default::default() }).is_err());
        // Fixed widths that exceed the run length.
        let over = KitchenInput {
            length: 1.0,
            base: vec![BaseModule { kind: BaseKind::Door, width: 2.0, count: 1 }],
            ..Default::default()
        };
        assert!(plan(&over).is_err(), "overflow rejected");
        // L/U require positive return-leg lengths.
        assert!(plan(&KitchenInput { shape: KitchenShape::L, length_b: 0.0, ..Default::default() }).is_err());
        assert!(plan(&KitchenInput { shape: KitchenShape::U, length_c: 0.0, ..Default::default() }).is_err());
    }
}
