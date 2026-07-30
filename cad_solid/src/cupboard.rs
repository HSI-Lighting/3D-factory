//! Parametric CABINET / cupboard configurator, ported from `CUPBOARD_BUILD.md` (Part B).
//!
//! The reference is ONE INSTANCE of a general thing: a **grid of bays and tiers where every cell is
//! independently filled** — a solid raised-panel door, a glazed door with a diamond muntin lattice,
//! a stack of drawers, an open niche, or a blank panel. Build the grid, not the cupboard (spec §A1).
//! Counts (doors / drawers / …) are DERIVED from the layout, never entered, so they can never
//! disagree with the geometry (§B1.4).
//!
//! Like [`crate::door`]/[`crate::spiral`] this module is PURE geometry (no CSG, no app types) so it
//! unit-tests against the spec's numeric acceptance table (§B8). It returns a [`SolidMesh`] whose
//! `face_ids` tag each component as a distinct selectable PIECE, plus a parallel [`Material`] per
//! part id so the app can paint wood / glass / chrome (a component is one material throughout).
//!
//! Reference frame (spec §B5): **Z up, metres.** `x = 0` is the cabinet centre, `y = 0` the FRONT
//! face of the carcass (the body extends into `-Y`), `z = 0` the floor. Fronts sit proud, in
//! `y ∈ [0, door_t]`. Everything is BOXES and picture-frame bars — **no boolean anywhere** (§B4);
//! mouldings mitre by overlap. The one non-axis-aligned part is the muntin bar (a diamond-lattice
//! chord clipped to the glass by Liang–Barsky, then built as a rotated prism).

use crate::architecture::ArchError;
use crate::SolidMesh;

/// What fills one cell of the grid (spec §B1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// One solid raised-panel door.
    Door,
    /// One glazed door with a diamond muntin lattice.
    Glass,
    /// A stack of `n` drawer fronts, evenly dividing the cell.
    Drawers(u8),
    /// Left open — nothing is built.
    Niche,
    /// A fixed blank panel.
    Panel,
}

impl Cell {
    pub fn label(self) -> &'static str {
        match self {
            Cell::Door => "Door",
            Cell::Glass => "Glass",
            Cell::Drawers(_) => "Drawers",
            Cell::Niche => "Niche",
            Cell::Panel => "Panel",
        }
    }
}

/// Which surface a component wears — so the app paints one flat colour per piece (§A3: fronts are
/// painted wood, panes are glass, pulls are chrome).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Material {
    Wood,
    Glass,
    Chrome,
}

/// All inputs. `cols`/`rows`/`layout` are the tool (§B1.2); the rest keep the measured reference
/// proportions and rarely change. Metres throughout (the spec's mm defaults ÷ 1000). Not `Copy`
/// because of the layout vectors — the app stores one and clones it to build.
#[derive(Clone, Debug)]
pub struct CupboardInput {
    // ── overall (§B1.1) ──
    /// Carcass width, EXCLUDING the cornice overhang.
    pub width: f32,
    /// Body only — no plinth, no cornice.
    pub carcass_height: f32,
    /// Front face to back.
    pub depth: f32,

    // ── the grid (§B1.2) ──
    /// Relative bay widths; length = number of bays. The bays always fill the width exactly.
    pub cols: Vec<f32>,
    /// Relative tier heights, **top first**; length = number of tiers.
    pub rows: Vec<f32>,
    /// `rows × cols` matrix of cell fills, top row first.
    pub layout: Vec<Vec<Cell>>,

    // ── carcass (§B1.5) ──
    pub panel_t: f32,
    pub back_t: f32,
    pub divider_t: f32,
    pub shelf_t: f32,

    // ── fronts (§B1.5) ──
    pub door_t: f32,
    pub door_gap: f32,
    pub edge_reveal: f32,
    pub stile_w: f32,

    // ── glazing (§B1.5) ──
    pub glass_t: f32,
    pub muntin_w: f32,
    pub muntin_t: f32,
    pub muntin_spacing: f32,

    // ── cornice / plinth (§B1.5) ──
    pub cornice_h: f32,
    pub cornice_proj: f32,
    pub plinth_h: f32,
    pub plinth_setback: f32,

