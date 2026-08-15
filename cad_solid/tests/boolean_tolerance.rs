//! The boolean tolerance, in its own test BINARY.
//!
//! It has to be its own binary. csgrs keeps the tolerance in a `OnceLock` whose first *reader*
//! initialises it to the default, so a test that shares a process with any other boolean test
//! cannot control which value is in force. One process, one boolean, one tolerance.

use cad_solid::{BoolOp, Model, Plane, Primitive};

/// Signed volume by the divergence theorem over the triangle soup. Correct for a closed mesh and
/// meaningless for an open one — which is the point: a collapsed BSP returns garbage here, loudly.
fn volume(m: &cad_solid::SolidMesh) -> f64 {
    let p = &m.positions;
    let mut v = 0.0f64;
    for t in p.chunks_exact(3) {
        let (a, b, c) = (t[0], t[1], t[2]);
        v += (a[0] as f64) * ((b[1] as f64) * (c[2] as f64) - (b[2] as f64) * (c[1] as f64))
           - (a[1] as f64) * ((b[0] as f64) * (c[2] as f64) - (b[2] as f64) * (c[0] as f64))
           + (a[2] as f64) * ((b[0] as f64) * (c[1] as f64) - (b[1] as f64) * (c[0] as f64));
    }
    (v / 6.0).abs()
}

/// The read-back that makes the setter trustworthy. csgrs's `set_tolerance` is
/// `let _ = CELL.set(v)` — it swallows its own failure — so "we called it" proves nothing.
#[test]
fn the_tolerance_is_actually_in_force_after_init() {
    let got = cad_solid::init_boolean_tolerance().expect("nothing may run a boolean before init");
    assert_eq!(got, cad_solid::BOOLEAN_TOLERANCE);

    // Idempotent: a second call must agree rather than report a drift.
    let again = cad_solid::init_boolean_tolerance().expect("second call still agrees");
    assert_eq!(again, cad_solid::BOOLEAN_TOLERANCE);
}

/// THE MEASUREMENT THAT MATTERS. A 100 mm plate cut by a hole.
///
/// The coplanarity test compares an exact orientation predicate that returns a *volume* against a
/// fixed threshold, so the band in DISTANCE widens as triangles get small — and this app works in
/// metres while the crate is dimensioned for millimetres.
///
/// CONFIRMED BOTH WAYS at this size: at the csgrs default of 1e-6 the result is **100 % wrong** —
/// an empty solid, because the BSP collapses and the difference eats the whole plate — and at
/// `BOOLEAN_TOLERANCE` it is exact. No error, no panic, no open-edge signal either way.
///
/// ONE HONEST CORRECTION to the figures that motivated this. The −33 %/+732 % reported for a
/// 100 mm plate was measured against csgrs DIRECTLY. Driven through `Model::eval` — placement,
/// plane basis and all — a 100 mm plate still comes out correct at the default, and the failure
/// appears an order of magnitude further down. The app's own path is more forgiving than the raw
/// numbers implied. The defect is real, reachable and silent; it bites at joinery scale rather
/// than at plinth scale. Do not quote the raw-csgrs figures as if they were app figures.
#[test]
fn a_small_plate_cut_by_a_hole_has_the_right_volume() {
    cad_solid::init_boolean_tolerance().expect("init");

    const S: f32 = 0.01;     // 10 mm plate
    const T: f32 = 0.001;    // 1 mm thick
    const R: f32 = S / 6.0;  // hole radius

    let mut m = Model::default();
    m.push(
        BoolOp::Union,
        Plane::default(),
        cad_solid::Placement::default(),
        Primitive::Box { w: S, d: S, h: T },
    );
    m.push(
        BoolOp::Difference,
        Plane::default(),
        cad_solid::Placement::default(),
        Primitive::Cylinder { r: R, h: T * 4.0, sides: 64 },
    );

    let mesh = m.eval();
    let got = volume(&mesh);
    let want = (S as f64) * (S as f64) * (T as f64)
        - std::f64::consts::PI * (R as f64).powi(2) * (T as f64);

    let err = (got - want).abs() / want * 100.0;
    assert!(
        err < 2.0,
        "volume error {err:.2}% (got {got:.9} m³, want {want:.9} m³) — \
         the BSP has collapsed and the boolean is returning the wrong solid",
    );
}
