use super::super::*;

#[cfg(test)]
mod cascade_tests {
    use super::*;
    use glam::{Mat4, Vec3};

    /// A camera looking across a villa-sized site, and the bounds of that site.
    fn setup() -> ([f32; 16], Vec3, (Vec3, Vec3), Vec3) {
        let eye = Vec3::new(0.0, -40.0, 12.0);
        let proj = Mat4::perspective_rh_gl(0.9, 16.0 / 9.0, 0.1, 500.0);
        let view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Z);
        let bounds = (Vec3::new(-50.0, -50.0, 0.0), Vec3::new(50.0, 50.0, 12.0));
        (
            (proj * view).to_cols_array(),
            eye,
            bounds,
            Vec3::new(0.3, 0.4, 0.87).normalize(),
        )
    }

    /// How much WORLD a single shadow texel covers, in metres — the number the whole feature is
    /// about. Recovered from the matrix rather than from the fit, so it measures what the shader
    /// will actually sample.
    fn metres_per_texel(m: &[f32; 16], map_size: i32) -> f32 {
        let inv = Mat4::from_cols_array(m).inverse();
        // Two light-space points one NDC unit apart map to half the map's width in world.
        let a = inv.project_point3(Vec3::new(-1.0, 0.0, 0.0));
        let b = inv.project_point3(Vec3::new(1.0, 0.0, 0.0));
        (b - a).length() / map_size as f32
    }

    /// The near cascade must be sharper than the far one. That is the entire point: one map over
    /// a whole site spends nearly all of its texels on ground nobody is looking at.
    #[test]
    fn the_near_cascade_is_sharper_than_the_far_one() {
        let (mvp, eye, bounds, sun) = setup();
        let c = sun_cascades(&mvp, eye, bounds, sun, 3, crate::light3d::SHADOW_MAP_SIZE);
        assert_eq!(c.len(), 3, "three cascades were asked for");
        let m: Vec<f32> = c
            .iter()
            .map(|x| metres_per_texel(x, crate::light3d::SHADOW_MAP_SIZE))
            .collect();
        assert!(
            m[0] < m[1] && m[1] < m[2],
            "cascades are not ordered tightest-first: {m:?}"
        );
        // …and the first must be a real improvement on the single whole-scene map it replaces.
        let one = sun_light_matrix(bounds.0, bounds.1, sun);
        let whole = metres_per_texel(&one, crate::light3d::SHADOW_MAP_SIZE);
        assert!(
            m[0] < whole * 0.35,
            "the near cascade is {:.1} mm/texel against the single map's {:.1} mm — barely worth the pass",
            m[0] * 1000.0,
            whole * 1000.0
        );
    }

    /// Asking for ONE cascade must still work, and must cover the scene.
    ///
    /// This is the fallback for a slow machine, and a "1" that quietly shadowed nothing would be
    /// worse than no setting at all.
    #[test]
    fn a_single_cascade_still_covers_the_view() {
        let (mvp, eye, bounds, sun) = setup();
        let c = sun_cascades(&mvp, eye, bounds, sun, 1, crate::light3d::SHADOW_MAP_SIZE);
        assert_eq!(c.len(), 1);
        // A point in the middle of the site must land inside the map.
        let p = Mat4::from_cols_array(&c[0]) * Vec3::new(0.0, 0.0, 1.0).extend(1.0);
        let n = p.truncate() / p.w;
        assert!(
            n.x.abs() <= 1.0 && n.y.abs() <= 1.0 && n.z.abs() <= 1.0,
            "the scene centre fell outside its own shadow map at {n:?}"
        );
    }

    /// Turning the camera on the spot must not change the cascades' SIZE.
    ///
    /// The slices are bounded by spheres for exactly this reason. A box fitted to the slice
    /// corners changes shape as the camera rotates, so every shadow in the scene would pulse while
    /// the user orbited — far more distracting than a slightly larger map.
    #[test]
    fn orbiting_does_not_resize_the_cascades() {
        let eye = Vec3::new(0.0, -40.0, 12.0);
        let bounds = (Vec3::new(-50.0, -50.0, 0.0), Vec3::new(50.0, 50.0, 12.0));
        let sun = Vec3::new(0.3, 0.4, 0.87).normalize();
        let proj = Mat4::perspective_rh_gl(0.9, 16.0 / 9.0, 0.1, 500.0);
        let size = |target: Vec3| {
            let mvp = (proj * Mat4::look_at_rh(eye, target, Vec3::Z)).to_cols_array();
            let c = sun_cascades(&mvp, eye, bounds, sun, 3, crate::light3d::SHADOW_MAP_SIZE);
            metres_per_texel(&c[0], crate::light3d::SHADOW_MAP_SIZE)
        };
        let a = size(Vec3::ZERO);
        let b = size(Vec3::new(30.0, 10.0, 0.0));
        assert!(
            (a - b).abs() < a * 0.02,
            "the near cascade went from {a:.4} to {b:.4} m/texel just by turning the camera"
        );
    }

    /// The fit must SNAP to whole texels, or the map slides continuously and every shadow edge in
    /// the scene crawls as the camera moves — which reads as the renderer being unstable.
    #[test]
    fn the_cascades_snap_to_their_own_texel_grid() {
        let sun = Vec3::new(0.3, 0.4, 0.87).normalize();
        let map = 2048;
        let r = 20.0f32;
        let unit = 2.0 * (r * 1.1) / map as f32; // one texel, in world metres
                                                 // Where a FIXED world point lands in the map, measured in texels. This is the thing that
                                                 // must not move: if it slides, the depth stored for that point changes every frame and the
                                                 // shadow's edge crawls. (Not matrix equality — the light-space round trip is a rotation
                                                 // and back, so the numbers wobble in the last decimal no matter how well it snaps.)
        let probe = Vec3::new(5.0, 3.0, 1.0);
        let texel_x = |m: &[f32; 16]| {
            let c = Mat4::from_cols_array(m) * probe.extend(1.0);
            let n = c.truncate() / c.w;
            n.x * 0.5 * map as f32
        };
        let a = sun_cascade_matrix(Vec3::new(5.0, 3.0, 1.0), r, sun, map);
        let b = sun_cascade_matrix(Vec3::new(5.0 + unit * 0.05, 3.0, 1.0), r, sun, map);
        assert!(
            (texel_x(&a) - texel_x(&b)).abs() < 0.05,
            "a twentieth-of-a-texel nudge slid the map by {:.3} texels",
            (texel_x(&a) - texel_x(&b)).abs()
        );
        // …and a nudge of several whole texels DOES move it, or the snapping is really a freeze.
        let c = sun_cascade_matrix(Vec3::new(5.0 + unit * 40.0, 3.0, 1.0), r, sun, map);
        assert!(
            (texel_x(&a) - texel_x(&c)).abs() > 1.0,
            "the map never moves at all — the fit is stuck rather than snapped"
        );
    }
}
#[cfg(test)]
mod snapshot_scaling_tests {
    use super::*;
    use crate::dbg_recorder::SNAP_GEOM_MAX;

    /// Benchmark, not an assertion — `cargo test -p cad_app -- --ignored --nocapture`.
    /// Proves snapshot cost is FLAT in drawing size (1k/100k/1M all ≈ the same µs and
    /// the same ~1.6 KB dump).
    #[test]
    #[ignore = "benchmark: run with --ignored --nocapture"]
    fn measure_snapshot_cost() {
        for n in [1_000usize, 100_000, 1_000_000] {
            let mut app = app_with(n);
            let t = std::time::Instant::now();
            snap(&mut app, "m");
            let us = t.elapsed().as_micros();
            let dump = app.dbg.dump_text();
            println!(
                "  {n:>9} dobjects → snapshot {us:>6} µs · dump {:>6} bytes",
                dump.len()
            );
        }
    }

    fn snap(app: &mut CadApp, tag: &str) {
        app.dbg.take_snapshot(
            CadApp::plan_doc_of(app.factory.session.as_ref(), &app.doc),
            tag,
            0,
            0,
            describe_verbose,
            std::panic::Location::caller(),
        );
    }

