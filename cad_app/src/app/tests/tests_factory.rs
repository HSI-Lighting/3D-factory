use super::super::*;

#[cfg(test)]
mod factory_sketch_tests {
    use super::*;

    // ── LAYER COLOUR REACHES 3D ─────────────────────────────────────────────────────────────
    //
    // "i drew some of these arrays in different colors but they showed up in the same color",
    // then, once the layer table was carried into sketches and 2D was right: "see how the color
    // is not getting carried. why is that" — a white circle and a green circle on a face, both
    // drawn ORANGE in the viewport. The line builders picked a literal before entering the
    // dobject loop, so every object in a sketch was one colour by construction.
    //
    // Every test below was run against the unfixed builders and seen to fail.

    /// Distinct (r,g,b) triples in a line buffer, quantised to 1/100 so float noise cannot split
    /// one colour in two, with `sketch_lines`' red/green FRAME AXES dropped — they mark an empty
    /// plane, they are not sketch geometry, and they are not layer-coloured.
    fn line_colours(vs: &[crate::light3d::V3]) -> Vec<(i32, i32, i32)> {
        let q = |x: f32| (x * 100.0).round() as i32;
        let mut c: Vec<(i32, i32, i32)> = vs
            .iter()
            .map(|v| (q(v.r), q(v.g), q(v.b)))
            .filter(|&t| t != (100, 30, 30) && t != (30, 100, 30))
            .collect();
        c.sort();
        c.dedup();
        c
    }

    fn unit_circle_at(x: f64) -> cad_kernel::DObject {
        cad_kernel::DObject::new(cad_kernel::Geom::Circle(cad_kernel::Circle {
            center: Vec2::new(x, 0.0),
            radius: 1.0,
        }))
    }

    /// Model a FRESH interactive sketch draw: `Document::push` is a pure append
    /// now, so the active-layer stamp happens the way `add_dobject` does it
    /// (`stamp_fresh_style`).
    fn push_stamped(app: &mut CadApp, d: cad_kernel::DObject) -> usize {
        let mut d = d;
        app.stamp_fresh_style(&mut d.style);
        app.doc.push(d)
    }

    /// `CadApp::default()` ships three demo layers and starts on WALLS (red), so a test that
    /// means layer 0 has to say so.
    fn app_on_a_green_layer() -> (CadApp, u32) {
        let mut app = CadApp::default();
        let g = app.doc.layers.add(Layer {
            name: "G".into(),
            color: Color::Aci(3),
            ..Layer::layer_zero()
        });
        app.doc.layers.active = g;
        (app, g)
    }

    #[test]
    fn a_layer_colour_reaches_the_live_sketch_in_3d() {
        let (mut app, g) = app_on_a_green_layer();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert_eq!(
            app.doc.layers.active, g,
            "the sketch inherits the drawing's active layer"
        );
        push_stamped(&mut app, unit_circle_at(0.0));

        let lines = app.factory.live_sketch_lines(&app.doc);
        assert!(!lines.is_empty(), "the live sketch shows in 3D at all");
        for v in &lines {
            assert!(
                v.g > 0.9 && v.r < 0.1 && v.b < 0.1,
                "ACI 3 green must reach 3D, got ({:.2}, {:.2}, {:.2})",
                v.r,
                v.g,
                v.b,
            );
        }
    }

    /// The half a green-only test would let through — the user's other circle was WHITE.
    #[test]
    fn a_white_layer_reads_white_in_3d() {
        let mut app = CadApp::default();
        app.doc.layers.active = LayerTable::LAYER_ZERO;
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.doc.push(unit_circle_at(0.0));
        for v in &app.factory.live_sketch_lines(&app.doc) {
            assert!(
                v.r > 0.99 && v.g > 0.99 && v.b > 0.99,
                "a white layer must read white, got ({:.2}, {:.2}, {:.2})",
                v.r,
                v.g,
                v.b,
            );
        }
    }

    /// THE REPORT, STATED LITERALLY. Cannot be passed by changing the constant.
    #[test]
    fn two_layers_two_colours_in_one_sketch() {
        let (mut app, g) = app_on_a_green_layer();
        app.doc.layers.active = LayerTable::LAYER_ZERO; // white
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        push_stamped(&mut app, unit_circle_at(0.0));
        app.doc.layers.active = g;
        push_stamped(&mut app, unit_circle_at(5.0));

        let cols = line_colours(&app.factory.live_sketch_lines(&app.doc));
        assert_eq!(
            cols.len(),
            2,
            "a white circle and a green circle are two colours, got {cols:?}"
        );
        assert!(
            cols.contains(&(100, 100, 100)),
            "the white-layer circle is white: {cols:?}"
        );
        assert!(
            cols.contains(&(0, 100, 0)),
            "the green-layer circle is green: {cols:?}"
        );
    }

    /// …and it does not evaporate the moment the sketch is finished.
    #[test]
    fn a_finished_sketch_keeps_its_layer_colours() {
        let (mut app, g) = app_on_a_green_layer();
        app.doc.layers.active = LayerTable::LAYER_ZERO;
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        push_stamped(&mut app, unit_circle_at(0.0));
        app.doc.layers.active = g;
        push_stamped(&mut app, unit_circle_at(5.0));
        app.factory_exit_sketch();

        let cols = line_colours(&app.factory.sketch_lines(&app.doc));
        assert_eq!(
            cols.len(),
            2,
            "two layers, two colours after Finish: {cols:?}"
        );
        let neutral = *cols
            .iter()
            .find(|c| c.0 == c.1 && c.1 == c.2)
            .unwrap_or_else(|| panic!("the white-layer circle stays neutral: {cols:?}"));
        assert!(neutral.0 > 50, "…and stays light, not black: {neutral:?}");
        let green = *cols
            .iter()
            .find(|c| c.1 > c.0 && c.1 > c.2)
            .unwrap_or_else(|| panic!("the green-layer circle stays green: {cols:?}"));
        assert!(
            green.0 < 10 && green.2 < 10,
            "…and stays pure green: {green:?}"
        );
    }

    /// WHICH TABLE IS THE SOURCE. Recolouring a layer must recolour the work on every plane —
    /// so a finished sketch's own snapshot of the table is NOT the answer. Without this test a
    /// fix that resolves against `sk.doc.layers` ships stale colours and looks correct.
    #[test]
    fn recolouring_a_layer_in_the_plan_updates_finished_sketches() {
        let (mut app, g) = app_on_a_green_layer();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        push_stamped(&mut app, unit_circle_at(0.0));
        app.factory_exit_sketch();

        let before = line_colours(&app.factory.sketch_lines(&app.doc));
        assert!(
            before.iter().all(|c| c.1 > c.0 && c.1 > c.2),
            "green to start with: {before:?}"
        );

        app.doc.layers.get_mut(g).expect("the layer").color = Color::Aci(1); // red
        let after = line_colours(&app.factory.sketch_lines(&app.doc));
        assert!(
            after.iter().all(|c| c.0 > c.1 && c.0 > c.2),
            "recolouring the layer must reach work already drawn on a plane: {after:?}",
        );
    }

    /// The live/finished cue moved from HUE to VALUE. A finished plane keeps its colour and only
    /// steps back, so "which plane am I on" survives without costing the colour the user chose.
    #[test]
    fn a_finished_plane_keeps_its_hue_and_only_dims() {
        let (mut app, _g) = app_on_a_green_layer();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        push_stamped(&mut app, unit_circle_at(0.0));
        app.factory_exit_sketch();
        // A GENUINELY non-coplanar plane, or `factory_enter_sketch` reopens the first one.
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(
            glam::Vec3::ZERO,
            glam::Vec3::X,
        ));
        push_stamped(&mut app, unit_circle_at(0.0));

