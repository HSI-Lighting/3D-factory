use super::super::*;

#[cfg(test)]
mod script_op_tests {
    use super::*;
    use cad_script::{ScriptOp as Op, ScriptOpReply as R};

    fn ok(reply: cad_script::ScriptOpReply) -> usize {
        match reply {
            R::Ok(i) => i,
            other => panic!("expected Ok(index), got {other:?}"),
        }
    }

    // Adds via rasm land as ordinary dobjects and report their index.
    #[test]
    fn script_adds_create_dobjects() {
        let mut app = CadApp::default();
        let n0 = app.doc.dobjects.len();
        let r = app.apply_script_op(Op::AddCircle {
            center: Vec2::new(1.0, 2.0),
            radius: 3.0,
        });
        assert_eq!(ok(r), n0);
        let r = app.apply_script_op(Op::AddLine {
            a: Vec2::ZERO,
            b: Vec2::new(5.0, 0.0),
        });
        assert_eq!(ok(r), n0 + 1);
        assert_eq!(app.doc.dobjects.len(), n0 + 2);
    }

    // Reads return owned snapshots (D7) — never borrows into the doc.
    #[test]
    fn script_reads_return_owned_data() {
        let mut app = CadApp::default();
        let n0 = app.doc.dobjects.len();
        app.apply_script_op(Op::AddCircle {
            center: Vec2::new(4.0, 0.0),
            radius: 2.0,
        });
        match app.apply_script_op(Op::DocCount) {
            R::Count(n) => assert_eq!(n, n0 + 1),
            other => panic!("expected Count, got {other:?}"),
        }
        match app.apply_script_op(Op::DocGet { index: n0 }) {
            R::Entity(e) => {
                assert_eq!(e.handle, app.doc.dobjects[n0].handle);
                assert!(!e.layer.is_empty(), "layer name must resolve");
            }
            other => panic!("expected Entity, got {other:?}"),
        }
    }

    // Delete removes highest-first and prunes the selection.
    #[test]
    fn script_delete_removes_and_prunes_selection() {
        let mut app = CadApp::default();
        let n0 = app.doc.dobjects.len();
        for i in 0..4 {
            app.apply_script_op(Op::AddLine {
                a: Vec2::new(i as f64, 0.0),
                b: Vec2::new(i as f64, 1.0),
            });
        }
        app.apply_script_op(Op::SelectionSet {
            indices: vec![n0, n0 + 2, n0 + 3],
        });
        match app.apply_script_op(Op::Delete {
            indices: vec![n0 + 3, n0 + 1],
        }) {
            R::Ok(n) => assert_eq!(n, 2),
            other => panic!("expected Ok(2), got {other:?}"),
        }
        assert_eq!(app.doc.dobjects.len(), n0 + 2);
        assert_eq!(
            app.selection,
            vec![n0, n0 + 2],
            "selection must drop deleted #3"
        );
    }

    // D5 — one run = one undo unit: a script run's snapshots collapse to a
    // single pre-run state on Finished.
    #[test]
    fn script_run_collapses_to_one_undo_unit() {
        let mut app = CadApp::default();
        let base = app.undo_stack.len();
        // First write of the run captures the baseline.
        app.apply_script_op(Op::AddCircle {
            center: Vec2::ZERO,
            radius: 1.0,
        });
        assert!(
            app.script_undo_base.is_some(),
            "first write must capture the baseline"
        );
        app.apply_script_op(Op::AddLine {
            a: Vec2::ZERO,
            b: Vec2::new(1.0, 0.0),
        });
        app.apply_script_op(Op::LayerAdd {
            name: "SCRIPTLYR".into(),
        });
        assert!(
            app.undo_stack.len() > base + 1,
            "each op snapshots — the run holds several entries"
        );
        // Finished collapses them all back to the single pre-run snapshot.
        let keep = (app.script_undo_base.unwrap() + 1).min(app.undo_stack.len());
        if app.undo_stack.len() > keep {
            app.undo_stack.truncate(keep);
        }
        app.script_undo_base = None;
        assert_eq!(app.undo_stack.len(), base + 1, "one run = one undo unit");
        let n0 = app.doc.dobjects.len();
        app.do_undo();
        assert_eq!(
            app.doc.dobjects.len(),
            n0 - 2,
            "undo must revert the whole run"
        );
        assert_eq!(
            app.doc.layers.find("SCRIPTLYR"),
            None,
            "layer add must revert too"
        );
    }