    fn app_with(n: usize) -> CadApp {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        for i in 0..n {
            app.doc
                .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                    cad_kernel::Line {
                        a: Vec2::new(i as f64, 0.0),
                        b: Vec2::new(i as f64, 1.0),
                    },
                )));
        }
        app.dbg.recording = true;
        app.dbg.session_started = Some(std::time::Instant::now());
        app
    }

    /// The dump keeps the first 20 dobjects' geometry — enough for draw-function
    /// tests — and NEVER more, whatever the drawing size.
    #[test]
    fn snapshot_caps_geometry_at_twenty() {
        let mut app = app_with(1_000);
        snap(&mut app, "test");
        let snap = app.dbg.snapshots.last().unwrap();
        assert_eq!(snap.geom.len(), SNAP_GEOM_MAX, "capped at {SNAP_GEOM_MAX}");
        assert_eq!(snap.omitted, 980, "and it SAYS how many it dropped");
        let dump = app.dbg.dump_text();
        assert!(
            dump.contains("980 MORE OMITTED"),
            "a capped dump must never look complete"
        );
        assert!(
            dump.contains("line  a=(0.000,0.000)"),
            "real coordinates, for draw tests"
        );
    }

    /// Small drawings are fully captured, with no omission notice.
    #[test]
    fn small_drawings_are_captured_whole() {
        let mut app = app_with(3);
        snap(&mut app, "test");
        let snap = app.dbg.snapshots.last().unwrap();
        assert_eq!(snap.geom.len(), 3);
        assert_eq!(snap.omitted, 0);
        assert!(!app.dbg.dump_text().contains("OMITTED"));
    }

    /// ⭐ THE PERFORMANCE CONTRACT. Snapshotting must be O(1) in the drawing size —
    /// it used to clone the whole Document (a clone nothing ever read), so an
    /// auto-snap every 50 events was O(n) time AND memory. At a million dobjects that
    /// is a freeze, not a big file.
    ///
    /// Asserts the SHAPE, not a wall-clock: 100k dobjects must cost no more captured
    /// geometry than 100 do. A restored clone would fail the memory claim silently, so
    /// we also assert the type no longer carries a Document.
    #[test]
    fn snapshotting_is_o1_in_drawing_size() {
        let mut small = app_with(100);
        snap(&mut small, "s");
        let mut huge = app_with(100_000);
        snap(&mut huge, "h");

        let a = small.dbg.snapshots.last().unwrap();
        let b = huge.dbg.snapshots.last().unwrap();
        assert_eq!(a.geom.len(), SNAP_GEOM_MAX);
        assert_eq!(
            b.geom.len(),
            SNAP_GEOM_MAX,
            "a 1000× bigger drawing captures the SAME amount"
        );
        assert_eq!(b.omitted, 100_000 - SNAP_GEOM_MAX);

        // the dump must stay small too — it is pasted into bug reports
        let dump = huge.dbg.dump_text();
        assert!(
            dump.len() < 8_000,
            "a 100k-dobject dump must stay readable, got {} bytes",
            dump.len()
        );
    }
}
#[cfg(test)]
mod counts_only_tests {
    use super::*;
    use crate::dbg_recorder::DbgEvent;

    fn rec() -> crate::dbg_recorder::DbgRecorder {
        let mut r = crate::dbg_recorder::DbgRecorder::default();
        r.recording = true;
        r.session_started = Some(std::time::Instant::now());
        r
    }

    /// Measures the win at the user's real numbers (916 selected, and 1M).
    #[test]
    #[ignore = "benchmark: --ignored --nocapture"]
    fn measure_dump_size() {
        for n in [916usize, 100_000, 1_000_000] {
            let mut a = rec();
            a.counts_only = false;
            a.push(sel_change(n), std::panic::Location::caller());
            let mut b = rec();
            b.counts_only = true;
            b.push(sel_change(n), std::panic::Location::caller());
            println!(
                "  {n:>9} selected → verbose {:>9} bytes · counts-only {:>4} bytes",
                a.dump_text().len(),
                b.dump_text().len()
            );
        }
    }

    fn sel_change(n: usize) -> DbgEvent {
        DbgEvent::SelectChange {
            basket_before: vec![],
            basket_after: (0..n).collect(),
            n_before: 0,
            n_after: 0,
            cause: "window".into(),
        }
    }

    /// Count-only keeps the NUMBER and drops the list — at CAPTURE, so the recorder's
    /// own memory is bounded too, not just the dump text.
    #[test]
    fn counts_only_drops_the_list_but_keeps_the_number() {
        let mut r = rec();
        r.counts_only = true;
        r.push(sel_change(50_000), std::panic::Location::caller());
        match &r.events[0].event {
            DbgEvent::SelectChange {
                basket_after,
                n_after,
                ..
            } => {
                assert!(
                    basket_after.is_empty(),
                    "the list must be dropped AT CAPTURE"
                );
                assert_eq!(*n_after, 50_000, "…but the count survives");
            }
            _ => panic!("wrong event"),
        }
        let dump = r.dump_text();
        assert!(
            dump.contains("✓ SEL 0 → 50000 dobject(s)"),
            "counts, got: {dump}"
        );
        // (the dump's TIMESTAMPS use brackets — check the SEL line itself)
        let sel_line = dump.lines().find(|l| l.contains("✓ SEL")).unwrap();
        assert!(
            !sel_line.contains("[0,"),
            "no index list on the SEL line: {sel_line}"
        );
        assert!(
            dump.len() < 400,
            "and the whole dump stays tiny: {} bytes",
            dump.len()
        );
    }

    /// Off by default → the full lists, exactly as before. This is a debugging aid;
    /// it must not silently change what a normal session records.
    #[test]
    fn off_by_default_keeps_the_full_list() {
        let mut r = rec();
        assert!(!r.counts_only, "must default OFF");
        r.push(sel_change(3), std::panic::Location::caller());
        match &r.events[0].event {
            DbgEvent::SelectChange {
                basket_after,
                n_after,
                ..
            } => {
                assert_eq!(basket_after.len(), 3, "lists preserved when off");
                assert_eq!(*n_after, 3, "count stamped either way");
            }
            _ => panic!("wrong event"),
        }
        assert!(r.dump_text().contains("[0, 1, 2]"));
    }

    /// ⭐ THE POINT. The user's 1.2M-dobject dump was too big to paste — the second
    /// half was truncated away. A 916-selection gesture prints its list TWICE.
    /// Count-only must make dump size independent of selection size.
    #[test]
    fn dump_size_stops_scaling_with_the_selection() {
        let big = |counts_only: bool| {
            let mut r = rec();
            r.counts_only = counts_only;
            r.push(sel_change(100_000), std::panic::Location::caller());
            r.dump_text().len()
        };
        let verbose = big(false);
        let counted = big(true);
        assert!(
            counted < 400,
            "counts-only dump must stay tiny, got {counted} bytes"
        );
        assert!(
            verbose > 100 * counted,
            "the verbose dump should dwarf it ({verbose} vs {counted})"
        );
    }
}
/// 3D SNAP, and re-placing something that already exists.
///
/// Reported as: "when a furniture is placed inside a building the placement mechanism isnt working
/// … how is the snapping affecting a furniture when being placed[?] where is the option to turn
/// on/off[?]"

#[cfg(test)]
mod snap_and_replace {
    use super::*;

    use crate::factory::FactoryState;
    use cad_solid::{BoolOp, Placement, Plane, Primitive};
    use glam::{Mat4, Vec3};

    fn app_in_3d() -> CadApp {
        let mut app = CadApp::default();
        app.factory.open = true;
        app.active_view = ActiveView::ThreeD;
        app
    }

    /// THE BUG IN THE SESSION. A cupboard was built (landing on the origin), `place` was typed to
    /// switch to click-placement, and the click went through the SELECTION path because nothing was
    /// waiting — the mode only ever governed the NEXT object added.
    #[test]
    fn choosing_click_re_places_what_is_already_selected() {
        let mut app = app_in_3d();
        app.factory.place_mode = crate::factory::PlaceMode::Origin; // so the add does not arm
        let mesh = crate::mesh_io::ObjMesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            color: None,
            alpha: Vec::new(),
        };
        let idx = app.factory.add_furniture_asset("Cupboard".into(), mesh);
        let at = app.factory.place_at();
        app.factory.place_furniture(idx, at);
        assert!(
            app.factory.awaiting_place.is_none(),
            "precondition: nothing is waiting"
        );

