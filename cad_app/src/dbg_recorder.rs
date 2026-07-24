//! Session recorder — high-fidelity event tap for short debug sessions.
//!
//! Captures EVERY user action, state transition, doc mutation, and
//! memory incident during a few-second recording. Output is structured
//! `DbgEvent` records with: timestamp, kind-specific payload, and the
//! source-code `Location` of the caller that fired the event (via
//! `#[track_caller]` + `Location::caller()`).
//!
//! Two use-cases:
//!   1. **Bug repro** — paste the JSON dump into a bug report; reader
//!      sees the EXACT input sequence + the EXACT outcome at each step.
//!   2. **Algorithm extraction** — record yourself performing the
//!      intended workflow; read the resulting event list as a spec for
//!      a new command.
//!
//! Design choices for fidelity:
//!   * Snapshots taken at session start, on demand, AND every N events
//!     (default 50). Document is `Clone`, so the cost is one large
//!     memcpy per snapshot — fine at ~100KB/snap.
//!   * No global-allocator override for memory tracking. Instead the
//!     "memory incident" payloads include sizes the kernel exposes:
//!     `Document.dobjects.len()`, undo-stack depth, spatial-index entry
//!     count. That's enough to spot the bugs we hit in practice.
//!   * Event push is hot (≤1 µs typical). Snapshot push is the cost
//!     spike (1–5 ms). Recording is OFF by default — never runs unless
//!     the user pressed Start in the Recorder window.

use cad_kernel::{Document, Vec2};
use std::fs::File;
use std::io::Write;
use std::panic::Location;
use std::path::PathBuf;
use std::time::Instant;

// ===========================================================================
//                              EVENT KINDS
// ===========================================================================

/// One captured event. Variants are deliberately chunky — a CmdRun
/// includes the parsed result and source, a CanvasClick includes the
/// hit-test outcome and drag-classification result, etc. We want
/// READABLE events, not raw key strokes.
#[derive(Clone, Debug)]
pub enum DbgEvent {
    /// Recorder armed / disarmed (matches Start/Stop button presses).
    SessionStart { reason: String },
    SessionStop  { reason: String, event_count: usize },

    /// Manual annotation — note dropped via the "📝 Note" button so the
    /// reader can mark "this is where the bug fired".
    Note { message: String },

    /// One full `Document` snapshot. Heavy. Tagged with the reason
    /// (auto-cadence / manual / pre-undo / post-snapshot_doc).
    DocSnapshot {
        reason:         String,
        dobject_count:  usize,
        undo_depth:     usize,
        redo_depth:     usize,
        layer_count:    usize,
        index_in_dump:  usize,  // index into DbgRecorder::snapshots
    },

    /// `run_command(raw)` fired. Records the RAW input + the parsed
    /// `Command` debug-print + where the call came from (typed in cmd
    /// line, menu button, replay).
    CmdRun {
        raw:           String,
        parsed_debug:  String,
        source:        CmdSource,
        /// Wall-clock the command took to EXECUTE, in µs. The event timestamp says
        /// WHEN it ran; this says HOW LONG it took — so when the app stalls, the dump
        /// names the command responsible instead of leaving you to guess.
        /// `None` = not measured (the value came from an older capture).
        elapsed_us:    Option<u64>,
    },

    /// A ZOOM operation (2D or 3D-Factory) — everything needed to judge "is zoom
    /// proper": the command as typed, the CHOICES the command line offers for it, the
    /// resolved sub-command, and the screen zoom status BEFORE vs AFTER. Without this,
    /// a zoom bug is invisible — the dump only showed a bare CMD line and a note.
    ZoomOp {
        cmd:      String, // raw: "zoom", "z", "zoom w", "zoom 2"
        choices:  String, // what the command line offers as zoom sub-commands
        action:   String, // resolved sub-command: "window armed", "extents", "in", …
        before:   String, // screen zoom status before
        after:    String, // screen zoom status after
    },

    /// Canvas mouse click — fully decoded.
    CanvasClick {
        screen:        (f32, f32),
        world:         Vec2,
        modifiers:     KeyModifiers,
        hit_dobject:   Option<usize>,
        active_tool:   String,
        active_state:  String,    // one-line summary of every state machine
    },
    /// Mouse press / release / drag (less decoded — just positions).
    CanvasPress   { screen: (f32, f32), world: Vec2, button: String },
    CanvasRelease { screen: (f32, f32), world: Vec2, button: String, drag_px: f32 },

    /// FULL gesture decoder — fires AFTER every press-release pair on
    /// the canvas, regardless of whether it was classified as click or
    /// drag. Captures both egui's classification AND the app's outcome,
    /// so a glance at this event tells you "the user dragged 263 px
    /// R→L but it was demoted to a click and nothing was selected".
    GestureClassification {
        press_screen:        (f32, f32),
        release_screen:      (f32, f32),
        press_world:         Vec2,
        release_world:       Vec2,
        motion_px:           f32,
        motion_dir:          String,    // "L→R ↓ 263 px", "stationary", "vertical ↑"
        egui_clicked:        bool,
        egui_drag_stopped:   bool,
        hit_at_press:        Option<usize>,
        hit_at_release:      Option<usize>,
        in_select_mode:      bool,
        active_tool:         String,
        /// EMPTIED at capture when `counts_only` is on — see `n_before`/`n_after`.
        selection_before:    Vec<usize>,
        selection_after:     Vec<usize>,
        /// True sizes, stamped by `push()` before any stripping. Always valid.
        n_before:            usize,
        n_after:             usize,
        app_action_taken:    String,    // "click_select(i=2)", "window_first=Some(...)", "add_window_selection", "NOOP — gesture had no effect"
        outcome_summary:     String,    // human-readable verdict
    },
    CanvasDrag {
        from_screen: (f32, f32),
        to_screen:   (f32, f32),
        from_world:  Vec2,
        to_world:    Vec2,
        button:      String,
    },