    // ── handles (§B1.5) ──
    pub handles: bool,
    pub handle_l: f32,
    pub handle_proj: f32,
    pub handle_d: f32,
    pub handle_inset: f32,
}

impl Default for CupboardInput {
    /// The reference instance (spec §A2 defaults, mm → m): a 3×3 grid — doors top and bottom, a
    /// glass / niche / glass middle band.
    fn default() -> Self {
        use Cell::*;
        Self {
            width: 1.250,
            carcass_height: 1.890,
            depth: 0.420,
            cols: vec![1.0, 1.0, 1.0],
            rows: vec![1.0, 1.0, 1.0],
            layout: vec![
                vec![Door, Door, Door],
                vec![Glass, Niche, Glass],
                vec![Door, Door, Door],
            ],
            panel_t: 0.018,
            back_t: 0.010,
            divider_t: 0.018,
            shelf_t: 0.018,
            door_t: 0.020,
            door_gap: 0.004,
            edge_reveal: 0.006,
            stile_w: 0.055,
            glass_t: 0.004,
            muntin_w: 0.012,
            muntin_t: 0.010,
            muntin_spacing: 0.140,
            cornice_h: 0.115,
            cornice_proj: 0.090,
            plinth_h: 0.095,
            plinth_setback: 0.022,
            handles: true,
            handle_l: 0.105,
            handle_proj: 0.032,
            handle_d: 0.010,
            handle_inset: 0.035,
        }
    }
}

/// The cornice profile: (fraction of `cornice_proj`, fraction of `cornice_h`) for each stacked
/// step, bottom → top (spec §B1.5 default 5-step moulding).
const CORNICE_STEPS: [(f32, f32); 5] =
    [(1.00, 0.22), (0.86, 0.16), (0.60, 0.30), (0.30, 0.20), (0.12, 0.12)];

/// Parts OVERLAP by this rather than abut, so no coplanar faces z-fight (spec §B6.4).
const OVERLAP: f32 = 0.0004;

/// Derived sizes + the DERIVED counts (spec §B1.4 / §B2). Counts are outputs, never inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct CupboardMetrics {
    pub total_w: f32,
    pub total_h: f32,
    /// Overall aspect H/W (the reference measured 1.468).
    pub aspect: f32,
    /// cornice overhang ÷ carcass width (the reference measured 0.072).
    pub overhang_ratio: f32,
    pub n_cols: usize,
    pub n_rows: usize,
    /// Absolute bay widths (m), left → right.
    pub bay_widths: Vec<f32>,
    /// Absolute tier heights (m), top → bottom.
    pub tier_heights: Vec<f32>,
    pub doors: usize,
    pub glazed: usize,
    pub drawers: usize,
    pub niches: usize,
    pub panels: usize,
    pub total_fronts: usize,
    pub handles: usize,
}

/// Split `[lo, hi]` in the given relative proportions → N+1 boundaries.
fn bounds(rel: &[f32], lo: f32, hi: f32) -> Vec<f32> {
    let tot: f32 = rel.iter().sum();
    let mut out = vec![lo];
    let mut acc = 0.0;
    for &v in rel {
        acc += v;
        out.push(lo + (hi - lo) * acc / tot);
    }
    out
}

/// Everything the geometry needs, derived once from the input (§B2).
struct Grid {
    col_x: Vec<f32>, // n_cols + 1 boundaries, left → right
    row_z: Vec<f32>, // n_rows + 1 boundaries, index 0 = TOP
    gap: f32,
}

fn derive_grid(inp: &CupboardInput) -> Grid {
    let carcass_z0 = inp.plinth_h;
    let carcass_z1 = inp.plinth_h + inp.carcass_height;
    let field_x0 = -inp.width / 2.0 + inp.edge_reveal;
    let field_x1 = inp.width / 2.0 - inp.edge_reveal;
    let field_z0 = carcass_z0 + inp.edge_reveal;
    let field_z1 = carcass_z1 - inp.edge_reveal;
    // `rows` is top-first but Z increases upward: reverse once here, index row 0 = top everywhere.
    let mut rev_rows = inp.rows.clone();
    rev_rows.reverse();
    let mut row_z = bounds(&rev_rows, field_z0, field_z1);
    row_z.reverse();
    Grid {
        col_x: bounds(&inp.cols, field_x0, field_x1),
        row_z,
        gap: inp.door_gap / 2.0,
    }
}