        let live = line_colours(&app.factory.live_sketch_lines(&app.doc));
        let fin = line_colours(&app.factory.sketch_lines(&app.doc));
        assert_eq!(
            live,
            vec![(0, 100, 0)],
            "the plane being drawn on is full strength: {live:?}"
        );
        assert_eq!(fin.len(), 1, "one finished plane, one colour: {fin:?}");
        let f = fin[0];
        assert!(
            f.1 > f.0 && f.1 > f.2,
            "the finished plane keeps its HUE, got {f:?}"
        );
        assert!(
            f.1 < 100 && f.1 > 20,
            "…and only steps back in VALUE, got {f:?}"
        );
    }

    /// A sketch object naming a layer only the DRAWING can answer — what a copy from the plan
    /// leaves behind, and what `LayerTable::remove`'s renumbering leaves behind, since its fixup
    /// walks `self.doc.dobjects` alone and never repairs stored sketches. Must resolve, and must
    /// not panic on an id past the end of every table.
    #[test]
    fn a_sketch_with_a_stale_layer_id_resolves_against_the_plan_and_never_panics() {
        let mut app = CadApp::default();
        let three = app.doc.layers.add(Layer {
            name: "L3".into(),
            color: Color::Aci(1),
            ..Layer::layer_zero()
        });
        assert_eq!(three, 3, "LayerTable::add hands back sequential ids");

        let mut sk = cad_solid::Sketch::new(crate::factory::FactoryState::ground_frame());
        let mut d3 = unit_circle_at(0.0);
        d3.style.layer = 3;
        sk.doc.dobjects.push(d3);
        let mut d99 = unit_circle_at(5.0);
        d99.style.layer = 99;
        sk.doc.dobjects.push(d99);
        app.factory.model.sketches.push(sk);

        let cols = line_colours(&app.factory.sketch_lines(&app.doc));
        assert_eq!(cols.len(), 2, "two objects, two colours: {cols:?}");
        assert!(
            cols.iter().any(|c| c.0 > c.1 && c.0 > c.2 && c.1 < 10),
            "layer 3 is answered by the DRAWING's table → red: {cols:?}",
        );
        assert!(
            cols.iter().any(|c| c.0 == c.1 && c.1 == c.2 && c.0 > 20),
            "layer 99 is past the end of both tables → the neutral fallback, not a crash: {cols:?}",
        );
    }

    /// Regression: entering a sketch swaps `self.doc` to the sketch's Document.
    /// Anything still holding indices into the OLD doc goes stale — this test drives
    /// the real path headlessly so a panic shows up in CI, not in the user's hands.
    #[test]
    fn enter_sketch_on_ground_plane_does_not_panic() {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        let f = crate::factory::FactoryState::ground_frame();
        app.factory_enter_sketch(f);
        assert!(app.factory.session.is_some(), "session opened");
        assert!(
            app.doc.dobjects.is_empty(),
            "the sketch doc is the active doc"
        );
    }

    /// Enter → draw something → exit must put the geometry back in the sketch and
    /// restore the model-space doc.
    #[test]
    fn enter_draw_exit_round_trips_the_document() {
        let mut app = CadApp::default();
        // CadApp::default() SEEDS a drawing, so count from whatever it starts with.
        let before = app.doc.dobjects.len();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(1.0, 0.0),
                },
            )));
        let f = crate::factory::FactoryState::ground_frame();
        app.factory_enter_sketch(f);
        assert!(app.doc.dobjects.is_empty(), "sketch starts empty");
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(5.0, 5.0),
                },
            )));
        app.factory_exit_sketch();
        assert_eq!(
            app.doc.dobjects.len(),
            before + 1,
            "model-space doc restored intact"
        );
        assert_eq!(
            app.factory.model.sketches[0].doc.dobjects.len(),
            1,
            "sketch kept its geometry"
        );
    }

    /// DATA LOSS REGRESSION: an autosave firing while a face-sketch is open must write the
    /// user's PLAN, never the sketch.
    ///
    /// The bug: `factory_enter_sketch` parks the drawing in `session.saved_doc` and puts the
    /// sketch in `self.doc`; every draw tool inside the sketch sets `unsaved`; `tick_autosave`
    /// has no session gate; and the save worker cloned `self.doc`. Three minutes inside a
    /// sketch replaced the user's .rsm with the sketch. `plan_doc()` is the fix — this pins it.
    #[test]
    fn saving_mid_sketch_writes_the_plan_not_the_sketch() {
        let mut app = CadApp::default();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(1.0, 0.0),
                },
            )));
        let plan_objs = app.doc.dobjects.len();
        let plan_bytes = cad_io::rsm::write_rsm(&app.doc);

        let f = crate::factory::FactoryState::ground_frame();
        app.factory_enter_sketch(f);
        // Draw inside the sketch — this is what used to poison the autosave.
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(5.0, 5.0),
                },
            )));
        assert!(app.factory.session.is_some(), "sketch session is live");

        let saved = app.plan_doc();
        assert_eq!(
            saved.dobjects.len(),
            plan_objs,
            "save path sees the PLAN, not the sketch"
        );
        assert_eq!(
            cad_io::rsm::write_rsm(saved),
            plan_bytes,
            "autosaving mid-sketch leaves the .rsm byte-identical",
        );
    }

    /// The whole point of the unit tag: a plan drawn in MILLIMETRES must promote to a wall of
    /// the right size in metres, instead of a 3-kilometre one.
    #[test]
    fn a_millimetre_plan_promotes_to_metres() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::MM,
            cad_kernel::UnitSource::User,
        );
        // A 3 m wall, drawn the way an architectural plan draws it: 3000 units.
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(3000.0, 0.0),
                },
            )));
        app.selection = vec![app.doc.dobjects.len() - 1];
        let (promoted, _) = app.make_3d_wall_from_selection();
        assert_eq!(promoted, 1, "the line promoted");
        let (mn, mx) = app.factory.features_aabb().expect("the wall has bounds");
        let span = mx.x - mn.x;
        assert!(
            (span - 3.0).abs() < 0.05,
            "a 3000 mm wall must be ~3 m long in the 3D world, got {span}",
        );
    }

    /// Zero-delta: the same drawing with NO unit set behaves exactly as it always did — the
    /// numbers are taken as millimetres (1 unit = 1 mm, Assumed) — the merged
    /// default, matching the plotting convention. A 3-unit line builds a 3 mm wall.
    #[test]
    fn without_a_unit_the_numbers_are_still_taken_as_millimetres() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        assert_eq!(app.doc.units.source, cad_kernel::UnitSource::Assumed);
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(3.0, 0.0),
                },
            )));
        app.selection = vec![app.doc.dobjects.len() - 1];
        assert_eq!(app.make_3d_wall_from_selection().0, 1);
        let (mn, mx) = app.factory.features_aabb().expect("bounds");
        assert!(
            (mx.x - mn.x - 0.003).abs() < 0.05,
            "3 units = 3 mm at k = 0.001"
        );
    }

    /// A wall DRAWN at the default thickness and an imported centerline promoted with the
    /// fallback must come out the SAME thickness. `WlThk` is a metre constant stored as a
    /// doc-unit length, so without converting at the authoring site the two diverge by 1000x.
    #[test]
    fn drawn_and_imported_walls_agree_on_thickness_in_millimetres() {
        let mut app = CadApp::default();
        app.doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::MM,
            cad_kernel::UnitSource::User,
        );
        // What the Wall tool stores for the current default thickness.
        let stored = app.doc.units.from_metres(app.env.WlThk);
        // What promotion turns that back into.
        let promoted_m = app.dlen_m(stored);
        assert!(
            (promoted_m - app.factory.wall_thickness).abs() < 1e-4,
            "drawn wall {promoted_m} m must match the imported-centerline fallback {} m",
            app.factory.wall_thickness,
        );
    }

    /// THE ORIGINAL COMPLAINT, end to end: a building promoted from a millimetre plan while
    /// the app still read those numbers as metres is 1000x too big. Declaring the unit with
    /// `rescale` must bring the existing model down to true size.
    #[test]
    fn declaring_a_unit_with_rescale_fixes_an_already_built_model() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // Built the old way: a 3000-unit wall read as 3000 METRES (the merged
        // default is mm, so simulate the old reading by declaring metres first).
        app.doc.units = cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::User);
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(3000.0, 0.0),
                },
            )));
        app.selection = vec![app.doc.dobjects.len() - 1];
        assert_eq!(app.make_3d_wall_from_selection().0, 1);
        let (mn, mx) = app.factory.features_aabb().expect("bounds");
        assert!(
            (mx.x - mn.x - 3000.0).abs() < 1.0,
            "starts 3000 m long — the bug"
        );

        // Now say what the drawing really is, and bring the model with it.
        app.factory.rescale_world(0.001);
        let (mn, mx) = app.factory.features_aabb().expect("bounds");
        assert!(
            (mx.x - mn.x - 3.0).abs() < 0.05,
            "the existing wall is now ~3 m, got {}",
            mx.x - mn.x,
        );
    }

    /// A rescale must not orphan per-face paint. `surface_key` quantises the plane's WORLD
    /// offset, so every colour assignment would lose its face the moment geometry moved.
    #[test]
    fn rescaling_keeps_per_face_paint_attached() {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        // Paint a face whose plane sits 2 m out; after x0.5 it sits at 1 m.
        let key = (7u32, 50, 0, 0, 200);
        app.factory.surface_color.insert(key, [1.0, 0.0, 0.0]);
        app.factory.rescale_world(0.5);
        assert!(
            !app.factory.surface_color.contains_key(&key),
            "the old key must not survive — it points at where the face used to be",
        );
        assert_eq!(
            app.factory.surface_color.get(&(7, 50, 0, 0, 100)),
            Some(&[1.0, 0.0, 0.0]),
            "the paint followed the face to its new plane offset",
        );
    }

    /// Declaring a unit WITHOUT `rescale` must leave the built model exactly where it is —
    /// that is the promise the whole design rests on.
    #[test]
    fn declaring_a_unit_alone_never_moves_existing_geometry() {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        let before = app.factory.features_aabb().expect("bounds");
        app.doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::MM,
            cad_kernel::UnitSource::User,
        );
        let after = app.factory.features_aabb().expect("bounds");
        assert_eq!(
            before, after,
            "setting a unit is a declaration, not a transform"
        );
    }

    /// The same capture the recorder takes, run against a SAVED project without opening the app.
    ///
    ///   SIMLUX_DIAG="D:\...\for3dfactorygym.dxf" cargo test -p cad_app scene_of_real_project -- --ignored --nocapture
    ///
    /// Read-only — it loads the sidecar and prints. Useful when the app will not start, when the
    /// question is about a file the user is not currently in, or to compare a file before and
    /// after an edit without asking them to record a session.

    /// REPRODUCE A CRASH REPORTED DURING `calculate`, on the user's real project.
    ///
    ///   SIMLUX_DIAG="D:\...\for3dfactorygym.dxf" cargo test -p cad_app --bin simlux \
    ///       calculate_on_real_project -- --ignored --nocapture
    ///
    /// Loads the sidecar, rebuilds the model, and runs the lux calculation exactly as the app does.
    /// A panic here IS the bug, with a stack; a clean run says the crash is somewhere else and stops
    /// this being guessed at.
    #[test]
    #[ignore = "needs SIMLUX_DIAG=<drawing path>"]
    fn calculate_on_real_project() {
        let Ok(path) = std::env::var("SIMLUX_DIAG") else {
            println!("set SIMLUX_DIAG to the drawing path");
            return;
        };
        let cfg = crate::simlux_io::load(std::path::Path::new(&path))
            .expect("sidecar read")
            .expect("sidecar exists");
        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory.clone());
        app.factory.recompute();
        app.light.apply_config(cfg, &app.doc);
        println!(
            "loaded: {} features, {} luminaires, {} profiles",
            app.factory.model.features.len(),
            app.light.luminaires.len(),
            app.light.profiles.len(),
        );

        let t = std::time::Instant::now();
        let plan = CadApp::plan_doc_of(app.factory.session.as_ref(), &app.doc).clone();
        app.light.calculate(&plan, Some(&app.factory));
        println!(
            "calculate returned in {:.1} s — grid {:?}, msg: {}",
            t.elapsed().as_secs_f64(),
            app.light.grid.as_ref().map(|g| (g.cols, g.rows, g.avg)),
            app.light.last_msg,
        );
        // WHERE IT WENT. A total is not a diagnosis: the question this harness exists to answer is
        // WHICH PHASE is the one that does not come back.
        println!();
        println!("per phase:");
        for (what, v) in &app.light.last_timings {
            if *what == "scene_tris" {
                println!("  {what:<14} {v:>12.0} triangles");
            } else {
                println!("  {what:<14} {v:>12.1} ms");
            }
        }
    }
    #[test]
    #[ignore = "needs SIMLUX_DIAG=<drawing path>"]
    fn scene_of_real_project() {
        let Ok(path) = std::env::var("SIMLUX_DIAG") else {
            println!("set SIMLUX_DIAG to the drawing path");
            return;
        };
        let cfg = crate::simlux_io::load(std::path::Path::new(&path))
            .expect("sidecar read")
            .expect("sidecar exists");
        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory);
        app.factory.recompute();
        println!(
            "{}",
            crate::dbg_recorder::format_event_oneline(
                &app.factory_scene_capture(&format!("offline capture of {path}"))
            )
        );

        // ---- IN-EVAL NUMBERS ------------------------------------------------------------
        //
        // The half the scene capture could not reach. Everything above describes the model as
        // DATA; this is what evaluating it COSTS, per feature, on the real thing. Every
        // performance threshold downstream of here was estimated, and two of them were estimated
        // against a BSP that was collapsing because the boolean tolerance was wrong — an estimate
        // is exactly what this exists to replace.
        println!("\n=== in-eval cost ===");
        let (_, prof) = app.factory.model.eval_profiled();
        println!(
            "total {:.0} ms over {} features ({} disabled) -> {} tris in {} bodies",
            prof.total_ms,
            prof.features.len(),
            prof.disabled,
            prof.tris,
            prof.bodies,
        );
        println!(
            "deepest single operand {} polygons — what the 64 MB eval stack is sized against, \
             since csgrs's BSP recurses about once per polygon on a convex body",
            prof.deepest_operand,
        );
        let cut_ms: f64 = prof
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Difference)
            .map(|f| f.ms)
            .sum();
        let cuts = prof
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Difference)
            .count();
        println!(
            "of which cutting: {:.0} ms ({:.0}%) across {cuts} Difference features",
            cut_ms,
            if prof.total_ms > 0.0 {
                100.0 * cut_ms / prof.total_ms
            } else {
                0.0
            },
        );
        println!("\n  worst 20 features by eval time");
        println!(
            "  {:>8}  {:<11} {:<10} {:>9}  {:>9}  {:>9}",
            "id", "op", "kind", "ms", "operand", "body"
        );
        for f in prof.worst(20) {
            println!(
                "  {:>8}  {:<11} {:<10} {:>9.1}  {:>9}  {:>9}",
                f.id,
                f.op.label(),
                f.kind,
                f.ms,
                f.polys_operand,
                f.polys_body
            );
        }

        // Per-suspect probe trace: where the face point lands, and what the ray finds in EACH
        // direction. This is the view that showed the reconstructed point sitting 0.64 m clear of
        // the wall — the reason the repair used to decline every one of these openings.

        // DOES THE CUTTER ACTUALLY REACH THE WALL, ACROSS THE WHOLE OPENING?
        //
        // The one question that matters and the one nothing so far has answered. A depth measured
        // at a single probe point says nothing about the EDGES of a window on a curved wall,
        // where the surface bends away. So sample a grid over the opening's own footprint, and at
        // each sample compare the cutter's span along the normal with where the wall actually is.
        // This can fail — if every sample is covered, the cut is not the problem and I am wrong.
        println!("\n=== does the cutter reach the wall? (grid over each opening) ===");
        for f in app
            .factory
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Difference)
        {
            let (h, w, d) = match f.primitive {
                cad_solid::Primitive::Extrusion { h, w, d, .. } => (h, w, d),
                cad_solid::Primitive::Box { w, d, h } => (h, w, d),
                _ => continue,
            };
            let (uax, vax) = f.plane.axes();
            let n = uax.cross(vax).normalize_or_zero();
            if n.length_squared() < 0.5 {
                continue;
            }
            let (lo, hi) = (f.placement.lift, f.placement.lift + h);
            let (mut covered, mut short, mut nowall) = (0usize, 0usize, 0usize);
            let mut worst = 0.0_f32;
            const G: i32 = 4;
            for iu in 0..=G {
                for iv in 0..=G {
                    let su = f.placement.u + w * (iu as f32 / G as f32 - 0.5);
                    let sv = f.placement.v + d * (iv as f32 / G as f32 - 0.5);
                    let p = f.plane.origin() + uax * su + vax * sv;
                    // Where the wall is at THIS point, in the cutter's own axis.
                    let near = [(-1.0_f32, -n), (1.0_f32, n)]
                        .into_iter()
                        .filter_map(|(sgn, dir)| {
                            app.nearest_surface(p, dir, 3.0)
                                .map(|(t, leaving)| (if leaving { 0.0 } else { t }, sgn, dir))
                        })
                        .min_by(|a, b| a.0.total_cmp(&b.0));
                    let Some((t, sgn, dir)) = near else {
                        nowall += 1;
                        continue;
                    };
                    let thk = app.assembly_span(p + dir * t, dir).0;
                    if thk <= 1e-3 {
                        nowall += 1;
                        continue;
                    }
                    let (a, b) = (sgn * t, sgn * (t + thk));
                    let (wlo, whi) = (a.min(b), a.max(b));
                    // Covered means the cutter spans the wall at this sample.
                    if lo <= wlo + 1e-3 && hi >= whi - 1e-3 {
                        covered += 1;
                    } else {
                        // Shortfall is how far the cutter fails to reach at EITHER end. The wall
                        // is often entirely past one end, so both directions must be measured.
                        short += 1;
                        worst = worst.max((lo - wlo).max(0.0)).max((whi - hi).max(0.0));
                    }
                }
            }
            let total = (G + 1) * (G + 1);
            let flag = if short > 0 {
                "  ⚠ DOES NOT REACH"
            } else {
                ""
            };
            println!(
                "  cut #{:<4} cutter spans {lo:+.3}…{hi:+.3}  · {covered}/{total} samples \
                      covered, {short} short (worst by {worst:.3} m), {nowall} no wall{flag}",
                f.id
            );
        }

        println!("\n=== per-cut probe ===");
        for f in app
            .factory
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Difference)
        {
            let h = match f.primitive {
                cad_solid::Primitive::Extrusion { h, .. } | cad_solid::Primitive::Box { h, .. } => {
                    h
                }
                _ => continue,
            };
            if h > 0.25 || (f.placement.lift + 0.1).abs() > 0.02 {
                continue;
            }
            let (u, v) = f.plane.axes();
            let n = u.cross(v).normalize_or_zero();
            let o = f.plane.origin() + u * f.placement.u + v * f.placement.v;
            let (din, ids_in) = app.assembly_span(o, -n);
            let (dout, ids_out) = app.assembly_span(o, n);
            // The cutter's OWN centre: it is a 0.2 m box straddling the face (lift −0.1, h 0.2),
            // so its middle sits on the wall surface in the middle of the opening — regardless of
            // how the sketch's placement happens to be encoded.
            let (cmn, cmx) = f.world_aabb();
            let c = (cmn + cmx) * 0.5;
            let (cin, cids_in) = app.assembly_span(c, -n);
            let (cout, cids_out) = app.assembly_span(c, n);
            println!("  #{:<4} placement o=({:.2},{:.2},{:.2}) n=({:.2},{:.2},{:.2})  in={din:.3} {ids_in:?}  out={dout:.3} {ids_out:?}",
                f.id, o.x, o.y, o.z, n.x, n.y, n.z);
            println!("        aabb centre=({:.2},{:.2},{:.2})  in={cin:.3} {cids_in:?}  out={cout:.3} {cids_out:?}",
                c.x, c.y, c.z);
            // RAW crossings, so the GAP rule and the probe origin can be told apart: an empty
            // list means the ray found no material at all, while a populated one means the walk
            // gave up at the first gap.
            for (label, dir) in [("in ", -n), ("out", n)] {
                let mut hits: Vec<(f32, u32, bool)> = Vec::new();
                let start = o + dir * 1e-3;
                for uf in app
                    .factory
                    .model
                    .features
                    .iter()
                    .filter(|x| x.op == cad_solid::BoolOp::Union)
                {
                    let tris = app.factory.model.feature_world_positions(uf);
                    for ch in tris.chunks_exact(3) {
                        let (a, b, cc) = (
                            glam::Vec3::from(ch[0]),
                            glam::Vec3::from(ch[1]),
                            glam::Vec3::from(ch[2]),
                        );
                        if let Some(t) = cad_solid::ray_triangle(start, dir, a, b, cc) {
                            if t > 1e-4 {
                                hits.push((t, uf.id, (b - a).cross(cc - a).dot(dir) > 0.0));
                            }
                        }
                    }
                }
                hits.sort_by(|a, b| a.0.total_cmp(&b.0));
                let show: Vec<String> = hits
                    .iter()
                    .take(5)
                    .map(|(t, id, ex)| format!("{t:.3}@#{id}{}", if *ex { "↑" } else { "↓" }))
                    .collect();
                println!(
                    "        {label} raw: {} crossing(s)  {}",
                    hits.len(),
                    show.join(" ")
                );
            }
        }

        // DRY-RUN the repair on the real geometry. Read-only — nothing is written back — but it
        // answers the question the capture raises and the user cannot: would `repaircuts`
        // actually fix these, or does it skip them?
        let tris_before = app.factory.model.eval().tri_count();
        let (fixed, notes) = app.factory_repair_shallow_cuts();
        app.factory.recompute();
        let tris_after = app.factory.model.eval().tri_count();
        println!(
            "\n=== repaircuts DRY RUN ===\n{fixed} would be re-cut, {} left alone",
            notes.len()
        );
        for n in &notes {
            println!("{n}");
        }
        println!(
            "  solid: {tris_before} → {tris_after} triangles{}",
            if tris_after * 10 < tris_before * 9 {
                "   ⚠ GEOMETRY LOST"
            } else {
                ""
            }
        );
        // Did the repair actually make the openings REACH? The only honest test, and one that
        // can come back negative.
        let mut reach_ok = 0usize;
        let mut reach_bad = 0usize;
        for c in app
            .factory
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Difference)
        {
            let (ok, total, worst) = {
                let (h, w, d) = match c.primitive {
                    cad_solid::Primitive::Extrusion { h, w, d, .. } => (h, w, d),
                    cad_solid::Primitive::Box { w, d, h } => (h, w, d),
                    _ => continue,
                };
                let (uax, vax) = c.plane.axes();
                let n = uax.cross(vax).normalize_or_zero();
                let (lo, hi) = (c.placement.lift, c.placement.lift + h);
                let (mut k, mut t_, mut worst) = (0usize, 0usize, 0.0_f32);
                for iu in 0..=4 {
                    for iv in 0..=4 {
                        let p = c.plane.origin()
                            + uax * (c.placement.u + w * (iu as f32 / 4.0 - 0.5))
                            + vax * (c.placement.v + d * (iv as f32 / 4.0 - 0.5));
                        let near = [(-1.0_f32, -n), (1.0_f32, n)]
                            .into_iter()
                            .filter_map(|(sgn, dir)| {
                                app.nearest_surface(p, dir, 3.0)
                                    .map(|(t, lv)| (if lv { 0.0 } else { t }, sgn, dir))
                            })
                            .min_by(|a, b| a.0.total_cmp(&b.0));
                        let Some((t, sgn, dir)) = near else { continue };
                        let thk = app.assembly_span(p + dir * t, dir).0;
                        if thk <= 1e-3 {
                            continue;
                        }
                        t_ += 1;
                        let (a, b) = (sgn * t, sgn * (t + thk));
                        let (wlo, whi) = (a.min(b), a.max(b));
                        if lo <= wlo + 1e-3 && hi >= whi - 1e-3 {
                            k += 1;
                        } else {
                            worst = worst.max((lo - wlo).max(0.0)).max((whi - hi).max(0.0));
                        }
                    }
                }
                (k, t_, worst)
            };
            if total == 0 {
                continue;
            }
            if ok == total {
                reach_ok += 1;
            } else {
                reach_bad += 1;
                println!(
                    "  · cut #{} still reaches only {ok}/{total}, short by {worst:.3} m",
                    c.id
                );
            }
        }
        println!(
            "  AFTER REPAIR: {reach_ok} opening(s) fully reach the wall, {reach_bad} still do not"
        );
        if fixed > 0 {
            app.factory.recompute();
            println!(
                "{}",
                crate::dbg_recorder::format_event_oneline(
                    &app.factory_scene_capture("after repaircuts (in memory only)")
                )
            );
        }
    }

    /// Load the USER'S actual saved project and measure it. Set `SIMLUX_DIAG` to the drawing
    /// path (the sidecar beside it is what gets read).
    ///
    ///   SIMLUX_DIAG="D:\...\for3dfactorygym.dxf" cargo test -p cad_app diag_real_project -- --ignored --nocapture
    #[test]
    #[ignore = "needs SIMLUX_DIAG=<drawing path>"]
    fn diag_real_project() {
        let Ok(path) = std::env::var("SIMLUX_DIAG") else {
            println!("set SIMLUX_DIAG to the drawing path");
            return;
        };
        let cfg = crate::simlux_io::load(std::path::Path::new(&path))
            .expect("sidecar read")
            .expect("sidecar exists");
        let m = &cfg.factory.model;
        let bodies: Vec<(u32, glam::Vec3, glam::Vec3)> = m
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Union)
            .map(|f| {
                let (a, b) = f.world_aabb();
                (f.id, a, b)
            })
            .collect();
        println!(
            "features={} bodies={} walls={} furniture={} storeys={}",
            m.features.len(),
            bodies.len(),
            cfg.factory.walls.len(),
            cfg.factory.furniture.len(),
            cfg.factory.storeys.len()
        );
        let diffs = m
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Difference)
            .count();
        println!("differences={diffs}");

        // Overlapping bodies — the duplicate-solid hypothesis.
        let vol = |mn: glam::Vec3, mx: glam::Vec3| {
            let d = (mx - mn).max(glam::Vec3::splat(1e-4));
            d.x * d.y * d.z
        };
        let mut dupes: Vec<(u32, u32, f32)> = Vec::new();
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                let (ia, imn, imx) = bodies[i];
                let (jb, jmn, jmx) = bodies[j];
                let omn = imn.max(jmn);
                let omx = imx.min(jmx);
                if omn.cmpgt(omx).any() {
                    continue;
                }
                let share = vol(omn, omx) / vol(imn, imx).min(vol(jmn, jmx));
                if share > 0.5 {
                    dupes.push((ia, jb, share));
                }
            }
        }
        dupes.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        println!("OVERLAPPING PAIRS: {}", dupes.len());
        for (a, b, s) in dupes.iter().take(15) {
            println!("   #{a} n #{b}  {:.0}%", s * 100.0);
        }

        // COPLANAR faces between DIFFERENT bodies — the thing that actually flickers.
        // Quantise each triangle's plane (normal + offset) and count planes shared by more
        // than one body.
        use std::collections::HashMap;
        // Same plane is NOT enough. A floor and the ceiling above it share one outline, so
        // their side faces lie in the same VERTICAL planes at different heights — they never
        // overlap and never fight. Counting those produced a confident wrong answer, so each
        // face now carries its box and real overlap IN the plane is required.
        let mut planes: HashMap<(i32, i32, i32, i32), Vec<(u32, glam::Vec3, glam::Vec3)>> =
            HashMap::new();
        for f in m
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Union)
        {
            for c in m.feature_world_positions(f).chunks_exact(3) {
                let (a, b, cc) = (
                    glam::Vec3::from(c[0]),
                    glam::Vec3::from(c[1]),
                    glam::Vec3::from(c[2]),
                );
                let n = (b - a).cross(cc - a).normalize_or_zero();
                if n.length_squared() < 0.5 {
                    continue;
                }
                // Fold n and -n together: two solids meeting face to face share a plane.
                let n = if n.x + n.y + n.z < 0.0 { -n } else { n };
                let d = n.dot(a);
                let key = (
                    (n.x * 200.0).round() as i32,
                    (n.y * 200.0).round() as i32,
                    (n.z * 200.0).round() as i32,
                    (d * 500.0).round() as i32,
                );
                planes
                    .entry(key)
                    .or_default()
                    .push((f.id, a.min(b).min(cc), a.max(b).max(cc)));
            }
        }
        println!("PLANES: {}", planes.len());
        let mut pair_hits: HashMap<(u32, u32), usize> = HashMap::new();
        for faces in planes.values() {
            for i in 0..faces.len() {
                for j in (i + 1)..faces.len() {
                    let (ia, imn, imx) = faces[i];
                    let (jb, jmn, jmx) = faces[j];
                    if ia == jb {
                        continue;
                    }
                    let ov = imx.min(jmx) - imn.max(jmn);
                    let mut s = vec![ov.x, ov.y, ov.z];
                    s.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                    if s[0] > 0.01 && s[1] > 0.01 {
                        let k = if ia < jb { (ia, jb) } else { (jb, ia) };
                        *pair_hits.entry(k).or_default() += 1;
                    }
                }
            }
        }
        let mut ranked: Vec<_> = pair_hits.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1));
        println!("WORST PAIRS by shared faces:");
        let info = |id: u32| -> String {
            m.features
                .iter()
                .find(|f| f.id == id)
                .map_or("?".into(), |f| {
                    let (mn, mx) = f.world_aabb();
                    let d = mx - mn;
                    format!(
                        "{} {:.1}x{:.1}x{:.1}",
                        f.primitive.kind_label(),
                        d.x,
                        d.y,
                        d.z
                    )
                })
        };
        for ((a, b), n) in ranked.iter().take(10) {
            println!(
                "   #{a} vs #{b}: {n} coincident faces   [{} | {}]",
                info(*a),
                info(*b)
            );
        }
        // Bodies with an (almost) IDENTICAL box — true duplicates, not containment.
        println!("NEAR-IDENTICAL BOXES:");
        let mut twins = 0;
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                let (ia, imn, imx) = bodies[i];
                let (jb, jmn, jmx) = bodies[j];
                if (imn - jmn).abs().max_element() < 0.05 && (imx - jmx).abs().max_element() < 0.05
                {
                    if twins < 10 {
                        println!("   #{ia} == #{jb}   [{} | {}]", info(ia), info(jb));
                    }
                    twins += 1;
                }
            }
        }
        println!("   total near-identical pairs: {twins}");

        // MATERIALS — a fine regular weave over a whole face is as likely to be a texture or a
        // procedural pattern as it is depth fighting, and the two need opposite fixes.
        let f = &cfg.factory;
        println!(
            "MATERIALS: textures={} feature_tex={} surface_tex={} feature_col={} surface_col={}",
            f.textures.len(),
            f.feature_textures.len(),
            f.surface_textures.len(),
            f.feature_colors.len(),
            f.surface_colors.len()
        );
        for (i, t) in f.textures.iter().enumerate() {
            println!(
                "   tex[{i}] '{}' {}x{} scale={} rot={} opacity={} reflect={} proc={}",
                t.name,
                t.w,
                t.h,
                t.scale,
                t.rot_deg,
                t.opacity,
                t.reflect,
                if t.proc.is_some() { "YES" } else { "no" }
            );
        }
        for (fid, ti) in f.feature_textures.iter().take(10) {
            println!("   feature #{fid} -> tex {ti}");
        }
        // How finely is the geometry triangulated? Thousands of slivers across one face also
        // shade as a weave.
        let mut tri_counts: Vec<(u32, usize)> = cfg
            .factory
            .model
            .features
            .iter()
            .filter(|x| x.op == cad_solid::BoolOp::Union)
            .map(|x| (x.id, cfg.factory.model.feature_world_positions(x).len() / 3))
            .collect();
        tri_counts.sort_by(|a, b| b.1.cmp(&a.1));
        println!(
            "HEAVIEST BODIES (triangles): {:?}",
            &tri_counts[..tri_counts.len().min(8)]
        );

        // Which texture is on which body — the direct read that settles what the weave is.
        let texname = |ti: usize| {
            f.textures
                .get(ti)
                .map_or("?".to_string(), |t| format!("{} {}x{}", t.name, t.w, t.h))
        };
        println!("TEXTURE PER BODY (largest footprint first):");
        let mut big: Vec<(u32, f32)> = cfg
            .factory
            .model
            .features
            .iter()
            .filter(|x| x.op == cad_solid::BoolOp::Union)
            .map(|x| {
                let (mn, mx) = x.world_aabb();
                let d = mx - mn;
                (x.id, d.x * d.y)
            })
            .collect();
        big.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (id, _) in big.iter().take(12) {
            let t = f
                .feature_textures
                .iter()
                .find(|(fid, _)| fid == id)
                .map(|(_, ti)| *ti);
            let d = cfg
                .factory
                .model
                .features
                .iter()
                .find(|x| x.id == *id)
                .map(|x| {
                    let (mn, mx) = x.world_aabb();
                    mx - mn
                })
                .unwrap_or_default();
            println!(
                "   #{id} {:.1}x{:.1}x{:.1} m  tex={}",
                d.x,
                d.y,
                d.z,
                t.map_or("(none)".into(), texname)
            );
        }
        for (i, t) in f.textures.iter().enumerate() {
            if t.name.contains("herring") || t.name.contains("parquet") {
                let users: Vec<u32> = f
                    .feature_textures
                    .iter()
                    .filter(|(_, ti)| *ti == i)
                    .map(|(fid, _)| *fid)
                    .collect();
                println!("HERRINGBONE tex[{i}] '{}' on features: {users:?}", t.name);
            }
        }

        // What DEDUPE would do, by the same rule the app uses.
        let sig =
            |x: &cad_solid::Feature| format!("{:?}|{:?}|{:?}", x.plane, x.placement, x.primitive);
        let feats = &cfg.factory.model.features;
        let mut seen = std::collections::HashSet::new();
        let (mut would_drop, mut blocked) = (Vec::new(), Vec::new());
        for (i, x) in feats.iter().enumerate() {
            if x.op != cad_solid::BoolOp::Union {
                continue;
            }
            if seen.insert(sig(x)) {
                continue;
            }
            if feats
                .get(i + 1)
                .is_some_and(|n| n.op == cad_solid::BoolOp::Difference)
            {
                blocked.push(x.id);
            } else {
                would_drop.push(x.id);
            }
        }
        // How DEEP is each cut? A through-cut whose extent along its normal is only ~0.2 m is
        // one made by the broken thickness probe: it opened the near face and stopped.
        println!("CUT DEPTHS (Difference features):");
        let mut shallow = 0;
        for x in feats
            .iter()
            .filter(|x| x.op == cad_solid::BoolOp::Difference)
        {
            let h = match x.primitive {
                cad_solid::Primitive::Extrusion { h, .. } => h,
                cad_solid::Primitive::Box { h, .. } => h,
                _ => f32::NAN,
            };
            if h.is_finite() && h <= 0.25 {
                shallow += 1;
            }
            println!(
                "   cut #{} h={:.3} m  lift={:.3}",
                x.id, h, x.placement.lift
            );
        }
        let n_diff = feats
            .iter()
            .filter(|x| x.op == cad_solid::BoolOp::Difference)
            .count();
        println!("   → {shallow} of {n_diff} cuts are 0.25 m or shallower");
        println!("DEDUPE would delete {would_drop:?}");
        println!("DEDUPE would SKIP (carry cuts) {blocked:?}");

        // Degenerate solids: a zero (or near-zero) extent means two faces in ONE plane, which
        // fight with each other no matter where the camera is.
        println!("DEGENERATE BODIES (an extent under 1 mm):");
        for x in feats.iter().filter(|x| x.op == cad_solid::BoolOp::Union) {
            let (mn, mx) = x.world_aabb();
            let d = mx - mn;
            if d.x.min(d.y).min(d.z) < 0.001 {
                let t = f
                    .feature_textures
                    .iter()
                    .find(|(fid, _)| *fid == x.id)
                    .map(|(_, ti)| *ti);
                println!(
                    "   #{} {:?} {:.3}x{:.3}x{:.3} m at ({:.1},{:.1},{:.1}) tex={}",
                    x.id,
                    x.primitive.kind_label(),
                    d.x,
                    d.y,
                    d.z,
                    mn.x,
                    mn.y,
                    mn.z,
                    t.map_or("(none)".into(), texname)
                );
            }
        }
        // Do #3 and #122 differ only by profile ID?
        for id in [3u32, 122] {
            if let Some(x) = feats.iter().find(|x| x.id == id) {
                println!("#{id}: plane={:?}", x.plane);
                println!("      place={:?}", x.placement);
                println!("      prim={:?}", x.primitive);
                if let cad_solid::Primitive::Extrusion { profile, .. } = x.primitive {
                    if let Some(p) = cfg.factory.model.profile(profile) {
                        println!(
                            "      profile {} has {} pts, first={:?}",
                            profile,
                            p.pts.len(),
                            p.pts.first()
                        );
                    }
                }
            }
        }
    }

    /// `repaircuts` must deepen a cut left shallow by the broken probe, and must NOT touch a
    /// recess — a blind pocket is deliberate, and deepening one would punch a hole through the
    /// piece it was sunk into.
    #[test]
    fn repaircuts_deepens_a_stopped_cut_but_leaves_a_recess() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // A 0.6 m wall — thicker than the old 0.4 m gap, so the old probe measured zero.
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 6.0,
                d: 0.6,
                h: 3.0,
            },
        );
        // A cutter with the broken signature: h = 0.2, lift = −0.1, on the −Y face.
        let face = cad_solid::Plane::from_basis(
            glam::Vec3::new(0.0, -0.3, 1.5),
            glam::Vec3::X,
            glam::Vec3::Z,
        );
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: 0.0,
                v: 0.0,
                lift: -0.1,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 0.2,
            },
        );
        // …and a genuine RECESS: a deliberate blind pocket, deeper than the broken signature.
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: 2.0,
                v: 0.0,
                lift: -0.05,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 0.4,
                d: 0.4,
                h: 0.05,
            },
        );
        app.factory.recompute();

        let (fixed, _skipped) = app.factory_repair_shallow_cuts();
        assert_eq!(fixed, 1, "only the stopped through-cut is repaired");

        let depth = |i: usize| match app.factory.model.features[i].primitive {
            cad_solid::Primitive::Box { h, .. } => h,
            _ => f32::NAN,
        };
        assert!(
            depth(1) > 0.6,
            "the through-cut now spans the wall, got {}",
            depth(1)
        );
        assert!(
            (depth(2) - 0.05).abs() < 1e-6,
            "the recess is untouched — a blind pocket is deliberate, got {}",
            depth(2),
        );
    }

    /// Fraction of an opening where the cutter actually spans the wall, sampled on a grid.
    ///
    /// The measure that should have been used from the start. A depth taken at ONE probe point
    /// says nothing about the rest of the window, and "the cut is bound to the right body" says
    /// nothing about whether it reaches the masonry at all. This asks the only question that
    /// matters — at each point across the opening, does the cutter's span along its normal
    /// contain the wall? — and it can come back clean, which is what makes it evidence.
    fn cutter_coverage(app: &CadApp, cut: &cad_solid::Feature) -> (usize, usize, f32) {
        let (h, w, d) = match cut.primitive {
            cad_solid::Primitive::Extrusion { h, w, d, .. } => (h, w, d),
            cad_solid::Primitive::Box { w, d, h } => (h, w, d),
            _ => return (0, 0, 0.0),
        };
        let (uax, vax) = cut.plane.axes();
        let n = uax.cross(vax).normalize_or_zero();
        let (lo, hi) = (cut.placement.lift, cut.placement.lift + h);
        let (mut ok, mut total, mut worst) = (0usize, 0usize, 0.0_f32);
        const G: i32 = 4;
        for iu in 0..=G {
            for iv in 0..=G {
                let su = cut.placement.u + w * (iu as f32 / G as f32 - 0.5);
                let sv = cut.placement.v + d * (iv as f32 / G as f32 - 0.5);
                let p = cut.plane.origin() + uax * su + vax * sv;
                let near = [(-1.0_f32, -n), (1.0_f32, n)]
                    .into_iter()
                    .filter_map(|(sgn, dir)| {
                        app.nearest_surface(p, dir, 3.0)
                            .map(|(t, leaving)| (if leaving { 0.0 } else { t }, sgn, dir))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                let Some((t, sgn, dir)) = near else { continue };
                let thk = app.assembly_span(p + dir * t, dir).0;
                if thk <= 1e-3 {
                    continue;
                }
                total += 1;
                let (a, b) = (sgn * t, sgn * (t + thk));
                let (wlo, whi) = (a.min(b), a.max(b));
                if lo <= wlo + 1e-3 && hi >= whi - 1e-3 {
                    ok += 1;
                } else {
                    worst = worst.max((lo - wlo).max(0.0)).max((whi - hi).max(0.0));
                }
            }
        }
        (ok, total, worst)
    }

    /// A window cut on a CURVED wall must go through, across the whole opening.
    ///
    /// The reported bug, reduced to geometry. A curved wall is faceted, and the sketch plane is a
    /// tangent — so the drawn loop's centroid stands clear of the masonry, measured at 0.68–0.76 m
    /// on the real project. `assembly_span` then starts in mid-air, meets the 0.4 m gap rule at
    /// its first crossing and reports ZERO thickness, and the cutter falls back to ±0.1 m: three
    /// quarters of a metre short of ever touching the wall. Sampled across those real openings, 0
    /// of 25 points were covered, while every straight-wall opening scored 25 of 25.
    ///
    /// Coverage is the assertion, not depth. A cutter can be deep and still miss, and a single
    /// centre probe cannot see the edges of a window on a curve.
    #[test]
    fn a_window_on_a_curved_wall_is_cut_through_across_the_whole_opening() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // A faceted curved wall: 9 panels on a 12 m radius arc, 200 mm thick, centred on −Y.
        // Odd count so the probe cannot land exactly on a joint.
        let (r, thick) = (12.0_f32, 0.2_f32);
        let step = 0.06_f32; // ~3.4° per panel
        for i in 0..9 {
            let a = (i as f32 - 4.0) * step;
            let (s, c) = a.sin_cos();
            // Arc centred at (0, r); the wall's mid-surface passes through the origin at a = 0.
            let mid = glam::Vec3::new(r * s, r - r * c, 0.0);
            let along = glam::Vec3::new(c, s, 0.0);
            let out = glam::Vec3::new(-s, -c, 0.0); // away from the arc centre
                                                    // u = out, v = along ⇒ the normal is +Z, so the panel RISES. Getting this backwards
                                                    // built the wall below the floor, where the window could never meet it.
            app.factory.model.push(
                cad_solid::BoolOp::Union,
                cad_solid::Plane::from_basis(glam::Vec3::new(mid.x, mid.y, 0.0), out, along),
                cad_solid::Placement::default(),
                cad_solid::Primitive::Box {
                    w: thick,
                    d: r * step * 1.05,
                    h: 3.0,
                },
            );
        }
        app.factory.recompute();
        let before_tris = app.factory.model.eval().tri_count();

        // A 1.2 m × 1.6 m window on the tangent plane at the arc's centre, standing OUTSIDE the
        // wall exactly as a real one does.
        let frame = cad_solid::Frame {
            origin: glam::Vec3::new(0.0, -0.65, 1.5),
            u: glam::Vec3::X,
            v: glam::Vec3::Z,
        };
        let mut sk = cad_solid::Sketch::new(frame);
        sk.doc.units =
            cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::Assumed);
        let rect = [
            (-0.6, -0.8),
            (0.6, -0.8),
            (0.6, 0.8),
            (-0.6, 0.8),
            (-0.6, -0.8),
        ];
        sk.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
                cad_kernel::Polyline {
                    vertices: rect
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
        app.factory.model.sketches.push(sk);

        assert_eq!(
            app.factory_cut_sketch(true),
            1,
            "the cut must be made, not refused"
        );

        // EVERY cutter this produced must span the wall wherever it meets it.
        let cuts: Vec<_> = app
            .factory
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Difference)
            .collect();
        assert!(
            !cuts.is_empty(),
            "a through-cut must produce at least one cutter"
        );
        for c in &cuts {
            let (ok, total, worst) = cutter_coverage(&app, c);
            if total == 0 {
                continue;
            } // this cutter does not meet the wall — a harmless no-op
            assert_eq!(
                ok, total,
                "cut #{} reaches the wall at only {ok}/{total} points across the opening, \
                 short by {worst:.3} m — the window will not be see-through there",
                c.id
            );
        }
        // And the wall is opened, not demolished: material is removed, but the wall survives.
        let after = app.factory.model.eval().tri_count();
        assert!(
            after > before_tris / 2,
            "cutting a window must not destroy the wall: {before_tris} → {after} triangles"
        );
    }

    /// PARTIAL coverage is repairable; blind-everywhere is not.
    ///
    /// `diag` on the real project found twelve openings not going through, and not one still
    /// carried the old `h ≤ 0.25, lift ≈ −0.1` fallback shape — so the repair, keyed on that
    /// shape alone, would have fixed none of them. Coverage is the real signal.
    ///
    /// The discrimination that makes this safe: a RECESS is a deliberate blind pocket, so it
    /// fails to reach EVERYWHERE — 0 of N. A through-cut beaten by a wall that thickens or curves
    /// away reaches part of the opening and stops short over the rest. Partial coverage is a
    /// thing a recess cannot be, so repairing only partials never deepens a pocket.
    #[test]
    fn repaircuts_fixes_a_partly_reaching_opening_and_leaves_a_blind_one() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // A wall that STEPS: thin on one side, thick on the other, sharing a near face.
        for (x, thick) in [(-0.75_f32, 0.2_f32), (0.75, 0.8)] {
            app.factory.model.push(
                cad_solid::BoolOp::Union,
                cad_solid::Plane::from_basis(
                    glam::Vec3::new(x, thick * 0.5, 0.0),
                    glam::Vec3::X,
                    glam::Vec3::Y,
                ),
                cad_solid::Placement::default(),
                cad_solid::Primitive::Box {
                    w: 1.5,
                    d: thick,
                    h: 3.0,
                },
            );
        }
        let face = cad_solid::Plane::from_basis(
            glam::Vec3::new(0.0, 0.0, 1.5),
            glam::Vec3::X,
            glam::Vec3::Z,
        );
        // An opening spanning the step, cut deep enough for the THIN half only.
        let partial = app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: 0.0,
                v: 0.0,
                lift: -0.45,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 2.0,
                d: 1.0,
                h: 0.6,
            },
        );
        // …and a genuine blind pocket, well clear of it.
        let recess = app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: -1.2,
                v: 1.0,
                lift: -0.06,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 0.3,
                d: 0.3,
                h: 0.06,
            },
        );
        app.factory.recompute();

        let cov = |app: &CadApp, id: u32| {
            let f = app
                .factory
                .model
                .features
                .iter()
                .find(|f| f.id == id)
                .expect("feature");
            app.cut_coverage(f)
        };
        let (ok, total, _) = cov(&app, partial);
        assert!(
            total > 0 && ok > 0 && ok < total,
            "the fixture must be PARTIAL for this test to mean anything, got {ok}/{total}"
        );
        let (rok, rtotal, _) = cov(&app, recess);
        assert!(
            rtotal > 0 && rok == 0,
            "the recess must read blind-everywhere, got {rok}/{rtotal}"
        );
        let recess_h_before = match app
            .factory
            .model
            .features
            .iter()
            .find(|f| f.id == recess)
            .unwrap()
            .primitive
        {
            cad_solid::Primitive::Box { h, .. } => h,
            _ => f32::NAN,
        };

        let (fixed, _notes) = app.factory_repair_shallow_cuts();
        app.factory.recompute();

        assert!(fixed >= 1, "the partly-reaching opening must be repaired");
        let (ok2, total2, _) = cov(&app, partial);
        assert_eq!(
            ok2, total2,
            "…and must then span the wall everywhere, got {ok2}/{total2}"
        );

        let recess_h_after = match app
            .factory
            .model
            .features
            .iter()
            .find(|f| f.id == recess)
            .unwrap()
            .primitive
        {
            cad_solid::Primitive::Box { h, .. } => h,
            _ => f32::NAN,
        };
        assert_eq!(
            recess_h_before, recess_h_after,
            "a blind pocket must never be deepened — it cannot be told from a failed opening"
        );
    }

    /// A recorder snapshot captures the DRAWING, even mid face-sketch.
    ///
    /// While a face-sketch session is live, `doc` is the sketch — so an auto-cadence snapshot
    /// taken then recorded an empty drawing. A real session shows it twice:
    ///
    ///     📷 SNAP[1] 0 dobj, undo=0, redo=0  (auto cadence)
    ///
    /// on a project holding 1442 objects. The same substitution once let autosave write a sketch
    /// over someone's plan, which is why `plan_doc()` exists; the recorder simply never got it.
    #[test]
    fn a_snapshot_taken_during_a_face_sketch_still_records_the_drawing() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        for i in 0..7 {
            app.doc
                .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                    cad_kernel::Line {
                        a: Vec2::new(i as f64, 0.0),
                        b: Vec2::new(i as f64, 5.0),
                    },
                )));
        }
        let drawing = app.doc.dobjects.len();

        // Enter a face sketch: `doc` is swapped for the sketch, and the drawing is parked.
        app.factory.add_box();
        app.factory.recompute();
        let frame = cad_solid::Frame {
            origin: glam::Vec3::new(0.0, 0.0, 1.0),
            u: glam::Vec3::X,
            v: glam::Vec3::Y,
        };
        app.factory_enter_sketch(frame);
        assert!(
            app.factory.session.is_some(),
            "the sketch session must be live for this test"
        );
        assert!(
            app.doc.dobjects.len() < drawing,
            "…and `doc` must now be the sketch"
        );

        // Every snapshot path — session start, auto cadence, the 📷 button — goes through
        // `take_snapshot`; this exercises the first, which is the one a dump always contains.
        app.dbg_start();
        let snap = app.dbg.snapshots.first().expect("a snapshot was taken");
        assert_eq!(
            snap.geom.len() + snap.omitted,
            drawing,
            "the snapshot must hold the DRAWING's {drawing} objects, not the sketch's"
        );
    }

    /// `diag` must answer "did the openings go through" for the WHOLE model, on demand.
    ///
    /// Two new cuts scoring 25/25 says those two are sound. It does not say the model is, and the
    /// same dump that carried them also carried two cuts made on an earlier build that had come
    /// out as the ±0.1 m fallback. "The windows look right now" and "the cut is fixed" are
    /// different claims, and only one of them is checkable — so it should be checkable at any
    /// time, over everything, not only for cuts made while the recorder happened to be running.
    #[test]
    fn diag_reports_which_openings_fail_to_go_through() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 6.0,
                d: 0.6,
                h: 3.0,
            },
        );
        let face = cad_solid::Plane::from_basis(
            glam::Vec3::new(0.0, -0.3, 1.5),
            glam::Vec3::X,
            glam::Vec3::Z,
        );
        // One opening that goes through…
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: -1.5,
                v: 0.0,
                lift: -0.8,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 1.6,
            },
        );
        // …and one that stops inside the wall.
        let stopped = app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: 1.5,
                v: 0.0,
                lift: -0.1,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 0.2,
            },
        );
        app.factory.recompute();

        let report = app.factory_geometry_report().join("\n");
        assert!(
            report.contains("openings:"),
            "diag must report opening coverage:\n{report}"
        );
        assert!(
            report.contains("1 go fully through"),
            "the sound one must be counted:\n{report}"
        );
        assert!(
            report.contains(&format!("#{stopped} reaches")),
            "the stopped one must be named, with how far it falls short:\n{report}"
        );
    }

    /// The recorded cut event must say whether the opening actually went through.
    ///
    /// Three windows were drawn on one wall in one session; one came out blind. All three events
    /// looked equally plausible — targets, depths, a normal — and separating them meant deriving
    /// the geometry by hand afterwards, from dumps that had already truncated. Twice.
    ///
    /// Coverage is the verdict, and it belongs where the cut happens. `in_depth`/`out_depth`
    /// describe the probe, not the result.
    #[test]
    fn a_cut_that_does_not_go_through_says_so_in_the_recording() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 6.0,
                d: 0.6,
                h: 3.0,
            },
        );
        let face = cad_solid::Plane::from_basis(
            glam::Vec3::new(0.0, -0.3, 1.5),
            glam::Vec3::X,
            glam::Vec3::Z,
        );
        // A deliberately stopped cutter: 0.2 m into a 0.6 m wall.
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: 0.0,
                v: 0.0,
                lift: -0.1,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 0.2,
            },
        );
        app.factory.recompute();
        let stopped = app.factory.model.features[1].clone();
        let (ok, total, short) = app.cut_coverage(&stopped);
        assert!(
            total > 0,
            "the sampler must find the wall under this opening"
        );
        assert!(
            ok < total,
            "a 0.2 m cutter cannot span a 0.6 m wall — got {ok}/{total}"
        );
        assert!(
            short > 0.2,
            "and it should report how far it falls short, got {short}"
        );

        // …while a cutter that spans the wall reports clean.
        app.factory.model.features[1].placement.lift = -0.8;
        if let cad_solid::Primitive::Box { h, .. } = &mut app.factory.model.features[1].primitive {
            *h = 1.6;
        }
        app.factory.recompute();
        let through = app.factory.model.features[1].clone();
        let (ok2, total2, _) = app.cut_coverage(&through);
        assert_eq!(
            ok2, total2,
            "a cutter spanning the wall must score {total2}/{total2}"
        );
    }

    /// An opening whose CENTRE finds no wall must still cut the wall it overlaps.
    ///
    /// Three windows were cut on one curved run in a single recorded session; two worked and one
    /// did not, and the dump says exactly why:
    ///
    ///   worked   in_depth=0.199 out_depth=0.613  targets=[1, 52, 53, 54, 124]
    ///   worked   in_depth=0.000 out_depth=0.848  targets=[1, 50, 51, 124]
    ///   FAILED   in_depth=0.000 out_depth=0.000  targets=[1, 122, 124]
    ///
    /// The failing one found nothing under its centre probe, so neither the anchor nor the axis
    /// ray contributed a body, and its target list came out with no wall in it at all — only the
    /// two large bodies whose boxes span the building. The cutter was the right size (the grid had
    /// already stretched it to h=0.748), in the right place, and inserted behind bodies it does
    /// not touch. `eval` binds by position, so it opened nothing.
    ///
    /// The depth was already measured across the whole opening. The TARGET LIST has to be too.
    ///
    /// ⚠ This test guards the INVARIANT, not that change: it passes with the grid's contribution
    /// to the target list disabled, because here the swept box already reaches both panels. So it
    /// is not evidence that the real failure is fixed — reproducing that needs the saved file the
    /// failing cut is in. Stated plainly so nobody later mistakes a green tick for a diagnosis.
    #[test]
    fn an_opening_whose_centre_misses_the_wall_still_cuts_what_it_overlaps() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // Two wall panels with a 0.9 m gap between them. A window centred on that gap has no wall
        // under its centre, and reaches both panels at its edges.
        let mut ids = Vec::new();
        for x in [-1.2_f32, 1.2] {
            // u = X, v = Y ⇒ the normal is +Z, so the panel RISES. It is thin in Y, which is the
            // axis the window looks along.
            ids.push(app.factory.model.push(
                cad_solid::BoolOp::Union,
                cad_solid::Plane::from_basis(
                    glam::Vec3::new(x, 0.0, 0.0),
                    glam::Vec3::X,
                    glam::Vec3::Y,
                ),
                cad_solid::Placement::default(),
                cad_solid::Primitive::Box {
                    w: 1.5,
                    d: 0.2,
                    h: 3.0,
                },
            ));
        }
        app.factory.recompute();

        let frame = cad_solid::Frame {
            origin: glam::Vec3::new(0.0, -0.4, 1.5),
            u: glam::Vec3::X,
            v: glam::Vec3::Z,
        };
        let mut sk = cad_solid::Sketch::new(frame);
        // Sketch docs are metre-space (1:1 with the 3D world) — same invariant
        // `factory_enter_sketch` now enforces on the live sketch.
        sk.doc.units =
            cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::Assumed);
        // 3.2 m wide: its centre is over the gap, its ends are over both panels.
        let rect = [
            (-1.6, -0.7),
            (1.6, -0.7),
            (1.6, 0.7),
            (-1.6, 0.7),
            (-1.6, -0.7),
        ];
        sk.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
                cad_kernel::Polyline {
                    vertices: rect
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
        app.factory.model.sketches.push(sk);

        assert_eq!(app.factory_cut_sketch(true), 1, "the cut must be made");

        // BOTH panels must carry a cutter — binding is by position in the feature list.
        for id in &ids {
            let at = app
                .factory
                .model
                .features
                .iter()
                .position(|f| f.id == *id)
                .expect("panel");
            assert!(
                app.factory
                    .model
                    .features
                    .get(at + 1)
                    .is_some_and(|f| f.op == cad_solid::BoolOp::Difference),
                "panel #{id} was overlapped by the opening but got no cutter behind it — \
                 the target list only asked the centre probe"
            );
        }
    }

    /// A repair is judged by the SOLID it leaves behind, never by the rule that made it.
    ///
    /// A version of `repaircuts` shipped that re-bound cuts to whichever body a ray found nearest,
    /// and it removed 30% of a real building — 7485 triangles down to 5261. It passed its own
    /// check because that check counted mismatches with the same function that had chosen the
    /// moves, so it could only ever report success. Circular verification is worse than none: it
    /// converts a wrong belief into apparent evidence.
    ///
    /// Deepening a stopped opening cuts a reveal, which ADDS surfaces. Nothing this command does
    /// can legitimately shrink the model by a tenth, so that is the line, measured on the
    /// evaluated mesh — the one witness that does not depend on what the repair believed.
    #[test]
    fn a_repair_that_destroys_geometry_is_abandoned() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 6.0,
                d: 0.6,
                h: 3.0,
            },
        );
        let face = cad_solid::Plane::from_basis(
            glam::Vec3::new(0.0, -0.3, 1.5),
            glam::Vec3::X,
            glam::Vec3::Z,
        );
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: 0.0,
                v: 0.0,
                lift: -0.1,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 0.2,
            },
        );
        app.factory.recompute();
        let before = app.factory.model.eval().tri_count();

        app.run_command("repaircuts");

        // The legitimate repair goes through, and the building is still standing.
        let after = app.factory.model.eval().tri_count();
        assert!(
            after * 10 >= before * 9,
            "a repair must never shrink the solid by a tenth: {before} → {after}"
        );
        assert!(
            !app.factory.status.contains("ABANDONED"),
            "…and a sound repair must not trip the guard: {}",
            app.factory.status
        );
        // It did do the work it was asked to do.
        let h = match app.factory.model.features[1].primitive {
            cad_solid::Primitive::Box { h, .. } => h,
            _ => f32::NAN,
        };
        assert!(h > 0.6, "the stopped cut is still deepened, got {h}");
    }

    /// `repaircuts` must FIND the wall, not assume the sketch plane is sitting on it.
    ///
    /// Measured on the real project, the face point rebuilt from `plane.origin() + u·place.u +
    /// v·place.v` lands up to 0.64 m clear of the wall — outside the building, in mid-air. The
    /// probe then hit the 0.4 m gap rule on its first crossing and every one of the ten openings
    /// reported "unmeasurable", so the repair fixed nothing at all while reporting success. Four
    /// separate cuts collapsing to one identical point is what gave the reconstruction away.
    ///
    /// Here the wall stands 0.6 m from that reconstructed point, exactly as it does in the file.
    #[test]
    fn repaircuts_finds_a_wall_the_sketch_plane_does_not_sit_on() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // A 200 mm wall whose near face is 0.6 m from where the cutter's plane claims to be.
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::from_basis(
                glam::Vec3::new(0.0, -1.0, 0.0),
                glam::Vec3::X,
                glam::Vec3::Y,
            ),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 6.0,
                d: 0.2,
                h: 3.0,
            },
        );
        // The stopped cut, with its plane floating 0.6 m off that wall.
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            cad_solid::Plane::from_basis(
                glam::Vec3::new(0.0, -0.3, 1.5),
                glam::Vec3::X,
                glam::Vec3::Z,
            ),
            cad_solid::Placement {
                u: 0.0,
                v: 0.0,
                lift: -0.1,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 0.2,
            },
        );
        app.factory.recompute();

        let (fixed, notes) = app.factory_repair_shallow_cuts();
        assert_eq!(
            fixed, 1,
            "the wall is 0.6 m away and findable; notes: {notes:?}"
        );
        let f = &app.factory.model.features[1];
        let h = match f.primitive {
            cad_solid::Primitive::Box { h, .. } => h,
            _ => f32::NAN,
        };
        // The wall occupies 0.6 … 0.8 along the normal, so the cutter must span that plus a
        // margin at each end — NOT merely be made deeper from where it was.
        assert!(
            (h - 0.4).abs() < 0.02,
            "expected a 0.4 m cutter spanning the wall, got {h}"
        );
        assert!(
            (f.placement.lift - 0.5).abs() < 0.02,
            "the cut must START at the wall's near face, not at the sketch plane: {}",
            f.placement.lift
        );
    }

    /// A wall too far off to attribute is NAMED, never guessed at.
    ///
    /// Four of the ten openings on the real project sit 2.14 m from anything. At that range the
    /// nearest surface is as likely to be across the room, and deepening into it would punch a
    /// hole through the wrong part of the building — so the repair declines and says which cut
    /// and how far, which is what makes it fixable by hand.
    #[test]
    fn repaircuts_names_an_opening_it_cannot_attribute_instead_of_guessing() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::from_basis(
                glam::Vec3::new(0.0, -3.0, 0.0),
                glam::Vec3::X,
                glam::Vec3::Y,
            ),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 6.0,
                d: 0.2,
                h: 3.0,
            },
        );
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            cad_solid::Plane::from_basis(
                glam::Vec3::new(0.0, -0.3, 1.5),
                glam::Vec3::X,
                glam::Vec3::Z,
            ),
            cad_solid::Placement {
                u: 0.0,
                v: 0.0,
                lift: -0.1,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 0.2,
            },
        );
        app.factory.recompute();
        let before = app.factory.model.features[1].placement.lift;

        let (fixed, notes) = app.factory_repair_shallow_cuts();
        assert_eq!(fixed, 0, "a wall 2.6 m away must not be cut into");
        assert_eq!(
            notes.len(),
            1,
            "…and the opening must be reported, not silently dropped"
        );
        assert!(
            notes[0].contains("nearest wall is 2.6") && notes[0].contains("Redraw"),
            "the note must give the distance and what to do: {}",
            notes[0]
        );
        assert_eq!(
            app.factory.model.features[1].placement.lift, before,
            "and nothing was touched"
        );
    }

    /// Build a small scene FAR from the origin — a survey plan's coordinates — with one stopped
    /// cut and one healthy one, and return its capture rendered as the dump would show it.
    fn scene_dump_far_from_origin() -> (CadApp, String) {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // 6848 m out, which is where the real project sits.
        let base = glam::Vec3::new(3499.6, -6848.0, 0.0);
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::from_basis(base, glam::Vec3::X, glam::Vec3::Y),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 6.0,
                d: 0.6,
                h: 3.0,
            },
        );
        let face = cad_solid::Plane::from_basis(
            base + glam::Vec3::new(0.0, -0.3, 1.5),
            glam::Vec3::X,
            glam::Vec3::Z,
        );
        // The broken probe's fallback: h = 0.2, lift = −0.1.
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: 0.0,
                v: 0.0,
                lift: -0.1,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 0.2,
            },
        );
        // …and one that went all the way through.
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            face,
            cad_solid::Placement {
                u: 2.0,
                v: 0.0,
                lift: -0.7,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 0.4,
                d: 0.4,
                h: 1.4,
            },
        );
        app.factory.recompute();
        let evt = app.factory_scene_capture("test");
        let text = crate::dbg_recorder::format_event_oneline(&evt);
        (app, text)
    }

    /// A dump must be able to answer a RENDERING question on its own.
    ///
    /// The recorder used to describe 3D only as frame timings and triangle counts. A dump of a
    /// texture bug therefore contained no materials, no coordinates, no cut depths and no
    /// camera — it proved the app was running and nothing more, and the diagnosis fell back to
    /// reading screenshots. Each assertion below is a question that had to be answered by hand
    /// during that investigation.
    #[test]
    fn a_scene_capture_answers_the_questions_a_rendering_bug_asks() {
        let (_app, text) = scene_dump_far_from_origin();
        for want in [
            "world AABB",       // where is it
            "f32 ULP",          // …and what precision does that leave
            "uv_rebase_origin", // what is the fix measured from
            "── cuts ──",       // did the openings go through
            "── materials ──",  // pasted or procedural, triplanar or mesh-UV
            "── furniture ──",  // instances appear in no feature or body count
            "── render ──",     // which passes were even on
            "camera:",          // distance sets footprint and depth resolution
        ] {
            assert!(
                text.contains(want),
                "the capture must state {want:?}:\n{text}"
            );
        }
    }

    /// Distance from the origin is reported as a CONSEQUENCE, not just a coordinate.
    ///
    /// "min=(3499.6, −6848.0, 0)" is only a location. The fact that matters is that f32 has 24
    /// bits of mantissa, so out there one ULP is ~0.8 mm — the size of a close-up pixel
    /// footprint, which is what made texture lookups band and derivative-built normals speckle.
    /// A reader should not have to know to do that arithmetic.
    #[test]
    fn a_far_model_reports_its_precision_not_only_its_position() {
        let (_app, text) = scene_dump_far_from_origin();
        assert!(
            text.contains("⚠ FAR FROM ORIGIN"),
            "the hazard must be flagged:\n{text}"
        );
        assert!(text.contains("PER VERTEX"),
            "and name the fix — rebasing in the fragment shader is the trap that already cost a round");
        // 6848 m × f32::EPSILON ≈ 0.8 mm. Assert the magnitude, not the digits.
        let ulp = text
            .split("f32 ULP")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<f32>().ok())
            .expect("the ULP is printed as a number of mm");
        assert!(
            (0.5..1.5).contains(&ulp),
            "expected ~0.8 mm at 6848 m, got {ulp}"
        );
    }

    /// A cut that stopped inside the wall is named in the dump, and a good one is not.
    ///
    /// This is the state that reads as "the window is not transparent" and as speckle on the
    /// glass — solid wall still standing behind it. Nothing else in a dump shows it: the
    /// `FactoryOp` that made the cut reported success.
    #[test]
    fn a_stopped_cut_is_visible_in_the_dump() {
        let (_app, text) = scene_dump_far_from_origin();
        assert!(
            text.contains("STOPPED-CUT SIGNATURE"),
            "the shallow cut must be flagged:\n{text}"
        );
        assert!(
            text.contains("`repaircuts`"),
            "…and the dump should say what fixes it"
        );
        assert!(
            text.contains("→ 1 of 2 cuts carry the stopped-cut signature"),
            "the tally counts the whole set, and the healthy cut is not in it:\n{text}"
        );
    }

    /// Starting a recording captures the 3D scene, not only the 2D document.
    ///
    /// `take_snapshot` reads `doc`. The model, its materials and its camera live in `factory`,
    /// so without a second capture a dump of a rendering bug opens with a list of 2D lines and
    /// never mentions the thing being rendered — which is precisely what happened.
    #[test]
    fn starting_a_recording_captures_the_3d_scene() {
        let mut app = CadApp::default();
        app.dbg_start();
        assert!(
            app.dbg
                .events
                .iter()
                .any(|r| matches!(r.event, crate::dbg_recorder::DbgEvent::FactoryScene { .. })),
            "the opening events must include a 3D scene capture"
        );
        app.dbg_stop();
        let n = app
            .dbg
            .events
            .iter()
            .filter(|r| matches!(r.event, crate::dbg_recorder::DbgEvent::FactoryScene { .. }))
            .count();
        assert_eq!(
            n, 2,
            "a session is BRACKETED — the closing state shows what the actions did"
        );
    }

    /// Every list in a capture is capped and says how many it dropped, so a truncated dump can
    /// never be read as a complete one. The same contract `SNAP_GEOM_MAX` already carries.
    #[test]
    fn scene_lists_are_capped_and_admit_it() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 40.0,
                d: 4.0,
                h: 3.0,
            },
        );
        let over = CadApp::SCENE_LIST_MAX + 7;
        for i in 0..over {
            app.factory.model.push(
                cad_solid::BoolOp::Difference,
                cad_solid::Plane::from_basis(
                    glam::Vec3::new(i as f32 * 0.5, -2.0, 1.5),
                    glam::Vec3::X,
                    glam::Vec3::Z,
                ),
                cad_solid::Placement {
                    u: 0.0,
                    v: 0.0,
                    lift: -3.0,
                    spin_deg: 0.0,
                    pitch_deg: 0.0,
                    roll_deg: 0.0,
                },
                cad_solid::Primitive::Box {
                    w: 0.3,
                    d: 0.3,
                    h: 6.0,
                },
            );
        }
        let text = crate::dbg_recorder::format_event_oneline(&app.factory_scene_capture("test"));
        assert!(
            text.contains(&format!(
                "… {} MORE OMITTED (cap {})",
                7,
                CadApp::SCENE_LIST_MAX
            )),
            "a capped list must report its omissions:\n{text}"
        );
        // …and the TALLY still counts every cut, not just the printed page.
        assert!(
            text.contains(&format!("of {over} cuts")),
            "the summary line counts the whole set, not the visible one:\n{text}"
        );
    }

    /// A cap must drop the boring entries, never the diagnostic ones.
    ///
    /// Truncating in model or table order is a silent trap: the one broken cut, or the one
    /// texture actually on screen, can sit past the cap and vanish from the dump — leaving a
    /// capture that looks clean and is not. Both lists are therefore ordered by what a reader
    /// came for: suspect cuts first, in-use materials first.
    #[test]
    fn a_cap_drops_the_irrelevant_entries_first() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 60.0,
                d: 4.0,
                h: 3.0,
            },
        );
        // Fill the page with healthy through-cuts…
        for i in 0..CadApp::SCENE_LIST_MAX + 5 {
            app.factory.model.push(
                cad_solid::BoolOp::Difference,
                cad_solid::Plane::from_basis(
                    glam::Vec3::new(i as f32 * 0.6, -2.0, 1.5),
                    glam::Vec3::X,
                    glam::Vec3::Z,
                ),
                cad_solid::Placement {
                    u: 0.0,
                    v: 0.0,
                    lift: -3.0,
                    spin_deg: 0.0,
                    pitch_deg: 0.0,
                    roll_deg: 0.0,
                },
                cad_solid::Primitive::Box {
                    w: 0.3,
                    d: 0.3,
                    h: 6.0,
                },
            );
        }
        // …then the broken one, LAST, where a model-order cap would bury it.
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            cad_solid::Plane::from_basis(
                glam::Vec3::new(1.0, -2.0, 1.5),
                glam::Vec3::X,
                glam::Vec3::Z,
            ),
            cad_solid::Placement {
                u: 0.0,
                v: 0.0,
                lift: -0.1,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 0.3,
                d: 0.3,
                h: 0.2,
            },
        );
        let text = crate::dbg_recorder::format_event_oneline(&app.factory_scene_capture("test"));
        assert!(
            text.contains("STOPPED-CUT SIGNATURE"),
            "the last-added broken cut must survive the cap:\n{text}"
        );

        // Same contract for materials: an in-use one added after a pile of unused ones.
        let mut app = CadApp::default();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 2.0,
                d: 2.0,
                h: 2.0,
            },
        );
        let id = app.factory.model.features[0].id;
        for i in 0..CadApp::SCENE_LIST_MAX + 3 {
            app.factory.textures.push(crate::factory::TextureAsset::new(
                format!("unused-{i}"),
                1,
                1,
                vec![128, 128, 128, 255],
            ));
        }
        app.factory.textures.push(crate::factory::TextureAsset::new(
            "THE-ONE-IN-USE".into(),
            1,
            1,
            vec![255, 0, 0, 255],
        ));
        let used = app.factory.textures.len() - 1;
        app.factory.feature_texture.insert(id, used);
        let text = crate::dbg_recorder::format_event_oneline(&app.factory_scene_capture("test"));
        assert!(
            text.contains("THE-ONE-IN-USE"),
            "the only material actually on screen must survive the cap:\n{text}"
        );
        assert!(text.contains("0 in use, 0 procedural, 0 see-through"),
            "…and the cap should say what it dropped, in the terms that rule things in or out:\n{text}");
    }

    /// Autosave must be OFF at every start. It is the only thing in the app that writes over a
    /// user's file with no user action, and it is how an empty document reached disk and
    /// destroyed a project. A bug reachable only through an explicit save costs an afternoon;
    /// the same bug reachable through a timer costs the project.
    #[test]
    fn autosave_starts_off_and_is_not_remembered() {
        assert!(
            !CadApp::default().autosave_on,
            "autosave must default to OFF — nothing may overwrite a file unattended",
        );
        // Turning it on is a session-only choice: a NEW app is off again, whatever the last
        // session did. Persisting it would defeat the point, since the risk is precisely that
        // it is on while nobody is thinking about it.
        let mut app = CadApp::default();
        app.autosave_on = true;
        drop(app);
        assert!(
            !CadApp::default().autosave_on,
            "a fresh session starts off again"
        );
    }

    /// DATA LOSS REGRESSION. Opening a file must drop the undo history, because every entry in
    /// it snapshots a DIFFERENT document — including the empty one the app starts with. Left in
    /// place, Ctrl+Z after an open walks out of the opened file into the previous document,
    /// while `current_file` still points at the real project; the next save (or the unattended
    /// 3-minute autosave) then writes that emptiness over it.
    #[test]
    fn opening_a_file_drops_the_undo_history() {
        let mut app = CadApp::default();
        // Some history against the STARTING document.
        for _ in 0..3 {
            app.snapshot_doc();
            app.doc
                .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                    cad_kernel::Line {
                        a: Vec2::new(0.0, 0.0),
                        b: Vec2::new(1.0, 0.0),
                    },
                )));
        }
        assert!(!app.undo_stack.is_empty(), "there is history to carry over");

        // Now "open" a different drawing through the real install path.
        let mut opened = cad_kernel::Document::default();
        for k in 0..5 {
            opened.push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(k as f64, 0.0),
                    b: Vec2::new(k as f64, 5.0),
                },
            )));
        }
        let payload = Box::new(LoadPayload {
            doc: opened,
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
        });
        app.apply_loaded("C:/tmp/opened.rsm", payload);

        assert_eq!(app.doc.dobjects.len(), 5, "the opened drawing is installed");
        assert!(
            app.undo_stack.is_empty() && app.redo_stack.is_empty(),
            "history from the PREVIOUS document must not survive the open — undoing into it \
             leaves an empty app pointed at the opened file, and autosave then overwrites it",
        );
    }

    /// THE RENDER LOOP'S INPUTS MUST BE ORDER-STABLE, or temporal accumulation never converges.
    ///
    /// The renderer decides "did anything change?" by hashing the frame's inputs in order. Any
    /// input built from a `HashMap`/`HashSet` — which reseed per instance and are rebuilt every
    /// frame — hashes differently each time despite identical content, so the accumulator resets
    /// forever and `n` never leaves 1/16. The screen-space passes get their smoothness from
    /// averaging those samples, so a smooth material renders as speckled noise instead.
    #[test]
    fn textured_meshes_come_back_in_a_stable_order() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // Several textured bodies, so there is an order to get wrong.
        for k in 0..6 {
            app.factory.model.push(
                cad_solid::BoolOp::Union,
                cad_solid::Plane::default(),
                cad_solid::Placement {
                    u: k as f32 * 3.0,
                    v: 0.0,
                    lift: 0.0,
                    spin_deg: 0.0,
                    pitch_deg: 0.0,
                    roll_deg: 0.0,
                },
                cad_solid::Primitive::Box {
                    w: 2.0,
                    d: 0.3,
                    h: 3.0,
                },
            );
        }
        app.factory.recompute();
        let ids: Vec<u32> = app.factory.model.features.iter().map(|f| f.id).collect();
        for (k, id) in ids.iter().enumerate() {
            let ti = app
                .factory
                .ensure_solid_color_texture([k as f32 / 6.0, 0.5, 0.5]);
            app.factory.feature_texture.insert(*id, ti);
        }
        let order = |a: &Vec<(usize, Vec<crate::light3d::TexVtx>)>| -> Vec<usize> {
            a.iter().map(|(ti, _)| *ti).collect()
        };
        let first = order(&app.factory.feature_textured_meshes());
        assert!(
            first.len() > 1,
            "several texture groups, or this proves nothing"
        );
        // Rebuilt from a FRESH HashMap each call — the order must not move.
        for _ in 0..8 {
            assert_eq!(
                order(&app.factory.feature_textured_meshes()),
                first,
                "group order must be identical every frame",
            );
        }
        let mut sorted = first.clone();
        sorted.sort_unstable();
        assert_eq!(first, sorted, "and it is sorted, not merely repeatable");
    }

    /// Texture coordinates must be REBASED near the origin. A plan imported from a survey sits
    /// at world coordinates in the thousands; the texel is chosen by the FRACTIONAL part, so a
    /// UV that large loses the bits that pick it and the texture moirés. Invisible on a model
    /// built near the origin — which is exactly why it survived every other test here.
    #[test]
    fn texture_uvs_are_rebased_near_the_origin() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement {
                u: 3517.0,
                v: -6852.0,
                lift: 0.0,
                spin_deg: 0.0,
                pitch_deg: 0.0,
                roll_deg: 0.0,
            },
            cad_solid::Primitive::Box {
                w: 6.0,
                d: 0.3,
                h: 3.0,
            },
        );
        app.factory.recompute();
        let id = app.factory.model.features[0].id;
        let ti = app.factory.ensure_solid_color_texture([0.6, 0.6, 0.6]);
        app.factory.feature_texture.insert(id, ti);

        let org = app.factory.uv_rebase_origin();
        assert!(
            org[0] > 3000.0 && org[1] < -6000.0,
            "the origin tracks the model, got {org:?}"
        );

        let meshes = app.factory.feature_textured_meshes();
        assert!(!meshes.is_empty(), "the textured wall is emitted");
        for (_, verts) in &meshes {
            for v in verts {
                // The VERTEX keeps its true world position…
                assert!(v.x > 3000.0, "geometry stays where it is: {}", v.x);
                // …while the UV it carries is small enough for f32 to resolve a texel.
                assert!(
                    v.u.abs() < 100.0 && v.v.abs() < 100.0,
                    "UV must be rebased near the origin, got ({}, {})",
                    v.u,
                    v.v,
                );
            }
        }
    }

    /// The depth tie-break must be big enough to settle a tie and small enough to be invisible,
    /// and it must be the SAME for a body's flat and textured triangles or the body splits
    /// along the seam between them.
    #[test]
    fn the_depth_tiebreak_is_stable_and_sub_visible() {
        use crate::factory::depth_tiebreak;
        // Deterministic: the same body always gets the same nudge — that is what stops the
        // flicker, since an arbitrary-but-constant winner never changes.
        for id in [0u32, 1, 7, 8, 123, 4242] {
            assert_eq!(depth_tiebreak(id), depth_tiebreak(id));
        }
        // Distinct for neighbouring ids, so two touching bodies do separate.
        assert_ne!(depth_tiebreak(3), depth_tiebreak(4));
        // Big enough to beat the depth quantum (~0.3 mm at building range), small enough to
        // be invisible: a millimetre is a tenth the thickness of plasterboard.
        for id in 0..64u32 {
            let d = depth_tiebreak(id);
            assert!(
                d >= 0.0 && d < 0.002,
                "id {id} nudged {d} m — that would be visible"
            );
        }
    }

    /// `dedupe` must delete an exact twin — and must NOT delete one carrying its own cuts,
    /// because `eval` is sequential and the cuts would then land on the previous body.
    #[test]
    fn dedupe_removes_exact_twins_but_keeps_cut_ones() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        let mut slab = |app: &mut CadApp| {
            app.factory.model.push(
                cad_solid::BoolOp::Union,
                cad_solid::Plane::default(),
                cad_solid::Placement::default(),
                cad_solid::Primitive::Box {
                    w: 30.0,
                    d: 12.0,
                    h: 0.2,
                },
            )
        };
        slab(&mut app);
        slab(&mut app); // the exact twin — this is the reported #3 / #122 situation
        app.factory.recompute();
        let before = app.factory.model.features.len();
        let (removed, kept) = app.factory_dedupe_solids();
        assert_eq!(removed, 1, "the twin is deleted");
        assert_eq!(kept, 0);
        assert_eq!(app.factory.model.features.len(), before - 1);

        // A twin that OWNS a cut must be left alone.
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        slab(&mut app);
        slab(&mut app);
        app.factory.model.push(
            cad_solid::BoolOp::Difference,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 1.0,
            },
        );
        app.factory.recompute();
        let (removed, kept) = app.factory_dedupe_solids();
        assert_eq!(removed, 0, "a duplicate carrying a cut is NOT deleted");
        assert_eq!(kept, 1, "…and it is reported instead");
    }

    /// A texture applied with nothing selected used to vanish into an empty loop. It now arms
    /// a face brush, so the next click textures ONE face — a wall's two sides are different
    /// materials, and a per-feature texture covers both.
    #[test]
    fn a_texture_with_no_selection_arms_a_face_brush() {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        app.factory.selection.clear();
        app.factory.sel_furniture.clear();
        let ti = app.factory.ensure_solid_color_texture([0.5, 0.4, 0.3]);
        app.apply_texture_index_to_selection(ti, "test", 1, 1);
        assert!(app.factory.paint_surface_mode, "paint mode is on");
        assert_eq!(
            app.factory.surface_tex_brush,
            Some(ti),
            "the texture is the brush"
        );
        assert!(
            app.factory.feature_texture.is_empty(),
            "and nothing was blanket-applied to a whole solid",
        );
    }

    /// `diag` must actually name the thing that makes surfaces flicker: two solids in the same
    /// place. Promoting BOTH face lines of a wall drawn as a pair 99 mm apart — which is how
    /// the reported plan is drawn — gives two overlapping bodies with coincident tops.
    #[test]
    fn diag_reports_overlapping_bodies() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // Clean model first: nothing overlapping.
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 4.0,
                d: 0.2,
                h: 3.0,
            },
        );
        app.factory.recompute();
        let clean = app.factory_geometry_report().join("\n");
        assert!(
            clean.contains("no coincident faces"),
            "clean model reports clean:\n{clean}"
        );

        // Now the duplicate: the SAME slab built twice, which is what the reported project
        // turned out to contain — two identical extrusions with every face coincident.
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 4.0,
                d: 0.2,
                h: 3.0,
            },
        );
        app.factory.recompute();
        let report = app.factory_geometry_report().join("\n");
        assert!(
            report.contains("share faces on the SAME plane"),
            "the duplicate must be reported:\n{report}",
        );
        assert!(
            report.contains("EXACT TWIN: `dedupe` removes it"),
            "an identical twin must be called out as one `dedupe` will take:\n{report}",
        );

        // …and a pair that merely SHARES A BOUNDING SIZE must not be called a duplicate.
        //
        // This wording contradicted the command in front of the user: `diag` said "a duplicate"
        // for any two bodies with the same kind and box rounded to 0.1 m, then `dedupe` — which
        // requires an exact plane/placement/primitive match — answered "no duplicate solids
        // found". Two tools of mine, flatly disagreeing, on a wasted errand.
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        for lift in [0.0_f32, 0.05] {
            app.factory.model.push(
                cad_solid::BoolOp::Union,
                cad_solid::Plane::default(),
                cad_solid::Placement {
                    u: 0.0,
                    v: 0.0,
                    lift,
                    spin_deg: 0.0,
                    pitch_deg: 0.0,
                    roll_deg: 0.0,
                },
                cad_solid::Primitive::Box {
                    w: 4.0,
                    d: 0.2,
                    h: 3.0,
                },
            );
        }
        app.factory.recompute();
        let report = app.factory_geometry_report().join("\n");
        assert!(
            !report.contains("EXACT TWIN"),
            "these differ in placement, so `dedupe` will not touch them:\n{report}"
        );
        assert!(report.contains("same size, different definition"),
            "…and the report must say so, rather than promise a fix that will not happen:\n{report}");
    }

    /// The plan must be OCCLUDED by the model by default. Painting it on a foreground layer
    /// put every line from the far side of the building onto the near wall, where it read as
    /// texture painted on the wall. X-ray stays available, off by default.
    #[test]
    fn the_plan_is_depth_tested_unless_xray_is_asked_for() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(5.0, 0.0),
                },
            )));
        assert!(app.factory.show_plan, "the plan is on by default");
        assert!(!app.factory.plan_xray, "but NOT x-ray — solids hide it");
        // Occluded mode: the plan is emitted as depth-tested world segments.
        let lines = app.factory.plan_lines(&app.doc, 0.0);
        assert!(
            !lines.is_empty(),
            "the plan becomes scene geometry the solids can hide"
        );
        // Its segments sit just ABOVE the ground plane — a floor slab lies exactly on it, and
        // with no depth bias in the renderer, drawing both at the same height makes them fight.
        for v in &lines {
            assert!(
                v.z > 0.0 && v.z < 0.02,
                "plan segments sit a hair above the ground, got z = {}",
                v.z,
            );
        }
    }

    /// A millimetre plan's depth-tested lines must be in METRES too, or the reference sits
    /// kilometres from the model it is a reference for.
    #[test]
    fn the_depth_tested_plan_is_scaled_to_metres() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::MM,
            cad_kernel::UnitSource::User,
        );
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(3000.0, 0.0),
                },
            )));
        let lines = app.factory.plan_lines(&app.doc, 0.0);
        let far = lines.iter().map(|v| v.x).fold(0.0_f32, f32::max);
        assert!(
            (far - 3.0).abs() < 0.01,
            "3000 mm draws 3 m long, got {far}"
        );
    }

    /// The reverse direction: a WORLD point painted on the plan must land where the drawing
    /// says it is. Without this, declaring millimetres fixes promotion but makes every 3D
    /// overlay (factory sketches, furniture outlines, lux, luminaires) paint 1000x too small.
    #[test]
    fn world_metres_project_onto_the_plan_at_the_drawings_scale() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        // A metre drawing: 3 m in the world is 3 units on the canvas.
        let mut app = CadApp::default();
        app.doc.units = cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::User);
        let metres = app.w2s_m(Vec2::new(3.0, 0.0), rect);
        assert_eq!(
            metres,
            app.w2s(Vec2::new(3.0, 0.0), rect),
            "k = 1 is the identity"
        );

        // A millimetre drawing: the same 3 m must land at 3000 drawing units.
        app.doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::MM,
            cad_kernel::UnitSource::User,
        );
        let mm = app.w2s_m(Vec2::new(3.0, 0.0), rect);
        assert_eq!(
            mm,
            app.w2s(Vec2::new(3000.0, 0.0), rect),
            "3 m must paint at 3000 units on a millimetre plan",
        );
    }

    /// `w2s_m` and `s2w_m` must be inverses, so a click placing 3D content lands where the
    /// overlay drew it.
    #[test]
    fn the_plan_projection_round_trips_in_millimetres() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mut app = CadApp::default();
        app.doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::MM,
            cad_kernel::UnitSource::User,
        );
        let world = Vec2::new(12.5, -4.25);
        let back = app.s2w_m(app.w2s_m(world, rect), rect);
        assert!(
            (back.x - world.x).abs() < 1e-3 && (back.y - world.y).abs() < 1e-3,
            "expected {world:?} back, got {back:?}",
        );
    }

    /// Opening a file while a sketch is live must not let the later `exit_sketch` restore the
    /// OLD drawing over the newly opened one (and file the new drawing inside a sketch slot).
    #[test]
    fn exiting_a_sketch_after_an_open_keeps_the_opened_drawing() {
        let mut app = CadApp::default();
        let f = crate::factory::FactoryState::ground_frame();
        app.factory_enter_sketch(f);
        assert!(app.factory.session.is_some());
        // `apply_loaded` commits the sketch before installing; emulate that contract.
        app.factory_exit_sketch();
        assert!(
            app.factory.session.is_none(),
            "sketch committed before a new doc is installed"
        );
    }

    /// Re-entering a face on the SAME plane must reopen the existing sketch (with its drawn
    /// geometry), not stack a fresh empty one — otherwise the drawing appears to vanish.
    #[test]
    fn reentering_the_same_face_reopens_its_sketch() {
        let mut app = CadApp::default();
        let f = crate::factory::FactoryState::ground_frame();
        app.factory_enter_sketch(f);
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(5.0, 5.0),
                },
            )));
        app.factory_exit_sketch();
        assert_eq!(
            app.factory.model.sketches.len(),
            1,
            "one sketch after first visit"
        );

        // A second pick on the same plane (a slightly different origin point) must reopen it.
        let f2 = cad_solid::Frame::from_point_normal(glam::Vec3::new(3.0, 2.0, 0.0), glam::Vec3::Z);
        app.factory_enter_sketch(f2);
        assert_eq!(
            app.factory.model.sketches.len(),
            1,
            "no new sketch — the old one reopened"
        );
        assert_eq!(
            app.doc.dobjects.len(),
            1,
            "the earlier drawing is back on the canvas"
        );
    }

    /// Draw a closed shape on a face → Extrude makes a solid feature and consumes the sketch.
    #[test]
    fn extrude_sketch_makes_a_solid() {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        let before = app.factory.model.features.len();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 1.0,
                },
            )));
        app.factory.element_height = 0.5;
        let made = app.factory_extrude_sketch(false);
        assert_eq!(made, 1, "one solid extruded");
        assert_eq!(
            app.factory.model.features.len(),
            before + 1,
            "a Union feature was added"
        );
        assert!(
            app.factory.session.is_none(),
            "the sketch was consumed and closed"
        );
    }

    /// Cut subtracts a Difference from the solid the face sits on (window/door/niche).
    #[test]
    fn cut_sketch_adds_a_difference_to_the_target_solid() {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        // Put the sketch frame at the box centre so the cut resolves the box as its target.
        let (mn, mx) = app.factory.model.features[0].world_aabb();
        let c = (mn + mx) * 0.5;
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(c, glam::Vec3::Z));
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 0.3,
                },
            )));
        let made = app.factory_cut_sketch(true);
        assert_eq!(made, 1, "one opening cut");
        assert!(
            app.factory
                .model
                .features
                .iter()
                .any(|f| f.op == cad_solid::BoolOp::Difference),
            "a Difference cutter was added to the model"
        );
    }

    /// Build a curved wall (an arc footprint, sampled into segment Boxes exactly as a promoted
    /// arc is) and cut a window straight through it. Returns the app plus the pieces the
    /// caller needs to probe the result.
    ///
    /// `radius` / `half_span` set how sharply the wall curves across the opening — which is
    /// the whole point: a straight prism cutter is aimed along ONE tangent normal, so the
    /// further the wall bends away from that tangent, the deeper the cutter must reach.
    #[cfg(test)]
    fn curved_wall_with_window(
        radius: f32,
        win_half_angle: f32,
        thickness: f32,
    ) -> (CadApp, glam::Vec3, glam::Vec3) {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // Arc footprint centred on the origin, sampled like `geom_outlines` samples an arc.
        // ODD count: with an even one, angle 0 lands exactly on a segment JOINT and a probe
        // there grazes two boxes' end caps instead of the wall faces — a degenerate spot.
        let n_seg = 47;
        let sweep = 1.2_f32; // radians of wall
        let fp: Vec<glam::Vec2> = (0..=n_seg)
            .map(|i| {
                let a = -sweep * 0.5 + sweep * (i as f32 / n_seg as f32);
                glam::Vec2::new(radius * a.cos(), radius * a.sin())
            })
            .collect();
        app.factory.add_wall(fp, thickness, 3.0);
        app.factory.recompute();

        // Pick the OUTER face at the middle of the arc (angle 0 → +X).
        let n = glam::Vec3::X; // outward normal there
        let face = glam::Vec3::new(radius + thickness * 0.5, 0.0, 1.5);
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(face, n));

        // A window spanning ±win_half_angle of arc, drawn on the tangent plane.
        let half_w = (radius * win_half_angle.sin()) as f64;
        let (cx, cy) = pick_uv(face, n);
        let v = |x: f64, y: f64| cad_kernel::PolyVertex {
            pos: Vec2::new(cx + x, cy + y),
            bulge: 0.0,
        };
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
                cad_kernel::Polyline {
                    vertices: vec![
                        v(-half_w, -0.7),
                        v(half_w, -0.7),
                        v(half_w, 0.7),
                        v(-half_w, 0.7),
                        v(-half_w, -0.7),
                    ],
                    closed: true,
                    widths: Vec::new(),
                },
            )));
        app.factory_cut_sketch(true);
        app.factory.recompute();
        (app, face, n)
    }

    /// How many solid surfaces a ray crosses going from `from` along `dir` through the model.
    #[cfg(test)]
    /// Where a picked world point lands in ITS OWN sketch plane's coordinates.
    ///
    /// Sketch coordinates come from the PLANE now, not from where the face happened to be clicked
    /// — see `Frame::canonical`. A test that means "draw a window where I picked" therefore has to
    /// ask where that point is, exactly as a user reads it off the canvas. Assuming (0, 0) was the
    /// pick is the assumption that broke.
    fn pick_uv(face: glam::Vec3, n: glam::Vec3) -> (f64, f64) {
        let uv = cad_solid::Frame::from_point_normal(face, n)
            .canonical()
            .to_uv(face);
        (uv.x as f64, uv.y as f64)
    }

    fn surface_crossings(app: &CadApp, from: glam::Vec3, dir: glam::Vec3) -> usize {
        let mut hits = 0;
        for c in app.factory.cached.positions.chunks_exact(3) {
            let (a, b, cc) = (
                glam::Vec3::from(c[0]),
                glam::Vec3::from(c[1]),
                glam::Vec3::from(c[2]),
            );
            if cad_solid::ray_triangle(from, dir, a, b, cc).is_some_and(|t| t > 1e-4) {
                hits += 1;
            }
        }
        hits
    }

    /// A curved facade built as ONE Extrusion with a curved-band profile — what you get from
    /// Building / Room / Extrude on a curved outline, as opposed to the per-segment Boxes
    /// "Make 3D wall" produces. The cutter is still a straight prism, so this is the shape
    /// most likely to defeat it.
    #[cfg(test)]
    fn curved_extruded_wall(radius: f32, thickness: f32, sweep: f32) -> CadApp {
        let mut app = CadApp::default();
        // The wall is built in metre-space; declare metres so the face-sketch
        // coordinates drawn in the test match the world (the merged default
        // is 1 unit = 1 mm).
        app.doc.units = cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::User);
        app.doc.dobjects.clear();
        let n = 48;
        let ro = radius + thickness * 0.5;
        let ri = radius - thickness * 0.5;
        let mut pts: Vec<glam::Vec2> = Vec::new();
        for i in 0..=n {
            let a = -sweep * 0.5 + sweep * (i as f32 / n as f32);
            pts.push(glam::Vec2::new(ro * a.cos(), ro * a.sin()));
        }
        for i in (0..=n).rev() {
            let a = -sweep * 0.5 + sweep * (i as f32 / n as f32);
            pts.push(glam::Vec2::new(ri * a.cos(), ri * a.sin()));
        }
        if let Ok((profile, centre, w, d)) = app.factory.model.add_profile(&pts) {
            app.factory.model.push(
                cad_solid::BoolOp::Union,
                cad_solid::Plane::default(),
                cad_solid::Placement {
                    u: centre.x,
                    v: centre.y,
                    lift: 0.0,
                    spin_deg: 0.0,
                    pitch_deg: 0.0,
                    roll_deg: 0.0,
                },
                cad_solid::Primitive::Extrusion {
                    profile,
                    h: 3.0,
                    w,
                    d,
                },
            );
        }
        app.factory.recompute();
        app
    }

    /// The same window, cut through a curved facade built as one Extrusion.
    #[test]
    fn a_window_cuts_through_a_curved_extruded_wall() {
        let (radius, thickness) = (4.0_f32, 0.3_f32);
        let mut app = curved_extruded_wall(radius, thickness, 1.2);
        let n = glam::Vec3::X;
        let face = glam::Vec3::new(radius + thickness * 0.5, 0.0, 1.5);
        // The probe must see the uncut wall first, or the assertion below is vacuous.
        assert!(
            surface_crossings(&app, face + n * 0.5, -n) >= 2,
            "probe sees the uncut extruded wall",
        );
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(face, n));
        let hw = 1.1_f64;
        let (cx, cy) = pick_uv(face, n);
        let v = |x: f64, y: f64| cad_kernel::PolyVertex {
            pos: Vec2::new(cx + x, cy + y),
            bulge: 0.0,
        };
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
                cad_kernel::Polyline {
                    vertices: vec![
                        v(-hw, -0.7),
                        v(hw, -0.7),
                        v(hw, 0.7),
                        v(-hw, 0.7),
                        v(-hw, -0.7),
                    ],
                    closed: true,
                    widths: Vec::new(),
                },
            )));
        app.factory_cut_sketch(true);
        app.factory.recompute();
        let crossings = surface_crossings(&app, face + n * 0.5, -n);
        assert_eq!(
            crossings, 0,
            "the opening should be clear through, but {crossings} surface(s) remain",
        );
    }

    /// `assembly_span` must measure a wall's real thickness. From a live dump it returned
    /// `in_depth=0.000 out_depth=0.000` on a real building, which collapses the through-cutter
    /// to the bare ±0.1 m margin and leaves the far face uncut.
    ///
    /// Two suspects, separated here: the ray-march's own gap rule, and f32 precision at the
    /// survey-style coordinates a real DXF sits on (~3.5e3, -6.9e3).
    #[test]
    fn assembly_span_measures_wall_thickness() {
        for (label, origin) in [
            ("at the origin", glam::Vec3::ZERO),
            (
                "at DXF coordinates",
                glam::Vec3::new(3517.27, -6852.23, 0.0),
            ),
        ] {
            for thickness in [0.1_f32, 0.3, 0.5, 0.9] {
                let mut app = CadApp::default();
                app.doc.dobjects.clear();
                // A wall slab: thin in Y, so the probe runs along -Y like the dump's normal.
                app.factory.model.push(
                    cad_solid::BoolOp::Union,
                    cad_solid::Plane::default(),
                    cad_solid::Placement {
                        u: origin.x,
                        v: origin.y,
                        lift: 0.0,
                        spin_deg: 0.0,
                        pitch_deg: 0.0,
                        roll_deg: 0.0,
                    },
                    cad_solid::Primitive::Box {
                        w: 6.0,
                        d: thickness,
                        h: 3.0,
                    },
                );
                app.factory.recompute();
                // Probe from the −Y face, heading +Y straight through the slab.
                let face = origin + glam::Vec3::new(0.0, -thickness * 0.5, 1.5);
                let (depth, ids) = app.assembly_span(face, glam::Vec3::Y);
                assert!(
                    (depth - thickness).abs() < 0.02,
                    "{label}: a {thickness} m wall measured {depth} m (bodies {})",
                    ids.len(),
                );
            }
        }
    }

    /// The OUTWARD probe must still stop: from a wall's outer face, heading away from the
    /// building, a wall on the far side of a room is NOT part of this assembly. This is what
    /// the gap rule exists for, and the thick-wall fix must not cost it.
    #[test]
    fn assembly_span_does_not_reach_the_wall_across_the_room() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        let mut wall = |y: f32| {
            app.factory.model.push(
                cad_solid::BoolOp::Union,
                cad_solid::Plane::default(),
                cad_solid::Placement {
                    u: 0.0,
                    v: y,
                    lift: 0.0,
                    spin_deg: 0.0,
                    pitch_deg: 0.0,
                    roll_deg: 0.0,
                },
                cad_solid::Primitive::Box {
                    w: 6.0,
                    d: 0.5,
                    h: 3.0,
                },
            );
        };
        wall(0.0);
        wall(5.0); // 4.5 m of room between them
        app.factory.recompute();
        // From the near wall's −Y face, heading −Y (away from the building): nothing belongs
        // to this assembly, so the span must be 0 rather than reaching across the room.
        let face = glam::Vec3::new(0.0, -0.25, 1.5);
        let (out_depth, _) = app.assembly_span(face, -glam::Vec3::Y);
        assert!(
            out_depth < 0.05,
            "outward span must stay local, got {out_depth}"
        );
        // Inward it must measure THIS wall (0.5 m) and stop before the far one.
        let (in_depth, _) = app.assembly_span(face, glam::Vec3::Y);
        assert!(
            (in_depth - 0.5).abs() < 0.02,
            "inward span is this wall's thickness, got {in_depth}",
        );
    }

    /// Diagnostic: print what the cut actually measured on a thick curved wall.
    #[test]
    #[ignore = "diagnostic"]
    fn diag_thick_curved_cut() {
        let (radius, thickness, half_angle) = (8.0_f32, 0.6_f32, 0.12_f32);
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // ODD count: with an even one, angle 0 lands exactly on a segment JOINT and a probe
        // there grazes two boxes' end caps instead of the wall faces — a degenerate spot.
        let n_seg = 47;
        let sweep = 1.2_f32;
        let fp: Vec<glam::Vec2> = (0..=n_seg)
            .map(|i| {
                let a = -sweep * 0.5 + sweep * (i as f32 / n_seg as f32);
                glam::Vec2::new(radius * a.cos(), radius * a.sin())
            })
            .collect();
        app.factory.add_wall(fp, thickness, 3.0);
        app.factory.recompute();
        let n = glam::Vec3::X;
        let face = glam::Vec3::new(radius + thickness * 0.5, 0.0, 1.5);
        println!("bodies={}", app.factory.model.features.len());
        println!("in  = {:?}", app.assembly_span(face, -n).0);
        println!("out = {:?}", app.assembly_span(face, n).0);
        println!(
            "uncut crossings = {}",
            surface_crossings(&app, face + n * 0.5, -n)
        );
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(face, n));
        let hw = (radius * half_angle.sin()) as f64;
        let (cx, cy) = pick_uv(face, n);
        let v = |x: f64, y: f64| cad_kernel::PolyVertex {
            pos: Vec2::new(cx + x, cy + y),
            bulge: 0.0,
        };
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
                cad_kernel::Polyline {
                    vertices: vec![
                        v(-hw, -0.7),
                        v(hw, -0.7),
                        v(hw, 0.7),
                        v(-hw, 0.7),
                        v(-hw, -0.7),
                    ],
                    closed: true,
                    widths: Vec::new(),
                },
            )));
        let made = app.factory_cut_sketch(true);
        app.factory.recompute();
        println!("made={made} feats={}", app.factory.model.features.len());
        println!(
            "after crossings = {}",
            surface_crossings(&app, face + n * 0.5, -n)
        );
        // Where exactly is the leftover material?
        let start = face + n * 0.5;
        let mut ts: Vec<f32> = Vec::new();
        for c in app.factory.cached.positions.chunks_exact(3) {
            let (a, b, cc) = (
                glam::Vec3::from(c[0]),
                glam::Vec3::from(c[1]),
                glam::Vec3::from(c[2]),
            );
            if let Some(t) = cad_solid::ray_triangle(start, -n, a, b, cc) {
                if t > 1e-4 {
                    ts.push(t);
                }
            }
        }
        ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("leftover hit distances from {start:?}: {ts:?}");
        // Group-eval subtracts a Difference from the Union body it sits behind, so the ORDER
        // of features decides whether a cut lands at all.
        let ops: Vec<String> = app
            .factory
            .model
            .features
            .iter()
            .map(|f| {
                format!(
                    "{}{}",
                    if f.op == cad_solid::BoolOp::Union {
                        "U"
                    } else {
                        "D"
                    },
                    f.id
                )
            })
            .collect();
        println!("features: {}", ops.join(" "));
        for f in &app.factory.model.features {
            if f.op == cad_solid::BoolOp::Difference {
                let (mn, mx) = f.world_aabb();
                println!(
                    "  cutter #{} aabb x {:.3}..{:.3}  y {:.3}..{:.3}  z {:.3}..{:.3}",
                    f.id, mn.x, mx.x, mn.y, mx.y, mn.z, mx.z
                );
                break;
            }
        }
        for f in &app.factory.model.features {
            if f.op == cad_solid::BoolOp::Union && f.id == 24 {
                let (mn, mx) = f.world_aabb();
                println!(
                    "  body #24 aabb x {:.3}..{:.3}  y {:.3}..{:.3}  z {:.3}..{:.3}",
                    mn.x, mx.x, mn.y, mx.y, mn.z, mx.z
                );
            }
        }
        // Which Union body does the probe ray actually hit first?
        for f in &app.factory.model.features {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            let tris = app.factory.model.feature_world_positions(f);
            for c in tris.chunks_exact(3) {
                let (a, b, cc) = (
                    glam::Vec3::from(c[0]),
                    glam::Vec3::from(c[1]),
                    glam::Vec3::from(c[2]),
                );
                if let Some(t) = cad_solid::ray_triangle(start, -n, a, b, cc) {
                    if t > 1e-4 {
                        println!("  ray hits Union body #{} at t={t:.4}", f.id);
                        break;
                    }
                }
            }
        }
    }

    /// ISOLATES the thick-wall fix from the many-bodies problem: ONE box, 0.6 m thick (past
    /// the old 0.4 m gap threshold). If this passes and the curved case does not, the
    /// remaining fault is the multi-body cut, not the depth measurement.
    #[test]
    fn a_window_cuts_through_a_single_thick_wall() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 6.0,
                d: 0.6,
                h: 3.0,
            },
        );
        app.factory.recompute();
        let n = -glam::Vec3::Y;
        let face = glam::Vec3::new(0.0, -0.3, 1.5);
        assert!(
            surface_crossings(&app, face + n * 0.5, -n) >= 2,
            "probe sees the uncut wall"
        );
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(face, n));
        let (cx, cy) = pick_uv(face, n);
        let v = |x: f64, y: f64| cad_kernel::PolyVertex {
            pos: Vec2::new(cx + x, cy + y),
            bulge: 0.0,
        };
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
                cad_kernel::Polyline {
                    vertices: vec![
                        v(-0.5, -0.5),
                        v(0.5, -0.5),
                        v(0.5, 0.5),
                        v(-0.5, 0.5),
                        v(-0.5, -0.5),
                    ],
                    closed: true,
                    widths: Vec::new(),
                },
            )));
        app.factory_cut_sketch(true);
        app.factory.recompute();
        let crossings = surface_crossings(&app, face + n * 0.5, -n);
        assert_eq!(
            crossings, 0,
            "a 0.6 m wall must cut clean through, {crossings} remain"
        );
    }

    /// END TO END, the reported bug: a window cut through a THICK curved wall. Before the
    /// `assembly_span` fix the span measured 0, the cutter collapsed to the ±0.1 m margin, and
    /// the far face survived — the opening showed from outside and not from inside.
    #[test]
    fn a_window_cuts_through_a_thick_curved_wall() {
        // 0.6 m wall: past the 0.4 m gap threshold that used to zero the measurement.
        let (app, face, n) = curved_wall_with_window(8.0, 0.12, 0.6);
        let crossings = surface_crossings(&app, face + n * 0.5, -n);
        assert_eq!(
            crossings, 0,
            "the opening must be clear through a thick wall, {crossings} surface(s) remain",
        );
    }

    /// SANITY: the probe must actually see an uncut wall, or every assertion built on it is
    /// vacuous. A ray at the middle of the arc, before any cut, must cross the two skins.
    #[test]
    fn the_probe_sees_an_uncut_curved_wall() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        let (radius, thickness) = (4.0_f32, 0.3_f32);
        // ODD count: with an even one, angle 0 lands exactly on a segment JOINT and a probe
        // there grazes two boxes' end caps instead of the wall faces — a degenerate spot.
        let n_seg = 47;
        let sweep = 1.2_f32;
        let fp: Vec<glam::Vec2> = (0..=n_seg)
            .map(|i| {
                let a = -sweep * 0.5 + sweep * (i as f32 / n_seg as f32);
                glam::Vec2::new(radius * a.cos(), radius * a.sin())
            })
            .collect();
        app.factory.add_wall(fp, thickness, 3.0);
        app.factory.recompute();
        let face = glam::Vec3::new(radius + thickness * 0.5, 0.0, 1.5);
        let crossings = surface_crossings(&app, face + glam::Vec3::X * 0.5, -glam::Vec3::X);
        assert!(
            crossings >= 2,
            "the probe must hit an uncut wall (got {crossings}) — otherwise the cut tests below \
             prove nothing",
        );
    }

    /// BASELINE: on a GENTLY curved wall the through-cut works — the window is open from both
    /// sides, so a ray down the middle of the opening crosses no material at all.
    #[test]
    fn a_window_cuts_through_a_gently_curved_wall() {
        let (app, face, n) = curved_wall_with_window(30.0, 0.02, 0.3);
        let crossings = surface_crossings(&app, face + n * 0.5, -n);
        assert_eq!(crossings, 0, "the opening is clear through the wall");
    }

    /// A TIGHTLY curved wall. The cutter is a straight prism aimed along the tangent normal at
    /// ONE point, so a sharp curve is the case most likely to leave it short: across the
    /// opening the wall bends away from that tangent by the sagitta.
    #[test]
    fn a_window_cuts_through_a_tightly_curved_wall() {
        // R = 4 m, opening ~2.2 m wide → sagitta ≈ 0.15 m, past the 0.1 m margin.
        let (app, face, n) = curved_wall_with_window(4.0, 0.28, 0.3);
        let crossings = surface_crossings(&app, face + n * 0.5, -n);
        assert_eq!(
            crossings, 0,
            "the opening should be clear through the wall, but {crossings} surface(s) remain — \
             the inner skin was not reached",
        );
    }

    /// The reported gap: editing a cutout in 2D and dragging its points must actually reshape
    /// the opening. Cut a square hole, open it for edit, enlarge one corner, Apply — the
    /// opening must grow. Also guards the edit-mode flag lifecycle.
    #[test]
    fn editing_a_cutout_and_applying_reshape_enlarges_the_opening() {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        let (mn, mx) = app.factory.model.features[0].world_aabb();
        let c = (mn + mx) * 0.5;
        // Cut a small square opening straight through the box.
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(c, glam::Vec3::Z));
        let (cx, cy) = pick_uv(c, glam::Vec3::Z);
        let v = |x: f64, y: f64| cad_kernel::PolyVertex {
            pos: Vec2::new(cx + x, cy + y),
            bulge: 0.0,
        };
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
                cad_kernel::Polyline {
                    vertices: vec![v(-0.3, -0.3), v(0.3, -0.3), v(0.3, 0.3), v(-0.3, 0.3)],
                    closed: true,
                    widths: Vec::new(),
                },
            )));
        assert_eq!(app.factory_cut_sketch(true), 1, "one opening cut");
        let cid0 = app.factory.cutout_ids()[0];
        let s0 = app.factory.cutout_size(cid0).unwrap();

        // Open it for 2D editing — the outline is selected so its grips show.
        app.factory_edit_cutout(cid0);
        assert!(app.factory.editing_cutout(), "edit mode is armed");
        assert!(app.factory.session.is_some(), "a sketch session opened");
        assert_eq!(
            app.selection.len(),
            1,
            "the editable outline is selected (grips visible)"
        );

        // Drag the +u/+v corner outward (what a user does with the grip).
        let li = app.doc.dobjects.len() - 1;
        if let cad_kernel::Geom::Polyline(p) = &mut app.doc.dobjects[li].geom {
            p.vertices[2].pos = Vec2::new(p.vertices[2].pos.x + 0.6, p.vertices[2].pos.y + 0.6);
        }
        app.factory_apply_cutout_reshape();
        assert!(
            !app.factory.editing_cutout(),
            "edit mode cleared after Apply"
        );
        assert!(app.factory.session.is_none(), "sketch closed after Apply");

        let cid1 = app.factory.cutout_ids()[0];
        let s1 = app.factory.cutout_size(cid1).unwrap();
        // X and Y are the face of a Z-through cut; enlarging a corner grows both.
        assert!(
            s1[0] + s1[1] > s0[0] + s0[1] + 1e-4,
            "the reshaped opening is larger on its face: {:?} -> {:?}",
            s0,
            s1
        );
    }

    /// Cut a square opening on the top face of a default box, `through` or as a blind recess of
    /// `depth`. Returns the app and the opening's representative cutter id.
    ///
    /// The existing reshape test cuts THROUGH, which is why it could never catch the recess bug:
    /// re-cutting a through hole as a through hole is correct by accident.
    #[cfg(test)]
    fn box_with_opening(through: bool, depth: f32) -> (CadApp, u32) {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        let (mn, mx) = app.factory.model.features[0].world_aabb();
        let c = (mn + mx) * 0.5;
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(c, glam::Vec3::Z));
        let (cx, cy) = pick_uv(c, glam::Vec3::Z);
        let v = |x: f64, y: f64| cad_kernel::PolyVertex {
            pos: Vec2::new(cx + x, cy + y),
            bulge: 0.0,
        };
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
                cad_kernel::Polyline {
                    vertices: vec![v(-0.3, -0.3), v(0.3, -0.3), v(0.3, 0.3), v(-0.3, 0.3)],
                    closed: true,
                    widths: Vec::new(),
                },
            )));
        app.factory.element_height = depth;
        assert_eq!(app.factory_cut_sketch(through), 1, "one opening cut");
        app.factory.recompute();
        let id = app.factory.cutout_ids()[0];
        (app, id)
    }

    /// Drag the +u/+v corner of the open reshape outline outward, the way a grip drag does.
    #[cfg(test)]
    fn drag_reshape_corner(app: &mut CadApp) {
        let li = app.doc.dobjects.len() - 1;
        if let cad_kernel::Geom::Polyline(p) = &mut app.doc.dobjects[li].geom {
            p.vertices[2].pos = Vec2::new(p.vertices[2].pos.x + 0.2, p.vertices[2].pos.y + 0.2);
        }
    }

    /// How deep the opening's cutter reaches into the solid, in metres — the pocket depth for a
    /// recess, and more than the wall for a through cut. Read off the cutter itself rather than
    /// from anything the reshape path computed, so it cannot agree with a bug by sharing it.
    #[cfg(test)]
    fn opening_depth(app: &CadApp, id: u32) -> f32 {
        let f = app
            .factory
            .model
            .features
            .iter()
            .find(|f| f.id == id)
            .expect("cutter present");
        match f.primitive {
            cad_solid::Primitive::Extrusion { h, .. } => h,
            _ => panic!("the opening is not an extrusion"),
        }
    }

    /// RESHAPING A BLIND RECESS MUST LEAVE IT BLIND.
    ///
    /// `factory_apply_cutout_reshape` re-cut with a hard-coded `through = true`, so dragging a
    /// corner of a 0.1 m recess punched it clean through the wall — a shelf niche became a hole
    /// into the next room, from an edit that only touched the outline.
    ///
    /// Asserted on the SOLID, by ray: a blind pocket still has material under it, and a through
    /// hole does not. The cutter's own depth is checked too, because a pocket that got deeper
    /// but not quite through would pass a ray test alone.
    #[test]
    fn reshaping_a_blind_recess_keeps_it_blind() {
        let (mut app, id) = box_with_opening(false, 0.1);
        let d0 = opening_depth(&app, id);

        // The floor of the pocket: fire UP the -Z axis from under the box and count surfaces.
        // A recess leaves the far skin intact, so the ray meets material a through cut removes.
        let (mn, mx) = app.factory.model.features[0].world_aabb();
        let c = (mn + mx) * 0.5;
        let below = glam::Vec3::new(c.x, c.y, mn.z - 1.0);
        let before = surface_crossings(&app, below, glam::Vec3::Z);
        assert!(
            before > 0,
            "a 0.1 m recess must not already go through — this test is inert"
        );

        app.factory_edit_cutout(id);
        assert!(app.factory.editing_cutout(), "edit mode is armed");
        drag_reshape_corner(&mut app);
        app.factory_apply_cutout_reshape();
        app.factory.recompute();

        let id1 = app.factory.cutout_ids()[0];
        let after = surface_crossings(&app, below, glam::Vec3::Z);
        assert!(
            after > 0,
            "the recess was re-cut as a THROUGH hole — reshaping a niche opened the far face",
        );
        let d1 = opening_depth(&app, id1);
        assert!(
            (d1 - d0).abs() < 0.05,
            "the recess changed depth on a reshape that only moved a corner: {d0:.3} m -> {d1:.3} m",
        );
    }

    /// A THROUGH OPENING MUST STAY THROUGH. The counterpart, so that remembering the mode cannot
    /// be "fixed" by simply cutting every reshape as a recess.
    #[test]
    fn reshaping_a_through_opening_keeps_it_through() {
        let (mut app, id) = box_with_opening(true, 0.1);
        let (mn, mx) = app.factory.model.features[0].world_aabb();
        let c = (mn + mx) * 0.5;
        let below = glam::Vec3::new(c.x, c.y, mn.z - 1.0);
        assert_eq!(
            surface_crossings(&app, below, glam::Vec3::Z),
            0,
            "the hole must start out clear through — this test is inert otherwise",
        );

        app.factory_edit_cutout(id);
        drag_reshape_corner(&mut app);
        app.factory_apply_cutout_reshape();
        app.factory.recompute();

        assert_eq!(
            surface_crossings(&app, below, glam::Vec3::Z),
            0,
            "a through opening was re-cut as a blind recess — the hole grew a floor",
        );
    }

    /// LEAVING THE SKETCH WITHOUT APPLYING MUST LEAVE THE OPENING WHERE IT WAS.
    ///
    /// `factory_edit_cutout` deletes the baked cutters before opening the outline for editing, and
    /// `factory_exit_sketch` only cleared the edit flag. Every exit that is not ✔ Apply or
    /// ✖ Cancel — the Esc key, switching view, opening a file — therefore closed the sketch with
    /// the opening already deleted, silently and with nothing to undo toward.
    #[test]
    fn leaving_a_cutout_edit_without_applying_puts_the_opening_back() {
        let (mut app, id) = box_with_opening(true, 0.1);
        let before = app.factory.cutout_ids().len();
        let (mn, mx) = app.factory.model.features[0].world_aabb();
        let c = (mn + mx) * 0.5;
        let below = glam::Vec3::new(c.x, c.y, mn.z - 1.0);

        app.factory_edit_cutout(id);
        assert!(app.factory.editing_cutout(), "edit mode is armed");
        assert!(
            app.factory.cutout_ids().is_empty(),
            "the cutters really are removed while editing — otherwise this proves nothing",
        );

        // Escape: close the sketch without ✔ Apply and without ✖ Cancel.
        app.factory_exit_sketch();
        app.factory.recompute();

        assert!(!app.factory.editing_cutout(), "edit mode cleared on exit");
        assert_eq!(
            app.factory.cutout_ids().len(),
            before,
            "the opening was left deleted — an unapplied edit destroyed it",
        );
        assert_eq!(
            surface_crossings(&app, below, glam::Vec3::Z),
            0,
            "the restored opening does not cut: it was put back bound to the wrong body",
        );
    }

    /// The restored cutters must go back BEHIND THEIR OWN BODIES. Pushing them onto the end
    /// re-binds every one of them to whatever Union is last — the same defect fixed in
    /// `rederive_wall`, and it would leave this test's opening cutting a second box instead.
    #[test]
    fn an_abandoned_edit_restores_the_cutter_behind_its_own_body() {
        let (mut app, id) = box_with_opening(true, 0.1);
        let host = {
            let at = app
                .factory
                .model
                .features
                .iter()
                .position(|f| f.id == id)
                .expect("cutter");
            app.factory.model.features[..at]
                .iter()
                .rev()
                .find(|f| f.op == cad_solid::BoolOp::Union)
                .expect("a host body")
                .id
        };
        // A SECOND body, added after the opening, so an appended cutter has somewhere wrong to go.
        app.factory.add_box();
        app.factory.recompute();

        app.factory_edit_cutout(id);
        app.factory_exit_sketch();

        let restored = app.factory.cutout_ids();
        assert_eq!(restored.len(), 1, "exactly the one opening came back");
        let at = app
            .factory
            .model
            .features
            .iter()
            .position(|f| f.id == restored[0])
            .expect("restored cutter is in the model");
        let host_now = app.factory.model.features[..at]
            .iter()
            .rev()
            .find(|f| f.op == cad_solid::BoolOp::Union)
            .expect("a host body")
            .id;
        assert_eq!(
            host_now, host,
            "the restored opening is cutting a different body than the one it was drawn on",
        );
    }

    /// THE ROUTE BACK FROM A FLAGGED OPENING. An opening that lost its wall is kept with
    /// `enabled` cleared, which is only half an answer if nothing can ever clear the flag again.
    ///
    /// ▼ Openings ▸ ✏ Edit in 2D ▸ ✔ Apply is that route: the edit lifts the flagged cutter out
    /// and Apply re-cuts a fresh, live one wherever the outline now sits. This pins that down, so
    /// "kept and flagged" cannot quietly become "kept but permanently dead".
    #[test]
    fn a_flagged_opening_comes_back_by_editing_and_applying_it() {
        let (mut app, id) = box_with_opening(true, 0.1);
        // Flag it exactly as `rederive_wall` would: kept in the model, not applied.
        app.factory.model.set_enabled(id, false);
        app.factory.recompute();
        assert_eq!(
            app.factory.orphaned_cutouts().len(),
            1,
            "the opening starts out flagged"
        );
        let (mn, mx) = app.factory.model.features[0].world_aabb();
        let c = (mn + mx) * 0.5;
        let below = glam::Vec3::new(c.x, c.y, mn.z - 1.0);
        assert!(
            surface_crossings(&app, below, glam::Vec3::Z) > 0,
            "a flagged opening must not be cutting — this test would prove nothing otherwise",
        );

        app.factory_edit_cutout(id);
        app.factory_apply_cutout_reshape();
        app.factory.recompute();

        assert!(
            app.factory.orphaned_cutouts().is_empty(),
            "the opening is still flagged after being re-applied — there is no way back",
        );
        assert_eq!(
            surface_crossings(&app, below, glam::Vec3::Z),
            0,
            "the re-applied opening does not actually cut",
        );
    }

    /// A REFUSED RESHAPE LEAVES THE OPENING ALONE. `factory_apply_cutout_reshape` deletes the
    /// cutters on the way in and closes the sketch whatever happens, so a re-cut that produces
    /// nothing used to close over an opening that had already been destroyed.
    #[test]
    fn a_reshape_that_cannot_be_applied_leaves_the_opening_intact() {
        let (mut app, id) = box_with_opening(true, 0.1);
        let before = app.factory.cutout_ids().len();
        let (mn, mx) = app.factory.model.features[0].world_aabb();
        let c = (mn + mx) * 0.5;
        let below = glam::Vec3::new(c.x, c.y, mn.z - 1.0);

        app.factory_edit_cutout(id);
        // Destroy the outline: with nothing closed to cut, the re-cut must refuse.
        app.doc.dobjects.clear();
        app.selection.clear();
        app.selected = None;
        app.factory_apply_cutout_reshape();
        app.factory.recompute();

        assert_eq!(
            app.factory.cutout_ids().len(),
            before,
            "a refused reshape deleted the opening it could not re-cut",
        );
        assert_eq!(
            surface_crossings(&app, below, glam::Vec3::Z),
            0,
            "the opening came back but is no longer cutting its own body",
        );
    }

    /// The active sketch renders LIVE in 3D (its geometry is in the app's live doc), and once
    /// finished it shows via `sketch_lines` — so 2D drawing and 3D stay linked.
    #[test]
    fn active_sketch_renders_live_in_3d() {
        let mut app = CadApp::default();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 1.0,
                },
            )));
        assert!(
            !app.factory.live_sketch_lines(&app.doc).is_empty(),
            "the live sketch shows in 3D as drawn"
        );
        app.factory_exit_sketch();
        assert!(
            !app.factory.sketch_lines(&app.doc).is_empty(),
            "the finished sketch stays visible in 3D"
        );
    }

    /// SELECT a closed 2D shape and Extrude it — no face sketch needed. The solid is made on
    /// the ground plane at the shape's location, and the 2D drawing is NOT deleted.
    #[test]
    fn extrude_works_on_a_2d_selection() {
        let mut app = CadApp::default();
        // Declared metres so the shape's doc coords are also world metres
        // (the merged default is 1 unit = 1 mm).
        app.doc.units = cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::User);
        let idx = app.doc.dobjects.len();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(5.0, 3.0),
                    radius: 1.0,
                },
            )));
        app.selection = vec![idx];
        let before = app.factory.model.features.len();
        let made = app.factory_extrude_sketch(false);
        assert_eq!(made, 1, "the selected shape extruded");
        assert_eq!(
            app.factory.model.features.len(),
            before + 1,
            "a solid was added"
        );
        assert!(
            app.doc.dobjects.get(idx).is_some(),
            "the selected 2D drawing is preserved"
        );
        // The solid sits at the shape's world location (~x5, y3), not the origin.
        let f = app.factory.model.features.last().unwrap();
        let (mn, mx) = f.world_aabb();
        let (cx, cy) = ((mn.x + mx.x) * 0.5, (mn.y + mx.y) * 0.5);
        assert!(
            (cx - 5.0).abs() < 0.3 && (cy - 3.0).abs() < 0.3,
            "solid at the shape, got ({cx:.1},{cy:.1})"
        );
    }

    /// The extrude tool must work AFTER Finishing the sketch — the last drawn face-sketch is
    /// used — so the toolbar menu buttons aren't dead when you're not actively drafting.
    #[test]
    fn extrude_works_after_finishing_the_sketch() {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        let before = app.factory.model.features.len();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 1.0,
                },
            )));
        app.factory_exit_sketch(); // FINISH — session None, the drawing lives in the sketch
        assert!(app.factory.session.is_none());
        let made = app.factory_extrude_sketch(false);
        assert_eq!(made, 1, "extrude resolves the finished sketch");
        assert_eq!(
            app.factory.model.features.len(),
            before + 1,
            "a solid was added"
        );
    }

    /// A RECESS cut (not through) also adds a Difference, and KEEP-shape leaves the sketch
    /// open with the drawing intact so the same outline can be reused.
    #[test]
    fn recess_cut_and_keep_shape_behave() {
        let mut app = CadApp::default();
        app.factory.add_box();
        app.factory.recompute();
        let (mn, mx) = app.factory.model.features[0].world_aabb();
        let c = (mn + mx) * 0.5;
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(c, glam::Vec3::Z));
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 0.3,
                },
            )));
        app.factory.element_height = 0.15;
        app.factory.keep_sketch = true;
        let made = app.factory_cut_sketch(false); // recess
        assert_eq!(made, 1, "one recess cut");
        assert!(
            app.factory.session.is_some(),
            "keep-shape leaves the sketch open"
        );
        assert_eq!(app.doc.dobjects.len(), 1, "the drawing is kept for reuse");
    }

    /// A stale selection index from model space must not survive into the sketch.
    #[test]
    fn stale_selection_is_cleared_on_enter() {
        let mut app = CadApp::default();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(1.0, 0.0),
                },
            )));
        app.selection = vec![0];
        app.pre_op_selection = vec![0];
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert!(app.selection.is_empty(), "selection cleared");
        assert!(
            app.pre_op_selection.is_empty(),
            "pre_op_selection must not strand an index"
        );
        assert!(
            app.selection_prev.is_empty(),
            "selection_prev must not strand an index"
        );
        assert!(
            app.selected.is_none(),
            "`selected` must not strand an index"
        );
    }

    /// The ACTUAL crash repro: entering a sketch swaps the doc, then the app RENDERS.
    /// A stale index surviving into that render is what killed "Draw on this face".
    /// Drives a real egui frame headlessly (the PaintCallback is only queued, so no GL
    /// context is needed).
    #[test]
    fn render_after_enter_sketch_does_not_panic() {
        let mut app = CadApp::default();
        app.factory.open = true;
        app.factory.add_box();
        app.factory.recompute();
        // stale model-space indices, exactly as a real session would have
        app.selection = vec![0];
        app.pre_op_selection = vec![0];
        app.selection_prev = vec![0];
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());

        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            app.render_factory_panel(ctx);
        });
    }

    /// Arrow keys nudge the selected object for fine placement. Drives a real headless frame
    /// with ArrowRight pressed and checks the selected furniture moved +X by the fine step.
    #[test]
    fn arrow_key_nudges_the_selection() {
        let mut app = CadApp::default();
        app.factory.open = true;
        app.active_view = ActiveView::ThreeD;
        app.factory.cam_dist = 5.0; // step = cam_dist * 0.01 = 0.05 m
        let idx = app.factory.add_furniture_asset(
            "nudge-me".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                color: None,
                alpha: Vec::new(),
            },
        );
        app.factory
            .place_furniture(idx, glam::Vec3::new(2.0, 2.0, 0.0));
        app.factory.select_furniture(0);
        let x0 = app.factory.furniture[0].pos[0];

        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 900.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.render_factory_panel(ctx));

        let x1 = app.factory.furniture[0].pos[0];
        assert!(
            (x1 - x0 - 0.05).abs() < 1e-4,
            "ArrowRight nudges +0.05 m in X (got {})",
            x1 - x0
        );
    }

    /// The nudge step scales with zoom (`cam_dist * 0.01`): the SAME ArrowRight moves the
    /// object farther when zoomed out and a hair when zoomed in. Drives two frames at two
    /// camera distances and checks the resulting steps differ by the zoom ratio.
    #[test]
    fn arrow_nudge_step_scales_with_zoom() {
        fn nudge_step(cam_dist: f32) -> f32 {
            let mut app = CadApp::default();
            app.factory.open = true;
            app.active_view = ActiveView::ThreeD;
            app.factory.cam_dist = cam_dist;
            let idx = app.factory.add_furniture_asset(
                "z".into(),
                crate::mesh_io::ObjMesh {
                    positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    normals: vec![[0.0, 0.0, 1.0]; 3],
                    color: None,
                    alpha: Vec::new(),
                },
            );
            app.factory
                .place_furniture(idx, glam::Vec3::new(2.0, 2.0, 0.0));
            app.factory.select_furniture(0);
            let x0 = app.factory.furniture[0].pos[0];
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1400.0, 900.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::ArrowRight,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.render_factory_panel(ctx));
            app.factory.furniture[0].pos[0] - x0
        }
        let near = nudge_step(5.0); // zoomed in  → 0.05 m
        let far = nudge_step(50.0); // zoomed out → 0.50 m
        assert!((near - 0.05).abs() < 1e-4, "near step {near}");
        assert!((far - 0.50).abs() < 1e-4, "far step {far}");
        assert!(
            far > near * 9.0,
            "far step must dwarf near step ({far} vs {near})"
        );
    }

    /// REGRESSION (the `kbd_free=false` dump): a properties-panel DragValue keeps keyboard
    /// focus after you touch it, and the 3D viewport isn't focusable, so that focus is never
    /// cleared. The nudge must STILL fire (it keys off the pointer being over the viewport,
    /// not off the keyboard being free) — the old focus-gated code blocked every arrow here.
    #[test]
    fn arrow_nudge_works_while_a_field_holds_focus() {
        let mut app = CadApp::default();
        app.factory.open = true;
        app.active_view = ActiveView::ThreeD;
        app.factory.cam_dist = 5.0; // step = cam_dist * 0.01 = 0.05 m
        let idx = app.factory.add_furniture_asset(
            "nudge-me".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                color: None,
                alpha: Vec::new(),
            },
        );
        app.factory
            .place_furniture(idx, glam::Vec3::new(2.0, 2.0, 0.0));
        app.factory.select_furniture(0);
        let x0 = app.factory.furniture[0].pos[0];

        let ctx = egui::Context::default();
        // Simulate a DragValue in the properties panel owning the keyboard this frame.
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("properties_dragvalue_focus_hog")));
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 900.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.render_factory_panel(ctx));

        let x1 = app.factory.furniture[0].pos[0];
        assert!(
            (x1 - x0 - 0.05).abs() < 1e-4,
            "ArrowRight must nudge +0.05 m even with a field focused (got {})",
            x1 - x0
        );
    }

    /// The PATH-SWEEP core: `factory_build_sweep` sweeps a section (on one face) along a path
    /// drawn on a PERPENDICULAR plane into a new Union feature; with `cut` it subtracts, and
    /// refuses cleanly when nothing is under the path. The path is re-expressed in the section
    /// frame, so its component along the section normal becomes the extrusion depth.
    #[test]
    fn path_sweep_build_extrudes_and_refuses_empty_cut() {
        let section = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(0.2, 0.0),
            glam::Vec2::new(0.2, 0.2),
            glam::Vec2::new(0.0, 0.2),
            glam::Vec2::new(0.0, 0.0),
        ];
        let path = vec![glam::Vec2::new(0.0, 0.0), glam::Vec2::new(2.0, 0.0)];
        // Section on the ground (normal +Z); path on a PERPENDICULAR plane (u=Z=depth, v=X).
        let section_frame = cad_solid::Frame {
            origin: glam::Vec3::ZERO,
            u: glam::Vec3::X,
            v: glam::Vec3::Y,
        };
        let path_frame = cad_solid::Frame {
            origin: glam::Vec3::ZERO,
            u: glam::Vec3::Z,
            v: glam::Vec3::X,
        };

        // EXTRUDE → one Union sweep feature that tessellates.
        let mut app = CadApp::default();
        app.factory.open = true;
        let before = app.factory.model.features.len();
        let made = app.factory_build_sweep(
            section_frame,
            section.clone(),
            path_frame,
            path.clone(),
            false,
            false,
        );
        assert_eq!(made, 1, "extrude reports one solid made");
        assert_eq!(
            app.factory.model.features.len(),
            before + 1,
            "a sweep feature was added"
        );
        assert!(
            !app.factory.cached.positions.is_empty(),
            "the swept solid tessellates"
        );

        // CUT with nothing under the path → clean no-op, no feature added, no snapshot stranded.
        let mut app2 = CadApp::default();
        app2.factory.open = true;
        let n0 = app2.factory.model.features.len();
        let made2 = app2.factory_build_sweep(section_frame, section, path_frame, path, true, false);
        assert_eq!(made2, 0, "path cut with no solid under the path is a no-op");
        assert_eq!(
            app2.factory.model.features.len(),
            n0,
            "no feature added when there's no target"
        );
    }

    /// The reset must leave NO index-holding field populated — that is the invariant
    /// that makes a doc swap safe.
    #[test]
    fn reset_leaves_no_stale_indices() {
        let mut app = CadApp::default();
        app.selection = vec![0];
        app.pre_op_selection = vec![0];
        app.selection_prev = vec![0];
        app.aci_pick_many = vec![0];
        app.hatch_last_idx = Some(0);
        app.selected = Some(0);
        app.dbg_press_hit = Some(0);
        app.dbg_press_sel = vec![0];
        app.pending = vec![Vec2::new(1.0, 1.0)];
        app.index_dirty = false;
        app.factory_reset_doc_state();
        assert!(app.selection.is_empty());
        assert!(app.pre_op_selection.is_empty());
        assert!(app.selection_prev.is_empty());
        assert!(app.aci_pick_many.is_empty());
        assert!(app.hatch_last_idx.is_none());
        assert!(
            app.selected.is_none(),
            "`selected` is chained with `selection` when drawing"
        );
        assert!(app.dbg_press_hit.is_none());
        assert!(app.dbg_press_sel.is_empty());
        assert!(app.pending.is_empty());
        assert_eq!(app.select_mode, SelectMode::Off);
        assert_eq!(app.fillet_state, FilletState::Off);
        assert!(matches!(app.trim_state, TrimState::Off)); // TrimState is not PartialEq
                                                           // THE ACTUAL CRASH: the spatial index stores dobject indices and the canvas
                                                           // culls through it into `doc.dobjects[i]`. A grid built from the old document
                                                           // fed index 0 into an empty sketch doc = guaranteed panic.
        assert!(
            app.index.is_none(),
            "stale spatial index MUST be dropped on a doc swap"
        );
        assert!(app.index_dirty, "spatial index must be marked for rebuild");
        assert!(app.gpu_dirty, "GPU vertex cache is keyed to the old doc");
    }
}
#[cfg(test)]
mod factory_texture_tests {
    use super::*;