    /// Selection mutated — what was the basket BEFORE vs AFTER.
    SelectChange {
        /// The baskets. EMPTIED at capture when `counts_only` is on — read
        /// `n_before`/`n_after` for the size, which is always populated.
        basket_before: Vec<usize>,
        basket_after:  Vec<usize>,
        /// True basket sizes, stamped by `push()` before any stripping. Always valid.
        n_before:      usize,
        n_after:       usize,
        cause:         String,    // "click_select(i=5, shift=false)" etc.
    },

    /// Tool changed. Tools mean "what's the user drafting / picking".
    ToolChange { from: String, to: String, cause: String },

    /// Generic state machine transition. `state_name` is the field name
    /// (`"trim_state"`, `"fillet_state"`, etc.). `before`/`after` are
    /// Debug-printed.
    StateChange { state_name: String, before: String, after: String, cause: String },

    /// `Document::push` — new dobject added. Records its index, the
    /// Geom variant name, the handle, the kind-summary.
    DocPush {
        index:         usize,
        geom_kind:     String,
        handle:        u64,
        summary:       String,
    },
    /// `Vec::remove(i)` on doc.dobjects — index + Geom kind + summary.
    DocRemove {
        index:         usize,
        geom_kind:     String,
        summary:       String,
    },

    /// `snapshot_doc()` called — the moment that pushes onto undo_stack.
    UndoSnapshotTaken {
        undo_depth_after: usize,
        bytes_estimate:   usize,
    },
    /// `do_undo()` fired — what fell off / what came back.
    UndoFired { from_depth: usize, to_depth: usize },
    /// `do_redo()` fired.
    RedoFired { from_depth: usize, to_depth: usize },

    /// One of the `apply_*` methods ran. Generic envelope.
    ApplyOp {
        name:               String,    // "apply_trim_pick", "apply_chprop", "apply_hatch"
        before_dobj_count:  usize,
        after_dobj_count:   usize,
        success:            bool,
        detail:             String,    // free-form per-op summary
    },

    /// A frame that BLEW THE BUDGET, with a breakdown of where it went.
    ///
    /// Emitted only when a frame exceeds one refresh (16.7 ms @ 60 Hz) — i.e. only
    /// when the user can actually SEE the stall — so it costs nothing on a healthy
    /// frame and cannot flood the dump.
    ///
    /// This exists because "zoom/pan got slow" was previously unfalsifiable: the dump
    /// showed clicks seconds apart with no way to tell user think-time from frame
    /// time. `candidates` is the key column — at 1.5M dobjects a zoomed-out query
    /// returns EVERY dobject, and the render loop then walks all of them.
    SlowFrame {
        total_us:   u64,
        query_us:   u64,
        draw_us:    u64,
        candidates: usize,
        drawn:      usize,
        capped:     bool,
    },

    /// Spatial-index rebuild — O(n) over the WHOLE drawing, triggered by ~58 places
    /// marking `index_dirty` (i.e. essentially every edit).
    ///
    /// It used to log only to the command history, so a dump could not see it: at 1.5M
    /// dobjects it is ~119 ms PER EDIT — bigger than the 98 ms undo clone that IS
    /// logged. Half the per-edit cost was invisible in the dump.
    IndexRebuild {
        dobjects:   usize,
        elapsed_us: u64,
    },

    /// Memory incident — Doc clone, grid rebuild, ACI table grow, etc.
    /// `bytes` is the BEST-EFFORT size estimate; `name` describes WHAT.
    MemoryEvent {
        name:        String,
        bytes:       usize,
        elapsed_us:  u64,
    },

    /// On-demand snapshot of the user-selected smart-dobject CANDIDATE —
    /// the full geometry (coordinates) of the chosen dobjects, whether an
    /// exploded set OR a block (then the block's DEFINITION-space contents
    /// are dumped too). This is the BASE geometry the parametric rule
    /// (from `StretchRecord` steps) transforms; together they're enough to
    /// convert the selection into a parametric smart block.
    GeometryCapture {
        label:   String,
        entries: Vec<String>,   // one multi-line block per captured dobject
    },

    /// A STRETCH captured for smart-block authoring. The box, the vector,
    /// and every dobject whose geometry CHANGED — with full before→after
    /// coordinates. This is the raw data to extract a parametric rule:
    /// which dobjects move, by what vector, inside which box. Recorded for
    /// every `stretch`/DDE-stretch while the recorder is running.
    StretchRecord {
        box_min:        (f64, f64),
        box_max:        (f64, f64),
        base:           (f64, f64),
        dest:           (f64, f64),
        vector:         (f64, f64),
        total_selected: usize,
        /// One pre-formatted multi-line block per CHANGED dobject
        /// (idx + handle + kind + verbose before/after coordinates).
        affected:       Vec<String>,
    },