impl Grid {
    /// Front-face rectangle of cell (r, c): (x0, x1, z0, z1), gaps applied (§B2).
    fn cell_rect(&self, r: usize, c: usize) -> (f32, f32, f32, f32) {
        let (x0, x1) = (self.col_x[c], self.col_x[c + 1]);
        let (z1, z0) = (self.row_z[r], self.row_z[r + 1]);
        (x0 + self.gap, x1 - self.gap, z0 + self.gap, z1 - self.gap)
    }
}

/// Validate + derive (spec §B2 / §B7). Hard errors → [`ArchError`]; soft checks → warnings.
pub fn plan(inp: &CupboardInput) -> Result<(CupboardMetrics, Vec<String>), ArchError> {
    let n_cols = inp.cols.len();
    let n_rows = inp.rows.len();
    if n_cols == 0 || n_rows == 0 {
        return Err(ArchError::Invalid("cupboard needs at least one bay and one tier"));
    }
    if inp.layout.len() != n_rows || inp.layout.iter().any(|r| r.len() != n_cols) {
        return Err(ArchError::Invalid("layout shape must be rows × cols"));
    }
    if inp.width <= 0.0 || inp.carcass_height <= 0.0 || inp.depth <= 0.0 {
        return Err(ArchError::NonPositive("cupboard size"));
    }
    if inp.depth <= 2.0 * inp.panel_t {
        return Err(ArchError::NonPositive("depth (must exceed twice the board thickness)"));
    }
    for row in &inp.layout {
        for cell in row {
            if let Cell::Drawers(n) = cell {
                if *n < 1 {
                    return Err(ArchError::Invalid("a drawer cell needs at least one drawer"));
                }
            }
        }
    }

    let grid = derive_grid(inp);
    let total_h = inp.plinth_h + inp.carcass_height + inp.cornice_h;
    let total_w = inp.width + 2.0 * inp.cornice_proj;

    let (mut doors, mut glazed, mut drawers, mut niches, mut panels) = (0, 0, 0, 0, 0);
    for row in &inp.layout {
        for cell in row {
            match cell {
                Cell::Door => doors += 1,
                Cell::Glass => glazed += 1,
                Cell::Drawers(n) => drawers += *n as usize, // SUM, not count (§B1.4)
                Cell::Niche => niches += 1,
                Cell::Panel => panels += 1,
            }
        }
    }
    let bay_widths: Vec<f32> = (0..n_cols).map(|c| grid.col_x[c + 1] - grid.col_x[c]).collect();
    let tier_heights: Vec<f32> = (0..n_rows).map(|r| grid.row_z[r] - grid.row_z[r + 1]).collect();

    let m = CupboardMetrics {
        total_w,
        total_h,
        aspect: total_h / total_w,
        overhang_ratio: inp.cornice_proj / inp.width,
        n_cols,
        n_rows,
        bay_widths,
        tier_heights,
        doors,
        glazed,
        drawers,
        niches,
        panels,
        total_fronts: doors + glazed + drawers + panels,
        handles: if inp.handles { doors + glazed + drawers } else { 0 },
    };

    // ── warnings (§B7) — build anyway, but report ──
    let mut warn = Vec::new();
    for r in 0..n_rows {
        for c in 0..n_cols {
            let (x0, x1, z0, z1) = grid.cell_rect(r, c);
            let (cw, ch) = (x1 - x0, z1 - z0);
            match inp.layout[r][c] {
                Cell::Door | Cell::Glass => {
                    if cw > 0.600 {
                        warn.push(format!(
                            "cell R{}C{} door is {:.0} mm wide — over 600 mm a single door sags; split the bay",
                            r + 1, c + 1, cw * 1000.0));
                    }
                    if cw.min(ch) < 2.2 * inp.stile_w {
                        warn.push(format!(
                            "cell R{}C{} is too small for a {:.0} mm stile — the frame closes up",
                            r + 1, c + 1, inp.stile_w * 1000.0));
                    }
                }
                Cell::Drawers(n) => {
                    if (ch / n as f32) < 0.090 {
                        warn.push(format!(
                            "cell R{}C{}: {} drawers gives {:.0} mm fronts — too shallow to be useful",
                            r + 1, c + 1, n, ch / n as f32 * 1000.0));
                    }
                }
                _ => {}
            }
        }
    }
    if inp.carcass_height > 2.400 {
        warn.push(format!(
            "carcass {:.0} mm is over 2400 mm — will not stand up under a standard ceiling once tilted",
            inp.carcass_height * 1000.0));
    }
    if inp.depth < 0.300 {
        warn.push(format!("depth {:.0} mm is under 300 mm — too shallow for plates or hanging", inp.depth * 1000.0));
    }
    Ok((m, warn))
}

