# Session Summary — Mode workspaces, unified rooms (2026-09-08)

## 3. Command line is a 2D drafting surface (follow-up, uncommitted)

- The bottom command bar (command line + history) now renders ONLY in the 2D
  view; the SIMLUX and 3D Factory workspaces are pure viewports — no typing
  strip reserved (viewports run to the right edge and down to the status bar).
  `cmd_window_open` still records the user's choice (closing it in 2D keeps it
  closed across a 3D visit) and the bar's stored height freezes while hidden,
  so it returns exactly as left.
- `command_bar_height_survives_workspace_round_trips` updated to the new
  behavior: bar drawn in Cad2D frames only, absent in Factory/SIMLUX frames,
  viewports fill `W - mode_panel`, height frozen + restored. Suite: 1517
  passed; the 9 failures (mesh_io FBX assets) are the pre-existing set.

## 4. Make-room asks for the details first (follow-up, uncommitted)

- 2D ROOMS ▸ "Make room (from selected outline)" no longer builds instantly:
  it validates the selection, then opens a modal details form — name, START
  height (the z the room stands on), CLEAR height, wall/floor/ceiling slab
  thicknesses (the factory settings as prefilled defaults) — and builds the
  room with the ANSWERS as ONE undo step. Cancel/Esc abandons the outline;
  while the modal is open it owns Esc and idle Enter/Space, so nothing under
  it also cancels a draft or repeats the last command.
- Factory core: `RoomBuildSpec` + `FactoryState::add_room_spec` construct a
  room from an explicit parameter set (settings only seed the defaults);
  `add_room` delegates with the settings' spec, and `build_designated_room`
  now honours the designated record's FULL spec — start height and slab/wall
  thicknesses, not just name + height.
- Key methods: `CadApp::request_make_room`, `render_room_form`,
  `build_room_form`; tests `make_room_asks_for_details_first`,
  `the_form_answers_become_the_room` (geometry lifts verified: slab at
  base_z, walls at base+floor, ceiling at base+floor+clear) and
  `cancelling_the_room_form_builds_nothing`.
- Suite: 1520 passed; the 9 failures (mesh_io FBX assets) are the
  pre-existing set, unchanged.

## 5. Room-form follow-up + demo plan at real sizes (follow-up, uncommitted)

- The room-details form gives WALL thickness its own labelled row (it shared
  a cramped row with the floor before); floor and ceiling slabs each get a
  full row too.
- The demo figures a fresh app ships with are now drawn at REAL room sizes in
  the default millimetre document: the closed pentagon became a closed
  6000 × 4500 mm (6 × 4.5 m) rectangle outline you can Make-room immediately;
  the line/circle/arc/ellipse/point sit around it in the same few-metre
  ranges (they were ±(20–90)-unit centimetre sketches). The view is framed
  once on the first real frame (`demo_view_set` + `maybe_frame_demo_plan`) —
  never overriding a user zoom or a file open — since the launch camera
  would otherwise show empty space.
- `hatch_center_line_survives_vertex_hits` no longer leans on the demo
  circle: it builds its own r=30 origin circle.
- Tests: `the_demo_plan_makes_a_real_metre_room`,
  `the_demo_plan_is_framed_once_on_first_launch`. Suite: 1522 passed; the 9
  failures (mesh_io FBX assets) are the pre-existing set, unchanged.


All work below lives on branch `rooms-unified` (uncommitted fixes folded in via
this summary's commit); earlier merged work is on `farzad-dev` (merge
`a2bfda9`, pushed). `.obsidian/*` + `TODO.md` local edits are NOT committed.

## 1. Exclusive mode tab bar (merged into `farzad-dev` as `a2bfda9`)

- Top tab bar switches ONE full-window workspace at a time:
  2D view | SIMLUX view | 3D Factory view (`Mode` enum = source of truth;
  `switch_mode_inner` rearranges `two_d_open` / `light.view3d_open` /
  `factory.open`; frame-start enforcement in `enforce_mode_workspaces`).
- Face-sketches take over the window (they draft on the 2D canvas) and return
  to the Factory workspace on finish unless the user switched tabs mid-sketch.
- The old DRAW/MODIFY icon rails are REMOVED; every workspace has a left
  command panel with collapsible (banded-header) categories.
- Command-bar ratchet bug fixed: the bottom bar lays out BEFORE the full-window
  viewports, and the SIMLUX max-width pins use the available rect, so the bar's
  stored height can no longer balloon and cover the canvas after a tab
  round-trip (regression test `command_bar_height_survives_workspace_round_trips`).
- "Place luminaire" works from the bare 2D workspace (armed tools make the
  SIMLUX 2D layer live), names the fitting it drops, and the Light panel is
  reachable from the 2D panel for editing placed lights.

## 2. ONE room list (branch `rooms-unified`, current work)

`factory.rooms` is now the only room definition list. `RoomInst` carries
`origin: RoomOrigin { Built ⌂ | PlanDesignated ◫ | ImportedLayer ⬚ }`,
`layer_name` and `handles`; `RoomRec` persists them (serde defaults → old files
load). `light.plan_rooms` and the legacy `light.room`/`RoomLayer` structures
are gone.

- Calc targets read the one list (mixed origins; each footprint ≥3 corners is
  a target with its own grid); no more factory-else-plan precedence. Imported-
  layer rooms extrude their layer geometry into the scene at the room height.
- Legacy configs (`plan_rooms`, `layers_3d`) migrate into the unified list in
  `migrate_legacy_rooms` (called from `install_simlux_config`); saves write the
  new format only (legacy fields left as read-compat empties).
- 2D panel ROOMS list, Light-panel ① Rooms and the Factory ▼ Rooms menu all
  show the same list with source badges; height editing and removal work for
  any origin; unbuilt rows are gated out of slab/fit controls.
- **Designate = build**: the 2D row is now "Make room (from selected outline)"
  and creates the REAL 3D room at once (`make_room_from_selected_outline` →
  `add_designated_room` + `build_designated_room`, one undo step). Unbuilt
  rows (legacy/migrated only) keep a ⬆ build action.

Key methods: `FactoryState::add_designated_room`, `build_designated_room`,
`import_layer_as_room`, `remove_imported_layer_room`, `room_index_of_layer`,
`RoomInst::is_built`, `RoomOrigin::{glyph,label}`; `CadApp::make_room_from_
selected_outline`, `migrate_legacy_rooms`, `room_error_why`.

Tests added: `rooms_are_one_list_in_factory_state` (4), `rooms_unification_
migration`, `plan_rooms_live_in_the_one_list_and_can_be_built`,
`making_a_room_from_a_plan_outline_builds_it_in_3d`. Suite: 1516 passed; the
10 failures (mesh_io FBX assets + one env-settings test) are pre-existing on
this machine, unchanged.

## Open items for next session

- ~~`making_a_room...` test passes with a direct outline; `slab_outline_from_selection` on a bare closed Polyline failed inside the test~~ — RESOLVED (2026-09-08 follow-up, test-only): a default `CadApp` document is NOT empty (it ships seed dobjects), so `selection.push(0)` named a seed line, not the pushed ring; the production path was fine. The test now clears `doc.dobjects` (as `promote_tests::app_with` does) and feeds the outline through `slab_outline_from_selection` — the exact "Make room" row path (`mode_workspaces_are_exclusive` 13 tests + `promote_tests` 10 tests pass).
- Unbuilt room rows / ImportedLayer rooms without a closed ring are scene-only
  (no calc target) by design.
