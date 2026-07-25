//! 3D Factory — the `cad_solid` 3D solid layer, wired into the real app.
//!
//! This is the sandbox's core (`cad_solid/examples/sandbox.rs`) brought inside `cad_app`,
//! where all ~31.8k lines of 2D drafting + modify already work — so every plane can get the
//! FULL 2D toolset with nothing reimplemented. See `mentor MD/VENUE_DECISION_2D_ON_EVERY_PLANE.md`.
//!
//! What is deliberately NOT here: a renderer, a camera math fn, a command line, a cursor.
//! The app already has all of those. We reuse [`crate::light3d`]'s `Scene3dRenderer` + `mvp`
//! (the sandbox had duplicated both) and drive them with a `cad_solid::Model`.

use cad_solid::{BoolOp, Feature, Frame, Model, Placement, Plane, Primitive, SolidMesh};
use glam::{Mat4, Vec2, Vec3};

use crate::light3d::V3;

/// The standard camera orientations the nav gizmo snaps to — the six orthographic
/// faces plus an isometric, exactly the set every 3D solid app puts in its corner cube.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdView {
    Top, Bottom, Front, Back, Left, Right, Iso,
}

/// The 3D-Factory zoom mode — mirrors the 2D zoom command. Bare `z` → `Window` (the 2D
/// default: DRAG a box, or click two corners, with an amber "zoom window" rubber-band);
/// `z r` → `RealTime` (drag up/down dollies). `Off` = idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoomMode {
    Off,
    Window,
    RealTime,
}

/// A promoted wall kept ALIVE — the Factory owns its **footprint** (the ground-plane
/// polyline) so the wall stays fully editable after promotion: change its height, or
/// move / add / delete a footprint vertex, and it re-derives.
///
/// The floor ring (`z = 0`) and the ceiling ring (`z = height`) are BOTH derived from the
/// SAME `footprint` points — so a vertex is a vertical edge present on *both* rings by
/// construction; they can never drift apart. This is why "add a vertex in Top view → it
/// lands on top AND bottom" is automatic (owner, 2026-07-22), not a special case: there is
/// only one set of points driving both rings.
///
/// Each consecutive footprint pair extrudes to one Box `Feature`; `segments[i]` is the
/// feature id of the i-th segment (`footprint.len() − 1` of them), in order. `rake`
/// (lean-from-vertical) is stored for the day the kernel gains a tilt DOF — today a
/// `Feature` is axis-aligned only, so it is not applied yet (and only then can top ≠
/// bottom, relaxing the "both rings" coupling).
#[derive(Clone, Debug)]
pub struct WallInst {
    /// Ground-plane footprint, ≥ 2 points. Shared by the floor and ceiling rings.
    pub footprint: Vec<Vec2>,
    /// One Box feature id per segment (`footprint.len() − 1` of them), in order.
    pub segments: Vec<u32>,
    pub thickness: f32,
    pub height: f32,
    pub rake_deg: f32,
    /// Z the wall STANDS ON — the base of the storey it was built on. Held on the wall
    /// rather than only in the feature's placement because `rederive_wall` drops and
    /// rebuilds the Boxes: without it, editing a vertex on the third floor would silently
    /// drop that wall to the ground.
    pub base_z: f32,
}

/// An open sketch-on-plane session.
///
/// **The core trick of 3D_Factory:** while this is live, the app's active `doc` IS the
/// sketch's `Document`. Every 2D tool in `cad_app` only ever knows `self.doc` — so draw,
/// fillet (with its R/T/M/P options), trim, extend, offset, chamfer, break, the command
/// line, snaps and layers ALL operate on the plane, **unchanged and complete**, with
/// nothing reimplemented. That is the whole thesis of this fork.
///
/// `undo_stack`/`redo_stack` are `Vec<Document>` (full snapshots), so they must be parked
/// alongside the model-space doc — otherwise an undo inside the sketch would restore a
/// model-space document over the sketch. The sketch gets a fresh, empty undo history.
pub struct SketchSession {
    /// Index into `Model::sketches`.
    pub idx: usize,
    pub saved_doc: cad_kernel::Document,
    /// The main drawing's undo/redo history, parked while the sketch owns `doc`. Holds
    /// `UndoStep`s (not bare Documents) since undo spans 2D and 3D in one stack.
    pub saved_undo: Vec<crate::app::UndoStep>,
    pub saved_redo: Vec<crate::app::UndoStep>,
}

/// Fixed key light, matching `light3d`'s shading so the two 3D views look alike.
fn shade(base: [f32; 3], n: Vec3) -> [f32; 3] {
    let dir = Vec3::new(0.35, 0.25, 0.9).normalize();
    let k = 0.35 + 0.65 * n.dot(dir).abs();
    [base[0] * k, base[1] * k, base[2] * k]
}

/// Brighter shading for imported furniture — a higher ambient floor (0.6) so meshes read
/// clearly instead of coming out murky-dark, which is how many imports looked.
fn shade_furniture(base: [f32; 3], n: Vec3) -> [f32; 3] {
    let dir = Vec3::new(0.35, 0.25, 0.9).normalize();
    let k = 0.6 + 0.4 * n.dot(dir).abs();
    [(base[0] * k).min(1.0), (base[1] * k).min(1.0), (base[2] * k).min(1.0)]
}

fn v(p: Vec3, c: [f32; 3]) -> V3 {
    V3 { x: p.x, y: p.y, z: p.z, r: c[0], g: c[1], b: c[2] }
}

/// Even–odd ray-cast point-in-polygon test in XY. Used to tell whether a ceiling triangle
/// sits over the OPEN room interior (a floor footprint) — where it is hidden — or over the
/// surrounding wall, where it is kept.
fn point_in_poly(poly: &[Vec2], x: f32, y: f32) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i].x, poly[i].y);
        let (xj, yj) = (poly[j].x, poly[j].y);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The 8 corners of an AABB, bit order x=1, y=2, z=4 (same as the sandbox's `corners_of`).
fn corners_of(mn: Vec3, mx: Vec3) -> [Vec3; 8] {
    let mut o = [Vec3::ZERO; 8];
    for (i, slot) in o.iter_mut().enumerate() {
        *slot = Vec3::new(
            if i & 1 == 0 { mn.x } else { mx.x },
            if i & 2 == 0 { mn.y } else { mx.y },
            if i & 4 == 0 { mn.z } else { mx.z },
        );
    }
    o
}

fn seg(out: &mut Vec<V3>, a: Vec3, b: Vec3, c: [f32; 3]) {
    out.push(v(a, c));
    out.push(v(b, c));
}

/// The 12 edges of an AABB.
fn aabb_lines(out: &mut Vec<V3>, mn: Vec3, mx: Vec3, c: [f32; 3]) {
    let k = corners_of(mn, mx);
    // pairs differing by exactly one bit = the 12 edges
    for i in 0..8usize {
        for bit in [1usize, 2, 4] {
            let j = i | bit;
            if j != i {
                seg(out, k[i], k[j], c);
            }
        }
    }
}

/// 3D Factory state — the model + its view. Lives on `CadApp` as one field.
pub struct FactoryState {
    pub open: bool,
    pub model: Model,
    /// Evaluated CSG mesh, rebuilt only when `dirty` (csgrs is not cheap).
    pub cached: SolidMesh,
    pub dirty: bool,
    pub selection: Vec<u32>,

    // orbit camera — `cam_target` is STORED, never recomputed from bounds each frame,
    // so the view does not jump when a solid is added or moved (sandbox lesson).
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    pub cam_dist: f32,
    pub cam_target: [f32; 3],
    /// Parallel (orthographic) projection — TRUE after a standard-view snap (Top/Front/…/
    /// Iso) so a cylinder reads as a true CIRCLE in Top (no perspective barrel); FALSE while
    /// free-orbiting (perspective depth). CAD convention: standard views are orthographic.
    pub ortho: bool,

    /// Live sketch-on-plane session (the app's `doc` is swapped while `Some`).
    pub session: Option<SketchSession>,
    /// Face picked by the last right-click — what the context menu acts on.
    pub pending_face: Option<Frame>,
    /// While sketching ON a face, the 3D object's feature edges projected onto the sketch
    /// plane (in that frame's u,v). Drawn faintly in the 2D canvas so it is a real 2D VIEW
    /// of the object instead of a blank void. Empty when not sketching on a face.
    pub sketch_ref: Vec<[Vec2; 2]>,

    pub box_w: f32,
    pub box_d: f32,
    pub box_h: f32,
    pub cyl_r: f32,
    pub cyl_h: f32,
    pub cyl_sides: u32,

    /// DRAW3D: the open primitive dialog (`None` = closed). The dialog OWNS the
    /// live parameters, so tweaking them costs nothing until Create is pressed —
    /// csgrs walks a BSP per boolean, so we never re-evaluate per keystroke.
    pub draw3d: Option<Draw3dDialog>,

    /// DRAW3D edit-binding: when exactly one solid is selected, the dialog's
    /// controllers edit THAT feature live. This holds the id currently bound, so the
    /// dialog reloads its fields only when the selection changes — not every frame,
    /// which would stomp the user's edits mid-drag.
    pub draw3d_edit: Option<u32>,
    /// A primitive built in the Draw3D dialog and awaiting a placement CLICK in the 3D
    /// view — created at the picked point (a Box's corner / everything else centred),
    /// not at the origin. `None` = nothing waiting to be placed.
    pub place_pending: Option<Primitive>,

    /// 3D wall extrusion height — the ONE thing a 2D wall lacks. A promoted wall keeps
    /// its own (per-wall) thickness and rises to this height. Kept in the 3D layer, NOT
    /// cad_kernel's `WallStyle` (that's CORE, shared with the 2D app / RUST_CAD).
    pub wall_height: f32,
    /// Thickness given to promoted geometry that carries NONE of its own — a line,
    /// polyline or arc, which is what an imported or traced plan consists of. A real
    /// `Geom::Wall` still uses its own thickness.
    ///
    /// Lives here beside `wall_height` so the pair is set in one place. It previously
    /// came from the 2D wall style, which meant the 3D view had no thickness control at
    /// all — you had to open the Wall Style Manager to change it.
    pub wall_thickness: f32,
    /// Live wall records — every promoted wall, so its height stays editable after the
    /// fact (the "walls are alive" requirement). Keyed to model features by `feature_id`.
    pub walls: Vec<WallInst>,

    /// The building's levels, bottom-up. NEVER empty — a building always has at least one
    /// storey, so `active_storey` always indexes something real.
    pub storeys: Vec<Storey>,
    /// Which storey new geometry is built on. Always a valid index into `storeys`.
    pub active_storey: usize,

    /// Vertex handle being dragged: `(wall index, vertex index)`. `None` = not dragging.
    /// Held across frames because a drag spans many, and the wall's feature ids change
    /// under it on every step (`rederive_wall`) — the WALL index is stable, the ids are not.
    pub wall_drag: Option<(usize, usize)>,

    /// Move-gizmo handle being dragged, plus the anchor for absolute (Free) dragging: the
    /// selection centre and the ground point grabbed when the drag began. `None` = idle.
    pub gizmo_drag: Option<GizmoHandle>,
    pub gizmo_grab_ground: Option<Vec3>,
    pub gizmo_start_center: Vec3,

    /// Which manipulation the gizmo performs (Move arms vs Rotate rings).
    pub gizmo_mode: GizmoMode,
    /// In-progress rotation-ring drag (`None` = idle).
    pub rot_drag: Option<RotDrag>,

    /// True while a dimension field in the properties panel is mid-interaction, so the
    /// whole drag/type is ONE undo step rather than one per keystroke.
    pub dim_edit_active: bool,

    /// Show the 2D drawing (the plan) as a ground-plane underlay in the 3D view — so you
    /// can see the plan you are building the 3D model from. Toggled from the panel toolbar.
    pub show_plan: bool,

    /// Feature ids that are ROOM CEILINGS — separate slab objects created by the room tool.
    /// Tracked so they can be hidden as a group without deleting them; the lighting model
    /// still contains them.
    pub ceilings: std::collections::HashSet<u32>,
    /// Feature ids DETECTED as ceiling/roof caps by GEOMETRY (a thin, horizontal slab that
    /// is the topmost cap of the model). Recomputed on every [`Self::recompute`]. This is
    /// the drift-proof backstop for [`Self::hide_ceilings`]: the hand-tracked `ceilings`
    /// set can go stale (feature ids are `max+1` and get reused across delete/undo), and a
    /// stale id hides NOTHING — the exact field failure. Geometry cannot drift, and it only
    /// ever matches a flat top cap, so walls are never sliced.
    pub ceiling_caps: std::collections::HashSet<u32>,
    /// SOLID building roofs to CLIP while hiding: `feature id → cut z`. The feature's
    /// triangles at/above the cut (its roof) are dropped; everything below (its walls) is
    /// kept, so a solid building you made over a room opens at the top instead of vanishing.
    /// Hide ceilings in the RENDER only, so you can see into rooms while the ceilings (and
    /// the lighting model) stay intact. Unlike a section cut, this hides ONLY the ceiling
    /// slabs — the surrounding roof and walls stay.
    pub hide_ceilings: bool,
    /// Cutaway (horizontal section) — hide everything above `cutaway_z` in the render.
    /// Geometric, so it ALWAYS works: it does not depend on any object being tagged a
    /// ceiling. VIEW ONLY; the model is untouched.
    pub cutaway: bool,
    pub cutaway_z: f32,
    /// Ceiling slab thickness, metres.
    pub ceiling_thickness: f32,

    /// Rubber-band box-select in progress: `(start, current)` screen points. `None` = idle.
    pub marquee: Option<(egui::Pos2, egui::Pos2)>,

    /// Imported furniture MESHES (the project library) + their PLACED instances. The
    /// library is stored in the project file so furniture can be reused later.
    pub furniture_lib: Vec<FurnitureAsset>,
    pub furniture: Vec<FurnitureInst>,
    /// The selected furniture instance, if any. Mutually exclusive with the CSG feature
    /// selection — selecting furniture clears the feature selection and vice versa, so the
    /// gizmo and properties panel always act on exactly one thing.
    pub sel_furniture: Option<usize>,

    /// Per-feature colour (Textures menu): CSG feature id → linear RGB. A feature with no
    /// entry renders in the default neutral. Furniture carries its colour on the instance.
    pub feature_color: std::collections::HashMap<u32, [f32; 3]>,
    /// Per-SURFACE colour: a flat face (a body's feature id + its world plane) → RGB. Lets
    /// the user paint one wall face rather than the whole solid. Takes priority over
    /// `feature_color` when a triangle's surface has an entry.
    pub surface_color: std::collections::HashMap<SurfaceKey, [f32; 3]>,
    /// When on, clicking a face in the 3D view PAINTS that surface with the palette colour
    /// instead of selecting the object.
    pub paint_surface_mode: bool,
    /// Last colour chosen in the Textures picker, so it persists across opens of the menu.
    pub last_pick_color: [f32; 3],

    /// BUILDING section — the storey height the structure rises to, in metres. Held on
    /// the state (not in the dialog) because it is a property of the BUILDING, so it
    /// persists across elements: set it once and every element the section opens starts
    /// at that height, the way `wall_height` already works for promoted walls.
    pub building_height: f32,

    /// ROOM properties. `room_height` is the CLEAR interior height of a carved room;
    /// `room_floor` is the slab thickness left BELOW it. A room is carved on the active
    /// storey as `[base + room_floor, base + room_floor + room_height]`, so every storey
    /// keeps its own floor — no more "thin film on storey 1, no floor above".
    pub room_height: f32,
    pub room_floor: f32,
    /// Open the room to the sky (no ceiling). Default OFF — a room has a ceiling, which is
    /// what a lighting calculation needs; turn this on only for a court/atrium open above.
    pub room_open_top: bool,

    /// Height (m) a drawn face-sketch is extruded by for Room-elements / Furniture extrude,
    /// and the depth of a Cut RECESS (a cut that stops short of going all the way through).
    pub element_height: f32,
    /// Keep the drawn shape after Extrude / Cut instead of consuming it, so you can extrude
    /// and cut the SAME outline (e.g. a recessed frame around a hole) without redrawing.
    pub keep_sketch: bool,

    /// Zoom, mirroring the 2D command. `zoom`/`z` arms `RealTime` (drag to dolly) and
    /// shows the choice menu; typing `w` switches to `Window` (a left drag rubber-bands a
    /// box that reframes on release). `zoom_drag`/`zoom_cur` are the live box corners.
    pub zoom_mode: ZoomMode,
    pub zoom_drag: Option<egui::Pos2>,
    pub zoom_cur: Option<egui::Pos2>,
    /// Camera snapshot before the last zoom, for `zoom previous`: (yaw,pitch,dist,tx,ty,tz).
    pub cam_prev: Option<[f32; 6]>,
    /// Screen-zoom status captured at the start of a real-time drag, for the recorder.
    pub zoom_rt_before: Option<String>,

    /// An in-flight 3D modifier over `selection`. This is the SAME `move` command as
    /// 2D — only the objects and the algorithm differ ("check 2d or 3d, take the right
    /// move in the background"). `cad_solid::modify` is spec-conformant + unit-tested.
    pub modify: Option<cad_solid::modify::Modify>,
    /// A 3D op waiting on its selection — the 3D twin of the app's `queued_op`.
    /// `move` with nothing picked → queue it, gather, Enter dispatches into the picks.
    pub queued: Option<cad_solid::modify::ModifyOp>,
    /// Live prompt for the running/queued 3D op.
    pub status: String,
    /// The selected features' own mesh + the selection it was built from (the cache
    /// key). Rebuilt only when the selection changes — never per frame.
    sel_mesh: SolidMesh,
    sel_key: Vec<u32>,
}

/// CARD cardinal lock on a WORLD delta: collapse the in-plane part to its dominant
/// axis, preserving the out-of-plane component (the 3D reading of the 2D H/V lock —
/// same rule `cad_solid::modify` applies internally).
fn card_lock_world(d: Vec3) -> Vec3 {
    if d.x.abs() >= d.y.abs() {
        Vec3::new(d.x, 0.0, d.z)
    } else {
        Vec3::new(0.0, d.y, d.z)
    }
}

/// Which primitive the Draw3D dialog is editing. One entry per menu item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Draw3dKind {
    Box,
    Sphere,
    Cylinder,
    Cone,
    Prism,
    Pyramid,
    Capsule,
    Torus,
    Tube,
    Ellipsoid,
}

impl Draw3dKind {
    /// Menu order — the owner's "basic 3D objects" list, minus the two that are
    /// NOT solids (Plane/Quad and Disk/Circle are 2D: that is what the sketch +
    /// plane system is for, not a CSG primitive).
    pub const ALL: [Draw3dKind; 10] = [
        Draw3dKind::Box,
        Draw3dKind::Sphere,
        Draw3dKind::Cylinder,
        Draw3dKind::Cone,
        Draw3dKind::Prism,
        Draw3dKind::Pyramid,
        Draw3dKind::Capsule,
        Draw3dKind::Torus,
        Draw3dKind::Tube,
        Draw3dKind::Ellipsoid,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Draw3dKind::Box => "Box / Cuboid",
            Draw3dKind::Sphere => "Sphere",
            Draw3dKind::Cylinder => "Cylinder",
            Draw3dKind::Cone => "Cone / Frustum",
            Draw3dKind::Prism => "Prism",
            Draw3dKind::Pyramid => "Pyramid",
            Draw3dKind::Capsule => "Capsule",
            Draw3dKind::Torus => "Torus",
            Draw3dKind::Tube => "Tube (hollow)",
            Draw3dKind::Ellipsoid => "Ellipsoid",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Draw3dKind::Box => "⬛",
            Draw3dKind::Sphere => "⬤",
            Draw3dKind::Cylinder => "⬮",
            Draw3dKind::Cone => "▲",
            Draw3dKind::Prism => "⬡",
            Draw3dKind::Pyramid => "◭",
            Draw3dKind::Capsule => "⬭",
            Draw3dKind::Torus => "◎",
            Draw3dKind::Tube => "◯",
            Draw3dKind::Ellipsoid => "⬯",
        }
    }
}

/// Render editable numeric fields for a primitive's DIMENSIONS (not its position — that is
/// the feature's placement). Returns `true` if any field changed. Type-in enabled: these
/// are `DragValue`s, so the user can click and type a number.
///
/// The Extrusion's outline is a stored profile, not a set of scalars, so only its HEIGHT
/// is editable here — the shape is fixed once drawn.
pub fn primitive_dim_fields(ui: &mut egui::Ui, p: &mut Primitive) -> bool {
    fn f(ui: &mut egui::Ui, label: &str, v: &mut f32, min: f32) -> bool {
        ui.horizontal(|ui| {
            ui.add_sized([64.0, 18.0], egui::Label::new(egui::RichText::new(label).small().weak()));
            ui.add(egui::DragValue::new(v).speed(0.02).range(min..=1e5).suffix(" m")).changed()
        })
        .inner
    }
    fn u(ui: &mut egui::Ui, label: &str, v: &mut u32, min: u32) -> bool {
        ui.horizontal(|ui| {
            ui.add_sized([64.0, 18.0], egui::Label::new(egui::RichText::new(label).small().weak()));
            ui.add(egui::DragValue::new(v).speed(1.0).range(min..=512)).changed()
        })
        .inner
    }
    let mut c = false;
    match p {
        Primitive::Box { w, d, h } => {
            c |= f(ui, "width", w, 0.001);
            c |= f(ui, "depth", d, 0.001);
            c |= f(ui, "height", h, 0.001);
        }
        Primitive::Cylinder { r, h, sides } => {
            c |= f(ui, "radius", r, 0.001);
            c |= f(ui, "height", h, 0.001);
            c |= u(ui, "sides", sides, 3);
        }
        Primitive::Sphere { r, segments, stacks } => {
            c |= f(ui, "radius", r, 0.001);
            c |= u(ui, "segments", segments, 3);
            c |= u(ui, "stacks", stacks, 2);
        }
        Primitive::Frustum { r_bottom, r_top, h, sides } => {
            c |= f(ui, "r bottom", r_bottom, 0.0);
            c |= f(ui, "r top", r_top, 0.0);
            c |= f(ui, "height", h, 0.001);
            c |= u(ui, "sides", sides, 3);
        }
        Primitive::Torus { major_r, minor_r, seg_major, seg_minor } => {
            c |= f(ui, "ring r", major_r, 0.001);
            c |= f(ui, "tube r", minor_r, 0.001);
            c |= u(ui, "seg ring", seg_major, 3);
            c |= u(ui, "seg tube", seg_minor, 3);
        }
        Primitive::Capsule { r, h, segments, stacks } => {
            c |= f(ui, "radius", r, 0.001);
            c |= f(ui, "length", h, 0.001);
            c |= u(ui, "segments", segments, 3);
            c |= u(ui, "stacks", stacks, 2);
        }
        Primitive::Tube { r_outer, r_inner, h, sides } => {
            c |= f(ui, "r outer", r_outer, 0.001);
            c |= f(ui, "r inner", r_inner, 0.0);
            c |= f(ui, "height", h, 0.001);
            c |= u(ui, "sides", sides, 3);
        }
        Primitive::Ellipsoid { rx, ry, rz, segments, stacks } => {
            c |= f(ui, "rx", rx, 0.001);
            c |= f(ui, "ry", ry, 0.001);
            c |= f(ui, "rz", rz, 0.001);
            c |= u(ui, "segments", segments, 3);
            c |= u(ui, "stacks", stacks, 2);
        }
        Primitive::Extrusion { h, .. } => {
            c |= f(ui, "height", h, 0.001);
            ui.label(egui::RichText::new("  outline shape is fixed").small().weak());
        }
    }
    c
}

/// Which manipulation the on-screen gizmo performs. Toggled from the 3D Factory bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GizmoMode {
    /// Translate arms + centre free-move (the original gizmo).
    #[default]
    Move,
    /// Three rotation rings (one per axis) — drag a ring to spin about that axis.
    Rotate,
}

/// One draggable handle of the gizmo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GizmoHandle {
    /// Constrain the move to the world X / Y / Z axis.
    X,
    Y,
    Z,
    /// Free move — the centre cube where the three arms meet; slides the object across the
    /// ground plane (a combination of X and Y at once).
    Free,
    /// Rotation ring about axis 0 / 1 / 2 (world axes for furniture, plane-local for a
    /// feature). Coloured red / green / blue like the move axes.
    RotX,
    RotY,
    RotZ,
}

impl GizmoHandle {
    /// World-axis direction, or `None` for Free.
    pub fn axis(self) -> Option<Vec3> {
        match self {
            GizmoHandle::X | GizmoHandle::RotX => Some(Vec3::X),
            GizmoHandle::Y | GizmoHandle::RotY => Some(Vec3::Y),
            GizmoHandle::Z | GizmoHandle::RotZ => Some(Vec3::Z),
            GizmoHandle::Free => None,
        }
    }

    /// Which of the three axes (0/1/2) this handle rotates about, if it's a ring.
    pub fn ring_axis(self) -> Option<usize> {
        match self {
            GizmoHandle::RotX => Some(0),
            GizmoHandle::RotY => Some(1),
            GizmoHandle::RotZ => Some(2),
            _ => None,
        }
    }

