// Command-line grammar.
//
// Grammar (all commands and keyword args are case-insensitive):
//
//   line     x1,y1 x2,y2
//   circle   cx,cy r
//   arc      cx,cy r start_deg end_deg            (CCW, start==end => full circle)
//   arc3p    p1 p2 p3                             (arc through three points)
//   arcse    cx,cy start end                      (center + start + end, CCW)
//   arccr    start end r [major|minor]            (chord + radius)
//   arccl    start end length [left|right]        (chord + arc length)
//   del      N
//   clear
//   help
//   grips                                         (toggle grip handles on selected)
//
//   end | mid | cen | int | per | tan | nea      (one-shot osnap override for
//                                                 the next click)

use crate::construct;
use crate::geom::*;
use crate::math::Vec2;
use crate::snap::SnapKind;

#[derive(Debug)]
pub enum Command {
    Add(Geom),
    Delete(usize),
    Clear,
    Help,
    /// Toggle grip handles on the currently selected dobject.
    GripsToggle,
    /// Arm an object-snap override for the next click.
    SnapOverride(SnapKind),
    /// Enter selection mode and, when the user presses Enter, dump details of
    /// each selected dobject to the command history. AutoCAD's `LIST`.
    List,
    /// Enter selection mode and keep the chosen dobjects as the active
    /// selection for follow-up commands. AutoCAD's `SELECT`.
    Select,
    /// Sub-commands recognised while a selection session is active. The
    /// parser emits these regardless of mode; the app ignores them outside
    /// of a select session.
    SelectAll,
    SelectPrevious,
    SelectNone,
    SelectRemoveMode,
    SelectAddMode,
    /// Arm window-selection: the next click captures the first corner of a
    /// window (selection mode only). Recognised both as a top-level
    /// convenience and as a sub-command during a select session.
    SelectWindow,
    /// Arm crossing-selection: same shape as SelectWindow but the next
    /// click captures corner 1 of a CROSSING window (R→L style — any
    /// dobject touching the box joins the basket).
    SelectCrossing,
    /// Add the most recently created dobject (highest index) to the
    /// selection basket. Distinct from `SelectPrevious` (which re-adds
    /// the whole last-finalised selection).
    SelectLast,
    /// Translate the current selection by the vector (end - base). The app
    /// captures the two clicks interactively.
    Move,
    /// Same as `Move` but leaves the originals in place — appends copies.
    Copy,
    /// Rotate the selection. App captures pivot + reference + target clicks.
    Rotate,
    /// Scale the selection uniformly. App captures pivot + reference distance + target distance.
    Scale,
    /// Mirror the selection across a line (two clicks define the line).
    Mirror,
    /// Fill closed polylines in the current selection with a named
    /// hatch pattern (or solid if `None`). `scale` and `angle_deg`
    /// default to 1.0 and 0.0 when not supplied. The app collects
    /// every closed polyline in the selection as boundary loops of
    /// ONE Hatch dobject (even-odd islands).
    Hatch { pattern: Option<String>, scale: f64, angle_deg: f64 },
    /// Delete every dobject in the current selection.
    DeleteSelected,
    /// Undo the most recent editing operation.
    Undo,
    /// Redo the most recently undone editing operation.
    Redo,
    /// Copy `style` (layer + color + linetype + lineweight + visibility)
    /// from a clicked source dobject to every dobject in the selection.
    MatchProps,
    /// Flip direction of every selected Line / Arc / EllipseArc / Polyline.
    Reverse,
    /// Bulk-set every selected dobject's `style.layer` to the active layer.
    ChangeLayer,
    /// Offset every selected dobject by a distance. App captures a side click.
    /// `None` means "use the persistent default" (`env.OfsDis`); the app
    /// resolves it. Matches the fillet pattern (radius optional).
    Offset(Option<f64>),
    /// Change-property command. Bulk-set one style attribute on every
    /// dobject in the current selection. AutoCAD CHPROP-style:
    ///   `chprop`                  → print sub-options
    ///   `chprop layer <name>`     → reassign to named layer
    ///   `chprop color <spec>`     → ACI number 0-256, "bylayer", "byblock"
    ///   `chprop linetype <name>`  → set linetype to named entry
    ///                               (or "bylayer" → CONTINUOUS id 0)
    /// Empty selection → enters select-mode and resumes via QueuedOp.
    ChProp(Option<(String, String)>),
    /// Linetype command — either list available linetypes (no arg) or
    /// set the ACTIVE layer's linetype to the named one. Inline with
    /// `layer`'s shape: `linetype` (list) / `linetype <name>` (set).
    /// Unknown name → app responds with a hint listing valid names.
    Linetype(Option<String>),
    /// Open the Session Recorder window. Pure UI toggle — does NOT
    /// start recording on its own. Use the Start button inside the
    /// window once it's open.
    DbgRecorder,
    /// Open the Text Style dialog (manage TextStyleTable entries —
    /// name, font, default height, optional color). With no args opens
    /// the dialog for a NEW style; with `style <name>` opens it on the
    /// existing style (case-insensitive lookup). Mirrors AutoCAD STYLE.
    TextStyle(Option<String>),
    /// Draw single-line text. Args: `text <string>` to inline the
    /// string; bare `text` opens a click-then-prompt flow (click
    /// position, then type the string). Heights default to the
    /// persisted SYSVAR `TxHt` (mirrors offset/wall sysvar pattern).
    /// Multi-word strings need quoting: `text "Hello world"`.
    Text(Option<String>),
    /// Draw a wall — two parallel lines spaced `thickness` apart,
    /// symmetric about the centerline between the two user clicks.
    /// `None` means "use the persistent default" (`env.WlThk`).
    /// Produces two normal `Line` dobjects so existing trim/extend/
    /// fillet operations work on the side lines directly.
    Wall(Option<f64>),
    /// AutoCAD-style smart dimension. No args — the app drives a
    /// 2- or 3-click flow with sub-kind auto-decided from the first
    /// click target (circle/arc → radius; key 'D' → diameter; point
    /// → linear, ortho inferred from the dimline drag direction).
    Dim,
    /// Open the Dim Style dialog. Bare `dimstyle` opens it for a NEW
    /// style; `dimstyle <name>` opens it on the existing style
    /// (case-insensitive). Mirrors AutoCAD DDIM / DIMSTYLE.
    DimStyle(Option<String>),
    /// Open the Wall Style Manager (Dry Wall / Structural / …). With a name,
    /// pre-select / edit that wall style.
    WallStyle(Option<String>),
    /// Create a block definition from the current selection: `block <name>`.
    /// App flow: select (universal model) → click base point → definition
    /// stored + selection replaced by one BlockRef instance.
    BlockDef(Option<String>),
    /// Place an instance of a named block: `insert <name>` → click point.
    Insert(Option<String>),
    /// Replace selected BlockRefs with transformed copies of their
    /// contents (select-first like erase).
    Explode,
    /// Compare two block definitions and report the candidate parametric
    /// rule (which geometry moves, by what vector). `blockdiff <A> <B>` by
    /// name, or bare `blockdiff` to PICK the two blocks on screen.
    /// Step 1 of programming-by-demonstration parametric blocks.
    BlockDiff(Option<(String, String)>),
    /// Block Task Recorder: explode the selected block into a temp sandbox
    /// and RECORD each stretch the user demonstrates as a parametric task
    /// (the reversible, intent-capturing alternative to `blockdiff`).
    BlockTaskRecorder,
    /// Finish the active Block Task Recorder → name the recorded tasks.
    BlockTaskFinish,
    /// CARD — cardinal-directions drafting lock (cursor constrained to
    /// ONLY horizontal or vertical from the anchor). `card` toggles;
    /// `card on` / `card off` set explicitly. Also on F8 and the
    /// status-strip badge.
    Card(Option<bool>),
    /// Lengthen every selected Line / Arc / EllipseArc by a signed delta.
    /// App captures a click on the end to extend.
    Lengthen(f64),
    /// Split a single selected dobject at the next click point.
    Break,
    /// Align: 4 clicks (2 source + 2 target). Translate + rotate.
    Align,
    /// Stretch: crossing window + 2 clicks (base + dest). Vertices inside
    /// the window translate by (dest - base).
    Stretch,
    /// Trim: two-basket flow (cutting edges + targets). App drives the
    /// select sessions and target-click phase.
    Trim,
    /// Extend: two-basket flow (boundary edges + targets). Mirror of trim.
    Extend,
    /// Fillet two dobjects with the given radius (None = use UserEnv default).
    /// App captures two clicks (object 1, object 2).
    Fillet(Option<f64>),
    /// Chamfer two dobjects with the given (d1, d2) distances. None for d2
    /// means d2 = d1. None for the option means use UserEnv defaults.
    Chamfer(Option<(f64, Option<f64>)>),
    /// Join: standard selection session; on Enter, merge the selected
    /// dobjects per the kernel's three-pass strategy (collinear lines →
    /// concentric arcs → chain-to-polyline).
    Join,
    /// AutoCAD DIST: measure distance + dx + dy + angle between two
    /// clicked points. Pure inspection — does not modify the doc.
    Dist,
    /// Open a file from disk (.dxf or .rsm) and load it into the document.
    Open(String),
    /// Save the current document to disk (.dxf or .rsm). Extension
    /// determines the format.
    SaveAs(String),
    /// Switch the active drawing tool — emitted when the user types a
    /// drawing keyword (`line` / `circle` / `arc` / `ellipse` / …) with
    /// NO coordinate arguments. The app sets self.tool to the matching
    /// variant and the user proceeds with clicks.
    SetTool(ToolKind),
}