        app.run_command("place");
        app.run_command("click");
        assert_eq!(
            app.factory.awaiting_place,
            Some(crate::factory::AwaitingPlace::Furniture(0)),
            "the selected piece must now be waiting for a click",
        );
    }

    /// With nothing selected it just sets the mode, as before — the re-place is a convenience, not
    /// a precondition.
    #[test]
    fn choosing_click_with_nothing_selected_only_sets_the_mode() {
        let mut app = app_in_3d();
        app.factory.place_mode = crate::factory::PlaceMode::Origin;
        app.run_command("place");
        app.run_command("click");
        assert_eq!(app.factory.place_mode, crate::factory::PlaceMode::Click);
        assert!(app.factory.awaiting_place.is_none());
    }

    // ---- the snap ------------------------------------------------------------------------

    fn a_building() -> FactoryState {
        let mut st = FactoryState::default();
        st.model.push(
            BoolOp::Union,
            Plane::default(),
            Placement::default(),
            Primitive::Box {
                w: 10.0,
                d: 10.0,
                h: 3.0,
            },
        );
        st.recompute();
        st.cam_dist = 20.0;
        st
    }

    fn mvp_of(st: &FactoryState, rect: egui::Rect) -> [f32; 16] {
        crate::light3d::mvp(
            st.cam_yaw,
            st.cam_pitch,
            st.cam_dist,
            st.cam_target,
            rect.width() / rect.height(),
            st.ortho,
        )
    }

    /// Screen position of a world point under `mvp`.
    fn to_screen(w: Vec3, mvp: &[f32; 16], rect: egui::Rect) -> egui::Pos2 {
        let n = Mat4::from_cols_array(mvp).project_point3(w);
        egui::pos2(
            rect.left() + (n.x * 0.5 + 0.5) * rect.width(),
            rect.top() + (0.5 - n.y * 0.5) * rect.height(),
        )
    }

    /// THE TOGGLE — "where is the option to turn on/off".
    #[test]
    fn the_snap_can_be_turned_off() {
        let mut st = a_building();
        assert!(st.snap_3d, "on by default");
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = mvp_of(&st, rect);
        // A corner of the box, aimed at exactly.
        let corner = Vec3::new(5.0, 5.0, 3.0);
        let at = to_screen(corner, &mvp, rect);
        assert!(
            st.snap_vertex(at, rect, &mvp).is_some(),
            "it must catch its own corner"
        );

        st.snap_3d = false;
        assert!(
            st.snap_vertex(at, rect, &mvp).is_none(),
            "off must mean off"
        );
    }

    /// IT NEVER SNAPS TO SOMETHING YOU CANNOT SEE. Working inside a building means "hide ceilings"
    /// is on — and the roof slab is still in the mesh, so a click in the middle of a room could
    /// jump to a ceiling corner floating above it with nothing on screen to explain why.
    #[test]
    fn a_hidden_ceiling_is_not_snappable() {
        let mut st = FactoryState::default();
        // A room-sized box with a thin slab capping it: the slab is what `hide_ceilings` hides.
        st.model.push(
            BoolOp::Union,
            Plane::default(),
            Placement::default(),
            Primitive::Box {
                w: 10.0,
                d: 10.0,
                h: 3.0,
            },
        );
        st.model.push(
            BoolOp::Union,
            Plane::default(),
            Placement {
                lift: 3.0,
                ..Placement::default()
            },
            Primitive::Box {
                w: 10.0,
                d: 10.0,
                h: 0.2,
            },
        );
        st.recompute();
        st.cam_dist = 20.0;
        let caps: Vec<u32> = st
            .model
            .features
            .iter()
            .map(|f| f.id)
            .filter(|id| st.is_hidden_ceiling(*id))
            .collect();
        assert!(
            !caps.is_empty(),
            "precondition: the slab is detected as a ceiling cap"
        );

        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = mvp_of(&st, rect);
        // A corner of the CAP — the highest geometry there is.
        let cap_corner = Vec3::new(5.0, 5.0, 3.2);
        let at = to_screen(cap_corner, &mvp, rect);

        st.hide_ceilings = false;
        let visible = st.snap_vertex(at, rect, &mvp);
        assert!(
            visible.is_some(),
            "with the ceiling shown it is a legitimate target"
        );

        st.hide_ceilings = true;
        if let Some((w, _)) = st.snap_vertex(at, rect, &mvp) {
            assert!(
                w.z < 3.15,
                "snapped to the hidden ceiling at z = {:.2} — invisible geometry must not catch",
                w.z,
            );
        }
    }

    /// A click's Z is NOT used for furniture: the piece sits on the storey floor whatever the snap
    /// caught. Snapping to a vertex 3 m up a wall moves it in plan, not into the air.
    #[test]
    fn a_placing_click_never_lifts_furniture_off_the_floor() {
        let mut st = a_building();
        let mesh = crate::mesh_io::ObjMesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            color: None,
            alpha: Vec::new(),
        };
        let idx = st.add_furniture_asset("Cupboard".into(), mesh);
        st.place_furniture(idx, Vec3::ZERO);
        st.awaiting_place = Some(crate::factory::AwaitingPlace::Furniture(0));

        // A point 3 m up — what snapping to a wall-top corner would hand us.
        st.place_awaiting_at(Vec3::new(4.0, 4.0, 3.0));
        assert!(
            (st.furniture[0].pos[2] - st.active_base_z()).abs() < 1e-6,
            "the piece left the floor: z = {}",
            st.furniture[0].pos[2],
        );
        assert!(
            (st.furniture[0].pos[0] - 4.0).abs() < 1e-6,
            "…but it did move in plan"
        );
    }
}
#[cfg(test)]
mod hatch_cancel_worker_tests {
    use super::*;

    fn dummy_worker(app: &CadApp) -> HatchWorker {
        let (tx, rx) = mpsc::channel::<HatchWorkerResult>();
        let _ = tx; // receiver alive; sender dropped
        HatchWorker {
            seed: Vec2::ZERO,
            pattern: cad_kernel::HatchPattern::Solid,
            active_layer: app.doc.layers.active,
            cancel: StdArc::new(AtomicBool::new(false)),
            rx,
        }
    }

    #[test]
    fn cancel_hatch_worker_drops_receiver_and_sets_flag() {
        let mut app = CadApp::default();
        app.op_cancel.store(false, Ordering::Relaxed);
        app.hatch_worker = Some(dummy_worker(&app));
        let cancelled = app.cancel_hatch_worker();
        assert!(cancelled, "helper reports it cancelled a running worker");
        assert!(
            app.hatch_worker.is_none(),
            "worker (receiver) dropped → no stale apply"
        );
        assert!(app.op_cancel.load(Ordering::Relaxed), "op_cancel set");
    }

    #[test]
    fn cancel_hatch_worker_is_noop_when_idle() {
        let mut app = CadApp::default();
        app.op_cancel.store(false, Ordering::Relaxed);
        assert!(!app.cancel_hatch_worker(), "no worker → returns false");
        assert!(
            !app.op_cancel.load(Ordering::Relaxed),
            "op_cancel untouched when idle"
        );
    }
}
#[cfg(test)]
mod hatch_dedupe_tests {
    use super::*;

    /// Issue #17 + hatch_aux: the kernel resolver skips boundaries on
    /// INVISIBLE layers — except `hatch_aux` synthetic boundaries (the
    /// pick-point trace's non-rendering polylines). A hatch whose
    /// boundary is a worker-made aux polyline must still resolve.
    #[test]
    fn aux_boundaries_still_resolve_but_user_hidden_do_not() {
        let mut app = CadApp::default();
        // User-hidden boundary: invisible, NOT aux → skipped by the resolver.
        let mut hidden = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 4.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 4.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        hidden.style.visible = false;
        let h_idx = app.doc.push(hidden);
        // Synthetic aux boundary: invisible BUT hatch_aux → still resolves.
        let mut aux = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 4.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 4.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        aux.style.visible = false;
        aux.style.hatch_aux = true;
        let a_idx = app.doc.push(aux);

        let hatch_hidden = cad_kernel::Hatch {
            boundary_handles: vec![app.doc.dobjects[h_idx].handle],
            pattern: cad_kernel::HatchPattern::Solid,
        };
        assert_eq!(
            app.resolve_hatch_loops(&hatch_hidden).len(),
            0,
            "user-hidden boundary must NOT resolve (issue #17)"
        );

        let hatch_aux = cad_kernel::Hatch {
            boundary_handles: vec![app.doc.dobjects[a_idx].handle],
            pattern: cad_kernel::HatchPattern::Solid,
        };
        assert_eq!(app.resolve_hatch_loops(&hatch_aux).len(), 1,
            "hatch_aux synthetic boundary MUST resolve — pick-point hatch fills are empty without this");
    }

