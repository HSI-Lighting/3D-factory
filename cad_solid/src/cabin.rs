//! Parametric **handleless cabinet UNIT**, ported from `CABIN_BUILD.md` (Part B). This is the
//! *close-range* object (looked at from ~2 m), so unlike [`crate::kitchen`] (rooms/runs) or
//! [`crate::cupboard`] (a quick grid configurator) the joinery is modelled for real:
//!
//! - a carcass with a **panel order** ([`PanelOrder`]) and a **rebated back**,
//! - **full-overlay fronts that tile the carcass OUTLINE** (not the interior openings), with the
//!   shadow gap taken off *internal* edges only — the single thing that stops it reading as a box,
//! - **grips** ([`Grip`]) as the parametric axis: `None` (push-to-open — the reference), `JGroove`
//!   (a routed *section*, not a box with a cut), `Bar`, `Rail`,
//! - **shelf-pin rows** as batched dark discs (the signature of a 32 mm system), and optional
//!   `Contrast` edge banding.
//!
//! PURE geometry (boxes + extruded profiles, **NO boolean**) → a [`SolidMesh`] whose `face_ids` tag
//! each component, plus a [`Material`] per part id (Carcass / Front / Edge / Metal) so the app paints
//! one flat swatch per surface. Frame (spec §B1): `u` across width `0..W`, `v` depth `0..carcass`
//! (0 at the BACK), `z` up `0..H`; fronts live proud at `v = carcass..carcass+leaf`.
//!
//! **`depth_nominal` INCLUDES the leaf** (spec §B2/§A3): `carcass_depth = depth_nominal − leaf`.
//!
//! Not ported (box-engine / flat-swatch subset): procedural anisotropic wood grain matched across
//! the gap (our engine paints flat swatches, no per-object noise), and visible hinge hardware bodies
//! (only seen with a door swung open). Door-open / drawer-pull *poses* ARE supported.

use crate::architecture::ArchError;
use crate::SolidMesh;

// ── defaults (spec §B3; metres) ──
const BOARD: f32 = 0.018; // carcass board
const LEAF: f32 = 0.019; // front thickness
const BACK_T: f32 = 0.008; // back panel
const SHADOW: f32 = 0.003; // shadow gap between leaves
const BACK_INSET: f32 = 0.012; // back rebate from the rear face
const PROUD: f32 = 0.0003; // leaf proud of the carcass edge — NEVER 0 (coplanar z-fight)
const SHELF_T: f32 = 0.018;
const PIN_PITCH: f32 = 0.032;
const PIN_DIA: f32 = 0.005;
const PIN_FROM_FACE: f32 = 0.037; // both from front and from back
const OVERLAP: f32 = 0.0002;

// ── grip geometry ──
const JG_LIP: f32 = 0.008; // lip left at the top of a J-groove leaf
const JG_CHANNEL_H: f32 = 0.028; // finger channel height
const JG_DEPTH_CUT: f32 = 0.030; // how far back the finger reaches
const BAR_L: f32 = 0.128; // D-bar pull length
const BAR_PROJ: f32 = 0.032;
const BAR_D: f32 = 0.012;
const RAIL_H: f32 = 0.028; // recessed rail channel height
const RAIL_DEPTH: f32 = 0.016; // how deep the rail is recessed

/// The grip system — the reference is `None` (handleless). Spec §B5.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// Push-to-open — no grip geometry at all (the reference).
    None,
    /// A routed J-pull section across the top of every leaf.
    JGroove,
    /// A conventional D-bar handle (for comparison).
    Bar,
    /// A recessed C-channel across the top edge of each leaf.
    Rail,
}

impl Grip {
    pub fn label(self) -> &'static str {
        match self {
            Grip::None => "None (handleless)",
            Grip::JGroove => "J-groove",
            Grip::Bar => "Bar (D-handle)",
            Grip::Rail => "Rail (recessed)",
        }
    }
}

/// Carcass construction order (spec §B4) — a real joinery choice, not a detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelOrder {
    /// Sides run full height; top and bottom fit between them.
    SidesOutside,
    /// Top and bottom run full width; sides fit between them.
    TopBottomOutside,
}

impl PanelOrder {
    pub fn label(self) -> &'static str {
        match self {
            PanelOrder::SidesOutside => "Sides outside",
            PanelOrder::TopBottomOutside => "Top/bottom outside",
        }
    }
}

/// Edge banding (spec §B5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeBand {
    /// Matches the front — no geometry.
    Match,
    /// A contrasting picture-frame band round each front (four mitred bars here).
    Contrast,
}

impl EdgeBand {
    pub fn label(self) -> &'static str {
        match self {
            EdgeBand::Match => "Match",
            EdgeBand::Contrast => "Contrast",
        }
    }
}