    /// Axis colour: X red, Y green, Z blue — the universal convention.
    pub fn color(self) -> egui::Color32 {
        match self {
            GizmoHandle::X | GizmoHandle::RotX => egui::Color32::from_rgb(235, 80, 80),
            GizmoHandle::Y | GizmoHandle::RotY => egui::Color32::from_rgb(90, 210, 90),
            GizmoHandle::Z | GizmoHandle::RotZ => egui::Color32::from_rgb(90, 150, 245),
            GizmoHandle::Free => egui::Color32::from_rgb(230, 230, 230),
        }
    }
}

/// In-progress rotation-ring drag. Captured on grab so the whole gesture is one undo step
/// and the rotation is measured relative to where you first grabbed the ring.
#[derive(Clone, Copy, Debug)]
pub struct RotDrag {
    pub handle: GizmoHandle,
    /// Unit rotation axis in WORLD space (a world axis for furniture; a plane-local axis for
    /// a feature).
    pub axis: Vec3,
    pub center: Vec3,
    /// Reference vector (in the rotation plane) where the grab started.
    pub r0: Vec3,
    /// Start rotation of the target: furniture Euler `[x,y,z]°`, or feature `[pitch,roll,spin]°`.
    pub start_rot: [f32; 3],
    /// For a feature, which placement angle (0=pitch,1=roll,2=spin) this ring drives.
    pub feat_axis: usize,
    pub is_furniture: bool,
}

/// One projected arm of the gizmo.
pub struct GizmoArm {
    pub handle: GizmoHandle,
    pub dir: Vec3,
    pub tip_s: egui::Pos2,
}

/// The gizmo projected to screen space for one frame. Arms are `Option` because an arm
/// tip can fall behind the camera; a `None` arm is simply not drawn or picked.
pub struct GizmoView {
    pub center_w: Vec3,
    pub center_s: egui::Pos2,
    pub len_w: f32,
    pub arms: [Option<GizmoArm>; 3],
}

/// One rotation ring projected to screen: the axis it spins about + its screen polyline.
pub struct Ring {
    pub handle: GizmoHandle,
    pub axis: Vec3,           // world-space unit rotation axis
    pub pts: Vec<egui::Pos2>, // projected circle (screen space)
}

/// The rotation gizmo projected to screen for one frame — three rings + centre.
pub struct RingView {
    pub center: Vec3,
    pub center_s: egui::Pos2,
    pub radius: f32,
    pub rings: Vec<Ring>,
    pub is_furniture: bool,
}

/// Gizmo sizing / pick tolerances (pixels).
const GIZMO_MIN_PX: f32 = 65.0;   // shortest on-screen arm length, so tiny objects stay grabbable
const GIZMO_AXIS_PICK: f32 = 8.0;
const GIZMO_CUBE_PICK: f32 = 9.0;

/// Distance from a point to a line segment, in screen space. Public alias for the app's
/// gizmo hover test.
pub fn seg_dist(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    dist_point_segment(p, a, b)
}

/// Distance from a point to a line segment, in screen space.
fn dist_point_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    let proj = a + ab * t;
    p.distance(proj)
}

/// Radius a vertex handle is drawn at, and the two pick apertures. The edge aperture is
/// deliberately smaller: a midpoint lies between two vertex handles, so a close call must
/// go to the vertex — otherwise dragging a corner would insert a point instead.
pub const HANDLE_DRAW_R: f32 = 4.5;
const HANDLE_PICK_R: f32 = 10.0;
const EDGE_PICK_R: f32 = 7.0;

/// Project a world point to screen. `None` when it falls outside the depth range (behind
/// the camera), so nothing is drawn or picked where the user cannot see it.
pub fn world_to_screen(w: Vec3, rect: egui::Rect, mvp: &[f32; 16]) -> Option<egui::Pos2> {
    let ndc = Mat4::from_cols_array(mvp).project_point3(w);
    if !(-1.0..=1.0).contains(&ndc.z) {
        return None;
    }
    Some(egui::pos2(
        rect.left() + (ndc.x * 0.5 + 0.5) * rect.width(),
        rect.top() + (0.5 - ndc.y * 0.5) * rect.height(),
    ))
}

/// Nearest candidate within `aperture` pixels of `cursor`.
fn nearest_within(
    items: Vec<(usize, egui::Pos2)>, cursor: egui::Pos2, aperture: f32,
) -> Option<usize> {
    items
        .into_iter()
        .map(|(i, p)| (p.distance(cursor), i))
        .filter(|(d, _)| *d <= aperture)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i)| i)
}

/// Why a room could not be carved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoomError {
    /// No solid to cut from — make a building first.
    NoBuilding,
    /// The outline itself was invalid (too few points / no area / self-crossing).
    Profile(cad_solid::ProfileError),
}


/// Identifies a flat SURFACE for per-face colouring: the body's feature id plus its world
/// plane, quantised (normal ×50, offset ×100) so all coplanar triangles of one face share
/// a key. Stable while the object doesn't move; a moved object is simply re-painted.
pub type SurfaceKey = (u32, i32, i32, i32, i32);

/// Compute a triangle's [`SurfaceKey`] from its face id and its three world positions.
pub fn surface_key(face_id: u32, a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> SurfaceKey {
    let av = Vec3::from(a);
    let n = (Vec3::from(b) - av).cross(Vec3::from(c) - av).normalize_or_zero();
    let d = n.dot(av);
    (
        face_id,
        (n.x * 50.0).round() as i32,
        (n.y * 50.0).round() as i32,
        (n.z * 50.0).round() as i32,
        (d * 100.0).round() as i32,
    )
}

/// Tolerance for "is this feature on that storey" / "is this wall at that base". Floor
/// heights are summed f32s, so an exact `==` would miss by an ulp after a few levels.
const Z_EPS: f32 = 1e-4;

/// Floor-to-floor heights below this are rejected — a zero-height storey has no z band,
/// so nothing could ever be assigned to it.
const MIN_STOREY_H: f32 = 0.1;

/// An imported furniture MESH held in the project library. Stored once; placed as many
/// times as you like via [`FurnitureInst`]. Triangle soup (positions/normals, 3 per tri).
#[derive(Clone, Debug, Default)]
pub struct FurnitureAsset {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Default colour for new instances — the file's diffuse if it had one, else a neutral
    /// light grey (NOT the old tan). The user can recolour per instance afterward.
    pub color: [f32; 3],
}

/// One PLACED copy of a library asset.
#[derive(Clone, Copy, Debug)]
pub struct FurnitureInst {
    /// Index into [`FactoryState::furniture_lib`].
    pub asset: usize,
    pub pos: [f32; 3],
    pub scale: f32,
    /// Euler rotation in DEGREES about the world X, Y, Z axes (applied X→Y→Z), pivoting
    /// on the instance's base-centre. `rot[2]` is the old yaw (Z), so upgrades are clean.
    pub rot: [f32; 3],
    /// Linear RGB, applied in the Textures menu (default a warm neutral).
    pub color: [f32; 3],
}

impl FurnitureInst {
    /// World rotation matrix (X→Y→Z Euler). Applied to a scaled local point/normal.
    pub fn rot_mat(&self) -> glam::Mat3 {
        glam::Mat3::from_euler(
            glam::EulerRot::XYZ,
            self.rot[0].to_radians(),
            self.rot[1].to_radians(),
            self.rot[2].to_radians(),
        )
    }
}

/// One level of the building.
///
/// **`base_z` is deliberately NOT stored** — it is derived by summing the heights of the
/// storeys below (see [`FactoryState::storey_base_z`]). Storing it would let the stack
/// drift out of contiguity: change one height and every stored base below it becomes a
/// lie that nothing forces you to fix. Derived, the stack is contiguous by construction.
///
/// A storey likewise does not hold a list of the features on it. Membership is derived
/// from the z band (see [`FactoryState::features_on_storey`]) because `rederive_wall`
/// drops and rebuilds a wall's Boxes with FRESH ids — any stored id list would silently
/// rot on the first vertex edit.
#[derive(Clone, Debug, PartialEq)]
pub struct Storey {
    pub name: String,
    /// Floor-to-floor height, metres. Always > 0.
    pub height: f32,
}

/// The live parameter set for the Draw3D dialog.
///
/// ONE struct holds every primitive's controllers (rather than one per shape) so
/// switching kinds keeps what you already typed — set a radius on Cylinder, switch
/// to Cone, and the radius carries over. Fields are named for the CONTROLLER, and
/// several are deliberately shared across shapes (`r`, `h`, `segments`).
#[derive(Clone, Debug)]
pub struct Draw3dDialog {
    pub kind: Draw3dKind,
    // lengths
    pub w: f32,
    pub d: f32,
    pub h: f32,
    pub r: f32,
    pub r_top: f32,
    pub r_inner: f32,
    pub major_r: f32,
    pub minor_r: f32,
    pub rx: f32,
    pub ry: f32,
    pub rz: f32,
    // tessellation (accuracy controllers)
    pub segments: u32,
    pub stacks: u32,
    pub sides: u32,
    pub seg_major: u32,
    pub seg_minor: u32,
}

impl Default for Draw3dDialog {
    fn default() -> Self {
        Self {
            kind: Draw3dKind::Box,
            w: 2.0,
            d: 2.0,
            h: 1.0,
            r: 1.0,
            r_top: 0.0,
            r_inner: 0.6,
            major_r: 2.0,
            minor_r: 0.5,
            rx: 1.0,
            ry: 1.5,
            rz: 0.75,
            segments: 32,
            stacks: 16,
            sides: 6,
            seg_major: 32,
            seg_minor: 16,
        }
    }
}

impl Draw3dDialog {
    pub fn new(kind: Draw3dKind) -> Self {
        Self { kind, ..Default::default() }
    }

    /// Load the controllers FROM an existing primitive — the inverse of `build()` — so
    /// selecting a solid shows its real dimensions. The Frustum family (cone / prism /
    /// pyramid / frustum) is disambiguated the same way `Primitive::kind_label` does,
    /// by `r_top` and `sides`. Fields are set to match how `build()` reads them (e.g.
    /// cone/cylinder/tube take their facet count from `segments`, prism/pyramid from
    /// `sides`), so a load→build round-trip is stable.
    pub fn load_from(&mut self, p: &Primitive) {
        match *p {
            // An extrusion has no controllers here — its shape is a stored profile, not a
            // set of scalars. It is never bound for editing (see the edit-binding guard in
            // `app.rs`), so this arm exists only for exhaustiveness and must not touch the
            // dialog: writing a kind here would misreport the solid.
            Primitive::Extrusion { .. } => {}
            Primitive::Box { w, d, h } => {
                self.kind = Draw3dKind::Box;
                self.w = w; self.d = d; self.h = h;
            }
            Primitive::Sphere { r, segments, stacks } => {
                self.kind = Draw3dKind::Sphere;
                self.r = r; self.segments = segments; self.stacks = stacks;
            }
            Primitive::Cylinder { r, h, sides } => {
                self.kind = Draw3dKind::Cylinder;
                self.r = r; self.h = h; self.segments = sides;
            }
            Primitive::Frustum { r_bottom, r_top, h, sides } => {
                self.r = r_bottom; self.r_top = r_top; self.h = h;
                self.sides = sides; self.segments = sides;
                self.kind = if r_top <= 1e-6 {
                    if sides == 4 { Draw3dKind::Pyramid } else { Draw3dKind::Cone }
                } else if (r_top - r_bottom).abs() <= 1e-6 {
                    Draw3dKind::Prism
                } else {
                    Draw3dKind::Cone // a true frustum edits via the cone controllers (bottom/top/height)
                };
            }
            Primitive::Torus { major_r, minor_r, seg_major, seg_minor } => {
                self.kind = Draw3dKind::Torus;
                self.major_r = major_r; self.minor_r = minor_r;
                self.seg_major = seg_major; self.seg_minor = seg_minor;
            }
            Primitive::Capsule { r, h, segments, stacks } => {
                self.kind = Draw3dKind::Capsule;
                self.r = r; self.h = h; self.segments = segments; self.stacks = stacks;
            }
            Primitive::Tube { r_outer, r_inner, h, sides } => {
                self.kind = Draw3dKind::Tube;
                self.r = r_outer; self.r_inner = r_inner; self.h = h; self.segments = sides;
            }
            Primitive::Ellipsoid { rx, ry, rz, segments, stacks } => {
                self.kind = Draw3dKind::Ellipsoid;
                self.rx = rx; self.ry = ry; self.rz = rz;
                self.segments = segments; self.stacks = stacks;
            }
        }
    }

    /// Build the primitive from the current controllers.
    ///
    /// Cone / Prism / Pyramid all map onto ONE `Primitive::Frustum` — they are the
    /// same solid with different controllers (`r_top = 0` → cone; `r_top = r` →
    /// prism; 4 sides + `r_top = 0` → pyramid). Keeping them as separate MENU items
    /// but one primitive is why there is no duplicated meshing code.
    pub fn build(&self) -> Primitive {
        match self.kind {
            Draw3dKind::Box => Primitive::Box { w: self.w, d: self.d, h: self.h },
            Draw3dKind::Sphere => {
                Primitive::Sphere { r: self.r, segments: self.segments, stacks: self.stacks }
            }
            Draw3dKind::Cylinder => {
                Primitive::Cylinder { r: self.r, h: self.h, sides: self.segments }
            }
            Draw3dKind::Cone => Primitive::Frustum {
                r_bottom: self.r,
                r_top: self.r_top,
                h: self.h,
                sides: self.segments,
            },
            Draw3dKind::Prism => Primitive::Frustum {
                r_bottom: self.r,
                r_top: self.r, // equal radii ⇒ a prism
                h: self.h,
                sides: self.sides,
            },
            Draw3dKind::Pyramid => Primitive::Frustum {
                r_bottom: self.r,
                r_top: 0.0, // apex
                h: self.h,
                sides: self.sides,
            },
            Draw3dKind::Capsule => Primitive::Capsule {
                r: self.r,
                h: self.h,
                segments: self.segments,
                stacks: self.stacks,
            },
            Draw3dKind::Torus => Primitive::Torus {
                major_r: self.major_r,
                minor_r: self.minor_r,
                seg_major: self.seg_major,
                seg_minor: self.seg_minor,
            },
            Draw3dKind::Tube => Primitive::Tube {
                r_outer: self.r,
                r_inner: self.r_inner,
                h: self.h,
                sides: self.segments,
            },
            Draw3dKind::Ellipsoid => Primitive::Ellipsoid {
                rx: self.rx,
                ry: self.ry,
                rz: self.rz,
                segments: self.segments,
                stacks: self.stacks,
            },
        }
    }

    /// Validity + the reason, shown live in the dialog so Create is never a
    /// guess (e.g. a tube whose bore is wider than its wall isn't a tube).
    pub fn problem(&self) -> Option<&'static str> {
        match self.kind {
            Draw3dKind::Tube if self.r_inner >= self.r => {
                Some("inner radius must be smaller than outer")
            }
            Draw3dKind::Torus if self.minor_r >= self.major_r => {
                Some("minor radius must be smaller than major (else it self-intersects)")
            }
            Draw3dKind::Cone if self.r_top >= self.r => Some("top radius must be < bottom (0 = cone)"),
            _ => None,
        }
    }
}

impl Default for FactoryState {
    fn default() -> Self {
        Self {
            open: false,
            model: Model::default(),
            cached: SolidMesh::default(),
            dirty: false,
            selection: Vec::new(),
            cam_yaw: 0.9,
            cam_pitch: 0.5,
            cam_dist: 12.0,
            cam_target: [0.0, 0.0, 0.0],
            ortho: false,
            session: None,
            pending_face: None,
            sketch_ref: Vec::new(),
            box_w: 2.0,
            box_d: 2.0,
            box_h: 1.0,
            cyl_r: 0.5,
            cyl_h: 2.0,
            cyl_sides: 24,
            draw3d: None,
            draw3d_edit: None,
            place_pending: None,
            wall_height: 2.7,
            wall_thickness: 0.2,
            walls: Vec::new(),
            building_height: 3.0,
            room_height: 2.7,
            room_floor: 0.2,
            room_open_top: false,
            element_height: 1.0,
            keep_sketch: false,
            // One storey at z = 0 — with a single level everything behaves exactly as it
            // did before storeys existed.
            storeys: vec![Storey { name: "Ground".into(), height: 3.0 }],
            active_storey: 0,
            wall_drag: None,
            gizmo_drag: None,
            gizmo_grab_ground: None,
            gizmo_start_center: Vec3::ZERO,
            gizmo_mode: GizmoMode::Move,
            rot_drag: None,
            dim_edit_active: false,
            show_plan: true,
            ceilings: std::collections::HashSet::new(),
            ceiling_caps: std::collections::HashSet::new(),
            hide_ceilings: false,
            cutaway: false,
            cutaway_z: 2.5,
            ceiling_thickness: 0.15,
            marquee: None,
            furniture_lib: Vec::new(),
            furniture: Vec::new(),
            sel_furniture: None,
            feature_color: std::collections::HashMap::new(),
            surface_color: std::collections::HashMap::new(),
            paint_surface_mode: false,
            last_pick_color: [0.8, 0.8, 0.82],
            zoom_mode: ZoomMode::Off,
            zoom_drag: None,
            zoom_cur: None,
            cam_prev: None,
            zoom_rt_before: None,
            modify: None,
            queued: None,
            status: String::new(),
            sel_mesh: SolidMesh::default(),
            sel_key: Vec::new(),
        }
    }
}

impl FactoryState {
    pub fn add_box(&mut self) {
        let p = Primitive::Box { w: self.box_w, d: self.box_d, h: self.box_h };
        // Built on the ACTIVE storey, like every other new solid.
        let placement = Placement { lift: self.active_base_z(), ..Placement::default() };
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.selection = vec![id];
        self.dirty = true;
    }

    // ===================================================================
    // Storeys — the building's levels
    // ===================================================================

    /// Z the floor of storey `i` sits at: the sum of the heights below it. Derived, so
    /// the stack is contiguous by construction and cannot drift.
    pub fn storey_base_z(&self, i: usize) -> f32 {
        self.storeys.iter().take(i).map(|s| s.height).sum()
    }

    /// Z that new geometry is built on.
    pub fn active_base_z(&self) -> f32 {
        self.storey_base_z(self.active_storey.min(self.storeys.len().saturating_sub(1)))
    }

    /// Total height of the building.
    pub fn building_total_height(&self) -> f32 {
        self.storeys.iter().map(|s| s.height).sum()
    }

    /// Feature ids whose origin lies in storey `i`'s z band — `[base, base + height)`,
    /// with the TOP storey's band closed at the top so geometry standing exactly on the
    /// roof line still belongs to something.
    ///
    /// Derived rather than tracked: see [`Storey`].
    pub fn features_on_storey(&self, i: usize) -> Vec<u32> {
        if i >= self.storeys.len() {
            return Vec::new();
        }
        let base = self.storey_base_z(i);
        let top = base + self.storeys[i].height;
        let is_top = i + 1 == self.storeys.len();
        self.model
            .features
            .iter()
            .filter(|f| {
                let z = f.world_origin().z;
                z >= base - Z_EPS && (z < top - Z_EPS || (is_top && z <= top + Z_EPS))
            })
            .map(|f| f.id)
            .collect()
    }

    /// Move every feature whose origin is at or above `from_z` by `dz`, and carry the
    /// walls' `base_z` with them. Used when a storey is inserted, deleted or resized —
    /// everything above has to follow, or the stack tears apart.
    fn shift_above(&mut self, from_z: f32, dz: f32) {
        if dz == 0.0 {
            return;
        }
        let ids: Vec<u32> = self
            .model
            .features
            .iter()
            .filter(|f| f.world_origin().z >= from_z - Z_EPS)
            .map(|f| f.id)
            .collect();
        for id in ids {
            if let Some(f) = self.model.get_mut(id) {
                *f = f.translated(Vec3::new(0.0, 0.0, dz));
            }
        }
        for w in &mut self.walls {
            if w.base_z >= from_z - Z_EPS {
                w.base_z += dz;
            }
        }
        self.dirty = true;
    }

    /// Add a storey directly above storey `i` and make it active. Everything above `i`
    /// moves up by the new storey's height so nothing is left overlapping.
    pub fn insert_storey_above(&mut self, i: usize, name: String, height: f32) -> usize {
        let h = height.max(MIN_STOREY_H);
        let i = i.min(self.storeys.len().saturating_sub(1));
        let top_of_i = self.storey_base_z(i) + self.storeys[i].height;
        self.shift_above(top_of_i, h);
        self.storeys.insert(i + 1, Storey { name, height: h });
        self.active_storey = i + 1;
        i + 1
    }

    /// Append a storey on top of the building and make it active.
    pub fn add_storey_on_top(&mut self) -> usize {
        let n = self.storeys.len();
        let h = self.storeys.last().map_or(self.building_height, |s| s.height);
        self.storeys.push(Storey { name: format!("Level {n}"), height: h.max(MIN_STOREY_H) });
        self.active_storey = self.storeys.len() - 1;
        self.active_storey
    }

    /// Duplicate the ACTIVE storey's geometry onto a new level directly above it — "add a
    /// floor to my building". Copies the buildings, walls and solids standing on the level
    /// (not slabs, which belong to the level beneath) up by the storey height, and makes
    /// the new level active. Returns the new storey index, or `None` if the level is empty.
    pub fn duplicate_storey_up(&mut self) -> Option<usize> {
        let src = self.active_storey.min(self.storeys.len().saturating_sub(1));
        let base_src = self.storey_base_z(src);
        let feat_ids = self.features_on_storey(src);
        let src_walls: Vec<WallInst> = self
            .walls
            .iter()
            .filter(|w| (w.base_z - base_src).abs() < Z_EPS)
            .cloned()
            .collect();
        if feat_ids.is_empty() && src_walls.is_empty() {
            return None;
        }
        let dz = self.storeys[src].height;
        // A new level above src (shifts anything higher up to make room).
        let dst = self.insert_storey_above(src, format!("Level {}", self.storeys.len()), dz);
        let new_base = self.storey_base_z(dst);

        // Copy the solids up.
        let feats: Vec<Feature> = feat_ids
            .iter()
            .filter_map(|id| self.model.features.iter().find(|f| f.id == *id).cloned())
            .collect();
        for f in feats {
            let nf = f.translated(Vec3::new(0.0, 0.0, dz));
            self.model.push_feature(nf);
        }
        // Copy the walls up (fresh segment ids, new base).
        for w in src_walls {
            let mut segs = Vec::new();
            for win in w.footprint.windows(2) {
                if let Some(id) = self.push_wall_box(win[0], win[1], w.thickness, w.height, new_base) {
                    segs.push(id);
                }
            }
            if !segs.is_empty() {
                self.walls.push(WallInst {
                    footprint: w.footprint.clone(),
                    segments: segs,
                    thickness: w.thickness,
                    height: w.height,
                    rake_deg: w.rake_deg,
                    base_z: new_base,
                });
            }
        }
        self.active_storey = dst;
        self.dirty = true;
        Some(dst)
    }

    /// Delete storey `i`, ERASING the geometry standing on it and dropping everything
    /// above down by its height. Returns false — changing nothing — when this is the last
    /// storey: a building always has at least one level, and an empty `storeys` would
    /// make `active_storey` index nothing.
    pub fn delete_storey(&mut self, i: usize) -> bool {
        if self.storeys.len() <= 1 || i >= self.storeys.len() {
            return false;
        }
        for id in self.features_on_storey(i) {
            self.model.remove(id);
        }
        let base = self.storey_base_z(i);
        let h = self.storeys[i].height;
        // Walls that stood on the deleted level go with it. Walls above are left alone
        // here — `shift_above` below brings them down.
        self.walls.retain(|w| (w.base_z - base).abs() > Z_EPS);
        self.storeys.remove(i);
        self.shift_above(base + h, -h);
        self.active_storey = self.active_storey.min(self.storeys.len() - 1);
        self.clear_selection();
        self.dirty = true;
        true
    }

    /// Change storey `i`'s height. Everything ABOVE it moves by the difference so the
    /// stack stays contiguous; the geometry ON the storey keeps its own height (a taller
    /// level does not stretch its walls).
    pub fn set_storey_height(&mut self, i: usize, height: f32) {
        if i >= self.storeys.len() {
            return;
        }
        let h = height.max(MIN_STOREY_H);
        let old = self.storeys[i].height;
        if (h - old).abs() < f32::EPSILON {
            return;
        }
        let top = self.storey_base_z(i) + old;
        self.storeys[i].height = h;
        self.shift_above(top, h - old);
    }

    // ===================================================================
    // Furniture — imported OBJ meshes
    // ===================================================================

