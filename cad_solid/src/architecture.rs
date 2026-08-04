//! Architectural generators — staircases (straight / U-shape switchback), spiral stairs, and
//! ramps — emitted as a neutral [`SolidMesh`] (triangle soup, metres, **Z-up**) ready to drop into
//! 3D Factory as a placed object.
//!
//! ## Axis convention
//! This crate is **Z-up** (see the module docs on [`SolidMesh`]: "metres, Z-up"), so — unlike the
//! Y-up wording in some briefs — height rises along **+Z**. The mapping used throughout here is:
//!
//! * **X** — stair *width* (side to side),
//! * **Y** — the *run* (horizontal travel; a flight climbs along ±Y),
//! * **Z** — *up* (riser height / floor-to-floor).
//!
//! ## Normals
//! Furniture shading uses `0.6 + 0.4·|n·light|` — the **sign of the normal is irrelevant** — so
//! every triangle simply carries its flat geometric face normal (`cross(b−a, c−a)`); no winding
//! bookkeeping is needed for correct lighting.
//!
//! Each component (tread, riser, landing slab, stringer, ramp deck, spiral tread) is a closed
//! convex body, so the assembled mesh has a well-defined bounding box and a positive enclosed
//! volume ([`mesh_volume`]) — the properties the acceptance tests assert.

use glam::{Mat3, Vec3};

use crate::SolidMesh;

/// Hard cap on generated steps — a runaway `total_height / desired_riser_height` (e.g. a near-zero
/// riser) must not try to build millions of boxes and hang the UI.
pub const MAX_STEPS: usize = 1000;

/// Why an architectural build was rejected. Inputs are validated up front so a bad dialog value
/// yields a clear message instead of a degenerate or gigantic mesh.
#[derive(Debug, Clone, PartialEq)]
pub enum ArchError {
    /// A dimension that must be strictly positive was `<= 0` (field name given).
    NonPositive(&'static str),
    /// U-shape landing shorter than one stair width — no room for the 180° turn.
    LandingTooShort { landing_depth: f32, min: f32 },
    /// The step count exceeded [`MAX_STEPS`] (riser far too small for the height).
    TooManySteps(usize),
    /// A structural invariant was violated (message given) — e.g. a cupboard layout whose shape
    /// does not match its row/column counts.
    Invalid(&'static str),
}

impl std::fmt::Display for ArchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchError::NonPositive(field) => write!(f, "{field} must be greater than 0"),
            ArchError::LandingTooShort { landing_depth, min } => write!(
                f,
                "landing depth {landing_depth:.3} m is too short — needs at least the stair width ({min:.3} m) to turn"
            ),
            ArchError::TooManySteps(n) => {
                write!(f, "{n} steps exceeds the {MAX_STEPS}-step limit — increase the riser height")
            }
            ArchError::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ArchError {}

// ─── Straight / U-shape staircase ───────────────────────────────────────────────────────────

/// Straight single flight, or a U-shape switchback (two flights + landing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StairLayout {
    Straight,
    UShape,
}

impl StairLayout {
    pub const ALL: [StairLayout; 2] = [StairLayout::Straight, StairLayout::UShape];
    pub fn label(self) -> &'static str {
        match self {
            StairLayout::Straight => "Straight",
            StairLayout::UShape => "U-shape (landing)",
        }
    }
}

/// All inputs for a normal staircase. `landing_depth` / `split_ratio` are read only for
/// [`StairLayout::UShape`].
#[derive(Clone, Copy, Debug)]
pub struct StairParams {
    pub layout: StairLayout,
    /// Floor-to-floor vertical height (m).
    pub total_height: f32,
    /// Stair width, side to side (m) — along X.
    pub step_width: f32,
    /// Going: horizontal run of one tread (m) — along Y.
    pub step_depth: f32,
    /// Target riser height (m); the exact riser is derived from `ceil(total_height/this)`.
    pub desired_riser_height: f32,
    /// Slab thickness of a tread (m).
    pub thickness_tread: f32,
    /// Thickness of a vertical riser face (m).
    pub thickness_riser: f32,
    /// Add triangular side stringers (the solid side slabs under the flights). Off by default —
    /// the reference stairs are open-sided with a balustrade instead.
    pub has_stringers: bool,
    /// Add a balustrade (newel posts + sloped top rail + per-step balusters) down both sides.
    pub has_handrails: bool,
    /// Height of the handrail above the tread nosing (m).
    pub handrail_height: f32,
    /// U-shape only: run-length of the flat landing between flights (m). Must be `>= step_width`.
    pub landing_depth: f32,
    /// U-shape only: fraction of steps in the FIRST flight (0..1); 0.5 splits evenly.
    pub split_ratio: f32,
}

impl Default for StairParams {
    fn default() -> Self {
        Self {
            layout: StairLayout::Straight,
            total_height: 3.0,
            step_width: 1.0,
            step_depth: 0.28,
            desired_riser_height: 0.18,
            thickness_tread: 0.05,
            thickness_riser: 0.03,
            has_stringers: false,
            has_handrails: true,
            handrail_height: 0.9,
            landing_depth: 1.2,
            split_ratio: 0.5,
        }
    }
}

/// Derived quantities for a staircase — surfaced in the UI as live feedback and reused by the
/// builder so the numbers on screen and the geometry never disagree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StairPlan {
    /// Total number of steps (`ceil(total_height / desired_riser_height)`).
    pub num_steps: usize,
    /// Exact riser height actually used (`total_height / num_steps`).
    pub riser_height: f32,
    /// Total horizontal run footprint (m).
    pub total_run: f32,
    /// Height of the landing above the floor (U-shape) — 0 for straight.
    pub landing_height: f32,
    /// Steps in the first flight (== num_steps for straight).
    pub flight1_steps: usize,
    /// Steps in the second flight (0 for straight).
    pub flight2_steps: usize,
}

/// Validate inputs and compute the [`StairPlan`] without building geometry — the single source of
/// the step count / riser math for both the live UI readout and [`build_stairs`].
pub fn plan_stairs(p: &StairParams) -> Result<StairPlan, ArchError> {
    if p.total_height <= 0.0 { return Err(ArchError::NonPositive("total_height")); }
    if p.step_width <= 0.0 { return Err(ArchError::NonPositive("step_width")); }
    if p.step_depth <= 0.0 { return Err(ArchError::NonPositive("step_depth")); }
    if p.desired_riser_height <= 0.0 { return Err(ArchError::NonPositive("desired_riser_height")); }
    if p.thickness_tread <= 0.0 { return Err(ArchError::NonPositive("thickness_tread")); }

    let num_steps = (p.total_height / p.desired_riser_height).ceil().max(1.0) as usize;
    if num_steps > MAX_STEPS { return Err(ArchError::TooManySteps(num_steps)); }
    let riser_height = p.total_height / num_steps as f32;

    let (flight1_steps, flight2_steps) = match p.layout {
        StairLayout::Straight => (num_steps, 0),
        StairLayout::UShape => {
            if p.landing_depth < p.step_width {
                return Err(ArchError::LandingTooShort { landing_depth: p.landing_depth, min: p.step_width });
            }
            if num_steps < 2 { return Err(ArchError::NonPositive("total steps (U-shape needs at least 2)")); }
            // ceil for the first flight (matches "N1 = ceil(N·ratio)"), clamped so BOTH flights
            // keep at least one step.
            let ratio = p.split_ratio.clamp(0.05, 0.95);
            let n1 = ((num_steps as f32 * ratio).ceil() as usize).clamp(1, num_steps - 1);
            (n1, num_steps - n1)
        }
    };

    let landing_height = match p.layout {
        StairLayout::Straight => 0.0,
        StairLayout::UShape => flight1_steps as f32 * riser_height,
    };
    let total_run = match p.layout {
        StairLayout::Straight => num_steps as f32 * p.step_depth,
        // The two flights double back, so the footprint depth is the longer flight + the landing.
        StairLayout::UShape => flight1_steps.max(flight2_steps) as f32 * p.step_depth + p.landing_depth,
    };

    Ok(StairPlan { num_steps, riser_height, total_run, landing_height, flight1_steps, flight2_steps })
}