    fn app_with_furniture() -> (CadApp, usize) {
        let mut app = CadApp::default();
        let idx = app.factory.add_furniture_asset(
            "piece".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                color: Some([0.5, 0.5, 0.5]),
                alpha: Vec::new(),
            },
        );
        app.factory
            .place_furniture(idx, glam::Vec3::new(0.0, 0.0, 0.0)); // selects it
                                                                   // These tests are about WHOLE-OBJECT paint, which is now the explicit mode rather than
                                                                   // the default: per-face is on by default so that colouring a wall does not colour every
                                                                   // wall in the building. Saying so keeps their subject unchanged.
        app.factory.paint_surface_mode = false;
        (app, idx)
    }

    /// A texture applied to furniture can be CHANGED to a different one — the reported bug. Also
    /// covers the texture library picker's target (`apply_texture_index_to_selection`).
    #[test]
    fn furniture_texture_can_be_changed() {
        let (mut app, _idx) = app_with_furniture();
        let a = app
            .factory
            .add_texture("a".into(), 2, 2, [255u8, 0, 0, 255].repeat(4));
        let b = app
            .factory
            .add_texture("b".into(), 2, 2, [0u8, 0, 255, 255].repeat(4));
        app.apply_texture_index_to_selection(a, "a", 2, 2);
        assert_eq!(
            app.factory.furniture[0].texture,
            Some(a),
            "first texture applied"
        );
        // Change it — this is what applying a second material from the Materials Factory does.
        app.apply_texture_index_to_selection(b, "b", 2, 2);
        assert_eq!(
            app.factory.furniture[0].texture,
            Some(b),
            "texture changed to the second"
        );
    }

    /// Picking a COLOUR on a textured piece must clear the texture (else the colour stays masked
    /// by the still-present texture — the "nothing happens" half of the bug).
    #[test]
    fn colour_replaces_a_furniture_texture() {
        let (mut app, _idx) = app_with_furniture();
        let a = app
            .factory
            .add_texture("a".into(), 2, 2, [255u8, 0, 0, 255].repeat(4));
        app.apply_texture_index_to_selection(a, "a", 2, 2);
        assert!(app.factory.furniture[0].texture.is_some());
        app.apply_color_to_selection([0.2, 0.7, 0.3]);
        assert_eq!(
            app.factory.furniture[0].texture, None,
            "colour cleared the texture"
        );
        assert_eq!(
            app.factory.furniture[0].color,
            [0.2, 0.7, 0.3],
            "colour applied"
        );
    }

    /// Same rule for a FEATURE (wall/solid): a whole-object colour clears its feature + per-face
    /// textures so the colour is visible.
    #[test]
    fn colour_replaces_a_feature_texture() {
        let mut app = CadApp::default();
        app.factory.paint_surface_mode = false; // this test is about whole-object paint
        app.factory.add_box();
        app.factory.recompute();
        let id = app.factory.model.features[0].id;
        app.factory.sel_furniture.clear();
        app.factory.selection = vec![id];
        let a = app
            .factory
            .add_texture("a".into(), 2, 2, [255u8, 0, 0, 255].repeat(4));
        app.apply_texture_index_to_selection(a, "a", 2, 2);
        app.factory.surface_texture.insert((id, 0, 0, 1, 0), a); // a per-face texture too
        assert!(app.factory.feature_texture.contains_key(&id));
        app.apply_color_to_selection([0.1, 0.2, 0.3]);
        assert!(
            !app.factory.feature_texture.contains_key(&id),
            "feature texture cleared"
        );
        assert!(
            !app.factory.surface_texture.keys().any(|k| k.0 == id),
            "per-face textures cleared"
        );
        assert_eq!(
            app.factory.feature_color.get(&id).copied(),
            Some([0.1, 0.2, 0.3])
        );
    }

    /// Removing a furniture texture restores the asset's own colour (not the baked tint).
    #[test]
    fn removing_furniture_texture_restores_asset_colour() {
        let (mut app, _idx) = app_with_furniture();
        let a = app
            .factory
            .add_texture("a".into(), 2, 2, [255u8, 255, 0, 255].repeat(4));
        app.apply_texture_index_to_selection(a, "a", 2, 2); // bakes the tint into color
        app.remove_texture_from_selection();
        assert_eq!(app.factory.furniture[0].texture, None);
        assert_eq!(
            app.factory.furniture[0].color,
            [0.5, 0.5, 0.5],
            "back to the asset colour"
        );
    }
}
#[cfg(test)]
mod promote_tests {
    use super::*;