    /// Solid even-odd must classify by CONTAINMENT, not loop index:
    /// two disjoint regions are BOTH fills (the old `i % 2` parity made
    /// the second invisible), and a nested island is a hole.
    #[test]
    fn solid_loop_classification_disjoint_and_nested() {
        let sq = |x: f64, y: f64, half: f64| -> Vec<Vec2> {
            vec![
                Vec2::new(x - half, y - half),
                Vec2::new(x + half, y - half),
                Vec2::new(x + half, y + half),
                Vec2::new(x - half, y + half),
            ]
        };
        // Two DISJOINT squares — both fills.
        let disjoint = vec![sq(0.0, 0.0, 2.0), sq(10.0, 0.0, 2.0)];
        assert!(solid_loop_is_fill(&disjoint, 0), "left square is a fill");
        assert!(
            solid_loop_is_fill(&disjoint, 1),
            "disjoint right square MUST be a fill — index parity made it invisible"
        );
        // Outer + nested island — island is a hole.
        let nested = vec![sq(0.0, 0.0, 5.0), sq(0.0, 0.0, 1.0)];
        assert!(solid_loop_is_fill(&nested, 0), "outer is a fill");
        assert!(!solid_loop_is_fill(&nested, 1), "island is a hole");
        // Island-in-hole — that loop is a fill again (even-odd).
        let island_in_hole = vec![
            sq(0.0, 0.0, 8.0), // outer
            sq(0.0, 0.0, 4.0), // hole
            sq(0.0, 0.0, 1.0), // fill
        ];
        assert!(solid_loop_is_fill(&island_in_hole, 0));
        assert!(!solid_loop_is_fill(&island_in_hole, 1));
        assert!(
            solid_loop_is_fill(&island_in_hole, 2),
            "island-in-hole is a fill again"
        );
    }

    #[test]
    fn dedupe_hits_collapses_vertex_double_counts() {
        // The 45° pattern line through the CENTER of a 64-vertex circle
        // hits at vertex angles 45/135/225/315° — each crossing reported
        // TWICE (once per adjacent edge). Old code: [ta,ta,tb,tb] pairs
        // consumed themselves → the whole line vanished. Dedupe must
        // collapse to [ta, tb].
        let mut hits = vec![1.0, 1.0, 9.0, 9.0];
        dedupe_hits(&mut hits);
        assert_eq!(hits, vec![1.0, 9.0]);
        // Genuinely distinct hits survive, unsorted input sorts.
        let mut hits2 = vec![5.0, 1.0, 3.0];
        dedupe_hits(&mut hits2);
        assert_eq!(hits2, vec![1.0, 3.0, 5.0]);
        // Duplicates in the middle collapse too.
        let mut hits3 = vec![0.0, 2.0, 2.0, 2.0, 7.0];
        dedupe_hits(&mut hits3);
        assert_eq!(hits3, vec![0.0, 2.0, 7.0]);
    }

    #[test]
    fn hatch_center_line_survives_vertex_hits() {
        // Regression: ANSI31 (45°) hatch over a 64-vertex circle AT THE
        // ORIGIN (tessellation vertex at 45°). The pattern line through
        // (0,0) hits the boundary exactly at vertices — it must still be
        // emitted by hatch_pattern_geometry.
        //
        // Self-contained fixture: the demo plan used to ship this circle;
        // it now ships a room outline at real sizes and must not be leaned
        // on as test geometry.
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        let mut circle: DObject = Circle {
            center: Vec2::new(0.0, 0.0),
            radius: 30.0,
        }
        .into();
        app.stamp_fresh_style(&mut circle.style);
        let circle_idx = app.doc.push(circle);
        let h = app.doc.dobjects[circle_idx].handle;
        let n0 = app.doc.dobjects.len();
        app.add_dobject(
            Geom::Hatch(cad_kernel::Hatch {
                boundary_handles: vec![h],
                pattern: cad_kernel::HatchPattern::Pattern {
                    name: "ANSI31".into(),
                    scale: 1.0,
                    angle_deg: 0.0,
                },
            }),
            "t",
        );
        let Geom::Hatch(hatch) = &app.doc.dobjects[n0].geom else {
            panic!()
        };
        let (segs, _circs) = app.hatch_pattern_world_geometry(hatch);
        // The 45° line through the origin crosses the r=30 circle at ±30√2
        // along it — pre-fix both hits were double-counted and the line
        // vanished. Now it survives as one segment.
        let on_diag = |s: &(Vec2, Vec2)| {
            (s.0.x.abs() - s.0.y.abs()).abs() < 1e-6 && (s.1.x.abs() - s.1.y.abs()).abs() < 1e-6
        };
        assert!(
            segs.iter().any(on_diag),
            "the 45° line through the circle centre must survive vertex hits, got {} segs",
            segs.len()
        );
    }
}
#[cfg(test)]
mod hatch_cache_invalidation_tests {
    use super::*;

    // Build the spatial index once so `ensure_index` early-returns when clean
    // (its clean-path guard is `!index_dirty && index.is_some()`).
    fn app_with_index() -> CadApp {
        let mut app = CadApp::default();
        app.index_dirty = true;
        app.ensure_index();
        assert!(app.index.is_some());
        app
    }

    // GP1: a pure SELECTION/highlight change (gpu_dirty, NOT index_dirty) must
    // NOT invalidate the hatch cache — else clicking a dense-hatch drawing
    // re-generates every hatch and stutters.
    #[test]
    fn selection_change_keeps_hatch_cache() {
        let mut app = app_with_index();
        app.hatch_cache.insert(42u64, HatchCacheEntry::default());
        // Selection changed: render-dirty but geometry unchanged.
        app.gpu_dirty = true;
        app.index_dirty = false;
        app.ensure_index(); // the invalidation point (Option B)
        assert!(
            app.hatch_cache.contains_key(&42u64),
            "selection change must NOT clear the hatch cache"
        );
    }

    // GP9: a GEOMETRY change (index_dirty — a boundary added/moved/edited) MUST
    // invalidate the hatch cache, else the fill goes stale.
    #[test]
    fn geometry_change_clears_hatch_cache() {
        let mut app = app_with_index();
        app.hatch_cache.insert(42u64, HatchCacheEntry::default());
        app.index_dirty = true; // geometry mutated
        app.ensure_index();
        assert!(
            app.hatch_cache.is_empty(),
            "geometry change must clear the hatch cache"
        );
    }
}
#[cfg(test)]
mod hatch_transform_tests {
    use super::*;

    // A 4×3 closed rect + a Solid hatch referencing it. Returns (boundary, hatch).
    // Vertex 1 sits at (4,0): rotating 90° CCW about the origin sends it to
    // (0,4), and a DOUBLE rotation to (-4,0) — the dedup test turns on that.
    fn doc_with_hatch(app: &mut CadApp) -> (usize, usize) {
        let rect = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 3.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 3.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        let bhandle = rect.handle;
        let bidx = app.doc.push(rect);
        let hatch = DObject::new(Geom::Hatch(cad_kernel::Hatch {
            boundary_handles: vec![bhandle],
            pattern: cad_kernel::HatchPattern::Solid,
        }));
        let hidx = app.doc.push(hatch);
        (bidx, hidx)
    }

    fn vertex(app: &CadApp, idx: usize, v: usize) -> Vec2 {
        match &app.doc.dobjects[idx].geom {
            Geom::Polyline(p) => p.vertices[v].pos,
            other => panic!("expected the boundary Polyline, got {other:?}"),
        }
    }

