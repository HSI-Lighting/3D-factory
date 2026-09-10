use super::super::*;

#[cfg(test)]
mod autosave_tests {
    use super::*;

    /// Atomic write replaces the target and leaves no temp file behind — the property that keeps
    /// an interrupted autosave from corrupting the drawing.
    #[test]
    fn atomic_write_replaces_and_cleans_up() {
        let p = std::env::temp_dir().join(format!("rustcad_atomic_{}.bin", std::process::id()));
        let ps = p.to_string_lossy().to_string();
        atomic_write(&ps, b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        atomic_write(&ps, b"world!!").unwrap(); // replace an existing file
        assert_eq!(std::fs::read(&p).unwrap(), b"world!!");
        // Temps are uniquely numbered per write (concurrent workers share the path);
        // after a successful rename none of them may remain.
        let leftovers = std::fs::read_dir(p.parent().unwrap())
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        e.file_name().to_string_lossy().starts_with(&format!(
                            "{}.savetmp.",
                            p.file_name().unwrap().to_string_lossy()
                        ))
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(leftovers, 0, "temp cleaned up");
        let _ = std::fs::remove_file(&p);
    }

    /// Autosave stays quiet unless there's an already-saved drawing with pending edits that is
    /// overdue — and it never blocks or writes an untitled/non-drawing file.
    #[test]
    fn autosave_gates_then_fires() {
        let mut app = CadApp::default();
        // Autosave is OFF at every start now (it is the only unattended writer, and it is how an
        // empty document once reached a user's file). Turn it on explicitly, exactly as the user
        // must — the gate below is the FIRST thing this test covers.
        assert!(!app.autosave_on, "off until asked for");
        app.tick_autosave();
        assert!(
            app.autosave_rx.is_none(),
            "off → no autosave, whatever else is true"
        );
        app.autosave_on = true;
        // Clean → never.
        app.unsaved = false;
        app.tick_autosave();
        assert!(app.autosave_rx.is_none(), "clean → no autosave");
        // Dirty but no file → never.
        app.unsaved = true;
        app.current_file = None;
        app.tick_autosave();
        assert!(app.autosave_rx.is_none(), "no file → no autosave");
        // Dirty + saved file but NOT overdue → never.
        let tmp = std::env::temp_dir().join(format!("rustcad_autosave_{}.rsm", std::process::id()));
        app.current_file = Some(tmp.clone());
        app.last_autosave = std::time::Instant::now();
        app.tick_autosave();
        assert!(app.autosave_rx.is_none(), "not overdue → no autosave");
        // Dirty + saved + overdue → fires (skips only right after boot, when a past Instant
        // can't be built).
        if let Some(past) =
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(300))
        {
            app.last_autosave = past;
            app.tick_autosave();
            assert!(app.autosave_rx.is_some(), "overdue dirty drawing autosaves");
            // Drain the worker so the write finishes, then verify it cleared `unsaved`.
            for _ in 0..600 {
                app.tick_autosave();
                if app.autosave_rx.is_none() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(app.autosave_rx.is_none(), "autosave completed");
            assert!(!app.unsaved, "unsaved cleared after a clean autosave");
            let _ = std::fs::remove_file(&tmp);
            let _ = std::fs::remove_file(tmp.with_extension("simlux.json"));
        }
    }
}
/// What an imported drawing's scale is taken to be when the file does not say.
///
/// Reported from the field: a DXF with no `$INSUNITS` — the common case, since most exporters
/// omit it — had its 4400-unit outline read as **4400 metres**. Extruded to a 3 m storey that is
/// a sheet 4.4 km across, and nothing on screen said why.

#[cfg(test)]
mod imported_drawing_scale {
    use super::*;

    fn payload(doc: Document) -> Box<LoadPayload> {
        Box::new(LoadPayload {
            doc,
            sidecar: None,
            furniture: Vec::new(),
            embedded: None,
            embedded_furniture: Vec::new(),
            sidecar_textures: Vec::new(),
            embedded_textures: Vec::new(),
            embedded_results: None,
            read_ms: 0,
            parse_ms: 0,
            sidecar_ms: 0,
            furn_ms: 0,
            dwg_note: None,
        })
    }

    /// A square whose corners are the ones from the report, in drawing units.
    fn square(size: f64) -> Document {
        let mut d = Document::default();
        d.push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
            cad_kernel::Polyline {
                vertices: [(0.0, 0.0), (size, 0.0), (size, size), (0.0, size)]
                    .iter()
                    .map(|&(x, y)| cad_kernel::PolyVertex {
                        pos: Vec2::new(x, y),
                        bulge: 0.0,
                    })
                    .collect(),
                closed: true,
                widths: Vec::new(),
            },
        )));
        d
    }

    /// A file that declares NOTHING is read at the Factory's working unit — millimetres by
    /// default — rather than assumed to be metres.
    #[test]
    fn an_undeclared_file_is_read_at_the_factory_working_unit() {
        let mut app = CadApp::default();
        assert!((app.factory.units.metres_per_unit - cad_kernel::Units::MM).abs() < 1e-12);
        app.apply_loaded("plan.dxf", payload(square(4400.0)));

        assert!(
            (app.doc.units.metres_per_unit - cad_kernel::Units::MM).abs() < 1e-12,
            "the drawing should now declare millimetres, not sit at an assumed metre",
        );
        assert_eq!(app.doc.units.source, cad_kernel::UnitSource::User);
        // 4400 units is 4.4 m, so the promotion scale is a thousandth.
        assert!(
            (app.doc_k() - 0.001).abs() < 1e-12,
            "plan→3D scale, got {}",
            app.doc_k()
        );
        assert!(
            app.history.iter().any(|h| h.contains("declares none")),
            "the substitution must be stated, not silent: {:?}",
            app.history.last(),
        );
    }

    /// The flat sheet itself: at the adopted scale a 4400-unit outline is 4.4 m across, which
    /// against a 3 m storey is a room. Before the fix it was 4400 m — a ratio of 1467.
    #[test]
    fn the_reported_outline_is_a_room_and_not_a_sheet() {
        let mut app = CadApp::default();
        app.apply_loaded("plan.dxf", payload(square(4400.0)));
        let loops = CadApp::closed_loops_of(&app.doc);
        let l = loops.first().expect("the square is closed");
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in l {
            lo = lo.min(p.x);
            hi = hi.max(p.x);
        }
        let footprint = hi - lo;
        assert!(
            (footprint - 4.4).abs() < 0.05,
            "4400 mm is 4.4 m, got {footprint} m"
        );
        let storey = app.factory.room_height.max(3.0);
        assert!(
            footprint / storey < 200.0,
            "{footprint:.1} m across a {storey:.1} m storey is still a sheet",
        );
    }

    /// A file that DOES declare its unit still wins — the working unit fills a gap, it does not
    /// overrule a statement the file made about itself.
    #[test]
    fn a_declared_file_unit_is_not_overwritten() {
        let mut app = CadApp::default();
        let mut doc = square(3.0);
        doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::M,
            cad_kernel::UnitSource::Declared,
        );
        app.apply_loaded("plan.dxf", payload(doc));
        assert!((app.doc.units.metres_per_unit - 1.0).abs() < 1e-12);
        assert_eq!(app.doc.units.source, cad_kernel::UnitSource::Declared);
        assert!((app.doc_k() - 1.0).abs() < 1e-12);
    }

    /// Setting the Factory to metres restores the old reading for anyone whose undeclared
    /// drawings really are in metres.
    #[test]
    fn a_metre_working_unit_reads_an_undeclared_file_as_metres() {
        let mut app = CadApp::default();
        app.factory.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::M,
            cad_kernel::UnitSource::User,
        );
        app.apply_loaded("plan.dxf", payload(square(3.0)));
        assert!((app.doc_k() - 1.0).abs() < 1e-12);
    }

    /// Geometry built IN MEMORY is untouched — it has no file to have declared anything, and its
    /// author meant whatever they typed. This is what keeps every existing behaviour intact.
    #[test]
    fn an_in_memory_document_keeps_assuming_millimetres() {
        let app = CadApp::default();
        assert_eq!(app.doc.units.source, cad_kernel::UnitSource::Assumed);
        assert!(
            (app.doc_k() - 0.001).abs() < 1e-12,
            "nothing imported, nothing changed"
        );
    }
}
/// A SAVE FOLLOWED BY A LOAD GIVES BACK WHAT WAS SAVED.
///
/// Reported as "its not saving it properly ... when i load it, it loads an older version", with a
/// sidecar on disk holding four features and `furniture_lib: []`, `furniture: []`, `textures: []`
/// against a model carrying a 481,738-triangle import.
///
/// The pieces were tested one at a time and each was correct, which is exactly the situation where
/// a round trip is worth more than the sum of them: this drives the REAL `save_file_worker` and
/// `load_file_worker`, through a real file on disk, and asks the only question that matters —
/// is what comes back what went in.

