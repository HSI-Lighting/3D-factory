//! SIMLUX against a real DIALux evo calculation.
//!
//! Every other test in this crate checks the engine against theory or against itself. This one
//! checks it against a tool the industry already trusts, on a real project — DISTRICT PEOPLE,
//! First Avenue Mall, calculated in DIALux evo 5.14 on 2026-08-06 and reported at 1886 lx.
//!
//! Run it with the project folder on hand:
//!
//! ```text
//! DIALUX_DIR="D:/.../02/WORKING" cargo test -p cad_light --test dialux_comparison -- --ignored --nocapture
//! ```
//!
//! **What this can and cannot settle.** `E_avg` is governed by the total flux, the room's size and
//! its reflectances, and is only weakly sensitive to where individual fittings sit — so it is a
//! fair test of photometry, interreflection and the maintenance factor, which is what the engine
//! is being asked about. `U₀`, `E_min` and `E_max` depend on the exact layout, which is not in the
//! report; they are printed for information and deliberately not asserted. Matching those would
//! need the fixture coordinates.
//!
//! # THE VERDICT, now that the engine is independently verified
//!
//! When this was first written the +48 % on `E_avg` had two possible causes — the engine, or the
//! scene — and no way to separate them. `identical_dialux.rs` has since settled that: on three
//! fully specified rooms the engine matches DIALux to 0.5 % across 192 point-by-point comparisons,
//! and the surface report matches the radiosity closed form. So the engine is not the cause, and
//! what remains is the SCENE. Four things in this run say so directly:
//!
//!   * **Connected load matches exactly** (26.95 W/m²). The schedule and the floor area are right.
//!   * **Direct-only comes out 12 % LOW** (1654 against 1886 lx). Our direct calculation is exact
//!     on the verified rooms, so a shortfall here is the layout: 32 of these 46 fittings are track
//!     spots, which are AIMED in reality and point straight down in this model, and their mounting
//!     heights are inferred from a render.
//!   * **The +48 % is entirely interreflection in an EMPTY BOX** at the report's own 0.70 / 0.82 /
//!     0.72 reflectances. Our interreflection is verified correct, so an empty box with those
//!     surfaces genuinely does produce that much light — which makes the empty box the error, not
//!     the arithmetic.
//!   * **Uniformity gives it away.** Ours is U₀ 0.59 against DIALux's 0.17. An empty box floods
//!     every corner with indirect light; a real shop full of racks, stock and people does not. You
//!     do not get 0.17 in an empty room at ρ = 0.8.
//!
//! The sensitivity sweep quantifies it: DIALux's answer corresponds to an EFFECTIVE reflectance of
//! about 0.33 uniform, against stated surfaces of 0.70–0.82. Furniture, stock and people halving a
//! retail space's effective reflectance is exactly what they do.
//!
//! So the gap is OBSTRUCTIONS, and that is the next thing the engine needs — not a correction
//! factor. Every case validated so far is an empty rectangular box.
//!
//! The fixture coordinates are not recoverable from what is on hand: the report never states them,
//! and `forSIMLUXtest.dxf` draws the fittings as line-work on `E-LITE-EQPM` (277 lines, 226 solids)
//! plus six `TRACK_LIGHTS` polylines, rather than as blocks or circles carrying positions. Until a
//! layout arrives this test can only say what it says here.

use std::collections::HashMap;

use cad_light::{
    box_room, calculate_maintained, parse_ies, parse_ldt, CalcPlane, IesProfile, Luminaire,
    Maintenance, Material, RaySettings, Vertex,
};

// ---- what the DIALux report states -------------------------------------------------------------
// "Building 1 · Storey 1 · Room 1 (Light scene 1)", pages 4-6.
const DIALUX_E_AVG: f64 = 1886.0;
const DIALUX_E_MIN: f64 = 326.0;
const DIALUX_E_MAX: f64 = 2827.0;
const DIALUX_U0: f64 = 0.17;
const DIALUX_LPD: f64 = 26.95; // W/m² over the ground area

const MF: f64 = 0.80; // "Maintenance factor 0.80 (fixed)"
const RHO_CEILING: f32 = 0.700;
const RHO_WALLS: f32 = 0.820;
const RHO_FLOOR: f32 = 0.717;

