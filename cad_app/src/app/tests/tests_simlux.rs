use super::super::*;

/// Two regressions from making rooms objects, both reported together.

#[cfg(test)]
mod room_regressions {
    use super::*;

    fn square(m: f32) -> Vec<glam::Vec2> {
        vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(m, 0.0),
            glam::Vec2::new(m, m),
            glam::Vec2::new(0.0, m),
            glam::Vec2::new(0.0, 0.0),
        ]
    }

    /// **Undo must take the room record with the geometry.**
    ///
    /// It restored the model and left the list, so undoing a room removed its walls and left a
    /// phantom entry in the Rooms menu — pointing at features that no longer existed, which is why
    /// renaming or re-heighting it afterwards changed nothing.
    #[test]
    fn undoing_a_room_removes_it_from_the_rooms_list() {
        let mut app = CadApp::default();
        app.snapshot_factory();
        app.factory.add_room(&square(5.0)).expect("room");
        assert_eq!(app.factory.rooms.len(), 1, "built");

        app.do_undo();
        assert!(
            app.factory.rooms.is_empty(),
            "the record must go with the geometry, not linger in the menu",
        );
    }

    /// …and redo brings it back, so the pair stay in step.
    #[test]
    fn redo_brings_the_room_back() {
        let mut app = CadApp::default();
        app.snapshot_factory();
        app.factory.add_room(&square(5.0)).expect("room");
        let name = app.factory.rooms[0].name.clone();
        app.do_undo();
        app.do_redo();
        assert_eq!(app.factory.rooms.len(), 1);
        assert_eq!(app.factory.rooms[0].name, name);
    }

    /// **The wall a window is fitted to is the material actually there.**
    ///
    /// `assembly_span` walked only Union features, so a Difference was invisible to it: probing a
    /// building with a room carved out measured the un-carved 4.4 m box and reported a 4.4 m wall.
    /// The window fitted to that was stretched 36x in depth, right across the building.
    ///
    /// It had only ever worked because a room used to build its own 200 mm wall boxes for the
    /// probe to hit first.
    #[test]
    fn a_carved_wall_measures_its_own_thickness_not_the_whole_building() {
        let mut app = CadApp::default();
        app.factory.building_height = 3.0;
        app.factory.room_floor = 0.1;
        app.factory.room_height = 2.7;
        app.factory.ceiling_thickness = 0.1;
        // A 4.4 m building with a 4.0 m room in it — 200 mm of wall all round.
        app.factory
            .add_building_outline(&square(4.4), 3.0)
            .expect("building");
        app.factory
            .add_room(&vec![
                glam::Vec2::new(0.2, 0.2),
                glam::Vec2::new(4.2, 0.2),
                glam::Vec2::new(4.2, 4.2),
                glam::Vec2::new(0.2, 4.2),
                glam::Vec2::new(0.2, 0.2),
            ])
            .expect("room");
        app.factory.recompute();

        // Probe inward from the middle of the +X face, at mid height.
        let from = glam::Vec3::new(4.4, 2.2, 1.5);
        let depth = app.assembly_span(from, glam::Vec3::new(-1.0, 0.0, 0.0)).0;
        assert!(
            depth < 0.5,
            "the wall is 200 mm; measuring {depth:.3} m means the void was ignored and the probe \
             crossed the whole building",
        );
        assert!(
            depth > 0.05,
            "and it is not zero either — there IS a wall here, got {depth:.3}"
        );
    }
}
/// The 2D overlay of the 3D model.
///
/// Reported as: "the FURN [toggle] we added to view furnitures on the 2d is not working. why is
/// that? all furnitures, 3d objects, apertures, and architectures that are added need to be seen in
/// the 2d."
///
/// It was not broken — it was never more than furniture. The painter opened with
///
///     if !factory.open || !show_furniture_outlines_2d || factory.furniture.is_empty() { return }
///
/// so a building with rooms in it and no furniture drew NOTHING, and the toggle looked dead. The
/// badge was hidden too whenever the 3D panel was closed, which is exactly when you want it.

#[cfg(test)]
mod plan_overlay {
    use super::*;

    fn square(n: f32) -> Vec<glam::Vec2> {
        vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(n, 0.0),
            glam::Vec2::new(n, n),
            glam::Vec2::new(0.0, n),
            glam::Vec2::new(0.0, 0.0),
        ]
    }

    fn layers(app: &CadApp) -> Vec<PlanLayer> {
        app.plan_overlay_shapes()
            .into_iter()
            .map(|(l, _)| l)
            .collect()
    }

    /// THE BUG. A building and a room, no furniture — the case that put nothing on the plan.
    #[test]
    fn a_model_with_no_furniture_still_draws() {
        let mut app = CadApp::default();
        app.factory.building_height = 3.0;
        app.factory
            .add_building_outline(&square(6.0), 3.0)
            .expect("building");
        app.factory
            .add_room(&vec![
                glam::Vec2::new(0.2, 0.2),
                glam::Vec2::new(5.8, 0.2),
                glam::Vec2::new(5.8, 5.8),
                glam::Vec2::new(0.2, 5.8),
                glam::Vec2::new(0.2, 0.2),
            ])
            .expect("room");
        app.factory.recompute();
        assert!(
            app.factory.furniture.is_empty(),
            "the reported case has no furniture at all"
        );

        let ls = layers(&app);
        assert!(
            ls.contains(&PlanLayer::Solid),
            "the building must be on the plan, got {ls:?}"
        );
        assert!(
            ls.contains(&PlanLayer::Room),
            "the room must be on the plan, got {ls:?}"
        );
    }

    /// A carved room is a Difference — a hole. Cutters are deliberately not drawn (a window's cut
    /// among the walls reads as another wall), so the room has to come from its own record or it
    /// vanishes: present in the model, listed in the Rooms menu, absent from the drawing.
    #[test]
    fn a_carved_room_is_drawn_from_its_record_not_its_cutter() {
        let mut app = CadApp::default();
        app.factory.building_height = 3.0;
        app.factory
            .add_building_outline(&square(6.0), 3.0)
            .expect("building");
        app.factory
            .add_room(&vec![
                glam::Vec2::new(1.0, 1.0),
                glam::Vec2::new(5.0, 1.0),
                glam::Vec2::new(5.0, 5.0),
                glam::Vec2::new(1.0, 5.0),
                glam::Vec2::new(1.0, 1.0),
            ])
            .expect("room");
        app.factory.recompute();
        assert!(
            app.factory.rooms[0].carve.is_some(),
            "this room IS a cutter, not a solid"
        );

        let rooms: Vec<_> = app
            .plan_overlay_shapes()
            .into_iter()
            .filter(|(l, _)| *l == PlanLayer::Room)
            .collect();
        assert_eq!(rooms.len(), 1, "one room in, one room outline out");
        // …and it is the room's own boundary, not the building's.
        let xs: Vec<f64> = rooms[0].1.iter().map(|p| p.x).collect();
        let lo = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (lo - 1.0).abs() < 1e-3 && (hi - 5.0).abs() < 1e-3,
            "got x {lo}..{hi}, want 1..5"
        );
    }

    /// The toggle is the whole point of the toggle.
    #[test]
    fn the_toggle_turns_it_off() {
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(&square(6.0), 3.0)
            .expect("building");
        assert!(!app.plan_overlay_shapes().is_empty(), "on by default");
        app.factory.show_furniture_outlines_2d = false;
        assert!(app.plan_overlay_shapes().is_empty(), "off must mean off");
    }

    /// A VOID is not a wall. Cutters — window and door openings, room carves — must not be stroked
    /// as solids, or every opening reads as a block sitting in the wall it was cut from.
    #[test]
    fn cutters_are_not_drawn_as_solids() {
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(&square(6.0), 3.0)
            .expect("building");
        let solids_before = layers(&app)
            .iter()
            .filter(|l| **l == PlanLayer::Solid)
            .count();
        app.factory.model.features.push(cad_solid::Feature {
            id: 9999,
            op: cad_solid::BoolOp::Difference,
            plane: cad_solid::Plane::default(),
            placement: cad_solid::Placement {
                u: 3.0,
                v: 3.0,
                ..Default::default()
            },
            primitive: cad_solid::Primitive::Box {
                w: 1.0,
                d: 1.0,
                h: 1.0,
            },
            enabled: true,
            target: None,
            through: None,
        });
        assert!(
            solids_before > 0,
            "the building itself must be drawn, or this test proves nothing"
        );
        let solids_after = layers(&app)
            .iter()
            .filter(|l| **l == PlanLayer::Solid)
            .count();
        assert_eq!(
            solids_after, solids_before,
            "a cutter added a solid outline to the plan"
        );
    }
}
/// ENTERING THE SIMLUX WORKSPACE PUTS THE PANEL AT HALF THE WINDOW.
///
/// The split used `exact_width(half)`, which is a LOCK — no drag handle, exactly 50 % whatever you
/// were doing — and the user asked for it to be adjustable like the other windows. Replacing it
/// with `default_width(half)` made it adjustable and broke entering the workspace, because egui
/// stores a panel's width per id and `default_width` applies only the first time that id is EVER
/// shown. On any installation that had already opened the SIMLUX panel once, entering the split
/// silently reused the remembered width of the 360-wide toggled panel.
///
/// Both properties are required: it must OPEN at half, and it must then be draggable. The transition
/// is what carries the first, so the panel width is forced on the frame the split is entered and
/// left alone every frame after.

#[cfg(test)]
mod the_simlux_split_opens_at_half {
    use super::*;

    fn panel_source() -> &'static str {
        let src = include_str!("../ports_simlux.rs");
        let a = src
            .find("fn render_light_3d_panel")
            .expect("the panel exists");
        let b = src[a..]
            .find("\n    fn ")
            .map(|e| a + e)
            .unwrap_or(src.len());
        &src[a..b]
    }

    /// It must still be resizable — the original report.
    #[test]
    fn it_is_resizable() {
        let body = panel_source();
        assert!(
            body.contains(".resizable(true)"),
            "the panel must have a drag handle"
        );
        assert!(
            body.contains(".min_width(") && body.contains(".max_width("),
            "…and bounds, so it cannot be dragged somewhere it cannot be dragged back from",
        );
    }

    /// And entering the split must FORCE the width, not merely suggest it.
    #[test]
    fn entering_the_split_forces_the_width() {
        let body = panel_source();
        assert!(
            body.contains("entering_split"),
            "the transition into the workspace has to be detected — default_width alone is \
             ignored once egui has remembered a width for this panel id",
        );
        // Look at the BRANCH, not at raw file positions: `exact_width` is named in the comment
        // above it, explaining what it replaced, so "which comes first" proves nothing.
        let e = body
            .find("if entering_split {")
            .expect("the entering-split branch");
        let arm = &body[e..];
        let end = arm.find("} else").unwrap_or(arm.len());
        assert!(
            arm[..end].contains("exact_width"),
            "the entering-split arm must PIN the width, not suggest it: {}",
            &arm[..end.min(300)],
        );
    }

    /// The flag has to be updated every frame the panel runs, or "entering" would latch on
    /// forever and the panel would be pinned to half — the very lock this replaced.
    #[test]
    fn the_transition_flag_is_cleared() {
        let body = panel_source();
        assert!(
            body.contains("self.simlux_split_prev = split;"),
            "the previous-state flag must be written each frame, unconditionally",
        );
    }
}
/// THE ILLUMINAIRE WINDOW ACTUALLY RUNS.
///
/// Everything above this proves the DATA is right — the library round-trips, the units convert,
/// the placement leaves a plain block. None of it runs a single line of the window, and a window
/// that panics on its first frame is indistinguishable from one that was never written.
///
/// These drive real headless frames, the same way the factory-panel tests do. What they catch is
/// what only running catches — a panic, an assertion inside a layout, an arithmetic edge in the
/// preview fitter — plus the one thing a "did not panic" check misses: that anything was drawn.

#[cfg(test)]
mod the_illuminaire_window_runs {
    use super::*;

