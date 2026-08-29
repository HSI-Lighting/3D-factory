//! Script engine — a dedicated worker thread running embedded CPython, driven
//! by a plain-Rust facade (`cad_app` never sees a pyo3 type — D2).
//!
//! Threading (D3 + mentor item 3): Python runs on ONE worker thread; the GIL is
//! held only there, only inside `Python::with_gil`. The app submits jobs and
//! drains replies once per frame — never blocking the UI thread.
//!
//! Slice 1 scope: run a text expression / a `.py` file, stream captured
//! stdout/stderr + the value/traceback back as `ScriptReply`s, and cancel a
//! running script (`Esc`/Stop) cooperatively. The reverse `ScriptOp` queue that
//! lets scripts read/mutate the document is slice 2/3 and is intentionally NOT
//! present yet (AGENTS rule 3 — no dead scaffolding).

use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyDict, PyList, PyModule};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use crate::op::{ScriptOpMsg, ScriptOpReply, ScriptOpReplyMsg};
use crate::rasm::OpCtx;

/// A unit of work sent app → worker.
enum ScriptJob {
    /// Run a command-line expression/statement (REPL semantics: an expression
    /// echoes its value; anything else executes).
    RunText(String),
    /// Run a `.py` file's full source (exec semantics — no value echo).
    RunFile(std::path::PathBuf),
    /// Run a named script from the scripts folder: exec semantics + the
    /// argument vector is exposed to the script as `rasm.args` (args only)
    /// and `sys.argv` (`[name, args…]`). `params` are the NAMED inputs
    /// (`rasm.params` dict of strings — a script's `rasm.main(fn)` converts
    /// them to the declared types). `preview` marks a ghost pass: the app
    /// routes every op to a shadow document and renders the net additions
    /// as a dashed overlay instead of committing anything.
    RunScript {
        path: std::path::PathBuf,
        name: String,
        args: Vec<String>,
        params: Vec<(String, String)>,
    },
    /// Read a script's parameter declaration (see `request_meta`). The file
    /// runs with every rasm op no-op'd and `rasm.main(fn)` records its
    /// signature instead of drawing.
    Meta { path: std::path::PathBuf },
    /// Terminate the worker (engine drop).
    Shutdown,
}

/// A message worker → app, drained by `poll()` each frame.
#[derive(Clone, Debug)]
pub enum ScriptReply {
    /// A chunk of captured stdout/stderr.
    Print(String),
    /// The `repr()` of an evaluated expression's value (command-line `py`).
    Value(String),
    /// A formatted Python traceback (the run raised). Never silent (rule 10).
    Error(String),
    /// End of a run. `ok=false` iff it raised (incl. a cancel KeyboardInterrupt).
    Finished { ok: bool },
    /// The parameter declaration of a named script (answer to
    /// `request_meta`). `None` = the script declares nothing (it doesn't
    /// call `rasm.main`).
    Meta(Option<crate::op::ScriptMeta>),
}

/// The public facade. `cad_app` holds one of these and talks only in owned
/// Rust values.
pub struct ScriptEngine {
    job_tx: Sender<ScriptJob>,
    reply_rx: Receiver<ScriptReply>,
    /// Reverse op queue (slices 2–3): the worker sends `ScriptOpMsg`s here
    /// and the host drains them via `drain_ops()`, replying with
    /// `reply_op()`. While a script waits on a reply it releases the GIL,
    /// so the host never blocks on Python.
    op_rx: Receiver<ScriptOpMsg>,
    op_reply_tx: Sender<ScriptOpReplyMsg>,
    /// True while a job is executing (gates per-frame work — rule 13).
    busy: Arc<AtomicBool>,
    /// The worker's Python thread ident, published at run start, so `cancel()`
    /// can raise KeyboardInterrupt into it via `PyThreadState_SetAsyncExc`.
    py_ident: Arc<AtomicI64>,
    worker: Option<JoinHandle<()>>,
}

impl ScriptEngine {
    /// Spawn the worker thread + initialize CPython on it (GIL released after).
    pub fn new() -> Self {
        let (job_tx, job_rx) = mpsc::channel::<ScriptJob>();
        let (reply_tx, reply_rx) = mpsc::channel::<ScriptReply>();
        let (op_tx, op_rx) = mpsc::channel::<ScriptOpMsg>();
        let (op_reply_tx, op_reply_rx) = mpsc::channel::<ScriptOpReplyMsg>();
        // Shared across runs: the worker loop keeps it and hands a clone to
        // each run's OpCtx (mpsc receivers can't be cloned directly).
        let op_reply_rx = Arc::new(std::sync::Mutex::new(op_reply_rx));
        let busy = Arc::new(AtomicBool::new(false));
        let py_ident = Arc::new(AtomicI64::new(0));
        let worker = {
            let busy = busy.clone();
            let py_ident = py_ident.clone();
            std::thread::Builder::new()
                .name("cad_script".into())
                .spawn(move || worker_loop(job_rx, reply_tx, busy, py_ident,
                                           op_tx, op_reply_rx))
                .expect("spawn cad_script worker")
        };
        ScriptEngine {
            job_tx, reply_rx, op_rx, op_reply_tx, busy, py_ident, worker: Some(worker),
        }
    }

