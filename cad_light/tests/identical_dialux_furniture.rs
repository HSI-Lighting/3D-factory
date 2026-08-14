//! The same three DIALux rooms, WITH the furniture in them.
//!
//! `identical_dialux.rs` proved the engine on three empty boxes. This proves the thing that was
//! actually broken: until recently `meshes_from_factory` never handed the light engine any
//! furniture at all, so every room was computed as an empty box no matter what stood in it. That
//! is the whole of the +48 % against DIALux on the DISTRICT PEOPLE shop.
//!
//! The user re-ran all three identical-room cases in DIALux with one object placed at the room
//! centre, and built the SAME object into a SIMLUX project — so for the first time there is ground
//! truth for the furniture path rather than a unit test of our own arithmetic.
//!
//! WHAT THE OBJECT IS. Furniture asset 0 of `testfiles.simlux.json`: a motorbike, 481 738
//! triangles, local AABB 0.481 × 0.970 × 1.000 m, origin centred in x/y and at the base in z. It
//! stands on the floor at the centre of the 4 × 4 m room — which the DIALux plans confirm, their
//! silhouette measuring 0.51 × 0.94 m centred on (1.97, 2.02).
//!
//! WHAT IT CAN AND CANNOT SHOW. Be honest about the size of the effect being measured. The grid
//! DIALux prints NEVER SAMPLES THE OBJECT: with a 0.010 m wall zone the eight columns fall at
//! x = 0.259, 0.756, 1.254, 1.751, 2.249, 2.746, 3.244, 3.741, and the object spans 1.760 to
//! 2.240 — the two innermost columns miss it by 8 mm on each side. So DIALux's field moves by
//! about a per cent, all of it interreflection off a light object standing on a 0.20 floor, and
//! its stated average moves from 200 lx to 199.
//!
//! That makes these three cases a test of the ROOM, not of the shadow: adding furniture must not
//! disturb a result it has no business disturbing, and our field must track theirs point for
//! point. The shadow itself is tested separately and directly, on a grid fine enough to see it —
//! see `the_shadow_lands_where_the_object_stands`. A future file with a tall obstruction between
//! the fittings and the plane would let DIALux speak to the shadow too; this one cannot.
//!
//! Sources:
//!   `D:\Dropbox\YASEEN\3d factory\tests\Identical testing\t{1,2,3} with furniture.pdf`
//!   `D:\Dropbox\YASEEN\3d factory\tests\Identical testing\testfiles.simlux.json`
//!
//! Run with:
//!   IDENTICAL_DIR="<that folder>" \
//!   IDENTICAL_FURNITURE="<path to furniture.bin>" \
//!   cargo test -p cad_light --test identical_dialux_furniture -- --ignored --nocapture
//!
//! `furniture.bin` is the asset's triangle soup as raw little-endian f32 (9 per triangle) in its
//! LOCAL frame — the same bytes the project file carries in `furniture_lib[0].pos_b64`, which is
//! deflate+base64 of exactly this. It is not committed: 17 MB of someone's scanned mesh does not
//! belong in the repository, and the test says plainly what it needs when the variable is unset.

use std::collections::HashMap;

use cad_light::{
    box_room, calculate_maintained, default_materials, parse_ldt, CalcPlane, IesProfile, Luminaire,
    Maintenance, Material, Mesh, RaySettings, Triangle, Vertex, MATERIAL_FURNITURE,
};

// ---- stated identically by all three reports ----------------------------------------------------
const ROOM: f32 = 4.000; // "Ground area 16.00 m²", square
const ROOM_H: f32 = 4.000; // "Clearance height 4.000 m"
const MOUNT_Z: f32 = 4.000; // "Mounting height 4.000 m"
const WORK_PLANE: f32 = 0.800; // "Height Working plane 0.800 m"
const WALL_ZONE: f32 = 0.010; // "Wall zone Working plane 0.010 m"
const MF: f64 = 0.80; // "Maintenance factor 0.80 (fixed)"
const FLUX: f64 = 4000.0;
const WATTS: f64 = 40.0;

/// Where the object stands: the centre of the room, on the floor.
const FURN_XY: (f32, f32) = (2.0, 2.0);

