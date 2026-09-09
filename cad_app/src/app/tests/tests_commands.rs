use super::super::*;

#[cfg(test)]
mod established_commands_are_untouchable {
    use super::*;

    /// MOVE IS MOVE. The established modifiers are ONE command each — there is no
    /// "3D Move". A previous change added a command-line intercept that hijacked `m`
    /// out of the 2D drawing into an invented 3D op (and, with no 3D selection, even
    /// BLOCKED it). This test exists so that can never come back.
    ///
    /// The canonical flow (owner, verbatim):
    ///   1. move is an established command
    ///   2. Move
    ///   3. select what is going to move     ← selection comes AFTER the verb, which
    ///   4. first point                        is why any "route by what's selected
    ///   5. second Point                       when the verb is typed" rule is wrong
    ///   6. and done
    ///
    /// If 3D solids ever need these verbs, dispatch by object type at APPLY time —
    /// never by intercepting the command.
    /// The dispatch signal must be the ACTIVE view, never "is the 3D panel open".
    /// `factory.open` is what hijacked `m` out of the 2D drawing; `active_view` only
    /// changes when you actually work inside a viewport.
    #[test]
    fn dispatch_keys_on_active_view_never_on_panel_open() {
        let src = include_str!("../mod.rs");
        // NOTE: `run_command` is now a thin timing wrapper — inspecting IT would make
        // this test vacuous. The dispatcher is `run_command_inner`.
        let start = src
            .find("fn run_command_inner")
            .expect("the dispatcher exists");
        let end = src[start..]
            .find("\n    fn ")
            .map(|e| start + e)
            .unwrap_or(src.len());
        // Check CODE, not prose — the dispatch block deliberately *documents* why it
        // does not gate on `factory.open`, and a naive string search trips on that.
        for line in src[start..end].lines() {
            let l = line.trim();
            if l.starts_with("//") || l.starts_with("///") {
                continue;
            }
            assert!(
                !l.contains("factory.open"),
                "run_command must NEVER gate on `factory.open` — a panel being visible \
                 says nothing about what you are editing (this is the bug that stole \
                 `m`). Offending line: {l}"
            );
        }
    }

    /// The 2D modifiers must run their normal select-first flow, with the 3D panel
    /// open, closed, or holding a stale selection — the panel is irrelevant to them.
    #[test]
    fn modifiers_run_the_established_2d_flow_regardless_of_the_3d_panel() {
        for (open, sel3d) in [(false, vec![]), (true, vec![]), (true, vec![1u32])] {
            for verb in ["move", "m", "copy", "rotate", "scale", "mirror"] {
                let mut app = CadApp::default();
                app.factory.open = open;
                app.factory.selection = sel3d.clone();
                app.selection.clear();
                app.run_command(verb);
                assert_eq!(
                    app.select_mode,
                    SelectMode::ForSelect,
                    "`{verb}` must start the established 2D select-first flow \
                     (3d panel open={open}, stale 3d sel={sel3d:?})"
                );
            }
        }
    }
}
#[cfg(test)]
mod cmd_timing_tests {
    use super::*;
    /// Every command must record HOW LONG it took, not just when it ran — "if
    /// something [is slow], we know what is the reason".
    #[test]
    fn a_command_records_its_execution_time() {
        let mut app = CadApp::default();
        app.dbg.recording = true;
        app.dbg.session_started = Some(std::time::Instant::now());
        app.run_command("move");
        let stamped = app.dbg.events.iter().any(|r| {
            matches!(
                &r.event,
                crate::dbg_recorder::DbgEvent::CmdRun {
                    elapsed_us: Some(_),
                    ..
                }
            )
        });
        assert!(stamped, "the CmdRun must carry an execution time");
    }

    /// A command past one frame (16 ms @ 60 Hz) is flagged ⚠ SLOW — that is the
    /// threshold where the user actually SEES the stall.
    #[test]
    fn slow_commands_are_flagged_in_the_dump() {
        use crate::dbg_recorder::{format_event_oneline, CmdSource, DbgEvent};
        let ev = |us: u64| {
            format_event_oneline(&DbgEvent::CmdRun {
                raw: "x".into(),
                parsed_debug: "X".into(),
                source: CmdSource::Typed,
                elapsed_us: Some(us),
            })
        };
        assert!(ev(500).contains("µs"), "sub-ms shows µs");
        assert!(ev(5_000).contains("5.0 ms") && !ev(5_000).contains("SLOW"));
        assert!(ev(20_000).contains("⚠ SLOW"), "past a frame → flagged");
    }

    /// RE-ENTRANCY: a select-session `p`/`l`/`d` re-enters run_command. Each CmdRun
    /// must get its OWN time — patching "the last one" would let the nested call steal
    /// the outer command's slot, and both would read wrong.
    #[test]
    fn nested_commands_each_get_their_own_time() {
        let mut app = CadApp::default();
        app.dbg.recording = true;
        app.dbg.session_started = Some(std::time::Instant::now());
        app.begin_selection(SelectMode::ForSelect);
        app.run_command("p"); // → rewrites to run_command("previous"), re-entrant
        let cmds: Vec<_> = app
            .dbg
            .events
            .iter()
            .filter_map(|r| match &r.event {
                crate::dbg_recorder::DbgEvent::CmdRun {
                    raw, elapsed_us, ..
                } => Some((raw.clone(), *elapsed_us)),
                _ => None,
            })
            .collect();
        assert!(
            cmds.len() >= 2,
            "outer `p` + nested `previous`, got {cmds:?}"
        );
        for (raw, us) in &cmds {
            assert!(
                us.is_some(),
                "`{raw}` was left unstamped — a nested call stole its slot"
            );
        }
    }
}
/// ONE command line, TWO windows.
///
/// Asked for as: "we will have a 3d command window. the 2d cad has a command window where the
/// parameters of a tool can be entered. we will use the same window. the user is using the 3d
/// factory[,] the command window's name will change to 3d command. and when the user click[s] on
/// the 2d window it changes back to just command so now it works for the cad. the 2d command
/// window's command won't work on the 3d factory or vice versa[;] for it to work the user ha[s] to
/// click on the window they want access to."

#[cfg(test)]
mod command_target {
    use super::*;

    fn app_in_3d() -> CadApp {
        let mut app = CadApp::default();
        app.factory.open = true;
        app.active_view = ActiveView::ThreeD;
        app
    }

    /// The title is a readout of the thing that actually decides, not a second opinion.
    #[test]
    fn the_target_follows_the_window_you_last_touched() {
        let mut app = app_in_3d();
        assert_eq!(app.command_target(), ActiveView::ThreeD);
        app.active_view = ActiveView::TwoD; // …a click on the plan
        assert_eq!(app.command_target(), ActiveView::TwoD);
    }

    /// A viewport that is not on screen cannot be the one you are working in. Without this the
    /// command line would keep swallowing 2D commands after the Factory panel was closed.
    #[test]
    fn closing_the_factory_hands_the_command_line_back() {
        let mut app = app_in_3d();
        app.factory.open = false;
        assert_eq!(app.command_target(), ActiveView::TwoD);
    }