impl Command {
    /// Canonical, capitalized name of the command — the form that gets
    /// echoed into the command-history "log book" so it's readable
    /// regardless of which alias the user actually typed (`F` → "Fillet",
    /// `o` → "Offset", etc.).
    pub fn canonical_name(&self) -> &'static str {
        match self {
            Command::Add(_)             => "Add",
            Command::Delete(_)          => "Delete",
            Command::Clear              => "Clear",
            Command::Help               => "Help",
            Command::GripsToggle        => "GripsToggle",
            Command::SnapOverride(_)    => "SnapOverride",
            Command::List               => "List",
            Command::Select             => "Select",
            Command::SelectAll          => "SelectAll",
            Command::SelectPrevious     => "SelectPrevious",
            Command::SelectNone         => "SelectNone",
            Command::SelectRemoveMode   => "SelectRemoveMode",
            Command::SelectAddMode      => "SelectAddMode",
            Command::SelectWindow       => "SelectWindow",
            Command::SelectCrossing     => "SelectCrossing",
            Command::SelectLast         => "SelectLast",
            Command::Move               => "Move",
            Command::Copy               => "Copy",
            Command::Rotate             => "Rotate",
            Command::Scale              => "Scale",
            Command::Mirror             => "Mirror",
            Command::Hatch { .. }       => "Hatch",
            Command::DeleteSelected     => "DeleteSelected",
            Command::Undo               => "Undo",
            Command::Redo               => "Redo",
            Command::MatchProps         => "MatchProps",
            Command::Reverse            => "Reverse",
            Command::ChangeLayer        => "ChangeLayer",
            Command::Offset(_)          => "Offset",
            Command::Wall(_)            => "Wall",
            Command::Text(_)            => "Text",
            Command::TextStyle(_)       => "TextStyle",
            Command::Dim                => "Dim",
            Command::DimStyle(_)        => "DimStyle",
            Command::WallStyle(_)       => "WallStyle",
            Command::BlockDef(_)        => "BlockDef",
            Command::Insert(_)          => "Insert",
            Command::Explode            => "Explode",
            Command::BlockDiff(_)       => "BlockDiff",
            Command::BlockTaskRecorder  => "BlockTaskRecorder",
            Command::BlockTaskFinish    => "BlockTaskFinish",
            Command::Card(_)            => "Card",
            Command::DbgRecorder        => "DbgRecorder",
            Command::Linetype(_)        => "Linetype",
            Command::ChProp(_)          => "ChProp",
            Command::Lengthen(_)        => "Lengthen",
            Command::Break              => "Break",
            Command::Align              => "Align",
            Command::Stretch            => "Stretch",
            Command::Trim               => "Trim",
            Command::Extend             => "Extend",
            Command::Fillet(_)          => "Fillet",
            Command::Chamfer(_)         => "Chamfer",
            Command::Join               => "Join",
            Command::Dist               => "Dist",
            Command::Open(_)            => "Open",
            Command::SaveAs(_)          => "SaveAs",
            Command::SetTool(_)         => "SetTool",
        }
    }
}