/// What lives in one grid position (spec §B2). `Door(n)` = `n` leaves; `Drawers(n)` = `n` fronts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    Door(u8),
    Drawers(u8),
    /// No front — the interior is open to view.
    Open,
    /// A fixed front (no opening), e.g. an end panel or filler.
    Panel,
}

impl Cell {
    pub fn label(self) -> &'static str {
        match self {
            Cell::Door(_) => "Door",
            Cell::Drawers(_) => "Drawers",
            Cell::Open => "Open",
            Cell::Panel => "Panel",
        }
    }
    fn has_front(self) -> bool {
        !matches!(self, Cell::Open)
    }
    fn is_drawer(self) -> bool {
        matches!(self, Cell::Drawers(_))
    }
}

/// The material a component wears — one flat swatch per part in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Material {
    Carcass,
    Front,
    Edge,
    Metal,
}

/// Inputs (spec §B2). Not `Copy` (grid lists). Metres throughout.
#[derive(Clone, Debug)]
pub struct CabinInput {
    pub width: f32,
    pub height: f32,
    /// OVERALL depth INCLUDING the leaf (catalogue convention). Carcass = this − leaf.
    pub depth_nominal: f32,
    /// Relative column widths (left→right).
    pub cols: Vec<f32>,
    /// Relative row heights, **TOP first**.
    pub rows: Vec<f32>,
    /// `layout[row][col]` — one [`Cell`] per position; `rows × cols`.
    pub layout: Vec<Vec<Cell>>,
    pub grip: Grip,
    pub panel_order: PanelOrder,
    pub edge_band: EdgeBand,
    /// Shelves per non-drawer cell.
    pub shelves: u8,
    /// Draw the 32 mm shelf-pin rows.
    pub pin_rows: bool,
    /// Pose: swing every door leaf by this many degrees (0 = closed; ≤110 per a std hinge).
    pub open_deg: f32,
    /// Pose: pull every drawer front out by this many metres (0 = shut).
    pub drawer_out: f32,
}

impl Default for CabinInput {
    /// The reference unit: 600 × 780 × 300, a single two-leaf opening, handleless.
    fn default() -> Self {
        Self {
            width: 0.600,
            height: 0.780,
            depth_nominal: 0.300,
            cols: vec![1.0],
            rows: vec![1.0],
            layout: vec![vec![Cell::Door(2)]],
            grip: Grip::None,
            panel_order: PanelOrder::SidesOutside,
            edge_band: EdgeBand::Match,
            shelves: 1,
            pin_rows: true,
            open_deg: 0.0,
            drawer_out: 0.0,
        }
    }
}

/// Derived sizes + DERIVED counts.
#[derive(Clone, Debug, PartialEq)]
pub struct CabinMetrics {
    pub width: f32,
    pub height: f32,
    pub depth_nominal: f32,
    pub carcass_depth: f32,
    pub cols: usize,
    pub rows: usize,
    pub door_leaves: usize,
    pub drawer_fronts: usize,
    pub opens: usize,
    pub panels: usize,
    pub shelves: usize,
    pub pins: usize,
}

// ── geometry: cumulative boundaries from relative sizes ─────────────────────────────────────

/// Cumulative fractions `[0, .., 1]` from relative sizes (len+1 entries).
fn cumulative(rel: &[f32]) -> Vec<f32> {
    let total: f32 = rel.iter().sum();
    let mut out = vec![0.0];
    let mut acc = 0.0;
    for &r in rel {
        acc += r;
        out.push(if total > 0.0 { acc / total } else { 0.0 });
    }
    out
}