    // The dobject the USER drew must stay independent of the hatch: applying a
    // hatch to a selected rectangle binds the fill to a baked, invisible copy,
    // not to the rectangle itself.
    //
    // Regression (owner-reported): rectangle / polygon / pline took the cheap
    // select-objects path, which referenced the user's dobject directly, so the
    // fill dragged along when the shape moved — while a traced circle/ellipse
    // (which always baked a copy) stayed independent. Same command, two
    // behaviours.
    #[test]
    fn hatching_a_shape_bakes_an_independent_boundary() {
        let mut app = CadApp::default();
        let rect = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 3.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 3.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        let user_handle = rect.handle;
        let user_idx = app.doc.push(rect);
        app.selection = vec![user_idx];
        app.pending_hatch_pattern = (Some("ANSI31".into()), 1.0, 0.0);
        app.apply_hatch();

        let hidx = app.hatch_last_idx.expect("a hatch must have been created");
        let Geom::Hatch(h) = &app.doc.dobjects[hidx].geom else {
            panic!("not a hatch")
        };
        assert_eq!(h.boundary_handles.len(), 1);
        let bound = h.boundary_handles[0];
        assert_ne!(
            bound, user_handle,
            "the hatch must NOT reference the dobject the user drew"
        );

        // The baked boundary exists, is invisible, and is flagged so loop
        // resolution still honours it.
        let bidx = app
            .doc
            .dobjects
            .iter()
            .position(|d| d.handle == bound)
            .expect("baked boundary must be in the document");
        assert!(
            !app.doc.dobjects[bidx].style.visible,
            "baked boundary must be invisible"
        );
        assert!(
            app.doc.dobjects[bidx].style.hatch_aux,
            "baked boundary must be flagged hatch_aux"
        );
        // …and it must still resolve to a usable loop, or the fill renders nothing.
        let loops = cad_kernel::resolve_hatch_loops(h, &app.doc);
        assert_eq!(loops.len(), 1, "the baked boundary must resolve");

        // The decisive behaviour: moving the user's shape leaves the fill put.
        app.selection = vec![user_idx];
        app.apply_move(Vec2::new(50.0, 0.0));
        let after = cad_kernel::resolve_hatch_loops(
            match &app.doc.dobjects[hidx].geom {
                Geom::Hatch(h) => h,
                _ => unreachable!(),
            },
            &app.doc,
        );
        let moved = after[0].iter().any(|p| p.x > 40.0);
        assert!(
            !moved,
            "moving the user's shape must not drag the hatch with it"
        );
    }

    // Selecting a hatch must expose GRIPS — the vertices of the boundary it
    // owns — so the fill can be reshaped by dragging, like any other dobject.
    // `Geom::Hatch::grip_points()` is empty (a hatch has no geometry of its
    // own), so the grips come from substituting its aux boundary into the
    // grip-target set.
    #[test]
    fn selecting_a_hatch_exposes_its_boundary_grips() {
        let mut app = CadApp::default();
        let rect = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 3.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 3.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        let user_idx = app.doc.push(rect);
        app.selection = vec![user_idx];
        app.pending_hatch_pattern = (Some("ANSI31".into()), 1.0, 0.0);
        app.apply_hatch();
        let hidx = app.hatch_last_idx.expect("hatch created");

        // Select ONLY the hatch.
        app.selection = vec![hidx];
        app.selected = None;
        let targets = app.editable_grip_targets();

        // The hatch itself yields no grips…
        assert!(
            app.doc.dobjects[hidx].geom.grip_points().is_empty(),
            "a Hatch has no grips of its own"
        );
        // …so its aux boundary must be in the target set, and must carry grips.
        let Geom::Hatch(h) = &app.doc.dobjects[hidx].geom else {
            panic!()
        };
        let bidx = app
            .doc
            .dobjects
            .iter()
            .position(|d| d.handle == h.boundary_handles[0])
            .unwrap();
        assert!(
            targets.contains(&bidx),
            "the hatch's aux boundary must be a grip target so the fill is draggable"
        );
        let grips = app.doc.dobjects[bidx].geom.grip_points();
        assert!(
            grips.len() >= 4,
            "expected boundary vertex grips, got {}",
            grips.len()
        );

        // Dragging one must reshape the fill.
        let (gp, role) = grips[0];
        let moved = app.doc.dobjects[bidx]
            .geom
            .with_grip_moved(role, gp + Vec2::new(-5.0, -5.0));
        app.doc.dobjects[bidx].geom = moved;
        let loops = cad_kernel::resolve_hatch_loops(
            match &app.doc.dobjects[hidx].geom {
                Geom::Hatch(h) => h,
                _ => unreachable!(),
            },
            &app.doc,
        );
        assert!(
            loops[0].iter().any(|p| p.x < -1.0 || p.y < -1.0),
            "dragging a boundary grip must reshape the hatch's resolved loop"
        );

        // The user's own shape must NOT be dragged in as a grip target by this.
        app.selection = vec![hidx];
        assert!(
            !app.editable_grip_targets().contains(&user_idx),
            "selecting a hatch must not expose grips on the shape the user drew"
        );
    }

    // (a) Selecting ONLY the hatch and rotating must rotate its boundary, so the
    // fill follows. Both stay SEPARATE dobjects — nothing is merged.
    #[test]
    fn rotating_a_hatch_also_rotates_its_boundary() {
        let mut app = CadApp::default();
        let (bidx, hidx) = doc_with_hatch(&mut app);
        app.selection = vec![hidx];
        app.apply_rotate(Vec2::ZERO, std::f64::consts::FRAC_PI_2);
        assert!(
            (vertex(&app, bidx, 1) - Vec2::new(0.0, 4.0)).len() < 1e-9,
            "boundary must rotate with the hatch: (4,0) -90°CCW-> (0,4)"
        );
        assert!(
            matches!(app.doc.dobjects[hidx].geom, Geom::Hatch(_)),
            "the hatch stays a separate Hatch dobject (not merged)"
        );
    }

    // (b) THE DEDUP TEST. Hatch AND its boundary both selected: the boundary must
    // rotate exactly ONCE. Twice would land it at (-4,0) — the `seen` guard in
    // transform_targets_with_hatch_boundaries is what prevents that.
    #[test]
    fn hatch_and_boundary_both_selected_rotates_boundary_once() {
        let mut app = CadApp::default();
        let (bidx, hidx) = doc_with_hatch(&mut app);
        app.selection = vec![bidx, hidx];
        app.apply_rotate(Vec2::ZERO, std::f64::consts::FRAC_PI_2);
        let got = vertex(&app, bidx, 1);
        assert!(
            (got - Vec2::new(0.0, 4.0)).len() < 1e-9,
            "boundary rotated exactly ONCE; got {got:?} — (-4,0) would mean twice"
        );
    }

    // Scale + mirror ride the same widened set.
    #[test]
    fn scaling_a_hatch_also_scales_its_boundary() {
        let mut app = CadApp::default();
        let (bidx, hidx) = doc_with_hatch(&mut app);
        app.selection = vec![hidx];
        app.apply_scale(Vec2::ZERO, 2.0);
        assert!(
            (vertex(&app, bidx, 1) - Vec2::new(8.0, 0.0)).len() < 1e-9,
            "boundary must scale with the hatch: (4,0) -2x-> (8,0)"
        );
    }

    #[test]
    fn mirroring_a_hatch_in_place_also_mirrors_its_boundary() {
        let mut app = CadApp::default();
        let (bidx, hidx) = doc_with_hatch(&mut app);
        app.selection = vec![hidx];
        // Mirror across the Y axis: (4,0) -> (-4,0).
        app.apply_mirror(Vec2::ZERO, Vec2::new(0.0, 1.0), false);
        assert!(
            (vertex(&app, bidx, 1) - Vec2::new(-4.0, 0.0)).len() < 1e-9,
            "boundary must mirror with the hatch across the Y axis"
        );
    }

    // (c) The COPY semantic is DELIBERATE and must not regress: a hatch copied
    // alone still references the ORIGINAL boundary, so the boundary is NOT
    // duplicated (rotate-copy clones the hatch itself and keeps its handles).
    #[test]
    fn rotate_copy_of_a_hatch_does_not_duplicate_the_boundary() {
        let mut app = CadApp::default();
        let (bidx, hidx) = doc_with_hatch(&mut app);
        let bhandle = app.doc.dobjects[bidx].handle;
        let before = app.doc.dobjects.len();
        app.selection = vec![hidx];
        app.rotate_copy = true;
        app.apply_rotate_or_copy(Vec2::ZERO, std::f64::consts::FRAC_PI_2);
        assert_eq!(
            app.doc.dobjects.len(),
            before + 1,
            "exactly ONE new dobject (the hatch copy) — the boundary is not copied"
        );
        // The copy is a Hatch still pointing at the ORIGINAL boundary handle:
        // the copy branch clones the hatch and only re-stamps the copy's own
        // handle — boundary handles are untouched. This is the deliberate semantic.
        match &app.doc.dobjects[before].geom {
            Geom::Hatch(h) => assert_eq!(
                h.boundary_handles,
                vec![bhandle],
                "the copied hatch still references the ORIGINAL boundary"
            ),
            other => panic!("the copy should be a Hatch, got {other:?}"),
        }
        // ...and the original boundary did not move: copy leaves originals alone.
        assert!(
            (vertex(&app, bidx, 1) - Vec2::new(4.0, 0.0)).len() < 1e-9,
            "rotate-COPY must not transform the original boundary in place"
        );
    }
}
#[cfg(test)]
mod a_loop_inside_a_loop_is_a_hole {
    use super::*;