/// Parser-level mirror of the app's `Tool` enum. The app maps each
/// variant to its own `Tool` when dispatching `Command::SetTool`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolKind {
    Line,
    Circle,
    Arc,
    Ellipse,
    EllipseArc,
    Point,
    Polyline,
    Spline,
    Wall,
    Text,
    /// Axis-aligned rectangle. The app captures two opposite corners by
    /// click, OR — after the first corner — accepts a typed `width height`
    /// (signed; negatives extend left/down). Committed as a CLOSED 4-vertex
    /// Polyline (one LWPOLYLINE, matching AutoCAD RECTANG).
    Rectangle,
}

pub fn parse(line: &str) -> Result<Command, String> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    if toks.is_empty() {
        return Err("empty".into());
    }
    let head = toks[0].to_ascii_lowercase();

    // Snap keywords first — they're single-token and match a SnapKind directly.
    if toks.len() == 1 {
        if let Some(k) = SnapKind::parse(&head) {
            return Ok(Command::SnapOverride(k));
        }
    }

    match head.as_str() {
        "line"   | "l"  => parse_line(&toks[1..]),
        "circle" | "ci" => parse_circle(&toks[1..]),
        "ellipse" | "el" => parse_ellipse(&toks[1..]),
        // Elliptical arc has no typed-coordinate form — it's a 5-click
        // interactive tool. Always enter the tool. Reached by the Draw menu /
        // command rail ("ellipsearc") and by repeat-last (`tool_command_word`
        // returns "ellipsearc" for Tool::EllipseArc). Without this arm those
        // all failed with "unknown command 'ellipsearc'".
        "ellipsearc" | "ellipticalarc" | "ellarc" | "ea"
            => Ok(Command::SetTool(ToolKind::EllipseArc)),
        "point"   | "po" => parse_point(&toks[1..]),
        "polyline" | "pl" | "pline" => parse_polyline(&toks[1..]),
        "spline"   | "spl"          => Ok(Command::SetTool(ToolKind::Spline)),
        "rectangle" | "rectang" | "rec" => Ok(Command::SetTool(ToolKind::Rectangle)),
        "arc"    | "a"  => parse_arc(&toks[1..]),
        "arc3p"         => parse_arc_3p(&toks[1..]),
        "arcse"         => parse_arc_se(&toks[1..]),
        "arccr"         => parse_arc_cr(&toks[1..]),
        "arccl"         => parse_arc_cl(&toks[1..]),
        "del"    | "d"  => {
            let n: usize = toks.get(1)
                .ok_or("del N")?
                .parse()
                .map_err(|_| "bad index".to_string())?;
            Ok(Command::Delete(n))
        }
        "clear"          => Ok(Command::Clear),
        "help"  | "?"    => Ok(Command::Help),
        "grips" | "grip" => Ok(Command::GripsToggle),
        "list"  | "ls"   => Ok(Command::List),
        "select" | "sel" => Ok(Command::Select),
        // Selection sub-commands — only meaningful while a select session is
        // active. Outside of one the app responds with a hint.
        "all"             => Ok(Command::SelectAll),
        "prev" | "previous" | "before" => Ok(Command::SelectPrevious),
        "none" | "deselect" => Ok(Command::SelectNone),
        "rem"  | "remove"  => Ok(Command::SelectRemoveMode),
        "addmode" | "amode" => Ok(Command::SelectAddMode),
        "window" | "win" => Ok(Command::SelectWindow),
        "crossing"       => Ok(Command::SelectCrossing),
        "last"           => Ok(Command::SelectLast),
        "move" | "m"      => Ok(Command::Move),
        "copy" | "c" | "cp" | "co" => Ok(Command::Copy),
        "rotate" | "ro"   => Ok(Command::Rotate),
        "scale" | "sc"    => Ok(Command::Scale),
        "mirror" | "mi"   => Ok(Command::Mirror),
        "hatch" | "h" | "bhatch" => {
            // Optional args:  hatch [NAME] [scale] [angle_deg]
            // Bare `hatch` = solid; bare `hatch ANSI31` = pattern with
            // defaults; `hatch ANSI31 2.0 30` = scale 2x at +30°.
            let pattern = toks.get(1).map(|s| s.to_string());
            let scale     = toks.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
            let angle_deg = toks.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            Ok(Command::Hatch { pattern, scale, angle_deg })
        }
        "delete" | "erase" | "e" => Ok(Command::DeleteSelected),
        "undo" | "u"      => Ok(Command::Undo),
        "redo" | "y"      => Ok(Command::Redo),
        "matchprop" | "mp" => Ok(Command::MatchProps),
        "reverse" | "rev" => Ok(Command::Reverse),
        "chlayer" | "cl"  => Ok(Command::ChangeLayer),
        "offset" | "o"    => {
            // Bare `offset` → use env.OfsDis (the app resolves None).
            // `offset <d>` → use that distance and overwrite OfsDis on
            // successful apply.
            match toks.get(1) {
                None    => Ok(Command::Offset(None)),
                Some(s) => {
                    let d: f64 = s.parse().map_err(|_| "bad distance".to_string())?;
                    if d.abs() < 1e-12 { return Err("offset distance must be non-zero".into()); }
                    Ok(Command::Offset(Some(d)))
                }
            }
        }
        "chprop" | "chpr" | "props" | "properties" => {
            // `chprop`                → no-arg form, prints help
            // `chprop layer Walls`    → set layer
            // `chprop color 1`        → set color (ACI number or "bylayer")
            // `chprop linetype Dashed`→ set linetype
            match (toks.get(1), toks.get(2)) {
                (None, _) => Ok(Command::ChProp(None)),
                (Some(_), None) => Err("chprop: usage — chprop <layer|color|linetype> <value>".into()),
                (Some(what), Some(val)) => {
                    let what_lc = what.to_ascii_lowercase();
                    let what_norm = match what_lc.as_str() {
                        "layer" | "la" | "l"      => "layer",
                        "color" | "col" | "c"     => "color",
                        "linetype" | "ltype" | "lt" => "linetype",
                        other => return Err(format!(
                            "chprop: unknown property '{}' (try layer / color / linetype)", other)),
                    };
                    Ok(Command::ChProp(Some((what_norm.to_string(), (*val).to_string()))))
                }
            }
        }
        "linetype" | "ltype" | "lt" => {
            // Bare → list; `linetype <name>` → set active layer's
            // linetype. Multiple-word names not supported (the catalog
            // uses single tokens like "Continuous", "Dashed", "DashDot").
            match toks.get(1) {
                None    => Ok(Command::Linetype(None)),
                Some(s) => Ok(Command::Linetype(Some((*s).to_string()))),
            }
        }
        // NOTE: `rec` now means RECTANGLE (see above). The session recorder
        // keeps `recorder` / `dbg`.
        "dbg" | "recorder" => Ok(Command::DbgRecorder),
        "style" | "txtstyle" | "textstyle" => {
            // `style`        → open dialog for a NEW style
            // `style <name>` → open dialog editing the named style
            Ok(Command::TextStyle(toks.get(1).map(|s| (*s).to_string())))
        }
        "dim" | "dimension" => {
            // AutoCAD smart dimension. No args — app drives the
            // 2/3-click flow; sub-kind decided at first click.
            Ok(Command::Dim)
        }
        "dimstyle" | "ddim" => {
            // `dimstyle`        → open dialog for a NEW dim style
            // `dimstyle <name>` → open dialog editing the named style
            Ok(Command::DimStyle(toks.get(1).map(|s| (*s).to_string())))
        }
        "wallstyle" | "wstyle" => {
            Ok(Command::WallStyle(toks.get(1).map(|s| (*s).to_string())))
        }
        "block" | "b" => {
            Ok(Command::BlockDef(toks.get(1).map(|s| (*s).to_string())))
        }
        "insert" | "i" => {
            Ok(Command::Insert(toks.get(1).map(|s| (*s).to_string())))
        }
        "explode" | "xp" => Ok(Command::Explode),
        "blockdiff" | "bdiff" => {
            match (toks.get(1), toks.get(2)) {
                (Some(a), Some(b)) =>
                    Ok(Command::BlockDiff(Some(((*a).to_string(), (*b).to_string())))),
                (None, None) => Ok(Command::BlockDiff(None)),   // pick on screen
                _ => Err("usage: blockdiff   (pick on screen)   OR   blockdiff <A> <B>".into()),
            }
        }
        "btr" | "blocktask" | "taskrec" => Ok(Command::BlockTaskRecorder),
        "finish" | "endrec" | "done" => Ok(Command::BlockTaskFinish),
        "card" => {
            // `card` → toggle; `card on` / `card off` → set.
            match toks.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
                None         => Ok(Command::Card(None)),
                Some("on")  | Some("1") => Ok(Command::Card(Some(true))),
                Some("off") | Some("0") => Ok(Command::Card(Some(false))),
                Some(other) => Err(format!(
                    "card: expected `on` or `off`, got '{}'", other)),
            }
        }
        "text" | "tx" => {
            // `text`              → click position, then prompt for string
            // `text Hello`        → use "Hello" as the string (single word)
            // `text "Hi there"`   → quoted multi-word strings
            // Parser detail: toks[1..] joined; if the first arg starts
            // with " and the last ends with ", strip the quotes and use
            // the rest verbatim.
            match toks.get(1) {
                None => Ok(Command::Text(None)),
                Some(_) => {
                    let joined = toks[1..].join(" ");
                    let cleaned = if joined.starts_with('"') && joined.ends_with('"')
                        && joined.len() >= 2
                    {
                        joined[1..joined.len()-1].to_string()
                    } else { joined };
                    Ok(Command::Text(Some(cleaned)))
                }
            }
        }
        "wall" | "w"      => {
            // Bare `wall` → use env.WlThk (the app resolves None).
            // `wall <t>` → use that thickness AND persist it. Same
            // shape as `offset` for muscle-memory consistency.
            match toks.get(1) {
                None    => Ok(Command::Wall(None)),
                Some(s) => {
                    let t: f64 = s.parse().map_err(|_| "bad thickness".to_string())?;
                    if t <= 1e-12 { return Err("wall thickness must be positive".into()); }
                    Ok(Command::Wall(Some(t)))
                }
            }
        }
        "lengthen" | "len" => {
            let d: f64 = toks.get(1).ok_or("usage: lengthen <delta>")?
                .parse().map_err(|_| "bad delta".to_string())?;
            Ok(Command::Lengthen(d))
        }
        "break" | "br"    => Ok(Command::Break),
        "align"           => Ok(Command::Align),
        "stretch" | "st" | "s"   => Ok(Command::Stretch),
        "trim" | "tr"     => Ok(Command::Trim),
        "extend" | "ex"   => Ok(Command::Extend),
        "fillet" | "flt" | "f" => {
            // `fillet` alone → use default radius from UserEnv.
            // `fillet <r>`    → use this radius (and update UserEnv default).
            match toks.get(1) {
                None    => Ok(Command::Fillet(None)),
                Some(s) => {
                    let r: f64 = s.parse().map_err(|_| "bad radius".to_string())?;
                    if r < 0.0 { return Err("fillet radius must be >= 0".into()); }
                    Ok(Command::Fillet(Some(r)))
                }
            }
        }
        "chamfer" | "cha" => {
            // `chamfer`         → use default distances from UserEnv.
            // `chamfer <d>`     → both distances = d.
            // `chamfer <d1> <d2>` → use these two.
            match (toks.get(1), toks.get(2)) {
                (None, _) => Ok(Command::Chamfer(None)),
                (Some(s1), None) => {
                    let d: f64 = s1.parse().map_err(|_| "bad distance".to_string())?;
                    if d < 0.0 { return Err("chamfer distance must be >= 0".into()); }
                    Ok(Command::Chamfer(Some((d, None))))
                }
                (Some(s1), Some(s2)) => {
                    let d1: f64 = s1.parse().map_err(|_| "bad distance".to_string())?;
                    let d2: f64 = s2.parse().map_err(|_| "bad distance".to_string())?;
                    if d1 < 0.0 || d2 < 0.0 {
                        return Err("chamfer distances must be >= 0".into());
                    }
                    Ok(Command::Chamfer(Some((d1, Some(d2)))))
                }
            }
        }
        "join" | "j"      => Ok(Command::Join),
        "dist" | "di"     => Ok(Command::Dist),
        "open"            => {
            let path = toks.get(1)
                .ok_or("usage: open <path.dxf|path.rsm>")?
                .to_string();
            Ok(Command::Open(path))
        }
        "save" | "saveas" => {
            let path = toks.get(1)
                .ok_or("usage: save <path.dxf|path.rsm>")?
                .to_string();
            Ok(Command::SaveAs(path))
        }
        other            => Err(format!("unknown command '{}'", other)),
    }
}