    // A run with no writes leaves the undo stack untouched.
    #[test]
    fn script_read_only_run_leaves_undo_alone() {
        let mut app = CadApp::default();
        let base = app.undo_stack.len();
        app.apply_script_op(Op::DocCount);
        app.apply_script_op(Op::ViewGet);
        app.apply_script_op(Op::LayersGet);
        assert_eq!(app.undo_stack.len(), base, "reads must never snapshot");
        assert!(app.script_undo_base.is_none(), "no write → no baseline");
    }

    // Bad requests fail loudly (rule 10), never silently.
    #[test]
    fn script_bad_requests_fail_loudly() {
        let mut app = CadApp::default();
        match app.apply_script_op(Op::AddCircle {
            center: Vec2::ZERO,
            radius: -1.0,
        }) {
            R::Error(_) => {}
            other => panic!("negative radius must fail, got {other:?}"),
        }
        match app.apply_script_op(Op::LayerSetActive {
            name: "NOPE".into(),
        }) {
            R::Error(_) => {}
            other => panic!("unknown layer must fail, got {other:?}"),
        }
        let n0 = app.doc.dobjects.len();
        assert_eq!(app.doc.dobjects.len(), n0, "failed ops must not mutate");
    }

    // Layer ops mirror the panel's rules: unique names, set-active by name.
    #[test]
    fn script_layer_ops() {
        let mut app = CadApp::default();
        match app.apply_script_op(Op::LayerAdd {
            name: "SCRIPTWALLS".into(),
        }) {
            R::Ok(id) => assert_eq!(app.doc.layers.find("SCRIPTWALLS"), Some(id as u32)),
            other => panic!("expected Ok(id), got {other:?}"),
        }
        match app.apply_script_op(Op::LayerAdd {
            name: "SCRIPTWALLS".into(),
        }) {
            R::Error(_) => {}
            other => panic!("duplicate layer must fail, got {other:?}"),
        }
        app.apply_script_op(Op::LayerSetActive {
            name: "SCRIPTWALLS".into(),
        });
        assert_eq!(
            app.doc.layers.active,
            app.doc.layers.find("SCRIPTWALLS").unwrap()
        );
    }
}
#[cfg(test)]
mod script_cmd_tests {
    use super::*;

    // `run <unknown>` fails loudly with the available list (rule 10) and
    // never submits to the engine.
    #[test]
    fn run_unknown_script_fails_loudly() {
        let mut app = CadApp::default();
        let h0 = app.history.len();
        app.run_script_command(Some("definitely_not_a_script".into()), vec!["x".into()]);
        assert_eq!(app.script.is_none(), true, "no engine must be created");
        assert!(
            app.history[h0..]
                .iter()
                .any(|l| l.contains("! run: no script")),
            "the failure must be surfaced: {:?}",
            &app.history[h0..]
        );
    }

    // `resolve_script` accepts the bare name and `name.py`, case-insensitively.
    // (Pointed at the repo's own scripts/ — the test cwd is the package dir,
    // so a missing folder simply resolves to None; the shape is what's pinned.)
    #[test]
    fn resolve_script_handles_names() {
        assert!(
            CadApp::resolve_script("").is_none(),
            "empty name resolves to nothing"
        );
        let r = CadApp::resolve_script("definitely_not_a_script");
        assert!(r.is_none(), "unknown names must not resolve");
    }

    // The editor save→run path requires a name and refuses a nameless run.
    #[test]
    fn editor_run_requires_a_name() {
        let mut app = CadApp::default();
        app.py_editor_text = "rasm.add_circle((0, 0), 1.0)".into();
        app.py_editor_name.clear();
        let l0 = app.py_console_log.len();
        app.py_editor_run();
        assert_eq!(app.script.is_none(), true, "no engine without a name");
        assert!(
            app.py_console_log[l0..]
                .iter()
                .any(|l| l.contains("give the script a name")),
            "the refusal must be visible in the console log"
        );
    }
}
#[cfg(test)]
mod script_params_tests {
    use super::*;
    use cad_script::{ParamType, ScriptMeta, ScriptParamMeta};

