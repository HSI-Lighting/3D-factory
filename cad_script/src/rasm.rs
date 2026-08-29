//! The `rasm` Python module — the script-facing surface (slices 2–3).
//!
//! Built ONCE per engine lifetime and injected into the interpreter's
//! persistent `__main__` globals (so the console behaves like a REPL). Every
//! function ships an op through the reverse `ScriptOp` queue and BLOCKS for
//! the host's reply with the GIL RELEASED (`Python::detach`): the host never
//! waits on Python, `Esc`/cancel can always acquire the GIL to interrupt,
//! and ops are strictly serialized with the host's own document mutations.
//!
//! Python only ever holds owned copies (D7) — replies are plain Rust values
//! converted to builtin types (int / str / bool / list / dict).

use std::cell::{Cell, RefCell};
use std::sync::mpsc::Sender;
use std::sync::{mpsc, Arc, Mutex};

use cad_kernel::geom::Geom;
use cad_kernel::math::Vec2;
use cad_kernel::{Arc as GeomArc, Circle, Ellipse, Line, Point as GeomPoint, PolyVertex, Polyline, Text};
use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyDict, PyList, PyModule};

use crate::op::{
    Entity, ParamType, ScriptMeta, ScriptOp, ScriptOpMsg, ScriptOpReply, ScriptOpReplyMsg,
    ScriptParamMeta, UnitsInfo, ViewInfo,
};

/// The worker-side half of the reverse op queue, installed per run. The
/// reply receiver is shared (Arc+Mutex) because the worker loop keeps it
/// across runs; the mutex is uncontended — one worker, one pending op at a
/// time.
pub struct OpCtx {
    tx: Sender<ScriptOpMsg>,
    rx: Arc<Mutex<mpsc::Receiver<ScriptOpReplyMsg>>>,
    next_id: u64,
}

impl OpCtx {
    pub fn new(tx: Sender<ScriptOpMsg>, rx: Arc<Mutex<mpsc::Receiver<ScriptOpReplyMsg>>>) -> Self {
        Self { tx, rx, next_id: 0 }
    }
}

thread_local! {
    static OP_CTX: RefCell<Option<OpCtx>> = const { RefCell::new(None) };
    /// True during a metadata pass (`request_meta`): every rasm call returns
    /// a dummy instead of touching the host, so top-level script code is
    /// harmless while `rasm.main(fn)` records the declaration.
    static META_MODE: Cell<bool> = const { Cell::new(false) };
}

/// Install the reverse-op context for the current run (worker thread only).
pub fn install_ctx(ctx: OpCtx) {
    OP_CTX.with(|c| *c.borrow_mut() = Some(ctx));
}

/// Clear the context after a run (worker thread only).
pub fn clear_ctx() {
    OP_CTX.with(|c| *c.borrow_mut() = None);
}

/// Turn the metadata pass on/off (worker thread). While on, the module's
/// `_META_ONLY` is set (so `rasm.main` records instead of drawing), the
/// spec slot resets, and `rasm.args`/`rasm.params` are emptied.
pub fn set_meta_mode(py: Python<'_>, rasm: &Bound<'_, PyAny>, on: bool) -> PyResult<()> {
    META_MODE.with(|m| m.set(on));
    rasm.setattr("_META_ONLY", on)?;
    if on {
        rasm.setattr("_meta_spec", py.None())?;
        rasm.setattr("params", PyDict::new(py))?;
        rasm.setattr("args", PyList::empty(py))?;
    }
    Ok(())
}

/// Read the spec `rasm.main` recorded during a metadata pass.
pub fn read_meta_spec(
    _py: Python<'_>,
    rasm: &Bound<'_, PyAny>,
) -> PyResult<Option<ScriptMeta>> {
    let spec = rasm.getattr("_meta_spec")?;
    if spec.is_none() {
        return Ok(None);
    }
    let list = spec.downcast::<PyList>()?;
    let mut params = Vec::new();
    for item in list.iter() {
        let d = item.downcast::<PyDict>()?;
        let name: String = match d.get_item("name")? {
            Some(v) => v.extract()?,
            None => return Err(pyo3::exceptions::PyRuntimeError::new_err("param without name")),
        };
        let ty: String = match d.get_item("type")? {
            Some(v) => v.extract()?,
            None => "str".into(),
        };
        let ptype = match ty.as_str() {
            "float" => ParamType::Float,
            "int" => ParamType::Int,
            "bool" => ParamType::Bool,
            "length" => ParamType::Length,
            "point" => ParamType::Point,
            "entity" => ParamType::Entity,
            "color" => ParamType::Color,
            "choice" => ParamType::Choice,
            "linetype" => ParamType::Linetype,
            "layer" => ParamType::Layer,
            "block" => ParamType::Block,
            "hatch_pattern" => ParamType::HatchPattern,
            "float_list" => ParamType::FloatList,
            "int_list" => ParamType::IntList,
            "str_list" => ParamType::StrList,
            "point_list" => ParamType::PointList,
            _ => ParamType::Str,
        };
        let default: String = match d.get_item("default")? {
            Some(v) => v.extract()?,
            None => String::new(),
        };
        let help: String = match d.get_item("help")? {
            Some(v) => v.extract()?,
            None => String::new(),
        };
        let choices: Vec<String> = match d.get_item("choices")? {
            Some(v) if !v.is_none() => v.extract()?,
            _ => Vec::new(),
        };
        let num = |k: &str| -> PyResult<Option<f64>> {
            match d.get_item(k)? {
                Some(v) if !v.is_none() => Ok(Some(v.extract()?)),
                _ => Ok(None),
            }
        };
        params.push(ScriptParamMeta {
            name,
            ptype,
            default,
            min: num("min")?,
            max: num("max")?,
            help,
            choices,
        });
    }
    Ok(Some(ScriptMeta {
        name: String::new(), // filled by the engine (file stem)
        params,
    }))
}

