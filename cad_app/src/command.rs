//! Command metadata registry — **Phase 1 (schema only)**.
//!
//! See `mentor MD/COMMAND_REGISTRY_MENTOR.md`. This defines the TYPES that
//! *describe* commands (id, dispatch token, title, tooltip, category, icon). It
//! does **not** execute anything — execution still flows through
//! `run_command(cmd.dispatch)`, unchanged (Phase 0 freeze). No data is populated
//! here (that is Phase 2) and nothing renders from it yet (Phase 3+), so the app
//! is visually identical.
//!
//! The [`CommandInfo`] struct ACCUMULATES in later phases (never re-declared):
//! `keywords` / `group` in Phase 5; `visible` / `enabled` predicates in Phase 6b.
//! There is **no `aliases` field, ever** (parser aliases stay kernel-internal).
#![allow(dead_code)] // Phase 1: the types exist but are not wired up yet.

use std::collections::HashMap;

use crate::app::{GlyphKind, Tool};

/// Stable, namespaced command identity — e.g. `"draw.line"`. This is the
/// registry / UI / palette identity, and the value persisted in `draw_items`
/// (Phase 3), so it must be stable before persistence.
///
/// It is a **string, not an int/enum** (Phase 1 lock — an enum would break
/// "reuse the existing dispatch"). It is **owned** (`String`) rather than
/// `&'static str` because Phase 2 DERIVES it by concatenation
/// (`"<category>." + dispatch`), which allocates. The execution token,
/// [`CommandInfo::dispatch`], stays `&'static str`.
pub type CommandId = String;

/// Which glyph family paints a command's icon — spans BOTH painters:
/// `DrawGlyph(id)` → `draw_draw_glyph` (the draw-rail id string, col 1 of
/// `DRAW_CMDS`); `ModifyGlyph(kind)` → `draw_cmd_glyph` (col 1 of `MODIFY_CMDS`).
/// A flat id could not serve both. Lucide icons arrive later (Phase 7 refactor).
#[derive(Clone, Copy, Debug)]
pub enum IconId {
    DrawGlyph(&'static str),
    ModifyGlyph(GlyphKind),
}

/// Top-level command category. Extensible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CommandCategory {
    Draw,
    Modify,
}

/// Read-only **projection** of `CadApp` state for context predicates (Phase 6b,
/// D7). Built ONCE PER FRAME by `CadApp` (via `build_ctx`) and holds ONLY copied
/// values a predicate reads — never a `&mut CadApp`, `Rc<RefCell>`, interior
/// mutability, or a gateway back into the app (`ctx.app.*()` is forbidden). It
/// is `Copy`, so surfaces read a cheap snapshot. Extend it with more fields
/// (clipboard/snap/mode/…) only as a predicate actually needs them.
#[derive(Clone, Copy)]
pub struct Ctx {
    /// Number of currently-selected dobjects.
    pub selection_count: usize,
    /// Whether anything is selected.
    pub has_selection: bool,
    /// The active tool.
    pub active_tool: Tool,
    /// Whether the clipboard holds dobjects to paste.
    pub has_clipboard: bool,
}

/// Default visibility predicate — the command always appears. Most commands
/// point at this (Phase 6b: ALL do, so the app is unchanged). A `fn` POINTER,
/// never a capturing closure (D7 — no hidden state).
pub fn always_visible(_ctx: &Ctx) -> bool { true }

/// Default enabled predicate — the command is always clickable. Most commands
/// point at this (Phase 6b: ALL do). A `fn` pointer, never a capturing closure.
pub fn always_enabled(_ctx: &Ctx) -> bool { true }