    /// THE BOUNDARY, 2D → 3D. `line` draws on the plan; against a 3D model it means nothing, and
    /// running it anyway is how a command "silently does nothing".
    #[test]
    fn a_2d_drawing_command_is_refused_in_the_3d_window() {
        let mut app = app_in_3d();
        let before = app.doc.dobjects.len();
        app.run_command("line");
        assert_eq!(app.doc.dobjects.len(), before, "nothing may be drawn");
        let last = app.history.last().cloned().unwrap_or_default();
        assert!(last.contains("2D command"), "it must SAY why, got {last:?}");
        assert!(
            last.contains("click the 2D window"),
            "…and name the way out, got {last:?}"
        );
    }

    /// …and the same command in its own window is untouched. A boundary that also blocks the thing
    /// it is meant to protect is just a break.
    #[test]
    fn the_same_command_still_works_in_the_2d_window() {
        let mut app = app_in_3d();
        app.active_view = ActiveView::TwoD;
        app.run_command("line");
        let last = app.history.last().cloned().unwrap_or_default();
        assert!(
            !last.contains("2D command"),
            "2D must keep its own vocabulary, got {last:?}"
        );
    }

    /// THE BOUNDARY, 3D → 2D. A Factory word typed at the 2D prompt is someone talking to the wrong
    /// window; "unknown command" would read as "that is not a thing".
    #[test]
    fn a_3d_command_is_refused_in_the_2d_window() {
        let mut app = CadApp::default();
        app.active_view = ActiveView::TwoD;
        app.run_command("place");
        let last = app.history.last().cloned().unwrap_or_default();
        assert!(last.contains("3D Factory command"), "got {last:?}");
        assert!(last.contains("click the 3D window"), "got {last:?}");
    }

    /// Shared commands are shared. `move` dispatches by active view and must not be caught by
    /// either refusal.
    #[test]
    fn a_shared_command_crosses_freely() {
        for view in [ActiveView::TwoD, ActiveView::ThreeD] {
            let mut app = app_in_3d();
            app.active_view = view;
            app.run_command("move");
            let last = app.history.last().cloned().unwrap_or_default();
            assert!(!last.contains("is a 2D command"), "{view:?}: {last:?}");
            assert!(
                !last.contains("is a 3D Factory command"),
                "{view:?}: {last:?}"
            );
        }
    }

    // ---- the placement vocabulary --------------------------------------------------------

    #[test]
    fn the_mode_words_set_the_mode() {
        use crate::factory::PlaceMode;
        let mut app = app_in_3d();
        for (word, want) in [
            ("origin", PlaceMode::Origin),
            ("centre", PlaceMode::Centre),
            ("center", PlaceMode::Centre),
            ("click", PlaceMode::Click),
        ] {
            app.run_command(word);
            assert_eq!(app.factory.place_mode, want, "typing '{word}'");
        }
    }

    /// `@900,0,0` in millimetres is 0.9 m — typed lengths go through the FACTORY's working unit
    /// like every other 3D number. Reading them as metres is the class of bug that built a 4.4 km
    /// building.
    #[test]
    fn at_coordinates_are_typed_in_the_working_unit() {
        let mut app = app_in_3d();
        app.factory.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::MM,
            cad_kernel::UnitSource::User,
        );
        app.run_command("@900,0,0");
        assert_eq!(app.factory.place_mode, crate::factory::PlaceMode::Offset);
        assert!(
            (app.factory.place_offset[0] - 0.9).abs() < 1e-6,
            "{:?}",
            app.factory.place_offset
        );
    }

    /// Both forms were described — "they will type @(their coordinates)" — and neither is wrong, so
    /// both parse, with a comma or a space between the numbers.
    #[test]
    fn the_coordinate_may_be_written_several_ways() {
        let u = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::M,
            cad_kernel::UnitSource::User,
        );
        for form in [
            "@900,0,0",
            "@(900,0,0)",
            "@(900, 0, 0)",
            "@900 0 0",
            "@900,0",
        ] {
            let got = CadApp::parse_at_coords(form, &u, &crate::calc::CalcStore::new())
                .unwrap_or_else(|| panic!("{form} must parse"));
            assert!((got[0] - 900.0).abs() < 1e-3, "{form} → {got:?}");
            assert!(
                got[1].abs() < 1e-6 && got[2].abs() < 1e-6,
                "{form} → {got:?}"
            );
        }
        // …and nonsense is refused rather than silently read as zero.
        for bad in ["@", "@abc", "@1", "@1,2,3,4", "900,0,0"] {
            assert!(
                CadApp::parse_at_coords(bad, &u, &crate::calc::CalcStore::new()).is_none(),
                "{bad} must not parse"
            );
        }
    }

    /// The coordinate sets the OFFSET and nothing else. An object already waiting for its click
    /// keeps waiting — typing a distance from the origin and placing the thing in front of you are
    /// two different intentions.
    #[test]
    fn a_typed_coordinate_leaves_a_waiting_object_waiting() {
        let mut app = app_in_3d();
        app.factory.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::M,
            cad_kernel::UnitSource::User,
        );
        app.factory.add_box();
        let id = *app.factory.selection.first().unwrap();
        let before = app
            .factory
            .model
            .features
            .iter()
            .find(|f| f.id == id)
            .map(|f| (f.placement.u, f.placement.v))
            .unwrap();
        app.run_command("@12,34");
        let after = app
            .factory
            .model
            .features
            .iter()
            .find(|f| f.id == id)
            .map(|f| (f.placement.u, f.placement.v))
            .unwrap();
        assert_eq!(before, after, "the waiting box must not move");
        assert!(
            (app.factory.place_offset[0] - 12.0).abs() < 1e-6,
            "…but the offset is set"
        );
    }

    /// Switching away from Click while something waits would strand it: armed forever, with no
    /// click coming for it.
    #[test]
    fn leaving_click_mode_releases_whatever_was_waiting() {
        let mut app = app_in_3d();
        app.factory.add_box();
        assert!(app.factory.awaiting_place.is_some());
        app.run_command("origin");
        assert!(
            app.factory.awaiting_place.is_none(),
            "nothing may be left waiting"
        );
    }

    /// `place` ASKS, in brackets, the way every other command in this app asks — "Specify center
    /// point for circle or [3P/2P/Ttr]:". The dropdown that used to ask this is gone.
    #[test]
    fn place_opens_a_bracketed_prompt() {
        let mut app = app_in_3d();
        app.run_command("place");
        assert!(
            app.place_prompt_open,
            "the prompt must be waiting for an answer"
        );
        let p = app.current_prompt.clone();
        for want in ["Placement", "Click", "Centre", "Origin", "Offset"] {
            assert!(
                p.contains(want),
                "the options must be visible: {p:?} is missing {want}"
            );
        }
        assert!(
            p.contains("<click>"),
            "and the current setting is the default: {p:?}"
        );
    }

    /// The next line answers it.
    #[test]
    fn answering_the_prompt_sets_the_mode() {
        let mut app = app_in_3d();
        app.run_command("place");
        app.run_command("origin");
        assert_eq!(app.factory.place_mode, crate::factory::PlaceMode::Origin);
        assert!(!app.place_prompt_open, "…and the prompt closes");
        assert!(app.current_prompt.is_empty());
    }

    /// A typo at an open prompt is a typo, not a new command: say what was expected and keep
    /// asking, exactly as the 2D prompts do.
    #[test]
    fn a_bad_answer_keeps_asking() {
        let mut app = app_in_3d();
        app.run_command("place");
        app.run_command("banana");
        assert!(app.place_prompt_open, "the question is still open");
        assert_eq!(
            app.factory.place_mode,
            crate::factory::PlaceMode::Click,
            "nothing changed"
        );
        let last = app.history.last().cloned().unwrap_or_default();
        assert!(last.contains("not one of the options"), "got {last:?}");
    }

    /// CHOOSING OFFSET ASKS FOR THE COORDINATE, THERE AND THEN.
    ///
    /// Reported as: "once a co ordinate is chosen it keeps on placing on the same co ordinate. why
    /// is that[?] the user need to be able to select a new coordinate." It did keep the old one,
    /// correctly by its own rules — choosing `offset` set only the MODE and left a note saying to
    /// type `@X,Y,Z` on a separate line. The session shows `place` → `offset` twice with no `@`
    /// after either, and every object landing on a coordinate set twenty minutes earlier.
    #[test]
    fn choosing_offset_asks_for_the_coordinate() {
        let mut app = app_in_3d();
        app.run_command("place");
        app.run_command("offset");
        assert!(app.place_coord_prompt, "it must ask for the coordinate");
        assert!(
            app.current_prompt.contains("Distance from origin"),
            "got {:?}",
            app.current_prompt,
        );
    }

    /// …and the answer is taken WITHOUT the `@`. The prompt says "X,Y,Z"; insisting on punctuation
    /// is a way of rejecting the right answer.
    #[test]
    fn the_coordinate_answer_needs_no_at_sign() {
        let mut app = app_in_3d();
        app.factory.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::MM,
            cad_kernel::UnitSource::User,
        );
        app.run_command("place");
        app.run_command("offset");
        app.run_command("200,300,500");
        assert!(!app.place_coord_prompt, "the question is answered");
        let o = app.factory.place_offset;
        assert!(
            (o[0] - 0.2).abs() < 1e-6 && (o[1] - 0.3).abs() < 1e-6 && (o[2] - 0.5).abs() < 1e-6,
            "got {o:?}"
        );
        // …and `@` is still fine, because that is what was taught first.
        app.run_command("place");
        app.run_command("offset");
        app.run_command("@1000,0,0");
        assert!((app.factory.place_offset[0] - 1.0).abs() < 1e-6);
    }

    /// A SECOND coordinate replaces the first. This is the whole complaint.
    #[test]
    fn a_new_coordinate_replaces_the_old_one() {
        let mut app = app_in_3d();
        app.factory.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::M,
            cad_kernel::UnitSource::User,
        );
        app.run_command("@1,2,3");
        assert!((app.factory.place_offset[0] - 1.0).abs() < 1e-6);
        app.run_command("place");
        app.run_command("offset");
        app.run_command("7,8,9");
        let o = app.factory.place_offset;
        assert!(
            (o[0] - 7.0).abs() < 1e-6 && (o[1] - 8.0).abs() < 1e-6 && (o[2] - 9.0).abs() < 1e-6,
            "the old coordinate was kept: {o:?}",
        );
    }

    /// Bare Enter at the coordinate prompt keeps what is there — the <default> in the brackets.
    #[test]
    fn the_coordinate_prompt_has_a_default() {
        let mut app = app_in_3d();
        app.factory.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::M,
            cad_kernel::UnitSource::User,
        );
        app.run_command("@4,5,6");
        app.run_command("place");
        app.run_command("offset");
        assert!(
            app.current_prompt.contains('4'),
            "the default must be visible: {:?}",
            app.current_prompt
        );
    }

    /// Nonsense keeps asking rather than falling through as a command — the same rule the mode
    /// question follows.
    #[test]
    fn a_bad_coordinate_keeps_asking() {
        let mut app = app_in_3d();
        app.run_command("place");
        app.run_command("offset");
        app.run_command("over there");
        assert!(app.place_coord_prompt, "still asking");
        let last = app.history.last().cloned().unwrap_or_default();
        assert!(last.contains("not a coordinate"), "got {last:?}");
    }
}

