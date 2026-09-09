use super::super::*;

#[cfg(test)]
mod mouse_rule_tests {

    /// MOUSE RULE (COMMAND_LINE_AND_MOUSE_RULES Part B), enforced as a test rather
    /// than a comment: the 3D viewport must orbit ONLY on middle-drag or Alt+Right,
    /// never on a bare `resp.dragged()` — the left button belongs to picking, and a
    /// left-drag that moved the camera is exactly the "view flies away while I drag a
    /// solid" bug. This greps the real source because the gesture itself needs a live
    /// egui pointer, which a unit test has no way to supply.
    #[test]
    fn factory_viewport_orbits_only_on_middle_or_alt_right() {
        let src = include_str!("../mod.rs");
        let start = src.find("fn render_factory_panel").expect("panel exists");
        let end = src[start..]
            .find("\n    fn ")
            .map(|e| start + e)
            .unwrap_or(src.len());
        let body = &src[start..end];

        assert!(
            body.contains("dragged_by(egui::PointerButton::Middle)"),
            "middle-drag must orbit"
        );
        assert!(
            body.contains("modifiers.alt") && body.contains("PointerButton::Secondary"),
            "Alt+Right must orbit (for mice without a middle button)"
        );
        // the regression this guards: a bare dragged() orbits on ANY button
        for line in body.lines() {
            let l = line.trim();
            if l.starts_with("//") {
                continue;
            }
            assert!(
                !l.contains("resp.dragged()") || !l.contains("cam_yaw"),
                "bare resp.dragged() must not drive the camera: {l}"
            );
        }
        // and the left button must be wired to SELECTION, not the view
        assert!(
            body.contains("pick_feature"),
            "left click selects a feature"
        );
    }

    /// BOTH viewports must show the active-viewport frame, from ONE implementation —
    /// the indicator is useless if only one view has it, or if the two drift apart.
    /// The 3D panel's frame must also be drawn AFTER its paint callback, or the GL
    /// render covers it.
    #[test]
    fn both_viewports_show_the_active_frame() {
        let src = include_str!("../mod.rs");
        assert!(
            src.contains("pub fn draw_viewport_active_frame"),
            "one shared impl"
        );
        // 3D panel
        let start = src.find("fn render_factory_panel").unwrap();
        let end = src[start..]
            .find("\n    fn ")
            .map(|e| start + e)
            .unwrap_or(src.len());
        let panel = &src[start..end];
        let cb = panel
            .find("Shape::Callback")
            .expect("the 3D paint callback");
        let frame = panel
            .find("draw_viewport_active_frame")
            .expect("3D shows the frame");
        assert!(
            frame > cb,
            "the frame must be painted AFTER the GL callback, else it is hidden"
        );
        // 2D canvas
        let c2d = src.find("egui::CentralPanel::default().show(ctx").unwrap();
        let e2d = src[c2d..]
            .find("\n        egui::TopBottomPanel")
            .map(|e| c2d + e)
            .unwrap_or(src.len());
        assert!(
            src[c2d..e2d].contains("draw_viewport_active_frame"),
            "the 2D canvas must show the frame too"
        );
    }

    /// Both canvases must use the SAME cursor glyphs — one implementation, so 2D and
    /// 3D can't drift apart.
    #[test]
    fn both_views_share_the_cursor_glyphs() {
        let src = include_str!("../mod.rs");
        assert!(
            src.contains("pub fn draw_draft_cursor"),
            "shared drafting cursor exists"
        );
        assert!(
            src.contains("pub fn draw_select_cursor"),
            "shared selection cursor exists"
        );
        // 2D canvas uses them
        assert!(
            src.contains("draw_draft_cursor(&painter, p);"),
            "2D uses the shared draft cursor"
        );
        // 3D panel uses them
        let start = src.find("fn render_factory_panel").unwrap();
        let end = src[start..]
            .find("\n    fn ")
            .map(|e| start + e)
            .unwrap_or(src.len());
        let body = &src[start..end];
        assert!(
            body.contains("draw_select_cursor"),
            "3D uses the shared selection cursor"
        );
        assert!(
            body.contains("draw_draft_cursor"),
            "3D uses the shared drafting cursor"
        );
    }
}
/// Who owns a keystroke — the focused field, or the global command cascade.
///
/// Reported from the field: entering a wall height in a menu and pressing Enter made the drawing
/// vanish. Enter was read from the global context with no regard for focus, so the value committed
/// AND the cascade ran to its last branch, which repeats the previous command. With `clear` behind
/// it that empties the document. The same hole let Delete — pressed to remove a digit — erase the
/// current selection.

#[cfg(test)]
mod keystroke_ownership {
    use super::*;

    fn field_id() -> egui::Id {
        egui::Id::new("some_drag_value")
    }

    /// A field other than the command line holds the keyboard ⇒ the key is that field's.
    #[test]
    fn a_focused_field_owns_its_own_keystrokes() {
        let mut app = CadApp::default();
        app.focus_at_frame_start = Some(field_id());
        assert!(app.typing_in_a_field());
    }

    /// The COMMAND LINE is the one field that drives the cascade — that is its entire job, and
    /// guarding it would break the primary way the app is driven.
    #[test]
    fn the_command_line_still_drives_the_cascade() {
        let mut app = CadApp::default();
        app.focus_at_frame_start = Some(CadApp::cmd_line_id());
        assert!(
            !app.typing_in_a_field(),
            "the command line must keep its Enter"
        );
    }

    /// Nothing focused — plain drafting — behaves exactly as before. This is what keeps Enter
    /// repeating the last command and Delete erasing the selection when the user means them to.
    #[test]
    fn with_nothing_focused_the_global_keys_still_work() {
        let app = CadApp::default();
        assert_eq!(app.focus_at_frame_start, None);
        assert!(!app.typing_in_a_field());
    }

    /// The command line's id is STABLE across calls. Derived fresh each time it is asked for, so
    /// if it were not deterministic the guard would reject the command line at random and Enter
    /// would intermittently stop working.
    #[test]
    fn the_command_line_id_is_stable() {
        assert_eq!(CadApp::cmd_line_id(), CadApp::cmd_line_id());
        assert_ne!(CadApp::cmd_line_id(), field_id());
    }

    /// **The reported failure.** A drawing, a last command of `clear`, and a value confirmed in a
    /// field: the document must survive. Before the guard the cascade re-ran `clear` and the 12
    /// objects went to zero, which is what "everything in the window disappears" was.
    #[test]
    fn confirming_a_value_in_a_field_does_not_repeat_the_last_command() {
        let mut app = CadApp::default();
        app.doc.dobjects.clear();
        for i in 0..12 {
            app.doc
                .push(cad_kernel::DObject::new(cad_kernel::Geom::Line(
                    cad_kernel::Line {
                        a: Vec2::new(0.0, i as f64),
                        b: Vec2::new(1000.0, i as f64),
                    },
                )));
        }
        app.last_command = Some("clear".into());
        assert_eq!(app.doc.dobjects.len(), 12);

        // A wall-height field holds the keyboard; the user presses Enter to confirm.
        app.focus_at_frame_start = Some(field_id());
        assert!(app.typing_in_a_field(), "the field owns this Enter");
        // The cascade's repeat branch is gated on exactly this, so it does not run.
        if !app.typing_in_a_field() {
            if let Some(last) = app.last_command.clone() {
                app.run_command(&last);
            }
        }
        assert_eq!(
            app.doc.dobjects.len(),
            12,
            "the drawing must still be there"
        );

        // …and with focus back on the command line, Enter repeats as designed.
        app.focus_at_frame_start = Some(CadApp::cmd_line_id());
        if !app.typing_in_a_field() {
            if let Some(last) = app.last_command.clone() {
                app.run_command(&last);
            }
        }
        assert_eq!(
            app.doc.dobjects.len(),
            0,
            "at the command prompt, Enter still repeats `clear`"
        );
    }
}
#[cfg(test)]
mod the_shortcut_table_is_honest {
    use super::*;