/// Validate + derive counts + soft warnings (spec §B9).
pub fn plan(inp: &CabinInput) -> Result<(CabinMetrics, Vec<String>), ArchError> {
    if inp.width <= 0.0 || inp.height <= 0.0 || inp.depth_nominal <= 0.0 {
        return Err(ArchError::NonPositive("cabinet dimensions"));
    }
    if inp.cols.is_empty() || inp.rows.is_empty() {
        return Err(ArchError::Invalid("the grid needs at least one column and one row"));
    }
    // §B9 — the layout shape assertion that "fired for real".
    if inp.layout.len() != inp.rows.len() {
        return Err(ArchError::Invalid("layout row count must equal rows.len()"));
    }
    for r in &inp.layout {
        if r.len() != inp.cols.len() {
            return Err(ArchError::Invalid("every layout row must have cols.len() cells"));
        }
    }
    let carcass_depth = inp.depth_nominal - LEAF;
    if carcass_depth <= BOARD + BACK_INSET + BACK_T {
        return Err(ArchError::Invalid("depth_nominal is too shallow for board + back rebate"));
    }
    if inp.grip == Grip::JGroove && JG_DEPTH_CUT >= carcass_depth {
        return Err(ArchError::Invalid("J-groove finger channel is deeper than the carcass interior"));
    }
    if inp.open_deg > 110.0 {
        return Err(ArchError::Invalid("door angle over 110° exceeds a standard hinge"));
    }

    let (mut door_leaves, mut drawer_fronts, mut opens, mut panels, mut shelves) = (0usize, 0, 0, 0, 0);
    for row in &inp.layout {
        for &cell in row {
            match cell {
                Cell::Door(n) => {
                    door_leaves += n.max(1) as usize;
                    shelves += inp.shelves as usize;
                }
                Cell::Drawers(n) => drawer_fronts += n.max(1) as usize,
                Cell::Open => {
                    opens += 1;
                    shelves += inp.shelves as usize;
                }
                Cell::Panel => panels += 1,
            }
        }
    }

    // Pin count: two rows on each vertical panel FACE (both outer sides + both faces of each inner
    // divider), pitched at 32 mm over the interior height.
    let hi = (inp.height - 2.0 * BOARD).max(0.0);
    let per_row = (hi / PIN_PITCH).floor() as usize + 1;
    let inner_dividers = inp.cols.len().saturating_sub(1);
    let faces = 2 + 2 * inner_dividers; // 2 outer sides + 2 faces per divider
    let pins = if inp.pin_rows { faces * 2 * per_row } else { 0 };

    let m = CabinMetrics {
        width: inp.width,
        height: inp.height,
        depth_nominal: inp.depth_nominal,
        carcass_depth,
        cols: inp.cols.len(),
        rows: inp.rows.len(),
        door_leaves,
        drawer_fronts,
        opens,
        panels,
        shelves,
        pins,
    };

    // ── warnings (spec §B9) ──
    let mut warn = Vec::new();
    let cx = cumulative(&inp.cols);
    let rz = cumulative(&inp.rows);
    let iw = inp.width - 2.0 * BOARD;
    let ih = inp.height - 2.0 * BOARD;
    for (r, row) in inp.layout.iter().enumerate() {
        for (c, &cell) in row.iter().enumerate() {
            let cell_w = (cx[c + 1] - cx[c]) * iw;
            let cell_h = (rz[r + 1] - rz[r]) * ih;
            match cell {
                Cell::Door(n) => {
                    let leaf = cell_w / n.max(1) as f32;
                    if leaf > 0.620 {
                        warn.push(format!("door leaf {:.0} mm is over 620 mm — will sag and foul; split the bay", leaf * 1000.0));
                    }
                }
                Cell::Drawers(n) => {
                    let front = cell_h / n.max(1) as f32;
                    if front < 0.095 {
                        warn.push(format!("drawer front {:.0} mm is under 95 mm — too shallow", front * 1000.0));
                    }
                }
                _ => {}
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
        for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            for &vi in &tri {
                mesh.positions.push(c[vi]);
                mesh.normals.push(n);
            }
            mesh.face_ids.push(part);
        }
    }
}

/// Which pair of axes a 2D profile lives in; the third is the extrusion axis.
#[derive(Clone, Copy)]
enum Plane {
    /// poly in (u, z); extrude along v (depth).
    Uz,
    /// poly in (v, z); extrude along u (width).
    Vz,
}

fn map3(plane: Plane, p: f32, q: f32, a: f32) -> [f32; 3] {
    match plane {
        Plane::Uz => [p, a, q], // (u, z) + v=a
        Plane::Vz => [a, p, q], // (v, z) + u=a
    }
}

/// Signed area of a 2D polygon (CCW positive).
fn signed_area(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    let mut s = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        s += poly[i][0] * poly[j][1] - poly[j][0] * poly[i][1];
    }
    s * 0.5
}

/// Ear-clip a simple (possibly concave) polygon into triangles of ORIGINAL indices. Robust enough
/// for the small profiles here (J-groove leaf, octagon disc).
fn earclip(poly: &[[f32; 2]]) -> Vec<[usize; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    // Work on a CCW index ring.
    let mut ring: Vec<usize> = if signed_area(poly) < 0.0 { (0..n).rev().collect() } else { (0..n).collect() };
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let in_tri = |p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        let d1 = cross(a, b, p);
        let d2 = cross(b, c, p);
        let d3 = cross(c, a, p);
        let neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
        let pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
        !(neg && pos)
    };
    let mut out = Vec::new();
    let mut guard = 0;
    while ring.len() > 3 && guard < 10_000 {
        guard += 1;
        let m = ring.len();
        let mut clipped = false;
        for i in 0..m {
            let ia = ring[(i + m - 1) % m];
            let ib = ring[i];
            let ic = ring[(i + 1) % m];
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross(a, b, c) <= 0.0 {
                continue; // reflex — not an ear
            }
            let mut contains = false;
            for &iv in &ring {
                if iv == ia || iv == ib || iv == ic {
                    continue;
                }
                if in_tri(poly[iv], a, b, c) {
                    contains = true;
                    break;
                }
            }
            if !contains {
                out.push([ia, ib, ic]);
                ring.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break; // degenerate — bail with what we have
        }
    }
    if ring.len() == 3 {
        out.push([ring[0], ring[1], ring[2]]);
    }
    out
}