#[cfg(test)]
mod a_project_survives_a_save_and_a_load {
    use super::*;

    fn furnished_app() -> CadApp {
        let mut app = CadApp::default();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(1000.0, 0.0),
                },
            )));
        app.factory.add_box();
        let mesh = crate::mesh_io::parse_obj(
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\nf 1 2 4\nf 1 3 4\nf 2 3 4\n",
        );
        let a = app
            .factory
            .add_furniture_asset("model_20260805-163358".into(), mesh);
        app.factory
            .place_furniture(a, glam::Vec3::new(2.99, 0.42, 0.68));
        app.factory
            .add_texture("mat0".into(), 4, 4, vec![255; 4 * 4 * 4]);
        app
    }

    /// THE WHOLE WAY ROUND, through a real file.
    #[test]
    fn the_furniture_and_textures_come_back_off_disk() {
        let app = furnished_app();
        assert_eq!(app.factory.furniture_lib.len(), 1, "fixture: one asset");
        assert_eq!(app.factory.furniture.len(), 1, "fixture: placed once");
        assert_eq!(app.factory.textures.len(), 1, "fixture: one texture");
        let tris_in = app.factory.furniture_lib[0].positions.len();
        assert!(tris_in > 0, "fixture: the asset has geometry");

        let dir = std::env::temp_dir().join("simlux_roundtrip_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("roundtrip.dxf").to_string_lossy().into_owned();

        // EXACTLY what the app does: a lite config plus the raw geometry, handed to the worker.
        let doc = app.plan_doc().clone();
        let cfg = app.build_simlux_config_lite();
        let geom = app.factory.furniture_geom_flat();
        assert_eq!(
            cfg.factory.furniture_lib.len(),
            1,
            "the config leaving the UI thread has it"
        );
        save_file_worker(
            &path,
            doc,
            cfg,
            geom,
            crate::simlux_io::ExtraDataStore::Sidecar,
            None,
        )
        .expect("the save must succeed");

        // …and exactly what an open does.
        let payload = load_file_worker(&path).expect("the load must succeed");
        let sidecar = payload.sidecar.as_ref().expect("a sidecar was written");
        assert_eq!(
            payload.furniture.len(),
            1,
            "the imported asset did not come back — the file reopens without it, which is what \
             an older version of the same project looks like",
        );
        assert_eq!(
            payload.furniture[0].positions.len(),
            tris_in,
            "the asset came back with a different amount of geometry",
        );
        assert_eq!(
            sidecar.factory.furniture.len(),
            1,
            "the PLACED instance did not come back"
        );
        assert_eq!(
            sidecar.factory.textures.len(),
            1,
            "the texture did not come back"
        );
        assert!(
            !sidecar.factory.model.features.is_empty(),
            "the solid model did not come back",
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(crate::simlux_io::sidecar_path(std::path::Path::new(&path)));
    }

    /// AND THE SIDECAR ON DISK ACTUALLY CONTAINS IT. The assertion above reads what the loader
    /// hands back; this reads the FILE, which is what the user's evidence was — an empty
    /// `furniture_lib` in the JSON. A loader that reconstructed something from nothing would
    /// satisfy the first test and still lose the project.
    #[test]
    fn the_file_on_disk_is_not_empty_of_imports() {
        let app = furnished_app();
        let dir = std::env::temp_dir().join("simlux_roundtrip_disk");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("ondisk.dxf").to_string_lossy().into_owned();

        save_file_worker(
            &path,
            app.plan_doc().clone(),
            app.build_simlux_config_lite(),
            app.factory.furniture_geom_flat(),
            crate::simlux_io::ExtraDataStore::Sidecar,
            None,
        )
        .expect("save");

        let side = crate::simlux_io::sidecar_path(std::path::Path::new(&path));
        let json = std::fs::read_to_string(&side).expect("the sidecar");
        assert!(
            !json.contains("\"furniture_lib\": []"),
            "the sidecar was written with an EMPTY furniture library — this is the exact text \
             found in the reported file",
        );
        assert!(
            !json.contains("\"textures\": []"),
            "the sidecar was written with no textures"
        );
        assert!(
            json.contains("model_20260805-163358"),
            "the asset's name is not in the file at all",
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(crate::simlux_io::sidecar_path(std::path::Path::new(&path)));
    }
}
/// THE DWG CONVERTER IS FOUND WHERE THE APP LOOKS FOR IT.
///
/// `dwg_converter` walks the ancestors of the running executable for
/// `tools/dwgconv/dwgconv.cmd`. That is a fact about the REPOSITORY LAYOUT — move the wrapper,
/// or ship a build without it, and DWG open stops working with a message about setting an
/// environment variable, which is a support call rather than a bug report.

#[cfg(test)]
mod the_dwg_converter_is_where_the_app_looks {
    /// The wrapper is in the tree, next to where the search expects it.
    #[test]
    fn the_wrapper_is_in_the_repository() {
        // The test binary lives in target/debug/deps, so the same ancestor walk finds the repo.
        let exe = std::env::current_exe().expect("the test binary's own path");
        let found = exe.ancestors().any(|a| {
            a.join("tools/dwgconv/dwgconv.cmd").is_file()
                || a.join("tools/dwgconv/dwgconv.sh").is_file()
        });
        assert!(
            found,
            "no tools/dwgconv wrapper above {} — DWG open will report 'no DWG converter found'",
            exe.display(),
        );
    }

    /// And `dwg_converter` actually returns it, rather than falling through to the PATH probe.
    ///
    /// WINDOWS ONLY: the fork's converter drives AutoCAD's `accoreconsole.exe`,
    /// which exists only on Windows — the fork ships no `tools/dwgconv/dwgconv.sh`
    /// (no Linux converter to point it at). On Linux the honest search result is
    /// None → "set RUSTCAD_DWGCONV" (see `convert_dwg_to_dxf`), which this test
    /// would otherwise fight.
    #[cfg(windows)]
    #[test]
    fn the_search_returns_it() {
        // The env override wins by design, so a machine that sets one is not being tested here.
        if std::env::var("RUSTCAD_DWGCONV").is_ok() {
            return;
        }
        let c = super::dwg_converter().expect("the converter search found nothing");
        assert!(
            c.to_lowercase().contains("dwgconv"),
            "the search returned {c:?}, which is not the wrapper",
        );
    }
}
/// SAVING AS DWG.
///
/// Asked as "why cant i save as dwg?" and then "and fix the file saving". The save path took
/// `.dxf` and `.rsm` and said so, which answers the question without solving it: a practice's
/// filing, its consultants and its clients ask for `.dwg`.
///
/// DWG IS A CLOSED FORMAT and nothing here writes one. The drawing goes out as the DXF this app
/// already writes, and AutoCAD's own headless core saves it on — the same tool, running the other
/// way, that opens a DWG today. Every test below uses a STUB converter rather than looking for
/// AutoCAD: a test that needed it would run on one machine and quietly skip on every other, which
/// is much the same as not having one.

#[cfg(test)]
mod a_drawing_can_be_saved_as_dwg {
    use super::*;

    /// A converter that just copies its input to its output — enough to prove the plumbing.
    // MUST CONTAIN `{in}`, or it is treated as a bare executable name rather than a shell
    // command — which is how the first version of these tests failed, with "cannot find the path".
    fn stub() -> &'static str {
        if cfg!(windows) {
            "copy /y \"{in}\" \"{out}\""
        } else {
            "cp {in} {out}"
        }
    }

    // Runs, exits zero, and writes nothing.
    fn stub_that_writes_nothing() -> &'static str {
        if cfg!(windows) {
            "echo {in} 1>nul"
        } else {
            "true {in}"
        }
    }

    /// THE ENVIRONMENT IS PROCESS-WIDE and tests run in parallel, so the two that set
    /// `RUSTCAD_DXF2DWG` have to take turns. They did not at first, and one duly picked up the
    /// other's deliberately-broken converter and failed for a reason that had nothing to do with
    /// what it was testing.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join("simlux_dwg_save");
        let _ = std::fs::create_dir_all(&d);
        d.join(name)
    }

    #[test]
    fn the_converter_is_run_and_its_output_is_the_file() {
        let dxf = tmp("in.dxf");
        let dwg = tmp("out.dwg");
        std::fs::write(&dxf, b"  0\nSECTION\n").expect("the dxf");
        let _ = std::fs::remove_file(&dwg);

        convert_dxf_to_dwg(stub(), &dxf, &dwg).expect("the stub converter runs");
        assert!(dwg.is_file(), "no file at {}", dwg.display());
        assert_eq!(std::fs::read(&dwg).expect("read"), b"  0\nSECTION\n");
        let _ = std::fs::remove_file(&dxf);
        let _ = std::fs::remove_file(&dwg);
    }

    /// A CONVERTER THAT PRODUCES NOTHING IS A FAILURE, whatever it claims.
    ///
    /// accoreconsole is cheerful about failure: a script that stopped at a prompt still exits
    /// zero. The DWG→DXF direction learned that the hard way — "the only symptom is a DXF that
    /// never appears" — so existence, not the exit code, is what is believed.
    #[test]
    fn a_converter_that_writes_nothing_is_reported() {
        let dxf = tmp("in2.dxf");
        let dwg = tmp("out2.dwg");
        std::fs::write(&dxf, b"x").expect("the dxf");
        let _ = std::fs::remove_file(&dwg);

        let e = convert_dxf_to_dwg(stub_that_writes_nothing(), &dxf, &dwg)
            .expect_err("a silent failure must be caught");
        assert!(e.contains("produced no file"), "unhelpful: {e}");
        assert!(
            e.contains("run"),
            "the message must say what to do next: {e}"
        );
        let _ = std::fs::remove_file(&dxf);
    }

    /// AND A FAILED SAVE DOES NOT DESTROY THE FILE IT WAS SAVING OVER.
    ///
    /// The whole reason the DXF goes to a temp file first. Writing it straight onto the `.dwg` and
    /// converting in place would, on any failure, leave a DXF wearing a `.dwg` name — a file that
    /// opens in nothing and reads as corrupt, in place of the drawing that was there.
    #[test]
    fn a_failed_conversion_leaves_the_previous_dwg_alone() {
        let path = tmp("keep.dwg");
        std::fs::write(&path, b"the previous drawing").expect("seed");

        let doc = Document::default();
        let cfg = crate::simlux_io::SimluxConfig::default();
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        // A converter that cannot be run at all.
        std::env::set_var("RUSTCAD_DXF2DWG", "simlux-no-such-converter-xyz {in} {out}");
        let r = save_file_worker(
            &path.to_string_lossy(),
            doc,
            cfg,
            Vec::new(),
            crate::simlux_io::ExtraDataStore::Sidecar,
            None,
        );
        std::env::remove_var("RUSTCAD_DXF2DWG");

        assert!(r.is_err(), "a save that could not convert reported success");
        assert_eq!(
            std::fs::read(&path).expect("still there"),
            b"the previous drawing",
            "the drawing that was already there was overwritten by a failed save",
        );
        let _ = std::fs::remove_file(&path);
    }

    /// THE EXTENSION IS ACCEPTED. Before, `.dwg` fell through to "unknown extension (expected .dxf
    /// or .rsm)" — the message in the report.
    #[test]
    fn dwg_is_no_longer_an_unknown_extension() {
        let path = tmp("ext.dwg");
        let _ = std::fs::remove_file(&path);
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("RUSTCAD_DXF2DWG", stub());
        let r = save_file_worker(
            &path.to_string_lossy(),
            Document::default(),
            crate::simlux_io::SimluxConfig::default(),
            Vec::new(),
            crate::simlux_io::ExtraDataStore::Sidecar,
            None,
        );
        std::env::remove_var("RUSTCAD_DXF2DWG");

        match r {
            Ok(_) => assert!(
                path.is_file(),
                "the save reported success and wrote no file"
            ),
            Err(e) => panic!("a .dwg save failed: {e}"),
        }
        // What landed is what the converter was given — the DXF this app writes.
        let head = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            head.contains("SECTION"),
            "the file does not look like the drawing that was sent"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// AND AN UNKNOWN ONE STILL IS, naming every format that works.
    #[test]
    fn an_unknown_extension_still_says_which_ones_work() {
        let e = save_file_worker(
            "whatever.pdf",
            Document::default(),
            crate::simlux_io::SimluxConfig::default(),
            Vec::new(),
            crate::simlux_io::ExtraDataStore::Sidecar,
            None,
        );
        let e = match e {
            Err(e) => e,
            Ok(_) => panic!("a .pdf was accepted as a drawing"),
        };
        for want in [".dxf", ".dwg", ".rsm"] {
            assert!(e.contains(want), "{want} is missing from {e:?}");
        }
    }
}
/// A HALF-SAVE IS NOT A SAVE.
///
/// Reported as: *"i made a calculation and saved the file. when i closed and opened the nothing was
/// saved."* Everything needed to catch this already existed — the rename is retried for ~1.5 s, the
/// temp is deliberately KEPT because it holds work that exists nowhere else, and the error names it
/// and says what to do. All of it went to the command history, which scrolls.
///
/// And `apply_saved` cleared `unsaved` regardless, so the app believed the project matched disk: the
/// close guard stayed quiet and the window shut on a `.savetmp` nobody knew to look for. On the
/// owner's project the live sidecar was two weeks older than the temp beside it.