/// The metadata-pass answer for an op — same SHAPE as the real replies so
/// scripts keep running, but the host is never touched and nothing is drawn.
fn dummy_reply(op: &ScriptOp) -> ScriptOpReply {
    match op {
        ScriptOp::DocCount => ScriptOpReply::Count(0),
        ScriptOp::DocGet { .. } => ScriptOpReply::Error("parameter scan does not read the document".into()),
        ScriptOp::DocAll => ScriptOpReply::Entities(Vec::new()),
        ScriptOp::SelectionGet => ScriptOpReply::Indices(Vec::new()),
        ScriptOp::LayersGet => ScriptOpReply::Layers(Vec::new()),
        ScriptOp::LayerActive => ScriptOpReply::LayerActive(0),
        ScriptOp::BlocksGet => ScriptOpReply::Blocks(Vec::new()),
        ScriptOp::SysVarGet { .. } => ScriptOpReply::SysVar(None),
        ScriptOp::ViewGet => ScriptOpReply::View(ViewInfo { center: Vec2::ZERO, scale: 1.0 }),
        ScriptOp::SelectionSet { .. } => ScriptOpReply::Indices(Vec::new()),
        ScriptOp::AddLine { .. }
        | ScriptOp::AddCircle { .. }
        | ScriptOp::AddArc { .. }
        | ScriptOp::AddEllipse { .. }
        | ScriptOp::AddPolyline { .. }
        | ScriptOp::AddPoint { .. }
        | ScriptOp::AddText { .. }
        | ScriptOp::Delete { .. }
        | ScriptOp::LayerAdd { .. } => ScriptOpReply::Ok(0),
        ScriptOp::LayerSetActive { .. }
        | ScriptOp::LayerSet { .. }
        | ScriptOp::BlockCreate { .. }
        | ScriptOp::BlockInsert { .. }
        | ScriptOp::SysVarSet { .. }
        | ScriptOp::ViewSet { .. } => ScriptOpReply::OkUnit,
        ScriptOp::Command { .. } => ScriptOpReply::CommandOutput(Vec::new()),
        ScriptOp::Save { .. } | ScriptOp::Open { .. } => ScriptOpReply::CommandOutput(Vec::new()),
        ScriptOp::DocUnits => ScriptOpReply::Units(UnitsInfo { name: "mm".into(), scene_per_unit: 1.0 }),
        ScriptOp::DocBounds => ScriptOpReply::Bounds(None),
        ScriptOp::LayoutsGet => ScriptOpReply::Layouts(Vec::new()),
        ScriptOp::LinetypesGet => ScriptOpReply::Linetypes(Vec::new()),
        ScriptOp::HatchPatternsGet => ScriptOpReply::Patterns(Vec::new()),
        ScriptOp::ModifyMove { .. }
        | ScriptOp::ModifyCopy { .. }
        | ScriptOp::ModifyRotate { .. }
        | ScriptOp::ModifyScale { .. }
        | ScriptOp::ModifyMirror { .. }
        | ScriptOp::SetEntityVisible { .. } => ScriptOpReply::Ok(0),
        ScriptOp::SetEntityColor { .. }
        | ScriptOp::SetEntityLinetype { .. }
        | ScriptOp::SetEntityLayer { .. }
        | ScriptOp::SetEntityLineweight { .. }
        | ScriptOp::SetEntityGeom { .. }
        | ScriptOp::LayoutSetActive { .. }
        | ScriptOp::UndoGroup
        | ScriptOp::SetCurrentColor { .. }
        | ScriptOp::SetCurrentLinetype { .. }
        | ScriptOp::SetCurrentLineweight { .. }
        | ScriptOp::ZoomExtents => ScriptOpReply::OkUnit,
        ScriptOp::AddHatch { .. } => ScriptOpReply::Ok(0),
        ScriptOp::HatchAt { .. } => ScriptOpReply::Indices(Vec::new()),
    }
}

/// Send one op and block for the host's reply. GIL-FREE wait: the caller
/// runs this inside `Python::detach`, so the host can always acquire the
/// GIL (cancel / any other Python work) while we wait. Times out-polling
/// every 20 ms so an engine shutdown (channels dropped) is detected fast.
fn call_op(op: ScriptOp) -> Result<ScriptOpReply, String> {
    OP_CTX.with(|c| {
        let mut guard = c.borrow_mut();
        let ctx = guard.as_mut().ok_or("script engine not running")?;
        let id = ctx.next_id;
        ctx.next_id += 1;
        ctx.tx
            .send(ScriptOpMsg { id, op })
            .map_err(|e| format!("script host unreachable: {e}"))?;
        loop {
            let recv = ctx
                .rx
                .lock()
                .map_err(|_| "op queue poisoned".to_string())?
                .recv_timeout(std::time::Duration::from_millis(20));
            match recv {
                Ok(msg) if msg.id == id => return Ok(msg.reply),
                Ok(_) => continue, // late reply to a previous op — skip
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("script host disconnected (app closed?)".into())
                }
            }
        }
    })
}

/// Ship an op, releasing the GIL while waiting, then surface a pending
/// KeyboardInterrupt (Esc) before returning. Central chokepoint for every
/// function below. During a metadata pass the op is answered with a dummy —
/// the host is never touched and nothing is drawn.
fn round_trip(py: Python<'_>, op: ScriptOp) -> PyResult<ScriptOpReply> {
    if META_MODE.with(|m| m.get()) {
        return Ok(dummy_reply(&op));
    }
    let rep = py
        .allow_threads(|| call_op(op))
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e))?;
    py.check_signals()?;
    Ok(rep)
}

fn rep_error(msg: impl Into<String>) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(msg.into())
}

/// The op failed on the host (already surfaced to the user there — rule 10).
fn op_failed(e: String) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(e)
}

/// The host answered with a variant we did not expect for this call.
fn unexpected() -> PyErr {
    rep_error("unexpected reply from the script host")
}

// ─────────────────────────────────────────────────────────────────────────────
// doc surface (rasm.doc.*)
// ─────────────────────────────────────────────────────────────────────────────

