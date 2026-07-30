//! Parametric HALF-TURN (dog-leg) staircase — two parallel flights of unequal length separated by
//! a well, joined by a landing. Ported from `PORTABLE_STAIRCASE_SPEC.md`: five inputs, everything
//! else derived. This module is PURE geometry + derivation (no CSG, no app types) so it can be unit
//! tested against the spec's regression values; the app turns [`DoglegPart`]s into editable CSG
//! `Box` / `Extrusion` features.
//!
//! Reference frame (spec §2.1): **X along the flights, Y across, Z up**, metres, origin at the
//! bottom front-left of step 1 at lower-floor level. The app is already Z-up metres → identity map.

use crate::architecture::ArchError;

/// The five client inputs (metres). Everything else derives from these.
#[derive(Clone, Copy, Debug)]
pub struct DoglegInput {
    /// Steps in the long (lower) flight.
    pub n_lower: usize,
    /// Steps in the short (upper) flight.
    pub n_upper: usize,
    /// Going — horizontal depth of one tread (spec's "width" of a tread).
    pub going: f32,
    /// Stair width, side to side (spec's "length").
    pub width: f32,
    /// Gap between the two flights (the well).
    pub well: f32,
    /// Floor-to-floor height.
    pub total_rise: f32,
}

impl Default for DoglegInput {
    /// The spec's reference staircase (24 steps, 18 + 6, 2.89 m).
    fn default() -> Self {
        Self { n_lower: 18, n_upper: 6, going: 0.30, width: 1.23, well: 0.18, total_rise: 2.89 }
    }
}

// ─── Construction detail (spec §1.3 preset — design choices, not client spec) ───────────────────
/// Stone tread slab thickness.
pub const TREAD_THICK: f32 = 0.04;
/// Tread overhang past the finished riser face.
pub const NOSING: f32 = 0.03;
/// Riser facing-panel thickness (structure is offset back by this).
pub const RISER_FACE_T: f32 = 0.015;
/// Vertical drop from the pitch line to the soffit (must exceed the riser).
pub const WAIST_V: f32 = 0.30;
/// Skirting-board thickness (eats into the clear walking width, spec §1.4.2).
pub const SKIRT_T: f32 = 0.025;
/// Newel-post width (the well-side line eats into the clear width).
pub const POST_W: f32 = 0.06;

/// Every derived quantity (spec §1.2), surfaced as the live UI readout + verification table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DoglegMetrics {
    pub n_steps: usize,
    pub riser: f32,
    pub slope: f32,
    pub pitch_deg: f32,
    pub run_lower: f32,
    pub run_upper: f32,
    pub landing_z: f32,
    pub shaft_len: f32,
    pub shaft_wid: f32,
    pub arrival_x: f32,
    /// Top tread top — MUST equal `total_rise` exactly.
    pub top_tread_z: f32,
    /// Clear walking width = width − skirt − newel line (spec §1.4.2); here width − RISER_FACE_T.
    pub clear_width: f32,
    /// Comfort rule 2·riser + going.
    pub comfort_2r_t: f32,
}

/// Which structural part a [`DoglegPart`] is — drives its default colour + a readable feature name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    FlightBody,
    Tread,
    Landing,
}

/// One editable solid of the staircase, in WORLD coords. The app maps each to a CSG feature.
#[derive(Clone, Debug)]
pub enum DoglegPart {
    /// Axis-aligned box `min..max` (a stone tread, the landing slab).
    Box { min: [f32; 3], max: [f32; 3], role: Role },
    /// A flight body: a closed sawtooth profile in the world **X-Z** plane, extruded across
    /// `Y ∈ [y0, y1]` (the stair width). → a CSG `Extrusion`.
    Flight { profile: Vec<[f32; 2]>, y0: f32, y1: f32, role: Role },
}

impl DoglegPart {
    pub fn role(&self) -> Role {
        match self {
            DoglegPart::Box { role, .. } | DoglegPart::Flight { role, .. } => *role,
        }
    }
}