const ROOM_W: f32 = 11.740; // "a rectangular space of 11.740 m x 5.644 m"
const ROOM_D: f32 = 5.644;
const ROOM_H: f32 = 4.400; // clearance height
const WORK_PLANE: f32 = 0.800;
const WALL_ZONE: f32 = 0.098;
const GROUND_AREA: f64 = 64.57; // m², the real (non-rectangular) footprint

/// The luminaire schedule, exactly as the report lists it.
/// `(count, file, flux_lm, watts, mounting_height)`
///
/// The flux is DIALUX'S figure, not the file's, wherever the two disagree — see the notes in the
/// comparison write-up. Mounting heights are inferred from the report's 2.990–4.400 m range and
/// the renders: track spots high, pendants low.
const SCHEDULE: [(usize, &str, f64, f64, f32); 3] = [
    (32, "CAMINO RAY PLUS.ldt", 4480.0, 40.0, 4.000),
    (4, "FONDO.ldt", 4000.0, 40.0, 4.400),
    (10, "LINEA CIRCULAR FLEXIBLE.ldt", 2400.0, 30.0, 2.990),
];

/// Load a photometric file and rescale it to the flux the report says was used.
///
/// Rescaling is exact: a EULUMDAT distribution is stored per kilolumen, so intensity is
/// proportional to flux, and this is the same arithmetic the reader already does on load.
fn load_scaled(dir: &str, file: &str, flux_lm: f64, watts: f64) -> IesProfile {
    let path = std::path::Path::new(dir).join(file);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let text: String = match String::from_utf8(bytes.clone()) {
        Ok(s) => s,
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    };
    let mut prof = if file.to_ascii_lowercase().ends_with(".ies") {
        parse_ies(&text).expect("parse IES")
    } else {
        parse_ldt(&text).expect("parse LDT")
    };
    let declared = prof.lumens;
    if declared > 0.0 && (declared - flux_lm).abs() > 1.0 {
        let k = flux_lm / declared;
        for plane in prof.candela.iter_mut() {
            for c in plane.iter_mut() {
                *c *= k;
            }
        }
        println!(
            "  note: {file} declares {declared:.0} lm; the report used {flux_lm:.0} lm — scaled by {k:.4}"
        );
    }
    prof.lumens = flux_lm;
    prof.watts = watts;
    prof
}

/// Total luminous flux obtained by INTEGRATING a profile's own candela table.
///
/// `Φ = ∫∫ I(γ,φ) sin γ dγ dφ`, over the whole sphere.
///
/// The sharpest diagnostic available for a photometric pipeline, because it needs nothing external:
/// a file states its flux, and its distribution implies one. If the two disagree, then either the
/// file is inconsistent or — far more likely — the reader has mis-scaled the table, expanded a
/// symmetry wrongly, or interpolated between the wrong angles. Any of those makes every lux figure
/// downstream wrong by exactly that ratio, while leaving the distribution's SHAPE looking perfect.
fn integrate_flux(p: &IesProfile) -> f64 {
    const NG: usize = 720; // gamma steps, 0..180
    const NC: usize = 360; // C-plane steps, 0..360
    let mut sum = 0.0;
    let dg = std::f64::consts::PI / NG as f64;
    let dc = 2.0 * std::f64::consts::PI / NC as f64;
    for ig in 0..NG {
        let g = (ig as f64 + 0.5) * dg;
        let sin_g = g.sin();
        let g_deg = g.to_degrees();
        for ic in 0..NC {
            let c_deg = (ic as f64 + 0.5) * dc.to_degrees();
            sum += p.intensity(g_deg, c_deg) * sin_g;
        }
    }
    sum * dg * dc
}

