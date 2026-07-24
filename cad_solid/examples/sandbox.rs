//! SIMLUX 3D Solid Sandbox — standalone modeler for parametric CSG solids.
//!
//! `cargo run -p cad_solid --example sandbox`
//!
//! Isolated window (no simLUX app, no persistence): pick a construction plane, add
//! parametric primitives, boolean them via csgrs, and modify selected solids with
//! the SAME select-first modifiers as the 2D app — Move / Copy / Rotate / Scale /
//! Mirror, base-point + second-point, with a CARD cardinal-lock. A corner view
//! navigator snaps to standard views. UI palette mirrors the app's `theme.rs`.

use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::{Arc, Mutex};

use eframe::glow;
use eframe::glow::HasContext;
use glam::{Mat4, Vec2, Vec3};

use cad_kernel::{find_snap, DObject, SnapKind, SnapSet};
use cad_solid::dbg_recorder::{DbgEvent, DbgRecorder};
use cad_solid::draw::{ArcMethod, CircleMethod, CmdOutcome, Draw, DrawTool, EllipseMethod};
use cad_solid::modify::{rot_about, scale_about, Feed, Modify, ModifyOp};
use cad_solid::{BoolOp, Frame, Model, Placement, Plane, PlaneKind, Primitive, Sketch, SolidMesh};

/// An in-progress sketch-on-face session: which sketch + the live draw command.
struct SketchMode {
    idx: usize, // index into model.sketches
    draw: Draw,
}

// ─────────────────────────────────────────────────────────────────────────────
// Design tokens — copied from cad_app/src/theme.rs so the sandbox reads like the
// app (teal-navy surfaces, cyan accent). Fonts are the app's one extra; deferred.
// ─────────────────────────────────────────────────────────────────────────────
mod theme {
    use egui::{Color32, Rounding, Stroke, Visuals};
    pub const SURFACE_0: Color32 = Color32::from_rgb(0x14, 0x1c, 0x25);
    pub const SURFACE_1: Color32 = Color32::from_rgb(0x1a, 0x24, 0x30);
    pub const SURFACE_2: Color32 = Color32::from_rgb(0x22, 0x2b, 0x34);
    pub const SURFACE_3: Color32 = Color32::from_rgb(0x2a, 0x37, 0x44);
    pub const BORDER: Color32 = Color32::from_rgb(0x34, 0x41, 0x4b);
    pub const ACCENT: Color32 = Color32::from_rgb(0x00, 0xe5, 0xff);
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xda, 0xe3, 0xef);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x93, 0xa1, 0xac);
    pub const WARNING: Color32 = Color32::from_rgb(0xf2, 0xb5, 0x3d);
    pub const DANGER: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);

    pub fn apply(ctx: &egui::Context) {
        let mut v = Visuals::dark();
        v.window_fill = SURFACE_3;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.extreme_bg_color = SURFACE_0;
        v.faint_bg_color = SURFACE_2;
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = Color32::from_rgba_unmultiplied(0x00, 0xe5, 0xff, 60);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
        w.inactive.bg_fill = SURFACE_2;
        w.inactive.weak_bg_fill = SURFACE_2;
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
        w.inactive.rounding = Rounding::same(8.0);
        w.hovered.bg_fill = SURFACE_3;
        w.hovered.weak_bg_fill = SURFACE_3;
        w.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        w.hovered.rounding = Rounding::same(8.0);
        w.active.bg_fill = SURFACE_3;
        w.active.weak_bg_fill = SURFACE_3;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.rounding = Rounding::same(8.0);
        ctx.set_visuals(v);
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 780.0])
            .with_title("SIMLUX — 3D Solid Sandbox"),
        ..Default::default()
    };
    eframe::run_native("cad_solid_sandbox", options, Box::new(|_cc| Ok(Box::new(Sandbox::new()))))
}

/// Standard camera views for the corner navigator.
#[derive(Clone, Copy)]
enum ViewPreset {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
    Iso,
}

impl ViewPreset {
    fn angles(self) -> (f32, f32) {
        let up = FRAC_PI_2 - 0.02;
        match self {
            ViewPreset::Top => (0.0, up),
            ViewPreset::Bottom => (0.0, -up),
            ViewPreset::Front => (-FRAC_PI_2, 0.0),
            ViewPreset::Back => (FRAC_PI_2, 0.0),
            ViewPreset::Right => (0.0, 0.0),
            ViewPreset::Left => (PI, 0.0),
            ViewPreset::Iso => (0.9, 0.62),
        }
    }
}

/// A command that has been RUN but still needs a selection — the 2D app's
/// `QueuedOp`. Run the command → gather a selection → Enter → execute.
#[derive(Clone, Copy)]
enum Queued {
    Modify(ModifyOp),
    Erase,
}

impl Queued {
    fn label(self) -> String {
        match self {
            Queued::Modify(op) => op.label().to_lowercase(),
            Queued::Erase => "erase".to_string(),
        }
    }
}

// Session recorder is `cad_solid::dbg_recorder::DbgRecorder` — copied VERBATIM
// from the app (byte-identical to RUST_CAD). Events are pushed via `self.note`.

/// A 2D modifier in progress on the flat sketch (base → destination), applied to
/// the sketch's selected dobjects via the kernel's own `DObject::translated`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FlatOp {
    Move,
    Copy,
    Rotate,
    Scale,
    Mirror,
}

impl FlatOp {
    const ALL: [FlatOp; 5] = [FlatOp::Move, FlatOp::Copy, FlatOp::Rotate, FlatOp::Scale, FlatOp::Mirror];

    fn label(self) -> &'static str {
        match self {
            FlatOp::Move => "move",
            FlatOp::Copy => "copy",
            FlatOp::Rotate => "rotate",
            FlatOp::Scale => "scale",
            FlatOp::Mirror => "mirror",
        }
    }
    /// Prompt for the FIRST pick (base / pivot / axis-A).
    fn base_name(self) -> &'static str {
        match self {
            FlatOp::Move | FlatOp::Copy => "BASE point",
            FlatOp::Rotate | FlatOp::Scale => "PIVOT",
            FlatOp::Mirror => "FIRST axis point",
        }
    }
    /// Prompt for the SECOND pick.
    fn second_name(self) -> &'static str {
        match self {
            FlatOp::Move | FlatOp::Copy => "DESTINATION",
            FlatOp::Rotate => "angle point",
            FlatOp::Scale => "scale reference (dist from pivot)",
            FlatOp::Mirror => "SECOND axis point",
        }
    }
}

struct FlatMod {
    op: FlatOp,
    base: Option<Vec2>, // in sketch (u,v)
}

/// An in-flight 2D EDIT command — the pick-based modify tools, all backed by
/// `cad_kernel` (`Geom::offset`/`trim_at`/`extend_to`/`split_at`, `fillet_geoms`,
/// `chamfer_geoms`). Values (distance/radius) come from the command line; objects and
/// points come from clicks.
enum FlatEdit {
    Offset { dist: Option<f64>, obj: Option<usize> },
    Trim,
    Extend,
    Fillet { radius: Option<f64>, first: Option<(usize, Vec2)> },
    Chamfer { dist: Option<f64>, first: Option<(usize, Vec2)> },
    Break { obj: Option<usize>, p1: Option<Vec2> },
}

/// 3D viewport display mode — the standard CAD wireframe / shaded / shaded-with-edges
/// toggle (the mesh/face/render "regulate the display" control).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DisplayMode {
    Shaded,
    ShadedEdges,
    Wireframe,
}