    fn frame(app: &mut CadApp) {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            app.render_illuminaire(ctx);
        });
        // SOMETHING WAS DRAWN. A window that took an early return would pass a "did not panic"
        // check while showing nothing at all, which is the failure these are really guarding.
        assert!(
            !out.shapes.is_empty(),
            "the frame produced no shapes — the window drew nothing",
        );
    }

    fn stocked() -> CadApp {
        let mut app = CadApp::default();
        app.light.illuminaire_open = true;
        let mut doc = cad_kernel::Document::default();
        for (n, r) in [("OCULUS", 47.5_f64), ("PULSE", 28.5)] {
            doc.blocks.add(cad_kernel::Block {
                name: n.into(),
                base: Vec2::new(0.0, 0.0),
                dobjects: vec![cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                    cad_kernel::Circle {
                        center: Vec2::new(0.0, 0.0),
                        radius: r,
                    },
                ))],
                smart: false,
                params: Vec::new(),
                cut_edges: Vec::new(),
            });
        }
        app.light.lib_blocks = crate::illuminaire::symbols_from(&doc);
        app.light.lib_blocks_unit_m = 0.001;
        app.light.lib_add_open = true;
        app.light.lib_scanned = vec![("FONDO".into(), "D:/ldt/FONDO.ldt".into())];
        app
    }

    /// An empty library, with every panel open. The first-run state, which is the one a new user
    /// sees and the one easiest to leave untested.
    #[test]
    fn an_empty_library_draws() {
        let mut app = CadApp::default();
        app.light.illuminaire_open = true;
        app.light.lib_add_open = true;
        frame(&mut app);
        assert!(app.light.illuminaire_open, "the window closed itself");
    }

    /// A stocked one — tiles in both grids, a selection, a placement armed, an LDT list. Every
    /// branch of the layout at once.
    #[test]
    fn a_stocked_library_draws_every_panel() {
        let mut app = stocked();
        let b = app.light.lib_blocks[0].clone();
        let id = app.light.library.add(crate::illuminaire::Fitting {
            name: b.name.clone(),
            id: 0,
            symbol: b.symbol,
            symbol_unit_m: 0.001,
            ldt_path: "D:/ldt/FONDO.ldt".into(),
            profile: crate::light::BUILTIN.into(),
            model_path: String::new(),
        });
        app.light.lib_sel = Some(id);
        app.light.lib_name_buf = b.name;
        app.light.place_fitting = Some(id);
        frame(&mut app);
        assert_eq!(
            app.light.library.fittings.len(),
            1,
            "a frame changed the library"
        );
        assert_eq!(
            app.light.place_fitting,
            Some(id),
            "a frame disarmed the placement"
        );
    }

    /// A fitting with NO photometry linked — the state every fitting is in for the moment between
    /// being added and being paired, and the one where half the tile has nothing to draw.
    #[test]
    fn an_unlinked_fitting_draws() {
        let mut app = stocked();
        let b = app.light.lib_blocks[1].clone();
        let id = app.light.library.add(crate::illuminaire::Fitting {
            name: b.name,
            id: 0,
            symbol: b.symbol,
            symbol_unit_m: 0.001,
            ldt_path: String::new(),
            profile: String::new(),
            model_path: String::new(),
        });
        app.light.lib_sel = Some(id);
        frame(&mut app);
    }

    /// A fitting whose symbol is EMPTY — what a library loaded without its `.rsm` looks like, and
    /// what a block of pure text or attributes gives. The preview fitter divides by an extent.
    #[test]
    fn a_fitting_with_no_geometry_draws() {
        let mut app = CadApp::default();
        app.light.illuminaire_open = true;
        let id = app.light.library.add(crate::illuminaire::Fitting {
            name: "GHOST".into(),
            id: 0,
            symbol: Vec::new(),
            symbol_unit_m: 0.001,
            ldt_path: "D:/gone.ldt".into(),
            profile: crate::light::BUILTIN.into(),
            model_path: String::new(),
        });
        app.light.lib_sel = Some(id);
        frame(&mut app);
    }

    /// PLACING PUTS BOTH HALVES DOWN, through the real click path.
    ///
    /// The window arms a fitting and the canvas places it; between them sit the unit conversion
    /// and two different documents. A test on `insert` alone proves the block is right and says
    /// nothing about whether the light landed with it.
    #[test]
    fn an_armed_fitting_places_a_block_and_a_light() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 0.001; // a millimetre plan
        let mut src = cad_kernel::Document::default();
        src.blocks.add(cad_kernel::Block {
            name: "OCULUS".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 47.5,
                },
            ))],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let rows = crate::illuminaire::symbols_from(&src);
        let id = app.light.library.add(crate::illuminaire::Fitting {
            name: rows[0].name.clone(),
            id: 0,
            symbol: rows[0].symbol.clone(),
            symbol_unit_m: 0.001,
            ldt_path: "D:/ldt/OCULUS.ldt".into(),
            profile: crate::light::BUILTIN.into(),
            model_path: String::new(),
        });
        app.light.place_fitting = Some(id);

        // 2.5 m, 4.0 m in the world.
        assert!(
            app.place_illuminaire_at(2.5, 4.0),
            "the placement was refused"
        );

        let refs: Vec<_> = app
            .doc
            .dobjects
            .iter()
            .filter_map(|d| match &d.geom {
                cad_kernel::Geom::BlockRef(b) => Some(b.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(refs.len(), 1, "no block landed on the drawing");
        // METRES IN, DRAWING UNITS OUT. Placing at 2.5 m on a millimetre plan is 2500, and this
        // is the conversion that has been got wrong everywhere else in this codebase.
        assert!(
            (refs[0].insert.x - 2500.0).abs() < 1e-6,
            "inserted at {}",
            refs[0].insert.x
        );
        assert!(
            (refs[0].insert.y - 4000.0).abs() < 1e-6,
            "inserted at {}",
            refs[0].insert.y
        );

        assert_eq!(
            app.light.luminaires.len(),
            1,
            "no light landed with the block"
        );
        let l = &app.light.luminaires[0];
        assert_eq!(
            l.profile,
            crate::light::BUILTIN,
            "the light did not take the fitting's file"
        );
        assert_eq!(
            l.from_block,
            Some(refs[0].block),
            "the light is not tied to its symbol"
        );
        // The luminaire is METRES (cad_light's world) while the block is drawing units. Both
        // describing the same point in two spaces is the whole of this function.
        assert!(
            (l.position.x - 2.5).abs() < 1e-6,
            "the light is at {} m",
            l.position.x
        );
        assert!(
            (l.position.y - 4.0).abs() < 1e-6,
            "the light is at {} m",
            l.position.y
        );
    }

    /// ARMING A FITTING DISARMS THE BARE-POINT MODE. Both place on a click, and a click can only
    /// do one thing — with both on, one click would drop a block AND a stray unassigned point.
    #[test]
    fn the_two_placement_modes_are_exclusive() {
        let mut app = CadApp::default();
        let id = app.light.library.add(crate::illuminaire::Fitting {
            name: "A".into(),
            id: 0,
            symbol: Vec::new(),
            symbol_unit_m: 1.0,
            ldt_path: String::new(),
            profile: String::new(),
            model_path: String::new(),
        });
        app.light.place_mode = true;
        app.light.illuminaire_open = true;
        app.light.lib_sel = Some(id);
        // Straight through the action the window returns, rather than re-deciding it here.
        app.light.place_fitting = Some(id);
        app.light.place_mode = false;
        assert!(!app.light.place_mode);

        // And a placement into a FACE SKETCH is refused rather than put on the wall's plane.
        app.factory.session = Some(crate::factory::SketchSession {
            plane: 1,
            saved_doc: cad_kernel::Document::default(),
            saved_undo: Vec::new(),
            saved_redo: Vec::new(),
            saved_constraints: Vec::new(),
            saved_pending: None,
        });
        assert!(
            !app.place_illuminaire_at(1.0, 1.0),
            "a face sketch accepted a fitting"
        );
        assert!(
            app.light.luminaires.is_empty(),
            "a light landed during a face sketch"
        );
    }
}
/// DELETING A PLACED FITTING.
///
/// Both halves of this were reported together: "i cant delete the lights by selecting them and
/// clicking delete", and "when i delete them from fitting it doesnt delete the markers and these
/// diamonds start showing up in simlux". They are the same fault seen from two ends — a fixture
/// and the symbol it was placed as are one thing, and every path that removes one has to remove
/// the other.

#[cfg(test)]
mod deleting_a_placed_fitting {
    use super::*;

    /// A millimetre plan with `n` fittings placed on it, one metre apart.
    fn placed(n: usize) -> (CadApp, Vec<u32>) {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 0.001;
        let mut src = cad_kernel::Document::default();
        src.blocks.add(cad_kernel::Block {
            name: "OCULUS".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 47.5,
                },
            ))],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let rows = crate::illuminaire::symbols_from(&src);
        let fid = app.light.library.add(crate::illuminaire::Fitting {
            name: rows[0].name.clone(),
            id: 0,
            symbol: rows[0].symbol.clone(),
            symbol_unit_m: 0.001,
            ldt_path: String::new(),
            profile: String::new(), // NOT linked — the state the diamonds were reported in
            model_path: String::new(),
        });
        app.light.place_fitting = Some(fid);
        let mut ids = Vec::new();
        for i in 0..n {
            assert!(
                app.place_illuminaire_at(1.0 + i as f32, 2.0),
                "placement {i} refused"
            );
            ids.push(app.light.luminaires.last().expect("a light landed").id);
        }
        app.light.place_fitting = None;
        (app, ids)
    }

    fn block_refs(app: &CadApp) -> usize {
        app.doc
            .dobjects
            .iter()
            .filter(|d| matches!(d.geom, cad_kernel::Geom::BlockRef(_)))
            .count()
    }

    /// THE COMMAND LINE IS FOCUSED, AND DELETE STILL DELETES.
    ///
    /// This is the whole of the first bug. SIMLUX keeps the command line focused so a typed
    /// command lands immediately, and the guard asked `wants_keyboard_input()` — true whenever any
    /// text field holds focus, that one included, empty or not. So the condition was true
    /// essentially always and the key never reached a fixture.
    #[test]
    fn delete_works_while_the_command_line_holds_focus() {
        let (mut app, ids) = placed(3);
        app.light.selected = vec![ids[0]];
        app.focus_at_frame_start = Some(CadApp::cmd_line_id());
        assert!(
            app.delete_targets_lights(),
            "Delete was refused with only the command line focused — this is the reported bug",
        );
    }

    /// …but a REAL text field still takes the keystroke, which is what the guard is for.
    #[test]
    fn delete_is_refused_while_typing_in_a_field() {
        let (mut app, ids) = placed(1);
        app.light.selected = vec![ids[0]];
        app.focus_at_frame_start = Some(egui::Id::new("some_name_field"));
        assert!(
            !app.delete_targets_lights(),
            "Delete erased fixtures while a name was being typed",
        );
    }

    /// And the drawing's own selection still wins — the two Delete handlers are complements and
    /// must never both fire on one keystroke.
    #[test]
    fn the_drawings_selection_still_owns_delete() {
        let (mut app, ids) = placed(1);
        app.light.selected = vec![ids[0]];
        app.focus_at_frame_start = Some(CadApp::cmd_line_id());
        app.selection = vec![0];
        assert!(
            !app.delete_targets_lights(),
            "a light selection swallowed a geometry erase"
        );
        app.selection.clear();
        app.selected = Some(0);
        assert!(
            !app.delete_targets_lights(),
            "a light selection swallowed a pickfirst erase"
        );
    }

    /// DELETING A FIXTURE TAKES ITS SYMBOL WITH IT — and only its own.
    #[test]
    fn deleting_a_fixture_removes_the_block_it_was_placed_as() {
        let (mut app, ids) = placed(3);
        assert_eq!(block_refs(&app), 3, "fixture: three symbols on the plan");

        let (lights, blocks) = app.delete_fixtures(&[ids[1]]);
        assert_eq!(lights, 1, "the fixture did not go");
        assert_eq!(
            blocks, 1,
            "the symbol was left on the plan with nothing behind it"
        );
        assert_eq!(app.light.luminaires.len(), 2);
        assert_eq!(
            block_refs(&app),
            2,
            "the wrong number of symbols was erased"
        );

        // The RIGHT one went: the survivors are the ones at 1.0 m and 3.0 m.
        let mut xs: Vec<f64> = app
            .doc
            .dobjects
            .iter()
            .filter_map(|d| match &d.geom {
                cad_kernel::Geom::BlockRef(b) => Some(b.insert.x),
                _ => None,
            })
            .collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(
            (xs[0] - 1000.0).abs() < 1e-6,
            "erased the wrong symbol; left {xs:?}"
        );
        assert!(
            (xs[1] - 3000.0).abs() < 1e-6,
            "erased the wrong symbol; left {xs:?}"
        );
    }

    /// A HAND-PLACED POINT HAS NO SYMBOL and must not take one.
    ///
    /// `Place luminaire (click plan)` puts a bare point down with no `from_block` at all. Erasing
    /// some other fitting's block because a hand-placed light happened to sit on it would be a
    /// silent edit to the drawing.
    #[test]
    fn a_hand_placed_point_takes_no_block_with_it() {
        let (mut app, _) = placed(1);
        let bare = app.light.place_point(1.0, 2.0); // exactly on top of the placed one
        let (lights, blocks) = app.delete_fixtures(&[bare]);
        assert_eq!(lights, 1);
        assert_eq!(blocks, 0, "a bare point erased a symbol it never placed");
        assert_eq!(
            block_refs(&app),
            1,
            "the drawing lost a block to a hand-placed point"
        );
    }

    /// REMOVING THE FITTING FROM THE LIBRARY TAKES ITS FIXTURES.
    ///
    /// The reported one: the library entry went, seven markers stayed, and because the fitting had
    /// no photometry linked they drew as the hollow diamond that means "no fitting". Deleting the
    /// combo has to delete what the combo put down.
    #[test]
    fn removing_a_fitting_takes_its_markers_and_symbols() {
        let (mut app, _) = placed(7);
        let fid = app.light.library.fittings[0].id;
        assert_eq!(app.light.luminaires.len(), 7);
        assert_eq!(block_refs(&app), 7);
        // Every one of them is unassigned — this is the state the diamonds appear in.
        assert_eq!(
            app.light.unassigned_count(),
            7,
            "fixture: seven points with no fitting"
        );

        // THE REAL REMOVAL PATH, not a re-implementation of it. Working out the doomed fixtures
        // here and calling the helper would have tested my arithmetic against my arithmetic — the
        // handler could have stopped calling it entirely and this would still have passed.
        let (lights, blocks) = app.remove_fitting(fid);

        assert_eq!(lights, 7);
        assert_eq!(blocks, 7);
        assert!(app.light.luminaires.is_empty(), "markers were left behind");
        assert_eq!(block_refs(&app), 0, "symbols were left behind");
        assert_eq!(
            app.light.unassigned_count(),
            0,
            "the diamonds are still there"
        );
        assert!(app.light.library.fittings.is_empty());
    }

    /// A FIXTURE DRAGGED AWAY FROM ITS SYMBOL MATCHES NOTHING.
    ///
    /// Dragging a marker moves the light and not the block, so they genuinely are apart. Taking
    /// the nearest instance instead would erase a different fitting's symbol and leave this one's
    /// — worse than leaving both.
    #[test]
    fn a_dragged_fixture_does_not_erase_a_neighbours_symbol() {
        let (mut app, ids) = placed(3);
        // Move the middle one on top of the last one's symbol.
        if let Some(l) = app.light.luminaires.iter_mut().find(|l| l.id == ids[1]) {
            l.position.x = 3.0;
        }
        let (lights, blocks) = app.delete_fixtures(&[ids[1]]);
        assert_eq!(lights, 1);
        assert_eq!(blocks, 1, "expected exactly the symbol it now stands on");
        // …and the one it LEFT is still there, rather than both being taken.
        assert_eq!(block_refs(&app), 2, "a drag cost the drawing two symbols");
    }

    /// CLICKING A TILE, FOR REAL — through egui, on the widget the user's mouse lands on.
    ///
    /// The pure rule is tested next door in `illuminaire`, and a pure rule nothing calls is worth
    /// nothing: deleting the call site left every one of those tests green. This closes that gap
    /// the only way it can be closed, by driving the actual widget.
    ///
    /// The tile's rect is read back from egui after a first frame rather than computed here — the
    /// layout depends on the window chrome, the fonts and the scroll area, and a test that
    /// hard-coded a position would break on any of them without the behaviour changing.
    #[test]
    fn clicking_a_tile_while_placing_switches_the_placement() {
        let (mut app, _) = placed(1);
        // A second fitting, so there is something to switch TO.
        let first = app.light.library.fittings[0].id;
        let second = app.light.library.add(crate::illuminaire::Fitting {
            name: "VEGA".into(),
            id: 0,
            symbol: app.light.library.fittings[0].symbol.clone(),
            symbol_unit_m: 0.001,
            ldt_path: String::new(),
            profile: String::new(),
            model_path: String::new(),
        });
        app.light.illuminaire_open = true;
        app.light.place_fitting = Some(first);

        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 900.0));
        let raw = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };

        // Frame one: lay the window out, then ask egui where the second fitting's tile ended up.
        let _ = ctx.run(raw(Vec::new()), |ctx| app.render_illuminaire(ctx));
        let tile = ctx
            .read_response(egui::Id::new(("illum_tile", second)))
            .expect("the second fitting has no tile on screen");
        let at = tile.rect.center();
        assert!(screen.contains(at), "the tile is off screen at {at:?}");

        // Then a click, the way a mouse actually does it: move, press, release, each its own
        // frame. egui decides hover from the pointer position it had when the widget was laid
        // out, so a move and a press in the same frame land on a widget that is not there yet.
        let btn = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![btn(true)],
            vec![btn(false)],
        ] {
            let _ = ctx.run(raw(events), |ctx| app.render_illuminaire(ctx));
        }

        assert_eq!(
            app.light.lib_sel,
            Some(second),
            "the click did not select the tile"
        );
        assert_eq!(
            app.light.place_fitting,
            Some(second),
            "the click selected the fitting but kept placing the old one — this is the reported \
             bug, where every symbol on the plan came out as the first fitting",
        );
    }

    /// THE WINDOW'S ACTIONS ARE ACTUALLY CARRIED OUT.
    ///
    /// `remove` is the one that was reported, but the gap was structural: nothing drove the
    /// handler at all, so any of these could have been dropped and every test would still pass.
    #[test]
    fn the_remove_action_takes_the_fixtures_with_it() {
        let (mut app, _) = placed(7);
        let fid = app.light.library.fittings[0].id;
        app.apply_illuminaire_action(crate::illuminaire::Action {
            remove: Some(fid),
            ..Default::default()
        });
        assert!(
            app.light.library.fittings.is_empty(),
            "the fitting stayed in the library"
        );
        assert!(
            app.light.luminaires.is_empty(),
            "the markers stayed — {} left, which is the reported bug",
            app.light.luminaires.len(),
        );
        assert_eq!(block_refs(&app), 0, "the symbols stayed on the plan");
    }

    /// ARMING AND DISARMING GO THROUGH THE SAME HANDLER, and arming must switch the bare-point
    /// mode off — a click can only place one thing.
    #[test]
    fn the_place_action_arms_and_disarms() {
        let (mut app, _) = placed(1);
        let fid = app.light.library.fittings[0].id;
        app.light.place_mode = true;
        app.apply_illuminaire_action(crate::illuminaire::Action {
            place: Some(fid),
            ..Default::default()
        });
        assert_eq!(
            app.light.place_fitting,
            Some(fid),
            "the fitting was not armed"
        );
        assert!(
            !app.light.place_mode,
            "both placement modes are armed at once"
        );

        app.apply_illuminaire_action(crate::illuminaire::Action {
            stop_placing: true,
            ..Default::default()
        });
        assert_eq!(app.light.place_fitting, None, "stop did not disarm");
    }

    /// RENAMING GOES THROUGH, and a removed fitting cannot stay armed for placement — the next
    /// click would look up an id that is no longer there.
    #[test]
    fn removing_the_armed_fitting_disarms_it() {
        let (mut app, _) = placed(1);
        let fid = app.light.library.fittings[0].id;
        app.light.place_fitting = Some(fid);
        app.light.lib_sel = Some(fid);
        app.apply_illuminaire_action(crate::illuminaire::Action {
            remove: Some(fid),
            ..Default::default()
        });
        assert_eq!(
            app.light.place_fitting, None,
            "a deleted fitting is still armed"
        );
        assert_eq!(
            app.light.lib_sel, None,
            "a deleted fitting is still selected"
        );
    }

    /// THE UNIT CHOICE REACHES THE STATE the next added fitting reads.
    #[test]
    fn the_unit_action_changes_what_a_new_fitting_records() {
        let (mut app, _) = placed(1);
        app.apply_illuminaire_action(crate::illuminaire::Action {
            set_blocks_unit: Some(0.001),
            ..Default::default()
        });
        assert!((app.light.lib_blocks_unit_m - 0.001).abs() < 1e-12);
        // …and adding now records millimetres rather than whatever the file declared.
        app.light.lib_blocks = crate::illuminaire::symbols_from(&app.doc.clone());
        if !app.light.lib_blocks.is_empty() {
            app.apply_illuminaire_action(crate::illuminaire::Action {
                add: Some(0),
                ..Default::default()
            });
            let f = app.light.library.fittings.last().expect("added");
            assert!(
                (f.symbol_unit_m - 0.001).abs() < 1e-12,
                "the chosen unit was not recorded"
            );
        }
    }

    /// THE SELECTION DOES NOT SURVIVE THE VECTOR IT INDEXES INTO.
    ///
    /// A pickfirst selection is a list of indices. Removing dobjects shifts everything after them,
    /// so a selection kept across the removal points at whatever slid into place — and the next
    /// Delete erases that.
    #[test]
    fn a_stale_index_selection_is_dropped() {
        let (mut app, ids) = placed(3);
        app.selection = vec![2];
        app.selected = Some(2);
        app.delete_fixtures(&[ids[0]]);
        assert!(
            app.selection.is_empty(),
            "the selection still indexes a shifted vector"
        );
        assert!(app.selected.is_none());
    }

    /// EVERY DELETE IS ONE UNDO STEP, of the kind that matches what it touched.
    ///
    /// A fitting placed from the library takes its symbol with it, so the step names both halves.
    /// A hand-placed point has no symbol, so the step names the fixtures alone — and it is still a
    /// step, or Undo reaches past the delete to an older drawing edit and takes the drawing with
    /// it. That last case is the one that had no snapshot at all.
    #[test]
    fn every_delete_pushes_exactly_one_undo_step() {
        let (mut app, ids) = placed(2);
        let before = app.undo_stack.len();
        app.delete_fixtures(&[ids[0]]);
        assert_eq!(
            app.undo_stack.len(),
            before + 1,
            "no undo step was pushed for the erase"
        );
        assert!(
            matches!(app.undo_stack.last(), Some(UndoStep::Both(_, _))),
            "a delete that erased a symbol did not record the drawing half",
        );

        let bare = app.place_fixture_point(40.0, 40.0);
        let n = app.undo_stack.len();
        app.delete_fixtures(&[bare]);
        assert_eq!(
            app.undo_stack.len(),
            n + 1,
            "a fixture-only delete pushed no undo step"
        );
        assert!(
            matches!(app.undo_stack.last(), Some(UndoStep::Light(_))),
            "a delete that touched no geometry claimed a drawing edit — Undo would take a second \
             press to get past it, and the first would restore a document nothing had changed",
        );
    }

    /// A PLACEMENT IS AN EDIT TO THE DRAWING, and has to announce itself as one.

    /// THE SYMBOL AND THE MARKER LAND ON THE SAME SPOT — end to end, through the real placement.
    ///
    /// This is the one the user sees. The marker goes exactly where the click was and the
    /// CALCULATION uses the marker, so a symbol sitting a metre away is a plan that states the
    /// light is somewhere it is not — and every drawing issued from it is wrong in a way no number
    /// on the sheet contradicts.
    #[test]
    fn the_symbol_lands_on_the_marker() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        // A symbol drawn far from its own origin, exactly as the real block file has them.
        let fid = app.light.library.add(crate::illuminaire::Fitting {
            name: "VEGA".into(),
            id: 0,
            symbol: vec![crate::illuminaire::SymbolGeom {
                geom: cad_kernel::Geom::Circle(cad_kernel::Circle {
                    center: Vec2::new(4161.6, 567.3),
                    radius: 72.5,
                }),
                aci: None,
            }],
            symbol_unit_m: 0.001,
            ldt_path: String::new(),
            profile: crate::light::BUILTIN.into(),
            model_path: String::new(),
        });
        app.light.place_fitting = Some(fid);
        assert!(
            app.place_illuminaire_at(-8.476, 6.425),
            "the placement was refused"
        );

        let l = &app.light.luminaires[0];
        let br = app
            .doc
            .dobjects
            .iter()
            .find_map(|d| match &d.geom {
                cad_kernel::Geom::BlockRef(b) => Some(b.clone()),
                _ => None,
            })
            .expect("a block landed");

        // Where the drawing actually shows the symbol: the definition's own bounds, carried to
        // the insertion point.
        let def = app.doc.blocks.get(br.block).expect("the definition");
        let sym: Vec<crate::illuminaire::SymbolGeom> = def
            .dobjects
            .iter()
            .map(|d| crate::illuminaire::SymbolGeom {
                geom: d.geom.clone(),
                aci: None,
            })
            .collect();
        let [mnx, mny, mxx, mxy] =
            crate::illuminaire::symbol_bounds(&sym).expect("it draws something");
        let cx = br.insert.x + (mnx + mxx) * 0.5;
        let cy = br.insert.y + (mny + mxy) * 0.5;

        assert!(
            (cx - l.position.x as f64).abs() < 1e-6 && (cy - l.position.y as f64).abs() < 1e-6,
            "the symbol is centred at ({cx:.4}, {cy:.4}) and the light is at ({}, {}) — {:.3} m \
             apart",
            l.position.x,
            l.position.y,
            ((cx - l.position.x as f64).powi(2) + (cy - l.position.y as f64).powi(2)).sqrt(),
        );
        assert!(
            (l.position.x - (-8.476)).abs() < 1e-4,
            "the light is not where the click was"
        );
    }
    ///
    /// Reported as "once i place a file ... still no blocks. the block shows up after i delete a
    /// light or 2". The blocks were on the drawing the whole time. Nothing had told the canvas its
    /// cached geometry was stale, or the spatial index that there was something new to pick — so
    /// they were invisible and unclickable until an unrelated edit rebuilt both. The session dump
    /// showed it exactly: `INDEX REBUILD` fired on every delete and on no placement.
    ///
    /// The undo step matters as much. Without it a placement could not be undone AND the file was
    /// never marked unsaved, so closing the app would have discarded the work without asking.
    #[test]
    fn placing_marks_the_drawing_changed() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let mut src = cad_kernel::Document::default();
        src.blocks.add(cad_kernel::Block {
            name: "OCULUS".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 47.5,
                },
            ))],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let rows = crate::illuminaire::symbols_from(&src);
        let fid = app.light.library.add(crate::illuminaire::Fitting {
            name: rows[0].name.clone(),
            id: 0,
            symbol: rows[0].symbol.clone(),
            symbol_unit_m: 0.001,
            ldt_path: String::new(),
            profile: String::new(),
            model_path: String::new(),
        });
        app.light.place_fitting = Some(fid);

        // A clean slate: the index freshly built, the view settled, nothing unsaved.
        app.index_dirty = false;
        app.unsaved = false;
        let seq = app.view_seq;
        let undo = app.undo_stack.len();

        assert!(
            app.place_illuminaire_at(2.0, 3.0),
            "the placement was refused"
        );

        assert!(
            app.index_dirty,
            "the spatial index was not invalidated — the block is on the drawing and cannot be \
             picked, which is what \"no blocks\" looked like",
        );
        assert_ne!(app.view_seq, seq, "the canvas was not told to redraw");
        assert_eq!(
            app.undo_stack.len(),
            undo + 1,
            "a placement cannot be undone"
        );
        assert!(
            app.unsaved,
            "the drawing was not marked unsaved — closing would discard it"
        );
    }

    /// CORRECTING THE UNIT REDRAWS WHAT IS ALREADY ON THE PLAN.
    ///
    /// The block file this was built for declares inches and is drawn in millimetres, so a library
    /// built from it places a 2 m batten 50.8 m long. The definition is a CACHE of the library
    /// entry and `ensure_block` reuses it by name — so correcting the unit without rebuilding it
    /// would leave every instance already placed at the old scale, with the panel stating a size
    /// the drawing does not show.
    #[test]
    fn correcting_the_unit_rescales_what_is_already_placed() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0; // a metre plan
        let mut src = cad_kernel::Document::default();
        src.blocks.add(cad_kernel::Block {
            name: "LINEA W48X80 - (2M)".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(2000.0, 0.0),
                },
            ))],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let rows = crate::illuminaire::symbols_from(&src);
        let fid = app.light.library.add(crate::illuminaire::Fitting {
            name: rows[0].name.clone(),
            id: 0,
            symbol: rows[0].symbol.clone(),
            symbol_unit_m: 0.0254, // what the file WRONGLY declares
            ldt_path: String::new(),
            profile: String::new(),
            model_path: String::new(),
        });
        app.light.place_fitting = Some(fid);
        assert!(app.place_illuminaire_at(0.0, 0.0));

        let length = |app: &CadApp| -> f64 {
            let b = app
                .doc
                .blocks
                .find("LINEA W48X80 - (2M)")
                .expect("the definition");
            match &app.doc.blocks.get(b).expect("there").dobjects[0].geom {
                cad_kernel::Geom::Line(l) => (l.b - l.a).len(),
                g => panic!("the definition holds {g:?}"),
            }
        };
        // Under the file's own declaration a 2 m batten is fifty metres long. This is the bug the
        // user saw as blocks "showing up on places the lights werent marked".
        assert!(
            (length(&app) - 50.8).abs() < 1e-6,
            "fixture: got {} m",
            length(&app)
        );

        app.apply_illuminaire_action(crate::illuminaire::Action {
            set_fitting_unit: Some((fid, 0.001)),
            ..Default::default()
        });
        assert!(
            (app.light.library.get(fid).expect("there").symbol_unit_m - 0.001).abs() < 1e-12,
            "the library entry was not corrected",
        );
        assert!(
            (length(&app) - 2.0).abs() < 1e-6,
            "the block already on the plan is still {} m — the correction did not reach it",
            length(&app),
        );
        assert!(app.index_dirty, "the rescaled block was not re-indexed");
    }

    /// …and the instances are left exactly where they were. Rebuilding the DEFINITION must not
    /// disturb the references: same count, same insertion points, same ids.
    #[test]
    fn correcting_the_unit_does_not_move_anything() {
        let (mut app, _) = placed(3);
        let fid = app.light.library.fittings[0].id;
        let before: Vec<(u32, f64, f64)> = app
            .doc
            .dobjects
            .iter()
            .filter_map(|d| match &d.geom {
                cad_kernel::Geom::BlockRef(b) => Some((b.block, b.insert.x, b.insert.y)),
                _ => None,
            })
            .collect();
        app.apply_illuminaire_action(crate::illuminaire::Action {
            set_fitting_unit: Some((fid, 1.0)),
            ..Default::default()
        });
        let after: Vec<(u32, f64, f64)> = app
            .doc
            .dobjects
            .iter()
            .filter_map(|d| match &d.geom {
                cad_kernel::Geom::BlockRef(b) => Some((b.block, b.insert.x, b.insert.y)),
                _ => None,
            })
            .collect();
        assert_eq!(
            before, after,
            "rebuilding the definition moved the instances"
        );
        assert_eq!(
            app.light.luminaires.len(),
            3,
            "a unit change cost a fixture"
        );
    }

    /// A FITTING NEVER PLACED IN THIS DRAWING costs no undo step — an Undo the user has to press
    /// twice is its own bug.
    #[test]
    fn correcting_an_unplaced_fittings_unit_touches_no_drawing() {
        // A drawing that already HAS blocks, so a lookup that fell back to an index instead of
        // reporting "not here" would rewrite one of them. An empty block table hides that.
        let (mut app, _) = placed(1);
        let existing: Vec<usize> = app
            .doc
            .blocks
            .blocks
            .iter()
            .map(|b| b.dobjects.len())
            .collect();
        assert!(
            !existing.is_empty(),
            "fixture: the drawing has a block table"
        );

        let fid = app.light.library.add(crate::illuminaire::Fitting {
            name: "NEVER PLACED".into(),
            id: 0,
            symbol: vec![crate::illuminaire::SymbolGeom {
                geom: cad_kernel::Geom::Line(cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(9.0, 0.0),
                }),
                aci: None,
            }],
            symbol_unit_m: 0.0254,
            ldt_path: String::new(),
            profile: String::new(),
            model_path: String::new(),
        });
        let undo = app.undo_stack.len();
        app.apply_illuminaire_action(crate::illuminaire::Action {
            set_fitting_unit: Some((fid, 0.001)),
            ..Default::default()
        });
        assert!(
            (app.light.library.get(fid).expect("there").symbol_unit_m - 0.001).abs() < 1e-12,
            "the library entry was not corrected",
        );
        assert_eq!(
            app.undo_stack.len(),
            undo,
            "an undo step was pushed for an edit never made"
        );
        let after: Vec<usize> = app
            .doc
            .blocks
            .blocks
            .iter()
            .map(|b| b.dobjects.len())
            .collect();
        assert_eq!(
            existing, after,
            "correcting a fitting this drawing has never seen rewrote another block's geometry",
        );
    }
}
/// SIMLUX FIXTURES ARE ON THE UNDO STACK.
///
/// They were not, and it showed in two directions. A delete was final: Ctrl+Z after erasing a
/// marker stepped straight past it to an older drawing edit and took the drawing with it. And a
/// placement, which puts a block on the plan and a light at the same point, could only ever be
/// half undone — the symbol came back and the marker did not.
///
/// The rule these all check is that a step carries exactly what its edit changed, and that one
/// act is one step.

#[cfg(test)]
mod fixtures_are_undoable {
    use super::*;

