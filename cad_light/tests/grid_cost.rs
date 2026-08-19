//! WHAT A GRID POINT COSTS — the measurement the display grid's point budget is set from.
//!
//! Every point on the calculation plane is a full trace against the scene, so the budget is a
//! wall-clock decision and it should be made from a number rather than from a round figure that
//! looks safe. Run it in RELEASE, which is what a person pressing ⚡ Calculate is running:
//!
//!     cargo test -p cad_light --release --test grid_cost -- --ignored --nocapture

use cad_light::*;
use std::collections::HashMap;
use std::time::Instant;

/// A room the size of the owner's gym, with a realistic number of fittings in it.
fn gym(cols: u32, rows: u32) -> (Vec<Mesh>, Vec<Luminaire>, HashMap<String, IesProfile>, CalcPlane) {
    let (w, d, h) = (33.0_f32, 13.0_f32, 3.3_f32);
    let meshes = extrude::box_room(w, d, h);
    let profile = downlight();
    let mut profiles = HashMap::new();
    profiles.insert("builtin".to_string(), profile);
    // A 6 x 3 array of fittings, which is about what a hall this size carries.
    let mut lums = Vec::new();
    for i in 0..6 {
        for j in 0..3 {
            lums.push(Luminaire {
                id: (i * 3 + j + 1) as u32,
                profile: "builtin".to_string(),
                position: Vertex::new(
                    w * (i as f32 + 0.5) / 6.0,
                    d * (j as f32 + 0.5) / 3.0,
                    h - 0.2,
                ),
                rotation_deg: 0.0,
                dimming: 1.0,
                from_block: None,
                flux_override: None,
                watts_override: None,
            });
        }
    }
    let plane = CalcPlane {
        origin: Vertex::new(0.0, 0.0, 0.8),
        width: w,
        depth: d,
        cols,
        rows,
    };
    (meshes, lums, profiles, plane)
}

#[test]
#[ignore = "benchmark — run with --release --ignored --nocapture"]
fn what_a_grid_point_costs() {
    println!("\n{:>12} {:>10} {:>12} {:>14}", "grid", "points", "elapsed", "per point");
    println!("{}", "-".repeat(52));
    let materials = default_materials();
    let settings = RaySettings::default();
    for (c, r) in [(64u32, 25u32), (64, 52), (132, 52), (200, 79), (264, 104)] {
        let (meshes, lums, profiles, plane) = gym(c, r);
        let t = Instant::now();
        let g = calc::calculate(&meshes, &lums, &profiles, &materials, &plane, &settings);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let n = (c as u64) * (r as u64);
        println!(
            "{:>5} x {:<4} {:>10} {:>9.1} ms {:>11.4} ms   avg {:.0} lx",
            c, r, n, ms, ms / n as f64, g.avg,
        );
    }
    println!();
}

/// A cosine downlight, 1000 cd on axis — the same stand-in the app's built-in profile uses, so
/// the timing is against a realistic photometric lookup rather than a constant.
fn downlight() -> IesProfile {
    let vertical_angles: Vec<f64> = (0..=18).map(|i| i as f64 * 5.0).collect();
    let candela: Vec<f64> =
        vertical_angles.iter().map(|g| 1000.0 * g.to_radians().cos().max(0.0)).collect();
    IesProfile {
        name: "builtin".into(),
        photometry: PhotometryType::C,
        lumens: -1.0,
        multiplier: 1.0,
        vertical_angles,
        horizontal_angles: vec![0.0],
        candela: vec![candela],
        watts: 0.0,
        width: 0.0,
        length: 0.0,
        height: 0.0,
        luminous_length: 0.0,
        luminous_width: 0.0,
    }
}

/// AND WHAT THE SCENE COSTS ON TOP. The room above is six quads; a real project carries furniture,
/// and the owner's gym scene capture showed roughly 6.5 million triangles of it. Every grid point
/// traces against all of it, so the per-point figure from a bare box is a FLOOR, not the number to
/// size a budget against.
#[test]
#[ignore = "benchmark — run with --release --ignored --nocapture"]
fn what_the_scene_costs_on_top_of_the_grid() {
    println!("\n{:>12} {:>12} {:>12} {:>14}", "scene tris", "points", "elapsed", "per point");
    println!("{}", "-".repeat(54));
    let materials = default_materials();
    let settings = RaySettings::default();
    for blocks in [0usize, 40, 200, 1000] {
        let (mut meshes, lums, profiles, plane) = gym(132, 52);
        // Furniture-shaped clutter: small boxes standing on the floor, each a closed mesh, spread
        // through the room so the BVH has real work to do rather than one hot cell.
        for i in 0..blocks {
            let (x, y) = ((i % 20) as f32 * 1.6 + 0.5, (i / 20) as f32 * 0.6 + 0.5);
            let mut b = extrude::box_room(0.6, 0.6, 0.8);
            for m in &mut b {
                for v in &mut m.vertices {
                    v.x += x;
                    v.y += y % 12.0;
                }
            }
            meshes.append(&mut b);
        }
        let tris: usize = meshes.iter().map(|m| m.triangles.len()).sum();
        let t = Instant::now();
        let _ = calc::calculate(&meshes, &lums, &profiles, &materials, &plane, &settings);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let n = 132u64 * 52;
        println!("{tris:>12} {n:>12} {ms:>9.1} ms {:>11.4} ms", ms / n as f64);
    }
    println!();
}
