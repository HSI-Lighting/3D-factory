//! The reverse `ScriptOp` queue (slices 2–3) — how scripts READ and MUTATE
//! the document without the host ever seeing a Python type (D2).
//!
//! A running script sends an op; the host (the app's main thread, or the
//! headless CLI) applies it against the document and replies with an owned
//! value. The Python side blocks until the reply arrives with the GIL
//! RELEASED (see `rasm.rs`), so ops are strictly serialized with the host's
//! own mutations and a cancelled script never deadlocks the host.
//!
//! Every value crossing this boundary is a plain Rust `Send` type (D7 —
//! Python holds only owned copies). The enum is deliberately serializable
//! so a future subprocess / JSON-RPC engine can reuse it unchanged
//! (PYTHON_SCRIPTING.md mentor item 5).

use cad_kernel::geom::Geom;
use cad_kernel::math::Vec2;
use cad_kernel::style::Style;

/// An op sent from the script worker to the host, tagged with the id the
/// reply must echo.
#[derive(Debug, Clone)]
pub struct ScriptOpMsg {
    pub id: u64,
    pub op: ScriptOp,
}

/// The host's answer to one op.
#[derive(Debug, Clone)]
pub struct ScriptOpReplyMsg {
    pub id: u64,
    pub reply: ScriptOpReply,
}

/// A request from a running script to read or mutate the document.
#[derive(Debug, Clone)]
pub enum ScriptOp {
    // ---- reads ----
    /// Number of model-space dobjects.
    DocCount,
    /// One dobject by index (owned copy).
    DocGet { index: usize },
    /// Every dobject (owned copies). Explicit — O(N) memory by choice.
    DocAll,
    /// The current selection indices.
    SelectionGet,
    /// All layers.
    LayersGet,
    /// The active layer id.
    LayerActive,
    /// All block definition names.
    BlocksGet,
    /// A sysvar value (None = unknown/unset).
    SysVarGet { name: String },
    /// The main canvas view (world centre + px-per-unit scale).
    ViewGet,

    // ---- writes ----
    /// Replace the selection.
    SelectionSet { indices: Vec<usize> },
    AddLine { a: Vec2, b: Vec2 },
    AddCircle { center: Vec2, radius: f64 },
    AddArc { center: Vec2, radius: f64, start_deg: f64, sweep_deg: f64 },
    AddEllipse { center: Vec2, major: Vec2, ratio: f64 },
    AddPolyline { vertices: Vec<Vec2>, closed: bool },
    AddPoint { at: Vec2 },
    AddText { text: String, at: Vec2, height: f64, angle_deg: f64 },
    /// Delete dobjects by index (highest first so earlier indices stay valid).
    Delete { indices: Vec<usize> },
    /// Create a layer. The host rejects duplicate / empty names.
    LayerAdd { name: String },
    LayerSetActive { name: String },
    /// Set one or more layer properties (None = leave unchanged).
    LayerSet {
        name: String,
        visible: Option<bool>,
        locked: Option<bool>,
        frozen: Option<bool>,
        plottable: Option<bool>,
        color_aci: Option<u8>,
    },
    /// Create a block definition from the CURRENT SELECTION and instance it
    /// at `base` (mirrors the `block <name>` command).
    BlockCreate { name: String, base: Vec2 },
    /// Insert a plain (non-parametric) block instance.
    BlockInsert { name: String, at: Vec2, rotation: f64 },
    /// Drive the existing command seams (run_command).
    Command { raw: String },
    SysVarSet { name: String, value: String },
    /// Move / zoom the main canvas. `scale` = px per world unit; None = pan only.
    ViewSet { center: Vec2, scale: Option<f64> },
    Save { path: String },
    Open { path: String },