/// ASK WHAT UNIT THIS IS, ONCE.
///
/// Asked for as: "theres a chance a user can miss it and draw with the wrong units. lets add a pop
/// up dialogue box when the user opens 3d factory for the 1st time. this shouldn't show every time
/// the user opens 3d factory. once set they can go in the place to change units but make sure a
/// command window opens everytime the user opens 3d factory."
///
/// Two frequencies on purpose: the dialog once, the command window every time.

#[cfg(test)]
mod unit_prompt {
    use super::*;

    /// One frame's worth of the open-transition check, which is all `update` does for this.
    fn tick(app: &mut CadApp) {
        if app.factory.open && !app.factory_was_open {
            app.on_factory_opened();
        }
        app.factory_was_open = app.factory.open;
    }

    #[test]
    fn opening_the_factory_the_first_time_asks() {
        let mut app = CadApp::default();
        assert!(
            !app.factory.ask_unit,
            "nothing is asked before the Factory is opened"
        );
        app.factory.open = true;
        tick(&mut app);
        assert!(app.factory.ask_unit, "the first open must ask");
    }

    /// THE POINT OF "once". A modal on every visit gets dismissed unread.
    #[test]
    fn it_does_not_ask_again_once_answered() {
        let mut app = CadApp::default();
        app.factory.open = true;
        tick(&mut app);
        // Answer it, the way the dialog does.
        app.factory.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::CM,
            cad_kernel::UnitSource::User,
        );
        app.factory.unit_asked = true;
        app.factory.ask_unit = false;

