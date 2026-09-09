use super::super::*;

#[cfg(test)]
mod nav_cube_tests {
    use super::*;
    use crate::factory::StdView;

    #[test]
    fn clicking_the_front_face_centre_snaps_to_that_view() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(120.0, 120.0));
        // yaw=0, pitch=0 → camera on +X looking at the origin = the RIGHT view; the +X
        // face is the only front face and projects onto the cube centre.
        assert_eq!(
            nav_cube_pick(rect, 0.0, 0.0, rect.center()),
            Some(StdView::Right)
        );
    }

    #[test]
    fn a_point_out_on_the_ring_picks_nothing() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(120.0, 120.0));
        assert_eq!(nav_cube_pick(rect, 0.0, 0.0, egui::pos2(2.0, 2.0)), None);
    }
}
#[cfg(test)]
mod active_view_dispatch_tests {
    use super::*;
    use cad_solid::modify::{Feed, ModifyOp};

    fn app_3d() -> CadApp {
        let mut app = CadApp::default();
        app.factory.open = true;
        app.factory.add_box();
        app.factory.recompute();
        app.active_view = ActiveView::ThreeD;
        app.selection.clear();
        app
    }

    /// There is NO `3dmove`. The user typed it and got
    /// `ParseErr(unknown command '3dmove')` — correctly. `move` is the only command;
    /// the ACTIVE VIEW decides the algorithm.
    #[test]
    fn there_is_no_3dmove_command() {
        let mut app = app_3d();
        app.run_command("3dmove");
        assert!(app.factory.modify.is_none() && app.factory.queued.is_none());
        assert!(
            app.history.iter().any(|h| h.contains("unknown command")),
            "`3dmove` must remain unknown — move is move"
        );
    }

    /// 3D ACTIVE + `m` + nothing picked → step 3: gather. NOT the 2D session.
    #[test]
    fn three_d_active_move_gathers_solids_not_2d_dobjects() {
        let mut app = app_3d();
        app.factory.selection.clear();
        app.run_command("m");
        assert_eq!(
            app.factory.queued,
            Some(ModifyOp::Move),
            "queued for the 3D gather"
        );
        assert_eq!(
            app.select_mode,
            SelectMode::Off,
            "the 2D session must NOT open"
        );
        assert!(app.factory.status.contains("select solids"));
    }

    /// The full six steps in 3D: Move → select → Enter → first point → second point.
    #[test]
    fn the_six_steps_move_a_solid_in_3d() {
        let mut app = app_3d();
        let id = app.factory.model.features[0].id;
        let before = app.factory.model.get_mut(id).unwrap().world_origin();
        // add_box() auto-selects what it creates; clear it so this exercises the
        // GATHER path (step 3) rather than pickfirst.
        app.factory.selection.clear();

        app.run_command("m"); // 2. Move
        app.factory.selection = vec![id]; // 3. select what is going to move
        assert!(app.factory_confirm_gather(), "Enter dispatches the gather");
        assert!(app.factory.modify.is_some(), "→ 4. first point");

        let plane = cad_solid::Plane::default();
        let mut md = app.factory.modify.take().unwrap();
        let f = md.feed(glam::Vec3::ZERO, &plane, &mut app.factory.model, false);
        assert_eq!(f, Feed::NeedMore, "4. first point = BASE");
        app.factory_after_feed(md, f);

        let mut md = app.factory.modify.take().unwrap();
        let f = md.feed(
            glam::Vec3::new(7.0, 0.0, 0.0),
            &plane,
            &mut app.factory.model,
            false,
        );
        assert_eq!(f, Feed::Applied, "5. second point applies");
        app.factory_after_feed(md, f);

        let after = app.factory.model.get_mut(id).unwrap().world_origin();
        assert!(
            (after.x - before.x - 7.0).abs() < 1e-3,
            "6. done: moved +7 in x"
        );
        assert!(
            app.factory.modify.is_none() && app.factory.selection.is_empty(),
            "op finished and cleared the selection, like apply_move"
        );
    }

    /// Pickfirst in 3D: solids already picked → skip the gather, straight to step 4.
    #[test]
    fn three_d_pickfirst_skips_the_gather() {
        let mut app = app_3d();
        app.factory.selection = vec![app.factory.model.features[0].id];
        app.run_command("move");
        assert!(app.factory.modify.is_some(), "straight to the first point");
        assert!(app.factory.queued.is_none());
    }

    /// ⭐ THE REGRESSION GUARD. 2D ACTIVE → the established command, ALWAYS — even with
    /// the 3D panel open and holding a stale selection. This is the state that stole
    /// `m` twice.
    #[test]
    fn two_d_active_always_runs_the_established_command() {
        for verb in ["move", "m", "copy", "rotate", "scale", "mirror"] {
            let mut app = app_3d();
            app.factory.selection = vec![1]; // stale 3D selection
            app.active_view = ActiveView::TwoD; // …but you are working in 2D
            app.selection.clear();
            app.run_command(verb);
            assert_eq!(
                app.select_mode,
                SelectMode::ForSelect,
                "`{verb}` must run the established 2D flow when 2D is active"
            );
            assert!(app.factory.modify.is_none(), "`{verb}` must not touch 3D");
        }
    }

    /// Drafting on the plane IS 2D. While you sketch you click the 2D canvas, which sets
    /// `active_view = TwoD`, so `move` is the established 2D command — sketch or not. The
    /// sketch is not what makes it 2D; the active view is.
    #[test]
    fn sketch_open_and_drafting_2d_is_the_2d_move() {
        let mut app = app_3d();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.active_view = ActiveView::TwoD; // you are drafting on the 2D canvas
        app.selection.clear();
        app.run_command("move");
        assert_eq!(
            app.select_mode,
            SelectMode::ForSelect,
            "2D drafting → the 2D move"
        );
        assert!(
            app.factory.modify.is_none() && app.factory.queued.is_none(),
            "the 3D branch must not claim a draft"
        );
    }

    /// ⭐ THE FIX (user, 2026-07-17: "move is not doing anything in moving 3d dobject …
    /// app should [know] 3d is active [and] make proper adjustment in move"). A sketch
    /// being OPEN must not block moving a solid you picked in the 3D viewport. When
    /// `active_view` is `ThreeD` you are working in the 3D view, so `move` is the 3D move
    /// — sketch open or not. Regression guard: the old `&& session.is_none()` gate sent
    /// this down the dead 2D path (`select_mode → ForSelect`) and the solid never moved.
    #[test]
    fn sketch_open_but_working_in_3d_moves_the_solid() {
        let mut app = app_3d();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.factory.selection = vec![app.factory.model.features[0].id]; // picked in 3D
        app.active_view = ActiveView::ThreeD;
        app.run_command("move");
        assert_eq!(
            app.select_mode,
            SelectMode::Off,
            "the 2D session must NOT open"
        );
        assert!(
            app.factory.modify.is_some(),
            "picked solid + 3D active → straight to the first point, sketch open or not"
        );
    }
}
#[cfg(test)]
mod perf_investigation {
    use super::*;