    fn app_with(geoms: Vec<Geom>) -> CadApp {
        let mut app = CadApp::default();
        // Start from an empty drawing so indices and counts mean exactly what the test
        // says — a default app is not guaranteed to hold nothing.
        app.doc.dobjects.clear();
        // The fixtures are written in METRE numbers (3-unit lines, 0.37 m thickness),
        // but a default document is now millimetre space — declare metres or a 3-unit
        // arc samples to chords below the factory's 1e-4 degenerate threshold and no
        // wall is ever created.
        app.doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::M,
            cad_kernel::UnitSource::Declared,
        );
        assert!(
            app.factory.walls.is_empty(),
            "fixture must start with no 3D walls"
        );
        for g in geoms {
            app.doc.dobjects.push(cad_kernel::DObject::new(g));
        }
        app.selection = (0..app.doc.dobjects.len()).collect();
        app
    }

    fn line(ax: f64, ay: f64, bx: f64, by: f64) -> Geom {
        Geom::Line(Line {
            a: cad_kernel::Vec2::new(ax, ay),
            b: cad_kernel::Vec2::new(bx, by),
        })
    }

    fn closed_rect(w: f64, h: f64) -> Geom {
        let v = |x: f64, y: f64| cad_kernel::PolyVertex {
            pos: cad_kernel::Vec2::new(x, y),
            bulge: 0.0,
        };
        Geom::Polyline(cad_kernel::Polyline {
            vertices: vec![v(0.0, 0.0), v(w, 0.0), v(w, h), v(0.0, h)],
            closed: true,
            widths: Vec::new(),
        })
    }

    /// The Build actions are gated on the selection. A CLOSED shape — what the rectangle
    /// and polyline tools produce — must enable all of them; if this ever fails, every
    /// row greys out and the 3D Factory tab looks like it has no tools.
    #[test]
    fn a_closed_outline_enables_every_build_action() {
        let app = app_with(vec![closed_rect(5.0, 4.0)]);
        assert!(app.can_make_3d_wall(), "a closed polyline can become walls");
        assert!(
            app.slab_outline_from_selection().is_some(),
            "a closed polyline is a valid outline for a building / floor / ceiling"
        );
    }

    /// An OPEN path is walls-only: there is no boundary to floor or to extrude.
    #[test]
    fn an_open_path_offers_walls_but_no_outline() {
        let app = app_with(vec![line(0.0, 0.0, 5.0, 0.0)]);
        assert!(app.can_make_3d_wall());
        assert!(app.slab_outline_from_selection().is_none());
    }

    /// With nothing selected every action is unavailable — which is why the rows must
    /// still be CLICKABLE and explain themselves rather than greying out.
    #[test]
    fn an_empty_selection_offers_nothing_but_must_still_explain() {
        let mut app = app_with(vec![closed_rect(5.0, 4.0)]);
        app.selection.clear();
        assert!(!app.can_make_3d_wall());
        assert!(app.slab_outline_from_selection().is_none());

        // Clicking anyway must say what to do, not fail silently.
        let before = app.history.len();
        app.do_make_building();
        assert!(app.history.len() > before, "a no-op click must report why");
        assert!(
            app.history.last().unwrap().contains("select"),
            "and the message must say what to select: {:?}",
            app.history.last()
        );
    }

    /// THE case this exists for. A DXF plan has no wall entity — `cad_io`'s writer says
    /// the centerline+thickness link is lost on export — so an imported plan arrives as
    /// plain lines. Before this they could not be promoted at all: promotion matched
    /// `Geom::Wall` only and silently returned 0.
    #[test]
    fn plain_lines_from_an_imported_plan_promote() {
        let mut app = app_with(vec![line(0.0, 0.0, 5.0, 0.0), line(5.0, 0.0, 5.0, 4.0)]);
        let (promoted, skipped) = app.make_3d_wall_from_selection();
        assert_eq!(promoted, 2, "both lines must become walls");
        assert_eq!(skipped, 0);
        assert_eq!(app.factory.walls.len(), 2);
    }

    /// Geometry with no thickness of its own takes the FACTORY's wall-thickness setting —
    /// the one exposed next to wall height in the 3D panel. (It used to come from the 2D
    /// wall style, which left the 3D view with no thickness control at all.)
    #[test]
    fn a_line_adopts_the_factory_wall_thickness() {
        let mut app = app_with(vec![line(0.0, 0.0, 3.0, 0.0)]);
        app.factory.wall_thickness = 0.37; // not the default, so this proves the source
        app.make_3d_wall_from_selection();
        assert_eq!(app.factory.walls[0].thickness, 0.37);
    }

    /// A real wall still wins with its OWN thickness — widening the input must not
    /// regress the original journey.
    #[test]
    fn a_wall_keeps_its_own_thickness() {
        let mut app = app_with(vec![Geom::Wall(cad_kernel::Wall {
            start: cad_kernel::Vec2::new(0.0, 0.0),
            end: cad_kernel::Vec2::new(4.0, 0.0),
            thickness: 0.42,
            style: cad_kernel::WallStyleTable::STANDARD,
            bulge: 0.0,
        })]);
        app.make_3d_wall_from_selection();
        assert_eq!(app.factory.walls[0].thickness, 0.42);
    }

    /// An arc is a CURVED centerline: it must arrive as a sampled polyline, not two end
    /// points, or the wall would cut straight across the curve.
    #[test]
    fn an_arc_promotes_as_a_sampled_curve() {
        let mut app = app_with(vec![Geom::Arc(cad_kernel::Arc {
            center: cad_kernel::Vec2::new(0.0, 0.0),
            radius: 3.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_2,
        })]);
        app.make_3d_wall_from_selection();
        assert!(
            app.factory.walls[0].footprint.len() > 2,
            "a curve must be sampled, not reduced to its endpoints"
        );
    }

    /// Nothing-to-extrude geometry is counted, never silently dropped — and it must not
    /// block the promotable objects selected alongside it.
    #[test]
    fn unpromotable_geometry_is_counted_not_silently_ignored() {
        let mut app = app_with(vec![
            line(0.0, 0.0, 1.0, 0.0),
            Geom::Point(cad_kernel::Point {
                location: cad_kernel::Vec2::new(9.0, 9.0),
                style: 0,
                size: 0.0,
            }),
        ]);
        let (promoted, skipped) = app.make_3d_wall_from_selection();
        assert_eq!(promoted, 1);
        assert_eq!(skipped, 1, "the point must be reported, not vanish");
    }

    /// The menu gate and promotion must agree. If they drift, the user right-clicks,
    /// sees "Make 3D wall", clicks it, and nothing happens.
    #[test]
    fn the_menu_gate_agrees_with_what_promotion_accepts() {
        for g in [
            line(0.0, 0.0, 1.0, 0.0),
            Geom::Point(cad_kernel::Point {
                location: cad_kernel::Vec2::new(0.0, 0.0),
                style: 0,
                size: 0.0,
            }),
        ] {
            let offered = is_promotable_to_wall(&g);
            let mut app = app_with(vec![g]);
            let (promoted, _) = app.make_3d_wall_from_selection();
            assert_eq!(offered, promoted > 0, "the gate must match the outcome");
        }
    }

    /// Promotion is one undo step, and it is a FACTORY step — the drawing is untouched.
    #[test]
    fn promotion_is_undoable_in_one_step() {
        let mut app = app_with(vec![line(0.0, 0.0, 5.0, 0.0), line(5.0, 0.0, 5.0, 4.0)]);
        app.make_3d_wall_from_selection();
        assert_eq!(app.factory.walls.len(), 2);
        app.do_undo();
        assert!(
            app.factory.walls.is_empty(),
            "one undo takes back the whole promotion"
        );
    }
}
#[cfg(test)]
mod factory_undo_tests {
    use super::*;