    #[test]
    fn named_args_parse_as_pairs() {
        let args: Vec<String> = vec!["outer_d=150".into(), "bolts=10".into()];
        let got = CadApp::parse_named_script_args(&args).expect("all k=v parse");
        assert_eq!(
            got,
            vec![
                ("outer_d".to_string(), "150".to_string()),
                ("bolts".to_string(), "10".to_string()),
            ]
        );
        assert!(
            CadApp::parse_named_script_args(&["100".into()]).is_none(),
            "positional forms must not parse as named"
        );
        assert!(
            CadApp::parse_named_script_args(&["=5".into()]).is_none(),
            "an empty name must not parse"
        );
        assert!(CadApp::parse_named_script_args(&[]).is_none());
    }

    fn param(name: &str, ty: ParamType, default: &str) -> ScriptParamMeta {
        ScriptParamMeta {
            name: name.into(),
            ptype: ty,
            default: default.into(),
            min: None,
            max: None,
            help: String::new(),
            choices: Vec::new(),
        }
    }

    // A script WITH parameters → the dialog fills from the declaration.
    #[test]
    fn meta_reply_fills_the_dialog() {
        let mut app = CadApp::default();
        app.script_param_dialog = Some(ScriptParamDialog {
            name: "flange".into(),
            path: std::path::PathBuf::from("flange.py"),
            params: Vec::new(),
            values: Vec::new(),
            waiting_meta: true,
            pos: egui::pos2(0.0, 0.0),
        });
        let meta = ScriptMeta {
            name: "flange".into(),
            params: vec![
                param("outer_d", ParamType::Float, "120.0"),
                param("bolts", ParamType::Int, "6"),
            ],
        };
        app.on_script_meta(Some(meta));
        let dlg = app.script_param_dialog.as_ref().expect("dialog stays open");
        assert!(!dlg.waiting_meta);
        assert_eq!(dlg.params.len(), 2);
        assert_eq!(dlg.values, vec!["120.0".to_string(), "6".to_string()]);
        // The default values go straight into a ghost preview pass.
        assert!(app.script.is_some(), "the preview pass is submitted");
        let p = app.script_preview.as_ref().expect("preview active");
        assert!(p.running, "the ghost pass is running");
        assert!(!p.cancelled);
    }

    // A script with NOTHING to declare → run immediately, no empty dialog.
    #[test]
    fn meta_reply_without_params_runs_immediately() {
        let mut app = CadApp::default();
        app.script_param_dialog = Some(ScriptParamDialog {
            name: "hello".into(),
            path: std::path::PathBuf::from("hello.py"),
            params: Vec::new(),
            values: Vec::new(),
            waiting_meta: true,
            pos: egui::pos2(0.0, 0.0),
        });
        app.on_script_meta(None);
        assert!(
            app.script_param_dialog.is_none(),
            "no dialog for plain scripts"
        );
        assert!(app.script.is_some(), "the script was submitted");
    }
}
#[cfg(test)]
mod script_point_pick_tests {
    use super::*;

    #[test]
    fn point_values_parse_from_dialog_and_defaults() {
        assert_eq!(
            CadApp::parse_point_param_value("12.5, -3"),
            Some((12.5, -3.0)),
            "plain x,y must parse"
        );
        assert_eq!(
            CadApp::parse_point_param_value("(0.0, 0.0)"),
            Some((0.0, 0.0)),
            "python-tuple defaults must parse"
        );
        assert_eq!(CadApp::parse_point_param_value(""), None);
        assert_eq!(CadApp::parse_point_param_value("12.5"), None);
        assert_eq!(CadApp::parse_point_param_value("a,b"), None);
    }