    /// HOW THE CACHED FRAME SCALES. The line cache removed the per-frame re-flattening; what is left
    /// is the memcpy of the cached buffer into the single list the renderer takes. This measures
    /// whether that residue is a rounding error or the next wall.
    #[test]
    #[ignore = "investigation: --ignored --nocapture"]
    fn how_the_cached_frame_scales() {
        println!(
            "\n{:>10}  {:>12}  {:>10}  {:>9}   |  {:>9}  {:>9}",
            "dobjects", "cached verts", "MB/frame", "ms/frame", "MB culled", "ms culled",
        );
        for n in [1_000usize, 10_000, 50_000, 200_000] {
            let mut app = CadApp::default();
            app.doc.dobjects.clear();
            // Polylines of 8 points, spread over a plan-sized area — the shape a traced drawing has.
            for i in 0..n {
                let (cx, cy) = ((i % 400) as f64 * 2.0, (i / 400) as f64 * 2.0);
                let vertices: Vec<cad_kernel::PolyVertex> = (0..8)
                    .map(|k| cad_kernel::PolyVertex {
                        pos: Vec2::new(cx + (k % 3) as f64 * 0.4, cy + (k / 3) as f64 * 0.4),
                        bulge: 0.0,
                    })
                    .collect();
                app.doc
                    .push(DObject::new(Geom::Polyline(cad_kernel::Polyline {
                        vertices,
                        closed: false,
                        widths: Vec::new(),
                    })));
            }
            app.factory.show_plan = true;
            app.refresh_cached_lines();
            let verts = app.cached_plan_lines.len();

            const N: u32 = 10;
            let t = std::time::Instant::now();
            let mut total = 0usize;
            for _ in 0..N {
                app.refresh_cached_lines();
                let mut lines = app.factory.overlay_lines();
                lines.extend_from_slice(&app.cached_sketch_lines);
                lines.extend_from_slice(&app.cached_plan_lines);
                total = lines.len();
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / N as f64;

            // …AND THE SAME FRAME CULLED to a 60 m window, which is what a designer working on one
            // room actually has on screen.
            let view = Some((glam::Vec2::new(0.0, 0.0), glam::Vec2::new(60.0, 60.0)));
            let t = std::time::Instant::now();
            let mut culled_total = 0usize;
            for _ in 0..N {
                let mut lines = app.factory.overlay_lines();
                lines.extend_from_slice(&app.cached_sketch_lines);
                app.emit_plan_lines(&mut lines, view);
                culled_total = lines.len();
            }
            let cull_ms = t.elapsed().as_secs_f64() * 1000.0 / N as f64;

            println!(
                "{n:>10}  {verts:>12}  {:>10.1}  {ms:>9.2}   |  {:>9.1}  {cull_ms:>9.2}",
                total as f64 * 40.0 / (1024.0 * 1024.0),
                culled_total as f64 * 40.0 / (1024.0 * 1024.0),
            );
        }
        println!("  (16.7 ms is one frame at 60 Hz)\n");
    }

    fn doc_with(n: usize) -> Document {
        let mut d = Document::default();
        d.dobjects.clear();
        for i in 0..n {
            let x = (i % 2000) as f64 * 10.0;
            let y = (i / 2000) as f64 * 10.0;
            d.push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(x, y),
                    b: Vec2::new(x + 8.0, y + 4.0),
                },
            )));
        }
        d
    }

    /// The owner's ACTUAL drawing: the 6-dobject seed arrayed ×250,000. My earlier
    /// benchmarks used flat lines and reported 163 ms; the real dump says 386-397 ms.
    /// bbox() is O(verts) for a polyline and non-trivial for arc/ellipse, and the
    /// rebuild sweeps bbox THREE times (auto_cell_size + build's world sweep + build's
    /// bucketing pass). Measure the real mix.
    #[test]
    #[ignore = "benchmark: --ignored --nocapture"]
    fn real_geometry_rebuild_cost() {
        use cad_kernel::{Arc as KArc, Circle, DObject, Ellipse, Geom, Line, Point, Polyline};
        let mut doc = Document::default();
        doc.dobjects.clear();
        let n_sets = 250_000usize;
        for k in 0..n_sets {
            let ox = (k % 500) as f64 * 500.0;
            let oy = (k / 500) as f64 * 500.0;
            doc.push(DObject::new(Geom::Line(Line {
                a: Vec2::new(ox - 40.0, oy - 20.0),
                b: Vec2::new(ox + 40.0, oy + 20.0),
            })));
            doc.push(DObject::new(Geom::Circle(Circle {
                center: Vec2::new(ox, oy),
                radius: 30.0,
            })));
            doc.push(DObject::new(Geom::Arc(KArc {
                center: Vec2::new(ox, oy),
                radius: 45.0,
                start_angle: 0.0,
                sweep_angle: std::f64::consts::PI,
            })));
            doc.push(DObject::new(Geom::Ellipse(Ellipse {
                center: Vec2::new(ox - 60.0, oy - 30.0),
                major: Vec2::new(25.0, 12.0),
                ratio: 0.55,
            })));
            doc.push(DObject::new(Geom::Point(Point {
                location: Vec2::new(ox + 60.0, oy - 40.0),
                style: 0,
                size: 0.0,
            })));
            let pv = |x: f64, y: f64| cad_kernel::PolyVertex {
                pos: Vec2::new(x, y),
                bulge: 0.0,
            };
            doc.push(DObject::new(Geom::Polyline(Polyline {
                vertices: vec![
                    pv(ox + 50.0, oy + 40.0),
                    pv(ox + 70.0, oy + 60.0),
                    pv(ox + 90.0, oy + 40.0),
                    pv(ox + 80.0, oy + 20.0),
                    pv(ox + 55.0, oy + 25.0),
                ],
                closed: true,
                widths: vec![],
            })));
        }
        println!(
            "\n=== {} dobjects (the owner's real 6-shape mix) ===",
            doc.dobjects.len()
        );

        let t = std::time::Instant::now();
        let mut acc = 0.0f64;
        for d in &doc.dobjects {
            let (a, b) = d.bbox();
            acc += a.x + b.y;
        }
        let sweep = t.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(acc);
        println!(
            "  ONE bbox() sweep            : {sweep:7.1} ms   (×3 per rebuild = {:.0} ms)",
            sweep * 3.0
        );

        let t = std::time::Instant::now();
        let cs = cad_kernel::UniformGrid::auto_cell_size(&doc.dobjects, 10.0);
        let a_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = std::time::Instant::now();
        let g = cad_kernel::UniformGrid::build(&doc.dobjects, cs);
        let b_ms = t.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(&g);
        println!("  auto_cell_size              : {a_ms:7.1} ms");
        println!("  build                       : {b_ms:7.1} ms");
        println!(
            "  REBUILD TOTAL (3 sweeps)    : {:7.1} ms   ← the dump says 386-397 ms",
            a_ms + b_ms
        );
        println!(
            "  ↳ bbox() is {:.0}% of it",
            (sweep * 3.0) / (a_ms + b_ms) * 100.0
        );

        let t = std::time::Instant::now();
        let g2 = cad_kernel::UniformGrid::build_auto(&doc.dobjects, 10.0);
        let one_ms = t.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(&g2);
        println!(
            "  build_auto (1 sweep)        : {one_ms:7.1} ms   → {:.2}× faster",
            (a_ms + b_ms) / one_ms
        );
    }

    /// THE SAME FREEZE, IN THE SELECTION PATHS — the draw loop was fixed with `sel_mask` and
    /// these two were left behind, so a crossing window still ran `selection.contains(&i)` per
    /// candidate: candidates × selected comparisons, MEASURED at 22.4 s over 500k objects and
    /// 830 ms at 100k. This is a PERFORMANCE refactor, so what has to be pinned is that the
    /// answer did not change — including the parts that are easy to get wrong.
    ///
    /// Run against a reference implementation of the old logic rather than against hand-written
    /// expectations: the two must agree exactly, on membership AND on order.
    #[test]
    fn the_fast_selection_paths_agree_with_the_slow_ones() {
        // The old add: linear `contains` per candidate, push in candidate order.
        fn slow_add(sel: &mut Vec<usize>, cands: &[usize]) {
            for &i in cands {
                if !sel.contains(&i) {
                    sel.push(i);
                }
            }
        }
        // The old remove: `position` then `Vec::remove`, one shift each.
        fn slow_remove(sel: &mut Vec<usize>, cands: &[usize]) {
            for &i in cands {
                if let Some(p) = sel.iter().position(|&x| x == i) {
                    sel.remove(p);
                }
            }
        }
        // The new remove: mark, then one retain.
        fn fast_remove(sel: &mut Vec<usize>, cands: &[usize], n: usize) {
            let mut drop = vec![false; n];
            let mut any = false;
            for &i in cands {
                if i < n && sel.contains(&i) {
                    drop[i] = true;
                    any = true;
                }
            }
            if any {
                sel.retain(|&x| x >= n || !drop[x]);
            }
        }
        // The new add: mask, push in candidate order.
        fn fast_add(sel: &mut Vec<usize>, cands: &[usize], n: usize) {
            let mut in_sel = vec![false; n];
            for &s in sel.iter() {
                if s < n {
                    in_sel[s] = true;
                }
            }
            for &i in cands {
                if i >= n || !in_sel[i] {
                    if i < n {
                        in_sel[i] = true;
                    }
                    sel.push(i);
                }
            }
        }

        let n = 400usize;
        // A starting selection that is NOT sorted and NOT contiguous — order preservation is the
        // property most likely to be lost by a `retain`, and a tidy fixture would hide it.
        let start: Vec<usize> = [317, 4, 128, 9, 250, 63, 199, 88].to_vec();
        // Candidates with duplicates and an out-of-range index, because both reach these
        // functions in practice — a stale index survives until the next reindex.
        let cands: Vec<usize> = vec![4, 4, 7, 128, 401, 9, 7, 250, 12];

        let (mut a, mut b) = (start.clone(), start.clone());
        slow_add(&mut a, &cands);
        fast_add(&mut b, &cands, n);
        assert_eq!(a, b, "add diverged (order or membership)");

        let (mut a, mut b) = (start.clone(), start.clone());
        slow_remove(&mut a, &cands);
        fast_remove(&mut b, &cands, n);
        assert_eq!(
            a, b,
            "remove diverged — retain must preserve the order of what stays"
        );

        // And on the empty and full edges.
        for base in [Vec::new(), (0..n).collect::<Vec<_>>()] {
            let (mut a, mut b) = (base.clone(), base.clone());
            slow_add(&mut a, &cands);
            fast_add(&mut b, &cands, n);
            assert_eq!(a, b, "add diverged on an edge case");

            let (mut a, mut b) = (base.clone(), base.clone());
            slow_remove(&mut a, &cands);
            fast_remove(&mut b, &cands, n);
            assert_eq!(a, b, "remove diverged on an edge case");
        }
    }

    /// ⭐ THE FREEZE, from the owner's real 1.5M session:
    ///     🐢 SLOW FRAME    27.5 ms  (draw    25.2)  candidates=143856 drawn=141766
    ///     🐢 SLOW FRAME 21817.5 ms  (draw 21815.4)  candidates=143856 drawn=141766
    /// Identical work, 800× slower — the only difference was 52,650 selected
    /// dobjects. `selection.contains(&i)` is a LINEAR Vec scan and the draw loop ran
    /// it per drawn dobject: 141,766 × 52,650 = 7.46 BILLION comparisons.
    ///
    /// This measures the mask (what the draw loop now does) against the old scan.
    #[test]
    #[ignore = "benchmark: --ignored --nocapture"]
    fn selection_lookup_mask_vs_linear_scan() {
        let drawn = 141_766usize;
        let n = 1_500_000usize;
        for sel_n in [0usize, 43_840, 52_650] {
            let selection: Vec<usize> = (0..sel_n).map(|i| i * 7 % n).collect();

            // OLD: linear Vec scan per drawn dobject
            let t = std::time::Instant::now();
            let mut hits = 0usize;
            for i in 0..drawn {
                if selection.contains(&i) {
                    hits += 1;
                }
            }
            let scan_ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(hits);

            // NEW: build the mask once, then O(1) per dobject
            let t = std::time::Instant::now();
            let mut mask = vec![false; n];
            for &i in &selection {
                mask[i] = true;
            }
            let mut hits2 = 0usize;
            for i in 0..drawn {
                if mask[i] {
                    hits2 += 1;
                }
            }
            let mask_ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(hits2);

            println!(
                "  {drawn} drawn × {sel_n:>6} selected → linear scan {scan_ms:9.1} ms · mask {mask_ms:6.2} ms  ({:.0}× faster)",
                scan_ms / mask_ms.max(0.0001)
            );
        }
    }

    /// ⭐ ZOOM/PAN slowness. `query_bbox` runs EVERY FRAME to decide what to render.
    /// It allocates O(n) per call — so panning a 1.5M drawing allocates megabytes per
    /// frame, before a single pixel is drawn.
    #[test]
    #[ignore = "benchmark: --ignored --nocapture"]
    fn query_cost_per_frame() {
        for n in [100_000usize, 1_500_000] {
            let doc = doc_with(n);
            let cs = cad_kernel::UniformGrid::auto_cell_size(&doc.dobjects, 10.0);
            let g = cad_kernel::UniformGrid::build(&doc.dobjects, cs);
            let (mn, mx) = {
                let mut a = Vec2::new(f64::MAX, f64::MAX);
                let mut b = Vec2::new(f64::MIN, f64::MIN);
                for d in &doc.dobjects {
                    let (p, q) = d.bbox();
                    a.x = a.x.min(p.x);
                    a.y = a.y.min(p.y);
                    b.x = b.x.max(q.x);
                    b.y = b.y.max(q.y);
                }
                (a, b)
            };
            println!("\n=== {n} dobjects ===");

            // ZOOMED OUT (the whole drawing) — hits the "≥80% of the grid" fast path
            let t = std::time::Instant::now();
            let mut hits = 0usize;
            for _ in 0..10 {
                hits = g.query_bbox(mn, mx).len();
            }
            let out_ms = t.elapsed().as_secs_f64() * 1000.0 / 10.0;
            println!(
                "  query zoomed OUT (whole drawing) : {out_ms:7.2} ms/frame  → {hits} candidates"
            );

            // ZOOMED IN (a small window) — the normal cell scan
            let w = (mx.x - mn.x) * 0.02;
            let h = (mx.y - mn.y) * 0.02;
            let c0 = Vec2::new(mn.x + (mx.x - mn.x) * 0.5, mn.y + (mx.y - mn.y) * 0.5);
            let t = std::time::Instant::now();
            let mut hits2 = 0usize;
            for _ in 0..10 {
                hits2 = g.query_bbox(c0, Vec2::new(c0.x + w, c0.y + h)).len();
            }
            let in_ms = t.elapsed().as_secs_f64() * 1000.0 / 10.0;
            println!(
                "  query zoomed IN  (2% window)     : {in_ms:7.2} ms/frame  → {hits2} candidates"
            );
            println!("  ↳ at 60 Hz a frame is 16.7 ms — anything above that IS the stall");
        }
    }

    /// ⭐ THE WIN, measured. A MOVE of a subset must re-bucket only that subset.
    #[test]
    #[ignore = "benchmark: --ignored --nocapture"]
    fn incremental_vs_rebuild() {
        for n in [100_000usize, 1_500_000] {
            let mut doc = doc_with(n);
            let cs = cad_kernel::UniformGrid::auto_cell_size(&doc.dobjects, 10.0);
            let mut g = cad_kernel::UniformGrid::build(&doc.dobjects, cs);

            // the owner's shape: move a scattered ~3150 of them, a SMALL delta
            let changed: Vec<usize> = (0..n).step_by(n / 3150).take(3150).collect();
            for &i in &changed {
                if let cad_kernel::Geom::Line(l) = &mut doc.dobjects[i].geom {
                    l.a.x += 1.0;
                    l.b.x += 1.0;
                }
            }

            let t = std::time::Instant::now();
            let ok = g.update(&doc.dobjects, &changed);
            let inc = t.elapsed().as_secs_f64() * 1000.0;

            let t = std::time::Instant::now();
            let cs2 = cad_kernel::UniformGrid::auto_cell_size(&doc.dobjects, 10.0);
            let g2 = cad_kernel::UniformGrid::build(&doc.dobjects, cs2);
            let full = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&g2);

            println!(
                "  {n:>9} dobj · moved {} → incremental {inc:7.2} ms (absorbed={ok}) vs rebuild {full:7.1} ms  → {:.0}× faster",
                changed.len(), full / inc.max(0.0001)
            );
        }
    }

    /// DRAWING ONE OBJECT NO LONGER REBUILDS THE WHOLE INDEX.
    ///
    /// `add_dobject` used to set `index_dirty`, which costs a full O(n) rebuild on the next
    /// query — measured at 13.4 ms per edit at 100k dobjects and 247.9 ms at 1.5M, to record the
    /// arrival of one line. An append shifts no existing index, so the grid absorbs it.
    #[test]
    fn drawing_an_object_does_not_rebuild_the_index() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        for i in 0..40 {
            app.doc.push(DObject::new(Geom::Line(Line {
                a: Vec2::new(i as f64, 0.0),
                b: Vec2::new(i as f64 + 1.0, 3.0),
            })));
        }
        app.ensure_index();
        assert!(!app.index_dirty, "the fixture must start with a live index");

        // Inside the existing extent, so the grid can take it.
        app.add_dobject(
            Geom::Line(Line {
                a: Vec2::new(5.0, 1.0),
                b: Vec2::new(6.0, 2.0),
            }),
            "test",
        );
        assert!(
            !app.index_dirty,
            "drawing one object still forces a full index rebuild",
        );

        // ABSORBED IS NOT THE SAME AS CORRECT. The new object has to be findable through the
        // index, or the speed has been bought by dropping it out of every selection.
        let idx = app.doc.dobjects.len() - 1;
        let found = app
            .index
            .as_ref()
            .expect("index")
            .query_bbox(Vec2::new(5.0, 1.0), Vec2::new(6.0, 2.0));
        assert!(
            found.contains(&(idx as u32)),
            "the drawn object is not in the index: {found:?}"
        );
    }

    /// …AND AN OBJECT DRAWN OUTSIDE THE GRID STILL FALLS BACK. The grid's bounds are fixed at
    /// build time; growing them is a rebuild, and refusing to do one would mis-bucket the object.
    #[test]
    fn drawing_far_outside_the_grid_falls_back_to_a_rebuild() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        for i in 0..10 {
            app.doc.push(DObject::new(Geom::Line(Line {
                a: Vec2::new(i as f64, 0.0),
                b: Vec2::new(i as f64 + 1.0, 1.0),
            })));
        }
        app.ensure_index();
        assert!(!app.index_dirty);
        app.add_dobject(
            Geom::Line(Line {
                a: Vec2::new(1e6, 1e6),
                b: Vec2::new(1e6 + 1.0, 1e6 + 1.0),
            }),
            "far away",
        );
        assert!(
            app.index_dirty,
            "an out-of-bounds draw must fall back to a rebuild"
        );
    }

    /// WHERE DOES THE TIME GO at 1.5M dobjects? Measures the per-EDIT and per-FRAME
    /// O(n) costs, so the answer to "why is it slow" is a number, not a hunch.
    #[test]
    #[ignore = "investigation: --ignored --nocapture"]
    fn where_does_the_time_go() {
        for n in [100_000usize, 1_500_000] {
            println!("\n=== {n} dobjects ===");
            let doc = doc_with(n);

            let t = std::time::Instant::now();
            let c = doc.clone();
            let clone_ms = t.elapsed().as_secs_f64() * 1000.0;
            println!("  undo snapshot_doc (Document clone) : {clone_ms:8.1} ms   ← PER EDIT");
            let mb = doc.approx_bytes() as f64 / (1024.0 * 1024.0);
            let budget_mb = UNDO_BUDGET_BYTES as f64 / (1024.0 * 1024.0);
            let steps_kept = (budget_mb / mb.max(1e-9))
                .floor()
                .max(UNDO_MIN_STEPS as f64);
            println!(
                "  undo history: {mb:6.1} MB per step   {:7.0} MB at the {}-step cap  ->  \
                 {:5.0} MB, {steps_kept:.0} steps within budget",
                mb * UNDO_STACK_CAP as f64,
                UNDO_STACK_CAP,
                (steps_kept * mb).min(budget_mb.max(mb * UNDO_MIN_STEPS as f64)),
            );
            std::hint::black_box(&c);

            // ensure_index() calls BOTH — so the real per-edit cost is their sum.
            let t = std::time::Instant::now();
            let cs = cad_kernel::UniformGrid::auto_cell_size(&doc.dobjects, 10.0);
            let auto_ms = t.elapsed().as_secs_f64() * 1000.0;
            let t = std::time::Instant::now();
            let g = cad_kernel::UniformGrid::build(&doc.dobjects, cs);
            let build_ms = t.elapsed().as_secs_f64() * 1000.0;
            println!("  index: auto_cell_size (bbox sweep)  : {auto_ms:8.1} ms");
            println!("  index: build (bbox sweep + bucket)  : {build_ms:8.1} ms");
            println!(
                "  index TOTAL per edit                : {:8.1} ms   ← PER EDIT",
                auto_ms + build_ms
            );

            // …AND WHAT DRAWING ONE OBJECT COSTS NOW. The rebuild above is what `index_dirty`
            // buys; this is what an absorbed append costs instead — same grid, one more line.
            {
                let mut g2 = cad_kernel::UniformGrid::build(&doc.dobjects, cs);
                let mut objs = doc.dobjects.clone();
                let (mn, _) = objs[0].bbox();
                objs.push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                    cad_kernel::Line {
                        a: mn,
                        b: cad_kernel::Vec2::new(mn.x + 0.5, mn.y + 0.5),
                    },
                )));
                let t = std::time::Instant::now();
                let ok = g2.insert_appended(&objs);
                let us = t.elapsed().as_secs_f64() * 1_000_000.0;
                println!(
                    "  index: absorb ONE appended object   : {us:8.1} µs   ← PER DRAW ({})",
                    if ok { "absorbed" } else { "REFUSED, rebuilt" },
                );
                // AND THE SECOND ONE, because the first push into a `with_capacity(n)` vector
                // reallocates 24 MB at 1.5M and that cost is a one-off, not the per-draw price.
                objs.push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                    cad_kernel::Line {
                        a: mn,
                        b: cad_kernel::Vec2::new(mn.x + 0.25, mn.y + 0.25),
                    },
                )));
                let t = std::time::Instant::now();
                g2.insert_appended(&objs);
                println!(
                    "  index: absorb a SECOND one          : {:8.1} µs   ← steady state",
                    t.elapsed().as_secs_f64() * 1_000_000.0,
                );
            }
            std::hint::black_box(&g);

            // How much of that is just bbox()? It is called THREE times per dobject
            // per rebuild: once in auto_cell_size, once in build's world sweep, once
            // in build's bucketing pass.
            let t = std::time::Instant::now();
            let mut acc = 0.0f64;
            for d in &doc.dobjects {
                let (a, b) = d.bbox();
                acc += a.x + b.y;
            }
            let one_sweep = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(acc);
            println!("  ↳ ONE bbox() sweep                  : {one_sweep:8.1} ms  (×3 per rebuild = {:.1} ms)", one_sweep * 3.0);

            let t = std::time::Instant::now();
            let mut mn = Vec2::new(f64::MAX, f64::MAX);
            let mut mx = Vec2::new(f64::MIN, f64::MIN);
            for d in &doc.dobjects {
                let (a, b) = d.bbox();
                mn.x = mn.x.min(a.x);
                mn.y = mn.y.min(a.y);
                mx.x = mx.x.max(b.x);
                mx.y = mx.y.max(b.y);
            }
            let ext_ms = t.elapsed().as_secs_f64() * 1000.0;
            println!("  full-doc bbox sweep (extents)      : {ext_ms:8.1} ms");
            std::hint::black_box((mn, mx));

            let bytes = n * 200; // rough per-dobject
            println!(
                "  ≈ resident per undo level          : {:8.1} MB",
                bytes as f64 / 1e6
            );
        }
    }
}
/// THE VIEW LIST — the faces actually drawn on, instead of fixed orthographic views.
///
/// Asked for as: "the views in cad[,] lets overhaul it. instead of showing the planes like top,
/// left right etc, lets get rid of it. now it will show only faces as planes the user draws on, the
/// user can even rename these view[s] so they can instantly look at a sketch they made. instead of
/// showing the whole side view, it will only show whatever face as a plane the user is drawing on."
/// Plus: "when i click on the face again to sketch it should show the same plane. there should be
/// an option to delete the face."