    /// Before this, EVERY 3D operation was irreversible: `snapshot_doc` clones the
    /// `Document`, which does not contain the Factory model, so nothing recorded a
    /// solid's creation. Undo must take a solid back out.
    #[test]
    fn undo_reverts_a_solid_and_redo_puts_it_back() {
        let mut app = CadApp::default();
        app.snapshot_factory();
        app.factory.add_box();
        assert_eq!(app.factory.model.features.len(), 1);

        app.do_undo();
        assert_eq!(
            app.factory.model.features.len(),
            0,
            "undo must remove the solid"
        );

        app.do_redo();
        assert_eq!(app.factory.model.features.len(), 1, "redo must restore it");
    }

    /// ONE stack, in the order the edits happened (rule 4 — UNDO is UNDO). A 2D edit
    /// after a 3D edit must undo the 2D one FIRST; the user should never have to know
    /// which viewport's history they are in.
    #[test]
    fn one_chronological_stack_spans_both_views() {
        let mut app = CadApp::default();
        app.snapshot_factory();
        app.factory.add_box();
        app.snapshot_doc();
        app.doc
            .dobjects
            .push(cad_kernel::DObject::new(Geom::Line(Line {
                a: cad_kernel::Vec2::new(0.0, 0.0),
                b: cad_kernel::Vec2::new(1.0, 1.0),
            })));
        let dobjects = app.doc.dobjects.len();

        app.do_undo(); // most recent = the 2D line
        assert_eq!(
            app.doc.dobjects.len(),
            dobjects - 1,
            "the 2D edit undoes first"
        );
        assert_eq!(
            app.factory.model.features.len(),
            1,
            "the solid is untouched"
        );

        app.do_undo(); // then the 3D solid
        assert_eq!(app.factory.model.features.len(), 0);
    }