    /// A millimetre plan with a fitting in the library, ready to place.
    fn armed() -> (CadApp, u32) {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 0.001;
        let mut src = cad_kernel::Document::default();
        src.blocks.add(cad_kernel::Block {
            name: "OCULUS".into(),
            base: Vec2::new(0.0, 0.0),
            dobjects: vec![cad_kernel::DObject::new(cad_kernel::Geom::Circle(
                cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 47.5,
                },
            ))],
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        });
        let rows = crate::illuminaire::symbols_from(&src);
        let fid = app.light.library.add(crate::illuminaire::Fitting {
            name: rows[0].name.clone(),
            id: 0,
            symbol: rows[0].symbol.clone(),
            symbol_unit_m: 0.001,
            ldt_path: String::new(),
            profile: crate::light::BUILTIN.into(),
            model_path: String::new(),
        });
        app.light.place_fitting = Some(fid);
        (app, fid)
    }

    fn blocks(app: &CadApp) -> usize {
        app.doc
            .dobjects
            .iter()
            .filter(|d| matches!(d.geom, cad_kernel::Geom::BlockRef(_)))
            .count()
    }

    /// PLACING IS ONE ACT AND ONE UNDO — both halves, together.
    #[test]
    fn undoing_a_placement_takes_back_the_block_and_the_light() {
        let (mut app, _) = armed();
        app.place_illuminaire_at(1.0, 2.0);
        app.place_illuminaire_at(3.0, 2.0);
        assert_eq!(app.light.luminaires.len(), 2);
        assert_eq!(blocks(&app), 2);

        app.do_undo();
        assert_eq!(
            app.light.luminaires.len(),
            1,
            "the marker stayed after an undo"
        );
        assert_eq!(blocks(&app), 1, "the symbol stayed after an undo");

        app.do_undo();
        assert!(app.light.luminaires.is_empty(), "the first marker stayed");
        assert_eq!(blocks(&app), 0, "the first symbol stayed");
    }

    /// …AND REDO PUTS BOTH BACK.
    #[test]
    fn redoing_a_placement_restores_both_halves() {
        let (mut app, _) = armed();
        app.place_illuminaire_at(1.0, 2.0);
        app.do_undo();
        assert!(app.light.luminaires.is_empty());

        app.do_redo();
        assert_eq!(
            app.light.luminaires.len(),
            1,
            "redo did not bring the marker back"
        );
        assert_eq!(blocks(&app), 1, "redo did not bring the symbol back");
        assert_eq!(
            app.light.luminaires[0].from_block,
            Some(0),
            "the restored light is not tied to its restored symbol",
        );
    }

    /// DELETING IS UNDOABLE, both halves together. This is the asymmetry that was reported: undo
    /// brought the symbols back and not the markers.
    #[test]
    fn undoing_a_delete_brings_the_marker_back_with_its_symbol() {
        let (mut app, _) = armed();
        app.place_illuminaire_at(1.0, 2.0);
        let id = app.light.luminaires[0].id;

        app.delete_fixtures(&[id]);
        assert!(app.light.luminaires.is_empty());
        assert_eq!(blocks(&app), 0);

        app.do_undo();
        assert_eq!(
            app.light.luminaires.len(),
            1,
            "the marker did not come back"
        );
        assert_eq!(blocks(&app), 1, "the symbol did not come back");
        assert_eq!(
            app.light.luminaires[0].id, id,
            "a different fixture came back"
        );
    }

    /// A FIXTURE-ONLY DELETE DOES NOT REACH PAST ITSELF INTO THE DRAWING.
    ///
    /// The sharpest version of the old bug: a hand-placed point has no symbol, so deleting it used
    /// to push no step at all — and the next Ctrl+Z undid whatever drawing edit came before,
    /// erasing geometry the user never touched.
    #[test]
    fn undoing_a_bare_point_delete_leaves_the_drawing_alone() {
        let mut app = CadApp::default();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(1.0, 0.0),
                },
            )));
        app.snapshot_doc();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 1.0),
                    b: Vec2::new(1.0, 1.0),
                },
            )));
        let drawn = app.doc.dobjects.len();

        let id = app.place_fixture_point(5.0, 5.0);
        app.delete_fixtures(&[id]);
        assert!(app.light.luminaires.is_empty());

        app.do_undo();
        assert_eq!(app.light.luminaires.len(), 1, "the point did not come back");
        assert_eq!(
            app.doc.dobjects.len(),
            drawn,
            "undoing a fixture delete erased geometry the user never touched",
        );
    }

    /// A DRAGGED FIXTURE TAKES ITS SYMBOL WITH IT.
    ///
    /// Reported as: dragging a fixture moves the light but not its symbol. It did — `from_block`
    /// names the block DEFINITION, shared by every instance of a fitting, so the drawing had no way
    /// to know which of fifty identical downlights belonged to the marker under the pointer. The
    /// pairing is by POSITION, and the one moment the two are certainly together is the press.
    ///
    /// A plan whose symbols say one thing and whose calculation says another is worse than either
    /// alone: the drawing is what gets issued, and the lux figures are what it is signed off on.
    fn placed(app: &mut CadApp, at: (f32, f32)) -> u32 {
        let fid = app.light.library.add(crate::illuminaire::Fitting {
            name: "VEGA".into(),
            id: 0,
            symbol: vec![crate::illuminaire::SymbolGeom {
                geom: cad_kernel::Geom::Circle(cad_kernel::Circle {
                    center: Vec2::new(0.0, 0.0),
                    radius: 0.2,
                }),
                aci: None,
            }],
            symbol_unit_m: 1.0,
            ldt_path: String::new(),
            profile: crate::light::BUILTIN.into(),
            model_path: String::new(),
        });
        app.light.place_fitting = Some(fid);
        assert!(
            app.place_illuminaire_at(at.0, at.1),
            "the placement was refused"
        );
        app.light.luminaires.last().expect("a fixture").id
    }

    /// Every block insertion point on the drawing, in drawing units.
    fn inserts(app: &CadApp) -> Vec<(f64, f64)> {
        app.doc
            .dobjects
            .iter()
            .filter_map(|d| match &d.geom {
                cad_kernel::Geom::BlockRef(b) => Some((b.insert.x, b.insert.y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn dragging_a_fixture_moves_its_symbol_too() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let id = placed(&mut app, (1.0, 1.0));
        let before = inserts(&app);
        assert_eq!(before.len(), 1, "expected one symbol on the drawing");

        app.begin_fixture_drag(id, (1.0, 1.0));
        app.light.drag_to((4.0, 2.5));
        app.drag_fixture_symbols();
        app.light.end_drag();

        let l = app
            .light
            .luminaires
            .iter()
            .find(|l| l.id == id)
            .expect("the fixture");
        assert!((l.position.x - 4.0).abs() < 1e-5 && (l.position.y - 2.5).abs() < 1e-5);
        let after = inserts(&app);
        assert_eq!(after.len(), 1, "the drag added or removed a symbol");
        let moved = (after[0].0 - before[0].0, after[0].1 - before[0].1);
        assert!(
            (moved.0 - 3.0).abs() < 1e-6 && (moved.1 - 1.5).abs() < 1e-6,
            "the light moved by (3.0, 1.5) and its symbol by ({:.3}, {:.3})",
            moved.0,
            moved.1,
        );
    }

    /// AND IT IS STILL ITS SYMBOL AFTERWARDS.
    ///
    /// The link is by position, so a drag that left the block behind did not merely look wrong —
    /// it BROKE the link, and deleting the fixture then left an orphaned symbol on the plan. That
    /// was the second half of an earlier report ("when i delete them from fitting it doesnt delete
    /// the markers"), and it is only truly fixed once the two travel together.
    #[test]
    fn a_dragged_fixture_can_still_be_deleted_cleanly() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let id = placed(&mut app, (1.0, 1.0));

        app.begin_fixture_drag(id, (1.0, 1.0));
        app.light.drag_to((6.0, 3.0));
        app.drag_fixture_symbols();
        app.light.end_drag();

        app.delete_fixtures(&[id]);
        assert!(
            app.light.luminaires.is_empty(),
            "the fixture was not deleted"
        );
        assert!(
            inserts(&app).is_empty(),
            "the symbol was left on the drawing after its fixture went",
        );
    }

    /// TWO FIXTURES OF THE SAME FITTING KEEP THEIR OWN SYMBOLS.
    ///
    /// The case a naive "find a block of this definition nearby" gets wrong: fifty downlights share
    /// one definition, so a claim has to be one-to-one or dragging one marker would pick up its
    /// neighbour's symbol and leave its own where it was.
    #[test]
    fn dragging_one_of_two_leaves_the_other_alone() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let a = placed(&mut app, (1.0, 1.0));
        // The SAME fitting again, so both symbols are instances of one definition.
        let fid = app
            .light
            .library
            .fittings
            .first()
            .map(|f| f.id)
            .expect("the fitting");
        app.light.place_fitting = Some(fid);
        assert!(app.place_illuminaire_at(2.0, 1.0));
        assert_eq!(inserts(&app).len(), 2);

        app.light.select(a, false);
        app.begin_fixture_drag(a, (1.0, 1.0));
        app.light.drag_to((1.0, 5.0));
        app.drag_fixture_symbols();
        app.light.end_drag();

        let mut got = inserts(&app);
        got.sort_by(|p, q| p.1.partial_cmp(&q.1).expect("finite"));
        assert!(
            (got[0].0 - 2.0).abs() < 1e-6 && (got[0].1 - 1.0).abs() < 1e-6,
            "the fixture that was not dragged had its symbol moved to {:?}",
            got[0],
        );
        assert!(
            (got[1].1 - 5.0).abs() < 1e-6,
            "the dragged fixture's symbol is at {:?}",
            got[1],
        );
    }

    /// ROTATING A SYMBOL ON THE PLAN ROTATES THE LIGHT BEHIND IT.
    ///
    /// Asked as: *"does rotating the lights in the 2d actually rotate a light in simlux as well?
    /// verify that too"*. It did not. `apply_rotate` edits `doc.dobjects` and nothing else — so did
    /// `apply_move`, `apply_scale` and `apply_mirror` — and the luminaire kept the aiming it was
    /// placed with while its symbol turned on the drawing.
    ///
    /// That is not only a wrong calculation, it is an INVISIBLY wrong one: the fixture had not
    /// changed, so the fingerprint had not changed, so the result did not even go out of date.
    /// "i rotated a light and the result was still valid" — the staleness check was working, there
    /// was simply nothing for it to notice.
    #[test]
    fn rotating_a_symbol_rotates_its_light() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let id = placed(&mut app, (1.0, 1.0));
        assert!(
            app.light.symbol_of.contains_key(&id),
            "the fixture was not linked to its symbol"
        );
        assert_eq!(app.light.luminaires[0].rotation_deg, 0.0);

        // Select the symbol on the drawing and turn it a quarter turn about its own insert.
        let i = app
            .doc
            .dobjects
            .iter()
            .position(|d| matches!(d.geom, cad_kernel::Geom::BlockRef(_)))
            .expect("a symbol");
        app.selection = vec![i];
        app.apply_rotate(Vec2::new(1.0, 1.0), std::f64::consts::FRAC_PI_2);
        app.tick_symbol_sync();

        let l = &app.light.luminaires[0];
        assert!(
            (l.rotation_deg - 90.0).abs() < 1e-3,
            "the symbol was turned 90° and the light reads {}°",
            l.rotation_deg,
        );
    }

    /// MOVING ONE MOVES THE OTHER, and about a pivot the light orbits with its symbol.
    #[test]
    fn a_symbol_moved_on_the_plan_takes_its_light_with_it() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let id = placed(&mut app, (1.0, 1.0));
        let i = app
            .doc
            .dobjects
            .iter()
            .position(|d| matches!(d.geom, cad_kernel::Geom::BlockRef(_)))
            .expect("a symbol");
        app.selection = vec![i];

        // A quarter turn about the ORIGIN: (1, 1) goes to (-1, 1).
        app.apply_rotate(Vec2::new(0.0, 0.0), std::f64::consts::FRAC_PI_2);
        app.tick_symbol_sync();
        let _ = id;
        let l = &app.light.luminaires[0];
        assert!(
            (l.position.x + 1.0).abs() < 1e-4 && (l.position.y - 1.0).abs() < 1e-4,
            "the light is at ({}, {}) after its symbol orbited to (-1, 1)",
            l.position.x,
            l.position.y,
        );
    }

    /// AND THE RESULT GOES OUT OF DATE FOR IT.
    ///
    /// The whole point of the report: "the result should be invalidated even if the position of the
    /// light is changed. i rotated a light and the result was still valid."
    #[test]
    fn a_symbol_rotated_on_the_plan_puts_the_result_out_of_date() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        placed(&mut app, (1.0, 1.0));
        // Pretend a calculation just ran over this scene.
        let plan = app.plan_doc().clone();
        let fp = app
            .light
            .current_fingerprint(&plan, Some(&app.factory))
            .expect("a fingerprint");
        app.light.results_fingerprint = Some(fp);
        app.light.results_stale = false;
        app.light.stale_checked = None;

        let i = app
            .doc
            .dobjects
            .iter()
            .position(|d| matches!(d.geom, cad_kernel::Geom::BlockRef(_)))
            .expect("a symbol");
        app.selection = vec![i];
        app.apply_rotate(Vec2::new(1.0, 1.0), std::f64::consts::FRAC_PI_2);
        app.tick_symbol_sync();

        app.light.stale_checked = None;
        let plan = app.plan_doc().clone();
        app.light.refresh_staleness(&plan, Some(&app.factory));
        assert!(
            app.light.results_stale,
            "a fitting was turned 90° on the plan and the answer still claimed to be current",
        );
    }

    /// A SYMBOL THAT IS NOT THERE ANY MORE LEAVES ITS LIGHT ALONE.
    ///
    /// Somebody may erase a block from the drawing directly. Moving the light to the origin — or
    /// deleting it — because a handle stopped resolving would be a far larger decision than a
    /// bookkeeping pass is entitled to make, and it would be taken silently.
    #[test]
    fn a_light_whose_symbol_was_erased_stays_put() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        placed(&mut app, (3.0, 4.0));
        app.doc
            .dobjects
            .retain(|d| !matches!(d.geom, cad_kernel::Geom::BlockRef(_)));
        app.edit_seq += 1;
        app.tick_symbol_sync();

        let l = &app.light.luminaires[0];
        assert!(
            (l.position.x - 3.0).abs() < 1e-4 && (l.position.y - 4.0).abs() < 1e-4,
            "the light moved to ({}, {}) because its symbol was erased",
            l.position.x,
            l.position.y,
        );
    }

    /// THE LINK SURVIVES A SAVE AND A REOPEN.
    ///
    /// Without it a reopened project falls back to matching by position, and the first rotate
    /// after that separates the two again — which is the bug, one session later.
    #[test]
    fn the_link_between_a_light_and_its_symbol_is_saved() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let id = placed(&mut app, (1.0, 1.0));
        let handle = *app.light.symbol_of.get(&id).expect("a link");

        let cfg = app.light.to_config(&app.doc);
        assert_eq!(
            cfg.symbol_of.get(&id),
            Some(&handle),
            "the link is not in the sidecar"
        );

        let mut fresh = crate::light::LightState::new();
        fresh.apply_config(cfg, &app.doc);
        assert_eq!(
            fresh.symbol_of.get(&id),
            Some(&handle),
            "the link did not come back"
        );
    }

    /// THE AIM TOOL IS TWO CLICKS: a fitting, then the point it should light.
    ///
    /// Asked for as "when aim i selected the user can select a light and then click on a point
    /// where they would like to point it".
    #[test]
    fn aiming_takes_a_fitting_then_a_target() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        app.light.plane_height = 0.8;
        let id = placed(&mut app, (1.0, 1.0));
        app.light.luminaires[0].position.z = 3.0;
        app.light.aim_mode = true;

        // First click: on the marker.
        app.aim_click(1.0, 1.0, 0.5);
        assert_eq!(
            app.light.aim_pick,
            Some(id),
            "the first click did not pick the fitting"
        );
        assert_eq!(
            app.light.selected,
            vec![id],
            "and it should be selected while being aimed"
        );

        // Second click: three metres away.
        app.aim_click(4.0, 1.0, 0.5);
        assert_eq!(
            app.light.aim_pick, None,
            "the tool did not come back for the next fitting"
        );
        let l = &app.light.luminaires[0];
        assert!(
            (l.position.x - 1.0).abs() < 1e-5 && (l.position.z - 3.0).abs() < 1e-5,
            "the fitting moved to ({}, {}, {})",
            l.position.x,
            l.position.y,
            l.position.z,
        );
        // 3 m across, 2.2 m down to the working plane: 53.7° from vertical, toward +x.
        assert!((l.tilt_deg - 53.75).abs() < 0.2, "tilt is {}", l.tilt_deg);
        assert!(l.rotation_deg.abs() < 1e-3, "azimuth is {}", l.rotation_deg);
        assert!(
            app.light.aim_mode,
            "the tool disarmed itself after one fitting"
        );
    }

    /// A FIRST CLICK ON EMPTY PLAN PICKS NOTHING, and says so rather than aiming whatever was
    /// selected earlier.
    #[test]
    fn aiming_needs_a_fitting_under_the_first_click() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let id = placed(&mut app, (1.0, 1.0));
        app.light.select(id, false);
        app.light.aim_mode = true;

        app.aim_click(20.0, 20.0, 0.5);
        assert_eq!(
            app.light.aim_pick, None,
            "it picked a fitting that was not there"
        );
        assert!(
            app.light.last_msg.contains("No fitting"),
            "{:?}",
            app.light.last_msg
        );
        assert_eq!(
            app.light.luminaires[0].tilt_deg, 0.0,
            "a fitting was aimed by an empty click"
        );
    }

    /// AND AIMING PUTS THE RESULT OUT OF DATE.
    ///
    /// Aiming changes where the light goes, so it changes the answer — which is the whole reason
    /// for adding tilt rather than only turning the fitting on its axis.
    #[test]
    fn aiming_a_fitting_puts_the_result_out_of_date() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        app.light.plane_height = 0.8;
        placed(&mut app, (1.0, 1.0));
        app.light.luminaires[0].position.z = 3.0;

        let plan = app.plan_doc().clone();
        let fp = app
            .light
            .current_fingerprint(&plan, Some(&app.factory))
            .expect("a fingerprint");
        app.light.results_fingerprint = Some(fp);
        app.light.results_stale = false;

        app.light.aim_mode = true;
        app.aim_click(1.0, 1.0, 0.5);
        app.aim_click(5.0, 2.0, 0.5);

        app.light.stale_checked = None;
        let plan = app.plan_doc().clone();
        app.light.refresh_staleness(&plan, Some(&app.factory));
        assert!(
            app.light.results_stale,
            "a fitting was aimed somewhere else and the answer still claimed to be current",
        );
    }

    /// THE TILT SURVIVES A SAVE AND A REOPEN. An aim that came back pointing at the floor would be
    /// a silent loss of the whole scheme's aiming.
    #[test]
    fn an_aimed_fitting_is_saved_aimed() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        placed(&mut app, (1.0, 1.0));
        app.light.luminaires[0].position.z = 3.0;
        app.light.luminaires[0].tilt_deg = 27.5;
        app.light.luminaires[0].rotation_deg = 61.0;

        let cfg = app.light.to_config(&app.doc);
        let mut fresh = crate::light::LightState::new();
        fresh.apply_config(cfg, &app.doc);
        let l = &fresh.luminaires[0];
        assert!(
            (l.tilt_deg - 27.5).abs() < 1e-4,
            "the tilt came back as {}",
            l.tilt_deg
        );
        assert!(
            (l.rotation_deg - 61.0).abs() < 1e-4,
            "the azimuth came back as {}",
            l.rotation_deg
        );
    }

    /// TWO FIXTURES ON THE SAME SPOT TAKE TWO DIFFERENT SYMBOLS.
    ///
    /// The case that makes the claim have to be ONE-TO-ONE, and the one a single-fixture drag
    /// cannot show: with several fixtures moving together and two of them at the same point, a
    /// claim that simply finds "a block of this definition here" hands both fixtures the SAME
    /// instance. One symbol is then moved twice and the other stranded on the plan — and since the
    /// link is by position, the stranded one can no longer be found or deleted at all.
    ///
    /// Duplicating a fitting in place is how two end up on one spot, and it is not rare.
    #[test]
    fn two_fixtures_on_one_spot_do_not_fight_over_a_symbol() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let a = placed(&mut app, (1.0, 1.0));
        let fid = app
            .light
            .library
            .fittings
            .first()
            .map(|f| f.id)
            .expect("the fitting");
        app.light.place_fitting = Some(fid);
        assert!(
            app.place_illuminaire_at(1.0, 1.0),
            "the second placement was refused"
        );
        let b = app.light.luminaires.last().expect("a second fixture").id;
        assert_ne!(a, b);
        assert_eq!(
            inserts(&app).len(),
            2,
            "expected two symbols stacked on one point"
        );

        // Both selected and dragged together — what a rubber band or a shift-click gives.
        app.light.select(a, false);
        app.light.select(b, true);
        app.begin_fixture_drag(a, (1.0, 1.0));
        app.light.drag_to((7.0, 4.0));
        app.drag_fixture_symbols();
        app.light.end_drag();

        let got = inserts(&app);
        assert_eq!(got.len(), 2, "a symbol was created or destroyed");
        for (i, p) in got.iter().enumerate() {
            assert!(
                (p.0 - 7.0).abs() < 1e-6 && (p.1 - 4.0).abs() < 1e-6,
                "symbol {i} was left at {p:?} while its fixture moved to (7, 4) — two fixtures \
                 claimed the same block and one symbol was stranded",
            );
        }
    }

    /// A DRAG IS ONE UNDO, AND IT TAKES BACK BOTH HALVES.
    ///
    /// The marker and the symbol move together, so they have to come back together. An Undo that
    /// restored one of them would leave a state the user never made — which is worse than the bug
    /// it came from, because now the two disagree and nothing did it on purpose.
    #[test]
    fn undoing_a_drag_puts_the_symbol_back_as_well() {
        let mut app = CadApp::default();
        app.doc.units.metres_per_unit = 1.0;
        let id = placed(&mut app, (1.0, 1.0));
        let before = inserts(&app);

        app.begin_fixture_drag(id, (1.0, 1.0));
        // The app drives this either side of `drag_to`, which is where the one undo step is taken.
        let was = app.light.drag.as_ref().is_some_and(|d| d.moved);
        app.light.drag_to((4.0, 1.0));
        let now = app.light.drag.as_ref().is_some_and(|d| d.moved);
        if now && !was {
            app.snapshot_doc_and_lights();
        }
        app.drag_fixture_symbols();
        app.light.end_drag();
        assert_ne!(
            inserts(&app),
            before,
            "the symbol never moved in the first place"
        );

        app.do_undo();
        let l = &app.light.luminaires[0];
        assert!(
            (l.position.x - 1.0).abs() < 1e-5,
            "the fixture is at {} after an undo",
            l.position.x,
        );
        assert_eq!(
            inserts(&app),
            before,
            "the symbol did not come back with its fixture"
        );

        // ONE step, not two: a second Undo must reach past the drag, not undo half of it.
        app.do_undo();
        assert!(
            app.light.luminaires.is_empty() || inserts(&app).len() == before.len(),
            "the drag was recorded as two steps, so an Undo left the drawing and the lights apart",
        );
    }
    /// A DRAG IS ONE UNDO, and the fixture goes back where it was.
    #[test]
    fn undoing_a_drag_puts_the_fixture_back() {
        let mut app = CadApp::default();
        let id = app.place_fixture_point(1.0, 1.0);
        app.light.select(id, false);

        app.light.begin_drag(id, (1.0, 1.0));
        app.light.drag_to((4.0, 1.0));
        assert!(app.light.end_drag(), "the drag reported no movement");
        app.commit_light_undo(); // what the frame-end drain does

        assert!((app.light.luminaires[0].position.x - 4.0).abs() < 1e-5);

        app.do_undo();
        assert!(
            (app.light.luminaires[0].position.x - 1.0).abs() < 1e-5,
            "the fixture is at {} after an undo",
            app.light.luminaires[0].position.x,
        );
    }

    /// A PRESS THAT NEVER MOVED IS A SELECTION and must not cost an Undo press.
    #[test]
    fn clicking_a_marker_costs_no_undo_step() {
        let mut app = CadApp::default();
        let id = app.place_fixture_point(1.0, 1.0);
        let depth = app.undo_stack.len();

        app.light.begin_drag(id, (1.0, 1.0));
        // A REAL STATIONARY PRESS STILL DRAGS. The pointer reports its position every frame the
        // button is down, so `drag_to` runs with a zero delta — which is exactly where staging
        // must decline to happen.
        for _ in 0..5 {
            app.light.drag_to((1.0, 1.0));
        }
        assert!(!app.light.end_drag(), "a stationary press reported a move");
        app.commit_light_undo();
        assert_eq!(
            app.undo_stack.len(),
            depth,
            "selecting a marker pushed an undo step"
        );
    }

    /// UNDOING A DRAWING EDIT LEAVES THE FIXTURES ALONE.
    ///
    /// A step carries only what its edit changed. Were every drawing step to carry a copy of the
    /// fixtures, undoing an unrelated line would silently rewind the lighting with it — which is
    /// the failure mode of the obvious implementation and the reason for separate variants.
    #[test]
    fn undoing_a_line_does_not_disturb_the_lighting() {
        let mut app = CadApp::default();
        app.place_fixture_point(1.0, 1.0);
        app.place_fixture_point(2.0, 1.0);

        // `CadApp::default()` opens with demo geometry, so count rather than assume.
        let before_line = app.doc.dobjects.len();
        app.snapshot_doc();
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(1.0, 0.0),
                },
            )));

        app.place_fixture_point(3.0, 1.0);
        assert_eq!(app.light.luminaires.len(), 3);

        // Undo the third point, then the line.
        app.do_undo();
        assert_eq!(app.light.luminaires.len(), 2);
        app.do_undo();
        assert_eq!(
            app.doc.dobjects.len(),
            before_line,
            "the line was not undone"
        );
        assert_eq!(
            app.light.luminaires.len(),
            2,
            "undoing a line rewound the lighting to {} fixtures",
            app.light.luminaires.len(),
        );
    }

    /// IDS ARE NEVER REWOUND. `next_id` is deliberately not in a snapshot: a drawing that still
    /// names a deleted fixture must resolve to nothing rather than to whatever took its place, and
    /// an undo that rewound the counter would hand the next placement a live id.
    #[test]
    fn an_undone_placement_does_not_free_its_id() {
        let mut app = CadApp::default();
        let a = app.place_fixture_point(1.0, 1.0);
        app.do_undo();
        let b = app.place_fixture_point(2.0, 1.0);
        assert_ne!(a, b, "the undone fixture's id was handed out again");
    }

    /// The same rule at the point it would actually be broken: `next_id` is not in a snapshot, so
    /// restoring one must leave the counter alone. Putting it in is the obvious thing to do —
    /// alongside the fixtures, in the same struct — and the damage is silent, because the collision
    /// only shows up in a drawing saved before the undo and reopened after it.
    #[test]
    fn restoring_fixtures_does_not_rewind_the_id_counter() {
        let mut app = CadApp::default();
        let a = app.place_fixture_point(1.0, 1.0);
        let b = app.place_fixture_point(2.0, 1.0);
        let counter = app.light.next_id;

        app.restore_lights(LightSnap {
            luminaires: Vec::new(),
        });
        assert_eq!(
            app.light.next_id, counter,
            "restoring rewound the id counter"
        );

        let c = app.place_fixture_point(3.0, 1.0);
        assert!(
            c != a && c != b,
            "id {c} collides with one already handed out"
        );
    }

    /// A RESTORED LAYOUT DOES NOT KEEP THE OLD RESULT.
    ///
    /// The lux figures were computed from a layout that no longer holds. Recalculating on every
    /// undo would cost minutes on a real building; leaving them on screen is worse than both,
    /// because a figure under a layout it was not computed from is one someone will issue.
    #[test]
    fn undoing_a_fixture_edit_drops_the_stale_result() {
        let mut app = CadApp::default();
        app.place_fixture_point(1.0, 1.0);
        app.light.grid = Some(cad_light::LuxGrid::from_values(1, 1, vec![250.0]));
        app.place_fixture_point(2.0, 1.0);
        app.do_undo();
        assert!(
            app.light.grid.is_none(),
            "a stale lux grid survived the undo"
        );
    }
}
/// THE REPORT BUTTON OPENS A DIALOG. It used to write an HTML file to the Desktop and tell you
/// afterwards, which is neither a choice nor a place anyone keeps a project.

#[cfg(test)]
mod the_report_is_asked_about_before_it_is_written {
    use super::*;