struct Case {
    name: &'static str,
    lums: &'static [(f32, f32)],
    /// Ē as the with-furniture report states it, or `None` where that report's summary is stale.
    e_avg: Option<f64>,
    /// Ē of the SAME layout with the room empty — from `identical_dialux.rs`. The comparison that
    /// carries the information is not our value against theirs, it is how far each of us MOVED.
    e_avg_bare: f64,
    /// The printed 8 × 8 grid, row 0 at the BOTTOM of the plan (+y up), matching `CalcPlane`.
    /// `None` marks a point the object's silhouette covers in the print — DIALux draws the object
    /// over its own number there, so those are genuinely unavailable rather than guessed. It is
    /// always the column at x = 1.751 and no other: labels are drawn to the RIGHT of their cross,
    /// so that one starts underneath the object while x = 2.249 starts clear of it.
    grid: [[Option<f64>; 8]; 8],
}

const X: Option<f64> = None;
const fn n(v: f64) -> Option<f64> {
    Some(v)
}

#[rustfmt::skip]
const CASES: [Case; 3] = [
    Case {
        name: "t1f — one fitting, centred, with furniture",
        lums: &[(2.0, 2.0)],
        e_avg: n(199.0), e_avg_bare: 200.0,
        grid: [
            [n( 29.5), n( 46.2), n( 88.0), n(121.0), n(119.0), n( 86.2), n( 44.9), n( 28.6)],
            [n( 47.2), n(117.0), n(177.0), n(213.0), n(213.0), n(176.0), n(115.0), n( 46.0)],
            [n( 89.5), n(178.0), n(283.0), n(418.0), n(417.0), n(279.0), n(177.0), n( 86.2)],
            [n(120.0), n(215.0), n(422.0),        X, n(650.0), n(417.0), n(213.0), n(118.0)],
            [n(123.0), n(216.0), n(425.0),        X, n(653.0), n(419.0), n(213.0), n(118.0)],
            [n( 89.3), n(179.0), n(286.0), n(425.0), n(423.0), n(283.0), n(179.0), n( 88.0)],
            [n( 47.8), n(118.0), n(178.0), n(215.0), n(214.0), n(178.0), n(117.0), n( 47.3)],
            [n( 30.1), n( 47.6), n( 90.2), n(122.0), n(122.0), n( 89.1), n( 47.1), n( 29.2)],
        ],
    },
    Case {
        name: "t2f — two fittings, upper half, with furniture",
        lums: &[(1.0, 3.0), (3.0, 3.0)],
        e_avg: n(336.0), e_avg_bare: 336.0,
        grid: [
            [n( 30.4), n( 33.1), n( 36.6), n( 37.1), n( 37.1), n( 37.0), n( 34.8), n( 31.1)],
            [n( 45.7), n( 53.5), n( 59.4), n( 59.5), n( 61.5), n( 59.7), n( 51.7), n( 45.2)],
            [n(104.0), n(139.0), n(149.0), n(135.0), n(135.0), n(149.0), n(139.0), n(104.0)],
            [n(205.0), n(249.0), n(266.0),        X, n(299.0), n(268.0), n(250.0), n(208.0)],
            [n(326.0), n(468.0), n(516.0),        X, n(470.0), n(516.0), n(467.0), n(324.0)],
            [n(475.0), n(715.0), n(788.0), n(647.0), n(647.0), n(784.0), n(714.0), n(473.0)],
            [n(480.0), n(722.0), n(795.0), n(656.0), n(654.0), n(794.0), n(721.0), n(480.0)],
            [n(337.0), n(492.0), n(542.0), n(496.0), n(493.0), n(541.0), n(491.0), n(341.0)],
        ],
    },
    Case {
        // Its summary page reports Ē 995 lx / U0 0.074 — which its OWN printed grid contradicts,
        // averaging 338 lx. The file was saved from the same session as t2 (their sizes differ by
        // 0.2 %), the layout is t2 mirrored about y = 2, and a mirrored scene cannot have three
        // times the illuminance. The stale figure is not used; the grid is.
        name: "t3f — two fittings, lower half, with furniture",
        lums: &[(1.0, 1.0), (3.0, 1.0)],
        e_avg: X, e_avg_bare: 336.0,
        grid: [
            [n(339.0), n(490.0), n(541.0), n(492.0), n(494.0), n(541.0), n(489.0), n(338.0)],
            [n(480.0), n(722.0), n(794.0), n(655.0), n(654.0), n(795.0), n(723.0), n(473.0)],
            [n(475.0), n(714.0), n(787.0), n(647.0), n(648.0), n(784.0), n(715.0), n(474.0)],
            [n(322.0), n(469.0), n(517.0),        X, n(470.0), n(516.0), n(469.0), n(325.0)],
            [n(206.0), n(249.0), n(266.0),        X, n(300.0), n(266.0), n(248.0), n(207.0)],
            [n(104.0), n(139.0), n(147.0),        X, n(136.0), n(150.0), n(139.0), n(105.0)],
            [n( 44.9), n( 53.8), n( 57.8), n( 62.1), n( 60.8), n( 60.5), n( 52.3), n( 45.3)],
            [n( 28.9), n( 34.0), n( 37.6), n( 37.2), n( 38.4), n( 38.5), n( 34.0), n( 29.1)],
        ],
    },
];