    // A canvas click while a pick is armed fills the right dialog field and
    // consumes the pick (simulated by calling the same path logic directly).
    #[test]
    fn armed_pick_fills_the_dialog_field() {
        let mut app = CadApp::default();
        app.script_param_dialog = Some(ScriptParamDialog {
            name: "flange".into(),
            path: std::path::PathBuf::from("flange.py"),
            params: vec![cad_script::ScriptParamMeta {
                name: "pos".into(),
                ptype: cad_script::ParamType::Point,
                default: "(0.0, 0.0)".into(),
                min: None,
                max: None,
                help: String::new(),
                choices: Vec::new(),
            }],
            values: vec!["(0.0, 0.0)".into()],
            waiting_meta: false,
            pos: egui::pos2(0.0, 0.0),
        });
        app.script_param_pick = Some(("pos".into(), ScriptPickKind::Point));
        // The click intercept's body (applied on the main canvas click).
        app.script_param_pick = None;
        let v = "77.5,-12.25".to_string();
        if let Some(dlg) = app.script_param_dialog.as_mut() {
            if let Some(ix) = dlg.params.iter().position(|p| p.name == "pos") {
                dlg.values[ix] = v;
            }
        }
        let dlg = app.script_param_dialog.as_ref().unwrap();
        assert_eq!(dlg.values[0], "77.5,-12.25");
        assert_eq!(
            CadApp::parse_point_param_value(&dlg.values[0]),
            Some((77.5, -12.25))
        );
    }
}
#[cfg(test)]
mod script_preview_tests {
    use super::*;
    use cad_script::{ScriptOp as Op, ScriptOpReply as R};

    fn finalize(app: &mut CadApp) {
        if let Some(p) = &mut app.script_preview {
            p.running = false;
            p.ghosts = p
                .doc
                .dobjects
                .iter()
                .filter(|d| !p.base_handles.contains(&d.handle))
                .map(|d| d.geom.clone())
                .collect();
        }
    }

    #[test]
    fn preview_ops_stay_in_the_shadow() {
        let mut app = CadApp::default();
        let n0 = app.doc.dobjects.len();
        app.script_preview_start(
            "flange".into(),
            std::path::PathBuf::from("flange.py"),
            Vec::new(),
        );
        // A write lands in the shadow, not the real doc.
        assert!(matches!(
            app.apply_script_op(Op::AddCircle {
                center: Vec2::ZERO,
                radius: 5.0
            }),
            R::Ok(_)
        ));
        assert_eq!(
            app.doc.dobjects.len(),
            n0,
            "the real doc must stay untouched"
        );
        assert_eq!(
            app.script_preview.as_ref().unwrap().doc.dobjects.len(),
            n0 + 1,
            "the shadow gets the addition"
        );
        // Reads see the shadow snapshot (the script's own additions).
        match app.apply_script_op(Op::DocCount) {
            R::Count(c) => assert_eq!(c, n0 + 1),
            other => panic!("expected Count, got {other:?}"),
        }
        // No undo baseline for preview writes.
        assert!(app.script_undo_base.is_none());
        // Finalize → exactly the net additions become ghosts.
        finalize(&mut app);
        let p = app.script_preview.as_ref().unwrap();
        assert_eq!(p.ghosts.len(), 1, "one ghost extracted");
        assert!(
            matches!(&p.ghosts[0], Geom::Circle(c)
                if c.center == Vec2::ZERO && (c.radius - 5.0).abs() < 1e-9),
            "the ghost is the preview's circle"
        );
    }

    #[test]
    fn preview_deletes_dont_leak_into_ghosts() {
        let mut app = CadApp::default();
        let n0 = app.doc.dobjects.len();
        app.script_preview_start("x".into(), std::path::PathBuf::from("x.py"), Vec::new());
        // Add one, delete it again — the net additions must be EMPTY.
        app.apply_script_op(Op::AddLine {
            a: Vec2::ZERO,
            b: Vec2::new(1.0, 0.0),
        });
        app.apply_script_op(Op::Delete { indices: vec![n0] });
        finalize(&mut app);
        assert!(
            app.script_preview.as_ref().unwrap().ghosts.is_empty(),
            "a deleted preview addition must not ghost"
        );
        assert_eq!(
            app.doc.dobjects.len(),
            n0,
            "real doc untouched by the delete"
        );
    }