    /// Dialog or palette state changed.
    WindowToggle { name: String, opened: bool },
    /// Captured pixel geometry of a UI menu — the whole frame plus every
    /// element (label, value box, swatch, button…) as `name: x y w h` in
    /// screen points. Lets the reader compare the RENDERED layout against
    /// the intended design.
    MenuLayout   { label: String, elements: Vec<String> },
    /// Menu button clicked from the menu bar.
    MenuClick    { path: String },
    /// Keyboard event handled outside the cmd-line text edit (Esc, F-keys).
    KeyEvent     { key: String, modifiers: KeyModifiers },
}

/// Where a `run_command` invocation came from. Lets the inspector
/// distinguish "user typed it" from "menu triggered it" from "replay".
#[derive(Clone, Debug)]
pub enum CmdSource {
    Typed,
    Menu(&'static str),
    Replay,
    Internal(&'static str),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl:  bool,
    pub alt:   bool,
}

// ===========================================================================
//                              RECORDER
// ===========================================================================

/// One recorded event with timestamp + file:line of the caller.
#[derive(Clone, Debug)]
pub struct DbgRecord {
    pub elapsed_ms: f64,
    pub event:      DbgEvent,
    pub location:   &'static Location<'static>,
}

/// How many dobjects' geometry a snapshot keeps. Enough to verify the draw
/// functions (what the snapshots are actually FOR) and nothing more.
///
/// ⚠️ This cap is a PERFORMANCE contract, not a display preference. Snapshots used to
/// hold a full `Document` **clone** — which nothing ever read — so an auto-snap every
/// 50 events cloned the entire drawing. At a million dobjects that is not a large
/// file, it is a freeze. Never restore the clone; if a future replay feature needs
/// full state, snapshot a DELTA or write to disk, don't clone the document into RAM.
pub const SNAP_GEOM_MAX: usize = 20;

#[derive(Clone)]
pub struct DbgSnapshot {
    pub event_index: usize,
    pub tag:         String,
    /// Geometry (full coordinates) of the first [`SNAP_GEOM_MAX`] dobjects.
    pub geom:        Vec<String>,
    /// How many dobjects were NOT captured. Reported in the dump so a capped
    /// snapshot can never be mistaken for a complete one.
    pub omitted:     usize,
}

/// The recorder. One per `CadApp`. When `recording == false` every
/// `record(...)` is a tiny no-op so we can leave the calls in for
/// production builds.
pub struct DbgRecorder {
    pub recording:        bool,
    /// **Count dobjects ONLY.** Keep the NUMBER of selected/affected dobjects and
    /// nothing else — never the index lists themselves.
    ///
    /// Why it exists: `SelectChange`/`GestureClassification` carry `Vec<usize>`
    /// baskets, and a gesture prints its selection TWICE (before → after). At 916
    /// selected that is ~11 KB per event; at a million it is ~11 MB per event, and the
    /// dump stops being pasteable at all. With this on, the lists are dropped AT
    /// CAPTURE — not merely hidden at render — so the recorder's own memory stays O(1)
    /// in the selection size.
    pub counts_only:      bool,
    pub session_started:  Option<Instant>,
    pub events:           Vec<DbgRecord>,
    pub snapshots:        Vec<DbgSnapshot>,
    /// Auto-snapshot cadence — every N events. 0 disables.
    pub auto_snap_every:  usize,
    /// Ring-buffer cap. Once exceeded, oldest events drop. 0 = no cap.
    pub max_events:       usize,
    /// `true` → capture full Backtrace per event (very slow). Off by default.
    pub capture_backtrace: bool,
    /// Path of the durable on-disk mirror for the CURRENT session (set on
    /// `start`). The UI shows this so the user knows where to find the
    /// dump after a crash / force-close.
    pub log_path:         Option<PathBuf>,
    /// Open handle to the on-disk mirror. Every event is written to it
    /// immediately (unbuffered `File` → bytes reach the kernel), so the
    /// timeline survives a panic, a hang the user force-closes, or an OOM
    /// kill. `None` when not recording or if the file couldn't be opened.
    file:                 Option<File>,
    /// Monotonic sequence number for on-disk line labels (unaffected by
    /// in-memory ring eviction, so file `#NNNNN` are always increasing).
    written:              usize,
}

impl Default for DbgRecorder {
    fn default() -> Self {
        Self {
            recording:        false,
            counts_only:      false,
            session_started:  None,
            events:           Vec::new(),
            snapshots:        Vec::new(),
            auto_snap_every:  50,
            max_events:       100_000,
            capture_backtrace: false,
            log_path:         None,
            file:             None,
            written:          0,
        }
    }
}

impl DbgRecorder {
    pub fn start(&mut self, reason: &str) {
        self.recording = true;
        self.session_started = Some(Instant::now());
        self.events.clear();
        self.snapshots.clear();
        self.written = 0;
        // Durable on-disk mirror — open a FRESH log so a crash / hang the
        // user force-closes / OOM kill still leaves the whole timeline on
        // disk. Best-effort: a failed open just disables the mirror; the
        // in-memory recorder is unaffected. A plain `File` (no BufWriter)
        // is deliberate — each write is handed to the kernel immediately,
        // so the bytes survive even a hard kill of this process.
        let path = std::env::temp_dir().join("rust_cad_session.log");
        self.file = File::create(&path).ok();
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "=== SESSION RECORDER (live mirror) — {} ===", reason);
        }
        eprintln!("[recorder] live crash-durable mirror → {}", path.display());
        self.log_path = Some(path);
        // Stamp the start event without an auto-snapshot — the caller
        // (CadApp::dbg_start) is responsible for pushing the initial
        // doc snapshot via `take_snapshot`. Decoupled because the
        // recorder doesn't own the Document.
        self.events.push(DbgRecord {
            elapsed_ms: 0.0,
            event: DbgEvent::SessionStart { reason: reason.to_string() },
            location: Location::caller(),
        });
        self.mirror_last_to_file();
    }