// ── mesh emitters ───────────────────────────────────────────────────────────────────────────

/// Allocate the next part id and record its material (each PIECE is one material throughout).
fn alloc(mats: &mut Vec<Material>, m: Material) -> u32 {
    mats.push(m);
    (mats.len() - 1) as u32
}

/// An axis-aligned box tagged with `part`, 12 triangles with outward flat normals (as [`crate::door`]).
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

/// A prism: a convex polygon in the XZ plane extruded along Y (depth), tagged with `part`. Outward
/// flat normals via a centroid test (robust for the convex muntin bars — the only rotated part).
fn push_prism_y(mesh: &mut SolidMesh, part: u32, poly: &[[f32; 2]], y0: f32, y1: f32) {
    let n = poly.len();
    if n < 3 || (y1 - y0).abs() < 1e-6 {
        return;
    }
    let (mut cx, mut cz) = (0.0f32, 0.0f32);
    for p in poly {
        cx += p[0];
        cz += p[1];
    }
    let centroid = [cx / n as f32, (y0 + y1) / 2.0, cz / n as f32];
    let vtx = |i: usize, y: f32| [poly[i][0], y, poly[i][1]];

    let mut tri = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
        let sub = |p: [f32; 3], q: [f32; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
        let cross = |u: [f32; 3], w: [f32; 3]| {
            [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]]
        };
        let dot = |u: [f32; 3], w: [f32; 3]| u[0] * w[0] + u[1] * w[1] + u[2] * w[2];
        let tc = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0];
        let out = sub(tc, centroid);
        // Flip winding so the geometric normal points away from the prism centre.
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
        tri(vtx(0, y1), vtx(i, y1), vtx(i + 1, y1)); // front cap
        tri(vtx(0, y0), vtx(i, y0), vtx(i + 1, y0)); // back cap
    }
    for i in 0..n {
        let j = (i + 1) % n;
        tri(vtx(i, y0), vtx(j, y0), vtx(j, y1));
        tri(vtx(i, y0), vtx(j, y1), vtx(i, y1));
    }
}

/// A frame-and-panel FRONT face as four overlapping bars (stiles + rails), no picture-frame profile
/// (§B4 done as boxes). Returns the inner opening `(x0, x1, z0, z1)` the panel/glass fills.
fn frame_bars(mesh: &mut SolidMesh, part: u32, x0: f32, x1: f32, z0: f32, z1: f32, sw: f32, y: [f32; 2]) -> (f32, f32, f32, f32) {
    push_box(mesh, part, [x0, x0 + sw], y, [z0, z1]); // left stile
    push_box(mesh, part, [x1 - sw, x1], y, [z0, z1]); // right stile
    push_box(mesh, part, [x0 + sw - OVERLAP, x1 - sw + OVERLAP], y, [z0, z0 + sw]); // bottom rail
    push_box(mesh, part, [x0 + sw - OVERLAP, x1 - sw + OVERLAP], y, [z1 - sw, z1]); // top rail
    (x0 + sw, x1 - sw, z0 + sw, z1 - sw)
}

/// Liang–Barsky: clip the infinite line through `(px,pz)` with direction `(dx,dz)` to the rectangle.
fn clip_segment(px: f32, pz: f32, dx: f32, dz: f32, x0: f32, x1: f32, z0: f32, z1: f32) -> Option<((f32, f32), (f32, f32))> {
    let (mut t0, mut t1) = (-1e9_f32, 1e9_f32);
    for (p, q) in [(-dx, px - x0), (dx, x1 - px), (-dz, pz - z0), (dz, z1 - pz)] {
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
    }
    if t0 >= t1 {
        return None;
    }
    Some(((px + t0 * dx, pz + t0 * dz), (px + t1 * dx, pz + t1 * dz)))
}