impl DisplayMode {
    const ALL: [DisplayMode; 3] = [DisplayMode::Shaded, DisplayMode::ShadedEdges, DisplayMode::Wireframe];
    fn label(self) -> &'static str {
        match self {
            DisplayMode::Shaded => "Shaded",
            DisplayMode::ShadedEdges => "Shaded+Edges",
            DisplayMode::Wireframe => "Wireframe",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// App state
// ─────────────────────────────────────────────────────────────────────────────
struct Sandbox {
    model: Model,
    cached: SolidMesh,
    dirty: bool,
    display: DisplayMode,
    recaptured: bool, // set right after an edit re-evals + re-centres, so the next
                      // frame logs each object's new screen-space 8-corner box

    // camera (orbit). `cam_target` is STORED, not recomputed from bounds each
    // frame — otherwise adding/moving an object shifts the bounds centre and the
    // whole view jumps. Only ⌖ Frame (zoom-extents) re-centres it.
    yaw: f32,
    pitch: f32,
    dist: f32,
    cam_target: Vec3,

    // interaction
    card: bool,
    selection: Vec<u32>,
    selected_face: Vec<usize>, // triangle indices of the highlighted face
    selected_face_frame: Option<Frame>, // frame of the highlighted face (for "sketch on it")
    modify: Option<Modify>,
    sketch: Option<SketchMode>,
    // flat 2D sketch editor (a RIGHT split panel shown whenever a sketch is
    // active). app w2s = center + (world + offset)*scale, Y-down.
    sketch_scale: f32,
    sketch_offset: egui::Vec2,
    snap_enabled: SnapSet,      // osnap running set (END/MID/CEN/QUA by default)
    snap_override: Option<SnapKind>, // one-shot inline snap override (typed END/MID/…)
    sketch_sel: Vec<usize>,     // selected dobject indices in the active sketch's doc
    flat_mod: Option<FlatMod>,  // in-flight 2D move/copy on the sketch
    flat_edit: Option<FlatEdit>, // in-flight 2D edit (offset/trim/extend/fillet/…)
    // command-driven select-first: a queued command gathering its selection
    cmd: String,
    flat_cmd: String, // the flat-sketch command line (drives 2D drafting)
    selecting: bool,
    queued: Option<Queued>,
    status: String,
    hover_plane_pt: Option<Vec3>,
    dbg: DbgRecorder,
    dbg_window_open: bool,
    dbg_note_buf: String,
    solid_verts: Vec<V3>, // cached flat-shaded triangles (rebuilt only on re-eval)

    // authoring state for the NEXT primitive
    plane: Plane,
    place: Placement,
    op: BoolOp,
    prim_is_box: bool,
    box_wdh: [f32; 3],
    cyl_rh: [f32; 2],
    cyl_sides: u32,

    renderer: Arc<Mutex<SceneRenderer>>,
}

impl Sandbox {
    fn new() -> Self {
        let mut model = Model::default();
        model.push(BoolOp::Union, Plane::default(), Placement::default(), Primitive::Box { w: 2.0, d: 2.0, h: 1.0 });
        model.push(
            BoolOp::Difference,
            Plane::default(),
            Placement { u: 0.0, v: 0.0, lift: -0.3, spin_deg: 0.0 },
            Primitive::Cylinder { r: 0.55, h: 1.6, sides: 32 },
        );
        let cached = model.eval();
        let solid_verts = mesh_verts(&cached);
        let cam_target = match cached.bounds() {
            Some((mn, mx)) => Vec3::new((mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, (mn[2] + mx[2]) * 0.5),
            None => Vec3::ZERO,
        };
        let mut dbg = DbgRecorder::default();
        dbg.start("sandbox launch");
        Self {
            model,
            cached,
            solid_verts,
            dbg,
            dbg_window_open: true,
            dbg_note_buf: String::new(),
            dirty: false,
            display: DisplayMode::Shaded,
            recaptured: false,
            yaw: 0.9,
            pitch: 0.62,
            dist: 6.5,
            cam_target,
            card: false,
            selection: Vec::new(),
            selected_face: Vec::new(),
            selected_face_frame: None,
            modify: None,
            sketch: None,
            sketch_scale: 60.0,
            sketch_offset: egui::Vec2::ZERO,
            snap_enabled: SnapSet::defaults(),
            snap_override: None,
            sketch_sel: Vec::new(),
            flat_mod: None,
            flat_edit: None,
            cmd: String::new(),
            flat_cmd: String::new(),
            selecting: false,
            queued: None,
            status: String::new(),
            hover_plane_pt: None,
            plane: Plane::default(),
            place: Placement::default(),
            op: BoolOp::Union,
            prim_is_box: true,
            box_wdh: [1.0, 1.0, 1.0],
            cyl_rh: [0.5, 1.0],
            cyl_sides: 32,
            renderer: Arc::new(Mutex::new(SceneRenderer::default())),
        }
    }

    /// Re-evaluate the CSG and rebuild the cached triangle verts (records timing).
    fn recompute(&mut self) {
        let t = std::time::Instant::now();
        self.cached = self.model.eval();
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        self.solid_verts = mesh_verts(&self.cached);
        self.selected_face.clear(); // triangle indices are stale after re-eval
        self.dirty = false;
        self.note(format!(
            "eval: {} feats → {} tris in {:.1}ms",
            self.model.features.len(),
            self.cached.tri_count(),
            ms
        ));
    }

    fn next_primitive(&self) -> Primitive {
        if self.prim_is_box {
            Primitive::Box { w: self.box_wdh[0], d: self.box_wdh[1], h: self.box_wdh[2] }
        } else {
            Primitive::Cylinder { r: self.cyl_rh[0], h: self.cyl_rh[1], sides: self.cyl_sides }
        }
    }

    /// Zoom-extents: re-centre the camera on the model bounds and fit the distance.
    /// This is the ONLY thing that moves `cam_target` — edits never do, so the view
    /// stays put when objects are added/moved (even if a new one ends up off-screen).
    fn frame(&mut self) {
        if let Some((mn, mx)) = self.cached.bounds() {
            let ext = ((mx[0] - mn[0]).max(mx[1] - mn[1]).max(mx[2] - mn[2])).max(0.5);
            self.dist = ext * 2.6;
            self.cam_target = self.target();
        }
    }

    fn target(&self) -> Vec3 {
        match self.cached.bounds() {
            Some((mn, mx)) => Vec3::new((mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, (mn[2] + mx[2]) * 0.5),
            None => Vec3::ZERO,
        }
    }

    /// Enter = confirm/advance: finalise a gather selection → base-point pick, end a
    /// running op, or finish a draw. Called from the keyboard AND from an empty
    /// command line, so confirming the selection works whether or not the command
    /// box has focus (the bug that trapped users in gather mode: the always-focused
    /// text box swallowed Enter, so the gather could never be confirmed).
    fn confirm(&mut self) {
        if self.selecting {
            self.selecting = false;
            let q = self.queued.take();
            if self.selection.is_empty() {
                self.status = "operation cancelled — nothing selected".to_string();
            } else if let Some(q) = q {
                self.begin_queued(q); // gather done → straight to the base/pivot pick
            }
        } else if self.modify.is_some() {
            self.modify = None; // ends a running op
            self.status.clear();
        } else {
            self.finish_draw();
        }
    }

    /// Command-line dispatch — mirrors the 2D app's `run_command`. A verb (typed
    /// or from a button) starts the select-first flow.
    fn run_command(&mut self, raw: &str) {
        let v = raw.trim().to_lowercase();
        if v.is_empty() {
            // Empty Enter finishes an active draw (open pline / end line chain).
            if self.sketch.as_ref().map_or(false, |s| s.draw.active()) {
                self.finish_draw();
                self.sync_flat_prompt();
            }
            return;
        }
        // SINGLE COMMAND LINE (RUST_CAD model): when a sketch is open, this main
        // command line drafts 2D — draw verbs (line/pline/circle 3p/…), coordinates
        // (x,y), and options (pline C/U) are consumed here. 3D modifier verbs are NOT
        // draw verbs, so they fall through and still drive the 3D view (independent).
        if self.sketch.is_some() && self.try_draw_command(raw) {
            return;
        }
        // If a 3D modifier is mid-pick, typed input is its value/keyword (degrees,
        // factor, R=reference, C=copy) — feed it there. Only if it's NOT consumed do
        // we fall through to parse `v` as a new command (which then overrides).
        if let Some(mut md) = self.modify.take() {
            if let Some(f) = md.type_value(&v, &self.plane, &mut self.model) {
                self.note(format!("  {} typed '{v}' [{}] → {:?}", md.op.label(), md.pick_name(), f));
                match f {
                    Feed::NeedMore => {
                        self.status = md.prompt();
                        self.modify = Some(md);
                    }
                    Feed::AppliedContinue => {
                        self.dirty = true;
                        self.status = md.prompt();
                        self.modify = Some(md);
                    }
                    Feed::Applied => {
                        self.dirty = true;
                        self.recaptured = true;
                        if let Some(s) = &md.last_summary {
                            self.note(format!("  {} ✓ {s}", md.op.label()));
                        }
                        self.status.clear();
                    }
                }
                return;
            }
            self.modify = Some(md); // not a modifier value → treat as a new command
        }
        self.note(format!("cmd '{v}'"));
        if v == "dump" {
            self.dump_session();
            return;
        }
        if matches!(v.as_str(), "recorder" | "rec" | "dbg") {
            self.dbg_window_open = !self.dbg_window_open;
            return;
        }
        let op = match v.as_str() {
            "move" | "m" => Some(ModifyOp::Move),
            "copy" | "c" | "co" | "cp" => Some(ModifyOp::Copy),
            "rotate" | "ro" => Some(ModifyOp::Rotate),
            "scale" | "sc" => Some(ModifyOp::Scale),
            "mirror" | "mi" => Some(ModifyOp::Mirror),
            _ => None,
        };
        if let Some(op) = op {
            self.run_modifier(op);
        } else if matches!(v.as_str(), "erase" | "delete" | "e") {
            self.abort_3d();
            self.run_queued(Queued::Erase);
        } else {
            self.status = format!("unknown command: {v}");
        }
    }

    /// Route a typed line to the active sketch's drafting: a coordinate (`x,y`), an
    /// option (pline `C`/`U`, method switch), or a draw VERB. Returns true if it was a
    /// drafting input (so the main command line consumed it). Mirrors the app's
    /// intercept cascade — the active tool consumes coords/options first, then a new
    /// verb can override. Draw verbs are explicit (no bare `c`/`a`) so 3D `copy` etc.
    /// still fall through to the 3D view.
    fn try_draw_command(&mut self, raw: &str) -> bool {
        let idx = match &self.sketch {
            Some(s) => s.idx,
            None => return false,
        };
        let t = raw.trim();
        if self.sketch.as_ref().map_or(false, |s| s.draw.active()) {
            if let Some(k) = snap_kind_from(&t.to_lowercase()) {
                self.snap_override = Some(k);
                self.status = format!("next point: {} snap", k.name());
                self.note(format!("snap override → {}", k.name()));
                return true;
            }
            let last = self.sketch.as_ref().and_then(|s| s.draw.pending.last().copied());
            if let Some(uv) = resolve_point(t, last) {
                self.draw_click(uv);
                self.note(format!("cmd point ({:.3},{:.3})", uv.x, uv.y));
                return true;
            }
            if let Some(o) = self.sketch.as_mut().unwrap().draw.option(t) {
                match o {
                    CmdOutcome::Committed(g) => {
                        let kind = geom_kind(&g);
                        self.model.sketches[idx].doc.push(cad_kernel::DObject::new(g));
                        self.note(format!("draw ✓ option '{t}' → {kind} committed"));
                    }
                    CmdOutcome::Consumed => self.note(format!("draw · option '{t}' applied")),
                }
                self.sync_flat_prompt();
                return true;
            }
            // not a coord/option → maybe a new draw verb (falls through below).
        }
        let first = t.split_whitespace().next().unwrap_or("").to_lowercase();
        let is_draw = matches!(
            first.as_str(),
            "line" | "l" | "pline" | "pl" | "polyline" | "rect" | "rec" | "rectangle"
                | "circle" | "ci" | "arc" | "ellipse" | "el" | "ellip" | "point" | "po"
        );
        if is_draw {
            // commit-on-interrupt: keep the in-progress polyline's picked points.
            if self.sketch.as_ref().map_or(false, |s| s.draw.active()) {
                self.finish_draw();
            }
            self.abort_2d();
            self.sketch.as_mut().unwrap().draw.start_verb(t);
            self.sync_flat_prompt();
            self.note(format!("draw start '{t}'"));
            return true;
        }
        false
    }

    /// Esc cancel cascade (works regardless of command-line focus): clear a typed
    /// command → cancel selection-gather → running op → sketch entity → sketch.
    fn escape(&mut self) {
        // a half-typed command clears first
        if !self.flat_cmd.is_empty() || !self.cmd.is_empty() {
            self.flat_cmd.clear();
            self.cmd.clear();
            self.note("esc: cleared command line".into());
            return;
        }
        if self.selecting {
            self.selecting = false;
            self.queued = None;
            self.note("esc: cancelled selection".into());
        } else if self.modify.is_some() {
            self.modify = None;
            self.note("esc: cancelled 3D modifier".into());
        } else if self.flat_mod.is_some() {
            self.flat_mod = None;
            self.note("esc: cancelled 2D modifier".into());
        } else if self.flat_edit.is_some() {
            self.flat_edit = None;
            self.note("esc: cancelled 2D edit".into());
        } else if let Some(sm) = &mut self.sketch {
            if sm.draw.cancel() {
                self.note("esc: cancelled in-progress draw".into());
            } else {
                self.sketch = None;
                self.note("esc: left sketch".into());
            }
        }
        self.status.clear();
    }

    /// 3D-view modifier (MODIFY panel + command line) — ALWAYS operates on 3D
    /// features, entirely INDEPENDENT of the 2D sketch view.
    fn run_modifier(&mut self, op: ModifyOp) {
        self.abort_3d(); // new 3D command overrides the old 3D command
        self.run_queued(Queued::Modify(op));
    }

    /// Abort the 3D-view command state only (does not touch the 2D sketch view).
    fn abort_3d(&mut self) {
        self.modify = None;
        self.selecting = false;
        self.queued = None;
    }

    /// Abort the 2D flat-sketch command state only (does not touch the 3D view).
    fn abort_2d(&mut self) {
        if let Some(sm) = self.sketch.as_mut() {
            sm.draw.set_tool(DrawTool::None);
        }
        self.flat_mod = None;
        self.flat_edit = None;
    }

    /// Erase the selected 2D sketch dobjects (flat view only).
    fn flat_erase(&mut self) {
        if let Some(idx) = self.sketch.as_ref().map(|s| s.idx) {
            let mut sel = std::mem::take(&mut self.sketch_sel);
            sel.sort_unstable();
            sel.dedup();
            let doc = &mut self.model.sketches[idx].doc;
            for &i in sel.iter().rev() {
                if i < doc.dobjects.len() {
                    doc.dobjects.remove(i);
                }
            }
            self.status.clear();
        }
    }

    /// Start a 2D modifier on the sketch (select-first, like the app).
    fn start_flat_mod(&mut self, op: FlatOp) {
        if self.sketch_sel.is_empty() {
            self.status = format!("{}: select object(s) first, then run {} again", op.label(), op.label());
        } else {
            self.flat_mod = Some(FlatMod { op, base: None });
            self.status = format!("{}: pick {}", op.label(), op.base_name());
        }
    }

    /// Feed a base/second pick to the in-flight 2D modifier. Applies via the kernel's
    /// own `DObject::translated/rotated/scaled/mirrored` (shared with RUST_CAD): Move/
    /// Rotate/Scale mutate in place, Copy/Mirror push duplicates.
    fn flat_mod_feed(&mut self, uv: Vec2) {
        let idx = match self.sketch.as_ref() {
            Some(s) => s.idx,
            None => return,
        };
        let (op, base) = match &self.flat_mod {
            Some(fm) => (fm.op, fm.base),
            None => return,
        };
        let b = match base {
            None => {
                if let Some(fm) = self.flat_mod.as_mut() {
                    fm.base = Some(uv);
                }
                self.status = format!("{}: pick {}", op.label(), op.second_name());
                return;
            }
            Some(b) => b,
        };
        let kb = cad_kernel::Vec2::new(b.x as f64, b.y as f64);
        let ku = cad_kernel::Vec2::new(uv.x as f64, uv.y as f64);
        let sel = self.sketch_sel.clone();
        self.flat_mod = None;
        let doc = &mut self.model.sketches[idx].doc;
        let mutate = |doc: &mut cad_kernel::Document, f: &dyn Fn(&DObject) -> DObject| {
            for &i in &sel {
                if let Some(d) = doc.dobjects.get(i).cloned() {
                    doc.dobjects[i] = f(&d);
                }
            }
        };
        let duplicate = |doc: &mut cad_kernel::Document, f: &dyn Fn(&DObject) -> DObject| {
            let copies: Vec<DObject> = sel.iter().filter_map(|&i| doc.dobjects.get(i).cloned()).map(|d| f(&d)).collect();
            for c in copies {
                doc.push(c);
            }
        };
        let summary = match op {
            FlatOp::Move => {
                let v = ku - kb;
                mutate(doc, &|d| d.translated(v));
                format!("move ({:.3},{:.3})", v.x, v.y)
            }
            FlatOp::Copy => {
                let v = ku - kb;
                duplicate(doc, &|d| d.translated(v));
                format!("copy ({:.3},{:.3})", v.x, v.y)
            }
            FlatOp::Rotate => {
                let ang = (ku.y - kb.y).atan2(ku.x - kb.x);
                mutate(doc, &|d| d.rotated(kb, ang));
                format!("rotate {:.1}°", ang.to_degrees())
            }
            FlatOp::Scale => {
                let f = (ku - kb).len().max(1e-4);
                mutate(doc, &|d| d.scaled(kb, f));
                format!("scale ×{f:.3}")
            }
            FlatOp::Mirror => {
                // keep the original + add the mirrored copies (symmetric-profile default)
                duplicate(doc, &|d| d.mirrored(kb, ku));
                "mirror (kept original)".to_string()
            }
        };
        self.note(format!("flat {} ✓ {summary}", op.label()));
        self.status.clear();
    }

    /// Pick the nearest sketch dobject to `uv` (replace, or shift-toggle).
    fn flat_select(&mut self, uv: Vec2, add: bool) {
        let idx = match self.sketch.as_ref() {
            Some(s) => s.idx,
            None => return,
        };
        let tol = 10.0 / self.sketch_scale;
        let mut best: Option<(f32, usize)> = None;
        for (i, d) in self.model.sketches[idx].doc.dobjects.iter().enumerate() {
            let dist = dist_to_geom(&d.geom, uv);
            if dist < tol && best.map_or(true, |(bd, _)| dist < bd) {
                best = Some((dist, i));
            }
        }
        match best {
            Some((_, i)) => {
                if add {
                    if let Some(p) = self.sketch_sel.iter().position(|x| *x == i) {
                        self.sketch_sel.remove(p);
                    } else {
                        self.sketch_sel.push(i);
                    }
                } else {
                    self.sketch_sel = vec![i];
                }
            }
            None => {
                if !add {
                    self.sketch_sel.clear();
                }
            }
        }
        self.note(format!("flat select → {} object(s)", self.sketch_sel.len()));
    }

    /// Index of the nearest sketch dobject to `uv` within the pick tolerance.
    fn flat_nearest_obj(&self, uv: Vec2) -> Option<usize> {
        let idx = self.sketch.as_ref()?.idx;
        let tol = 10.0 / self.sketch_scale;
        let mut best: Option<(f32, usize)> = None;
        for (i, d) in self.model.sketches[idx].doc.dobjects.iter().enumerate() {
            let dist = dist_to_geom(&d.geom, uv);
            if dist < tol && best.map_or(true, |(bd, _)| dist < bd) {
                best = Some((dist, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// Start a pick-based 2D edit command (offset/trim/extend/fillet/chamfer/break).
    fn start_flat_edit(&mut self, e: FlatEdit) {
        self.status = match &e {
            FlatEdit::Offset { .. } => "offset: type distance".into(),
            FlatEdit::Fillet { .. } => "fillet: type radius".into(),
            FlatEdit::Chamfer { .. } => "chamfer: type distance".into(),
            FlatEdit::Trim => "trim: click the part of an object to cut (others = cutters) · Esc ends".into(),
            FlatEdit::Extend => "extend: click an object end (others = boundaries) · Esc ends".into(),
            FlatEdit::Break { .. } => "break: pick the object".into(),
        };
        self.flat_edit = Some(e);
    }

    /// A typed number for the active edit's value step (offset dist / fillet radius /
    /// chamfer dist). Returns true if consumed.
    fn flat_edit_value(&mut self, n: f64) -> bool {
        match self.flat_edit.as_mut() {
            Some(FlatEdit::Offset { dist, .. }) if dist.is_none() => {
                *dist = Some(n);
                self.status = "offset: pick object to offset".into();
                true
            }
            Some(FlatEdit::Fillet { radius, .. }) if radius.is_none() => {
                *radius = Some(n.max(0.0));
                self.status = "fillet: pick FIRST object".into();
                true
            }
            Some(FlatEdit::Chamfer { dist, .. }) if dist.is_none() => {
                *dist = Some(n.max(0.0));
                self.status = "chamfer: pick FIRST object".into();
                true
            }
            _ => false,
        }
    }

    /// Feed a click (sketch `u,v`) to the active pick-based edit.
    fn flat_edit_pick(&mut self, uv: Vec2) {
        let idx = match self.sketch.as_ref() {
            Some(s) => s.idx,
            None => return,
        };
        let ku = cad_kernel::Vec2::new(uv.x as f64, uv.y as f64);
        let hit = self.flat_nearest_obj(uv);
        let edit = match self.flat_edit.take() {
            Some(e) => e,
            None => return,
        };
        match edit {
            FlatEdit::Offset { dist: Some(d), obj: None } => match hit {
                Some(oi) => {
                    self.flat_edit = Some(FlatEdit::Offset { dist: Some(d), obj: Some(oi) });
                    self.status = "offset: pick side".into();
                }
                None => self.flat_edit = Some(FlatEdit::Offset { dist: Some(d), obj: None }),
            },
            FlatEdit::Offset { dist: Some(d), obj: Some(oi) } => {
                let doc = &mut self.model.sketches[idx].doc;
                if let Some(dobj) = doc.dobjects.get(oi).cloned() {
                    match dobj.geom.offset(d, ku) {
                        Ok(g) => {
                            doc.push(DObject::new(g));
                            self.note("flat offset ✓".into());
                        }
                        Err(e) => self.status = format!("offset: {e}"),
                    }
                }
                self.flat_edit = None;
            }
            FlatEdit::Trim => {
                if let Some(oi) = hit {
                    let doc = &mut self.model.sketches[idx].doc;
                    let cutters: Vec<cad_kernel::Geom> =
                        doc.dobjects.iter().enumerate().filter(|(i, _)| *i != oi).map(|(_, d)| d.geom.clone()).collect();
                    let target = doc.dobjects[oi].clone();
                    match target.geom.trim_at(&cutters, ku, false) {
                        Ok(survivors) => {
                            doc.dobjects.remove(oi);
                            for g in survivors {
                                doc.push(DObject::with_style(g, target.style.clone()));
                            }
                            self.note("flat trim ✓".into());
                        }
                        Err(e) => self.status = format!("trim: {e}"),
                    }
                }
                self.flat_edit = Some(FlatEdit::Trim); // stay active (repeat)
            }
            FlatEdit::Extend => {
                if let Some(oi) = hit {
                    let doc = &mut self.model.sketches[idx].doc;
                    let boundaries: Vec<cad_kernel::Geom> =
                        doc.dobjects.iter().enumerate().filter(|(i, _)| *i != oi).map(|(_, d)| d.geom.clone()).collect();
                    let target = doc.dobjects[oi].geom.clone();
                    match target.extend_to(&boundaries, ku, false) {
                        Ok(g) => {
                            doc.dobjects[oi].geom = g;
                            self.note("flat extend ✓".into());
                        }
                        Err(e) => self.status = format!("extend: {e}"),
                    }
                }
                self.flat_edit = Some(FlatEdit::Extend);
            }
            FlatEdit::Fillet { radius: Some(r), first: None } => match hit {
                Some(i1) => {
                    self.flat_edit = Some(FlatEdit::Fillet { radius: Some(r), first: Some((i1, uv)) });
                    self.status = "fillet: pick SECOND object".into();
                }
                None => self.flat_edit = Some(FlatEdit::Fillet { radius: Some(r), first: None }),
            },
            FlatEdit::Fillet { radius: Some(r), first: Some((i1, p1)) } => {
                if let Some(i2) = hit {
                    self.apply_fillet_chamfer(idx, i1, p1, i2, uv, Some(r), None);
                }
                self.flat_edit = None;
            }
            FlatEdit::Chamfer { dist: Some(d), first: None } => match hit {
                Some(i1) => {
                    self.flat_edit = Some(FlatEdit::Chamfer { dist: Some(d), first: Some((i1, uv)) });
                    self.status = "chamfer: pick SECOND object".into();
                }
                None => self.flat_edit = Some(FlatEdit::Chamfer { dist: Some(d), first: None }),
            },
            FlatEdit::Chamfer { dist: Some(d), first: Some((i1, p1)) } => {
                if let Some(i2) = hit {
                    self.apply_fillet_chamfer(idx, i1, p1, i2, uv, None, Some(d));
                }
                self.flat_edit = None;
            }
            FlatEdit::Break { obj: None, .. } => match hit {
                Some(oi) => {
                    self.flat_edit = Some(FlatEdit::Break { obj: Some(oi), p1: None });
                    self.status = "break: pick FIRST break point".into();
                }
                None => self.flat_edit = Some(FlatEdit::Break { obj: None, p1: None }),
            },
            FlatEdit::Break { obj: Some(oi), p1: None } => {
                self.flat_edit = Some(FlatEdit::Break { obj: Some(oi), p1: Some(uv) });
                self.status = "break: pick SECOND break point".into();
            }
            FlatEdit::Break { obj: Some(oi), p1: Some(p1) } => {
                let doc = &mut self.model.sketches[idx].doc;
                if let Some(dobj) = doc.dobjects.get(oi).cloned() {
                    let kp1 = cad_kernel::Vec2::new(p1.x as f64, p1.y as f64);
                    // split at p1 → keep the far piece; split that at p2 → drop the middle
                    if let Ok((a, _mid0)) = dobj.geom.split_at(kp1) {
                        if let Ok((_mid1, b)) = dobj.geom.split_at(ku) {
                            doc.dobjects.remove(oi);
                            doc.push(DObject::with_style(a, dobj.style.clone()));
                            doc.push(DObject::with_style(b, dobj.style.clone()));
                            self.note("flat break ✓".into());
                        }
                    }
                }
                self.flat_edit = None;
            }
            other => self.flat_edit = Some(other),
        }
    }

    /// Apply a fillet (radius) or chamfer (distance) between two picked objects.
    fn apply_fillet_chamfer(&mut self, idx: usize, i1: usize, p1: Vec2, i2: usize, p2: Vec2, radius: Option<f64>, cham: Option<f64>) {
        let kp1 = cad_kernel::Vec2::new(p1.x as f64, p1.y as f64);
        let kp2 = cad_kernel::Vec2::new(p2.x as f64, p2.y as f64);
        let doc = &mut self.model.sketches[idx].doc;
        let (g1, g2) = match (doc.dobjects.get(i1), doc.dobjects.get(i2)) {
            (Some(a), Some(b)) => (a.geom.clone(), b.geom.clone()),
            _ => return,
        };
        if let Some(r) = radius {
            match cad_kernel::fillet_geoms(&g1, kp1, &g2, kp2, r) {
                Ok(out) => {
                    doc.dobjects[i1].geom = out.g1_new;
                    doc.dobjects[i2].geom = out.g2_new;
                    if let Some(arc) = out.arc {
                        doc.push(DObject::new(arc));
                    }
                    self.note("flat fillet ✓".into());
                }
                Err(e) => self.status = format!("fillet: {e}"),
            }
        } else if let Some(d) = cham {
            match cad_kernel::chamfer_geoms(&g1, kp1, &g2, kp2, d, d) {
                Ok(out) => {
                    doc.dobjects[i1].geom = out.g1_new;
                    doc.dobjects[i2].geom = out.g2_new;
                    doc.push(DObject::new(out.bridge));
                    self.note("flat chamfer ✓".into());
                }
                Err(e) => self.status = format!("chamfer: {e}"),
            }
        }
    }

    /// JOIN the selected sketch dobjects into merged polylines (select-first).
    fn flat_join(&mut self) {
        let idx = match self.sketch.as_ref() {
            Some(s) => s.idx,
            None => return,
        };
        if self.sketch_sel.len() < 2 {
            self.status = "join: select 2+ touching objects first".into();
            return;
        }
        let mut sel = self.sketch_sel.clone();
        sel.sort_unstable();
        sel.dedup();
        let doc = &mut self.model.sketches[idx].doc;
        let input: Vec<(usize, cad_kernel::Geom)> =
            sel.iter().filter_map(|&i| doc.dobjects.get(i).map(|d| (i, d.geom.clone()))).collect();
        let out = cad_kernel::join_geoms(&input);
        if out.merged.is_empty() {
            self.status = "join: nothing merged (objects must touch end-to-end)".into();
            return;
        }
        let mut consumed = out.consumed_indices.clone();
        consumed.sort_unstable();
        consumed.dedup();
        for &i in consumed.iter().rev() {
            if i < doc.dobjects.len() {
                doc.dobjects.remove(i);
            }
        }
        for g in out.merged {
            doc.push(DObject::new(g));
        }
        self.sketch_sel.clear();
        self.note("flat join ✓".into());
        self.status.clear();
    }

    /// Push a Note event with the caller's source location, in the recorder's own
    /// format. `#[track_caller]` so `Location::caller()` points at the call site.
    #[track_caller]
    fn note(&mut self, message: String) {
        self.dbg.push(DbgEvent::Note { message }, std::panic::Location::caller());
    }

    /// Print the session recorder to STDERR (identical format to RUST_CAD; also
    /// mirrored live to /tmp/rust_cad_session.log). Paste it into chat to debug.
    fn dump_session(&mut self) {
        eprint!("{}", self.dbg.dump_text());
        self.status = format!("dumped {} events to the terminal (stderr)", self.dbg.events.len());
    }

    #[track_caller]
    fn dbg_snap(&mut self, reason: &str) {
        let loc = std::panic::Location::caller();
        if let Some(idx) = self.sketch.as_ref().map(|s| s.idx) {
            self.dbg.take_snapshot(&self.model.sketches[idx].doc, reason, 0, 0, loc);
        } else {
            let doc = cad_kernel::Document::default();
            self.dbg.take_snapshot(&doc, reason, 0, 0, loc);
        }
    }

    fn dbg_start(&mut self) {
        self.dbg.start("user pressed Start");
        self.dbg_snap("session start");
    }

    fn dbg_stop(&mut self) {
        self.dbg.stop("user pressed Stop");
    }

    /// Floating Session Recorder window — the app's `render_dbg_recorder_window`
    /// (identical controls: Start/Stop/Clear/Snap · status · Note · Copy timeline
    /// · backtrace · auto-snap). The two app-only capture sections (smart-block,
    /// menu-layout) are dropped — they depend on app-specific state.
    fn render_dbg_recorder_window(&mut self, ctx: &egui::Context) {
        if !self.dbg_window_open {
            return;
        }
        let mut open = self.dbg_window_open;
        egui::Window::new("🛰 Session Recorder")
            .open(&mut open)
            .default_pos(egui::pos2(40.0, 40.0))
            .default_size(egui::vec2(380.0, 200.0))
            .resizable(true)
            .collapsible(true)
            .show(ctx, |ui| {
                let is_recording = self.dbg.recording;
                ui.horizontal(|ui| {
                    let start_btn = egui::Button::new(egui::RichText::new("▶ Start").strong().color(egui::Color32::WHITE))
                        .fill(if is_recording {
                            egui::Color32::from_rgb(50, 90, 50)
                        } else {
                            egui::Color32::from_rgb(40, 130, 50)
                        });
                    if ui.add_enabled(!is_recording, start_btn).clicked() {
                        self.dbg_start();
                    }
                    let stop_btn = egui::Button::new(egui::RichText::new("■ Stop").strong().color(egui::Color32::WHITE))
                        .fill(if is_recording {
                            egui::Color32::from_rgb(160, 50, 50)
                        } else {
                            egui::Color32::from_rgb(80, 50, 50)
                        });
                    if ui.add_enabled(is_recording, stop_btn).clicked() {
                        self.dbg_stop();
                    }
                    ui.separator();
                    if ui.button("🗑 Clear").clicked() {
                        self.dbg.clear();
                    }
                    if ui.button("📷 Snap").clicked() {
                        self.dbg_snap("manual snap");
                    }
                });
                ui.add_space(4.0);
                ui.label(format!(
                    "Status: {}  ·  {} events  ·  {} snapshots",
                    if is_recording { "🔴 RECORDING" } else { "⚪ idle" },
                    self.dbg.events.len(),
                    self.dbg.snapshots.len()
                ));
                ui.add_space(6.0);
                ui.separator();
                ui.label("Annotate (📝 added with current ms):");
                ui.horizontal(|ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.dbg_note_buf)
                            .desired_width(240.0)
                            .hint_text("bug fired here / picked the wrong dobject / etc."),
                    );
                    if (ui.button("Drop note").clicked()
                        || (resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter))))
                        && !self.dbg_note_buf.is_empty()
                    {
                        let msg = std::mem::take(&mut self.dbg_note_buf);
                        self.note(msg);
                    }
                });
                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("📋 Copy timeline").clicked() {
                        let dump = self.dbg.dump_text();
                        ctx.copy_text(dump);
                    }
                    if ui.button("⤓ Dump → stderr").clicked() {
                        self.dump_session();
                    }
                    ui.checkbox(&mut self.dbg.capture_backtrace, "Capture backtrace (slow)");
                });
                ui.horizontal(|ui| {
                    ui.label("Auto-snap every:");
                    ui.add(
                        egui::DragValue::new(&mut self.dbg.auto_snap_every)
                            .speed(1.0)
                            .range(0..=10_000)
                            .suffix(" events"),
                    );
                });
            });
        self.dbg_window_open = open;
    }

    /// Run a command: with a live selection, execute immediately; otherwise enter
    /// the selection-gathering phase and remember what to do (the 2D `QueuedOp` +
    /// `begin_selection`). Enter later finalises → `begin_queued`.
    fn run_queued(&mut self, q: Queued) {
        self.note(format!("run {} (selection={})", q.label(), self.selection.len()));
        if self.selection.is_empty() {
            self.selecting = true;
            self.queued = Some(q);
            self.status = format!("{}: select objects, Enter to continue [Esc cancels]", q.label());
        } else {
            self.begin_queued(q);
        }
    }

    /// Execute a queued command against the current selection.
    fn begin_queued(&mut self, q: Queued) {
        // §8: record WHAT is highlighted (the actual handles), not just a count, so a
        // dump alone reconstructs the run.
        self.note(format!("begin {} on {} object(s) — sel={:?}", q.label(), self.selection.len(), self.selection));
        match q {
            Queued::Erase => {
                for id in std::mem::take(&mut self.selection) {
                    self.model.remove(id);
                }
                self.dirty = true;
                self.status.clear();
            }
            Queued::Modify(op) => {
                let m = Modify::new(op, self.selection.clone());
                self.status = m.prompt();
                self.modify = Some(m);
            }
        }
    }

    /// Unproject `cursor` (in `rect`) to a ray, intersect the active construction
    /// plane. `None` if the ray is parallel to or points away from the plane.
    fn cursor_on_plane(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<Vec3> {
        let (near, dir) = self.ray(cursor, rect, mvp);
        let n = self.plane.normal();
        let denom = dir.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (self.plane.origin() - near).dot(n) / denom;
        (t >= 0.0).then(|| near + dir * t)
    }

    /// Ray-pick the front-most feature under `cursor` via its world AABB.
    fn pick(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<u32> {
        let (near, dir) = self.ray(cursor, rect, mvp);
        let mut best: Option<(f32, u32)> = None;
        for f in &self.model.features {
            let (mn, mx) = f.world_aabb();
            if let Some(t) = cad_solid::ray_aabb(near, dir, mn, mx) {
                if best.map_or(true, |(bt, _)| t < bt) {
                    best = Some((t, f.id));
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// World-space ray (origin, unit dir) through a screen cursor.
    fn ray(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> (Vec3, Vec3) {
        let ndc_x = 2.0 * (cursor.x - rect.left()) / rect.width().max(1.0) - 1.0;
        let ndc_y = 1.0 - 2.0 * (cursor.y - rect.top()) / rect.height().max(1.0);
        let inv = Mat4::from_cols_array(mvp).inverse();
        let near = inv.project_point3(Vec3::new(ndc_x, ndc_y, -1.0));
        let far = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        (near, (far - near).normalize_or_zero())
    }

    fn is_drawing(&self) -> bool {
        self.sketch.as_ref().map_or(false, |s| s.draw.active())
    }

    /// Ray-pick the front-most solid SURFACE under the cursor → (hit point, face
    /// normal), testing every triangle of the evaluated mesh.
    fn pick_face(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<(Vec3, Vec3)> {
        let (orig, dir) = self.ray(cursor, rect, mvp);
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for tri in self.cached.positions.chunks_exact(3) {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                if best.map_or(true, |(bt, _, _)| t < bt) {
                    let n = (b - a).cross(c - a).normalize_or_zero();
                    best = Some((t, orig + dir * t, n));
                }
            }
        }
        best.map(|(_, p, n)| (p, n))
    }

    /// Ray-pick the front-most surface triangle index (for face selection).
    fn pick_triangle(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<usize> {
        let (orig, dir) = self.ray(cursor, rect, mvp);
        let mut best: Option<(f32, usize)> = None;
        for (i, tri) in self.cached.positions.chunks_exact(3).enumerate() {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                if best.map_or(true, |(bt, _)| t < bt) {
                    best = Some((t, i));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// One line per feature giving its 8 world-AABB corners projected to SCREEN
    /// pixels under `mvp` — the recorder's "how it looks on screen" evidence, so a
    /// dump alone confirms where each object landed before/after an edit (the view
    /// re-centres after every edit, so world coords aren't enough). Corner order is
    /// the `corners_of` bit order: x=bit0, y=bit1, z=bit2.
    fn screen_verts_lines(&self, rect: egui::Rect, mvp: &[f32; 16]) -> Vec<String> {
        let m = Mat4::from_cols_array(mvp);
        self.model
            .features
            .iter()
            .map(|f| {
                let (mn, mx) = f.world_aabb();
                let pts: Vec<String> = corners_of(mn, mx)
                    .iter()
                    .map(|w| {
                        let ndc = m.project_point3(*w);
                        let sx = rect.left() + (ndc.x * 0.5 + 0.5) * rect.width();
                        let sy = rect.top() + (0.5 - ndc.y * 0.5) * rect.height();
                        format!("({sx:.0},{sy:.0})")
                    })
                    .collect();
                format!("#{} 8v: {}", f.id, pts.join(" "))
            })
            .collect()
    }

    /// Push a `screen_verts_lines` snapshot into the recorder under `tag`.
    fn log_screen_verts(&mut self, tag: &str, rect: egui::Rect, mvp: &[f32; 16]) {
        for l in self.screen_verts_lines(rect, mvp) {
            self.note(format!("  {tag} {l}"));
        }
    }

    /// 3D vertex osnap for modifier base/destination picks — the nearest solid
    /// mesh vertex whose screen projection is within the aperture. Returns its
    /// world position (so copy/move snap to the solid's corners).
    fn snap_3d(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<(Vec3, egui::Pos2)> {
        let m = Mat4::from_cols_array(mvp);
        let aperture = 12.0f32;
        let mut best: Option<(f32, Vec3, egui::Pos2)> = None;
        for p in &self.cached.positions {
            let w = Vec3::from(*p);
            let ndc = m.project_point3(w);
            if !(-1.0..=1.0).contains(&ndc.z) {
                continue; // behind the camera / clipped
            }
            let sx = rect.left() + (ndc.x * 0.5 + 0.5) * rect.width();
            let sy = rect.top() + (0.5 - ndc.y * 0.5) * rect.height();
            let d = ((sx - cursor.x).powi(2) + (sy - cursor.y).powi(2)).sqrt();
            if d < aperture && best.map_or(true, |(bd, _, _)| d < bd) {
                best = Some((d, w, egui::pos2(sx, sy)));
            }
        }
        best.map(|(_, w, s)| (w, s))
    }

    /// Cursor → point on the ACTIVE SKETCH frame's plane.
    fn cursor_on_sketch(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<Vec3> {
        let sm = self.sketch.as_ref()?;
        let fr = self.model.sketches.get(sm.idx)?.frame;
        let (near, dir) = self.ray(cursor, rect, mvp);
        let n = fr.normal();
        let denom = dir.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (fr.origin - near).dot(n) / denom;
        (t >= 0.0).then(|| near + dir * t)
    }

    /// Feed a pick (in sketch-plane u,v) to the active draw command; commit any
    /// completed geom as a real `DObject` in the sketch's document.
    fn draw_click(&mut self, uv: Vec2) {
        let idx = match &self.sketch {
            Some(s) => s.idx,
            None => return,
        };
        // PLINE auto-close: clicking within ~8px of the first vertex (≥3 verts)
        // commits the run CLOSED (PLINE_GUIDE §4) — no need to type `C`.
        let auto_close = {
            let sm = self.sketch.as_ref().unwrap();
            sm.draw.tool == DrawTool::Polyline
                && sm.draw.pending.len() >= 3
                && sm.draw.pending.first().map_or(false, |v0| (uv - *v0).length() < 8.0 / self.sketch_scale)
        };
        if auto_close {
            self.flat_close_polyline();
            self.note("draw ✓ pline auto-closed on first vertex".into());
            return;
        }
        self.snap_override = None; // one-shot: consumed by this pick
        let (geom, prompt) = {
            let sm = self.sketch.as_mut().unwrap();
            (sm.draw.feed(uv), sm.draw.prompt())
        };
        // §recorder: log the pick and whether it committed an entity (kind + count).
        match geom {
            Some(g) => {
                let kind = geom_kind(&g);
                self.model.sketches[idx].doc.push(cad_kernel::DObject::new(g));
                let n = self.model.sketches[idx].doc.dobjects.len();
                self.note(format!("draw ✓ {kind} committed (#{n} in sketch) @ pick ({:.3},{:.3})", uv.x, uv.y));
            }
            None => self.note(format!("draw · pick ({:.3},{:.3}) — {prompt}", uv.x, uv.y)),
        }
        self.status = prompt;
    }

    /// Enter: commit an in-progress polyline / end a line chain.
    fn finish_draw(&mut self) {
        let idx = match &self.sketch {
            Some(s) => s.idx,
            None => return,
        };
        let geom = self.sketch.as_mut().unwrap().draw.finish();
        if let Some(g) = geom {
            let kind = geom_kind(&g);
            self.model.sketches[idx].doc.push(cad_kernel::DObject::new(g));
            let n = self.model.sketches[idx].doc.dobjects.len();
            self.note(format!("draw ✓ finish → {kind} committed (#{n} in sketch)"));
        } else {
            self.note("draw · finish (nothing to commit)".into());
        }
    }

    /// The FLAT-SKETCH command line — the RUST_CAD model: one entry, an intercept
    /// cascade. While a draw tool is active the command line IS the prompt (a typed
    /// `x,y` is a point answer, interchangeable with a click; letters are options).
    /// Otherwise a verb starts a tool. A new verb overrides the active tool.
    fn flat_command(&mut self, raw: &str) {
        let idx = match &self.sketch {
            Some(s) => s.idx,
            None => return,
        };
        let t = raw.trim().to_string();
        if !t.is_empty() {
            self.note(format!("flat cmd '{t}'")); // §recorder: every submitted line
        }
        // active EDIT value step (offset dist / fillet radius / chamfer dist)
        if self.flat_edit.is_some() {
            if let Ok(n) = t.parse::<f64>() {
                if self.flat_edit_value(n) {
                    return;
                }
            }
        }
        let drawing = self.sketch.as_ref().map_or(false, |s| s.draw.active());

        // ── intercept: the active draw tool consumes the line ──
        if drawing {
            if t.is_empty() {
                self.finish_draw(); // Enter = finish (open pline / end line chain)
                self.sync_flat_prompt();
                return;
            }
            // inline osnap override: `end`/`mid`/`cen`/… forces the NEXT pick's snap.
            if let Some(k) = snap_kind_from(&t.to_lowercase()) {
                self.snap_override = Some(k);
                self.status = format!("next point: {} snap", k.name());
                self.note(format!("snap override → {}", k.name()));
                return;
            }
            // a typed coordinate is a point answer (absolute / @relative / @polar)
            let last = self.sketch.as_ref().and_then(|s| s.draw.pending.last().copied());
            if let Some(uv) = resolve_point(&t, last) {
                self.draw_click(uv);
                return;
            }
            // a letter option for the active tool (pline C/U, method switches)
            let outcome = self.sketch.as_mut().unwrap().draw.option(&t);
            if let Some(o) = outcome {
                match o {
                    CmdOutcome::Committed(g) => {
                        let kind = geom_kind(&g);
                        self.model.sketches[idx].doc.push(cad_kernel::DObject::new(g));
                        self.note(format!("draw ✓ option '{t}' → {kind} committed"));
                    }
                    CmdOutcome::Consumed => self.note(format!("draw · option '{t}' applied")),
                }
                self.sync_flat_prompt();
                return;
            }
            // else: not a point/option → fall through so a new verb overrides.
        }

        if t.is_empty() {
            return;
        }
        // COMMIT-ON-INTERRUPT: a new command COMMITS the in-progress polyline's picked
        // points (≥2) instead of discarding them — so switching tools never loses work.
        if self.sketch.as_ref().map_or(false, |s| s.draw.active()) {
            self.finish_draw();
        }
        // start a draw verb (overrides any active tool — "new command wins")
        let started = {
            self.abort_2d();
            self.sketch.as_mut().unwrap().draw.start_verb(&t)
        };
        if started {
            self.note(format!("draw start '{t}'"));
            self.sync_flat_prompt();
            return;
        }
        // flat modifiers (2D transforms) + editing tools (offset/trim/…) + erase
        match t.to_lowercase().as_str() {
            "move" | "m" => self.start_flat_mod(FlatOp::Move),
            "copy" | "co" | "cp" => self.start_flat_mod(FlatOp::Copy),
            "rotate" | "ro" => self.start_flat_mod(FlatOp::Rotate),
            "scale" | "sc" => self.start_flat_mod(FlatOp::Scale),
            "mirror" | "mi" => self.start_flat_mod(FlatOp::Mirror),
            "offset" | "o" => self.start_flat_edit(FlatEdit::Offset { dist: None, obj: None }),
            "trim" | "tr" => self.start_flat_edit(FlatEdit::Trim),
            "extend" | "ex" => self.start_flat_edit(FlatEdit::Extend),
            "fillet" | "f" => self.start_flat_edit(FlatEdit::Fillet { radius: None, first: None }),
            "chamfer" | "cha" => self.start_flat_edit(FlatEdit::Chamfer { dist: None, first: None }),
            "break" | "br" => self.start_flat_edit(FlatEdit::Break { obj: None, p1: None }),
            "join" | "j" => self.flat_join(),
            "erase" | "e" | "delete" => self.flat_erase(),
            other => self.status = format!("unknown: {other}"),
        }
    }

    /// Leave the sketch, committing any in-progress polyline first (commit-on-interrupt).
    fn leave_sketch(&mut self) {
        if self.sketch.as_ref().map_or(false, |s| s.draw.active()) {
            self.finish_draw();
        }
        self.sketch = None;
        self.flat_mod = None;
    }

    /// Push the active draw tool's prompt into the status line.
    fn sync_flat_prompt(&mut self) {
        self.status = self.sketch.as_ref().map(|s| s.draw.prompt()).unwrap_or_default();
    }

    /// C: commit the in-progress polyline as a CLOSED loop.
    fn flat_close_polyline(&mut self) {
        let idx = match &self.sketch {
            Some(s) => s.idx,
            None => return,
        };
        let geom = self.sketch.as_mut().unwrap().draw.close();
        if let Some(g) = geom {
            let kind = geom_kind(&g);
            self.model.sketches[idx].doc.push(cad_kernel::DObject::new(g));
            self.note(format!("draw ✓ close → {kind} committed"));
        }
    }
}

impl eframe::App for Sandbox {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        theme::apply(ctx);
        // Esc ALWAYS cancels — even while the auto-focused command line holds the
        // keyboard (which makes `wants_keyboard_input()` true and would otherwise gate
        // this out). Handled once, here, so it never double-fires with the gated block.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.escape();
        }
        // Keyboard drives the command loop — but not while a text box has focus (so
        // typing "move"+Enter runs the command, not the finalisers).
        if !ctx.wants_keyboard_input() {
            // Enter: finalise the selection → end a running op → finish a draw.
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.confirm();
            }
            // Del erases the selection directly (only when idle).
            if self.modify.is_none()
                && self.sketch.is_none()
                && !self.selecting
                && !self.selection.is_empty()
                && ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
            {
                for id in std::mem::take(&mut self.selection) {
                    self.model.remove(id);
                }
                self.dirty = true;
            }
        }
        // Re-evaluate the CSG only when idle — never mid-drag — so dragging a
        // value in the panel doesn't re-run csgrs every frame (the lag source).
        if self.dirty && !ctx.is_using_pointer() {
            self.recompute();
        }
        self.controls_panel(ctx);
        self.cmd_bar(ctx);
        // Split view: the 3D viewport is always shown; when a sketch is active its
        // plane also "pops flat" into a right-side 2D panel — draw in both.
        if self.sketch.is_some() {
            self.flat_sketch_panel(ctx);
        }
        self.viewport_panel(ctx);
        self.navigator(ctx);
        self.render_dbg_recorder_window(ctx);
        if self.dbg.want_auto_snap() {
            self.dbg_snap("auto-snap cadence");
        }
    }
}

impl Sandbox {
    fn controls_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("controls")
            .exact_width(300.0)
            .frame(egui::Frame::none().fill(theme::SURFACE_1).inner_margin(14.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("3D Solid Sandbox");
                    ui.label(egui::RichText::new("parametric CSG · csgrs").color(theme::TEXT_MUTED).size(11.0));
                    ui.add_space(10.0);

                    // ── Modify (2D-identical: select-first, base + 2nd point) ──
                    section(ui, "MODIFY");
                    ui.horizontal_wrapped(|ui| {
                        for op in ModifyOp::ALL {
                            if ui.button(op.label()).clicked() {
                                self.run_modifier(op);
                            }
                        }
                        if ui.button("Erase").clicked() {
                            self.run_queued(Queued::Erase);
                        }
                    });
                    ui.horizontal(|ui| {
                        let card = ui.selectable_label(self.card, "CARD");
                        if card.clicked() {
                            self.card = !self.card;
                        }
                        card.on_hover_text("Cardinal lock: Move → single axis, Rotate → 90° snap");
                        if self.modify.is_some() && ui.button("Cancel (Esc)").clicked() {
                            self.modify = None;
                            self.status.clear();
                        }
                    });
                    if !self.status.is_empty() {
                        ui.label(egui::RichText::new(&self.status).color(theme::ACCENT).size(12.0));
                    } else {
                        ui.label(
                            egui::RichText::new(format!("{} selected", self.selection.len()))
                                .color(theme::TEXT_MUTED)
                                .size(11.0),
                        );
                    }
                    ui.add_space(12.0);

                    // ── Sketch on face ──────────────────────────────────────
                    section(ui, "SKETCH");
                    if self.sketch.is_some() {
                        ui.label(egui::RichText::new("editing in the flat panel →").color(theme::TEXT_MUTED).size(11.0));
                        if ui.button("Finish sketch").clicked() {
                            self.leave_sketch();
                            self.status.clear();
                        }
                    } else {
                        let has_face = self.selected_face_frame.is_some() && !self.selected_face.is_empty();
                        if ui
                            .add_enabled(has_face, egui::Button::new("▸ Sketch on SELECTED face").fill(theme::SURFACE_3))
                            .clicked()
                        {
                            if let Some(frame) = self.selected_face_frame {
                                let face_tris = self.selected_face.clone();
                                let reference = self.face_boundary_uv(&face_tris, &frame);
                                self.enter_sketch(frame, reference);
                            }
                        }
                        if ui.button("＋ Sketch on construction plane").clicked() {
                            let (u, v) = self.plane.axes();
                            let fr = Frame { origin: self.plane.origin(), u, v };
                            self.enter_sketch(fr, Vec::new());
                        }
                        ui.label(
                            egui::RichText::new("click a face to select → ▸ Sketch on it  ·  or right-click a face")
                                .color(theme::TEXT_MUTED)
                                .size(11.0),
                        );
                    }
                    ui.add_space(12.0);

                    // ── Construction plane ──────────────────────────────────
                    section(ui, "CONSTRUCTION PLANE");
                    ui.horizontal(|ui| {
                        for k in PlaneKind::ALL {
                            if ui.selectable_label(self.plane.kind == k, k.label()).clicked() {
                                self.plane.kind = k;
                            }
                        }
                        ui.label("offset");
                        ui.add(egui::DragValue::new(&mut self.plane.offset).speed(0.05).suffix(" m"));
                    });
                    ui.add_space(12.0);

                    // ── Add primitive ───────────────────────────────────────
                    section(ui, "ADD PRIMITIVE");
                    ui.horizontal(|ui| {
                        if ui.selectable_label(self.prim_is_box, "Box").clicked() {
                            self.prim_is_box = true;
                        }
                        if ui.selectable_label(!self.prim_is_box, "Cylinder").clicked() {
                            self.prim_is_box = false;
                        }
                    });
                    if self.prim_is_box {
                        xyz_row(ui, "size", &mut self.box_wdh);
                    } else {
                        ui.horizontal(|ui| {
                            ui.label("radius");
                            ui.add(egui::DragValue::new(&mut self.cyl_rh[0]).speed(0.02).range(0.02..=100.0));
                            ui.label("height");
                            ui.add(egui::DragValue::new(&mut self.cyl_rh[1]).speed(0.02).range(0.02..=100.0));
                            ui.label("sides");
                            ui.add(egui::DragValue::new(&mut self.cyl_sides).range(3..=128));
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("bool").color(theme::TEXT_MUTED).size(11.0));
                        for op in BoolOp::ALL {
                            if ui.selectable_label(self.op == op, format!("{} {}", op.glyph(), op.label())).clicked() {
                                self.op = op;
                            }
                        }
                    });
                    if ui
                        .add_sized([ui.available_width(), 26.0], egui::Button::new("＋ Add feature").fill(theme::SURFACE_3))
                        .clicked()
                    {
                        let prim = self.next_primitive();
                        let id = self.model.push(self.op, self.plane, self.place, prim);
                        self.selection = vec![id];
                        self.dirty = true;
                    }
                    ui.add_space(12.0);

                    // ── Selected feature (edit when exactly one) ────────────
                    self.selected_section(ui);

                    // ── Feature history ─────────────────────────────────────
                    section(ui, &format!("FEATURES ({})", self.model.features.len()));
                    let mut click_select: Option<u32> = None;
                    let add = ui.input(|i| i.modifiers.shift);
                    egui::ScrollArea::vertical().id_salt("feat_list").max_height(140.0).show(ui, |ui| {
                        for (i, f) in self.model.features.iter().enumerate() {
                            let tag = if i == 0 {
                                format!("#{}  base  {}", f.id, f.primitive.kind_label())
                            } else {
                                format!("#{}  {} {}", f.id, f.op.glyph(), f.primitive.kind_label())
                            };
                            if ui.selectable_label(self.selection.contains(&f.id), tag).clicked() {
                                click_select = Some(f.id);
                            }
                        }
                    });
                    if let Some(id) = click_select {
                        toggle_select(&mut self.selection, id, add);
                    }

                    ui.add_space(12.0);
                    section(ui, "VIEW");
                    ui.horizontal(|ui| {
                        if ui.button("⌖ Frame").clicked() {
                            self.frame();
                        }
                        ui.label(
                            egui::RichText::new(format!("{} tris", self.cached.tri_count()))
                                .color(theme::TEXT_MUTED)
                                .size(11.0),
                        );
                    });
                    // Display mode — standard wireframe / shaded / shaded+edges.
                    ui.horizontal(|ui| {
                        for m in DisplayMode::ALL {
                            if ui.selectable_label(self.display == m, m.label()).clicked() {
                                self.display = m;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("click=select object+face · shift=add · middle-drag=orbit · scroll=zoom · Del=erase")
                            .color(theme::TEXT_MUTED)
                            .size(11.0),
                    );

                    ui.add_space(12.0);
                    section(ui, "DEBUG");
                    ui.horizontal(|ui| {
                        if ui.selectable_label(self.dbg_window_open, "🛰 Recorder").clicked() {
                            self.dbg_window_open = !self.dbg_window_open;
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {} events",
                                if self.dbg.recording { "🔴 rec" } else { "idle" },
                                self.dbg.events.len()
                            ))
                            .color(theme::TEXT_MUTED)
                            .size(11.0),
                        );
                    });
                });
            });
    }

    fn selected_section(&mut self, ui: &mut egui::Ui) {
        if self.selection.len() != 1 {
            return;
        }
        let id = self.selection[0];
        let Some(idx) = self.model.features.iter().position(|f| f.id == id) else {
            return;
        };
        section(ui, &format!("SELECTED  #{}", id));
        let mut changed = false;
        let mut delete = false;
        {
            let f = &mut self.model.features[idx];
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("bool").color(theme::TEXT_MUTED).size(11.0));
                for op in BoolOp::ALL {
                    if ui.selectable_label(f.op == op, format!("{} {}", op.glyph(), op.label())).clicked() {
                        f.op = op;
                        changed = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("move u/v");
                changed |= ui.add(egui::DragValue::new(&mut f.placement.u).speed(0.05)).changed();
                changed |= ui.add(egui::DragValue::new(&mut f.placement.v).speed(0.05)).changed();
                ui.label("lift");
                changed |= ui.add(egui::DragValue::new(&mut f.placement.lift).speed(0.05)).changed();
            });
            ui.horizontal(|ui| {
                ui.label("rotate");
                changed |= ui.add(egui::DragValue::new(&mut f.placement.spin_deg).speed(1.0).suffix("°")).changed();
            });
            match &mut f.primitive {
                Primitive::Box { w, d, h } => {
                    ui.horizontal(|ui| {
                        ui.label("size");
                        for c in [w, d, h] {
                            changed |= ui.add(egui::DragValue::new(c).speed(0.02).range(0.02..=1000.0)).changed();
                        }
                    });
                }
                Primitive::Cylinder { r, h, sides } => {
                    ui.horizontal(|ui| {
                        ui.label("r/h");
                        changed |= ui.add(egui::DragValue::new(r).speed(0.02).range(0.02..=1000.0)).changed();
                        changed |= ui.add(egui::DragValue::new(h).speed(0.02).range(0.02..=1000.0)).changed();
                        changed |= ui.add(egui::DragValue::new(sides).range(3..=128)).changed();
                    });
                }
                // The Draw3D primitives (Sphere/Frustum/Torus/Capsule/Tube/Ellipsoid)
                // are authored in the APP — `3D Factory ▸ Draw3D` — which is where the
                // per-shape controllers live. This sandbox is superseded by that panel
                // and is scheduled for retirement (README slice 5); it is left here
                // read-only rather than growing a second editor for them.
                other => {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} — edit in the app: 3D Factory ▸ Draw3D",
                            other.kind_label()
                        ))
                        .weak(),
                    );
                }
            }
        }
        if ui
            .add_sized([ui.available_width(), 24.0], egui::Button::new(egui::RichText::new("🗑 Delete").color(theme::DANGER)))
            .clicked()
        {
            delete = true;
        }
        ui.add_space(12.0);
        if delete {
            self.model.remove(id);
            self.selection.clear();
            self.dirty = true;
        } else if changed {
            self.dirty = true;
        }
    }

    /// The command line — type a modifier verb (move/copy/rotate/scale/mirror/
    /// erase) + Enter; the live prompt echoes beside it. This is the primary
    /// trigger, identical to the app: run command → it asks for selection.
    /// While a literal TEXT body / text height is being typed, SPACE must stay a
    /// LITERAL space and never submit — else the first space in `"Hello world"` fires
    /// the command line. Spec `COMMAND_LINE_RULES.md` §4.3; RUST_CAD gates the same way
    /// on `TextDraftState::WaitingForString | text_waiting_height` (`app.rs:12707`).
    ///
    /// The sandbox has no text entry yet, so this is vacuously false. It exists as the
    /// SINGLE hook to extend when the flat sketch gains TEXT — one place to change, and
    /// nobody has to rediscover why `"Hello world"` broke.
    fn in_text_body(&self) -> bool {
        false
    }

    /// SPACE = ENTER — does this frame's input submit `buf`?
    /// Spec `COMMAND_LINE_RULES.md` §4; mirrors RUST_CAD `app.rs:12703-12712`.
    ///
    /// * **Enter** fires on `has_focus` OR `lost_focus` (egui surrenders focus on Enter).
    /// * **Space** fires ONLY when the box is FOCUSED — a canvas Space must never run a
    ///   command — and never inside a text body (§4.3).
    ///
    /// egui has ALREADY inserted the space into `buf` by the time we read the key event,
    /// so trailing spaces are stripped first: `"move "` → `"move"` (submits the command),
    /// `" "` → `""` (empty ⇒ the caller routes to `confirm()`). Both halves of §4 run
    /// through this ONE path, so a single space can never both submit a command and fire
    /// the empty-confirm in the same frame (the double-fire RUST_CAD works around with
    /// `if space_now { self.cmd.clear(); }` repeated at ~10 cascade arms).
    ///
    /// ⚠️ Consequence (§7): Space=Enter forecloses TYPED multi-token commands — after
    /// this, `circle 3p` submits at `circle`. Reach the method via the verb→option flow
    /// (`circle` ⏎ `3p`), which `Draw::option` already supports. Every 3D verb is
    /// single-token, so the 3D line is unaffected.
    fn cmd_submit(ui: &egui::Ui, r: &egui::Response, buf: &mut String, in_text_body: bool) -> bool {
        let enter =
            (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let space = r.has_focus()
            && !in_text_body
            && ui.input(|i| i.key_pressed(egui::Key::Space));
        if space {
            *buf = buf.trim_end_matches(' ').to_string();
        }
        enter || space
    }

    fn cmd_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("cmdline")
            .frame(egui::Frame::none().fill(theme::SURFACE_2).inner_margin(8.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("⌘").color(theme::TEXT_MUTED));
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.cmd)
                            .desired_width(300.0)
                            .hint_text("3D: move·copy·rotate·scale·mirror·erase  (Space or ⏎ submits)"),
                    );
                    let itb = self.in_text_body();
                    if Self::cmd_submit(ui, &r, &mut self.cmd, itb) {
                        let c = std::mem::take(&mut self.cmd);
                        // Empty Enter/Space in the box = confirm (finalise gather / end
                        // op), so a focused command line never traps the selection step.
                        if c.trim().is_empty() {
                            self.confirm();
                        } else {
                            self.run_command(&c);
                        }
                        r.request_focus();
                    } else if self.sketch.is_none() && ui.memory(|m| m.focused().is_none()) {
                        // keyboard-ready in the 3D view: grab the caret when nothing
                        // else has it (the flat command line owns focus when sketching).
                        r.request_focus();
                    }
                    if !self.status.is_empty() {
                        ui.label(egui::RichText::new(&self.status).color(theme::ACCENT).size(13.0));
                    }
                });
            });
    }