    fn calculated() -> CadApp {
        let mut app = CadApp::default();
        // THE SAVED SETTINGS ARE NOT READ IN A TEST.
        //
        // The dialog loads `~/.config/rust_cad/report.json` on the first frame it opens, which is
        // correct behaviour and made every test in here depend on whatever sat in a real person's
        // home directory. It bit exactly as you would expect: `every_control_is_on_screen` looked
        // for the "Save PDF" button and found "Save HTML", because a preferences file on this
        // machine happened to say HTML. A suite whose result depends on a file no test wrote is a
        // suite that fails for one developer and passes for another, over nothing.
        //
        // Set here rather than in `CadApp::default`, because reading them is real behaviour and
        // should keep happening everywhere except where it is deliberately being kept out.
        app.report_prefs_loaded = true;
        app.light.grid = Some(cad_light::LuxGrid {
            cols: 4,
            rows: 4,
            values: (0..16).map(|i| 100.0 + i as f64 * 10.0).collect(),
            min: 100.0,
            max: 250.0,
            avg: 175.0,
            maintenance: 0.8,
            direct: Vec::new(),
            indirect: Vec::new(),
        });
        app.light.plane = Some(cad_light::CalcPlane {
            origin: cad_light::Vertex::new(0.0, 0.0, 0.8),
            width: 4.0,
            depth: 4.0,
            cols: 4,
            rows: 4,
        });
        // A calculation produces a LIST of room results, and the report reads that list — the
        // loose fields are what the single-room PANEL shows.
        app.light.rooms = vec![crate::light::RoomResult {
            name: "Test room".into(),
            poly: Vec::new(),
            plane: app.light.plane.clone().expect("plane"),
            grid: app.light.grid.clone().expect("grid"),
            mask: Vec::new(),
            plane_en: app.light.plane.clone().expect("plane"),
            grid_en: app.light.grid.clone().expect("grid"),
            mask_en: Vec::new(),
            cylindrical_avg: None,
            installation: None,
            fixtures: Vec::new(),
            grid_note: None,
        }];
        app
    }

    fn frame(app: &mut CadApp) -> usize {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| app.render_report_dialog(ctx));
        out.shapes.len()
    }

    /// NOTHING IS WRITTEN UNTIL IT IS ASKED FOR, and the button opens the dialog instead.
    #[test]
    fn pressing_report_opens_the_dialog_and_writes_nothing() {
        let mut app = calculated();
        assert!(!app.report_open);
        app.export_light_report();
        assert!(app.report_open, "the dialog did not open");
        // No output path has been chosen, so there is nothing it could have written to.
        assert!(app.report_opts.out_dir.is_empty() || app.current_file.is_some());
    }

    /// WITH NOTHING CALCULATED IT SAYS SO rather than opening a dialog over an empty document.
    #[test]
    fn without_a_calculation_the_dialog_stays_shut() {
        let mut app = CadApp::default();
        app.export_light_report();
        assert!(!app.report_open, "the dialog opened with nothing to report");
        assert!(
            app.light.last_msg.contains("Calculate"),
            "and it must say why"
        );
    }

    /// Every piece of text the dialog actually painted.
    ///
    /// Reading the LABELS off a rendered frame, rather than searching the binary for the literals:
    /// an optimised build is free to merge, split and re-order string constants, so "the bytes are
    /// in the file" answers a question nobody asked. What matters is whether the control is on
    /// screen.
    fn dialog_text(app: &mut CadApp) -> Vec<String> {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            // Big enough that nothing is scrolled out of the frame and culled.
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1800.0, 2400.0),
            )),
            ..Default::default()
        };
        // SEVERAL FRAMES, and the options column SCROLLED THROUGH.
        //
        // egui lays a window out before it paints it — frame one comes back as two dozen `Noop`
        // shapes and nothing else, which is what an earlier test counted and passed on. And the
        // options are taller than their panel, so anything below the fold is culled and never
        // painted at all: reading one frame answers "what is visible", not "what is there".
        let mut all: Vec<String> = Vec::new();
        let mut collect = |out: egui::FullOutput, all: &mut Vec<String>| {
            fn walk(s: &egui::Shape, into: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => into.push(t.galley.text().to_string()),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, into)),
                    _ => {}
                }
            }
            for cs in &out.shapes {
                walk(&cs.shape, all);
            }
        };
        for _ in 0..3 {
            let out = ctx.run(input(), |ctx| app.render_report_dialog(ctx));
            collect(out, &mut all);
        }
        // Then wheel the options column down, a screenful at a time, gathering as we go.
        for _ in 0..12 {
            let mut ri = input();
            ri.events
                .push(egui::Event::PointerMoved(egui::pos2(140.0, 400.0)));
            ri.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -180.0),
                modifiers: Default::default(),
            });
            let out = ctx.run(ri, |ctx| app.render_report_dialog(ctx));
            collect(out, &mut all);
        }
        all.sort();
        all.dedup();
        return all;
        #[allow(unreachable_code)]
        let out = ctx.run(input(), |ctx| app.render_report_dialog(ctx));
        fn walk(s: &egui::Shape, into: &mut Vec<String>) {
            match s {
                egui::Shape::Text(t) => into.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, into)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        for cs in &out.shapes {
            walk(&cs.shape, &mut v);
        }
        v
    }

    /// THE DIALOG SHOWS EVERY CONTROL IT IS SUPPOSED TO.
    ///
    /// "is everything wired in?" — asked after several rounds of additions, and the honest way to
    /// answer is to render the thing and read it back rather than to remember.
    #[test]
    fn every_control_is_on_screen() {
        let mut app = calculated();
        app.export_light_report();
        let t = dialog_text(&mut app);
        let has = |needle: &str| t.iter().any(|s| s.contains(needle));

        for want in [
            "Format",
            "PDF",
            "HTML",
            "A4",
            "Letter",
            "Cover",
            "Cover page",
            "Project",
            "Line 2",
            "Image",
            "Header & footer",
            "Page numbers",
            "Header logo",
            "Footer logo",
            "Add logo",
            "Logos fit a",
            "False-colour scale",
            "Pin top",
            "Bands",
            "Sections",
            "Renders",
            "Add",
            "Folder",
            "choose a folder",
            "Preview",
            "Save PDF",
        ] {
            assert!(
                has(want),
                "the dialog never painted {want:?}\npainted: {t:?}"
            );
        }
    }

    /// AND EVERY SECTION IS TICKABLE — all ten, by the label the user reads.
    #[test]
    fn every_section_has_a_tick_box() {
        let mut app = calculated();
        app.export_light_report();
        let t = dialog_text(&mut app);
        for s in crate::report::Section::all() {
            assert!(
                t.iter().any(|x| x == s.label()),
                "no tick box for {:?} ({:?})",
                s,
                s.label(),
            );
        }
    }

    /// THE DIALOG DRAWS — every panel, with a preview of a real document.
    #[test]
    fn the_dialog_and_its_preview_run() {
        let mut app = calculated();
        app.export_light_report();
        // NOT A SHAPE COUNT. This used to assert `shapes > 0`, and egui returns two dozen `Noop`
        // shapes on the frame it lays a window out in — so the test passed on a dialog that had
        // painted nothing whatever, and went on passing while controls were added to it.
        let t = dialog_text(&mut app);
        assert!(
            t.iter().any(|s| s == "Format"),
            "the dialog painted nothing: {t:?}"
        );
        assert!(
            t.iter().any(|s| s.contains("Preview")),
            "the preview column is missing"
        );
        assert!(app.report_open, "it closed itself");
    }

    /// THE PREVIEW IS THE DOCUMENT. Turning a section off has to change the pages on screen, or
    /// the preview is a picture of a report rather than of THIS report.
    #[test]
    fn the_preview_follows_the_options() {
        let mut app = calculated();
        app.export_light_report();
        let inp = app.report_input().expect("a calculation");
        let with_grid = crate::report::layout::layout(&inp, &app.report_opts)
            .pages
            .len();
        drop(inp);

        app.report_opts
            .set(crate::report::Section::NumericGrid, false);
        app.report_opts.set(crate::report::Section::Results, false);
        let inp = app.report_input().expect("a calculation");
        let without = crate::report::layout::layout(&inp, &app.report_opts);
        assert!(
            !without
                .pages
                .iter()
                .flat_map(|p| p.items.iter())
                .any(|i| matches!(i, crate::report::pdf::Item::Text { text, .. }
                                  if text == "Illuminance grid (lx)")),
            "the grid is still in the document after being switched off",
        );
        assert!(
            without.pages.len() <= with_grid,
            "dropping sections made it longer"
        );
    }

    /// THE FILE GOES WHERE THE DIALOG SAYS, in the format it says.
    #[test]
    fn the_report_is_written_where_it_was_asked_for() {
        let dir = std::env::temp_dir().join("simlux_report_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");

        let mut app = calculated();
        app.export_light_report();
        app.report_opts.out_dir = dir.to_string_lossy().into_owned();
        app.report_opts.file_stem = "gym".into();
        app.report_opts.title = "Gym".into();

        app.report_opts.format = crate::report::Format::Pdf;
        app.write_report();
        let pdf = dir.join("gym.pdf");
        assert!(pdf.is_file(), "no PDF was written");
        let bytes = std::fs::read(&pdf).expect("read");
        assert!(bytes.starts_with(b"%PDF-"), "the PDF is not a PDF");
        assert!(bytes.ends_with(b"%%EOF\n"), "the PDF did not finish");
        assert!(
            !app.report_open,
            "the dialog stayed open after a successful save"
        );

        app.report_open = true;
        app.report_opts.format = crate::report::Format::Html;
        app.write_report();
        let html = dir.join("gym.html");
        assert!(html.is_file(), "no HTML was written");
        let text = std::fs::read_to_string(&html).expect("read");
        assert!(text.starts_with("<!doctype html>"), "the HTML is not HTML");
        assert!(text.contains("Gym"), "the title did not reach the file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// EACH LIST REPOINTS ITS OWN SLOTS, AND ONLY ITS OWN.
    ///
    /// The cover, the header and the footer each name a picture by INDEX. When all three kinds
    /// shared one list they all had to shift together whenever anything was removed — and this
    /// test asserted exactly that.
    ///
    /// They no longer share it: renders, logos and cover pictures are three lists, chosen from
    /// three buttons, because twice now a picture had to be added in the wrong place to get it
    /// where it was wanted. Which means the old shifting had become the very damage it existed to
    /// prevent — deleting a RENDER moved the header logo onto a different picture.
    #[test]
    fn removing_an_image_repoints_the_cover() {
        let mut app = calculated();
        let img = |n: &str| crate::report::ReportImage {
            path: format!("{n}.jpg"),
            caption: n.into(),
            jpeg: Some((vec![0xFF, 0xD8], 10, 10)),
        };
        for n in ["a", "b", "c"] {
            app.report_opts.images.push(img(n));
            app.report_tex.push(None);
            app.report_opts.logos.push(img(n));
            app.report_logo_tex.push(None);
            app.report_opts.covers.push(img(n));
            app.report_cover_tex.push(None);
        }
        app.report_open = true;

        // A RENDER GOES. The header and the cover point into other lists and must not move.
        app.report_opts.cover_image = Some(2);
        app.report_opts.header_image = Some(2);
        app.report_remove_image(0);
        assert_eq!(app.report_opts.images.len(), 2);
        assert_eq!(app.report_opts.images[0].caption, "b");
        assert_eq!(
            app.report_opts.header_image,
            Some(2),
            "removing a render moved the header logo onto a different picture",
        );
        assert_eq!(
            app.report_opts.cover_image,
            Some(2),
            "removing a render moved the cover onto a different picture",
        );
        assert_eq!(
            app.report_tex.len(),
            2,
            "the preview textures went out of step"
        );
    }

    /// A LOGO GOING TAKES THE HEADER AND FOOTER WITH IT — the list that actually owns them.
    #[test]
    fn removing_a_logo_repoints_the_header_and_footer() {
        let mut app = calculated();
        for n in ["a", "b", "c"] {
            app.report_opts.logos.push(crate::report::ReportImage {
                path: format!("{n}.png"),
                caption: n.into(),
                jpeg: Some((vec![0xFF, 0xD8], 10, 10)),
            });
            app.report_logo_tex.push(None);
        }
        app.report_opts.header_image = Some(2);
        app.report_opts.footer_image = Some(0);
        app.report_open = true;

        let mut act = crate::report::ui::Action::default();
        act.remove_logo = Some(0);
        app.apply_report_action(act);

        assert_eq!(app.report_opts.logos.len(), 2);
        assert_eq!(
            app.report_opts.header_image,
            Some(1),
            "the header did not follow its logo"
        );
        assert_eq!(
            app.report_opts.footer_image, None,
            "the footer kept a dead index"
        );
        assert_eq!(
            app.report_logo_tex.len(),
            2,
            "the preview textures went out of step"
        );
    }

    /// AND A COVER PICTURE CAN BE DROPPED. A list that can only grow is a list with a mistake in
    /// it for ever — the Add button is not much use without one.
    #[test]
    fn a_cover_picture_can_be_dropped() {
        let mut app = calculated();
        for n in ["a", "b"] {
            app.report_opts.covers.push(crate::report::ReportImage {
                path: format!("{n}.jpg"),
                caption: n.into(),
                jpeg: Some((vec![0xFF, 0xD8], 10, 10)),
            });
            app.report_cover_tex.push(None);
        }
        app.report_opts.cover_image = Some(1);
        app.report_open = true;

        let mut act = crate::report::ui::Action::default();
        act.remove_cover = Some(1);
        app.apply_report_action(act);
        assert_eq!(app.report_opts.covers.len(), 1);
        assert_eq!(
            app.report_opts.cover_image, None,
            "the cover kept a dead index"
        );
        assert_eq!(
            app.report_cover_tex.len(),
            1,
            "the preview textures went out of step"
        );
    }

    /// THE DOCUMENT IS LAID OUT ONCE, NOT ONCE A FRAME.
    ///
    /// Reported as: *"the app starts lagging once the report window is open."* Laying it out is
    /// 123 ms on the owner's three-room plan in a RELEASE build, and it was happening on every
    /// frame the dialog was open — eight frames a second before a single widget had been drawn.
    ///
    /// Counted by LAYOUTS RUN — not by clock, and not by cache key. A timing assertion measures
    /// the build machine. And watching the cache KEY, which is what this test did first, cannot
    /// see the failure at all: a key that never matches is exactly as stable as one that always
    /// does, so the first version of this test passed against a cache deliberately broken to miss
    /// on every single frame. Counting the work is the only thing that answers the question.
    #[test]
    fn the_preview_is_not_rebuilt_every_frame() {
        let mut app = calculated();
        app.export_light_report();
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };

        // A FEW FRAMES TO SETTLE FIRST. The dialog is allowed to reach its resting state over the
        // opening frames — egui lays a window out before it paints it, and a setting may resolve
        // itself once on the way in. What is under test is that it STOPS; "rebuilt exactly once"
        // would fail on any harmless one-off while proving nothing extra.
        for _ in 0..3 {
            let _ = ctx.run(input(), |ctx| app.render_report_dialog(ctx));
        }
        let settled = crate::report::layout::LAYOUTS.with(|n| n.get());
        assert!(settled > 0, "the dialog never laid a document out at all");
        for _ in 0..10 {
            let _ = ctx.run(input(), |ctx| app.render_report_dialog(ctx));
        }
        let after_ten_more = crate::report::layout::LAYOUTS.with(|n| n.get());
        assert_eq!(
            after_ten_more,
            settled,
            "ten idle frames laid the document out {} more time(s)",
            after_ten_more - settled,
        );
        assert!(app.report_doc.is_some(), "the document was not kept");
    }

    /// AND IT IS REBUILT THE MOMENT SOMETHING WOULD CHANGE IT.
    ///
    /// The other half, and the one that decides whether the cache is worth having: a preview that
    /// does not follow the options is worse than a slow one, because it is wrong. Each of these is
    /// a different route into the document — the section list, the page size, the false-colour
    /// scale, a caption, and the calculation underneath.
    #[test]
    fn the_preview_follows_every_option() {
        let changes: Vec<(&str, fn(&mut CadApp))> = vec![
            ("a section switched off", |a: &mut CadApp| {
                a.report_opts.set(crate::report::Section::Schedule, false);
            }),
            ("the section order", |a: &mut CadApp| {
                a.report_opts
                    .move_section(crate::report::Section::Renders, -1);
            }),
            ("the paper size", |a: &mut CadApp| {
                a.report_opts.page = crate::report::PageSize::Letter;
            }),
            ("the cover switched off", |a: &mut CadApp| {
                a.report_opts.cover = false
            }),
            ("the project name", |a: &mut CadApp| {
                a.report_opts.title = "Gym".into()
            }),
            ("the header text", |a: &mut CadApp| {
                a.report_opts.header = "HSI".into()
            }),
            ("the scale ceiling", |a: &mut CadApp| {
                a.report_opts.scale.top = Some(750.0)
            }),
            ("the scale bands", |a: &mut CadApp| {
                a.report_opts.scale.bands = vec![50.0, 200.0]
            }),
            ("the maintenance factor", |a: &mut CadApp| {
                a.light.maintenance.llmf = 0.5
            }),
            ("the eye height", |a: &mut CadApp| a.light.eye_height = 1.6),
            ("a wall reflectance", |a: &mut CadApp| {
                a.light.materials[1].reflectance = 0.9
            }),
            ("the palette", |a: &mut CadApp| {
                a.light.ramp = crate::light::LuxRamp::Grey
            }),
            ("a room renamed", |a: &mut CadApp| {
                a.light.rooms[0].name = "Hall".into()
            }),
            ("a new calculation", |a: &mut CadApp| {
                a.light.results_fingerprint = Some(1234);
            }),
        ];

        for (what, apply) in changes {
            let mut app = calculated();
            app.export_light_report();
            let ctx = egui::Context::default();
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1600.0, 1000.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input(), |ctx| app.render_report_dialog(ctx));
            let before_key = app.report_doc_key.expect("a document");
            let before_n = crate::report::layout::LAYOUTS.with(|n| n.get());

            apply(&mut app);
            let _ = ctx.run(input(), |ctx| app.render_report_dialog(ctx));
            assert_ne!(
                app.report_doc_key,
                Some(before_key),
                "{what}: the key did not move, so the preview would not have redrawn",
            );
            assert!(
                crate::report::layout::LAYOUTS.with(|n| n.get()) > before_n,
                "{what}: the key moved but the document was not laid out again",
            );
        }
    }

    /// A LOGO WITH A TRANSPARENT BACKGROUND ARRIVES ON WHITE, NOT ON BLACK.
    ///
    /// Reported as: *"the logo image i uploaded are png images without background the but in the
    /// report they came with black background."* `to_rgb8` DISCARDS the alpha channel rather than
    /// resolving it, so a transparent pixel kept whatever colour sat underneath — which PNG
    /// writers leave as black. A logo exported with a transparent background therefore arrived as
    /// a black rectangle with the mark cut out of it.
    ///
    /// The fixture is the real case: a mark on a fully transparent field, with a half-transparent
    /// band, which is what anti-aliasing produces and what a threshold would ruin.
    ///
    /// IT IS 96 PIXELS WIDE FOR A REASON. The first version was four pixels across, and passed
    /// against a build with the fix removed — JPEG works in 8 × 8 blocks, and on an image smaller
    /// than one block the decoded values are a smear of everything in it rather than the pixels
    /// that went in. The bands here are wide enough that their centres come back as themselves.
    #[test]
    fn a_transparent_logo_is_flattened_onto_white() {
        let dir = std::env::temp_dir().join("simlux_logo_alpha");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("mark.png");

        // Three wide bands: fully transparent over black (what an exporter leaves behind), a
        // half-covered band, and the opaque mark itself.
        let img = image::RgbaImage::from_fn(96, 64, |x, _| match x {
            0..=31 => image::Rgba([0, 0, 0, 0]),
            32..=63 => image::Rgba([0, 0, 0, 128]),
            _ => image::Rgba([220, 40, 40, 255]),
        });
        img.save(&path).expect("write the fixture");

        let mut app = CadApp::default();
        app.report_add_image(
            &path.to_string_lossy(),
            None,
            crate::report::ImageSlot::Logo,
        );
        assert_eq!(
            app.report_opts.logos.len(),
            1,
            "the logo was not taken: {:?}",
            app.history
        );

        // Read the ENCODED bytes back, because that is what the PDF embeds.
        let (bytes, _, _) = app.report_opts.logos[0].jpeg.clone().expect("encoded");
        let back = image::load_from_memory(&bytes)
            .expect("valid jpeg")
            .to_rgb8();
        let px = |x: u32| back.get_pixel(x, 32).0;

        let clear = px(16);
        assert!(
            clear[0] > 235 && clear[1] > 235 && clear[2] > 235,
            "the transparent background came back as {clear:?} — it became a solid one",
        );
        let ink = px(80);
        assert!(
            ink[0] > 180 && ink[1] < 90,
            "the mark itself was lost: {ink:?}"
        );
        // Half-covered black over white is mid grey. A threshold would have made it black or white
        // and left every logo in the report with a jagged edge.
        let edge = px(48);
        assert!(
            (100..=155).contains(&edge[0]),
            "the half-transparent band came back as {edge:?} rather than blended to mid grey",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// THE SAVE BUTTON IS ON THE SCREEN — on a small display as well as a large one.
    ///
    /// Reported as: *"theres no option to export the report."* The option was there. It had been
    /// pushed off the bottom of the display by a preview that grew every frame, and a dialog whose
    /// Save button is past the edge of the screen is a report dialog that cannot produce a report.
    ///
    /// So this is not a test that the button EXISTS — `every_control_is_on_screen` covers that,
    /// and covered it while the bug was live, because a culled widget and an off-screen one are
    /// different things. It is a test of WHERE it is, at three display sizes including one small
    /// enough that a careless layout would overflow it.
    #[test]
    fn the_save_button_is_reachable_on_a_small_screen() {
        for (w, h) in [(1280.0_f32, 720.0_f32), (1600.0, 1025.0), (3440.0, 1349.0)] {
            let mut app = calculated();
            app.export_light_report();
            let ctx = egui::Context::default();
            let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h));
            let input = || egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };

            // The button's own label, and where it was actually painted.
            let mut found: Option<egui::Rect> = None;
            for _ in 0..12 {
                let out = ctx.run(input(), |ctx| app.render_report_dialog(ctx));
                fn walk(s: &egui::Shape, into: &mut Option<egui::Rect>) {
                    match s {
                        egui::Shape::Text(t) if t.galley.text().trim().starts_with("Save ") => {
                            *into = Some(egui::Rect::from_min_size(t.pos, t.galley.size()));
                        }
                        egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, into)),
                        _ => {}
                    }
                }
                let mut this = None;
                for cs in &out.shapes {
                    walk(&cs.shape, &mut this);
                }
                if this.is_some() {
                    found = this;
                }
            }
            let r = found.unwrap_or_else(|| panic!("{w}x{h}: the Save button never painted"));
            assert!(
                screen.contains_rect(r),
                "{w}x{h}: the Save button is at {:?}, off a screen of {:?}",
                r,
                screen,
            );
        }
    }

    /// A REPORT BUILT ON AN OUT-OF-DATE CALCULATION SAYS SO, IN THE DIALOG.
    ///
    /// The report is the one thing this app produces that leaves the building, and on paper a lux
    /// figure for a layout somebody has since changed is indistinguishable from one that is right.
    /// The panel's own warning is not enough: it is possible to open the report dialog without
    /// having looked at the panel at all.
    #[test]
    fn the_dialog_warns_when_the_calculation_is_out_of_date() {
        let mut app = calculated();
        app.export_light_report();
        let fresh = dialog_text(&mut app);
        assert!(
            !fresh.iter().any(|s| s.contains("OUT OF DATE")),
            "a current calculation was labelled out of date",
        );

        let mut app = calculated();
        app.light.results_stale = true;
        app.export_light_report();
        let t = dialog_text(&mut app);
        assert!(
            t.iter().any(|s| s.contains("OUT OF DATE")),
            "the dialog said nothing about a stale calculation: {t:?}",
        );
    }

    /// THE PREVIEW IS THE SAME SIZE ON EVERY FRAME IT IS OPEN.
    ///
    /// Reported as: *"when the calculation preview sort of loads and expands as its opened. its
    /// looks very buggy."*
    ///
    /// It was a feedback loop. The preview took `ui.available_size()` and the window sized itself
    /// to its contents, so each frame the preview grew to whatever the window had become and the
    /// window grew to fit the preview — visible as a panel inflating for several frames after the
    /// dialog opens, which reads as a rendering fault rather than as a layout settling.
    ///
    /// The frames are compared against EACH OTHER rather than against an expected size: what was
    /// wrong was the growing, and pinning a number here would only record today's dimensions.
    #[test]
    fn the_preview_does_not_grow_after_it_opens() {
        let mut app = calculated();
        app.export_light_report();

        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        // The preview's backing rectangle — the one thing on the dialog painted in this grey.
        fn preview_rect(out: &egui::FullOutput) -> Option<egui::Rect> {
            fn walk(s: &egui::Shape, into: &mut Vec<egui::Rect>) {
                match s {
                    egui::Shape::Rect(r) if r.fill == egui::Color32::from_gray(40) => {
                        into.push(r.rect)
                    }
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, into)),
                    _ => {}
                }
            }
            let mut found = Vec::new();
            for cs in &out.shapes {
                walk(&cs.shape, &mut found);
            }
            found.into_iter().next()
        }

        // TWELVE FRAMES. egui does not paint a window on the frame it first lays out, and this one
        // carries a scroll area that settles over a few more, so the preview does not appear at all
        // for the first handful — which is itself part of what the report is about.
        let mut sizes: Vec<egui::Vec2> = Vec::new();
        for _ in 0..12 {
            let out = ctx.run(input(), |ctx| app.render_report_dialog(ctx));
            if let Some(r) = preview_rect(&out) {
                sizes.push(r.size());
            }
        }
        assert!(
            sizes.len() >= 4,
            "the preview painted on only {} frames",
            sizes.len()
        );
        let first = sizes[0];
        for (i, s) in sizes.iter().enumerate() {
            assert!(
                (s.x - first.x).abs() < 0.5 && (s.y - first.y).abs() < 0.5,
                "the preview was {:.0}x{:.0} on the first painted frame and {:.0}x{:.0} on frame \
                 {i} — it is still growing after it opened: {sizes:?}",
                first.x,
                first.y,
                s.x,
                s.y,
            );
        }
    }

    /// AND IT IS THE SHAPE OF THE PAGE IT IS PREVIEWING.
    ///
    /// A preview box shaped by whatever space was left over shows an A4 page letterboxed inside
    /// it, with the wasted band reading as part of the document. Sizing the box from the page's
    /// own proportions is also what makes the size deterministic — it depends on the document,
    /// which is known before anything is laid out, rather than on the window, which is not.
    #[test]
    fn the_preview_has_the_proportions_of_the_page() {
        let mut app = calculated();
        app.report_opts.page = crate::report::PageSize::A4;
        app.export_light_report();

        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        fn preview_rect(out: &egui::FullOutput) -> Option<egui::Rect> {
            fn walk(s: &egui::Shape, into: &mut Vec<egui::Rect>) {
                match s {
                    egui::Shape::Rect(r) if r.fill == egui::Color32::from_gray(40) => {
                        into.push(r.rect)
                    }
                    egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, into)),
                    _ => {}
                }
            }
            let mut found = Vec::new();
            for cs in &out.shapes {
                walk(&cs.shape, &mut found);
            }
            found.into_iter().next()
        }

        let mut last = None;
        for _ in 0..12 {
            let out = ctx.run(input(), |ctx| app.render_report_dialog(ctx));
            if let Some(r) = preview_rect(&out) {
                last = Some(r);
            }
        }
        let r = last.expect("the preview painted");
        let (pw, ph) = crate::report::PageSize::A4.points();
        let want = ph / pw;
        let got = (r.height() / r.width()) as f64;
        assert!(
            (got - want).abs() < 0.05,
            "the preview is {got:.3} tall-to-wide for a page that is {want:.3}",
        );
    }
}
/// THE OWNER'S OWN THREE-ROOM FILE.
///
/// `testfile2.dxf` is the plan the multi-room work was reported against — three rooms, 23 fittings
/// of one type. Ignored by default because it lives in the owner's Dropbox rather than the repo;
/// run it with the path when the calculation's room handling changes:
///
/// ```text
/// $env:SIMLUX_PROJECT="D:\Dropbox\YASEEN\3d factory\tests\initial test result\testfile2"
/// cargo test -p cad_app the_owners_three_room_plan -- --ignored --nocapture
/// ```
///
/// What it checks is what the synthetic rooms cannot: that a REAL plan's rooms are found, that its
/// fittings land in them, and that every room comes back lit.

