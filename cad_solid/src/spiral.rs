//! Parametric SPIRAL (helical) staircase — treads winding around a central pole, with a balustrade.
//! Ported from `SPIRAL_STAIRCASE_SPEC.md`. Like [`crate::dogleg`] this module is PURE geometry +
//! derivation (no CSG, no app types) so it can be unit-tested against the spec's regression values;
//! the app turns each [`SpiralPart`] into an editable CSG `Cylinder` / `Extrusion` feature.
//!
//! The one structural fact (spec §5): **treads and every rail are the SAME helix, sampled at
//! different radii and vertical offsets** — one pitch governs the whole assembly, so the rails stay
//! parallel to the tread line. Reference frame: **Z up, metres**, axis on Z, origin at the lower
//! floor. `clockwise` (default) turns the stair clockwise as it climbs, viewed from above.

use crate::architecture::{ArchError, MAX_STEPS};

/// Client inputs. The four the user actually asked to drive — number of turns, overall height,
/// width (outer radius) and step count — plus balustrade options. Everything else derives.
#[derive(Clone, Copy, Debug)]
pub struct SpiralInput {
    /// Number of treads.
    pub n_steps: usize,
    /// Floor-to-floor height (the overall rise).
    pub total_height: f32,
    /// Number of turns; total sweep = `turns × 360°`. Need not be whole (0.75 and 1.25 are common).
    pub turns: f32,
    /// Outer tread radius — how far each tread reaches from the axis (the stair's "width").
    pub radius: f32,
    /// Clockwise as you climb (viewed from above). The spec's reference stair is clockwise.
    pub clockwise: bool,
    /// Build the balustrade (infill rails + handrail + stanchions).
    pub handrail: bool,
    /// Handrail height above the tread nosing (m). Spec's asset is 0.80; 0.90 is code-typical.
    pub handrail_height: f32,
    /// Number of horizontal infill rails between the treads and the handrail.
    pub n_infill: usize,
    /// Build the metal support wedge under each tread.
    pub brackets: bool,
}

impl Default for SpiralInput {
    /// The spec's reference staircase ("Modern Spiral Stairs #04"): 21 steps, 3.15 m, one turn.
    fn default() -> Self {
        Self {
            n_steps: 21,
            total_height: 3.15,
            turns: 1.0,
            radius: 1.2707,
            clockwise: true,
            handrail: true,
            handrail_height: 0.90,
            n_infill: 4,
            brackets: true,
        }
    }
}

// ─── Construction detail (spec §2.3 measured dimensions — design choices, not client spec) ───────
/// Central pole radius.
pub const POLE_R: f32 = 0.0537;
/// Tread-sleeve (collar) outside radius; the stack is one solid (spec §10.2).
pub const SLEEVE_R: f32 = 0.0800;
/// Tread slab thickness.
pub const TREAD_THICK: f32 = 0.0480;
/// The tread's inner edge runs this far INSIDE the sleeve so the two never abut (spec §10.3).
pub const INNER_OVERLAP: f32 = 0.003;
/// Support-bracket radial depth.
pub const BRACKET_DEPTH: f32 = 0.0839;
/// Support-bracket angular span.
pub const BRACKET_SPAN_DEG: f32 = 20.0;
/// The bracket pokes this far UP into the tread rather than touching its underside (spec §10.3).
pub const BRACKET_RISE_INTO_TREAD: f32 = 0.004;
/// Overlap added to |step angle| for the tread blade, so consecutive treads overlap (spec §6.2).
pub const TREAD_OVERLAP_DEG: f32 = 22.5;
/// Infill-rail radius beyond the tread edge (spec: 1.3346 vs R_OUT 1.2707).
pub const RAIL_R_OFF: f32 = 0.064;
/// Handrail radius beyond the tread edge (spec: 1.3488).
pub const HANDRAIL_R_OFF: f32 = 0.078;
/// Infill-rail tube radius (Ø 0.0214 → r ≈ 0.011, rounded up for a sturdier read).
pub const INFILL_TUBE_R: f32 = 0.012;
/// Handrail tube radius (Ø 0.0346 → r ≈ 0.017).
pub const HANDRAIL_TUBE_R: f32 = 0.017;
/// Stanchion tube radius (Ø 0.024).
pub const STANCH_TUBE_R: f32 = 0.012;