/// Build the staircase mesh (metres, Z-up), resting on z = 0 with the near end at y = 0.
///
/// For [`StairLayout::UShape`] the second flight is offset one width in +X and runs in −Y, joined
/// by a landing that spans both flights — a real switchback stairwell. See the module docs and
/// [`build_flight`] for the per-step matrices.
pub fn build_stairs(p: &StairParams) -> Result<SolidMesh, ArchError> {
    let plan = plan_stairs(p)?;
    let mut m = SolidMesh::default();
    let w = p.step_width;
    let riser = plan.riser_height;
    let part = &mut 0u32; // per-primitive id → each tread/riser/baluster is its own selectable piece

    match p.layout {
        StairLayout::Straight => {
            build_flight(&mut m, p, plan.flight1_steps, riser, 0.0, 0.0, 0.0, 1.0, part);
            if p.has_stringers {
                add_stringers(&mut m, p, plan.flight1_steps, riser, 0.0, 0.0, 0.0, 1.0, part);
            }
            if p.has_handrails {
                add_handrail(&mut m, p, plan.flight1_steps, riser, 0.0, 0.0, 0.0, 1.0, part);
            }
        }
        StairLayout::UShape => {
            let sd = p.step_depth;
            // Flight 1: width lane x∈[0, w], climbing +Y from y = 0, base z = 0.
            build_flight(&mut m, p, plan.flight1_steps, riser, 0.0, 0.0, 0.0, 1.0, part);

            // Landing: a level slab at the top of flight 1, spanning BOTH width lanes so it joins
            // the two flights. Its run-extent starts where flight 1 ends and is `landing_depth` long.
            let y_land0 = plan.flight1_steps as f32 * sd;
            let z_land = plan.landing_height;
            push_aabb(
                &mut m,
                Vec3::new(0.0, y_land0, z_land - p.thickness_tread),
                Vec3::new(2.0 * w, y_land0 + p.landing_depth, z_land),
            );
            seal_part(&mut m, part);

            // Flight 2: offset one width in +X (the second lane), starting at the FAR edge of the
            // landing and running back in −Y, climbing from the landing height to the top floor.
            let y2 = y_land0 + p.landing_depth;
            build_flight(&mut m, p, plan.flight2_steps, riser, w, z_land, y2, -1.0, part);
            if p.has_stringers {
                add_stringers(&mut m, p, plan.flight1_steps, riser, 0.0, 0.0, 0.0, 1.0, part);
                add_stringers(&mut m, p, plan.flight2_steps, riser, w, z_land, y2, -1.0, part);
            }
            if p.has_handrails {
                add_handrail(&mut m, p, plan.flight1_steps, riser, 0.0, 0.0, 0.0, 1.0, part);
                add_handrail(&mut m, p, plan.flight2_steps, riser, w, z_land, y2, -1.0, part);
            }
        }
    }
    Ok(m)
}

/// Emit one flight's treads + risers.
///
/// * `x0` — the near-side X of this flight's width lane (spans `x0..x0+step_width`).
/// * `base_z` — floor height this flight starts from (0 for flight 1, the landing height for 2).
/// * `y_start` — the run coordinate of the flight's FIRST nosing.
/// * `dir` — climb direction along Y: `+1.0` forward, `−1.0` back (the U-shape return flight).
///
/// Step `i` (0-based): tread top at `base_z + (i+1)·riser`, its run occupying
/// `[y_start + dir·i·depth , y_start + dir·(i+1)·depth]`. A riser closes the front of every step
/// except the last (the top nosing meets the floor/landing, so no riser is placed there — matching
/// "do NOT place a riser on the last step").
fn build_flight(
    m: &mut SolidMesh,
    p: &StairParams,
    steps: usize,
    riser: f32,
    x0: f32,
    base_z: f32,
    y_start: f32,
    dir: f32,
    part: &mut u32,
) {
    let w = p.step_width;
    let sd = p.step_depth;
    for i in 0..steps {
        let z_top = base_z + (i + 1) as f32 * riser;
        let ya = y_start + dir * i as f32 * sd;
        let yb = y_start + dir * (i + 1) as f32 * sd;
        // Tread: horizontal slab you step onto, hanging `thickness_tread` below its top surface.
        push_aabb(
            m,
            Vec3::new(x0, ya.min(yb), z_top - p.thickness_tread),
            Vec3::new(x0 + w, ya.max(yb), z_top),
        );
        seal_part(m, part); // each tread its own piece
        // Riser: vertical face at the FRONT edge (the low-run side, `ya`) of each step but the last.
        if i + 1 < steps {
            let z0 = base_z + i as f32 * riser;
            let front = ya; // nosing edge for this step
            let (ry0, ry1) = if dir >= 0.0 { (front, front + p.thickness_riser) } else { (front - p.thickness_riser, front) };
            push_aabb(m, Vec3::new(x0, ry0, z0), Vec3::new(x0 + w, ry1, z_top));
            seal_part(m, part); // each riser its own piece
        }
    }
}

/// Add the two triangular side stringers under a flight — a right-triangle profile (run × rise)
/// following the underside slope, extruded a small thickness against each outer edge of the treads.
fn add_stringers(
    m: &mut SolidMesh,
    p: &StairParams,
    steps: usize,
    riser: f32,
    x0: f32,
    base_z: f32,
    y_start: f32,
    dir: f32,
    part: &mut u32,
) {
    if steps == 0 { return; }
    let w = p.step_width;
    let run = steps as f32 * p.step_depth;
    let rise = steps as f32 * riser;
    let t = (w * 0.08).clamp(0.03, 0.12); // stringer thickness

    // Profile in (y, z): the solid triangle under the diagonal, from the near-bottom nosing up to
    // the far-top. `dir` mirrors it for the return flight.
    let y_far = y_start + dir * run;
    let profile = [
        [y_start, base_z],
        [y_far, base_z],
        [y_far, base_z + rise],
    ];
    // One stringer flush against each side of the width lane.
    push_prism(m, &profile, x0, x0 + t);
    seal_part(m, part);
    push_prism(m, &profile, x0 + w - t, x0 + w);
    seal_part(m, part);
}