#[cfg(test)]
mod view_planes {
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

    fn wall_face() -> cad_solid::Frame {
        // A point on the +X wall, looking at it.
        cad_solid::Frame::from_point_normal(glam::Vec3::new(6.0, 3.0, 1.5), glam::Vec3::X)
    }

    /// A plane only exists once it has been DRAWN ON. Nothing is offered up front.
    #[test]
    fn the_list_starts_with_nothing_but_the_ground_plan() {
        let app = a_building();
        assert!(
            app.factory.model.sketches.is_empty(),
            "no faces until one is drawn on"
        );
        assert!(
            app.factory.session.is_none(),
            "…and the canvas is the ground plan"
        );
    }

    /// THE KEY BEHAVIOUR. Clicking the same face twice must land in the same plane with the same
    /// drawing on it — not a second, empty plane that looks like the work vanished.
    #[test]
    fn returning_to_a_face_returns_to_the_same_plane() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        let first = app.factory.session.as_ref().expect("in a sketch").plane;
        app.factory_exit_sketch();

        app.factory_enter_sketch(wall_face());
        let again = app.factory.session.as_ref().expect("in a sketch").plane;
        assert_eq!(first, again, "the same face must reopen the same plane");
        assert_eq!(
            app.factory.model.sketches.len(),
            1,
            "and must NOT make a second one"
        );
    }

    /// …and so must picking it by name out of the list.
    #[test]
    fn opening_a_plane_by_name_is_the_same_plane() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        let idx = app.factory.session.as_ref().unwrap().plane;
        app.factory_open_plane(None); // back to the ground plan
        assert!(app.factory.session.is_none());

        app.factory_open_plane(Some(idx));
        assert_eq!(app.factory.session.as_ref().map(|s| s.plane), Some(idx));
    }

    /// Named after the object the face belongs to, so an unrenamed plane is still findable.
    #[test]
    fn a_new_plane_is_named_after_what_it_is_a_face_of() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        let name = app.factory.model.sketches[0].name.clone();
        assert!(name.starts_with("Building 1"), "got {name:?}");
        assert!(name.contains("face 1"), "and says which face: {name:?}");

        // A second face of the same object counts on.
        app.factory_exit_sketch();
        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(
            glam::Vec3::new(3.0, 0.0, 1.5),
            glam::Vec3::Y,
        ));
        let second = app.factory.model.sketches[1].name.clone();
        assert!(second.contains("face 2"), "got {second:?}");
    }

    /// A room is the thing with a name the USER chose, so it wins over the solid it was carved out
    /// of — that is the name they will look for.
    #[test]
    fn a_face_inside_a_named_room_takes_the_rooms_name() {
        let mut app = a_building();
        app.factory
            .add_room(&vec![
                glam::Vec2::new(1.0, 1.0),
                glam::Vec2::new(5.0, 1.0),
                glam::Vec2::new(5.0, 5.0),
                glam::Vec2::new(1.0, 5.0),
                glam::Vec2::new(1.0, 1.0),
            ])
            .expect("room");
        app.factory.rooms[0].name = "Kitchen".into();
        app.factory.recompute();

        app.factory_enter_sketch(cad_solid::Frame::from_point_normal(
            glam::Vec3::new(3.0, 3.0, 1.2),
            glam::Vec3::X,
        ));
        let name = app.factory.model.sketches[0].name.clone();
        assert!(name.starts_with("Kitchen"), "got {name:?}");
    }

    /// Renaming is the whole point — "so they can instantly look at a sketch they made".
    #[test]
    fn a_plane_can_be_renamed() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        app.factory.model.sketches[0].name = "Kitchen elevation".into();
        assert_eq!(app.factory.model.sketches[0].name, "Kitchen elevation");
    }

    /// Deleting a face takes its drawing with it, and is one Ctrl+Z away.
    #[test]
    fn a_plane_can_be_deleted() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        app.factory_exit_sketch();
        assert_eq!(app.factory.model.sketches.len(), 1);

        let id = app.factory.model.sketches[0].id;
        app.factory_delete_plane(id);
        assert!(app.factory.model.sketches.is_empty(), "the plane is gone");
        app.run_command("undo");
        assert_eq!(
            app.factory.model.sketches.len(),
            1,
            "…and undo brings it back"
        );
    }

    /// Deleting the plane you are STANDING ON must leave it first, or the exit would write the
    /// drawing back into a plane that is no longer there.
    #[test]
    fn deleting_the_open_plane_leaves_it_first() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        let open = app.factory.session.as_ref().expect("a session").plane;
        app.factory_delete_plane(open);
        assert!(app.factory.session.is_none(), "it must not still be open");
        assert!(app.factory.model.sketches.is_empty());
    }

    /// Deleting an EARLIER plane slides every plane after it down a row. An open session survived
    /// that only because `factory_delete_plane` carried a hand-written fixup; it now survives it
    /// because a plane is named by id, and an id does not move when its neighbour goes.
    #[test]
    fn deleting_an_earlier_plane_keeps_the_open_one_open() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face()); // the first plane
        app.factory_exit_sketch();
        let first = app.factory.model.sketches[0].id;
        let second =
            cad_solid::Frame::from_point_normal(glam::Vec3::new(3.0, 0.0, 1.5), glam::Vec3::Y);
        app.factory_enter_sketch(second); // the second plane — and stay in it
        let open = app.factory.session.as_ref().expect("a session").plane;
        assert_ne!(
            open, first,
            "the fixture must be standing on the SECOND plane"
        );
        let open_name = app
            .factory
            .model
            .sketch_by_id(open)
            .expect("open plane")
            .name
            .clone();

        app.factory_delete_plane(first);
        assert_eq!(
            app.factory.session.as_ref().map(|s| s.plane),
            Some(open),
            "the open plane changed identity when a different one was deleted",
        );
        assert_eq!(
            app.factory
                .model
                .sketch_by_id(open)
                .expect("still there")
                .name,
            open_name,
            "and it is still the SAME plane, not the one that was deleted",
        );
    }

    /// A PENDING RENAME NAMES A PLANE, NOT A ROW.
    ///
    /// The rename dialog is a plain window and the view list behind it stays live, so this is an
    /// ordinary sequence: start renaming one plane, tidy up another, press Rename. While the
    /// pending rename held a ROW NUMBER, deleting an earlier plane slid a different plane into
    /// that row and the rename landed on it — silently renaming something the user never opened
    /// the dialog for. This is the one reference to a plane that never got the fixup the session
    /// had, which is why the fixup was the wrong answer.
    #[test]
    fn a_pending_rename_follows_its_plane_when_another_is_deleted() {
        let mut app = a_building();
        // Three planes, so that after a delete the target's OLD row still exists and can be hit.
        let frames = [
            wall_face(),
            cad_solid::Frame::from_point_normal(glam::Vec3::new(3.0, 0.0, 1.5), glam::Vec3::Y),
            cad_solid::Frame::from_point_normal(glam::Vec3::new(0.0, 3.0, 1.5), glam::Vec3::X),
        ];
        for f in frames {
            app.factory_enter_sketch(f);
            app.factory_exit_sketch();
        }
        assert_eq!(
            app.factory.model.sketches.len(),
            3,
            "three planes in the fixture"
        );
        let ids: Vec<u32> = app.factory.model.sketches.iter().map(|s| s.id).collect();
        let (first, target, bystander) = (ids[0], ids[1], ids[2]);
        let bystander_name = app
            .factory
            .model
            .sketch_by_id(bystander)
            .unwrap()
            .name
            .clone();

        // Start renaming the MIDDLE plane, then delete the first one — the middle plane slides
        // into row 0 and the last one into row 1, which is the row the rename was opened on.
        app.factory.rename_plane = Some((target, "Kitchen elevation".into()));
        app.factory_delete_plane(first);
        app.factory_commit_plane_rename("Kitchen elevation");

        assert_eq!(
            app.factory
                .model
                .sketch_by_id(target)
                .expect("the target survives")
                .name,
            "Kitchen elevation",
            "the rename did not reach the plane the dialog was opened on",
        );
        assert_eq!(
            app.factory
                .model
                .sketch_by_id(bystander)
                .expect("still there")
                .name,
            bystander_name,
            "a plane nobody opened the rename dialog for was renamed",
        );
    }

    /// AND DELETING THE PLANE BEING RENAMED CLOSES THE DIALOG. There is nothing left to rename,
    /// and leaving it open would hold a live text box over a plane that no longer exists.
    #[test]
    fn deleting_the_plane_being_renamed_closes_the_dialog() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        app.factory_exit_sketch();
        let id = app.factory.model.sketches[0].id;

        app.factory.rename_plane = Some((id, "gone".into()));
        app.factory_delete_plane(id);
        assert!(
            app.factory.rename_plane.is_none(),
            "the dialog outlived its plane"
        );
    }

    /// EVERY PLANE HAS ITS OWN ID. Ids are handed out from a counter that never rewinds, so two
    /// planes cannot share one and a deleted plane's id is never handed to its replacement — the
    /// same rule feature ids follow, for the same reason: an id that comes back lets a stale
    /// reference resolve, which is worse than one that fails.
    #[test]
    fn plane_ids_are_unique_and_never_reused() {
        let mut app = a_building();
        app.factory_enter_sketch(wall_face());
        app.factory_exit_sketch();
        let gone = app.factory.model.sketches[0].id;
        app.factory_delete_plane(gone);

        app.factory_enter_sketch(wall_face());
        app.factory_exit_sketch();
        let fresh = app.factory.model.sketches[0].id;
        assert_ne!(
            fresh, gone,
            "the new plane inherited the deleted plane's identity"
        );
    }
}
/// TWO UNIT SPACES, ONE APP.
///
/// The 2D plan is measured in whatever the drawing declares — millimetres, for an architectural
/// DXF. A face sketch is a `Document` of its OWN and its coordinates are METRES. `self.doc`
/// alternates between the two as a sketch is opened and closed, and anything carried across
/// without conversion changes meaning by a factor of a thousand with nothing to report it.
///
/// A metre-declared plan and a face sketch are byte-identical `{1.0, Declared}`, so NOTHING ABOUT
/// THE NUMBER can tell them apart — every fix here works from the RATIO between the plan document
/// and the active one, which is exactly 1 when no sketch is open.