/// A closed 2D profile extruded along the third axis, tagged `part`. Caps are ear-clipped; sides are
/// per-edge quads. Every triangle's winding/normal is oriented outward from the solid centroid, so
/// concave profiles (the J-groove leaf) come out right without tracking orientation by hand.
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
    let centroid = map3(plane, cp / n as f32, cq / n as f32, (a0 + a1) / 2.0);
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
    for t in earclip(poly) {
        let (i, j, k) = (t[0], t[1], t[2]);
        tri(map3(plane, poly[i][0], poly[i][1], a1), map3(plane, poly[j][0], poly[j][1], a1), map3(plane, poly[k][0], poly[k][1], a1));
        tri(map3(plane, poly[i][0], poly[i][1], a0), map3(plane, poly[j][0], poly[j][1], a0), map3(plane, poly[k][0], poly[k][1], a0));
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

/// A flat octagon disc facing ±u, `r` radius, centred at `(v, z)` on the panel face at `u = uf`,
/// standing `sign * 0.2 mm` proud (a blind shelf-pin hole, spec §B6 — 8 tris, no boolean).
fn push_disc(mesh: &mut SolidMesh, part: u32, uf: f32, vc: f32, zc: f32, r: f32, sign: f32) {
    let up = uf + sign * 0.0002;
    let nrm = [sign, 0.0, 0.0];
    let ring: Vec<[f32; 3]> = (0..8)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / 8.0;
            [up, vc + r * a.cos(), zc + r * a.sin()]
        })
        .collect();
    let center = [up, vc, zc];
    for k in 0..8 {
        let (b, c) = if sign >= 0.0 { (ring[k], ring[(k + 1) % 8]) } else { (ring[(k + 1) % 8], ring[k]) };
        for v in [center, b, c] {
            mesh.positions.push(v);
            mesh.normals.push(nrm);
        }
        mesh.face_ids.push(part);
    }
}

/// The carcass shell (sides, top, bottom) per [`PanelOrder`], the rebated back, and any dividers /
/// rails on the interior boundaries. All `carc`.
fn build_carcass(mesh: &mut SolidMesh, carc: u32, inp: &CabinInput, d: f32) {
    let (w, h) = (inp.width, inp.height);
    match inp.panel_order {
        PanelOrder::SidesOutside => {
            push_box(mesh, carc, [0.0, BOARD], [0.0, d], [0.0, h]); // left side
            push_box(mesh, carc, [w - BOARD, w], [0.0, d], [0.0, h]); // right side
            push_box(mesh, carc, [BOARD, w - BOARD], [0.0, d], [0.0, BOARD]); // bottom
            push_box(mesh, carc, [BOARD, w - BOARD], [0.0, d], [h - BOARD, h]); // top
        }
        PanelOrder::TopBottomOutside => {
            push_box(mesh, carc, [0.0, w], [0.0, d], [0.0, BOARD]); // bottom
            push_box(mesh, carc, [0.0, w], [0.0, d], [h - BOARD, h]); // top
            push_box(mesh, carc, [0.0, BOARD], [0.0, d], [BOARD, h - BOARD]); // left side
            push_box(mesh, carc, [w - BOARD, w], [0.0, d], [BOARD, h - BOARD]); // right side
        }
    }
    // Back in a rebate — not flush with the rear face.
    push_box(mesh, carc, [BOARD, w - BOARD], [BACK_INSET, BACK_INSET + BACK_T], [BOARD, h - BOARD]);

    // Dividers on interior column boundaries (centred), full interior height & depth.
    let cx = cumulative(&inp.cols);
    let (iu0, iu1) = (BOARD, w - BOARD);
    let (iz0, iz1) = (BOARD, h - BOARD);
    for i in 1..inp.cols.len() {
        let u = iu0 + (iu1 - iu0) * cx[i];
        push_box(mesh, carc, [u - BOARD / 2.0, u + BOARD / 2.0], [0.0, d], [iz0, iz1]);
    }
    // Rails on interior row boundaries (centred), full interior width.
    let rz = cumulative(&inp.rows);
    for i in 1..inp.rows.len() {
        let z = iz1 - (iz1 - iz0) * rz[i]; // rows are TOP-first
        push_box(mesh, carc, [iu0, iu1], [BACK_INSET + BACK_T, d], [z - BOARD / 2.0, z + BOARD / 2.0]);
    }
}