/// Add a balustrade down BOTH sides of a flight: a newel post at each end, a sloped top rail
/// parallel to the pitch, and one baluster per step. Matches the reference open-sided stair. Arg
/// meaning mirrors [`build_flight`] (`x0`, `base_z`, `y_start`, `dir`).
fn add_handrail(
    m: &mut SolidMesh,
    p: &StairParams,
    steps: usize,
    riser: f32,
    x0: f32,
    base_z: f32,
    y_start: f32,
    dir: f32,
    part: &mut u32,
) {
    if steps == 0 { return; }
    let w = p.step_width;
    let sd = p.step_depth;
    let hr = p.handrail_height.max(0.3);
    let run = steps as f32 * sd;
    let rise = steps as f32 * riser;
    // Bar sizes (m): balusters thin, rail chunkier, newels chunkiest.
    let bal = (w * 0.03).clamp(0.02, 0.04);
    let rail_r = (w * 0.04).clamp(0.025, 0.05);
    let newel = (w * 0.06).clamp(0.05, 0.08);
    let inset = newel + 0.01; // keep the rail just inside the tread edge

    // The rail runs parallel to the pitch, `hr` above each nosing: from above tread 1 to above the
    // top tread. Linear in y so balusters can find their exact rail height.
    let rail_z = |y: f32| {
        let f = if run.abs() < 1e-6 { 0.0 } else { (y - y_start) / (dir * run) };
        base_z + riser + hr + f * (rise - riser)
    };

    for &side in &[x0 + inset, x0 + w - inset] {
        // Sloped top rail (a round rail from the first nosing-top to the last).
        let a = Vec3::new(side, y_start, rail_z(y_start));
        let b = Vec3::new(side, y_start + dir * run, rail_z(y_start + dir * run));
        push_rod(m, a, b, rail_r);
        seal_part(m, part); // the rail is its own piece

        // Newel posts: bottom rests on the floor this flight starts from; top on the last tread.
        push_rod(
            m,
            Vec3::new(side, y_start, base_z),
            Vec3::new(side, y_start, rail_z(y_start) + rail_r),
            newel,
        );
        seal_part(m, part);
        let y_top = y_start + dir * run;
        push_rod(
            m,
            Vec3::new(side, y_top, base_z + rise - riser),
            Vec3::new(side, y_top, rail_z(y_top) + rail_r),
            newel,
        );
        seal_part(m, part);

        // One baluster per step, at mid-tread, from the tread top up to the rail.
        for i in 0..steps {
            let yb = y_start + dir * (i as f32 + 0.5) * sd;
            let tread_top = base_z + (i + 1) as f32 * riser;
            let top = rail_z(yb);
            if top - tread_top > bal {
                push_rod(m, Vec3::new(side, yb, tread_top), Vec3::new(side, yb, top), bal);
                seal_part(m, part); // each baluster its own piece
            }
        }
    }
}

// ─── Spiral staircase ───────────────────────────────────────────────────────────────────────

/// Inputs for a spiral (helical) stair around a central post.
#[derive(Clone, Copy, Debug)]
pub struct SpiralParams {
    pub total_height: f32,
    /// Radial length of each tread outward from the inner radius (m).
    pub step_width: f32,
    /// Inner radius — the clear radius of the central post/void (m).
    pub center_radius: f32,
    /// Treads per full 360° turn.
    pub steps_per_turn: usize,
    /// Number of turns (may be fractional).
    pub total_turns: f32,
    pub thickness_tread: f32,
    /// Add an outer balustrade (per-tread balusters + a helical top rail).
    pub has_handrail: bool,
    /// Height of the handrail above each tread (m).
    pub handrail_height: f32,
}

impl Default for SpiralParams {
    fn default() -> Self {
        Self {
            total_height: 3.0,
            step_width: 0.9,
            center_radius: 0.2,
            steps_per_turn: 12,
            total_turns: 1.0,
            thickness_tread: 0.05,
            has_handrail: true,
            handrail_height: 0.9,
        }
    }
}

/// Derived spiral quantities for the UI: total steps and total rotation (degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpiralPlan {
    pub num_steps: usize,
    pub riser_height: f32,
    pub total_rotation_deg: f32,
}

pub fn plan_spiral(p: &SpiralParams) -> Result<SpiralPlan, ArchError> {
    if p.total_height <= 0.0 { return Err(ArchError::NonPositive("total_height")); }
    if p.step_width <= 0.0 { return Err(ArchError::NonPositive("step_width")); }
    if p.center_radius < 0.0 { return Err(ArchError::NonPositive("center_radius")); }
    if p.steps_per_turn < 3 { return Err(ArchError::NonPositive("steps_per_turn (need at least 3)")); }
    if p.total_turns <= 0.0 { return Err(ArchError::NonPositive("total_turns")); }
    if p.thickness_tread <= 0.0 { return Err(ArchError::NonPositive("thickness_tread")); }

    let num_steps = ((p.steps_per_turn as f32) * p.total_turns).round().max(1.0) as usize;
    if num_steps > MAX_STEPS { return Err(ArchError::TooManySteps(num_steps)); }
    let riser_height = p.total_height / num_steps as f32;
    let total_rotation_deg = num_steps as f32 * (360.0 / p.steps_per_turn as f32);
    Ok(SpiralPlan { num_steps, riser_height, total_rotation_deg })
}

/// Build a spiral stair: wedge-like treads fanned around a central square post, each rotated
/// `2π/steps_per_turn` from the last and lifted one riser. Rises along +Z.
pub fn build_spiral(p: &SpiralParams) -> Result<SolidMesh, ArchError> {
    let plan = plan_spiral(p)?;
    let mut m = SolidMesh::default();
    let part = &mut 0u32; // per-primitive id → each tread / baluster is its own selectable piece
    let dθ = std::f32::consts::TAU / p.steps_per_turn as f32;
    let r_mid = p.center_radius + p.step_width * 0.5;
    // Tangential tread size ≈ the arc it subtends, trimmed 5% so neighbours don't interpenetrate.
    let tang = (dθ * r_mid) * 0.95;

    for k in 0..plan.num_steps {
        let θ = k as f32 * dθ;
        let z_top = (k + 1) as f32 * plan.riser_height;
        let center = Vec3::new(r_mid * θ.cos(), r_mid * θ.sin(), z_top - p.thickness_tread * 0.5);
        let half = Vec3::new(p.step_width * 0.5, tang * 0.5, p.thickness_tread * 0.5);
        push_obb(&mut m, center, half, Mat3::from_rotation_z(θ));
        seal_part(&mut m, part); // each tread its own piece
    }

    // Central post: a round column spanning the full height, sized to the inner radius.
    let post_r = (p.center_radius * 0.9).clamp(0.05, 0.25);
    push_cylinder(&mut m, 0.0, 0.0, post_r, 0.0, p.total_height, 20);
    seal_part(&mut m, part);

    // Outer balustrade: a round baluster at each tread's outer edge, and ONE continuous helical
    // rail swept through their tops (no per-segment gaps).
    if p.has_handrail {
        let hr = p.handrail_height.max(0.3);
        let r_out = (p.center_radius + p.step_width) * 0.97; // just inside the tread's outer edge
        let bal = 0.03_f32;
        let rail_r = 0.035_f32;
        // Rail node above tread k's outer edge.
        let node = |k: usize| -> Vec3 {
            let θ = k as f32 * dθ;
            let z_top = (k + 1) as f32 * plan.riser_height;
            Vec3::new(r_out * θ.cos(), r_out * θ.sin(), z_top + hr)
        };
        for k in 0..plan.num_steps {
            let θ = k as f32 * dθ;
            let z_top = (k + 1) as f32 * plan.riser_height;
            let base = Vec3::new(r_out * θ.cos(), r_out * θ.sin(), z_top - p.thickness_tread);
            push_rod(&mut m, base, node(k), bal);
            seal_part(&mut m, part); // each baluster its own piece
        }
        // A single tube through every node = one unbroken helical handrail.
        let nodes: Vec<Vec3> = (0..plan.num_steps).map(node).collect();
        push_polyline_tube(&mut m, &nodes, rail_r);
        seal_part(&mut m, part);
    }
    Ok(m)
}

