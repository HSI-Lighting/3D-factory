//! cad_script — embedded Python scripting engine for RUST-AutoRASM.
//!
//! This is the ONLY crate that depends on pyo3 (D2): `cad_app` talks to the
//! plain-Rust facade [`ScriptEngine`] and never sees a Python type. The kernel
//! stays zero-dependency; only three parser `Command` variants there refer to
//! scripting (by name), with all execution here.
//!
//! Design + rationale: `coding agent md/PYTHON_SCRIPTING.md`.
//!
//! Runtime (mentor addendum): pyo3 `abi3-py311` stable ABI (works on CPython
//! 3.11+, not version-locked); the interpreter runs on ONE worker thread with
//! the GIL held there via `Python::with_gil` (standard GIL CPython, not the
//! free-threaded build). One script run = one undo unit (the app snapshots
//! once at start; a failure leaves the snapshot pushed → Ctrl+Z reverts the run).
//!
//! Slice 1: the engine plumbing — run text / files, stream output +
//! tracebacks, cancel. Slices 2–3: the reverse `ScriptOp` queue (`op`) and
//! the `rasm` Python module (read/write surface) over it. Slice 4 (docked
//! REPL console) lives in the app.

mod engine;
pub mod op;
pub mod rasm;

pub use engine::{ScriptEngine, ScriptReply};
pub use op::{
    Entity, LayerInfo, LayoutInfo, ParamType, ScriptMeta, ScriptOp, ScriptOpMsg, ScriptOpReply,
    ScriptOpReplyMsg, ScriptParamMeta, UnitsInfo, ViewInfo,
};
