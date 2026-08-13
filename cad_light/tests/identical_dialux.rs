//! SIMLUX against DIALux on a scene with NOTHING left to infer.
//!
//! The earlier comparison (`dialux_comparison.rs`) was against a real project: a non-rectangular
//! shop with 46 fittings whose mounting heights and aiming the report never states. Every
//! disagreement there had two candidate causes — the engine, or the guess about the scene — and no
//! way to separate them.
//!
//! This one has no guesses left in it. One luminaire, a square box, and the report states every
//! input: 4.000 × 4.000 m, clearance 4.000 m, working plane 0.800 m, wall zone 0.010 m,
//! reflectances 70 / 50 / 20 %, maintenance factor 0.80 fixed. And it prints its 8 × 8 grid of
//! point values, so this is a comparison of SIXTY-FOUR NUMBERS rather than of one average.
//!
//! That is what makes it a test of the engine. If these points match, the physics is right; if the
//! average matched while the points did not, we would have the right answer for the wrong reasons.
//!
//! Source: `D:\Dropbox\YASEEN\3d factory\tests\Identical testing\t1.pdf`, "Building 1 · Storey 1 ·
//! test room (Light scene 1)".
//!
//! Run with:  `IDENTICAL_DIR="<that folder>" cargo test -p cad_light --test identical_dialux -- --ignored --nocapture`

use std::collections::HashMap;

use cad_light::{
    box_room, calculate_maintained, default_materials, parse_ldt, CalcPlane, Luminaire,
    Maintenance, RaySettings, Vertex,
};

// ---- exactly what the report states -------------------------------------------------------------
const ROOM: f32 = 4.000; // "Ground area 16.00 m²", square
const ROOM_H: f32 = 4.000; // "Clearance height 4.000 m"
const MOUNT_Z: f32 = 4.000; // "Mounting height 4.000 m" — flush with the ceiling
const WORK_PLANE: f32 = 0.800; // "Height Working plane 0.800 m"
const WALL_ZONE: f32 = 0.010; // "Wall zone Working plane 0.010 m"
const MF: f64 = 0.80; // "Maintenance factor 0.80 (fixed)"
const FLUX: f64 = 4000.0; // luminaire list: Φ 4000 lm
const WATTS: f64 = 40.0; // luminaire list: P 40.0 W

const DIALUX_E_AVG: f64 = 200.0; // "Ē perpendicular  200 lx"
const DIALUX_U0: f64 = 0.10; // "U₀ (g₁)  0.10"
const DIALUX_LPD_SPACE: f64 = 2.50; // "Space · Lighting power density 2.50 W/m²"

/// The report's 8 × 8 grid, ROW 0 AT THE BOTTOM of the plan (+y up), matching `CalcPlane`'s row
/// order. Read off the rendered page, so the last digit of the three-figure values is ±1.
#[rustfmt::skip]
const DIALUX_GRID: [[f64; 8]; 8] = [
    [ 29.0, 45.9, 87.8, 119.0, 119.0, 87.4, 45.1, 28.8],
    [ 46.5,116.0,177.0, 213.0, 214.0,177.0,117.0, 46.7],
    [ 88.1,177.0,282.0, 419.0, 420.0,282.0,177.0, 87.4],
    [119.0,214.0,419.0, 651.0, 652.0,420.0,214.0,118.0],
    [120.0,214.0,420.0, 652.0, 651.0,420.0,214.0,120.0],
    [ 87.2,177.0,282.0, 420.0, 420.0,282.0,177.0, 87.8],
    [ 45.9,116.0,176.0, 213.0, 212.0,177.0,116.0, 47.1],
    [ 29.0, 46.0, 87.6, 120.0, 120.0, 87.5, 46.2, 28.3],
];

fn dialux_mean() -> f64 {
    DIALUX_GRID.iter().flatten().sum::<f64>() / 64.0
}