// ─── Ramp ───────────────────────────────────────────────────────────────────────────────────

/// Inputs for a straight ramp (an inclined deck).
#[derive(Clone, Copy, Debug)]
pub struct RampParams {
    pub vertical_height: f32,
    pub horizontal_length: f32,
    pub width: f32,
    /// Deck thickness measured perpendicular to the slope (m).
    pub thickness: f32,
}

impl Default for RampParams {
    fn default() -> Self {
        Self { vertical_height: 1.0, horizontal_length: 4.0, width: 1.5, thickness: 0.15 }
    }
}

pub fn ramp_slope_deg(p: &RampParams) -> f32 {
    p.vertical_height.atan2(p.horizontal_length).to_degrees()
}

/// Build a ramp: an inclined slab of `thickness` whose walking surface rises from `(y=0, z=0)` to
/// `(y=horizontal_length, z=vertical_height)`, extruded across `width`. Rests on z = 0.
pub fn build_ramp(p: &RampParams) -> Result<SolidMesh, ArchError> {
    if p.vertical_height <= 0.0 { return Err(ArchError::NonPositive("vertical_height")); }
    if p.horizontal_length <= 0.0 { return Err(ArchError::NonPositive("horizontal_length")); }
    if p.width <= 0.0 { return Err(ArchError::NonPositive("width")); }
    if p.thickness <= 0.0 { return Err(ArchError::NonPositive("thickness")); }

    let (l, h, t) = (p.horizontal_length, p.vertical_height, p.thickness);
    let len = (l * l + h * h).sqrt().max(1e-6);
    // Unit perpendicular to the slope, pointing DOWN-and-forward (below the deck) in (y, z).
    let (perp_y, perp_z) = (h / len * t, -l / len * t);
    // Deck as a parallelogram: top surface (0,0)→(l,h), bottom offset by the perpendicular.
    let mut profile = [
        [0.0, 0.0],
        [l, h],
        [l + perp_y, h + perp_z],
        [perp_y, perp_z],
    ];
    // Lift so the lowest point rests on z = 0.
    let min_z = profile.iter().fold(f32::INFINITY, |a, q| a.min(q[1]));
    for q in &mut profile { q[1] -= min_z; }

    let mut m = SolidMesh::default();
    push_prism(&mut m, &profile, 0.0, p.width);
    seal_part(&mut m, &mut 0u32);
    Ok(m)
}

// ─── Helical (spiral) ramp ───────────────────────────────────────────────────────────────────

/// Which edges of a helical ramp carry a balustrade (spec §B1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalustradeEdges {
    Both,
    OuterOnly,
    InnerOnly,
}

/// Parameters for a helical ramp — a sloped annular deck winding around a free inner edge, with a
/// balustrade on each edge (spec `RAMP_BUILD.md` §B1). The four primaries define it; the rest keep
/// the reference proportions.
#[derive(Clone, Copy, Debug)]
pub struct HelicalRampParams {
    pub ramp_height: f32,
    pub r_inner: f32,
    pub r_outer: f32,
    pub turns: f32,
    pub slab_thickness: f32,
    /// +1 = anticlockwise ascending, −1 = clockwise.
    pub direction: f32,
    pub start_angle: f32,
    pub segments_per_turn: usize,
    pub rail_height: f32,
    pub rail_count: usize,
    pub rail_lowest: f32,
    pub rail_tube_d: f32,
    pub post_d: f32,
    pub post_spacing: f32,
    pub rail_inset: f32,
    pub end_rails: bool,
    pub balustrade_edges: BalustradeEdges,
}

impl Default for HelicalRampParams {
    /// The spec's reference ramp (§B8): 7 m over two turns, 1.75 → 5.0 m radii (ratio 0.35).
    fn default() -> Self {
        Self {
            ramp_height: 7.0,
            r_inner: 1.75,
            r_outer: 5.0,
            turns: 2.0,
            slab_thickness: 0.30,
            direction: 1.0,
            start_angle: 0.0,
            segments_per_turn: 96,
            rail_height: 1.10,
            rail_count: 4,
            rail_lowest: 0.15,
            rail_tube_d: 0.040,
            post_d: 0.040,
            post_spacing: 1.10,
            rail_inset: 0.060,
            end_rails: true,
            balustrade_edges: BalustradeEdges::Both,
        }
    }
}

/// Everything derived from a [`HelicalRampParams`] (spec §B2) — the slope/headroom/post readout the
/// dialog surfaces and the acceptance tests assert.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HelicalRampMetrics {
    pub sweep_deg: f32,
    pub deck_width: f32,
    pub r_mean: f32,
    pub radius_ratio: f32,
    pub pitch_m_per_deg: f32,
    pub path_length: f32,
    pub slope_mean: f32,
    pub slope_inner: f32,
    pub rise_per_turn: f32,
    pub headroom: f32,
    pub top_of_deck: f32,
    pub posts_outer: usize,
    pub posts_inner: usize,
    /// Turns needed to reach a 1:12 slope at the mean radius (spec §B7 "print the fix").
    pub turns_for_1_12: f32,
}

/// Validate + derive a helical ramp (spec §B2 / §B7). Hard errors → [`ArchError`].
pub fn plan_helical_ramp(p: &HelicalRampParams) -> Result<(HelicalRampMetrics, Vec<String>), ArchError> {
    if p.ramp_height <= 0.0 { return Err(ArchError::NonPositive("ramp_height")); }
    if p.slab_thickness <= 0.0 { return Err(ArchError::NonPositive("slab_thickness")); }
    if p.turns <= 0.0 { return Err(ArchError::NonPositive("turns")); }
    if p.r_inner < 0.0 { return Err(ArchError::NonPositive("r_inner")); }
    if p.r_outer <= p.r_inner { return Err(ArchError::NonPositive("r_outer (must exceed r_inner)")); }
    if p.rail_height <= p.rail_lowest { return Err(ArchError::NonPositive("rail_height (must exceed rail_lowest)")); }

    let sweep = p.direction.signum() * 360.0 * p.turns;
    let sweep_rad = sweep.abs().to_radians();
    let r_mean = (p.r_inner + p.r_outer) / 2.0;
    let path_length = sweep_rad * r_mean;
    let inner_len = sweep_rad * p.r_inner.max(1e-3);
    let r_out_rail = (p.r_outer - p.rail_inset).max(1e-3);
    let r_in_rail = (p.r_inner + p.rail_inset).max(1e-3);
    let posts = |r: f32| ((sweep_rad * r) / p.post_spacing.max(1e-3)).round().max(2.0) as usize;
    let m = HelicalRampMetrics {
        sweep_deg: sweep,
        deck_width: p.r_outer - p.r_inner,
        r_mean,
        radius_ratio: p.r_inner / p.r_outer,
        pitch_m_per_deg: p.ramp_height / sweep.abs(),
        path_length,
        slope_mean: p.ramp_height / path_length,
        slope_inner: p.ramp_height / inner_len,
        rise_per_turn: p.ramp_height / p.turns,
        headroom: p.ramp_height / p.turns - p.slab_thickness,
        top_of_deck: p.slab_thickness + p.ramp_height,
        posts_outer: posts(r_out_rail),
        posts_inner: posts(r_in_rail),
        turns_for_1_12: p.ramp_height * 12.0 / (2.0 * std::f32::consts::PI * r_mean),
    };

    let mut warn = Vec::new();
    let one_in = |s: f32| if s > 0.0 { 1.0 / s } else { f32::INFINITY };
    if m.slope_mean > 1.0 / 12.0 {
        warn.push(format!("slope 1:{:.1} at the mean radius is steeper than the 1:12 accessibility limit", one_in(m.slope_mean)));
    }
    if m.slope_inner > 1.0 / 12.0 {
        warn.push(format!("slope 1:{:.1} at the INNER edge (the governing figure — people cut the inside)", one_in(m.slope_inner)));
    }
    if m.headroom < 2.10 {
        warn.push(format!("headroom {:.2} m is below the 2.10 m walk-under minimum", m.headroom));
    }
    if m.deck_width < 1.20 {
        warn.push(format!("deck {:.2} m is below the 1.20 m two-way minimum", m.deck_width));
    }
    if !(0.90..=1.10).contains(&p.rail_height) {
        warn.push(format!("rail {:.2} m is outside the usual 0.90–1.10 m", p.rail_height));
    }
    if m.rise_per_turn > 0.75 {
        warn.push(format!("rise/turn {:.2} m — a continuous helix has no landing (most codes require one above 0.75 m)", m.rise_per_turn));
    }
    warn.push(format!("to reach 1:12 at the mean radius, use {:.2} turns", m.turns_for_1_12));
    Ok((m, warn))
}