/// Which floor a flight springs from — a floor slab has a solid toe, a landing edge needs full
/// waist depth right at the joint or it hangs concrete in mid-air (spec §5.1).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spring {
    Floor,
    Landing,
}

/// Validate + derive. Returns the metrics and any non-fatal warnings (spec §6 / §1.4). Hard errors
/// become [`ArchError`].
pub fn plan(inp: &DoglegInput) -> Result<(DoglegMetrics, Vec<String>), ArchError> {
    if inp.going <= 0.0 { return Err(ArchError::NonPositive("going")); }
    if inp.width <= 0.0 { return Err(ArchError::NonPositive("width")); }
    if inp.total_rise <= 0.0 { return Err(ArchError::NonPositive("total_rise")); }
    if inp.well < 0.0 { return Err(ArchError::NonPositive("well (must be ≥ 0)")); }
    if inp.n_lower < 1 { return Err(ArchError::NonPositive("lower-flight steps")); }
    if inp.n_upper < 1 { return Err(ArchError::NonPositive("upper-flight steps")); }
    // The upper flight overrunning the entry can't be built as a simple dog-leg (spec §6).
    if inp.n_upper > inp.n_lower {
        return Err(ArchError::NonPositive("upper flight longer than lower (swap the two, or reduce it)"));
    }
    let n_steps = inp.n_lower + inp.n_upper;
    if n_steps > crate::architecture::MAX_STEPS {
        return Err(ArchError::TooManySteps(n_steps));
    }
    let riser = inp.total_rise / n_steps as f32;
    // The soffit sits WAIST_V below the pitch line, the step armpits sit `riser` below it. If
    // WAIST_V ≤ riser the soffit cuts through the steps and the flight polygon self-intersects.
    if WAIST_V <= riser {
        return Err(ArchError::NonPositive("riser too tall for the soffit (waist must exceed the riser)"));
    }
    let slope = riser / inp.going;
    let run_lower = inp.n_lower as f32 * inp.going;
    let run_upper = inp.n_upper as f32 * inp.going;
    let m = DoglegMetrics {
        n_steps,
        riser,
        slope,
        pitch_deg: slope.atan().to_degrees(),
        run_lower,
        run_upper,
        landing_z: inp.n_lower as f32 * riser,
        shaft_len: run_lower + inp.width, // landing depth = stair width
        shaft_wid: 2.0 * inp.width + inp.well,
        arrival_x: run_lower - run_upper,
        top_tread_z: n_steps as f32 * riser,
        clear_width: inp.width - SKIRT_T - POST_W,
        comfort_2r_t: 2.0 * riser + inp.going,
    };

    let mut warn = Vec::new();
    if !(0.15..=0.20).contains(&riser) {
        warn.push(format!("riser {:.3} m is outside the comfortable 0.15–0.20 m — building the spec exactly", riser));
    }
    if !(0.60..=0.66).contains(&m.comfort_2r_t) {
        warn.push(format!("2·riser + going = {:.3} m is outside the ideal 0.60–0.66 m", m.comfort_2r_t));
    }
    if inp.going < 0.22 {
        warn.push(format!("going {:.3} m is shallow (< 0.22 m)", inp.going));
    }
    warn.push(format!("clear walking width ≈ {:.3} m (structural {:.3} m minus the finishes)", m.clear_width, inp.width));
    warn.push(format!("footprint {:.2} × {:.2} m — if it won't fit, change the step split, not the going", m.shaft_len, m.shaft_wid));
    Ok((m, warn))
}