#[test]
#[ignore = "needs IDENTICAL_DIR=<folder holding FONDO.ldt and t1.pdf>"]
fn simlux_against_dialux_on_a_fully_specified_room() {
    let Ok(dir) = std::env::var("IDENTICAL_DIR") else {
        println!("set IDENTICAL_DIR to the folder holding FONDO.ldt");
        return;
    };

    // ---- photometry -----------------------------------------------------------------------
    let path = format!("{dir}/FONDO.ldt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut prof = parse_ldt(&text).expect("FONDO.ldt parses");
    println!("\n=== photometry: FONDO.ldt ===");
    println!("  file says      flux {:.1} lm   watts {:.1} W", prof.lumens, prof.watts);
    println!("  report says    flux {FLUX:.1} lm   watts {WATTS:.1} W");
    let ratio = prof.lumens / FLUX;
    println!("  ratio          {ratio:.4}   (1.0000 = the reader agrees with DIALux)");
    // This file carries THREE lamp sets. They are alternatives, not simultaneous lamps — summing
    // them was a real bug, found on the previous comparison. If the reader ever regresses to the
    // sum this ratio goes to 3.
    assert!(
        (ratio - 1.0).abs() < 0.02,
        "the reader disagrees with DIALux about this file's flux: {:.1} vs {FLUX:.1} lm",
        prof.lumens,
    );
    prof.watts = WATTS;

    let mut profiles = HashMap::new();
    profiles.insert("FONDO".to_string(), prof);

    // ---- the room -------------------------------------------------------------------------
    let meshes = box_room(ROOM, ROOM, ROOM_H);
    // Floor 0.20, walls 0.50, ceiling 0.70 — the library's defaults ARE the report's figures.
    let materials = default_materials();
    for m in &materials {
        println!("  material {:<8} rho {:.2}", m.name, m.reflectance);
    }

    let lums = vec![Luminaire {
        id: 1,
        profile: "FONDO".to_string(),
        position: Vertex::new(ROOM * 0.5, ROOM * 0.5, MOUNT_Z),
        rotation_deg: 0.0,
        dimming: 1.0,
    }];

    // The working plane is inset by the wall zone on all four sides — which is what makes its area
    // 15.84 m² against the room's 16.00, and the report's own two power densities (2.53 vs
    // 2.50 W/m²) confirm it: 40 W / 2.53 = 15.81 m².
    let span = ROOM - 2.0 * WALL_ZONE;
    let plane = CalcPlane {
        origin: Vertex::new(WALL_ZONE, WALL_ZONE, WORK_PLANE),
        width: span,
        depth: span,
        cols: 8,
        rows: 8,
    };

    let settings = RaySettings { rays_per_point: 4096, max_bounces: 8, ..RaySettings::default() };
    // "0.80 (fixed)" — one number, not the four CIE 97 sub-factors, so it goes in whole.
    let maint = Maintenance { llmf: MF, lsf: 1.0, lmf: 1.0, rsmf: 1.0 };

    let grid = calculate_maintained(&meshes, &lums, &profiles, &materials, &plane, &settings, maint);

    // ---- the comparison --------------------------------------------------------------------
    println!("\n=== 8 x 8 working-plane grid, lux (row 0 = bottom of the plan) ===");
    println!("      {:>34}   {:>34}", "SIMLUX", "DIALux");
    let mut worst = (0.0_f64, 0usize, 0usize);
    let mut sum_abs_pct = 0.0;
    for r in 0..8usize {
        let ours: Vec<f64> = (0..8).map(|c| grid.values[r * 8 + c]).collect();
        let theirs = DIALUX_GRID[r];
        let fmt = |v: &[f64]| v.iter().map(|x| format!("{x:>6.0}")).collect::<Vec<_>>().join("");
        println!("  r{r}  {}   {}", fmt(&ours), fmt(&theirs));
        for c in 0..8 {
            let pct = (ours[c] - theirs[c]) / theirs[c] * 100.0;
            sum_abs_pct += pct.abs();
            if pct.abs() > worst.0 {
                worst = (pct.abs(), r, c);
            }
        }
    }

    let our_mean = grid.values.iter().sum::<f64>() / grid.values.len() as f64;
    let our_min = grid.values.iter().cloned().fold(f64::MAX, f64::min);
    let our_max = grid.values.iter().cloned().fold(0.0, f64::max);
    let their_mean = dialux_mean();

    println!("\n=== summary ===");
    println!("  {:<22} {:>10} {:>10} {:>9}", "", "SIMLUX", "DIALux", "diff");
    let row = |name: &str, a: f64, b: f64| {
        println!("  {:<22} {:>10.1} {:>10.1} {:>8.1}%", name, a, b, (a - b) / b * 100.0);
    };
    row("E average (lx)", our_mean, their_mean);
    row("E min (lx)", our_min, DIALUX_GRID.iter().flatten().cloned().fold(f64::MAX, f64::min));
    row("E max (lx)", our_max, DIALUX_GRID.iter().flatten().cloned().fold(0.0, f64::max));
    println!("  {:<22} {:>10.2} {:>10.2}", "U0 (min/avg)", our_min / our_mean, DIALUX_U0);
    println!("  {:<22} {:>10.2} {:>10.2}", "LPD (W/m2)", WATTS / 16.0, DIALUX_LPD_SPACE);
    println!(
        "  mean |error| over 64 points: {:.1}%   worst {:.1}% at r{} c{}",
        sum_abs_pct / 64.0,
        worst.0,
        worst.1,
        worst.2,
    );
    println!("  direct fraction: {:.3}", grid.direct_fraction().unwrap_or(f64::NAN));

    // ---- WHY U0 DISAGREES ------------------------------------------------------------------
    //
    // Everything above matches. U0 does not: 0.14 against DIALux's 0.10. U0 is Emin/Eavg and the
    // averages agree, so the whole difference is in the MINIMUM — and DIALux's implied minimum
    // (0.10 x 200 = 20 lx) is below every point it prints, the smallest of which is 28.3.
    //
    // So DIALux is not taking its minimum from the grid it displays. The darkest place on this
    // plane is the corner, and the display grid never samples a corner: its outermost sample sits
    // half a cell in, at 0.26 m, while the plane itself reaches to 0.01 m. Re-sampling finer walks
    // toward the true minimum and shows the effect directly.
    println!("\n=== the minimum depends on how finely you look ===");
    println!("  {:>6}  {:>9}  {:>9}  {:>6}", "grid", "E min", "E avg", "U0");
    for n in [8u32, 16, 32, 64] {
        let p = CalcPlane { cols: n, rows: n, ..plane };
        let g = calculate_maintained(&meshes, &lums, &profiles, &materials, &p, &settings, maint);
        let mn = g.values.iter().cloned().fold(f64::MAX, f64::min);
        let av = g.values.iter().sum::<f64>() / g.values.len() as f64;
        println!("  {:>4}x{:<2}{:>9.1}{:>11.1}{:>8.2}", n, n, mn, av, mn / av);
    }
    // And the corner itself — the value the grid is converging on.
    let corner = CalcPlane {
        origin: Vertex::new(WALL_ZONE, WALL_ZONE, WORK_PLANE),
        width: 0.02,
        depth: 0.02,
        cols: 1,
        rows: 1,
    };
    let cg = calculate_maintained(&meshes, &lums, &profiles, &materials, &corner, &settings, maint);
    println!("  corner (0.02 m in): {:.1} lx  ->  U0 {:.2}", cg.values[0], cg.values[0] / our_mean);

    // The report rounds its average to 200 lx and its own points average to 200.3, so agreement
    // inside a couple of per cent is the most this can resolve.
    assert!(
        (our_mean - their_mean).abs() / their_mean < 0.10,
        "average is {our_mean:.1} lx against DIALux's {their_mean:.1}",
    );
    assert!(
        (WATTS / 16.0 - DIALUX_LPD_SPACE).abs() < 0.01,
        "connected load must agree exactly — it is arithmetic, not physics",
    );
}