/// Number of model-space entities in the document.
#[pyfunction(name = "count")]
fn doc_count(py: Python<'_>) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::DocCount)?;
    match rep {
        ScriptOpReply::Count(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// The entity at `index` as a dict of owned data (IndexError when out of range).
#[pyfunction(name = "get")]
fn doc_get(py: Python<'_>, index: i64) -> PyResult<Py<PyDict>> {
    if index < 0 {
        return Err(pyo3::exceptions::PyIndexError::new_err("index must be >= 0"));
    }
    let rep = round_trip(py, ScriptOp::DocGet { index: index as usize })?;
    match rep {
        ScriptOpReply::Entity(e) => Ok(entity_dict(py, &e).unbind()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Every model-space entity as a list of dicts (O(N) copies — explicit).
#[pyfunction(name = "entities")]
fn doc_entities(py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
    let rep = round_trip(py, ScriptOp::DocAll)?;
    match rep {
        ScriptOpReply::Entities(v) => {
            let out = v.iter()
                .map(|e| Python::with_gil(|py| entity_dict(py, e).unbind()))
                .collect();
            Ok(out)
        }
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// All layers as a list of dicts {id, name, visible, locked, frozen, plottable, color}.
#[pyfunction(name = "layers")]
fn doc_layers(py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
    let rep = round_trip(py, ScriptOp::LayersGet)?;
    match rep {
        ScriptOpReply::Layers(v) => {
            let out = v.iter()
                .map(|l| Python::with_gil(|py| {
                    let d = PyDict::new(py);
                    let _ = d.set_item("id", l.id);
                    let _ = d.set_item("name", &l.name);
                    let _ = d.set_item("visible", l.visible);
                    let _ = d.set_item("locked", l.locked);
                    let _ = d.set_item("frozen", l.frozen);
                    let _ = d.set_item("plottable", l.plottable);
                    let _ = d.set_item("color", &l.color);
                    d.unbind()
                }))
                .collect();
            Ok(out)
        }
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Id of the active (current) layer.
#[pyfunction(name = "active_layer")]
fn doc_active_layer(py: Python<'_>) -> PyResult<u32> {
    let rep = round_trip(py, ScriptOp::LayerActive)?;
    match rep {
        ScriptOpReply::LayerActive(id) => Ok(id),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Names of all block definitions.
#[pyfunction]
fn doc_blocks(py: Python<'_>) -> PyResult<Vec<String>> {
    let rep = round_trip(py, ScriptOp::BlocksGet)?;
    match rep {
        ScriptOpReply::Blocks(v) => Ok(v),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// The document's unit settings: {name, scene_per_unit}.
#[pyfunction(name = "units")]
fn doc_units(py: Python<'_>) -> PyResult<Py<PyDict>> {
    let rep = round_trip(py, ScriptOp::DocUnits)?;
    match rep {
        ScriptOpReply::Units(u) => {
            let d = PyDict::new(py);
            let _ = d.set_item("name", &u.name);
            let _ = d.set_item("scene_per_unit", u.scene_per_unit);
            Ok(d.unbind())
        }
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// The bbox of all model-space entities: {"min": (x, y), "max": (x, y)}
/// or None for an empty document.
#[pyfunction(name = "bounds")]
fn doc_bounds(py: Python<'_>) -> PyResult<Option<Py<PyDict>>> {
    let rep = round_trip(py, ScriptOp::DocBounds)?;
    match rep {
        ScriptOpReply::Bounds(None) => Ok(None),
        ScriptOpReply::Bounds(Some((min, max))) => {
            let d = PyDict::new(py);
            let _ = d.set_item("min", (min.x, min.y));
            let _ = d.set_item("max", (max.x, max.y));
            Ok(Some(d.unbind()))
        }
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// The paper-space layouts: list of {id, name, active}.
#[pyfunction(name = "layouts")]
fn doc_layouts(py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
    let rep = round_trip(py, ScriptOp::LayoutsGet)?;
    match rep {
        ScriptOpReply::Layouts(v) => Ok(v
            .iter()
            .map(|l| {
                let d = PyDict::new(py);
                let _ = d.set_item("id", l.id);
                let _ = d.set_item("name", &l.name);
                let _ = d.set_item("active", l.active);
                d.unbind()
            })
            .collect()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// The linetype catalog names.
#[pyfunction(name = "linetypes")]
fn doc_linetypes(py: Python<'_>) -> PyResult<Vec<String>> {
    let rep = round_trip(py, ScriptOp::LinetypesGet)?;
    match rep {
        ScriptOpReply::Linetypes(v) => Ok(v),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// selection
// ─────────────────────────────────────────────────────────────────────────────

/// The current selection as a list of entity indices.
#[pyfunction]
fn selection(py: Python<'_>) -> PyResult<Vec<usize>> {
    let rep = round_trip(py, ScriptOp::SelectionGet)?;
    match rep {
        ScriptOpReply::Indices(v) => Ok(v),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Replace the selection with the given entity indices (out-of-range dropped).
#[pyfunction]
fn set_selection(py: Python<'_>, indices: Vec<usize>) -> PyResult<Vec<usize>> {
    let rep = round_trip(py, ScriptOp::SelectionSet { indices })?;
    match rep {
        ScriptOpReply::Indices(v) => Ok(v),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// add_* — geometry creation. Each returns the new entity's index.
// ─────────────────────────────────────────────────────────────────────────────

#[pyfunction]
fn add_line(py: Python<'_>, a: (f64, f64), b: (f64, f64)) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::AddLine {
        a: Vec2::new(a.0, a.1), b: Vec2::new(b.0, b.1),
    })?;
    match rep {
        ScriptOpReply::Ok(i) => Ok(i),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

#[pyfunction]
fn add_circle(py: Python<'_>, center: (f64, f64), radius: f64) -> PyResult<usize> {
    if !(radius > 0.0) {
        return Err(pyo3::exceptions::PyValueError::new_err("radius must be > 0"));
    }
    let rep = round_trip(py, ScriptOp::AddCircle {
        center: Vec2::new(center.0, center.1), radius,
    })?;
    match rep {
        ScriptOpReply::Ok(i) => Ok(i),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

#[pyfunction]
fn add_arc(
    py: Python<'_>,
    center: (f64, f64),
    radius: f64,
    start_deg: f64,
    sweep_deg: f64,
) -> PyResult<usize> {
    if !(radius > 0.0) {
        return Err(pyo3::exceptions::PyValueError::new_err("radius must be > 0"));
    }
    let rep = round_trip(py, ScriptOp::AddArc {
        center: Vec2::new(center.0, center.1),
        radius, start_deg, sweep_deg,
    })?;
    match rep {
        ScriptOpReply::Ok(i) => Ok(i),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

#[pyfunction]
fn add_ellipse(
    py: Python<'_>,
    center: (f64, f64),
    major: (f64, f64),
    ratio: f64,
) -> PyResult<usize> {
    let m = Vec2::new(major.0, major.1);
    if m.len() <= 0.0 {
        return Err(pyo3::exceptions::PyValueError::new_err("major axis must be non-zero"));
    }
    let rep = round_trip(py, ScriptOp::AddEllipse {
        center: Vec2::new(center.0, center.1), major: m, ratio,
    })?;
    match rep {
        ScriptOpReply::Ok(i) => Ok(i),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

#[pyfunction(signature = (points, closed = false))]
fn add_polyline(
    py: Python<'_>,
    points: Vec<(f64, f64)>,
    closed: bool,
) -> PyResult<usize> {
    if points.len() < 2 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "a polyline needs at least 2 points",
        ));
    }
    let rep = round_trip(py, ScriptOp::AddPolyline {
        vertices: points.iter().map(|p| Vec2::new(p.0, p.1)).collect(),
        closed,
    })?;
    match rep {
        ScriptOpReply::Ok(i) => Ok(i),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

#[pyfunction]
fn add_point(py: Python<'_>, at: (f64, f64)) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::AddPoint { at: Vec2::new(at.0, at.1) })?;
    match rep {
        ScriptOpReply::Ok(i) => Ok(i),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

#[pyfunction(signature = (text, at, height = 2.5, angle_deg = 0.0))]
fn add_text(
    py: Python<'_>,
    text: String,
    at: (f64, f64),
    height: f64,
    angle_deg: f64,
) -> PyResult<usize> {
    if text.is_empty() {
        return Err(pyo3::exceptions::PyValueError::new_err("text cannot be empty"));
    }
    let rep = round_trip(py, ScriptOp::AddText {
        text, at: Vec2::new(at.0, at.1), height, angle_deg,
    })?;
    match rep {
        ScriptOpReply::Ok(i) => Ok(i),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Delete the entities at the given indices. Returns how many were removed.
#[pyfunction]
fn delete(py: Python<'_>, indices: Vec<usize>) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::Delete { indices })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// modify existing entities (P1)
// ─────────────────────────────────────────────────────────────────────────────

/// Move the entities at `indices` by (dx, dy) — in place, undoable.
#[pyfunction(name = "move")]
fn move_entities(py: Python<'_>, indices: Vec<usize>, dx: f64, dy: f64) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::ModifyMove {
        indices, delta: Vec2::new(dx, dy),
    })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Copy the entities at `indices` by (dx, dy); returns the NEW indices.
#[pyfunction(name = "copy")]
fn copy_entities(py: Python<'_>, indices: Vec<usize>, dx: f64, dy: f64) -> PyResult<Vec<usize>> {
    let rep = round_trip(py, ScriptOp::ModifyCopy {
        indices, delta: Vec2::new(dx, dy),
    })?;
    match rep {
        ScriptOpReply::Indices(v) => Ok(v),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Rotate the entities at `indices` around `center` by `angle_deg` (in place).
#[pyfunction(name = "rotate")]
fn rotate_entities(
    py: Python<'_>,
    indices: Vec<usize>,
    center: (f64, f64),
    angle_deg: f64,
) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::ModifyRotate {
        indices,
        pivot: Vec2::new(center.0, center.1),
        angle_deg,
    })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Scale the entities at `indices` by `factor` around `center` (in place).
#[pyfunction(name = "scale")]
fn scale_entities(
    py: Python<'_>,
    indices: Vec<usize>,
    center: (f64, f64),
    factor: f64,
) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::ModifyScale {
        indices,
        pivot: Vec2::new(center.0, center.1),
        factor,
    })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Mirror the entities at `indices` across the axis a→b (in place).
#[pyfunction(name = "mirror")]
fn mirror_entities(
    py: Python<'_>,
    indices: Vec<usize>,
    a: (f64, f64),
    b: (f64, f64),
) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::ModifyMirror {
        indices,
        a: Vec2::new(a.0, a.1),
        b: Vec2::new(b.0, b.1),
    })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Parse a color argument: None/"bylayer" → -1, "byblock" → -2, int 0..=255 → ACI.
fn parse_color_opt(v: Option<&Bound<'_, PyAny>>) -> PyResult<i32> {
    match v {
        None => Ok(-1),
        Some(o) => {
            if o.is_none() {
                return Ok(-1);
            }
            if let Ok(s) = o.extract::<String>() {
                return match s.to_ascii_lowercase().as_str() {
                    "bylayer" | "layer" => Ok(-1),
                    "byblock" | "block" => Ok(-2),
                    other => Err(pyo3::exceptions::PyValueError::new_err(format!(
                        "unknown color '{}' — use an int 0..=255, 'bylayer', 'byblock', or None",
                        other
                    ))),
                };
            }
            let n: i64 = o.extract()?;
            if !(0..=255).contains(&n) {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "ACI color must be 0..=255",
                ));
            }
            Ok(n as i32)
        }
    }
}

/// Set the color of the entities at `indices` (in place, undoable).
/// `color`: ACI int 0..=255, "bylayer"/"byblock", or None (= ByLayer).
#[pyfunction]
#[allow(clippy::needless_pass_by_value)]
fn set_color(
    py: Python<'_>,
    indices: Vec<usize>,
    color: Option<Bound<'_, PyAny>>,
) -> PyResult<usize> {
    let c = parse_color_opt(color.as_ref())?;
    let rep = round_trip(py, ScriptOp::SetEntityColor { indices, color: c })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Move the entities at `indices` onto the named layer.
#[pyfunction]
fn set_layer_of(py: Python<'_>, indices: Vec<usize>, name: String) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::SetEntityLayer { indices, name })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Set the linetype of the entities at `indices` (None/"" = ByLayer).
#[pyfunction]
fn set_linetype(
    py: Python<'_>,
    indices: Vec<usize>,
    name: Option<String>,
) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::SetEntityLinetype {
        indices,
        name: name.unwrap_or_default(),
    })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Set the lineweight of the entities at `indices` in mm (negative = ByLayer).
#[pyfunction]
fn set_lineweight(
    py: Python<'_>,
    indices: Vec<usize>,
    mm: Option<f64>,
) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::SetEntityLineweight {
        indices,
        mm: mm.unwrap_or(-1.0),
    })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Show / hide the entities at `indices`.
#[pyfunction]
fn set_visible(py: Python<'_>, indices: Vec<usize>, visible: bool) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::SetEntityVisible { indices, visible })?;
    match rep {
        ScriptOpReply::Ok(n) => Ok(n),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Replace entity `index`'s GEOMETRY (shape-specific properties) with the
/// given entity dict — take a snapshot from `rasm.doc.get(i)`, edit the
/// fields you need, and write it back. The style/layer stay unchanged.
#[pyfunction]
fn set_geom(py: Python<'_>, index: usize, entity: &Bound<'_, PyDict>) -> PyResult<()> {
    let geom = geom_from_dict(entity)?;
    let rep = round_trip(py, ScriptOp::SetEntityGeom { index, geom })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// One entity dict → geometry. Supports the editable types; everything else
/// fails loudly.
fn geom_from_dict(d: &Bound<'_, PyDict>) -> PyResult<Geom> {
    let ty: String = d
        .get_item("type")?
        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("entity dict needs a 'type'"))?
        .extract()?;
    let pt = |key: &str| -> PyResult<Vec2> {
        let (x, y): (f64, f64) = d
            .get_item(key)?
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!("missing '{key}'"))
            })?
            .extract()?;
        Ok(Vec2::new(x, y))
    };
    let f = |key: &str| -> PyResult<f64> {
        d.get_item(key)?
            .ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!("missing '{key}'"))
            })?
            .extract()
    };
    match ty.as_str() {
        "line" => Ok(Geom::Line(Line { a: pt("start")?, b: pt("end")? })),
        "circle" => Ok(Geom::Circle(Circle { center: pt("center")?, radius: f("radius")? })),
        "arc" => Ok(Geom::Arc(GeomArc {
            center: pt("center")?,
            radius: f("radius")?,
            start_angle: f("start_deg")?.to_radians(),
            sweep_angle: f("sweep_deg")?.to_radians(),
        })),
        "ellipse" => Ok(Geom::Ellipse(Ellipse {
            center: pt("center")?,
            major: pt("major")?,
            ratio: f("ratio")?,
        })),
        "polyline" => {
            let pts: Vec<(f64, f64)> = d
                .get_item("points")?
                .ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err("missing 'points'")
                })?
                .extract()?;
            if pts.len() < 2 {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "a polyline needs at least 2 points",
                ));
            }
            let closed: bool = d.get_item("closed")?.map_or(Ok(false), |v| v.extract())?;
            let bulges: Vec<f64> = d
                .get_item("bulges")?
                .map_or(Ok(Vec::new()), |v| v.extract())?;
            let vertices: Vec<PolyVertex> = pts
                .iter()
                .enumerate()
                .map(|(i, p)| PolyVertex {
                    pos: Vec2::new(p.0, p.1),
                    bulge: bulges.get(i).copied().unwrap_or(0.0),
                })
                .collect();
            Ok(Geom::Polyline(Polyline { vertices, closed, widths: Vec::new() }))
        }
        "point" => Ok(Geom::Point(GeomPoint { location: pt("at")?, style: 0, size: 0.0 })),
        "text" => {
            let s: String = d
                .get_item("text")?
                .ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err("missing 'text'")
                })?
                .extract()?;
            let mut t = Text::empty();
            t.position = pt("at")?;
            t.height = d.get_item("height")?.map_or(Ok(2.5), |v| v.extract())?;
            t.angle = d
                .get_item("angle_deg")?
                .map_or(Ok(0.0), |v| v.extract::<f64>())?
                .to_radians();
            t.text = s;
            Ok(Geom::Text(t))
        }
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "cannot replace a '{}' geometry — supported: line, circle, arc, ellipse, polyline, point, text",
            other
        ))),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// layers
// ─────────────────────────────────────────────────────────────────────────────

#[pyfunction(signature = (name, set_current = true))]
fn add_layer(py: Python<'_>, name: String, set_current: bool) -> PyResult<u32> {
    let rep = round_trip(py, ScriptOp::LayerAdd { name: name.clone() })?;
    match rep {
        ScriptOpReply::Ok(id) => {
            if set_current {
                let _ = round_trip(py, ScriptOp::LayerSetActive { name })?;
            }
            Ok(id as u32)
        }
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Make the named layer current.
#[pyfunction]
fn set_layer(py: Python<'_>, name: String) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::LayerSetActive { name })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

#[pyfunction(signature = (name, visible = None, locked = None, frozen = None, plottable = None, color = None))]
#[allow(clippy::too_many_arguments)]
fn layer_set(
    py: Python<'_>,
    name: String,
    visible: Option<bool>,
    locked: Option<bool>,
    frozen: Option<bool>,
    plottable: Option<bool>,
    color: Option<u8>,
) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::LayerSet {
        name, visible, locked, frozen, plottable, color_aci: color,
    })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// blocks
// ─────────────────────────────────────────────────────────────────────────────

/// Create a block definition from the CURRENT SELECTION and instance it at
/// `base` (the selection is consumed — mirrors the `block <name>` command).
#[pyfunction]
fn create_block(py: Python<'_>, name: String, base: (f64, f64)) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::BlockCreate {
        name, base: Vec2::new(base.0, base.1),
    })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

#[pyfunction(signature = (name, at, rotation_deg = 0.0))]
fn insert_block(
    py: Python<'_>,
    name: String,
    at: (f64, f64),
    rotation_deg: f64,
) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::BlockInsert {
        name,
        at: Vec2::new(at.0, at.1),
        rotation: rotation_deg.to_radians(),
    })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// commands / sysvars / view / files
// ─────────────────────────────────────────────────────────────────────────────

/// Run an interactive command line ("line 0,0 10,0", "circle 5,5 2", …).
/// Returns the transcript lines the command produced.
#[pyfunction]
fn command(py: Python<'_>, raw: String) -> PyResult<Vec<String>> {
    let rep = round_trip(py, ScriptOp::Command { raw })?;
    match rep {
        ScriptOpReply::CommandOutput(lines) => Ok(lines),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Read a sysvar (None when unknown/unset).
#[pyfunction]
fn sysvar(py: Python<'_>, name: String) -> PyResult<Option<String>> {
    let rep = round_trip(py, ScriptOp::SysVarGet { name })?;
    match rep {
        ScriptOpReply::SysVar(v) => Ok(v),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Set a sysvar (persisted like the SETVAR command).
#[pyfunction]
fn setvar(py: Python<'_>, name: String, value: String) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::SysVarSet { name, value })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// The main canvas view as {center: (x, y), scale: px_per_unit}.
#[pyfunction]
fn view(py: Python<'_>) -> PyResult<Py<PyDict>> {
    let rep = round_trip(py, ScriptOp::ViewGet)?;
    match rep {
        ScriptOpReply::View(v) => {
            let d = Python::with_gil(|py| {
                let d = PyDict::new(py);
                let _ = d.set_item("center", (v.center.x, v.center.y));
                let _ = d.set_item("scale", v.scale);
                d.unbind()
            });
            Ok(d)
        }
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Move / zoom the canvas: center is the world point to centre on;
/// scale is px-per-world-unit (None = pan only).
#[pyfunction]
fn set_view(py: Python<'_>, center: (f64, f64), scale: Option<f64>) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::ViewSet {
        center: Vec2::new(center.0, center.1), scale,
    })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Save the document (.rsm or .dxf by extension). Returns the transcript
/// lines (the "!" line surfaces a failed save — never silent, rule 10).
#[pyfunction]
fn save(py: Python<'_>, path: String) -> PyResult<Vec<String>> {
    let rep = round_trip(py, ScriptOp::Save { path })?;
    match rep {
        ScriptOpReply::CommandOutput(lines) => Ok(lines),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Open a document from disk (replaces the current one). Returns the
/// transcript lines (the "!" line surfaces a failed open).
#[pyfunction]
fn open(py: Python<'_>, path: String) -> PyResult<Vec<String>> {
    let rep = round_trip(py, ScriptOp::Open { path })?;
    match rep {
        ScriptOpReply::CommandOutput(lines) => Ok(lines),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// layouts / undo groups / current style / zoom (P2-P4)
// ─────────────────────────────────────────────────────────────────────────────

/// Switch to the named paper-space layout.
#[pyfunction]
fn set_layout(py: Python<'_>, name: String) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::LayoutSetActive { name })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Mark an undo-group boundary: everything since the previous boundary (or
/// the start of the run) becomes ONE undo unit when the run finishes.
#[pyfunction]
fn undo_group(py: Python<'_>) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::UndoGroup)?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Set the CURRENT color for NEW entities (the script's own adds).
/// Same argument forms as set_color.
#[pyfunction]
fn set_current_color(
    py: Python<'_>,
    color: Option<Bound<'_, PyAny>>,
) -> PyResult<()> {
    let c = parse_color_opt(color.as_ref())?;
    let rep = round_trip(py, ScriptOp::SetCurrentColor { color: c })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Set the CURRENT linetype for NEW entities (name from rasm.doc.linetypes()).
#[pyfunction]
fn set_current_linetype(py: Python<'_>, name: String) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::SetCurrentLinetype { name })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Set the CURRENT lineweight (mm) for NEW entities.
#[pyfunction]
fn set_current_lineweight(py: Python<'_>, mm: f64) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::SetCurrentLineweight { mm })?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Zoom to the extents of the whole drawing (like `zoom e`).
#[pyfunction]
fn zoom_extents(py: Python<'_>) -> PyResult<()> {
    let rep = round_trip(py, ScriptOp::ZoomExtents)?;
    match rep {
        ScriptOpReply::OkUnit => Ok(()),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// hatching
// ─────────────────────────────────────────────────────────────────────────────

/// The hatch-pattern catalog names ("SOLID", "ANSI31", "BRICK", …).
#[pyfunction]
fn hatch_patterns(py: Python<'_>) -> PyResult<Vec<String>> {
    let rep = round_trip(py, ScriptOp::HatchPatternsGet)?;
    match rep {
        ScriptOpReply::Patterns(v) => Ok(v),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Create a hatch from EXPLICIT boundary entity indices. Accepted
/// boundary kinds: closed polylines, circles, ellipses, closed splines
/// (other kinds are skipped loudly). `pattern` = "SOLID" or a catalog
/// name from `rasm.hatch_patterns()`. Returns the new hatch's index.
#[pyfunction]
fn add_hatch(py: Python<'_>, boundary_indices: Vec<usize>, pattern: String) -> PyResult<usize> {
    let rep = round_trip(py, ScriptOp::AddHatch {
        boundary_indices,
        pattern,
    })?;
    match rep {
        ScriptOpReply::Ok(i) => Ok(i),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

/// Trace the smallest CLOSED REGION containing the world point (the app's
/// pick-point primitive, islands included) and hatch it. Returns the
/// boundary entity indices, or an EMPTY list when no closed region
/// contains the point (a normal search outcome, not an error).
#[pyfunction]
fn hatch_at(py: Python<'_>, point: (f64, f64), pattern: String) -> PyResult<Vec<usize>> {
    let rep = round_trip(py, ScriptOp::HatchAt {
        point: Vec2::new(point.0, point.1),
        pattern,
    })?;
    match rep {
        ScriptOpReply::Indices(v) => Ok(v),
        ScriptOpReply::Error(e) => Err(op_failed(e)),
        _ => Err(unexpected()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// value conversion + module installation
// ─────────────────────────────────────────────────────────────────────────────

fn pt(p: Vec2) -> (f64, f64) { (p.x, p.y) }

/// One dobject → a Python dict of owned values. Geometry-specific keys sit
/// flat next to `handle` / `layer` / `type` so scripts can read them
/// without a second hop.
fn entity_dict<'a>(py: Python<'a>, e: &'a Entity) -> Bound<'a, PyDict> {
    let d = PyDict::new(py);
    let _ = d.set_item("handle", e.handle);
    let _ = d.set_item("layer", &e.layer);
    let _ = d.set_item("color", &e.color);
    let _ = d.set_item("linetype", &e.linetype);
    let _ = d.set_item("lineweight", e.lineweight);
    let _ = d.set_item("visible", e.visible);
    match &e.geom {
        Geom::Line(l) => {
            let _ = d.set_item("type", "line");
            let _ = d.set_item("start", pt(l.a));
            let _ = d.set_item("end", pt(l.b));
        }
        Geom::Xline(x) => {
            let _ = d.set_item("type", "xline");
            let _ = d.set_item("base", pt(x.base));
            let _ = d.set_item("dir", pt(x.dir));
        }
        Geom::Ray(r) => {
            let _ = d.set_item("type", "ray");
            let _ = d.set_item("base", pt(r.base));
            let _ = d.set_item("dir", pt(r.dir));
        }
        Geom::Donut(dn) => {
            let _ = d.set_item("type", "donut");
            let _ = d.set_item("center", pt(dn.center));
            let _ = d.set_item("inner_radius", dn.inner_radius);
            let _ = d.set_item("outer_radius", dn.outer_radius);
        }
        Geom::Wipeout(wo) => {
            let _ = d.set_item("type", "wipeout");
            let _ = d.set_item("pts", wo.pts.iter().map(|p| pt(*p)).collect::<Vec<_>>());
        }
        Geom::Region(rg) => {
            let _ = d.set_item("type", "region");
            let _ = d.set_item("loop_pts", rg.loop_pts.iter().map(|p| pt(*p)).collect::<Vec<_>>());
        }
        Geom::Xref(x) => {
            let _ = d.set_item("type", "xref");
            let _ = d.set_item("name", &x.name);
            let _ = d.set_item("path", &x.path);
            let _ = d.set_item("insert", pt(x.insert));
            let _ = d.set_item("scale", x.scale);
            let _ = d.set_item("rotation", x.rotation);
            let _ = d.set_item("children", x.cached.len() as i64);
        }
        Geom::Table(t) => {
            let _ = d.set_item("type", "table");
            let _ = d.set_item("insert", pt(t.insert));
            let _ = d.set_item("rows", t.n_rows as i64);
            let _ = d.set_item("cols", t.n_cols as i64);
            let _ = d.set_item("row_h", t.row_h);
            let _ = d.set_item("col_w", t.col_w);
        }
        Geom::Circle(c) => {
            let _ = d.set_item("type", "circle");
            let _ = d.set_item("center", pt(c.center));
            let _ = d.set_item("radius", c.radius);
        }
        Geom::Arc(a) => {
            let _ = d.set_item("type", "arc");
            let _ = d.set_item("center", pt(a.center));
            let _ = d.set_item("radius", a.radius);
            let _ = d.set_item("start_deg", a.start_angle.to_degrees());
            let _ = d.set_item("sweep_deg", a.sweep_angle.to_degrees());
        }
        Geom::Ellipse(el) => {
            let _ = d.set_item("type", "ellipse");
            let _ = d.set_item("center", pt(el.center));
            let _ = d.set_item("major", pt(el.major));
            let _ = d.set_item("ratio", el.ratio);
        }
        Geom::EllipseArc(ea) => {
            let _ = d.set_item("type", "ellipse_arc");
            let _ = d.set_item("center", pt(ea.ellipse.center));
            let _ = d.set_item("major", pt(ea.ellipse.major));
            let _ = d.set_item("ratio", ea.ellipse.ratio);
            let _ = d.set_item("start_param_deg", ea.start_param.to_degrees());
            let _ = d.set_item("sweep_param_deg", ea.sweep_param.to_degrees());
        }
        Geom::Point(p) => {
            let _ = d.set_item("type", "point");
            let _ = d.set_item("at", pt(p.location));
            let _ = d.set_item("pdmode", p.style);
            let _ = d.set_item("pdsize", p.size);
        }
        Geom::Polyline(p) => {
            let _ = d.set_item("type", "polyline");
            let pts: Vec<(f64, f64)> = p.vertices.iter().map(|v| pt(v.pos)).collect();
            let _ = d.set_item("points", pts);
            let _ = d.set_item("closed", p.closed);
            let bulges: Vec<f64> = p.vertices.iter().map(|v| v.bulge).collect();
            let _ = d.set_item("bulges", bulges);
            let _ = d.set_item("widths", p.widths.clone());
        }
        Geom::Wall(w) => {
            let _ = d.set_item("type", "wall");
            let _ = d.set_item("start", pt(w.start));
            let _ = d.set_item("end", pt(w.end));
            let _ = d.set_item("thickness", w.thickness);
        }
        Geom::Text(t) => {
            let _ = d.set_item("type", "text");
            let _ = d.set_item("text", &t.text);
            let _ = d.set_item("at", pt(t.position));
            let _ = d.set_item("height", t.height);
            let _ = d.set_item("angle_deg", t.angle.to_degrees());
        }
        Geom::Hatch(h) => {
            let _ = d.set_item("type", "hatch");
            let _ = d.set_item("boundary_loops", h.boundary_handles.len());
            let _ = d.set_item("pattern", format!("{:?}", h.pattern));
        }
        Geom::Spline(s) => {
            let _ = d.set_item("type", "spline");
            let _ = d.set_item("degree", s.degree);
            let pts: Vec<(f64, f64)> = s.control_points.iter().map(|p| pt(*p)).collect();
            let _ = d.set_item("control_points", pts);
        }
        Geom::Dimension(dim) => {
            let _ = d.set_item("type", "dimension");
            let _ = d.set_item("kind", format!("{:?}", dim.kind));
            let _ = d.set_item("value", dim.measured_value());
        }
        Geom::BlockRef(br) => {
            let _ = d.set_item("type", "blockref");
            let _ = d.set_item("block", br.block);
            let _ = d.set_item("at", pt(br.insert));
            let _ = d.set_item("scale", br.scale);
            let _ = d.set_item("rotation_deg", br.rotation.to_degrees());
        }
        Geom::Viewport(vp) => {
            let _ = d.set_item("type", "viewport");
            let _ = d.set_item("center", pt(vp.center));
            let _ = d.set_item("width", vp.width);
            let _ = d.set_item("height", vp.height);
        }
        Geom::Leader(l) => {
            let _ = d.set_item("type", "leader");
            let pts: Vec<(f64, f64)> = l.pts.iter().map(|p| pt(*p)).collect();
            let _ = d.set_item("points", pts);
            let _ = d.set_item("text", &l.label.text);
            let _ = d.set_item("height", l.label.height);
        }
        Geom::AttrDef(a) => {
            let _ = d.set_item("type", "attdef");
            let _ = d.set_item("tag", &a.tag);
            let _ = d.set_item("prompt", &a.prompt);
            let _ = d.set_item("default", &a.default);
            let _ = d.set_item("at", pt(a.position));
            let _ = d.set_item("height", a.height);
        }
        Geom::CenterMark(cm) => {
            let _ = d.set_item("type", "centermark");
            let _ = d.set_item("at", pt(cm.center));
            let _ = d.set_item("size", cm.size);
            let _ = d.set_item("rotation", cm.rotation);
        }
    }
    d
}

const RASM_DOC: &str = "\
rasm — RUST-AutoRASM scripting surface.

READ
    rasm.doc.count()            → number of entities
    rasm.doc.get(i)             → dict snapshot of entity i (geometry + style)
    rasm.doc.entities()         → list of every entity (dicts)
    rasm.doc.layers()           → list of layer dicts
    rasm.doc.active_layer()     → current layer id
    rasm.doc.blocks()           → block definition names
    rasm.doc.units()            → {name, scene_per_unit}
    rasm.doc.bounds()           → {min, max} bbox or None
    rasm.doc.layouts()          → list of {id, name, active}
    rasm.doc.linetypes()        → linetype catalog names
    rasm.selection()            → selected entity indices
    rasm.sysvar(name)           → sysvar value (or None)
    rasm.view()                 → {center: (x, y), scale: px_per_unit}

WRITE — create
    rasm.add_line(a, b)
    rasm.add_circle(center, radius)
    rasm.add_arc(center, radius, start_deg, sweep_deg)
    rasm.add_ellipse(center, major, ratio)
    rasm.add_polyline(points, closed=False)
    rasm.add_point(at)
    rasm.add_text(text, at, height=2.5, angle_deg=0.0)
    rasm.delete(indices)
    rasm.set_selection(indices)

WRITE — modify existing entities (shape-specific properties)
    rasm.move(indices, dx, dy)
    rasm.copy(indices, dx, dy)              → new indices
    rasm.rotate(indices, center, angle_deg)
    rasm.scale(indices, center, factor)
    rasm.mirror(indices, a, b)
    rasm.set_color(indices, color=None)     → int 0-255 / 'bylayer' / 'byblock'
    rasm.set_layer_of(indices, name)
    rasm.set_linetype(indices, name=None)
    rasm.set_lineweight(indices, mm=None)
    rasm.set_visible(indices, visible)
    rasm.set_geom(i, entity_dict)           → replace one entity's geometry

WRITE — document
    rasm.add_layer(name, set_current=True)
    rasm.set_layer(name)                    → active layer
    rasm.layer_set(name, visible=…, locked=…, frozen=…, plottable=…, color=…)
    rasm.create_block(name, base)           # consumes the selection
    rasm.insert_block(name, at, rotation_deg=0.0)
    rasm.set_layout(name)                   → switch paper-space layout
    rasm.command('line 0,0 10,0')           # any interactive command
    rasm.setvar(name, value)
    rasm.set_view(center, scale=None)
    rasm.zoom_extents()
    rasm.save(path) / rasm.open(path)
    rasm.undo_group()                       # next undo unit boundary
    rasm.hatch_patterns()                   # hatch pattern catalog
    rasm.hatch_at(point, pattern)           # trace + hatch the closed region
    rasm.add_hatch(boundary_indices, pattern)  # hatch explicit boundaries
    rasm.set_current_color(color=None)
    rasm.set_current_linetype(name)
    rasm.set_current_lineweight(mm)

One script run = one undo unit (Ctrl+Z reverts the whole run; undo_group()
splits a run into units). Esc cancels a running script. Points are (x, y)
tuples; angles in degrees.
Named scripts (`run <name> [k=v …]`) read their inputs from `rasm.params`
(named) or `rasm.args` (positional) and `sys.argv` (['<name>', *args]).
Declare typed inputs with `rasm.main(fn)` — the function's signature +
docstring ARE the declaration (types from defaults or string annotations:
float / int / bool / str / length / point / entity / color / choice /
float_list / int_list / str_list / point_list; `name: help (min..max)`
docstring lines give help text and ranges, `name: help [a, b, c]` declares
a dropdown choice). Lengths are edited
in the document's DISPLAY unit (suffixes accepted: `25`, `25cm`, `6'`) and
arrive as scene units. The app shows an input dialog (points/entities are
canvas-pickable, colors open the ACI wheel); `run <name> k=v …` skips it.
Example scripts live in the scripts/ folder (run with `pyfile scripts/<name>.py`
or `run <name>`).
";

/// The `rasm.main(fn)` runtime + its metadata-pass behavior, exec'd into
/// the module's own dict. Types come from the defaults (or annotations);
/// help/range come from docstring lines `name: help` / `name: help (min..max)`.
/// During a metadata pass (`_META_ONLY`) it RECORDS the signature instead of
/// calling the function. On a real run it converts `rasm.params` to the
/// declared types and calls `fn(**values)` — missing params use the
/// function's own defaults.
const RASM_MAIN_PY: &str = r#"
import inspect, re

_META_ONLY = False
_meta_spec = None

def main(fn):
    """Declare this script's inputs from fn's signature and run it.

    Types come from string annotations or the defaults:
    float / int / bool / str / length (a distance — the host converts it
    from the document's display unit to scene units before it arrives) /
    point (a (x, y) tuple) / entity (an existing entity's index — the user
    clicks it; -1 when unpicked) / color (an ACI number) / choice (a
    dropdown — declare the options as `name: help [a, b, c]` in the
    docstring) / float_list / int_list / str_list / point_list
    (comma-separated values; point_list uses `x,y; x,y; …`).
    Docstring lines of the form `name: help` and `name: help (min..max)`
    become the parameter help text and range.

        def run(outer_d=120.0, bolts=6, pos: 'point' = (0.0, 0.0),
                color: 'color' = 5, target: 'entity' = -1,
                pattern: 'choice' = 'ANSI31', ratios: 'float_list' = (1.0, 1.0)):
            '''outer_d: outer diameter (10..500)
            bolts: number of bolts (3..24)
            pattern: hatch pattern [SOLID, ANSI31, BRICK]
            '''
            ...
        rasm.main(run)
    """
    hints = {'float': 'float', 'int': 'int', 'bool': 'bool', 'str': 'str',
             'length': 'length', 'point': 'point', 'entity': 'entity',
             'color': 'color', 'choice': 'choice',
             'linetype': 'linetype', 'layer': 'layer', 'block': 'block',
             'hatch_pattern': 'hatch_pattern',
             'float_list': 'float_list', 'int_list': 'int_list',
             'str_list': 'str_list', 'point_list': 'point_list'}
    spec = []
    for p in inspect.signature(fn).parameters.values():
        t = 'str'
        if isinstance(p.annotation, str) and p.annotation in hints:
            t = p.annotation
        elif p.default is not None:
            t = {float: 'float', int: 'int', bool: 'bool', str: 'str',
                 tuple: 'point'}.get(type(p.default), 'str')
        default = ''
        if p.default is not inspect.Parameter.empty:
            # Plain str() — repr() would wrap STRING defaults in quotes
            # ("'ANSI31'"), which then fails dropdown/catalog validation.
            default = str(p.default)
        spec.append({'name': p.name, 'type': t, 'default': default,
                     'min': None, 'max': None, 'help': '', 'choices': []})
    if fn.__doc__:
        for line in fn.__doc__.splitlines():
            line = line.strip()
            if ':' not in line:
                continue
            n, _, rest = line.partition(':')
            n = n.strip()
            rest = rest.strip()
            m = re.match(r'^(.*)\(\s*(-?[0-9.]+)\s*\.\.\s*(-?[0-9.]+)\s*\)\s*$', rest)
            # `name: help [a, b, c]` → a choice dropdown with those options.
            cm = re.match(r'^(.*)\[(.*)\]\s*$', rest)
            for s in spec:
                if s['name'] != n:
                    continue
                if cm:
                    s['help'] = cm.group(1).strip()
                    s['choices'] = [c.strip() for c in cm.group(2).split(',')
                                    if c.strip()]
                elif m:
                    s['help'] = m.group(1).strip()
                    s['min'] = float(m.group(2))
                    s['max'] = float(m.group(3))
                else:
                    s['help'] = rest
    if _META_ONLY:
        global _meta_spec
        _meta_spec = spec
        return
    provided = dict(params)
    if not provided:
        # positional fallback: `run <name> v1 v2` maps to the declared order
        for i, s in enumerate(spec):
            if i < len(args):
                provided[s['name']] = args[i]
    values = {}
    for s in spec:
        v = provided.get(s['name'])
        if v is None:
            continue
        try:
            if s['type'] in ('float', 'length'):
                values[s['name']] = float(v)
            elif s['type'] == 'int':
                values[s['name']] = int(float(v))
            elif s['type'] == 'bool':
                values[s['name']] = str(v).strip().lower() in ('1', 'true', 'yes', 'on')
            elif s['type'] == 'color':
                values[s['name']] = int(float(v))
            elif s['type'] == 'entity':
                values[s['name']] = int(float(v)) if str(v).strip() else -1
            elif s['type'] == 'choice':
                v = str(v).strip()
                if s['choices'] and v not in s['choices']:
                    raise SystemExit('! %s: %r is not one of %s'
                                     % (s['name'], v, s['choices']))
                values[s['name']] = v
            elif s['type'] in ('float_list', 'int_list', 'str_list', 'point_list'):
                text = str(v).strip().strip('[]()')
                parts = [x.strip() for x in text.split(',') if x.strip()]
                if s['type'] == 'float_list':
                    values[s['name']] = [float(x) for x in parts]
                elif s['type'] == 'int_list':
                    values[s['name']] = [int(float(x)) for x in parts]
                elif s['type'] == 'point_list':
                    pts = []
                    for chunk in str(v).split(';'):
                        xy = chunk.strip().strip('()').split(',')
                        if len(xy) == 2:
                            pts.append((float(xy[0].strip()), float(xy[1].strip())))
                    values[s['name']] = pts
                else:
                    # Strip surrounding quotes — tuple defaults arrive as
                    # ('a', 'b'), so elements carry repr quotes.
                    values[s['name']] = [x.strip("'\"") for x in parts]
            elif s['type'] == 'point':
                parts = str(v).strip().strip('()').split(',')
                if len(parts) != 2:
                    raise ValueError
                values[s['name']] = (float(parts[0].strip()), float(parts[1].strip()))
            else:
                values[s['name']] = str(v)
        except (TypeError, ValueError):
            raise SystemExit('! %s: %r is not a valid %s' % (s['name'], v, s['type']))
    fn(**values)
"#;

/// Build the `rasm` module (with its `doc` submodule) and inject it into the
/// interpreter's persistent globals. Idempotent across submissions.
pub fn install_rasm(py: Python<'_>, globals: &Bound<'_, PyDict>) -> PyResult<()> {
    let m = PyModule::new(py, "rasm")?;
    let _ = m.add("__doc__", RASM_DOC);
    m.add_function(wrap_pyfunction!(add_line, &m)?)?;
    m.add_function(wrap_pyfunction!(add_circle, &m)?)?;
    m.add_function(wrap_pyfunction!(add_arc, &m)?)?;
    m.add_function(wrap_pyfunction!(add_ellipse, &m)?)?;
    m.add_function(wrap_pyfunction!(add_polyline, &m)?)?;
    m.add_function(wrap_pyfunction!(add_point, &m)?)?;
    m.add_function(wrap_pyfunction!(add_text, &m)?)?;
    m.add_function(wrap_pyfunction!(delete, &m)?)?;
    m.add_function(wrap_pyfunction!(move_entities, &m)?)?;
    m.add_function(wrap_pyfunction!(copy_entities, &m)?)?;
    m.add_function(wrap_pyfunction!(rotate_entities, &m)?)?;
    m.add_function(wrap_pyfunction!(scale_entities, &m)?)?;
    m.add_function(wrap_pyfunction!(mirror_entities, &m)?)?;
    m.add_function(wrap_pyfunction!(set_color, &m)?)?;
    m.add_function(wrap_pyfunction!(set_layer_of, &m)?)?;
    m.add_function(wrap_pyfunction!(set_linetype, &m)?)?;
    m.add_function(wrap_pyfunction!(set_lineweight, &m)?)?;
    m.add_function(wrap_pyfunction!(set_visible, &m)?)?;
    m.add_function(wrap_pyfunction!(set_geom, &m)?)?;
    m.add_function(wrap_pyfunction!(selection, &m)?)?;
    m.add_function(wrap_pyfunction!(set_selection, &m)?)?;
    m.add_function(wrap_pyfunction!(add_layer, &m)?)?;
    m.add_function(wrap_pyfunction!(set_layer, &m)?)?;
    m.add_function(wrap_pyfunction!(layer_set, &m)?)?;
    m.add_function(wrap_pyfunction!(create_block, &m)?)?;
    m.add_function(wrap_pyfunction!(insert_block, &m)?)?;
    m.add_function(wrap_pyfunction!(command, &m)?)?;
    m.add_function(wrap_pyfunction!(sysvar, &m)?)?;
    m.add_function(wrap_pyfunction!(setvar, &m)?)?;
    m.add_function(wrap_pyfunction!(view, &m)?)?;
    m.add_function(wrap_pyfunction!(set_view, &m)?)?;
    m.add_function(wrap_pyfunction!(save, &m)?)?;
    m.add_function(wrap_pyfunction!(open, &m)?)?;
    m.add_function(wrap_pyfunction!(set_layout, &m)?)?;
    m.add_function(wrap_pyfunction!(undo_group, &m)?)?;
    m.add_function(wrap_pyfunction!(set_current_color, &m)?)?;
    m.add_function(wrap_pyfunction!(set_current_linetype, &m)?)?;
    m.add_function(wrap_pyfunction!(set_current_lineweight, &m)?)?;
    m.add_function(wrap_pyfunction!(zoom_extents, &m)?)?;
    m.add_function(wrap_pyfunction!(hatch_patterns, &m)?)?;
    m.add_function(wrap_pyfunction!(add_hatch, &m)?)?;
    m.add_function(wrap_pyfunction!(hatch_at, &m)?)?;

    let doc = PyModule::new(py, "rasm.doc")?;
    let _ = doc.add("__doc__", "Read surface of the document (see help(rasm)).");
    doc.add_function(wrap_pyfunction!(doc_count, &doc)?)?;
    doc.add_function(wrap_pyfunction!(doc_get, &doc)?)?;
    doc.add_function(wrap_pyfunction!(doc_entities, &doc)?)?;
    doc.add_function(wrap_pyfunction!(doc_layers, &doc)?)?;
    doc.add_function(wrap_pyfunction!(doc_active_layer, &doc)?)?;
    doc.add_function(wrap_pyfunction!(doc_blocks, &doc)?)?;
    doc.add_function(wrap_pyfunction!(doc_units, &doc)?)?;
    doc.add_function(wrap_pyfunction!(doc_bounds, &doc)?)?;
    doc.add_function(wrap_pyfunction!(doc_layouts, &doc)?)?;
    doc.add_function(wrap_pyfunction!(doc_linetypes, &doc)?)?;
    m.add("doc", &doc)?;

    // The rasm.main() runtime + param plumbing live in the module's dict
    // (so `main` sees `_META_ONLY` / `params` as module globals).
    let c = std::ffi::CString::new(RASM_MAIN_PY)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    py.run(c.as_c_str(), Some(&m.dict()), None)?;

    globals.set_item("rasm", &m)?;
    Ok(())
}