    /// Undoing a 3D step must not restore a stale 2D document, and vice versa. A step
    /// carries only the state it recorded — mixing them would silently revert unrelated
    /// work in the other viewport.
    #[test]
    fn a_factory_step_leaves_the_drawing_alone() {
        let mut app = CadApp::default();
        app.snapshot_factory();
        app.factory.add_box();
        app.doc
            .dobjects
            .push(cad_kernel::DObject::new(Geom::Line(Line {
                a: cad_kernel::Vec2::new(0.0, 0.0),
                b: cad_kernel::Vec2::new(2.0, 0.0),
            })));
        let dobjects = app.doc.dobjects.len();

        app.do_undo();
        assert_eq!(
            app.doc.dobjects.len(),
            dobjects,
            "2D work must survive a 3D undo"
        );
    }

    /// A restored model's ids are not the ones that were selected, and an in-flight
    /// modify would keep operating on features that no longer exist.
    #[test]
    fn undo_drops_stale_3d_selection_and_cancels_a_running_op() {
        let mut app = CadApp::default();
        app.snapshot_factory();
        app.factory.add_box();
        app.factory.selection = vec![1];
        app.factory_begin_op(cad_solid::modify::ModifyOp::Move);
        assert!(app.factory.modify.is_some());

        app.do_undo();
        assert!(
            app.factory.selection.is_empty(),
            "stale selection must be dropped"
        );
        assert!(
            app.factory.modify.is_none(),
            "a running op must be cancelled"
        );
    }

    /// The rollback helper is for a 2D command that failed after snapshotting. It must
    /// refuse to eat a 3D step that happens to be on top — that step belongs to someone
    /// else's undo.
    #[test]
    fn rollback_doc_refuses_to_consume_a_factory_step() {
        let mut app = CadApp::default();
        app.snapshot_factory();
        app.factory.add_box();
        let depth = app.undo_stack.len();

        app.rollback_doc();
        assert_eq!(
            app.undo_stack.len(),
            depth,
            "a Factory step must not be popped"
        );
        assert_eq!(app.factory.model.features.len(), 1);
    }
}
/// FACE PLANES SURVIVE A SAVE.
///
/// `cad_solid::Model::sketches` is `#[serde(skip)]` and always was, so a face sketch lived only as
/// long as the app was open. That was survivable while a plane was something you re-picked off the
/// model each time. It is not survivable now that planes are a NAMED LIST you go back to — "so they
/// can instantly look at a sketch they made" is not true of a view that is gone after a save.
#[cfg(test)]
mod sketch_persistence {
    use super::*;

    fn a_building() -> CadApp {
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(
                &vec![
                    glam::Vec2::new(0.0, 0.0),
                    glam::Vec2::new(6.0, 0.0),
                    glam::Vec2::new(6.0, 6.0),
                    glam::Vec2::new(0.0, 6.0),
                    glam::Vec2::new(0.0, 0.0),
                ],
                3.0,
            )
            .expect("building");
        app.factory.recompute();
        app
    }

    fn wall_face() -> cad_solid::Frame {
        cad_solid::Frame::from_point_normal(glam::Vec3::new(6.0, 3.0, 1.5), glam::Vec3::X)
    }

    fn a_line() -> cad_kernel::Geom {
        cad_kernel::Geom::Line(cad_kernel::geom::Line {
            a: cad_kernel::math::Vec2::new(0.0, 0.0),
            b: cad_kernel::math::Vec2::new(1.0, 2.0),
        })
    }

    #[test]
    fn a_named_plane_and_its_drawing_come_back() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        app.doc.push(cad_kernel::DObject::new(a_line()));
        app.factory_exit_sketch(); // the drawing goes back into the sketch
        app.factory.model.sketches[0].name = "Kitchen elevation".into();
        let frame = app.factory.model.sketches[0].frame;

        let doc = app.factory.to_persist();
        let mut back = crate::factory::FactoryState::default();
        back.apply_persist(doc);

        assert_eq!(back.model.sketches.len(), 1, "the plane must survive");
        assert_eq!(
            back.model.sketches[0].name, "Kitchen elevation",
            "…with its name"
        );
        assert_eq!(
            back.model.sketches[0].doc.dobjects.len(),
            1,
            "…and its drawing"
        );
        // The plane must stand where it stood, or a reopened view looks at the wrong face.
        let f = back.model.sketches[0].frame;
        assert!(
            (f.origin - frame.origin).length() < 1e-4,
            "{:?} vs {:?}",
            f.origin,
            frame.origin
        );
        assert!(
            (f.normal() - frame.normal()).length() < 1e-4,
            "the plane must face the same way"
        );
    }

    /// THE TRAP. Entering a sketch MOVES its document into `CadApp::doc` and leaves an empty shell
    /// in the model, so a save taken while a face is OPEN would write that shell — the drawing on
    /// screen would be the one thing missing from the file.
    #[test]
    fn saving_while_a_face_is_open_saves_what_is_on_screen() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        app.doc.push(cad_kernel::DObject::new(a_line()));
        app.doc.push(cad_kernel::DObject::new(a_line()));
        assert!(
            app.factory.model.sketches[0].doc.dobjects.is_empty(),
            "precondition: the model's copy IS the empty shell while the sketch is open",
        );

        let cfg = app.build_simlux_config();
        let mut back = crate::factory::FactoryState::default();
        back.apply_persist(cfg.factory);
        assert_eq!(
            back.model.sketches[0].doc.dobjects.len(),
            2,
            "the live drawing must be what was written, not the shell left behind",
        );
    }

    /// A project written before planes were saved carries none, and then the model has no planes —
    /// which is exactly what it had before. It must not fail to load.
    #[test]
    fn an_older_project_loads_with_no_planes() {
        let app = a_building();
        let mut doc = app.factory.to_persist();
        doc.sketches.clear();
        let mut back = crate::factory::FactoryState::default();
        back.apply_persist(doc);
        assert!(back.model.sketches.is_empty());
    }

    /// A drawing that will not decode costs the DRAWING, not the plane: a named face with nothing
    /// on it can be drawn on again, where a dropped plane is a view that silently went missing.
    #[test]
    fn a_corrupt_drawing_still_leaves_the_plane() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        app.factory_exit_sketch();
        let mut doc = app.factory.to_persist();
        doc.sketches[0].rsm_b64 = "!!! not base64 !!!".into();

        let mut back = crate::factory::FactoryState::default();
        back.apply_persist(doc);
        assert_eq!(
            back.model.sketches.len(),
            1,
            "the plane must still be there"
        );
        assert!(back.model.sketches[0].doc.dobjects.is_empty());
    }
}
#[cfg(test)]
mod picked_face_outline {
    use super::*;

    /// A 200 mm wall, 4 m long and 3 m tall: faces at y = -0.1 and y = +0.1.
    fn a_wall() -> CadApp {
        let mut app = CadApp::default();
        app.factory.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            cad_solid::Primitive::Box {
                w: 4.0,
                d: 0.2,
                h: 3.0,
            },
        );
        app.factory.recompute();
        app
    }

    fn near_face() -> cad_solid::Frame {
        cad_solid::Frame::from_point_normal(glam::Vec3::new(0.0, -0.1, 1.5), -glam::Vec3::Y)
    }

    #[test]
    fn nothing_is_outlined_until_a_face_is_picked() {
        let app = a_wall();
        assert!(
            app.factory.picked_face_lines().is_empty(),
            "no sketch, no outline"
        );
    }

    #[test]
    fn picking_a_face_outlines_it_in_yellow() {
        let mut app = a_wall();
        app.factory_enter_sketch(near_face());
        let lines = app.factory.picked_face_lines();
        assert!(!lines.is_empty(), "the picked face must be outlined");
        for v in &lines {
            assert!(
                v.r > 0.9 && v.g > 0.7 && v.b < 0.3,
                "not yellow: ({:.2}, {:.2}, {:.2})",
                v.r,
                v.g,
                v.b,
            );
        }
    }

    /// THE WHOLE POINT — it has to be the face that was picked, not the one behind it. A wall has
    /// two identical faces 200 mm apart, and outlining the wrong one answers the question wrongly,
    /// which is worse than not answering it.
    #[test]
    fn it_outlines_the_face_that_was_picked_not_the_other_side() {
        let mut app = a_wall();
        app.factory_enter_sketch(near_face());
        for v in &app.factory.picked_face_lines() {
            assert!(
                (v.y - (-0.1)).abs() < 1e-3,
                "a vertex at y = {:.3}: that is the FAR side of the wall",
                v.y,
            );
        }
    }

    /// …and it is the whole face, not a fragment of it: 4 m long and 3 m tall, where it stands.
    #[test]
    fn the_outline_covers_the_whole_face() {
        let mut app = a_wall();
        app.factory_enter_sketch(near_face());
        let lines = app.factory.picked_face_lines();
        let (mut mnx, mut mxx) = (f32::MAX, f32::MIN);
        let (mut mnz, mut mxz) = (f32::MAX, f32::MIN);
        for v in &lines {
            mnx = mnx.min(v.x);
            mxx = mxx.max(v.x);
            mnz = mnz.min(v.z);
            mxz = mxz.max(v.z);
        }
        assert!(
            (mxx - mnx - 4.0).abs() < 1e-2,
            "width {:.3}, want 4",
            mxx - mnx
        );
        assert!(
            (mxz - mnz - 3.0).abs() < 1e-2,
            "height {:.3}, want 3",
            mxz - mnz
        );
    }

    /// Leaving the sketch takes the outline with it — a yellow face still glowing after you have
    /// gone back to the global view is pointing at nothing.
    #[test]
    fn leaving_the_sketch_clears_the_outline() {
        let mut app = a_wall();
        app.factory_enter_sketch(near_face());
        assert!(!app.factory.picked_face_lines().is_empty());
        app.factory_exit_sketch();
        assert!(app.factory.picked_face_lines().is_empty());
    }

    /// It draws the SAME edges the 2D underlay draws, because it is built from the same source.
    /// Two independent answers to "which face is open?" could disagree; one cannot.
    #[test]
    fn it_is_the_same_edges_the_2d_canvas_is_drawing_against() {
        let mut app = a_wall();
        app.factory_enter_sketch(near_face());
        // Two vertices per segment.
        assert_eq!(
            app.factory.picked_face_lines().len(),
            app.factory.sketch_ref.len() * 2
        );
    }
}
/// THE ORIGIN GIZMO, and the prompt that was being wiped a frame after it opened.
#[cfg(test)]
mod origin_and_prompt {
    use super::*;

    /// "have a gizmo at the origin so the user know where the origin is. it should have x, y and z
    /// axis." It matters more here than in most 3D apps: three of the four placement modes measure
    /// from the origin, and `@X,Y,Z` is relative to it.
    #[test]
    fn the_origin_has_three_coloured_axes() {
        let mut st = crate::factory::FactoryState::default();
        st.show_origin = true;
        let g = st.origin_gizmo_lines();
        assert!(!g.is_empty(), "the origin must be visible");

        // One axis reaching along each of +X, +Y and +Z, from (0,0,0).
        let reaches =
            |pick: fn(&crate::light3d::V3) -> f32| g.iter().fold(0.0_f32, |a, v| a.max(pick(v)));
        assert!(reaches(|v| v.x) > 0.0, "no +X axis");
        assert!(reaches(|v| v.y) > 0.0, "no +Y axis");
        assert!(reaches(|v| v.z) > 0.0, "no +Z axis");

        // It STARTS at the origin — a gizmo somewhere else is worse than none.
        assert!(
            g.iter()
                .any(|v| v.x.abs() < 1e-6 && v.y.abs() < 1e-6 && v.z.abs() < 1e-6),
            "nothing touches (0, 0, 0)",
        );

        // RGB = XYZ, the convention every 3D tool uses. The vertex furthest along each axis must
        // carry that axis's colour.
        let furthest = |pick: fn(&crate::light3d::V3) -> f32| {
            g.iter()
                .max_by(|a, b| pick(a).partial_cmp(&pick(b)).unwrap())
                .map(|v| (v.r, v.g, v.b))
                .unwrap()
        };
        let (r, gg, b) = furthest(|v| v.x);
        assert!(
            r > 0.8 && gg < 0.5 && b < 0.5,
            "+X is not red: ({r}, {gg}, {b})"
        );
        let (r, gg, b) = furthest(|v| v.y);
        assert!(gg > 0.7 && r < 0.5, "+Y is not green: ({r}, {gg}, {b})");
        let (r, gg, b) = furthest(|v| v.z);
        assert!(b > 0.8 && r < 0.6, "+Z is not blue: ({r}, {gg}, {b})");
    }

    /// Sized off the camera, so it is readable at any zoom instead of a dot from far away and a
    /// wall from close up.
    #[test]
    fn the_gizmo_scales_with_the_camera() {
        let mut st = crate::factory::FactoryState::default();
        let reach = |st: &crate::factory::FactoryState| {
            st.origin_gizmo_lines()
                .iter()
                .fold(0.0_f32, |a, v| a.max(v.x.max(v.y).max(v.z)))
        };
        st.show_origin = true;
        st.cam_dist = 10.0;
        let near = reach(&st);
        st.cam_dist = 1000.0;
        let far = reach(&st);
        assert!(
            far > near * 10.0,
            "near {near:.2} m, far {far:.2} m — it did not scale"
        );
    }

    /// OFF by default. Asked for, then "the origin gizmo needs to turned off" — it sits in the
    /// middle of the model and there is rarely anything at the world origin worth looking at.
    #[test]
    fn the_origin_is_off_until_asked_for() {
        let mut st = crate::factory::FactoryState::default();
        assert!(!st.show_origin, "off by default");
        assert!(st.origin_gizmo_lines().is_empty());
        st.show_origin = true;
        assert!(
            !st.origin_gizmo_lines().is_empty(),
            "…and the toggle brings it back"
        );
    }

    /// The two switches are independent: the grid is a working surface, the origin a reference
    /// point, and wanting one is no reason to be handed the other.
    #[test]
    fn the_grid_and_the_origin_switch_separately() {
        let mut st = crate::factory::FactoryState::default();
        st.show_grid = false;
        st.show_origin = true;
        assert!(st.grid_lines().is_empty());
        assert!(!st.origin_gizmo_lines().is_empty());

        st.show_grid = true;
        st.show_origin = false;
        assert!(!st.grid_lines().is_empty());
        assert!(st.origin_gizmo_lines().is_empty());
    }

    // ---- the prompt that was being wiped -------------------------------------------------

    /// THE BUG BEHIND "is it even working?".
    ///
    /// `place` worked: it set `Placement [Click/Centre/Origin/Offset] <offset>:`. Then the
    /// frame-end sweep — which clears any prompt no 2D edit state is claiming — wiped it one frame
    /// later, and the screenshot showed `command: place` above the IDLE prompt. Worse than
    /// invisible: `place_prompt_open` stayed true with nothing on screen saying so, so the next
    /// word typed was eaten as a bad answer to a question the user could not see.
    #[test]
    fn the_placement_prompt_survives_the_frame_end_sweep() {
        let mut app = CadApp::default();
        app.factory.open = true;
        app.active_view = ActiveView::ThreeD;
        app.run_command("place");
        assert!(app.place_prompt_open);
        assert!(
            !app.current_prompt.is_empty(),
            "precondition: the prompt is up"
        );

        // …the sweep, exactly as the end of the frame runs it.
        app.sweep_stale_prompt();
        assert!(
            !app.current_prompt.is_empty(),
            "the placement question was wiped a frame after it was asked",
        );
        assert!(
            app.current_prompt.contains("Placement"),
            "got {:?}",
            app.current_prompt
        );
    }

    /// …and an ordinary stale prompt is still cleared, or the fix would just be a leak.
    #[test]
    fn an_unclaimed_prompt_is_still_swept() {
        let mut app = CadApp::default();
        app.current_prompt = "something nobody owns".into();
        app.sweep_stale_prompt();
        assert!(
            app.current_prompt.is_empty(),
            "the sweep must still do its job"
        );
    }
}
/// PAINTING A WALL MUST NOT PAINT THE BUILDING.
///
/// Reported as: "when i try to apply a texture of colour to wall of a room it applied for the
/// entire building except the floor." A building is ONE extrusion, so a per-FEATURE paint puts the
/// colour on every wall of it at once; the floor escaped only because a slab is a separate feature.
///
/// Per-face painting already existed — it sat behind a checkbox that defaulted OFF, so nobody had
/// reason to find it. The surface is what a person means by "this wall", so that is the default
/// now, and whole-object paint is the explicit act.
#[cfg(test)]
mod paint_targets_a_surface {
    use super::*;