/// Shelves (per non-drawer cell) inset behind the leaf. `carc`.
fn build_shelves(mesh: &mut SolidMesh, carc: u32, inp: &CabinInput, d: f32) {
    if inp.shelves == 0 {
        return;
    }
    let cx = cumulative(&inp.cols);
    let rz = cumulative(&inp.rows);
    let (iu0, iu1) = (BOARD, inp.width - BOARD);
    let (iz0, iz1) = (BOARD, inp.height - BOARD);
    for (r, row) in inp.layout.iter().enumerate() {
        for (c, &cell) in row.iter().enumerate() {
            if cell.is_drawer() || cell == Cell::Panel {
                continue;
            }
            let u0 = iu0 + (iu1 - iu0) * cx[c] + BOARD / 2.0;
            let u1 = iu0 + (iu1 - iu0) * cx[c + 1] - BOARD / 2.0;
            let ztop = iz1 - (iz1 - iz0) * rz[r];
            let zbot = iz1 - (iz1 - iz0) * rz[r + 1];
            let n = inp.shelves as usize;
            for k in 0..n {
                let z = zbot + (ztop - zbot) * (k + 1) as f32 / (n + 1) as f32;
                push_box(mesh, carc, [u0, u1], [BACK_INSET + BACK_T, d - 0.020], [z, z + SHELF_T]);
            }
        }
    }
}

/// Batched 32 mm shelf-pin rows (spec §B6): two rows on every vertical panel FACE — both outer
/// sides and both faces of each inner divider — as dark discs proud of the face. One `part`.
fn build_pins(mesh: &mut SolidMesh, pin: u32, inp: &CabinInput, d: f32) {
    let (iz0, iz1) = (BOARD, inp.height - BOARD);
    let rows_v = [PIN_FROM_FACE, d - PIN_FROM_FACE];
    let per = ((iz1 - iz0) / PIN_PITCH).floor() as usize;
    let z_at = |k: usize| iz0 + PIN_PITCH * k as f32 + (iz1 - iz0 - PIN_PITCH * per as f32) / 2.0;

    // A vertical face at u = uf, discs standing off toward `sign`.
    let mut face = |uf: f32, sign: f32| {
        for &vc in &rows_v {
            for k in 0..=per {
                push_disc(mesh, pin, uf, vc, z_at(k), PIN_DIA / 2.0, sign);
            }
        }
    };
    face(BOARD, 1.0); // left side inner face → +u
    face(inp.width - BOARD, -1.0); // right side inner face → −u
    let cx = cumulative(&inp.cols);
    let (iu0, iu1) = (BOARD, inp.width - BOARD);
    for i in 1..inp.cols.len() {
        let u = iu0 + (iu1 - iu0) * cx[i];
        face(u - BOARD / 2.0, -1.0); // divider left face
        face(u + BOARD / 2.0, 1.0); // divider right face
    }
}

/// Append `src` into `dst`, rotated `ang` (rad) about a vertical axis at `pivot=(u,v)` and then
/// translated `+dv` in v (drawer pull). Remaps part ids. Rigid → winding/normals preserved.
fn append_posed(dst: &mut SolidMesh, dmats: &mut Vec<Material>, src: &SolidMesh, smats: &[Material], pivot: [f32; 2], ang: f32, dv: f32) {
    let base = dmats.len() as u32;
    dmats.extend_from_slice(smats);
    let (s, c) = ang.sin_cos();
    for p in &src.positions {
        let (x, y) = (p[0] - pivot[0], p[1] - pivot[1]);
        dst.positions.push([x * c - y * s + pivot[0], x * s + y * c + pivot[1] + dv, p[2]]);
    }
    for n in &src.normals {
        dst.normals.push([n[0] * c - n[1] * s, n[0] * s + n[1] * c, n[2]]);
    }
    for f in &src.face_ids {
        dst.face_ids.push(f + base);
    }
}