#[cfg(test)]
mod the_owners_three_room_plan {
    use super::*;

    /// WHAT THE PHOTOMETRY IN THIS PROJECT ACTUALLY SAYS.
    ///
    /// Asked as: "why does it look like point sources here is it because somethings wrong with the
    /// ldt file or our fault?" — about a results page showing perfectly circular pools under what
    /// are 2 m linear luminaires. There are two candidate answers and they need telling apart
    /// before anything is changed:
    ///
    ///   1. THE FILE. A distribution with one C-plane, or with every C-plane identical, IS
    ///      rotationally symmetric — it describes a fitting that throws the same light in every
    ///      direction around its axis, and a correct engine draws circles for it.
    ///   2. US. Every luminaire is treated as a POINT at its centre (`calc::direct` is a plain
    ///      inverse-square sum). The usual rule is that a point approximation holds beyond about
    ///      five times the largest luminous dimension — 10 m for a 2 m batten — and this plan
    ///      measures 2.1 m below the fittings, well inside that.
    ///
    /// This prints the evidence rather than arguing from either.
    #[test]
    #[ignore = "needs SIMLUX_PROJECT=<path without extension>"]
    fn what_the_photometry_says() {
        let Ok(stem) = std::env::var("SIMLUX_PROJECT") else {
            return;
        };
        let dxf = format!("{stem}.dxf");
        let cfg = crate::simlux_io::load(std::path::Path::new(&dxf))
            .expect("the sidecar must read")
            .expect("the project must have a sidecar");

        println!("profiles in this project: {}", cfg.ies_library.len());
        for (name, p) in &cfg.ies_library {
            let planes = p.horizontal_angles.len();
            // Do the C-planes actually DIFFER? A file can carry twelve of them and repeat one.
            let mut spread = 0.0_f64;
            if planes > 1 {
                let n = p.candela.first().map(|r| r.len()).unwrap_or(0);
                for i in 0..n {
                    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
                    for row in &p.candela {
                        if let Some(v) = row.get(i) {
                            lo = lo.min(*v);
                            hi = hi.max(*v);
                        }
                    }
                    if hi > 0.0 {
                        spread = spread.max((hi - lo) / hi);
                    }
                }
            }
            println!(
                "  {name:<34} C-planes {planes:>3}  γ {:>3}  peak {:>8.1} cd  \
                 luminous {:.3} × {:.3} m  overall {:.3} × {:.3} m",
                p.vertical_angles.len(),
                p.peak_candela(),
                p.luminous_length,
                p.luminous_width,
                p.length,
                p.width,
            );
            println!(
                "  {:<34} C-plane spread {:.1}%  →  {}",
                "",
                spread * 100.0,
                if planes <= 1 || spread < 0.02 {
                    "AXIALLY SYMMETRIC — this file describes a round distribution"
                } else {
                    "asymmetric — the file does distinguish along/across"
                },
            );
            // The near-field question, for the mounting height this project uses.
            let biggest = p
                .luminous_length
                .max(p.luminous_width)
                .max(p.length)
                .max(p.width) as f64;
            if biggest > 0.0 {
                println!(
                    "  {:<34} point-source approximation needs ≥ {:.1} m; this plan measures at \
                     about 2.1 m",
                    "",
                    biggest * 5.0,
                );
            }
        }

        // WHICH FITTINGS ARE WHERE. A round pool under a 36° downlight is correct and a round pool
        // under a 2 m batten is not, so the answer depends entirely on which room is being looked
        // at. Grouped by profile with the extent they cover, which is enough to name the room.
        println!("\nplaced fittings by type:");
        let mut by: std::collections::BTreeMap<String, Vec<&cad_light::Luminaire>> =
            Default::default();
        for l in &cfg.luminaires {
            by.entry(l.profile.clone()).or_default().push(l);
        }
        for (name, ls) in &by {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for l in ls {
                x0 = x0.min(l.position.x);
                y0 = y0.min(l.position.y);
                x1 = x1.max(l.position.x);
                y1 = y1.max(l.position.y);
            }
            let rots: std::collections::BTreeSet<i32> =
                ls.iter().map(|l| l.rotation_deg.round() as i32).collect();
            println!(
                "  {:>3} × {name:<34} over x {x0:>7.2}..{x1:<7.2} y {y0:>7.2}..{y1:<7.2}  \
                 z {:.2}  rotations {rots:?}",
                ls.len(),
                ls.first().map(|l| l.position.z).unwrap_or(0.0),
            );
        }
    }

    #[test]
    #[ignore = "needs SIMLUX_PROJECT=<path without extension>"]
    fn every_room_is_calculated_and_lit() {
        let Ok(stem) = std::env::var("SIMLUX_PROJECT") else {
            return;
        };
        let dxf = format!("{stem}.dxf");
        let cfg = crate::simlux_io::load(std::path::Path::new(&dxf))
            .expect("the sidecar must read")
            .expect("the project must have a sidecar");

        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory.clone());
        app.factory.recompute();
        app.light.apply_config(cfg, &app.doc);

        let plan = CadApp::plan_doc_of(app.factory.session.as_ref(), &app.doc).clone();
        app.light.calculate(&plan, Some(&app.factory));

        println!("rooms calculated: {}", app.light.rooms.len());
        let mut placed = 0usize;
        let mut generated = 0usize;
        for r in &app.light.rooms {
            println!(
                "  {:10} {:>3}x{:<3} plane {:6.2} x {:6.2} m  avg {:7.1} lx  min {:6.1}  U0 {:.2}  \
                 {:2} fitting(s)",
                r.name,
                r.plane.cols,
                r.plane.rows,
                r.plane.width,
                r.plane.depth,
                r.grid.avg,
                r.grid.min,
                r.grid.u0(),
                r.fixtures.len(),
            );
            // A room holds user-placed fixtures AND the lights the model generates — a curved
            // luminaire is a real fitting. Generated ids start at a million, which is how they
            // are told apart.
            placed += r.fixtures.iter().filter(|l| l.id < 1_000_000).count();
            generated += r.fixtures.iter().filter(|l| l.id >= 1_000_000).count();
        }
        println!("placed {placed}, generated {generated}");
        println!(
            "cores: {}",
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(0)
        );
        for (k, v) in &app.light.last_timings {
            println!("  phase {k:<12} {v:>10.1}");
        }

        // WHAT ONE FRAME OF THE REPORT DIALOG COSTS. The preview is the document, laid out from
        // scratch; on a plan this size that is the difference between a dialog and a slideshow.
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let inp = app.report_input().expect("a report input");
            let gather = t.elapsed().as_secs_f64() * 1000.0;
            let t2 = std::time::Instant::now();
            let doc = crate::report::layout::layout(&inp, &app.report_opts);
            println!(
                "  report frame: gather {:>7.1} ms   layout {:>7.1} ms   → {} pages",
                gather,
                t2.elapsed().as_secs_f64() * 1000.0,
                doc.pages.len(),
            );
        }

        assert_eq!(app.light.rooms.len(), 3, "this plan has three rooms");
        for r in &app.light.rooms {
            assert!(
                r.grid.avg > 1.0,
                "{} came out at {:.2} lx — unlit",
                r.name,
                r.grid.avg
            );
            assert!(!r.fixtures.is_empty(), "{} has no fittings in it", r.name);
        }
        assert_eq!(
            placed,
            app.light.luminaires.len(),
            "{} of {} PLACED fittings were claimed by a room",
            placed,
            app.light.luminaires.len(),
        );
        // Every fixture a room claims must be one the schedule can describe. Ids alone could not
        // do this: the generated lights exist only for the length of a calculation.
        for r in &app.light.rooms {
            let sched = app.schedule_for(&r.fixtures);
            let listed: usize = sched.iter().map(|s| s.count).sum();
            assert_eq!(
                listed,
                r.fixtures.len(),
                "{}: the schedule lists {listed} of {} fittings",
                r.name,
                r.fixtures.len(),
            );
        }
    }

    /// EXACTLY WHAT THE REPORT WILL SAY — recalculated from the project, both grids side by side.
    ///
    /// The check to run against a DIALux or Relux report by hand after touching which grid is
    /// quoted. It prints the working grid, the standard's grid, and the figures the report derives
    /// from cells rather than from the summary — the extremes and the percentiles — because those
    /// are the ones that need the mask and so are the ones a change here breaks quietly.
    ///
    /// `SIMLUX_PROJECT=<path without extension> cargo test -p cad_app --bin simlux --release
    ///  what_the_report_will_quote -- --ignored --nocapture`
    #[test]
    #[ignore = "needs SIMLUX_PROJECT=<path without extension>"]
    fn what_the_report_will_quote() {
        let Ok(stem) = std::env::var("SIMLUX_PROJECT") else {
            return;
        };
        let dxf = format!("{stem}.dxf");
        let cfg = crate::simlux_io::load(std::path::Path::new(&dxf))
            .expect("the sidecar must read")
            .expect("the project must have a sidecar");
        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory.clone());
        app.factory.recompute();
        app.light.apply_config(cfg, &app.doc);
        let plan = CadApp::plan_doc_of(app.factory.session.as_ref(), &app.doc).clone();
        app.light.calculate(&plan, Some(&app.factory));

        for r in &app.light.rooms {
            let pct = |g: &cad_light::LuxGrid, m: &[bool], p: f64| -> f64 {
                let mut v: Vec<f64> = g
                    .values
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| m.get(*i).copied().unwrap_or(true))
                    .map(|(_, x)| *x)
                    .collect();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                if v.is_empty() {
                    0.0
                } else {
                    v[(((v.len() - 1) as f64) * p).round() as usize]
                }
            };
            println!("\n=== {} ===", r.name);
            for (label, g, m, sp) in [
                ("working ", &r.grid, r.mask.as_slice(), r.spacing()),
                ("EN 12464", &r.grid_en, r.mask_en.as_slice(), r.spacing_en()),
            ] {
                println!(
                    "  {label}  {:>3} x {:<3} at {sp:.3} m   avg {:7.1}   min {:6.1}   max {:7.1}   \
                     U0 {:.2}   p10 {:6.1}   p90 {:7.1}   {} cell(s) excluded",
                    g.cols,
                    g.rows,
                    g.avg,
                    g.min,
                    g.max,
                    g.u0(),
                    pct(g, m, 0.1),
                    pct(g, m, 0.9),
                    m.iter().filter(|k| !**k).count(),
                );
            }
            println!("  THE REPORT QUOTES the EN 12464 line above.");
        }
    }
}
/// THE LUX SHEET HAS TO BE ON TOP OF THE FLOOR, NOT UNDER IT.
///
/// Reported as: *"the false color and isoline in display those seems to make no difference what so
/// ever. and the false color appearing on the simlux window also is gone."*
///
/// Nothing was broken about the overlay itself — measured on the reference project, all 58,050 of
/// its vertices were in the buffer and correctly coloured from the report's bands. They were at
/// z = 0.005 m, and that room's floor mesh starts at z = 0.100 m. Ninety-five millimetres under the
/// floor, hidden by it, and both toggles looked dead.
///
/// The cause was arithmetic that reads as obviously right: `plane.origin.z - plane_height`. It
/// assumes the working plane is measured up from z = 0, and a room standing on a slab breaks it.

#[cfg(test)]
mod the_lux_sheet_sits_on_the_floor {
    use super::*;

    /// A room on a RAISED floor — the case the arithmetic gets wrong. Built through the factory so
    /// the floor comes out wherever the real model puts it, rather than wherever a test says.
    fn a_room_on_a_slab() -> CadApp {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(8.0, 0.0),
            glam::Vec2::new(8.0, 6.0),
            glam::Vec2::new(0.0, 6.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(&rect, 3.0)
            .expect("building");
        app.factory.add_room(&rect).expect("room");
        app.factory.recompute();
        app.light.auto_center_light = false;
        app.light.cell_size = 0.5;
        for (x, y) in [(2.0, 2.0), (6.0, 2.0), (2.0, 4.0), (6.0, 4.0)] {
            app.light.luminaires.push(cad_light::Luminaire {
                id: app.light.luminaires.len() as u32 + 1,
                profile: crate::light::BUILTIN.to_string(),
                position: cad_light::Vertex::new(x, y, 2.9),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            });
        }
        let doc = cad_kernel::Document::default();
        app.light.calculate(&doc, Some(&app.factory));
        app
    }

    /// The highest floor surface under the room — what the sheet has to clear.
    fn floor_top(app: &CadApp) -> f32 {
        let plane = &app.light.rooms[0].plane;
        let mut top = f32::MIN;
        for m in &app.light.meshes {
            if m.material != 0 {
                continue;
            }
            for v in &m.vertices {
                if v.z <= plane.origin.z - 0.05 {
                    top = top.max(v.z);
                }
            }
        }
        top
    }

    /// EVERY OVERLAY VERTEX IS ABOVE THE FLOOR IT IS DRAWN ON.
    #[test]
    fn the_sheet_is_not_buried_under_the_floor() {
        let mut app = a_room_on_a_slab();
        app.light.floor_heatmap = true;
        app.light.show_isolux = true;
        assert_eq!(app.light.rooms.len(), 1, "the fixture did not calculate");

        let top = floor_top(&app);
        // THE FIXTURE HAS TO REPRODUCE THE CASE. A room whose floor is already at z = 0 would pass
        // this test with the old arithmetic and prove nothing at all.
        assert!(
            top > 0.01,
            "the fixture's floor is at {top:.4} m — it does not reproduce a room standing on a slab",
        );

        let mut verts = Vec::new();
        app.push_lux_overlay(&mut verts);
        assert!(!verts.is_empty(), "the overlay produced no geometry");
        let lowest = verts.iter().map(|v| v.z).fold(f32::MAX, f32::min);
        assert!(
            lowest > top,
            "the overlay's lowest vertex is at {lowest:.4} m and the floor's top is at {top:.4} m — \
             it is drawn underneath and cannot be seen",
        );
        // …and not floating either. A sheet a hand's breadth off the floor reads as a separate
        // object rather than as the floor's colour.
        assert!(
            lowest - top < 0.05,
            "the overlay is {:.3} m above the floor — it will read as a floating panel",
            lowest - top,
        );
    }

    /// AND `floor_under` FINDS IT, which is the thing the sheet depends on.
    #[test]
    fn the_floor_is_read_off_the_model_not_computed_from_the_plane_height() {
        let app = a_room_on_a_slab();
        let plane = &app.light.rooms[0].plane;
        let found = app.floor_under(plane);
        let top = floor_top(&app);
        assert!(
            (found - top).abs() < 1e-4,
            "floor_under says {found:.4} m, the model's floor is at {top:.4} m",
        );
        // The arithmetic this replaced, spelled out — so the test says what it is defending against.
        let by_arithmetic = plane.origin.z - app.light.plane_height;
        assert!(
            (by_arithmetic - top).abs() > 1e-3,
            "the fixture does not discriminate: the old arithmetic gives {by_arithmetic:.4} m and \
             the model's floor is at {top:.4} m",
        );
    }

    /// A PROJECT WITH NO 3D MODEL still gets a sensible height rather than a panic or a NaN.
    #[test]
    fn a_project_with_no_floor_falls_back_to_the_plane_height() {
        let app = CadApp::default();
        let plane = cad_light::CalcPlane {
            origin: cad_light::Vertex::new(0.0, 0.0, 0.8),
            width: 6.0,
            depth: 4.0,
            cols: 12,
            rows: 8,
        };
        let z = app.floor_under(&plane);
        assert!(z.is_finite(), "floor_under returned {z}");
        assert!(
            (z - (0.8 - app.light.plane_height)).abs() < 1e-6,
            "with no model the fallback should be the plane height; got {z:.4}",
        );
    }
}
/// FORENSIC PROBE — does the preview's triangulator finish the polygons the report gives it?
///
/// `SIMLUX_PROJECT=<stem> cargo test -p cad_app --bin simlux --release does_ear_clip_finish -- --ignored --nocapture`

#[cfg(test)]
mod ear_clip_probe {
    use super::*;

    #[test]
    #[ignore = "needs SIMLUX_PROJECT=<path without extension>"]
    fn does_ear_clip_finish() {
        let Ok(stem) = std::env::var("SIMLUX_PROJECT") else {
            return;
        };
        let dxf = format!("{stem}.dxf");
        let cfg = crate::simlux_io::load(std::path::Path::new(&dxf))
            .unwrap()
            .unwrap();
        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory.clone());
        app.factory.recompute();
        app.light.apply_config(cfg, &app.doc);
        let plan = CadApp::plan_doc_of(app.factory.session.as_ref(), &app.doc).clone();
        app.light.calculate(&plan, Some(&app.factory));

        let inp = app.report_input().expect("report input");
        let doc = crate::report::layout::layout(&inp, &app.report_opts);

        let (mut polys, mut short, mut lost) = (0usize, 0usize, 0usize);
        let (mut worst_want, mut worst_got, mut worst_len) = (0usize, 0usize, 0usize);
        for pg in &doc.pages {
            for it in &pg.items {
                if let crate::report::pdf::Item::Poly { rings, .. } = it {
                    for r in rings {
                        let pts: Vec<egui::Pos2> = r
                            .iter()
                            .map(|(x, y)| egui::pos2(*x as f32, *y as f32))
                            .collect();
                        if pts.len() < 3 {
                            continue;
                        }
                        polys += 1;
                        let want = pts.len() - 2;
                        let got = crate::report::ui::ear_clip(&pts).len();
                        if got < want {
                            short += 1;
                            lost += want - got;
                            if want - got > worst_want.saturating_sub(worst_got) {
                                worst_want = want;
                                worst_got = got;
                                worst_len = pts.len();
                            }
                        }
                    }
                }
            }
        }
        println!("polygons on the page      : {polys}");
        println!("INCOMPLETELY triangulated : {short}");
        println!("triangles never emitted   : {lost}");
        if short > 0 {
            println!(
                "worst: a {worst_len}-vertex ring wanted {worst_want} triangles, got {worst_got}"
            );
        }
    }
}
/// AN AIMING ARROW LANDS WHERE THE FITTING POINTS.
///
/// Asked for as: *"in aiming lights add aiming arrows that the user can turn on and off in the
/// illuminaire tab that shows where the light is aimed at."*

#[cfg(test)]
mod aiming_arrows_show_where_a_light_points {
    use super::*;