/// Build a helical ramp as a [`SolidMesh`] (spec §B3): the annular deck is one continuous sweep
/// (walking surface + both fascias + soffit come from a single solid — no internal joints), plus a
/// balustrade (helical rails, arc-spaced posts, radial end rails) on the chosen edges. Every element
/// takes its height from `deck_top_z(angle)` so one pitch governs the whole assembly (spec §A2/§B6).
pub fn build_helical_ramp(p: &HelicalRampParams) -> Result<SolidMesh, ArchError> {
    let (met, _w) = plan_helical_ramp(p)?; // validates
    let mut m = SolidMesh::default();
    let mut part = 0u32;
    let sweep = met.sweep_deg;
    let start = p.start_angle;
    let t = p.slab_thickness;
    let (ri, ro) = (p.r_inner, p.r_outer);
    // Segment count, clamped so an extreme turns/tessellation can't build a runaway mesh.
    let n = ((p.turns * p.segments_per_turn.max(8) as f32).round() as usize).clamp(4, 8000);

    let ztop = |a_deg: f32| t + (a_deg - start) / sweep * p.ramp_height;
    let pt = |r: f32, a_deg: f32, z: f32| -> [f32; 3] {
        let a = a_deg.to_radians();
        [r * a.cos(), r * a.sin(), z]
    };

    // ── Deck: the slab cross-section swept along the helix — ONE smooth continuous solid (walking
    //    surface, soffit, inner + outer fascia), then the two radial end caps. Not steps. ──
    for i in 0..n {
        let a0 = start + sweep * i as f32 / n as f32;
        let a1 = start + sweep * (i + 1) as f32 / n as f32;
        let (z0, z1) = (ztop(a0), ztop(a1));
        let it0 = pt(ri, a0, z0); let ot0 = pt(ro, a0, z0);
        let ib0 = pt(ri, a0, z0 - t); let ob0 = pt(ro, a0, z0 - t);
        let it1 = pt(ri, a1, z1); let ot1 = pt(ro, a1, z1);
        let ib1 = pt(ri, a1, z1 - t); let ob1 = pt(ro, a1, z1 - t);
        push_quad(&mut m, it0, ot0, ot1, it1); // walking surface
        push_quad(&mut m, ib0, ib1, ob1, ob0); // soffit
        push_quad(&mut m, it0, it1, ib1, ib0); // inner fascia
        push_quad(&mut m, ot0, ob0, ob1, ot1); // outer fascia
    }
    {
        let (a, z) = (start, ztop(start));
        push_quad(&mut m, pt(ri, a, z), pt(ri, a, z - t), pt(ro, a, z - t), pt(ro, a, z)); // bottom cap
        let a = start + sweep; let z = ztop(a);
        push_quad(&mut m, pt(ri, a, z), pt(ro, a, z), pt(ro, a, z - t), pt(ri, a, z - t)); // top cap
    }
    seal_part(&mut m, &mut part); // the whole deck is one swept piece

    // ── Balustrade. Each rail is a gapless helical tube; posts are arc-spaced per edge; the ends
    //    get a straight radial rail per level. Every height comes from `ztop(a) + offset`. ──
    let rail_r = p.rail_tube_d * 0.5;
    let post_r = p.post_d * 0.5;
    let rail_off = |k: usize| -> f32 {
        if p.rail_count <= 1 { p.rail_height } else { p.rail_lowest + (p.rail_height - p.rail_lowest) * k as f32 / (p.rail_count - 1) as f32 }
    };
    let edges: &[bool] = match p.balustrade_edges {
        BalustradeEdges::Both => &[true, true],       // [outer, inner]
        BalustradeEdges::OuterOnly => &[true, false],
        BalustradeEdges::InnerOnly => &[false, true],
    };
    let edge_r = [(ro - p.rail_inset).max(1e-3), (ri + p.rail_inset).max(1e-3)];
    for (e, &on) in edges.iter().enumerate() {
        if !on { continue; }
        let re = edge_r[e];
        // Rails.
        for k in 0..p.rail_count.max(1) {
            let off = rail_off(k);
            let pts: Vec<Vec3> = (0..=n)
                .map(|i| {
                    let a = start + sweep * i as f32 / n as f32;
                    Vec3::from(pt(re, a, ztop(a) + off))
                })
                .collect();
            push_polyline_tube(&mut m, &pts, rail_r);
            seal_part(&mut m, &mut part);
        }
        // Posts — arc-spaced on THIS edge (spec §B6.2). Constant radius → even in angle.
        let np = ((sweep.abs().to_radians() * re) / p.post_spacing.max(1e-3)).round().max(2.0) as usize;
        for j in 0..np {
            let a = start + sweep * j as f32 / np as f32;
            let base = Vec3::from(pt(re, a, ztop(a)));
            let top = Vec3::from(pt(re, a, ztop(a) + p.rail_height));
            push_rod(&mut m, base, top, post_r);
            seal_part(&mut m, &mut part);
        }
    }
    // End rails — radial, inner rail to outer rail at each level, at both ends (needs both edges).
    if p.end_rails && matches!(p.balustrade_edges, BalustradeEdges::Both) {
        for &a in &[start, start + sweep] {
            let z = ztop(a);
            for k in 0..p.rail_count.max(1) {
                let off = rail_off(k);
                let inner = Vec3::from(pt(edge_r[1], a, z + off));
                let outer = Vec3::from(pt(edge_r[0], a, z + off));
                push_rod(&mut m, inner, outer, rail_r);
                seal_part(&mut m, &mut part);
            }
        }
    }

    Ok(m)
}

// ─── Mesh emission primitives ────────────────────────────────────────────────────────────────