fn parse_pt(s: &str) -> Result<Vec2, String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return Err(format!("expected x,y, got '{}'", s));
    }
    let x: f64 = parts[0].trim().parse().map_err(|_| format!("bad x: '{}'", parts[0]))?;
    let y: f64 = parts[1].trim().parse().map_err(|_| format!("bad y: '{}'", parts[1]))?;
    Ok(Vec2::new(x, y))
}

fn parse_line(args: &[&str]) -> Result<Command, String> {
    // Bare `line` → enter draw-line mode (interactive). With args, add now.
    if args.is_empty() {
        return Ok(Command::SetTool(ToolKind::Line));
    }
    if args.len() != 2 {
        return Err("usage: line  OR  line x1,y1 x2,y2".into());
    }
    Ok(Command::Add(Geom::Line(Line {
        a: parse_pt(args[0])?,
        b: parse_pt(args[1])?,
    })))
}

fn parse_point(args: &[&str]) -> Result<Command, String> {
    if args.len() != 1 {
        return Err("usage: point x,y".into());
    }
    Ok(Command::Add(Geom::Point(Point {
        location: parse_pt(args[0])?,
        style:    0,
        size:     0.0,
    })))
}

fn parse_polyline(args: &[&str]) -> Result<Command, String> {
    // Bare `polyline` / `pl` / `pline` → enter interactive draw-polyline mode.
    // With args, add now: `polyline x1,y1 x2,y2 …` (closed if the last token is
    // the literal "close" / "closed").
    if args.is_empty() {
        return Ok(Command::SetTool(ToolKind::Polyline));
    }
    if args.len() < 2 {
        return Err("usage: polyline  OR  polyline x1,y1 x2,y2 [x3,y3 …] [close]".into());
    }
    let (vert_args, closed) = match args.last().map(|s| s.to_ascii_lowercase()) {
        Some(ref s) if s == "close" || s == "closed" => (&args[..args.len()-1], true),
        _ => (args, false),
    };
    let mut vertices = Vec::with_capacity(vert_args.len());
    for tok in vert_args {
        vertices.push(PolyVertex { pos: parse_pt(tok)?, bulge: 0.0 });
    }
    Ok(Command::Add(Geom::Polyline(Polyline { vertices, closed, widths: Vec::new() })))
}