#[cfg(test)]
mod two_unit_spaces {
    use super::*;

    /// A plan in millimetres with a wall on it, and nothing else.
    fn mm_plan() -> CadApp {
        let mut app = CadApp::default();
        app.doc.units =
            cad_kernel::Units::from_metres_per_unit(0.001, cad_kernel::UnitSource::Declared);
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(3000.0, 0.0), // a 3 m wall, in millimetres
                },
            )));
        app
    }

    // ── THE GRID ───────────────────────────────────────────────────────────────────────────

    /// `GrdSpc` IS THE SAME PHYSICAL SPACING IN BOTH SPACES. 10 on a millimetre plan is 10 mm; on
    /// a face sketch measured in metres the same stored 10 was a TEN METRE grid — three orders of
    /// magnitude coarser than the wall being drawn on, and a snap rounding every click to the
    /// nearest ten metres.
    #[test]
    fn the_grid_is_the_same_size_on_a_plane_as_on_the_plan() {
        let mut app = mm_plan();
        app.env.GrdSpc = 10.0; // 10 mm
        let on_plan = app.grid_spacing();
        assert!(
            (on_plan - 10.0).abs() < 1e-9,
            "the plan's own grid changed: {on_plan}"
        );

        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        let in_sketch = app.grid_spacing();
        // The sketch is in metres, so the same 10 mm is 0.01 of a unit there.
        assert!(
            (in_sketch - 0.01).abs() < 1e-9,
            "a 10 mm grid became {in_sketch} units in a face sketch — {}x out",
            in_sketch / 0.01,
        );
    }

    /// AND NOTHING CHANGES WHEN THERE IS NO SKETCH. The ratio is exactly 1, so every existing
    /// drawing snaps and draws its grid precisely as before — the property that makes this safe
    /// to add at all.
    #[test]
    fn the_grid_is_untouched_with_no_sketch_open() {
        for unit in [1.0_f64, 0.001, 0.01, 25.4 / 1000.0] {
            let mut app = CadApp::default();
            app.doc.units =
                cad_kernel::Units::from_metres_per_unit(unit, cad_kernel::UnitSource::Declared);
            app.env.GrdSpc = 7.5;
            assert_eq!(
                app.grid_spacing(),
                7.5,
                "at {unit} m/unit the grid moved without a sketch being open",
            );
        }
    }

    // ── THE CLIPBOARD ──────────────────────────────────────────────────────────────────────

    /// A 3 m WALL STAYS 3 m ACROSS THE SPACES. Copied off a millimetre plan it is 3000 units;
    /// pasted into a metre-measured sketch unconverted it became a THREE KILOMETRE wall — no
    /// error, the view simply jumps and the drawing is somewhere past the horizon.
    #[test]
    fn a_wall_copied_from_the_plan_keeps_its_size_on_a_plane() {
        let mut app = mm_plan();
        // The LAST index, not 0 — a default `CadApp` is not an empty drawing.
        app.selection = vec![app.doc.dobjects.len() - 1];
        app.copy_selection();
        assert_eq!(
            app.clipboard_dobjects.len(),
            1,
            "the fixture must copy something"
        );

        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        let before = app.doc.dobjects.len();
        app.start_paste();
        app.commit_paste(Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0));
        assert_eq!(app.doc.dobjects.len(), before + 1, "nothing was pasted");

        let g = &app.doc.dobjects[before].geom;
        let (mn, mx) = g.bbox();
        let len = mx.x - mn.x;
        assert!(
            (len - 3.0).abs() < 1e-6,
            "a 3 m wall pasted into a metre sketch as {len} units — {:.0}x out",
            len / 3.0,
        );
    }

    /// AND A PASTE INSIDE ONE DOCUMENT IS UNTOUCHED — the identity, which is every copy and paste
    /// anybody has ever done in this app.
    #[test]
    fn pasting_within_one_drawing_is_unchanged() {
        let mut app = mm_plan();
        app.selection = vec![app.doc.dobjects.len() - 1];
        app.copy_selection();
        let before = app.doc.dobjects.len();
        app.start_paste();
        app.commit_paste(Vec2::new(0.0, 0.0), Vec2::new(500.0, 0.0));

        let g = &app.doc.dobjects[before].geom;
        let (mn, mx) = g.bbox();
        assert!(
            ((mx.x - mn.x) - 3000.0).abs() < 1e-6,
            "a same-document paste rescaled the geometry to {}",
            mx.x - mn.x,
        );
        assert!(
            (mn.x - 500.0).abs() < 1e-6,
            "…and it landed at {}, not the destination",
            mn.x
        );
    }

    /// THE OTHER DIRECTION TOO. Drawn on a plane in metres, pasted onto a millimetre plan — the
    /// conversion is a ratio, and a ratio that only works one way round is a coincidence.
    #[test]
    fn geometry_drawn_on_a_plane_keeps_its_size_back_on_the_plan() {
        let mut app = mm_plan();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        // A 2 m line, drawn in the sketch's metres.
        app.doc
            .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                cad_kernel::Line {
                    a: Vec2::new(0.0, 0.0),
                    b: Vec2::new(2.0, 0.0),
                },
            )));
        let i = app.doc.dobjects.len() - 1;
        app.selection = vec![i];
        app.copy_selection();
        app.factory_exit_sketch();

        let before = app.doc.dobjects.len();
        app.start_paste();
        app.commit_paste(Vec2::new(0.0, 0.0), Vec2::new(0.0, 0.0));
        let (mn, mx) = app.doc.dobjects[before].geom.bbox();
        assert!(
            ((mx.x - mn.x) - 2000.0).abs() < 1e-3,
            "a 2 m line pasted onto a millimetre plan as {} units, not 2000",
            mx.x - mn.x,
        );
    }
}
#[cfg(test)]
mod a_debug_assert_never_does_the_work {
    /// The rule is narrow on purpose. A `debug_assert!` over a pure predicate is fine and there
    /// are plenty of those; what is banned is a MUTATION inside one.
    #[test]
    fn nothing_is_mutated_inside_a_debug_assert() {
        // Verbs that change the model rather than ask it a question. Spelled without their
        // opening bracket so this list does not trip its own check.
        const MUTATORS: [&str; 6] = [
            "insert_after",
            "insert_at",
            ".push",
            ".remove",
            ".pop",
            ".take",
        ];
        for (name, src) in [
            ("mod.rs", include_str!("../mod.rs")),
            ("factory.rs", include_str!("../../factory.rs")),
        ] {
            let mut from = 0usize;
            while let Some(rel) = src[from..].find("debug_assert!(") {
                let open = from + rel + "debug_assert!".len();
                // Balanced scan to the closing bracket, so a call spread over several lines is
                // covered as well as a one-liner.
                let mut depth = 0i32;
                let mut end = open;
                for (i, c) in src[open..].char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = open + i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let inside = &src[open..end];
                let line = src[..open].lines().count();
                for verb in MUTATORS {
                    assert!(
                        !inside.contains(&format!("{verb}(")),
                        "{name}:{line}: `{verb}` is called inside a debug_assert!, so it does not \
                         happen in a release build:\n    {}",
                        inside.trim(),
                    );
                }
                from = end.max(open + 1);
            }
        }
    }
}
/// WHAT YOU SEE IS NOT WHAT FEEDS EXTRUDE — and both halves of that matter.
///
/// The 3D viewport flattened 2D geometry through `cad_solid::geom_outlines_scaled`, which knows
/// Line, Circle, Arc, Ellipse, EllipseArc, Polyline and Point and returns nothing for anything
/// else. A spline, a wall and an imported block therefore existed in the drawing and were simply
/// absent from the 3D view — an imported plan made of blocks showed as very nearly nothing.
///
/// The fix is a SECOND flattener rather than a wider one. `geom_outlines_scaled` also answers
/// "what may Extrude and Make-3D-wall consume?", and a block's contents are full of closed loops:
/// widening it would make Make-building start extruding the furniture. The guard test below is
/// the one that would catch that, and it is the reason this is two functions.