    fn viewport_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme::SURFACE_0))
            .show(ctx, |ui| {
                let size = ui.available_size();
                let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

                let aspect = (rect.width() / rect.height().max(1.0)).max(0.01);
                let mvp = mvp(self.yaw, self.pitch, self.dist, self.cam_target.into(), aspect);

                self.hover_plane_pt = resp.hover_pos().and_then(|p| self.cursor_on_plane(p, rect, &mvp));

                // After an edit re-evals + re-centres the camera (dirty cleared), log
                // each object's screen box under the NEW projection — so the dump
                // shows where things ended up once the view moved.
                if self.recaptured && !self.dirty {
                    self.log_screen_verts("recap", rect, &mvp);
                    self.recaptured = false;
                }

                // Right-click a solid FACE → start a sketch plane on that face.
                if resp.secondary_clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        if let (Some((p, n)), Some(tri)) =
                            (self.pick_face(pos, rect, &mvp), self.pick_triangle(pos, rect, &mvp))
                        {
                            // canonical (click-independent) frame so re-entering the
                            // SAME face reuses its sketch instead of spawning a new one.
                            let frame = canonical_frame(p, n);
                            let face_tris = cad_solid::coplanar_face(&self.cached.positions, tri);
                            let reference = self.face_boundary_uv(&face_tris, &frame);
                            self.note(format!("right-click FACE → sketch, {} boundary edges", reference.len()));
                            self.enter_sketch(frame, reference);
                        } else {
                            self.note("right-click missed any face".to_string());
                        }
                    }
                }
                // A left click: gather selection, feed a modifier, place a sketch
                // point, or (re)select.
                if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let mode = if self.selecting {
                            "gather"
                        } else if self.modify.is_some() {
                            "modify"
                        } else if self.is_drawing() {
                            "draw"
                        } else {
                            "idle"
                        };
                        self.note(format!("click @({:.0},{:.0}) mode={mode}", pos.x, pos.y));
                        if self.selecting {
                            if let Some(id) = self.pick(pos, rect, &mvp) {
                                toggle_select(&mut self.selection, id, true);
                            }
                            self.note(format!("  gather → {} selected", self.selection.len()));
                            if let Some(q) = self.queued {
                                self.status = format!(
                                    "{}: {} selected — press ENTER to pick base point (Esc cancels)",
                                    q.label(),
                                    self.selection.len()
                                );
                            }
                        } else if let Some(mut md) = self.modify.take() {
                            // OSNAP: prefer a snapped solid vertex, else the construction plane.
                            let snapped = self.snap_3d(pos, rect, &mvp).map(|(w, _)| w);
                            let w = snapped.or_else(|| self.cursor_on_plane(pos, rect, &mvp));
                            if let Some(w) = w {
                                let plane = self.plane;
                                // §8: name the pick + snap kind BEFORE feed mutates state.
                                let pick = md.pick_name();
                                let snaptag = if snapped.is_some() { "END" } else { "none" };
                                // Capture each object's 8 screen-corners BEFORE the edit
                                // (only emitted if it actually applies).
                                let before = self.screen_verts_lines(rect, &mvp);
                                let f = md.feed(w, &plane, &mut self.model, self.card);
                                self.note(format!(
                                    "  {} {pick} = ({:.2},{:.2},{:.2}) [snap={snaptag}] → {:?}",
                                    md.op.label(), w.x, w.y, w.z, f
                                ));
                                match f {
                                    Feed::NeedMore => {
                                        self.status = md.prompt();
                                        self.modify = Some(md);
                                    }
                                    Feed::Applied | Feed::AppliedContinue => {
                                        self.dirty = true;
                                        if let Some(s) = &md.last_summary {
                                            self.note(format!("  {} ✓ {s}", md.op.label()));
                                        }
                                        // Before/after screen boxes (same view) — the
                                        // camera re-centres next frame, logged then too.
                                        for l in &before {
                                            self.note(format!("  before {l}"));
                                        }
                                        let after = self.screen_verts_lines(rect, &mvp);
                                        for l in &after {
                                            self.note(format!("  after  {l}"));
                                        }
                                        self.recaptured = true;
                                        if matches!(f, Feed::AppliedContinue) {
                                            self.status = md.prompt();
                                            self.modify = Some(md);
                                        } else {
                                            self.status.clear();
                                        }
                                    }
                                }
                            } else {
                                self.note("  modify: click missed the plane".to_string());
                                self.modify = Some(md); // stay armed
                            }
                        } else if self.is_drawing() {
                            if let Some(w) = self.cursor_on_sketch(pos, rect, &mvp) {
                                if let Some(uv) = self.sketch.as_ref().map(|s| self.model.sketches[s.idx].frame.to_uv(w)) {
                                    self.draw_click(uv);
                                }
                                let ng = self.sketch.as_ref().map_or(0, |s| self.model.sketches[s.idx].doc.dobjects.len());
                                self.note(format!("  draw pt → {ng} dobjects in sketch"));
                            } else {
                                self.note("  draw: click missed the sketch plane".to_string());
                            }
                        } else {
                            let add = ui.input(|i| i.modifiers.shift);
                            let feat = self.pick(pos, rect, &mvp);
                            match feat {
                                Some(id) => toggle_select(&mut self.selection, id, add),
                                None => {
                                    if !add {
                                        self.selection.clear();
                                    }
                                }
                            }
                            // also highlight the exact FACE under the cursor + its
                            // frame, so it can be "popped flat" via the panel button
                            self.selected_face = self
                                .pick_triangle(pos, rect, &mvp)
                                .map(|t| cad_solid::coplanar_face(&self.cached.positions, t))
                                .unwrap_or_default();
                            self.selected_face_frame = self.pick_face(pos, rect, &mvp).map(|(p, n)| canonical_frame(p, n));
                            self.note(format!("  idle select: feat={feat:?} face={} tris", self.selected_face.len()));
                        }
                    }
                }
                // Orbit ONLY with the middle mouse button (or the corner
                // navigator). Left-drag stays free for picks, so moving a solid
                // never sends the camera flying.
                if resp.dragged_by(egui::PointerButton::Middle) {
                    let d = resp.drag_delta();
                    self.yaw -= d.x * 0.01;
                    self.pitch = (self.pitch + d.y * 0.01).clamp(-FRAC_PI_2 + 0.01, FRAC_PI_2 - 0.01);
                }
                if resp.hovered() {
                    let scroll = ui.input(|i| i.raw_scroll_delta.y);
                    if scroll != 0.0 {
                        self.dist = (self.dist * (1.0 - scroll * 0.0015)).clamp(0.4, 400.0);
                    }
                }

                // Display mode: Shaded = filled tris; Wireframe = mesh edges, no fill;
                // Shaded+Edges = both.
                let tris = if self.display == DisplayMode::Wireframe {
                    Vec::new()
                } else {
                    self.solid_verts.clone()
                };
                let mut lines = plane_grid(&self.plane);
                if matches!(self.display, DisplayMode::Wireframe | DisplayMode::ShadedEdges) {
                    let col = if self.display == DisplayMode::Wireframe { [0.60, 0.68, 0.78] } else { [0.25, 0.30, 0.36] };
                    for tri in self.cached.positions.chunks_exact(3) {
                        let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
                        seg(&mut lines, a, b, col);
                        seg(&mut lines, b, c, col);
                        seg(&mut lines, c, a, col);
                    }
                }
                // selection highlights (whole-object AABB)
                for id in &self.selection {
                    if let Some(f) = self.model.features.iter().find(|f| f.id == *id) {
                        let (mn, mx) = f.world_aabb();
                        aabb_lines(&mut lines, mn, mx, [0.0, 0.9, 1.0]);
                    }
                }
                // selected FACE highlight (warm outline over its triangles)
                for &t in &self.selected_face {
                    if 3 * t + 2 < self.cached.positions.len() {
                        let a = Vec3::from(self.cached.positions[3 * t]);
                        let b = Vec3::from(self.cached.positions[3 * t + 1]);
                        let c = Vec3::from(self.cached.positions[3 * t + 2]);
                        let col = [1.0, 0.62, 0.12];
                        seg(&mut lines, a, b, col);
                        seg(&mut lines, b, c, col);
                        seg(&mut lines, c, a, col);
                    }
                }
                // MODIFIER PREVIEW: baseline + pivot cross + live rotate/scale ghost.
                // The ghost is each target's world-AABB transformed by the LIVE value,
                // so the rotation/scale is visible before the click commits (the app's
                // translucent-ghost behaviour, spec §0.6). The numeric label is drawn
                // as a 2D overlay after the paint callback.
                let mut modify_label: Option<String> = None;
                if let Some(md) = &self.modify {
                    let plane = self.plane;
                    if let (Some(a), Some(h)) = (md.anchor_world(&plane), self.hover_plane_pt) {
                        seg(&mut lines, a, h, [0.95, 0.71, 0.24]); // baseline pivot→cursor
                        if matches!(md.op, ModifyOp::Rotate | ModifyOp::Scale) {
                            let ud = (plane.from_uv(Vec2::X) - plane.origin()).normalize_or_zero() * 0.5;
                            let vd = (plane.from_uv(Vec2::Y) - plane.origin()).normalize_or_zero() * 0.5;
                            seg(&mut lines, a - ud, a + ud, [1.0, 0.78, 0.31]);
                            seg(&mut lines, a - vd, a + vd, [1.0, 0.78, 0.31]);
                        }
                        if let Some(ang) = md.preview_angle(&plane, h, self.card) {
                            let axis = plane.normal();
                            for id in &md.targets {
                                if let Some(f) = self.model.features.iter().find(|f| f.id == *id) {
                                    let (mn, mx) = f.world_aabb();
                                    let c8 = corners_of(mn, mx).map(|p| rot_about(p, a, axis, ang));
                                    ghost_box(&mut lines, c8, [0.92, 0.92, 0.98]);
                                }
                            }
                            modify_label = Some(format!("{:.1}°{}", ang.to_degrees(), if md.copy { "  (copy)" } else { "" }));
                        }
                        if let Some(k) = md.preview_factor(&plane, h) {
                            for id in &md.targets {
                                if let Some(f) = self.model.features.iter().find(|f| f.id == *id) {
                                    let (mn, mx) = f.world_aabb();
                                    let c8 = corners_of(mn, mx).map(|p| scale_about(p, a, k));
                                    ghost_box(&mut lines, c8, [0.80, 0.95, 0.82]);
                                }
                            }
                            modify_label = Some(format!("×{:.3}{}", k, if md.copy { "  (copy)" } else { "" }));
                        }
                    }
                }
                // all sketches, then the active sketch's frame grid + in-progress preview
                for sk in &self.model.sketches {
                    sketch_lines(&mut lines, sk);
                }
                if let Some(sm) = &self.sketch {
                    let sk = &self.model.sketches[sm.idx];
                    frame_grid(&mut lines, &sk.frame);
                    // LIVE SHAPE PREVIEW in 3D too — same provisional geometry as the
                    // flat panel, mapped through the sketch frame onto the plane.
                    if sm.draw.active() {
                        if let Some(h) = resp.hover_pos().and_then(|p| self.cursor_on_sketch(p, rect, &mvp)) {
                            let cuv = sk.frame.to_uv(h);
                            for g in sm.draw.preview(cuv) {
                                for path in cad_solid::geom_outlines(&g) {
                                    let w: Vec<Vec3> = path.iter().map(|uv| sk.frame.from_uv(*uv)).collect();
                                    for s in w.windows(2) {
                                        seg(&mut lines, s[0], s[1], [0.95, 0.71, 0.24]);
                                    }
                                }
                            }
                        }
                    }
                }

                let renderer = self.renderer.clone();
                let cb = egui_glow::CallbackFn::new(move |info, painter| {
                    let gl = painter.gl();
                    let vp = info.viewport_in_pixels();
                    let screen = info.screen_size_px;
                    if let Ok(mut r) = renderer.lock() {
                        r.render(
                            gl, &tris, &lines, &mvp,
                            vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px,
                            screen[0] as i32, screen[1] as i32,
                        );
                    }
                });
                ui.painter().add(egui::PaintCallback { rect, callback: Arc::new(cb) });

                // 3D osnap marker (END square) at the hovered solid vertex during a
                // modifier base/destination pick — same glyph as the 2D view.
                if self.modify.is_some() {
                    if let Some(hp) = resp.hover_pos() {
                        if let Some((_, sp)) = self.snap_3d(hp, rect, &mvp) {
                            draw_snap_glyph(&ui.painter_at(rect), sp, SnapKind::End, theme::ACCENT);
                        }
                    }
                }
                // Live rotate-degree / scale-factor readout at the cursor (the app's
                // "{deg}°" / "×{factor}" label).
                if let (Some(txt), Some(hp)) = (modify_label, resp.hover_pos()) {
                    ui.painter_at(rect).text(
                        hp + egui::vec2(14.0, -14.0),
                        egui::Align2::LEFT_BOTTOM,
                        txt,
                        egui::FontId::proportional(14.0),
                        theme::ACCENT,
                    );
                }
            });
    }

    fn navigator(&mut self, ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("viewcube"))
            .anchor(egui::Align2::LEFT_TOP, egui::vec2(316.0, 16.0))
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(112.0, 150.0), egui::Sense::click());
                let p = ui.painter();
                let c = rect.center_top() + egui::vec2(0.0, 56.0);
                let radius = 50.0;
                p.circle_filled(c, radius, egui::Color32::from_rgba_unmultiplied(0x1a, 0x24, 0x30, 220));
                p.circle_stroke(c, radius, egui::Stroke::new(1.0, theme::BORDER));
                let sq = egui::Rect::from_center_size(c, egui::vec2(34.0, 34.0));
                p.rect_filled(sq, egui::Rounding::same(6.0), theme::SURFACE_3);
                p.rect_stroke(sq, egui::Rounding::same(6.0), egui::Stroke::new(1.0, theme::BORDER));
                let f = egui::FontId::proportional(12.0);
                p.text(c, egui::Align2::CENTER_CENTER, "TOP", f.clone(), theme::TEXT_PRIMARY);
                for (t, dir) in [("N", egui::vec2(0.0, -1.0)), ("E", egui::vec2(1.0, 0.0)), ("S", egui::vec2(0.0, 1.0)), ("W", egui::vec2(-1.0, 0.0))] {
                    p.text(c + dir * (radius - 12.0), egui::Align2::CENTER_CENTER, t, f.clone(), theme::ACCENT);
                }
                let by = rect.top() + 122.0;
                let btm = egui::Rect::from_min_size(egui::pos2(c.x - 52.0, by), egui::vec2(50.0, 22.0));
                let iso = egui::Rect::from_min_size(egui::pos2(c.x + 2.0, by), egui::vec2(50.0, 22.0));
                for (r, t) in [(btm, "Bottom"), (iso, "Iso")] {
                    p.rect_filled(r, egui::Rounding::same(6.0), theme::SURFACE_2);
                    p.rect_stroke(r, egui::Rounding::same(6.0), egui::Stroke::new(1.0, theme::BORDER));
                    p.text(r.center(), egui::Align2::CENTER_CENTER, t, egui::FontId::proportional(11.0), theme::TEXT_PRIMARY);
                }
                if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let preset = if btm.contains(pos) {
                            Some(ViewPreset::Bottom)
                        } else if iso.contains(pos) {
                            Some(ViewPreset::Iso)
                        } else if sq.contains(pos) {
                            Some(ViewPreset::Top)
                        } else if (pos - c).length() <= radius {
                            let d = pos - c;
                            if d.x.abs() > d.y.abs() {
                                Some(if d.x > 0.0 { ViewPreset::Right } else { ViewPreset::Left })
                            } else {
                                Some(if d.y > 0.0 { ViewPreset::Front } else { ViewPreset::Back })
                            }
                        } else {
                            None
                        };
                        if let Some(v) = preset {
                            let (yaw, pitch) = v.angles();
                            self.yaw = yaw;
                            self.pitch = pitch;
                        }
                    }
                }
                let _ = theme::WARNING;
            });
    }

    /// Start a sketch on `frame`, with `reference` face-outline geometry (u,v).
    fn enter_sketch(&mut self, frame: Frame, reference: Vec<cad_kernel::Geom>) {
        self.modify = None;
        self.sketch_sel.clear();
        self.flat_mod = None;
        self.sketch_offset = egui::Vec2::ZERO;
        self.sketch_scale = 60.0;
        // REUSE the sketch already on this plane — a sketch is a GROUP of 2D geometry
        // LINKED to its plane, so re-entering the same plane must show the same
        // drawing (not spawn a fresh empty sketch each time). Match by canonical frame.
        if let Some(idx) = self.model.sketches.iter().position(|s| same_plane(&s.frame, &frame)) {
            let n = self.model.sketches[idx].doc.dobjects.len();
            self.sketch = Some(SketchMode { idx, draw: Draw::new() });
            self.note(format!("re-entered sketch #{idx} on this plane ({n} object(s))"));
            self.status = format!("sketch #{idx} — {n} object(s); pick a draw tool (Esc = finish)");
            return;
        }
        let idx = self.model.sketches.len();
        let mut sk = Sketch::new(frame);
        sk.reference = reference;
        self.model.sketches.push(sk);
        self.sketch = Some(SketchMode { idx, draw: Draw::new() });
        self.note(format!("new sketch #{idx} on this plane"));
        self.status = "sketch active — pick a draw tool (Esc = finish)".to_string();
    }

    /// Boundary edges of a face (its triangle set), projected into the sketch
    /// frame's (u,v) as reference `Line` geoms — the outline the user sees + snaps to.
    fn face_boundary_uv(&self, face_tris: &[usize], frame: &Frame) -> Vec<cad_kernel::Geom> {
        use std::collections::HashMap;
        let pos = &self.cached.positions;
        let key = |p: [f32; 3]| -> (i64, i64, i64) {
            ((p[0] as f64 * 1e4).round() as i64, (p[1] as f64 * 1e4).round() as i64, (p[2] as f64 * 1e4).round() as i64)
        };
        // undirected edge → (count, world endpoints); boundary edges appear once
        let mut edges: HashMap<((i64, i64, i64), (i64, i64, i64)), (u32, [Vec3; 2])> = HashMap::new();
        for &t in face_tris {
            for e in 0..3 {
                let (pa, pb) = (pos[3 * t + e], pos[3 * t + (e + 1) % 3]);
                let (mut ka, mut kb) = (key(pa), key(pb));
                let (mut wa, mut wb) = (Vec3::from(pa), Vec3::from(pb));
                if ka > kb {
                    std::mem::swap(&mut ka, &mut kb);
                    std::mem::swap(&mut wa, &mut wb);
                }
                edges.entry((ka, kb)).or_insert((0, [wa, wb])).0 += 1;
            }
        }
        edges
            .values()
            .filter(|(c, _)| *c == 1)
            .map(|(_, [a, b])| {
                let (ua, ub) = (frame.to_uv(*a), frame.to_uv(*b));
                cad_kernel::Geom::Line(cad_kernel::Line {
                    a: cad_kernel::Vec2::new(ua.x as f64, ua.y as f64),
                    b: cad_kernel::Vec2::new(ub.x as f64, ub.y as f64),
                })
            })
            .collect()
    }

    /// Run the SHARED osnap engine (`cad_kernel::find_snap`) over the flat sketch —
    /// its drawn geometry + the face reference — at the cursor. Identical to the app.
    fn compute_flat_snap(&self, resp: &egui::Response, rect: egui::Rect) -> Option<cad_kernel::SnapHit> {
        let sm = self.sketch.as_ref()?;
        let hp = resp.hover_pos()?;
        let cursor = self.s2w_flat(hp, rect);
        let sk = &self.model.sketches[sm.idx];
        let mut objs: Vec<DObject> = sk.doc.dobjects.clone();
        for g in &sk.reference {
            objs.push(DObject::new(g.clone()));
        }
        // PHANTOM SNAP: feed the IN-PROGRESS pline/line vertices as a temporary object
        // so you can snap END to your OWN points (chain back, close the loop) before
        // they're committed — the app's `pline_phantom_dobject`.
        if sm.draw.active() && !sm.draw.pending.is_empty() {
            let pts = &sm.draw.pending;
            let g = if pts.len() >= 2 {
                let verts = pts
                    .iter()
                    .map(|p| cad_kernel::PolyVertex { pos: cad_kernel::Vec2::new(p.x as f64, p.y as f64), bulge: 0.0 })
                    .collect();
                cad_kernel::Geom::Polyline(cad_kernel::Polyline { vertices: verts, closed: false, widths: Vec::new() })
            } else {
                cad_kernel::Geom::Point(cad_kernel::Point {
                    location: cad_kernel::Vec2::new(pts[0].x as f64, pts[0].y as f64),
                    style: 0,
                    size: 0.0,
                })
            };
            objs.push(DObject::new(g));
        }
        if objs.is_empty() {
            return None;
        }
        let world_radius = (12.0 / self.sketch_scale) as f64;
        find_snap(
            cad_kernel::Vec2::new(cursor.x as f64, cursor.y as f64),
            world_radius,
            self.snap_enabled,
            self.snap_override, // one-shot inline override (typed END/MID/…)
            None,
            &objs,
            None,
        )
    }

    /// Flat-view world→screen (the app's `w2s`): centre + (world+offset)·scale, Y-down.
    fn w2s_flat(&self, uv: Vec2, rect: egui::Rect) -> egui::Pos2 {
        let c = rect.center();
        egui::pos2(
            c.x + (uv.x + self.sketch_offset.x) * self.sketch_scale,
            c.y - (uv.y + self.sketch_offset.y) * self.sketch_scale,
        )
    }

    /// Flat-view screen→world (inverse of `w2s_flat`).
    fn s2w_flat(&self, p: egui::Pos2, rect: egui::Rect) -> Vec2 {
        let c = rect.center();
        Vec2::new(
            (p.x - c.x) / self.sketch_scale - self.sketch_offset.x,
            -((p.y - c.y) / self.sketch_scale) - self.sketch_offset.y,
        )
    }

    fn draw_flat_grid(&self, painter: &egui::Painter, rect: egui::Rect) {
        let col = egui::Color32::from_rgb(0x22, 0x2c, 0x36);
        let axis = egui::Color32::from_rgb(0x3a, 0x4a, 0x54);
        let tl = self.s2w_flat(rect.left_top(), rect);
        let br = self.s2w_flat(rect.right_bottom(), rect);
        let x0 = tl.x.min(br.x).floor() as i32;
        let x1 = tl.x.max(br.x).ceil() as i32;
        let y0 = tl.y.min(br.y).floor() as i32;
        let y1 = tl.y.max(br.y).ceil() as i32;
        if (x1 - x0) > 500 || (y1 - y0) > 500 {
            return; // too zoomed out — skip the grid to avoid overdraw
        }
        for x in x0..=x1 {
            let a = self.w2s_flat(Vec2::new(x as f32, y0 as f32), rect);
            let b = self.w2s_flat(Vec2::new(x as f32, y1 as f32), rect);
            painter.line_segment([a, b], egui::Stroke::new(1.0, if x == 0 { axis } else { col }));
        }
        for y in y0..=y1 {
            let a = self.w2s_flat(Vec2::new(x0 as f32, y as f32), rect);
            let b = self.w2s_flat(Vec2::new(x1 as f32, y as f32), rect);
            painter.line_segment([a, b], egui::Stroke::new(1.0, if y == 0 { axis } else { col }));
        }
    }

    /// The flat 2D sketch editor — the plane "popped flat to screen". Renders the
    /// sketch's `Document` in 2D (app w2s) with pan/zoom; draw tools place picks.
    fn flat_sketch_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("flat_sketch")
            .default_width(560.0)
            .resizable(true)
            .frame(egui::Frame::none().fill(theme::SURFACE_0).inner_margin(6.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("✓ Done").clicked() {
                        self.leave_sketch();
                    }
                    ui.label(egui::RichText::new("FLAT SKETCH").color(theme::ACCENT).size(12.0).strong());
                    let n = self.sketch.as_ref().map_or(0, |s| self.model.sketches[s.idx].doc.dobjects.len());
                    ui.label(egui::RichText::new(format!("{n} obj · {} sel", self.sketch_sel.len())).color(theme::TEXT_MUTED).size(11.0));
                });
                // 2D toolbar — this view's OWN tools, independent of the 3D panel.
                ui.horizontal_wrapped(|ui| {
                    let cur = self.sketch.as_ref().map(|s| s.draw.tool);
                    for t in DrawTool::ALL {
                        if ui.selectable_label(cur == Some(t), t.label()).clicked() {
                            self.abort_2d();
                            if let Some(sm) = self.sketch.as_mut() {
                                sm.draw.set_tool(t);
                            }
                            self.status = self.sketch.as_ref().map(|s| s.draw.prompt()).unwrap_or_default();
                        }
                    }
                    ui.separator();
                    let mod_op = self.flat_mod.as_ref().map(|m| m.op);
                    for op in FlatOp::ALL {
                        let cap = match op {
                            FlatOp::Move => "Move",
                            FlatOp::Copy => "Copy",
                            FlatOp::Rotate => "Rotate",
                            FlatOp::Scale => "Scale",
                            FlatOp::Mirror => "Mirror",
                        };
                        if ui.selectable_label(mod_op == Some(op), cap).clicked() {
                            self.abort_2d();
                            self.start_flat_mod(op);
                        }
                    }
                    if ui.button("Erase").clicked() {
                        self.abort_2d();
                        self.flat_erase();
                    }
                });
                // EDIT tools (kernel-backed: offset/trim/extend/fillet/chamfer/break/join)
                ui.horizontal_wrapped(|ui| {
                    let active = self.flat_edit.is_some();
                    let btn = |ui: &mut egui::Ui, on: bool, cap: &str| {
                        ui.selectable_label(on, cap).clicked()
                    };
                    if btn(ui, false, "Offset") {
                        self.abort_2d();
                        self.start_flat_edit(FlatEdit::Offset { dist: None, obj: None });
                    }
                    if btn(ui, false, "Trim") {
                        self.abort_2d();
                        self.start_flat_edit(FlatEdit::Trim);
                    }
                    if btn(ui, false, "Extend") {
                        self.abort_2d();
                        self.start_flat_edit(FlatEdit::Extend);
                    }
                    if btn(ui, false, "Fillet") {
                        self.abort_2d();
                        self.start_flat_edit(FlatEdit::Fillet { radius: None, first: None });
                    }
                    if btn(ui, false, "Chamfer") {
                        self.abort_2d();
                        self.start_flat_edit(FlatEdit::Chamfer { dist: None, first: None });
                    }
                    if btn(ui, false, "Break") {
                        self.abort_2d();
                        self.start_flat_edit(FlatEdit::Break { obj: None, p1: None });
                    }
                    if btn(ui, false, "Join") {
                        self.abort_2d();
                        self.flat_join();
                    }
                    let _ = active;
                });
                // Per-tool construction METHOD + polyline controls (the app's Circle
                // 2P/3P/Ttr, Arc modes, pline Close/Undo — the "not limited" part).
                let tool = self.sketch.as_ref().map(|s| s.draw.tool);
                match tool {
                    Some(DrawTool::Circle) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("method:").color(theme::TEXT_MUTED).size(11.0));
                            let cur = self.sketch.as_ref().map(|s| s.draw.circle_method);
                            for m in CircleMethod::ALL {
                                if ui.selectable_label(cur == Some(m), m.label()).clicked() {
                                    if let Some(sm) = self.sketch.as_mut() {
                                        sm.draw.circle_method = m;
                                        sm.draw.pending.clear();
                                    }
                                }
                            }
                        });
                    }
                    Some(DrawTool::Arc) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("method:").color(theme::TEXT_MUTED).size(11.0));
                            let cur = self.sketch.as_ref().map(|s| s.draw.arc_method);
                            for m in ArcMethod::ALL {
                                if ui.selectable_label(cur == Some(m), m.label()).clicked() {
                                    if let Some(sm) = self.sketch.as_mut() {
                                        sm.draw.arc_method = m;
                                        sm.draw.pending.clear();
                                    }
                                }
                            }
                        });
                    }
                    Some(DrawTool::Ellipse) => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("method:").color(theme::TEXT_MUTED).size(11.0));
                            let cur = self.sketch.as_ref().map(|s| s.draw.ellipse_method);
                            for m in EllipseMethod::ALL {
                                if ui.selectable_label(cur == Some(m), m.label()).clicked() {
                                    if let Some(sm) = self.sketch.as_mut() {
                                        sm.draw.ellipse_method = m;
                                        sm.draw.pending.clear();
                                    }
                                }
                            }
                        });
                    }
                    Some(DrawTool::Polyline) => {
                        ui.horizontal_wrapped(|ui| {
                            let mode = self.sketch.as_ref().map(|s| s.draw.pline_mode);
                            if ui.selectable_label(mode == Some(cad_solid::draw::PlineMode::Line), "Line (L)").clicked() {
                                if let Some(sm) = self.sketch.as_mut() {
                                    sm.draw.pline_mode = cad_solid::draw::PlineMode::Line;
                                }
                            }
                            if ui.selectable_label(mode == Some(cad_solid::draw::PlineMode::Arc), "Arc (A)").clicked() {
                                if let Some(sm) = self.sketch.as_mut() {
                                    sm.draw.pline_mode = cad_solid::draw::PlineMode::Arc;
                                }
                            }
                            ui.separator();
                            if ui.button("Close (C)").clicked() {
                                self.flat_close_polyline();
                            }
                            if ui.button("Undo (U)").clicked() {
                                if let Some(sm) = self.sketch.as_mut() {
                                    sm.draw.undo_point();
                                }
                            }
                            if ui.button("Finish (⏎)").clicked() {
                                self.finish_draw();
                            }
                        });
                    }
                    _ => {}
                }
                if !self.status.is_empty() {
                    ui.label(egui::RichText::new(&self.status).color(theme::WARNING).size(11.0));
                }
                // COMMAND LINE — the primary driver (RUST_CAD model). Type a verb
                // (line/pline/circle 3p/arc sce/rect/ellipse/point), a coordinate x,y
                // (same as a click), or an option (pline C/U). Enter runs / advances.
                // Boxed + auto-focused so it's unmistakably an input and typing "just
                // works" (the app's `refocus_cmd`: grab focus whenever nothing else has
                // it — e.g. right after a canvas pick).
                egui::Frame::none()
                    .fill(theme::SURFACE_2)
                    .stroke(egui::Stroke::new(1.0, theme::ACCENT))
                    .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                    .rounding(4.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("⌘ Command").color(theme::ACCENT).strong());
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut self.flat_cmd)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("pline ␣ 0,0 ␣ 4,0 ␣ 4,3 ␣ C   ·   circle ␣ 3p   ·   arc ␣ sce   ·   rect · x,y"),
                            );
                            // Space=Enter (§4). The method is a SEPARATE submission now
                            // (`circle` ␣ `3p`) — `circle 3p` on one line submits at
                            // `circle`, so the hint above teaches the verb→option flow
                            // that `Draw::option` already supports (§7).
                            let itb = self.in_text_body();
                            if Self::cmd_submit(ui, &r, &mut self.flat_cmd, itb) {
                                let c = std::mem::take(&mut self.flat_cmd);
                                self.flat_command(&c);
                                r.request_focus();
                            } else if ui.memory(|m| m.focused().is_none()) {
                                // nothing else focused (e.g. after a canvas click) → grab it
                                r.request_focus();
                            }
                        });
                    });
                ui.separator();

                let size = ui.available_size();
                let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

                // A point-pick phase (drawing / flat modifier / flat edit) is
                // "click-only": press fires the pick, drag never pans.
                let point_pick = self.is_drawing() || self.flat_mod.is_some() || self.flat_edit.is_some();

                // pan (middle drag always; left drag ONLY when not in a point-pick) + zoom
                if resp.dragged_by(egui::PointerButton::Middle)
                    || (resp.dragged_by(egui::PointerButton::Primary) && !point_pick)
                {
                    let d = resp.drag_delta();
                    self.sketch_offset.x += d.x / self.sketch_scale;
                    self.sketch_offset.y -= d.y / self.sketch_scale;
                }
                if resp.hovered() {
                    let scroll = ui.input(|i| i.raw_scroll_delta.y);
                    if scroll != 0.0 {
                        let f = (1.0 + scroll * 0.0015).clamp(0.5, 2.0);
                        self.sketch_scale = (self.sketch_scale * f).clamp(2.0, 4000.0);
                    }
                }

                // OSNAP — the SHARED cad_kernel engine over the sketch + face reference
                let snap = self.compute_flat_snap(&resp, rect);
                let snap_uv = snap.as_ref().map(|h| Vec2::new(h.point.x as f32, h.point.y as f32));

                // Click routing: DRAW / flat MODIFIER = PRESS-fires-click (the app's
                // pickbox: capture the point on the PRESS frame, drift-proof — using
                // release/`clicked()` here lost picks to any small wobble). SELECT stays
                // on `clicked()`.
                let press_now = resp.contains_pointer() && ui.input(|i| i.pointer.primary_pressed());
                if self.is_drawing() {
                    if press_now {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let uv = snap_uv.unwrap_or_else(|| self.s2w_flat(pos, rect));
                            self.draw_click(uv);
                            self.note(format!("flat draw uv=({:.3},{:.3}) snap={:?}", uv.x, uv.y, snap.as_ref().map(|h| h.kind)));
                        }
                    }
                } else if self.flat_mod.is_some() {
                    if press_now {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let uv = snap_uv.unwrap_or_else(|| self.s2w_flat(pos, rect));
                            self.flat_mod_feed(uv);
                        }
                    }
                } else if self.flat_edit.is_some() {
                    if press_now {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let uv = snap_uv.unwrap_or_else(|| self.s2w_flat(pos, rect));
                            self.flat_edit_pick(uv);
                        }
                    }
                } else if resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let uv = self.s2w_flat(pos, rect);
                        let add = ui.input(|i| i.modifiers.shift);
                        self.flat_select(uv, add);
                    }
                }

                let painter = ui.painter_at(rect);
                self.draw_flat_grid(&painter, rect);
                if let Some(sm) = &self.sketch {
                    let sk = &self.model.sketches[sm.idx];
                    // face reference outline (faint) — shows WHERE you're drawing
                    let ref_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(0x36, 0x6c, 0x7e));
                    for g in &sk.reference {
                        for path in cad_solid::geom_outlines(g) {
                            let pts: Vec<egui::Pos2> = path.iter().map(|uv| self.w2s_flat(*uv, rect)).collect();
                            for w in pts.windows(2) {
                                painter.line_segment([w[0], w[1]], ref_stroke);
                            }
                        }
                    }
                    // drawn geometry
                    let stroke = egui::Stroke::new(1.4, egui::Color32::from_rgb(0x9a, 0xdc, 0xa0));
                    for d in &sk.doc.dobjects {
                        for path in cad_solid::geom_outlines(&d.geom) {
                            let pts: Vec<egui::Pos2> = path.iter().map(|uv| self.w2s_flat(*uv, rect)).collect();
                            for w in pts.windows(2) {
                                painter.line_segment([w[0], w[1]], stroke);
                            }
                        }
                    }
                    // selected dobjects (accent)
                    let sel_stroke = egui::Stroke::new(2.0, theme::ACCENT);
                    for &i in &self.sketch_sel {
                        if let Some(d) = sk.doc.dobjects.get(i) {
                            for path in cad_solid::geom_outlines(&d.geom) {
                                let pts: Vec<egui::Pos2> = path.iter().map(|uv| self.w2s_flat(*uv, rect)).collect();
                                for w in pts.windows(2) {
                                    painter.line_segment([w[0], w[1]], sel_stroke);
                                }
                            }
                        }
                    }
                    // COPY/MOVE shadow — ghost the selection at the cursor (app's approach:
                    // draw d.geom.translated(cursor − base) translucent + a base→cursor line)
                    if let (Some(fm), Some(hp)) = (self.flat_mod.as_ref(), resp.hover_pos()) {
                        if let Some(b) = fm.base {
                            let cur = snap_uv.unwrap_or_else(|| self.s2w_flat(hp, rect));
                            let v = cad_kernel::Vec2::new((cur.x - b.x) as f64, (cur.y - b.y) as f64);
                            let ghost = egui::Stroke::new(1.4, egui::Color32::from_rgba_unmultiplied(255, 200, 100, 200));
                            for &i in &self.sketch_sel {
                                if let Some(d) = sk.doc.dobjects.get(i) {
                                    let moved = d.geom.translated(v);
                                    for path in cad_solid::geom_outlines(&moved) {
                                        let pts: Vec<egui::Pos2> = path.iter().map(|uv| self.w2s_flat(*uv, rect)).collect();
                                        for w in pts.windows(2) {
                                            painter.line_segment([w[0], w[1]], ghost);
                                        }
                                    }
                                }
                            }
                            let (bs, cs) = (self.w2s_flat(b, rect), self.w2s_flat(cur, rect));
                            painter.line_segment([bs, cs], egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 200, 100)));
                        }
                    }
                    // LIVE SHAPE PREVIEW — the true geometry as if the cursor were the
                    // next click (circle/arc/ellipse/rect/line/pline), plus a marker at
                    // the first pick so it's obvious drawing is in progress.
                    if sm.draw.active() {
                        let cursor_uv = snap_uv.or_else(|| resp.hover_pos().map(|hp| self.s2w_flat(hp, rect)));
                        if let Some(cuv) = cursor_uv {
                            let pstroke = egui::Stroke::new(1.4, theme::WARNING);
                            for g in sm.draw.preview(cuv) {
                                for path in cad_solid::geom_outlines(&g) {
                                    let pts: Vec<egui::Pos2> = path.iter().map(|uv| self.w2s_flat(*uv, rect)).collect();
                                    for w in pts.windows(2) {
                                        painter.line_segment([w[0], w[1]], pstroke);
                                    }
                                }
                            }
                        }
                        if let Some(fp) = sm.draw.first_point() {
                            let s = self.w2s_flat(fp, rect);
                            painter.rect_stroke(
                                egui::Rect::from_center_size(s, egui::vec2(7.0, 7.0)),
                                0.0,
                                egui::Stroke::new(1.5, theme::ACCENT),
                            );
                        }
                    }
                }
                // osnap marker on top (identical glyphs to the app)
                if let Some(hit) = &snap {
                    let sp = self.w2s_flat(Vec2::new(hit.point.x as f32, hit.point.y as f32), rect);
                    draw_snap_glyph(&painter, sp, hit.kind, theme::ACCENT);
                }
            });
    }
}