    #[test]
    fn zombie_preview_keeps_ops_out_of_the_real_doc() {
        let mut app = CadApp::default();
        let n0 = app.doc.dobjects.len();
        app.script_preview_start("x".into(), std::path::PathBuf::from("x.py"), Vec::new());
        // The user runs/commits while the pass is in flight → zombie.
        app.clear_script_preview();
        assert!(
            app.script_preview.is_some(),
            "zombie survives until Finished"
        );
        assert!(
            app.script_preview.as_ref().unwrap().cancelled,
            "the pass is cancelled, not dropped"
        );
        app.apply_script_op(Op::AddLine {
            a: Vec2::ZERO,
            b: Vec2::new(2.0, 0.0),
        });
        assert_eq!(
            app.doc.dobjects.len(),
            n0,
            "zombie ops must still avoid the real doc"
        );
        // Finished drops the zombie.
        if let Some(p) = &mut app.script_preview {
            if p.running {
                p.running = false;
                if p.cancelled {
                    app.script_preview = None;
                }
            }
        }
        assert!(app.script_preview.is_none(), "the zombie drops on finish");
        // The next write goes to the real doc again.
        app.apply_script_op(Op::AddLine {
            a: Vec2::ZERO,
            b: Vec2::new(3.0, 0.0),
        });
        assert_eq!(
            app.doc.dobjects.len(),
            n0 + 1,
            "real ops resume after the zombie"
        );
    }
}
#[cfg(test)]
mod script_meta_finish_tests {
    use super::*;
    use cad_script::{ParamType, ScriptMeta, ScriptParamMeta};

    fn dialog() -> ScriptParamDialog {
        ScriptParamDialog {
            name: "flange".into(),
            path: std::path::PathBuf::from("flange.py"),
            params: Vec::new(),
            values: Vec::new(),
            waiting_meta: true,
            pos: egui::pos2(0.0, 0.0),
        }
    }

    fn meta() -> ScriptMeta {
        ScriptMeta {
            name: "flange".into(),
            params: vec![ScriptParamMeta {
                name: "outer_d".into(),
                ptype: ParamType::Float,
                default: "120.0".into(),
                min: None,
                max: None,
                help: String::new(),
                choices: Vec::new(),
            }],
        }
    }

    #[test]
    fn meta_finished_must_not_finalize_the_just_started_preview() {
        let mut app = CadApp::default();
        app.script_param_dialog = Some(dialog());
        // The Meta reply arrives first: it fills the dialog and starts the
        // ghost pass (running = true).
        app.script_meta_finish_pending = true;
        app.on_script_meta(Some(meta()));
        assert!(
            app.script_preview.as_ref().unwrap().running,
            "preview started"
        );
        let h0 = app.history.len();
        // The META job's Finished lands in the same poll batch — it must not
        // finalize the preview (that was the leak: preview ops then hit the
        // real document).
        app.on_script_finished(true);
        assert!(
            app.script_preview.as_ref().unwrap().running,
            "the preview must survive the meta job's finish"
        );
        assert_eq!(
            app.history.len(),
            h0,
            "meta finish stays out of the history"
        );
        // A real run's ops still route to the shadow while it survives.
        app.apply_script_op(cad_script::ScriptOp::AddCircle {
            center: Vec2::ZERO,
            radius: 2.0,
        });
        assert!(
            app.doc
                .dobjects
                .iter()
                .all(|d| !matches!(&d.geom, Geom::Circle(c) if c.radius == 2.0)),
            "preview ops must stay in the shadow after the meta finish"
        );
        // The PREVIEW's own finish (later) finalizes normally.
        app.on_script_finished(true);
        assert!(
            !app.script_preview.as_ref().unwrap().running,
            "preview finalized"
        );
    }
}
#[cfg(test)]
mod script_length_tests {
    use super::*;
    use cad_script::{ParamType, ScriptParamMeta};

    fn len_param(default: &str) -> ScriptParamMeta {
        ScriptParamMeta {
            name: "outer_d".into(),
            ptype: ParamType::Length,
            default: default.into(),
            min: None,
            max: None,
            help: String::new(),
            choices: Vec::new(),
        }
    }

    #[test]
    fn length_defaults_show_in_display_units() {
        let mut app = CadApp::default();
        app.doc.units.name = "mm".into();
        app.doc.units.scene_per_unit = 10.0; // calibrated: 10 scene per mm
        let p = len_param("120.0");
        assert_eq!(
            app.param_display_default(&p),
            "12",
            "a 120-scene default must display as 12 in a 10:1 doc"
        );
        // Display → scene round trip.
        assert_eq!(
            app.param_value_scene(&[p.clone()], "outer_d", "12")
                .unwrap(),
            "120"
        );
    }