#[cfg(test)]
mod the_viewport_sees_what_the_drawing_has {
    use super::*;

    fn app_with(geoms: Vec<Geom>) -> CadApp {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        for g in geoms {
            app.doc.push(DObject::new(g));
        }
        app
    }

    /// An unrotated, unscaled instance of `id` at `insert`.
    fn block_ref(id: u32, insert: Vec2) -> cad_kernel::BlockRef {
        cad_kernel::BlockRef {
            block: id,
            insert,
            scale: 1.0,
            scale_y: 1.0,
            rotation: 0.0,
            mirror_x: false,
            param_values: [0.0; cad_kernel::MAX_BLOCK_PARAMS],
            attr_values: Vec::new(),
        }
    }

    /// A closed 2 x 2 square as a block definition, plus one reference to it at `at`.
    fn block_with_a_square(app: &mut CadApp, at: Vec2) -> usize {
        let mut blk = cad_kernel::Block {
            name: "FURNITURE".into(),
            base: Vec2::ZERO,
            dobjects: Vec::new(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        };
        blk.dobjects.push(DObject::new(Geom::Polyline(Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(0.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(2.0, 0.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(2.0, 2.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(0.0, 2.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        })));
        let id = app.doc.blocks.add(blk);
        app.doc
            .push(DObject::new(Geom::BlockRef(block_ref(id, at))))
    }

    /// A SPLINE IS DRAWN IN 3D. Traced curves are how a real plan describes anything organic,
    /// and the viewport showed nothing at all where one was.
    #[test]
    fn a_spline_appears_in_the_3d_view() {
        let app = app_with(vec![Geom::Spline(Spline::new_bspline(
            3,
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 4.0),
                Vec2::new(5.0, -2.0),
                Vec2::new(8.0, 1.0),
            ],
        ))]);
        assert!(
            !app.factory.plan_lines(&app.doc, 0.0).is_empty(),
            "a spline in the drawing produced no 3D geometry",
        );
    }

    /// A WALL IS DRAWN AS A WALL — both faces, not a bare centreline.
    #[test]
    fn a_wall_appears_in_the_3d_view_with_both_faces() {
        let app = app_with(vec![Geom::Wall(Wall {
            start: Vec2::new(0.0, 0.0),
            end: Vec2::new(10.0, 0.0),
            thickness: 0.2,
            style: 0,
            bulge: 0.0,
        })]);
        let lines = app.factory.plan_lines(&app.doc, 0.0);
        assert!(
            !lines.is_empty(),
            "a wall in the drawing produced no 3D geometry"
        );
        // Two faces, either side of y = 0 — a centreline-only fallback would put every
        // vertex on it.
        let (lo, hi) = lines.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
            (lo.min(v.y), hi.max(v.y))
        });
        assert!(
            hi - lo > 1e-4,
            "the wall was drawn as a single centreline, not as two faces"
        );
    }

    /// AN IMPORTED BLOCK IS DRAWN. A DXF floor plan arrives as block references; without this
    /// the 3D view of an imported drawing is very nearly empty.
    #[test]
    fn an_imported_block_appears_in_the_3d_view() {
        let mut app = app_with(vec![]);
        block_with_a_square(&mut app, Vec2::new(20.0, 5.0));
        let lines = app.factory.plan_lines(&app.doc, 0.0);
        assert!(
            !lines.is_empty(),
            "a block reference produced no 3D geometry"
        );
        // Drawn WHERE IT WAS INSERTED, not at the block's own origin.
        let k = app.doc.units.metres_per_unit as f32;
        assert!(
            lines.iter().any(|v| v.x > 19.0 * k),
            "the block was drawn at the origin instead of at its insertion point",
        );
    }

    /// THE GUARD. Everything above is display; none of it may reach the construction path.
    ///
    /// A block's contents are closed loops, and Extrude / Make-building take every closed loop
    /// they are handed. If the two flatteners were ever merged, dropping a furniture block on a
    /// plan and pressing Make-building would extrude the furniture into the building.
    #[test]
    fn a_block_is_visible_but_not_buildable() {
        let mut app = app_with(vec![]);
        block_with_a_square(&mut app, Vec2::new(20.0, 5.0));

        assert!(
            !app.factory.plan_lines(&app.doc, 0.0).is_empty(),
            "the fixture must be VISIBLE, or the assertion below proves nothing",
        );
        assert!(
            CadApp::closed_loops_of(&app.doc).is_empty(),
            "a block reference reached the construction path — Make-building would extrude it",
        );
        let br = &app.doc.dobjects[0].geom;
        assert!(
            !is_promotable_to_wall(br),
            "a block reference must not promote to a wall"
        );
    }

    /// ...and the buildable types are still buildable, so the guard above is not simply "nothing
    /// can be built any more".
    #[test]
    fn a_drawn_square_is_still_buildable() {
        let app = app_with(vec![Geom::Polyline(Polyline {
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
        })]);
        assert_eq!(
            CadApp::closed_loops_of(&app.doc).len(),
            1,
            "an ordinary closed polyline stopped being buildable",
        );
    }

    /// A BLOCK CAN BE INSERTED ONTO A PLANE. The block TABLE lives in the drawing, and a sketch
    /// document was built from `Document::default()` — so on a plane there were no blocks to
    /// insert, and a reference pushed there pointed at a definition that did not exist.
    ///
    /// The same reasoning, and the same fix, as the layer table before it: a block definition is
    /// a property of the DRAWING, not of one plane of it.
    #[test]
    fn a_block_can_be_inserted_onto_a_plane_and_is_drawn_there() {
        let mut app = app_with(vec![]);
        let mut blk = cad_kernel::Block {
            name: "CHAIR".into(),
            base: Vec2::ZERO,
            dobjects: Vec::new(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        };
        blk.dobjects.push(DObject::new(Geom::Line(Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(1.0, 1.0),
        })));
        let id = app.doc.blocks.add(blk);
        let blocks_before = app.doc.blocks.len();

        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert_eq!(
            app.doc.blocks.len(),
            blocks_before,
            "the drawing's blocks did not come with us onto the plane — nothing to insert",
        );
        app.add_dobject(Geom::BlockRef(block_ref(id, Vec2::new(3.0, 3.0))), "test");
        assert!(
            !app.factory.live_sketch_lines(&app.doc).is_empty(),
            "a block inserted on a plane is not drawn in 3D",
        );
    }

    /// ...AND A BLOCK DEFINED ON A PLANE COMES BACK OUT, exactly as a layer created there does.
    /// Otherwise the plane is a place where block work quietly disappears on Finish.
    #[test]
    fn a_block_defined_on_a_plane_returns_to_the_drawing() {
        let mut app = app_with(vec![]);
        let before = app.doc.blocks.len();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        let mut blk = cad_kernel::Block {
            name: "MADE-ON-A-PLANE".into(),
            base: Vec2::ZERO,
            dobjects: Vec::new(),
            smart: false,
            params: Vec::new(),
            cut_edges: Vec::new(),
        };
        blk.dobjects.push(DObject::new(Geom::Line(Line {
            a: Vec2::ZERO,
            b: Vec2::new(1.0, 0.0),
        })));
        app.doc.blocks.add(blk);
        app.factory_exit_sketch();
        assert_eq!(
            app.doc.blocks.len(),
            before + 1,
            "a block defined while drawing on a plane was lost when the sketch closed",
        );
    }
}
/// THE 3D LINE CACHE MUST NEVER SHOW YOU GEOMETRY THAT IS NO LONGER THERE.
///
/// A stale render cache is the worst shape of bug in this file: the picture is wrong, nothing
/// errors, and the user's own drawing is the thing lying to them. So every input to `lines_sig`
/// gets a test that changes it and asserts the cache notices — and the general test at the end
/// asserts the cached buffers equal what the uncached builders would have produced, which is the
/// property all of it exists to preserve.

#[cfg(test)]
mod the_line_cache_cannot_go_stale {
    use super::*;

    fn app_with_a_plan() -> CadApp {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        for i in 0..8 {
            let x = i as f64 * 3.0;
            app.doc.push(DObject::new(Geom::Line(Line {
                a: Vec2::new(x, 0.0),
                b: Vec2::new(x + 2.0, 4.0),
            })));
        }
        app.factory.show_plan = true;
        app.factory.plan_xray = false;
        app
    }

    /// Build the cache, run `change`, and report whether the cache rebuilt.
    fn rebuilds_after(app: &mut CadApp, change: impl FnOnce(&mut CadApp)) -> bool {
        app.refresh_cached_lines();
        assert!(
            !app.refresh_cached_lines(),
            "the cache is not settled — it rebuilds every call"
        );
        change(app);
        app.refresh_cached_lines()
    }