    pub fn stop(&mut self, reason: &str) {
        let count = self.events.len();
        if self.recording {
            self.events.push(DbgRecord {
                elapsed_ms: self.elapsed_ms(),
                event: DbgEvent::SessionStop {
                    reason: reason.to_string(),
                    event_count: count,
                },
                location: Location::caller(),
            });
            self.mirror_last_to_file();
            if let Some(f) = self.file.as_mut() {
                let _ = writeln!(f, "=== END SESSION ({} events, {} snapshots) ===",
                    self.events.len(), self.snapshots.len());
                let _ = f.flush();
            }
        }
        self.file = None;   // close the handle — the OS flushes to disk
        self.recording = false;
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.snapshots.clear();
    }

    /// Push one event. Cheap when `!recording`. The `loc` is the
    /// CALLER's location, threaded through by the `dbg_event!` macro.
    /// Stamp a `CmdRun`'s execution time — the FIRST one at or after `from_idx`.
    ///
    /// Indexed, not "the last CmdRun", because `run_command` is RE-ENTRANT: a select
    /// session rewrites `p`/`l`/`d` into `run_command("previous")` &co. Patching "the
    /// last" would let a nested call steal the outer command's slot. Each caller
    /// remembers the event count from before its own push and patches from there, so
    /// an outer command's time correctly INCLUDES the inner one it spawned.
    pub fn patch_cmd_elapsed_at(&mut self, from_idx: usize, us: u64) {
        if !self.recording { return; }
        for r in self.events.iter_mut().skip(from_idx) {
            if let DbgEvent::CmdRun { elapsed_us, .. } = &mut r.event {
                if elapsed_us.is_none() {
                    *elapsed_us = Some(us);
                    return;
                }
            }
        }
    }

    pub fn push(&mut self, mut event: DbgEvent, loc: &'static Location<'static>) {
        if !self.recording { return; }
        // COUNT-ONLY — strip dobject index lists at CAPTURE. Done here, in the single
        // choke point every event passes through, so no emission site can forget it
        // and no future one can bypass it. The counts survive (see `n_before/n_after`).
        // Counts are stamped ALWAYS (so the render never has to trust the vecs);
        // the lists are dropped only in counts-only mode.
        let strip = self.counts_only;
        match &mut event {
            DbgEvent::SelectChange { basket_before, basket_after, n_before, n_after, .. } => {
                *n_before = basket_before.len();
                *n_after = basket_after.len();
                if strip {
                    *basket_before = Vec::new();
                    *basket_after = Vec::new();
                }
            }
            DbgEvent::GestureClassification { selection_before, selection_after, n_before, n_after, .. } => {
                *n_before = selection_before.len();
                *n_after = selection_after.len();
                if strip {
                    *selection_before = Vec::new();
                    *selection_after = Vec::new();
                }
            }
            _ => {}
        }
        if self.max_events > 0 && self.events.len() >= self.max_events {
            // Ring eviction — drop oldest 5% to amortise the move cost.
            let drop = self.max_events / 20;
            self.events.drain(..drop);
        }
        self.events.push(DbgRecord {
            elapsed_ms: self.elapsed_ms(),
            event,
            location: loc,
        });
        self.mirror_last_to_file();
    }