/// Emit ONE leaf (rectangle `[u0,u1] × [z0,z1]` in the front plane) into a fresh mesh in its own
/// local frame, with the chosen grip + optional contrast banding. `d` = carcass depth (front back
/// face). Returns (mesh, mats). Posed by the caller for opening.
fn leaf_mesh(inp: &CabinInput, u0: f32, u1: f32, z0: f32, z1: f32, d: f32) -> (SolidMesh, Vec<Material>) {
    let mut mesh = SolidMesh::default();
    let mut mats: Vec<Material> = Vec::new();
    let front = alloc(&mut mats, Material::Front);
    let (v0, v1) = (d, d + LEAF);
    match inp.grip {
        Grip::JGroove => {
            // A routed section in (v, z), extruded across the leaf width — the finger channel is at
            // the top. Full height, notch near z1. Concave → push_prism ear-clips it.
            let ch = z1 - JG_CHANNEL_H;
            let profile = [
                [v0, z0],
                [v1, z0],
                [v1, z1],
                [v1 - JG_LIP, z1],
                [v1 - JG_LIP, ch],
                [v0 - JG_DEPTH_CUT + JG_LIP, ch],
                [v0 - JG_DEPTH_CUT + JG_LIP, ch - 0.004],
                [v0, ch - 0.004],
            ];
            push_prism(&mut mesh, front, &profile, Plane::Vz, u0, u1);
        }
        _ => {
            // Flat leaf.
            push_box(&mut mesh, front, [u0, u1], [v0, v1], [z0, z1]);
        }
    }
    // Grip add-ons that sit ON a flat leaf.
    match inp.grip {
        Grip::Bar => {
            let metal = alloc(&mut mats, Material::Metal);
            let cu = (u0 + u1) / 2.0;
            let cz = z1 - 0.045;
            push_box(&mut mesh, metal, [cu - BAR_L / 2.0, cu + BAR_L / 2.0], [v1 + BAR_PROJ - BAR_D, v1 + BAR_PROJ], [cz - BAR_D / 2.0, cz + BAR_D / 2.0]);
            for su in [cu - BAR_L / 2.0, cu + BAR_L / 2.0 - BAR_D] {
                push_box(&mut mesh, metal, [su, su + BAR_D], [v1 - OVERLAP, v1 + BAR_PROJ], [cz - BAR_D / 2.0, cz + BAR_D / 2.0]);
            }
        }
        Grip::Rail => {
            // A recessed channel across the top of the leaf.
            let edge = alloc(&mut mats, Material::Edge);
            push_box(&mut mesh, edge, [u0 + 0.010, u1 - 0.010], [v1 - RAIL_DEPTH, v1 - OVERLAP], [z1 - RAIL_H, z1 - 0.004]);
        }
        _ => {}
    }
    // Contrast edge banding — four mitred bars round the leaf face.
    if inp.edge_band == EdgeBand::Contrast {
        let edge = alloc(&mut mats, Material::Edge);
        let t = 0.004;
        let vf = [v1 - OVERLAP, v1 + 0.0006]; // just proud of the face
        push_box(&mut mesh, edge, [u0, u1], vf, [z0, z0 + t]);
        push_box(&mut mesh, edge, [u0, u1], vf, [z1 - t, z1]);
        push_box(&mut mesh, edge, [u0, u0 + t], vf, [z0, z1]);
        push_box(&mut mesh, edge, [u1 - t, u1], vf, [z0, z1]);
    }
    (mesh, mats)
}

/// Build the fronts for one cell: door leaves (split across u, hinges mirror about the centre) or
/// drawer fronts (split across z), each posed for the open state.
#[allow(clippy::too_many_arguments)]
fn build_cell_fronts(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &CabinInput, cell: Cell, u0: f32, u1: f32, z0: f32, z1: f32, d: f32) {
    let open = inp.open_deg.to_radians();
    match cell {
        Cell::Door(_) | Cell::Panel => {
            let n = if let Cell::Door(n) = cell { n.max(1) as usize } else { 1 };
            let w = (u1 - u0) / n as f32;
            for k in 0..n {
                let a = u0 + k as f32 * w + if k > 0 { SHADOW / 2.0 } else { 0.0 };
                let b = u0 + (k + 1) as f32 * w - if k + 1 < n { SHADOW / 2.0 } else { 0.0 };
                let (leaf, lmats) = leaf_mesh(inp, a, b, z0, z1, d);
                // Hinge side mirrors about the cell centre: left half hinges left (+), right half −.
                let (pivot_u, sign) = if (a + b) / 2.0 < (u0 + u1) / 2.0 { (a, 1.0) } else { (b, -1.0) };
                let ang = if matches!(cell, Cell::Door(_)) { sign * open } else { 0.0 };
                append_posed(mesh, mats, &leaf, &lmats, [pivot_u, d], ang, 0.0);
            }
        }
        Cell::Drawers(n) => {
            let n = n.max(1) as usize;
            let h = (z1 - z0) / n as f32;
            for k in 0..n {
                let a = z0 + k as f32 * h + if k > 0 { SHADOW / 2.0 } else { 0.0 };
                let b = z0 + (k + 1) as f32 * h - if k + 1 < n { SHADOW / 2.0 } else { 0.0 };
                let (leaf, lmats) = leaf_mesh(inp, u0, u1, a, b, d);
                append_posed(mesh, mats, &leaf, &lmats, [0.0, 0.0], 0.0, inp.drawer_out);
            }
            // A simple drawer box behind each front when pulled out (bottom + 2 sides + back).
            if inp.drawer_out > 0.001 {
                let carc = alloc(mats, Material::Carcass);
                let box_d = (d - BACK_INSET - BACK_T - 0.02).max(0.05);
                for k in 0..n {
                    let a = z0 + k as f32 * h + 0.02;
                    let vb0 = d - box_d + inp.drawer_out;
                    let vb1 = d + inp.drawer_out - 0.02;
                    push_box(mesh, carc, [u0 + 0.02, u1 - 0.02], [vb0, vb1], [a, a + 0.012]); // bottom
                    push_box(mesh, carc, [u0 + 0.02, u0 + 0.032], [vb0, vb1], [a, a + h * 0.6]); // side
                    push_box(mesh, carc, [u1 - 0.032, u1 - 0.02], [vb0, vb1], [a, a + h * 0.6]); // side
                    push_box(mesh, carc, [u0 + 0.02, u1 - 0.02], [vb0, vb0 + 0.012], [a, a + h * 0.6]); // back
                }
            }
        }
        Cell::Open => {}
    }
}