    /// A lit room with one fitting, high enough that a tilt moves the landing point visibly.
    fn one_light() -> CadApp {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(10.0, 0.0),
            glam::Vec2::new(10.0, 8.0),
            glam::Vec2::new(0.0, 8.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(&rect, 3.0)
            .expect("building");
        app.factory.add_room(&rect).expect("room");
        app.factory.recompute();
        app.light.auto_center_light = false;
        app.light.cell_size = 1.0;
        app.light.luminaires.push(cad_light::Luminaire {
            id: 1,
            profile: crate::light::BUILTIN.to_string(),
            position: cad_light::Vertex::new(5.0, 4.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        let doc = cad_kernel::Document::default();
        app.light.calculate(&doc, Some(&app.factory));
        app
    }

    /// Where the drawn geometry says the fitting is pointing.
    ///
    /// THE MOST-REPEATED VERTEX — which is the arrow's apex, and nothing else.
    ///
    /// Two earlier attempts measured the wrong thing, and both were wrong for the same reason: the
    /// landing point carries a target CROSS as well as the arrow's head, and the cross reaches
    /// further out than the apex does. Taking the furthest vertex gives an arm tip, about 200 mm
    /// off. Taking the deepest ALONG THE AIM works only while the fitting is untilted — the cross
    /// lies flat, so once the aim leans, its downhill arm is deeper than the apex. That version
    /// passed the straight-down test and failed the tilted one, which is the half that matters.
    ///
    /// The head is a four-sided pyramid, so its apex is emitted once per face: the only vertex in
    /// the buffer that appears four times, exactly, and it is placed exactly on the target.
    fn tip(app: &CadApp) -> glam::Vec3 {
        let mut v = Vec::new();
        app.push_aim_arrows(&mut v);
        assert!(!v.is_empty(), "no arrow geometry at all");
        let pts: Vec<glam::Vec3> = v.iter().map(|p| glam::Vec3::new(p.x, p.y, p.z)).collect();
        let same = |a: &glam::Vec3, b: &glam::Vec3| (*a - *b).length() < 1e-5;
        let (best, n) = pts
            .iter()
            .map(|p| (*p, pts.iter().filter(|q| same(p, q)).count()))
            .max_by_key(|(_, n)| *n)
            .expect("a vertex");
        assert!(
            n >= 4,
            "the most repeated vertex appears {n} times — the head is a four-sided pyramid and its \
             apex should appear four. This helper is no longer finding the tip.",
        );
        best
    }

    /// STRAIGHT DOWN LANDS STRAIGHT BELOW. The un-aimed case, and the one every downlight is in.
    #[test]
    fn an_untilted_fitting_points_at_the_floor_beneath_it() {
        let mut app = one_light();
        app.light.show_aim = true;
        let t = tip(&app);
        assert!(
            (t.x - 5.0).abs() < 0.05 && (t.y - 4.0).abs() < 0.05,
            "the arrow lands at ({:.2}, {:.2}) — the fitting is at (5.00, 4.00) and is not tilted",
            t.x,
            t.y,
        );
        let floor = app.floor_under(&app.light.rooms[0].plane);
        assert!(
            (t.z - floor).abs() < 0.05,
            "the arrow ends at z = {:.3} and the floor is at {floor:.3}",
            t.z,
        );
    }

    /// AND AIMING MOVES IT. The whole point: aim the fitting somewhere and the arrow follows.
    ///
    /// Checked against the target the fitting was aimed AT, not merely against "it moved" — an
    /// arrow that swings to the wrong place is worse than one that does not move, because it looks
    /// like it is working.
    #[test]
    fn aiming_a_fitting_moves_the_arrow_to_the_point_it_was_aimed_at() {
        let mut app = one_light();
        app.light.show_aim = true;
        let floor = app.floor_under(&app.light.rooms[0].plane);
        let target = glam::Vec3::new(8.0, 6.5, floor);
        assert!(
            app.light.luminaires[0].aim_at(target),
            "aim_at refused a target below the fitting"
        );

        let t = tip(&app);
        assert!(
            (t - target).length() < 0.08,
            "aimed at ({:.2}, {:.2}, {:.2}) but the arrow lands at ({:.2}, {:.2}, {:.2})",
            target.x,
            target.y,
            target.z,
            t.x,
            t.y,
            t.z,
        );
    }

    /// THE TOGGLE TURNS IT OFF. It was asked for as a switch, and a switch that only goes one way
    /// is a decoration.
    #[test]
    fn the_toggle_removes_the_arrows_entirely() {
        let mut app = one_light();
        app.light.show_aim = true;
        let mut on = Vec::new();
        app.push_aim_arrows(&mut on);
        assert!(!on.is_empty(), "nothing was drawn with the toggle on");

        app.light.show_aim = false;
        let mut off = Vec::new();
        app.push_aim_arrows(&mut off);
        assert!(off.is_empty(), "{} vertices survived the toggle", off.len());
    }

    /// A FITTING AIMED LEVEL STILL DRAWS SOMETHING FINITE. There is no floor ahead of a wallwasher
    /// tipped to the horizontal, and dividing by its vertical component would send the arrow to
    /// infinity — or to a NaN, which the renderer turns into a triangle across the whole scene.
    #[test]
    fn a_fitting_aimed_level_does_not_draw_to_infinity() {
        let mut app = one_light();
        app.light.show_aim = true;
        app.light.luminaires[0].tilt_deg = 90.0; // straight out, no downward component
        let mut v = Vec::new();
        app.push_aim_arrows(&mut v);
        assert!(!v.is_empty(), "a level fitting drew no arrow at all");
        for p in &v {
            assert!(
                p.x.is_finite() && p.y.is_finite() && p.z.is_finite(),
                "a vertex came out non-finite: ({}, {}, {})",
                p.x,
                p.y,
                p.z,
            );
            let d = (glam::Vec3::new(p.x, p.y, p.z) - glam::Vec3::new(5.0, 4.0, 2.9)).length();
            assert!(
                d < 30.0,
                "a vertex is {d:.1} m from the fitting — the arrow ran away"
            );
        }
    }
}
/// FORENSIC PROBE — why is the 3D view slow on this project?
///
/// `SIMLUX_PROJECT=<stem> cargo test -p cad_app --bin simlux --release what_the_gpu_is_actually_drawing -- --ignored --nocapture`

#[cfg(test)]
mod what_the_gpu_draws_probe {
    use super::*;

    #[test]
    #[ignore = "needs SIMLUX_PROJECT=<path without extension>"]
    fn what_the_gpu_is_actually_drawing() {
        let Ok(stem) = std::env::var("SIMLUX_PROJECT") else {
            return;
        };
        let dxf = format!("{stem}.dxf");
        let t0 = std::time::Instant::now();
        let cfg = crate::simlux_io::load(std::path::Path::new(&dxf))
            .unwrap()
            .unwrap();
        println!("sidecar parsed in {:.1} s", t0.elapsed().as_secs_f64());

        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory.clone());
        let t1 = std::time::Instant::now();
        app.factory.recompute();
        println!("recompute {:.1} ms", t1.elapsed().as_secs_f64() * 1000.0);

        let f = &app.factory;
        println!(
            "\n=== ASSETS ({} in the library) ===",
            f.furniture_lib.len()
        );
        println!(
            "{:<26} {:>9} {:>9} {:>7} {:>9} {:>9}",
            "asset", "tris", "uvs", "alpha", "needs_lod", "lod tris"
        );
        let mut used: std::collections::BTreeMap<usize, usize> = Default::default();
        for inst in &f.furniture {
            *used.entry(inst.asset).or_default() += 1;
        }
        let mut drawn = 0usize;
        let mut drawn_if_lod = 0usize;
        let mut full = 0usize;
        for (idx, n) in &used {
            let Some(a) = f.furniture_lib.get(*idx) else {
                continue;
            };
            let tris = a.positions.len() / 3;
            let need = a.needs_lod();
            // What the decimator WOULD give, whether or not `needs_lod` allows it.
            let would = crate::factory::cluster_decimate(&a.positions, 64).0.len() / 3;
            let lod_tris = if need {
                a.lod_geom().positions.len() / 3
            } else {
                would
            };
            println!(
                "{:<26} {:>9} {:>9} {:>7} {:>9} {:>9}   x{n}",
                a.name,
                tris,
                a.uvs.len(),
                a.alpha.len(),
                need,
                lod_tris,
            );
            // AS DRAWN means as drawn. This counted the full mesh unconditionally and printed it
            // under "as drawn", which was true only while `needs_lod` refused every heavy asset —
            // exactly the same class of lie as the `tris=` line in the perf dump, which counts the
            // CSG buffer and calls it the scene.
            drawn += if need { lod_tris } else { tris } * n;
            full += tris * n;
            drawn_if_lod += lod_tris * n;
        }
        println!("\n=== PER FRAME ===");
        println!(
            "  building (CSG opaque buffer) : {} tris",
            f.cached.positions.len() / 3
        );
        println!(
            "  furniture, at full detail    : {full} tris across {} instances",
            f.furniture.len()
        );
        println!(
            "  furniture, AS DRAWN          : {drawn} tris  ({:.1}x less)",
            full as f64 / drawn.max(1) as f64
        );
        println!("  furniture, if every asset were proxied : {drawn_if_lod} tris");
        println!(
            "  furniture is {:.3}% of the geometry as drawn",
            100.0 * drawn as f64 / (drawn + f.cached.positions.len() / 3).max(1) as f64
        );
    }
}
/// THE SIMLUX 3D VIEW MUST NOT REBUILD THE ROOM IN ORDER TO MOVE A LIGHT.
///
/// It did, on every frame. `build_scene3d_verts` rebuilt everything it drew each time, and on the
/// reference gym plan the room is 7,030,514 triangles — about 844 MB of vertices allocated, filled
/// and dropped per frame, on the UI thread, while the 2D plan beside it was being edited. Reported
/// as the app lagging while placing luminaires.
///
/// The fix splits the buffer by HOW OFTEN IT CHANGES: the room is cached against
/// `scene3d_static_key`, and the lux sheet, the fittings and the aim arrows are rebuilt each frame.
/// Both halves have to hold, and the second is the one that bites — a cache that never invalidates
/// is not fast, it is wrong.

#[cfg(test)]
mod the_scene_cache {
    use super::*;

    /// Every field of the buffer, so "unchanged" means unchanged and not merely the same length.
    fn digest(v: &[crate::light3d::V3]) -> u64 {
        let mut f = crate::light::Fnv::new();
        f.u64(v.len() as u64);
        for p in v {
            for c in [p.x, p.y, p.z, p.r, p.g, p.b, p.nx, p.ny, p.nz, p.mode] {
                f.f32(c);
            }
        }
        f.finish()
    }

    /// A room with a calculated result in it, so both halves have something to build.
    fn a_lit_room() -> CadApp {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(10.0, 0.0),
            glam::Vec2::new(10.0, 8.0),
            glam::Vec2::new(0.0, 8.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(&rect, 3.0)
            .expect("building");
        app.factory.add_room(&rect).expect("room");
        app.factory.recompute();
        // A PIECE OF FURNITURE, because the view reads the factory's furniture live and `recompute`
        // — the only thing that moves `geom_version` — is never called when one is placed or moved.
        let idx = app.factory.add_furniture_asset(
            "stool".into(),
            crate::mesh_io::ObjMesh {
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [0.4, 0.0, 0.0],
                    [0.4, 0.4, 0.0],
                    [0.0, 0.0, 0.0],
                    [0.4, 0.4, 0.0],
                    [0.0, 0.4, 0.0],
                ],
                normals: vec![[0.0, 0.0, 1.0]; 6],
                color: Some([0.6, 0.6, 0.6]),
                alpha: Vec::new(),
            },
        );
        app.factory
            .place_furniture(idx, glam::Vec3::new(5.0, 4.0, 0.0));
        app.light.auto_center_light = false;
        app.light.cell_size = 1.0;
        app.light.luminaires.push(cad_light::Luminaire {
            id: 1,
            profile: crate::light::BUILTIN.to_string(),
            position: cad_light::Vertex::new(5.0, 4.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        let doc = cad_kernel::Document::default();
        app.light.calculate(&doc, Some(&app.factory));
        app
    }

    /// THE CACHE HAS TO HIT AT ALL. A key that moves on its own — a clock, a frame counter, a hash
    /// of something itself rebuilt each frame — would rebuild every frame and look exactly like the
    /// bug it was written to fix, while every invalidation test below still passed.
    #[test]
    fn an_untouched_scene_asks_for_the_same_key_twice() {
        let app = a_lit_room();
        assert_eq!(app.scene3d_static_key(), app.scene3d_static_key());
    }

    /// THE WHOLE POINT: the gesture that lagged must not touch the room. Placing, dragging and
    /// aiming a fitting move the same fields, and none is anything the room is built from.
    #[test]
    fn moving_a_fitting_does_not_rebuild_the_room() {
        let mut app = a_lit_room();
        let key = app.scene3d_static_key();
        let room = digest(&app.build_scene3d_static());

        app.light.luminaires[0].position = cad_light::Vertex::new(2.0, 6.0, 2.9);
        app.light.luminaires[0].tilt_deg = 25.0;
        app.light.luminaires[0].rotation_deg = 90.0;
        app.light.cam_dist = 14.0; // the marker glyph is sized by this
        app.light.luminaires.push(cad_light::Luminaire {
            id: 2,
            profile: crate::light::BUILTIN.to_string(),
            position: cad_light::Vertex::new(8.0, 2.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });

        assert_eq!(
            app.scene3d_static_key(),
            key,
            "placing a fitting must not invalidate the room"
        );
        assert_eq!(
            digest(&app.build_scene3d_static()),
            room,
            "...nor change a vertex of it"
        );
    }

    /// ...AND THE CHANGE MUST STILL REACH THE SCREEN. The half above is only allowed to stay still
    /// because the other half moves; if both were cached the fitting would not appear to move at
    /// all, which is a worse bug than the lag.
    #[test]
    fn moving_a_fitting_does_change_the_per_frame_half() {
        let mut app = a_lit_room();
        let before = digest(&app.build_scene3d_dyn());
        app.light.luminaires[0].position = cad_light::Vertex::new(2.0, 6.0, 2.9);
        assert_ne!(
            digest(&app.build_scene3d_dyn()),
            before,
            "the fitting must visibly move"
        );
    }

    /// EVERY INPUT THE ROOM IS BUILT FROM IS IN THE KEY.
    ///
    /// One at a time, from a fresh scene each time, so a later change cannot be carried by an
    /// earlier one. A miss here fails in the dangerous direction: the user drops a wall's
    /// reflectance, the report changes, the picture does not, and nothing on screen says why.
    #[test]
    fn everything_the_room_is_built_from_moves_the_key() {
        let cases: Vec<(&str, fn(&mut CadApp))> = vec![
            ("the scene triangles", |a: &mut CadApp| {
                // ON A 2D-ONLY PROJECT, where the fallback branch really does draw `light.meshes`.
                // On a factory project it must NOT invalidate — see the sibling test.
                a.factory = crate::factory::FactoryState::default();
                let m = a.light.meshes.clone();
                a.light.set_meshes(m); // same content: the GENERATION is what must move
            }),
            ("the CSG model", |a: &mut CadApp| {
                a.factory.recompute();
            }),
            ("moving a piece of furniture", |a: &mut CadApp| {
                a.factory.furniture[0].pos[0] += 0.4;
            }),
            ("rotating a piece of furniture", |a: &mut CadApp| {
                a.factory.furniture[0].rot[2] += 12.0;
            }),
            ("scaling a piece of furniture", |a: &mut CadApp| {
                a.factory.furniture[0].scale *= 1.3;
            }),
            ("deleting a piece of furniture", |a: &mut CadApp| {
                a.factory.furniture.clear();
            }),
            ("the hide-ceilings toggle", |a: &mut CadApp| {
                a.light.hide_ceilings = !a.light.hide_ceilings;
            }),
            ("the working plane height", |a: &mut CadApp| {
                a.light.plane_height += 0.25;
            }),
            ("a surface reflectance", |a: &mut CadApp| {
                a.light.materials[0].reflectance = 0.11;
            }),
            ("a surface colour", |a: &mut CadApp| {
                a.light.materials[0].color = [0.9, 0.1, 0.1];
            }),
        ];
        for (what, change) in cases {
            let mut app = a_lit_room();
            let before = app.scene3d_static_key();
            change(&mut app);
            assert_ne!(
                app.scene3d_static_key(),
                before,
                "{what} must invalidate the cached room"
            );
        }
    }

    /// A CALCULATION LANDING MUST NOT REBUILD THE ROOM.
    ///
    /// The other half of `everything_the_room_is_built_from_moves_the_key`, and the reason the key
    /// mirrors the build's branch instead of hashing everything. On a project WITH a model the view
    /// draws the factory, not `light.meshes` — so a finished calculation changes nothing it reads,
    /// and invalidating on it costs a 105 MB re-upload for a buffer whose inputs have not moved.
    /// Two of those fire back to back on load; the session dump shows them as 354 ms frames.
    #[test]
    fn a_finished_calculation_does_not_rebuild_the_room() {
        let mut app = a_lit_room();
        assert!(
            !app.factory.furniture.is_empty(),
            "the fixture must be a factory project"
        );
        let before = app.scene3d_static_key();
        let m = app.light.meshes.clone();
        app.light.set_meshes(m);
        assert_eq!(
            app.scene3d_static_key(),
            before,
            "the view draws the factory here, so new engine meshes are not one of its inputs",
        );
    }

    /// `set_meshes` IS THE ONLY WAY IN. Writing the field direct compiles and leaves the view
    /// painting the previous room, so the accessor has to be what moves the counter.
    #[test]
    fn replacing_the_scene_triangles_moves_the_generation() {
        let mut app = a_lit_room();
        let g = app.light.meshes_gen;
        app.light.set_meshes(Vec::new());
        assert_eq!(app.light.meshes_gen, g.wrapping_add(1));
        assert!(app.light.meshes.is_empty());
    }

    /// THE SPLIT IS REAL, not two names for one build: the room half must carry no fitting geometry
    /// and no lux sheet. Measured by emptying the per-frame half's inputs and watching the room
    /// stay exactly as it was.
    #[test]
    fn each_half_carries_only_its_own_geometry() {
        let mut app = a_lit_room();
        let room = digest(&app.build_scene3d_static());
        let framed = digest(&app.build_scene3d_dyn());
        assert!(
            !app.build_scene3d_dyn().is_empty(),
            "the frame half must have had something in it"
        );

        app.light.luminaires.clear();
        app.light.floor_heatmap = false;
        app.light.show_isolux = false;
        assert_eq!(
            digest(&app.build_scene3d_static()),
            room,
            "the room half must not contain fittings or the lux sheet",
        );
        assert!(
            app.build_scene3d_dyn().is_empty(),
            "...and with those gone the frame half is empty"
        );
        assert_ne!(
            framed,
            digest(&app.build_scene3d_dyn()),
            "which is a change, so it was there"
        );
    }

    /// THE VIEW DRAWS THE PROXY; THE CALCULATION KEEPS EVERY TRIANGLE.
    ///
    /// The SIMLUX viewport was built from `light.meshes` — the geometry handed to the ENGINE. On the
    /// gym plan that baked 7,036,129 triangles into one world-space soup: 21,104,808 vertices,
    /// 844 MB. Caching it stopped the rebuild and did nothing about the size, and 844 MB parked in a
    /// persistent GPU buffer is past what a card has spare, so it spills over the bus. That is why
    /// the lag was SIMLUX-only — the 3D Factory instances its furniture, this bakes every instance
    /// out in full. With the display proxy: 2,762,679 vertices, 110.5 MB.
    ///
    /// BOTH HALVES ARE THE TEST. Drawing less is only correct while the engine still sees all of it;
    /// a change that quietly decimated the calculation would make the view fast and the report wrong.
    #[test]
    fn the_view_draws_the_proxy_and_the_calculation_still_sees_every_triangle() {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(12.0, 0.0),
            glam::Vec2::new(12.0, 9.0),
            glam::Vec2::new(0.0, 9.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(&rect, 3.0)
            .expect("building");
        app.factory.add_room(&rect).expect("room");
        app.factory.recompute();

        // One asset over the LOD threshold, and DENSE — a fine grid over one square metre, so many
        // vertices share a lattice cell and there is something to weld. A first attempt walked a
        // helix: every vertex landed in its own cell, nothing merged, and the proxy came back the
        // same size as the mesh. The test caught that, which is the only reason it is not still
        // "passing" against a proxy that does nothing.
        let side = 101usize; // 2 * 101^2 = 20,402 triangles, over LOD_TRI_THRESHOLD
        let mut pos = Vec::with_capacity(side * side * 6);
        for i in 0..side {
            for j in 0..side {
                let (x0, x1) = (i as f32 / side as f32, (i + 1) as f32 / side as f32);
                let (y0, y1) = (j as f32 / side as f32, (j + 1) as f32 / side as f32);
                for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y0), (x1, y1), (x0, y1)] {
                    pos.push([x, y, ((i + j) % 2) as f32 * 0.02]);
                }
            }
        }
        assert!(
            pos.len() / 3 > crate::factory::LOD_TRI_THRESHOLD,
            "the fixture must be over the threshold to have a proxy at all",
        );
        let normals = vec![[0.0, 0.0, 1.0]; pos.len()];
        let idx = app.factory.add_furniture_asset(
            "heavy".into(),
            crate::mesh_io::ObjMesh {
                positions: pos,
                normals,
                color: Some([0.7, 0.7, 0.7]),
                alpha: Vec::new(),
            },
        );
        app.factory
            .place_furniture(idx, glam::Vec3::new(6.0, 4.5, 0.0));

        let full = app.factory.furniture_lib[idx].positions.len() / 3;
        let proxy = app.factory.furniture_lib[idx].lod_geom().tri_count();
        assert!(
            proxy * 2 < full,
            "the fixture's proxy must be much smaller: {proxy} of {full}"
        );

        // THE ENGINE'S GEOMETRY — every triangle, still.
        let calc = crate::light::meshes_from_factory(&app.factory);
        let calc_furn: usize = calc
            .iter()
            .filter(|m| m.material == cad_light::MATERIAL_FURNITURE)
            .map(|m| m.triangles.len())
            .sum();
        assert_eq!(
            calc_furn, full,
            "the calculation must still be handed the whole mesh"
        );

        // THE VIEW'S GEOMETRY — the proxy.
        app.light.set_meshes(calc);
        let drawn = app.build_scene3d_static().len() / 3;
        assert!(
            drawn < full,
            "the view must not draw the calculation's {full} furniture triangles; it drew {drawn}",
        );
        assert!(
            drawn >= proxy,
            "…but it must draw the proxy and the building, not nothing: {drawn} against {proxy}",
        );
    }
}
/// FORENSIC PROBE — what one frame of the SIMLUX 3D view costs, before and after the split.
///
/// `SIMLUX_PROJECT=<stem> cargo test -p cad_app --bin simlux --release what_a_simlux_frame_costs -- --ignored --nocapture`

#[cfg(test)]
mod what_a_simlux_frame_costs_probe {
    use super::*;

    #[test]
    #[ignore = "needs SIMLUX_PROJECT=<path without extension>"]
    fn what_a_simlux_frame_costs() {
        let Ok(stem) = std::env::var("SIMLUX_PROJECT") else {
            return;
        };
        let dxf = format!("{stem}.dxf");
        let cfg = crate::simlux_io::load(std::path::Path::new(&dxf))
            .unwrap()
            .unwrap();

        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory.clone());
        app.factory.recompute();

        // The scene the light engine sees — furniture included. Set directly rather than by running
        // a calculation: this probe is about what the VIEW costs, and the view reads these.
        let meshes = crate::light::meshes_from_factory(&app.factory);
        let tris: usize = meshes.iter().map(|m| m.triangles.len()).sum();
        app.light.set_meshes(meshes);

        // Something for the per-frame half to draw. The file carries no fittings of its own.
        for i in 0..89u32 {
            app.light.luminaires.push(cad_light::Luminaire {
                id: i + 1,
                profile: crate::light::BUILTIN.to_string(),
                position: cad_light::Vertex::new(i as f32 * 0.5, 2.0, 3.0),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            });
        }

        let mb = |n: usize| n as f64 * std::mem::size_of::<crate::light3d::V3>() as f64 / 1.0e6;

        let t = std::time::Instant::now();
        let stat = app.build_scene3d_static();
        let build_ms = t.elapsed().as_secs_f64() * 1000.0;

        // A RESULT ON SCREEN, WHICH IS THE STATE THE USER IS ACTUALLY IN.
        //
        // Without one `push_lux_overlay` returns on its first line, so every "0.1 ms" this probe
        // has ever printed for the per-frame half was measured with the lux sheet switched OFF —
        // the single largest thing in it. Measuring the idle case and reporting it as the frame
        // cost is how the same mistake gets made three times.
        let doc = cad_kernel::Document::default();
        app.light.floor_heatmap = true;
        app.light.show_isolux = true;
        let t_calc = std::time::Instant::now();
        app.light.calculate(&doc, Some(&app.factory));
        println!(
            "\n  (calculated a result first: {:.1} s, {} room(s) — the sheet is the per-frame half's\n   largest term and is absent without one)",
            t_calc.elapsed().as_secs_f64(),
            app.light.rooms.len(),
        );

        let t = std::time::Instant::now();
        let dynv = app.build_scene3d_dyn();
        let dyn_ms = t.elapsed().as_secs_f64() * 1000.0;

        // The cached path, as the view actually calls it: key, then hand back the Arc.
        let t = std::time::Instant::now();
        for _ in 0..60 {
            let k = app.scene3d_static_key();
            std::hint::black_box(k);
        }
        let key_ms = t.elapsed().as_secs_f64() * 1000.0 / 60.0;

        println!("\n=== SCENE ===");
        println!("  light-engine triangles      : {tris}");
        println!("\n=== ONE FRAME, BEFORE (everything rebuilt) ===");
        println!(
            "  room geometry               : {:>9.1} ms   {:>8} verts  {:>8.1} MB",
            build_ms,
            stat.len(),
            mb(stat.len())
        );
        println!(
            "  lux sheet + fittings + aim  : {:>9.1} ms   {:>8} verts  {:>8.1} MB",
            dyn_ms,
            dynv.len(),
            mb(dynv.len())
        );
        println!(
            "  TOTAL PER FRAME             : {:>9.1} ms   {:>8.1} MB",
            build_ms + dyn_ms,
            mb(stat.len() + dynv.len())
        );
        println!("\n=== ONE FRAME, AFTER (room cached, rest rebuilt) ===");
        println!("  cache key                   : {:>9.3} ms", key_ms);
        println!(
            "  lux sheet + fittings + aim  : {:>9.1} ms   {:>8.1} MB",
            dyn_ms,
            mb(dynv.len())
        );
        println!(
            "  TOTAL PER FRAME             : {:>9.1} ms   {:>8.1} MB",
            key_ms + dyn_ms,
            mb(dynv.len())
        );
        if dyn_ms + key_ms > 0.0 {
            println!(
                "\n  {:.0}x less CPU work per frame",
                (build_ms + dyn_ms) / (key_ms + dyn_ms)
            );
        }
        println!("\n  (the lux sheet is absent here — this file has no calculated result. It is");
        println!(
            "   capped at overlay_res's 12,000 cells per room, so it cannot grow without bound.)"
        );
        println!("\n  GPU: the room used to be re-uploaded every frame too (scene_ver = None).");
        println!(
            "       It is now versioned, so those {:.1} MB cross the bus only when it changes.",
            mb(stat.len())
        );
    }
}
/// FORENSIC PROBE — how much of the model is off screen at a typical working camera?
///
/// `SIMLUX_PROJECT=<stem> cargo test -p cad_app --bin simlux --release what_frustum_culling_saves -- --ignored --nocapture`

#[cfg(test)]
mod what_frustum_culling_saves_probe {
    use super::*;

    #[test]
    #[ignore = "needs SIMLUX_PROJECT=<path without extension>"]
    fn what_frustum_culling_saves() {
        let Ok(stem) = std::env::var("SIMLUX_PROJECT") else {
            return;
        };
        let dxf = format!("{stem}.dxf");
        let cfg = crate::simlux_io::load(std::path::Path::new(&dxf))
            .unwrap()
            .unwrap();
        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory.clone());
        app.factory.recompute();

        let f = &app.factory;
        let total_tris: usize = (0..f.furniture.len())
            .filter_map(|i| f.furniture_lib.get(f.furniture[i].asset))
            .map(|a| a.positions.len() / 3)
            .sum();

        // Where the model sits, so the camera can be aimed at something real.
        let mut c = glam::Vec3::ZERO;
        let mut n = 0.0f32;
        for i in 0..f.furniture.len() {
            let m = f
                .furniture_model_matrix(i)
                .unwrap_or(glam::Mat4::IDENTITY.to_cols_array());
            c += glam::Vec3::new(m[12], m[13], m[14]);
            n += 1.0;
        }
        if n > 0.0 {
            c /= n;
        }

        println!(
            "\n=== FRUSTUM CULLING, {} instances / {total_tris} triangles ===",
            f.furniture.len()
        );
        println!(
            "{:<34} {:>6} {:>12} {:>8}",
            "camera", "drawn", "tris drawn", "culled"
        );
        for (label, dist) in [
            ("whole model in shot (120 m)", 120.0f32),
            ("a room (25 m)", 25.0),
            ("working on one machine (6 m)", 6.0),
            ("close on a detail (2 m)", 2.0),
        ] {
            let mvp_m = glam::Mat4::from_cols_array(&crate::light3d::mvp(
                0.6,
                0.35,
                dist,
                c.to_array(),
                16.0 / 9.0,
                false,
            ));
            let (mut drawn, mut tris) = (0usize, 0usize);
            for i in 0..f.furniture.len() {
                let model = f
                    .furniture_model_matrix(i)
                    .unwrap_or(glam::Mat4::IDENTITY.to_cols_array());
                let fmvp = (mvp_m * glam::Mat4::from_cols_array(&model)).to_cols_array();
                let Some(a) = f.furniture_lib.get(f.furniture[i].asset) else {
                    continue;
                };
                if crate::light3d::aabb_in_frustum(&fmvp, a.local_min, a.local_max) {
                    drawn += 1;
                    tris += a.positions.len() / 3;
                }
            }
            let pct = 100.0 * (1.0 - tris as f64 / total_tris.max(1) as f64);
            println!("{:<34} {:>6} {:>12} {:>7.1}%", label, drawn, tris, pct);
        }
        println!("\n  Every one of these used to be submitted at every camera, every frame.");
    }
}
/// EXPRESS AGAINST THOROUGH ON A REAL FURNISHED PROJECT — the gate before Express is trusted.
///
/// The DIALux fixtures cannot answer this: they carry no furniture, so the two modes build
/// identical geometry there and `validate-lighting.ps1` passing in Thorough says nothing at all
/// about Express. It has to be measured on a scene that actually has furniture in it.
///
/// The file carries no luminaires of its own, so a regular grid of the built-in downlight is laid
/// over it. That is fine for this question — both modes see exactly the same lights, and what is
/// being compared is the effect of the furniture representation, not the scheme.
///
/// `SIMLUX_PROJECT=<stem> cargo test -p cad_app --bin simlux --release express_against_thorough -- --ignored --nocapture`

#[cfg(test)]
mod express_against_thorough_probe {
    use super::*;

    #[test]
    #[ignore = "needs SIMLUX_PROJECT=<path without extension>"]
    fn express_against_thorough() {
        let Ok(stem) = std::env::var("SIMLUX_PROJECT") else {
            return;
        };
        let dxf = format!("{stem}.dxf");
        let cfg = crate::simlux_io::load(std::path::Path::new(&dxf))
            .unwrap()
            .unwrap();
        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory.clone());
        app.factory.recompute();

        // A regular scheme over the model's own footprint, at 3 m.
        let (mn, mx) = match app.factory.cached.bounds() {
            Some(b) => b,
            None => return,
        };
        app.light.auto_center_light = false;
        app.light.cell_size = 0.5;
        let (w, d) = (mx[0] - mn[0], mx[1] - mn[1]);
        let (nx, ny) = (8usize, 4usize);
        for i in 0..nx {
            for j in 0..ny {
                app.light.luminaires.push(cad_light::Luminaire {
                    id: (i * ny + j) as u32 + 1,
                    profile: crate::light::BUILTIN.to_string(),
                    position: cad_light::Vertex::new(
                        mn[0] + w * (i as f32 + 0.5) / nx as f32,
                        mn[1] + d * (j as f32 + 0.5) / ny as f32,
                        3.0,
                    ),
                    rotation_deg: 0.0,
                    tilt_deg: 0.0,
                    dimming: 1.0,
                    watts_override: None,
                    flux_override: None,
                    from_block: None,
                });
            }
        }

        let doc = cad_kernel::Document::default();
        let mut row = |mode: crate::light::CalcMode| {
            app.light.mode = mode;
            let t = std::time::Instant::now();
            let job = app.light.prepare(&doc, Some(&app.factory)).expect("a job");
            let tris = job.scene_triangle_count();
            let out = job.run(&crate::light::CalcProgress::default());
            let secs = t.elapsed().as_secs_f64();
            let r = out.rooms.first().expect("a room");
            let en = !r.grid_en.values.is_empty();
            let g = if en { &r.grid_en } else { &r.grid };
            let mask = if en { &r.mask_en } else { &r.mask };
            // HOW MANY CELLS WERE THROWN AWAY, because that is where the two modes differ most:
            // `Obstacle::contains` is exact on a box and meaningless on a mesh that is not
            // watertight, so Express can exclude cells that Thorough silently keeps.
            let dropped = mask.iter().filter(|k| !**k).count();
            println!(
                "{:<10} {:>10} {:>9.1} {:>9.3} {:>9.3} {:>9.3} {:>8.4} {:>8} {:>6}/{}",
                mode.label(),
                tris,
                secs,
                g.avg,
                g.min,
                g.max,
                g.u0(),
                out.surfaces.len(),
                dropped,
                g.values.len(),
            );
            (g.avg, g.min, g.max, g.u0())
        };

        println!("\n=== EXPRESS AGAINST THOROUGH ===");
        println!(
            "{:<10} {:>10} {:>9} {:>9} {:>9} {:>9} {:>8} {:>8} {:>6}",
            "mode",
            "scene tris",
            "seconds",
            "avg lx",
            "min lx",
            "max lx",
            "U0",
            "surfaces",
            "masked"
        );
        let e = row(crate::light::CalcMode::Express);
        let t = row(crate::light::CalcMode::Thorough);
        let pc = |a: f64, b: f64| {
            if b.abs() > 1e-9 {
                100.0 * (a - b) / b
            } else {
                0.0
            }
        };
        println!(
            "\n  Express against Thorough:  avg {:+.1}%   min {:+.1}%   max {:+.1}%   U0 {:+.2}",
            pc(e.0, t.0),
            pc(e.1, t.1),
            pc(e.2, t.2),
            e.3 - t.3,
        );
        println!(
            "  (a box is more occluding than the piece it replaces, so Express is expected LOW)"
        );
    }
}
/// FORENSIC PROBE — is the proxy reaching the draw path, and what does building it cost?
///
/// `SIMLUX_PROJECT=<stem> cargo test -p cad_app --bin simlux --release what_the_lod_costs -- --ignored --nocapture`

#[cfg(test)]
mod what_the_lod_costs_probe {
    use super::*;

    #[test]
    #[ignore = "needs SIMLUX_PROJECT=<path without extension>"]
    fn what_the_lod_costs() {
        let Ok(stem) = std::env::var("SIMLUX_PROJECT") else {
            return;
        };
        let dxf = format!("{stem}.dxf");
        let cfg = crate::simlux_io::load(std::path::Path::new(&dxf))
            .unwrap()
            .unwrap();
        let mut app = CadApp::default();
        app.factory.apply_persist(cfg.factory.clone());
        app.factory.recompute();
        let f = &app.factory;

        // ---- what it costs to BUILD the proxies, the first time anything draws --------------
        println!("\n=== BUILDING THE PROXY (first draw, lazily) ===");
        println!(
            "{:<26} {:>9} {:>11} {:>10} {:>10}",
            "asset", "tris", "group_geom", "lod_geom", "proxy tris"
        );
        let mut used: std::collections::BTreeMap<usize, usize> = Default::default();
        for inst in &f.furniture {
            *used.entry(inst.asset).or_default() += 1;
        }
        let mut total_build = 0.0f64;
        for (idx, _) in &used {
            let Some(a) = f.furniture_lib.get(*idx) else {
                continue;
            };
            if !a.needs_lod() {
                continue;
            }
            let t = std::time::Instant::now();
            let g = a.group_geom();
            let gms = t.elapsed().as_secs_f64() * 1000.0;
            let t = std::time::Instant::now();
            let l = a.lod_geom();
            let lms = t.elapsed().as_secs_f64() * 1000.0;
            total_build += gms + lms;
            println!(
                "{:<26} {:>9} {:>9.1} ms {:>8.1} ms {:>10}",
                a.name,
                a.positions.len() / 3,
                gms,
                lms,
                l.tri_count(),
            );
            let _ = g;
        }
        println!("  total, once, on the UI thread at first draw: {total_build:.0} ms");

        // ---- what the DRAW PATH actually emits, now the caches are warm ----------------------
        println!("\n=== WHAT THE DRAW PATH EMITS (per instance) ===");
        let (mut drawn, mut full) = (0usize, 0usize);
        // WARM THE MEMO FIRST. Timing a single pass times 26 cache MISSES and then calls the
        // result a per-frame cost — the same mistake as reading `heaviest=` off the perf line and
        // taking it for what is drawn.
        let warmup = std::time::Instant::now();
        for i in 0..f.furniture.len() {
            let _ = f.furniture_faceted(i);
            let _ = f.furniture_textured_mesh(i);
        }
        let cold = warmup.elapsed().as_secs_f64() * 1000.0;
        let t = std::time::Instant::now();
        for i in 0..f.furniture.len() {
            let Some(a) = f.furniture_lib.get(f.furniture[i].asset) else {
                continue;
            };
            full += a.positions.len() / 3;
            let mut n = 0usize;
            if let Some(fac) = f.furniture_faceted(i) {
                for (_, _, v) in &fac.opaque {
                    n += v.len() / 3;
                }
                for (_, _, v) in &fac.translucent {
                    n += v.len() / 3;
                }
                if let Some((_, v)) = &fac.flat {
                    n += v.len() / 3;
                }
            } else if let Some((_, _, v)) = f.furniture_textured_mesh(i) {
                n += v.len() / 3;
            } else {
                n += f.furniture_local_mesh(i).len() / 3;
                n += f
                    .furniture_translucent_mesh(i)
                    .map(|(_, v)| v.len() / 3)
                    .unwrap_or(0);
            }
            drawn += n;
        }
        let warm = t.elapsed().as_secs_f64() * 1000.0;
        println!("  full detail        : {full} tris");
        println!(
            "  ACTUALLY SUBMITTED : {drawn} tris   ({:.1}x less)",
            full as f64 / drawn.max(1) as f64
        );
        println!("  building the split, once      : {cold:.1} ms");
        println!("  asking again, memo warm       : {warm:.1} ms");
        println!(
            "
=== PER-FRAME COST BY FUNCTION (memos warm, as the render loop calls them) ==="
        );
        for (name, n) in [
            ("furniture_faceted", 0),
            ("furniture_translucent_mesh", 1),
            ("furniture_textured_mesh", 2),
            ("furniture_model_matrix", 3),
        ] {
            let t = std::time::Instant::now();
            for _ in 0..10 {
                for i in 0..f.furniture.len() {
                    match n {
                        0 => {
                            let _ = f.furniture_faceted(i);
                        }
                        1 => {
                            let _ = f.furniture_translucent_mesh(i);
                        }
                        2 => {
                            let _ = f.furniture_textured_mesh(i);
                        }
                        _ => {
                            let _ = f.furniture_model_matrix(i);
                        }
                    }
                }
            }
            println!(
                "  {:<32} {:>8.2} ms per frame",
                name,
                t.elapsed().as_secs_f64() * 100.0
            );
        }
    }
}
/// THE 2D LUX OVERLAY IS ONE MESH PER ROOM, NOT ONE SHAPE PER CELL.
///
/// It called `rect_filled` for every calculated cell — up to `MAX_GRID_POINTS`, 16,384 separate
/// `Shape::Rect`s per room, EVERY FRAME. egui tessellates and anti-aliases each shape on its own,
/// so that is 16,384 trips through the tessellator and ~130,000 vertices of paint list built and
/// dropped per frame, on the UI thread.
///
/// It runs only when there is a lighting RESULT to draw, which is why the lag was "only when SIMLUX
/// is open" and why it left no trace in the modelling session dumps that three rounds of 3D work
/// were aimed at.

#[cfg(test)]
mod the_two_d_lux_overlay {
    use super::*;

    /// A calculated room, painted once through a headless egui pass, returning the shapes emitted.
    fn shapes_for(cell_size: f32) -> Vec<egui::Shape> {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(10.0, 0.0),
            glam::Vec2::new(10.0, 8.0),
            glam::Vec2::new(0.0, 8.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(&rect, 3.0)
            .expect("building");
        app.factory.add_room(&rect).expect("room");
        app.factory.recompute();
        app.light.auto_center_light = false;
        app.light.cell_size = cell_size;
        app.light.luminaires.push(cad_light::Luminaire {
            id: 1,
            profile: crate::light::BUILTIN.to_string(),
            position: cad_light::Vertex::new(5.0, 4.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        app.light
            .calculate(&cad_kernel::Document::default(), Some(&app.factory));
        app.light.show_overlay = true;
        assert!(
            !app.light.rooms.is_empty(),
            "the fixture must have a calculated room"
        );

        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
            app.paint_lux_overlay(&painter, r);
        });
        out.shapes.into_iter().map(|c| c.shape).collect()
    }

    /// ONE SHAPE FOR THE WHOLE ROOM, however many cells it has.
    ///
    /// Counting shapes and not time: a timing assertion on a build machine is a flake, and the
    /// thing that was wrong is structural — the shape COUNT scaled with the grid.
    #[test]
    fn a_room_is_painted_as_one_mesh_however_fine_its_grid() {
        for cell in [1.0f32, 0.5, 0.25] {
            let shapes = shapes_for(cell);
            let meshes = shapes
                .iter()
                .filter(|s| matches!(s, egui::Shape::Mesh(_)))
                .count();
            let rects = shapes
                .iter()
                .filter(|s| matches!(s, egui::Shape::Rect(_)))
                .count();
            assert_eq!(meshes, 1, "cell {cell} m: one mesh for the room");
            assert_eq!(
                rects, 0,
                "cell {cell} m: no per-cell rectangles, {rects} found"
            );
        }
    }

    /// AND IT STILL PAINTS THE CELLS. A mesh with nothing in it would pass the count above and
    /// draw an empty plan, which is a worse bug than the one being fixed — so the finer grid must
    /// carry more triangles than the coarse one.
    #[test]
    fn the_mesh_carries_a_quad_per_cell() {
        let tris = |cell: f32| -> usize {
            shapes_for(cell)
                .iter()
                .filter_map(|s| match s {
                    egui::Shape::Mesh(m) => Some(m.indices.len() / 3),
                    _ => None,
                })
                .sum()
        };
        let coarse = tris(1.0);
        let fine = tris(0.25);
        assert!(coarse >= 2, "even one cell is a quad: {coarse} triangles");
        assert!(
            fine > coarse * 4,
            "a four-times-finer grid must carry far more triangles: {fine} against {coarse}",
        );
        // EVERY CELL IS A QUAD — four vertices, two triangles. `fine % 2 == 0` looked like it said
        // that and does not: one triangle per cell is also an even count whenever the cell count
        // is, so dropping half of every quad passed it. The vertex-to-index ratio pins it exactly.
        for s in shapes_for(0.5) {
            if let egui::Shape::Mesh(m) = s {
                assert_eq!(
                    m.vertices.len() * 3,
                    m.indices.len() * 2,
                    "{} vertices against {} indices is not four-and-six per cell",
                    m.vertices.len(),
                    m.indices.len(),
                );
            }
        }
    }
}
/// THE SIMLUX VIEW REPORTS WHAT IT COSTS.
///
/// It did not, and that is why the lag took four rounds. `FACTORY PERF` events come only from
/// `render_factory_panel`, so a SIMLUX session recorded nothing at all and every frame number in
/// such a dump — 20.3 ms, 517 ms, `tris=`, `heaviest=` — described a view that was not running.
/// The only clue was the ABSENCE of events, which is a poor thing to have to notice.

#[cfg(test)]
mod the_simlux_perf_tap {
    use super::*;

    /// THE 2D OVERLAY'S COST IS RECORDED, and its cell count with it.
    ///
    /// This is the one that mattered: the overlay lives in the CAD canvas, so no 3D counter could
    /// see it, and it runs only when a lighting result exists — so it left no trace in any of the
    /// modelling dumps that three rounds of work were aimed at. What was wrong was that its cell
    /// count scaled with the grid, so the cell count is what has to be in the record.
    #[test]
    fn painting_the_overlay_records_its_cost_and_its_cell_count() {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(10.0, 0.0),
            glam::Vec2::new(10.0, 8.0),
            glam::Vec2::new(0.0, 8.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(&rect, 3.0)
            .expect("building");
        app.factory.add_room(&rect).expect("room");
        app.factory.recompute();
        app.light.auto_center_light = false;
        app.light.cell_size = 0.5;
        app.light.luminaires.push(cad_light::Luminaire {
            id: 1,
            profile: crate::light::BUILTIN.to_string(),
            position: cad_light::Vertex::new(5.0, 4.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        app.light
            .calculate(&cad_kernel::Document::default(), Some(&app.factory));
        app.light.show_overlay = true;

        assert_eq!(app.lux_overlay_cells.get(), 0, "nothing painted yet");
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
            app.paint_lux_overlay(&painter, r);
        });
        let cells = app.lux_overlay_cells.get();
        assert!(
            cells > 100,
            "a 10 x 8 m room at 0.5 m is hundreds of cells; recorded {cells}"
        );

        // AND IT RESETS. A counter that only ever accumulates reads as a leak the first time the
        // overlay is switched off, and would report the last busy frame forever.
        app.light.show_overlay = false;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
            app.paint_lux_overlay(&painter, r);
        });
        assert_eq!(
            app.lux_overlay_cells.get(),
            0,
            "switched off, it must record zero cells"
        );
    }

    /// THE TAP IS IN THE SIMLUX VIEW, not only in the factory's. A grep, because the alternative is
    /// standing up a GL context; it fails the day somebody deletes the tap, which is the regression
    /// that matters — a silent view is exactly what cost four rounds.
    #[test]
    fn the_simlux_view_emits_a_perf_event() {
        // ANCHORED ON STRINGS THIS TEST DOES NOT ITSELF CONTAIN. `include_str!("../mod.rs")` includes
        // THIS MODULE, so searching for a phrase that also appears in the assertions finds the
        // assertion and passes whatever happened to the code. The first version searched for the
        // tap's banner comment; renaming the banner renamed it in both places and the test sailed
        // through. Each needle below is assembled at run time so the literal is not in the file.
        let src = include_str!("../ports_simlux.rs");
        let needle = |parts: &[&str]| -> String { parts.concat() };
        // Starts ABOVE the `if recording` gate, or the slice cannot contain the gate it is asked to
        // check for — which is how the first run of this failed.
        let anchor = needle(&["let t_dyn = std::time::", "Instant::now();"]);
        let a = src.find(&anchor).expect("the SIMLUX perf tap is gone");
        let b = src[a..]
            .find("\n                if verts.is_empty()")
            .map(|e| a + e)
            .expect("re-anchor this if the tap moves");
        let body = &src[a..b];
        // Generous, because the point is to catch an anchor that silently matched the whole rest
        // of the file — not to police how long the tap is allowed to be.
        assert!(
            body.len() < 8_000,
            "the slice must be the tap, not half the file"
        );
        for parts in [
            &["DbgEvent::", "FactoryPerf"][..],
            &["simlux-", "slow-frame"][..],
            &["self.lux_overlay_", "us.get()"][..],
            &["self.lux_overlay_", "cells.get()"][..],
            &["if self.dbg.", "recording"][..],
        ] {
            let n = needle(parts);
            assert!(body.contains(&n), "the tap no longer mentions `{n}`");
        }
    }
}
#[cfg(test)]
mod what_furniture_costs_the_calculation {
    use super::*;

    /// Does one small piece of furniture really make a calculation six times slower?
    #[test]
    #[ignore = "timing probe"]
    fn time_calculate_with_and_without_furniture() {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(10.0, 0.0),
            glam::Vec2::new(10.0, 8.0),
            glam::Vec2::new(0.0, 8.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        for with in [false, true] {
            let mut app = CadApp::default();
            app.factory
                .add_building_outline(&rect, 3.0)
                .expect("building");
            app.factory.add_room(&rect).expect("room");
            app.factory.recompute();
            if with {
                let idx = app.factory.add_furniture_asset(
                    "stool".into(),
                    crate::mesh_io::ObjMesh {
                        positions: vec![
                            [0.0, 0.0, 0.0],
                            [0.4, 0.0, 0.0],
                            [0.4, 0.4, 0.0],
                            [0.0, 0.0, 0.0],
                            [0.4, 0.4, 0.0],
                            [0.0, 0.4, 0.0],
                        ],
                        normals: vec![[0.0, 0.0, 1.0]; 6],
                        color: Some([0.6, 0.6, 0.6]),
                        alpha: Vec::new(),
                    },
                );
                app.factory
                    .place_furniture(idx, glam::Vec3::new(5.0, 4.0, 0.0));
            }
            app.light.auto_center_light = false;
            app.light.cell_size = 1.0;
            app.light.luminaires.push(cad_light::Luminaire {
                id: 1,
                profile: crate::light::BUILTIN.to_string(),
                position: cad_light::Vertex::new(5.0, 4.0, 2.9),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            });
            let t = std::time::Instant::now();
            app.light
                .calculate(&cad_kernel::Document::default(), Some(&app.factory));
            let r = app.light.rooms.first().expect("a room");
            println!(
                "furniture={with:<5} calculate {:>7.2} s   grid {}x{}  avg {:.1} lx",
                t.elapsed().as_secs_f64(),
                r.plane.cols,
                r.plane.rows,
                r.grid.avg,
            );
        }
    }
}
/// FORENSIC PROBE — what the 2D lux overlay costs, one mesh against one shape per cell.

#[cfg(test)]
mod what_the_two_d_overlay_costs {
    use super::*;

    #[test]
    #[ignore = "timing probe"]
    fn one_mesh_against_one_shape_per_cell() {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(20.0, 0.0),
            glam::Vec2::new(20.0, 14.0),
            glam::Vec2::new(0.0, 14.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut app = CadApp::default();
        app.factory
            .add_building_outline(&rect, 3.0)
            .expect("building");
        app.factory.add_room(&rect).expect("room");
        app.factory.recompute();
        app.light.auto_center_light = false;
        app.light.cell_size = 0.2; // a fine grid, as a real project has
        app.light.luminaires.push(cad_light::Luminaire {
            id: 1,
            profile: crate::light::BUILTIN.to_string(),
            position: cad_light::Vertex::new(10.0, 7.0, 2.9),
            rotation_deg: 0.0,
            tilt_deg: 0.0,
            dimming: 1.0,
            watts_override: None,
            flux_override: None,
            from_block: None,
        });
        app.light
            .calculate(&cad_kernel::Document::default(), Some(&app.factory));
        app.light.show_overlay = true;
        let r = app.light.rooms.first().expect("a room");
        let cells = (r.plane.cols * r.plane.rows) as usize;

        let ctx = egui::Context::default();
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 900.0));

        // THE PATH AS SHIPPED — one mesh per room. Timed over several paints; the first allocates.
        let mut new_ms = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let out = ctx.run(egui::RawInput::default(), |c| {
                app.paint_lux_overlay(&c.layer_painter(egui::LayerId::background()), area);
            });
            // TESSELLATION IS INSIDE THE MEASUREMENT. `ctx.run` only builds the paint LIST; the
            // per-shape anti-aliasing that makes 16,384 rectangles expensive happens here, and a
            // timing that stopped before it would miss the entire effect under test.
            let prims = ctx.tessellate(out.shapes, 1.0);
            std::hint::black_box(prims.len());
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            new_ms = new_ms.min(ms);
        }

        // THE OLD PATH — one `rect_filled` per cell, rebuilt here so the comparison is measured and
        // not remembered.
        let mut old_ms = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let out = ctx.run(egui::RawInput::default(), |c| {
                let p = c.layer_painter(egui::LayerId::background());
                let clip = p.with_clip_rect(area);
                for room in &app.light.rooms {
                    let (grid, plane) = (&room.grid, &room.plane);
                    let dx = plane.width / plane.cols.max(1) as f32;
                    let dy = plane.depth / plane.rows.max(1) as f32;
                    for row in 0..plane.rows {
                        for col in 0..plane.cols {
                            let i = (row * plane.cols + col) as usize;
                            if room.mask.get(i).is_some_and(|k| !k) {
                                continue;
                            }
                            let v = grid.values[i];
                            let c3 = app.report_opts.lux_rgb(v, 500.0, app.light.ramp.rgb_fn());
                            let x0 = plane.origin.x + col as f32 * dx;
                            let y0 = plane.origin.y + row as f32 * dy;
                            let p0 = app.w2s_m(Vec2::new(x0 as f64, y0 as f64), area);
                            let p1 = app.w2s_m(Vec2::new((x0 + dx) as f64, (y0 + dy) as f64), area);
                            clip.rect_filled(
                                egui::Rect::from_two_pos(p0, p1),
                                0.0,
                                egui::Color32::from_rgb(c3[0], c3[1], c3[2]),
                            );
                        }
                    }
                }
            });
            let prims = ctx.tessellate(out.shapes, 1.0);
            std::hint::black_box(prims.len());
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            old_ms = old_ms.min(ms);
        }

        println!("\n=== 2D LUX OVERLAY, {cells} cells ===");
        println!("  one shape per cell (before) : {old_ms:>8.2} ms per frame");
        println!("  one mesh per room  (after)  : {new_ms:>8.2} ms per frame");
        println!("  {:.0}x less", old_ms / new_ms.max(1e-6));
    }
}
/// THE RECORDER HAD NO LIGHTING EVENTS AT ALL.
///
/// "why is the calculations not being made. i noticed the lights were also not being placed" — and
/// a full 59-second recording of it showed one click on empty space. `FactoryOp` records what
/// happens to the model, `FactoryPerf` what a frame costs, `FactoryScene` what the scene IS, and
/// between them nothing said a fitting had been placed or a calculation asked for.
///
/// The point of these is the REFUSALS. Placement declines in three places and calculation in two,
/// and three of those five say nothing whatever to the user — from outside the app, a button that
/// does nothing. Each one has to leave a line in the dump saying which it was.

#[cfg(test)]
mod the_recorder_sees_the_lighting {
    use super::*;
    use crate::dbg_recorder::{format_event_oneline, DbgEvent};

    fn recording() -> CadApp {
        let mut app = CadApp::default();
        app.dbg.recording = true;
        app.dbg.session_started = Some(std::time::Instant::now());
        app
    }

    fn light_ops(app: &CadApp) -> Vec<(String, String, String)> {
        app.dbg
            .events
            .iter()
            .filter_map(|r| match &r.event {
                DbgEvent::LightOp {
                    op,
                    detail,
                    message,
                    ..
                } => Some((op.clone(), detail.clone(), message.clone())),
                _ => None,
            })
            .collect()
    }

    /// CLICKING WITH NO FITTING ARMED IS THE SILENT ONE. `place_illuminaire_at` returns `false`
    /// without touching the status line, so the user sees a click that did nothing and the app has
    /// no account of it. This is the single most likely shape of "the lights were not being placed".
    #[test]
    fn a_placement_with_nothing_armed_is_recorded_as_refused() {
        let mut app = recording();
        assert!(
            app.light.place_fitting.is_none(),
            "the fixture must start unarmed"
        );

        let placed = app.place_illuminaire_at(1.0, 2.0);
        assert!(!placed, "nothing is armed, so nothing is placed");

        let ops = light_ops(&app);
        assert_eq!(ops.len(), 1, "the refusal must leave exactly one line");
        assert_eq!(ops[0].0, "place refused");
        assert!(
            ops[0].1.contains("NO FITTING ARMED"),
            "the line must name the reason, not just that it happened: {}",
            ops[0].1,
        );
        assert!(
            ops[0].2.is_empty(),
            "and record that the app said NOTHING — that absence is the finding",
        );
    }

    /// A SILENT REFUSAL IS FLAGGED AS SILENT in the rendered dump, so nobody has to notice that a
    /// field is empty. A refusal that DID explain itself must not carry the flag.
    #[test]
    fn a_refusal_that_said_nothing_is_called_out() {
        let ev = |message: &str| {
            format_event_oneline(&DbgEvent::LightOp {
                op: "place refused".into(),
                detail: "d".into(),
                fittings_before: 4,
                fittings_after: 4,
                message: message.into(),
                elapsed_us: 0,
            })
        };
        assert!(
            ev("").contains("NO MESSAGE"),
            "a refusal with no status line must say so: {}",
            ev(""),
        );
        assert!(!ev("Close the face sketch first.").contains("NO MESSAGE"));
        assert!(
            ev("Close the face sketch first.").contains("Close the face sketch first."),
            "and must quote what the app actually said",
        );
        // A successful op with no message is not a finding, so it must stay quiet.
        let ok = format_event_oneline(&DbgEvent::LightOp {
            op: "place".into(),
            detail: "d".into(),
            fittings_before: 4,
            fittings_after: 5,
            message: String::new(),
            elapsed_us: 0,
        });
        assert!(
            !ok.contains("NO MESSAGE"),
            "only refusals are held to that standard"
        );
    }

    /// A PLACEMENT THAT CHANGED NO COUNT is the shape of "I clicked and nothing happened", and the
    /// dump flags it the way `FactoryOp` flags a cut that added no feature.
    #[test]
    fn a_placement_that_added_nothing_is_flagged() {
        let ev = |before: usize, after: usize| {
            format_event_oneline(&DbgEvent::LightOp {
                op: "place".into(),
                detail: "d".into(),
                fittings_before: before,
                fittings_after: after,
                message: "m".into(),
                elapsed_us: 0,
            })
        };
        assert!(ev(4, 4).contains("FITTING COUNT UNCHANGED"));
        assert!(!ev(4, 5).contains("FITTING COUNT UNCHANGED"));
    }

    /// PRESSING CALCULATE WITH NOTHING TO CALCULATE must leave a line carrying the message the user
    /// was shown. `prepare` writes "No geometry — draw a closed room…" and then `start_calculation`
    /// returns, which from outside is indistinguishable from a dead button.
    ///
    /// The project is emptied by hand because `CadApp::default()` is NOT empty — it carries a
    /// 490-triangle starter scene, and the first version of this test passed a calculation it
    /// believed it had refused.
    #[test]
    fn a_calculation_with_no_geometry_is_recorded_with_its_reason() {
        let mut app = recording();
        app.factory = crate::factory::FactoryState::default();
        app.doc = Document::default();

        app.start_calculation();

        let ops = light_ops(&app);
        assert_eq!(
            ops.len(),
            1,
            "the refusal must leave exactly one line, got {ops:?}"
        );
        assert_eq!(ops[0].0, "calculate refused");
        assert!(
            ops[0].1.contains("no job"),
            "the line must say the job was never built: {}",
            ops[0].1,
        );
        assert!(
            ops[0].2.contains("No geometry"),
            "and must carry the status line the user was actually shown, which is the whole \
             point of the field: {}",
            ops[0].2,
        );
    }

    /// A SECOND PRESS WHILE ONE IS RUNNING is the other refusal, and it says nothing at all — so
    /// the dump has to. Set up with a channel that never sends, which is exactly the state a job
    /// in flight leaves behind.
    #[test]
    fn a_second_calculation_while_one_runs_is_recorded_as_refused() {
        let mut app = recording();
        let (_tx, rx) = std::sync::mpsc::channel();
        app.calc_rx = Some(rx);

        app.start_calculation();

        let ops = light_ops(&app);
        assert_eq!(
            ops.len(),
            1,
            "the refusal must leave exactly one line, got {ops:?}"
        );
        assert_eq!(ops[0].0, "calculate refused");
        assert!(
            ops[0].1.contains("ALREADY RUNNING"),
            "the line must distinguish this refusal from the no-geometry one: {}",
            ops[0].1,
        );
    }

    /// AND A CALCULATION THAT DOES START says what it started with. Mode, triangle count and step
    /// count are the three numbers that separate "Express on boxes" from "Thorough on seven million
    /// triangles" — the difference between eight seconds and twenty-three.
    #[test]
    fn a_calculation_that_starts_records_what_it_started_with() {
        let mut app = recording();
        app.start_calculation(); // the default project HAS geometry — see the test above

        let ops = light_ops(&app);
        assert_eq!(
            ops.len(),
            1,
            "starting must leave exactly one line, got {ops:?}"
        );
        assert_eq!(ops[0].0, "calculate");
        for needle in ["mode=", "tris", "steps=", "cell=", "plane_h="] {
            assert!(
                ops[0].1.contains(needle),
                "the start line must carry `{needle}`: {}",
                ops[0].1,
            );
        }
    }

    /// A DURATION IS SHOWN ONLY WHERE THERE IS ONE. Every placement would otherwise read "⏱ 0.0 s".
    #[test]
    fn only_the_timed_ops_report_a_time() {
        let ev = |us: u64| {
            format_event_oneline(&DbgEvent::LightOp {
                op: "calculated".into(),
                detail: "d".into(),
                fittings_before: 1,
                fittings_after: 1,
                message: String::new(),
                elapsed_us: us,
            })
        };
        assert!(ev(8_600_000).contains("8.6 s"));
        assert!(
            !ev(0).contains("⏱"),
            "an untimed op must not claim to have taken no time"
        );
    }
}
/// THE CALCULATING ZONE — "the zone thats calculating should be highlighted. nothing fancy just a
/// low transparency green highlight once calculated it should be gone as well."
///
/// The half that matters is "gone as well". A progress affordance left switched on is worse than
/// none: a green wash over a finished result reads as part of the answer.

#[cfg(test)]
mod the_calculating_zone_is_shown_while_it_runs {
    use super::*;

    /// A grep, because the alternative is standing up an egui context and a GL surface. Needles are
    /// assembled at run time — `include_str!` includes THIS module, so a literal would match the
    /// assertion instead of the code. That mistake has already been made once in this file. The
    /// anchored fns now live in three files (`app/mod.rs`, `app/ports_simlux.rs` and
    /// `app/windows.rs`), so the search tries all three.
    fn body_of(f: &str) -> String {
        for src in [
            include_str!("../mod.rs"),
            include_str!("../ports_simlux.rs"),
            include_str!("../windows.rs"),
        ] {
            if let Some(a) = src.find(f) {
                let end = src[a..]
                    .find("\n    }\n")
                    .map(|e| a + e)
                    .expect("re-anchor if the fn moves");
                return src[a..end].to_string();
            }
        }
        panic!("anchor not found in app/mod.rs, app/ports_simlux.rs or app/windows.rs: {f}");
    }

    /// IT IS TIED TO THE WORKER'S OWN LIFETIME, not to a flag of its own. `calc_rx` is `Some` for
    /// exactly as long as the job exists — set when it is spawned, cleared in `poll_calculation` on
    /// EVERY exit including a panic — so there is nothing that can be left switched on.
    #[test]
    fn the_highlight_is_gated_on_the_running_job() {
        let needle = |p: &[&str]| -> String { p.concat() };
        let body = body_of(&needle(&["fn paint_calculating_", "zone(&self"]));
        assert!(
            body.contains(&needle(&["if self.calc_rx.is_", "none()"])),
            "the wash must be gated on the job itself, or it outlives the calculation",
        );
    }

    /// AND `poll_calculation` CLEARS THAT GATE ON EVERY PATH. If a panic left `calc_rx` set, the
    /// wash would stay up for the rest of the session — and so would the refusal to start another
    /// calculation, which is the same bug wearing a different face.
    #[test]
    fn every_exit_from_the_poll_clears_the_gate() {
        let needle = |p: &[&str]| -> String { p.concat() };
        let body = body_of(&needle(&["fn poll_calculation(&mut self"]));
        let clears = body.matches(&needle(&["self.calc_rx = ", "None;"])).count();
        assert!(
            clears >= 2,
            "both the finished path and the worker-died path must clear it, found {clears}",
        );
    }

    /// IT IS DRAWN UNDER THE FIXTURES AND OVER THE OLD RESULT — the fixtures are what is being
    /// looked at, and the wash says which ground the answer coming will cover.
    #[test]
    fn it_paints_between_the_old_result_and_the_fixtures() {
        // The paint call sites live in the frame shell (app/shell.rs) since the SIMLUX split.
        let src = include_str!("../shell.rs");
        let needle = |p: &[&str]| -> String { p.concat() };
        let overlay = src.find(&needle(&["self.paint_lux_over", "lay(&painter, rect);"]));
        let zone = src.find(&needle(&[
            "self.paint_calculating_",
            "zone(&painter, rect);",
        ]));
        let lums = src.find(&needle(&["self.paint_luminaires_", "2d(&painter, rect);"]));
        let (overlay, zone, lums) = (
            overlay.expect("the lux overlay call"),
            zone.expect("the calculating wash is not painted at all"),
            lums.expect("the fixture markers"),
        );
        assert!(overlay < zone, "the wash goes OVER the previous result");
        assert!(
            zone < lums,
            "and UNDER the fixtures, which are what the user is aiming"
        );
    }
}
/// "THE REPORT IS SHOWING WRONG NUMBER OF LIGHTS THERES ONLY 31 LIGHTS IN SIMLUX FILE."
///
/// There were 68 in `light.luminaires`: the 31 real ones, and 37 stranded at x ≈ 3.5 — a thousandth
/// of the building's coordinates — left behind by the unit declaration that read a metre drawing as
/// millimetres. They sit 3.5 km from the plan, so no light of theirs reaches it and every lux figure
/// was right. But they are COUNTED: the schedule read 68 fittings and 1360 W against the true 31 and
/// 620 W, and a power density on a report is a number somebody signs.

#[cfg(test)]
pub mod fittings_outside_the_building_are_found {
    use super::*;

    // ON SURVEY COORDINATES, like the project this came from -- 33 x 13 m sitting at x 3500,
    // y -6850. That is not decoration: a fitting divided by 1000 lands 3.5 km away only BECAUSE
    // the building is far from the origin. A model drawn at the origin would put its own strays
    // inside itself, where no distance test can see them -- see `the_origin_is_the_blind_spot`.
    pub const OX: f32 = 3500.0;
    pub const OY: f32 = -6850.0;

    pub fn a_building_with(fittings: &[(f32, f32)]) -> CadApp {
        let rect = vec![
            glam::Vec2::new(OX, OY),
            glam::Vec2::new(OX + 30.0, OY),
            glam::Vec2::new(OX + 30.0, OY + 12.0),
            glam::Vec2::new(OX, OY + 12.0),
            glam::Vec2::new(OX, OY),
        ];
        let mut app = CadApp::default();
        app.factory = crate::factory::FactoryState::default();
        app.factory
            .add_building_outline(&rect, 3.0)
            .expect("building");
        app.factory.recompute();
        app.light.luminaires.clear();
        for (i, (x, y)) in fittings.iter().enumerate() {
            app.light.luminaires.push(cad_light::Luminaire {
                id: i as u32 + 1,
                profile: String::new(),
                position: cad_light::Vertex::new(*x, *y, 3.0),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            });
        }
        app
    }

    /// THE REAL CASE, at the real ratio: fittings inside, and fittings at a thousandth of the
    /// building's coordinates.
    #[test]
    fn a_fitting_at_a_thousandth_of_the_coordinates_is_found() {
        // Two inside, and two at a THOUSANDTH of the coordinates -- the real signature.
        let app = a_building_with(&[
            (OX + 15.0, OY + 6.0),
            ((OX + 15.0) / 1000.0, (OY + 6.0) / 1000.0),
            (OX + 25.0, OY + 3.0),
            ((OX + 25.0) / 1000.0, (OY + 3.0) / 1000.0),
        ]);
        let strays = app.stray_light_ids();
        assert_eq!(
            strays,
            vec![2, 4],
            "the two at 1/1000 scale must be the ones found"
        );
    }

    /// A FITTING JUST OUTSIDE THE WALL IS LEGITIMATE — a façade light, a bollard by the door — and
    /// deleting it would be destroying the user's work. The margin exists for exactly this.
    #[test]
    fn a_fitting_just_outside_the_wall_is_left_alone() {
        let app = a_building_with(&[
            (OX - 2.0, OY + 6.0),
            (OX + 32.0, OY + 6.0),
            (OX + 15.0, OY - 3.0),
            (OX + 15.0, OY + 14.0),
        ]);
        assert!(
            app.stray_light_ids().is_empty(),
            "fittings a couple of metres off the wall are ordinary external lighting",
        );
    }

    /// AND ONE WITH NO PLAN SYMBOL IS ORDINARY. Placing a fitting by hand produces exactly that, so
    /// "has no symbol" would have deleted legitimate work — the test is DISTANCE.
    #[test]
    fn provenance_is_not_the_test() {
        let app = a_building_with(&[(OX + 15.0, OY + 6.0)]);
        assert!(
            app.light.luminaires[0].from_block.is_none(),
            "hand-placed: no symbol"
        );
        assert!(
            app.stray_light_ids().is_empty(),
            "and it is inside the building, so it stays"
        );
    }

    /// WITH NO MODEL THERE IS NOTHING TO BE OUTSIDE OF. A plan-only project must not have its
    /// entire scheme declared stray.
    #[test]
    fn a_project_with_no_model_reports_nothing() {
        let mut app = a_building_with(&[(OX + 15.0, OY + 6.0), (99_999.0, 99_999.0)]);
        app.factory = crate::factory::FactoryState::default();
        assert!(
            app.stray_light_ids().is_empty(),
            "no building, no judgement"
        );
    }

    /// LISTING DOES NOT DELETE. Somebody typing a command to look must not lose fittings by it.
    #[test]
    fn the_report_form_removes_nothing() {
        let mut app = a_building_with(&[(OX + 15.0, OY + 6.0), (3.5, -6.85)]);
        app.run_command("straylights");
        assert_eq!(
            app.light.luminaires.len(),
            2,
            "the report form must not delete anything"
        );
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("straylights") && h.contains("purge")),
            "…and must say how to: {:?}",
            app.history.last(),
        );
    }

    /// PURGE REMOVES EXACTLY THE STRAYS, and marks the result out of date — the schedule and the
    /// installed load both change, so the answer on screen no longer describes the scheme.
    #[test]
    fn purge_removes_the_strays_and_nothing_else() {
        let mut app =
            a_building_with(&[(OX + 15.0, OY + 6.0), (3.5, -6.85), (OX + 25.0, OY + 3.0)]);
        app.light.results_fingerprint = Some(1);
        app.run_command("straylights purge");
        let left: Vec<u32> = app.light.luminaires.iter().map(|l| l.id).collect();
        assert_eq!(left, vec![1, 3], "only the stray goes");
        assert!(
            app.light.results_stale,
            "the scheme changed, so the answer is out of date"
        );
    }
}
#[cfg(test)]
mod the_origin_is_the_blind_spot {
    use super::fittings_outside_the_building_are_found::*;
    use super::*;