#[cfg(test)]
mod a_failed_save_is_not_reported_as_a_save {
    use super::*;

    fn payload(failed: Option<&str>) -> SavePayload {
        SavePayload {
            bytes: 1234,
            note: "  saved".into(),
            simlux_failed: failed.map(|s| s.to_string()),
        }
    }

    /// THE PROJECT STAYS UNSAVED, which is what makes the close guard fire. Without it the window
    /// shuts on work that reached disk under another name.
    #[test]
    fn a_half_save_leaves_the_project_unsaved() {
        let mut app = CadApp::default();
        app.unsaved = true;
        app.apply_saved("C:/x/plan.dxf", payload(Some("could not replace …")));
        assert!(
            app.unsaved,
            "the SIMLUX half never reached disk, so the project does NOT match it — clearing this \
             is what let the window close on a stranded temp",
        );
    }

    /// AND A REAL SAVE STILL CLEARS IT, or every save would nag forever and the flag would stop
    /// meaning anything.
    #[test]
    fn a_whole_save_clears_it() {
        let mut app = CadApp::default();
        app.unsaved = true;
        app.apply_saved("C:/x/plan.dxf", payload(None));
        assert!(!app.unsaved, "a save that wrote both halves matches disk");
        assert!(app.save_failure.is_none(), "and raises nothing");
    }