/// The object, posed where it stands: raw little-endian f32, 9 per triangle, local frame.
fn furniture_mesh(path: &str) -> Mesh {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert!(bytes.len() % 36 == 0, "{path}: {} bytes is not a whole number of triangles", bytes.len());
    let floats: Vec<f32> =
        bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();

    let mut vertices = Vec::with_capacity(floats.len() / 3);
    for p in floats.chunks_exact(3) {
        // The asset's origin is centred in x/y and at its base in z, so the placement is a
        // translation onto the room centre with the base left on the floor.
        vertices.push(Vertex::new(p[0] + FURN_XY.0, p[1] + FURN_XY.1, p[2]));
    }
    let triangles =
        (0..vertices.len() as u32 / 3).map(|t| Triangle { a: t * 3, b: t * 3 + 1, c: t * 3 + 2 }).collect();
    Mesh { vertices, triangles, material: MATERIAL_FURNITURE }
}

fn plane_at(cols: u32, rows: u32) -> CalcPlane {
    let span = ROOM - 2.0 * WALL_ZONE;
    CalcPlane { origin: Vertex::new(WALL_ZONE, WALL_ZONE, WORK_PLANE), width: span, depth: span, cols, rows }
}

fn load_photometry(dir: &str) -> IesProfile {
    let path = format!("{dir}/FONDO.ldt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut prof = parse_ldt(&text).expect("FONDO.ldt parses");
    let ratio = prof.lumens / FLUX;
    assert!((ratio - 1.0).abs() < 0.02, "flux {:.1} lm against DIALux's {FLUX:.1}", prof.lumens);
    prof.watts = WATTS;
    prof
}

fn luminaires(case: &Case) -> Vec<Luminaire> {
    case.lums
        .iter()
        .enumerate()
        .map(|(i, (x, y))| Luminaire {
            id: i as u32 + 1,
            profile: "FONDO".to_string(),
            position: Vertex::new(*x, *y, MOUNT_Z),
            rotation_deg: 0.0,
            dimming: 1.0, watts_override: None, flux_override: None })
        .collect()
}

fn run(
    meshes: &[Mesh],
    materials: &[Material],
    case: &Case,
    profiles: &HashMap<String, IesProfile>,
    settings: &RaySettings,
) -> Vec<f64> {
    let maint = Maintenance { llmf: MF, lsf: 1.0, lmf: 1.0, rsmf: 1.0 };
    calculate_maintained(
        meshes,
        &luminaires(case),
        profiles,
        materials,
        &plane_at(8, 8),
        settings,
        maint,
    )
    .values
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

#[test]
#[ignore = "needs IDENTICAL_DIR (FONDO.ldt) and IDENTICAL_FURNITURE (furniture.bin)"]
fn simlux_against_dialux_with_the_furniture_in_the_room() {
    let (Ok(dir), Ok(furn)) =
        (std::env::var("IDENTICAL_DIR"), std::env::var("IDENTICAL_FURNITURE"))
    else {
        println!(
            "set IDENTICAL_DIR to the folder holding FONDO.ldt and IDENTICAL_FURNITURE to the \
             furniture blob (raw le f32, 9 per triangle — furniture_lib[0].pos_b64 inflated)"
        );
        return;
    };

    let mut profiles = HashMap::new();
    profiles.insert("FONDO".to_string(), load_photometry(&dir));
    let materials = default_materials();
    let settings = RaySettings { rays_per_point: 4096, max_bounces: 8, ..RaySettings::default() };

    let empty = box_room(ROOM, ROOM, ROOM_H);
    let piece = furniture_mesh(&furn);
    println!("\n=== the object ===");
    println!("  {} triangles", piece.triangles.len());
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &piece.vertices {
        for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            lo[k] = lo[k].min(c);
            hi[k] = hi[k].max(c);
        }
    }
    println!("  world AABB  x {:.3}..{:.3}   y {:.3}..{:.3}   z {:.3}..{:.3}", lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]);
    assert!(lo[2] > -1e-3 && lo[2] < 1e-3, "it must stand ON the floor, not float or sink: z starts at {:.3}", lo[2]);
    assert!(hi[2] > WORK_PLANE, "an object entirely under the working plane could not shade it at all");

    let mut furnished = empty.clone();
    furnished.push(piece);

    let mut worst_avg_err = 0.0_f64;
    for case in &CASES {
        let bare = run(&empty, &materials, case, &profiles, &settings);
        let ours = run(&furnished, &materials, case, &profiles, &settings);

        println!("\n================ {} ================", case.name);
        println!("      {:>34}   {:>34}", "SIMLUX (furnished)", "DIALux (furnished)");
        let (mut sum_abs_pct, mut counted, mut worst) = (0.0, 0usize, (0.0_f64, 0usize, 0usize));
        for r in 0..8usize {
            let fmt_ours =
                (0..8).map(|c| format!("{:>6.0}", ours[r * 8 + c])).collect::<Vec<_>>().join("");
            let fmt_dial = (0..8)
                .map(|c| case.grid[r][c].map_or("     ·".to_string(), |v| format!("{v:>6.0}")))
                .collect::<Vec<_>>()
                .join("");
            println!("  r{r}  {fmt_ours}   {fmt_dial}");
            for c in 0..8 {
                let Some(theirs) = case.grid[r][c] else { continue };
                let pct = (ours[r * 8 + c] - theirs) / theirs * 100.0;
                sum_abs_pct += pct.abs();
                counted += 1;
                if pct.abs() > worst.0 {
                    worst = (pct.abs(), r, c);
                }
            }
        }

        // THE MEASUREMENT THAT CARRIES THE INFORMATION. Both programs were given the same room and
        // the same object; what is comparable is not the absolute level (already established on the
        // empty rooms) but the SHIFT the object caused in each.
        let our_avg = mean(&ours);
        let our_bare = mean(&bare);
        let our_shift = (our_avg - our_bare) / our_bare * 100.0;

        println!("  ---");
        println!("  E average   {our_avg:>8.1} lx   empty room {our_bare:>7.1} lx   shift {our_shift:>+6.2}%");
        if let Some(theirs) = case.e_avg {
            let their_shift = (theirs - case.e_avg_bare) / case.e_avg_bare * 100.0;
            let err = (our_avg - theirs) / theirs * 100.0;
            println!("  DIALux      {theirs:>8.1} lx   empty room {:>7.1} lx   shift {their_shift:>+6.2}%", case.e_avg_bare);
            println!("  against DIALux: {err:>+.2}%");
            worst_avg_err = worst_avg_err.max(err.abs());
            assert!(
                err.abs() < 3.0,
                "{}: {our_avg:.1} lx against DIALux's {theirs:.0}",
                case.name,
            );
        } else {
            println!("  DIALux      (summary stale — grid only)");
        }
        println!(
            "  field: mean |error| {:.1}% over {counted} points, worst {:.1}% at r{} c{}",
            sum_abs_pct / counted as f64,
            worst.0,
            worst.1,
            worst.2,
        );

        // The object is 3 % of the plan area and mostly below the plane. If including it moved the
        // room by more than a few per cent, something is wrong with the pose, the scale or the
        // material — and DIALux, given the same object, moves by well under one.
        assert!(
            our_shift.abs() < 5.0,
            "{}: adding a 0.47 m² object to a 15.84 m² plane moved the average by {our_shift:+.1}% \
             — check the pose and the scale before believing it",
            case.name,
        );
        // …but it must move it SOMEHOW. An exact tie to the last bit would mean the mesh never
        // reached the tracer, which is the bug this whole file exists to catch.
        assert!(
            ours.iter().zip(&bare).any(|(a, b)| (a - b).abs() > 1e-6),
            "{}: the furnished room came out bit-identical to the empty one — the object never \
             reached the tracer",
            case.name,
        );
    }
    println!("\n=== worst average error against DIALux, furnished: {worst_avg_err:.2}% ===");
}