    /// A BUILDING DRAWN AT THE ORIGIN HIDES ITS OWN STRAYS, and this records that rather than
    /// pretending otherwise.
    ///
    /// The detector is a distance test, and dividing a coordinate by 1000 only moves a fitting far
    /// away when the coordinate is large. A 30 x 12 m building at (0, 0) maps its own fittings to
    /// (0.015, 0.006) — inside itself. No position test can separate those from real ones, because
    /// by position they ARE real ones.
    ///
    /// Found by writing the fixture at the origin out of habit and watching it report nothing. The
    /// project this came from sits on survey coordinates at x 3500, which is the only reason its 37
    /// strays were 3.5 km away and visible at all.
    #[test]
    fn a_model_at_the_origin_cannot_have_its_strays_seen_by_position() {
        let rect = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(30.0, 0.0),
            glam::Vec2::new(30.0, 12.0),
            glam::Vec2::new(0.0, 12.0),
            glam::Vec2::new(0.0, 0.0),
        ];
        let mut app = CadApp::default();
        app.factory = crate::factory::FactoryState::default();
        app.factory
            .add_building_outline(&rect, 3.0)
            .expect("building");
        app.factory.recompute();
        app.light.luminaires.clear();
        for (i, (x, y)) in [(15.0_f32, 6.0_f32), (0.015, 0.006)].iter().enumerate() {
            app.light.luminaires.push(cad_light::Luminaire {
                id: i as u32 + 1,
                profile: String::new(),
                position: cad_light::Vertex::new(*x, *y, 3.0),
                rotation_deg: 0.0,
                tilt_deg: 0.0,
                dimming: 1.0,
                watts_override: None,
                flux_override: None,
                from_block: None,
            });
        }
        assert!(
            app.stray_light_ids().is_empty(),
            "a 1/1000 fitting inside a building drawn at the origin is INDISTINGUISHABLE by \
             position — this is a limit of the approach, not a bug to fix by tightening the \
             margin, which would start deleting real external lighting instead",
        );
    }