    /// THE FAILURE IS RAISED WHERE IT WILL BE SEEN, carrying the path of the file that holds the
    /// work. A message in the scrollback is what this replaces.
    #[test]
    fn the_failure_is_raised_with_the_path_that_holds_the_work() {
        let mut app = CadApp::default();
        app.apply_saved(
            "C:/x/plan.dxf",
            payload(Some("…IS saved, in 'C:/x/plan.simlux.json.savetmp'")),
        );
        let raised = app.save_failure.expect("a half-save must raise the dialog");
        assert!(
            raised.contains("savetmp"),
            "the dialog must name the file the work is actually in: {raised}",
        );
    }

    /// THE WORKER ITSELF SETS THE FLAG. Everything above tests what `apply_saved` does with it; if
    /// nothing ever set it, all of that would pass over a feature that never fires. The failure is
    /// forced portably by making the sidecar's destination a DIRECTORY -- the drawing writes
    /// normally, and the rename onto it cannot succeed on any platform.
    #[test]
    fn the_worker_records_a_sidecar_failure() {
        let dir = std::env::temp_dir().join(format!("simlux_halfsave_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let drawing = dir.join("plan.dxf");
        // The sidecar path, occupied by a directory.
        std::fs::create_dir_all(dir.join("plan.simlux.json")).expect("blocker");

        let out = save_file_worker(
            &drawing.to_string_lossy(),
            Document::default(),
            crate::simlux_io::SimluxConfig::default(),
            Vec::new(),
            crate::simlux_io::ExtraDataStore::Sidecar,
            None,
        )
        .expect("the DRAWING half must still succeed -- that is what makes this a HALF save");

        assert!(
            out.simlux_failed.is_some(),
            "the worker must record the sidecar failure as data, or nothing downstream can know",
        );
        assert!(
            out.note.contains("DID NOT SAVE"),
            "and say so in the line it writes: {}",
            out.note,
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AND IT IS ANNOUNCED AS A FAILURE, not as a save with a clause on the end. A line opening
    /// with the word "saved" is read as success and closed on.
    #[test]
    fn the_history_line_does_not_open_by_claiming_success() {
        let mut app = CadApp::default();
        let mut p = payload(Some("boom"));
        p.note = "  ⚠ SIMLUX DID NOT SAVE — the drawing was written but …".into();
        app.apply_saved("C:/x/plan.dxf", p);
        let line = app.history.last().expect("the save writes a line");
        assert!(
            !line.trim_start().starts_with("saved"),
            "it must not open with success: {line}"
        );
        assert!(
            line.contains("DID NOT SAVE"),
            "and must say what happened: {line}"
        );
    }
}
/// A DRAWING MAKES TWO CLAIMS ABOUT ITS SCALE and nothing checked them against each other.
///
/// The owner's gym plan declared MILLIMETRES and contained METRES: a 3.4-unit wall, which is an
/// ordinary wall in metres and 3.4 mm in millimetres. Under that declaration the fittings synced to
/// 1/1000 scale and landed 3.5 km from the building, the furniture outlines drew 3.5 million units
/// off screen, and every calculation returned 0 lx. It cost a session to find, and the
/// contradiction was in the file the whole time.

#[cfg(test)]
mod the_declared_unit_is_checked_against_the_drawing {
    use super::*;

    /// A drawing `span` units across, declared at `k` metres per unit.
    fn drawing(span: f64, k: f64) -> CadApp {
        let mut app = CadApp::default();
        app.doc = Document::default();
        app.doc.units =
            cad_kernel::Units::from_metres_per_unit(k, cad_kernel::UnitSource::Declared);
        app.doc.push(
            Line {
                a: Vec2::new(0.0, 0.0),
                b: Vec2::new(span, span * 0.4),
            }
            .into(),
        );
        app
    }

    /// THE REAL CASE: 33 units across, declared millimetres — 33 mm, which is not a building.
    #[test]
    fn the_gym_plan_would_have_been_caught() {
        let app = drawing(33.0, 0.001);
        let w = app
            .unit_sanity_note()
            .expect("33 mm is not a building and must be flagged");
        assert!(
            w.contains("`units m`"),
            "it must name the unit that works: {w}"
        );
        assert!(w.contains("33"), "and show the span it measured: {w}");
        // AND MUST NOT NAME UNITS THAT DO NOT HELP. Listing all six is not advice, it is the
        // reader doing the arithmetic themselves -- which is the job this is here to do.
        for useless in ["`units mm`", "`units cm`", "`units in`"] {
            assert!(
                !w.contains(useless),
                "{useless} would make it {:.3} m, still not a building -- it must not be offered: {w}",
                33.0 * match useless {
                    "`units mm`" => 0.001,
                    "`units cm`" => 0.01,
                    _ => 0.0254,
                },
            );
        }
    }

    /// THE OTHER DIRECTION, which is the more common mistake: a millimetre drawing read as metres.
    /// 33 000 units at 1 m/unit is a 33 km building.
    #[test]
    fn a_millimetre_drawing_read_as_metres_is_caught() {
        let app = drawing(33_000.0, 1.0);
        let w = app
            .unit_sanity_note()
            .expect("a 33 km building must be flagged");
        assert!(
            w.contains("`units mm`"),
            "millimetres is the unit that makes it 33 m: {w}"
        );
    }

    /// AND A DRAWING THAT AGREES WITH ITSELF SAYS NOTHING. A warning that fires on correct work is
    /// one that gets ignored, which is how the original was missed.
    #[test]
    fn a_sane_drawing_is_silent() {
        assert!(
            drawing(33.0, 1.0).unit_sanity_note().is_none(),
            "33 m in metres is a building"
        );
        assert!(
            drawing(33_000.0, 0.001).unit_sanity_note().is_none(),
            "33 000 mm is the same building, declared the other way",
        );
        assert!(
            drawing(4.0, 1.0).unit_sanity_note().is_none(),
            "a 4 m room is legitimate"
        );
        assert!(
            drawing(1_200.0, 1.0).unit_sanity_note().is_none(),
            "so is a 1.2 km site plan"
        );
    }

    /// AN EMPTY DRAWING MAKES NO CLAIM, so there is nothing to contradict — and a new file must not
    /// open with a warning on it.
    #[test]
    fn an_empty_drawing_is_silent() {
        let mut app = CadApp::default();
        app.doc = Document::default();
        app.doc.units =
            cad_kernel::Units::from_metres_per_unit(0.001, cad_kernel::UnitSource::Declared);
        assert!(
            app.unit_sanity_note().is_none(),
            "nothing drawn, nothing to check"
        );
    }

    /// IT MEASURES THE SPAN, NOT THE COORDINATES. The gym plan sits 6.8 km from the origin on
    /// survey coordinates; that says nothing about how big the building is, and a check that read
    /// position instead of size would pass every wrong file and fail every right one.
    #[test]
    fn a_drawing_far_from_the_origin_is_judged_on_its_size() {
        let mut app = CadApp::default();
        app.doc = Document::default();
        app.doc.units =
            cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::Declared);
        app.doc.push(
            Line {
                a: Vec2::new(3_499.6, -6_852.6),
                b: Vec2::new(3_532.7, -6_839.6),
            }
            .into(),
        );
        assert!(
            app.unit_sanity_note().is_none(),
            "33 m across at 6.8 km from the origin is the owner's actual plan, correctly declared",
        );
    }
}
/// "WHEN I TRIED TO OPEN A DWG FILE IN ANOTHER ITS WAS SHOWING UN RECOGNISED FORMAT."
///
/// SIMLUX ships the DWG converter and always has — `tools\dwgconv\dwgconv.cmd` sits beside
/// `simlux.exe` and the package's integrity check lists it. What it cannot ship is AutoCAD:
/// the script drives `accoreconsole.exe`, AutoCAD's own headless core, found by scanning
/// `C:\Program Files\Autodesk`. On a machine without AutoCAD there is nothing to drive.
///
/// The script says so, in full sentences, on stderr — and a GUI user has no console to read it in.
/// The app reported "converter exited 3 (no DXF produced)": an exit code where an explanation had
/// been written and thrown away.

#[cfg(test)]
mod the_dwg_converter_explains_itself {
    use super::*;

    /// Write a stand-in converter that runs on THIS platform: a `.cmd` batch on
    /// Windows (spawned via `cmd /c` by `run_dwg_conversion`), an executable
    /// `#!/bin/sh` script elsewhere (spawned directly). `body` is the platform's
    /// own script text minus the shebang / `@echo off` preamble.
    fn write_converter(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let script = if cfg!(windows) {
            dir.join(format!("{}.cmd", name))
        } else {
            dir.join(format!("{}.sh", name))
        };
        if cfg!(windows) {
            std::fs::write(&script, format!("@echo off\r\n{}\r\n", body)).expect("write");
        } else {
            std::fs::write(&script, format!("#!/bin/sh\n{}\n", body)).expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&script).expect("stat").permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&script, perms).expect("chmod");
            }
        }
        script
    }