/// The object HAS to be seen by the tracer, and seen WHERE IT STANDS.
///
/// Nothing above can show this on its own, because DIALux's 8 × 8 grid never samples the object:
/// its four innermost columns sit at x = 1.751 and 2.249 while the object spans 1.760 to 2.240, so
/// every printed point misses it by 8 mm. A pose error of a few centimetres — or no furniture in
/// the tracer at all — would leave all three grids looking exactly as correct.
///
/// So this asks the question directly, on a 200 × 200 grid fine enough to resolve a motorbike's
/// frame: run the working plane with the object and without it, and look at WHERE the light went.
/// It does not compare an average over a bounding box — the AABB of a motorbike is mostly air, and
/// asserting on it would be asserting on the wheelbase. It compares the two fields cell by cell.
#[test]
#[ignore = "needs IDENTICAL_DIR and IDENTICAL_FURNITURE"]
fn the_shadow_lands_where_the_object_stands() {
    let (Ok(dir), Ok(furn)) =
        (std::env::var("IDENTICAL_DIR"), std::env::var("IDENTICAL_FURNITURE"))
    else {
        return;
    };
    let mut profiles = HashMap::new();
    profiles.insert("FONDO".to_string(), load_photometry(&dir));
    let materials = default_materials();

    let empty = box_room(ROOM, ROOM, ROOM_H);
    let mut furnished = empty.clone();
    furnished.push(furniture_mesh(&furn));

    // DIRECT ONLY, one ray: bounced light fills a shadow in, and this is measuring the shadow.
    let settings = RaySettings { rays_per_point: 1, max_bounces: 0, shadows: true };
    let lums = luminaires(&CASES[0]); // the centred fitting, straight above the object
    const N: usize = 200;
    let plane =
        CalcPlane { origin: Vertex::new(1.0, 1.0, WORK_PLANE), width: 2.0, depth: 2.0, cols: N as u32, rows: N as u32 };
    let maint = Maintenance { llmf: MF, lsf: 1.0, lmf: 1.0, rsmf: 1.0 };
    let run_one = |m: &[Mesh]| {
        calculate_maintained(m, &lums, &profiles, &materials, &plane, &settings, maint).values
    };
    let (open, shaded) = (run_one(&empty), run_one(&furnished));

    // A cell is "lost" when the object took most of its direct light away.
    let (mut lost, mut deepest) = (Vec::new(), 1.0_f64);
    for r in 0..N {
        for c in 0..N {
            let i = r * N + c;
            if open[i] <= 0.0 {
                continue;
            }
            let ratio = shaded[i] / open[i];
            deepest = deepest.min(ratio);
            if ratio < 0.5 {
                lost.push((1.0 + (c as f32 + 0.5) * 2.0 / N as f32, 1.0 + (r as f32 + 0.5) * 2.0 / N as f32));
            }
        }
    }
    let area = lost.len() as f64 * (2.0 / N as f64).powi(2);
    println!("\n{} of {} cells lost over half their direct light — {area:.4} m²", lost.len(), N * N);
    println!("deepest cell keeps {:.1}% of its open-room direct light", deepest * 100.0);

    assert!(
        !lost.is_empty(),
        "not one cell darkened: the mesh never reached the tracer, which is the bug this whole \
         file exists to catch",
    );
    // Every darkened cell must lie under the object. The fitting is directly overhead, so the
    // shadow is cast almost straight down and cannot appear outside the footprint — if it does,
    // the pose is wrong.
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for &(x, y) in &lost {
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    }
    println!("shadow extent  x {:.3}..{:.3}   y {:.3}..{:.3}", lo.0, hi.0, lo.1, hi.1);
    // The object's own footprint, plus 6 cm for the spread of a shadow cast from 3.2 m above by a
    // source of finite size and read on a plane 0.8 m up.
    let (hx, hy) = (0.2404 + 0.06, 0.4849 + 0.06);
    assert!(
        lo.0 > FURN_XY.0 - hx && hi.0 < FURN_XY.0 + hx && lo.1 > FURN_XY.1 - hy && hi.1 < FURN_XY.1 + hy,
        "the shadow falls outside the object's footprint — the mesh is posed somewhere it is not",
    );
    // And it has to be a real shadow rather than one stray triangle clipped by the plane.
    //
    // Do not read the SIZE of it as small. The fitting is directly overhead and the plane is at
    // 0.800 m, so the only part of the object that can shade anything is the 0.200 m above that
    // line — the handlebars and the top of the tank, and nothing else. The wheels, the engine and
    // the frame all sit below the plane and cast their shadows onto the floor, not onto it. The
    // measured 0.026 m² at x 1.82–2.19, y 2.00–2.19 is exactly the handlebar and tank, and the
    // rest of the object correctly shades nothing.
    assert!(area > 0.01, "only {area:.4} m² darkened — too little to be the object");
    // Somewhere the direct component has to be gone OUTRIGHT. A grazing edge dims a cell; only
    // solid geometry between the fitting and the plane removes it.
    assert!(deepest < 0.05, "nothing was fully occluded — the deepest cell kept {:.0}%", deepest * 100.0);
}