    /// Add a parsed OBJ mesh to the project library. Auto-normalises very large or very
    /// small meshes toward a ~1 m size (many OBJ exports use cm or mm), and re-seats it so
    /// its base sits on z = 0. Returns the library index.
    pub fn add_furniture_asset(&mut self, name: String, mesh: crate::mesh_io::ObjMesh) -> usize {
        let asset_color = mesh.color.unwrap_or([0.82, 0.82, 0.84]); // file diffuse, else neutral
        let mut positions = mesh.positions;
        let normals = mesh.normals;
        let bounds = {
            let mut mn = [f32::INFINITY; 3];
            let mut mx = [f32::NEG_INFINITY; 3];
            for p in &positions {
                for k in 0..3 { mn[k] = mn[k].min(p[k]); mx[k] = mx[k].max(p[k]); }
            }
            mn[0].is_finite().then_some((mn, mx))
        };
        if let Some((mn, mx)) = bounds {
            let size = [(mx[0] - mn[0]), (mx[1] - mn[1]), (mx[2] - mn[2])];
            let longest = size[0].max(size[1]).max(size[2]).max(1e-4);
            // Scale toward ~1.5 m only for wildly off sizes (cm/mm exports, or giant units).
            let k = if longest > 20.0 || longest < 0.05 { 1.5 / longest } else { 1.0 };
            for p in &mut positions {
                p[0] = (p[0] - (mn[0] + mx[0]) * 0.5) * k; // centre X
                p[1] = (p[1] - (mn[1] + mx[1]) * 0.5) * k; // centre Y
                p[2] = (p[2] - mn[2]) * k;                  // base on z = 0
            }
        }
        self.furniture_lib.push(FurnitureAsset { name, positions, normals, color: asset_color });
        self.furniture_lib.len() - 1
    }

    /// Place a copy of library asset `asset` at world point `at` (seated on the plane),
    /// and select it so it can be moved/scaled immediately.
    pub fn place_furniture(&mut self, asset: usize, at: Vec3) {
        if asset >= self.furniture_lib.len() {
            return;
        }
        let color = self.furniture_lib[asset].color;
        self.furniture.push(FurnitureInst {
            asset,
            pos: [at.x, at.y, self.active_base_z()],
            scale: 1.0,
            rot: [0.0, 0.0, 0.0],
            color,
        });
        self.select_furniture(self.furniture.len() - 1);
    }

    /// World-space vertex of instance `i`'s local mesh point — pose applied
    /// (scale → 3-axis rotate → translate).
    fn furniture_point(&self, inst: &FurnitureInst, p: [f32; 3]) -> Vec3 {
        let lp = Vec3::new(p[0] * inst.scale, p[1] * inst.scale, p[2] * inst.scale);
        inst.rot_mat() * lp + Vec3::from(inst.pos)
    }

    /// World AABB of a placed furniture instance.
    pub fn furniture_aabb(&self, i: usize) -> Option<(Vec3, Vec3)> {
        let inst = self.furniture.get(i)?;
        let asset = self.furniture_lib.get(inst.asset)?;
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for p in &asset.positions {
            let w = self.furniture_point(inst, *p);
            mn = mn.min(w);
            mx = mx.max(w);
        }
        mn.x.is_finite().then_some((mn, mx))
    }