    fn square(cx: f32, cy: f32, half: f32) -> Vec<glam::Vec2> {
        vec![
            glam::Vec2::new(cx - half, cy - half),
            glam::Vec2::new(cx + half, cy - half),
            glam::Vec2::new(cx + half, cy + half),
            glam::Vec2::new(cx - half, cy + half),
        ]
    }

    /// Two loops side by side are two objects, and must stay two — the case that would break if
    /// nesting were decided by size alone.
    #[test]
    fn two_separate_shapes_stay_two_shapes() {
        let out = CadApp::nest_loops(vec![square(0.0, 0.0, 4.0), square(20.0, 0.0, 1.0)]);
        assert_eq!(
            out.len(),
            2,
            "two disjoint loops became {} object(s)",
            out.len()
        );
        assert!(
            out.iter().all(|(_, h)| h.is_empty()),
            "a disjoint loop was taken as a hole"
        );
    }

    /// The headline. One plate, four bolt holes, ONE object.
    #[test]
    fn a_plate_with_four_bolt_circles_is_one_object_with_four_holes() {
        let mut loops = vec![square(0.0, 0.0, 8.0)];
        for &(x, y) in &[(-4.0_f32, -4.0_f32), (4.0, -4.0), (4.0, 4.0), (-4.0, 4.0)] {
            loops.push(square(x, y, 1.0));
        }
        let out = CadApp::nest_loops(loops);
        assert_eq!(
            out.len(),
            1,
            "the plate and its bolt holes became {} objects",
            out.len()
        );
        assert_eq!(
            out[0].1.len(),
            4,
            "the plate came out with {} holes",
            out[0].1.len()
        );
    }

    /// ORDER MUST NOT MATTER. The loops arrive in whatever order the drafter drew them, and a
    /// version that assumed the outline came first would work for everybody who drew the plate
    /// before the holes and silently invert for everybody who did not.
    #[test]
    fn the_holes_are_found_whichever_order_they_were_drawn_in() {
        let inner = square(0.0, 0.0, 1.0);
        let outer = square(0.0, 0.0, 8.0);
        for (a, b) in [
            (outer.clone(), inner.clone()),
            (inner.clone(), outer.clone()),
        ] {
            let out = CadApp::nest_loops(vec![a, b]);
            assert_eq!(
                out.len(),
                1,
                "drawn in this order it made {} objects",
                out.len()
            );
            assert_eq!(out[0].1.len(), 1, "the hole was lost");
            // And the OUTLINE is the big one, not the small one.
            let span = out[0].0.iter().fold(0.0_f32, |m, p| m.max(p.x));
            assert!(
                (span - 8.0).abs() < 1e-4,
                "the hole was taken as the outline"
            );
        }
    }

    /// AN ISLAND IN A HOLE IS SOLID AGAIN. Even-odd nesting is the only rule that gets this
    /// right: a washer with a pin standing in its bore is a washer with a hole AND a pin, not a
    /// washer with two holes one of which is filled in.
    #[test]
    fn an_island_inside_a_hole_becomes_its_own_object() {
        let out = CadApp::nest_loops(vec![
            square(0.0, 0.0, 8.0), // the plate
            square(0.0, 0.0, 4.0), // a hole in it
            square(0.0, 0.0, 1.0), // a pin standing in the hole
        ]);
        assert_eq!(
            out.len(),
            2,
            "expected the plate and the pin, got {} objects",
            out.len()
        );
        let plate = out
            .iter()
            .find(|(o, _)| o.iter().any(|p| p.x > 7.0))
            .expect("the plate");
        assert_eq!(plate.1.len(), 1, "the plate must have exactly one hole");
        let pin = out
            .iter()
            .find(|(o, _)| o.iter().all(|p| p.x.abs() < 2.0))
            .expect("the pin");
        assert!(pin.1.is_empty(), "the pin picked up a hole of its own");
    }

    /// A HOLE BELONGS TO THE LOOP THAT ACTUALLY CONTAINS IT, not to the biggest one on the sheet.
    /// Two plates side by side, one hole in each: sorting by area and attaching every hole to the
    /// largest outline would put both holes in one plate and leave the other solid.
    #[test]
    fn each_plate_keeps_its_own_hole() {
        let out = CadApp::nest_loops(vec![
            square(0.0, 0.0, 8.0),
            square(0.0, 0.0, 1.0),
            square(30.0, 0.0, 6.0),
            square(30.0, 0.0, 1.0),
        ]);
        assert_eq!(out.len(), 2, "expected two plates, got {}", out.len());
        for (outline, holes) in &out {
            assert_eq!(
                holes.len(),
                1,
                "a plate ended up with {} holes",
                holes.len()
            );
            let cx = outline.iter().map(|p| p.x).sum::<f32>() / outline.len() as f32;
            let hx = holes[0].iter().map(|p| p.x).sum::<f32>() / holes[0].len() as f32;
            assert!(
                (cx - hx).abs() < 1.0,
                "a hole at x = {hx} was attached to the plate at x = {cx}",
            );
        }
    }

    /// One loop on its own is one object with no holes — the ordinary case, and the early return.
    #[test]
    fn a_single_loop_is_one_object() {
        let out = CadApp::nest_loops(vec![square(0.0, 0.0, 4.0)]);
        assert_eq!(out.len(), 1);
        assert!(out[0].1.is_empty());
    }
}
#[cfg(test)]
mod hatch_erase_and_grip_parity_tests {
    use super::*;

