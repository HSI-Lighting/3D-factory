//! Integration tests for the architectural generators — every generated solid must have a
//! measurable bounding box and a positive enclosed volume, and the U-shape stair must carry the
//! right split of steps.

use cad_solid::architecture::{
    build_ramp, build_spiral, build_stairs, mesh_volume, plan_spiral, plan_stairs, ArchError,
    RampParams, SpiralParams, StairLayout, StairParams,
};

/// Every generator produces a non-empty solid with positive extents in all three axes and V > 0.
fn assert_solid(m: &cad_solid::SolidMesh) {
    assert!(m.tri_count() > 0, "mesh has triangles");
    let (mn, mx) = m.bounds().expect("mesh has a bounding box");
    for k in 0..3 {
        assert!(mx[k] - mn[k] > 1e-4, "axis {k} has measurable extent ({} .. {})", mn[k], mx[k]);
    }
    assert!(mesh_volume(m) > 0.0, "solid has positive volume");
}

#[test]
fn straight_staircase_is_a_valid_solid() {
    let p = StairParams { layout: StairLayout::Straight, ..Default::default() };
    assert_solid(&build_stairs(&p).unwrap());
}

#[test]
fn ushape_staircase_has_correct_step_split_and_is_solid() {
    let p = StairParams {
        layout: StairLayout::UShape,
        total_height: 3.6,
        desired_riser_height: 0.18,
        step_width: 1.0,
        landing_depth: 1.4,
        split_ratio: 0.5,
        has_handrails: false, // exact top-height check below wants tread geometry only
        ..Default::default()
    };
    let plan = plan_stairs(&p).unwrap();
    // ceil(3.6 / 0.18) = 20 steps → 10 + 10.
    assert_eq!(plan.num_steps, 20);
    assert_eq!(plan.flight1_steps + plan.flight2_steps, plan.num_steps);
    assert_eq!(plan.flight1_steps, 10);
    assert_eq!(plan.flight2_steps, 10);
    let m = build_stairs(&p).unwrap();
    assert_solid(&m);
    let (_mn, mx) = m.bounds().unwrap();
    assert!((mx[2] - 3.6).abs() < 1e-3, "reaches the floor-to-floor height");
}

#[test]
fn ushape_uneven_split_ratio() {
    let p = StairParams {
        layout: StairLayout::UShape,
        total_height: 3.0,
        desired_riser_height: 0.15, // 20 steps
        step_width: 1.0,
        landing_depth: 1.2,
        split_ratio: 0.7,
        ..Default::default()
    };
    let plan = plan_stairs(&p).unwrap();
    assert_eq!(plan.num_steps, 20);
    // ceil(20 * 0.7) = 14 first, 6 second.
    assert_eq!(plan.flight1_steps, 14);
    assert_eq!(plan.flight2_steps, 6);
}

#[test]
fn landing_shorter_than_width_is_rejected() {
    let p = StairParams {
        layout: StairLayout::UShape,
        step_width: 1.2,
        landing_depth: 0.9,
        ..Default::default()
    };
    assert!(matches!(plan_stairs(&p), Err(ArchError::LandingTooShort { .. })));
}

#[test]
fn spiral_staircase_is_a_valid_solid() {
    let p = SpiralParams { steps_per_turn: 14, total_turns: 2.0, ..Default::default() };
    let plan = plan_spiral(&p).unwrap();
    assert_eq!(plan.num_steps, 28);
    assert_solid(&build_spiral(&p).unwrap());
}

#[test]
fn ramp_is_a_valid_solid() {
    let p = RampParams { vertical_height: 1.2, horizontal_length: 5.0, width: 1.5, thickness: 0.15 };
    assert_solid(&build_ramp(&p).unwrap());
}

#[test]
fn invalid_inputs_are_rejected() {
    let bad = StairParams { total_height: 0.0, ..Default::default() };
    assert!(matches!(plan_stairs(&bad), Err(ArchError::NonPositive(_))));
    let bad_ramp = RampParams { width: 0.0, ..Default::default() };
    assert!(build_ramp(&bad_ramp).is_err());
}
