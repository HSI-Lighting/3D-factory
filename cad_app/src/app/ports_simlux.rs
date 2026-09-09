use super::*;

// ============ SIMLUX lighting integration (grafted from simLUX main) ============
// Child module of `app` (same pattern as pedit/io/ports_*): CadApp methods
// that drive the `cad_light` engine, the SIMLUX viewport and its panels.
// Methods called from elsewhere in `app` are `pub(super)`/`pub(crate)`;
// everything else stays module-private.

impl CadApp {
    // ===== SIMLUX lighting integration (grafted from simLUX main) ========
    /// Drives the `cad_light` engine on the shared document; `Calculate` needs
    /// `&self.doc`, so the panel returns an action instead of touching it.
    pub(super) fn render_light_panel(&mut self, ctx: &egui::Context) {
        if !self.light.window_open {
            return;
        }
        // THE PLAN, NOT WHATEVER IS ON THE CANVAS — at all three sites below, and they have to
        // agree with each other. SIMLUX lights the BUILDING; a face sketch is a drawing on one
        // plane of it and is never the thing being lit.
        //
        // The picker and the import are fixed TOGETHER deliberately. Layer ids are positions in
        // the table, so fixing only the import would list one document's layer names while
        // importing the other document's layer at that number — worse than either alone.
        //
        // `calculate` is the one that reads oddly on its own: `scene_meshes` early-returns the
        // Factory model's geometry whenever the model holds anything, so on a project with a
        // building the document argument never reaches the lux figures and the validated numbers
        // cannot move. It still matters for a 2D-only project, where `bbox(doc)` is what places
        // the calculation plane — a sketch's (u, v) there put the plane somewhere unrelated to
        // the drawing.
        let plan = Self::plan_doc_of(self.factory.session.as_ref(), &self.doc);
        // Snapshot (layer id, name) OUTSIDE the window closure so panel_ui never
        // borrows self.doc while self.light is borrowed mutably (Phase B picker).
        let layers: Vec<(u32, String)> = plan
            .layers
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| (i as u32, l.name.clone()))
            .collect();
        let mut open = self.light.window_open;
        let mut action = crate::light::LightAction::default();
        let rooms = self.factory.rooms.clone();
        egui::Window::new("SIMLUX — Light")
            .open(&mut open)
            .default_width(288.0)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    action = self.light.panel_ui(ui, &layers, &rooms);
                });
            });
        self.light.window_open = open;
        // The panel edits fixtures inside its own closure, where `self.doc` is out of reach — so
        // it leaves the copy it overwrote staged and this turns it into a step.
        self.commit_light_undo();
        // …and the two that delete go through the shared path, so a fixture placed from the
        // library takes its symbol with it. Deleting here used to leave the block on the plan.
        if let Some(id) = action.remove_fixture {
            self.delete_fixtures(&[id]);
        }
        if action.clear_fixtures {
            let all: Vec<u32> = self.light.luminaires.iter().map(|l| l.id).collect();
            let (lights, blocks) = self.delete_fixtures(&all);
            self.light.last_msg = format!("Cleared {lights} fixture(s) and {blocks} symbol(s).");
        }
        if action.shift_to_simlux {
            self.shift_selection_to_simlux_layer();
        }
        if let Some(id) = action.import_layer {
            // A live face-sketch has its own layers; the plan is the room's home.
            let plan = Self::plan_doc_of(self.factory.session.as_ref(), &self.doc).clone();
            self.factory.import_layer_as_room(&plan, id);
        }
        if let Some((id, h)) = action.set_room_height {
            self.factory.set_room_height(id, h);
        }
        if let Some(id) = action.remove_room {
            let name = self
                .factory
                .rooms
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.name.clone())
                .unwrap_or_default();
            self.factory.delete_room(id);
            self.history.push(format!("  room '{name}' removed"));
        }
        if action.import_photometry {
            self.open_file_dialog(FileDialogMode::ImportIes, ".ies");
        }
        if action.calculate {
            self.start_calculation();
        }
    }

    /// WHICH WINDOW the command line is talking to.
    ///
    /// The 3D Factory has to be on screen for the answer to be "3D": `active_view` remembers the
    /// last viewport interacted with and does not forget when the panel closes, and a command line
    /// aimed at a viewport that is not there would swallow every 2D command typed after it.
    pub(super) fn command_target(&self) -> ActiveView {
        if self.factory.open && self.active_view == ActiveView::ThreeD {
            ActiveView::ThreeD
        } else {
            ActiveView::TwoD
        }
    }

    /// Words that belong to the 3D Factory alone. Used to tell someone which window owns what they
    /// typed, rather than answering "unknown command".
    pub(super) fn is_factory_only_command(raw: &str) -> bool {
        let w = raw.trim().to_ascii_lowercase();
        let head = w.split_whitespace().next().unwrap_or("");
        raw.trim_start().starts_with('@')
            || matches!(head, "place" | "centre" | "center" | "click" | "offset")
    }

    /// Parse a typed coordinate — `@900,0,0`, `@(900, 0, 0)`, `@900 0 0` — into METRES.
    ///
    /// `@` is the whole placement UI now. Asked for as: "instead of having a slider to cho[o]se the
    /// co ordinates i want the user to type in the co ordinate. once they cho[o]se they want to
    /// have the origin offset they will type @(their coordinates). this will be the system."
    ///
    /// The parentheses are optional because both forms were described and neither is wrong; the
    /// separator may be a comma or a space for the same reason. Numbers are in the FACTORY's
    /// working unit — reading a typed length as metres is the class of bug that built a 4.4 km
    /// building. Each number may be an expression (`@x*2,0,0`).
    pub(super) fn parse_at_coords(
        raw: &str,
        u: &cad_kernel::Units,
        store: &crate::calc::CalcStore,
    ) -> Option<[f32; 3]> {
        let s = raw.trim();
        let s = s.strip_prefix('@')?.trim();
        // `(x,y,z)` and `x,y,z` are the same thing said two ways.
        let s = s
            .strip_prefix('(')
            .map(|t| t.trim_end().strip_suffix(')').unwrap_or(t))
            .unwrap_or(s);
        let parts: Vec<f64> = s
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|t| !t.is_empty())
            .map(|t| {
                if let Ok(v) = t.parse::<f64>() {
                    return Ok(v);
                }
                crate::calc::eval(store, t)
            })
            .collect::<Result<_, _>>()
            .ok()?;
        if parts.len() < 2 || parts.len() > 3 {
            return None;
        }
        let k = u.metres_per_unit;
        Some([
            (parts[0] * k) as f32,
            (parts[1] * k) as f32,
            (parts.get(2).copied().unwrap_or(0.0) * k) as f32,
        ])
    }

    /// The 3D command line's PLACEMENT vocabulary. Returns true when the line was one of these.
    ///
    /// There is no menu for this any more — the toolbar dropdown was replaced by an AutoCAD-style
    /// bracketed prompt in the command box, which is where the rest of this app already asks
    /// questions ("Specify center point for circle or [3P/2P/Ttr]:"):
    ///
    ///   `place`            — Placement [Click/Centre/Origin/Offset] <current>:
    ///   `@X,Y,Z`           — set the offset distance, in the working unit
    ///
    /// The mode words are still accepted on their own line, because a prompt you have to walk
    /// through every time is slower than the thing it replaced.
    pub(super) fn factory_place_command(&mut self, raw: &str) -> bool {
        use crate::factory::PlaceMode;
        let lc = raw.trim().to_ascii_lowercase();

        // `@X,Y,Z` — the coordinate. It sets the OFFSET and nothing else: an object already waiting
        // for its click keeps waiting, which is what was asked for. Typing a distance from the
        // origin and placing the thing in front of you are two different intentions.
        if lc.starts_with('@') {
            let Some(o) = Self::parse_at_coords(&lc, &self.factory.units, &self.calc) else {
                self.factory.status = "@X,Y,Z — two or three numbers, e.g. @900,0,0".into();
                self.history.push("  @: expected @X,Y,Z".into());
                return true;
            };
            self.factory.place_offset = o;
            // ONE placement, like the prompt — a coordinate is a point, not a mode. The sticky
            // version is still there and still explicit: ▼ placement → "Offset from origin",
            // which stays on and says so in the menu.
            if self.factory.place_mode != PlaceMode::Offset {
                self.factory.place_mode_before_offset = self.factory.place_mode;
            }
            self.factory.place_offset_once = true;
            self.factory.place_mode = PlaceMode::Offset;
            self.factory.awaiting_place = None;
            let u = self.factory.units.clone();
            let msg = format!(
                "The next object lands at {}, {}, {} from the origin",
                crate::factory::length_str(&u, o[0]),
                crate::factory::length_str(&u, o[1]),
                crate::factory::length_str(&u, o[2]),
            );
            self.history.push(format!("  {msg}"));
            self.factory.status = msg;
            self.clear_prompt();
            return true;
        }

        // An answer to the prompt below — or the same word typed cold, which does the same thing.
        let answered = match lc.as_str() {
            "click" => Some(PlaceMode::Click),
            "centre" | "center" => Some(PlaceMode::Centre),
            "origin" => Some(PlaceMode::Origin),
            "offset" => Some(PlaceMode::Offset),
            // Bare Enter at the prompt keeps what is already set — the AutoCAD <default>.
            "" if self.place_prompt_open => Some(self.factory.place_mode),
            _ => None,
        };
        if let Some(m) = answered {
            self.place_prompt_open = false;
            self.factory.place_mode = m;
            // Switching AWAY from Click while something waits would strand it: armed forever, with
            // no click coming for it.
            if m != PlaceMode::Click {
                self.factory.awaiting_place = None;
            }
            // OFFSET ASKS FOR THE COORDINATE, THERE AND THEN.
            //
            // Reported as: "once a co ordinate is chosen it keeps on placing on the same co
            // ordinate. why is that[?] the user need to be able to select a new coordinate."
            //
            // It did keep the old one — correctly, by its own rules — because choosing `offset`
            // only set the MODE and left a note saying to type `@X,Y,Z` as a separate line. The
            // session shows exactly that: `place` → `offset`, twice, with no `@` after either, and
            // every object landing on the coordinate from twenty minutes earlier. Choosing the mode
            // and choosing the point are one intention, so they are now one exchange.
            if m == PlaceMode::Offset {
                self.set_place_coord_prompt();
                return true;
            }
            // CLICK MODE RE-PLACES WHAT IS ALREADY SELECTED.
            //
            // Reported as "when a furniture is placed inside a building the placement mechanism
            // isn't working", and the session shows exactly why: a cupboard was built at 2.5 s
            // (landing on the origin), `place` was typed at 5.3 s, and the click at 7.1 s went
            // through the SELECTION path — `3D pick → feature#3` — because nothing was waiting.
            //
            // The mode only ever governed the NEXT object added, so choosing "click to place"
            // after building something did nothing you could see. Arming the selection is what
            // the user is asking for by that sequence, and it is what `place` used to do before
            // it became a mode question.
            if m == PlaceMode::Click && self.factory_arm_selection_placement() {
                let msg = "Click the 2D or 3D window to place the selection  [Esc leaves it]";
                self.history.push(format!("  {msg}"));
                self.factory.status = msg.into();
                self.clear_prompt();
                return true;
            }
            let msg = m.hint().to_string();
            self.history.push(format!("  {msg}"));
            self.factory.status = msg;
            self.clear_prompt();
            return true;
        }
        // The answer to that coordinate question. `@` is optional here — it is already unambiguous
        // once the app has asked for a coordinate, and insisting on punctuation at a prompt that
        // says "X,Y,Z" is a way of rejecting the right answer.
        if self.place_coord_prompt && !lc.is_empty() {
            let with_at = if lc.starts_with('@') {
                lc.clone()
            } else {
                format!("@{lc}")
            };
            let Some(o) = Self::parse_at_coords(&with_at, &self.factory.units, &self.calc) else {
                self.history.push(format!("  '{lc}' is not a coordinate"));
                self.set_place_coord_prompt();
                return true;
            };
            self.factory.place_offset = o;
            // ONE placement, not a mode — see `FactoryState::arm_placement`. Remember what the
            // mode was so the next add goes back to it instead of stacking on this coordinate.
            if self.factory.place_mode != PlaceMode::Offset {
                self.factory.place_mode_before_offset = self.factory.place_mode;
            }
            self.factory.place_offset_once = true;
            self.factory.place_mode = PlaceMode::Offset;
            self.factory.awaiting_place = None;
            self.place_coord_prompt = false;
            self.clear_prompt();
            let msg = self.place_offset_summary();
            self.history.push(format!("  {msg}"));
            self.factory.status = msg;
            return true;
        }
        // An unrecognised reply to an open prompt is a typo, not a new command — say what was
        // expected and keep asking, exactly as the 2D prompts do.
        if self.place_prompt_open && !lc.is_empty() {
            self.history
                .push(format!("  '{lc}' is not one of the options"));
            self.set_place_prompt();
            return true;
        }

        if lc == "place" || lc == "pl" {
            self.set_place_prompt();
            return true;
        }
        false
    }

    /// Hand the current 3D selection to a placing click. False when nothing is selected.
    ///
    /// Furniture first: it is the thing most often being repositioned, and when a furniture piece
    /// and the solid behind it are both in the selection, the piece is what the click was aimed at.
    fn factory_arm_selection_placement(&mut self) -> bool {
        use crate::factory::AwaitingPlace;
        if let Some(i) = self.factory.sel_furn_primary() {
            self.factory.awaiting_place = Some(AwaitingPlace::Furniture(i));
            return true;
        }
        if let Some(&id) = self.factory.selection.first() {
            self.factory.awaiting_place = Some(AwaitingPlace::Feature(id));
            return true;
        }
        false
    }

    /// "The next object lands at 900 mm, 0 mm, 0 mm from the origin" — in the working unit.
    ///
    /// NEXT, singular, when the coordinate was typed: it is one placement and the line has to say
    /// so, or the mode is invisible and the next five objects stack on the same point. The sticky
    /// version comes from the placement menu and says "New objects".
    pub(super) fn place_offset_summary(&self) -> String {
        let u = self.factory.units.clone();
        let o = self.factory.place_offset;
        let who = if self.factory.place_offset_once {
            "The next object lands"
        } else {
            "New objects land"
        };
        format!(
            "{who} at {}, {}, {} from the origin",
            crate::factory::length_str(&u, o[0]),
            crate::factory::length_str(&u, o[1]),
            crate::factory::length_str(&u, o[2]),
        )
    }

    /// Ask for the offset coordinate, with the current one as the `<default>` bare Enter keeps.
    pub(super) fn set_place_coord_prompt(&mut self) {
        self.place_prompt_open = false;
        self.place_coord_prompt = true;
        let u = self.factory.units.clone();
        let o = self.factory.place_offset;
        self.current_prompt = format!(
            "Distance from origin  X,Y,Z <{}, {}, {}>:",
            crate::factory::length_str(&u, o[0]),
            crate::factory::length_str(&u, o[1]),
            crate::factory::length_str(&u, o[2]),
        );
    }

    /// Ask the placement question, AutoCAD-style: the options in brackets, the current setting as
    /// the `<default>` that bare Enter accepts.
    fn set_place_prompt(&mut self) {
        self.place_prompt_open = true;
        self.current_prompt = format!(
            "Placement [Click/Centre/Origin/Offset] <{}>:",
            self.factory.place_mode.keyword(),
        );
    }

    /// Paint the computed lux grid as a 2D false-colour overlay on the plan,
    /// mapping each work-plane cell through the same `w2s` view transform as the
    /// geometry so the heatmap tracks pan/zoom exactly. Clipped to the canvas.
    pub(super) fn paint_lux_overlay(&self, painter: &egui::Painter, rect: egui::Rect) {
        // WHAT THIS COSTS, RECORDED — see `CadApp::lux_overlay_us`. It runs only when a lighting
        // result exists, and it lives in the 2D canvas, so no 3D counter could ever see it. It is
        // where the SIMLUX lag actually was, and it went four rounds unmeasured.
        let t0 = std::time::Instant::now();
        self.lux_overlay_cells.set(0);
        self.paint_lux_overlay_inner(painter, rect);
        self.lux_overlay_us.set(t0.elapsed().as_micros() as u64);
    }

    /// WHAT IS BEING CALCULATED, WHILE IT IS BEING CALCULATED — and gone the moment it lands.
    ///
    /// Asked for as: *"the zone thats calculating should be highlighted. nothing fancy just a low
    /// transparency green highlight once calculated it should be gone as well."* A Thorough run on
    /// this building takes twelve seconds, and until now the plan gave no sign which ground the
    /// answer was going to cover — so a wrong region only became visible twelve seconds later, in
    /// the result itself.
    ///
    /// Gated on `calc_rx`, which is `Some` for exactly the life of the worker: set when the job is
    /// spawned, cleared in `poll_calculation` on every exit, success or panic. There is no separate
    /// flag that could be left switched on.
    pub(super) fn paint_calculating_zone(&self, painter: &egui::Painter, rect: egui::Rect) {
        if self.calc_rx.is_none() || self.factory.session.is_some() {
            return;
        }
        // Low-transparency green — a wash, not a highlight that hides the plan under it.
        let fill = egui::Color32::from_rgba_unmultiplied(80, 200, 120, 40);
        let clip = painter.with_clip_rect(rect);
        let rooms: Vec<&Vec<glam::Vec2>> = self
            .factory
            .rooms
            .iter()
            .map(|r| &r.footprint)
            .filter(|f| f.len() >= 3)
            .collect();
        if !rooms.is_empty() {
            // THE ACTUAL TARGETS, drawn as the rooms they are — so a project where one room is
            // being calculated does not look like the whole plan is.
            for poly in rooms {
                let pts: Vec<egui::Pos2> = poly
                    .iter()
                    .map(|p| self.w2s_m(Vec2::new(p.x as f64, p.y as f64), rect))
                    .collect();
                clip.add(egui::Shape::convex_polygon(pts, fill, egui::Stroke::NONE));
            }
            return;
        }
        // NO ROOMS DRAWN, so the target is the whole building, and its extent is the honest thing
        // to show. The cell-by-cell boundary is decided on the worker by the floor test (see
        // `measurable_mask`) and is not available here.
        if let Some((mn, mx)) = self.factory.features_aabb() {
            let a = self.w2s_m(Vec2::new(mn.x as f64, mn.y as f64), rect);
            let b = self.w2s_m(Vec2::new(mx.x as f64, mx.y as f64), rect);
            clip.rect_filled(egui::Rect::from_two_pos(a, b), 0.0, fill);
        }
    }

    fn paint_lux_overlay_inner(&self, painter: &egui::Painter, rect: egui::Rect) {
        if !self.light.show_overlay {
            return;
        }
        // NOT IN A SKETCH. The grid is a result measured on the WORK PLANE, in world coordinates;
        // a face sketch puts the canvas on some other plane entirely — a wall, a tilted roof — and
        // painting a horizontal result across it draws a false-colour field over a surface it was
        // never measured on. It reads as data and is not. The overlay belongs to the global plan
        // view, which is where the plane it describes actually is.
        if self.factory.session.is_some() {
            return;
        }
        if self.light.rooms.is_empty() {
            return;
        }
        // THE REPORT'S SCALE AND BAND COLOURS — the same rule the 3D sheet and the page use.
        //
        // Asked for as: *"change the 3d and 2d false colors to reports bands."* This read the
        // panel's own palette and its own ceiling, so the plan, the 3D view and the report were
        // three pictures of one field in three different schemes.
        let room_max = self.light_room_max();
        let ramp = self.light.ramp.rgb_fn();
        let opts = &self.report_opts;
        let clip = painter.with_clip_rect(rect);
        // EVERY ROOM, not just the one in the panel. A plan with two rooms used to show one lit
        // and one dark, which reads as a room that failed rather than a room nobody calculated.
        //
        // ONE MESH PER ROOM, NOT ONE SHAPE PER CELL.
        //
        // This called `rect_filled` for every calculated cell — up to `MAX_GRID_POINTS`, 16,384
        // separate `Shape::Rect`s per room, EVERY FRAME. egui tessellates and anti-aliases each
        // shape independently, so that is 16,384 trips through the tessellator and roughly 130,000
        // vertices of paint list built and thrown away per frame, on the UI thread.
        //
        // It is also why the lag was "only when SIMLUX is open": this runs only when there is a
        // lighting result to draw, so nothing about it shows up in a modelling session — which is
        // exactly what the earlier session dumps were, and part of why three rounds of work went
        // into the 3D view instead.
        //
        // The same fix as the report preview's rings, for the same reason: a `Mesh` shares its
        // vertices and has no interior edges to feather, so the cells also stop showing the faint
        // seams that per-shape AA left between them.
        for room in &self.light.rooms {
            let (grid, plane) = (&room.grid, &room.plane);
            if grid.values.is_empty() {
                continue;
            }
            // THE SAME INTERPOLATED FIELD THE REPORT DRAWS, at the resolution the 3D view already
            // uses. This painted one flat quad per GRID CELL — reported as *"in the simlux and 2d
            // view the false color look like low poly cubes"* — while the report samples BETWEEN
            // cells and bands the result, which is why its bands curve where these staircased at
            // the grid pitch. Two pictures of one calculation that did not look like each other.
            let (nx, ny) = Self::overlay_res(grid.cols as usize, grid.rows as usize);
            let f = crate::isolux::sample(grid, &room.mask, nx, ny);
            let dx = plane.width / nx.max(1) as f32;
            let dy = plane.depth / ny.max(1) as f32;
            let mut mesh = egui::epaint::Mesh::default();
            mesh.reserve_triangles(nx * ny * 2);
            mesh.reserve_vertices(nx * ny * 4);
            for row in 0..ny as u32 {
                for col in 0..nx as u32 {
                    let i = row as usize * nx + col as usize;
                    // Cells outside the room are not the room's result and are not painted. A grid
                    // is a rectangle and a room need not be; those cells were computed, but
                    // colouring them reports illuminance on ground the room does not occupy.
                    if !f.inside.get(i).copied().unwrap_or(true) {
                        continue;
                    }
                    // The sub-cell's CENTRE — the mean of its four corners — so a band edge falls
                    // between samples rather than on one.
                    let (ci, cj) = (col as usize, row as usize);
                    let v = 0.25
                        * (f.at(ci, cj)
                            + f.at(ci + 1, cj)
                            + f.at(ci, cj + 1)
                            + f.at(ci + 1, cj + 1));
                    let c = opts.lux_rgb(v, room_max, ramp);
                    let color = egui::Color32::from_rgb(c[0], c[1], c[2]);
                    let x0 = plane.origin.x + col as f32 * dx;
                    let y0 = plane.origin.y + row as f32 * dy;
                    // The calc plane is METRES (cad_light's contract; its bounds come from
                    // `extrude::bbox`, which scales by the document's unit).
                    let p0 = self.w2s_m(Vec2::new(x0 as f64, y0 as f64), rect);
                    let p1 = self.w2s_m(Vec2::new((x0 + dx) as f64, (y0 + dy) as f64), rect);
                    let r = egui::Rect::from_two_pos(p0, p1);
                    let base = mesh.vertices.len() as u32;
                    mesh.colored_vertex(r.left_top(), color);
                    mesh.colored_vertex(r.right_top(), color);
                    mesh.colored_vertex(r.right_bottom(), color);
                    mesh.colored_vertex(r.left_bottom(), color);
                    mesh.add_triangle(base, base + 1, base + 2);
                    mesh.add_triangle(base, base + 2, base + 3);
                }
            }
            if !mesh.is_empty() {
                // Quads painted, for the perf tap — the number that scaled with the grid.
                self.lux_overlay_cells
                    .set(self.lux_overlay_cells.get() + (mesh.indices.len() / 6) as u32);
                clip.add(egui::Shape::Mesh(mesh));
            }
        }
    }

    /// Pick tolerance for a luminaire marker, converted from screen pixels to WORLD METRES.
    ///
    /// The marker is drawn at a fixed pixel size, so its grab radius has to be a fixed pixel size
    /// too — a world-space tolerance would be impossible to hit zoomed out and would swallow half
    /// the room zoomed in. Two conversions, because the plan is in drawing units and the lighting
    /// layout is in metres: px → drawing units (`self.scale`) → metres (the document unit).
    fn lum_pick_tol_m(&self) -> f32 {
        let px_per_m = self.doc.units.from_metres(1.0) as f32 * self.scale;
        crate::light::PICK_PX / px_per_m.max(1e-6)
    }

    /// SIMLUX luminaire editing on the 2D plan — place, select, drag, delete.
    ///
    /// This is the whole point of the two-step workflow: a light POINT is a mark on the plan that
    /// stays editable. Before this, "place" was a checkbox that nothing read — no click handler
    /// existed anywhere — so a fixture could only ever arrive via the grid array and, once there,
    /// could not be moved at all.
    ///
    /// Returns TRUE when the gesture belongs to the lighting layout, so the drafting canvas can
    /// skip its own click/drag handling for that frame. It deliberately does NOT feed
    /// `canvas_locked`: panning and zooming must keep working while points are being placed, and
    /// that gate stops them.
    /// Let a click on the 2D PLAN say where a newly added 3D object goes.
    ///
    /// Asked for as: "a place the user can click on the 3d or 2d window". The 3D viewport has had a
    /// placement click since Draw3D; the plan never has, so anything added while the 2D canvas was
    /// in front landed wherever the code chose and stayed there.
    ///
    /// Returns TRUE when the click was the placing one, so the drafting canvas skips its own
    /// click handling for that frame — otherwise saying where a sofa goes also starts a line.
    /// Like [`Self::simlux_pointer_2d`] it does not feed `canvas_locked`: panning and zooming to
    /// find the right spot must keep working while an object waits.
    pub(super) fn factory_placing_pointer_2d(&mut self, resp: &egui::Response, rect: egui::Rect) -> bool {
        if self.factory.awaiting_place.is_none() {
            return false;
        }
        if !resp.clicked() {
            // Still armed, so the click gates stay suppressed for the whole gesture — a press that
            // has not yet become a click must not fall through and start drawing.
            return resp.is_pointer_button_down_on();
        }
        let Some(pos) = resp.interact_pointer_pos() else {
            return false;
        };
        // The plan is in DOCUMENT units and the model is in metres; `s2w_m` is the one that
        // converts. Z comes from the storey the object was built on, not from the plan.
        let w = self.s2w_m(pos, rect);
        self.factory_finish_awaited_placement(glam::Vec3::new(w.x as f32, w.y as f32, 0.0));
        true
    }

    /// Land the waiting object at `at` and say so — the shared tail of the 2D and 3D placing
    /// clicks, so both windows behave identically.
    fn factory_finish_awaited_placement(&mut self, at: glam::Vec3) {
        // Snapshot BEFORE the move, and only now that a real point exists: an armed object that is
        // never clicked must not leave an empty undo step behind.
        self.snapshot_factory();
        let what = self.factory.place_awaiting_at(at);
        if what.is_none() {
            return; // the object went away underneath us (undo, delete) — nothing to place
        }
        let name = match what {
            Some(crate::factory::AwaitingPlace::Furniture(i)) => self
                .factory
                .furniture
                .get(i)
                .and_then(|f| self.factory.furniture_lib.get(f.asset))
                .map(|a| a.name.clone())
                .unwrap_or_else(|| "object".into()),
            _ => "solid".into(),
        };
        self.factory.recompute();
        let u = self.factory.units.clone();
        self.factory.status = format!(
            "{name} placed at {}, {}",
            crate::factory::length_str(&u, at.x),
            crate::factory::length_str(&u, at.y),
        );
        self.factory_note(format!("place ✓ {name} ({:.2},{:.2})", at.x, at.y));
    }

    /// Is the SIMLUX layer — the fixture markers on the 2D canvas and the pointer that places
    /// and drags them — live right now?
    ///
    /// TWO conditions, and the second is the one that was missing. SIMLUX has to be on screen,
    /// AND the canvas has to be showing the PLAN. A luminaire's position is a WORLD (x, y) in
    /// metres; during a face sketch the canvas shows that plane's (u, v), so the two are simply
    /// different spaces. Drawn there, a fitting at world (4, 2) painted itself 4 m along and 2 m
    /// up an elevation and read as a light mounted on that wall; clicked there, `place_point`
    /// wrote a point measured on the wall straight into a luminaire's world x/y, and a drag
    /// applied a delta measured on the sketch plane to a world position.
    ///
    /// `paint_lux_overlay` already refused for exactly this reason, with a note about the work
    /// plane. Its two neighbours did not — so the heatmap correctly vanished while the fixtures
    /// it was computed from kept drawing. One predicate now, so they cannot drift again.
    pub(super) fn simlux_2d_layer_live(&self) -> bool {
        if self.factory.session.is_some() {
            return false;
        }
        // An ARMED placement/aim tool counts as SIMLUX being on screen: the
        // 2D workspace keeps no SIMLUX panel open, and switching "Place
        // luminaire" on from the mode command panel (or the SIMLUX menu) must
        // make the plan answer the very next click. Without this the mode
        // armed itself, the ghost marker never showed, and every click fell
        // through to the drafting canvas — "clicking place luminaire does
        // nothing".
        self.light.simlux_mode
            || self.light.view3d_open
            || self.light.window_open
            || self.light.place_mode
            || self.light.aim_mode
            || self.light.place_fitting.is_some()
    }

    pub(super) fn simlux_pointer_2d(
        &mut self,
        resp: &egui::Response,
        rect: egui::Rect,
        ctx: &egui::Context,
    ) -> bool {
        // Only while SIMLUX is on screen AND the canvas is the plan. The markers are not drawn
        // otherwise either, and an invisible thing must never eat a click. Clearing hover/drag on
        // the way out also cancels a drag left in flight by entering a sketch mid-gesture, rather
        // than leaving it dangling.
        if !self.simlux_2d_layer_live() {
            self.light.hover = None;
            self.light.drag = None;
            self.light_gesture = false;
            return false;
        }
        let tol = self.lum_pick_tol_m();
        // `interact_pointer_pos` first: during a drag the pointer may leave the canvas, and the
        // fixture should keep following it rather than freeze at the edge.
        let ptr = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let at = ptr.map(|p| {
            let w = self.s2w_m(p, rect);
            (w.x as f32, w.y as f32)
        });
        let over_canvas = ptr.is_some_and(|p| rect.contains(p));
        self.light.hover = match at {
            Some((x, y)) if over_canvas => self.light.pick_at(x, y, tol),
            _ => None,
        };

        // ---- a drag in progress owns everything until the button comes up ----
        if self.light.drag.is_some() {
            if let Some(a) = at {
                // THE MOMENT THE DRAG BECOMES A MOVE, caught either side of `drag_to`.
                //
                // The drawing has not been touched yet here, so it is still the "before" — and the
                // fixtures' own "before" is what `drag_to` has just staged. ONE step covering both,
                // because a marker and its symbol move together and an Undo that took back one of
                // them would leave a state nobody ever made.
                let was_moved = self.light.drag.as_ref().is_some_and(|d| d.moved);
                self.light.drag_to(a);
                let now_moved = self.light.drag.as_ref().is_some_and(|d| d.moved);
                if now_moved && !was_moved {
                    self.snapshot_doc_and_lights();
                }
                self.drag_fixture_symbols();
            }
            if ctx.input(|i| i.pointer.primary_released()) {
                // A PRESS THAT NEVER MOVED IS A SELECTION, not an edit. `begin_drag` stages the
                // fixtures either way because it cannot know yet; this is where that is settled,
                // so clicking a marker to pick it costs no Undo press.
                // ONLY WHAT WAS STAGED. By the time a drag ends the fixtures have already moved,
                // so taking a snapshot here would capture the new positions and undo to them —
                // a step that changes nothing. `commit_light_undo` pushes the copy `begin_drag`
                // put aside, or nothing.
                // Nothing to settle here any more: `drag_to` stages only once the drag has
                // actually become a move, so a press that merely selected a marker has left
                // nothing behind, and the frame-end drain commits whatever a real move did.
                self.light.end_drag();
                // The claim belongs to the gesture. Kept past it, the indices go stale the moment
                // anything else edits the drawing, and the next drag would move whatever had
                // drifted into those slots.
                self.light_drag_symbols.clear();
            }
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            self.light_gesture = true;
            return true;
        }

        // ---- press: grab a marker, or drop a new point ----
        if ctx.input(|i| i.pointer.primary_pressed()) && over_canvas {
            if let Some((x, y)) = at {
                // THE AIM TOOL OWNS THE CLICK WHILE IT IS ARMED, both halves of it.
                //
                // Before selection, before placement, before the drafting canvas: a tool somebody
                // deliberately switched on has to be what answers the next click, or the first
                // click picks a fitting and the second drags it somewhere.
                if self.light.aim_mode {
                    self.aim_click(x, y, tol);
                    self.light_gesture = true;
                    return true;
                }
                let additive = ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                if let Some(id) = self.light.pick_at(x, y, tol) {
                    if additive {
                        self.light.select(id, true);
                    } else {
                        // A press on a marker starts a drag AND selects it. A press that never
                        // moves ends as a plain selection (`end_drag` reports it did nothing), so
                        // one gesture covers both "pick this one" and "move this one".
                        self.begin_fixture_drag(id, (x, y));
                    }
                    self.light_gesture = true;
                    return true;
                }
                // AN ARMED FITTING WINS. Both modes place at a click and only one can act; the
                // Illuminaire one is the deliberate choice made in a window, so it goes first.
                if self.light.place_fitting.is_some() {
                    if self.place_illuminaire_at(x, y) {
                        if let Some(&id) = self.light.selected.first() {
                            self.begin_fixture_drag(id, (x, y));
                            // The placement step already covers this gesture — dropping and
                            // positioning in one press is one act, and one Undo.
                            self.light.discard_staged_undo();
                        }
                        self.light_gesture = true;
                    }
                    return true;
                }
                if self.light.place_mode {
                    let id = self.place_fixture_point(x, y);
                    // Placing arms a drag on the new point, so press-move-release puts it down
                    // AND positions it in one gesture — and a plain click leaves it where it fell.
                    self.begin_fixture_drag(id, (x, y));
                    self.light_gesture = true;
                    return true;
                }
                // A click on bare plan with fixtures selected drops the selection — but does NOT
                // consume the click, so the drafting canvas selects entities exactly as before.
                if !self.light.selected.is_empty() {
                    self.light.clear_selection();
                }
            }
        }
        if ctx.input(|i| i.pointer.primary_released()) {
            self.light_gesture = false;
        }

        // ---- keys: Esc leaves placement, Del removes the selected fixtures ----
        //
        // Only while nothing has keyboard focus. `Context::input` reads RAW key state, which a
        // focused text field does not filter — without this guard, pressing Delete while editing
        // the command line or a fitting's name would quietly delete fixtures on the plan.
        // A TEXT FIELD TAKING KEYSTROKES, not merely something being focused.
        //
        // `focused().is_some()` is true of ANY focused widget — a button, a list row, a checkbox —
        // and egui leaves focus on the last thing clicked. So after any click in the Light Editor
        // or the SIMLUX panel, Delete silently did nothing, which is exactly how it was reported:
        // "delete does nothing when a light is selected".
        //
        // `wants_keyboard_input` is true only while something is actually consuming typing, which
        // is the case this guard exists for — Delete while editing the command line or a fitting's
        // name must edit the text rather than erase fixtures.
        // Esc takes the same reading as Delete — see `delete_targets_lights` for why
        // `wants_keyboard_input` is the wrong question in an app that keeps a command line
        // focused.
        let typing = self.typing_in_a_field();
        if !typing && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.light.aim_mode {
                // FIRST, because it is the mode most likely to be on by mistake — it stays armed
                // between fittings so a run can be aimed without re-arming, which also means it
                // outlives the moment somebody stops thinking about it.
                self.light.aim_mode = false;
                self.light.aim_pick = None;
                self.light.last_msg = "Stopped aiming.".into();
            } else if self.light.place_fitting.is_some() {
                self.light.place_fitting = None;
                self.light.last_msg = "Stopped placing.".into();
            } else if self.light.place_mode {
                self.light.place_mode = false;
                self.light.last_msg = "Stopped placing.".into();
            } else if !self.light.selected.is_empty() {
                self.light.clear_selection();
            }
        }
        if self.delete_targets_lights() && ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.delete_selected_fixtures();
        }

        // Cursor + gesture ownership: while placing, the plan IS the placement tool, so clicks
        // belong to it for as long as the mode is on.
        if self.light.hover.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
            return over_canvas;
        }
        if (self.light.place_mode || self.light.place_fitting.is_some()) && over_canvas {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            return true;
        }
        self.light_gesture
    }

    /// Paint each room's NAME on the plan, at the centre of its outline.
    ///
    /// A room is named in the ▼ Rooms list; this is where that name becomes useful. On a plan with
    /// eight rooms in it, "which one is the store?" is otherwise answered by clicking each in turn.
    ///
    /// Drawn at the AREA-WEIGHTED centroid so an L-shaped room labels inside itself — the mean of
    /// the corners can fall outside a concave outline entirely, putting the name in another room.
    pub(super) fn paint_room_names_2d(&self, painter: &egui::Painter, rect: egui::Rect) {
        if self.factory.rooms.is_empty() {
            return;
        }
        let clip = painter.with_clip_rect(rect);
        for r in &self.factory.rooms {
            let c = r.label_point();
            // Room footprints are METRES (the Factory's world), like every other 3D overlay.
            let p = self.w2s_m(Vec2::new(c.x as f64, c.y as f64), rect);
            if !rect.contains(p) {
                continue;
            }
            let label = format!(
                "{}\n{}",
                r.name,
                crate::factory::length_str(&self.factory.units, r.height),
            );
            // A soft plate behind the text, because a name over a hatched or dense plan is
            // unreadable without one.
            let galley = clip.layout_no_wrap(
                label.clone(),
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(210, 225, 240),
            );
            let pad = egui::vec2(6.0, 3.0);
            let box_rect = egui::Rect::from_center_size(p, galley.size() + pad * 2.0);
            clip.rect_filled(
                box_rect,
                3.0,
                egui::Color32::from_rgba_unmultiplied(16, 28, 40, 190),
            );
            clip.galley(box_rect.min + pad, galley, egui::Color32::PLACEHOLDER);
        }
    }

    /// Paint placed luminaires as markers on the 2D plan.
    ///
    /// Three states have to be readable at a glance, because each one means a different next
    /// action: SELECTED (what a drag/delete/assign will act on), UNASSIGNED (a point that will
    /// emit nothing until a fitting is chosen), and hovered (what a press will grab). An
    /// unassigned point is drawn HOLLOW — it is a mark on the plan, not a light yet.
    pub(super) fn paint_luminaires_2d(&self, painter: &egui::Painter, rect: egui::Rect) {
        // The markers are WORLD metres; a face sketch's canvas is not. See `simlux_2d_layer_live`.
        if self.factory.session.is_some() {
            return;
        }
        // Unbuilt rooms (plan designations / layer imports, footprint in
        // METRES) paint as thin cyan outlines with their names, whether or not
        // any SIMLUX panel is open — they are plan features. Built 3D rooms
        // are drawn by the factory's own room-name overlay.
        let unbuilt: Vec<&crate::factory::RoomInst> = self
            .factory
            .rooms
            .iter()
            .filter(|r| !r.is_built() && r.footprint.len() >= 3)
            .collect();
        let show_ghost = self.light.place_mode && self.simlux_2d_layer_live();
        if self.light.luminaires.is_empty() && !show_ghost && unbuilt.is_empty() {
            return;
        }
        let clip = painter.with_clip_rect(rect);

        if !unbuilt.is_empty() {
            let room_col = egui::Color32::from_rgb(90, 200, 230);
            // (Names come from `paint_room_names_2d`, which labels EVERY room —
            // these rings mark the unbuilt ones so the plan shows which rooms
            // are still outline-only.)
            for r in unbuilt {
                let fp: Vec<egui::Pos2> = r
                    .footprint
                    .iter()
                    .map(|p| self.w2s_m(Vec2::new(p.x as f64, p.y as f64), rect))
                    .collect();
                if fp.len() >= 3 {
                    let mut pts = fp.clone();
                    pts.push(fp[0]); // close the ring
                    clip.add(egui::Shape::line(
                        pts,
                        egui::Stroke::new(
                            1.2,
                            egui::Color32::from_rgba_unmultiplied(90, 200, 230, 190),
                        ),
                    ));
                }
            }
        }

        let gold = egui::Color32::from_rgb(255, 214, 90);
        let dark = egui::Color32::from_rgb(70, 48, 0);
        let sel = egui::Color32::from_rgb(120, 190, 255);
        let waiting = egui::Color32::from_rgb(230, 170, 90);
        for l in &self.light.luminaires {
            // Luminaire positions are METRES (cad_light `Vertex`).
            let p = self.w2s_m(Vec2::new(l.position.x as f64, l.position.y as f64), rect);
            let is_sel = self.light.selected.contains(&l.id);
            let is_hot = self.light.hover == Some(l.id);
            let assigned = self.light.is_assigned(l);
            let body = if assigned { gold } else { waiting };
            if assigned {
                clip.circle_filled(p, 5.0, body);
            } else {
                // Hollow: nothing is emitting here yet.
                clip.circle_stroke(p, 5.0, egui::Stroke::new(1.5, body));
            }
            clip.circle_stroke(
                p,
                8.0,
                egui::Stroke::new(1.5, if is_sel { sel } else { dark }),
            );
            if is_sel || is_hot {
                clip.circle_stroke(
                    p,
                    11.0,
                    egui::Stroke::new(1.0, if is_sel { sel } else { gold }),
                );
            }
            clip.line_segment(
                [egui::pos2(p.x - 9.0, p.y), egui::pos2(p.x + 9.0, p.y)],
                egui::Stroke::new(1.0, body),
            );
            clip.line_segment(
                [egui::pos2(p.x, p.y - 9.0), egui::pos2(p.x, p.y + 9.0)],
                egui::Stroke::new(1.0, body),
            );
        }
        // The ghost marker under the cursor, so placement mode is visible on the PLAN and not only
        // in a toolbar that is on the other half of the screen.
        if show_ghost {
            if let Some(p) = painter.ctx().pointer_hover_pos() {
                if rect.contains(p) {
                    let faint = egui::Color32::from_rgba_unmultiplied(255, 214, 90, 110);
                    clip.circle_stroke(p, 5.0, egui::Stroke::new(1.0, faint));
                    clip.line_segment(
                        [egui::pos2(p.x - 12.0, p.y), egui::pos2(p.x + 12.0, p.y)],
                        egui::Stroke::new(1.0, faint),
                    );
                    clip.line_segment(
                        [egui::pos2(p.x, p.y - 12.0), egui::pos2(p.x, p.y + 12.0)],
                        egui::Stroke::new(1.0, faint),
                    );
                }
            }
        }
    }

    /// The top of the scale, when it is on AUTO — the brightest cell in the project.
    ///
    /// The SAME number the report window computes for itself, so an auto scale puts its band edges
    /// in the same places on screen as it does on the page. Taken over ALL rooms rather than
    /// per-room: two rooms drawn to different ceilings cannot be compared, which is the whole
    /// reason the report has a shared scale.
    /// FROM THE REPORTED GRID, matching `RoomInput::reported` — the report quotes EN 12464-1's
    /// grid, so an auto scale on screen has to be topped by the same number or the viewport and the
    /// page put their band edges in different places.
    fn light_room_max(&self) -> f64 {
        let over_rooms = self
            .light
            .rooms
            .iter()
            .map(|r| {
                if r.grid_en.values.is_empty() {
                    r.grid.max
                } else {
                    r.grid_en.max
                }
            })
            .fold(0.0_f64, f64::max);
        if over_rooms > 0.0 {
            return over_rooms;
        }
        self.light.grid.as_ref().map(|g| g.max).unwrap_or(0.0)
    }

    /// THE RESULT, DRAWN OVER THE FLOOR — the report's picture, in the viewport.
    ///
    /// Asked for as: *"in the simlux window the false colors are not appearing how it appears in
    /// the report. it should be exactly as in the report"*, and *isolux contour lines in the SIMLUX
    /// window*.
    ///
    /// Both read `report_opts` — the same scale, the same band thresholds, the same hand-picked
    /// band colours — so there is no second set of settings to keep in step and no way for the two
    /// pictures to disagree. Changing a band in the report dialog changes the viewport, which is
    /// what *"any changes made in the false color in calculate or report will appear in both
    /// places"* asks for.
    pub(super) fn push_lux_overlay(&self, out: &mut Vec<crate::light3d::V3>) {
        if !self.light.floor_heatmap && !self.light.show_isolux {
            return;
        }
        let room_max = self.light_room_max();
        let ramp = self.light.ramp.rgb_fn();
        let opts = &self.report_opts;
        let paint = |lux: f64| -> [f32; 3] {
            let c = opts.lux_rgb(lux, room_max, ramp);
            // Straight through, as display values — `V3::mode = 0` marks a swatch the viewport does
            // not light, so what is written here is what is seen. That is what makes these the
            // report's bytes and not merely its palette.
            [
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
            ]
        };

        // Every room, each on its own plane — a project is not always one rectangle, and the report
        // draws them one at a time too. With no rooms at all, the whole-model fallback plane.
        let mut sheets: Vec<(&cad_light::LuxGrid, &cad_light::CalcPlane, &[bool])> = self
            .light
            .rooms
            .iter()
            .map(|r| (&r.grid, &r.plane, r.mask.as_slice()))
            .collect();
        if sheets.is_empty() {
            if let (Some(g), Some(p)) = (self.light.grid.as_ref(), self.light.plane.as_ref()) {
                sheets.push((g, p, &[]));
            }
        }

        for (grid, plane, mask) in sheets {
            if grid.values.is_empty() {
                continue;
            }
            let (nx, ny) = Self::overlay_res(grid.cols as usize, grid.rows as usize);
            let f = crate::isolux::sample(grid, mask, nx, ny);
            // The floor, not the working plane: the result has always been read off the floor here
            // and moving it would be a surprise, not a fix.
            //
            // FROM THE MODEL, NOT FROM ARITHMETIC. This was `plane.origin.z - plane_height`, which
            // assumes the working plane is measured up from z = 0. It is not: the reference project
            // stands on a slab whose top is at 0.100 m, so the sheet was laid at 0.005 m — ninety
            // -five millimetres UNDERNEATH the floor, hidden by it completely, and both toggles
            // looked dead. Measured, not guessed: the vertex buffer had all 58,050 of its overlay
            // vertices in it, correctly coloured, at z = 0.005 against a floor mesh starting at
            // 0.100.
            let floor_z = self.floor_under(plane);
            if self.light.floor_heatmap {
                crate::light3d::push_lux_sheet(out, &f, plane, floor_z + 0.005, &paint);
            }
            if self.light.show_isolux {
                // The BAND EDGES, so a line is always the boundary of a colour rather than a
                // second scale drawn over the first. On a continuous scale there are no bands to
                // take, so a round set of steps up to the ceiling stands in — an isolux line has
                // to be AT some number to mean anything.
                let edges = if self.report_opts.scale.bands.is_empty() {
                    let top = self.report_opts.scale.top_lx(room_max);
                    (1..=5).map(|k| top * k as f64 / 6.0).collect::<Vec<_>>()
                } else {
                    self.report_opts.scale.bands.clone()
                };
                // Scaled to the room so the lines stay legible in a big space and do not swamp a
                // small one.
                let w = (plane.width.max(plane.depth) * 0.0015).clamp(0.008, 0.05);
                for t in edges {
                    let segs = crate::isolux::trace(&f, t);
                    crate::light3d::push_isolux_lines(
                        out,
                        &segs,
                        nx,
                        ny,
                        plane,
                        floor_z + 0.010,
                        w,
                        [0.08, 0.09, 0.10],
                    );
                }
            }
        }
    }

    /// WHERE THE FLOOR ACTUALLY IS beneath a working plane, read off the model.
    ///
    /// The lux sheet is laid on the floor, so it has to know where the floor is — and the obvious
    /// arithmetic is wrong. `plane.origin.z - plane_height` assumes the plane is measured up from
    /// z = 0; a room standing on a slab has its floor at the slab's top, and on the reference
    /// project that is 0.100 m. The sheet went to 0.005 m, under the floor, invisible.
    ///
    /// TWO THINGS MAKE THIS FIDDLIER THAN IT LOOKS. Material 0 is "floor", but materials here are
    /// assigned by ORIENTATION — so a ceiling's upward-facing top face is filed as floor too, which
    /// is why only surfaces below the plane count. And a building has more than one storey, so only
    /// surfaces under THIS room's footprint count: the highest floor below the plane is the one this
    /// room stands on.
    pub(super) fn floor_under(&self, plane: &cad_light::CalcPlane) -> f32 {
        let (x0, x1) = (plane.origin.x, plane.origin.x + plane.width);
        let (y0, y1) = (plane.origin.y, plane.origin.y + plane.depth);
        // A margin, so a floor sitting a hair under the plane through rounding is not mistaken for
        // the plane itself.
        let ceiling = plane.origin.z - 0.05;
        let mut best: Option<f32> = None;
        for m in &self.light.meshes {
            if m.material != 0 {
                continue;
            }
            for v in &m.vertices {
                if v.z <= ceiling
                    && v.x >= x0 - 0.5
                    && v.x <= x1 + 0.5
                    && v.y >= y0 - 0.5
                    && v.y <= y1 + 0.5
                {
                    best = Some(best.map_or(v.z, |b: f32| b.max(v.z)));
                }
            }
        }
        // No floor in the model at all — a 2D-only project lit from the plan extrusion. The old
        // arithmetic is then the best available, and it is right for exactly that case.
        best.unwrap_or(plane.origin.z - self.light.plane_height)
    }

    /// How finely the overlay is resampled, from the grid it is drawn from.
    ///
    /// The report interpolates its bands, so an overlay drawn one quad per CALCULATED cell would
    /// staircase where the report curves — the two pictures would differ in exactly the way this
    /// work exists to remove. So the field is supersampled, and the factor is whatever the budget
    /// allows: the SIMLUX view rebuilds its whole vertex buffer every frame, and a sheet is the one
    /// thing here that scales with the grid rather than with the model.
    /// The most sub-cells the false-colour overlay may draw, in 2D and in 3D.
    ///
    /// Public so the test asserts against THIS number rather than a copy of it — the first version
    /// hard-coded 12 000 in both places, and raising one silently failed the other.
    pub(crate) const OVERLAY_BUDGET: usize = 64_000;

    pub(crate) fn overlay_res(cols: usize, rows: usize) -> (usize, usize) {
        // RAISED FROM 12 000, which was the whole of "the false color look like low poly cubes".
        // The reference plan grids 132 x 52 = 6 864 cells: one subdivision is 27 456 quads, so the
        // old budget rejected it and fell to ss = 1 -- a flat colour per 0.25 m cell, staircased,
        // while the report subdivides four times and bands a smooth field. The two drawings of one
        // calculation did not look like each other.
        //
        // 64 000 buys ss = 3 on that plan (396 x 156 = 61 776). The cost is vertices in the 3D
        // buffer and quads in the 2D mesh, both of which are rebuilt per frame -- so this is a
        // number to watch in the perf tap, not one to raise on the grounds that smoother is nicer.
        const BUDGET: usize = CadApp::OVERLAY_BUDGET;
        let (c, r) = (cols.max(1), rows.max(1));
        for ss in [4_usize, 3, 2, 1] {
            if c * ss * r * ss <= BUDGET {
                return (c * ss, r * ss);
            }
        }
        // PAST THE BUDGET EVEN AT ONE QUAD PER CELL. `MAX_GRID_POINTS` is 16,384, so this is
        // reachable on a real project rather than theoretical — and returning `(c, r)` here, which
        // is what this did until a test asked, quietly spends 98,000 vertices A FRAME on a grid
        // finer than the screen resolves. Coarsened to fit, keeping the aspect.
        let k = (BUDGET as f64 / (c as f64 * r as f64)).sqrt();
        let mut x = ((c as f64 * k).floor() as usize).max(2);
        let mut y = ((r as f64 * k).floor() as usize).max(2);
        // A LONG THIN GRID rounds its short axis UP to the floor of two and then spends the budget
        // twice over on the long one — 16,384 × 1 came back as 14,021 × 2. Whichever axis was
        // forced up, the other gives way.
        if x * y > BUDGET {
            x = (BUDGET / y).max(2);
        }
        if x * y > BUDGET {
            y = (BUDGET / x).max(2);
        }
        (x, y)
    }

    /// Stamp a phase boundary in this frame — see [`Self::frame_marks`]. Cheap enough to leave in
    /// unconditionally: one `Instant::now` and a push, a handful of times a frame.
    pub(super) fn mark(&mut self, what: &'static str) {
        if let Some(t0) = self.frame_start {
            let us = t0.elapsed().as_micros() as u32;
            self.frame_marks.push((what, us));
        }
    }

    /// The phase boundaries as `name delta-ms`, longest first — the line that says where a frame
    /// actually went. Sub-millisecond spans are dropped; they are never the answer and they crowd
    /// out the one that is.
    fn frame_marks_summary(&self) -> String {
        let mut spans: Vec<(&str, u32)> = Vec::with_capacity(self.frame_marks.len());
        let mut prev = 0u32;
        for (name, at) in &self.frame_marks {
            spans.push((name, at.saturating_sub(prev)));
            prev = *at;
        }
        spans.sort_by(|a, b| b.1.cmp(&a.1));
        spans
            .iter()
            .take(5)
            .filter(|(_, us)| *us > 500)
            .map(|(n, us)| format!("{n} {:.1}", *us as f64 / 1000.0))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// EVERYTHING THE CACHED HALF OF THE SIMLUX SCENE IS BUILT FROM, as one number.
    ///
    /// Only the inputs of [`Self::build_scene3d_static`] belong here. Getting this wrong in the
    /// stale direction is worse than the lag it cures — a user who drops a wall's reflectance and
    /// sees nothing change has been told a lie about their room — so the rule is that anything the
    /// static build reads is either in this key or is a counter that moves when it changes.
    ///
    /// The two heavy inputs are counters rather than content, because hashing them would cost what
    /// rebuilding costs: `meshes_gen` moves whenever the scene triangles are replaced, and the
    /// factory's own `geom_version` moves whenever the CSG model is rebuilt. Everything else here
    /// is a handful of scalars and is hashed by value.
    ///
    /// IT MIRRORS THE BUILD'S BRANCH, and both halves of that matter.
    ///
    /// The build stopped reading `light.meshes` when the view moved to display detail; it reads the
    /// FACTORY now, except on a 2D-only project. Hashing `meshes_gen` regardless then cost a full
    /// 105 MB re-upload every time a calculation landed, for a buffer whose inputs had not moved —
    /// two of those fire back to back on load, and the session dump shows them as 354 ms frames.
    ///
    /// The other direction was worse and was the real bug: `geom_version` is bumped by `recompute`,
    /// which is the CSG solve — **placing, moving, rotating or deleting FURNITURE does not touch
    /// it**. So once the build began reading furniture live, nothing in this key moved when a piece
    /// did, and the SIMLUX view would have gone on showing the old pose indefinitely. The instances
    /// are hashed through the very matrix the build uses, so the two cannot disagree about what a
    /// pose is.
    pub(super) fn scene3d_static_key(&self) -> u64 {
        let mut f = crate::light::Fnv::new();
        // Shared by both branches.
        f.u64(self.light.hide_ceilings as u64);
        f.f32(self.light.plane_height);
        // THROUGH JSON, for the reason `hash_json` gives: a material gains fields, and a hand
        // written list of them is a list that falls behind without saying so. The failure here
        // would be a reflectance edit that changes the report and not the picture.
        crate::light::hash_json(&mut f, "materials", &self.light.materials);

        let from_factory =
            self.factory.cached.positions.len() >= 3 || !self.factory.furniture.is_empty();
        f.u64(from_factory as u64);
        if from_factory {
            f.u64(self.factory.geom_version);
            f.u64(self.factory.cached.positions.len() as u64);
            // EVERY INSTANCE, THROUGH ITS OWN MODEL MATRIX — 26 of them on the reference plan, so
            // 416 floats, which is nothing beside the buffer it guards.
            f.u64(self.factory.furniture.len() as u64);
            for (i, inst) in self.factory.furniture.iter().enumerate() {
                f.u64(inst.asset as u64);
                if let Some(m) = self.factory.furniture_model_matrix(i) {
                    for v in m {
                        f.f32(v);
                    }
                }
            }
        } else {
            // The 2D-only fallback is the one path that still draws `light.meshes`, so this is the
            // one place its generation counter is an input.
            f.u64(self.light.meshes_gen);
        }
        f.finish()
    }

    /// The cached room geometry, rebuilt only when [`Self::scene3d_static_key`] moves.
    fn scene3d_static(&mut self) -> StdArc<Vec<crate::light3d::V3>> {
        let key = self.scene3d_static_key();
        if self.scene3d_static_key != Some(key) {
            self.scene3d_static = StdArc::new(self.build_scene3d_static());
            self.scene3d_static_key = Some(key);
        }
        self.scene3d_static.clone()
    }

    /// THE PER-FRAME HALF: the lux sheet, the fittings, and where they point.
    ///
    /// Small by construction — the sheet is capped at `overlay_res`'s 12,000 cells per room and the
    /// rest scales with the number of fittings — and genuinely frame-dependent: the marker glyph is
    /// sized by camera distance, and the scale the sheet is painted in is edited live from the
    /// toolbar. Kept out of the cache so that editing either updates on the next frame, with no
    /// cache key to get wrong.
    pub(super) fn build_scene3d_dyn(&self) -> Vec<crate::light3d::V3> {
        let mut verts = Vec::new();
        if self.light.meshes.is_empty() {
            return verts;
        }
        self.push_lux_overlay(&mut verts);
        // EACH FITTING AT ITS REAL SIZE, when its file declares one.
        //
        // "i need to see the illuminare as the dimensions in the ies/ldt files not the diamond
        // icon." The marker is sized by camera distance and clamped to 100–600 mm, so it says
        // nothing about the product. The housing dimensions are already parsed and already in
        // metres — LDT converts mm→m, IES converts feet — so this only has to draw them.
        //
        // The icon stays as the fallback and that is not an edge case: most profiles declare no
        // dimensions at all, including the built-in downlight and every curved-light emitter, and a
        // body of size zero would be nothing to look at.
        let s = (self.light.cam_dist * 0.02).clamp(0.05, 0.3);
        for l in &self.light.luminaires {
            let body = self
                .light
                .profiles
                .get(&l.profile)
                .and_then(|p| p.housing_shape().zip(p.housing()));
            match body {
                Some((shape, (_, _, h))) => crate::light3d::push_luminaire_body(
                    &mut verts,
                    [l.position.x, l.position.y, l.position.z],
                    shape,
                    h as f32,
                    l.rotation_deg,
                ),
                None => crate::light3d::push_luminaire_marker(
                    &mut verts,
                    l.position.x,
                    l.position.y,
                    l.position.z,
                    s,
                ),
            }
        }
        self.push_aim_arrows(&mut verts);
        verts
    }

    /// Build the flat-shaded room geometry for the 3D view — the CACHED half.
    pub(super) fn build_scene3d_static(&self) -> Vec<crate::light3d::V3> {
        if self.light.meshes.is_empty() {
            return Vec::new();
        }
        // THE FLOOR IS A FLOOR. The result is drawn over it as its own sheet — see
        // `light3d::push_lux_sheet` for the measurement that says why vertex colours could never
        // have done it.
        let floor = None;
        // HIDE CEILINGS. The result is painted on the FLOOR, and a closed box hides the one
        // surface this view exists to show. Filtered here rather than in the mesh build so the
        // CALCULATION still sees the ceiling — it is 70 % of the interreflection, and a view
        // option that changed the answer would be a trap.
        // …and filtered by FEATURE, not by material. Material here is assigned by ORIENTATION, so
        // dropping material 2 took only the ceiling's UNDERSIDE: its top face is `n.z > 0.7`, i.e.
        // material 0 (floor), and stayed — as did the building's own roof. Looking down, the room
        // was still lidded and the toggle looked broken, which is what was reported. Twice.
        //
        // Rebuilt from the factory with the ceiling features left out, the way the 3D Factory does
        // it, so both faces go. `self.light.meshes` is untouched and Calculate still sees the
        // ceiling. With no 3D model — a 2D-only project lit from the plan extrusion — there are no
        // features to ask about, so the old material filter is still the best available.
        // AT DISPLAY DETAIL, NOT THE CALCULATION'S.
        //
        // This took `self.light.meshes` — the geometry the ENGINE was handed — and drew it. On the
        // gym plan that is 7,036,129 triangles baked into one world-space soup: 21,104,808 vertices,
        // 844 MB. Caching it stopped the rebuild and did nothing about the size, and parking 844 MB
        // in a persistent GPU buffer is past what a card has spare, so it spills over the bus and
        // every draw starves. That is why the lag was SIMLUX-only: the 3D Factory instances its
        // furniture — one buffer per asset, twenty-six matrices — where this bakes each instance out
        // in full, so the same mesh is paid for twenty-six times.
        //
        // `FurnitureDetail::Proxy` is the geometry the 3D Factory already displays, so the two views
        // agree, and `light.meshes` is untouched — Calculate still sees every triangle.
        let plane_z = self.light.plane_height.max(0.1);
        let shown: Vec<cad_light::Mesh> =
            if self.factory.cached.positions.len() >= 3 || !self.factory.furniture.is_empty() {
                // Everything horizontal above the working plane. See `meshes_from_factory_ex` for why
                // this is geometric and not per-material or per-feature — both of those were tried and
                // measured, and both left the room lidded.
                crate::light::meshes_from_factory_detail(
                    &self.factory,
                    self.light.hide_ceilings.then_some(plane_z),
                    crate::light::FurnitureDetail::Proxy,
                )
            } else if !self.light.hide_ceilings {
                self.light.meshes.clone()
            } else {
                self.light
                    .meshes
                    .iter()
                    .filter(|m| m.material != 2)
                    .cloned()
                    .collect()
            };
        crate::light3d::build_scene_verts(&shown, &self.light.materials, floor)
    }

    /// WHERE EVERY FITTING IS POINTING — a shaft to the floor, with a cross where it lands.
    ///
    /// Asked for as: *"in aiming lights add aiming arrows that the user can turn on and off in the
    /// illuminaire tab that shows where the light is aimed at."*
    ///
    /// The arrow ends where the aim ray MEETS THE FLOOR, because that is the question: the aim tool
    /// takes a point on the plan, so the arrow should land on the point that was clicked. Casting
    /// against the whole model instead would be more literal and worse — 89 fittings against a
    /// hundred thousand triangles, every frame, to move the tip onto a desk.
    pub(super) fn push_aim_arrows(&self, out: &mut Vec<crate::light3d::V3>) {
        if !self.light.show_aim || self.light.luminaires.is_empty() {
            return;
        }
        // The floor under the room being looked at. One height for all of them: the arrows are a
        // readout of direction, and a per-fitting floor search would cost more than it tells.
        let floor_z = match self.light.rooms.first() {
            Some(r) => self.floor_under(&r.plane),
            None => match self.light.plane.as_ref() {
                Some(p) => self.floor_under(p),
                None => 0.0,
            },
        };
        for l in &self.light.luminaires {
            let from = glam::Vec3::new(l.position.x, l.position.y, l.position.z);
            let (aim, _, _) = l.frame();
            // How far along the aim the floor is. `aim.z` is negative for anything pointing down,
            // which is every fitting `aim_at` will produce — it refuses a target that is not below.
            let drop = from.z - floor_z;
            let t = if aim.z < -1e-3 {
                drop / -aim.z
            } else {
                // Aimed level or upward — a wallwasher tipped to the horizontal. There is no floor
                // to land on, so the arrow is drawn a fixed length and simply says which way.
                drop.max(1.0)
            };
            // A ray aimed a hair off horizontal would otherwise draw a kilometre across the site.
            let t = t.clamp(0.05, drop.max(1.0) * 6.0);
            let hit = from + aim * t;
            // AMBER, the colour the fitting markers already use, so the arrow reads as belonging to
            // its light rather than as another kind of object in the room.
            crate::light3d::push_aim_arrow(out, from, hit, [1.0, 0.72, 0.25]);
            // Only mark the landing point when it IS a landing point.
            if aim.z < -1e-3 {
                let size = (self.light.cam_dist * 0.012).clamp(0.05, 0.25);
                crate::light3d::push_aim_target(out, hit, size, [1.0, 0.72, 0.25]);
            }
        }
    }

    /// SIMLUX 3D viewport — a docked right panel that renders the extruded room
    /// via the offscreen-FBO glow renderer. Drag to orbit, scroll to zoom. Must
    /// be added BEFORE the CentralPanel so it reserves the right edge.
    /// Log a 3D Factory event into the SESSION RECORDER.
    ///
    /// The 3D view previously logged NOTHING, so a dump of a 3D problem showed only
    /// 2D noise and the user had no way to see what the 3D side did — the recorder is
    /// this project's review instrument, so a subsystem that doesn't write to it is
    /// undebuggable by design.
    pub(super) fn factory_note(&mut self, msg: String) {
        crate::dbg_event!(self, crate::dbg_recorder::DbgEvent::Note { message: msg });
    }

    /// Record a ZOOM operation for the session recorder: the command, the CHOICES the
    /// command line offers, the resolved sub-command, and the screen zoom status BEFORE →
    /// AFTER. "Without proper data it's not possible to bring [the zoom bug] up."
    pub(super) fn dbg_zoom(
        &mut self,
        cmd: &str,
        choices: &str,
        action: String,
        before: String,
        after: String,
    ) {
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::ZoomOp {
                cmd: cmd.to_string(),
                choices: choices.to_string(),
                action,
                before,
                after,
            }
        );
    }

    /// Handle one zoom OPTION while a 3D zoom is live — the 3D analog of the 2D
    /// `zoom_input_text`. Returns true if the token was a zoom option (handled), false if
    /// it wasn't (the caller then exits zoom and re-dispatches the token as a command).
    pub(super) fn zoom3d_option(&mut self, input: &str) -> bool {
        use crate::factory::ZoomMode;
        let before = self.factory.zoom_status();
        let (action, done): (String, bool) = match input.trim().to_ascii_lowercase().as_str() {
            "r" | "real" | "realtime" => {
                self.factory.zoom_mode = ZoomMode::RealTime;
                self.factory.zoom_drag = None;
                self.factory.zoom_cur = None;
                self.factory.status = "ZOOM real-time — drag up/down to zoom  [Esc]".into();
                ("real-time (drag up/down)".into(), false)
            }
            "" | "w" | "window" => {
                self.factory.zoom_mode = ZoomMode::Window;
                self.factory.zoom_drag = None;
                self.factory.zoom_cur = None;
                self.factory.status =
                    "ZOOM window — drag a box (or click two corners)  [Esc]".into();
                ("window (drag a box / click two corners)".into(), false)
            }
            "e" | "extents" | "a" | "all" => {
                self.factory.zoom_save_prev();
                if self.factory.dirty {
                    self.factory.recompute();
                }
                self.factory.fit();
                ("extents (fit all)".into(), true)
            }
            "p" | "prev" | "previous" => {
                self.factory.zoom_restore_previous();
                ("previous (restored the pre-zoom view)".into(), true)
            }
            other => match other.trim_end_matches('x').parse::<f32>() {
                Ok(f) if f > 0.0 => {
                    self.factory.zoom_save_prev();
                    self.factory.zoom_by(1.0 / f); // "2" ⇒ 2× closer, like AutoCAD nX
                    (format!("scale {f}x"), true)
                }
                _ => return false, // not a zoom option → caller re-dispatches
            },
        };
        if done {
            self.factory.zoom_mode = ZoomMode::Off;
            self.factory.status.clear();
        }
        let after = self.factory.zoom_status();
        self.dbg_zoom(
            "zoom opt",
            "W=window(click) · E=extents · P=previous · nX=scale · drag=real-time",
            action,
            before,
            after,
        );
        true
    }

    /// Start the 3D op's pick phase against the current 3D selection (step 4).
    pub(super) fn factory_begin_op(&mut self, op: cad_solid::modify::ModifyOp) {
        // Snapshot at the START of the op, not at apply: `Modify::feed` mutates the model
        // as the gesture resolves, so by apply time the pre-op state is already gone.
        self.snapshot_factory();
        let m = cad_solid::modify::Modify::new(op, self.factory.selection.clone());
        self.factory.status = m.prompt();
        self.factory.queued = None;
        // THE GIZMO HAS TO AGREE WITH THE COMMAND. Reported as: "once i switched to move after
        // selecting rotate via commands i see the rotate gizmos still."
        //
        // `gizmo_mode` was a toolbar toggle that nothing else touched, so typing MOVE started a
        // move while three rotation rings stayed on screen — showing the user a control for an
        // operation they had just left. The command is the more specific statement of intent, so
        // it wins: the same verb, whichever way it was reached, leaves the same handles on screen.
        use cad_solid::modify::ModifyOp as Op;
        match op {
            Op::Rotate => self.factory.gizmo_mode = crate::factory::GizmoMode::Rotate,
            Op::Move | Op::Copy => self.factory.gizmo_mode = crate::factory::GizmoMode::Move,
            // Scale and Mirror have no gizmo of their own; leaving the rings up would suggest
            // they do, and the move arms at least point at what the op is about to change.
            Op::Scale | Op::Mirror => self.factory.gizmo_mode = crate::factory::GizmoMode::Move,
        }
        self.factory_note(format!(
            "3D begin {} on {} solid(s) — sel={:?}",
            op.label(),
            self.factory.selection.len(),
            self.factory.selection
        ));
        self.factory.modify = Some(m);
    }

    /// Enter during a 3D gather: finalise the basket → the pick phase (mirrors the 2D
    /// `finalise_selection` dispatching a `queued_op`).
    pub(super) fn factory_confirm_gather(&mut self) -> bool {
        let Some(op) = self.factory.queued else {
            return false;
        };
        if self.factory.selection.is_empty() {
            self.factory.abort_op();
            self.history
                .push(format!("  ! {}: nothing selected — cancelled", op.label()));
            return true;
        }
        self.factory_begin_op(op);
        self.history.push(format!("  {}", self.factory.status));
        true
    }

    /// Common tail for a 3D `feed`/`type_value`: apply, log the committed params, keep
    /// or drop the op.
    pub(super) fn factory_after_feed(&mut self, md: cad_solid::modify::Modify, f: cad_solid::modify::Feed) {
        use cad_solid::modify::Feed;
        match f {
            Feed::NeedMore | Feed::AppliedContinue => {
                if matches!(f, Feed::AppliedContinue) {
                    self.factory.dirty = true;
                }
                self.factory.status = md.prompt();
                self.factory.modify = Some(md);
            }
            Feed::Applied => {
                self.factory.dirty = true;
                if let Some(s) = &md.last_summary {
                    let line = format!("3D {} ✓ {s}", md.op.label());
                    self.factory_note(line.clone());
                    self.history.push(format!("  {line}"));
                }
                self.factory.selection.clear(); // step 6: apply clears the selection
                self.factory.abort_op();
            }
        }
    }

    /// Promote selected 2D geometry into 3D Factory wall solids — the practical wall
    /// journey (owner, 2026-07-17): draft in 2D (snapping / ortho / corner-join), select,
    /// right-click → Make 3D wall. Each centerline becomes placed Boxes extruded to the
    /// Factory wall height on the ground plane. Returns `(promoted, skipped)`.
    ///
    /// TWO sources of centerline, deliberately:
    /// - `Geom::Wall` carries its OWN thickness, so it uses that.
    /// - Plain geometry (line / polyline / arc / circle / ellipse) is a bare centerline
    ///   with no thickness. This is what an **imported plan** looks like: DXF has no wall
    ///   entity, and `cad_io`'s own writer notes the centerline+thickness link is lost on
    ///   export. Without this branch an imported or traced plan could not be promoted at
    ///   all. Such geometry takes the CURRENT wall style's thickness — the width a wall
    ///   drawn right now would get, rather than a newly invented default.
    ///
    /// Flattening goes through `cad_solid::geom_outlines`, which reuses the KERNEL's own
    /// arc / ellipse samplers and DXF bulge handling. No geometry is re-derived here
    /// (rule 3).
    pub(super) fn make_3d_wall_from_selection(&mut self) -> (usize, usize) {
        let h = self.factory.wall_height;
        // Thickness for geometry that carries none. Taken from the FACTORY setting, which
        // is editable right next to the wall height — the 2D wall style used to be the
        // only source, which left the 3D view with no thickness control at all.
        let fallback_t = self.factory.wall_thickness;
        // Each selection → ONE alive wall carrying its whole centerline as a footprint
        // (straight = 2 pts, curved = sampled polyline). Keeping it as one footprint is
        // what makes the wall editable as a unit — add/move/delete vertices.
        let mut footprints: Vec<(Vec<glam::Vec2>, f32)> = Vec::new();
        let mut skipped = 0usize;
        for &i in &self.selection {
            let Some(g) = self.doc.dobjects.get(i).map(|d| &d.geom) else {
                continue;
            };
            match g {
                Geom::Wall(w) => {
                    let t = self.dlen_m(w.thickness);
                    let k = self.doc_k();
                    let fp: Vec<glam::Vec2> = w
                        .centerline_polyline(16)
                        .iter()
                        .map(|p| glam::Vec2::new((p.x * k) as f32, (p.y * k) as f32))
                        .collect();
                    if fp.len() >= 2 {
                        footprints.push((fp, t));
                    } else {
                        skipped += 1;
                    }
                }
                other if is_promotable_to_wall(other) => {
                    let paths = self.outlines_m(other);
                    let mut any = false;
                    for p in paths.into_iter().filter(|p| p.len() >= 2) {
                        footprints.push((p, fallback_t));
                        any = true;
                    }
                    if !any {
                        skipped += 1;
                    }
                }
                // Text, dimensions, hatches, points, splines — nothing to extrude.
                // Counted, never silently ignored.
                _ => skipped += 1,
            }
        }
        if footprints.is_empty() {
            return (0, skipped);
        }
        // ONE snapshot for the whole promotion — undoing it should take back the entire
        // "Make 3D wall", not one wall per Ctrl+Z.
        self.snapshot_factory();
        self.factory.open = true;
        let mut promoted = 0usize;
        for (fp, t) in footprints {
            if self.factory.add_wall(fp, t, h).is_some() {
                promoted += 1;
            }
        }
        self.factory.recompute();
        self.factory.fit();
        (promoted, skipped)
    }

    // ===================================================================
    // The four "make it 3D" actions
    // ===================================================================
    //
    // ONE implementation each, called from BOTH surfaces: the 2D canvas right-click AND
    // the 3D Factory menu. Duplicating them per surface is how the two drift until one
    // silently does something the other doesn't.

    /// Is there anything in the selection that could become a wall?
    pub(super) fn can_make_3d_wall(&self) -> bool {
        self.selection.iter().any(|&i| {
            self.doc
                .dobjects
                .get(i)
                .is_some_and(|d| is_promotable_to_wall(&d.geom))
        })
    }

    /// Promote the selection to 3D walls, reporting both what converted and what did not.
    /// Refuse a PLAN-level build while a face sketch is open, and say why.
    ///
    /// The ▼ Building rows — Make building / room / floor / ceiling / 3D walls — read
    /// `self.selection`, which indexes whatever document is installed, and `self.doc`, which during
    /// a session IS the sketch. Draw a rectangle on a wall, select it, click Make floor, and a slab
    /// lands flat on the GROUND at that wall sketch's local (u, v): the reported circle-on-the-floor
    /// bug promoted to a permanent CSG feature. Make room is worse — it carves a void, walls and a
    /// ceiling out of the actual building.
    ///
    /// SWAPPING IN `plan_doc()` WOULD BE WRONG HERE, which is the trap. `self.selection` indexes the
    /// installed document — `factory_reset_doc_state` clears it on every swap precisely to keep that
    /// invariant — so plan indices would silently build a DIFFERENT wrong thing. There is no correct
    /// answer while a sketch is open, so the honest one is to decline and say so.
    pub(super) fn refuse_plan_action_in_sketch(&mut self, what: &str) -> bool {
        if self.factory.session.is_none() {
            return false;
        }
        let msg = format!(
            "{what} works on the PLAN, and a face sketch is open — finish the sketch first \
             (✔ Finish sketch), then select the outline on the plan."
        );
        self.history.push(format!("  ! {msg}"));
        self.factory.status = msg;
        true
    }

    pub(super) fn do_make_3d_wall(&mut self) {
        if self.refuse_plan_action_in_sketch("Make 3D walls") {
            return;
        }
        if !self.can_make_3d_wall() {
            let msg = "select walls / lines / polylines in 2D first, then Make 3D walls";
            self.factory.status = msg.into();
            self.history.push(format!("  ! {msg}"));
            return;
        }
        let (n, skipped) = self.make_3d_wall_from_selection();
        self.history.push(format!("  {n} wall(s) → 3D Factory"));
        if skipped > 0 {
            // Never silent about what did NOT convert.
            self.history.push(format!(
                "  ! {skipped} object(s) skipped (nothing to extrude)"
            ));
        }
        self.factory_note(format!("{n} wall(s) promoted to 3D"));
    }

    /// Extrude the selected closed outline into one solid mass on the active storey.
    pub(super) fn do_make_building(&mut self) {
        if self.refuse_plan_action_in_sketch("Make building") {
            return;
        }
        let Some(outline) = self.slab_outline_from_selection() else {
            // Say what to do, rather than doing nothing. On the STATUS line as well as
            // the history: a user who clicked a menu row is looking at the viewport, not
            // at the command log.
            let msg = "select a CLOSED outline in 2D first (polyline / rectangle / \
                       imported plan), then Make building";
            self.factory.status = msg.into();
            self.history.push(format!("  ! building: {msg}"));
            return;
        };
        self.snapshot_factory();
        let h = self.factory.building_height;
        match self.factory.add_building_outline(&outline, h) {
            Ok(_) => {
                self.factory.open = true;
                self.factory.recompute();
                self.factory.fit();
                self.history.push(format!("  building raised to {h:.2} m"));
            }
            Err(e) => {
                // Refused — pop the snapshot so a failed action leaves no empty undo
                // step, and say WHY.
                self.undo_stack.pop();
                let why = match e {
                    cad_solid::ProfileError::TooFewPoints => "outline needs at least 3 corners",
                    cad_solid::ProfileError::Degenerate => "outline encloses no area",
                    cad_solid::ProfileError::SelfIntersecting => "outline crosses itself",
                };
                self.history.push(format!("  ! building: {why}"));
            }
        }
    }

    /// Carve a room (an interior void) out of the building from the selected closed
    /// outline. The whole point of "add a room": click a polygon inside the building and
    /// it hollows out that space.
    pub(super) fn do_make_room(&mut self) {
        if self.refuse_plan_action_in_sketch("Make room") {
            return;
        }
        let Some(outline) = self.slab_outline_from_selection() else {
            let msg = "select a CLOSED outline first, then Make room";
            self.factory.status = msg.into();
            self.history.push(format!("  ! room: {msg}"));
            return;
        };
        self.snapshot_factory();
        let ceilings_before = self.factory.ceilings.len();
        let feats_before = self.factory.feature_count();
        match self.factory.add_room(&outline) {
            Ok(_) => {
                self.factory.open = true;
                self.factory.recompute();
                let got_ceiling = self.factory.ceilings.len() > ceilings_before;
                // Report the exact geometry so it's clear the walls/ceiling were built.
                let added = self.factory.feature_count().saturating_sub(feats_before);
                let walls = added.saturating_sub(1 + got_ceiling as usize); // minus floor (+ceiling)
                let zspan = self
                    .factory
                    .cached
                    .bounds()
                    .map(|(mn, mx)| format!(", {:.1} m tall", mx[2] - mn[2]))
                    .unwrap_or_default();
                self.history.push(format!(
                    "  room built — floor + {walls} wall(s){}{zspan}",
                    if got_ceiling {
                        " + ceiling"
                    } else {
                        " (open to sky)"
                    }
                ));
                self.factory.status = format!(
                    "room built{zspan} — {}. Orbit (middle-drag) to see it.",
                    if got_ceiling {
                        "has ceiling"
                    } else {
                        "open to sky"
                    }
                );
            }
            Err(e) => {
                self.undo_stack.pop();
                let why = room_error_why(e);
                self.factory.status = format!("room: {why}");
                self.history.push(format!("  ! room: {why}"));
            }
        }
    }

    /// Floor (`is_floor`) or ceiling slab across the selected outline, on the active storey.
    pub(super) fn do_make_slab(&mut self, is_floor: bool) {
        if self.refuse_plan_action_in_sketch("Make floor / ceiling") {
            return;
        }
        let what = if is_floor { "floor" } else { "ceiling" };
        let Some(outline) = self.slab_outline_from_selection() else {
            let msg = format!("{what}: select a CLOSED outline in 2D first");
            self.factory.status = msg.clone();
            self.history.push(format!("  ! {msg}"));
            return;
        };
        let lvl = self
            .factory
            .storeys
            .get(self.factory.active_storey)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        self.snapshot_factory();
        let made = if is_floor {
            self.factory.add_floor(&outline, SLAB_THICKNESS)
        } else {
            self.factory.add_ceiling(&outline, SLAB_THICKNESS)
        };
        match made {
            // Exact for ANY shape now — the slab is an extrusion of the real outline.
            Some(_) => {
                self.factory.open = true;
                self.factory.recompute();
                self.history.push(format!("  {what} added on '{lvl}'"));
            }
            None => {
                self.undo_stack.pop();
                self.history
                    .push(format!("  ! {what}: outline is not a valid closed shape"));
            }
        }
    }

    /// The material section of the properties panel: what this surface IS, and the way through to
    /// the one place that changes it.
    ///
    /// It used to APPLY colour and texture here — a swatch row, a paste/load/procedural row and the
    /// whole texture library, duplicated again in the ▼ Textures menu. Three routes to one property
    /// meant three sets of scoping rules, and the bug the user hit came straight out of that: a
    /// colour applied from the panel went to the whole feature, so painting one wall painted every
    /// wall in the building. Material editing now lives in the Materials Factory, on the user's own
    /// recommendation, and this shows the material and opens it there.
    fn factory_color_section(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Material").small().weak());
        self.factory_texture_section(ui);
    }

    /// Texture controls for the current selection — only shown when it carries a pasted
    /// texture. Lets the user retune the tiling (image repeats) and remove the texture.
    fn factory_texture_section(&mut self, ui: &mut egui::Ui) {
        // PER-SURFACE apply mode — shown whenever a FURNITURE object is selected, so the user can
        // choose Whole object / Face / Piece BEFORE picking a texture from the Textures menu.
        if let Some(fi) = self.factory.sel_furn_primary() {
            use crate::factory::FurnPaintMode as M;
            ui.add_space(2.0);
            ui.label(egui::RichText::new("Apply texture to").small().weak());
            let mut mode = self.factory.furn_paint_mode;
            let label = |m: M| match m {
                M::WholeObject => "Whole object",
                M::Face => "One face",
                M::Piece => "One piece",
            };
            // A ComboBox (not a 3-button row) so it fits the narrow properties column.
            egui::ComboBox::from_id_salt("furn_tex_mode")
                .selected_text(label(mode))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut mode, M::WholeObject, label(M::WholeObject))
                        .on_hover_text("A picked texture covers the entire object.");
                    ui.selectable_value(&mut mode, M::Face, label(M::Face))
                        .on_hover_text("Pick a texture, then click a flat face to texture just that surface.");
                    ui.selectable_value(&mut mode, M::Piece, label(M::Piece))
                        .on_hover_text("Pick a texture, then click a connected sub-part to texture the whole piece.");
                });
            if mode != self.factory.furn_paint_mode {
                self.factory.furn_paint_mode = mode;
                if mode == M::WholeObject {
                    self.factory.furn_tex_brush = None; // leaving paint mode disarms the brush
                    self.factory.furn_face_sel = None;
                }
            }
            if mode != M::WholeObject {
                let has_sel = self
                    .factory
                    .furn_face_sel
                    .as_ref()
                    .map_or(false, |(f, _)| *f == fi);
                let armed = self.factory.furn_tex_brush.is_some();
                let msg = if armed {
                    "  ▸ click faces of the object to texture them"
                } else if has_sel {
                    "  ▸ face selected — pick a texture (▼ Textures) to apply"
                } else {
                    "  ▸ click a face of the object, then pick a texture (▼ Textures)"
                };
                ui.label(
                    egui::RichText::new(msg)
                        .small()
                        .color(crate::theme::color::ACCENT),
                );
            }
            let has_faces = self
                .factory
                .furniture
                .get(fi)
                .map_or(false, |f| !f.surface_texture.is_empty());
            if has_faces
                && ui
                    .small_button(
                        egui::RichText::new("Clear per-face textures")
                            .color(egui::Color32::from_rgb(230, 170, 170)),
                    )
                    .clicked()
            {
                self.snapshot_factory();
                if let Some(f) = self.factory.furniture.get_mut(fi) {
                    f.surface_texture.clear();
                }
                self.factory.status = "per-face textures cleared".into();
            }
        }

        self.factory_cuts_panel(ui);

        // NO APPLY UI HERE. The swatch row, the paste/load/procedural row and the texture library
        // all lived at this point and all wrote through a DIFFERENT scoping rule from the Materials
        // Factory's — which is how a colour meant for one wall reached every wall in the building.
        // Picking and applying a material happens in the Materials Factory now; what stays here is
        // the target (which surface) and the way in.

        // WHAT are we tuning? A selected face/piece has its OWN material (so opacity/reflection/
        // tiling land on JUST that piece); otherwise the whole object / feature. `face_sel` is the
        // active per-face selection on the selected furniture (in Face/Piece mode).
        let face_sel: Option<(usize, Vec<u32>)> = if let Some(fi) = self.factory.sel_furn_primary()
        {
            if self.factory.furn_paint_mode != crate::factory::FurnPaintMode::WholeObject {
                self.factory.furn_face_sel.clone().filter(|(f, _)| *f == fi)
            } else {
                None
            }
        } else {
            None
        };
        // Selected CSG feature-solid(s) — e.g. individual treads of a boolean stair. Each gets its
        // own material on tune (copy-on-write), so opacity/reflection land on JUST those solids.
        let feat_sel: Option<Vec<u32>> =
            if self.factory.sel_furniture.is_empty() && !self.factory.selection.is_empty() {
                Some(self.factory.selection.clone())
            } else {
                None
            };
        // The index whose CURRENT values we display: the piece's material if painted, else the
        // whole-object texture (the piece splits off its own copy on first change); for features,
        // the first selected solid's texture.
        let tex_idx: Option<usize> = if let Some((fi, ref groups)) = face_sel {
            self.factory
                .face_material(fi, groups)
                .or_else(|| self.factory.furniture.get(fi).and_then(|f| f.texture))
        } else if let Some(fi) = self.factory.sel_furn_primary() {
            self.factory.furniture.get(fi).and_then(|f| f.texture)
        } else if let Some(ref ids) = feat_sel {
            ids.iter()
                .find_map(|id| self.factory.feature_texture.get(id).copied())
        } else {
            None
        };
        // A selected piece/solid can ALWAYS be tuned (its material is minted on first change).
        // An object with no material yet used to fall through to the picker that sat above; with
        // the picker gone that would be a dead end — selected, and no way to give it a material at
        // all. So it gets one minted on the spot, which is what the Materials Factory would have
        // done on the first edit anyway.
        if tex_idx.is_none() && face_sel.is_none() && feat_sel.is_none() {
            if self.factory.has_any_selection()
                && ui
                    .button("🎨  Give it a material…")
                    .on_hover_text(
                        "Create a material for this object and open it in the Materials Factory.",
                    )
                    .clicked()
            {
                self.snapshot_factory();
                let n = self.factory.textures.len() + 1;
                let base = self
                    .factory
                    .sel_furn_primary()
                    .and_then(|fi| self.factory.furniture.get(fi))
                    .map(|f| f.color)
                    .unwrap_or([0.75, 0.75, 0.75]);
                let i = self.factory.add_procedural_texture(
                    format!("Material {n}"),
                    crate::factory::ProcDef::solid(base),
                );
                self.apply_texture_index_to_selection(i, &format!("Material {n}"), 1, 1);
                self.materials.graphs.remove(&i);
                self.materials.sel = Some(i);
                self.materials.sel_node = None;
                self.materials_open = true;
            }
            return;
        }

        ui.add_space(2.0);
        // The property EDITORS moved to the 🎨 Materials Factory (one place for all material
        // editing — pattern/colours/tiling/opacity/PBR live in its node inspector). This button
        // resolves the tuning target (minting a copy-on-write material for an isolated face/piece,
        // exactly as the old sliders did) and opens it there.
        let isolated = face_sel.is_some() || feat_sel.as_ref().map_or(false, |v| !v.is_empty());
        if ui
            .button(if isolated { "🎨  Edit piece material…" } else { "🎨  Edit material…" })
            .on_hover_text("Open this surface's material in the Materials Factory — colours, pattern, tiling, opacity (Alpha), metallic/roughness and maps are edited there and update live.")
            .clicked()
        {
            self.snapshot_factory();
            if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                self.materials.graphs.remove(&ti); // re-seed from the material's current state
                self.materials.sel = Some(ti);
                self.materials.sel_node = None;
                self.materials_open = true;
            }
        }
        if ui
            .small_button(
                egui::RichText::new("Remove texture").color(egui::Color32::from_rgb(230, 170, 170)),
            )
            .clicked()
        {
            if let Some((fi, groups)) = &face_sel {
                // Remove only THIS piece's per-face textures, leaving the rest of the object.
                self.snapshot_factory();
                if let Some(inst) = self.factory.furniture.get_mut(*fi) {
                    for g in groups {
                        inst.surface_texture.remove(g);
                    }
                }
                self.factory.status = "piece texture removed".into();
            } else {
                self.remove_texture_from_selection();
            }
        }
    }

    #[allow(dead_code)]
    fn factory_adjust_legacy(
        &mut self,
        ui: &mut egui::Ui,
        face_sel: Option<(usize, Vec<u32>)>,
        feat_sel: Option<Vec<u32>>,
        tex_idx: Option<usize>,
    ) {
        let furniture = !self.factory.sel_furniture.is_empty();
        // Is the surface a PROCEDURAL material? Its pattern/colours/grain are edited below; the
        // image-only tiling/move/rotate controls are hidden (it's world-space, not UV-mapped).
        let proc_def: Option<crate::factory::ProcDef> = tex_idx
            .and_then(|ti| self.factory.textures.get(ti))
            .and_then(|t| t.proc);
        if let Some(mut d) = proc_def {
            use crate::factory::ProcPattern;
            let mut changed = false;
            let mut started = false;
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::Label::new(egui::RichText::new("pattern").small().weak()),
                );
                egui::ComboBox::from_id_salt("proc_pattern")
                    .width(110.0)
                    .selected_text(d.pattern.label())
                    .show_ui(ui, |ui| {
                        for p in ProcPattern::ALL {
                            if ui.selectable_value(&mut d.pattern, p, p.label()).changed() {
                                changed = true;
                                started = true;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::Label::new(egui::RichText::new("colours").small().weak()),
                )
                .on_hover_text("Dark→light ramp for wood/marble/noise; the two cells for checker.");
                let ra = ui.color_edit_button_rgb(&mut d.col_a);
                let rb = ui.color_edit_button_rgb(&mut d.col_b);
                for r in [&ra, &rb] {
                    if r.changed() {
                        changed = true;
                    }
                    if r.drag_started() {
                        started = true;
                    }
                }
            });
            // Grain scale — anisotropic: across / along / through. Bigger = finer/denser grain.
            ui.horizontal(|ui| {
                ui.add_sized([40.0, 18.0], egui::Label::new(egui::RichText::new("grain").small().weak()))
                    .on_hover_text("World-space scale across / along / through the grain (anisotropic — squash across, stretch along, for timber).");
                for k in 0..3 {
                    let r = ui.add(egui::DragValue::new(&mut d.scale[k]).update_while_editing(false).speed(0.2).range(0.1..=400.0));
                    if r.changed() { changed = true; }
                    if r.drag_started() { started = true; }
                }
            });
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::Label::new(egui::RichText::new("detail").small().weak()),
                )
                .on_hover_text("Noise octaves (fractal detail) and ramp contrast.");
                let rd = ui.add(
                    egui::DragValue::new(&mut d.detail)
                        .update_while_editing(false)
                        .speed(0.1)
                        .range(1.0..=8.0)
                        .prefix("oct "),
                );
                let rc = ui.add(
                    egui::DragValue::new(&mut d.contrast)
                        .update_while_editing(false)
                        .speed(0.02)
                        .range(0.2..=4.0)
                        .prefix("×"),
                );
                for r in [&rd, &rc] {
                    if r.changed() {
                        changed = true;
                    }
                    if r.drag_started() {
                        started = true;
                    }
                }
            });
            if changed {
                if started {
                    self.snapshot_factory();
                }
                if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                    if let Some(t) = self.factory.textures.get_mut(ti) {
                        t.proc = Some(d);
                        t.avg = d.avg_color();
                    }
                }
            }
        }
        let is_proc = proc_def.is_some();
        // Seed the slider values from the display texture, or sensible defaults for an untextured
        // piece (opacity 100 = opaque, reflect 100 = reflects its surroundings properly).
        let (mut scale, mut off, mut rot, mut opacity_pct, mut reflect_pct) =
            match tex_idx.and_then(|ti| self.factory.textures.get(ti)) {
                Some(t) => (
                    t.scale,
                    t.offset,
                    t.rot_deg,
                    (t.opacity * 100.0).round().clamp(1.0, 100.0),
                    (t.reflect * 100.0).round().clamp(1.0, 100.0),
                ),
                None => (1.0, [0.0, 0.0], 0.0, 100.0, 100.0),
            };
        if !is_proc {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::Label::new(egui::RichText::new("tiling").small().weak()),
                )
                .on_hover_text(if furniture {
                    "How many times the image repeats across the piece."
                } else {
                    "Image repeats per metre of surface."
                });
                let r = ui.add(
                    egui::DragValue::new(&mut scale)
                        .update_while_editing(false)
                        .speed(0.05)
                        .range(0.05..=200.0),
                );
                if r.drag_started() || (r.changed() && !r.dragged()) {
                    self.snapshot_factory();
                }
                if r.changed() {
                    if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                        self.factory.textures[ti].scale = scale;
                    }
                }
            });
            // MOVE: slide the image across the surface (in tile units).
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::Label::new(egui::RichText::new("move").small().weak()),
                )
                .on_hover_text("Shift the image across the surface (U/V, in tile units).");
                let ru = ui.add(
                    egui::DragValue::new(&mut off[0])
                        .update_while_editing(false)
                        .speed(0.02)
                        .prefix("U ")
                        .range(-100.0..=100.0),
                );
                let rv = ui.add(
                    egui::DragValue::new(&mut off[1])
                        .update_while_editing(false)
                        .speed(0.02)
                        .prefix("V ")
                        .range(-100.0..=100.0),
                );
                for r in [&ru, &rv] {
                    if r.drag_started() || (r.changed() && !r.dragged()) {
                        self.snapshot_factory();
                    }
                }
                if ru.changed() || rv.changed() {
                    if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                        self.factory.textures[ti].offset = off;
                    }
                }
            });
            // ROTATE: spin the image about the tile centre.
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::Label::new(egui::RichText::new("rotate").small().weak()),
                )
                .on_hover_text("Rotate the image on the surface (degrees).");
                let r = ui.add(
                    egui::DragValue::new(&mut rot)
                        .update_while_editing(false)
                        .speed(1.0)
                        .suffix("°")
                        .range(-360.0..=360.0),
                );
                if r.drag_started() || (r.changed() && !r.dragged()) {
                    self.snapshot_factory();
                }
                if r.changed() {
                    if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                        self.factory.textures[ti].rot_deg = rot;
                    }
                }
                if ui
                    .small_button("⟳ 90°")
                    .on_hover_text("Rotate 90°")
                    .clicked()
                {
                    self.snapshot_factory();
                    if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                        self.factory.textures[ti].rot_deg = (rot + 90.0).rem_euclid(360.0);
                    }
                }
            });
        } // end !is_proc (image-only tiling/move/rotate)
          // TRANSPARENCY: 1 = fully see-through, 100 = opaque (spec-style 1..100 slider).
        ui.horizontal(|ui| {
            ui.add_sized(
                [40.0, 18.0],
                egui::Label::new(egui::RichText::new("opacity").small().weak()),
            )
            .on_hover_text("Surface transparency: 1 = fully transparent, 100 = fully opaque.");
            let r = ui.add(egui::Slider::new(&mut opacity_pct, 1.0..=100.0).show_value(true));
            if r.drag_started() || (r.changed() && !r.dragged()) {
                self.snapshot_factory();
            }
            if r.changed() {
                if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                    self.factory.textures[ti].opacity = (opacity_pct / 100.0).clamp(0.01, 1.0);
                }
            }
        });
        // REFLECTION: 100 (the default) = the physically correct amount for this material's
        // roughness and IOR; below that is an artistic knock-down, not a physical statement.
        // The mapping is `pct/100`, NOT `(pct-1)/99`: a stored 0 has to keep meaning "written
        // before reflections worked" so `decode_texture_rec` can migrate it, and the old mapping
        // let a user author exactly 0 at 1%.
        ui.horizontal(|ui| {
            ui.add_sized([40.0, 18.0], egui::Label::new(egui::RichText::new("reflect").small().weak()))
                .on_hover_text("How much of the surface's true reflection to keep: 100 = physically correct, lower = artificially dulled. Use ROUGHNESS to make something matte.");
            let r = ui.add(egui::Slider::new(&mut reflect_pct, 1.0..=100.0).show_value(true));
            if r.drag_started() || (r.changed() && !r.dragged()) { self.snapshot_factory(); }
            if r.changed() { if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) { self.factory.textures[ti].reflect = (reflect_pct / 100.0).clamp(0.01, 1.0); } }
        });

        // ── Surface (PBR maps) — a tangent-space NORMAL map + ROUGHNESS, lit by the sun (Texture
        // Phase 2). Roughness works everywhere; the normal/roughness MAPS need the ☀ Sun enabled to
        // show. Maps are picked from textures already in the library (load them first).
        {
            let cur = tex_idx.and_then(|ti| self.factory.textures.get(ti));
            let mut roughness = cur.map(|t| t.roughness).unwrap_or(0.5);
            let cur_nrm = cur.and_then(|t| t.normal_map);
            let cur_rgh = cur.and_then(|t| t.rough_map);
            let lib: Vec<(usize, String)> = self
                .factory
                .textures
                .iter()
                .enumerate()
                .map(|(i, t)| (i, t.name.clone()))
                .collect();
            let name_of = |o: Option<usize>| -> String {
                o.and_then(|i| lib.iter().find(|(j, _)| *j == i))
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| "none".into())
            };
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Surface (PBR maps · needs ☀ Sun)")
                    .small()
                    .weak(),
            );
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::Label::new(egui::RichText::new("rough").small().weak()),
                )
                .on_hover_text(
                    "Roughness: 0 = glossy (tight highlight), 1 = matte. Lit by the sun.",
                );
                let r = ui.add(egui::Slider::new(&mut roughness, 0.0..=1.0).show_value(true));
                if r.drag_started() || (r.changed() && !r.dragged()) {
                    self.snapshot_factory();
                }
                if r.changed() {
                    if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                        self.factory.textures[ti].roughness = roughness;
                    }
                }
            });
            let mut new_nrm = cur_nrm;
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::Label::new(egui::RichText::new("normal").small().weak()),
                )
                .on_hover_text(
                    "A tangent-space normal map (load it as a texture first, then pick it here).",
                );
                egui::ComboBox::from_id_salt("pbr_nrm_map")
                    .width(120.0)
                    .selected_text(name_of(cur_nrm))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut new_nrm, None, "none");
                        for (i, n) in &lib {
                            ui.selectable_value(&mut new_nrm, Some(*i), n);
                        }
                    });
            });
            if new_nrm != cur_nrm {
                self.snapshot_factory();
                if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                    self.factory.textures[ti].normal_map = new_nrm;
                }
            }
            let mut new_rgh = cur_rgh;
            ui.horizontal(|ui| {
                ui.add_sized(
                    [40.0, 18.0],
                    egui::Label::new(egui::RichText::new("r-map").small().weak()),
                )
                .on_hover_text("A roughness map (its red channel drives roughness). Optional.");
                egui::ComboBox::from_id_salt("pbr_rgh_map")
                    .width(120.0)
                    .selected_text(name_of(cur_rgh))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut new_rgh, None, "none");
                        for (i, n) in &lib {
                            ui.selectable_value(&mut new_rgh, Some(*i), n);
                        }
                    });
            });
            if new_rgh != cur_rgh {
                self.snapshot_factory();
                if let Some(ti) = self.tune_target(&face_sel, &feat_sel, tex_idx) {
                    self.factory.textures[ti].rough_map = new_rgh;
                }
            }
        }

        if ui
            .small_button(
                egui::RichText::new("Remove texture").color(egui::Color32::from_rgb(230, 170, 170)),
            )
            .clicked()
        {
            if let Some((fi, groups)) = &face_sel {
                // Remove only THIS piece's per-face textures, leaving the rest of the object.
                self.snapshot_factory();
                if let Some(inst) = self.factory.furniture.get_mut(*fi) {
                    for g in groups {
                        inst.surface_texture.remove(g);
                    }
                }
                self.factory.status = "piece texture removed".into();
            } else {
                self.remove_texture_from_selection();
            }
        }
    }

    /// The texture index a tuning slider should WRITE to. For a selected furniture face/piece OR
    /// CSG feature-solid(s) it mints (copy-on-write) a material EXCLUSIVE to that selection so the
    /// change is isolated; otherwise the shared whole-object texture (`whole_ti`). `None` only when
    /// there is nothing to tune.
    fn tune_target(
        &mut self,
        face_sel: &Option<(usize, Vec<u32>)>,
        feat_sel: &Option<Vec<u32>>,
        whole_ti: Option<usize>,
    ) -> Option<usize> {
        if let Some((fi, groups)) = face_sel {
            Some(self.factory.private_piece_material(*fi, groups))
        } else if let Some(ids) = feat_sel {
            if ids.is_empty() {
                whole_ti
            } else {
                Some(self.factory.private_feature_material(ids))
            }
        } else {
            whole_ti
        }
    }

    /// Drop the texture from the current selection (furniture instance or feature). One undo
    /// step; a feature move needs a re-eval so its triangles rejoin the flat batch.
    pub(super) fn remove_texture_from_selection(&mut self) {
        self.snapshot_factory();
        if let Some(fi) = self.factory.sel_furn_primary() {
            // Drop the baked-in tint back to the asset's own colour, else the piece stays
            // stuck at the texture's average colour after removal.
            let asset_col = self
                .factory
                .furniture
                .get(fi)
                .and_then(|f| self.factory.furniture_lib.get(f.asset))
                .map(|a| a.color);
            if let Some(f) = self.factory.furniture.get_mut(fi) {
                f.texture = None;
                f.surface_texture.clear(); // …and any per-face textures
                if let Some(c) = asset_col {
                    f.color = c;
                }
            }
            self.factory.status = "texture removed".into();
        } else {
            for id in self.factory.selection.clone() {
                self.factory.feature_texture.remove(&id);
                self.factory.feature_color.remove(&id); // back to the default neutral, not the tint
            }
            self.factory.recompute();
            self.factory.status = "texture removed".into();
        }
    }

    /// The Textures menu — the same palette (applied to the selection) plus the phased
    /// create-texture entry.
    /// The Apertures menu — doors & windows. Two ways in: IMPORT one as a free-standing piece to
    /// position by hand, or DRAW a rectangle on a wall and let the app cut the opening and fit a
    /// scaled door/window into it. The draw actions read the rectangle from the active face sketch.
    fn factory_apertures_menu(&mut self, ui: &mut egui::Ui) {
        use crate::factory::ApertureKind;
        let drafting = self.factory.session.is_some();

        ui.label(egui::RichText::new("  Draw an opening").small().weak());
        ui.label(
            egui::RichText::new(
                "  Right-click a wall → Draw on this face, draw a rectangle, then:",
            )
            .small()
            .weak(),
        );
        ui.add_enabled_ui(drafting, |ui| {
            if ui
                .button("🚪  Door in drawn rectangle")
                .on_hover_text("Cut the drawn rectangle through the wall and fit a DOOR scaled to the opening")
                .clicked()
            {
                self.factory_draw_aperture(ApertureKind::Door);
                ui.close_menu();
            }
            if ui
                .button("🪟  Window in drawn rectangle")
                .on_hover_text("Cut the drawn rectangle through the wall and fit a WINDOW scaled to the opening")
                .clicked()
            {
                self.factory_draw_aperture(ApertureKind::Window);
                ui.close_menu();
            }
        });
        if !drafting {
            ui.label(
                egui::RichText::new("  (enabled while drafting on a wall face)")
                    .small()
                    .weak(),
            );
        }

        ui.separator();
        ui.label(egui::RichText::new("  Insert by parameters").small().weak());
        if ui
            .button("🚪  Door (enter parameters)…")
            .on_hover_text(
                "Open the parametric door dialog — set the leaf/frame/casing sizes, then build it",
            )
            .clicked()
        {
            self.arch_tab = ArchTab::Door;
            self.arch_modal_open = true;
            ui.close_menu();
        }

        ui.separator();
        ui.label(egui::RichText::new("  Import free-standing").small().weak());
        if ui
            .button("🚪  Import door")
            .on_hover_text(
                "Drop the default door at the model centre — move/scale it yourself (no cut)",
            )
            .clicked()
        {
            self.factory_import_aperture(ApertureKind::Door);
            ui.close_menu();
        }
        if ui
            .button("🪟  Import window")
            .on_hover_text(
                "Drop the default window at the model centre — move/scale it yourself (no cut)",
            )
            .clicked()
        {
            self.factory_import_aperture(ApertureKind::Window);
            ui.close_menu();
        }

        // ---- PLACED apertures, GROUPED BY ROOM ------------------------------------------------
        //
        // A flat list of "Window 7, Window 8, Door 3" is unusable on a real plan: the names carry
        // no location, so finding the door of the store means clicking each in turn. Grouping by
        // the room the piece actually stands in turns the list into the plan.
        let grouped: Vec<(String, Vec<usize>)> = self
            .factory
            .rooms
            .iter()
            .map(|r| (r.name.clone(), self.factory.openings_in_room(r.id)))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        let orphans = self.factory.openings_without_a_room();
        if !grouped.is_empty() || !orphans.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("  Placed — by room").small().weak());
            let mut pick: Option<usize> = None;
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for (name, items) in &grouped {
                        ui.label(
                            egui::RichText::new(format!("  {name}  ({})", items.len()))
                                .small()
                                .strong(),
                        );
                        for &k in items {
                            let label = self
                                .factory
                                .furniture
                                .get(k)
                                .and_then(|f| self.factory.furniture_lib.get(f.asset))
                                .map(|a| a.name.clone())
                                .unwrap_or_else(|| format!("piece {k}"));
                            if ui.small_button(format!("      {label}")).clicked() {
                                pick = Some(k);
                            }
                        }
                    }
                    // Anything in no room at all is LISTED, not dropped: an opening the app cannot
                    // place is exactly the one worth being told about, and omitting it silently reads
                    // as "there are none".
                    if !orphans.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("  Not in a room  ({})", orphans.len()))
                                .small()
                                .color(egui::Color32::from_rgb(230, 170, 90)),
                        )
                        .on_hover_text("In an external wall, or outside every room outline");
                        for &k in &orphans {
                            let label = self
                                .factory
                                .furniture
                                .get(k)
                                .and_then(|f| self.factory.furniture_lib.get(f.asset))
                                .map(|a| a.name.clone())
                                .unwrap_or_else(|| format!("piece {k}"));
                            if ui.small_button(format!("      {label}")).clicked() {
                                pick = Some(k);
                            }
                        }
                    }
                });
            if let Some(k) = pick {
                self.factory.select_furniture(k);
                ui.close_menu();
            }
        }
    }

    /// The Architecture menu — open the generator modal on the chosen tab. Each entry just picks a
    /// generator and shows the dialog; all parameters and the Build action live in
    /// [`Self::render_arch_dialog`].
    fn factory_architecture_menu(&mut self, ui: &mut egui::Ui) {
        for (tab, glyph, label, hint) in [
            (ArchTab::Staircase, "🪜", "Generate staircase", "Open-sided straight flight or U-shape switchback, with a balustrade"),
            (ArchTab::Spiral, "🌀", "Generate spiral", "Helical treads around a round central post, with a handrail"),
            (ArchTab::Ramp, "📐", "Generate ramp", "A straight inclined deck"),
            (ArchTab::HelicalRamp, "🌀", "Helical ramp", "A sloped annular deck winding around a free inner edge, balustrades on both edges"),
            (ArchTab::Dogleg, "🧱", "Dog-leg stair (editable / boolean)", "Half-turn stair as EDITABLE CSG solids you can cut, union and re-proportion"),
            (ArchTab::SpiralCsg, "🌀", "Spiral stair (editable / boolean)", "Helical stair as EDITABLE CSG solids — set turns, height, radius; cut/union like any solid"),
        ] {
            if ui.button(format!("{glyph}  {label}…")).on_hover_text(hint).clicked() {
                self.arch_tab = tab;
                self.arch_modal_open = true;
                ui.close_menu();
            }
        }
    }

    /// Record any opening the last wall edit orphaned, given the count from BEFORE it.
    ///
    /// The status bar says it and the 3D view marks it, but both are read at the moment they
    /// happen and a wall edit is a busy moment. The history is the line the user scrolls back to
    /// when a window turns out to be missing an hour later, so the loss belongs in it.
    fn note_orphaned_openings(&mut self, lost_before: usize) {
        let lost = self
            .factory
            .orphaned_cutouts()
            .len()
            .saturating_sub(lost_before);
        if lost > 0 {
            self.history.push(format!(
                "  ! {lost} opening(s) lost their wall segment — kept, not applied (▼ Openings)"
            ));
        }
    }

    /// The Openings menu — every cutout (window/door/recess = a Difference feature) listed so
    /// it can be SELECTED (→ its Position/Dimensions load into the properties panel for resize/
    /// move) or DELETED (the opening fills back in). Clicking a cutout in the 3D view is
    /// ambiguous (the hole shows the wall behind), so the list is the reliable way to reach one.
    fn factory_openings_menu(&mut self, ui: &mut egui::Ui) {
        let ids = self.factory.cutout_ids();
        if ids.is_empty() {
            ui.label(egui::RichText::new("  No cutouts yet.").small().weak());
            ui.label(
                egui::RichText::new("  Draw on a face → Cut, to make a window/door.")
                    .small()
                    .weak(),
            );
            return;
        }
        ui.label(
            egui::RichText::new(format!("  {} cutout(s)", ids.len()))
                .small()
                .weak(),
        );
        ui.label(
            egui::RichText::new("  ✏ Edit in 2D reshapes the opening on its plane.")
                .small()
                .weak(),
        );
        // ORPHANS ARE ANNOUNCED, not left to be found. An opening that lost its wall segment is
        // kept but not applied, so the wall renders whole — the list is the only place that can
        // say a window you drew is no longer cutting anything.
        let orphaned = self.factory.orphaned_cutouts();
        if !orphaned.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "  ⚠ {} opening(s) lost their wall — kept, not applied.",
                    orphaned.len()
                ))
                .small()
                .color(egui::Color32::from_rgb(230, 150, 150)),
            );
            // AND THE ROUTE BACK IS NAMED. "Kept and flagged" is only half an answer if nobody
            // can find how to clear the flag — it quietly becomes "kept but permanently dead".
            // ✏ Edit in 2D lifts the flagged cutter out and ✔ Apply re-cuts a live one wherever
            // the outline now sits, which is exactly what this needs; it was already the route
            // and this line is the only place that can say so.
            ui.label(
                egui::RichText::new("  Marked in red in 3D. ✏ Edit in 2D → ✔ Apply re-cuts it.")
                    .small()
                    .weak(),
            );
        }
        ui.separator();
        let selected = self.factory.selected_single();
        let mut to_delete: Option<u32> = None;
        let mut to_select: Option<u32> = None;
        let mut to_edit: Option<u32> = None;
        for (n, id) in ids.iter().enumerate() {
            let size = self.factory.cutout_size(*id).unwrap_or([0.0; 3]);
            // The two largest extents are the opening's face size; the smallest is its depth.
            let mut s = size;
            s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let lost = orphaned.contains(id);
            let label = format!(
                "{}Opening {}  ·  {:.2} × {:.2} m",
                if lost { "⚠ " } else { "" },
                n + 1,
                s[2],
                s[1]
            );
            ui.horizontal(|ui| {
                let mut row = egui::RichText::new(label);
                if lost {
                    row = row.color(egui::Color32::from_rgb(230, 150, 150));
                }
                let resp = ui.selectable_label(selected == Some(*id), row);
                let resp = if lost {
                    resp.on_hover_text(
                        "This opening's wall segment no longer exists — a vertex was added to or \
                         removed from the wall, and no rebuilt segment contains it.\n\nIt is KEPT \
                         but not applied: deleting it would destroy your window, and re-binding it \
                         would cut a different wall. Delete it, or draw the opening again on the \
                         wall you want.",
                    )
                } else {
                    resp
                };
                if resp.clicked() {
                    to_select = Some(*id);
                }
                if ui
                    .small_button("✏")
                    .on_hover_text(
                        "Edit in 2D — reshape/move the opening on its wall plane, then re-Cut",
                    )
                    .clicked()
                {
                    to_edit = Some(*id);
                }
                if ui
                    .small_button(
                        egui::RichText::new("🗑").color(egui::Color32::from_rgb(230, 150, 150)),
                    )
                    .on_hover_text("Delete this cutout — the opening fills back in")
                    .clicked()
                {
                    to_delete = Some(*id);
                }
            });
        }
        if let Some(id) = to_edit {
            self.factory_edit_cutout(id);
            ui.close_menu();
        }
        if let Some(id) = to_select {
            self.factory.select_cutout(id);
            self.active_view = ActiveView::ThreeD;
            self.factory.status = "cutout selected — or ✏ Edit in 2D to reshape it".into();
            ui.close_menu();
        }
        if let Some(id) = to_delete {
            self.snapshot_factory();
            self.factory.delete_cutout(id);
            self.factory.recompute();
            self.factory.status = "cutout removed".into();
            self.history.push("  cutout deleted".into());
        }
    }

    /// EDIT A CUTOUT IN 2D: recover the opening's outline, delete the baked cut, and re-open
    /// the outline as an editable closed polyline ON ITS OWN PLANE in the 2D canvas. The user
    /// reshapes/moves it with the normal 2D tools, then ▼ Room elements ▸ Cut re-applies it.
    /// This is why a cutout is edited here and not nudged in 3D: its natural axes are the
    /// wall-face plane, and a world-space nudge pushes it INTO the wall instead of along it.
    pub(super) fn factory_edit_cutout(&mut self, id: u32) {
        let Some(f) = self
            .factory
            .model
            .features
            .iter()
            .find(|g| g.id == id)
            .copied()
        else {
            return;
        };
        let cad_solid::Primitive::Extrusion { profile, .. } = f.primitive else {
            self.factory.status = "this cutout can't be edited in 2D (not an extrusion)".into();
            return;
        };
        let Some(prof) = self
            .factory
            .model
            .profiles
            .iter()
            .find(|p| p.id == profile)
            .cloned()
        else {
            self.factory.status = "cutout outline not found".into();
            return;
        };
        if prof.pts.len() < 3 {
            self.factory.status = "cutout outline is degenerate".into();
            return;
        }
        // Outline → plane-local (u,v) sketch coordinates: the profile is centred, the placement
        // holds its centre on the plane.
        let (pu, pv) = (f.placement.u, f.placement.v);
        let verts: Vec<PolyVertex> = prof
            .pts
            .iter()
            .map(|q| PolyVertex {
                pos: Vec2::new((pu + q[0]) as f64, (pv + q[1]) as f64),
                bulge: 0.0,
            })
            .collect();
        // The wall-face plane as a drafting frame.
        let (ua, va) = f.plane.axes();
        let frame = cad_solid::Frame {
            origin: f.plane.origin(),
            u: ua,
            v: va,
        };

        // WAS THIS A THROUGH CUT, OR A BLIND RECESS? Asked before anything is removed, because the
        // answer is measured against the solid the cutter still sits in. Apply used to assume
        // THROUGH, which turned a shelf niche into a hole into the next room on a corner drag.
        //
        // `total == 0` means no wall was found under the opening at all — nothing to conclude from,
        // so keep the old assumption rather than inventing a recess out of a failed measurement.
        //
        // ASK THE CUT FIRST. It records what it was for now (see `Feature::through`), and the
        // measurement below is the FALLBACK for openings cut before it did. The inference is not
        // merely less direct, it is wrong in one direction it cannot detect: a through-cut that
        // failed to reach through measures as a recess, so reshaping it re-cut it as the pocket it
        // had accidentally become and the window never came back.
        let through = f.through.unwrap_or_else(|| {
            let (cov_ok, cov_total, _) = self.cut_coverage(&f);
            cov_total == 0 || cov_ok == cov_total
        });
        // The pocket depth to re-cut with, recovered from the cutter itself so a reshape that only
        // moves a corner reproduces the depth exactly. `factory_cut_sketch` builds a recess as
        // `h = depth + CUT_EPS`, so undoing that term here keeps the round-trip stable instead of
        // deepening the pocket by a hair on every edit.
        let depth = match f.primitive {
            cad_solid::Primitive::Extrusion { h, .. } => (h - Self::CUT_EPS).max(0.01),
            _ => self.factory.element_height.max(0.02),
        };

        self.snapshot_factory();
        // Delete every Difference that is THIS opening (same profile + coincident centre) — a
        // through cut may have made one per body; they all rebuild from the re-cut.
        let same: Vec<u32> = self.factory.model.features.iter()
            .filter(|g| g.op == cad_solid::BoolOp::Difference)
            .filter(|g| matches!(g.primitive, cad_solid::Primitive::Extrusion { profile: pp, .. } if pp == profile))
            .filter(|g| (g.placement.u - pu).abs() < 1e-4 && (g.placement.v - pv).abs() < 1e-4)
            .map(|g| g.id)
            .collect();
        // STASH THEM, WITH THE BODY EACH ONE OPENS. Between here and ✔ Apply the opening exists
        // nowhere else in the app — it is out of the model, and the outline in the canvas is only
        // a drawing. Recording the HOST rather than merely the cutter is what lets an abandoned
        // edit put each one back behind its own body instead of behind whichever Union ends up
        // last, which is the defect `rederive_wall` was just fixed for.
        let mut stash: Vec<crate::factory::StashedCut> = Vec::with_capacity(same.len());
        for gid in &same {
            let Some(at) = self
                .factory
                .model
                .features
                .iter()
                .position(|g| g.id == *gid)
            else {
                continue;
            };
            let host = self.factory.model.features[..at]
                .iter()
                .rev()
                .find(|g| g.op == cad_solid::BoolOp::Union)
                .map(|g| g.id);
            stash.push(crate::factory::StashedCut {
                feature: self.factory.model.features[at],
                host,
                at,
            });
        }
        for gid in same {
            self.factory.model.remove(gid);
        }
        self.factory.recompute();
        // Re-open the outline on its plane for editing.
        self.factory_enter_sketch(frame);
        self.add_dobject(
            Geom::Polyline(Polyline {
                vertices: verts,
                closed: true,
                widths: Vec::new(),
            }),
            "cutout-edit",
        );
        // SELECT the outline we just added so its vertex grips show immediately — otherwise the
        // polyline is inert and "drag the points" does nothing until the user thinks to click it.
        // (The polyline is the last dobject pushed.)
        if !self.doc.dobjects.is_empty() {
            let last = self.doc.dobjects.len() - 1;
            self.selection = vec![last];
            self.selected = Some(last);
        }
        // Remember we are reshaping an opening → the sketch panel shows a prominent
        // "drag the points, then Apply" banner and Finish re-cuts automatically.
        self.factory.cutout_edit = Some(crate::factory::CutoutEdit {
            stash,
            through,
            depth,
        });
        self.active_view = ActiveView::TwoD;
        self.factory.status =
            "opening opened in 2D — drag its corner points, then ✔ Apply reshape".into();
        self.history.push("  cutout opened for 2D editing".into());
    }

    /// APPLY a cutout reshape: re-cut the (possibly edited) outline AS THE OPENING IT WAS —
    /// through if it went through, a pocket of the same depth if it was a recess — close the
    /// sketch, and return to 3D so the reshaped hole is visible. Counterpart to
    /// [`Self::factory_edit_cutout`]; the "✔ Apply reshape" button routes here.
    ///
    /// This used to re-cut with a hard-coded `through = true`, so dragging one corner of a
    /// 100 mm niche punched it clean through the wall. A reshape edits the OUTLINE; nothing the
    /// user did here says anything about depth, so nothing here may change it.
    pub(super) fn factory_apply_cutout_reshape(&mut self) {
        // TAKEN FIRST, and that ordering is load-bearing: `factory_exit_sketch` puts an
        // un-consumed stash back, and `factory_cut_sketch` exits the sketch itself on success. If
        // the stash were still held then, the old cutters would be restored alongside the new
        // ones and the opening would be cut twice.
        let edit = self.factory.cutout_edit.take();
        let through = edit.as_ref().is_none_or(|e| e.through);
        let depth = edit.as_ref().map_or(0.0, |e| e.depth);
        // A recess re-cuts at ITS OWN depth. `factory_cut_sketch` reads the pocket depth from the
        // panel's height box, which is a live UI value that has nothing to do with the opening
        // being reshaped — so it is lent the remembered depth and handed straight back.
        let saved_height = self.factory.element_height;
        if let Some(e) = &edit {
            if !e.through {
                self.factory.element_height = e.depth;
            }
        }
        // Must run while the sketch session is still active — `factory_cut_sketch` reads the
        // outline from the live sketch document.
        let made = self.factory_cut_sketch(through);
        self.factory.element_height = saved_height;
        if made == 0 {
            // THE RE-CUT WAS REFUSED, so the opening must come back. Handing the stash back makes
            // the exit below restore it — the same path an abandoned edit takes. Without this a
            // rejected reshape closed the sketch over an opening that had already been deleted.
            self.factory.cutout_edit = edit;
        }
        self.factory_exit_sketch();
        self.active_view = ActiveView::ThreeD;
        if made > 0 {
            self.factory.status = "opening reshaped".into();
            self.history.push(if through {
                "  opening reshaped (re-cut through)".to_string()
            } else {
                format!(
                    "  opening reshaped (re-cut as a {:.0} mm recess)",
                    depth * 1000.0
                )
            });
        } else {
            self.factory.status =
                "reshape not applied — the opening is unchanged (the outline must stay a single \
                 closed shape)"
                    .into();
        }
    }

    /// Put an abandoned cutout edit's cutters back, each BEHIND THE BODY IT OPENS.
    ///
    /// `csg::eval` binds a `Difference` to the nearest `Union` above it, so where a cutter sits is
    /// what decides which body it cuts. Pushing these onto the end would bind every one of them to
    /// whichever body happens to be last — an opening quietly moving house, which is the defect
    /// `rederive_wall` was fixed for and must not be reintroduced by its restore path.
    ///
    /// A body deleted DURING the edit leaves its cutter with nowhere right to go. Rather than
    /// guess, it goes back to the index it came from and the loss is reported: that cutter is now
    /// bound to whatever precedes that index, which may be nothing at all.
    fn factory_restore_stashed_cutters(&mut self, edit: crate::factory::CutoutEdit) {
        if edit.stash.is_empty() {
            return;
        }
        let n = edit.stash.len();
        let mut homeless = 0;
        for s in edit.stash {
            let alive = s
                .host
                .is_some_and(|h| self.factory.model.features.iter().any(|f| f.id == h));
            match (alive, s.host) {
                (true, Some(h)) => {
                    // CALL FIRST, ASSERT SECOND, and this is not style.
                    //
                    // `debug_assert!(expr)` compiles to `if cfg!(debug_assertions) { assert!(expr) }`
                    // — and `cfg!` is a compile-time constant, so in a RELEASE build the
                    // expression is never evaluated. Wrapping the insert in one meant the shipped
                    // binary restored NOTHING: escaping a cutout edit left the openings deleted,
                    // which is exactly the data loss the stash was built to prevent, and the debug
                    // build the tests run in could never see it.
                    //
                    // The host is present — the check above is the guarantee — so `false` here
                    // would be a real bug and the assert says so.
                    let placed = self.factory.model.insert_after(h, s.feature);
                    debug_assert!(
                        placed,
                        "the host was found a moment ago and cannot have moved"
                    );
                }
                _ => {
                    self.factory.model.insert_at(s.at, s.feature);
                    homeless += 1;
                }
            }
        }
        self.factory.dirty = true;
        self.factory.recompute();
        self.factory.status = if homeless > 0 {
            format!("opening restored — {homeless} of {n} cut(s) lost the body they opened")
        } else {
            "reshape abandoned — the opening is back as it was".into()
        };
        self.history.push(if homeless > 0 {
            format!("  ! opening restored, {homeless} of {n} cut(s) lost their body (▼ Openings)")
        } else {
            "  cutout edit abandoned — opening restored".into()
        });
    }

    /// **▼ FBC scene import** — bring a whole Blender scene in, rather than a single prop.
    ///
    /// A scene differs from furniture in two ways that the ordinary import gets wrong:
    ///
    /// - **Scale.** `add_furniture_asset` shrinks anything longer than 20 m toward a 1.5 m asset,
    ///   which is right for a chair exported in millimetres and wrong for a building exported at
    ///   real size. A scene is placed at 1:1 by undoing that exactly (`1 / import_scale`).
    /// - **Placement.** A prop is dropped at the cursor; a scene belongs at the world origin, in
    ///   the coordinates it was authored in, so its sun position and its plan still line up.
    fn factory_scene_import_menu(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("  From Blender").small().weak());
        if ui
            .button("⭳  Import scene (.glb / .gltf / .fbx)…")
            .on_hover_text(
                "Place a whole exported scene at 1:1 in world coordinates, keeping every material as \
                 its own paintable part. Export from Blender with File ▸ Export ▸ glTF 2.0 (.glb), \
                 +Y up, and materials set to Export.",
            )
            .clicked()
        {
            self.scene_import_pending = true;
            let dir = std::path::Path::new(r"G:\blender dev\staircase");
            if dir.is_dir() {
                self.file_dialog_dir = Some(dir.to_path_buf());
            }
            self.open_file_dialog(FileDialogMode::ImportObj, ".glb");
            ui.close_menu();
        }
        if ui
            .button("🏠  Reload the villa scene")
            .on_hover_text(
                "Re-import the Goan villa test scene from the Blender build.\n\
                 134 MB, 2.01 M triangles — a few seconds. No longer loaded at startup;\n\
                 set SIMLUX_VILLA=1 to bring that back.",
            )
            .clicked()
        {
            self.scene_import_pending = true;
            self.import_scene_file(VILLA_SCENE);
            ui.close_menu();
        }
        ui.separator();
        // What a glTF can and cannot carry, stated where someone is about to rely on it — this is
        // the exact thing that made the first villa import render white.
        ui.label(egui::RichText::new("  glTF carries a Principled BSDF whose Base Color is\n  ONE image or a flat colour. A procedural, or an\n  image reached through mix nodes, exports as a\n  flat colour — usually white. Bake those first.").small().weak());
        ui.separator();
        ui.label(egui::RichText::new("  In this project").small().weak());
        let scenes: Vec<(usize, String, usize)> = self
            .factory
            .furniture_lib
            .iter()
            .enumerate()
            .filter(|(_, a)| a.positions.len() / 3 > 50_000)
            .map(|(i, a)| (i, a.name.clone(), a.positions.len() / 3))
            .collect();
        if scenes.is_empty() {
            ui.label(
                egui::RichText::new("  (no scenes imported yet)")
                    .small()
                    .weak(),
            );
        } else {
            for (i, name, tris) in scenes {
                let parts = self.factory.furniture_lib[i]
                    .part_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                ui.label(
                    egui::RichText::new(format!("  {name} — {tris} tris, {parts} materials"))
                        .small(),
                );
            }
        }
    }

    /// Import `path` as a SCENE: 1:1 world scale, at the origin, materials as parts.
    pub(super) fn import_scene_file(&mut self, path: &str) {
        if !std::path::Path::new(path).exists() {
            self.factory.status = format!("Scene import: {path} not found");
            return;
        }
        let before = self.factory.furniture.len();
        self.import_furniture_obj(path);
        if self.factory.furniture.len() == before {
            return; // the importer already reported why
        }
        let Some(fi) = self.factory.sel_furn_primary() else {
            return;
        };
        let k = self
            .factory
            .furniture
            .get(fi)
            .and_then(|f| self.factory.furniture_lib.get(f.asset))
            .map(|a| a.import_scale)
            .unwrap_or(1.0);
        let (tris, parts) = self
            .factory
            .furniture
            .get(fi)
            .and_then(|f| self.factory.furniture_lib.get(f.asset))
            .map(|a| {
                (
                    a.positions.len() / 3,
                    a.part_ids
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len(),
                )
            })
            .unwrap_or((0, 0));
        if let Some(inst) = self.factory.furniture.get_mut(fi) {
            inst.scale = if k > 1e-6 { 1.0 / k } else { 1.0 };
            // A scene is authored about its own origin — dropping it at the cursor would put the
            // building somewhere the plan and the sun no longer agree with.
            inst.pos = [0.0, 0.0, 0.0];
            inst.rot = [0.0, 0.0, 0.0];
        }
        // SWITCH THE DAYLIGHT ON. A scene arrives from Blender having been lit there; left in the
        // app's default studio mode it renders with no sun, no sky and no shadows — a flat grey
        // background, uniform white walls and no cast shadow anywhere. That is not a small
        // difference in quality, it is a different image, and it is why an imported scene looked
        // nothing like the render it came from. Only ever turned ON, and only when the user has
        // not already set it up, so it cannot overwrite a chosen sun.
        let lit = if !self.factory.sun.enabled {
            self.factory.sun.enabled = true;
            self.factory.sun.shadows = true;
            self.factory.sun.sky_backdrop = true;
            // Grade it the way the scene was graded where it came from: Blender's reference render
            // uses AgX with the "Punchy" look at -0.2 EV. Plain AgX leaves an architectural render
            // pale and desaturated — measured on the villa, this takes mean saturation from 0.13
            // to 0.22. Only applied alongside the daylight, so it never overrides a chosen grade.
            self.factory.color.punchy = 1.0;
            self.factory.color.exposure = -0.2;
            ", daylight on, AgX Punchy"
        } else {
            ""
        };
        self.factory.open = true;
        self.factory.fit_all();
        self.active_view = ActiveView::ThreeD;
        self.factory.status =
            format!("scene imported — {tris} triangles, {parts} materials, 1:1 world scale{lit}");
        self.history.push(format!(
            "  imported scene {path} ({tris} tris, {parts} materials)"
        ));
    }

    fn factory_textures_menu(&mut self, ui: &mut egui::Ui) {
        // Paint mode: click individual faces instead of colouring the whole object.
        // WHICH SURFACE gets painted. Per-face is now the default: a building is one extrusion, so
        // per-FEATURE paint puts a wall colour on every wall in the model, which is what
        // "it applied for the entire building except the floor" was. The floor escaped only
        // because a slab is a separate feature.
        let mut paint = self.factory.paint_surface_mode;
        if ui
            .checkbox(&mut paint, "  Paint one face at a time")
            .on_hover_text(
                "On (default): pick a colour or texture, then click a face to paint JUST that \
                 surface.\nOff: the paint covers every face of the selected object — a whole \
                 building at once, if the building is one solid.",
            )
            .changed()
        {
            self.factory.paint_surface_mode = paint;
            if !paint {
                self.factory.surface_tex_brush = None;
            } // leaving paint mode disarms it
        }
        if self.factory.paint_surface_mode {
            if let Some(ti) = self.factory.surface_tex_brush {
                let name = self
                    .factory
                    .textures
                    .get(ti)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                ui.label(
                    egui::RichText::new(format!("  🖌 brush: texture '{name}' — click faces"))
                        .small()
                        .weak(),
                );
            } else {
                ui.label(
                    egui::RichText::new("  → pick a colour or load a texture, then click faces")
                        .small()
                        .weak(),
                );
            }
        } else if self.factory.has_any_selection() {
            ui.label(
                egui::RichText::new("  Colour / texture the selected object")
                    .small()
                    .weak(),
            );
        } else {
            ui.label(
                egui::RichText::new("  Select an object first")
                    .small()
                    .weak(),
            );
        }
        ui.separator();
        // ONE PLACE TO PICK AND APPLY. This menu used to carry its own colour palette, its own
        // paste/load buttons and its own copy of the texture library — a third route to the same
        // property, each with its own idea of what "the selection" meant. What stays here is the
        // brush TARGET above, which is a mode of the 3D view rather than a material.
        if ui
            .button("  🎨 Open Materials Factory…")
            .on_hover_text(
                "Materials — colours, images from the clipboard or disk, procedural patterns, \
                 opacity and PBR — are created and applied there. Click a surface in the 3D view \
                 and it opens that surface's own material.",
            )
            .clicked()
        {
            self.materials_open = true;
            ui.close_menu();
        }
    }

    /// Read the image on the OS clipboard, store it as a texture asset, and apply it to the
    /// current selection. The visible effect for now is the image's average colour (the
    /// existing colour pipeline); the bitmap is kept for the UV-mapped pass. One undo step.
    fn paste_texture_onto_selection(&mut self) {
        if !self.factory.has_any_selection() && !self.factory.paint_surface_mode {
            self.factory.status = "select an object first, then paste a texture".into();
            return;
        }
        if let Some((idx, name, w, h)) = self.paste_texture_as_material() {
            self.apply_texture_index_to_selection(idx, &name, w, h);
        }
    }

    /// Capture the clipboard image as a MATERIAL and return it — without touching any selection.
    ///
    /// Split out because the Materials Factory's job is to build a library, and a library entry
    /// does not need a surface to live on. Requiring one made "paste a texture" fail silently
    /// whenever nothing happened to be selected, which mattered little while the properties panel
    /// carried a second paste button and matters a great deal now that it does not.
    pub(super) fn paste_texture_as_material(&mut self) -> Option<(usize, String, u32, u32)> {
        let img = match arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
            Ok(img) => img,
            Err(e) => {
                self.factory.status =
                    format!("no image on the clipboard — copy a picture first ({e})");
                self.history.push(format!("  ! paste texture: {e}"));
                return None;
            }
        };
        let (w, h) = (img.width as u32, img.height as u32);
        let rgba = img.bytes.into_owned(); // arboard hands over RGBA8, top row first
        if w == 0 || h == 0 || rgba.len() < (w as usize * h as usize * 4) {
            self.factory.status = "clipboard image was empty or malformed".into();
            return None;
        }
        let name = format!("clip-{}x{}-#{}", w, h, self.factory.textures.len() + 1);
        let idx = self.factory.add_texture(name.clone(), w, h, rgba);
        Some((idx, name, w, h))
    }

    /// Load an image FILE (PNG/JPG/…) as a MATERIAL, and apply it if something is selected — the
    /// on-disk counterpart of the clipboard paste. Feeds the bundled CC0 texture library
    /// (`assets/cc0/textures`).
    ///
    /// A selection is NOT required. This is reached from the Materials Factory, whose job is to
    /// build the library; refusing to load an image because no surface happened to be selected
    /// made the only remaining route to a file texture fail for no reason.
    pub(super) fn load_texture_from_file(&mut self, path: &str) {
        let img = match image::open(path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                self.factory.status = format!("could not read image: {e}");
                self.history.push(format!("  ! load texture: {e}"));
                return;
            }
        };
        let (w, h) = (img.width(), img.height());
        let rgba = img.into_raw();
        if w == 0 || h == 0 {
            self.factory.status = "image was empty".into();
            return;
        }
        let name = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "texture".into());
        let idx = self.factory.add_texture(name.clone(), w, h, rgba);
        self.materials.sel = Some(idx);
        self.materials.sel_node = None;
        if self.factory.has_any_selection() || self.factory.paint_surface_mode {
            self.apply_texture_index_to_selection(idx, &name, w, h);
        } else {
            self.factory.status =
                format!("material '{name}' ({w}×{h}) loaded — select a surface and Apply");
        }
    }

    /// Assign stored texture `idx` to the current selection (furniture instance or feature[s])
    /// and tint by its average colour so it shows even before the textured pass. One undo step.
    pub(super) fn apply_texture_index_to_selection(&mut self, idx: usize, name: &str, w: u32, h: u32) {
        // PAINT-SINGLE-SURFACE mode: don't cover the whole solid — arm this texture as the
        // brush and let the user click individual faces (each wall face its own texture).
        if self.factory.paint_surface_mode {
            self.factory.surface_tex_brush = Some(idx);
            self.factory.status =
                format!("texture '{name}' ready — click each face to apply it ({w}×{h})");
            self.history
                .push(format!("  texture '{name}' armed for per-surface painting"));
            return;
        }
        // PER-SURFACE FURNITURE (Face/Piece mode). Two orders:
        //  • the user already clicked a face (furn_face_sel set) → apply the texture to it NOW;
        //  • otherwise ARM the brush so the next face click textures it.
        if let Some(fi) = self.factory.sel_furn_primary() {
            if self.factory.furn_paint_mode != crate::factory::FurnPaintMode::WholeObject {
                let what = if self.factory.furn_paint_mode == crate::factory::FurnPaintMode::Piece {
                    "piece"
                } else {
                    "face"
                };
                let sel = self.factory.furn_face_sel.clone();
                if self.dbg.recording {
                    crate::dbg_event!(self, crate::dbg_recorder::DbgEvent::Note {
                        message: format!(
                            "TEX APPLY '{name}' → per-{what} path; face_sel={:?} (matches inst#{fi}: {})",
                            sel.as_ref().map(|(f, g)| (*f, g.len())),
                            sel.as_ref().map_or(false, |(f, _)| *f == fi),
                        )
                    });
                }
                match sel {
                    Some((sfi, groups)) if sfi == fi => {
                        self.snapshot_factory();
                        self.factory.apply_face_texture(fi, &groups, idx);
                        // Applied → DESELECT the face and disarm (Esc also deselects); the next
                        // click on the object selects a fresh face.
                        self.factory.furn_face_sel = None;
                        self.factory.furn_tex_brush = None;
                        let painted = self.factory.furniture_textured_tri_count(fi);
                        let total = self
                            .factory
                            .furniture
                            .get(fi)
                            .and_then(|f| self.factory.furniture_lib.get(f.asset))
                            .map(|a| a.positions.len() / 3)
                            .unwrap_or(0);
                        if self.dbg.recording {
                            crate::dbg_event!(self, crate::dbg_recorder::DbgEvent::Note {
                                message: format!("TEX APPLIED to {what}: now {painted}/{total} tris textured ({} group(s))", groups.len())
                            });
                        }
                        self.factory.status = format!("texture '{name}' applied to the {what} — {painted}/{total} tris ({w}×{h})");
                        self.history.push(format!(
                            "  texture '{name}' applied to a {what} ({painted}/{total} tris)"
                        ));
                    }
                    _ => {
                        self.factory.furn_tex_brush = Some(idx);
                        self.factory.status = format!("texture '{name}' ready — click a {what} of the object to apply it ({w}×{h})");
                        self.history
                            .push(format!("  texture '{name}' armed for per-{what} painting"));
                    }
                }
                return;
            }
        }
        let tint = self
            .factory
            .textures
            .get(idx)
            .map(|t| t.avg)
            .unwrap_or([0.8, 0.8, 0.82]);
        self.snapshot_factory();
        if let Some(fi) = self.factory.sel_furn_primary() {
            if let Some(inst) = self.factory.furniture.get_mut(fi) {
                inst.texture = Some(idx);
                inst.color = tint;
            }
        } else if self.factory.selection.is_empty() {
            // NOTHING SELECTED. This used to iterate an empty list and silently do nothing.
            // Arm the texture as a face brush instead: clicking a face then textures THAT
            // face, which is what an architectural surface usually wants — a wall's two sides
            // are different materials, and a per-feature texture covers both.
            self.undo_stack.pop(); // nothing changed yet; the paint click takes its own snapshot
            self.factory.paint_surface_mode = true;
            self.factory.surface_tex_brush = Some(idx);
            self.factory.status = format!(
                "texture '{name}' armed — click a FACE to texture just that surface \
                         ({w}×{h}). Select an object first to cover all of its faces."
            );
            self.history
                .push(format!("  texture '{name}' armed for per-face painting"));
            return;
        } else {
            for id in self.factory.selection.clone() {
                self.factory.feature_texture.insert(id, idx);
                self.factory.feature_color.insert(id, tint);
            }
            self.factory.recompute();
        }
        // Say WHICH it was. A per-feature texture covers every face of the solid — both sides
        // of a wall — and being surprised by that reads as "the texture is on the other side".
        let n = self.factory.selection.len();
        self.factory.status = if n > 0 {
            format!(
                "texture '{name}' applied to ALL faces of {n} object(s) ({w}×{h}) — \
                     for one face only, tick 'Paint single surface' and click it"
            )
        } else {
            format!("texture '{name}' applied ({w}×{h})")
        };
        self.history
            .push(format!("  texture '{name}' applied ({w}×{h})"));
    }

    /// Apply a colour to whatever is selected — a CSG feature (building/wall/…) or, if no
    /// feature is selected, the most recently placed furniture. One undo step.
    pub(super) fn apply_color_to_selection(&mut self, c: [f32; 3]) {
        if let Some(fi) = self.factory.sel_furn_primary() {
            // PER-FACE COLOUR: in Face/Piece mode a colour lands on the clicked face/piece (as a
            // 1×1 solid texture), NOT the whole object — matching how per-face TEXTURE works. This
            // is the fix for "I selected a face and the colour changed the whole object".
            if self.factory.furn_paint_mode != crate::factory::FurnPaintMode::WholeObject {
                let what = if self.factory.furn_paint_mode == crate::factory::FurnPaintMode::Piece {
                    "piece"
                } else {
                    "face"
                };
                match self.factory.furn_face_sel.clone() {
                    Some((sfi, groups)) if sfi == fi => {
                        let idx = self.factory.ensure_solid_color_texture(c);
                        self.snapshot_factory();
                        self.factory.apply_face_texture(fi, &groups, idx);
                        let painted = self.factory.furniture_textured_tri_count(fi);
                        let total = self
                            .factory
                            .furniture
                            .get(fi)
                            .and_then(|f| self.factory.furniture_lib.get(f.asset))
                            .map(|a| a.positions.len() / 3)
                            .unwrap_or(0);
                        if self.dbg.recording {
                            crate::dbg_event!(
                                self,
                                crate::dbg_recorder::DbgEvent::Note {
                                    message: format!(
                                        "COLOUR to {what}: now {painted}/{total} tris coloured"
                                    )
                                }
                            );
                        }
                        self.factory.status =
                            format!("colour applied to the {what} — {painted}/{total} tris");
                        self.history.push(format!(
                            "  colour applied to a {what} ({painted}/{total} tris)"
                        ));
                    }
                    _ => {
                        self.factory.status = format!(
                            "{what} mode: click a {what} of the object first, then pick a colour"
                        );
                    }
                }
                return;
            }
            self.snapshot_factory();
            if let Some(inst) = self.factory.furniture.get_mut(fi) {
                inst.color = c;
                inst.texture = None; // a colour REPLACES the texture, else it stays masked
                inst.surface_texture.clear(); // …including any per-face textures
            }
            self.history.push("  furniture colour applied".into());
        } else if !self.factory.selection.is_empty() && self.factory.paint_surface_mode {
            // ONE FACE, NOT THE WHOLE SOLID — "the changes are still getting applied to [the]
            // whole building".
            //
            // The guard used to live in the Textures MENU, so picking a colour there behaved while
            // the properties panel's own swatches called straight through and coloured the entire
            // extrusion — every wall of a building at once. A rule enforced at one of two entry
            // points is not a rule; it belongs here, where every caller passes.
            self.factory.last_pick_color = c;
            self.factory.surface_tex_brush = None; // a colour supersedes any texture brush
            self.factory.status =
                "colour ready — click a face to paint just that surface (turn off \
                 'Paint one face at a time' to cover the whole object)"
                    .into();
            self.history
                .push("  colour armed for per-face painting".into());
        } else if !self.factory.selection.is_empty() {
            self.snapshot_factory();
            for id in self.factory.selection.clone() {
                self.factory.feature_color.insert(id, c);
                // A whole-object colour replaces its texture(s), else the texture keeps masking it.
                self.factory.feature_texture.remove(&id);
                self.factory.surface_texture.retain(|k, _| k.0 != id);
            }
            self.factory.recompute();
            self.history.push("  colour applied".into());
        } else {
            self.factory.status = "select an object first, then pick a colour".into();
        }
    }

    /// Import a furniture mesh (OBJ text or 3DS binary), add it to the project library,
    /// and place one copy at the origin of the active storey. One undo step; the library
    /// persists in the project. Format is chosen by file extension.
    pub(super) fn import_furniture_obj(&mut self, path: &str) {
        let lower = path.to_ascii_lowercase();
        // FBX diagnostics, captured so a broken import is answerable from the dump.
        let mut fbx_note = String::new();
        // glTF PBR extras (real UVs + base-colour image), applied after the asset is added.
        let mut gltf_pbr: Option<crate::mesh_io::GltfPbr> = None;
        let mesh = if lower.ends_with(".3ds") {
            match std::fs::read(path) {
                Ok(bytes) => crate::mesh_io::parse_3ds(&bytes),
                Err(e) => {
                    self.history.push(format!("  ! furniture import: {e}"));
                    return;
                }
            }
        } else if lower.ends_with(".glb") || lower.ends_with(".gltf") {
            match std::fs::read(path) {
                Ok(bytes) => {
                    let base = std::path::Path::new(path).parent();
                    let (m, pbr) = crate::mesh_io::parse_gltf_ex(&bytes, base);
                    gltf_pbr = Some(pbr);
                    m
                }
                Err(e) => {
                    self.history.push(format!("  ! furniture import: {e}"));
                    return;
                }
            }
        } else if lower.ends_with(".fbx") {
            match std::fs::read(path) {
                Ok(bytes) => {
                    let base = std::path::Path::new(path).parent();
                    if crate::mesh_io::is_ascii_fbx(&bytes) {
                        // TEXT FBX (SketchUp / FBX SDK): parse geometry + UVs + the material/texture
                        // graph, flowing textures through the same per-part channel glTF uses.
                        let (m, pbr) = crate::mesh_io::parse_fbx_ascii(&bytes, base);
                        fbx_note = format!(
                            " fbx_ascii=true fbx_tris={} fbx_mats={} fbx_uv={}",
                            m.tri_count(),
                            pbr.textures.len(),
                            !pbr.uvs.is_empty(),
                        );
                        if m.tri_count() == 0 {
                            self.factory.status =
                                "ASCII FBX parsed but produced no triangles — send me this .fbx to fix it".into();
                            self.history.push(format!(
                                "  ! furniture import: ASCII FBX yielded 0 triangles ({fbx_note})"
                            ));
                            return;
                        }
                        gltf_pbr = Some(pbr);
                        m
                    } else {
                        // BINARY FBX. Read geometry, UVs, and each material's IMAGE — embedded in
                        // the file, or the file it names beside it — falling back to its diffuse
                        // colour, flowing them through the same per-part channel glTF uses so each
                        // material is a selectable/paintable part wearing its own texture.
                        let (m, pbr) = crate::mesh_io::parse_fbx_pbr_at(&bytes, base);
                        let images = pbr
                            .textures
                            .iter()
                            .filter(|(w, h, _)| *w > 1 || *h > 1)
                            .count();
                        fbx_note = format!(
                            " fbx_binary=true fbx_tris={} fbx_mats={} fbx_images={} fbx_uv={}",
                            m.tri_count(),
                            pbr.textures.len(),
                            images,
                            !pbr.uvs.is_empty(),
                        );
                        if m.tri_count() == 0 {
                            // Fall back to the geometry-only reader for the richer diagnostic.
                            let (_m2, info2) = crate::mesh_io::parse_fbx_ex(&bytes);
                            self.factory.status = format!(
                                "FBX parsed but produced no triangles (geometries={}, verts={}) — send me this .fbx to fix it",
                                info2.geometries, info2.total_verts,
                            );
                            self.history.push(format!(
                                "  ! furniture import: FBX yielded 0 triangles ({fbx_note})"
                            ));
                            return;
                        }
                        if !pbr.textures.is_empty() {
                            gltf_pbr = Some(pbr); // per-material colours → per-region binding below
                        }
                        m
                    }
                }
                Err(e) => {
                    self.history.push(format!("  ! furniture import: {e}"));
                    return;
                }
            }
        } else {
            match std::fs::read_to_string(path) {
                Ok(t) => crate::mesh_io::parse_obj_dir(&t, std::path::Path::new(path).parent()),
                Err(e) => {
                    self.history.push(format!("  ! furniture import: {e}"));
                    return;
                }
            }
        };
        if mesh.tri_count() == 0 {
            // Surface it on screen too — a silent history line looked like "nothing happened".
            self.factory.status = format!(
                "'{}' imported no geometry — unsupported/compressed mesh or an empty file",
                std::path::Path::new(path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            self.history
                .push("  ! furniture import: no triangles found in the file".into());
            return;
        }
        let name = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "furniture".into());
        let features_before = self.factory.model.features.len();
        self.snapshot_factory();
        let tris = mesh.tri_count();
        let verts = mesh.positions.len();
        let idx = self.factory.add_furniture_asset(name.clone(), mesh);
        // Remember where it came from + that its alpha is authoritative, so a project saved now can
        // re-derive transparency on a later load (see `refresh_imported_furniture_transparency`).
        if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
            a.source_path = std::fs::canonicalize(path)
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
                .or_else(|| Some(path.to_string()));
            a.alpha_resolved = true;
        }
        // glTF PBR passthrough: real UVs + per-primitive PART ids + EVERY material's base colour,
        // so a multi-material model shows each region's own texture (and each primitive is its own
        // selectable piece). `per_part_tex` maps each part id → the global texture index to bind
        // AFTER the instance is placed (via its per-face `surface_texture`).
        let mut gltf_tex_note = String::new();
        let mut per_part_tex: Vec<Option<usize>> = Vec::new();
        if let Some(pbr) = gltf_pbr {
            let ntri = self
                .factory
                .furniture_lib
                .get(idx)
                .map(|a| a.positions.len() / 3)
                .unwrap_or(0);
            if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
                if !pbr.uvs.is_empty() {
                    a.uvs = pbr.uvs;
                }
                if pbr.part_ids.len() == ntri {
                    a.part_ids = pbr.part_ids; // each glTF primitive = one selectable piece
                }
            }
            // Register every distinct material image/colour as a texture → global indices.
            let globals: Vec<usize> = pbr
                .textures
                .iter()
                .enumerate()
                .map(|(i, (tw, th, rgba))| {
                    self.factory
                        .add_texture(format!("{name} mat{i}"), *tw, *th, rgba.clone())
                })
                .collect();
            per_part_tex = pbr
                .part_texture
                .iter()
                .map(|slot| slot.and_then(|s| globals.get(s).copied()))
                .collect();
            // Carry the material's SURFACE properties, not just its colour. Without these every
            // import sat at the app's default 0.5 roughness: a pool authored at 0.035 — a mirror —
            // rendered as flat cyan paint with nothing to reflect, and the whole scene read matte.
            // …and its MAPS. `globals` already holds every image the model referenced — base
            // colours and maps alike — so a map is just another index into it.
            let map_of = |v: &Vec<Option<usize>>, part: usize| -> Option<usize> {
                v.get(part)
                    .copied()
                    .flatten()
                    .and_then(|s| globals.get(s).copied())
            };
            for (part, slot) in pbr.part_texture.iter().enumerate() {
                let (Some(s), Some(g)) = (slot, slot.and_then(|s| globals.get(s).copied())) else {
                    continue;
                };
                let _ = s;
                let nrm = map_of(&pbr.part_normal, part);
                let rgh = map_of(&pbr.part_rough_map, part);
                let mtl = map_of(&pbr.part_metal_map, part);
                let ao = map_of(&pbr.part_ao_map, part);
                if let Some(t) = self.factory.textures.get_mut(g) {
                    if let Some(r) = pbr.part_rough.get(part) {
                        t.roughness = r.clamp(0.0, 1.0);
                    }
                    if let Some(m) = pbr.part_metal.get(part) {
                        t.metallic = m.clamp(0.0, 1.0);
                    }
                    // A MEDIUM, as opposed to a surface with holes — the difference decides whether
                    // this is a volume whose back faces must be dropped. See `TextureAsset`.
                    if let Some(x) = pbr.part_transmission.get(part) {
                        t.transmission = x.clamp(0.0, 1.0);
                    }
                    // A map WINS over the scalar in the shader, so only overwrite when there is
                    // one — a second part sharing this base image must not clear the first's maps.
                    if nrm.is_some() {
                        t.normal_map = nrm;
                    }
                    if rgh.is_some() {
                        t.rough_map = rgh;
                    }
                    if mtl.is_some() {
                        t.metal_map = mtl;
                    }
                    if ao.is_some() {
                        t.ao_map = ao;
                    }
                    // These models carry real UVs, so the maps must be sampled through them and
                    // not world-projected — triplanar would slide a normal map across the wall.
                    t.triplanar = false;
                }
            }
            gltf_tex_note = format!(" gltf_mats={} parts={}", globals.len(), per_part_tex.len());
        }
        // Drop it at the MODEL's centre, not world origin. Imported drawings live at their
        // DXF coordinates (e.g. X≈3619, Y≈956), so placing at (0,0,0) put furniture km away,
        // off-screen — it looked like the import "did nothing". Land it where the building is.
        let at = self.factory.place_at();
        self.factory.place_furniture(idx, at); // selects the new instance (sel_furniture)
                                               // Bind each material to its region via the per-face system: every face-group inherits the
                                               // texture of the PART (glTF primitive) it belongs to, so each material shows on its own
                                               // area (real UVs map the image). A single-material model → every face-group → the one tex.
        if !per_part_tex.is_empty() {
            if let Some(fi) = self.factory.sel_furn_primary() {
                let asset_idx = self.factory.furniture.get(fi).map(|f| f.asset);
                if let Some(ai) = asset_idx {
                    if let Some(a) = self.factory.furniture_lib.get(ai) {
                        let g = a.group_geom();
                        let ntri = a.positions.len() / 3;
                        let mut fg_tex: std::collections::HashMap<u32, usize> =
                            std::collections::HashMap::new();
                        for t in 0..ntri {
                            let part = a.part_ids.get(t).copied().unwrap_or(0) as usize;
                            if let Some(Some(gtex)) = per_part_tex.get(part) {
                                fg_tex.insert(g.face[t], *gtex);
                            }
                        }
                        if let Some(inst) = self.factory.furniture.get_mut(fi) {
                            inst.surface_texture = fg_tex;
                        }
                    }
                }
            }
        }
        self.factory.open = true;
        // Do NOT re-frame the scene on import — the furniture lands at the model centre,
        // which is already in view, and a `fit()` here yanks the camera out to a zoomed-out
        // "whole canvas" angle every time. Keep the user's current viewpoint.
        // Furniture is a mesh INSTANCE, not a CSG feature — so it never shows up in the
        // Union body count and can't be a cut target; record it so a "where did my import
        // go?" is answerable from the dump (asset idx, mesh size, live instance count).
        let fmt = if lower.ends_with(".3ds") {
            "3ds"
        } else if lower.ends_with(".fbx") {
            "fbx"
        } else if lower.ends_with(".glb") {
            "glb"
        } else if lower.ends_with(".gltf") {
            "gltf"
        } else {
            "obj"
        };
        let detail = format!(
            "file={name} fmt={fmt} mesh_tris={tris} mesh_verts={verts} asset_idx={idx} \
             furniture_insts={}{fbx_note}{gltf_tex_note}",
            self.factory.furniture.len(),
        );
        self.factory_op_evt("import-mesh", "file", detail, features_before);
        self.history.push(format!(
            "  furniture '{name}' imported ({tris} tris) and placed"
        ));
    }

    /// Read + parse a mesh file (`.obj` / `.fbx` / `.3ds` / `.glb` / `.gltf`) into an `ObjMesh`.
    /// The shared parser dispatch behind furniture AND aperture loading (no UI, no placement).
    fn factory_parse_mesh_file(path: &str) -> Option<crate::mesh_io::ObjMesh> {
        let lower = path.to_ascii_lowercase();
        let m = if lower.ends_with(".3ds") {
            crate::mesh_io::parse_3ds(&std::fs::read(path).ok()?)
        } else if lower.ends_with(".glb") || lower.ends_with(".gltf") {
            let bytes = std::fs::read(path).ok()?;
            crate::mesh_io::parse_gltf_ex(&bytes, std::path::Path::new(path).parent()).0
        } else if lower.ends_with(".fbx") {
            let bytes = std::fs::read(path).ok()?;
            if crate::mesh_io::is_ascii_fbx(&bytes) {
                crate::mesh_io::parse_fbx_ascii(&bytes, std::path::Path::new(path).parent()).0
            } else {
                crate::mesh_io::parse_fbx_ex(&bytes).0
            }
        } else {
            crate::mesh_io::parse_obj_dir(
                &std::fs::read_to_string(path).ok()?,
                std::path::Path::new(path).parent(),
            )
        };
        (m.tri_count() > 0).then_some(m)
    }

    /// Ensure the bundled default mesh for `kind` is loaded into the furniture library, returning
    /// its asset index. Parsed once (from `assets/apertures/`), then cached in
    /// `factory.aperture_asset` and reused by every door/window placed.
    fn factory_ensure_aperture_asset(
        &mut self,
        kind: crate::factory::ApertureKind,
    ) -> Option<usize> {
        if let Some(i) = self.factory.aperture_asset[kind.idx()] {
            if i < self.factory.furniture_lib.len() {
                return Some(i);
            }
        }
        // After a reload the cache is empty but the mesh is already in the library (persisted);
        // reuse it by name so we don't import a duplicate copy each time.
        if let Some(i) = self
            .factory
            .furniture_lib
            .iter()
            .position(|a| a.name == kind.label())
        {
            self.factory.aperture_asset[kind.idx()] = Some(i);
            return Some(i);
        }
        // The DOOR is now a PARAMETRIC build (default "Door classic" proportions), not a bundled
        // FBX — so it's clean geometry with per-component pieces. The window still loads its OBJ.
        if kind == crate::factory::ApertureKind::Door {
            let idx =
                self.factory_add_door_asset(&cad_solid::door::DoorInput::default(), kind.label())?;
            self.factory.aperture_asset[kind.idx()] = Some(idx);
            return Some(idx);
        }
        let path = kind.asset_file();
        let path = &path.to_string_lossy().into_owned();
        let mesh = match Self::factory_parse_mesh_file(path) {
            Some(m) => m,
            None => {
                self.history.push(format!(
                    "  ! aperture: could not load default {} mesh at '{}'",
                    kind.label(),
                    path
                ));
                return None;
            }
        };
        let idx = self
            .factory
            .add_furniture_asset(kind.label().to_string(), mesh);
        self.factory.aperture_asset[kind.idx()] = Some(idx);
        Some(idx)
    }

    /// The handle library folder. Ships beside the Blender assets; absent on a bare checkout, in
    /// Load the handle library once, on first need. A bare checkout has no asset folder, and the
    /// picker says so rather than pretending the library is empty.
    pub(super) fn handle_lib_ensure(&mut self) {
        if self.handle_lib.is_none() {
            if let Ok(l) = crate::handles::HandleLibrary::load(Self::handle_lib_dir()) {
                if self.handle_sel.is_empty() {
                    self.handle_sel = l.handles.first().map(|h| h.id.clone()).unwrap_or_default();
                }
                self.handle_lib = Some(l);
            }
        }
    }

    /// The Handles dialog: preview tiles for the whole library, opened from the Door panel's
    /// Handle row. Drawn at the top level so it floats over the panel instead of reflowing it.
    pub(super) fn handle_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.handle_dialog {
            return;
        }
        let door = self.arch_door;
        let mut open = true;
        egui::Window::new("Handles")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                self.handle_lib_ensure();
                if self.handle_lib.is_none() {
                    ui.label(
                        egui::RichText::new(format!(
                            "handle library not found at {}",
                            Self::handle_lib_dir().display()
                        ))
                        .small()
                        .weak(),
                    );
                    return;
                }
                ui.label(
                    egui::RichText::new(format!(
                        "for a {:.0} × {:.0} mm leaf at {:.0} mm backset",
                        door.door_width * 1000.0,
                        door.door_height * 1000.0,
                        door.handle_backset * 1000.0
                    ))
                    .small()
                    .weak(),
                );
                ui.separator();
                self.handle_picker(ui, &door);
                ui.separator();
                if ui.button("Done").clicked() {
                    self.handle_dialog = false;
                }
            });
        if !open {
            self.handle_dialog = false;
        }
    }

    /// The DOOR PREVIEW: what you are about to insert, before you insert it.
    ///
    /// Not a screenshot of the viewport — the point is to decide *without* putting a door in the
    /// model first, then hunting it down and undoing it. It renders the very mesh
    /// [`Self::door_mesh`] hands to the builder, welded handle and all, so agreeing with the
    /// picture and agreeing with the result are the same act.
    ///
    /// Returns true when the user asked to build from in here.
    pub(super) fn door_preview_window(&mut self, ctx: &egui::Context) -> bool {
        if !self.door_preview.open {
            return false;
        }
        let inp = self.arch_door;
        let mut open = true;
        let mut build = false;

        // ── The mesh: rebuilt only when the door or its handle actually changed ──────────────
        let geom_sig = preview_hash(
            &[
                inp.door_width,
                inp.door_height,
                inp.door_thickness,
                inp.frame_depth,
                inp.frame_face_width,
                inp.frame_stop_depth,
                inp.arch_width,
                inp.arch_head_width,
                inp.arch_thickness,
                inp.handle_backset,
                inp.handle_height,
                inp.hinge_side,
                inp.stile_width,
                inp.rail_width,
                inp.panel_mould_width,
            ],
            &[&self.handle_sel, &self.handle_finish],
        );
        if self.door_preview.geom_sig != geom_sig || self.door_preview.mesh.is_none() {
            self.door_preview.geom_sig = geom_sig;
            match self.door_mesh(&inp) {
                Ok((m, obj, ids, note)) => {
                    self.door_preview.caption = format!(
                        "structural opening {:.0} × {:.0} mm · casing {:.0} × {:.0} mm · depth {:.0} mm{}",
                        m.structural_opening_w * 1000.0, m.structural_opening_h * 1000.0,
                        m.overall_w * 1000.0, m.overall_h * 1000.0, m.overall_depth * 1000.0,
                        note,
                    );
                    self.door_preview.mesh = Some((obj, ids));
                }
                Err(e) => {
                    self.door_preview.caption = format!("⚠ {e}");
                    self.door_preview.mesh = None;
                }
            }
            self.door_preview.tex_ss = 0; // force a re-render of the picture too
        }

        egui::Window::new("Door preview")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                const VIEW: usize = 460;

                // SIX FIXED VIEWS. See `mesh_preview::View` for why this is not a free orbit.
                ui.horizontal(|ui| {
                    for v in crate::mesh_preview::View::ALL {
                        ui.selectable_value(&mut self.door_preview.view, v, v.label());
                    }
                });
                ui.add_space(2.0);

                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(VIEW as f32, VIEW as f32),
                    egui::Sense::click_and_drag(),
                );

                // ZOOM: scroll on the image, or drag it vertically. Both land on the same number,
                // so there is one thing to reason about.
                let mut zoom = self.door_preview.zoom;
                if resp.hovered() {
                    let scroll = ui.ctx().input(|i| i.raw_scroll_delta.y);
                    if scroll.abs() > 0.5 {
                        zoom *= 1.0 + scroll * 0.0018;
                    }
                }
                if resp.dragged() {
                    zoom *= 1.0 - resp.drag_delta().y * 0.004;
                }
                self.door_preview.zoom = zoom.clamp(0.4, 6.0);

                // Everything the picture depends on EXCEPT how finely it is sampled. Materials
                // belong here, not in the geometry signature: changing oak to walnut must
                // re-shade, never re-parse the handle's FBX.
                let o = self.door_preview.view.orbit(self.door_preview.zoom);
                let img_sig = preview_hash(&[o.yaw, o.pitch, o.zoom], &[&self.handle_finish])
                    ^ self.door_preview.geom_sig
                    ^ self.door_mats.key().wrapping_mul(0x9E37_79B9_7F4A_7C15);

                // PROGRESSIVE, in two levels. A change draws a DRAFT immediately — half
                // resolution, one sample, no procedural relief, which is sixteen times less
                // shading — and asks for one more frame. If nothing has changed by then, the same
                // image is rendered properly. Nobody should wait on the expensive version to find
                // out they turned the wrong knob, and the draft is upscaled by the same bilinear
                // filter the final image is drawn through, so it reads as soft rather than broken.
                let changed = self.door_preview.img_sig != img_sig;
                let level = if changed { 1 } else { 2 };
                if changed || self.door_preview.tex_ss < level || self.door_preview.tex.is_none() {
                    self.door_preview.img_sig = img_sig;
                    self.door_preview.tex_ss = level;
                    if level < 2 {
                        ui.ctx().request_repaint(); // come back and refine it
                    }
                    let (size, ss) = if level < 2 { (VIEW / 2, 1) } else { (VIEW, 2) };
                    // Joinery follows the chosen materials, ironmongery the handle finish — the
                    // same resolution the built door uses, so the two cannot disagree.
                    let mats = self.door_mats;
                    let finish = self.handle_finish.clone();
                    let look = move |id: u32| door_part_look(&mats, &finish, id);
                    let px = match &self.door_preview.mesh {
                        Some((obj, ids)) => crate::mesh_preview::render(
                            &obj.positions,
                            &obj.normals,
                            ids,
                            &look,
                            o,
                            size,
                            ss,
                            self.factory.color,
                        ),
                        None => crate::mesh_preview::render(
                            &[],
                            &[],
                            &[],
                            &look,
                            o,
                            size,
                            ss,
                            self.factory.color,
                        ),
                    };
                    let tex = ui.ctx().load_texture(
                        "door_preview",
                        egui::ColorImage::from_rgba_unmultiplied([size, size], &px),
                        egui::TextureOptions::LINEAR,
                    );
                    self.door_preview.tex = Some(tex);
                }
                if let Some(t) = &self.door_preview.tex {
                    ui.painter().image(
                        t.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                resp.on_hover_text("scroll or drag up/down to zoom");

                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Zoom").small().weak());
                    ui.add(
                        egui::Slider::new(&mut self.door_preview.zoom, 0.4..=6.0)
                            .logarithmic(true)
                            .show_value(false),
                    );
                    ui.label(
                        egui::RichText::new(format!("{:.0}%", self.door_preview.zoom * 100.0))
                            .small()
                            .weak(),
                    );
                    if ui.small_button("Fit").clicked() {
                        self.door_preview.zoom = 1.0;
                    }
                });
                ui.label(
                    egui::RichText::new(&self.door_preview.caption)
                        .small()
                        .color(crate::theme::color::ACCENT),
                );
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let can = self.door_preview.mesh.is_some();
                    if ui
                        .add_enabled(
                            can,
                            egui::Button::new(egui::RichText::new("✚  Build door").strong()),
                        )
                        .clicked()
                    {
                        build = true;
                    }
                    if ui.button("Handles…").clicked() {
                        self.handle_dialog = true;
                    }
                    if ui.button("Close").clicked() {
                        self.door_preview.open = false;
                    }
                });
            });
        if !open {
            self.door_preview.open = false;
        }
        if build {
            self.door_preview.open = false;
        }
        build
    }

    /// Append the chosen handle's geometry to a door mesh, both faces, in the door's own frame.
    /// Returns a note for the status line (empty when nothing was welded).
    ///
    /// One object, not three. The handle is part of the door: erase, move or copy the door and the
    /// hardware goes with it, because it IS the door. Its parts still get their own `part_ids`, so
    /// "select a piece" and per-part recolouring keep working on the lever and the rose.
    ///
    /// The two traps from HANDLES_BUILD.md are in `mount_matrix`: the X coefficient is `hand` on
    /// BOTH faces (the mirror is through the leaf's mid-plane, so it reverses Y and leaves X
    /// alone), and `det M < 0` on exactly one face — where lettering is reflected about `x = 0`
    /// first, so a glyph changes column and the mount's mirror carries it back.
    fn factory_weld_handle(
        &mut self,
        choice: &HandleChoice,
        obj: &mut crate::mesh_io::ObjMesh,
        part_ids: &mut Vec<u32>,
    ) -> String {
        let (h, fit) = match choice {
            HandleChoice::Builtin => return String::new(),
            HandleChoice::Refused(why) => return format!(" ({why})"),
            HandleChoice::Use(h, fit) => (h, *fit),
        };
        let Some(lib) = self.handle_lib.clone() else {
            return String::new();
        };

        let path = lib.mesh_path(h);
        let Ok(bytes) = std::fs::read(&path) else {
            self.history
                .push(format!("  ! handle mesh unreadable: {}", path.display()));
            return String::new();
        };
        let (hm, hp) = crate::mesh_io::parse_fbx_pbr_at(&bytes, path.parent());
        if hm.tri_count() == 0 {
            self.history
                .push(format!("  ! handle mesh empty: {}", path.display()));
            return String::new();
        }

        crate::handles::weld_onto(
            &fit,
            &hm.positions,
            &hm.normals,
            &hp.part_ids,
            &mut obj.positions,
            &mut obj.normals,
            part_ids,
        );
        self.history.push(format!(
            "  handle '{}' welded into the door, both faces",
            h.id
        ));
        format!(" with {}", h.name)
    }

    /// Decide, ONCE, which handle a door wears — and hand back the door input that matches, with
    /// its own lever switched off when a library handle is going on instead.
    ///
    /// Both the preview and the build go through here, which is the point: what the user is shown
    /// and what gets inserted cannot disagree about the hardware.
    fn door_resolved(
        &mut self,
        inp: &cad_solid::door::DoorInput,
    ) -> (cad_solid::door::DoorInput, HandleChoice) {
        self.handle_lib_ensure();
        let choice = match self
            .handle_lib
            .as_ref()
            .and_then(|l| l.get(&self.handle_sel))
            .cloned()
        {
            None => HandleChoice::Builtin,
            Some(h) => {
                let fit = crate::handles::DoorFit {
                    door_width_mm: inp.door_width * 1000.0,
                    door_height_mm: inp.door_height * 1000.0,
                    leaf_thickness_mm: inp.door_thickness * 1000.0,
                    handle_backset_mm: inp.handle_backset * 1000.0,
                    handle_height_mm: inp.handle_height * 1000.0,
                    hinge_side: inp.hinge_side,
                };
                let f = crate::handles::fitness(&h, &fit);
                if f.ok() {
                    HandleChoice::Use(Box::new(h), fit)
                } else {
                    HandleChoice::Refused(format!("no {} — {}", h.name, f.summary()))
                }
            }
        };
        // A refused handle falls BACK to the built-in lever: better a plain door than a bare hole
        // where the hardware should be.
        let builtin = !matches!(choice, HandleChoice::Use(..));
        (
            cad_solid::door::DoorInput {
                builtin_hardware: builtin,
                ..*inp
            },
            choice,
        )
    }

    /// Bind the chosen materials to a placed door, per component.
    ///
    /// The door arrives as ONE mesh whose triangles carry `Part` ids, so the materials go on the
    /// same way a glTF's per-primitive materials do: part id → face group → texture index, stored
    /// on the instance. Nothing is duplicated and nothing is re-meshed — a glazed panel is the
    /// same geometry with a see-through material bound to it.
    pub(super) fn door_bind_materials(&mut self, fi: usize) {
        use crate::door_mat::Slot;
        // One texture per DISTINCT material, reused by name — see `DoorMaterial::install`.
        let mut by_part: HashMap<u32, usize> = HashMap::new();
        for slot in Slot::ALL {
            let ti = self.door_mats.get(slot).install(&mut self.factory);
            for &p in slot.parts() {
                by_part.insert(p, ti);
            }
        }
        // Ironmongery — hinges, the door's own lever, and every part a welded handle contributed —
        // takes the handle FINISH, so the whole door is materialled and nothing falls back to the
        // asset's base colour.
        let hw = self.door_hardware_texture();
        let Some(inst) = self.factory.furniture.get(fi) else {
            return;
        };
        let map = self.factory.furniture_lib.get(inst.asset).map(|a| {
            let g = a.group_geom();
            let mut m: HashMap<u32, usize> = HashMap::new();
            for t in 0..a.positions.len() / 3 {
                let part = a.part_ids.get(t).copied().unwrap_or(0);
                let ti = by_part
                    .get(&part)
                    .copied()
                    .or_else(|| (part >= crate::door_mat::FIRST_HARDWARE_PART).then_some(hw));
                if let Some(ti) = ti {
                    m.insert(g.face[t], ti);
                }
            }
            m
        });
        if let (Some(m), Some(inst)) = (map, self.factory.furniture.get_mut(fi)) {
            inst.surface_texture = m;
        }
    }

    /// A swatch of the current handle finish, reused by name. Shares [`finish_look`] with the
    /// preview so the ironmongery is the same colour in both.
    fn door_hardware_texture(&mut self) -> usize {
        let name = format!(
            "Door — hardware ({})",
            if self.handle_finish.is_empty() {
                "satin nickel"
            } else {
                &self.handle_finish
            }
        );
        if let Some(i) = self.factory.textures.iter().position(|t| t.name == name) {
            return i;
        }
        let look = finish_look(&self.handle_finish);
        // `finish_look` is authored in LINEAR reflectance; a swatch stores sRGB bytes.
        let px = |c: f32| {
            (crate::color::linear_to_srgb(c) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        let rgb = [px(look.albedo[0]), px(look.albedo[1]), px(look.albedo[2])];
        let i = self
            .factory
            .add_texture(name, 1, 1, vec![rgb[0], rgb[1], rgb[2], 255]);
        if let Some(t) = self.factory.textures.get_mut(i) {
            t.metallic = look.metallic;
            t.roughness = look.roughness;
            // A polished lever mirrors the room and a matt-black one does not — but that is what
            // ROUGHNESS says, and the shader already reads it. `reflect` stays at its 1.0 default.
        }
        i
    }

    /// The finished door — leaf, lining, casing and whichever handle it wears — as one mesh.
    /// Shared by the preview and the build so they cannot drift apart.
    pub(super) fn door_mesh(
        &mut self,
        inp: &cad_solid::door::DoorInput,
    ) -> Result<
        (
            cad_solid::door::DoorMetrics,
            crate::mesh_io::ObjMesh,
            Vec<u32>,
            String,
        ),
        String,
    > {
        let (resolved, choice) = self.door_resolved(inp);
        let (m, mesh) = cad_solid::door::build(&resolved).map_err(|e| e.to_string())?;
        let mut part_ids = mesh.face_ids.clone();
        let mut obj = crate::mesh_io::ObjMesh {
            positions: mesh.positions,
            normals: mesh.normals,
            color: Some([0.62, 0.50, 0.38]), // wood; recolour a piece via Textures
            alpha: Vec::new(),
        };
        // WELD THE HANDLE INTO THE LEAF. A door and its hardware are one thing: you erase it, move
        // it or copy it as a unit, and a handle left as its own instance is something the user has
        // to chase around separately — and can delete by accident, leaving a door with a hole where
        // its lever was. `mount_matrix` already works in the door's OWN frame, which is the frame
        // this mesh is built in, so the handle's vertices transform straight in with no parenting
        // and no second instance.
        let note = self.factory_weld_handle(&choice, &mut obj, &mut part_ids);
        Ok((m, obj, part_ids, note))
    }

    /// The handle library folder. Ships beside the Blender assets; absent on a bare checkout, in
    /// which case the picker says so rather than pretending the library is empty.
    /// The bundled handle library.
    ///
    /// This was an absolute path into a personal working folder on a G: drive, so the door-handle
    /// picker was empty on every machine but one — and silently, because a missing library just
    /// means "no handles to offer". The library now ships in `assets/handles` and resolves
    /// relative to the executable like everything else.
    fn handle_lib_dir() -> std::path::PathBuf {
        crate::assets::path("assets/handles")
    }

    /// Decode a handle's preview PNG into an egui texture, once. The tiles are 640 square with an
    /// alpha channel — rendered against nothing, so they sit on any panel background in either
    /// theme.
    fn handle_preview(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        path: &std::path::Path,
    ) -> Option<egui::TextureHandle> {
        if let Some(t) = self.handle_tex.get(id) {
            return Some(t.clone());
        }
        let bytes = std::fs::read(path).ok()?;
        let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let tex = ctx.load_texture(
            format!("handle_{id}"),
            egui::ColorImage::from_rgba_unmultiplied([w, h], img.as_raw()),
            egui::TextureOptions::LINEAR,
        );
        self.handle_tex.insert(id.to_string(), tex.clone());
        Some(tex)
    }

    /// The handle picker (HANDLES_BUILD.md section B5).
    ///
    /// Tiles are drawn at HONEST RELATIVE SCALE: the smart lock's plate is seven times the height
    /// of the round rose, and a picker that normalises every tile to the same box misleads exactly
    /// the user who has to fit one to a door. A handle that fails validation is GREYED OUT with the
    /// reason on hover, never hidden — someone who picked the smart lock and cannot see why it
    /// vanished will assume the library is broken.
    fn handle_picker(&mut self, ui: &mut egui::Ui, door: &cad_solid::door::DoorInput) {
        if self.handle_lib.is_none() {
            match crate::handles::HandleLibrary::load(Self::handle_lib_dir()) {
                Ok(l) => self.handle_lib = Some(l),
                Err(e) => {
                    ui.label(
                        egui::RichText::new(format!("handle library unavailable — {e}"))
                            .small()
                            .weak(),
                    );
                    return;
                }
            }
        }
        let Some(lib) = self.handle_lib.clone() else {
            return;
        };
        let fit = crate::handles::DoorFit {
            door_width_mm: door.door_width * 1000.0,
            door_height_mm: door.door_height * 1000.0,
            leaf_thickness_mm: door.door_thickness * 1000.0,
            handle_backset_mm: door.handle_backset * 1000.0,
            handle_height_mm: door.handle_height * 1000.0,
            hinge_side: door.hinge_side,
        };
        if self.handle_sel.is_empty() {
            self.handle_sel = lib
                .handles
                .first()
                .map(|h| h.id.clone())
                .unwrap_or_default();
        }

        ui.label(egui::RichText::new("Handle").small().weak());
        // The tallest plate sets the scale for every tile, so their sizes stay comparable.
        let tallest = lib
            .handles
            .iter()
            .map(|h| h.bbox_mm.max[1] - h.bbox_mm.min[1])
            .fold(1.0f32, f32::max);
        const TILE: f32 = 78.0;

        let mut styles: Vec<String> = lib.handles.iter().map(|h| h.style.clone()).collect();
        styles.dedup();
        let ctx = ui.ctx().clone();
        let mut picked: Option<String> = None;
        for style in styles {
            ui.label(egui::RichText::new(&style).small().weak());
            ui.horizontal_wrapped(|ui| {
                for h in lib.handles.iter().filter(|h| h.style == style) {
                    let f = crate::handles::fitness(h, &fit);
                    let usable = f.ok();
                    let tex = self.handle_preview(&ctx, &h.id, &lib.preview_path(h));
                    let selected = self.handle_sel == h.id;
                    ui.vertical(|ui| {
                        ui.set_width(TILE);
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(TILE, TILE), egui::Sense::click());
                        if selected {
                            ui.painter().rect_filled(
                                rect,
                                3.0,
                                egui::Color32::from_rgb(38, 74, 102),
                            );
                        }
                        if let Some(t) = &tex {
                            // Honest scale: each tile is drawn at its true height relative to the
                            // tallest handle in the library, floored so a rose is still visible.
                            let hmm = h.bbox_mm.max[1] - h.bbox_mm.min[1];
                            let art = TILE * (hmm / tallest).clamp(0.3, 1.0);
                            let r =
                                egui::Rect::from_center_size(rect.center(), egui::vec2(art, art));
                            let tint = if usable {
                                egui::Color32::WHITE
                            } else {
                                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 55)
                            };
                            ui.painter().image(
                                t.id(),
                                r,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                tint,
                            );
                        }
                        let label = egui::RichText::new(&h.name).small();
                        ui.label(if usable { label } else { label.weak() });
                        // The two numbers that decide whether a handle fits a doorway at all.
                        ui.label(
                            egui::RichText::new(format!(
                                "{:.0} out · {:.0} reach",
                                h.projection_mm, h.lever_reach_mm
                            ))
                            .small()
                            .weak(),
                        );
                        let tip = if usable {
                            if f.warnings.is_empty() {
                                format!(
                                    "{}\n{} mount · min backset {:.0} mm",
                                    h.name, h.mount, h.min_backset_mm
                                )
                            } else {
                                format!("{}\n⚠ {}", h.name, f.summary())
                            }
                        } else {
                            format!("{} — unavailable\n{}", h.name, f.summary())
                        };
                        if resp.clicked() && usable {
                            picked = Some(h.id.clone());
                        }
                        resp.on_hover_text(tip);
                    });
                }
            });
        }
        if let Some(id) = picked {
            // Changing the handle must not rebuild the door — this swaps which handle is mounted.
            self.handle_sel = id;
        }

        // Finish is a SECOND, independent control, preserved across handle changes where the new
        // handle offers the same one.
        if let Some(h) = lib.get(&self.handle_sel) {
            if self.handle_finish.is_empty() || !h.finishes.contains(&self.handle_finish) {
                self.handle_finish = h.default_finish.clone();
            }
            if !h.finishes.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_sized([150.0, 18.0], egui::Label::new("Finish").selectable(false));
                    egui::ComboBox::from_id_salt("handle_finish")
                        .selected_text(self.handle_finish.replace('_', " "))
                        .show_ui(ui, |ui| {
                            for fin in &h.finishes {
                                ui.selectable_value(
                                    &mut self.handle_finish,
                                    fin.clone(),
                                    fin.replace('_', " "),
                                );
                            }
                        });
                });
            }
            for w in &crate::handles::fitness(h, &fit).warnings {
                ui.label(
                    egui::RichText::new(format!("  ⚠ {}", w.message))
                        .small()
                        .weak(),
                );
            }
        }
    }

    /// Build the parametric door as a furniture ASSET (no placement) and return its library index.
    /// Each structural component (leaf / lining / casing / hardware) is tagged via `part_ids`, so
    /// "select a piece" picks one part and it can be recoloured on its own. Shared by the default
    /// aperture door, the fitted (drawn) door, and the parameters dialog.
    fn factory_add_door_asset(
        &mut self,
        inp: &cad_solid::door::DoorInput,
        name: &str,
    ) -> Option<usize> {
        // Through `door_mesh`, so a door DRAWN on a wall wears the same handle as one built from
        // the dialog. Two routes to the same object should not produce two different objects.
        let (_m, obj, part_ids, _note) = match self.door_mesh(inp) {
            Ok(v) => v,
            Err(e) => {
                self.factory.status = format!("Door: {e}");
                return None;
            }
        };
        let idx = self.factory.add_furniture_asset(name.to_string(), obj);
        if let Some(a) = self.factory.furniture_lib.get_mut(idx) {
            a.alpha_resolved = true;
            if part_ids.len() == a.positions.len() / 3 {
                a.part_ids = part_ids;
            }
        }
        Some(idx)
    }

    /// Re-derive glass transparency on the bundled APERTURE window after a load. A project saved
    /// before the transparency feature has its "Window" asset stored WITHOUT per-vertex alpha, and
    /// because apertures are reused BY NAME ([`Self::factory_ensure_aperture_asset`]), even a newly
    /// drawn window would inherit that stale opaque asset — so the glass fix would appear not to
    /// work on any pre-existing project. Re-parse the bundled source (deterministic, identical
    /// vertex order) and copy its alpha onto the loaded asset when the vertex counts line up; this
    /// repairs every existing window instance AND every future placement at once. Best-effort:
    /// silently skipped if the file can't be read or the geometry no longer matches.
    pub(super) fn refresh_aperture_transparency(&mut self) {
        use crate::factory::ApertureKind;
        // Only the window carries glass; the door mesh is opaque (and FBX alpha isn't parsed).
        let kind = ApertureKind::Window;
        let path = kind.asset_file();
        let path = &path.to_string_lossy().into_owned();
        if !path.to_ascii_lowercase().ends_with(".obj") {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let parsed = crate::mesh_io::parse_obj_dir(&text, std::path::Path::new(path).parent());
        if parsed.alpha.is_empty() {
            return; // bundled window declares no transparency — nothing to copy
        }
        let label = kind.label();
        let mut fixed = 0usize;
        for a in self.factory.furniture_lib.iter_mut() {
            if a.name == label && !a.is_translucent() && a.positions.len() == parsed.positions.len()
            {
                a.alpha = parsed.alpha.clone();
                fixed += 1;
            }
        }
        if fixed > 0 {
            self.history.push(format!(
                "  aperture window: recovered glass transparency on {fixed} loaded asset(s)"
            ));
        }
        self.refresh_imported_furniture_transparency();
    }

    /// Re-derive glass transparency for IMPORTED furniture from its stored source file, once, for a
    /// project saved before the alpha was persisted. Only meshes with a known `source_path` in a
    /// format our parser reads transparency from (OBJ/glTF) are re-parsed, and only while their
    /// alpha is still unresolved — so a heavy opaque piece is never re-parsed on every load.
    /// Best-effort: a moved/deleted source is simply left for a later load. Furniture imported by
    /// a build that predates `source_path` has none stored and can only be fixed by re-importing.
    fn refresh_imported_furniture_transparency(&mut self) {
        let mut fixed = 0usize;
        for a in self.factory.furniture_lib.iter_mut() {
            if a.alpha_resolved {
                continue;
            }
            let Some(path) = a.source_path.clone() else {
                continue;
            };
            let lower = path.to_ascii_lowercase();
            let dir = std::path::Path::new(&path).parent();
            let parsed = if lower.ends_with(".obj") {
                std::fs::read_to_string(&path)
                    .ok()
                    .map(|t| crate::mesh_io::parse_obj_dir(&t, dir))
            } else if lower.ends_with(".glb") || lower.ends_with(".gltf") {
                std::fs::read(&path)
                    .ok()
                    .map(|b| crate::mesh_io::parse_gltf_ex(&b, dir).0)
            } else {
                // FBX/3DS transparency isn't parsed — nothing to re-derive; don't retry it.
                a.alpha_resolved = true;
                continue;
            };
            let Some(m) = parsed else { continue }; // unreadable now → retry on a later load
            if !m.alpha.is_empty() && m.positions.len() == a.positions.len() && !a.is_translucent()
            {
                a.alpha = m.alpha;
                fixed += 1;
            }
            a.alpha_resolved = true; // parsed OK → authoritative, don't re-parse next load
        }
        if fixed > 0 {
            self.history.push(format!(
                "  furniture: recovered glass transparency on {fixed} imported asset(s) from source"
            ));
        }
    }

    /// IMPORT a door/window as a free-standing piece at the model centre (no cut). The user then
    /// positions it manually — the counterpart to the "draw an aperture" flow that cuts + fits.
    fn factory_import_aperture(&mut self, kind: crate::factory::ApertureKind) {
        let Some(idx) = self.factory_ensure_aperture_asset(kind) else {
            return;
        };
        self.snapshot_factory();
        let at = self.factory.place_at();
        self.factory.place_furniture(idx, at); // uniform scale, user moves/sizes it
        self.factory.open = true;
        self.factory.status = format!("{} imported — move it into place", kind.label());
        self.history
            .push(format!("  {} imported (free-standing)", kind.label()));
    }

    /// DRAW AN APERTURE: turn the rectangle the user drew on a wall face into a real opening —
    /// cut it through the wall AND drop a door/window scaled to fill it exactly. Reuses the
    /// existing through-cut, then places the aperture with [`crate::factory::FactoryState::place_aperture`].
    /// Everything (cut + placed mesh) reverts in ONE undo because `factory_cut_sketch` snapshots
    /// the whole factory (furniture included) before it runs.
    fn factory_draw_aperture(&mut self, kind: crate::factory::ApertureKind) {
        // 1. The drawn rectangle + its wall plane.
        let Some((frame, loops, _, _)) = self.factory_resolve_extrude() else {
            self.factory.status =
                "draw a rectangle on a wall face first, then place the door/window".into();
            return;
        };
        let loop0 = &loops[0];
        if loop0.len() < 3 {
            self.factory.status = "the opening outline needs at least 3 points".into();
            return;
        }
        let n = frame.normal();
        // World corners of the drawn outline.
        let corners: Vec<glam::Vec3> = loop0.iter().map(|p| frame.from_uv(*p)).collect();
        // Opening axes, derived from the WALL (not the sketch basis, which may be rotated):
        //  - vertical = world-up projected into the wall plane (exact +Z for a vertical wall)
        //  - horizontal = wall-normal × vertical (level, in-plane)
        let up = glam::Vec3::Z - n * glam::Vec3::Z.dot(n);
        let vert = if up.length_squared() > 1e-6 {
            up.normalize()
        } else {
            frame.v.normalize_or_zero()
        };
        let mut u_h = n.cross(vert);
        u_h = if u_h.length_squared() < 1e-9 {
            frame.u.normalize_or_zero()
        } else {
            u_h.normalize()
        };
        // Extents of the outline along those axes → width, height + the in-plane centre.
        let (mut umin, mut umax) = (f32::MAX, f32::MIN);
        let (mut vmin, mut vmax) = (f32::MAX, f32::MIN);
        for w in &corners {
            let du = w.dot(u_h);
            let dv = w.dot(vert);
            umin = umin.min(du);
            umax = umax.max(du);
            vmin = vmin.min(dv);
            vmax = vmax.max(dv);
        }
        let width = umax - umin;
        let height = vmax - vmin;
        if width < 1e-3 || height < 1e-3 {
            self.factory.status = "opening is too small to place a door/window".into();
            return;
        }
        let d_n = corners[0].dot(n); // plane offset (all corners share it)
        let center_face = u_h * ((umin + umax) * 0.5) + vert * ((vmin + vmax) * 0.5) + n * d_n;
        // Wall thickness at the opening: probe the solid along the normal both ways; the side that
        // runs INTO the wall gives the full thickness (the other exits immediately).
        let din = self.assembly_span(center_face, -n).0;
        let dout = self.assembly_span(center_face, n).0;
        let (thick, inward) = if din >= dout { (din, -n) } else { (dout, n) };
        let thick = thick.max(0.05);
        let center_mid = center_face + inward * (thick * 0.5);

        // 2. Cut the opening THROUGH the wall (this snapshots the whole factory for undo). Do this
        //    BEFORE building any bespoke asset, so undo cleanly removes the cut, the asset and the
        //    placement together.
        let made = self.factory_cut_sketch(true);
        if made == 0 {
            self.factory.status =
                "couldn't cut an opening here — draw the rectangle on a wall face".into();
            return;
        }
        // 3. Drop the door/window into the opening.
        if kind == crate::factory::ApertureKind::Door {
            // Build a PARAMETRIC door FITTED to this opening: its structural opening (door +
            // 2·frame_face_width) equals the cut hole, so the lining beds into the reveal and the
            // casing overhangs the wall face — mouldings and hardware at their true (unstretched)
            // proportions. Placed at native size, not stretched.
            let d = cad_solid::door::DoorInput::default();
            let inp = cad_solid::door::DoorInput {
                door_width: (width - 2.0 * d.frame_face_width).max(0.30),
                door_height: (height - d.frame_face_width - d.door_gap_bottom).max(0.50),
                frame_depth: thick.max(d.door_thickness + 0.005),
                ..d
            };
            if let Some(asset) = self.factory_add_door_asset(&inp, "Door") {
                // The door's structural-opening centre, mid-wall, lands on the opening centre.
                let anchor = glam::Vec3::new(0.0, -thick * 0.5, height * 0.5);
                if let Some(fi) = self
                    .factory
                    .place_aperture_native(asset, center_mid, u_h, anchor)
                {
                    // A door drawn on a wall wears the same materials as one built from the panel.
                    self.door_bind_materials(fi);
                }
            }
        } else {
            let Some(asset) = self.factory_ensure_aperture_asset(kind) else {
                return;
            };
            // Window: stretch the bundled mesh to fill the opening exactly.
            self.factory
                .place_aperture(asset, center_mid, u_h, width, height, thick);
        }
        // 5. Leave the sketch and show the result in 3D.
        self.factory_exit_sketch();
        self.active_view = ActiveView::ThreeD;
        self.factory.status = format!(
            "{} placed — {:.2} × {:.2} m opening",
            kind.label(),
            width,
            height
        );
        self.history.push(format!(
            "  {} aperture: cut + placed ({:.2}×{:.2} m, wall {:.2} m)",
            kind.label(),
            width,
            height,
            thick
        ));
    }

    /// Paint the 2D drawing on the ground plane as a reference underlay, projected to
    /// screen and drawn on a FOREGROUND layer so it reads THROUGH the solids — a plan
    /// hidden behind the model is useless. Every dobject goes through
    /// `cad_solid::geom_outlines`, the same flattener the build tools use, so what you see
    /// is exactly what they will consume. Muted blue = reference, not built geometry.
    fn paint_plan_underlay(&self, ui: &egui::Ui, rect: egui::Rect, mvp: &[f32; 16]) {
        // X-RAY ONLY. By default the plan goes in with the depth-tested scene lines instead,
        // so a wall in front of it hides it. See the call site in the 3D render.
        // THE PLAN, NOT WHATEVER IS ON THE CANVAS — the same hazard as the depth-tested branch.
        // While a face sketch is open `self.doc` IS the sketch, so this drew the sketch a second
        // time, flat on the ground, in the x-ray path too.
        let plan = Self::plan_doc_of(self.factory.session.as_ref(), &self.doc);
        if !self.factory.show_plan || !self.factory.plan_xray || plan.dobjects.is_empty() {
            return;
        }
        // Clipped to the viewport — a foreground layer otherwise spans the WHOLE window,
        // so projected plan lines would bleed into the 2D canvas on the left.
        let fg = ui
            .ctx()
            .layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("factory_plan_underlay"),
            ))
            .with_clip_rect(rect);
        let stroke = egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 175, 225, 160),
        );
        for d in &plan.dobjects {
            // METRES: this projects into the 3D view's world space, so it must be scaled the
            // same way the build tools scale it — otherwise a millimetre plan draws its
            // reference kilometres away from the model it is meant to sit under.
            //
            // BY THE PLAN'S OWN UNITS. `outlines_m` goes through `doc_k()`, which reads
            // `self.doc.units` — the SKETCH's while a session is open, and a sketch document is
            // `Document::default()` at 1.0 m/unit. So a millimetre plan (k = 0.001) drew this
            // underlay at 1000×: the comment directly above describes that failure, and the code
            // walked into it anyway.
            //
            // This is the second half of a fix that was made an hour earlier and corrected only
            // WHICH DOCUMENT. The sibling branch is right for a structural reason —
            // `FactoryState::plan_lines` takes `k` from the document it is handed, so it cannot
            // disagree with itself. This one took the document from a local and the scale from a
            // global, which is precisely how the two came apart.
            let k = plan.units.metres_per_unit;
            // The DISPLAY flattener, like the depth-tested `plan_lines` it is the alternative to.
            // The two draw the same plan by two routes, so a spline or a block missing from one
            // and present in the other would read as a rendering bug.
            for path in cad_solid::geom_display_outlines_scaled(&d.geom, plan, k) {
                for w in path.windows(2) {
                    let a = crate::factory::world_to_screen(
                        glam::Vec3::new(w[0].x, w[0].y, 0.0),
                        rect,
                        mvp,
                    );
                    let b = crate::factory::world_to_screen(
                        glam::Vec3::new(w[1].x, w[1].y, 0.0),
                        rect,
                        mvp,
                    );
                    if let (Some(a), Some(b)) = (a, b) {
                        fg.line_segment([a, b], stroke);
                    }
                }
            }
        }
    }

    /// The 3D Factory PROPERTIES panel — shown in the right-hand panel when something is
    /// selected. Every field is type-in editable (`DragValue`), and every edit is one
    /// undo step. Nothing shows when the selection is empty, so the panel is quiet until
    /// it has something to say.
    fn factory_properties_panel(&mut self, ui: &mut egui::Ui) {
        // Lengths below are stored in metres and typed in the Factory working unit.
        let ufu = self.factory.units.clone();
        if !self.factory.has_any_selection() {
            ui.label(
                egui::RichText::new("Select an object to edit its properties.")
                    .small()
                    .weak(),
            );
            return;
        }
        ui.separator();

        // A selected FURNITURE instance: position, scale, rotation, colour, delete.
        if let Some(fi) = self.factory.sel_furn_primary() {
            if let Some(inst) = self.factory.furniture.get(fi).cloned() {
                let name = self
                    .factory
                    .furniture_lib
                    .get(inst.asset)
                    .map(|a| a.name.clone())
                    .unwrap_or_default();
                ui.label(egui::RichText::new(format!("Furniture · {name}")).strong());
                let mut pos = inst.pos;
                let mut scale = inst.scale;
                let mut rot = inst.rot;
                // Three side-by-side columns so the wide panel isn't wasted: placement on
                // the left, rotation in the middle, colour/texture on the right. `columns`
                // splits the panel's ACTUAL width three ways — no fixed min-widths that
                // could overflow the dock and collapse the viewport below it.
                ui.columns(3, |cols| {
                    // Column 1 — Position + uniform scale.
                    let ui = &mut cols[0];
                    ui.label(egui::RichText::new("Position").small().weak());
                    for (axis, lbl) in [(0usize, "X"), (1, "Y"), (2, "Z")] {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [36.0, 18.0],
                                egui::Label::new(egui::RichText::new(lbl).small().weak()),
                            );
                            let r = crate::factory::length_ui(
                                ui,
                                &ufu,
                                &mut pos[axis],
                                0.02,
                                -1e5,
                                1e5,
                                &self.calc,
                            );
                            if r.drag_started() || (r.changed() && !r.dragged()) {
                                self.snapshot_factory();
                            }
                            if r.changed() {
                                self.factory.furniture[fi].pos[axis] = pos[axis];
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [36.0, 18.0],
                            egui::Label::new(egui::RichText::new("scale").small().weak()),
                        );
                        let r = ui.add(
                            egui::DragValue::new(&mut scale)
                                .update_while_editing(false)
                                .speed(0.01)
                                .range(0.01..=100.0),
                        );
                        if r.drag_started() || (r.changed() && !r.dragged()) {
                            self.snapshot_factory();
                        }
                        if r.changed() {
                            self.factory.furniture[fi].scale = scale;
                        }
                    });

                    // Column 2 — Rotation about world X/Y/Z.
                    let ui = &mut cols[1];
                    ui.label(egui::RichText::new("Rotation (°)").small().weak());
                    for (axis, lbl) in [(0usize, "X"), (1, "Y"), (2, "Z")] {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [36.0, 18.0],
                                egui::Label::new(egui::RichText::new(lbl).small().weak()),
                            );
                            let r = ui.add(
                                egui::DragValue::new(&mut rot[axis])
                                    .update_while_editing(false)
                                    .speed(1.0)
                                    .suffix("°"),
                            );
                            if r.drag_started() || (r.changed() && !r.dragged()) {
                                self.snapshot_factory();
                            }
                            if r.changed() {
                                self.factory.furniture[fi].rot[axis] = rot[axis];
                            }
                        });
                    }

                    // Column 3 — Colour / texture swatches.
                    self.factory_color_section(&mut cols[2]);
                });
                self.factory_delete_button(ui);
                return;
            }
        }

        // A single selected WALL: its own live height + thickness (in-place edits).
        if let Some(fid) = self.factory.selected_single() {
            if let Some(wi) = self.factory.wall_index(fid) {
                ui.label(egui::RichText::new("Wall").strong());
                let mut h = self.factory.walls[wi].height;
                let mut t = self.factory.walls[wi].thickness;
                // Two columns: dimensions on the left, colour/texture on the right.
                ui.columns(2, |cols| {
                    let ui = &mut cols[0];
                    ui.label(egui::RichText::new("Dimensions").small().weak());
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [56.0, 18.0],
                            egui::Label::new(egui::RichText::new("height").small().weak()),
                        );
                        let r = crate::factory::length_ui(
                            ui, &ufu, &mut h, 0.02, 0.05, 100.0, &self.calc,
                        );
                        if r.drag_started() || (r.changed() && !r.dragged()) {
                            self.snapshot_factory();
                        }
                        if r.changed() {
                            self.factory.set_wall_height(fid, h);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [56.0, 18.0],
                            egui::Label::new(egui::RichText::new("thickness").small().weak()),
                        );
                        let r = crate::factory::length_ui(
                            ui, &ufu, &mut t, 0.01, 0.02, 5.0, &self.calc,
                        );
                        if r.drag_started() || (r.changed() && !r.dragged()) {
                            self.snapshot_factory();
                        }
                        if r.changed() {
                            self.factory.set_wall_thickness(fid, t);
                        }
                    });
                    self.factory_color_section(&mut cols[1]);
                });
                self.factory_delete_button(ui);
                return;
            }
        }

        // A single selected SOLID: position + dimensions.
        if let Some((id, mut prim, origin)) = self.factory.selected_primitive() {
            ui.label(egui::RichText::new(prim.kind_label()).strong());
            // Three columns to match the furniture panel: placement + size + scale on the
            // left, rotation in the middle, colour/texture on the right.
            ui.columns(3, |cols| {
                // Column 0 — Position, Dimensions, uniform scale.
                let ui = &mut cols[0];
                ui.label(egui::RichText::new("Position").small().weak());
                for (axis, name) in [(0usize, "X"), (1, "Y"), (2, "Z")] {
                    let mut v = origin[axis];
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [36.0, 18.0],
                            egui::Label::new(egui::RichText::new(name).small().weak()),
                        );
                        let r = crate::factory::length_ui(
                            ui, &ufu, &mut v, 0.02, -1e5, 1e5, &self.calc,
                        );
                        if r.drag_started() || (r.changed() && !r.dragged()) {
                            self.snapshot_factory();
                        }
                        if r.changed() {
                            self.factory.set_feature_origin_axis(id, axis, v);
                        }
                    });
                }
                ui.label(egui::RichText::new("Dimensions").small().weak());
                // The dimension editor reports whether anything changed; snapshot on the
                // first change of an interaction only (a held drag keeps the same undo step).
                if crate::factory::primitive_dim_fields(ui, ufu, &mut prim, &self.calc) {
                    if !self.factory.dim_edit_active {
                        self.snapshot_factory();
                        self.factory.dim_edit_active = true;
                    }
                    self.factory.set_feature_primitive(id, prim);
                } else if !ui.ctx().input(|i| i.pointer.any_down()) {
                    // Interaction finished — the next edit starts a fresh undo step.
                    self.factory.dim_edit_active = false;
                }
                // Uniform scale about the object's centre — a quick "make it bigger/smaller".
                ui.label(egui::RichText::new("scale ×").small().weak());
                ui.horizontal(|ui| {
                    if ui.small_button("−10%").clicked() {
                        self.snapshot_factory();
                        self.factory.scale_selection(0.9);
                    }
                    if ui.small_button("+10%").clicked() {
                        self.snapshot_factory();
                        self.factory.scale_selection(1.1);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.small_button("×2").clicked() {
                        self.snapshot_factory();
                        self.factory.scale_selection(2.0);
                    }
                    if ui.small_button("÷2").clicked() {
                        self.snapshot_factory();
                        self.factory.scale_selection(0.5);
                    }
                });

                // Column 1 — Rotation about the object's LOCAL axes (pitch/roll/spin), the
                // same values the rotation-ring gizmo writes. Degrees.
                if let Some(mut rot) = self.factory.feature_rotation(id) {
                    let ui = &mut cols[1];
                    ui.label(egui::RichText::new("Rotation (°)").small().weak());
                    for (axis, lbl) in [(0usize, "pitch"), (1, "roll"), (2, "spin")] {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [40.0, 18.0],
                                egui::Label::new(egui::RichText::new(lbl).small().weak()),
                            );
                            let r = ui.add(
                                egui::DragValue::new(&mut rot[axis])
                                    .update_while_editing(false)
                                    .speed(1.0)
                                    .suffix("°"),
                            );
                            if r.drag_started() || (r.changed() && !r.dragged()) {
                                self.snapshot_factory();
                            }
                            if r.changed() {
                                self.factory.set_feature_rotation(id, axis, rot[axis]);
                                self.factory.recompute();
                            }
                        });
                    }
                }

                // Column 2 — Colour / texture swatches.
                self.factory_color_section(&mut cols[2]);
            });
            self.factory_delete_button(ui);
            return;
        }

        // Multiple objects: no single dimension set — offer position nudge + delete.
        ui.label(
            egui::RichText::new(format!("{} objects selected", self.factory.selection.len()))
                .strong(),
        );
        self.factory_delete_button(ui);
    }

    /// Duplicate + delete buttons for the current 3D selection (with keyboard-shortcut hints).
    fn factory_delete_button(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .button("⧉ Duplicate")
                .on_hover_text("Make an offset copy of the selected object — furniture, an extruded solid, or a door/window. Also: Ctrl+C then Ctrl+V.")
                .clicked()
            {
                self.factory_duplicate_selection();
            }
            if ui
                .button(egui::RichText::new("🗑 Delete").color(egui::Color32::from_rgb(240, 150, 150)))
                .on_hover_text("Remove the selected object(s). Shortcut: Delete key.")
                .clicked()
            {
                self.factory_delete_selection();
            }
        });
        // GROUP / EXPLODE — only for CSG solids (a multi-selection or an existing group). A group
        // moves / deletes / selects as one entity; Explode dissolves it back to individual pieces.
        let grouped = self.factory.selection_group().is_some();
        let can_group = self.factory.selection.len() >= 2;
        if grouped || can_group {
            ui.horizontal(|ui| {
                if grouped {
                    if ui.button("💥 Explode")
                        .on_hover_text("Ungroup — release the pieces so each is independent again.")
                        .clicked()
                    {
                        self.snapshot_factory();
                        let n = self.factory.ungroup_selection();
                        self.factory.status = format!("exploded — {n} pieces released");
                        self.history.push(format!("  exploded group ({n} pieces)"));
                    }
                } else if ui.button("🧩 Group")
                    .on_hover_text("Group the selected solids into ONE entity — they then select, move and delete together. Shift-click to multi-select first.")
                    .clicked()
                {
                    self.snapshot_factory();
                    let n = self.factory.group_selection();
                    if n >= 2 {
                        self.factory.status = format!("grouped {n} objects — click any to select all");
                        self.history.push(format!("  grouped {n} objects"));
                    } else {
                        self.undo_stack.pop();
                        self.factory.status = "select 2+ solids (shift-click) to group".into();
                    }
                }
            });
        }
    }

    /// Duplicate the current 3D selection in place (offset copy), whatever its type — an imported
    /// furniture instance, a drawn APERTURE (door/window mesh), or an EXTRUDED solid / any CSG
    /// feature. Copy + paste in one action, so it doesn't depend on the OS clipboard or keyboard
    /// event form. The counterpart of Ctrl+C→Ctrl+V, but always reliable and discoverable.
    fn factory_duplicate_selection(&mut self) {
        if !self.factory.copy_selection() {
            self.factory.status = "select a single 3D object to duplicate".into();
            return;
        }
        self.snapshot_factory();
        match self.factory.paste_clipboard() {
            Some(is_feature) => {
                if is_feature {
                    self.factory.recompute();
                }
                self.factory.status =
                    "duplicated (offset 0.3 m) — drag or arrow-nudge to place".into();
                self.history.push("  duplicated selection".into());
            }
            None => {
                self.undo_stack.pop(); // nothing pasted → discard the snapshot
                self.factory.status = "nothing to duplicate".into();
            }
        }
    }

    /// Delete the current 3D selection — one undo step, then re-evaluate. Shared by the
    /// panel button and the Delete key.
    /// Both kinds go in ONE undo step. A marquee can pick up walls and furniture together, and
    /// splitting that into two steps would mean the user has to press undo twice to put back what
    /// one Delete took away — with the drawing in a half-restored state in between.
    pub(super) fn factory_delete_selection(&mut self) {
        let furn = self.factory.sel_furniture.len();
        let feat = self.factory.selection.len();
        if furn == 0 && feat == 0 {
            return;
        }
        self.snapshot_factory();
        // Furniture first: it is not part of the CSG model, so it needs no re-evaluation and
        // removing it cannot disturb the feature ids about to be erased.
        if furn > 0 {
            self.factory.erase_selected_furniture();
        }
        if feat > 0 {
            self.factory.erase_selection();
            self.factory.recompute();
        }
        let what = match (feat, furn) {
            (0, n) => format!("{n} furniture"),
            (n, 0) => format!("{n} object(s)"),
            (a, b) => format!("{a} object(s) + {b} furniture"),
        };
        self.history.push(format!("  3D Factory: {what} deleted"));
        self.factory_note(format!("{what} deleted"));
    }

    /// The best closed outline in the current 2D selection, for a slab boundary.
    ///
    /// Reuses `cad_solid::geom_outlines` — the SAME flattener promotion uses — so a
    /// rectangle drawn with the polyline tool, a `rectangle`, or an imported plan's
    /// closed polyline all work identically. Picks the LARGEST closed path, since a room
    /// drawn with an inner detail should floor the room, not the detail.
    ///
    /// A wall's own footprint counts too: a room promoted to 3D walls can be floored
    /// without re-selecting the 2D geometry it came from.
    pub(super) fn slab_outline_from_selection(&self) -> Option<Vec<glam::Vec2>> {
        let mut best: Option<(f32, Vec<glam::Vec2>)> = None;
        let mut consider = |path: Vec<glam::Vec2>| {
            if path.len() < 4 {
                return; // a triangle is the smallest ring worth flooring
            }
            let closed = (path[path.len() - 1] - path[0]).length() < 1e-3;
            if !closed {
                return;
            }
            let a = polygon_area(&path).abs();
            if a > 1e-6 && best.as_ref().is_none_or(|(ba, _)| a > *ba) {
                best = Some((a, path));
            }
        };
        let k = self.doc_k();
        for &i in &self.selection {
            let Some(g) = self.doc.dobjects.get(i).map(|d| &d.geom) else {
                continue;
            };
            match g {
                Geom::Wall(w) => {
                    let fp: Vec<glam::Vec2> = w
                        .centerline_polyline(16)
                        .iter()
                        .map(|p| glam::Vec2::new((p.x * k) as f32, (p.y * k) as f32))
                        .collect();
                    consider(fp);
                }
                other => {
                    for path in self.outlines_m(other) {
                        consider(path);
                    }
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// Reset every piece of **doc-relative** interactive state.
    ///
    /// Swapping `self.doc` (enter/leave a sketch) instantly invalidates anything holding
    /// an INDEX into the old document — `selection`, `pre_op_selection`, `selection_prev`,
    /// a half-finished fillet's first pick, a trim basket, a grip drag. The app has ~20
    /// unguarded `self.doc.dobjects[i]` sites, so a stale index is a **panic**, not a
    /// glitch. That was the "Draw on this face" crash: the swap left the seeded drawing's
    /// selection pointing into a document that was no longer there.
    ///
    /// The app has no File>New reset to reuse, so this is the single place that knows the
    /// full list. **If you add a `*_state` field to `CadApp`, add it here too.**
    pub(super) fn factory_reset_doc_state(&mut self) {
        // --- baskets / caches holding dobject INDICES ---
        // This list is the EXHAUSTIVE set of `Option<usize>` / `Vec<usize>` fields on
        // CadApp that index `doc.dobjects`. `selected` is the easy one to miss: it is
        // chained with `selection` when drawing (`selection.iter().chain(self.selected)`),
        // so a stale `selected` panics the canvas on its own.
        self.selection.clear();
        self.selection_prev.clear();
        self.pre_op_selection.clear();
        self.aci_pick_many.clear();
        self.selected = None;
        self.hatch_last_idx = None;
        self.grip_drag = None;
        // command-internal-undo baselines (depth/count, not indices — but they describe
        // the parked document, so they are meaningless against the new one)
        self.cmd_undo_base = None;
        self.cmd_base_objs = None;
        // recorder press-state (holds a hit index + a selection snapshot)
        self.dbg_press_hit = None;
        self.dbg_press_sel.clear();
        // --- in-progress draw buffers ---
        self.tool = Tool::None;
        self.pending.clear();
        self.pending_bulges.clear();
        self.pending_widths.clear();
        self.spline_width_wait = false; // spline width entry (value is sticky)
                                        // --- selection session ---
        self.select_mode = SelectMode::Off;
        self.select_remove_mode = false;
        // --- every modify / inquiry state machine ---
        self.move_state = MoveState::Off;
        self.copy_state = CopyState::Off;
        self.rotate_state = RotateState::Off;
        self.scale_state = ScaleState::Off;
        self.mirror_state = MirrorState::Off;
        self.matchprops_state = MatchPropsState::Off;
        self.paste_state = PasteState::Off;
        self.pedit_state = PeditState::Off;
        self.offset_state = OffsetState::Off;
        self.dist_state = DistState::Off;
        self.area_state = None;
        self.xline_state = XlineState::Off;
        self.boundary_state = BoundaryState::Off;
        self.xref_pending = None;
        self.attr_def_flow = AttrDefFlow::Off;
        self.attedit_state = AttEditState::Off;
        self.centermark_state = CenterMarkState::Off;
        self.ray_state = RayState::Off;
        self.donut_state = DonutState::Off;
        self.wipeout_state = WipeoutState::Off;
        self.layer_pick = LayerPickState::Off;
        self.ptdist_state = PtDistribState::Off;
        self.pick_cycle_index = 0;
        self.pick_cycle_cell = None;
        self.pick_cands.clear();
        self.pick_cycle_at = None;
        self.lengthen_state = LengthenState::Off;
        self.break_state = BreakState::Off;
        self.align_state = AlignState::Off;
        self.stretch_state = StretchState::Off;
        self.fillet_state = FilletState::Off;
        self.chamfer_state = ChamferState::Off;
        self.trim_state = TrimState::Off;
        self.extend_state = ExtendState::Off;
        self.fence_armed = false;
        self.fence_first = None;
        self.insert_state = InsertState::Off;
        self.block_def_state = BlockDefState::Off;
        // --- drafts + flows ---
        self.text_draft = TextDraftState::Off;
        self.dim_draft = DimDraftState::Off;
        self.cmd_flow = None;
        self.zoom_state = ZoomState::Off;
        self.var_set_pending = None;

        // --- DERIVED CACHES KEYED TO THE OLD DOCUMENT ---
        // This is the group that actually caused the "Draw on this face" crash, and it
        // is NOT about selection at all: `index` (the UniformGrid) stores dobject
        // indices. The canvas culls through it (`candidates` → `doc.dobjects[i]`), so a
        // grid built from the model-space doc fed index 0 into an empty sketch doc:
        //   app.rs — `let e = &self.doc.dobjects[i];`
        //   panic: index out of bounds: the len is 0 but the index is 0
        //
        // The field list below mirrors the app's own `clear_all()` ("one-stop wipe
        // everything geometry-related") MINUS its `doc.dobjects.clear()` — we are
        // swapping a document in, not erasing one. **Keep these two in sync.**
        self.index = None;
        self.index_dirty = true;
        self.index_label.clear();
        self.intersections.clear();
        self.last_intersect_label.clear();
        self.snap_override = None;
        self.blockdiff_overlay = None;
        self.param_name_dialog = None;
        self.block_task_rec = None;
        self.touch_view(); // the GPU renderer caches vertex data per dobject
    }

    /// Enter a sketch on `frame`: create the sketch and **swap the app's active document
    /// to it**. From this moment every 2D tool — draw, fillet (R/T/M/P), trim, extend,
    /// offset, chamfer, break, the command line, snaps, layers — operates ON THE PLANE,
    /// complete and unchanged, because they all only know `self.doc`. Nothing is
    /// reimplemented. This is the point of 3D_Factory.
    ///
    /// `undo_stack`/`redo_stack` are `Vec<Document>` snapshots, so they are parked with
    /// the model-space doc and the sketch starts with a clean undo history — otherwise an
    /// undo in the sketch would restore a model-space document over it.
    /// Do two frames lie on the SAME plane (parallel normals, and one origin sits on the
    /// other's plane)? Used to reopen the existing sketch when a face on that plane is picked
    /// again, instead of stacking a new empty sketch each time.
    fn frames_coplanar(a: &cad_solid::Frame, b: &cad_solid::Frame) -> bool {
        let (na, nb) = (a.normal(), b.normal());
        na.dot(nb).abs() > 0.999 && (b.origin - a.origin).dot(na).abs() < 0.05
    }

    /// Bounding box (in the 2D canvas's coordinate system) of the projected face reference,
    /// or `None` if there is nothing to frame.
    fn sketch_ref_bbox(edges: &[[glam::Vec2; 2]]) -> Option<(Vec2, Vec2)> {
        if edges.is_empty() {
            return None;
        }
        let (mut mnx, mut mny, mut mxx, mut mxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for e in edges {
            for p in e {
                mnx = mnx.min(p.x as f64);
                mny = mny.min(p.y as f64);
                mxx = mxx.max(p.x as f64);
                mxy = mxy.max(p.y as f64);
            }
        }
        Some((Vec2::new(mnx, mny), Vec2::new(mxx, mxy)))
    }

    /// Put the 2D canvas on a plane from the view list. `None` = the ground plan.
    ///
    /// It opens the sketch BY INDEX rather than by rebuilding a frame, which is what makes picking
    /// a name out of the list and right-clicking the face itself land in the same place with the
    /// same drawing on it.
    pub(super) fn factory_open_plane(&mut self, which: Option<u32>) {
        match which {
            None => {
                self.factory_exit_sketch();
                self.factory.status =
                    "Global view — the whole model from above, furniture and all.".into();
            }
            Some(id) => {
                let Some(sk) = self.factory.model.sketch_by_id(id) else {
                    self.factory.status = "That plane is gone.".into();
                    return;
                };
                let (frame, name) = (sk.frame, sk.name.clone());
                self.factory_enter_sketch(frame);
                self.factory.status =
                    format!("{name} — every 2D tool draws on this plane, and stays on it.");
            }
        }
    }

    /// Delete a face plane AND the drawing on it. Asked for as "there should be an option to delete
    /// the face".
    ///
    /// Snapshotted, so it is one Ctrl+Z away: this throws work away, and the only safe way to offer
    /// that in a single click is to make it reversible.
    pub(super) fn factory_delete_plane(&mut self, id: u32) {
        let Some(sk) = self.factory.model.sketch_by_id(id) else {
            return;
        };
        let name = sk.name.clone();
        let n = sk.doc.dobjects.len();
        self.snapshot_factory();
        // LEAVE IT FIRST if it is the one open, or the exit would write its drawing back into a
        // plane that is about to be removed — and the session would be left naming nothing.
        //
        // The vector shuffle that used to need patching here is gone: every reference to a plane
        // is an id now, and an id does not move when its neighbour is deleted. What remains is
        // the one real question — was this the plane being edited?
        if self.factory.session.as_ref().is_some_and(|s| s.plane == id) {
            self.factory_exit_sketch();
        }
        self.factory.model.remove_sketch(id);
        // A rename dialog naming this plane no longer has anything to rename. Closed, rather
        // than left open to land on a survivor.
        if self
            .factory
            .rename_plane
            .as_ref()
            .is_some_and(|(k, _)| *k == id)
        {
            self.factory.rename_plane = None;
        }
        self.factory.dirty = true;
        self.factory.status = format!("Deleted {name} and the {n} object(s) drawn on it — Ctrl+Z");
        self.history
            .push(format!("  - plane '{name}' deleted ({n} object(s))"));
    }

    pub(super) fn factory_enter_sketch(&mut self, frame: cad_solid::Frame) {
        // THE PLANE DECIDES THE COORDINATES, NOT THE CLICK. `Frame::from_point_normal` takes the
        // hit point as the origin, so the sketch's (0, 0) landed wherever the cursor happened to
        // be and the face's own corner appeared at minus that offset — "even though the face's
        // corner is on the origin the face is shown as if it's in some other place". See
        // `Frame::canonical` for the rule this replaces it with.
        //
        // The PICK is still kept, because naming needs it: a plane is named after the room the
        // face is in, and that is a question about the point the user touched. The canonical
        // origin is the plane's nearest point to the world origin, which for a wall is a corner
        // and can easily be in a different room — or in none.
        let picked = frame;
        let frame = frame.canonical();
        if self.factory.session.is_some() {
            self.factory_exit_sketch();
        }
        // REOPEN an existing sketch on this same plane rather than starting a blank new one,
        // so returning to a face shows the work already drawn there — otherwise every visit
        // makes a fresh empty sketch and the previous drawing seems to vanish. This is also what
        // makes "when i click on the face again to sketch it should show the same plane" true:
        // clicking a face and picking its name out of the view list land in the SAME sketch.
        let plane = match self
            .factory
            .model
            .sketches
            .iter()
            .find(|sk| Self::frames_coplanar(&sk.frame, &frame))
        {
            Some(sk) => sk.id,
            None => {
                // Name it after the object it is a face of, so it is findable in the view list
                // before anyone gets round to renaming it.
                let mut sk = cad_solid::Sketch::new(frame);
                sk.name = self.factory.sketch_auto_name(&picked);
                self.factory.model.push_sketch(sk)
            }
        };
        let auto = self.factory.sketch_auto_name(&picked);
        let idx = self
            .factory
            .model
            .sketch_index(plane)
            .expect("just resolved or just pushed");
        // A model loaded from a file written before planes had names carries blanks. Fill them in
        // on the way past rather than showing an empty row in the list.
        if self.factory.model.sketches[idx].name.is_empty() {
            self.factory.model.sketches[idx].name = auto;
        }
        // THE FACE, AND ONLY THE FACE. This used to project the WHOLE model onto the plane, which
        // on a wall of a 139 m building draws every wall, room and opening in it flattened on top
        // of each other — "now it looks confusing for the user". An edge lying IN the plane is a
        // boundary of a face standing on that plane, which is what "draw on this face" means.
        self.factory.sketch_ref = self
            .factory
            .frame_face_edges(&self.factory.model.sketches[idx].frame);

        let mut sketch_doc = std::mem::take(&mut self.factory.model.sketches[idx].doc);
        // LAYERS ARE THE PROJECT'S, NOT THE SKETCH'S.
        //
        // Reported as: "i drew some of these arrays in different colors but they showed up in the
        // same color why is that are the layers tool no integrated into this?" They are not — and
        // this is why. `Sketch::new` builds a `Document::default()`, which carries a DEFAULT
        // LayerTable: the project's layers, with the names and colours the user set, live in the
        // plan document and simply do not exist inside a sketch. So everything drawn on a face
        // landed on the base layer, an Array copied a layer id that resolved against a table with
        // one entry in it, and the Layers panel opened during a sketch was editing the SKETCH's
        // empty table rather than the drawing's.
        //
        // A layer is a property of the DRAWING, not of one plane of it. The table is carried in on
        // the way through, and carried back out again on exit so a layer created while sketching is
        // not lost. Ids stay valid because both sides share the one table.
        // AND SO IS EVERY OTHER TABLE — which is what `Document::adopt_tables_from` now says in
        // one place. The block table followed the layers here after someone found the Insert list
        // empty on a plane ("a BlockRef naming a definition that does not exist"); the linetypes,
        // pens, true colours, text styles, dimension styles and wall styles were still default,
        // and would have been found the same way, one complaint at a time. A dobject names all of
        // them BY INDEX, so a Text drawn on a plane against the sketch's default table means
        // something else the moment it is read against the drawing's.
        sketch_doc.adopt_tables_from(&self.doc);
        let saved_doc = std::mem::replace(&mut self.doc, sketch_doc);
        let saved_undo = std::mem::take(&mut self.undo_stack);
        let saved_redo = std::mem::take(&mut self.redo_stack);
        // THE CONSTRAINTS GO WITH THE DOCUMENT THEY CONSTRAIN. `prune_constraints` runs against
        // `self.doc` every frame the parametric panel is up, and a sketch shares no handle with
        // the plan, so leaving them installed deleted the drawing's whole constraint set on the
        // first frame of every sketch. See `SketchSession::saved_constraints`.
        let saved_constraints = std::mem::take(&mut self.parametric.constraints);
        let saved_pending = self.parametric.pending.take();
        // The derived caches describe geometry that is no longer installed; `analyze_doc`
        // recomputes them next frame. `last_solved_sig` is zeroed so the sketch gets a solve of
        // its own rather than matching the plan's signature by accident and skipping one.
        self.parametric.defined.clear();
        self.parametric.dof = 0;
        self.parametric.fully_defined = false;
        self.parametric.redundant = false;
        self.parametric.last_solved_sig = 0;
        self.factory.session = Some(crate::factory::SketchSession {
            plane,
            saved_doc,
            saved_undo,
            saved_redo,
            saved_constraints,
            saved_pending,
        });
        // Entering a plane moves its document out from under `sketch_lines` and installs it as
        // the live one, so the cached finished-plane buffer IS stale here — and nothing is added
        // for it deliberately. `factory_reset_doc_state` below already calls `touch_view` (the
        // spatial index and GPU vertex cache are keyed to the document that just went away), and
        // `lines_sig` hashes the open plane's index on top of that. A mark was added here first
        // and then removed: no test could tell the difference, which is what redundant means.
        // land in a clean drafting state on the new plane. MUST run after the swap:
        // every index-holding field still points into the model-space doc we just
        // parked, and the app panics on a stale `doc.dobjects[i]`.
        self.factory_reset_doc_state();
        // THE SKETCH IS METRE-SPACE BY DESIGN. A face sketch's coordinates ARE the
        // 3D world's (1:1 with the plane's u/v), and the whole 3D side works in
        // metres — so the sketch document always reads 1 unit = 1 metre, regardless
        // of the plan's declared unit (which `adopt_tables_from` deliberately does
        // NOT carry: a sketch that adopted the plan's mm would draw everything
        // 1000x too small and cut nothing).
        self.doc.units =
            cad_kernel::Units::from_metres_per_unit(1.0, cad_kernel::UnitSource::Assumed);
        // Frame the projected object (the sketch starts empty, so `zoom_extents` has nothing
        // to fit) — so you land looking straight at the face's 2D view, not an empty void.
        if let Some((mn, mx)) = Self::sketch_ref_bbox(&self.factory.sketch_ref) {
            self.zoom_fit_bbox(mn, mx, 0.8);
        } else {
            self.zoom_extents();
        }
        self.set_prompt(
            "3D Factory — drafting on the picked plane. FULL 2D toolset (try: f → r → 10). \
             Finish in the 3D Factory panel.",
        );
    }

    /// Close the sketch: put the drawn document back into the model and restore the
    /// model-space document + its undo history.
    pub(super) fn factory_exit_sketch(&mut self) {
        self.factory.sketch_ref.clear();
        // AN UNAPPLIED CUTOUT EDIT PUTS ITS OPENING BACK, whichever way the sketch was left.
        //
        // `factory_edit_cutout` deletes the baked cutters before handing the outline to the 2D
        // canvas, so between there and ✔ Apply the opening exists only in the stash. This used to
        // clear the flag and nothing else — so the Esc key, switching view, or opening a file all
        // closed the sketch with the opening already destroyed, silently. Restoring HERE rather
        // than at each exit is the point: this is the one function every one of them goes through.
        //
        // ✔ Apply takes the stash before re-cutting, so a successful reshape restores nothing.
        if let Some(edit) = self.factory.cutout_edit.take() {
            self.factory_restore_stashed_cutters(edit);
        }
        if let Some(s) = self.factory.session.take() {
            // …AND THE TABLES COME BACK OUT. A layer recoloured, a block defined, a text style
            // added while drawing on a face belongs to the DRAWING, not to that face — see the
            // note on the way in. Cloned BEFORE the swap, because after it `self.doc` is the plan
            // again; and left on the sketch as well, so the ids on the plane still resolve when it
            // is drawn in the plan and in 3D.
            let edited = self.doc.clone_tables_only();
            let sketch_doc = std::mem::replace(&mut self.doc, s.saved_doc);
            self.doc.adopt_tables_from(&edited);
            if let Some(sk) = self.factory.model.sketch_by_id_mut(s.plane) {
                sk.doc = sketch_doc;
            }
            // Leaving one moves the document back the other way, and nothing is added for that
            // here either — same reasons as on the way in. The marks that ARE needed are on the
            // two paths that rewrite a plane's drawing WITHOUT it being the open one:
            // `factory_clear_sketch_on` and `factory_consume_sketch`, where no session index
            // changes and no 2D edit happens, so nothing else would notice.
            // 3D WORK DONE INSIDE A SKETCH IS STILL UNDOABLE.
            //
            // This used to be `self.undo_stack = s.saved_undo`, discarding the whole session
            // stack. That is right for the DOC steps — a sketch has its own 2D history, and it
            // ends with the sketch — and wrong for the FACTORY ones, which are snapshots of the
            // 3D MODEL and outlive it. `factory_extrude_sketch` snapshots, builds the solid, then
            // `factory_consume_sketch` calls this: the operation destroyed its own undo step, so
            // Ctrl+Z afterwards walked back into whatever 2D action preceded entering the sketch
            // while the new solid stayed. That is the flagship flow — draw on a face, Extrude —
            // losing exactly its own history.
            //
            // The model steps are carried out and appended, so the most recent 3D operation is
            // the first thing Ctrl+Z reaches. Each already records the sketch's real contents
            // (see `snapshot_factory`), so undoing one cannot erase the drawing on the face.
            let carried: Vec<UndoStep> = std::mem::take(&mut self.undo_stack)
                .into_iter()
                .filter(|u| matches!(u, UndoStep::Factory(_)))
                .collect();
            self.undo_stack = s.saved_undo;
            self.undo_stack.extend(carried);
            if self.undo_stack.len() > UNDO_STACK_CAP {
                let drop = self.undo_stack.len() - UNDO_STACK_CAP;
                self.undo_stack.drain(0..drop);
            }
            // The redo stack does NOT come out. A redo is only meaningful against the undo history
            // it was made from, and that history has just been replaced.
            self.redo_stack = s.saved_redo;
            // The drawing's constraints come back with the drawing. Anything constrained INSIDE
            // the sketch is dropped here with the rest of the sketch's parametric state — its
            // handles belong to the sketch document, and there is nowhere to keep them: `CRef` is
            // not serialisable and nothing persists constraints across a save even for the plan.
            self.parametric.constraints = s.saved_constraints;
            self.parametric.pending = s.saved_pending;
            self.parametric.defined.clear();
            self.parametric.last_solved_sig = 0;
            // same hazard in reverse: sketch-relative indices must not survive into
            // the model-space document.
            self.factory_reset_doc_state();
            self.clear_prompt();
            self.zoom_extents();
        }
    }

    /// Closed loops in a document, in its (u,v). Only closed shapes (circles, closed
    /// polylines, rectangles) can become a solid; open lines are ignored.
    /// Output is in METRES, scaled by the document's OWN unit — which is what makes this work
    /// for both callers: a face sketch is its own Document (metre-space, k = 1) while the plan
    /// may be millimetres. The `1e-3` closure test is likewise a real 1 mm once scaled, instead
    /// of a metre-shaped epsilon judging drawing units.
    /// GROUP CLOSED LOOPS INTO OUTLINES AND THE HOLES INSIDE THEM.
    ///
    /// Every loop `closed_loops_of` finds used to become its own solid, so a plate drawn with four
    /// bolt circles on it extruded as five posts and the drafter had to cut the holes back out by
    /// hand — four more features, each of which then had to be kept beside the plate for ever.
    ///
    /// EVEN-ODD NESTING, which is the rule every CAD package uses and the only one that gets an
    /// island right: a loop's DEPTH is how many loops contain it, loops at even depth are
    /// outlines, and a loop at odd depth is a hole in its immediate parent. So a washer drawn as
    /// a disc, a bore and a pin in the bore comes out as a washer with a hole and a separate pin —
    /// rather than as a washer with two holes, one of which is solid.
    ///
    /// Containment is tested with the loop's first point, which is ON its own boundary and inside
    /// or outside every OTHER loop unambiguously — the loops here come from distinct drawn
    /// entities, so two of them sharing an edge is a drawing to fix rather than a case to model.
    pub(super) fn nest_loops(loops: Vec<Vec<glam::Vec2>>) -> Vec<(Vec<glam::Vec2>, Vec<Vec<glam::Vec2>>)> {
        let n = loops.len();
        if n < 2 {
            return loops.into_iter().map(|l| (l, Vec::new())).collect();
        }
        let area = |l: &[glam::Vec2]| -> f32 {
            let m = l.len();
            (0..m)
                .map(|i| {
                    let (p, q) = (l[i], l[(i + 1) % m]);
                    p.x * q.y - q.x * p.y
                })
                .sum::<f32>()
                .abs()
                * 0.5
        };
        let areas: Vec<f32> = loops.iter().map(|l| area(l)).collect();
        let inside = |a: usize, b: usize| -> bool {
            // `a` inside `b`. Area first, because a loop cannot contain one at least as big as
            // itself and the check is a comparison rather than a ray cast.
            areas[a] < areas[b]
                && crate::factory::point_in_poly(&loops[b], loops[a][0].x, loops[a][0].y)
        };
        // The SMALLEST loop that contains each one — its immediate parent.
        let parent: Vec<Option<usize>> = (0..n)
            .map(|i| {
                (0..n)
                    .filter(|&j| j != i && inside(i, j))
                    .min_by(|&x, &y| areas[x].total_cmp(&areas[y]))
            })
            .collect();
        let depth = |mut i: usize| -> usize {
            let mut d = 0;
            // Bounded by `n`: a cycle in `parent` is impossible (each step strictly decreases
            // area), but a bound costs nothing and a hang costs the app.
            while let Some(p) = parent[i] {
                d += 1;
                i = p;
                if d > n {
                    break;
                }
            }
            d
        };
        let depths: Vec<usize> = (0..n).map(depth).collect();
        let mut out: Vec<(Vec<glam::Vec2>, Vec<Vec<glam::Vec2>>)> = Vec::new();
        let mut slot: Vec<Option<usize>> = vec![None; n];
        for i in 0..n {
            if depths[i] % 2 == 0 {
                slot[i] = Some(out.len());
                out.push((loops[i].clone(), Vec::new()));
            }
        }
        for i in 0..n {
            if depths[i] % 2 == 1 {
                if let Some(k) = parent[i].and_then(|p| slot[p]) {
                    out[k].1.push(loops[i].clone());
                }
            }
        }
        out
    }

    pub(super) fn closed_loops_of(doc: &cad_kernel::Document) -> Vec<Vec<glam::Vec2>> {
        let mut out = Vec::new();
        let k = doc.units.metres_per_unit;
        for d in &doc.dobjects {
            for path in cad_solid::geom_outlines_scaled(&d.geom, k) {
                if path.len() >= 4 && (path[0] - path[path.len() - 1]).length() < 1e-3 {
                    out.push(path);
                }
            }
        }
        out
    }

    /// Pick a GROUND-PLANE 2D object from a click in the 3D view — so plan geometry shown in
    /// 3D can be selected there and fed to the extrude / cut tools. Prefers the smallest
    /// closed shape CONTAINING the click; else the nearest line within a small tolerance.
    pub(super) fn factory_pick_ground_dobject(
        &self,
        pos: egui::Pos2,
        rect: egui::Rect,
        mvp: &[f32; 16],
    ) -> Option<usize> {
        // NOTHING ON THE GROUND IS PICKABLE WHILE A SKETCH IS OPEN — and this REFUSES rather
        // than redirecting, which is the part worth reading before "fixing" it.
        //
        // The returned index goes straight into `self.selection`, and `self.selection` indexes
        // whatever document is installed. So swapping in `plan_doc()` here — the fix that is
        // right for the two plan underlays, and the obvious thing to pattern-match — would hand
        // back PLAN indices to be stored against the SKETCH: a wrong-document bug traded for an
        // out-of-range one, which in this file is a panic and not a glitch.
        //
        // Unguarded, it ray-cast to world z = 0 and then point-in-polygoned against `self.doc`,
        // i.e. the sketch, treating the plane's (u, v) as world (x, y). Clicking bare floor next
        // to a building cleared the 3D selection, put grips on a shape the user never clicked,
        // and printed "2D shape selected — ▼ Room elements ▸ Extrude / Cut", which was untrue:
        // `factory_resolve_extrude` takes the session branch and ignores that selection entirely.
        //
        // Returning None lets the caller's existing `else if !add` arm deselect, which is the
        // honest answer to a click on empty ground.
        if self.factory.session.is_some() {
            return None;
        }
        // `cursor_on_plane` returns a WORLD (metre) point, so the plan outlines it is compared
        // against must be metres too — otherwise the `0.3` nearest-line tolerance below is
        // metres judging drawing units, and picking a line from the 3D view never fires.
        let w = self.factory.cursor_on_plane(pos, rect, mvp)?;
        let p = glam::Vec2::new(w.x, w.y);
        let mut best_inside: Option<(f32, usize)> = None;
        let mut best_near: Option<(f32, usize)> = None;
        for (i, d) in self.doc.dobjects.iter().enumerate() {
            for path in self.outlines_m(&d.geom) {
                let closed = path.len() >= 4 && (path[0] - path[path.len() - 1]).length() < 1e-3;
                if closed {
                    // even–odd point-in-polygon
                    let (mut inside, n) = (false, path.len());
                    let mut j = n - 1;
                    for k in 0..n {
                        let (a, b) = (path[k], path[j]);
                        if (a.y > p.y) != (b.y > p.y)
                            && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x
                        {
                            inside = !inside;
                        }
                        j = k;
                    }
                    if inside {
                        let mut a2 = 0.0;
                        for k in 0..n {
                            let q = path[(k + 1) % n];
                            a2 += path[k].x * q.y - q.x * path[k].y;
                        }
                        let area = a2.abs() * 0.5;
                        if best_inside.map_or(true, |(ba, _)| area < ba) {
                            best_inside = Some((area, i));
                        }
                    }
                }
                for s in path.windows(2) {
                    let (a, b) = (s[0], s[1]);
                    let ab = b - a;
                    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                    let dist = (a + ab * t - p).length();
                    if best_near.map_or(true, |(bd, _)| dist < bd) {
                        best_near = Some((dist, i));
                    }
                }
            }
        }
        if let Some((_, i)) = best_inside {
            return Some(i);
        }
        best_near.filter(|(d, _)| *d < 0.3).map(|(_, i)| i)
    }

    /// Closed loops among the CURRENTLY SELECTED 2D objects, in world XY — so you can select
    /// a shape on the drawing and extrude/cut it without any face sketch.
    fn selected_closed_loops(&self) -> Vec<Vec<glam::Vec2>> {
        let mut out = Vec::new();
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get(i) {
                for path in self.outlines_m(&d.geom) {
                    if path.len() >= 4 && (path[0] - path[path.len() - 1]).length() < 1e-3 {
                        out.push(path);
                    }
                }
            }
        }
        out
    }

    /// What the extrude / cut tools act on, as `(frame, closed loops, finished-sketch index,
    /// is-selection)`. Priority:
    ///   1. the face sketch you are ACTIVELY drafting (loops from live `self.doc`),
    ///   2. the current 2D SELECTION — extruded flat on the active ground plane,
    ///   3. the most recent FINISHED face sketch that still holds a closed shape.
    /// `None` when nothing anywhere holds a closed shape.
    fn factory_resolve_extrude(
        &self,
    ) -> Option<(cad_solid::Frame, Vec<Vec<glam::Vec2>>, Option<usize>, bool)> {
        if let Some(session) = self.factory.session.as_ref() {
            let frame = self.factory.model.sketch_by_id(session.plane)?.frame;
            let loops = Self::closed_loops_of(&self.doc);
            return (!loops.is_empty()).then_some((frame, loops, None, false));
        }
        // A 2D selection → extrude on the active ground plane (u=X, v=Y so world XY maps 1:1).
        if !self.selection.is_empty() {
            let loops = self.selected_closed_loops();
            if !loops.is_empty() {
                let z = self.factory.active_base_z();
                let frame = cad_solid::Frame {
                    origin: glam::Vec3::new(0.0, 0.0, z),
                    u: glam::Vec3::X,
                    v: glam::Vec3::Y,
                };
                return Some((frame, loops, None, true));
            }
        }
        for (i, sk) in self.factory.model.sketches.iter().enumerate().rev() {
            let loops = Self::closed_loops_of(&sk.doc);
            if !loops.is_empty() {
                return Some((sk.frame, loops, Some(i), false));
            }
        }
        None
    }

    /// First usable polyline in a document, in its (u,v). Used to read the path a user drew
    /// (open or closed) — the sweep only needs the point sequence.
    fn first_polyline_of(doc: &cad_kernel::Document) -> Option<Vec<glam::Vec2>> {
        // Metres, by the document's own unit — same contract as `closed_loops_of`, since a
        // sweep pairs this path with a section that came from there.
        let k = doc.units.metres_per_unit;
        for d in &doc.dobjects {
            for pl in cad_solid::geom_outlines_scaled(&d.geom, k) {
                if pl.len() >= 2 {
                    return Some(pl);
                }
            }
        }
        None
    }

    /// The two drawing planes PERPENDICULAR to the section face, each containing the extrusion
    /// (normal) direction — the only planes where a path can carry the section "into depth".
    /// Both share the face's origin (so the sweep is anchored at the face). Returned with a
    /// human view-name (nearest world axis of each plane's normal).
    fn sweep_path_planes(section: &cad_solid::Frame) -> [(String, cad_solid::Frame); 2] {
        let n = section.normal();
        // Plane 1: spanned by (normal, v) — for a front face this is the SIDE (left/right).
        let f1 = cad_solid::Frame {
            origin: section.origin,
            u: n,
            v: section.v,
        };
        // Plane 2: spanned by (normal, u) — for a front face this is the TOP.
        let f2 = cad_solid::Frame {
            origin: section.origin,
            u: n,
            v: section.u,
        };
        [
            (Self::view_name(f1.normal()), f1),
            (Self::view_name(f2.normal()), f2),
        ]
    }

    /// Nearest standard-view name for a plane normal (informative label only).
    fn view_name(n: glam::Vec3) -> String {
        let axes = [
            (glam::Vec3::X, "Right"),
            (glam::Vec3::NEG_X, "Left"),
            (glam::Vec3::Y, "Back"),
            (glam::Vec3::NEG_Y, "Front"),
            (glam::Vec3::Z, "Top"),
            (glam::Vec3::NEG_Z, "Bottom"),
        ];
        axes.iter()
            .max_by(|a, b| n.dot(a.0).partial_cmp(&n.dot(b.0)).unwrap())
            .map(|(_, s)| s.to_string())
            .unwrap_or_else(|| "Side".into())
    }

    /// Start the PATH-SWEEP flow. Step 1: the user right-clicks a face, draws the CROSS-SECTION,
    /// and presses Enter / Finish — then the app offers the perpendicular views for the path.
    pub(super) fn factory_begin_sweep_flow(&mut self, cut: bool, furniture: bool) {
        self.factory.modify = None;
        self.factory.open = true;
        self.factory.sweep_flow = Some(crate::factory::SweepFlow {
            cut,
            furniture,
            stage: crate::factory::SweepStage::Section,
            section_frame: None,
            section_loop: None,
            views: Vec::new(),
            path_frame: None,
        });
        let what = if cut {
            "Path cut"
        } else if furniture {
            "Path extrude → furniture"
        } else {
            "Path extrude"
        };
        self.factory.status = format!(
            "{what}: right-click a face → “Draw on this face”, draw the CROSS-SECTION, then press Enter."
        );
    }

    /// Finish the current sweep stage (called on Enter / “Finish sketch” while a sweep flow is
    /// active). Section stage → capture the profile + offer the perpendicular views. Path stage
    /// → capture the path and build the sweep.
    fn factory_finish_sweep_stage(&mut self) {
        let Some(flow) = self.factory.sweep_flow.clone() else {
            self.factory_exit_sketch();
            return;
        };
        match flow.stage {
            crate::factory::SweepStage::Section => {
                // The section is whatever closed loop was drawn on the face.
                let frame = self
                    .factory
                    .session
                    .as_ref()
                    .and_then(|s| self.factory.model.sketch_by_id(s.plane))
                    .map(|sk| sk.frame);
                let loop_ = Self::closed_loops_of(&self.doc).into_iter().next();
                self.factory_exit_sketch();
                match (frame, loop_) {
                    (Some(frame), Some(loop_)) => {
                        let views = Self::sweep_path_planes(&frame);
                        let mut f = flow;
                        f.section_frame = Some(frame);
                        f.section_loop = Some(loop_);
                        f.views = views.to_vec();
                        f.stage = crate::factory::SweepStage::ChooseView;
                        self.factory.status =
                            "Cross-section captured — pick which view to draw the PATH on.".into();
                        self.factory.sweep_flow = Some(f);
                    }
                    _ => {
                        self.factory.sweep_flow = None;
                        self.factory.status =
                            "no closed cross-section was drawn — path sweep cancelled".into();
                    }
                }
            }
            crate::factory::SweepStage::Path => {
                let path = Self::first_polyline_of(&self.doc);
                self.factory_exit_sketch();
                match (
                    flow.section_frame,
                    flow.section_loop.clone(),
                    flow.path_frame,
                    path,
                ) {
                    (Some(sf), Some(section), Some(pf), Some(path)) => {
                        self.factory.sweep_flow = None;
                        let made = self.factory_build_sweep(
                            sf,
                            section,
                            pf,
                            path,
                            flow.cut,
                            flow.furniture,
                        );
                        // Clear the section + path sketches so they don't linger as stray lines
                        // in the 3D view (unless the user asked to keep drawings).
                        if made > 0 && !self.factory.keep_sketch {
                            self.factory_clear_sketch_on(sf);
                            self.factory_clear_sketch_on(pf);
                        }
                    }
                    _ => {
                        self.factory.sweep_flow = None;
                        self.factory.status = "no path was drawn — path sweep cancelled".into();
                    }
                }
            }
            crate::factory::SweepStage::ChooseView => {}
        }
    }

    /// Clear the drawing of the sketch coplanar with `frame` (used to consume the section /
    /// path sketches after a sweep so they don't linger as stray reference lines).
    pub(super) fn factory_clear_sketch_on(&mut self, frame: cad_solid::Frame) {
        if let Some(sk) = self
            .factory
            .model
            .sketches
            .iter_mut()
            .find(|s| Self::frames_coplanar(&s.frame, &frame))
        {
            sk.doc.dobjects.clear();
            self.touch_view(); // a plane's drawing changed → the cached sketch lines are stale
        }
    }

    /// The user picked one of the perpendicular views: enter a sketch on that plane for the path.
    fn factory_choose_sweep_view(&mut self, which: usize) {
        let Some(mut flow) = self.factory.sweep_flow.clone() else {
            return;
        };
        let Some((name, frame)) = flow.views.get(which).cloned() else {
            return;
        };
        flow.stage = crate::factory::SweepStage::Path;
        flow.path_frame = Some(frame);
        self.factory.sweep_flow = Some(flow);
        self.factory_enter_sketch(frame);
        self.factory.status = format!("Draw the PATH on the {name} view, then press Enter.");
    }

    /// Consume the drawn shape after an extrude / cut (unless "keep shape" is on, or it came
    /// from the 2D selection — a selected drawing is never deleted). Clears the live sketch
    /// and exits it, or clears the finished sketch's doc.
    fn factory_consume_sketch(&mut self, finished_idx: Option<usize>, is_selection: bool) {
        if self.factory.keep_sketch || is_selection {
            return;
        }
        match finished_idx {
            None => {
                self.doc.dobjects.clear();
                self.factory_exit_sketch();
            }
            Some(i) => {
                if let Some(sk) = self.factory.model.sketches.get_mut(i) {
                    sk.doc.dobjects.clear();
                }
                self.touch_view(); // consumed a plane's drawing → cached sketch lines are stale
            }
        }
    }

    /// EXTRUDE the shapes drawn on the current face into solids that rise `element_height`
    /// along the face's outward normal. `furniture` only changes the tint/label — the solid
    /// is a normal movable feature either way; it NEVER cuts the building. Consumes the
    /// sketch and returns to the model view. Returns how many solids were made.
    pub(super) fn factory_extrude_sketch(&mut self, furniture: bool) -> usize {
        let Some((frame, loops, fin_idx, is_sel)) = self.factory_resolve_extrude() else {
            self.factory.status =
                "select a closed shape, or draw one on a face (right-click a face → Draw on this face), then Extrude".into();
            return 0;
        };
        let source = if is_sel {
            "2d-selection"
        } else if fin_idx.is_some() {
            "finished-sketch"
        } else {
            "active-face-sketch"
        };
        let features_before = self.factory.model.features.len();
        let n_loops = loops.len();
        self.snapshot_factory();
        let h = self.factory.element_height.max(0.02);
        let plane = cad_solid::Plane::from_basis(frame.origin, frame.u, frame.v);
        let colour = if furniture {
            [0.60, 0.62, 0.70]
        } else {
            [0.72, 0.66, 0.52]
        };
        let mut made = 0;
        let mut last = None;
        // A LOOP INSIDE ANOTHER LOOP IS A HOLE IN IT, not a second solid standing in the middle of
        // the first. A plate drawn with four bolt circles on it used to extrude as five posts.
        for (outer, holes) in Self::nest_loops(loops) {
            if let Ok((profile, centre, w, d)) =
                self.factory.model.add_profile_with_holes(&outer, &holes)
            {
                let placement = cad_solid::Placement {
                    u: centre.x,
                    v: centre.y,
                    lift: 0.0,
                    spin_deg: 0.0,
                    pitch_deg: 0.0,
                    roll_deg: 0.0,
                };
                let id = self.factory.model.push(
                    cad_solid::BoolOp::Union,
                    plane,
                    placement,
                    cad_solid::Primitive::Extrusion { profile, h, w, d },
                );
                self.factory.feature_color.insert(id, colour);
                last = Some(id);
                made += 1;
            }
        }
        if made == 0 {
            self.undo_stack.pop();
            self.factory.status =
                "shapes could not be extruded (degenerate / self-crossing)".into();
            return 0;
        }
        let kind = if furniture { "furniture" } else { "element" };
        self.factory_consume_sketch(fin_idx, is_sel);
        // Always SELECT the new solid (and clear any furniture selection) so it can be moved,
        // recoloured, or Deleted straight away — otherwise the just-made piece felt stuck.
        if let Some(id) = last {
            self.factory.sel_furniture.clear();
            self.factory.selection = vec![id];
        }
        self.factory.recompute();
        let detail = format!(
            "furniture={furniture} height={h:.3} loops={n_loops} made={made} new_feat={} \
             plane_origin=({:.2},{:.2},{:.2})",
            last.map(|id| format!("#{id}"))
                .unwrap_or_else(|| "none".into()),
            frame.origin.x,
            frame.origin.y,
            frame.origin.z,
        );
        self.factory_op_evt(
            if furniture {
                "furniture-extrude"
            } else {
                "extrude"
            },
            source,
            detail,
            features_before,
        );
        self.factory.status = if self.factory.keep_sketch {
            format!("{made} {kind} solid(s) extruded — shape kept")
        } else if is_sel {
            format!("{made} {kind} solid(s) extruded {h:.2} m from the selected shape")
        } else {
            format!("{made} {kind} solid(s) extruded {h:.2} m from the face")
        };
        made
    }

    /// The CUTS list for the selected piece — what makes a cut editable rather than final.
    ///
    /// Every row is a hole that can be switched off, re-depthed or deleted, and each change replays
    /// the whole list against the untouched original. Switching them all off gives back exactly the
    /// piece that was there before anything was cut, which is the property the list exists for.
    fn factory_cuts_panel(&mut self, ui: &mut egui::Ui) {
        let ufu = self.factory.units.clone();
        let Some(fi) = self.factory.sel_furn_primary() else {
            return;
        };
        let n = self.factory.furniture.get(fi).map_or(0, |f| f.cuts.len());
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Cuts").small().weak());
        if n == 0 {
            ui.label(
                egui::RichText::new(
                    "  right-click a face → Draw on this face, draw a shape, then Cut",
                )
                .small()
                .weak(),
            );
            return;
        }

        let mut rebuild = false;
        let mut remove: Option<usize> = None;
        for i in 0..n {
            let Some(cut) = self.factory.furniture[fi].cuts.get(i).cloned() else {
                continue;
            };
            ui.horizontal(|ui| {
                let mut on = cut.enabled;
                if ui
                    .checkbox(&mut on, "")
                    .on_hover_text("apply this cut")
                    .changed()
                {
                    self.factory.furniture[fi].cuts[i].enabled = on;
                    rebuild = true;
                }
                ui.add_sized(
                    [86.0, 18.0],
                    egui::Label::new(egui::RichText::new(&cut.label).small()).selectable(false),
                );
                let mut through = cut.through;
                if ui
                    .selectable_label(through, "through")
                    .on_hover_text("punch all the way out the far side")
                    .clicked()
                {
                    through = !through;
                    self.factory.furniture[fi].cuts[i].through = through;
                    rebuild = true;
                }
                if !through {
                    let mut d = cut.depth;
                    if crate::factory::length_ui(ui, &ufu, &mut d, 0.002, 0.001, 2.0, &self.calc)
                        .changed()
                    {
                        self.factory.furniture[fi].cuts[i].depth = d;
                        rebuild = true;
                    }
                }
                if ui
                    .small_button(
                        egui::RichText::new("✕").color(egui::Color32::from_rgb(230, 170, 170)),
                    )
                    .on_hover_text("delete this cut — the piece returns to what it was")
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }

        if let Some(i) = remove {
            self.snapshot_factory();
            self.factory.furniture[fi].cuts.remove(i);
            rebuild = true;
        } else if rebuild {
            self.snapshot_factory();
        }
        if rebuild {
            match self.factory.rebuild_cut_asset(fi) {
                Ok(()) => {
                    self.factory.recompute();
                    let live = self.factory.furniture[fi]
                        .cuts
                        .iter()
                        .filter(|c| c.enabled)
                        .count();
                    self.factory.status = match live {
                        0 => "no cuts applied — the piece is back to its original".into(),
                        k => format!("{k} cut(s) applied"),
                    };
                }
                Err(e) => self.factory.status = format!("cut: {e}"),
            }
        }
    }

    /// Cut the drawn shapes into a FURNITURE piece — the mesh half of [`Self::factory_cut_sketch`].
    ///
    /// The cut is recorded on the instance and its geometry rebuilt from the original; nothing is
    /// baked, so it can be switched off or deleted afterwards from the Cuts list in the properties
    /// panel. A refusal (an imported mesh, an open shell) leaves the piece untouched and says which
    /// measurement it failed on, rather than producing a mangled body that looks like a bug.
    fn factory_cut_furniture(
        &mut self,
        fi: usize,
        frame: cad_solid::Frame,
        loops: &[Vec<glam::Vec2>],
        through: bool,
        fin_idx: Option<usize>,
        source: &str,
    ) -> usize {
        let features_before = self.factory.model.features.len();
        let depth = self.factory.element_height.max(0.005);
        self.snapshot_factory();
        let name = self
            .factory
            .furniture_lib
            .get(self.factory.furniture[fi].source_asset())
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "piece".into());

        match self
            .factory
            .add_furniture_cut(fi, &frame, loops, through, depth)
        {
            Ok(0) => {
                self.undo_stack.pop();
                self.factory.status = "shapes could not be cut (degenerate / self-crossing)".into();
                0
            }
            Ok(made) => {
                self.factory_consume_sketch(fin_idx, false);
                self.factory.select_furniture(fi);
                self.factory.recompute();
                let what = if through {
                    "opening(s) cut through"
                } else {
                    "recess(es) cut into"
                };
                self.factory.status =
                    format!("{made} {what} {name} — edit or remove them under Cuts in the panel");
                self.history.push(format!(
                    "  furniture cut: {made} on '{name}' (through={through})"
                ));
                self.factory_op_evt(
                    if through {
                        "furniture-cut-through"
                    } else {
                        "furniture-recess"
                    },
                    source,
                    format!(
                        "inst=#{fi} through={through} depth={depth:.3} loops={} cuts_now={}",
                        loops.len(),
                        self.factory.furniture[fi].cuts.len()
                    ),
                    features_before,
                );
                made
            }
            Err(e) => {
                self.undo_stack.pop();
                self.factory.status = format!("{name}: {e}");
                self.history
                    .push(format!("  ! furniture cut refused on '{name}': {e}"));
                0
            }
        }
    }

    /// CUT the shapes drawn on the current face into the solid that face belongs to. With
    /// `through`, the cut punches all the way through (a window/door); otherwise it stops at
    /// `element_height` depth (a recess / niche / blind pocket). The cut is a Difference
    /// relocated to sit right after the target solid so the group-based eval subtracts it
    /// from THAT body only. Returns count.
    pub(super) fn factory_cut_sketch(&mut self, through: bool) -> usize {
        let Some((frame, loops, fin_idx, is_sel)) = self.factory_resolve_extrude() else {
            self.factory.status = "select a closed shape, or draw one on a face, then Cut".into();
            return 0;
        };
        let source = if is_sel {
            "2d-selection"
        } else if fin_idx.is_some() {
            "finished-sketch"
        } else {
            "active-face-sketch"
        };

        // FURNITURE FIRST. Doors, cupboards, kitchens, stairs, ramps and apertures are all mesh
        // INSTANCES, not CSG features, so the Difference feature built below can never reach them.
        // If the shape was drawn on one of their faces, this is a mesh cut instead — same command,
        // same gesture, different machinery underneath.
        if !is_sel {
            if let Some(fi) = self.factory.furniture_at_face(&frame) {
                return self.factory_cut_furniture(fi, frame, &loops, through, fin_idx, source);
            }
        }

        let features_before = self.factory.model.features.len();
        let n_loops = loops.len();
        // The solid to cut: the SMALLEST Union feature whose box contains a point UNDER the
        // shape (its first loop's centre, mapped to the plane) — works for a face OR a 2D
        // selection sitting over a building.
        let o = {
            let l = &loops[0];
            // A closed loop REPEATS its first point, so a plain mean double-counts that corner
            // and drags the probe off centre — for a rectangle, a fifth of the way toward one
            // corner. On a flat wall that still lands on the face and nothing shows; on a
            // CURVED one it lands off the surface in mid-air, and the thickness probe below
            // then measures the gap instead of the wall.
            let pts = if l.len() > 2 && (l[0] - l[l.len() - 1]).length() < 1e-6 {
                &l[..l.len() - 1]
            } else {
                &l[..]
            };
            let c = pts.iter().copied().fold(glam::Vec2::ZERO, |a, p| a + p)
                / (pts.len().max(1) as f32);
            frame.from_uv(c)
        };
        let mut target: Option<(usize, f32)> = None;
        for (i, f) in self.factory.model.features.iter().enumerate() {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            let (mn, mx) = f.world_aabb();
            let inside = o.x >= mn.x - 0.1
                && o.x <= mx.x + 0.1
                && o.y >= mn.y - 0.1
                && o.y <= mx.y + 0.1
                && o.z >= mn.z - 0.1
                && o.z <= mx.z + 0.1;
            if inside {
                let vol =
                    (mx.x - mn.x).max(0.01) * (mx.y - mn.y).max(0.01) * (mx.z - mn.z).max(0.01);
                if target.map_or(true, |(_, v)| vol < v) {
                    target = Some((i, vol));
                }
            }
        }
        // Nothing's box contains the point? Then FIND the solid by ray. A sketch plane is not
        // reliably on the surface it was drawn against — on a curved wall it is a tangent, and
        // measured on a real project the loop centroid lands 0.68–0.76 m clear of the masonry,
        // which is outside every candidate box. Refusing the cut there is wrong: the wall is
        // right in front of the sketch, just not underneath its centroid.
        let nrm = frame.u.cross(frame.v).normalize_or_zero();
        let by_id = |me: &Self, id: u32| -> Option<(usize, f32)> {
            me.factory
                .model
                .features
                .iter()
                .position(|f| f.id == id)
                .map(|i| (i, 0.0_f32))
        };
        let target = target
            .or_else(|| {
                [-nrm, nrm]
                    .into_iter()
                    .filter_map(|d| self.nearest_surface_body(o, d, Self::CUT_SEARCH))
                    .find_map(|id| by_id(self, id))
            })
            // Last resort: ask the WHOLE opening. Its centre can be over a gap between panels or
            // past the end of one, in which case neither the box test nor a centre ray finds
            // anything — and refusing a window that plainly overlaps a wall is the worst of the
            // three answers.
            .or_else(|| {
                let mut seen = Vec::new();
                let _ = self.wall_span_over_opening(&frame, nrm, &loops, -nrm, &mut seen);
                seen.into_iter().find_map(|id| by_id(self, id))
            });
        let Some((tidx, _)) = target else {
            self.factory.status = "no solid under this face to cut".into();
            return 0;
        };
        let target_id = self.factory.model.features[tidx].id;
        let (mn, mx) = self.factory.model.features[tidx].world_aabb();

        // ---- Direction: cut INWARD only, never both sides ---------------------------
        // Inward = INTO the solid. For a picked FACE, `pick_face` returns an OUTWARD normal,
        // so inward is simply −n — robust even when the target body is a thin sliver (where a
        // centroid test is unstable because the centroid sits almost ON the face). For a 2D
        // GROUND selection the plane normal is world +Z, so there orient by the target
        // centroid (the solid sits below/around the plane).
        // The recess over-reach, shared with `factory_edit_cutout`, which subtracts it back off to
        // recover the depth a pocket was cut at.
        const EPS: f32 = CadApp::CUT_EPS;
        let n = frame.u.cross(frame.v).normalize_or_zero();
        let center = (mn + mx) * 0.5;
        let inward = if is_sel {
            if n.dot(center - frame.origin) > 0.0 {
                n
            } else {
                -n
            }
        } else {
            -n
        };

        // ---- Depth + which bodies to cut -------------------------------------------
        // Ray-march EVERY Union body along the cut axis. THROUGH must open the FULL wall
        // thickness from BOTH faces: depending on which surface you picked, a one-sided cut
        // leaves the opposite skin ("visible inside, not outside" — the reported symptom). So
        // measure how far solid runs OUTWARD (−inward) and INWARD (+inward) from the drawn
        // face, each bounded by ITS OWN gap (so the wall across the room is never reached),
        // and span the cutter across the whole run + a margin on each face. RECESS stays a
        // one-sided blind pocket, capped so it can never break through.
        const MARGIN: f32 = 0.1;
        let want = self.factory.element_height.max(0.02);

        // RE-ANCHOR THE PROBE ONTO THE WALL before measuring anything.
        //
        // `o` is the drawn loop's centroid on the SKETCH plane, and that plane is not reliably on
        // the surface. On a curved wall it is a tangent, and measured on a real project the
        // centroid ends up 0.68–0.76 m clear of the masonry. `assembly_span` then starts in
        // mid-air, meets the 0.4 m gap rule on its first crossing, and reports a thickness of
        // ZERO — so the cutter falls back to the bare ±0.1 m margin and stops three quarters of a
        // metre before it reaches the wall. Sampled across those openings, 0 of 25 points were
        // covered; on the straight walls, where the probe lands on the surface, all 25 were.
        //
        // A crossing the ray LEAVES through means the sketch plane is already inside the wall, so
        // the anchor is here and the distance is zero.
        let anchor = [inward, -inward]
            .into_iter()
            .filter_map(|d| {
                self.nearest_surface(o, d, Self::CUT_SEARCH)
                    .map(|(t, leaving)| (if leaving { 0.0 } else { t }, d))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(o, |(t, d)| o + d * t);
        // Where that anchor sits along the plane normal. Every depth below is measured from it,
        // but `lift`/`h` are in the plane's own coordinates, so the offset has to be carried
        // through. `za` is ZERO when the sketch plane was already on the wall — which is the case
        // this code used to assume, so a flat wall behaves exactly as before.
        let za = (anchor - o).dot(n);

        let (in_depth, _in_ids) = self.assembly_span(anchor, inward);
        let s_in = if inward.dot(n) > 0.0 { 1.0_f32 } else { -1.0 }; // sign of inward along +Z(=n)
                                                                     // Every body the WHOLE opening touches, gathered while measuring its depth below. These
                                                                     // join the target list: the centre probe alone misses a wall the opening only partly
                                                                     // overlaps, and a cutter that is never inserted behind a body cannot open it.
        let mut grid_bodies: Vec<u32> = Vec::new();
        // First decide how far the cutter reaches on each face → lift/h in plane-local Z.
        let (lift, h, out_depth): (f32, f32, f32) = if through {
            let (out_depth, _out_ids) = self.assembly_span(anchor, -inward);
            let zeta_in = za + s_in * (in_depth + MARGIN);
            let zeta_out = za - s_in * (out_depth + MARGIN);
            let (mut lo, mut hi) = (zeta_in.min(zeta_out), zeta_in.max(zeta_out));
            // MEASURE ACROSS THE WHOLE OPENING, not just under its centre.
            //
            // One probe describes one point. A curved wall is faceted, so within a single window
            // the far surface is further away at the edges than in the middle, and an opening
            // that crosses a facet joint meets a panel half a metre deeper along the normal.
            // Measured on the real project after the centre probe was fixed, openings still
            // reached only 13 of 25 sample points, short by up to 0.519 m — a hole that is
            // see-through in the middle and blind at one side.
            //
            // So take the union of the wall's span over a grid across the opening. The cutter is
            // only ever made LONGER by this, and only along its own axis, so it opens more of the
            // wall it is already cutting and cannot wander into anything else.
            if let Some((glo, ghi)) =
                self.wall_span_over_opening(&frame, n, &loops, inward, &mut grid_bodies)
            {
                lo = lo.min(glo - MARGIN);
                hi = hi.max(ghi + MARGIN);
            }
            (lo, hi - lo, out_depth)
        } else {
            // recess: blind pocket from the WALL face inward by `want` (never through).
            let wall = (in_depth - EPS).max(EPS);
            let d = want.min(wall);
            let (lift, h) = if s_in > 0.0 {
                (za - EPS, d + EPS)
            } else {
                (za - d, d + EPS)
            };
            (lift, h, 0.0)
        };
        // Which bodies to cut: EVERY Union body whose world AABB overlaps the cutter's swept
        // box. A single probe ray misses bodies the opening actually passes through (the
        // reported bug — the exterior shell was a separate body the ray never touched). The
        // swept box is gap-bounded (in_depth/out_depth), so it stays local to this wall and
        // never reaches the wall across the room; a Difference that doesn't touch a body is a
        // harmless no-op, so over-selecting is safe.
        let mut bmin = glam::Vec3::splat(f32::INFINITY);
        let mut bmax = glam::Vec3::splat(f32::NEG_INFINITY);
        for lp in &loops {
            for &p in lp {
                let w = frame.from_uv(p);
                for z in [lift, lift + h] {
                    let q = w + n * z;
                    bmin = bmin.min(q);
                    bmax = bmax.max(q);
                }
            }
        }
        let pad = 0.05;
        let mut targets: Vec<u32> = Vec::new();
        for f in &self.factory.model.features {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            let (amn, amx) = f.world_aabb();
            if amn.x <= bmax.x + pad
                && amx.x >= bmin.x - pad
                && amn.y <= bmax.y + pad
                && amx.y >= bmin.y - pad
                && amn.z <= bmax.z + pad
                && amx.z >= bmin.z - pad
            {
                targets.push(f.id);
            }
        }
        // …AND every body the cut AXIS actually runs through. A box search compares volumes: a
        // thin cutter offset from the wall matches the big bodies whose boxes span the building
        // and misses the wall itself. The ray answers "what does this opening pass through"
        // directly. ADDING targets is safe — each gets its own cutter behind its own body, and a
        // Difference that touches nothing is a no-op. (Never MOVE an existing one: a cutter that
        // is harmless behind one body can remove most of another. That mistake cost 30% of this
        // building once already.)
        for d in [inward, -inward] {
            for id in self.assembly_span(anchor, d).1 {
                if !targets.contains(&id) {
                    targets.push(id);
                }
            }
        }
        // …and everything the grid across the opening found, which is the only one of the three
        // that sees a wall the opening merely CLIPS. Recorded on a real session: the failing
        // window reported `in_depth=0.000 out_depth=0.000` and `targets=[1, 122, 124]` — no wall
        // among them — while its two neighbours on the same curved run listed 52/53/54 and 50/51
        // and cut correctly.
        for id in grid_bodies {
            if !targets.contains(&id) {
                targets.push(id);
            }
        }
        if targets.is_empty() {
            targets.push(target_id);
        }
        self.snapshot_factory();
        let plane = cad_solid::Plane::from_basis(frame.origin, frame.u, frame.v);
        let mut made = 0;
        let mut lost_target = 0;
        for pts in loops {
            if let Ok((profile, centre, w, d)) = self.factory.model.add_profile(&pts) {
                // One Difference per target body, each relocated to sit right AFTER its body.
                // `eval` is a SEQUENTIAL accumulator — a Union flushes the body before it and
                // starts a new one, and a Difference applies only to the most recent Union — so
                // a cutter must sit directly behind every body it is meant to open. Appending a
                // single cutter at the end instead would reach only the LAST body. Re-find the
                // body by id every time, because each insert shifts the indices after it.
                let placement = cad_solid::Placement {
                    u: centre.x,
                    v: centre.y,
                    lift,
                    spin_deg: 0.0,
                    pitch_deg: 0.0,
                    roll_deg: 0.0,
                };
                for &tid in &targets {
                    self.factory.model.push(
                        cad_solid::BoolOp::Difference,
                        plane,
                        placement,
                        cad_solid::Primitive::Extrusion { profile, h, w, d },
                    );
                    if let Some(mut diff) = self.factory.model.features.pop() {
                        // WHAT THIS CUT WAS FOR, recorded where it happens. Every reader
                        // afterwards had to infer it from the geometry, and the only inference
                        // available — "does the cutter span the wall?" — a recess fails by
                        // construction. See `Feature::through`.
                        diff.through = Some(through);
                        // A MISSING TARGET IS NOT A NO-OP. `insert_after` appends when it cannot
                        // find the host, so the cutter is still in the model — bound to whatever
                        // Union happens to be last, which is some other body. That is the same
                        // silent mis-binding `rederive_wall` was fixed for, and it must not be
                        // discarded just because the return type makes it easy to.
                        if !self.factory.model.insert_after(tid, diff) {
                            lost_target += 1;
                        }
                    }
                }
                made += 1;
            }
        }
        if lost_target > 0 {
            self.history.push(format!(
                "  ! {lost_target} cutter(s) could not find their body — appended, so they may cut \
                 the wrong one (▼ Openings to check or delete)"
            ));
        }
        if made == 0 {
            self.undo_stack.pop();
            self.factory.status = "shapes could not be cut (degenerate / self-crossing)".into();
            return 0;
        }
        let what = if through {
            "opening(s) cut through"
        } else {
            "recess(es) cut"
        };
        self.factory_consume_sketch(fin_idx, is_sel);
        self.factory.recompute();
        // DID IT ACTUALLY GO THROUGH? Sample the finished cutter against the wall across the whole
        // opening and record the verdict with the op.
        //
        // Everything else here describes what the cut INTENDED. Three windows drawn on one wall in
        // one session produced three plausible-looking events and one blind window, and telling
        // them apart meant re-deriving the geometry by hand afterwards — twice, from dumps that
        // had already truncated. A cut that does not reach should say so where it happens.
        let (cov_ok, cov_total, cov_short) = self
            .factory
            .model
            .features
            .iter()
            .rev()
            .find(|f| f.op == cad_solid::BoolOp::Difference)
            .map_or((0, 0, 0.0), |c| self.cut_coverage(c));
        // A RECESS THAT STOPS INSIDE THE WALL IS DOING ITS JOB, so it is not warned about. The
        // coverage numbers are still printed — a pocket that found no wall at all is worth seeing.
        let verdict = if cov_total == 0 {
            "  ⚠ NO WALL under this opening".to_string()
        } else if !through {
            String::new()
        } else if cov_ok < cov_total {
            format!(
                "  ⚠ DOES NOT GO THROUGH at {}/{cov_total} points, short by {cov_short:.3} m",
                cov_total - cov_ok
            )
        } else {
            String::new()
        };
        let detail = format!(
            "through={through} primary=#{target_id} targets={targets:?} \
             probe=({:.2},{:.2},{:.2}) normal=({:.2},{:.2},{:.2}) inward=({:.2},{:.2},{:.2}) \
             in_depth={in_depth:.3} out_depth={out_depth:.3} lift={:.3} h={:.3} loops={n_loops} \
             bodies_cut={} made={made} coverage={cov_ok}/{cov_total}{verdict}",
            o.x,
            o.y,
            o.z,
            n.x,
            n.y,
            n.z,
            inward.x,
            inward.y,
            inward.z,
            lift,
            h,
            targets.len(),
        );
        self.factory_op_evt(
            if through { "cut-through" } else { "recess" },
            source,
            detail,
            features_before,
        );
        self.factory.status = if self.factory.keep_sketch {
            format!("{made} {what} the solid — shape kept")
        } else {
            format!("{made} {what} the solid")
        };
        made
    }

    /// Build a swept solid (Union) or cutter (Difference): the CROSS-SECTION drawn on
    /// `section_frame` is swept along the PATH drawn on `path_frame` (a plane perpendicular to
    /// the section). The path is re-expressed in the section's local frame — so its component
    /// along the section NORMAL becomes the extrusion depth — then csgrs sweeps the profile
    /// perpendicular along it (parallel-transport frames). For a CUT the Difference is relocated
    /// right after the body under the path so group-eval subtracts it from THAT body only.
    pub(super) fn factory_build_sweep(
        &mut self,
        section_frame: cad_solid::Frame,
        section: Vec<glam::Vec2>,
        path_frame: cad_solid::Frame,
        path: Vec<glam::Vec2>,
        cut: bool,
        furniture: bool,
    ) -> usize {
        let source = "sweep-flow";
        let features_before = self.factory.model.features.len();
        let (o, u, v, n) = (
            section_frame.origin,
            section_frame.u,
            section_frame.v,
            section_frame.normal(),
        );
        // Path points → WORLD (via the path plane) → SECTION-LOCAL (x=u, y=v, z=normal/depth).
        let path_local: Vec<glam::Vec3> = path
            .iter()
            .map(|p| {
                let w = path_frame.from_uv(*p);
                let dp = w - o;
                glam::Vec3::new(dp.dot(u), dp.dot(v), dp.dot(n))
            })
            .collect();

        // For a CUT, find the target body under the path's WORLD centroid BEFORE mutating.
        let target = if cut {
            let c = path.iter().copied().fold(glam::Vec2::ZERO, |a, p| a + p)
                / (path.len().max(1) as f32);
            let probe = path_frame.from_uv(c);
            let mut best: Option<(u32, f32)> = None;
            for f in &self.factory.model.features {
                if f.op != cad_solid::BoolOp::Union {
                    continue;
                }
                let (mn, mx) = f.world_aabb();
                let inside = probe.x >= mn.x - 0.1
                    && probe.x <= mx.x + 0.1
                    && probe.y >= mn.y - 0.1
                    && probe.y <= mx.y + 0.1
                    && probe.z >= mn.z - 0.1
                    && probe.z <= mx.z + 0.1;
                if inside {
                    let s = mx - mn;
                    let vol = s.x.abs().max(0.01) * s.y.abs().max(0.01) * s.z.abs().max(0.01);
                    if best.map_or(true, |(_, v)| vol < v) {
                        best = Some((f.id, vol));
                    }
                }
            }
            match best {
                Some((tid, _)) => Some(tid),
                None => {
                    self.factory.status = "no solid under this path to cut".into();
                    return 0;
                }
            }
        } else {
            None
        };

        self.snapshot_factory();
        let Ok((profile, _centre, w, d)) = self.factory.model.add_profile(&section) else {
            self.undo_stack.pop();
            self.factory.status = "cross-section unusable (degenerate / self-crossing)".into();
            return 0;
        };
        let Some((path_id, pmn, pmx)) = self.factory.model.add_path(&path_local) else {
            self.undo_stack.pop();
            self.factory.status = "path needs at least two points".into();
            return 0;
        };
        let (bmin, bmax) = Self::sweep_local_aabb(pmn, pmx, w, d);
        let plane = cad_solid::Plane::from_basis(o, u, v);
        let op = if cut {
            cad_solid::BoolOp::Difference
        } else {
            cad_solid::BoolOp::Union
        };
        let id = self.factory.model.push(
            op,
            plane,
            cad_solid::Placement::default(),
            cad_solid::Primitive::Sweep {
                profile,
                path: path_id,
                bmin,
                bmax,
            },
        );
        match target {
            Some(tid) => {
                // Relocate the Difference to sit right after its target body — and say so if it
                // could not be found, because the fallback is to append, and an appended cutter
                // opens whichever body is last rather than none.
                if let Some(diff) = self.factory.model.features.pop() {
                    if !self.factory.model.insert_after(tid, diff) {
                        self.history.push(
                            "  ! swept cut could not find its body — appended, so it may cut the \
                             wrong one (▼ Openings to check or delete)"
                                .into(),
                        );
                    }
                }
            }
            None => {
                let colour = if furniture {
                    [0.60, 0.62, 0.70]
                } else {
                    [0.72, 0.66, 0.52]
                };
                self.factory.feature_color.insert(id, colour);
                self.factory.sel_furniture.clear();
                self.factory.selection = vec![id];
            }
        }
        self.factory.recompute();
        let op_name = if cut {
            "path-cut"
        } else if furniture {
            "path-extrude-furniture"
        } else {
            "path-extrude"
        };
        let detail = format!(
            "furniture={furniture} cut={cut} section_verts={} path_verts={} target={:?}",
            section.len(),
            path.len(),
            target,
        );
        self.factory_op_evt(op_name, source, detail, features_before);
        self.factory.status = if cut {
            format!("channel cut along a {}-point path", path.len())
        } else {
            format!("swept solid created along a {}-point path", path.len())
        };
        1
    }

    /// Conservative LOCAL AABB of a swept solid: the path's bbox expanded by the section's
    /// reach (half its diagonal) on every axis, since the section can face any direction
    /// along the path.
    fn sweep_local_aabb(pmn: glam::Vec3, pmx: glam::Vec3, w: f32, d: f32) -> ([f32; 3], [f32; 3]) {
        let r = 0.5 * (w * w + d * d).sqrt() + 0.02;
        (
            [pmn.x - r, pmn.y - r, pmn.z - r],
            [pmx.x + r, pmx.y + r, pmx.z + r],
        )
    }

    /// Emit a `FactoryOp` recorder event stamped with the current model size (features,
    /// Union bodies, triangles). `features_before` is captured by the caller before the
    /// op ran. No-op unless recording — this is the "record what's going on in 3D" tap.
    pub(super) fn factory_op_evt(&mut self, op: &str, source: &str, detail: String, features_before: usize) {
        if !self.dbg.recording {
            return;
        }
        let features_after = self.factory.model.features.len();
        let bodies = self
            .factory
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Union)
            .count();
        let tris = self.factory.cached.positions.len() / 3;
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::FactoryOp {
                op: op.to_string(),
                source: source.to_string(),
                detail,
                features_before,
                features_after,
                bodies,
                tris,
            }
        );
    }

    /// Re-measure openings cut by the broken thickness probe, which stopped 100 mm into the
    /// wall instead of going through. Returns `(repaired, left_alone)`.
    ///
    /// `assembly_span` used to treat the run from the probe to the FIRST surface as a gap — but
    /// that run IS the wall — so any wall thicker than the 0.4 m gap measured zero and the
    /// cutter fell back to the bare ±0.1 m margin. Those cuts are still sitting in saved
    /// projects, because a file records the cut that was made, not the one that was meant.
    ///
    /// Each is re-probed with the FIXED span and its depth rewritten. A cut whose wall still
    /// cannot be found is left exactly as it is rather than guessed at.
    ///
    /// The probe does NOT trust the face point reconstructed from the sketch plane. Measured on
    /// the real project, that point lands up to 0.64 m clear of the wall — outside the building,
    /// in mid-air — so the 0.4 m gap rule broke the walk on the very first crossing and every one
    /// of the ten openings reported "unmeasurable". `plane.origin() + u·place.u + v·place.v` is
    /// evidently not where the sketch was drawn, and the original pick point is not stored, so it
    /// cannot be recovered. Four cuts sharing one identical reconstructed point gave that away.
    ///
    /// So the wall is FOUND instead of assumed: cast along the cutter's normal in both directions,
    /// take the nearest surface within [`SEARCH`], and measure the assembly from there — which is
    /// the situation `assembly_span` was built for, a probe sitting ON a face. Self-correcting,
    /// and it does not depend on how a placement happens to be encoded.
    ///
    /// Returns `(repaired, notes)` — one note per opening left alone, saying how far the nearest
    /// wall actually was. Four of the ten on the real project sit 2.14 m from anything, which is
    /// too far to attribute to a wall with any confidence: at that range the nearest surface is
    /// as likely to be across the room, and deepening into it would punch a hole through the
    /// wrong piece of the building. Naming them is more use than guessing at them.
    pub(super) fn factory_repair_shallow_cuts(&mut self) -> (usize, Vec<String>) {
        const MARGIN: f32 = 0.1;
        /// How far to look for the wall this opening belongs to. Comfortably past the 0.64 m seen
        /// in practice, and well short of the room's width so it cannot grab the wall opposite.
        const SEARCH: f32 = 1.5;
        // WHAT COUNTS AS BROKEN. Two signatures, and between them they leave recesses alone.
        //
        //   1. the old fallback's shape — h ≤ 0.25 with lift ≈ −0.1, which no hand-made cut has;
        //   2. PARTIAL coverage: the cutter spans the wall at some points across the opening and
        //      not others.
        //
        // The second is the one that matters now, and it is safe for a reason worth stating. A
        // RECESS is a deliberate blind pocket, so it fails to reach EVERYWHERE — 0 of N. A
        // through-cut defeated by a curved wall reaches the middle of the window and stops short
        // at one side. Partial coverage is therefore a thing a recess cannot be.
        //
        // Measured on the real project, `diag` reported twelve openings not going through, and
        // not one of them still carried the old signature — so the first rule alone had stopped
        // repairing anything. Six are partial (21/25, 13/25, 13/15) and get fixed; the other six
        // are 0 of N and stay untouched, because at that point a blind pocket and a failed
        // opening look identical and only the person who drew it knows which it was.
        type Suspect = (
            usize,
            u32,
            glam::Vec3,
            glam::Vec3,
            glam::Vec3,
            glam::Vec3,
            f32,
            f32,
        );
        let suspects: Vec<Suspect> = self
            .factory
            .model
            .features
            .iter()
            .enumerate()
            .filter(|(_, f)| f.op == cad_solid::BoolOp::Difference)
            .filter_map(|(i, f)| {
                // `w`/`d` are the opening's own footprint on its plane — carried through so the
                // wall can be measured across the WHOLE window, not only under its centre.
                let (h, w, d) = match f.primitive {
                    cad_solid::Primitive::Extrusion { h, w, d, .. } => (h, w, d),
                    cad_solid::Primitive::Box { w, d, h } => (h, w, d),
                    _ => return None,
                };
                let fallback_shape = h <= 0.25 && (f.placement.lift + 0.1).abs() <= 0.02;
                if !fallback_shape {
                    let (ok, total, _) = self.cut_coverage(f);
                    // Partial only. `ok == total` is sound; `ok == 0` is indistinguishable from a
                    // recess and must not be guessed at.
                    if total == 0 || ok == 0 || ok == total {
                        return None;
                    }
                }
                // A STARTING GUESS at where the sketch was, and nothing more — measured on the
                // real project this lands up to 0.64 m off the wall, and four separate cuts
                // reduce to one identical point. The wall is found by search below.
                let (u, v) = f.plane.axes();
                let n = u.cross(v).normalize_or_zero();
                let o = f.plane.origin() + u * f.placement.u + v * f.placement.v;
                (n.length_squared() > 0.5).then_some((i, f.id, o, n, u, v, w, d))
            })
            .collect();

        // Measure FIRST, mutate after — `assembly_span` borrows the model.
        let mut plans: Vec<(usize, f32, f32)> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for (i, id, o, n, uax, vax, w, d) in suspects {
            // The opening's footprint, centred on `o` — a rectangle is the right over-cover here
            // (see `wall_span_over_opening`): extra samples can only lengthen the cutter along
            // its own axis, never widen it.
            let prof = vec![vec![
                glam::Vec2::new(-w * 0.5, -d * 0.5),
                glam::Vec2::new(w * 0.5, -d * 0.5),
                glam::Vec2::new(w * 0.5, d * 0.5),
                glam::Vec2::new(-w * 0.5, d * 0.5),
            ]];
            // FIND the wall. Whichever side it is on, the nearest surface within SEARCH is the
            // one this opening belongs to; `sgn` records which way that was along the normal.
            // A crossing the ray LEAVES through means the probe already stands in the wall, so
            // there is nothing to travel: the face is here, at zero.
            let toward = [(-1.0_f32, -n), (1.0_f32, n)]
                .into_iter()
                .filter_map(|(sgn, d)| {
                    self.nearest_surface(o, d, SEARCH)
                        .map(|(t, leaving)| (if leaving { 0.0 } else { t }, sgn, d))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            let Some((face_t, sgn, dir)) = toward else {
                // Say how far away the nearest wall actually is — that number is what tells the
                // user whether this is a near miss or an opening adrift in the middle of a room.
                const REPORT: f32 = 8.0;
                let far = [-n, n]
                    .into_iter()
                    .filter_map(|d| self.nearest_surface(o, d, REPORT).map(|(t, _)| t))
                    .min_by(f32::total_cmp);
                notes.push(match far {
                    Some(t) => format!(
                        "  · cut #{id}: nearest wall is {t:.2} m away — too far to attribute \
                         ({SEARCH:.1} m limit). Redraw this opening on the wall itself."
                    ),
                    None => format!(
                        "  · cut #{id}: no wall within {REPORT:.0} m in either direction. \
                         Redraw this opening on the wall itself."
                    ),
                });
                continue;
            };
            // Measure the assembly from ON that face, which is what the span expects.
            let face = o + dir * face_t;
            let thk = self.assembly_span(face, dir).0;
            if thk <= 0.05 {
                notes
                    .push(format!(
                    "  · cut #{id}: found a surface {face_t:.2} m away but could not measure its \
                     thickness — left as it is."));
                continue;
            }
            // Back into the cutter's own axis: the wall occupies `near … face_t + thk` along
            // `dir`, which is `sgn` times that along the plane normal. Margin on both ends so the
            // cut breaks cleanly through instead of leaving a skin.
            //
            // When the probe started INSIDE (`face_t == 0`) the wall also runs BEHIND it, so the
            // near edge is measured backwards rather than assumed to be at the probe. Otherwise
            // the near edge is simply where the ray entered.
            let near = if face_t <= 1e-6 {
                -self.assembly_span(o, -dir).0
            } else {
                face_t
            };
            let (a, b) = (sgn * near, sgn * (face_t + thk));
            let (mut lo, mut hi) = (a.min(b) - MARGIN, a.max(b) + MARGIN);
            // …and across the WHOLE opening, not just under its centre — the same correction the
            // cut itself makes. A curved wall bends away toward a window's edges, so a cutter
            // sized from one probe is see-through in the middle and blind at one side. Measured
            // on the real project, repairing from the centre alone still left openings reaching
            // 13 of 25 sample points, short by up to 0.519 m.
            let frame = cad_solid::Frame {
                origin: o,
                u: uax,
                v: vax,
            };
            // The repair only re-measures an existing cutter; it never re-targets one, so the
            // bodies the grid touches are of no use here.
            let mut _seen = Vec::new();
            if let Some((glo, ghi)) = self.wall_span_over_opening(&frame, n, &prof, -n, &mut _seen)
            {
                lo = lo.min(glo - MARGIN);
                hi = hi.max(ghi + MARGIN);
            }
            plans.push((i, lo, hi - lo));
        }
        for (i, lift, h) in &plans {
            let f = &mut self.factory.model.features[*i];
            f.placement.lift = *lift;
            match &mut f.primitive {
                cad_solid::Primitive::Extrusion { h: fh, .. } => *fh = *h,
                cad_solid::Primitive::Box { h: fh, .. } => *fh = *h,
                _ => {}
            }
        }
        (plans.len(), notes)
    }

    /// Delete solids that are an EXACT copy of an earlier one — same shape, same plane, same
    /// placement. Returns `(removed, left_alone)`.
    ///
    /// Two solids in the same place have no depth bias to separate them, so which one draws
    /// flips as the camera moves; that is the flicker this exists to remove. Removing an exact
    /// twin cannot change the picture, because the survivor occupies precisely the same space.
    ///
    /// The one thing it will NOT do is delete a duplicate that carries its own cuts. `eval` is
    /// sequential — a Union starts a new body and the Differences after it belong to it — so
    /// removing such a Union would hand its cuts to the PREVIOUS body and punch holes in the
    /// wrong solid. Those are counted and reported instead of guessed at.
    pub(super) fn factory_dedupe_solids(&mut self) -> (usize, usize) {
        // Exact signature from the Debug form: every field of these types is a Copy scalar, so
        // this is faithful, and it avoids widening `cad_solid`'s public API just to compare.
        let sig =
            |f: &cad_solid::Feature| format!("{:?}|{:?}|{:?}", f.plane, f.placement, f.primitive);
        let feats = &self.factory.model.features;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut drop_ids: Vec<u32> = Vec::new();
        let mut left = 0usize;
        for (i, f) in feats.iter().enumerate() {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            let s = sig(f);
            if seen.insert(s) {
                continue; // first of its kind — the one that stays
            }
            // A duplicate. Safe to drop only if no Difference is riding on it.
            let owns_cut = feats
                .get(i + 1)
                .is_some_and(|n| n.op == cad_solid::BoolOp::Difference);
            if owns_cut {
                left += 1;
            } else {
                drop_ids.push(f.id);
            }
        }
        for id in &drop_ids {
            self.factory.model.features.retain(|f| f.id != *id);
            self.factory.feature_color.remove(id);
            self.factory.feature_texture.remove(id);
            self.factory.ceilings.remove(id);
        }
        (drop_ids.len(), left)
    }

    /// How far to look for the wall an opening belongs to, when the sketch plane is not sitting
    /// on it. Comfortably past the 0.76 m measured on a real curved wall, and well short of a
    /// room's width so the search can never reach the wall opposite.
    ///
    /// Shared by the cut and by `repaircuts`, deliberately: the repair exists to redo what the
    /// cut should have done, so the two must agree on what counts as "this wall".
    const CUT_SEARCH: f32 = 1.5;

    /// The sliver a cut over-reaches by, so a cutter never leaves a coplanar face for the BSP to
    /// argue about. A recess is built `depth + CUT_EPS` deep.
    ///
    /// Named and shared because `factory_edit_cutout` has to SUBTRACT it to recover the depth a
    /// recess was cut at. Two copies of `0.01` would drift, and the symptom would be a pocket
    /// that grows by a hair every time it is reshaped.
    const CUT_EPS: f32 = 0.01;

    /// The wall's extent along the plane normal, taken over a GRID across the whole opening and
    /// returned as `(lo, hi)` in plane-local Z. `None` when no sample finds a wall.
    ///
    /// A single probe under the opening's centre is only correct on a flat wall square to it. A
    /// curved wall is faceted: within one window the surface bends away toward the edges, and an
    /// opening spanning a facet joint meets the next panel further along the normal. Sizing the
    /// cutter from the centre alone leaves an opening that is see-through in the middle and blind
    /// at one side — measured at 13 of 25 sample points on the real project, short by 0.519 m.
    ///
    /// The grid is over the opening's (u,v) bounding box, which over-covers a non-rectangular
    /// profile. That is the safe direction: a sample off the profile can only make the cutter
    /// longer along its own axis, never wider, so it opens more of the wall it is already cutting.
    /// `bodies` collects every Union the grid touched. The caller MUST fold these into its target
    /// list: a window whose CENTRE overhangs the end of a panel, or spans a gap between two, finds
    /// no wall under the centre probe at all — measured in a real session as
    /// `in_depth=0.000 out_depth=0.000`, with the wall bodies absent from `targets` and the
    /// opening left blind, while two neighbouring windows on the same wall cut correctly. The
    /// depth was already being taken across the whole opening; the target list has to be too.
    fn wall_span_over_opening(
        &self,
        frame: &cad_solid::Frame,
        n: glam::Vec3,
        loops: &[Vec<glam::Vec2>],
        inward: glam::Vec3,
        bodies: &mut Vec<u32>,
    ) -> Option<(f32, f32)> {
        const G: i32 = 4; // 5×5 samples — enough for a facet joint, cheap enough to run per cut
        let (mut umin, mut umax) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut vmin, mut vmax) = (f32::INFINITY, f32::NEG_INFINITY);
        for lp in loops {
            for p in lp {
                umin = umin.min(p.x);
                umax = umax.max(p.x);
                vmin = vmin.min(p.y);
                vmax = vmax.max(p.y);
            }
        }
        if !umin.is_finite() || !vmin.is_finite() {
            return None;
        }

        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for iu in 0..=G {
            for iv in 0..=G {
                let u = umin + (umax - umin) * (iu as f32 / G as f32);
                let v = vmin + (vmax - vmin) * (iv as f32 / G as f32);
                let p = frame.from_uv(glam::Vec2::new(u, v));
                // Find the wall at THIS point, the same way the centre probe does.
                let near = [inward, -inward]
                    .into_iter()
                    .filter_map(|d| {
                        self.nearest_surface(p, d, Self::CUT_SEARCH)
                            .map(|(t, leaving)| (if leaving { 0.0 } else { t }, d))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                let Some((t, dir)) = near else { continue };
                let (thk, ids) = self.assembly_span(p + dir * t, dir);
                if thk <= 1e-3 {
                    continue;
                }
                for id in ids {
                    if !bodies.contains(&id) {
                        bodies.push(id);
                    }
                }
                // Into the plane's own axis, where `lift`/`h` live.
                let sgn = dir.dot(n);
                let (a, b) = (sgn * t, sgn * (t + thk));
                lo = lo.min(a.min(b));
                hi = hi.max(a.max(b));
            }
        }
        (lo.is_finite() && hi > lo).then_some((lo, hi))
    }

    /// How much of an opening the cutter actually spans: `(covered, sampled, worst shortfall)`.
    ///
    /// The only honest test of a through-cut. Depth describes one point; a cutter can be deep and
    /// still miss, and on a curved wall it routinely opens the middle of a window and leaves one
    /// side blind. `sampled` counts only the grid points that FIND a wall, so an opening hanging
    /// past the end of a panel is not scored against the air beside it.
    pub fn cut_coverage(&self, cut: &cad_solid::Feature) -> (usize, usize, f32) {
        let (h, w, d) = match cut.primitive {
            cad_solid::Primitive::Extrusion { h, w, d, .. } => (h, w, d),
            cad_solid::Primitive::Box { w, d, h } => (h, w, d),
            _ => return (0, 0, 0.0),
        };
        let (uax, vax) = cut.plane.axes();
        let n = uax.cross(vax).normalize_or_zero();
        if n.length_squared() < 0.5 {
            return (0, 0, 0.0);
        }
        let (lo, hi) = (cut.placement.lift, cut.placement.lift + h);
        let (mut ok, mut total, mut worst) = (0usize, 0usize, 0.0_f32);
        const G: i32 = 4;
        for iu in 0..=G {
            for iv in 0..=G {
                let p = cut.plane.origin()
                    + uax * (cut.placement.u + w * (iu as f32 / G as f32 - 0.5))
                    + vax * (cut.placement.v + d * (iv as f32 / G as f32 - 0.5));
                let near = [(-1.0_f32, -n), (1.0_f32, n)]
                    .into_iter()
                    .filter_map(|(sgn, dir)| {
                        self.nearest_surface(p, dir, Self::CUT_SEARCH)
                            .map(|(t, leaving)| (if leaving { 0.0 } else { t }, sgn, dir))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                let Some((t, sgn, dir)) = near else { continue };
                let thk = self.assembly_span(p + dir * t, dir).0;
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

    /// Which Union body the nearest surface along `dir` belongs to, within `max`.
    fn nearest_surface_body(&self, origin: glam::Vec3, dir: glam::Vec3, max: f32) -> Option<u32> {
        let start = origin + dir * 1e-3;
        let mut best: Option<(f32, u32)> = None;
        for f in &self.factory.model.features {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            for c in self
                .factory
                .model
                .feature_world_positions(f)
                .chunks_exact(3)
            {
                let (a, b, cc) = (
                    glam::Vec3::from(c[0]),
                    glam::Vec3::from(c[1]),
                    glam::Vec3::from(c[2]),
                );
                if let Some(t) = cad_solid::ray_triangle(start, dir, a, b, cc) {
                    if t > 1e-4 && t <= max && best.is_none_or(|(z, _)| t < z) {
                        best = Some((t, f.id));
                    }
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// How many entries any one list in a scene capture prints. Past this the list says how
    /// many it dropped. A 27-piece furniture set and a 159-feature model both fit; a survey
    /// import with thousands cannot flood the dump.
    pub(super) const SCENE_LIST_MAX: usize = 48;

    /// Materials get a much tighter cap than the other lists.
    ///
    /// A working project accumulates dozens of near-identical clipboard textures — this one has
    /// 77, most of them one 540×360 clip used by one feature — and printing 48 of them TWICE
    /// (session start and end) pushed a real dump past the paste limit and truncated away the
    /// very cut events being investigated. Twice. The list is ordered by use, so the few that
    /// matter are at the top; the rest are summarised in a line.
    const SCENE_MATERIALS_MAX: usize = 12;

    /// EVERYTHING the 3D renderer is fed, as one recorder event — see [`DbgEvent::FactoryScene`].
    ///
    /// Written after a rendering bug took several rounds to pin down because the dump could not
    /// see any of it. The recorder knew the frame times and the 2D drawing; it did not know the
    /// model's coordinates, its materials, its cut depths or its camera, so the only evidence
    /// left was screenshots. Each section below is one of the questions that had to be answered
    /// by hand at the time.
    ///
    /// Read-only, and cheap enough to take on demand: it walks the feature list and the texture
    /// table once and formats strings. It does NOT walk the triangle soup.
    pub(super) fn factory_scene_capture(&self, reason: &str) -> crate::dbg_recorder::DbgEvent {
        use cad_solid::BoolOp;
        let f = &self.factory;
        let mut sections: Vec<(String, Vec<String>)> = Vec::new();

        // ── WHERE THE MODEL IS ───────────────────────────────────────────────────────────
        // First, because it silently sets the precision of everything downstream. A survey
        // plan sits at coordinates in the thousands, and f32 has 24 bits of mantissa: at
        // 6850 m one ULP is 0.8 mm. That is the size of a close-up pixel footprint, so any
        // texture lookup or screen-space DERIVATIVE taken from an un-rebased world position
        // is quantised — banding that crawls, and normals that speckle. Nothing in a frame
        // time or a triangle count hints at it, which is exactly why it belongs here, stated
        // as a number with its consequence spelled out.
        let bounds = f.cached.bounds();
        let mut model_lines = Vec::new();
        match bounds {
            Some((mn, mx)) => {
                model_lines.push(format!(
                    "world AABB  min=({:.3}, {:.3}, {:.3})  max=({:.3}, {:.3}, {:.3})  size=({:.2} × {:.2} × {:.2} m)",
                    mn[0], mn[1], mn[2], mx[0], mx[1], mx[2],
                    mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]));
                let far = mn
                    .iter()
                    .chain(mx.iter())
                    .fold(0.0f32, |a, c| a.max(c.abs()));
                let ulp = if far > 0.0 { far * f32::EPSILON } else { 0.0 };
                model_lines.push(format!(
                    "largest |coord| = {far:.1} m  →  f32 ULP {:.3} mm{}",
                    ulp * 1000.0,
                    if ulp > 1e-4 {
                        "   ⚠ FAR FROM ORIGIN — anything reading world position at texel or \
                         derivative scale MUST be rebased, and rebased PER VERTEX"
                    } else {
                        ""
                    }
                ));
            }
            None => model_lines.push("world AABB — empty model".into()),
        }
        let org = f.uv_rebase_origin();
        model_lines.push(format!(
            "uv_rebase_origin = ({:.1}, {:.1}, {:.1})",
            org[0], org[1], org[2]
        ));
        // THE PLAN'S UNIT, not the installed canvas's. A sketch document is `Document::default()`
        // — 1.0 m/unit, "Assumed" — so with a face open this reported the drawing's plan↔3D scale
        // as 1.0 when it was 0.001. `scene` exists to give measured truth instead of a guess, and
        // mm-vs-m is the live problem area it is most often pointed at, so it was misleading
        // exactly when it was being trusted.
        //
        // `doc_k()` itself is left alone: the sketch-lift paths depend on its current meaning.
        let plan = self.plan_doc();
        let u = plan.units.clone();
        model_lines.push(format!(
            "doc units: {:.6} m/unit ({:?})   ·   factory working unit: {}   ·   plan↔3D scale {:.6}{}",
            u.metres_per_unit, u.source, self.factory.units.label(), u.metres_per_unit,
            if self.factory.session.is_some() {
                "   ·   [sketch open: canvas is that plane's (u,v) at 1.0 m/unit]"
            } else { "" }));
        // A footprint hundreds of times the height is the signature of a wrong plan→3D scale: a
        // millimetre outline read as metres builds a thousand times too wide under a storey-height
        // extrusion, and the result draws as a sheet with nothing else on screen explaining it.
        // Stated here as a number, because that is the one place someone will look afterwards.
        if let Some((mn, mx)) = bounds {
            let footprint = (mx[0] - mn[0]).max(mx[1] - mn[1]);
            let h = mx[2] - mn[2];
            if h > 1e-6 && footprint / h > 200.0 {
                model_lines.push(format!(
                    "⚠ FOOTPRINT {:.0}× THE HEIGHT ({:.1} m across, {:.2} m tall) — the shape of a \
                     wrong plan↔3D scale. Check the units line above.",
                    footprint / h, footprint, h));
            }
        }
        sections.push(("model".into(), model_lines));

        // ── CUTS ─────────────────────────────────────────────────────────────────────────
        // A cut that stops inside the wall leaves solid material behind the glass, which
        // reads as "the window is not transparent" and as the wall's own texture speckling
        // through it. It is invisible in every other event: the FactoryOp that made it
        // reported success, and the depth only becomes wrong later. Printed with the
        // signature the broken thickness probe used to leave, so `repaircuts`' work — or the
        // absence of it — is legible at a glance.
        let mut cuts: Vec<&cad_solid::Feature> = f
            .model
            .features
            .iter()
            .filter(|x| x.op == BoolOp::Difference)
            .collect();
        // SUSPECTS FIRST, then by id. A cap that truncates in model order can hide the very
        // cuts the section exists to show — on the real project all ten fitted, but only by
        // luck. Ordering by what is wrong makes the cap safe instead of lucky.
        let is_stopped = |c: &cad_solid::Feature| {
            matches!(c.primitive,
                cad_solid::Primitive::Extrusion { h, .. } | cad_solid::Primitive::Box { h, .. } if h <= 0.25)
                && (c.placement.lift + 0.1).abs() <= 0.02
        };
        cuts.sort_by_key(|c| (!is_stopped(c), c.id));
        let mut cut_lines = Vec::new();
        for c in cuts.iter().take(Self::SCENE_LIST_MAX) {
            let h = match c.primitive {
                cad_solid::Primitive::Extrusion { h, .. } | cad_solid::Primitive::Box { h, .. } => {
                    Some(h)
                }
                _ => None,
            };
            let lift = c.placement.lift;
            let suspect = is_stopped(c);
            cut_lines.push(format!(
                "#{:<4} {:<10} depth={} lift={lift:+.3}{}",
                c.id,
                c.primitive.kind_label(),
                h.map_or("n/a".into(), |h| format!("{h:.3} m")),
                if suspect {
                    "   ⚠ STOPPED-CUT SIGNATURE (h≤0.25, lift≈−0.1) — run `repaircuts`"
                } else {
                    ""
                }
            ));
        }
        // Count the whole set, not only the printed page — a cap must never understate this.
        let stopped_all = cuts.iter().filter(|c| is_stopped(c)).count();
        if cuts.len() > Self::SCENE_LIST_MAX {
            cut_lines.push(format!(
                "… {} MORE OMITTED (cap {})",
                cuts.len() - Self::SCENE_LIST_MAX,
                Self::SCENE_LIST_MAX
            ));
        }
        cut_lines.push(format!(
            "→ {stopped_all} of {} cuts carry the stopped-cut signature",
            cuts.len()
        ));
        sections.push(("cuts".into(), cut_lines));

        // ── MATERIALS ────────────────────────────────────────────────────────────────────
        // Which path a surface takes through the shader is the first thing worth knowing
        // about a texture that looks wrong, and it was never recorded. Procedural vs pasted
        // and triplanar vs mesh-UV are DIFFERENT code paths with different failure modes, so
        // "the marble looks like bars" means nothing until you know which one drew it.
        // IN USE FIRST. A project accumulates clipboard leftovers — the real one carries 77
        // textures of which most are unused — and truncating in table order buries the handful
        // that are actually on screen behind dozens that are not. Ordering by use makes the cap
        // drop the irrelevant ones.
        let usage = |i: usize| {
            f.feature_texture.values().filter(|v| **v == i).count()
                + f.surface_texture.values().filter(|v| **v == i).count()
                + f.furniture
                    .iter()
                    .filter(|fu| fu.texture == Some(i))
                    .count()
        };
        let mut by_use: Vec<usize> = (0..f.textures.len()).collect();
        by_use.sort_by_key(|&i| (std::cmp::Reverse(usage(i)), i));
        let mut mat_lines = Vec::new();
        for &i in by_use.iter().take(Self::SCENE_MATERIALS_MAX) {
            let t = &f.textures[i];
            let feats = f.feature_texture.values().filter(|v| **v == i).count();
            let surfs = f.surface_texture.values().filter(|v| **v == i).count();
            let furn = f
                .furniture
                .iter()
                .filter(|fu| fu.texture == Some(i))
                .count();
            mat_lines.push(format!(
                "[{i:>2}] {:<22} {}  tiles/m={:.3} opacity={:.2} reflect={:.2} rot={:.0}° \
                 offset=({:.2},{:.2}) maps:{}{}  used by {feats} feat / {surfs} surf / {furn} furn",
                t.name,
                match &t.proc {
                    Some(p) => format!("PROCEDURAL {:?} (shader, world-space)", p.pattern),
                    None => format!("image {}×{}", t.w, t.h),
                },
                t.scale,
                t.opacity,
                t.reflect,
                t.rot_deg,
                t.offset[0],
                t.offset[1],
                if t.normal_map.is_some() { " nrm" } else { "" },
                if t.rough_map.is_some() { " rgh" } else { "" }
            ));
        }
        if f.textures.len() > Self::SCENE_MATERIALS_MAX {
            let rest: Vec<usize> = by_use
                .iter()
                .skip(Self::SCENE_MATERIALS_MAX)
                .copied()
                .collect();
            let used = rest.iter().filter(|&&i| usage(i) > 0).count();
            let procs = rest
                .iter()
                .filter(|&&i| f.textures[i].proc.is_some())
                .count();
            let transl = rest
                .iter()
                .filter(|&&i| f.textures[i].opacity < 0.999)
                .count();
            // Say what was dropped in the terms that matter, so the summary can still rule things
            // in or out: a procedural or see-through material is a different shader path.
            mat_lines.push(format!(
                "… {} more (cap {}), least-used first: {used} in use, {procs} procedural, \
                 {transl} see-through",
                rest.len(),
                Self::SCENE_MATERIALS_MAX
            ));
            // A material that is ON SCREEN must never be invisible here — the cap exists to stop
            // the dump truncating, not to hide the thing being investigated. Ids only, so it
            // stays one line however many there are.
            let used_ids: Vec<String> = rest
                .iter()
                .filter(|&&i| usage(i) > 0)
                .map(|i| i.to_string())
                .collect();
            if !used_ids.is_empty() {
                mat_lines.push(format!(
                    "    in use but not listed: [{}]",
                    used_ids.join(", ")
                ));
            }
        }
        if f.textures.is_empty() {
            mat_lines.push("(no textures — everything is flat-shaded)".into());
        }
        sections.push(("materials".into(), mat_lines));

        // ── FURNITURE ────────────────────────────────────────────────────────────────────
        // Instances are triangle soup, not CSG, so they appear in NO feature or body count.
        // A window that is a placed asset is invisible to every other section here.
        let mut furn_lines = Vec::new();
        // Translucent-vertex count PER ASSET, cached: the meshes run to half a million verts and
        // the same asset is placed many times over.
        let mut glassy: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for (i, inst) in f.furniture.iter().enumerate().take(Self::SCENE_LIST_MAX) {
            let a = f.furniture_lib.get(inst.asset);
            furn_lines.push(format!(
                "[{i:>2}] {:<24} tris={:<7} pos=({:.2},{:.2},{:.2}) rot=({:.0},{:.0},{:.0})° \
                 scale={:.3}{} tex={:?} cuts={}",
                a.map_or("<missing asset>", |a| a.name.as_str()),
                a.map_or(0, |a| a.positions.len() / 3),
                inst.pos[0],
                inst.pos[1],
                inst.pos[2],
                inst.rot[0],
                inst.rot[1],
                inst.rot[2],
                inst.scale,
                inst.fit.map_or(String::new(), |t| format!(
                    " fit=({:.2},{:.2},{:.2})",
                    t[0], t[1], t[2]
                )),
                inst.texture,
                inst.cuts.len()
            ));
            // WHICH PASS this piece draws in, which is the question `tex=None` raises and does
            // not answer. A piece with translucent triangles is peeled into the blended glass
            // pass — a different shader with different failure modes — and untextured pieces
            // never touch the textured program at all. Knowing that immediately is the
            // difference between fixing the right shader and fixing a neighbouring one.
            if let Some(a) = a {
                let glass = *glassy
                    .entry(inst.asset)
                    .or_insert_with(|| a.alpha.iter().filter(|v| **v < 0.999).count());
                let pass = match (
                    inst.texture.is_some() || !inst.surface_texture.is_empty(),
                    glass > 0,
                ) {
                    (true, true) => "textured + blended-glass",
                    (true, false) => "textured",
                    (false, true) => "FLAT + blended-glass (TRANSP program)",
                    (false, false) => "flat",
                };
                furn_lines.push(format!(
                    "       ↳ pass={pass}  translucent_verts={glass}  surface_tex={}",
                    inst.surface_texture.len()
                ));
            }
        }
        if f.furniture.len() > Self::SCENE_LIST_MAX {
            furn_lines.push(format!(
                "… {} MORE OMITTED (cap {})",
                f.furniture.len() - Self::SCENE_LIST_MAX,
                Self::SCENE_LIST_MAX
            ));
        }
        if f.furniture.is_empty() {
            furn_lines.push("(none placed)".into());
        }
        sections.push(("furniture".into(), furn_lines));

        // ── RENDER STATE + CAMERA ────────────────────────────────────────────────────────
        // Which passes were even switched on. Half of these can produce an artefact on their
        // own, and asking the user to list them one at a time is how a diagnosis turns into
        // a conversation. The camera is here because distance sets the depth resolution and
        // the pixel footprint — both of which decide whether a precision fault is visible.
        let s = &f.sun;
        sections.push(("render".into(), vec![
            format!("sun={} shadows={} cascades={} intensity={:.2} turbidity={:.1} sky_backdrop={}",
                s.enabled, s.shadows, s.shadow_cascades, s.intensity, s.turbidity, s.sky_backdrop),
            format!("ao={} gi={} ssr={} refract={} reflections={:.2}",
                s.ao.enabled, s.gi.enabled, s.ssr.enabled, s.refract.enabled, s.reflections),
            format!("env_map={} strength={:.2} rot={:.0}°  taa={}  clay={}",
                f.env_map.is_some(), f.env_strength, f.env_rot_deg, f.taa_samples, f.clay_mode),
            format!("hide_ceilings={} cutaway={} @ z={:.2}  plan_xray={} furn_outlines_2d={}",
                f.hide_ceilings, f.cutaway, f.cutaway_z, f.plan_xray, f.show_furniture_outlines_2d),
            format!("camera: target=({:.2},{:.2},{:.2}) dist={:.2} m yaw={:.1}° pitch={:.1}° ortho={}",
                f.cam_target[0], f.cam_target[1], f.cam_target[2],
                f.cam_dist, f.cam_yaw.to_degrees(), f.cam_pitch.to_degrees(), f.ortho),
        ]));

        // ── THE LIGHT STATE, which this dump reported NOTHING about. ─────────────────────────
        //
        // "why is the calculation not being made. i noticed the lights were also not being placed"
        // — and the answer was not in here. The dump described the building in detail and the
        // lighting not at all: no fitting count, no result, no mode, no status line. There are
        // three separate ways a calculation declines and each one writes a message, and the
        // message was the single thing a session recording could not show.
        //
        // The same lesson as the frame checkpoints: a question gets answered by guessing for as
        // long as the trace does not name the subsystem being asked about.
        let l = &self.light;
        let mut light = vec![
            format!(
                "fittings={} profiles={} mode={} rooms_calculated={} overlay_shown={}",
                l.luminaires.len(),
                l.profiles.len(),
                l.mode.label(),
                l.rooms.len(),
                l.show_overlay,
            ),
            format!(
                "result={}  stale={}  grid={} plane={}  calc_running={}",
                if l.results_fingerprint.is_some() {
                    "present"
                } else {
                    "NONE — nothing calculated"
                },
                l.results_stale,
                l.grid.is_some(),
                l.plane.is_some(),
                self.calc_rx.is_some(),
            ),
            // THE STATUS LINE IS THE ANSWER when a calculation refuses to run. "No geometry —
            // draw a closed room…" and "Nothing to calculate." are refusals that look, from
            // outside the app, exactly like a button that does nothing at all.
            format!("last_msg = {:?}", l.last_msg),
            format!(
                "cell={:.3} m plane_h={:.3} eye_h={:.3} wall_zone={:.3} room_h={:.3}",
                l.cell_size, l.plane_height, l.eye_height, l.wall_zone, l.room_height,
            ),
            format!(
                "scene_meshes={} surfaces={}",
                l.meshes.len(),
                l.materials.len()
            ),
        ];
        // The fittings themselves, because "not being placed" is a claim about this list. Capped:
        // a real job carries hundreds, and what is wanted is the count plus a readable sample.
        for (i, x) in l.luminaires.iter().take(12).enumerate() {
            light.push(format!(
                "  [{:2}] {} @ ({:.2},{:.2},{:.2}) rot={:.0}° tilt={:.0}° dim={:.2}{}",
                i,
                x.profile,
                x.position.x,
                x.position.y,
                x.position.z,
                x.rotation_deg,
                x.tilt_deg,
                x.dimming,
                if x.from_block.is_some() {
                    "  (follows a plan symbol)"
                } else {
                    ""
                },
            ));
        }
        if l.luminaires.len() > 12 {
            light.push(format!("  … {} more", l.luminaires.len() - 12));
        }
        if l.luminaires.is_empty() {
            light.push("  (NO fittings at all — placement is not reaching LightState)".into());
        }
        sections.push(("light".into(), light));

        let bodies = f
            .model
            .features
            .iter()
            .filter(|x| x.op == BoolOp::Union)
            .count();
        crate::dbg_recorder::DbgEvent::FactoryScene {
            reason:  reason.to_string(),
            summary: format!(
                "build={} ({})  features={} bodies={} cuts={} tris={} furniture={} textures={} dirty={}",
                option_env!("SIMLUX_BUILD_NO").unwrap_or("?"),
                option_env!("SIMLUX_BUILD").unwrap_or("unknown"),
                f.model.features.len(), bodies, cuts.len(),
                f.cached.positions.len() / 3, f.furniture.len(), f.textures.len(), f.dirty),
            sections,
        }
    }

    /// What the 3D model is actually made of — aimed squarely at surfaces that flicker or
    /// interleave as the camera moves.
    ///
    /// There is no depth bias anywhere in the renderer, so two surfaces at the SAME place have
    /// nothing to break the tie: which one wins is decided by floating-point noise and changes
    /// with the camera. That reads as a pattern crawling across a wall, and it is routinely
    /// mistaken for a texture. The usual cause is duplicated solids — most often a plan whose
    /// walls are drawn as two parallel face lines, where promoting BOTH lines yields two
    /// overlapping wall bodies with exactly coincident tops.
    ///
    /// Read-only: this reports, it never edits.
    pub(super) fn factory_geometry_report(&self) -> Vec<String> {
        let mut out = vec!["  ── geometry report ─────────────────────────".to_string()];

        // DO THE OPENINGS ACTUALLY GO THROUGH? Every cut, measured — not just the ones carrying
        // the old fallback's signature.
        //
        // This lives here rather than in the automatic scene capture because it samples every
        // opening against every body and costs a second on a real project; `diag` is typed
        // deliberately, so it can afford that. It exists because "the windows look fine now" and
        // "the cut is fixed" are different claims, and only one of them is checkable. A cutter
        // can carry a healthy-looking depth and still leave one side of a window blind.
        let cuts: Vec<&cad_solid::Feature> = self
            .factory
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Difference)
            .collect();
        if !cuts.is_empty() {
            let mut bad: Vec<String> = Vec::new();
            let (mut full, mut nowall, mut pockets) = (0usize, 0usize, 0usize);
            for c in &cuts {
                // A RECESS IS NOT A FAILED THROUGH-CUT. `cut_coverage` asks "does the cutter span
                // the wall?", which is the only question geometry alone can answer — and a pocket
                // fails it by construction, because stopping inside the wall is the whole point of
                // a pocket. Measured on the real project: 18 flagged, 6 of them recesses working
                // exactly as drawn. `Feature::through` records the intent at cut time so they can
                // be told apart; `None` is an opening cut before it existed, and those still go
                // through the inference below.
                if c.through == Some(false) {
                    pockets += 1;
                    continue;
                }
                let (ok, total, short) = self.cut_coverage(c);
                if total == 0 {
                    nowall += 1;
                } else if ok == total {
                    full += 1;
                } else {
                    bad.push(format!(
                        "      #{} reaches {ok}/{total} of the opening — short by {short:.3} m",
                        c.id
                    ));
                }
            }
            out.push(format!(
                "  openings: {full} go fully through, {} do not, {nowall} sit over no wall, \
                 {pockets} are recesses and are meant to stop (of {} cuts)",
                bad.len(),
                cuts.len()
            ));
            for b in bad.iter().take(12) {
                out.push(b.clone());
            }
            if bad.len() > 12 {
                out.push(format!("      … and {} more", bad.len() - 12));
            }
            if bad.is_empty() && nowall == 0 {
                out.push("  every opening spans its wall at every sampled point".into());
            }
        }
        let bodies: Vec<(u32, glam::Vec3, glam::Vec3)> = self
            .factory
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Union)
            .map(|f| {
                let (mn, mx) = f.world_aabb();
                (f.id, mn, mx)
            })
            .collect();
        out.push(format!(
            "  {} solid bodies, {} features, {} triangles, {} furniture",
            bodies.len(),
            self.factory.model.features.len(),
            self.factory.cached.positions.len() / 3,
            self.factory.furniture.len(),
        ));

        // COINCIDENT FACES, not overlapping boxes. Box overlap is a poor signal here: a floor
        // slab's box legitimately contains every wall standing on it, so it reports "100%
        // overlap" for geometry that is perfectly fine. What actually flickers is two
        // TRIANGLES on the same plane belonging to DIFFERENT bodies, so that is what is
        // counted: each triangle's plane is quantised (normal folded so n and −n agree, since
        // two solids meeting face to face share a plane) and planes carrying more than one
        // body are the fighting ones.
        // Being on the same plane is NOT enough. A floor slab and the ceiling above it share
        // the same outline, so their perimeter side faces lie in the same VERTICAL planes —
        // at different heights, where they never overlap and never fight. Counting those gave
        // a confident, entirely wrong answer. A pair only counts when their triangles on that
        // plane actually occupy the same region of it, so each face carries its own box.
        use std::collections::HashMap;
        type Face = (u32, glam::Vec3, glam::Vec3); // body id + world AABB of the triangle
        let mut planes: HashMap<(i32, i32, i32, i32), Vec<Face>> = HashMap::new();
        for f in self
            .factory
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Union)
        {
            for c in self
                .factory
                .model
                .feature_world_positions(f)
                .chunks_exact(3)
            {
                let (a, b, cc) = (
                    glam::Vec3::from(c[0]),
                    glam::Vec3::from(c[1]),
                    glam::Vec3::from(c[2]),
                );
                let n = (b - a).cross(cc - a).normalize_or_zero();
                if n.length_squared() < 0.5 {
                    continue;
                }
                let n = if n.x + n.y + n.z < 0.0 { -n } else { n };
                let d = n.dot(a);
                let key = (
                    (n.x * 200.0).round() as i32,
                    (n.y * 200.0).round() as i32,
                    (n.z * 200.0).round() as i32,
                    (d * 500.0).round() as i32,
                );
                let mn = a.min(b).min(cc);
                let mx = a.max(b).max(cc);
                planes.entry(key).or_default().push((f.id, mn, mx));
            }
        }
        let mut pair_hits: HashMap<(u32, u32), usize> = HashMap::new();
        for faces in planes.values() {
            for i in 0..faces.len() {
                for j in (i + 1)..faces.len() {
                    let (ia, imn, imx) = faces[i];
                    let (jb, jmn, jmx) = faces[j];
                    if ia == jb {
                        continue; // one body's own faces meeting edge-on is not a fight
                    }
                    // Real overlap IN the plane, not merely the same plane.
                    let omn = imn.max(jmn);
                    let omx = imx.min(jmx);
                    let ov = omx - omn;
                    // Two of the three axes must overlap by a real area; the third is the
                    // plane's own thickness, which is ~0 by construction.
                    let mut spans: Vec<f32> = vec![ov.x, ov.y, ov.z];
                    spans.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                    if spans[0] > 0.01 && spans[1] > 0.01 {
                        let k = if ia < jb { (ia, jb) } else { (jb, ia) };
                        *pair_hits.entry(k).or_default() += 1;
                    }
                }
            }
        }
        let mut ranked: Vec<_> = pair_hits.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1));

        // EXACTLY what `factory_dedupe_solids` treats as a duplicate, so the report and the
        // command can never disagree about the word. `None` = no such body.
        let dedupe_sig = |id: u32| -> Option<String> {
            self.factory
                .model
                .features
                .iter()
                .find(|f| f.id == id)
                .map(|f| format!("{:?}|{:?}|{:?}", f.plane, f.placement, f.primitive))
        };
        let describe = |id: u32| -> String {
            self.factory
                .model
                .features
                .iter()
                .find(|f| f.id == id)
                .map_or("?".into(), |f| {
                    let (mn, mx) = f.world_aabb();
                    let d = mx - mn;
                    format!(
                        "{} {:.1}×{:.1}×{:.1} m",
                        f.primitive.kind_label(),
                        d.x,
                        d.y,
                        d.z
                    )
                })
        };

        if ranked.is_empty() {
            out.push("  no coincident faces between bodies — nothing should flicker".into());
        } else {
            out.push(format!(
                "  ⚠ {} body pair(s) share faces on the SAME plane.",
                ranked.len()
            ));
            out.push("    There is no depth bias in the renderer, so coincident faces have".into());
            out.push("    nothing to break the tie: which one shows flips as the camera".into());
            out.push("    moves. That is the pattern that looks like texture on a surface.".into());
            for ((a, b), n) in ranked.iter().take(8) {
                // Say only what is known. This used to print "← SAME SIZE: a duplicate" whenever
                // two bodies shared a KIND and a bounding box rounded to 0.1 m — which is not
                // what `dedupe` means by a duplicate at all. `dedupe` requires an exact match of
                // plane, placement and primitive, so it correctly refused a pair `diag` had just
                // called a duplicate, and the two tools flatly contradicted each other in front
                // of the user. Use `dedupe`'s own test, and when it does not hold, say what the
                // pair actually is and who has to deal with it.
                let dup = if dedupe_sig(*a).is_some() && dedupe_sig(*a) == dedupe_sig(*b) {
                    "  ← EXACT TWIN: `dedupe` removes it"
                } else if describe(*a) == describe(*b) {
                    "  ← same size, different definition — `dedupe` will NOT touch it"
                } else {
                    ""
                };
                out.push(format!("      #{a} vs #{b}: {n} coincident faces{dup}"));
                out.push(format!("         {}  |  {}", describe(*a), describe(*b)));
            }
            if ranked.len() > 8 {
                out.push(format!("      … and {} more pair(s)", ranked.len() - 8));
            }
            out.push(
                "    `dedupe` removes EXACT twins only. Anything marked otherwise is two".into(),
            );
            out.push(
                "    different solids sharing space — decide which is redundant and delete".into(),
            );
            out.push("    it in the 3D view; nothing can make that call for you.".into());
        }
        out
    }

    /// The nearest Union surface along `dir` as `(distance, leaving)`, or `None` if there is none
    /// within `max`. Unlike [`Self::assembly_span`] there is no gap rule here: this is looking for
    /// a surface that may be some way off, which is the very case the gap rule exists to reject.
    ///
    /// `leaving` is the part that matters to callers. If the first crossing is one the ray EXITS
    /// through, the probe was already inside material and the distance is to the far side — so
    /// anchoring at it would skip the whole wall and start the cut beyond it.
    pub(super) fn nearest_surface(
        &self,
        origin: glam::Vec3,
        dir: glam::Vec3,
        max: f32,
    ) -> Option<(f32, bool)> {
        let start = origin + dir * 1e-3;
        let mut best: Option<(f32, bool)> = None;
        for f in &self.factory.model.features {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            for c in self
                .factory
                .model
                .feature_world_positions(f)
                .chunks_exact(3)
            {
                let (a, b, cc) = (
                    glam::Vec3::from(c[0]),
                    glam::Vec3::from(c[1]),
                    glam::Vec3::from(c[2]),
                );
                if let Some(t) = cad_solid::ray_triangle(start, dir, a, b, cc) {
                    if t > 1e-4 && t <= max && best.is_none_or(|(z, _)| t < z) {
                        best = Some((t, (b - a).cross(cc - a).dot(dir) > 0.0));
                    }
                }
            }
        }
        best
    }

    /// Ray-march EVERY Union body from `origin` along `dir` and return `(depth, body_ids)` —
    /// how far solid material runs before the first big interior gap (`GAP`), plus the ids of
    /// every body in that run. Called once per direction (inward and outward) so a THROUGH
    /// cut spans the whole local wall assembly regardless of which face was picked, while the
    /// per-side gap keeps it from reaching the wall across the room. Reuses the kernel's
    /// `ray_triangle`, the same primitive the 3D picker uses.
    pub(super) fn assembly_span(&self, origin: glam::Vec3, dir: glam::Vec3) -> (f32, Vec<u32>) {
        const GAP: f32 = 0.4; // an empty span bigger than this = the room → stop
        let start = origin + dir * 1e-3;
        // Every surface crossing along the ray, tagged with the body it belongs to and with
        // whether the ray is LEAVING material there (the triangle faces the same way the ray
        // travels) or arriving at it.
        let mut hits: Vec<(f32, u32, bool)> = Vec::new();
        for f in &self.factory.model.features {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            let tris = self.factory.model.feature_world_positions(f);
            for c in tris.chunks_exact(3) {
                let (a, b, cc) = (
                    glam::Vec3::from(c[0]),
                    glam::Vec3::from(c[1]),
                    glam::Vec3::from(c[2]),
                );
                if let Some(t) = cad_solid::ray_triangle(start, dir, a, b, cc) {
                    if t > 1e-4 {
                        let exiting = (b - a).cross(cc - a).dot(dir) > 0.0;
                        hits.push((t, f.id, exiting));
                    }
                }
            }
        }
        hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        // Walk from the face outward; a gap larger than GAP means we have left the assembly.
        //
        // A GAP only exists between LEAVING material and ENTERING it again. The run between an
        // entry and the following exit is the inside of a wall, however thick, and must never
        // be measured against `GAP` — doing that is what made a through-cut stop short:
        //
        //   * a probe sitting ON the face starts INSIDE, so the very first crossing is the far
        //     surface. Judged as a gap, any wall thicker than 400 mm reported a thickness of
        //     ZERO, the cutter fell back to the bare ±0.1 m margin, and the opening never
        //     reached the far side — while still reporting success.
        //   * a probe sitting just OFF a curved face starts OUTSIDE. The march would then stop
        //     at the near surface and report the width of the air gap instead of the wall.
        //
        // Whether the ray began inside is not assumed: it is read from the first crossing —
        // a face the ray is LEAVING can only have been entered before the march started.
        // The march, shared by both passes below.
        let march = |hits: Vec<(f32, u32, bool)>| -> (f32, Vec<u32>) {
            let mut depth = 0.0_f32;
            let mut prev = 0.0_f32;
            let mut ids: Vec<u32> = Vec::new();
            let mut inside: Option<bool> = None;
            for (t, id, exiting) in hits {
                let was_inside = inside.unwrap_or(exiting);
                if !was_inside && t - prev > GAP {
                    break;
                }
                prev = t;
                depth = t;
                if !ids.contains(&id) {
                    ids.push(id);
                }
                inside = Some(!exiting);
            }
            (depth, ids)
        };
        // IDS come from the feature walk above — which bodies lie along the ray is what the
        // cutter needs in order to target them.
        let (feature_depth, ids) = march(hits);

        // DEPTH comes from the EVALUATED solid, not from the raw Union features.
        //
        // Walking Unions alone measures geometry as it was BEFORE any Difference was applied, so a
        // room carved out of a building is invisible to it: the probe crossed the building's
        // un-carved 4.4 m box and reported a 4.4 m wall, and the window fitted to that was stretched
        // 36x in depth — right across the building, which is how this was reported.
        //
        // It only ever worked by accident. A room used to build its own 200 mm wall boxes, and the
        // probe hit one of those first; the moment a carved room stopped adding them, the reading
        // was the whole building. The evaluated mesh is the material that is actually there, voids
        // and all, so it answers the question that was being asked all along.
        let mut solid: Vec<(f32, u32, bool)> = Vec::new();
        for c in self.factory.cached.positions.chunks_exact(3) {
            let (a, b, cc) = (
                glam::Vec3::from(c[0]),
                glam::Vec3::from(c[1]),
                glam::Vec3::from(c[2]),
            );
            if let Some(t) = cad_solid::ray_triangle(start, dir, a, b, cc) {
                if t > 1e-4 {
                    let exiting = (b - a).cross(cc - a).dot(dir) > 0.0;
                    solid.push((t, 0, exiting));
                }
            }
        }
        if solid.is_empty() {
            // Nothing evaluated yet — the mesh is rebuilt lazily. Fall back to the feature reading
            // rather than reporting a wall of zero thickness, which reads as "no wall here" and
            // makes the cutter give up entirely.
            return (feature_depth, ids);
        }
        solid.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        (march(solid).0, ids)
    }

    /// DRAW3D dialog — the controllers for the primitive being created.
    ///
    /// The dialog OWNS its parameters (`factory.draw3d`) and nothing touches the model
    /// until **Create** is pressed: csgrs walks a BSP per boolean, so re-evaluating on
    /// every keystroke is the known lag source. Segment/stack counts are surfaced as
    /// first-class "accuracy" controllers, not hidden constants.
    pub(super) fn render_draw3d_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.factory.draw3d.clone() else {
            return;
        };

        // ── EDIT vs CREATE ───────────────────────────────────────────────
        // Exactly one solid selected → the SAME controllers edit THAT solid live: the
        // dialog loads its real dimensions and every tweak flows back to the feature.
        // `Create` stays available (it always makes a NEW independent solid), so there
        // is no mode to get stuck in — the fields simply mirror "the current solid"
        // when one is picked, and act as the template for Create when none is.
        // An Extrusion is deliberately NOT bindable: its shape lives in the shared profile
        // table, not in these controllers, so binding it would let a slider rewrite a
        // building outline into a Box. Selecting one simply leaves the dialog as a
        // template for Create.
        let edit_id: Option<u32> = match self.factory.selection.as_slice() {
            [only] => self
                .factory
                .model
                .features
                .iter()
                .find(|f| f.id == *only)
                .filter(|f| !matches!(f.primitive, cad_solid::Primitive::Extrusion { .. }))
                .map(|f| f.id),
            _ => None,
        };
        if let Some(id) = edit_id {
            // Load the picked solid's dimensions only when the binding CHANGES — never
            // every frame, or it would stomp the edits the user is making right now.
            if self.factory.draw3d_edit != Some(id) {
                if let Some(f) = self.factory.model.features.iter().find(|f| f.id == id) {
                    dlg.load_from(&f.primitive);
                    self.factory.draw3d_edit = Some(id);
                }
            }
        } else {
            self.factory.draw3d_edit = None;
        }

        let mut open = true; // the window's own ✕
        let mut close_btn = false; // our Close button (separate: egui holds `open`)
        let mut create = false;
        let changed = std::cell::Cell::new(false); // did any dimension control change this frame?
        let title = match edit_id {
            Some(id) => format!("{}  {} · editing #{id}", dlg.kind.icon(), dlg.kind.label()),
            None => format!("{}  {}", dlg.kind.icon(), dlg.kind.label()),
        };
        egui::Window::new(title)
            .id(egui::Id::new("draw3d_dialog")) // stable id ⇒ the window keeps its place when the title changes
            .open(&mut open)
            .resizable(false)
            .default_width(320.0)
            .show(ctx, |ui| {
                // ── the shape's own controllers ─────────────────────────────
                egui::Grid::new("draw3d_params")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        // Every dimension is a SLIDER (drag for a feel of the size) paired with
                        // a DragValue (type any value, unbounded past the 20 m drag range) — the
                        // same control the Hatch dialog uses. One helper ⇒ every primitive's
                        // L/W/H/radius gets the slider, which is what "all these dialogs" means.
                        let ulen = self.factory.units.clone();
                        let mut len = |ui: &mut egui::Ui, label: &str, v: &mut f32, min: f32| {
                            ui.label(label);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().slider_width = 132.0;
                                // The slider spans the same physical 0..20 m whatever the unit is
                                // named in; only the numbers printed on it change.
                                let mut shown = ulen.from_metres(*v as f64);
                                let s = ui.add(
                                    egui::Slider::new(
                                        &mut shown,
                                        ulen.from_metres(min as f64)..=ulen.from_metres(20.0),
                                    )
                                    .show_value(false),
                                );
                                if s.changed() {
                                    *v = ulen.to_metres(shown) as f32;
                                }
                                let d = crate::factory::length_ui(
                                    ui, &ulen, v, 0.05, min as f64, 1e4, &self.calc,
                                );
                                if s.changed() || d.changed() {
                                    changed.set(true);
                                }
                            });
                            ui.end_row();
                        };
                        match dlg.kind {
                            crate::factory::Draw3dKind::Box => {
                                len(ui, "width", &mut dlg.w, 0.001);
                                len(ui, "depth", &mut dlg.d, 0.001);
                                len(ui, "height", &mut dlg.h, 0.001);
                            }
                            crate::factory::Draw3dKind::Sphere => {
                                len(ui, "radius", &mut dlg.r, 0.001)
                            }
                            crate::factory::Draw3dKind::Cylinder => {
                                len(ui, "radius", &mut dlg.r, 0.001);
                                len(ui, "height", &mut dlg.h, 0.001);
                            }
                            crate::factory::Draw3dKind::Cone => {
                                len(ui, "bottom radius", &mut dlg.r, 0.001);
                                len(ui, "top radius", &mut dlg.r_top, 0.0);
                                len(ui, "height", &mut dlg.h, 0.001);
                            }
                            crate::factory::Draw3dKind::Prism
                            | crate::factory::Draw3dKind::Pyramid => {
                                len(ui, "radius", &mut dlg.r, 0.001);
                                len(ui, "height", &mut dlg.h, 0.001);
                            }
                            crate::factory::Draw3dKind::Capsule => {
                                len(ui, "radius", &mut dlg.r, 0.001);
                                len(ui, "length (barrel)", &mut dlg.h, 0.0);
                            }
                            crate::factory::Draw3dKind::Torus => {
                                len(ui, "major radius (ring)", &mut dlg.major_r, 0.001);
                                len(ui, "minor radius (tube)", &mut dlg.minor_r, 0.001);
                            }
                            crate::factory::Draw3dKind::Tube => {
                                len(ui, "outer radius", &mut dlg.r, 0.001);
                                len(ui, "inner radius", &mut dlg.r_inner, 0.0);
                                len(ui, "height", &mut dlg.h, 0.001);
                            }
                            crate::factory::Draw3dKind::Ellipsoid => {
                                len(ui, "radius X", &mut dlg.rx, 0.001);
                                len(ui, "radius Y", &mut dlg.ry, 0.001);
                                len(ui, "radius Z", &mut dlg.rz, 0.001);
                            }
                        }
                    });

                // ── accuracy controllers (tessellation) ─────────────────────
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Accuracy — tessellation")
                        .small()
                        .color(egui::Color32::from_rgb(150, 165, 185)),
                );
                egui::Grid::new("draw3d_tess")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        let mut cnt = |ui: &mut egui::Ui, label: &str, v: &mut u32, min: u32| {
                            ui.label(label);
                            if ui
                                .add(
                                    egui::DragValue::new(v)
                                        .update_while_editing(false)
                                        .speed(1.0)
                                        .range(min..=512),
                                )
                                .changed()
                            {
                                changed.set(true);
                            }
                            ui.end_row();
                        };
                        use crate::factory::Draw3dKind as K;
                        match dlg.kind {
                            K::Box => {
                                ui.label(egui::RichText::new("(flat faces — none)").small().weak());
                                ui.end_row();
                            }
                            K::Sphere | K::Capsule | K::Ellipsoid => {
                                cnt(ui, "segments (longitude)", &mut dlg.segments, 3);
                                cnt(ui, "stacks (latitude)", &mut dlg.stacks, 2);
                            }
                            K::Cylinder | K::Cone | K::Tube => {
                                cnt(ui, "radial segments", &mut dlg.segments, 3);
                            }
                            K::Prism | K::Pyramid => {
                                cnt(ui, "sides", &mut dlg.sides, 3);
                            }
                            K::Torus => {
                                cnt(ui, "major segments", &mut dlg.seg_major, 3);
                                cnt(ui, "minor segments", &mut dlg.seg_minor, 3);
                            }
                        }
                    });

                ui.separator();
                let problem = dlg.problem();
                if let Some(p) = problem {
                    ui.label(
                        egui::RichText::new(format!("⚠ {p}"))
                            .color(egui::Color32::from_rgb(255, 158, 30)),
                    );
                }
                if edit_id.is_some() {
                    ui.label(
                        egui::RichText::new("editing the selected solid — changes apply live")
                            .small()
                            .color(egui::Color32::from_rgb(120, 200, 140)),
                    );
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(problem.is_none(), egui::Button::new("✚  Create"))
                        .clicked()
                    {
                        create = true;
                    }
                    if ui.button("Close").clicked() {
                        close_btn = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} feature(s)",
                                self.factory.feature_count()
                            ))
                            .small()
                            .weak(),
                        );
                    });
                });
            });

        // EDIT MODE: a dimension changed → rebuild the primitive and write it back to
        // the selected feature. Recompute is deferred to render_factory_panel (idle
        // only, never mid-drag — the existing csgrs perf guard), so dragging a slider
        // resizes the solid on release rather than re-evaluating the CSG per frame.
        if let (Some(id), true) = (edit_id, changed.get()) {
            let p = dlg.build();
            if let Some(f) = self.factory.model.get_mut(id) {
                f.primitive = p;
            }
            self.factory.dirty = true;
        }
        if create {
            // Create no longer drops the solid at the origin — it ARMS a placement: the
            // next click in the 3D view sets the point (a Box's near corner / others
            // centred). The dialog stays open so you can place several.
            let p = dlg.build();
            self.factory.open = true;
            self.factory.place_pending = Some(p);
            self.factory.status = format!(
                "Place {} — click a point in the 3D view  [Esc cancels]",
                dlg.kind.label()
            );
        }
        self.factory.draw3d = if open && !close_btn { Some(dlg) } else { None };
    }

    /// 3D FACTORY viewport — the sandbox's 3D view, now a panel in the real app.
    /// Reuses `light3d::{mvp, Scene3dRenderer}` (the sandbox had duplicated both) and
    /// renders a `cad_solid::Model`. Docked right, mirroring the SIMLUX 3D panel.
    pub(super) fn render_factory_panel(&mut self, ctx: &egui::Context) {
        if !self.factory.open {
            return;
        }
        // csgrs is expensive — only re-evaluate when idle, never mid-drag.
        if self.factory.dirty && !ctx.is_using_pointer() {
            self.factory.recompute();
        }
        let mut open = self.factory.open;
        // Bounded for the same reason the SIMLUX panel is: this one is laid out SECOND, so egui
        // already clamps it to what is left — but "what is left" can be the whole window with the
        // 2D canvas at zero width. The canvas keeps `MIN_VIEW_W`, and only while it is OPEN: with
        // the 2D view closed this panel is free to fill everything the SIMLUX panel is not using.
        let keep_2d = if self.two_d_open { MIN_VIEW_W } else { 0.0 };
        let factory_max = (ctx.available_rect().width() - keep_2d).max(MIN_VIEW_W);
        // MODE ENTRY PIN: the 3D Factory workspace was just entered (tab bar)
        // → pin the panel to the whole window for this one frame, so it opens
        // maximised instead of at whatever width a past drag left stored for
        // this panel id. See `factory_full_pin`.
        let pin_full = self.factory_full_pin;
        self.factory_full_pin = false;
        let base = egui::SidePanel::right("factory_3d_panel")
            .min_width(240.0)
            .max_width(factory_max)
            .resizable(true)
            .default_width(420.0);
        let base = if pin_full {
            base.exact_width(factory_max)
        } else {
            base
        };
        base.show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("3D Factory");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("✕").on_hover_text("Close 3D Factory view").clicked() {
                            open = false;
                        }
                        ui.label(
                            egui::RichText::new("orbit: mid / Alt+R · pan: Shift+L · zoom: scroll")
                                .small()
                                .weak(),
                        )
                        .on_hover_text(
                            "Orbit — middle-drag, or Alt + right-drag, or drag the nav cube.\n\
                             Pan — Shift + left-drag.\n\
                             Zoom — scroll wheel.",
                        );
                    });
                });
                ui.separator();
                // SKETCH BANNER — while a sketch is open the app's whole 2D canvas IS
                // this plane, so say so loudly and give a way out.
                if self.factory.session.is_some() {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(52, 38, 12))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 158, 30)))
                        .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                        .rounding(4.0)
                        .show(ui, |ui| {
                            let editing_cutout = self.factory.editing_cutout();
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(if editing_cutout {
                                        "✎ reshaping opening"
                                    } else {
                                        "✎ drafting on plane"
                                    })
                                    .color(egui::Color32::from_rgb(255, 178, 60))
                                    .strong(),
                                );
                                if editing_cutout {
                                    // The one button that actually applies a dragged-point edit:
                                    // re-cut the opening AS ITSELF and return to 3D.
                                    if ui
                                        .button(egui::RichText::new("✔ Apply reshape").strong())
                                        .on_hover_text("Re-cut the opening with the edited outline — through if it went through, at its own depth if it was a recess — then show it in 3D")
                                        .clicked()
                                    {
                                        self.factory_apply_cutout_reshape();
                                    }
                                    if ui
                                        .button("✖ Cancel")
                                        .on_hover_text("Discard the reshape and leave the opening as it was")
                                        .clicked()
                                    {
                                        // Leaving the sketch IS the restore now — the stash goes
                                        // back behind each cutter's own body, on this route and
                                        // on every other way out.
                                        //
                                        // This used to also `do_undo()`, which was the only thing
                                        // putting the opening back. That is worse than redundant
                                        // now: an undo step is popped blind, and any 3D edit made
                                        // DURING the sketch is on top of it — so Cancel would undo
                                        // that instead, and still leave the opening deleted.
                                        self.factory_exit_sketch();
                                        self.active_view = ActiveView::ThreeD;
                                    }
                                } else if ui.button("✔ Finish sketch").clicked() {
                                    self.factory_exit_sketch();
                                }
                            });
                            ui.label(
                                egui::RichText::new(if editing_cutout {
                                    "drag the opening's corner points to resize/reshape it, then ✔ Apply reshape"
                                } else {
                                    "the 2D canvas is this plane — full toolset (try: f → r → 10)"
                                })
                                .small()
                                .weak(),
                            );
                            // ROOM ELEMENTS — turn the drawn shape into geometry on this face.
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("height / depth").small().weak());
                                let u = self.factory.units.clone();
                                crate::factory::length_ui(ui, &u, &mut self.factory.element_height, 0.05, 0.001, 1e4, &self.calc);
                                if ui
                                    .button("⬆ Extrude")
                                    .on_hover_text("Extrude the drawn closed shape into a solid element on this face")
                                    .clicked()
                                {
                                    self.factory_extrude_sketch(false);
                                }
                                if ui
                                    .button("◳ Cut through")
                                    .on_hover_text("Cut the drawn shape ALL THE WAY THROUGH the wall/solid (window, door)")
                                    .clicked()
                                {
                                    self.factory_cut_sketch(true);
                                }
                                if ui
                                    .button("◱ Recess")
                                    .on_hover_text("Cut a blind pocket of the given DEPTH into the solid (niche, reveal) — not through")
                                    .clicked()
                                {
                                    self.factory_cut_sketch(false);
                                }
                                if ui
                                    .button("🪑 As furniture")
                                    .on_hover_text("Extrude the shape as a free-standing furniture solid — never cuts the building")
                                    .clicked()
                                {
                                    self.factory_extrude_sketch(true);
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("path sweep").small().weak());
                                if ui
                                    .button("〰 Path extrude")
                                    .on_hover_text("Draw the cross-section on a face → Enter → pick a perpendicular view → draw the path → Enter. Sweeps the section along the path.")
                                    .clicked()
                                {
                                    self.factory_begin_sweep_flow(false, false);
                                }
                                if ui
                                    .button("〰 Path cut")
                                    .on_hover_text("Draw the cross-section on a face → Enter → pick a perpendicular view → draw the path → Enter. Cuts a channel along the path.")
                                    .clicked()
                                {
                                    self.factory_begin_sweep_flow(true, false);
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut self.factory.keep_sketch, "keep shape")
                                    .on_hover_text("Keep the drawing after Extrude/Cut so you can act on the SAME outline again (e.g. recess + through)");
                                // In the banner, because it is a property of the sketch you are in
                                // and this is the one place that is only ever on screen while you
                                // are in one.
                                ui.checkbox(&mut self.factory.show_other_planes, "other planes")
                                    .on_hover_text(
                                        "Show what is drawn on the OTHER planes, projected onto \
                                         this one. Off by default: a face sketch is its own \
                                         drawing, and another plane's work here is reference you \
                                         cannot select.",
                                    );
                                ui.label(
                                    egui::RichText::new("· draw a CLOSED shape, then act on it")
                                        .small()
                                        .weak(),
                                );
                            });
                        });
                    ui.separator();
                }
                // The 3D Factory's OWN toolbar — the build tools live HERE, in the panel,
                // not up in the main menu bar. Two dropdowns:
                //   3D solids  — every parametric primitive (opens the Draw3D dialog)
                //   Building   — the plan→3D actions (walls / building / slabs)
                // plus Frame and Clear. Both dropdowns drive the SAME code the main-menu
                // rows do, so there is one implementation, not two.
                ui.horizontal_wrapped(|ui| {
                    // WORKING UNIT — what every length in the Factory is typed and shown in.
                    //
                    // First on the bar because it is the first decision: a building is dimensioned
                    // in millimetres on the drawings it comes from, and typing 2700 for a wall is
                    // what the person doing it expects. Changing it NEVER moves geometry — the
                    // model is stored in metres either way — so it is safe to switch at any point
                    // and safe to switch back.
                    let cur = self.factory.units.clone();
                    click_menu_button(ui, format!("▼ {}", cur.label()), |ui| {
                        ui.label(egui::RichText::new("working unit — what you type in").small().weak());
                        for (label, m) in [
                            ("millimetres  mm", cad_kernel::Units::MM),
                            ("centimetres  cm", cad_kernel::Units::CM),
                            ("metres  m", cad_kernel::Units::M),
                            ("inches  in", cad_kernel::Units::INCH),
                            ("feet  ft", cad_kernel::Units::FOOT),
                        ] {
                            let on = (cur.metres_per_unit - m).abs() < 1e-9;
                            if ui.selectable_label(on, label).clicked() {
                                self.factory.units =
                                    cad_kernel::Units::from_metres_per_unit(m, cad_kernel::UnitSource::User);
                                self.factory.status = format!(
                                    "Working unit is now {}. Nothing moved — the model is stored in \
                                     metres and only the numbers you type and read have changed.",
                                    self.factory.units.label()
                                );
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new(
                                "Geometry is always stored in metres.\nSwitching units never moves anything.",
                            )
                            .small()
                            .weak(),
                        );
                    })
                    .response
                    .on_hover_text("The unit every length in the 3D Factory is typed and shown in");


                    ui.separator();
                    click_menu_button(ui, "▼ 3D solids", |ui| {
                        for k in crate::factory::Draw3dKind::ALL {
                            if ui.button(format!("{}  {}", k.icon(), k.label())).clicked() {
                                // Same as the main-menu path: open the dialog; nothing is
                                // built until Create (csgrs walks a BSP per boolean).
                                self.factory.draw3d =
                                    Some(crate::factory::Draw3dDialog::new(k));
                                ui.close_menu();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Parametric 3D primitives — opens the Draw3D dialog");

                    click_menu_button(ui, "▼ Building", |ui| {
                        // Live count, so it is obvious WHY a row might report "select
                        // something first".
                        let sel = self.selection.len();
                        ui.label(
                            egui::RichText::new(if sel == 0 {
                                "  no 2D selection".to_string()
                            } else {
                                format!("  {sel} object(s) selected")
                            })
                            .small()
                            .weak(),
                        );
                        ui.separator();
                        if ui.button("⌂  Make building (from outline)").clicked() {
                            self.do_make_building();
                            ui.close_menu();
                        }
                        if ui.button("⬒  Make 3D walls").clicked() {
                            self.do_make_3d_wall();
                            ui.close_menu();
                        }
                        // New-wall defaults — height + thickness given to promoted 2D
                        // geometry that carries none of its own. Type-in fields, right
                        // where walls are created (the old always-on sliders are gone).
                        ui.horizontal(|ui| {
                            ui.add_sized([70.0, 18.0], egui::Label::new(egui::RichText::new("  new wall h").small().weak()));
                            { let u = self.factory.units.clone(); crate::factory::length_ui(ui, &u, &mut self.factory.wall_height, 0.02, 0.3, 50.0, &self.calc); }
                        });
                        ui.horizontal(|ui| {
                            ui.add_sized([70.0, 18.0], egui::Label::new(egui::RichText::new("  new wall t").small().weak()));
                            { let u = self.factory.units.clone(); crate::factory::length_ui(ui, &u, &mut self.factory.wall_thickness, 0.01, 0.02, 2.0, &self.calc); }
                        });
                        if ui.button("⬓  Make floor").clicked() {
                            self.do_make_slab(true);
                            ui.close_menu();
                        }
                        if ui.button("⬒  Make ceiling").clicked() {
                            self.do_make_slab(false);
                            ui.close_menu();
                        }
                    })
                    .response
                    .on_hover_text(
                        "Turn the selected 2D geometry into 3D — walls, a building mass, \
                         floors and ceilings",
                    );

                    // ▼ ROOMS — every room built, editable after the fact.
                    //
                    // A room used to be unreachable once drawn: its height was fixed at the moment
                    // of creation and the only way to change it was to delete every piece and draw
                    // it again, losing any window already cut into its walls.
                    {
                        let n = self.factory.rooms.len();
                        click_menu_button(ui, format!("▼ Rooms ({n})"), |ui| {
                            if n == 0 {
                                ui.label(
                                    egui::RichText::new("No rooms yet.\nDraw a closed outline, then ▼ Room → Make room.")
                                        .small()
                                        .weak(),
                                );
                                return;
                            }
                            ui.label(
                                egui::RichText::new(
                                    "⌂ built · ◫ plan · ⬚ layer — all rooms; all are lux targets",
                                )
                                .small()
                                .weak(),
                            );
                            let u = self.factory.units.clone();
                            let mut rename: Option<(u32, String)> = None;
                            let mut set_h: Option<(u32, f32)> = None;
                            let mut select: Option<u32> = None;
                            let mut remove: Option<u32> = None;
                            let mut set_floor: Option<(u32, f32)> = None;
                            let mut set_ceil: Option<(u32, f32)> = None;
                            let mut fit: Option<(u32, f32)> = None;
                            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                                for r in &self.factory.rooms {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(format!("{}", r.origin.glyph()))
                                                .size(11.0),
                                        )
                                        .on_hover_text(r.origin.label());
                                        let mut name = r.name.clone();
                                        if ui
                                            .add(
                                                egui::TextEdit::singleline(&mut name)
                                                    .desired_width(120.0)
                                                    .hint_text("room name"),
                                            )
                                            .changed()
                                        {
                                            rename = Some((r.id, name));
                                        }
                                        let mut h = r.height;
                                        if crate::factory::length_ui(ui, &u, &mut h, 0.02, 0.3, 30.0, &self.calc)
                                            .on_hover_text("Clear height — floor top to ceiling underside. The slabs are extra.")
                                            .changed()
                                        {
                                            set_h = Some((r.id, h));
                                        }
                                        if ui.small_button("◎").on_hover_text("Select this room's geometry").clicked() {
                                            select = Some(r.id);
                                        }
                                        if ui.small_button("🗑").on_hover_text("Delete the whole room").clicked() {
                                            remove = Some(r.id);
                                        }
                                    });
                                    if !r.is_built() {
                                        ui.label(
                                            egui::RichText::new(
                                                "      unbuilt — only the footprint; build it from the 2D view ▸ ROOMS",
                                            )
                                            .small()
                                            .weak(),
                                        );
                                    }
                                    ui.horizontal(|ui| {
                                        ui.add_space(10.0);
                                        ui.label(egui::RichText::new("floor").small().weak());
                                        let mut ft = r.floor_t;
                                        if crate::factory::length_ui(ui, &u, &mut ft, 0.01, 0.02, 2.0, &self.calc)
                                            .on_hover_text("Slab below the room")
                                            .changed()
                                        {
                                            set_floor = Some((r.id, ft));
                                        }
                                        ui.label(egui::RichText::new("ceiling").small().weak());
                                        let mut ctk = r.ceiling_t;
                                        if crate::factory::length_ui(ui, &u, &mut ctk, 0.01, 0.02, 2.0, &self.calc)
                                            .on_hover_text("Slab above the room")
                                            .changed()
                                        {
                                            set_ceil = Some((r.id, ctk));
                                        }
                                    });
                                    // The sum, and whether it fits — only meaningful once the
                                    // room owns its slabs. The three numbers interact, and the
                                    // result is otherwise visible only as a picture that looks
                                    // wrong — which is how this was reported in the first place.
                                    let built = r.is_built();
                                    let over = r.overall_height();
                                    let fits = built
                                        && over <= self.factory.effective_building_height() + 1e-4;
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "      {} overall · {} openings{}",
                                            crate::factory::length_str(&u, over),
                                            self.factory.openings_in_room(r.id).len(),
                                            if fits {
                                                String::new()
                                            } else {
                                                format!(
                                                    "   ⚠ {} over the building",
                                                    crate::factory::length_str(
                        &u,
                                                        over - self.factory.effective_building_height(),
                                                    )
                                                )
                                            },
                                        ))
                                        .small()
                                        .color(if fits {
                                            egui::Color32::from_gray(140)
                                        } else {
                                            egui::Color32::from_rgb(230, 170, 90)
                                        }),
                                    );
                                    if !fits && built && ui.small_button("      ⤓ Fit to building").clicked() {
                                        let ct = if r.open_top { 0.0 } else { r.ceiling_t };
                                        fit = Some((
                                            r.id,
                                            (self.factory.effective_building_height() - r.floor_t - ct).max(0.05),
                                        ));
                                    }
                                    ui.separator();
                                }
                            });
                            if let Some((id, h)) = fit {
                                self.snapshot_factory();
                                self.factory.set_room_height(id, h);
                            }
                            if let Some((id, t)) = set_floor {
                                self.snapshot_factory();
                                self.factory.set_room_floor(id, t);
                            }
                            if let Some((id, t)) = set_ceil {
                                self.snapshot_factory();
                                self.factory.set_room_ceiling(id, t);
                            }
                            if let Some((id, name)) = rename {
                                self.factory.rename_room(id, &name);
                            }
                            if let Some((id, h)) = set_h {
                                self.snapshot_factory();
                                self.factory.set_room_height(id, h);
                            }
                            if let Some(id) = select {
                                let i = self.factory.room_index(id);
                                if i.is_some_and(|i| !self.factory.rooms[i].is_built()) {
                                    self.light.last_msg =
                                        "Unbuilt room — nothing to select yet; build it first (2D view ▸ ROOMS ⬆).".into();
                                } else {
                                    self.factory.selection = self.factory.room_features(id);
                                }
                                ui.close_menu();
                            }
                            if let Some(id) = remove {
                                self.snapshot_factory();
                                self.factory.delete_room(id);
                                ui.close_menu();
                            }
                        })
                        .response
                        .on_hover_text("Every room built — rename, change its height, select or delete it");
                    }

                    click_menu_button(ui, "▼ Room", |ui| {
                        ui.label(
                            egui::RichText::new("  Carve an interior out of the building")
                                .small()
                                .weak(),
                        );
                        ui.separator();
                        // Room properties — the void is carved on the ACTIVE storey as
                        // [floor thickness, floor thickness + room height], so every
                        // storey keeps its own floor.
                        ui.horizontal(|ui| {
                            ui.add_sized([84.0, 18.0], egui::Label::new(egui::RichText::new("  room height").small().weak()));
                            { let u = self.factory.units.clone(); crate::factory::length_ui(ui, &u, &mut self.factory.room_height, 0.02, 0.3, 30.0, &self.calc) }
                                .on_hover_text("Clear height. The room opens through the storey top so it is visible; set higher than the storey to cut above it.");
                        });
                        ui.horizontal(|ui| {
                            ui.add_sized([84.0, 18.0], egui::Label::new(egui::RichText::new("  floor thickness").small().weak()));
                            { let u = self.factory.units.clone(); crate::factory::length_ui(ui, &u, &mut self.factory.room_floor, 0.01, 0.0, 2.0, &self.calc) }
                                .on_hover_text("Slab left BELOW the room on this storey");
                        });
                        ui.horizontal(|ui| {
                            ui.add_sized([84.0, 18.0], egui::Label::new(egui::RichText::new("  ceiling thickness").small().weak()));
                            { let u = self.factory.units.clone(); crate::factory::length_ui(ui, &u, &mut self.factory.ceiling_thickness, 0.01, 0.02, 2.0, &self.calc) }
                                .on_hover_text("Slab ABOVE the room. Counts toward the overall height, like the floor does.");
                        });
                        // SUGGEST a height that fits, rather than leaving the user to subtract two
                        // slab thicknesses from the building height themselves — which is precisely
                        // the sum they were getting wrong.
                        {
                            let want = self.factory.suggested_room_height();
                            let u = self.factory.units.clone();
                            if (self.factory.room_height - want).abs() > 1e-3
                                && ui
                                    .button(format!(
                                        "  ⤓ Use {} — fills the {} building",
                                        crate::factory::length_str(&u, want),
                                        crate::factory::length_str(&u, self.factory.building_height),
                                    ))
                                    .on_hover_text("Building height less the floor and ceiling slabs")
                                    .clicked()
                            {
                                self.factory.room_height = want;
                            }
                        }
                        ui.checkbox(&mut self.factory.room_open_top, "  open to sky (no ceiling)")
                            .on_hover_text(
                                "Off (default): the room has a ceiling — what the lighting \
                                 calc needs.\nOn: cut through the top for a court / atrium \
                                 open above.",
                            );
                        // THE ARITHMETIC, SHOWN.
                        //
                        // "Room height" is the CLEAR height — floor top to ceiling underside,
                        // which is what the term means in a drawing. The slab below and the slab
                        // above are extra, so a 3900 room inside a 4000 building is 4250 overall
                        // and pokes 250 out of the top. That is correct and it is invisible: two
                        // numbers interact and the only evidence is a picture that looks wrong.
                        // Reported exactly that way — "I made the building 4 m tall and gave the
                        // room 3900, why is the room taller?".
                        {
                            let f = &self.factory;
                            let u = f.units.clone();
                            let ct = if f.room_open_top { 0.0 } else { f.ceiling_thickness.max(0.02) };
                            let overall = f.room_floor.max(0.02) + f.room_height.max(0.05) + ct;
                            let fits = overall <= f.effective_building_height() + 1e-4;
                            ui.label(
                                egui::RichText::new(format!(
                                    "  overall {}  =  {} floor + {} clear{}",
                                    crate::factory::length_str(&u, overall),
                                    crate::factory::length_str(&u, f.room_floor.max(0.02)),
                                    crate::factory::length_str(&u, f.room_height.max(0.05)),
                                    if ct > 0.0 {
                                        format!(" + {} ceiling", crate::factory::length_str(&u, ct))
                                    } else {
                                        String::new()
                                    },
                                ))
                                .small()
                                .color(if fits {
                                    egui::Color32::from_rgb(150, 200, 150)
                                } else {
                                    egui::Color32::from_rgb(230, 170, 90)
                                }),
                            )
                            .on_hover_text(
                                "Room height is the CLEAR height — floor top to ceiling underside. \
                                 The slabs above and below are additional, so the structure is \
                                 always taller than the number you type.",
                            );
                            if !fits {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "  ⚠ {} taller than the {} building — it will stand proud of the top",
                                        crate::factory::length_str(&u, overall - f.building_height),
                                        crate::factory::length_str(&u, f.building_height),
                                    ))
                                    .small()
                                    .color(egui::Color32::from_rgb(230, 170, 90)),
                                );
                            }
                        }
                        ui.separator();
                        if ui
                            .button("⬚  Make room (from outline)")
                            .on_hover_text(
                                "Select a CLOSED outline, then this builds a complete room \
                                 on the active storey — floor, walls and ceiling. No \
                                 separate building needed.",
                            )
                            .clicked()
                        {
                            self.do_make_room();
                            ui.close_menu();
                        }
                    })
                    .response
                    .on_hover_text("Carve interior rooms out of a building solid");

                    click_menu_button(ui, "▼ Room elements", |ui| {
                        ui.label(
                            egui::RichText::new("  Extrude / cut a shape drawn on a face")
                                .small()
                                .weak(),
                        );
                        ui.separator();
                        // How to feed these tools — draw on a face; you can act on it while
                        // drafting OR after Finishing (the last drawn face-sketch is used).
                        ui.label(egui::RichText::new("  1. Right-click a face → “Draw on this face”").small().weak());
                        ui.label(egui::RichText::new("  2. Draw a CLOSED shape (rectangle / circle)").small().weak());
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.add_sized([94.0, 18.0], egui::Label::new(egui::RichText::new("  height / depth").small().weak()));
                            { let u = self.factory.units.clone(); crate::factory::length_ui(ui, &u, &mut self.factory.element_height, 0.05, 0.02, 100.0, &self.calc); }
                        });
                        ui.checkbox(&mut self.factory.keep_sketch, "  keep shape after")
                            .on_hover_text("Reuse the SAME outline for more than one action (e.g. recess then cut through)");
                        ui.separator();
                        if ui
                            .button("⬆  Extrude element")
                            .on_hover_text("Raise the drawn shape into a solid on the face")
                            .clicked()
                        {
                            self.factory_extrude_sketch(false);
                            ui.close_menu();
                        }
                        if ui
                            .button("◳  Cut through (window / door)")
                            .on_hover_text("Cut the shape all the way through the wall/solid")
                            .clicked()
                        {
                            self.factory_cut_sketch(true);
                            ui.close_menu();
                        }
                        if ui
                            .button("◱  Recess (blind pocket)")
                            .on_hover_text("Cut a pocket of the given DEPTH — a niche/reveal, not through")
                            .clicked()
                        {
                            self.factory_cut_sketch(false);
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .button("〰 Path extrude")
                            .on_hover_text("Sweep a cross-section along a path into a solid (moldings, beams, pipes). Draw the section on a face → Enter → pick a perpendicular view → draw the path → Enter.")
                            .clicked()
                        {
                            self.factory_begin_sweep_flow(false, false);
                            ui.close_menu();
                        }
                        if ui
                            .button("〰 Path cut (channel)")
                            .on_hover_text("Subtract a swept cross-section along a path — a routed channel / shaped opening. Draw the section on a face → Enter → pick a perpendicular view → draw the path → Enter.")
                            .clicked()
                        {
                            self.factory_begin_sweep_flow(true, false);
                            ui.close_menu();
                        }
                    })
                    .response
                    .on_hover_text("Extrude or cut shapes drawn on a face — enabled while drafting on a face");

                    click_menu_button(ui, "▼ Furniture", |ui| {
                        if ui
                            .button("⭳  Import OBJ / 3DS / FBX / glTF…")
                            .on_hover_text("Import a furniture mesh (.obj, .3ds, .fbx, .glb or .gltf). Stored in the project for reuse.")
                            .clicked()
                        {
                            // Default the picker to the bundled CC0 furniture folder if present.
                            let cc0 = crate::assets::path("assets/cc0/furniture");
                            if cc0.is_dir() {
                                self.file_dialog_dir = Some(cc0.to_path_buf());
                            }
                            self.open_file_dialog(FileDialogMode::ImportObj, ".obj");
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(egui::RichText::new("  In this project").small().weak());
                        if self.factory.furniture_lib.is_empty() {
                            ui.label(egui::RichText::new("  (none imported yet)").small().weak());
                        } else {
                            // Click a library asset to place another copy.
                            let names: Vec<(usize, String)> = self
                                .factory
                                .furniture_lib
                                .iter()
                                .enumerate()
                                .map(|(i, a)| (i, a.name.clone()))
                                .collect();
                            for (i, name) in names {
                                if ui
                                    .button(format!("  ▫ {name}"))
                                    .on_hover_text("Place another copy at the model centre")
                                    .clicked()
                                {
                                    self.snapshot_factory();
                                    // Drop it where the building is, NOT at world origin —
                                    // otherwise it lands km away (DXF coords) and is invisible.
                                    let at = self.factory.place_at();
                                    self.factory.place_furniture(i, at);
                                    // No fit() — placing furniture must not yank the camera
                                    // out to a zoomed-out view; it lands at the model centre,
                                    // already on screen. (Matches import_furniture_obj.)
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!(
                                    "  {} placed",
                                    self.factory.furniture.len()
                                ))
                                .small()
                                .weak(),
                            );
                        }
                        ui.separator();
                        ui.label(egui::RichText::new("  Make furniture by parameters").small().weak());
                        if ui
                            .button("🗄  Cupboard (grid configurator)…")
                            .on_hover_text("A cabinet as a grid of bays × tiers — fill each cell with a door, glass, drawers, a niche or a panel")
                            .clicked()
                        {
                            self.arch_tab = ArchTab::Cupboard;
                            self.arch_modal_open = true;
                            ui.close_menu();
                        }
                        if ui
                            .button("🍳  Kitchen cabinets (run)…")
                            .on_hover_text("A kitchen run — base cabinets + worktop + plinth, with optional hung wall (upper) cabinets you can toggle on/off")
                            .clicked()
                        {
                            self.arch_tab = ArchTab::Kitchen;
                            self.arch_modal_open = true;
                            ui.close_menu();
                        }
                        if ui
                            .button("🚪  Cabinet unit (handleless)…")
                            .on_hover_text("A single close-range cabinet: full-overlay fronts, real joinery, shadow gaps and a grip (handleless / J-groove / bar / rail). Grid of doors, drawers, open and panel cells.")
                            .clicked()
                        {
                            self.arch_tab = ArchTab::Cabin;
                            self.arch_modal_open = true;
                            ui.close_menu();
                        }
                        if ui
                            .button("💡  Curved light (sweep)…")
                            .on_hover_text("A curved office luminaire: an extruded aluminium profile with a glowing lens face, swept along a ring, racetrack or S-curve path, hung on rods. The lens material is emissive — it glows in the ⏺ raytraced render.")
                            .clicked()
                        {
                            self.arch_tab = ArchTab::SweepLight;
                            self.arch_modal_open = true;
                            ui.close_menu();
                        }
                        if ui
                            .button("🛋  Sofa (straight / L / U)…")
                            .on_hover_text("A wood-frame sofa built as a RUN CHAIN: list the straight segments and each extra one adds a +90° corner unit, so the same tool makes a two-seater, an L-sectional or a U. Arch arms, spindle back and pillowed cushions are all deletable features.")
                            .clicked()
                        {
                            self.arch_tab = ArchTab::Couch;
                            self.arch_modal_open = true;
                            ui.close_menu();
                        }
                        if ui
                            .button("🖥  Office desk (workstation)…")
                            .on_hover_text("A workstation desk built as a FEATURE TREE: chamfered top, curved fabric privacy screen, splayed Λ legs, a drawer pedestal with a fingerprint lock, an under-top rail and cable ports. Every feature can be switched off — and a deleted port takes its hole with it.")
                            .clicked()
                        {
                            self.arch_tab = ArchTab::Desk;
                            self.arch_modal_open = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(egui::RichText::new("  Make furniture by drawing").small().weak());
                        if ui
                            .button("⬆  Extrude drawn shape")
                            .on_hover_text("Right-click a face → Draw on this face, draw a shape, then this extrudes it into a free-standing furniture solid (never cuts the building)")
                            .clicked()
                        {
                            self.factory_extrude_sketch(true);
                            ui.close_menu();
                        }
                        if ui
                            .button("〰 Path extrude")
                            .on_hover_text("Sweep a cross-section along a path into a furniture piece (tubular frames, rails, trim). Draw the section on a face → Enter → pick a perpendicular view → draw the path → Enter.")
                            .clicked()
                        {
                            self.factory_begin_sweep_flow(false, true);
                            ui.close_menu();
                        }
                    })
                    .response
                    .on_hover_text("Import furniture (.obj/.3ds/.fbx), place it, or extrude/sweep a drawn shape into furniture");

                    click_menu_button(ui, "▼ FBC scene import", |ui| {
                        self.factory_scene_import_menu(ui);
                    })
                    .response
                    .on_hover_text("Import a whole scene exported from Blender (.glb / .gltf / .fbx) at its real-world size, with its materials and textures");

                    click_menu_button(ui, "▼ Textures", |ui| {
                        self.factory_textures_menu(ui);
                    })
                    .response
                    .on_hover_text("Colour or texture the selected object (paste/load an image, or reuse one from the library); tune tiling · opacity · reflection in the properties panel");

                    click_menu_button(ui, "▼ Openings", |ui| {
                        self.factory_openings_menu(ui);
                    })
                    .response
                    .on_hover_text("Select, resize or delete cutouts (windows/doors/recesses)");

                    click_menu_button(ui, "▼ Apertures", |ui| {
                        self.factory_apertures_menu(ui);
                    })
                    .response
                    .on_hover_text("Doors & windows: import one, or draw a rectangle on a wall and the app cuts the opening + fits a door/window into it");

                    click_menu_button(ui, "▼ Architecture", |ui| {
                        self.factory_architecture_menu(ui);
                    })
                    .response
                    .on_hover_text("Generate a staircase (straight or U-shape), a spiral stair, or a ramp (doors are in ▼ Apertures, cupboards in ▼ Furniture)");

                    if ui
                        .button(if self.factory.sun.enabled { "☀ Sun" } else { "☀ Sun…" })
                        .on_hover_text("Daylight: locate the sun from the building's latitude/longitude, date and time (Radiance's model) and light the scene by it")
                        .clicked()
                    {
                        self.sun_modal_open = !self.sun_modal_open;
                    }

                    if ui
                        .selectable_label(self.render_modal_open, "⏺ Render")
                        .on_hover_text("Path-traced render (raytracing): true global illumination, reflections, soft shadows and glass — refines progressively, like Blender's Cycles. Choose CPU or GPU in the dialog.")
                        .clicked()
                    {
                        self.render_modal_open = !self.render_modal_open;
                    }

                    ui.separator();
                    // 2D plan underlay toggle — a highlighted button when on.
                    let plan_on = self.factory.show_plan;
                    if ui
                        .selectable_label(plan_on, "▦ Plan")
                        .on_hover_text(
                            "Show the 2D drawing on the ground as a reference underlay. Solids \
                             in front of it hide it, like any other geometry — use X-ray to see \
                             it through the model.",
                        )
                        .clicked()
                    {
                        self.factory.show_plan = !plan_on;
                    }
                    if plan_on {
                        let xray = self.factory.plan_xray;
                        if ui
                            .selectable_label(xray, "X-ray")
                            .on_hover_text(
                                "Draw the plan THROUGH the model. Useful when it is a bare \
                                 outline you are tracing; on a full drawing every line on the \
                                 far side lands on the near wall and looks like texture on it.",
                            )
                            .clicked()
                        {
                            self.factory.plan_xray = !xray;
                        }
                    }
                    // Hide ceilings — hide ONLY the room ceiling slabs in the view, so you
                    // see into the rooms while the surrounding roof and walls stay. VIEW
                    // ONLY: the ceilings (and the lighting model) are untouched.
                    let hide_on = self.factory.hide_ceilings;
                    let n_ceil = self.factory.ceilings.len();
                    // The count is shown so it's obvious when there is NOTHING to hide — a
                    // solid building or bare walls have no ceiling slab, and "open to sky"
                    // rooms make none. `(0)` explains a toggle that "does nothing".
                    if ui
                        .selectable_label(hide_on, format!("▤ Hide ceilings ({n_ceil})"))
                        .on_hover_text(
                            "Hide room/ceiling slabs so you can see inside. Only tracked \
                             ceilings count — a solid building's top or bare walls have \
                             none. Make a room (ceiling on) to get one.",
                        )
                        .clicked()
                    {
                        self.factory.hide_ceilings = !hide_on;
                        self.factory.dirty = true;
                        self.factory.recompute();
                        if n_ceil == 0 {
                            self.factory.status =
                                "no ceilings to hide — try 'Cutaway' to see inside anything".into();
                        }
                    }
                    ui.separator();
                    // Gizmo mode: Move arms vs Rotate rings. Applies to the selected
                    // furniture or room-element solid.
                    let mode = self.factory.gizmo_mode;
                    if ui.selectable_label(mode == crate::factory::GizmoMode::Move, "↔ Move")
                        .on_hover_text("Drag the arms to move the selection").clicked()
                    {
                        self.factory.gizmo_mode = crate::factory::GizmoMode::Move;
                    }
                    if ui.selectable_label(mode == crate::factory::GizmoMode::Rotate, "⟳ Rotate")
                        .on_hover_text("Drag a ring to rotate the selection about that axis (X red · Y green · Z blue)").clicked()
                    {
                        self.factory.gizmo_mode = crate::factory::GizmoMode::Rotate;
                    }
                    ui.separator();
                    if ui.button("⌖ Frame").clicked() {
                        if self.factory.dirty {
                            self.factory.recompute();
                        }
                        self.factory.fit();
                    }
                    if ui.button("🗑 Clear").clicked() {
                        self.snapshot_factory();
                        self.factory.clear();
                    }
                });
                // ---- ACTIVE STOREY — where new geometry is placed -----------
                // The build tools live in this panel, but the storey list is in the main
                // menu — so without this line it is not obvious which level you are
                // building on, and a building placed 3 m up reads as "nothing happened".
                // This shows (and lets you change) the active level right where you build.
                ui.horizontal(|ui| {
                    let n = self.factory.storeys.len();
                    let act = self.factory.active_storey.min(n - 1);
                    let base = self.factory.active_base_z();
                    ui.label(egui::RichText::new("Active level").small().weak());
                    if ui.add_enabled(act > 0, egui::Button::new("▼")).on_hover_text("Down a level").clicked() {
                        self.factory.active_storey = act - 1;
                    }
                    ui.label(
                        egui::RichText::new(format!("{}", self.factory.storeys[act].name))
                            .strong()
                            .color(egui::Color32::from_rgb(255, 200, 120)),
                    );
                    if ui.add_enabled(act + 1 < n, egui::Button::new("▲")).on_hover_text("Up a level").clicked() {
                        self.factory.active_storey = act + 1;
                    }
                    ui.label(
                        egui::RichText::new(format!("builds at {base:.2} m"))
                            .small()
                            .weak(),
                    )
                    .on_hover_text(
                        "New buildings, walls and solids are placed on this level.\n\
                         Add levels in the 3D Factory menu (Storey on top).",
                    );
                    if ui.small_button("＋ level").on_hover_text("Add an empty storey on top and make it active").clicked() {
                        self.snapshot_factory();
                        let i = self.factory.add_storey_on_top();
                        self.history.push(format!(
                            "  storey '{}' added — new geometry now builds at {:.2} m",
                            self.factory.storeys[i].name,
                            self.factory.storey_base_z(i),
                        ));
                    }
                    if ui
                        .small_button("⇑ duplicate floor")
                        .on_hover_text("Copy this level's building, walls and solids onto a new level above")
                        .clicked()
                    {
                        self.snapshot_factory();
                        match self.factory.duplicate_storey_up() {
                            Some(i) => {
                                self.factory.recompute();
                                self.factory.fit();
                                self.history.push(format!(
                                    "  floor duplicated up to '{}' (at {:.2} m)",
                                    self.factory.storeys[i].name,
                                    self.factory.storey_base_z(i),
                                ));
                            }
                            None => {
                                self.undo_stack.pop();
                                self.factory.status =
                                    "nothing on this level to duplicate — build something first".into();
                            }
                        }
                    }
                });
                // Room dimensions live in the ▼ Room menu (set them before Make room). The
                // old always-on row here was redundant with that menu — removed. A built
                // solid's real size is edited in the PROPERTIES panel when it's selected.
                if self.factory.zoom_mode != crate::factory::ZoomMode::Off
                    || self.factory.place_pending.is_some()
                    || !self.factory.status.is_empty()
                {
                    ui.label(
                        egui::RichText::new(&self.factory.status)
                            .small()
                            .color(egui::Color32::from_rgb(120, 200, 255)),
                    );
                }
                // ---- PROPERTIES — appears only when something is selected ----
                // Replaces the old always-on wall-height/thickness sliders (which did
                // nothing until a wall existed). Everything here is a type-in field.
                //
                // The panel is wrapped in a COLLAPSIBLE + height-RESIZABLE region so a
                // tall selection (transform + texture editors) can't crowd the 3D
                // preview below (which fills whatever vertical space is left): the user
                // drags the handle to set its height, or collapses the header entirely.
                if self.factory.has_any_selection() {
                    egui::CollapsingHeader::new("Selection properties")
                        .id_salt("factory_props_collapse")
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::Resize::default()
                                .id_salt("factory_props_resize")
                                .resizable([false, true])
                                .default_height(260.0)
                                .min_height(90.0)
                                .show(ui, |ui| {
                                    egui::ScrollArea::vertical()
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            self.factory_properties_panel(ui);
                                        });
                                });
                        });
                } else {
                    // Nothing selected — just the one-line hint, no chrome to collapse.
                    self.factory_properties_panel(ui);
                }
                ui.label(
                    egui::RichText::new(format!(
                        "{} feature(s) · {} tris · {} selected",
                        self.factory.feature_count(),
                        self.factory.tri_count(),
                        self.factory.selection.len()
                    ))
                    .small()
                    .weak(),
                );
                // Live prompt for a running 3D op — the same feedback the 2D command
                // line gives, so a 3D op is never silently waiting for a pick.

                ui.separator();

                let size = ui.available_size();
                let (rect, resp) =
                    ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                let painter = ui.painter_at(rect);

                // ---- VIEWPOINT CHANGE — MIDDLE drag, or ALT + RIGHT drag -----
                // The project's long-standing mouse rule (COMMAND_LINE_AND_MOUSE_RULES
                // Part B), now applied to 3D: the LEFT button is reserved for picks and
                // selection and NEVER moves the camera — otherwise dragging a solid
                // sends the view flying. Only the middle button (or Alt+Right, for
                // mice without one) orbits.
                // ACTIVE VIEW — a press INSIDE the 3D viewport makes 3D active
                // (orbiting counts: you are working in this view).
                if resp.is_pointer_button_down_on() || resp.clicked() || resp.dragged() {
                    self.active_view = ActiveView::ThreeD;
                }
                let alt = ui.input(|i| i.modifiers.alt);
                let shift = ui.input(|i| i.modifiers.shift);
                let mut return_after_click = false; // a running 3D op consumed the click
                // PAN — Shift + MIDDLE (scroll-wheel button) drag slides the view. Middle alone
                // orbits, so holding Shift with it pans. The LEFT button stays free for picks /
                // gizmo drags. Tested BEFORE gizmo / select and suppresses them via `panning`.
                let panning = shift && resp.dragged_by(egui::PointerButton::Middle);
                if panning {
                    let d = resp.drag_delta();
                    self.factory.pan(d.x, d.y);
                }
                // ORBIT — middle drag (WITHOUT shift), or Alt + Right drag.
                let orbiting = !panning
                    && (resp.dragged_by(egui::PointerButton::Middle)
                        || (alt && resp.dragged_by(egui::PointerButton::Secondary)));
                if orbiting {
                    let d = resp.drag_delta();
                    self.factory.cam_yaw -= d.x * 0.01;
                    self.factory.cam_pitch =
                        (self.factory.cam_pitch + d.y * 0.01).clamp(-1.45, 1.45);
                    self.factory.ortho = false; // free orbit → back to perspective
                }
                if resp.hovered() {
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll.abs() > 0.0 {
                        let max = self.factory.max_cam_dist();
                        self.factory.cam_dist =
                            (self.factory.cam_dist * (1.0 - scroll * 0.0015)).clamp(0.4, max);
                    }
                }

                // ---- ARROW-KEY NUDGE — fine placement of the selected object ----------
                // Arrows nudge the selection in the ground plane, PageUp/Dn in Z; Shift = a
                // coarser 5× step. Works for furniture (instant) and CSG features (re-eval'd).
                //
                // ROOT CAUSE (confirmed by the `nudge blocked … kbd_free=false` dump): a
                // Position/Rotation DragValue in the properties panel keeps keyboard focus
                // after you touch it, and the 3D viewport is NOT focusable, so clicking a
                // furniture never clears that focus. A focus-based gate therefore blocked
                // EVERY arrow. The right signal is not "is the keyboard free" but "is the
                // pointer over the 3D viewport" — if so, the arrows are meant for nudging.
                let over_viewport = resp.hovered() || resp.contains_pointer();
                let can_nudge = (self.active_view == ActiveView::ThreeD || over_viewport)
                    && self.factory.session.is_none()
                    && self.factory.has_any_selection();
                if can_nudge {
                    // CONSUME the keys (with the current Shift state) so a still-focused
                    // DragValue can't also react to them.
                    let mods = if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
                    let (mut dx, mut dy, mut dz) = (0.0f32, 0.0f32, 0.0f32);
                    ui.input_mut(|i| {
                        if i.consume_key(mods, egui::Key::ArrowRight) { dx += 1.0; }
                        if i.consume_key(mods, egui::Key::ArrowLeft)  { dx -= 1.0; }
                        if i.consume_key(mods, egui::Key::ArrowUp)    { dy += 1.0; }
                        if i.consume_key(mods, egui::Key::ArrowDown)  { dy -= 1.0; }
                        if i.consume_key(mods, egui::Key::PageUp)     { dz += 1.0; }
                        if i.consume_key(mods, egui::Key::PageDown)   { dz -= 1.0; }
                    });
                    if dx != 0.0 || dy != 0.0 || dz != 0.0 {
                        // Drop any lingering DragValue focus so the next press is clean and
                        // the field doesn't keep stealing the keyboard.
                        if let Some(id) = ui.ctx().memory(|m| m.focused()) {
                            ui.ctx().memory_mut(|m| m.surrender_focus(id));
                        }
                        // Step SCALES WITH ZOOM (cam_dist): zoomed far out → coarse moves that
                        // keep pace with the view; zoomed right in → millimetre steps for fine
                        // placement. Shift = a coarser 5× step. Clamped so it's never zero.
                        let base = (self.factory.cam_dist * 0.01).clamp(0.002, 20.0);
                        let step = if shift { base * 5.0 } else { base };
                        let delta = glam::Vec3::new(dx * step, dy * step, dz * step);
                        self.snapshot_factory();
                        self.factory.move_selection(delta);
                        // Furniture is not in the CSG model; a feature move needs a re-eval.
                        if self.factory.sel_furniture.is_empty() {
                            self.factory.recompute();
                        }
                        self.factory.status = format!(
                            "nudged ({:+.3}, {:+.3}, {:+.3}) m (step {:.3} m, zoom-scaled) — arrows: X/Y · PgUp/PgDn: Z · Shift: ×5",
                            delta.x, delta.y, delta.z, step,
                        );
                        ui.ctx().request_repaint();
                    }
                }

                // ---- COPY / PASTE (Ctrl+C / Ctrl+V) — 3D objects --------------
                // Gated like the nudge: 3D view active or pointer over the viewport, no sketch
                // open. Keys are CONSUMED so the 2D canvas's own copy/paste doesn't also fire.
                // ---- 3D VIEWPORT HOTKEYS ------------------------------------------------
                //
                // Gated on the ACTIVE VIEW — never on the panel being open, and never on the mouse
                // merely hovering. That distinction is the one `established_commands_are_untouchable`
                // exists to protect: `m` is the 2D MOVE command and a panel being visible says
                // nothing about what you are editing. Here you must have clicked INTO the 3D
                // viewport, no sketch may be open, and nothing may hold keyboard focus — so a bare
                // letter cannot be taken out of the command line or out of a numeric field.
                let typing = ui.memory(|m| m.focused().is_some());
                if self.active_view == ActiveView::ThreeD
                    && !typing
                    && self.factory.session.is_none()
                {
                    use crate::factory::{GizmoMode as GM, StdView};
                    let n = egui::Modifiers::NONE;
                    let sh = egui::Modifiers::SHIFT;
                    let alt_m = egui::Modifiers::ALT;
                    let cmd = egui::Modifiers::COMMAND;
                    let k = ui.input_mut(|i| {
                        [
                            i.consume_key(n, egui::Key::M),        // 0 move gizmo
                            i.consume_key(n, egui::Key::R),        // 1 rotate gizmo
                            i.consume_key(n, egui::Key::F),        // 2 frame
                            i.consume_key(n, egui::Key::G),        // 3 grid
                            i.consume_key(n, egui::Key::H),        // 4 hide ceilings
                            i.consume_key(n, egui::Key::Escape),   // 5 clear selection
                            i.consume_key(n, egui::Key::Tab),      // 6 cycle gizmo
                            i.consume_key(cmd, egui::Key::D),      // 7 duplicate
                            i.consume_key(n, egui::Key::A),        // 8 select all
                            i.consume_key(alt_m, egui::Key::A),    // 9 deselect all
                            i.consume_key(n, egui::Key::Num1),     // 10 front
                            i.consume_key(n, egui::Key::Num2),     // 11 right
                            i.consume_key(n, egui::Key::Num3),     // 12 top
                            i.consume_key(n, egui::Key::Num0),     // 13 iso
                            i.consume_key(n, egui::Key::X),        // 14 x-ray
                            i.consume_key(sh, egui::Key::X),       // 15 cutaway
                            i.consume_key(n, egui::Key::S),        // 16 snapping
                            i.consume_key(n, egui::Key::OpenBracket),  // 17 storey down
                            i.consume_key(n, egui::Key::CloseBracket), // 18 storey up
                        ]
                    });
                    let on = |b: bool| if b { "on" } else { "off" };
                    if k[0] {
                        self.factory.gizmo_mode = GM::Move;
                        self.factory.status = "move gizmo — drag an arrow to constrain, the centre to slide".into();
                    }
                    if k[1] {
                        self.factory.gizmo_mode = GM::Rotate;
                        self.factory.status = "rotate gizmo — drag a ring to spin about that axis".into();
                    }
                    if k[6] {
                        // One key for the whole gizmo, for the hand that never leaves the mouse.
                        self.factory.gizmo_mode =
                            if self.factory.gizmo_mode == GM::Move { GM::Rotate } else { GM::Move };
                        self.factory.status = format!(
                            "{} gizmo",
                            if self.factory.gizmo_mode == GM::Move { "move" } else { "rotate" }
                        );
                    }
                    if k[2] {
                        self.factory.fit_all();
                        self.factory.status = "framed the model".into();
                    }
                    if k[3] {
                        self.factory.show_grid = !self.factory.show_grid;
                        self.factory.status = format!("grid {}", on(self.factory.show_grid));
                    }
                    if k[4] {
                        self.factory.hide_ceilings = !self.factory.hide_ceilings;
                        self.factory.recompute();
                        self.factory.status = format!(
                            "ceilings {}",
                            if self.factory.hide_ceilings { "hidden" } else { "shown" }
                        );
                    }
                    if k[5] {
                        self.factory.clear_selection();
                        self.selection.clear();
                        self.factory.status = "selection cleared".into();
                    }
                    if k[7] {
                        // Duplicate in place — copy + paste in one gesture, which is the pair of
                        // keys anybody actually wanted when they pressed Ctrl+C in a 3D view.
                        if self.factory.copy_selection() {
                            self.snapshot_factory();
                            match self.factory.paste_clipboard() {
                                Some(is_feature) => {
                                    if is_feature {
                                        self.factory.recompute();
                                    }
                                    self.factory.status =
                                        "duplicated (offset 0.3 m) — drag or arrow-nudge to place".into();
                                }
                                None => {
                                    self.undo_stack.pop();
                                    self.factory.status = "nothing to duplicate".into();
                                }
                            }
                        } else {
                            self.factory.status = "select a 3D object first, then Ctrl+D".into();
                        }
                    }
                    if k[8] {
                        self.factory.select_all();
                        self.factory.status = format!(
                            "{} solid(s), {} piece(s) selected",
                            self.factory.selection.len(),
                            self.factory.sel_furniture.len(),
                        );
                    }
                    if k[9] {
                        self.factory.clear_selection();
                        self.selection.clear();
                        self.factory.status = "deselected".into();
                    }
                    for (hit, view, name) in [
                        (k[10], StdView::Front, "front"),
                        (k[11], StdView::Right, "right"),
                        (k[12], StdView::Top, "top"),
                        (k[13], StdView::Iso, "iso"),
                    ] {
                        if hit {
                            self.factory.set_view(view);
                            self.factory.status = format!("{name} view");
                        }
                    }
                    if k[14] {
                        self.factory.plan_xray = !self.factory.plan_xray;
                        self.factory.status = format!("x-ray {}", on(self.factory.plan_xray));
                    }
                    if k[15] {
                        self.factory.cutaway = !self.factory.cutaway;
                        self.factory.status = format!("cutaway {}", on(self.factory.cutaway));
                    }
                    if k[16] {
                        self.factory.snap_3d = !self.factory.snap_3d;
                        self.factory.status = format!("3D snapping {}", on(self.factory.snap_3d));
                    }
                    if k[17] || k[18] {
                        let n_st = self.factory.storeys.len().max(1);
                        let cur = self.factory.active_storey;
                        let next = if k[18] {
                            (cur + 1).min(n_st - 1)
                        } else {
                            cur.saturating_sub(1)
                        };
                        if next != cur {
                            self.factory.active_storey = next;
                            self.factory.status = format!(
                                "storey {} of {n_st}{}",
                                next + 1,
                                self.factory
                                    .storeys
                                    .get(next)
                                    .map(|s| format!(" — {}", s.name))
                                    .unwrap_or_default(),
                            );
                        }
                    }
                    if k.iter().any(|b| *b) {
                        ui.ctx().request_repaint();
                    }
                }
                if (self.active_view == ActiveView::ThreeD || over_viewport)
                    && self.factory.session.is_none()
                {
                    // eframe usually delivers Ctrl+C/V as Event::Copy/Paste (NOT raw Key
                    // presses), so catch BOTH forms — otherwise 3D copy/paste silently misses on
                    // platforms that only emit the events. Consuming them keeps the command line
                    // from also reacting.
                    let (do_copy, do_paste) = ui.input_mut(|i| {
                        let mut c = i.consume_key(egui::Modifiers::COMMAND, egui::Key::C);
                        let mut v = i.consume_key(egui::Modifiers::COMMAND, egui::Key::V);
                        i.events.retain(|e| match e {
                            egui::Event::Copy | egui::Event::Cut => { c = true; false }
                            egui::Event::Paste(_) => { v = true; false }
                            _ => true,
                        });
                        (c, v)
                    });
                    if do_copy {
                        if self.factory.copy_selection() {
                            self.factory.status = "copied — Ctrl+V to paste".into();
                        } else {
                            self.factory.status = "select a 3D object first, then Ctrl+C".into();
                        }
                        ui.ctx().request_repaint();
                    }
                    if do_paste {
                        self.snapshot_factory();
                        match self.factory.paste_clipboard() {
                            Some(is_feature) => {
                                if is_feature {
                                    self.factory.recompute();
                                }
                                self.factory.status = "pasted a copy (offset 0.3 m) — drag or arrow-nudge to place".into();
                            }
                            None => {
                                self.undo_stack.pop(); // nothing pasted → discard the snapshot
                                self.factory.status = "clipboard is empty — Ctrl+C to copy first".into();
                            }
                        }
                        ui.ctx().request_repaint();
                    }
                }

                // Record 3D-viewport pointer activity WHILE a zoom is live — the 3D view
                // doesn't emit the 2D canvas press/click events, so without this a zoom
                // that "does nothing" is invisible (can't tell a drag from a click).
                if self.factory.zoom_mode != crate::factory::ZoomMode::Off {
                    if resp.drag_started() {
                        self.factory_note(format!(
                            "3D vp: DRAG start (primary={})",
                            resp.dragged_by(egui::PointerButton::Primary)
                        ));
                    }
                    if resp.drag_stopped() { self.factory_note("3D vp: DRAG stop".into()); }
                    if resp.clicked() { self.factory_note("3D vp: CLICK".into()); }
                }

                // ---- ZOOM interaction (mirrors 2D). RealTime: a LEFT drag up/down dollies
                // smoothly (drag UP = zoom in). Window: DRAG a box OR click two corners,
                // reframing on release / second click. Both suppress the pick while zooming.
                match self.factory.zoom_mode {
                    crate::factory::ZoomMode::Window => {
                        // WINDOW — the 2D default: DRAG a box (press one corner, drag, release
                        // the other) OR click two corners. Amber rubber-band + "zoom window"
                        // label, IDENTICAL to the 2D zoom-window preview.
                        if resp.dragged_by(egui::PointerButton::Primary) {
                            let pos = resp.interact_pointer_pos();
                            if self.factory.zoom_drag.is_none() { self.factory.zoom_drag = pos; }
                            self.factory.zoom_cur = pos;
                        } else if self.factory.zoom_drag.is_some() {
                            if let Some(p) = resp.hover_pos() { self.factory.zoom_cur = Some(p); }
                        }

                        // Commit on drag-release, or on the SECOND corner click.
                        let commit: Option<egui::Pos2> = if resp.drag_stopped() {
                            self.factory.zoom_cur
                        } else if resp.clicked() {
                            match (resp.interact_pointer_pos(), self.factory.zoom_drag) {
                                (Some(pos), None) => {
                                    self.factory.zoom_drag = Some(pos); // FIRST corner
                                    self.factory.zoom_cur = Some(pos);
                                    self.factory.status =
                                        "ZOOM window — click the OPPOSITE corner  [Esc]".into();
                                    let s = self.factory.zoom_status();
                                    self.dbg_zoom("window corner 1", "(click the opposite corner)",
                                        "first corner placed".into(), s.clone(), s);
                                    None
                                }
                                (Some(pos), Some(_)) => Some(pos), // SECOND corner
                                _ => None,
                            }
                        } else {
                            None
                        };

                        if let (Some(e), Some(s)) = (commit, self.factory.zoom_drag) {
                            if (e - s).length() > 4.0 {
                                let before = self.factory.zoom_status();
                                self.factory.zoom_window(rect, s, e);
                                let after = self.factory.zoom_status();
                                self.dbg_zoom("window", "(drag a box OR click two corners → reframe)",
                                    format!("reframe to {:.0}×{:.0} px box", (e.x - s.x).abs(), (e.y - s.y).abs()),
                                    before, after);
                                self.factory.zoom_drag = None;
                                self.factory.zoom_cur = None;
                                self.factory.zoom_mode = crate::factory::ZoomMode::Off;
                                self.factory.status.clear();
                            } else {
                                self.factory_note("3D zoom window: box too small — pick the opposite corner".into());
                            }
                        }

                        // Amber rubber-band + "zoom window" label — EXACTLY like 2D. Painted
                        // on a FOREGROUND layer, because the 3D scene is an opaque texture
                        // drawn AFTER this in the same painter and was hiding the preview
                        // (the gizmo shows for the same reason — it's a foreground Area).
                        if let (Some(s), Some(e)) = (self.factory.zoom_drag, self.factory.zoom_cur) {
                            let r = egui::Rect::from_two_pos(s, e);
                            let fg = ui.ctx().layer_painter(egui::LayerId::new(
                                egui::Order::Foreground,
                                egui::Id::new("factory_zoom_preview"),
                            )).with_clip_rect(rect);
                            fg.rect_filled(r, 0.0,
                                egui::Color32::from_rgba_unmultiplied(255, 200, 100, 26));
                            fg.rect_stroke(r, 0.0,
                                egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 200, 100)));
                            fg.text(
                                e + egui::vec2(12.0, 12.0),
                                egui::Align2::LEFT_TOP,
                                "zoom window",
                                crate::theme::typ::data_code(),
                                egui::Color32::from_rgb(255, 220, 140),
                            );
                            ui.ctx().request_repaint(); // keep the preview tracking the cursor
                        }
                        return_after_click = true;
                    }
                    crate::factory::ZoomMode::RealTime => {
                        if resp.drag_started() {
                            self.factory.zoom_rt_before = Some(self.factory.zoom_status());
                        }
                        if resp.dragged_by(egui::PointerButton::Primary) {
                            let dy = resp.drag_delta().y;
                            if dy != 0.0 {
                                // drag UP (dy<0) zooms in, DOWN zooms out — AutoCAD real-time
                                let max = self.factory.max_cam_dist();
                                self.factory.cam_dist =
                                    (self.factory.cam_dist * (1.0 + dy * 0.006)).clamp(0.4, max);
                            }
                        }
                        if resp.drag_stopped() {
                            if let Some(before) = self.factory.zoom_rt_before.take() {
                                let after = self.factory.zoom_status();
                                self.dbg_zoom("real-time drag", "(drag up = in · down = out)",
                                    "real-time dolly".into(), before, after);
                            }
                        }
                        return_after_click = true;
                    }
                    crate::factory::ZoomMode::Off => {}
                }

                // ---- corner NAV GIZMO — standard views + zoom, like every 3D app ----
                // A foreground Area top-right of the viewport, so its buttons take clicks
                // before the orbit/select handler underneath. Views snap the camera;
                // ＋/－/⌖ are zoom in / out / extents (the wheel already dollies).
                egui::Area::new(egui::Id::new("factory_nav_gizmo"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(rect.right() - 130.0, rect.top() + 8.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style())
                            .inner_margin(egui::Margin::symmetric(5.0, 4.0))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                                ui.spacing_mut().button_padding = egui::vec2(4.0, 1.0);
                                use crate::factory::StdView::*;

                                // THE VIEW CUBE — the orientation control IS the cube now:
                                // click a FACE to snap to that view (Top/Bottom/Front/Back/
                                // Left/Right — the face under the cursor lights up), DRAG to
                                // orbit, DOUBLE-CLICK for isometric. The ring + dot show the
                                // live heading.
                                let (crect, cresp) = ui.allocate_exact_size(
                                    egui::vec2(120.0, 112.0),
                                    egui::Sense::click_and_drag(),
                                );
                                let (cyaw, cpitch) = (self.factory.cam_yaw, self.factory.cam_pitch);
                                if cresp.dragged() {
                                    let d = cresp.drag_delta();
                                    self.factory.cam_yaw -= d.x * 0.01;
                                    self.factory.cam_pitch =
                                        (self.factory.cam_pitch + d.y * 0.01).clamp(-1.45, 1.45);
                                    self.factory.ortho = false; // free orbit → perspective
                                } else if cresp.clicked() {
                                    if let Some(pos) = cresp.interact_pointer_pos() {
                                        if let Some(v) = nav_cube_pick(crect, cyaw, cpitch, pos) {
                                            self.factory.set_view(v);
                                        }
                                    }
                                }
                                if cresp.double_clicked() {
                                    self.factory.set_view(Iso);
                                }
                                let hover = cresp.hover_pos()
                                    .and_then(|pos| nav_cube_pick(crect, cyaw, cpitch, pos));
                                draw_nav_cube(ui.painter(), crect, cyaw, cpitch, hover);

                                // Zoom controls (view orientation lives on the cube now).
                                ui.horizontal(|ui| {
                                    if ui.small_button("▣").on_hover_text("Zoom window — drag a box").clicked() {
                                        let before = self.factory.zoom_status();
                                        self.factory.zoom_mode = crate::factory::ZoomMode::Window;
                                        self.factory.zoom_drag = None;
                                        self.factory.zoom_cur = None;
                                        self.factory.status = "ZOOM window — drag a box (or click two corners)  [Esc]".into();
                                        let after = self.factory.zoom_status();
                                        self.dbg_zoom("gizmo ▣", "window", "window (drag a box)".into(), before, after);
                                    }
                                    if ui.small_button("＋").on_hover_text("Zoom in").clicked() {
                                        let before = self.factory.zoom_status();
                                        self.factory.zoom_by(0.8);
                                        let after = self.factory.zoom_status();
                                        self.dbg_zoom("gizmo ＋", "in", "in (×0.8 dolly)".into(), before, after);
                                    }
                                    if ui.small_button("－").on_hover_text("Zoom out").clicked() {
                                        let before = self.factory.zoom_status();
                                        self.factory.zoom_by(1.25);
                                        let after = self.factory.zoom_status();
                                        self.dbg_zoom("gizmo －", "out", "out (×1.25 dolly)".into(), before, after);
                                    }
                                    if ui.small_button("⌖").on_hover_text("Zoom extents").clicked() {
                                        let before = self.factory.zoom_status();
                                        if self.factory.dirty { self.factory.recompute(); }
                                        self.factory.fit();
                                        let after = self.factory.zoom_status();
                                        self.dbg_zoom("gizmo ⌖", "extents", "extents (fit all)".into(), before, after);
                                    }
                                });
                            });
                    });

                let aspect = if rect.height() > 0.0 { rect.width() / rect.height() } else { 1.0 };
                let mvp = crate::light3d::mvp(
                    self.factory.cam_yaw,
                    self.factory.cam_pitch,
                    self.factory.cam_dist,
                    self.factory.cam_target,
                    aspect,
                    self.factory.ortho,
                );
                // ---- WALL VERTEX HANDLES (Track A slices 2–5) ----------------
                // Reshape a promoted wall in place. Handles show for the SELECTED wall
                // only. Gestures (owner-confirmed 2026-07-23):
                //   drag a dot          → move that corner
                //   click an edge dot   → insert a corner there
                //   right-click a dot   → delete that corner
                // The engine (`wall_move_vertex` / `_insert_` / `_delete_`) was already
                // written and unit-tested; this is the viewport wiring it lacked.
                //
                // Runs BEFORE selection so a click on a handle is never read as "select
                // whatever solid is behind it", and consumes the gesture when it acts.
                let mut handles_took_input = false;

                // ---- MOVE GIZMO ---------------------------------------------
                // A 3-axis translate handle at the selection centre. Shown for a selected
                // SOLID (a wall is reshaped by its vertex handles instead, so the two
                // draggable overlays never fight). Drag an arm to move along that axis;
                // drag the centre cube to slide across the ground plane.
                let gizmo_active = !orbiting
                    && !panning
                    && self.factory.zoom_mode == crate::factory::ZoomMode::Off
                    && self.factory.selected_wall().is_none()
                    && self.factory.has_any_selection()
                    // A RUNNING COMMAND OWNS THE CLICKS. While MOVE is asking for its base point,
                    // a drag that started on a gizmo arm would begin a gizmo drag instead of
                    // answering the prompt — two ways to move the same object, fighting over one
                    // gesture. The command was asked for explicitly, so it wins until it is done.
                    && self.factory.modify.is_none();
                let rotate_mode = self.factory.gizmo_mode == crate::factory::GizmoMode::Rotate;
                if gizmo_active && !rotate_mode {
                    // Begin a drag: pick a handle under the cursor.
                    if self.factory.gizmo_drag.is_none()
                        && resp.drag_started_by(egui::PointerButton::Primary)
                    {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            // Grab a gizmo handle; failing that, if the drag started on the
                            // SELECTED furniture's body, treat it as a free (ground-plane)
                            // move — dragging the object itself should move it, not marquee.
                            let mut h = self.factory.pick_gizmo(pos, rect, &mvp);
                            // Dragging from ANY member of the selection moves the whole set —
                            // grabbing one chair of a selected row and having only that chair
                            // follow would silently break the arrangement you just made.
                            if h.is_none()
                                && self
                                    .factory
                                    .pick_furniture(pos, rect, &mvp)
                                    .is_some_and(|i| self.factory.sel_furniture.contains(&i))
                            {
                                h = Some(crate::factory::GizmoHandle::Free);
                            }
                            if let Some(h) = h {
                                self.snapshot_factory();       // ONE step for the whole drag
                                self.factory.gizmo_drag = Some(h);
                                self.factory.gizmo_start_center =
                                    self.factory.selection_center().unwrap_or(glam::Vec3::ZERO);
                                self.factory.gizmo_grab_ground =
                                    self.factory.cursor_on_plane(pos, rect, &mvp);
                                handles_took_input = true;
                            }
                        }
                    }
                    // Continue a drag.
                    if let Some(h) = self.factory.gizmo_drag {
                        if resp.dragged_by(egui::PointerButton::Primary) {
                            match h.axis() {
                                // AXIS move: incremental — the cursor's motion projected
                                // onto the axis's SCREEN direction, scaled to world units.
                                Some(dir) => {
                                    if let Some(v) = self.factory.gizmo_view(rect, &mvp) {
                                        let arm = v
                                            .arms
                                            .iter()
                                            .flatten()
                                            .find(|a| a.dir == dir);
                                        if let Some(arm) = arm {
                                            let sdir = (arm.tip_s - v.center_s);
                                            let slen = sdir.length();
                                            if slen > 1e-3 {
                                                let sdir = sdir / slen;
                                                let wpp = v.len_w / slen; // world per pixel along axis
                                                let dd = resp.drag_delta();
                                                let along = dd.x * sdir.x + dd.y * sdir.y;
                                                self.factory.move_selection(dir * (along * wpp));
                                                // Furniture isn't in the CSG model — skip the
                                                // (expensive) re-eval when moving it.
                                                if self.factory.sel_furniture.is_empty() {
                                                    self.factory.recompute();
                                                }
                                            }
                                        }
                                    }
                                }
                                // FREE move: absolute — follow the ground point under the
                                // cursor from where it was grabbed (X+Y together, Z fixed).
                                None => {
                                    if let (Some(pos), Some(grab), Some(cur)) = (
                                        resp.interact_pointer_pos(),
                                        self.factory.gizmo_grab_ground,
                                        self.factory.selection_center(),
                                    ) {
                                        if let Some(now) =
                                            self.factory.cursor_on_plane(pos, rect, &mvp)
                                        {
                                            let want = self.factory.gizmo_start_center
                                                + glam::Vec3::new(now.x - grab.x, now.y - grab.y, 0.0);
                                            self.factory
                                                .move_selection(glam::Vec3::new(want.x - cur.x, want.y - cur.y, 0.0));
                                            if self.factory.sel_furniture.is_empty() {
                                                self.factory.recompute();
                                            }
                                        }
                                    }
                                }
                            }
                            handles_took_input = true;
                        }
                        if resp.drag_stopped() {
                            self.factory.gizmo_drag = None;
                            self.factory.gizmo_grab_ground = None;
                            self.factory_note("3D move".into());
                        }
                    }
                }
                // ---- ROTATE mode: drag a ring to spin about its axis ---------------
                if gizmo_active && rotate_mode {
                    if self.factory.rot_drag.is_none()
                        && resp.drag_started_by(egui::PointerButton::Primary)
                    {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            if let Some(h) = self.factory.pick_ring(pos, rect, &mvp) {
                                // rot_begin captures the grab + start angles WITHOUT mutating,
                                // so snapshotting after it still records the pre-rotation state.
                                if self.factory.rot_begin(h, pos, rect, &mvp) {
                                    self.snapshot_factory();
                                    handles_took_input = true;
                                }
                            }
                        }
                    }
                    if self.factory.rot_drag.is_some() {
                        if resp.dragged_by(egui::PointerButton::Primary) {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                self.factory.rot_update(pos, rect, &mvp);
                                if self.factory.sel_furniture.is_empty() {
                                    self.factory.recompute();
                                }
                            }
                            handles_took_input = true;
                        }
                        if resp.drag_stopped() {
                            self.factory.rot_end();
                            self.factory_note("3D rotate".into());
                        }
                    }
                }

                // 2D PLAN underlay — painted on a foreground layer so it shows THROUGH the
                // solids (a plan hidden behind the model would be pointless).
                self.paint_plan_underlay(ui, rect, &mvp);

                // ---- MARQUEE box-select --------------------------------------
                // Left-drag on EMPTY space rubber-bands a box and selects everything whose
                // centre falls inside. Only starts when nothing else claimed the drag (not
                // panning / orbiting / a gizmo or wall-handle grab), so it never fights the
                // move tools. Single-click still picks one object (that path is unchanged).
                let marquee_allowed = !panning
                    && !orbiting
                    && !handles_took_input
                    && self.factory.zoom_mode == crate::factory::ZoomMode::Off
                    && self.factory.gizmo_drag.is_none()
                    && self.factory.rot_drag.is_none()
                    && self.factory.wall_drag.is_none()
                    && self.factory.place_pending.is_none()
                    && self.factory.modify.is_none()
                    && self.factory.session.is_none();
                if marquee_allowed {
                    if self.factory.marquee.is_none()
                        && resp.drag_started_by(egui::PointerButton::Primary)
                    {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            self.factory.marquee = Some((pos, pos));
                        }
                    }
                    if let Some((start, _)) = self.factory.marquee {
                        if resp.dragged_by(egui::PointerButton::Primary) {
                            if let Some(pos) = resp.hover_pos().or(resp.interact_pointer_pos()) {
                                self.factory.marquee = Some((start, pos));
                            }
                        }
                        if resp.drag_stopped() {
                            if let Some((a, b)) = self.factory.marquee.take() {
                                let band = egui::Rect::from_two_pos(a, b);
                                if band.width() > 3.0 || band.height() > 3.0 {
                                    let add = ui.input(|i| i.modifiers.shift);
                                    let (feat, furn) =
                                        self.factory.select_in_marquee(band, rect, &mvp, add);
                                    self.factory_note(match (feat, furn) {
                                        (0, 0) => "3D marquee → nothing inside".into(),
                                        (n, 0) => format!("3D marquee → {n} solid(s)"),
                                        (0, n) => format!("3D marquee → {n} furniture"),
                                        (a, b) => format!("3D marquee → {a} solid(s) + {b} furniture"),
                                    });
                                }
                            }
                        }
                    }
                } else {
                    // Some other tool owns the drag — abandon any half-started band.
                    self.factory.marquee = None;
                }
                // Draw the rubber-band on the foreground layer (over the opaque 3D scene).
                if let Some((a, b)) = self.factory.marquee {
                    let band = egui::Rect::from_two_pos(a, b);
                    let fg = ui.ctx().layer_painter(egui::LayerId::new(
                        egui::Order::Foreground,
                        egui::Id::new("factory_marquee"),
                    )).with_clip_rect(rect);
                    fg.rect_filled(band, 0.0, egui::Color32::from_rgba_unmultiplied(90, 190, 255, 30));
                    fg.rect_stroke(band, 0.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 190, 255)));
                    ui.ctx().request_repaint();
                }

                if !orbiting && !panning && self.factory.zoom_mode == crate::factory::ZoomMode::Off {
                    if let Some(wi) = self.factory.selected_wall() {
                        // --- drag: move a corner ---
                        if resp.drag_started_by(egui::PointerButton::Primary) {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                if let Some(vi) = self.factory.pick_wall_vertex(wi, pos, rect, &mvp) {
                                    // ONE snapshot for the whole drag, not one per frame.
                                    self.snapshot_factory();
                                    self.factory.wall_drag = Some((wi, vi));
                                    handles_took_input = true;
                                }
                            }
                        }
                        if let Some((dwi, dvi)) = self.factory.wall_drag {
                            if resp.dragged_by(egui::PointerButton::Primary) {
                                if let Some(pos) = resp.interact_pointer_pos() {
                                    if let Some(w) = self.factory.cursor_on_plane(pos, rect, &mvp) {
                                        self.factory.wall_move_vertex(
                                            dwi, dvi, glam::Vec2::new(w.x, w.y),
                                        );
                                        // The rebuild minted new ids — re-select so the
                                        // handles survive the rest of the drag.
                                        self.factory.select_wall(dwi);
                                        self.factory.recompute();
                                    }
                                }
                                handles_took_input = true;
                            }
                            if resp.drag_stopped() {
                                self.factory.wall_drag = None;
                                self.factory_note("3D wall: vertex moved".into());
                            }
                        }
                        // --- click an edge midpoint: insert a corner ---
                        if !handles_took_input && resp.clicked() {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                if let Some(si) = self.factory.pick_wall_edge(wi, pos, rect, &mvp) {
                                    if let Some(w) = self.factory.cursor_on_plane(pos, rect, &mvp) {
                                        self.snapshot_factory();
                                        let lost_before = self.factory.orphaned_cutouts().len();
                                        self.factory.wall_insert_vertex(
                                            wi, si, glam::Vec2::new(w.x, w.y),
                                        );
                                        self.factory.select_wall(wi);
                                        self.factory.recompute();
                                        self.history.push("  3D wall: vertex added".into());
                                        self.note_orphaned_openings(lost_before);
                                        handles_took_input = true;
                                        return_after_click = true;
                                    }
                                }
                            }
                        }
                        // --- right-click a dot: delete that corner ---
                        // (Alt+right is orbit, so it must not also delete.)
                        if !alt && resp.secondary_clicked() {
                            if let Some(pos) = resp.interact_pointer_pos() {
                                if let Some(vi) = self.factory.pick_wall_vertex(wi, pos, rect, &mvp) {
                                    self.snapshot_factory();
                                    let lost_before = self.factory.orphaned_cutouts().len();
                                    if self.factory.wall_delete_vertex(wi, vi) {
                                        self.factory.select_wall(wi);
                                        self.factory.recompute();
                                        self.history.push("  3D wall: vertex deleted".into());
                                        self.note_orphaned_openings(lost_before);
                                    } else {
                                        // Refused (a wall needs 2 points) — say why, and
                                        // do not leave a no-op undo step behind.
                                        self.undo_stack.pop();
                                        self.history.push(
                                            "  ! 3D wall: a wall needs at least 2 points".into(),
                                        );
                                    }
                                    handles_took_input = true;
                                }
                            }
                        }
                    }
                }
                // Painted on a FOREGROUND layer — the 3D scene is an opaque texture drawn
                // after this painter, and would hide the handles otherwise.
                if let Some(wi) = self.factory.selected_wall() {
                    if self.factory.zoom_mode == crate::factory::ZoomMode::Off {
                        let fg = ui.ctx().layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::new("factory_wall_handles"),
                        )).with_clip_rect(rect);
                        let hover = resp.hover_pos();
                        // Edge midpoints first, smaller and dimmer: they are the "add"
                        // affordance and must not compete with the corners.
                        for (_, p) in self.factory.wall_edge_handles(wi, rect, &mvp) {
                            fg.circle_filled(
                                p,
                                crate::factory::HANDLE_DRAW_R * 0.6,
                                egui::Color32::from_rgba_unmultiplied(120, 200, 255, 150),
                            );
                        }
                        for (vi, p) in self.factory.wall_vertex_handles(wi, rect, &mvp) {
                            let hot = self.factory.wall_drag.map(|(_, d)| d) == Some(vi)
                                || hover.is_some_and(|h| {
                                    h.distance(p) <= crate::factory::HANDLE_DRAW_R * 2.2
                                });
                            let c = if hot {
                                egui::Color32::from_rgb(255, 210, 90)
                            } else {
                                egui::Color32::from_rgb(90, 190, 255)
                            };
                            fg.circle_filled(p, crate::factory::HANDLE_DRAW_R, c);
                            fg.circle_stroke(
                                p,
                                crate::factory::HANDLE_DRAW_R,
                                egui::Stroke::new(1.2, egui::Color32::from_rgb(20, 30, 40)),
                            );
                        }
                    }
                }

                // ---- MOVE GIZMO drawing --------------------------------------
                // Foreground layer, same reason as the wall handles: the 3D scene is an
                // opaque texture painted over this. Arms are X/Y/Z coloured; the centre
                // cube is the free-move grab.
                if gizmo_active && !rotate_mode {
                    if let Some(v) = self.factory.gizmo_view(rect, &mvp) {
                        let fg = ui.ctx().layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::new("factory_move_gizmo"),
                        )).with_clip_rect(rect);
                        let hover = resp.hover_pos();
                        for arm in v.arms.iter().flatten() {
                            let hot = self.factory.gizmo_drag == Some(arm.handle)
                                || hover.is_some_and(|h| {
                                    crate::factory::seg_dist(h, v.center_s, arm.tip_s) <= 8.0
                                });
                            let w = if hot { 3.5 } else { 2.2 };
                            fg.line_segment(
                                [v.center_s, arm.tip_s],
                                egui::Stroke::new(w, arm.handle.color()),
                            );
                            // Arrowhead: a filled dot at the tip.
                            fg.circle_filled(arm.tip_s, if hot { 5.0 } else { 3.5 }, arm.handle.color());
                        }
                        // Centre cube — the free (combination) move.
                        let free_hot = self.factory.gizmo_drag == Some(crate::factory::GizmoHandle::Free)
                            || hover.is_some_and(|h| h.distance(v.center_s) <= 9.0);
                        let s = if free_hot { 6.0 } else { 4.5 };
                        let cube = egui::Rect::from_center_size(v.center_s, egui::vec2(s * 2.0, s * 2.0));
                        fg.rect_filled(cube, 1.5, egui::Color32::from_rgb(240, 240, 240));
                        fg.rect_stroke(cube, 1.5, egui::Stroke::new(1.2, egui::Color32::from_rgb(30, 40, 50)));
                    }
                }
                // ---- ROTATE GIZMO drawing (three rings) ----------------------
                if gizmo_active && rotate_mode {
                    if let Some(rv) = self.factory.rotation_rings(rect, &mvp) {
                        let fg = ui.ctx().layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::new("factory_rotate_gizmo"),
                        )).with_clip_rect(rect);
                        let hover = resp.hover_pos();
                        for ring in &rv.rings {
                            let hot = self.factory.rot_drag.as_ref().map(|d| d.handle) == Some(ring.handle)
                                || hover.is_some_and(|h| ring.pts.windows(2)
                                    .any(|s| crate::factory::seg_dist(h, s[0], s[1]) <= 8.0));
                            let w = if hot { 3.5 } else { 2.0 };
                            for s in ring.pts.windows(2) {
                                fg.line_segment([s[0], s[1]], egui::Stroke::new(w, ring.handle.color()));
                            }
                        }
                        // small centre dot for reference
                        fg.circle_filled(rv.center_s, 3.0, egui::Color32::from_rgb(230, 230, 230));
                    }
                }

                // ---- PER-SURFACE TARGET highlight ---------------------------
                // Outline the face/piece the user clicked (Face/Piece texture mode), so it's clear
                // that a SURFACE — not the whole object — is targeted.
                if self.factory.furn_face_sel.is_some() {
                    let segs = self.factory.furniture_face_highlight_segments(rect, &mvp);
                    if !segs.is_empty() {
                        let fg = ui.ctx().layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::new("factory_face_highlight"),
                        )).with_clip_rect(rect);
                        for s in &segs {
                            fg.line_segment(*s, egui::Stroke::new(2.0, egui::Color32::from_rgb(80, 220, 255)));
                        }
                        // A highlight that stops halfway round a face reads as a hole in the
                        // face, so say it was cut short rather than let the picture lie.
                        if self.factory.face_highlight_truncated() {
                            fg.text(
                                rect.left_bottom() + egui::vec2(8.0, -8.0),
                                egui::Align2::LEFT_BOTTOM,
                                "outline clipped — selection is very large",
                                egui::FontId::proportional(11.0),
                                egui::Color32::from_rgb(80, 220, 255),
                            );
                        }
                    }
                }

                // ---- LEFT CLICK = SELECT (never camera) ----------------------
                // Part B of the mouse rule: the left button is the selector. Shift
                // adds, a click on empty space clears. The camera is untouched.
                // A gizmo drag consumed the click, so selection must not also run.
                if resp.clicked() && !handles_took_input {
                    if let Some(pos) = resp.interact_pointer_pos() {
                    }
                }
                if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        // PLACEMENT owns the click first: a dialog-built primitive waiting
                        // for its point. The click places it (Box corner / others centred);
                        // a missed plane stays armed.
                        if let Some(prim) = self.factory.place_pending.take() {
                            let snapped = self.factory.snap_vertex(pos, rect, &mvp).map(|(w, _)| w);
                            if let Some(w) = snapped.or_else(|| self.factory.cursor_on_plane(pos, rect, &mvp)) {
                                // Snapshot only once the plane is actually hit — a missed
                                // click stays armed and changes nothing, so it must not
                                // leave an empty undo step behind.
                                self.snapshot_factory();
                                self.factory.place_primitive(prim, w);
                                self.factory.status.clear();
                                self.factory_note(format!("3D place ✓ ({:.2},{:.2})", w.x, w.y));
                            } else {
                                self.factory.place_pending = Some(prim); // missed the plane
                            }
                            return_after_click = true;
                        }
                        // …then the object that was just ADDED and is waiting to be told where it
                        // goes. Same rule as above: a click that misses the plane stays armed.
                        else if self.factory.awaiting_place.is_some() {
                            let snapped = self.factory.snap_vertex(pos, rect, &mvp).map(|(w, _)| w);
                            if let Some(w) =
                                snapped.or_else(|| self.factory.cursor_on_plane(pos, rect, &mvp))
                            {
                                self.factory_finish_awaited_placement(w);
                            }
                            return_after_click = true;
                        }
                        // A running op owns the click — it is a POINT (step 4/5), not a
                        // selection. Same cascade rule as the 2D canvas.
                        else if let Some(mut md) = self.factory.modify.take() {
                            let snapped = self.factory.snap_vertex(pos, rect, &mvp).map(|(w, _)| w);
                            let w = snapped.or_else(|| self.factory.cursor_on_plane(pos, rect, &mvp));
                            if let Some(w) = w {
                                let plane = cad_solid::Plane::default();
                                let pick = md.pick_name();
                                let snaptag = if snapped.is_some() { "END" } else { "none" };
                                let f = md.feed(w, &plane, &mut self.factory.model, self.env.CrdEnb);
                                self.factory_note(format!(
                                    "3D {} {pick} = ({:.2},{:.2},{:.2}) [snap={snaptag}] → {f:?}",
                                    md.op.label(), w.x, w.y, w.z
                                ));
                                self.factory_after_feed(md, f);
                            } else {
                                self.factory.modify = Some(md); // missed the plane → stay armed
                            }
                            return_after_click = true;
                        }
                    }
                }
                // PATH-SWEEP flow — Esc cancels it and leaves any open sketch.
                if self.factory.sweep_flow.is_some()
                    && ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    self.factory.sweep_flow = None;
                    if self.factory.session.is_some() { self.factory_exit_sketch(); }
                    self.factory.status = "path sweep cancelled".into();
                }
                // Esc DESELECTS a targeted face/piece and disarms the texture brush (the sweep
                // flow above owns Esc while it runs).
                else if (self.factory.furn_face_sel.is_some() || self.factory.furn_tex_brush.is_some())
                    && ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    self.factory.furn_face_sel = None;
                    self.factory.furn_tex_brush = None;
                    self.factory.status = "face/piece deselected".into();
                }
                // PATH-SWEEP flow — Enter finishes the current stage (section → offer views;
                // path → build). Only while a sketch is open, so it doesn't clash elsewhere.
                if self.factory.sweep_flow.is_some()
                    && self.factory.session.is_some()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    self.factory_finish_sweep_stage();
                }
                // PER-SURFACE FURNITURE — a furniture object is SELECTED and the user clicks ON IT
                // again: the click drills into the face/piece under the cursor (the DCC convention:
                // first click = object, second click = face) — NO mode prerequisite. The Face/Piece
                // scope comes from `furn_paint_mode` when set; a drill from Whole-object mode
                // auto-switches to Face so the Apply UI and the click agree. Two orders both work:
                //  • a texture was picked first (brush armed) → the click TEXTURES the face now;
                //  • no brush yet → the click TARGETS the face (furn_face_sel); the next texture
                //    applied lands on it ("click a face, then pick a texture").
                // A click that MISSES the selected object falls through to normal selection.
                // `furn_handled` suppresses the normal selection/paint branches when we consumed it.
                let mut furn_handled = false;
                // Face painting acts on the PRIMARY: a face belongs to one object.
                let furn_primary = self.factory.sel_furn_primary();
                if furn_primary.is_some()
                    && resp.clicked()
                    && !return_after_click
                {
                    if let (Some(pos), Some(fi)) = (resp.interact_pointer_pos(), furn_primary) {
                        let piece = self.factory.furn_paint_mode == crate::factory::FurnPaintMode::Piece;
                        let hit = self.factory.furniture_face_at(fi, pos, rect, &mvp, piece);
                        // Diagnostic: which face/piece the click resolved to (or why it missed).
                        if self.dbg.recording {
                            let asset_tris = self.factory.sel_furn_primary()
                                .and_then(|i| self.factory.furniture.get(i))
                                .and_then(|inst| self.factory.furniture_lib.get(inst.asset))
                                .map(|a| a.positions.len() / 3)
                                .unwrap_or(0);
                            let has_parts = self.factory.furniture_has_parts(fi);
                            // How many TRIANGLES the picked groups cover (the real "how much will
                            // this paint" number — a group count of 1 can still be the whole side).
                            let sel_tris = hit.as_ref().map(|g| {
                                let set: std::collections::HashSet<u32> = g.iter().copied().collect();
                                self.factory.furniture.get(fi)
                                    .and_then(|inst| self.factory.furniture_lib.get(inst.asset))
                                    .map(|a| a.group_geom().face.iter().filter(|f| set.contains(f)).count())
                                    .unwrap_or(0)
                            }).unwrap_or(0);
                            let msg = match &hit {
                                Some(g) => format!(
                                    "FURN FACE pick @({:.0},{:.0}) inst#{fi} mode={} → {} group(s) = {sel_tris}/{asset_tris} tris {:?} (part_ids={has_parts}, brush={:?})",
                                    pos.x, pos.y, if piece { "piece" } else { "face" }, g.len(),
                                    &g[..g.len().min(6)], self.factory.furn_tex_brush,
                                ),
                                // `furniture_face_at` returns None for two unrelated reasons, and
                                // reporting both as "the ray missed" sends the reader hunting a
                                // pick bug that is not there. A decimated mesh REFUSES the pick
                                // outright — face painting simply does not work on it — which is
                                // a different problem with a different fix.
                                None if self.factory.furniture.get(fi)
                                    .and_then(|inst| self.factory.furniture_lib.get(inst.asset))
                                    .is_some_and(|a| a.needs_lod()) => format!(
                                    "FURN FACE pick @({:.0},{:.0}) inst#{fi} mode={} → REFUSED: \
                                     {asset_tris}-triangle mesh is decimated for display, so face \
                                     picking is unavailable on it (not a miss)",
                                    pos.x, pos.y, if piece { "piece" } else { "face" },
                                ),
                                None => format!(
                                    "FURN FACE pick @({:.0},{:.0}) inst#{fi} mode={} → MISS (ray didn't hit the mesh)",
                                    pos.x, pos.y, if piece { "piece" } else { "face" },
                                ),
                            };
                            crate::dbg_event!(self, crate::dbg_recorder::DbgEvent::Note { message: msg });
                        }
                        if let Some(groups) = hit {
                            furn_handled = true;
                            // Drilling from Whole-object auto-switches the scope to Face so the
                            // Apply UI (Materials Factory / Textures panel) matches what's selected.
                            if self.factory.furn_paint_mode == crate::factory::FurnPaintMode::WholeObject {
                                self.factory.furn_paint_mode = crate::factory::FurnPaintMode::Face;
                            }
                            if let Some(ti) = self.factory.furn_tex_brush {
                                self.snapshot_factory();
                                self.factory.apply_face_texture(fi, &groups, ti);
                                // Painted → no lingering selection (the brush stays armed for the
                                // next face; Esc disarms it).
                                self.factory.furn_face_sel = None;
                                self.materials.sel = Some(ti); // MF follows the painted material
                                self.factory.status = "painted — click more faces, or Esc to stop".into();
                                self.history.push(format!("  furniture {} textured", if piece { "piece" } else { "face" }));
                            } else {
                                // SYNC the Materials Factory to the clicked face's material, so its
                                // properties (Alpha/colour/tiling in the node editor) edit exactly
                                // what was clicked — the reason "change the opacity of a face"
                                // silently did nothing before: the edit landed on whichever
                                // material happened to be selected there.
                                let face_mat = groups.first().and_then(|g| {
                                    self.factory
                                        .furniture
                                        .get(fi)
                                        .and_then(|inst| inst.surface_texture.get(g).copied().or(inst.texture))
                                });
                                self.factory.furn_face_sel = Some((fi, groups));
                                if let Some(mi) = face_mat {
                                    if self.materials.sel != Some(mi) {
                                        // Re-seed the graph from the material's CURRENT state, so
                                        // the editor opens showing the truth (not a stale seed).
                                        self.materials.graphs.remove(&mi);
                                    }
                                    self.materials.sel = Some(mi);
                                    self.materials.sel_node = None;
                                    let mname = self.factory.textures.get(mi).map(|t| t.name.clone()).unwrap_or_default();
                                    self.factory.status = format!(
                                        "{} selected — its material '{mname}' is open in 🎨 Materials Factory (edit Alpha there for opacity)",
                                        if piece { "piece" } else { "face" }
                                    );
                                } else {
                                    self.factory.status = format!(
                                        "{} selected (no material yet) — Apply one from 🎨 Materials Factory or ▼ Textures",
                                        if piece { "piece" } else { "face" }
                                    );
                                }
                            }
                        }
                        // A miss (clicked empty space / off the object) falls through to selection.
                    }
                }
                // NOTHING PAINTS UNLESS THE MATERIALS FACTORY IS OPEN.
                //
                // Reported as: "when i click on a surface it applied a texture to it, i cant select
                // a building … a texture application should only work when materials factory window
                // is open."
                //
                // Loading or pasting an image ARMS a face brush — `apply_texture_index_to_selection`
                // does that whenever nothing is selected — and the brush used to be consumed by any
                // click anywhere, with the Materials Factory shut and nothing on screen saying a
                // brush was live. So every click painted a face instead of selecting the building,
                // and the only way out was to notice a mode you were never told you were in.
                //
                // The rule is now the user's: the Materials Factory window is the paint tool. Shut,
                // a click selects — always. Open, an ARMED brush paints the clicked face, and with
                // no brush the click still selects normally AND opens that face's material, so the
                // building stays selectable either way.
                let brush_painted = if self.materials_open
                    && self.factory.surface_tex_brush.is_some()
                    && resp.clicked()
                    && !return_after_click
                    && !furn_handled
                {
                    let ti = self.factory.surface_tex_brush.unwrap_or_default();
                    match resp.interact_pointer_pos() {
                        Some(pos) => {
                            self.snapshot_factory();
                            let hit = self.factory.paint_surface_texture(pos, rect, &mvp, ti);
                            if hit {
                                self.factory.recompute();
                                self.history.push("  surface textured".into());
                            } else {
                                self.undo_stack.pop(); // missed — no-op, drop the snapshot
                                self.factory.status =
                                    "click a surface to apply the material to it".into();
                            }
                            hit
                        }
                        None => false,
                    }
                } else {
                    false
                };
                if furn_handled {
                    // consumed by per-surface furniture handling above
                }
                else if brush_painted {
                    // consumed by the Materials Factory's brush
                }
                else if resp.clicked() && !return_after_click {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let add = ui.input(|i| i.modifiers.shift);
                        // Pick the nearest furniture, the nearest APERTURE, and the nearest feature
                        // — each WITH its ray depth. An aperture (door/window) is embedded flush in
                        // the wall, so it gets PRIORITY: if it is hit and sits within its own
                        // thickness of the nearest other surface, it wins even when the wall (or
                        // another furniture piece) is marginally in front. Tracking the aperture
                        // separately fixes the inconsistency where the "nearest furniture" was some
                        // other piece and the aperture never got considered.
                        let (fur, ap) = self.factory.pick_furniture_ex(pos, rect, &mvp);
                        let feat = self.factory.pick_feature_t(pos, rect, &mvp);
                        // Depth of the nearest NON-aperture surface (other furniture or the feature).
                        let other_t = {
                            let f_t = fur
                                .filter(|&(fi, _)| ap.map_or(true, |(ai, _)| ai != fi))
                                .map(|(_, t)| t);
                            [f_t, feat.map(|(_, t)| t)].into_iter().flatten().fold(f32::INFINITY, f32::min)
                        };
                        let ap_wins = ap.map_or(false, |(ai, at)| {
                            at <= other_t + self.factory.aperture_pick_tol(ai)
                        });

                        // Diagnostic: exact picks + depths + decision, so a mis-selection is
                        // answerable straight from a session dump.
                        if self.dbg.recording {
                            let r3 = |t: f32| (t * 1000.0).round() / 1000.0;
                            let decision = if ap_wins {
                                format!("aperture#{}", ap.map(|(i, _)| i).unwrap_or(usize::MAX))
                            } else {
                                match (fur, feat) {
                                    (Some((fi, ft)), Some((id, gt))) =>
                                        if ft <= gt + 0.02 { format!("furniture#{fi}") } else { format!("feature#{id}") },
                                    (Some((fi, _)), None) => format!("furniture#{fi}"),
                                    (None, Some((id, _))) => format!("feature#{id}"),
                                    (None, None) => "none".into(),
                                }
                            };
                            let msg = format!(
                                "3D pick @({:.1},{:.1}): fur={:?} ap={:?} feat={:?} other_t={:.3} tol={:.3} → {decision}",
                                pos.x, pos.y,
                                fur.map(|(i, t)| (i, r3(t))),
                                ap.map(|(i, t)| (i, r3(t))),
                                feat.map(|(id, t)| (id, r3(t))),
                                if other_t.is_finite() { other_t } else { -1.0 },
                                ap.map(|(ai, _)| self.factory.aperture_pick_tol(ai)).unwrap_or(0.0),
                            );
                            crate::dbg_event!(self, crate::dbg_recorder::DbgEvent::Note { message: msg });
                        }

                        // Helper: select a feature id, honouring shift-add. Picking a grouped
                        // feature selects the WHOLE group so it behaves as one entity.
                        let mut select_feature = |app: &mut Self, id: u32| {
                            app.factory.sel_furniture.clear();
                            if add {
                                if !app.factory.selection.contains(&id) { app.factory.selection.push(id); }
                            } else {
                                app.factory.selection = vec![id];
                            }
                            app.factory.expand_selection_to_groups();
                        };
                        // Shift-click a piece of furniture to ADD it to the selection (or take it
                        // back out), the same gesture that already extends a feature selection.
                        let mut select_furn = |app: &mut Self, i: usize| {
                            if add {
                                app.factory.toggle_furniture(i);
                            } else {
                                app.factory.select_furniture(i);
                            }
                        };

                        if let (true, Some((ai, _))) = (ap_wins, ap) {
                            select_furn(self, ai);
                        } else {
                            match (fur, feat) {
                                (Some((fi, ft)), Some((id, gt))) => {
                                    // 2 cm tolerance so furniture flush against a wall still wins.
                                    if ft <= gt + 0.02 {
                                        select_furn(self, fi);
                                    } else {
                                        select_feature(self, id);
                                    }
                                }
                                (Some((fi, _)), None) => select_furn(self, fi),
                                (None, Some((id, _))) => select_feature(self, id),
                                (None, None) => {
                                    // No solid/furniture hit — try a 2D plan shape on the ground,
                                    // so geometry drawn in 2D can be selected FROM the 3D view and
                                    // then extruded / cut.
                                    if let Some(i) = self.factory_pick_ground_dobject(pos, rect, &mvp) {
                                        self.factory.clear_selection(); // drop any 3D feature selection
                                        if add {
                                            if !self.selection.contains(&i) {
                                                self.selection.push(i);
                                            }
                                        } else {
                                            self.selection = vec![i];
                                        }
                                        self.factory.status =
                                            "2D shape selected — ▼ Room elements ▸ Extrude / Cut".into();
                                    } else if !add {
                                        self.factory.clear_selection();
                                        self.selection.clear();
                                    }
                                }
                            }
                        }
                    }
                }
                // …AND THE MATERIALS FACTORY FOLLOWS THE SAME CLICK.
                //
                // "When the user clicks on a surface the materials factory will show every
                // parameter related to it." It runs AFTER the selection above rather than instead
                // of it, which is the whole difference from the version that broke: choosing what
                // to edit must not cost you the ability to select the building.
                //
                // The face is given its own material if it has none, so editing it cannot silently
                // repaint every other face sharing the object's material.
                if self.materials_open
                    && resp.clicked()
                    && !return_after_click
                    && !furn_handled
                    && !brush_painted
                {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        if let Some(key) = self.factory.pick_surface_key(pos, rect, &mvp) {
                            self.materials_select_surface(key);
                        }
                    }
                }
                // ---- RIGHT-CLICK → pick a face → sketch on it ----------------
                // The face under the cursor becomes a `Frame`; the context menu then
                // hands that plane to the app's real 2D drafting.
                // Skipped while ALT is held — that combination is the orbit gesture,
                // so it must not also open a menu.
                if resp.secondary_clicked() && !alt {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        self.factory.pending_face = self.factory.pick_face(pos, rect, &mvp);
                    }
                }
                let pending = self.factory.pending_face;
                let sketching = self.factory.session.is_some();
                let mut want_sketch: Option<cad_solid::Frame> = None;
                let mut want_finish = false;
                let mut want_choose_view: Option<usize> = None;
                // Square the view up to a face. Deferred like the others — the menu closure cannot
                // borrow `self` mutably while `pending` is read from it.
                let mut want_face_on: Option<cad_solid::Frame> = None;

                // PATH-SWEEP "which view?" overlay — shown after the cross-section is finished.
                // Offers the two perpendicular planes for drawing the path (path defines where
                // the swept solid runs). Deferred via `want_choose_view` (can't borrow self here).
                if let Some(flow) = self.factory.sweep_flow.as_ref() {
                    if flow.stage == crate::factory::SweepStage::ChooseView {
                        let views = flow.views.clone();
                        egui::Area::new(egui::Id::new("sweep_view_picker"))
                            .fixed_pos(rect.left_top() + egui::vec2(10.0, 40.0))
                            .show(ui.ctx(), |ui| {
                                egui::Frame::popup(ui.style()).show(ui, |ui| {
                                    ui.label(egui::RichText::new("Draw the PATH on which view?").strong());
                                    ui.label(egui::RichText::new("(perpendicular to the section)").small().weak());
                                    for (i, (name, _)) in views.iter().enumerate() {
                                        if ui.button(format!("▦  {name} view")).clicked() {
                                            want_choose_view = Some(i);
                                        }
                                    }
                                    if ui.button("✕  Cancel").clicked() {
                                        want_choose_view = Some(usize::MAX);
                                    }
                                });
                            });
                    }
                }
                resp.context_menu(|ui| {
                    ui.set_min_width(210.0);
                    if sketching {
                        if ui.button("✔  Finish sketch").clicked() {
                            want_finish = true;
                            ui.close_menu();
                        }
                        return;
                    }
                    match pending {
                        Some(f) => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "face @ ({:.2}, {:.2}, {:.2})",
                                    f.origin.x, f.origin.y, f.origin.z
                                ))
                                .small()
                                .weak(),
                            );
                            // Square the view up FIRST — listed above "draw" because on a face at
                            // an angle it is the difference between drafting and guessing.
                            if ui
                                .button("⊾  Look square-on at this face")
                                .on_hover_text(
                                    "Orbit until you are looking straight at this face, and switch \
                                     to parallel projection.\n\n\
                                     On a face turned away from you, a pixel of mouse movement is \
                                     a long way across the face, so nothing drawn on it can be \
                                     placed accurately. Square-on, a millimetre on screen is a \
                                     millimetre on the face — everywhere on it.",
                                )
                                .clicked()
                            {
                                want_face_on = Some(f);
                                ui.close_menu();
                            }
                            if ui
                                .button("✎  Draw on this face")
                                .on_hover_text("Sketch here with the FULL 2D toolset")
                                .clicked()
                            {
                                want_sketch = Some(f);
                                ui.close_menu();
                            }
                        }
                        None => {
                            ui.label(
                                egui::RichText::new("no face under the cursor").small().weak(),
                            );
                        }
                    }
                    ui.separator();
                    if ui.button("▦  Draw on ground plane").clicked() {
                        want_sketch = Some(crate::factory::FactoryState::ground_frame());
                        ui.close_menu();
                    }
                });

                // Opaque solids + placed furniture, from a cache that only rebuilds when
                // the scene actually changes — so orbiting past a heavy imported mesh no
                // longer re-transforms every vertex each frame (was the post-import lag).
                //
                // ---- PERF MONITOR (session recorder) -----------------------
                // Furniture is a triangle-soup mesh INSTANCE, invisible to FactoryOp's
                // feature/body counts, so a 90k-tri import's LOAD and frame cost show up
                // nowhere else. Time the buffer build, detect a rebuild by Arc identity, and
                // measure the whole-frame delta — then log it on the import/edit moment (a
                // rebuild) or on a slow orbit frame (throttled so it can't flood the dump).
                let now = std::time::Instant::now();
                let mut interval_us = self
                    .factory_perf_last_frame
                    .map(|t| now.saturating_duration_since(t).as_micros() as u64)
                    .unwrap_or(0);
                // A >1 s "frame" means the 3D view was closed/idle, not a stall — don't
                // mistake reopening the view for a dropped frame.
                if interval_us > 1_000_000 { interval_us = 0; }
                self.factory_perf_last_frame = Some(now);
                // WORK, NOT INTERVAL. The delta between paints is not the frame's cost: the app
                // idles at a 5 Hz heartbeat (`request_repaint_after(200 ms)`), so an untouched
                // frame reads 200 ms-plus and clears any sane SLOW bar while doing nothing at all.
                // `frame_us` is now the time spent in THIS frame's `update`, which is what the
                // threshold below was always meant to test.
                let frame_us = self
                    .frame_start
                    .map(|t| now.saturating_duration_since(t).as_micros() as u64)
                    .unwrap_or(interval_us);

                // Push the resolved sun light to the (thread-local) shader BEFORE any shaded buffer
                // is built this frame, so the scene + furniture bake under the current daylight.
                // Also derive the per-frame sun uniforms + shadow-map matrix passed to render().
                let (sun_en, sun_dir, sun_col, env_render) = self.factory.scene_env();
                // Cloned out of the borrow so the paint callback (which owns nothing of `self`) can
                // hand the prefiltered chain to the renderer. Only ~10 MB, and only while an HDRI
                // is loaded; the version means it is uploaded once, not once a frame.
                let env_chain = self.factory.env_chain.clone();
                let env_source = self.factory.env_map.clone(); // Arc — the 4K pixels are not copied
                let env_version = self.factory.env_version;
                crate::factory::set_sun_light(sun_en, sun_dir, sun_col, env_render.sh);
                crate::factory::set_clay(self.factory.clay_mode);
                let clay = self.factory.clay_mode;
                // Display transform for this frame (exposure + view transform + sRGB encode). The
                // 3D Factory renders scene-referred linear light, so it always wants a real one.
                let color_pipeline = self.factory.color;
                // Materials Factory selection → pulse-highlight that material's surfaces in the
                // scene, so the user sees WHERE it is while tuning it. Only while the editor is open.
                let mf_highlight: Option<(usize, f32)> = if self.materials_open {
                    self.materials.sel.map(|i| {
                        let t = ctx.input(|inp| inp.time);
                        (i, (0.55 + 0.45 * (t * 4.0).sin()) as f32)
                    })
                } else {
                    None
                };
                // Only the DIRECT sun travels here now; the ambient half is the sky in `env_render`.
                let sun_uniform: Option<([f32; 3], [f32; 3])> =
                    if sun_en { Some(([sun_dir.x, sun_dir.y, sun_dir.z], sun_col)) } else { None };
                // Shadows only while the sun is meaningfully above the horizon (a near-horizon sun
                // makes an enormous, useless frustum). Framed to the whole drawn scene.
                let shadow_mvp: Vec<[f32; 16]> = if sun_en && self.factory.sun.shadows && sun_dir.z > 0.05 {
                    match self.factory.render_bounds() {
                        Some(b) => {
                            let eye = crate::light3d::cam_eye(
                                self.factory.cam_yaw, self.factory.cam_pitch,
                                self.factory.cam_dist, self.factory.cam_target,
                            );
                            let c = sun_cascades(
                                &mvp, eye.into(), b, sun_dir,
                                self.factory.sun.shadow_cascades as usize,
                                crate::light3d::SHADOW_MAP_SIZE,
                            );
                            // A degenerate camera (an un-invertible matrix mid-resize) leaves the
                            // cascade fit with nothing to work from. Fall back to the single
                            // whole-scene map rather than dropping shadows for that frame.
                            if c.is_empty() { vec![sun_light_matrix(b.0, b.1, sun_dir)] } else { c }
                        }
                        None => Vec::new(),
                    }
                } else {
                    Vec::new()
                };
                let t_build = std::time::Instant::now();
                let verts = self.factory.opaque_verts();
                let build_us = t_build.elapsed().as_micros() as u64;
                // The flat ground surface, if the GND badge is on: one camera-following quad
                // through the per-frame opaque slot (`dyn_verts`) — it must NOT ride in the
                // model-keyed cache, because it moves with the camera and would rebuild the
                // whole heavy buffer every frame.
                let ground = self.factory.ground_plane_verts();

                // FURNITURE: one GPU-instanced draw per piece. Each mesh is baked to LOCAL space
                // ONCE (cached by asset+colour) and uploaded to its own GPU buffer once by the
                // renderer; here we only compute the per-instance model matrix (camera·model).
                // So importing / moving / rotating furniture — even a multi-million-triangle
                // piece — never CPU-transforms or re-uploads the mesh (that was the stutter).
                let mvp_m = glam::Mat4::from_cols_array(&mvp);
                const IDENT16: [f32; 16] =
                    [1.0,0.0,0.0,0.0, 0.0,1.0,0.0,0.0, 0.0,0.0,1.0,0.0, 0.0,0.0,0.0,1.0];
                // Each opaque draw also carries its WORLD model matrix (last [f32;16]) so the sun
                // shadow pass + normal maps can transform local geometry to world.
                // `(key, mesh, camera·model, world model, in_view)` — see `aabb_in_frustum` for why
                // the off-screen ones are carried in the list rather than dropped from it.
                let mut furn_draws: Vec<(u64, StdArc<Vec<crate::light3d::V3>>, [f32; 16], [f32; 16], bool)> =
                    Vec::with_capacity(self.factory.furniture.len());
                // Textured furniture is peeled off into its own list (image-mapped pass); the
                // rest stay flat-shaded. `tex_assets_owned` carries the pixels the pass needs.
                let mut tex_draws_owned: Vec<(usize, u64, StdArc<Vec<crate::light3d::TexVtx>>, [f32; 16], [f32; 16])> =
                    Vec::new();
                // Translucent furniture (glass panes), tagged with a camera-space depth so the
                // blended pass can be sorted back-to-front. Sorted + stripped of the depth below.
                let mut transp_draws_owned: Vec<(f32, u64, StdArc<Vec<crate::light3d::V3A>>, [f32; 16], [f32; 16])> =
                    Vec::new();
                // Translucent TEXTURED furniture (glass that shows an image), same depth tagging + model.
                let mut tex_transp_draws_owned: Vec<(f32, usize, u64, StdArc<Vec<crate::light3d::TexVtx>>, [f32; 16], [f32; 16])> =
                    Vec::new();
                let mut needed_tex: std::collections::HashSet<usize> = std::collections::HashSet::new();
                for i in 0..self.factory.furniture.len() {
                    let model = self.factory.furniture_model_matrix(i).unwrap_or(IDENT16);
                    let fmvp = (mvp_m * glam::Mat4::from_cols_array(&model)).to_cols_array();
                    // Depth (NDC z of the instance origin) for back-to-front sorting of any glass.
                    let origin = glam::Vec3::new(model[12], model[13], model[14]);
                    let depth = mvp_m.project_point3(origin).z;
                    // IS ANY OF IT ON SCREEN? Against the asset's own cached local bounds, through
                    // `fmvp` — no per-instance world AABB to build or keep in step with a move.
                    // An asset that has somehow gone missing counts as visible: the draw below will
                    // find nothing to draw, and guessing "invisible" would hide a real piece.
                    let in_view = self
                        .factory
                        .furniture_lib
                        .get(self.factory.furniture[i].asset)
                        .map(|a| crate::light3d::aabb_in_frustum(&fmvp, a.local_min, a.local_max))
                        .unwrap_or(true);
                    // PER-SURFACE textured piece: split into one textured group per texture used
                    // plus a flat remainder. Only returns Some when the instance has per-face
                    // textures (else the whole-object paths below handle it unchanged).
                    if let Some(fac) = self.factory.furniture_faceted(i) {
                        // `fac` is a cached Arc; the inner Vec is cloned into the GPU-mesh cache only
                        // on a miss (first frame after a paint), never on steady frames.
                        for (tex_idx, key, verts) in &fac.opaque {
                            let mesh = self
                                .furniture_tex_meshes
                                .entry(*key)
                                .or_insert_with(|| StdArc::new(verts.clone()))
                                .clone();
                            needed_tex.insert(*tex_idx);
                            tex_draws_owned.push((*tex_idx, *key, mesh, fmvp, model));
                        }
                        // See-through textured faces → the blended textured pass (depth-sorted).
                        for (tex_idx, key, verts) in &fac.translucent {
                            let mesh = self
                                .furniture_tex_meshes
                                .entry(*key)
                                .or_insert_with(|| StdArc::new(verts.clone()))
                                .clone();
                            needed_tex.insert(*tex_idx);
                            tex_transp_draws_owned.push((depth, *tex_idx, *key, mesh, fmvp, model));
                        }
                        if let Some((key, verts)) = &fac.flat {
                            let mesh = self
                                .furniture_gpu_meshes
                                .entry(*key)
                                .or_insert_with(|| StdArc::new(verts.clone()))
                                .clone();
                            furn_draws.push((*key, mesh, fmvp, model, in_view));
                        }
                        continue;
                    }
                    let tex_op = self.factory.furniture_textured_mesh(i);
                    let tex_tr = self.factory.furniture_textured_translucent_mesh(i);
                    if tex_op.is_some() || tex_tr.is_some() {
                        // A textured piece: opaque faces in the image pass, glass in the blended one.
                        if let Some((tex_idx, key, verts)) = tex_op {
                            let mesh = self
                                .furniture_tex_meshes
                                .entry(key)
                                .or_insert_with(|| StdArc::new(verts))
                                .clone();
                            needed_tex.insert(tex_idx);
                            tex_draws_owned.push((tex_idx, key, mesh, fmvp, model));
                        }
                        if let Some((tex_idx, key, verts)) = tex_tr {
                            let mesh = self
                                .furniture_tex_meshes
                                .entry(key)
                                .or_insert_with(|| StdArc::new(verts))
                                .clone();
                            needed_tex.insert(tex_idx);
                            tex_transp_draws_owned.push((depth, tex_idx, key, mesh, fmvp, model));
                        }
                    } else {
                        let key = self.factory.furniture_key(i);
                        if !self.furniture_gpu_meshes.contains_key(&key) {
                            let mesh = StdArc::new(self.factory.furniture_local_mesh(i));
                            self.furniture_gpu_meshes.insert(key, mesh);
                        }
                        let mesh = self.furniture_gpu_meshes[&key].clone();
                        furn_draws.push((key, mesh, fmvp, model, in_view));
                        // Peel this piece's see-through triangles into the blended pass.
                        // THE KEY FIRST, THE GEOMETRY ONLY ON A MISS. This called
                        // `furniture_translucent_mesh(i)` unconditionally and handed the result to
                        // `or_insert_with`, which DROPS it on a cache hit: the peel walked all six
                        // windows' 39,667 triangles apiece every frame to build vectors that were
                        // then thrown away. Measured at 0.36 ms/frame here — small, but it is the
                        // same shape as the bug that once cost 600 ms/frame in `furniture_faceted`,
                        // and it grows with the model.
                        if let Some(tkey) = self.factory.furniture_translucent_key(i) {
                            if !self.furniture_transp_meshes.contains_key(&tkey) {
                                if let Some((_, tverts)) = self.factory.furniture_translucent_mesh(i)
                                {
                                    self.furniture_transp_meshes.insert(tkey, StdArc::new(tverts));
                                }
                            }
                            let tmesh = match self.furniture_transp_meshes.get(&tkey) {
                                Some(m) => m.clone(),
                                None => continue, // no see-through triangles after all
                            };
                            // The WORLD model too, not only camera·model: the glass shader has to
                            // build a normal, and doing that from a depth-buffer reconstruction is
                            // what made every pane speckle on a plan sited at 6852 m. See TRANSP_VS.
                            transp_draws_owned.push((depth, tkey, tmesh, fmvp, model));
                        }
                    }
                }
                // Back-to-front for correct alpha blending between separate glass pieces.
                transp_draws_owned.sort_by(|a, b| b.0.total_cmp(&a.0));
                tex_transp_draws_owned.sort_by(|a, b| b.0.total_cmp(&a.0));
                // Camera eye — for the reflection sheen AND to depth-sort translucent features.
                let cam_pos = crate::light3d::cam_eye(
                    self.factory.cam_yaw, self.factory.cam_pitch, self.factory.cam_dist, self.factory.cam_target,
                );
                // Textured FEATURE surfaces (walls/floors, CSG solids) — world-space, grouped by
                // texture, re-uploaded each frame. Split by the texture's opacity: opaque groups
                // draw in the normal pass; see-through ones (a tuned CSG solid) go to the blended
                // pass, sorted back-to-front so overlapping translucent solids read correctly.
                let mut tex_feat_owned: Vec<(usize, Vec<crate::light3d::TexVtx>)> = Vec::new();
                let mut tex_feat_transp_owned: Vec<(usize, Vec<crate::light3d::TexVtx>)> = Vec::new();
                // Keep the shader's triplanar lookup on the SAME rebased origin the CPU UVs
                // use — handed to the renderer once it is locked, below.
                let uv_org = self.factory.uv_rebase_origin();
                for (idx, verts) in self.factory.feature_textured_meshes() {
                    needed_tex.insert(idx);
                    let translucent = self.factory.textures.get(idx)
                        .map_or(false, |t| t.opacity < crate::factory::ALPHA_OPAQUE);
                    if translucent {
                        tex_feat_transp_owned.push((idx, verts));
                    } else {
                        tex_feat_owned.push((idx, verts));
                    }
                }
                for (_idx, verts) in &mut tex_feat_transp_owned {
                    let depth = |t: &[crate::light3d::TexVtx]| {
                        let c = [(t[0].x + t[1].x + t[2].x) / 3.0, (t[0].y + t[1].y + t[2].y) / 3.0, (t[0].z + t[1].z + t[2].z) / 3.0];
                        let d = [c[0] - cam_pos[0], c[1] - cam_pos[1], c[2] - cam_pos[2]];
                        d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
                    };
                    let mut tris: Vec<[crate::light3d::TexVtx; 3]> =
                        verts.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                    tris.sort_by(|a, b| depth(b).total_cmp(&depth(a)));
                    *verts = tris.into_iter().flatten().collect();
                }
                // Pixels for every texture referenced this frame (Arc → cheap clone; the
                // renderer uploads each to GL once and caches by index).
                let mut tex_assets_owned: Vec<(usize, StdArc<Vec<u8>>, i32, i32)> = Vec::new();
                let mut tex_reflect_owned: Vec<(usize, f32)> = Vec::new();
                let mut tex_proc_owned: Vec<(usize, crate::light3d::ProcParams)> = Vec::new();
                let mut tex_pbr_owned: Vec<(usize, crate::light3d::PbrParams)> = Vec::new();
                // Every PBR map referenced by a needed texture must ALSO upload — all FOUR of them.
                // Normal and roughness alone was enough while maps only ever came from a texture
                // set the user assembled by hand; a glTF import binds metallic and occlusion too,
                // and a map whose image never reaches the GPU is a sampler reading black.
                let pbr_maps: Vec<usize> = needed_tex
                    .iter()
                    .filter_map(|&i| self.factory.textures.get(i))
                    .flat_map(|t| {
                        t.normal_map.into_iter().chain(t.rough_map).chain(t.metal_map).chain(t.ao_map)
                    })
                    .collect();
                for m in pbr_maps {
                    needed_tex.insert(m);
                }
                // SORTED, and this is load-bearing. `HashSet` seeds a fresh random state per
                // instance and this one is rebuilt every frame, so iterating it directly yields a
                // different order each time. Everything built in this loop is folded IN ORDER
                // into the renderer's "did anything change?" key, so an unstable order made
                // identical content hash differently every frame and restarted the temporal
                // accumulation forever — the screen-space passes never averaged, and a smooth
                // material rendered as speckled noise that moved with the camera.
                let mut needed_tex: Vec<usize> = needed_tex.into_iter().collect();
                needed_tex.sort_unstable();
                for &idx in &needed_tex {
                    if let Some(t) = self.factory.textures.get(idx) {
                        let rgba = self
                            .texture_rgba
                            .entry(idx)
                            .or_insert_with(|| StdArc::new(t.rgba.clone()))
                            .clone();
                        tex_assets_owned.push((idx, rgba, t.w as i32, t.h as i32));
                        // Every texture, not just the ones above some threshold. `reflect` is now
                        // "how much of the physical reflection to keep", 1.0 by default, so a
                        // `> 0.0` filter would have silently dropped nothing — but a material
                        // turned DOWN to matte has to reach the renderer just as much as one
                        // turned up, and the old gate would have skipped exactly those.
                        tex_reflect_owned.push((idx, t.reflect));
                        // A procedural texture carries its shader params instead of an image.
                        if let Some(def) = &t.proc {
                            tex_proc_owned.push((idx, def.params()));
                        }
                        // Tangent-space normal / roughness maps (Texture Phase 2).
                        if t.has_pbr() {
                            tex_pbr_owned.push((idx, t.pbr_params()));
                        }
                    }
                }
                let rebuilt = match &self.factory_perf_prev {
                    Some(p) => !StdArc::ptr_eq(p, &verts),
                    None => true,
                };
                self.factory_perf_prev = Some(verts.clone());
                // Bump the GPU scene version only when the buffer actually changed, so the
                // renderer re-uploads it once (not every frame).
                if rebuilt { self.factory_scene_ver = self.factory_scene_ver.wrapping_add(1); }

                if self.dbg.recording {
                    // ABOVE vsync, not at it. 16.7 ms IS 60 Hz, so a threshold there flags every
                    // single frame on a vsync-locked display and the warning stops meaning
                    // anything — which is exactly what it did: whole sessions of ⚠ SLOW describing
                    // a renderer that was keeping perfect time. A frame only cost something when it
                    // MISSED a refresh, and a missed refresh lands near 33 ms; 20 ms leaves room
                    // for ordinary jitter while still catching the first real stumble.
                    const SLOW_FRAME_US: u64 = 20_000;
                    let slow_frame = frame_us >= SLOW_FRAME_US;
                    let throttle_ok = self
                        .factory_perf_last_slow
                        .map(|t| now.saturating_duration_since(t).as_millis() >= 300)
                        .unwrap_or(true);
                    if rebuilt || (slow_frame && throttle_ok) {
                        if slow_frame && !rebuilt {
                            self.factory_perf_last_slow = Some(now);
                        }
                        let n = verts.len();
                        let sz = std::mem::size_of::<crate::light3d::V3>();
                        // Any program the driver refused rides along in the phase string. A failed
                        // shader takes its feature out of the picture silently — the user sees
                        // "part of the render is wrong" and has no stderr to look at, so the one
                        // report they DO know how to produce has to carry it.
                        // …and so does the frame's GEOMETRY: the viewport, the buffers it drew
                        // into and their sizes. A viewport that disagrees with its framebuffer, or
                        // an accumulation buffer left at a stale size, both surface as "part of the
                        // picture is wrong" with nothing to point at from a screenshot.
                        let (bad, geom) = self
                            .light3d_renderer
                            .lock()
                            .ok()
                            .map(|r| {
                                // …AND WHY THE REFINEMENT RESTARTED, when it did. `n=1/16` on
                                // every frame is either an orbit in progress or a frame key that
                                // will never settle — the same line in a report, very different
                                // problems.
                                let why = r.taa_reason();
                                let geom = if why.is_empty() {
                                    r.last_geom().to_string()
                                } else {
                                    format!("{} taa-reset[{why}]", r.last_geom())
                                };
                                (r.shader_failures().join(","), geom)
                            })
                            .unwrap_or_default();
                        let phase = match (rebuilt, bad.is_empty()) {
                            // The interval rides along, and says whether anything is DRIVING the
                            // app: near 200 ms is the idle heartbeat, not a stall.
                            (_, false) => format!("SHADERS-FAILED[{bad}] {geom}"),
                            (true, _) => format!("buffer-rebuilt {geom}"),
                            _ => format!(
                                "slow-frame {geom} ({:.0} ms since last paint{}) [{}]",
                                interval_us as f64 / 1000.0,
                                if interval_us > 150_000 { ", idle heartbeat" } else { "" },
                                self.frame_marks_summary(),
                            ),
                        };
                        crate::dbg_event!(
                            self,
                            crate::dbg_recorder::DbgEvent::FactoryPerf {
                                phase,
                                frame_us: if rebuilt { 0 } else { frame_us },
                                build_us,
                                scene_tris: n / 3,
                                furniture_insts: self.factory.furniture.len(),
                                heaviest_tris: self.factory.heaviest_furniture_tris(),
                                upload_bytes: n * sz,
                                cache_rebuilt: rebuilt,
                            }
                        );
                    }
                }
                // THE TWO EXPENSIVE BUILDERS RUN ONLY WHEN SOMETHING THEY READ CHANGED. Measured
                // at 15.7 ms and 12.2 MB per frame on a 265-object drawing before this; the
                // signature is `lines_sig`, and what is deliberately left out of it is documented
                // there.
                self.refresh_cached_lines();
                let mut lines = self.factory.overlay_lines();
                lines.extend_from_slice(&self.cached_sketch_lines); // 2D work, lifted onto its plane
                // LAST of the three, so the yellow face outline is drawn over the sketch geometry
                // sitting on it rather than under. It is the answer to "is this the right face?",
                // and an answer half-hidden behind the drawing is no answer.
                lines.extend(self.factory.picked_face_lines());
                lines.extend(self.factory.live_sketch_lines(&self.doc)); // active sketch, live
                // The 2D PLAN goes in with the depth-tested segments UNLESS x-ray is asked
                // for. `lines` are occluded by the opaque solids, which is what makes a plan
                // behind a wall stay behind it. The x-ray alternative — painting the same
                // lines on a foreground egui layer that always sits over the 3D texture — is
                // `paint_plan_underlay`, and on a full drawing it reads as texture on the near
                // wall, because every line on the FAR side of the building projects onto it.
                //
                // THE PLAN, NOT WHATEVER IS ON THE CANVAS. Reported as: "i drew a sketch on a wall
                // … see how the same drawing is getting drawn in the ground plane too."
                //
                // `factory_enter_sketch` REPLACES `self.doc` with the sketch and parks the real
                // drawing in `session.saved_doc`, so while a face sketch is open `self.doc` is the
                // sketch. Read here, its u/v coordinates were laid out as world x/y at ground
                // level: the circle drawn on the wall appeared a second time lying on the floor.
                // The line above draws the sketch on its own plane, which is the one that is right.
                //
                // `plan_doc()` exists for exactly this and has to be used at every site that means
                // THE PLAN — the swap makes the wrong document the default.
                // Cached above, with the show_plan / x-ray / empty tests applied there — an empty
                // buffer IS the "do not draw the plan" answer, rather than a second copy of the
                // condition that could drift out of step with the one the cache used.
                //
                // AND CULLED TO WHAT THE CAMERA CAN SEE. Copying the whole plan every frame is
                // 26.7 MB and 6.2 ms at 50k dobjects, 106.8 MB and 24.8 ms at 200k — past the
                // entire frame budget for a drawing that is mostly off screen. `None` bounds mean
                // the view reaches the horizon, and then everything is copied, which is correct
                // and merely slow.
                let plan_z = self.factory.active_base_z();
                let view = crate::factory::FactoryState::ground_view_bounds(rect, &mvp, plan_z);
                let culled = self.emit_plan_lines(&mut lines, view);
                let _ = culled;

                // ---- §0.6 PREVIEW — shade / ghost / marching-ants / blip -----
                // The complete move, per BASIC_MODIFIERS_RULES §0.6 and §1:
                //   select → the picked solids SHADE → click BASE → a live GHOST and
                //   an animated path follow the constrained cursor → click DEST.
                // Everything below is redrawn each frame at the CONSTRAINED cursor
                // (osnap → CARD → raw), so what you see is exactly where it lands.
                self.factory.sync_selection_mesh();
                let mut overlay = self.factory.shade_verts();
                // ---- PLACED LUMINAIRES, AT THEIR REAL SIZE --------------------
                //
                // "i also need to see it in the 3d factory." Fittings were only ever drawn in the
                // SIMLUX view, so a layout laid out in SIMLUX was invisible in the window where the
                // building is modelled — and the two views are meant to be the same scene.
                //
                // On the OVERLAY list rather than in `opaque_verts`: that one is cached behind a
                // signature for render performance (a heavy model must not be rebuilt because a
                // light moved), and a box is 36 vertices, so rebuilding these every frame costs
                // nothing. Same bodies, same fallback icon, one implementation shared with SIMLUX.
                {
                    let s = (self.factory.cam_dist * 0.02).clamp(0.05, 0.3);
                    for l in &self.light.luminaires {
                        match self
                            .light
                            .profiles
                            .get(&l.profile)
                            .and_then(|p| p.housing_shape().zip(p.housing()))
                        {
                            Some((shape, (_, _, h))) => crate::light3d::push_luminaire_body(
                                &mut overlay,
                                [l.position.x, l.position.y, l.position.z],
                                shape,
                                h as f32,
                                l.rotation_deg,
                            ),
                            None => crate::light3d::push_luminaire_marker(
                                &mut overlay, l.position.x, l.position.y, l.position.z, s,
                            ),
                        }
                    }
                }
                // (Furniture is drawn full-form via the GPU model-matrix pass — see `furn_draws`.)
                // ---- §0.6 preview: the ghost + the path -----------------------
                // "while moving it shows the path" — redrawn each frame at the
                // CONSTRAINED cursor, so what you see is where the click lands.
                let mut ants: Option<(egui::Pos2, egui::Pos2, egui::Color32)> = None;
                if let Some(md) = &self.factory.modify {
                    let cw = resp.hover_pos().and_then(|hp| {
                        self.factory
                            .snap_vertex(hp, rect, &mvp)
                            .map(|(w, _)| w)
                            .or_else(|| self.factory.cursor_on_plane(hp, rect, &mvp))
                    });
                    if let (Some(cw), Some(base)) =
                        (cw, md.anchor_world(&cad_solid::Plane::default()))
                    {
                        overlay.extend(self.factory.modify_ghost(cw, self.env.CrdEnb));
                        let m = glam::Mat4::from_cols_array(&mvp);
                        let to_s = |w: glam::Vec3| {
                            let n = m.project_point3(w);
                            egui::pos2(
                                rect.left() + (n.x * 0.5 + 0.5) * rect.width(),
                                rect.top() + (0.5 - n.y * 0.5) * rect.height(),
                            )
                        };
                        use cad_solid::modify::ModifyOp as MO;
                        let accent = match md.op {
                            MO::Move => egui::Color32::from_rgb(255, 200, 100),
                            MO::Copy => egui::Color32::from_rgb(150, 230, 170),
                            MO::Mirror => egui::Color32::from_rgb(200, 160, 255),
                            _ => egui::Color32::from_rgb(235, 235, 245),
                        };
                        ants = Some((to_s(base), to_s(cw), accent));
                    }
                }
                // The ground counts as a scene: with the GND badge on, an empty model still has
                // something to look at, so the placeholder must not replace the render.
                if verts.is_empty() && lines.is_empty() && ground.is_empty()
                    && furn_draws.is_empty() && transp_draws_owned.is_empty()
                    && tex_transp_draws_owned.is_empty() && tex_draws_owned.is_empty()
                    && tex_feat_owned.is_empty() && tex_feat_transp_owned.is_empty()
                {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Add a Box or Cylinder — then right-click a face to draw on it",
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_gray(150),
                    );
                } else {
                    let renderer = self.light3d_renderer.clone();
                    // Copied out of `self` so the `move` paint closure can carry them.
                    let scene_ver = self.factory_scene_ver;
                    let taa_samples = self.factory.taa_samples;
                    // Keep asking for frames while the picture is still being refined. Read from
                    // LAST frame's renderer state — the paint callback runs after this — which is
                    // harmless: at worst it costs one extra repaint at the end of a convergence.
                    if renderer.lock().map(|r| r.taa_converging()).unwrap_or(false) {
                        ctx.request_repaint();
                    }
                    painter.add(egui::Shape::Callback(egui::PaintCallback {
                        rect,
                        callback: StdArc::new(egui_glow::CallbackFn::new(move |info, gp| {
                            let gl = gp.gl();
                            let vp = info.viewport_in_pixels();
                            let s = info.screen_size_px;
                            // Furniture draws as (key, &local_mesh, camera·model, world model,
                            // in_view) slices. Off-screen instances stay in the list — only the
                            // camera pass may skip them.
                            let furn: Vec<(u64, &[crate::light3d::V3], [f32; 16], [f32; 16], bool)> =
                                furn_draws.iter().map(|(k, m, mv, md, v)| (*k, m.as_slice(), *mv, *md, *v)).collect();
                            // Translucent furniture (already sorted back-to-front); drop the depth.
                            let transp: Vec<(u64, &[crate::light3d::V3A], [f32; 16], [f32; 16])> =
                                transp_draws_owned.iter()
                                    .map(|(_d, k, m, mv, md)| (*k, m.as_slice(), *mv, *md)).collect();
                            // Textured furniture + the pixels its images need.
                            let tex_assets: Vec<(usize, i32, i32, &[u8])> = tex_assets_owned
                                .iter()
                                .map(|(i, r, w, h)| (*i, *w, *h, r.as_slice()))
                                .collect();
                            let tex_draws: Vec<(usize, u64, &[crate::light3d::TexVtx], [f32; 16], [f32; 16])> =
                                tex_draws_owned
                                    .iter()
                                    .map(|(ti, k, m, mv, md)| (*ti, *k, m.as_slice(), *mv, *md))
                                    .collect();
                            // Translucent textured furniture (already sorted back-to-front).
                            let tex_transp: Vec<(usize, u64, &[crate::light3d::TexVtx], [f32; 16], [f32; 16])> =
                                tex_transp_draws_owned
                                    .iter()
                                    .map(|(_d, ti, k, m, mv, md)| (*ti, *k, m.as_slice(), *mv, *md))
                                    .collect();
                            let tex_feat: Vec<(usize, &[crate::light3d::TexVtx])> =
                                tex_feat_owned.iter().map(|(ti, m)| (*ti, m.as_slice())).collect();
                            let tex_feat_transp: Vec<(usize, &[crate::light3d::TexVtx])> =
                                tex_feat_transp_owned.iter().map(|(ti, m)| (*ti, m.as_slice())).collect();
                            if let Ok(mut r) = renderer.lock() {
                                // The HDR environment. The version check inside makes this a no-op
                                // on every frame but the one where the environment actually changed.
                                r.set_environment(
                                    gl,
                                    env_source.as_deref().map(|src| crate::light3d::EnvUpload {
                                        chain: &env_chain,
                                        source: src,
                                        version: env_version,
                                    }),
                                );
                                r.set_taa(gl, taa_samples);
                                r.uv_origin = uv_org;
                                r.render(
                                    // The 3D Factory already keeps its whole scene in the versioned
                                    // buffer, so it has no per-frame opaque geometry to add.
                                    gl, &verts, &overlay, &lines, &mvp, Some(scene_ver), 0, &ground,
                                    &furn, &transp, &tex_assets, &tex_draws, &tex_transp, &tex_feat,
                                    &tex_feat_transp, cam_pos, &tex_reflect_owned, &tex_proc_owned,
                                    &tex_pbr_owned, sun_uniform, &shadow_mvp, clay, mf_highlight, color_pipeline, env_render, true,
                                    vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px,
                                    s[0] as i32, s[1] as i32,
                                );
                            }
                        })),
                    }));
                }
                // marching-ants path + base blip — the SAME helpers the 2D move
                // uses (phase = time*60), so the 3D path animates identically.
                if let Some((base_s, dest_s, accent)) = ants {
                    draw_base_blip(&painter, base_s, accent);
                    let phase = ctx.input(|i| i.time) as f32 * 60.0;
                    draw_dashed_line(&painter, base_s, dest_s, 6.0, 4.0, phase,
                        egui::Stroke::new(1.2, accent));
                    ctx.request_repaint();
                }
                if !self.factory.status.is_empty() {
                    painter.text(
                        rect.left_top() + egui::vec2(10.0, 10.0),
                        egui::Align2::LEFT_TOP,
                        &self.factory.status,
                        egui::FontId::monospace(12.0),
                        egui::Color32::from_rgb(255, 200, 100),
                    );
                }

                // ---- ACTIVE-VIEWPORT frame ----------------------------------
                // yellow = this view owns the next command · gray = idle
                draw_viewport_active_frame(&painter, rect, self.active_view == ActiveView::ThreeD);

                // ---- CURSOR — the SAME glyphs as the 2D canvas ---------------
                // Reused via `draw_select_cursor` / `draw_draft_cursor`, so the two
                // views can never drift. Which one is showing tells you what the LEFT
                // button will do, exactly as in 2D:
                //   orbiting            → nothing (the OS arrow; you're moving the view)
                //   drafting on a plane → square + cross (a click places a POINT)
                //   otherwise           → "^" pickbox   (a click selects an OBJECT)
                if !orbiting {
                    if let Some(p) = resp.hover_pos() {
                        // a running 3D op is a POINT-PICK phase → drafting glyph,
                        // exactly as `in_click_only_phase` gates it in 2D
                        if self.factory.session.is_some() || self.factory.modify.is_some() {
                            draw_draft_cursor(&painter, p);
                        } else {
                            draw_select_cursor(&painter, p);
                        }
                        ui.ctx().set_cursor_icon(egui::CursorIcon::None); // ours replaces it
                    }
                }
                if resp.dragged()
                    || (resp.hovered() && ui.input(|i| i.smooth_scroll_delta.y != 0.0))
                {
                    ui.ctx().request_repaint();
                }
                // applied outside the context-menu closure (it can't borrow self mutably)
                if let Some(f) = want_face_on {
                    // `pick_face` anchors the frame at the point the ray hit, so this centres on
                    // the spot you right-clicked rather than on the face's centroid — which is what
                    // you want on a long wall, where the centroid may be off screen entirely.
                    self.factory.look_at_frame(&f, f.origin);
                    let n = f.normal();
                    self.factory.status = format!(
                        "looking square-on at the face ({:.2}, {:.2}, {:.2}) — parallel projection",
                        n.x, n.y, n.z
                    );
                    ui.ctx().request_repaint();
                }
                if let Some(f) = want_sketch {
                    self.factory_enter_sketch(f);
                }
                if want_finish {
                    // In a sweep flow, "Finish" advances the flow (capture section / build);
                    // otherwise it just closes the sketch.
                    if self.factory.sweep_flow.is_some() {
                        self.factory_finish_sweep_stage();
                    } else {
                        self.factory_exit_sketch();
                    }
                }
                if let Some(i) = want_choose_view {
                    if i == usize::MAX {
                        self.factory.sweep_flow = None;
                        self.factory.status = "path sweep cancelled".into();
                    } else {
                        self.factory_choose_sweep_view(i);
                    }
                }
            });
        if !open && self.factory.open {
            self.active_view = ActiveView::TwoD; // the 3D view is gone → 2D is active
        }
        self.factory.open = open;
    }

    /// SWITCH THE WHOLE WINDOW TO ONE WORKSPACE (the mode tab bar).
    ///
    /// Exactly one of the three views is open at a time — see [`Mode`]. This is
    /// the single place that rearranges the view flags; the tab bar calls it on
    /// a click, and the frame-start hooks in `update()` call it when a view is
    /// opened from inside another workspace (so an import that opens the 3D
    /// Factory lands the user on the 3D Factory tab).
    ///
    /// `user_intent` distinguishes a tab click from an automatic switch: an
    /// automatic return-to-Factory pending after a face-sketch is cancelled by
    /// a tab click (the user who changed tabs wants to stay where they are
    /// when the sketch ends), never by an automatic switch.
    fn switch_mode_inner(&mut self, m: Mode, user_intent: bool) {
        if self.mode == m && self.mode_view_open() {
            return;
        }
        if user_intent {
            self.factory_return_after_sketch = false;
        }
        self.mode = m;
        match m {
            Mode::Cad2D => {
                self.two_d_open = true;
                self.factory.open = false;
                self.light.view3d_open = false;
                self.light.simlux_mode = false;
                self.active_view = ActiveView::TwoD;
            }
            Mode::Simlux => {
                self.two_d_open = false;
                self.factory.open = false;
                self.light.view3d_open = true;
                self.light.simlux_mode = false;
                self.active_view = ActiveView::ThreeD;
                // Maximise the viewport on the frame the workspace is entered;
                // egui remembers a panel's width per id, so without the pin a
                // maximised workspace would reopen at whatever width a past
                // drag left it at.
                self.simlux_full_pin = true;
            }
            Mode::Factory => {
                self.two_d_open = false;
                self.factory.open = true;
                self.light.view3d_open = false;
                self.light.simlux_mode = false;
                self.active_view = ActiveView::ThreeD;
                self.factory_full_pin = true;
                if self.factory.dirty {
                    self.factory.recompute();
                }
            }
        }
    }

    /// Tab-bar click: switch workspaces and cancel any pending automatic
    /// return (see [`Self::switch_mode_inner`]).
    pub(super) fn switch_mode(&mut self, m: Mode) {
        self.switch_mode_inner(m, true);
    }

    /// Whether the current workspace's own view is open — the invariant the
    /// exclusive tabs keep (`mode_view_open()` is true after every frame).
    pub(super) fn mode_view_open(&self) -> bool {
        match self.mode {
            Mode::Cad2D => self.two_d_open,
            Mode::Simlux => self.light.view3d_open || self.light.simlux_mode,
            Mode::Factory => self.factory.open,
        }
    }

    /// Never leave the user with no view at all.
    ///
    /// The three views can each be closed — "they can close any of them to have any of the single
    /// window open" — but closing the LAST one leaves a window with a menu bar and nothing under
    /// it, and no obvious way back except the menu they just used. Whichever was closed last is
    /// re-opened, so the answer to "where did everything go" is that it never went.
    pub(super) fn keep_one_view_open(&mut self) {
        if !self.two_d_open
            && !self.factory.open
            && !self.light.view3d_open
            && !self.light.simlux_mode
        {
            self.two_d_open = true;
        }
    }

    /// ENFORCE THE MODE TAB BAR — one full-window workspace at a time.
    ///
    /// Called at frame start, before any panel draws (see `update()`). `mode`
    /// (set by the tab bar) is the single source of truth; three things can
    /// break the invariant during a frame, and all three are repaired here:
    ///
    ///   1. A view opens from inside another workspace (an import needs the
    ///      3D Factory, the Materials Factory opens its live preview, SIMLUX
    ///      machinery opens its 3D view) — switch to that view's tab, so the
    ///      window shows what the code just asked for.
    ///   2. The workspace's own view is closed from inside (its ✕) — fall
    ///      back to the 2D drafting workspace.
    ///   3. A face-sketch opens: it drafts ON the 2D canvas, which is hidden
    ///      in the Factory workspace, so the sketch takes over the whole
    ///      window; when it ends, the app returns to the workspace it was
    ///      started from (unless the user switched tabs in between).
    ///
    /// The sketch hooks run FIRST: taking the window over for drafting
    /// changes the view flags, and the two view-rising hooks below must see
    /// the flags the sketch hook left.
    pub(super) fn enforce_mode_workspaces(&mut self) {
        // 3: sketch transitions. A sketch's document IS `self.doc` while it is
        // open, and it is drawn on the 2D canvas — so an open sketch always
        // means the drafting workspace owns the window.
        let in_sketch = self.factory.session.is_some();
        if in_sketch && !self.factory_was_in_sketch {
            if !self.two_d_open {
                // The canvas is hidden → the sketch claims the window. Only the
                // Factory workspace starts sketches today (its panel is where
                // every "draw on this face" lives), but any mode with the canvas
                // hidden would behave the same way.
                self.factory_return_after_sketch = self.mode == Mode::Factory;
                self.switch_mode_inner(Mode::Cad2D, false);
            }
        } else if !in_sketch && self.factory_was_in_sketch {
            if self.factory_return_after_sketch {
                self.factory_return_after_sketch = false;
                self.switch_mode_inner(Mode::Factory, false);
            } else if self.mode == Mode::Cad2D {
                // A view that opened WHILE the sketch was being drafted (e.g.
                // the Materials Factory opened its live 3D preview mid-sketch)
                // is put away again: the drafting workspace shows only the
                // canvas, and the sketch is what ended.
                self.factory.open = false;
                self.light.view3d_open = false;
                self.light.simlux_mode = false;
            }
        }
        self.factory_was_in_sketch = in_sketch;

        // The 3D Factory just opened (rising edge on LAST frame's value — the
        // sketch hook above may have closed it this very frame, and that close
        // is not an open).
        let factory_rising = self.factory.open && !self.factory_was_open;
        self.factory_was_open = self.factory.open;
        if factory_rising {
            self.on_factory_opened();
        }

        // 1: a view that was asked for from inside another workspace brings its
        // tab forward. Gated on `!in_sketch`: while a sketch is being drafted
        // the canvas owns the window, and nothing should yank it away mid-draft
        // — the requested view opens beside the canvas instead, and is put away
        // again when the sketch ends (above).
        let simlux_open = self.light.view3d_open || self.light.simlux_mode;
        if !in_sketch && simlux_open && !self.simlux_was_open && self.mode != Mode::Simlux {
            self.switch_mode_inner(Mode::Simlux, false);
        }
        if !in_sketch && factory_rising && self.mode != Mode::Factory {
            self.switch_mode_inner(Mode::Factory, false);
        }

        // 2: the workspace's own view was closed (its ✕) → back to 2D drafting.
        if !self.mode_view_open() {
            if self.mode == Mode::Cad2D {
                self.two_d_open = true;
            } else {
                self.switch_mode_inner(Mode::Cad2D, true);
            }
        }
        self.simlux_was_open = simlux_open;
    }

    /// Help ▸ Keyboard shortcuts — the whole table, grouped by where each key applies.
    ///
    /// Generated from [`SHORTCUTS`] rather than written out here, so a key that moves in the code
    /// cannot leave a stale line on a help page. Each group states its SCOPE, because the common
    /// way for a shortcut to look broken is being pressed in the wrong view.
    pub(super) fn render_shortcuts_window(&mut self, ctx: &egui::Context) {
        if !self.shortcuts_open {
            return;
        }
        let mut open = true;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .resizable(true)
            .default_width(560.0)
            .default_height(600.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (i, g) in SHORTCUTS.iter().enumerate() {
                        if i > 0 {
                            ui.add_space(10.0);
                        }
                        ui.heading(g.title);
                        ui.label(egui::RichText::new(g.scope).small().weak());
                        ui.add_space(4.0);
                        egui::Grid::new(("shortcut_grid", i))
                            .num_columns(2)
                            .spacing([18.0, 4.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for row in g.rows {
                                    ui.label(
                                        egui::RichText::new(row.keys)
                                            .monospace()
                                            .strong()
                                            .color(crate::theme::color::ACCENT),
                                    );
                                    ui.label(row.what);
                                    ui.end_row();
                                }
                            });
                    }
                });
            });
        self.shortcuts_open = open;
    }

    pub(super) fn render_light_3d_panel(&mut self, ctx: &egui::Context) {
        // The strip says how many fittings there are, and the model's own lights have to be in
        // that count — otherwise a room lit entirely by curved lights reports "0 fixture(s)".
        self.light.refresh_model_fixtures(&self.factory);
        // SIMLUX workspace mode force-shows this as the right HALF of the window
        // and keeps it in sync with the 2D drawing; else it's the toggled panel.
        let split = self.light.simlux_mode;
        if !self.light.view3d_open && !split {
            return;
        }
        if split {
            // Live: re-extrude the current room so the 3D tracks the 2D plan. The PLAN — with a
            // face-sketch open `self.doc` is the sketch, and the room would collapse to its
            // handful of construction lines every frame the sketch was live.
            // ONLY WHEN SOMETHING IT READS HAS MOVED — and this was the lag, all of it.
            //
            // This ran unconditionally, every frame the workspace was open. `rebuild_live_meshes_with`
            // is `scene_meshes` → `meshes_from_factory_mode(.., Thorough)`, which transforms EVERY
            // furniture triangle into a fresh buffer: 7,036,129 triangles and 21.1 M vertices on the
            // reference gym plan, about 253 MB, sixty times a second. Measured at ~205 ms of a
            // ~210 ms frame. The `plan_doc().clone()` above it copied all 1,442 drawing objects
            // again for good measure.
            //
            // It is SIMLUX-only because nothing else enters split mode, which is exactly what the
            // user said from the first report and what four rounds of work in the 3D renderer
            // failed to hear. The display buffer was cached three builds ago; this is the same
            // mistake one level upstream, still regenerating the geometry that cache exists to
            // avoid touching.
            let sig = self.light.live_mesh_sig_of(Some(&self.factory));
            // `None` means the scene comes from the 2D document, which cannot be summarised
            // cheaply — that path is the cheap one and keeps rebuilding, as before.
            if sig.is_none() || self.light.live_mesh_sig != sig {
                let plan = self.plan_doc().clone();
                // …and hand it the 3D MODEL, which is the real building. The extrusion behind this
                // is only a footprint pulled to one height: no openings, no slabs, no storeys. It
                // stays as the fallback for a plan-only project.
                self.light
                    .rebuild_live_meshes_with(&plan, Some(&self.factory));
                self.light.live_mesh_sig = sig;
            }
        }
        let half = ctx.screen_rect().width() * 0.5;
        let mut open = self.light.view3d_open;
        let mut leave_workspace = false;
        // Opening the picker mutates `self.file_dialog`, and the panel closure already holds
        // `self` — so the request is carried out after the panel is drawn.
        let mut open_ies_picker = false;
        // RESIZABLE IN BOTH MODES.
        //
        // Reported as: "the window of simlux isnt adjustable like we can do for 3d factory or 2d
        // cad." The workspace ("live") mode used `exact_width(half)`, which is not a default but a
        // LOCK — the panel had no drag handle at all and sat on exactly 50% of the screen whatever
        // you were doing. Half is a sensible place to START; it is not a sensible place to be
        // stuck, since the whole point of the split is to work in BOTH halves.
        //
        // …but `default_width` is NOT a substitute for it, and swapping one for the other broke
        // entering the workspace. egui stores a panel's width per id and `default_width` applies
        // only the FIRST time that id is ever shown — so on any installation that had already
        // opened the SIMLUX panel, entering the split reused whatever width was remembered from
        // the 360-wide toggled panel instead of taking half the window. The split then opened
        // wrong, and the 3D Factory panel beside it took the space, which is how "the simlux
        // window is completely broken" looks.
        //
        // So force the width on the frame the workspace is ENTERED — which is what half-the-window
        // always meant, a starting position — and leave it freely resizable every frame after.
        // LEAVE ROOM FOR THE PANELS THAT COME AFTER THIS ONE.
        //
        // Reported twice: "the simlux window ... only extends to a length and when extend it
        // beyond that it goes behind the 3d factory window."
        //
        // `screen - 260` reserved 260 px for everything else — but "everything else" is the 3D
        // Factory panel AND the 2D canvas, and the Factory alone has a min_width of 240. This
        // panel is laid out FIRST, so once it takes more than the window can spare the Factory has
        // nowhere to go: it ends up starting at x = 0, overlapping this one, and because it is
        // drawn second it paints on top — which is what "goes behind" looks like from here.
        //
        // Measured, before and after, in `panel_geometry_measurement`: on a 2763 px window, asking
        // for 2400 used to put the Factory at 0..449 against a SIMLUX panel starting at 363 — an
        // 86 px overlap, with the 2D canvas squeezed to zero width.
        //
        // Only for the views that are actually OPEN, though. "make sure the windows of 2d cad 3d
        // factory and simlux can be extended to which ever length the user prefers and they can
        // close any of them to have any of the single window open" — so a closed view reserves
        // nothing, and with the other two closed this panel can take the entire window.
        //
        // The Factory's width comes from egui's own panel state rather than an assumption, because
        // that one is draggable too.
        let factory_w = if self.factory.open {
            egui::containers::panel::PanelState::load(ctx, egui::Id::new("factory_3d_panel"))
                .map_or(420.0, |s| s.rect.width())
        } else {
            0.0
        };
        let keep_2d = if self.two_d_open { MIN_VIEW_W } else { 0.0 };
        // The limit is measured against what the window ACTUALLY has left for
        // this panel — NOT `screen_rect()`. The left workspace panels (the
        // mode command panel) reserve width before this viewport renders, and
        // a pin computed from the raw screen width would overlap them (the
        // maximised panel used to start at x = 0, on top of the command list,
        // for the first frame of every SIMLUX workspace visit).
        let avail_w = ctx.available_rect().width();
        let max = (avail_w - factory_w - keep_2d).max(MIN_VIEW_W);
        let entering_split = split && !self.simlux_split_prev;
        self.simlux_split_prev = split;
        // MODE ENTRY PIN: the SIMLUX workspace was just entered (tab bar) →
        // pin the panel to the whole window for this one frame, the same way
        // entering the split pins it to half. See `simlux_full_pin`.
        let full_pin = self.simlux_full_pin;
        self.simlux_full_pin = false;
        let base = egui::SidePanel::right("simlux_3d_panel")
            .resizable(true)
            .min_width(220.0)
            // Never let it swallow the window: the half it is paired with has to stay usable, and a
            // panel dragged past the far edge cannot be dragged back.
            .max_width(max);
        let base = if entering_split {
            // One frame of exact_width pins the STORED width to half; from the next frame on the
            // drag handle is back and it stays wherever the user puts it.
            base.exact_width(half)
        } else if full_pin {
            base.exact_width(max)
        } else if split {
            base.default_width(half)
        } else {
            base.default_width(360.0)
        };
        base
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(if split { "SIMLUX — 3D  (live)" } else { "SIMLUX — 3D" });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let tip = if split { "Exit SIMLUX workspace" } else { "Close 3D view" };
                        if ui.button("✕").on_hover_text(tip).clicked() {
                            if split { leave_workspace = true; } else { open = false; }
                        }
                        ui.label(
                            egui::RichText::new("drag: orbit · shift/mid: pan · scroll: zoom")
                                .small()
                                .weak(),
                        );
                    });
                });
                ui.separator();

                // The SIMLUX toolbar, in the same grouped-menu shape as the 3D Factory's. It used
                // to be a bare colour legend here and a tall column of numbered steps in a side
                // panel, so "how do I add a light" had no answer anywhere on screen.
                // The toolbar edits the REPORT's scale — one set of settings for the window and
                // the page alike. Taken out and put back so both borrows are short.
                let room_max = self.light_room_max();
                let mut ropts = std::mem::take(&mut self.report_opts);
                let strays = self.stray_light_ids().len();
                let act = self.light.toolbar_ui(ui, &mut ropts, room_max, strays,
                    &|s| crate::calc::parse_drag(&self.calc, s));
                self.report_opts = ropts;
                if act.import_photometry {
                    open_ies_picker = true;
                }
                if act.calculate {
                    // THE PLAN. The live preview 100 lines above already reads `plan_doc()`; this
                    // reached for `self.doc` again, so the picture and the number printed under it
                    // were fed by two different documents.
                    self.start_calculation();
                }
                if act.export_report {
                    self.export_light_report();
                }
                if self.light.grid.is_some() {
                    // THE REPORT'S BANDS — the same scale the floor sheet is painted in.
                    crate::light::band_legend(
                        ui,
                        &self.report_opts,
                        self.light_room_max(),
                        self.light.ramp,
                    );
                }
                ui.separator();

                let size = ui.available_size();
                let (resp, painter) = ui.allocate_painter(size, egui::Sense::drag());
                let rect = resp.rect;

                // ---- orbit + PAN + zoom ------------------------------------
                //
                // Orbit and zoom alone cannot reach the corner of a large plan: the pivot stays put
                // and the room swings around it. Pan is on the same gestures the 3D Factory uses —
                // Shift + drag, or the middle button — so the two viewports do not need separate
                // muscle memory. Middle-drag ALSO pans here (rather than orbiting as it does in the
                // Factory) because left-drag already orbits in this view and nothing else is on it.
                let shift = ui.input(|i| i.modifiers.shift);
                let panning = resp.dragged()
                    && (shift || resp.dragged_by(egui::PointerButton::Middle));
                if panning {
                    let d = resp.drag_delta();
                    self.light.pan(d.x, d.y);
                } else if resp.dragged() {
                    let d = resp.drag_delta();
                    self.light.cam_yaw -= d.x * 0.01;
                    self.light.cam_pitch = (self.light.cam_pitch + d.y * 0.01).clamp(-1.45, 1.45);
                }
                if resp.hovered() {
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll.abs() > 0.0 {
                        self.light.cam_dist =
                            (self.light.cam_dist * (1.0 - scroll * 0.0015)).clamp(0.5, 500.0);
                    }
                }

                // ---- scene + camera ----------------------------------------
                let aspect = if rect.height() > 0.0 { rect.width() / rect.height() } else { 1.0 };
                let mvp = crate::light3d::mvp(
                    self.light.cam_yaw,
                    self.light.cam_pitch,
                    self.light.cam_dist,
                    self.light.cam_target,
                    aspect,
                    false, // SIMLUX room view keeps perspective
                );
                // THE ROOM, FROM THE CACHE — rebuilt only when the model, the calculation, the
                // materials or the ceiling toggle move. `Arc` so it can cross into the paint
                // callback without a copy; on a real project this is 844 MB that used to be
                // rebuilt every frame while the plan beside it was being edited.
                let scene_ver = self.scene3d_static_key();
                let verts = self.scene3d_static();
                // …and the parts that really do change every frame: the lux sheet, the fittings and
                // where they point. Bounded, and cheap enough to leave uncached — which also means
                // there is no key to get wrong when the scale is edited from the toolbar.
                let t_dyn = std::time::Instant::now();
                let dyn_verts = self.build_scene3d_dyn();
                let dyn_us = t_dyn.elapsed().as_micros() as u64;

                // ---- SIMLUX PERF TAP -------------------------------------------------------
                //
                // The 3D Factory has had one of these for a year; this view had none, so a SIMLUX
                // session recorded NOTHING and every frame number in such a dump described a view
                // that was not even running. It carries the 2D overlay's cost too, because that
                // lives in the CAD canvas but only ever runs when a lighting result exists — so it
                // is a SIMLUX cost that no 3D counter can see, and it is where the lag was.
                if self.dbg.recording {
                    let now = std::time::Instant::now();
                    // WORK, NOT INTERVAL — and the difference is the whole reason a session of an
                    // IDLE app got reported as running at 4.8 fps.
                    //
                    // This measured the wall time between paints. The app deliberately idles at a
                    // 5 Hz heartbeat (`request_repaint_after(200 ms)`), so an untouched frame reads
                    // 210–230 ms, every one of them cleared the 20 ms bar, and sixty consecutive
                    // ⚠ SLOW lines described a window that was asleep. `cpu_us` is the time spent
                    // in THIS frame's `update` so far, which is what SLOW is supposed to mean.
                    let cpu_us = self
                        .frame_start
                        .map(|t| now.saturating_duration_since(t).as_micros() as u64)
                        .unwrap_or(0);
                    let interval_us = self
                        .simlux_perf_last_frame
                        .map(|t| now.saturating_duration_since(t).as_micros() as u64)
                        .unwrap_or(0);
                    let frame_us = cpu_us;
                    self.simlux_perf_last_frame = Some(now);
                    let rebuilt = self.simlux_perf_key != Some(scene_ver);
                    self.simlux_perf_key = Some(scene_ver);
                    // The same threshold and throttle the factory tap uses: 16.7 ms IS 60 Hz, so a
                    // bar there flags every frame on a vsync-locked display and stops meaning
                    // anything. A missed refresh lands near 33 ms; 20 ms catches the first stumble.
                    const SLOW_FRAME_US: u64 = 20_000;
                    let slow = cpu_us >= SLOW_FRAME_US;
                    let throttle_ok = self
                        .simlux_perf_last_slow
                        .map(|t| now.saturating_duration_since(t).as_millis() >= 300)
                        .unwrap_or(true);
                    if rebuilt || (slow && throttle_ok) {
                        if slow && !rebuilt {
                            self.simlux_perf_last_slow = Some(now);
                        }
                        let sz = std::mem::size_of::<crate::light3d::V3>();
                        let what = if rebuilt { "simlux-rebuilt" } else { "simlux-slow-frame" };
                        crate::dbg_event!(
                            self,
                            crate::dbg_recorder::DbgEvent::FactoryPerf {
                                // `gl` IS THE ONE NUMBER THAT SPLITS THE FRAME.
                                //
                                // `frame_us` is the wall time BETWEEN SIMLUX paints — the whole
                                // app's frame, not this view's cost — so a 210 ms reading says
                                // nothing about where the 210 ms went, and I have twice guessed
                                // wrong from it. This is the GL draw alone, `glFinish` either side,
                                // so `frame - gl` is everything else: the 2D canvas, egui, the rest.
                                // It is LAST FRAME's value: the paint callback runs after this.
                                phase: format!(
                                    "{what} cpu {:.1} ms of {:.0} ms since last paint{} [{}] room={}v dyn={}v gl {:.1} ms 2d-overlay {:.1} ms / {} cells",
                                    cpu_us as f64 / 1000.0,
                                    interval_us as f64 / 1000.0,
                                    // The app idles at a 5 Hz heartbeat, so an interval near 200 ms
                                    // means NOBODY IS DRIVING IT — the frame is not slow, it is
                                    // asleep. Said on the line so the two are never confused again.
                                    if interval_us > 150_000 { " (idle heartbeat)" } else { "" },
                                    self.frame_marks_summary(),
                                    verts.len(),
                                    dyn_verts.len(),
                                    self.simlux_gl_us.load(std::sync::atomic::Ordering::Relaxed)
                                        as f64
                                        / 1000.0,
                                    self.lux_overlay_us.get() as f64 / 1000.0,
                                    self.lux_overlay_cells.get(),
                                ),
                                frame_us,
                                build_us: dyn_us,
                                scene_tris: (verts.len() + dyn_verts.len()) / 3,
                                furniture_insts: self.factory.furniture.len(),
                                heaviest_tris: self.factory.heaviest_furniture_tris(),
                                // The per-frame upload is the DYNAMIC half; the room is versioned
                                // and crosses the bus only when it is rebuilt.
                                upload_bytes: dyn_verts.len() * sz
                                    + if rebuilt { verts.len() * sz } else { 0 },
                                cache_rebuilt: rebuilt,
                            }
                        );
                    }
                }

                if verts.is_empty() && dyn_verts.is_empty() {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Draw a room, then press Calculate",
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_gray(150),
                    );
                } else {
                    let renderer = self.light3d_renderer.clone();
                    // TIME THE DRAW ITSELF, while the recorder is on. `glFinish` is a stall and
                    // would be indefensible every frame, but it is exactly what makes the number
                    // mean GPU time rather than "how far ahead the driver let us run".
                    let gl_us = self.simlux_gl_us.clone();
                    let time_gl = self.dbg.recording;
                    let cb = egui::PaintCallback {
                        rect,
                        callback: StdArc::new(egui_glow::CallbackFn::new(move |info, gp| {
                            let gl = gp.gl();
                            let vp = info.viewport_in_pixels();
                            let s = info.screen_size_px;
                            let t_gl = time_gl.then(|| {
                                crate::light3d::gl_finish(gl);
                                std::time::Instant::now()
                            });
                            if let Ok(mut r) = renderer.lock() {
                                // The lux view NEVER accumulates: it shares the renderer with the
                                // factory, and averaging jittered samples of a false-colour scale
                                // would blur the reading it exists to report.
                                r.set_taa(gl, 0);
                                r.render(
                                    gl, &verts, &[], &[], &mvp, Some(scene_ver), 1, &dyn_verts,
                                    &[], &[], &[], &[], &[], &[], &[], [0.0, 0.0, 0.0], &[], &[],
                                    &[], None, &[], false, None,
                                    // The lux heatmap's vertex colours ARE the false-colour scale —
                                    // re-grading them would misreport illuminance, so: no transform.
                                    crate::color::ColorPipeline::passthrough(),
                                    // …and no environment either: the SIMLUX view is a measurement,
                                    // not a picture. Sky light or occlusion would change what it says.
                                    crate::env::EnvRender::none(),
                                    false,
                                    vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px,
                                    s[0] as i32, s[1] as i32,
                                );
                            }
                            if let Some(t) = t_gl {
                                crate::light3d::gl_finish(gl);
                                gl_us.store(
                                    t.elapsed().as_micros() as u64,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            }
                        })),
                    };
                    painter.add(egui::Shape::Callback(cb));
                }
                if resp.dragged() || (resp.hovered() && ui.input(|i| i.smooth_scroll_delta.y != 0.0)) {
                    ui.ctx().request_repaint();
                }
            });
        if open_ies_picker {
            self.open_file_dialog(FileDialogMode::ImportIes, ".ies");
        }
        if leave_workspace {
            // ✕ in workspace mode exits the split entirely.
            self.light.simlux_mode = false;
            self.light.view3d_open = false;
        } else {
            self.light.view3d_open = open;
        }
    }
    // ===== end SIMLUX lighting ==========================================
}