    /// Ray-pick the front-most furniture instance under the cursor.
    pub fn pick_furniture(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<usize> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let mut best: Option<(f32, usize)> = None;
        for (i, inst) in self.furniture.iter().enumerate() {
            let Some(asset) = self.furniture_lib.get(inst.asset) else { continue };
            let mut ft: Option<f32> = None;
            for tri in asset.positions.chunks_exact(3) {
                let a = self.furniture_point(inst, tri[0]);
                let b = self.furniture_point(inst, tri[1]);
                let c = self.furniture_point(inst, tri[2]);
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                    if ft.map_or(true, |x| t < x) {
                        ft = Some(t);
                    }
                }
            }
            if let Some(t) = ft {
                if best.map_or(true, |(bt, _)| t < bt) {
                    best = Some((t, i));
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// Is furniture instance `fi` in front of feature `id` along the pick ray? Used to
    /// break a tie when a click hits both.
    pub fn furniture_nearer_than_feature(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16], fi: usize, id: u32,
    ) -> bool {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let fur_t = self.furniture.get(fi).and_then(|inst| {
            let asset = self.furniture_lib.get(inst.asset)?;
            let mut best: Option<f32> = None;
            for tri in asset.positions.chunks_exact(3) {
                let a = self.furniture_point(inst, tri[0]);
                let b = self.furniture_point(inst, tri[1]);
                let c = self.furniture_point(inst, tri[2]);
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                    if best.map_or(true, |x| t < x) { best = Some(t); }
                }
            }
            best
        });
        let feat_t = self.model.features.iter().find(|f| f.id == id).and_then(|f| {
            let tris = self.model.feature_world_positions(f);
            let mut best: Option<f32> = None;
            for c in tris.chunks_exact(3) {
                let (a, b, cc) = (Vec3::from(c[0]), Vec3::from(c[1]), Vec3::from(c[2]));
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, cc) {
                    if best.map_or(true, |x| t < x) { best = Some(t); }
                }
            }
            best
        });
        match (fur_t, feat_t) {
            (Some(a), Some(b)) => a <= b,
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Select a furniture instance — clears the CSG feature selection (they are mutually
    /// exclusive, so exactly one thing is edited).
    pub fn select_furniture(&mut self, i: usize) {
        if i < self.furniture.len() {
            self.sel_furniture = Some(i);
            self.selection.clear();
            self.sel_key.clear();
        }
    }

    /// True when anything is selected — a feature OR a furniture instance. The gizmo and
    /// properties panel key off this.
    pub fn has_any_selection(&self) -> bool {
        !self.selection.is_empty() || self.sel_furniture.is_some()
    }

    // ===================================================================
    // Selection: bounds, move, delete, per-object properties
    // ===================================================================

    /// The single selected feature id, if exactly one solid is selected. Position and
    /// dimension editing act on ONE object; a multi-selection has no single set of
    /// dimensions.
    pub fn selected_single(&self) -> Option<u32> {
        match self.selection.as_slice() {
            [id] if self.model.features.iter().any(|f| f.id == *id) => Some(*id),
            _ => None,
        }
    }

    /// World AABB of the current selection — a furniture instance if one is selected,
    /// otherwise every selected feature. Drives the gizmo size and centre.
    pub fn selection_aabb(&self) -> Option<(Vec3, Vec3)> {
        if let Some(i) = self.sel_furniture {
            return self.furniture_aabb(i);
        }
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        let mut any = false;
        for &id in &self.selection {
            if let Some(f) = self.model.features.iter().find(|f| f.id == id) {
                let (a, b) = f.world_aabb();
                mn = mn.min(a);
                mx = mx.max(b);
                any = true;
            }
        }
        any.then_some((mn, mx))
    }

    /// Geometric centre of the selection.
    pub fn selection_center(&self) -> Option<Vec3> {
        self.selection_aabb().map(|(mn, mx)| (mn + mx) * 0.5)
    }

    /// World AABB of the whole model (every feature). `None` when there are no features.
    /// Used by the room tool to size a void against the ACTUAL building, not a UI default.
    pub fn features_aabb(&self) -> Option<(Vec3, Vec3)> {
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for f in &self.model.features {
            let (a, b) = f.world_aabb();
            mn = mn.min(a);
            mx = mx.max(b);
        }
        (mn.x.is_finite()).then_some((mn, mx))
    }

    /// Select every feature whose projected centre falls inside a screen rectangle —
    /// rubber-band box-select. `additive` keeps the existing selection (Shift-drag).
    ///
    /// Centre-in-box is the intuitive rule for a box-select: an object counts as picked
    /// when its middle is inside the band, so a small overlap at the edge doesn't grab it.
    pub fn select_in_marquee(
        &mut self, band: egui::Rect, viewport: egui::Rect, mvp: &[f32; 16], additive: bool,
    ) {
        let mut hits = Vec::new();
        for f in &self.model.features {
            let (mn, mx) = f.world_aabb();
            if let Some(s) = world_to_screen((mn + mx) * 0.5, viewport, mvp) {
                if band.contains(s) {
                    hits.push(f.id);
                }
            }
        }
        if !additive {
            self.selection.clear();
        }
        for id in hits {
            if !self.selection.contains(&id) {
                self.selection.push(id);
            }
        }
        self.sel_key.clear();
    }

    /// Move the current selection by a world delta — the selected furniture instance, or
    /// every selected feature. In place, so the selection (and the gizmo) survives.
    pub fn move_selection(&mut self, delta: Vec3) {
        if delta.length_squared() < 1e-12 {
            return;
        }
        if let Some(i) = self.sel_furniture {
            if let Some(inst) = self.furniture.get_mut(i) {
                inst.pos[0] += delta.x;
                inst.pos[1] += delta.y;
                inst.pos[2] += delta.z;
            }
            return; // furniture is not part of the CSG model — nothing to re-eval
        }
        for &id in &self.selection.clone() {
            if let Some(f) = self.model.get_mut(id) {
                *f = f.translated(delta);
            }
        }
        self.dirty = true;
    }

    /// Uniformly scale the current selection about its own centre — furniture instance or
    /// a single feature. `factor` multiplies the current size (1.0 = no change).
    pub fn scale_selection(&mut self, factor: f32) {
        let k = factor.clamp(0.02, 50.0);
        if (k - 1.0).abs() < 1e-4 {
            return;
        }
        if let Some(i) = self.sel_furniture {
            if let Some(inst) = self.furniture.get_mut(i) {
                inst.scale = (inst.scale * k).clamp(0.001, 1000.0);
            }
            return;
        }
        // A single feature: scale its primitive about its own centre.
        if let Some(id) = self.selected_single() {
            if let Some(f) = self.model.features.iter().find(|f| f.id == id).cloned() {
                let (mn, mx) = f.world_aabb();
                let pivot = (mn + mx) * 0.5;
                if let Some(fm) = self.model.get_mut(id) {
                    *fm = f.scaled(pivot, k);
                }
                self.dirty = true;
            }
        }
    }

    /// Paint the SURFACE (coplanar face) under the cursor with `color`. Ray-tests the
    /// cached mesh, finds the front-most triangle, and colours every triangle sharing its
    /// surface key. Returns true if a surface was hit.
    pub fn paint_surface(
        &mut self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16], color: [f32; 3],
    ) -> bool {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let mut best: Option<(f32, SurfaceKey)> = None;
        for (i, tri) in self.cached.positions.chunks_exact(3).enumerate() {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, c) {
                if best.map_or(true, |(bt, _)| t < bt) {
                    let fid = self.cached.face_ids.get(i).copied().unwrap_or(0);
                    best = Some((t, surface_key(fid, tri[0], tri[1], tri[2])));
                }
            }
        }
        if let Some((_, key)) = best {
            self.surface_color.insert(key, color);
            return true;
        }
        false
    }

    /// Set a feature's world origin directly — the Position fields in the properties
    /// panel. `axis` 0/1/2 = x/y/z.
    pub fn set_feature_origin_axis(&mut self, id: u32, axis: usize, value: f32) {
        if let Some(f) = self.model.get_mut(id) {
            let mut o = f.world_origin();
            o[axis] = value;
            *f = f.with_world_origin(o);
            self.dirty = true;
        }
    }

    /// Replace a feature's primitive — the dimension fields in the properties panel.
    pub fn set_feature_primitive(&mut self, id: u32, p: Primitive) {
        if let Some(f) = self.model.get_mut(id) {
            f.primitive = p;
            self.dirty = true;
        }
    }

    /// Set one of a feature's rotation angles (degrees) about its plane-LOCAL axes:
    /// axis 0 = pitch (about plane u), 1 = roll (about plane v), 2 = spin (about the
    /// normal). Drives both the numeric fields and the rotation-ring gizmo.
    pub fn set_feature_rotation(&mut self, id: u32, axis: usize, deg: f32) {
        if let Some(f) = self.model.get_mut(id) {
            match axis {
                0 => f.placement.pitch_deg = deg,
                1 => f.placement.roll_deg = deg,
                _ => f.placement.spin_deg = deg,
            }
            self.dirty = true;
        }
    }

    /// A feature's current rotation `[pitch, roll, spin]` in degrees, for the panel/gizmo.
    pub fn feature_rotation(&self, id: u32) -> Option<[f32; 3]> {
        let f = self.model.features.iter().find(|f| f.id == id)?;
        Some([f.placement.pitch_deg, f.placement.roll_deg, f.placement.spin_deg])
    }

    /// The primitive of the single selected feature, for the properties panel.
    pub fn selected_primitive(&self) -> Option<(u32, Primitive, Vec3)> {
        let id = self.selected_single()?;
        let f = self.model.features.iter().find(|f| f.id == id)?;
        Some((id, f.primitive, f.world_origin()))
    }

    // ===================================================================
    // Move gizmo — a 3-axis translate handle at the selection centre
    // ===================================================================

    /// Screen geometry of the move gizmo, or `None` when nothing is selected / the centre
    /// is off-screen. The arm length scales with the object (so a big object gets a big
    /// gizmo) but has a screen-space floor (so a tiny one stays grabbable), and reaches
    /// PAST the object so the arms are not buried inside it.
    pub fn gizmo_view(&self, rect: egui::Rect, mvp: &[f32; 16]) -> Option<GizmoView> {
        let (mn, mx) = self.selection_aabb()?;
        let c = (mn + mx) * 0.5;
        let center_s = world_to_screen(c, rect, mvp)?;
        let half = ((mx - mn) * 0.5).max_element().max(1e-3);
        let ppw = self.px_per_world(c, rect, mvp);
        // Arms reach 1.4× the object's half-size, but never less than ~65 px on screen.
        let min_world = if ppw > 1e-6 { GIZMO_MIN_PX / ppw } else { half };
        let len = (half * 1.4).max(min_world);
        let mk = |h: GizmoHandle, d: Vec3| {
            let tip_w = c + d * len;
            world_to_screen(tip_w, rect, mvp).map(|tip_s| GizmoArm { handle: h, dir: d, tip_s })
        };
        Some(GizmoView {
            center_w: c,
            center_s,
            len_w: len,
            arms: [
                mk(GizmoHandle::X, Vec3::X),
                mk(GizmoHandle::Y, Vec3::Y),
                mk(GizmoHandle::Z, Vec3::Z),
            ],
        })
    }

    /// Approximate pixels-per-world at a point — the max screen speed over the three axes,
    /// so it stays non-zero even when one axis points at the camera.
    fn px_per_world(&self, c: Vec3, rect: egui::Rect, mvp: &[f32; 16]) -> f32 {
        let Some(cs) = world_to_screen(c, rect, mvp) else { return 0.0 };
        let probe = 0.5;
        let mut best = 0.0f32;
        for d in [Vec3::X, Vec3::Y, Vec3::Z] {
            if let Some(p) = world_to_screen(c + d * probe, rect, mvp) {
                best = best.max(cs.distance(p));
            }
        }
        best / probe
    }

    /// Which gizmo handle is under the cursor. The centre cube wins over the axes (it sits
    /// where all three arms meet), so a click there is always the free-move, never an axis.
    pub fn pick_gizmo(
        &self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<GizmoHandle> {
        let v = self.gizmo_view(rect, mvp)?;
        if v.center_s.distance(cursor) <= GIZMO_CUBE_PICK {
            return Some(GizmoHandle::Free);
        }
        let mut best: Option<(f32, GizmoHandle)> = None;
        for arm in v.arms.iter().flatten() {
            let d = dist_point_segment(cursor, v.center_s, arm.tip_s);
            if d <= GIZMO_AXIS_PICK && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, arm.handle));
            }
        }
        best.map(|(_, h)| h)
    }

    // ===================================================================
    // Rotation-ring gizmo
    // ===================================================================

    /// The rotation target: `(center_world, [axis0, axis1, axis2], is_furniture)`. Furniture
    /// rotates about WORLD axes; a single feature about its plane's LOCAL axes (u, v, n) — so
    /// ring 0→pitch(u), 1→roll(v), 2→spin(n), matching `set_feature_rotation`. `None` unless
    /// exactly one furniture OR one feature is selected.
    fn rot_target(&self) -> Option<(Vec3, [Vec3; 3], bool)> {
        let (mn, mx) = self.selection_aabb()?;
        let center = (mn + mx) * 0.5;
        if self.sel_furniture.is_some() {
            return Some((center, [Vec3::X, Vec3::Y, Vec3::Z], true));
        }
        let id = self.selected_single()?;
        let f = self.model.features.iter().find(|f| f.id == id)?;
        let (u, v) = f.plane.axes();
        let (u, v) = (u.normalize_or_zero(), v.normalize_or_zero());
        let n = u.cross(v).normalize_or_zero();
        Some((center, [u, v, n], false))
    }

    /// Rotation gizmo geometry for this frame — three rings sized like the move arms.
    pub fn rotation_rings(&self, rect: egui::Rect, mvp: &[f32; 16]) -> Option<RingView> {
        let (center, axes, is_furniture) = self.rot_target()?;
        let (mn, mx) = self.selection_aabb()?;
        let half = ((mx - mn) * 0.5).max_element().max(1e-3);
        let ppw = self.px_per_world(center, rect, mvp);
        let min_world = if ppw > 1e-6 { GIZMO_MIN_PX / ppw } else { half };
        let radius = (half * 1.3).max(min_world);
        let center_s = world_to_screen(center, rect, mvp)?;
        let handles = [GizmoHandle::RotX, GizmoHandle::RotY, GizmoHandle::RotZ];
        const SEG: usize = 48;
        let mut rings = Vec::with_capacity(3);
        for i in 0..3 {
            let (a, b) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
            let mut pts = Vec::with_capacity(SEG + 1);
            for k in 0..=SEG {
                let t = (k as f32) / (SEG as f32) * std::f32::consts::TAU;
                let w = center + (a * t.cos() + b * t.sin()) * radius;
                if let Some(s) = world_to_screen(w, rect, mvp) {
                    pts.push(s);
                }
            }
            rings.push(Ring { handle: handles[i], axis: axes[i], pts });
        }
        Some(RingView { center, center_s, radius, rings, is_furniture })
    }

    /// Which rotation ring is under the cursor (nearest ring polyline within tolerance).
    pub fn pick_ring(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<GizmoHandle> {
        let rv = self.rotation_rings(rect, mvp)?;
        let mut best: Option<(f32, GizmoHandle)> = None;
        for ring in &rv.rings {
            let mut d = f32::INFINITY;
            for seg in ring.pts.windows(2) {
                d = d.min(dist_point_segment(cursor, seg[0], seg[1]));
            }
            if d <= GIZMO_AXIS_PICK && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, ring.handle));
            }
        }
        best.map(|(_, h)| h)
    }

    /// The unit vector from the ring centre to where the cursor ray meets the ring's plane
    /// (axis-component removed). `None` if the ray is parallel to the plane or hits the centre.
    fn ray_to_ring_vec(&self, cursor: egui::Pos2, center: Vec3, axis: Vec3, rect: egui::Rect, mvp: &[f32; 16]) -> Option<Vec3> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let denom = dir.dot(axis);
        if denom.abs() < 1e-5 { return None; }
        let t = (center - orig).dot(axis) / denom;
        if t <= 0.0 { return None; }
        let p = orig + dir * t;
        let r = (p - center) - axis * (p - center).dot(axis);
        let len = r.length();
        if len < 1e-5 { return None; }
        Some(r / len)
    }

    /// Begin a rotation-ring drag on `handle`. Captures the grab reference + start rotation so
    /// the gesture is one undo step. Returns false if the grab can't be established.
    pub fn rot_begin(&mut self, handle: GizmoHandle, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> bool {
        let Some((center, axes, is_furniture)) = self.rot_target() else { return false };
        let Some(ai) = handle.ring_axis() else { return false };
        let axis = axes[ai];
        let Some(r0) = self.ray_to_ring_vec(cursor, center, axis, rect, mvp) else { return false };
        let start_rot = if is_furniture {
            self.sel_furniture.and_then(|fi| self.furniture.get(fi)).map(|f| f.rot).unwrap_or([0.0; 3])
        } else {
            self.selected_single().and_then(|id| self.feature_rotation(id)).unwrap_or([0.0; 3])
        };
        self.rot_drag = Some(RotDrag {
            handle, axis, center, r0, start_rot,
            feat_axis: ai, is_furniture,
        });
        true
    }

    /// Apply the current cursor position to the live rotation drag. Furniture composes a
    /// world-axis quaternion (kept as Euler for the numeric fields); a feature adds the swept
    /// angle to the ring's plane-local placement angle.
    pub fn rot_update(&mut self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) {
        let Some(d) = self.rot_drag else { return };
        let Some(r1) = self.ray_to_ring_vec(cursor, d.center, d.axis, rect, mvp) else { return };
        // Signed angle r0→r1 about the axis (radians).
        let angle = d.r0.cross(r1).dot(d.axis).atan2(d.r0.dot(r1));
        if d.is_furniture {
            if let Some(inst) = self.sel_furniture.and_then(|fi| self.furniture.get_mut(fi)) {
                let q_start = glam::Quat::from_euler(
                    glam::EulerRot::XYZ,
                    d.start_rot[0].to_radians(), d.start_rot[1].to_radians(), d.start_rot[2].to_radians(),
                );
                let q = glam::Quat::from_axis_angle(d.axis, angle) * q_start;
                let (x, y, z) = q.to_euler(glam::EulerRot::XYZ);
                inst.rot = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
            }
        } else if let Some(id) = self.selected_single() {
            let deg = d.start_rot[d.feat_axis] + angle.to_degrees();
            self.set_feature_rotation(id, d.feat_axis, deg);
        }
    }

    /// End a rotation drag.
    pub fn rot_end(&mut self) {
        self.rot_drag = None;
    }

    // ===================================================================
    // Wall vertex handles — reshaping an alive wall in the 3D view
    // ===================================================================

    /// The wall the current 3D selection belongs to. Handles are shown for THIS wall
    /// only: drawing them for every wall at once would bury the model in dots.
    pub fn selected_wall(&self) -> Option<usize> {
        self.selection.iter().find_map(|&id| self.wall_index(id))
    }

    /// Re-select a wall by its segments.
    ///
    /// Needed after every SHAPE edit: `rederive_wall` drops and rebuilds the Boxes, so the
    /// old ids are gone and the selection would be empty — the handles would vanish
    /// mid-gesture. (Height edits are different: they mutate in place and ids survive.)
    pub fn select_wall(&mut self, wi: usize) {
        if let Some(w) = self.walls.get(wi) {
            self.selection = w.segments.clone();
            self.sel_key.clear();
        }
    }

    /// World position of footprint vertex `vi` — on the wall's OWN storey, so handles on
    /// an upper floor appear up there rather than on the ground.
    fn wall_vertex_world(&self, wi: usize, vi: usize) -> Option<Vec3> {
        let w = self.walls.get(wi)?;
        let p = w.footprint.get(vi)?;
        Some(Vec3::new(p.x, p.y, w.base_z))
    }

    /// Screen positions of wall `wi`'s footprint vertices, as `(vertex index, position)`.
    /// Vertices behind the camera are omitted, so nothing is drawn or picked where the
    /// user cannot see it.
    pub fn wall_vertex_handles(
        &self, wi: usize, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Vec<(usize, egui::Pos2)> {
        let Some(w) = self.walls.get(wi) else { return Vec::new() };
        (0..w.footprint.len())
            .filter_map(|vi| {
                let world = self.wall_vertex_world(wi, vi)?;
                Some((vi, world_to_screen(world, rect, mvp)?))
            })
            .collect()
    }

    /// Screen positions of each EDGE's midpoint, as `(segment index, position)` — the
    /// click target for inserting a vertex.
    pub fn wall_edge_handles(
        &self, wi: usize, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Vec<(usize, egui::Pos2)> {
        let Some(w) = self.walls.get(wi) else { return Vec::new() };
        (0..w.footprint.len().saturating_sub(1))
            .filter_map(|si| {
                let a = self.wall_vertex_world(wi, si)?;
                let b = self.wall_vertex_world(wi, si + 1)?;
                Some((si, world_to_screen((a + b) * 0.5, rect, mvp)?))
            })
            .collect()
    }

    /// Vertex handle under the cursor, if any. Nearest wins, so overlapping handles
    /// resolve predictably.
    pub fn pick_wall_vertex(
        &self, wi: usize, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<usize> {
        nearest_within(self.wall_vertex_handles(wi, rect, mvp), cursor, HANDLE_PICK_R)
    }

    /// Edge midpoint under the cursor. A TIGHTER aperture than a vertex, because a
    /// midpoint sits between two vertex handles — the vertex must win a close call, or
    /// dragging a corner would insert a point instead.
    pub fn pick_wall_edge(
        &self, wi: usize, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16],
    ) -> Option<usize> {
        if self.pick_wall_vertex(wi, cursor, rect, mvp).is_some() {
            return None;
        }
        nearest_within(self.wall_edge_handles(wi, rect, mvp), cursor, EDGE_PICK_R)
    }

    // ===================================================================
    // Slabs — floors and ceilings
    // ===================================================================

    /// Add a horizontal slab spanning `footprint`, `thickness` thick, with its **top face
    /// at `top_z`**. Returns `(feature id, exact)`.
    ///
    /// `exact == false` means the outline is not a rectangle and the slab was built from
    /// its bounding box, so it OVER-COVERS (an L-shaped room gets a rectangular floor).
    /// The caller must report that — a silently wrong floor is worse than none, and it
    /// would hand the light calc a surface that is not the room.
    ///
    /// Why the limit exists: `Primitive::Box` is the only slab-shaped primitive
    /// `cad_solid` has, and an arbitrary profile needs the extrusion primitive that is
    /// still awaiting sign-off (`mentor MD/CAD_SOLID_EXTRUSION_PRIMITIVE_SPEC_2026-07-23.md`).
    /// A rotated rectangle IS exact — `Placement::spin_deg` carries the angle.
    /// Add a horizontal slab spanning `footprint`, `thickness` thick, top face at `top_z`.
    ///
    /// A slab is just an EXTRUSION of the outline by its thickness, so it is exact for ANY
    /// shape — L-rooms, circles, arbitrary polygons — not only rectangles. (It used to
    /// fall back to a bounding box for non-rectangles, which is why every non-rectangular
    /// floor came out a plain rectangle.) Returns `None` if the outline is not a valid
    /// closed profile (too few points / no area / self-crossing).
    pub fn add_slab(&mut self, footprint: &[Vec2], thickness: f32, top_z: f32) -> Option<u32> {
        let t = thickness.max(0.01);
        let (profile, centre, w, d) = self.model.add_profile(footprint).ok()?;
        // An extrusion rises +Z from its placement, so lift so the TOP face lands on
        // `top_z` — a floor's top is what you stand on, a ceiling's underside what you see.
        let placement = Placement { u: centre.x, v: centre.y, lift: top_z - t, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 };
        let p = Primitive::Extrusion { profile, h: t, w, d };
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.dirty = true;
        Some(id)
    }

    /// BUILDING OUTLINE: extrude a closed outline into one solid mass on the active
    /// storey, rising to `height`.
    ///
    /// This is what the greyed-out Building-outline row was waiting for. Unlike
    /// [`Self::add_slab`], an arbitrary shape is EXACT here — no bounding box — because
    /// `Primitive::Extrusion` carries the real profile.
    ///
    /// Returns the new feature id, or the reason the outline was refused.
    pub fn add_building_outline(
        &mut self, footprint: &[Vec2], height: f32,
    ) -> Result<u32, cad_solid::ProfileError> {
        let (profile, centre, w, d) = self.model.add_profile(footprint)?;
        let placement = Placement {
            u: centre.x, v: centre.y, lift: self.active_base_z(), spin_deg: 0.0,
            pitch_deg: 0.0, roll_deg: 0.0,
        };
        let id = self.model.push(
            BoolOp::Union,
            Plane::default(),
            placement,
            Primitive::Extrusion { profile, h: height.max(0.01), w, d },
        );
        self.selection = vec![id];
        self.dirty = true;
        Ok(id)
    }

    /// ROOM: carve an interior space out of the building solid from a closed outline.
    ///
    /// A building is a SOLID mass; a room is the void inside it. The outline is extruded
    /// and SUBTRACTED (`BoolOp::Difference`), and the void is inset vertically by a floor
    /// slab and a ceiling slab, so what remains around it reads as a real room — walls
    /// (the material between the outline and the building's edge), a floor below, and a
    /// ceiling above.
    ///
    /// Requires an existing solid to cut from: `csg::eval` treats the FIRST feature as the
    /// base regardless of its op, so a lone Difference would perversely render as a solid.
    /// Refused with [`RoomError::NoBuilding`] until a building exists.
    /// Carve the room's interior column out of an enclosing SOLID building, turning that
    /// building into a WALL (an annulus around the room). Returns true if a carve happened.
    ///
    /// "Enclosing building" = a THICK Union feature whose outline CONTAINS every point of the
    /// room footprint. The carve is the room footprint extruded from `base` up through the
    /// building's top, subtracted (`BoolOp::Difference`). The Difference feature is placed
    /// IMMEDIATELY AFTER the building so the group-based `eval` applies it to that body.
    fn carve_interior_from_building(&mut self, footprint: &[Vec2], base: f32) -> bool {
        // Find the enclosing building and its feature index + top height.
        let mut target: Option<(usize, f32)> = None;
        for (i, f) in self.model.features.iter().enumerate() {
            if f.op != BoolOp::Union {
                continue;
            }
            let (mn, mx) = f.world_aabb();
            if (mx.z - mn.z) <= 0.5 {
                continue; // a thin slab is a floor/ceiling, not a building mass
            }
            if let Some(outline) = self.feature_world_outline(f) {
                if outline.len() >= 3 && footprint.iter().all(|p| point_in_poly(&outline, p.x, p.y)) {
                    target = Some((i, mx.z)); // last (top-most in list) enclosing solid wins
                }
            }
        }
        let Some((idx, top)) = target else { return false };
        // Build the void, then move it to sit right after the building it cuts.
        let Ok((profile, centre, w, d)) = self.model.add_profile(footprint) else {
            return false;
        };
        let void_h = (top - base).max(0.1) + 0.02; // punch fully through the building
        let placement = Placement { u: centre.x, v: centre.y, lift: base, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 };
        self.model.push(
            BoolOp::Difference,
            Plane::default(),
            placement,
            Primitive::Extrusion { profile, h: void_h, w, d },
        );
        // `push` appended at the end; relocate it to just after the building feature so the
        // difference cuts the BUILDING body and nothing else.
        if let Some(void) = self.model.features.pop() {
            self.model.features.insert(idx + 1, void);
        }
        self.dirty = true;
        true
    }

    pub fn add_room(&mut self, footprint: &[Vec2]) -> Result<u32, RoomError> {
        // CONSTRUCTIVE room — built from an outline as a complete enclosed space, with NO
        // pre-existing building required:
        //
        //   floor slab   [base, base+floor]                       — always
        //   perimeter    walls on each edge, [base+floor, +height]— the room's walls
        //   walls
        //   ceiling slab [base+floor+height, +ceiling]            — unless open to sky
        //
        // This is what "draw a room, get a room" should mean. (The old behaviour carved a
        // void from a solid building — which left a hollow ring whenever there was no
        // matching building.)
        let base = self.active_base_z();
        let floor_t = self.room_floor.max(0.02);
        let h = self.room_height.max(0.05);
        let wall_t = self.wall_thickness.max(0.02);

        // If this room sits inside a SOLID building, carve its interior column out of that
        // building so the building becomes a WALL (an annulus around the room) rather than a
        // solid cap. Then hiding the room's ceiling reveals the floor while the surrounding
        // wall — its own solid, with its own top — stays. Without this, a solid building
        // over a room can never be "seen into".
        self.carve_interior_from_building(footprint, base);

        // Distinct default colours (≈ real reflectances) so floor / walls / ceiling are
        // TELLABLE APART from any angle — including straight down, where hiding the light
        // ceiling to reveal the dark floor is now an obvious change.
        const FLOOR_COL: [f32; 3] = [0.34, 0.31, 0.28];   // dark, ~0.2
        const WALL_COL: [f32; 3] = [0.62, 0.62, 0.64];    // mid, ~0.5
        const CEIL_COL: [f32; 3] = [0.90, 0.90, 0.93];    // light, ~0.7

        // FLOOR slab: top face at base + floor_t, so the walls stand on it.
        let floor_id = match self.add_slab(footprint, floor_t, base + floor_t) {
            Some(id) => id,
            None => return Err(RoomError::Profile(cad_solid::ProfileError::Degenerate)),
        };
        self.feature_color.insert(floor_id, FLOOR_COL);

        // WALLS: one box per outline edge, sitting on the floor slab.
        let wall_base = base + floor_t;
        for e in footprint.windows(2) {
            if let Some(id) = self.push_wall_box(e[0], e[1], wall_t, h, wall_base) {
                self.feature_color.insert(id, WALL_COL);
            }
        }
        // Close the loop if the outline wasn't already closed.
        if footprint.len() >= 3 {
            let (a, b) = (footprint[footprint.len() - 1], footprint[0]);
            if (a - b).length() > 1e-4 {
                if let Some(id) = self.push_wall_box(a, b, wall_t, h, wall_base) {
                    self.feature_color.insert(id, WALL_COL);
                }
            }
        }

        // CEILING slab on top of the walls, tracked so it can be hidden — unless open sky.
        if !self.room_open_top {
            let ct = self.ceiling_thickness.max(0.02);
            if let Some(cid) = self.add_slab(footprint, ct, wall_base + h + ct) {
                self.feature_color.insert(cid, CEIL_COL);
                self.ceilings.insert(cid);
            }
        }

        self.selection = vec![floor_id];
        self.dirty = true;
        Ok(floor_id)
    }

    /// Floor of the active storey — its top face is the level the walls stand on.
    ///
    /// Note the consequence: the slab's BODY lies below that base, so
    /// [`Self::features_on_storey`] records an upper floor on the storey BENEATH it. That
    /// is structurally what it is — level 1's floor and level 0's ceiling are one slab —
    /// and it is what decides which level `delete_storey` takes it with.
    pub fn add_floor(&mut self, footprint: &[Vec2], thickness: f32) -> Option<u32> {
        let z = self.active_base_z();
        self.add_slab(footprint, thickness, z)
    }

    /// Ceiling of the active storey — its top face is the floor level of the storey above,
    /// so a ceiling and the floor above it meet rather than overlap.
    pub fn add_ceiling(&mut self, footprint: &[Vec2], thickness: f32) -> Option<u32> {
        let i = self.active_storey.min(self.storeys.len().saturating_sub(1));
        let z = self.storey_base_z(i) + self.storeys[i].height;
        let id = self.add_slab(footprint, thickness, z)?;
        // Track it as a ceiling so "Hide ceilings" hides THIS one too — not just room
        // ceilings. Without this, a ceiling made with the Make-ceiling tool was unhideable.
        self.ceilings.insert(id);
        Some(id)
    }

    /// Drop the 3D selection AND the cached highlight mesh key. Both must go together:
    /// leaving `sel_key` set would make `sync_selection_mesh` think the (now empty)
    /// selection is already drawn, and the old highlight would linger.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
        self.sel_furniture = None;
        self.sel_key.clear();
    }

    /// PERSISTENCE: capture the 3D model for the sidecar. Camera, selection and any live
    /// sketch session are deliberately NOT captured — they are view state, not the
    /// building.
    pub fn to_persist(&self) -> crate::simlux_io::FactoryDoc {
        crate::simlux_io::FactoryDoc {
            model: self.model.clone(),
            walls: self
                .walls
                .iter()
                .map(|w| crate::simlux_io::WallRec {
                    footprint: w.footprint.iter().map(|p| [p.x, p.y]).collect(),
                    segments: w.segments.clone(),
                    thickness: w.thickness,
                    height: w.height,
                    rake_deg: w.rake_deg,
                    base_z: w.base_z,
                })
                .collect(),
            wall_height: self.wall_height,
            building_height: self.building_height,
            storeys: self
                .storeys
                .iter()
                .map(|s| crate::simlux_io::StoreyRec { name: s.name.clone(), height: s.height })
                .collect(),
            active_storey: self.active_storey,
            ceilings: self.ceilings.iter().copied().collect(),
            furniture_lib: self
                .furniture_lib
                .iter()
                .map(|a| crate::simlux_io::FurnitureAssetRec {
                    name: a.name.clone(),
                    positions: a.positions.clone(),
                    normals: a.normals.clone(),
                    color: a.color,
                })
                .collect(),
            furniture: self
                .furniture
                .iter()
                .map(|f| crate::simlux_io::FurnitureInstRec {
                    asset: f.asset,
                    pos: f.pos,
                    scale: f.scale,
                    rot_deg: f.rot[2],
                    rot_xy: [f.rot[0], f.rot[1]],
                    color: f.color,
                })
                .collect(),
            feature_colors: self.feature_color.iter().map(|(&k, &v)| (k, v)).collect(),
            surface_colors: self
                .surface_color
                .iter()
                .map(|(&(f, a, b, c, d), &col)| (f, a, b, c, d, col))
                .collect(),
        }
    }

    /// PERSISTENCE: restore a model read from the sidecar. Returns the number of wall
    /// records DROPPED as unusable, so the caller can report it — a silently vanishing
    /// wall would look like data loss with no explanation.
    ///
    /// A wall is dropped when its footprint is too short to extrude (< 2 points) or when
    /// any segment id names a feature the model does not contain — that link is what
    /// makes a wall editable, and a dangling one would panic or mis-edit later.
    ///
    /// Leaves the model `dirty` rather than re-evaluating: `recompute()` walks a BSP per
    /// boolean, and the caller decides when to pay that.
    pub fn apply_persist(&mut self, d: crate::simlux_io::FactoryDoc) -> usize {
        let have: std::collections::HashSet<u32> =
            d.model.features.iter().map(|f| f.id).collect();
        let mut dropped = 0usize;
        let mut walls = Vec::with_capacity(d.walls.len());
        for w in d.walls {
            let usable = w.footprint.len() >= 2
                && !w.segments.is_empty()
                && w.segments.iter().all(|id| have.contains(id));
            if !usable {
                dropped += 1;
                continue;
            }
            walls.push(WallInst {
                footprint: w.footprint.iter().map(|p| Vec2::new(p[0], p[1])).collect(),
                segments: w.segments,
                thickness: w.thickness,
                height: w.height,
                rake_deg: w.rake_deg,
                base_z: w.base_z,
            });
        }
        self.model = d.model;
        self.walls = walls;
        // A zero height means the sidecar predates the field — keep the live default
        // rather than adopting a building of no height.
        if d.wall_height > 0.0 {
            self.wall_height = d.wall_height;
        }
        if d.building_height > 0.0 {
            self.building_height = d.building_height;
        }
        // A pre-storeys sidecar has no levels. Substitute the single ground storey rather
        // than leaving `storeys` empty, which would make `active_storey` index nothing.
        // Zero-height levels are dropped for the same reason (no z band ⇒ nothing can
        // ever belong to them).
        let levels: Vec<Storey> = d
            .storeys
            .into_iter()
            .filter(|s| s.height >= MIN_STOREY_H)
            .map(|s| Storey { name: s.name, height: s.height })
            .collect();
        self.storeys = if levels.is_empty() {
            vec![Storey { name: "Ground".into(), height: self.building_height.max(MIN_STOREY_H) }]
        } else {
            levels
        };
        self.active_storey = d.active_storey.min(self.storeys.len() - 1);
        // Ceilings that still exist in the restored model.
        let have: std::collections::HashSet<u32> =
            self.model.features.iter().map(|f| f.id).collect();
        self.ceilings = d.ceilings.into_iter().filter(|id| have.contains(id)).collect();
        self.furniture_lib = d
            .furniture_lib
            .into_iter()
            .map(|a| {
                let color = if a.color == [0.0, 0.0, 0.0] { [0.82, 0.82, 0.84] } else { a.color };
                FurnitureAsset { name: a.name, positions: a.positions, normals: a.normals, color }
            })
            .collect();
        // Keep only instances whose asset still exists.
        let nlib = self.furniture_lib.len();
        self.furniture = d
            .furniture
            .into_iter()
            .filter(|f| f.asset < nlib)
            .map(|f| FurnitureInst {
                asset: f.asset,
                pos: f.pos,
                scale: if f.scale > 0.0 { f.scale } else { 1.0 },
                rot: [f.rot_xy[0], f.rot_xy[1], f.rot_deg],
                color: f.color,
            })
            .collect();
        self.feature_color = d.feature_colors.into_iter().collect();
        self.surface_color = d
            .surface_colors
            .into_iter()
            .map(|(f, a, b, c, dd, col)| ((f, a, b, c, dd), col))
            .collect();
        // Ids are safe to carry across: `Model::push` mints `max(id) + 1`, so restored
        // ids are never reused. Selection, though, indexed the OLD model — drop it.
        self.clear_selection();
        self.dirty = true;
        dropped
    }

    /// DRAW3D: commit the dialog's primitive into the model (at the origin).
    pub fn add_primitive(&mut self, p: Primitive) {
        // Built on the ACTIVE storey, like every other new solid.
        let placement = Placement { lift: self.active_base_z(), ..Placement::default() };
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.selection = vec![id];
        self.dirty = true;
    }

    /// DRAW3D: place a dialog-built primitive at a picked point. The click is a CORNER for
    /// a Box (it extends +w,+d,+h from there) and the CENTRE for everything else.
    pub fn place_primitive(&mut self, p: Primitive, at: Vec3) {
        let plane = Plane::default();
        let uv = plane.to_uv(at);
        let (ox, oy) = match p {
            Primitive::Box { w, d, .. } => (w * 0.5, d * 0.5), // click = the near corner
            _ => (0.0, 0.0),                                   // click = the centre
        };
        // The click gives x,y; the ACTIVE storey gives z. Clicking the ground plane while
        // level 2 is active must build on level 2, not under it.
        let placement = Placement {
            u: uv.x + ox, v: uv.y + oy, lift: self.active_base_z(), spin_deg: 0.0,
            pitch_deg: 0.0, roll_deg: 0.0,
        };
        let id = self.model.push(BoolOp::Union, plane, placement, p);
        self.selection = vec![id];
        self.dirty = true;
    }

    pub fn add_cylinder(&mut self) {
        let p = Primitive::Cylinder { r: self.cyl_r, h: self.cyl_h, sides: self.cyl_sides.max(3) };
        // Built on the ACTIVE storey, like every other new solid.
        let placement = Placement { lift: self.active_base_z(), ..Placement::default() };
        let id = self.model.push(BoolOp::Union, Plane::default(), placement, p);
        self.selection = vec![id];
        self.dirty = true;
    }

    // ── 2D → 3D wall promotion ──────────────────────────────────────────────────────
    // The practical journey (owner, 2026-07-17): draft the wall in 2D with the real
    // `wall` tool (snapping / ortho / corner-join), select it, right-click → Make 3D
    // wall. Each selected `Geom::Wall`'s centerline becomes placed Boxes here.

    /// Extrude ONE footprint edge `a→b` to a placed Box and push it, returning its feature
    /// id (or `None` if degenerate). `a`,`b` are ground-plane centerline points (a 2D
    /// wall's coords ARE the ground uv); the Box keeps `thickness` and rises to `height`.
    /// Pure Box + Placement (see `Plane::world_matrix`), so no `cad_solid` change is needed.
    fn push_wall_box(
        &mut self, a: Vec2, b: Vec2, thickness: f32, height: f32, base_z: f32,
    ) -> Option<u32> {
        let d = b - a;
        let len = d.length();
        if len < 1e-4 || thickness <= 0.0 || height <= 0.0 {
            return None; // ignore degenerate input
        }
        let mid = (a + b) * 0.5;
        let p = Primitive::Box { w: len, d: thickness, h: height };
        let placement = Placement {
            u: mid.x, v: mid.y, lift: base_z, spin_deg: d.y.atan2(d.x).to_degrees(),
            pitch_deg: 0.0, roll_deg: 0.0,
        };
        Some(self.model.push(BoolOp::Union, Plane::default(), placement, p))
    }

    /// Promote a **footprint** (≥ 2 ground-plane points) to a live wall: one Box per edge,
    /// all sharing `thickness` and `height`. The wall stays ALIVE — its footprint and
    /// height are remembered so vertices and rise can be edited later. Degenerate edges are
    /// skipped; returns the new wall's index, or `None` if every edge was degenerate.
    pub fn add_wall(&mut self, footprint: Vec<Vec2>, thickness: f32, height: f32) -> Option<usize> {
        if footprint.len() < 2 {
            return None;
        }
        // New geometry is built on the ACTIVE storey — that is what makes the storey
        // selector mean anything.
        let base_z = self.active_base_z();
        let mut segments = Vec::new();
        for w in footprint.windows(2) {
            if let Some(id) = self.push_wall_box(w[0], w[1], thickness, height, base_z) {
                segments.push(id);
            }
        }
        if segments.is_empty() {
            return None;
        }
        self.walls.push(WallInst {
            footprint, segments, thickness, height, rake_deg: 0.0, base_z,
        });
        self.dirty = true;
        Some(self.walls.len() - 1)
    }

    /// Back-compat + simplest promotion: a single centerline segment → a 2-point wall.
    pub fn add_wall_segment(&mut self, a: Vec2, b: Vec2, thickness: f32, height: f32) {
        self.add_wall(vec![a, b], thickness, height);
    }

    /// Index of the live-wall record OWNING `feature_id` (any of its segments), if any.
    pub fn wall_index(&self, feature_id: u32) -> Option<usize> {
        self.walls.iter().position(|w| w.segments.contains(&feature_id))
    }

    /// Rebuild every segment Box of wall `wi` from its current footprint + params. The old
    /// Boxes are dropped and fresh ones pushed (the segment count changes when a vertex is
    /// added or removed). Both rings follow the one footprint, so they stay coincident.
    /// Segment feature ids change — callers that track a selection must refresh it.
    fn rederive_wall(&mut self, wi: usize) {
        if wi >= self.walls.len() {
            return;
        }
        for id in std::mem::take(&mut self.walls[wi].segments) {
            self.model.remove(id);
        }
        let fp = self.walls[wi].footprint.clone();
        let (t, h) = (self.walls[wi].thickness, self.walls[wi].height);
        // Rebuild at the wall's OWN base, not the active storey's: editing a vertex on
        // the third floor must not drop the wall to the ground.
        let base_z = self.walls[wi].base_z;
        let mut segments = Vec::new();
        for w in fp.windows(2) {
            if let Some(id) = self.push_wall_box(w[0], w[1], t, h, base_z) {
                segments.push(id);
            }
        }
        self.walls[wi].segments = segments;
        self.dirty = true;
    }

    /// Change a live wall's height and re-derive — the "walls are alive" edit. Updates each
    /// segment Box IN PLACE (feature ids stay stable, so a selection survives), keeping each
    /// segment's length and thickness; only the rise changes.
    pub fn set_wall_height(&mut self, feature_id: u32, height: f32) {
        let h = height.max(0.01);
        if let Some(i) = self.wall_index(feature_id) {
            self.walls[i].height = h;
            let t = self.walls[i].thickness;
            let fp = self.walls[i].footprint.clone();
            let segs = self.walls[i].segments.clone();
            for (k, w) in fp.windows(2).enumerate() {
                if let Some(&fid) = segs.get(k) {
                    let len = (w[1] - w[0]).length();
                    if let Some(f) = self.model.get_mut(fid) {
                        f.primitive = Primitive::Box { w: len, d: t, h };
                    }
                }
            }
            self.dirty = true;
        }
    }

    /// Change a live wall's THICKNESS, the twin of [`Self::set_wall_height`]. Updates each
    /// segment Box in place (a Box's `d` IS the wall's thickness), so feature ids stay
    /// stable and a selection — and its handles — survive the edit.
    pub fn set_wall_thickness(&mut self, feature_id: u32, thickness: f32) {
        let t = thickness.max(0.01);
        if let Some(i) = self.wall_index(feature_id) {
            self.walls[i].thickness = t;
            let h = self.walls[i].height;
            let fp = self.walls[i].footprint.clone();
            let segs = self.walls[i].segments.clone();
            for (k, w) in fp.windows(2).enumerate() {
                if let Some(&fid) = segs.get(k) {
                    let len = (w[1] - w[0]).length();
                    if let Some(f) = self.model.get_mut(fid) {
                        f.primitive = Primitive::Box { w: len, d: t, h };
                    }
                }
            }
            self.dirty = true;
        }
    }

    /// Move footprint vertex `vi` of wall `wi` to `to`, then re-derive — this is how a 3D
    /// handle drag "shifts the surface". Because both rings share the footprint, the whole
    /// vertical edge moves together.
    pub fn wall_move_vertex(&mut self, wi: usize, vi: usize, to: Vec2) {
        let ok = matches!(self.walls.get(wi), Some(w) if vi < w.footprint.len());
        if !ok {
            return;
        }
        self.walls[wi].footprint[vi] = to;
        self.rederive_wall(wi);
    }

    /// Insert a vertex at `at` into wall `wi`, splitting the segment between
    /// `footprint[seg]` and `footprint[seg + 1]`. The new corner exists on BOTH the floor
    /// and ceiling rings by construction (they share the footprint). Returns the new
    /// vertex index, or `None` if `seg` is out of range.
    pub fn wall_insert_vertex(&mut self, wi: usize, seg: usize, at: Vec2) -> Option<usize> {
        let n = self.walls.get(wi)?.footprint.len();
        if seg + 1 >= n {
            return None;
        }
        self.walls[wi].footprint.insert(seg + 1, at);
        self.rederive_wall(wi);
        Some(seg + 1)
    }

    /// Delete footprint vertex `vi` of wall `wi`, then re-derive. A wall keeps a minimum of
    /// 2 points (one segment); returns `false` if the delete was rejected.
    pub fn wall_delete_vertex(&mut self, wi: usize, vi: usize) -> bool {
        match self.walls.get(wi) {
            Some(w) if w.footprint.len() > 2 && vi < w.footprint.len() => {}
            _ => return false,
        }
        self.walls[wi].footprint.remove(vi);
        self.rederive_wall(wi);
        true
    }

    pub fn erase_selection(&mut self) {
        for id in std::mem::take(&mut self.selection) {
            self.model.remove(id);
            self.ceilings.remove(&id); // keep the ceiling set in step with the model
        }
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.ceilings.clear();
        self.model = Model::default();
        self.selection.clear();
        self.walls.clear();
        self.dirty = true;
    }

    /// Re-evaluate the CSG tree. Call ONLY when idle — csgrs walks a BSP per boolean.
    ///
    /// Hiding ceilings is NOT done here — it is a RENDER-time filter in [`Self::scene_verts`]
    /// (keyed on each triangle's feature id), so toggling it is instant and never depends
    /// on a re-evaluation. `cached` always holds the FULL model (undo / save / the light
    /// calc all see every ceiling).
    pub fn recompute(&mut self) {
        self.cached = self.model.eval();
        self.ceiling_caps = self.detect_ceiling_caps();
        self.sel_key.clear(); // the model changed → the selection's mesh is stale
        self.ensure_sel_mesh();
        self.dirty = false;
    }

    /// Is feature `id` hidden while "Hide ceilings" is on? True if it is a tracked room
    /// ceiling OR a geometrically-detected top cap. The geometry arm is what makes the
    /// toggle RELIABLE — it works even when the tracked id-set has drifted (the field bug).
    pub fn is_hidden_ceiling(&self, id: u32) -> bool {
        self.ceilings.contains(&id) || self.ceiling_caps.contains(&id)
    }

    /// Find the feature ids that are ceiling / roof CAPS purely from geometry: a thin,
    /// horizontal slab that sits at the TOP of the model. This is what "hide the ceiling"
    /// should target, and it cannot drift like the hand-maintained `ceilings` set.
    ///
    /// A cap must be (per its own world AABB):
    ///   * THIN in Z         — a slab, not a wall or a tall solid,
    ///   * FLAT              — far wider than it is thick,
    ///   * ELEVATED          — its underside is well above the ground (so a FLOOR at z≈0 is
    ///                         never mistaken for a ceiling),
    ///   * TOPMOST           — its top is level with the model's highest point (so an
    ///                         intermediate storey's slab is NOT hidden, only the roof/ceiling).
    /// World outline (XY polygon) of a slab/box feature, or `None` for a shape with no
    /// closed outline. Extrusions carry their real profile; a Box is its rotated rectangle.
    fn feature_world_outline(&self, f: &cad_solid::Feature) -> Option<Vec<Vec2>> {
        match &f.primitive {
            Primitive::Extrusion { profile, .. } => {
                let p = self.model.profile(*profile)?;
                // Stored pts are centred on the profile; placement (u,v) is that centre.
                Some(
                    p.pts
                        .iter()
                        .map(|q| Vec2::new(q[0] + f.placement.u, q[1] + f.placement.v))
                        .collect(),
                )
            }
            Primitive::Box { w, d, .. } => {
                let (hw, hd) = (w * 0.5, d * 0.5);
                let a = f.placement.spin_deg.to_radians();
                let (c, s) = (a.cos(), a.sin());
                Some(
                    [(-hw, -hd), (hw, -hd), (hw, hd), (-hw, hd)]
                        .iter()
                        .map(|(x, y)| {
                            Vec2::new(
                                f.placement.u + x * c - y * s,
                                f.placement.v + x * s + y * c,
                            )
                        })
                        .collect(),
                )
            }
            _ => None,
        }
    }

    fn detect_ceiling_caps(&self) -> std::collections::HashSet<u32> {
        let mut caps = std::collections::HashSet::new();
        let Some((_, world_mx)) = self.cached.bounds() else {
            return caps;
        };
        let world_top = world_mx[2];
        // Cache each Union feature's world AABB once — used twice below.
        let unions: Vec<(u32, Vec3, Vec3)> = self
            .model
            .features
            .iter()
            .filter(|f| f.op == cad_solid::BoolOp::Union) // cutters (room voids) aren't surfaces
            .map(|f| {
                let (mn, mx) = f.world_aabb();
                (f.id, mn, mx)
            })
            .collect();

        // PASS 1 — thin flat ELEVATED TOPMOST slabs: these are ceiling / roof CAPS.
        for &(id, mn, mx) in &unions {
            let dz = mx.z - mn.z;
            let (dx, dy) = (mx.x - mn.x, mx.y - mn.y);
            let thin = dz <= 0.5;
            let flat = dx > 3.0 * dz && dy > 3.0 * dz;
            let elevated = mn.z > 0.5;
            let topmost = mx.z >= world_top - 0.05;
            if thin && flat && elevated && topmost {
                caps.insert(id);
            }
        }

        caps
    }

    /// Refresh the selection mesh if the selection moved on (cheap no-op otherwise).
    pub fn sync_selection_mesh(&mut self) {
        self.ensure_sel_mesh();
    }

    /// Upper bound for `cam_dist`, scaled to the model so you can always dolly back far
    /// enough to frame the WHOLE scene — a fixed cap (was 400) was too small for large
    /// imports (e.g. an architectural DXF in millimetres, span 100 000+). 20× the largest
    /// span, never below 400 so small/empty scenes keep the old generous headroom.
    pub fn max_cam_dist(&self) -> f32 {
        self.cached
            .bounds()
            .map(|(mn, mx)| {
                let span = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(mx[2] - mn[2]);
                (span * 20.0).max(400.0)
            })
            .unwrap_or(400.0)
    }

    /// Zoom-extents: the ONLY thing that moves `cam_target`.
    pub fn fit(&mut self) {
        if let Some((mn, mx)) = self.cached.bounds() {
            self.cam_target = [
                (mn[0] + mx[0]) * 0.5,
                (mn[1] + mx[1]) * 0.5,
                (mn[2] + mx[2]) * 0.5,
            ];
            let span = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(mx[2] - mn[2]);
            self.cam_dist = (span * 2.5).clamp(1.0, self.max_cam_dist());
        } else {
            self.cam_target = [0.0, 0.0, 0.0];
            self.cam_dist = 12.0;
        }
    }

    /// Pan the view by a screen drag `(dx, dy)` in pixels — slides the camera target in
    /// the camera's own right/up plane (right = screen →, up = screen ↑). Scaled by
    /// distance so a drag covers a consistent fraction of the view at any zoom.
    ///
    /// Only the TARGET moves, so orientation and zoom are untouched — exactly what pan
    /// should do.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let (cp, sp) = (self.cam_pitch.cos(), self.cam_pitch.sin());
        let (cy, sy) = (self.cam_yaw.cos(), self.cam_yaw.sin());
        let fwd = Vec3::new(cp * cy, cp * sy, sp);
        let right = {
            let x = fwd.cross(Vec3::Z);
            if x.length() < 1e-4 { Vec3::X } else { x.normalize() }
        };
        let up = right.cross(fwd).normalize();
        let k = self.cam_dist * 0.0018;
        let mut t = Vec3::from(self.cam_target);
        // Screen-right drag moves the world LEFT under a fixed camera → target goes right;
        // screen-down drag → target goes up. Signs chosen so content follows the cursor.
        t += right * (-dx * k) + up * (dy * k);
        self.cam_target = t.into();
    }

    /// Snap the orbit camera to a standard view — the nav-gizmo action. Sets `(yaw,
    /// pitch)`; `cam_target`/`cam_dist` are left alone (Zoom-extents is the only thing
    /// that moves the target). `mvp` flips its up-vector near ±90° so Top/Bottom are
    /// stable even though the free-orbit drag clamps pitch to ±1.45.
    pub fn set_view(&mut self, v: StdView) {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
        let (yaw, pitch) = match v {
            StdView::Top    => (-FRAC_PI_2,  FRAC_PI_2),
            StdView::Bottom => (-FRAC_PI_2, -FRAC_PI_2),
            StdView::Front  => (-FRAC_PI_2,  0.0),
            StdView::Back   => ( FRAC_PI_2,  0.0),
            StdView::Right  => ( 0.0,        0.0),
            StdView::Left   => ( PI,         0.0),
            StdView::Iso    => (-FRAC_PI_4,  0.6155), // 35.26° — the classic SE isometric
        };
        self.cam_yaw = yaw;
        self.cam_pitch = pitch;
        self.ortho = true; // standard views are orthographic (true CAD Top/Front/…)
    }

    /// Dolly the camera by a factor: `<1` zooms in (closer), `>1` zooms out. The same
    /// clamp as the scroll wheel, so command / gizmo / wheel all agree.
    pub fn zoom_by(&mut self, factor: f32) {
        let max = self.max_cam_dist();
        self.cam_dist = (self.cam_dist * factor).clamp(0.4, max);
    }

    /// Reframe the camera to a screen rectangle — the 2D "zoom window", in 3D. Moves the
    /// target under the box centre (on the target's view plane) and dollies in so the box
    /// fills the viewport height. `vp` is the viewport rect; `p0`,`p1` the drag corners.
    /// Snapshot the camera so `zoom previous` can restore it.
    pub fn zoom_save_prev(&mut self) {
        self.cam_prev = Some([
            self.cam_yaw, self.cam_pitch, self.cam_dist,
            self.cam_target[0], self.cam_target[1], self.cam_target[2],
        ]);
    }

    /// Restore the camera saved before the last zoom (`zoom previous`). No-op if none.
    pub fn zoom_restore_previous(&mut self) {
        if let Some(p) = self.cam_prev.take() {
            self.cam_yaw = p[0];
            self.cam_pitch = p[1];
            self.cam_dist = p[2];
            self.cam_target = [p[3], p[4], p[5]];
        }
    }

    pub fn zoom_window(&mut self, vp: egui::Rect, p0: egui::Pos2, p1: egui::Pos2) {
        self.zoom_save_prev();
        let bh = (p1.y - p0.y).abs().max(1.0);
        let bc = egui::pos2((p0.x + p1.x) * 0.5, (p0.y + p1.y) * 0.5);
        // box centre → normalised device coords (y up)
        let ndc_x = (bc.x - vp.center().x) / (vp.width() * 0.5).max(1.0);
        let ndc_y = -(bc.y - vp.center().y) / (vp.height() * 0.5).max(1.0);
        // camera basis — matches `light3d::mvp`
        let (cp, sp) = (self.cam_pitch.cos(), self.cam_pitch.sin());
        let (cy, sy) = (self.cam_yaw.cos(), self.cam_yaw.sin());
        let fwd = -Vec3::new(cp * cy, cp * sy, sp); // eye → target
        let up_world = if sp.abs() > 0.999 { Vec3::Y } else { Vec3::Z };
        let right = fwd.cross(up_world).normalize();
        let up = right.cross(fwd).normalize();
        // world half-extents on the target's view plane (45° vertical FOV, as in mvp)
        let half_h = (45f32.to_radians() * 0.5).tan() * self.cam_dist;
        let half_w = half_h * (vp.width() / vp.height().max(1.0));
        let t = Vec3::from(self.cam_target) + right * (ndc_x * half_w) + up * (ndc_y * half_h);
        self.cam_target = [t.x, t.y, t.z];
        let factor = (bh / vp.height().max(1.0)).clamp(0.02, 1.0);
        let max = self.max_cam_dist();
        self.cam_dist = (self.cam_dist * factor).clamp(0.4, max);
    }

    /// One-line screen-zoom status for the session recorder: how zoomed-in the camera is
    /// (`dist`), what it is centred on (`target`), and the orbit angles. Comparing this
    /// before vs after a zoom is how we tell whether the zoom actually did anything.
    pub fn zoom_status(&self) -> String {
        format!(
            "dist={:.2} target=({:.1},{:.1},{:.1}) yaw={:.0}° pitch={:.0}°",
            self.cam_dist,
            self.cam_target[0], self.cam_target[1], self.cam_target[2],
            self.cam_yaw.to_degrees(), self.cam_pitch.to_degrees(),
        )
    }

    /// The SELECTED features' own geometry, as a mesh.
    ///
    /// `cached` is the fused CSG result — after booleans, individual features have no
    /// identity in it, so the selected solid's triangles cannot be picked back out.
    /// This evaluates just the selection into its own mesh, which is what both the
    /// selection SHADE and the modifier GHOST draw.
    ///
    /// **Cached on the selection**, because csgrs walks a BSP per boolean — doing this
    /// per frame is precisely the lag source the whole panel is careful to avoid.
    fn ensure_sel_mesh(&mut self) {
        if self.sel_key == self.selection {
            return;
        }
        let mut m = Model::default();
        for id in &self.selection {
            if let Some(f) = self.model.features.iter().find(|f| f.id == *id) {
                let mut f = *f;
                f.op = BoolOp::Union; // isolated: a lone Difference would erase itself
                m.push_feature(f);
            }
        }
        self.sel_mesh = m.eval();
        self.sel_key = self.selection.clone();
    }

    /// Selection SHADE — the selected solids tinted in place (§0.6's "selected
    /// dobjects get a shade"). Drawn in the translucent overlay pass, which uses
    /// `depth_func(LEQUAL)` so coincident geometry tints instead of z-fighting.
    pub fn shade_verts(&self) -> Vec<V3> {
        if self.selection.is_empty() || self.modify.as_ref().is_some_and(|m| m.has_base()) {
            return Vec::new(); // once the base is picked the GHOST is the feedback
        }
        let c = [0.0, 0.75, 0.95];
        self.sel_mesh.positions.iter().map(|p| v(Vec3::from(*p), c)).collect()
    }

    /// GHOST — the selected solids under the op's LIVE transform, at the constrained
    /// cursor (spec §0.6: "while moving it shows the path").
    fn ghost_verts(&self, c: [f32; 3], xf: impl Fn(Vec3) -> Vec3) -> Vec<V3> {
        self.sel_mesh.positions.iter().map(|p| v(xf(Vec3::from(*p)), c)).collect()
    }

    /// The live ghost for the running op. Colours per §0.6: Move accent(255,200,100) ·
    /// Copy green(150,230,170) · Rotate/Scale white · Mirror violet(200,160,255).
    pub fn modify_ghost(&self, cursor_world: Vec3, card: bool) -> Vec<V3> {
        use cad_solid::modify::{rot_about, scale_about, ModifyOp};
        let Some(md) = &self.modify else { return Vec::new() };
        let plane = Plane::default();
        let Some(base) = md.anchor_world(&plane) else { return Vec::new() };
        match md.op {
            ModifyOp::Move | ModifyOp::Copy => {
                let d = cursor_world - base;
                let d = if card { card_lock_world(d) } else { d };
                let c = if md.op == ModifyOp::Move { [1.0, 0.78, 0.39] } else { [0.59, 0.90, 0.67] };
                self.ghost_verts(c, |p| p + d)
            }
            ModifyOp::Rotate => {
                let a = md.preview_angle(&plane, cursor_world, card).unwrap_or(0.0);
                self.ghost_verts([0.92, 0.92, 0.98], |p| rot_about(p, base, Vec3::Z, a))
            }
            ModifyOp::Scale => {
                let k = md.preview_factor(&plane, cursor_world).unwrap_or(1.0);
                self.ghost_verts([0.80, 0.95, 0.82], |p| scale_about(p, base, k))
            }
            ModifyOp::Mirror => {
                let line = (cursor_world - base).normalize_or_zero();
                let n = Vec3::Z.cross(line).normalize_or_zero();
                if n.length_squared() < 1e-9 { return Vec::new(); }
                self.ghost_verts([0.78, 0.63, 1.0], |p| p - n * (2.0 * (p - base).dot(n)))
            }
        }
    }

    /// Cancel any queued/running 3D op.
    pub fn abort_op(&mut self) {
        self.modify = None;
        self.queued = None;
        self.status.clear();
    }

    /// Flat-shaded triangle soup for the evaluated solid.
    pub fn scene_verts(&self) -> Vec<V3> {
        let default_base = [0.62, 0.68, 0.78];
        let default_n = [0.0f32, 0.0, 1.0];
        let mut out = Vec::with_capacity(self.cached.positions.len());
        // Each triangle is coloured by, in priority order: its SURFACE (a painted face),
        // then its body's feature colour, then the neutral default. `face_ids` has one
        // entry per triangle.
        for (i, tri) in self.cached.positions.chunks_exact(3).enumerate() {
            let fid = self.cached.face_ids.get(i).copied();
            // HIDE CEILINGS: drop triangles that belong to a tracked ceiling. Done here (at
            // render time, keyed on the triangle's feature id) rather than by re-evaluating
            // the model — so the toggle is instant and cannot silently fail.
            if self.hide_ceilings {
                if let Some(id) = fid {
                    // Hide the whole ceiling slab. The surrounding WALL is a separate solid
                    // (an annulus, once the room is carved from its building) whose own top
                    // stays — so removing the ceiling opens the room but leaves the wall
                    // capped. No fragile per-triangle clipping of a disc cap.
                    if self.is_hidden_ceiling(id) {
                        continue;
                    }
                }
            }
            // CUTAWAY: drop any triangle lying ENTIRELY above the cut plane, so ceilings,
            // roofs and upper floors vanish and you can see into the structure. Walls and
            // floors that cross the plane stay whole.
            if self.cutaway && tri.iter().all(|p| p[2] >= self.cutaway_z - 1e-4) {
                continue;
            }
            let base = fid
                .and_then(|id| {
                    let key = surface_key(id, tri[0], tri[1], tri[2]);
                    self.surface_color.get(&key).copied()
                })
                .or_else(|| fid.and_then(|id| self.feature_color.get(&id).copied()))
                .unwrap_or(default_base);
            for (k, p) in tri.iter().enumerate() {
                let n = self.cached.normals.get(i * 3 + k).copied().unwrap_or(default_n);
                out.push(v(Vec3::from(*p), shade(base, Vec3::from(n))));
            }
        }
        out
    }

    /// Placed furniture as shaded triangles, ready to draw alongside the scene. Each
    /// instance's mesh is posed by its `pos` / `scale` / `rot` (3-axis) and tinted by its colour.
    pub fn furniture_verts(&self) -> Vec<V3> {
        let mut out = Vec::new();
        for inst in &self.furniture {
            let Some(asset) = self.furniture_lib.get(inst.asset) else { continue };
            let s = inst.scale;
            let rm = inst.rot_mat();
            let pos = Vec3::from(inst.pos);
            for (i, p) in asset.positions.iter().enumerate() {
                // scale → 3-axis rotate → translate (normals rotate the same way)
                let lp = Vec3::new(p[0] * s, p[1] * s, p[2] * s);
                let wp = rm * lp + pos;
                let n = asset.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
                let wn = rm * Vec3::from(n);
                out.push(v(wp, shade_furniture(inst.color, wn)));
            }
        }
        out
    }

    /// Grid on the construction plane + a cyan AABB around each selected feature.
    pub fn overlay_lines(&self) -> Vec<V3> {
        let mut out = Vec::new();
        let g = [0.22, 0.25, 0.30];
        let n = 10i32;
        let s = 1.0f32;
        for i in -n..=n {
            let t = i as f32 * s;
            let e = n as f32 * s;
            seg(&mut out, Vec3::new(t, -e, 0.0), Vec3::new(t, e, 0.0), g);
            seg(&mut out, Vec3::new(-e, t, 0.0), Vec3::new(e, t, 0.0), g);
        }
        for id in &self.selection {
            if let Some(f) = self.model.features.iter().find(|f| f.id == *id) {
                let (mn, mx) = f.world_aabb();
                aabb_lines(&mut out, mn, mx, [0.0, 0.9, 1.0]);
            }
        }
        // Highlight a selected furniture instance the same way.
        if let Some(i) = self.sel_furniture {
            if let Some((mn, mx)) = self.furniture_aabb(i) {
                aabb_lines(&mut out, mn, mx, [1.0, 0.75, 0.2]);
            }
        }
        out
    }

    /// Screen cursor → world ray (origin, unit dir), by inverting the MVP.
    fn ray(cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> (Vec3, Vec3) {
        let ndc_x = 2.0 * (cursor.x - rect.left()) / rect.width().max(1.0) - 1.0;
        let ndc_y = 1.0 - 2.0 * (cursor.y - rect.top()) / rect.height().max(1.0);
        let inv = Mat4::from_cols_array(mvp).inverse();
        let near = inv.project_point3(Vec3::new(ndc_x, ndc_y, -1.0));
        let far = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        (near, (far - near).normalize_or_zero())
    }

    /// Ray-pick the front-most FEATURE (solid) under `cursor`, by world AABB.
    /// This is what the LEFT button does in the 3D view — selection, never camera.
    pub fn pick_feature(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<u32> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        // Ray-test the actual TRIANGLES of each visible body, not its bounding box — a big
        // building's AABB encloses a ceiling sitting on it, so an AABB pick could never
        // reach the ceiling. Skip Difference/Intersection features: those are cutters
        // (a room is a void), not clickable surfaces.
        //
        // Tiebreak on near-equal depth by the SMALLER body, so the specific object on top
        // (a ceiling slab) wins over the large solid it overlaps (the building).
        let mut best: Option<(f32, f32, u32)> = None; // (t, aabb volume, id)
        for f in &self.model.features {
            if f.op != cad_solid::BoolOp::Union {
                continue;
            }
            // A HIDDEN ceiling is not drawn, so it must not be clickable either — otherwise
            // you would select the invisible ceiling instead of what is behind it.
            if self.hide_ceilings && self.is_hidden_ceiling(f.id) {
                continue;
            }
            let tris = self.model.feature_world_positions(f);
            let mut ft: Option<f32> = None;
            for c in tris.chunks_exact(3) {
                let (a, b, cc) = (Vec3::from(c[0]), Vec3::from(c[1]), Vec3::from(c[2]));
                if let Some(t) = cad_solid::ray_triangle(orig, dir, a, b, cc) {
                    if ft.map_or(true, |x| t < x) {
                        ft = Some(t);
                    }
                }
            }
            if let Some(t) = ft {
                let (mn, mx) = f.world_aabb();
                let s = mx - mn;
                let vol = s.x.abs() * s.y.abs() * s.z.abs();
                let better = match best {
                    None => true,
                    Some((bt, bv, _)) => t < bt - 1e-3 || (t < bt + 1e-3 && vol < bv),
                };
                if better {
                    best = Some((t, vol, f.id));
                }
            }
        }
        best.map(|(_, _, id)| id)
    }

    /// Ray-pick the front-most solid FACE under `cursor` and return a sketch [`Frame`]
    /// sitting on it — the basis for sketch-on-face. `None` if the ray misses.
    pub fn pick_face(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<Frame> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
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
        best.map(|(_, p, n)| Frame::from_point_normal(p, n))
    }

    /// Unproject `cursor` onto the active construction plane (XY at z=0) — the 3D
    /// analog of the 2D canvas's screen→world. `None` if the ray is parallel to it.
    pub fn cursor_on_plane(&self, cursor: egui::Pos2, rect: egui::Rect, mvp: &[f32; 16]) -> Option<Vec3> {
        let (orig, dir) = Self::ray(cursor, rect, mvp);
        let n = Vec3::Z;
        let denom = dir.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = -orig.dot(n) / denom;
        (t >= 0.0).then(|| orig + dir * t)
    }

    /// OSNAP for 3D picks — the nearest solid mesh VERTEX whose screen projection is
    /// within the aperture. Mirrors the 2D pickbox: snapping to a real corner is what
    /// makes "move this corner to that corner" exact instead of eyeballed.
    pub fn snap_vertex(
        &self,
        cursor: egui::Pos2,
        rect: egui::Rect,
        mvp: &[f32; 16],
    ) -> Option<(Vec3, egui::Pos2)> {
        let m = Mat4::from_cols_array(mvp);
        let aperture = 12.0f32;
        let mut best: Option<(f32, Vec3, egui::Pos2)> = None;
        for p in &self.cached.positions {
            let w = Vec3::from(*p);
            let ndc = m.project_point3(w);
            if !(-1.0..=1.0).contains(&ndc.z) {
                continue;
            }
            let sx = rect.left() + (ndc.x * 0.5 + 0.5) * rect.width();
            let sy = rect.top() + (0.5 - ndc.y * 0.5) * rect.height();
            let sp = egui::pos2(sx, sy);
            let d = sp.distance(cursor);
            if d <= aperture && best.map_or(true, |(bd, _, _)| d < bd) {
                best = Some((d, w, sp));
            }
        }
        best.map(|(_, w, sp)| (w, sp))
    }

    /// The ground (XY) plane at the origin — the fallback sketch surface when the
    /// right-click misses a solid, so you can always start drawing.
    pub fn ground_frame() -> Frame {
        Frame::from_point_normal(Vec3::ZERO, Vec3::Z)
    }

    /// The solid's FEATURE EDGES projected onto `frame`'s (u,v) plane — a clean line
    /// drawing of the 3D object for use as a reference underlay when sketching on a face.
    ///
    /// An edge is a "feature" edge if it is shared by ONLY ONE triangle (a true boundary) or
    /// by two triangles whose normals differ by more than ~20° (a real crease). Interior
    /// tessellation edges — the diagonals that split a flat quad — are dropped, so what you
    /// get is the object's outline and its hard edges, not a triangle-soup mess.
    pub fn frame_reference_edges(&self, frame: &Frame) -> Vec<[Vec2; 2]> {
        use std::collections::HashMap;
        // Quantise a world position so the two triangles sharing an edge hash together.
        let q = |p: [f32; 3]| -> (i64, i64, i64) {
            const S: f32 = 1.0e4;
            ((p[0] * S).round() as i64, (p[1] * S).round() as i64, (p[2] * S).round() as i64)
        };
        // undirected edge key → (endpoint a, endpoint b, adjacent triangle normals)
        let mut map: HashMap<((i64, i64, i64), (i64, i64, i64)), ([f32; 3], [f32; 3], Vec<Vec3>)> =
            HashMap::new();
        for tri in self.cached.positions.chunks_exact(3) {
            let (a, b, c) = (Vec3::from(tri[0]), Vec3::from(tri[1]), Vec3::from(tri[2]));
            let n = (b - a).cross(c - a).normalize_or_zero();
            for (p0, p1) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let (k0, k1) = (q(p0), q(p1));
                let key = if k0 <= k1 { (k0, k1) } else { (k1, k0) };
                map.entry(key).or_insert((p0, p1, Vec::new())).2.push(n);
            }
        }
        let cos_thresh = 20.0_f32.to_radians().cos();
        let mut out = Vec::new();
        for (_, (a, b, normals)) in map {
            let is_feature = normals.len() == 1
                || normals.iter().enumerate().any(|(i, na)| {
                    normals[i + 1..].iter().any(|nb| na.dot(*nb) < cos_thresh)
                });
            if is_feature {
                out.push([frame.to_uv(Vec3::from(a)), frame.to_uv(Vec3::from(b))]);
            }
        }
        out
    }

    /// Every sketch's geometry, lifted from its frame's `(u,v)` back into world space,
    /// as GL_LINES. This is what makes 2D work drawn on a plane visible in 3D.
    pub fn sketch_lines(&self) -> Vec<V3> {
        let mut out = Vec::new();
        for (i, sk) in self.model.sketches.iter().enumerate() {
            // the sketch being edited right now is drawn hot, the others cool
            let active = self.session.as_ref().is_some_and(|s| s.idx == i);
            let c = if active { [1.0, 0.62, 0.12] } else { [0.55, 0.62, 0.72] };
            for d in &sk.doc.dobjects {
                for poly in cad_solid::geom_outlines(&d.geom) {
                    for w in poly.windows(2) {
                        seg(
                            &mut out,
                            sk.frame.from_uv(Vec2::new(w[0].x, w[0].y)),
                            sk.frame.from_uv(Vec2::new(w[1].x, w[1].y)),
                            c,
                        );
                    }
                }
            }
            // frame axes, so an empty sketch plane is still visible
            if active {
                let o = sk.frame.origin;
                seg(&mut out, o, o + sk.frame.u * 1.5, [1.0, 0.3, 0.3]);
                seg(&mut out, o, o + sk.frame.v * 1.5, [0.3, 1.0, 0.3]);
            }
        }
        out
    }

    /// The ACTIVE sketch's LIVE geometry — which lives in the app's swapped-in document,
    /// passed in here — lifted onto its frame, so what you draw on a face appears in the 3D
    /// view immediately (2D↔3D linked). `sketch_lines` can't show it because the active
    /// sketch's own `doc` is empty while it is being edited.
    pub fn live_sketch_lines(&self, doc: &cad_kernel::Document) -> Vec<V3> {
        let mut out = Vec::new();
        let Some(session) = self.session.as_ref() else { return out };
        let Some(sk) = self.model.sketches.get(session.idx) else { return out };
        let c = [1.0, 0.62, 0.12]; // hot — the sketch you are drawing right now
        for d in &doc.dobjects {
            for poly in cad_solid::geom_outlines(&d.geom) {
                for w in poly.windows(2) {
                    seg(
                        &mut out,
                        sk.frame.from_uv(Vec2::new(w[0].x, w[0].y)),
                        sk.frame.from_uv(Vec2::new(w[1].x, w[1].y)),
                        c,
                    );
                }
            }
        }
        out
    }

    pub fn tri_count(&self) -> usize {
        self.cached.tri_count()
    }

    pub fn feature_count(&self) -> usize {
        self.model.features.len()
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    fn view(st: &FactoryState, rect: egui::Rect) -> [f32; 16] {
        let aspect = rect.width() / rect.height();
        crate::light3d::mvp(st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target, aspect, st.ortho)
    }

    /// The user reports "3D dobject not selecting". Picking is pure math (screen →
    /// ray → AABB), so it CAN be tested headlessly even though the click itself
    /// needs a live egui pointer. If this passes, selection math is sound and the
    /// fault is in reachability/routing, not geometry.
    #[test]
    fn clicking_the_centre_of_the_view_picks_the_solid_there() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        st.fit(); // aim the camera at the solid, as ⌖ Frame does
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = view(&st, rect);
        let hit = st.pick_feature(rect.center(), rect, &mvp);
        assert!(hit.is_some(), "a ray through the centre must hit the centred solid");
        assert_eq!(hit.unwrap(), st.model.features[0].id);
    }

    /// The face-sketch reference must be a clean OUTLINE of the object (its real edges),
    /// not a triangle-soup: a box projects to its 12 edges, with the per-face tessellation
    /// diagonals dropped.
    #[test]
    fn frame_reference_is_the_box_outline_not_triangle_soup() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        let edges = st.frame_reference_edges(&FactoryState::ground_frame());
        assert_eq!(edges.len(), 12, "a box projects to its 12 feature edges, got {}", edges.len());
    }

    /// …and a ray into empty space must MISS (else everything is always selected).
    #[test]
    fn clicking_far_from_the_solid_misses() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        st.fit();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = view(&st, rect);
        let corner = egui::pos2(rect.left() + 2.0, rect.top() + 2.0);
        assert!(st.pick_feature(corner, rect, &mvp).is_none(), "corner ray must miss");
    }

    /// Face-pick (the right-click → "Draw on this face" path) must land ON the solid.
    #[test]
    fn face_pick_returns_a_frame_on_the_solid() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        st.fit();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mvp = view(&st, rect);
        let f = st.pick_face(rect.center(), rect, &mvp);
        assert!(f.is_some(), "centre ray must hit a face of the centred solid");
    }
}