/// Metadata that DESCRIBES a command — never how it executes.
///
/// Phase 1 shape. This struct is EXTENDED in later phases, never re-declared.
/// Holds no closures / `FnMut` / `&mut CadApp`.
#[derive(Clone, Debug)]
pub struct CommandInfo {
    /// Namespaced identity, e.g. `"draw.line"` (the HashMap key). See [`CommandId`].
    pub id: CommandId,
    /// The `run_command` token, e.g. `"line"`. The ONLY value ever passed to
    /// `run_command` — surfaces call `run_command(cmd.dispatch)`, never
    /// `run_command(cmd.id)`.
    pub dispatch: &'static str,
    /// Display name, e.g. `"Line"`.
    pub title: String,
    /// Hover text, e.g. `"Line  (L)"`.
    pub tooltip: String,
    /// `Draw` | `Modify`.
    pub category: CommandCategory,
    /// How the command's icon is painted.
    pub icon: IconId,
    /// Phase 5 — hand-authored UI **search terms** for the palette (`segment`,
    /// `straight` → Line). Single-source app metadata; **NOT parser aliases**
    /// (D6 — they never read or touch the parser). Nothing consumes these yet
    /// (the palette does, Phase 7).
    pub keywords: &'static [&'static str],
    /// Phase 5 — optional sub-group *within* a category (`Category → Section →
    /// Command`, e.g. Draw ▸ Curves ▸ Circle/Arc). `None` for all commands for
    /// now; Phase 6 menus assign and consume it.
    pub section: Option<&'static str>,
    /// Phase 6b — should the command appear at all? A pure `fn(&Ctx) -> bool`
    /// (fn pointer, never a capturing closure). Defaults to [`always_visible`].
    pub visible: fn(&Ctx) -> bool,
    /// Phase 6b — is the command clickable right now? A pure `fn(&Ctx) -> bool`.
    /// Defaults to [`always_enabled`]. UI: `if visible { if enabled { active }
    /// else { disabled } }`.
    pub enabled: fn(&Ctx) -> bool,
}

/// The metadata registry: lookup by [`CommandId`]. **Empty in Phase 1**
/// (populated in Phase 2). Holds no execution state — description only.
///
/// (The canonical ordered index used for deterministic menu order is added in
/// Phase 6; it is not needed yet.)
#[derive(Default)]
pub struct CommandRegistry {
    pub commands: HashMap<CommandId, CommandInfo>,
    /// Canonical order (seed / array order) — populated in [`build`]. A raw
    /// `HashMap` iterates randomly, so ordered surfaces (menus, the add-tool
    /// list) iterate THIS, filtered. Rails keep their own custom order in
    /// `draw_items` / `modify_items`. (Two orders: canonical vs custom.)
    pub order: Vec<CommandId>,
}

impl CommandRegistry {
    /// A new, empty registry.
    pub fn new() -> Self {
        Self { commands: HashMap::new(), order: Vec::new() }
    }

    /// Defensive lookup by id — `None` for a stale / unknown id (never panics).
    pub fn get(&self, id: &str) -> Option<&CommandInfo> {
        self.commands.get(id)
    }

    /// Ids of one category in canonical (seed) order — for the add-tool list and
    /// (later) menu generation. Iterates [`Self::order`], never the HashMap.
    pub fn by_category(&self, cat: CommandCategory) -> Vec<CommandId> {
        self.order
            .iter()
            .filter(|id| self.commands.get(*id).map(|c| c.category) == Some(cat))
            .cloned()
            .collect()
    }
}

/// Derive the display `title` from a rail tooltip by stripping a trailing
/// `(KEY)` hint: `"Line  (L)"` → `"Line"`, `"Rectangle  (REC)"` → `"Rectangle"`,
/// `"Wall"` → `"Wall"` (no hint), `"Elliptical arc"` → `"Elliptical arc"`.
/// (Wording is refined in a later phase; this is the derived seed.)
fn derive_title(tooltip: &str) -> String {
    match (tooltip.rfind('('), tooltip.ends_with(')')) {
        (Some(i), true) => tooltip[..i].trim_end().to_string(),
        _ => tooltip.trim().to_string(),
    }
}