/// The stile width for a cell, clamped so a fixed stile on a narrow front cannot fill the opening
/// (§B6.6).
fn clamp_stile(inp: &CupboardInput, w: f32, h: f32) -> f32 {
    inp.stile_w.min(w * 0.30).min(h * 0.30)
}

/// A solid raised-panel front: a frame of bars + a raised field with a moulded border (§B3 #6-8),
/// all wood.
fn build_solid_front(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &CupboardInput, x0: f32, x1: f32, z0: f32, z1: f32) {
    let frame = alloc(mats, Material::Wood);
    let sw = clamp_stile(inp, x1 - x0, z1 - z0);
    let (px0, px1, pz0, pz1) = frame_bars(mesh, frame, x0, x1, z0, z1, sw, [0.0, inp.door_t]);

    // Raised field standing slightly proud of the leaf face, with a four-bar moulded border.
    let panel = alloc(mats, Material::Wood);
    let pm = inp.panel_mould().min((px1 - px0) / 2.0 - 0.001).min((pz1 - pz0) / 2.0 - 0.001).max(0.0);
    push_box(mesh, panel, [px0, px1], [inp.door_t - 0.006, inp.door_t + 0.0012], [pz0, pz1]); // field
    if pm > 0.0 {
        let my = [inp.door_t - 0.001, inp.door_t + 0.004];
        push_box(mesh, panel, [px0 - OVERLAP, px0 + pm], my, [pz0 - OVERLAP, pz1 + OVERLAP]); // left
        push_box(mesh, panel, [px1 - pm, px1 + OVERLAP], my, [pz0 - OVERLAP, pz1 + OVERLAP]); // right
        push_box(mesh, panel, [px0 - OVERLAP, px1 + OVERLAP], my, [pz0 - OVERLAP, pz0 + pm]); // bottom
        push_box(mesh, panel, [px0 - OVERLAP, px1 + OVERLAP], my, [pz1 - pm, pz1 + OVERLAP]); // top
    }
}

/// A glazed front: a wood frame + a glass pane + a diamond lattice of wood muntins at ±45° (§B3
/// #6,9,10), each lattice line clipped to the glass by Liang–Barsky (§A4 — no boolean).
fn build_glazed_front(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &CupboardInput, x0: f32, x1: f32, z0: f32, z1: f32) {
    let frame = alloc(mats, Material::Wood);
    let sw = clamp_stile(inp, x1 - x0, z1 - z0);
    let (fx0, fx1, fz0, fz1) = frame_bars(mesh, frame, x0, x1, z0, z1, sw, [0.0, inp.door_t]);

    // Glass pane, tucked slightly under the frame edge, at mid-depth.
    let glass = alloc(mats, Material::Glass);
    let (gx0, gx1) = (fx0 - 0.002, fx1 + 0.002);
    let (gz0, gz1) = (fz0 - 0.002, fz1 + 0.002);
    let gy = (inp.door_t - inp.glass_t) / 2.0;
    push_box(mesh, glass, [gx0, gx1], [gy, gy + inp.glass_t], [gz0, gz1]);

    // Two families of parallel ±45° lines, each clipped to the glass rectangle.
    let muntins = alloc(mats, Material::Wood);
    let (my0, my1) = (inp.door_t - inp.muntin_t, inp.door_t + 0.0005);
    let span = (gx1 - gx0) + (gz1 - gz0);
    let n = (span / inp.muntin_spacing) as i32 + 2;
    let inv = 1.0 / 2.0_f32.sqrt();
    for &sign in &[1.0_f32, -1.0] {
        let (dx, dz) = (inv, sign * inv);
        for k in -n..=n {
            let ox = gx0 + (-dz) * k as f32 * inp.muntin_spacing;
            let oz = gz0 + dx * k as f32 * inp.muntin_spacing;
            if let Some(((ax, az), (bx, bz))) = clip_segment(ox, oz, dx, dz, gx0, gx1, gz0, gz1) {
                if (bx - ax).hypot(bz - az) < inp.muntin_w {
                    continue;
                }
                // A flat bar `muntin_w` across, from a→b, extruded through the door thickness.
                let (ddx, ddz) = (bx - ax, bz - az);
                let l = ddx.hypot(ddz);
                let (nx, nz) = (-ddz / l * inp.muntin_w / 2.0, ddx / l * inp.muntin_w / 2.0);
                let poly = [[ax + nx, az + nz], [bx + nx, bz + nz], [bx - nx, bz - nz], [ax - nx, az - nz]];
                push_prism_y(mesh, muntins, &poly, my0, my1);
            }
        }
    }
}