fn parse_circle(args: &[&str]) -> Result<Command, String> {
    // Bare `circle` / `ci` → enter draw-circle mode. With args, add now.
    if args.is_empty() {
        return Ok(Command::SetTool(ToolKind::Circle));
    }
    if args.len() != 2 {
        return Err("usage: circle  OR  circle cx,cy r".into());
    }
    let r: f64 = args[1].parse().map_err(|_| "bad radius".to_string())?;
    if r <= 0.0 {
        return Err("radius must be > 0".into());
    }
    Ok(Command::Add(Geom::Circle(Circle {
        center: parse_pt(args[0])?,
        radius: r,
    })))
}

/// `ellipse cx,cy major_end_x,major_end_y minor_len` — three tokens.
/// `major_end` is in WORLD coordinates: a point at the tip of the
/// semi-major axis (relative to the centre, that gives both direction and
/// length). `minor_len` is the semi-minor axis length.
fn parse_ellipse(args: &[&str]) -> Result<Command, String> {
    // Bare `ellipse` / `el` → enter the interactive draw tool (matches
    // line/circle/arc/polyline). With args, build the ellipse now.
    if args.is_empty() {
        return Ok(Command::SetTool(ToolKind::Ellipse));
    }
    if args.len() != 3 {
        return Err("usage: ellipse  OR  ellipse cx,cy major_end_x,major_end_y minor_len".into());
    }
    let center = parse_pt(args[0])?;
    let major_end = parse_pt(args[1])?;
    let minor: f64 = args[2].parse().map_err(|_| "bad minor length".to_string())?;
    construct::ellipse_center_major_minor(center, major_end, minor)
        .map(|e| Command::Add(Geom::Ellipse(e)))
        .ok_or_else(|| "degenerate inputs (zero major or minor)".into())
}