/// Tag every triangle emitted since the previous seal with the current part id, then bump it. Each
/// generated primitive (a tread, a riser, a baluster, a rail…) gets its OWN part id in
/// `SolidMesh::face_ids`, so downstream "select a piece" picks one part instead of the whole welded
/// run (the treads/risers share welded edges and would otherwise read as a single connected body).
fn seal_part(m: &mut SolidMesh, part: &mut u32) {
    let ntri = m.positions.len() / 3;
    while m.face_ids.len() < ntri {
        m.face_ids.push(*part);
    }
    *part += 1;
}

/// Push one triangle with its flat geometric normal (sign-independent, per the module note).
fn push_tri(m: &mut SolidMesh, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
    let (va, vb, vc) = (Vec3::from(a), Vec3::from(b), Vec3::from(c));
    let n = (vb - va).cross(vc - va).normalize_or_zero();
    let na = if n.length_squared() < 1e-12 { [0.0, 0.0, 1.0] } else { n.to_array() };
    for p in [a, b, c] {
        m.positions.push(p);
        m.normals.push(na);
    }
}

/// Push a planar quad `a→b→c→d` as two triangles.
fn push_quad(m: &mut SolidMesh, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
    push_tri(m, a, b, c);
    push_tri(m, a, c, d);
}

/// Push an axis-aligned box spanning `mn..mx` (6 quad faces).
fn push_aabb(m: &mut SolidMesh, mn: Vec3, mx: Vec3) {
    let c = |x: f32, y: f32, z: f32| [x, y, z];
    let (x0, y0, z0, x1, y1, z1) = (mn.x, mn.y, mn.z, mx.x, mx.y, mx.z);
    // bottom / top
    push_quad(m, c(x0, y0, z0), c(x1, y0, z0), c(x1, y1, z0), c(x0, y1, z0));
    push_quad(m, c(x0, y0, z1), c(x0, y1, z1), c(x1, y1, z1), c(x1, y0, z1));
    // front (y0) / back (y1)
    push_quad(m, c(x0, y0, z0), c(x0, y0, z1), c(x1, y0, z1), c(x1, y0, z0));
    push_quad(m, c(x0, y1, z0), c(x1, y1, z0), c(x1, y1, z1), c(x0, y1, z1));
    // left (x0) / right (x1)
    push_quad(m, c(x0, y0, z0), c(x0, y1, z0), c(x0, y1, z1), c(x0, y0, z1));
    push_quad(m, c(x1, y0, z0), c(x1, y0, z1), c(x1, y1, z1), c(x1, y1, z0));
}

/// Push an oriented box: a box of half-extents `half` centred at `center`, rotated by `rot`.
fn push_obb(m: &mut SolidMesh, center: Vec3, half: Vec3, rot: Mat3) {
    let corner = |sx: f32, sy: f32, sz: f32| {
        (center + rot * Vec3::new(sx * half.x, sy * half.y, sz * half.z)).to_array()
    };
    let c000 = corner(-1.0, -1.0, -1.0);
    let c100 = corner(1.0, -1.0, -1.0);
    let c110 = corner(1.0, 1.0, -1.0);
    let c010 = corner(-1.0, 1.0, -1.0);
    let c001 = corner(-1.0, -1.0, 1.0);
    let c101 = corner(1.0, -1.0, 1.0);
    let c111 = corner(1.0, 1.0, 1.0);
    let c011 = corner(-1.0, 1.0, 1.0);
    push_quad(m, c000, c100, c110, c010); // -Z
    push_quad(m, c001, c011, c111, c101); // +Z
    push_quad(m, c000, c001, c101, c100); // -Y
    push_quad(m, c010, c110, c111, c011); // +Y
    push_quad(m, c000, c010, c011, c001); // -X
    push_quad(m, c100, c101, c111, c110); // +X
}

/// Segments around a rail/baluster cross-section — smooth enough to read as round, cheap enough to
/// stamp hundreds of.
const ROD_SEG: usize = 10;

/// A stable circular cross-section basis (two perpendicular unit vectors) for an axis `t`. Uses Z as
/// the reference up unless the axis is near-vertical (then X), so consecutive rings along a gently
/// curving path stay consistently oriented (no twist → no gaps when swept).
fn ring_basis(t: Vec3) -> (Vec3, Vec3) {
    let up = if t.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let nx = up.cross(t).normalize_or_zero();
    let ny = t.cross(nx).normalize_or_zero();
    (nx, ny)
}

/// Push a round rod (a cylinder of radius `r`) between two points `a` and `b`, with fan end caps —
/// used for handrail members (rails, balusters, newel posts) at any angle. Degenerate (near-zero
/// length) rods are skipped.
fn push_rod(m: &mut SolidMesh, a: Vec3, b: Vec3, r: f32) {
    let d = b - a;
    let len = d.length();
    if len < 1e-5 {
        return;
    }
    let (nx, ny) = ring_basis(d / len);
    let ring = |c: Vec3| -> Vec<Vec3> {
        (0..ROD_SEG)
            .map(|i| {
                let ang = std::f32::consts::TAU * i as f32 / ROD_SEG as f32;
                c + (nx * ang.cos() + ny * ang.sin()) * r
            })
            .collect()
    };
    let (ra, rb) = (ring(a), ring(b));
    for i in 0..ROD_SEG {
        let j = (i + 1) % ROD_SEG;
        push_quad(m, ra[i].to_array(), ra[j].to_array(), rb[j].to_array(), rb[i].to_array());
        push_tri(m, a.to_array(), ra[j].to_array(), ra[i].to_array()); // start cap
        push_tri(m, b.to_array(), rb[i].to_array(), rb[j].to_array()); // end cap
    }
}

/// Sweep a round cross-section (radius `r`) along a polyline `pts` as ONE continuous tube — a ring
/// per node (perpendicular to that node's tangent) joined by quads, so joints never gap. Fan caps
/// at both ends. Used for the spiral's single-piece helical handrail.
fn push_polyline_tube(m: &mut SolidMesh, pts: &[Vec3], r: f32) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let rings: Vec<Vec<Vec3>> = (0..n)
        .map(|i| {
            let t = if i == 0 {
                pts[1] - pts[0]
            } else if i == n - 1 {
                pts[n - 1] - pts[n - 2]
            } else {
                pts[i + 1] - pts[i - 1]
            };
            let (nx, ny) = ring_basis(t.normalize_or_zero());
            (0..ROD_SEG)
                .map(|k| {
                    let ang = std::f32::consts::TAU * k as f32 / ROD_SEG as f32;
                    pts[i] + (nx * ang.cos() + ny * ang.sin()) * r
                })
                .collect()
        })
        .collect();
    for i in 0..n - 1 {
        for k in 0..ROD_SEG {
            let j = (k + 1) % ROD_SEG;
            push_quad(m, rings[i][k].to_array(), rings[i][j].to_array(), rings[i + 1][j].to_array(), rings[i + 1][k].to_array());
        }
    }
    for k in 0..ROD_SEG {
        let j = (k + 1) % ROD_SEG;
        push_tri(m, pts[0].to_array(), rings[0][j].to_array(), rings[0][k].to_array());
        push_tri(m, pts[n - 1].to_array(), rings[n - 1][k].to_array(), rings[n - 1][j].to_array());
    }
}