        // Close and reopen, twice.
        for _ in 0..2 {
            app.factory.open = false;
            tick(&mut app);
            app.factory.open = true;
            tick(&mut app);
            assert!(
                !app.factory.ask_unit,
                "asked again after it had been answered"
            );
        }
        assert!(
            (app.factory.units.metres_per_unit - cad_kernel::Units::CM).abs() < 1e-12,
            "…and the answer stands",
        );
    }

    /// The command window opens EVERY time — that is the half that is meant to be unmissable.
    #[test]
    fn the_command_window_opens_every_time() {
        let mut app = CadApp::default();
        app.factory.unit_asked = true; // already answered; only the window behaviour is under test
        for _ in 0..3 {
            app.cmd_window_open = false; // …the user closed it
            app.factory.open = false;
            tick(&mut app);
            app.factory.open = true;
            tick(&mut app);
            assert!(
                app.cmd_window_open,
                "the command window must come back every time"
            );
        }
    }

    /// …and it says which unit, because a window that opens and says nothing warns nobody.
    #[test]
    fn it_states_the_working_unit_on_opening() {
        let mut app = CadApp::default();
        app.factory.unit_asked = true;
        app.factory.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::FOOT,
            cad_kernel::UnitSource::User,
        );
        app.factory.open = true;
        tick(&mut app);
        let last = app.history.last().cloned().unwrap_or_default();
        assert!(last.contains("working unit"), "got {last:?}");
        assert!(
            last.contains(&app.factory.units.label()),
            "…and names it: {last:?}"
        );
    }

    /// Staying open is not re-opening. The transition is the rising edge, or every frame would
    /// re-open the command window and the user could never close it.
    #[test]
    fn a_factory_that_stays_open_does_not_re_fire() {
        let mut app = CadApp::default();
        app.factory.unit_asked = true;
        app.factory.open = true;
        tick(&mut app);
        let n = app.history.len();
        app.cmd_window_open = false; // the user closes it while the Factory is still open
        for _ in 0..5 {
            tick(&mut app);
        }
        assert_eq!(
            app.history.len(),
            n,
            "it announced itself again without being reopened"
        );
        assert!(!app.cmd_window_open, "…and forced the window back open");
    }

    /// The answer belongs to the PROJECT, and travels with it.
    #[test]
    fn the_answer_survives_a_save() {
        let mut st = crate::factory::FactoryState::default();
        st.unit_asked = true;
        let doc = st.to_persist();
        let mut back = crate::factory::FactoryState::default();
        back.apply_persist(doc);
        assert!(
            back.unit_asked,
            "a reopened project must not be asked again"
        );
    }

    /// A project written before the flag existed still RECORDS its unit, and a recorded unit is an
    /// answer. Re-asking someone who already told us is exactly the "shows every time" this is
    /// meant to avoid.
    #[test]
    fn an_older_project_that_declares_a_unit_is_not_asked() {
        let mut st = crate::factory::FactoryState::default();
        st.units = cad_kernel::Units::from_metres_per_unit(
            cad_kernel::Units::MM,
            cad_kernel::UnitSource::User,
        );
        let mut doc = st.to_persist();
        doc.unit_asked = None; // as written by a build that predates the flag
        assert!(
            doc.working_unit_m > 0.0,
            "precondition: it does record a unit"
        );

        let mut back = crate::factory::FactoryState::default();
        back.apply_persist(doc);
        assert!(back.unit_asked, "a declared unit IS the answer");
    }

    /// A brand-new project has nothing recorded, so it IS asked.
    #[test]
    fn a_project_with_no_unit_recorded_is_asked() {
        let mut doc = crate::factory::FactoryState::default().to_persist();
        doc.unit_asked = None;
        doc.working_unit_m = 0.0;
        let mut back = crate::factory::FactoryState::default();
        back.unit_asked = true; // …even if this session had already answered for another project
        back.apply_persist(doc);
        assert!(!back.unit_asked);
    }
}

/// THE PICKED FACE, OUTLINED IN YELLOW IN 3D.
///
/// Asked for as: "when a face is selected to sketch, have a yellow outline show up in the 3d view
/// so the user can know if he selected the right face in 3d." The 2D canvas swings onto the plane
/// the moment a face is picked, and the 3D view said nothing about WHICH face that was — on a
/// building with a dozen similar walls the only way to find out was to draw something and see where
/// it landed.
#[cfg(test)]
mod layerstate_flow_tests {
    use super::*;

    fn add_layer(app: &mut CadApp, name: &str) -> u32 {
        app.doc.layers.add(cad_kernel::Layer {
            name: name.into(),
            color: cad_kernel::color::Color::Aci(7),
            linetype: 0,
            lineweight: cad_kernel::lineweight::Lineweight::Default,
            visible: true,
            locked: false,
            frozen: false,
            plottable: true,
            order: 0,
        })
    }

    #[test]
    fn save_restore_via_command() {
        let mut app = CadApp::default();
        let id = add_layer(&mut app, "TestLayerAlpha");
        if let Some(l) = app.doc.layers.get_mut(id) {
            l.visible = false;
        }
        app.run_command("layerstate save TestState");
        assert_eq!(app.doc.layer_states.len(), 1, "state stored");
        assert!(
            !app.doc.layer_states[0]
                .entries
                .iter()
                .find(|e| e.layer == "TestLayerAlpha")
                .unwrap()
                .visible,
            "captured hidden"
        );
        assert!(app.history.iter().any(|h| h.contains("saved 'TestState'")));
        // Restore flips visibility back on.
        if let Some(l) = app.doc.layers.get_mut(id) {
            l.visible = true;
        }
        app.apply_layerstate(&["TestState".into()]); // direct (bypass parser)
        assert!(!app.doc.layers.get(id).unwrap().visible, "restored");
        // Undo restores the visible=true state.
        app.do_undo();
        assert!(app.doc.layers.get(id).unwrap().visible);
    }

    #[test]
    fn list_delete_rename() {
        let mut app = CadApp::default();
        app.run_command("layerstate save A");
        app.run_command("layerstate save B");
        app.run_command("layerstate ?");
        assert!(app.history.iter().any(|h| h.contains("A")));
        app.run_command("layerstate delete A");
        app.run_command("layerstate rename B C");
        app.run_command("layerstate ?");
        assert!(
            app.history.iter().any(|h| h.contains("C"))
                && !app.history.iter().any(|h| h.contains("list: A"))
        );
    }

    #[test]
    fn unknown_state_fails_visibly() {
        let mut app = CadApp::default();
        app.run_command("layerstate restore Nope");
        assert!(app.history.iter().any(|h| h.contains("no state named")));
        // Restore with no args lists.
        app.run_command("layerstate");
        assert!(app.history.iter().any(|h| h.contains("layerstate:")));
    }
}
#[cfg(test)]
mod ucs_flow_tests {
    use super::*;

    #[test]
    fn ucs_origin_creates_and_sets_current() {
        let mut app = CadApp::default();
        app.run_command("ucs origin 100,200");
        assert_eq!(app.doc.current_ucs, 1);
        let u = app.current_ucs().cloned().unwrap();
        assert_eq!((u.origin.x, u.origin.y), (100.0, 200.0));
        assert_eq!(u.rotation, 0.0);
        // Typed points land in WORLD space (UCS converted).
        assert!((app.ucs_to_world(Vec2::new(1.0, 2.0)) - Vec2::new(101.0, 202.0)).len() < 1e-9);
        // Readout shows UCS coords.
        assert!((app.world_to_ucs(Vec2::new(101.0, 202.0)) - Vec2::new(1.0, 2.0)).len() < 1e-9);
    }

    #[test]
    fn ucs_save_list_set_delete() {
        let mut app = CadApp::default();
        app.run_command("ucs origin 10,20 90");
        app.run_command("ucs save Site");
        assert_eq!(app.doc.ucs_list.len(), 2, "unnamed + named");
        assert_eq!(app.doc.current_ucs, 2);
        assert_eq!(app.ucs_name(), "Site");
        // Switch back to world.
        app.run_command("ucs world");
        assert_eq!(app.ucs_name(), "World");
        // Re-select by name.
        app.run_command("ucs site");
        assert_eq!(app.ucs_name(), "Site");
        // Rotated UCS: x-axis points +Y (90°).
        let u = app.current_ucs().unwrap();
        let w = u.to_world(Vec2::new(1.0, 0.0));
        assert!(
            (w - Vec2::new(10.0, 21.0)).len() < 1e-9,
            "origin + rotated x-axis"
        );
        // Delete → back to world.
        app.run_command("ucs delete Site");
        assert_eq!(app.ucs_name(), "World");
        assert_eq!(app.doc.ucs_list.len(), 1);
        // Unknown name fails visibly.
        app.run_command("ucs Nope");
        assert!(app.history.iter().any(|h| h.contains("no system named")));
    }