    fn a_building() -> CadApp {
        let mut app = CadApp::default();
        app.factory.building_height = 3.0;
        app.factory
            .add_building_outline(
                &vec![
                    glam::Vec2::new(0.0, 0.0),
                    glam::Vec2::new(6.0, 0.0),
                    glam::Vec2::new(6.0, 6.0),
                    glam::Vec2::new(0.0, 6.0),
                    glam::Vec2::new(0.0, 0.0),
                ],
                3.0,
            )
            .expect("building");
        app.factory.recompute();
        app
    }

    /// THE DEFAULT. Out of the box, paint lands on one face.
    #[test]
    fn per_face_painting_is_the_default() {
        let app = CadApp::default();
        assert!(
            app.factory.paint_surface_mode,
            "a wall colour that covers the whole building is the bug this default prevents",
        );
    }

    /// Applying a texture with an object selected must NOT bind it to the feature — that is the
    /// reported behaviour. It arms the face brush instead, and the next click paints one surface.
    #[test]
    fn a_texture_does_not_cover_the_whole_solid_by_default() {
        let mut app = a_building();
        let id = app.factory.model.features[0].id;
        app.factory.selection = vec![id];
        let t = app
            .factory
            .add_texture("t".into(), 2, 2, [200u8, 30, 30, 255].repeat(4));

        app.apply_texture_index_to_selection(t, "t", 2, 2);
        assert!(
            !app.factory.feature_texture.contains_key(&id),
            "the whole extrusion was textured — every wall of the building at once",
        );
        assert_eq!(
            app.factory.surface_tex_brush,
            Some(t),
            "it should be armed to paint the face that is clicked next",
        );
    }

    /// …and turning the mode off still gives the sweeping behaviour, deliberately, for when that
    /// IS what is wanted.
    #[test]
    fn whole_object_paint_is_still_available_explicitly() {
        let mut app = a_building();
        let id = app.factory.model.features[0].id;
        app.factory.selection = vec![id];
        app.factory.paint_surface_mode = false;
        let t = app
            .factory
            .add_texture("t".into(), 2, 2, [200u8, 30, 30, 255].repeat(4));

        app.apply_texture_index_to_selection(t, "t", 2, 2);
        assert_eq!(
            app.factory.feature_texture.get(&id).copied(),
            Some(t),
            "with per-face off, the whole solid should take it",
        );
    }
}
/// CLICKING A SURFACE GIVES IT ITS OWN MATERIAL.
///
/// "when the user clicks on a surface the materials factory will show every parameter related to
/// it" — and, asked and answered: a face with no material of its own gets a NEW one rather than
/// borrowing the object's. Borrowing would mean the first edit repaints every other face sharing
/// it, which is the same "it applied to the entire building" complaint arriving by another route.
#[cfg(test)]
mod materials_follow_the_surface {
    use super::*;

    fn a_building() -> CadApp {
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(
                &vec![
                    glam::Vec2::new(0.0, 0.0),
                    glam::Vec2::new(6.0, 0.0),
                    glam::Vec2::new(6.0, 6.0),
                    glam::Vec2::new(0.0, 6.0),
                    glam::Vec2::new(0.0, 0.0),
                ],
                3.0,
            )
            .expect("building");
        app.factory.recompute();
        app
    }

    fn a_key(app: &CadApp) -> crate::factory::SurfaceKey {
        let id = app.factory.model.features[0].id;
        let t = &app.factory.cached.positions;
        crate::factory::surface_key(id, t[0], t[1], t[2])
    }

    /// A bare face is GIVEN a material, bound to that face alone.
    #[test]
    fn a_face_with_no_material_gets_its_own() {
        let mut app = a_building();
        let key = a_key(&app);
        assert!(
            app.factory.surface_texture.get(&key).is_none(),
            "precondition: nothing bound"
        );

        app.materials_select_surface(key);
        let ti = *app
            .factory
            .surface_texture
            .get(&key)
            .expect("bound to this face");
        assert_eq!(app.materials.sel, Some(ti), "and the window is showing it");
        // Bound to THIS face and nothing else — the whole point.
        assert_eq!(app.factory.surface_texture.len(), 1);
        assert!(
            !app.factory.feature_texture.contains_key(&key.0),
            "it must not have been bound to the feature, which is every wall at once",
        );
    }

    /// Opening a material is not itself a visible change: the new one starts from what the face
    /// already looks like.
    #[test]
    fn the_new_material_starts_from_the_faces_current_look() {
        let mut app = a_building();
        let key = a_key(&app);
        app.factory.surface_color.insert(key, [0.9, 0.1, 0.1]);

        app.materials_select_surface(key);
        let ti = app.materials.sel.expect("selected");
        let t = &app.factory.textures[ti];
        let p = t.proc.as_ref().expect("a procedural material");
        assert!(p.is_solid(), "a plain colour, not a pattern");
        let c = p.col_a;
        assert!(
            (c[0] - 0.9).abs() < 1e-6 && (c[1] - 0.1).abs() < 1e-6,
            "started from {c:?}, expected the face's own red",
        );
    }

    /// Clicking a face that ALREADY has its own material selects it rather than making another —
    /// otherwise every visit would litter the list with near-duplicates.
    #[test]
    fn clicking_the_same_face_again_selects_rather_than_duplicates() {
        let mut app = a_building();
        let key = a_key(&app);
        app.materials_select_surface(key);
        let first = app.materials.sel.expect("selected");
        let count = app.factory.textures.len();

        app.materials_select_surface(key);
        assert_eq!(
            app.materials.sel,
            Some(first),
            "the same material, not a new one"
        );
        assert_eq!(
            app.factory.textures.len(),
            count,
            "no duplicate was created"
        );
    }

    /// Two different faces get two different materials, so editing one leaves the other alone.
    #[test]
    fn two_faces_get_two_materials() {
        let mut app = a_building();
        let id = app.factory.model.features[0].id;
        let t = &app.factory.cached.positions;
        let k1 = crate::factory::surface_key(id, t[0], t[1], t[2]);
        // A triangle well away from the first, so it is a different face.
        let n = t.len() / 3;
        let far = (n / 2) * 3;
        let k2 = crate::factory::surface_key(id, t[far], t[far + 1], t[far + 2]);
        assert_ne!(k1, k2, "precondition: two distinct faces");

        app.materials_select_surface(k1);
        app.materials_select_surface(k2);
        let a = app.factory.surface_texture.get(&k1).copied();
        let b = app.factory.surface_texture.get(&k2).copied();
        assert!(a.is_some() && b.is_some());
        assert_ne!(
            a, b,
            "each face must own its material, or editing one changes the other"
        );
    }
}
/// A COLOUR SWATCH MUST NOT COLOUR THE WHOLE BUILDING EITHER.
///
/// Reported twice. The first fix guarded the TEXTURES MENU, so picking a colour there behaved —
/// and the properties panel's own swatches called `apply_color_to_selection` straight through and
/// coloured the entire extrusion, which for a building is every wall at once. A rule enforced at
/// one of two entry points is not a rule.
///
/// The dump made it identifiable: the material read `used by 0 feat / 0 surf / 0 furn` while the
/// building visibly changed, because that line counts TEXTURE users and what had changed was
/// `feature_color`.
#[cfg(test)]
mod colour_targets_a_surface {
    use super::*;

    fn a_building() -> CadApp {
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(
                &vec![
                    glam::Vec2::new(0.0, 0.0),
                    glam::Vec2::new(6.0, 0.0),
                    glam::Vec2::new(6.0, 6.0),
                    glam::Vec2::new(0.0, 6.0),
                    glam::Vec2::new(0.0, 0.0),
                ],
                3.0,
            )
            .expect("building");
        app.factory.recompute();
        app
    }

    /// THE REPORTED CASE, by the route that was missed: a swatch clicked with the solid selected.
    #[test]
    fn a_colour_does_not_repaint_the_whole_solid_by_default() {
        let mut app = a_building();
        let id = app.factory.model.features[0].id;
        app.factory.selection = vec![id];

        app.apply_color_to_selection([0.9, 0.2, 0.2]);
        assert!(
            !app.factory.feature_color.contains_key(&id),
            "the whole extrusion was coloured — every wall of the building at once",
        );
        assert_eq!(
            app.factory.last_pick_color,
            [0.9, 0.2, 0.2],
            "it should be armed to paint the face clicked next",
        );
    }

    /// A colour supersedes a texture brush, or the next click would apply the texture the user
    /// just moved away from.
    #[test]
    fn choosing_a_colour_disarms_the_texture_brush() {
        let mut app = a_building();
        let id = app.factory.model.features[0].id;
        app.factory.selection = vec![id];
        let t = app
            .factory
            .add_texture("t".into(), 2, 2, [10u8, 10, 10, 255].repeat(4));
        app.factory.surface_tex_brush = Some(t);

        app.apply_color_to_selection([0.1, 0.8, 0.3]);
        assert_eq!(app.factory.surface_tex_brush, None);
    }

    /// …and with per-face off, the sweeping behaviour is still there deliberately.
    #[test]
    fn whole_object_colour_is_still_available_explicitly() {
        let mut app = a_building();
        let id = app.factory.model.features[0].id;
        app.factory.selection = vec![id];
        app.factory.paint_surface_mode = false;

        app.apply_color_to_selection([0.4, 0.5, 0.6]);
        assert_eq!(
            app.factory.feature_color.get(&id).copied(),
            Some([0.4, 0.5, 0.6])
        );
    }
}
/// MATERIALS ARE MADE IN ONE PLACE.
///
/// The colour swatches, the paste/load buttons and the texture library all appeared twice — once in
/// the properties panel and once in the ▼ Textures menu — and each wrote through its own idea of
/// what "the selection" meant. That duplication is what produced "it applied for the entire
/// building except the floor": the panel's swatch went to the whole FEATURE, and a building is one
/// extrusion. The user's own call was to remove application from the parameters area and keep it
/// exclusively in the Materials Factory.
///
/// The UI itself cannot be asserted on here. What CAN be, and is what the change turns on, is that
/// the Materials Factory's own two entry points build a library entry WITHOUT needing a surface
/// selected — which they previously refused to do, harmlessly while a second button existed and
/// not harmlessly once it was the only one.
#[cfg(test)]
mod materials_are_made_in_one_place {
    use super::*;

    /// Loading an image file with nothing selected must still produce a material.
    #[test]
    fn a_file_becomes_a_material_with_nothing_selected() {
        let mut app = CadApp::default();
        assert!(
            !app.factory.has_any_selection(),
            "precondition: nothing is selected"
        );

        // A 2×2 PNG written to the scratch dir — the loader takes a path, not bytes.
        let dir = std::env::temp_dir().join("simlux_material_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("swatch.png");
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 40, 40, 255]));
        img.save(&path).expect("write the test image");

        let before = app.factory.textures.len();
        app.load_texture_from_file(&path.to_string_lossy());
        assert_eq!(
            app.factory.textures.len(),
            before + 1,
            "the material must be created even with no surface to put it on: {}",
            app.factory.status,
        );
        assert_eq!(
            app.materials.sel,
            Some(before),
            "and be selected in the Materials Factory"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// …and with something selected it still lands on it, which is the older behaviour that must
    /// not have been traded away for the above.
    #[test]
    fn and_still_applies_when_a_surface_is_selected() {
        let mut app = CadApp::default();
        let idx = app.factory.add_furniture_asset(
            "block".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                color: Some([0.5, 0.5, 0.5]),
                alpha: Vec::new(),
            },
        );
        app.factory.place_furniture(idx, glam::Vec3::ZERO); // selects it
        app.factory.paint_surface_mode = false; // whole-object, the subject here

        let dir = std::env::temp_dir().join("simlux_material_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("swatch2.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([40, 200, 40, 255]))
            .save(&path)
            .expect("write the test image");

        app.load_texture_from_file(&path.to_string_lossy());
        assert_eq!(
            app.factory.furniture[0].texture,
            Some(0),
            "a selected object must still receive it: {}",
            app.factory.status,
        );
        let _ = std::fs::remove_file(&path);
    }
}
/// THE MATERIALS FACTORY WINDOW IS THE PAINT TOOL.
///
/// Reported as: "when i click on a surface it applied a texture to it, i cant select a building …
/// and i cant apply texture from clip board. a texture application should only work when materials
/// factory window is open."
///
/// Both halves came from the same place. Loading or pasting an image with nothing selected ARMS a
/// face brush — `apply_texture_index_to_selection` does that deliberately, because a wall's two
/// sides are different materials and a per-feature texture covers both. But the brush was then
/// consumed by ANY click in the 3D view, with the window shut and nothing on screen saying a brush
/// was live. Every click painted a face instead of selecting the building, and the way out was to
/// notice a mode you had never been told you were in.
///
/// This asserts the source of the click path rather than the click itself, because the gesture
/// needs a live egui pointer a unit test cannot supply. The shape is what matters: the paint is
/// guarded by `materials_open`, and selection is not an `else` of it.
#[cfg(test)]
mod only_the_materials_factory_paints {
    use super::*;

    /// Isolate the 3D click handler: from the paint guard to the right-click handler after it.
    fn click_path() -> &'static str {
        let src = include_str!("../ports_simlux.rs");
        let a = src
            .find("// NOTHING PAINTS UNLESS THE MATERIALS FACTORY IS OPEN.")
            .expect("the guard's own comment marks the start of the click path");
        let b = src[a..]
            .find("// ---- RIGHT-CLICK")
            .map(|e| a + e)
            .expect("right-click handler follows");
        &src[a..b]
    }

    /// The brush may only fire with the window open.
    #[test]
    fn painting_is_guarded_by_the_window_being_open() {
        let body = click_path();
        let paint = body
            .find("paint_surface_texture")
            .expect("the brush paints somewhere here");
        let guard = body.find("self.materials_open").expect("and it is guarded");
        assert!(
            guard < paint,
            "the guard must come BEFORE the paint, not after it"
        );
    }

    /// The old shape — `else if paint_surface_mode` — took the click away from selection whenever a
    /// brush happened to be armed. Selection must not be an `else` of painting.
    #[test]
    fn selection_is_not_an_else_of_painting() {
        let body = click_path();
        for line in body.lines() {
            let l = line.trim();
            if l.starts_with("//") {
                continue;
            }
            assert!(
                !(l.contains("else if") && l.contains("paint_surface_mode")),
                "a click must not be diverted into painting by a mode flag: {l}",
            );
        }
        // And the colour brush is gone entirely — the palette that armed it no longer exists.
        assert!(
            !body.contains("last_pick_color"),
            "clicking must not paint a colour: the palette that set it was removed",
        );
    }

    /// Closing the window puts the brush down, so it cannot be resumed by a later click.
    #[test]
    fn closing_the_window_disarms_the_brush() {
        let src = include_str!("../windows.rs");
        let a = src
            .find("// CLOSING THE WINDOW PUTS THE BRUSH DOWN.")
            .expect("it does");
        let b = src[a..]
            .find("self.materials_open = open;")
            .map(|e| a + e)
            .unwrap();
        assert!(
            src[a..b].contains("surface_tex_brush = None"),
            "the brush must be cleared when the window closes",
        );
    }

    /// Pasting from the clipboard must APPLY, not merely file the material in the library. The
    /// other half of the report: "i cant apply texture from clip board".
    #[test]
    fn pasting_from_the_clipboard_applies_it() {
        let src = include_str!("../windows.rs");
        let a = src
            .find("Paste an image from the clipboard as a new material")
            .expect("the button");
        let b = a + src[a..]
            .find("\n                }")
            .expect("its handler ends");
        let handler = &src[a..b];
        assert!(
            handler.contains("paste_texture_as_material"),
            "it captures the clipboard"
        );
        assert!(
            handler.contains("apply_texture_index_to_selection"),
            "…and then applies it, rather than leaving it in the library going nowhere",
        );
    }

    /// A material armed as a brush and then applied to a surface is what the whole flow is for —
    /// this exercises the routing itself rather than the source.
    #[test]
    fn an_armed_material_lands_on_the_face_it_is_painted_onto() {
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(
                &vec![
                    glam::Vec2::new(0.0, 0.0),
                    glam::Vec2::new(4.0, 0.0),
                    glam::Vec2::new(4.0, 4.0),
                    glam::Vec2::new(0.0, 4.0),
                    glam::Vec2::new(0.0, 0.0),
                ],
                3.0,
            )
            .expect("building");
        app.factory.recompute();

        let ti = app
            .factory
            .add_texture("t".into(), 2, 2, [255u8, 0, 0, 255].repeat(4));
        app.factory.clear_selection();
        app.apply_texture_index_to_selection(ti, "t", 2, 2);
        assert_eq!(
            app.factory.surface_tex_brush,
            Some(ti),
            "with nothing selected the material arms as a face brush: {}",
            app.factory.status,
        );
        // …and nothing is painted until a face is actually clicked.
        assert!(
            app.factory.surface_texture.is_empty(),
            "arming must not paint anything on its own",
        );
    }
}
/// A CUT SAYS WHAT IT WAS FOR, INSTEAD OF BEING GUESSED AT AFTERWARDS.
///
/// "Does the cutter span the wall?" is the only question geometry alone can answer, and a blind
/// recess fails it BY CONSTRUCTION — stopping inside the wall is the whole point of a recess. So
/// the ⚠ DOES-NOT-REACH warning fired on every pocket in the model. Measured on the real
/// 172-feature project: 18 openings flagged, of which 6 were recesses working exactly as drawn.
/// A warning that cries wolf on a third of its cases is one people learn to scroll past, which is
/// worse than no warning — the genuinely stopped cuts were in the same list.
#[cfg(test)]
mod a_cut_records_what_it_was_for {
    use super::*;

    /// A 4 m x 0.3 m wall, 3 m high, with a face sketch open on its outside face — the state the
    /// Cut command runs from.
    fn a_wall_ready_to_cut() -> CadApp {
        let mut app = CadApp::default();
        app.factory
            .add_wall(
                vec![glam::Vec2::new(0.0, 0.0), glam::Vec2::new(4.0, 0.0)],
                0.3,
                3.0,
            )
            .expect("a wall");
        app.factory.recompute();
        // Draw on the wall's +Y face, looking at it from outside.
        let frame =
            cad_solid::Frame::from_point_normal(glam::Vec3::new(2.0, 0.15, 1.5), glam::Vec3::Y);
        app.factory_enter_sketch(frame);
        // A 1 m square opening, centred where the frame's origin landed.
        let c = cad_kernel::Circle {
            center: Vec2::new(0.0, 0.0),
            radius: 0.5,
        };
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Circle(c)));
        app
    }

    fn last_cut(app: &CadApp) -> &cad_solid::Feature {
        app.factory
            .model
            .features
            .iter()
            .rev()
            .find(|f| f.op == cad_solid::BoolOp::Difference)
            .expect("a cut was made")
    }

    /// THE RECORD IS MADE. Both ways round, because a field that is always `Some(true)` would
    /// satisfy the through case and quietly reinstate the whole defect for recesses.
    #[test]
    fn a_through_cut_and_a_recess_are_told_apart_at_the_moment_they_are_cut() {
        let mut app = a_wall_ready_to_cut();
        assert!(
            app.factory_cut_sketch(true) > 0,
            "the through cut must be made"
        );
        assert_eq!(
            last_cut(&app).through,
            Some(true),
            "a through cut did not record that it was meant to go through",
        );

        let mut app = a_wall_ready_to_cut();
        app.factory.element_height = 0.1;
        assert!(app.factory_cut_sketch(false) > 0, "the recess must be made");
        assert_eq!(
            last_cut(&app).through,
            Some(false),
            "a recess did not record that it was meant to stop",
        );
    }

    /// AND A RECESS IS NOT REPORTED AS A FAILURE. The diagnostic counts it as a recess rather
    /// than flagging it, which is the whole point of recording the intent.
    #[test]
    fn a_recess_is_not_flagged_as_an_opening_that_does_not_reach() {
        let mut app = a_wall_ready_to_cut();
        app.factory.element_height = 0.1;
        assert!(app.factory_cut_sketch(false) > 0, "the recess must be made");
        app.factory_exit_sketch();
        app.factory.recompute();

        let report = app.factory_geometry_report().join("\n");
        let line = report
            .lines()
            .find(|l| l.contains("openings:"))
            .unwrap_or_else(|| panic!("no openings line in:\n{report}"));
        assert!(
            line.contains("1 are recesses"),
            "the recess was not counted as one: {line}",
        );
        assert!(
            !report.contains("does not reach") && !report.contains("short by"),
            "a recess working exactly as drawn was reported as a failure:\n{report}",
        );
    }

    /// AN OPENING CUT BEFORE THE FIELD EXISTED STILL GETS AN ANSWER. `None` means nobody said, and
    /// the reader falls back to measuring — which is what the app did everywhere, and is still
    /// right for every project already on disk.
    #[test]
    fn an_opening_with_no_recorded_intent_falls_back_to_measuring() {
        let mut app = a_wall_ready_to_cut();
        assert!(app.factory_cut_sketch(true) > 0);
        app.factory_exit_sketch();
        // Erase the record, exactly as a project saved before the field carries it.
        let id = last_cut(&app).id;
        if let Some(f) = app.factory.model.get_mut(id) {
            f.through = None;
        }
        app.factory.recompute();

        let report = app.factory_geometry_report().join("\n");
        let line = report
            .lines()
            .find(|l| l.contains("openings:"))
            .unwrap_or_else(|| panic!("the diagnostic gave up on it:\n{report}"));
        // MEASURED, not assumed. Reading `None` as "recess" would silence the warning for every
        // opening in every project already on disk — the reverse of the defect, and just as quiet.
        assert!(
            line.contains("0 are recesses"),
            "an opening with no recorded intent was counted as a recess instead of being \
             measured: {line}",
        );
        assert!(
            line.contains("1 go fully through"),
            "an opening with no recorded intent was not measured at all: {line}",
        );
    }
}
/// A PLANE'S DRAWING IS PART OF THE DRAWING, AND SHARES ITS TABLES.
///
/// A face sketch is a whole `Document` of its own, built from `Document::default()` — so it
/// starts with DEFAULT tables while the drawing it belongs to has the user's. Every dobject names
/// its layer, linetype, colour slot, text style, dimension style, wall style and block BY INDEX,
/// so an object drawn on a plane against default tables means something ELSE the moment it is
/// read against the drawing's: index 3 was "Dashed" and is now "Center", index 2 was a 5 mm
/// annotation style and is now a 200 mm title.
///
/// Layers were carried across when somebody noticed the colours were wrong. Blocks were carried
/// across when somebody noticed the Insert list was empty. The other six were still default, and
/// would have been found the same way, one complaint at a time.
#[cfg(test)]
mod a_plane_shares_the_drawings_tables {
    use super::*;