/// Short name of a geometry kind (for recorder logs).
fn geom_kind(g: &cad_kernel::Geom) -> &'static str {
    use cad_kernel::Geom as G;
    match g {
        G::Line(_) => "line",
        G::Circle(_) => "circle",
        G::Arc(_) => "arc",
        G::Ellipse(_) => "ellipse",
        G::EllipseArc(_) => "ellipse-arc",
        G::Polyline(p) => {
            if p.closed {
                "polyline(closed)"
            } else {
                "polyline"
            }
        }
        G::Point(_) => "point",
        _ => "geom",
    }
}

/// A canonical sketch frame for a plane through `p` with normal `n`: the origin is
/// the foot of the perpendicular from the WORLD origin onto the plane (`n·(n·p)`),
/// which depends only on the plane — NOT on where the face was clicked. So clicking
/// anywhere on the same face yields the same frame, and its sketch can be reused.
fn canonical_frame(p: Vec3, n: Vec3) -> Frame {
    let nn = n.normalize_or_zero();
    Frame::from_point_normal(nn * nn.dot(p), nn)
}

/// Whether two frames describe the SAME drawing plane (parallel normals + coincident
/// canonical origin) — used to reuse a sketch when re-entering its plane.
fn same_plane(a: &Frame, b: &Frame) -> bool {
    a.normal().dot(b.normal()).abs() > 0.999 && (a.origin - b.origin).length() < 1e-3
}