/// Push a vertical cylinder (axis along +Z) centred at `(cx, cy)`, radius `r`, from `z0` to `z1`,
/// approximated by `seg` side facets with fan-triangulated end caps. Used for the spiral's post.
fn push_cylinder(m: &mut SolidMesh, cx: f32, cy: f32, r: f32, z0: f32, z1: f32, seg: usize) {
    let seg = seg.max(3);
    let ring: Vec<[f32; 2]> = (0..seg)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / seg as f32;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect();
    let ctr_lo = [cx, cy, z0];
    let ctr_hi = [cx, cy, z1];
    for i in 0..seg {
        let j = (i + 1) % seg;
        let (p0, p1) = (ring[i], ring[j]);
        let lo0 = [p0[0], p0[1], z0];
        let lo1 = [p1[0], p1[1], z0];
        let hi0 = [p0[0], p0[1], z1];
        let hi1 = [p1[0], p1[1], z1];
        push_quad(m, lo0, lo1, hi1, hi0); // side
        push_tri(m, ctr_lo, lo1, lo0); // bottom cap
        push_tri(m, ctr_hi, hi0, hi1); // top cap
    }
}

/// Push a prism: a closed convex polygon `profile` (points in the Y-Z plane) extruded along X from
/// `x_lo` to `x_hi`. Two fan-triangulated caps plus one quad per profile edge. Used for the
/// triangular stringers and the ramp deck.
fn push_prism(m: &mut SolidMesh, profile: &[[f32; 2]], x_lo: f32, x_hi: f32) {
    let n = profile.len();
    if n < 3 { return; }
    let at = |x: f32, q: [f32; 2]| [x, q[0], q[1]];
    // Caps (fan from vertex 0), wound OPPOSITE ways so each faces out of its own end.
    //
    // Winding is irrelevant to this app's shading, which uses |n·l| — but it is not irrelevant to
    // everything. A BSP boolean classifies inside from outside using each polygon's plane, so a cap
    // whose normal points into the solid makes a cut come out inside-out. Two identically-wound
    // caps also leave the prism non-orientable, which is what `meshcut::closure` reports as four
    // unmatched edges on a ramp that looks perfectly closed.
    for k in 1..n - 1 {
        push_tri(m, at(x_lo, profile[0]), at(x_lo, profile[k + 1]), at(x_lo, profile[k]));
        push_tri(m, at(x_hi, profile[0]), at(x_hi, profile[k]), at(x_hi, profile[k + 1]));
    }
    // Side walls.
    for k in 0..n {
        let p0 = profile[k];
        let p1 = profile[(k + 1) % n];
        push_quad(m, at(x_lo, p0), at(x_lo, p1), at(x_hi, p1), at(x_hi, p0));
    }
}

// ─── Measurements ─────────────────────────────────────────────────────────────────────────────