/// Every derived quantity (spec §2.2 / §9), for the live UI readout + the verification table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpiralMetrics {
    pub n_steps: usize,
    pub riser: f32,
    /// Signed degrees per step (negative = clockwise).
    pub step_angle_deg: f32,
    /// Metres of rise per degree of sweep — identical for the treads and every rail (spec §5).
    pub pitch_m_per_deg: f32,
    pub total_rise: f32,
    pub total_sweep_deg: f32,
    pub overall_dia: f32,
    /// Top of the handrail above the floor.
    pub handrail_top_z: f32,
    /// The going a climber actually walks (spec §9.2), measured at r ≈ 0.7 · R_OUT.
    pub going_walk: f32,
    /// Clear vertical space under the turn above — only meaningful past one full turn (spec §9.3).
    pub headroom: f32,
}

/// Which structural part a [`SpiralPart`] is — drives its default colour + a readable feature name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Pole,
    Sleeve,
    Tread,
    Bracket,
    Infill,
    Handrail,
    Stanchion,
}

/// One editable solid of the staircase, in WORLD coords. The app maps each to a CSG feature.
#[derive(Clone, Debug)]
pub enum SpiralPart {
    /// A vertical cylinder on the Z axis at `(cx, cy)`, from `z0` to `z1` (pole, sleeve, stanchion).
    Cyl { cx: f32, cy: f32, r: f32, z0: f32, z1: f32, sides: u32, role: Role },
    /// A closed world-XY polygon extruded up `z0..z1` (a tread blade, a bracket wedge). → `Extrusion`.
    Prism { poly: Vec<[f32; 2]>, z0: f32, z1: f32, role: Role },
    /// A straight rod from `a` to `b` (one chord of a helical rail). → an oriented `Cylinder`.
    Seg { a: [f32; 3], b: [f32; 3], r: f32, sides: u32, role: Role },
}

impl SpiralPart {
    pub fn role(&self) -> Role {
        match self {
            SpiralPart::Cyl { role, .. }
            | SpiralPart::Prism { role, .. }
            | SpiralPart::Seg { role, .. } => *role,
        }
    }
}

/// Validate + derive. Returns the metrics and any non-fatal warnings (spec §7.1 / §9). Hard errors
/// become [`ArchError`].
pub fn plan(inp: &SpiralInput) -> Result<(SpiralMetrics, Vec<String>), ArchError> {
    if inp.total_height <= 0.0 {
        return Err(ArchError::NonPositive("total_height"));
    }
    if inp.turns <= 0.0 {
        return Err(ArchError::NonPositive("turns"));
    }
    if inp.radius <= SLEEVE_R {
        return Err(ArchError::NonPositive("radius (must clear the central pole)"));
    }
    if inp.n_steps < 1 {
        return Err(ArchError::NonPositive("steps"));
    }
    if inp.n_steps > MAX_STEPS {
        return Err(ArchError::TooManySteps(inp.n_steps));
    }

    let riser = inp.total_height / inp.n_steps as f32;
    let sweep = inp.turns * 360.0 * if inp.clockwise { -1.0 } else { 1.0 };
    let step_angle = sweep / inp.n_steps as f32;
    let pitch = riser / step_angle.abs();
    let r_walk = 0.7 * inp.radius;
    let m = SpiralMetrics {
        n_steps: inp.n_steps,
        riser,
        step_angle_deg: step_angle,
        pitch_m_per_deg: pitch,
        total_rise: inp.n_steps as f32 * riser,
        total_sweep_deg: sweep,
        overall_dia: 2.0 * (inp.radius + HANDRAIL_R_OFF),
        handrail_top_z: inp.total_height + inp.handrail_height,
        going_walk: r_walk * step_angle.abs().to_radians(),
        // Rise gained over one full turn, less the tread+bracket thickness that hangs below it.
        headroom: 360.0 * pitch - (TREAD_THICK + BRACKET_DEPTH),
    };

    let mut warn = Vec::new();
    if !(0.15..=0.20).contains(&riser) {
        warn.push(format!("riser {:.3} m is outside the comfortable 0.15–0.20 m — building it as asked", riser));
    }
    if inp.handrail && inp.handrail_height < 0.90 {
        warn.push(format!("handrail {:.2} m above the nosing is below the 0.90–1.00 m code guideline — a visual asset, not a compliant guard", inp.handrail_height));
    }
    if m.going_walk < 0.22 {
        warn.push(format!("going at the walk line ≈ {:.3} m is steep (< 0.22 m) — normal for a tight spiral", m.going_walk));
    }
    if inp.turns > 1.0 && m.headroom < 2.0 {
        warn.push(format!("headroom under the turn above ≈ {:.2} m — a climber's head meets the underside past one turn", m.headroom));
    }
    warn.push(format!(
        "{} steps · {:.2} turns · riser {:.3} m · Ø {:.2} m · handrail top {:.2} m",
        inp.n_steps, inp.turns, riser, m.overall_dia, m.handrail_top_z,
    ));
    Ok((m, warn))
}