// ---- Arc construction methods ----

/// METHOD 1 — center + radius + start_deg + end_deg.
/// Bare `arc` / `a` → enter interactive draw-arc mode.
fn parse_arc(args: &[&str]) -> Result<Command, String> {
    if args.is_empty() {
        return Ok(Command::SetTool(ToolKind::Arc));
    }
    if args.len() != 4 {
        return Err("usage: arc  OR  arc cx,cy r start_deg end_deg".into());
    }
    let r:   f64 = args[1].parse().map_err(|_| "bad radius".to_string())?;
    let sd:  f64 = args[2].parse().map_err(|_| "bad start angle".to_string())?;
    let ed:  f64 = args[3].parse().map_err(|_| "bad end angle".to_string())?;
    if r <= 0.0 {
        return Err("radius must be > 0".into());
    }
    let sweep_deg = (ed - sd).rem_euclid(360.0);
    let sweep_deg = if sweep_deg < 1e-6 { 360.0 } else { sweep_deg };
    Ok(Command::Add(Geom::Arc(Arc {
        center: parse_pt(args[0])?,
        radius: r,
        start_angle: sd.to_radians().rem_euclid(std::f64::consts::TAU),
        sweep_angle: sweep_deg.to_radians(),
    })))
}

/// METHOD 2 — three points on the arc
fn parse_arc_3p(args: &[&str]) -> Result<Command, String> {
    if args.len() != 3 {
        return Err("usage: arc3p p1 p2 p3".into());
    }
    let p1 = parse_pt(args[0])?;
    let p2 = parse_pt(args[1])?;
    let p3 = parse_pt(args[2])?;
    let arc = construct::arc_three_points(p1, p2, p3)
        .ok_or_else(|| "three points are collinear, no arc".to_string())?;
    Ok(Command::Add(Geom::Arc(arc)))
}