    /// THE POINT OF THE WHOLE THING: a settled cache does not rebuild. Without this the rest of
    /// the tests could all pass on a cache that never caches anything.
    #[test]
    fn a_settled_cache_does_not_rebuild() {
        let mut app = app_with_a_plan();
        assert!(app.refresh_cached_lines(), "the first call must build");
        assert!(
            !app.refresh_cached_lines(),
            "nothing changed, so nothing should rebuild"
        );
        assert!(!app.refresh_cached_lines(), "…still nothing");
        assert!(
            !app.cached_plan_lines.is_empty(),
            "the fixture must produce plan lines"
        );
    }

    /// THE CACHED BUFFERS ARE WHAT THE BUILDERS WOULD HAVE RETURNED. Equality on the actual
    /// vertices, because "it rebuilt at the right times" is worthless if what it rebuilt is wrong.
    #[test]
    fn the_cache_holds_exactly_what_the_builders_produce() {
        let mut app = app_with_a_plan();
        app.refresh_cached_lines();

        let plan = CadApp::plan_doc_of(app.factory.session.as_ref(), &app.doc);
        let z = app.factory.active_base_z();
        let fresh_plan = app.factory.plan_lines(plan, z);
        let fresh_sketch = app.factory.sketch_lines(&app.doc);

        assert_eq!(
            app.cached_plan_lines.len(),
            fresh_plan.len(),
            "plan line count differs"
        );
        assert_eq!(
            app.cached_sketch_lines.len(),
            fresh_sketch.len(),
            "sketch line count differs"
        );

        // COMPARED AS A SET, NOT IN ORDER, and the change is deliberate. The cached plan is sorted
        // into world-space cells for view culling, so a cell is one contiguous run — which
        // reorders the buffer. Order carries no meaning within the plan: every line is the same
        // colour at the same height. (It DOES carry meaning between builders — the picked-face
        // outline has to be emitted after the sketches — and that ordering is still exact.)
        //
        // This assertion used to be positional and kept passing after the bucketing landed,
        // because a fixture small enough to fall in one cell sorts stably. It was asserting a
        // property the code no longer promises, and would have failed later for a reason that had
        // nothing to do with whatever change tripped it.
        let key = |v: &crate::light3d::V3| (v.x.to_bits(), v.y.to_bits(), v.z.to_bits());
        let cached: std::collections::BTreeSet<_> = app.cached_plan_lines.iter().map(key).collect();
        let built: std::collections::BTreeSet<_> = fresh_plan.iter().map(key).collect();
        assert_eq!(
            cached, built,
            "the cached plan is not the geometry the builder produces"
        );
    }

    // ---- one per input to `lines_sig` -------------------------------------------------------

    /// Drawing something. This is the one that goes through `touch_view`, i.e. the sixty-odd
    /// ordinary edit sites.
    #[test]
    fn drawing_an_object_invalidates_it() {
        let mut app = app_with_a_plan();
        assert!(
            rebuilds_after(&mut app, |a| {
                a.add_dobject(
                    Geom::Line(Line {
                        a: Vec2::new(0.0, 9.0),
                        b: Vec2::new(5.0, 9.0),
                    }),
                    "test",
                );
            }),
            "a new object did not invalidate the cached plan"
        );
    }

    /// Moving one. The count is unchanged, so a cache keyed on `dobjects.len()` alone would
    /// happily serve the old position — which is the classic way this bug appears.
    #[test]
    fn moving_an_object_invalidates_it() {
        let mut app = app_with_a_plan();
        assert!(
            rebuilds_after(&mut app, |a| {
                a.snapshot_doc();
                a.doc.dobjects[0] = a.doc.dobjects[0].translated(Vec2::new(50.0, 50.0));
                a.touch_view();
            }),
            "a moved object did not invalidate the cache — the plan would draw where it WAS"
        );
    }

    /// Hiding the plan.
    #[test]
    fn toggling_the_plan_off_invalidates_it() {
        let mut app = app_with_a_plan();
        assert!(
            rebuilds_after(&mut app, |a| a.factory.show_plan = false),
            "hiding the plan did not invalidate the cache"
        );
        assert!(
            app.cached_plan_lines.is_empty(),
            "a hidden plan must cache as nothing"
        );
    }

    /// Switching to x-ray, which moves the plan to a different renderer entirely.
    #[test]
    fn switching_to_xray_invalidates_it() {
        let mut app = app_with_a_plan();
        assert!(
            rebuilds_after(&mut app, |a| a.factory.plan_xray = true),
            "x-ray did not invalidate the depth-tested copy"
        );
        assert!(
            app.cached_plan_lines.is_empty(),
            "in x-ray the depth-tested plan must be empty"
        );
    }

    /// Changing storey, which is the height the plan is drawn AT.
    #[test]
    fn changing_the_active_storey_invalidates_it() {
        let mut app = app_with_a_plan();
        let before = app.factory.active_base_z();
        assert!(
            rebuilds_after(&mut app, |a| {
                a.factory.storeys.push(crate::factory::Storey {
                    name: "First".into(),
                    height: 3.0,
                });
                a.factory.active_storey = a.factory.storeys.len() - 1;
            }),
            "the plan did not move to the new storey's height"
        );
        assert!(
            app.factory.active_base_z() > before + 1e-4,
            "the fixture must actually raise the plan, or this proves nothing",
        );
    }

    /// Redeclaring the drawing's unit. Not one coordinate changes; every line moves.
    #[test]
    fn redeclaring_the_unit_invalidates_it() {
        let mut app = app_with_a_plan();
        // A default document is ALREADY millimetre space, so redeclaring millimetres
        // would change nothing — first move the doc to metres, then the redeclaration
        // back to millimetres is what must invalidate the cache.
        app.doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::M,
            cad_kernel::UnitSource::User,
        );
        assert!(
            rebuilds_after(&mut app, |a| {
                a.doc.units = cad_kernel::Units::from_metres_per_unit(
                    cad_kernel::Units::MM,
                    cad_kernel::UnitSource::User,
                );
            }),
            "a unit change did not invalidate the cache — every line is at the wrong scale"
        );
    }

    /// Opening a plane. Its drawing moves from `sketch_lines` to `live_sketch_lines`, so a stale
    /// cache draws it twice: once dimmed where it was, once live.
    #[test]
    fn entering_a_sketch_invalidates_it() {
        let mut app = app_with_a_plan();
        assert!(
            rebuilds_after(&mut app, |a| {
                a.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
            }),
            "entering a plane did not invalidate the finished-plane lines"
        );
    }

    /// …and closing it moves the drawing back the other way.
    #[test]
    fn leaving_a_sketch_invalidates_it() {
        let mut app = app_with_a_plan();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert!(
            rebuilds_after(&mut app, |a| a.factory_exit_sketch()),
            "leaving a plane did not invalidate the cache"
        );
    }

    /// A 3D model edit, which reaches the cache through the factory's own version counter —
    /// `sketch_lines` resolves colour through the layer table and the plan sits on the storey.
    #[test]
    fn a_model_rebuild_invalidates_it() {
        let mut app = app_with_a_plan();
        assert!(
            rebuilds_after(&mut app, |a| {
                a.factory.add_box();
                a.factory.recompute();
            }),
            "a 3D model change did not invalidate the cache"
        );
    }

    /// A PLANE'S DRAWING CAN CHANGE WHILE IT IS NOT THE OPEN ONE, and that is the case nothing
    /// else notices. `factory_clear_sketch_on` wipes a finished plane after a sweep consumes it:
    /// no session index changes, no 2D edit happens, and `view_seq` would not move — so the 3D
    /// view would go on drawing a sketch that has been deleted.
    #[test]
    fn clearing_a_finished_plane_invalidates_it() {
        let mut app = app_with_a_plan();
        // A plane with something on it, closed again so it is drawn by `sketch_lines`.
        let frame = crate::factory::FactoryState::ground_frame();
        app.factory_enter_sketch(frame);
        app.add_dobject(
            Geom::Circle(Circle {
                center: Vec2::new(1.0, 1.0),
                radius: 2.0,
            }),
            "test",
        );
        app.factory_exit_sketch();
        app.refresh_cached_lines();
        assert!(
            !app.cached_sketch_lines.is_empty(),
            "the fixture must leave a drawing on a finished plane, or this proves nothing",
        );

        assert!(
            rebuilds_after(&mut app, |a| a.factory_clear_sketch_on(frame)),
            "clearing a finished plane did not invalidate the cached sketch lines"
        );
        assert!(
            app.cached_sketch_lines.is_empty(),
            "the cleared plane is still being drawn — the cache is showing deleted geometry",
        );
    }
}
/// THE UNDO HISTORY IS BOUNDED BY MEMORY, NOT BY A COUNT OF STEPS.
///
/// A step is a whole document. Measured before this existed: 15.7 MB per step at 100k dobjects —
/// a gigabyte of history — and 240.2 MB at 1.5M, i.e. **15.4 GB** at the old 64-step cap. A count
/// is simply the wrong unit for something whose steps vary by four orders of magnitude.

#[cfg(test)]
mod the_undo_history_is_bounded_by_memory {
    use super::*;

    /// A document with `n` polylines of `pts` points each — the cheap way to make a snapshot big.
    fn doc_of(n: usize, pts: usize) -> Document {
        let mut d = Document::default();
        d.dobjects.clear();
        for i in 0..n {
            let vertices: Vec<cad_kernel::PolyVertex> = (0..pts)
                .map(|k| cad_kernel::PolyVertex {
                    pos: Vec2::new(i as f64 + k as f64, k as f64),
                    bulge: 0.0,
                })
                .collect();
            d.push(DObject::new(Geom::Polyline(cad_kernel::Polyline {
                vertices,
                closed: false,
                widths: Vec::new(),
            })));
        }
        d
    }

    fn stack_bytes(app: &CadApp) -> usize {
        app.undo_stack.iter().map(|s| s.approx_bytes()).sum()
    }

    /// A SMALL DOCUMENT STILL GETS THE FULL COUNT. The budget must not quietly shorten history
    /// for the ordinary case it was never about.
    #[test]
    fn a_small_document_keeps_the_full_step_cap() {
        let mut app = CadApp::default();
        app.doc = doc_of(20, 4);
        for _ in 0..(UNDO_STACK_CAP + 10) {
            app.snapshot_doc();
        }
        assert_eq!(
            app.undo_stack.len(),
            UNDO_STACK_CAP,
            "a small document lost history it should keep"
        );
        assert!(
            stack_bytes(&app) < UNDO_BUDGET_BYTES,
            "…and is nowhere near the budget"
        );
    }

    /// A BIG DOCUMENT IS EVICTED BY SIZE, long before it reaches the step cap.
    #[test]
    fn a_large_document_is_bounded_by_the_byte_budget() {
        let mut app = CadApp::default();
        // ~40 MB a step, so the 512 MB budget bites at about a dozen steps — well inside the cap.
        app.doc = doc_of(200_000, 8);
        let per_step = app.doc.approx_bytes();
        assert!(
            per_step > 8 << 20,
            "the fixture must be big enough to matter: {per_step} B"
        );

        for _ in 0..UNDO_STACK_CAP {
            app.snapshot_doc();
        }
        assert!(
            app.undo_stack.len() < UNDO_STACK_CAP,
            "the step cap was reached before the byte budget — the budget is doing nothing",
        );
        assert!(
            stack_bytes(&app) <= UNDO_BUDGET_BYTES,
            "the history is over budget: {} MB",
            stack_bytes(&app) / (1024 * 1024),
        );
    }