    /// Append the most-recently-pushed event to the durable on-disk mirror
    /// (if open). Formats one line identical to `dump_text`, using a
    /// monotonic sequence number so the file stays readable even after
    /// in-memory ring eviction. Best-effort — write errors are ignored so
    /// logging can never break the app.
    fn mirror_last_to_file(&mut self) {
        if self.file.is_none() { return; }
        let seq = self.written;
        let line = match self.events.last() {
            Some(r) => format!(
                "[{:6.1} ms] #{:05} @ {}:{} — {}",
                r.elapsed_ms, seq,
                r.location.file().rsplit('/').next().unwrap_or(r.location.file()),
                r.location.line(),
                format_event_oneline(&r.event)),
            None => return,
        };
        if let Some(f) = self.file.as_mut() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.write_all(b"\n");
        }
        self.written += 1;
    }

    /// Push a Document snapshot. Caller (CadApp) owns the Document and
    /// passes a clone. Records a `DocSnapshot` event referencing the
    /// snapshot's index into `self.snapshots`.
    /// Snapshot the drawing. `describe` formats one dobject's geometry — passed in so
    /// the recorder stays free of app formatting.
    ///
    /// Captures at most [`SNAP_GEOM_MAX`] dobjects: O(20), not O(n). Previously this
    /// cloned the whole `Document` — a clone no code ever read — which made every
    /// auto-snap O(n) in both time and memory.
    pub fn take_snapshot(
        &mut self,
        doc: &Document,
        reason: &str,
        undo_depth: usize,
        redo_depth: usize,
        describe: impl Fn(&cad_kernel::Geom) -> String,
        loc: &'static Location<'static>,
    ) {
        if !self.recording { return; }
        let idx = self.snapshots.len();
        let n = doc.dobjects.len();
        let geom: Vec<String> = doc
            .dobjects
            .iter()
            .take(SNAP_GEOM_MAX)
            .enumerate()
            .map(|(i, d)| format!("#{i:<4} h={:<6} {}", d.handle, describe(&d.geom)))
            .collect();
        self.snapshots.push(DbgSnapshot {
            event_index: self.events.len(),
            tag:         reason.to_string(),
            geom,
            omitted:     n.saturating_sub(SNAP_GEOM_MAX),
        });
        let dobject_count = doc.dobjects.len();
        let layer_count   = doc.layers.len();
        self.push(
            DbgEvent::DocSnapshot {
                reason:         reason.to_string(),
                dobject_count,
                undo_depth,
                redo_depth,
                layer_count,
                index_in_dump:  idx,
            },
            loc,
        );
    }

    /// Should auto-cadence fire a snapshot AT THIS POINT? Counts events
    /// since the last snapshot.
    pub fn want_auto_snap(&self) -> bool {
        if !self.recording || self.auto_snap_every == 0 { return false; }
        let last_snap_at = self.snapshots.last().map(|s| s.event_index).unwrap_or(0);
        let since = self.events.len().saturating_sub(last_snap_at);
        since >= self.auto_snap_every
    }

    fn elapsed_ms(&self) -> f64 {
        self.session_started
            .map(|t| t.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }

    /// Export the whole session as a human-readable text dump.
    /// Format: one event per line, time-stamped, file:line stamped.
    /// Designed for paste-into-bug-report + read-by-eye.
    pub fn dump_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "=== SESSION DUMP ({} events, {} snapshots) ===\n",
            self.events.len(), self.snapshots.len()));
        for (i, r) in self.events.iter().enumerate() {
            out.push_str(&format!(
                "[{:6.1} ms] #{:05} @ {}:{} — {}\n",
                r.elapsed_ms,
                i,
                r.location.file().rsplit('/').next().unwrap_or(r.location.file()),
                r.location.line(),
                format_event_oneline(&r.event),
            ));
        }
        // ---- GEOMETRY --------------------------------------------------
        // The first SNAP_GEOM_MAX dobjects, with full coordinates — enough to verify
        // the draw functions, which is what these snapshots are FOR. Deliberately
        // capped: dumping a million dobjects would produce an unreadable, enormous
        // file, and the interesting ones for a draw test are the ones you just drew.
        // `omitted` is always reported, so a capped dump can never be misread as a
        // complete one.
        for (i, snap) in self.snapshots.iter().enumerate() {
            out.push_str(&format!(
                "\n--- SNAP[{}] geometry ({}) — {} shown{} ---\n",
                i,
                snap.tag,
                snap.geom.len(),
                if snap.omitted > 0 {
                    format!(", {} MORE OMITTED (cap {})", snap.omitted, SNAP_GEOM_MAX)
                } else {
                    String::new()
                },
            ));
            for g in &snap.geom {
                out.push_str(&format!("  {g}\n"));
            }
            if snap.geom.is_empty() {
                out.push_str("  (empty drawing)\n");
            }
        }
        out.push_str(&format!(
            "=== END SESSION ({} snapshots in side-buffer) ===\n",
            self.snapshots.len()));
        out
    }
}