/// A shallow bar pull on two standoffs — chrome. `axis` V runs along Z (doors), H along X (drawers):
/// a drawer pull is horizontal and a door pull vertical (§B6.2).
fn build_handle(mesh: &mut SolidMesh, mats: &mut Vec<Material>, inp: &CupboardInput, cx: f32, cz: f32, vertical: bool) {
    let h = alloc(mats, Material::Chrome);
    let (d, l, proj) = (inp.handle_d, inp.handle_l, inp.handle_proj);
    let y_bar = [inp.door_t + proj - d, inp.door_t + proj];
    let y_post = [inp.door_t - OVERLAP, inp.door_t + proj];
    if vertical {
        push_box(mesh, h, [cx - d / 2.0, cx + d / 2.0], y_bar, [cz - l / 2.0, cz + l / 2.0]); // bar
        push_box(mesh, h, [cx - d / 2.0, cx + d / 2.0], y_post, [cz - l / 2.0, cz - l / 2.0 + d]); // lower post
        push_box(mesh, h, [cx - d / 2.0, cx + d / 2.0], y_post, [cz + l / 2.0 - d, cz + l / 2.0]); // upper post
    } else {
        push_box(mesh, h, [cx - l / 2.0, cx + l / 2.0], y_bar, [cz - d / 2.0, cz + d / 2.0]);
        push_box(mesh, h, [cx - l / 2.0, cx - l / 2.0 + d], y_post, [cz - d / 2.0, cz + d / 2.0]);
        push_box(mesh, h, [cx + l / 2.0 - d, cx + l / 2.0], y_post, [cz - d / 2.0, cz + d / 2.0]);
    }
}