/// Map a typed osnap keyword to a one-shot forced `SnapKind` (inline override).
fn snap_kind_from(t: &str) -> Option<SnapKind> {
    match t {
        "end" | "endp" | "endpoint" => Some(SnapKind::End),
        "mid" | "midpoint" => Some(SnapKind::Mid),
        "cen" | "center" | "centre" => Some(SnapKind::Cen),
        "qua" | "quad" | "quadrant" => Some(SnapKind::Qua),
        "int" | "intersection" => Some(SnapKind::Int),
        "per" | "perp" | "perpendicular" => Some(SnapKind::Per),
        "tan" | "tangent" => Some(SnapKind::Tan),
        "nea" | "near" | "nearest" => Some(SnapKind::Nea),
        _ => None,
    }
}

/// Parse a typed `x,y` (comma- or space-separated) into a sketch-frame `(u,v)` point.
fn parse_point_uv(s: &str) -> Option<Vec2> {
    let parts: Vec<&str> = s.trim().split(|c| c == ',' || c == ' ').filter(|p| !p.is_empty()).collect();
    if parts.len() != 2 {
        return None;
    }
    Some(Vec2::new(parts[0].parse::<f32>().ok()?, parts[1].parse::<f32>().ok()?))
}

/// Resolve a typed coordinate answer (AutoCAD modes): absolute `x,y`, RELATIVE
/// `@dx,dy` (offset from the last point), or POLAR `@dist<angle°` — the relative
/// forms need a `last` point (the previous vertex).
fn resolve_point(text: &str, last: Option<Vec2>) -> Option<Vec2> {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix('@') {
        let base = last?;
        if let Some((d, a)) = rest.split_once('<') {
            let dist = d.trim().parse::<f32>().ok()?;
            let ang = a.trim().parse::<f32>().ok()?.to_radians();
            return Some(base + Vec2::new(dist * ang.cos(), dist * ang.sin()));
        }
        return parse_point_uv(rest).map(|p| base + p);
    }
    parse_point_uv(t)
}

