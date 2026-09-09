use super::*;

// ============ the per-frame shell: egui App impl ============
// `impl eframe::App` (window chrome, mode tabs, toolbars, floating windows,
// panels, canvas) — the frame layout every workspace shares. Child module of
// `app`; trait methods need no visibility (eframe drives them), and the body
// calls CadApp privates via `use super::*`.
impl eframe::App for CadApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // WHEN THIS FRAME'S WORK BEGAN — see `CadApp::frame_start`. Taken first, so nothing else
        // can be charged to it.
        self.frame_start = Some(std::time::Instant::now());
        self.frame_marks.clear();

        // The app's GL context (eframe runs the glow renderer) — the GPU path tracer draws on it.
        self.pt_gl = _frame.gl().cloned();

        // Wake regularly, but do NOT spin. This used to be an unconditional `request_repaint()`,
        // which pinned the app at full framerate forever: a 2 M-triangle scene re-rendered 60 times
        // a second whether or not anything had changed, holding a CPU core and the GPU busy the
        // whole time the window was open. That is felt as the whole machine being slow, and the
        // recorder shows it as every frame logged at ~17 ms and flagged SLOW from startup.
        //
        // egui repaints immediately on any input by itself, and everything that genuinely animates
        // — a drag, the path tracer, a hatch build, the progress spinner — already asks for its own
        // repaint at its own site. So a 5 Hz heartbeat keeps the "never frozen" guarantee while
        // letting an idle viewport cost nothing.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        self.trim_debug_frame = self.trim_debug_frame.wrapping_add(1);

        // WHO OWNS THE KEYBOARD, taken BEFORE any panel is drawn.
        //
        // Enter is read from the global context further down and run through the command cascade,
        // which ends in "repeat the last command". That handler never asked whether anyone was
        // typing — so entering a wall height in a menu, or any other number in any other field,
        // committed the value AND re-ran the previous command. With `clear` behind it that wipes
        // the drawing, which is what "everything in the window disappears" was.
        //
        // Read here rather than at the cascade because a field that commits on Enter SURRENDERS
        // FOCUS as it is drawn; by the time the cascade runs, `focused()` is already None and the
        // guard would pass exactly when it is needed. Frame start is the last moment the answer is
        // still the truth about the keystroke.
        self.focus_at_frame_start = ctx.memory(|m| m.focused());

        // THE MODE TAB BAR — one full-window workspace at a time. Enforced
        // here, at frame start, before any panel draws: an open sketch claims
        // the drafting workspace, a view opened from inside another workspace
        // brings its tab forward, and a closed workspace view falls back to
        // 2D drafting. `enforce_mode_workspaces` also fires the one-time
        // "3D Factory just opened" notice (its rising edge is read off
        // `factory_was_open` there), so nothing else needs to.
        self.enforce_mode_workspaces();

        // First real frame of a fresh app: frame the demo plan once. The
        // canvas rect exists from the previous frame (else we would fit to
        // the 800×600 fallback and misframe the real window), the user has
        // not zoomed anywhere yet (view history empty), and no file is open
        // (open does its own fit). After this the view is the user's.
        self.maybe_frame_demo_plan();

        // Install the global design-token Visuals so every default-styled widget
        // (menus, dialogs, buttons, checkboxes, fields) reads the one teal-navy
        // theme. Also fixes square menu corners (menu_rounding = ZERO).
        crate::theme::apply(ctx);

        // ---- Auto-load the VILLA test model on startup — OPT-IN ---------------------------------
        // The villa is a 134 MB glTF at 2.01 M triangles: measured 715 ms to parse in release and
        // **6.5 s in debug**, and that is only the parse — the same first frame then builds and
        // uploads ~240 MB of vertex buffers for it. Doing that before the window has drawn anything
        // is exactly the "why does the app take so long to open now?" the autoload caused. It is a
        // RENDERING TEST FIXTURE, not something a user opening the app wants, so it now needs
        // `SIMLUX_VILLA=1`. ▼ FBC scene import → "🏠 Reload the villa scene" loads it on demand,
        // with the busy overlay up, which is the honest way to spend six seconds.
        //
        // One-shot, and only into a FRESH session (empty scene) so opening a project later isn't
        // polluted. A moved or absent model just skips silently.
        if !self.villa_autoloaded {
            self.villa_autoloaded = true;
            let want = std::env::var("SIMLUX_VILLA")
                .map(|v| v != "0")
                .unwrap_or(false);
            let empty = self.factory.furniture.is_empty() && self.factory.model.features.is_empty();
            if want && empty {
                // Exactly the path ▼ FBC scene import runs, so the autoload and a manual import
                // cannot drift apart in how they scale or place a scene.
                self.import_scene_file(VILLA_SCENE);
            }
        }

        // ---- SIMLUX_REPAIR=<drawing> — open it and repair its openings, once ---------------------
        // Same shape as the villa autoload above, and for a sharper reason: this repair was twice
        // run against a stale binary, where it keyed on a signature the failing cuts no longer
        // carried and so reported "no shallow cuts found" — indistinguishable from success. Driving
        // it from the binary that was just built makes "which build is this?" unanswerable-wrong.
        //
        // It does not save. The result goes to the history panel AND to stderr, so it can be read
        // from the console without touching the file.
        match self.startup_repair {
            0 => match std::env::var("SIMLUX_REPAIR") {
                Ok(p) if !p.is_empty() => {
                    self.startup_repair = 1;
                    eprintln!("[repair] opening {p}");
                    self.do_open(&p);
                }
                _ => self.startup_repair = 3,
            },
            // The open runs on the busy worker over several frames; wait for it to land. A failed
            // open clears `busy` too, and the report below then simply finds nothing to do.
            1 if self.busy.is_none() => {
                self.startup_repair = 2;
                let before = self.factory.model.eval().tri_count();
                self.run_command("repaircuts");
                self.run_command("diag");
                let after = self.factory.model.eval().tri_count();
                for l in self
                    .history
                    .iter()
                    .rev()
                    .take(40)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    eprintln!("[repair] {l}");
                }
                eprintln!("[repair] solid {before} → {after} triangles");
                eprintln!("[repair] NOT saved — review it, then Ctrl+S if you are happy.");
            }
            _ => {}
        }

        // ---- Loading / saving overlay ----------------------------------------------
        // The file work runs on a BACKGROUND worker thread so the UI thread never blocks —
        // that is the whole point: a blocked UI thread stops pumping Windows messages and the
        // OS stamps "(Not Responding)" on the window. The state machine over three-plus frames:
        //   frame 1: paint the overlay (so the window is on screen before any work starts)
        //   frame 2: spawn the worker (for a save, clone the doc + build the config first)
        //   frame 3+: poll the worker each frame while the overlay keeps animating
        // On completion we install the result on the main thread and self-calibrate the estimate.
        if self.busy.is_some() {
            self.render_busy_overlay(ctx); // always paints + request_repaint → smooth spinner
            let (painted, spawned) = {
                let b = self.busy.as_ref().unwrap();
                (b.painted, b.rx.is_some())
            };
            if !painted {
                self.busy.as_mut().unwrap().painted = true;
            } else if !spawned {
                self.spawn_busy_worker();
            } else {
                use std::sync::mpsc::TryRecvError;
                let recv = self.busy.as_ref().unwrap().rx.as_ref().unwrap().try_recv();
                match recv {
                    Ok(msg) => {
                        let b = self.busy.take().unwrap();
                        let ms = b.started.elapsed().as_millis() as u64;
                        match msg {
                            BusyMsg::Loaded(res) => {
                                match res {
                                    Ok(payload) => self.apply_loaded(&b.path, payload),
                                    Err(e) => {
                                        self.history.push(format!("  ! open '{}': {}", b.path, e))
                                    }
                                }
                                self.last_load_ms = ms.clamp(100, 120_000);
                            }
                            BusyMsg::Saved(res) => {
                                match res {
                                    Ok(payload) => self.apply_saved(&b.path, payload),
                                    Err(e) => {
                                        self.history.push(format!("  ! save '{}': {}", b.path, e))
                                    }
                                }
                                self.last_save_ms = ms.clamp(100, 120_000);
                            }
                        }
                    }
                    // Worker still running — keep the overlay up and try again next frame.
                    Err(TryRecvError::Empty) => {}
                    // Worker thread died without sending (panic) — surface it, don't hang.
                    Err(TryRecvError::Disconnected) => {
                        let b = self.busy.take().unwrap();
                        let verb = match b.kind {
                            BusyKind::Load => "open",
                            BusyKind::Save => "save",
                        };
                        self.history.push(format!(
                            "  ! {} '{}': worker failed (no result)",
                            verb, b.path
                        ));
                    }
                }
            }
        }

        // ---- Close confirmation (unsaved-changes guard) -----------------------------
        // Intercept a window/menu close: if the drawing has unsaved edits, veto the close and
        // ask Save / Don't Save / Cancel — so an accidental X can't discard work.
        if self.pending_close {
            // A prior choice authorised the close; do it now (unsaved already cleared).
            self.pending_close = false;
            self.unsaved = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) {
            if self.unsaved && self.busy.is_none() && !self.close_confirm {
                self.close_confirm = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            // else: nothing unsaved (or a save is finishing) → let the close proceed.
        }
        if self.close_confirm {
            self.render_close_confirm(ctx);
        }
        // The room-details modal (2D ROOMS ▸ "Make room (from selected
        // outline)") — bails when no form is open.
        self.render_room_form(ctx);

        // Autosave: silent background re-save of the current file a few minutes after an edit.
        self.tick_autosave();
        // After-calculation embed save (embedded-mode drawings) — drain + re-try.
        self.tick_results_save();
        // Calculator-variables sidecar patch (worker) — drain its result.
        self.tick_calc_sidecar();

        // Menu-layout recorder: arm geometry capture for THIS whole frame and
        // reset the shared buffer BEFORE any panel/menu renders. Every menu that
        // calls pp_capture / pp_cap_ui records into it; the dump runs at the very
        // end of update() so it captures ANY open menu (rails, dropdowns, popups,
        // Properties) — not just the Properties panel.
        let cap_on = self.props_layout_capture || self.ui_inspect;
        ctx.data_mut(|d| {
            d.insert_temp(egui::Id::new("pp_cap_on"), cap_on);
            if cap_on {
                d.insert_temp(
                    egui::Id::new("pp_cap_buf"),
                    Vec::<(String, egui::Rect)>::new(),
                );
            }
        });

        // Drain any in-flight hatch trace worker. If the worker
        // finished this frame, this materialises the result (Success
        // → push polylines + hatch dobject; Failure → fall back to
        // cheap path; Cancelled → log + clear prompt). No-op when
        // no worker is running. Runs FIRST so the rest of the frame
        // sees the updated doc state.
        self.poll_hatch_worker();
        // WP-SCRIPT: drain the Python worker's output into the command history.
        self.poll_script_engine(ctx);

        // FPS — exponential moving average so the number doesn't jitter
        let dt = ctx.input(|i| i.stable_dt);
        if dt > 0.0 {
            let instant = 1.0 / dt;
            self.fps_smooth = if self.fps_smooth == 0.0 {
                instant
            } else {
                self.fps_smooth * 0.9 + instant * 0.1
            };
        }

        // global drafting-mode toggles (AutoCAD F-keys). Fire on key-press
        // anywhere — they're modeless and don't interfere with text input
        // since F-keys aren't typeable characters. Each one persists via
        // env.save() so the state survives restart.
        if ctx.input(|i| i.key_pressed(egui::Key::F7)) {
            self.env.GrdEnb = !self.env.GrdEnb;
            let _ = self.env.save();
            self.history.push(format!(
                "  GRID {}",
                if self.env.GrdEnb { "on" } else { "off" }
            ));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F8)) {
            // CARD — cardinal-directions drafting lock (cursor
            // constrained to ONLY horizontal or vertical from the
            // anchor).
            self.env.CrdEnb = !self.env.CrdEnb;
            let _ = self.env.save();
            self.history.push(format!(
                "  CARD {}",
                if self.env.CrdEnb { "on" } else { "off" }
            ));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F9)) {
            self.env.GrdSnp = !self.env.GrdSnp;
            let _ = self.env.save();
            self.history.push(format!(
                "  SNAP {}",
                if self.env.GrdSnp { "on" } else { "off" }
            ));
        }

        // Ctrl+Z — Undo. Consumed so the command-line TextEdit doesn't also undo
        // its own typed text; Ctrl+Z always undoes the DRAWING (AutoCAD-style).
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
            self.run_command("undo");
        }

        // global Esc: cancel any in-progress draw or pick / intersect / select mode
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            // The room-details question OWNS Esc while it is open: it cancels
            // the question, and nothing under the modal may also cancel — an
            // Esc that is answering a dialog is not also abandoning a draft.
            if self.room_form.is_some() {
                self.room_form = None;
                self.room_form_focus_name = false;
                return;
            }
            // 3D placement: Esc cancels a primitive waiting for its point.
            if self.factory.place_pending.is_some() {
                self.factory.place_pending = None;
                self.factory.status.clear();
                self.factory_note("3D place: cancelled".into());
                return;
            }
            // The placement question, abandoned. Without this the prompt vanished but
            // `place_prompt_open` stayed true, and the next word typed was eaten as a bad answer
            // to a question no longer on screen.
            if self.place_prompt_open || self.place_coord_prompt {
                self.place_prompt_open = false;
                self.place_coord_prompt = false;
                self.clear_prompt();
                self.history.push("  place: cancelled".into());
                return;
            }
            // An object waiting to be told where it goes: Esc stops the waiting and LEAVES IT
            // WHERE IT IS. It is already a real object — Esc means "here is fine", not "throw it
            // away", which is why nothing is undone here.
            if self.factory.awaiting_place.is_some() {
                self.factory.awaiting_place = None;
                self.factory.status = "Left where it landed — Ctrl+Z to remove it".into();
                self.factory_note("place: left in place".into());
                return;
            }
            // 3D zoom: Esc exits any zoom mode, before the 2D cancel-everything below.
            if self.factory.zoom_mode != crate::factory::ZoomMode::Off {
                self.factory.zoom_mode = crate::factory::ZoomMode::Off;
                self.factory.zoom_drag = None;
                self.factory.zoom_cur = None;
                self.factory.zoom_rt_before = None;
                self.factory.status.clear();
                self.factory_note("3D zoom: exited".into());
                return;
            }
            // PLINE/SPLINE: Esc removes ONLY the last placed vertex (the last
            // segment), never the whole run. Whatever is already drawn stays
            // live and commits when the command ends/interrupts (see
            // `commit_active_draw`). Esc leaves the tool only once no vertices
            // remain. This is the one gesture that does NOT commit.
            if matches!(self.tool, Tool::Polyline | Tool::Spline) && !self.pending.is_empty() {
                self.pending.pop();
                if self.tool == Tool::Polyline {
                    self.pending_bulges.pop();
                    if !self.pending_widths.is_empty() {
                        self.pending_widths.pop();
                    }
                    self.pline_arc_sub = PlineArcSub::Normal;
                    self.pline_dir_override = None;
                }
                if self.pending.is_empty() {
                    self.pline_mode = PlineMode::Line;
                    self.pline_width_cap = PlineWidthCap::None;
                    self.tool = Tool::None;
                    self.clear_prompt();
                    self.history.push("  draw ended (no vertices left)".into());
                } else if self.tool == Tool::Polyline {
                    self.update_pline_prompt();
                    self.history.push("  ⎌ removed last vertex".into());
                } else {
                    self.set_prompt(current_hint(self.tool, self.arc_method, self.pending.len()));
                    self.history.push("  ⎌ removed last control point".into());
                }
                return;
            }
            self.pending.clear();
            self.pending_bulges.clear();
            self.pending_widths.clear();
            self.spline_width_wait = false; // spline width entry (value is sticky)
            self.spline_degree_override = None;
            self.pline_mode = PlineMode::Line;
            self.pline_arc_sub = PlineArcSub::Normal;
            self.pline_dir_override = None;
            self.pline_width_cap = PlineWidthCap::None;
            // pline_next_width persists (sticky default) across cancel too.
            self.wall_waiting_thickness = false;
            if self.block_def_state != BlockDefState::Off {
                self.block_def_state = BlockDefState::Off;
                self.history.push("  block creation cancelled".into());
            }
            if self.insert_state != InsertState::Off {
                self.insert_state = InsertState::Off;
                self.history.push("  insert cancelled".into());
            }
            self.flow_cancel(); // cancel an active prompt-driven flow (CIRCLE)
            self.zoom_cancel(); // cancel an active ZOOM sub-flow
            self.pedit_exit(); // cancel an active PEDIT sub-flow
            self.tool = Tool::None;
            self.picking_source = false;
            // Set the cooperative-cancellation flag for any long-
            // running op currently in flight (or queued — flag is
            // reset at op start). When async/threading lands, this
            // is the signal the worker thread reads to bail out.
            self.op_cancel.store(true, Ordering::Relaxed);
            // A1: DROP the hatch-trace worker's receiver too, so a
            // completed-but-undrained Success can't materialize after Esc
            // (Background-Ops I-3). The op_cancel above only speeds the
            // worker's exit; dropping the receiver is what makes the cancel
            // correct.
            self.cancel_hatch_worker();
            // WP-SCRIPT: Esc interrupts a running Python script (Background-Ops
            // I-3). Cooperative — raises KeyboardInterrupt into the worker.
            if let Some(s) = &self.script {
                if s.is_busy() {
                    s.cancel();
                    self.history.push("  python: cancel requested".into());
                }
            }
            // WP-SCRIPT: Esc disarms an armed script-parameter pick
            // (the dialog's "pick on canvas").
            if self.script_param_pick.is_some() {
                self.script_param_pick = None;
                self.history.push("  python: pick cancelled".into());
            }
            self.layout_selection.clear();
            self.hatch_pick_point_armed = false;
            self.hatch_pick_point_session = None;
            self.pending_hatch_pattern = (None, 1.0, 0.0);
            self.hatch_dialog_open = false;
            self.block_dialog = None;
            // Abandon any parked Block-dialog round-trip (Select / Pick).
            self.block_dialog_stash = None;
            self.block_dialog_pick_base = false;
            // Close / cancel the Insert dialog + its pick + a pending placement.
            self.insert_dialog = None;
            self.insert_dialog_pick = false;
            self.insert_param_pick = None;
            self.pending_insert = None;
            self.insert_live = None;
            // Drop any captured stretch crossing box (e.g. Esc mid-select).
            self.stretch_window_box = None;
            // Dismiss the block-diff parametric-point highlight + cancel a
            // compare pick / the Set-parameters dialog in progress.
            self.blockdiff_overlay = None;
            self.param_name_dialog = None;
            if self.insert_param_prompt.is_some() {
                self.insert_param_prompt = None;
                self.history.push("  insert cancelled".into());
            }
            // Cancel a Block Task Recorder: delete the sandbox, restore the
            // block's own instances, discard recordings.
            self.btr_awaiting_name = false;
            if let Some(rec) = self.block_task_rec.take() {
                self.doc
                    .dobjects
                    .retain(|d| !rec.temp_handles.contains(&d.handle));
                for d in &rec.removed_instances {
                    self.doc.push(d.clone());
                }
                self.index_dirty = true;
                self.touch_view();
                self.history
                    .push("  Block Task Recorder cancelled (sandbox discarded)".into());
            }
            if self.blockdiff_pick != BlockDiffPick::Off {
                self.blockdiff_pick = BlockDiffPick::Off;
                self.history.push("  compare cancelled".into());
            }
            if let Some(d) = self.file_dialog.take() {
                self.file_dialog_dir = Some(d.dir);
            }
            self.intersect_pending_click = false;
            self.intersect_view_pending = false;
            self.snap_override = None;
            // Item 3: Esc clears the command line input AND any current
            // pretext shown above it. The 2-stage-Enter counter resets too.
            self.cmd.clear();
            self.clear_prompt();
            if self.select_mode != SelectMode::Off {
                self.cancel_selection();
            }
            if self.move_state != MoveState::Off {
                self.move_state = MoveState::Off;
                self.history.push("  move cancelled".into());
            }
            if self.copy_state != CopyState::Off {
                self.copy_state = CopyState::Off;
                self.history.push("  copy cancelled".into());
            }
            if self.paste_state != PasteState::Off {
                self.paste_state = PasteState::Off;
                self.history.push("  paste cancelled".into());
            }
            if self.rotate_state != RotateState::Off {
                self.rotate_state = RotateState::Off;
                self.rotate_copy = false;
                self.history.push("  rotate cancelled".into());
            }
            if self.scale_state != ScaleState::Off {
                self.scale_state = ScaleState::Off;
                self.scale_copy = false;
                self.history.push("  scale cancelled".into());
            }
            if self.mirror_state != MirrorState::Off {
                self.mirror_state = MirrorState::Off;
                self.history.push("  mirror cancelled".into());
            }
            if self.matchprops_state != MatchPropsState::Off {
                self.matchprops_state = MatchPropsState::Off;
                self.history.push("  matchprop cancelled".into());
            }
            if self.offset_state != OffsetState::Off {
                self.offset_state = OffsetState::Off;
                self.offset_erase = false;
                self.offset_layer_src = false;
                self.offset_applied_count = 0;
                self.history.push("  offset cancelled".into());
            }
            if self.dist_state != DistState::Off {
                self.dist_state = DistState::Off;
                self.history.push("  dist cancelled".into());
            }
            if self.area_state.is_some() {
                self.area_state = None;
                self.history.push("  area cancelled".into());
            }
            if self.xline_state != XlineState::Off {
                self.xline_state = XlineState::Off;
                self.history.push("  xline cancelled".into());
            }
            if self.centermark_state != CenterMarkState::Off {
                self.centermark_state = CenterMarkState::Off;
                self.history.push("  centermark cancelled".into());
            }
            if self.boundary_state != BoundaryState::Off {
                self.boundary_state = BoundaryState::Off;
                self.history.push("  boundary cancelled".into());
            }
            if self.xref_pending.is_some() {
                self.xref_pending = None;
                self.clear_prompt();
                self.history.push("  xref attach cancelled".into());
            }
            if self.attr_def_flow != AttrDefFlow::Off {
                self.attr_def_flow = AttrDefFlow::Off;
                self.clear_prompt();
                self.history.push("  attdef cancelled".into());
            }
            if self.attedit_state != AttEditState::Off {
                self.attedit_state = AttEditState::Off;
                self.clear_prompt();
                self.history.push("  attedit cancelled".into());
            }
            if self.array_pick_center {
                self.array_pick_center = false;
                self.clear_prompt();
                self.history
                    .push("  array (polar) centre pick cancelled".into());
            }
            if self.array_pick_path {
                self.array_pick_path = false;
                self.clear_prompt();
                self.history.push("  array (path) pick cancelled".into());
            }
            if self.ray_state != RayState::Off {
                self.ray_state = RayState::Off;
                self.history.push("  ray cancelled".into());
            }
            if self.donut_state != DonutState::Off {
                self.donut_state = DonutState::Off;
                self.history.push("  donut cancelled".into());
            }
            if self.wipeout_state != WipeoutState::Off {
                self.wipeout_state = WipeoutState::Off;
                self.history.push("  wipeout cancelled".into());
            }
            if self.layer_pick != LayerPickState::Off {
                self.layer_pick = LayerPickState::Off;
                self.history.push("  layer pick cancelled".into());
            }
            if self.ptdist_state != PtDistribState::Off {
                self.ptdist_state = PtDistribState::Off;
                self.history.push("  divide/measure cancelled".into());
            }
            if self.lengthen_state != LengthenState::Off {
                self.lengthen_state = LengthenState::Off;
                self.history.push("  lengthen cancelled".into());
            }
            if self.break_state != BreakState::Off {
                self.break_state = BreakState::Off;
                self.history.push("  break cancelled".into());
            }
            if self.align_state != AlignState::Off {
                self.align_state = AlignState::Off;
                self.history.push("  align cancelled".into());
            }
            if self.var_set_pending.is_some() {
                self.var_set_pending = None;
                self.clear_prompt();
                self.history.push("  setvar cancelled".into());
            }
            if self.fillet_state != FilletState::Off {
                self.fillet_state = FilletState::Off;
                self.fillet_multiple = false;
                self.fillet_waiting_radius = false;
                self.fillet_poly_all = false;
                self.history.push("  fillet cancelled".into());
            }
            if self.chamfer_state != ChamferState::Off {
                self.chamfer_state = ChamferState::Off;
                self.chamfer_multiple = false;
                self.chamfer_poly_all = false;
                self.chamfer_dist_wait = ChamferDistWait::Off;
                self.history.push("  chamfer cancelled".into());
            }
            if self.dim_draft != DimDraftState::Off {
                self.dim_draft = DimDraftState::Off;
                self.history.push("  dim cancelled".into());
            }
            if self.grip_drag.is_some() {
                self.grip_drag = None;
                self.grip_drag_peers.clear();
                self.history.push("  grip drag cancelled".into());
            }
            if self.text_draft != TextDraftState::Off {
                self.text_draft = TextDraftState::Off;
                self.pending_text = None;
                self.text_waiting_height = false;
                self.tool = Tool::None;
                self.history.push("  text cancelled".into());
            }
            if self.stretch_state != StretchState::Off {
                self.stretch_state = StretchState::Off;
                self.stretch_window_box = None;
                self.history.push("  stretch cancelled".into());
            }
            // Trim / extend cancel — also restore the stashed main selection
            // if we're still in the cutting/boundary select phase.
            let trim_running = !matches!(self.trim_state, TrimState::Off);
            let extend_running = !matches!(self.extend_state, ExtendState::Off);
            if trim_running {
                self.trim_dbg("=== TRIM session END (Esc cancel) ===");
                self.trim_state = TrimState::Off;
                self.history.push("  trim cancelled".into());
            }
            if extend_running {
                self.trim_dbg("=== EXTEND session END (Esc cancel) ===");
                self.extend_state = ExtendState::Off;
                self.history.push("  extend cancelled".into());
            }
            if trim_running || extend_running {
                self.fence_armed = false;
                self.fence_first = None;
                // Esc clears EVERYTHING — including any selection that was
                // stashed when the op began. Restoring it would resurrect
                // dashed-gray ghosts of items the user just cancelled
                // (bug from screenshot 2026-06-02). pre_op_selection is
                // only restored on SUCCESSFUL finalise, never on cancel.
                self.pre_op_selection.clear();
                self.selection.clear();
            }
            // Esc always returns to a clean slate (AutoCAD convention): drop
            // any pickfirst selection + half-started window, and reclaim the
            // command line so the next typed command lands immediately.
            self.selection.clear();
            self.selected = None;
            self.window_first = None;
            self.refocus_cmd = true;
        }

        // Delete key — erase the current pickfirst selection directly, without
        // routing through the command line. This is focus-independent, so it
        // works even on the frame right after a drag-select where a typed `e`
        // could be dropped by egui's end-of-frame focus grant. Guarded so it
        // never fires while editing text or running another command/flow.
        if ctx.input(|i| i.key_pressed(egui::Key::Delete))
            && !self.typing_in_a_field()
            && !self.selection.is_empty()
            && self.layer_rename.is_none()
            && self.cmd.trim().is_empty()
            && self.cmd_flow.is_none()
            && self.zoom_state == ZoomState::Off
            && self.tool == Tool::None
            && self.select_mode == SelectMode::Off
            && self.text_draft == TextDraftState::Off
            && !self.text_waiting_height
        {
            self.run_command("erase");
        }
        // Delete in the 3D Factory — same key, dispatched on the active view (rule 4:
        // Delete is Delete). Independent of the 2D branch above: in 3D the 2D `selection`
        // is empty, so that branch does not fire.
        if ctx.input(|i| i.key_pressed(egui::Key::Delete))
            && !self.typing_in_a_field()
            && self.active_view == ActiveView::ThreeD
            && self.factory.has_any_selection()
            && self.cmd.trim().is_empty()
            && self.cmd_flow.is_none()
            && self.factory.gizmo_drag.is_none()
            && self.factory.wall_drag.is_none()
        {
            self.factory_delete_selection();
        }

        // Ctrl+C / Ctrl+V — copy / paste selected dobjects. IMPORTANT: eframe
        // turns Ctrl+C/X/V into `Event::Copy` / `Event::Cut` / `Event::Paste`
        // (NOT Key presses), so we must match those events — `key_pressed(C)`
        // is never true for Ctrl+C. We run BEFORE the command-line widget and
        // CONSUME the events so the focused (empty) command line doesn't also
        // do a text copy/paste. Skipped while editing text so fields keep
        // normal clipboard behavior.
        {
            let editing_text = self.layer_rename.is_some()
                || !self.cmd.trim().is_empty()
                || self.text_draft != TextDraftState::Off
                || self.text_waiting_height;
            // When the 3D FACTORY view is active, Ctrl+C/V belong to the 3D copy/paste
            // (handled in render_factory_panel) — this 2D handler must NOT hijack them.
            let three_d_active = self.factory.open && self.active_view == ActiveView::ThreeD;
            if !editing_text && !three_d_active {
                let (copy, paste, select_all, deselect_all) = ctx.input_mut(|i| {
                    // Key fallback in case the platform delivers raw keys.
                    let mut copy = i.modifiers.command && i.key_pressed(egui::Key::C);
                    let mut paste = i.modifiers.command && i.key_pressed(egui::Key::V);
                    // Edit shortcuts (MENU_DROPDOWN §1) — app-layer input, NOT parser
                    // words: Select All = Shift+A, Deselect All = Ctrl/Cmd+D. Verified
                    // conflict-free (no other A/D binding).
                    let select_all =
                        i.modifiers.shift && !i.modifiers.command && i.key_pressed(egui::Key::A);
                    let deselect_all = i.modifiers.command && i.key_pressed(egui::Key::D);
                    // Primary path: eframe's clipboard events (Ctrl+C/X/V).
                    i.events.retain(|e| match e {
                        egui::Event::Copy | egui::Event::Cut => {
                            copy = true;
                            false
                        }
                        egui::Event::Paste(_) => {
                            paste = true;
                            false
                        }
                        // Drop the "A" text event so Shift+A selects (not types "A").
                        egui::Event::Text(t) if select_all && t == "A" => false,
                        _ => true,
                    });
                    (copy, paste, select_all, deselect_all)
                });
                if copy {
                    self.copy_selection();
                    // Mirror to the OS clipboard so a later Ctrl+V reliably
                    // emits Event::Paste (eframe may skip it for an empty clip).
                    ctx.copy_text(format!(
                        "RUST-AutoRASM: {} object(s) on clipboard",
                        self.clipboard_dobjects.len()
                    ));
                }
                if paste {
                    self.start_paste();
                }
                // Ctrl+G — group the current selection (G is a normal key, not
                // a clipboard event, so plain key detection works here).
                if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::G)) {
                    self.group_selection();
                }
                // Shift+A / Ctrl+D — select-all / deselect-all (mirror the Edit menu).
                if select_all {
                    self.run_command("select");
                    self.run_command("all");
                }
                if deselect_all {
                    self.selection.clear();
                    self.selected = None;
                }
            }
        }

        // Enter (when the command line is empty) finalises an in-progress
        // selection — this is the LibreCAD / AutoCAD convention. The cmd
        // box's own Enter handler only fires when the text isn't empty, so
        // there's no double-handling.
        // ONE Enter press = ONE state transition per frame. See memo
        // `feedback_rust_cad_user_terminates_sessions` — the program
        // never auto-terminates editing sessions; only the user does,
        // and a single Enter must not chain through multiple phases.
        // A field other than the command line owns the keyboard ⇒ this Enter is ITS Enter.
        //
        // Without this the cascade below also ran, and its last branch repeats the previous
        // command — so committing a value in any numeric field re-ran whatever was last typed.
        // With `clear` or `erase` behind it that empties the drawing, and the only visible effect
        // is that everything vanishes the moment a number is confirmed.
        //
        // The room-details modal owns Enter the same way: an idle Enter (or
        // Space) under it must not repeat the last command behind the dialog —
        // eat the key so nothing below can see it.
        if self.room_form.is_some() && !self.typing_in_a_field() {
            let _ = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let _ = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
        }
        let typing_elsewhere = self.typing_in_a_field();
        let enter_now = !typing_elsewhere && ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let space_now = !typing_elsewhere && ctx.input(|i| i.key_pressed(egui::Key::Space));
        let cmd_is_empty = self.cmd.trim().is_empty();
        // Same gate as the cmd-line space-submit: while typing a text
        // body (or entering text height), Space is a LITERAL space —
        // never Enter. Without this gate, the empty-cmd Space trigger
        // would cancel the draft on the very first space character.
        let in_text_body = matches!(self.text_draft, TextDraftState::WaitingForString(_))
            || self.text_waiting_height;
        // Item 4 — Space on a truly empty cmd line acts like Enter for the
        // repeat-last + 2-stage cancel logic below. We let the TextEdit
        // also see the space (harmless: cmd stays "trim-empty"), then
        // strip any leading whitespace at the end of this block.
        // A 3D gather owns Enter (step 3 → step 4), exactly as the 2D
        // `finalise_selection` dispatches its `queued_op`.
        if (enter_now || (space_now && cmd_is_empty && !in_text_body))
            && self.active_view == ActiveView::ThreeD
            && self.factory.queued.is_some()
            && self.factory_confirm_gather()
        {
            if space_now {
                self.cmd.clear();
            }
            return;
        }
        let trigger = enter_now || (space_now && cmd_is_empty && !in_text_body);
        // The placement prompt's `<default>`. AutoCAD's bare Enter accepts what is in the angle
        // brackets, and the command-line TextEdit never forwards an empty line to the dispatcher —
        // so the default has to be taken here or the brackets would be a lie.
        if trigger && cmd_is_empty && self.place_coord_prompt {
            // Bare Enter keeps the coordinate already set — the <default> in the angle brackets.
            self.place_coord_prompt = false;
            let msg = self.place_offset_summary();
            self.history.push(format!("  {msg}"));
            self.factory.status = msg;
            self.clear_prompt();
            if space_now {
                self.cmd.clear();
            }
            return;
        }
        if trigger && cmd_is_empty && self.place_prompt_open {
            self.place_prompt_open = false;
            let m = self.factory.place_mode;
            // Enter on <offset> goes straight to the coordinate question, exactly as typing the
            // word does. A default that takes a different route from the answer it stands for is a
            // trap, not a shortcut.
            if m == crate::factory::PlaceMode::Offset {
                self.set_place_coord_prompt();
                if space_now {
                    self.cmd.clear();
                }
                return;
            }
            self.history.push(format!("  {}", m.label()));
            self.factory.status = m.hint().into();
            self.clear_prompt();
            if space_now {
                self.cmd.clear();
            }
            return;
        }
        // Persistent hatch pick-point — Enter ends the session.
        // Sits at the top of the cascade so it consumes Enter before
        // any of the other handlers (which might re-run the last
        // command, etc.).
        if trigger && cmd_is_empty && self.hatch_pick_point_armed {
            self.hatch_pick_point_armed = false;
            self.hatch_pick_point_session = None;
            self.pending_hatch_pattern = (None, 1.0, 0.0);
            self.clear_prompt();
            self.hatch_dbg("pick-point session ended via Enter");
            self.history.push("  hatch pick-point session ended".into());
            if space_now {
                self.cmd.clear();
            }
            return;
        }
        // Block Task Recorder — an empty Enter ENDS the session and opens the
        // "Set parameters on block" dialog. Two cases, both meaning "I'm done":
        //   (a) we're waiting to NAME the just-recorded stretch, or
        //   (b) we're sitting in a stretch-select with an EMPTY basket
        //       (nothing more to stretch).
        // A NON-empty basket still finalises normally (→ base/dest), so the
        // user can keep adding stretches. Sits above the generic select-mode
        // "please make a selection" 2-stage cancel so it wins.
        if trigger
            && cmd_is_empty
            && self.block_task_rec.is_some()
            && (self.btr_awaiting_name || self.selection.is_empty())
        {
            self.btr_awaiting_name = false;
            if space_now {
                self.cmd.clear();
            }
            self.finish_block_task_recorder();
            return;
        }
        // Draw tool at its FIRST-point prompt: Enter/Space continues from the
        // last picked point (AutoCAD "last point"). Only fires when nothing is
        // pending yet, so it never steals Enter from finish/close.
        if trigger && cmd_is_empty && self.feed_first_point_from_last() {
            if space_now {
                self.cmd.clear();
            }
            return;
        }
        // Insert ANGLE step — empty Enter = 0° rotation, place + cut.
        if trigger && cmd_is_empty {
            if let InsertState::WaitingForAngle { block, insert } = self.insert_state {
                self.insert_state = InsertState::Off;
                if space_now {
                    self.cmd.clear();
                }
                self.apply_insert(block, insert, 0.0);
                return;
            }
        }
        // ZOOM flow — empty Enter (or Space) advances/exits the active flow.
        // Empty input never reaches run_command, so it must be handled here.
        if trigger && cmd_is_empty && self.zoom_state != ZoomState::Off {
            if space_now {
                self.cmd.clear();
            }
            match self.zoom_state {
                // Menu default = real-time zoom.
                ZoomState::Menu => self.zoom_set_state(
                    ZoomState::RealTime,
                    "real-time zoom: drag UP = in, DOWN = out  [Enter/Esc = exit]",
                ),
                ZoomState::RealTime => {
                    self.zoom_finish();
                    self.history.push("  zoom: real-time done".into());
                }
                ZoomState::ObjectSel => {
                    self.zoom_object();
                    self.zoom_finish();
                }
                ZoomState::CenterMag(c) => {
                    self.zoom_center(c, None, None);
                    self.zoom_finish();
                }
                // Point-pick steps: empty Enter just keeps waiting for the pick.
                _ => {}
            }
            return;
        }
        // AREA — an empty Enter folds the current point polygon into the
        // running total (the session stays live until Esc). Empty input
        // never reaches run_command, so it has to be handled here.
        if trigger && cmd_is_empty && self.area_state.is_some() {
            if space_now {
                self.cmd.clear();
            }
            self.area_finish_polygon();
            return;
        }
        // DIVIDE / MEASURE value prompt — an empty Enter exits the value
        // entry and the command (no default value exists).
        if trigger
            && cmd_is_empty
            && matches!(
                self.ptdist_state,
                PtDistribState::DivideValue(_) | PtDistribState::MeasureValue(_)
            )
        {
            if space_now {
                self.cmd.clear();
            }
            self.ptdist_state = PtDistribState::Off;
            self.clear_prompt();
            return;
        }
        // PEDIT — empty Enter exits the menu (Width step returns to the menu).
        if trigger && cmd_is_empty && self.pedit_state != PeditState::Off {
            if space_now {
                self.cmd.clear();
            }
            match self.pedit_state {
                PeditState::Width(h) => {
                    self.pedit_state = PeditState::Menu(h);
                    self.pedit_reprompt(h);
                }
                _ => self.pedit_exit(),
            }
            return;
        }
        // PLINE width / arc sub-flow: an empty Enter belongs to the sub-flow
        // (accept the default width, or cancel an arc sub-flow) — it must NOT
        // fall through and FINISH the polyline. Without this, pressing Enter to
        // accept a default width committed the whole polyline and dropped the
        // width entry.
        if trigger
            && cmd_is_empty
            && self.tool == Tool::Polyline
            && (self.pline_width_cap != PlineWidthCap::None
                || self.pline_arc_sub != PlineArcSub::Normal)
        {
            if space_now {
                self.cmd.clear();
            }
            if self.pline_width_cap != PlineWidthCap::None {
                self.pline_width_accept_default();
            } else {
                self.pline_arc_sub = PlineArcSub::Normal;
                self.pline_dir_override = None;
                self.history.push("  pline: cancelled arc sub-flow".into());
                self.update_pline_prompt();
            }
            return;
        }
        if trigger && cmd_is_empty {
            if self.select_mode != SelectMode::Off {
                // Item 4 — 2-stage cancel for a select-mode wait with an
                // EMPTY basket on non-cutter/non-boundary sessions.
                // (TRIM ForCuttingEdges and EXTEND ForBoundaryEdges keep
                // their documented "Enter = use ALL dobjects" semantics.)
                let basket_empty = self.selection.is_empty();
                let is_cutter_or_bound = matches!(
                    self.select_mode,
                    SelectMode::ForCuttingEdges | SelectMode::ForBoundaryEdges
                );
                if basket_empty && !is_cutter_or_bound {
                    self.empty_enter_count_in_select += 1;
                    if self.empty_enter_count_in_select == 1 {
                        self.set_prompt("please make a selection (Enter again to cancel)");
                    } else {
                        // 2nd empty Enter cancels the whole command.
                        self.cancel_selection();
                        self.queued_op = QueuedOp::None;
                        self.clear_prompt();
                    }
                } else {
                    // Non-empty basket OR cutter/boundary mode → finalise
                    // (the existing behaviour, including "Enter = all").
                    self.finalise_selection();
                }
            } else if matches!(
                self.trim_state,
                TrimState::PickingTargets(_) | TrimState::PickingTargetsAll
            ) {
                self.trim_dbg("=== TRIM session END (Enter) ===");
                self.trim_state = TrimState::Off;
                self.fence_armed = false;
                self.fence_first = None;
                self.clear_prompt();
            } else if matches!(
                self.extend_state,
                ExtendState::PickingTargets(_) | ExtendState::PickingTargetsAll
            ) {
                self.trim_dbg("=== EXTEND session END (Enter) ===");
                self.extend_state = ExtendState::Off;
                self.fence_armed = false;
                self.fence_first = None;
                self.clear_prompt();
            } else if (self.tool == Tool::Polyline && self.pending.len() >= 2)
                || (self.tool == Tool::Spline && self.pending.len() >= 3)
            {
                // Enter FINISHES the in-progress polyline/spline as an OPEN
                // dobject — the same commit used on interrupt (single source of
                // truth in `commit_active_draw`).
                self.commit_active_draw();
            } else if self.tool == Tool::Wall && !self.pending.is_empty() {
                // Chained wall run: each segment is committed live, so Enter
                // just ends the run. Tool stays active for a fresh run.
                self.pending.clear();
                self.history.push("  wall: run ended".into());
                self.clear_prompt();
            } else if matches!(self.offset_state, OffsetState::WaitingForObject(_)) {
                // Offset's loop exit: Enter at "Select object" returns
                // to Off. Matches AutoCAD's `Select object to offset
                // or [Exit/Undo] <Exit>:` default-Enter-is-Exit.
                self.offset_state = OffsetState::Off;
                self.offset_erase = false;
                self.offset_layer_src = false;
                let n = self.offset_applied_count;
                self.offset_applied_count = 0;
                self.clear_prompt();
                self.history.push(format!(
                    "  offset: exit ({} offset{} applied)",
                    n,
                    if n == 1 { "" } else { "s" }
                ));
            } else if let MirrorState::AwaitingKeep(a, b) = self.mirror_state {
                // Enter at the mirror keep-original prompt = keep a copy
                // (the default). n/no/ni (typed) erases; handled in
                // run_command's mirror intercept.
                self.mirror_state = MirrorState::Off;
                self.apply_mirror(a, b, true);
                self.clear_prompt();
            } else if self.fillet_waiting_radius {
                // Fillet radius sub-prompt ("radius <r> (Enter = keep)").
                // A bare Enter accepts the CURRENT radius and advances to
                // the pick phase. The keep-branch in `run_command` (which
                // does the same thing) is unreachable for an empty cmd
                // line — `run_command` is only called when the cmd is
                // non-empty — so the empty Enter must be handled here, or
                // it falls through to "repeat last command" and the radius
                // prompt appears stuck.
                let v = self.env.FltRad;
                self.fillet_waiting_radius = false;
                self.fillet_state = FilletState::WaitingForFirst(v);
                self.history.push(format!("  fillet: radius kept at {}", v));
                self.refresh_fillet_prompt();
            } else if matches!(
                self.fillet_state,
                FilletState::WaitingForFirst(_) | FilletState::WaitingForSecond(..)
            ) {
                // Enter at the MAIN fillet prompt ("fillet (r=0.5, …): click
                // FIRST line"): the user pressed Enter without typing a new
                // radius — treat it as "keep the current value and carry on".
                // Just re-issue the prompt; do NOT fall through to repeat-last
                // (which would restart the command). Lines are picked with
                // the mouse, not Enter.
                self.refresh_fillet_prompt();
            } else if self.chamfer_dist_wait != ChamferDistWait::Off {
                // Mirror of the fillet radius-keep, for chamfer's two
                // distance sub-prompts. Same unreachable-empty-cmd reason.
                match self.chamfer_dist_wait {
                    ChamferDistWait::WaitingD1 => {
                        let d1 = self.env.ChmDs1;
                        self.chamfer_dist_wait = ChamferDistWait::WaitingD2(d1);
                        self.history
                            .push(format!("  chamfer: first distance kept at {}", d1));
                        self.set_prompt(format!(
                            "chamfer: second distance <{}> (Enter = same as first)  [Esc=cancel]",
                            d1
                        ));
                    }
                    ChamferDistWait::WaitingD2(d1) => {
                        self.env.ChmDs1 = d1;
                        self.env.ChmDs2 = d1; // empty 2nd → d2 = d1
                        let _ = self.env.save();
                        self.chamfer_state = ChamferState::WaitingForFirst(d1, d1);
                        self.chamfer_dist_wait = ChamferDistWait::Off;
                        self.history
                            .push(format!("  chamfer: distances → ({}, {})", d1, d1));
                        self.refresh_chamfer_prompt();
                    }
                    ChamferDistWait::Off => unreachable!(),
                }
            } else if matches!(
                self.chamfer_state,
                ChamferState::WaitingForFirst(..) | ChamferState::WaitingForSecond(..)
            ) {
                // Enter at the main chamfer prompt — keep current distances,
                // stay in the command (see fillet rationale above).
                self.refresh_chamfer_prompt();
            } else {
                // Item 4 — fully idle: Enter on empty cmd repeats last cmd.
                if let Some(last) = self.last_command.clone() {
                    self.run_command(&last);
                }
            }
            // The Space that TextEdit also saw may have left a single
            // whitespace char in self.cmd; clean it so the repeated cmd's
            // box stays empty.
            if space_now {
                self.cmd.clear();
            }
        }
        // Dim sub-option: typing `D` (or `dia`/`diameter`) during a
        // Radius draft flips the kind to Diameter (and vice versa with
        // `R`/`rad`/`radius`). Lets the user switch sub-kind on the
        // fly without restarting the command. Eaten before the main
        // parser so it doesn't collide with the global `dim` keyword.
        if self.tool == Tool::Dim && enter_now && !cmd_is_empty {
            if let DimDraftState::WaitingForDimLinePos { kind } = self.dim_draft.clone() {
                let t = self.cmd.trim().to_ascii_lowercase();
                let flip_to_dia = matches!(t.as_str(), "d" | "dia" | "diameter");
                let flip_to_rad = matches!(t.as_str(), "r" | "rad" | "radius");
                let new_kind = match (&kind, flip_to_dia, flip_to_rad) {
                    (DimDraftKind::Radius { center, on_circle }, true, _) => {
                        Some(DimDraftKind::Diameter {
                            center: *center,
                            on_circle: *on_circle,
                        })
                    }
                    (DimDraftKind::Diameter { center, on_circle }, _, true) => {
                        Some(DimDraftKind::Radius {
                            center: *center,
                            on_circle: *on_circle,
                        })
                    }
                    _ => None,
                };
                if let Some(k) = new_kind {
                    self.dim_draft = DimDraftState::WaitingForDimLinePos { kind: k };
                    self.cmd.clear();
                    self.history.push("  dim: kind toggled".into());
                }
            }
        }

        // Polyline `c`/`close` then Enter — handled separately because
        // it consumes a non-empty cmd line, so it doesn't collide with
        // the empty-Enter cascade above.
        if self.tool == Tool::Polyline && enter_now && !cmd_is_empty {
            let trimmed = self.cmd.trim().to_ascii_lowercase();
            if trimmed == "c" || trimmed == "close" || trimmed == "closed" {
                if self.pending.len() >= 2 {
                    let (verts, widths) = self.drain_pline_pending(true);
                    self.add_dobject(
                        Geom::Polyline(Polyline {
                            vertices: verts,
                            closed: true,
                            widths,
                        }),
                        "canvas (closed)",
                    );
                    self.cmd.clear();
                } else {
                    self.history
                        .push("  ! polyline needs at least 2 vertices".into());
                    self.pending.clear();
                    self.pending_bulges.clear();
                    self.pending_widths.clear();
                    self.pline_mode = PlineMode::Line;
                    self.pline_width_cap = PlineWidthCap::None;
                }
            }
        }

        // Wall `t`/`thickness` sub-option — set the wall thickness mid
        // command (persists to WlThk). `t` alone arms a wait for the next
        // number; `t <val>` (or `t5`) sets it directly; when armed, a bare
        // number sets it. Eaten before the parser so `t` isn't a bad cmd.
        if self.tool == Tool::Wall && enter_now && !cmd_is_empty {
            let s = self.cmd.trim().to_ascii_lowercase();
            let mut new_thk: Option<f64> = None;
            let mut handled = false;
            if s == "t" || s == "thickness" {
                self.wall_waiting_thickness = true;
                self.set_prompt(format!(
                    "wall: type thickness then Enter  (current {})",
                    self.env.WlThk
                ));
                handled = true;
            } else if s.starts_with('t') && s.len() > 1 {
                // `t <expr>` — evaluate the ORIGINAL case (variables are
                // case-sensitive, so a lowercased copy must not be used).
                let rest_raw = &self.cmd.trim()[1..];
                if let Ok(v) = self.eval_number(rest_raw.trim()) {
                    new_thk = Some(v);
                    handled = true;
                }
            } else if self.wall_waiting_thickness {
                if let Ok(v) = self.eval_number(&self.cmd) {
                    new_thk = Some(v);
                    handled = true;
                }
            }
            if let Some(v) = new_thk {
                if v > 1e-9 {
                    self.env.WlThk = v;
                    let _ = self.env.save();
                    self.history.push(format!("  wall: thickness = {}", v));
                } else {
                    self.history
                        .push("  ! wall: thickness must be positive".into());
                }
                self.wall_waiting_thickness = false;
            }
            if handled {
                self.cmd.clear();
            }
        }

        // ---- TOP BAR: logo (spanning) + Quick Access row + menu categories
        // The AutoRASM logo sits in a tall left column that spans BOTH the
        // Quick Access row and the menu-category row. Every menu item
        // dispatches via `run_command` so its behaviour matches typing the
        // same cmd — one source of truth.
        // Last frame's menu-bar rect — keeps a hover-opened category menu open
        // while the pointer is over its button (see `menu_autoclose`).
        let bar_rect = self.menubar_rect;
        let topbar_ir = egui::TopBottomPanel::top("topbar").show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                self.draw_logo_column(ui);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    // --- line 1: customizable Quick Access shortcuts ---
                    ui.horizontal(|ui| {
                        let actions = self.qat_actions.clone();
                        for act in actions {
                            if qat_button(ui, act) { self.run_qat_action(act); }
                        }
                        ui.add_space(2.0);
                        let chev = qat_customize_button(ui, self.qat_customize_open);
                        self.qat_chevron_rect = Some(chev.rect);
                        if chev.clicked() {
                            if self.qat_customize_open {
                                self.qat_customize_open = false;
                            } else {
                                self.qat_customize_open = true;
                                self.qat_just_opened = true;
                            }
                        }
                        // Autosave toggle — right after the Quick Access chevron.
                        ui.add_space(8.0);
                        let a_on = self.autosave_on;
                        if ui
                            .selectable_label(a_on, egui::RichText::new(
                                if a_on { "⤓ Autosave: on" } else { "⤓ Autosave: off" }).small())
                            .on_hover_text(
                                "Silently re-saves the open .dxf/.rsm a few minutes after an edit \
                                 (atomic write — an interrupted save can't corrupt the file).\n\n\
                                 OFF every time the app starts, and not remembered between \
                                 sessions: it is the only thing here that writes over your file \
                                 with no action from you, so it is never on unless you just \
                                 turned it on. Click to toggle.")
                            .clicked()
                        {
                            self.autosave_on = !a_on;
                            self.history.push(
                                if self.autosave_on { "  autosave enabled".into() }
                                else { "  autosave disabled".into() });
                        }
                        // Product title, far right.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(12.0);
                            ui.label(egui::RichText::new("AutoRASM  2026")
                                .size(15.0)
                                .color(egui::Color32::from_rgb(176, 192, 208)));
                        });
                    });
                    ui.add_space(1.0);
                    // --- line 2: fixed menu categories ---
                    egui::menu::bar(ui, |ui| {
                // Phase 6b: one read-only Ctx snapshot per frame; the registry-
                // driven Draw/Modify items filter/grey against it (all-default now).
                let cmd_ctx = self.build_ctx();
                // File — custom rows (MENU_DROPDOWN). New/Open/Save/Save-As show the
                // toolbar glyphs via `icon_for` (§7); Import opens the generalized
                // flyout (§9). The Save label shows the active file so it's clear
                // where it writes; the rest reserve the empty slot.
                custom_menu(ui, "File", |ui| {
                    let nc = PP_TEXT;
                    let save_label = match &self.current_file {
                        Some(p) => format!("Save  ({})", p.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "current".into())),
                        None => "Save".to_string(),
                    };
                    let (arrow_x, w) = menu_hug_geometry(ui, &[
                        ("New", RowT::Plain), ("Open .rsm / .dxf…", RowT::Plain),
                        (save_label.as_str(), RowT::Plain),
                        ("Save As .dxf…", RowT::Plain),                         ("Save As .rsm…", RowT::Plain),
                        ("Import", RowT::Arrow),
                        ("Export", RowT::Arrow),
                        ("Plot Style Tables", RowT::Arrow),
                        ("Plot…", RowT::Plain),
                        ("CTB Test Scene", RowT::Plain),
                        ("New parametric sketch", RowT::Plain), ("Exit", RowT::Plain),
                    ]);
                    ui.set_width(w);
                    if paint_menu_row(ui, w, arrow_x, icon_for("file.new"), "New", nc, RowT::Plain).0
                        { self.run_command("clear"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for("file.open"), "Open .rsm / .dxf…", nc, RowT::Plain).0
                        { self.open_file_dialog(FileDialogMode::Open, ".rsm"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for("file.save"), &save_label, nc, RowT::Plain).0
                        { self.do_save_current(); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for("file.saveas"), "Save As .dxf…", nc, RowT::Plain).0
                        { self.open_file_dialog(FileDialogMode::Save, ".dxf"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for("file.saveas"), "Save As .rsm…", nc, RowT::Plain).0
                        { self.open_file_dialog(FileDialogMode::Save, ".rsm"); ui.close_menu(); }
                    menu_divider(ui, w);
                    // Import ▸ — generalized flyout (raster / vector).
                    let (_ib, ia, irect) = paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Import", nc, RowT::Arrow);
                    if ia { self.open_flyout(FlyMenu::Import, irect); }
                    menu_divider(ui, w);
                    // Export ▸ — quick PDF/SVG/PNG export (whole drawing).
                    let (_xb, xa, xrect) = paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Export", nc, RowT::Arrow);
                    if xa { self.open_flyout(FlyMenu::Export, xrect); }
                    menu_divider(ui, w);
                    // Plot Style Tables ▸ — CTB manager: New / built-ins / saved
                    // tables (defined in the app's <exe_dir>/ctb as .pst files).
                    let (ptb, pta, ptrect) = paint_menu_row(
                        ui, w, arrow_x, MenuIcon::None, "Plot Style Tables", nc, RowT::Arrow);
                    if pta { self.open_flyout(FlyMenu::PlotStyleTables, ptrect); }
                    let _ = ptb;
                    menu_divider(ui, w);
                    // Plot → PDF (plot/print/Ctrl+P) — model space only; the
                    // plot command itself refuses while a layout tab is active.
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Plot…", nc, RowT::Plain).0
                        { self.run_command("plot"); ui.close_menu(); }
                    // CTB Test Scene — a shapes + layout + test-CTB playground so
                    // cap/join/width/dash pen styles can be eyeballed per viewport
                    // (and in exports) without hand-building a test document.
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "CTB Test Scene", nc, RowT::Plain).0 {
                        self.create_ctb_test_scene();
                        ui.close_menu();
                    }
                    menu_divider(ui, w);
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "New parametric sketch", nc, RowT::Plain).0 {
                        self.run_command("clear");
                        self.parametric = crate::param_editor::ParamSession::new();
                        self.parametric.active = true;
                        self.parametric.status =
                            "parametric mode: draw lines with the Line tool, select them, \
                             add constraints, then Solve".into();
                        ui.close_menu();
                    }
                    menu_divider(ui, w);
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Exit", nc, RowT::Plain).0
                        { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
                    pp_cap_ui(ui, "menu: File");
                });
                // Edit — custom rows (MENU_DROPDOWN §1). Undo/Redo/Copy/Erase/Match
                // have Cmd glyphs; the rest reserve the empty 20px slot so all names
                // align at the fixed name column. Shortcuts render right-aligned in
                // the trailing zone (Mono-11, muted) — Ctrl+C/V/G + Shift+A (Select
                // All) / Ctrl+D (Deselect All), wired in the input block below.
                custom_menu(ui, "Edit", |ui| {
                    let nc = PP_TEXT;
                    let (arrow_x, w) = menu_hug_geometry(ui, &[
                        ("Undo", RowT::Plain), ("Redo", RowT::Plain),
                        ("Copy", RowT::Shortcut("Ctrl+C")),
                        ("Paste", RowT::Shortcut("Ctrl+V")),
                        ("Group", RowT::Shortcut("Ctrl+G")),
                        ("Add to Group", RowT::Plain), ("Ungroup", RowT::Plain),
                        ("Select All", RowT::Shortcut("Shift+A")),
                        ("Deselect All", RowT::Shortcut("Ctrl+D")),
                        ("Erase", RowT::Plain),
                        ("Match Properties", RowT::Plain),
                        ("Settings…", RowT::Plain),
                    ]);
                    ui.set_width(w);
                    if paint_menu_row(ui, w, arrow_x, icon_for("edit.undo"), "Undo", nc, RowT::Plain).0
                        { self.run_command("undo"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for("edit.redo"), "Redo", nc, RowT::Plain).0
                        { self.run_command("redo"); ui.close_menu(); }
                    menu_divider(ui, w);
                    if paint_menu_row(ui, w, arrow_x, icon_for("edit.copy"), "Copy", nc, RowT::Shortcut("Ctrl+C")).0 {
                        self.copy_selection();
                        let n = self.clipboard_dobjects.len();
                        ui.ctx().copy_text(format!("RUST-AutoRASM: {} object(s) on clipboard", n));
                        ui.close_menu();
                    }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Paste", nc, RowT::Shortcut("Ctrl+V")).0
                        { self.start_paste(); ui.close_menu(); }
                    menu_divider(ui, w);
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Group", nc, RowT::Shortcut("Ctrl+G")).0
                        { self.group_selection(); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Add to Group", nc, RowT::Plain).0
                        { self.add_to_group(); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Ungroup", nc, RowT::Plain).0
                        { self.ungroup_selection(); ui.close_menu(); }
                    menu_divider(ui, w);
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Select All", nc, RowT::Shortcut("Shift+A")).0
                        { self.run_command("select"); self.run_command("all"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Deselect All", nc, RowT::Shortcut("Ctrl+D")).0
                        { self.selection.clear(); self.selected = None; ui.close_menu(); }
                    menu_divider(ui, w);
                    if paint_menu_row(ui, w, arrow_x, icon_for("edit.erase"), "Erase", nc, RowT::Plain).0
                        { self.run_command("erase"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for("edit.matchprop"), "Match Properties", nc, RowT::Plain).0
                        { self.run_command("matchprop"); ui.close_menu(); }
                    menu_divider(ui, w);
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Settings…", nc, RowT::Plain).0
                        { self.settings_open = true; ui.close_menu(); }
                    pp_cap_ui(ui, "menu: Edit");
                });
                // Draw — custom-painted rows (MENU_DROPDOWN_MENTOR): every command
                // iconized in one aligned column, 26 band / 20 icon box / 14 gap
                // (palette-identical), cyan (CODE) marker, surface-2 hover, ▸ on one
                // aligned column, hug width. `custom_menu` owns the edge-to-edge
                // chrome (§3); `menu_hug_geometry` owns the width (§2/§7).
                custom_menu(ui, "Draw", |ui| {
                    let circle_key  = self.current_method_key("circle");
                    let circle_code = self.current_method_short("circle");
                    let arc_key     = self.current_method_key("arc");
                    let arc_code    = self.current_method_short("arc");
                    let nc = PP_TEXT;   // command-name colour

                    // Pass 1 — hug geometry from every FULL line (incl. (CODE)/hint).
                    let (arrow_x, w) = menu_hug_geometry(ui, &[
                        ("Line", RowT::Plain), ("Rectangle", RowT::Plain),
                        ("Circle", RowT::CodeArrow(&circle_code, PP_ACCENT)),
                        ("Arc",    RowT::CodeArrow(&arc_code, PP_ACCENT)),
                        ("Ellipse", RowT::Plain), ("Ellipse Arc", RowT::Plain),
                        ("Polyline", RowT::Plain), ("Spline", RowT::Plain),
                        ("Point", RowT::Plain), ("Hatch…", RowT::Plain),
                        ("Wall", RowT::Hint("(t = thickness)")),
                        ("Block…", RowT::Plain), ("Insert Block", RowT::Plain),
                    ]);

                    // Pass 2 — render + dispatch. §3: pin frame inner width to `w`.
                    ui.set_width(w);
                    if paint_menu_row(ui, w, arrow_x, icon_for("draw.line"), "Line", nc, RowT::Plain).0
                        { self.execute("draw.line"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for("draw.rectangle"), "Rectangle", nc, RowT::Plain).0
                        { self.execute("draw.rectangle"); ui.close_menu(); }
                    // Circle — body runs remembered; ▸ opens the method flyout
                    // (top-level, survives the dropdown closing — §2.1).
                    let (cb, ca, crect) = paint_menu_row(ui, w, arrow_x,
                        MenuIcon::Method("circle", &circle_key), "Circle", nc,
                        RowT::CodeArrow(&circle_code, PP_ACCENT));
                    if cb { self.execute("draw.circle"); ui.close_menu(); }
                    if ca { self.open_flyout(FlyMenu::Method("circle".into()), crect); }
                    // Arc
                    let (ab, aa, arect) = paint_menu_row(ui, w, arrow_x,
                        MenuIcon::Method("arc", &arc_key), "Arc", nc,
                        RowT::CodeArrow(&arc_code, PP_ACCENT));
                    if ab { self.execute("draw.arc"); ui.close_menu(); }
                    if aa { self.open_flyout(FlyMenu::Method("arc".into()), arect); }
                    for (name, id) in [
                        ("Ellipse",     "draw.ellipse"),
                        ("Ellipse Arc", "draw.ellipsearc"),
                        ("Polyline",    "draw.pline"),
                        ("Spline",      "draw.spline"),
                        ("Point",       "draw.point"),
                    ] {
                        if paint_menu_row(ui, w, arrow_x, icon_for(id), name, nc, RowT::Plain).0
                            { self.execute(id); ui.close_menu(); }
                    }
                    if paint_menu_row(ui, w, arrow_x, icon_for("draw.hatch"), "Hatch…", nc, RowT::Plain).0
                        { self.run_command("hatch"); ui.close_menu(); }
                    // Wall (t = thickness) — muted parenthetical hint.
                    if paint_menu_row(ui, w, arrow_x, icon_for("draw.wall"), "Wall", nc, RowT::Hint("(t = thickness)")).0
                        { self.run_command("wall"); ui.close_menu(); }
                    menu_divider(ui, w);   // group divider hairline (kept)
                    if paint_menu_row(ui, w, arrow_x, icon_for("draw.block"), "Block…", nc, RowT::Plain).0
                        { self.run_command("block"); ui.close_menu(); }
                    // Insert Block ▸ — opens the block-name flyout (top-level, §2.1).
                    let (ib, ia, irect) = paint_menu_row(ui, w, arrow_x, icon_for("draw.insert"), "Insert Block", nc, RowT::Arrow);
                    if ia { self.open_flyout(FlyMenu::Insert, irect); }
                    let _ = ib;
                    pp_cap_ui(ui, "menu: Draw");
                });
                // Modify — custom rows (MENU_DROPDOWN). Every command has a Cmd
                // glyph except Inspector (reserved slot). Fillet is the ONE method
                // row: cyan (CODE) + ▸ hover flyout (MenuFlyout::Method), body runs
                // the remembered method (§4). Explode/Inspector keep their hover
                // tooltips. `cmd_ctx` predicates are all-default now (as in Draw) —
                // dispatch straight through execute(id).
                let _ = &cmd_ctx;
                custom_menu(ui, "Modify", |ui| {
                    let nc = PP_TEXT;
                    let fillet_key  = self.current_method_key("fillet");
                    let fillet_code = self.current_method_short("fillet");
                    let (arrow_x, w) = menu_hug_geometry(ui, &[
                        ("Move", RowT::Plain), ("Copy", RowT::Plain),
                        ("Rotate", RowT::Plain), ("Scale", RowT::Plain),
                        ("Mirror", RowT::Plain), ("Stretch", RowT::Plain),
                        ("Align", RowT::Plain), ("Trim", RowT::Plain),
                        ("Extend", RowT::Plain),
                        ("Fillet", RowT::CodeArrow(&fillet_code, PP_ACCENT)),
                        ("Chamfer", RowT::Plain), ("Offset", RowT::Plain),
                        ("Join", RowT::Plain), ("Break", RowT::Plain),
                        ("Lengthen", RowT::Plain), ("Reverse", RowT::Plain),
                        ("Array…", RowT::Plain), ("Explode", RowT::Plain),
                        ("Inspector…", RowT::Plain),
                        ("Match Properties", RowT::Plain),
                        ("Change Layer to Current", RowT::Plain),
                        ("Erase", RowT::Plain),
                    ]);
                    ui.set_width(w);
                    // Transform group. Icon via `icon_for` on the id's base (before
                    // any arg, e.g. "modify.lengthen 1"); dispatch via the full id.
                    let icon_key = |id: &'static str| id.split(' ').next().unwrap_or(id);
                    for (name, id) in [
                        ("Move", "modify.move"), ("Copy", "modify.copy"),
                        ("Rotate", "modify.rotate"), ("Scale", "modify.scale"),
                        ("Mirror", "modify.mirror"), ("Stretch", "modify.stretch"),
                        ("Align", "modify.align"),
                    ] {
                        if paint_menu_row(ui, w, arrow_x, icon_for(icon_key(id)), name, nc, RowT::Plain).0
                            { self.execute(id); ui.close_menu(); }
                    }
                    menu_divider(ui, w);
                    // Edit-geometry group.
                    for (name, id) in [("Trim", "modify.trim"), ("Extend", "modify.extend")] {
                        if paint_menu_row(ui, w, arrow_x, icon_for(icon_key(id)), name, nc, RowT::Plain).0
                            { self.execute(id); ui.close_menu(); }
                    }
                    // Fillet — method row (cyan CODE + ▸ hover flyout, §4/§2.1).
                    let (fb, fa, frect) = paint_menu_row(ui, w, arrow_x,
                        MenuIcon::Method("fillet", &fillet_key), "Fillet", nc,
                        RowT::CodeArrow(&fillet_code, PP_ACCENT));
                    if fb { self.execute("modify.fillet"); ui.close_menu(); }
                    if fa { self.open_flyout(FlyMenu::Method("fillet".into()), frect); }
                    for (name, id) in [
                        ("Chamfer", "modify.chamfer"), ("Offset", "modify.offset"),
                        ("Join", "modify.join"), ("Break", "modify.break"),
                        ("Lengthen", "modify.lengthen 1"), ("Reverse", "modify.reverse"),
                    ] {
                        if paint_menu_row(ui, w, arrow_x, icon_for(icon_key(id)), name, nc, RowT::Plain).0
                            { self.execute(id); ui.close_menu(); }
                    }
                    menu_divider(ui, w);
                    // Multi-instance / properties group.
                    if paint_menu_row(ui, w, arrow_x, icon_for("modify.array"), "Array…", nc, RowT::Plain).0
                        { self.array_open = true; ui.close_menu(); }
                    let (eb, _, erect) = paint_menu_row(ui, w, arrow_x, icon_for("modify.explode"), "Explode", nc, RowT::Plain);
                    ui.interact(erect, egui::Id::new(("modtip", "explode")), egui::Sense::hover())
                        .on_hover_text("Break selected block instances back into their component dobjects");
                    if eb { self.run_command("explode"); ui.close_menu(); }
                    // Inspector… — `props`, not a registry command; reserved slot.
                    let (ipb, _, iprect) = paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Inspector…", nc, RowT::Plain);
                    ui.interact(iprect, egui::Id::new(("modtip", "inspector")), egui::Sense::hover())
                        .on_hover_text("Edit common properties of the selected dobject(s) — \
                                        layer, color, linetype, lineweight, and type-specific style");
                    if ipb { self.run_command("props"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for(icon_key("modify.matchprop")),
                                      "Match Properties", nc, RowT::Plain).0
                        { self.execute("modify.matchprop"); ui.close_menu(); }
                    // `chlayer` (bulk-set selection to the active layer) is no longer a
                    // rail entry — the layers glyph now opens the Layer Manager (`layer`).
                    // Dispatch the token directly so this menu keeps the function.
                    if paint_menu_row(ui, w, arrow_x, icon_for("modify.chlayer"),
                                      "Change Layer to Current", nc, RowT::Plain).0
                        { self.run_command("chlayer"); ui.close_menu(); }
                    menu_divider(ui, w);
                    if paint_menu_row(ui, w, arrow_x, icon_for("modify.erase"), "Erase", nc, RowT::Plain).0
                        { self.execute("modify.erase"); ui.close_menu(); }
                    pp_cap_ui(ui, "menu: Modify");
                });
                // View — custom rows (MENU_DROPDOWN). None of the zoom commands
                // have a glyph yet → all reserve the empty 20px slot (§1); labels
                // kept verbatim ("Zoom Extents (fit all)" flagged for a trim).
                custom_menu(ui, "View", |ui| {
                    let nc = PP_TEXT;
                    let (arrow_x, w) = menu_hug_geometry(ui, &[
                        ("Zoom Extents", RowT::Plain),
                        ("Zoom Window", RowT::Plain),
                        ("Zoom Previous", RowT::Plain),
                        ("Reset View", RowT::Plain),
                    ]);
                    ui.set_width(w);
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Zoom Extents", nc, RowT::Plain).0
                        { self.zoom_extents(); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Zoom Window", nc, RowT::Plain).0
                        { self.zoom_start("w"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Zoom Previous", nc, RowT::Plain).0
                        { self.zoom_previous(); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Reset View", nc, RowT::Plain).0 {
                        self.view_push_history();
                        self.scale = 20.0;
                        self.world_offset = egui::vec2(0.0, 0.0);
                        ui.close_menu();
                    }
                    pp_cap_ui(ui, "menu: View");
                });
                // Formative — custom rows (MENU_DROPDOWN). Layers/Pens toggle their
                // panels; Dimension + Styles open the generalized flyout (§9) — the
                // Styles flyout itself nests the current dim/wall-style pickers.
                custom_menu(ui, "Formative", |ui| {
                    let nc = PP_TEXT;
                    let (arrow_x, w) = menu_hug_geometry(ui, &[
                        ("Layers…", RowT::Plain), ("Pens…", RowT::Plain),
                        ("Dimension", RowT::Arrow), ("Styles", RowT::Arrow),
                    ]);
                    ui.set_width(w);
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Layers…", nc, RowT::Plain).0
                        { self.layer_panel_open = !self.layer_panel_open; ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Pens…", nc, RowT::Plain).0
                        { self.pen_panel_open = !self.pen_panel_open; ui.close_menu(); }
                    menu_divider(ui, w);
                    let (_db, da, drect) = paint_menu_row(ui, w, arrow_x,
                        icon_for("formative.dimension"), "Dimension", nc, RowT::Arrow);
                    if da { self.open_flyout(FlyMenu::Dimension, drect); }
                    let (_sb, sa, srect) = paint_menu_row(ui, w, arrow_x,
                        MenuIcon::None, "Styles", nc, RowT::Arrow);
                    if sa { self.open_flyout(FlyMenu::Styles, srect); }
                    pp_cap_ui(ui, "menu: Formative");
                });
                // Utilities — custom rows (MENU_DROPDOWN). Both inquiry commands
                // have Cmd glyphs (Dist / List). Labels kept verbatim ("Distance
                // (2 clicks)" flagged for a trim).
                custom_menu(ui, "Utilities", |ui| {
                    let nc = PP_TEXT;
                    let (arrow_x, w) = menu_hug_geometry(ui, &[
                        ("Distance", RowT::Plain),
                        ("List", RowT::Plain),
                    ]);
                    ui.set_width(w);
                    if paint_menu_row(ui, w, arrow_x, icon_for("util.dist"), "Distance", nc, RowT::Plain).0
                        { self.run_command("dist"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for("util.list"), "List", nc, RowT::Plain).0
                        { self.run_command("list"); ui.close_menu(); }
                    pp_cap_ui(ui, "menu: Utilities");
                });
                // Tools — custom rows (MENU_DROPDOWN). §8 section headings +
                // cyan-checkbox rows for the panel-visibility toggles; the Command
                // palette shows its shortcut (right-aligned, §1); Debug tools opens
                // the generalized flyout (§9). Checkbox rows omit `close_menu` so you
                // can flip several without the menu closing.
                custom_menu(ui, "Tools", |ui| {
                    let nc = PP_TEXT;
                    let (arrow_x, w) = menu_hug_geometry(ui, &[
                        ("Palettes", RowT::Plain),
                        ("Command line", RowT::Plain),
                        ("Command palette…", RowT::Shortcut("Ctrl+Shift+P")),
                        ("Layers", RowT::Plain), ("Pens", RowT::Plain),
                        ("Inspector", RowT::Plain), ("DObjects list", RowT::Plain),
                        ("Snap window", RowT::Plain), ("Toggle Grips", RowT::Plain),
                        ("Session Recorder", RowT::Plain),
                        ("Inquiry", RowT::Plain),
                        ("Distance", RowT::Plain), ("List", RowT::Plain),
                        ("Debug tools", RowT::Arrow),
                    ]);
                    ui.set_width(w);
                    // §8 checkbox row: cyan check in the slot; click toggles, menu stays open.
                    macro_rules! check_row { ($name:expr, $field:expr) => {
                        if paint_menu_row(ui, w, arrow_x, MenuIcon::Check($field), $name, nc, RowT::Plain).0
                            { $field = !$field; }
                    }; }
                    // §10 panel toggle: like check_row, but on OPEN it (a) records the
                    // menu-launch anchor (adjacent-right, top-aligned to this row) so the
                    // panel first opens beside the menu — not backward/left — and (b)
                    // queues a bring-to-front so it opens ON TOP, never behind the
                    // dropdown or other panels. `$id` must match the panel's dock id.
                    macro_rules! panel_row { ($name:expr, $field:expr, $id:expr) => {{
                        let (clk, _, r) = paint_menu_row(ui, w, arrow_x, MenuIcon::Check($field), $name, nc, RowT::Plain);
                        if clk {
                            $field = !$field;
                            if $field {
                                self.menu_launch_anchor.insert($id, egui::pos2(r.right() + 8.0, r.top()));
                                self.raise_windows.push($id);
                            }
                        }
                    }}; }
                    menu_heading_row(ui, w, "Palettes");
                    panel_row!("Command line", self.cmd_window_open, "Command line");
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Command palette…", nc, RowT::Shortcut("Ctrl+Shift+P")).0 {
                        self.palette_open = true; self.palette_focus = true;
                        self.palette_query.clear(); self.palette_sel = 0;
                        ui.close_menu();
                    }
                    panel_row!("Layers", self.layers_window_open, "Layers");
                    panel_row!("Pens", self.pens_window_open, "Pens");
                    panel_row!("Inspector", self.info_window_open, "Inspector");
                    panel_row!("DObjects list", self.dobjects_window_open, "DObjects");
                    menu_divider(ui, w);
                    check_row!("Snap window", self.snap_window_open);
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Toggle Grips", nc, RowT::Plain).0 {
                        self.env.GrpEnb = !self.env.GrpEnb; let _ = self.env.save(); ui.close_menu();
                    }
                    // (Text Style… removed — it lives in Formative → Styles now.)
                    menu_divider(ui, w);
                    // Session Recorder — a §8 toggle row that also anchors + raises (§10).
                    let (sr_clk, _, sr_r) = paint_menu_row(ui, w, arrow_x,
                        MenuIcon::Check(self.dbg_window_open), "Session Recorder", nc, RowT::Plain);
                    if sr_clk {
                        self.dbg_window_open = !self.dbg_window_open;
                        if self.dbg_window_open {
                            self.menu_launch_anchor.insert("Session Recorder", egui::pos2(sr_r.right() + 8.0, sr_r.top()));
                            self.raise_windows.push("Session Recorder");
                        }
                        crate::dbg_event!(self, crate::dbg_recorder::DbgEvent::MenuClick {
                            path: format!("Tools → Session Recorder ({})",
                                if self.dbg_window_open { "open" } else { "close" }) });
                    }
                    menu_divider(ui, w);
                    menu_heading_row(ui, w, "Inquiry");
                    if paint_menu_row(ui, w, arrow_x, icon_for("util.dist"), "Distance", nc, RowT::Plain).0
                        { self.run_command("dist"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, icon_for("util.list"), "List", nc, RowT::Plain).0
                        { self.run_command("list"); ui.close_menu(); }
                    menu_divider(ui, w);
                    // Debug tools ▸ — generalized flyout (§9): toggles, index, ∩, clear.
                    // Debug tools ▸ — generalized flyout (§9): toggles, index, ∩, clear.
                    let (_gb, ga, grect) = paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Debug tools", nc, RowT::Arrow);
                    if ga { self.open_flyout(FlyMenu::Debug, grect); }
                    // Scripts ▸ — WP-SCRIPT: scripts/*.py → run, editor, guide, console.
                    let (_sb, sa, srect) = paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Scripts", nc, RowT::Arrow);
                    if sa { self.scan_py_examples(); self.open_flyout(FlyMenu::Scripts, srect); }
                    pp_cap_ui(ui, "menu: Tools");
                });
                // Help — custom rows (MENU_DROPDOWN). Neither command has a glyph
                // yet → both reserve the empty 20px slot (§1).
                // SIMLUX — independent lighting (lux) section. Plain egui widgets
                // inside the custom dropdown chrome; opens the grafted Light panels.
                // ---- 3D FACTORY -------------------------------------------
                // The cad_solid 3D solid layer, wired into the real app. Sits between
                // Tools and SIMLUX. Everything here drives `self.factory`
                // (crate::factory::FactoryState) and reuses light3d's renderer.
                custom_menu(ui, "3D Factory", |ui| {
                    ui.set_min_width(248.0);
                    ui.add_space(4.0);
                    // The view checkboxes that used to live here are GONE — the
                    // three views are exclusive workspaces now, switched by the
                    // tab bar at the top (2D view | SIMLUX view | 3D Factory
                    // view). This row just takes the user there.
                    if ui
                        .button("  ◧  3D Factory view")
                        .on_hover_text("Switch to the 3D Factory workspace (the whole window). Draw/modify, building, furniture and storey commands are listed on the left.")
                        .clicked()
                    {
                        self.switch_mode(Mode::Factory);
                        ui.close_menu();
                    }
                    ui.separator();
                    // ---- STOREYS -----------------------------------------
                    // The building's levels. `base_z` is DERIVED (sum of the heights
                    // below), so the stack is contiguous by construction — the UI shows
                    // it but never stores it.
                    ui.label(
                        egui::RichText::new("  Storeys")
                            .small()
                            .color(egui::Color32::from_rgb(150, 165, 185)),
                    );
                    let n_storeys = self.factory.storeys.len();
                    let mut set_active: Option<usize> = None;
                    let mut set_height: Option<(usize, f32)> = None;
                    let mut delete: Option<usize> = None;
                    // Top-down, the way a building is read.
                    for i in (0..n_storeys).rev() {
                        let base = self.factory.storey_base_z(i);
                        let active = i == self.factory.active_storey;
                        let mut h = self.factory.storeys[i].height;
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            if ui
                                .selectable_label(
                                    active,
                                    format!("{}  ⌄{:.2} m", self.factory.storeys[i].name, base),
                                )
                                .on_hover_text("Build new geometry on this level")
                                .clicked()
                            {
                                set_active = Some(i);
                            }
                            let u = self.factory.units.clone();
                            if crate::factory::length_ui(ui, &u, &mut h, 0.05, 0.1, 30.0, &self.calc)
                                .on_hover_text("Floor-to-floor height — levels above move to suit")
                                .changed()
                            {
                                set_height = Some((i, h));
                            }
                            // A building always keeps at least one level.
                            if ui
                                .add_enabled(n_storeys > 1, egui::Button::new("✖"))
                                .on_hover_text("Delete this storey AND the geometry on it")
                                .clicked()
                            {
                                delete = Some(i);
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        if ui.button("＋ Storey on top").clicked() {
                            self.snapshot_factory();
                            let i = self.factory.add_storey_on_top();
                            self.history.push(format!(
                                "  storey '{}' added (base {:.2} m)",
                                self.factory.storeys[i].name,
                                self.factory.storey_base_z(i)
                            ));
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "total {:.2} m",
                                self.factory.building_total_height()
                            ))
                            .small()
                            .weak(),
                        );
                    });
                    // Applied AFTER the loop — mutating the list while iterating it would
                    // invalidate the indices the rows were drawn from.
                    if let Some(i) = set_active {
                        self.factory.active_storey = i;
                    }
                    if let Some((i, h)) = set_height {
                        self.snapshot_factory();
                        self.factory.set_storey_height(i, h);
                    }
                    if let Some(i) = delete {
                        self.snapshot_factory();
                        let name = self.factory.storeys[i].name.clone();
                        if self.factory.delete_storey(i) {
                            self.factory.recompute();
                            self.history.push(format!("  storey '{name}' deleted"));
                        }
                    }
                    ui.separator();
                    // ---- BUILDING ----------------------------------------
                    // The main structure being modelled. Shape-named elements do NOT
                    // belong here — Draw3D below already offers Box / Cylinder / Prism,
                    // and a second door onto the same primitive is duplication. What is
                    // left is the outline, which the palette genuinely cannot do; it
                    // stays disabled until the extrusion primitive is signed off.
                    ui.label(
                        egui::RichText::new("  Building")
                            .small()
                            .color(egui::Color32::from_rgb(150, 165, 185)),
                    );
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label("Building height");
                        let u = self.factory.units.clone();
                        let mut bh = self.factory.building_height;
                        let r = crate::factory::length_ui(ui, &u, &mut bh, 0.05, 0.05, 500.0, &self.calc)
                            .on_hover_text(
                                "Storey height the structure rises to.\n\
                                 Changing it RESIZES a building already standing — it is not only \
                                 a template for the next one.",
                            );
                        // It used to be a template ONLY: the field moved and the building did not,
                        // so a building resized elsewhere went on being reported at this stale
                        // number and every room check measured against a building that had gone.
                        if r.drag_started() || (r.changed() && !r.dragged()) {
                            self.snapshot_factory();
                        }
                        if r.changed() {
                            self.factory.set_building_height(bh);
                            self.factory.recompute();
                        }
                    });
                    ui.separator();
                    // The build TOOLS (3D solids, Make building / walls / floor / ceiling,
                    // Frame, Clear) live in the 3D Factory workspace now — the tab bar or
                    // the row above takes the user there, and the mode command panel +
                    // the viewport's own toolbar carry the tools. This menu keeps only the
                    // things that belong at the top level: the workspace switch, the
                    // storey stack, and the building height above.
                    ui.label(
                        egui::RichText::new("  Build tools are in the 3D Factory workspace ▸")
                            .small()
                            .weak(),
                    );
                    if self.mode != Mode::Factory
                        && ui.button("  ◧  Go to the 3D Factory view").clicked()
                    {
                        self.switch_mode(Mode::Factory);
                        ui.close_menu();
                    }
                    ui.add_space(4.0);
                    pp_cap_ui(ui, "menu: 3D Factory");
                });
                custom_menu(ui, "Materials", |ui| {
                    ui.set_min_width(248.0);
                    ui.add_space(4.0);
                    // The Materials Factory is wired straight into the 3D Factory preview: every
                    // node/parameter edit compiles onto the material and shows there the same frame.
                    if ui
                        .checkbox(&mut self.materials_open, "  🎨  Materials Factory  (node editor)")
                        .on_hover_text("Blender-style shader graph: Texture → Principled BSDF → Output. Edits show live in the 3D Factory view.")
                        .changed()
                        && self.materials_open
                    {
                        self.factory.open = true; // the live preview lives in the 3D Factory panel
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("  Rendering").small().color(egui::Color32::from_rgb(150, 165, 185)));
                    ui.checkbox(&mut self.render_modal_open, "  ⏺  Render  (path tracer — CPU/GPU)")
                        .on_hover_text("Raytraced render of the 3D Factory scene: global illumination, reflections, glass. Progressive; pick CPU or GPU.");
                    ui.checkbox(&mut self.sun_modal_open, "  ☀  Sun / daylight…")
                        .on_hover_text("Locate the sun (city, date, time) — the light both the viewport and the renders use.");
                    if ui
                        .button("  ▶  Run Radiance simulation")
                        .on_hover_text("Export the scene and run LBNL Radiance (oconv → rpict) for a physically-accurate daylight render, shown in-app when done. Needs Radiance installed.")
                        .clicked()
                    {
                        self.pick_radiance_folder(true);
                        ui.close_menu();
                    }
                    ui.add_space(4.0);
                    pp_cap_ui(ui, "menu: Materials");
                });
                custom_menu(ui, "SIMLUX", |ui| {
                    ui.set_min_width(232.0);
                    ui.add_space(4.0);
                    // The "SIMLUX workspace (2D | 3D)" split and the "3D view"
                    // checkbox are GONE — SIMLUX is its own full-window workspace
                    // now, switched by the tab bar at the top. This row takes the
                    // user there (the 3D lighting viewport fills the window).
                    if ui
                        .button("  ◧  SIMLUX view  (3D lighting)")
                        .on_hover_text("Switch to the SIMLUX workspace: the 3D lighting viewport fills the window. Calculation, results and display commands are listed on the left.")
                        .clicked()
                    {
                        self.switch_mode(Mode::Simlux);
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("  Lighting · lux calculation").small()
                        .color(egui::Color32::from_rgb(150, 165, 185)));
                    if ui.button("  ⚡  Calculate lux").clicked() {
                        self.light.window_open = true;
                        self.start_calculation();
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.checkbox(&mut self.light.window_open, "  Light panel (settings + results)");
                    if ui
                        .checkbox(&mut self.light.illuminaire_open, "  💡 Illuminaire (fittings)")
                        .on_hover_text(
                            "Your library of fittings — a 2D block paired with a photometric \
                             file. Place one and the drawing gets an ordinary block; SIMLUX gets \
                             a luminaire.",
                        )
                        .changed()
                        && self.light.illuminaire_open
                    {
                        // Scan on OPEN, so a folder set last session is populated without the user
                        // having to press anything to see what is in it.
                        self.light.lib_scanned =
                            crate::illuminaire::scan_folder(&self.light.lib_folder);
                    }
                    // Placement acts on the PLAN, so it belongs to the 2D workspace
                    // (it is listed in the 2D mode command panel too).
                    ui.checkbox(&mut self.light.place_mode, "  Place luminaire (click plan)");
                    ui.separator();
                    ui.checkbox(&mut self.light.show_overlay, "  Lux overlay on 2D plan");
                    ui.checkbox(&mut self.light.floor_heatmap, "  Heatmap floor in 3D");
                    ui.checkbox(&mut self.light.show_isolux, "  Isolux lines in 3D");
                    ui.add_space(4.0);
                    pp_cap_ui(ui, "menu: SIMLUX");
                });
                custom_menu(ui, "Help", |ui| {
                    let nc = PP_TEXT;
                    let (arrow_x, w) = menu_hug_geometry(ui, &[
                        ("Keyboard shortcuts", RowT::Plain),
                        ("Command help", RowT::Plain),
                        ("About RUST_CAD", RowT::Plain),
                    ]);
                    ui.set_width(w);
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Keyboard shortcuts", nc, RowT::Plain).0 {
                        self.shortcuts_open = true;
                        ui.close_menu();
                    }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "Command help", nc, RowT::Plain).0
                        { self.run_command("help"); ui.close_menu(); }
                    if paint_menu_row(ui, w, arrow_x, MenuIcon::None, "About RUST_CAD", nc, RowT::Plain).0 {
                        self.history.push("  RUST_CAD — pure-Rust 2-D CAD math workbench".into());
                        self.history.push("  github.com/HSI-Lighting/RUST-AutoRASM".into());
                        ui.close_menu();
                    }
                    pp_cap_ui(ui, "menu: Help");
                });
            });
                    });
                });
        });
        self.menubar_rect = topbar_ir.response.rect;
        // The customize drop window (anchored just under the QAT bar).
        self.mark("topbar");
        self.qat_customize_window(ctx);

        // ---- MODE TAB BAR -----------------------------------------------
        // The top tab bar: 2D view | SIMLUX view | 3D Factory view. Exactly one
        // workspace owns the window; clicking a tab switches the view flags
        // (see `switch_mode` / the enforcement hooks at frame start).
        self.render_mode_tabs(ctx);

        // ---- top toolbar ------------------------------------------------
        // Top toolbar — all draw/modify icons moved to the left rails, so this
        // panel only exists to surface an intersection-result label. Render it
        // ONLY when there's a label, otherwise it's a blank gap above the canvas.
        if !self.last_intersect_label.is_empty() {
            egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(2.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(180, 200, 220),
                        &self.last_intersect_label,
                    );
                });
                ui.add_space(4.0);
            });
        }

        // ---- debug window (CPU/GPU render toggle + stats) -------------------
        if self.debug_open {
            let mut keep = true;
            let mode_before = self.render_mode;
            let dobject_count = self.doc.dobjects.len();
            let circle_count = self
                .doc
                .dobjects
                .iter()
                .filter(|d| matches!(d.geom, Geom::Circle(_)))
                .count();
            let fps = self.fps_smooth;
            let win = egui::Window::new("DEBUG — render mode")
                .open(&mut keep)
                .resizable(true)
                .default_width(310.0)
                .min_width(280.0)
                .min_height(180.0)
                .default_pos(egui::pos2(20.0, 130.0));
            let win = self.apply_dock_pos("DEBUG — render mode", ctx, win);
            let resp = win.show(ctx, |ui| {
                ui.label(egui::RichText::new("Render mode").monospace().strong());
                // CPU / GPU / APX are one mutually-exclusive axis of
                // fidelity-vs-speed:
                //   CPU — egui painter (exact; slow on huge scenes).
                //   GPU — OpenGL instanced pipelines (all geom types).
                //   APX — every dobject a single dot, one instanced draw
                //         call (fastest; approximate). Selection/snap
                //         still use real geometry.
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.render_mode, RenderMode::Cpu, "CPU (egui painter)");
                });
                ui.horizontal(|ui| {
                    ui.radio_value(
                        &mut self.render_mode,
                        RenderMode::Gpu,
                        "GPU (OpenGL pipelines)",
                    );
                });
                ui.horizontal(|ui| {
                    ui.radio_value(
                        &mut self.render_mode,
                        RenderMode::Apx,
                        "APX (draft — all dots)",
                    );
                });
                ui.separator();
                ui.monospace(format!("FPS         {:>6.1}", fps));
                ui.monospace(format!("dobjects    {:>6}", dobject_count));
                ui.monospace(format!("  circles   {:>6}", circle_count));
                ui.monospace(format!(
                    "  other     {:>6}",
                    dobject_count.saturating_sub(circle_count)
                ));
                ui.separator();
                ui.label(egui::RichText::new("Notes").small());
                ui.small("• GPU: instanced pipelines — circles/arcs/ellipses (analytic),");
                ui.small("  lines/splines/polylines (batched), wall poché + hatch fills");
                ui.small("• APX: every dobject renders as a single instanced dot");
                ui.small("• Selection / snap / hit-test always use real geometry");
            });
            self.process_dock_after_show("DEBUG — render mode", ctx, resp);
            if !keep {
                self.debug_open = false;
            }
            if self.render_mode != mode_before {
                self.history
                    .push(format!("  render mode → {:?}", self.render_mode));
                self.touch_view();
            }
        }

        // ---- snap settings window -----------------------------------------
        // (Object-snap settings are now a popup anchored to the status-bar
        // "dsnap" button — see render of the bottom status bar.)

        // ---- User-Environment Settings window (registry-driven) ------------
        self.mark("toolbar");
        self.settings_window(ctx);

        // ---- arc method picker ----------------------------------------------
        if self.arc_picker_open {
            let mut keep = true;
            egui::Window::new("ARC CREATION METHODS")
                .open(&mut keep)
                .resizable(false)
                .collapsible(false)
                .default_pos(egui::pos2(20.0, 80.0))
                .show(ctx, |ui| {
                    ui.set_min_width(310.0);
                    let mut chosen: Option<ArcMethod> = None;
                    for (i, &m) in ALL_ARC_METHODS.iter().enumerate() {
                        // visually group: 1 alone, 2-4 S,C,*, 5-7 S,E,*, 8-10 C,S,*, 11 Continue
                        if i == 1 || i == 4 || i == 7 || i == 10 {
                            ui.separator();
                        }
                        if arc_method_row(ui, self.arc_method, m) {
                            chosen = Some(m);
                        }
                    }
                    if let Some(m) = chosen {
                        self.arc_method = m;
                        self.tool = Tool::Arc;
                        self.pending.clear();
                        self.arc_picker_open = false;
                    }
                });
            if !keep {
                self.arc_picker_open = false;
            }
        }

        // ---- array dialog --------------------------------------------------
        // The dialog uses the STANDARD selection-basket flow for picking
        // sources: "Select sources ↓" begins a SelectMode::ForSelect
        // session with QueuedOp::Array. The array dialog HIDES during
        // that session (`self.select_mode != SelectMode::Off` is the
        // gate); user clicks dobjects (basket grows, dashed-gray
        // rendering), presses Enter; QueuedOp::Array's finalise handler
        // re-shows this dialog. Multi-source: every source goes into
        // every grid cell.
        let in_array_pick = self.array_open
            && self.queued_op == QueuedOp::Array
            && self.select_mode != SelectMode::Off;
        if self.array_open && !in_array_pick {
            {
                let mut do_generate = false;
                let mut close_it = false;
                let mut start_pick = false;
                let sources: Vec<(usize, String)> = self
                    .selection
                    .iter()
                    .filter_map(|&i| self.doc.dobjects.get(i).map(|d| (i, describe(&d.geom))))
                    .collect();
                egui::Window::new("Array")
                    .resizable(false)
                    .collapsible(false)
                    .show(ctx, |ui| {
                        ui.set_min_width(360.0);
                        ui.label("Duplicates the selected dobject(s).");
                        ui.horizontal(|ui| {
                            for (m, name) in [
                                (ArrayMethod::Linear, "Linear"),
                                (ArrayMethod::Polar, "Polar"),
                                (ArrayMethod::Path, "Path"),
                            ] {
                                if ui.selectable_label(self.array_method == m, name).clicked() {
                                    self.array_method = m;
                                    self.array_pick_center = false;
                                    self.array_pick_path = false;
                                }
                            }
                        });
                        ui.separator();

                        // Source row: "Select sources" button + count + first-source preview
                        ui.horizontal(|ui| {
                            if ui
                                .button("Select sources ↓")
                                .on_hover_text(
                                    "Begin a selection session — click dobjects, Enter to finish",
                                )
                                .clicked()
                            {
                                start_pick = true;
                            }
                            if sources.is_empty() {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 140, 140),
                                    "no sources selected",
                                );
                            } else {
                                ui.label(format!("{} source(s):", sources.len()));
                            }
                        });
                        if !sources.is_empty() {
                            egui::ScrollArea::vertical()
                                .id_salt("array_sources")
                                .max_height(80.0)
                                .show(ui, |ui| {
                                    for (i, d) in &sources {
                                        ui.monospace(format!("  #{} {}", i, d));
                                    }
                                });
                        }
                        ui.separator();

                        ui.horizontal(|ui| {
                            ui.label("columns");
                            // Count fields are integers: `3*3` commits, `2.5*2`
                            // does not (no silent rounding).
                            let calc = &self.calc;
                            ui.add(
                                egui::DragValue::new(&mut self.array_cols)
                                    .update_while_editing(false)
                                    .range(1..=3000_usize)
                                    .speed(1)
                                    .custom_parser(move |s| {
                                        crate::calc::parse_drag_int(calc, s, 1, 3000)
                                            .map(|n| n as f64)
                                    }),
                            );
                            ui.label("× rows");
                            let calc = &self.calc;
                            ui.add(
                                egui::DragValue::new(&mut self.array_rows)
                                    .update_while_editing(false)
                                    .range(1..=3000_usize)
                                    .speed(1)
                                    .custom_parser(move |s| {
                                        crate::calc::parse_drag_int(calc, s, 1, 3000)
                                            .map(|n| n as f64)
                                    }),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("dx");
                            let calc = &self.calc;
                            ui.add(
                                egui::DragValue::new(&mut self.array_dx)
                                    .update_while_editing(false)
                                    .speed(1.0)
                                    .custom_parser(move |s| crate::calc::parse_drag(calc, s)),
                            );
                            ui.label("    dy");
                            let calc = &self.calc;
                            ui.add(
                                egui::DragValue::new(&mut self.array_dy)
                                    .update_while_editing(false)
                                    .speed(1.0)
                                    .custom_parser(move |s| crate::calc::parse_drag(calc, s)),
                            );
                        });
                        // ---- per-method parameters ----
                        if self.array_method == ArrayMethod::Polar {
                            ui.horizontal(|ui| {
                                ui.label("center X");
                                let calc = &self.calc;
                                let mut cx = self.array_center.x;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut cx)
                                            .update_while_editing(false)
                                            .speed(1.0)
                                            .custom_parser(move |s| {
                                                crate::calc::parse_drag(calc, s)
                                            }),
                                    )
                                    .changed()
                                {
                                    self.array_center.x = cx;
                                }
                                ui.label("Y");
                                let calc = &self.calc;
                                let mut cy = self.array_center.y;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut cy)
                                            .update_while_editing(false)
                                            .speed(1.0)
                                            .custom_parser(move |s| {
                                                crate::calc::parse_drag(calc, s)
                                            }),
                                    )
                                    .changed()
                                {
                                    self.array_center.y = cy;
                                }
                                if ui
                                    .button(if self.array_pick_center {
                                        "Picking…"
                                    } else {
                                        "Pick ⌖"
                                    })
                                    .clicked()
                                {
                                    self.array_pick_center = !self.array_pick_center;
                                    if self.array_pick_center {
                                        self.set_prompt(
                                            "array (polar): click the CENTRE point  [Esc cancels]",
                                        );
                                    } else {
                                        self.clear_prompt();
                                    }
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("items");
                                let calc = &self.calc;
                                ui.add(
                                    egui::DragValue::new(&mut self.array_count)
                                        .update_while_editing(false)
                                        .range(2..=4096_usize)
                                        .speed(1)
                                        .custom_parser(move |s| {
                                            crate::calc::parse_drag_int(calc, s, 2, 4096)
                                                .map(|n| n as f64)
                                        }),
                                );
                                ui.label("fill°");
                                let calc = &self.calc;
                                ui.add(
                                    egui::DragValue::new(&mut self.array_fill_deg)
                                        .update_while_editing(false)
                                        .speed(1.0)
                                        .range(-360.0..=360.0)
                                        .custom_parser(move |s| crate::calc::parse_drag(calc, s)),
                                );
                            });
                            ui.checkbox(&mut self.array_rotate_items, "rotate items");
                        }
                        if self.array_method == ArrayMethod::Path {
                            ui.horizontal(|ui| {
                                let picked = self
                                    .array_path_idx
                                    .map(|pi| self.doc.dobjects.get(pi).is_some())
                                    .unwrap_or(false);
                                if ui
                                    .add_enabled(
                                        !self.array_pick_path,
                                        egui::Button::new(if picked {
                                            "Re-pick path ✓"
                                        } else {
                                            "Pick path ⌖"
                                        }),
                                    )
                                    .clicked()
                                {
                                    self.array_pick_path = true;
                                    self.set_prompt(
                                        "array (path): click the path CURVE  [Esc cancels]",
                                    );
                                }
                                if self.array_pick_path {
                                    ui.label("picking…");
                                }
                            });
                            ui.checkbox(&mut self.array_path_measure, "measure (fixed distance)");
                            if self.array_path_measure {
                                ui.horizontal(|ui| {
                                    ui.label("distance");
                                    let calc = &self.calc;
                                    ui.add(
                                        egui::DragValue::new(&mut self.array_path_dist)
                                            .update_while_editing(false)
                                            .speed(1.0)
                                            .range(0.001..=1e6)
                                            .custom_parser(move |s| {
                                                crate::calc::parse_drag(calc, s)
                                            }),
                                    );
                                });
                                ui.horizontal(|ui| {
                                    for (a, name) in [
                                        (PathAnchor::Start, "Left"),
                                        (PathAnchor::Middle, "Middle"),
                                        (PathAnchor::End, "Right"),
                                    ] {
                                        if ui
                                            .selectable_label(self.array_path_anchor == a, name)
                                            .clicked()
                                        {
                                            self.array_path_anchor = a;
                                        }
                                    }
                                });
                            } else {
                                ui.horizontal(|ui| {
                                    ui.label("items");
                                    let calc = &self.calc;
                                    ui.add(
                                        egui::DragValue::new(&mut self.array_count)
                                            .update_while_editing(false)
                                            .range(2..=4096_usize)
                                            .speed(1)
                                            .custom_parser(move |s| {
                                                crate::calc::parse_drag_int(calc, s, 2, 4096)
                                                    .map(|n| n as f64)
                                            }),
                                    );
                                });
                            }
                            ui.checkbox(&mut self.array_path_align, "align to path");
                        }
                        // ---- count preview ----
                        let (cells, new_dobjects) = match self.array_method {
                            ArrayMethod::Linear => {
                                let c = self.array_cols * self.array_rows;
                                (c, c.saturating_sub(1) * sources.len().max(1))
                            }
                            ArrayMethod::Polar => (
                                self.array_count,
                                self.array_count.saturating_sub(1) * sources.len().max(1),
                            ),
                            ArrayMethod::Path => {
                                let path_items = self
                                    .array_path_idx
                                    .and_then(|pi| {
                                        self.doc.dobjects.get(pi).map(|d| d.geom.clone())
                                    })
                                    .map(|g| self.path_array_placements(&g).len())
                                    .unwrap_or(0);
                                (path_items, path_items * sources.len().max(1))
                            }
                        };
                        let total_after = self.doc.dobjects.len() + new_dobjects;
                        ui.label(format!(
                            "{} cell(s) × {} source(s) = {} new dobjects → {} total",
                            cells,
                            sources.len(),
                            new_dobjects,
                            total_after
                        ));
                        if total_after > 1500 {
                            ui.colored_label(
                                egui::Color32::from_rgb(255, 200, 80),
                                "• intersection recompute will be skipped above ~1500 (O(N²))",
                            );
                        }
                        if total_after > 50_000 {
                            ui.colored_label(
                                egui::Color32::from_rgb(255, 140, 140),
                                "• rendering above ~50k dobjects may lag (CPU painter)",
                            );
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!sources.is_empty(), egui::Button::new("Generate"))
                                .clicked()
                            {
                                do_generate = true;
                            }
                            if ui.button("Close").clicked() {
                                close_it = true;
                            }
                        });
                    });
                if start_pick {
                    // Begin a fresh selection session for picking
                    // sources. The basket may already hold whatever
                    // the user selected before opening the dialog —
                    // we DON'T clear it, so prior selections carry
                    // over (user can shift-click to remove).
                    self.queued_op = QueuedOp::Array;
                    self.begin_selection(SelectMode::ForSelect);
                    self.set_prompt(
                        "array: pick source dobject(s), Enter to finish  [Esc=cancel]".to_string(),
                    );
                }
                if do_generate {
                    match self.array_method {
                        ArrayMethod::Linear => self.generate_array(),
                        ArrayMethod::Polar => self.generate_polar_array(),
                        ArrayMethod::Path => self.generate_path_array(),
                    }
                    self.array_pick_center = false;
                    self.array_pick_path = false;
                }
                if close_it {
                    self.array_open = false;
                }
            }
        }

        // ---- floating: Screen Stats (renderer's view of the doc) -------
        // Always called — it checks `screen_stats_open` and bails if
        // closed. Open by default since the user wanted to confirm
        // the app knows what's on screen.
        self.mark("settings");
        self.render_screen_stats_window(ctx);

        // ---- left panel: Layer dock (Slice B) ---------------------------
        if self.layer_panel_open {
            self.render_layer_panel(ctx);
        }

        // ---- left panel (further left): Pen palette (Slice C) -----------
        if self.pen_panel_open {
            self.render_pen_palette(ctx);
        }

        // ---- floating: Trim Debug log (instrumentation) ----------------
        if self.trim_debug_open {
            self.render_trim_debug_window(ctx);
        }

        // ---- floating: ACI color picker (polar wheel) ------------------
        // Renders only when a call site has set `aci_pick_request`.
        self.render_aci_picker_window(ctx);

        // ---- floating: Hatch attributes dialog -------------------------
        // Modal-ish (resizable=false, collapsible=false), opens when
        // bare `hatch` is typed; closes on OK / Cancel / X.
        self.render_hatch_dialog(ctx);
        self.render_text_style_dialog(ctx);
        self.render_save_failure(ctx);
        self.render_dim_style_manager(ctx);
        self.render_wall_style_manager(ctx);
        self.render_wall_style_dialog(ctx);
        self.render_block_dialog(ctx);
        self.render_insert_dialog(ctx);
        self.render_param_name_dialog(ctx);
        self.render_block_editor(ctx);
        self.render_file_dialog(ctx);
        self.render_raster_editor(ctx);
        self.render_param_panel(ctx); // parametric MODE constraint panel
        self.render_dim_style_dialog(ctx);
        self.render_text_input_dialog(ctx);
        self.render_py_console(ctx);
        // WP-SCRIPT slice 5 — in-app script editor + run-parameters dialog +
        // the scripting API reference window.
        self.render_py_editor(ctx);
        self.render_script_param_dialog(ctx);
        self.render_scripting_doc(ctx);
        // PLOT: dockable Page Setup + floating Plot dialog + preview + Plot
        // Style Table Editor + its sub-dialogs (ladder, New-CTB). The CTB table
        // cache is refreshed each frame (cheap fingerprint check).
        self.refresh_ctb_tables();
        self.render_new_ctb_dialog(ctx);
        if self.plot_dialog_open {
            self.render_plot_dialog(ctx);
        }
        if self.units_dialog_open {
            self.render_units_dialog(ctx);
        }
        if self.qselect.open {
            self.render_qselect_dialog(ctx);
        }
        self.render_attedit_dialog(ctx);
        if self.point_style_picker_open {
            self.render_point_style_picker(ctx);
        }
        if self.laywalk_open {
            let mut keep = true;
            egui::Window::new("LAYER WALK")
                .order(egui::Order::Foreground)
                .open(&mut keep)
                .resizable(true)
                .default_pos(egui::pos2(240.0, 120.0))
                .default_size(egui::vec2(280.0, 360.0))
                .show(ctx, |ui| {
                    ui.set_min_width(260.0);
                    ui.label(
                        egui::RichText::new("Click a layer to preview it in isolation").size(11.0),
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Filter");
                        ui.text_edit_singleline(&mut self.layer_search);
                    });
                    ui.add_space(4.0);
                    let search = self.layer_search.to_ascii_lowercase();
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            let rows: Vec<(usize, String, bool, bool)> = self
                                .doc
                                .layers
                                .layers
                                .iter()
                                .enumerate()
                                .filter(|(_, l)| {
                                    search.is_empty()
                                        || l.name.to_ascii_lowercase().contains(&search)
                                })
                                .map(|(id, l)| (id, l.name.clone(), l.visible, l.frozen))
                                .collect();
                            for (id, name, vis, frozen) in rows {
                                let label = format!(
                                    "{}  [{}]{}",
                                    name,
                                    if vis { "on" } else { "off" },
                                    if frozen { " \u{2744}" } else { "" }
                                );
                                if ui
                                    .selectable_label(id == self.laywalk_current, label)
                                    .clicked()
                                {
                                    self.laywalk_current = id;
                                    // Isolate: freeze every OTHER layer; the
                                    // restore snapshot heals on Close.
                                    for (i, ll) in self.doc.layers.layers.iter_mut().enumerate() {
                                        ll.frozen = i != id;
                                    }
                                    self.gpu_dirty = true;
                                }
                            }
                        });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Apply (keep)").clicked() {
                            self.laywalk_restore = None;
                            self.laywalk_open = false;
                            self.clear_prompt();
                        }
                        if ui.button("Close (restore)").clicked() {
                            if let Some(states) = self.laywalk_restore.take() {
                                for (l, (vis, fro)) in
                                    self.doc.layers.layers.iter_mut().zip(states.iter())
                                {
                                    l.visible = *vis;
                                    l.frozen = *fro;
                                }
                                self.gpu_dirty = true;
                            }
                            self.laywalk_open = false;
                            self.clear_prompt();
                        }
                    });
                });
            if !keep {
                self.laywalk_open = false;
            }
        }
        if self.plot_preview_open {
            self.render_plot_preview_window(ctx);
        }
        self.render_plot_style_editor(ctx);
        self.render_plot_ladder_dialog(ctx);
        self.render_pagesetup_dialog(ctx);
        self.render_dbg_recorder_window(ctx);
        self.mark("dialogs");
        // FRAME-END STATE POLL — capture every transition that wasn't
        // explicitly wired. Tools, state machines, window open/close,
        // SYSVAR flips, undo depth — all get StateChange/ToolChange/
        // WindowToggle events automatically. Agent-inspector behaviour.
        // Promote any pending GestureClassification — the click
        // handlers in the canvas update have now had their turn, so
        // we can read the resulting selection/state delta + classify.
        self.dbg_emit_pending_gesture();
        self.dbg_poll_state();
        // Auto-cadence doc snapshot — fires AFTER the per-frame UI
        // has had its turn so any mutations are already recorded.
        self.dbg_maybe_auto_snap();
        self.render_hatch_confirm_panel(ctx);

        // ---- floating: Hatch Debug Log (instrumentation) ---------------
        if self.hatch_debug_open {
            self.render_hatch_debug_window(ctx);
        }

        // ---- DObjects palette — floating Window -------------------------
        let mut dobjects_open = self.dobjects_window_open;
        let dobjects_count = self.doc.dobjects.len();
        let win = egui::Window::new(format!("DObjects ({})", dobjects_count))
            .open(&mut dobjects_open)
            .default_pos(egui::pos2(ctx.screen_rect().right() - 320.0, 70.0))
            .default_size(egui::vec2(300.0, 520.0))
            .min_width(220.0)
            .resizable(true)
            .collapsible(true);
        let win = self.apply_dock_pos("DObjects", ctx, win);
        let resp = win.show(ctx, |ui| {
            if self.picking_source {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 220, 100),
                    "PICK MODE — click any dobject below or on the canvas",
                );
            }
            // Virtual scrolling — only renders rows actually on screen, so the
            // list cost is bounded by visible_rows, not by dobject count.
            let row_h = ui.text_style_height(&egui::TextStyle::Body);
            let dobject_count = self.doc.dobjects.len();
            let mut to_delete: Option<usize> = None;
            egui::ScrollArea::vertical()
                .id_salt("ent_scroll")
                .max_height(ui.available_height() * 0.55)
                .auto_shrink([false; 2])
                .show_rows(ui, row_h, dobject_count, |ui, range| {
                    for i in range {
                        let label = format!("#{:>6}  {}", i, describe(&self.doc.dobjects[i].geom));
                        ui.horizontal(|ui| {
                            let resp = ui.selectable_label(self.selected == Some(i), label);
                            if resp.clicked() {
                                self.selected = Some(i);
                                if self.picking_source {
                                    self.picking_source = false;
                                }
                            }
                            if ui.small_button("✕").clicked() {
                                to_delete = Some(i);
                            }
                        });
                    }
                });
            if let Some(i) = to_delete {
                self.doc.dobjects.remove(i);
                self.selected = None;
                self.intersections.clear();
                self.index_dirty = true;
            }

            ui.separator();
            ui.heading(format!("Intersections ({})", self.intersections.len()));
            egui::ScrollArea::vertical()
                .id_salt("int_scroll")
                .show(ui, |ui| {
                    for (i, p) in self.intersections.iter().enumerate() {
                        ui.monospace(format!("{:>3}  ({:>10.4}, {:>10.4})", i, p.x, p.y));
                    }
                });
        });
        self.raise_after_show("DObjects", ctx, &resp);
        self.process_dock_after_show("DObjects", ctx, resp);
        self.dobjects_window_open = dobjects_open;

        // ---- UI.1: STATUS BAR (very bottom) -----------------------------
        // Declared BEFORE the cmd panel so it sits at the absolute bottom
        // edge; cmd panel ends up above it (egui stacks bottoms inward).
        egui::TopBottomPanel::bottom("status_bar")
            .exact_height(22.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // ---- LEFT: cursor world coords -----------------------
                    let cursor_world = ctx.input(|i| i.pointer.hover_pos())
                        .and_then(|p| {
                            // Translate from screen to world via the same
                            // formula as self.s2w (we don't have `rect`
                            // here, so reproduce it from ctx).
                            let r = ctx.screen_rect();
                            let c = r.center();
                            Some(Vec2::new(
                                ((p.x - c.x) / self.scale - self.world_offset.x) as f64,
                                (-(p.y - c.y) / self.scale - self.world_offset.y) as f64,
                            ))
                        });
                    let coord_text = match cursor_world {
                        Some(w) => format!("{:>11.4}, {:>11.4}", w.x, w.y),
                        None    => format!("{:>11}, {:>11}", "—", "—"),
                    };
                    ui.label(egui::RichText::new(coord_text)
                        .monospace()
                        .color(egui::Color32::from_rgb(160, 200, 240)));

                    ui.separator();

                    // ---- ACTIVE LAYER ------------------------------------
                    let active_layer_name = self.doc.layers.get(self.doc.layers.active)
                        .map(|l| l.name.as_str()).unwrap_or("?").to_string();
                    let active_layer_col = self.doc.layers.get(self.doc.layers.active)
                        .map(|l| {
                            let (r, g, b) = resolve_color(
                                l.color, self.doc.layers.active,
                                &self.doc.layers, &self.doc.truecolors);
                            egui::Color32::from_rgb(r, g, b)
                        })
                        .unwrap_or(egui::Color32::WHITE);
                    let (swatch_rect, _) = ui.allocate_exact_size(
                        egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().rect_filled(swatch_rect, 1.0, active_layer_col);
                    ui.painter().rect_stroke(swatch_rect, 1.0,
                        egui::Stroke::new(0.6, egui::Color32::from_rgb(60, 70, 85)));
                    ui.label(egui::RichText::new(format!("Layer: {}", active_layer_name))
                        .monospace().small());

                    ui.separator();

                    // ---- SELECTION COUNT ---------------------------------
                    let sel_n = self.selection.len();
                    if sel_n > 0 {
                        ui.label(egui::RichText::new(format!("{} sel", sel_n))
                            .monospace().small()
                            .color(egui::Color32::from_rgb(180, 220, 100)));
                        ui.separator();
                    }

                    // ---- RIGHT-aligned controls --------------------------
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Zoom level (rightmost)
                        ui.label(egui::RichText::new(
                            format!("zoom {:.2}× ({:.2} px/u)",
                                self.scale, self.scale)
                        ).monospace().small().color(egui::Color32::from_rgb(150, 165, 185)));
                        ui.separator();
                        // ---- Render-mode badges: [CPU] [GPU] [APX] ----
                        // Three INDEPENDENT, directly-selectable render-mode
                        // badges (NOT a cycling toggle): click CPU / GPU / APX
                        // to jump straight to that mode. Direct-select matters
                        // because CPU can bog down on a very heavy drawing — you
                        // want to switch STRAIGHT to GPU, not cycle through the
                        // slow CPU frame to reach it.
                        let mode_badge = |ui: &mut egui::Ui, label: &str, active: bool, tip: &str| {
                            let col = if active {
                                egui::Color32::from_rgb(255, 200, 80)   // warm amber when active
                            } else {
                                egui::Color32::from_rgb(80, 90, 105)
                            };
                            let resp = ui.add(egui::Label::new(
                                egui::RichText::new(label).monospace().small().strong().color(col)
                            ).sense(egui::Sense::click()));
                            resp.on_hover_text(tip).clicked()
                        };
                        ui.label(egui::RichText::new("render").small()
                            .color(egui::Color32::from_rgb(120, 130, 145)));
                        if mode_badge(ui, "CPU", self.render_mode == RenderMode::Cpu,
                            "CPU render — egui painter, full fidelity. Bogs down on \
                             very heavy drawings, so it's CAPPED per frame for safety \
                             (switch to GPU/APX to see everything).")
                        {
                            self.set_render_mode(RenderMode::Cpu);
                        }
                        if mode_badge(ui, "GPU", self.render_mode == RenderMode::Gpu,
                            "GPU render — our OpenGL instanced pipelines (fast; egui \
                             fallback for not-yet-ported fills). Best for heavy drawings.")
                        {
                            self.set_render_mode(RenderMode::Gpu);
                        }
                        if mode_badge(ui, "APX", self.render_mode == RenderMode::Apx,
                            "APX render — every dobject a single dot, one draw call. \
                             Fastest, approximate; selection/snap still use real geometry.")
                        {
                            self.set_render_mode(RenderMode::Apx);
                        }
                        ui.separator();
                        // EdgMod toggle
                        let mut em = self.env.EdgMod;
                        if ui.checkbox(&mut em, "EdgMod").changed() {
                            self.env.EdgMod = em;
                            let _ = self.env.save();
                        }
                        ui.separator();
                        // GrpEnb toggle
                        let mut ge = self.env.GrpEnb;
                        if ui.checkbox(&mut ge, "Grips").changed() {
                            self.env.GrpEnb = ge;
                            let _ = self.env.save();
                        }
                        ui.separator();
                        // DSNAP — opens the object-snap menu as a popup right
                        // above the button; click-outside closes it.
                        let dsnap = ui.button("dsnap")
                            .on_hover_text("DObject-snap (DSNAP) settings");
                        let snap_popup = ui.make_persistent_id("dsnap_popup");
                        if dsnap.clicked() { ui.memory_mut(|m| m.toggle_popup(snap_popup)); }
                        egui::popup_above_or_below_widget(ui, snap_popup, &dsnap,
                            egui::AboveOrBelow::Above,
                            egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                                ui.set_min_width(258.0);
                                ui.label("Snaps found automatically while you hover:");
                                ui.separator();
                                for k in SnapKind::ALL {
                                    let mut on = self.snap_enabled.is_enabled(k);
                                    let label = format!("{:<5}  {}", k.name(), snap_blurb(k));
                                    if ui.checkbox(&mut on, label).changed() {
                                        self.snap_enabled.set(k, on);
                                    }
                                }
                                ui.separator();
                                ui.horizontal(|ui| {
                                    ui.label("search radius");
                                    ui.add(egui::Slider::new(&mut self.env.SpTGSZ, 4..=80)
                                        .suffix(" px"));
                                });
                                ui.horizontal(|ui| {
                                    if ui.button("All on").clicked() {
                                        for k in SnapKind::ALL { self.snap_enabled.set(k, true); }
                                    }
                                    if ui.button("All off").clicked() {
                                        self.snap_enabled = SnapSet::default();
                                    }
                                    if ui.button("Defaults").clicked() {
                                        self.snap_enabled = SnapSet::defaults();
                                    }
                                });
                                pp_cap_ui(ui, "object-snap (dsnap) popup");
                            });
                        ui.separator();
                        // Drafting-mode toggles: GRID (F7), SNAP (F9), ORTHO (F8).
                        // Clickable badges (same affordance as the osnap row).
                        let drafting_badge = |ui: &mut egui::Ui, label: &str, on: bool, tip: &str| {
                            let col = if on {
                                egui::Color32::from_rgb(120, 240, 255)
                            } else {
                                egui::Color32::from_rgb(80, 90, 105)
                            };
                            let resp = ui.add(egui::Label::new(
                                egui::RichText::new(label).monospace().small().color(col)
                            ).sense(egui::Sense::click()));
                            resp.on_hover_text(tip).clicked()
                        };
                        if drafting_badge(ui, "CARD", self.env.CrdEnb,
                            "CARD (F8, or type `card`) — cursor pulled to horizontal or vertical from the anchor point.")
                        {
                            self.env.CrdEnb = !self.env.CrdEnb;
                            let _ = self.env.save();
                        }
                        if drafting_badge(ui, "SNAP", self.env.GrdSnp,
                            "Snap to grid (F9) — cursor rounds to the nearest GrdSpc multiple.")
                        {
                            self.env.GrdSnp = !self.env.GrdSnp;
                            let _ = self.env.save();
                        }
                        if drafting_badge(ui, "GRID", self.env.GrdEnb,
                            "Show grid (F7) — dots at GrdSpc world-unit intervals.")
                        {
                            self.env.GrdEnb = !self.env.GrdEnb;
                            let _ = self.env.save();
                        }
                        if drafting_badge(ui, "UCS", self.env.UcsIcn,
                            "UCS indicator (UcsIcn) — origin marker with X/Y axes. \
                             Anchors at world (0,0) when visible, else pins to the \
                             bottom-left corner as a reference.")
                        {
                            self.env.UcsIcn = !self.env.UcsIcn;
                            let _ = self.env.save();
                        }
                        // Gated on there being a MODEL, not on the 3D panel being open: the whole
                        // point is to see the 3D work while drawing in 2D, and the panel is
                        // routinely closed to make room for the canvas.
                        let has_model = !self.factory.model.features.is_empty()
                            || !self.factory.furniture.is_empty();
                        // STILL CALLED FURN. It was briefly renamed "3D" when it grew from
                        // furniture-only to the whole model, and the rename cost more than the
                        // accuracy gained — "what happened to furn where we could turn on or off to
                        // see the furnitures in 2d". The name people look for wins; the tooltip
                        // carries what it actually does now.
                        if has_model && drafting_badge(ui, "FURN", self.factory.show_furniture_outlines_2d,
                            "Show the 3D model on the 2D plan — furniture, buildings, rooms, \
                             apertures and architecture, as footprints you can trace and snap to.")
                        {
                            self.factory.show_furniture_outlines_2d = !self.factory.show_furniture_outlines_2d;
                        }
                        // The 3D view's own ground grid. Gated on the PANEL being open rather than
                        // on there being a model: an empty scene is exactly when the grid is the
                        // only thing saying where the ground is.
                        if self.factory.open
                            && drafting_badge(ui, "GRID3D", self.factory.show_grid,
                                "Ground grid in the 3D view. Follows the camera and steps its \
                                 spacing with the zoom, so it covers the whole view at any scale.")
                        {
                            self.factory.show_grid = !self.factory.show_grid;
                        }
                        // The flat ground surface under the model — the rooms sit on something
                        // instead of floating in the void. Drawn with the grid, in the same
                        // colour family, so the two read as one ground plane.
                        if self.factory.open
                            && drafting_badge(ui, "GND", self.factory.show_ground,
                                "Flat ground surface at z = 0 under the model. Follows the \
                                 camera like the grid, catches the model's shadows, and turns \
                                 off with this badge.")
                        {
                            self.factory.show_ground = !self.factory.show_ground;
                        }
                        // The world-origin axes. Off by default and separate from the grid: one is
                        // a working surface, the other a reference point.
                        if self.factory.open
                            && drafting_badge(ui, "ORG", self.factory.show_origin,
                                "Three-axis gizmo at the world origin (0, 0, 0) — the point the \
                                 `origin` and `@X,Y,Z` placement modes measure from.")
                        {
                            self.factory.show_origin = !self.factory.show_origin;
                        }
                        // 3D object snap. The 2D snap badges to the right drive `snap_enabled`,
                        // which the 3D side has never read — it has its own, and until now no way
                        // to turn it off.
                        if self.factory.open
                            && drafting_badge(ui, "SNAP3D", self.factory.snap_3d,
                                "3D object snap: clicks in the 3D view land on the nearest solid \
                                 CORNER within 12 px. Off = the raw point under the cursor.")
                        {
                            self.factory.snap_3d = !self.factory.snap_3d;
                        }
                        ui.separator();
                        // Snap badges — click any letter to toggle.
                        for k in SnapKind::ALL {
                            let on = self.snap_enabled.is_enabled(k);
                            let label = k.name();   // "END", "MID", …
                            let col = if on {
                                egui::Color32::from_rgb(120, 240, 255)
                            } else {
                                egui::Color32::from_rgb(80, 90, 105)
                            };
                            let resp = ui.add(egui::Label::new(
                                egui::RichText::new(label).monospace().small().color(col),
                            ).sense(egui::Sense::click()));
                            if resp.clicked() {
                                self.snap_enabled.set(k, !on);
                            }
                            if resp.hovered() {
                                resp.on_hover_text(snap_blurb(k));
                            }
                        }
                    });
                });
            });

        // ---- left mode command panel + right Inspector dock — added BEFORE the
        // command bar so the panel + dock span full height (menu → status) and
        // the command bar stays confined to the CENTER column (WORKSPACE_SYSTEM).
        // The dockable DRAW/MODIFY icon rails are GONE — their commands are all
        // listed (with names and icons) in the mode command panel, so the
        // vertical bars are not drawn in any workspace.
        self.render_mode_command_panel(ctx);
        if self.info_panel_open {
            self.render_info_panel(ctx);
        }
        // Is the answer on screen still an answer about THIS building? Asked before anything draws
        // it, and throttled inside — see `LightState::refresh_staleness`.
        // The fixtures follow their symbols before anything asks whether the answer is still true —
        // a light that has just been rotated on the plan must read as a change, not as unchanged.
        self.tick_symbol_sync();
        self.mark("statusbar");
        self.refresh_light_staleness();
        self.mark("staleness");
        // SIMLUX Light panel (SIMLUX menu ▸ Light panel) — bails if closed.
        self.render_light_panel(ctx);
        self.mark("lightpanel");
        // Illuminaire (SIMLUX menu ▸ Illuminaire) — the fitting library. Bails if closed.
        self.render_illuminaire(ctx);
        self.render_report_dialog(ctx);
        // The calculation runs on a worker; this collects it and shows how it is getting on.
        self.poll_calculation(ctx);
        // ---- Command bar — a bottom strip docked directly above the status bar
        // (which is bottommost). No float / drag / dock host (upstream parity):
        // user-resizable height by dragging the top edge; remembered across runs
        // (`env.CmdBarH`). A visual left grip (§2, no move handle) whose
        // right-click opens the sticky "Close command bar" flyout.
        //
        // A 2D DRAFTING SURFACE — the SIMLUX and 3D Factory workspaces are
        // viewports, and show no command line. `cmd_window_open` still records
        // the user's choice across tab switches (closing the bar in the 2D
        // workspace keeps it closed after a 3D visit), and the bar's stored
        // height is untouched by the frames it is absent from, so it returns
        // exactly as it was left. The 3D workspace viewports therefore run
        // full-width to the window's right edge AND full-height to the status
        // bar — no strip is reserved for typing there.
        if self.cmd_window_open && self.mode == Mode::Cad2D {
            use crate::theme::color as tc;
            // §7 height computed from the mono line height so CMD_HIST_LINES
            // whole lines fit; the bar is USER-RESIZABLE by dragging its top
            // edge, and a remembered height is restored on launch.
            let line_h = {
                let fid = ctx
                    .style()
                    .text_styles
                    .get(&egui::TextStyle::Monospace)
                    .cloned()
                    .unwrap_or_else(|| egui::FontId::monospace(13.0));
                ctx.fonts(|f| f.row_height(&fid))
            };
            let default_h = CMD_PAD_TOP
                + CMD_HIST_LINES as f32 * line_h
                + CMD_PAD_TOP
                + CMD_PILL_H
                + CMD_PAD_BELOW;
            // Min height = the pill + CMD_HIST_LINES whole history lines, so the
            // log can never shrink to a one-line sliver (the scroll area fills
            // whatever remains).
            let min_h = CMD_PAD_BELOW + CMD_PILL_H + CMD_PAD_TOP + CMD_HIST_LINES as f32 * line_h;
            // No upper cap: the bar can grow to any height the window allows.
            let bar_h = if self.env.CmdBarH > 0.0 {
                self.env.CmdBarH.max(min_h)
            } else {
                default_h
            };
            let cmd_panel = egui::TopBottomPanel::bottom("command")
                // §7 resize: a drag handle on the top edge; the panel keeps its
                // size for the session, and the final height is persisted below.
                .resizable(true)
                .default_height(bar_h)
                .min_height(min_h)
                // No egui separator line — the top edge gets a soft drop shadow
                // + our own painted resize handle instead.
                .show_separator_line(false)
                .frame(egui::Frame::none().fill(tc::SURFACE_1))
                .show(ctx, |ui| {
                    let h = ui.available_height(); // known panel height for the body
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        // §2 visual grip strip (no drag / no move cursor).
                        let (grip_rect, gresp) =
                            ui.allocate_exact_size(egui::vec2(16.0, h), egui::Sense::click());
                        let p = ui.painter_at(grip_rect);
                        p.rect_filled(grip_rect, 0.0, tc::BORDER); // #34414B strip
                        let ppp = ui.ctx().pixels_per_point();
                        let snap = |v: f32| (v * ppp).round() / ppp;
                        let (ln_w, ln_h, pitch) = (5.0_f32, 1.5_f32, 3.0_f32);
                        let lx = snap(grip_rect.center().x - ln_w * 0.5);
                        let ty = snap(grip_rect.top() + 6.0); // pinned to TOP
                        for i in 0..5 {
                            let y = snap(ty + i as f32 * pitch);
                            p.rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(lx, y),
                                    egui::vec2(ln_w, ln_h),
                                ),
                                0.0,
                                tc::SURFACE_1,
                            );
                        }
                        if gresp.secondary_clicked() {
                            self.open_flyout(FlyMenu::CommandBar, grip_rect);
                        }
                        // Body fills the rest at the full (known) height and
                        // the FULL remaining width — the log scrollbar must
                        // sit at the panel's right edge, not mid-bar.
                        ui.vertical(|ui| {
                            ui.set_width(ui.available_width());
                            self.command_bar_body(ui, None);
                        });
                    });
                });
            // §7 resize: persist the new height when the user finishes dragging
            // the top handle (`__resize` is the panel's own resize-interaction
            // id, the same one egui reads inside `TopBottomPanel`).
            let resize_id = egui::Id::new("command").with("__resize");
            if let Some(r) = ctx.read_response(resize_id) {
                if r.drag_stopped() {
                    self.env.CmdBarH = cmd_panel.response.rect.height();
                    let _ = self.env.save();
                }
            }
            // Soft drop shadow on the command bar's canvas-facing (top) edge,
            // full width, on a Middle-order painter so the bands fall onto the
            // canvas above the bar.
            let cmd_p = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::new("command_bar_shadow"),
            ));
            paint_top_shadow(&cmd_p, cmd_panel.response.rect, 6);
            // §7 resize affordance: a painted handle ON the bar's top edge, ABOVE
            // the shadow. Dim 2px by default; a thicker ACCENT bar while
            // hovered/dragging.
            let edge = cmd_panel.response.rect;
            let resizing = ctx
                .read_response(resize_id)
                .map(|r| r.hovered() || r.dragged())
                .unwrap_or(false);
            let (hcol, hh) = if resizing {
                (tc::ACCENT, 3.0_f32)
            } else {
                (egui::Color32::from_rgb(0x46, 0x52, 0x5E), 2.0_f32)
            };
            cmd_p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(edge.left() + 1.0, edge.top() - 1.0),
                    egui::pos2(edge.right() - 1.0, edge.top() - 1.0 + hh),
                ),
                egui::Rounding::ZERO,
                hcol,
            );
        }

        // SIMLUX 3D viewport (docked right; reserves the right edge before Central).
        self.render_shortcuts_window(ctx);
        self.mark("illum+report");
        self.render_light_3d_panel(ctx);
        self.mark("simlux3d");
        // 3D FACTORY viewport (docked right, like the SIMLUX 3D view).
        self.render_factory_panel(ctx);
        self.mark("factory3d");
        self.render_draw3d_dialog(ctx);
        if self.arch_modal_open {
            self.render_arch_dialog(ctx);
        }
        if self.sun_modal_open {
            self.render_sun_dialog(ctx);
        }
        self.handle_dialog_window(ctx);
        // Both are top-level windows so they float over the Architecture panel instead of
        // reflowing it, and so the preview and the handle picker can be open together — changing
        // a handle with the preview up is the whole point of having both.
        if self.door_preview_window(ctx) {
            let inp = self.arch_door;
            self.factory_build_door(&inp);
        }
        if self.materials_open {
            self.render_materials_factory(ctx);
        }
        if self.render_modal_open || self.pt_job.is_some() || self.pt_gpu.is_some() {
            self.render_pathtrace_dialog(ctx);
        }
        self.render_radiance_dialog(ctx); // shown only while a Radiance run exists
        self.render_rename_plane_dialog(ctx); // shown only while a plane is being renamed
        self.render_unit_prompt_dialog(ctx); // once per project, the first time the Factory opens
                                             // A file carrying both an embedded project and a sidecar — which one loads?
        self.render_extra_choice_dialog(ctx);
        // Command palette (Phase 7) — registry-driven; also handles Ctrl+Shift+P.
        self.render_command_palette(ctx);
        self.render_menu_flyouts(ctx); // generalized top-level dropdown flyouts (§9)
                                       // Model/Layout tab strip (upstream parity) + the New Layout dialog.
        self.render_layout_tabs(ctx);
        if self.show_new_layout_dialog {
            self.render_new_layout_dialog(ctx);
        }
        // Viewport creation/scale + properties dialogs (paper space).
        if self.viewport_scale_dialog_open {
            self.render_viewport_scale_dialog(ctx);
        }
        self.render_vp_edit_dialog(ctx);

        // ---- central panel: canvas --------------------------------------
        //
        // ALWAYS SHOWN, even when the 2D view is "closed". egui's CentralPanel takes whatever the
        // side panels left, so skipping it does not give that space away — it just leaves a hole
        // that nothing paints. Closing the 2D view instead means it reserves no WIDTH (see
        // `MIN_VIEW_W`), and with both panels open across the whole window this is the sliver
        // between them, drawing nothing anyone can see.
        egui::CentralPanel::default().show(ctx, |ui| {
            let avail = ui.available_size();
            let (resp, painter) =
                ui.allocate_painter(avail, egui::Sense::click_and_drag());
            let rect = resp.rect;
            // ACTIVE VIEW — a press INSIDE the 2D canvas makes 2D active. Only real
            // interaction flips it; merely having the 3D panel open never does (that
            // was the bug that hijacked `m`). See `ActiveView`.
            if resp.is_pointer_button_down_on() || resp.clicked() || resp.dragged() {
                self.active_view = ActiveView::TwoD;
            }
            // Stash the canvas rect for the dock helpers — they need
            // the canvas area (below toolbar, above status bar) not
            // the full window rect.
            self.canvas_screen_rect = Some(rect);
            // ISOLATION: while the Block Editor is open, the main canvas must
            // not react to ANY pointer gesture (pan/zoom/select/grip/draw) —
            // all block editing happens inside that window. Every interaction
            // gate below is AND-ed with `!canvas_locked`. See `BlockEditor`.
            // Also parked while the Plot dialog's "Window" area is being
            // picked — the drag defines the plot region, it must NOT select.
            let canvas_locked = self.block_editor.is_some() || self.plot_win_pick != 0;
            // LAYOUT TAB: true while a layout (paper space) tab is active.
            let in_layout = self.doc.active_layout.is_some();
            // SIMLUX: placing / moving light points on the plan. Gates the primary click+drag
            // handlers only (see `simlux_pointer_2d`) — pan, zoom and the right-click menu stay
            // live, because laying out a lighting grid means moving around the drawing.
            let light_grab = !canvas_locked && self.simlux_pointer_2d(&resp, rect, ctx);
            // 3D FACTORY placement: an object added with "Click to place" is waiting to be told
            // where it goes, and the plan is a perfectly good place to say so — asked for as "a
            // place the user can click on the 3d or 2d window". Gated in alongside `light_grab` so
            // the placing click never also selects or starts drawing something.
            let place_grab = !canvas_locked && self.factory_placing_pointer_2d(&resp, rect);

            // ---- Plot "Window" area pick — rubber-band, no selection ----
            if self.plot_win_pick != 0 {
                if resp.drag_started() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        if rect.contains(pos) { self.plot_win_p1 = Some(self.s2w(pos, rect)); }
                    }
                }
                // Live preview: red rubber-band from the first corner to the cursor.
                if let (Some(p1), Some(cur)) = (self.plot_win_p1, resp.hover_pos().or_else(|| resp.interact_pointer_pos())) {
                    let a = self.w2s(p1, rect);
                    let rb = egui::Rect::from_two_pos(a, cur);
                    let lp = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Foreground, egui::Id::new("plot_win_rubber")));
                    lp.rect_filled(rb, 0.0, egui::Color32::from_rgba_unmultiplied(0xE5, 0x48, 0x4D, 26));
                    lp.rect_stroke(rb, 0.0, egui::Stroke::new(1.2, egui::Color32::from_rgb(0xE5, 0x48, 0x4D)));
                }
                if resp.drag_stopped() {
                    if let (Some(p1), Some(pos)) = (self.plot_win_p1, resp.interact_pointer_pos()) {
                        let p2 = self.s2w(pos, rect);
                        let mn = Vec2::new(p1.x.min(p2.x), p1.y.min(p2.y));
                        let mx = Vec2::new(p1.x.max(p2.x), p1.y.max(p2.y));
                        if (mx.x - mn.x) > 1e-3 && (mx.y - mn.y) > 1e-3 {
                            self.plot_window = Some((mn, mx));
                            self.plot_area_kind = 1;   // Window is the active area
                            self.history.push(format!(
                                "  plot: window ({:.2},{:.2})–({:.2},{:.2})", mn.x, mn.y, mx.x, mx.y));
                        }
                    }
                    // Done (or a zero-size drag) → unpark the dialog.
                    self.plot_win_pick = 0;
                    self.plot_win_p1 = None;
                    self.clear_prompt();
                    self.set_prompt("plot — set paper / area / scale in the dialog, then Plot");
                }
                return;
            }
            // ---- right-click shortcut (context) menu --------------------
            // Opens on a secondary CLICK (a right-DRAG still pans). Shown only
            // when there's a selection or something on the clipboard.
            if !canvas_locked
                && !in_layout
                && (!self.selection.is_empty() || !self.clipboard_dobjects.is_empty())
            {
                resp.context_menu(|ui| {
                    ui.set_min_width(150.0);
                    // Group ▸ (flyout submenu)
                    ui.menu_button("Group", |ui| {
                        if ui.button("Group     Ctrl+G").clicked() {
                            self.group_selection();
                            ui.close_menu();
                        }
                        if ui.button("Add to Group").clicked() {
                            self.add_to_group();
                            ui.close_menu();
                        }
                        if ui.button("Ungroup").clicked() {
                            self.ungroup_selection();
                            ui.close_menu();
                        }
                    });
                    // Clipboard ▸ (flyout submenu)
                    ui.menu_button("Clipboard", |ui| {
                        if ui.button("Copy      Ctrl+C").clicked() {
                            self.copy_selection();
                            let n = self.clipboard_dobjects.len();
                            ui.ctx().copy_text(format!(
                                "RUST-AutoRASM: {} object(s) on clipboard", n));
                            ui.close_menu();
                        }
                        if ui.button("Paste     Ctrl+V").clicked() {
                            self.start_paste();
                            ui.close_menu();
                        }
                    });
                    // DObject Snap ▸ (running-osnap toggles, same set as DSNAP)
                    ui.menu_button("DObject Snap", |ui| {
                        for k in SnapKind::ALL {
                            let mut on = self.snap_enabled.is_enabled(k);
                            let label = format!("{:<5}  {}", k.name(), snap_blurb(k));
                            if ui.checkbox(&mut on, label).changed() {
                                self.snap_enabled.set(k, on);
                            }
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("All on").clicked() {
                                for k in SnapKind::ALL { self.snap_enabled.set(k, true); }
                            }
                            if ui.button("All off").clicked() {
                                self.snap_enabled = SnapSet::default();
                            }
                            if ui.button("Defaults").clicked() {
                                self.snap_enabled = SnapSet::defaults();
                            }
                        });
                    });
                    // Make 3D wall — promote the selection into Factory solids. The
                    // practical wall journey: draft (or import) in 2D, then extrude here.
                    // Gated on the SAME predicate promotion uses, so the row never offers
                    // something promotion would refuse.
                    if self.can_make_3d_wall() {
                        ui.separator();
                        if ui
                            .button("⬒ Make 3D wall")
                            .on_hover_text(
                                "Extrude the selection into 3D Factory solids.\n\
                                 Walls use their own thickness; lines, polylines and arcs \
                                 (an imported or traced plan) use the current wall style's \
                                 thickness. Height comes from the Factory 'wall height'.",
                            )
                            .clicked()
                        {
                            self.do_make_3d_wall();
                            ui.close_menu();
                        }
                        // Floor / ceiling / building for the ACTIVE storey, from the
                        // selected closed outline. Offered only when there IS a closed
                        // outline — a slab or a mass needs a boundary, and an open path
                        // has none.
                        if self.slab_outline_from_selection().is_some() {
                            // MAKE BUILDING — the outline extruded as one solid mass.
                            // Mirrors the confirmed wall journey (draft → select →
                            // right-click), so no new command verb appears.
                            if ui
                                .button("⌂ Make building")
                                .on_hover_text(
                                    "Extrude the selected outline into one solid mass on \
                                     the active storey, rising to the Factory 'building \
                                     height'. Arbitrary shapes are exact.",
                                )
                                .clicked()
                            {
                                self.do_make_building();
                                ui.close_menu();
                            }
                            let lvl = self
                                .factory
                                .storeys
                                .get(self.factory.active_storey)
                                .map(|s| s.name.clone())
                                .unwrap_or_default();
                            for (label, is_floor) in
                                [("⬓ Make 3D floor", true), ("⬒ Make 3D ceiling", false)]
                            {
                                if ui
                                    .button(label)
                                    .on_hover_text(format!(
                                        "Slab across the selected outline, on storey '{lvl}'.\n\
                                         Non-rectangular outlines are approximated by their \
                                         bounding box until the extrusion primitive lands."
                                    ))
                                    .clicked()
                                {
                                    self.do_make_slab(is_floor);
                                    ui.close_menu();
                                }
                            }
                        }
                    }
                    ui.separator();
                    if ui.button("Inspector").clicked() {
                        self.info_window_open = true;
                        ui.close_menu();
                    }
                    pp_cap_ui(ui, "canvas right-click menu");
                });
            }
            // ZOOM real-time mode: a primary drag zooms the view instead of
            // selecting / drawing. Gates the click/grip handlers below so the
            // drag is consumed only by the zoom logic. See the zoom_* methods.
            let rt_zoom = !canvas_locked && self.zoom_state == ZoomState::RealTime;
            // Stash the live cursor (raw world) so the command line's
            // direct-distance entry knows which way to throw a typed
            // distance (CARD-locked when CARD is on).
            let lc = resp.hover_pos().map(|p| self.s2w(p, rect));
            if lc.is_some() { self.last_cursor_raw_world = lc; }

            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(18, 22, 28));

            // LAYOUT TAB: paper-space render (page outline + paper entities +
            // viewport windows of the model) replaces the model canvas below.
            if in_layout && !canvas_locked {
                self.paper_space_render(ui, rect, &painter, &resp);
            }
            if !in_layout {
            // ---- Reference raster underlays (behind everything) -----------
            self.sync_underlay_textures(ctx);
            self.draw_raster_underlays(&painter, rect);
            // A 3D-Factory face sketch shows the object it is drawn on as a faint reference.
            self.draw_factory_sketch_reference(&painter, rect);
            // All finished face-sketches, projected onto the plan — always visible.
            self.draw_factory_sketches_2d(&painter, rect);
            // Placed furniture footprints, projected onto the plan — toggled by the FURN badge.
            self.draw_furniture_outlines_2d(&painter, rect);
            // Room names, above the outlines so a label is never buried under furniture.
            self.paint_room_names_2d(&painter, rect);
            // SIMLUX: lux heatmap + luminaire markers on the 2D plan (overlay default off).
            self.paint_lux_overlay(&painter, rect);
            // WHILE A CALCULATION IS RUNNING, over the old result and under the fixtures: the
            // fixtures are what the user is looking at, and the wash says which ground the answer
            // being computed will cover.
            self.paint_calculating_zone(&painter, rect);
            self.paint_luminaires_2d(&painter, rect);
            }

            // ---- Background grid (GrdEnb) ---------------------------------
            //
            // Renders a dot grid at GrdSpc world-unit intervals across the
            // visible viewport. Skips entirely if disabled, if spacing is
            // non-positive, or if the spacing would produce > 50 000 dots
            // (zoomed too far out — would be a black smear anyway). Drawn
            // BEFORE dobjects so they sit on top of it. The same GrdSpc
            // value is what GrdSnp rounds to.
            let grid_s = self.grid_spacing();
            if self.env.GrdEnb && grid_s > 0.0 {
                let s = grid_s;
                let bl = self.s2w(rect.left_bottom(), rect);
                let tr = self.s2w(rect.right_top(),   rect);
                let x0 = (bl.x / s).floor() * s;
                let y0 = (bl.y / s).floor() * s;
                let x1 = (tr.x / s).ceil()  * s;
                let y1 = (tr.y / s).ceil()  * s;
                let cols = ((x1 - x0) / s).round() as i64 + 1;
                let rows = ((y1 - y0) / s).round() as i64 + 1;
                if cols > 0 && rows > 0 && (cols * rows) < 50_000 {
                    let dot_col = egui::Color32::from_rgb(60, 70, 85);
                    let mut y = y0;
                    while y <= y1 + s * 0.5 {
                        let mut x = x0;
                        while x <= x1 + s * 0.5 {
                            let p = self.w2s(Vec2::new(x, y), rect);
                            painter.circle_filled(p, 0.9, dot_col);
                            x += s;
                        }
                        y += s;
                    }
                }
            }

            // pan with middle/right drag
            if !canvas_locked
                && (resp.dragged_by(egui::PointerButton::Middle)
                    || resp.dragged_by(egui::PointerButton::Secondary))
            {
                let d = resp.drag_delta();
                self.world_offset += egui::vec2(d.x / self.scale, -d.y / self.scale);
            }

            // wheel zoom around cursor
            let scroll = if canvas_locked { 0.0 } else { ui.input(|i| i.raw_scroll_delta.y) };
            if scroll != 0.0 {
                if let Some(cursor) = resp.hover_pos() {
                    let before = self.s2w(cursor, rect);
                    let factor = (scroll * 0.0015).exp();
                    self.scale = (self.scale * factor).clamp(0.01, 5000.0);
                    let after = self.s2w(cursor, rect);
                    let dx = (after.x - before.x) as f32;
                    let dy = (after.y - before.y) as f32;
                    self.world_offset += egui::vec2(dx, dy);
                }
            }

            // ---- ZOOM real-time (press-drag) -------------------------------
            // Primary drag up = zoom in, down = zoom out, about the viewport
            // center. The click/grip handlers below are gated with `!rt_zoom`
            // so they don't also fire. Enter/Esc exit (command line / Esc).
            if rt_zoom && resp.dragged_by(egui::PointerButton::Primary) {
                let dy = resp.drag_delta().y;
                if dy.abs() > 0.0 {
                    let factor = (-dy as f64 * 0.005).exp();
                    self.scale = ((self.scale as f64 * factor) as f32).clamp(0.01, 5000.0);
                }
            }

            // LAYOUT TAB — PAPER-SPACE INPUT LANE. When no modal draw/edit
            // flow is active, paper interactions (select / move / grip-resize /
            // viewport pick + the layout context menu) replace the model
            // canvas's click handling entirely. Modal flows (draw tools, move /
            // copy / … ) keep the model monolith below — their clicks commit
            // through `add_dobject`, which routes into the active layout's
            // paper entities.
            if in_layout && !canvas_locked && self.hatch_pick_point_armed {
                // Hatch pick-point clicks resolve against MODEL boundaries;
                // hatches are a model-space operation (upstream parity).
                self.hatch_pick_point_armed = false;
                self.hatch_pick_point_session = None;
                self.hatch_confirm_open = false;
                self.hatch_dialog_open = false;
                self.history.push("  hatch: cancelled — hatching works in Model space".into());
            }
            let layout_modal_flow =
                   self.tool                != Tool::None
                || self.move_state          != MoveState::Off
                || self.copy_state          != CopyState::Off
                || self.paste_state         != PasteState::Off
                || self.rotate_state        != RotateState::Off
                || self.scale_state         != ScaleState::Off
                || self.mirror_state        != MirrorState::Off
                || self.align_state         != AlignState::Off
                || self.stretch_state       != StretchState::Off
                || self.break_state         != BreakState::Off
                || self.dist_state          != DistState::Off
                || self.area_state.is_some()
                || self.layer_pick          != LayerPickState::Off
                || self.ptdist_state        != PtDistribState::Off
                || self.insert_state        != InsertState::Off
                || self.cmd_flow.is_some()
                || self.block_def_state     != BlockDefState::Off;
            if in_layout && !canvas_locked && !layout_modal_flow && !rt_zoom {
                self.paper_space_input(ctx, ui, rect, &painter, &resp);
                return;
            }

            // ---- snap candidates + Tab-cycling -----------------------------
            // Collect EVERY viable snap target at the current cursor, sorted
            // by (priority, distance). The first is the default; Tab cycles
            // through the rest. Cursor motion (> 4 px) resets the cycle.
            //
            // Snap fires ONLY for true point-pick phases — phases where the
            // click commits a precise coordinate (draw, move base/dest,
            // mirror axis pts, etc.). Pure hit-test phases (trim cutter
            // pick, trim target click, fillet pick-line, matchprops source,
            // …) suppress snap candidates because the click identifies a
            // dobject, not a point — snap markers are visual noise there.
            //
            // The typed-in-cmd snap override (END / MID / …) ALWAYS wins
            // via find_all_snaps's `forced` parameter and re-enables snap
            // for one click even in hit-test phases. See memo
            // `feedback_rust_cad_inline_snap_override_supersedes`.
            let snap_phase_active =
                self.tool != Tool::None
                || self.snap_override.is_some()
                || self.move_state       != MoveState::Off
                || self.copy_state       != CopyState::Off
                || self.paste_state      != PasteState::Off
                || self.rotate_state     != RotateState::Off
                || self.scale_state      != ScaleState::Off
                || self.mirror_state     != MirrorState::Off
                || self.align_state      != AlignState::Off
                || self.stretch_state    != StretchState::Off
                || self.break_state      != BreakState::Off
                // Point-pick phases that commit a precise coordinate via
                // `click_world` — they need running osnap exactly like the
                // transform ops above. `dist` measures point→point;
                // `insert` places a block at a point; `block` picks the
                // definition's base point. All were silently snap-dead.
                || self.dist_state       != DistState::Off
                // `area` point-polygon clicks commit coordinates too.
                || self.area_state.is_some()
                || self.insert_state     != InsertState::Off
                || self.cmd_flow.is_some()
                // A grip drag commits a precise coordinate too — it needs
                // running osnap so the dragged grip lands ON an END/MID/CEN/…
                || self.grip_drag.is_some()
                || self.block_def_state  != BlockDefState::Off;
            // Phantom dobject for the in-progress polyline so snap kinds
            // (END / MID / CEN / …) work against vertices the user has
            // just clicked but hasn't committed yet. Cheap — only built
            // when the pline tool is mid-flow with at least 2 vertices.
            let pline_phantom: Option<DObject> = self.pline_phantom_dobject();

            // Fillet/Chamfer pick ENTITIES, not points — object snap (the CEN
            // aperture + radius-line to an arc's centre, END/MID markers, …) is
            // pure visual noise there and must be suppressed even if a draw
            // tool was left active when the command started (which otherwise
            // keeps `snap_phase_active` true via `self.tool`).
            let entity_pick_phase = self.fillet_state != FilletState::Off
                || self.chamfer_state != ChamferState::Off
                || self.flow_picks_object();   // circle Ttr tangent-object picks
            let has_sketch_ref =
                self.factory.session.is_some() && !self.factory.sketch_ref.is_empty();
            let snap_candidates: Vec<SnapHit> = if snap_phase_active
                && !entity_pick_phase
                && !self.picking_source && !self.intersect_pending_click
                && (!self.doc.dobjects.is_empty() || pline_phantom.is_some() || has_sketch_ref)
            {
                // PER / TAN need a "from" anchor. During a draw it's the
                // last pending point; during an edit op (move/copy/STRETCH
                // destination, mirror, rotate) it's that op's base/pivot —
                // `card_anchor()` returns exactly that. Previously only
                // `pending.last()` was fed, so PER/TAN were dead during
                // every edit-op point pick (the reported "PER unresponsive
                // during stretch").
                let snap_anchor = self.card_anchor();
                resp.hover_pos().map(|cur| {
                    let world = self.s2w(cur, rect);
                    let world_radius = self.env.SpTGSZ as f64 / self.scale as f64;
                    let grid = if self.index_dirty { None } else { self.index.as_ref() };
                    let mut hits = if self.doc.dobjects.is_empty() {
                        Vec::new()
                    } else {
                        find_all_snaps(
                            world, world_radius,
                            self.snap_enabled, self.snap_override,
                            snap_anchor,
                            &self.doc.dobjects, grid,
                        )
                    };
                    let mut merged_extra = false;
                    if let Some(ref phantom) = pline_phantom {
                        let phantom_slice = std::slice::from_ref(phantom);
                        let phantom_hits = find_all_snaps(
                            world, world_radius,
                            self.snap_enabled, self.snap_override,
                            snap_anchor,
                            phantom_slice, None,
                        );
                        hits.extend(phantom_hits);
                        merged_extra = true;
                    }
                    // Block snap-through: explode every visible block's
                    // contents in-memory and snap against the real geometry,
                    // so END/MID/CEN/… land on lines/arcs INSIDE a block
                    // (the kernel's BlockRef snap only offers the insertion
                    // point — it can't reach the block table).
                    let block_phantoms = self.block_snap_phantoms();
                    if !block_phantoms.is_empty() {
                        let block_hits = find_all_snaps(
                            world, world_radius,
                            self.snap_enabled, self.snap_override,
                            snap_anchor,
                            &block_phantoms, None,
                        );
                        hits.extend(block_hits);
                        merged_extra = true;
                    }
                    // Factory face-sketch REFERENCE — snap to the projected object's outline
                    // (its endpoints / midpoints / edges) so a sketch lands exactly on the
                    // 3D object. The reference lives in the sketch's (u,v) = the canvas's own
                    // coordinates, so its lines feed find_all_snaps unchanged.
                    if self.factory.session.is_some() && !self.factory.sketch_ref.is_empty() {
                        let ref_phantoms: Vec<DObject> = self
                            .factory
                            .sketch_ref
                            .iter()
                            .map(|[a, b]| {
                                DObject::new(cad_kernel::Geom::Line(cad_kernel::Line {
                                    a: Vec2::new(a.x as f64, a.y as f64),
                                    b: Vec2::new(b.x as f64, b.y as f64),
                                }))
                            })
                            .collect();
                        let ref_hits = find_all_snaps(
                            world, world_radius,
                            self.snap_enabled, self.snap_override,
                            snap_anchor,
                            &ref_phantoms, None,
                        );
                        hits.extend(ref_hits);
                        merged_extra = true;
                    }
                    if merged_extra {
                        // Re-sort merged list by (priority, distance) so the
                        // closest snap across all sources wins.
                        hits.sort_by(|a, b| {
                            a.kind.priority().cmp(&b.kind.priority())
                                .then(a.point.dist(world).partial_cmp(&b.point.dist(world))
                                    .unwrap_or(std::cmp::Ordering::Equal))
                        });
                    }
                    hits
                }).unwrap_or_default()
            } else {
                Vec::new()
            };

            // Reset cycle when cursor moves to a meaningfully different spot.
            if let Some(cur) = resp.hover_pos() {
                let moved_far = self.snap_cycle_anchor
                    .map_or(true, |anc| (cur - anc).length() > 4.0);
                if moved_far {
                    self.snap_cycle_index = 0;
                    self.snap_cycle_anchor = Some(cur);
                }
            }

            // Tab → cycle to next candidate. Consume the key so egui doesn't
            // shuffle widget focus (the cmd line will reclaim it anyway).
            let tab_pressed = ctx.input_mut(|i|
                i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
            );
            if tab_pressed && !snap_candidates.is_empty() {
                self.snap_cycle_index = (self.snap_cycle_index + 1) % snap_candidates.len();
            }
            // Clamp in case the candidate count shrank since last frame.
            if !snap_candidates.is_empty() && self.snap_cycle_index >= snap_candidates.len() {
                self.snap_cycle_index = 0;
            }
            let snap_hit: Option<SnapHit> = snap_candidates
                .get(self.snap_cycle_index).copied();
            // Pointer-mode Tab (no snap phase): cycle the SELECTION through
            // stacked dobjects under the cursor. Same consumed key — the two
            // never compete because a snap phase is only active mid-draw /
            // mid-point-pick (see `snap_phase_active`).
            if tab_pressed && snap_candidates.is_empty()
                && !snap_phase_active && self.tool == Tool::None
                && self.select_mode == SelectMode::Off
                && self.doc.active_layout.is_none()
            {
                if let Some(cur) = resp.hover_pos() {
                    let w = self.s2w(cur, rect);
                    let tol = 10.0 / (self.scale as f64).max(1e-6);
                    self.cycle_pick_candidate(w, (cur.x as i32, cur.y as i32), tol);
                }
            }

            // Left-click handling:
            //   - if SELECT MODE is active: toggle dobject under cursor, or
            //     start / close a window-selection rectangle.
            //   - if ∩ click is armed: compute intersections in 50px around it.
            //   - if PICK MODE (array source): hit-test dobjects.
            //   - else if a tool is active: register a draw point.
            // Diagnostic for "click on screen didn't start drawing" reports.
            // Logs which click handler branch a left-click ended up in. The
            // Trim Debug Log window already exists; we piggyback on it.
            // resp.clicked() fires only when egui classifies the press+release
            // as a click (small motion); drag_stopped() fires when a release
            // ends a drag — including a "click" that egui saw as a tiny drag.
            let click_now    = resp.clicked() && !canvas_locked && !rt_zoom && !light_grab && !place_grab;
            let drag_stopped = resp.drag_stopped() && !canvas_locked && !rt_zoom && !light_grab && !place_grab;
            // SESSION RECORDER — every mouse press/release with the
            // FULL gesture context. Press position is stashed on self
            // and re-read at release time to derive the real drag
            // distance + emit a GestureClassification event with both
            // egui's classification AND the app's outcome.
            // Skipped entirely while the Block Editor owns input.
            if !canvas_locked {
                let primary_press = ctx.input(|i|
                    i.pointer.button_pressed(egui::PointerButton::Primary));
                let secondary_press = ctx.input(|i|
                    i.pointer.button_pressed(egui::PointerButton::Secondary));
                let primary_release = ctx.input(|i|
                    i.pointer.button_released(egui::PointerButton::Primary));
                let secondary_release = ctx.input(|i|
                    i.pointer.button_released(egui::PointerButton::Secondary));
                if primary_press || secondary_press {
                    if let Some(pos) = resp.hover_pos() {
                        if rect.contains(pos) {
                            let world = self.s2w(pos, rect);
                            let tol = 10.0 / self.scale as f64;
                            let hit = self.nearest_entity_under(world, tol);
                            self.dbg_press_pos = Some((pos.x, pos.y, world));
                            self.dbg_press_hit = hit;
                            self.dbg_press_sel = self.selection.clone();
                            let button = if primary_press { "Primary" } else { "Secondary" };
                            crate::dbg_event!(self,
                                crate::dbg_recorder::DbgEvent::CanvasPress {
                                    screen: (pos.x, pos.y),
                                    world,
                                    button: button.to_string(),
                                });
                        }
                    }
                }
                if primary_release || secondary_release {
                    if let Some(pos) = resp.hover_pos() {
                        if rect.contains(pos) {
                            let world_r = self.s2w(pos, rect);
                            let button = if primary_release { "Primary" } else { "Secondary" };
                            // Pull from self — the press handler above
                            // already stashed it.
                            let (press_screen, press_world) = self.dbg_press_pos
                                .map(|(sx, sy, w)| ((sx, sy), w))
                                .unwrap_or(((pos.x, pos.y), world_r));
                            let dx = pos.x - press_screen.0;
                            let dy = pos.y - press_screen.1;
                            let dist = (dx*dx + dy*dy).sqrt();
                            crate::dbg_event!(self,
                                crate::dbg_recorder::DbgEvent::CanvasRelease {
                                    screen:  (pos.x, pos.y),
                                    world:   world_r,
                                    button:  button.to_string(),
                                    drag_px: dist,
                                });
                            // Cache the values we need for the after-
                            // app-handler GestureClassification event.
                            // We can't emit it here because the click
                            // handlers below haven't run yet — the
                            // outcome (click_select / window_first /
                            // add_window_selection / NOOP) is unknown.
                            // Emit at the end of the canvas update block.
                            self.dbg_pending_gesture = Some(PendingGesture {
                                press_screen,
                                release_screen: (pos.x, pos.y),
                                press_world,
                                release_world:  world_r,
                                motion_px:      dist,
                                hit_at_press:   self.dbg_press_hit,
                                selection_before: std::mem::take(&mut self.dbg_press_sel),
                            });
                            self.dbg_press_pos = None;
                        }
                    }
                }
            }
            // Capture every drag release with full direction info.
            // The drag classifier below uses the same points; we just
            // dump them to the recorder too so the timeline shows
            // "drag (L→R), 142 px, primary button".
            if drag_stopped {
                if let (Some(p), Some(r)) = (
                    ctx.input(|i| i.pointer.press_origin()),
                    resp.interact_pointer_pos(),
                ) {
                    let p_w = self.s2w(p, rect);
                    let r_w = self.s2w(r, rect);
                    let dx = r.x - p.x;
                    let dy = r.y - p.y;
                    let dist = (dx*dx + dy*dy).sqrt();
                    let dir_h = if dx > 0.5 { "L→R" }
                              else if dx < -0.5 { "R→L" }
                              else { "vertical" };
                    let dir_v = if dy > 0.5 { "↓" }
                              else if dy < -0.5 { "↑" }
                              else { "horizontal" };
                    let primary = ctx.input(|i|
                        i.pointer.button_down(egui::PointerButton::Primary)
                        || i.pointer.button_clicked(egui::PointerButton::Primary));
                    let button = if primary { "Primary" } else { "Secondary" };
                    crate::dbg_event!(self,
                        crate::dbg_recorder::DbgEvent::CanvasDrag {
                            from_screen: (p.x, p.y),
                            to_screen:   (r.x, r.y),
                            from_world:  p_w,
                            to_world:    r_w,
                            button:      format!("{} ({} {} {:.1}px)",
                                button, dir_h, dir_v, dist),
                        });
                }
            }
            // Unified click/drag classifier:
            //   1. select mode is active → DRAG is the rubber-band window
            //   2. pointer-mode-idle → DRAG is the rubber-band window
            //   3. Shift held during press → DRAG is an ad-hoc window
            //   4. anything else → press-release is ALWAYS a click
            // The 5-px motion heuristic is gone — egui's idea of "this was
            // a tiny drag" doesn't get a vote.
            //
            // Read press position from self.press_pos (stashed at press
            // time), NOT from egui's press_origin() — egui clears that
            // by the time drag_stopped() fires on the release frame, so
            // press_release_dist would always read 0 and the classifier
            // would silently demote every drag to a click.
            let press_release_dist = match (
                self.press_pos,
                resp.interact_pointer_pos(),
            ) {
                (Some((p, _)), Some(r)) => (r - p).length(),
                _ => 0.0,
            };
            let shift_held = ctx.input(|i| i.modifiers.shift);
            let in_select  = self.select_mode != SelectMode::Off;
            // Track press time so the classifier below can enforce a
            // hold-threshold before treating a drag as a window. See
            // feedback_rust_cad_universal_selection_model.
            //
            // Order is critical: capture the elapsed-hold reading from
            // the OLD press_time BEFORE the release event clears it.
            // Previous version cleared on release first, so on the
            // same frame as `drag_stopped()` fires, press_held_secs
            // read zero and the gate failed — entire drag-window
            // gestures were silently discarded as fast clicks. Now we
            // compute the gate, classify, THEN update press_time.
            let now = ctx.input(|i| i.time);
            let hold_thresh_secs = (self.env.SelDmTm as f64) / 1000.0;
            let press_held_secs = self.press_time.map(|t0| now - t0).unwrap_or(0.0);
            let hold_threshold_passed = press_held_secs >= hold_thresh_secs;
            // Now update press_time + press_pos for next frame. press_pos
            // is our own copy of the press position, kept because
            // egui::Pointer::press_origin() is cleared by drag_stopped()
            // and the classifier above needs it on the release frame.
            //
            // Order matters: capture the snapshot BEFORE the release
            // handler clears self.press_pos, so the window-drag handler
            // and the rubber-band preview further down can still read it.
            let press_pos_this_frame = self.press_pos;
            if ctx.input(|i| i.pointer.primary_pressed()) && resp.contains_pointer() {
                self.press_time = Some(now);
                if let Some(pos) = resp.hover_pos() {
                    let world = self.s2w(pos, rect);
                    self.press_pos = Some((pos, world));
                }
            }
            if ctx.input(|i| i.pointer.primary_released()) {
                self.press_time = None;
                self.press_pos  = None;
            }
            let in_click_only_phase =
                self.tool != Tool::None
                || matches!(self.trim_state,
                    TrimState::PickingTargets(_) | TrimState::PickingTargetsAll)
                || matches!(self.extend_state,
                    ExtendState::PickingTargets(_) | ExtendState::PickingTargetsAll)
                || self.move_state       != MoveState::Off
                || self.copy_state       != CopyState::Off
                || self.paste_state      != PasteState::Off
                || self.rotate_state     != RotateState::Off
                || self.scale_state      != ScaleState::Off
                || self.mirror_state     != MirrorState::Off
                || self.align_state      != AlignState::Off
                || self.break_state      != BreakState::Off
                || self.lengthen_state   != LengthenState::Off
                || self.offset_state     != OffsetState::Off
                || self.stretch_state    != StretchState::Off
                || self.matchprops_state != MatchPropsState::Off
                || self.fillet_state     != FilletState::Off
                || self.chamfer_state    != ChamferState::Off
                || self.dist_state       != DistState::Off
                || self.area_state.is_some()
                || self.layer_pick       != LayerPickState::Off
                || self.ptdist_state     != PtDistribState::Off
                // Block/insert POINT-PICK phases: a single click captures a
                // coordinate (block base point, insert insertion point), so
                // they must be click-only — otherwise grips on the still-
                // selected source dobjects steal the press (`grip_drag`) and
                // the point is never captured.
                || self.block_dialog_pick_base
                || self.insert_dialog_pick
                || self.insert_param_pick.is_some()
                || self.insert_live.is_some()
                || self.block_def_state != BlockDefState::Off
                || self.insert_state    != InsertState::Off
                || self.cmd_flow.is_some()
                || self.blockdiff_pick  != BlockDiffPick::Off
                || self.picking_source
                // ZOOM point steps (center / window corners) capture a single
                // click each, so they're click-only like a drafting pick.
                // (ObjectSel and RealTime are excluded — they want the normal
                // selection handler / a drag, respectively.)
                || self.zoom_state.wants_point()
                || self.intersect_pending_click;
            // Pointer mode (Tool::None, no edit phase, no active select
            // session) is the always-on selection tool per
            // feedback_rust_cad_pointer_is_selector — a drag on the bare
            // canvas should rubber-band the same as if a SelectMode were
            // active. Without this, naked-canvas drags get demoted to
            // clicks and the user's window-selection intent is lost.
            // This var also gets reused by the grip-drag handler below.
            let pointer_mode_idle = !in_click_only_phase && !in_select;
            // Drag is the rubber-band window when (a) we're in select
            // mode, (b) we're in pointer-mode-idle (the always-on
            // selector), or (c) the user held Shift to request a
            // window drag. Edit phases (trim/draw/move/…) keep the
            // "always click" semantic.
            //
            // Time-gated activation: the press must have been held
            // longer than env.SelDmTm (default 250 ms) before a drag
            // counts as a window. A fast accidental drag during a
            // click = still a click. The rubber-band preview honors
            // the same gate. Reference:
            // feedback_rust_cad_universal_selection_model.
            //
            // Shift-drag is exempt from the time gate — when the user
            // is explicitly holding Shift to force a window-drag,
            // they don't need to also hold the button to "prove" it.
            let drag_intent_is_window =
                ((in_select && hold_threshold_passed)
                 || (pointer_mode_idle && hold_threshold_passed)
                 || (shift_held && !in_click_only_phase))
                && press_release_dist > 1.0;     // any real motion at all
            let drag_was_a_click = drag_stopped && !drag_intent_is_window;

            // ---- Drafting-mode PRESS-fires-click override ---------------
            //
            // Where the gesture has no drag semantic (every drawing tool
            // + every point-pick edit phase), register the click at PRESS
            // time instead of release. Same affordance as the AutoCAD
            // pickbox: pressing AT a point captures THAT point, even if
            // the cursor drifts a few pixels between press and release.
            // Drag-meaningful gestures (select-mode rubber-band, Shift-
            // drag window, grip drag) stay on release — they need both
            // endpoints. Visual cue: the square+cross drafting cursor
            // is drawn iff in_click_only_phase.
            let press_fires_click = in_click_only_phase;
            let press_now = press_fires_click
                && !rt_zoom
                && !light_grab
                && !place_grab
                && ctx.input(|i| i.pointer.primary_pressed())
                && resp.contains_pointer();
            let click_now = if press_fires_click { press_now } else { click_now };
            // In drafting mode the press fired the click; suppress the
            // release-time drag-promoted click so we don't double-fire
            // when egui later reports a tiny accidental drag_stopped.
            let drag_was_a_click = drag_was_a_click && !press_fires_click;
            // ---- Grip drag handling (v2: per-grip role semantics) -----------
            // Grips are DRAG-ONLY: press on a grip → drag → release commits.
            // (Click-to-grab/click-to-place was removed: a plain click near a
            // grip silently armed a stretch and a later stray click anywhere
            // warped the dobject. A drag is an explicit, unambiguous gesture
            // that a stray click can never trigger.) The kernel's
            // Geom::with_grip_moved() decides what changes (e.g. circle
            // quadrant → radius; line midpoint → translate whole line).
            let mut grip_drag_consumed_click = false;
            if pointer_mode_idle && self.env.GrpEnb && !rt_zoom && !light_grab && !place_grab {
                let drag_started = resp.drag_started_by(egui::PointerButton::Primary)
                    && !canvas_locked;
                // Drag-grab ONLY: a primary-button drag that begins near a grip.
                // There is no click-grab (a plain click must never arm a grip —
                // that was the warp bug). `pending_release_swallow` still guards
                // the release tail of a press-fires-click gesture (a point-pick
                // phase captures on PRESS; without this the release frame could
                // spuriously start a grip).
                let try_grab = !self.pending_release_swallow && drag_started;
                if try_grab && self.grip_drag.is_none() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let cur_world = self.s2w(pos, rect);
                        // Match the visual hover-highlight radius so any
                        // grip that looks lit-up actually grabs on click.
                        // GrpHvR is in screen pixels; convert to world.
                        let tol = self.env.GrpHvR as f64 / self.scale as f64;
                        let targets = self.editable_grip_targets();
                        'outer: for &idx in &targets {
                            let Some(d) = self.doc.dobjects.get(idx) else { continue; };
                            for (gp, role) in d.geom.grip_points() {
                                if cur_world.dist(gp) < tol {
                                    self.grip_drag = Some(GripDrag {
                                        dobject_idx: idx,
                                        role,
                                        grip_origin: gp,
                                    });
                                    // Phase-10: a grip drag on a hatch's aux
                                    // boundary reshapes that FILL — say so, or
                                    // the log shows an edit to an invisible
                                    // polyline with no stated connection.
                                    if self.doc.dobjects.get(idx)
                                        .map(|d| d.style.hatch_aux).unwrap_or(false)
                                    {
                                        let h = self.doc.dobjects[idx].handle;
                                        let owners: Vec<usize> = self.doc.dobjects.iter()
                                            .enumerate()
                                            .filter(|(_, d)| matches!(&d.geom,
                                                Geom::Hatch(hh) if hh.boundary_handles.contains(&h)))
                                            .map(|(i, _)| i).collect();
                                        self.hatch_dbg(format!(
                                            "--- [10] grip drag on hatch boundary #{} ({:?}) at \
                                             ({:.3},{:.3}) — reshapes hatch {:?}",
                                            idx, role, gp.x, gp.y, owners));
                                    }
                                    // Every OTHER selected dobject with a grip
                                    // at this SAME point joins the drag, so
                                    // shapes sharing a corner stay joined and a
                                    // hatch stays with its boundary (AutoCAD
                                    // moves all coincident grips of the
                                    // selection together). Previously the first
                                    // match won and the rest silently stayed put.
                                    let peers: Vec<(usize, cad_kernel::GripRole)> = targets
                                        .iter()
                                        .filter(|&&o| o != idx)
                                        .filter_map(|&o| {
                                            let od = self.doc.dobjects.get(o)?;
                                            let (_, orole) = od.geom.grip_points()
                                                .into_iter()
                                                .find(|(ogp, _)| ogp.dist(gp) < tol)?;
                                            Some((o, orole))
                                        })
                                        .collect();
                                    if !peers.is_empty() {
                                        self.hatch_dbg(format!(
                                            "--- [10] grip drag: {} coincident grip(s) move together \
                                             — primary #{}, peers {:?}",
                                            peers.len() + 1, idx,
                                            peers.iter().map(|(i, _)| *i).collect::<Vec<_>>()));
                                    }
                                    self.grip_drag_peers = peers;
                                    // Don't treat this click as a "select-
                                    // toggle click" — it's a grab.
                                    grip_drag_consumed_click = true;
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
                // Commit on drag_stopped — releasing the drag ends the grip move.
                if let Some(gd) = self.grip_drag {
                    let drag_release = resp.drag_stopped_by(egui::PointerButton::Primary);
                    if drag_release {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            // Honor running osnap (and CARD/grid) so the grip
                            // lands ON the highlighted END/MID/CEN/… instead of
                            // the raw cursor. Matches the live preview below.
                            let drop_world = self
                                .cursor_world_constrained(Some(pos), rect, snap_hit.map(|h| h.point))
                                .unwrap_or_else(|| self.s2w(pos, rect));
                            let delta = drop_world - gd.grip_origin;
                            // Suppress accidental no-op drags (tiny mouse
                            // jitter) — zero motion just clears the grip state.
                            if delta.len() > 1e-9 {
                                self.snapshot_doc();
                                let uniform = ctx.input(|inp| inp.modifiers.shift);
                                // The dragged grip PLUS every coincident grip
                                // on the other selected dobjects — all land on
                                // the same drop point, so shapes that shared a
                                // corner still share it afterwards.
                                let mut moves: Vec<(usize, cad_kernel::GripRole)> =
                                    vec![(gd.dobject_idx, gd.role)];
                                moves.extend(self.grip_drag_peers.iter().copied());
                                for (idx, role) in moves {
                                    let Some(d) = self.doc.dobjects.get_mut(idx) else { continue };
                                    // Issue #39 — corner grips on a closed
                                    // 4-vertex polyline SCALE from the
                                    // opposite corner (Shift = uniform);
                                    // everything else keeps its vertex move.
                                    if let cad_kernel::GripRole::PolyVertex(i) = role {
                                        if let Some(scaled) =
                                            d.geom.with_corner_scale(i, drop_world, uniform)
                                        {
                                            d.geom = scaled;
                                        } else {
                                            d.geom = d.geom.with_grip_moved(role, drop_world);
                                        }
                                    } else {
                                        d.geom = d.geom.with_grip_moved(role, drop_world);
                                    }
                                }
                                self.intersections.clear();
                                self.index_dirty = true;
                                self.touch_view();
                                self.history.push(format!(
                                    "  ⊕ grip: #{} {:?} → ({:.3}, {:.3})",
                                    gd.dobject_idx, gd.role,
                                    drop_world.x, drop_world.y));
                            }
                        }
                        self.grip_drag = None;
                        self.grip_drag_peers.clear();
                        grip_drag_consumed_click = true;
                    }
                }
            }
            // Drag-window handler: when drag_intent_is_window fires on
            // release, capture (press, release) as the two opposite
            // corners and apply a selection window. L→R = window (only
            // fully-inside dobjects); R→L = crossing (anything touching).
            // For ad-hoc Shift+drag (no select_mode), open a transient
            // ForSelect session, apply the window, then finalise
            // immediately so the basket persists.
            let mut window_drag_consumed_click = false;
            if drag_stopped && drag_intent_is_window && !grip_drag_consumed_click {
                // Read press from our stashed snapshot — egui's
                // press_origin() is cleared by the time drag_stopped()
                // fires, which is exactly now.
                if let (Some((_p_screen, press_world_stashed)), Some(r)) = (
                    press_pos_this_frame,
                    resp.interact_pointer_pos(),
                ) {
                    let press_world   = press_world_stashed;
                    let release_world = self.s2w(r, rect);
                    let shift = ctx.input(|i| i.modifiers.shift);
                    let alt   = ctx.input(|i| i.modifiers.alt);
                    let was_off = self.select_mode == SelectMode::Off;
                    // Pointer-mode drag (was_off) applies DIRECTLY to the live
                    // selection — do NOT begin_selection(), which clears it and
                    // would turn Shift+drag (add) / Alt+drag (remove) into a
                    // replace. `fresh = was_off`: a plain idle drag replaces;
                    // Shift adds, Alt removes; inside a session it accumulates.
                    self.add_window_selection(
                        press_world, release_world, shift, alt, was_off);
                    // Window-selecting any group member selects the whole group.
                    if was_off && !alt { self.expand_selection_to_groups(); }
                    // STRETCH: the crossing window is also the per-vertex
                    // test region — remember it (last window wins).
                    if self.queued_op == QueuedOp::Stretch {
                        let wmin = Vec2::new(press_world.x.min(release_world.x),
                                             press_world.y.min(release_world.y));
                        let wmax = Vec2::new(press_world.x.max(release_world.x),
                                             press_world.y.max(release_world.y));
                        self.stretch_window_box = Some((wmin, wmax));
                    }
                    if !was_off {
                        // Inside select_mode: stay in the session so the
                        // user can add more windows / clicks. window_first
                        // would re-arm naturally on next click.
                        self.window_first = None;
                    }
                    // Reclaim the command line so the NEXT typed command (e.g.
                    // `e` to erase the just-selected items) lands — the click-
                    // select paths all do this; the drag path used to omit it,
                    // dropping the first keystroke after a window selection.
                    self.refocus_cmd = true;
                    window_drag_consumed_click = true;
                }
            }
            // Swallow the release of a press-fires-click whose op already
            // ended on the press (e.g. stretch/move/copy destination): the
            // op leaves click-only mode, so this release would otherwise be
            // re-read as a pointer-mode click and spuriously select.
            let primary_released_now = ctx.input(|i| i.pointer.primary_released());
            let release_swallow = self.pending_release_swallow && primary_released_now;
            if primary_released_now { self.pending_release_swallow = false; }
            if press_now && !primary_released_now { self.pending_release_swallow = true; }
            let click_fired = (click_now || drag_was_a_click)
                && !grip_drag_consumed_click
                && !window_drag_consumed_click
                && !release_swallow;
            if (click_now || drag_stopped) && self.trim_debug_open {
                // Only log when the user has the diagnostic window open, to
                // keep the log uncluttered.
                let drag_motion = if drag_stopped {
                    press_release_dist
                } else { 0.0 };
                let gates = format!(
                    "tool={:?} pending={} move={:?} copy={:?} rotate={:?} \
                     scale={:?} mirror={:?} align={:?} break={:?} lengthen={:?} \
                     offset={:?} stretch={:?} matchprops={:?} trim={} extend={} \
                     select_mode={:?} pick_src={} ∩pend={}",
                    self.tool, self.pending.len(),
                    self.move_state, self.copy_state, self.rotate_state,
                    self.scale_state, self.mirror_state, self.align_state,
                    self.break_state, self.lengthen_state,
                    self.offset_state, self.stretch_state, self.matchprops_state,
                    match self.trim_state {
                        TrimState::Off => "Off",
                        TrimState::SelectingCutters => "SelectingCutters",
                        TrimState::PickingTargets(_) => "PickingTargets(list)",
                        TrimState::PickingTargetsAll => "PickingTargetsAll",
                    },
                    match self.extend_state {
                        ExtendState::Off => "Off",
                        ExtendState::SelectingBoundaries => "SelectingBoundaries",
                        ExtendState::PickingTargets(_) => "PickingTargets(list)",
                        ExtendState::PickingTargetsAll => "PickingTargetsAll",
                    },
                    self.select_mode, self.picking_source, self.intersect_pending_click,
                );
                self.trim_dbg(format!(
                    "CLICK {} (press→release={:.1}px{}) | {}",
                    if click_now { "clicked()" } else { "drag_stopped()" },
                    drag_motion,
                    if drag_was_a_click { ", promoted to click" } else { "" },
                    gates,
                ));
            }
            if click_fired {
                if let Some(pos) = resp.interact_pointer_pos() {
                    // `world` keeps its old meaning (raw cursor world pos)
                    // for downstream branches that need it unconstrained
                    // (selection hit-test, intersect-click, etc.).
                    let world = self.s2w(pos, rect);
                    // Priority for the captured POINT: osnap > CARD
                    // > grid-snap > raw. Used wherever a click commits a
                    // point (line endpoint, move base/dest, …).
                    let click_world = snap_hit.map(|h| h.point)
                        .unwrap_or_else(|| self.apply_constraints(world));
                    // Remember the last picked point during a draw tool/flow, so
                    // Enter/Space at the next command's first-point prompt
                    // continues from here (AutoCAD "last point").
                    if self.tool != Tool::None || self.cmd_flow.is_some() {
                        self.last_point = Some(click_world);
                    }
                    // SESSION RECORDER — every canvas click is captured
                    // with screen + world coords, hit-test result, tool,
                    // and a one-line state summary so the reader can
                    // see WHY the click triggered what it did.
                    {
                        let modifiers = ctx.input(|i| i.modifiers);
                        let mods = crate::dbg_recorder::KeyModifiers {
                            shift: modifiers.shift,
                            ctrl:  modifiers.ctrl,
                            alt:   modifiers.alt,
                        };
                        let hit = self.nearest_entity_under(
                            click_world, 10.0 / self.scale as f64);
                        let active_state = format!(
                            "tool={:?} sel_mode={:?} trim={:?} ext={:?} fillet={:?} chamfer={:?} offset={:?} dist={:?} text_draft={:?} grip={}",
                            self.tool, self.select_mode,
                            self.trim_state, self.extend_state,
                            self.fillet_state, self.chamfer_state,
                            self.offset_state, self.dist_state,
                            self.text_draft,
                            self.grip_drag.is_some());
                        crate::dbg_event!(self,
                            crate::dbg_recorder::DbgEvent::CanvasClick {
                                screen:       (pos.x, pos.y),
                                world:        click_world,
                                modifiers:    mods,
                                hit_dobject:  hit,
                                active_tool:  format!("{:?}", self.tool),
                                active_state,
                            });
                    }

                    // WP-SCRIPT slice 5: an armed script-parameter pick
                    // consumes the click (highest priority: it must beat
                    // every command/tool click handler). `point` fills the
                    // world coordinate; `entity` hit-tests the nearest
                    // shape and fills its index.
                    if let Some((pname, kind)) = self.script_param_pick.clone() {
                        let value = match kind {
                            ScriptPickKind::Point => Some(format!(
                                "{:.4},{:.4}",
                                click_world.x, click_world.y
                            )),
                            ScriptPickKind::Entity => {
                                let tol = 10.0 / self.scale as f64;
                                self.nearest_entity_under(click_world, tol)
                                    .map(|i| i.to_string())
                            }
                        };
                        match value {
                            Some(v) => {
                                self.script_param_pick = None;
                                if let Some(dlg) = self.script_param_dialog.as_mut() {
                                    if let Some(ix) = dlg
                                        .params
                                        .iter()
                                        .position(|p| p.name == pname)
                                    {
                                        dlg.values[ix] = v.clone();
                                    }
                                }
                                self.script_preview_dirty();
                                self.history.push(format!(
                                    "  python: {} = {}",
                                    pname, v
                                ));
                                self.clear_prompt();
                                self.refocus_cmd = true;
                                return;
                            }
                            None => {
                                // Missed — stay armed, tell the user (rule 10).
                                self.fail_op(format!(
                                    "script: no shape at that point — click a dobject for '{}' (Esc cancels)",
                                    pname
                                ));
                                return;
                            }
                        }
                    }


                    // ZOOM point pick (center / window corners). Consumes the
                    // click before any selection/tool handler. ObjectSel is NOT
                    // a point step — it falls through to normal selection.
                    if self.zoom_state.wants_point() {
                        self.zoom_input_point(click_world);
                        self.refocus_cmd = true;
                        return;
                    }

                    // Prompt-driven flow (CIRCLE) — a point answer. Consumes
                    // the click before any other handler. The clicked world
                    // point is already snap-applied (`click_world`).
                    if self.cmd_flow.is_some() && self.flow_wants_point() {
                        self.flow_input_point(click_world);
                        self.refocus_cmd = true;
                        return;
                    }

                    // Hatch pick-point — consumes the click before any
                    // other handler. BPOLY-style pipeline: tries a
                    // self-closed containing dobject first; falls
                    // through to ray-cast + boundary trace + island
                    // detect + materialise if needed.
                    //
                    // Session stays ARMED across clicks: each pick
                    // creates one hatch, then we re-fill
                    // pending_hatch_pattern from the session snapshot
                    // and update the prompt for the next pick.
                    // Enter / Esc ends the session.
                    if self.hatch_pick_point_armed {
                        self.hatch_dbg(format!(
                            "pick-point click at world ({:.3}, {:.3})",
                            world.x, world.y));
                        self.apply_pick_point_hatch(world);
                        // Restore the pattern for the next click in
                        // this session; refresh the prompt so the user
                        // knows we're still waiting for picks.
                        if let Some(pat) = self.hatch_pick_point_session.clone() {
                            self.pending_hatch_pattern = pat;
                            let style = self.hatch_pick_point_session.as_ref()
                                .and_then(|(n, _, _)| n.clone())
                                .unwrap_or_else(|| "SOLID".to_string());
                            self.set_prompt(format!(
                                "hatch ({}): click another region OR Enter to finish  [Esc=cancel]",
                                style));
                        }
                        return;
                    }

                    // Compare: picking the two blocks on screen.
                    if self.block_diff_pick_click(world) {
                        self.refocus_cmd = true;
                        return;
                    }

                    if self.move_state != MoveState::Off {
                        match self.move_state {
                            MoveState::WaitingForBase => {
                                self.move_state = MoveState::WaitingForDest(click_world);
                                self.history.push(format!(
                                    "    move: BASE = ({:.3}, {:.3}) — click DESTINATION",
                                    click_world.x, click_world.y));
                            }
                            MoveState::WaitingForDest(base) => {
                                let v = click_world - base;
                                self.apply_move(v);
                                self.move_state = MoveState::Off;
                                self.history.push(format!(
                                    "  move ✓ vector ({:.3}, {:.3}) applied to {} dobject(s)",
                                    v.x, v.y, self.selection.len()));
                            }
                            MoveState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.copy_state != CopyState::Off {
                        match self.copy_state {
                            CopyState::WaitingForBase => {
                                self.copy_state = CopyState::WaitingForDest(click_world);
                                self.history.push(format!(
                                    "    copy: BASE = ({:.3}, {:.3}) — click DESTINATION",
                                    click_world.x, click_world.y));
                            }
                            CopyState::WaitingForDest(base) => {
                                let v = click_world - base;
                                self.apply_copy(v);
                                self.copy_state = CopyState::Off;
                            }
                            CopyState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.paste_state != PasteState::Off {
                        match self.paste_state {
                            PasteState::WaitingForBase => {
                                self.paste_state = PasteState::WaitingForDest(click_world);
                                self.set_prompt(
                                    "paste: click DESTINATION   [Esc=cancel]");
                            }
                            PasteState::WaitingForDest(base) => {
                                self.commit_paste(base, click_world);
                            }
                            PasteState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.rotate_state != RotateState::Off {
                        match self.rotate_state {
                            RotateState::WaitingForPivot => {
                                self.rotate_state = RotateState::WaitingForAngle(click_world);
                                self.set_prompt(format!(
                                    "rotate (pivot=({:.2},{:.2})): click to pick angle, or type number (CCW=+), R=reference, C={}",
                                    click_world.x, click_world.y,
                                    if self.rotate_copy { "copy ON" } else { "copy off" }));
                            }
                            RotateState::WaitingForAngle(pivot) => {
                                // Default: angle from pivot to click point.
                                // Zero baseline = +X axis (atan2 of vector
                                // from pivot to cursor). Positive = CCW.
                                let signed = (click_world - pivot).angle();
                                self.apply_rotate_or_copy(pivot, signed);
                                self.rotate_state = RotateState::Off;
                                self.rotate_copy = false;
                                self.clear_prompt();
                            }
                            // ---- Reference sub-command (3 picks) -------
                            // Mirrors scale-R: 2 picks define the source
                            // direction (anywhere), then ONE click anchored
                            // at the pivot defines the new direction.
                            RotateState::WaitingForRefSrc1(pivot) => {
                                self.rotate_state = RotateState::WaitingForRefSrc2(pivot, click_world);
                                self.set_prompt("rotate-R: click SOURCE point 2 (defines current direction)");
                            }
                            RotateState::WaitingForRefSrc2(pivot, s1) => {
                                let src_angle = (click_world - s1).angle();
                                self.rotate_state = RotateState::WaitingForRefTgt(pivot, src_angle);
                                self.set_prompt(format!(
                                    "rotate-R: click NEW direction (anchored at pivot) OR type angle [src={:.2}°]",
                                    src_angle.to_degrees()));
                            }
                            RotateState::WaitingForRefTgt(pivot, src_angle) => {
                                let tgt = (click_world - pivot).angle();
                                let mut dtheta = (tgt - src_angle).rem_euclid(std::f64::consts::TAU);
                                if dtheta > std::f64::consts::PI {
                                    dtheta -= std::f64::consts::TAU;
                                }
                                self.apply_rotate_or_copy(pivot, dtheta);
                                self.rotate_state = RotateState::Off;
                                self.rotate_copy = false;
                                self.clear_prompt();
                            }
                            RotateState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.scale_state != ScaleState::Off {
                        match self.scale_state {
                            ScaleState::WaitingForPivot => {
                                self.scale_state = ScaleState::WaitingForFactor(click_world);
                                self.set_prompt(format!(
                                    "scale (pivot=({:.2},{:.2})): click for factor (= distance from pivot), type number, R=reference, C={}",
                                    click_world.x, click_world.y,
                                    if self.scale_copy { "copy ON" } else { "copy off" }));
                            }
                            ScaleState::WaitingForFactor(pivot) => {
                                // Default: click distance from pivot = scale factor.
                                let factor = pivot.dist(click_world);
                                if factor < EPS {
                                    self.history.push("  ! click too close to pivot — factor would be 0".into());
                                } else {
                                    self.apply_scale_or_copy(pivot, factor);
                                }
                                self.scale_state = ScaleState::Off;
                                self.scale_copy  = false;
                                self.clear_prompt();
                            }
                            // ---- Reference sub-command (R) -------------
                            ScaleState::WaitingForRefStart(pivot) => {
                                self.scale_state = ScaleState::WaitingForRefEnd(pivot, click_world);
                                self.set_prompt("scale-R: click REFERENCE end (defines old length)");
                            }
                            ScaleState::WaitingForRefEnd(pivot, ref_start) => {
                                let ref_d = ref_start.dist(click_world);
                                if ref_d < EPS {
                                    self.history.push("  ! reference endpoints coincide".into());
                                    self.scale_state = ScaleState::Off;
                                    self.scale_copy  = false;
                                    self.clear_prompt();
                                } else {
                                    self.scale_state = ScaleState::WaitingForNewLength(pivot, ref_d);
                                    self.set_prompt(format!(
                                        "scale-R: click for NEW length (= distance from pivot) OR type number  [ref={:.3}]",
                                        ref_d));
                                }
                            }
                            ScaleState::WaitingForNewLength(pivot, ref_d) => {
                                let new_len = pivot.dist(click_world);
                                if new_len < EPS {
                                    self.history.push("  ! click too close to pivot".into());
                                } else {
                                    self.apply_scale_or_copy(pivot, new_len / ref_d);
                                }
                                self.scale_state = ScaleState::Off;
                                self.scale_copy  = false;
                                self.clear_prompt();
                            }
                            ScaleState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.fence_armed
                        && (matches!(self.trim_state,
                                TrimState::PickingTargets(_) | TrimState::PickingTargetsAll)
                            || matches!(self.extend_state,
                                ExtendState::PickingTargets(_) | ExtendState::PickingTargetsAll))
                    {
                        // FENCE: two clicks define a crossing line. First click
                        // parks in `fence_first`; the second runs it and
                        // disarms. In the TRIM target phase it trims every
                        // crossed dobject at the crossing; EXTEND grows them.
                        let trimming = matches!(self.trim_state,
                            TrimState::PickingTargets(_) | TrimState::PickingTargetsAll);
                        if let Some(first) = self.fence_first.take() {
                            if trimming {
                                self.apply_trim_fence(first, world);
                                self.set_prompt(
                                    "trim: click a target, drag a window, or F = fence  [Enter/Esc ends]");
                            } else {
                                self.apply_extend_fence(first, world);
                                self.set_prompt(
                                    "extend: click a target, drag a window, or F = fence  [Enter/Esc ends]");
                            }
                            self.fence_armed = false;
                        } else {
                            self.fence_first = Some(world);
                            self.history.push(
                                "    fence: click SECOND point (the line's other end)".into());
                            self.set_prompt(if trimming {
                                "trim: fence — click second point  [Esc cancels the fence]"
                            } else {
                                "extend: fence — click second point  [Esc cancels the fence]"
                            });
                        }
                        self.refocus_cmd = true;
                    } else if self.armed_window_inside.is_some()
                        && (matches!(self.trim_state,
                                TrimState::PickingTargets(_) | TrimState::PickingTargetsAll)
                            || matches!(self.extend_state,
                                ExtendState::PickingTargets(_) | ExtendState::PickingTargetsAll))
                    {
                        // Two-click WINDOW / CROSSING box in the target phase
                        // (typed `w` / `c`): the box edges act as a rectangular
                        // fence. Direction doesn't change what is trimmed —
                        // only the rubber-band colour.
                        let trimming = matches!(self.trim_state,
                            TrimState::PickingTargets(_) | TrimState::PickingTargetsAll);
                        if let Some(first) = self.window_first.take() {
                            self.armed_window_inside = None;
                            if trimming {
                                self.apply_trim_window(first, world);
                                self.set_prompt(
                                    "trim: click a target, drag a window, or F = fence  [Enter/Esc ends]");
                            } else {
                                self.apply_extend_window(first, world);
                                self.set_prompt(
                                    "extend: click a target, drag a window, or F = fence  [Enter/Esc ends]");
                            }
                        } else {
                            self.window_first = Some(world);
                            self.history.push(
                                "    window: click OPPOSITE corner".into());
                            self.set_prompt(if trimming {
                                "trim: window — click second corner  [Esc cancels]"
                            } else {
                                "extend: window — click second corner  [Esc cancels]"
                            });
                        }
                        self.refocus_cmd = true;
                    } else if matches!(
                        self.trim_state,
                        TrimState::PickingTargets(_) | TrimState::PickingTargetsAll)
                    {
                        // Resolve the effective cutter list. In "all" mode
                        // it's recomputed from doc.dobjects every click so
                        // pieces created by THIS session's trims keep
                        // acting as cutters.
                        let all_mode = matches!(self.trim_state, TrimState::PickingTargetsAll);
                        let cutters: Vec<usize> = if all_mode {
                            (0..self.doc.dobjects.len()).collect()
                        } else if let TrimState::PickingTargets(c) = &self.trim_state {
                            c.clone()
                        } else { Vec::new() };
                        let tol_world = 10.0 / self.scale as f64;
                        let hit = self.nearest_entity_under(world, tol_world);
                        self.trim_dbg(format!(
                            "TRIM target click  world={}  screen={}  hit={}  cutters={}",
                            Self::fmt_v(click_world),
                            format!("({:.1},{:.1})", pos.x, pos.y),
                            match hit {
                                Some(i) => format!("#{}", i),
                                None    => "VOID (no dobject under cursor)".into(),
                            },
                            if all_mode {
                                format!("ALL(dynamic, n={})", cutters.len())
                            } else {
                                format!("{:?}", cutters)
                            },
                        ));
                        if let Some(tgt) = hit {
                            // FULL TARGET GEOMETRY BEFORE TRIM — bug-reportable.
                            // Tells you exactly what kernel::trim_at was given.
                            self.trim_dbg_dobject(tgt, "TARGET (pre-trim)");
                            let n_before = self.doc.dobjects.len();
                            // Note this BEFORE the trim — we use it to
                            // decide whether the new pieces inherit cutter
                            // status from a cutter parent (see memo
                            // `feedback_rust_cad_trim_pieces_inherit_cutter_status`).
                            let tgt_was_cutter = cutters.contains(&tgt);
                            let did_trim = self.apply_trim_pick(&cutters, tgt, click_world);
                            let n_after = self.doc.dobjects.len();
                            let net = n_after as i64 - n_before as i64;
                            self.trim_dbg(format!(
                                "  → apply_trim_pick success={}  dobjects {}→{}  (net {:+})",
                                did_trim, n_before, n_after, net));
                            // FULL GEOMETRY OF EVERY NEW-BORN PIECE.
                            // apply_trim_pick removes target_idx (indices
                            // above shift down by 1), then appends N new
                            // pieces at the end. New pieces live at indices
                            // [n_after - n_pieces .. n_after) where
                            // n_pieces = n_after + 1 - n_before.
                            if did_trim && n_after + 1 > n_before {
                                let n_pieces = n_after + 1 - n_before;
                                let first_new = n_after - n_pieces;
                                self.trim_dbg(format!(
                                    "  --- {} new piece(s) at indices {}..{} ---",
                                    n_pieces, first_new, n_after));
                                for i in first_new..n_after {
                                    self.trim_dbg_dobject(i, "NEW PIECE");
                                }
                            } else if did_trim {
                                self.trim_dbg(
                                    "  --- 0 new pieces (whole target dropped — degenerate trim) ---"
                                        .to_string());
                            }
                            // Patch the cutter list ONLY in explicit-list
                            // mode and ONLY when the doc actually changed.
                            // In all-mode the next click re-derives cutters
                            // from doc, so no patch is needed.
                            if did_trim && !all_mode {
                                // After remove(tgt) + append N pieces, the
                                // doc has (n_before - 1 + n_pieces) entries.
                                // n_pieces = n_after - (n_before - 1).
                                let n_pieces = n_after + 1 - n_before;
                                let first_new = n_after - n_pieces;
                                let patched: Vec<usize> = if let TrimState::PickingTargets(c) = &mut self.trim_state {
                                    c.retain(|&i| i != tgt);
                                    for c_i in c.iter_mut() {
                                        if *c_i > tgt { *c_i -= 1; }
                                    }
                                    // INHERIT: if the trimmed target was a
                                    // cutter, its new pieces are cutters too.
                                    if tgt_was_cutter && n_pieces > 0 {
                                        c.extend(first_new..n_after);
                                    }
                                    c.clone()
                                } else { Vec::new() };
                                if tgt_was_cutter && n_pieces > 0 {
                                    self.trim_dbg(format!(
                                        "  → cutters patched (parent #{} was a cutter → {} new pieces inherit) = {:?}",
                                        tgt, n_pieces, patched));
                                } else {
                                    self.trim_dbg(format!(
                                        "  → cutters patched = {:?}", patched));
                                }
                            } else if all_mode {
                                self.trim_dbg(
                                    "  → cutters: ALL mode (next click re-derives from doc)".to_string());
                            } else {
                                self.trim_dbg(
                                    "  → cutter list UNCHANGED (trim failed; preserving cutters)".to_string());
                            }
                        } else {
                            // Void click — log + do nothing. Session continues.
                            self.history.push(
                                "  trim — void click (no dobject) — session continues, click another target or press Enter".into());
                        }
                        self.refocus_cmd = true;
                    } else if matches!(
                        self.extend_state,
                        ExtendState::PickingTargets(_) | ExtendState::PickingTargetsAll)
                    {
                        let all_bounds_mode = matches!(self.extend_state, ExtendState::PickingTargetsAll);
                        let bounds: Vec<usize> = if all_bounds_mode {
                            (0..self.doc.dobjects.len()).collect()
                        } else if let ExtendState::PickingTargets(b) = &self.extend_state {
                            b.clone()
                        } else { Vec::new() };
                        let tol_world = 10.0 / self.scale as f64;
                        let hit = self.nearest_entity_under(world, tol_world);
                        self.trim_dbg(format!(
                            "EXTEND target click  world={}  hit={}  bounds={:?}",
                            Self::fmt_v(click_world),
                            match hit {
                                Some(i) => format!("#{}", i),
                                None    => "VOID".into(),
                            },
                            bounds,
                        ));
                        if let Some(tgt) = hit {
                            self.apply_extend_pick(&bounds, tgt, click_world);
                        } else {
                            self.history.push(
                                "  extend — void click — session continues, click another target or press Enter".into());
                        }
                        self.refocus_cmd = true;
                    } else if self.offset_state != OffsetState::Off {
                        let tol_world = 10.0 / self.scale as f64;
                        let hit = self.nearest_entity_under(world, tol_world);
                        match self.offset_state {
                            OffsetState::WaitingForObject(mode) => {
                                if let Some(idx) = hit {
                                    self.offset_state = OffsetState::WaitingForSide(mode, idx);
                                    let msg = match mode {
                                        OffsetMode::Distance(d) => format!(
                                            "  offset: source = #{} — click SIDE to offset toward (d={})", idx, d),
                                        OffsetMode::Through => format!(
                                            "  offset: source = #{} — click THROUGH-point", idx),
                                    };
                                    self.history.push(msg);
                                    self.refresh_offset_prompt();
                                } else {
                                    self.history.push("  offset — click ON a dobject; missed".into());
                                }
                            }
                            OffsetState::WaitingForSide(mode, src_idx) => {
                                self.apply_offset_single(mode, src_idx, click_world);
                                // Loop back to "Select object" — AutoCAD-style.
                                let next_mode = match mode {
                                    OffsetMode::Through => OffsetMode::Through,
                                    OffsetMode::Distance(_) => OffsetMode::Distance(self.env.OfsDis),
                                };
                                self.offset_state = OffsetState::WaitingForObject(next_mode);
                                self.refresh_offset_prompt();
                            }
                            OffsetState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.area_state.is_some() {
                        // AREA — a click on a closed object measures it; an
                        // empty click starts/extends the point polygon.
                        self.area_click(click_world);
                        self.refocus_cmd = true;
                    } else if self.array_pick_center {
                        // ARRAY (polar) — this click is the ring centre.
                        self.array_center = click_world;
                        self.array_pick_center = false;
                        self.clear_prompt();
                        self.history.push(format!(
                            "  array (polar): centre = ({:.3},{:.3})",
                            click_world.x, click_world.y));
                        self.refocus_cmd = true;
                    } else if self.array_pick_path {
                        // ARRAY (path) — this click picks the path curve.
                        let tol = (self.env.PkBxSz.max(8) as f64)
                            / (self.scale as f64).max(1e-6);
                        match self.nearest_entity_under(click_world, tol) {
                            Some(pi) => {
                                self.array_path_idx = Some(pi);
                                self.array_pick_path = false;
                                self.clear_prompt();
                                self.history.push(format!(
                                    "  array (path): path = #{}", pi));
                            }
                            None => self.history.push(
                                "  array (path) — click ON a curve; missed".into()),
                        }
                        self.refocus_cmd = true;
                    } else if self.xref_pending.is_some() {
                        // XREF attach — this click is the insertion point
                        // (place-multiple).
                        self.commit_xref_at(click_world);
                        self.refocus_cmd = true;
                    } else if self.boundary_state == BoundaryState::WaitingForPick {
                        // BOUNDARY — click INSIDE a closed region (place-multiple).
                        self.commit_boundary_at(click_world);
                        self.refocus_cmd = true;
                    } else if let CenterMarkState::WaitingForClick { size_override } =
                        self.centermark_state.clone()
                    {
                        // CENTERMARK: this click places the mark (place-multiple).
                        self.commit_centermark_at(click_world, size_override);
                        self.refocus_cmd = true;
                    } else if let AttrDefFlow::AwaitingPosition { tag, prompt, default } =
                        self.attr_def_flow.clone()
                    {
                        // ATTDEF placement click — tag/prompt/default already set.
                        self.commit_attdef_at(click_world, &tag, &prompt, &default);
                        self.refocus_cmd = true;
                    } else if self.attedit_state == AttEditState::WaitingForPick {
                        // ATTEDIT pick click — a block instance with attributes.
                        if let Some(i) = self.attedit_pick_block(click_world) {
                            self.attedit_open_for(i);
                        } else {
                            self.fail_op(
                                "attedit: click a block instance with attributes");
                            self.attedit_state = AttEditState::Off;
                            self.clear_prompt();
                        }
                        self.refocus_cmd = true;
                    } else if self.xline_state != XlineState::Off {
                        // XLINE: base click → direction click; place-multiple.
                        match self.xline_state.clone() {
                            XlineState::WaitingForBase => {
                                self.xline_state = XlineState::WaitingForDir { base: click_world };
                                self.history.push(format!(
                                    "    xline: BASE = ({:.3},{:.3}) — click DIRECTION point (or H/V/A)",
                                    click_world.x, click_world.y));
                                self.set_prompt(
                                    "xline: click DIRECTION point  [H/V/A/Off  Esc exits]");
                            }
                            XlineState::WaitingForDir { base } => {
                                let dir = click_world - base;
                                if dir.len() < 1e-9 {
                                    self.fail_op("xline: direction coincides with base");
                                } else {
                                    self.commit_xline(base, dir);
                                }
                            }
                            XlineState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.ray_state != RayState::Off {
                        // RAY: base click → direction click; place-multiple.
                        match self.ray_state.clone() {
                            RayState::WaitingForBase => {
                                self.ray_state = RayState::WaitingForDir { base: click_world };
                                self.history.push(format!(
                                    "    ray: BASE = ({:.3},{:.3}) — click DIRECTION point (or H/V/A)",
                                    click_world.x, click_world.y));
                                self.set_prompt(
                                    "ray: click DIRECTION point  [H/V/A  Esc exits]");
                            }
                            RayState::WaitingForDir { base } => {
                                let d = click_world - base;
                                if d.len() < 1e-9 {
                                    self.fail_op("ray: direction point coincides with base");
                                } else {
                                    self.commit_ray(base, d);
                                }
                            }
                            RayState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.donut_state != DonutState::Off {
                        // DONUT: center → outer → inner; place-multiple.
                        match self.donut_state.clone() {
                            DonutState::WaitingCenter => {
                                self.donut_state = DonutState::WaitingOuter { center: click_world };
                                self.set_prompt(
                                    "donut: click OUTER radius point  [Esc exits]");
                            }
                            DonutState::WaitingOuter { center } => {
                                let r = click_world.dist(center);
                                if r < 1e-9 {
                                    self.fail_op("donut: outer radius is zero");
                                } else {
                                    self.donut_state = DonutState::WaitingInner {
                                        center, outer_radius: r,
                                    };
                                    self.set_prompt(
                                        "donut: click INNER radius point  [Esc exits]");
                                }
                            }
                            DonutState::WaitingInner { center, outer_radius } => {
                                self.commit_donut(center, outer_radius,
                                    click_world.dist(center));
                            }
                            DonutState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.wipeout_state != WipeoutState::Off {
                        // WIPEOUT: two opposite corners; single placement.
                        match self.wipeout_state.clone() {
                            WipeoutState::WaitingFirstCorner => {
                                self.wipeout_state =
                                    WipeoutState::WaitingSecondCorner { first: click_world };
                                self.set_prompt(
                                    "wipeout: pick opposite corner  [Esc exits]");
                            }
                            WipeoutState::WaitingSecondCorner { first } => {
                                self.commit_wipeout(first, click_world);
                            }
                            WipeoutState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if matches!(self.ptdist_state,
                        PtDistribState::DivideObject | PtDistribState::MeasureObject)
                    {
                        // DIVIDE / MEASURE — this click picks the curve.
                        let is_measure = matches!(self.ptdist_state, PtDistribState::MeasureObject);
                        let hit = self.nearest_entity_under(click_world,
                                (self.env.PkBxSz.max(8) as f64)
                                    / (self.scale as f64).max(1e-6));
                        match hit {
                            Some(idx) => {
                                self.ptdist_state = if is_measure {
                                    PtDistribState::MeasureValue(idx)
                                } else {
                                    PtDistribState::DivideValue(idx)
                                };
                                self.set_prompt(if is_measure {
                                    "measure: type the segment LENGTH (drawing units)  [Esc cancels]"
                                        .to_string()
                                } else {
                                    "divide: type the number of segments  [Esc cancels]"
                                        .to_string()
                                });
                            }
                            None => self.history.push(
                                "  divide/measure — click ON a curve; missed".into()),
                        }
                        self.refocus_cmd = true;
                    } else if self.layer_pick != LayerPickState::Off {
                        // LAYISO / LAYFRZ / LAYOFF — the click's dobject
                        // layer is the target.
                        let lid = self.nearest_entity_under(click_world,
                                (self.env.PkBxSz.max(8) as f64)
                                    / (self.scale as f64).max(1e-6))
                            .and_then(|i| self.doc.dobjects.get(i))
                            .map(|d| d.style.layer);
                        match lid {
                            Some(l) => self.handle_layer_pick(l),
                            None => self.fail_op(
                                "layiso/layfrz/layoff: click a dobject on a layer"),
                        }
                        self.refocus_cmd = true;
                    } else if self.dist_state != DistState::Off {
                        match self.dist_state {
                            DistState::WaitingForP1 => {
                                self.dist_state = DistState::WaitingForP2(click_world);
                                self.history.push(format!(
                                    "  dist: P1 = ({:.4}, {:.4}) — click SECOND point",
                                    click_world.x, click_world.y));
                                self.set_prompt("dist: click SECOND point  [Esc=cancel]");
                            }
                            DistState::WaitingForP2(p1) => {
                                let dx = click_world.x - p1.x;
                                let dy = click_world.y - p1.y;
                                let d  = (dx * dx + dy * dy).sqrt();
                                let ang_rad = dy.atan2(dx);
                                let ang_deg = ang_rad.to_degrees();
                                self.history.push(format!(
                                    "  dist: P2 = ({:.4}, {:.4})",
                                    click_world.x, click_world.y));
                                self.history.push(format!(
                                    "  dist: distance = {:.6}", d));
                                self.history.push(format!(
                                    "  dist: ΔX = {:+.6}   ΔY = {:+.6}", dx, dy));
                                self.history.push(format!(
                                    "  dist: angle (from X-axis) = {:+.4}°", ang_deg));
                                self.dist_state = DistState::Off;
                                self.clear_prompt();
                            }
                            DistState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.lengthen_state != LengthenState::Off {
                        if let LengthenState::WaitingForSide(d) = self.lengthen_state {
                            self.apply_lengthen(d, click_world);
                        }
                        self.lengthen_state = LengthenState::Off;
                        self.refocus_cmd = true;
                    } else if self.break_state != BreakState::Off {
                        self.apply_break(click_world);
                        self.break_state = BreakState::Off;
                        self.refocus_cmd = true;
                    } else if self.align_state != AlignState::Off {
                        match self.align_state {
                            AlignState::WaitingForSrc1 => {
                                self.align_state = AlignState::WaitingForSrc2(click_world);
                                self.history.push(format!(
                                    "    align: SRC1 = ({:.2},{:.2}) — click SOURCE point 2",
                                    click_world.x, click_world.y));
                                self.set_prompt(
                                    "align: click SOURCE point 2  [Esc=cancel]");
                            }
                            AlignState::WaitingForSrc2(s1) => {
                                self.align_state = AlignState::WaitingForTgt1(s1, click_world);
                                self.history.push(
                                    "    align: SRC2 captured — click TARGET point 1".into());
                                self.set_prompt(
                                    "align: click TARGET point 1  [Esc=cancel]");
                            }
                            AlignState::WaitingForTgt1(s1, s2) => {
                                self.align_state = AlignState::WaitingForTgt2(s1, s2, click_world);
                                self.history.push(
                                    "    align: TGT1 captured — click TARGET point 2".into());
                                self.set_prompt(
                                    "align: click TARGET point 2  [Esc=cancel]");
                            }
                            AlignState::WaitingForTgt2(s1, s2, t1) => {
                                self.apply_align(s1, s2, t1, click_world);
                                self.align_state = AlignState::Off;
                                self.clear_prompt();
                            }
                            AlignState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.stretch_state != StretchState::Off {
                        match self.stretch_state {
                            StretchState::WaitingForBase(wmin, wmax) => {
                                self.stretch_state = StretchState::WaitingForDest(wmin, wmax, click_world);
                                self.set_prompt("stretch: click DESTINATION point  [Esc=cancel]");
                                self.history.push(
                                    "    stretch: BASE captured — click DESTINATION".into());
                            }
                            StretchState::WaitingForDest(wmin, wmax, base) => {
                                self.apply_stretch(wmin, wmax, base, click_world);
                                self.stretch_state = StretchState::Off;
                                self.clear_prompt();
                            }
                            StretchState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.block_dialog_pick_base {
                        // Dialog "Pick ⊕" — this click is the insertion point.
                        // Write it into the parked dialog and reopen.
                        self.block_dialog_pick_base = false;
                        if let Some(mut dlg) = self.block_dialog_stash.take() {
                            dlg.base_x = format!("{:.4}", click_world.x);
                            dlg.base_y = format!("{:.4}", click_world.y);
                            self.block_dialog = Some(dlg);
                        }
                        self.clear_prompt();
                        self.refocus_cmd = true;
                    } else if self.insert_dialog_pick {
                        // Insert-dialog "Pick ⊕" — this click is the insertion point.
                        self.insert_dialog_pick = false;
                        if let Some(dlg) = self.insert_dialog.as_mut() {
                            dlg.at_x = format!("{:.4}", click_world.x);
                            dlg.at_y = format!("{:.4}", click_world.y);
                        }
                        self.clear_prompt();
                        self.refocus_cmd = true;
                    } else if let Some((k, first)) = self.insert_param_pick {
                        // Insert-dialog "↔" — two clicks; their distance = the value.
                        match first {
                            None => {
                                self.insert_param_pick = Some((k, Some(click_world)));
                                self.set_prompt(
                                    "insert: click the SECOND point  [Esc=cancel]");
                            }
                            Some(p1) => {
                                let dist = (click_world - p1).len();
                                self.insert_param_pick = None;
                                if let Some(dlg) = self.insert_dialog.as_mut() {
                                    if let Some((_, val)) = dlg.params.get_mut(k) {
                                        *val = format!("{dist:.4}");
                                    }
                                }
                                self.clear_prompt();
                            }
                        }
                        self.refocus_cmd = true;
                    } else if self.insert_live.is_some() {
                        // LIVE parametric insert: this click FIXES the current
                        // parameter (value read from the cursor), advances, and
                        // places the block once every parameter is set.
                        let val = {
                            let live = self.insert_live.as_ref().unwrap();
                            self.live_param_value(live, click_world)
                        };
                        let place = {
                            let live = self.insert_live.as_mut().unwrap();
                            if live.idx < live.values.len() { live.values[live.idx] = val; }
                            live.idx += 1;
                            live.idx >= live.values.len()
                        };
                        if place {
                            let live = self.insert_live.take().unwrap();
                            let mut pv = [0.0; cad_kernel::MAX_BLOCK_PARAMS];
                            for (k, v) in live.values.iter().enumerate() {
                                if k < cad_kernel::MAX_BLOCK_PARAMS { pv[k] = *v; }
                            }
                            self.place_block_full(live.block, live.insert, live.scale,
                                live.rotation, pv);
                            self.clear_prompt();
                        } else {
                            let name = {
                                let lv = self.insert_live.as_ref().unwrap();
                                self.doc.blocks.get(lv.block)
                                    .and_then(|b| b.params.get(lv.idx))
                                    .map(|p| p.name.clone()).unwrap_or_default()
                            };
                            self.set_prompt(format!(
                                "insert: drag to set '{name}', click to fix  [Esc=cancel]"));
                        }
                        self.refocus_cmd = true;
                    } else if self.block_def_state != BlockDefState::Off {
                        // block creation — this click is the BASE point.
                        if let BlockDefState::WaitingForBase { name } =
                            std::mem::replace(&mut self.block_def_state, BlockDefState::Off)
                        {
                            self.apply_block_create(&name, click_world, None, false);
                            self.clear_prompt();
                        }
                        self.refocus_cmd = true;
                    } else if let InsertState::WaitingForPoint { block } = self.insert_state {
                        if let Some(pend) = self.pending_insert.take() {
                            // Dialog-configured insert: the click IS the insertion
                            // point. If the block is SMART, don't place yet — enter
                            // LIVE parameter dragging (base fixed, drag each vector,
                            // block deforms under the cursor, click fixes it).
                            self.insert_state = InsertState::Off;
                            let nparams = self.doc.blocks.get(pend.block)
                                .map(|b| b.params.len()).unwrap_or(0);
                            if nparams == 0 {
                                self.place_block_full(
                                    pend.block, click_world, pend.scale, pend.rotation,
                                    pend.param_values);
                                self.clear_prompt();
                            } else {
                                let values: Vec<f64> = {
                                    let blk = self.doc.blocks.get(pend.block).unwrap();
                                    (0..nparams).map(|k| {
                                        let dv = pend.param_values[k];
                                        if dv.abs() > 1e-9 { dv } else { blk.params[k].original }
                                    }).collect()
                                };
                                let name = self.doc.blocks.get(pend.block)
                                    .and_then(|b| b.params.first())
                                    .map(|p| p.name.clone()).unwrap_or_default();
                                self.insert_live = Some(InsertLive {
                                    block: pend.block, insert: click_world,
                                    scale: pend.scale, rotation: pend.rotation,
                                    values, idx: 0,
                                });
                                self.set_prompt(format!(
                                    "insert: drag to set '{name}', click to fix  \
                                     (or type a value)  [Esc=cancel]"));
                            }
                            self.refocus_cmd = true;
                        } else {
                            // Legacy flow: point chosen; now ask for the rotation
                            // ANGLE (click a direction, or Enter = 0).
                            self.insert_state = InsertState::WaitingForAngle {
                                block, insert: click_world };
                            self.set_prompt(
                                "insert: specify ROTATION — click a direction point  \
                                 (Enter = 0°)  [Esc=cancel]");
                            self.refocus_cmd = true;
                        }
                    } else if let InsertState::WaitingForAngle { block, insert } =
                        self.insert_state
                    {
                        // Direction click → rotation, then place + cut.
                        let rotation = (click_world - insert).angle();
                        self.insert_state = InsertState::Off;
                        self.apply_insert(block, insert, rotation);
                        self.refocus_cmd = true;
                    } else if self.matchprops_state == MatchPropsState::WaitingForSource {
                        // Pick the SOURCE, then hand off to a normal selection
                        // session for the targets (so window / crossing / W / C
                        // / B / all all work). Applied on Enter via
                        // QueuedOp::MatchPropPaint.
                        let tol_world = 10.0 / self.scale as f64;
                        match self.nearest_entity_under(world, tol_world) {
                            Some(src) => {
                                // Preserve any pre-existing bank so `B` (before)
                                // recovers it after begin_selection clears it.
                                if !self.selection.is_empty() {
                                    self.selection_prev = self.selection.clone();
                                }
                                self.matchprops_state = MatchPropsState::Off;
                                self.begin_selection(SelectMode::ForSelect);
                                self.queued_op = QueuedOp::MatchPropPaint(src);
                                self.set_prompt(
                                    "matchprop: select TARGETS — click, drag window/crossing, \
                                     or W/C/B/all; Enter applies  [Esc=cancel]");
                                self.history.push(format!(
                                    "  matchprop — source #{}; select targets \
                                     (window/crossing/B/all), Enter to apply", src));
                            }
                            None => self.history.push(
                                "  matchprop — no object under cursor (click the SOURCE, Esc cancels)".into()),
                        }
                        self.refocus_cmd = true;
                    } else if self.fillet_state != FilletState::Off {
                        // Slice M.3 — pick first object, then second.
                        let tol_world = 10.0 / self.scale as f64;
                        let hit = self.nearest_entity_under(world, tol_world);
                        match (self.fillet_state, hit) {
                            // Polyline mode (P option): the first pick is a
                            // polyline whose every corner gets rounded.
                            (FilletState::WaitingForFirst(r), Some(i)) if self.fillet_poly_all => {
                                self.apply_fillet_poly_all(r, i);
                                if self.fillet_waiting_radius {
                                    // Radius too large — stay put; the apply
                                    // armed a "type a smaller radius" prompt.
                                } else if self.fillet_multiple {
                                    self.fillet_state = FilletState::WaitingForFirst(r);
                                    self.refresh_fillet_prompt();
                                } else {
                                    self.fillet_state = FilletState::Off;
                                }
                            }
                            (FilletState::WaitingForFirst(r), Some(i)) => {
                                self.fillet_state = FilletState::WaitingForSecond(r, i, click_world);
                                self.history.push(format!(
                                    "  fillet — first = #{}. Click SECOND object (or another segment of the same polyline).", i));
                            }
                            (FilletState::WaitingForSecond(r, i1, p1), Some(i2)) => {
                                self.apply_fillet(r, i1, p1, i2, click_world);
                                // Multiple-mode loop: re-enter the
                                // first-pick state with the same radius
                                // instead of returning to Off. Esc
                                // exits. Single-mode (default) → Off.
                                if self.fillet_waiting_radius {
                                    // Radius too large — stay; re-prompt armed.
                                } else if self.fillet_multiple {
                                    self.fillet_state = FilletState::WaitingForFirst(r);
                                    self.refresh_fillet_prompt();
                                } else {
                                    self.fillet_state = FilletState::Off;
                                }
                            }
                            _ => self.history.push(
                                "  fillet — click ON a line; missed".into()),
                        }
                        self.refocus_cmd = true;
                    } else if self.chamfer_state != ChamferState::Off {
                        // Slice M.4 — pick first object, then second.
                        let tol_world = 10.0 / self.scale as f64;
                        let hit = self.nearest_entity_under(world, tol_world);
                        match (self.chamfer_state, hit) {
                            (ChamferState::WaitingForFirst(d1, d2), Some(i)) if self.chamfer_poly_all => {
                                self.apply_chamfer_poly_all(d1, d2, i);
                                if self.chamfer_dist_wait != ChamferDistWait::Off {
                                    // Distance too large — stay; re-prompt armed.
                                } else if self.chamfer_multiple {
                                    self.chamfer_state = ChamferState::WaitingForFirst(d1, d2);
                                    self.refresh_chamfer_prompt();
                                } else {
                                    self.chamfer_state = ChamferState::Off;
                                }
                            }
                            (ChamferState::WaitingForFirst(d1, d2), Some(i)) => {
                                self.chamfer_state =
                                    ChamferState::WaitingForSecond(d1, d2, i, click_world);
                                self.history.push(format!(
                                    "  chamfer — first = #{}. Click SECOND object (or another segment of the same polyline).", i));
                            }
                            (ChamferState::WaitingForSecond(d1, d2, i1, p1), Some(i2)) => {
                                self.apply_chamfer(d1, d2, i1, p1, i2, click_world);
                                if self.chamfer_dist_wait != ChamferDistWait::Off {
                                    // Distance too large — stay; re-prompt armed.
                                } else if self.chamfer_multiple {
                                    self.chamfer_state =
                                        ChamferState::WaitingForFirst(d1, d2);
                                    self.refresh_chamfer_prompt();
                                } else {
                                    self.chamfer_state = ChamferState::Off;
                                }
                            }
                            _ => self.history.push(
                                "  chamfer — click ON a line; missed".into()),
                        }
                        self.refocus_cmd = true;
                    } else if self.mirror_state != MirrorState::Off {
                        match self.mirror_state {
                            MirrorState::WaitingForA => {
                                self.mirror_state = MirrorState::WaitingForB(click_world);
                                self.history.push(format!(
                                    "    mirror: A = ({:.3}, {:.3}) — click SECOND axis point",
                                    click_world.x, click_world.y));
                            }
                            MirrorState::WaitingForB(a) => {
                                // Axis fixed — ask whether to keep the
                                // original before committing.
                                self.mirror_state =
                                    MirrorState::AwaitingKeep(a, click_world);
                                self.set_prompt(
                                    "mirror: keep original? [Y]/n  \
                                     (Enter = keep a copy, n/no = erase original)");
                            }
                            MirrorState::AwaitingKeep(..) => {
                                // Answer comes from the command line, not a
                                // click — ignore stray canvas clicks here.
                            }
                            MirrorState::Off => unreachable!(),
                        }
                        self.refocus_cmd = true;
                    } else if self.select_mode != SelectMode::Off {
                        let shift = ctx.input(|i| i.modifiers.shift);
                        let alt   = ctx.input(|i| i.modifiers.alt);
                        let tol_world = 10.0 / self.scale as f64;
                        // If the user explicitly typed `w` or `c`, their
                        // intent is unambiguous: this click starts a
                        // window-selection rectangle, not a single-dobject
                        // pick. Skip the nearest-entity heuristic — at high
                        // zoom-out the 10-px tol almost always hits SOMETHING
                        // and would steal the click. The armed flag survives
                        // here and gets consumed by add_window_selection on
                        // the second corner click.
                        let armed_window = self.armed_window_inside.is_some();
                        let hit = if armed_window {
                            None
                        } else {
                            self.nearest_entity_under(world, tol_world)
                        };
                        if let Some(i) = hit {
                            // In a command select session — accumulate (not fresh).
                            self.click_select(i, shift, alt, false);
                            self.window_first = None;   // any half-started window is dropped
                        } else if let Some(first) = self.window_first.take() {
                            self.add_window_selection(first, world, shift, alt, false);
                        } else {
                            self.window_first = Some(world);
                            let hint = match self.armed_window_inside {
                                Some(true)  => "    window (armed INSIDE): click OPPOSITE corner".to_string(),
                                Some(false) => "    window (armed CROSSING): click OPPOSITE corner".to_string(),
                                None        => "    window: click opposite corner (L→R inside, R→L crossing — Shift adds, Alt subtracts)".to_string(),
                            };
                            self.history.push(hint);
                        }
                        self.refocus_cmd = true;
                    } else if self.intersect_pending_click {
                        let world_r = 50.0 / self.scale as f64;
                        self.intersect_near(world, world_r);
                        self.intersect_pending_click = false;
                    } else if self.tool == Tool::None {
                        // Pointer-mode click — the always-on selector. Click
                        // on a dobject adds it to the basket; Shift removes.
                        // Click on EMPTY space deselects everything (AutoCAD
                        // convention — gives the user a clean slate without
                        // typing anything). Shift-click on empty preserves
                        // the basket so missing the dobject by a few pixels
                        // mid-shift-multi-select doesn't wipe it.
                        // See `feedback_rust_cad_pointer_is_selector` memo.
                        let shift = ctx.input(|i| i.modifiers.shift);
                        let alt   = ctx.input(|i| i.modifiers.alt);
                        let tol_world = 10.0 / self.scale as f64;
                        // A Tab-cycled deeper candidate is honored when the
                        // click lands at the cycle spot (any pointer-mode
                        // click then consumes the cycle state).
                        let hit = self.cycled_pick_for(world, tol_world)
                            .or_else(|| self.nearest_entity_under(world, tol_world));
                        self.clear_pick_cycle();
                        if let Some(i) = hit {
                            // Pointer-mode pick: plain = fresh selection.
                            self.click_select(i, shift, alt, true);
                            // Picking a group member selects the whole group.
                            if !alt { self.expand_selection_to_groups(); }
                        } else if !shift && !alt {
                            self.selection.clear();
                            self.selected = None;
                        }
                        self.refocus_cmd = true;
                    } else if self.picking_source {
                        let tol_world = 10.0 / self.scale as f64;
                        match self.nearest_entity_under(world, tol_world) {
                            Some(i) => {
                                self.selected = Some(i);
                                self.picking_source = false;
                                self.history.push(format!("  + picked dobject #{}", i));
                            }
                            None => {
                                self.history.push("  ! no dobject near click — try clicking closer to the curve, or pick from the right panel".into());
                            }
                        }
                    } else if self.tool != Tool::None {
                        // Use the precomputed snap hit if one is available.
                        // One-shot typed overrides are consumed regardless of
                        // whether the hit succeeded — Esc still cancels.
                        let click_world = match snap_hit {
                            Some(h) => {
                                self.history.push(format!(
                                    "  ↳ {} → ({:.3},{:.3})",
                                    h.kind.name(), h.point.x, h.point.y
                                ));
                                h.point
                            }
                            None => {
                                if self.snap_override.is_some() {
                                    self.history.push(
                                        "  ! snap missed — used raw click".into());
                                }
                                // Apply CARD + grid-snap (priority osnap >
                                // CARD > grid > raw), same as the click_world
                                // used for move/copy/etc. WITHOUT this the
                                // committed draw point ignored CARD and grid
                                // even though the live preview honoured them.
                                self.apply_constraints(world)
                            }
                        };
                        if self.snap_override.is_some() {
                            self.snap_override = None;
                        }
                        // Polyline maintains a parallel bulge per segment:
                        // bulge[i] is the segment from pending[i] to
                        // pending[i+1]. In Arc sub-mode the just-clicked
                        // segment gets a tangent-continuous arc bulge;
                        // in Line sub-mode (or any other tool) it stays 0.
                        //
                        // Second-pt flow (typed `s` in Arc mode) is
                        // 2-stage: the first click captures an on-arc
                        // midpoint WITHOUT committing a vertex; the
                        // second click commits the endpoint using a
                        // 3-point-arc bulge.
                        let pline_handled = if self.tool == Tool::Polyline {
                            match self.pline_arc_sub {
                                PlineArcSub::AwaitingSecondPt => {
                                    self.pline_arc_sub =
                                        PlineArcSub::AwaitingSecondPtEnd(click_world);
                                    self.history.push(format!(
                                        "    pline·ARC second-pt: ({:.3},{:.3}) — click ENDPOINT",
                                        click_world.x, click_world.y));
                                    self.update_pline_prompt();
                                    true
                                }
                                PlineArcSub::AwaitingSecondPtEnd(mid) => {
                                    if let Some(&start) = self.pending.last() {
                                        let bulge = bulge_from_three_points(
                                            start, mid, click_world);
                                        self.pending_bulges.push(bulge);
                                        self.pending_widths.push(self.pline_next_width);
                                        self.pline_next_width =
                                            (self.pline_next_width.1, self.pline_next_width.1);
                                        self.pending.push(click_world);
                                    }
                                    self.pline_arc_sub = PlineArcSub::Normal;
                                    self.update_pline_prompt();
                                    true
                                }
                                // Direction: this click sets the arc's START
                                // tangent (last vertex → click). No vertex is
                                // committed; the NEXT click places the endpoint
                                // and the arc bulge uses this override.
                                PlineArcSub::AwaitingDirection => {
                                    if let Some(&start) = self.pending.last() {
                                        let dir = click_world - start;
                                        if dir.len() > EPS {
                                            let u = dir / dir.len();
                                            self.pline_dir_override = Some(u);
                                            self.history.push(format!(
                                                "    pline·ARC direction ({:.3},{:.3}) — click ENDPOINT",
                                                u.x, u.y));
                                        }
                                    }
                                    self.pline_arc_sub = PlineArcSub::Normal;
                                    self.update_pline_prompt();
                                    true
                                }
                                PlineArcSub::Normal => false,
                            }
                        } else { false };
                        if !pline_handled {
                            // PLINE auto-close: a click that lands on
                            // (or within pickbox-tolerance of) vertex[0]
                            // when at least 3 vertices are already in
                            // `pending` commits the polyline as CLOSED
                            // and exits drawing mode. Matches AutoCAD's
                            // behaviour where snapping back to the start
                            // ends the PLINE command.
                            let auto_closed = self.tool == Tool::Polyline
                                && self.pending.len() >= 3
                                && {
                                    let first = self.pending[0];
                                    let world_tol = (self.env.PkBxSz.max(4) as f64)
                                        / (self.scale as f64).max(1e-6);
                                    (click_world - first).len() < world_tol
                                };
                            if auto_closed {
                                let (verts, widths) = self.drain_pline_pending(true);
                                self.add_dobject(Geom::Polyline(Polyline {
                                    vertices: verts, closed: true, widths,
                                }), "canvas (auto-closed on first vertex)");
                                self.update_pline_prompt();
                            } else {
                                if self.tool == Tool::Polyline && !self.pending.is_empty() {
                                    let new_bulge = if self.pline_mode == PlineMode::Arc {
                                        self.pline_arc_bulge_to(click_world)
                                    } else {
                                        0.0
                                    };
                                    self.pending_bulges.push(new_bulge);
                                    // Width for this segment; the end width
                                    // carries as the next segment's start.
                                    self.pending_widths.push(self.pline_next_width);
                                    self.pline_next_width =
                                        (self.pline_next_width.1, self.pline_next_width.1);
                                    // The Direction override applies to ONE arc
                                    // segment only — consume it now.
                                    self.pline_dir_override = None;
                                }
                                // Smart-dim flow: handle the click via
                                // dim_draft instead of pending so the
                                // sub-kind branching is explicit and
                                // doesn't fight the count-based
                                // try_finalise dispatch.
                                let dim_handled = if self.tool == Tool::Dim {
                                    self.handle_dim_click(click_world);
                                    true
                                } else { false };
                                if !dim_handled {
                                    self.pending.push(click_world);
                                    if self.tool == Tool::Polyline {
                                        self.update_pline_prompt();
                                    }
                                    self.try_finalise();
                                }
                            }
                        }
                        // canvas click steals focus away from the command box;
                        // restore it so typing keeps working without a manual
                        // click into the field.
                        self.refocus_cmd = true;
                    }
                }
            }

            // axes — model-space decoration; on a layout tab the paper render
            // already painted the page (modal flows keep the canvas lanes, so
            // the model geometry must not re-draw OVER the paper below).
            if !in_layout {
            let origin = self.w2s(Vec2::ZERO, rect);
            let axis_col = egui::Color32::from_rgb(46, 56, 70);
            painter.line_segment(
                [egui::pos2(rect.left(), origin.y),
                 egui::pos2(rect.right(), origin.y)],
                egui::Stroke::new(1.0, axis_col),
            );
            painter.line_segment(
                [egui::pos2(origin.x, rect.top()),
                 egui::pos2(origin.x, rect.bottom())],
                egui::Stroke::new(1.0, axis_col),
            );
            painter.circle_stroke(
                origin, 5.0,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(240, 210, 70)),
            );
            }

            // dobjects — viewport-culled. We compute the visible world rect once
            // and skip any dobject whose bbox doesn't overlap it. Still O(N) per
            // frame in the worst case (everything visible), but the painter cost
            // is the real bottleneck, and culling lets it scale far better when
            // you zoom in on a corner of a big drawing.
            let v_tl = self.s2w(rect.left_top(),     rect);
            let v_br = self.s2w(rect.right_bottom(), rect);
            let v_min = Vec2::new(v_tl.x.min(v_br.x), v_tl.y.min(v_br.y));
            let v_max = Vec2::new(v_tl.x.max(v_br.x), v_tl.y.max(v_br.y));
            self.last_visible = Some((v_min, v_max));

            // Execute deferred "∩ view" now that we know the viewport bbox.
            if self.intersect_view_pending {
                self.intersect_view_pending = false;
                self.intersect_in_bbox(v_min, v_max);
            }

            // Source the candidate indices: if a fresh index exists, query it
            // (O(visible cells)); otherwise fall back to O(N) iteration. The
            // index loop is dramatically faster at 1M+ dobjects.
            //
            // Rebuild the index FIRST if dirty — otherwise every move /
            // copy / array invalidates it and the renderer wastes a frame
            // iterating all N. One rebuild pays for itself in milliseconds
            // when N reaches the millions (≈100 ms rebuild vs ≈100 ms per
            // frame of full-N iteration, every frame, forever).
            //
            // Collected into a Vec so we can both COUNT the candidates
            // (= `in_viewport` in the screen-stats panel) and iterate
            // them twice (CPU vs GPU branches consume the same set).
            let _ = self.ensure_index();
            // FRAME INSTRUMENTATION — see DbgEvent::SlowFrame. Two Instants per frame
            // (~40 ns) is free next to what we're measuring, and "is zoom/pan slow?"
            // was otherwise unfalsifiable from a dump.
            let t_frame = std::time::Instant::now();
            let t_query = std::time::Instant::now();
            let candidates: Vec<usize> = if in_layout {
                // Paper space: the model was already drawn inside the layout's
                // viewports by `paper_space_render`; the modal lanes must not
                // re-draw it over the page.
                Vec::new()
            } else if let (Some(g), false) = (self.index.as_ref(), self.index_dirty) {
                g.query_bbox(v_min, v_max).into_iter().map(|u| u as usize).collect()
            } else {
                (0..self.doc.dobjects.len()).collect()
            };
            let query_us = t_query.elapsed().as_micros() as u64;
            let in_viewport = candidates.len();

            // SELECTION MASK — built ONCE per frame so the draw loop's "is this
            // selected?" test is O(1) instead of a linear Vec scan per dobject.
            // See `sel_mask`: this is the difference between a 25 ms frame and a
            // 21,815 ms one.
            self.sel_mask.clear();
            self.sel_mask.resize(self.doc.dobjects.len(), false); // reuses the alloc
            for &i in &self.selection {
                if let Some(m) = self.sel_mask.get_mut(i) { *m = true; }
            }

            let t_draw = std::time::Instant::now();

            // ---- Hatch geometry cache (the perf fix) ---------------------
            // Hatch generation (pattern scanline-clip, solid ear-clip) is the
            // expensive part; without caching it re-ran for EVERY hatch EVERY
            // frame → dense/many hatches froze the app. Now it runs once per
            // GEOMETRY change (cache cleared in `ensure_index`, which dirties
            // only on geometry — a pure selection/highlight/camera change must
            // NOT re-generate every hatch, B24/GP1), with a PER-FRAME
            // GENERATION BUDGET so a huge batch of new hatches fills in over
            // several frames instead of freezing one. Skipped in APX (dots
            // only). `hatch_building` drives the left-corner "building…" note.
            let mut hatch_building = 0usize;
            if self.render_mode != RenderMode::Apx {
                let pending: Vec<usize> = candidates.iter().copied().filter(|&i| {
                    self.doc.dobjects.get(i).map_or(false, |d|
                        matches!(d.geom, Geom::Hatch(_))
                        && !self.hatch_cache.contains_key(&d.handle))
                }).collect();
                hatch_building = pending.len();
                // Budget by GENERATED-PRIMITIVE count, not hatch count — one
                // dense pattern hatch can be 10k+ lines, so a plain count cap
                // could still spike a frame. Always builds at least one so a
                // single huge hatch still makes progress.
                let mut work = 0usize;
                for i in pending {
                    if work >= HATCH_GEN_WORK_BUDGET { break; }
                    let handle = self.doc.dobjects[i].handle;
                    let entry = self.build_hatch_cache_entry(i);
                    work += entry.segs.len() + entry.circs.len()
                          + entry.solid.iter().map(|(_, t)| t.len()).sum::<usize>();
                    self.hatch_cache.insert(handle, entry);
                    hatch_building -= 1;
                }
                // Keep painting until the cache is fully built.
                if hatch_building > 0 { ctx.request_repaint(); }
            }

            let candidate_iter: Box<dyn Iterator<Item = usize>> =
                Box::new(candidates.into_iter());

            // DObject supplying the active snap, if any — highlighted in cyan
            // so the user can see "this is what I'm anchoring against" even
            // when the snap point lands far away on the dobject's extension.
            let snap_source: Option<usize> = snap_hit.and_then(|h| h.dobject);

            // ---- Cutter / boundary highlighting during trim / extend
            // target-pick phase. Cutters render in warm orange so the user
            // sees what's actually intersecting. See memo
            // `feedback_rust_cad_trim_default_all_cutters`.
            // `Some(None)` means ALL-mode (paint every visible dobject as
            // cutter/boundary); `Some(Some(&[..]))` means explicit list.
            let trim_cutters: Option<Option<&Vec<usize>>> = match &self.trim_state {
                TrimState::PickingTargets(c)    => Some(Some(c)),
                TrimState::PickingTargetsAll    => Some(None),
                _                               => None,
            };
            let extend_bounds: Option<Option<&Vec<usize>>> = match &self.extend_state {
                ExtendState::PickingTargets(b)  => Some(Some(b)),
                ExtendState::PickingTargetsAll  => Some(None),
                _                               => None,
            };
            let cutter_color   = egui::Color32::from_rgb(255, 170,  60); // warm orange
            let boundary_color = egui::Color32::from_rgb(255, 220,  90); // warm amber
            // Item 5 — pulse alpha for the cutter/boundary OVERLAY. Real
            // dobject color renders normally underneath so similar-coloured
            // neighbours stay distinguishable. The overlay pulses in/out
            // at ~1.4 Hz (≈ 700 ms full cycle), driven by ctx.input().time.
            // When the trim/extend session is live we request a repaint
            // every 80 ms so the animation stays smooth without burning
            // GPU at full vsync.
            // Keep the pulse animation refreshing whenever there's
            // anything pulsing: trim cutters, extend boundaries, OR a
            // non-empty selection basket (the dashed overlay shares
            // the same pulse). Without this the basket would freeze
            // at whatever phase it was in when the last user input
            // arrived.
            let cutter_or_bound_active =
                trim_cutters.is_some() || extend_bounds.is_some();
            let fillet_or_chamfer_picked = matches!(
                self.fillet_state,  FilletState::WaitingForSecond(..))
                || matches!(self.chamfer_state, ChamferState::WaitingForSecond(..));
            let offset_picked = matches!(
                self.offset_state, OffsetState::WaitingForSide(..));
            let dist_picked = matches!(self.dist_state, DistState::WaitingForP2(_));
            let text_typing = matches!(self.text_draft,
                TextDraftState::WaitingForString(_));
            let pulse_animation_active =
                cutter_or_bound_active
                || !self.selection.is_empty()
                || fillet_or_chamfer_picked
                || offset_picked
                || dist_picked
                || text_typing;
            if pulse_animation_active {
                ctx.request_repaint_after(std::time::Duration::from_millis(80));
            }
            let pulse_t = ctx.input(|i| i.time);
            // sin: -1..1  →  pulse: 0.15..0.85
            let pulse = 0.5 + 0.35 * (pulse_t * std::f64::consts::TAU * 1.4).sin();
            let pulse_alpha = (pulse.clamp(0.15, 0.85) * 255.0) as u8;

            let mut drawn   = 0usize;
            let mut skipped = 0usize;
            let mut gpu_circles_count = 0usize;
            let mut gpu_line_count = 0usize;
            let mut gpu_fill_count = 0usize;
            // Set true when the CPU path hit CPU_DRAW_BUDGET and stopped
            // early — drives the "switch to GPU/APX" safety banner.
            let mut cpu_capped = false;

            // === APX (draft display) render branch ===
            // In APX mode every visible dobject becomes a single dot at its
            // gravity point, pushed into the GPU instanced-circle pipeline as a
            // tiny world-radius circle — one draw call for the entire scene, FPS
            // recovers from single-digit to 60+. The full-geometry render_mode
            // arms are skipped in APX. Hit-testing, snap, and selection still
            // use the underlying geometry — only the visual is approximate.
            if self.render_mode == RenderMode::Apx {
                let mut dots: Vec<CircleInstance> = Vec::new();
                // Screen-target dot radius → world radius for this frame.
                // 1.5 px is small enough not to clutter at typical zooms
                // but visible as a clear dot.
                let dot_world_r = (1.5_f64 / (self.scale as f64).max(1e-9)) as f32;
                for i in candidate_iter {
                    let e = &self.doc.dobjects[i];
                    if !e.style.visible || !self.doc.layers.renders(e.style.layer) {
                        skipped += 1;
                        continue;
                    }
                    let (emin, emax) = e.bbox();
                    if emax.x < v_min.x || emin.x > v_max.x
                    || emax.y < v_min.y || emin.y > v_max.y {
                        continue;
                    }
                    // Anchor strategy. Only 0 (bbox center) implemented
                    // this slice; 1 (primitive center) and 2 (first
                    // vertex) fall back to bbox center.
                    let anchor = match self.env.LodAnc {
                        _ => Vec2::new((emin.x + emax.x) * 0.5,
                                       (emin.y + emax.y) * 0.5),
                    };
                    // Selection / snap highlight wins over per-dobject
                    // color so the user can still pick out selected items.
                    let in_selection = self.sel_mask.get(i).copied().unwrap_or(false);
                    let color = if self.selected == Some(i) || in_selection {
                        egui::Color32::from_rgb(255, 200, 80)
                    } else if snap_source == Some(i) {
                        egui::Color32::from_rgb(120, 240, 255)
                    } else {
                        let (r, g, b) = resolve_color(
                            e.style.color, e.style.layer,
                            &self.doc.layers, &self.doc.truecolors);
                        egui::Color32::from_rgb(r, g, b)
                    };
                    let packed: u32 =
                          ((color.r() as u32) << 24)
                        | ((color.g() as u32) << 16)
                        | ((color.b() as u32) <<  8)
                        |  (color.a() as u32);
                    dots.push(CircleInstance {
                        x: (anchor.x + self.world_offset.x as f64) as f32,
                        y: (anchor.y + self.world_offset.y as f64) as f32,
                        r: dot_world_r,
                        color: packed,
                    });
                    drawn += 1;
                }
                gpu_circles_count = dots.len();
                if !dots.is_empty() {
                    // Camera-relative: world_offset is folded into the instance
                    // coords above, so the view matrix uses offset 0.
                    let view = view_matrix(
                        rect.width(), rect.height(),
                        self.scale, 0.0, 0.0,
                    );
                    let renderer = self.gpu_renderer.clone();
                    let gpu_pad = 2.0 / self.scale.max(1e-6) as f32;
                    let cb = egui::PaintCallback {
                        rect,
                        callback: StdArc::new(
                            egui_glow::CallbackFn::new(
                                move |_info, gl_painter| {
                                    let gl = gl_painter.gl();
                                    let mut r = renderer.lock().unwrap();
                                    r.ensure_init(gl);
                                    r.render(gl, &[], &dots, &[], &[], &[], &view, gpu_pad);
                                },
                            ),
                        ),
                    };
                    painter.add(egui::Shape::Callback(cb));
                }
                self.gpu_dirty = false;
            } else { match self.render_mode {
                RenderMode::Cpu => {
                    for i in candidate_iter {
                        // Safety budget: stop before the frame time explodes.
                        // Everything drawn so far still shows; the rest is
                        // deferred with a banner telling the user to switch to
                        // GPU/APX. Keeps the app responsive on heavy drawings
                        // instead of freezing until the OS force-closes it.
                        if drawn >= CPU_DRAW_BUDGET { cpu_capped = true; break; }
                        let e = &self.doc.dobjects[i];
                        // Layer-level visibility gate — hidden/frozen layers
                        // skip render entirely. Per-Dobject visibility is
                        // honoured the same way.
                        if !e.style.visible || !self.doc.layers.renders(e.style.layer) {
                            skipped += 1;
                            continue;
                        }
                        // ---- Hatch short-circuit (BEFORE viewport /
                        // micro-cull / state-branches). Hatch's bbox is a
                        // (0,0) placeholder (the kernel can't resolve
                        // boundary handles to a real bbox), so the
                        // viewport-bbox cull and the bbox_px micro-cull
                        // below would both wrongly drop it. The selection-
                        // dashed and trim-pulse branches also call
                        // render functions that stub Hatch as a no-op.
                        // Dispatch directly to render_hatch_fill here so
                        // none of that matters.
                        if let Geom::Hatch(_) = &e.geom {
                            let color = if self.selected == Some(i)
                                || self.sel_mask.get(i).copied().unwrap_or(false)
                            {
                                egui::Color32::from_rgb(255, 200, 80)
                            } else if snap_source == Some(i) {
                                egui::Color32::from_rgb(120, 240, 255)
                            } else {
                                let (r, g, b) = resolve_color(
                                    e.style.color, e.style.layer,
                                    &self.doc.layers, &self.doc.truecolors);
                                egui::Color32::from_rgb(r, g, b)
                            };
                            // Cached geometry (built once per doc change). Pattern
                            // → line/circle strokes; solid → a triangle mesh
                            // (even loops = color, odd = bg over-draw for holes).
                            if let Some(entry) = self.hatch_cache.get(&e.handle) {
                                // Shared painter (also used by the layout
                                // viewport path) — one depth-ordered mesh.
                                paint_cached_hatch(&painter, rect, self, entry, color,
                                    egui::Color32::from_rgb(18, 22, 28));
                            }
                            drawn += 1;
                            continue;
                        }
                        // BlockRefs resolve their real extent through the
                        // block table — the kernel bbox is just the insertion
                        // point (0×0), which would BOTH pass the viewport cull
                        // spuriously AND trip the sub-pixel micro-cull below,
                        // making every instance invisible (same reason Hatch
                        // is short-circuited above).
                        let (emin, emax) = match &e.geom {
                            Geom::BlockRef(br) => self.resolved_blockref_bbox(br),
                            _ => e.bbox(),
                        };
                        if emax.x < v_min.x || emin.x > v_max.x
                        || emax.y < v_min.y || emin.y > v_max.y {
                            continue;
                        }
                        let bbox_px = (emax.x - emin.x).max(emax.y - emin.y) as f32 * self.scale;
                        let in_selection = self.sel_mask.get(i).copied().unwrap_or(false);
                        if bbox_px < 1.0
                            && self.selected != Some(i)
                            && snap_source != Some(i)
                            && !in_selection
                        {
                            skipped += 1;
                            continue;
                        }
                        // Trim/extend visualization: cutters render warm-orange,
                        // boundaries warm-amber. Solid thick lines (NOT dashed)
                        // so they're distinguishable from basket dashed-gray.
                        let is_cutter = match trim_cutters {
                            Some(None)    => true,                 // ALL-mode
                            Some(Some(c)) => c.contains(&i),
                            None          => false,
                        };
                        let is_boundary = match extend_bounds {
                            Some(None)    => true,
                            Some(Some(b)) => b.contains(&i),
                            None          => false,
                        };
                        if is_cutter || is_boundary {
                            // Real dobject color FIRST (so similar-coloured
                            // neighbours stay distinguishable; the user's
                            // "stop turning everything yellow" complaint).
                            let (r, g, b) = resolve_color(
                                e.style.color, e.style.layer, &self.doc.layers,
                                &self.doc.truecolors,
                            );
                            draw_dobject(&painter, rect, self, &e.geom,
                                egui::Color32::from_rgb(r, g, b));
                            // Pulsing overlay on top — warm orange for
                            // cutters, warm amber for boundaries, alpha
                            // breathing in/out so the user sees motion
                            // instead of a colour swap.
                            let base = if is_cutter { cutter_color } else { boundary_color };
                            let pulse_col = egui::Color32::from_rgba_unmultiplied(
                                base.r(), base.g(), base.b(), pulse_alpha,
                            );
                            draw_dobject_thick(&painter, rect, self, &e.geom,
                                pulse_col, 4.0);
                            drawn += 1;
                            continue;
                        }
                        // Basket members + fillet/chamfer first-pick:
                        // render the real dobject (its resolved color)
                        // first, THEN overlay an animated dashed pulse
                        // on top. Mirrors the trim/extend method —
                        // same pulse_alpha breathing rate, just dashed
                        // instead of thick-solid.
                        //
                        // Color, width, dash/gap and pulse range are
                        // hardcoded for now; planned SYSVARs listed in
                        // Variables.md.
                        let is_fillet_first = matches!(
                            self.fillet_state,
                            FilletState::WaitingForSecond(_, fi, _) if fi == i);
                        let is_chamfer_first = matches!(
                            self.chamfer_state,
                            ChamferState::WaitingForSecond(_, _, ci, _) if ci == i);
                        let is_offset_src = matches!(
                            self.offset_state,
                            OffsetState::WaitingForSide(_, oi) if oi == i);
                        if in_selection || is_fillet_first || is_chamfer_first || is_offset_src {
                            let (r, g, b) = resolve_color(
                                e.style.color, e.style.layer, &self.doc.layers,
                                &self.doc.truecolors);
                            draw_dobject(&painter, rect, self, &e.geom,
                                egui::Color32::from_rgb(r, g, b));
                            // Pulsing dashed overlay — gray-cyan reads
                            // distinctly against both light and dark
                            // dobject colors.
                            let base = egui::Color32::from_rgb(180, 210, 230);
                            let pulse_col = egui::Color32::from_rgba_unmultiplied(
                                base.r(), base.g(), base.b(), pulse_alpha);
                            draw_dobject_dashed(&painter, rect, self, &e.geom,
                                pulse_col, 6.0, 4.0);
                            drawn += 1;
                            continue;
                        }
                        let color = if self.selected == Some(i) {
                            egui::Color32::from_rgb(255, 200, 80)
                        } else if snap_source == Some(i) {
                            egui::Color32::from_rgb(120, 240, 255)
                        } else {
                            // Resolve through ByLayer / ByBlock to a concrete RGB.
                            let (r, g, b) = resolve_color(
                                e.style.color, e.style.layer, &self.doc.layers,
                                &self.doc.truecolors,
                            );
                            egui::Color32::from_rgb(r, g, b)
                        };
                        // Hatch needs Document access (to resolve its
                        // boundary-handle references) — short-circuit to
                        // a dedicated renderer that has &self.
                        if let Geom::Hatch(h) = &e.geom {
                            self.render_hatch_fill(&painter, rect, h, color);
                            drawn += 1;
                            continue;
                        }
                        // Honour style.linetype (resolved through ByLayer
                        // when needed). Solid for Continuous; dashed via
                        // egui::Shape::dashed_line otherwise.
                        paint_dobject_with_style(&painter, rect, self, e, color);
                        drawn += 1;
                    }
                }
                RenderMode::Gpu => {
                    // GPU path. Pipelines: CIRCLE / ARC / ELLIPSE (analytic ring
                    // SDFs), LINE (instanced SDF), FILL (triangle soup). Mapping:
                    //   Line / Point / Spline / straight-thin Polyline → LINE
                    //   Circle → CIRCLE ·  Arc → ARC ·  Ellipse → ELLIPSE (all
                    //     analytic, no CPU tessellation) ·  EllipseArc → LINE
                    //   NON-continuous linetype (any stroked geom) → dashed LINE
                    //   pattern Hatch → LINE + CIRCLE ·  solid Hatch → FILL
                    //   Wall → FILL (poché) + LINE (faces/insulation/centerline)
                    // Still on the egui painter (already GPU-drawn by egui):
                    //   Text, Dimension, BlockRef, curved/wide Polyline.
                    //
                    // Coords are camera-relative: (world + world_offset) as f32,
                    // f64 add first, so f32 stays precise far from the origin.
                    // Color resolution matches the CPU branch (selection=yellow,
                    // snap source=cyan, else the dobject's resolved style color).
                    let mut circles:  Vec<CircleInstance>  = Vec::new();
                    let mut arcs:     Vec<ArcInstance>     = Vec::new();
                    let mut ellipses: Vec<EllipseInstance> = Vec::new();
                    let mut lines:    Vec<LineInstance>    = Vec::new();
                    let mut fills:    Vec<FillVertex>      = Vec::new();
                    let ox = self.world_offset.x as f64;
                    let oy = self.world_offset.y as f64;
                    // Default hairline half-width in WORLD units (~1.6px stroke);
                    // the shader also enforces a screen minimum.
                    let half_w = 0.8_f32 / self.scale.max(1e-6);
                    let snap_col = egui::Color32::from_rgb(120, 240, 255);
                    let sel_col  = egui::Color32::from_rgb(255, 200, 80);
                    for i in candidate_iter {
                        let e = &self.doc.dobjects[i];
                        if !e.style.visible || !self.doc.layers.renders(e.style.layer) {
                            skipped += 1;
                            continue;
                        }
                        let (emin, emax) = match &e.geom {
                            Geom::BlockRef(br) => self.resolved_blockref_bbox(br),
                            _ => e.bbox(),
                        };
                        if emax.x < v_min.x || emin.x > v_max.x
                        || emax.y < v_min.y || emin.y > v_max.y {
                            skipped += 1;
                            continue;
                        }
                        let in_selection = self.sel_mask.get(i).copied().unwrap_or(false);
                        let color = if self.selected == Some(i) || in_selection {
                            sel_col
                        } else if snap_source == Some(i) {
                            snap_col
                        } else {
                            let (r, g, b) = resolve_color(
                                e.style.color, e.style.layer,
                                &self.doc.layers, &self.doc.truecolors);
                            egui::Color32::from_rgb(r, g, b)
                        };
                        let packed: u32 =
                              ((color.r() as u32) << 24)
                            | ((color.g() as u32) << 16)
                            | ((color.b() as u32) <<  8)
                            |  (color.a() as u32);
                        let sc = self.scale.max(1e-6);
                        // Non-continuous linetype → emit DASHES (world polylines
                        // → dash walk → LINE pipeline), matching the CPU
                        // paint_dobject_with_style path. Covers every stroked
                        // geom (incl. arc/circle/ellipse via tessellation), so a
                        // dashed line no longer shows solid in GPU mode.
                        if let Some(pat) = self.effective_dash_pattern(e) {
                            let mut dashes: Vec<(Vec2, Vec2)> = Vec::new();
                            for pl in self.preview_world_polylines(&e.geom) {
                                dash_world_segments(&pl, &pat, &mut dashes);
                            }
                            for (a, b) in dashes {
                                gpu_push_seg(&mut lines, a, b, ox, oy, half_w, packed);
                            }
                            drawn += 1;
                            continue;
                        }
                        match &e.geom {
                            Geom::Circle(c) => {
                                circles.push(CircleInstance {
                                    x: (c.center.x + ox) as f32,
                                    y: (c.center.y + oy) as f32,
                                    r: c.radius as f32,
                                    color: packed,
                                });
                            }
                            Geom::Line(l) => {
                                gpu_push_seg(&mut lines, l.a, l.b, ox, oy, half_w, packed);
                            }
                            Geom::Point(pt) => {
                                let s = 4.0 / sc as f64;
                                gpu_push_seg(&mut lines,
                                    Vec2::new(pt.location.x - s, pt.location.y),
                                    Vec2::new(pt.location.x + s, pt.location.y),
                                    ox, oy, half_w, packed);
                                gpu_push_seg(&mut lines,
                                    Vec2::new(pt.location.x, pt.location.y - s),
                                    Vec2::new(pt.location.x, pt.location.y + s),
                                    ox, oy, half_w, packed);
                            }
                            Geom::Arc(a) => {
                                // Analytic arc SDF — no CPU tessellation.
                                // Canonicalise to CCW positive sweep so the
                                // shader's angular test is a single compare.
                                let (mut a0, mut sw) = (a.start_angle, a.sweep_angle);
                                if sw < 0.0 { a0 += sw; sw = -sw; }
                                let tau = std::f64::consts::TAU;
                                if sw > tau { sw = tau; }
                                arcs.push(ArcInstance {
                                    x: (a.center.x + ox) as f32,
                                    y: (a.center.y + oy) as f32,
                                    r: a.radius as f32,
                                    a0: a0 as f32,
                                    sweep: sw as f32,
                                    color: packed,
                                });
                            }
                            Geom::Ellipse(el) => {
                                // Analytic ellipse SDF — no CPU tessellation.
                                let a = el.major.len();
                                ellipses.push(EllipseInstance {
                                    x: (el.center.x + ox) as f32,
                                    y: (el.center.y + oy) as f32,
                                    a: a as f32,
                                    b: (a * el.ratio) as f32,
                                    rot: el.major.y.atan2(el.major.x) as f32,
                                    color: packed,
                                });
                            }
                            Geom::EllipseArc(ea) => {
                                let n = (((ea.ellipse.major.len() as f32) * sc) * 0.7)
                                    .clamp(12.0, 512.0) as usize;
                                let mut prev = ea.ellipse.point_at(ea.start_param);
                                for k in 1..=n {
                                    let p = ea.ellipse.point_at(
                                        ea.start_param + (k as f64 / n as f64) * ea.sweep_param);
                                    gpu_push_seg(&mut lines, prev, p, ox, oy, half_w, packed);
                                    prev = p;
                                }
                            }
                            Geom::Spline(s) => {
                                let pts = s.tessellate(64);
                                for w in pts.windows(2) {
                                    gpu_push_seg(&mut lines, w[0], w[1], ox, oy, half_w, packed);
                                }
                            }
                            Geom::Polyline(p)
                                if p.widths.is_empty()
                                && p.vertices.iter().all(|v| v.bulge.abs() < 1e-9) =>
                            {
                                // Straight, thin polyline (incl. rectangles).
                                for w in p.vertices.windows(2) {
                                    gpu_push_seg(&mut lines, w[0].pos, w[1].pos,
                                        ox, oy, half_w, packed);
                                }
                                if p.closed && p.vertices.len() >= 2 {
                                    let a = p.vertices[p.vertices.len() - 1].pos;
                                    let b = p.vertices[0].pos;
                                    gpu_push_seg(&mut lines, a, b, ox, oy, half_w, packed);
                                }
                            }
                            Geom::Wall(w) => {
                                // Full wall on GPU: poché fill → triangle soup,
                                // faces / insulation / centerline → line pipeline.
                                // Mirrors draw_dobject_thick's Wall arm; the
                                // dashed centerline is drawn SOLID here until the
                                // linetype-on-GPU phase lands.
                                let wstyle = self.doc.wall_styles.get(w.style);
                                let face_col = match wstyle {
                                    Some(s) if s.face_color != 0 => {
                                        let (r, g, b) = aci_palette(s.face_color.min(255) as u8);
                                        egui::Color32::from_rgb(r, g, b)
                                    }
                                    _ => color,
                                };
                                let face_packed = pack_rgba(face_col);
                                let fill_aci = wstyle.map(|s| s.fill_color).unwrap_or(0);
                                let (left_faces, right_faces) = wall_face_world_pts(self, w);
                                // Poché — only an un-broken single strip (matches CPU).
                                if fill_aci != 0
                                    && left_faces.len() == 1 && right_faces.len() == 1
                                    && left_faces[0].len() >= 2
                                    && left_faces[0].len() == right_faces[0].len()
                                {
                                    let (lp, rp) = (&left_faces[0], &right_faces[0]);
                                    let (fr, fg, fb) = aci_palette(fill_aci.min(255) as u8);
                                    let fc = pack_rgba(
                                        egui::Color32::from_rgba_unmultiplied(fr, fg, fb, 80));
                                    let mut push = |v: Vec2| fills.push(FillVertex {
                                        x: (v.x + ox) as f32, y: (v.y + oy) as f32, color: fc });
                                    for i in 0..lp.len() - 1 {
                                        // Quad (lp[i], rp[i], lp[i+1], rp[i+1]) → 2 tris.
                                        push(lp[i]);   push(rp[i]);   push(lp[i + 1]);
                                        push(rp[i]);   push(rp[i + 1]); push(lp[i + 1]);
                                    }
                                }
                                // Faces — every piece as connected segments.
                                for piece in left_faces.iter().chain(right_faces.iter()) {
                                    for s in piece.windows(2) {
                                        gpu_push_seg(&mut lines, s[0], s[1],
                                            ox, oy, half_w, face_packed);
                                    }
                                }
                                // Batt-insulation sine wave.
                                if wstyle.map(|s| s.insulation).unwrap_or(false) {
                                    let wave = wall_insulation_wave(w);
                                    for s in wave.windows(2) {
                                        gpu_push_seg(&mut lines, s[0], s[1],
                                            ox, oy, half_w, face_packed);
                                    }
                                }
                                // Centerline — in the wall style's linetype (app map) when
                                // set (dashed via the GPU dash-walker); else solid.
                                let cl_ltype = self.wall_centerline_ltype.get(&w.style).copied();
                                if cl_ltype.is_some() || self.env.WlCnL {
                                    let n_face = left_faces.iter().chain(right_faces.iter())
                                        .map(|p| p.len()).max().unwrap_or(2);
                                    let clp = pack_rgba(egui::Color32::from_rgba_unmultiplied(
                                        color.r(), color.g(), color.b(), 110));
                                    let cl = w.centerline_polyline(n_face.max(3) - 1);
                                    let pattern = cl_ltype
                                        .and_then(|id| self.doc.linetypes.get(id))
                                        .filter(|lt| !lt.is_continuous())
                                        .map(|lt| lt.pattern.clone());
                                    match pattern {
                                        Some(pat) => {
                                            let mut dashes = Vec::new();
                                            dash_world_segments(&cl, &pat, &mut dashes);
                                            for (a, b) in dashes {
                                                gpu_push_seg(&mut lines, a, b, ox, oy, half_w, clp);
                                            }
                                        }
                                        _ => {
                                            for s in cl.windows(2) {
                                                gpu_push_seg(&mut lines, s[0], s[1], ox, oy, half_w, clp);
                                            }
                                        }
                                    }
                                }
                            }
                            // Not yet ported → CPU painter (always correct).
                            _ => {
                                if let Geom::Hatch(_) = &e.geom {
                                    // Read pre-built (cached) geometry. Pattern
                                    // → line/circle pipelines; solid → fill
                                    // triangles (even loops = `packed`, odd
                                    // loops = bg over-draw for even-odd holes).
                                    if let Some(entry) = self.hatch_cache.get(&e.handle) {
                                        for (a, b) in &entry.segs {
                                            gpu_push_seg(&mut lines, *a, *b,
                                                ox, oy, half_w, packed);
                                        }
                                        for (centre, r_world) in &entry.circs {
                                            if (*r_world as f32) * sc < 0.5 { continue; }
                                            circles.push(CircleInstance {
                                                x: (centre.x + ox) as f32,
                                                y: (centre.y + oy) as f32,
                                                r: *r_world as f32,
                                                color: packed,
                                            });
                                        }
                                        if !entry.solid.is_empty() {
                                            let bgp = pack_rgba(
                                                egui::Color32::from_rgb(18, 22, 28));
                                            // Depth-ascending batches pushed in
                                            // buffer order so the rasterizer
                                            // draws nested islands (even depth)
                                            // after the hole (odd depth) that
                                            // contains them.
                                            for (is_fill, tris) in &entry.solid {
                                                let col = if *is_fill { packed } else { bgp };
                                                for tri in tris {
                                                    for v in tri {
                                                        fills.push(FillVertex {
                                                            x: (v.x + ox) as f32,
                                                            y: (v.y + oy) as f32,
                                                            color: col });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    draw_dobject(&painter, rect, self, &e.geom, color);
                                }
                            }
                        }
                        drawn += 1;
                    }
                    gpu_circles_count = circles.len() + arcs.len() + ellipses.len();
                    gpu_line_count = lines.len();
                    gpu_fill_count = fills.len() / 3;
                    if !circles.is_empty() || !arcs.is_empty() || !ellipses.is_empty()
                        || !lines.is_empty() || !fills.is_empty() {
                        // Camera-relative: offset folded into the instances, so
                        // the view matrix uses offset 0.
                        let view = view_matrix(
                            rect.width(), rect.height(),
                            self.scale, 0.0, 0.0,
                        );
                        let renderer = self.gpu_renderer.clone();
                        let gpu_pad = 2.0 / self.scale.max(1e-6) as f32;
                        let cb = egui::PaintCallback {
                            rect,
                            callback: StdArc::new(
                                egui_glow::CallbackFn::new(
                                    move |_info, gl_painter| {
                                        let gl = gl_painter.gl();
                                        let mut r = renderer.lock().unwrap();
                                        r.ensure_init(gl);
                                        r.render(gl, &fills, &circles, &arcs, &ellipses, &lines, &view, gpu_pad);
                                    },
                                ),
                            ),
                        };
                        painter.add(egui::Shape::Callback(cb));
                    }
                    self.gpu_dirty = false;
                }
                RenderMode::Apx => {}   // handled by the APX branch above (keeps the match exhaustive)
            } } // close `match self.render_mode` and outer `else` from APX branch
            // WP-SCRIPT slice 5 — script preview ghosts: the net additions of
            // the last ghost pass, dashed + translucent over the real drawing
            // (best-effort: hatches are skipped — their fills need the real
            // doc's boundary resolution).
            if let Some(p) = &self.script_preview {
                if !p.ghosts.is_empty() {
                    let ghost = egui::Color32::from_rgba_unmultiplied(90, 220, 255, 170);
                    for g in &p.ghosts {
                        if matches!(g, Geom::Hatch(_)) {
                            continue;
                        }
                        draw_dobject_dashed(&painter, rect, self, g, ghost, 7.0, 4.0);
                    }
                }
            }
            // Parametric DOF overlay: tint under-defined geometry blue and
            // fully-defined geometry black-ish, SolidWorks style. Reads the
            // per-handle map cached by render_param_panel earlier this frame.
            if self.parametric.active && self.parametric.show_dof {
                draw_param_overlay(&painter, rect, self);
            }

            // Insert preview — the translucent "shade" of the block being placed.
            self.paint_insert_preview(&painter, rect);
            // Live parametric insert — the block deforming under the cursor.
            self.paint_insert_live(&painter, rect);

            // Insert-dialog "↔" parameter pick: rubber-band + live distance
            // from the first clicked point to the cursor.
            if let Some((_, Some(p1))) = self.insert_param_pick {
                if let Some(cur) = painter.ctx().input(|i| i.pointer.hover_pos()) {
                    let a = self.w2s(p1, rect);
                    let d = (self.s2w(cur, rect) - p1).len();
                    let col = egui::Color32::from_rgb(150, 200, 255);
                    painter.line_segment([a, cur], egui::Stroke::new(1.5, col));
                    painter.text(cur + egui::vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM,
                        format!("{d:.3}"), egui::FontId::proportional(12.0), col);
                }
            }

            // Grip handles on the selected dobject (drawn on top of the
            // geometry, under the snap marker / rubber band).
            if self.env.GrpEnb {
                if let Some(i) = self.selected {
                    if let Some(e) = self.doc.dobjects.get(i) {
                        draw_grips(&painter, rect, self, &e.geom);
                    }
                }
            }

            // Block-diff parametric-point highlight — drawn on every
            // instance of the two compared blocks, colour-coded per
            // candidate parameter, with a direction arrow per cluster.
            if let Some(ov) = &self.blockdiff_overlay {
                // Distinct per-parameter colours (P1, P2, …).
                const PAL: [egui::Color32; 6] = [
                    egui::Color32::from_rgb(255, 90, 90),    // P1 red
                    egui::Color32::from_rgb(90, 200, 255),   // P2 cyan
                    egui::Color32::from_rgb(160, 255, 120),  // P3 green
                    egui::Color32::from_rgb(255, 200, 70),   // P4 amber
                    egui::Color32::from_rgb(220, 140, 255),  // P5 violet
                    egui::Color32::from_rgb(255, 150, 90),   // P6 orange
                ];
                for d in &self.doc.dobjects {
                    let Geom::BlockRef(br) = &d.geom else { continue };
                    // BASE block only — the moved points are base-relative to
                    // block A; drawing them on B's instance floats them in
                    // empty space (B's geometry is the moved result).
                    if br.block != ov.block_a { continue; }
                    let to_world = |rel: Vec2| {
                        let s = rel * br.scale;
                        let (c, sn) = (br.rotation.cos(), br.rotation.sin());
                        br.insert + Vec2::new(s.x * c - s.y * sn, s.x * sn + s.y * c)
                    };
                    for (ci, (pts, dir)) in ov.clusters.iter().enumerate() {
                        let col = PAL[ci % PAL.len()];
                        // Cluster centroid (world) for the direction arrow.
                        let mut cw = Vec2::ZERO;
                        for p in pts {
                            let w = to_world(*p);
                            cw = cw + w;
                            let sp = self.w2s(w, rect);
                            painter.circle_filled(sp, 4.0, col);
                            painter.circle_stroke(sp, 4.0,
                                egui::Stroke::new(1.0, egui::Color32::BLACK));
                        }
                        if !pts.is_empty() {
                            cw = cw / (pts.len() as f64);
                            // Rotate the (base-relative) direction by the
                            // instance rotation so the arrow points right.
                            let (c, sn) = (br.rotation.cos(), br.rotation.sin());
                            let wd = Vec2::new(dir.x * c - dir.y * sn,
                                               dir.x * sn + dir.y * c);
                            let a0 = self.w2s(cw, rect);
                            let tip = a0 + egui::vec2(wd.x as f32, -wd.y as f32) * 26.0;
                            painter.line_segment([a0, tip], egui::Stroke::new(2.0, col));
                            // simple arrowhead
                            let back = egui::vec2(wd.x as f32, -wd.y as f32);
                            let perp = egui::vec2(-back.y, back.x);
                            painter.line_segment(
                                [tip, tip - back * 7.0 + perp * 4.0],
                                egui::Stroke::new(2.0, col));
                            painter.line_segment(
                                [tip, tip - back * 7.0 - perp * 4.0],
                                egui::Stroke::new(2.0, col));
                            painter.text(tip + egui::vec2(3.0, -3.0),
                                egui::Align2::LEFT_BOTTOM,
                                format!("P{}", ci + 1),
                                egui::FontId::proportional(12.0), col);
                        }
                    }
                }
            }

            // UCS indicator (origin marker) — anchors at world (0,0)
            // when on-screen, else pinned to the bottom-left corner.
            // Toggle: env.UcsIcn (SYSVAR). Renders OVER everything
            // (after dobjects + grips) so it stays visible against
            // any color underneath.
            if self.env.UcsIcn {
                draw_ucs_icon(&painter, rect, self);
            }

            // HUD: FPS + drawn/skipped/total + index status + render mode.
            let idx_state = if self.index.is_some() && !self.index_dirty { "idx ✓" }
                            else { "idx stale" };
            let mode_str = match self.render_mode {
                RenderMode::Apx => format!("APX ({} dots instanced)", gpu_circles_count),
                RenderMode::Cpu => format!("CPU"),
                RenderMode::Gpu => format!(
                    "GPU ({} circ/arc + {} line-seg + {} fill-tri instanced)",
                    gpu_circles_count, gpu_line_count, gpu_fill_count),
            };
            painter.text(
                rect.right_top() + egui::vec2(-8.0, 8.0),
                egui::Align2::RIGHT_TOP,
                format!("FPS {:>5.1}    drawn {}  sub-px-skip {}  /{}    {}    {}",
                    self.fps_smooth, drawn, skipped, self.doc.dobjects.len(),
                    idx_state, mode_str),
                crate::theme::typ::data_code(),
                egui::Color32::from_rgb(200, 220, 240),
            );
            // ---- Left-corner perf NOTICE (advisory only — NO forced switch).
            // A small pill in the TOP-LEFT. The render mode is never yanked;
            // this just tells the user what's happening and what they can do.
            // Priority: still building hatch cache → APX-overwhelmed → CPU
            // capped → low FPS.
            let notice: Option<(String, egui::Color32)> =
                if hatch_building > 0 {
                    Some((format!("⏳ building hatch cache… {} left", hatch_building),
                          egui::Color32::from_rgb(120, 240, 255)))
                } else if self.render_mode == RenderMode::Apx && self.fps_smooth < 15.0 {
                    Some((format!("⚠ {:.0} FPS — too heavy even for APX; reduce dobjects",
                                  self.fps_smooth),
                          egui::Color32::from_rgb(255, 90, 90)))
                } else if cpu_capped {
                    Some((format!("⚠ CPU showing {}/{} — press GPU or APX for all",
                                  CPU_DRAW_BUDGET, in_viewport),
                          egui::Color32::from_rgb(255, 200, 80)))
                } else if self.render_mode != RenderMode::Apx && self.fps_smooth < 15.0 {
                    Some((format!("⚠ {:.0} FPS — press APX for speed", self.fps_smooth),
                          egui::Color32::from_rgb(255, 200, 80)))
                } else {
                    None
                };
            // ---- SLOW-FRAME report -------------------------------------
            // Only when the frame blew one refresh (16.7 ms @ 60 Hz) — i.e. only when
            // the stall is VISIBLE. Costs nothing on a healthy frame and cannot flood
            // the dump. `candidates` is the column that matters: zoomed out at 1.5M it
            // is EVERY dobject, and the draw loop then walks all of them.
            {
                let total_us = t_frame.elapsed().as_micros() as u64;
                if total_us >= 16_700 {
                    let draw_us = t_draw.elapsed().as_micros() as u64;
                    crate::dbg_event!(self, crate::dbg_recorder::DbgEvent::SlowFrame {
                        total_us,
                        query_us,
                        draw_us,
                        candidates: in_viewport,
                        drawn,
                        capped: cpu_capped,
                    });
                }
            }
            if let Some((msg, col)) = notice {
                let font = egui::FontId::monospace(12.0);
                let ink  = egui::Color32::from_rgb(20, 20, 24);
                let pos  = rect.left_top() + egui::vec2(8.0, 8.0);
                let galley = painter.layout_no_wrap(msg.clone(), font.clone(), ink);
                let pad = egui::vec2(8.0, 5.0);
                let bg = egui::Rect::from_min_size(pos, galley.size() + pad * 2.0);
                painter.rect_filled(bg, 4.0, col);
                painter.text(pos + pad, egui::Align2::LEFT_TOP, msg, font, ink);
            }
            // Snapshot for the Screen Stats panel — covers both CPU
            // and GPU render paths (they share `drawn`/`skipped`).
            self.last_render_stats = RenderStats {
                total:           self.doc.dobjects.len(),
                in_viewport,
                drawn,
                skipped_hidden:  skipped,    // combined hidden+subpx for now
                skipped_subpx:   0,          // (split is a future refinement)
                frame_dt:        dt,
                index_label:     if self.index_label.is_empty() {
                    idx_state.to_string()
                } else { self.index_label.clone() },
            };
            if !self.index_label.is_empty() {
                painter.text(
                    rect.right_top() + egui::vec2(-8.0, 24.0),
                    egui::Align2::RIGHT_TOP,
                    &self.index_label,
                    egui::FontId::monospace(10.0),
                    egui::Color32::from_rgb(140, 160, 180),
                );
            }

            // ∩ click preview — show the 50-pixel search circle on the cursor.
            if self.intersect_pending_click {
                if let Some(cur) = resp.hover_pos() {
                    painter.circle_stroke(
                        cur, 50.0,
                        egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 220, 100)),
                    );
                    painter.text(
                        cur + egui::vec2(0.0, 60.0),
                        egui::Align2::CENTER_CENTER,
                        "click to ∩ here (Esc cancels)",
                        crate::theme::typ::data_code(),
                        egui::Color32::from_rgb(255, 220, 100),
                    );
                }
            }

            // intersection markers on top
            for p in &self.intersections {
                let sp = self.w2s(*p, rect);
                painter.circle_filled(sp, 4.5, egui::Color32::from_rgb(255, 90, 90));
                painter.circle_stroke(
                    sp, 4.5,
                    egui::Stroke::new(1.0, egui::Color32::WHITE),
                );
            }


            // pick-mode hover preview: highlight the dobject that would be selected
            if self.picking_source {
                if let Some(cur) = resp.hover_pos() {
                    let world = self.s2w(cur, rect);
                    let tol_world = 10.0 / self.scale as f64;
                    let mut best: Option<(usize, f64)> = None;
                    for (i, e) in self.doc.dobjects.iter().enumerate() {
                        let d = e.distance_to_point(world);
                        if d < tol_world && best.map_or(true, |(_, bd)| d < bd) {
                            best = Some((i, d));
                        }
                    }
                    if let Some((i, _)) = best {
                        draw_dobject(&painter, rect, self, &self.doc.dobjects[i].geom,
                                    egui::Color32::from_rgb(120, 240, 255));
                    }
                }
            }

            // OSNAP marker: glyph at the snap point + the dashed extension
            // line/arc from the on-dobject anchor when the foot lies on the
            // imaginary extension (PER/TAN past a segment endpoint or a
            // swept-arc boundary).
            if let Some(h) = snap_hit {
                let sp = self.w2s(h.point, rect);
                let glyph_col = egui::Color32::from_rgb(80, 230, 240);
                let from_anchor = self.pending.last().copied();

                // Faint connector from the cursor to the snap point — only
                // drawn when they're visibly apart. PER/TAN can land their
                // foot far from where the user is hovering (especially on
                // extensions); this thin line removes the "where IS my snap?"
                // confusion without making close hovers visually noisy.
                if let Some(cur) = resp.hover_pos() {
                    let gap_px = (cur - sp).length();
                    if gap_px > 20.0 {
                        painter.line_segment(
                            [cur, sp],
                            egui::Stroke::new(0.8, glyph_col.gamma_multiply(0.30)),
                        );
                    }
                }

                // The dashed indicator for the "imaginary extension":
                //   - on a line dobject, the extension is the infinite line, so
                //     a straight dashed segment between anchor and foot is right.
                //   - on an arc dobject, the extension is the rest of the
                //     underlying circle, so the dashes should curve along that
                //     circle from the arc endpoint (anchor) to the foot.
                //   - a circle dobject has no extension (PER's two feet are
                //     always on the circle), so this branch never fires for it.
                if let Some(anchor) = h.extension_anchor {
                    let time = ctx.input(|i| i.time) as f32;
                    let phase = time * 60.0;
                    let a = (0.55 + 0.35 * (time * 4.0).sin()).clamp(0.25, 0.95);
                    let ext_col = egui::Color32::from_rgba_unmultiplied(
                        255, 200, 90, (a * 255.0) as u8);
                    let ext_stroke = egui::Stroke::new(1.2, ext_col);
                    let geom_ref = h.dobject.and_then(|i| self.doc.dobjects.get(i)).map(|d| &d.geom);
                    match geom_ref {
                        Some(Geom::Arc(arc)) => {
                            // Walk the shorter way around the underlying circle
                            // from anchor angle to foot angle.
                            let ca = (anchor  - arc.center).angle();
                            let cf = (h.point - arc.center).angle();
                            let raw = (cf - ca).rem_euclid(std::f64::consts::TAU);
                            let sweep = if raw > std::f64::consts::PI {
                                raw - std::f64::consts::TAU
                            } else {
                                raw
                            };
                            draw_dashed_arc(
                                &painter, rect, self,
                                arc.center, arc.radius, ca, sweep,
                                7.0, 4.0, phase, ext_stroke,
                            );
                        }
                        _ => {
                            draw_dashed_line(
                                &painter,
                                self.w2s(anchor, rect), sp,
                                7.0, 4.0, phase, ext_stroke,
                            );
                        }
                    }
                }

                // Connector hint from the user's last pending click to the
                // snap point — only meaningful for snaps that "do something
                // with the anchor" (PER/TAN); for END/MID/CEN it's just a
                // soft guide.
                if let Some(from) = from_anchor {
                    painter.line_segment(
                        [self.w2s(from, rect), sp],
                        egui::Stroke::new(1.0, glyph_col.gamma_multiply(0.35)),
                    );
                }

                draw_snap_glyph(&painter, sp, h.kind, glyph_col);
                let label = if snap_candidates.len() > 1 {
                    format!("{}  ⇥ {}/{}",
                        h.kind.name(),
                        self.snap_cycle_index + 1,
                        snap_candidates.len())
                } else {
                    h.kind.name().to_string()
                };
                painter.text(
                    sp + egui::vec2(12.0, -12.0),
                    egui::Align2::LEFT_BOTTOM,
                    label,
                    crate::theme::typ::data_code(),
                    glyph_col,
                );
                if snap_candidates.len() > 1 {
                    painter.text(
                        sp + egui::vec2(12.0, 12.0),
                        egui::Align2::LEFT_TOP,
                        "Tab: next snap",
                        egui::FontId::monospace(10.0),
                        glyph_col.gamma_multiply(0.7),
                    );
                }
            }

            // ---- Grip handles (Issue 3) ----------------------------------
            // Render small filled squares at each grip point of every
            // selected dobject when in pointer mode + GrpEnb is on. v1
            // semantic: dragging any grip translates the whole dobject.
            // A COUNT CAP, THE WAY AutoCAD HAS ONE. `GRIPOBJLIMIT` defaults to 100 there and does
            // exactly this: past the limit, grips stop being drawn for the selection.
            //
            // Everything below is per-selected-object per-frame — a clone, a sort, a dedup, then
            // a loop that reads each dobject and paints a square at every grip point. Selecting a
            // few thousand objects, which one crossing window does, turned that into 25-80 ms
            // EVERY FRAME. There is no viewport cull anywhere in this block, and adding one is
            // harder than adding a count test: grips are screen-space squares, so culling means
            // projecting each grip point rather than rejecting a bbox.
            //
            // It is also the honest behaviour. Grips exist so you can drag ONE thing; nobody
            // reaches for a grip on the 900th object of a selection, and a canvas solid with
            // handles is less usable, not more.
            let grips_shown = self.selection.len() <= GRIP_OBJ_LIMIT;
            if self.env.GrpEnb && grips_shown && !in_click_only_phase
                && self.select_mode == SelectMode::Off
            {
                let mut grip_targets: Vec<usize> = self.selection.clone();
                if let Some(s) = self.selected { grip_targets.push(s); }
                grip_targets.sort_unstable(); grip_targets.dedup();
                // Render preview translation if dragging.
                let drag_delta: Option<Vec2> = self.grip_drag.and_then(|gd| {
                    let cur = resp.hover_pos()?;
                    let w = self.cursor_world_constrained(Some(cur), rect, snap_hit.map(|h| h.point))
                        .unwrap_or_else(|| self.s2w(cur, rect));
                    Some(w - gd.grip_origin)
                });
                let gsz = self.env.GrpSz as f32;
                let (cu_r, cu_g, cu_b) = (
                    (self.env.GrClrU >> 16 & 0xFF) as u8,
                    (self.env.GrClrU >>  8 & 0xFF) as u8,
                    (self.env.GrClrU       & 0xFF) as u8,
                );
                let (cs_r, cs_g, cs_b) = (
                    (self.env.GrClrS >> 16 & 0xFF) as u8,
                    (self.env.GrClrS >>  8 & 0xFF) as u8,
                    (self.env.GrClrS       & 0xFF) as u8,
                );
                let grip_col_u = egui::Color32::from_rgb(cu_r, cu_g, cu_b);
                let grip_col_s = egui::Color32::from_rgb(cs_r, cs_g, cs_b);
                for idx in &grip_targets {
                    let Some(d) = self.doc.dobjects.get(*idx) else { continue; };
                    // If this dobject is the active drag target, preview
                    // the role-specific edit at the cursor position.
                    let preview_geom = if let Some(gd) = self.grip_drag {
                        if gd.dobject_idx == *idx {
                            if let Some(cur) = resp.hover_pos() {
                                let w = self.cursor_world_constrained(
                                    Some(cur), rect, snap_hit.map(|h| h.point))
                                    .unwrap_or_else(|| self.s2w(cur, rect));
                                Some(d.geom.with_grip_moved(gd.role, w))
                            } else { None }
                        } else { None }
                    } else { None };
                    let _ = drag_delta;  // legacy; preview now uses with_grip_moved
                    let geom_ref = preview_geom.as_ref().unwrap_or(&d.geom);
                    // Ghost the dobject in dim white during the drag.
                    if preview_geom.is_some() {
                        draw_dobject(&painter, rect, self, geom_ref,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 160));
                    }
                    // Hover-highlight: any grip within GrpHvR px of the
                    // cursor lights up so the user knows clicking will
                    // grab it. Same threshold drives the grab-on-click
                    // tolerance below — no risk of "looks highlighted
                    // but doesn't grab". Skipped while a drag is in
                    // progress (the dragged grip already glows).
                    let cursor_screen = resp.hover_pos();
                    let hover_r2_px = (self.env.GrpHvR as f32).powi(2);
                    for (gp, _role) in geom_ref.grip_points() {
                        let sp = self.w2s(gp, rect);
                        let active_drag = self.grip_drag
                            .map(|gd| gd.dobject_idx == *idx
                                 && gd.grip_origin.dist(gp) < 1e-6)
                            .unwrap_or(false);
                        let hover = !active_drag
                            && self.grip_drag.is_none()
                            && cursor_screen.map(|c| {
                                let d = sp - c;
                                d.x*d.x + d.y*d.y <= hover_r2_px
                            }).unwrap_or(false);
                        let hot = active_drag || hover;
                        // Slightly larger square when hot so it reads
                        // as a "ready to grab" affordance, not just a
                        // color swap.
                        let half = if hot { gsz + 2.0 } else { gsz };
                        let r = egui::Rect::from_center_size(
                            sp, egui::vec2(half * 2.0, half * 2.0));
                        let col = if hot { grip_col_s } else { grip_col_u };
                        painter.rect_filled(r, 0.0, col);
                        // Pale outline ring around a hovered grip — extra
                        // visual cue the user can spot from the corner
                        // of their eye.
                        if hover {
                            painter.circle_stroke(sp, (gsz + 4.0).max(8.0),
                                egui::Stroke::new(1.2, egui::Color32::from_rgba_unmultiplied(
                                    255, 255, 255, 200)));
                        }
                        painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0,
                            egui::Color32::from_rgb(20, 20, 20)));
                    }
                }
            }

            // Live rubber-band preview while a window-drag is in progress.
            // Mirrors the classifier triggers: select-mode active, OR
            // pointer-mode-idle (always-on selector), OR Shift held in
            // a non-edit phase. L→R draws BLUE (window — fully-inside);
            // R→L draws GREEN (crossing — anything touching).
            //
            // Time-gated to match the classifier: no preview until the
            // user has held the button past env.SelDmTm. Without this
            // the visual would lie — preview appears, but the gesture
            // gets discarded as a click on release.
            let preview_pointer_mode_idle = !in_click_only_phase && !in_select;
            if resp.dragged()
                && !rt_zoom    // a real-time-zoom drag is not a selection marquee
                && ((in_select && hold_threshold_passed)
                    || (preview_pointer_mode_idle && hold_threshold_passed)
                    || (shift_held && !in_click_only_phase))
            {
                // Read press from our stashed snapshot so the preview
                // stays correct even on the release frame (egui clears
                // press_origin() by drag_stopped()). press_pos_this_frame
                // is the screen+world position of the active press.
                if let (Some((p, _)), Some(c)) = (
                    press_pos_this_frame,
                    resp.hover_pos(),
                ) {
                    let r = egui::Rect::from_two_pos(p, c);
                    let crossing = c.x < p.x;
                    let (fill, stroke) = if crossing {
                        (egui::Color32::from_rgba_unmultiplied(140, 220, 100, 28),
                         egui::Color32::from_rgb(140, 220, 100))
                    } else {
                        (egui::Color32::from_rgba_unmultiplied(120, 170, 255, 28),
                         egui::Color32::from_rgb(120, 170, 255))
                    };
                    painter.rect_filled(r, 0.0, fill);
                    painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0, stroke));
                }
            }

            // ---- Dist ghost line ------------------------------------------
            // After the first click, render a live line from P1 to the
            // cursor plus a running readout of distance / Δx / Δy /
            // angle so the user can preview the measurement before
            // clicking P2.
            if let DistState::WaitingForP2(p1) = self.dist_state {
                if let Some(cur_s) = ctx.input(|i| i.pointer.hover_pos()) {
                    let p1_s = self.w2s(p1, rect);
                    let cur_w = self.s2w(cur_s, rect);
                    // Warm-amber dashed line — same family as offset's
                    // ghost so the user reads it as "preview".
                    let base = egui::Color32::from_rgb(255, 200, 100);
                    let col = egui::Color32::from_rgba_unmultiplied(
                        base.r(), base.g(), base.b(), pulse_alpha);
                    for s in egui::Shape::dashed_line(
                        &[p1_s, cur_s],
                        egui::Stroke::new(1.6, col), 6.0, 4.0)
                    {
                        painter.add(s);
                    }
                    // Small ✕ marks at both endpoints.
                    for p in [p1_s, cur_s] {
                        let pen = egui::Stroke::new(1.2, base);
                        painter.line_segment(
                            [p + egui::vec2(-6.0, -6.0),
                             p + egui::vec2( 6.0,  6.0)], pen);
                        painter.line_segment(
                            [p + egui::vec2(-6.0,  6.0),
                             p + egui::vec2( 6.0, -6.0)], pen);
                    }
                    // Running readout near the cursor.
                    let dx = cur_w.x - p1.x;
                    let dy = cur_w.y - p1.y;
                    let d  = (dx * dx + dy * dy).sqrt();
                    let ang = dy.atan2(dx).to_degrees();
                    painter.text(
                        cur_s + egui::vec2(12.0, 12.0),
                        egui::Align2::LEFT_TOP,
                        format!(
                            "d={:.3}  Δ=({:+.3},{:+.3})  ∠{:+.2}°",
                            d, dx, dy, ang),
                        crate::theme::typ::data_code(),
                        egui::Color32::from_rgb(255, 220, 140));
                }
            }

            // ---- ZOOM Window preview --------------------------------------
            // Once the first corner is captured (WinSecond), draw a live
            // rectangle from that corner to the cursor so the user sees the
            // area that will fill the viewport before picking corner 2. Amber,
            // to read differently from the green/blue selection marquee above.
            if let ZoomState::WinSecond(a) = self.zoom_state {
                if let Some(cur_s) = resp.hover_pos() {
                    let r = egui::Rect::from_two_pos(self.w2s(a, rect), cur_s);
                    painter.rect_filled(r, 0.0,
                        egui::Color32::from_rgba_unmultiplied(255, 200, 100, 26));
                    painter.rect_stroke(r, 0.0,
                        egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 200, 100)));
                    painter.text(
                        cur_s + egui::vec2(12.0, 12.0),
                        egui::Align2::LEFT_TOP,
                        "zoom window",
                        crate::theme::typ::data_code(),
                        egui::Color32::from_rgb(255, 220, 140));
                    // Keep the preview tracking the cursor smoothly.
                    ctx.request_repaint();
                }
            }

            // ---- Text live preview --------------------------------
            // While the text tool is WaitingForString, render whatever
            // the user is typing AT the anchor in the actual style +
            // height — so they see the final size/font BEFORE
            // committing. Source of truth:
            //   1. text_input_dialog_open  → text_input_dialog_buf
            //                                + dialog height + dialog
            //                                style's font
            //   2. else (cmd-line path)    → self.cmd  + env.TxHt
            //                                + STANDARD font
            if let TextDraftState::WaitingForString(pos) = &self.text_draft {
                let (body, height, font_name) = if self.text_input_dialog_open {
                    let f = self.doc.text_styles
                        .get(self.text_input_dialog_style_id)
                        .map(|s| s.font_name.clone())
                        .unwrap_or_else(|| "standard".into());
                    (self.text_input_dialog_buf.clone(),
                     self.text_input_dialog_height,
                     f)
                } else {
                    (self.cmd.trim_end_matches('\n').to_string(),
                     self.env.TxHt,
                     "standard".into())
                };
                let size_px = (height * self.scale as f64) as f32;
                if size_px >= 4.0 {
                    let anchor = self.w2s(*pos, rect);
                    // Half-alpha pulse so it reads as a preview, not a
                    // committed dobject.
                    let preview_col = egui::Color32::from_rgba_unmultiplied(
                        200, 230, 255, pulse_alpha);
                    if !body.is_empty() {
                        let font_id = font_id_for_font_name(&font_name, size_px);
                        painter.text(
                            anchor,
                            egui::Align2::LEFT_BOTTOM,
                            &body,
                            font_id,
                            preview_col,
                        );
                    }
                    // Caret marker — always drawn so the user sees the
                    // anchor location even with an empty buffer.
                    let caret_col = egui::Color32::from_rgba_unmultiplied(
                        255, 220, 140, pulse_alpha);
                    painter.line_segment(
                        [anchor + egui::vec2(0.0, -size_px),
                         anchor + egui::vec2(0.0,  2.0)],
                        egui::Stroke::new(1.5, caret_col));
                }
            }

            // ---- Dim live ghost preview ------------------------------------
            // During WaitingForDimLinePos, build a ghost Dim with the
            // cursor as the dimline_pos / leader_end and render it via
            // the real draw_dimension fn. Half-alpha so the user reads
            // it as "preview, not committed yet". Also draws a small
            // marker between p1 and p2 during WaitingForP2 so the
            // chord direction is visible while picking.
            if matches!(self.tool, Tool::Dim) {
                use cad_kernel::{Dim, DimKind};
                if let Some(cur_s) = ctx.input(|i| i.pointer.hover_pos()) {
                    let cur_w = self.s2w(cur_s, rect);
                    match &self.dim_draft {
                        DimDraftState::WaitingForP2 { p1 } => {
                            let stroke = egui::Stroke::new(1.0,
                                egui::Color32::from_rgba_unmultiplied(
                                    200, 230, 255, pulse_alpha));
                            painter.line_segment(
                                [self.w2s(*p1, rect), cur_s], stroke);
                        }
                        DimDraftState::WaitingForDimLinePos { kind } => {
                            let preview_kind = match kind {
                                DimDraftKind::Linear { p1, p2, ortho } => DimKind::Linear {
                                    p1: *p1, p2: *p2, dimline_pos: cur_w, ortho: *ortho,
                                },
                                DimDraftKind::Radius { center, on_circle } => DimKind::Radius {
                                    center: *center, on_circle: *on_circle, leader_end: cur_w,
                                },
                                DimDraftKind::Diameter { center, on_circle } => DimKind::Diameter {
                                    center: *center, on_circle: *on_circle, leader_end: cur_w,
                                },
                            };
                            let ghost = Dim {
                                kind: preview_kind,
                                style: self.current_dim_style,
                                text_override: None,
                            };
                            let preview_col = egui::Color32::from_rgba_unmultiplied(
                                200, 230, 255, pulse_alpha);
                            draw_dimension(&painter, rect, self, &ghost, preview_col);
                        }
                        _ => {}
                    }
                }
            }

            // ---- Offset live ghost preview --------------------------------
            // While WaitingForSide, compute the would-be offset for the
            // current cursor position and ghost-render it so the user
            // sees where the new dobject will land before clicking.
            // Distance mode: cursor is the side hint, distance is fixed
            //   (env.OfsDis). Result distance = env.OfsDis, side toward
            //   the cursor.
            // Through mode: cursor IS the through-point, distance is
            //   computed from cursor to source.
            if let OffsetState::WaitingForSide(mode, src_idx) = self.offset_state {
                if let Some(src) = self.doc.dobjects.get(src_idx) {
                    let cursor_w = ctx.input(|i| i.pointer.hover_pos())
                        .map(|p| self.s2w(p, rect));
                    if let Some(cur) = cursor_w {
                        let (dist, side_hint, ok) = match mode {
                            OffsetMode::Distance(d) => (d, cur, true),
                            OffsetMode::Through => {
                                let d = distance_world_to_geom(&src.geom, cur);
                                (d, cur, d.is_finite() && d.abs() > 1e-9)
                            }
                        };
                        if ok {
                            if let Ok(ghost_geom) = src.geom.offset(dist, side_hint) {
                                // Warm-amber ghost — distinct from the
                                // gray-cyan basket dashes so it reads as
                                // "preview, not committed". Pulses with
                                // the same alpha as everything else.
                                let base = egui::Color32::from_rgb(255, 200, 100);
                                let ghost_col = egui::Color32::from_rgba_unmultiplied(
                                    base.r(), base.g(), base.b(), pulse_alpha);
                                draw_dobject_dashed(&painter, rect, self,
                                    &ghost_geom, ghost_col, 8.0, 5.0);
                                // Through-mode also draws a small x at
                                // the cursor so the through-point is
                                // visually located.
                                if matches!(mode, OffsetMode::Through) {
                                    let c_s = self.w2s(cur, rect);
                                    let pen = egui::Stroke::new(1.2, base);
                                    painter.line_segment(
                                        [c_s + egui::vec2(-6.0, -6.0),
                                         c_s + egui::vec2( 6.0,  6.0)], pen);
                                    painter.line_segment(
                                        [c_s + egui::vec2(-6.0,  6.0),
                                         c_s + egui::vec2( 6.0, -6.0)], pen);
                                }
                            }
                        }
                    }
                }
            }

            // ---- Rotate live preview --------------------------------------
            // WaitingForAngle: ghost-render the selection rotated to the
            // current cursor angle (atan2(cursor − pivot)) so the user
            // sees the rotation form up before clicking. Also draws the
            // pivot mark + a baseline from pivot to cursor.
            // Reference sub-states: just show the points captured so far.
            match self.rotate_state {
                RotateState::WaitingForAngle(pivot) => {
                    let pivot_s = self.w2s(pivot, rect);
                    let mark    = egui::Color32::from_rgb(255, 200, 80);
                    painter.circle_stroke(pivot_s, 5.0, egui::Stroke::new(1.4, mark));
                    painter.line_segment(
                        [pivot_s + egui::vec2(-9.0, 0.0), pivot_s + egui::vec2(9.0, 0.0)],
                        egui::Stroke::new(0.8, mark));
                    painter.line_segment(
                        [pivot_s + egui::vec2(0.0, -9.0), pivot_s + egui::vec2(0.0, 9.0)],
                        egui::Stroke::new(0.8, mark));
                    if let Some(cur) = resp.hover_pos() {
                        // Use the CONSTRAINED cursor so CARD snaps the
                        // rotation to the cardinal directions and the ghost
                        // matches what the click commits.
                        let cur_world = self.cursor_world_constrained(
                            Some(cur), rect, snap_hit.map(|h| h.point))
                            .unwrap_or_else(|| self.s2w(cur, rect));
                        let cur_s = self.w2s(cur_world, rect);
                        let angle = (cur_world - pivot).angle();
                        // Baseline pivot→cursor.
                        painter.line_segment(
                            [pivot_s, cur_s],
                            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 200, 80, 180)));
                        // Ghost of the rotated selection.
                        let ghost = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 130);
                        for &i in &self.selection {
                            let Some(d) = self.doc.dobjects.get(i) else { continue; };
                            let g = d.geom.rotated(pivot, angle);
                            draw_dobject(&painter, rect, self, &g, ghost);
                        }
                        // Angle label near cursor.
                        let deg = angle.to_degrees();
                        painter.text(
                            cur_s + egui::vec2(12.0, -12.0),
                            egui::Align2::LEFT_BOTTOM,
                            format!("{:.1}°{}", deg, if self.rotate_copy { "  (copy)" } else { "" }),
                            crate::theme::typ::data_value(), mark);
                    }
                }
                RotateState::WaitingForRefSrc2(_, s1) => {
                    // First source point captured; show it + a cursor
                    // baseline indicating the in-progress source direction.
                    let mark = egui::Color32::from_rgb(180, 220, 120);
                    painter.circle_filled(self.w2s(s1, rect), 3.0, mark);
                    if let Some(cur) = resp.hover_pos() {
                        painter.line_segment(
                            [self.w2s(s1, rect), cur],
                            egui::Stroke::new(1.0,
                                egui::Color32::from_rgba_unmultiplied(180, 220, 120, 180)));
                    }
                }
                RotateState::WaitingForRefTgt(pivot, src_angle) => {
                    // Source direction captured; the NEW direction is
                    // anchored at the pivot. Pivot mark + live baseline
                    // pivot→cursor + ghost-rendered selection rotated to
                    // (cursor angle − src_angle).
                    let pivot_s = self.w2s(pivot, rect);
                    let mark    = egui::Color32::from_rgb(255, 200, 80);
                    painter.circle_stroke(pivot_s, 5.0, egui::Stroke::new(1.4, mark));
                    painter.line_segment(
                        [pivot_s + egui::vec2(-9.0, 0.0), pivot_s + egui::vec2(9.0, 0.0)],
                        egui::Stroke::new(0.8, mark));
                    painter.line_segment(
                        [pivot_s + egui::vec2(0.0, -9.0), pivot_s + egui::vec2(0.0, 9.0)],
                        egui::Stroke::new(0.8, mark));
                    if let Some(cur) = resp.hover_pos() {
                        let cur_world = self.s2w(cur, rect);
                        let tgt   = (cur_world - pivot).angle();
                        let dtheta = {
                            let mut d = (tgt - src_angle).rem_euclid(std::f64::consts::TAU);
                            if d > std::f64::consts::PI { d -= std::f64::consts::TAU; }
                            d
                        };
                        painter.line_segment(
                            [pivot_s, cur],
                            egui::Stroke::new(1.0,
                                egui::Color32::from_rgba_unmultiplied(255, 200, 80, 180)));
                        let ghost = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 130);
                        for &i in &self.selection {
                            let Some(d) = self.doc.dobjects.get(i) else { continue; };
                            let g = d.geom.rotated(pivot, dtheta);
                            draw_dobject(&painter, rect, self, &g, ghost);
                        }
                        painter.text(
                            cur + egui::vec2(12.0, -12.0),
                            egui::Align2::LEFT_BOTTOM,
                            format!("{:.1}°{}", dtheta.to_degrees(),
                                if self.rotate_copy { "  (copy)" } else { "" }),
                            crate::theme::typ::data_value(), mark);
                    }
                }
                _ => {}
            }

            // ---- Scale live preview ---------------------------------------
            // WaitingForFactor: ghost-render the selection scaled by the
            // current cursor distance from the pivot. Reference sub-states
            // visualise the captured ref endpoints.
            match self.scale_state {
                ScaleState::WaitingForFactor(pivot) => {
                    let pivot_s = self.w2s(pivot, rect);
                    let mark    = egui::Color32::from_rgb(255, 200, 80);
                    painter.circle_stroke(pivot_s, 5.0, egui::Stroke::new(1.4, mark));
                    painter.line_segment(
                        [pivot_s + egui::vec2(-9.0, 0.0), pivot_s + egui::vec2(9.0, 0.0)],
                        egui::Stroke::new(0.8, mark));
                    painter.line_segment(
                        [pivot_s + egui::vec2(0.0, -9.0), pivot_s + egui::vec2(0.0, 9.0)],
                        egui::Stroke::new(0.8, mark));
                    if let Some(cur) = resp.hover_pos() {
                        // Constrained cursor so CARD-locked H/V scaling
                        // previews exactly what the click commits.
                        let cur_world = self.cursor_world_constrained(
                            Some(cur), rect, snap_hit.map(|h| h.point))
                            .unwrap_or_else(|| self.s2w(cur, rect));
                        let cur_s     = self.w2s(cur_world, rect);
                        let factor    = pivot.dist(cur_world);
                        // Baseline pivot→cursor.
                        painter.line_segment(
                            [pivot_s, cur_s],
                            egui::Stroke::new(1.0,
                                egui::Color32::from_rgba_unmultiplied(255, 200, 80, 180)));
                        // Ghost of the scaled selection.
                        let ghost = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 130);
                        for &i in &self.selection {
                            let Some(d) = self.doc.dobjects.get(i) else { continue; };
                            let g = d.geom.scaled(pivot, factor);
                            draw_dobject(&painter, rect, self, &g, ghost);
                        }
                        painter.text(
                            cur_s + egui::vec2(12.0, -12.0),
                            egui::Align2::LEFT_BOTTOM,
                            format!("×{:.3}{}", factor,
                                if self.scale_copy { "  (copy)" } else { "" }),
                            crate::theme::typ::data_value(), mark);
                    }
                }
                ScaleState::WaitingForRefEnd(_, s) => {
                    let mark = egui::Color32::from_rgb(180, 220, 120);
                    painter.circle_filled(self.w2s(s, rect), 3.0, mark);
                }
                ScaleState::WaitingForNewLength(pivot, _) => {
                    let pivot_s = self.w2s(pivot, rect);
                    let mark    = egui::Color32::from_rgb(255, 200, 80);
                    painter.circle_stroke(pivot_s, 5.0, egui::Stroke::new(1.4, mark));
                    if let Some(cur) = resp.hover_pos() {
                        painter.line_segment(
                            [pivot_s, cur],
                            egui::Stroke::new(1.0,
                                egui::Color32::from_rgba_unmultiplied(255, 200, 80, 180)));
                    }
                }
                _ => {}
            }

            // pending click points + rubber-band preview
            let preview_col = egui::Color32::from_rgb(255, 220, 100);
            for p in &self.pending {
                painter.circle_filled(self.w2s(*p, rect), 4.0, preview_col);
            }
            // ---- Prompt-flow (CIRCLE) live ghost preview ------------------
            // Drafting commands preview by default. The flow's current step
            // declares its ghost geometry (`flow_preview`); render it at the
            // constrained/snapped cursor. (Flow runs with tool == None, so it
            // sits OUTSIDE the tool-preview block below.) See COMMAND_LINE.md.
            if self.cmd_flow.is_some() && self.draft_preview {
                if let Some(raw_cursor) = resp.hover_pos() {
                    let cw = self.cursor_world_constrained(
                        Some(raw_cursor), rect, snap_hit.map(|h| h.point))
                        .unwrap_or_else(|| self.s2w(raw_cursor, rect));
                    let cursor = self.w2s(cw, rect);
                    let gcol = egui::Color32::from_rgba_unmultiplied(
                        120, 220, 255, pulse_alpha);
                    let (ghosts, anchor) = self.flow_preview(cw);
                    for g in &ghosts {
                        if let Geom::Circle(c) = g { if c.radius <= 1e-9 { continue; } }
                        draw_dobject(&painter, rect, self, g, gcol);
                    }
                    if let Some(a) = anchor {
                        painter.line_segment([self.w2s(a, rect), cursor],
                            egui::Stroke::new(0.5, gcol.gamma_multiply(0.5)));
                    }
                }
            }
            if self.tool != Tool::None {
                if let Some(raw_cursor) = resp.hover_pos() {
                    // The preview must agree with what the click will
                    // actually commit, in the SAME priority the capture
                    // path uses: osnap > CARD > grid > raw. Without the
                    // constraint pass the rubber-band tracked the raw
                    // cursor while the committed point honoured CARD/grid,
                    // so a CARD-locked line previewed diagonal but drew
                    // horizontal. `cw` is the constrained world point and
                    // `cursor` its exact screen position, so the band end
                    // and the snap marker glyph always coincide.
                    let cw = self.cursor_world_constrained(
                        Some(raw_cursor), rect, snap_hit.map(|h| h.point))
                        .unwrap_or_else(|| self.s2w(raw_cursor, rect));
                    let cursor = self.w2s(cw, rect);
                    let dash = egui::Stroke::new(1.0, preview_col);
                    let hint = egui::Stroke::new(0.5, preview_col.gamma_multiply(0.45));

                    // Tool::Line + Tool::Circle + Tool::Ellipse — independent
                    // of arc_method.
                    match (self.tool, self.pending.as_slice()) {
                        (Tool::Line, [a]) => {
                            painter.line_segment([self.w2s(*a, rect), cursor], dash);
                        }
                        (Tool::Rectangle, [a]) => {
                            // Rubber-band rectangle from the first corner to
                            // the (constrained) cursor — the shape the click
                            // will commit.
                            let a_s = self.w2s(*a, rect);
                            let r = egui::Rect::from_two_pos(a_s, cursor);
                            painter.rect_stroke(r, 0.0, dash);
                        }
                        (Tool::Wall, [a]) => {
                            // Ghost = the wall the user is about to
                            // commit. Build a temporary Geom::Wall and
                            // walk its side-line accessors so the
                            // preview matches what apply will draw.
                            // Same metre→doc-unit conversion as the commit path (Tool::Wall),
                            // so the dashed preview is the width the wall will actually get.
                            let ghost = cad_kernel::Wall {
                                start: *a, end: cw,
                                thickness: self.doc.units.from_metres(self.env.WlThk),
                                style: self.current_wall_style, bulge: 0.0,
                            };
                            if let Some(l) = ghost.left_line() {
                                painter.line_segment(
                                    [self.w2s(l.a, rect), self.w2s(l.b, rect)], dash);
                            }
                            if let Some(r) = ghost.right_line() {
                                painter.line_segment(
                                    [self.w2s(r.a, rect), self.w2s(r.b, rect)], dash);
                            }
                            painter.line_segment(
                                [self.w2s(*a, rect), cursor], hint);
                        }
                        (Tool::Circle, [c]) => {
                            let r_px = c.dist(cw) as f32 * self.scale;
                            painter.circle_stroke(self.w2s(*c, rect), r_px, dash);
                            painter.line_segment([self.w2s(*c, rect), cursor], hint);
                        }
                        // Polyline live preview — every captured segment
                        // (solid hint) plus a rubber-band from the last
                        // vertex to the cursor. Arc-mode segments
                        // tessellate from their bulge so the user sees
                        // the actual arc, not its chord. Each captured
                        // vertex gets a small filled dot.
                        (Tool::Polyline, verts) if !verts.is_empty() => {
                            // LIVE WIDTH preview: if any width is set, fill the
                            // committed segments + the rubber-band with their
                            // widths so the user sees the wide shape WHILE
                            // drawing (not only after the command finishes).
                            let any_w = self.pline_next_width.0.abs() > 1e-9
                                || self.pline_next_width.1.abs() > 1e-9
                                || self.pending_widths.iter()
                                    .any(|&(s, e)| s.abs() > 1e-9 || e.abs() > 1e-9);
                            if any_w {
                                let push_seg = |cl: &mut Vec<(Vec2, f64)>,
                                                a: Vec2, b: Vec2, bulge: f64,
                                                sw: f64, ew: f64, first: bool| {
                                    let mut pts = vec![a];
                                    append_arc_world_samples(a, b, bulge, &mut pts);
                                    let mm = pts.len().max(1);
                                    for (j, pt) in pts.iter().enumerate() {
                                        let t = if mm > 1 { j as f64 / (mm - 1) as f64 } else { 0.0 };
                                        let w = sw + (ew - sw) * t;
                                        if !first && j == 0 {
                                            // coincident re-emit only on a width step
                                            if let Some(&(_, pw)) = cl.last() {
                                                if (pw - w).abs() > 1e-9 { cl.push((*pt, w)); }
                                            }
                                            continue;
                                        }
                                        cl.push((*pt, w));
                                    }
                                };
                                let mut cl: Vec<(Vec2, f64)> = Vec::new();
                                for i in 0..verts.len().saturating_sub(1) {
                                    let (sw, ew) = self.pending_widths
                                        .get(i).copied().unwrap_or((0.0, 0.0));
                                    let bulge = self.pending_bulges
                                        .get(i).copied().unwrap_or(0.0);
                                    push_seg(&mut cl, verts[i], verts[i + 1], bulge, sw, ew, i == 0);
                                }
                                // Rubber-band segment with the CURRENT width.
                                if let Some(&last) = verts.last() {
                                    let rb = matches!((self.pline_mode, self.pline_arc_sub),
                                        (PlineMode::Line, _) | (PlineMode::Arc, PlineArcSub::Normal));
                                    if rb {
                                        let bulge = if self.pline_mode == PlineMode::Arc {
                                            self.pline_arc_bulge_to(cw)
                                        } else { 0.0 };
                                        let (sw, ew) = self.pline_next_width;
                                        let first = cl.is_empty();
                                        push_seg(&mut cl, last, cw, bulge, sw, ew, first);
                                    }
                                }
                                fill_width_strip(&painter, rect, self, &cl,
                                    preview_col.gamma_multiply(0.40));
                            }
                            let solid = egui::Stroke::new(
                                1.0, preview_col.gamma_multiply(0.7));
                            for w in verts.iter() {
                                painter.circle_filled(self.w2s(*w, rect), 3.0, preview_col);
                            }
                            for i in 0..verts.len().saturating_sub(1) {
                                let a = verts[i];
                                let b = verts[i + 1];
                                let bulge = self.pending_bulges
                                    .get(i).copied().unwrap_or(0.0);
                                self.draw_pline_preview_segment(
                                    &painter, rect, a, b, bulge, solid);
                            }
                            // Rubber-band from last captured to cursor.
                            // In Arc mode this is also a tessellated
                            // arc so the user sees the actual curvature
                            // the click will commit. The Second-pt
                            // sub-flow shows two distinct shapes for
                            // its two clicks.
                            if let Some(last) = verts.last() {
                                match (self.pline_mode, self.pline_arc_sub) {
                                    (PlineMode::Arc, PlineArcSub::AwaitingSecondPt) => {
                                        // Two reference lines (last→cursor and a
                                        // dotted hint suggesting the second point
                                        // will lie on the arc).
                                        painter.line_segment(
                                            [self.w2s(*last, rect), cursor], dash);
                                        painter.circle_filled(cursor, 4.0,
                                            preview_col.gamma_multiply(0.8));
                                    }
                                    (PlineMode::Arc, PlineArcSub::AwaitingSecondPtEnd(mid)) => {
                                        // 3-point arc through (last, mid, cursor).
                                        let mid_s = self.w2s(mid, rect);
                                        let bulge = bulge_from_three_points(*last, mid, cw);
                                        self.draw_pline_preview_segment(
                                            &painter, rect, *last, cw, bulge, dash);
                                        painter.circle_filled(mid_s, 4.0,
                                            preview_col.gamma_multiply(0.9));
                                    }
                                    (PlineMode::Arc, PlineArcSub::Normal) => {
                                        let bulge = self.pline_arc_bulge_to(cw);
                                        self.draw_pline_preview_segment(
                                            &painter, rect, *last, cw, bulge, dash);
                                    }
                                    (PlineMode::Arc, PlineArcSub::AwaitingDirection) => {
                                        // Cursor defines the start tangent — show
                                        // it as a reference line from the last
                                        // vertex toward the cursor.
                                        painter.line_segment(
                                            [self.w2s(*last, rect), cursor], dash);
                                    }
                                    (PlineMode::Line, _) => {
                                        painter.line_segment(
                                            [self.w2s(*last, rect), cursor], dash);
                                    }
                                }
                            }
                        }
                        // Spline live preview — control-polygon hint
                        // (thin chord lines between successive captured
                        // control points) PLUS the actual NURBS curve
                        // sampled through pending + cursor as the live
                        // next control point. Cubic by default (degree
                        // 3); lower degrees fall in until the user has
                        // ≥ 4 control points.
                        (Tool::Spline, verts) if !verts.is_empty() => {
                            // Captured control points as small dots.
                            for w in verts.iter() {
                                painter.circle_filled(self.w2s(*w, rect), 3.0, preview_col);
                            }
                            // Faint control polygon.
                            let hint_pen = egui::Stroke::new(
                                0.7, preview_col.gamma_multiply(0.4));
                            for pair in verts.windows(2) {
                                painter.line_segment(
                                    [self.w2s(pair[0], rect), self.w2s(pair[1], rect)],
                                    hint_pen);
                            }
                            // Last captured → cursor (control-polygon
                            // continuation, hinting where the next
                            // ctrl lands).
                            if let Some(last) = verts.last() {
                                painter.line_segment(
                                    [self.w2s(*last, rect), cursor], hint_pen);
                            }
                            // The CURVE — build a transient Spline
                            // including the cursor as the live next
                            // control point, tessellate, draw dashed.
                            let mut ctrls: Vec<Vec2> = verts.to_vec();
                            ctrls.push(cw);
                            if ctrls.len() >= 2 {
                                let degree = 3.min(ctrls.len() - 1);
                                let s = cad_kernel::Spline::new_bspline(degree, ctrls);
                                let n = (s.bbox().1 - s.bbox().0).len() as f32 * self.scale;
                                let n = (n * 0.5).clamp(32.0, 256.0) as usize;
                                let samples = s.tessellate(n);
                                if samples.len() >= 2 {
                                    let pts: Vec<egui::Pos2> = samples.iter()
                                        .map(|w| self.w2s(*w, rect)).collect();
                                    painter.add(egui::Shape::line(pts, dash));
                                }
                            }
                        }
                        // Ellipse 3-click flow.
                        // Stage 1 (pending=[centre]): rubber-band line from
                        // centre to cursor — defines the major axis.
                        (Tool::Ellipse, [c]) => {
                            painter.line_segment([self.w2s(*c, rect), cursor], dash);
                        }
                        // Stage 2 (pending=[centre, major_end]): show the
                        // major-axis line and a live ellipse using the
                        // current cursor for the minor.
                        (Tool::Ellipse, [c, me]) => {
                            draw_ellipse_preview(&painter, rect, self,
                                *c, *me, cw, dash, hint);
                        }
                        // ----- elliptical-arc 5-stage preview -----
                        (Tool::EllipseArc, [c]) => {
                            painter.line_segment([self.w2s(*c, rect), cursor], dash);
                        }
                        (Tool::EllipseArc, [c, me]) => {
                            draw_ellipse_preview(&painter, rect, self,
                                *c, *me, cw, dash, hint);
                        }
                        (Tool::EllipseArc, [c, me, mp]) => {
                            // Ellipse is now defined. Live preview shows the
                            // fixed full ellipse + a marker where the cursor
                            // projects onto it (= future start point).
                            let major = *me - *c;
                            if major.len() > EPS {
                                let v_hat = major.normalized().perp();
                                let semi_minor = (*mp - *c).dot(v_hat).abs();
                                if let Some(el) = ellipse_center_major_minor(*c, *me, semi_minor) {
                                    draw_polyline_full_ellipse(&painter, rect, self, &el, hint);
                                    let t = el.nearest_param(cw);
                                    let on = self.w2s(el.point_at(t), rect);
                                    painter.line_segment([self.w2s(*c, rect), on], dash);
                                    painter.circle_filled(on, 3.5, preview_col);
                                }
                            }
                        }
                        (Tool::EllipseArc, [c, me, mp, sp]) => {
                            // Ellipse fixed + start fixed; live preview shows
                            // a partial elliptical arc from start to cursor's
                            // projection (CCW).
                            let major = *me - *c;
                            if major.len() > EPS {
                                let v_hat = major.normalized().perp();
                                let semi_minor = (*mp - *c).dot(v_hat).abs();
                                if let Some(el) = ellipse_center_major_minor(*c, *me, semi_minor) {
                                    draw_polyline_full_ellipse(&painter, rect, self, &el, hint);
                                    let t_start = el.nearest_param(*sp);
                                    let t_end   = el.nearest_param(cw);
                                    let sweep_raw = (t_end - t_start)
                                        .rem_euclid(std::f64::consts::TAU);
                                    let sweep = if sweep_raw < 1e-6 {
                                        std::f64::consts::TAU
                                    } else {
                                        sweep_raw
                                    };
                                    let ea = EllipseArc {
                                        ellipse: el,
                                        start_param: t_start.rem_euclid(std::f64::consts::TAU),
                                        sweep_param: sweep,
                                    };
                                    let n = 64;
                                    let mut pts = Vec::with_capacity(n + 1);
                                    for i in 0..=n {
                                        let t = ea.start_param +
                                            (i as f64 / n as f64) * ea.sweep_param;
                                        pts.push(self.w2s(el.point_at(t), rect));
                                    }
                                    painter.add(egui::Shape::line(pts, dash));
                                }
                            }
                        }
                        _ => {}
                    }

                    // Arc preview depends on the current method, since the
                    // semantics of pending[0] / pending[1] differ per method.
                    let arc_polyline = |arc: Arc| -> Vec<egui::Pos2> {
                        let n = 64;
                        let mut pts = Vec::with_capacity(n + 1);
                        for i in 0..=n {
                            let t = arc.start_angle
                                  + (i as f64 / n as f64) * arc.sweep_angle;
                            let p = Vec2::new(
                                arc.center.x + arc.radius * t.cos(),
                                arc.center.y + arc.radius * t.sin(),
                            );
                            pts.push(self.w2s(p, rect));
                        }
                        pts
                    };
                    let ccw_arc_from_center_endpoints = |c: Vec2, s: Vec2, e: Vec2| -> Arc {
                        let radius = c.dist(s);
                        let sa = (s - c).angle();
                        let ea = (e - c).angle();
                        let sweep_raw = (ea - sa).rem_euclid(std::f64::consts::TAU);
                        let sweep = if sweep_raw < 1e-6 {
                            std::f64::consts::TAU
                        } else {
                            sweep_raw
                        };
                        Arc { center: c, radius, start_angle: sa, sweep_angle: sweep }
                    };

                    if self.tool == Tool::Arc {
                        match (self.arc_method, self.pending.as_slice()) {
                            // ---- 3-POINT: pending = points ON the arc, no centre at all ----
                            (ArcMethod::ThreePoints, [p1]) => {
                                // just a chord-hint line, no circle
                                painter.line_segment([self.w2s(*p1, rect), cursor], hint);
                            }
                            (ArcMethod::ThreePoints, [p1, p2]) => {
                                if let Some(arc) = arc_three_points(*p1, *p2, cw) {
                                    painter.add(egui::Shape::line(arc_polyline(arc), dash));
                                } else {
                                    // collinear → no preview, just guide chords
                                    painter.line_segment([self.w2s(*p1, rect), cursor], hint);
                                    painter.line_segment([self.w2s(*p2, rect), cursor], hint);
                                }
                            }

                            // ---- S,C,E: pending = [start, (center)] ----
                            (ArcMethod::StartCenterEnd, [s]) => {
                                // next click is the centre — show a hint line
                                painter.line_segment([self.w2s(*s, rect), cursor], hint);
                            }
                            (ArcMethod::StartCenterEnd, [s, c]) => {
                                // full radius circle hint + the CCW arc s→cursor around c
                                let r_px = c.dist(*s) as f32 * self.scale;
                                painter.circle_stroke(self.w2s(*c, rect), r_px, hint);
                                let arc = ccw_arc_from_center_endpoints(*c, *s, cw);
                                painter.add(egui::Shape::line(arc_polyline(arc), dash));
                            }

                            // ---- C,S,E: pending = [center, (start)] ----
                            (ArcMethod::CenterStartEnd, [c]) => {
                                // next click is the start — show radius line
                                painter.line_segment([self.w2s(*c, rect), cursor], hint);
                            }
                            (ArcMethod::CenterStartEnd, [c, s]) => {
                                let r_px = c.dist(*s) as f32 * self.scale;
                                painter.circle_stroke(self.w2s(*c, rect), r_px, hint);
                                let arc = ccw_arc_from_center_endpoints(*c, *s, cw);
                                painter.add(egui::Shape::line(arc_polyline(arc), dash));
                            }

                            // Frozen methods don't draw anything live.
                            _ => {}
                        }
                    }
                }
            }

            // ---- VIEW LIST — which plane the 2D canvas is drawing on -------------------------
            //
            // This used to be a fixed row of Top / Front / Back / Left / Right views of the whole
            // model. It is now the FACES ACTUALLY DRAWN ON, because that is what someone is looking
            // for when they come back to a drawing:
            //
            //   "instead of showing the planes like top, left right etc, lets get rid of it. now it
            //    will show only faces as planes the user draws on, the user can even rename these
            //    view[s] so they can instantly look at a sketch they made. instead of showing the
            //    whole side view, it will only show whatever face as a plane the user is drawing
            //    on."
            //
            // GROUND PLAN stays, and is the only entry that is not a sketch plane: it is the drawing
            // itself, the document every 2D tool has always worked on.
            //
            // The list IS `model.sketches`, so a face reached by right-clicking it in 3D and a face
            // picked by name here are the same plane with the same drawing on it — there is no
            // second record to fall out of step.
            {
                let open = self.factory.session.as_ref().map(|s| s.plane);
                // BY ID, not by row number. The row a plane is on changes whenever another is
                // deleted, and these values are read back AFTER the menu closes — so an index
                // captured here would be answered against a different list.
                let names: Vec<(u32, String)> = self
                    .factory
                    .model
                    .sketches
                    .iter()
                    .map(|s| (s.id, s.name.clone()))
                    .collect();
                let name_of = |id: u32| {
                    names.iter().find(|(k, _)| *k == id).map(|(_, n)| n.clone())
                };
                let cur_label = match open {
                    Some(id) => name_of(id).unwrap_or_else(|| "Plane".into()),
                    None => "Global view".to_string(),
                };
                let mut want: Option<Option<u32>> = None; // Some(None) = the global view
                let mut rename: Option<u32> = None;
                let mut delete: Option<u32> = None;
                // INSIDE THE CANVAS, not floating over the window.
                //
                // Reported as: "the views menu should be visible in the 2d cad window … now it
                // overflows into other windows when 2d cad is moved". An `egui::Area` is a
                // TOP-LEVEL layer: it is positioned in screen coordinates and clipped by nothing,
                // so as soon as the canvas narrowed — which is exactly what the SIMLUX split does —
                // the button and its dropdown spilled across the panels beside it.
                //
                // A child UI of the canvas is clipped to the canvas. It belongs to the 2D window
                // because it is a control OF the 2D window.
                let toggle_rect = egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(10.0, 6.0),
                    egui::vec2((rect.width() - 20.0).max(60.0), 30.0),
                );
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(toggle_rect).layout(egui::Layout::left_to_right(egui::Align::Min)),
                    |ui| {
                        // An `Area` takes its width from its content, and a menu button's label
                        // will WRAP to fit rather than push the button wider — so "Global view"
                        // came out stacked three lines tall in a button barely wider than the
                        // arrow. Extend, don't wrap, and give the row a floor to sit on.
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        egui::Frame::popup(ui.style())
                            .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                            .show(ui, |ui| {
                                ui.set_min_width(150.0);
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                ui.menu_button(format!("▼  {cur_label}"), |ui| {
                                    ui.set_min_width(230.0);
                                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                    if ui
                                        .selectable_label(open.is_none(), "Global view")
                                        .on_hover_text(
                                            "The whole model from above — solids, rooms and \
                                             furniture — and the drawing every 2D tool works on",
                                        )
                                        .clicked()
                                    {
                                        want = Some(None);
                                        ui.close_menu();
                                    }
                                    if names.is_empty() {
                                        ui.separator();
                                        ui.label(
                                            egui::RichText::new(
                                                "No face planes yet.\nRight-click a face in 3D and\nchoose \"draw on this face\".",
                                            )
                                            .small()
                                            .weak(),
                                        );
                                        return;
                                    }
                                    ui.separator();
                                    ui.label(egui::RichText::new("faces drawn on").small().weak());
                                    for (i, name) in &names {
                                        ui.horizontal(|ui| {
                                            if ui
                                                .selectable_label(open == Some(*i), name)
                                                .clicked()
                                            {
                                                want = Some(Some(*i));
                                                ui.close_menu();
                                            }
                                            if ui.small_button("✎").on_hover_text("Rename").clicked() {
                                                rename = Some(*i);
                                                ui.close_menu();
                                            }
                                            if ui.small_button("🗑").on_hover_text("Delete this plane and the drawing on it").clicked() {
                                                delete = Some(*i);
                                                ui.close_menu();
                                            }
                                        });
                                    }
                                });
                            });
                    },
                );
                if let Some(w) = want {
                    self.factory_open_plane(w);
                }
                if let Some(id) = rename {
                    self.factory.rename_plane = Some((id, name_of(id).unwrap_or_default()));
                }
                if let Some(id) = delete {
                    self.factory_delete_plane(id);
                }
            }

            // HUD: cursor world coords + tool hint. Below the view toggle, which owns the corner.
            if let Some(pos) = resp.hover_pos() {
                let w = self.s2w(pos, rect);
                painter.text(
                    rect.left_top() + egui::vec2(10.0, 38.0),
                    egui::Align2::LEFT_TOP,
                    format!(
                        "cursor: ({:>9.3}, {:>9.3})   scale: {:>6.2} px/u",
                        w.x, w.y, self.scale
                    ),
                    crate::theme::typ::data_code(),
                    egui::Color32::from_rgb(200, 220, 240),
                );
            }
            painter.text(
                rect.left_top() + egui::vec2(10.0, 58.0),
                egui::Align2::LEFT_TOP,
                current_hint(self.tool, self.arc_method, self.pending.len()),
                crate::theme::typ::data_code(),
                egui::Color32::from_rgb(255, 220, 120),
            );

            // ---- move tool overlay (live preview + base→cursor arrow) --
            if self.move_state != MoveState::Off {
                let hint_text = match self.move_state {
                    MoveState::WaitingForBase =>
                        format!("MOVE: click BASE point for {} dobject(s)    [Esc cancels]",
                            self.selection.len()),
                    MoveState::WaitingForDest(_) =>
                        format!("MOVE: click DESTINATION ({} dobject(s) following)",
                            self.selection.len()),
                    MoveState::Off => unreachable!(),
                };
                painter.text(
                    rect.left_top() + egui::vec2(10.0, 48.0),
                    egui::Align2::LEFT_TOP,
                    hint_text,
                    crate::theme::typ::data_code(),
                    egui::Color32::from_rgb(255, 200, 100),
                );

                if let MoveState::WaitingForDest(base) = self.move_state {
                    let cur_world = self.cursor_world_constrained(
                        resp.hover_pos(), rect, snap_hit.map(|h| h.point));
                    if let Some(cw) = cur_world {
                        let v = cw - base;
                        let base_s = self.w2s(base, rect);
                        let dest_s = self.w2s(cw, rect);
                        let accent = egui::Color32::from_rgb(255, 200, 100);
                        // base BLIP + animated dashed vector to cursor
                        draw_base_blip(&painter, base_s, accent);
                        let time = ctx.input(|i| i.time) as f32;
                        let phase = time * 60.0;   // marching-ants speed (px/s)
                        draw_dashed_line(&painter, base_s, dest_s,
                            6.0, 4.0, phase,
                            egui::Stroke::new(1.2, accent));
                        // ghost-render the selected dobjects at +v
                        let ghost_col = egui::Color32::from_rgba_unmultiplied(255, 200, 100, 180);
                        for &i in &self.selection {
                            if let Some(d) = self.doc.dobjects.get(i) {
                                let moved = d.geom.translated(v);
                                draw_dobject(&painter, rect, self, &moved, ghost_col);
                            }
                        }
                    }
                }
            }

            // ---- copy tool overlay (mirrors move; greener accent so the
            //      user can tell the two apart at a glance) --------------
            if self.copy_state != CopyState::Off {
                let hint_text = match self.copy_state {
                    CopyState::WaitingForBase =>
                        format!("COPY: click BASE point for {} dobject(s)    [Esc cancels]",
                            self.selection.len()),
                    CopyState::WaitingForDest(_) =>
                        format!("COPY: click DESTINATION ({} dobject(s) being duplicated)",
                            self.selection.len()),
                    CopyState::Off => unreachable!(),
                };
                painter.text(
                    rect.left_top() + egui::vec2(10.0, 48.0),
                    egui::Align2::LEFT_TOP,
                    hint_text,
                    crate::theme::typ::data_code(),
                    egui::Color32::from_rgb(150, 230, 170),
                );

                if let CopyState::WaitingForDest(base) = self.copy_state {
                    let cur_world = self.cursor_world_constrained(
                        resp.hover_pos(), rect, snap_hit.map(|h| h.point));
                    if let Some(cw) = cur_world {
                        let v = cw - base;
                        let base_s = self.w2s(base, rect);
                        let dest_s = self.w2s(cw, rect);
                        let accent = egui::Color32::from_rgb(150, 230, 170);
                        draw_base_blip(&painter, base_s, accent);
                        let time = ctx.input(|i| i.time) as f32;
                        let phase = time * 60.0;
                        draw_dashed_line(&painter, base_s, dest_s,
                            6.0, 4.0, phase,
                            egui::Stroke::new(1.2, accent));
                        let ghost_col = egui::Color32::from_rgba_unmultiplied(150, 230, 170, 180);
                        for &i in &self.selection {
                            if let Some(d) = self.doc.dobjects.get(i) {
                                let copied = d.geom.translated(v);
                                draw_dobject(&painter, rect, self, &copied, ghost_col);
                            }
                        }
                    }
                }
            }

            // ---- paste tool overlay (base→dest ghost, like copy) -------
            if self.paste_state != PasteState::Off {
                let hint_text = match self.paste_state {
                    PasteState::WaitingForBase =>
                        format!("PASTE: click BASE point for {} dobject(s)   [Esc cancels]",
                            self.clipboard_dobjects.len()),
                    PasteState::WaitingForDest(_) =>
                        format!("PASTE: click DESTINATION ({} dobject(s))",
                            self.clipboard_dobjects.len()),
                    PasteState::Off => unreachable!(),
                };
                painter.text(
                    rect.left_top() + egui::vec2(10.0, 48.0),
                    egui::Align2::LEFT_TOP,
                    hint_text,
                    crate::theme::typ::data_code(),
                    egui::Color32::from_rgb(150, 230, 170),
                );
                if let PasteState::WaitingForDest(base) = self.paste_state {
                    let cur_world = self.cursor_world_constrained(
                        resp.hover_pos(), rect, snap_hit.map(|h| h.point));
                    if let Some(cw) = cur_world {
                        // Base is auto (clipboard lower-left) → no base blip /
                        // dashed line; just ghost the content under the cursor.
                        let v = cw - base;
                        let ghost_col = egui::Color32::from_rgba_unmultiplied(150, 230, 170, 180);
                        for d in &self.clipboard_dobjects {
                            let g = d.geom.translated(v);
                            draw_dobject(&painter, rect, self, &g, ghost_col);
                        }
                    }
                }
            }

            // ---- mirror axis overlay (preview line + ghost result) -----
            // While picking the second axis point (WaitingForB) the axis is
            // a→cursor; once fixed (AwaitingKeep) it's a→b. Either way we
            // draw the dashed axis (extended past both ends so it reads as
            // a mirror line) plus a translucent ghost of the mirrored
            // selection, so the result is visible before committing.
            let mirror_axis: Option<(Vec2, Option<Vec2>)> = match self.mirror_state {
                MirrorState::WaitingForB(a)     => Some((a, None)),
                MirrorState::AwaitingKeep(a, b) => Some((a, Some(b))),
                _ => None,
            };
            if let Some((a, b_fixed)) = mirror_axis {
                let b = b_fixed.or_else(|| self.cursor_world_constrained(
                    resp.hover_pos(), rect, snap_hit.map(|h| h.point)));
                if let Some(b) = b {
                    let dir = b - a;
                    let len = dir.len();
                    if len > EPS {
                        let accent = egui::Color32::from_rgb(200, 160, 255);
                        let u = dir / len;
                        let ext = 40.0 / self.scale as f64;   // ~40 px past each end
                        let p0 = self.w2s(a - u * ext, rect);
                        let p1 = self.w2s(b + u * ext, rect);
                        let time = ctx.input(|i| i.time) as f32;
                        draw_dashed_line(&painter, p0, p1, 8.0, 5.0,
                            time * 40.0, egui::Stroke::new(1.4, accent));
                        draw_base_blip(&painter, self.w2s(a, rect), accent);
                        painter.circle_filled(self.w2s(b, rect), 3.0, accent);
                        // Ghost of the mirrored selection.
                        let ghost = egui::Color32::from_rgba_unmultiplied(
                            200, 160, 255, 150);
                        for &i in &self.selection {
                            if let Some(d) = self.doc.dobjects.get(i) {
                                let m = d.geom.mirrored(a, b);
                                draw_dobject(&painter, rect, self, &m, ghost);
                            }
                        }
                    }
                }
                let hint = match self.mirror_state {
                    MirrorState::WaitingForB(_) =>
                        "MIRROR: click SECOND axis point (axis + ghost shown)",
                    _ => "MIRROR: keep original? [Y]/n  (Enter = keep a copy)",
                };
                painter.text(rect.left_top() + egui::vec2(10.0, 48.0),
                    egui::Align2::LEFT_TOP, hint,
                    crate::theme::typ::data_code(),
                    egui::Color32::from_rgb(200, 160, 255));
            }

            // ---- stretch overlay (window box + vector + ghost) ---------
            // Once the crossing window is captured, keep it drawn (faint
            // green box) and — while picking the destination — show the
            // base→cursor vector plus a ghost of the stretched result so
            // the user sees exactly which vertices move and by how much.
            match self.stretch_state {
                StretchState::WaitingForBase(wmin, wmax)
                | StretchState::WaitingForDest(wmin, wmax, _) => {
                    let box_col = egui::Color32::from_rgb(140, 220, 100);
                    let r = egui::Rect::from_two_pos(
                        self.w2s(wmin, rect), self.w2s(wmax, rect));
                    painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0,
                        egui::Color32::from_rgba_unmultiplied(140, 220, 100, 160)));
                    if let StretchState::WaitingForDest(_, _, base) = self.stretch_state {
                        let cur = self.cursor_world_constrained(
                            resp.hover_pos(), rect, snap_hit.map(|h| h.point));
                        if let Some(cw) = cur {
                            let v = cw - base;
                            let base_s = self.w2s(base, rect);
                            let dest_s = self.w2s(cw, rect);
                            draw_base_blip(&painter, base_s, box_col);
                            let time = ctx.input(|i| i.time) as f32;
                            draw_dashed_line(&painter, base_s, dest_s, 6.0, 4.0,
                                time * 60.0, egui::Stroke::new(1.2, box_col));
                            // Ghost of the stretched geometry — only the
                            // SELECTED dobjects (the stretch set).
                            let ghost = egui::Color32::from_rgba_unmultiplied(
                                140, 220, 100, 150);
                            for &i in &self.selection {
                                if let Some(d) = self.doc.dobjects.get(i) {
                                    let g = stretch_one(&d.geom, wmin, wmax, v);
                                    draw_dobject(&painter, rect, self, &g, ghost);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }

            // ---- selection mode overlay --------------------------------
            //
            // Rubber-band rectangle from the first-corner click to the
            // current cursor. Left-to-right drag = "inside" window (solid
            // blue); right-to-left = "crossing" window (dashed green).
            if self.select_mode != SelectMode::Off {
                let label = match self.select_mode {
                    SelectMode::ForList         => "LIST: select dobjects, Enter when done (Esc cancels)",
                    SelectMode::ForSelect       => "SELECT: pick dobjects, Enter when done (Esc cancels)",
                    SelectMode::ForCuttingEdges => "TRIM: pick CUTTING edges, Enter when done (Esc cancels)",
                    SelectMode::ForBoundaryEdges=> "EXTEND: pick BOUNDARY edges, Enter when done (Esc cancels)",
                    SelectMode::Off             => unreachable!(),
                };
                painter.text(
                    rect.left_top() + egui::vec2(10.0, 48.0),
                    egui::Align2::LEFT_TOP,
                    format!("{}    [{} selected]", label, self.selection.len()),
                    crate::theme::typ::data_code(),
                    egui::Color32::from_rgb(255, 220, 120),
                );

                if let (Some(first), Some(cur)) = (self.window_first, resp.hover_pos()) {
                    let p1 = self.w2s(first, rect);
                    let p2 = cur;
                    let crossing = p2.x < p1.x;
                    let col = if crossing {
                        egui::Color32::from_rgba_unmultiplied(120, 230, 120, 60)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(120, 170, 255, 60)
                    };
                    let edge = if crossing {
                        egui::Color32::from_rgb(120, 230, 120)
                    } else {
                        egui::Color32::from_rgb(120, 170, 255)
                    };
                    let r = egui::Rect::from_two_pos(p1, p2);
                    painter.rect_filled(r, 0.0, col);
                    if crossing {
                        // dashed edges for the crossing window
                        let time = ctx.input(|i| i.time) as f32;
                        let phase = time * 40.0;
                        draw_dashed_line(&painter,
                            r.left_top(), r.right_top(),  6.0, 4.0, phase,
                            egui::Stroke::new(1.2, edge));
                        draw_dashed_line(&painter,
                            r.right_top(), r.right_bottom(), 6.0, 4.0, phase,
                            egui::Stroke::new(1.2, edge));
                        draw_dashed_line(&painter,
                            r.right_bottom(), r.left_bottom(), 6.0, 4.0, phase,
                            egui::Stroke::new(1.2, edge));
                        draw_dashed_line(&painter,
                            r.left_bottom(), r.left_top(), 6.0, 4.0, phase,
                            egui::Stroke::new(1.2, edge));
                    } else {
                        painter.rect_stroke(r, 0.0, egui::Stroke::new(1.2, edge));
                    }
                }
            }

            // ---- Drafting cursor overlay --------------------------------
            //
            // While drafting mode is active (drawing tool OR any
            // point-pick edit phase), draw a square (the "pickbox") with
            // a cross through it at the hover position. Same visual cue
            // AutoCAD uses to tell the user "I'm in command, click to
            // place a point". This is also when press-fires-click is in
            // effect — see the click pipeline override above.
            //
            // The cross sits at the CONSTRAINED position (after osnap /
            // ortho / grid-snap) so the user sees exactly where the
            // click will land, not where their raw cursor is. Drawn
            // last so it sits on top of dobjects and previews.
            // ---- ACTIVE-VIEWPORT frame (drawn last → above the drawing) -----
            // Only when the 3D view is also up: with 2D alone there is no choice to
            // make, and a permanently-yellow frame would just be noise.
            if self.factory.open {
                draw_viewport_active_frame(&painter, rect, self.active_view == ActiveView::TwoD);
            }

            // THE CURSOR SAYS WHICH MODE YOU ARE IN — always, not only sometimes.
            //
            // Two cursors already existed, but the selection one was drawn ONLY during an explicit
            // select-mode session. The normal idle state is itself the always-on selector, and it
            // drew nothing at all — so most of the time the canvas showed the plain OS arrow and
            // the mode was invisible. Now exactly one of the two is on screen whenever the pointer
            // is over the canvas:
            //
            //   OPERATION  square + full crosshair — a click places a POINT, at the snapped
            //              position, so the cursor sits where the point will land rather than
            //              where the mouse is.
            //   SELECTION  pickbox + arrow — a click picks an OBJECT, so it follows the mouse
            //              exactly and the box reads as the pick aperture.
            // …unless the LIGHTING layer owns the pointer. It sets its own crosshair for placing a
            // fixture and a grab hand over a marker, and those say something this pair cannot.
            let light_owns_cursor = self.light.place_mode
                || self.light.place_fitting.is_some()
                || self.light.hover.is_some()
                || self.light.drag.is_some();
            if !canvas_locked && !light_owns_cursor {
                if let Some(raw_p) = resp.hover_pos() {
                    if rect.contains(raw_p) {
                        // Hide the OS arrow so the drawn cursor is THE cursor — two pointers on a
                        // drafting canvas read as a glitch, and the offset between them is
                        // actively misleading while snapping.
                        ctx.set_cursor_icon(egui::CursorIcon::None);
                        // An object waiting to be placed is asking for a POINT, so the plan shows
                        // the drafting cursor even though no 2D tool is running. The pointer is
                        // the app's answer to "what is this click going to do?".
                        if in_click_only_phase || self.factory.awaiting_place.is_some() {
                            let constrained_world = snap_hit
                                .map(|h| h.point)
                                .unwrap_or_else(|| self.apply_constraints(self.s2w(raw_p, rect)));
                            let p = self.w2s(constrained_world, rect);
                            draw_draft_cursor(&painter, p);
                        } else {
                            draw_select_cursor(&painter, raw_p);
                        }
                    }
                }
            }

            ctx.request_repaint();
        });

        // Menu-layout recorder DUMP — runs after the WHOLE frame's UI, so it
        // captures every menu that was open this frame (rails + their flyouts /
        // add / remove popups, menu-bar dropdowns, the canvas right-click menu,
        // the object-snap popup, and the Properties panel). One-shot: disarms
        // itself. Open the menu(s) you want measured, THEN click "Capture menu
        // layout" so they're on-screen during the captured frame.
        // ---- Live UI inspector overlay (devtools-style ruler) -------------
        // All captured elements get a faint outline; the smallest one under the
        // pointer is highlighted with its name + exact w×h + position.
        if self.ui_inspect {
            let buf: Vec<(String, egui::Rect)> =
                ctx.data(|d| d.get_temp(egui::Id::new("pp_cap_buf")).unwrap_or_default());
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("ui_inspect_overlay"),
            ));
            for (_, r) in &buf {
                painter.rect_stroke(
                    *r,
                    0.0,
                    egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(0, 0xe5, 0xff, 40),
                    ),
                );
            }
            if let Some(p) = ctx.pointer_hover_pos() {
                if let Some((name, r)) = buf.iter().filter(|(_, r)| r.contains(p)).min_by(|a, b| {
                    (a.1.width() * a.1.height())
                        .partial_cmp(&(b.1.width() * b.1.height()))
                        .unwrap_or(std::cmp::Ordering::Equal)
                }) {
                    painter.rect_stroke(
                        *r,
                        0.0,
                        egui::Stroke::new(1.5, egui::Color32::from_rgb(0, 0xe5, 0xff)),
                    );
                    // Multi-line tooltip: geometry first, then each " | " segment
                    // of the rich capture on its own (wrapped) line.
                    let mut lines = vec![format!(
                        "▸ {:.0}×{:.0}   @({:.0}, {:.0})",
                        r.width(),
                        r.height(),
                        r.left(),
                        r.top()
                    )];
                    for part in name.split(" | ") {
                        lines.push(part.trim().to_string());
                    }
                    let font = crate::theme::typ::data_code();
                    let galley = painter.layout(
                        lines.join("\n"),
                        font,
                        egui::Color32::from_rgb(0xda, 0xe3, 0xef),
                        360.0,
                    );
                    let sz = galley.size();
                    let sr = ctx.screen_rect();
                    let mut lp = egui::pos2(p.x + 14.0, p.y + 16.0);
                    if lp.x + sz.x + 10.0 > sr.right() {
                        lp.x = (sr.right() - sz.x - 10.0).max(4.0);
                    }
                    if lp.y + sz.y + 10.0 > sr.bottom() {
                        lp.y = (p.y - sz.y - 16.0).max(4.0);
                    }
                    let bg = egui::Rect::from_min_size(
                        lp - egui::vec2(7.0, 6.0),
                        sz + egui::vec2(14.0, 12.0),
                    );
                    painter.rect(
                        bg,
                        4.0,
                        egui::Color32::from_black_alpha(238),
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(0, 0xe5, 0xff)),
                    );
                    painter.galley(lp, galley, egui::Color32::WHITE);
                }
                // ---- Spacing: pointer in the whitespace BETWEEN two boxes →
                // draw the gap (px) so row/section spacing can be checked against
                // the §5.1 tokens (ROW_GAP 8, GROUP_GAP 12, SECTION_GAP 12, …).
                let containing: Vec<egui::Rect> = buf
                    .iter()
                    .map(|(_, r)| *r)
                    .filter(|r| r.contains(p))
                    .collect();
                let cmpf = |a: &f32, b: &f32| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal);
                // Vertical gap: nearest box above & below along the pointer column.
                let up = buf
                    .iter()
                    .map(|(_, r)| *r)
                    .filter(|r| {
                        !r.contains(p) && r.left() <= p.x && p.x <= r.right() && r.bottom() <= p.y
                    })
                    .max_by(|a, b| cmpf(&a.bottom(), &b.bottom()));
                let down = buf
                    .iter()
                    .map(|(_, r)| *r)
                    .filter(|r| {
                        !r.contains(p) && r.left() <= p.x && p.x <= r.right() && r.top() >= p.y
                    })
                    .min_by(|a, b| cmpf(&a.top(), &b.top()));
                if let (Some(u), Some(d)) = (up, down) {
                    let gap = d.top() - u.bottom();
                    // Only when it's true whitespace — no leaf box sits in the band.
                    let inside_leaf = containing
                        .iter()
                        .any(|c| c.top() >= u.bottom() - 1.0 && c.bottom() <= d.top() + 1.0);
                    if gap > 0.5 && gap <= 400.0 && !inside_leaf {
                        let l = u.left().max(d.left());
                        let rgt = u.right().min(d.right());
                        pp_gap_dim(
                            &painter,
                            egui::Rect::from_min_max(
                                egui::pos2(l, u.bottom()),
                                egui::pos2(rgt, d.top()),
                            ),
                            true,
                            gap,
                        );
                    }
                }
                // Horizontal gap: nearest box left & right along the pointer row.
                let lft = buf
                    .iter()
                    .map(|(_, r)| *r)
                    .filter(|r| {
                        !r.contains(p) && r.top() <= p.y && p.y <= r.bottom() && r.right() <= p.x
                    })
                    .max_by(|a, b| cmpf(&a.right(), &b.right()));
                let rgh = buf
                    .iter()
                    .map(|(_, r)| *r)
                    .filter(|r| {
                        !r.contains(p) && r.top() <= p.y && p.y <= r.bottom() && r.left() >= p.x
                    })
                    .min_by(|a, b| cmpf(&a.left(), &b.left()));
                if let (Some(a), Some(b)) = (lft, rgh) {
                    let gap = b.left() - a.right();
                    let inside_leaf = containing
                        .iter()
                        .any(|c| c.left() >= a.right() - 1.0 && c.right() <= b.left() + 1.0);
                    if gap > 0.5 && gap <= 400.0 && !inside_leaf {
                        let t = a.top().max(b.top());
                        let bot = a.bottom().min(b.bottom());
                        pp_gap_dim(
                            &painter,
                            egui::Rect::from_min_max(
                                egui::pos2(a.right(), t),
                                egui::pos2(b.left(), bot),
                            ),
                            false,
                            gap,
                        );
                    }
                }
            }
            // ---- Click-to-log: record the clicked element + its box hierarchy.
            if ctx.input(|i| i.pointer.primary_clicked()) {
                if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                    let mut under: Vec<&(String, egui::Rect)> =
                        buf.iter().filter(|(_, r)| r.contains(p)).collect();
                    if !under.is_empty() {
                        // outer → inner (largest area first); last = the smallest,
                        // i.e. the element actually clicked.
                        under.sort_by(|a, b| {
                            (b.1.width() * b.1.height())
                                .partial_cmp(&(a.1.width() * a.1.height()))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let (tn, tr) = under.last().unwrap();
                        self.ui_inspect_log.push(format!(
                            "═══ CLICK: {}   →   {:.0}×{:.0}  @({:.0}, {:.0})",
                            tn,
                            tr.width(),
                            tr.height(),
                            tr.left(),
                            tr.top()
                        ));
                        self.ui_inspect_log
                            .push("    box hierarchy (outer → inner):".to_string());
                        for (n, r) in &under {
                            self.ui_inspect_log.push(format!(
                                "      {:<30} w={:6.1} h={:5.1}   x={:7.1} y={:7.1}",
                                n,
                                r.width(),
                                r.height(),
                                r.left(),
                                r.top()
                            ));
                        }
                        self.ui_inspect_log.push(String::new());
                    }
                }
            }
            ctx.request_repaint();
        }

        // ---- UI Inspect Log window (copyable report) ----------------------
        if self.ui_inspect {
            // The title-bar × closes the whole UI-inspect tool (overlay + window);
            // re-enable via Tools ▸ Debug ▸ "UI inspect". `open` is a local so the
            // window's &mut borrow doesn't clash with the body closure's &mut self.
            let mut open = true;
            egui::Window::new("UI Inspect Log")
                .open(&mut open)
                .default_width(560.0)
                .default_pos(egui::pos2(60.0, 90.0))
                .show(ctx, |ui| {
                    let full = self.ui_inspect_log.join("\n");
                    ui.horizontal(|ui| {
                        if ui.button("📋 Copy dump").clicked() {
                            ui.output_mut(|o| o.copied_text = full.clone());
                        }
                        if ui.button("Clear").clicked() {
                            self.ui_inspect_log.clear();
                        }
                        ui.label(format!(
                            "{} lines — click an element, Copy, paste to me",
                            self.ui_inspect_log.len()
                        ));
                    });
                    let mut text = full;
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut text)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_rows(14)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                });
            // Closing the window turns the inspector off entirely.
            if !open {
                self.ui_inspect = false;
            }
        }

        // ---- Command registry dump (Phase-2 verification, temporary) ------
        // Proof the registry populated from DRAW_CMDS/MODIFY_CMDS. Nothing else
        // reads the registry yet (rails still use the arrays until Phase 3).
        if self.cmd_dump_open {
            let mut entries: Vec<&crate::command::CommandInfo> =
                self.command_registry.commands.values().collect();
            // HashMap is unordered — sort for a stable dump (category, then id).
            entries.sort_by(|a, b| (a.category as u8, &a.id).cmp(&(b.category as u8, &b.id)));
            let mut text = format!("Command registry — {} entries\n\n", entries.len());
            text.push_str(&format!(
                "{:<16} {:<14} {:<18} {:<7} {:<22} {:<8} keywords\n",
                "id", "dispatch", "title", "cat", "icon", "section"
            ));
            for e in &entries {
                let cat = match e.category {
                    crate::command::CommandCategory::Draw => "Draw",
                    crate::command::CommandCategory::Modify => "Modify",
                };
                text.push_str(&format!(
                    "{:<16} {:<14} {:<18} {:<7} {:<22} {:<8} [{}]\n",
                    e.id,
                    e.dispatch,
                    e.title,
                    cat,
                    format!("{:?}", e.icon),
                    e.section.unwrap_or("—"),
                    e.keywords.join(", ")
                ));
            }
            let mut open = true;
            egui::Window::new("Command registry dump")
                .open(&mut open)
                .default_width(600.0)
                .default_pos(egui::pos2(90.0, 120.0))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("📋 Copy").clicked() {
                            ui.output_mut(|o| o.copied_text = text.clone());
                        }
                        ui.label(format!("{} commands (Draw + Modify)", entries.len()));
                    });
                    let mut body = text.clone();
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut body)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_rows(20)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                });
            if !open {
                self.cmd_dump_open = false;
            }
        }

        if self.props_layout_capture {
            let buf: Vec<(String, egui::Rect)> =
                ctx.data(|d| d.get_temp(egui::Id::new("pp_cap_buf")).unwrap_or_default());
            let fmt = |name: &str, r: egui::Rect| {
                format!(
                    "{:<32} x={:7.1} y={:7.1}  w={:7.1} h={:6.1}",
                    name,
                    r.left(),
                    r.top(),
                    r.width(),
                    r.height()
                )
            };
            let mut elements: Vec<String> = buf.iter().map(|(n, r)| fmt(n, *r)).collect();
            let n = elements.len();
            if n == 0 {
                elements.push(
                    "(no instrumented menu was open — open a menu first, then capture)".into(),
                );
            }
            crate::dbg_event!(
                self,
                crate::dbg_recorder::DbgEvent::MenuLayout {
                    label: "menus".into(),
                    elements
                }
            );
            self.props_layout_capture = false;
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("pp_cap_on"), false));
            self.history.push(format!(
                "  📐 menu layout captured ({} elements) — see recorder timeline",
                n
            ));
        }

        self.sweep_stale_prompt();

        // ONE DRAIN FOR EVERY PATH. A fixture edit made inside a panel closure leaves the copy it
        // overwrote staged; the panels that know about it drain their own, and this catches the
        // rest — importing a photometric file assigns it to the points already marked out, and
        // that is an edit nobody would think to wire up.
        //
        // NOT while a drag is in flight: `begin_drag` stages on the press and the step belongs to
        // the whole gesture, so pushing it now would spend the copy and leave the release with
        // nothing to commit.
        if self.light.drag.is_none() {
            self.commit_light_undo();
        }
    }
}

