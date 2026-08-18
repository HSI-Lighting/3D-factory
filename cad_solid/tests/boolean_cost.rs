//! What a boolean actually costs — the measurement M6 exists to take.
//!
//! Every threshold in the plan downstream of here was estimated, and two of them were estimated
//! against a BSP that was collapsing because the tolerance was wrong. This measures the shapes the
//! app really builds instead: a window cut into a long traced wall, and a building with many
//! openings in it.
//!
//! IGNORED BY DEFAULT — it is a benchmark, not an assertion, and it runs for tens of seconds:
//!
//!     cargo test -p cad_solid --test boolean_cost -- --ignored --nocapture
//!
//! Run it once with `mesh-bbopt` OFF and once with it ON (one word in `cad_solid/Cargo.toml`) and
//! compare. The optimisation puts only the polygons that overlap the other mesh's bounding box
//! into the BSP and passes the rest through untouched, so a small cutter against a large wall
//! should build a tree from a handful of polygons rather than from the whole wall.

use cad_solid::{BoolOp, Model, Placement, Plane, Primitive};
use glam::Vec2;
use std::time::Instant;

/// A traced curved wall as ONE extrusion: a closed band of `n` points per side, which is what a
/// DXF-traced facade actually arrives as. `n` drives the polygon count, and therefore the BSP.
fn curved_wall_profile(n: usize, radius: f32, thickness: f32) -> Vec<Vec2> {
    let (ro, ri) = (radius + thickness * 0.5, radius - thickness * 0.5);
    let sweep = std::f32::consts::PI * 0.8;
    let mut pts = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = sweep * i as f32 / (n - 1) as f32;
        pts.push(Vec2::new(ro * t.cos(), ro * t.sin()));
    }
    for i in (0..n).rev() {
        let t = sweep * i as f32 / (n - 1) as f32;
        pts.push(Vec2::new(ri * t.cos(), ri * t.sin()));
    }
    pts
}

fn wall_model(n: usize) -> (Model, f32) {
    let mut m = Model::default();
    let pts = curved_wall_profile(n, 12.0, 0.3);
    let (profile, centre, w, d) = m.add_profile(&pts).expect("wall profile");
    m.push(
        BoolOp::Union,
        Plane::default(),
        Placement { u: centre.x, v: centre.y, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
        Primitive::Extrusion { profile, h: 3.0, w, d },
    );
    (m, 12.0)
}

/// One window-sized cutter on the wall's ring, at angle `t` along the sweep.
fn add_window(m: &mut Model, radius: f32, t: f32) {
    let (c, s) = (t.cos(), t.sin());
    m.push(
        BoolOp::Difference,
        Plane::default(),
        Placement {
            u: radius * c, v: radius * s, lift: 1.0,
            spin_deg: s.atan2(c).to_degrees(), pitch_deg: 0.0, roll_deg: 0.0,
        },
        Primitive::Box { w: 1.2, d: 1.0, h: 1.4 },
    );
}

fn report(label: &str, m: &Model) {
    let t = Instant::now();
    let mesh = m.eval();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{label:<44} {ms:>9.1} ms   {:>8} tris   {:>4} features",
        mesh.tri_count(),
        m.features.len(),
    );
}

#[test]
#[ignore = "benchmark — run with --ignored --nocapture"]
fn what_a_boolean_costs() {
    println!(
        "\nmesh-bbopt: {}\n",
        if cfg!(feature = "bbopt") { "ON (measured)" } else { "OFF" },
    );
    println!("{:<44} {:>12}   {:>8}   {:>4}", "case", "eval", "tris", "feat");
    println!("{}", "-".repeat(78));

    // THE SHAPE THE PLAN NAMES: one small cutter against a large wall. The wall's polygon count
    // is what the BSP is built from today; almost none of it can be touched by the cutter.
    for n in [24usize, 96, 240] {
        let (m, r) = wall_model(n);
        report(&format!("wall {n:>4} pts, no cut"), &m);

        let (mut m, r2) = (m, r);
        add_window(&mut m, r2, 0.4);
        report(&format!("wall {n:>4} pts, 1 window"), &m);
    }

    // A REAL FACADE: many openings in the one wall, which is where the cost actually lands —
    // every cut re-evaluates the whole accumulated body.
    for cuts in [4usize, 12, 24] {
        let (mut m, r) = wall_model(96);
        for i in 0..cuts {
            add_window(&mut m, r, 0.15 + 2.2 * i as f32 / cuts as f32);
        }
        report(&format!("wall  96 pts, {cuts:>2} windows"), &m);
    }
    println!();
}

/// THE SPEED IS WORTHLESS IF THE SOLID CHANGES. Run in BOTH configurations — this is not
/// `#[ignore]`d — so the shape is pinned by the same assertions either way.
///
/// Asserted on things a tessellation cannot alter. The bounding-box optimisation legitimately
/// produces FEWER triangles for the same solid (polygons it can prove are untouched are passed
/// through whole instead of being split by a BSP that had no business splitting them), so a
/// triangle count would be the wrong thing to compare and would fail for a good reason.
#[test]
fn the_bounding_box_optimisation_does_not_change_the_solid() {
    let (mut m, r) = wall_model(96);
    let plain = m.eval();
    let (pmn, pmx) = plain.bounds().expect("the wall has bounds");

    add_window(&mut m, r, 0.4);
    let cut = m.eval();
    let (cmn, cmx) = cut.bounds().expect("the cut wall has bounds");

    // 1. THE WALL IS STILL THE SAME SIZE. A cutter entirely inside the wall's footprint must not
    //    move its extents — if the partition ever dropped a polygon it could not prove safe,
    //    this is where a missing face would show.
    for k in 0..3 {
        assert!(
            (cmn[k] - pmn[k]).abs() < 1e-3 && (cmx[k] - pmx[k]).abs() < 1e-3,
            "axis {k}: the cut changed the wall's extents, {:?}..{:?} -> {:?}..{:?}",
            pmn, pmx, cmn, cmx,
        );
    }

    // 2. THE HOLE IS ACTUALLY THERE. Fire a ray straight through where the window was cut and
    //    count surface crossings: an uncut wall gives 2 (in and out), a wall with a hole in it
    //    gives more, because the ray meets the reveal faces the cut created.
    let dir = glam::Vec3::new(-(0.4_f32).cos(), -(0.4_f32).sin(), 0.0);
    let at = glam::Vec3::new(20.0 * (0.4_f32).cos(), 20.0 * (0.4_f32).sin(), 1.7);
    let crossings = |mesh: &cad_solid::SolidMesh| {
        mesh.positions.chunks_exact(3).filter(|c| {
            cad_solid::ray_triangle(
                at, dir,
                glam::Vec3::from(c[0]), glam::Vec3::from(c[1]), glam::Vec3::from(c[2]),
            ).is_some()
        }).count()
    };
    let (before, after) = (crossings(&plain), crossings(&cut));
    assert!(before > 0, "the probe ray must hit the uncut wall, or it proves nothing");
    assert_ne!(after, before, "the window cut left the wall unchanged along the probe");

    // 3. AND THE SOLID IS STILL CLOSED. Every triangle count is a multiple of one triangle;
    //    what matters is that the cut produced MORE surface, not less — a hole adds reveals.
    assert!(
        cut.tri_count() > plain.tri_count(),
        "cutting a window removed surface instead of adding reveals: {} -> {}",
        plain.tri_count(), cut.tri_count(),
    );
}