    /// A help page that lists a key the code does not bind is worse than no help page. These are
    /// the ones this session added, checked against the handler that binds them.
    #[test]
    fn the_3d_keys_listed_are_the_3d_keys_bound() {
        let src = include_str!("../mod.rs");
        let a = src
            .find("// ---- 3D VIEWPORT HOTKEYS")
            .expect("the handler");
        let b = src[a..]
            .find("\n                if (self.active_view")
            .map(|e| a + e)
            .unwrap();
        let body = &src[a..b];
        let group = SHORTCUTS
            .iter()
            .find(|g| g.title == "3D Factory")
            .expect("the group");
        for (listed, key) in [
            ("M", "Key::M"),
            ("R", "Key::R"),
            ("Tab", "Key::Tab"),
            ("F", "Key::F"),
            ("G", "Key::G"),
            ("H", "Key::H"),
            ("S", "Key::S"),
            ("A", "Key::A"),
        ] {
            assert!(
                group
                    .rows
                    .iter()
                    .any(|r| r.keys == listed || r.keys.starts_with(listed)),
                "{listed} is bound but not listed",
            );
            assert!(
                body.contains(key),
                "{listed} is listed but {key} is not bound"
            );
        }
    }

    /// No blanks, and no duplicated key within one group — two rows claiming the same key is the
    /// shape of a shortcut that was moved and half-updated.
    #[test]
    fn every_row_is_filled_in_and_unique_within_its_group() {
        for g in SHORTCUTS {
            assert!(
                !g.title.is_empty() && !g.scope.is_empty(),
                "a group needs a title and a scope"
            );
            assert!(!g.rows.is_empty(), "{} lists nothing", g.title);
            let mut seen: Vec<&str> = Vec::new();
            for row in g.rows {
                assert!(
                    !row.keys.is_empty() && !row.what.is_empty(),
                    "{}: a blank row",
                    g.title
                );
                assert!(
                    !seen.contains(&row.keys),
                    "{}: '{}' listed twice",
                    g.title,
                    row.keys
                );
                seen.push(row.keys);
            }
        }
    }

    /// The 3D group must say it needs the viewport to be active. A key that silently does nothing
    /// because you have not clicked into the view yet reads as a broken key.
    #[test]
    fn the_3d_group_states_its_gate() {
        let g = SHORTCUTS.iter().find(|g| g.title == "3D Factory").unwrap();
        assert!(
            g.scope.contains("ACTIVE"),
            "the gate has to be stated: {}",
            g.scope
        );
    }
}
/// A WRAPPED MENU BAR MUST NOT SWITCH MENUS ON HOVER.
///
/// Reported as: "while selecting something from the drop down menu, when i move the cursor it
/// opens the menu of whatever is below it." egui opens a menu on
/// `button.clicked() || (button.hovered() && another menu is open)` — right for a single-row menu
/// bar, wrong for one that WRAPS, because the next row sits directly beneath the one you opened
/// and `menu_spacing` leaves a strip of it exposed above the panel.
///
/// The first attempt made the bar a single scrolling row. That removed the second row, and with it
/// the symptom, by changing the layout — which is not what was asked for, and the user said so:
/// "i asked to fix the hover selecting problem not change the layout."

#[cfg(test)]
mod the_toolbars_wrap_and_do_not_hover_switch {
    use super::*;

    fn source_between(anchor: &'static str, src: &'static str) -> String {
        let a = src
            .find(anchor)
            .unwrap_or_else(|| panic!("anchor not found: {anchor}"));
        let w = src[a..]
            .find("horizontal_wrapped")
            .map(|e| a + e)
            .expect("the bar wraps");
        let bytes = src.as_bytes();
        let (mut depth, mut seen, mut end) = (0i32, false, src.len());
        for (i, &c) in bytes.iter().enumerate().skip(w) {
            if c == b'{' {
                depth += 1;
                seen = true;
            } else if c == b'}' {
                depth -= 1;
            }
            if seen && depth == 0 {
                end = i;
                break;
            }
        }
        src[w..end].to_string()
    }

    /// THE LAYOUT STAYS WRAPPED. Both bars stack when the panel is narrow, as they always did.
    #[test]
    fn both_bars_still_wrap() {
        let app = include_str!("../mod.rs");
        let light = include_str!("../../light.rs");
        assert!(
            source_between(
                "plus Frame and Clear. Both dropdowns drive the SAME code",
                app
            )
            .starts_with("horizontal_wrapped"),
            "the 3D Factory toolbar must wrap, not scroll",
        );
        assert!(
            source_between("pub fn toolbar_ui", light).starts_with("horizontal_wrapped"),
            "the SIMLUX toolbar must wrap, not scroll",
        );
        // Checked on the EXTRACTED bodies, never on the whole file: a test that greps a file for
        // the thing it forbids finds its own assertion and fails on itself. The first version of
        // this did exactly that.
        for (name, body) in [
            (
                "3D Factory",
                source_between(
                    "plus Frame and Clear. Both dropdowns drive the SAME code",
                    app,
                ),
            ),
            ("SIMLUX", source_between("pub fn toolbar_ui", light)),
        ] {
            assert!(
                !body.contains("ScrollArea::horizontal"),
                "{name} toolbar scrolls instead of wrapping",
            );
        }
    }

    /// …and every menu on them opens on CLICK ONLY. A single `ui.menu_button` left on either bar
    /// is one button that still swaps under the cursor.
    #[test]
    fn no_hover_switching_menu_survives_on_either_bar() {
        for (name, body) in [
            (
                "3D Factory",
                source_between(
                    "plus Frame and Clear. Both dropdowns drive the SAME code",
                    include_str!("../mod.rs"),
                ),
            ),
            (
                "SIMLUX",
                source_between("pub fn toolbar_ui", include_str!("../../light.rs")),
            ),
        ] {
            assert!(
                !body.contains("ui.menu_button("),
                "{name} toolbar still has a hover-switching menu_button",
            );
            assert!(
                body.contains("click_menu_button("),
                "{name} toolbar uses the click-only menu"
            );
        }
    }

    /// The fix is clearing the BUTTON's hover, and it must be the response handed to egui — not
    /// the one handed back to the caller, which still needs its hover for tooltips.
    #[test]
    fn the_gate_is_on_the_response_egui_reads() {
        let src = include_str!("../mod.rs");
        let a = src
            .find("pub(crate) fn click_menu_button")
            .expect("the helper");
        let b = src[a..].find("\n}").map(|e| a + e).unwrap();
        let f = &src[a..b];
        assert!(
            f.contains("gated.hovered = false"),
            "the gated copy is what egui must see"
        );
        assert!(
            f.contains("bar_menu(&gated"),
            "…and it is what is passed to bar_menu"
        );
        assert!(
            f.contains("InnerResponse::new(inner, resp)"),
            "the caller gets the real response"
        );
    }
}
/// WHERE DOES THE 3D FACTORY VIEWPORT ACTUALLY GO?
///
/// Reported twice: "the simlux window ... only extends to a length and when extend it beyond that
/// it goes behind the 3d factory window." The screenshots are cropped, so the geometry cannot be
/// read off them. This measures it instead: run real headless egui frames with the SIMLUX panel
/// widened step by step, and record what rect each panel ends up with.

#[cfg(test)]
mod panel_geometry_measurement {
    use super::*;

    const W: f32 = 2763.0;
    const H: f32 = 1298.0;

