# Session Summary — Mode workspaces, unified rooms (2026-09-08)

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

- `making_a_room...` test passes with a direct outline; `slab_outline_from_
  selection` on a bare closed Polyline failed inside the test (works in the
  app) — revisit with the debug print if it matters.
- Unbuilt room rows / ImportedLayer rooms without a closed ring are scene-only
  (no calc target) by design.