    // Two selected dobjects sharing a corner must move that corner TOGETHER
    // when it is dragged — the AutoCAD rule for coincident grips.
    //
    // Regression (owner-reported): the grab loop stopped at the first match
    // (`break 'outer`) and the apply touched only that one dobject, so shapes
    // that shared a corner came apart. Same root cause made a hatch part
    // company with the boundary it was built from.
    #[test]
    fn coincident_grips_of_a_selection_move_together() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        let shared = Vec2::new(10.0, 0.0);
        // Two squares meeting at `shared`.
        let a = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: shared,
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(10.0, 10.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 10.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        let b = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: shared,
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(20.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(20.0, -10.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(10.0, -10.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        let ia = app.doc.push(a);
        let ib = app.doc.push(b);
        app.selection = vec![ia, ib];

        // Both must expose a grip AT the shared corner.
        let grips_a = app.doc.dobjects[ia].geom.grip_points();
        let grips_b = app.doc.dobjects[ib].geom.grip_points();
        let ra = grips_a
            .iter()
            .find(|(p, _)| p.dist(shared) < 1e-9)
            .map(|(_, r)| *r)
            .expect("shape A has a grip at the shared corner");
        let rb = grips_b
            .iter()
            .find(|(p, _)| p.dist(shared) < 1e-9)
            .map(|(_, r)| *r)
            .expect("shape B has a grip at the shared corner");

        // Simulate the grab+apply: primary + its coincident peer.
        let drop = Vec2::new(14.0, 4.0);
        for (idx, role) in [(ia, ra), (ib, rb)] {
            let g = app.doc.dobjects[idx].geom.with_grip_moved(role, drop);
            app.doc.dobjects[idx].geom = g;
        }

        // Neither shape may still hold the OLD corner — they moved as one.
        for (idx, name) in [(ia, "A"), (ib, "B")] {
            let Geom::Polyline(p) = &app.doc.dobjects[idx].geom else {
                panic!()
            };
            assert!(
                p.vertices.iter().any(|v| v.pos.dist(drop) < 1e-6),
                "shape {} must have the corner at the drop point",
                name
            );
            assert!(
                !p.vertices.iter().any(|v| v.pos.dist(shared) < 1e-6),
                "shape {} must not keep the old corner — the shapes came apart",
                name
            );
        }
    }

    // Erasing a hatch must take its invisible auxiliary boundary with it.
    //
    // Regression (owner-reported): the boundary survived as unreachable
    // garbage AND stayed a closed region the pick-point scan could choose, so
    // the next hatch in that area silently reused the DELETED hatch's outline
    // instead of the polyline the user drew.
    #[test]
    fn erasing_a_hatch_also_erases_its_aux_boundary() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear(); // CadApp::default() seeds a demo scene
        let rect = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(4.0, 3.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 3.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        let user_idx = app.doc.push(rect);
        app.selection = vec![user_idx];
        app.pending_hatch_pattern = (Some("ANSI31".into()), 1.0, 0.0);
        app.apply_hatch();
        let hidx = app.hatch_last_idx.expect("hatch created");
        let n_after_hatch = app.doc.dobjects.len();
        assert_eq!(n_after_hatch, 3, "user shape + baked boundary + hatch");
        // The confirm panel is open after apply — close it (Accept) so the
        // erase command dispatches instead of bouncing on the panel gate.
        app.hatch_confirm_accept();

        // Erase ONLY the hatch.
        app.selection = vec![hidx];
        app.run_command("erase");

        // Both the hatch and its aux boundary are gone; the user's shape stays.
        assert_eq!(
            app.doc.dobjects.len(),
            1,
            "erasing the hatch must remove its aux boundary too, leaving only the user's shape"
        );
        assert!(
            !app.doc.dobjects.iter().any(|d| d.style.hatch_aux),
            "no orphaned hatch_aux boundary may survive"
        );
        assert!(
            matches!(app.doc.dobjects[0].geom, Geom::Polyline(_)),
            "the shape the user drew must survive"
        );
    }

    // The pick-point scan and the smallest-containing picker must AGREE about
    // which dobjects are eligible. Both must ignore invisible boundaries.
    //
    // Regression: only the collector filtered visibility, so the log read
    // "candidates: #16 (the user's polyline)" and then "chose #17" — a hidden
    // hatch_aux boundary with a smaller bbox.
    #[test]
    fn hidden_boundaries_are_ignored_by_both_pick_scans() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear(); // the demo circle also contains the seed
                                  // Big visible square.
        let big = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(20.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(20.0, 20.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 20.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        let big_idx = app.doc.push(big);
        // SMALLER hidden aux boundary around the same seed — the shape that
        // used to win because nothing filtered it out.
        let mut small = DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(5.0, 5.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(15.0, 5.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(15.0, 15.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(5.0, 15.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }));
        small.style.visible = false;
        small.style.hatch_aux = true;
        app.doc.push(small);

        let seed = Vec2::new(10.0, 10.0); // inside BOTH
        let cands = app.collect_closed_containing_scoped(seed, None);
        assert_eq!(cands.len(), 1, "only the visible square may be a candidate");
        assert_eq!(cands[0].0, big_idx);
        let chosen = app.find_smallest_containing_closed_scoped(seed, None);
        assert_eq!(
            chosen,
            Some(big_idx),
            "the picker must agree with the candidate scan and ignore the hidden boundary"
        );
    }
}
#[cfg(test)]
mod qselect_tests {
    use super::*;

    #[test]
    fn qselect_filters_by_type_and_layer() {
        let mut app = CadApp::default();
        let base = app.doc.dobjects.len();
        // A red circle + a blue circle + a line.
        let mk = |g: Geom, c: u8| {
            let mut d = DObject::new(g);
            d.style.color = cad_kernel::color::Color::Aci(c);
            d
        };
        app.doc.push(mk(
            Geom::Circle(cad_kernel::Circle {
                center: Vec2::new(0.0, 0.0),
                radius: 1.0,
            }),
            1,
        ));
        app.doc.push(mk(
            Geom::Circle(cad_kernel::Circle {
                center: Vec2::new(5.0, 0.0),
                radius: 1.0,
            }),
            3,
        ));
        app.doc.push(mk(
            Geom::Line(cad_kernel::Line {
                a: Vec2::new(0.0, 0.0),
                b: Vec2::new(9.0, 0.0),
            }),
            1,
        ));
        assert_eq!(
            app.doc.dobjects.len(),
            base + 3,
            "3 pushes land (base={base})"
        );
        let base_circles = app
            .doc
            .dobjects
            .iter()
            .filter(|d| matches!(d.geom, Geom::Circle(_)))
            .count();
        app.qselect.kind_filter = Some("Circle".into());
        app.qselect.include = true;
        app.apply_qselect();
        assert_eq!(app.selection.len(), base_circles, "all circles");

        app.qselect.color_filter = Some(1);
        app.apply_qselect();
        assert_eq!(app.selection.len(), 1, "red circle only");
        if let Geom::Circle(c) = &app.doc.dobjects[app.selection[0]].geom {
            assert!((c.center.x - 0.0).abs() < 1e-9);
        } else {
            panic!("wrong pick");
        }

        // Exclude mode = everything except matches.
        app.qselect.include = false;
        app.apply_qselect();
        assert_eq!(app.selection.len(), base + 2, "everything else");
    }

    #[test]
    fn qselect_command_opens_dialog() {
        let mut app = CadApp::default();
        app.run_command("qselect");
        assert!(app.qselect.open);
    }
}
#[cfg(test)]
mod entity_flow_tests {
    use super::*;

    #[test]
    fn xline_and_ray_commits_land_with_correct_geom() {
        let mut app = CadApp::default();
        app.run_command("xline");
        assert!(app.xline_state != XlineState::Off);
        app.commit_xline(Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0));
        let last = app.doc.dobjects.last().unwrap();
        if let Geom::Xline(x) = &last.geom {
            assert!(x.dir.dist(Vec2::new(1.0, 0.0)) < 1e-9);
        } else {
            panic!("expected Xline");
        }
        app.run_command("ray");
        app.commit_ray(Vec2::new(2.0, 0.0), Vec2::new(0.0, 1.0));
        let last = app.doc.dobjects.last().unwrap();
        if let Geom::Ray(r) = &last.geom {
            assert!(r.dir.dist(Vec2::new(0.0, 1.0)) < 1e-9);
        } else {
            panic!("expected Ray");
        }
    }

    #[test]
    fn donut_requires_inner_smaller_than_outer() {
        let mut app = CadApp::default();
        app.commit_donut(Vec2::ZERO, 5.0, 2.0);
        let last = app.doc.dobjects.last().unwrap();
        if let Geom::Donut(d) = &last.geom {
            assert_eq!(d.inner_radius, 2.0);
            assert_eq!(d.outer_radius, 5.0);
        } else {
            panic!("expected Donut");
        }
        let n = app.doc.dobjects.len();
        // Invalid (inner >= outer) must fail without committing.
        app.commit_donut(Vec2::ZERO, 5.0, 6.0);
        assert_eq!(app.doc.dobjects.len(), n, "no dobject for an invalid donut");
        assert!(app.donut_state != DonutState::Off, "flow re-arms");
    }

    #[test]
    fn wipeout_commits_a_rect_and_resets_flow() {
        let mut app = CadApp::default();
        app.commit_wipeout(Vec2::new(1.0, 1.0), Vec2::new(5.0, 4.0));
        let last = app.doc.dobjects.last().unwrap();
        if let Geom::Wipeout(w) = &last.geom {
            assert_eq!(w.pts.len(), 4);
        } else {
            panic!("expected Wipeout");
        }
        assert_eq!(app.wipeout_state, WipeoutState::Off);
    }
}
#[cfg(test)]
mod centermark_tests {
    use super::*;

    #[test]
    fn centermark_commits_with_sized_mark() {
        let mut app = CadApp::default();
        // Place a circle so the click near its centre sizes the mark.
        app.doc.push(DObject::new(Geom::Circle(cad_kernel::Circle {
            center: Vec2::new(10.0, 10.0),
            radius: 4.0,
        })));
        app.commit_centermark_at(Vec2::new(10.0, 10.0), None);
        let last = app.doc.dobjects.last().unwrap();
        if let Geom::CenterMark(cm) = &last.geom {
            assert!(
                (cm.size - 4.0 * 0.18).abs() < 1e-6,
                "mark sized from the circle radius: {}",
                cm.size
            );
            assert!(cm.center.dist(Vec2::new(10.0, 10.0)) < 1e-9);
        } else {
            panic!("expected CenterMark");
        }
        // Size override wins.
        app.commit_centermark_at(Vec2::new(30.0, 30.0), Some(2.5));
        let last = app.doc.dobjects.last().unwrap();
        if let Geom::CenterMark(cm) = &last.geom {
            assert!((cm.size - 2.5).abs() < 1e-6);
        } else {
            panic!("expected CenterMark");
        }
    }
}
#[cfg(test)]
mod region_boundary_tests {
    use super::*;

    #[test]
    fn region_converts_closed_selection_curves() {
        let mut app = CadApp::default();
        let sq = app.doc.push(DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(10.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(10.0, 10.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 10.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        })));
        let line = app.doc.push(DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(20.0, 20.0),
            b: Vec2::new(40.0, 20.0),
        })));
        app.selection = vec![sq, line];
        app.apply_region();
        assert!(
            matches!(app.doc.dobjects[sq].geom, Geom::Region(_)),
            "closed curve became a Region"
        );
        assert!(
            matches!(app.doc.dobjects[line].geom, Geom::Line(_)),
            "open line untouched"
        );
    }