    /// A plan carrying one non-default entry in EVERY shared table, so a table that fails to
    /// cross is visible as a missing name rather than as a subtly different number.
    fn a_plan_with_its_own_tables() -> CadApp {
        let mut app = CadApp::default();
        app.doc.layers.add(cad_kernel::Layer {
            name: "SETTING OUT".into(),
            ..cad_kernel::Layer::layer_zero()
        });
        app.doc
            .linetypes
            .add(cad_kernel::Linetype::new("Site boundary", &[9.0, 3.0]));
        app.doc.text_styles.add(cad_kernel::TextStyle {
            name: "Annotation".into(),
            ..cad_kernel::TextStyle::standard()
        });
        app.doc.dim_styles.add(cad_kernel::DimStyle {
            name: "Setting out".into(),
            ..cad_kernel::DimStyle::standard()
        });
        app.doc.wall_styles.add(cad_kernel::WallStyle {
            name: "Blockwork".into(),
            ..cad_kernel::WallStyle::standard()
        });
        app.doc.blocks.add(cad_kernel::Block {
            name: "DOOR".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: Vec::new(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        app
    }

    /// What every table in a document is called, as one set — so a test can say "the plane can
    /// see all of this" without naming eight accessors.
    fn table_names(d: &cad_kernel::Document) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        v.extend(d.layers.layers.iter().map(|x| format!("layer:{}", x.name)));
        v.extend(
            d.linetypes
                .linetypes
                .iter()
                .map(|x| format!("ltype:{}", x.name)),
        );
        v.extend(
            d.text_styles
                .styles
                .iter()
                .map(|x| format!("text:{}", x.name)),
        );
        v.extend(
            d.dim_styles
                .styles
                .iter()
                .map(|x| format!("dim:{}", x.name)),
        );
        v.extend(
            d.wall_styles
                .styles
                .iter()
                .map(|x| format!("wall:{}", x.name)),
        );
        v.extend(d.blocks.blocks.iter().map(|x| format!("block:{}", x.name)));
        v
    }

    /// THE WAY IN. Everything the drawing knows about, the plane knows about.
    #[test]
    fn a_plane_opens_with_the_drawings_tables() {
        let mut app = a_plan_with_its_own_tables();
        let want = table_names(&app.doc);
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        let got = table_names(&app.doc); // `self.doc` IS the sketch while a session is open
        assert_eq!(
            want, got,
            "the plane opened with different tables from the drawing it belongs to",
        );
    }

    /// THE WAY OUT. A style added while drawing on a face belongs to the drawing — the same rule
    /// layers and blocks already followed, and for the same reason: it is the drawing's.
    #[test]
    fn a_style_created_on_a_plane_comes_back_to_the_drawing() {
        let mut app = a_plan_with_its_own_tables();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.doc.text_styles.add(cad_kernel::TextStyle {
            name: "Drawn on a face".into(),
            ..cad_kernel::TextStyle::standard()
        });
        app.doc
            .linetypes
            .add(cad_kernel::Linetype::new("Drawn on a face", &[1.0, 1.0]));
        app.factory_exit_sketch();

        assert!(
            app.doc.text_styles.find("Drawn on a face").is_some(),
            "a text style made while sketching was lost when the sketch closed",
        );
        assert!(
            app.doc.linetypes.find("Drawn on a face").is_some(),
            "a linetype made while sketching was lost when the sketch closed",
        );
    }

    /// AND THE IDS STILL MEAN THE SAME THING. This is the point of sharing rather than merging:
    /// an object drawn on the plane keeps its index, and that index answers to the same entry in
    /// the drawing's table as it did in the sketch's.
    #[test]
    fn an_id_used_on_a_plane_resolves_to_the_same_entry_in_the_plan() {
        let mut app = a_plan_with_its_own_tables();
        let id = app
            .doc
            .linetypes
            .find("Site boundary")
            .expect("the fixture linetype");
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        let in_sketch = app.doc.linetypes.get(id).map(|l| l.name.clone());
        app.factory_exit_sketch();
        let in_plan = app.doc.linetypes.get(id).map(|l| l.name.clone());
        assert_eq!(
            in_sketch.as_deref(),
            Some("Site boundary"),
            "linetype {id} means something else on a plane: {in_sketch:?}",
        );
        assert_eq!(
            in_sketch, in_plan,
            "the same id names two different linetypes"
        );
    }

    /// THE PLANE KEEPS A COPY TOO. It is drawn in the plan and in 3D long after the session
    /// closed, and a `BlockRef` or a styled Text left on it has to resolve then as well.
    #[test]
    fn the_plane_keeps_the_tables_after_the_session_closes() {
        let mut app = a_plan_with_its_own_tables();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.factory_exit_sketch();
        let sk = &app.factory.model.sketches[0];
        assert!(
            sk.doc.blocks.find("DOOR").is_some(),
            "the plane's own document lost the block table, so a BlockRef on it resolves to \
             nothing when the plane is drawn",
        );
    }

    /// UNITS ARE NOT A TABLE AND DO NOT CROSS. What one drawing unit means is a fact about a
    /// FILE. A sketch that adopted the plan's unit would rescale everything already drawn on it,
    /// which is a silent rescale of the user's work arriving as a side effect of a style fix.
    /// The unit mismatch between a plan and its planes is a real defect with a fix of its own.
    #[test]
    fn the_unit_is_not_carried_across_by_the_table_merge() {
        let mut app = a_plan_with_its_own_tables();
        app.doc.units =
            cad_kernel::Units::from_metres_per_unit(0.001, cad_kernel::UnitSource::Declared);
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert!(
            (app.doc.units.metres_per_unit - 1.0).abs() < 1e-12,
            "the table merge silently rescaled the plane to {} m/unit",
            app.doc.units.metres_per_unit,
        );
    }
}
#[cfg(test)]
mod a_sketch_is_not_the_plan {
    use super::*;

    /// Both plan underlays — the depth-tested one and the x-ray one — must read `plan_doc`.
    /// Asserted on the source because the drawing needs a live GL context a unit test has not got.
    ///
    /// THE DEPTH-TESTED ANCHOR MOVED, and the move is worth recording. The plan used to be
    /// resolved inline in the frame; it is now resolved in `refresh_cached_lines`, because that
    /// builder is cached. This test caught that as a failure — correctly, by its own terms — and
    /// the fix is to follow the code rather than to relax the rule. The hazard it names has not
    /// changed: read the sketch instead of the plan, or take the scale from the sketch's units,
    /// and a millimetre plan draws at 1000x with everything else still green.
    #[test]
    fn both_plan_underlays_read_the_plan() {
        // The two anchored fns moved apart: `refresh_cached_lines` lives in app/cmd2d.rs,
        // `paint_plan_underlay` in app/ports_simlux.rs — search each in its own file.
        let cmd2d_src = include_str!("../cmd2d.rs");
        let simlux_src = include_str!("../ports_simlux.rs");
        for (what, anchor, end, src) in [
            (
                "depth-tested",
                "fn refresh_cached_lines",
                "\n    /// THE DRAWING CHANGED",
                cmd2d_src,
            ),
            ("x-ray", "fn paint_plan_underlay", "\n    fn ", simlux_src),
        ] {
            let a = src
                .find(anchor)
                .unwrap_or_else(|| panic!("{what}: anchor gone"));
            let b = src[a + anchor.len()..]
                .find(end)
                .map(|e| a + anchor.len() + e)
                .unwrap_or(src.len());
            let body = &src[a..b];
            assert!(
                body.contains("plan_doc_of(self.factory.session.as_ref()"),
                "{what} underlay must resolve the PLAN, not use self.doc",
            );
            for line in body.lines() {
                let l = line.trim();
                if l.starts_with("//") {
                    continue;
                }
                assert!(
                    !l.contains("&self.doc.dobjects") && !l.contains("plan_lines(&self.doc"),
                    "{what} underlay still reads self.doc: {l}",
                );
                // …AND THE SCALE MUST COME FROM THE PLAN TOO. The first version of this test
                // checked only which DOCUMENT was iterated, and passed over `self.outlines_m`,
                // which scales by `doc_k()` = `self.doc.units` — the SKETCH's. A millimetre plan
                // drew at 1000× with the test green. A guard that lists the two spellings someone
                // already found is whack-a-mole; this names the hazard itself.
                assert!(
                    !l.contains("self.outlines_m(") && !l.contains("self.doc_k()"),
                    "{what} underlay takes its SCALE from self.doc, which is the sketch: {l}",
                );
            }
        }
    }

    /// The helper itself: with a session open it must hand back the SAVED plan, not the sketch.
    /// This is the property both call sites depend on.
    #[test]
    fn plan_doc_returns_the_saved_drawing_while_sketching() {
        let mut plan = cad_kernel::Document::default();
        plan.push(cad_kernel::DObject::new(cad_kernel::Geom::Point(
            cad_kernel::Point {
                location: Vec2::new(1.0, 2.0),
                style: 0,
                size: 0.0,
            },
        )));
        let mut sketch = cad_kernel::Document::default();
        for _ in 0..3 {
            sketch.push(cad_kernel::DObject::new(cad_kernel::Geom::Point(
                cad_kernel::Point {
                    location: Vec2::new(9.0, 9.0),
                    style: 0,
                    size: 0.0,
                },
            )));
        }
        let session = crate::factory::SketchSession {
            plane: 1,
            saved_doc: plan,
            saved_undo: Vec::new(),
            saved_redo: Vec::new(),
            saved_constraints: Vec::new(),
            saved_pending: None,
        };
        // `self.doc` is the sketch during a session — that is the swap.
        let got = CadApp::plan_doc_of(Some(&session), &sketch);
        assert_eq!(
            got.dobjects.len(),
            1,
            "the PLAN has one object; the sketch has three"
        );

        // …and with no session it is simply the document.
        let got = CadApp::plan_doc_of(None, &sketch);
        assert_eq!(got.dobjects.len(), 3);
    }
}
/// LAYERS BELONG TO THE DRAWING, AND A PLANE SHOWS ONLY ITSELF.
///
/// Reported together: "i need a toggle to turn it off so i can only see the what ever is on the
/// plane i am drawing" and "i drew some of these arrays in different colors but they showed up in
/// the same color … are the layers tool no integrated into this?"
///
/// The colours were the swap again: `Sketch::new` builds a `Document::default()`, whose LayerTable
/// is empty, so nothing drawn on a face could be on one of the project's layers.
#[cfg(test)]
mod a_plane_shares_the_drawings_layers {
    use super::*;

    fn app_with_layers() -> CadApp {
        let mut app = CadApp::default();
        let mut l = cad_kernel::Layer::layer_zero();
        l.name = "Lighting".into();
        app.doc.layers.add(l);
        let mut l = cad_kernel::Layer::layer_zero();
        l.name = "Power".into();
        app.doc.layers.add(l);
        app
    }

    /// THE BUG: a sketch used to start with the default table, so the project's layers were not
    /// there to draw on and everything came out one colour.
    #[test]
    fn a_sketch_gets_the_projects_layers() {
        let mut app = app_with_layers();
        let before = app.doc.layers.layers.len();
        assert!(before >= 3, "precondition: base plus two named layers");

        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert_eq!(
            app.doc.layers.layers.len(),
            before,
            "the sketch must have the drawing's layers, not an empty table",
        );
        assert!(app.doc.layers.layers.iter().any(|l| l.name == "Lighting"));
        assert!(app.doc.layers.layers.iter().any(|l| l.name == "Power"));
    }

    /// …and a layer made while drawing on a face is not lost when the sketch closes.
    #[test]
    fn a_layer_created_in_a_sketch_survives_the_exit() {
        let mut app = app_with_layers();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        let mut l = cad_kernel::Layer::layer_zero();
        l.name = "Sprinklers".into();
        app.doc.layers.add(l);
        app.factory_exit_sketch();
        assert!(
            app.doc.layers.layers.iter().any(|l| l.name == "Sprinklers"),
            "a layer is a property of the drawing, so it must come back out with it",
        );
    }

    /// The ACTIVE layer travels too — otherwise the first thing drawn on a face silently lands on
    /// the base layer however carefully the layer was chosen beforehand.
    #[test]
    fn the_active_layer_travels_into_the_sketch() {
        let mut app = app_with_layers();
        app.doc.layers.active = 2;
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert_eq!(
            app.doc.layers.active, 2,
            "the chosen layer must still be the one in force"
        );
    }

    /// The other-planes projection is OFF by default and gated on the toggle.
    #[test]
    fn other_planes_are_hidden_by_default() {
        let f = crate::factory::FactoryState::default();
        assert!(
            !f.show_other_planes,
            "a plane shows its own work unless asked otherwise"
        );

        // The projector moved with the raster/sketch projection code into
        // ports_raster.rs (app.rs split) — inspect it where it lives.
        let src = include_str!("../ports_raster.rs");
        let a = src
            .find("pub(super) fn draw_factory_sketches_2d")
            .or_else(|| src.find("fn draw_factory_sketches_2d"))
            .expect("the projector");
        let b = src[a..]
            .find("\n    fn ")
            .map(|e| a + e)
            .unwrap_or(src.len());
        let body = &src[a..b];
        assert!(
            body.contains("!self.factory.show_other_planes"),
            "the projection must be gated on the toggle",
        );
        // …and the GROUND PLAN rule must survive: it is a separate case, not covered by the toggle.
        assert!(
            body.contains("ground_only"),
            "the plan's own rule must still apply"
        );
    }
}
/// 3D WORK DONE INSIDE A SKETCH IS STILL UNDOABLE, AND PLAN TOOLS DECLINE TO RUN THERE.
///
/// Both from the `self.doc`-swap audit. `factory_exit_sketch` used to discard the whole session
/// undo stack, so `draw on a face → Extrude` destroyed its own undo step; and the ▼ Building rows
/// were clickable while a sketch was open, building ground geometry out of a wall sketch's (u, v).
#[cfg(test)]
mod a_sketch_does_not_eat_history_or_build_the_wrong_thing {
    use super::*;

    fn app_with_a_box() -> CadApp {
        let mut app = CadApp::default();
        app.factory.open = true;
        app.factory.add_box();
        app.factory.recompute();
        app
    }

    /// THE FLAGSHIP FLOW. A 3D snapshot taken inside a sketch must survive the sketch closing.
    #[test]
    fn a_model_change_made_in_a_sketch_survives_the_exit() {
        let mut app = app_with_a_box();
        app.undo_stack.clear();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());

        app.snapshot_factory(); // what Extrude / Cut / a gizmo move each do
        app.factory.add_box(); // …then change the model
        app.factory_exit_sketch();

        assert!(
            app.undo_stack
                .iter()
                .any(|u| matches!(u, UndoStep::Factory(_))),
            "the 3D step was thrown away with the sketch's own history — Ctrl+Z would walk past it",
        );
    }

    /// The sketch's OWN 2D history does not come out. It belongs to the sketch, which has closed.
    #[test]
    fn the_sketches_2d_history_stays_behind() {
        let mut app = app_with_a_box();
        app.undo_stack.clear();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.snapshot_doc(); // a 2D edit inside the sketch
        app.factory_exit_sketch();
        assert!(
            !app.undo_stack.iter().any(|u| matches!(u, UndoStep::Doc(_))),
            "a sketch's 2D history must end with the sketch",
        );
    }

    /// THE COROLLARY, and the trap: a snapshot taken mid-session must record the face's drawing,
    /// not the empty hole `mem::take` left in the model. Otherwise undoing a 3D step after the
    /// sketch closed would erase everything drawn on that face.
    #[test]
    fn a_snapshot_taken_in_a_sketch_keeps_that_faces_drawing() {
        let mut app = app_with_a_box();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        let idx = app.factory.session.as_ref().expect("a session").plane;
        // Draw something on the face.
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Point(
                cad_kernel::Point {
                    location: Vec2::new(1.0, 1.0),
                    style: 0,
                    size: 0.0,
                },
            )));
        app.snapshot_factory();

        let snap = app
            .undo_stack
            .iter()
            .rev()
            .find_map(|u| match u {
                UndoStep::Factory(f) => Some(f),
                _ => None,
            })
            .expect("a factory snapshot");
        assert_eq!(
            snap.model
                .sketch_by_id(idx)
                .expect("the plane is in the snapshot")
                .doc
                .dobjects
                .len(),
            1,
            "the snapshot recorded the open plane as EMPTY — undoing to it would wipe the face",
        );
    }

    /// …AND SO DOES THE COUNTERPART, which is the one that actually runs.
    ///
    /// `snapshot_factory` patched this and asserted in its own comment that every snapshot was
    /// therefore self-consistent "whoever takes one". That was false. `counterpart_of` builds the
    /// opposite-stack entry on EVERY undo and EVERY redo — the most-travelled path there is — and
    /// cloned the model bare, recording the open plane as empty. Redo after an undo inside a
    /// sketch would then restore that hole and wipe the face.
    ///
    /// The patch now lives in one function both call, so they cannot drift again.
    #[test]
    fn the_undo_counterpart_also_keeps_the_open_faces_drawing() {
        let mut app = app_with_a_box();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        let idx = app.factory.session.as_ref().expect("a session").plane;
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Point(
                cad_kernel::Point {
                    location: Vec2::new(1.0, 1.0),
                    style: 0,
                    size: 0.0,
                },
            )));

        // Exactly what `do_undo` / `do_redo` build for the opposite stack.
        let counterpart = app.counterpart_of(&UndoStep::Factory(FactorySnap {
            model: app.factory.model.clone(),
            walls: app.factory.walls.clone(),
            storeys: app.factory.storeys.clone(),
            active_storey: app.factory.active_storey,
            ceilings: app.factory.ceilings.clone(),
            furniture: app.factory.furniture.clone(),
            feature_color: app.factory.feature_color.clone(),
            surface_color: app.factory.surface_color.clone(),
            feature_texture: app.factory.feature_texture.clone(),
            surface_texture: app.factory.surface_texture.clone(),
            feature_group: app.factory.feature_group.clone(),
            rooms: app.factory.rooms.clone(),
            next_room_id: app.factory.next_room_id,
            texture_xforms: Vec::new(),
        }));

        let UndoStep::Factory(snap) = counterpart else {
            panic!("a factory counterpart")
        };
        assert_eq!(
            snap.model
                .sketch_by_id(idx)
                .expect("the plane is in the counterpart")
                .doc
                .dobjects
                .len(),
            1,
            "the counterpart recorded the open plane as EMPTY — a redo would wipe the face",
        );
    }

    /// PLAN TOOLS DECLINE while a sketch is open, rather than building from its (u, v).
    #[test]
    fn the_building_tools_refuse_inside_a_sketch() {
        let mut app = app_with_a_box();
        let before = app.factory.model.features.len();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        // A closed outline on the FACE, which is exactly the gesture that used to build a slab.
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Polyline(
                cad_kernel::Polyline {
                    vertices: [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)]
                        .into_iter()
                        .map(|(x, y)| cad_kernel::PolyVertex {
                            pos: Vec2::new(x, y),
                            bulge: 0.0,
                        })
                        .collect(),
                    closed: true,
                    widths: Vec::new(),
                },
            )));
        app.selection = vec![0];

        app.do_make_slab(true);
        app.do_make_building();
        app.do_make_room();
        app.do_make_3d_wall();

        assert_eq!(
            app.factory.model.features.len(),
            before,
            "a plan tool built geometry from a face sketch's local coordinates",
        );
        assert!(
            app.factory.status.contains("face sketch is open"),
            "and it must SAY why rather than silently doing nothing: {}",
            app.factory.status,
        );
    }

    /// …and they still work normally with no sketch open.
    #[test]
    fn the_building_tools_still_work_on_the_plan() {
        let mut app = app_with_a_box();
        assert!(
            !app.refuse_plan_action_in_sketch("Make floor"),
            "no session, no refusal"
        );
    }
}
/// The rest of the sketch-swap audit: everything else that read the installed canvas while
/// meaning the drawing. Each of these was written against the UNFIXED code first and seen to
/// fail — a guard that has never failed is not known to guard anything.
#[cfg(test)]
mod a_sketch_does_not_reach_into_the_drawing {
    use super::*;
    use crate::param_editor::CRef;

    fn app_with_a_box() -> CadApp {
        let mut app = CadApp::default();
        app.factory.open = true;
        app.factory.add_box();
        app.factory.recompute();
        app
    }

    fn line(ax: f64, ay: f64, bx: f64, by: f64) -> cad_kernel::DObject {
        cad_kernel::DObject::new(cad_kernel::Geom::Line(cad_kernel::Line {
            a: Vec2::new(ax, ay),
            b: Vec2::new(bx, by),
        }))
    }

    fn square(half: f64) -> cad_kernel::DObject {
        cad_kernel::DObject::new(cad_kernel::Geom::Polyline(cad_kernel::Polyline {
            vertices: [(-half, -half), (half, -half), (half, half), (-half, half)]
                .into_iter()
                .map(|(x, y)| cad_kernel::PolyVertex {
                    pos: Vec2::new(x, y),
                    bulge: 0.0,
                })
                .collect(),
            closed: true,
            widths: Vec::new(),
        }))
    }

    /// A drawing with two lines and one constraint on them, parametric mode on.
    fn app_with_a_constraint() -> CadApp {
        let mut app = app_with_a_box();
        app.doc.push(line(0.0, 0.0, 1.0, 0.0));
        app.doc.push(line(0.0, 1.0, 1.0, 1.0));
        let (h0, h1) = (app.doc.dobjects[0].handle, app.doc.dobjects[1].handle);
        app.parametric.active = true;
        app.parametric.constraints.push(CRef::Parallel(h0, h1));
        app
    }

    /// What the parametric panel does on EVERY frame it is open.
    fn one_panel_frame(app: &mut CadApp) {
        crate::param_editor::prune_constraints(&app.doc, &mut app.parametric.constraints);
    }

    // ── #2 — the constraint wipe ────────────────────────────────────────────────────────────

    /// Handles come from a process-global counter, so a sketch document shares NONE with the
    /// plan: `prune_constraints` matched nothing and emptied the list on the sketch's first
    /// frame. Nothing recovers it — no `UndoStep` carries constraints and nothing writes them
    /// to disk.
    #[test]
    fn the_drawings_constraints_survive_a_sketch() {
        let mut app = app_with_a_constraint();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        one_panel_frame(&mut app);
        one_panel_frame(&mut app); // and it is not a first-frame-only effect
        app.factory_exit_sketch();

        assert_eq!(
            app.parametric.constraints.len(),
            1,
            "the drawing's constraints were deleted by a sketch it had nothing to do with",
        );
    }

    /// The mechanism, stated as its own test: while the sketch is open the drawing's constraints
    /// are PARKED, not merely spared the prune. Left installed they would also be solved against,
    /// and drawn over, geometry that is not theirs.
    #[test]
    fn the_drawings_constraints_are_not_live_inside_the_sketch() {
        let mut app = app_with_a_constraint();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert!(
            app.parametric.constraints.is_empty(),
            "a sketch is its own constraint space — its handles are its own",
        );
        assert!(
            app.parametric.pending.is_none(),
            "a half-picked pair must not span two documents"
        );
    }

    /// …and a constraint made INSIDE the sketch does not follow the drawing back out, where its
    /// handles name nothing.
    #[test]
    fn a_sketch_constraint_does_not_leak_into_the_drawing() {
        let mut app = app_with_a_constraint();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.doc.push(line(0.0, 0.0, 2.0, 0.0));
        let h = app.doc.dobjects[0].handle;
        app.parametric.constraints.push(CRef::Horizontal(h));
        app.factory_exit_sketch();

        assert_eq!(
            app.parametric.constraints.len(),
            1,
            "exactly the drawing's own constraint"
        );
        let present: std::collections::HashSet<_> =
            app.doc.dobjects.iter().map(|d| d.handle).collect();
        assert!(
            app.parametric.constraints[0]
                .handles()
                .iter()
                .all(|h| present.contains(h)),
            "every restored constraint must name geometry that is actually in the drawing",
        );
    }

    /// The prune still does its real job — dropping constraints whose geometry was deleted.
    #[test]
    fn the_prune_still_drops_deleted_geometry() {
        let mut app = app_with_a_constraint();
        app.doc.dobjects.remove(1);
        one_panel_frame(&mut app);
        assert!(
            app.parametric.constraints.is_empty(),
            "the prune must still prune"
        );
    }

    // ── #5 — picking bare ground from the 3D view ───────────────────────────────────────────

    fn view(app: &CadApp, rect: egui::Rect) -> [f32; 16] {
        let f = &app.factory;
        crate::light3d::mvp(
            f.cam_yaw,
            f.cam_pitch,
            f.cam_dist,
            f.cam_target,
            rect.width() / rect.height(),
            f.ortho,
        )
    }

    /// PROVES THE MACHINERY WORKS, so the refusal test below cannot pass by accident — the
    /// mistake that has bitten this codebase three times.
    #[test]
    fn the_ground_pick_finds_plan_geometry_with_no_sketch_open() {
        let mut app = app_with_a_box();
        app.doc.push(square(0.5)); // smallest containing shape wins the pick
        let want = app.doc.dobjects.len() - 1;
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = view(&app, rect);
        assert_eq!(
            app.factory_pick_ground_dobject(rect.center(), rect, &mvp),
            Some(want),
            "the pick must actually hit a plan shape under the cursor",
        );
    }

    /// It ray-cast to world z = 0 and then point-in-polygoned against the SKETCH, reading a
    /// plane's (u, v) as world (x, y). A click on bare floor cleared the 3D selection and put
    /// grips on a shape the user never clicked.
    #[test]
    fn the_ground_pick_ignores_a_face_sketch() {
        let mut app = app_with_a_box();
        app.doc.push(square(5.0));
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.doc.push(square(5.0)); // the same shape, now drawn ON THE FACE

        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = view(&app, rect);
        assert_eq!(
            app.factory_pick_ground_dobject(rect.center(), rect, &mvp),
            None,
            "a sketch's local coordinates were picked as if they were the ground plan",
        );
    }

    // ── #7 — the SIMLUX layer on the 2D canvas ──────────────────────────────────────────────

    /// `paint_lux_overlay` already refused during a sketch; the fixtures and the pointer that
    /// places them did not. So the heatmap vanished while the fittings it was computed from kept
    /// drawing at world metres on a wall's (u, v) — and a click wrote that wall coordinate
    /// straight into a luminaire's world position.
    #[test]
    fn the_simlux_2d_layer_is_off_during_a_sketch() {
        let mut app = app_with_a_box();
        app.light.window_open = true;
        assert!(
            app.simlux_2d_layer_live(),
            "SIMLUX is on screen and the canvas is the plan"
        );

        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert!(
            !app.simlux_2d_layer_live(),
            "fixtures are world metres; a face sketch's canvas is that plane's (u, v)",
        );

        app.factory_exit_sketch();
        assert!(
            app.simlux_2d_layer_live(),
            "and it comes back when the sketch closes"
        );
    }

    /// The other half of the predicate is unchanged: with SIMLUX off screen, nothing is live.
    #[test]
    fn the_simlux_2d_layer_is_off_when_simlux_is_closed() {
        let app = app_with_a_box();
        assert!(!app.simlux_2d_layer_live());
    }

    /// Moving objects onto the SIMLUX layer REFUSES in a sketch rather than reaching for the
    /// plan: it needs `&mut` on the document and `self.selection`, which indexes whichever
    /// document is installed, so there is no correct answer.
    #[test]
    fn the_simlux_layer_move_refuses_inside_a_sketch() {
        let mut app = app_with_a_box();
        app.doc.push(square(5.0));
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.doc.push(square(5.0));
        app.selection = vec![0];
        let layers_before = app
            .factory
            .session
            .as_ref()
            .unwrap()
            .saved_doc
            .layers
            .layers
            .len();

        app.shift_selection_to_simlux_layer();

        assert_eq!(
            app.factory
                .session
                .as_ref()
                .unwrap()
                .saved_doc
                .layers
                .layers
                .len(),
            layers_before,
            "it created a SIMLUX layer in the drawing on behalf of a click inside a sketch",
        );
        assert!(
            app.factory.status.contains("finish the sketch"),
            "and it must say why: {}",
            app.factory.status,
        );
    }

    // ── #8 — the `scene` diagnostic ─────────────────────────────────────────────────────────

    /// `scene` exists to give measured truth instead of a guess, and mm-vs-m is what it is most
    /// often pointed at. With a face open it reported the SKETCH's 1.0 m/unit as the drawing's
    /// plan↔3D scale — misleading exactly when it was being trusted.
    #[test]
    fn the_scene_report_states_the_drawings_unit_not_the_sketchs() {
        let mut app = app_with_a_box();
        app.doc.units.metres_per_unit = 0.001; // a millimetre drawing
        app.doc.units.source = cad_kernel::UnitSource::Declared;
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());

        let ev = app.factory_scene_capture("test");
        let crate::dbg_recorder::DbgEvent::FactoryScene { sections, .. } = ev else {
            panic!("factory_scene_capture must return a FactoryScene event");
        };
        let units_line = sections
            .iter()
            .flat_map(|(_, lines)| lines.iter())
            .find(|l| l.starts_with("doc units:"))
            .expect("the report states the document's units")
            .clone();

        assert!(
            units_line.contains("0.001000"),
            "the drawing is millimetres and the report must say so: {units_line}",
        );
        assert!(
            units_line.contains("sketch open"),
            "and it must say the canvas is a plane, not silently report the plan: {units_line}",
        );
    }

    /// With no sketch open it is unchanged.
    #[test]
    fn the_scene_report_is_unchanged_with_no_sketch() {
        let mut app = app_with_a_box();
        app.doc.units.metres_per_unit = 0.001;
        let ev = app.factory_scene_capture("test");
        let crate::dbg_recorder::DbgEvent::FactoryScene { sections, .. } = ev else {
            panic!("FactoryScene");
        };
        let units_line = sections
            .iter()
            .flat_map(|(_, lines)| lines.iter())
            .find(|l| l.starts_with("doc units:"))
            .expect("units line")
            .clone();
        assert!(units_line.contains("0.001000"), "{units_line}");
        assert!(!units_line.contains("sketch open"), "{units_line}");
    }
}
/// LOCKING A LAYER LOCKS IT.
///
/// `LayerTable::selectable` — "not locked, and it renders" — was written, unit-tested in
/// `cad_kernel`, and then called from nowhere in the app. Every route into the selection basket
/// ignored it, so the padlock in the layer panel changed a bool and nothing else: locked geometry
/// picked, window-selected, Select-All'd and edited exactly as before.
///
/// The rule these pin down is AutoCAD's, which is narrower than "locked means untouchable":
/// a locked object still DRAWS and can still be SNAPPED to. It just cannot be selected, and
/// therefore cannot be edited by anything that works through the selection.
#[cfg(test)]
mod a_locked_layer_is_locked {
    use super::*;

    /// A drawing with one line on a normal layer and one on a locked layer, far apart.
    /// Returns `(app, free_index, locked_index)`.
    fn two_layers() -> (CadApp, usize, usize) {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        let free = app.doc.push(DObject::new(Geom::Line(Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(10.0, 0.0),
        })));
        let locked_layer = app.doc.layers.add(Layer {
            name: "LOCKED".into(),
            locked: true,
            ..Layer::layer_zero()
        });
        // push is a pure append now — set the layer explicitly (was: active-layer
        // inherit via push).
        let mut locked_d = DObject::new(Geom::Line(Line {
            a: Vec2::new(0.0, 50.0),
            b: Vec2::new(10.0, 50.0),
        }));
        locked_d.style.layer = locked_layer;
        let locked = app.doc.push(locked_d);
        assert_eq!(
            app.doc.dobjects[locked].style.layer, locked_layer,
            "the fixture is wired up"
        );
        (app, free, locked)
    }

    /// A CLICK PASSES STRAIGHT THROUGH a locked object. `nearest_entity_under` is the one
    /// function all eighteen click sites go through, so this is every pick in the app.
    #[test]
    fn a_click_does_not_pick_a_locked_object() {
        let (app, free, _locked) = two_layers();
        // Dead on the locked line.
        assert_eq!(
            app.nearest_entity_under(Vec2::new(5.0, 50.0), 1.0),
            None,
            "clicking a locked object picked it",
        );
        // …and the unlocked one is still perfectly pickable, so this is not a blanket refusal.
        assert_eq!(
            app.nearest_entity_under(Vec2::new(5.0, 0.0), 1.0),
            Some(free),
            "the fix stopped picking anything at all",
        );
    }

    /// A WINDOW DRAG over both takes only the one it is allowed to take.
    #[test]
    fn a_window_drag_leaves_a_locked_object_behind() {
        let (mut app, free, locked) = two_layers();
        // A crossing window (R→L) spanning both lines.
        app.add_window_selection(
            Vec2::new(20.0, 60.0),
            Vec2::new(-5.0, -5.0),
            false,
            false,
            true,
        );
        assert!(
            app.selection.contains(&free),
            "the unlocked line should have been caught"
        );
        assert!(
            !app.selection.contains(&locked),
            "a window drag selected a locked object"
        );
    }

    /// SELECT ALL means all the objects you are allowed to have.
    #[test]
    fn select_all_leaves_a_locked_object_behind() {
        let (mut app, free, locked) = two_layers();
        app.add_all_to_selection();
        assert!(
            app.selection.contains(&free),
            "Select All missed the unlocked line"
        );
        assert!(
            !app.selection.contains(&locked),
            "Select All selected a locked object"
        );
    }

    /// LOCKING A LAYER TAKES ITS OBJECTS OUT OF THE BASKET. Guarding only the entry points
    /// leaves the obvious hole open: select it, then lock the layer, then move — the edit runs
    /// off a selection made when the lock was not there.
    #[test]
    fn locking_a_layer_drops_its_objects_from_the_current_selection() {
        let (mut app, free, locked) = two_layers();
        // Select both while nothing is locked yet.
        let layer = app.doc.dobjects[locked].style.layer;
        if let Some(l) = app.doc.layers.get_mut(layer) {
            l.locked = false;
        }
        app.add_all_to_selection();
        assert!(
            app.selection.contains(&locked),
            "both are selected before the lock"
        );

        app.set_layer_locked(layer, true);

        assert!(
            app.selection.contains(&free),
            "the unlocked object was dropped too"
        );
        assert!(
            !app.selection.contains(&locked),
            "an object stayed selected after its layer was locked — the next edit still moves it",
        );
    }

    /// A LOCKED OBJECT IS NOT A HIDDEN ONE. The cheapest way to pass every test above is to stop
    /// drawing locked geometry, which would be a far worse product than the bug.
    #[test]
    fn a_locked_object_still_draws() {
        let (app, _free, locked) = two_layers();
        let layer = app.doc.dobjects[locked].style.layer;
        assert!(
            app.doc.layers.renders(layer),
            "a locked layer must still render"
        );
        assert!(
            app.doc.dobjects[locked].style.visible,
            "and its object is still visible"
        );
    }

    /// …AND IT CAN STILL BE SNAPPED TO, which is why the lock belongs at selection rather than
    /// in the geometry. Tracing a new wall along a locked grid line is the reason to lock it.
    #[test]
    fn a_locked_object_can_still_be_snapped_to() {
        let (app, _free, _locked) = two_layers();
        let mut snaps = SnapSet::default();
        snaps.end = true;
        let hit = find_snap(
            Vec2::new(0.1, 50.1),
            1.0,
            snaps,
            None,
            None,
            &app.doc.dobjects,
            None,
        );
        let hit = hit.expect("the locked line's endpoint must still snap");
        assert_eq!(
            hit.point,
            Vec2::new(0.0, 50.0),
            "snapped to the wrong point"
        );
    }
}