#[cfg(test)]
mod outline_tests {
    use super::*;

    fn ell() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0), Vec2::new(6.0, 0.0), Vec2::new(6.0, 3.0),
            Vec2::new(3.0, 3.0), Vec2::new(3.0, 6.0), Vec2::new(0.0, 6.0),
        ]
    }

    /// The Building-outline tool now has a primitive behind it — the greyed row's reason
    /// for being disabled is gone.
    #[test]
    fn an_l_shaped_building_is_exact_not_a_bounding_box() {
        let mut st = FactoryState::default();
        st.add_building_outline(&ell(), 4.0).expect("an L is a valid outline");
        st.recompute();
        let (mn, mx) = st.cached.bounds().expect("the building must have geometry");
        assert!((mx[2] - mn[2] - 4.0).abs() < 1e-3, "it rises to the given height");
        // A bounding-box approximation would fill the whole 6×6 square. The L's area is
        // 27 of that 36, so an exact extrusion has strictly less volume.
        let tris = st.cached.tri_count();
        assert!(tris > 12, "an L has more faces than a box ({tris} triangles)");
    }

    /// A building is built on the ACTIVE storey, like every other new solid.
    #[test]
    fn a_building_rises_from_the_active_storey() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let base = st.active_base_z();
        let id = st.add_building_outline(&ell(), 3.0).unwrap();
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert!((f.placement.lift - base).abs() < 1e-4);
    }

    /// A bad outline is refused WITH ITS REASON and stores nothing — the app turns each
    /// variant into a message, so a silent failure would leave the user guessing.
    #[test]
    fn a_crossed_outline_is_refused_with_its_reason() {
        let mut st = FactoryState::default();
        let bowtie = vec![
            Vec2::new(0.0, 0.0), Vec2::new(4.0, 4.0),
            Vec2::new(4.0, 0.0), Vec2::new(0.0, 4.0),
        ];
        assert_eq!(
            st.add_building_outline(&bowtie, 3.0),
            Err(cad_solid::ProfileError::SelfIntersecting)
        );
        assert!(st.model.features.is_empty(), "nothing may be built from a bad outline");
        assert!(st.model.profiles.is_empty(), "and no profile may be left behind");
    }

    /// A building survives save/reopen — the profile table rides in the same `Model` the
    /// sidecar already stores.
    #[test]
    fn a_building_outline_round_trips_through_the_sidecar() {
        let mut st = FactoryState::default();
        st.add_building_outline(&ell(), 4.0).unwrap();
        let json = serde_json::to_string(&st.to_persist()).unwrap();
        let back: crate::simlux_io::FactoryDoc = serde_json::from_str(&json).unwrap();

        let mut re = FactoryState::default();
        re.apply_persist(back);
        assert_eq!(re.model.profiles.len(), 1, "the outline itself must survive");
        re.recompute();
        assert!(re.cached.tri_count() > 0, "and still build geometry after reload");
    }
}