    /// Queue a command-line expression/statement. Non-blocking.
    pub fn submit_text(&self, code: impl Into<String>) {
        let _ = self.job_tx.send(ScriptJob::RunText(code.into()));
    }

    /// Queue a `.py` file. Non-blocking.
    pub fn submit_file(&self, path: impl Into<std::path::PathBuf>) {
        let _ = self.job_tx.send(ScriptJob::RunFile(path.into()));
    }

    /// Queue a named script (from the scripts folder) with its argument
    /// vector. The script sees `rasm.args` (args only) and
    /// `sys.argv = [name, …args]`. Non-blocking.
    pub fn submit_script(
        &self,
        path: impl Into<std::path::PathBuf>,
        name: impl Into<String>,
        args: Vec<String>,
    ) {
        let _ = self.job_tx.send(ScriptJob::RunScript {
            path: path.into(),
            name: name.into(),
            args,
            params: Vec::new(),
        });
    }

    /// Like `submit_script`, plus NAMED inputs: `params` becomes the
    /// `rasm.params` dict (string → string); a script's `rasm.main(fn)`
    /// converts each to its declared type. Missing params fall back to the
    /// function's own defaults.
    pub fn submit_script_with_params(
        &self,
        path: impl Into<std::path::PathBuf>,
        name: impl Into<String>,
        args: Vec<String>,
        params: Vec<(String, String)>,
    ) {
        let _ = self.job_tx.send(ScriptJob::RunScript {
            path: path.into(),
            name: name.into(),
            args,
            params,
        });
    }

    /// A GHOST run of a named script with the given params: the app routes
    /// every op to a shadow document (nothing commits, no undo, no history)
    /// and draws the net additions as a dashed preview overlay. The script
    /// itself behaves identically (reads see the shadow snapshot, writes
    /// land in the shadow).
    pub fn submit_script_preview(
        &self,
        path: impl Into<std::path::PathBuf>,
        name: impl Into<String>,
        params: Vec<(String, String)>,
    ) {
        let _ = self.job_tx.send(ScriptJob::RunScript {
            path: path.into(),
            name: name.into(),
            args: Vec::new(),
            params,
        });
    }

    /// Ask the worker for a script's parameter declaration (async — the
    /// answer arrives as `ScriptReply::Meta`). Runs the file with every op
    /// no-op'd, so nothing is drawn and the host is never touched.
    pub fn request_meta(&self, path: impl Into<std::path::PathBuf>) {
        let _ = self.job_tx.send(ScriptJob::Meta { path: path.into() });
    }

    /// Drain all pending replies (call once per frame). Never blocks.
    pub fn poll(&self) -> Vec<ScriptReply> {
        self.reply_rx.try_iter().collect()
    }

    /// Drain every pending script op (call once per frame, BEFORE `poll`).
    /// Never blocks. The host applies each op and answers with `reply_op`.
    pub fn drain_ops(&self) -> Vec<ScriptOpMsg> {
        self.op_rx.try_iter().collect()
    }

    /// Answer one op drained via `drain_ops`. The waiting script resumes.
    pub fn reply_op(&self, id: u64, reply: ScriptOpReply) {
        let _ = self.op_reply_tx.send(ScriptOpReplyMsg { id, reply });
    }