/// A closed world-XY sector polygon between radii `[r_in, r_out]` and angles `[a0, a1]` (radians),
/// `seg` chords per arc. Winding is left to [`crate::Model::add_profile`], which normalises it.
fn sector(r_in: f32, r_out: f32, a0: f32, a1: f32, seg: usize) -> Vec<[f32; 2]> {
    let mut p = Vec::with_capacity(2 * (seg + 1));
    for k in 0..=seg {
        let t = a0 + (a1 - a0) * k as f32 / seg as f32;
        p.push([r_in * t.cos(), r_in * t.sin()]);
    }
    for k in 0..=seg {
        let t = a1 + (a0 - a1) * k as f32 / seg as f32;
        p.push([r_out * t.cos(), r_out * t.sin()]);
    }
    p
}

/// Build the whole spiral as WORLD-space [`SpiralPart`]s: pole + sleeve stack, one tread (and
/// optional bracket) per step around the helix, and — if `inp.handrail` — a set of infill rails, a
/// handrail and stanchions, every one following the SAME nosing helix (spec §5).
pub fn build(inp: &SpiralInput) -> Result<(SpiralMetrics, Vec<SpiralPart>), ArchError> {
    let (m, _warn) = plan(inp)?;
    let (riser, step) = (m.riser, m.step_angle_deg.to_radians());
    let n = inp.n_steps;
    let mut parts = Vec::new();

    // ── Central pole + one-piece sleeve stack (spec §6.1, §10.2). ──
    parts.push(SpiralPart::Cyl { cx: 0.0, cy: 0.0, r: POLE_R, z0: 0.0, z1: inp.total_height, sides: 28, role: Role::Pole });
    parts.push(SpiralPart::Cyl { cx: 0.0, cy: 0.0, r: SLEEVE_R, z0: 0.0, z1: inp.total_height, sides: 24, role: Role::Sleeve });

    // ── Treads (and brackets). Each tread's leading edge is at `step·i`; the blade spans back by
    //    |step| + overlap so consecutive treads overlap (spec §6.2). ──
    let r_in = SLEEVE_R - INNER_OVERLAP;
    let span = (step.abs() + TREAD_OVERLAP_DEG.to_radians()).min(170.0_f32.to_radians());
    let bracket_span = BRACKET_SPAN_DEG.to_radians();
    for i in 1..=n {
        let a_lead = step * i as f32; // leading edge of tread i
        let a0 = a_lead - step.signum() * span; // trailing (covered) edge
        let (lo, hi) = if a0 < a_lead { (a0, a_lead) } else { (a_lead, a0) };
        let z_top = i as f32 * riser;
        parts.push(SpiralPart::Prism {
            poly: sector(r_in, inp.radius, lo, hi, 6),
            z0: z_top - TREAD_THICK,
            z1: z_top,
            role: Role::Tread,
        });
        if inp.brackets {
            let a_mid = a_lead - step.signum() * span * 0.5;
            let (blo, bhi) = (a_mid - bracket_span * 0.5, a_mid + bracket_span * 0.5);
            let btop = z_top - TREAD_THICK + BRACKET_RISE_INTO_TREAD; // pokes up into the tread
            parts.push(SpiralPart::Prism {
                poly: sector(0.05, inp.radius * 0.958, blo, bhi, 3),
                z0: btop - BRACKET_DEPTH,
                z1: btop,
                role: Role::Bracket,
            });
        }
    }

    // ── Balustrade — infill rails, handrail, stanchions — all on the nosing helix (spec §5, §7). ──
    if inp.handrail {
        let rail_r = inp.radius + RAIL_R_OFF;
        let hand_r = inp.radius + HANDRAIL_R_OFF;
        // Vertical offset of each element above the tread top at the same angle.
        let hand_off = inp.handrail_height;
        let base_off = 0.12_f32.min(hand_off * 0.4); // lowest infill above the tread
        let top_off = (hand_off - 0.10).max(base_off); // highest infill, just under the handrail
        let infill_off = |j: usize| -> f32 {
            if inp.n_infill <= 1 {
                base_off
            } else {
                base_off + (top_off - base_off) * j as f32 / (inp.n_infill - 1) as f32
            }
        };
        // Sample the helix at every tread centre; a rail is the chord run through those points.
        let point = |i: usize, r: f32, off: f32| -> [f32; 3] {
            let a = step * (i as f32 - 0.5);
            [r * a.cos(), r * a.sin(), i as f32 * riser + off]
        };
        let mut rail = |r: f32, off: f32, tube: f32, role: Role, out: &mut Vec<SpiralPart>| {
            for i in 1..n {
                out.push(SpiralPart::Seg { a: point(i, r, off), b: point(i + 1, r, off), r: tube, sides: 8, role });
            }
        };
        for j in 0..inp.n_infill {
            rail(rail_r, infill_off(j), INFILL_TUBE_R, Role::Infill, &mut parts);
        }
        rail(hand_r, hand_off, HANDRAIL_TUBE_R, Role::Handrail, &mut parts);

        // Stanchions tie the rails together at ~90° intervals (spec §7): pick up to 5 tread
        // positions spread along the run, each a vertical post from the lowest infill to the handrail.
        let n_stanch = 5.min(n);
        for s in 0..n_stanch {
            let i = 1 + (s * (n - 1)) / n_stanch.max(1);
            let a = step * (i as f32 - 0.5);
            let (cx, cy) = (rail_r * a.cos(), rail_r * a.sin());
            let z_base = i as f32 * riser + infill_off(0);
            let z_top = i as f32 * riser + hand_off;
            if z_top > z_base {
                parts.push(SpiralPart::Cyl { cx, cy, r: STANCH_TUBE_R, z0: z_base, z1: z_top, sides: 8, role: Role::Stanchion });
            }
        }
    }

    Ok((m, parts))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec §2.2 / §11 — the reference call reproduces the measured regression values.
    #[test]
    fn reference_metrics_match_spec() {
        let (m, _w) = plan(&SpiralInput::default()).unwrap();
        assert_eq!(m.n_steps, 21);
        assert!((m.riser - 0.150).abs() < 1e-6, "riser {}", m.riser);
        assert!((m.step_angle_deg - (-17.142857)).abs() < 1e-4, "step angle {}", m.step_angle_deg);
        assert!((m.pitch_m_per_deg - 0.00875).abs() < 1e-6, "pitch {}", m.pitch_m_per_deg);
        assert!((m.total_rise - 3.150).abs() < 1e-4, "total rise {}", m.total_rise);
        assert!((m.total_sweep_deg + 360.0).abs() < 1e-4, "sweep {}", m.total_sweep_deg);
    }

    /// Spec §5 / §11 — the single most valuable check: the tread pitch and every rail pitch are the
    /// same helix. We assert the rails climb at exactly `pitch × |step|` per step, i.e. one riser.
    #[test]
    fn every_rail_shares_the_tread_pitch() {
        let (m, parts) = build(&SpiralInput::default()).unwrap();
        let step_dz = m.pitch_m_per_deg * m.step_angle_deg.abs(); // rise per step along any helix
        assert!((step_dz - m.riser).abs() < 1e-5, "one pitch: {} vs {}", step_dz, m.riser);
        // Consecutive handrail chords rise by exactly one riser.
        let hand: Vec<_> = parts.iter().filter(|p| p.role() == Role::Handrail).collect();
        assert!(hand.len() >= 2, "handrail is segmented");
        for p in &hand {
            if let SpiralPart::Seg { a, b, .. } = p {
                assert!((b[2] - a[2] - m.riser).abs() < 1e-4, "each handrail chord climbs one riser");
            }
        }
    }

    /// The build yields a pole, a sleeve, 21 treads + 21 brackets, and a balustrade — all finite,
    /// the top tread reaching the floor height.
    #[test]
    fn build_yields_editable_parts() {
        let inp = SpiralInput::default();
        let (m, parts) = build(&inp).unwrap();
        let count = |r: Role| parts.iter().filter(|p| p.role() == r).count();
        assert_eq!(count(Role::Pole), 1);
        assert_eq!(count(Role::Sleeve), 1);
        assert_eq!(count(Role::Tread), 21);
        assert_eq!(count(Role::Bracket), 21);
        assert_eq!(count(Role::Handrail), 20, "one chord between each pair of treads");
        assert_eq!(count(Role::Infill), 20 * inp.n_infill);
        let mut tread_top = f32::NEG_INFINITY; // treads/pole reach the floor height
        let mut overall_top = f32::NEG_INFINITY; // the handrail rises above it
        for p in &parts {
            match p {
                SpiralPart::Cyl { cx, cy, r, z0, z1, role, .. } => {
                    for v in [cx, cy, r, z0, z1] { assert!(v.is_finite()); }
                    assert!(z1 >= z0);
                    overall_top = overall_top.max(*z1);
                    if *role == Role::Pole { tread_top = tread_top.max(*z1); }
                }
                SpiralPart::Prism { poly, z0, z1, role, .. } => {
                    assert!(z1 > z0 && poly.len() >= 3);
                    for v in poly { assert!(v[0].is_finite() && v[1].is_finite()); }
                    overall_top = overall_top.max(*z1);
                    if *role == Role::Tread { tread_top = tread_top.max(*z1); }
                }
                SpiralPart::Seg { a, b, r, .. } => {
                    for v in a.iter().chain(b).chain([r]) { assert!(v.is_finite()); }
                    overall_top = overall_top.max(a[2].max(b[2]));
                }
            }
        }
        assert!((tread_top - inp.total_height).abs() < 0.05, "top tread/pole reaches {} m", inp.total_height);
        assert!((overall_top - m.handrail_top_z).abs() < 0.05, "handrail top at {} m", m.handrail_top_z);
    }

    /// End-to-end: the parts convert to CSG `Cylinder`/`Extrusion` features, UNION into a `Model`,
    /// and evaluate to a valid solid at the right height and diameter.
    #[test]
    fn spiral_builds_a_valid_csg_solid() {
        use crate::{BoolOp, Model, Placement, Plane, Primitive};
        use glam::{Vec2, Vec3};
        // A small stair keeps the boolean union quick; no balustrade so the top is the tread run.
        let inp = SpiralInput { n_steps: 8, total_height: 1.2, turns: 0.75, radius: 0.9, handrail: false, brackets: false, ..Default::default() };
        let (_m, parts) = build(&inp).unwrap();
        let mut model = Model::default();
        for part in parts {
            match part {
                SpiralPart::Cyl { cx, cy, r, z0, z1, sides, .. } => {
                    let pl = Placement { u: cx, v: cy, lift: z0, ..Default::default() };
                    model.push(BoolOp::Union, Plane::default(), pl, Primitive::Cylinder { r, h: z1 - z0, sides });
                }
                SpiralPart::Prism { poly, z0, z1, .. } => {
                    let pts: Vec<Vec2> = poly.iter().map(|p| Vec2::new(p[0], p[1])).collect();
                    if let Ok((prof, centre, w, d)) = model.add_profile(&pts) {
                        let pl = Placement { u: centre.x, v: centre.y, lift: z0, ..Default::default() };
                        model.push(BoolOp::Union, Plane::default(), pl, Primitive::Extrusion { profile: prof, h: z1 - z0, w, d });
                    }
                }
                SpiralPart::Seg { a, b, r, sides, .. } => {
                    let (a, b) = (Vec3::from(a), Vec3::from(b));
                    let dir = b - a;
                    let len = dir.length();
                    if len < 1e-4 { continue; }
                    let dir = dir / len;
                    let up = if dir.z.abs() > 0.9 { Vec3::X } else { Vec3::Z };
                    let u = up.cross(dir).normalize();
                    let v = dir.cross(u).normalize();
                    let pl = Placement { u: 0.0, v: 0.0, lift: 0.0, ..Default::default() };
                    model.push(BoolOp::Union, Plane::from_basis(a, u, v), pl, Primitive::Cylinder { r, h: len, sides });
                }
            }
        }
        let mesh = model.eval();
        assert!(mesh.tri_count() > 0, "the union evaluates to geometry");
        let (mn, mx) = mesh.bounds().expect("evaluated spiral has bounds");
        assert!((mx[2] - inp.total_height).abs() < 0.1, "reaches the floor height: {} vs {}", mx[2], inp.total_height);
        assert!(mx[0] - mn[0] > inp.radius, "spans at least the tread radius");
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(plan(&SpiralInput { total_height: 0.0, ..Default::default() }).is_err());
        assert!(plan(&SpiralInput { turns: 0.0, ..Default::default() }).is_err());
        assert!(plan(&SpiralInput { radius: 0.05, ..Default::default() }).is_err(), "radius inside the pole");
        assert!(plan(&SpiralInput { n_steps: 0, ..Default::default() }).is_err());
    }

    /// The oriented-segment basis is right-handed (`u × v = dir`), so rail cylinders don't render
    /// inside-out — the spec §3.3 handedness pitfall, checked at the source.
    #[test]
    fn segment_basis_is_right_handed() {
        use glam::Vec3;
        let dir = Vec3::new(0.3, -0.7, 0.5).normalize();
        let up = if dir.z.abs() > 0.9 { Vec3::X } else { Vec3::Z };
        let u = up.cross(dir).normalize();
        let v = dir.cross(u).normalize();
        assert!((u.cross(v) - dir).length() < 1e-5, "u × v reproduces the segment direction");
    }
}