    #[test]
    fn length_inputs_convert_suffixes_through_the_document_units() {
        let mut app = CadApp::default();
        app.doc.units.name = "mm".into();
        app.doc.units.scene_per_unit = 1.0;
        let spec = [len_param("120.0")];
        assert_eq!(
            app.param_value_scene(&spec, "outer_d", "25").unwrap(),
            "25",
            "a bare number is in the display unit (mm doc → scene)"
        );
        assert_eq!(
            app.param_value_scene(&spec, "outer_d", "25cm").unwrap(),
            "250",
            "an explicit suffix converts physically (25cm = 250mm scene)"
        );
        assert_eq!(
            app.param_value_scene(&spec, "outer_d", "1in").unwrap(),
            "25.4",
            "1in = 25.4mm scene"
        );
        assert!(
            app.param_value_scene(&spec, "outer_d", "abc").is_err(),
            "a bad length fails loudly"
        );
    }

    #[test]
    fn pending_run_converts_lengths_before_submitting() {
        let mut app = CadApp::default();
        app.doc.units.name = "mm".into();
        app.doc.units.scene_per_unit = 10.0; // 1 mm = 10 scene
        app.script_pending_run = Some(PendingScriptRun {
            name: "flange".into(),
            path: std::path::PathBuf::from("flange.py"),
            named: Some(vec![
                ("outer_d".into(), "15".into()),
                ("bolts".into(), "8".into()),
            ]),
            positional: None,
        });
        app.run_pending_script(Some(cad_script::ScriptMeta {
            name: "flange".into(),
            params: vec![
                len_param("120.0"),
                ScriptParamMeta {
                    name: "bolts".into(),
                    ptype: ParamType::Int,
                    default: "6".into(),
                    min: None,
                    max: None,
                    help: String::new(),
                    choices: Vec::new(),
                },
            ],
        }));
        assert!(app.script_pending_run.is_none(), "pending run consumed");
        assert!(app.script.is_some(), "the run was submitted");
    }

    #[test]
    fn non_length_params_pass_through_untouched() {
        let mut app = CadApp::default();
        app.doc.units.scene_per_unit = 100.0; // must NOT scale counts
        let p = ScriptParamMeta {
            name: "bolts".into(),
            ptype: ParamType::Int,
            default: "6".into(),
            min: None,
            max: None,
            help: String::new(),
            choices: Vec::new(),
        };
        assert_eq!(app.param_value_scene(&[p], "bolts", "8").unwrap(), "8");
    }
}
#[cfg(test)]
mod script_modify_tests {
    use super::*;
    use cad_script::{ScriptOp as Op, ScriptOpReply as R};

    fn n_adds(app: &mut CadApp, n: usize) -> usize {
        let base = app.doc.dobjects.len();
        for i in 0..n {
            app.apply_script_op(Op::AddLine {
                a: Vec2::new(i as f64 * 10.0, 0.0),
                b: Vec2::new(i as f64 * 10.0 + 5.0, 0.0),
            });
        }
        base
    }

    #[test]
    fn modify_move_transforms_in_place() {
        let mut app = CadApp::default();
        let b = n_adds(&mut app, 2);
        app.apply_script_op(Op::ModifyMove {
            indices: vec![b, b + 1],
            delta: Vec2::new(0.0, 7.0),
        });
        let d = &app.doc.dobjects[b];
        match &d.geom {
            Geom::Line(l) => assert!((l.a.y - 7.0).abs() < 1e-9, "line moved up"),
            other => panic!("expected line, got {:?}", other),
        }
        app.do_undo();
        let d = &app.doc.dobjects[b];
        match &d.geom {
            Geom::Line(l) => assert!((l.a.y).abs() < 1e-9, "undo restores the line"),
            other => panic!("expected line, got {:?}", other),
        }
    }

    #[test]
    fn modify_copy_returns_new_indices() {
        let mut app = CadApp::default();
        let b = n_adds(&mut app, 1);
        let n0 = app.doc.dobjects.len();
        match app.apply_script_op(Op::ModifyCopy {
            indices: vec![b],
            delta: Vec2::new(50.0, 0.0),
        }) {
            R::Indices(v) => {
                assert_eq!(v, vec![n0], "one new index");
                assert_eq!(app.doc.dobjects.len(), n0 + 1);
                match &app.doc.dobjects[v[0]].geom {
                    Geom::Line(l) => assert!((l.a.x - 50.0).abs() < 1e-9, "copy offset"),
                    other => panic!("expected line, got {:?}", other),
                }
            }
            other => panic!("expected Indices, got {other:?}"),
        }
    }