#[cfg(test)]
mod pan_tests {
    use super::*;

    /// Pan moves ONLY the target — orientation and zoom must be untouched.
    #[test]
    fn pan_moves_the_target_not_the_orientation_or_zoom() {
        let mut st = FactoryState::default();
        let (yaw, pitch, dist) = (st.cam_yaw, st.cam_pitch, st.cam_dist);
        let t0 = st.cam_target;
        st.pan(40.0, -25.0);
        assert_ne!(st.cam_target, t0, "the target must move");
        assert_eq!(st.cam_yaw, yaw, "pan must not orbit");
        assert_eq!(st.cam_pitch, pitch);
        assert_eq!(st.cam_dist, dist, "pan must not zoom");
    }

    /// A zero drag is a no-op.
    #[test]
    fn a_zero_pan_changes_nothing() {
        let mut st = FactoryState::default();
        let t0 = st.cam_target;
        st.pan(0.0, 0.0);
        assert_eq!(st.cam_target, t0);
    }

    /// Panning right then left by the same amount returns to where it started.
    #[test]
    fn opposite_pans_cancel() {
        let mut st = FactoryState::default();
        let t0 = st.cam_target;
        st.pan(30.0, 15.0);
        st.pan(-30.0, -15.0);
        let d = glam::Vec3::from(st.cam_target) - glam::Vec3::from(t0);
        assert!(d.length() < 1e-4, "a pan and its inverse must cancel");
    }
}

#[cfg(test)]
mod furniture_and_color_tests {
    use super::*;

    fn tetra() -> crate::mesh_io::ObjMesh {
        crate::mesh_io::parse_obj(
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\nf 1 2 4\nf 1 3 4\nf 2 3 4\n",
        )
    }

    /// An imported asset enters the library; placing it adds an instance that renders.
    #[test]
    fn import_and_place_furniture_renders() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("chair".into(), tetra());
        assert_eq!(st.furniture_lib.len(), 1);
        st.place_furniture(idx, Vec3::new(2.0, 3.0, 0.0));
        assert_eq!(st.furniture.len(), 1);
        assert!(!st.furniture_verts().is_empty(), "placed furniture must produce geometry");
        assert!((st.furniture[0].pos[0] - 2.0).abs() < 1e-4, "placed at the given point");
    }

    /// The library persists across save/reload, and instances keep their asset/pose.
    #[test]
    fn furniture_round_trips_through_the_sidecar() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("lamp".into(), tetra());
        st.place_furniture(idx, Vec3::new(1.0, 1.0, 0.0));
        let json = serde_json::to_string(&st.to_persist()).unwrap();
        let back: crate::simlux_io::FactoryDoc = serde_json::from_str(&json).unwrap();

        let mut re = FactoryState::default();
        re.apply_persist(back);
        assert_eq!(re.furniture_lib.len(), 1, "the imported mesh is stored in the project");
        assert_eq!(re.furniture.len(), 1);
        assert!(!re.furniture_verts().is_empty(), "and still renders after reload");
    }

    /// Furniture is selectable, and selecting it clears the feature selection (they are
    /// mutually exclusive — the gizmo/properties act on one thing).
    #[test]
    fn furniture_selection_is_exclusive_with_features() {
        let mut st = FactoryState::default();
        st.add_box();                          // selects the feature
        assert!(!st.selection.is_empty());
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);   // selects the furniture
        assert_eq!(st.sel_furniture, Some(0));
        assert!(st.selection.is_empty(), "selecting furniture clears the feature selection");
    }

    /// Furniture rotates about all three axes: yaw 90°/Z sends local +X→+Y; pitch 90°/X
    /// sends +Y→+Z. (Single-axis cases hold regardless of Euler order.)
    #[test]
    fn furniture_rotation_is_three_axis() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        st.furniture[0].rot = [0.0, 0.0, 90.0];
        let p = st.furniture_point(&st.furniture[0], [1.0, 0.0, 0.0]);
        assert!(p.x.abs() < 1e-4 && (p.y - 1.0).abs() < 1e-4, "yaw 90° sends +X→+Y, got {p:?}");
        st.furniture[0].rot = [90.0, 0.0, 0.0];
        let q = st.furniture_point(&st.furniture[0], [0.0, 1.0, 0.0]);
        assert!((q.z - 1.0).abs() < 1e-4, "pitch 90° sends +Y→+Z, got {q:?}");
    }

    /// The 3-axis furniture rotation survives the sidecar (Z via `rot_deg`, X/Y via `rot_xy`).
    #[test]
    fn furniture_three_axis_rotation_survives_the_sidecar() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        st.furniture[0].rot = [12.0, 34.0, 56.0];
        let json = serde_json::to_string(&st.to_persist()).unwrap();
        let back: crate::simlux_io::FactoryDoc = serde_json::from_str(&json).unwrap();
        let mut re = FactoryState::default();
        re.apply_persist(back);
        let r = re.furniture[0].rot;
        assert!((r[0] - 12.0).abs() < 1e-3 && (r[1] - 34.0).abs() < 1e-3 && (r[2] - 56.0).abs() < 1e-3,
            "3-axis rot round-trips, got {r:?}");
    }

    /// A feature's local rotation is settable, reads back, and the model still meshes.
    #[test]
    fn feature_rotation_setter_round_trips() {
        let mut st = FactoryState::default();
        st.add_box();
        let id = st.selected_single().unwrap();
        st.set_feature_rotation(id, 2, 45.0); // spin (about normal)
        st.set_feature_rotation(id, 0, 30.0); // pitch (about u)
        assert_eq!(st.feature_rotation(id), Some([30.0, 0.0, 45.0]));
        st.recompute();
        assert!(!st.cached.positions.is_empty(), "rotated feature still produces a mesh");
    }

    /// The gizmo drives furniture: move_selection shifts the selected instance's position,
    /// and its AABB (what the gizmo hangs off) follows.
    #[test]
    fn move_selection_moves_selected_furniture() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        let c0 = st.selection_center().unwrap();
        st.move_selection(Vec3::new(3.0, -2.0, 1.0));
        let c1 = st.selection_center().unwrap();
        assert!((c1 - c0 - Vec3::new(3.0, -2.0, 1.0)).length() < 1e-3);
    }

    /// Scaling furniture grows its instance scale (and its bounds).
    #[test]
    fn scale_selection_scales_furniture() {
        let mut st = FactoryState::default();
        let idx = st.add_furniture_asset("x".into(), tetra());
        st.place_furniture(idx, Vec3::ZERO);
        let (mn0, mx0) = st.selection_aabb().unwrap();
        st.scale_selection(2.0);
        let (mn1, mx1) = st.selection_aabb().unwrap();
        let span0 = (mx0 - mn0).length();
        let span1 = (mx1 - mn1).length();
        assert!(span1 > span0 * 1.8, "the instance must grow when scaled up");
        assert!((st.furniture[0].scale - 2.0).abs() < 1e-4);
    }

    /// Scaling a selected SOLID grows its primitive about its centre.
    #[test]
    fn scale_selection_scales_a_solid() {
        let mut st = FactoryState::default();
        st.add_box();
        let (mn0, mx0) = st.selection_aabb().unwrap();
        st.scale_selection(2.0);
        let (mn1, mx1) = st.selection_aabb().unwrap();
        assert!((mx1 - mn1).length() > (mx0 - mn0).length() * 1.8);
    }

    /// Cutaway drops triangles ENTIRELY above the cut plane (top faces) and keeps those
    /// crossing it (walls) — a reliable "see inside" that needs no ceiling tagging. And it
    /// is view-only (the mesh itself is unchanged).
    #[test]
    fn cutaway_hides_geometry_above_the_plane() {
        let mut st = FactoryState::default();
        // A 2×2×4 box spanning z = 0..4.
        st.model.push(cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement::default(), Primitive::Box { w: 2.0, d: 2.0, h: 4.0 });
        st.recompute();
        let full = st.scene_verts().len();
        let mesh_tris = st.cached.tri_count();

        st.cutaway = true;
        st.cutaway_z = 2.0;
        assert!(st.scene_verts().len() < full, "the top cap above the plane is dropped");
        assert!(!st.scene_verts().is_empty(), "the walls crossing the plane remain");
        assert_eq!(st.cached.tri_count(), mesh_tris, "cutaway is view-only, mesh unchanged");
    }

    /// A HIDDEN ceiling must not be pickable — otherwise you select the invisible ceiling
    /// instead of what is behind/below it.
    #[test]
    fn a_hidden_ceiling_is_not_pickable() {
        let mut st = FactoryState::default();
        st.add_building_outline(&[
            Vec2::new(0.0, 0.0), Vec2::new(6.0, 0.0), Vec2::new(6.0, 6.0), Vec2::new(0.0, 6.0),
        ], 3.0).unwrap();
        st.add_room(&[
            Vec2::new(1.0, 1.0), Vec2::new(5.0, 1.0), Vec2::new(5.0, 5.0), Vec2::new(1.0, 5.0),
        ]).unwrap();
        let cid = *st.ceilings.iter().next().expect("a ceiling was made");
        st.recompute();

        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        st.set_view(StdView::Top);
        st.fit();
        let mvp = crate::light3d::mvp(st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target, 800.0/600.0, st.ortho);

        // Straight down the middle from the top: without hiding, the ceiling can be hit.
        st.hide_ceilings = true;
        st.recompute();
        let hit = st.pick_feature(rect.center(), rect, &mvp);
        assert_ne!(hit, Some(cid), "a hidden ceiling must never be the pick result");
    }

    /// Painting a surface stores a per-surface colour that scene_verts uses.
    #[test]
    fn painting_a_surface_colours_only_that_face() {
        let mut st = FactoryState::default();
        st.add_box();                       // 2×2×1 box, feature id 1
        st.recompute();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        st.set_view(StdView::Top);
        st.fit();
        let mvp = crate::light3d::mvp(st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target, 800.0/600.0, st.ortho);

        // From the top, the ray hits the top face — paint it red.
        assert!(st.paint_surface(rect.center(), rect, &mvp, [1.0, 0.0, 0.0]), "the top face must be hit");
        assert_eq!(st.surface_color.len(), 1, "one surface painted");
        // Some triangles now render red; not all (only the painted face).
        let verts = st.scene_verts();
        assert!(verts.iter().any(|v| v.r > 0.8 && v.g < 0.3), "the painted face is red");
        assert!(verts.iter().any(|v| !(v.r > 0.8 && v.g < 0.3)), "other faces are not");
    }

    /// A colour assigned to a feature tints that body's triangles (and only tints — it
    /// does not change the geometry).
    #[test]
    fn feature_colour_tints_only_that_body() {
        let mut st = FactoryState::default();
        st.add_box();                    // feature id 1
        st.recompute();
        let plain = st.scene_verts();
        let id = st.selected_single().unwrap();
        st.feature_color.insert(id, [1.0, 0.0, 0.0]);
        let tinted = st.scene_verts();
        assert_eq!(plain.len(), tinted.len(), "colour must not change triangle count");
        assert!(
            tinted.iter().any(|v| v.r > v.g && v.r > v.b),
            "the coloured body must render reddish"
        );
    }

    /// A feature with no assigned colour renders in the neutral (not blank).
    #[test]
    fn uncoloured_features_use_the_default() {
        let mut st = FactoryState::default();
        st.add_box();
        st.recompute();
        assert!(st.scene_verts().iter().all(|v| v.r > 0.0 || v.g > 0.0 || v.b > 0.0));
    }
}

#[cfg(test)]
mod gizmo_and_props_tests {
    use super::*;

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
    }
    fn view(st: &FactoryState) -> [f32; 16] {
        crate::light3d::mvp(
            st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target,
            rect().width() / rect().height(), st.ortho,
        )
    }
    fn one_box() -> FactoryState {
        let mut st = FactoryState::default();
        st.add_box();          // id 1, selected
        st.recompute();
        st.fit();
        st
    }

    /// The selection centre is the AABB centre — the gizmo hangs off it.
    #[test]
    fn selection_center_is_the_aabb_center() {
        let st = one_box();
        let (mn, mx) = st.selection_aabb().expect("a selected box has bounds");
        assert_eq!(st.selection_center().unwrap(), (mn + mx) * 0.5);
    }

    /// Moving the selection shifts its centre by exactly the delta, in place (id kept).
    #[test]
    fn move_selection_shifts_the_center_and_keeps_ids() {
        let mut st = one_box();
        let c0 = st.selection_center().unwrap();
        let ids0 = st.selection.clone();
        st.move_selection(Vec3::new(2.0, -1.0, 3.0));
        let c1 = st.selection_center().unwrap();
        assert!((c1 - c0 - Vec3::new(2.0, -1.0, 3.0)).length() < 1e-3);
        assert_eq!(st.selection, ids0, "a move must not renumber the selection");
    }

    /// A position field writes one axis of the world origin and leaves the others.
    #[test]
    fn setting_one_position_axis_leaves_the_others() {
        let mut st = one_box();
        let id = st.selected_single().unwrap();
        let o0 = st.model.features.iter().find(|f| f.id == id).unwrap().world_origin();
        st.set_feature_origin_axis(id, 2, 5.0);   // Z
        let o1 = st.model.features.iter().find(|f| f.id == id).unwrap().world_origin();
        assert!((o1.z - 5.0).abs() < 1e-4);
        assert!((o1.x - o0.x).abs() < 1e-4 && (o1.y - o0.y).abs() < 1e-4);
    }

    /// A dimension field replaces the primitive.
    #[test]
    fn setting_a_dimension_replaces_the_primitive() {
        let mut st = one_box();
        let (id, prim, _) = st.selected_primitive().unwrap();
        let Primitive::Box { w, d, .. } = prim else { panic!("default add_box is a Box") };
        st.set_feature_primitive(id, Primitive::Box { w, d, h: 9.0 });
        let (_, after, _) = st.selected_primitive().unwrap();
        match after {
            Primitive::Box { h, .. } => assert_eq!(h, 9.0),
            other => panic!("expected a Box, got {other:?}"),
        }
    }

    /// The gizmo projects, and clicking its centre returns the Free handle — the
    /// combination-move grab where all three arms meet.
    #[test]
    fn the_center_cube_picks_the_free_handle() {
        let st = one_box();
        let mvp = view(&st);
        let v = st.gizmo_view(rect(), &mvp).expect("a selected object has a gizmo");
        assert_eq!(st.pick_gizmo(v.center_s, rect(), &mvp), Some(GizmoHandle::Free));
    }

    /// Clicking partway along an arm picks that axis, not Free.
    #[test]
    fn clicking_an_arm_picks_that_axis() {
        let st = one_box();
        let mvp = view(&st);
        let v = st.gizmo_view(rect(), &mvp).unwrap();
        for arm in v.arms.iter().flatten() {
            // 70% along the arm — clear of the centre cube.
            let p = v.center_s + (arm.tip_s - v.center_s) * 0.7;
            assert_eq!(
                st.pick_gizmo(p, rect(), &mvp),
                Some(arm.handle),
                "a click along the {:?} arm must pick {:?}",
                arm.handle, arm.handle
            );
        }
    }

    /// The gizmo has a screen-space FLOOR: a tiny object still gets a grabbable gizmo
    /// (arms at least ~65 px), so it stays visible at any object size.
    #[test]
    fn a_tiny_object_still_gets_a_visible_gizmo() {
        let mut st = FactoryState::default();
        let id = st.model.push(
            cad_solid::BoolOp::Union,
            cad_solid::Plane::default(),
            cad_solid::Placement::default(),
            Primitive::Box { w: 0.02, d: 0.02, h: 0.02 },
        );
        st.selection = vec![id];
        st.recompute();
        st.fit();
        let mvp = view(&st);
        let v = st.gizmo_view(rect(), &mvp).unwrap();
        let arm = v.arms.iter().flatten().next().unwrap();
        assert!(
            v.center_s.distance(arm.tip_s) >= 40.0,
            "even a 2 cm object needs a grabbable on-screen gizmo"
        );
    }

    /// Marquee box-select grabs every feature whose projected centre is inside the band,
    /// and leaves the rest alone.
    #[test]
    fn marquee_selects_features_inside_the_band() {
        let mut st = FactoryState::default();
        // Two boxes, far apart in X.
        let a = st.model.push(cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement { u: -3.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            Primitive::Box { w: 0.5, d: 0.5, h: 0.5 });
        let b = st.model.push(cad_solid::BoolOp::Union, cad_solid::Plane::default(),
            cad_solid::Placement { u: 3.0, v: 0.0, lift: 0.0, spin_deg: 0.0, pitch_deg: 0.0, roll_deg: 0.0 },
            Primitive::Box { w: 0.5, d: 0.5, h: 0.5 });
        st.selection.clear();
        st.recompute();
        st.fit();
        let mvp = view(&st);

        // A band around box A's screen centre only.
        let sa = crate::factory::world_to_screen(
            { let f = st.model.features.iter().find(|f| f.id == a).unwrap();
              let (mn, mx) = f.world_aabb(); (mn + mx) * 0.5 },
            rect(), &mvp,
        ).unwrap();
        let band = egui::Rect::from_center_size(sa, egui::vec2(30.0, 30.0));
        st.select_in_marquee(band, rect(), &mvp, false);
        assert!(st.selection.contains(&a), "A is inside the band");
        assert!(!st.selection.contains(&b), "B is outside it");
    }

    /// Empty selection: no gizmo, nothing to pick.
    #[test]
    fn no_selection_no_gizmo() {
        let mut st = one_box();
        st.clear_selection();
        let mvp = view(&st);
        assert!(st.gizmo_view(rect(), &mvp).is_none());
    }

    /// Deleting the selection removes its features.
    #[test]
    fn erase_selection_removes_the_features() {
        let mut st = one_box();
        assert_eq!(st.model.features.len(), 1);
        st.erase_selection();
        assert!(st.model.features.is_empty());
    }
}

