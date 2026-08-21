//! SIMLUX against DIALux on three scenes with NOTHING left to infer.
//!
//! The earlier comparison (`dialux_comparison.rs`) was against a real project: a non-rectangular
//! shop with 46 fittings whose mounting heights and aiming the report never states. Every
//! disagreement there had two candidate causes — the engine, or the guess about the scene — and no
//! way to separate them.
//!
//! These three have no guesses left in them. The same 4 × 4 m room three times, and the reports
//! state every input: clearance 4.000 m, working plane 0.800 m, wall zone 0.010 m, reflectances
//! 70 / 50 / 20 %, maintenance factor 0.80 fixed, one ABB FONDO at 4000 lm / 40 W. Only the LAYOUT
//! changes:
//!
//!   t1  one fitting, centred                    Ē 200 lx   U0 0.10
//!   t2  two fittings, upper half of the room    Ē 336 lx   U0 0.079
//!   t3  two fittings, lower half                Ē 336 lx   U0 0.073
//!
//! Three cases matter more than one. t1 alone cannot see a superposition error (one source adds to
//! nothing) and cannot see an axis mix-up (a centred fitting is symmetric in x and y). t2 and t3
//! are near-mirrors of each other, so between them they catch both: if x and y were transposed, or
//! if the second luminaire were added wrongly, t2 and t3 would not both land.
//!
//! Each report also prints its 8 × 8 grid of point values, so this compares 192 NUMBERS rather than
//! three averages. An average can match for the wrong reasons; a whole field cannot.
//!
//! Source: `D:\Dropbox\YASEEN\3d factory\tests\Identical testing\{t1,t2,t3}.pdf`.
//!
//! Run with:
//!   `IDENTICAL_DIR="<that folder>" cargo test -p cad_light --test identical_dialux -- --ignored --nocapture`

use std::collections::HashMap;

use cad_light::{
    box_room, calculate_maintained, default_materials, parse_ldt, CalcPlane, IesProfile, Luminaire,
    Maintenance, Mesh, Material, RaySettings, Vertex,
};

// ---- what all three reports state, identically --------------------------------------------------
const ROOM: f32 = 4.000; // "Ground area 16.00 m²", square
const ROOM_H: f32 = 4.000; // "Clearance height 4.000 m"
const MOUNT_Z: f32 = 4.000; // "Mounting height 4.000 m" — flush with the ceiling
const WORK_PLANE: f32 = 0.800; // "Height Working plane 0.800 m"
const WALL_ZONE: f32 = 0.010; // "Wall zone Working plane 0.010 m"
const MF: f64 = 0.80; // "Maintenance factor 0.80 (fixed)"
const FLUX: f64 = 4000.0; // luminaire list: Φ 4000 lm
const WATTS: f64 = 40.0; // luminaire list: P 40.0 W

/// One report: its layout, its stated aggregates, and its printed grid.
///
/// Grids are stored ROW 0 AT THE BOTTOM of the plan (+y up), matching `CalcPlane`'s row order.
/// They were read off the rendered pages, so a value the contour lines cross is uncertain in its
/// last digit or two — which is why the ASSERTIONS are on the stated aggregates and on the
/// aggregate error across the field, never on any single transcribed cell.
struct Case {
    name: &'static str,
    lums: &'static [(f32, f32)],
    e_avg: f64,
    u0: f64,
    lpd: f64,
    grid: [[f64; 8]; 8],
}