    /// CTRL+Z ALWAYS DOES SOMETHING. One snapshot of a large enough document exceeds the budget on
    /// its own; evicting on size alone would leave an empty stack and an undo that silently does
    /// nothing — indistinguishable, to the user, from an edit that never registered.
    #[test]
    fn undo_survives_a_document_bigger_than_the_whole_budget() {
        let mut app = CadApp::default();
        app.doc = doc_of(2_000, 8);
        // THE PATHOLOGICAL CASE, reached by shrinking the budget rather than by allocating half a
        // gigabyte in a unit test: one snapshot is now far over it on its own, which is exactly
        // the position a 300 MB document is in against the real 512 MB default.
        app.undo_budget_bytes = 1_024;
        assert!(
            app.doc.approx_bytes() > app.undo_budget_bytes,
            "the fixture must exceed the budget in ONE step, or the floor is never reached",
        );
        for _ in 0..40 {
            app.snapshot_doc();
        }
        assert!(
            app.undo_stack.len() >= UNDO_MIN_STEPS,
            "the budget emptied the undo stack — Ctrl+Z would silently do nothing, which a user \
             cannot tell from an edit that never registered",
        );
    }

    /// OLDEST FIRST. Undo is a stack; evicting from the wrong end would throw away the step the
    /// user is about to press Ctrl+Z for.
    #[test]
    fn eviction_takes_the_oldest_step_first() {
        let mut app = CadApp::default();
        app.doc = doc_of(10, 4);
        // A marker in the FIRST snapshot only, then fill past the cap.
        app.doc.push(DObject::new(Geom::Circle(cad_kernel::Circle {
            center: Vec2::new(-999.0, -999.0),
            radius: 1.0,
        })));
        app.snapshot_doc();
        let marked = app.doc.dobjects.len();
        app.doc.dobjects.pop();
        for _ in 0..UNDO_STACK_CAP {
            app.snapshot_doc();
        }
        let oldest_still_marked = match &app.undo_stack[0] {
            UndoStep::Doc(d) => d.dobjects.len() == marked,
            _ => false,
        };
        assert!(
            !oldest_still_marked,
            "the oldest step survived while newer ones were dropped"
        );
    }
}
/// CULLING MUST NEVER DROP SOMETHING THAT IS ON SCREEN.
///
/// The failure mode is the worst kind this file has: geometry vanishes, nothing errors, and it
/// reads as data loss rather than as a rendering bug. So the governing test compares the culled
/// output against the whole buffer for a view that covers everything — they must be equal — and
/// the rest check that a smaller view drops only what is genuinely outside it.

#[cfg(test)]
mod the_cull_never_drops_what_you_can_see {
    use super::*;

    /// A plan of short segments spread over a 400 x 400 m area.
    fn app_with_spread_plan() -> CadApp {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        // The fixture is written in METRE numbers (a 400 m plan under a 40 m view),
        // but a default document is now millimetre space — declare metres or the plan
        // shrinks to 0.4 m and a 40 m view culls nothing, which is the opposite of
        // what this module exists to prove.
        app.doc.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::M,
            cad_kernel::UnitSource::Declared,
        );
        for i in 0..2_000 {
            let (x, y) = ((i % 50) as f64 * 8.0, (i / 50) as f64 * 8.0);
            app.doc.push(DObject::new(Geom::Line(Line {
                a: Vec2::new(x, y),
                b: Vec2::new(x + 3.0, y + 3.0),
            })));
        }
        app.factory.show_plan = true;
        app.refresh_cached_lines();
        app
    }

    fn seg_set(v: &[crate::light3d::V3]) -> std::collections::BTreeSet<(u32, u32, u32, u32)> {
        v.chunks_exact(2)
            .map(|c| {
                (
                    c[0].x.to_bits(),
                    c[0].y.to_bits(),
                    c[1].x.to_bits(),
                    c[1].y.to_bits(),
                )
            })
            .collect()
    }

    /// THE BUCKETING KEEPS EVERY SEGMENT. It reorders the buffer — that is how a cell becomes one
    /// contiguous run — so the comparison is on the SET of segments, not on their order. Order is
    /// immaterial here: every plan line is the same colour at the same height.
    #[test]
    fn bucketing_preserves_every_segment() {
        let app = app_with_spread_plan();
        let plan = CadApp::plan_doc_of(app.factory.session.as_ref(), &app.doc);
        let fresh = app.factory.plan_lines(plan, app.factory.active_base_z());

        assert_eq!(
            app.cached_plan_lines.len(),
            fresh.len(),
            "bucketing changed the vertex count"
        );
        assert_eq!(
            seg_set(&app.cached_plan_lines),
            seg_set(&fresh),
            "bucketing lost or invented a segment",
        );
        assert!(
            !app.cached_plan_cells.is_empty(),
            "the fixture must actually bucket"
        );
    }

    /// A VIEW THAT COVERS EVERYTHING CULLS NOTHING — the property that makes the whole scheme
    /// safe, checked against the full buffer rather than against a count.
    #[test]
    fn a_view_covering_the_whole_plan_draws_the_whole_plan() {
        let app = app_with_spread_plan();
        let mut out = Vec::new();
        let skipped = app.emit_plan_lines(
            &mut out,
            Some((glam::Vec2::new(-1e4, -1e4), glam::Vec2::new(1e4, 1e4))),
        );
        assert_eq!(skipped, 0, "a view containing everything skipped something");
        assert_eq!(
            seg_set(&out),
            seg_set(&app.cached_plan_lines),
            "segments went missing"
        );
    }

    /// UNBOUNDED VIEW ⇒ NO CULL. A camera tilted at the horizon sees a region that cannot be
    /// bounded, and inventing a limit there would cull geometry off the screen edge.
    #[test]
    fn an_unbounded_view_draws_everything() {
        let app = app_with_spread_plan();
        let mut out = Vec::new();
        let skipped = app.emit_plan_lines(&mut out, None);
        assert_eq!(skipped, 0);
        assert_eq!(
            out.len(),
            app.cached_plan_lines.len(),
            "an unbounded view culled"
        );
    }

    /// A SMALL VIEW CULLS — and everything it keeps genuinely touches that view. Both halves
    /// matter: the first is the point, the second is what stops the first being achieved by
    /// dropping the wrong things.
    #[test]
    fn a_small_view_keeps_only_what_reaches_it() {
        let app = app_with_spread_plan();
        let (vmn, vmx) = (glam::Vec2::new(0.0, 0.0), glam::Vec2::new(40.0, 40.0));
        let mut out = Vec::new();
        let skipped = app.emit_plan_lines(&mut out, Some((vmn, vmx)));

        assert!(
            skipped > 0,
            "a 40 m window over a 400 m plan culled nothing"
        );
        assert!(
            !out.is_empty(),
            "…and it culled everything, which is the opposite failure"
        );
        // Every segment kept must be in a CELL that overlaps the view. Individual segments in a
        // kept cell may sit outside it — that is the cost of bucketing, and it is why this asserts
        // on cell overlap rather than on each segment.
        for c in out.chunks_exact(2) {
            let (sx, sy) = (c[0].x.min(c[1].x), c[0].y.min(c[1].y));
            let (bx, by) = (c[0].x.max(c[1].x), c[0].y.max(c[1].y));
            let in_a_kept_cell = app.cached_plan_cells.iter().any(|(cmn, cmx, _, _)| {
                cmx.x >= vmn.x
                    && cmn.x <= vmx.x
                    && cmx.y >= vmn.y
                    && cmn.y <= vmx.y
                    && bx >= cmn.x
                    && sx <= cmx.x
                    && by >= cmn.y
                    && sy <= cmx.y
            });
            assert!(
                in_a_kept_cell,
                "a segment was drawn from a cell that does not meet the view"
            );
        }
    }

    /// EVERY SEGMENT INSIDE THE VIEW IS DRAWN. The direct statement of the promise, checked
    /// segment by segment rather than through the cell index that implements it.
    #[test]
    fn every_segment_inside_the_view_is_drawn() {
        let app = app_with_spread_plan();
        let (vmn, vmx) = (glam::Vec2::new(50.0, 50.0), glam::Vec2::new(150.0, 150.0));
        let mut out = Vec::new();
        app.emit_plan_lines(&mut out, Some((vmn, vmx)));
        let drawn = seg_set(&out);

        for c in app.cached_plan_lines.chunks_exact(2) {
            let (sx, sy) = (c[0].x.min(c[1].x), c[0].y.min(c[1].y));
            let (bx, by) = (c[0].x.max(c[1].x), c[0].y.max(c[1].y));
            let touches = bx >= vmn.x && sx <= vmx.x && by >= vmn.y && sy <= vmx.y;
            if touches {
                let k = (
                    c[0].x.to_bits(),
                    c[0].y.to_bits(),
                    c[1].x.to_bits(),
                    c[1].y.to_bits(),
                );
                assert!(drawn.contains(&k), "a segment inside the view was culled");
            }
        }
    }

    /// The camera maths: a plan view straight down bounds the ground; a view along the horizon
    /// does not, and says so rather than guessing.
    #[test]
    fn ground_bounds_are_found_looking_down_and_refused_at_the_horizon() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let down = crate::light3d::mvp(
            0.0,
            -89.9_f32.to_radians(),
            30.0,
            [0.0, 0.0, 0.0],
            800.0 / 600.0,
            false,
        );
        let b = crate::factory::FactoryState::ground_view_bounds(rect, &down, 0.0);
        let (mn, mx) = b.expect("looking straight down must bound the ground");
        assert!(
            mx.x > mn.x && mx.y > mn.y,
            "degenerate bounds {mn:?}..{mx:?}"
        );

        let level = crate::light3d::mvp(0.0, 0.0, 30.0, [0.0, 0.0, 0.0], 800.0 / 600.0, false);
        assert!(
            crate::factory::FactoryState::ground_view_bounds(rect, &level, 0.0).is_none(),
            "a level camera sees to the horizon — the region is unbounded and must not be guessed",
        );
    }
}
/// WHERE THE ZEROES ARE.
///
/// Reported as: "our min lux was 0 while for relux it was 133… its an obvious error. find the root
/// cause." A minimum of exactly zero is not a low reading, it is a point that received nothing at
/// all — which in a room with five bounces of interreflection is impossible unless the point is
/// enclosed. This prints where they are, so the answer comes from the grid rather than a guess.
///
///     RESULT=<path to .simlux-result.json> cargo test -p cad_app --bin simlux where_the_zeroes_are -- --ignored --nocapture