    // ---- entity modification (P1: transform existing shapes) ----
    /// Move entities in place (hatch boundaries included).
    ModifyMove { indices: Vec<usize>, delta: Vec2 },
    /// Copy entities by `delta` (fresh handles); reply = the new indices.
    ModifyCopy { indices: Vec<usize>, delta: Vec2 },
    ModifyRotate { indices: Vec<usize>, pivot: Vec2, angle_deg: f64 },
    ModifyScale { indices: Vec<usize>, pivot: Vec2, factor: f64 },
    ModifyMirror { indices: Vec<usize>, a: Vec2, b: Vec2 },
    /// Per-entity style. Color: `-1` = ByLayer, `-2` = ByBlock, 0..=255 = ACI.
    SetEntityColor { indices: Vec<usize>, color: i32 },
    /// `name` = linetype name, or empty = ByLayer.
    SetEntityLinetype { indices: Vec<usize>, name: String },
    /// Move entities onto another layer (name).
    SetEntityLayer { indices: Vec<usize>, name: String },
    /// Lineweight in mm; negative = ByLayer.
    SetEntityLineweight { indices: Vec<usize>, mm: f64 },
    SetEntityVisible { indices: Vec<usize>, visible: bool },
    /// Replace ONE entity's GEOMETRY — the shape-specific properties
    /// (endpoints, center/radius, text string, …). The style stays.
    SetEntityGeom { index: usize, geom: Geom },

    // ---- P2 document-state reads ----
    DocUnits,
    /// Bbox of all model-space entities ((min), (max)); None = empty doc.
    DocBounds,
    LayoutsGet,
    LayoutSetActive { name: String },
    LinetypesGet,

    // ---- P3 ----
    /// Explicit undo-group boundary: everything since the last boundary (or
    /// the run start) collapses into ONE undo unit at the end of the run.
    UndoGroup,
    /// Current style for NEW entities (the script's own adds).
    SetCurrentColor { color: i32 },
    SetCurrentLinetype { name: String },
    SetCurrentLineweight { mm: f64 },

    // ---- P4 convenience ----
    ZoomExtents,

    // ---- hatching ----
    /// Create a hatch from EXPLICIT boundary entity indices (closed
    /// polylines / circles / ellipses / closed splines are accepted; other
    /// kinds are skipped loudly). `pattern` = "SOLID" or a catalog name.
    AddHatch { boundary_indices: Vec<usize>, pattern: String },
    /// Trace the smallest closed region around a world point (the app's
    /// pick-point primitive, islands included) and hatch it. Reply =
    /// `Indices(boundary)` — EMPTY when no closed region contains the point.
    HatchAt { point: Vec2, pattern: String },
    /// The hatch-pattern catalog names.
    HatchPatternsGet,
}

impl ScriptOp {
    /// Does this op mutate the document (and therefore need an undo unit)?
    pub fn is_write(&self) -> bool {
        !matches!(
            self,
            ScriptOp::DocCount
                | ScriptOp::DocGet { .. }
                | ScriptOp::DocAll
                | ScriptOp::SelectionGet
                | ScriptOp::LayersGet
                | ScriptOp::LayerActive
                | ScriptOp::BlocksGet
                | ScriptOp::SysVarGet { .. }
                | ScriptOp::ViewGet
                | ScriptOp::DocUnits
                | ScriptOp::DocBounds
                | ScriptOp::LayoutsGet
                | ScriptOp::LinetypesGet
                | ScriptOp::HatchPatternsGet
        )
    }
}

/// The host's answer to one op — owned data only.
#[derive(Debug, Clone)]
pub enum ScriptOpReply {
    Count(usize),
    Entity(Entity),
    Entities(Vec<Entity>),
    Indices(Vec<usize>),
    Layers(Vec<LayerInfo>),
    LayerActive(u32),
    Blocks(Vec<String>),
    SysVar(Option<String>),
    View(ViewInfo),
    /// Transcript of the history lines a `Command` op produced.
    CommandOutput(Vec<String>),
    /// The document's unit settings.
    Units(UnitsInfo),
    /// ((min), (max)) of all model-space entities; None = empty document.
    Bounds(Option<(Vec2, Vec2)>),
    /// The paper-space layouts.
    Layouts(Vec<LayoutInfo>),
    /// The linetype catalog names.
    Linetypes(Vec<String>),
    /// The hatch-pattern catalog names.
    Patterns(Vec<String>),
    /// Success with a number (new dobject index / new layer id).
    Ok(usize),
    /// Success, nothing to return.
    OkUnit,
    /// The op failed — the host has already surfaced the message to the user.
    Error(String),
}