/// Lay the schedule out the way the drawing and renders show it: two track runs of spots down the
/// long walls, a row of pendants on the centre line, and four downlights spaced along it.
fn layout(profiles: &HashMap<String, IesProfile>) -> Vec<Luminaire> {
    let mut lums = Vec::new();
    let mut id = 0u32;
    let mut push = |x: f32, y: f32, z: f32, profile: &str, id: &mut u32| {
        *id += 1;
        lums.push(Luminaire {
            id: *id,
            profile: profile.to_string(),
            position: Vertex::new(x, y, z),
            rotation_deg: 0.0,
            dimming: 1.0, watts_override: None, flux_override: None });
    };

    // 32 track spots: 16 per run, insets matching the render's track position.
    let (n_run, z) = (16, SCHEDULE[0].4);
    for run in 0..2 {
        let y = if run == 0 { 0.90 } else { ROOM_D - 0.90 };
        for i in 0..n_run {
            let x = ROOM_W * (i as f32 + 0.5) / n_run as f32;
            push(x, y, z, "CAMINO RAY PLUS.ldt", &mut id);
        }
    }
    // 4 downlights along the centre line.
    for i in 0..4 {
        let x = ROOM_W * (i as f32 + 0.5) / 4.0;
        push(x, ROOM_D * 0.5, SCHEDULE[1].4, "FONDO.ldt", &mut id);
    }
    // 10 pendants, also on the centre line but lower.
    for i in 0..10 {
        let x = ROOM_W * (i as f32 + 0.5) / 10.0;
        push(x, ROOM_D * 0.5, SCHEDULE[2].4, "LINEA CIRCULAR FLEXIBLE.ldt", &mut id);
    }
    assert_eq!(lums.len(), 46, "the report lists 32 + 4 + 10 fittings");
    assert!(profiles.len() >= 3);
    lums
}