/// Enclosed volume (m³) of a closed triangle mesh via the divergence theorem
/// (`Σ a·(b×c) / 6`). For an assembly of touching/overlapping closed boxes this is positive and
/// approximates the union volume — enough for the "volume > 0" acceptance check.
pub fn mesh_volume(m: &SolidMesh) -> f32 {
    let mut v = 0.0f64;
    for t in m.positions.chunks_exact(3) {
        let a = t[0].map(|x| x as f64);
        let b = t[1].map(|x| x as f64);
        let c = t[2].map(|x| x as f64);
        v += a[0] * (b[1] * c[2] - b[2] * c[1])
            - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
    }
    (v / 6.0).abs() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec §B8 — the default helical ramp reproduces the measured derivations.
    #[test]
    fn helical_ramp_metrics_match_spec() {
        let (m, _w) = plan_helical_ramp(&HelicalRampParams::default()).unwrap();
        assert!((m.deck_width - 3.25).abs() < 1e-4, "deck width {}", m.deck_width);
        assert!((m.radius_ratio - 0.35).abs() < 1e-4, "ratio {}", m.radius_ratio);
        assert!((m.sweep_deg - 720.0).abs() < 1e-3, "sweep {}", m.sweep_deg);
        assert!((m.pitch_m_per_deg - 0.0097222).abs() < 1e-6, "pitch {}", m.pitch_m_per_deg);
        assert!((m.rise_per_turn - 3.5).abs() < 1e-4, "rise/turn {}", m.rise_per_turn);
        assert!((m.path_length - 42.4115).abs() < 1e-2, "walked {}", m.path_length);
        assert!((m.slope_mean - 0.16505).abs() < 1e-4, "slope mean {}", m.slope_mean);
        assert!((m.slope_inner - 0.31831).abs() < 1e-4, "slope inner {}", m.slope_inner);
        assert!((m.headroom - 3.20).abs() < 1e-4, "headroom {}", m.headroom);
        assert!((m.top_of_deck - 7.30).abs() < 1e-4, "top {}", m.top_of_deck);
        assert!((m.turns_for_1_12 - 3.96).abs() < 0.02, "turns for 1:12 {}", m.turns_for_1_12);
    }

    /// Spec §B8 parametric row — `top_of_deck == slab + ramp_height` for several very different
    /// ramps, and reversing `direction` gives identical derived numbers (a mirror image).
    #[test]
    fn helical_ramp_is_parametric_and_mirror_symmetric() {
        for (h, ri, ro, turns) in [(7.0, 1.75, 5.0, 2.0), (4.2, 3.0, 6.0, 3.0), (2.8, 1.2, 3.0, 1.0)] {
            let p = HelicalRampParams { ramp_height: h, r_inner: ri, r_outer: ro, turns, ..Default::default() };
            let (m, _) = plan_helical_ramp(&p).unwrap();
            assert!((m.top_of_deck - (p.slab_thickness + h)).abs() < 1e-5);
            let (mr, _) = plan_helical_ramp(&HelicalRampParams { direction: -1.0, ..p }).unwrap();
            assert!((mr.slope_mean - m.slope_mean).abs() < 1e-6, "mirror keeps the slope");
            assert!((mr.top_of_deck - m.top_of_deck).abs() < 1e-6, "mirror keeps the top");
        }
    }

    /// The build yields a solid mesh (positive enclosed volume from the deck alone), reaches the top
    /// of the walking surface, and tags many pieces (deck + rails + posts + end rails).
    #[test]
    fn helical_ramp_builds_a_valid_solid() {
        let p = HelicalRampParams { segments_per_turn: 48, ..Default::default() };
        let m = build_helical_ramp(&p).unwrap();
        assert!(m.tri_count() > 0);
        assert_eq!(m.face_ids.len(), m.tri_count(), "every triangle tagged");
        let pieces = m.face_ids.iter().copied().max().unwrap() + 1;
        assert!(pieces > 50, "deck + 8 rails + many posts + end rails, got {pieces}");
        let (mn, mx) = m.bounds().unwrap();
        assert!((mx[2] - (p.slab_thickness + p.ramp_height + p.rail_height)).abs() < 0.1, "reaches the top rail");
        assert!(mn[2] >= -1e-3, "the soffit rests on z = 0");
        // Spans the full outer diameter.
        assert!(mx[0] - mn[0] > 2.0 * p.r_outer - 0.5, "spans the outer diameter");
    }

    #[test]
    fn helical_ramp_rejects_bad_inputs() {
        assert!(plan_helical_ramp(&HelicalRampParams { r_outer: 1.0, r_inner: 2.0, ..Default::default() }).is_err());
        assert!(plan_helical_ramp(&HelicalRampParams { turns: 0.0, ..Default::default() }).is_err());
        assert!(plan_helical_ramp(&HelicalRampParams { slab_thickness: 0.0, ..Default::default() }).is_err());
        assert!(plan_helical_ramp(&HelicalRampParams { rail_height: 0.1, rail_lowest: 0.2, ..Default::default() }).is_err());
    }

    #[test]
    fn straight_stairs_step_count_and_riser() {
        let p = StairParams { total_height: 3.0, desired_riser_height: 0.18, ..Default::default() };
        let plan = plan_stairs(&p).unwrap();
        // ceil(3.0 / 0.18) = ceil(16.67) = 17
        assert_eq!(plan.num_steps, 17);
        assert!((plan.riser_height - 3.0 / 17.0).abs() < 1e-6);
        let m = build_stairs(&p).unwrap();
        assert!(m.tri_count() > 0);
        assert!(mesh_volume(&m) > 0.0);
    }

    #[test]
    fn ushape_splits_and_landing() {
        let p = StairParams {
            layout: StairLayout::UShape,
            total_height: 3.0,
            desired_riser_height: 0.18,
            step_width: 1.0,
            landing_depth: 1.2,
            split_ratio: 0.5,
            has_handrails: false, // pure tread geometry so the exact-height check is unambiguous
            ..Default::default()
        };
        let plan = plan_stairs(&p).unwrap();
        assert_eq!(plan.num_steps, 17);
        // even split: ceil(17/2) = 9 first flight, 8 second.
        assert_eq!(plan.flight1_steps, 9);
        assert_eq!(plan.flight2_steps, 8);
        assert_eq!(plan.flight1_steps + plan.flight2_steps, plan.num_steps);
        assert!((plan.landing_height - 9.0 * plan.riser_height).abs() < 1e-5);
        let m = build_stairs(&p).unwrap();
        let (mn, mx) = m.bounds().unwrap();
        // Two width lanes wide, rises to the full height.
        assert!(mx[0] - mn[0] > 1.9 && mx[0] - mn[0] < 2.1, "spans two widths");
        assert!((mx[2] - 3.0).abs() < 1e-3, "reaches floor-to-floor height");
        assert!(mesh_volume(&m) > 0.0);
    }

    #[test]
    fn ushape_landing_too_short_is_rejected() {
        let p = StairParams {
            layout: StairLayout::UShape,
            step_width: 1.0,
            landing_depth: 0.5, // < step_width
            ..Default::default()
        };
        assert!(matches!(plan_stairs(&p), Err(ArchError::LandingTooShort { .. })));
    }

    #[test]
    fn tiny_riser_is_capped_not_hung() {
        let p = StairParams { total_height: 3.0, desired_riser_height: 0.0001, ..Default::default() };
        assert!(matches!(plan_stairs(&p), Err(ArchError::TooManySteps(_))));
    }

    #[test]
    fn spiral_steps_rotation_and_solid() {
        // has_handrail off so the top of the mesh is the post, not a rail 0.9 m higher.
        let p = SpiralParams { steps_per_turn: 12, total_turns: 1.5, has_handrail: false, ..Default::default() };
        let plan = plan_spiral(&p).unwrap();
        assert_eq!(plan.num_steps, 18); // 12 * 1.5
        assert!((plan.total_rotation_deg - 540.0).abs() < 1e-3);
        let m = build_spiral(&p).unwrap();
        assert!(m.tri_count() > 0 && mesh_volume(&m) > 0.0);
        let (mn, mx) = m.bounds().unwrap();
        assert!((mx[2] - mn[2] - p.total_height).abs() < 1e-3, "post spans the height");
    }

    #[test]
    fn stairs_tag_each_primitive_as_its_own_part() {
        // A straight flight tags every tread/riser/baluster/rail with a distinct face_id, so
        // "select a piece" downstream picks ONE part, not the whole welded run.
        let p = StairParams { total_height: 2.0, desired_riser_height: 0.2, ..Default::default() };
        let m = build_stairs(&p).unwrap();
        assert_eq!(m.face_ids.len(), m.positions.len() / 3, "one part id per triangle");
        let parts = m.face_ids.iter().copied().max().unwrap() + 1;
        assert!(parts > 10, "many distinct pieces (treads+risers+rails+balusters), got {parts}");
        // No single part covers the whole mesh.
        let ntri = m.positions.len() / 3;
        for pid in 0..parts {
            let cnt = m.face_ids.iter().filter(|&&f| f == pid).count();
            assert!(cnt < ntri, "part {pid} is not the whole object");
        }
    }

    #[test]
    fn straight_handrail_adds_members_above_the_treads() {
        // Same stair with and without the balustrade: rails must add triangles and raise the top.
        let bare = StairParams { has_handrails: false, ..Default::default() };
        let railed = StairParams { has_handrails: true, ..Default::default() };
        let mb = build_stairs(&bare).unwrap();
        let mr = build_stairs(&railed).unwrap();
        assert!(mr.tri_count() > mb.tri_count(), "handrail adds geometry");
        let (_, mxb) = mb.bounds().unwrap();
        let (_, mxr) = mr.bounds().unwrap();
        // The rail sits ~handrail_height above the top tread, so the railed mesh is taller.
        assert!(mxr[2] > mxb[2] + 0.5, "rail rises above the treads ({} vs {})", mxr[2], mxb[2]);
        assert!(mesh_volume(&mr) > 0.0);
    }

    #[test]
    fn spiral_post_is_round_and_handrail_present() {
        let bare = SpiralParams { has_handrail: false, ..Default::default() };
        let railed = SpiralParams { has_handrail: true, ..Default::default() };
        let mb = build_spiral(&bare).unwrap();
        let mr = build_spiral(&railed).unwrap();
        assert!(mr.tri_count() > mb.tri_count(), "handrail adds geometry");
        // The round post: at z just above the floor its cross-section is a disc, so the X-extent of
        // vertices near the axis is ~2·post_r and roughly equal to the Y-extent (not a square only
        // touching on the diagonal). Sample the post ring by taking vertices with |z small|.
        let mut rx = 0.0_f32;
        let mut ry = 0.0_f32;
        for v in bare_post_ring(&mb) {
            rx = rx.max(v[0].abs());
            ry = ry.max(v[1].abs());
        }
        assert!(rx > 1e-3 && (rx - ry).abs() < 0.05 * rx.max(ry), "post is round-ish: rx={rx} ry={ry}");
    }

    /// Vertices of the spiral post's bottom cap (z ≈ 0) — used to check the post is round.
    fn bare_post_ring(m: &SolidMesh) -> Vec<[f32; 3]> {
        m.positions.iter().cloned().filter(|v| v[2].abs() < 1e-3 && (v[0] * v[0] + v[1] * v[1]).sqrt() < 0.4).collect()
    }

    #[test]
    fn ramp_slope_and_solid() {
        let p = RampParams { vertical_height: 1.0, horizontal_length: 4.0, width: 1.5, thickness: 0.15 };
        assert!((ramp_slope_deg(&p) - 14.036).abs() < 0.01);
        let m = build_ramp(&p).unwrap();
        assert!(m.tri_count() > 0 && mesh_volume(&m) > 0.0);
        let (mn, mx) = m.bounds().unwrap();
        assert!(mn[2] >= -1e-4, "rests on z=0");
        assert!(mx[1] - mn[1] >= 4.0 - 1e-3, "spans the horizontal length");
        assert!(mx[0] - mn[0] >= 1.5 - 1e-3, "spans the width");
    }
}