/// The document's unit settings (P2).
#[derive(Debug, Clone)]
pub struct UnitsInfo {
    /// Display unit name ("mm", "cm", "m", "in", "ft", …).
    pub name: String,
    /// Scene units per ONE display unit.
    pub scene_per_unit: f64,
}

/// One paper-space layout (P2).
#[derive(Debug, Clone)]
pub struct LayoutInfo {
    pub id: u32,
    pub name: String,
    pub active: bool,
}

/// An owned snapshot of one dobject (D7).
#[derive(Debug, Clone)]
pub struct Entity {
    pub handle: u64,
    /// Resolved layer NAME (the host resolves the id → name).
    pub layer: String,
    /// RESOLVED style, ready to display: color as `"aci N"` / `"bylayer"` /
    /// `"byblock"` / `"#RRGGBB"`, linetype as its NAME (or "bylayer"),
    /// lineweight in mm (or -1.0 = ByLayer).
    pub color: String,
    pub linetype: String,
    pub lineweight: f32,
    pub visible: bool,
    pub geom: Geom,
    pub style: Style,
}

/// An owned snapshot of one layer.
#[derive(Debug, Clone)]
pub struct LayerInfo {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub frozen: bool,
    pub plottable: bool,
    /// "bylayer" | "aci N" | "truecolor #RRGGBB" (host-formatted).
    pub color: String,
}

/// The main canvas view.
#[derive(Debug, Clone, Copy)]
pub struct ViewInfo {
    /// World point at the canvas centre.
    pub center: Vec2,
    /// Pixels per world unit.
    pub scale: f64,
}

/// A script parameter's declared type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Float,
    Int,
    Bool,
    Str,
    /// A distance/length: the dialog edits it in the DOCUMENT'S DISPLAY
    /// unit (and the command line accepts suffixes — `25`, `25cm`, `6'`),
    /// the script receives SCENE units (consistent with all geometry).
    Length,
    /// A canvas position — the dialog shows x/y fields + "pick on canvas";
    /// the script receives a `(x, y)` tuple.
    Point,
    /// An EXISTING entity — the dialog's Pick button hit-tests the next
    /// canvas click; the script receives the entity's index (or -1 when
    /// nothing was picked).
    Entity,
    /// An ACI color — the dialog shows a swatch + the ACI picker wheel;
    /// the script receives the ACI number (0..=255).
    Color,
    /// A dropdown choice — the dialog shows a ComboBox over the declared
    /// `choices` (docstring: `name: help [a, b, c]`); the script receives
    /// the selected string (validated against the list).
    Choice,
    /// A LINETYPE dropdown — the host fills the choices from the document's
    /// linetype catalog at dialog time; the script receives the name.
    Linetype,
    /// A LAYER dropdown — the existing layers; the script receives the name.
    Layer,
    /// A BLOCK dropdown — the existing block definitions; the script
    /// receives the name.
    Block,
    /// A HATCH-PATTERN dropdown — the app's pattern catalog (same list as
    /// `rasm.hatch_patterns()`); the script receives the name.
    HatchPattern,
    /// A comma-separated list of floats — `"1, 2.5, 3"`; the script
    /// receives a `list[float]`. The dialog shows one text field.
    FloatList,
    /// A comma-separated list of ints → `list[int]`.
    IntList,
    /// A comma-separated list of strings → `list[str]`.
    StrList,
    /// A `;`-separated list of `x,y` points → `list[(x, y)]`.
    PointList,
}

/// One named, typed input a script declares (via `rasm.main(fn)` — the
/// function's signature + docstring are the declaration).
#[derive(Debug, Clone)]
pub struct ScriptParamMeta {
    pub name: String,
    pub ptype: ParamType,
    /// Default value as the script declared it (string form; "" = none).
    pub default: String,
    /// Optional range from the docstring: `name: help (min..max)`.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// Help text (docstring line `name: help`).
    pub help: String,
    /// The dropdown options for `ParamType::Choice` (docstring
    /// `name: help [a, b, c]`). Empty for other types.
    pub choices: Vec<String>,
}

/// The parameter declaration of one named script, read back from a
/// metadata pass (the file executes with every op no-op'd and
/// `rasm.main(fn)` records instead of drawing).
#[derive(Debug, Clone)]
pub struct ScriptMeta {
    pub name: String,
    pub params: Vec<ScriptParamMeta>,
}
