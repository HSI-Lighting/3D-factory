//! cad_param — an INDEPENDENT 2D geometric constraint solver for RUST_CAD.
//!
//! This crate is the "parametric" half of the app. It is deliberately ISOLATED:
//!
//! - It has its OWN data structure (`Sketch` — points + lines + constraints),
//!   NOT the core `cad_kernel::Document`. It only borrows `Vec2` for math.
//! - It has its OWN file format (`.rsmp`, see [`io`]). The core RSM/DXF readers
//!   and the kernel `Document`/`Geom` types are never touched.
//! - The core never depends on `cad_param`, so the core can keep merging from the
//!   upstream repo cleanly while this evolves separately. A future 3D module
//!   layers in the same isolated way.
//!
//! The solver is a damped least-squares (Levenberg–Marquardt) Newton iteration
//! over the free point coordinates, with a numerical Jacobian and a self-contained
//! dense linear solver (no external deps — pure Rust, like the rest of the core).
//! Under-constrained sketches still solve (LM stays near the current guess);
//! over/well-constrained ones converge to residual ≈ 0.

pub mod expr;
pub mod io;
pub mod model;
pub mod solve;

pub use expr::{eval, Var, VarTable};
pub use io::{read_rsmp, write_rsmp};
pub use model::{Circle, CircleId, Constraint, Line, LineId, PointId, ScalarId, Sketch};
pub use solve::{current_rms, dof_analysis, residual_breakdown, residuals, solve, DofReport, SolveReport};