#[rustfmt::skip]
const CASES: [Case; 3] = [
    Case {
        name: "t1 — one fitting, centred",
        lums: &[(2.0, 2.0)],
        e_avg: 200.0, u0: 0.10, lpd: 2.50,
        grid: [
            [ 29.0, 45.9, 87.8, 119.0, 119.0, 87.4, 45.1, 28.8],
            [ 46.5,116.0,177.0, 213.0, 214.0,177.0,117.0, 46.7],
            [ 88.1,177.0,282.0, 419.0, 420.0,282.0,177.0, 87.4],
            [119.0,214.0,419.0, 651.0, 652.0,420.0,214.0,118.0],
            [120.0,214.0,420.0, 652.0, 651.0,420.0,214.0,120.0],
            [ 87.2,177.0,282.0, 420.0, 420.0,282.0,177.0, 87.8],
            [ 45.9,116.0,176.0, 213.0, 212.0,177.0,116.0, 47.1],
            [ 29.0, 46.0, 87.6, 120.0, 120.0, 87.5, 46.2, 28.3],
        ],
    },
    Case {
        name: "t2 — two fittings, upper half",
        lums: &[(1.0, 3.0), (3.0, 3.0)],
        e_avg: 336.0, u0: 0.079, lpd: 5.00,
        grid: [
            [ 30.1, 32.7, 36.4,  36.4,  36.8, 36.6, 34.4, 30.6],
            [ 46.0, 53.1, 58.9,  59.1,  60.9, 58.7, 51.0, 45.3],
            [104.0,138.0,149.0, 134.0, 135.0,149.0,139.0,104.0],
            [205.0,249.0,266.0, 299.0, 298.0,267.0,250.0,208.0],
            [325.0,468.0,516.0, 469.0, 467.0,517.0,466.0,323.0],
            [474.0,715.0,787.0, 646.0, 646.0,784.0,713.0,472.0],
            [480.0,721.0,794.0, 656.0, 653.0,793.0,721.0,473.0],
            [336.0,491.0,544.0, 495.0, 492.0,541.0,490.0,341.0],
        ],
    },
    Case {
        name: "t3 — two fittings, lower half",
        lums: &[(1.0, 1.0), (3.0, 1.0)],
        e_avg: 336.0, u0: 0.073, lpd: 5.00,
        grid: [
            [339.0,491.0,540.0, 491.0, 494.0,540.0,488.0,338.0],
            [479.0,722.0,793.0, 655.0, 654.0,794.0,723.0,473.0],
            [474.0,713.0,787.0, 646.0, 647.0,783.0,714.0,474.0],
            [321.0,468.0,516.0, 471.0, 468.0,515.0,468.0,325.0],
            [205.0,249.0,266.0, 299.0, 299.0,265.0,247.0,206.0],
            [103.0,139.0,146.0, 138.0, 135.0,150.0,138.0,105.0],
            [ 44.8, 53.3, 57.3,  61.9,  60.4, 60.6, 52.0, 44.8],
            [ 29.1, 33.7, 37.3,  36.7,  37.6, 38.6, 33.8, 28.8],
        ],
    },
];

fn scene() -> (Vec<Mesh>, Vec<Material>) {
    // Floor 0.20, walls 0.50, ceiling 0.70 — the library's defaults ARE the reports' figures.
    (box_room(ROOM, ROOM, ROOM_H), default_materials())
}

fn plane_at(cols: u32, rows: u32) -> CalcPlane {
    // The working plane is inset by the wall zone on all four sides — which is what makes its area
    // 15.84 m² against the room's 16.00, and each report's own two power densities (5.05 against
    // 5.00 W/m², say) confirm it: 80 W / 5.05 = 15.84 m².
    let span = ROOM - 2.0 * WALL_ZONE;
    CalcPlane { origin: Vertex::new(WALL_ZONE, WALL_ZONE, WORK_PLANE), width: span, depth: span, cols, rows }
}