/// Hand-authored UI **search keywords** per command (Phase 5), keyed on the
/// `dispatch` token. These are app-side discovery metadata for the palette
/// (Phase 7) — synonyms and related terms a user might type. They live in this
/// ONE place (no drift) and are **NOT parser aliases** (D6): they never touch or
/// read the parser. Unlisted commands get an empty slice.
fn keywords_for(dispatch: &str) -> &'static [&'static str] {
    match dispatch {
        // ── Draw ─────────────────────────────────────────────────────────
        "pointer"    => &["select", "selection", "pick", "escape"],
        "line"       => &["segment", "straight", "edge", "draw"],
        "pline"      => &["polyline", "connected", "chain", "multi-segment"],
        "circle"     => &["round", "ring", "radius", "diameter"],
        "arc"        => &["curve", "bend", "segment"],
        "rectangle"  => &["rect", "box", "square"],
        "ellipse"    => &["oval", "elliptical"],
        "ellipsearc" => &["oval", "elliptical", "arc", "curve"],
        "point"      => &["node", "dot", "vertex", "marker"],
        "spline"     => &["curve", "nurbs", "freeform", "bezier"],
        "wall"       => &["partition", "architectural", "double-line"],
        "text"       => &["label", "annotation", "note", "mtext"],
        "dim"        => &["dimension", "measure", "distance"],
        "hatch"      => &["fill", "pattern", "shade", "crosshatch"],
        // ── Modify ───────────────────────────────────────────────────────
        "move"       => &["translate", "shift", "relocate"],
        "copy"       => &["duplicate", "clone"],
        "rotate"     => &["turn", "spin", "angle"],
        "scale"      => &["resize", "size"],
        "mirror"     => &["flip", "reflect", "symmetry"],
        "stretch"    => &["extend", "resize", "deform"],
        "align"      => &["arrange", "line up"],
        "trim"       => &["cut", "clip", "shorten"],
        "extend"     => &["lengthen", "grow", "reach"],
        "fillet"     => &["round", "corner", "radius"],
        "chamfer"    => &["bevel", "corner", "angle"],
        "offset"     => &["parallel", "duplicate", "spacing"],
        "join"       => &["merge", "connect", "weld"],
        "break"      => &["split", "cut", "divide"],
        "lengthen 1" => &["lengthen", "extend", "shorten", "resize"],
        "reverse"    => &["flip", "invert", "direction"],
        "array"      => &["grid", "pattern", "repeat", "duplicate"],
        "matchprop"  => &["match", "properties", "copy format", "paint"],
        "chlayer"    => &["change layer", "move layer"],
        "erase"      => &["delete", "remove", "del"],
        "block"      => &["group", "symbol", "make block"],
        "insert"     => &["place", "block", "symbol"],
        "explode"    => &["ungroup", "break apart", "separate"],
        _            => &[],
    }
}

/// Build the registry by DERIVING every entry from the existing rail arrays
/// (`DRAW_CMDS` / `MODIFY_CMDS`) — the arrays stay the single source of truth
/// (Phase 2). Called once at startup by `CadApp`. Nothing renders from the
/// result yet.
///
/// Per entry (all derived, no hand-typing): `dispatch` = col 2; `id` =
/// `"<category>." + dispatch`; `tooltip` = col 3; `title` = [`derive_title`];
/// `icon` = `DrawGlyph`/`ModifyGlyph` of col 1.
pub fn build(
    draw: &[(&'static str, &'static str, &'static str)],
    modify: &[(GlyphKind, &'static str, &'static str)],
) -> CommandRegistry {
    let mut reg = CommandRegistry::new();
    for &(icon_id, dispatch, tooltip) in draw {
        let info = CommandInfo {
            id: format!("draw.{}", dispatch),
            dispatch,
            title: derive_title(tooltip),
            tooltip: tooltip.to_string(),
            category: CommandCategory::Draw,
            icon: IconId::DrawGlyph(icon_id),
            keywords: keywords_for(dispatch),
            section: None,   // Phase 6 assigns sub-groups; none yet
            visible: always_visible,   // Phase 6b: all default (app unchanged)
            enabled: always_enabled,
        };
        reg.order.push(info.id.clone());          // canonical (array) order
        reg.commands.insert(info.id.clone(), info);
    }
    for &(kind, dispatch, tooltip) in modify {
        let info = CommandInfo {
            id: format!("modify.{}", dispatch),
            dispatch,
            title: derive_title(tooltip),
            tooltip: tooltip.to_string(),
            category: CommandCategory::Modify,
            icon: IconId::ModifyGlyph(kind),
            keywords: keywords_for(dispatch),
            section: None,   // Phase 6 assigns sub-groups; none yet
            visible: always_visible,   // Phase 6b: all default (app unchanged)
            enabled: always_enabled,
        };
        reg.order.push(info.id.clone());
        reg.commands.insert(info.id.clone(), info);
    }
    reg
}