    /// Is a script currently running?
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }

    /// Cooperatively interrupt a running script (Esc / Stop). Raises
    /// KeyboardInterrupt into the worker's interpreter at the next bytecode
    /// boundary. No-op if idle. (A pure-Python loop releases the GIL every few
    /// ms, letting this `with_gil` acquire it to set the async exception.)
    pub fn cancel(&self) {
        if !self.is_busy() {
            return;
        }
        let ident = self.py_ident.load(Ordering::Relaxed);
        if ident == 0 {
            return;
        }
        Python::with_gil(|_py| unsafe {
            pyo3::ffi::PyThreadState_SetAsyncExc(
                ident as std::os::raw::c_ulong as _,
                pyo3::ffi::PyExc_KeyboardInterrupt,
            );
        });
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ScriptEngine {
    fn drop(&mut self) {
        let _ = self.job_tx.send(ScriptJob::Shutdown);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

/// The worker: init Python once, then run jobs until Shutdown.
fn worker_loop(
    job_rx: Receiver<ScriptJob>,
    reply_tx: Sender<ScriptReply>,
    busy: Arc<AtomicBool>,
    py_ident: Arc<AtomicI64>,
    op_tx: Sender<ScriptOpMsg>,
    op_reply_rx: Arc<std::sync::Mutex<Receiver<ScriptOpReplyMsg>>>,
) {
    pyo3::prepare_freethreaded_python();
    // Publish our Python thread ident so cancel() can target us.
    Python::with_gil(|py| {
        if let Ok(id) = current_ident(py) {
            py_ident.store(id, Ordering::Relaxed);
        }
    });
    while let Ok(job) = job_rx.recv() {
        match job {
            ScriptJob::Shutdown => break,
            ScriptJob::RunText(code) => {
                busy.store(true, Ordering::Relaxed);
                // Install the reverse-op context for THIS run: the `rasm`
                // module's functions send ops through these channels and
                // block (GIL-free) until the host replies.
                crate::rasm::install_ctx(OpCtx::new(op_tx.clone(), op_reply_rx.clone()));
                run_one(&reply_tx, &code, true, None, &[]);
                crate::rasm::clear_ctx();
                busy.store(false, Ordering::Relaxed);
            }
            ScriptJob::RunFile(path) => {
                busy.store(true, Ordering::Relaxed);
                crate::rasm::install_ctx(OpCtx::new(op_tx.clone(), op_reply_rx.clone()));
                match std::fs::read_to_string(&path) {
                    Ok(src) => run_one(&reply_tx, &src, false, None, &[]),
                    Err(e) => {
                        let _ = reply_tx.send(ScriptReply::Error(format!(
                            "cannot read {}: {}",
                            path.display(),
                            e
                        )));
                        let _ = reply_tx.send(ScriptReply::Finished { ok: false });
                    }
                }
                crate::rasm::clear_ctx();
                busy.store(false, Ordering::Relaxed);
            }
            ScriptJob::RunScript { path, name, args, params } => {
                busy.store(true, Ordering::Relaxed);
                crate::rasm::install_ctx(OpCtx::new(op_tx.clone(), op_reply_rx.clone()));
                match std::fs::read_to_string(&path) {
                    Ok(src) => run_one(&reply_tx, &src, false, Some((&name, &args)), &params),
                    Err(e) => {
                        let _ = reply_tx.send(ScriptReply::Error(format!(
                            "cannot read {}: {}",
                            path.display(),
                            e
                        )));
                        let _ = reply_tx.send(ScriptReply::Finished { ok: false });
                    }
                }
                crate::rasm::clear_ctx();
                busy.store(false, Ordering::Relaxed);
            }
            ScriptJob::Meta { path } => {
                busy.store(true, Ordering::Relaxed);
                run_meta(&reply_tx, &path);
                busy.store(false, Ordering::Relaxed);
            }
        }
    }
}

/// `threading.get_ident()` — the id `PyThreadState_SetAsyncExc` expects.
fn current_ident(py: Python<'_>) -> PyResult<i64> {
    py.import("threading")?
        .getattr("get_ident")?
        .call0()?
        .extract::<i64>()
}

/// Run one chunk: capture stdout/stderr, eval-then-exec (REPL) or exec (file),
/// and ship value/prints/traceback back. Globals persist in `__main__` so the
/// console behaves like a REPL across submissions.
///
/// `argv` (`Some((name, args))`) marks a named-script run: the script reads
/// its positional inputs via `rasm.args` (args only) and `sys.argv`
/// (`[name, args…]`). `params` are the NAMED inputs — the `rasm.params`
/// dict (str → str) a script's `rasm.main(fn)` converts to declared types.
/// Both `rasm.args` and `rasm.params` are ALWAYS present (empty for plain
/// `py` runs).
///
/// The capture swap is guarded by a process-wide lock: `sys.stdout` is
/// interpreter-global, and two engines running concurrently (e.g. parallel
/// tests) would otherwise capture into each other's buffers.
fn run_one(
    reply_tx: &Sender<ScriptReply>,
    code: &str,
    repl: bool,
    argv: Option<(&str, &[String])>,
    params: &[(String, String)],
) {
    static CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _capture_guard = match CAPTURE_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let outcome = Python::with_gil(|py| -> PyResult<()> {
        let sys = py.import("sys")?;
        let io = py.import("io")?;
        let buf = io.getattr("StringIO")?.call0()?;
        let old_out = sys.getattr("stdout")?;
        let old_err = sys.getattr("stderr")?;
        sys.setattr("stdout", &buf)?;
        sys.setattr("stderr", &buf)?;

        let globals = py.import("__main__")?.dict();
        // Inject the `rasm` module on the first run; the globals persist, so
        // it stays available across submissions (REPL semantics).
        if !globals.contains("rasm")? {
            crate::rasm::install_rasm(py, &globals)?;
        }
        // Per-run inputs: `rasm.args` and `rasm.params` always exist;
        // `sys.argv` only for named-script runs (a plain REPL keeps the
        // previous argv).
        let args: &[String] = argv.map(|(_, a)| a).unwrap_or(&[]);
        if let Ok(Some(rasm)) = globals.get_item("rasm") {
            rasm.setattr("args", PyList::new(py, args)?)?;
            let p = PyDict::new(py);
            for (k, v) in params {
                p.set_item(k, v)?;
            }
            rasm.setattr("params", p)?;
        }
        if let Some((name, _)) = argv {
            let mut full = vec![name.to_string()];
            full.extend(args.iter().cloned());
            sys.setattr("argv", PyList::new(py, &full)?)?;
        }
        let run_result = run_code(py, code, repl, &globals);

        // Restore streams before touching the buffer / reporting.
        sys.setattr("stdout", old_out)?;
        sys.setattr("stderr", old_err)?;
        let captured: String = buf.getattr("getvalue")?.call0()?.extract()?;
        if !captured.is_empty() {
            let _ = reply_tx.send(ScriptReply::Print(captured));
        }

        match run_result {
            Ok(Some(value_repr)) => {
                let _ = reply_tx.send(ScriptReply::Value(value_repr));
            }
            Ok(None) => {}
            Err(err) => {
                let _ = reply_tx.send(ScriptReply::Error(format_traceback(py, &err)));
                let _ = reply_tx.send(ScriptReply::Finished { ok: false });
                return Ok(());
            }
        }
        let _ = reply_tx.send(ScriptReply::Finished { ok: true });
        Ok(())
    });
    // A failure INSIDE the reporting path (rare) must still not be silent.
    if let Err(e) = outcome {
        let _ = reply_tx.send(ScriptReply::Error(format!("script engine error: {e}")));
        let _ = reply_tx.send(ScriptReply::Finished { ok: false });
    }
}

/// The metadata pass behind `request_meta`: run a script file with every
/// rasm op no-op'd (`rasm._META_ONLY`), so top-level code is harmless and
/// `rasm.main(fn)` records the function's parameter declaration instead of
/// drawing. Replies `ScriptReply::Meta(Some(spec))` (or `None` when the
/// script never calls `rasm.main`), then `Finished`.
fn run_meta(reply_tx: &Sender<ScriptReply>, path: &std::path::Path) {
    static CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _capture_guard = match CAPTURE_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            let _ = reply_tx.send(ScriptReply::Error(format!(
                "cannot read {}: {}",
                path.display(),
                e
            )));
            let _ = reply_tx.send(ScriptReply::Finished { ok: false });
            return;
        }
    };
    let outcome = Python::with_gil(|py| -> PyResult<()> {
        let globals = py.import("__main__")?.dict();
        if !globals.contains("rasm")? {
            crate::rasm::install_rasm(py, &globals)?;
        }
        let Some(rasm) = globals.get_item("rasm")? else {
            let _ = reply_tx.send(ScriptReply::Meta(None));
            let _ = reply_tx.send(ScriptReply::Finished { ok: true });
            return Ok(());
        };
        crate::rasm::set_meta_mode(py, &rasm, true)?;
        let run_result = run_code(py, &src, false, &globals);
        let mut meta = crate::rasm::read_meta_spec(py, &rasm)?;
        if let Some(m) = &mut meta {
            m.name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        crate::rasm::set_meta_mode(py, &rasm, false)?;
        match run_result {
            Ok(_) => {
                let _ = reply_tx.send(ScriptReply::Meta(meta));
                let _ = reply_tx.send(ScriptReply::Finished { ok: true });
            }
            Err(err) => {
                // A broken script surfaces its traceback; no dialog (rule 10).
                let _ = reply_tx.send(ScriptReply::Error(format_traceback(py, &err)));
                let _ = reply_tx.send(ScriptReply::Finished { ok: false });
            }
        }
        Ok(())
    });
    if let Err(e) = outcome {
        let _ = reply_tx.send(ScriptReply::Error(format!("script engine error: {e}")));
        let _ = reply_tx.send(ScriptReply::Finished { ok: false });
    }
}

/// Try eval (expression → value) when `repl`; fall back to exec on a
/// SyntaxError; always exec for files. Returns the value's `repr` if any.
fn run_code<'py>(
    py: Python<'py>,
    code: &str,
    repl: bool,
    globals: &Bound<'py, PyDict>,
) -> PyResult<Option<String>> {
    let c = CString::new(code).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    if repl {
        match py.eval(c.as_c_str(), Some(globals), None) {
            Ok(v) => {
                if v.is_none() {
                    return Ok(None);
                }
                let r: String = v.repr()?.extract()?;
                return Ok(Some(r));
            }
            Err(e) if e.is_instance_of::<pyo3::exceptions::PySyntaxError>(py) => {
                // Not an expression — execute as statements.
            }
            Err(e) => return Err(e),
        }
    }
    py.run(c.as_c_str(), Some(globals), None)?;
    Ok(None)
}

/// Format a Python exception as a full traceback string (the `traceback`
/// module, exactly like an uncaught error would print).
fn format_traceback(py: Python<'_>, err: &PyErr) -> String {
    let fallback = || err.to_string();
    let render = || -> PyResult<String> {
        let tb_mod = PyModule::import(py, "traceback")?;
        let tb = err.traceback(py);
        let parts = tb_mod.getattr("format_exception")?.call1((
            err.get_type(py),
            err.value(py),
            tb,
        ))?;
        let joined: String = parts
            .try_iter()?
            .filter_map(|x| x.ok())
            .filter_map(|x| x.extract::<String>().ok())
            .collect();
        Ok(joined)
    };
    render().unwrap_or_else(|_| fallback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    pub(super) fn drain_until_finished(eng: &ScriptEngine) -> Vec<ScriptReply> {
        let mut all = Vec::new();
        let start = Instant::now();
        loop {
            for r in eng.poll() {
                let done = matches!(r, ScriptReply::Finished { .. });
                all.push(r);
                if done {
                    return all;
                }
            }
            if start.elapsed() > Duration::from_secs(10) {
                panic!("script did not finish");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn expr_echoes_value() {
        let eng = ScriptEngine::new();
        eng.submit_text("1+1");
        let replies = drain_until_finished(&eng);
        assert!(replies.iter().any(|r| matches!(r, ScriptReply::Value(v) if v == "2")));
        assert!(replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })));
    }

    #[test]
    fn print_is_captured() {
        let eng = ScriptEngine::new();
        eng.submit_text("print('hello')");
        let replies = drain_until_finished(&eng);
        assert!(replies.iter().any(|r| matches!(r, ScriptReply::Print(s) if s.contains("hello"))));
    }

    #[test]
    fn statement_persists_state() {
        let eng = ScriptEngine::new();
        eng.submit_text("x = 40 + 2");
        drain_until_finished(&eng);
        eng.submit_text("x");
        let replies = drain_until_finished(&eng);
        assert!(replies.iter().any(|r| matches!(r, ScriptReply::Value(v) if v == "42")));
    }

    #[test]
    fn error_yields_traceback_not_silent() {
        let eng = ScriptEngine::new();
        eng.submit_text("1/0");
        let replies = drain_until_finished(&eng);
        assert!(replies.iter().any(|r| matches!(r, ScriptReply::Error(s) if s.contains("ZeroDivisionError"))));
        assert!(replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: false })));
    }

    #[test]
    fn cancel_interrupts_infinite_loop() {
        let eng = ScriptEngine::new();
        eng.submit_text("\nwhile True:\n    pass\n");
        // Wait for it to actually start, then cancel.
        let start = Instant::now();
        while !eng.is_busy() && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(eng.is_busy(), "loop should be running");
        std::thread::sleep(Duration::from_millis(30));
        eng.cancel();
        let replies = drain_until_finished(&eng);
        assert!(replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: false })));
        assert!(!eng.is_busy());
    }

    /// Slices 2–3: the `rasm` module round-trips ops through a fake host.
    /// Pins the blocking contract — the script's adds/reads are answered by
    /// the host's replies and the value echoes back into Python.
    #[test]
    fn rasm_ops_round_trip_through_fake_host() {
        use crate::op::{ScriptOp, ScriptOpReply};

        // A toy host: a counter standing in for the document. The TEST thread
        // is the host — the worker runs the script, blocks on each op, and
        // this loop services them (the exact app-side pattern).
        let mut count: usize = 0;
        let eng = ScriptEngine::new();
        eng.submit_text(
            "i = rasm.add_circle((0.0, 0.0), 1.0)\n\
             n = rasm.doc.count()\n\
             assert i == 0 and n == 1, (i, n)\n\
             layers = rasm.doc.layers()\n\
             assert isinstance(layers, list)\n\
             print('ops ok')\n",
        );
        let mut replies = Vec::new();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            for msg in eng.drain_ops() {
                let reply = match msg.op {
                    ScriptOp::AddCircle { .. } => {
                        let i = count;
                        count += 1;
                        ScriptOpReply::Ok(i)
                    }
                    ScriptOp::DocCount => ScriptOpReply::Count(count),
                    ScriptOp::LayersGet => ScriptOpReply::Layers(Vec::new()),
                    other => ScriptOpReply::Error(format!("unexpected op {other:?}")),
                };
                eng.reply_op(msg.id, reply);
            }
            let mut done = false;
            for r in eng.poll() {
                done |= matches!(r, ScriptReply::Finished { .. });
                replies.push(r);
            }
            if done {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })),
            "script must finish ok: {replies:?}"
        );
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Print(s) if s.contains("ops ok"))),
            "prints must flow back: {replies:?}"
        );
        assert_eq!(count, 1, "host applied the add");
    }

    /// Slice 5: named-script runs expose inputs — `rasm.args` (args only)
    /// and `sys.argv` (`[name, …args]`) — while a plain `py` run sees an
    /// empty `rasm.args`.
    #[test]
    fn named_script_receives_args() {
        let mut path = std::env::temp_dir();
        path.push(format!("cad_script_test_{}.py", std::process::id()));
        let src = "import sys\n\
                   assert rasm.args == ['8', '6'], rasm.args\n\
                   assert sys.argv == ['grid', '8', '6'], sys.argv\n\
                   print('argv ok')\n";
        std::fs::write(&path, src).expect("write temp script");
        let eng = ScriptEngine::new();
        eng.submit_script(path.clone(), "grid", vec!["8".into(), "6".into()]);
        let replies = drain_until_finished(&eng);
        let _ = std::fs::remove_file(&path);
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })),
            "named script must finish ok: {replies:?}"
        );
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Print(s) if s.contains("argv ok"))),
            "argv prints must flow back: {replies:?}"
        );
        // A plain `py` run afterwards must see an EMPTY rasm.args (not the
        // previous run's leftovers).
        eng.submit_text("assert rasm.args == [], rasm.args");
        let replies = drain_until_finished(&eng);
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })),
            "plain run must see empty rasm.args: {replies:?}"
        );
    }

    /// The metadata pass: `request_meta` runs the file with every op no-op'd
    /// and returns the `rasm.main(fn)` declaration (names, types, defaults,
    /// help, ranges) — without the host ever seeing an op.
    #[test]
    fn meta_pass_returns_param_declaration() {
        use crate::op::{ParamType, ScriptParamMeta};
        let mut path = std::env::temp_dir();
        path.push(format!("cad_script_meta_test_{}.py", std::process::id()));
        let src = "def run(outer_d=120.0, bolts=6, label='part'):\n\
                   \x20   '''outer_d: outer diameter (10..500)\n\
                   \x20   bolts: number of bolts\n\
                   \x20   '''\n\
                   \x20   rasm.add_circle((0, 0), outer_d / 2.0)\n\
                   rasm.main(run)\n";
        std::fs::write(&path, src).expect("write temp script");
        let eng = ScriptEngine::new();
        eng.request_meta(path.clone());
        let replies = drain_until_finished(&eng);
        let _ = std::fs::remove_file(&path);
        let meta = replies.iter().find_map(|r| match r {
            ScriptReply::Meta(m) => Some(m.clone()),
            _ => None,
        });
        let meta = meta.expect("a Meta reply must arrive");
        let meta = meta.expect("the script declares params");
        assert_eq!(meta.params.len(), 3, "three params declared: {meta:?}");
        let p: &ScriptParamMeta = &meta.params[0];
        assert_eq!(p.name, "outer_d");
        assert_eq!(p.ptype, ParamType::Float);
        assert_eq!(p.default, "120.0");
        assert_eq!((p.min, p.max), (Some(10.0), Some(500.0)));
        assert!(p.help.contains("outer diameter"), "help parsed: {p:?}");
        assert_eq!(meta.params[1].ptype, ParamType::Int);
        assert_eq!(meta.params[2].ptype, ParamType::Str);
        assert_eq!(meta.params[2].default, "part", "string defaults carry NO repr quotes");
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })),
            "meta pass finishes: {replies:?}"
        );
    }

    /// A named-params run: `rasm.main` converts `rasm.params` to the declared
    /// types and passes them to the function; missing ones keep defaults.
    #[test]
    fn named_params_reach_the_script() {
        let mut path = std::env::temp_dir();
        path.push(format!("cad_script_params_test_{}.py", std::process::id()));
        let src = "def run(outer_d=120.0, bolts=6):\n\
                   \x20   print('got', repr(outer_d), repr(bolts))\n\
                   rasm.main(run)\n";
        std::fs::write(&path, src).expect("write temp script");
        let eng = ScriptEngine::new();
        eng.submit_script_with_params(
            path.clone(),
            "params_test",
            Vec::new(),
            vec![("outer_d".into(), "55.5".into()), ("bolts".into(), "9".into())],
        );
        let replies = drain_until_finished(&eng);
        let _ = std::fs::remove_file(&path);
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Print(s) if s.contains("got 55.5 9"))),
            "typed values must reach the function: {replies:?}"
        );
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })),
            "params run finishes ok: {replies:?}"
        );
    }

    /// Legacy positional inputs (`run <name> v1 v2`) fall back onto the
    /// declared parameter order when no named params are given.
    #[test]
    fn positional_args_map_to_declared_order() {
        let mut path = std::env::temp_dir();
        path.push(format!("cad_script_pos_test_{}.py", std::process::id()));
        let src = "def run(outer_d=120.0, bolts=6):\n\
                   \x20   print('got', repr(outer_d), repr(bolts))\n\
                   rasm.main(run)\n";
        std::fs::write(&path, src).expect("write temp script");
        let eng = ScriptEngine::new();
        eng.submit_script(path.clone(), "pos_test", vec!["77.5".into(), "4".into()]);
        let replies = drain_until_finished(&eng);
        let _ = std::fs::remove_file(&path);
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Print(s) if s.contains("got 77.5 4"))),
            "positional args must map to declared order: {replies:?}"
        );
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })),
            "positional run finishes ok: {replies:?}"
        );
    }

    /// `point` and `color` declarations: a tuple default types a param as a
    /// canvas position, a string annotation types an int default as an ACI
    /// color — and the values convert to `(x, y)` / int at run time.
    #[test]
    fn point_and_color_params_declare_and_convert() {
        use crate::op::ParamType;
        let mut path = std::env::temp_dir();
        path.push(format!("cad_script_pt_test_{}.py", std::process::id()));
        let src = "def run(pos: 'point' = (1.0, 2.0), holes_color: 'color' = 5):\n\
                   \x20   print('got', repr(pos), repr(holes_color))\n\
                   rasm.main(run)\n";
        std::fs::write(&path, src).expect("write temp script");
        let eng = ScriptEngine::new();
        eng.request_meta(path.clone());
        let replies = drain_until_finished(&eng);
        let meta = replies.iter().find_map(|r| match r {
            ScriptReply::Meta(m) => Some(m.clone()),
            _ => None,
        });
        let meta = meta.expect("a Meta reply must arrive").expect("params declared");
        assert_eq!(meta.params[0].ptype, ParamType::Point, "{:?}", meta.params[0]);
        assert_eq!(meta.params[0].default, "(1.0, 2.0)");
        assert_eq!(meta.params[1].ptype, ParamType::Color);
        assert_eq!(meta.params[1].default, "5");
        // And a real run converts them.
        eng.submit_script_with_params(
            path.clone(),
            "pt_test",
            Vec::new(),
            vec![("pos".into(), "30.5,40.25".into()), ("holes_color".into(), "3".into())],
        );
        let replies = drain_until_finished(&eng);
        let _ = std::fs::remove_file(&path);
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Print(s)
                if s.contains("got (30.5, 40.25) 3"))),
            "point must convert to a tuple and color to an int: {replies:?}"
        );
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })),
            "point/color run finishes ok: {replies:?}"
        );
    }

    /// `length` declarations: the metadata reports the type; the value
    /// arrives pre-converted by the host (scene units) and just parses as
    /// a float.
    #[test]
    fn length_params_declare_and_convert() {
        use crate::op::ParamType;
        let mut path = std::env::temp_dir();
        path.push(format!("cad_script_len_test_{}.py", std::process::id()));
        let src = "def run(outer_d: 'length' = 120.0):\n\
                   \x20   print('got', repr(outer_d))\n\
                   rasm.main(run)\n";
        std::fs::write(&path, src).expect("write temp script");
        let eng = ScriptEngine::new();
        eng.request_meta(path.clone());
        let replies = drain_until_finished(&eng);
        let meta = replies.iter().find_map(|r| match r {
            ScriptReply::Meta(m) => Some(m.clone()),
            _ => None,
        });
        let meta = meta.expect("a Meta reply must arrive").expect("params declared");
        assert_eq!(meta.params[0].ptype, ParamType::Length, "{:?}", meta.params[0]);
        assert_eq!(meta.params[0].default, "120.0");
        eng.submit_script_with_params(
            path.clone(),
            "len_test",
            Vec::new(),
            vec![("outer_d".into(), "250".into())],
        );
        let replies = drain_until_finished(&eng);
        let _ = std::fs::remove_file(&path);
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Print(s) if s.contains("got 250.0"))),
            "length values arrive as scene-unit floats: {replies:?}"
        );
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })),
            "length run finishes ok: {replies:?}"
        );
    }
}