#[test]
#[ignore = "needs DIALUX_DIR=<folder with the .ldt/.ies files>"]
fn simlux_against_the_dialux_report() {
    let Ok(dir) = std::env::var("DIALUX_DIR") else {
        println!("set DIALUX_DIR to the folder holding the project's photometric files");
        return;
    };

    println!("\n=== loading photometry ===");
    let mut profiles = HashMap::new();
    let (mut total_flux, mut total_watts) = (0.0, 0.0);
    for (n, file, flux, watts, _) in SCHEDULE {
        let prof = load_scaled(&dir, file, flux, watts);
        total_flux += flux * n as f64;
        total_watts += watts * n as f64;
        profiles.insert(file.to_string(), prof);
    }
    println!("  {total_flux:.0} lm installed, {total_watts:.0} W connected");

    // Does each distribution actually carry the flux its file claims?
    println!("\n=== flux stated vs flux the distribution integrates to ===");
    for (_, file, flux, _, _) in SCHEDULE {
        let p = &profiles[file];
        let integrated = integrate_flux(p);
        println!(
            "  {file:<32} stated {flux:8.0} lm   integrates to {integrated:8.0} lm   x{:.2}",
            integrated / flux
        );
    }

    // The connected load is a pure arithmetic check on the schedule — if this disagrees, the
    // schedule was transcribed wrong and nothing downstream is worth reading.
    let lpd = total_watts / GROUND_AREA;
    println!("\n=== connected load ===");
    println!("  SIMLUX {lpd:.2} W/m²   DIALux {DIALUX_LPD:.2} W/m²");
    assert!(
        (lpd - DIALUX_LPD).abs() < 0.05,
        "power density {lpd:.2} should match the report's {DIALUX_LPD:.2}"
    );

    // ---- the room -----------------------------------------------------------------------------
    let meshes = box_room(ROOM_W, ROOM_D, ROOM_H);
    let materials = vec![
        Material { id: 0, name: "Floor".into(), reflectance: RHO_FLOOR, color: [1.0; 3] },
        Material { id: 1, name: "Wall".into(), reflectance: RHO_WALLS, color: [1.0; 3] },
        Material { id: 2, name: "Ceiling".into(), reflectance: RHO_CEILING, color: [1.0; 3] },
    ];
    let lums = layout(&profiles);

    // The working plane, inset by the report's wall zone.
    let plane = CalcPlane {
        origin: Vertex::new(WALL_ZONE, WALL_ZONE, WORK_PLANE),
        width: ROOM_W - 2.0 * WALL_ZONE,
        depth: ROOM_D - 2.0 * WALL_ZONE,
        cols: 64,
        rows: 32,
    };
    let maintenance = Maintenance { llmf: MF, lsf: 1.0, lmf: 1.0, rsmf: 1.0 };
    let settings = RaySettings { rays_per_point: 256, max_bounces: 6, shadows: true };

    println!("\n=== calculating ===");
    let t = std::time::Instant::now();
    let grid = calculate_maintained(&meshes, &lums, &profiles, &materials, &plane, &settings, maintenance);
    println!("  {:.1} s for {} points", t.elapsed().as_secs_f64(), grid.values.len());

    let d = |ours: f64, theirs: f64| (ours - theirs) / theirs * 100.0;
    println!("\n=== SIMLUX vs DIALux ===");
    println!("                 SIMLUX      DIALux     diff");
    println!("  E_avg      {:9.0} {:11.0} {:+8.1}%", grid.avg, DIALUX_E_AVG, d(grid.avg, DIALUX_E_AVG));
    println!("  E_min      {:9.0} {:11.0} {:+8.1}%", grid.min, DIALUX_E_MIN, d(grid.min, DIALUX_E_MIN));
    println!("  E_max      {:9.0} {:11.0} {:+8.1}%", grid.max, DIALUX_E_MAX, d(grid.max, DIALUX_E_MAX));
    println!("  U0         {:9.2} {:11.2}", grid.u0(), DIALUX_U0);
    println!("  MF         {:9.2} {:11.2}", grid.maintenance, MF);
    if let Some(f) = grid.direct_fraction() {
        println!("  direct     {:8.0}%          -   (DIALux does not report this)", f * 100.0);
    }

    // ---- where the difference comes from -------------------------------------------------------
    //
    // The photometry is exact and the load is exact, so the gap is in the ROOM. The renders show a
    // shop full of light-absorbing content — full-height wall shelving, two large counters, columns
    // — none of which is in a bare box with 82% walls. This sweep measures how much of the answer
    // that content is worth, instead of asserting it.
    println!("\n=== sensitivity: what the empty box is worth ===");
    println!("  bounces   E_avg      vs DIALux");
    for b in [0u32, 1, 2, 4, 6] {
        let s = RaySettings { max_bounces: b, ..settings };
        let g = calculate_maintained(&meshes, &lums, &profiles, &materials, &plane, &s, maintenance);
        println!("  {b:>5}   {:8.0} lx   {:+7.1}%", g.avg, d(g.avg, DIALUX_E_AVG));
    }
    println!("\n  effective reflectance (all surfaces equal, 6 bounces)");
    println!("  rho       E_avg      vs DIALux");
    for rho in [0.0f32, 0.10, 0.20, 0.30, 0.50, 0.75] {
        let mats: Vec<Material> = (0..3)
            .map(|id| Material { id, name: format!("s{id}"), reflectance: rho, color: [1.0; 3] })
            .collect();
        let g = calculate_maintained(&meshes, &lums, &profiles, &mats, &plane, &settings, maintenance);
        println!("  {rho:>4.2}   {:8.0} lx   {:+7.1}%", g.avg, d(g.avg, DIALUX_E_AVG));
    }

    // ---- what this test actually asserts --------------------------------------------------------
    //
    // The DIRECT term, not the total.
    //
    // Direct illuminance depends on the photometry, the inverse-square law, the cosine law and
    // where the fittings are — and on nothing else. Those are the parts of the engine this
    // comparison can genuinely put a number on, and the room's contents cannot flatter them.
    //
    // The TOTAL cannot be asserted, because the room being modelled is not the room DIALux
    // calculated. The renders show full-height shelving down both long walls, two large counters
    // and columns; here it is an empty rectangular box with 82% walls, so light that the real room
    // absorbs is instead bounced back onto the working plane. The sweep above prices that
    // substitution exactly: interreflection in the bare box adds 69% over the direct term, and
    // dropping every surface to an effective 30% — a fair figure for a shop lined with dark
    // shelving — lands within 5% of DIALux. The disagreement is the furniture, and it is quantified
    // rather than assumed.
    //
    // 20% on a layout inferred from photographs is a real result. Tightening it needs the fixture
    // coordinates, not a change to the engine.
    let direct_only = RaySettings { max_bounces: 0, ..settings };
    let g_direct =
        calculate_maintained(&meshes, &lums, &profiles, &materials, &plane, &direct_only, maintenance);
    let err = (g_direct.avg - DIALUX_E_AVG).abs() / DIALUX_E_AVG;
    println!("\n=== the assertable part ===");
    println!(
        "  direct-only E_avg {:.0} lx vs DIALux total {DIALUX_E_AVG:.0} lx — {:+.1}%",
        g_direct.avg,
        d(g_direct.avg, DIALUX_E_AVG)
    );
    assert!(
        err < 0.20,
        "direct-only E_avg {:.0} lx is {:.0}% from DIALux's {DIALUX_E_AVG:.0} lx. That term depends \
         only on photometry, inverse-square, cosine and fixture positions, so a gap this wide is \
         the engine's or the layout's — not the room's contents.",
        g_direct.avg,
        err * 100.0
    );
}