    /// Drive one frame with the SIMLUX panel forced to `simlux_w`, and report
    /// `(simlux_rect, factory_rect, central_rect)`.
    fn frame(
        app: &mut CadApp,
        ctx: &egui::Context,
        simlux_w: f32,
    ) -> (egui::Rect, egui::Rect, egui::Rect) {
        // Force the stored panel width the way a drag would.
        ctx.data_mut(|d| {
            d.insert_persisted(
                egui::Id::new("simlux_3d_panel"),
                egui::containers::panel::PanelState {
                    rect: egui::Rect::from_min_size(
                        egui::pos2(W - simlux_w, 0.0),
                        egui::vec2(simlux_w, H),
                    ),
                },
            );
        });
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(W, H),
            )),
            ..Default::default()
        };
        let mut central = egui::Rect::NOTHING;
        let _ = ctx.run(input, |ctx| {
            app.render_light_3d_panel(ctx);
            app.render_factory_panel(ctx);
            egui::CentralPanel::default().show(ctx, |ui| {
                central = ui.max_rect();
            });
        });
        let get = |id: &str| {
            egui::containers::panel::PanelState::load(ctx, egui::Id::new(id))
                .map(|s| s.rect)
                .unwrap_or(egui::Rect::NOTHING)
        };
        (get("simlux_3d_panel"), get("factory_3d_panel"), central)
    }

    /// THE MEASUREMENT. Prints the geometry at a range of SIMLUX widths so the failure mode is
    /// visible as numbers rather than inferred from a cropped screenshot.
    #[test]
    fn the_panels_never_overlap_at_any_simlux_width() {
        let mut app = CadApp::default();
        app.two_d_open = true;
        app.factory.open = true;
        app.light.view3d_open = true;
        app.factory.add_box();
        app.factory.recompute();
        let ctx = egui::Context::default();

        // A warm-up frame: egui needs one pass to settle panel state and widget sizes.
        let _ = frame(&mut app, &ctx, 600.0);

        let mut bad = Vec::new();
        for w in [
            400.0_f32, 800.0, 1200.0, 1600.0, 2000.0, 2200.0, 2400.0, 2500.0, 2600.0, 2700.0,
        ] {
            let (s, f, c) = frame(&mut app, &ctx, w);
            println!(
                "asked {w:>6.0}  simlux {:>7.1}..{:<7.1} ({:>6.1})   factory {:>7.1}..{:<7.1} ({:>6.1})   central {:>7.1}..{:<7.1} ({:>6.1})",
                s.left(), s.right(), s.width(),
                f.left(), f.right(), f.width(),
                c.left(), c.right(), c.width(),
            );
            // The factory panel must sit entirely LEFT of the SIMLUX panel.
            if f.right() > s.left() + 0.5 {
                bad.push(format!(
                    "at simlux width {w:.0}: factory panel reaches {:.1} but SIMLUX starts at {:.1} \
                     — they overlap by {:.1} px",
                    f.right(), s.left(), f.right() - s.left(),
                ));
            }
            // …and the 2D canvas must keep something usable.
            if c.width() < 1.0 {
                bad.push(format!(
                    "at simlux width {w:.0}: the 2D canvas is {:.1} px wide",
                    c.width()
                ));
            }
        }
        assert!(
            bad.is_empty(),
            "panel geometry breaks down:\n  {}",
            bad.join("\n  ")
        );
    }

    /// EACH VIEW ALONE CAN FILL THE WINDOW. "make sure the windows of 2d cad 3d factory and simlux
    /// can be extended to which ever length the user prefers and they can close any of them to
    /// have any of the single window open."
    #[test]
    fn one_view_alone_gets_the_whole_window() {
        // SIMLUX alone.
        let mut app = CadApp::default();
        app.two_d_open = false;
        app.factory.open = false;
        app.light.view3d_open = true;
        let ctx = egui::Context::default();
        let _ = frame(&mut app, &ctx, 600.0);
        let (s, _f, _c) = frame(&mut app, &ctx, W);
        println!(
            "simlux alone: {:.1}..{:.1} ({:.1}) of {W}",
            s.left(),
            s.right(),
            s.width()
        );
        assert!(
            s.width() > W - MIN_VIEW_W - 1.0,
            "with the others closed SIMLUX must take the window, got {:.1} of {W}",
            s.width(),
        );

        // The 3D Factory alone. A panel keeps whatever width it was dragged to — it does not grow
        // on its own — so what is under test is that its LIMIT allows the whole window. Drag it
        // there, the way the user would.
        let mut app = CadApp::default();
        app.two_d_open = false;
        app.factory.open = true;
        app.light.view3d_open = false;
        let ctx = egui::Context::default();
        let _ = frame(&mut app, &ctx, 600.0);
        ctx.data_mut(|d| {
            d.insert_persisted(
                egui::Id::new("factory_3d_panel"),
                egui::containers::panel::PanelState {
                    rect: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(W, H)),
                },
            );
        });
        let (_s, f, _c) = frame(&mut app, &ctx, 600.0);
        println!(
            "factory alone: {:.1}..{:.1} ({:.1}) of {W}",
            f.left(),
            f.right(),
            f.width()
        );
        assert!(
            f.width() > W - MIN_VIEW_W - 1.0,
            "with the others closed the 3D Factory must be allowed the window, got {:.1} of {W}",
            f.width(),
        );

        // The 2D canvas alone.
        let mut app = CadApp::default();
        app.two_d_open = true;
        app.factory.open = false;
        app.light.view3d_open = false;
        let ctx = egui::Context::default();
        let _ = frame(&mut app, &ctx, 600.0);
        let (_s, _f, c) = frame(&mut app, &ctx, 600.0);
        println!(
            "2D alone: {:.1}..{:.1} ({:.1}) of {W}",
            c.left(),
            c.right(),
            c.width()
        );
        assert!(
            c.width() > W - 40.0,
            "the 2D canvas alone must take the window, got {:.1}",
            c.width()
        );
    }

    /// Closing the last view must not leave an empty window.
    #[test]
    fn the_last_view_cannot_be_closed() {
        let mut app = CadApp::default();
        app.two_d_open = false;
        app.factory.open = false;
        app.light.view3d_open = false;
        app.light.simlux_mode = false;
        app.keep_one_view_open();
        assert!(app.two_d_open, "closing the last view must bring one back");
    }
}
/// THE MODE TAB BAR — EXACTLY ONE FULL-WINDOW WORKSPACE AT A TIME.
///
/// The tab bar (2D view | SIMLUX view | 3D Factory view) replaces the old
/// per-view checkboxes that let all three views share the window. `mode` is
/// the single source of truth; `switch_mode` rearranges the view open-flags,
/// and `enforce_mode_workspaces` (frame start) repairs whatever broke the
/// invariant mid-frame: a view opened from inside another workspace, a
/// workspace view closed with its ✕, and a face-sketch that needs the 2D
/// canvas the Factory workspace hides.

#[cfg(test)]
mod mode_workspaces_are_exclusive {
    use super::*;

    #[test]
    fn the_app_starts_in_the_2d_drafting_workspace() {
        let app = CadApp::default();
        assert_eq!(app.mode, Mode::Cad2D);
        assert!(app.two_d_open && app.mode_view_open());
        assert!(!app.factory.open && !app.light.view3d_open && !app.light.simlux_mode);
    }

    /// Switching tabs must close the other two views outright — that is what
    /// makes each workspace "full window".
    #[test]
    fn switching_tabs_rearranges_the_view_flags() {
        let mut app = CadApp::default();
        app.switch_mode(Mode::Factory);
        assert_eq!(app.mode, Mode::Factory);
        assert!(app.factory.open && app.mode_view_open());
        assert!(!app.two_d_open && !app.light.view3d_open && !app.light.simlux_mode);

        app.switch_mode(Mode::Simlux);
        assert_eq!(app.mode, Mode::Simlux);
        assert!(app.light.view3d_open && app.mode_view_open());
        assert!(!app.factory.open && !app.two_d_open && !app.light.simlux_mode);

        app.switch_mode(Mode::Cad2D);
        assert_eq!(app.mode, Mode::Cad2D);
        assert!(app.two_d_open && app.mode_view_open());
        assert!(!app.factory.open && !app.light.view3d_open);
    }