    #[test]
    fn ucs_origin_requires_coords() {
        let mut app = CadApp::default();
        app.run_command("ucs origin");
        assert!(app.history.iter().any(|h| h.contains("origin needs X,Y")));
        assert_eq!(app.doc.current_ucs, 0, "nothing created");
    }
}
#[cfg(test)]
mod overkill_flow_tests {
    use super::*;

    #[test]
    fn overkill_removes_duplicates_from_whole_drawing() {
        let mut app = CadApp::default();
        let before = app.doc.dobjects.len();
        // Add two duplicate lines.
        let l1 = Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(5.0, 0.0),
        });
        let l2 = Geom::Line(cad_kernel::Line {
            a: Vec2::new(5.0, 0.0),
            b: Vec2::new(0.0, 0.0),
        });
        app.add_dobject(l1, "test");
        app.add_dobject(l2, "test");
        assert_eq!(app.doc.dobjects.len(), before + 2);
        app.commit_overkill(None);
        assert_eq!(app.doc.dobjects.len(), before + 1);
        // Undo restores both.
        app.do_undo();
        assert_eq!(app.doc.dobjects.len(), before + 2);
    }

    #[test]
    fn overkill_no_duplicates_fails_visibly() {
        let mut app = CadApp::default();
        let before = app.doc.dobjects.len();
        app.commit_overkill(None);
        assert_eq!(app.doc.dobjects.len(), before, "nothing removed");
        assert!(
            app.history.iter().any(|h| h.contains("no duplicates")),
            "failure is reported: {:?}",
            app.history
        );
    }

    #[test]
    fn overkill_selection_subset() {
        let mut app = CadApp::default();
        let base = app.doc.dobjects.len();
        let c1 = Geom::Circle(cad_kernel::Circle {
            center: Vec2::new(0.0, 0.0),
            radius: 1.0,
        });
        let c2 = Geom::Circle(cad_kernel::Circle {
            center: Vec2::new(0.0, 0.0),
            radius: 1.0,
        });
        let l = Geom::Line(cad_kernel::Line {
            a: Vec2::new(9.0, 9.0),
            b: Vec2::new(10.0, 9.0),
        });
        app.add_dobject(c1, "test");
        app.add_dobject(c2, "test");
        app.add_dobject(l, "test");
        // Select the two circles only.
        app.commit_overkill(Some(vec![base, base + 1]));
        assert_eq!(app.doc.dobjects.len(), base + 2, "line survives");
    }
}
#[cfg(test)]
mod purge_flow_tests {
    use super::*;

    fn add_layer(app: &mut CadApp, name: &str) -> u32 {
        app.doc.layers.add(cad_kernel::Layer {
            name: name.into(),
            color: cad_kernel::color::Color::Aci(7),
            linetype: 0,
            lineweight: cad_kernel::lineweight::Lineweight::Default,
            visible: true,
            locked: false,
            frozen: false,
            plottable: true,
            order: 0,
        })
    }

    #[test]
    fn purge_removes_unused_layer_and_keeps_used() {
        let mut app = CadApp::default();
        // Drain whatever the default doc leaves unused, then test cleanly.
        app.commit_purge();
        let unused_id = add_layer(&mut app, "PurgeMe");
        let used_id = add_layer(&mut app, "KeepMe");
        let mut d = DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(1.0, 0.0),
        }));
        d.style.layer = used_id;
        app.doc.push(d);

        app.commit_purge();
        assert!(app.doc.layers.find("PurgeMe").is_none(), "unused purged");
        let kept = app.doc.layers.find("KeepMe").expect("used kept");
        let l = app
            .doc
            .dobjects
            .iter()
            .rev()
            .find(|x| matches!(x.geom, Geom::Line(_)))
            .unwrap();
        assert_eq!(l.style.layer, kept, "reference remapped");
        assert_eq!(
            app.doc.layers.get(l.style.layer).map(|x| x.name.as_str()),
            Some("KeepMe")
        );
        assert!(app.history.iter().any(|h| h.contains("PurgeMe")));
        let _ = unused_id;
    }

    #[test]
    fn purge_nothing_fails_visibly() {
        let mut app = CadApp::default();
        // First purge drains the default doc's unused entries...
        app.commit_purge();
        // ...so the second has nothing left → visible failure.
        app.commit_purge();
        assert!(
            app.history.iter().any(|h| h.contains("nothing to purge")),
            "failure is reported: {:?}",
            app.history
        );
    }

    #[test]
    fn purge_undo_restores() {
        let mut app = CadApp::default();
        app.commit_purge(); // drain defaults first
        let n_layers = app.doc.layers.layers.len();
        add_layer(&mut app, "Temp");
        assert_eq!(app.doc.layers.layers.len(), n_layers + 1);
        app.commit_purge();
        assert_eq!(app.doc.layers.layers.len(), n_layers);
        app.do_undo();
        assert_eq!(app.doc.layers.layers.len(), n_layers + 1);
    }

    #[test]
    fn purge_clamps_the_active_layer() {
        let mut app = CadApp::default();
        app.commit_purge(); // drain defaults first
                            // Make the active layer an unused one, then purge it away.
        let id = add_layer(&mut app, "ActiveVictim");
        app.doc.layers.active = id;
        app.commit_purge();
        assert!(
            app.doc.layers.find("ActiveVictim").is_none(),
            "victim purged"
        );
        assert_eq!(
            app.doc.layers.active,
            cad_kernel::LayerTable::LAYER_ZERO,
            "active layer clamped back to layer 0, not left dangling"
        );
        // A dobject on the clamped layer is visible/selectable again.
        let mut d = DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(1.0, 0.0),
        }));
        app.doc.push(d);
        let idx = app.doc.dobjects.len() - 1;
        assert!(
            app.doc.is_visible(idx) && app.doc.is_selectable(idx),
            "picks work after the purge"
        );
    }
}
#[cfg(test)]
mod area_lay_flow_tests {
    use super::*;

    fn add_layer(app: &mut CadApp, name: &str) -> u32 {
        app.doc.layers.add(cad_kernel::Layer {
            name: name.into(),
            color: cad_kernel::color::Color::Aci(7),
            linetype: 0,
            lineweight: cad_kernel::lineweight::Lineweight::Default,
            visible: true,
            locked: false,
            frozen: false,
            plottable: true,
            order: 0,
        })
    }

    /// CadApp::default() loads the three-demo-dobject workbench on its own
    /// demo layer — deterministic tests clear it all first.
    fn fresh_app() -> CadApp {
        let mut app = CadApp::default();
        app.run_command("clear");
        app.commit_purge();
        app
    }

    #[test]
    fn area_click_on_circle_reports_its_area() {
        let mut app = CadApp::default();
        app.add_dobject(
            Geom::Circle(cad_kernel::Circle {
                center: Vec2::new(0.0, 0.0),
                radius: 2.0,
            }),
            "test",
        );
        app.run_command("area");
        assert!(app.area_state.is_some(), "area session armed");
        // Click exactly on the circumference (world point ON the circle).
        app.area_click(Vec2::new(2.0, 0.0));
        let expect = std::f64::consts::PI * 4.0;
        assert!(
            app.history
                .iter()
                .any(|h| h.contains(&format!("circle area={:.4}", expect))),
            "circle measured: {:?}",
            app.history
        );
        assert!(
            app.history
                .iter()
                .any(|h| h.contains(&format!("total (+)= {:.4}", expect))),
            "total folded in: {:?}",
            app.history
        );
    }