#[cfg(test)]
mod handle_tests {
    use super::*;

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
    }

    fn view(st: &FactoryState) -> [f32; 16] {
        crate::light3d::mvp(
            st.cam_yaw, st.cam_pitch, st.cam_dist, st.cam_target,
            rect().width() / rect().height(), st.ortho,
        )
    }

    fn wall_app() -> (FactoryState, usize) {
        let mut st = FactoryState::default();
        let wi = st
            .add_wall(
                vec![Vec2::new(-2.0, 0.0), Vec2::new(2.0, 0.0), Vec2::new(2.0, 3.0)],
                0.2,
                2.5,
            )
            .unwrap();
        st.recompute();
        st.fit();
        (st, wi)
    }

    /// Handles belong to the SELECTED wall. With nothing selected there are none — the
    /// model would be buried in dots if every wall showed them at once.
    #[test]
    fn handles_follow_the_selection() {
        let (mut st, wi) = wall_app();
        assert!(st.selected_wall().is_none(), "nothing selected ⇒ no wall");
        st.select_wall(wi);
        assert_eq!(st.selected_wall(), Some(wi));
    }

    /// One handle per footprint point, and one edge handle per segment.
    #[test]
    fn there_is_one_handle_per_vertex_and_one_per_edge() {
        let (st, wi) = wall_app();
        let mvp = view(&st);
        assert_eq!(st.wall_vertex_handles(wi, rect(), &mvp).len(), 3);
        assert_eq!(st.wall_edge_handles(wi, rect(), &mvp).len(), 2);
    }

    /// Clicking a drawn handle picks THAT vertex — the projection used for drawing and
    /// the one used for picking must agree, or handles would be un-grabbable.
    #[test]
    fn picking_at_a_drawn_handle_returns_that_vertex() {
        let (st, wi) = wall_app();
        let mvp = view(&st);
        for (vi, p) in st.wall_vertex_handles(wi, rect(), &mvp) {
            assert_eq!(st.pick_wall_vertex(wi, p, rect(), &mvp), Some(vi));
        }
    }

    /// A vertex must WIN a close call against an edge midpoint — otherwise dragging a
    /// corner would insert a point instead of moving it.
    #[test]
    fn a_vertex_beats_an_edge_midpoint_on_a_close_call() {
        let (st, wi) = wall_app();
        let mvp = view(&st);
        let (_, vp) = st.wall_vertex_handles(wi, rect(), &mvp)[0];
        assert!(st.pick_wall_vertex(wi, vp, rect(), &mvp).is_some());
        assert!(
            st.pick_wall_edge(wi, vp, rect(), &mvp).is_none(),
            "on a vertex, the edge pick must stand down"
        );
    }

    /// Clicking empty space grabs nothing.
    #[test]
    fn picking_away_from_every_handle_returns_nothing() {
        let (st, wi) = wall_app();
        let mvp = view(&st);
        let far = egui::pos2(5.0, 5.0);
        assert!(st.pick_wall_vertex(wi, far, rect(), &mvp).is_none());
        assert!(st.pick_wall_edge(wi, far, rect(), &mvp).is_none());
    }

    /// THE hazard of this slice: a shape edit calls `rederive_wall`, which drops the
    /// wall's Boxes and pushes new ones. `Model::push` mints `max(id) + 1`, so the new
    /// ids differ whenever the wall was NOT the highest-numbered feature — here, a solid
    /// added after the wall. The stale selection then resolves to no wall at all, and the
    /// handles would vanish mid-drag. `select_wall` is the refresh that prevents it.
    ///
    /// (With a lone wall the ids happen to be reused, because removing them empties the
    /// model and numbering restarts — which is exactly why this test adds the box.)
    #[test]
    fn reselecting_keeps_the_handles_alive_across_a_shape_edit() {
        let (mut st, wi) = wall_app();
        st.add_box();                 // now the wall is no longer the highest id
        st.select_wall(wi);
        let before = st.selection.clone();

        st.wall_move_vertex(wi, 1, Vec2::new(3.0, 1.0));
        assert_ne!(st.walls[wi].segments, before, "the rebuild really did mint new ids");
        assert!(
            st.selected_wall().is_none(),
            "the stale selection no longer resolves — the test is meaningful"
        );

        st.select_wall(wi);
        assert_eq!(st.selected_wall(), Some(wi), "handles must survive the edit");
        assert_ne!(st.selection, before, "and they track the NEW ids");
    }

    /// Handles sit on the wall's OWN storey, not the ground.
    #[test]
    fn handles_sit_on_the_walls_own_storey() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let base = st.active_base_z();
        let wi = st.add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(3.0, 0.0)], 0.2, 2.5).unwrap();
        assert_eq!(st.wall_vertex_world(wi, 0).unwrap().z, base);
    }
}

#[cfg(test)]
mod slab_tests {
    use super::*;

    fn square(s: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0), Vec2::new(s, 0.0), Vec2::new(s, s), Vec2::new(0.0, s),
            Vec2::new(0.0, 0.0),
        ]
    }

    /// A tessellated circle — exactly what "make room from a plan circle" feeds `add_room`.
    fn circle(r: f32, n: usize) -> Vec<Vec2> {
        let mut v: Vec<Vec2> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec2::new(r * a.cos(), r * a.sin())
            })
            .collect();
        v.push(v[0]); // close it
        v
    }

    /// Highest rendered surface DIRECTLY ABOVE `(x, y)` — the z of the topmost triangle
    /// whose 2D projection contains the point. Robust to how the cap is triangulated (unlike
    /// a centroid-proximity test). Used to check the interior opens over the room centre:
    /// ceiling level before hiding, floor level after. Returns `f32::MIN` if nothing covers it.
    fn ceiling_z_at(st: &FactoryState, x: f32, y: f32) -> f32 {
        let mut best = f32::MIN;
        for t in st.scene_verts().chunks_exact(3) {
            let (ax, ay) = (t[0].x, t[0].y);
            let (bx, by) = (t[1].x, t[1].y);
            let (cx, cy) = (t[2].x, t[2].y);
            let d1 = (x - bx) * (ay - by) - (ax - bx) * (y - by);
            let d2 = (x - cx) * (by - cy) - (bx - cx) * (y - cy);
            let d3 = (x - ax) * (cy - ay) - (cx - ax) * (y - ay);
            let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
            let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
            if !(has_neg && has_pos) {
                let z = (t[0].z + t[1].z + t[2].z) / 3.0;
                if z > best {
                    best = z;
                }
            }
        }
        best
    }

    /// REPRODUCTION of the user's report: a room made from a circle in the plan, then
    /// Hide-ceilings does nothing. Dumps the tracked ceiling id against the face_ids that
    /// actually reach the renderer, so we can SEE whether they match.
    #[test]
    fn repro_hide_ceiling_on_a_circle_room() {
        let mut st = FactoryState::default();
        st.add_room(&circle(25.0, 32)).unwrap();
        st.recompute();

        let ceil_ids: Vec<u32> = st.ceilings.iter().copied().collect();
        let mut uniq: Vec<u32> = st.cached.face_ids.clone();
        uniq.sort_unstable();
        uniq.dedup();
        eprintln!("REPRO: ceilings set = {ceil_ids:?}");
        eprintln!("REPRO: unique rendered face_ids = {uniq:?}");
        let ceil_tris = st
            .cached
            .face_ids
            .iter()
            .filter(|id| st.ceilings.contains(id))
            .count();
        eprintln!("REPRO: rendered triangles tagged as a tracked ceiling = {ceil_tris}");

        // Over the room CENTRE the ceiling must open up (framed-opening look keeps a border
        // over the walls, so the global top may stay — the interior is what must clear).
        let center_shown = ceiling_z_at(&st, 0.0, 0.0);
        st.hide_ceilings = true;
        let center_hidden = ceiling_z_at(&st, 0.0, 0.0);
        eprintln!("REPRO: centre top shown = {center_shown:.3}, hidden = {center_hidden:.3}");
        assert!(ceil_tris > 0, "the ceiling must contribute triangles that hide can drop");
        assert!(center_shown > 2.5, "the ceiling covers the centre before hiding");
        assert!(
            center_hidden < 1.0,
            "the ceiling over the interior must open (hidden centre z {center_hidden:.2})"
        );
    }

    /// THE drift-proof guarantee: "Hide ceilings" must still hide the ceiling even when the
    /// tracked id-set is WRONG — which is exactly what fails in the field (a stale/empty set
    /// hides nothing). Here we deliberately clear `ceilings`; only the GEOMETRIC cap
    /// detection can save it. Hiding must still drop the ceiling and keep the floor + walls.
    #[test]
    fn hide_ceilings_works_even_with_a_broken_ceiling_set() {
        let mut st = FactoryState::default();
        st.add_room(&circle(25.0, 32)).unwrap();
        // Simulate the field bug: the tracked ceiling id no longer matches reality.
        st.ceilings.clear();
        st.recompute(); // recompute detects the cap by GEOMETRY into `ceiling_caps`

        assert!(
            !st.ceiling_caps.is_empty(),
            "geometry must detect the ceiling cap even with the tracked set cleared"
        );

        // Over the centre the ceiling opens even though the id-set was empty (geometry).
        let center_shown = ceiling_z_at(&st, 0.0, 0.0);
        st.hide_ceilings = true;
        let verts = st.scene_verts();
        let center_hidden = ceiling_z_at(&st, 0.0, 0.0);
        eprintln!("REPRO2: centre top shown = {center_shown:.3}, hidden = {center_hidden:.3}");
        assert!(center_shown > 2.5, "the ceiling covers the centre before hiding");
        assert!(
            center_hidden < 1.0,
            "the interior must open by geometry alone (hidden centre z {center_hidden:.2})"
        );
        // The floor is still there to look at (dark floor triangles near z≈0.2 survive).
        let floor_min = verts.iter().map(|v| v.z).fold(f32::MAX, f32::min);
        assert!(floor_min < 0.3, "the floor slab must remain visible, got min z {floor_min:.2}");
        // And the WALLS are NOT removed — plenty of geometry remains.
        assert!(verts.len() > 200, "walls + floor must survive, got {}", verts.len());
    }

    /// DIAGNOSTIC: what does a pure circle room actually contain? Compares against the
    /// user's live model (67 features / 1524 tris) to tell whether there is an extra solid.
    #[test]
    fn diag_circle_room_feature_and_tri_counts() {
        for n in [24usize, 32, 48, 64] {
            let mut st = FactoryState::default();
            st.add_room(&circle(23.0, n)).unwrap();
            st.recompute();
            eprintln!(
                "DIAG: circle({n} seg) room -> {} features, {} tris, ceilings={}, caps={}",
                st.model.features.len(),
                st.cached.tri_count(),
                st.ceilings.len(),
                st.ceiling_caps.len(),
            );
        }
        // Now: a BUILDING solid + a room on the same outline — the "made a building first"
        // path, which would leave a thick disc capping the view after the ceiling is hidden.
        // A building (outer, R=25) with a SMALLER room inside (R=18): the WALL is the ring
        // between them. Hiding must open the room interior and keep the wall ring capped.
        let mut st = FactoryState::default();
        st.add_building_outline(&circle(25.0, 48), 3.0).unwrap();
        st.add_room(&circle(18.0, 48)).unwrap();
        st.recompute();

        // Flat roof at z≈3.0 forming a BORDER over the walls, and the building's WALL tris.
        let roof_border = |st: &FactoryState| {
            st.scene_verts()
                .chunks_exact(3)
                .filter(|t| t.iter().all(|v| (v.z - 3.0).abs() < 0.05))
                .count()
        };
        let building_walls = |st: &FactoryState| {
            st.scene_verts()
                .chunks_exact(3)
                .filter(|t| {
                    t.iter().any(|v| (v.z - 3.0).abs() < 0.05) && t.iter().any(|v| v.z < 1.0)
                })
                .count()
        };
        let center_before = ceiling_z_at(&st, 0.0, 0.0); // over the room interior
        let wall_before = ceiling_z_at(&st, 21.5, 0.0); // over the annulus WALL (R 18→25)
        let walls_before = building_walls(&st);
        st.hide_ceilings = true;
        let center_after = ceiling_z_at(&st, 0.0, 0.0);
        let wall_after = ceiling_z_at(&st, 21.5, 0.0);
        let border_after = roof_border(&st);
        let walls_after = building_walls(&st);
        eprintln!(
            "DIAG: building+room -> {} features, {} tris; centre {center_before:.2}->{center_after:.2}, wall {wall_before:.2}->{wall_after:.2}, border={border_after}, walls {walls_before}->{walls_after}",
            st.model.features.len(),
            st.cached.tri_count(),
        );
        // THE FIX: the roof over the room INTERIOR opens (you see in) …
        assert!(center_before > 2.5, "roof caps the interior to begin with");
        assert!(center_after < 1.0, "hiding opens the interior — no roof over the centre");
        // … the roof over the WALL RING stays (the wall is capped, not opened) …
        assert!(wall_before > 2.5, "the wall ring is capped to begin with");
        assert!(wall_after > 2.5, "the wall ring MUST keep its cap after hiding");
        // … a border of roof remains, and the building's WALLS stay solid.
        assert!(border_after > 0, "a roof cap must remain over the wall ring");
        assert!(walls_before > 0, "the building has walls to begin with");
        assert_eq!(walls_after, walls_before, "the building walls must stay solid");
    }

    /// A plain solid building with NO room under it must NOT be touched by "Hide ceilings" —
    /// the roof-clip only applies to a solid that actually encloses a room.
    #[test]
    fn a_plain_building_is_not_touched_by_hide_ceilings() {
        let mut st = FactoryState::default();
        st.add_building_outline(&square(6.0), 3.0).unwrap();
        st.recompute();
        let shown = st.scene_verts().len();
        st.hide_ceilings = true;
        assert_eq!(
            st.scene_verts().len(),
            shown,
            "a lone building has no room cap, so nothing is hidden"
        );
    }

    /// The walls must NOT be treated as ceilings — hiding must never make a wall vanish.
    /// (Cap detection keys on flat/thin/elevated/topmost, none of which a wall satisfies.)
    #[test]
    fn hiding_ceilings_keeps_the_walls() {
        let mut st = FactoryState::default();
        st.add_room(&square(4.0)).unwrap();
        st.recompute();
        // A wall box is tall/vertical → never a cap.
        for f in &st.model.features {
            let (mn, mx) = f.world_aabb();
            let is_wall = (mx.z - mn.z) > 1.0; // walls span the room height
            if is_wall {
                assert!(!st.ceiling_caps.contains(&f.id), "a wall must not be a ceiling cap");
            }
        }
        st.hide_ceilings = true;
        // Vertical wall faces (normal ~horizontal) must survive hiding.
        let has_vertical = st
            .scene_verts()
            .chunks_exact(3)
            .any(|t| {
                let z = (t[0].z + t[1].z + t[2].z) / 3.0;
                z > 0.5 && z < 2.5 // mid-wall height band
            });
        assert!(has_vertical, "wall geometry at mid height must remain after hiding");
    }

    /// An L-shape — the case a Box genuinely cannot represent.
    fn ell() -> Vec<Vec2> {
        vec![
            Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 2.0),
            Vec2::new(2.0, 2.0), Vec2::new(2.0, 4.0), Vec2::new(0.0, 4.0),
            Vec2::new(0.0, 0.0),
        ]
    }

    /// A slab is now an EXTRUSION of the real outline, so an L-shaped room gets an
    /// L-shaped floor — not the bounding-box rectangle it used to get. This is THE bug
    /// this rewrite fixes.
    #[test]
    fn an_l_shaped_room_gets_an_l_shaped_floor() {
        let mut st = FactoryState::default();
        let id = st.add_floor(&ell(), 0.2).expect("an L must slab");
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert!(
            matches!(f.primitive, Primitive::Extrusion { .. }),
            "a non-rectangular slab must be an extrusion, not a Box"
        );
        // The extruded L has more triangles than a 6-face box would.
        st.recompute();
        assert!(st.cached.tri_count() > 12, "the L outline must be preserved in the mesh");
    }

    /// A room is a VOID carved from the building — a Difference feature, not a solid.
    #[test]
    fn a_room_is_built_constructively_with_walls() {
        let mut st = FactoryState::default();
        // No building needed — a room builds itself.
        let id = st.add_room(&square(4.0)).expect("a room builds from an outline");
        // The returned id is the floor slab, an extrusion.
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert_eq!(f.op, cad_solid::BoolOp::Union);
        // Walls were added as their own boxes (one per edge of the square = 4).
        let wall_boxes = st.model.features.iter()
            .filter(|x| matches!(x.primitive, Primitive::Box { .. })).count();
        assert!(wall_boxes >= 4, "a square room has at least 4 wall boxes, got {wall_boxes}");
        st.recompute();
        assert!(st.cached.tri_count() > 0, "the room renders");
    }

    /// Toggling hide_ceilings changes scene_verts IMMEDIATELY — no recompute needed. This
    /// is the mechanism the UI relies on; if it regresses, hide silently stops working.
    #[test]
    fn toggling_hide_changes_scene_verts_without_recompute() {
        let mut st = FactoryState::default();
        st.add_room(&square(4.0)).unwrap();
        st.recompute();
        let shown = st.scene_verts().len();
        st.hide_ceilings = true;         // NO recompute
        let hidden = st.scene_verts().len();
        st.hide_ceilings = false;        // NO recompute
        let shown_again = st.scene_verts().len();
        assert!(hidden < shown, "hiding drops triangles at render time");
        assert_eq!(shown, shown_again, "unhiding restores them");
    }

    /// THE actual goal: hiding the ceiling opens the room INTERIOR so from above you see in,
    /// while a border of ceiling is kept over the walls (the framed-opening look). Check the
    /// highest rendered point OVER THE CENTRE drops from the ceiling to the floor.
    #[test]
    fn hiding_removes_the_room_top() {
        let mut st = FactoryState::default();
        st.add_room(&square(4.0)).unwrap(); // ceiling top ≈ 3.05, centre at (2,2)
        st.recompute();
        let center_shown = ceiling_z_at(&st, 2.0, 2.0);
        st.hide_ceilings = true;
        let center_hidden = ceiling_z_at(&st, 2.0, 2.0);
        assert!(center_shown > 2.5, "the ceiling covers the centre before hiding");
        assert!(
            center_hidden < 1.0,
            "hiding must open the interior over the centre (hidden centre z {center_hidden:.2})"
        );
    }

    /// A default room must be TALL — floor + room_height walls + ceiling ≈ 3 m — not a
    /// flat pancake. If this fails, the walls aren't getting their height.
    #[test]
    fn a_default_room_is_full_height() {
        let mut st = FactoryState::default();
        // Defaults: floor 0.2, height 2.7, ceiling 0.15 → top ≈ 3.05 m.
        assert!((st.room_height - 2.7).abs() < 1e-4, "default room height is 2.7");
        st.add_room(&square(4.0)).unwrap();
        st.recompute();
        let (mn, mx) = st.cached.bounds().expect("the room has geometry");
        let tall = mx[2] - mn[2];
        assert!(tall > 2.5, "a default room must be ~3 m tall, got {tall:.2} m");
    }

    /// A room needs NO pre-existing building — it constructs its own floor, walls, ceiling.
    #[test]
    fn a_room_builds_standalone() {
        let mut st = FactoryState::default();
        assert!(st.add_room(&square(4.0)).is_ok(), "a room must build with no building");
        assert!(!st.model.features.is_empty());
        assert_eq!(st.ceilings.len(), 1, "and it has a ceiling");
    }

    /// A room has an explicit floor on the base and a tracked ceiling above the walls.
    #[test]
    fn a_room_has_a_separate_floor_and_ceiling_by_default() {
        let mut st = FactoryState::default();
        st.room_floor = 0.25;
        st.room_height = 2.5;   // floor→ceiling clear height
        st.ceiling_thickness = 0.15;
        st.add_room(&square(4.0)).unwrap();
        // The ceiling sits above the floor + walls: base(0) + floor(0.25) + height(2.5).
        assert_eq!(st.ceilings.len(), 1, "one separate ceiling object");
        let cid = *st.ceilings.iter().next().unwrap();
        let c = st.model.features.iter().find(|f| f.id == cid).unwrap();
        // add_slab lifts by (top_z - thickness): top at 0.25+2.5+0.15 = 2.9, lift = 2.75.
        assert!((c.placement.lift - 2.75).abs() < 1e-3, "ceiling underside at floor + height");
    }

    /// The open-to-sky toggle makes NO ceiling slab.
    #[test]
    fn open_top_room_has_no_ceiling() {
        let mut st = FactoryState::default();
        st.room_open_top = true;
        st.add_building_outline(&square(10.0), 3.0).unwrap();
        st.add_room(&square(4.0)).unwrap();
        assert!(st.ceilings.is_empty(), "an open room has no ceiling object");
    }

    /// A ceiling made with the Make-ceiling TOOL is tracked, so Hide-ceilings hides it too
    /// (previously only room ceilings were hideable).
    #[test]
    fn a_make_ceiling_ceiling_is_hideable() {
        let mut st = FactoryState::default();
        st.add_building_outline(&square(6.0), 3.0).unwrap();
        st.add_ceiling(&square(6.0), 0.2).expect("ceiling made");
        assert_eq!(st.ceilings.len(), 1, "the Make-ceiling result is tracked");
        st.recompute();
        let shown = st.scene_verts().len();
        st.hide_ceilings = true;
        // No recompute needed — hiding is a render-time filter.
        assert!(st.scene_verts().len() < shown, "hiding removes it from the render");
        assert_eq!(st.cached.tri_count(), st.model.eval().tri_count(), "the mesh itself is unchanged");
    }

    /// Hiding ceilings drops ONLY the ceiling slabs from the render — the model keeps them
    /// (for the lighting calc), and nothing else disappears.
    #[test]
    fn hide_ceilings_is_view_only() {
        let mut st = FactoryState::default();
        st.add_building_outline(&square(10.0), 3.0).unwrap();
        st.add_room(&square(4.0)).unwrap();
        st.recompute();
        let shown = st.scene_verts().len();
        let features = st.model.features.len();

        st.hide_ceilings = true;
        assert!(st.scene_verts().len() < shown, "the ceiling slab is not drawn");
        assert_eq!(st.model.features.len(), features, "but no feature is deleted");
        assert_eq!(st.ceilings.len(), 1, "the ceiling is still tracked");
    }

    /// On an UPPER storey the room is built on THAT storey's base — the void clears from
    /// the upper base and the floor slab sits there, not on the ground.
    #[test]
    fn an_upper_storey_room_sits_on_its_own_floor() {
        let mut st = FactoryState::default();
        st.room_floor = 0.2;
        st.room_height = 2.5;
        st.add_storey_on_top();     // storey 1 active
        let base = st.active_base_z();
        assert!(base > 0.0, "we are on an upper storey");
        st.add_room(&square(4.0)).unwrap();
        // The ceiling sits a storey up (base + floor + height, above the ground storey).
        let cid = *st.ceilings.iter().next().unwrap();
        let c = st.model.features.iter().find(|f| f.id == cid).unwrap();
        assert!(c.placement.lift > base, "the room is built on the upper storey");
    }

    /// A rectangle still slabs fine — the general path must not regress the simple case.
    #[test]
    fn a_rectangular_outline_still_slabs() {
        let mut st = FactoryState::default();
        assert!(st.add_floor(&square(4.0), 0.2).is_some());
    }

    /// A floor's TOP face is the level you stand on, so it sits below the storey base.
    #[test]
    fn a_floor_sits_below_the_level_it_serves() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let base = st.active_base_z();
        let id = st.add_floor(&square(3.0), 0.25).unwrap();
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert!(
            (f.placement.lift - (base - 0.25)).abs() < 1e-4,
            "the floor's top face must land on the storey base"
        );
    }

    /// A ceiling closes the storey at the level above, so a ceiling and the floor above
    /// it meet rather than overlap.
    #[test]
    fn a_ceiling_closes_the_storey_at_the_level_above() {
        let mut st = FactoryState::default();
        let top = st.storeys[0].height;
        let id = st.add_ceiling(&square(3.0), 0.2).unwrap();
        let f = st.model.features.iter().find(|f| f.id == id).unwrap();
        assert!((f.placement.lift + 0.2 - top).abs() < 1e-4);
    }

    /// A degenerate outline has no slab — better nothing than a zero-volume solid.
    #[test]
    fn a_zero_area_outline_makes_no_slab() {
        let mut st = FactoryState::default();
        let flat = vec![
            Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(2.0, 0.0),
            Vec2::new(0.0, 0.0),
        ];
        assert!(st.add_slab(&flat, 0.2, 0.0).is_none());
    }

    /// Slabs are ordinary features, so the derived z-band rule assigns them like anything
    /// else — and a floor's BODY lies below the level it serves. So an upper floor is
    /// recorded on the storey beneath, which is structurally what it is: level 1's floor
    /// and level 0's ceiling are the same slab. Pinned here because it decides what
    /// `delete_storey` takes with it.
    #[test]
    fn an_upper_floor_belongs_to_the_storey_beneath_it() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        st.add_floor(&square(3.0), 0.2);
        assert!(
            st.features_on_storey(1).is_empty(),
            "the slab lies below level 1's base, so it is not level 1's own geometry"
        );
        assert!(!st.features_on_storey(0).is_empty(), "it caps level 0");
    }

    /// A CEILING, by contrast, lies inside its own storey's band.
    #[test]
    fn a_ceiling_belongs_to_its_own_storey() {
        let mut st = FactoryState::default();
        st.add_ceiling(&square(3.0), 0.2);
        assert!(!st.features_on_storey(0).is_empty());
    }
}