    /// The SIMLUX tab is the 3D lighting viewport ONLY — the old 2D|3D split
    /// workspace (`simlux_mode`) is gone with the checkbox that turned it on.
    #[test]
    fn entering_simlux_never_uses_the_split_workspace() {
        let mut app = CadApp::default();
        app.light.simlux_mode = true; // stale state from a pre-tab session
        app.switch_mode(Mode::Simlux);
        assert!(!app.light.simlux_mode, "the tab owns the workspace now");
        assert!(app.light.view3d_open);
    }

    /// An import or the Materials Factory opens `factory.open` from the middle
    /// of another workspace; the next frame must land on the Factory tab — the
    /// window shows what the code just asked for.
    #[test]
    fn a_view_opened_from_inside_another_workspace_brings_its_tab_forward() {
        let mut app = CadApp::default();
        app.factory.open = true; // Materials Factory opened its live preview
        app.enforce_mode_workspaces();
        assert_eq!(app.mode, Mode::Factory);
        assert!(app.factory.open && !app.two_d_open);

        let mut app = CadApp::default();
        app.light.view3d_open = true; // SIMLUX machinery opened its 3D view
        app.enforce_mode_workspaces();
        assert_eq!(app.mode, Mode::Simlux);
        assert!(app.light.view3d_open && !app.two_d_open);
    }

    /// A workspace whose own view was closed (the panel's ✕) must fall back to
    /// 2D drafting — never to an empty window.
    #[test]
    fn closing_the_workspace_view_falls_back_to_2d() {
        let mut app = CadApp::default();
        app.switch_mode(Mode::Factory);
        app.factory.open = false; // the ✕ in the factory panel header
        app.enforce_mode_workspaces();
        assert_eq!(app.mode, Mode::Cad2D);
        assert!(app.two_d_open, "the 2D canvas is open again");

        let mut app = CadApp::default();
        app.switch_mode(Mode::Simlux);
        app.light.view3d_open = false; // the ✕ in the SIMLUX panel header
        app.enforce_mode_workspaces();
        assert_eq!(app.mode, Mode::Cad2D);
        assert!(app.two_d_open);
    }

    /// A face-sketch drafts ON the 2D canvas. Started from the Factory
    /// workspace (which hides the canvas), it takes over the whole window for
    /// drafting; finishing it returns to the Factory.
    #[test]
    fn a_factory_sketch_takes_over_the_window_and_returns() {
        let mut app = CadApp::default();
        app.switch_mode(Mode::Factory);
        assert!(!app.two_d_open, "the Factory workspace hides the canvas");
        let before = app.doc.dobjects.len();

        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert!(app.factory.session.is_some());
        app.enforce_mode_workspaces();
        assert_eq!(
            app.mode,
            Mode::Cad2D,
            "the sketch claims the drafting window"
        );
        assert!(app.two_d_open && !app.factory.open);
        assert!(app.factory_return_after_sketch);

        app.factory_exit_sketch();
        assert_eq!(app.doc.dobjects.len(), before, "the plan document is back");
        app.enforce_mode_workspaces();
        assert_eq!(app.mode, Mode::Factory, "finishing returns to the Factory");
        assert!(app.factory.open);
        assert!(!app.factory_return_after_sketch);
    }

    /// A user who switched tabs mid-sketch does not get yanked back when the
    /// sketch ends: the tab click cancelled the automatic return.
    #[test]
    fn a_tab_click_mid_sketch_cancels_the_automatic_return() {
        let mut app = CadApp::default();
        app.switch_mode(Mode::Factory);
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        app.enforce_mode_workspaces();
        assert_eq!(app.mode, Mode::Cad2D);
        assert!(app.factory_return_after_sketch);

        app.switch_mode(Mode::Simlux); // explicit user intent
        assert!(
            !app.factory_return_after_sketch,
            "the tab click cancelled the return"
        );

        app.factory_exit_sketch();
        app.enforce_mode_workspaces();
        assert_eq!(
            app.mode,
            Mode::Simlux,
            "the user stays where they switched to"
        );
    }