/// Build the whole cabinet: carcass, cornice, plinth, fronts and handles (spec §B3), as a triangle
/// soup whose `face_ids` tag each selectable piece, plus the [`Material`] per part id.
pub fn build(inp: &CupboardInput) -> Result<(CupboardMetrics, SolidMesh, Vec<Material>), ArchError> {
    let (m, _w) = plan(inp)?;
    let grid = derive_grid(inp);
    let mut mesh = SolidMesh::default();
    let mut mats: Vec<Material> = Vec::new();

    let hw = inp.width / 2.0;
    let (z0, z1) = (inp.plinth_h, inp.plinth_h + inp.carcass_height);
    let d = inp.depth;
    let pt = inp.panel_t;

    // ── Carcass: sides / top / bottom / back + dividers + shelves — all one wood piece. ──
    let carcass = alloc(&mut mats, Material::Wood);
    push_box(&mut mesh, carcass, [-hw, -hw + pt], [-d, 0.0], [z0, z1]); // left
    push_box(&mut mesh, carcass, [hw - pt, hw], [-d, 0.0], [z0, z1]); // right
    push_box(&mut mesh, carcass, [-hw, hw], [-d, 0.0], [z1 - pt, z1]); // top
    push_box(&mut mesh, carcass, [-hw, hw], [-d, 0.0], [z0, z0 + pt]); // bottom
    push_box(&mut mesh, carcass, [-hw, hw], [-d, -d + inp.back_t], [z0, z1]); // back
    for c in 1..grid.col_x.len() - 1 {
        let x = grid.col_x[c];
        push_box(&mut mesh, carcass, [x - inp.divider_t / 2.0, x + inp.divider_t / 2.0], [-d + inp.back_t, 0.0], [z0 + pt, z1 - pt]);
    }
    for r in 1..grid.row_z.len() - 1 {
        let z = grid.row_z[r];
        push_box(&mut mesh, carcass, [-hw + pt, hw - pt], [-d + inp.back_t, 0.0], [z - inp.shelf_t / 2.0, z + inp.shelf_t / 2.0]);
    }

    // ── Cornice: stacked steps, projecting front + both sides, back flush against the wall. ──
    let cornice = alloc(&mut mats, Material::Wood);
    let mut cz = z1;
    for (pf, hf) in CORNICE_STEPS {
        let p = inp.cornice_proj * pf;
        let hgt = inp.cornice_h * hf;
        push_box(&mut mesh, cornice, [-hw - p, hw + p], [-d, p], [cz, cz + hgt + OVERLAP]);
        cz += hgt;
    }

    // ── Plinth: set back from the carcass face. ──
    let plinth = alloc(&mut mats, Material::Wood);
    let phw = hw - inp.plinth_setback;
    push_box(&mut mesh, plinth, [-phw, phw], [-d, -inp.plinth_setback], [0.0, inp.plinth_h + OVERLAP]);

    // ── Fronts: tile the front FACE by cell boundary (overlay, covering the dividers — §B2). ──
    for r in 0..grid.row_z.len() - 1 {
        for c in 0..grid.col_x.len() - 1 {
            let (x0, x1, cz0, cz1) = grid.cell_rect(r, c);
            match inp.layout[r][c] {
                Cell::Niche => {}
                Cell::Panel => {
                    let p = alloc(&mut mats, Material::Wood);
                    push_box(&mut mesh, p, [x0, x1], [0.0, inp.door_t], [cz0, cz1]);
                }
                Cell::Drawers(n) => {
                    let n = n.max(1) as usize;
                    let h = (cz1 - cz0 - inp.door_gap * (n as f32 - 1.0)) / n as f32;
                    for i in 0..n {
                        let dz0 = cz0 + i as f32 * (h + inp.door_gap);
                        build_solid_front(&mut mesh, &mut mats, inp, x0, x1, dz0, dz0 + h);
                        if inp.handles {
                            build_handle(&mut mesh, &mut mats, inp, (x0 + x1) / 2.0, dz0 + h / 2.0, false);
                        }
                    }
                }
                Cell::Door | Cell::Glass => {
                    if inp.layout[r][c] == Cell::Door {
                        build_solid_front(&mut mesh, &mut mats, inp, x0, x1, cz0, cz1);
                    } else {
                        build_glazed_front(&mut mesh, &mut mats, inp, x0, x1, cz0, cz1);
                    }
                    if inp.handles {
                        // Handle on the OPENING edge, opposite the auto hinge (§B1.3).
                        let hinge_left = (c as f32 + 0.5) < inp.cols.len() as f32 / 2.0;
                        let hx = if hinge_left { x1 - inp.handle_inset } else { x0 + inp.handle_inset };
                        build_handle(&mut mesh, &mut mats, inp, hx, (cz0 + cz1) / 2.0, true);
                    }
                }
            }
        }
    }

    Ok((m, mesh, mats))
}