/// METHOD 3 — center, start point, end point (CCW)
fn parse_arc_se(args: &[&str]) -> Result<Command, String> {
    if args.len() != 3 {
        return Err("usage: arcse cx,cy start end".into());
    }
    let c = parse_pt(args[0])?;
    let s = parse_pt(args[1])?;
    let e = parse_pt(args[2])?;
    let arc = construct::arc_center_start_end(c, s, e)
        .ok_or_else(|| "zero radius (start coincides with center)".to_string())?;
    Ok(Command::Add(Geom::Arc(arc)))
}

/// METHOD 4 — chord (start, end) + radius
fn parse_arc_cr(args: &[&str]) -> Result<Command, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err("usage: arccr start end r [major|minor]".into());
    }
    let s = parse_pt(args[0])?;
    let e = parse_pt(args[1])?;
    let r: f64 = args[2].parse().map_err(|_| "bad radius".to_string())?;
    let major = match args.get(3).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("major") => true,
        Some("minor") | None => false,
        Some(other) => return Err(format!("expected 'major' or 'minor', got '{}'", other)),
    };
    let arc = construct::arc_chord_radius(s, e, r, major)
        .ok_or_else(|| "chord longer than 2r, or zero inputs".to_string())?;
    Ok(Command::Add(Geom::Arc(arc)))
}

/// METHOD 5 — chord (start, end) + arc length
fn parse_arc_cl(args: &[&str]) -> Result<Command, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err("usage: arccl start end length [left|right]".into());
    }
    let s = parse_pt(args[0])?;
    let e = parse_pt(args[1])?;
    let length: f64 = args[2].parse().map_err(|_| "bad length".to_string())?;
    let flip = match args.get(3).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("right") => true,
        Some("left") | None => false,
        Some(other) => return Err(format!("expected 'left' or 'right', got '{}'", other)),
    };
    let arc = construct::arc_chord_length(s, e, length, flip)
        .ok_or_else(|| "chord longer than arc length, or degenerate".to_string())?;
    Ok(Command::Add(Geom::Arc(arc)))
}