    /// Headless smoke test: every workspace must render its own panels — the
    /// mode tab bar, the mode command panel, and the workspace's view — without
    /// panicking, and the 3D workspaces must come up maximised (the full-window
    /// pin), not at whatever width a previous session stored.
    #[test]
    fn each_workspace_renders_a_full_frame_of_its_own_panels() {
        for mode in [Mode::Cad2D, Mode::Simlux, Mode::Factory] {
            let mut app = CadApp::default();
            app.switch_mode(mode);
            if mode == Mode::Factory {
                app.factory.add_box();
                app.factory.recompute();
            }
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1920.0, 1080.0),
                )),
                ..Default::default()
            };
            let mut tab_shown = false;
            let _ = ctx.run(input, |ctx| {
                // The same call order update() uses.
                app.render_mode_tabs(ctx);
                tab_shown = true;
                app.render_mode_command_panel(ctx);
                if mode == Mode::Simlux {
                    app.render_light_3d_panel(ctx);
                }
                if mode == Mode::Factory {
                    app.render_factory_panel(ctx);
                }
                egui::CentralPanel::default().show(ctx, |_| {});
            });
            assert!(tab_shown, "the tab bar rendered in {mode:?}");
            // The 3D workspace view must occupy the window (only the mode
            // command panel on the left may be reserved).
            if mode == Mode::Simlux || mode == Mode::Factory {
                let (id, label) = if mode == Mode::Simlux {
                    (egui::Id::new("simlux_3d_panel"), "SIMLUX")
                } else {
                    (egui::Id::new("factory_3d_panel"), "3D Factory")
                };
                let w = egui::containers::panel::PanelState::load(&ctx, id)
                    .map(|s| s.rect.width())
                    .unwrap_or(0.0);
                assert!(
                    w > 1400.0,
                    "{label} viewport must come up maximised in its workspace, got {w:.0} px",
                );
            }
        }
    }

    /// The drafting workspace with a face-sketch open renders its sketch
    /// section (Finish / reshape buttons and actions) without panicking.
    #[test]
    fn a_sketch_renders_its_actions_in_the_2d_command_panel() {
        let mut app = CadApp::default();
        app.factory_enter_sketch(crate::factory::FactoryState::ground_frame());
        assert!(app.factory.session.is_some());
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            app.render_mode_tabs(ctx);
            app.render_mode_command_panel(ctx);
            egui::CentralPanel::default().show(ctx, |_| {});
        });
        assert_eq!(app.mode, Mode::Cad2D, "drafting owns the window");
    }

    /// REGRESSION — the bottom COMMAND BAR belongs to the 2D drafting
    /// workspace: it must keep its height, the 3D viewports must fill the
    /// whole workspace (no command strip reserved there), and the bar must
    /// come back exactly as it was left, across tab round-trips.
    ///
    /// The bug it pins: the full-window SIMLUX/3D Factory viewports reserved
    /// every pixel before the command bar was laid out, squeezing the bar into
    /// a sliver at the bottom of the centre column; egui ratcheted that
    /// panel's stored HEIGHT upward every frame it stayed squeezed, and
    /// returning to the 2D view reopened the command bar as a wall covering
    /// the canvas (fixed by dragging its top edge back down). The bar is now
    /// laid out BEFORE the viewports, and only in the drafting workspace: the
    /// SIMLUX and 3D Factory workspaces never lay it out at all, so their
    /// viewports run to the window's right edge and down to the status bar
    /// while the bar's stored height freezes for the visit — it cannot ratchet
    /// because nothing ever squeezes it.
    #[test]
    fn command_bar_height_survives_workspace_round_trips() {
        const W: f32 = 1920.0;
        const H: f32 = 1080.0;
        let mut central = egui::Rect::NOTHING;
        let mut bar_added = false;
        let mut frame = |app: &mut CadApp, ctx: &egui::Context| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(W, H),
                )),
                ..Default::default()
            };
            central = egui::Rect::NOTHING;
            bar_added = false;
            let _ = ctx.run(input, |ctx| {
                // update() order: tabs → mode panel → command bar (2D only) → viewports.
                app.render_mode_tabs(ctx);
                app.render_mode_command_panel(ctx);
                if app.cmd_window_open && app.mode == Mode::Cad2D {
                    bar_added = true;
                    use crate::theme::color as tc;
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
                    let min_h =
                        CMD_PAD_BELOW + CMD_PILL_H + CMD_PAD_TOP + CMD_HIST_LINES as f32 * line_h;
                    let bar_h = if app.env.CmdBarH > 0.0 {
                        app.env.CmdBarH.max(min_h)
                    } else {
                        default_h
                    };
                    egui::TopBottomPanel::bottom("command")
                        .resizable(true)
                        .default_height(bar_h)
                        .min_height(min_h)
                        .show_separator_line(false)
                        .frame(egui::Frame::none().fill(tc::SURFACE_1))
                        .show(ctx, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width());
                                app.command_bar_body(ui, None);
                            });
                        });
                }
                if app.mode == Mode::Simlux {
                    app.render_light_3d_panel(ctx);
                }
                if app.mode == Mode::Factory {
                    app.render_factory_panel(ctx);
                }
                egui::CentralPanel::default().show(ctx, |ui| {
                    central = ui.max_rect();
                });
            });
            let get = |id: &str| {
                egui::containers::panel::PanelState::load(ctx, egui::Id::new(id))
                    .map(|s| s.rect)
                    .unwrap_or(egui::Rect::NOTHING)
            };
            (
                get("mode_cmd_panel").width(),
                get("simlux_3d_panel").width(),
                get("factory_3d_panel").width(),
                get("command").height(),
                get("command").width(),
                central.width(),
                get("simlux_3d_panel").height(),
                get("factory_3d_panel").height(),
                bar_added,
            )
        };
        let mut app = CadApp::default();
        let ctx = egui::Context::default();
        let (_, _, _, fresh_h, _, _, _, _, added) = frame(&mut app, &ctx);
        assert!(added, "the 2D workspace shows the command bar");
        assert!(
            fresh_h > 60.0 && fresh_h < 200.0,
            "sane bar height: {fresh_h:.0}"
        );

        app.switch_mode(Mode::Factory);
        for _ in 0..3 {
            let (m, _, f, h, _, _, _, fh, added) = frame(&mut app, &ctx);
            assert!(!added, "the Factory workspace draws no command bar");
            assert!(
                (h - fresh_h).abs() < 1.0,
                "bar height frozen while hidden: {h:.0}"
            );
            assert!(
                f >= W - m - 1.0,
                "the Factory viewport fills the workspace width: {f:.0}"
            );
            assert!(fh > 990.0, "…and runs down to the status bar: {fh:.0}");
        }
        app.switch_mode(Mode::Cad2D);
        for _ in 0..2 {
            frame(&mut app, &ctx);
        }
        let (m, _, f, h, w, c, _, _, added) = frame(&mut app, &ctx);
        assert!(added, "back in 2D the command bar is drawn again");
        assert!(
            (h - fresh_h).abs() < 1.0,
            "bar height back in 2D: {h:.0} vs {fresh_h:.0}"
        );
        assert!(w > 1400.0, "bar spans the workspace width: {w:.0}");
        assert!(
            (m - 242.0).abs() < 0.5,
            "mode panel keeps its width: {m:.0}"
        );
        assert!(c > 1200.0, "canvas has room again: {c:.0}");
        let _ = (f, w);

        app.switch_mode(Mode::Simlux);
        for _ in 0..3 {
            let (m, s, _, h, _, _, sh, _, added) = frame(&mut app, &ctx);
            assert!(!added, "the SIMLUX workspace draws no command bar");
            assert!(
                (h - fresh_h).abs() < 1.0,
                "bar height frozen while hidden: {h:.0}"
            );
            assert!(
                s >= W - m - 1.0 && s <= W - m + 1.0,
                "SIMLUX viewport fills the whole workspace: {s:.0} (mode panel {m:.0})"
            );
            assert!(sh > 990.0, "…down to the status bar: {sh:.0}");
        }
        app.switch_mode(Mode::Cad2D);
        for _ in 0..2 {
            frame(&mut app, &ctx);
        }
        let (_, _, _, h, _, c, _, _, added) = frame(&mut app, &ctx);
        assert!(added, "back in 2D the command bar is drawn again");
        assert!(
            (h - fresh_h).abs() < 1.0,
            "bar height still intact after SIMLUX: {h:.0}"
        );
        assert!(c > 1200.0, "canvas still has room: {c:.0}");
    }

    /// The 2D workspace keeps no SIMLUX panel open, so arming "Place
    /// luminaire" from the mode command panel used to do nothing: the SIMLUX
    /// 2D layer (whose pointer places the points) only went live while a
    /// SIMLUX panel or 3D view was open. An ARMED tool is SIMLUX being on
    /// screen — the plan must answer the very next click.
    #[test]
    fn place_luminaire_arms_the_plan_in_the_2d_workspace() {
        let mut app = CadApp::default();
        assert_eq!(app.mode, Mode::Cad2D);
        assert!(
            !app.light.simlux_mode && !app.light.view3d_open && !app.light.window_open,
            "plain 2D workspace, no SIMLUX on screen",
        );
        assert!(!app.simlux_2d_layer_live(), "SIMLUX layer is off");

        // Exactly what the mode command panel's "Place luminaire" row does.
        app.light.place_mode = true;
        app.light.aim_mode = false;
        app.light.aim_pick = None;
        assert!(
            app.simlux_2d_layer_live(),
            "arming placement makes the plan live"
        );

        let before = app.light.luminaires.len();
        let id = app.place_fixture_point(1.25, 2.5); // a click on the plan
        assert_eq!(
            app.light.luminaires.len(),
            before + 1,
            "the click placed a point"
        );
        assert!(app.light.luminaires.iter().any(|l| l.id == id));

        // Esc-style stop puts the layer back off.
        app.light.place_mode = false;
        assert!(!app.simlux_2d_layer_live());
    }

    /// Designating rooms on the plan creates unbuilt rooms in the ONE factory
    /// list; building one promotes it in place (same name, own solids).
    #[test]
    fn plan_rooms_live_in_the_one_list_and_can_be_built() {
        let mut app = CadApp::default();
        let rect = |x: f32, y: f32| {
            vec![
                glam::Vec2::new(x, y),
                glam::Vec2::new(x + 5.0, y),
                glam::Vec2::new(x + 5.0, y + 4.0),
                glam::Vec2::new(x, y + 4.0),
            ]
        };
        app.factory.add_designated_room("", &rect(0.0, 0.0));
        app.factory.add_designated_room("", &rect(20.0, 0.0));
        assert_eq!(app.factory.rooms.len(), 2);
        assert_eq!(app.factory.rooms[0].name, "Room 1");
        assert_eq!(app.factory.rooms[1].name, "Room 2");
        assert!(!app.factory.rooms[0].is_built());

        // Build the first — a real 3D room with the same footprint + name.
        let built_id = app
            .factory
            .build_designated_room(app.factory.rooms[0].id)
            .unwrap();
        assert_eq!(
            app.factory.rooms.len(),
            2,
            "built replaces the unbuilt record"
        );
        let built = app.factory.rooms.iter().find(|r| r.id == built_id).unwrap();
        assert_eq!(built.name, "Room 1", "the name survived the build");
        assert!(built.is_built());
        assert!(
            built.floor.is_some() && !built.walls.is_empty(),
            "it owns solids now"
        );

        // Deleting any room takes its record (and its solids) with it.
        app.factory.delete_room(built_id);
        assert_eq!(app.factory.rooms.len(), 1);
    }

    /// Making a room from a selected plan outline builds it AT ONCE — the
    /// walls/floor/ceiling exist in the 3D Factory view the moment the row is
    /// clicked (a plan room listed under ▼ Rooms but invisible is the bug this
    /// pins), and the room is a lux calc target from the start.
    ///
    /// The outline comes through `slab_outline_from_selection` — the SAME path
    /// the "Make room" row uses — not a hand-built vector. That path reads
    /// `self.selection` against `doc.dobjects`, and a default app does NOT ship
    /// an empty document (it holds seed geometry), so the fixture clears it and
    /// selects the pushed index, exactly like `promote_tests::app_with`.
    #[test]
    fn making_a_room_from_a_plan_outline_builds_it_in_3d() {
        let mut app = CadApp::default();
        app.doc.units = cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::User);
        app.doc.dobjects.clear();
        let ring = cad_kernel::Geom::Polyline(cad_kernel::Polyline {
            vertices: [(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)]
                .iter()
                .map(|&(x, y)| cad_kernel::PolyVertex {
                    pos: cad_kernel::Vec2::new(x, y),
                    bulge: 0.0,
                })
                .collect(),
            closed: true,
            widths: Vec::new(),
        });
        app.doc.push(cad_kernel::DObject::new(ring));
        app.selection.push(0);

        let outline = app
            .slab_outline_from_selection()
            .expect("a selected closed polyline must yield an outline");
        app.make_room_from_selected_outline(outline);
        assert_eq!(app.factory.rooms.len(), 1);
        let r = &app.factory.rooms[0];
        assert!(
            r.is_built(),
            "the room is built, not an outline-only record"
        );
        assert!(r.floor.is_some(), "it owns a floor");
        assert!(!r.walls.is_empty(), "it owns walls");
        assert_eq!(r.name, "Room 1");
        assert!(
            r.footprint.len() >= 4,
            "the plan outline became its footprint"
        );
        assert_eq!(
            app.undo_stack.len(),
            1,
            "making the room is ONE undoable step"
        );
    }

    /// "Make room" no longer builds instantly: a valid closed outline opens
    /// the details form (nothing built, settings as the prefilled defaults);
    /// an open path still explains what to do instead of opening anything.
    #[test]
    fn make_room_asks_for_details_first() {
        let mut app = CadApp::default();
        app.doc.units = cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::User);
        app.doc.dobjects.clear();
        let ring = cad_kernel::Geom::Polyline(cad_kernel::Polyline {
            vertices: [(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)]
                .iter()
                .map(|&(x, y)| cad_kernel::PolyVertex {
                    pos: cad_kernel::Vec2::new(x, y),
                    bulge: 0.0,
                })
                .collect(),
            closed: true,
            widths: Vec::new(),
        });
        app.doc.push(cad_kernel::DObject::new(ring));
        app.selection.push(0);

        let f = &app.factory;
        let (expect_base, expect_h) = (f.active_base_z(), f.room_height.max(0.05));
        app.request_make_room();
        assert!(
            app.room_form.is_some(),
            "a valid closed outline must open the details form"
        );
        assert!(
            app.factory.rooms.is_empty(),
            "opening the form must not build anything yet"
        );
        let f = app.room_form.as_ref().unwrap();
        assert_eq!(
            f.name, "Room 1",
            "the default name comes from the next room number"
        );
        assert_eq!(
            f.base_z, expect_base,
            "start height defaults to the active storey"
        );
        assert_eq!(
            f.height, expect_h,
            "clear height defaults to the room-height setting"
        );
        assert!(
            f.outline.len() >= 4,
            "the selected outline travels into the form"
        );

        // An OPEN path is walls-only → say what to do; no form.
        let mut app2 = CadApp::default();
        app2.doc.units = cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::User);
        app2.doc.dobjects.clear();
        let open = cad_kernel::Geom::Line(cad_kernel::Line {
            a: cad_kernel::Vec2::new(0.0, 0.0),
            b: cad_kernel::Vec2::new(5.0, 0.0),
        });
        app2.doc.push(cad_kernel::DObject::new(open));
        app2.selection.push(0);
        app2.request_make_room();
        assert!(
            app2.room_form.is_none(),
            "an open path must not open the form"
        );
        assert!(
            app2.history
                .last()
                .unwrap_or(&String::new())
                .contains("CLOSED outline"),
            "and it must say what is wrong: {:?}",
            app2.history.last()
        );
    }

    /// The form's ANSWERS are the room: confirming builds it at the asked
    /// start height with the asked clear height and thicknesses — and the
    /// built geometry really stands there (floor slab bottom at base_z, walls
    /// from base_z + floor, ceiling above the clear height), not wherever the
    /// settings/active storey would have put it.
    #[test]
    fn the_form_answers_become_the_room() {
        let mut app = CadApp::default();
        app.doc.units = cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::User);
        app.doc.dobjects.clear();
        let ring = cad_kernel::Geom::Polyline(cad_kernel::Polyline {
            vertices: [(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)]
                .iter()
                .map(|&(x, y)| cad_kernel::PolyVertex {
                    pos: cad_kernel::Vec2::new(x, y),
                    bulge: 0.0,
                })
                .collect(),
            closed: true,
            widths: Vec::new(),
        });
        app.doc.push(cad_kernel::DObject::new(ring));
        app.selection.push(0);
        app.request_make_room();

        let mut f = app.room_form.take().unwrap();
        f.name = "Lobby".into();
        f.base_z = 1.2; // start height — above the ground
        f.height = 3.1; // clear height
        f.floor_t = 0.2;
        f.wall_t = 0.3;
        f.ceiling_t = 0.25;
        app.build_room_form(f);

        assert_eq!(app.factory.rooms.len(), 1);
        let r = &app.factory.rooms[0];
        assert!(r.is_built());
        assert_eq!(r.name, "Lobby", "the asked name is the built name");
        assert!((r.base_z - 1.2).abs() < 1e-4, "start height: {}", r.base_z);
        assert!((r.height - 3.1).abs() < 1e-4, "clear height: {}", r.height);
        assert!(
            (r.wall_t - 0.3).abs() < 1e-4,
            "wall thickness: {}",
            r.wall_t
        );
        assert!((r.floor_t - 0.2).abs() < 1e-4, "floor slab: {}", r.floor_t);
        assert!(
            (r.ceiling_t - 0.25).abs() < 1e-4,
            "ceiling slab: {}",
            r.ceiling_t
        );
        assert!(r.floor.is_some() && !r.walls.is_empty() && r.ceiling.is_some());

        // The GEOMETRY stands where it was asked: floor slab lift at the start
        // height, walls on top of it, ceiling at base + floor + clear.
        let lift_of = |st: &crate::factory::FactoryState, id: u32| {
            st.model
                .features
                .iter()
                .find(|f| f.id == id)
                .map(|f| f.placement.lift)
                .unwrap_or(f32::NAN)
        };
        assert!((lift_of(&app.factory, r.floor.unwrap()) - 1.2).abs() < 1e-3);
        for w in &r.walls {
            assert!(
                (lift_of(&app.factory, *w) - 1.4).abs() < 1e-3,
                "walls stand on the floor slab at base + floor"
            );
        }
        assert!(
            (lift_of(&app.factory, r.ceiling.unwrap()) - 4.5).abs() < 1e-3,
            "ceiling underside at base + floor + clear height"
        );
        assert_eq!(
            app.undo_stack.len(),
            1,
            "one undoable step for the whole act"
        );
    }

    /// Escaping / cancelling the form abandons the outline: no room, no undo
    /// entry, nothing in the history about a built room.
    #[test]
    fn cancelling_the_room_form_builds_nothing() {
        let mut app = CadApp::default();
        app.doc.units = cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::User);
        app.doc.dobjects.clear();
        let ring = cad_kernel::Geom::Polyline(cad_kernel::Polyline {
            vertices: [(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)]
                .iter()
                .map(|&(x, y)| cad_kernel::PolyVertex {
                    pos: cad_kernel::Vec2::new(x, y),
                    bulge: 0.0,
                })
                .collect(),
            closed: true,
            widths: Vec::new(),
        });
        app.doc.push(cad_kernel::DObject::new(ring));
        app.selection.push(0);
        app.request_make_room();
        assert!(app.room_form.is_some());

        // Exactly what the Esc handler and the Cancel button both do.
        app.room_form = None;
        app.room_form_focus_name = false;

        assert!(app.factory.rooms.is_empty(), "cancelling must not build");
        assert!(
            app.undo_stack.is_empty(),
            "…and must not leave an undo entry"
        );
        assert!(
            !app.history
                .iter()
                .any(|h| h.contains("made from the plan outline")),
            "…and must not claim a room was made"
        );
    }

    /// The demo plan a fresh app ships with must make a REAL room: its closed
    /// rectangle is 6000 × 4500 drawing-units in the default MILLIMETRE
    /// document, so as a room it comes out ~6 × 4.5 m. (The old demo figures
    /// were ±(20–90)-unit centimetre sketches — no room could ever come out
    /// of those.)
    #[test]
    fn the_demo_plan_makes_a_real_metre_room() {
        let mut app = CadApp::default();
        assert_eq!(
            app.doc.units.metres_per_unit, 0.001,
            "a fresh document is millimetre space"
        );
        let ring_idx = app
            .doc
            .dobjects
            .iter()
            .position(|d| matches!(&d.geom, cad_kernel::Geom::Polyline(p) if p.closed))
            .expect("the demo plan ships a closed room outline");
        app.selection.push(ring_idx);

        let outline = app
            .slab_outline_from_selection()
            .expect("the closed demo outline must yield one");
        let (mut mnx, mut mny, mut mxx, mut mxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &outline {
            mnx = mnx.min(p.x);
            mny = mny.min(p.y);
            mxx = mxx.max(p.x);
            mxy = mxy.max(p.y);
        }
        assert!(
            (mxx - mnx - 6.0).abs() < 1e-3,
            "room width ≈ 6 m, got {}",
            mxx - mnx
        );
        assert!(
            (mxy - mny - 4.5).abs() < 1e-3,
            "room depth ≈ 4.5 m, got {}",
            mxy - mny
        );

        app.make_room_from_selected_outline(outline);
        assert_eq!(app.factory.rooms.len(), 1);
        let r = &app.factory.rooms[0];
        assert!(r.is_built(), "the demo outline builds a real room");
        assert!(r.floor.is_some() && r.ceiling.is_some(), "with both slabs");
        assert_eq!(
            r.walls.len(),
            4,
            "a rectangle room has four perimeter walls"
        );
    }

    /// The first-frame hook frames the demo plan once — the launch camera
    /// would otherwise show empty space around the origin — and never touches
    /// the view again.
    #[test]
    fn the_demo_plan_is_framed_once_on_first_launch() {
        let mut app = CadApp::default();
        app.canvas_screen_rect = Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1600.0, 1000.0),
        ));
        let (s0, o0) = (app.scale, app.world_offset);

        app.maybe_frame_demo_plan();
        assert!(app.demo_view_set, "the hook must have run");
        assert!(
            app.scale < s0,
            "the plan needs zooming OUT, not the 6 px/unit launch zoom"
        );
        assert_ne!(app.world_offset, o0, "the view recentred on the plan");
        let s1 = app.scale;

        // A second call — and calls once the user has zoomed — are no-ops.
        app.maybe_frame_demo_plan();
        assert_eq!(app.scale, s1, "the view is framed exactly once");
        app.view_history.push((0.5, egui::Vec2::ZERO));
        app.maybe_frame_demo_plan();
        assert_eq!(app.scale, s1, "a user zoom never gets overridden");
    }
}
/// THE QUIT DIALOG MUST STAY REACHABLE.
///
/// Reported as: "when i close the app and the save or not windows shows, if i click accidentally
/// outside the window the app stops responding."
///
/// The dimmed backdrop and the dialog were both `Order::Foreground`. Within one order egui raises
/// an area to the top when you interact with it, so clicking the backdrop put a full-screen,
/// click-swallowing overlay ON TOP of the dialog: the buttons became unreachable, `close_confirm`
/// stayed true and the close stayed vetoed. The app was running and repainting the whole time —
/// it just could not be answered or quit.