/// Build the whole unit: carcass + interior + pins + fronts. Returns the mesh (per-component
/// `face_ids`) + the material per part id.
pub fn build(inp: &CabinInput) -> Result<(CabinMetrics, SolidMesh, Vec<Material>), ArchError> {
    let (metrics, _w) = plan(inp)?;
    let d = metrics.carcass_depth;

    let mut mesh = SolidMesh::default();
    let mut mats: Vec<Material> = Vec::new();

    let carc = alloc(&mut mats, Material::Carcass);
    build_carcass(&mut mesh, carc, inp, d);
    build_shelves(&mut mesh, carc, inp, d);
    if inp.pin_rows {
        let pin = alloc(&mut mats, Material::Edge);
        build_pins(&mut mesh, pin, inp, d);
    }

    // ── Fronts tile the OUTLINE (full overlay); shadow gaps only on INTERNAL edges. ──
    let cx = cumulative(&inp.cols);
    let rz = cumulative(&inp.rows);
    let nc = inp.cols.len();
    let nr = inp.rows.len();
    // Front-field boundaries: outer edges get full overlay (proud); internal ones sit on the
    // divider / rail centres so leaves meet over the carcass member.
    let (iu0, iu1) = (BOARD, inp.width - BOARD);
    let (iz0, iz1) = (BOARD, inp.height - BOARD);
    let fu = |c: usize| -> f32 {
        if c == 0 {
            -PROUD
        } else if c == nc {
            inp.width + PROUD
        } else {
            iu0 + (iu1 - iu0) * cx[c]
        }
    };
    let fz = |r: usize| -> f32 {
        // r counts rows TOP-first; return the z of that horizontal boundary.
        if r == 0 {
            inp.height + PROUD
        } else if r == nr {
            -PROUD
        } else {
            iz1 - (iz1 - iz0) * rz[r]
        }
    };

    for (r, row) in inp.layout.iter().enumerate() {
        for (c, &cell) in row.iter().enumerate() {
            if !cell.has_front() {
                continue;
            }
            // Cell rect in the field, then shave the shadow gap off internal edges only.
            let mut u0 = fu(c);
            let mut u1 = fu(c + 1);
            let mut z1 = fz(r); // top (higher z)
            let mut z0 = fz(r + 1); // bottom
            if c > 0 {
                u0 += SHADOW / 2.0;
            }
            if c + 1 < nc {
                u1 -= SHADOW / 2.0;
            }
            if r > 0 {
                z1 -= SHADOW / 2.0;
            }
            if r + 1 < nr {
                z0 += SHADOW / 2.0;
            }
            build_cell_fronts(&mut mesh, &mut mats, inp, cell, u0, u1, z0, z1, d);
        }
    }

    Ok((metrics, mesh, mats))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(m: &SolidMesh) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &m.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    /// Spec §A3/§B2/§B10.1 — the reference box closes: overall W × H × depth_nominal (the leaf is
    /// proud, and `carcass_depth = depth_nominal − leaf`, so the front face lands at depth_nominal).
    #[test]
    fn reference_box_closes() {
        let (m, mesh, mats) = build(&CabinInput::default()).unwrap();
        assert!((m.carcass_depth - (0.300 - LEAF)).abs() < 1e-6);
        let (lo, hi) = bounds(&mesh);
        assert!((hi[0] - lo[0] - 0.600).abs() < 0.002, "width {}", hi[0] - lo[0]);
        assert!((hi[2] - lo[2] - 0.780).abs() < 0.002, "height {}", hi[2] - lo[2]);
        // v spans 0 (carcass back) .. depth_nominal (front face).
        assert!(lo[1].abs() < 1e-4 && (hi[1] - 0.300).abs() < 0.002, "depth {}..{}", lo[1], hi[1]);
        assert_eq!(mesh.face_ids.len(), mesh.tri_count());
        assert!((*mesh.face_ids.iter().max().unwrap() as usize) < mats.len());
        for p in &mesh.positions {
            for v in p {
                assert!(v.is_finite());
            }
        }
    }

    /// Counts derive from the layout; the reference is two leaves, one shelf, and pin rows present.
    #[test]
    fn reference_counts() {
        let (m, _w) = plan(&CabinInput::default()).unwrap();
        assert_eq!(m.door_leaves, 2);
        assert_eq!(m.drawer_fronts, 0);
        assert_eq!(m.shelves, 1);
        assert!(m.pins > 0, "pin rows present");
    }

    /// Full overlay: the fronts cover the whole carcass outline (leaf field ≈ full width & height).
    #[test]
    fn fronts_cover_the_outline() {
        let (_m, mesh, mats) = build(&CabinInput::default()).unwrap();
        // Collect only Front triangles and check their (u, z) extent reaches the outline.
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for t in 0..mesh.tri_count() {
            if mats[mesh.face_ids[t] as usize] != Material::Front {
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
        assert!(lo[0] <= 0.0 && hi[0] >= 0.600, "fronts span the width {}..{}", lo[0], hi[0]);
        assert!(lo[2] <= 0.0 && hi[2] >= 0.780, "fronts span the height {}..{}", lo[2], hi[2]);
    }

    /// J-groove leaf is a real routed section (concave profile) — builds, stays finite, and reaches
    /// back past the carcass front (the finger channel).
    #[test]
    fn j_groove_is_a_section() {
        let inp = CabinInput { grip: Grip::JGroove, ..Default::default() };
        let (m, mesh, _mats) = build(&inp).unwrap();
        let (lo, _hi) = bounds(&mesh);
        // The channel reaches back to carcass_front − depth_cut + lip.
        let reach = m.carcass_depth - JG_DEPTH_CUT + JG_LIP;
        assert!(lo[1] <= reach + 1e-4, "channel reaches back to {}", reach);
        for p in &mesh.positions {
            for v in p {
                assert!(v.is_finite());
            }
        }
    }

    /// A 3×2 grid with a drawer bank + open bay builds, tags cleanly, and derives its counts.
    #[test]
    fn mixed_grid_builds() {
        let inp = CabinInput {
            width: 0.900,
            height: 1.200,
            depth_nominal: 0.400,
            cols: vec![1.0, 1.0, 1.0],
            rows: vec![1.0, 2.0],
            layout: vec![
                vec![Cell::Door(1), Cell::Drawers(3), Cell::Door(1)],
                vec![Cell::Door(1), Cell::Open, Cell::Door(2)],
            ],
            grip: Grip::JGroove,
            shelves: 1,
            ..Default::default()
        };
        let (m, mesh, mats) = build(&inp).unwrap();
        assert_eq!(m.door_leaves, 1 + 1 + 1 + 2);
        assert_eq!(m.drawer_fronts, 3);
        assert_eq!(m.opens, 1);
        assert!(mesh.tri_count() > 0);
        assert!((*mesh.face_ids.iter().max().unwrap() as usize) < mats.len());
        let used: std::collections::HashSet<Material> = mesh.face_ids.iter().map(|&id| mats[id as usize]).collect();
        assert!(used.contains(&Material::Carcass) && used.contains(&Material::Front));
    }

    /// An open door pose swings leaves out of the front plane — the free edge sweeps forward into
    /// the room, so the posed mesh reaches well past the closed unit's front face in +v.
    #[test]
    fn open_pose_swings_the_leaf() {
        let closed = build(&CabinInput::default()).unwrap();
        let open = build(&CabinInput { open_deg: 100.0, ..Default::default() }).unwrap();
        let (_clo, chi) = bounds(&closed.1);
        let (_olo, ohi) = bounds(&open.1);
        assert!(ohi[1] > chi[1] + 0.05, "an open leaf swings forward past the closed front {} -> {}", chi[1], ohi[1]);
    }

    #[test]
    fn rejects_bad_inputs() {
        // Layout shape mismatch (spec §B9 — the assertion that fired for real).
        let bad = CabinInput { rows: vec![1.0, 1.0], layout: vec![vec![Cell::Door(1)]], ..Default::default() };
        assert!(plan(&bad).is_err(), "row/layout mismatch rejected");
        // Too shallow for board + back rebate.
        assert!(plan(&CabinInput { depth_nominal: 0.030, ..Default::default() }).is_err());
        // Hinge angle over 110°.
        assert!(plan(&CabinInput { open_deg: 130.0, ..Default::default() }).is_err());
    }
}