impl CupboardInput {
    /// Panel moulding width — a fixed fraction of the stile (kept out of the public params, which
    /// already carry enough headline sizes).
    fn panel_mould(&self) -> f32 {
        self.stile_w * 0.45
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec §B8 numeric acceptance — the reference layout must reproduce the measured proportions.
    #[test]
    fn reference_matches_spec() {
        let (m, _w) = plan(&CupboardInput::default()).unwrap();
        let mm = |v: f32| v * 1000.0;
        assert!((mm(m.total_w) - 1430.0).abs() < 1.0, "total W {}", mm(m.total_w));
        assert!((mm(m.total_h) - 2100.0).abs() < 1.0, "total H {}", mm(m.total_h));
        assert!((m.aspect - 1.4685).abs() < 0.001, "aspect {}", m.aspect);
        assert!((m.overhang_ratio - 0.0720).abs() < 0.001, "overhang ratio {}", m.overhang_ratio);
        for w in &m.bay_widths {
            assert!((mm(*w) - 413.0).abs() < 1.0, "bay width {}", mm(*w));
        }
        for h in &m.tier_heights {
            assert!((mm(*h) - 626.0).abs() < 1.0, "tier height {}", mm(*h));
        }
        assert_eq!((m.doors, m.glazed, m.drawers, m.niches), (6, 2, 0, 1));
        assert_eq!(m.total_fronts, 8);
        assert_eq!(m.handles, 8);
    }

    /// Spec §B8 parametric rows — counts are derived from the layout, and the bays/tiers always
    /// fill the field exactly (proving it is a configurator, not one baked cupboard).
    #[test]
    fn parametric_counts_and_fill() {
        use Cell::*;
        let cases: Vec<(CupboardInput, (usize, usize, usize, usize))> = vec![
            (CupboardInput::default(), (6, 2, 0, 1)),
            (
                CupboardInput {
                    cols: vec![1.0, 1.0],
                    rows: vec![1.35, 1.0],
                    layout: vec![vec![Glass, Glass], vec![Drawers(3), Drawers(3)]],
                    ..Default::default()
                },
                (0, 2, 6, 0),
            ),
            (
                CupboardInput {
                    cols: vec![1.0],
                    rows: vec![1.0, 2.0],
                    layout: vec![vec![Drawers(2)], vec![Door]],
                    ..Default::default()
                },
                (1, 0, 2, 0),
            ),
        ];
        for (inp, (d, g, dr, ni)) in cases {
            let (m, _w) = plan(&inp).unwrap();
            assert_eq!((m.doors, m.glazed, m.drawers, m.niches), (d, g, dr, ni));
            let field_w = inp.width - 2.0 * inp.edge_reveal;
            let field_h = inp.carcass_height - 2.0 * inp.edge_reveal;
            assert!((m.bay_widths.iter().sum::<f32>() - field_w).abs() < 1e-4, "bays fill width");
            assert!((m.tier_heights.iter().sum::<f32>() - field_h).abs() < 1e-4, "tiers fill height");
        }
    }

    /// The build yields a non-empty mesh, one material per part id, one part id per triangle, all
    /// coordinates finite, and every material present in the reference (wood + glass + chrome).
    #[test]
    fn build_yields_a_tagged_mesh() {
        let (_m, mesh, mats) = build(&CupboardInput::default()).unwrap();
        assert!(mesh.tri_count() > 0);
        assert_eq!(mesh.face_ids.len(), mesh.tri_count(), "one part id per triangle");
        let max_part = *mesh.face_ids.iter().max().unwrap() as usize;
        assert!(max_part < mats.len(), "every part id has a material");
        for p in &mesh.positions {
            for v in p {
                assert!(v.is_finite());
            }
        }
        let used: std::collections::HashSet<Material> =
            mesh.face_ids.iter().map(|&id| mats[id as usize]).collect();
        for want in [Material::Wood, Material::Glass, Material::Chrome] {
            assert!(used.contains(&want), "mesh carries {want:?}");
        }
    }

    /// A drawer cell yields one front + one HORIZONTAL pull per drawer; a door yields a VERTICAL
    /// pull. Distinct chrome-part counts prove drawers were not built as doors (§B6.2).
    #[test]
    fn drawers_and_doors_differ() {
        use Cell::*;
        let (_m, mesh, mats) = build(&CupboardInput {
            cols: vec![1.0],
            rows: vec![1.0, 1.0],
            layout: vec![vec![Door], vec![Drawers(3)]],
            ..Default::default()
        })
        .unwrap();
        let chrome_parts: std::collections::HashSet<u32> = mesh
            .face_ids
            .iter()
            .filter(|&&id| mats[id as usize] == Material::Chrome)
            .copied()
            .collect();
        // 1 door handle + 3 drawer handles = 4 distinct chrome pieces.
        assert_eq!(chrome_parts.len(), 4, "one pull per front");
    }

    #[test]
    fn rejects_bad_inputs() {
        use Cell::*;
        assert!(plan(&CupboardInput { layout: vec![vec![Door, Door]], ..Default::default() }).is_err(), "layout shape");
        assert!(plan(&CupboardInput { width: 0.0, ..Default::default() }).is_err(), "zero width");
        assert!(plan(&CupboardInput { depth: 0.01, ..Default::default() }).is_err(), "depth ≤ 2·panel_t");
        assert!(
            plan(&CupboardInput {
                cols: vec![1.0],
                rows: vec![1.0],
                layout: vec![vec![Drawers(0)]],
                ..Default::default()
            })
            .is_err(),
            "zero drawers"
        );
    }
}