    #[test]
    fn boundary_traces_a_closed_square() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        app.doc.push(DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(20.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(20.0, 20.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 20.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        })));
        let before = app.doc.dobjects.len();
        app.commit_boundary_at(Vec2::new(10.0, 10.0)); // inside
        assert!(
            app.doc.dobjects.len() > before,
            "a boundary polyline was emitted"
        );
        let added = &app.doc.dobjects[before..];
        assert!(
            added.iter().any(|d| matches!(&d.geom,
                Geom::Polyline(p) if p.closed && p.vertices.len() == 4)),
            "closed 4-vertex polyline traced"
        );
    }
}
#[cfg(test)]
mod array_polar_path_tests {
    use super::*;

    fn mk_source(app: &mut CadApp) -> usize {
        app.selection.clear();
        app.doc.push(DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(2.0, 0.0),
        })))
    }

    #[test]
    fn polar_array_rings_around_the_centre() {
        let mut app = CadApp::default();
        let src = mk_source(&mut app);
        app.selection = vec![src];
        app.array_method = ArrayMethod::Polar;
        app.array_count = 8;
        app.array_fill_deg = 360.0;
        app.array_rotate_items = false;
        app.array_center = Vec2::ZERO;
        let before = app.doc.dobjects.len();
        app.generate_polar_array();
        assert_eq!(
            app.doc.dobjects.len(),
            before + 7,
            "8 items: source + 7 copies"
        );
        // Copies sit on a ring of radius 2 around the origin (source bbox
        // anchor starts at (1,0), so revolve keeps distance 1... every copy
        // bbox-anchored at distance ~1 from the centre).
        let copies: Vec<Vec2> = app.doc.dobjects[before..]
            .iter()
            .map(|d| d.geom.bbox())
            .map(|(mn, mx)| (mn + mx) * 0.5)
            .collect();
        for c in &copies {
            assert!(
                (c.len() - 1.0).abs() < 1e-6,
                "copy anchor on the 1-unit ring: {:?}",
                c
            );
        }
    }

    #[test]
    fn path_array_measure_places_items_along_a_line() {
        let mut app = CadApp::default();
        let src = mk_source(&mut app);
        let path = app.doc.push(DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(50.0, 0.0),
        })));
        app.selection = vec![src];
        app.array_method = ArrayMethod::Path;
        app.array_path_idx = Some(path);
        app.array_path_measure = true;
        app.array_path_dist = 10.0;
        app.array_path_anchor = PathAnchor::Start;
        app.array_path_align = false;
        let before = app.doc.dobjects.len();
        app.generate_path_array();
        // 50-unit path @ 10-unit spacing = 6 placements (0..50).
        assert_eq!(
            app.doc.dobjects.len(),
            before + 6,
            "6 path placements × 1 source"
        );
        // Copies are centred on the path samples (bbox centre → path pt).
        let cs: Vec<f64> = app.doc.dobjects[before..]
            .iter()
            .map(|d| d.geom.bbox())
            .map(|(mn, mx)| (mn.x + mx.x) * 0.5)
            .collect();
        assert!(
            (cs[0] - 0.0).abs() < 1e-6 && (cs[5] - 50.0).abs() < 1e-6,
            "centred on the path samples 0..50 @10: {:?}",
            cs
        );
    }

    #[test]
    fn path_array_divide_spans_the_whole_path() {
        let mut app = CadApp::default();
        let src = mk_source(&mut app);
        let path = app.doc.push(DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(40.0, 0.0),
        })));
        app.selection = vec![src];
        app.array_method = ArrayMethod::Path;
        app.array_path_idx = Some(path);
        app.array_path_measure = false;
        app.array_count = 5;
        app.array_path_align = false;
        let before = app.doc.dobjects.len();
        app.generate_path_array();
        assert_eq!(
            app.doc.dobjects.len(),
            before + 5,
            "5 divide placements fill the path"
        );
    }
}
#[cfg(test)]
mod attdef_attedit_tests {
    use super::*;

    #[test]
    fn attdef_flow_places_definition_entities() {
        let mut app = CadApp::default();
        app.run_command("attdef");
        assert_eq!(app.attr_def_flow, AttrDefFlow::AwaitingTag);
        // Feed the fields directly (the intercept does the same).
        app.attr_def_flow = AttrDefFlow::AwaitingPosition {
            tag: "DOOR_NO".into(),
            prompt: "Door number".into(),
            default: "1".into(),
        };
        let before = app.doc.dobjects.len();
        app.commit_attdef_at(Vec2::new(5.0, 5.0), "DOOR_NO", "Door number", "1");
        assert_eq!(app.doc.dobjects.len(), before + 1);
        let last = app.doc.dobjects.last().unwrap();
        if let Geom::AttrDef(a) = &last.geom {
            assert_eq!(a.tag, "DOOR_NO");
            assert_eq!(a.default, "1");
            assert!(a.position.dist(Vec2::new(5.0, 5.0)) < 1e-9);
        } else {
            panic!("expected AttrDef");
        }
        // Flow re-arms for the next definition.
        assert_eq!(app.attr_def_flow, AttrDefFlow::AwaitingTag);
    }

    fn block_with_attr_def(app: &mut CadApp) -> (u32, usize) {
        let mut blk = cad_kernel::Block {
            name: "door".into(),
            base: Vec2::ZERO,
            dobjects: Vec::new(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        };
        let def = DObject::new(Geom::AttrDef(cad_kernel::AttrDef {
            tag: "NO".into(),
            prompt: String::new(),
            default: "42".into(),
            position: Vec2::ZERO,
            height: 1.0,
            angle: 0.0,
            style: cad_kernel::TextStyleTable::STANDARD,
            visible: true,
        }));
        blk.dobjects.push(def);
        let id = app.doc.blocks.add(blk);
        let idx = app
            .doc
            .push(DObject::new(Geom::BlockRef(cad_kernel::BlockRef {
                block: id,
                insert: Vec2::ZERO,
                scale: 1.0,
                scale_y: 1.0,
                rotation: 0.0,
                mirror_x: false,
                attr_values: Vec::new(),
                param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
            })));
        (id, idx)
    }

    #[test]
    fn attedit_opens_and_writes_instance_values() {
        let mut app = CadApp::default();
        let (_, idx) = block_with_attr_def(&mut app);
        app.attedit_open_for(idx);
        assert!(matches!(app.attedit_state, AttEditState::Editing { .. }));
        let AttEditState::Editing { values, .. } = app.attedit_state.clone() else {
            unreachable!()
        };
        assert_eq!(values, vec!["42".to_string()], "prefill = the default");
        // Apply: value → instance.
        assert!(app.attedit_write_values(idx, &["NO".to_string()], &["7".to_string()]));
        let d = app.doc.dobjects.get(idx).unwrap();
        if let Geom::BlockRef(br) = &d.geom {
            assert_eq!(br.attr_values, vec!["7".to_string()]);
        } else {
            panic!("expected BlockRef");
        }
    }

    #[test]
    fn attedit_pick_only_accepts_blocks_with_attributes() {
        let mut app = CadApp::default();
        // Plain block (no AttrDefs) — pick must refuse.
        let mut plain = cad_kernel::Block {
            name: "plain".into(),
            base: Vec2::ZERO,
            dobjects: Vec::new(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        };
        let pid = app.doc.blocks.add(plain);
        let pidx = app
            .doc
            .push(DObject::new(Geom::BlockRef(cad_kernel::BlockRef {
                block: pid,
                insert: Vec2::new(0.0, 0.0),
                scale: 1.0,
                scale_y: 1.0,
                rotation: 0.0,
                mirror_x: false,
                attr_values: Vec::new(),
                param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
            })));
        let (_, aidx) = block_with_attr_def(&mut app);
        // Move the attr block off the plain one before picking.
        if let Some(d) = app.doc.dobjects.get_mut(aidx) {
            if let Geom::BlockRef(br) = &mut d.geom {
                br.insert = Vec2::new(40.0, 0.0);
            }
        }
        assert_eq!(app.attedit_pick_block(Vec2::new(41.0, 0.0)), Some(aidx));
        assert_eq!(app.attedit_pick_block(Vec2::new(2.0, 2.0)), None);
        let _ = pidx;
    }
}