    #[test]
    fn area_open_object_fails_visibly() {
        let mut app = CadApp::default();
        app.add_dobject(
            Geom::Line(cad_kernel::Line {
                a: Vec2::new(0.0, 0.0),
                b: Vec2::new(4.0, 0.0),
            }),
            "test",
        );
        app.run_command("area");
        // Click ON the line (its distance_to_point is 0 there) — not closed.
        app.area_click(Vec2::new(2.0, 0.0));
        assert!(
            app.history.iter().any(|h| h.contains("not closed")),
            "failure reported: {:?}",
            app.history
        );
    }

    #[test]
    fn area_point_polygon_finishes_and_accumulates() {
        let mut app = fresh_app();
        app.run_command("area");
        // Right triangle (0,0) (4,0) (0,3) → area 6, perimeter 12.
        app.area_click(Vec2::new(0.0, 0.0));
        app.area_click(Vec2::new(4.0, 0.0));
        app.area_click(Vec2::new(0.0, 3.0));
        app.area_finish_polygon();
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("point polygon area=6.0000")),
            "polygon folded: {:?}",
            app.history
        );
        // A second polygon in SUBTRACT mode folds in with a minus sign.
        app.run_command("s"); // subtract mode (area session still live)
        assert!(app.history.iter().any(|h| h.contains("SUBTRACT")));
        app.area_click(Vec2::new(0.0, 0.0));
        app.area_click(Vec2::new(7.0, 0.0));
        app.area_click(Vec2::new(0.0, 2.0));
        app.area_finish_polygon();
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("(subtract): point polygon area=7.0000")),
            "subtract measured: {:?}",
            app.history
        );
        assert!(
            app.history.iter().any(|h| h.contains("total (-)= 1.0000")),
            "6 - 7 leaves a negative running total: {:?}",
            app.history
        );
        // Esc-style cancel clears the session.
        app.area_state = None;
    }

    #[test]
    fn area_needs_three_points_to_finish() {
        let mut app = CadApp::default();
        app.run_command("area");
        app.area_click(Vec2::new(0.0, 0.0));
        app.area_click(Vec2::new(4.0, 0.0));
        app.area_finish_polygon();
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("need at least 3 points")),
            "too-few failure reported: {:?}",
            app.history
        );
    }

    #[test]
    fn layiso_freezes_every_other_layer() {
        let mut app = CadApp::default();
        let keep = add_layer(&mut app, "Keep");
        let other = add_layer(&mut app, "HideMe");
        app.run_command("layiso");
        assert_eq!(app.layer_pick, LayerPickState::Iso);
        app.handle_layer_pick(keep);
        assert!(
            !app.doc.layers.get(keep).unwrap().frozen,
            "kept layer stays"
        );
        assert!(app.doc.layers.get(other).unwrap().frozen, "other frozen");
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("layiso:") && h.contains("'Keep' stays")),
            "report: {:?}",
            app.history
        );
        // One undo entry undoes the freeze.
        app.do_undo();
        assert!(!app.doc.layers.get(other).unwrap().frozen, "undo thaws");
    }

    #[test]
    fn layfrz_and_layoff_target_the_picked_layer() {
        let mut app = CadApp::default();
        let lid = add_layer(&mut app, "Target");
        app.run_command("layfrz");
        app.handle_layer_pick(lid);
        assert!(app.doc.layers.get(lid).unwrap().frozen);
        app.run_command("layoff");
        app.handle_layer_pick(lid);
        assert!(!app.doc.layers.get(lid).unwrap().visible, "off hides");
        assert!(
            app.doc.layers.get(lid).unwrap().frozen,
            "frz state survives layoff"
        );
        app.do_undo();
        app.do_undo();
        let l = app.doc.layers.get(lid).unwrap();
        assert!(l.visible && !l.frozen, "two undos restore: {:?}", l);
    }

    #[test]
    fn laypick_noop_rolls_back_and_reports() {
        let mut app = CadApp::default();
        let lid = add_layer(&mut app, "Noop");
        let depth_before = app.undo_stack.len();
        app.run_command("layoff");
        app.handle_layer_pick(lid);
        assert!(!app.doc.layers.get(lid).unwrap().visible);
        // Second pick on the same (now hidden) layer: nothing changes.
        app.handle_layer_pick(lid);
        assert!(
            app.history.iter().any(|h| h.contains("nothing changed")),
            "no-op reported: {:?}",
            app.history
        );
        assert_eq!(
            app.undo_stack.len(),
            depth_before + 1,
            "no-op leaves no extra undo entry"
        );
    }

    #[test]
    fn layon_restores_all_layers_in_one_undo_step() {
        let mut app = fresh_app();
        let a = add_layer(&mut app, "A");
        let b = add_layer(&mut app, "B");
        app.run_command("layfrz");
        app.handle_layer_pick(a);
        app.run_command("layoff");
        app.handle_layer_pick(b);
        let before = app.undo_stack.len();
        app.run_command("layon");
        let la = app.doc.layers.get(a).unwrap();
        let lb = app.doc.layers.get(b).unwrap();
        assert!(
            la.visible && !la.frozen && lb.visible && !lb.frozen,
            "all restored: A {:?} B {:?}",
            la,
            lb
        );
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("layon: 2 layer(s) restored")),
            "report: {:?}",
            app.history
        );
        // One undo entry undoes the whole LayOn.
        assert_eq!(app.undo_stack.len(), before + 1);
        app.do_undo();
        let la = app.doc.layers.get(a).unwrap();
        let lb = app.doc.layers.get(b).unwrap();
        assert!(
            la.frozen && !lb.visible,
            "undo restores both ops: A {:?} B {:?}",
            la,
            lb
        );
    }

    #[test]
    fn layon_noop_fails_visibly() {
        let mut app = fresh_app();
        let before = app.undo_stack.len();
        app.run_command("layon");
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("no hidden or frozen")),
            "no-op reported: {:?}",
            app.history
        );
        assert_eq!(app.undo_stack.len(), before, "no undo entry for a no-op");
    }

    #[test]
    fn area_and_layer_states_survive_parser_dispatch() {
        // The run_command arms (not just the helpers) must arm the states.
        let mut app = CadApp::default();
        app.run_command("layfrz");
        assert_eq!(app.layer_pick, LayerPickState::Frz);
        app.run_command("layoff");
        assert_eq!(app.layer_pick, LayerPickState::OffLayer);
        app.run_command("layiso");
        assert_eq!(app.layer_pick, LayerPickState::Iso);
        app.run_command("area");
        assert!(app.area_state.is_some());
        // Typed sub-options while the area session is live are consumed.
        app.run_command("add");
        assert_eq!(app.area_state.as_ref().unwrap().sign, 1.0);
        app.run_command("sub");
        assert_eq!(app.area_state.as_ref().unwrap().sign, -1.0);
        // Esc-cancel path: the reset helper clears both cleanly.
        app.area_state = None;
        app.layer_pick = LayerPickState::Off;
    }

    fn add_line(app: &mut CadApp, a: Vec2, b: Vec2) -> usize {
        app.add_dobject(Geom::Line(cad_kernel::Line { a, b }), "test");
        app.doc.dobjects.len() - 1
    }

    fn count_points(app: &CadApp, from: usize) -> usize {
        app.doc.dobjects[from..]
            .iter()
            .filter(|d| matches!(d.geom, Geom::Point(_)))
            .count()
    }

    #[test]
    fn divide_line_places_interior_marks_only() {
        let mut app = fresh_app();
        let base = app.doc.dobjects.len();
        let idx = add_line(&mut app, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        app.selection = vec![idx]; // pickfirst → straight to the value prompt
        app.run_command("divide");
        assert_eq!(app.ptdist_state, PtDistribState::DivideValue(idx));
        app.run_command("5");
        assert_eq!(app.ptdist_state, PtDistribState::Off, "command finished");
        assert_eq!(
            count_points(&app, base + 1),
            4,
            "5 segments → 4 interior marks"
        );
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("divide: 4 mark(s) placed")),
            "{:?}",
            app.history
        );
        let xs: Vec<f64> = app.doc.dobjects[base + 1..]
            .iter()
            .filter_map(|d| match d.geom {
                Geom::Point(p) => Some(p.location.x),
                _ => None,
            })
            .collect();
        assert_eq!(
            xs,
            vec![2.0, 4.0, 6.0, 8.0],
            "marks at every division point"
        );
        // One undo entry removes them all.
        app.do_undo();
        assert_eq!(app.doc.dobjects.len(), base + 1, "undo removes the marks");
    }

    #[test]
    fn measure_line_steps_by_segment_length() {
        let mut app = fresh_app();
        let base = app.doc.dobjects.len();
        let idx = add_line(&mut app, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        app.selection = vec![idx];
        app.run_command("measure");
        assert_eq!(app.ptdist_state, PtDistribState::MeasureValue(idx));
        app.run_command("3");
        assert_eq!(count_points(&app, base + 1), 3, "floor(10/3) = 3 marks");
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("measure: 3 mark(s) placed")),
            "{:?}",
            app.history
        );
        let xs: Vec<f64> = app.doc.dobjects[base + 1..]
            .iter()
            .filter_map(|d| match d.geom {
                Geom::Point(p) => Some(p.location.x),
                _ => None,
            })
            .collect();
        assert_eq!(xs, vec![3.0, 6.0, 9.0]);
    }

    #[test]
    fn divide_circle_marks_all_around() {
        let mut app = fresh_app();
        let base = app.doc.dobjects.len();
        app.add_dobject(
            Geom::Circle(cad_kernel::Circle {
                center: Vec2::new(0.0, 0.0),
                radius: 10.0,
            }),
            "test",
        );
        let idx = app.doc.dobjects.len() - 1;
        app.selection = vec![idx];
        app.run_command("divide");
        app.run_command("4");
        assert_eq!(count_points(&app, base + 1), 4, "closed loop → n marks");
        let pts: Vec<Vec2> = app.doc.dobjects[base + 1..]
            .iter()
            .filter_map(|d| match d.geom {
                Geom::Point(p) => Some(p.location),
                _ => None,
            })
            .collect();
        for (k, p) in pts.iter().enumerate() {
            let ang = k as f64 / 4.0 * std::f64::consts::TAU;
            let expect = Vec2::new(ang.cos(), ang.sin()) * 10.0;
            assert!(
                (*p - expect).len() < 1e-6,
                "mark {k} at {:?} ≈ {:?}",
                p,
                expect
            );
        }
    }

    #[test]
    fn divide_bad_values_fail_visibly() {
        let mut app = fresh_app();
        let idx = add_line(&mut app, Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
        app.selection = vec![idx];
        app.run_command("divide");
        app.run_command("2.5");
        assert!(
            app.history.iter().any(|h| h.contains("whole number")),
            "{:?}",
            app.history
        );
        assert_eq!(
            app.ptdist_state,
            PtDistribState::DivideValue(idx),
            "session survives a bad value"
        );
        app.run_command("measure");
        app.run_command("0");
        assert!(
            app.history.iter().any(|h| h.contains("must be positive")),
            "{:?}",
            app.history
        );
        assert_eq!(app.ptdist_state, PtDistribState::MeasureValue(idx));
        // A valid value after the failures still applies: 10/5 → 1 mark
        // at 5 (the endpoint at 10 is not marked).
        app.run_command("5");
        assert_eq!(count_points(&app, 0), 1, "{:?}", app.history);
    }

    #[test]
    fn divide_measure_distance_too_long_places_nothing() {
        let mut app = fresh_app();
        let base = app.doc.dobjects.len();
        let idx = add_line(&mut app, Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0));
        app.selection = vec![idx];
        app.run_command("measure");
        app.run_command("10");
        assert_eq!(app.doc.dobjects.len(), base + 1, "nothing placed");
        assert!(
            app.history.iter().any(|h| h.contains("no marks to place")),
            "{:?}",
            app.history
        );
    }

    #[test]
    fn selection_cycle_steps_through_stacked_dobjects() {
        let mut app = fresh_app();
        let base = app.doc.dobjects.len();
        let a = add_line(&mut app, Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0));
        let b = add_line(&mut app, Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0));
        add_line(&mut app, Vec2::new(50.0, 50.0), Vec2::new(52.0, 50.0));
        let w = Vec2::new(1.0, 0.0); // on top of both stacked lines
        let cands = app.pick_candidates_at(w, 1.0);
        assert_eq!(cands, vec![a, b], "stacked candidates, nearest first");
        let cell = (7_i32, 9_i32);
        // First Tab at a fresh spot selects the top candidate.
        app.cycle_pick_candidate(w, cell, 1.0);
        assert_eq!(app.selection, vec![a], "top candidate selected first");
        // Second Tab at the same spot advances to the stacked one beneath.
        app.cycle_pick_candidate(w, cell, 1.0);
        assert_eq!(app.selection, vec![b], "cycle advanced to #{}", b);
        assert_eq!(app.pick_cycle_at, Some((2, 1, w)));
        // The next click at the cycle spot honors the deeper candidate…
        assert_eq!(app.cycled_pick_for(w, 1.0), Some(b));
        // …but a click away from it (or after any other click) does not.
        assert_eq!(app.cycled_pick_for(Vec2::new(0.5, 1.0), 1.0), None);
        app.clear_pick_cycle();
        assert_eq!(app.cycled_pick_for(w, 1.0), None);
        assert_eq!(app.selection, vec![b], "clearing keeps the selection");
    }

    #[test]
    fn selection_cycle_ignores_invisible_and_lone_candidates() {
        let mut app = fresh_app();
        let base = app.doc.dobjects.len();
        let a = add_line(&mut app, Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0));
        let lid = add_layer(&mut app, "Hidden");
        let mut d = DObject::new(Geom::Line(cad_kernel::Line {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(2.0, 0.0),
        }));
        d.style.layer = lid;
        app.doc.push(d);
        app.doc.layers.get_mut(lid).unwrap().visible = false;
        let w = Vec2::new(1.0, 0.0);
        let cands = app.pick_candidates_at(w, 1.0);
        assert_eq!(cands, vec![a], "hidden layer dobject is not a candidate");
        // A single candidate: Tab picks it and arms no deeper cycle.
        app.cycle_pick_candidate(w, (3, 4), 1.0);
        assert_eq!(app.selection, vec![a]);
        assert_eq!(app.pick_cycle_at, None, "nothing stacked");
        let _ = base;
    }
}
#[cfg(test)]
mod calc_command_tests {
    use super::*;