/// The sawtooth flight profile in LOCAL `(x, z)` (spec §5): up each riser, across each tread, down
/// the back face, along the soffit. Then shifted back `RISER_FACE_T` and down `TREAD_THICK` so the
/// concrete structure sits BEHIND its finishes (spec §6 — the single most important line).
fn flight_profile(n: usize, going: f32, riser: f32, slope: f32, spring: Spring) -> Vec<[f32; 2]> {
    let soffit_top = slope * (n as f32 * going) + riser - WAIST_V;
    let mut p: Vec<[f32; 2]> = Vec::new();
    match spring {
        Spring::Landing => {
            p.push([0.0, riser - WAIST_V]); // full waist right at the joint
            p.push([0.0, 0.0]);
        }
        Spring::Floor => p.push([0.0, 0.0]), // solid toe on the floor slab
    }
    for i in 1..=n {
        p.push([(i - 1) as f32 * going, i as f32 * riser]); // up riser i
        p.push([i as f32 * going, i as f32 * riser]); // across tread i
    }
    p.push([n as f32 * going, soffit_top]); // down the back face
    if spring == Spring::Floor {
        p.push([(WAIST_V - riser) / slope, 0.0]); // soffit meets the floor slab
    }
    // Finish offset (spec §6): back by RISER_FACE_T, down by TREAD_THICK.
    p.into_iter().map(|[x, z]| [x + RISER_FACE_T, z - TREAD_THICK]).collect()
}

/// The stone tread slabs of one flight, in LOCAL `(x, z)` extents (spec §7). `x` runs from the
/// nosing lip to the next structural riser; `z` is the finished-tread slab.
fn flight_tread_local(i: usize, going: f32, riser: f32) -> ([f32; 2], [f32; 2]) {
    let x_riser = (i - 1) as f32 * going;
    let x_nose = x_riser - NOSING;
    let x_back = i as f32 * going + RISER_FACE_T;
    let z_top = i as f32 * riser;
    ([x_nose, x_back], [z_top - TREAD_THICK, z_top])
}