#[cfg(test)]
mod the_quit_dialog_cannot_trap_the_app {
    use super::*;

    fn body() -> &'static str {
        let src = include_str!("../mod.rs");
        let a = src.find("fn render_close_confirm").expect("the dialog");
        let b = src[a..]
            .find("\n    /// ")
            .map(|e| a + e)
            .unwrap_or(src.len());
        &src[a..b]
    }

    /// The backdrop must be strictly BELOW the dialog, so no click can raise it over the buttons.
    #[test]
    fn the_backdrop_is_below_the_dialog() {
        let b = body();
        let backdrop = b.find("close_confirm_backdrop").expect("the backdrop");
        let win = b
            .find("Window::new(\"close_confirm\")")
            .expect("the dialog window");
        assert!(
            backdrop < win,
            "the backdrop must be declared before the dialog"
        );

        // The order each one is given, taken from its own section of the function.
        let backdrop_order = b[backdrop..win].contains("Order::Middle");
        let dialog_order = b[win..].contains("Order::Foreground");
        assert!(
            backdrop_order,
            "the backdrop must be Order::Middle — at Foreground a click raises it over the dialog",
        );
        assert!(
            dialog_order,
            "the dialog must be Order::Foreground, above the backdrop"
        );
        assert!(
            !b[backdrop..win].contains("Order::Foreground"),
            "the backdrop must NOT share the dialog's order — that is the reported hang",
        );
    }

    /// And there must be a keyboard way out regardless.
    #[test]
    fn escape_dismisses_it() {
        let b = body();
        let esc = b.find("Key::Escape").expect("Esc must dismiss the dialog");
        let backdrop = b.find("close_confirm_backdrop").unwrap();
        assert!(
            esc < backdrop,
            "the Esc check must run before anything can swallow input"
        );
        assert!(
            b[esc..backdrop].contains("self.close_confirm = false"),
            "Esc must actually clear the flag that vetoes the close",
        );
    }

    /// Answering it must not leave the app permanently unable to quit: every button clears
    /// `close_confirm`, and exactly one of them authorises the close.
    #[test]
    fn every_button_clears_the_veto() {
        let b = body();
        assert_eq!(
            b.matches("self.close_confirm = false").count(),
            4,
            "Esc + Save + Don't Save + Cancel must each clear the veto",
        );
        assert!(
            b.contains("self.pending_close = true"),
            "Close without saving authorises the close"
        );
        assert!(
            b.contains("self.close_after_save = true"),
            "Save and close authorises it after the save"
        );
    }
}
/// Reported as: "when i try to import an ies/ldt file in the block to fitting tab its not
/// detecting the ies/ldt files in the folder. instead its opening this" — with a screenshot of a
/// window titled "Choose output folder · Radiance render", listing "(no folders or * files here)",
/// pointed at a directory full of .ldt files.
///
/// Three separate things, one dialog:
///   * `PickFolder` listed NO files at all, "folders only". Right for choosing where to WRITE,
///     wrong for choosing a folder because of what is IN it.
///   * the title was the Radiance one whichever of the three callers had opened it;
///   * and the empty-list line printed the filter raw, so an empty filter read "* files".