#[cfg(test)]
mod storey_tests {
    use super::*;

    fn fp() -> Vec<Vec2> {
        vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0)]
    }

    /// A building always has exactly one level to begin with, and it starts at zero — so
    /// everything behaves as it did before storeys existed.
    #[test]
    fn a_new_building_has_one_ground_storey() {
        let st = FactoryState::default();
        assert_eq!(st.storeys.len(), 1);
        assert_eq!(st.storey_base_z(0), 0.0);
        assert_eq!(st.active_base_z(), 0.0);
    }

    /// `base_z` is DERIVED, never stored — so the stack is contiguous by construction.
    #[test]
    fn bases_are_the_running_sum_of_the_heights_below() {
        let mut st = FactoryState::default();
        st.storeys = vec![
            Storey { name: "G".into(), height: 3.0 },
            Storey { name: "1".into(), height: 2.5 },
            Storey { name: "2".into(), height: 4.0 },
        ];
        assert_eq!(st.storey_base_z(0), 0.0);
        assert_eq!(st.storey_base_z(1), 3.0);
        assert_eq!(st.storey_base_z(2), 5.5);
        assert_eq!(st.building_total_height(), 9.5);
    }

    /// New geometry is built on the ACTIVE storey — otherwise the selector means nothing.
    #[test]
    fn new_geometry_lands_on_the_active_storey() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();                    // level 1, active
        let base = st.active_base_z();
        assert!(base > 0.0, "the second level cannot start at the ground");

        st.add_wall(fp(), 0.2, 2.5).expect("wall must promote");
        assert_eq!(st.walls[0].base_z, base);
        let z = st.model.features.last().unwrap().world_origin().z;
        assert!((z - base).abs() < 1e-3, "the solid must stand on the active level");
    }

    /// Membership is derived from the z band, so it survives a `rederive_wall` that mints
    /// brand-new feature ids — the failure a stored id list would have.
    #[test]
    fn storey_membership_survives_a_rederive() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let wi = st.add_wall(fp(), 0.2, 2.5).unwrap();
        let before = st.features_on_storey(1);
        assert!(!before.is_empty());

        st.wall_insert_vertex(wi, 0, Vec2::new(2.0, 0.0));   // rebuilds with fresh ids
        let after = st.features_on_storey(1);
        assert!(!after.is_empty(), "the wall must still belong to level 1");
        assert_ne!(before, after, "ids really did change — the test is meaningful");
    }

    /// Thickness is editable after the fact, like height — and IN PLACE, so feature ids
    /// (and therefore the selection and its handles) survive the edit.
    #[test]
    fn wall_thickness_is_editable_without_changing_ids() {
        let mut st = FactoryState::default();
        let wi = st.add_wall(fp(), 0.2, 2.5).unwrap();
        let fid = st.walls[wi].segments[0];
        let ids = st.walls[wi].segments.clone();

        st.set_wall_thickness(fid, 0.45);
        assert_eq!(st.walls[wi].thickness, 0.45);
        assert_eq!(st.walls[wi].segments, ids, "an in-place edit must not renumber");
        // The Box's depth IS the wall thickness.
        let f = st.model.features.iter().find(|f| f.id == fid).unwrap();
        match f.primitive {
            cad_solid::Primitive::Box { d, .. } => assert_eq!(d, 0.45),
            other => panic!("a wall segment must stay a Box, got {other:?}"),
        }
        assert_eq!(st.walls[wi].height, 2.5, "changing thickness must not touch height");
    }

    /// Promoted geometry with no thickness of its own takes the FACTORY setting — the one
    /// that is editable in the 3D panel.
    #[test]
    fn thickness_less_geometry_uses_the_factory_setting() {
        let mut st = FactoryState::default();
        st.wall_thickness = 0.33;
        let wi = st.add_wall(fp(), st.wall_thickness, 2.5).unwrap();
        assert_eq!(st.walls[wi].thickness, 0.33);
    }

    /// Editing a wall on an upper level must not drop it to the ground. This is why
    /// `base_z` lives on the wall and not only in the feature placement.
    #[test]
    fn editing_an_upper_wall_keeps_it_on_its_level() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let base = st.active_base_z();
        let wi = st.add_wall(fp(), 0.2, 2.5).unwrap();

        st.wall_move_vertex(wi, 1, Vec2::new(6.0, 0.0));
        assert_eq!(st.walls[wi].base_z, base);
        for id in &st.walls[wi].segments {
            let z = st.model.features.iter().find(|f| f.id == *id).unwrap().world_origin().z;
            assert!((z - base).abs() < 1e-3, "the wall fell off its storey");
        }
    }

    /// Changing a level's height moves everything ABOVE it, keeping the stack contiguous,
    /// without stretching the geometry that stands ON it.
    #[test]
    fn raising_a_storey_lifts_everything_above_it() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        let upper = st.add_wall(fp(), 0.2, 2.5).unwrap();
        let upper_base = st.walls[upper].base_z;
        let wall_height = st.walls[upper].height;

        st.set_storey_height(0, st.storeys[0].height + 1.0);
        assert_eq!(st.walls[upper].base_z, upper_base + 1.0, "the upper level must rise");
        assert_eq!(st.walls[upper].height, wall_height, "its walls must not stretch");
        assert_eq!(st.storey_base_z(1), st.storeys[0].height, "stack stays contiguous");
    }

    /// "Duplicate floor up" copies the active level's geometry onto a new level above,
    /// stacked by the storey height — the visible "add a floor" the user expected.
    #[test]
    fn duplicate_storey_up_stacks_a_copy() {
        let sq = |s: f32| vec![
            Vec2::new(0.0, 0.0), Vec2::new(s, 0.0), Vec2::new(s, s), Vec2::new(0.0, s),
            Vec2::new(0.0, 0.0),
        ];
        let mut st = FactoryState::default();
        st.add_building_outline(&sq(6.0), 3.0).unwrap();  // a ground-floor building
        let before = st.model.features.len();
        let dst = st.duplicate_storey_up().expect("there is geometry to duplicate");
        assert_eq!(st.storeys.len(), 2, "a new level was added");
        assert_eq!(st.active_storey, dst, "the copy's level becomes active");
        assert!(st.model.features.len() > before, "the building was copied, not moved");
        let base = st.storey_base_z(dst);
        assert!(base > 0.0);
        assert!(
            st.model.features.iter().any(|f| (f.world_origin().z - base).abs() < 0.5),
            "the copy stands on the new level"
        );
    }

    /// Duplicating an empty level does nothing (and reports so via `None`).
    #[test]
    fn duplicating_an_empty_level_is_a_noop() {
        let mut st = FactoryState::default();
        assert!(st.duplicate_storey_up().is_none());
        assert_eq!(st.storeys.len(), 1, "no phantom level is created");
    }

    /// Deleting a level takes its geometry with it and closes the gap.
    #[test]
    fn deleting_a_storey_removes_its_geometry_and_closes_the_gap() {
        let mut st = FactoryState::default();
        st.add_wall(fp(), 0.2, 2.5);            // ground
        st.add_storey_on_top();
        st.add_wall(fp(), 0.2, 2.5);            // level 1
        assert_eq!(st.walls.len(), 2);

        assert!(st.delete_storey(0), "deleting the ground level must succeed");
        assert_eq!(st.storeys.len(), 1);
        assert_eq!(st.walls.len(), 1, "the ground wall went with its storey");
        assert_eq!(st.walls[0].base_z, 0.0, "the surviving level dropped to the ground");
    }

    /// A building must always have a level — otherwise `active_storey` indexes nothing.
    #[test]
    fn the_last_storey_cannot_be_deleted() {
        let mut st = FactoryState::default();
        assert!(!st.delete_storey(0));
        assert_eq!(st.storeys.len(), 1);
    }

    /// Levels survive save/reopen, and a pre-storeys sidecar loads as one ground level
    /// rather than a building with none.
    #[test]
    fn storeys_round_trip_and_old_files_get_a_ground_level() {
        let mut st = FactoryState::default();
        st.add_storey_on_top();
        st.add_wall(fp(), 0.2, 2.5);
        let json = serde_json::to_string(&st.to_persist()).unwrap();
        let back: crate::simlux_io::FactoryDoc = serde_json::from_str(&json).unwrap();

        let mut re = FactoryState::default();
        re.apply_persist(back);
        assert_eq!(re.storeys.len(), 2);
        assert_eq!(re.active_storey, 1);
        assert_eq!(re.walls[0].base_z, st.walls[0].base_z, "the wall kept its level");

        // A sidecar written before storeys existed.
        let mut old = FactoryState::default();
        old.apply_persist(crate::simlux_io::FactoryDoc::default());
        assert_eq!(old.storeys.len(), 1, "an old file must still have a level");
        assert_eq!(old.active_storey, 0);
    }
}

#[cfg(test)]
mod persist_tests {
    use super::*;

    /// A building modelled in 3D must survive save → reopen. Before this, nothing wrote
    /// `factory.model` anywhere: you could model a building, close the app, and lose it.
    /// This proves the whole path INCLUDING the JSON hop, not just the struct copy.
    #[test]
    fn model_and_walls_survive_a_json_round_trip() {
        let mut st = FactoryState::default();
        st.wall_height = 3.4;
        st.building_height = 6.5;
        let fp = vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 3.0)];
        st.add_wall(fp.clone(), 0.25, 3.4).expect("wall must promote");
        st.add_box();
        let features_before = st.model.features.len();

        let json = serde_json::to_string(&st.to_persist()).expect("must serialize");
        let back: crate::simlux_io::FactoryDoc =
            serde_json::from_str(&json).expect("must deserialize");

        let mut re = FactoryState::default();
        assert_eq!(re.apply_persist(back), 0, "nothing should be dropped");
        assert_eq!(re.model.features.len(), features_before);
        assert_eq!(re.walls.len(), 1);
        assert_eq!(re.walls[0].footprint.len(), fp.len(), "footprint must survive intact");
        assert_eq!(re.walls[0].footprint[1], Vec2::new(4.0, 0.0));
        assert_eq!(re.walls[0].thickness, 0.25);
        assert_eq!(re.wall_height, 3.4);
        assert_eq!(re.building_height, 6.5);
        assert!(re.dirty, "a restored model must re-evaluate before it can be drawn");
    }

    /// A wall whose segments name features the model does not have is unusable — the
    /// link is what makes it editable. It must be dropped AND counted, never restored
    /// dangling and never dropped in silence.
    #[test]
    fn a_wall_with_dangling_feature_ids_is_dropped_and_counted() {
        let mut st = FactoryState::default();
        st.add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0)], 0.2, 2.5);
        let mut doc = st.to_persist();
        doc.walls[0].segments = vec![9999];       // no such feature

        let mut re = FactoryState::default();
        assert_eq!(re.apply_persist(doc), 1, "the bad wall must be counted as dropped");
        assert!(re.walls.is_empty());
        assert!(!re.model.features.is_empty(), "the solids themselves still load");
    }

    /// An older sidecar has no heights (serde fills 0.0). Adopting that would give a
    /// building of no height; the live defaults must win.
    #[test]
    fn absent_heights_do_not_flatten_the_building() {
        let mut re = FactoryState::default();
        let (wh, bh) = (re.wall_height, re.building_height);
        assert_eq!(re.apply_persist(crate::simlux_io::FactoryDoc::default()), 0);
        assert_eq!(re.wall_height, wh);
        assert_eq!(re.building_height, bh);
    }

    /// A drawing with no 3D model must not write a factory block.
    #[test]
    fn an_untouched_factory_persists_as_empty() {
        assert!(FactoryState::default().to_persist().is_empty());
    }
}

#[cfg(test)]
mod building_tests {
    use super::*;

    // NOTE (owner, 2026-07-23): the Building section must not re-expose a primitive the
    // Draw3D palette already offers — rectangular / circular / polygonal "elements" were
    // removed because they were Box / Cylinder / Prism under new labels. The section now
    // holds ACTIONS (Make building / walls / floor / ceiling), none of them shape-named,
    // so the `BuildingTool` enum that once guarded this is gone with it.

    /// Building height is a property of the BUILDING, so it lives on the state (like
    /// `wall_height`) and survives across operations rather than resetting per dialog.
    /// It is what the outline will rise to once the extrusion primitive lands.
    #[test]
    fn building_height_is_state_not_dialog() {
        let mut st = FactoryState::default();
        assert!(st.building_height > 0.0, "a building must have a usable default height");
        st.building_height = 4.25;
        st.add_box();   // an unrelated modelling op must not disturb it
        assert_eq!(st.building_height, 4.25);
    }
}

#[cfg(test)]
mod draw3d_edit_tests {
    use super::*;

    /// EDIT-MODE invariant (owner, 2026-07-17: "if one 3d dobject selected, with these
    /// controllers we should be able to change its dimension"). Selecting a solid loads
    /// it into the dialog via `load_from`; editing then rebuilds via `build`. If the two
    /// are not inverses, tweaking one field would silently corrupt the others. This
    /// proves `load_from → build` reproduces the primitive for every shape (compared by
    /// Debug, since `Primitive` isn't `PartialEq`). The Frustum family is the tricky one:
    /// cone / prism / pyramid all share one variant but different controllers.
    #[test]
    fn load_from_then_build_round_trips() {
        let cases = [
            Primitive::Box { w: 3.0, d: 4.0, h: 2.5 },
            Primitive::Cylinder { r: 1.2, h: 5.0, sides: 20 },
            Primitive::Sphere { r: 2.0, segments: 40, stacks: 18 },
            Primitive::Frustum { r_bottom: 2.0, r_top: 0.0, h: 3.0, sides: 24 }, // cone
            Primitive::Frustum { r_bottom: 1.5, r_top: 1.5, h: 2.0, sides: 6 },  // prism
            Primitive::Frustum { r_bottom: 2.0, r_top: 0.0, h: 3.0, sides: 4 },  // pyramid
            Primitive::Torus { major_r: 3.0, minor_r: 0.8, seg_major: 36, seg_minor: 18 },
            Primitive::Capsule { r: 0.7, h: 2.0, segments: 24, stacks: 12 },
            Primitive::Tube { r_outer: 2.0, r_inner: 1.0, h: 3.0, sides: 28 },
            Primitive::Ellipsoid { rx: 1.0, ry: 2.0, rz: 0.5, segments: 32, stacks: 16 },
        ];
        for p in cases {
            let mut dlg = Draw3dDialog::new(Draw3dKind::Box);
            dlg.load_from(&p);
            let rebuilt = dlg.build();
            assert_eq!(
                format!("{rebuilt:?}"), format!("{p:?}"),
                "load_from → build must reproduce the primitive"
            );
        }
    }
}

#[cfg(test)]
mod wall_tests {
    use super::*;

    /// 2D→3D wall promotion (owner, 2026-07-17): a centerline segment → ONE wall solid,
    /// a Box of length × thickness × height placed at the midpoint, spun along the run.
    #[test]
    fn wall_segment_is_a_placed_box() {
        let mut st = FactoryState::default();
        st.add_wall_segment(Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), 0.3, 2.5); // 4 m along +X
        assert_eq!(st.model.features.len(), 1, "one segment → one solid");

        let f = &st.model.features[0];
        match f.primitive {
            Primitive::Box { w, d, h } => {
                assert!((w - 4.0).abs() < 1e-4, "length spans the centerline");
                assert!((d - 0.3).abs() < 1e-4, "depth = the wall's own thickness");
                assert!((h - 2.5).abs() < 1e-4, "height = the 3D wall height");
            }
            other => panic!("wall segment must be a Box, got {other:?}"),
        }
        assert!((f.placement.u - 2.0).abs() < 1e-4, "placed at the midpoint u");
        assert!(f.placement.v.abs() < 1e-4, "placed at the midpoint v");
        assert!(f.placement.spin_deg.abs() < 1e-4, "run along +X → spin 0°");
    }

    /// Orientation: a +Y run spins 90°; degenerate input is ignored.
    #[test]
    fn wall_segment_orientation_and_degenerate_guard() {
        let mut st = FactoryState::default();
        st.add_wall_segment(Vec2::new(0.0, 0.0), Vec2::new(0.0, 3.0), 0.2, 2.7); // +Y
        assert_eq!(st.model.features.len(), 1);
        assert!((st.model.features[0].placement.spin_deg - 90.0).abs() < 1e-3, "+Y run → spin 90°");

        st.add_wall_segment(Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0), 0.2, 2.7); // zero length
        st.add_wall_segment(Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), 0.0, 2.7); // zero thickness
        assert_eq!(st.model.features.len(), 1, "degenerate segments are ignored");
    }

    /// Walls stay ALIVE (owner, 2026-07-17): a promotion records a live wall whose height
    /// re-derives the Box on the fly, keeping its length and thickness.
    #[test]
    fn wall_stays_alive_height_re_derives() {
        let mut st = FactoryState::default();
        st.add_wall_segment(Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), 0.3, 2.5);
        assert_eq!(st.walls.len(), 1, "promotion records a live wall");
        let fid = st.walls[0].segments[0];

        st.set_wall_height(fid, 3.2);
        assert!((st.walls[0].height - 3.2).abs() < 1e-4, "registry height updated");
        match st.model.get_mut(fid).unwrap().primitive {
            Primitive::Box { w, d, h } => {
                assert!((h - 3.2).abs() < 1e-4, "box height re-derived");
                assert!((w - 4.0).abs() < 1e-4 && (d - 0.3).abs() < 1e-4, "length & thickness kept");
            }
            _ => panic!("a wall is a Box"),
        }
        st.clear();
        assert!(st.walls.is_empty(), "clear drops the live-wall records too");
    }

    /// Footprint editing (owner, 2026-07-22): a wall is driven by ONE ground-plane
    /// footprint, so N points → N−1 Box segments, and adding/moving/deleting a vertex
    /// re-derives. The new corner is on BOTH rings by construction: every segment Box
    /// rises the full height from z=0, so the vertex exists at the floor AND the ceiling.
    #[test]
    fn footprint_wall_add_vertex_couples_rings_and_reshapes() {
        let mut st = FactoryState::default();
        // An L-shaped footprint: (0,0)-(4,0)-(4,3) → 2 segments.
        let wi = st
            .add_wall(vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 3.0)], 0.3, 2.7)
            .expect("L footprint promotes");
        assert_eq!(st.walls[wi].footprint.len(), 3);
        assert_eq!(st.walls[wi].segments.len(), 2, "N points → N−1 segments");
        assert_eq!(st.model.features.len(), 2);

        // Add a corner mid first edge, at (2,0): 4 points / 3 segments.
        let vi = st.wall_insert_vertex(wi, 0, Vec2::new(2.0, 0.0)).expect("split edge 0");
        assert_eq!(vi, 1);
        assert_eq!(st.walls[wi].footprint.len(), 4);
        assert_eq!(st.walls[wi].segments.len(), 3, "add vertex → +1 segment");

        // Both rings share the footprint: EVERY segment Box rises the full height from the
        // ground, so the new corner is present on both the floor (z=0) and ceiling (z=h).
        for &fid in &st.walls[wi].segments {
            match st.model.get_mut(fid).expect("segment feature").primitive {
                Primitive::Box { h, .. } => {
                    assert!((h - 2.7).abs() < 1e-4, "segment rises full height → vertex on floor & ceiling")
                }
                _ => panic!("a wall segment must be a Box"),
            }
        }

        // Drag the corner → the surface shifts; still 3 segments.
        st.wall_move_vertex(wi, 1, Vec2::new(2.0, 1.0));
        assert_eq!(st.walls[wi].segments.len(), 3);
        assert!((st.walls[wi].footprint[1] - Vec2::new(2.0, 1.0)).length() < 1e-6, "vertex moved");

        // Delete the corner → back to 3 points / 2 segments.
        assert!(st.wall_delete_vertex(wi, 1), "delete a corner");
        assert_eq!(st.walls[wi].footprint.len(), 3);
        assert_eq!(st.walls[wi].segments.len(), 2);
        // Delete down to the 2-point minimum (one segment), then reject any further delete.
        assert!(st.wall_delete_vertex(wi, 0), "delete down to a single segment");
        assert_eq!(st.walls[wi].footprint.len(), 2);
        assert_eq!(st.walls[wi].segments.len(), 1);
        assert!(!st.wall_delete_vertex(wi, 0), "a wall never drops below 2 points");
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::*;

    /// Zoom-window (owner, 2026-07-17: "we need zoom as it is in 2d"): a CENTERED box keeps
    /// the target where it is and dollies in by the box/viewport height ratio.
    #[test]
    fn zoom_window_centered_box_keeps_target_and_dollies_in() {
        let mut st = FactoryState::default();
        st.cam_target = [5.0, 5.0, 0.0];
        st.cam_dist = 20.0;
        let vp = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let c = vp.center();
        // a centered box, half the viewport height (300 px) → target unchanged, dist halved
        st.zoom_window(vp, egui::pos2(c.x - 100.0, c.y - 150.0), egui::pos2(c.x + 100.0, c.y + 150.0));
        assert!((st.cam_target[0] - 5.0).abs() < 1e-3 && (st.cam_target[1] - 5.0).abs() < 1e-3,
                "a centered box keeps the target");
        assert!((st.cam_dist - 10.0).abs() < 1e-2, "a half-height box halves the distance");
    }

    /// An off-centre box shifts the target toward it (here: box to the RIGHT of centre).
    #[test]
    fn zoom_window_offcentre_box_shifts_target() {
        let mut st = FactoryState::default();
        st.cam_dist = 20.0; // Iso-ish default yaw/pitch
        let vp = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let c = vp.center();
        let before = st.cam_target;
        st.zoom_window(vp, egui::pos2(c.x + 100.0, c.y - 50.0), egui::pos2(c.x + 300.0, c.y + 50.0));
        let moved = (st.cam_target[0] - before[0]).abs()
            + (st.cam_target[1] - before[1]).abs()
            + (st.cam_target[2] - before[2]).abs();
        assert!(moved > 1e-3, "an off-centre window must move the target");
    }

    /// `zoom previous` restores the camera saved before the last zoom.
    #[test]
    fn zoom_previous_restores_the_pre_zoom_camera() {
        let mut st = FactoryState::default();
        st.cam_dist = 20.0;
        st.cam_target = [1.0, 2.0, 3.0];
        st.zoom_save_prev();
        st.cam_dist = 5.0;
        st.cam_target = [9.0, 9.0, 9.0];
        st.zoom_restore_previous();
        assert!((st.cam_dist - 20.0).abs() < 1e-4, "distance restored");
        assert!((st.cam_target[0] - 1.0).abs() < 1e-4 && (st.cam_target[2] - 3.0).abs() < 1e-4,
                "target restored");
        // a second restore is a no-op (the snapshot was consumed)
        st.zoom_restore_previous();
        assert!((st.cam_dist - 20.0).abs() < 1e-4, "second restore is harmless");
    }
}

#[cfg(test)]
mod place_tests {
    use super::*;

    /// Point placement (owner, 2026-07-22): a Box places its NEAR CORNER at the click
    /// (extends +w,+d from there); every other primitive places its CENTRE at the click.
    #[test]
    fn box_corner_and_cylinder_centre() {
        let mut st = FactoryState::default();
        // Box 2×2×1, corner at (10, 20) → centre offset by half-extents (+1, +1).
        st.place_primitive(Primitive::Box { w: 2.0, d: 2.0, h: 1.0 }, Vec3::new(10.0, 20.0, 0.0));
        assert_eq!(st.model.features.len(), 1);
        let pl = st.model.features[0].placement;
        assert!((pl.u - 11.0).abs() < 1e-4 && (pl.v - 21.0).abs() < 1e-4,
                "box's near corner sits at the click");

        // Cylinder centred at the click.
        st.place_primitive(Primitive::Cylinder { r: 1.0, h: 2.0, sides: 24 }, Vec3::new(5.0, -5.0, 0.0));
        let pl2 = st.model.features[1].placement;
        assert!((pl2.u - 5.0).abs() < 1e-4 && (pl2.v + 5.0).abs() < 1e-4,
                "cylinder centre sits at the click");
    }
}