/// Toggle/replace an id in the selection (shift = add/toggle, else replace).
fn toggle_select(sel: &mut Vec<u32>, id: u32, add: bool) {
    if add {
        if let Some(pos) = sel.iter().position(|x| *x == id) {
            sel.remove(pos);
        } else {
            sel.push(id);
        }
    } else {
        *sel = vec![id];
    }
}

fn section(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(theme::TEXT_MUTED).size(11.0).strong());
    ui.add_space(4.0);
}

fn xyz_row(ui: &mut egui::Ui, label: &str, v: &mut [f32; 3]) {
    ui.horizontal(|ui| {
        ui.label(label);
        for c in v.iter_mut() {
            ui.add(egui::DragValue::new(c).speed(0.02).range(0.02..=1000.0));
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Vertex assembly (CPU side)
// ─────────────────────────────────────────────────────────────────────────────
#[repr(C)]
#[derive(Clone, Copy)]
struct V3 {
    x: f32,
    y: f32,
    z: f32,
    r: f32,
    g: f32,
    b: f32,
}

fn light_dir() -> Vec3 {
    Vec3::new(0.35, 0.25, 0.9).normalize()
}

fn mesh_verts(m: &SolidMesh) -> Vec<V3> {
    let base = [0.72, 0.75, 0.80];
    let mut out = Vec::with_capacity(m.positions.len());
    for tri in m.positions.chunks_exact(3) {
        let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
        let n = (b - a).cross(c - a).normalize_or_zero();
        let k = 0.35 + 0.65 * n.dot(light_dir()).abs();
        let col = [base[0] * k, base[1] * k, base[2] * k];
        for pt in [a, b, c] {
            out.push(V3 { x: pt.x, y: pt.y, z: pt.z, r: col[0], g: col[1], b: col[2] });
        }
    }
    out
}

fn plane_grid(plane: &Plane) -> Vec<V3> {
    let (u, v) = plane.axes();
    let o = plane.origin();
    let h = 6.0f32;
    let n = 12i32;
    let step = 2.0 * h / n as f32;
    let faint = [0.26, 0.30, 0.36];
    let axis_u = [0.58, 0.34, 0.34];
    let axis_v = [0.34, 0.52, 0.38];
    let mut out = Vec::new();
    for i in 0..=n {
        let t = -h + i as f32 * step;
        seg(&mut out, o + u * t - v * h, o + u * t + v * h, if t.abs() < 1e-4 { axis_v } else { faint });
        seg(&mut out, o + v * t - u * h, o + v * t + u * h, if t.abs() < 1e-4 { axis_u } else { faint });
    }
    out
}

/// A sketch's dobjects, flattened (kernel tessellation) and projected frame→world.
fn sketch_lines(out: &mut Vec<V3>, sk: &Sketch) {
    let col = [0.45, 0.85, 0.55];
    for d in &sk.doc.dobjects {
        for path in cad_solid::geom_outlines(&d.geom) {
            let w: Vec<Vec3> = path.iter().map(|uv| sk.frame.from_uv(*uv)).collect();
            for i in 0..w.len().saturating_sub(1) {
                seg(out, w[i], w[i + 1], col);
            }
        }
    }
}

/// A small grid on a sketch frame so the active sketch plane is visible.
fn frame_grid(out: &mut Vec<V3>, fr: &Frame) {
    let (u, v, o) = (fr.u, fr.v, fr.origin);
    // A small patch centred on the pick, so the sketch plane reads as sitting ON
    // the face instead of a huge grid sprawling past it.
    let h = 1.0f32;
    let n = 4i32;
    let step = 2.0 * h / n as f32;
    let col = [0.20, 0.60, 0.72];
    for i in 0..=n {
        let t = -h + i as f32 * step;
        seg(out, o + u * t - v * h, o + u * t + v * h, col);
        seg(out, o + v * t - u * h, o + v * t + u * h, col);
    }
}

fn aabb_lines(out: &mut Vec<V3>, mn: Vec3, mx: Vec3, c: [f32; 3]) {
    let corner = |i: usize| {
        Vec3::new(
            if i & 1 == 0 { mn.x } else { mx.x },
            if i & 2 == 0 { mn.y } else { mx.y },
            if i & 4 == 0 { mn.z } else { mx.z },
        )
    };
    for a in 0..8usize {
        for bit in [1usize, 2, 4] {
            let b = a ^ bit;
            if a < b {
                seg(out, corner(a), corner(b), c);
            }
        }
    }
}

fn seg(out: &mut Vec<V3>, a: Vec3, b: Vec3, c: [f32; 3]) {
    out.push(V3 { x: a.x, y: a.y, z: a.z, r: c[0], g: c[1], b: c[2] });
    out.push(V3 { x: b.x, y: b.y, z: b.z, r: c[0], g: c[1], b: c[2] });
}

/// The 8 corners of an axis-aligned box (min/max) — for a transformed ghost.
fn corners_of(mn: Vec3, mx: Vec3) -> [Vec3; 8] {
    let mut c = [Vec3::ZERO; 8];
    for (i, slot) in c.iter_mut().enumerate() {
        *slot = Vec3::new(
            if i & 1 == 0 { mn.x } else { mx.x },
            if i & 2 == 0 { mn.y } else { mx.y },
            if i & 4 == 0 { mn.z } else { mx.z },
        );
    }
    c
}

/// Draw the 12 edges of a box given its 8 (already-transformed) corners.
fn ghost_box(out: &mut Vec<V3>, c8: [Vec3; 8], col: [f32; 3]) {
    for a in 0..8usize {
        for bit in [1usize, 2, 4] {
            let b = a ^ bit;
            if a < b {
                seg(out, c8[a], c8[b], col);
            }
        }
    }
}

/// Min distance (in u,v units) from a point to a geom's tessellated outline.
fn dist_to_geom(g: &cad_kernel::Geom, p: Vec2) -> f32 {
    let mut best = f32::INFINITY;
    for path in cad_solid::geom_outlines(g) {
        for w in path.windows(2) {
            best = best.min(dist_point_segment(p, w[0], w[1]));
        }
    }
    best
}

fn dist_point_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-12 {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

/// AutoCAD-style osnap marker glyphs — copied VERBATIM from cad_app::draw_snap_glyph.
fn draw_snap_glyph(p: &egui::Painter, c: egui::Pos2, k: SnapKind, col: egui::Color32) {
    let s = 6.0; // half-extent
    let stroke = egui::Stroke::new(1.6, col);
    match k {
        SnapKind::End => {
            let r = egui::Rect::from_min_max(egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s));
            p.rect_stroke(r, 0.0, stroke);
        }
        SnapKind::Mid => {
            let pts = vec![
                egui::pos2(c.x, c.y - s),
                egui::pos2(c.x + s, c.y + s),
                egui::pos2(c.x - s, c.y + s),
                egui::pos2(c.x, c.y - s),
            ];
            p.add(egui::Shape::line(pts, stroke));
        }
        SnapKind::Cen => {
            p.circle_stroke(c, s, stroke);
            p.circle_filled(c, 1.5, col);
        }
        SnapKind::Qua => {
            let pts = vec![
                egui::pos2(c.x, c.y - s),
                egui::pos2(c.x + s, c.y),
                egui::pos2(c.x, c.y + s),
                egui::pos2(c.x - s, c.y),
                egui::pos2(c.x, c.y - s),
            ];
            p.add(egui::Shape::line(pts, stroke));
        }
        SnapKind::Int => {
            p.line_segment([egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)], stroke);
            p.line_segment([egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y - s)], stroke);
        }
        SnapKind::Per => {
            p.line_segment([egui::pos2(c.x, c.y - s), egui::pos2(c.x, c.y + s)], stroke);
            p.line_segment([egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y + s)], stroke);
        }
        SnapKind::Tan => {
            p.circle_stroke(c, s * 0.75, stroke);
            let y = c.y - s * 0.75;
            p.line_segment([egui::pos2(c.x - s, y), egui::pos2(c.x + s, y)], stroke);
        }
        SnapKind::Nea => {
            let pts = vec![
                egui::pos2(c.x - s, c.y - s),
                egui::pos2(c.x + s, c.y - s),
                egui::pos2(c.x - s, c.y + s),
                egui::pos2(c.x + s, c.y + s),
                egui::pos2(c.x - s, c.y - s),
            ];
            p.add(egui::Shape::line(pts, stroke));
        }
    }
}

fn mvp(yaw: f32, pitch: f32, dist: f32, target: [f32; 3], aspect: f32) -> [f32; 16] {
    let t = Vec3::from(target);
    let (cp, sp) = (pitch.cos(), pitch.sin());
    let (cy, sy) = (yaw.cos(), yaw.sin());
    let eye = t + Vec3::new(cp * cy, cp * sy, sp) * dist.max(0.1);
    let view = Mat4::look_at_rh(eye, t, Vec3::Z);
    let proj = Mat4::perspective_rh_gl(45f32.to_radians(), aspect.max(0.01), 0.05, (dist * 8.0).max(120.0));
    (proj * view).to_cols_array()
}

// ─────────────────────────────────────────────────────────────────────────────
// GL renderer — offscreen FBO, triangles + a line pass. Trimmed from
// cad_app/src/light3d.rs (proven), extended to also draw GL_LINES.
// ─────────────────────────────────────────────────────────────────────────────
const SCENE_VS: &str = r#"
    #version 330 core
    layout(location=0) in vec3 a_pos;
    layout(location=1) in vec3 a_col;
    uniform mat4 u_mvp;
    out vec3 v_col;
    void main() { gl_Position = u_mvp * vec4(a_pos, 1.0); v_col = a_col; }
"#;
const SCENE_FS: &str = r#"
    #version 330 core
    in vec3 v_col;
    out vec4 frag;
    void main() { frag = vec4(v_col, 1.0); }
"#;
const BLIT_VS: &str = r#"
    #version 330 core
    layout(location=0) in vec2 a_pos;
    layout(location=1) in vec2 a_uv;
    out vec2 v_uv;
    void main() { v_uv = a_uv; gl_Position = vec4(a_pos, 0.0, 1.0); }
"#;
const BLIT_FS: &str = r#"
    #version 330 core
    in vec2 v_uv;
    out vec4 frag;
    uniform sampler2D u_tex;
    void main() { frag = texture(u_tex, v_uv); }
"#;

#[derive(Default)]
struct SceneRenderer {
    inited: bool,
    scene_prog: Option<glow::Program>,
    u_mvp: Option<glow::UniformLocation>,
    scene_vao: Option<glow::VertexArray>,
    scene_vbo: Option<glow::Buffer>,
    blit_prog: Option<glow::Program>,
    u_tex: Option<glow::UniformLocation>,
    blit_vao: Option<glow::VertexArray>,
    blit_vbo: Option<glow::Buffer>,
    fbo: Option<glow::Framebuffer>,
    color: Option<glow::Texture>,
    depth: Option<glow::Renderbuffer>,
    fbo_w: i32,
    fbo_h: i32,
}

unsafe impl Send for SceneRenderer {}
unsafe impl Sync for SceneRenderer {}

impl SceneRenderer {
    fn ensure_init(&mut self, gl: &glow::Context) {
        if self.inited {
            return;
        }
        unsafe {
            let scene_prog = compile(gl, SCENE_VS, SCENE_FS);
            self.u_mvp = gl.get_uniform_location(scene_prog, "u_mvp");
            let svbo = gl.create_buffer().unwrap();
            let svao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(svao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(svbo));
            let stride = std::mem::size_of::<V3>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);

            let blit_prog = compile(gl, BLIT_VS, BLIT_FS);
            self.u_tex = gl.get_uniform_location(blit_prog, "u_tex");
            let bvbo = gl.create_buffer().unwrap();
            let bvao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(bvao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(bvbo));
            let bstride = (4 * std::mem::size_of::<f32>()) as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, bstride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, bstride, 8);

            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            self.scene_prog = Some(scene_prog);
            self.scene_vao = Some(svao);
            self.scene_vbo = Some(svbo);
            self.blit_prog = Some(blit_prog);
            self.blit_vao = Some(bvao);
            self.blit_vbo = Some(bvbo);
            self.inited = true;
        }
    }

    unsafe fn ensure_fbo(&mut self, gl: &glow::Context, w: i32, h: i32) {
        if self.fbo.is_some() && self.fbo_w == w && self.fbo_h == h {
            return;
        }
        if let Some(f) = self.fbo.take() {
            gl.delete_framebuffer(f);
        }
        if let Some(t) = self.color.take() {
            gl.delete_texture(t);
        }
        if let Some(d) = self.depth.take() {
            gl.delete_renderbuffer(d);
        }
        let color = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(color));
        gl.tex_image_2d(
            glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0,
            glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

        let depth = gl.create_renderbuffer().unwrap();
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
        gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, w, h);

        let fbo = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(color), 0);
        gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, Some(depth));

        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_renderbuffer(glow::RENDERBUFFER, None);
        self.fbo = Some(fbo);
        self.color = Some(color);
        self.depth = Some(depth);
        self.fbo_w = w;
        self.fbo_h = h;
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        gl: &glow::Context,
        tris: &[V3],
        lines: &[V3],
        mvp: &[f32; 16],
        vp_left: i32,
        vp_from_bottom: i32,
        vp_w: i32,
        vp_h: i32,
        screen_w: i32,
        screen_h: i32,
    ) {
        if vp_w <= 0 || vp_h <= 0 {
            return;
        }
        self.ensure_init(gl);
        unsafe {
            self.ensure_fbo(gl, vp_w, vp_h);
            gl.bind_framebuffer(glow::FRAMEBUFFER, self.fbo);
            gl.disable(glow::SCISSOR_TEST);
            gl.viewport(0, 0, vp_w, vp_h);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.disable(glow::BLEND);
            gl.clear_color(0.055, 0.07, 0.093, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

            if let (Some(prog), Some(vao), Some(vbo)) = (self.scene_prog, self.scene_vao, self.scene_vbo) {
                gl.use_program(Some(prog));
                if let Some(loc) = &self.u_mvp {
                    gl.uniform_matrix_4_f32_slice(Some(loc), false, mvp);
                }
                gl.bind_vertex_array(Some(vao));
                if !tris.is_empty() {
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(tris), glow::DYNAMIC_DRAW);
                    gl.draw_arrays(glow::TRIANGLES, 0, tris.len() as i32);
                }
                if !lines.is_empty() {
                    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(lines), glow::DYNAMIC_DRAW);
                    gl.draw_arrays(glow::LINES, 0, lines.len() as i32);
                }
                gl.bind_vertex_array(None);
            }

            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, screen_w.max(1), screen_h.max(1));
            gl.enable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::BLEND);
            let sw = screen_w.max(1) as f32;
            let sh = screen_h.max(1) as f32;
            let x0 = 2.0 * vp_left as f32 / sw - 1.0;
            let x1 = 2.0 * (vp_left + vp_w) as f32 / sw - 1.0;
            let y0 = 2.0 * vp_from_bottom as f32 / sh - 1.0;
            let y1 = 2.0 * (vp_from_bottom + vp_h) as f32 / sh - 1.0;
            let quad: [f32; 24] = [
                x0, y0, 0.0, 0.0, x1, y0, 1.0, 0.0, x1, y1, 1.0, 1.0,
                x0, y0, 0.0, 0.0, x1, y1, 1.0, 1.0, x0, y1, 0.0, 1.0,
            ];
            if let (Some(prog), Some(vao), Some(vbo), Some(color)) =
                (self.blit_prog, self.blit_vao, self.blit_vbo, self.color)
            {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(&quad), glow::DYNAMIC_DRAW);
                gl.use_program(Some(prog));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(color));
                if let Some(loc) = &self.u_tex {
                    gl.uniform_1_i32(Some(loc), 0);
                }
                gl.bind_vertex_array(Some(vao));
                gl.draw_arrays(glow::TRIANGLES, 0, 6);
                gl.bind_vertex_array(None);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            gl.enable(glow::BLEND);
            gl.use_program(None);
        }
    }
}

unsafe fn compile(gl: &glow::Context, vs: &str, fs: &str) -> glow::Program {
    let program = gl.create_program().expect("create_program");
    let one = |src: &str, kind: u32| -> glow::Shader {
        let s = gl.create_shader(kind).expect("create_shader");
        gl.shader_source(s, src);
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            panic!("sandbox shader compile failed:\n{}", gl.get_shader_info_log(s));
        }
        s
    };
    let v = one(vs, glow::VERTEX_SHADER);
    let f = one(fs, glow::FRAGMENT_SHADER);
    gl.attach_shader(program, v);
    gl.attach_shader(program, f);
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        panic!("sandbox program link failed:\n{}", gl.get_program_info_log(program));
    }
    gl.delete_shader(v);
    gl.delete_shader(f);
    program
}

fn bytes<T: Copy>(slice: &[T]) -> &[u8] {
    let len = std::mem::size_of_val(slice);
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, len) }
}