#[cfg(test)]
mod result_forensics {
    #[test]
    #[ignore = "needs RESULT=<path>"]
    fn where_the_zeroes_are() {
        let Ok(path) = std::env::var("RESULT") else {
            return;
        };
        let text = std::fs::read_to_string(&path).expect("read the result");
        let stored: crate::light_store::StoredResults =
            serde_json::from_str(&text).expect("parse the result");
        let rooms = stored.rooms().expect("rebuild the rooms");
        for r in &rooms {
            let g = &r.grid;
            let (gc, gr) = (g.cols as usize, g.rows as usize);
            let p = &r.plane;
            println!(
                "\n{}: {gc}x{gr} over {:.2} x {:.2} m at z {:.2}",
                r.name, p.width, p.depth, p.origin.z
            );
            println!("  avg {:.1}  min {:.1}  max {:.1}", g.avg, g.min, g.max);

            let inside = |i: usize| r.mask.get(i).copied().unwrap_or(true);
            let mut lows: Vec<(f64, f32, f32)> = Vec::new();
            let mut n_in = 0usize;
            let mut n_zero = 0usize;
            for j in 0..gr {
                for i in 0..gc {
                    let k = j * gc + i;
                    if !inside(k) {
                        continue;
                    }
                    n_in += 1;
                    let v = g.values.get(k).copied().unwrap_or(0.0);
                    let x = p.origin.x + (i as f32 + 0.5) * (p.width / gc as f32);
                    let y = p.origin.y + (j as f32 + 0.5) * (p.depth / gr as f32);
                    if v < 1.0 {
                        n_zero += 1;
                    }
                    lows.push((v, x, y));
                }
            }
            lows.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite"));
            println!("  {n_in} cells in the room, {n_zero} of them under 1 lx");
            println!("  the twelve darkest:");
            for (v, x, y) in lows.iter().take(12) {
                println!("    {v:8.2} lx at ({x:7.2}, {y:7.2})");
            }
            // What the room would report if those cells were not there.
            let kept: Vec<f64> = lows.iter().map(|l| l.0).filter(|v| *v >= 1.0).collect();
            if !kept.is_empty() {
                let mn = kept.iter().cloned().fold(f64::MAX, f64::min);
                let avg = kept.iter().sum::<f64>() / kept.len() as f64;
                println!("  ignoring cells under 1 lx: min {mn:.1}, avg {avg:.1}");
            }
        }
    }
}
#[cfg(test)]
mod max_forensics {
    #[test]
    #[ignore = "needs RESULT=<path>"]
    fn how_much_of_the_peak_is_the_grid() {
        let Ok(path) = std::env::var("RESULT") else {
            return;
        };
        let text = std::fs::read_to_string(&path).expect("read");
        let stored: crate::light_store::StoredResults = serde_json::from_str(&text).expect("parse");
        for r in stored.rooms().expect("rooms") {
            let g = &r.grid;
            let (gc, gr) = (g.cols as usize, g.rows as usize);
            let p = &r.plane;
            let cell = p.width / gc as f32;
            println!(
                "\n{}: {gc}x{gr}, cells {:.3} m, max {:.1} lx",
                r.name, cell, g.max
            );
            // What a COARSER grid would have reported, over every possible phase.
            for step in [2usize, 3, 4] {
                let mut best = f64::MIN;
                let mut worst = f64::MAX;
                for oy in 0..step {
                    for ox in 0..step {
                        let mut m = f64::MIN;
                        for j in (oy..gr).step_by(step) {
                            for i in (ox..gc).step_by(step) {
                                let k = j * gc + i;
                                if r.mask.get(k).copied().unwrap_or(true) {
                                    m = m.max(g.values[k]);
                                }
                            }
                        }
                        best = best.max(m);
                        worst = worst.min(m);
                    }
                }
                println!(
                    "  at {:.2} m spacing ({}x coarser): max lands between {worst:.0} and {best:.0} lx",
                    cell * step as f32, step,
                );
            }
        }
    }

    /// HOW WIDE IS THE PEAK? — the question behind "our max is 1800 and theirs is 1400".
    ///
    /// A maximum is the brightest CELL CENTRE on a grid, so whether a tool reports the peak under a
    /// downlight depends on whether it puts a point there. That is only worth saying if the peak is
    /// genuinely narrow, which is a fact about the field and is measurable: how much of the room
    /// stands above each level, and how far across the bright patch actually is.
    ///
    /// `RESULT=path/to/x.simlux-result.json cargo test -p cad_app --bin simlux --release
    ///  how_wide_is_the_peak -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn how_wide_is_the_peak() {
        let Ok(path) = std::env::var("RESULT") else {
            return;
        };
        let text = std::fs::read_to_string(&path).expect("read");
        let stored: crate::light_store::StoredResults = serde_json::from_str(&text).expect("parse");
        for r in stored.rooms().expect("rooms") {
            let g = &r.grid;
            let (gc, gr) = (g.cols as usize, g.rows as usize);
            let p = &r.plane;
            let (cw, ch) = (p.width / gc as f32, p.depth / gr as f32);
            println!("\n{}: {gc}x{gr}, cells {cw:.3} x {ch:.3} m", r.name);
            println!("  avg {:.1}  min {:.1}  max {:.1} lx", g.avg, g.min, g.max);

            let live = |k: usize| r.mask.get(k).copied().unwrap_or(true);
            let total = (0..gc * gr).filter(|k| live(*k)).count();

            for cut in [1700.0_f64, 1600.0, 1400.0, 1200.0, 1000.0, 800.0] {
                let mut n = 0usize;
                let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) =
                    (usize::MAX, 0usize, usize::MAX, 0usize);
                for j in 0..gr {
                    for i in 0..gc {
                        let k = j * gc + i;
                        if live(k) && g.values[k] >= cut {
                            n += 1;
                            lo_x = lo_x.min(i);
                            hi_x = hi_x.max(i);
                            lo_y = lo_y.min(j);
                            hi_y = hi_y.max(j);
                        }
                    }
                }
                if n == 0 {
                    println!("  above {cut:>6.0} lx: nothing");
                    continue;
                }
                println!(
                    "  above {cut:>6.0} lx: {n:>4} of {total} cells ({:>4.1}% of the room), \
                     bounding box {:.2} x {:.2} m",
                    100.0 * n as f64 / total as f64,
                    (hi_x - lo_x + 1) as f32 * cw,
                    (hi_y - lo_y + 1) as f32 * ch,
                );
            }

            // THE PEAK CELL, and how fast it falls away from it — the shape of the spike.
            let peak = (0..gc * gr).filter(|k| live(*k)).max_by(|a, b| {
                g.values[*a]
                    .partial_cmp(&g.values[*b])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some(k) = peak {
                let (pi, pj) = (k % gc, k / gc);
                println!(
                    "  peak cell ({pi}, {pj}) = {:.0} lx. Walking away in x, one cell ({cw:.3} m) \
                     at a time:",
                    g.values[k],
                );
                let mut line = String::new();
                for d in 0..=7 {
                    if pi + d < gc {
                        line.push_str(&format!("{:>7.0}", g.values[pj * gc + pi + d]));
                    }
                }
                println!("   {line}");
            }
        }
    }
}
#[cfg(test)]
mod point_style_tests {
    use super::*;

    #[test]
    fn point_commit_stamps_current_style_and_size() {
        let mut app = CadApp::default();
        app.current_point_style = 35; // Circle·X
        app.current_point_size = -5.0; // 5% of view
                                       // Emulate the point-tool commit (Tool::Point, 1 pending click).
        app.tool = Tool::Point;
        app.pending = vec![Vec2::new(3.0, 4.0)];
        app.try_finalise(); // (Tool::Point, 1) arm
        let last = app.doc.dobjects.last().expect("a point was committed");
        if let Geom::Point(p) = &last.geom {
            assert_eq!(p.style, 35);
            assert_eq!(p.size, -5.0);
        } else {
            panic!("expected a Point");
        }
    }
}
#[cfg(test)]
mod layout_plot_tests {
    use super::*;
    use cad_kernel::plotstyle::PlotTarget;

    #[test]
    fn plot_config_targets_the_active_layout() {
        let mut app = CadApp::default();
        assert_eq!(app.doc.active_layout, None);
        let cfg = app.plot_config(PlotTarget::PdfFile(std::path::PathBuf::from("/tmp/x.pdf")));
        assert_eq!(cfg.plot_layout_index, None, "model tab → model plot");
        // Enter a layout tab: the same config now carries the active layout.
        let mut l = cad_kernel::Layout::new(
            "A0 PLOT",
            cad_kernel::plotstyle::PaperSize::A0,
            cad_kernel::plotstyle::Orientation::Landscape,
        );
        l.ctb_name = Some("monochrome".into());
        app.doc.layouts.push(l);
        app.switch_to_tab(Some(0));
        assert_eq!(app.doc.active_layout, Some(0));
        let cfg = app.plot_config(PlotTarget::PdfFile(std::path::PathBuf::from("/tmp/y.pdf")));
        assert_eq!(
            cfg.plot_layout_index,
            Some(0),
            "layout tab → the layout is plotted 1:1"
        );
    }

    #[test]
    fn layout_plot_dialog_does_not_require_a_window_pick() {
        let mut app = CadApp::default();
        let mut l = cad_kernel::Layout::new(
            "A4",
            cad_kernel::plotstyle::PaperSize::A4,
            cad_kernel::plotstyle::Orientation::Portrait,
        );
        l.ctb_name = Some("monochrome".into());
        app.doc.layouts.push(l);
        app.switch_to_tab(Some(0));
        // Area kind 1 = window with NO window picked — legal for a layout.
        app.plot_area_kind = 1;
        app.plot_window = None;
        app.plot_pdf_path = "/tmp/layout.pdf".into();
        app.plot_pdf_browse = false;
        // run_plot would try to WRITE the file; instead assert the guard it
        // uses is the index-aware one by exercising plot_config + the guard
        // expression exactly as run_plot does.
        let cfg = app.plot_config(PlotTarget::PdfFile(std::path::PathBuf::from("/tmp/z.pdf")));
        let would_require_window =
            cfg.plot_layout_index.is_none() && app.plot_area_kind == 1 && app.plot_window.is_none();
        assert!(!would_require_window, "layout plots never demand a window");
        app.switch_to_tab(None);
        let cfg = app.plot_config(PlotTarget::PdfFile(std::path::PathBuf::from("/tmp/w.pdf")));
        let would_require_window =
            cfg.plot_layout_index.is_none() && app.plot_area_kind == 1 && app.plot_window.is_none();
        assert!(would_require_window, "model plots do");
    }
}
#[cfg(test)]
mod rooms_unification_migration {
    use super::*;

    /// A 4000 mm closed square on the layer (doc default units are millimetres
    /// unless declared), so the imported footprint comes out in metres.
    fn square_ring_mm() -> cad_kernel::Geom {
        cad_kernel::Geom::Polyline(cad_kernel::Polyline {
            vertices: [
                (0.0, 0.0),
                (4000.0, 0.0),
                (4000.0, 4000.0),
                (0.0, 4000.0),
                (0.0, 0.0),
            ]
            .iter()
            .map(|&(x, y)| cad_kernel::PolyVertex {
                pos: cad_kernel::Vec2::new(x, y),
                bulge: 0.0,
            })
            .collect(),
            closed: true,
            widths: Vec::new(),
        })
    }

    /// A config written BEFORE the ONE-room-list unification carries rooms in
    /// `layers_3d` and `plan_rooms`; loading it folds both into `factory.rooms`
    /// with their provenance, and saving writes the unified format only.
    #[test]
    fn legacy_rooms_migrate_into_the_one_factory_list() {
        let mut app = CadApp::default();
        let lid = app.doc.layers.add(cad_kernel::Layer {
            name: "WALLS".into(),
            ..cad_kernel::Layer::layer_zero()
        });
        app.doc.push(cad_kernel::DObject::new(square_ring_mm()));
        app.doc.dobjects[0].style.layer = lid;

        let mut cfg = app.light.to_config(&app.doc);
        cfg.layers_3d.insert("WALLS".into(), 3.2);
        cfg.plan_rooms.push(crate::simlux_io::PlanRoomRec {
            name: "Office".into(),
            footprint: vec![[0.0, 0.0], [5.0, 0.0], [5.0, 4.0], [0.0, 4.0]],
        });
        app.install_simlux_config(cfg, Vec::new(), None);

        assert_eq!(
            app.factory.rooms.len(),
            2,
            "both legacy sources became rooms"
        );
        let layer_room = app
            .factory
            .rooms
            .iter()
            .find(|r| r.origin == crate::factory::RoomOrigin::ImportedLayer)
            .expect("the layer import migrated");
        assert_eq!(layer_room.name, "WALLS");
        assert_eq!(layer_room.height, 3.2, "the per-layer height came along");
        assert!(
            !layer_room.footprint.is_empty(),
            "the ring became the footprint"
        );

        let plan_room = app
            .factory
            .rooms
            .iter()
            .find(|r| r.origin == crate::factory::RoomOrigin::PlanDesignated)
            .expect("the designation migrated");
        assert_eq!(plan_room.name, "Office");
        assert_eq!(plan_room.footprint.len(), 4);
        assert!(!plan_room.is_built());

        // The unified room list survives a save: the legacy fields go empty.
        let saved = app.light.to_config(&app.doc);
        assert!(saved.layers_3d.is_empty(), "no legacy layer map on save");
        assert!(saved.plan_rooms.is_empty(), "no legacy plan rooms on save");
    }
}