    /// THE REASON REACHES THE MESSAGE. A converter that fails and says why must have its words
    /// carried, not replaced by its exit status.
    #[test]
    fn a_failing_converter_has_its_reason_carried() {
        let dir = std::env::temp_dir().join(format!("simlux_dwgconv_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        // A stand-in converter that fails exactly as `dwgconv.cmd` does with no AutoCAD present.
        let script = write_converter(
            &dir,
            "fake",
            "echo accoreconsole.exe not found under Program Files Autodesk. 1>&2\nexit 3",
        );
        let out = dir.join("out.dxf");

        let err = run_dwg_conversion(
            &script.to_string_lossy(),
            &dir.join("in.dwg").to_string_lossy(),
            &out,
        )
        .expect_err("a converter that produces no DXF must fail");

        assert!(
            err.contains("accoreconsole"),
            "the converter's own explanation must survive into the message: {err}",
        );
        assert!(
            err.contains("no DXF"),
            "…and it must still say what went wrong: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A SILENT FAILURE STILL EXPLAINS ITSELF. Some converters exit non-zero saying nothing, and
    /// "exited 1" tells a user nothing they can act on — the standing cause is worth naming.
    #[test]
    fn a_silent_failure_still_names_the_usual_cause() {
        let dir = std::env::temp_dir().join(format!("simlux_dwgquiet_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let script = write_converter(&dir, "quiet", "exit 1");

        let err = run_dwg_conversion(
            &script.to_string_lossy(),
            &dir.join("in.dwg").to_string_lossy(),
            &dir.join("out.dxf"),
        )
        .expect_err("no DXF, so it must fail");

        assert!(
            err.contains("AutoCAD") && err.contains("RUSTCAD_DWGCONV"),
            "a converter that says nothing must still leave the user something to do: {err}",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AND A CONVERTER THAT WORKS IS NOT SECOND-GUESSED — the diagnostic re-run happens only on
    /// failure, so a successful open costs one process, not two.
    #[test]
    fn a_working_converter_is_run_once() {
        let dir = std::env::temp_dir().join(format!("simlux_dwgok_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        // Appends a line each run, so a second invocation is visible.
        let script = write_converter(
            &dir,
            "ok",
            "echo ran >> \"$(dirname \"$0\")/runs.txt\"\necho 0 > \"$2\"",
        );
        let out = dir.join("out.dxf");

        run_dwg_conversion(
            &script.to_string_lossy(),
            &dir.join("in.dwg").to_string_lossy(),
            &out,
        )
        .expect("it produced a DXF, so it succeeded");

        let runs = std::fs::read_to_string(dir.join("runs.txt")).unwrap_or_default();
        assert_eq!(
            runs.lines().count(),
            1,
            "a converter that worked must not be re-run: {runs:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
#[cfg(test)]
mod xref_wblock_tests {
    use super::*;

    #[test]
    fn xref_attach_detach_list_round_trip() {
        let mut app = CadApp::default();
        let mut doc = cad_kernel::Document::default();
        doc.dobjects.push(DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(5.0, 5.0),
        })));
        app.doc
            .dobjects
            .push(DObject::new(Geom::Xref(cad_kernel::Xref {
                name: "plan-a".into(),
                path: "/tmp/x.rsm".into(),
                insert: Vec2::ZERO,
                scale: 1.0,
                rotation: 0.0,
                cached: doc.dobjects,
            })));
        // list
        app.apply_xref(&[]);
        let listed = app.history.iter().any(|h| h.contains("plan-a"));
        assert!(listed, "list names the reference");
        // detach
        app.apply_xref(&["detach".into(), "plan-a".into()]);
        assert!(!app
            .doc
            .dobjects
            .iter()
            .any(|d| matches!(d.geom, Geom::Xref(_))));
    }

    #[test]
    fn xref_pending_commit_places_at_click() {
        let mut app = CadApp::default();
        let mut doc = cad_kernel::Document::default();
        doc.dobjects
            .push(DObject::new(Geom::Circle(cad_kernel::Circle {
                center: Vec2::ZERO,
                radius: 3.0,
            })));
        app.xref_pending = Some(cad_kernel::Xref {
            name: "plan-b".into(),
            path: "/tmp/b.rsm".into(),
            insert: Vec2::ZERO,
            scale: 1.0,
            rotation: 0.0,
            cached: doc.dobjects,
        });
        app.commit_xref_at(Vec2::new(9.0, 4.0));
        let last = app.doc.dobjects.last().unwrap();
        if let Geom::Xref(x) = &last.geom {
            assert!(x.insert.dist(Vec2::new(9.0, 4.0)) < 1e-9);
            assert_eq!(x.cached.len(), 1);
        } else {
            panic!("expected Xref");
        }
        assert!(app.xref_pending.is_some(), "place-multiple stays armed");
    }

    #[test]
    fn wblock_subdoc_holds_selection_and_whole_drawing() {
        let mut app = CadApp::default();
        app.selection.clear();
        // Selection mode: only the picked dobjects go to the sub-doc.
        let a = app.doc.push(DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(1.0, 0.0),
        })));
        let _b = app.doc.push(DObject::new(Geom::Circle(cad_kernel::Circle {
            center: Vec2::ZERO,
            radius: 1.0,
        })));
        app.selection = vec![a];
        app.apply_wblock();
        let sub = app.wblock_subdoc.as_ref().expect("sub-doc pending");
        assert_eq!(sub.dobjects.len(), 1, "selection only");
        assert_eq!(app.save_dialog_purpose, 1);
        // Whole drawing when selection empty.
        app.selection.clear();
        app.apply_wblock();
        let sub = app.wblock_subdoc.as_ref().expect("sub-doc pending");
        assert!(sub.dobjects.len() >= 2, "whole drawing (no selection)");
    }
}
/// THE 3D PROJECT CAN LIVE INSIDE THE DRAWING FILE ("Inside this file" in Save
/// As) instead of in `.simlux.json` beside it — RSM extra-blobs / DXF XRECORDs.
/// These tests drive the same real workers the app uses, in both directions.

#[cfg(test)]
mod an_embedded_project_survives_a_save_and_a_load {
    use super::*;

    /// A drawing + one solid + one placed furniture asset + one texture — the
    /// same shape the sidecar round-trip test uses, so the two tests compare.
    fn furnished_app() -> CadApp {
        let mut app = CadApp::default();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(1000.0, 0.0),
                },
            )));
        app.factory.add_box();
        let mesh = crate::mesh_io::parse_obj(
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\nf 1 2 4\nf 1 3 4\nf 2 3 4\n",
        );
        let a = app.factory.add_furniture_asset("model_embed".into(), mesh);
        app.factory
            .place_furniture(a, glam::Vec3::new(2.0, 3.0, 0.0));
        app.factory
            .add_texture("mat0".into(), 4, 4, vec![255; 4 * 4 * 4]);
        app
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("simlux_embed_{name}"));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn clean(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(crate::simlux_io::sidecar_path(path));
        let _ = std::fs::remove_file(crate::light_store::result_path(path));
    }

    /// The whole way round through a real .dxf: save embedded → reopen → the
    /// model, the furniture and the texture are back, and NO sidecar exists.
    #[test]
    fn a_dxf_saved_embedded_reopens_self_contained() {
        let app = furnished_app();
        let path = scratch("dxf").join("plan.dxf");
        let path_s = path.to_string_lossy().into_owned();
        clean(&path);

        let payload = save_file_worker(
            &path_s,
            app.plan_doc().clone(),
            app.build_simlux_config_lite(),
            app.factory.furniture_geom_flat(),
            crate::simlux_io::ExtraDataStore::Embedded,
            None,
        )
        .expect("the embedded save must succeed");
        assert!(payload.simlux_failed.is_none(), "{}", payload.note);

        // The file alone carries the project — nothing was written beside it.
        assert!(
            !crate::simlux_io::sidecar_path(&path).exists(),
            ".simlux.json must not exist after an embedded save",
        );
        assert!(
            !crate::light_store::result_path(&path).exists(),
            ".simlux-result.json must not exist after an embedded save",
        );
        // And the drawing bytes actually contain the payload.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("SIMLUX_DATA"),
            "the DXF must carry the SIMLUX_DATA dictionary",
        );

        // …and exactly what an open does.
        let payload = load_file_worker(&path_s).expect("the load must succeed");
        assert!(
            payload.sidecar.is_none(),
            "no sidecar was written, so none can load"
        );
        let embedded = payload
            .embedded
            .as_ref()
            .expect("the embedded project must load");
        assert_eq!(
            embedded.factory.furniture.len(),
            1,
            "the PLACED instance is back"
        );
        assert_eq!(embedded.factory.textures.len(), 1, "the texture is back");
        assert!(
            !embedded.factory.model.features.is_empty(),
            "the solid model is back",
        );
        assert_eq!(
            payload.embedded_furniture.len(),
            1,
            "the asset geometry is back"
        );
        assert!(
            !payload.embedded_furniture[0].positions.is_empty(),
            "the asset came back with geometry",
        );

        clean(&path);
    }

    /// The RSM leg of the same story, TRANSPORT-ONLY: the config rides inside
    /// the v201 stream and comes back to the loader. (The drawing is a bare 2D
    /// plan — a factory model places furniture-symbol dobjects with ByLayer
    /// linetypes, which trip a separate pre-existing RSM validation bug this
    /// feature does not touch.)
    #[test]
    fn an_rsm_saved_embedded_reopens_self_contained() {
        // A bare plan document and a hand-built config — CadApp::default()'s doc
        // already carries default dobjects with ByLayer linetypes, which trip a
        // separate pre-existing RSM validation bug this feature does not touch.
        let mut doc = cad_kernel::Document::default();
        doc.push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
            cad_kernel::Line {
                a: Vec2::new(0.0, 0.0),
                b: Vec2::new(1000.0, 0.0),
            },
        )));
        let path = scratch("rsm").join("plan.rsm");
        let path_s = path.to_string_lossy().into_owned();
        clean(&path);