#[cfg(test)]
mod the_folder_browser_shows_what_is_in_the_folder {
    use super::*;

    /// THE REPORTED CASE. A photometry folder pick must list photometry.
    #[test]
    fn a_photometry_folder_pick_lists_ies_and_ldt() {
        for name in ["FONDO.ldt", "FONDO.LDT", "downlight.ies", "TRACK.IES"] {
            assert!(
                CadApp::dialog_lists(FileDialogMode::PickFolder, ".ies|.ldt", name),
                "{name} was not listed, so the folder looks empty",
            );
        }
    }

    /// …AND NOTHING ELSE, or the list becomes every file on disk and says nothing.
    #[test]
    fn a_photometry_folder_pick_lists_only_photometry() {
        for name in ["plan.dxf", "notes.txt", "render.hdr", "ldt", "ies.txt"] {
            assert!(
                !CadApp::dialog_lists(FileDialogMode::PickFolder, ".ies|.ldt", name),
                "{name} has no business in a photometry list",
            );
        }
    }

    /// AN OUTPUT FOLDER STILL SHOWS FOLDERS ONLY. You are choosing where to WRITE, and what
    /// happens to be there already is none of the caller's business — so an empty filter keeps the
    /// old behaviour exactly, and the Radiance picker is unchanged.
    #[test]
    fn an_output_folder_pick_lists_no_files() {
        for name in ["anything.ldt", "old_render.hdr", "plan.dxf"] {
            assert!(
                !CadApp::dialog_lists(FileDialogMode::PickFolder, "", name),
                "{name} appeared in an output-folder pick",
            );
        }
    }