/// One-line summary of an event for the text dump. Verbose enough to
/// read at a glance, terse enough that 1000 events fit on screen.
pub fn format_event_oneline(e: &DbgEvent) -> String {
    match e {
        DbgEvent::SessionStart { reason } =>
            format!("◆ SESSION START — {}", reason),
        DbgEvent::SessionStop { reason, event_count } =>
            format!("◆ SESSION STOP — {} ({} events)", reason, event_count),
        DbgEvent::Note { message } =>
            format!("📝 NOTE: {}", message),
        DbgEvent::SlowFrame { total_us, query_us, draw_us, candidates, drawn, capped } => {
            format!(
                "🐢 SLOW FRAME {:.1} ms  (query {:.1} · draw {:.1})  candidates={} drawn={}{}",
                *total_us as f64 / 1000.0,
                *query_us as f64 / 1000.0,
                *draw_us as f64 / 1000.0,
                candidates,
                drawn,
                if *capped { "  ⚠ DRAW CAPPED (frame truncated)" } else { "" },
            )
        }
        DbgEvent::IndexRebuild { dobjects, elapsed_us } => {
            let ms = *elapsed_us as f64 / 1000.0;
            // 16 ms = one frame @ 60 Hz — past that the stall is VISIBLE
            let flag = if *elapsed_us >= 16_000 { "  ⚠ SLOW" } else { "" };
            format!("🗂 INDEX REBUILD {dobjects} dobj  ⏱ {ms:.1} ms{flag}")
        }
        DbgEvent::DocSnapshot { reason, dobject_count, undo_depth, redo_depth, index_in_dump, .. } =>
            format!("📷 SNAP[{}] {} dobj, undo={}, redo={}  ({})",
                index_in_dump, dobject_count, undo_depth, redo_depth, reason),
        DbgEvent::CmdRun { raw, parsed_debug, source, elapsed_us } => {
            // Flag the slow ones so they stand out while skimming: 16 ms is one frame
            // at 60 Hz — past that the user SEES the stall.
            let t = match elapsed_us {
                Some(us) if *us >= 16_000 => format!("  ⏱ {:.1} ms ⚠ SLOW", *us as f64 / 1000.0),
                Some(us) if *us >= 1_000 => format!("  ⏱ {:.1} ms", *us as f64 / 1000.0),
                Some(us) => format!("  ⏱ {us} µs"),
                None => String::new(),
            };
            format!("⌨ CMD \"{}\" → {}  [{:?}]{}", raw, parsed_debug, source, t)
        }
        DbgEvent::ZoomOp { cmd, choices, action, before, after } => {
            let delta = if before == after { "  (view UNCHANGED)" } else { "" };
            format!(
                "🔍 ZOOM \"{cmd}\" → {action}{delta}\n        choices: [{choices}]\n        before: {before}\n        after : {after}"
            )
        }
        DbgEvent::CanvasClick { world, hit_dobject, active_tool, active_state, .. } =>
            format!("🖱 CLICK world=({:.3},{:.3})  hit={:?}  tool={}  state={}",
                world.x, world.y, hit_dobject, active_tool, active_state),
        DbgEvent::CanvasPress { world, button, .. } =>
            format!("🖱 PRESS {} @ ({:.3},{:.3})", button, world.x, world.y),
        DbgEvent::CanvasRelease { world, button, drag_px, .. } =>
            format!("🖱 RELEASE {} @ ({:.3},{:.3})  drag={:.1}px", button, world.x, world.y, drag_px),
        DbgEvent::GestureClassification {
            motion_px, motion_dir, egui_clicked, egui_drag_stopped,
            press_world, release_world, hit_at_press, hit_at_release,
            in_select_mode, active_tool, selection_before, selection_after,
            n_before, n_after, app_action_taken, outcome_summary, ..
        } => {
            // The selection is printed TWICE here (before → after), so this line is
            // the single biggest dump amplifier: ~11 KB at 916 selected. In
            // counts-only mode print the SIZES instead.
            let sel = if selection_before.is_empty() && selection_after.is_empty()
                && (*n_before > 0 || *n_after > 0)
            {
                format!("{n_before} → {n_after} dobject(s)")
            } else {
                format!("{selection_before:?} → {selection_after:?}")
            };
            // Multi-line so the timeline reader can SEE the whole story.
            format!(
                "🔍 GESTURE press=({:.2},{:.2}) hit_press={:?}  →  release=({:.2},{:.2}) hit_release={:?}\n         \
                 motion={} px ({})  egui: clicked={} drag_stopped={}  select_mode={}  tool={}\n         \
                 sel: {}  | action: {}\n         \
                 verdict: {}",
                press_world.x, press_world.y, hit_at_press,
                release_world.x, release_world.y, hit_at_release,
                motion_px, motion_dir, egui_clicked, egui_drag_stopped,
                in_select_mode, active_tool,
                sel,
                app_action_taken, outcome_summary)
        }
        DbgEvent::CanvasDrag { from_world, to_world, button, .. } =>
            format!("🖱 DRAG {} ({:.3},{:.3})→({:.3},{:.3})",
                button, from_world.x, from_world.y, to_world.x, to_world.y),
        DbgEvent::SelectChange { basket_before, basket_after, n_before, n_after, cause } => {
            if basket_before.is_empty() && basket_after.is_empty() && (*n_before > 0 || *n_after > 0) {
                // counts-only: the lists were dropped at capture
                format!("✓ SEL {} → {} dobject(s)  ({})", n_before, n_after, cause)
            } else {
                format!("✓ SEL {:?} → {:?}  ({})", basket_before, basket_after, cause)
            }
        }
        DbgEvent::ToolChange { from, to, cause } =>
            format!("🔧 TOOL {} → {}  ({})", from, to, cause),
        DbgEvent::StateChange { state_name, before, after, cause } =>
            format!("🔁 {} {} → {}  ({})", state_name, before, after, cause),
        DbgEvent::DocPush { index, geom_kind, handle, summary } =>
            format!("➕ PUSH #{} ({}) handle={:#x} — {}", index, geom_kind, handle, summary),
        DbgEvent::DocRemove { index, geom_kind, summary } =>
            format!("➖ REMOVE #{} ({}) — {}", index, geom_kind, summary),
        DbgEvent::UndoSnapshotTaken { undo_depth_after, bytes_estimate } =>
            format!("💾 UNDO-SNAP depth={} (~{} bytes)", undo_depth_after, bytes_estimate),
        DbgEvent::UndoFired { from_depth, to_depth } =>
            format!("↶ UNDO {} → {}", from_depth, to_depth),
        DbgEvent::RedoFired { from_depth, to_depth } =>
            format!("↷ REDO {} → {}", from_depth, to_depth),
        DbgEvent::ApplyOp { name, before_dobj_count, after_dobj_count, success, detail } =>
            format!("⚙ {} ok={} dobj {}→{} — {}",
                name, success, before_dobj_count, after_dobj_count, detail),
        DbgEvent::MemoryEvent { name, bytes, elapsed_us } =>
            format!("🧠 {} ~{} bytes in {} µs", name, bytes, elapsed_us),
        DbgEvent::GeometryCapture { label, entries } => {
            let mut s = format!(
                "📐 GEOMETRY CAPTURE — {} ({} entr{})",
                label, entries.len(),
                if entries.len() == 1 { "y" } else { "ies" });
            for e in entries {
                s.push_str("\n         ");
                s.push_str(e);
            }
            s
        }
        DbgEvent::StretchRecord {
            box_min, box_max, base, dest, vector, total_selected, affected,
        } => {
            let mag = (vector.0 * vector.0 + vector.1 * vector.1).sqrt();
            let mut s = format!(
                "✂REC STRETCH  box=({:.3},{:.3})→({:.3},{:.3})  \
                 vec=({:.3},{:.3}) |v|={:.3}  dir=({:.4},{:.4})  \
                 base=({:.3},{:.3})→dest=({:.3},{:.3})  {}/{} changed",
                box_min.0, box_min.1, box_max.0, box_max.1,
                vector.0, vector.1, mag,
                if mag > 1e-9 { vector.0 / mag } else { 0.0 },
                if mag > 1e-9 { vector.1 / mag } else { 0.0 },
                base.0, base.1, dest.0, dest.1,
                affected.len(), total_selected);
            for a in affected {
                s.push_str("\n         ");
                s.push_str(a);
            }
            s
        }
        DbgEvent::WindowToggle { name, opened } =>
            format!("🪟 {} {}", name, if *opened {"OPENED"} else {"CLOSED"}),
        DbgEvent::MenuLayout { label, elements } => {
            let mut s = format!("📐 MENU LAYOUT — {} ({} elements)  [x y w h, screen pts]",
                label, elements.len());
            for e in elements {
                s.push_str("\n         ");
                s.push_str(e);
            }
            s
        }
        DbgEvent::MenuClick { path } =>
            format!("☰ MENU {}", path),
        DbgEvent::KeyEvent { key, modifiers } =>
            format!("⌨ KEY {} {:?}", key, modifiers),
    }
}