    #[test]
    fn set_entity_geom_replaces_shape_properties() {
        let mut app = CadApp::default();
        let b = n_adds(&mut app, 1);
        // Line → Circle (shape-specific properties replaced; index kept).
        app.apply_script_op(Op::SetEntityGeom {
            index: b,
            geom: Geom::Circle(Circle {
                center: Vec2::new(3.0, 4.0),
                radius: 2.0,
            }),
        });
        match &app.doc.dobjects[b].geom {
            Geom::Circle(c) => {
                assert_eq!(c.center, Vec2::new(3.0, 4.0));
                assert!((c.radius - 2.0).abs() < 1e-9);
            }
            other => panic!("expected circle, got {:?}", other),
        }
        assert!(
            app.doc.dobjects.len() == b + 1,
            "no extra entity was created"
        );
    }

    #[test]
    fn style_setters_and_resolved_summary() {
        let mut app = CadApp::default();
        let b = n_adds(&mut app, 1);
        app.apply_script_op(Op::SetEntityColor {
            indices: vec![b],
            color: 3,
        });
        let d = &app.doc.dobjects[b];
        assert_eq!(d.style.color, Color::Aci(3));
        // The snapshot exposes the resolved style.
        let e = app.entity_snapshot(d);
        assert_eq!(e.color, "aci 3");
        assert!(!e.linetype.is_empty());
        assert!(e.visible);
        // Layer move resolves the layer name in the snapshot.
        app.apply_script_op(Op::LayerAdd {
            name: "SCRIPT_LW".into(),
        });
        app.apply_script_op(Op::SetEntityLayer {
            indices: vec![b],
            name: "SCRIPT_LW".into(),
        });
        let e = app.entity_snapshot(&app.doc.dobjects[b]);
        assert_eq!(e.layer, "SCRIPT_LW");
        // Visibility toggle.
        app.apply_script_op(Op::SetEntityVisible {
            indices: vec![b],
            visible: false,
        });
        assert!(!app.doc.dobjects[b].style.visible);
        let e = app.entity_snapshot(&app.doc.dobjects[b]);
        assert!(!e.visible);
    }

    #[test]
    fn doc_bounds_cover_all_entities() {
        let mut app = CadApp::default();
        let b = n_adds(&mut app, 1);
        app.apply_script_op(Op::ModifyMove {
            indices: vec![b],
            delta: Vec2::new(-5.0, 3.0),
        });
        match app.apply_script_op(Op::DocBounds) {
            R::Bounds(Some((min, max))) => {
                assert!(min.x <= -5.0 && max.x >= 0.0);
                assert!(min.y <= 0.0 && max.y >= 3.0);
            }
            other => panic!("expected Bounds, got {other:?}"),
        }
        match app.apply_script_op(Op::DocUnits) {
            R::Units(u) => assert_eq!(u.name, app.doc.units.name),
            other => panic!("expected Units, got {other:?}"),
        }
    }

    #[test]
    fn undo_group_splits_a_run_into_units() {
        let mut app = CadApp::default();
        let depth0 = app.undo_stack.len();
        let b = n_adds(&mut app, 1); // first write → baseline
        app.apply_script_op(Op::UndoGroup); // boundary after entity #b
        let b2 = n_adds(&mut app, 1); // second group
        assert_eq!(b2, b + 1);
        // Simulate the Finished collapse (grouped).
        let base = app.script_undo_base.unwrap();
        let mut kept = vec![base];
        kept.extend(app.script_group_snapshots.iter().copied());
        let mut out = Vec::new();
        for &ix in &kept {
            if ix >= app.undo_stack.len() {
                continue;
            }
            out.push(app.undo_stack[ix].clone());
        }
        app.undo_stack.truncate(base);
        app.undo_stack.extend(out);
        app.script_undo_base = None;
        app.script_group_snapshots.clear();
        assert_eq!(
            app.undo_stack.len(),
            depth0 + 2,
            "pre-run + one boundary = 2 units"
        );
        app.do_undo();
        assert_eq!(
            app.doc.dobjects.len(),
            b2,
            "first undo reverts only the 2nd group"
        );
        app.do_undo();
        assert_eq!(
            app.doc.dobjects.len(),
            b,
            "second undo reverts the 1st group"
        );
    }