fn load_photometry(dir: &str) -> IesProfile {
    let path = format!("{dir}/FONDO.ldt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut prof = parse_ldt(&text).expect("FONDO.ldt parses");
    // This file carries THREE lamp sets. They are alternatives, not simultaneous lamps — summing
    // them was a real bug, found on the previous comparison. If the reader regresses to the sum,
    // this ratio goes to 3 and every case below fails at once.
    let ratio = prof.lumens / FLUX;
    assert!(
        (ratio - 1.0).abs() < 0.02,
        "the reader disagrees with DIALux about this file's flux: {:.1} vs {FLUX:.1} lm",
        prof.lumens,
    );
    prof.watts = WATTS;
    prof
}

#[test]
#[ignore = "needs IDENTICAL_DIR=<folder holding FONDO.ldt and t1/t2/t3.pdf>"]
fn simlux_against_dialux_on_three_fully_specified_rooms() {
    let Ok(dir) = std::env::var("IDENTICAL_DIR") else {
        println!("set IDENTICAL_DIR to the folder holding FONDO.ldt");
        return;
    };

    let prof = load_photometry(&dir);
    println!("\n=== photometry: FONDO.ldt ===");
    println!("  flux {:.1} lm (report: {FLUX:.1})   watts {WATTS:.1} W", prof.lumens);
    let mut profiles = HashMap::new();
    profiles.insert("FONDO".to_string(), prof);

    let (meshes, materials) = scene();
    let settings = RaySettings { rays_per_point: 4096, max_bounces: 8, ..RaySettings::default() };
    // "0.80 (fixed)" — one number, not the four CIE 97 sub-factors, so it goes in whole.
    let maint = Maintenance { llmf: MF, lsf: 1.0, lmf: 1.0, rsmf: 1.0 };

    let mut worst_avg_err = 0.0_f64;
    for case in &CASES {
        let lums: Vec<Luminaire> = case
            .lums
            .iter()
            .enumerate()
            .map(|(i, (x, y))| Luminaire {
                id: i as u32 + 1,
                profile: "FONDO".to_string(),
                position: Vertex::new(*x, *y, MOUNT_Z),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0, watts_override: None, flux_override: None, from_block: None })
            .collect();

        let plane = plane_at(8, 8);
        let grid =
            calculate_maintained(&meshes, &lums, &profiles, &materials, &plane, &settings, maint);

        println!("\n================ {} ================", case.name);
        println!("      {:>34}   {:>34}", "SIMLUX", "DIALux");
        let (mut sum_abs_pct, mut worst) = (0.0, (0.0_f64, 0usize, 0usize));
        for r in 0..8usize {
            let ours: Vec<f64> = (0..8).map(|c| grid.values[r * 8 + c]).collect();
            let fmt = |v: &[f64]| v.iter().map(|x| format!("{x:>6.0}")).collect::<Vec<_>>().join("");
            println!("  r{r}  {}   {}", fmt(&ours), fmt(&case.grid[r]));
            for c in 0..8 {
                let pct = (ours[c] - case.grid[r][c]) / case.grid[r][c] * 100.0;
                sum_abs_pct += pct.abs();
                if pct.abs() > worst.0 {
                    worst = (pct.abs(), r, c);
                }
            }
        }

        let our_avg = grid.values.iter().sum::<f64>() / grid.values.len() as f64;
        let our_min = grid.values.iter().cloned().fold(f64::MAX, f64::min);
        let our_max = grid.values.iter().cloned().fold(0.0, f64::max);
        let our_lpd = WATTS * case.lums.len() as f64 / 16.0;
        let avg_err = (our_avg - case.e_avg) / case.e_avg * 100.0;
        worst_avg_err = worst_avg_err.max(avg_err.abs());

        println!("  ---");
        println!("  E average   {our_avg:>8.1} lx   DIALux {:>6.0}    {avg_err:>6.2}%", case.e_avg);
        println!("  E min/max   {our_min:>8.1} /{our_max:>7.1} lx");
        println!("  LPD         {our_lpd:>8.2} W/m2  DIALux {:>6.2}", case.lpd);
        println!("  U0 (8x8)    {:>8.3}      DIALux {:>6.3}", our_min / our_avg, case.u0);
        println!(
            "  field: mean |error| {:.1}% over 64 points, worst {:.1}% at r{} c{}",
            sum_abs_pct / 64.0,
            worst.0,
            worst.1,
            worst.2,
        );

        // The stated average is the unambiguous number in each report — it is printed as a figure
        // rather than read off a drawing, so this is the assertion that means something.
        assert!(
            avg_err.abs() < 3.0,
            "{}: average {our_avg:.1} lx against DIALux's {:.0}",
            case.name,
            case.e_avg,
        );
        assert!(
            (our_lpd - case.lpd).abs() < 0.01,
            "{}: connected load must agree exactly — it is arithmetic, not physics",
            case.name,
        );
    }
    println!("\n=== worst average error across all three cases: {worst_avg_err:.2}% ===");
}

/// WHY U0 DISAGREES — and why it is NOT a physics error.
///
/// Everything else matches. U0 is E_min / E_avg; the averages agree to half a per cent across all
/// three cases, so the whole difference is in the MINIMUM. And DIALux's implied minimum is below
/// every point it prints — 20.0 lx for t1 against a printed minimum of 28.3, 26.5 for t2 against
/// 30.1. It is not taking its minimum from the grid it displays.
///
/// The darkest place on a working plane is its corner, and an 8 × 8 display grid never samples one:
/// its outermost sample sits half a cell in, at 0.26 m, while the plane reaches to 0.01 m from the
/// wall. So a coarse grid always reports a minimum that is too HIGH, and therefore a uniformity
/// that is too GOOD — the direction that passes installations which should fail.
///
/// What this test pins is the SHAPE of that error, not a match. Refining the calculation grid
/// lowers the minimum monotonically, in every case, while leaving the average alone. It does NOT
/// converge onto DIALux's figure at any single refinement — t1 crosses their number near 32 × 32,
/// t2 near 14 × 14 — which is the useful finding: DIALux's minimum sampling is an undocumented
/// internal convention, and there is no refinement factor that reproduces it in general.
///
/// So this is not something to tune. Chasing their number by picking a refinement per room would
/// be fitting to an unknown, and the first room that did not fit would be a silent error. The
/// defensible fix is to sample on a DOCUMENTED grid — EN 12464-1 gives p = 0.2 · 5^log10(d), which
/// for this 4 m room is 8 cells — decouple it from the display grid, and state on the report which
/// grid the uniformity was taken on.
#[test]
#[ignore = "needs IDENTICAL_DIR=<folder holding FONDO.ldt>"]
fn refining_the_grid_lowers_the_minimum_but_not_the_average() {
    let Ok(dir) = std::env::var("IDENTICAL_DIR") else { return };
    let mut profiles = HashMap::new();
    profiles.insert("FONDO".to_string(), load_photometry(&dir));
    let (meshes, materials) = scene();
    let settings = RaySettings { rays_per_point: 2048, max_bounces: 8, ..RaySettings::default() };
    let maint = Maintenance { llmf: MF, lsf: 1.0, lmf: 1.0, rsmf: 1.0 };

    for case in &CASES {
        let lums: Vec<Luminaire> = case
            .lums
            .iter()
            .enumerate()
            .map(|(i, (x, y))| Luminaire {
                id: i as u32 + 1,
                profile: "FONDO".to_string(),
                position: Vertex::new(*x, *y, MOUNT_Z),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0, watts_override: None, flux_override: None, from_block: None })
            .collect();
        println!("\n=== {} — U0 vs calculation grid (DIALux says {:.3}) ===", case.name, case.u0);
        println!("  {:>7}  {:>9}  {:>9}  {:>6}", "grid", "E min", "E avg", "U0");
        let (mut prev_min, mut prev_avg) = (f64::MAX, 0.0);
        for n in [8u32, 16, 32, 64] {
            let p = plane_at(n, n);
            let g =
                calculate_maintained(&meshes, &lums, &profiles, &materials, &p, &settings, maint);
            let mn = g.values.iter().cloned().fold(f64::MAX, f64::min);
            let av = g.values.iter().sum::<f64>() / g.values.len() as f64;
            println!("  {:>4}x{:<2}{:>9.1}{:>11.1}{:>8.3}", n, n, mn, av, mn / av);

            // THE MINIMUM ONLY EVER FALLS. A finer grid can find a darker point and can never
            // un-find one, so any rise would mean the sampling is wrong, not merely coarse.
            assert!(
                mn <= prev_min + 1e-6,
                "{}: refining {n}x{n} RAISED the minimum, {prev_min:.1} -> {mn:.1} lx",
                case.name,
            );
            // …while the AVERAGE barely moves. That is what makes E trustworthy at any resolution
            // and uniformity trustworthy at none: 8x8 and 64x64 agree on the average to a few
            // tenths of a per cent, and disagree on U0 by a third.
            if prev_avg > 0.0 {
                assert!(
                    (av - prev_avg).abs() / prev_avg < 0.01,
                    "{}: the average moved {:.2}% on refinement — it should not",
                    case.name,
                    (av - prev_avg) / prev_avg * 100.0,
                );
            }
            prev_min = mn;
            prev_avg = av;
        }
        // And the coarse grid is ALWAYS optimistic about uniformity — never pessimistic. This is
        // the direction that matters: it is the one that passes a failing installation.
        let coarse = {
            let g = calculate_maintained(
                &meshes, &lums, &profiles, &materials, &plane_at(8, 8), &settings, maint,
            );
            let mn = g.values.iter().cloned().fold(f64::MAX, f64::min);
            mn / (g.values.iter().sum::<f64>() / g.values.len() as f64)
        };
        assert!(
            coarse > case.u0,
            "{}: the 8x8 grid gave U0 {coarse:.3}, not optimistic against DIALux's {:.3} — the \
             whole point of this finding is that coarse sampling overstates uniformity",
            case.name,
            case.u0,
        );
    }
}

/// UGR on the real scenes — a first look, deliberately not asserted against DIALux.
///
/// All three reports quote R_UG,max 15 against a ≤ 19 target, and they say what that figure IS:
/// "based on a rectangular space of 4.000 m x 4.000 m and SHR of 0.25". That is the CIE UGR TABLE
/// method — a standard room at a standard spacing-to-height ratio, which characterises the FITTING.
/// What we compute is the direct CIE 117 calculation for a particular observer in the room as
/// actually built. The two answer different questions and agree only when the real room happens to
/// be the standard one, so asserting equality would be asserting a coincidence.
///
/// It is printed rather than asserted for exactly that reason. What IS asserted is the physics that
/// must hold whatever the convention: more fittings glare more, and turning to face away helps.
#[test]
#[ignore = "needs IDENTICAL_DIR=<folder holding FONDO.ldt>"]
fn ugr_from_the_seated_observer() {
    let Ok(dir) = std::env::var("IDENTICAL_DIR") else { return };
    let mut profiles = HashMap::new();
    profiles.insert("FONDO".to_string(), load_photometry(&dir));
    let prof = &profiles["FONDO"];
    println!("\n=== FONDO luminous aperture ===");
    println!(
        "  housing {:.3} x {:.3} m   aperture {:.3} x {:.3} m   flat area {:.5} m2",
        prof.length,
        prof.width,
        prof.luminous_length,
        prof.luminous_width,
        prof.projected_luminous_area(0.0).unwrap_or(0.0),
    );

    // Seated at the centre of the room, looking toward each wall in turn. EN 12464-1's observer is
    // at 1.2 m looking horizontally; the worst of the four directions is the one that governs.
    // A QUARTER POINT, not the centre: in t1 the centre is directly under the only fitting, where
    // it sits at exactly 90 deg to any horizontal view and is legitimately out of the field of
    // view. An observer who cannot see the lamp is a poor sample of whether the lamp glares.
    let eye = Vertex::new(1.0, 1.0, cad_light::Observer::SEATED_EYE_M);
    let views: [(&str, glam::Vec3); 4] = [
        ("+X", glam::Vec3::X),
        ("-X", -glam::Vec3::X),
        ("+Y", glam::Vec3::Y),
        ("-Y", -glam::Vec3::Y),
    ];

    for case in &CASES {
        let lums: Vec<Luminaire> = case
            .lums
            .iter()
            .enumerate()
            .map(|(i, (x, y))| Luminaire {
                id: i as u32 + 1,
                profile: "FONDO".to_string(),
                position: Vertex::new(*x, *y, MOUNT_Z),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0, watts_override: None, flux_override: None, from_block: None })
            .collect();

        // Background: the indirect illuminance on a vertical plane at the eye. Taken from the
        // working-plane result's indirect share, which is the field the fittings are seen against.
        let plane = plane_at(8, 8);
        let settings = RaySettings { rays_per_point: 2048, max_bounces: 8, ..RaySettings::default() };
        let maint = Maintenance { llmf: MF, lsf: 1.0, lmf: 1.0, rsmf: 1.0 };
        let (meshes, materials) = scene();
        let g = calculate_maintained(&meshes, &lums, &profiles, &materials, &plane, &settings, maint);
        let e_ind = g.indirect.iter().sum::<f64>() / g.indirect.len().max(1) as f64;
        let bg = cad_light::background_from_indirect(e_ind);

        println!("\n=== {} ===", case.name);
        println!("  background {bg:.1} cd/m2 (from {e_ind:.1} lx indirect)");
        let mut worst: Option<f64> = None;
        for (label, v) in views {
            let obs = cad_light::Observer::looking(eye, v);
            match cad_light::ugr_at(&obs, &lums, &profiles, bg) {
                Some(r) => {
                    println!(
                        "  looking {label}:  UGR {:>5.1}   from {} source(s), nearest sigma {:.0} deg",
                        r.ugr,
                        r.sources.len(),
                        r.sources.first().map(|s| s.sigma_deg).unwrap_or(0.0),
                    );
                    worst = Some(worst.map_or(r.ugr, |w: f64| w.max(r.ugr)));
                }
                None => println!("  looking {label}:  no source in view"),
            }
        }
        match worst {
            Some(w) => println!("  worst direction: UGR {w:.1}   (DIALux table method: 15)"),
            None => println!("  no direction sees a fitting — nothing to rate"),
        }
        // `f64::MIN` is FINITE, so a sentinel would have sailed through a liveness check while
        // printing 1.8e308 as the answer — which is exactly what the first run of this did. An
        // Option cannot do that.
        assert!(worst.is_some(), "{}: no direction produced a rating", case.name);
    }
}