    /// The idle-prompt calculator: bare expressions evaluate, echo `= value`,
    /// and store `ans`. Commands still win (a known command never reaches it).
    #[test]
    fn idle_expressions_evaluate_and_store_ans() {
        let mut app = CadApp::default();
        app.run_command("2+3*4");
        assert!(
            app.history.iter().any(|h| h == "  = 14"),
            "history: {:?}",
            app.history
        );
        assert!(app.calc.contains("ans"));
        app.run_command("ans/5");
        assert!(
            app.history.iter().any(|h| h == "  = 2.8"),
            "history: {:?}",
            app.history
        );
    }

    #[test]
    fn assignments_define_lazy_variables() {
        let mut app = CadApp::default();
        app.run_command("x=5");
        assert!(
            app.history.iter().any(|h| h == "  x = 5"),
            "history: {:?}",
            app.history
        );
        app.run_command("x*2");
        assert!(
            app.history.iter().any(|h| h == "  = 10"),
            "history: {:?}",
            app.history
        );
        // Lazy: re-evaluation after the definition changes.
        app.run_command("w=3");
        app.run_command("hh=w*2");
        app.run_command("hh");
        assert!(
            app.history.iter().any(|h| h == "  = 6"),
            "history: {:?}",
            app.history
        );
        app.run_command("w=7");
        app.run_command("hh");
        assert!(
            app.history.iter().any(|h| h == "  = 14"),
            "history: {:?}",
            app.history
        );
    }