/// Build the whole dog-leg as WORLD-space [`DoglegPart`]s: two flight bodies, the landing, and the
/// stone treads. Flight A springs off the floor and ascends +X in Y-band 1; flight B springs off
/// the landing and ascends −X in Y-band 2 (the 180° turn). Treads optional via `with_treads`.
pub fn build(inp: &DoglegInput, with_treads: bool) -> Result<(DoglegMetrics, Vec<DoglegPart>), ArchError> {
    let (m, _warn) = plan(inp)?;
    let (going, riser, width, well) = (inp.going, m.riser, inp.width, inp.well);
    let mut parts = Vec::new();

    // Y-bands (spec §2.1).
    let a_y0 = 0.0;
    let a_y1 = width;
    let b_y0 = width + well;
    let b_y1 = 2.0 * width + well;

    // ── Flight A: local (x, z) == world (x, z); band 1; springs off the floor. ──
    let pa = flight_profile(inp.n_lower, going, riser, m.slope, Spring::Floor);
    parts.push(DoglegPart::Flight { profile: pa, y0: a_y0, y1: a_y1, role: Role::FlightBody });

    // ── Flight B: mirror X about run_lower, lift Z by landing_z; band 2; springs off the landing.
    let pb_local = flight_profile(inp.n_upper, going, riser, m.slope, Spring::Landing);
    // World map (x, z) = (run_lower − lx, landing_z + lz); reverse the point order so the mirrored
    // ring keeps a consistent winding.
    let pb: Vec<[f32; 2]> = pb_local
        .iter()
        .rev()
        .map(|[lx, lz]| [m.run_lower - lx, m.landing_z + lz])
        .collect();
    parts.push(DoglegPart::Flight { profile: pb, y0: b_y0, y1: b_y1, role: Role::FlightBody });

    // ── Landing slab: spans the FULL width, the turning area at the top of flight A. ──
    let land_th = (m.landing_z - riser * 0.0).min(0.16).max(TREAD_THICK); // a sane slab thickness
    let land_th = land_th.min(m.landing_z); // never below the floor
    parts.push(DoglegPart::Box {
        min: [m.run_lower, 0.0, m.landing_z - land_th],
        max: [m.shaft_len, 2.0 * width + well, m.landing_z],
        role: Role::Landing,
    });

    // ── Stone treads. ──
    if with_treads {
        for i in 1..=inp.n_lower {
            let ([x0, x1], [z0, z1]) = flight_tread_local(i, going, riser);
            parts.push(DoglegPart::Box {
                min: [x0, a_y0, z0],
                max: [x1, a_y1, z1],
                role: Role::Tread,
            });
        }
        for i in 1..=inp.n_upper {
            let ([lx0, lx1], [lz0, lz1]) = flight_tread_local(i, going, riser);
            // Mirror X about run_lower, lift Z by landing_z.
            parts.push(DoglegPart::Box {
                min: [m.run_lower - lx1, b_y0, m.landing_z + lz0],
                max: [m.run_lower - lx0, b_y1, m.landing_z + lz1],
                role: Role::Tread,
            });
        }
    }

    Ok((m, parts))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec §7.1 — the reference call must reproduce these exactly.
    #[test]
    fn reference_metrics_match_spec() {
        let (m, _w) = plan(&DoglegInput::default()).unwrap();
        assert_eq!(m.n_steps, 24);
        assert!((m.riser - 0.1204167).abs() < 1e-6, "riser {}", m.riser);
        assert!((m.pitch_deg - 21.8700).abs() < 1e-3, "pitch {}", m.pitch_deg);
        assert!((m.run_lower - 5.40).abs() < 1e-4);
        assert!((m.run_upper - 1.80).abs() < 1e-4);
        assert!((m.landing_z - 2.1675).abs() < 1e-4);
        assert!((m.shaft_len - 6.63).abs() < 1e-4);
        assert!((m.shaft_wid - 2.64).abs() < 1e-4);
        assert!((m.arrival_x - 3.60).abs() < 1e-4);
        assert!((m.top_tread_z - 2.89).abs() < 1e-6, "top tread MUST be exact: {}", m.top_tread_z);
        assert!((m.clear_width - 1.145).abs() < 1e-4);
        assert!((m.comfort_2r_t - 0.5408).abs() < 1e-3);
    }

    /// Spec §5.3 — flight B (n = 6, spring = landing) is 15 vertices with these exact values.
    #[test]
    fn flight_b_profile_matches_spec() {
        let (_m, _w) = plan(&DoglegInput::default()).unwrap();
        let riser = 2.89 / 24.0;
        let slope = riser / 0.30;
        let p = flight_profile(6, 0.30, riser, slope, Spring::Landing);
        assert_eq!(p.len(), 15, "15 vertices");
        // A few spec regression points (post finish-offset).
        assert!((p[0][0] - 0.015).abs() < 1e-4 && (p[0][1] - (-0.219583)).abs() < 1e-4, "v0 {:?}", p[0]);
        assert!((p[1][0] - 0.015).abs() < 1e-4 && (p[1][1] - (-0.040)).abs() < 1e-4, "v1 {:?}", p[1]);
        assert!((p[2][0] - 0.015).abs() < 1e-4 && (p[2][1] - 0.080417).abs() < 1e-4, "v2 {:?}", p[2]);
        assert!((p[13][0] - 1.815).abs() < 1e-4 && (p[13][1] - 0.6825).abs() < 1e-4, "v13 {:?}", p[13]);
        assert!((p[14][0] - 1.815).abs() < 1e-4 && (p[14][1] - 0.502917).abs() < 1e-4, "v14 {:?}", p[14]);
    }

    /// Flight A (n = 18, spring = floor) is 39 vertices (spec §5.3).
    #[test]
    fn flight_a_profile_vertex_count() {
        let riser = 2.89 / 24.0;
        let slope = riser / 0.30;
        let p = flight_profile(18, 0.30, riser, slope, Spring::Floor);
        assert_eq!(p.len(), 39);
    }

    /// The build yields both flights, a landing, and 24 treads, all finite, at the right heights.
    #[test]
    fn build_yields_editable_parts() {
        let (m, parts) = build(&DoglegInput::default(), true).unwrap();
        let flights = parts.iter().filter(|p| p.role() == Role::FlightBody).count();
        let treads = parts.iter().filter(|p| p.role() == Role::Tread).count();
        let landings = parts.iter().filter(|p| p.role() == Role::Landing).count();
        assert_eq!(flights, 2);
        assert_eq!(treads, 24, "18 + 6 stone treads");
        assert_eq!(landings, 1);
        // Every coordinate finite; top tread reaches the floor height.
        let mut top = f32::NEG_INFINITY;
        for p in &parts {
            match p {
                DoglegPart::Box { min, max, .. } => {
                    for k in 0..3 { assert!(min[k].is_finite() && max[k].is_finite() && max[k] >= min[k]); }
                    top = top.max(max[2]);
                }
                DoglegPart::Flight { profile, y0, y1, .. } => {
                    assert!(y1 > y0);
                    for v in profile { assert!(v[0].is_finite() && v[1].is_finite()); }
                }
            }
        }
        assert!((top - m.top_tread_z).abs() < 1e-3, "top tread reaches {}", m.top_tread_z);
    }

    /// End-to-end: the parts convert to CSG `Box`/`Extrusion` features, UNION into a `Model`, and
    /// evaluate to a valid solid at the right height/footprint (catches a bad sawtooth profile).
    #[test]
    fn dogleg_builds_a_valid_csg_solid() {
        use crate::{BoolOp, Model, Placement, Plane, Primitive};
        let (m, parts) = build(&DoglegInput::default(), true).unwrap();
        let mut model = Model::default();
        for part in parts {
            match part {
                DoglegPart::Box { min, max, .. } => {
                    let (w, d, h) = (max[0] - min[0], max[1] - min[1], max[2] - min[2]);
                    let pl = Placement { u: (min[0] + max[0]) * 0.5, v: (min[1] + max[1]) * 0.5, lift: min[2], spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 };
                    model.push(BoolOp::Union, Plane::default(), pl, Primitive::Box { w, d, h });
                }
                DoglegPart::Flight { profile, y0, y1, .. } => {
                    let pts: Vec<glam::Vec2> = profile.iter().map(|p| glam::Vec2::new(p[0], p[1])).collect();
                    let (prof, centre, w, d) = model.add_profile(&pts).expect("flight sawtooth is a valid profile");
                    let plane = Plane::from_basis(glam::Vec3::new(0.0, y1, 0.0), glam::Vec3::X, glam::Vec3::Z);
                    let pl = Placement { u: centre.x, v: centre.y, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 };
                    model.push(BoolOp::Union, plane, pl, Primitive::Extrusion { profile: prof, h: y1 - y0, w, d });
                }
            }
        }
        let mesh = model.eval();
        assert!(mesh.tri_count() > 0, "the union evaluates to geometry");
        let (mn, mx) = mesh.bounds().expect("evaluated stair has bounds");
        assert!((mx[2] - m.top_tread_z).abs() < 0.05, "reaches the floor height: {} vs {}", mx[2], m.top_tread_z);
        assert!(mx[0] - mn[0] > 5.0 && mx[1] - mn[1] > 2.0, "spans the {:.1}×{:.1} footprint", m.shaft_len, m.shaft_wid);
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(plan(&DoglegInput { n_upper: 0, ..Default::default() }).is_err());
        assert!(plan(&DoglegInput { n_lower: 6, n_upper: 18, ..Default::default() }).is_err(), "upper > lower");
        assert!(plan(&DoglegInput { total_rise: 0.0, ..Default::default() }).is_err());
        // A riser taller than the waist self-intersects the soffit → rejected.
        assert!(plan(&DoglegInput { n_lower: 2, n_upper: 1, total_rise: 3.0, ..Default::default() }).is_err());
    }
}