// WP-SCRIPT audit follow-up: choice + list param declarations and
// conversions.
#[cfg(test)]
mod choice_list_param_tests {
    use super::tests::drain_until_finished;
    use super::*;
    use crate::op::ParamType;

    #[test]
    fn choice_and_list_params_declare_and_convert() {
        let mut path = std::env::temp_dir();
        path.push(format!("cad_script_choice_test_{}.py", std::process::id()));
        let src = "def run(pattern: 'choice' = 'ANSI31', xs: 'float_list' = (1.0, 2.0),\n\
                   \x20        names: 'str_list' = ('a', 'b'),\n\
                   \x20        grid: 'point_list' = ((0.0, 0.0), (10.0, 0.0))):\n\
                   \x20   '''pattern: hatch pattern [SOLID, ANSI31, BRICK]\n\
                   \x20   '''\n\
                   \x20   print('got', pattern, xs, names, grid)\n\
                   rasm.main(run)\n";
        std::fs::write(&path, src).expect("write temp script");
        let eng = ScriptEngine::new();
        eng.request_meta(path.clone());
        let replies = drain_until_finished(&eng);
        let meta = replies.iter().find_map(|r| match r {
            ScriptReply::Meta(m) => Some(m.clone()),
            _ => None,
        });
        let meta = meta.expect("a Meta reply must arrive").expect("params declared");
        assert_eq!(meta.params[0].ptype, ParamType::Choice);
        assert_eq!(meta.params[0].choices, vec!["SOLID", "ANSI31", "BRICK"]);
        assert_eq!(meta.params[0].help, "hatch pattern");
        assert_eq!(meta.params[1].ptype, ParamType::FloatList);
        assert_eq!(meta.params[2].ptype, ParamType::StrList);
        assert_eq!(meta.params[3].ptype, ParamType::PointList);
        // A run converts each form.
        eng.submit_script_with_params(
            path.clone(),
            "choice_test",
            Vec::new(),
            vec![
                ("pattern".into(), "BRICK".into()),
                ("xs".into(), "3,4,5".into()),
                ("names".into(), "x, y".into()),
                ("grid".into(), "(1,2); (3,4)".into()),
            ],
        );
        let replies = drain_until_finished(&eng);
        let _ = std::fs::remove_file(&path);
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Print(s)
                if s.contains("got BRICK [3.0, 4.0, 5.0] ['x', 'y'] [(1.0, 2.0), (3.0, 4.0)]"))),
            "lists must convert: {replies:?}"
        );
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Finished { ok: true })),
            "choice/list run finishes ok: {replies:?}"
        );
        // Catalog-backed types declare via annotations too.
        let src2 = "def run(lt: 'linetype' = 'Continuous', ly: 'layer' = '0',\n\
                     \x20        bl: 'block' = 'B', hp: 'hatch_pattern' = 'SOLID'):\n\
                     \x20   print('ok')\n\
                     rasm.main(run)\n";
        std::fs::write(&path, src2).expect("write temp script");
        eng.request_meta(path.clone());
        let replies2 = drain_until_finished(&eng);
        let meta2 = replies2.iter().find_map(|r| match r {
            ScriptReply::Meta(m) => Some(m.clone()),
            _ => None,
        });
        let meta2 = meta2.expect("a Meta reply must arrive").expect("params declared");
        assert_eq!(meta2.params[0].ptype, ParamType::Linetype, "{:?}", meta2.params[0]);
        assert_eq!(meta2.params[1].ptype, ParamType::Layer);
        assert_eq!(meta2.params[2].ptype, ParamType::Block);
        assert_eq!(meta2.params[3].ptype, ParamType::HatchPattern);
    }

    #[test]
    fn bad_choice_value_fails_loudly() {
        let mut path = std::env::temp_dir();
        path.push(format!("cad_script_badchoice_test_{}.py", std::process::id()));
        let src = "def run(pattern: 'choice' = 'SOLID'):\n\
                   \x20   '''pattern: hatch pattern [SOLID, ANSI31]\n\
                   \x20   '''\n\
                   \x20   print('ran')\n\
                   rasm.main(run)\n";
        std::fs::write(&path, src).expect("write temp script");
        let eng = ScriptEngine::new();
        eng.submit_script_with_params(
            path.clone(),
            "bad_choice",
            Vec::new(),
            vec![("pattern".into(), "NOPE".into())],
        );
        let replies = drain_until_finished(&eng);
        let _ = std::fs::remove_file(&path);
        assert!(
            replies.iter().any(|r| matches!(r, ScriptReply::Error(e) if e.contains("not one of"))),
            "a value outside the choices must fail loudly: {replies:?}"
        );
    }
}