    /// A lazy definition that references an undefined variable stores fine —
    /// the error surfaces at USE, not at definition.
    #[test]
    fn unknown_variables_error_at_use_not_definition() {
        let mut app = CadApp::default();
        app.run_command("hh=w*2");
        assert!(
            !app.history.last().unwrap().starts_with("  !"),
            "definition must succeed: {:?}",
            app.history.last()
        );
        app.run_command("hh+0");
        assert!(
            app.history
                .iter()
                .any(|h| h.starts_with("  ! calc: unknown variable 'w'")),
            "history: {:?}",
            app.history
        );
    }

    #[test]
    fn cycles_are_reported() {
        let mut app = CadApp::default();
        app.run_command("q1=q2");
        app.run_command("q2=q1");
        app.run_command("q1");
        assert!(
            app.history
                .iter()
                .any(|h| h.starts_with("  ! calc: variable cycle: q1 → q2 → q1")),
            "history: {:?}",
            app.history
        );
    }

    #[test]
    fn trig_is_in_degrees_and_functions_work() {
        let mut app = CadApp::default();
        app.run_command("sin(30)");
        assert!(
            app.history.iter().any(|h| h == "  = 0.5"),
            "history: {:?}",
            app.history
        );
        app.run_command("sqrt(16)+2^3");
        assert!(
            app.history.iter().any(|h| h == "  = 12"),
            "history: {:?}",
            app.history
        );
    }

    #[test]
    fn pasted_spaces_are_tolerated() {
        let mut app = CadApp::default();
        app.run_command("2 + 3");
        assert!(
            app.history.iter().any(|h| h == "  = 5"),
            "history: {:?}",
            app.history
        );
    }

    /// SYSVAR names keep today's meaning: a bare name is a SYSVAR query/set,
    /// and defining a VARIABLE with a SYSVAR name is rejected.
    #[test]
    fn sysvar_names_are_reserved() {
        let mut app = CadApp::default();
        app.run_command("CrsHrS=5");
        assert!(
            app.history
                .iter()
                .any(|h| h.starts_with("  ! calc: 'CrsHrS' is a system variable")),
            "history: {:?}",
            app.history
        );
        assert!(!app.calc.contains("CrsHrS"), "nothing stored");
        // A bare SYSVAR name still behaves as a SYSVAR (today's path).
        app.run_command("CrsHrS");
        assert!(
            app.history.iter().any(|h| h.contains("CrsHrS")),
            "history: {:?}",
            app.history
        );
    }

    /// Commands always win: `m` is Move even if a variable named `m` exists.
    #[test]
    fn commands_beat_variables_at_the_idle_prompt() {
        let mut app = CadApp::default();
        app.run_command("m=5");
        app.run_command("m");
        // `m` ran MOVE — no `= 5` echo for it, and a move flow started.
        assert!(
            app.history.iter().all(|h| h != "  = 5"),
            "history: {:?}",
            app.history
        );
    }

    /// A malformed expression reports `! calc: …` and keeps the unknown
    /// command error quiet.
    #[test]
    fn syntax_errors_are_calc_errors() {
        let mut app = CadApp::default();
        app.run_command("2+");
        assert!(
            app.history
                .iter()
                .any(|h| h.starts_with("  ! calc: syntax error")),
            "history: {:?}",
            app.history
        );
    }

    /// `calc` prints the syntax summary.
    #[test]
    fn calc_help_prints_the_summary() {
        let mut app = CadApp::default();
        app.run_command("calc");
        assert!(
            app.history
                .iter()
                .any(|h| h.contains("expression calculator")),
            "history: {:?}",
            app.history
        );
    }

    /// Assignment with a drawing open patches the sidecar immediately; the
    /// expressions come back verbatim (string storage — no float drift).
    #[test]
    fn variables_survive_the_sidecar_round_trip() {
        let mut app = CadApp::default();
        app.doc.units = cad_kernel::Units::default();
        let dir = std::env::temp_dir().join("calc_sidecar_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("calcvars.rsm");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(crate::simlux_io::sidecar_path(&path));
        app.current_file = Some(path.clone());
        app.run_command("w=2");
        // The sidecar write is a background patch — wait for it (and let the
        // NEXT assignment see the finished state, one patch at a time).
        wait_for_calc_sidecar(&mut app);
        // No spaces around `=` (Space=Enter submits; a spaced form like
        // "hh = w*2.5" parses as a Hatch command and commands win).
        app.run_command("hh=w*2.5");
        wait_for_calc_sidecar(&mut app);
        // The sidecar write happened at assignment time.
        let side = crate::simlux_io::sidecar_path(&path);
        assert!(
            side.exists(),
            "assignment with a drawing open writes the sidecar"
        );
        let cfg = crate::simlux_io::load(&path)
            .expect("load")
            .expect("sidecar present");
        assert_eq!(cfg.vars.get("hh").map(String::as_str), Some("w*2.5"));
        // Reopen: the store is rebuilt from the sidecar (a fresh app).
        let mut reopened = CadApp::default();
        reopened.calc = crate::calc::CalcStore::from_persist(cfg.vars);
        reopened.run_command("hh+0");
        assert!(
            reopened.history.iter().any(|h| h == "  = 5"),
            "history: {:?}",
            reopened.history
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&side);
    }

    /// Poll the calculator sidecar patch worker until it finishes.
    fn wait_for_calc_sidecar(app: &mut CadApp) {
        for _ in 0..400 {
            if app.calc_sidecar_rx.is_none() {
                return;
            }
            app.tick_calc_sidecar();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("calc sidecar patch did not finish");
    }
}