    /// EVERY OTHER MODE IS UNTOUCHED. The filter was moved out of the directory walk to make it
    /// testable, and a refactor that quietly changed one of these would be worse than the defect.
    #[test]
    fn the_other_modes_filter_exactly_as_before() {
        let cases: &[(FileDialogMode, &str, &[&str], &[&str])] = &[
            (
                FileDialogMode::Open,
                "",
                &["a.dxf", "a.rsm", "a.DWG"],
                &["a.ies", "a.png"],
            ),
            (
                FileDialogMode::ImportIes,
                "",
                &["a.ies", "a.LDT"],
                &["a.dxf", "a.png"],
            ),
            (
                FileDialogMode::ImportHdri,
                "",
                &["sky.hdr", "sky.EXR"],
                &["sky.png", "sky.dxf"],
            ),
            (
                FileDialogMode::ImportObj,
                "",
                &["c.obj", "c.3ds", "c.fbx", "c.glb", "c.gltf"],
                &["c.dxf"],
            ),
            (
                FileDialogMode::ImportTexture,
                "",
                &["t.png", "t.JPG", "t.tiff"],
                &["t.dxf", "t.hdr"],
            ),
            (FileDialogMode::Save, ".rsm", &["out.rsm"], &["out.dxf"]),
        ];
        for (mode, ext, yes, no) in cases {
            for n in *yes {
                assert!(
                    CadApp::dialog_lists(*mode, ext, n),
                    "{mode:?} must list {n}"
                );
            }
            for n in *no {
                assert!(
                    !CadApp::dialog_lists(*mode, ext, n),
                    "{mode:?} must not list {n}"
                );
            }
        }
    }

    /// A SAVE WITH NO EXTENSION LISTS NOTHING rather than everything. `ends_with("")` is true for
    /// every string, so the empty-filter guard is load-bearing here as well as on the folder pick.
    #[test]
    fn an_empty_filter_never_matches_everything() {
        assert!(!CadApp::dialog_lists(
            FileDialogMode::Save,
            "",
            "anything.at.all"
        ));
    }

    /// THE TITLE NAMES THE FOLDER YOU ARE ACTUALLY CHOOSING. Three callers share one dialog mode,
    /// and it announced a Radiance render to all three.
    #[test]
    fn the_folder_dialog_is_titled_for_whoever_asked() {
        let mut app = CadApp::default();
        assert!(
            app.folder_dialog_title().contains("Radiance"),
            "the default caller is the render output folder",
        );

        app.photometry_wants_folder = true;
        let t = app.folder_dialog_title();
        assert!(
            !t.contains("Radiance") && (t.contains(".ldt") || t.contains(".ies")),
            "the photometry pick is titled {t:?}",
        );

        app.photometry_wants_folder = false;
        app.texset_target = Some(0);
        let t = app.folder_dialog_title();
        assert!(
            !t.contains("Radiance"),
            "the texture-set pick is titled {t:?}"
        );
    }

    /// CLICKING A FILE IN A FOLDER PICK MUST NOT GO THROUGH `filename`.
    ///
    /// Reported as "why cant i select it", looking at a browser listing FONDO.ldt. The files were
    /// deliberately inert, because a folder pick reads `filename` as the NEW SUBFOLDER TO CREATE —
    /// so selecting FONDO.ldt would have offered to make a directory called FONDO.ldt inside the
    /// folder the user was trying to choose.
    ///
    /// Clicking one now confirms the FOLDER directly, and the `clear()` before it is the whole of
    /// what keeps the old hazard shut. Asserted on the source because the click needs a live egui
    /// context, and because the thing worth protecting is the ORDER of two statements — which no
    /// behavioural test of the finished dialog would notice until somebody had already lost a
    /// folder to it.
    #[test]
    fn a_folder_pick_never_confirms_with_a_file_name_in_the_subfolder_field() {
        let src = include_str!("../mod.rs");
        let anchor = "IN A FOLDER PICK, CLICKING A FILE TAKES ITS FOLDER.";
        let a = src.find(anchor).expect("the folder-pick file row");
        // The row ends where the ordinary (non-folder) rows resume.
        let b = src[a..]
            .find("let selected = dlg.filename.eq_ignore_ascii_case(f);")
            .map(|e| a + e)
            .expect("the ordinary file row follows it");
        let body = &src[a..b];

        assert!(
            body.contains("do_confirm = true;"),
            "clicking a file in a folder pick does nothing — which is what was reported",
        );
        let clear = body.find("dlg.filename.clear();").expect(
            "the file name must be cleared, or the folder pick treats it as a subfolder to create",
        );
        let confirm = body.find("do_confirm = true;").expect("checked above");
        assert!(
            clear < confirm,
            "the clear must come BEFORE the confirm, or FONDO.ldt is read as a new subfolder",
        );
    }

    /// AND THE PREVIEW PANE STOPS ASKING FOR SOMETHING THIS MODE CANNOT DO. "(select a file)" is
    /// right for an Open and, in a folder pick, invites the one action that used to do nothing.
    #[test]
    fn a_folder_pick_does_not_ask_you_to_select_a_file() {
        let src = include_str!("../mod.rs");
        let a = src
            .find("SAY WHAT THIS MODE ACTUALLY WANTS")
            .expect("the preview hint");
        let b = src[a..]
            .find("crate::theme::typ::data_code()")
            .map(|e| a + e)
            .unwrap();
        let body = &src[a..b];
        assert!(
            body.contains("FileDialogMode::PickFolder"),
            "the preview hint no longer distinguishes a folder pick",
        );
    }
}