// ===========================================================================
//                       PER-FRAME WATCHED STATE
// ===========================================================================
//
// Snapshot of every field the recorder watches. CadApp keeps the
// previous frame's snapshot and diffs against the current one — every
// changed field emits a `StateChange` (or category-specific) event. No
// per-assignment instrumentation needed.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchedState {
    /// Which viewport the user is working in (2D plan / 3D Factory). This is the
    /// signal the modifiers dispatch on, so a dump must show it.
    pub active_view:         String,
    pub tool:                String,
    pub select_mode:         String,
    // The five core modifiers. They were MISSING from the watched set, which is why a
    // dump of a MOVE showed the command being typed and then nothing — the one state
    // that was live was the one state never printed.
    pub move_state:          String,
    pub copy_state:          String,
    pub rotate_state:        String,
    pub scale_state:         String,
    pub mirror_state:        String,
    pub trim_state:          String,
    pub extend_state:        String,
    pub fillet_state:        String,
    pub chamfer_state:       String,
    pub offset_state:        String,
    pub dist_state:          String,
    pub text_draft:          String,
    pub matchprops_state:    String,
    pub align_state:         String,
    pub stretch_state:       String,
    pub break_state:         String,
    pub lengthen_state:      String,
    /// Block / insert POINT-PICK phases. These capture a coordinate on a
    /// single click; if a pick "catches grips" or selects instead, the
    /// transition here vs. the click that fired is the smoking gun.
    pub block_def_state:     String,
    pub insert_state:        String,
    /// Block dialog "Pick ⊕" base-point capture in progress.
    pub block_pick_base:     bool,
    pub grip_drag:           bool,
    // NOTE: there is deliberately NO `selection` field here.
    //
    // It used to hold a `Vec<usize>` CLONE of the selection, rebuilt EVERY FRAME while
    // recording — and no code ever diffed or read it. Selecting a million dobjects
    // meant an 8 MB allocation per frame for a value nobody looked at.
    //
    // It is also redundant: selection changes are captured precisely AT THE SOURCE by
    // `SelectChange { basket_before, basket_after, cause }` — the "✓ SEL [] → [5]"
    // lines — which carry the cause too. Polling for them would be strictly worse
    // information at O(n) cost. Do not add it back.
    /// Queued select-mode follow-up op (erase, move, hatch, …). When
    /// the basket is empty and the user types one of these, the cmd
    /// puts itself here and waits for the user to pick. If you see
    /// `queued_op != None` linger past Enter, the queued op never
    /// fired — almost always the bug shape.
    pub queued_op:           String,
    /// Override for the next window — `Some(true)` = force inside
    /// (user typed `w`), `Some(false)` = force crossing (`c`), `None`
    /// = use direction default. Consumed by the FIRST completing
    /// window. Lingering `Some` past a gesture = the override didn't
    /// fire — diagnostic gold.
    pub armed_window_inside: String,
    /// Captured first corner of a two-click window gesture. While
    /// `Some`, the next click commits the window. `None` after a
    /// successful or aborted gesture.
    pub window_first:        String,
    pub doc_dobjects_len:    usize,
    pub undo_depth:          usize,
    pub redo_depth:          usize,
    /// All Window-state flags concatenated as bits → "open/closed"
    /// map of every palette. One field per Window.
    pub window_flags:        WindowFlags,
    /// Persisted SYSVARs we want to see flip live. Cherry-picked —
    /// not every byte of `env` (would flood the timeline).
    pub sysvar_summary:      String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WindowFlags {
    pub cmd_window:        bool,
    pub layers_window:     bool,
    pub pens_window:       bool,
    pub info_window:       bool,
    pub dobjects_window:   bool,
    pub snap_window:       bool,
    pub trim_debug:        bool,
    pub hatch_debug:       bool,
    pub hatch_dialog:      bool,
    pub hatch_confirm:     bool,
    pub text_style_dialog: bool,
    pub dim_style_dialog:  bool,
    pub dbg_window:        bool,
}