        let mut cfg = crate::simlux_io::SimluxConfig::default();
        cfg.vars.insert("width".to_string(), "4.2*2".to_string());
        cfg.layers_3d.insert("WALLS".to_string(), 3.0);
        // One furniture ASSET in the library — its geometry must ride the native
        // deflated blob, not base64 text inside the config JSON.
        cfg.factory
            .furniture_lib
            .push(crate::simlux_io::FurnitureAssetRec {
                name: "chair".into(),
                ..Default::default()
            });
        let geom = vec![crate::factory::FurnitureGeomRaw {
            pos: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            nrm: vec![0.0f32; 12],
            uv: vec![],
            alpha: vec![],
        }];
        save_file_worker(
            &path_s,
            doc,
            cfg,
            geom,
            crate::simlux_io::ExtraDataStore::Embedded,
            Some(crate::light_store::StoredResults::default()),
        )
        .expect("the embedded .rsm save must succeed");
        assert!(
            !crate::simlux_io::sidecar_path(&path).exists(),
            "no sidecar beside an .rsm"
        );
        let bytes = std::fs::read(&path).unwrap();
        let ver = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
        assert_eq!(ver, 201, "the RSM must be v201 when it embeds data");

        let payload = load_file_worker(&path_s).expect("the load must succeed");
        assert!(payload.sidecar.is_none(), "no sidecar was written");
        let embedded = payload.embedded.expect("the embedded project must load");
        assert_eq!(
            embedded.vars.get("width").map(|s| s.as_str()),
            Some("4.2*2")
        );
        assert_eq!(embedded.layers_3d.get("WALLS"), Some(&3.0));
        assert!(
            payload.embedded_results.is_some(),
            "the saved light results rode inside the file too",
        );
        // The furniture geometry came back from the NATIVE blob. The record is
        // drained out of the config by the decode (exactly as the sidecar path
        // drains it), so what matters is the decoded asset holding the meshes.
        assert_eq!(
            payload.embedded_furniture.len(),
            1,
            "the native geometry decoded"
        );
        assert!(
            embedded.factory.furniture_lib.is_empty(),
            "the decoded records are drained from the config",
        );
        assert_eq!(
            payload.embedded_furniture[0].positions.len(),
            4,
            "all four vertices came back from the deflated blob",
        );
        clean(&path);
    }

    /// A SIDECAR MODE save beside an older embedded file is the reverse migration:
    /// the file is rewritten WITHOUT the payload and the sidecar comes back. (The
    /// old sidecar the file used to read from is long gone — it was deleted at the
    /// embedded save.)
    #[test]
    fn saving_sidecar_after_embedded_rewrites_a_clean_drawing() {
        let app = furnished_app();
        let path = scratch("back").join("plan.dxf");
        let path_s = path.to_string_lossy().into_owned();
        clean(&path);
        save_file_worker(
            &path_s,
            app.plan_doc().clone(),
            app.build_simlux_config_lite(),
            app.factory.furniture_geom_flat(),
            crate::simlux_io::ExtraDataStore::Embedded,
            None,
        )
        .expect("embedded save");
        assert!(
            !crate::simlux_io::sidecar_path(&path).exists(),
            "no sidecar yet"
        );

        save_file_worker(
            &path_s,
            app.plan_doc().clone(),
            app.build_simlux_config_lite(),
            app.factory.furniture_geom_flat(),
            crate::simlux_io::ExtraDataStore::Sidecar,
            None,
        )
        .expect("sidecar save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("SIMLUX_DATA"),
            "a sidecar-mode drawing must not carry the SIMLUX_DATA dictionary",
        );
        let doc = cad_io::dxf::read_dxf(&text).expect("still parses");
        assert!(
            doc.extra_blobs.is_empty(),
            "the payload was not left in the file"
        );
        assert!(
            crate::simlux_io::sidecar_path(&path).exists(),
            "the sidecar is back"
        );
        clean(&path);
    }

    /// The embedded STALE sidecar cleanup: saving embedded over a drawing that
    /// still had old JSON files beside it removes them, so the next open does not
    /// find two copies and ask.
    #[test]
    fn an_embedded_save_removes_a_stale_sidecar_pair() {
        let app = furnished_app();
        let path = scratch("stale").join("plan.dxf");
        let path_s = path.to_string_lossy().into_owned();
        clean(&path);
        // Plant an old sidecar pair next to the path, as an earlier save left it.
        let side = crate::simlux_io::sidecar_path(&path);
        let res = crate::light_store::result_path(&path);
        std::fs::write(&side, b"{ }").unwrap();
        std::fs::write(&res, b"{ }").unwrap();

        save_file_worker(
            &path_s,
            app.plan_doc().clone(),
            app.build_simlux_config_lite(),
            app.factory.furniture_geom_flat(),
            crate::simlux_io::ExtraDataStore::Embedded,
            None,
        )
        .expect("embedded save");
        assert!(!side.exists(), "the stale .simlux.json must be removed");
        assert!(
            !res.exists(),
            "the stale .simlux-result.json must be removed"
        );
        clean(&path);
    }

    /// A file carrying BOTH an embedded project and a sidecar: the load worker
    /// hands back both, and `apply_loaded` asks instead of silently picking one —
    /// the choice installs the picked copy and sets the store mode for later saves.
    #[test]
    fn a_file_with_both_copies_asks_and_the_answer_sticks() {
        let app = furnished_app();
        let path = scratch("both").join("plan.dxf");
        let path_s = path.to_string_lossy().into_owned();
        clean(&path);
        // One copy inside the file…
        save_file_worker(
            &path_s,
            app.plan_doc().clone(),
            app.build_simlux_config_lite(),
            app.factory.furniture_geom_flat(),
            crate::simlux_io::ExtraDataStore::Embedded,
            None,
        )
        .expect("embedded save");
        // …and an older sidecar planted back beside it (e.g. a restored backup).
        // Written by hand via the sidecar writer — a full SIDE-CAR MODE save of
        // the drawing would rewrite the file without the embedded copy, which is
        // the reverse migration rather than the both-copies situation.
        let mut cfg = app.build_simlux_config_lite();
        cfg.vars.insert("older".to_string(), "1".to_string());
        crate::simlux_io::save(&path, &cfg).expect("plant the older sidecar");
        assert!(
            crate::simlux_io::sidecar_path(&path).exists(),
            "both copies now exist"
        );

        let payload = load_file_worker(&path_s).expect("the load must succeed");
        assert!(payload.sidecar.is_some(), "the sidecar copy was read");
        assert!(payload.embedded.is_some(), "the embedded copy was read");

        // The real install path: the drawing opens, the 3D side waits on the user.
        let mut app = CadApp::default();
        app.apply_loaded(&path_s, payload);
        assert!(
            app.extra_choice.is_some(),
            "both copies must raise the question"
        );
        assert!(
            app.factory.furniture.is_empty(),
            "nothing is installed before the answer"
        );
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("TWO copies") || h.contains("both")),
            "the ask must be recorded in the history: {:?}",
            app.history.last(),
        );

        // Pick the sidecar copy — the mode follows for every later save.
        app.choose_extra_data(ExtraPick::Sidecar);
        assert!(app.extra_choice.is_none(), "the question is answered");
        assert_eq!(
            app.factory.furniture.len(),
            1,
            "the chosen copy is installed"
        );
        assert_eq!(
            app.extra_store,
            crate::simlux_io::ExtraDataStore::Sidecar,
            "a sidecar answer makes further saves write sidecars",
        );

        clean(&path);
    }

    /// The question also honours the EMBEDDED answer: choosing the copy inside
    /// the file installs it and makes further saves embed.
    #[test]
    fn the_answer_can_be_the_embedded_copy() {
        let app = furnished_app();
        let path = scratch("both2").join("plan.dxf");
        let path_s = path.to_string_lossy().into_owned();
        clean(&path);
        save_file_worker(
            &path_s,
            app.plan_doc().clone(),
            app.build_simlux_config_lite(),
            app.factory.furniture_geom_flat(),
            crate::simlux_io::ExtraDataStore::Embedded,
            None,
        )
        .expect("embedded save");
        let mut cfg = app.build_simlux_config_lite();
        crate::simlux_io::save(&path, &cfg).expect("plant the older sidecar");

        let mut app = CadApp::default();
        let payload = load_file_worker(&path_s).expect("load");
        app.apply_loaded(&path_s, payload);
        assert!(app.extra_choice.is_some());
        app.choose_extra_data(ExtraPick::Embedded);
        assert_eq!(app.extra_store, crate::simlux_io::ExtraDataStore::Embedded);
        assert_eq!(
            app.factory.furniture.len(),
            1,
            "the embedded copy is installed"
        );
        assert!(app.extra_choice.is_none());
        clean(&path);
    }

    /// While the question is unanswered the factory is still empty — a save now
    /// would write that blank state over the file, so it is refused until the
    /// user picks. (This is the autosave + stale-history class of bug, again.)
    #[test]
    fn save_is_refused_while_the_question_is_unanswered() {
        let mut app = CadApp::default();
        app.extra_choice = Some(Box::new(PendingExtraChoice {
            path: "x.dxf".into(),
            sidecar_cfg: None,
            sidecar_furn: Vec::new(),
            sidecar_tex: Vec::new(),
            embedded_cfg: None,
            embedded_furn: Vec::new(),
            embedded_tex: Vec::new(),
            embedded_results: None,
        }));
        app.do_save("x.dxf");
        assert!(app.busy.is_none(), "the save must not start");
        assert!(
            app.history.iter().any(|h| h.contains("save deferred")),
            "and must say why: {:?}",
            app.history.last(),
        );
    }

    /// Loading a file that ONLY embeds (no sidecar) sets the mode to Embedded, so
    /// plain Save and autosave keep the project inside the file.
    #[test]
    fn an_embedded_only_load_follows_with_embedded_saves() {
        let app = furnished_app();
        let path = scratch("only").join("plan.dxf");
        let path_s = path.to_string_lossy().into_owned();
        clean(&path);
        save_file_worker(
            &path_s,
            app.plan_doc().clone(),
            app.build_simlux_config_lite(),
            app.factory.furniture_geom_flat(),
            crate::simlux_io::ExtraDataStore::Embedded,
            None,
        )
        .expect("embedded save");

        let mut app = CadApp::default();
        let payload = load_file_worker(&path_s).expect("load");
        assert!(payload.sidecar.is_none());
        app.apply_loaded(&path_s, payload);
        assert_eq!(app.extra_store, crate::simlux_io::ExtraDataStore::Embedded);
        assert_eq!(
            app.factory.furniture.len(),
            1,
            "the embedded copy was installed"
        );
        assert!(
            app.extra_choice.is_none(),
            "no question when only one copy exists"
        );
        clean(&path);
    }
}