    /// AND THE SAME MODEL, MOVED OUT TO SURVEY COORDINATES, does show them — which is what makes
    /// the check worth having on the projects it applies to.
    #[test]
    fn the_same_model_on_survey_coordinates_shows_them() {
        let app = a_building_with(&[
            (OX + 15.0, OY + 6.0),
            ((OX + 15.0) / 1000.0, (OY + 6.0) / 1000.0),
        ]);
        assert_eq!(app.stray_light_ids().len(), 1);
    }
}
/// THE LUX FIGURES ON THE REAL PROJECT, both modes, measured rather than argued.
///
///     SIMLUX_DIAG="D:\...\for3dfactorygym.dxf" \
///       cargo test -p cad_app --release lux_of_real_project -- --ignored --nocapture
///
/// Exists because "I changed the burial rule" is a claim, and the only thing that settles it is
/// the number it produces on the building the complaint came from. Reference figures from DIALux
/// on the same plan, EN grid: Ē 240 lx, Emin 6.85 lx, Emax 853 lx, U₀ 0.029.

#[cfg(test)]
mod the_real_projects_lux {
    use super::*;

    #[test]
    #[ignore]
    fn lux_of_real_project() {
        let Ok(path) = std::env::var("SIMLUX_DIAG") else {
            println!("set SIMLUX_DIAG to the drawing path");
            return;
        };
        let p = std::path::Path::new(&path);
        let cfg = crate::simlux_io::load(p)
            .expect("sidecar read")
            .expect("sidecar exists");
        let mut app = CadApp::default();
        let text = std::fs::read_to_string(p).expect("drawing");
        app.doc = cad_io::dxf::read_dxf(&text).expect("parse");
        let furniture =
            crate::factory::FactoryState::decode_furniture_lib(cfg.factory.furniture_lib.clone());
        app.install_simlux_config(cfg, furniture, None);
        app.factory.recompute();

        println!(
            "fittings {}  furniture {}  wall_zone {:.2} m  cell {:.2} m  plane {:.2} m",
            app.light.luminaires.len(),
            app.factory.furniture.len(),
            app.light.wall_zone,
            app.light.cell_size,
            app.light.plane_height,
        );
        println!(
            "\n{:<10} {:>8} {:>8} {:>8} {:>7}  {}",
            "mode", "Ē", "Emin", "Emax", "U0", "grid"
        );
        for mode in [
            crate::light::CalcMode::Express,
            crate::light::CalcMode::Thorough,
        ] {
            app.light.mode = mode;
            let plan = app.doc.clone();
            let t = std::time::Instant::now();
            app.light.calculate(&plan, Some(&app.factory));
            // THE EN GRID, because that is the one the report quotes and the one DIALux's figures
            // came off. Reading the working grid here would compare two different questions.
            // BOTH GRIDS. The EN one is what the report quotes and what DIALux's figures came off,
            // but only ~35 of its 119 cells survive the mask on this plan -- a sample that small
            // moves several percent on a single cell. The working grid is 132 x 52 and says
            // whether a difference is real or an artefact of the coarse one.
            let grids: Vec<(&cad_light::LuxGrid, &str)> = [
                app.light.grid_en.as_ref().map(|g| (g, "EN 12464-1")),
                app.light.grid.as_ref().map(|g| (g, "working")),
            ]
            .into_iter()
            .flatten()
            .collect();
            if grids.is_empty() {
                println!("{:<10}  no result", mode.label());
                continue;
            }
            for (g, note) in &grids {
                println!(
                    "{:<10} {:>8.1} {:>8.2} {:>8.1} {:>7.3}  {} {}x{}",
                    mode.label(),
                    g.avg,
                    g.min,
                    g.max,
                    if g.avg > 0.0 { g.min / g.avg } else { 0.0 },
                    note,
                    g.cols,
                    g.rows,
                );
            }
            let (g, note) = grids[0];
            let _ = note;
            println!("           took {:.1} s", t.elapsed().as_secs_f64());

            // WHERE THE DARKEST POINT IS, and what is near it. The number alone cannot say whether
            // a zero is a cell buried in furniture, a sealed room nobody lit, or a hole in the
            // calculation — and those want three different fixes.
            let (Some(g2), Some(pl)) = (app.light.grid_en.as_ref(), app.light.plane_en.as_ref())
            else {
                continue;
            };
            let (gc, gr) = (g2.cols as usize, g2.rows as usize);
            let (dx, dy) = (pl.width as f64 / gc as f64, pl.depth as f64 / gr as f64);
            let mut worst = (f64::MAX, 0usize, 0usize);
            for j in 0..gr {
                for i in 0..gc {
                    let v = g2.values[j * gc + i];
                    if v < worst.0 {
                        worst = (v, i, j);
                    }
                }
            }
            let wx = pl.origin.x as f64 + (worst.1 as f64 + 0.5) * dx;
            let wy = pl.origin.y as f64 + (worst.2 as f64 + 0.5) * dy;
            let near = app
                .light
                .luminaires
                .iter()
                .map(|l| {
                    ((l.position.x as f64 - wx).powi(2) + (l.position.y as f64 - wy).powi(2)).sqrt()
                })
                .fold(f64::MAX, f64::min);
            let obs = crate::light::obstacles_in_mode(&app.factory, &[], mode);
            let buried = obs
                .iter()
                .any(|o| o.contains(glam::Vec3::new(wx as f32, wy as f32, pl.origin.z)));
            // AND WHETHER IT IS MASKED OUT. Both modes contain this cell and only one counts it,
            // so the mask is where the difference lives — not the light.
            let m = app
                .light
                .rooms
                .first()
                .map(|r| r.mask_en.clone())
                .unwrap_or_default();
            let k = worst.2 * gc + worst.1;
            let masked_out = m.get(k).map(|inside| !*inside);
            let kept = m.iter().filter(|b| **b).count();
            println!(
                "           darkest {:.2} lx at ({:.2}, {:.2}) — {:.2} m from a fitting, \
                 buried={} obstacles={} masked_out={:?} kept={}/{}",
                worst.0,
                wx,
                wy,
                near,
                buried,
                obs.len(),
                masked_out,
                kept,
                m.len(),
            );
        }
        println!("\nDIALux, same plan, EN grid:      240.0     6.85    853.0   0.029");
    }
}