impl WindowFlags {
    /// Pretty-print which windows are open vs closed.
    pub fn summarize(&self) -> String {
        let mut open = Vec::new();
        let mut closed = Vec::new();
        macro_rules! flag {
            ($name:literal, $field:ident) => {
                if self.$field { open.push($name); } else { closed.push($name); }
            };
        }
        flag!("cmd",          cmd_window);
        flag!("layers",       layers_window);
        flag!("pens",         pens_window);
        flag!("info",         info_window);
        flag!("dobjects",     dobjects_window);
        flag!("snap",         snap_window);
        flag!("trim_debug",   trim_debug);
        flag!("hatch_debug",  hatch_debug);
        flag!("hatch_dialog", hatch_dialog);
        flag!("hatch_confirm",hatch_confirm);
        flag!("text_style",   text_style_dialog);
        flag!("dim_style",    dim_style_dialog);
        flag!("dbg_recorder", dbg_window);
        format!("open=[{}] closed=[{}]", open.join(","), closed.join(","))
    }
}

/// Diff two WatchedState snapshots and push one event per changed
/// field. Cause string is `"state poll (frame end)"` so the inspector
/// shows the source of the inference. Returns the number of changes.
pub fn diff_watched(
    rec: &mut DbgRecorder,
    prev: &WatchedState,
    curr: &WatchedState,
    loc: &'static Location<'static>,
) -> usize {
    if !rec.recording { return 0; }
    let mut n = 0;
    macro_rules! diff_field {
        ($name:expr, $field:ident) => {
            if prev.$field != curr.$field {
                rec.push(DbgEvent::StateChange {
                    state_name: $name.to_string(),
                    before:     format!("{:?}", prev.$field),
                    after:      format!("{:?}", curr.$field),
                    cause:      "state poll (frame end)".to_string(),
                }, loc);
                n += 1;
            }
        };
    }
    // Tool gets its own dedicated event variant for easier filtering.
    if prev.tool != curr.tool {
        rec.push(DbgEvent::ToolChange {
            from:  prev.tool.clone(),
            to:    curr.tool.clone(),
            cause: "state poll (frame end)".to_string(),
        }, loc);
        n += 1;
    }
    diff_field!("active_view",       active_view);
    diff_field!("select_mode",       select_mode);
    diff_field!("move_state",        move_state);
    diff_field!("copy_state",        copy_state);
    diff_field!("rotate_state",      rotate_state);
    diff_field!("scale_state",       scale_state);
    diff_field!("mirror_state",      mirror_state);
    diff_field!("trim_state",        trim_state);
    diff_field!("extend_state",      extend_state);
    diff_field!("fillet_state",      fillet_state);
    diff_field!("chamfer_state",     chamfer_state);
    diff_field!("offset_state",      offset_state);
    diff_field!("dist_state",        dist_state);
    diff_field!("text_draft",        text_draft);
    diff_field!("matchprops_state",  matchprops_state);
    diff_field!("align_state",       align_state);
    diff_field!("stretch_state",     stretch_state);
    diff_field!("break_state",       break_state);
    diff_field!("lengthen_state",    lengthen_state);
    diff_field!("block_def_state",   block_def_state);
    diff_field!("insert_state",      insert_state);
    if prev.block_pick_base != curr.block_pick_base {
        rec.push(DbgEvent::StateChange {
            state_name: "block_pick_base".into(),
            before:     prev.block_pick_base.to_string(),
            after:      curr.block_pick_base.to_string(),
            cause:      "state poll (frame end)".into(),
        }, loc);
        n += 1;
    }
    diff_field!("queued_op",           queued_op);
    diff_field!("armed_window_inside", armed_window_inside);
    diff_field!("window_first",        window_first);
    if prev.grip_drag != curr.grip_drag {
        rec.push(DbgEvent::StateChange {
            state_name: "grip_drag".into(),
            before:     prev.grip_drag.to_string(),
            after:      curr.grip_drag.to_string(),
            cause:      "state poll (frame end)".into(),
        }, loc);
        n += 1;
    }
    // Per-window toggle event for any flag flipped.
    macro_rules! window_diff {
        ($name:literal, $field:ident) => {
            if prev.window_flags.$field != curr.window_flags.$field {
                rec.push(DbgEvent::WindowToggle {
                    name:   $name.to_string(),
                    opened: curr.window_flags.$field,
                }, loc);
                n += 1;
            }
        };
    }
    window_diff!("cmd",           cmd_window);
    window_diff!("layers",        layers_window);
    window_diff!("pens",          pens_window);
    window_diff!("info",          info_window);
    window_diff!("dobjects",      dobjects_window);
    window_diff!("snap",          snap_window);
    window_diff!("trim_debug",    trim_debug);
    window_diff!("hatch_debug",   hatch_debug);
    window_diff!("hatch_dialog",  hatch_dialog);
    window_diff!("hatch_confirm", hatch_confirm);
    window_diff!("text_style",    text_style_dialog);
    window_diff!("dbg_recorder",  dbg_window);
    diff_field!("sysvar_summary", sysvar_summary);
    n
}

// ===========================================================================
//                              MACRO
// ===========================================================================

/// Push an event with the CALLER's `Location` automatically attached.
/// Cheap no-op when not recording.
#[macro_export]
macro_rules! dbg_event {
    ($app:expr, $event:expr) => {{
        if $app.dbg.recording {
            $app.dbg.push($event, std::panic::Location::caller());
        }
    }};
}

/// Take a Document snapshot tagged with `reason`. Forwards undo/redo
/// depths and the call site.
#[macro_export]
macro_rules! dbg_snapshot {
    ($app:expr, $reason:expr) => {{
        if $app.dbg.recording {
            $app.dbg.take_snapshot(
                &$app.doc,
                $reason,
                $app.undo_stack.len(),
                $app.redo_stack.len(),
                std::panic::Location::caller(),
            );
        }
    }};
}