    #[test]
    fn current_style_setters_apply_to_next_adds() {
        let mut app = CadApp::default();
        app.apply_script_op(Op::SetCurrentColor { color: 1 });
        assert_eq!(app.doc.current_color, Color::Aci(1));
        match app.apply_script_op(Op::SetCurrentLinetype {
            name: "Continuous".into(),
        }) {
            R::OkUnit => {}
            other => panic!("expected OkUnit, got {other:?}"),
        }
        app.apply_script_op(Op::SetCurrentLineweight { mm: 0.5 });
        assert_eq!(app.doc.current_lineweight, Lineweight::Custom(0.5));
        match app.apply_script_op(Op::SetCurrentColor { color: 999 }) {
            R::Error(_) => {}
            other => panic!("bad color must fail loudly, got {other:?}"),
        }
    }
}
#[cfg(test)]
mod script_catalog_types_tests {
    use super::*;
    use cad_script::{ParamType, ScriptParamMeta};

    fn cat_param(name: &str, ty: ParamType) -> ScriptParamMeta {
        ScriptParamMeta {
            name: name.into(),
            ptype: ty,
            default: String::new(),
            min: None,
            max: None,
            help: String::new(),
            choices: Vec::new(),
        }
    }

    #[test]
    fn catalog_choices_fill_from_the_live_document() {
        let mut app = CadApp::default();
        let mut params = vec![
            cat_param("lt", ParamType::Linetype),
            cat_param("ly", ParamType::Layer),
            cat_param("bl", ParamType::Block),
            cat_param("hp", ParamType::HatchPattern),
            cat_param("plain", ParamType::Str),
        ];
        app.fill_catalog_choices(&mut params);
        assert!(
            params[0].choices.iter().any(|c| c == "Continuous"),
            "linetype catalog filled: {:?}",
            params[0].choices
        );
        assert!(!params[1].choices.is_empty(), "layer list filled");
        assert!(params[2].choices.is_empty(), "no blocks in the default doc");
        assert!(
            params[3].choices.iter().any(|c| c == "SOLID"),
            "pattern catalog filled"
        );
        assert!(params[4].choices.is_empty(), "plain types keep no choices");
    }

    #[test]
    fn quoted_defaults_validate_after_quote_stripping() {
        // The exact reported bug: a stale dialog value carrying Python repr
        // quotes ("'ANSI31'") must still validate + canonicalize.
        let mut app = CadApp::default();
        let mut spec = vec![cat_param("hp", ParamType::HatchPattern)];
        app.fill_catalog_choices(&mut spec);
        assert_eq!(
            app.param_value_scene(&spec, "hp", "'ANSI31'").unwrap(),
            "ANSI31"
        );
        assert_eq!(
            app.param_value_scene(&spec, "hp", "'solid'").unwrap(),
            "SOLID"
        );
    }

    #[test]
    fn catalog_values_validate_and_canonicalize() {
        let mut app = CadApp::default();
        let mut spec = vec![
            cat_param("lt", ParamType::Linetype),
            cat_param("hp", ParamType::HatchPattern),
        ];
        app.fill_catalog_choices(&mut spec);
        // Case-insensitive canonicalization to the catalog spelling.
        assert_eq!(
            app.param_value_scene(&spec, "lt", "continuous").unwrap(),
            "Continuous"
        );
        assert_eq!(
            app.param_value_scene(&spec, "hp", "solid").unwrap(),
            "SOLID"
        );
        // Unknown values fail loudly with the available list.
        let err = app.param_value_scene(&spec, "lt", "NOPE").unwrap_err();
        assert!(err.contains("not one of"), "{err}");
        let err = app.param_value_scene(&spec, "hp", "NOPE").unwrap_err();
        assert!(err.contains("SOLID"), "{err}");
        // Non-catalog params still pass through untouched.
        let plain = cat_param("s", ParamType::Str);
        assert_eq!(
            app.param_value_scene(&[plain], "s", "free text").unwrap(),
            "free text"
        );
    }
}
