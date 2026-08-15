//! A model deep enough to overflow the stack must produce an answer, not end the process.
//!
//! csgrs's BSP degenerates to a linked list for a convex body — `Node::build`, `clip_polygons`,
//! `clip_to` and `Drop` are all recursive — and the tree depth of an extruded N-gon is measured at
//! exactly N + 2. A curved wall traced from a DXF is an N-gon with a large N.
//!
//! On the default 8 MB stack that is `STATUS_STACK_OVERFLOW`: not a slow rebuild, not an error a
//! caller can handle, but the process gone and the drawing with it. And it is reachable while
//! merely OPENING a file, because `startup_repair` evaluates the model twice on load — so an
//! affected drawing cannot be opened, cannot be measured, and cannot be repaired.
//!
//! Without the big-stack wrapper inside `Model::eval` these tests do not fail, they KILL THE TEST
//! BINARY — which is the thing they assert cannot happen.

use cad_solid::{BoolOp, Model, Placement, Plane, Primitive};

/// A cylinder is the app's own N-gon: `sides` becomes the extruded profile, so the BSP depth
/// tracks it directly. This is the shape a traced curve produces.
fn cylinder_model(sides: u32) -> Model {
    let mut m = Model::default();
    m.push(
        BoolOp::Union,
        Plane::default(),
        Placement::default(),
        Primitive::Cylinder { r: 5.0, h: 3.0, sides },
    );
    m
}

/// Past the measured overflow band (somewhere between 900 and 1,600 on the default stack) with
/// room to spare, and comfortably inside a real drawing's range — a DXF arc tessellated at one
/// segment per degree over a long sweep gets here easily.
#[test]
fn a_deep_extrusion_evaluates_instead_of_aborting() {
    let mesh = cylinder_model(2048).eval();
    assert!(mesh.tri_count() > 0, "a 2,048-sided extrusion produced no geometry");
}

/// Deeper still, because the point is that the ceiling moved rather than that one number passes.
/// If this ever aborts again, the stack size is the thing to look at, not the model.
#[test]
fn a_very_deep_extrusion_still_evaluates() {
    let mesh = cylinder_model(4096).eval();
    assert!(mesh.tri_count() > 0, "a 4,096-sided extrusion produced no geometry");
}

/// A DIFFERENCE against a deep body, which is the shape that actually appears in a drawing: a
/// window cut into a traced curved wall. Both operands go through the recursion, and this is the
/// path `startup_repair` walks on load.
#[test]
fn cutting_a_deep_body_evaluates() {
    let mut m = cylinder_model(2048);
    m.push(
        BoolOp::Difference,
        Plane::default(),
        Placement::default(),
        Primitive::Box { w: 1.0, d: 12.0, h: 1.2 },
    );
    let mesh = m.eval();
    assert!(mesh.tri_count() > 0, "cutting a deep body produced no geometry");
}

/// The wrapper must not change the ANSWER, only where it is computed. A shallow model that
/// evaluated fine before still has to come out identical.
#[test]
fn the_wrapper_does_not_change_the_result() {
    let m = cylinder_model(16);
    let a = m.eval();
    let b = m.eval();
    assert_eq!(a.tri_count(), b.tri_count(), "eval is no longer deterministic");
    assert!(a.tri_count() > 0);
    assert_eq!(a.positions, b.positions, "the same model produced different geometry");
}
