// egui front-end. Pure visualization + command dispatch + interactive draw tools.
// All geometry comes from cad_kernel — no math defined in this file.

use crate::dock::DockHost;
use eframe::egui; // brings `.show()` into scope for the dock host

/// The Goan villa scene, exported from the Blender dev build by
/// `villa scene/build/export_factory.py`: architecture, hard landscape, the procedural frangipani
/// and the scattered grass, merged into three meshes across 25 material parts. ~2.0 M triangles —
/// the full Blender scene is 89.7 M, of which 98% is imported planting no real-time viewport can
/// hold. Authored in **metres at real-world size** (86 × 74 × 10.6 m), so it imports 1:1.
const VILLA_SCENE: &str = r"G:\blender dev\staircase\villa scene\villa_scene.glb";

/// Which handle a parametric door will actually wear. Resolved ONCE per build, because two
/// decisions hang off it that must not disagree: whether to weld the library mesh on, and whether
/// the door draws its own lever. Deciding them separately is how a leaf ends up with two levers at
/// the same spindle, or with none at all.
enum HandleChoice {
    /// No library, or nothing chosen — the door wears its own built-in rose and lever.
    Builtin,
    /// One was chosen but does not fit this leaf. Falls back to the built-in lever and SAYS SO;
    /// hardware that silently fails to appear reads as a bug in the door.
    Refused(String),
    /// Weld this one on, and drop the built-in lever.
    Use(Box<crate::handles::Handle>, crate::handles::DoorFit),
}

/// The door preview window's state.
///
/// TWO caches, not one, because the two costs are an order of magnitude apart: rebuilding the mesh
/// re-reads and re-parses the handle's FBX, while re-rendering only re-shades pixels. Orbiting must
/// pay the second and never the first.
#[derive(Default)]
struct DoorPreview {
    open: bool,
    view: crate::mesh_preview::View,
    zoom: f32,
    /// Parameters + handle the cached MESH was built from.
    geom_sig: u64,
    mesh: Option<(crate::mesh_io::ObjMesh, Vec<u32>)>,
    /// What the door reports about itself, and any note about a refused handle.
    caption: String,
    /// Everything the picture depends on EXCEPT its quality.
    img_sig: u64,
    /// The supersampling the cached image was rendered at — 0 = nothing cached. Draft first,
    /// refined on the next idle frame; see `door_preview_window`.
    tex_ss: usize,
    tex: Option<egui::TextureHandle>,
}

/// Hash anything the preview's picture depends on. Floats go in by bit pattern — the point is
/// "did this change", not "is this close to that".
fn preview_hash(parts: &[f32], text: &[&str]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for v in parts {
        v.to_bits().hash(&mut h);
    }
    for s in text {
        s.hash(&mut h);
    }
    h.finish()
}

/// How a door's triangle should look, by its `Part` id: joinery follows the chosen materials,
/// ironmongery follows the handle finish. Both the preview and the built object resolve through
/// this one function, so a door cannot preview in walnut and insert in oak.
fn door_part_look(
    mats: &crate::door_mat::DoorMaterials,
    finish: &str,
    part: u32,
) -> crate::mesh_preview::PartLook {
    match mats.for_part(part) {
        Some(m) => m.look(),
        // Hinges, the door's own lever, and every part a welded handle brought with it.
        None => finish_look(finish),
    }
}

/// A handle finish as a surface. Names come from the manifest (`satin_nickel`, `matt_black`, …);
/// matching on a KEYWORD rather than the exact string means a new finish in a future manifest
/// looks approximately right instead of defaulting to nothing.
fn finish_look(name: &str) -> crate::mesh_preview::PartLook {
    use crate::mesh_preview::PartLook;
    let n = name.to_ascii_lowercase();
    let m = |albedo: [f32; 3], roughness: f32| PartLook {
        albedo,
        roughness,
        metallic: 1.0,
        ..Default::default()
    };
    let d = |albedo: [f32; 3], roughness: f32| PartLook {
        albedo,
        roughness,
        ..Default::default()
    };
    if n.contains("black") || n.contains("anthracite") {
        // Powder coat is a dielectric, not a metal — as metal it renders as a dark mirror.
        d([0.020, 0.020, 0.022], 0.42)
    } else if n.contains("white") {
        d([0.80, 0.80, 0.79], 0.40)
    } else if n.contains("brass") || n.contains("gold") {
        m(
            [0.62, 0.48, 0.21],
            if n.contains("polish") { 0.12 } else { 0.30 },
        )
    } else if n.contains("bronze") || n.contains("copper") {
        m([0.44, 0.28, 0.20], 0.34)
    } else if n.contains("chrome") {
        m([0.56, 0.57, 0.58], 0.06)
    } else if n.contains("steel") || n.contains("stainless") {
        m([0.52, 0.52, 0.52], 0.22)
    } else {
        m([0.52, 0.51, 0.49], 0.28) // satin nickel — the library default
    }
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc as StdArc;
use std::sync::Mutex;
use std::thread;

use cad_kernel::*;

use crate::gpu::{
    view_matrix, ArcInstance, CircleInstance, EllipseInstance, FillVertex, GpuShapeRenderer,
    LineInstance,
};
use crate::settings::UserEnv;

// PEDIT (polyline edit) methods live in `src/app/pedit.rs`. Child module of
// `app`, so it can reach CadApp's private fields/helpers; its methods are
// `pub(crate)` so call sites in this file work unchanged.
mod pedit;
mod ui;
use self::ui::*;
// `click_menu_button` is called from `light.rs` as `crate::app::…`, so the
// glob above (private) is not enough: the name needs a pub re-export.
pub(crate) use self::ui::click_menu_button;
// `GlyphKind` lives in `ui.rs` but `command.rs` names it as `crate::app::GlyphKind`.
pub(crate) use self::ui::GlyphKind;
// `point_in_polygon` exists in both `cad_kernel` (glob above) and `ui`; the
// app's own generic version must win for the callers in this module.
use self::ui::point_in_polygon;
mod render2d;
use self::render2d::*;
mod ports_gpu;
use self::ports_gpu::*;
mod io;
use self::io::*;
mod ports_raster;
use self::ports_raster::*;
mod ports_param;
use self::ports_param::*;
// SIMLUX lighting integration methods (grafted from simLUX main) live in
// `src/app/ports_simlux.rs`. Child module of `app`, so it can reach CadApp's
// private fields/helpers; entry-point methods are `pub(crate)` so call sites
// in this file work unchanged.
mod ports_simlux;
// The egui `App` impl (per-frame shell: chrome, tabs, panels, canvas) lives
// in `src/app/shell.rs`. Trait methods need no visibility — eframe drives
// them; the shell calls CadApp privates via `use super::*`.
mod shell;
// The left mode command panel (2D | SIMLUX | Factory sections + the room
// details modal) lives in `src/app/panel.rs`. Methods are `pub(super)` so
// the shell and tests can reach them.
mod panel;
// The 2D command machinery (Slice K/L/M applies, Blocks, command flows,
// trim/fillet/chamfer/join, coords, array) lives in `src/app/cmd2d.rs`.
mod cmd2d;
// Floating windows/dialogs (ACI picker, inspector + command palette,
// Materials Factory, path-tracer render dialogs, Groups, fixture sync)
// live in `src/app/windows.rs`.
mod windows;
// The 2D workspace's remaining dialogs & flows (layer panel chrome +
// dock glue, hatch/text dialogs, python engine pump + editor, ATTDEF/
// ATTEDIT, CTB + plot pipeline) live in `src/app/dialogs.rs`.
mod dialogs;

// The test modules that used to fill the tail of this file now live in
// `src/app/tests/` (child module of `app`, so they still see its privates).
#[cfg(test)]
mod tests;

// Soft cap on candidate-set pair count. Above this an ∩ query refuses to
// compute (with a message), to prevent multi-second / multi-minute freezes.
// 5 million pairs is roughly half a second on this CPU.
const PAIR_LIMIT: usize = 5_000_000;

/// Spacing of the point sources a linear fitting is represented by, metres.
///
/// A curved light is a LINE of light and the engine's luminaire is a point with a distribution, so
/// the run is sampled into points that share its flux. 0.25 m is well under any real mounting
/// height, and the error of the approximation only shows closer to the fitting than the spacing
/// itself — at which distance a real diffuser is not a line source either.
const EMITTER_SPACING_M: f32 = 0.25;

/// The narrowest a view is allowed to be squeezed to while it is open, in points.
///
/// The three views — 2D CAD, 3D Factory, SIMLUX — are peers, and each may be dragged to any width
/// the user likes; the only rule is that an OPEN view keeps enough to be a view. Without it the
/// first panel laid out can take the whole window, leaving the next one nowhere to go: it starts
/// at x = 0, overlaps the first, and — being drawn second — paints on top. That is the reported
/// "it only extends to a length and when extend it beyond that it goes behind the 3d factory
/// window". A CLOSED view reserves nothing, so any one of them alone can fill the window.
const MIN_VIEW_W: f32 = 260.0;

/// The most emitting points one fitting may be sampled into.
///
/// WHY THERE HAS TO BE A CEILING. The spacing above is a length, so the count is the PATH LENGTH
/// divided by it — and a curved light swept along a drawn 2D curve has no bound on its path. A ring
/// of 30 m radius is 188 m around, which at 0.25 m is 753 point sources for ONE fitting; three of
/// them is 2 259 luminaires, and every calculation point, every cylindrical sample and every
/// surface sample then fires a shadow ray at each. That is the freeze the user hit on Calculate.
///
/// Capping the COUNT rather than the flux keeps the physics: the total is `path × W/m × efficacy`
/// however many points it is divided into, so a longer run simply gets a coarser sampling of the
/// same line. What that costs is accuracy CLOSE to the fitting, within about one spacing of it —
/// so the spacing actually used is stated on the fixture rather than applied quietly.
pub const MAX_EMITTERS_PER_FIXTURE: usize = 120;

/// Emitter spacing for a run of `path_len` metres: [`EMITTER_SPACING_M`], opened up only as far as
/// [`MAX_EMITTERS_PER_FIXTURE`] requires.
fn emitter_spacing_for(path_len: f32) -> f32 {
    (path_len / MAX_EMITTERS_PER_FIXTURE as f32).max(EMITTER_SPACING_M)
}

// ── Properties-panel chrome ────────────────────────────────────────────────
// These now ALIAS the design-token module (`crate::theme`) — the single source
// of truth (THEME_SYSTEM.md §5). Do not put raw hex here; change values in
// `theme.rs`. Kept as `PP_*` names so the (many) existing call sites are
// untouched while the token migration proceeds panel-by-panel.
const PP_BG_LO: egui::Color32 = crate::theme::color::SURFACE_0;
const PP_BG_HI: egui::Color32 = crate::theme::color::SURFACE_2;
const PP_BORDER: egui::Color32 = crate::theme::color::BORDER;
const PP_TEXT: egui::Color32 = crate::theme::color::TEXT_PRIMARY;
const PP_LABEL: egui::Color32 = crate::theme::color::TEXT_MUTED;
const PP_MUTED: egui::Color32 = crate::theme::color::TEXT_SECONDARY;
const PP_ACCENT: egui::Color32 = crate::theme::color::ACCENT;
const PP_LABEL_W: f32 = 84.0; // fixed left-label column width
const PP_ROW_H: f32 = crate::theme::space::CONTROL_H; // uniform field height (24)
const PP_ROW_GAP: f32 = crate::theme::space::ROW_GAP; // vertical gap between rows (8)
/// Trailing area (name/value + dropdown arrow) reserved to the right of the
/// linetype dash / lineweight bar preview, so both previews share one length.
/// Wide enough for a 10-letter linetype name + variant (e.g. "Continuous").
const PP_PREVIEW_TRAIL: f32 = 96.0;
/// Standard synthetic-italic shear angle (~12°) — the value the text dialog's
/// Row-4 Italic toggle writes to `TextStyle.oblique` / `Text.oblique`.
const ITALIC_RAD: f64 = 0.209_439_5;

/// A labelled property row: muted fixed-width label on the left, a value
/// area that fills the rest. `add` receives the value-cell width so every
/// field lines up at the same left edge and width.
fn pp_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui, f32)) {
    ui.horizontal(|ui| {
        let (lr, _) =
            ui.allocate_exact_size(egui::vec2(PP_LABEL_W, PP_ROW_H), egui::Sense::hover());
        let ltr = ui.painter().text(
            egui::pos2(lr.left(), lr.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            crate::theme::typ::body(),
            PP_LABEL,
        );
        pp_capture(
            ui,
            &format!(
                "{lbl} · LABEL  text='{lbl}' color={} font=13px  col-width={:.0}  pad-top={:.1}",
                pp_hex(PP_LABEL),
                PP_LABEL_W,
                ltr.top() - lr.top(),
                lbl = label
            ),
            lr,
        );
        let w = (ui.available_width() - 2.0).max(40.0);
        add(ui, w);
    });
    ui.add_space(PP_ROW_GAP);
}

/// Color32 → `#RRGGBB` for the UI-inspect dump.
fn pp_hex(c: egui::Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

/// Painted downward dropdown triangle (▼) — a real shape, since the `▾` char
/// renders as a tofu box in the default font. Sits `input-pad`(8)+ from the
/// field's right edge, vertically centered. Returns its rect (for capture).
fn pp_arrow(p: &egui::Painter, box_rect: egui::Rect) -> egui::Rect {
    let cx = box_rect.right() - 12.0;
    let cy = box_rect.center().y;
    let s = 3.5;
    p.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx - s, cy - s * 0.5),
            egui::pos2(cx + s, cy - s * 0.5),
            egui::pos2(cx, cy + s * 0.75),
        ],
        PP_LABEL,
        egui::Stroke::NONE,
    ));
    egui::Rect::from_center_size(egui::pos2(cx, cy), egui::vec2(2.0 * s, 2.0 * s))
}

/// Rich capture for a field, so the UI-inspect dump can be rebuilt as HTML:
/// records the box (fill / border / radius) + the value text (content, color,
/// font, and REAL measured top/bottom/left padding from the drawn text rect) +
/// whether it carries a dropdown arrow.
fn pp_cap_field(
    ui: &egui::Ui,
    label: &str,
    box_rect: egui::Rect,
    text_rect: egui::Rect,
    text: &str,
    text_col: egui::Color32,
    font: f32,
    arrow: bool,
) {
    let pl = text_rect.left() - box_rect.left();
    let pt = text_rect.top() - box_rect.top();
    let pb = box_rect.bottom() - text_rect.bottom();
    let detail = format!(
        "{lbl} · FIELD  w={:.0} h={:.0} @({:.0},{:.0})  |  box fill={} border={} radius={}  |  \
text='{txt}' color={tc} font={ft:.0}px  pad-left={:.1} pad-top={:.1} pad-bottom={:.1}  |  arrow={arw}",
        box_rect.width(), box_rect.height(), box_rect.left(), box_rect.top(),
        pp_hex(PP_BG_LO), pp_hex(PP_BORDER), crate::theme::radius::SM,
        pl, pt, pb,
        lbl = label, txt = text, tc = pp_hex(text_col), ft = font,
        arw = if arrow { "down-triangle ▼" } else { "none" });
    pp_capture(ui, &detail, box_rect);
}

/// Read-only key/value row — muted label + a muted mono value at the unified
/// value start, with NO field box (design: derived/computed values like Length,
/// Angle, Area render borderless and muted).
fn pp_kv(ui: &mut egui::Ui, label: &str, value: &str) {
    pp_row(ui, label, |ui, _w| {
        let r = ui.add(egui::Label::new(
            egui::RichText::new(value)
                .font(crate::theme::typ::data_value())
                .color(PP_MUTED),
        ));
        pp_capture(
            ui,
            &format!(
                "{lbl} · READ-ONLY  value='{val}' color={} font=12px-mono  NO box/border",
                pp_hex(PP_MUTED),
                lbl = label,
                val = value
            ),
            r.rect,
        );
    });
}

/// Two column headers ("Start" / "End", or "X" / "Y") above a coordinate-pair
/// row — aligned over the two value boxes.
fn pp_pair_headers(ui: &mut egui::Ui, a: &str, b: &str) {
    ui.horizontal(|ui| {
        let (lr, _) = ui.allocate_exact_size(egui::vec2(PP_LABEL_W, 13.0), egui::Sense::hover());
        let vx = lr.right() + ui.spacing().item_spacing.x;
        let gap = crate::theme::space::COLUMN_GAP;
        let half = (ui.available_width() - gap) / 2.0;
        let y = lr.center().y;
        let p = ui.painter();
        // Lighter than field labels: dim colour, 11/400 regular (MENTOR §5.1).
        let hdr = crate::theme::color::COLUMN_HEADER;
        p.text(
            egui::pos2(vx, y),
            egui::Align2::LEFT_CENTER,
            a,
            crate::theme::typ::hint(),
            hdr,
        );
        p.text(
            egui::pos2(vx + half + gap, y),
            egui::Align2::LEFT_CENTER,
            b,
            crate::theme::typ::hint(),
            hdr,
        );
    });
    ui.add_space(2.0);
}

/// Paint a numeric field that looks EXACTLY like a `pp_box` field (surface-0,
/// radius 4, 1px border drawn UNCLIPPED so the whole border shows) with a
/// frameless Mono-12 DragValue inside, instead of egui's default DragValue
/// chrome. The caller supplies the exact `rect`, so a Start/End pair gets precise
/// geometry (equal halves + an 8px gap summing to the full field width).
/// Widget-visual + font overrides are saved/restored so they don't leak.
/// INSPECTOR_DESIGN_MENTOR §5.
fn pp_num_field(ui: &mut egui::Ui, rect: egui::Rect, v: &mut f64) -> egui::Response {
    ui.painter().rect(
        rect,
        egui::Rounding::same(crate::theme::radius::SM),
        PP_BG_LO,
        egui::Stroke::new(1.0, PP_BORDER),
    );
    let pad = crate::theme::space::INPUT_PAD;
    let inner = egui::Rect::from_min_size(
        egui::pos2(rect.left() + pad, rect.center().y - 8.0),
        egui::vec2((rect.width() - 2.0 * pad).max(10.0), 16.0),
    );
    let saved_font = ui.style().override_font_id.clone();
    let saved_widgets = ui.visuals().widgets.clone();
    ui.style_mut().override_font_id = Some(crate::theme::typ::data_value());
    {
        let vis = ui.visuals_mut();
        for st in [
            &mut vis.widgets.inactive,
            &mut vis.widgets.hovered,
            &mut vis.widgets.active,
        ] {
            st.weak_bg_fill = egui::Color32::TRANSPARENT;
            st.bg_fill = egui::Color32::TRANSPARENT;
            st.bg_stroke = egui::Stroke::NONE;
        }
    }
    let r = ui.put(
        inner,
        egui::DragValue::new(v)
            .update_while_editing(false)
            .speed(0.5)
            .max_decimals(2),
    );
    ui.style_mut().override_font_id = saved_font;
    ui.visuals_mut().widgets = saved_widgets;
    r
}

/// A label + two side-by-side numeric fields (Start/End or X/Y). Returns
/// (changed, [both responses]) so the caller can snapshot + write back.
fn pp_pair_row(
    ui: &mut egui::Ui,
    label: &str,
    a: &mut f64,
    b: &mut f64,
) -> (bool, Vec<egui::Response>) {
    let mut changed = false;
    let mut resps = Vec::new();
    pp_row(ui, label, |ui, w| {
        let gap = crate::theme::space::COLUMN_GAP;
        let half = ((w - gap) / 2.0).max(40.0);
        // Explicit rects so the pair spans the full value width exactly:
        // [half] + gap(8) + [half] = w, matching the GENERAL bars.
        let (row, _) = ui.allocate_exact_size(egui::vec2(w, PP_ROW_H), egui::Sense::hover());
        let a_rect = egui::Rect::from_min_size(row.min, egui::vec2(half, PP_ROW_H));
        let b_rect = egui::Rect::from_min_size(
            egui::pos2(row.left() + half + gap, row.top()),
            egui::vec2(half, PP_ROW_H),
        );
        let ra = pp_num_field(ui, a_rect, a);
        let rb = pp_num_field(ui, b_rect, b);
        changed = ra.changed() || rb.changed();
        let cap = |ui: &egui::Ui, tag: &str, r: &egui::Rect, v: f64| {
            pp_capture(ui,
            &format!("{lbl} {tag} · FIELD  w={:.0} h={:.0} @({:.0},{:.0})  |  box \
pp_box fill=#141C25 border=#34414B radius=4  |  value='{:.2}' color={} font=12px-mono  |  arrow=none",
                r.width(), r.height(), r.left(), r.top(), v, pp_hex(PP_TEXT), lbl=label, tag=tag), *r)
        };
        cap(ui, "start", &a_rect, *a);
        cap(ui, "end", &b_rect, *b);
        resps.push(ra);
        resps.push(rb);
    });
    (changed, resps)
}

/// A label + one numeric field (Radius, Lt scale). Returns (changed, response).
fn pp_num_row(ui: &mut egui::Ui, label: &str, v: &mut f64) -> (bool, egui::Response) {
    let mut out = None;
    pp_row(ui, label, |ui, w| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, PP_ROW_H), egui::Sense::hover());
        out = Some(pp_num_field(ui, rect, v));
    });
    let r = out.expect("pp_row always invokes the body");
    (r.changed(), r)
}

/// Draw a flat bordered field box of `w × PP_ROW_H`. Returns its rect +
/// response (click sense when `clickable`); caller paints the contents.
fn pp_box(ui: &mut egui::Ui, w: f32, clickable: bool) -> (egui::Rect, egui::Response) {
    let sense = if clickable {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, PP_ROW_H), sense);
    // Draw with the UNCLIPPED ui painter: `painter_at(rect)` clips a boundary
    // stroke to its inner half, so the 1px border showed only faintly at the
    // rounded corners. surface-0 fill, 1px border, radius sm(4) — MENTOR §4.
    let edge = if clickable && resp.hovered() {
        PP_ACCENT
    } else {
        PP_BORDER
    };
    ui.painter().rect(
        rect,
        egui::Rounding::same(crate::theme::radius::SM),
        PP_BG_LO,
        egui::Stroke::new(1.0, edge),
    );
    (rect, resp)
}

/// Abbreviate a linetype name for the Inspector field (INSPECTOR_DESIGN_MENTOR
/// §5): first 10 letters of the base + the size-variant's first letter in parens.
/// Real names are like "Divide" / "Divide (small)" / "Center (tiny)", so
/// "Divide (small)" → "Divide (s)", "Dot" → "Dot". Full name is shown on hover.
fn abbrev_linetype(name: &str) -> String {
    if let Some(open) = name.find('(') {
        let base: String = name[..open].trim().chars().take(10).collect();
        let variant = name[open + 1..].trim_start();
        let first = variant.chars().next().unwrap_or(' ').to_ascii_lowercase();
        format!("{} ({})", base, first)
    } else {
        name.chars().take(10).collect()
    }
}

/// Shared preview length `L` for the linetype dash and the lineweight bar so
/// they stay matched at any field width (INSPECTOR_DESIGN_MENTOR §5): starts at
/// the input pad and stretches with the box, reserving a fixed trailing area
/// (`PP_PREVIEW_TRAIL`) for the abbreviated name / value + the dropdown arrow.
fn pp_preview_len(box_w: f32) -> f32 {
    (box_w - crate::theme::space::INPUT_PAD - 9.0 - PP_PREVIEW_TRAIL).max(24.0)
}

/// Record one element's rect into the layout-capture buffer (egui temp
/// data), but only while capture is armed. Free helpers and call sites
/// sprinkle this so the recorder can dump the whole menu's geometry.
fn pp_capture(ui: &egui::Ui, name: &str, rect: egui::Rect) {
    let on = ui.data(|d| {
        d.get_temp::<bool>(egui::Id::new("pp_cap_on"))
            .unwrap_or(false)
    });
    if !on {
        return;
    }
    ui.data_mut(|d| {
        let id = egui::Id::new("pp_cap_buf");
        let mut v: Vec<(String, egui::Rect)> = d.get_temp(id).unwrap_or_default();
        v.push((name.to_string(), rect));
        d.insert_temp(id, v);
    });
}

/// UI-inspect: draw a devtools-style spacing dimension across a gap `band`
/// between two captured boxes — a translucent amber fill, a centered measure
/// line with end ticks, and a px label chip. `vertical` = the gap is measured
/// top↕bottom (else left↔right). Amber so it reads distinct from the cyan box
/// outlines. Used to verify row/section spacing against the §5.1 tokens.
fn pp_gap_dim(painter: &egui::Painter, band: egui::Rect, vertical: bool, gap: f32) {
    let amber = egui::Color32::from_rgb(0xF2, 0xB5, 0x3D);
    painter.rect_filled(
        band,
        0.0,
        egui::Color32::from_rgba_unmultiplied(0xF2, 0xB5, 0x3D, 36),
    );
    let stroke = egui::Stroke::new(1.0, amber);
    if vertical {
        let x = band.center().x;
        painter.line_segment(
            [egui::pos2(x, band.top()), egui::pos2(x, band.bottom())],
            stroke,
        );
        for y in [band.top(), band.bottom()] {
            painter.line_segment([egui::pos2(x - 4.0, y), egui::pos2(x + 4.0, y)], stroke);
        }
    } else {
        let y = band.center().y;
        painter.line_segment(
            [egui::pos2(band.left(), y), egui::pos2(band.right(), y)],
            stroke,
        );
        for x in [band.left(), band.right()] {
            painter.line_segment([egui::pos2(x, y - 4.0), egui::pos2(x, y + 4.0)], stroke);
        }
    }
    // px label — integer when the gap is (near) whole, else one decimal.
    let label = if (gap - gap.round()).abs() < 0.05 {
        format!("{:.0}", gap)
    } else {
        format!("{:.1}", gap)
    };
    let galley =
        painter.layout_no_wrap(label, crate::theme::typ::data_code(), egui::Color32::BLACK);
    let c = band.center();
    let lp = egui::pos2(c.x - galley.size().x / 2.0, c.y - galley.size().y / 2.0);
    let chip = egui::Rect::from_min_size(
        lp - egui::vec2(4.0, 2.0),
        galley.size() + egui::vec2(8.0, 4.0),
    );
    painter.rect_filled(chip, 3.0, amber);
    painter.galley(lp, galley, egui::Color32::BLACK);
}

/// Auto-close a top category menu once the pointer has left BOTH the menu bar
/// (`bar_rect`) and this menu's popup. Call at the END of a top-level menu
/// closure. NOTE: only safe for menus WITHOUT submenus — a side-opening submenu
/// flyout sits outside `ui.min_rect()`, so this would wrongly close the parent
/// while the user is in the child. Submenu-bearing menus keep click-to-close.
fn menu_autoclose(ui: &mut egui::Ui, bar_rect: egui::Rect) {
    let pop = ui.min_rect().expand(6.0);
    match ui.input(|i| i.pointer.hover_pos()) {
        Some(p) if pop.contains(p) || bar_rect.contains(p) => {}
        _ => ui.close_menu(),
    }
}

/// Record the CONTAINER rect of a menu/popup body. Call ONCE at the end of
/// any menu closure (`ui.menu_button` body, `context_menu`, popup) — by then
/// `ui.min_rect()` bounds everything the body added, so this captures the
/// whole dropdown/popup box. One line makes any menu show up in the recorder.
/// Captured widgets inside (rail icons, etc.) record themselves via `pp_capture`.
fn pp_cap_ui(ui: &egui::Ui, name: &str) {
    pp_capture(ui, name, ui.min_rect());
}

/// A lightweight text-only "button" with a UNIFIED hover affordance: the label
/// brightens (muted → primary) and gets a faint rounded highlight, so plain
/// clickable text (rail footer + / reset, etc.) reads as pressable — matching
/// the header × treatment. Returns the click response.
fn text_button(ui: &mut egui::Ui, text: &str, size: f32) -> egui::Response {
    let galley = ui.fonts(|f| {
        f.layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(size),
            egui::Color32::PLACEHOLDER,
        )
    });
    let pad = egui::vec2(8.0, 3.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::click());
    let hov = resp.hovered();
    if hov {
        ui.painter().rect_filled(
            rect,
            crate::theme::radius::SM,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 14),
        );
    }
    let col = if hov {
        crate::theme::color::TEXT_PRIMARY
    } else {
        crate::theme::color::TEXT_MUTED
    };
    ui.painter().galley(rect.min + pad, galley, col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A 1px full-width divider line in the panel border colour.
fn pp_divider(ui: &mut egui::Ui) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(egui::vec2(w, 1.0), egui::Sense::hover());
    ui.painter().line_segment(
        [r.left_center(), r.right_center()],
        egui::Stroke::new(1.0, PP_BORDER),
    );
    pp_capture(ui, "divider", r);
}

/// A collapsible section: full-width clickable header with a painted triangle
/// (label colour, 30% larger than the title) + caps title. No body indent /
/// left vline. Open state persists per `id_src`.
fn pp_section(
    ui: &mut egui::Ui,
    id_src: &str,
    title: &str,
    default_open: bool,
    divider: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    // 1px hairline above every section header except the first (MENTOR §3).
    if divider {
        pp_divider(ui);
        ui.add_space(crate::theme::space::SM);
    }
    let id = ui.make_persistent_id(id_src);
    let mut open = ui
        .data_mut(|d| d.get_temp::<bool>(id))
        .unwrap_or(default_open);
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 18.0), egui::Sense::click());
    let p = ui.painter_at(rect);
    let c = egui::pos2(rect.left() + 7.0, rect.center().y);
    let sz = 4.5; // ~30% larger than egui's default header triangle
    let tri = if open {
        vec![
            egui::pos2(c.x - sz, c.y - sz * 0.55),
            egui::pos2(c.x + sz, c.y - sz * 0.55),
            egui::pos2(c.x, c.y + sz * 0.7),
        ]
    } else {
        vec![
            egui::pos2(c.x - sz * 0.55, c.y - sz),
            egui::pos2(c.x - sz * 0.55, c.y + sz),
            egui::pos2(c.x + sz * 0.7, c.y),
        ]
    };
    p.add(egui::Shape::convex_polygon(
        tri,
        PP_LABEL,
        egui::Stroke::NONE,
    ));
    // Caption 11/500 (THEME_SYSTEM §5.7), not the old monospace 10.
    p.text(
        egui::pos2(rect.left() + 20.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        crate::theme::typ::caption(),
        PP_LABEL,
    );
    pp_capture(
        ui,
        &format!(
            "SECTION '{title}' · HEADER  h=18  caption color={} font=11px  chevron={}",
            pp_hex(PP_LABEL),
            if open { "▼ open" } else { "▸ collapsed" },
            title = title
        ),
        rect,
    );
    if resp.clicked() {
        open = !open;
    }
    ui.data_mut(|d| d.insert_temp(id, open));
    // Section header → content = 12 (SECTION_GAP); between sections = 12 (GROUP_GAP).
    if open {
        ui.add_space(crate::theme::space::SECTION_GAP);
        body(ui);
    }
    ui.add_space(crate::theme::space::GROUP_GAP);
}

// ─────────────────────────────────────────────────────────────────────────────
// Text-dialog reusable widgets (ported from the upstream RUST-AutoRASM text
// dialog). All draw the Inspector field look — surface-0 fill, UNCLIPPED 1px
// border (`painter_at` clips it), radius SM, height PP_ROW_H — matching
// `hatch_num_box` / `hatch_spec_bar`.
// ─────────────────────────────────────────────────────────────────────────────

/// A dropdown box (no swatch): label at the left + the solid ▼ arrow. Greyed
/// (text-disabled, no click) when `enabled` is false.
fn pp_dropdown_box(ui: &mut egui::Ui, w: f32, text: &str, enabled: bool) -> egui::Response {
    use crate::theme::color as tc;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(w, PP_ROW_H),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    ui.painter().rect(
        rect,
        egui::Rounding::same(crate::theme::radius::SM),
        tc::SURFACE_0,
        egui::Stroke::new(1.0, tc::BORDER),
    );
    let p = ui.painter_at(rect);
    pp_arrow(&p, rect);
    let col = if enabled {
        tc::TEXT_PRIMARY
    } else {
        tc::TEXT_DISABLED
    };
    // Clip the label so a long name doesn't run under the arrow.
    let cl = p.with_clip_rect(egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right() - 16.0, rect.bottom()),
    ));
    cl.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        crate::theme::typ::body(),
        col,
    );
    if enabled && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A static (read-only) value box — surface-0 field with left-aligned text in
/// the Mono data font. Used for STUB fields (`Auto`); greyed when disabled.
fn pp_value_static(ui: &mut egui::Ui, w: f32, text: &str, enabled: bool) -> egui::Response {
    use crate::theme::color as tc;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, PP_ROW_H), egui::Sense::hover());
    ui.painter().rect(
        rect,
        egui::Rounding::same(crate::theme::radius::SM),
        tc::SURFACE_0,
        egui::Stroke::new(1.0, tc::BORDER),
    );
    let col = if enabled {
        tc::TEXT_PRIMARY
    } else {
        tc::TEXT_DISABLED
    };
    ui.painter_at(rect).text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        crate::theme::typ::data_value(),
        col,
    );
    resp
}

/// A pill button (New / Set current): rounded box, accent outline+text when
/// `accent`, else neutral. Returns whether it was clicked.
fn pp_pill(ui: &mut egui::Ui, w: f32, label: &str, accent: bool) -> bool {
    use crate::theme::color as tc;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, PP_ROW_H), egui::Sense::click());
    let hov = resp.hovered();
    let border = if accent { tc::ACCENT } else { tc::BORDER };
    let fill = if hov { tc::SURFACE_2 } else { tc::SURFACE_0 };
    ui.painter().rect(
        rect,
        egui::Rounding::same(crate::theme::radius::SM),
        fill,
        egui::Stroke::new(1.0, border),
    );
    let col = if accent { tc::ACCENT } else { tc::TEXT_PRIMARY };
    ui.painter_at(rect).text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        crate::theme::typ::body(),
        col,
    );
    if hov {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// A square glyph toggle/button: `sz`×`sz`, surface-3 backfill when `active`,
/// faint surface-2 on hover. The glyph is painted by `draw(painter, rect,
/// color)` where color = accent (active) / muted (idle) / disabled. Returns the
/// response (the caller decides what a click does).
fn pp_glyph_btn(
    ui: &mut egui::Ui,
    sz: f32,
    active: bool,
    enabled: bool,
    draw: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
) -> egui::Response {
    use crate::theme::color as tc;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(sz, sz),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let hov = resp.hovered() && enabled;
    if active {
        ui.painter()
            .rect_filled(rect, crate::theme::radius::SM, tc::SURFACE_3);
    } else if hov {
        ui.painter()
            .rect_filled(rect, crate::theme::radius::SM, tc::SURFACE_2);
    }
    let col = if !enabled {
        tc::TEXT_DISABLED
    } else if active {
        tc::ACCENT
    } else {
        tc::TEXT_MUTED
    };
    draw(&ui.painter_at(rect), rect, col);
    if hov {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A disabled (stub) checkbox row: empty box + label, greyed. For Spell Check /
/// Edit Dictionary until those features exist.
fn pp_check_stub(ui: &mut egui::Ui, label: &str) -> egui::Response {
    use crate::theme::color as tc;
    let galley = ui.fonts(|f| {
        f.layout_no_wrap(
            label.to_owned(),
            crate::theme::typ::body(),
            tc::TEXT_DISABLED,
        )
    });
    let bw = 15.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(bw + 8.0 + galley.size().x, 18.0),
        egui::Sense::hover(),
    );
    let box_r = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - bw * 0.5),
        egui::vec2(bw, bw),
    );
    ui.painter().rect(
        box_r,
        egui::Rounding::same(crate::theme::radius::SM),
        tc::SURFACE_0,
        egui::Stroke::new(1.0, tc::BORDER),
    );
    ui.painter().galley(
        egui::pos2(box_r.right() + 8.0, rect.center().y - galley.size().y * 0.5),
        galley,
        tc::TEXT_DISABLED,
    );
    resp.on_hover_text("Not in the kernel yet")
}

// ── Text-dialog parameter/tool glyphs (painter primitives; `c` = colour) ─────

/// Height — a large `A` with a small `A` (cap-height marker).
fn text_height_glyph(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let cy = r.center().y;
    p.text(
        egui::pos2(r.left() + 5.0, cy),
        egui::Align2::LEFT_CENTER,
        "A",
        egui::FontId::proportional(15.0),
        c,
    );
    p.text(
        egui::pos2(r.right() - 4.0, cy + 2.0),
        egui::Align2::RIGHT_CENTER,
        "A",
        egui::FontId::proportional(9.0),
        c,
    );
}

/// Line spacing — two stacked bars with a small vertical double-arrow.
fn text_linespacing_glyph(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let s = egui::Stroke::new(1.2, c);
    let x0 = r.left() + 8.0;
    let x1 = r.right() - 3.0;
    let cy = r.center().y;
    p.line_segment([egui::pos2(x0, cy - 5.0), egui::pos2(x1, cy - 5.0)], s);
    p.line_segment([egui::pos2(x0, cy + 5.0), egui::pos2(x1, cy + 5.0)], s);
    // vertical double-arrow on the left
    let ax = r.left() + 4.0;
    p.line_segment([egui::pos2(ax, cy - 5.0), egui::pos2(ax, cy + 5.0)], s);
    p.line_segment(
        [egui::pos2(ax, cy - 5.0), egui::pos2(ax - 2.0, cy - 2.5)],
        s,
    );
    p.line_segment(
        [egui::pos2(ax, cy - 5.0), egui::pos2(ax + 2.0, cy - 2.5)],
        s,
    );
    p.line_segment(
        [egui::pos2(ax, cy + 5.0), egui::pos2(ax - 2.0, cy + 2.5)],
        s,
    );
    p.line_segment(
        [egui::pos2(ax, cy + 5.0), egui::pos2(ax + 2.0, cy + 2.5)],
        s,
    );
}

/// Oblique — an upright `O` and a leaning slash, with a gap so the slash never
/// touches the O (owner note).
fn text_oblique_glyph(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let cy = r.center().y;
    p.text(
        egui::pos2(r.left() + 3.0, cy),
        egui::Align2::LEFT_CENTER,
        "O",
        egui::FontId::proportional(13.0),
        c,
    );
    let s = egui::Stroke::new(1.4, c);
    let x = r.right() - 4.0;
    p.line_segment(
        [egui::pos2(x - 2.5, cy + 5.0), egui::pos2(x + 1.5, cy - 5.0)],
        s,
    );
}

/// Tracking — `ab` over a horizontal double-arrow (letter spacing).
fn text_tracking_glyph(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let cx = r.center().x;
    let cy = r.center().y;
    p.text(
        egui::pos2(cx, cy - 4.0),
        egui::Align2::CENTER_CENTER,
        "ab",
        egui::FontId::proportional(9.0),
        c,
    );
    let s = egui::Stroke::new(1.1, c);
    let x0 = r.left() + 4.0;
    let x1 = r.right() - 4.0;
    let y = cy + 5.0;
    p.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], s);
    p.line_segment([egui::pos2(x0, y), egui::pos2(x0 + 2.5, y - 2.0)], s);
    p.line_segment([egui::pos2(x0, y), egui::pos2(x0 + 2.5, y + 2.0)], s);
    p.line_segment([egui::pos2(x1, y), egui::pos2(x1 - 2.5, y - 2.0)], s);
    p.line_segment([egui::pos2(x1, y), egui::pos2(x1 - 2.5, y + 2.0)], s);
}

/// List — three text lines with bullet dots (`bullet`) or numbered ticks.
fn glyph_list(p: &egui::Painter, r: egui::Rect, c: egui::Color32, bullet: bool) {
    let s = egui::Stroke::new(1.2, c);
    let mx = r.left() + 6.0;
    let x0 = r.left() + 11.0;
    let x1 = r.right() - 6.0;
    for (k, dy) in [(-6.0_f32), 0.0, 6.0].iter().enumerate() {
        let y = r.center().y + dy;
        if bullet {
            p.circle_filled(egui::pos2(mx, y), 1.3, c);
        } else {
            // small numeral tick
            p.line_segment([egui::pos2(mx, y - 2.0), egui::pos2(mx, y + 2.0)], s);
        }
        let _ = k;
        p.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], s);
    }
}

/// Alignment — four text lines ragged per `mode` (0=L, 1=C, 2=R, 3=justify).
fn glyph_align(p: &egui::Painter, r: egui::Rect, c: egui::Color32, mode: u8) {
    let s = egui::Stroke::new(1.2, c);
    let full_l = r.left() + 5.0;
    let full_r = r.right() - 5.0;
    let full = full_r - full_l;
    let widths = [full, full * 0.6, full * 0.85, full * 0.5];
    for (k, dy) in [(-6.0_f32), -2.0, 2.0, 6.0].iter().enumerate() {
        let y = r.center().y + dy;
        // last line of justify is short/left; others full
        let w = if mode == 3 {
            if k == 3 {
                full * 0.6
            } else {
                full
            }
        } else {
            widths[k]
        };
        let (x0, x1) = match mode {
            0 => (full_l, full_l + w),                             // left
            1 => (r.center().x - w * 0.5, r.center().x + w * 0.5), // center
            2 => (full_r - w, full_r),                             // right
            _ => (full_l, full_l + w),                             // justify
        };
        p.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], s);
    }
}

/// `abc` with an underline (stub toggle glyph).
fn glyph_abc(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let cx = r.center().x;
    let cy = r.center().y;
    p.text(
        egui::pos2(cx, cy - 2.0),
        egui::Align2::CENTER_CENTER,
        "abc",
        egui::FontId::proportional(11.0),
        c,
    );
    p.line_segment(
        [
            egui::pos2(cx - 8.0, cy + 6.0),
            egui::pos2(cx + 8.0, cy + 6.0),
        ],
        egui::Stroke::new(1.2, c),
    );
}

/// Find — a magnifier with a small `a` inside the lens.
fn glyph_find(p: &egui::Painter, r: egui::Rect, c: egui::Color32) {
    let lens = egui::pos2(r.center().x - 1.5, r.center().y - 1.5);
    let rad = 6.0;
    p.circle_stroke(lens, rad, egui::Stroke::new(1.3, c));
    p.text(
        lens,
        egui::Align2::CENTER_CENTER,
        "a",
        egui::FontId::proportional(8.0),
        c,
    );
    p.line_segment(
        [
            lens + egui::vec2(rad * 0.7, rad * 0.7),
            lens + egui::vec2(rad * 1.5, rad * 1.5),
        ],
        egui::Stroke::new(1.6, c),
    );
}

/// §4 A Layer / Color "bar" — surface-0 box (r4, border) with a color swatch
/// at the left, `label` text, and the category-style dropdown arrow (`pp_arrow`).
/// Returns the click response so the caller can open its picker.
fn hatch_spec_bar(ui: &mut egui::Ui, swatch: egui::Color32, label: &str) -> egui::Response {
    use crate::theme::color as tc;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), PP_ROW_H),
        egui::Sense::click(),
    );
    // Box (fill + border) with the UNCLIPPED ui painter — `painter_at(rect)`
    // clips a boundary stroke to its inner half, so the 1px border showed only
    // faintly (it looked "covered by the fill"). Matches the Inspector `pp_box`.
    ui.painter().rect(
        rect,
        egui::Rounding::same(crate::theme::radius::SM),
        tc::SURFACE_0,
        egui::Stroke::new(1.0, tc::BORDER),
    );
    let p = ui.painter_at(rect);
    let sw = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 5.0, rect.center().y - 7.0),
        egui::vec2(14.0, 14.0),
    );
    p.rect_filled(sw, 2.0, swatch);
    p.rect_stroke(sw, 2.0, egui::Stroke::new(1.0, tc::BORDER));
    let _arrow = pp_arrow(&p, rect); // §4 SAME arrow as the Specs-rail color box
    p.text(
        egui::pos2(sw.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        crate::theme::typ::body(),
        tc::TEXT_PRIMARY,
    );
    resp
}

/// An Inspector-style numeric value box (§6 field: surface-0 fill, 1px border,
/// radius 4, Mono 12) sized `w × 24`, editable (type or drag). Matches
/// `pp_num_field` but with a caller-set speed / range / suffix (the hatch Scale
/// wants finer speed + a clamp than the Inspector default).
fn hatch_num_box(
    ui: &mut egui::Ui,
    w: f32,
    v: &mut f64,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
    suffix: &str,
    calc: &crate::calc::CalcStore,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, PP_ROW_H), egui::Sense::hover());
    ui.painter().rect(
        rect,
        egui::Rounding::same(crate::theme::radius::SM),
        PP_BG_LO,
        egui::Stroke::new(1.0, PP_BORDER),
    );
    let pad = crate::theme::space::INPUT_PAD;
    let inner = egui::Rect::from_min_size(
        egui::pos2(rect.left() + pad, rect.center().y - 8.0),
        egui::vec2((rect.width() - 2.0 * pad).max(10.0), 16.0),
    );
    let sf = ui.style().override_font_id.clone();
    let sw = ui.visuals().widgets.clone();
    ui.style_mut().override_font_id = Some(crate::theme::typ::data_value());
    {
        let vis = ui.visuals_mut();
        for st in [
            &mut vis.widgets.inactive,
            &mut vis.widgets.hovered,
            &mut vis.widgets.active,
        ] {
            st.weak_bg_fill = egui::Color32::TRANSPARENT;
            st.bg_fill = egui::Color32::TRANSPARENT;
            st.bg_stroke = egui::Stroke::NONE;
            st.fg_stroke.color = PP_TEXT;
        }
    }
    let r = ui.put(
        inner,
        egui::DragValue::new(v)
            .speed(speed)
            .range(range)
            .max_decimals(2)
            .suffix(suffix)
            .update_while_editing(false)
            // Expressions commit in the field too (`2+3`, `x*2`).
            .custom_parser(move |s| crate::calc::parse_drag(calc, s)),
    );
    ui.style_mut().override_font_id = sf;
    ui.visuals_mut().widgets = sw;
    r
}

/// Where the user's customised ACI-wheel permutation lives. Sits next to
/// the other project-root files (Audit.html, Variables.md, etc.) so the
/// arrangement travels with the codebase, not a per-user config dir.
fn aci_mapping_path() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR resolves to `<repo>/cad_app/` at compile time;
    // step one level up to reach the workspace root where the other
    // top-level project artefacts live.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(|p| p.join("aci_mapping.json"))
        .unwrap_or_else(|| std::path::PathBuf::from("aci_mapping.json"))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tool {
    None,
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
    Dim,
    Rectangle,
}

/// Sub-mode for the polyline draw tool — mirrors AutoCAD PLINE's Line /
/// Arc toggle. `a` (or `arc`) switches Line→Arc; `l` (or `line`)
/// switches Arc→Line. Independent from `Tool::Arc` (a separate draw tool).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PlineMode {
    Line,
    Arc,
}

/// Per-click flow modifier inside PLINE Arc mode. Default `Normal` =
/// tangent-continuous arc by endpoint click. `SecondPt` paths take TWO
/// clicks (the on-arc midpoint then the endpoint) and build a 3-point
/// arc instead. Resets to `Normal` after each arc commits, or on
/// mode switch / Esc.
#[derive(Clone, Copy, PartialEq, Debug)]
enum PlineArcSub {
    Normal,
    AwaitingSecondPt,          // user typed `s`; next click = on-arc midpoint
    AwaitingSecondPtEnd(Vec2), // midpoint captured; next click = endpoint
    AwaitingDirection,         // user typed `d`; next click/angle = start tangent
}

/// PLINE Width capture flow (AutoCAD `W`/`H`): after typing `w`, the user
/// enters a starting width then an ending width (Enter on the second repeats
/// the first). `H`/halfwidth doubles the entered values. The captured
/// (start, end) becomes `pline_next_width`, applied to subsequent segments.
#[derive(Clone, Copy, PartialEq, Debug)]
enum PlineWidthCap {
    None,
    AwaitingStart { half: bool },
    AwaitingEnd { half: bool, start: f64 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ArcMethod {
    ThreePoints,
    StartCenterEnd,
    StartCenterAngle,
    StartCenterLength,
    StartEndAngle,
    StartEndDirection,
    StartEndRadius,
    CenterStartEnd,
    CenterStartAngle,
    CenterStartLength,
    Continue,
}

impl ArcMethod {
    /// Compact code shown on the rail flyout + icon (e.g. "SCE").
    fn short(&self) -> &'static str {
        match self {
            ArcMethod::ThreePoints => "3P",
            ArcMethod::StartCenterEnd => "SCE",
            ArcMethod::StartCenterAngle => "SCA",
            ArcMethod::StartCenterLength => "SCL",
            ArcMethod::StartEndAngle => "SEA",
            ArcMethod::StartEndDirection => "SED",
            ArcMethod::StartEndRadius => "SER",
            ArcMethod::CenterStartEnd => "CSE",
            ArcMethod::CenterStartAngle => "CSA",
            ArcMethod::CenterStartLength => "CSL",
            ArcMethod::Continue => "CON",
        }
    }
    fn name(&self) -> &'static str {
        match self {
            ArcMethod::ThreePoints => "3-Point",
            ArcMethod::StartCenterEnd => "Start, Center, End",
            ArcMethod::StartCenterAngle => "Start, Center, Angle",
            ArcMethod::StartCenterLength => "Start, Center, Length",
            ArcMethod::StartEndAngle => "Start, End, Angle",
            ArcMethod::StartEndDirection => "Start, End, Direction",
            ArcMethod::StartEndRadius => "Start, End, Radius",
            ArcMethod::CenterStartEnd => "Center, Start, End",
            ArcMethod::CenterStartAngle => "Center, Start, Angle",
            ArcMethod::CenterStartLength => "Center, Start, Length",
            ArcMethod::Continue => "Continue",
        }
    }
    /// Only purely-click-driven methods are wired now. Methods that need a
    /// numeric input (angle / length / radius) or that need previous-dobject
    /// tracking (Continue) are listed-but-frozen until that infra exists.
    fn enabled(&self) -> bool {
        matches!(
            self,
            ArcMethod::ThreePoints | ArcMethod::StartCenterEnd | ArcMethod::CenterStartEnd
        )
    }
    fn click_count(&self) -> usize {
        3
    }
    fn hint(&self, n: usize) -> &'static str {
        match (self, n) {
            (ArcMethod::ThreePoints, 0) => "arc 3p: click first point on arc",
            (ArcMethod::ThreePoints, 1) => "arc 3p: click second point",
            (ArcMethod::ThreePoints, _) => "arc 3p: click third point    [Esc cancels]",
            (ArcMethod::StartCenterEnd, 0) => "arc S,C,E: click START",
            (ArcMethod::StartCenterEnd, 1) => "arc S,C,E: click CENTER",
            (ArcMethod::StartCenterEnd, _) => "arc S,C,E: click END    [Esc cancels]",
            (ArcMethod::CenterStartEnd, 0) => "arc C,S,E: click CENTER",
            (ArcMethod::CenterStartEnd, 1) => "arc C,S,E: click START",
            (ArcMethod::CenterStartEnd, _) => "arc C,S,E: click END (CCW)    [Esc cancels]",
            _ => "(frozen method — pick another from the arc menu)",
        }
    }
}

const ALL_ARC_METHODS: &[ArcMethod] = &[
    ArcMethod::ThreePoints,
    ArcMethod::StartCenterEnd,
    ArcMethod::StartCenterAngle,
    ArcMethod::StartCenterLength,
    ArcMethod::StartEndAngle,
    ArcMethod::StartEndDirection,
    ArcMethod::StartEndRadius,
    ArcMethod::CenterStartEnd,
    ArcMethod::CenterStartAngle,
    ArcMethod::CenterStartLength,
    ArcMethod::Continue,
];

/// Build an axis-aligned rectangle (CLOSED 4-vertex Polyline) from two
/// opposite corners. Vertices are emitted CCW-ish in corner order; `closed`
/// makes the 4th→1st edge implicit. Mirrors AutoCAD RECTANG (one LWPOLYLINE).
/// A file operation running on a BACKGROUND worker thread, shown as a modal loading/saving
/// overlay. The UI thread never blocks on the read/parse/serialize/write — it paints the
/// overlay once (`painted`), then spawns the worker (`rx` becomes `Some`) and polls it each
/// frame. Because the UI thread keeps returning, Windows can never mark the window
/// "(Not Responding)" no matter how large the project is.
struct BusyOp {
    kind: BusyKind,
    path: String,
    subtitle: String,
    started: std::time::Instant,
    est_ms: u64,
    /// The overlay has been rendered at least once — safe to spawn the worker next frame.
    painted: bool,
    /// Worker result channel. `None` until the worker is spawned (one frame after `painted`).
    rx: Option<std::sync::mpsc::Receiver<BusyMsg>>,
}

#[derive(Clone, Copy, PartialEq)]
enum BusyKind {
    Load,
    Save,
}

/// What a background file worker sends back when it finishes.
enum BusyMsg {
    Loaded(Result<Box<LoadPayload>, String>),
    Saved(Result<SavePayload, String>),
}

/// Everything a load worker parsed OFF the UI thread. Installing it into the app
/// (`apply_loaded`) is fast and happens back on the main thread.
struct LoadPayload {
    doc: Document,
    /// The parsed SIMLUX sidecar, if one exists beside the drawing. Its `factory.furniture_lib`
    /// has been drained — the decoded meshes are in `furniture` below.
    sidecar: Option<crate::simlux_io::SimluxConfig>,
    /// Furniture meshes DECODED on the worker (blob → verts + AABB) — the multi-second part of a
    /// load, kept off the UI thread. Empty when there is no sidecar.
    furniture: Vec<crate::factory::FurnitureAsset>,
    /// The parsed SIMLUX config EMBEDDED inside the drawing (RSM v201 section /
    /// DXF XRECORD), when the file carries one. Same drained-`furniture_lib`
    /// convention as `sidecar`: decoded meshes land in `embedded_furniture`.
    embedded: Option<crate::simlux_io::SimluxConfig>,
    embedded_furniture: Vec<crate::factory::FurnitureAsset>,
    /// Textures DECODED on the worker (PNG → RGBA), in record order, so the UI
    /// thread never blocks on dozens of PNG decodes at install. One list per
    /// source, aligned with `sidecar` / `embedded`.
    sidecar_textures: Vec<crate::factory::TextureAsset>,
    embedded_textures: Vec<crate::factory::TextureAsset>,
    /// The saved light results embedded in the drawing, if the file carried them.
    embedded_results: Option<crate::light_store::StoredResults>,
    /// Per-stage timings (ms) captured in the worker, re-emitted to the recorder on the main
    /// thread: `(read, parse, sidecar-read+parse, furniture-decode)`.
    read_ms: u64,
    parse_ms: u64,
    sidecar_ms: u64,
    furn_ms: u64,
    /// How the DWG was read -- natively or via the converter -- and what it dropped. `None` for
    /// anything that is not a DWG. Carried rather than logged on the worker, because the worker
    /// has no history panel to log to.
    dwg_note: Option<String>,
}

/// A drawing that carried BOTH an embedded 3D project AND a `.simlux.json` beside
/// it — which is the file's extra data? Shown once, right after the open; the
/// user's answer also decides the storage mode of the next save (following the
/// file's mode, per the Save-As dialog).
struct PendingExtraChoice {
    path: String,
    /// The sidecar-file source (config + its decoded furniture).
    sidecar_cfg: Option<crate::simlux_io::SimluxConfig>,
    sidecar_furn: Vec<crate::factory::FurnitureAsset>,
    /// The embedded-in-file source.
    embedded_cfg: Option<crate::simlux_io::SimluxConfig>,
    embedded_furn: Vec<crate::factory::FurnitureAsset>,
    /// Decoded textures for each source (worker-side PNG decode; aligned with
    /// the config's record order).
    sidecar_tex: Vec<crate::factory::TextureAsset>,
    embedded_tex: Vec<crate::factory::TextureAsset>,
    /// Embedded saved light results — restored only if the embedded side wins,
    /// else the sidecar result file is tried (the historic behaviour).
    embedded_results: Option<crate::light_store::StoredResults>,
}

/// The answer to the "which copy of the 3D project data?" dialog — and the
/// storage mode of every later save follows it (a file loaded from its embedded
/// payload keeps saving embedded; one loaded from its sidecar keeps using the
/// sidecar). See [`PendingExtraChoice`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExtraPick {
    Embedded,
    Sidecar,
    Neither,
}

/// Result of a save worker: bytes written + a note for the history log.
struct SavePayload {
    bytes: usize,
    note: String,
    /// SET WHEN THE DRAWING WAS WRITTEN AND THE SIMLUX HALF WAS NOT.
    ///
    /// A half-save was already REPORTED -- the note opens "SIMLUX DID NOT SAVE" -- but it was only
    /// ever a line in the command history, and `apply_saved` went on to clear `unsaved` as if the
    /// whole project had reached disk. So the app believed it was saved, the close guard never
    /// fired, and the only copy of the 3D model, the furniture, the fittings and the results sat in
    /// a `.savetmp` nobody knew to look for. Found on the owner's project two weeks after the fact:
    /// the live sidecar was from the 10th, the temp beside it from the 24th.
    ///
    /// Carried as DATA rather than parsed back out of `note`, so the state and the message cannot
    /// disagree about whether the save worked.
    simlux_failed: Option<String>,
}

/// The file's stem (name without directory or extension), for the overlay subtitle.
fn file_stem_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Resolve a BUNDLED asset path (e.g. `assets/architecture/normal_stairs.fbx`) to a real file,
/// searching the current directory AND locations relative to the executable — so a bundled model
/// loads whether the app was launched from the repo root (`cargo run`, cwd = root) or by
/// double-clicking `target/debug/simlux.exe` (cwd elsewhere; the assets sit two levels up from the
/// exe). Returns the first existing candidate, or `None` if the asset genuinely isn't on disk.
#[allow(dead_code)] // kept for future bundled-asset loads (e.g. retrofitting aperture paths)
fn resolve_asset_path(rel: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut tries: Vec<PathBuf> = vec![PathBuf::from(rel)];
    if let Ok(exe) = std::env::current_exe() {
        // Walk up a few levels from the exe dir (…/target/debug/, …/target/release/, or a
        // packaged bin dir) looking for `<ancestor>/<rel>`.
        let mut dir = exe.parent();
        for _ in 0..4 {
            let Some(d) = dir else { break };
            tries.push(d.join(rel));
            dir = d.parent();
        }
    }
    tries.into_iter().find(|p| p.exists())
}

/// Read + parse a drawing (and its SIMLUX sidecar) entirely OFF the UI thread. A pure function
/// of the path, so it runs on a worker; `CadApp::apply_loaded` installs the result on the main
/// thread. This is what makes a 22-second load stop freezing the window — all the reading and
/// `serde` parsing happens here while the UI keeps painting the "Loading…" overlay.
fn load_file_worker(path: &str) -> Result<Box<LoadPayload>, String> {
    let lower = path.to_ascii_lowercase();
    let t = std::time::Instant::now();
    // What the DWG path did, for the open log -- native or converted, and what it dropped.
    let mut dwg_note: Option<String> = None;
    // ---- read + parse the 2D document (DWG is read natively, or converted) ------------
    let (doc, read_ms, parse_ms) = if lower.ends_with(".dxf") {
        let text = std::fs::read_to_string(path).map_err(|e| format!("read '{path}': {e}"))?;
        let read_ms = t.elapsed().as_millis() as u64;
        let t2 = std::time::Instant::now();
        let doc = cad_io::dxf::read_dxf(&text).map_err(|e| format!("parse dxf: {e}"))?;
        (doc, read_ms, t2.elapsed().as_millis() as u64)
    } else if lower.ends_with(".rsm") {
        let bytes = std::fs::read(path).map_err(|e| format!("read '{path}': {e}"))?;
        let read_ms = t.elapsed().as_millis() as u64;
        let t2 = std::time::Instant::now();
        let doc = cad_io::rsm::read_rsm(&bytes).map_err(|e| format!("parse rsm: {e}"))?;
        (doc, read_ms, t2.elapsed().as_millis() as u64)
    } else if lower.ends_with(".dwg") {
        // NATIVELY FIRST, AND ON MOST MACHINES THAT IS THE WHOLE STORY. `cad_io::dwg` is compiled
        // in, so a DWG opens with nothing installed — which is what "cant our installation file
        // have just the required files so dwg can open without any hassel" was asking for, and the
        // answer needs no extra files at all.
        //
        // THE CONVERTER STAYS AS THE FALLBACK. The native reader is new and DWG has thirty years of
        // versions behind it; where it cannot read a file AutoCAD still can, and a user who has
        // AutoCAD must not lose that because we added something. It costs nothing when unused.
        let t2 = std::time::Instant::now();
        match cad_io::dwg::read_dwg(std::path::Path::new(path)) {
            Ok((doc, tally)) => {
                // COUNTED, NOT INFERRED. "2336 entities" and "2336 kept" read identically in a log
                // and mean very different things; a drawing that quietly lost its dimensions
                // should say so on the way in.
                if tally.skipped > 0 {
                    dwg_note = Some(format!(
                        "  dwg: read natively — {} entities, {} not yet supported (text, \
                         dimensions, hatches and blocks are skipped rather than approximated)",
                        tally.kept, tally.skipped,
                    ));
                } else {
                    dwg_note = Some(format!("  dwg: read natively — {} entities", tally.kept));
                }
                (
                    doc,
                    t.elapsed().as_millis() as u64,
                    t2.elapsed().as_millis() as u64,
                )
            }
            Err(native) => {
                let conv = dwg_converter().ok_or_else(|| {
                    format!(
                        "could not read '{path}'.\n    natively: {native}\n    and no DWG \
                         converter is installed to fall back on — AutoCAD provides one, or set \
                         RUSTCAD_DWGCONV to another as \"cmd {{in}} {{out}}\"."
                    )
                })?;
                let out = std::env::temp_dir().join("rustcad_dwg_open.dxf");
                run_dwg_conversion(&conv, path, &out)?;
                let text = std::fs::read_to_string(&out)
                    .map_err(|e| format!("read converted dxf: {e}"))?;
                let read_ms = t.elapsed().as_millis() as u64;
                let t3 = std::time::Instant::now();
                let doc = cad_io::dxf::read_dxf(&text).map_err(|e| format!("parse dxf: {e}"))?;
                dwg_note = Some(format!(
                    "  dwg: the native reader could not open this one ({native}) — converted via \
                     the external converter instead"
                ));
                (doc, read_ms, t3.elapsed().as_millis() as u64)
            }
        }
    } else {
        return Err(format!(
            "unknown extension on '{path}': expected .dxf, .dwg or .rsm"
        ));
    };
    // ---- read + parse the SIMLUX config: EMBEDDED in the file and/or sidecar ------
    // Two places the 3D project can live. A drawing saved with "inside the file"
    // carries its payload as RSM extra-blobs / DXF XRECORDs (taken out of the doc
    // below, so the loaded document goes to editing with none); one saved with
    // separate files has `<drawing>.simlux.json` beside it. Both are parsed here
    // off the UI thread — historically the slow, freezing stage — and when both
    // exist the MAIN thread asks which one to load (see `apply_loaded`).
    //
    // ONLY the two formats this app writes itself can carry an embedded project.
    // A `.dwg` never gets one on save, and one that shows up anyway (a converter's
    // DXF may preserve the dictionary from an embedded source file) is dropped —
    // DWG is always sidecar, and an embedded mode on a file that cannot embed
    // would strand every later calculation result. Everything else under the
    // blob keys or with a foreign name is dropped too: blobs exist only between a
    // load and the app re-embedding its own data at the next save.
    let t3 = std::time::Instant::now();
    let mut doc = doc;
    let embeddable_format = lower.ends_with(".dxf") || lower.ends_with(".rsm");
    // RSM-only: the native geometry blob (see `furniture_geom_native`). DXF has
    // no such blob — its geometry lives inside the config JSON as base64.
    let native_geom = if lower.ends_with(".rsm") {
        doc.take_extra_blob(crate::simlux_io::GEOM_BLOB)
    } else {
        None
    };
    let embedded_cfg_bytes = if embeddable_format {
        doc.take_extra_blob(crate::simlux_io::CFG_BLOB)
    } else {
        None
    };
    let embedded_results = if embeddable_format {
        doc.take_extra_blob(crate::simlux_io::RESULTS_BLOB)
            .and_then(|b| crate::light_store::from_embed_bytes(&b))
    } else {
        None
    };
    doc.extra_blobs.clear();
    let mut sidecar =
        crate::simlux_io::load(std::path::Path::new(path)).map_err(|e| format!("sidecar: {e}"))?;
    let mut embedded = match &embedded_cfg_bytes {
        Some(b) => Some(
            crate::simlux_io::cfg_from_embed_bytes(b)
                .map_err(|e| format!("embedded extra data: {e}"))?,
        ),
        None => None,
    };
    let sidecar_ms = t3.elapsed().as_millis() as u64;
    // ---- decode furniture geometry HERE, off the UI thread ----------------------------
    // A 2M-triangle asset is ~74 MB of floats; decoding it (base64+inflate) + computing its
    // AABB took ~8 s on the main thread and froze the window. Draining it into `furniture`
    // means the main-thread install just moves ready meshes in.
    let t4 = std::time::Instant::now();
    let furniture = match sidecar.as_mut() {
        Some(cfg) => crate::factory::FactoryState::decode_furniture_lib(std::mem::take(
            &mut cfg.factory.furniture_lib,
        )),
        None => Vec::new(),
    };
    // PNG → RGBA on the WORKER too (dozens of textures on the gym plan). The
    // install would otherwise decode every one of them on the UI thread.
    let sidecar_textures = match sidecar.as_ref() {
        Some(cfg) => crate::factory::decode_texture_list(&cfg.factory.textures),
        None => Vec::new(),
    };
    let embedded_furniture = match embedded.as_mut() {
        // An RSM embedded save stores the geometry as raw deflated bytes in its
        // own blob — decode that (no base64). A file WITHOUT the blob (an older
        // embed, or the blob damaged away) falls back to the JSON base64 fields,
        // which is where DXF embeds always carry it.
        Some(cfg) if native_geom.is_some() => {
            crate::factory::FactoryState::decode_furniture_lib_native(
                std::mem::take(&mut cfg.factory.furniture_lib),
                native_geom.as_deref().unwrap_or_default(),
            )
        }
        Some(cfg) => crate::factory::FactoryState::decode_furniture_lib(std::mem::take(
            &mut cfg.factory.furniture_lib,
        )),
        None => Vec::new(),
    };
    let embedded_textures = match embedded.as_ref() {
        Some(cfg) => crate::factory::decode_texture_list(&cfg.factory.textures),
        None => Vec::new(),
    };
    let furn_ms = t4.elapsed().as_millis() as u64;
    Ok(Box::new(LoadPayload {
        doc,
        sidecar,
        furniture,
        embedded,
        embedded_furniture,
        sidecar_textures,
        embedded_textures,
        embedded_results,
        read_ms,
        parse_ms,
        sidecar_ms,
        furn_ms,
        dwg_note,
    }))
}

/// Serialize + write a drawing and its SIMLUX sidecar entirely OFF the UI thread. The caller
/// (`spawn_busy_worker`) has already cloned the doc and built `cfg` on the main thread, so this
/// worker only does the heavy `serde`/IO — keeping the "Saving…" overlay animating smoothly.
/// Write `bytes` to `path` atomically: to a sibling temp file, then rename over the target.
/// `std::fs::rename` replaces the destination on Windows and POSIX, so a reader never sees a
/// half-written file and an interrupted write leaves the previous file untouched.
fn atomic_write(path: &str, bytes: &[u8]) -> std::io::Result<()> {
    // The rename is RETRIED, and the temp is KEPT if it still cannot be done — see
    // `simlux_io::replace_file`. This used to `remove_file(&tmp)` on failure, throwing away the
    // only copy of what had just been written.
    //
    // The temp name is UNIQUE PER WRITE, exactly like the sidecar writer's: three background
    // save workers (modal, autosave, after-calculation embed) can now overlap on one path, and
    // a shared `{path}.savetmp` would let one writer truncate another's temp mid-write and
    // publish a torn file at the rename.
    let tmp = format!(
        "{path}.savetmp.{}",
        TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    std::fs::write(&tmp, bytes)?;
    crate::simlux_io::replace_file(std::path::Path::new(&tmp), std::path::Path::new(path))
}

/// Sequence for unique `.savetmp` names (see [`atomic_write`]).
static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Normalize a document for serialization. At RUNTIME the layer tables live
/// in the SWAPPED arrangement while a layout tab is active (doc.layers = that
/// layout's paper table, the model table sits inside `layouts[i].layers`).
/// Files always store doc.layers = MODEL table + each layout's OWN paper table,
/// so swap back and clear `active_layout` before any writer sees the doc —
/// otherwise saving from a layout tab silently persists the swapped pair
/// (paper table as the model table) and the file corrupts or refuses to load.
fn normalize_layers_for_save(doc: &mut Document) {
    if let Some(li) = doc.active_layout {
        if let Some(layout) = doc.layouts.get_mut(li) {
            std::mem::swap(&mut doc.layers, &mut layout.layers);
        }
        doc.active_layout = None;
    }
}

fn save_file_worker(
    path: &str,
    doc: Document,
    mut cfg: crate::simlux_io::SimluxConfig,
    furn_geom: Vec<crate::factory::FurnitureGeomRaw>,
    store: crate::simlux_io::ExtraDataStore,
    results: Option<crate::light_store::StoredResults>,
) -> Result<SavePayload, String> {
    let lower = path.to_ascii_lowercase();
    // EMBED or SIDECAR? "Inside the file" is only possible for the two formats this
    // app writes itself — RSM (its native v201 section) and DXF (its SIMLUX_DATA
    // XRECORDs). A DWG goes out through AutoCAD's converter and is read back with a
    // model-space-only parser, so its extra data always lives in the sidecar files.
    let embed = crate::simlux_io::can_embed(store, std::path::Path::new(path));
    // An RSM payload is BINARY end to end, so the furniture geometry skips the
    // base64 layer entirely: the config JSON stays geometry-less and the meshes
    // ride as raw deflated bytes in their own blob (`simlux-geom`). DXF is text —
    // its XRECORDs cannot hold raw bytes — so it keeps the base64-in-JSON form.
    let native_geom = embed && lower.ends_with(".rsm");
    // Compress furniture geometry HERE (deflate of tens of MB) — the expensive part of a save,
    // kept off the UI thread. `cfg` came from `build_simlux_config_lite` with empty blobs, in
    // the same order as `furn_geom`. The native-RSM path does not run this loop: its geometry
    // goes to `furniture_geom_native` directly, and the JSON must stay free of the base64 text
    // (which is exactly what opening no longer has to decode).
    if !native_geom {
        for (rec, g) in cfg.factory.furniture_lib.iter_mut().zip(furn_geom.iter()) {
            rec.pos_b64 = crate::factory::encode_f32_blob(&g.pos);
            rec.nrm_b64 = crate::factory::encode_f32_blob(&g.nrm);
            rec.uv_b64 = if g.uv.is_empty() {
                String::new()
            } else {
                crate::factory::encode_f32_blob(&g.uv)
            };
            rec.alpha_b64 = if g.alpha.is_empty() {
                String::new()
            } else {
                crate::factory::encode_f32_blob(&g.alpha)
            };
        }
    }
    // CRITICAL: never serialize the swapped (active-tab) table arrangement —
    // the RSM/DXF writers expect doc.layers = model table.
    let mut doc = doc;
    normalize_layers_for_save(&mut doc);
    if embed {
        if native_geom {
            let geom_bytes = crate::factory::furniture_geom_native(&furn_geom);
            doc.set_extra_blob(crate::simlux_io::GEOM_BLOB, geom_bytes);
        }
        // Attach the payload to the document BEFORE the writer runs, so the bytes
        // land inside the drawing. Compact serialization — this copy is written on
        // every save and is not meant for human reading (see `cfg_to_embed_bytes`).
        let cfg_bytes = crate::simlux_io::cfg_to_embed_bytes(&cfg)
            .map_err(|e| format!("serialize extra data: {e}"))?;
        doc.set_extra_blob(crate::simlux_io::CFG_BLOB, cfg_bytes);
        if let Some(r) = results {
            match crate::light_store::to_embed_bytes(&r) {
                Ok(b) => doc.set_extra_blob(crate::simlux_io::RESULTS_BLOB, b),
                Err(e) => return Err(format!("serialize the saved calculation: {e}")),
            }
        }
    }
    let bytes: Vec<u8> = if lower.ends_with(".dxf") || lower.ends_with(".dwg") {
        // A DWG GOES OUT AS DXF FIRST. Nothing here writes DWG — it is closed, versioned and
        // undocumented — so the drawing is written in the format this app owns and handed to the
        // same converter that opens a DWG, running the other way. See `save_as_dwg`.
        cad_io::dxf::write_dxf(&doc).into_bytes()
    } else if lower.ends_with(".rsm") {
        cad_io::rsm::write_rsm(&doc)
    } else {
        return Err("unknown extension (expected .dxf, .dwg or .rsm)".to_string());
    };
    // Write ATOMICALLY (temp file + rename) so an interrupted write — e.g. a crash or a close
    // during an autosave — can never truncate the real file; the rename either happens whole or
    // not at all, leaving the previous good file intact.
    //
    // A DWG is written to a TEMP DXF and converted ONTO the real path, so a conversion that fails
    // leaves the previous `.dwg` untouched rather than replacing it with a DXF wearing its name —
    // which would open in nothing and read as a corrupt file.
    if lower.ends_with(".dwg") {
        let stem = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "drawing".into());
        let tmp = std::env::temp_dir().join(format!("simlux_save_{stem}.dxf"));
        atomic_write(&tmp.to_string_lossy(), &bytes)
            .map_err(|e| format!("write '{}': {e}", tmp.display()))?;
        let r = save_as_dwg(&tmp, std::path::Path::new(path));
        let _ = std::fs::remove_file(&tmp);
        r?;
    } else {
        atomic_write(path, &bytes).map_err(|e| format!("write '{path}': {e}"))?;
    }
    // The extra data is then stored the way the user chose: embedded in the file just
    // written (and any now-stale sidecar files removed, so a later open cannot find
    // two copies and ask which to load), or written as the sidecar it always was.
    // Either way a failure is reported but does not fail the drawing save.
    let solids = cfg.factory.model.features.len();
    let walls = cfg.factory.walls.len();
    // WHAT WENT INTO THE FILE, COUNTED — including the imports, which are the expensive half of a
    // project and the half that says nothing at all when it goes missing.
    //
    // Reported as "its not saving it properly ... when i load it, it loads an older version". The
    // sidecar on disk held four features and `furniture_lib: []`, `furniture: []`, `textures: []`
    // — the model saved and everything imported into it did not, which reopens looking exactly
    // like an older version of the same file. Every step of the save chain is tested and keeps
    // them, so a file that still comes out empty was empty BEFORE the save; this line says so at
    // the moment it happens rather than a week later.
    let furn = cfg.factory.furniture_lib.len();
    let insts = cfg.factory.furniture.len();
    let texs = cfg.factory.textures.len();
    let imports = format!(", {furn} asset(s)/{insts} placed, {texs} texture(s)");
    let mut simlux_failed = None;
    let note = if embed {
        // The stale sidecar pair (project + results) is deleted — the file now IS the
        // project, and leaving the older copies beside it would make every open ask
        // which one to load. Best-effort: a file that cannot be removed is left (the
        // open-time dialog still offers both) and the leftover is NAMED, not silent.
        let mut leftover = None;
        let dwg_path = std::path::Path::new(path);
        for stale in [
            crate::simlux_io::sidecar_path(dwg_path),
            crate::light_store::result_path(dwg_path),
        ] {
            if stale.exists() {
                if let Err(e) = std::fs::remove_file(&stale) {
                    leftover = Some(format!("old '{}' left in place: {}", stale.display(), e));
                }
            }
        }
        let note = if solids > 0 {
            format!(
                "  saved '{}'  ({} bytes) · SIMLUX embedded in file: {} solid(s), {} wall(s){imports}",
                path, bytes.len(), solids, walls)
        } else {
            format!(
                "  saved '{}'  ({} bytes) · SIMLUX embedded in file{imports}",
                path,
                bytes.len()
            )
        };
        match leftover {
            Some(l) => format!("{note}\n     ⚠ {l}"),
            None => note,
        }
    } else {
        // THE SIDECAR — written exactly as this worker always wrote it.
        match crate::simlux_io::save(std::path::Path::new(path), &cfg) {
            Ok(_) if solids > 0 => format!(
                "  saved '{}'  ({} bytes) · SIMLUX + {} solid(s), {} wall(s){imports}",
                path,
                bytes.len(),
                solids,
                walls
            ),
            Ok(_) => format!(
                "  saved '{}'  ({} bytes) · SIMLUX{imports}",
                path,
                bytes.len()
            ),
            // NOT "saved". The drawing was written and the SIMLUX half — the 3D model, the furniture,
            // the fittings, the results — was NOT, and a line that opens with the word "saved" is read
            // as success and closed on. Reported as: made a calculation, saved, reopened, "nothing was
            // saved" — because the only thing saying otherwise was a clause at the end of a line that
            // began by claiming the opposite.
            Err(e) => {
                // AND AS DATA, not only as prose. `apply_saved` has to know the project did not reach
                // disk, and parsing that fact back out of a sentence is how a message and a state come
                // to disagree about it.
                simlux_failed = Some(e.clone());
                format!(
                    "  ⚠ SIMLUX DID NOT SAVE — the drawing '{}' was written ({} bytes) but the 3D \
                     model, furniture, fittings and results were NOT.\n     {}",
                    path,
                    bytes.len(),
                    e,
                )
            }
        }
    };
    Ok(SavePayload {
        bytes: bytes.len(),
        note,
        simlux_failed,
    })
}

/// Turn a written DXF into the DWG the user actually asked for.
///
/// Reported as: *"why cant i save as dwg?"* — and *"and fix the file saving"*. The save path took
/// `.dxf` and `.rsm` and said so, which answers the question without solving it: a practice's
/// filing, its consultants and its clients ask for `.dwg`.
///
/// THE ERROR IS THE POINT OF THIS FUNCTION. Every failure here is one somebody has to act on — no
/// AutoCAD on this machine, a converter that could not be run, a conversion that produced nothing —
/// and each has a different thing to do about it. "Save failed" would send them looking at the
/// drawing, which is fine.
fn save_as_dwg(dxf: &std::path::Path, dwg: &std::path::Path) -> Result<(), String> {
    let conv = dxf_to_dwg_converter().ok_or_else(|| {
        "no DXF→DWG converter found. DWG is a closed format, so SIMLUX writes a DXF and asks \
         AutoCAD's headless core (accoreconsole) to save it on — the same tool that opens a DWG. \
         Install AutoCAD, or set RUSTCAD_DXF2DWG to \"yourconverter {in} {out}\". Saving as .dxf \
         needs none of this and loses nothing."
            .to_string()
    })?;
    convert_dxf_to_dwg(&conv, dxf, dwg)
}

/// Run one named converter.
///
/// SPLIT FROM [`save_as_dwg`] so the conversion can be exercised with a stub. A test that had to
/// find AutoCAD would run on one machine and quietly skip on every other, which is much the same
/// as not having a test.
fn convert_dxf_to_dwg(
    conv: &str,
    dxf: &std::path::Path,
    dwg: &std::path::Path,
) -> Result<(), String> {
    // CONVERTED TO A TEMP FILE AND RENAMED ON, never written over the target.
    //
    // The first version deleted the destination before running the converter, so a conversion that
    // then failed had already destroyed the drawing it was saving over. Caught by the test named
    // for exactly that, which is the reason to write the test before believing the code.
    //
    // Unique per call, like the drawing temp: an after-calculation embed save can run alongside
    // a modal save, and two converters writing one staging file would fight over it.
    let staged = dwg.with_extension(format!(
        "dwg.savetmp.{}",
        TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&staged);
    let (in_s, out_s) = (
        dxf.to_string_lossy().to_string(),
        staged.to_string_lossy().to_string(),
    );

    // The converter takes the same shapes as the DWG→DXF one: a `{in}/{out}` template through the
    // shell, a `.cmd` wrapper through `cmd /c`, or a bare executable.
    let status = if conv.contains("{in}") {
        let cmd = conv.replace("{in}", &in_s).replace("{out}", &out_s);
        if cfg!(windows) {
            // RAW, NOT `arg`. Rust escapes an argument for a normal Windows program — quotes get
            // backslashed — and `cmd` does not use those rules, so a command with a quoted path in
            // it arrives mangled and exits 1. This is why the template form never worked here.
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                std::process::Command::new("cmd")
                    .arg("/c")
                    .raw_arg(&cmd)
                    .status()
            }
            #[cfg(not(windows))]
            {
                unreachable!()
            }
        } else {
            let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
            let shcmd = conv.replace("{in}", &q(&in_s)).replace("{out}", &q(&out_s));
            std::process::Command::new("sh")
                .arg("-c")
                .arg(&shcmd)
                .status()
        }
    } else if cfg!(windows)
        && (conv.to_ascii_lowercase().ends_with(".cmd")
            || conv.to_ascii_lowercase().ends_with(".bat"))
    {
        std::process::Command::new("cmd")
            .arg("/c")
            .arg(conv)
            .arg(&in_s)
            .arg(&out_s)
            .status()
    } else {
        std::process::Command::new(conv)
            .arg(&in_s)
            .arg(&out_s)
            .status()
    };
    let outcome = match status {
        // EXISTENCE, NOT THE EXIT CODE. accoreconsole is cheerful about failure — a script that
        // stopped at a prompt still exits zero — so the only thing worth believing is whether the
        // file is there. The import direction learned this the same way.
        Ok(_) if staged.is_file() => Ok(()),
        Ok(s) => Err(format!(
            "the DWG converter exited {s} and produced no file. A drawing that needs a font or an \
             xref AutoCAD cannot find will stop at a prompt with nothing to answer it; run \
             '{conv}' by hand on the DXF to see which."
        )),
        Err(e) => Err(format!("could not run the DWG converter '{conv}': {e}")),
    };
    if outcome.is_err() {
        let _ = std::fs::remove_file(&staged);
        return outcome;
    }
    std::fs::rename(&staged, dwg).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!(
            "the DWG was converted but could not be put in place at '{}': {e}",
            dwg.display()
        )
    })
}

fn rect_polyline(a: Vec2, b: Vec2) -> Geom {
    let v = |x: f64, y: f64| PolyVertex {
        pos: Vec2::new(x, y),
        bulge: 0.0,
    };
    Geom::Polyline(Polyline {
        vertices: vec![v(a.x, a.y), v(b.x, a.y), v(b.x, b.y), v(a.x, b.y)],
        closed: true,
        widths: Vec::new(),
    })
}

/// Explode a polyline (a rectangle is just a closed polyline) into its
/// individual segments: a `Line` per straight span and an `Arc` per bulged
/// span. The closing segment is included when the polyline is closed.
fn explode_polyline(p: &Polyline) -> Vec<Geom> {
    let n = p.vertices.len();
    if n < 2 {
        return Vec::new();
    }
    let pairs = if p.closed { n } else { n - 1 };
    let mut out = Vec::with_capacity(pairs);
    for i in 0..pairs {
        let v0 = &p.vertices[i];
        let a = v0.pos;
        let b = p.vertices[(i + 1) % n].pos;
        // DXF convention: the bulge on the START vertex curves segment i→i+1.
        if v0.bulge.abs() > 1e-9 {
            if let Some((center, radius, a0, sweep)) = cad_kernel::bulge_arc(a, b, v0.bulge) {
                out.push(Geom::Arc(cad_kernel::Arc {
                    center,
                    radius,
                    start_angle: a0,
                    sweep_angle: sweep,
                }));
                continue;
            }
        }
        out.push(Geom::Line(Line { a, b }));
    }
    out
}

/// The per-vertex stretch rule (used by the live ghost preview; mirrors the
/// logic in `apply_stretch`): any vertex / centre inside the box moves by
/// `v`, the rest stay. Returns the moved geom (an unchanged clone when
/// nothing in this dobject is inside).
/// Defining points of a geom (for the Block Task Recorder's affected-point
/// capture) — the same set `stretch_one` would move. Mirrors blockdiff's
/// feature points; bbox corners for variants not modelled here.
fn geom_def_points(g: &Geom) -> Vec<Vec2> {
    match g {
        Geom::Line(l) => vec![l.a, l.b],
        Geom::Polyline(p) => p.vertices.iter().map(|v| v.pos).collect(),
        Geom::Circle(c) => vec![c.center],
        Geom::Arc(a) => {
            let (s, e) = a.endpoints();
            vec![s, e]
        }
        Geom::Point(pt) => vec![pt.location],
        other => {
            let (mn, mx) = other.bbox();
            vec![mn, mx]
        }
    }
}

fn stretch_one(g: &Geom, win_min: Vec2, win_max: Vec2, v: Vec2) -> Geom {
    let inside =
        |p: Vec2| p.x >= win_min.x && p.x <= win_max.x && p.y >= win_min.y && p.y <= win_max.y;
    match g {
        Geom::Line(l) => Geom::Line(Line {
            a: if inside(l.a) { l.a + v } else { l.a },
            b: if inside(l.b) { l.b + v } else { l.b },
        }),
        Geom::Polyline(p) => Geom::Polyline(Polyline {
            vertices: p
                .vertices
                .iter()
                .map(|vt| {
                    if inside(vt.pos) {
                        PolyVertex {
                            pos: vt.pos + v,
                            bulge: vt.bulge,
                        }
                    } else {
                        *vt
                    }
                })
                .collect(),
            closed: p.closed,
            widths: p.widths.clone(),
        }),
        Geom::Circle(c) if inside(c.center) => Geom::Circle(Circle {
            center: c.center + v,
            radius: c.radius,
        }),
        Geom::Arc(a) => {
            let (start_pt, end_pt) = a.endpoints();
            let c_in = inside(a.center);
            let s_in = inside(start_pt);
            let e_in = inside(end_pt);
            if c_in {
                // Center in window → translate the whole arc (shape kept).
                Geom::Arc(Arc {
                    center: a.center + v,
                    ..*a
                })
            } else if s_in || e_in {
                // An ENDPOINT is in the window → it must land at end+v.
                // Rebuild the arc through the (possibly moved) endpoints,
                // preserving the bulge (curvature + CCW direction). Both
                // endpoints in → both move = a pure translation.
                let bulge = (a.sweep_angle * 0.25).tan();
                let ns = if s_in { start_pt + v } else { start_pt };
                let ne = if e_in { end_pt + v } else { end_pt };
                match cad_kernel::bulge_arc(ns, ne, bulge) {
                    Some((c, r, a0, sw)) => Geom::Arc(Arc {
                        center: c,
                        radius: r,
                        start_angle: a0.rem_euclid(std::f64::consts::TAU),
                        sweep_angle: sw,
                    }),
                    // Degenerate (coincident endpoints / ~straight) → leave as-is.
                    None => g.clone(),
                }
            } else {
                g.clone()
            }
        }
        Geom::Ellipse(e) if inside(e.center) => Geom::Ellipse(Ellipse {
            center: e.center + v,
            major: e.major,
            ratio: e.ratio,
        }),
        Geom::EllipseArc(ea) if inside(ea.ellipse.center) => Geom::EllipseArc(EllipseArc {
            ellipse: Ellipse {
                center: ea.ellipse.center + v,
                major: ea.ellipse.major,
                ratio: ea.ellipse.ratio,
            },
            start_param: ea.start_param,
            sweep_param: ea.sweep_param,
        }),
        Geom::Point(pt) if inside(pt.location) => Geom::Point(Point {
            location: pt.location + v,
            style: pt.style,
            size: pt.size,
        }),
        // A block INSTANCE is stretched by moving it WHOLE when its
        // insertion point is inside the window — AutoCAD's rule (block
        // contents aren't individually stretchable from outside; explode
        // or use the Block Task Recorder for that). Without this arm a
        // selected block never moved on stretch (it fell through to the
        // clone-unchanged catch-all → "0/N changed"). BlockRef is Copy.
        Geom::BlockRef(br) if inside(br.insert) => {
            let mut nb = br.clone();
            nb.insert = br.insert + v;
            Geom::BlockRef(nb)
        }
        other => other.clone(),
    }
}

/// CARD (cardinal H/V) lock: snap a vector to its dominant axis so the
/// recorded displacement direction is a clean ±X or ±Y. Used by the Block
/// Editor when CARD is on. (Project convention: this is "CARD", never "ortho".)
fn card_lock_vec(v: Vec2) -> Vec2 {
    if v.x.abs() >= v.y.abs() {
        Vec2::new(v.x, 0.0)
    } else {
        Vec2::new(0.0, v.y)
    }
}

/// Collect the defining endpoints of a (cut-edge) geom — used to build the
/// opening region a block cuts into host geometry.
fn edge_endpoints(g: &Geom, out: &mut Vec<Vec2>) {
    match g {
        Geom::Line(l) => {
            out.push(l.a);
            out.push(l.b);
        }
        Geom::Polyline(p) => {
            for v in &p.vertices {
                out.push(v.pos);
            }
        }
        Geom::Arc(a) => {
            let (e1, e2) = a.endpoints();
            out.push(e1);
            out.push(e2);
        }
        other => {
            let (mn, mx) = other.bbox();
            out.push(mn);
            out.push(Vec2::new(mx.x, mn.y));
            out.push(mx);
            out.push(Vec2::new(mn.x, mx.y));
        }
    }
}

/// Convex hull (Andrew's monotone chain) of a point set. Returns the hull in
/// CCW order; fewer than 3 unique points returns them as-is.
fn convex_hull(pts: &[Vec2]) -> Vec<Vec2> {
    let mut p: Vec<Vec2> = pts.to_vec();
    p.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap()
            .then(a.y.partial_cmp(&b.y).unwrap())
    });
    p.dedup_by(|a, b| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut lower: Vec<Vec2> = Vec::new();
    for &pt in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], pt) <= 0.0 {
            lower.pop();
        }
        lower.push(pt);
    }
    let mut upper: Vec<Vec2> = Vec::new();
    for &pt in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], pt) <= 0.0 {
            upper.pop();
        }
        upper.push(pt);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// True if `p` is inside (or on the boundary of) the convex polygon `poly`.
fn point_in_convex(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut sign = 0.0_f64;
    for k in 0..n {
        let a = poly[k];
        let b = poly[(k + 1) % n];
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if cross.abs() > 1e-9 {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
    }
    true
}

/// Intersection parameter `t` (on segment a→b) where it crosses segment p→q,
/// or None if parallel / not crossing within both segments.
fn seg_seg_param(a: Vec2, b: Vec2, p: Vec2, q: Vec2) -> Option<f64> {
    let r = b - a;
    let s = q - p;
    let rxs = r.x * s.y - r.y * s.x;
    if rxs.abs() < 1e-12 {
        return None;
    }
    let qp = p - a;
    let t = (qp.x * s.y - qp.y * s.x) / rxs;
    let u = (qp.x * r.y - qp.y * r.x) / rxs;
    if t >= -1e-9 && t <= 1.0 + 1e-9 && u >= -1e-9 && u <= 1.0 + 1e-9 {
        Some(t.clamp(0.0, 1.0))
    } else {
        None
    }
}

/// Clip segment a→b against convex polygon `poly`, returning the parts that
/// lie OUTSIDE the polygon (the inside part = the opening is removed).
fn clip_line_outside(a: Vec2, b: Vec2, poly: &[Vec2]) -> Vec<(Vec2, Vec2)> {
    let n = poly.len();
    let mut ts: Vec<f64> = vec![0.0, 1.0];
    for k in 0..n {
        if let Some(t) = seg_seg_param(a, b, poly[k], poly[(k + 1) % n]) {
            ts.push(t);
        }
    }
    ts.sort_by(|x, y| x.partial_cmp(y).unwrap());
    ts.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
    let mut out = Vec::new();
    for w in ts.windows(2) {
        let (t0, t1) = (w[0], w[1]);
        let mid = a + (b - a) * ((t0 + t1) * 0.5);
        if !point_in_convex(mid, poly) {
            out.push((a + (b - a) * t0, a + (b - a) * t1));
        }
    }
    out
}

/// Ttr offset curves of an object at distance `r`: the loci of centres of a
/// radius-`r` circle tangent to it. A line → two parallel lines (±r, extended
/// long so the kernel `intersect` finds crossings beyond the segment); a
/// circle/arc → two concentric circles (R+r and |R−r|).
fn ttr_offsets(g: &Geom, r: f64) -> Vec<Geom> {
    const EXT: f64 = 1.0e6;
    match g {
        Geom::Line(l) => {
            let d = l.b - l.a;
            let len = d.len();
            if len < 1e-9 {
                return Vec::new();
            }
            let u = d / len;
            let n = Vec2::new(-u.y, u.x);
            let mid = (l.a + l.b) * 0.5;
            let (a0, b0) = (mid - u * EXT, mid + u * EXT);
            vec![
                Geom::Line(Line {
                    a: a0 + n * r,
                    b: b0 + n * r,
                }),
                Geom::Line(Line {
                    a: a0 - n * r,
                    b: b0 - n * r,
                }),
            ]
        }
        Geom::Circle(c) => {
            let mut v = vec![Geom::Circle(Circle {
                center: c.center,
                radius: c.radius + r,
            })];
            let inner = (c.radius - r).abs();
            if inner > 1e-9 {
                v.push(Geom::Circle(Circle {
                    center: c.center,
                    radius: inner,
                }));
            }
            v
        }
        Geom::Arc(a) => {
            let mut v = vec![Geom::Circle(Circle {
                center: a.center,
                radius: a.radius + r,
            })];
            let inner = (a.radius - r).abs();
            if inner > 1e-9 {
                v.push(Geom::Circle(Circle {
                    center: a.center,
                    radius: inner,
                }));
            }
            v
        }
        _ => Vec::new(),
    }
}

/// Foot of tangency on object `g` for a tangent-circle centre `c` — the point
/// on `g` (its infinite line / full circle) nearest `c`.
fn ttr_foot(g: &Geom, c: Vec2) -> Vec2 {
    match g {
        Geom::Line(l) => {
            let d = l.b - l.a;
            let len2 = d.dot(d);
            if len2 < 1e-18 {
                return l.a;
            }
            l.a + d * ((c - l.a).dot(d) / len2)
        }
        Geom::Circle(ci) => {
            let v = c - ci.center;
            let len = v.len();
            if len < 1e-12 {
                ci.center
            } else {
                ci.center + v * (ci.radius / len)
            }
        }
        Geom::Arc(a) => {
            let v = c - a.center;
            let len = v.len();
            if len < 1e-12 {
                a.center
            } else {
                a.center + v * (a.radius / len)
            }
        }
        _ => c,
    }
}

/// Parse a typed coordinate `x,y` into a point. None unless both sides are
/// numbers — each side may be an expression (`2+1,3*2`), evaluated against
/// the calculator store.
fn parse_xy(store: &crate::calc::CalcStore, s: &str) -> Option<Vec2> {
    let (a, b) = s.split_once(',')?;
    Some(Vec2::new(
        if let Ok(v) = a.trim().parse::<f64>() {
            v
        } else {
            crate::calc::eval(store, a.trim()).ok()?
        },
        if let Ok(v) = b.trim().parse::<f64>() {
            v
        } else {
            crate::calc::eval(store, b.trim()).ok()?
        },
    ))
}

/// Parse an `nX` relative-scale factor (case-insensitive), e.g. `2x` → 2.0,
/// `0.5x` → 0.5. Returns None for a bare number or an `nXP` (paper-space)
/// factor — this app is model-space only and has no drawing limits, so only
/// the view-relative `nX` form is supported. See the ZOOM Scale wiring.
fn parse_scale_x(s: &str) -> Option<f64> {
    let body = s.trim().to_ascii_lowercase();
    let body = body.strip_suffix('x')?; // must end in a lone 'x' (not 'xp')
    if body.is_empty() {
        return None;
    }
    body.parse::<f64>().ok().filter(|v| *v > 0.0)
}

/// Shortest distance from point `p` to segment `a`→`b` (world units).
fn point_seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(ab);
    if len2 < 1e-18 {
        return (p - a).len();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).len()
}

/// Distinct per-parameter colour for the Block Editor (cycles through a
/// small high-contrast palette so each recorded parameter's window + arrow
/// is visually separable).
fn editor_param_color(k: usize) -> egui::Color32 {
    const PAL: [(u8, u8, u8); 6] = [
        (120, 220, 140),
        (255, 170, 90),
        (140, 180, 255),
        (240, 130, 200),
        (235, 220, 110),
        (130, 220, 220),
    ];
    let (r, g, b) = PAL[k % PAL.len()];
    egui::Color32::from_rgb(r, g, b)
}

/// Draw a simple displacement arrow (shaft + two barbs) for the Block
/// Editor's recorded-parameter overlay.
fn draw_editor_arrow(p: &egui::Painter, from: egui::Pos2, to: egui::Pos2, col: egui::Color32) {
    let stroke = egui::Stroke::new(1.4, col);
    p.line_segment([from, to], stroke);
    let d = egui::vec2(to.x - from.x, to.y - from.y);
    let len = (d.x * d.x + d.y * d.y).sqrt();
    if len < 1e-3 {
        return;
    }
    let u = egui::vec2(d.x / len, d.y / len);
    let n = egui::vec2(-u.y, u.x);
    let head = 9.0_f32.min(len * 0.4);
    let base = egui::pos2(to.x - u.x * head, to.y - u.y * head);
    p.line_segment(
        [
            to,
            egui::pos2(base.x + n.x * head * 0.5, base.y + n.y * head * 0.5),
        ],
        stroke,
    );
    p.line_segment(
        [
            to,
            egui::pos2(base.x - n.x * head * 0.5, base.y - n.y * head * 0.5),
        ],
        stroke,
    );
}

fn current_hint(tool: Tool, arc_method: ArcMethod, n: usize) -> &'static str {
    match (tool, n) {
        (Tool::None,   _) => "select a tool above, or type a command below",
        (Tool::Line,   0) => "line: click first point",
        (Tool::Line,   _) => "line: click next point — connected segments    [Esc ends]",
        (Tool::Wall,   0) => "wall: click first point of the run   (t = thickness)  [Esc exits]",
        (Tool::Wall,   _) => "wall: click next point; corners auto-join.  t = thickness · Enter ends run  [Esc cancels]",
        (Tool::Text,   _) => "text: click anchor position    [Esc cancels]",
        (Tool::Dim,    _) => "dim: click defining point (D toggles radius/diameter, Esc cancels)",
        (Tool::Circle, 0) => "circle: click center",
        (Tool::Circle, _) => "circle: click point on circumference    [Esc cancels]",
        (Tool::Ellipse, 0) => "ellipse: click CENTER",
        (Tool::Ellipse, 1) => "ellipse: click END of major axis (sets rotation + a)",
        (Tool::Ellipse, _) => "ellipse: click a point on the minor side (sets b)    [Esc cancels]",
        (Tool::EllipseArc, 0) => "ell.arc: click CENTER",
        (Tool::EllipseArc, 1) => "ell.arc: click END of major axis",
        (Tool::EllipseArc, 2) => "ell.arc: click a point on the minor side",
        (Tool::EllipseArc, 3) => "ell.arc: click START point on the ellipse",
        (Tool::EllipseArc, _) => "ell.arc: click END point on the ellipse (CCW)    [Esc cancels]",
        (Tool::Point, _) => "point: click to place    [Esc cancels]",
        (Tool::Rectangle, 0) => "rectangle: click FIRST corner    [Esc cancels]",
        (Tool::Rectangle, _) => "rectangle: click OPPOSITE corner — or type  width height  (e.g. 5 3)    [Esc cancels]",
        (Tool::Polyline, 0) => "polyline: click first vertex    [Esc cancels]",
        (Tool::Polyline, 1) => "polyline: click next vertex; Enter finishes (open); 'c' Enter closes",
        (Tool::Polyline, _) => "polyline: keep clicking vertices; Enter finishes (open); 'c' Enter closes",
        (Tool::Spline,   0) => "spline: click first control point    [Esc cancels]",
        (Tool::Spline,   1) => "spline: click next control point",
        (Tool::Spline,   2) => "spline: click next control point (Enter finishes after ≥3 ctrls)",
        (Tool::Spline,   _) => "spline: keep clicking control points; Enter finishes (open)",
        (Tool::Arc,    _) => arc_method.hint(n),
    }
}

/// Which generator the Architecture modal is editing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArchTab {
    Staircase,
    Spiral,
    Ramp,
    /// Parametric half-turn (dog-leg) stair — built as EDITABLE, boolean-able CSG solids.
    Dogleg,
    /// Parametric helical (spiral) stair — built as EDITABLE, boolean-able CSG solids.
    SpiralCsg,
    /// Parametric panelled door — built as a furniture mesh (leaf + lining + casing + hardware).
    Door,
    /// Parametric helical (spiral) ramp — a sloped annular deck with balustrades, as a furniture mesh.
    HelicalRamp,
    /// Parametric cabinet configurator — a grid of bays × tiers, each cell independently filled.
    Cupboard,
    /// Parametric kitchen cabinet run — base + optional wall lanes, worktop, plinth, tall units.
    Kitchen,
    /// Parametric handleless cabinet UNIT — close-range joinery, overlay outline fronts, grips.
    Cabin,
    /// Curved office luminaire — a lighting profile swept along a path (ring/racetrack/S-curve).
    SweepLight,
    /// Office workstation desk — a deletable feature tree (top, screen, supports, rail, grommets).
    Desk,
    /// Wood-frame sofa — a run chain of straight segments joined by +90° corner units.
    Couch,
}

/// Build the sun's light-space matrix (orthographic, framed to the scene AABB) for the shadow map.
/// `dir` points TO the sun; the light looks back along `-dir` at the scene centre.
fn sun_light_matrix(mn: glam::Vec3, mx: glam::Vec3, dir: glam::Vec3) -> [f32; 16] {
    let center = (mn + mx) * 0.5;
    let radius = ((mx - mn) * 0.5).length().max(0.5);
    sun_cascade_matrix(center, radius, dir, 0)
}

/// One cascade's light matrix: an orthographic box around the sphere `(center, radius)`, looking
/// along the sun.
///
/// `map_size` > 0 snaps the box to whole shadow texels. That snapping is not a refinement — without
/// it the map slides continuously as the camera moves and every shadow edge in the scene crawls
/// with a life of its own, which is far more distracting than a slightly coarser shadow.
fn sun_cascade_matrix(
    center: glam::Vec3,
    radius: f32,
    dir: glam::Vec3,
    map_size: i32,
) -> [f32; 16] {
    let d = dir.normalize_or_zero();
    if d == glam::Vec3::ZERO {
        return [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
    }
    let up = if d.z.abs() > 0.95 {
        glam::Vec3::Y
    } else {
        glam::Vec3::Z
    };
    let r = radius * 1.1;
    let center = if map_size > 0 {
        // Snap in LIGHT space, where a texel is axis-aligned, then come back.
        let rot = glam::Mat4::look_at_rh(glam::Vec3::ZERO, -d, up);
        let mut lc = rot.transform_point3(center);
        let unit = 2.0 * r / map_size as f32;
        lc.x = (lc.x / unit).floor() * unit;
        lc.y = (lc.y / unit).floor() * unit;
        rot.inverse().transform_point3(lc)
    } else {
        center
    };
    let eye = center + d * (radius * 2.0);
    let view = glam::Mat4::look_at_rh(eye, center, up);
    let proj = glam::Mat4::orthographic_rh_gl(-r, r, -r, r, radius * 0.1, radius * 3.9);
    (proj * view).to_cols_array()
}

/// Split the view frustum into shadow cascades, TIGHTEST FIRST.
///
/// The near ground gets its own small map and the distance gets the coarse one, which is where the
/// resolution belongs: one map over a whole site spends nearly all of itself on ground nobody is
/// looking at, and a window mullion's shadow comes out two texels wide.
///
/// Slices are cut with the standard practical scheme — part logarithmic (which is what texel
/// density actually wants, since a perspective camera's world-per-pixel grows linearly with
/// distance) and part uniform (which keeps the first slice from collapsing to nothing near the
/// camera). The far end is clamped to the scene, because there is no point giving a cascade to
/// empty air past the last building.
fn sun_cascades(
    mvp: &[f32; 16],
    eye: glam::Vec3,
    scene: (glam::Vec3, glam::Vec3),
    dir: glam::Vec3,
    n: usize,
    map_size: i32,
) -> Vec<[f32; 16]> {
    let inv = glam::Mat4::from_cols_array(mvp).inverse();
    if !inv.is_finite() {
        return Vec::new();
    }
    // The frustum's eight corners, in world space.
    let corner = |x: f32, y: f32, z: f32| -> glam::Vec3 {
        let p = inv * glam::Vec4::new(x, y, z, 1.0);
        if p.w.abs() < 1e-9 {
            glam::Vec3::ZERO
        } else {
            p.truncate() / p.w
        }
    };
    let quad = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let near: Vec<glam::Vec3> = quad.iter().map(|&(x, y)| corner(x, y, -1.0)).collect();
    let far: Vec<glam::Vec3> = quad.iter().map(|&(x, y)| corner(x, y, 1.0)).collect();
    if near.iter().chain(&far).any(|p| !p.is_finite()) {
        return Vec::new();
    }
    let depth = (far[0] - near[0]).length();
    if depth < 1e-3 {
        return Vec::new();
    }
    let d_near = (near[0] - eye).length();
    // Nothing past the far side of the model can cast a shadow anyone will see, so that is where
    // the cascades stop — otherwise an adaptive far plane out at 10 km would hand the last cascade
    // to empty sky and waste a third of the shadow budget.
    let (mn, mx) = scene;
    let scene_c = (mn + mx) * 0.5;
    let scene_r = ((mx - mn) * 0.5).length().max(0.5);
    let d_far = ((eye - scene_c).length() + scene_r).clamp(d_near + 1.0, d_near + depth);

    let n = n.clamp(1, 8);
    let mut out = Vec::with_capacity(n);
    let mut t0 = 0.0f32;
    for i in 1..=n {
        let p = i as f32 / n as f32;
        let log = d_near * (d_far / d_near.max(1e-3)).powf(p);
        let uni = d_near + (d_far - d_near) * p;
        let d = 0.75 * log + 0.25 * uni;
        let t1 = ((d - d_near) / depth).clamp(0.0, 1.0);
        // The slice's own eight corners, then the sphere around them. A SPHERE rather than a box:
        // it is invariant to the camera's rotation, so turning on the spot does not change the
        // cascade's size — and a size that changed with heading would make every shadow in the
        // scene pulse as the user orbited.
        let mut pts = [glam::Vec3::ZERO; 8];
        for k in 0..4 {
            pts[k] = near[k].lerp(far[k], t0);
            pts[k + 4] = near[k].lerp(far[k], t1);
        }
        let c = pts.iter().fold(glam::Vec3::ZERO, |a, &p| a + p) / 8.0;
        let r = pts
            .iter()
            .fold(0.0f32, |m, &p| m.max((p - c).length()))
            .max(0.5);
        out.push(sun_cascade_matrix(c, r, dir, map_size));
        t0 = t1;
    }
    out
}

pub struct CadApp {
    doc: Document,
    intersections: Vec<Vec2>,
    cmd: String,
    history: Vec<String>,
    /// Short "what to do next" prompt for the current state, shown right
    /// above the command input. Replaces the long placeholder hint with
    /// a context-aware line — empty when idle. See `set_prompt`.
    current_prompt: String,
    /// Most recent command line the user actually ran (non-empty,
    /// non-whitespace). Pressing Enter on an EMPTY cmd at the top-level
    /// prompt re-runs this — AutoCAD's "repeat last command".
    last_command: Option<String>,
    /// Which widget held the keyboard at the START of this frame, before any panel drew.
    ///
    /// The global Enter/Space cascade consults this to tell "the user pressed Enter at the command
    /// prompt" from "the user finished typing a number into a field". Captured early because a
    /// field that commits on Enter gives up focus while it is being drawn, so by the time the
    /// cascade runs the answer would be None — passing the guard exactly when it is needed.
    focus_at_frame_start: Option<egui::Id>,
    /// Counter for the 2-stage "Enter to cancel" pattern during a
    /// select-mode wait with an empty basket: 0 = no Enters yet, 1 =
    /// notice shown, 2 = next Enter cancels. Resets on any state
    /// transition or any cmd input.
    empty_enter_count_in_select: u8,
    /// The `Placement [Click/Centre/Origin/Offset] <x>:` prompt is open and the next line is its
    /// answer. Replaces the toolbar dropdown that used to ask this.
    place_prompt_open: bool,
    /// The `Distance from origin  X,Y,Z <…>:` follow-up is open and the next line is a coordinate.
    place_coord_prompt: bool,
    /// Grip drag — Some(GripDrag) while the user is dragging a grip
    /// handle of a selected dobject. v1 semantic: dragging any grip
    /// translates the whole dobject by the cursor delta.
    grip_drag: Option<GripDrag>,
    /// OTHER selected dobjects whose grip sits at the SAME world point as the
    /// one being dragged, with the role to move on each. AutoCAD moves every
    /// coincident grip of the selection together — that is how shapes sharing
    /// a corner stay joined, and how a hatch stays with the boundary it was
    /// built from. Empty unless `grip_drag` is active.
    grip_drag_peers: Vec<(usize, cad_kernel::GripRole)>,
    /// Snapshot from the last render pass — how many dobjects exist,
    /// how many landed in the viewport, how many were painted, plus
    /// per-cull skip counters. Surfaced in the "Screen Stats" floating
    /// window so the user can verify the renderer's view of the doc.
    last_render_stats: RenderStats,
    /// Whether the "Screen Stats" window is currently visible. Toggle
    /// via Tools menu.
    screen_stats_open: bool,
    /// Previous-frame value of `screen_stats_open`. Lets render code
    /// detect the false→true edge (window reopened via menu) and
    /// clear any stale dock-pos so the window appears at its default
    /// position instead of getting stuck behind a panel.
    screen_stats_was_open: bool,
    /// Background worker for hatch trace. `None` when no trace is in
    /// flight. While `Some`, every frame `poll_hatch_worker` tries to
    /// receive the result; on receipt the loops are materialised as
    /// boundary polylines + a Hatch dobject and this field clears.
    /// See `HatchWorker` doc comment for the full lifecycle.
    hatch_worker: Option<HatchWorker>,
    /// Cooperative-cancellation flag for long-running operations
    /// (hatch trace, intersect-everything, bulk modify, …). The flag
    /// is set by the global Esc handler; cancellable functions read
    /// it periodically and bail out early when set.
    ///
    /// CAVEAT: today the heavy ops run SYNCHRONOUSLY inside `update`,
    /// so the egui input snapshot doesn't refresh mid-operation —
    /// Esc presses that happen DURING a long op aren't seen until the
    /// op completes. To make mid-op Esc work we need to background
    /// the op (std::thread or rayon) and have the worker check this
    /// flag while the UI thread keeps spinning. That refactor is the
    /// next slice; this primitive is the API the worker will use.
    ///
    /// For now the flag is useful in two cases:
    ///   1. Esc PRE-set before a new op starts (resets at op begin)
    ///   2. Multi-phase ops can check between phases (already partial
    ///      protection — a long phase still freezes the UI)
    op_cancel: StdArc<AtomicBool>,
    /// Edge-docked positions for floating Windows. When the user drags
    /// a Window within `DOCK_THRESHOLD_PX` of any screen edge, we snap
    /// it flush to that edge and record the snapped position here;
    /// the next frame's window-show pass passes the stored position
    /// via `current_pos(...)` so the snap "sticks". Dragging the
    /// Window away further than the threshold removes the entry, so
    /// the snap is reversible.
    ///
    /// Stores both the snapped position AND the edge the window is
    /// docked to. Edge drives sizing: docked to TOP/BOTTOM forces the
    /// window to span the full screen width (it becomes a horizontal
    /// strip); docked to LEFT/RIGHT forces full screen height
    /// (vertical strip). Corner docks pin position but leave size
    /// content-driven.
    docked_window_pos: HashMap<&'static str, DockState>,
    /// §10 menu-launch anchor: window-id → the screen pos a menu item wants the
    /// panel to FIRST open at (adjacent to the launching row). Applied as
    /// `default_pos` in `apply_dock_pos`, so it only governs the first open /
    /// no-remembered-position case — once the user moves the panel, egui's own
    /// remembered geometry wins (WORKSPACE §5).
    menu_launch_anchor: HashMap<&'static str, egui::Pos2>,
    /// §10 bring-to-front: window-ids to raise above other panels this frame (a
    /// menu just toggled them ON). Consumed (removed) when the window renders and
    /// gets `move_to_top`'d — so it fires once on open regardless of render order,
    /// and regardless of whether the panel uses a remembered position.
    raise_windows: Vec<&'static str>,
    /// Accumulated hold-time on each docked window's title bar. Once
    /// it reaches `DOCK_UNDOCK_HOLD_SEC` (~200 ms), the dock state is
    /// cleared and the window goes back to free positioning so the
    /// user can drag it elsewhere. Reset to zero whenever the button
    /// is released. Per-window so two windows being held simultaneously
    /// don't share a clock.
    dock_undock_hold: HashMap<&'static str, f32>,
    /// IDs of windows that the user is currently dragging. Used by
    /// `process_dock_after_show` to evaluate snap-to-edge ONLY at the
    /// end of a drag (drag-release transition). Without this gate,
    /// snap would fire on any idle frame where the window happens to
    /// sit near a screen edge — which auto-docks freshly-opened
    /// windows on appear, before the user has even touched them.
    dock_dragging: std::collections::HashSet<&'static str>,
    /// Timestamp (egui `ctx.input(|i| i.time)` units, seconds since
    /// app launch) of the most recent primary-button press on the
    /// canvas. Reset to None on release. Drives the time-gated drag
    /// classifier: a window-drag only activates after the press has
    /// been held longer than `env.SelDmTm`. Without this gate, fast
    /// accidental drags during a click registered as windows.
    press_time: Option<f64>,
    /// Screen + world position of the most recent primary-button press
    /// on the canvas. Reset to None on release. We stash this because
    /// `egui::Pointer::press_origin()` is cleared by the time
    /// `drag_stopped()` fires on the release frame — relying on it
    /// makes press_release_dist read 0, which silently kills every
    /// window-drag gesture. Read by the unified click/drag classifier,
    /// the window-drag application, and the rubber-band preview.
    press_pos: Option<(egui::Pos2, Vec2)>,
    /// Screen-space rect of the canvas (central panel) from the last
    /// frame. Docking uses this — NOT `ctx.screen_rect()` — so docked
    /// strips align with the canvas area instead of overlapping the
    /// menubar / toolbar / status bar. Captured inside the
    /// CentralPanel show closure, read by `apply_dock_pos` /
    /// `process_dock_after_show` which run earlier in the same frame
    /// (so they actually read the PREVIOUS frame's rect — fine, since
    /// the canvas rect only changes when the user resizes the app
    /// window).
    canvas_screen_rect: Option<egui::Rect>,
    /// The demo plan a fresh app ships with is drawn at REAL room sizes
    /// (thousands of drawing units in the default millimetre document), so
    /// the default launch camera — origin plus a couple of hundred units —
    /// would show an empty canvas. Framed once on the first real frame, while
    /// the view is still pristine (`view_history` empty), so the plan is on
    /// screen; a file open refits by itself and the user's own zoom is theirs.
    demo_view_set: bool,
    /// Open/closed state for each dockable Window panel. Default true
    /// for the most-used panels. Toggled from the Tools menu.
    cmd_window_open: bool,
    /// Was the 3D Factory on screen LAST frame? The rising edge is what `on_factory_opened`
    /// fires on -- thirty places set `factory.open`, and none of them has to know.
    factory_was_open: bool,
    /// Was the SIMLUX 3D view on screen LAST frame? Same rising-edge pattern:
    /// the mode hooks switch to the SIMLUX tab when its view opens.
    simlux_was_open: bool,
    /// The current workspace, chosen by the top tab bar. See [`Mode`]: exactly
    /// one full-window workspace at a time, enforced at frame start.
    mode: Mode,
    /// Was a factory face-sketch open LAST frame? The rising edge is when the
    /// sketch takes over the whole window for drafting; the falling edge is
    /// when finishing it returns to the workspace it started from.
    factory_was_in_sketch: bool,
    /// A face-sketch was started from the 3D Factory workspace (which hides
    /// the 2D canvas). The sketch drafts on the canvas, so it took over the
    /// window; finishing/cancelling it returns to the Factory. Cleared by any
    /// manual tab click — a user who switched tabs wants to stay where they
    /// are when the sketch ends.
    factory_return_after_sketch: bool,
    /// Pin the SIMLUX 3D panel to the full window for one frame after its mode
    /// is entered (egui remembers panel widths per id, so without this a
    /// maximised workspace would reopen at whatever narrow width a past drag
    /// left). Same idea as the split-mode half-width pin in
    /// `render_light_3d_panel`.
    simlux_full_pin: bool,
    /// Same one-frame full-window pin for the 3D Factory panel.
    factory_full_pin: bool,
    /// Mode command panel sections the user has COLLAPSED (the `id` each
    /// section is keyed by). Absent = open. Shared across the three workspaces
    /// so a choice made in one mode is honoured in the others.
    mode_sections_closed: std::collections::HashSet<String>,
    /// The open "Make room" details form, or None. Set by the 2D ROOMS row
    /// when the selection is a valid closed outline; the outline + the asked
    /// parameters live here until the user confirms (one undo step) or
    /// cancels / Escs / the workspace changes away from the 2D drafting view.
    room_form: Option<RoomForm>,
    /// Request focus for the form's NAME field on the next frame it draws
    /// (set when the form opens, so the first keystroke goes to the name).
    room_form_focus_name: bool,
    /// Screen rect of the top menu-bar panel (updated each frame). Used to
    /// keep a hover-opened category menu open while the pointer is still over
    /// its button, and close it once the pointer leaves both bar and popup.
    menubar_rect: egui::Rect,
    /// Command-bar docking state (docked region vs floating), driven by the
    /// unified dock host (see dock.rs) — the same engine the Inspector uses.
    /// Defaults to floating lower-left; drag its header to an edge to dock
    /// (Bottom = the AutoCAD-style command strip), drag out to float again.
    layers_window_open: bool,
    pens_window_open: bool,
    info_window_open: bool,
    /// Inspector docking state (docked region vs floating), driven by the
    /// unified dock host (see dock.rs). Drag the header out to float; drag the
    /// float to an edge to re-dock.
    inspector_dock_state: crate::dock::DockState,
    /// One-shot: when true, the next Properties-panel render records the
    /// pixel geometry of every element into the session recorder, then
    /// clears the flag. Activated from the recorder window.
    props_layout_capture: bool,
    /// Live UI inspector: when on, every `pp_capture`'d element is measured each
    /// frame and hovering one shows its name + exact w×h + position — a
    /// devtools-style ruler for comparing the build against the design spec.
    ui_inspect: bool,
    /// Click-to-log buffer for the UI inspector: each click records the clicked
    /// element + its full box hierarchy (name, w, h, x, y). Shown in a copyable
    /// "UI Inspect Log" window so the user can dump a precise report.
    ui_inspect_log: Vec<String>,
    /// Last-invoked command name, for the active highlight in the mode command
    /// panel rows (and the menus that reuse it).
    rail_active: String,
    /// Per-command visible lists — **registry ids** (`"draw.line"`), in the
    /// user's order. `registry.get(id)` supplies icon/tooltip; order lives in
    /// the Vec. (The DRAW/MODIFY icon rails that used to display them are gone;
    /// the mode command panel lists the same commands.)
    draw_items: Vec<crate::command::CommandId>,
    modify_items: Vec<crate::command::CommandId>,
    /// Command metadata registry (COMMAND_REGISTRY_MENTOR). Derived from
    /// `DRAW_CMDS`/`MODIFY_CMDS` at startup. The mode command panel and the
    /// command palette render from it; everything dispatches via `execute`.
    command_registry: crate::command::CommandRegistry,
    /// Temporary Phase-2 verification: Tools ▸ Debug ▸ "Command registry dump".
    cmd_dump_open: bool,
    /// Command palette (Phase 7): registry-driven fuzzy command search.
    /// Ctrl+Shift+P or Tools ▸ "Command palette". Searches title + keywords,
    /// dispatches via `execute(id)` (D2), applies the 6b predicates.
    palette_open: bool,
    palette_query: String,
    palette_sel: usize,
    /// One-shot: request focus on the search box the frame after opening.
    palette_focus: bool,
    /// Palette window position — `None` until first dragged (then it's user-
    /// placed and persists for the session). Dragging the Floating header band
    /// moves it (§3.1).
    palette_pos: Option<egui::Pos2>,
    /// Open flyout STACK (§9) — frame 0 spawned by a menubar row's ▸, deeper frames
    /// by a flyout row's ▸ (nesting). Rendered at the top level so it survives the
    /// parent dropdown closing (MENU_DROPDOWN §2.1); opens on HOVER, closes after a
    /// short delay once the pointer leaves the whole stack. Empty = nothing open.
    menu_flyouts: Vec<FlyFrame>,
    /// Set true each frame the pointer is over an arrow (by the menu) or the
    /// flyout (by `render_menu_flyouts`); drives the hover stay-open + close delay.
    menu_flyout_hot: bool,
    /// `ctx.input().time` when the pointer left both the arrow and the flyout —
    /// the flyout closes `MENU_FLYOUT_CLOSE_DELAY` seconds later (travel tolerance).
    menu_flyout_leave_t: Option<f64>,
    /// Per-command **preferred method** (dispatch token → method key), session
    /// level. Set by a command's ▼ flyout; APPLIED by `execute(id)` so every
    /// surface (menu, rail, palette, shortcut) runs the remembered method. The
    /// rail icon also renders that method's glyph. (Command line is exempt — it
    /// runs base/explicit until the deferred command-methods track.)
    command_method: std::collections::HashMap<String, String>,
    dobjects_window_open: bool,
    /// Pattern args for the in-progress hatch op — set when the user
    /// runs `hatch [NAME] [scale] [angle]`, consumed by `apply_hatch`
    /// after the boundary-selection session finalises. Default
    /// (None, 1.0, 0.0) = solid fill.
    pending_hatch_pattern: (Option<String>, f64, f64),
    /// "Choose Hatch Attributes" modal — open when the user runs bare
    /// `hatch` so they can pick pattern + scale + angle with a live
    /// preview before any boundary is selected. Mirrors LibreCAD's
    /// dialog. State lives here so the dialog persists last-used
    /// choices across openings.
    hatch_dialog_open: bool,
    hatch_dialog_solid: bool,
    hatch_dialog_name: String, // catalog name when solid==false
    hatch_dialog_scale: f64,
    hatch_dialog_angle: f64,
    /// "Click inside a region to hatch it" mode — armed by the
    /// dialog's "Pick Point" button. Next pointer-mode click runs the
    /// smallest-containing-closed-dobject search, hatches it, and —
    /// per user request — stays armed for additional picks until
    /// Enter or Esc ends the session. Pattern args are remembered
    /// across clicks via `hatch_pick_point_session`.
    hatch_pick_point_armed: bool,
    /// Pattern args for the active pick-point SESSION (the period
    /// from "Pick Point button clicked" through "Enter / Esc ends
    /// it"). Restored into `pending_hatch_pattern` before each
    /// click's apply, since `apply_hatch` consumes that field —
    /// without snapshotting, the second click would render SOLID
    /// instead of the originally-chosen pattern.
    hatch_pick_point_session: Option<(Option<String>, f64, f64)>,
    /// Post-apply confirmation panel — shown after every successful
    /// `apply_hatch()` so the user can confirm, edit, or extend the
    /// hatch they just created without leaving the hatch flow. Set by
    /// `apply_hatch` on success, cleared by the panel's buttons.
    hatch_confirm_open: bool,
    /// Snapshot of the document state captured at the START of a hatch
    /// preview session — taken on the FIRST `apply_hatch` in the
    /// session, restored on "Discard". On "Confirm" we promote it onto
    /// `undo_stack` so a single undo from then on reverts the whole
    /// hatch flow (including any "Change pattern/Scale" tweaks) as one
    /// step. While the session is open we DON'T push to `undo_stack`,
    /// so iterative tweaking doesn't bloat undo history.
    hatch_preview_snap: Option<cad_kernel::Document>,
    /// Index of the LAST Hatch dobject created in this flow, used by
    /// the post-apply confirmation panel's "Change pattern/Scale"
    /// action to MODIFY the existing hatch in-place instead of creating
    /// a new one. None means no recent hatch (panel hides).
    hatch_last_idx: Option<usize>,
    /// When true, the next dialog OK applies its pattern/scale/angle to
    /// `hatch_last_idx` (in-place edit) instead of pushing a new Hatch.
    /// Set by the confirm panel's "Change pattern/Scale" button.
    hatch_dialog_edit_mode: bool,
    /// ACI polar-wheel picker — shared state for the floating picker
    /// window. The same window serves every call site; `pick_request`
    /// names who asked for the pick so the chosen ACI flows back to
    /// the right slot. See `aci_picker.rs` and the user's reference
    /// HTML at `~/workspace/RUST_CAD/ACI_Picker_UI.html`.
    aci_picker: crate::aci_picker::AciPickerState,
    aci_pick_request: Option<AciPickRequest>,
    /// Target indices for `AciPickRequest::DobjectMany` (Properties dialog
    /// group color). Kept off the enum so `AciPickRequest` stays `Copy`.
    aci_pick_many: Vec<usize>,
    /// Highlight overlay for the parametric points found by `blockdiff` —
    /// drawn on every instance of the two compared blocks. Cleared on Esc,
    /// `clear`, or a new compare. See `BlockDiffOverlay`.
    blockdiff_overlay: Option<BlockDiffOverlay>,
    /// Pick-two-blocks-on-screen flow for compare (see `BlockDiffPick`).
    blockdiff_pick: BlockDiffPick,
    /// "Set parameters on block" dialog (opens after the pick). See
    /// `ParamNameDialog`.
    param_name_dialog: Option<ParamNameDialog>,
    /// Insert-time value prompt for a parametric block. See `InsertParamPrompt`.
    insert_param_prompt: Option<InsertParamPrompt>,
    /// Active Block Task Recorder session. See `BlockTaskRec`.
    block_task_rec: Option<BlockTaskRec>,
    /// True right after a btr stretch: the next cmd-line input names that
    /// recorded function (Enter skips). Returns control to the recorder.
    btr_awaiting_name: bool,
    selected: Option<usize>,

    tool: Tool,
    arc_method: ArcMethod,
    arc_picker_open: bool,
    /// The last point PICKED/placed during a draw command (AutoCAD's "last
    /// point"). When a draw tool/flow is waiting for its FIRST point, pressing
    /// Enter/Space uses this point — so a new line/arc/circle/… continues from
    /// where the previous command ended.
    last_point: Option<Vec2>,
    pending: Vec<Vec2>,
    /// Per-segment bulge for the polyline tool. `pending_bulges[i]` is the
    /// AutoCAD bulge (tan(theta/4)) of the segment from `pending[i]` to
    /// `pending[i+1]`, so it's always one shorter than `pending` while
    /// drawing. `0.0` = straight segment. Maintained only by the polyline
    /// tool — other tools leave it empty.
    pending_bulges: Vec<f64>,
    /// Polyline draw sub-mode (AutoCAD PLINE's Line / Arc toggle).
    /// Toggled inline by typing `a` (Arc) or `l` (Line) while pline is
    /// active. Each captured vertex inherits this mode's segment kind.
    pline_mode: PlineMode,
    /// Per-click flow inside PLINE Arc mode — `Normal` for the default
    /// tangent-continuous arc, `AwaitingSecondPt` after the user typed
    /// `s` (the next click captures a midpoint on the arc curve), then
    /// `AwaitingSecondPtEnd(mid)` until the endpoint click commits a
    /// 3-point arc. Resets to `Normal` after each arc commits.
    pline_arc_sub: PlineArcSub,
    /// PLINE Arc `d`/Direction override: when set, the NEXT arc segment uses
    /// this unit vector as its START tangent (instead of the previous segment's
    /// exit tangent). Captured by the click/angle after `d`, consumed (cleared)
    /// once that arc segment commits.
    pline_dir_override: Option<Vec2>,
    /// PLINE Width: the (start, end) width applied to the NEXT committed
    /// segment. After a segment commits, the end width carries as the next
    /// start (AutoCAD behaviour). (0,0) = thin.
    pline_next_width: (f64, f64),
    /// In-progress `w`/`h` width-entry flow.
    pline_width_cap: PlineWidthCap,
    /// Per-segment (start,end) widths captured so far, parallel to
    /// `pending_bulges`. Drained into `Polyline.widths` on commit.
    pending_widths: Vec<(f64, f64)>,
    /// SPLINE uniform ribbon width — one value for the whole curve (splines
    /// have no per-segment taper). 0 = thin stroke. Applied at commit
    /// (`Spline::with_width`). STICKY across splines like the pline width.
    spline_width: f64,
    /// Spline-tool degree override (issue #47): `qb`/QuadBezier drives the
    /// spline tool at degree 2 — three control clicks (P0, P1, P2), then
    /// Enter commits a quadratic B-spline. `None` = cubic (the default).
    spline_degree_override: Option<usize>,
    /// The spline tool's `w`/`width` sub-command is waiting for a number (the
    /// user chose `w`/`width`). The next command-line number sets `spline_width`.
    spline_width_wait: bool,

    scale: f32,
    world_offset: egui::Vec2,

    // ---- ZOOM command ----------------------------------------------------
    /// Active ZOOM sub-flow; `Off` when no ZOOM command is running.
    zoom_state: ZoomState,
    /// View history for ZOOM Previous — `(scale, world_offset)` snapshots,
    /// most recent last, capped at 10 (AutoCAD's depth). Pushed before any
    /// programmatic view change so Previous steps back through zoom history.
    view_history: Vec<(f32, egui::Vec2)>,

    // array dialog
    array_open: bool,
    picking_source: bool, // dialog hidden, waiting for the user to click an dobject
    array_cols: usize,
    array_rows: usize,
    array_dx: f64,
    array_dy: f64,
    // Polar / path methods — see ArrayMethod.
    array_method: ArrayMethod,
    array_count: usize,
    array_fill_deg: f64,
    array_rotate_items: bool,
    array_center: Vec2,
    array_pick_center: bool,
    array_path_idx: Option<usize>,
    array_path_measure: bool,
    array_path_dist: f64,
    array_path_align: bool,
    array_path_anchor: PathAnchor,
    array_pick_path: bool,

    // intersection modes (no more global O(N²) auto-recompute)
    intersect_pending_click: bool, // one-shot "intersect near next click"
    intersect_view_pending: bool,  // deferred — needs canvas-rect to know what's visible
    last_visible: Option<(Vec2, Vec2)>, // visible world bbox from the last frame
    last_intersect_label: String,  // shown next to the buttons

    // object-snap override, single-shot: armed by typing a snap code (PER, …),
    // consumed by the next canvas click during a draw.
    snap_override: Option<SnapKind>,

    // persistent osnap state — checkboxes in the floating snap window. The
    // screen-space search radius lives in `env.SpTGSZ` (User-Environment
    // Settings); same for grip enable in `env.GrpEnb`.
    snap_enabled: SnapSet,
    snap_window_open: bool,

    /// User-Environment Settings — cryptic-named field for each AutoCAD-
    /// style SYSVAR. Persisted to `$HOME/.config/rust_cad/user_env.txt`.
    env: UserEnv,
    /// Settings window visibility.
    settings_open: bool,
    /// Which section (left sidebar) is selected in the settings window.
    /// Empty → default to the first section on first render.
    settings_section: String,
    /// When `Some(name)`, the command line is waiting for the NEW VALUE of a
    /// SYSVAR (after `setvar NAME` or a bare variable name). The next input is
    /// consumed as that value (validated via varreg::env_set). Esc cancels.
    var_set_pending: Option<String>,

    /// Command-line calculator + lazy user variables (see calc.rs). Persisted
    /// per drawing through the SIMLUX sidecar (`SimluxConfig.vars`); no
    /// drawing open → session-only.
    calc: crate::calc::CalcStore,
    /// Result channel of the in-flight calculator-variables sidecar patch
    /// (None = no patch running). The patch runs on a worker so a big sidecar
    /// never freezes the command line, and one patch at a time keeps
    /// consecutive assignments sequentially consistent.
    calc_sidecar_rx: Option<std::sync::mpsc::Receiver<Result<(), String>>>,

    // ---- Quick Access Toolbar (top line) -----------------------------
    /// Ordered list of file/common actions shown as icon shortcuts on the
    /// top Quick Access row. Customizable via the `▾` dropdown.
    qat_actions: Vec<QatAction>,
    /// Whether the QAT "customize" drop window is open.
    qat_customize_open: bool,
    /// True only on the frame the drop window opened — suppresses the
    /// click-outside dismissal for that frame (the opening click was the
    /// chevron, which sits outside the window).
    qat_just_opened: bool,
    /// Screen rect of the customize chevron, captured each frame so the drop
    /// window can open directly beneath it.
    qat_chevron_rect: Option<egui::Rect>,
    /// Brand logo texture (loaded once from assets/logo.png). None until
    /// loaded; `logo_load_tried` prevents re-attempting every frame when the
    /// file is absent (falls back to a painted placeholder).
    logo_tex: Option<egui::TextureHandle>,
    logo_load_tried: bool,

    // "Always-listen" command line: set when something else stole keyboard
    // focus (canvas click, window switch). The command-box renderer
    // reclaims focus on the next frame.
    refocus_cmd: bool,

    // Snap-candidate cycling. When multiple snap targets are within range
    // (e.g. CEN + NEA on the same arc, or two QUA quadrants), Tab cycles
    // through them. The index is reset to 0 whenever the cursor moves more
    // than a few pixels (a different hover position is a different question).
    snap_cycle_index: usize,
    snap_cycle_anchor: Option<egui::Pos2>,

    // Multi-dobject selection (separate from the `selected: Option<usize>`
    // single-pick used by the array dialog). Built up by the `list` / `select`
    // commands; consumed when the user presses Enter to finalise. AutoCAD-
    // style sub-modes (Add / Remove / Previous / None / All) are typed at the
    // command line during the session.
    select_mode: SelectMode,
    selection: Vec<usize>,
    /// First corner of an in-progress window selection.
    window_first: Option<Vec2>,
    /// When the user explicitly typed `w` / `c` to arm window/crossing
    /// mode, this overrides the drag-direction default for the next two
    /// clicks. None = direction-based default (L→R inside, R→L crossing).
    /// Some(true) = forced inside-window, Some(false) = forced crossing.
    armed_window_inside: Option<bool>,
    /// FENCE arming for TRIM/EXTEND target phases (and select sessions):
    /// two clicks define a crossing line, every dobject it crosses is cut /
    /// extended (or added to the selection). Cleared when the run finishes.
    fence_armed: bool,
    /// First click of an armed fence, awaiting the second.
    fence_first: Option<Vec2>,
    /// When true, canvas clicks REMOVE the under-cursor dobject from the
    /// selection instead of adding. Toggled by typing `remove` / `add`
    /// during the session.
    select_remove_mode: bool,
    /// The last finalised selection — `prev` re-adds these indices.
    selection_prev: Vec<usize>,
    /// The last ERASE's dobjects (doc order) — the `oops` command restores
    /// them independent of the undo stack (AutoCAD OOPS).
    last_erased: Vec<DObject>,

    // ---- Move tool (uses the active selection) ----
    move_state: MoveState,
    /// Operation queued behind an in-progress selection session. When the
    /// user finalises the session with Enter, this op is dispatched (e.g.
    /// `Move` → enter MoveState::WaitingForBase). `None` means a plain
    /// `select` / `list` with no follow-up.
    queued_op: QueuedOp,

    // FPS smoothing
    fps_smooth: f32,

    // spatial index — lazily (re)built on first ∩ query, kept around for both
    // ∩ modes and (when fresh) viewport culling.
    index: Option<UniformGrid>,
    index_dirty: bool,
    index_label: String,

    // GPU renderer + render-mode switch (debug window)
    render_mode: RenderMode,
    debug_open: bool,
    gpu_renderer: StdArc<Mutex<GpuShapeRenderer>>,
    /// SIMLUX lighting: engine state + Light panel (IES, materials, room
    /// height, computed lux grid). Driven by the `cad_light` crate.
    light: crate::light::LightState,
    /// SIMLUX 3D viewport renderer (offscreen-FBO glow), shared into the egui
    /// PaintCallback like `gpu_renderer`.
    light3d_renderer: StdArc<Mutex<crate::light3d::Scene3dRenderer>>,
    /// SIMLUX 3D: THE HEAVY HALF OF THE SCENE BUFFER, BUILT ONLY WHEN IT CHANGES.
    ///
    /// `build_scene3d_verts` ran every frame the view was open and rebuilt everything in it. The
    /// room geometry is the expensive part by a distance: on the reference gym plan the furniture
    /// alone is 7,030,514 triangles, so every frame cloned or re-transformed all of them and
    /// expanded them into 40-byte vertices — roughly 844 MB allocated, filled and dropped, on the UI
    /// thread, while the 2D plan beside it was being edited. That is what "it lags while I place
    /// luminaires" was.
    ///
    /// SPLIT BY HOW OFTEN IT CHANGES, not by what it draws. This half is the room, and it moves only
    /// when the model, the calculation, the materials or the ceiling toggle do — rarely, and never
    /// during a drag. The lux sheet, the fitting markers and the aim arrows are still rebuilt every
    /// frame: they are bounded (the sheet by `overlay_res`'s 12,000-cell budget, the rest by the
    /// number of fittings) and they depend on things that really do change per frame, like the
    /// camera distance the marker glyph is sized by.
    ///
    /// `Arc` because the buffer is handed to a paint callback that outlives the borrow, and cloning
    /// it to do that would hand back everything the cache saves.
    scene3d_static: StdArc<Vec<crate::light3d::V3>>,
    /// What [`Self::scene3d_static`] was last built from — see `scene3d_static_key`. `None` means
    /// never built, which is not the same as built-from-an-empty-scene.
    scene3d_static_key: Option<u64>,
    /// SIMLUX PERF MONITOR — the same taps the 3D Factory has, for the view that had none.
    ///
    /// `FACTORY PERF` events come only from `render_factory_panel`, so a SIMLUX session records
    /// NOTHING and every frame number in such a dump describes a view that was not running. Three
    /// rounds of work went into the 3D Factory's triangle count on the strength of measurements
    /// from the wrong window before that was noticed; the absence of perf events in a SIMLUX dump
    /// was the only clue, and absence is a poor signal to have to read.
    simlux_perf_last_frame: Option<std::time::Instant>,
    simlux_perf_last_slow: Option<std::time::Instant>,
    /// The static key at the previous SIMLUX frame, so a rebuild is visible as a rebuild.
    simlux_perf_key: Option<u64>,
    /// What the 2D LUX OVERLAY cost at its last paint, and over how many cells.
    ///
    /// It lives in the 2D canvas, not the SIMLUX window, but it runs only when a lighting result
    /// exists — so it is a SIMLUX cost that no 3D counter can see, and it is where the lag actually
    /// was: one egui shape per cell, up to 16,384 of them, every frame. `Cell` because
    /// `paint_lux_overlay` takes `&self`.
    lux_overlay_us: std::cell::Cell<u64>,
    lux_overlay_cells: std::cell::Cell<u32>,
    /// The SIMLUX GL draw alone, microseconds, `glFinish` either side — written by the paint
    /// callback and read by the tap on the NEXT frame. Atomic because the callback owns nothing.
    ///
    /// Only this separates "the GPU is slow" from "everything else in the frame is slow", and
    /// inter-frame wall time cannot: it is the whole app frame, and I have twice reasoned wrongly
    /// from it. `glFinish` is a stall, so it is only issued while the recorder is running.
    simlux_gl_us: StdArc<std::sync::atomic::AtomicU64>,
    /// WHEN THE CURRENT FRAME'S WORK BEGAN, set at the top of `update`.
    ///
    /// The perf taps measured the wall time BETWEEN paints and called it the frame time. That is
    /// not what it is: the app deliberately idles at a 5 Hz heartbeat
    /// (`request_repaint_after(200 ms)`), so an idle frame reads as 210-230 ms and every one of
    /// them was flagged SLOW. A whole session of that was reported as "the app is at 4.8 fps" when
    /// it was asleep. The interval is still worth printing -- it says whether the app is being
    /// driven at all -- but the number that decides SLOW has to be WORK.
    frame_start: Option<std::time::Instant>,
    /// PHASE CHECKPOINTS WITHIN ONE FRAME, microseconds since `frame_start`.
    ///
    /// `cpu` says the frame costs 205 ms and `gl` says only 4-25 ms of that is the SIMLUX draw,
    /// and both taps report the SAME cpu 0.2 ms apart -- so the time is spent before either of
    /// them and neither can say where. Bisecting a 2,000-line `update` by reading it is how the
    /// last four rounds went; this is the version that answers in one dump.
    frame_marks: Vec<(&'static str, u32)>,
    /// 3D FACTORY: the cad_solid model + its view. Reuses `light3d_renderer`.
    factory: crate::factory::FactoryState,
    /// 3D-Factory PERF MONITOR (recorder tap). Previous opaque render buffer, so a rebuild
    /// (scene changed — e.g. a furniture import) is detected by `Arc` identity; the instant
    /// of the previous 3D frame, for the whole-frame delta; and a throttle so `slow-frame`
    /// events can't flood the dump. Purely diagnostic — all `None` until the 3D view paints.
    factory_perf_prev: Option<StdArc<Vec<crate::light3d::V3>>>,
    factory_perf_last_frame: Option<std::time::Instant>,
    factory_perf_last_slow: Option<std::time::Instant>,
    /// LOCAL mesh of the furniture currently being dragged (instance idx + baked verts), built
    /// once per drag and drawn via a GPU model matrix so a move/rotate keeps full form without
    /// re-transforming vertices each frame. `None` when not dragging furniture.
    /// Cache of furniture LOCAL meshes (baked once, keyed by asset+colour), handed to the
    /// renderer which uploads each to its own GPU buffer once. Furniture is then drawn with
    /// just a model matrix — never CPU-transformed on import/move/rotate, however heavy.
    furniture_gpu_meshes: std::collections::HashMap<u64, StdArc<Vec<crate::light3d::V3>>>,
    /// Per-key TEXTURED furniture meshes (box-projected UVs), cached like `furniture_gpu_meshes`
    /// so a textured piece is built once and drawn from a persistent GPU buffer thereafter.
    furniture_tex_meshes: std::collections::HashMap<u64, StdArc<Vec<crate::light3d::TexVtx>>>,
    /// Per-key TRANSLUCENT furniture meshes (glass panes), cached like `furniture_gpu_meshes`
    /// so the see-through triangles are built once and drawn from a persistent GPU buffer.
    furniture_transp_meshes: std::collections::HashMap<u64, StdArc<Vec<crate::light3d::V3A>>>,
    /// Clipboard-texture pixels shared into the GL paint closure as `Arc` (cheap per-frame
    /// clone, no pixel copy). The renderer uploads each to a GL texture once, keyed by index.
    texture_rgba: std::collections::HashMap<usize, StdArc<Vec<u8>>>,
    /// Version of the opaque CSG buffer's GPU upload, bumped only when the scene actually
    /// changes — so a static scene isn't re-uploaded every frame.
    factory_scene_ver: u64,
    /// Which viewport is ACTIVE (last interacted with) — the modifier dispatch signal.
    active_view: ActiveView,
    /// O(1) "is dobject i selected?" lookup for the DRAW LOOP, rebuilt once per frame.
    ///
    /// ⚠️ This exists because `self.selection.contains(&i)` is a LINEAR SCAN of a Vec,
    /// and the draw loop ran it PER DRAWN DOBJECT PER FRAME. Measured on a real 1.5M
    /// session: 141,766 drawn × 52,650 selected = 7.46 BILLION comparisons → a
    /// 21,815 ms frame. The same frame with nothing selected drew in 25.2 ms. Select a
    /// big window and the app froze for 20 seconds.
    ///
    /// The Vec allocation is REUSED across frames (clear+resize, no realloc), so the
    /// per-frame cost is an O(n) memset (~0.1 ms at 1.5M) plus O(selection) to fill —
    /// instead of O(drawn × selection).
    sel_mask: Vec<bool>,
    gpu_dirty: bool,
    /// MONOTONIC COUNTER OF "the drawing changed", bumped by every site that sets `gpu_dirty`.
    ///
    /// `gpu_dirty` cannot be a cache key, and the reason is worth stating: it is CONSUMED —
    /// `std::mem::take` in the 2D path, plain assignment in two others — so whichever consumer
    /// runs first eats the signal and the rest never see it. Two edits in one frame are also
    /// indistinguishable from one. A counter has neither problem: readers compare against their
    /// own last-seen value and no reader can destroy another's.
    ///
    /// Bumped in `touch_view`, which is what all 60-odd mutation sites call. The invariant is
    /// therefore structural rather than remembered: you cannot mark the view dirty without
    /// advancing the counter, because there is one function that does both.
    view_seq: u64,
    /// The two EXPENSIVE 3D line builders, held across frames. See [`Self::lines_sig`].
    ///
    /// Measured before this existed: a 265-object drawing spent 15.7 ms per frame re-flattening
    /// its plan into 312,576 vertices — 12.2 MB, rebuilt sixty times a second for a drawing that
    /// was not changing, and 112% of a frame's budget before anything was drawn.
    ///
    /// Kept as TWO buffers rather than one concatenated block because ORDER between the builders
    /// is load-bearing: the picked-face outline has to be emitted after the finished sketches so
    /// the yellow reads over the drawing rather than z-fighting under it, and it sits between
    /// these two.
    cached_sketch_lines: Vec<crate::light3d::V3>,
    cached_plan_lines: Vec<crate::light3d::V3>,
    /// `lines_sig()` when the two buffers above were built. `u64::MAX` = never built.
    cached_lines_sig: u64,
    /// The cached plan lines, bucketed into world-space cells for view culling.
    ///
    /// `(min, max, start, end)` — the cell's XY bounds and the half-open range of
    /// `cached_plan_lines` holding its segments. The buffer is SORTED by cell when it is built, so
    /// each cell is one contiguous run and a visible cell is one `extend_from_slice`.
    ///
    /// WHY THIS EXISTS. With the line cache in place the per-frame cost is the memcpy of the whole
    /// plan into the single list the renderer takes. Measured: 5.3 MB and 1.7 ms at 10k dobjects,
    /// 26.7 MB and 6.2 ms at 50k, and 106.8 MB and 24.8 ms at 200k — past the entire 16.7 ms frame
    /// budget, for a drawing that is not changing and mostly not on screen.
    ///
    /// Cells are part of the CACHED, geometry-keyed data. Which cells are copied is decided per
    /// frame from the camera, so the cache stays camera-independent and the version check keeps
    /// meaning what it meant.
    cached_plan_cells: Vec<(glam::Vec2, glam::Vec2, usize, usize)>,
    /// Cached per-hatch WORLD-space render geometry, keyed by dobject handle.
    /// Hatch generation (pattern scanline-clip, solid ear-clip) is EXPENSIVE;
    /// caching it means it runs once per doc change instead of every frame,
    /// which is what makes many/dense hatches usable. Colour is applied at
    /// draw time (not baked in), so selection/snap highlight still work.
    /// Invalidated wholesale ONLY on a GEOMETRY change — cleared in
    /// `ensure_index` when the spatial index rebuilds (`index_dirty`). It must
    /// NOT clear on a pure selection/highlight/camera change (`gpu_dirty`
    /// alone), or clicking/box-selecting a dense-hatch drawing re-generates
    /// every hatch and stutters (B24 / GP1). Hatch fill depends only on
    /// boundary-dobject geometry, which always sets `index_dirty`.
    hatch_cache: HashMap<cad_kernel::Handle, HatchCacheEntry>,

    // ---- Layer panel (Slice B) ----
    /// Is the layer dock open? Toggled from the top toolbar.
    layer_panel_open: bool,
    /// LayerId currently being renamed in the panel (click name to enter
    /// rename mode); None = no rename in progress.
    layer_rename: Option<LayerId>,
    /// Scratch buffer for the in-progress rename text.
    layer_rename_buf: String,
    /// True on the first frame after a rename was activated — the
    /// rename TextEdit calls `request_focus()` once to steal focus from
    /// the always-listen command line. Cleared after the focus grab so
    /// the user's clicks within the field aren't fighting us.
    layer_rename_focus_pending: bool,
    /// Counter for default layer names ("Layer1", "Layer2", …).
    layer_name_counter: u32,

    // ---- Pen palette (Slice C) ----
    /// Is the pen palette dock open? Toggled from the top toolbar.
    pen_panel_open: bool,

    // ---- Entity Info panel (Slice D) ----
    /// Is the entity-info dock open? Toggled from the top toolbar.
    info_panel_open: bool,

    // ---- Editing operations (Slice J) ----
    copy_state: CopyState,
    rotate_state: RotateState,
    /// Set by the `C` sub-command during a rotate session. When true,
    /// applying the rotation produces COPIES of the selected dobjects
    /// (originals untouched). Cleared when the session ends.
    rotate_copy: bool,
    /// Same toggle for scale's `C` sub-command. When true, scale commits
    /// a duplicated, scaled copy instead of mutating the selection.
    scale_copy: bool,
    scale_state: ScaleState,
    mirror_state: MirrorState,
    /// Snapshot-based undo stack — every editing operation pushes the
    /// pre-mutation state. `undo` pops and restores. Bounded so a
    /// long editing session doesn't grow without bound.
    undo_stack: Vec<UndoStep>,
    /// Companion redo stack. `undo` pops undo_stack and pushes current
    /// state to redo_stack. `redo` does the reverse. Any new editing op
    /// CLEARS redo_stack (new branch — can't redo onto a different
    /// history).
    redo_stack: Vec<UndoStep>,

    // ---- Slice K: matchprop click capture ----
    matchprops_state: MatchPropsState,
    // ---- Blocks ----
    block_def_state: BlockDefState,
    insert_state: InsertState,
    block_dialog: Option<BlockDialog>,
    /// Block dialog parked while a round-trip runs (Select objects → canvas
    /// selection, or Pick point → one canvas click). Restored when the
    /// round-trip finishes so the user lands back in the dialog.
    block_dialog_stash: Option<BlockDialog>,
    /// Armed by the dialog's "Pick ⊕" button — the next canvas click is
    /// captured as the block's insertion point and the dialog reopens.
    block_dialog_pick_base: bool,
    /// The **Insert Block** dialog (bare `insert`). `None` when closed.
    insert_dialog: Option<InsertDialog>,
    /// Armed by the Insert dialog's "Pick ⊕" — next canvas click sets the
    /// insertion point and reopens the dialog.
    insert_dialog_pick: bool,
    /// Set when the Insert dialog is committed — the next canvas click places
    /// this configured block at the clicked insertion point (with live shade).
    pending_insert: Option<PendingInsert>,
    /// Insert-dialog "↔" distance pick for a smart parameter: (param index,
    /// FIRST point once captured). Two clicks → their distance = the value.
    insert_param_pick: Option<(usize, Option<Vec2>)>,
    /// LIVE parametric insertion — base fixed, dragging sets each parameter with
    /// a live-deforming preview. `None` unless mid-insert of a smart block.
    insert_live: Option<InsertLive>,
    /// Live cursor in world coords (RAW, before constraints), refreshed
    /// every canvas frame. Lets the command line's direct-distance entry
    /// know which direction to throw a typed distance.
    last_cursor_raw_world: Option<Vec2>,
    /// Armed when a press-fires-click captured a press; the MATCHING release
    /// is then swallowed. Without it, a press-fires-click edit op that ENDS
    /// on the press (e.g. stretch destination, move/copy dest, fillet 2nd
    /// pick) leaves click-only mode, so the release of that same physical
    /// click gets re-classified as a pointer-mode click and spuriously
    /// selects whatever sits under the cursor.
    pending_release_swallow: bool,
    /// The crossing window box captured during a STRETCH selection (last
    /// crossing window). `apply_stretch` tests vertices against it. `None`
    /// → fall back to the selection's bbox (whole-object move).
    stretch_window_box: Option<(Vec2, Vec2)>,
    /// In-app file browser (Open / Save As). `None` when closed.
    file_dialog: Option<FileDialog>,
    /// A pointer gesture on the plan that belongs to the SIMLUX lighting layout (placing a point,
    /// or dragging a fixture). Latched from press to release, because the drafting canvas decides
    /// what a click meant at RELEASE time: without the latch, dropping a light point on press
    /// would still let the release select whatever entity was underneath.
    light_gesture: bool,
    /// Imported raster awaiting the raster→vector editor (see `cad_raster`).
    raster_doc: Option<cad_raster::RasterDoc>,
    /// The open raster→vector editor (examine + adjust). `Some` after import.
    raster_editor: Option<RasterEditor>,
    /// GPU texture cache for the document's raster underlays
    /// (`doc.raster_images`), index-aligned. Rebuilt by `sync_underlay_textures`
    /// whenever `underlay_sig` (a cheap signature of the image set) changes.
    underlay_tex: Vec<egui::TextureHandle>,
    underlay_sig: u64,
    /// Parametric-sketch session (constraints, solver, DOF). See `param_editor`.
    parametric: crate::param_editor::ParamSession,
    /// Directory the file browser last sat in, so reopening lands there.
    file_dialog_dir: Option<std::path::PathBuf>,
    /// Parsed preview of the file currently selected in the Open dialog.
    /// Rebuilt only when the selection changes; rendered as a fit-to-rect
    /// wireframe in the dialog's preview pane.
    file_preview: Option<FilePreview>,
    /// Path of the currently open/saved drawing — set on successful open/save.
    /// `Save` writes here directly; `None` falls back to Save As.
    current_file: Option<std::path::PathBuf>,
    /// Where the 3D project's extra data goes on save: separate `.simlux.json`
    /// files beside the drawing (the historic behaviour) or embedded INSIDE the
    /// `.rsm`/`.dxf` (RSM v201 section / DXF XRECORDs).
    ///
    /// FOLLOWS THE FILE'S MODE: an open that loaded its project from the file
    /// sets [`ExtraDataStore::Embedded`], one that used a sidecar sets
    /// [`ExtraDataStore::Sidecar`] — so plain Save and autosave keep writing
    /// the way the file was stored, and only the Save-As dialog changes it.
    /// A `.dwg` is always sidecar (the DWG path cannot carry the payload
    /// reliably); saving a DWG forces this back to Sidecar.
    extra_store: crate::simlux_io::ExtraDataStore,
    /// A just-opened drawing carried BOTH an embedded 3D project and a
    /// `.simlux.json` beside it — `Some` while the choose-one dialog is up.
    /// See [`PendingExtraChoice`] and [`Self::render_extra_choice_dialog`].
    extra_choice: Option<Box<PendingExtraChoice>>,
    /// A background save carrying a fresh CALCULATION into an embedded-mode
    /// drawing (the embedded analogue of `save_light_results`) — its result
    /// channel while one is running, plus a re-try flag for when the app was
    /// busy (modal save / autosave) at the moment the calculation finished.
    results_rx: Option<std::sync::mpsc::Receiver<BusyMsg>>,
    results_due: bool,
    /// True when the drawing has edits not yet written to disk. Set on every snapshot (an edit),
    /// cleared on open/save. Drives the close-confirmation prompt so an accidental window-close
    /// can't silently discard work.
    unsaved: bool,
    /// While `true`, the "You have unsaved changes" modal is showing (a window/menu close was
    /// vetoed and is waiting on the user's Save / Don't Save / Cancel choice).
    close_confirm: bool,
    /// Architecture generator modal (staircase / spiral / ramp): open flag, active tab, and the
    /// live parameter sets. Kept on the app so edits persist while the dialog is open.
    arch_modal_open: bool,
    /// The ☀ Sun / daylight settings window is open.
    sun_modal_open: bool,
    /// The 🎨 Materials Factory (node-based material editor) window is open.
    materials_open: bool,
    /// Materials Factory authoring state (per-material node graphs + canvas interaction).
    materials: MaterialsFactoryState,
    /// The ⏺ Render (path tracer) window is open.
    render_modal_open: bool,
    /// Is the 2D CAD canvas shown?
    ///
    /// The three views — 2D CAD, 3D Factory, SIMLUX — are peers: "make sure the windows of 2d cad
    /// 3d factory and simlux can be extended to which ever length the user prefers and they can
    /// close any of them to have any of the single window open". The 2D canvas is egui's
    /// CentralPanel, which always exists and always takes the remainder, so "closed" here means it
    /// reserves no width — leaving the whole window to whichever panels are open.
    ///
    /// Never all three off; see [`CadApp::keep_one_view_open`].
    two_d_open: bool,
    /// The Help ▸ Keyboard shortcuts window.
    shortcuts_open: bool,
    /// Was the SIMLUX workspace split on LAST frame? Only the transition matters: the panel's
    /// width is forced to half the window on the frame the split is entered, and left alone (drag
    /// handle and all) every frame after.
    simlux_split_prev: bool,
    /// The running/finished path-trace job, its live preview texture, and the last pass uploaded.
    pt_job: Option<crate::pathtrace::RenderJob>,
    /// The GPU render job (fragment-shader tracer), stepped from the UI thread each frame.
    pt_gpu: Option<crate::pathtrace_gpu::GpuTracer>,
    /// The app's glow context, captured from eframe each frame — the GPU tracer renders on it.
    pt_gl: Option<StdArc<eframe::glow::Context>>,
    pt_preview: Option<egui::TextureHandle>,
    pt_last_pass: u32,
    /// Radiance render size choice (same table as the path tracer's `pt_res`).
    rad_res: u32,
    /// When the folder-picker confirms: run the pipeline (true) or export only (false).
    rad_run_after_pick: bool,
    /// A background Radiance pipeline run + its result window state.
    rad_job: Option<StdArc<Mutex<RadState>>>,
    rad_started: Option<std::time::Instant>,
    rad_preview: Option<egui::TextureHandle>,
    rad_loaded: bool,
    /// Render settings chosen in the dialog.
    pt_device: crate::pathtrace::Device,
    pt_res: u32,
    pt_passes: u32,
    /// Run the à-trous denoiser on the CPU tracer's preview and saved PNG.
    pt_denoise: bool,
    /// Which material a pending folder pick is loading a PBR texture set into (the folder dialog is
    /// modal and asynchronous, so the target has to outlive the click that started it).
    texset_target: Option<usize>,
    /// The Light Editor asked for the folder picker, so the PickFolder result is its photometry
    /// folder rather than a Radiance output or a texture set. One picker, three callers.
    photometry_wants_folder: bool,
    /// The next Open pick is a BLOCK LIBRARY for Illuminaire, not a project to open.
    illuminaire_wants_blocks: bool,
    // ---- report ----
    report_open: bool,
    report_opts: crate::report::Options,
    report_page: usize,
    /// A preview texture per report image, parallel to `report_opts.images`.
    report_tex: Vec<Option<egui::TextureHandle>>,
    /// An image was added without a `Context` to make its texture from — the next frame has one.
    report_tex_dirty: bool,
    /// The next folder pick is the report's output directory.
    report_wants_dir: bool,
    /// The next image pick is a report image: `Some(false)` a render, `Some(true)` a logo.
    report_wants_images: Option<crate::report::ImageSlot>,
    // ---- the calculation, off the UI thread ----
    calc_rx: Option<std::sync::mpsc::Receiver<crate::light::CalcOutcome>>,
    calc_progress: Option<std::sync::Arc<crate::light::CalcProgress>>,
    /// Which room the panel will show when the answer arrives — read when the job STARTED, since
    /// the selection can change while it runs.
    calc_selected: Option<usize>,
    calc_started: Option<std::time::Instant>,
    /// The symbol each dragged fixture is carrying — `(fixture id, dobject index)`.
    ///
    /// Paired at the PRESS, while the marker and its block are still in the same place, and held
    /// for the gesture. See `begin_fixture_drag`: the link between a fixture and its symbol is by
    /// position, so it can only be established before the fixture moves.
    light_drag_symbols: Vec<(u32, usize)>,
    /// The `edit_seq` the fixtures were last re-read from their symbols at.
    symbol_sync_seq: u64,
    /// The saved report settings have been read, and their logo images with them.
    report_prefs_loaded: bool,
    /// THE LAID-OUT DOCUMENT THE PREVIEW IS PAINTING, kept between frames.
    ///
    /// Laying it out is 123 ms on a real three-room plan in a release build, and it was being done
    /// on every frame the dialog was open — reported as "the app starts lagging once the report
    /// window is open". The preview IS the document and that is the right design; rebuilding it
    /// sixty times a second when nothing has moved is not part of it.
    report_doc: Option<crate::report::pdf::Doc>,
    /// What that document was built from — see [`crate::report::layout::Input::preview_key`].
    report_doc_key: Option<u64>,
    /// A preview texture per LOGO, parallel to `report_opts.logos`.
    report_logo_tex: Vec<Option<egui::TextureHandle>>,
    /// A preview texture per COVER picture, parallel to `report_opts.covers`.
    report_cover_tex: Vec<Option<egui::TextureHandle>>,
    /// Memory the undo history may hold, in bytes. Defaults to [`UNDO_BUDGET_BYTES`].
    ///
    /// A FIELD rather than the bare constant, for two reasons. It is the kind of limit a user with
    /// 8 GB and a user with 128 GB should be able to set differently; and a floor that only engages
    /// when one snapshot exceeds the whole budget is otherwise untestable without allocating half a
    /// gigabyte in a unit test.
    undo_budget_bytes: usize,
    /// True while the file picker was opened by ▼ FBC scene import rather than ▼ Furniture — the
    /// two share one dialog but place their result very differently.
    scene_import_pending: bool,
    /// One-shot: on the first frame, auto-load the villa test model (opt-in — see the autoload
    /// site for why it is no longer on by default).
    villa_autoloaded: bool,
    /// `SIMLUX_REPAIR=<drawing>` — open that file and run `repaircuts` + `diag` on it, once.
    ///
    /// 0 = not looked at · 1 = open issued, waiting · 2 = finished · 3 = not requested.
    ///
    /// This exists because the repair was run twice against a STALE BUILD without anyone
    /// noticing. The older binary keyed the repair on a signature that none of the failing cuts
    /// carried any more, so it correctly reported "no shallow cuts found" and did nothing — which
    /// reads exactly like success. Running it from the same binary that was just built removes
    /// that whole class of confusion.
    ///
    /// It deliberately does NOT save. Opening and repairing are recoverable; overwriting a 304 MB
    /// project from a startup flag is not, and this project has already lost work to an automatic
    /// write once.
    startup_repair: u8,
    arch_tab: ArchTab,
    arch_stair: cad_solid::architecture::StairParams,
    arch_spiral: cad_solid::architecture::SpiralParams,
    arch_ramp: cad_solid::architecture::RampParams,
    arch_dogleg: cad_solid::dogleg::DoglegInput,
    arch_dogleg_treads: bool,
    arch_spiral_csg: cad_solid::spiral::SpiralInput,
    arch_door: cad_solid::door::DoorInput,
    /// The swappable door-handle library, loaded once on first use, plus the decoded preview tiles
    /// and the chosen handle/finish. Empty when the asset folder is not on this machine.
    handle_lib: Option<crate::handles::HandleLibrary>,
    handle_tex: std::collections::HashMap<String, egui::TextureHandle>,
    handle_sel: String,
    handle_finish: String,
    /// Whether the Handles dialog is open (opened from the Door panel).
    handle_dialog: bool,
    /// The "look at it before you insert it" window for the parametric door.
    door_preview: DoorPreview,
    /// What the parametric door is made of, per component.
    door_mats: crate::door_mat::DoorMaterials,
    arch_helical: cad_solid::architecture::HelicalRampParams,
    arch_cupboard: cad_solid::cupboard::CupboardInput,
    arch_kitchen: cad_solid::kitchen::KitchenInput,
    arch_cabin: cad_solid::cabin::CabinInput,
    arch_sweep: cad_solid::sweeplight::SweepInput,
    arch_desk: cad_solid::desk::DeskInput,
    arch_couch: cad_solid::couch::CouchInput,
    /// Set when the user chose "Save and close": the async save is running, and [`Self::apply_saved`]
    /// will close the window once the bytes are on disk.
    close_after_save: bool,
    /// One-shot: send the real close command next frame (the point where `ctx` is available), with
    /// `unsaved` already cleared so the close isn't vetoed again.
    pending_close: bool,
    /// Autosave: silently re-save the CURRENT file in the background every few minutes when there
    /// are unsaved edits. Only ever writes to an already-saved `.dxf`/`.rsm`, never an untitled
    /// doc.
    ///
    /// **OFF at every start, and deliberately not remembered.** It is the only thing in the app
    /// that writes over a user's file with no user action at all, and it destroyed a project:
    /// undoing past an open left the app holding its empty startup state while still pointed at
    /// the real file, and three minutes later autosave committed that emptiness to disk. The
    /// undo bug is fixed, but the asymmetry stands — a bug reachable only through an explicit
    /// save is a bad afternoon, the same bug reachable through a timer is a lost project.
    ///
    /// So it starts off every session and the user turns it on when they want it. Persisting the
    /// choice would defeat the point: the whole risk is it being on when nobody is thinking
    /// about it.
    autosave_on: bool,
    /// The background autosave worker's result channel while one is running (`None` = idle).
    autosave_rx: Option<std::sync::mpsc::Receiver<BusyMsg>>,
    /// A SAVE THAT WROTE THE DRAWING AND NOT THE PROJECT, held until the user acknowledges it.
    ///
    /// The message already existed and went to the command history, which scrolls -- so two weeks
    /// of failed saves passed unseen while the app reported "saved". Anything that leaves work on
    /// disk in one file and nowhere else has to be in front of the user, not behind them.
    save_failure: Option<String>,
    /// When the last autosave fired — the interval timer.
    last_autosave: std::time::Instant,
    /// Monotonic edit counter, bumped on every snapshot (2D or 3D). An autosave records the value
    /// it captured; on completion it clears `unsaved` ONLY if the counter is unchanged — so an edit
    /// made WHILE the save ran is not mistaken for saved.
    edit_seq: u64,
    /// The `edit_seq` value captured when the running autosave was spawned.
    autosave_seq: u64,
    /// A pending load/save shown behind a modal progress overlay (see [`BusyOp`]).
    busy: Option<BusyOp>,
    /// Last measured open / save durations (ms), so the overlay's time estimate self-calibrates.
    last_load_ms: u64,
    last_save_ms: u64,
    /// Clipboard for Copy / Paste — clones of the copied dobjects. Paste
    /// re-adds them with fresh handles (via `DObject::with_style`).
    clipboard_dobjects: Vec<DObject>,
    /// What one drawing unit was worth, in METRES, when those were copied.
    ///
    /// A face sketch is a document of its own measured in METRES while the plan may be in
    /// millimetres, and Ctrl+C / Ctrl+V crosses freely between them. Without this a 3 m wall
    /// copied off a millimetre plan — 3000 units — pastes into a sketch as a THREE KILOMETRE
    /// wall: no error, the view simply jumps and the drawing is somewhere past the horizon.
    clipboard_unit_m: f64,
    /// Active PASTE placement flow (base → destination); `Off` when idle.
    paste_state: PasteState,
    /// Active PEDIT (polyline edit) flow; `Off` when idle.
    pedit_state: PeditState,
    /// Object groups — each is a list of member dobject HANDLES (stable across
    /// reindexing). Selecting any member selects the whole group; Ungroup
    /// dissolves it. In-session only for now (not yet persisted to file).
    groups: Vec<Vec<u64>>,

    // ---- Slice L: medium editing actions ----
    offset_state: OffsetState,
    dist_state: DistState,
    /// AREA measurement: `Some` while an `area` session is live (click a
    /// closed object or accumulate a point polygon; `a`/`s` toggle
    /// add/subtract; Enter folds the polygon in; Esc exits). Pure
    /// inspection — never mutates the doc.
    area_state: Option<AreaState>,
    /// Pending XREF attach — awaiting the insertion-point click.
    xref_pending: Option<cad_kernel::Xref>,
    // ---- ATTDEF / ATTEDIT flows (tool = None; state-machine driven) ----
    attr_def_flow: AttrDefFlow,
    attedit_state: AttEditState,
    /// WBLOCK: the sub-document (selection / whole drawing) to write when
    /// the Save dialog confirms. See `save_dialog_purpose`.
    wblock_subdoc: Option<cad_kernel::Document>,
    /// What the current Save-mode file dialog is for: 0 = normal Save As /
    /// quick export, 1 = WBLOCK (so a cancelled wblock dialog can never
    /// hijack a later Save As).
    save_dialog_purpose: u8,
    // ---- XLINE / RAY / DONUT / WIPEOUT placement flows (tool = None) ----
    xline_state: XlineState,
    boundary_state: BoundaryState,
    centermark_state: CenterMarkState,
    ray_state: RayState,
    donut_state: DonutState,
    wipeout_state: WipeoutState,
    /// LAYISO / LAYFRZ / LAYOFF click-pick — the clicked dobject's layer
    /// is the target; `Off` when idle. LayOn is immediate (no pick).
    layer_pick: LayerPickState,
    /// DIVIDE / MEASURE — pick a curve, then type the segment COUNT
    /// (divide, equal parts) or the segment LENGTH (measure, stepped from
    /// the start). Marks are POINT dobjects; `Off` when idle.
    ptdist_state: PtDistribState,
    /// Selection cycling (pointer-mode Tab): every dobject within the pick
    /// aperture of the cursor, the current cycle index, and the screen
    /// cell the list was built for. `pick_cycle_at` = (len, index, world)
    /// of the highlighted candidate, honored by the next pointer click at
    /// the same spot.
    pick_cycle_index: usize,
    pick_cycle_cell: Option<(i32, i32)>,
    pick_cands: Vec<usize>,
    pick_cycle_at: Option<(usize, usize, Vec2)>,
    /// Properties dialog: true while a numeric/text field edit GESTURE is
    /// in progress, so the per-change `snapshot_doc` fires once at the
    /// start of a drag/type (one undo step per gesture, not per frame).
    props_edit_gesture: bool,
    /// Block Editor — `Some` while the isolated parametric-editing window is
    /// open. While set, the main canvas pointer interaction is GATED OFF so
    /// nothing bleeds into the main screen. See `BlockEditor`.
    block_editor: Option<BlockEditor>,
    /// Active prompt-driven command flow (AutoCAD-style prompt sequence).
    /// `Some` while a command like CIRCLE is collecting its inputs. See
    /// `CmdFlow` + COMMAND_LINE.md.
    cmd_flow: Option<CmdFlow>,
    /// Command-internal UNDO ("U" keyword) baselines, captured when a fresh
    /// top-level command begins. `cmd_undo_base` = undo-stack depth (so a
    /// command that snapshots each result — offset/trim/extend — can pop its
    /// own results without crossing into the prior command). `cmd_base_objs` =
    /// dobject count (so chained draws like LINE know which segments they
    /// added). See `command_internal_undo`.
    cmd_undo_base: Option<usize>,
    cmd_base_objs: Option<usize>,
    /// Command-line TRANSCRIPT — every (prompt, reply) the flow exchanges with
    /// the user. Kept on the command structure for review / replay / future AI.
    transcript: Vec<PromptReply>,
    /// Drafting-command live preview ON by default; `preview off` disables it.
    /// Part of the drafting-command architecture (every flow previews unless
    /// this is off). See `flow_preview`.
    draft_preview: bool,
    /// Text drafting state. While `WaitingForString`, the next cmd-line
    /// input is captured as the text body (NOT parsed as a command).
    text_draft: TextDraftState,
    /// Smart-dim drafting state. Drives the 2/3-click flow for the
    /// `dim` command; sub-kind auto-decided at the first click.
    dim_draft: DimDraftState,
    /// When `text "Hello"` is run inline, the string is captured here so
    /// the next click commits Text without re-prompting for the body.
    pending_text: Option<String>,
    /// When the user types `H` alone during the text tool, this flag
    /// arms; the NEXT cmd-line input is consumed as the height value
    /// (NOT passed to the main parser). Mirrors `fillet_waiting_radius`.
    text_waiting_height: bool,
    /// Wall tool `t`/`thickness` sub-option: a bare `t` arms this; the NEXT
    /// cmd-line number is consumed as the wall thickness (persisted to
    /// `WlThk`). Mirrors `text_waiting_height`.
    wall_waiting_thickness: bool,
    /// Text Style dialog state — Add / Edit a TextStyle entry. None
    /// while the dialog is closed; Some when it's open and editing
    /// the given style id (or `None` inside to indicate "new style").
    text_style_dialog: Option<TextStyleDialog>,
    dim_style_dialog: Option<DimStyleDialog>,
    /// Dimension Style Manager (the `dimstyle` page) — list + preview +
    /// Set Current / New / Modify buttons. The `dim_style_dialog` above
    /// is the New/Edit sub-form it launches.
    dim_style_manager_open: bool,
    /// Style id highlighted in the manager's Styles list (drives the
    /// preview + Modify/Set-Current targets). Not necessarily current.
    dim_style_manager_sel: u32,
    /// The "current" dim style — new dims are created with this id.
    /// Mirrors AutoCAD's DIMSTYLE current. Defaults to STANDARD (0).
    current_dim_style: u32,
    /// The "current" wall style — new walls are created with this id, and
    /// Set Current in the Wall Style Manager updates it (+ syncs WlThk).
    current_wall_style: u32,
    /// Wall Style Manager (the `wallstyle` page) open flag + selected id;
    /// `wall_style_dialog` is the New/Edit sub-form it launches.
    wall_style_manager_open: bool,
    wall_style_manager_sel: u32,
    wall_style_dialog: Option<WallStyleDialog>,
    /// APP-LAYER wall-style extension: wall-style id → centerline linetype id. Kept here
    /// (not in cad_kernel's `WallStyle`) so core stays byte-identical to RUST_CAD. A style
    /// absent from the map draws no centerline (faces only). Sidecar-persisted.
    wall_centerline_ltype: std::collections::HashMap<u32, u32>,
    /// Text input popup — drives the discoverable Enter-Text dialog
    /// that opens at the click anchor when the user picks a text
    /// position. Without this the body capture happens silently in
    /// the cmd line; new users can't find it.
    text_input_dialog_open: bool,
    text_input_dialog_buf: String,
    text_input_dialog_anchor: Option<Vec2>,
    text_input_dialog_focus: bool,
    text_input_dialog_height: f64,
    text_input_dialog_style_id: u32,
    /// Style-table length captured right before the "+ New…" button
    /// launches the TextStyleDialog. If the count grows by the next
    /// frame, the dialog committed a new style — auto-select it in
    /// the text input dialog. Sentinel `usize::MAX` = not armed.
    text_input_dialog_style_count_before: usize,
    /// Dockable Text editor dialog (ported from upstream RUST-AutoRASM) — the
    /// Inspector-style panel that replaces the old floating "Enter text" popup.
    /// Reuses the `text_input_dialog_*` fields above for anchor / body / style /
    /// height / auto-select machinery; these three carry the extra dialog state.
    text_dialog_dock_state: crate::dock::DockState,
    /// Oblique (italic shear) in DEGREES — mirrors the selected style's
    /// `TextStyle.oblique` (radians); edits write back to that style.
    text_dialog_oblique_deg: f64,
    /// Rotation angle (degrees) for NEWLY placed text — captured into each
    /// Text's `angle`. Replaces the old oblique box (italic is now a toggle).
    text_dialog_angle_deg: f64,
    /// Horizontal alignment for placed text (Parameter → align toggles).
    text_dialog_halign: cad_kernel::TextHAlign,
    /// Paragraph list mode (§3 PARAGRAPH/TOOLS): bulleted / numbered / none.
    text_list_mode: cad_kernel::TextListKind,
    /// Font-picker (properties panel) keyboard state: the arrow/type-ahead
    /// highlighted row + the font armed when the popup opened. Live: the
    /// highlighted font is applied to the style each frame so the ghost updates.
    font_picker_highlight: usize,
    /// The font the picker opened on — restored when Esc / click-out cancels.
    font_picker_orig: Option<String>,
    /// Text engine (cad_text) — real TTF fonts for the text dialog's preview
    /// swatch + the placed-text ghost. Lazily loads fonts on first use.
    font_manager: std::cell::RefCell<cad_text::FontManager>,
    /// `text_waiting_angle` — armed by a bare `angle`/`a` during the text
    /// anchor step: the next input IS the rotation angle in degrees.
    text_waiting_angle: bool,
    /// WP-SCRIPT Python engine (worker thread + embedded CPython). Lazily
    /// created on the first `py`/`pyfile` so a session that never scripts pays
    /// nothing. `cad_app` only ever touches the plain-Rust facade.
    script: Option<cad_script::ScriptEngine>,
    /// WP-SCRIPT slice 4 — docked Python REPL console panel.
    py_console_open: bool,
    /// Console output log (prints, values, tracebacks, submitted lines).
    /// Capped so a runaway print loop can't grow memory (rule 11).
    py_console_log: Vec<String>,
    /// The console input line.
    py_console_input: String,
    /// File name for "Save as script" (scripts/<name>.py — slice 5).
    py_console_name: String,
    /// Submitted lines for ↑/↓ recall.
    py_console_hist: Vec<String>,
    /// Current position in the recall list (None = editing a fresh line).
    py_console_hist_idx: Option<usize>,
    /// Console dock state (Bottom by default, floatable).
    py_console_dock: crate::dock::DockState,
    /// Example scripts found in `scripts/*.py` (name, path) — scanned when the
    /// console opens and on demand via the refresh button.
    py_examples: Vec<(String, std::path::PathBuf)>,
    /// Selected example index in the console's combo.
    py_example_sel: usize,
    /// WP-SCRIPT slice 5 — in-app script editor (create/edit/save/run
    /// scripts/*.py). The buffer lives in the app so closing the panel loses
    /// nothing; only Save writes to disk.
    py_editor_open: bool,
    /// Editor text buffer (the script being edited).
    py_editor_text: String,
    /// The file the buffer was loaded from / saved to (None = untitled).
    py_editor_path: Option<std::path::PathBuf>,
    /// Script name field (file stem, without .py).
    py_editor_name: String,
    /// Buffer differs from the on-disk file.
    py_editor_dirty: bool,
    /// Pending "discard unsaved changes?" target when the user picks a
    /// different file (or New) while dirty.
    py_editor_confirm: Option<PyEditorConfirm>,
    /// Run-script parameter dialog (slice 5): `run <name>` bare or a menu
    /// pick opens it; named `run <name> k=v …` skips it.
    script_param_dialog: Option<ScriptParamDialog>,
    /// Armed pick for a script parameter (slice 5): `Some((name, kind))` —
    /// the next canvas click fills that parameter's value and clears this.
    script_param_pick: Option<(String, ScriptPickKind)>,
    /// Live ghost preview while the parameter dialog is open (slice 5).
    script_preview: Option<ScriptPreview>,
    /// A `run <name> k=v …` invocation waiting on its meta reply (slice 5 —
    /// length conversion needs the declaration).
    script_pending_run: Option<PendingScriptRun>,
    /// The `pyhelp` reference window (slice 5): shows the full scripting
    /// API document (docs/scripting_api.md — the AI-agent reference).
    scripting_doc_open: bool,
    /// The document text, lazy-loaded once on first open.
    scripting_doc_text: Option<String>,
    /// A Meta reply arrived and its job's `Finished` is next in the poll
    /// stream — that finish must NOT finalize a preview the Meta reply just
    /// started (both arrive in one poll batch). Consumed by
    /// `on_script_finished`.
    script_meta_finish_pending: bool,
    /// WP-SCRIPT D5 (one run = one undo unit): undo depth captured when the
    /// running script made its FIRST write. On `Finished` the per-op
    /// snapshots collapse back to `base + 1` — the single pre-run state —
    /// so one Ctrl+Z reverts the whole run. `None` = no script write so far.
    script_undo_base: Option<usize>,
    /// Explicit `rasm.undo_group()` boundary snapshots (indices into
    /// `undo_stack`), reset per run. At `Finished` a grouped run keeps the
    /// pre-run snapshot + one entry per boundary — each group = one undo unit.
    script_group_snapshots: Vec<usize>,
    // ---- Plot Style Table Editor (CTB color→pen) state ----
    plotstyle_open: bool,
    /// Screen rect of the Plot dialog's Edit-style pencil button + the Plot
    /// dialog's own rect — so the editor can dock next to the pencil (6px) and
    /// cap its height to the dialog.
    plotstyle_edit_anchor: Option<egui::Rect>,
    plot_dialog_rect: Option<egui::Rect>,
    /// The editor's last rendered rect — so its own sub-menus (ladder, .pst)
    /// open centred on it even after the user drags it (positioning §2b).
    plotstyle_editor_rect: Option<egui::Rect>,
    /// Open-transition trackers. `!prev` on a shown frame == "just opened this
    /// frame" → force the child to its parent's centre (a dragged parent carries
    /// its children); afterwards the child is freely draggable. Positioning §2b.
    prev_plotstyle_open: bool,
    prev_plot_ladder_open: bool,
    prev_plot_preview_open: bool,
    /// Open-counters. Bumped once per open → the child window gets a FRESH egui Id
    /// each open, so egui has no remembered position and `default_pos(parent)`
    /// always wins (a stale remembered spot can't hijack it); dragging still works
    /// because the Id is stable for the whole open. Positioning §2b.
    plotstyle_open_seq: u64,
    plot_ladder_open_seq: u64,
    plot_preview_open_seq: u64,
    /// Menu-stack returns (avoid menu-on-menu): reopen the Plot dialog when the
    /// editor closes; reopen the editor when a sub-dialog (ladder / .pst) closes.
    plotstyle_return_to_plot: bool,
    plotstyle_return_after_sub: bool,
    /// Screen rect of the editor's footer button row (anchor for sub-dialogs).
    plotstyle_footer_rect: Option<egui::Rect>,
    /// Selected ACI colors (1..=255) in the editor's left list. Multi-select:
    /// edits in the property panel apply to all of these.
    plotstyle_sel: Vec<u8>,
    /// Shift-range anchor for the color list.
    plotstyle_anchor: Option<u8>,
    /// Editor tab: 0 = General, 1 = Table View, 2 = Form View.
    plotstyle_tab: u8,
    /// Edit-Lineweights sub-dialog state.
    plotstyle_ladder_open: bool,
    /// Working copy of the ladder while the Edit-Lineweights dialog is open;
    /// committed to `table.lineweight_ladder` on OK.
    plotstyle_ladder_work: Vec<f32>,
    plotstyle_ladder_sel: Option<usize>,
    /// Display units in the Edit-Lineweights dialog (values are stored mm).
    plotstyle_ladder_inch: bool,
    /// When set, the shared file dialog is targeting a `.pst` plot-style table:
    /// Some(true) = Save As, Some(false) = Load. None = normal drawing I/O.
    plotstyle_pst_io: Option<bool>,
    /// The `.pst` file the Plot Style Table Editor is bound to when opened from
    /// the CTB menu (New/Edit CTB) — "Save changes" writes the table back to
    /// this path. None = the document's own table (Save As / Load .pst as before).
    plotstyle_edit_file: Option<std::path::PathBuf>,
    /// CTB menu → New CTB… — name-entry dialog state.
    new_ctb_open: bool,
    new_ctb_name: String,
    /// Cache of the saved CTB tables (name → table) so the layout print-preview
    /// applies each CTB's per-ACI colour rules without a per-frame file read.
    /// Rebuilt by `refresh_ctb_tables` when the on-disk fingerprint (name, len,
    /// mtime of every `.pst`) changes — covers in-app saves AND external edits.
    ctb_table_cache: std::collections::HashMap<String, cad_kernel::plotstyle::PlotStyleTable>,
    ctb_fingerprint: Vec<(String, u64, Option<std::time::SystemTime>)>,
    /// Layer-status glyph textures (on/off/frozen/…), rasterised once from the
    /// SVG snippets in `layer_glyphs.rs` and reused by every panel that blits
    /// them (`blit_layer_glyph`).
    layer_glyph_tex: std::collections::HashMap<&'static str, egui::TextureHandle>,

    // ---- Plot dialog ----
    plot_dialog_open: bool,
    // ---- QSELECT panel (filter selection) ----
    qselect: QSelectState,
    // ---- POINT style/size (PDMODE §6) ----
    /// Current point display style (PDMODE) stamped onto each new Point.
    current_point_style: u8,
    /// Current point size (PDSIZE): negative = % of view height, positive =
    /// drawing units, 0 = default. Default -5.0 = 5% of the viewport height.
    current_point_size: f32,
    /// PDMODE picker window (typed `pdmode` / `ddptype`).
    point_style_picker_open: bool,
    // ---- LAYWALK panel (preview layers in isolation) ----
    laywalk_open: bool,
    laywalk_restore: Option<Vec<(bool, bool)>>,
    laywalk_current: usize,
    layer_search: String,
    // ---- Drawing Units dialog (DDUNITS) ----
    units_dialog_open: bool,
    units_dialog_draft: cad_kernel::Units,
    units_dlg_n: f64,
    units_dlg_m: f64,
    /// Output format: "pdf" (default), "svg" or "png".
    plot_format: String,
    plot_suppress_viewer: bool,
    plot_pdf_path: String, // output path (extension per `plot_format`)
    plot_paper: cad_kernel::plotstyle::PaperSize,
    plot_landscape: bool,
    plot_area_kind: u8, // 0 = Extents, 1 = Window, 2 = Display
    plot_window: Option<(Vec2, Vec2)>,
    plot_offset_center: bool,
    plot_offset_x: f32,
    plot_offset_y: f32,
    plot_scale_fit: bool,   // true = Fit, false = 1:N
    plot_scale_n: f64,      // the N in 1:N (model units per paper mm)
    plot_scale_lw: bool,    // "Scale lineweights" — default OFF
    plot_with_styles: bool, // "Plot with plot styles" — default ON
    plot_object_lw: bool,   // "Plot object lineweights" — default ON
    plot_mono: bool,        // "Plot all black" — default OFF
    /// Window-area pick: 0 = idle, 1 = waiting first corner, 2 = waiting second.
    plot_win_pick: u8,
    plot_win_p1: Option<Vec2>,
    /// When set, the shared file dialog is choosing the output PDF path.
    plot_pdf_browse: bool,
    /// Paper-side scale/offset unit: false = mm (default), true = inch.
    plot_unit_inch: bool,
    /// When the Save dialog was opened by the Plot button (choose path → write
    /// → open the PDF), not by a standalone Browse.
    plot_run_after_save: bool,
    /// The full print-preview window (renders the real output + Confirm) is open.
    plot_preview_open: bool,
    /// Page Setup (dockable) dialog — model-space plot configuration.
    pagesetup_open: bool,
    pagesetup_dock_state: crate::dock::DockState,
    // ---- Model/Layout tabs (upstream parity) ----
    /// Paper-space selection (indices into the ACTIVE layout's `entities`).
    /// Model selections live in `self.selection`; cleared on every tab switch.
    layout_selection: Vec<usize>,
    /// Drag state for paper-space MOVE (world pos of the last drag frame).
    layout_move_last: Option<Vec2>,
    /// In-progress paper grip drag: (entity index, role, origin).
    layout_grip_drag: Option<(usize, cad_kernel::GripRole, Vec2)>,
    /// Model-space camera saved when the user switches INTO a layout and
    /// restored when they come back (each layout keeps its own camera in
    /// `Layout.camera`).
    saved_model_scale: Option<f32>,
    saved_model_offset: Option<egui::Vec2>,
    /// "New Layout" dialog state.
    show_new_layout_dialog: bool,
    new_layout_name: String,
    new_layout_paper: cad_kernel::plotstyle::PaperSize,
    new_layout_landscape: bool,
    new_layout_ctb: String,
    /// Layout PRINT-PREVIEW toggle. OFF (default) = viewports show the model in
    /// its true colors (identical to the model render). ON = apply the plot pen
    /// / CTB colors so the layout previews what will print.
    layout_print_preview: bool,
    /// A viewport-drawing flow is mid-flight (0 = none; 1 = first corner
    /// placed, waiting for the second).
    viewport_draw_state: u8,
    /// The layout the in-flight viewport-create flow belongs to (corners are
    /// paper mm of THAT layout; the scale dialog must not redirect the create
    /// onto whatever tab is active when "Create Viewport" is clicked).
    viewport_draw_li: Option<usize>,
    viewport_draw_p1: Option<Vec2>,
    viewport_draw_p2: Option<Vec2>,
    /// Selected viewport's scale dialog.
    viewport_scale_dialog_open: bool,
    viewport_scale_text: String,
    viewport_custom_n: String,
    viewport_custom_m: String,
    viewport_custom_unit: String,
    viewport_ctb_name: String,
    /// Double-clicked viewport → its edit dialog (layout index, viewport
    /// index) — bound to the LAYOUT it was opened from, so switching tabs can
    /// never redirect Apply onto a different space.
    vp_edit_dialog: Option<(usize, usize)>,
    /// Edit-dialog seed buffers (per-open init tracked by `vp_edit_init_for`).
    vp_edit_scale_text: String,
    vp_edit_ctb: String,
    vp_edit_init_for: Option<(usize, usize)>,
    /// Session Recorder — captures every user action / state transition
    /// / doc mutation when armed. OFF by default; user pushes Start in
    /// the Recorder window before a debug session. See
    /// `dbg_recorder.rs` for the data model.
    pub dbg: crate::dbg_recorder::DbgRecorder,
    pub dbg_window_open: bool,
    pub dbg_note_buf: String,
    /// Last-frame snapshot of every "watched" state field. The frame
    /// hook compares this to the current frame and emits a
    /// `StateChange` event for each field that differs — no need to
    /// wrap every assignment with a setter. Gives the recorder
    /// agent-inspector behaviour: it sees EVERY state transition
    /// automatically, including ones we haven't yet identified.
    dbg_last_watched: Option<crate::dbg_recorder::WatchedState>,
    /// Last press position + selection snapshot, captured on every
    /// CanvasPress and read on the matching release to derive the
    /// GestureClassification. Without this the drag distance reads 0
    /// (egui clears press_origin by release time).
    dbg_press_pos: Option<(f32, f32, Vec2)>,
    dbg_press_hit: Option<usize>,
    dbg_press_sel: Vec<usize>,
    /// Captured at release; promoted to GestureClassification once the
    /// click handlers below have a chance to mutate selection / state.
    dbg_pending_gesture: Option<PendingGesture>,
    /// AutoCAD offset Erase mode (transient — resets per command).
    /// When true, the source dobject is deleted right after the offset
    /// completes. Toggle with `e`.
    offset_erase: bool,
    /// AutoCAD offset Layer mode: false = Current layer (default),
    /// true = Source layer. Toggle with `l` cycling.
    offset_layer_src: bool,
    /// Count of successful offsets since the last `Off`. Used by the
    /// in-command `u` undo to know whether anything is on the
    /// per-offset stack to pop (prevents popping prior unrelated state).
    offset_applied_count: u32,
    lengthen_state: LengthenState,
    break_state: BreakState,
    align_state: AlignState,
    stretch_state: StretchState,

    // ---- Slices M.3 / M.4: fillet / chamfer ----
    fillet_state: FilletState,
    /// AutoCAD "Multiple" mode for Fillet — when true, completing one
    /// fillet re-enters WaitingForFirst instead of returning to Off.
    /// Esc exits. Transient (not a SYSVAR — matches AutoCAD's behavior
    /// where each F command starts in single mode unless you type `m`).
    fillet_multiple: bool,
    /// True when fillet is waiting for the user to type a radius
    /// value (after they typed `r` alone with no arg). The very next
    /// non-empty cmd-line input is consumed as a number — NOT passed
    /// to the main parser. Cleared on success or Esc.
    fillet_waiting_radius: bool,
    /// Polyline mode (the AutoCAD `P` option). When ON the next picked
    /// object must be a polyline; the fillet is applied to EVERY corner.
    /// Toggled with `p`; cleared when fillet exits.
    fillet_poly_all: bool,
    chamfer_state: ChamferState,
    /// Same as `fillet_multiple` for Chamfer.
    chamfer_multiple: bool,
    /// Polyline mode for Chamfer (the `P` option). See `fillet_poly_all`.
    chamfer_poly_all: bool,
    /// Chamfer's two-step distance entry after `d` alone (AutoCAD
    /// style): Off → WaitingD1 → WaitingD2(d1) → back to WaitingForFirst.
    /// At each step, empty input keeps the current value. Inline
    /// `d 2 3` / `d 2,3` / `d1=2 d2=3` bypasses both steps.
    chamfer_dist_wait: ChamferDistWait,

    // ---- Slice M.1 / M.2: trim / extend (two-basket) ----
    trim_state: TrimState,
    extend_state: ExtendState,
    /// When a trim/extend session begins, the main `selection` is stashed
    /// here so the cutting-edge/boundary-edge select-session can reuse
    /// `self.selection` as its working basket without nuking the user's
    /// real selection. Restored on finalise/cancel.
    pre_op_selection: Vec<usize>,

    // ---- Trim debug log (instrumentation for diagnosis) ----
    /// Detailed click-by-click log of the active trim/extend session.
    /// Auto-cleared at each `trim`/`extend` command start. User opens
    /// the Trim Debug window, copies the log, pastes to repro reports.
    trim_debug_log: Vec<String>,
    trim_debug_open: bool,
    /// Hatch debug log — same shape as `trim_debug_log`. Records every
    /// state transition in the hatch flow (dialog open/buttons, pattern
    /// changes, pick-point search results, apply_hatch params, resolve
    /// loop counts, render kicks). Logged only when the window is open
    /// so the Vec doesn't grow forever in normal use.
    hatch_debug_log: Vec<String>,
    hatch_debug_open: bool,
    /// Frame counter for log timestamping — gives ordering even when
    /// multiple clicks happen close in wall-clock time.
    trim_debug_frame: u64,
}

/// What the script editor wants to switch to, once the user confirms
/// discarding unsaved changes (slice 5).
#[derive(Clone, Debug)]
pub enum PyEditorConfirm {
    /// Load an existing scripts/*.py file.
    File(std::path::PathBuf),
    /// Start a fresh untitled buffer.
    New,
}

/// What an armed script-parameter pick returns on the next canvas click.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScriptPickKind {
    /// `'point'` param — the world coordinate fills the value.
    Point,
    /// `'entity'` param — the nearest entity's index fills the value.
    Entity,
}

/// The run-script parameter dialog: one typed, named field per declared
/// input, prefilled from the script's defaults (slice 5).
pub struct ScriptParamDialog {
    /// Script stem (for history + the run).
    pub name: String,
    /// Resolved scripts/<name>.py path.
    pub path: std::path::PathBuf,
    /// The declaration (filled when the Meta reply arrives).
    pub params: Vec<cad_script::ScriptParamMeta>,
    /// Editable values, parallel to `params`.
    pub values: Vec<String>,
    /// True until the engine's Meta reply lands.
    pub waiting_meta: bool,
    /// Floating position (the header band drags the dialog, palette-style).
    pub pos: egui::Pos2,
}

/// A `run <name> k=v …` / positional invocation waiting on the script's
/// metadata — needed so LENGTH inputs convert through the document's
/// display unit before the run (slice 5).
pub struct PendingScriptRun {
    pub name: String,
    pub path: std::path::PathBuf,
    /// Named inputs (raw user strings) — `Some` = the k=v form.
    pub named: Option<Vec<(String, String)>>,
    /// Positional inputs (raw) — `Some` = the legacy form.
    pub positional: Option<Vec<String>>,
}

/// WP-SCRIPT slice 5 — the live script preview: a GHOST pass runs the script
/// on the worker while the parameter dialog is open; its ops land in a
/// shadow `Document` (never the real one — no undo, no history, no GPU
/// invalidation) and the net additions render as a dashed overlay.
pub struct ScriptPreview {
    /// Shadow document: a clone of the real doc taken at pass start.
    doc: Document,
    /// Real-document handles at pass start — a shadow dobject whose handle
    /// is NOT here is a script addition (robust to script deletes).
    base_handles: std::collections::HashSet<u64>,
    /// True while a ghost pass executes on the worker.
    running: bool,
    /// The user ran / closed the dialog mid-pass — drop on finish instead
    /// of finalizing ghosts.
    cancelled: bool,
    /// The dialog values changed since the pass started (restart pending).
    dirty: bool,
    /// Restart throttle.
    last_restart: std::time::Instant,
    /// Net additions of the last completed pass (rendered dashed).
    ghosts: Vec<Geom>,
}

const UNDO_STACK_CAP: usize = 64;
/// Command bar geometry (§7 height computation — COMMAND_BAR_MENTOR).
const CMD_PILL_H: f32 = 25.0; // input pill height
const CMD_PAD_TOP: f32 = 8.0; // history top margin + gap above the pill
const CMD_PAD_BELOW: f32 = 16.0; // gap from the pill bottom to the bar bottom
const CMD_HIST_LINES: usize = 3; // default fully-visible history lines

/// HOW MUCH MEMORY THE UNDO HISTORY MAY HOLD, in bytes.
///
/// The stack used to be capped at 64 STEPS, and a step is a whole document. Measured: 15.7 MB per
/// step at 100k dobjects — a gigabyte of history — and 240.2 MB per step at 1.5M, which is
/// **15.4 GB** at the cap. A count is simply the wrong unit: 64 steps of a 200-object sketch is
/// nothing at all, and 64 steps of a real plan is more memory than the machine has.
///
/// 512 MB is a judgement, and a generous one beside a 300 MB document: it keeps dozens of steps on
/// anything of ordinary size and a handful on the largest plans, which is the trade a user would
/// make if asked. It is not a promise about peak memory — the live document, the render buffers
/// and the CSG model all sit outside it.
const UNDO_BUDGET_BYTES: usize = 512 << 20;

/// Steps kept REGARDLESS of the budget, so Ctrl+Z always does something.
///
/// A single snapshot of a large enough document exceeds the budget on its own. Evicting on size
/// alone would then leave an empty stack and an undo that silently does nothing — which is worse
/// than the memory it saves, because the user cannot tell it from an edit that did not register.
const UNDO_MIN_STEPS: usize = 2;

/// Above this many selected objects, grips stop being drawn — AutoCAD's `GRIPOBJLIMIT`, whose
/// default is the same 100.
///
/// The grip pass is per-selected-object per-frame with no viewport cull, so a few thousand
/// selected objects cost 25–80 ms EVERY FRAME. Grips exist to drag ONE thing; nobody reaches for a
/// handle on the 900th object of a selection, and a canvas solid with squares is less usable, not
/// more. Selection itself is unaffected — only the handles.
const GRIP_OBJ_LIMIT: usize = 100;

/// The 3D Factory state an undo step restores. Model + alive walls only: the camera,
/// the selection and any open dialog are VIEW state, and yanking the camera back on
/// Ctrl+Z would be disorienting rather than helpful.
#[derive(Clone)]
pub struct FactorySnap {
    model: cad_solid::Model,
    walls: Vec<crate::factory::WallInst>,
    /// The storey stack rides along: adding or deleting a level MOVES geometry, so
    /// restoring the model without restoring the levels it was positioned against would
    /// leave the building and its stack disagreeing about what sits where.
    storeys: Vec<crate::factory::Storey>,
    active_storey: usize,
    ceilings: std::collections::HashSet<u32>,
    furniture: Vec<crate::factory::FurnitureInst>,
    feature_color: std::collections::HashMap<u32, [f32; 3]>,
    surface_color: std::collections::HashMap<crate::factory::SurfaceKey, [f32; 3]>,
    feature_texture: std::collections::HashMap<u32, usize>,
    surface_texture: std::collections::HashMap<crate::factory::SurfaceKey, usize>,
    /// Feature groups (`feature_id → group_id`) so Group / Explode are undoable.
    feature_group: std::collections::HashMap<u32, u32>,
    /// ROOM records. Undo restored the model but left the room list behind, so undoing a room
    /// removed its geometry and left a phantom entry in the Rooms menu — pointing at features that
    /// no longer existed, which is why renaming or re-heighting it then changed nothing.
    rooms: Vec<crate::factory::RoomInst>,
    next_room_id: u32,
    /// Per-texture (tiling, move, rotate, opacity, reflect) — the LIGHT parts, so those edits are
    /// undoable WITHOUT cloning every texture's pixels into every 3D-edit snapshot.
    texture_xforms: Vec<(f32, [f32; 2], f32, f32, f32)>,
}

/// One reversible step.
///
/// The 2D document and the 3D model are separate state, and a step records only the one
/// it is about to change — so a 2D edit never clones the building, and a 3D edit never
/// clones the drawing. That matters: `snapshot_doc` runs on all 44 editing paths, and a
/// combined snapshot would put a full CSG model into every one of them.
///
/// Both kinds share ONE chronological stack, because UNDO is UNDO (rule 4): Ctrl+Z
/// reverts the last thing you did, whichever viewport you did it in. Two parallel stacks
/// would force the user to know which history they were in.
#[derive(Clone)]
pub enum UndoStep {
    Doc(Document),
    Factory(FactorySnap),
    /// SIMLUX FIXTURES ONLY — a drag, a delete, a fitting assignment.
    ///
    /// The lights used to be off the stack entirely, so Delete on a marker was final and Undo
    /// stepped straight past it to an older drawing edit, taking the drawing with it.
    Light(LightSnap),
    /// A DRAWING EDIT AND A FIXTURE EDIT THAT ARE ONE ACT.
    ///
    /// Placing a fitting puts a block on the plan and a light at the same point; deleting one
    /// takes both away. An undo that took back half of that would leave the project in a state
    /// the user never made — a symbol with nothing behind it, or a marker with no symbol.
    ///
    /// A step carries ONLY what it changed, which is why this is a variant rather than a field on
    /// `Doc`. Were every doc step to carry the fixtures, a later light-only edit would make those
    /// copies stale, and undoing an unrelated line would silently rewind the lighting too.
    Both(Document, LightSnap),
}

/// The SIMLUX fixtures as of one moment, for the undo stack.
///
/// `next_id` is deliberately NOT here. Ids are minted once and never reused — a drawing that still
/// names a deleted fixture must resolve to nothing rather than to whatever took its place — so
/// rewinding the counter is the one thing an undo must not do.
#[derive(Clone)]
pub struct LightSnap {
    pub luminaires: Vec<cad_light::Luminaire>,
}

impl LightSnap {
    fn approx_bytes(&self) -> usize {
        // The profile NAME is the only heap in a `Luminaire`; the rest is a fixed struct.
        self.luminaires.capacity() * std::mem::size_of::<cad_light::Luminaire>()
            + self
                .luminaires
                .iter()
                .map(|l| l.profile.capacity())
                .sum::<usize>()
    }
}

impl UndoStep {
    /// Roughly how much memory this step holds. See [`UNDO_BUDGET_BYTES`].
    ///
    /// The Factory arm counts its feature list and the shared profile/path tables, which is where
    /// a 3D model's weight actually is; the side maps are per-feature and small beside them. As
    /// with `Document::approx_bytes`, a figure within a few percent computed cheaply is worth far
    /// more here than an exact one, because this runs on every edit.
    fn approx_bytes(&self) -> usize {
        match self {
            UndoStep::Doc(d) => d.approx_bytes(),
            UndoStep::Light(l) => l.approx_bytes(),
            UndoStep::Both(d, l) => d.approx_bytes() + l.approx_bytes(),
            UndoStep::Factory(f) => {
                let m = &f.model;
                std::mem::size_of::<FactorySnap>()
                    + m.features.capacity() * std::mem::size_of::<cad_solid::Feature>()
                    + m.profiles
                        .iter()
                        .map(|p| p.pts.capacity() * 8)
                        .sum::<usize>()
                    + m.paths.iter().map(|p| p.pts.capacity() * 12).sum::<usize>()
                    + f.walls.capacity() * std::mem::size_of::<crate::factory::WallInst>()
                    + f.furniture.capacity() * std::mem::size_of::<crate::factory::FurnitureInst>()
            }
        }
    }
}

/// Signed area of a closed path (shoelace). Sign gives winding; callers here want the
/// magnitude, to pick the largest ring.
fn polygon_area(p: &[glam::Vec2]) -> f32 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let q = p[(i + 1) % p.len()];
        a += p[i].x * q.y - q.x * p[i].y;
    }
    a * 0.5
}

/// Default slab thickness, metres. A floor slab is structure, not a surface — 0 would
/// give the light calc a zero-volume solid.
const SLAB_THICKNESS: f32 = 0.2;

/// Which 2D geometry can become a 3D wall.
///
/// ONE source of that answer, read by BOTH the right-click gate and the promotion
/// itself — otherwise the menu could offer "Make 3D wall" on a selection that promotion
/// then silently refuses.
///
/// Included: `Wall` (carries its own thickness) plus the bare centerlines an **imported
/// or traced plan** consists of — DXF has no wall entity, so a real floor plan arrives as
/// lines, polylines and arcs. Closed shapes (circle / ellipse) promote as rings.
///
/// Excluded: text, dimensions, hatches and points (nothing to extrude); block references, whose
/// contents are closed loops that would turn Make-building into "extrude the furniture"; and
/// splines, which `cad_solid::geom_outlines_scaled` still has no sampler for, so accepting one
/// here would promise a wall that could not be built.
///
/// The spline exclusion is now a CHOICE rather than an absence: `geom_display_outlines_scaled`
/// tessellates one for the viewport, so the geometry is available. Admitting curved splines to
/// the construction path is its own piece of work — a wall promoted from one has to decide how
/// its centreline is stored — and is not smuggled in behind a rendering change.
fn is_promotable_to_wall(g: &Geom) -> bool {
    matches!(
        g,
        Geom::Wall(_)
            | Geom::Line(_)
            | Geom::Polyline(_)
            | Geom::Arc(_)
            | Geom::Circle(_)
            | Geom::Ellipse(_)
            | Geom::EllipseArc(_)
    )
}

/// Active selection-gathering session. `ForList` dumps the chosen dobjects
/// to the command history when finalised; `ForSelect` just keeps them as
/// the current selection for follow-up commands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectMode {
    Off,
    ForList,
    ForSelect,
    /// Selecting cutting edges for `trim`. On Enter, the basket
    /// transfers to `TrimState::PickingTargets` and the user's main
    /// `selection` is restored from `pre_op_selection`.
    ForCuttingEdges,
    /// Selecting boundary edges for `extend`. Symmetric to ForCuttingEdges.
    ForBoundaryEdges,
}

/// Displacement-tool state machine. `WaitingForBase` is entered when the
/// user runs `move` with a non-empty selection, or when a `move`-queued
/// select session finalises. The next click sets the base point and
/// transitions to `WaitingForDest(base)`; the click after that commits the
/// translation `(dest - base)` to every selected dobject.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MoveState {
    Off,
    WaitingForBase,
    WaitingForDest(Vec2),
}

/// Command queued behind a selection session — finalising the session
/// transitions straight into the queued operation instead of just
/// "keeping" the selection. Lets commands like `move` work nested:
///   `move` → auto-enter select mode → user picks → Enter → base/dest clicks.
///
/// Extend this enum when adding copy / rotate / scale / mirror / trim …
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum QueuedOp {
    None,
    Move,
    Copy,
    Rotate,
    Scale,
    Mirror,
    /// Join — applied on Enter when the selection session ends. Drives the
    /// kernel's three-pass merge (collinear lines, concentric arcs, chain
    /// → polyline).
    Join,
    /// Hatch — applied on Enter. For every closed polyline in the
    /// finalised selection, append a Hatch dobject whose boundary copies
    /// that polyline's vertices, filled with the active color.
    Hatch,
    /// Array — applied on Enter. The finalised selection becomes the
    /// SOURCES for the array generation; the array dialog re-shows
    /// itself and the user adjusts rows/cols/dx/dy then clicks
    /// Generate. Multi-source: every grid cell instantiates a copy of
    /// every dobject in the selection (offset by cell position).
    Array,
    /// Align — on Enter the finalised selection becomes the align basket
    /// and the command proceeds to the 4-point capture (src1 → src2 →
    /// tgt1 → tgt2). Same select-first flow as Move/Copy/Rotate.
    Align,
    /// Block definition — on Enter the finalised selection becomes the
    /// block contents; the command proceeds to the base-point click.
    BlockDef(String),
    /// Block DIALOG re-open — the user clicked "Select objects" inside the
    /// Block dialog. On Enter the finalised selection is kept and the
    /// (stashed) dialog re-opens so they can finish defining the block.
    BlockReopen,
    /// Region — applied on Enter: closed curves in the finalised selection
    /// are converted to Region dobjects (one undo entry).
    Region,
    /// WBlock — applied on Enter: the finalised selection (or the whole
    /// drawing when empty) is written to its own file.
    WBlock,
    /// Stretch — the selection session (crossing window; Shift excludes)
    /// picks the objects; on Enter we capture the crossing box and proceed
    /// to the base / second-point clicks.
    Stretch,
    /// Explode — on Enter every selected BlockRef is replaced by
    /// transformed copies of its contents (one level).
    Explode,
    /// Overkill — on Enter the finalised selection is deduped; an empty
    /// basket means the WHOLE drawing (AutoCAD semantics).
    Overkill,
    /// Erase — applied on Enter. The finalised selection is deleted
    /// in one batch. AutoCAD's ERASE: type `erase`, pick targets,
    /// Enter to commit.
    Erase,
    /// ChProp — applied on Enter. The finalised selection has the
    /// captured (property, value) pair applied to every dobject:
    /// layer/color/linetype reassignment. Pair stored as Strings so
    /// the kernel layer-id / linetype-id lookups happen at apply time.
    ChProp(String, String),
    /// MatchProp — applied on Enter. After the SOURCE is clicked, the target
    /// phase runs as a normal selection session (so crossing/window/W/C/B/all
    /// all work); on Enter every selected dobject takes the source's
    /// properties. Carries the source dobject index.
    MatchPropPaint(usize),
    /// PEDIT Join — the user picked objects (lines/arcs/open-plines/splines)
    /// to merge into the target polyline; on Enter they're joined. Carries the
    /// target polyline's handle.
    PeditJoin(u64),
    /// PEDIT start — `pedit` was typed with nothing selected, so a selection
    /// session is open to pick the ONE object to edit (AutoCAD's "Select
    /// polyline" prompt). On Enter `pedit_start` runs again with the pick.
    PeditStart,
}

/// State machine for the interactive copy tool — same shape as MoveState.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CopyState {
    Off,
    WaitingForBase,
    WaitingForDest(Vec2),
}

/// State machine for PASTE (Edit ▸ Paste) — places the dobject clipboard via a
/// base→destination pick with a live ghost preview, exactly like COPY.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PasteState {
    Off,
    WaitingForBase,
    WaitingForDest(Vec2),
}

/// PEDIT (polyline edit) flow. The `u64` is the target polyline's HANDLE
/// (stable across reindexing; Join replaces it with the merged result).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PeditState {
    Off,
    /// Showing the [Close/Open/Join/Width/Undo/eXit] menu for this polyline.
    Menu(u64),
    /// Waiting for a width value to apply to the polyline.
    Width(u64),
}

/// State machine for the interactive rotate tool — AutoCAD ROTATE flow:
///   1. WaitingForPivot                        — click the pivot.
///   2. WaitingForAngle(pivot)                 — default: click → angle =
///      atan2(click − pivot); OR type a number in degrees; OR type `R`
///      to switch to reference mode; OR type `C` to toggle copy mode
///      (rotate produces a copy instead of moving the original).
///   3. Reference sub-states (3 picks total): RefSrc1 → RefSrc2 defines
///      the source direction (= atan2(s2 − s1)); RefTgt is ONE pick
///      anchored at the PIVOT (target direction = atan2(click − pivot)).
///      Matches scale's reference shape — after the 2 source picks,
///      the next click / typed value is measured from the pivot.
///      Rotation = target − source.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RotateState {
    Off,
    WaitingForPivot,
    WaitingForAngle(Vec2),         // pivot
    WaitingForRefSrc1(Vec2),       // pivot
    WaitingForRefSrc2(Vec2, Vec2), // pivot, src1
    WaitingForRefTgt(Vec2, f64),   // pivot, src_angle
}

/// State machine for the interactive scale tool — AutoCAD SCALE flow:
///   1. WaitingForPivot                       — click the base point.
///   2. WaitingForFactor(pivot)               — default: click → factor =
///      |click − pivot|; OR type a number directly; OR type `R` to switch
///      to reference mode; OR type `C` to toggle copy mode.
///   3. Reference sub-states (3 picks): RefStart → RefEnd → NewLength.
///      ref_d = |RefEnd − RefStart|; factor = NewLength / ref_d, where
///      NewLength is either |click − pivot| or a typed number.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ScaleState {
    Off,
    WaitingForPivot,
    WaitingForFactor(Vec2),         // pivot
    WaitingForRefStart(Vec2),       // pivot
    WaitingForRefEnd(Vec2, Vec2),   // pivot, ref_start
    WaitingForNewLength(Vec2, f64), // pivot, ref_d
}

/// State machine for the interactive mirror tool — two clicks define the
/// axis, then a keep-original prompt.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MirrorState {
    Off,
    WaitingForA,
    WaitingForB(Vec2),
    /// Axis fixed (a, b); waiting for the keep-original answer
    /// (Enter/Y = keep a copy, n/no/ni = erase the original).
    AwaitingKeep(Vec2, Vec2),
}

/// State machine for matchprop (AutoCAD MATCHPROP): click ONE source dobject;
/// the target phase then runs as a normal SELECTION SESSION (click / window /
/// crossing / W/C/B/all) with `QueuedOp::MatchPropPaint` queued, so on Enter
/// every selected dobject takes the source's general properties (layer / color
/// / linetype) plus the matching geom-embedded style (wall / dim / text).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MatchPropsState {
    Off,
    /// Click the source dobject.
    WaitingForSource,
}

/// `block <name>` — after the selection is confirmed, one click picks the
/// BASE point; the definition is stored and the selection is replaced by
/// a single instance (visual unchanged: insert == base).
#[derive(Clone, PartialEq, Debug)]
pub enum BlockDefState {
    Off,
    WaitingForBase { name: String },
}

/// `insert <name>` — one click places an instance of the named block.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum InsertState {
    Off,
    WaitingForPoint {
        block: u32,
    },
    /// Insertion point chosen; waiting for the ROTATION angle — click a
    /// direction point (angle = direction from `insert`), or Enter = 0.
    WaitingForAngle {
        block: u32,
        insert: Vec2,
    },
}

/// The **Insert Block** dialog — pick a defined block, set scale + rotation +
/// insertion point (typed X/Y or picked on screen), then place it. Opened by
/// bare `insert` (no name) or Draw ▸ Insert Block…
pub struct InsertDialog {
    /// Selected block definition id (index into `Document.blocks`).
    pub block: Option<u32>,
    pub scale: String,
    pub rotation: String,
    pub at_x: String,
    pub at_y: String,
    /// Parametric values for a SMART block: (param name, value text). Synced
    /// from the selected block's definition each frame; edited by the user.
    pub params: Vec<(String, String)>,
}

impl InsertDialog {
    fn new(block: Option<u32>) -> Self {
        InsertDialog {
            block,
            scale: "1.0".into(),
            rotation: "0".into(),
            at_x: "0.0000".into(),
            at_y: "0.0000".into(),
            params: Vec::new(),
        }
    }
    fn scale_f(&self, store: &crate::calc::CalcStore) -> Result<f64, String> {
        if let Ok(v) = self.scale.trim().parse::<f64>() {
            Ok(v)
        } else {
            crate::calc::eval(store, self.scale.trim()).map_err(|e| format!("calc: scale — {}", e))
        }
    }
    fn rot_rad(&self, store: &crate::calc::CalcStore) -> Result<f64, String> {
        let deg = if let Ok(v) = self.rotation.trim().parse::<f64>() {
            v
        } else {
            crate::calc::eval(store, self.rotation.trim())
                .map_err(|e| format!("calc: rotation — {}", e))?
        };
        Ok(deg.to_radians())
    }
    fn at(&self) -> Vec2 {
        Vec2::new(
            self.at_x.trim().parse().unwrap_or(0.0),
            self.at_y.trim().parse().unwrap_or(0.0),
        )
    }
}

/// A block configured in the Insert dialog, waiting for the user to CLICK its
/// insertion point on the canvas. Carries the dialog's scale / rotation /
/// parametric values so the click places the fully-configured instance. A block
/// is NEVER placed without a clicked insertion point (user rule 2026-07-09).
#[derive(Clone)]
pub struct PendingInsert {
    pub block: u32,
    pub scale: f64,
    pub rotation: f64,
    pub param_values: [f64; cad_kernel::MAX_BLOCK_PARAMS],
}

/// LIVE parametric insertion (AutoCAD dynamic-block feel): the base point is
/// fixed, then the user drags to set each smart parameter in turn — the block
/// deforms under the cursor and a click fixes that value. `values[idx]` is the
/// parameter currently being dragged.
#[derive(Clone)]
pub struct InsertLive {
    pub block: u32,
    pub insert: Vec2,
    pub scale: f64,
    pub rotation: f64,
    pub values: Vec<f64>,
    pub idx: usize,
}

/// Pick-two-blocks-on-screen flow for `blockdiff` / the dialog Compare
/// button. Click the BASE block, then the variant; runs the diff.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BlockDiffPick {
    Off,
    WaitingForA,
    WaitingForB(u32), // base block id already picked
}

/// Block dialog — opened by bare `block` (no name) and the Draw menu.
/// Collects everything needed to define a block: name, the contents
/// (Select objects round-trips to the canvas), insertion/base point
/// (typed X/Y or picked), instance color (ACI wheel), and the smart-block
/// flag. A live preview shows the current selection. Also lists existing
/// blocks with one-click Insert.
pub struct BlockDialog {
    pub name: String,
    /// Insertion / base point, edited as text so partial input doesn't
    /// snap. Defaults to the selection's lower-left corner on open.
    pub base_x: String,
    pub base_y: String,
    /// Instance color. `None` = inherit the first selected dobject's color
    /// (legacy behavior); `Some(aci)` = explicit ACI for the instance.
    pub color_aci: Option<u8>,
    /// Smart-block marker — stored on the created `Block`. The re-derive
    /// algorithm is supplied later; today this only sets the flag + a badge.
    pub smart: bool,
}

impl BlockDialog {
    fn new(base: Vec2) -> Self {
        BlockDialog {
            name: String::new(),
            base_x: format!("{:.4}", base.x),
            base_y: format!("{:.4}", base.y),
            color_aci: None,
            smart: false,
        }
    }
    /// Parse the X/Y text fields into a world point. Each field may be an
    /// expression (`x*2`); on error the dialog stays open and reports it
    /// (a half-typed value must not silently become 0).
    fn base_point(&self, store: &crate::calc::CalcStore) -> Result<Vec2, String> {
        let x = if let Ok(v) = self.base_x.trim().parse::<f64>() {
            v
        } else {
            crate::calc::eval(store, self.base_x.trim()).map_err(|e| format!("calc: X — {}", e))?
        };
        let y = if let Ok(v) = self.base_y.trim().parse::<f64>() {
            v
        } else {
            crate::calc::eval(store, self.base_y.trim()).map_err(|e| format!("calc: Y — {}", e))?
        };
        Ok(Vec2::new(x, y))
    }
}

/// Highlight overlay produced by `blockdiff`: the parametric (moved)
/// points, drawn on every placed instance of the two compared blocks so
/// the user SEES which points are parametric. Points are base-relative
/// (block-definition space); the renderer maps them through each
/// instance's transform.
pub struct BlockDiffOverlay {
    /// BASE block id — the highlight draws on this block's instances (the
    /// moved points are base-relative to it).
    pub block_a: u32,
    /// One entry per STRONG cluster (candidate parameter): the cluster's
    /// base-relative before-points and its displacement direction (for the
    /// arrow). Drawn in a per-cluster colour.
    pub clusters: Vec<(Vec<Vec2>, Vec2)>,
}

/// One extracted modifier vector (block-diff cluster) awaiting a name +
/// value mapping, shown as a row in the "Set parameters on block" dialog.
pub struct ParamRow {
    pub win_min: Vec2,
    pub win_max: Vec2,
    pub dir: Vec2,
    pub magnitude: f64,
    pub dx: f64,
    pub dy: f64,
    pub points: Vec<Vec2>,
    /// Editable fields.
    pub name: String,
    /// The value the SOURCE block represents (displacement 0). Insert
    /// displacement = gain·(entered value − original) along `dir`.
    pub original: String,
    /// Per-vector gain (see `cad_kernel::ParamVector::gain`). 1.0 = full.
    pub gain: f64,
}

/// "Set parameters on block" dialog — opens after picking source + target
/// blocks. Each extracted modifier vector becomes a row; the user names it
/// (`width`, `thickness`, …) and sets its source/target values; Save writes
/// the params onto the SOURCE block, making it a parametric block.
pub struct ParamNameDialog {
    pub source_id: u32,
    pub source_name: String,
    pub target_name: String,
    pub rows: Vec<ParamRow>,
}

/// Self-contained **Block Editor** — an ISOLATED surface for turning a
/// block into a parametric block. It draws the block's geometry in its own
/// embedded canvas (own pan/zoom) and records STRETCH gestures as named
/// parameters, reading input ONLY from the editor canvas's egui response —
/// never the main canvas pipeline (selection / grips / stretch_state / DDE /
/// command terminators). That isolation is the whole point: it ends the
/// "mixing with the main screen" class of bugs. The window title is
/// "Block Editor"; the main canvas interaction is gated off while it's open.
pub struct BlockEditor {
    /// Block definition being parametrized.
    pub block_id: u32,
    pub name: String,
    /// Embedded-canvas view: the world point shown at the canvas centre, and
    /// px-per-world zoom. Independent of the main view.
    pub view_center: Vec2,
    pub view_scale: f64,
    /// First-open fit-to-view done? (also reset by the "Fit view" button)
    pub fitted: bool,
    /// Which half of the stretch gesture we're collecting.
    pub phase: EdPhase,
    /// World point where the current primary drag began (rubber-band anchor).
    pub drag_start: Option<Vec2>,
    /// Recorded parameter rows (BASE-RELATIVE windows — `save_block_params`
    /// re-adds the block base). Saved onto the block on Save. Rows sharing
    /// a `name` form ONE task (linked, correlated vectors).
    pub rows: Vec<ParamRow>,
    /// Auto-name counter (P1, P2, …) for freshly recorded rows.
    pub next_pid: u32,
    /// The ACTIVE TASK new stretches join — its name (e.g. `opening`) and the
    /// measured `original` value freshly recorded vectors inherit. Empty name
    /// → each new stretch gets an auto `P{n}` name (its own task).
    pub active_name: String,
    pub active_original: String,
    /// Default gain applied to freshly recorded vectors (1.0, or 0.5 for a
    /// symmetric/centered task the user is building).
    pub active_gain: f64,
    /// CARD (cardinal H/V) lock for THIS editor — constrains the displacement
    /// and the measure to the nearest axis. Local to the window (isolation);
    /// named CARD, never "ortho" (project convention).
    pub card: bool,
    /// Indices (into the block's `dobjects`) marked as OPENING CUT EDGES — the
    /// door/window jambs whose enclosed region trims host geometry on insert.
    pub cut_edges: Vec<usize>,
    /// While true, a canvas click toggles the nearest edge's cut-edge mark
    /// instead of doing a stretch gesture.
    pub mark_cut: bool,
}

/// Which half of a stretch demonstration the Block Editor is collecting.
#[derive(Clone, Copy, PartialEq)]
pub enum EdPhase {
    /// Drag a crossing WINDOW over the vertices that should move.
    Window,
    /// Window chosen; drag the DISPLACEMENT (base → destination).
    Displace { win_min: Vec2, win_max: Vec2 },
    /// Measuring the active task's ORIGINAL distance — drag/click two points;
    /// their distance becomes the baseline value.
    Measure,
}

/// Insert-time value prompt for a parametric block: after the insertion
/// point is clicked, the command line asks for each variable in turn
/// ("set width [1000]:"); on the last, the instance is placed.
pub struct InsertParamPrompt {
    pub block: u32,
    pub insert: Vec2,
    /// Rotation (radians) chosen at the insert ANGLE step, applied to the
    /// placed instance and its cut region.
    pub rotation: f64,
    pub names: Vec<String>,
    pub defaults: Vec<f64>,
    pub values: Vec<f64>, // collected so far (len = current index)
}

/// One recorded prompt↔reply pair — the command line's transcript. The whole
/// command structure keeps this so the exchange can be reviewed, replayed, or
/// later fed to an AI/external resolver (see COMMAND_LINE.md). Reply is the
/// canonical answer (a coordinate, number, or keyword), not the raw keystrokes.
#[derive(Clone, Debug)]
pub struct PromptReply {
    /// Command this exchange belongs to (e.g. "circle").
    pub cmd: &'static str,
    pub prompt: String,
    pub reply: String,
}

/// A live, prompt-driven command flow (AutoCAD-style): the command line shows
/// the current step's exact prompt; the user answers with a point / number /
/// keyword; each answer is recorded in `CadApp.transcript` and advances the
/// step. Slice 1 implements CIRCLE; the model generalises later.
pub struct CmdFlow {
    /// Command being run (e.g. "circle") — for the transcript + display.
    pub name: &'static str,
    /// Current step of the CIRCLE state machine.
    pub circle: CircleStep,
}

/// CIRCLE prompt-flow steps. Each carries the geometry captured so far. The
/// prompt text + valid keywords per step live in `flow_prompt`.
#[derive(Clone, Copy, Debug)]
pub enum CircleStep {
    /// "Specify center point for circle or [3P/2P/Ttr (tan tan radius)]:"
    Center,
    /// "Specify radius of circle or [Diameter]:"  (center captured)
    Radius(Vec2),
    /// "Specify diameter of circle:"
    Diameter(Vec2),
    /// 3-point: first / second / third point on the circle.
    P3a,
    P3b(Vec2),
    P3c(Vec2, Vec2),
    /// 2-point: first / second endpoint of the diameter.
    P2a,
    P2b(Vec2),
    /// Ttr (tangent-tangent-radius): pick first object, second object, then
    /// type the radius. Each pick stores (dobject index, click point) — the
    /// click point disambiguates which tangent solution to use.
    TtrObj1,
    TtrObj2(usize, Vec2),
    TtrRadius(usize, Vec2, usize, Vec2),
}

/// ZOOM command sub-option flow (AutoCAD-style). `zoom` / `z` enters `Menu`;
/// the user then types an option letter / `nX` scale / coordinate, or picks
/// point(s) on the canvas. Interactive point steps capture clicks through
/// `zoom_input_point`; `Object` reuses the normal pointer selection and applies
/// on Enter; `RealTime` consumes a primary drag to zoom. See the `zoom_*`
/// methods. Dynamic is intentionally not implemented (use Window / wheel).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ZoomState {
    Off,
    /// Top-level prompt: awaiting an option letter, an `nX` factor, a
    /// coordinate, or a canvas click (= first corner of an implicit Window).
    Menu,
    /// Center: awaiting the center point.
    CenterPoint,
    /// Center: center captured; awaiting magnification (`nX`) or height (`n`).
    CenterMag(Vec2),
    /// Window: awaiting the first corner.
    WinFirst,
    /// Window: first corner captured; awaiting the opposite corner.
    WinSecond(Vec2),
    /// Object: awaiting object selection, then Enter to fit them.
    ObjectSel,
    /// Real-time: primary drag UP = zoom in, DOWN = out; Enter / Esc exits.
    RealTime,
}

impl ZoomState {
    /// Steps where a canvas click supplies a POINT (handled by
    /// `zoom_input_point`). A click at the top-level `Menu` starts an implicit
    /// Window. `ObjectSel` is excluded — it uses the normal selection handler.
    fn wants_point(self) -> bool {
        matches!(
            self,
            ZoomState::Menu
                | ZoomState::CenterPoint
                | ZoomState::WinFirst
                | ZoomState::WinSecond(_)
        )
    }
}

/// Block Task Recorder session — the block is exploded into a TEMP sandbox
/// (tracked by handle); each stretch the user demonstrates is recorded as a
/// `ParamRow` (base-relative). On Finish the sandbox is deleted and the
/// recorded tasks are named in the "Set parameters on block" dialog, then
/// attached as parameters to the original block. Forward-recording =
/// reversible (each task IS the function), unlike the block-diff.
pub struct BlockTaskRec {
    pub source_block: u32,
    pub source_name: String,
    /// World offset to subtract from recorded world windows to get the
    /// base-relative ones (= the sandbox instance's insert point). `save_
    /// block_params` re-adds the block base for definition space.
    pub world_offset: Vec2,
    /// Handles of the temp sandbox dobjects (deleted on Finish/cancel).
    pub temp_handles: Vec<u64>,
    /// The block's own instances, REMOVED for the session so they don't
    /// overlap the sandbox (and get grabbed by the stretch window). Re-added
    /// on Finish/cancel.
    pub removed_instances: Vec<DObject>,
    /// Recorded tasks (one per demonstrated stretch).
    pub recorded: Vec<ParamRow>,
}

/// Open vs Save As for the in-app file browser.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileDialogMode {
    Open,
    Save,
    ImportImage,
    ImportRaster,
    ImportObj,
    ImportTexture,
    PickFolder,
    ImportHdri,
    ImportIes,
}

/// Pure-Rust file browser (no native-dialog dependency — `std::fs` only).
/// Lists directories + .dxf/.rsm files, lets the user navigate, type/pick
/// a name, and (for Save) choose the format. Confirm routes to
/// `do_open` / `do_save`.
pub struct FileDialog {
    pub mode: FileDialogMode,
    pub dir: std::path::PathBuf,
    pub filename: String,
    /// Editable text for the path bar. Decoupled from `dir` so typing doesn't
    /// re-navigate on every keystroke — committed via Enter / "Go" only. Kept
    /// in sync with `dir` whenever navigation happens (folder click, `..`).
    pub path_buf: String,
    /// Active file-type filter AND (in Save mode) the format extension —
    /// ".dxf" or ".rsm". The directory list shows only files matching it.
    pub ext: String,
    /// Save mode only — where the 3D project's extra data goes: inside the file
    /// (`.dxf`/`.rsm` only) or into separate `.simlux.json` files beside it.
    /// Seeded from the app's current mode on open; confirming commits the choice.
    pub embed_extra: bool,
    pub error: Option<String>,
}

impl FileDialog {
    fn new(mode: FileDialogMode, dir: std::path::PathBuf, ext: &str) -> Self {
        let path_buf = dir.to_string_lossy().to_string();
        FileDialog {
            mode,
            dir,
            filename: String::new(),
            path_buf,
            ext: ext.to_string(),
            embed_extra: false,
            error: None,
        }
    }
}

/// Cached, parsed preview of the file selected in the Open dialog. Built once
/// per selection by `update_file_preview`; `render_file_preview` draws `doc`
/// fit-to-rect each frame. `doc` is None when the file couldn't be parsed.
struct FilePreview {
    path: std::path::PathBuf,
    doc: Option<Document>,
    bbox: Option<(Vec2, Vec2)>,
    info: String,
}

/// Drive/root paths for the path-bar dropdown. On Windows, probe A:..Z: and
/// keep the roots that exist; on other platforms just the filesystem root.
fn list_drive_roots() -> Vec<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let mut v = Vec::new();
        for c in b'A'..=b'Z' {
            let root = format!("{}:\\", c as char);
            let p = std::path::PathBuf::from(&root);
            if p.exists() {
                v.push(p);
            }
        }
        if v.is_empty() {
            v.push(std::path::PathBuf::from("C:\\"));
        }
        v
    }
    #[cfg(not(windows))]
    {
        vec![std::path::PathBuf::from("/")]
    }
}

/// State machines for the Slice-L click-driven actions.
///
/// AutoCAD-style multi-phase offset command. The flow:
///   Off → (user types `offset`) → WaitingForObject(mode)
///       click object → WaitingForSide(mode, src_idx)
///       click side / through-point → apply, back to WaitingForObject
///       Enter or Esc → Off
/// `mode` carries either an explicit distance or "Through" semantics.
/// BOUNDARY/BPOLY click flow: click INSIDE a closed region; the hatch
/// tracer emits its boundary (outer loop + islands) as closed polylines.
/// Loops until Esc.
#[derive(Clone, PartialEq, Debug)]
pub enum BoundaryState {
    Off,
    WaitingForPick,
}

/// ATTDEF flow — the three typed text fields of a block attribute
/// definition, then a click (or typed x,y) places the slot.
#[derive(Clone, PartialEq, Debug)]
pub enum AttrDefFlow {
    Off,
    AwaitingTag,
    AwaitingPrompt {
        tag: String,
    },
    AwaitingDefault {
        tag: String,
        prompt: String,
    },
    AwaitingPosition {
        tag: String,
        prompt: String,
        default: String,
    },
}

/// ATTEDIT — pick a block instance, then edit its attribute values.
#[derive(Clone, PartialEq, Debug)]
pub enum AttEditState {
    Off,
    WaitingForPick,
    Editing { idx: usize, values: Vec<String> },
}

/// CENTERMARK click flow: click a circle/arc (sizes the mark) or any
/// point (default/override size). Place-multiple; Esc exits.
#[derive(Clone, PartialEq, Debug)]
pub enum CenterMarkState {
    Off,
    WaitingForClick { size_override: Option<f64> },
}

/// XLINE click flow: base point, then a direction point (or H/V/A/Off).
#[derive(Clone, PartialEq, Debug)]
pub enum XlineState {
    Off,
    WaitingForBase,
    WaitingForDir { base: Vec2 },
}

/// RAY click flow: base point, then a direction point (or H/V/A). The
/// ray extends forward from the base only.
#[derive(Clone, PartialEq, Debug)]
pub enum RayState {
    Off,
    WaitingForBase,
    WaitingForDir { base: Vec2 },
}

/// DONUT click flow: center → outer radius → inner radius; place-multiple
/// until Esc/Enter. Radii come from the click distances.
#[derive(Clone, PartialEq, Debug)]
pub enum DonutState {
    Off,
    WaitingCenter,
    WaitingOuter { center: Vec2 },
    WaitingInner { center: Vec2, outer_radius: f64 },
}

/// WIPEOUT click flow: two opposite corners define the opaque mask rect.
#[derive(Clone, PartialEq, Debug)]
pub enum WipeoutState {
    Off,
    WaitingFirstCorner,
    WaitingSecondCorner { first: Vec2 },
}

/// Type `t`/`e`/`l`/`u`/<number> at any waiting prompt to swap modes
/// or change distance; see the run_command intercept.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OffsetMode {
    Distance(f64),
    Through,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OffsetState {
    Off,
    WaitingForObject(OffsetMode),
    WaitingForSide(OffsetMode, usize),
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LengthenState {
    Off,
    WaitingForSide(f64),
}

/// Chamfer two-step distance prompt — AutoCAD CHAMFER `d` flow:
/// type `d`, then "first distance <X>:" (empty keeps X), then
/// "second distance <d1>:" (empty makes d2 = d1).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ChamferDistWait {
    Off,
    WaitingD1,
    WaitingD2(f64),
}

/// DIST measurement: click two points, get distance / dx / dy / angle
/// printed to the history. Pure inspection — never mutates the doc.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DistState {
    Off,
    WaitingForP1,
    WaitingForP2(Vec2),
}

/// AREA session state: `total` accumulates measured areas (sign ±1 in
/// add/subtract mode); `pts` is the point polygon being picked.
#[derive(Clone, PartialEq, Debug)]
pub struct AreaState {
    pub total: f64,
    pub sign: f64,
    pub pts: Vec<Vec2>,
}

/// Layer-pick commands — the next click chooses a dobject whose layer the
/// armed op targets. `Off` when idle.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LayerPickState {
    Off,
    Iso,
    Frz,
    OffLayer,
}

/// DIVIDE / MEASURE — place POINT dobjects along a curve. Pick the object
/// (Object phases), then type a segment COUNT (divide → equal parts) or a
/// spacing DISTANCE (measure → stepped from the start end). The usize
/// holds the picked object index.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PtDistribState {
    Off,
    DivideObject,
    DivideValue(usize),
    MeasureObject,
    MeasureValue(usize),
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BreakState {
    Off,
    WaitingForPoint,
}

/// Text drafting flow:
///   Off                          — tool inactive
///   WaitingForPosition           — user clicks; anchor captured
///   WaitingForString(anchor)     — user types the string in the cmd
///                                  line; Enter commits the Text dobject
#[derive(Clone, PartialEq, Debug)]
pub enum TextDraftState {
    Off,
    WaitingForPosition,
    WaitingForString(Vec2),
}

/// Drafting state for the smart `dim` command. Single-tool flow that
/// auto-decides the sub-kind based on what the first click hits:
///
///   Off                                   — tool inactive
///   WaitingForP1                          — first click; circle/arc → radius;
///                                           point → linear, store as p1
///   WaitingForP2 { p1 }                   — linear only; click p2
///   WaitingForDimLinePos { kind, p1, p2 } — click anywhere through which the
///                                           dim line / leader should pass;
///                                           commits the Dim dobject
///
/// `RadiusPending { center, on_circle }` is a transient flavor of
/// WaitingForDimLinePos that lets the user press 'D' to flip the kind
/// to Diameter before the second click.
#[derive(Clone, PartialEq, Debug)]
pub enum DimDraftState {
    Off,
    WaitingForP1,
    WaitingForP2 {
        p1: Vec2,
    },
    WaitingForDimLinePos {
        /// Encoded as one of the kernel `DimKind` variants with the
        /// final leader/dimline_pos NOT yet set — that comes from the
        /// upcoming click.
        kind: DimDraftKind,
    },
}

/// The half-built kind during `WaitingForDimLinePos`. Last click fills
/// in the dimline_pos / leader_end.
#[derive(Clone, PartialEq, Debug)]
pub enum DimDraftKind {
    Linear {
        p1: Vec2,
        p2: Vec2,
        ortho: cad_kernel::LinearOrtho,
    },
    Radius {
        center: Vec2,
        on_circle: Vec2,
    },
    Diameter {
        center: Vec2,
        on_circle: Vec2,
    },
}

/// Captured-at-release scratch data used by the canvas update block to
/// emit a single `GestureClassification` event AFTER the click handlers
/// have had their chance to mutate selection/state. Promoted to the
/// recorder at the bottom of the canvas update.
#[derive(Clone, Debug)]
pub struct PendingGesture {
    pub press_screen: (f32, f32),
    pub release_screen: (f32, f32),
    pub press_world: Vec2,
    pub release_world: Vec2,
    pub motion_px: f32,
    pub hit_at_press: Option<usize>,
    pub selection_before: Vec<usize>,
}

/// Editable state for the "New / Edit Text Style" dialog. When
/// `editing_id` is `Some`, the dialog UPDATES that style on OK;
/// when `None`, it CREATES a new style.
#[derive(Clone, Debug)]
pub struct TextStyleDialog {
    pub editing_id: Option<u32>,
    pub name: String,
    pub font_name: String,
    pub default_height: f64,
    /// Per-style color used for new Text dobjects created on this
    /// style. When `None` the style defers to ByLayer (the dobject's
    /// layer color wins). When `Some(aci)` the style supplies the ACI.
    /// Stored on the style as metadata; the kernel's `TextStyle`
    /// itself doesn't carry color today (LibreCAD doesn't either —
    /// it's an app-level convention). For v1 we surface the picker
    /// in the dialog but DON'T persist it on the kernel side —
    /// follow-up slice when the field lands.
    pub color_aci: Option<u8>,
}

impl TextStyleDialog {
    pub fn new_blank() -> Self {
        Self {
            editing_id: None,
            name: String::new(),
            font_name: "standard".into(),
            default_height: 0.25,
            color_aci: None, // ByLayer
        }
    }
    pub fn from_existing(id: u32, s: &cad_kernel::TextStyle) -> Self {
        Self {
            editing_id: Some(id),
            name: s.name.clone(),
            font_name: s.font_name.clone(),
            default_height: s.default_height,
            color_aci: None,
        }
    }
}

/// Arrowhead style chosen in the Dim Style form. Maps to DimStyle's
/// `arrow_filled` + `tick_size` on OK.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArrowKind {
    Filled,
    Hollow,
    Tick,
}

/// Which element color the shared ACI wheel is editing for the Dim Style
/// form. Carried on `AciPickRequest::DimStyleForm`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DimColorSlot {
    DimLine,
    ExtLine,
    Text,
}

/// Add/Edit form for a `cad_kernel::DimStyle` — parallel to
/// `TextStyleDialog`. Surfaces the most-used style fields across Lines /
/// Arrows / Text / Units. On OK the full DimStyle is built by cloning the
/// source style (STANDARD for new, the edited style for edit) and patching
/// only these fields, so the other DIMVARs survive untouched. Each color
/// is an independent ACI; `None` = ByBlock (0).
pub struct DimStyleDialog {
    pub editing_id: Option<u32>,
    pub name: String,
    pub arrow_size: f64,
    pub text_height: f64,
    pub decimal_places: i32,
    /// Dim-line color. `None` = ByBlock; `Some(aci)` = ACI 1..=255.
    pub color_aci: Option<u8>,
    /// Extension-line color.
    pub ext_color_aci: Option<u8>,
    /// Text color.
    pub text_color_aci: Option<u8>,
    /// Text vertical position (DIMTAD): 0 = centered on line, 1 = above,
    /// 4 = below.
    pub text_vert_pos: i32,
    /// Rotate text to align with the dim line (vs always horizontal).
    pub text_aligned: bool,
    pub arrow_kind: ArrowKind,
}

impl DimStyleDialog {
    pub fn new_blank() -> Self {
        let s = cad_kernel::DimStyle::standard();
        Self {
            editing_id: None,
            name: String::new(),
            arrow_size: s.arrow_size,
            text_height: s.text_height,
            decimal_places: s.decimal_places,
            color_aci: None,
            ext_color_aci: None,
            text_color_aci: None,
            text_vert_pos: s.text_vert_pos,
            text_aligned: !s.text_inside_horiz,
            arrow_kind: ArrowKind::Filled,
        }
    }
    pub fn from_existing(id: u32, s: &cad_kernel::DimStyle) -> Self {
        let aci = |c: u32| if c == 0 { None } else { Some(c.min(255) as u8) };
        Self {
            editing_id: Some(id),
            name: s.name.clone(),
            arrow_size: s.arrow_size,
            text_height: s.text_height,
            decimal_places: s.decimal_places,
            color_aci: aci(s.color_dim_line),
            ext_color_aci: aci(s.color_ext_line),
            text_color_aci: aci(s.color_text),
            text_vert_pos: s.text_vert_pos,
            text_aligned: !s.text_inside_horiz,
            arrow_kind: if s.tick_size > 0.0 {
                ArrowKind::Tick
            } else if s.arrow_filled {
                ArrowKind::Filled
            } else {
                ArrowKind::Hollow
            },
        }
    }
}

/// Which WallStyle color the shared ACI wheel is editing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WallColorSlot {
    Fill,
    Face,
}

/// Add/Edit form for a `cad_kernel::WallStyle` — the wall "type" (Dry Wall,
/// Structural, …). Parallel to `DimStyleDialog`.
pub struct WallStyleDialog {
    pub editing_id: Option<u32>,
    pub name: String,
    pub thickness: f64,
    /// Poché fill ACI; `None` = no fill (hollow wall).
    pub fill_aci: Option<u8>,
    /// Face-line ACI; `None` = ByLayer (use the dobject color).
    pub face_aci: Option<u8>,
    /// Draw the batt-insulation sine wave in the cavity.
    pub insulation: bool,
    pub description: String,
    /// Centerline linetype id (APP-LAYER — deliberately NOT in cad_kernel's `WallStyle`,
    /// so core stays byte-identical to RUST_CAD). `None` = don't draw a centerline (faces
    /// only, as before). Loaded from / saved to the app's `wall_centerline_ltype` map.
    pub centerline_ltype: Option<u32>,
}

impl WallStyleDialog {
    pub fn new_blank() -> Self {
        let s = cad_kernel::WallStyle::standard();
        Self {
            editing_id: None,
            name: String::new(),
            thickness: s.thickness,
            fill_aci: None,
            face_aci: None,
            insulation: false,
            description: String::new(),
            centerline_ltype: None,
        }
    }
    pub fn from_existing(id: u32, s: &cad_kernel::WallStyle) -> Self {
        let aci = |c: u32| if c == 0 { None } else { Some(c.min(255) as u8) };
        Self {
            editing_id: Some(id),
            name: s.name.clone(),
            thickness: s.thickness,
            fill_aci: aci(s.fill_color),
            face_aci: aci(s.face_color),
            insulation: s.insulation,
            description: s.description.clone(),
            centerline_ltype: None, // set by the caller from the app-layer map
        }
    }
}

/// Grip drag — recorded when the user grabs a grip handle of a selected
/// dobject (either by pressing+dragging OR clicking on it). v2: each grip
/// has a role (`GripRole`) that decides what changes when the user moves
/// it (e.g. line endpoint moves only that endpoint; circle quadrant
/// changes radius; line midpoint translates the whole line). Math lives
/// in `cad_kernel::Geom::with_grip_moved`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GripDrag {
    pub dobject_idx: usize,
    pub role: GripRole,
    pub grip_origin: Vec2,
}

/// Fillet — Slice M.3. Two-click flow: pick first object → pick second.
/// `radius` is captured at session start so re-running `fillet` with a
/// different value mid-flow doesn't change behaviour (sticky session).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FilletState {
    Off,
    WaitingForFirst(f64),
    WaitingForSecond(f64, usize, Vec2), // radius, idx of first, click point
}

/// Chamfer — Slice M.4. Same shape as FilletState, with two distances.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ChamferState {
    Off,
    WaitingForFirst(f64, f64),
    WaitingForSecond(f64, f64, usize, Vec2),
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AlignState {
    Off,
    WaitingForSrc1,
    WaitingForSrc2(Vec2),
    WaitingForTgt1(Vec2, Vec2),
    WaitingForTgt2(Vec2, Vec2, Vec2),
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum StretchState {
    Off,
    WaitingForBase(Vec2, Vec2), // crossing box captured (min,max); click base
    WaitingForDest(Vec2, Vec2, Vec2), // box + base; click dest to apply
}

/// Trim/extend session state. Each holds the confirmed cutting/boundary
/// basket; the target-pick phase loops on canvas clicks until the user
/// presses Enter or Esc.
#[derive(Clone, Debug)]
pub enum TrimState {
    Off,
    SelectingCutters,           // running a ForCuttingEdges select session
    PickingTargets(Vec<usize>), // cutters confirmed; loop on click
    /// "Empty Enter at cutter prompt" mode: every CURRENT dobject in the
    /// doc is a cutter, recomputed on every click. Pieces created by
    /// prior trims this session automatically join the cutter set. This
    /// is the AutoCAD default and the only way "trim against everything
    /// you see" can stay true across multiple clicks.
    PickingTargetsAll,
}

#[derive(Clone, Debug)]
pub enum ExtendState {
    Off,
    SelectingBoundaries,
    PickingTargets(Vec<usize>),
    /// Same "use every current dobject as a boundary" mode as
    /// `TrimState::PickingTargetsAll`.
    PickingTargetsAll,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenderMode {
    Cpu,
    Gpu,
    /// APX draft: every dobject drawn as a single dot — one instanced GPU
    /// draw call for the whole scene (fastest; approximate visual).
    Apx,
}

/// Which screen edge (if any) a floating Window is docked against.
/// Drives the size behavior in `apply_dock_pos`:
///   * Top/Bottom → full-width strip, content-fit height
///   * Left/Right → full-height strip, content-fit width
///   * Corners    → pinned position only, size content-fit
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DockEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug)]
pub struct DockState {
    pub pos: egui::Pos2,
    pub edge: DockEdge,
}

/// Snapshot of what the render loop did this frame. Surfaced in the
/// "Screen Stats" floating window so the user can see at a glance how
/// many dobjects exist, how many are in the viewport, and how many
/// actually got painted (vs filtered by visibility/sub-pixel culls).
///
/// The `in_viewport` count tells you whether the spatial-index broad-
/// phase is doing useful work: `in_viewport == total` at any zoom
/// means the cull isn't helping (typically because the index is
/// stale or absent).
#[derive(Clone, Debug, Default)]
pub struct RenderStats {
    /// Total dobjects in the document.
    pub total: usize,
    /// Passed the bbox-cull (spatial-index query OR full scan if index
    /// is stale). This is the candidate set the render loop iterates.
    pub in_viewport: usize,
    /// Painted this frame. Less than `in_viewport` when some
    /// candidates were skipped (hidden / frozen layer / sub-pixel).
    pub drawn: usize,
    /// Skipped: hidden Dobject style.visible == false OR layer is
    /// hidden/frozen.
    pub skipped_hidden: usize,
    /// Skipped: bbox < 1 pixel (the micro-cull); no visible benefit
    /// to painting them.
    pub skipped_subpx: usize,
    /// Frame time (seconds). Inverse of FPS.
    pub frame_dt: f32,
    /// Last frame's spatial-index status string ("idx N entries" /
    /// "idx stale" / etc.).
    pub index_label: String,
}

/// Result delivered from the hatch trace worker thread back to the
/// main UI thread via mpsc. The worker bundles its log buffer with
/// the result so the per-hit attempt diagnostics and the
/// tessellate/split/cluster counts arrive together with the loops
/// (otherwise we'd need a streaming log channel, which complicates
/// the lifecycle for no real gain).
pub enum HatchWorkerResult {
    /// Trace succeeded — `loops[0]` is the outer, the rest are islands.
    Success {
        loops: Vec<Vec<Vec2>>,
        log_lines: Vec<String>,
    },
    /// Trace failed (dead-ended, no valid face contains the seed, etc.).
    /// Caller should fall back to cheap path with auto-islands.
    Failure {
        reason: String,
        log_lines: Vec<String>,
    },
    /// User pressed Esc; the worker's cancel flag tripped a check
    /// somewhere in tessellate / split / cluster / trace.
    Cancelled { log_lines: Vec<String> },
}

/// Handle on a background hatch-trace operation. Held in
/// `CadApp::hatch_worker` while a worker is alive; `poll_hatch_worker`
/// drains it each frame and materialises the result when ready.
///
/// Lifecycle:
///   * spawn: `apply_pick_point_hatch` decides trace path → clones the
///     Document (CHEAP via Arc-internals for handles + Vec for
///     dobjects), spawns a `std::thread`, stores the receiver here.
///   * poll: each `update` call tries `rx.try_recv`; on Ok the
///     worker output is materialised and the field is set to None.
///   * cancel: Esc handler sets `cancel` and drops the worker; the
///     thread reads the flag at its next CANCEL_CHECK_STRIDE point
///     and exits (its send may fail — that's fine, we already dropped
///     the receiver).
///   * replace: a fresh Pick Point click while a worker is in flight
///     cancels the current one + spawns a new one. The old thread
///     exits naturally; its result is discarded.
pub struct HatchWorker {
    pub seed: Vec2,
    pub pattern: cad_kernel::HatchPattern,
    pub active_layer: LayerId,
    #[allow(dead_code)] // mirror of op_cancel; kept for direct debugging access
    pub cancel: StdArc<AtomicBool>,
    pub rx: mpsc::Receiver<HatchWorkerResult>,
}

/// Who asked the floating ACI picker for a color, so the chosen ACI flows
/// back to the right slot when the user clicks a circle in the wheel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AciPickRequest {
    /// Picker is editing a layer's color.
    Layer(LayerId),
    /// Picker is editing a dobject's color (Info palette dobject edit).
    Dobject(usize),
    /// Picker is editing the color of MANY dobjects at once (Properties
    /// dialog, group selection). The index list lives in
    /// `CadApp.aci_pick_many` so this enum stays `Copy`.
    DobjectMany,
    /// Picker is editing one of the Dim Style add/edit form's element
    /// colors (`dim_style_dialog`). The slot says which one.
    DimStyleForm(DimColorSlot),
    /// Picker is editing a Wall Style form color (fill or face).
    WallStyleForm(WallColorSlot),
    /// Picker is editing the Block dialog's instance color.
    BlockForm,
    /// Picker is editing one field of the run-script parameter dialog. The
    /// index is into `ScriptParamDialog.params`, so the enum stays `Copy`).
    ScriptParam(usize),
    /// Picker is editing the drawing's CURRENT color (text dialog parameter
    /// section — the color new dobjects are born with).
    CurrentColor,
    /// Picker is editing the Plot Style Table Editor's color for the selected
    /// plot styles.
    PlotStyleColor,
}

/// Which viewport the user is currently working in.
///
/// **This is the signal the modifiers dispatch on** — `move` is ONE command; the
/// active view decides which algorithm runs behind it (owner's rule: "knowing the
/// active one, you get move command, check 2d or 3d, take the right move in the
/// background").
///
/// ⚠️ It is deliberately NOT "which panel is open". A panel can be OPEN while you
/// work in the other view — gating on `factory.open` is precisely what hijacked `m`
/// out of the 2D drawing. ACTIVE means *last interacted with*: it only changes when
/// you actually click/drag inside a viewport.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ActiveView {
    /// The 2D plan — the default, and where every established command lives.
    #[default]
    TwoD,
    /// The 3D Factory viewport.
    ThreeD,
}

impl ActiveView {
    pub fn label(self) -> &'static str {
        match self {
            ActiveView::TwoD => "2D",
            ActiveView::ThreeD => "3D",
        }
    }
}

/// WHICH WORKSPACE THE WINDOW IS IN — chosen by the mode tab bar at the top.
///
/// The app has three full-window views — the 2D drafting canvas, the SIMLUX 3D
/// lighting viewport and the 3D Factory — and each has its own workspace with
/// its own command list on the left. EXACTLY ONE is shown at a time: switching
/// tabs closes the other two views, so the window never splits itself between
/// workspaces again (the old per-view checkboxes that let 2D + SIMLUX + Factory
/// share the window are gone; the tab bar is the only way in).
///
/// `mode` is the single source of truth; the three open flags
/// (`two_d_open` / `light.view3d_open` / `factory.open`) are its outputs. Code
/// that opens a view for its own reasons (an import that needs the Factory, the
/// Materials Factory preview, …) is still allowed to set `factory.open` — the
/// rising-edge hook in `update()` then switches the tab to match, so the window
/// shows what the code asked for. Enforcement lives in one place (frame start),
/// never at the thirty call sites.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// 2D drafting canvas — draw / modify / light placement. The default.
    #[default]
    Cad2D,
    /// SIMLUX 3D lighting viewport — calculation results and 3D display.
    Simlux,
    /// 3D Factory — parametric solids, building, furniture, storeys.
    Factory,
}

impl Mode {
    /// The tab's short name (the exact wording the tab bar shows).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Cad2D => "2D view",
            Mode::Simlux => "SIMLUX view",
            Mode::Factory => "3D Factory view",
        }
    }

    /// The tab's glyph.
    pub fn glyph(self) -> &'static str {
        match self {
            Mode::Cad2D => "▤",
            Mode::Simlux => "◧",
            Mode::Factory => "⬒",
        }
    }

    /// One-line description, shown as the tab's tooltip.
    pub fn blurb(self) -> &'static str {
        match self {
            Mode::Cad2D => "Drafting canvas: draw, modify, place luminaires on the plan.",
            Mode::Simlux => "3D lighting viewport: lux results, heatmap, isolux.",
            Mode::Factory => "3D modeling: solids, building, rooms, furniture, storeys.",
        }
    }
}

/// The "Make room (from selected outline)" details form — what the app asks
/// before a plan outline becomes a real 3D room. Opened from the 2D ROOMS
/// panel with the validated outline; the parameters here (with the factory
/// settings as defaults) are exactly what the built room is constructed with,
/// so asking is honest: the answer is what gets built.
#[derive(Clone)]
pub struct RoomForm {
    /// The closed outline (metres) the room is built from.
    pub outline: Vec<glam::Vec2>,
    /// What the room is called; empty falls back to "Room N".
    pub name: String,
    /// START height — the Z the room stands on (floor slab bottom).
    pub base_z: f32,
    /// CLEAR height — floor top to ceiling underside; the slabs are extra.
    pub height: f32,
    pub floor_t: f32,
    pub wall_t: f32,
    pub ceiling_t: f32,
}

/// Transient authoring state for the **Materials Factory** node editor. The node graphs live here,
/// keyed by texture index; each is seeded from its [`crate::factory::TextureAsset`] on first open and
/// compiled back onto it on every edit, so nothing extra is persisted (the material's flat fields
/// already round-trip). See [`crate::material_graph`].
#[derive(Default)]
pub struct MaterialsFactoryState {
    /// The material (texture index) currently being edited.
    pub sel: Option<usize>,
    /// One authoring graph per material, built lazily.
    pub graphs: std::collections::HashMap<usize, crate::material_graph::MaterialGraph>,
    /// Canvas pan offset (screen px).
    pub pan: egui::Vec2,
    /// A wire being dragged from an OUTPUT socket `(node_id, out_index)` toward an input.
    pub drag_from: Option<(crate::material_graph::NodeId, u8)>,
    /// A node being dragged by its header `(node_id)` — so the header owns the drag, not the canvas.
    pub drag_node: Option<crate::material_graph::NodeId>,
    /// The node selected on the canvas (its properties show in the inspector).
    pub sel_node: Option<crate::material_graph::NodeId>,
    /// The rendered **material ball**, cached against a signature of the material it shows. It is a
    /// CPU render ([`crate::matball`]) of a few thousand pixels, which is cheap once and far too
    /// expensive every frame — hence the signature rather than a dirty flag, since a material is
    /// edited by writing its fields from a dozen different places.
    pub ball: Option<(u64, egui::TextureHandle)>,
}

/// State of a background Radiance run (`render.bat`: oconv → rpict → pfilt → ra_bmp), shared
/// between the worker thread and the "Radiance — offline render" window.
pub struct RadState {
    pub done: bool,
    pub ok: bool,
    /// Combined stdout+stderr of the pipeline (shown when it fails).
    pub log: String,
    /// The finished `render.bmp` as RGBA8.
    pub image: Option<(usize, usize, Vec<u8>)>,
    pub dir: std::path::PathBuf,
}

impl Default for CadApp {
    fn default() -> Self {
        // Build the command registry from the seed arrays FIRST, then derive the
        // default rail lists as ids in canonical (array) order (Phase 3). One
        // source: the arrays → registry → item ids.
        let command_registry = crate::command::build(DRAW_CMDS, MODIFY_CMDS);
        let draw_items = command_registry.by_category(crate::command::CommandCategory::Draw);
        let modify_items = command_registry.by_category(crate::command::CommandCategory::Modify);
        let mut s = Self {
            doc: Document::default(),
            intersections: Vec::new(),
            cmd: String::new(),
            history: Vec::new(),
            current_prompt: String::new(),
            last_command: None,
            focus_at_frame_start: None,
            empty_enter_count_in_select: 0,
            place_prompt_open: false,
            place_coord_prompt: false,
            grip_drag: None,
            grip_drag_peers: Vec::new(),
            last_render_stats: RenderStats::default(),
            screen_stats_open: false,
            screen_stats_was_open: false,
            hatch_worker: None,
            op_cancel: StdArc::new(AtomicBool::new(false)),
            docked_window_pos: HashMap::new(),
            menu_launch_anchor: HashMap::new(),
            raise_windows: Vec::new(),
            dock_undock_hold: HashMap::new(),
            dock_dragging: std::collections::HashSet::new(),
            canvas_screen_rect: None,
            demo_view_set: false,
            press_time: None,
            press_pos: None,
            cmd_window_open: true,
            factory_was_open: false,
            simlux_was_open: false,
            mode: Mode::Cad2D,
            factory_was_in_sketch: false,
            factory_return_after_sketch: false,
            simlux_full_pin: false,
            factory_full_pin: false,
            mode_sections_closed: std::collections::HashSet::new(),
            room_form: None,
            room_form_focus_name: false,
            menubar_rect: egui::Rect::NOTHING,
            layers_window_open: false,
            pens_window_open: false,
            info_window_open: false,
            inspector_dock_state: crate::dock::DockState::Docked(crate::dock::DockRegion::Right),
            props_layout_capture: false,
            ui_inspect: false,
            ui_inspect_log: Vec::new(),
            rail_active: "line".to_string(),
            draw_items,
            modify_items,
            command_registry,
            cmd_dump_open: false,
            palette_open: false,
            palette_query: String::new(),
            palette_sel: 0,
            palette_focus: false,
            palette_pos: None,
            menu_flyouts: Vec::new(),
            menu_flyout_hot: false,
            menu_flyout_leave_t: None,
            command_method: std::collections::HashMap::new(),
            dobjects_window_open: false,
            aci_picker: {
                let mut p = crate::aci_picker::AciPickerState::default();
                p.try_load_mapping(&aci_mapping_path());
                p
            },
            aci_pick_request: None,
            aci_pick_many: Vec::new(),
            blockdiff_overlay: None,
            blockdiff_pick: BlockDiffPick::Off,
            param_name_dialog: None,
            insert_param_prompt: None,
            block_task_rec: None,
            btr_awaiting_name: false,
            selected: None,
            tool: Tool::None,
            last_point: None,
            arc_method: ArcMethod::ThreePoints,
            arc_picker_open: false,
            pending: Vec::new(),
            pending_bulges: Vec::new(),
            pline_mode: PlineMode::Line,
            pline_arc_sub: PlineArcSub::Normal,
            pline_dir_override: None,
            pline_next_width: (0.0, 0.0),
            pline_width_cap: PlineWidthCap::None,
            pending_widths: Vec::new(),
            spline_width: 0.0,
            spline_width_wait: false,
            spline_degree_override: None,
            scale: 6.0,
            world_offset: egui::Vec2::ZERO,
            zoom_state: ZoomState::Off,
            view_history: Vec::new(),
            array_open: false,
            array_method: ArrayMethod::Linear,
            array_count: 6,
            array_fill_deg: 360.0,
            array_rotate_items: true,
            array_center: Vec2::new(100.0, 100.0),
            array_pick_center: false,
            array_path_idx: None,
            array_path_measure: false,
            array_path_dist: 25.0,
            array_path_align: true,
            array_path_anchor: PathAnchor::Start,
            array_pick_path: false,
            picking_source: false,
            array_cols: 10,
            array_rows: 10,
            array_dx: 50.0,
            array_dy: 50.0,
            intersect_pending_click: false,
            intersect_view_pending: false,
            last_visible: None,
            last_intersect_label: String::new(),
            snap_override: None,
            snap_enabled: SnapSet::defaults(),
            snap_window_open: false,
            env: UserEnv::load(),
            settings_open: false,
            settings_section: String::new(),
            var_set_pending: None,
            calc: crate::calc::CalcStore::new(),
            calc_sidecar_rx: None,
            qat_actions: QatAction::default_set(),
            qat_customize_open: false,
            qat_just_opened: false,
            qat_chevron_rect: None,
            logo_tex: None,
            logo_load_tried: false,
            refocus_cmd: true,
            snap_cycle_index: 0,
            snap_cycle_anchor: None,
            select_mode: SelectMode::Off,
            selection: Vec::new(),
            window_first: None,
            armed_window_inside: None,
            fence_armed: false,
            fence_first: None,
            select_remove_mode: false,
            selection_prev: Vec::new(),
            last_erased: Vec::new(),
            move_state: MoveState::Off,
            queued_op: QueuedOp::None,
            pending_hatch_pattern: (None, 1.0, 0.0),
            hatch_dialog_open: false,
            hatch_dialog_solid: false,
            hatch_dialog_name: "ANSI31".into(),
            hatch_dialog_scale: 1.0,
            hatch_dialog_angle: 0.0,
            hatch_pick_point_armed: false,
            hatch_pick_point_session: None,
            hatch_confirm_open: false,
            hatch_last_idx: None,
            hatch_dialog_edit_mode: false,
            hatch_preview_snap: None,
            fps_smooth: 0.0,
            index: None,
            index_dirty: true,
            index_label: String::new(),
            render_mode: RenderMode::Cpu,
            hatch_cache: HashMap::new(),
            debug_open: false,
            gpu_renderer: StdArc::new(Mutex::new(GpuShapeRenderer::default())),
            light: crate::light::LightState::new(),
            light3d_renderer: StdArc::new(Mutex::new(crate::light3d::Scene3dRenderer::default())),
            scene3d_static: StdArc::new(Vec::new()),
            scene3d_static_key: None,
            simlux_perf_last_frame: None,
            simlux_perf_last_slow: None,
            simlux_perf_key: None,
            lux_overlay_us: std::cell::Cell::new(0),
            lux_overlay_cells: std::cell::Cell::new(0),
            simlux_gl_us: StdArc::new(std::sync::atomic::AtomicU64::new(0)),
            frame_start: None,
            frame_marks: Vec::new(),
            factory: crate::factory::FactoryState::default(),
            factory_perf_prev: None,
            factory_perf_last_frame: None,
            factory_perf_last_slow: None,
            furniture_gpu_meshes: std::collections::HashMap::new(),
            furniture_tex_meshes: std::collections::HashMap::new(),
            furniture_transp_meshes: std::collections::HashMap::new(),
            texture_rgba: std::collections::HashMap::new(),
            factory_scene_ver: 0,
            active_view: ActiveView::TwoD,
            sel_mask: Vec::new(),
            gpu_dirty: true,
            view_seq: 1,
            cached_sketch_lines: Vec::new(),
            cached_plan_lines: Vec::new(),
            cached_lines_sig: u64::MAX,
            cached_plan_cells: Vec::new(),
            layer_panel_open: true,
            layer_rename: None,
            layer_rename_buf: String::new(),
            layer_rename_focus_pending: false,
            layer_name_counter: 0,
            pen_panel_open: true,
            info_panel_open: true,
            copy_state: CopyState::Off,
            rotate_state: RotateState::Off,
            rotate_copy: false,
            scale_copy: false,
            scale_state: ScaleState::Off,
            mirror_state: MirrorState::Off,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            matchprops_state: MatchPropsState::Off,
            block_def_state: BlockDefState::Off,
            block_dialog_stash: None,
            block_dialog_pick_base: false,
            last_cursor_raw_world: None,
            pending_release_swallow: false,
            stretch_window_box: None,
            file_dialog: None,
            light_gesture: false,
            raster_doc: None,
            raster_editor: None,
            underlay_tex: Vec::new(),
            underlay_sig: 0,
            parametric: crate::param_editor::ParamSession::new(),
            file_dialog_dir: None,
            file_preview: None,
            current_file: None,
            extra_store: crate::simlux_io::ExtraDataStore::default(),
            extra_choice: None,
            results_rx: None,
            results_due: false,
            unsaved: false,
            close_confirm: false,
            arch_modal_open: false,
            sun_modal_open: false,
            materials_open: false,
            materials: MaterialsFactoryState::default(),
            render_modal_open: false,
            two_d_open: true,
            shortcuts_open: false,
            simlux_split_prev: false,
            pt_job: None,
            pt_gpu: None,
            pt_gl: None,
            pt_preview: None,
            pt_last_pass: 0,
            rad_res: 2, // 1280×960 — a good accuracy/time default for rpict
            rad_run_after_pick: false,
            rad_job: None,
            rad_started: None,
            rad_preview: None,
            rad_loaded: false,
            pt_device: crate::pathtrace::Device::Gpu, // falls back to CPU if the driver refuses
            pt_res: 1,
            pt_passes: 64,
            pt_denoise: true,
            texset_target: None,
            photometry_wants_folder: false,
            illuminaire_wants_blocks: false,
            report_open: false,
            report_opts: crate::report::Options::default(),
            report_page: 0,
            report_tex: Vec::new(),
            report_tex_dirty: false,
            report_wants_dir: false,
            report_wants_images: None,
            calc_rx: None,
            calc_progress: None,
            calc_selected: None,
            calc_started: None,
            light_drag_symbols: Vec::new(),
            symbol_sync_seq: 0,
            report_prefs_loaded: false,
            report_doc: None,
            report_doc_key: None,
            report_logo_tex: Vec::new(),
            report_cover_tex: Vec::new(),
            undo_budget_bytes: UNDO_BUDGET_BYTES,
            scene_import_pending: false,
            villa_autoloaded: false,
            startup_repair: 0,
            arch_tab: ArchTab::Staircase,
            arch_stair: cad_solid::architecture::StairParams::default(),
            arch_spiral: cad_solid::architecture::SpiralParams::default(),
            arch_ramp: cad_solid::architecture::RampParams::default(),
            arch_dogleg: cad_solid::dogleg::DoglegInput::default(),
            arch_dogleg_treads: true,
            arch_spiral_csg: cad_solid::spiral::SpiralInput::default(),
            arch_door: cad_solid::door::DoorInput::default(),
            handle_lib: None,
            handle_tex: std::collections::HashMap::new(),
            handle_sel: String::new(),
            handle_finish: String::new(),
            handle_dialog: false,
            door_preview: DoorPreview {
                zoom: 1.0,
                ..DoorPreview::default()
            },
            door_mats: crate::door_mat::DoorMaterials::default(),
            arch_helical: cad_solid::architecture::HelicalRampParams::default(),
            arch_cupboard: cad_solid::cupboard::CupboardInput::default(),
            arch_kitchen: cad_solid::kitchen::KitchenInput::default(),
            arch_cabin: cad_solid::cabin::CabinInput::default(),
            arch_sweep: cad_solid::sweeplight::SweepInput::default(),
            arch_desk: cad_solid::desk::DeskInput::default(),
            arch_couch: cad_solid::couch::CouchInput::default(),
            close_after_save: false,
            pending_close: false,
            autosave_on: false, // OFF every start — see the field's docs
            autosave_rx: None,
            save_failure: None,
            last_autosave: std::time::Instant::now(),
            edit_seq: 0,
            autosave_seq: 0,
            busy: None,
            last_load_ms: 1200, // seeded; self-calibrates after the first real open/save
            last_save_ms: 400,
            clipboard_dobjects: Vec::new(),
            clipboard_unit_m: 1.0,
            paste_state: PasteState::Off,
            pedit_state: PeditState::Off,
            groups: Vec::new(),
            insert_state: InsertState::Off,
            block_dialog: None,
            insert_dialog: None,
            insert_dialog_pick: false,
            pending_insert: None,
            insert_param_pick: None,
            insert_live: None,
            offset_state: OffsetState::Off,
            dist_state: DistState::Off,
            area_state: None,
            xref_pending: None,
            attr_def_flow: AttrDefFlow::Off,
            attedit_state: AttEditState::Off,
            wblock_subdoc: None,
            save_dialog_purpose: 0,
            xline_state: XlineState::Off,
            boundary_state: BoundaryState::Off,
            centermark_state: CenterMarkState::Off,
            ray_state: RayState::Off,
            donut_state: DonutState::Off,
            wipeout_state: WipeoutState::Off,
            layer_pick: LayerPickState::Off,
            ptdist_state: PtDistribState::Off,
            pick_cycle_index: 0,
            pick_cycle_cell: None,
            pick_cands: Vec::new(),
            pick_cycle_at: None,
            props_edit_gesture: false,
            block_editor: None,
            cmd_flow: None,
            cmd_undo_base: None,
            cmd_base_objs: None,
            transcript: Vec::new(),
            draft_preview: true,
            text_draft: TextDraftState::Off,
            dim_draft: DimDraftState::Off,
            pending_text: None,
            text_waiting_height: false,
            wall_waiting_thickness: false,
            text_style_dialog: None,
            dim_style_dialog: None,
            dim_style_manager_open: false,
            dim_style_manager_sel: 0,
            current_dim_style: 0,
            current_wall_style: 0,
            wall_style_manager_open: false,
            wall_style_manager_sel: 0,
            wall_style_dialog: None,
            wall_centerline_ltype: std::collections::HashMap::new(),
            text_input_dialog_open: false,
            text_input_dialog_buf: String::new(),
            text_input_dialog_anchor: None,
            text_input_dialog_focus: false,
            text_input_dialog_height: 0.0,
            text_input_dialog_style_id: cad_kernel::TextStyleTable::STANDARD,
            text_input_dialog_style_count_before: usize::MAX,
            text_dialog_dock_state: crate::dock::DockState::Floating(egui::pos2(1240.0, 420.0)),
            text_dialog_oblique_deg: 0.0,
            text_dialog_angle_deg: 0.0,
            text_dialog_halign: cad_kernel::TextHAlign::Left,
            text_list_mode: cad_kernel::TextListKind::None,
            font_picker_highlight: 0,
            font_picker_orig: None,
            font_manager: std::cell::RefCell::new(cad_text::FontManager::new()),
            text_waiting_angle: false,
            script: None,
            py_console_open: false,
            py_console_log: Vec::new(),
            py_console_input: String::new(),
            py_console_name: String::new(),
            py_console_hist: Vec::new(),
            py_console_hist_idx: None,
            py_console_dock: crate::dock::DockState::Docked(crate::dock::DockRegion::Bottom),
            py_examples: Vec::new(),
            py_example_sel: 0,
            py_editor_open: false,
            py_editor_text: String::new(),
            py_editor_path: None,
            py_editor_name: String::new(),
            py_editor_dirty: false,
            py_editor_confirm: None,
            script_param_dialog: None,
            script_param_pick: None,
            script_preview: None,
            script_pending_run: None,
            scripting_doc_open: false,
            scripting_doc_text: None,
            script_meta_finish_pending: false,
            script_undo_base: None,
            script_group_snapshots: Vec::new(),
            plotstyle_open: false,
            plotstyle_edit_anchor: None,
            plot_dialog_rect: None,
            plotstyle_editor_rect: None,
            prev_plotstyle_open: false,
            prev_plot_ladder_open: false,
            prev_plot_preview_open: false,
            plotstyle_open_seq: 0,
            plot_ladder_open_seq: 0,
            plot_preview_open_seq: 0,
            plotstyle_return_to_plot: false,
            plotstyle_return_after_sub: false,
            plotstyle_footer_rect: None,
            plotstyle_sel: Vec::new(),
            plotstyle_anchor: None,
            plotstyle_tab: 0,
            plotstyle_ladder_open: false,
            plotstyle_ladder_work: Vec::new(),
            plotstyle_ladder_sel: None,
            plotstyle_ladder_inch: false,
            plotstyle_pst_io: None,
            plotstyle_edit_file: None,
            new_ctb_open: false,
            new_ctb_name: String::new(),
            ctb_table_cache: std::collections::HashMap::new(),
            ctb_fingerprint: Vec::new(),
            layer_glyph_tex: std::collections::HashMap::new(),
            plot_dialog_open: false,
            qselect: QSelectState::default(),
            current_point_style: 0,
            current_point_size: -5.0,
            point_style_picker_open: false,
            laywalk_open: false,
            laywalk_restore: None,
            laywalk_current: 0,
            layer_search: String::new(),
            units_dialog_open: false,
            units_dialog_draft: cad_kernel::Units::default(),
            units_dlg_n: 1.0,
            units_dlg_m: 1.0,
            plot_format: "pdf".into(),
            plot_suppress_viewer: false,
            plot_pdf_path: String::new(),
            plot_paper: cad_kernel::plotstyle::PaperSize::A4,
            plot_landscape: false,
            plot_area_kind: 0,
            plot_window: None,
            plot_offset_center: true,
            plot_offset_x: 0.0,
            plot_offset_y: 0.0,
            plot_scale_fit: true,
            plot_scale_n: 1.0,
            plot_scale_lw: false,
            plot_with_styles: true,
            plot_object_lw: true,
            plot_mono: false,
            plot_win_pick: 0,
            plot_win_p1: None,
            plot_pdf_browse: false,
            plot_unit_inch: false,
            plot_run_after_save: false,
            plot_preview_open: false,
            layout_selection: Vec::new(),
            layout_move_last: None,
            layout_grip_drag: None,
            saved_model_scale: None,
            saved_model_offset: None,
            show_new_layout_dialog: false,
            new_layout_name: String::new(),
            new_layout_paper: cad_kernel::plotstyle::PaperSize::A4,
            new_layout_landscape: false,
            new_layout_ctb: String::new(),
            layout_print_preview: true,
            viewport_draw_state: 0,
            viewport_draw_li: None,
            viewport_draw_p1: None,
            viewport_draw_p2: None,
            viewport_scale_dialog_open: false,
            viewport_scale_text: String::new(),
            viewport_custom_n: String::new(),
            viewport_custom_m: String::new(),
            viewport_custom_unit: "mm".into(),
            viewport_ctb_name: String::new(),
            vp_edit_dialog: None,
            vp_edit_scale_text: String::new(),
            vp_edit_ctb: String::new(),
            vp_edit_init_for: None,
            pagesetup_open: false,
            pagesetup_dock_state: crate::dock::DockState::Floating(egui::pos2(1240.0, 420.0)),
            dbg: crate::dbg_recorder::DbgRecorder::default(),
            dbg_window_open: false,
            dbg_note_buf: String::new(),
            dbg_last_watched: None,
            dbg_press_pos: None,
            dbg_press_hit: None,
            dbg_press_sel: Vec::new(),
            dbg_pending_gesture: None,
            offset_erase: false,
            offset_layer_src: false,
            offset_applied_count: 0,
            lengthen_state: LengthenState::Off,
            break_state: BreakState::Off,
            align_state: AlignState::Off,
            stretch_state: StretchState::Off,
            fillet_state: FilletState::Off,
            fillet_multiple: false,
            fillet_waiting_radius: false,
            fillet_poly_all: false,
            chamfer_state: ChamferState::Off,
            chamfer_multiple: false,
            chamfer_poly_all: false,
            chamfer_dist_wait: ChamferDistWait::Off,
            trim_state: TrimState::Off,
            extend_state: ExtendState::Off,
            pre_op_selection: Vec::new(),
            trim_debug_log: Vec::new(),
            trim_debug_open: false,
            hatch_debug_log: Vec::new(),
            hatch_debug_open: false,
            trim_debug_frame: 0,
        };
        // Demo layers so the Layer panel has visible content at first
        // launch. ACI palette colors keep these out of the truecolor
        // table (per the "ACI is primary" memo + the storage refactor
        // that made TrueColors a shared, dedup'd table).
        let walls = s.doc.layers.add(Layer {
            name: "WALLS".into(),
            color: Color::Aci(1), // red
            ..Layer::layer_zero()
        });
        let _hidden = s.doc.layers.add(Layer {
            name: "HIDDEN".into(),
            color: Color::Aci(3), // green
            visible: false,
            ..Layer::layer_zero()
        });
        s.doc.layers.active = walls;

        // Demo dobjects so the canvas is never empty on first launch.
        // (push is a pure append now — stamp the fresh-draw style so the
        // demos land on the active WALLS layer as before.)
        //
        // SIZED AS A REAL PLAN, NOT DECORATION: the drawing unit is the
        // document's default millimetre (1 unit = 1 mm), and these figures
        // are drawn at ROOM magnitudes — the closed rectangle is a 6000 ×
        // 4500 mm (6 × 4.5 m) room outline, selectable and Make-room-able
        // right away; the other figures sit around it in the same few-metre
        // ranges. They used to be ±(20–90)-unit centimetre-scale sketches,
        // which no room could ever be made from. The view is framed once on
        // the first real frame (`demo_view_set`), so the plan is on screen.
        let mut demo_line: DObject = Line {
            a: Vec2::new(-8000.0, -5000.0),
            b: Vec2::new(-3500.0, -1500.0),
        }
        .into();
        s.stamp_fresh_style(&mut demo_line.style);
        s.doc.push(demo_line);
        let mut demo_circle: DObject = Circle {
            center: Vec2::new(9000.0, 4500.0),
            radius: 2500.0,
        }
        .into();
        s.stamp_fresh_style(&mut demo_circle.style);
        s.doc.push(demo_circle);
        let mut demo_arc: DObject = Arc {
            center: Vec2::new(8000.0, -3500.0),
            radius: 2500.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::PI,
        }
        .into();
        s.stamp_fresh_style(&mut demo_arc.style);
        s.doc.push(demo_arc);
        let mut demo_ellipse: DObject = Ellipse {
            center: Vec2::new(-7500.0, 3500.0),
            major: Vec2::new(2000.0, 1000.0), // semi-major ≈ 2.2 m, rotation ≈ 26.6°
            ratio: 0.55,                      // semi-minor ≈ 1.2 m
        }
        .into();
        s.stamp_fresh_style(&mut demo_ellipse.style);
        s.doc.push(demo_ellipse);
        // Demo point (Slice E preview).
        let mut demo_point: DObject = Point {
            location: Vec2::new(-4500.0, -6000.0),
            style: 0,
            size: 0.0,
        }
        .into();
        s.stamp_fresh_style(&mut demo_point.style);
        s.doc.push(demo_point);
        // The room — a closed 6000 × 4500 mm rectangle (6 × 4.5 m). The one
        // figure that can become a room TODAY: select it, then 2D view ▸
        // ROOMS ▸ "Make room (from selected outline)".
        let mut demo_pl: DObject = Polyline {
            vertices: vec![
                PolyVertex {
                    pos: Vec2::new(-3000.0, -2250.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(3000.0, -2250.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(3000.0, 2250.0),
                    bulge: 0.0,
                },
                PolyVertex {
                    pos: Vec2::new(-3000.0, 2250.0),
                    bulge: 0.0,
                },
            ],
            closed: true,
            widths: Vec::new(),
        }
        .into();
        s.stamp_fresh_style(&mut demo_pl.style);
        s.doc.push(demo_pl);
        s.recompute();
        s.history.push(
            "Fresh drawing — a 6 × 4.5 m room outline plus demo figures (millimetre units).".into(),
        );
        s.history.push("Select the closed rectangle and make it a room: 2D view ▸ ROOMS ▸ Make room (from selected outline).".into());
        s.history
            .push("Pick a tool from the top toolbar, or type 'help'.".into());
        s
    }
}

const HELP: &str = "\
line     x1,y1 x2,y2                 - draw line segment
circle   cx,cy r                     - draw circle

arc      cx,cy r start_deg end_deg   - center + radius + start/end angles (CCW)
arc3p    p1 p2 p3                    - through three points
arcse    cx,cy start end             - center + start point + end point (CCW)
arccr    start end r [major|minor]   - chord + radius (default minor)
arccl    start end length [left|right] - chord + arc length (default left)

del N                                - delete dobject N
clear                                - remove everything
help                                 - this message

  toolbar:
  pointer  - no tool (commands only)
  line     - click two endpoints
  circle   - click center, then any point on the rim
  arc      - click center, start point, end point (CCW)
  Esc cancels an in-progress draw";

/// The `calc` / `calc help` syntax summary (the idle-prompt calculator).
const CALC_HELP: &str = "\
calc — expression calculator + user variables
  expressions : 2+3*4, (1+2)^3, sqrt(16), sin(30) [DEGREES], pi, e, ans
  operators   : + - * / ^ %  (precedence: unary > ^ > * / % > + -)
  functions   : sqrt abs round floor ceil min max sin cos tan
                asin acos atan atan2 exp ln log10   (trig in degrees)
  assignments : x=5   h=w*2  (LAZY — re-evaluated on every use)
  variables   : [A-Za-z_][A-Za-z0-9_]*, case-sensitive, persisted per drawing
  note        : typed expressions have NO spaces (Space=Enter submits);
                pasted '2 + 3' works fine";

/// Outcome of a command-internal "U" press. Drives whether `run_command`
/// returns (handled), reports "nothing left", or falls back to global undo.
enum CmdUndo {
    Handled,
    NothingLeft,
    NoCommand,
}

impl CadApp {
    /// The command line's own widget id.
    ///
    /// The one field allowed to drive the global Enter/Space cascade. Everything else that can
    /// hold the keyboard — every DragValue, every dialog field — owns its own Enter, and must not
    /// have it re-read as "repeat the last command".
    fn cmd_line_id() -> egui::Id {
        egui::Id::new("simlux::command_line")
    }

    /// True when a field OTHER than the command line holds the keyboard.
    ///
    /// Every global key handler that can destroy work has to ask this. A keystroke belongs to
    /// whoever has focus: Enter in a wall-height box confirms a wall height, and Delete in one
    /// removes a digit. Read globally and acted on regardless, those same two keys repeat the last
    /// command and erase the selection — so typing a number could empty the drawing, which is
    /// exactly what was reported.
    ///
    /// Uses the focus captured at FRAME START, because a field that commits on Enter surrenders
    /// focus while it is being drawn; asking afterwards returns None precisely when it matters.
    #[inline]
    fn typing_in_a_field(&self) -> bool {
        self.focus_at_frame_start
            .is_some_and(|id| id != Self::cmd_line_id())
    }

    /// Whether a Delete keypress belongs to the SIMLUX FIXTURE selection.
    ///
    /// Delete is the drafting ERASE key first, so this only claims it when the drawing has no
    /// selection of its own — the two handlers are exact complements and cannot both fire.
    ///
    /// THE FOCUS TEST IS `typing_in_a_field`, NOT `wants_keyboard_input`. SIMLUX keeps the command
    /// line focused so a typed command lands immediately ("reclaim the command line" on every
    /// Esc), and `wants_keyboard_input` is true whenever ANY text field holds focus — including
    /// that one, including when it is empty. So the guard was true essentially always and Delete
    /// could never reach a selected fixture. Reported as "i cant delete the lights by selecting
    /// them and clicking delete".
    ///
    /// `typing_in_a_field` asks the question actually meant: is a field OTHER than the command
    /// line taking the keystroke. It is what the drafting Delete has always used, two hundred
    /// lines away, which is why erase worked and this did not.
    fn delete_targets_lights(&self) -> bool {
        !self.typing_in_a_field()
            && !self.light.selected.is_empty()
            && self.selection.is_empty()
            && self.selected.is_none()
    }

    /// Delete SIMLUX fixtures AND the block instances they were placed as. Returns (lights, blocks).
    ///
    /// A PLACED FITTING IS ONE THING IN TWO WORLDS — a symbol on the drawing and a light in the
    /// calculation — so deleting it has to take both. Removing one and leaving the other is the
    /// exact shape of what was reported: markers left behind with nothing to light, and symbols
    /// left behind with nothing behind them.
    ///
    /// BOTH HALVES ARE UNDOABLE, in one step. They were not: SIMLUX state was off the undo stack
    /// entirely, so Undo after a delete brought the symbols back and not the markers — and Undo
    /// after a fixture-only delete stepped straight past it to an older drawing edit, taking the
    /// drawing with it.
    ///
    /// A fixture placed by hand (`Place luminaire`) has no `from_block` at all and takes nothing
    /// with it, which is right: there was never a symbol.
    fn delete_fixtures(&mut self, ids: &[u32]) -> (usize, usize) {
        if ids.is_empty() {
            return (0, 0);
        }
        // The instances FIRST, while the fixtures that name them are still here.
        //
        // A face sketch swaps `self.doc` for the sketch plane's own document, and the blocks are
        // on the PLAN — so the drawing half is skipped there rather than aimed at the wrong file.
        let idx = if self.factory.session.is_some() {
            Vec::new()
        } else {
            let doomed: Vec<&cad_light::Luminaire> = self
                .light
                .luminaires
                .iter()
                .filter(|l| ids.contains(&l.id))
                .collect();
            crate::illuminaire::instances_for(
                &self.doc,
                doomed.into_iter(),
                self.doc.units.metres_per_unit,
            )
        };

        // ONE STEP, WHICHEVER HALVES IT TOUCHES. A fitting placed from the library has a symbol on
        // the plan; a hand-placed point does not, and an undo step naming a drawing edit that
        // never happened would take a second Undo press to get past.
        if idx.is_empty() {
            self.snapshot_lights();
        } else {
            self.snapshot_doc_and_lights();
        }

        let before = self.light.luminaires.len();
        self.light.luminaires.retain(|l| !ids.contains(&l.id));
        self.light.selected.retain(|s| !ids.contains(s));
        self.light.drag = None;
        self.light.hover = None;
        let lights = before - self.light.luminaires.len();

        if !idx.is_empty() {
            // Highest index downward, so the earlier ones stay valid — the same rule ERASE
            // follows, and for the same reason.
            for &i in idx.iter().rev() {
                if i < self.doc.dobjects.len() {
                    self.doc.dobjects.remove(i);
                }
            }
            // A pickfirst selection is a list of INDICES into the vector that just shifted under
            // it, so it is dropped rather than left pointing at whatever slid into place.
            self.selection.clear();
            self.selected = None;
            self.intersections.clear();
            self.index_dirty = true;
            self.touch_view();
        }
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::LightOp {
                op: "delete".into(),
                detail: format!(
                    "{lights} fitting(s) and {} plan symbol(s) removed; {} id(s) asked for",
                    idx.len(),
                    ids.len(),
                ),
                fittings_before: before,
                fittings_after: self.light.luminaires.len(),
                message: self.light.last_msg.clone(),
                elapsed_us: 0,
            }
        );
        (lights, idx.len())
    }

    /// The Delete key, on a SIMLUX fixture selection.
    fn delete_selected_fixtures(&mut self) {
        let ids = self.light.selected.clone();
        let (lights, blocks) = self.delete_fixtures(&ids);
        if lights == 0 {
            return;
        }
        self.light.last_msg = format!(
            "Deleted {lights} fixture(s){} — {} left.",
            if blocks > 0 {
                format!(" and {blocks} symbol(s)")
            } else {
                String::new()
            },
            self.light.luminaires.len(),
        );
    }

    /// Remove a fitting from the library, and take what it placed on this project with it.
    ///
    /// Reported as "when i delete them from fitting it doesnt delete the markers and these
    /// diamonds start showing up" — the library entry went and its fixtures stayed, drawn as the
    /// hollow diamond that means "a point with no fitting". Seven of them, from a combo the user
    /// had already deleted.
    ///
    /// SCOPED TO THIS DRAWING, which is all it can be: the library spans projects and the fixtures
    /// do not. Removing a fitting used in a file that is not open leaves that file alone, and it
    /// reopens with its symbols and its lights exactly as they were.
    ///
    /// Returns (fixtures, symbols) removed.
    fn remove_fitting(&mut self, id: u32) -> (usize, usize) {
        let Some(name) = self.light.library.get(id).map(|f| f.name.clone()) else {
            return (0, 0);
        };
        // The fixtures are tied to the block DEFINITION they were placed as, which is looked up
        // by the fitting's current name — the same name `ensure_block` writes.
        let doomed: Vec<u32> = match self.doc.blocks.find(&name) {
            Some(b) => self
                .light
                .luminaires
                .iter()
                .filter(|l| l.from_block == Some(b))
                .map(|l| l.id)
                .collect(),
            None => Vec::new(),
        };
        let (lights, blocks) = self.delete_fixtures(&doomed);

        self.light.library.remove(id);
        if self.light.lib_sel == Some(id) {
            self.light.lib_sel = None;
            self.light.lib_name_buf.clear();
        }
        if self.light.place_fitting == Some(id) {
            self.light.place_fitting = None;
        }
        self.light.last_msg = if lights == 0 {
            format!("Removed \"{name}\" from the library.")
        } else {
            format!("Removed \"{name}\" — {lights} fixture(s) and {blocks} symbol(s) deleted.")
        };
        (lights, blocks)
    }

    /// Read the Illuminaire library off disk and parse the photometry each fitting is linked to.
    ///
    /// BOTH HALVES, or the promise breaks. The library file stores a PATH and a profile NAME; the
    /// profile itself lives in `light.profiles`, which starts with only the built-in. Without this
    /// second step a library restored from disk shows every tile as "no LDT" and every placed
    /// fitting calculates as nothing — the combo would survive the save and not the reopen, which
    /// is the one thing it exists to do.
    ///
    /// A file that has since been moved or deleted leaves the LINK intact and logs it. Clearing it
    /// would silently discard the user's pairing because a drive was not mounted this morning.
    pub fn load_illuminaire_library(&mut self) {
        match crate::illuminaire::Library::load() {
            Ok(l) => self.light.library = l,
            // AN UNREADABLE LIBRARY IS NOT AN EMPTY ONE. Starting blank here would let the next
            // save write over a file that was merely locked or half-copied this once.
            Err(e) => {
                self.history
                    .push(format!("  illuminaire: library not loaded — {e}"));
                self.light.illuminaire_locked = true;
                return;
            }
        }
        let links: Vec<(String, String)> = self
            .light
            .library
            .fittings
            .iter()
            .filter(|f| !f.ldt_path.is_empty())
            .map(|f| (f.name.clone(), f.ldt_path.clone()))
            .collect();
        let mut missing = 0usize;
        for (name, path) in &links {
            if !self.light.load_photometry(path) {
                missing += 1;
                self.history.push(format!(
                    "  illuminaire: \"{name}\" — {path} is not readable"
                ));
            }
        }
        if !self.light.library.fittings.is_empty() {
            self.history.push(format!(
                "  illuminaire: {} fitting(s) loaded{}",
                self.light.library.fittings.len(),
                if missing == 0 {
                    String::new()
                } else {
                    format!(", {missing} missing photometry")
                },
            ));
        }
        // `load_photometry` sets the ACTIVE profile as a side effect, which would leave the last
        // file read selected for new hand-placed luminaires. That is a startup detail, not a
        // choice the user made.
        self.light.active_profile = crate::light::BUILTIN.to_string();
    }

    /// The Illuminaire window — the fitting library. Bails if closed.
    ///
    /// Everything the window needs is snapshotted BEFORE it opens and everything it asks for is
    /// carried out after: `self.doc` and `self.light` cannot both be borrowed while egui holds a
    /// closure, which is the same reason `render_light_panel` returns an action.
    fn render_illuminaire(&mut self, ctx: &egui::Context) {
        if !self.light.illuminaire_open {
            return;
        }
        // Taken for the frame so the window owns them without a borrow on `self.light` outliving
        // the closure egui holds.
        let lib = std::mem::take(&mut self.light.library);
        let blocks = std::mem::take(&mut self.light.lib_blocks);
        let scanned = std::mem::take(&mut self.light.lib_scanned);
        let profiles = self.light.profiles.clone();
        let mut open = self.light.illuminaire_open;
        let mut sel = self.light.lib_sel;
        let mut add_open = self.light.lib_add_open;
        let mut name_buf = std::mem::take(&mut self.light.lib_name_buf);

        let act = crate::illuminaire::window_ui(
            ctx,
            &mut open,
            &lib,
            &mut sel,
            &mut add_open,
            &mut name_buf,
            crate::illuminaire::WindowInput {
                blocks: &blocks,
                blocks_from: &self.light.lib_blocks_from,
                scanned: &scanned,
                folder: &self.light.lib_folder,
                profiles: &profiles,
                placing: self.light.place_fitting,
                blocks_unit_m: self.light.lib_blocks_unit_m,
            },
        );

        self.light.library = lib;
        self.light.lib_blocks = blocks;
        self.light.lib_scanned = scanned;
        self.light.lib_sel = sel;
        self.light.lib_add_open = add_open;
        self.light.lib_name_buf = name_buf;
        self.light.illuminaire_open = open;

        self.apply_illuminaire_action(act);
    }

    /// Carry out what the Illuminaire window asked for.
    ///
    /// SPLIT FROM THE WINDOW so the wiring is testable. With the two halves fused, the only way to
    /// reach this code in a test was to synthesise a pointer click on a particular tile — so
    /// nothing did, and a handler that quietly stopped calling `remove_fitting` passed every test
    /// while leaving markers on the plan. That is the bug this file is fixing; it should not be
    /// possible to reintroduce it silently.
    fn apply_illuminaire_action(&mut self, act: crate::illuminaire::Action) {
        // ---- carry out what it asked for ----
        let mut dirty = false;

        if act.browse_blocks {
            self.illuminaire_wants_blocks = true;
            self.open_file_dialog(FileDialogMode::Open, "");
        }
        if act.browse_folder {
            self.photometry_wants_folder = true;
            // WITH A FILTER, so the browser shows what is in each folder. The answer is still the
            // folder — but you are choosing it BECAUSE of the .ldt files in it, and a browser that
            // showed a directory of thirty of them as empty is why this was reported as "its not
            // detecting the ies/ldt files".
            self.open_file_dialog(FileDialogMode::PickFolder, ".ies|.ldt");
        }
        if act.rescan {
            self.light.lib_scanned = crate::illuminaire::scan_folder(&self.light.lib_folder);
            self.history.push(format!(
                "  illuminaire: {} photometry file(s) found",
                self.light.lib_scanned.len()
            ));
        }
        if act.blocks_from_drawing {
            let plan = Self::plan_doc_of(self.factory.session.as_ref(), &self.doc);
            self.light.lib_blocks_unit_m = plan.units.metres_per_unit;
            self.light.lib_blocks = crate::illuminaire::symbols_from(plan);
            self.light.lib_blocks_from = format!("{} in this drawing", self.light.lib_blocks.len());
            self.light.lib_add_open = true;
        }
        if let Some(i) = act.add {
            // The symbol is copied in the units of whatever document it was read FROM — a block
            // file drawn in millimetres stays millimetres in the library, and the conversion
            // happens once, at placement, against the destination drawing.
            if let Some(b) = self.light.lib_blocks.get(i).cloned() {
                let unit = self.light.lib_blocks_unit_m;
                let name = b.name.clone();
                let id = self.light.library.add(crate::illuminaire::Fitting {
                    name: name.clone(),
                    id: 0,
                    symbol: b.symbol,
                    symbol_unit_m: unit,
                    ldt_path: String::new(),
                    profile: String::new(),
                    model_path: String::new(),
                });
                self.light.lib_sel = Some(id);
                self.light.lib_name_buf = name.clone();
                self.history
                    .push(format!("  illuminaire: added \"{name}\""));
                dirty = true;
            }
        }
        if let Some((id, path)) = act.link {
            // PARSE FIRST, LINK ONLY IF IT PARSED. Linking a name that is not in the profile
            // library would place fittings whose photometry never resolves — no light and no
            // error, which is the shape of bug this whole feature exists to remove.
            if self.light.load_photometry(&path) {
                let prof = self.light.active_profile.clone();
                if let Some(f) = self.light.library.get_mut(id) {
                    f.ldt_path = path.clone();
                    f.profile.clone_from(&prof);
                    let n = f.name.clone();
                    self.history.push(format!("  illuminaire: {n} ← {prof}"));
                    dirty = true;
                }
            } else {
                self.history
                    .push(format!("  illuminaire: could not read {path}"));
            }
        }
        if let Some((id, name)) = act.rename {
            if let Some(f) = self.light.library.get_mut(id) {
                f.name.clone_from(&name);
                dirty = true;
            }
            self.light.lib_name_buf = name;
        }
        if let Some(id) = act.remove {
            self.remove_fitting(id);
            self.history
                .push(format!("  illuminaire: {}", self.light.last_msg));
            dirty = true;
        }
        if let Some(id) = act.place {
            self.light.place_fitting = Some(id);
            // The placement and aiming modes are mutually exclusive: a click cannot drop a bare
            // point, insert a block AND aim something, and leaving another armed would try.
            self.light.place_mode = false;
            self.light.aim_mode = false;
            self.light.aim_pick = None;
            let what = self
                .light
                .library
                .get(id)
                .map(|f| f.name.clone())
                .unwrap_or_default();
            self.light.last_msg = format!("Placing \"{what}\" — click the plan. Esc stops.");
        }
        if let Some(m) = act.set_blocks_unit {
            self.light.lib_blocks_unit_m = m;
        }
        if let Some((id, m)) = act.set_fitting_unit {
            if let Some(f) = self.light.library.get_mut(id) {
                f.symbol_unit_m = m;
            }
            // AND THE DRAWING FOLLOWS. The block definition is a cache of the library entry, and
            // `ensure_block` reuses it by name — so correcting the unit without rebuilding it
            // would leave every instance already placed at the old scale, with the panel now
            // stating a size the plan does not show.
            if self.factory.session.is_none() {
                if let Some(f) = self.light.library.get(id).cloned() {
                    let u = self.doc.units.metres_per_unit;
                    let k = if u.is_finite() && u > 0.0 { u } else { 1.0 };
                    // Only when this drawing HAS one — a fitting never placed here has nothing
                    // to correct, and an undo step for an edit that did not happen is a press
                    // the user has to spend twice.
                    if self.doc.blocks.find(&f.name).is_some() {
                        self.snapshot_doc();
                        crate::illuminaire::rebuild_block(&mut self.doc, &f, k);
                        self.intersections.clear();
                        self.index_dirty = true;
                        self.touch_view();
                    }
                }
            }
            dirty = true;
        }
        if act.stop_placing {
            self.light.place_fitting = None;
            self.light.last_msg = "Stopped placing.".into();
        }

        if dirty {
            if self.light.illuminaire_locked {
                self.history.push(
                    "  illuminaire: NOT saved — the library on disk could not be read at startup"
                        .to_string(),
                );
            } else if let Err(e) = self.light.library.save() {
                self.history
                    .push(format!("  illuminaire: could not save the library — {e}"));
            }
        }
    }

    /// Read a drawing purely for its BLOCK TABLE, and offer what is in it to the add panel.
    ///
    /// The project is untouched: this is the "LIGHT BLOCK.dwg" case, a file that exists to hold
    /// symbols and nothing else. Opening it as a drawing would replace what the user is working
    /// on, which is the opposite of what picking a block library means.
    fn load_illuminaire_blocks(&mut self, path: &str) {
        // A .dwg IS THE COMMON CASE HERE — the file this feature was asked for is "LIGHT
        // BLOCK.dwg". Converting it in place is the same route `do_open` takes, so a block
        // library needs no more preparation than a drawing does.
        let converted;
        let mut path = path;
        if path.to_lowercase().ends_with(".dwg") {
            match self.convert_dwg_to_dxf(path) {
                Ok(p) => {
                    converted = p.to_string_lossy().into_owned();
                    path = &converted;
                }
                Err(e) => {
                    self.history.push(format!("  illuminaire: {path} — {e}"));
                    return;
                }
            }
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            // DXF is 7-bit in practice but not guaranteed; a Latin-1 drawing must not be an error.
            Err(_) => match std::fs::read(path) {
                Ok(b) => b.iter().map(|&c| c as char).collect(),
                Err(e) => {
                    self.history.push(format!("  illuminaire: {path} — {e}"));
                    return;
                }
            },
        };
        match cad_io::dxf::read_dxf(&text) {
            Ok(doc) => {
                self.light.lib_blocks_unit_m = doc.units.metres_per_unit;
                self.light.lib_blocks = crate::illuminaire::symbols_from(&doc);
                self.light.lib_blocks_from =
                    format!("{} in {}", self.light.lib_blocks.len(), file_stem_of(path),);
                self.light.lib_add_open = true;
                self.light.illuminaire_open = true;
                self.history.push(format!(
                    "  illuminaire: {} block(s) read from {}",
                    self.light.lib_blocks.len(),
                    file_stem_of(path),
                ));
            }
            Err(e) => {
                self.history
                    .push(format!("  illuminaire: could not read {path} — {e}"));
            }
        }
    }

    /// Drop one fitting on the plan at world metres `(x, y)`. `false` if nothing was armed.
    ///
    /// Two things land, at the same spot and in different worlds: an ordinary `BlockRef` on the
    /// 2D drawing, and a SIMLUX luminaire carrying the linked photometry. The block is what a CAD
    /// package sees; the luminaire is what the calculation sees.
    fn place_illuminaire_at(&mut self, x: f32, y: f32) -> bool {
        // EVERY WAY THIS DECLINES IS RECORDED, because two of them say nothing to the user and the
        // third is easy to miss. "the lights were also not being placed" was reported against a
        // session recording that could not show a single one of them.
        let n = self.light.luminaires.len();
        let Some(fid) = self.light.place_fitting else {
            crate::dbg_event!(
                self,
                crate::dbg_recorder::DbgEvent::LightOp {
                    op: "place refused".into(),
                    detail: format!(
                    "at ({x:.2}, {y:.2}) — NO FITTING ARMED (light.place_fitting is None), so the \
                     click is discarded. Arming happens in the fitting library; if the user \
                     believes a fitting is selected, that is where it was lost."
                ),
                    fittings_before: n,
                    fittings_after: n,
                    message: String::new(), // deliberately: this path tells the user nothing
                    elapsed_us: 0,
                }
            );
            return false;
        };
        // A FACE SKETCH IS A DIFFERENT DOCUMENT, in metres, on a wall. The luminaire markers are
        // world metres and are not drawn there at all (`paint_luminaires_2d` bails), so inserting
        // into it would put a block on the wall's sketch plane and a light nowhere visible.
        if self.factory.session.is_some() {
            self.light.last_msg = "Close the face sketch before placing fittings.".into();
            crate::dbg_event!(
                self,
                crate::dbg_recorder::DbgEvent::LightOp {
                    op: "place refused".into(),
                    detail: format!("at ({x:.2}, {y:.2}) — a face sketch is open"),
                    fittings_before: n,
                    fittings_after: n,
                    message: self.light.last_msg.clone(),
                    elapsed_us: 0,
                }
            );
            return false;
        }
        let Some(f) = self.light.library.get(fid).cloned() else {
            self.light.place_fitting = None;
            crate::dbg_event!(
                self,
                crate::dbg_recorder::DbgEvent::LightOp {
                    op: "place refused".into(),
                    detail: format!(
                        "at ({x:.2}, {y:.2}) — armed fitting id {fid} is NOT IN THE LIBRARY ({} \
                     entries), so the arming was silently cleared",
                        self.light.library.fittings.len(),
                    ),
                    fittings_before: n,
                    fittings_after: n,
                    message: String::new(), // deliberately: this path tells the user nothing either
                    elapsed_us: 0,
                }
            );
            return false;
        };
        // Metres → DRAWING units. A plan in millimetres wants 3000, not 3.
        let u = self.doc.units.metres_per_unit;
        let k = if u.is_finite() && u > 0.0 { u } else { 1.0 };
        let at = Vec2::new(x as f64 / k, y as f64 / k);

        // AN EDIT TO THE DRAWING IS AN EDIT TO THE DRAWING, and has to announce itself the same
        // way every other one does. This did not, and the symptom was reported as "once i place a
        // file ... still no blocks. the block shows up after i delete a light or 2".
        //
        // The blocks were there the whole time. Nothing had told the canvas its cached geometry
        // was stale (`touch_view`) or the spatial index that there was something new to pick
        // (`index_dirty`), so they were invisible and unclickable until an unrelated edit rebuilt
        // both — deleting a light, in the report.
        //
        // ONE STEP FOR BOTH HALVES: a block lands on the plan and a light at the same point, and
        // an Undo that took back one of them would leave a state the user never made.
        self.snapshot_doc_and_lights();
        let block = crate::illuminaire::insert(&mut self.doc, &f, at, k);
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();

        // THE HANDLE OF THE INSTANCE JUST PUSHED — the durable link between this fixture and this
        // symbol. `from_block` names the block DEFINITION, which every instance of a fitting
        // shares, so it can say WHAT the symbol is and never WHICH ONE.
        let handle = self.doc.dobjects.last().map(|d| d.handle);
        let id = self.light.place_point(x, y);
        if let Some(l) = self.light.luminaires.iter_mut().find(|l| l.id == id) {
            l.profile.clone_from(&f.profile);
            l.from_block = Some(block);
        }
        if let Some(h) = handle {
            self.light.symbol_of.insert(id, h);
        }
        // The fixture and its symbol are in step as of now, so the sync must not read the edit it
        // has just made as somebody else's rotation.
        self.symbol_sync_seq = self.edit_seq;
        // `place_point` stages its own copy, and the step above already covers the whole act —
        // block and light together. Left staged, it would be picked up by the NEXT snapshot as if
        // it were that edit's "before", so one Undo would reach back through two placements.
        self.light.discard_staged_undo();
        self.light.last_msg = format!(
            "Placed \"{}\"{} at ({x:.2}, {y:.2}).",
            f.name,
            if f.profile.is_empty() {
                " (no photometry linked)"
            } else {
                ""
            },
        );
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::LightOp {
                op: "place".into(),
                detail: format!(
                    "\"{}\" id={id} at ({x:.2}, {y:.2})  profile={}  symbol_block={block}",
                    f.name,
                    if f.profile.is_empty() {
                        "NONE — no photometry linked"
                    } else {
                        &f.profile
                    },
                ),
                fittings_before: n,
                fittings_after: self.light.luminaires.len(),
                message: self.light.last_msg.clone(),
                elapsed_us: 0,
            }
        );
        true
    }

    // ---- commands & math -----------------------------------------------

    /// True when ANY command / draw tool / modify flow is mid-operation. Used
    /// by command-internal undo to decide whether a bare "U" is an in-command
    /// step-back (something active) or a global undo (nothing active).
    fn any_command_active(&self) -> bool {
        self.tool != Tool::None
            || self.cmd_flow.is_some()
            || self.select_mode != SelectMode::Off
            || !matches!(self.trim_state, TrimState::Off)
            || !matches!(self.extend_state, ExtendState::Off)
            || self.move_state != MoveState::Off
            || self.copy_state != CopyState::Off
            || self.paste_state != PasteState::Off
            || self.pedit_state != PeditState::Off
            || self.rotate_state != RotateState::Off
            || self.scale_state != ScaleState::Off
            || self.mirror_state != MirrorState::Off
            || self.align_state != AlignState::Off
            || self.break_state != BreakState::Off
            || self.lengthen_state != LengthenState::Off
            || self.offset_state != OffsetState::Off
            || self.stretch_state != StretchState::Off
            || self.matchprops_state != MatchPropsState::Off
            || self.fillet_state != FilletState::Off
            || self.chamfer_state != ChamferState::Off
            || self.picking_source
            || self.intersect_pending_click
    }

    /// Reverse the LAST step of the currently-active command only (AutoCAD "U"
    /// keyword). PLINE is NOT handled here — it has its own richer U handler in
    /// the `Tool::Polyline` block (arc sub-flows, bulges, widths). Returns
    /// whether it reversed a step, found nothing to reverse, or found no active
    /// command (so the caller does a global undo). NEVER undoes command
    /// initiation itself — that's what Esc is for.
    fn command_internal_undo(&mut self) -> CmdUndo {
        // 1) CIRCLE prompt-flow — walk the state machine back one captured point.
        let flow = self.cmd_flow.as_ref().map(|f| (f.name, f.circle));
        if let Some((name, step)) = flow {
            if name == "circle" {
                let back = match step {
                    CircleStep::P3c(p1, _) => Some(CircleStep::P3b(p1)),
                    CircleStep::P3b(_) => Some(CircleStep::P3a),
                    CircleStep::P3a => Some(CircleStep::Center),
                    CircleStep::P2b(_) => Some(CircleStep::P2a),
                    CircleStep::P2a => Some(CircleStep::Center),
                    CircleStep::TtrRadius(o1, p1, _, _) => Some(CircleStep::TtrObj2(o1, p1)),
                    CircleStep::TtrObj2(_, _) => Some(CircleStep::TtrObj1),
                    CircleStep::TtrObj1 => Some(CircleStep::Center),
                    CircleStep::Radius(_) | CircleStep::Diameter(_) => Some(CircleStep::Center),
                    CircleStep::Center => None, // at start — can't
                };
                return match back {
                    Some(s) => {
                        self.cmd_flow = Some(CmdFlow {
                            name: "circle",
                            circle: s,
                        });
                        self.flow_show_prompt();
                        self.history.push("  circle: stepped back one point".into());
                        CmdUndo::Handled
                    }
                    None => CmdUndo::NothingLeft,
                };
            }
            return CmdUndo::NothingLeft; // some other flow active — leave it be
        }

        // 2) Selection in progress — drop the most-recently picked dobject.
        if self.select_mode != SelectMode::Off {
            return match self.selection.pop() {
                Some(i) => {
                    self.history.push(format!(
                        "  selection: removed #{} ({} left)",
                        i,
                        self.selection.len()
                    ));
                    self.touch_view();
                    CmdUndo::Handled
                }
                None => CmdUndo::NothingLeft,
            };
        }

        // 3) Committed-result modify commands (offset / trim / extend / fillet
        //    / chamfer). Each result snapshotted the doc, so pop back to — but
        //    never past — the depth captured when the command began. Fillet and
        //    chamfer also step back an in-progress FIRST pick when no committed
        //    result remains (so U "un-picks" before it "un-does").
        let committed_active = self.offset_state != OffsetState::Off
            || !matches!(self.trim_state, TrimState::Off)
            || !matches!(self.extend_state, ExtendState::Off)
            || self.fillet_state != FilletState::Off
            || self.chamfer_state != ChamferState::Off;
        if committed_active {
            if let Some(base) = self.cmd_undo_base {
                if self.undo_stack.len() > base {
                    self.do_undo();
                    self.history
                        .push("  ↶ reversed last result (still in command)".into());
                    return CmdUndo::Handled;
                }
            }
            // No committed result left — for fillet/chamfer, clear a pending
            // first-object pick instead (back to "select first object").
            if let FilletState::WaitingForSecond(r, _, _) = self.fillet_state {
                self.fillet_state = FilletState::WaitingForFirst(r);
                self.refresh_fillet_prompt();
                self.history.push("  fillet: cleared first pick".into());
                return CmdUndo::Handled;
            }
            if let ChamferState::WaitingForSecond(d1, d2, _, _) = self.chamfer_state {
                self.chamfer_state = ChamferState::WaitingForFirst(d1, d2);
                self.history.push("  chamfer: cleared first pick".into());
                return CmdUndo::Handled;
            }
            return CmdUndo::NothingLeft;
        }

        // 4) In-memory draw flows (uncommitted pending points).
        match self.tool {
            Tool::Line => {
                // Chained: the last committed segment is the last dobject; its
                // start point `a` is where the rubber-band returns to.
                let base = self.cmd_base_objs.unwrap_or(0);
                if self.doc.dobjects.len() > base {
                    if let Some(d) = self.doc.dobjects.pop() {
                        if let Geom::Line(l) = d.geom {
                            self.pending = vec![l.a];
                            self.last_point = Some(l.a);
                        }
                        self.intersections.clear();
                        self.index_dirty = true;
                        self.touch_view();
                        self.history.push("  line: removed last segment".into());
                    }
                    CmdUndo::Handled
                } else if self.pending.pop().is_some() {
                    self.last_point = None;
                    self.set_prompt(current_hint(Tool::Line, self.arc_method, 0));
                    self.history.push("  line: removed first point".into());
                    CmdUndo::Handled
                } else {
                    CmdUndo::NothingLeft
                }
            }
            Tool::Arc => {
                if self.pending.pop().is_some() {
                    let n = self.pending.len();
                    self.set_prompt(current_hint(Tool::Arc, self.arc_method, n));
                    self.history.push("  arc: removed last point".into());
                    CmdUndo::Handled
                } else {
                    CmdUndo::NothingLeft
                }
            }
            Tool::Rectangle => {
                if self.pending.pop().is_some() {
                    self.set_prompt(current_hint(Tool::Rectangle, self.arc_method, 0));
                    self.history.push("  rectangle: removed last corner".into());
                    CmdUndo::Handled
                } else {
                    CmdUndo::NothingLeft
                }
            }
            Tool::Spline => {
                if self.pending.pop().is_some() {
                    self.pending_bulges.pop();
                    self.pending_widths.pop();
                    self.history.push("  spline: removed last point".into());
                    CmdUndo::Handled
                } else {
                    CmdUndo::NothingLeft
                }
            }
            // Any other active draw tool: pop a pending point if present.
            t if t != Tool::None => {
                if self.pending.pop().is_some() {
                    self.history.push("  removed last point".into());
                    CmdUndo::Handled
                } else {
                    CmdUndo::NothingLeft
                }
            }
            // No draw tool: either another modify state is mid-flight (nothing
            // to step back) or truly idle (caller does a global undo).
            _ => {
                if self.any_command_active() {
                    CmdUndo::NothingLeft
                } else {
                    CmdUndo::NoCommand
                }
            }
        }
    }

    /// Update the live status line shown above the cmd input. Empty
    /// string clears it. Replaces history-pushed prompts so the user sees
    /// only the CURRENT instruction, not a growing pile.
    fn set_prompt<S: Into<String>>(&mut self, s: S) {
        self.current_prompt = s.into();
        self.empty_enter_count_in_select = 0;
    }
    fn clear_prompt(&mut self) {
        self.current_prompt.clear();
        self.empty_enter_count_in_select = 0;
    }

    /// Compact number formatter for prompts / history (4 decimals, trailing
    /// zeros trimmed). Ported with the spline ribbon-width sub-command.
    fn fmt_r(v: f64) -> String {
        let s = format!("{:.4}", v);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s.is_empty() || s == "-0" {
            "0".into()
        } else {
            s.to_string()
        }
    }

    #[track_caller]
    /// Timing wrapper — measures how long a command takes to EXECUTE and stamps it
    /// onto the `CmdRun` event, so a dump answers "what made the app hang?" instead of
    /// only "what was typed". Wrapping is required: `run_command_inner` has ~27
    /// early-return intercepts, so there is no single tail to time.
    ///
    /// Re-entrancy safe: we remember the event index BEFORE the inner call pushes its
    /// CmdRun, then patch from there. A nested `run_command("previous")` stamps its own
    /// event; the outer stamps its own, whose time legitimately includes the nested one.
    fn run_command(&mut self, raw: &str) {
        let t0 = std::time::Instant::now();
        let from = self.dbg.events.len();
        self.run_command_inner(raw);
        let us = t0.elapsed().as_micros() as u64;
        self.dbg.patch_cmd_elapsed_at(from, us);
    }

    fn run_command_inner(&mut self, raw: &str) {
        // Echo the line into the history. A TOP-LEVEL command (typed at the
        // idle "command:" prompt) reads "command: circle"; a reply to an
        // active prompt reads "> 50". Both then stack upward in the history.
        if self.current_prompt.is_empty() {
            self.history.push(format!("command: {}", raw));
        } else {
            self.history.push(format!("> {}", raw));
        }
        let trimmed = raw.trim();
        // Any non-empty input cancels the 2-stage-Enter notice.
        self.empty_enter_count_in_select = 0;
        // A2: a NEW command must cancel any in-flight hatch-trace worker and
        // drop its result (Background-Ops I-3) — else a trace that finishes
        // after the switch drops invisible boundary polylines into the doc.
        // A hatch-trace worker leaves `tool == None` and every `*_state` Off,
        // so a guarded cancel would skip it — cancel unconditionally here.
        self.cancel_hatch_worker();
        // SESSION RECORDER — capture every cmd-line invocation BEFORE
        // the intercepts run. The parsed result is recorded only after
        // parsing succeeds; pre-parser intercepts (text body, fillet
        // radius, etc.) emit their own events from within their arms.
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::CmdRun {
                raw: raw.to_string(),
                parsed_debug: match cad_kernel::parser::parse(trimmed) {
                    Ok(c) => format!("{:?}", c),
                    Err(e) => format!("ParseErr({})", e),
                },
                source: crate::dbg_recorder::CmdSource::Typed,
                elapsed_us: None, // stamped by the `run_command` wrapper once it returns
            }
        );

        // ---- ACTIVE-VIEW dispatch — ONE command, two algorithms -----------
        // "check first which viewport is active — sketch or 3D — then I give the move
        // command, act accordingly."
        //
        // There is NO `3dmove`. `move` is `move`. This branch runs the SAME six steps
        // (Move → select what is going to move → first point → second point → done)
        // against 3D solids when the 3D viewport is the one you are working in.
        //
        // ⚠️ The gate is `active_view` — the viewport you LAST INTERACTED WITH — never
        // `factory.open`. A visible panel says nothing about what you are editing;
        // gating on it hijacked `m` out of the 2D drawing (twice). With `TwoD` active
        // this whole block is skipped and the established 2D command runs untouched,
        // whatever the 3D panel is doing.
        //
        // A sketch does NOT force 2D here. While you draft ON the plane you are clicking
        // the 2D canvas, which sets `active_view = TwoD`, so this block is already
        // skipped — drafting stays 2D. `active_view` becomes `ThreeD` only when you
        // interact with the 3D viewport (pick or orbit a solid), and then you DO want the
        // 3D move, sketch open or not. An earlier `&& session.is_none()` clause here was
        // WRONG: it fired only in that exact state (sketch open + working in the 3D view)
        // and sent `move` down the dead 2D path, so a picked solid never moved
        // (`select_mode → ForSelect`, `NOTE: 3D select` forever). Dispatch on
        // `active_view` ALONE — that is the whole rule.
        if self.active_view == ActiveView::ThreeD {
            // (1) a running op consumes typed values (degrees / factor / R / C)
            if let Some(mut md) = self.factory.modify.take() {
                let plane = cad_solid::Plane::default();
                // A VALUE may be an expression (`r*2`, `2+3`): rewrite the
                // token through the calculator BEFORE the modifier sees it.
                // Gated on the STRICT expression shape, never a bare word:
                // `e` must stay erase, `r`/`c` stay reference/copy, and a
                // defined variable named `r` must not hijack reference mode.
                let tok = if crate::calc::looks_like_expr_token(trimmed) {
                    match self.eval_number(trimmed) {
                        Ok(v) => crate::calc::fmt_value(v),
                        Err(_) => trimmed.to_string(),
                    }
                } else {
                    trimmed.to_string()
                };
                if let Some(f) = md.type_value(&tok, &plane, &mut self.factory.model) {
                    self.factory_note(format!(
                        "3D {} typed '{trimmed}' [{}] → {f:?}",
                        md.op.label(),
                        md.pick_name()
                    ));
                    self.factory_after_feed(md, f);
                    return;
                }
                self.factory.modify = Some(md); // not a value → may be a new command
            }
            // ZOOM in 3D — the 2D zoom, on the camera (dispatched by active_view). Bare
            // `zoom`/`z` (or `zoom w`) arms a WINDOW: drag a box in the view and the camera
            // reframes to it — the 2D default, NOT a jump to extents. Explicit verbs still
            // work: e/a extents, in/out dolly, a bare factor scales.
            // ---- ZOOM in 3D — MIRRORS the 2D zoom command. ----
            // If a zoom is already live, the next typed token is an OPTION (w/e/p/nX);
            // an unrecognised token exits zoom and falls through to normal dispatch —
            // exactly like the 2D `zoom_state` machine.
            if self.factory.zoom_mode != crate::factory::ZoomMode::Off {
                if self.zoom3d_option(trimmed) {
                    return;
                }
                self.factory.zoom_mode = crate::factory::ZoomMode::Off;
                self.factory.status.clear();
                // fall through — the token was a real command, not a zoom option
            }
            let lc = trimmed.to_ascii_lowercase();
            if lc == "zoom" || lc == "z" || lc.starts_with("zoom ") || lc.starts_with("z ") {
                let arg = lc.split_whitespace().nth(1).unwrap_or("").to_string();
                if arg.is_empty() {
                    // Bare `zoom`/`z` → WINDOW (the 2D default): drag a box, or click two
                    // corners, with the amber "zoom window" rubber-band. `z r` = real-time.
                    self.factory.zoom_mode = crate::factory::ZoomMode::Window;
                    self.factory.zoom_drag = None;
                    self.factory.zoom_cur = None;
                    self.factory.zoom_rt_before = None;
                    self.factory.status =
                        "ZOOM window — drag a box (or click two corners) · [R]eal-time [E]xtents [P]revious · nX scale  [Esc]".into();
                    let s = self.factory.zoom_status();
                    self.dbg_zoom(
                        trimmed,
                        "WINDOW (default: drag a box / click 2 corners) · R=real-time · E=extents · P=previous · nX=scale",
                        "window ARMED — drag a box or click two corners".into(),
                        s.clone(), s,
                    );
                    self.history.push(
                        "  ZOOM window — drag a box (or click two corners) · [R]eal-time [E]xtents [P]revious · nX scale".into());
                } else {
                    // Inline arg (`z r`, `z e`, `z 2`): arm window, then apply the option.
                    self.factory.zoom_mode = crate::factory::ZoomMode::Window;
                    if !self.zoom3d_option(&arg) {
                        self.factory.zoom_mode = crate::factory::ZoomMode::Off;
                        self.factory.status.clear();
                    }
                }
                return;
            }
            use cad_solid::modify::ModifyOp as MO;
            let op3d = match trimmed.to_ascii_lowercase().as_str() {
                "move" | "m" => Some(MO::Move),
                "copy" | "c" | "co" | "cp" => Some(MO::Copy),
                "rotate" | "ro" => Some(MO::Rotate),
                "scale" | "sc" => Some(MO::Scale),
                "mirror" | "mi" => Some(MO::Mirror),
                _ => None,
            };
            if let Some(op) = op3d {
                self.factory.abort_op(); // a new command supersedes the old one
                if self.factory.selection.is_empty() {
                    // step 3 — select what is going to move. Clicks in the 3D view fill
                    // the basket; Enter dispatches into the picks (the 2D `queued_op`
                    // pattern, mirrored).
                    self.factory.queued = Some(op);
                    self.factory.status = format!(
                        "{}: select solids, Enter to continue [Esc cancels]",
                        op.label()
                    );
                } else {
                    // pickfirst — skip the session, straight to the first point
                    self.factory_begin_op(op);
                }
                self.factory_note(format!("3D {} — {}", op.label(), self.factory.status));
                self.history.push(format!("  {}", self.factory.status));
                return;
            }
            if matches!(
                trimmed.to_ascii_lowercase().as_str(),
                "erase" | "delete" | "e"
            ) && !self.factory.selection.is_empty()
            {
                let n = self.factory.selection.len();
                self.snapshot_factory();
                self.factory.erase_selection();
                self.factory_note(format!("3D erase ✓ {n} solid(s)"));
                self.history.push(format!("  - erased {n} solid(s)"));
                return;
            }
            // ---- 3D COMMAND vocabulary — placement ------------------------------------
            // "we will have a 3d command window … the 2d command window's commands won't work on
            // the 3d factory or vice versa; for it to work the user has to click on the window
            // they want access to."
            if self.factory_place_command(trimmed) {
                return;
            }

            // ---- THE BOUNDARY. A 2D DRAWING command does not run against the 3D model.
            //
            // `Add` and `SetTool` are exactly the set that creates 2D geometry — line, circle,
            // wall, text and the rest. BOTH are needed: `line x1,y1 x2,y2` parses to `Add`, but a
            // bare `line` — the form people actually type — parses to `SetTool`, and matching only
            // `Add` let every interactive 2D tool straight through.
            //
            // Refusing on those two discriminators rather than a hand-kept list of names is what
            // keeps this honest as the 2D side grows: a command added there is covered here the day
            // it is added, and nothing shared (zoom, select, undo, setvar) is caught by accident.
            //
            // It REFUSES rather than silently doing nothing, and it names the way out. A command
            // that vanishes without explanation is the bug this feature exists to fix, not a
            // shape to reproduce.
            //
            // Gated on `command_target()`, not on `active_view` alone, so the refusal and the dock
            // title can never disagree — a window titled "Command" must never refuse a 2D command.
            // The enclosing block's gate is deliberately left as `active_view` ALONE: requiring
            // `factory.open` there is the change that hijacked `m` out of 2D drawing twice.
            if self.command_target() == ActiveView::ThreeD
                && matches!(
                    cad_kernel::parser::parse(trimmed),
                    Ok(cad_kernel::parser::Command::Add(_)
                        | cad_kernel::parser::Command::SetTool(_))
                )
            {
                let msg =
                    format!("'{trimmed}' is a 2D command — click the 2D window to draw with it.");
                self.history.push(format!("  ⚠ {msg}"));
                self.factory.status = msg;
                return;
            }
            // anything else falls through to the 2D dispatcher untouched
        }
        // ---- …and the same boundary the other way. A 3D word typed at the 2D command line is a
        // sign the user thinks they are talking to the Factory. Say which window owns it instead of
        // reporting "unknown command", which reads as "that is not a thing".
        //
        // Only words the 2D parser does NOT claim: where both sides recognise a name, the window
        // you are in wins. 2D is the default view, so 2D keeps its own vocabulary intact.
        else if Self::is_factory_only_command(trimmed)
            && cad_kernel::parser::parse(trimmed).is_err()
        {
            let msg =
                format!("'{trimmed}' is a 3D Factory command — click the 3D window to use it.");
            self.history.push(format!("  ⚠ {msg}"));
            return;
        }

        // ---- SYSVAR value entry (after `setvar NAME` or a bare var name) ----
        // The next input is the new value; empty keeps current; bad input
        // re-prompts (Esc cancels via the Esc handler).
        if let Some(name) = self.var_set_pending.clone() {
            if trimmed.is_empty() {
                let cur = crate::varreg::env_get(&self.env, &name).unwrap_or_default();
                self.history.push(format!("  {} kept at {}", name, cur));
                self.var_set_pending = None;
                self.clear_prompt();
            } else {
                // Numeric SYSVARs go through the calculator — `setvar TxHt`
                // then `0.3*2` sets 0.6. Text/Choice/Bool keep the raw reply.
                let vstr = self.sysvar_value_string(&name, trimmed);
                match crate::varreg::env_set(&mut self.env, &name, &vstr) {
                    Ok(_) => {
                        let _ = self.env.save();
                        let nv = crate::varreg::env_get(&self.env, &name).unwrap_or_default();
                        self.history.push(format!("  {} = {}", name, nv));
                        self.var_set_pending = None;
                        self.clear_prompt();
                    }
                    Err(e) => {
                        self.history.push(format!("  ! {}", e));
                        let cur = crate::varreg::env_get(&self.env, &name).unwrap_or_default();
                        self.set_prompt(format!(
                            "Enter new value for {} <{}>:  [Esc=cancel]",
                            name, cur
                        ));
                    }
                }
            }
            return;
        }

        // ---- Command-internal UNDO ("U" keyword) -------------------------
        // "u"/"undo" while a command is ACTIVE reverses only the last step of
        // THAT command (segment / vertex / pick / result). With nothing active
        // it falls back to the global undo. Must run BEFORE the cmd_flow and
        // parser intercepts so "u" is caught mid-circle/arc/etc. PLINE keeps
        // its own richer U handler (arc sub-flows / bulges / widths), so we
        // let "u" fall through to the Tool::Polyline block for that tool.
        if matches!(trimmed.to_ascii_lowercase().as_str(), "u" | "undo")
            && self.tool != Tool::Polyline
        {
            match self.command_internal_undo() {
                CmdUndo::Handled => return,
                CmdUndo::NothingLeft => {
                    self.history
                        .push("  ! nothing left to undo in this command — Esc cancels it".into());
                    return;
                }
                CmdUndo::NoCommand => {
                    self.do_undo();
                    return;
                }
            }
        }
        // ---- Capture command-internal-undo baselines at command start ----
        // A fresh top-level command (typed at the idle prompt) records the
        // undo-stack depth + dobject count so its own "U" can reverse only its
        // own results. Skipped for replies to an active prompt (prompt set).
        if self.current_prompt.is_empty() && !trimmed.is_empty() {
            self.cmd_undo_base = Some(self.undo_stack.len());
            self.cmd_base_objs = Some(self.doc.dobjects.len());
        }

        // ---- Prompt-driven command flow (Slice 1: CIRCLE) ----------------
        // When a flow is live the command line IS its prompt — route typed
        // input there. Otherwise `circle`/`ci` STARTS the flow. Each prompt +
        // reply lands in `self.transcript`. See COMMAND_LINE.md.
        if self.cmd_flow.is_some() {
            self.flow_input_text(trimmed);
            return;
        }
        if matches!(trimmed.to_ascii_lowercase().as_str(), "circle" | "ci")
            && self.select_mode == SelectMode::Off
        {
            self.circle_flow_start();
            return;
        }
        // ---- ZOOM command flow -------------------------------------------
        // `zoom` / `z` (optionally `zoom <opt>`) runs an AutoCAD-style
        // sub-option flow. While active the command line is captured here, so
        // sub-option letters (A/C/E/P/S/W/O) never reach the parser. Point
        // picks come in via zoom_input_point; empty Enter via update(). See
        // the zoom_* methods.
        if self.zoom_state != ZoomState::Off {
            self.zoom_input_text(trimmed);
            return;
        }
        // PEDIT — while active, the command line drives the sub-options.
        if self.pedit_state != PeditState::Off {
            self.pedit_input(trimmed);
            return;
        }
        if matches!(trimmed.to_ascii_lowercase().as_str(), "pedit" | "pe") {
            // Register as the repeatable command so empty-Enter re-runs PEDIT
            // (this intercept returns early, bypassing the parse-ok path that
            // normally sets last_command — same pattern as circle/zoom).
            self.last_command = Some("pedit".into());
            self.pedit_start();
            return;
        }
        {
            let lc = trimmed.to_ascii_lowercase();
            if lc == "zoom" || lc == "z" {
                self.zoom_start("");
                return;
            }
            if let Some(rest) = lc.strip_prefix("zoom ").or_else(|| lc.strip_prefix("z ")) {
                self.zoom_start(rest.trim());
                return;
            }
        }
        // ---- SYSVAR access: `setvar [NAME [VALUE]]` / `setvar ?`, or a bare
        // variable name typed as a command (AutoCAD-style). ------------------
        {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if let Some(&first) = parts.first() {
                if first.eq_ignore_ascii_case("setvar") {
                    let name = parts.get(1).copied();
                    let value = if parts.len() > 2 {
                        parts[2..].join(" ")
                    } else {
                        String::new()
                    };
                    self.handle_setvar(name, &value);
                    return;
                }
                // A bare variable name (optionally with an inline value).
                if crate::varreg::find(first).is_some() {
                    let value = if parts.len() > 1 {
                        parts[1..].join(" ")
                    } else {
                        String::new()
                    };
                    self.handle_setvar(Some(first), &value);
                    return;
                }
            }
        }
        // `group` / `ungroup` — operate on the current selection.
        match trimmed.to_ascii_lowercase().as_str() {
            "group" => {
                self.group_selection();
                return;
            }
            "ungroup" => {
                self.ungroup_selection();
                return;
            }
            _ => {}
        }
        // Selection sub-command shortcuts — only while the command line is
        // asking for a selection (a select session is active):
        //   p = previous selection, L = last drafted dobject, D = deselect mode.
        // Single letters are ambiguous at top level (l = Line), so they only
        // mean this inside a select session.
        if self.select_mode != SelectMode::Off {
            match trimmed.to_ascii_lowercase().as_str() {
                "p" => {
                    self.run_command("previous");
                    return;
                }
                "l" => {
                    self.run_command("last");
                    return;
                }
                "d" => {
                    self.run_command("remove");
                    return;
                }
                _ => {}
            }
        }
        // `preview [on|off]` — toggle drafting-command live preview.
        {
            let lc = trimmed.to_ascii_lowercase();
            if lc == "preview" || lc == "preview on" || lc == "preview off" {
                self.draft_preview = match lc.as_str() {
                    "preview on" => true,
                    "preview off" => false,
                    _ => !self.draft_preview,
                };
                self.history.push(format!(
                    "  drafting preview: {}",
                    if self.draft_preview { "ON" } else { "off" }
                ));
                return;
            }
        }
        // `transcript` — dump the recorded prompt↔reply log (the command
        // structure's memory; later fed to an AI/external resolver).
        if matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "transcript" | "trans"
        ) {
            if self.transcript.is_empty() {
                self.history.push("  transcript: (empty)".into());
            } else {
                self.history.push(format!(
                    "  transcript — {} exchange(s):",
                    self.transcript.len()
                ));
                for (i, pr) in self.transcript.iter().enumerate() {
                    self.history.push(format!(
                        "    {:>2}. [{}] {}  →  {}",
                        i + 1,
                        pr.cmd,
                        pr.prompt,
                        pr.reply
                    ));
                }
            }
            return;
        }

        // ---- Text-tool sub-command intercepts (must run FIRST) ----
        // The `text` tool consumes the cmd line until the user clicks
        // a position OR presses Esc. Without this intercept, typing
        // `H` (intent: set height) leaks through to the main parser
        // which matches `h` as the Hatch command — wrong tool fires.

        // (a) `text_waiting_height` was armed by a prior `H` with no
        //     argument; this input IS the height value.
        if self.text_waiting_height {
            self.text_waiting_height = false;
            if trimmed.is_empty() {
                self.history
                    .push(format!("  text: height kept at {}", self.env.TxHt));
            } else {
                match self.eval_number(trimmed) {
                    Ok(v) if v > 1e-9 => {
                        self.env.TxHt = v;
                        self.text_input_dialog_height = v;
                        let _ = self.env.save();
                        self.history
                            .push(format!("  text: height → {}", crate::calc::fmt_value(v)));
                    }
                    Ok(_) => self
                        .history
                        .push("  ! text: height must be positive".into()),
                    Err(e) => self.history.push(format!("  ! {}", e)),
                }
            }
            let p = self.text_anchor_prompt(false);
            self.set_prompt(p);
            return;
        }

        // (a2) `text_waiting_angle` was armed by a bare `angle`; this input IS
        //      the rotation angle in degrees (blank = keep).
        if self.text_waiting_angle {
            self.text_waiting_angle = false;
            if trimmed.is_empty() {
                self.history.push(format!(
                    "  text: angle kept at {}\u{00B0}",
                    self.text_dialog_angle_deg
                ));
            } else {
                match self.eval_number(trimmed) {
                    Ok(v) => {
                        self.text_dialog_angle_deg = v;
                        self.history.push(format!(
                            "  text: angle → {}\u{00B0}",
                            crate::calc::fmt_value(v)
                        ));
                    }
                    Err(e) => self.history.push(format!("  ! {}", e)),
                }
            }
            let p = self.text_anchor_prompt(false);
            self.set_prompt(p);
            return;
        }

        // (b) Text body capture — after the user clicked a position,
        //     the next non-empty input IS the text string. NOT a cmd.
        //     Skipped when the popup dialog owns body entry; otherwise
        //     both paths would race for the same keystrokes.
        if let TextDraftState::WaitingForString(pos) = self.text_draft.clone() {
            if self.text_input_dialog_open {
                // Dialog handles body — don't consume cmd input here.
                // Let the input fall through to the global parser so
                // the user can still issue commands while the dialog
                // is open (e.g. zoom, snap toggle).
            } else {
                if trimmed.is_empty() {
                    // Empty Enter — cancel the draft.
                    self.text_draft = TextDraftState::Off;
                    self.tool = Tool::None;
                    self.clear_prompt();
                    self.history.push("  text: cancelled (empty input)".into());
                    return;
                }
                let body = trimmed.to_string();
                let height = self.env.TxHt;
                self.commit_text_at(pos, &body, height);
                self.text_draft = TextDraftState::Off;
                self.tool = Tool::None;
                return;
            }
        }

        // (c) Text tool is active + waiting for a position click. Allow
        //     sub-options inline:
        //       `H`             — arm height capture (next input → value)
        //       `H 5` / `h 0.3` — set height immediately
        //       `height 5`      — long form, same as above
        //     Anything else with the text tool active just falls through
        //     to the global parser (user might want to switch tools).
        if self.text_draft == TextDraftState::WaitingForPosition {
            let lc = trimmed.to_ascii_lowercase();
            let toks: Vec<&str> = lc.split_whitespace().collect();
            let kw = toks.first().copied().unwrap_or("");
            // HEIGHT sub-command chip / keyword.
            if kw == "h" || kw == "height" {
                if toks.len() == 1 {
                    // Bare H — arm the next input as the value.
                    self.text_waiting_height = true;
                    self.set_prompt(format!("text: enter height <{}>", self.env.TxHt));
                } else {
                    // Evaluate the ORIGINAL case (toks is lowercased, but
                    // variables are case-sensitive).
                    let val = trimmed.split_whitespace().nth(1).unwrap_or("");
                    match self.eval_number(val) {
                        Ok(v) if v > 1e-9 => {
                            self.env.TxHt = v;
                            self.text_input_dialog_height = v;
                            let _ = self.env.save();
                            self.history
                                .push(format!("  text: height → {}", crate::calc::fmt_value(v)));
                        }
                        Ok(_) => self
                            .history
                            .push("  ! text: height must be positive".into()),
                        Err(e) => self.history.push(format!("  ! {}", e)),
                    }
                    let p = self.text_anchor_prompt(false);
                    self.set_prompt(p);
                }
                return;
            }
            // ANGLE sub-command chip / keyword (rotation, degrees).
            if kw == "a" || kw == "angle" {
                if toks.len() == 1 {
                    self.text_waiting_angle = true;
                    self.set_prompt(format!(
                        "text: enter angle\u{00B0} <{}>",
                        self.text_dialog_angle_deg
                    ));
                } else {
                    match toks[1].parse::<f64>() {
                        Ok(v) => {
                            self.text_dialog_angle_deg = v;
                            self.history.push(format!("  text: angle → {}\u{00B0}", v));
                        }
                        Err(_) => self
                            .history
                            .push(format!("  ! text: '{}' is not a number", toks[1])),
                    }
                    let p = self.text_anchor_prompt(false);
                    self.set_prompt(p);
                }
                return;
            }
            // CLOSE chip / keyword — end the text session (mirrors grip→Close).
            if kw == "close" {
                self.close_text_dialog(true);
                self.history.push("  text: finished".into());
                return;
            }
        }

        // ---- Rectangle width/height entry --------------------------
        // After the first corner is captured, the user can type `W H`
        // (or `W,H`) instead of clicking the opposite corner — the two
        // ways to draw a rectangle. Signed: negative width/height extend
        // left/down from the first corner. Must run before the main
        // parser, which would otherwise choke on "5 3".
        if self.tool == Tool::Rectangle && self.pending.len() == 1 {
            let toks: Vec<&str> = trimmed
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .collect();
            if toks.len() == 2 {
                // Each token evaluates independently — `W H`, `W,H`, or
                // pasted `w*2 h/2` (typed multi-token is foreclosed by
                // Space=Enter; the paste path tolerates it).
                if let (Ok(w), Ok(h)) = (self.eval_number(toks[0]), self.eval_number(toks[1])) {
                    let a = self.pending[0];
                    self.pending.clear();
                    if w.abs() < EPS || h.abs() < EPS {
                        self.history
                            .push("  ! rectangle: width and height must be non-zero".into());
                    } else {
                        let b = Vec2::new(a.x + w, a.y + h);
                        self.add_dobject(rect_polyline(a, b), "canvas (W×H)");
                        self.history.push(format!(
                            "  ▭ rectangle {}×{} from ({:.3},{:.3})",
                            w, h, a.x, a.y
                        ));
                    }
                    // Tool stays active for the next rectangle (like line).
                    self.set_prompt(current_hint(self.tool, self.arc_method, 0));
                    return;
                }
            }
            // Not two numbers — fall through (user may be switching tools).
        }

        // ---- Mirror keep-original answer ---------------------------
        // After the axis is set the user answers Y (keep a copy — the
        // default) or n/no/ni (erase the original). Must run before the
        // main parser so "no"/"n" aren't treated as commands. The empty
        // Enter = keep is handled in the empty-Enter cascade.
        if let MirrorState::AwaitingKeep(a, b) = self.mirror_state {
            let t = trimmed.to_ascii_lowercase();
            let keep = match t.as_str() {
                "" | "y" | "yes" | "keep" => Some(true),
                "n" | "no" | "ni" => Some(false),
                _ => None,
            };
            match keep {
                Some(k) => {
                    self.mirror_state = MirrorState::Off;
                    self.apply_mirror(a, b, k);
                    self.clear_prompt();
                }
                None => self.history.push(
                    "  ! mirror: answer Y (keep a copy) or n (erase original) — Esc cancels".into(),
                ),
            }
            return;
        }

        // ---- Direct distance entry (DDE) ---------------------------
        // While a line / move / copy is waiting for its NEXT point, a
        // single typed number = the distance from the anchor along the
        // current cursor DIRECTION. With CARD on that direction is the
        // locked H/V axis, so "100 ⏎" drops the point exactly 100 units
        // horizontally/vertically — the user's "type a value = second
        // point" flow. Without CARD it throws toward the raw cursor
        // (standard AutoCAD DDE). Must precede the main parser.
        {
            let dde_anchor: Option<Vec2> = if self.tool == Tool::Line && self.pending.len() == 1 {
                Some(self.pending[0])
            } else if let MoveState::WaitingForDest(base) = self.move_state {
                Some(base)
            } else if let CopyState::WaitingForDest(base) = self.copy_state {
                Some(base)
            } else if let StretchState::WaitingForDest(_, _, base) = self.stretch_state {
                Some(base)
            } else {
                None
            };
            if let Some(anchor) = dde_anchor {
                if let Ok(dist) = self.eval_number(trimmed) {
                    match self.last_cursor_raw_world {
                        Some(raw) => {
                            let constrained = self.apply_constraints(raw);
                            let dir = constrained - anchor;
                            if dir.len() > EPS {
                                let p = anchor + dir / dir.len() * dist;
                                if self.tool == Tool::Line {
                                    self.last_point = Some(p);
                                    self.pending.push(p);
                                    self.try_finalise();
                                } else if let MoveState::WaitingForDest(base) = self.move_state {
                                    self.apply_move(p - base);
                                    self.move_state = MoveState::Off;
                                    self.clear_prompt();
                                } else if let CopyState::WaitingForDest(base) = self.copy_state {
                                    self.apply_copy(p - base);
                                    self.copy_state = CopyState::Off;
                                    self.clear_prompt();
                                } else if let StretchState::WaitingForDest(wmin, wmax, base) =
                                    self.stretch_state
                                {
                                    self.apply_stretch(wmin, wmax, base, p);
                                    self.stretch_state = StretchState::Off;
                                    self.clear_prompt();
                                }
                            } else {
                                self.history.push(
                                    "  ! move the cursor to set a direction, then type the distance".into());
                            }
                        }
                        None => self.history.push(
                            "  ! hover the canvas to set a direction, then type the distance"
                                .into(),
                        ),
                    }
                    return;
                }
            }
        }

        // ---- Block Task Recorder: name the just-recorded function -------
        // After a btr stretch, the next typed line names that function.
        // `finish`/`endrec`/`done` (or an empty Enter — handled in the key
        // handler) ends the session and opens the dialog instead.
        if self.btr_awaiting_name && self.block_task_rec.is_some() {
            let low = trimmed.to_ascii_lowercase();
            if low == "finish" || low == "endrec" || low == "done" {
                self.btr_awaiting_name = false;
                self.finish_block_task_recorder();
                return;
            }
            if matches!(low.as_str(), "stretch" | "st" | "s") {
                // Record another without naming this one (name it at finish).
                self.btr_awaiting_name = false;
                // fall through to the parser → starts a new stretch.
            } else {
                self.btr_awaiting_name = false;
                if !trimmed.is_empty() {
                    if let Some(rec) = self.block_task_rec.as_mut() {
                        if let Some(last) = rec.recorded.last_mut() {
                            last.name = trimmed.to_string();
                        }
                    }
                    self.history
                        .push(format!("  ◉ function named '{}'", trimmed));
                } else {
                    self.history
                        .push("  ◉ function left unnamed (name it at finish)".into());
                }
                // Auto-re-enter stretch for the NEXT parameter — the user can
                // crossing-window again, or type `finish` to stop.
                self.start_btr_stretch();
                return;
            }
        }

        // ---- LIVE parametric insert: a typed number sets the current param ----
        // (manual command-line input alongside the drag-to-set flow).
        if self.insert_live.is_some() {
            if trimmed.is_empty() {
                return;
            } // wait for a value or a click
            let v = match self.eval_number(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    self.history
                        .push(format!("  ! insert: {} — type a value or click", e));
                    return;
                }
            };
            let place = {
                let live = self.insert_live.as_mut().unwrap();
                if live.idx < live.values.len() {
                    live.values[live.idx] = v;
                }
                live.idx += 1;
                live.idx >= live.values.len()
            };
            if place {
                let live = self.insert_live.take().unwrap();
                let mut pv = [0.0; cad_kernel::MAX_BLOCK_PARAMS];
                for (k, vv) in live.values.iter().enumerate() {
                    if k < cad_kernel::MAX_BLOCK_PARAMS {
                        pv[k] = *vv;
                    }
                }
                self.place_block_full(live.block, live.insert, live.scale, live.rotation, pv);
                self.clear_prompt();
            } else {
                let name = {
                    let lv = self.insert_live.as_ref().unwrap();
                    self.doc
                        .blocks
                        .get(lv.block)
                        .and_then(|b| b.params.get(lv.idx))
                        .map(|p| p.name.clone())
                        .unwrap_or_default()
                };
                self.set_prompt(format!(
                    "insert: drag to set '{name}', click to fix  (or type a value)  [Esc=cancel]"
                ));
            }
            return;
        }

        // ---- Insert ANGLE step: typed degrees (Enter=0 handled in key cascade) ----
        if let InsertState::WaitingForAngle { block, insert } = self.insert_state {
            let rot = match self.eval_number(trimmed) {
                Ok(deg) => deg.to_radians(),
                Err(e) => {
                    self.history.push(format!(
                        "  ! insert: {} — type degrees, click a \
                         direction, or Enter=0",
                        e
                    ));
                    return;
                }
            };
            self.insert_state = InsertState::Off;
            self.apply_insert(block, insert, rot);
            return;
        }

        // ---- Insert-time parametric value prompt (must run FIRST) ----
        // While placing a parametric block, each typed line is the value
        // for the current variable (empty = its default). On the last one,
        // the instance is placed.
        if let Some(mut p) = self.insert_param_prompt.take() {
            let idx = p.values.len();
            let val = if trimmed.is_empty() {
                p.defaults[idx]
            } else {
                match self.eval_number(trimmed) {
                    Ok(v) => v,
                    Err(e) => {
                        self.history.push(format!(
                            "  ! insert: {} — set {} [{}] or Esc",
                            e, p.names[idx], p.defaults[idx]
                        ));
                        self.insert_param_prompt = Some(p);
                        return;
                    }
                }
            };
            p.values.push(val);
            if p.values.len() < p.names.len() {
                let i = p.values.len();
                self.set_prompt(format!(
                    "insert: set {} [{}]  (Enter=default)  [Esc=cancel]",
                    p.names[i], p.defaults[i]
                ));
                self.insert_param_prompt = Some(p);
            } else {
                self.place_parametric_insert(p);
            }
            return;
        }

        // ---- Pending-sub-arg intercepts (must run FIRST) ----------
        // When a fillet/chamfer sub-option has prompted for a numeric
        // arg (e.g. user typed `r` alone), the next non-empty input
        // is consumed as that number and must NOT reach the main
        // parser. Without this, typing `2` after `r` produced
        // `unknown command '2'`.
        if self.fillet_waiting_radius {
            // Empty input keeps the current radius.
            if trimmed.is_empty() {
                let v = self.env.FltRad;
                self.fillet_state = FilletState::WaitingForFirst(v);
                self.fillet_waiting_radius = false;
                self.history.push(format!("  fillet: radius kept at {}", v));
                self.refresh_fillet_prompt();
                return;
            }
            // Lenient: accepts "2", "r=2", " 2 ", etc.
            match parse_dist_lenient(&self.calc, trimmed) {
                Some(v) => {
                    self.env.FltRad = v;
                    let _ = self.env.save();
                    self.fillet_state = FilletState::WaitingForFirst(v);
                    self.fillet_waiting_radius = false;
                    self.history.push(format!("  fillet: radius → {}", v));
                    self.refresh_fillet_prompt();
                }
                None => {
                    self.history.push(format!(
                        "  ! fillet: '{}' is not a number — type a radius (current {}) or Esc",
                        trimmed, self.env.FltRad
                    ));
                }
            }
            return;
        }
        // Chamfer two-step distance entry (AutoCAD-style):
        //   d → "first distance <X>:"  (X = ChmDs1)
        //       empty → keep, Some(v) → set, then →
        //   "second distance <d1>:"
        //       empty → d2 = d1, Some(v) → set
        // Inline `d 2 3` / `d 2,3` / `d1=2 d2=3` is handled in the
        // chamfer sub-cmd intercept further down — bypasses both
        // wait states.
        match self.chamfer_dist_wait {
            ChamferDistWait::WaitingD1 => {
                let d1 = if trimmed.is_empty() {
                    self.env.ChmDs1
                } else {
                    match parse_dist_lenient(&self.calc, trimmed) {
                        Some(v) => {
                            self.env.ChmDs1 = v;
                            let _ = self.env.save();
                            v
                        }
                        None => {
                            self.history.push(format!(
                                "  ! chamfer: '{}' is not a number (current d1={}); type a number, blank for default, or Esc",
                                trimmed, self.env.ChmDs1));
                            return;
                        }
                    }
                };
                self.chamfer_dist_wait = ChamferDistWait::WaitingD2(d1);
                self.history
                    .push(format!("  chamfer: first distance → {}", d1));
                self.set_prompt(format!(
                    "chamfer: second distance <{}> (Enter = same as first)  [Esc=cancel]",
                    d1
                ));
                return;
            }
            ChamferDistWait::WaitingD2(d1) => {
                let d2 = if trimmed.is_empty() {
                    d1
                } else {
                    match parse_dist_lenient(&self.calc, trimmed) {
                        Some(v) => v,
                        None => {
                            self.history.push(format!(
                                "  ! chamfer: '{}' is not a number; type a number, blank for d2 = d1 = {}, or Esc",
                                trimmed, d1));
                            return;
                        }
                    }
                };
                self.env.ChmDs1 = d1;
                self.env.ChmDs2 = d2;
                let _ = self.env.save();
                self.chamfer_state = ChamferState::WaitingForFirst(d1, d2);
                self.chamfer_dist_wait = ChamferDistWait::Off;
                self.history
                    .push(format!("  chamfer: distances → ({}, {})", d1, d2));
                self.refresh_chamfer_prompt();
                return;
            }
            ChamferDistWait::Off => {}
        }

        // ---- Hatch-confirm-panel cmd-line shortcuts -----------------
        // When a hatch preview is awaiting confirmation, the floating
        // panel can also be driven from the command line so the user
        // never has to leave the keyboard.
        if self.hatch_confirm_open {
            let lc = trimmed.to_ascii_lowercase();
            match lc.as_str() {
                "" | "y" | "yes" | "confirm" | "c" | "ok" => {
                    self.hatch_confirm_accept();
                    return;
                }
                "n" | "no" | "discard" | "cancel" | "x" => {
                    self.hatch_confirm_discard();
                    return;
                }
                "change" | "edit" | "ch" | "ed" | "r" | "redo" => {
                    self.hatch_confirm_change();
                    return;
                }
                "+p" | "p" | "point" | "addpoint" | "pickpoint" => {
                    self.hatch_confirm_add_point();
                    return;
                }
                "+d" | "d" | "dobj" | "dobject" | "adddobject" => {
                    self.hatch_confirm_add_dobject();
                    return;
                }
                _ => {
                    self.history.push(
                        "  ! hatch panel awaiting: type `c`onfirm / `d`iscard / `ch`ange / `p`oint / `D`object".into());
                    return;
                }
            }
        }

        // ---- XLINE / RAY sub-options (H/V/A...) while their flows are armed.
        if self.xline_state != XlineState::Off && self.xline_suboption(trimmed) {
            return;
        }
        if self.ray_state != RayState::Off && self.ray_suboption(trimmed) {
            return;
        }

        // ---- TRIM / EXTEND target phase: still the RUNNING command --------
        // Typed input is a sub-command (Fence / Window / Crossing / Undo),
        // NOT a new command — no hijack (typing `f` mustn't jump to Fillet,
        // `line` mustn't start Line).
        let in_trim_target = matches!(
            self.trim_state,
            TrimState::PickingTargets(_) | TrimState::PickingTargetsAll
        );
        let in_ext_target = matches!(
            self.extend_state,
            ExtendState::PickingTargets(_) | ExtendState::PickingTargetsAll
        );
        if in_trim_target || in_ext_target {
            match trimmed.to_ascii_lowercase().as_str() {
                "f" | "fence" => {
                    // arm a trim / extend fence
                    self.fence_armed = true;
                    self.fence_first = None;
                    self.set_prompt(if in_ext_target {
                        "extend: fence — click first point  [Esc cancels the fence]"
                    } else {
                        "trim: fence — click first point  [Esc cancels the fence]"
                    });
                    return;
                }
                "w" | "c" => {
                    // arm a two-click window / crossing box
                    self.fence_armed = false;
                    self.fence_first = None;
                    self.armed_window_inside = Some(trimmed.to_ascii_lowercase() == "w");
                    self.set_prompt(if in_ext_target {
                        "extend: window — click FIRST corner  [Esc cancels]"
                    } else {
                        "trim: window — click FIRST corner  [Esc cancels]"
                    });
                    return;
                }
                // undo/redo stay live so a bad cut can be fixed without leaving.
                "u" | "undo" | "redo" | "" => {}
                _ => {
                    // swallow — never launch a new command mid-trim/extend
                    self.history.push(if in_ext_target {
                        "  (still extending — click a target, Fence, or Enter/Esc to finish)".into()
                    } else {
                        "  (still trimming — click a target, Fence, or Enter/Esc to finish)".into()
                    });
                    return;
                }
            }
        }

        // ---- ATTDEF flow: tag → prompt → default → (click places it).
        // Each bare line feeds the next field; empty keeps default; a typed
        // `x,y` while awaiting the position commits there.
        if self.attr_def_flow != AttrDefFlow::Off {
            match self.attr_def_flow.clone() {
                AttrDefFlow::AwaitingTag => {
                    let tag = trimmed.trim().to_string();
                    if tag.is_empty() {
                        self.attr_def_flow = AttrDefFlow::Off;
                        self.clear_prompt();
                        self.history.push("  attdef: cancelled".into());
                    } else {
                        self.attr_def_flow = AttrDefFlow::AwaitingPrompt { tag: tag.clone() };
                        self.set_prompt(format!(
                            "attdef: prompt text for \"{tag}\"  (Enter = none)  [Esc cancels]"
                        ));
                    }
                    return;
                }
                AttrDefFlow::AwaitingPrompt { tag } => {
                    let prompt = trimmed.trim().to_string();
                    self.attr_def_flow = AttrDefFlow::AwaitingDefault {
                        tag: tag.clone(),
                        prompt,
                    };
                    self.set_prompt(format!(
                        "attdef: default value for \"{tag}\"  (Enter = none)  [Esc cancels]"
                    ));
                    return;
                }
                AttrDefFlow::AwaitingDefault { tag, prompt } => {
                    let default = trimmed.trim().to_string();
                    self.attr_def_flow = AttrDefFlow::AwaitingPosition {
                        tag: tag.clone(),
                        prompt: prompt.clone(),
                        default,
                    };
                    self.set_prompt(format!(
                        "attdef: click position for \"{tag}\"  [Esc cancels]"
                    ));
                    return;
                }
                AttrDefFlow::AwaitingPosition {
                    tag,
                    prompt,
                    default,
                } => {
                    if let Ok(p) = cad_kernel::parser::parse_pt(trimmed) {
                        self.commit_attdef_at(p, &tag, &prompt, &default);
                        return;
                    }
                    // Esc or a click places the def; other commands fall
                    // through (the flow stays armed until then).
                }
                AttrDefFlow::Off => {}
            }
        }

        // ---- SPLINE sub-command: Width ------------------------------------
        // One UNIFORM ribbon width for the whole spline (no per-segment taper).
        // `w`/`width` arms a one-value entry; the next typed number sets it and
        // it applies to THIS spline (and sticks as the default for the next).
        // Intercepted before the parser so the number doesn't leak to another
        // command.
        if self.tool == Tool::Spline {
            let lc = trimmed.to_ascii_lowercase();
            if self.spline_width_wait {
                if let Ok(v) = lc.parse::<f64>() {
                    self.spline_width = v.max(0.0);
                    self.spline_width_wait = false;
                    self.history.push(format!(
                        "  spline: width = {}",
                        Self::fmt_r(self.spline_width)
                    ));
                    self.set_prompt(current_hint(
                        Tool::Spline,
                        self.arc_method,
                        self.pending.len(),
                    ));
                    return;
                }
                // Not a number → cancel the wait and let the token fall
                // through to its own handler below.
                self.spline_width_wait = false;
            }
            if matches!(lc.as_str(), "w" | "width") {
                self.spline_width_wait = true;
                self.set_prompt(format!(
                    "spline: type ribbon width  <{}>  (0 = thin)",
                    Self::fmt_r(self.spline_width)
                ));
                return;
            }
        }

        // ---- Drawing Units dialog (AutoCAD DDUNITS). Bare `ddunits` opens
        // the dialog; `units` keeps the fork's parser command (unit set +
        // optional rescale).
        if trimmed.eq_ignore_ascii_case("ddunits") {
            self.open_units_dialog();
            return;
        }

        // ---- POINT style dialog: pdmode / pd / ddptype -------------------
        if trimmed.eq_ignore_ascii_case("pdmode")
            || trimmed.eq_ignore_ascii_case("pd")
            || trimmed.eq_ignore_ascii_case("ddptype")
        {
            self.point_style_picker_open = !self.point_style_picker_open;
            self.clear_prompt();
            return;
        }

        // ---- PLINE sub-command intercept (AutoCAD PLINE Line/Arc flow) ----
        //
        // While the polyline tool is active, single-letter inputs are
        // PLINE SUB-COMMANDS, not the same-letter global commands.
        // Phase 1 wires the most-used three; the remaining options
        // (W/H/Length/Center/Direction/Radius/Second-pt/Angle) print
        // "not yet wired" so the user knows they're recognised but
        // queued. The prompt mentions all of them.
        if self.tool == Tool::Polyline {
            let lc = trimmed.to_ascii_lowercase();
            // PLINE Width entry: capture starting then ending width. `h`
            // (halfwidth) doubles the entered values. Empty ending = same as
            // start (uniform). Takes priority over the letter match below.
            match self.pline_width_cap {
                PlineWidthCap::AwaitingStart { half } => {
                    if let Ok(v) = self.eval_number(trimmed) {
                        if v < 0.0 {
                            self.history.push("  ! pline width: must be ≥ 0".into());
                            return;
                        }
                        let start = if half { v * 2.0 } else { v };
                        self.pline_width_cap = PlineWidthCap::AwaitingEnd { half, start };
                        self.set_prompt(format!(
                            "pline {}: ending width <{:.4}>  [Enter = same]",
                            if half { "halfwidth" } else { "width" },
                            if half { start * 0.5 } else { start }
                        ));
                        self.refocus_cmd = true;
                        return;
                    }
                    // non-numeric → cancel width entry, fall through
                    self.pline_width_cap = PlineWidthCap::None;
                }
                PlineWidthCap::AwaitingEnd { half, start } => {
                    let end = if trimmed.is_empty() {
                        start
                    } else if let Ok(v) = self.eval_number(trimmed) {
                        if v < 0.0 {
                            self.history.push("  ! pline width: must be ≥ 0".into());
                            return;
                        }
                        if half {
                            v * 2.0
                        } else {
                            v
                        }
                    } else {
                        self.pline_width_cap = PlineWidthCap::None;
                        start
                    };
                    self.pline_next_width = (start, end);
                    self.pline_width_cap = PlineWidthCap::None;
                    self.history
                        .push(format!("  pline: width start={:.4} end={:.4}", start, end));
                    self.update_pline_prompt();
                    return;
                }
                PlineWidthCap::None => {}
            }
            // PLINE Arc Direction: a typed angle (degrees) while awaiting the
            // start tangent sets it directly (alternative to clicking a point).
            if self.pline_arc_sub == PlineArcSub::AwaitingDirection {
                if let Ok(deg) = self.eval_number(trimmed) {
                    let r = deg.to_radians();
                    self.pline_dir_override = Some(Vec2::new(r.cos(), r.sin()));
                    self.pline_arc_sub = PlineArcSub::Normal;
                    self.history.push(format!(
                        "  pline·ARC direction set to {:.1}° — click ENDPOINT",
                        deg
                    ));
                    self.update_pline_prompt();
                    return;
                }
            }
            match lc.as_str() {
                "a" | "arc" => {
                    self.pline_mode = PlineMode::Arc;
                    self.pline_arc_sub = PlineArcSub::Normal;
                    self.update_pline_prompt();
                    return;
                }
                // Close — commit the run as a CLOSED polyline (AutoCAD's `C`:
                // closes immediately and ends the run; the tool stays active
                // for a fresh polyline). Was previously missing, so `c` leaked
                // to the global Copy command.
                "c" | "close" => {
                    if self.pending.len() >= 2 {
                        let (verts, widths) = self.drain_pline_pending(true);
                        self.add_dobject(
                            Geom::Polyline(Polyline {
                                vertices: verts,
                                closed: true,
                                widths,
                            }),
                            "canvas",
                        );
                        self.update_pline_prompt();
                    } else {
                        self.history
                            .push("  ! pline: need at least 2 vertices before Close".into());
                    }
                    return;
                }
                "l" | "line" if self.pline_mode == PlineMode::Arc => {
                    self.pline_mode = PlineMode::Line;
                    self.pline_arc_sub = PlineArcSub::Normal;
                    self.update_pline_prompt();
                    return;
                }
                "u" | "undo" => {
                    // If a Second-pt flow is mid-step, undo that sub-step
                    // first instead of yanking a committed vertex.
                    if self.pline_arc_sub != PlineArcSub::Normal {
                        self.pline_arc_sub = PlineArcSub::Normal;
                        self.pline_dir_override = None;
                        self.history.push("  pline: cancelled arc sub-flow".into());
                        self.update_pline_prompt();
                        return;
                    }
                    if let Some(last) = self.pending.pop() {
                        self.pending_bulges.pop();
                        self.pending_widths.pop();
                        self.history.push(format!(
                            "  pline: removed vertex ({:.3},{:.3})",
                            last.x, last.y
                        ));
                        self.update_pline_prompt();
                    } else {
                        self.history.push("  ! pline: nothing to undo".into());
                    }
                    return;
                }
                // Second-pt: 3-click arc (start = last vertex, second pt on
                // arc, endpoint). Only meaningful in Arc mode and with at
                // least one captured vertex to start the arc from.
                "s" | "second" if self.pline_mode == PlineMode::Arc => {
                    if self.pending.is_empty() {
                        self.history
                            .push("  ! pline: need a starting vertex before Second-pt".into());
                        return;
                    }
                    self.pline_arc_sub = PlineArcSub::AwaitingSecondPt;
                    self.update_pline_prompt();
                    return;
                }
                // Direction: set the START tangent of the NEXT arc segment.
                // Then click a point (tangent = last-vertex → point) or type an
                // angle in degrees. Arc mode only; needs a starting vertex.
                "d" | "direction" if self.pline_mode == PlineMode::Arc => {
                    if self.pending.is_empty() {
                        self.history
                            .push("  ! pline: need a starting vertex before Direction".into());
                        return;
                    }
                    self.pline_arc_sub = PlineArcSub::AwaitingDirection;
                    self.update_pline_prompt();
                    return;
                }
                // Width / Halfwidth — start the (start, end) width entry flow.
                "w" | "width" | "h" | "halfwidth" => {
                    let half = lc == "h" || lc == "halfwidth";
                    self.pline_width_cap = PlineWidthCap::AwaitingStart { half };
                    let cur = if half {
                        self.pline_next_width.0 * 0.5
                    } else {
                        self.pline_next_width.0
                    };
                    self.set_prompt(format!(
                        "pline {}: starting width <{:.4}>",
                        if half { "halfwidth" } else { "width" },
                        cur
                    ));
                    self.refocus_cmd = true;
                    return;
                }
                // Recognised-but-not-wired sub-options: tell the user
                // they're known so they don't keep retyping. Wire-up
                // lands in the Phase 2 slice.
                "len" | "length" | "ce" | "center" | "r" | "radius" | "ang" | "angle" => {
                    self.history.push(format!(
                        "  pline: sub-option '{}' recognised but not yet wired (Phase 2)",
                        trimmed
                    ));
                    return;
                }
                _ => {}
            }
        }

        // ---- Fillet sub-command intercept ------------------------------
        // While fillet is active (waiting for first or second pick), the
        // user can type sub-options:
        //   r <num>  — set radius (also `r` alone re-prompts for it)
        //   t        — toggle trim mode (TrmMd SYSVAR)
        //   m        — toggle Multiple mode (loop after each fillet)
        // Anything else falls through to the normal parser.
        if matches!(
            self.fillet_state,
            FilletState::WaitingForFirst(_) | FilletState::WaitingForSecond(_, _, _)
        ) {
            let lc = trimmed.to_ascii_lowercase();
            let mut toks = lc.split_ascii_whitespace();
            match toks.next() {
                Some("t") | Some("trim") | Some("nt") | Some("notrim") => {
                    self.env.TrmMd = !self.env.TrmMd;
                    let _ = self.env.save();
                    self.history.push(format!(
                        "  fillet: trim mode → {}",
                        if self.env.TrmMd {
                            "TRIM (lines cut to arc)"
                        } else {
                            "NO TRIM (lines kept, arc added)"
                        }
                    ));
                    self.refresh_fillet_prompt();
                    return;
                }
                Some("m") | Some("multiple") => {
                    self.fillet_multiple = !self.fillet_multiple;
                    self.history.push(format!(
                        "  fillet: multiple mode → {}",
                        if self.fillet_multiple {
                            "ON (loops after each fillet, Esc to exit)"
                        } else {
                            "OFF (one fillet then exit)"
                        }
                    ));
                    self.refresh_fillet_prompt();
                    return;
                }
                Some("p") | Some("polyline") => {
                    self.fillet_poly_all = !self.fillet_poly_all;
                    self.history.push(format!(
                        "  fillet: polyline mode → {}",
                        if self.fillet_poly_all {
                            "ON (pick ONE polyline → rounds ALL its corners)"
                        } else {
                            "OFF"
                        }
                    ));
                    self.refresh_fillet_prompt();
                    return;
                }
                Some("r") | Some("radius") => {
                    // Inline form: `r 2`, `r 2.5`, `r r=3`, `r,2` all
                    // accepted via parse_dist_tokens leniency.
                    let nums = parse_dist_tokens(&self.calc, trimmed);
                    // First token is "r"/"radius" itself — it parses
                    // as None, so parse_dist_tokens returns only the
                    // numeric tail.
                    if let Some(&v) = nums.first() {
                        self.env.FltRad = v;
                        let _ = self.env.save();
                        self.fillet_state = FilletState::WaitingForFirst(v);
                        self.history.push(format!("  fillet: radius → {}", v));
                        self.refresh_fillet_prompt();
                        return;
                    }
                    // `r` alone — arm pending-radius-input. The next
                    // numeric input fires the pending-input intercept
                    // above and sets the radius.
                    self.fillet_waiting_radius = true;
                    self.set_prompt(format!(
                        "fillet: radius <{}> (Enter = keep)  [Esc=cancel]",
                        self.env.FltRad
                    ));
                    return;
                }
                _ => {}
            }
        }

        // ---- Chamfer sub-command intercept -----------------------------
        // Same shape as Fillet's. Sub-options:
        //   d <a> <b> — set distances (b defaults to a)
        //   t         — toggle trim mode (shared TrmMd)
        //   m         — toggle Multiple mode
        if matches!(
            self.chamfer_state,
            ChamferState::WaitingForFirst(_, _) | ChamferState::WaitingForSecond(_, _, _, _)
        ) {
            let lc = trimmed.to_ascii_lowercase();
            let mut toks = lc.split_ascii_whitespace();
            match toks.next() {
                Some("t") | Some("trim") | Some("nt") | Some("notrim") => {
                    self.env.TrmMd = !self.env.TrmMd;
                    let _ = self.env.save();
                    self.history.push(format!(
                        "  chamfer: trim mode → {}",
                        if self.env.TrmMd { "TRIM" } else { "NO TRIM" }
                    ));
                    self.refresh_chamfer_prompt();
                    return;
                }
                Some("m") | Some("multiple") => {
                    self.chamfer_multiple = !self.chamfer_multiple;
                    self.history.push(format!(
                        "  chamfer: multiple mode → {}",
                        if self.chamfer_multiple { "ON" } else { "OFF" }
                    ));
                    self.refresh_chamfer_prompt();
                    return;
                }
                Some("p") | Some("polyline") => {
                    self.chamfer_poly_all = !self.chamfer_poly_all;
                    self.history.push(format!(
                        "  chamfer: polyline mode → {}",
                        if self.chamfer_poly_all {
                            "ON (pick ONE polyline → bevels ALL its corners)"
                        } else {
                            "OFF"
                        }
                    ));
                    self.refresh_chamfer_prompt();
                    return;
                }
                Some("d") | Some("distance") => {
                    // Inline form, lenient about separators:
                    //   d 2 3        → (2, 3)
                    //   d 2,3        → (2, 3)
                    //   d 2          → (2, 2)  (d2 defaults to d1)
                    //   d1=2 d2=3    → (2, 3)
                    //   d 2.5 4      → (2.5, 4)
                    // parse_dist_tokens strips the leading "d"/"distance"
                    // token (returns None) and the "d1="/"d2=" prefixes.
                    let nums = parse_dist_tokens(&self.calc, trimmed);
                    if let Some(&a) = nums.first() {
                        let b = nums.get(1).copied().unwrap_or(a);
                        self.env.ChmDs1 = a;
                        self.env.ChmDs2 = b;
                        let _ = self.env.save();
                        self.chamfer_state = ChamferState::WaitingForFirst(a, b);
                        self.history
                            .push(format!("  chamfer: distances → ({}, {})", a, b));
                        self.refresh_chamfer_prompt();
                        return;
                    }
                    // `d` alone — arm AutoCAD's two-step prompt:
                    // first distance, then second. Empty input at
                    // each step keeps the current default.
                    self.chamfer_dist_wait = ChamferDistWait::WaitingD1;
                    self.set_prompt(format!(
                        "chamfer: first distance <{}> (Enter = keep)  [Esc=cancel]",
                        self.env.ChmDs1
                    ));
                    return;
                }
                _ => {}
            }
        }

        // ---- Offset sub-command intercept ------------------------------
        // Sub-options recognized while offset is active:
        //   t / through         → switch to Through-point mode
        //   e / erase           → toggle Erase-source mode
        //   l / layer           → cycle Current ↔ Source layer
        //   u / undo            → undo the last in-command offset
        //   <number>            → set distance + switch back to Distance mode
        // All only fire while offset_state != Off.
        if self.offset_state != OffsetState::Off {
            let lc = trimmed.to_ascii_lowercase();
            let mut toks = lc.split_ascii_whitespace();
            let mut toks_raw = trimmed.split_ascii_whitespace();
            match toks.next() {
                Some("t") | Some("through") => {
                    self.offset_state = OffsetState::WaitingForObject(OffsetMode::Through);
                    self.history.push(
                        "  offset: mode → THROUGH (next click after object = through-point)".into(),
                    );
                    self.refresh_offset_prompt();
                    return;
                }
                Some("e") | Some("erase") => {
                    self.offset_erase = !self.offset_erase;
                    self.history.push(format!(
                        "  offset: erase source → {}",
                        if self.offset_erase {
                            "ON (originals deleted)"
                        } else {
                            "OFF"
                        }
                    ));
                    self.refresh_offset_prompt();
                    return;
                }
                Some("l") | Some("layer") => {
                    self.offset_layer_src = !self.offset_layer_src;
                    self.history.push(format!(
                        "  offset: result layer → {}",
                        if self.offset_layer_src {
                            "SOURCE"
                        } else {
                            "CURRENT"
                        }
                    ));
                    self.refresh_offset_prompt();
                    return;
                }
                Some("u") | Some("undo") => {
                    if self.offset_applied_count == 0 {
                        self.history
                            .push("  ! offset: nothing to undo in this command".into());
                    // Offset's in-command `u` steps back through ITS OWN doc snapshots.
                    // Peek before popping: a 3D step on top is not this command's to
                    // consume, and popping it would silently discard someone's undo.
                    } else if matches!(self.undo_stack.last(), Some(UndoStep::Doc(_))) {
                        let Some(UndoStep::Doc(prev)) = self.undo_stack.pop() else {
                            unreachable!("just checked the top is a Doc step")
                        };
                        self.doc = prev;
                        self.offset_applied_count -= 1;
                        self.intersections.clear();
                        self.index_dirty = true;
                        self.touch_view();
                        self.history.push("  offset: ↺ last offset undone".into());
                        // Return to "Select object" with whatever mode
                        // was current — don't change the distance.
                        let mode = match self.offset_state {
                            OffsetState::WaitingForObject(m) => m,
                            OffsetState::WaitingForSide(m, _) => m,
                            OffsetState::Off => OffsetMode::Distance(self.env.OfsDis),
                        };
                        self.offset_state = OffsetState::WaitingForObject(mode);
                        self.refresh_offset_prompt();
                    } else {
                        self.history.push("  ! offset: undo stack empty".into());
                    }
                    return;
                }
                Some("d") | Some("distance") => {
                    // AutoCAD "D" = Distance option. Accept an inline value
                    // ("d 5") or, with none, switch to Distance mode and ask
                    // for the number (the arm below consumes it). The value
                    // is the SECOND token, in ORIGINAL case (toks is
                    // lowercased, but variables are case-sensitive).
                    if let Some(v) = toks_raw.nth(1).and_then(|s| self.eval_number(s).ok()) {
                        if v.abs() < 1e-12 {
                            self.history
                                .push("  ! offset: distance must be non-zero".into());
                        } else {
                            self.env.OfsDis = v;
                            let _ = self.env.save();
                            self.offset_state =
                                OffsetState::WaitingForObject(OffsetMode::Distance(v));
                            self.history.push(format!("  offset: distance → {}", v));
                        }
                    } else {
                        self.offset_state =
                            OffsetState::WaitingForObject(OffsetMode::Distance(self.env.OfsDis));
                        self.history.push(format!(
                            "  offset: type the offset distance  <{}>",
                            self.env.OfsDis
                        ));
                    }
                    self.refresh_offset_prompt();
                    return;
                }
                _ => {
                    // A bare number or expression ("5", "x*2", "d=2") sets
                    // the distance directly — evaluated in ORIGINAL case.
                    if let Some(raw) = toks_raw.next() {
                        if let Ok(v) = self.eval_number(raw) {
                            if v.abs() < 1e-12 {
                                self.history
                                    .push("  ! offset: distance must be non-zero".into());
                                return;
                            }
                            self.env.OfsDis = v;
                            let _ = self.env.save();
                            self.offset_state =
                                OffsetState::WaitingForObject(OffsetMode::Distance(v));
                            self.history.push(format!("  offset: distance → {}", v));
                            self.refresh_offset_prompt();
                            return;
                        }
                    }
                }
            }
        }

        // ---- Rotate sub-command intercept (AutoCAD ROTATE flow) -----
        // During WaitingForAngle: typing a NUMBER applies that rotation
        // (degrees, CCW positive); typing `r` switches to reference
        // mode; typing `c` toggles copy mode for the commit.
        if let RotateState::WaitingForAngle(pivot) = self.rotate_state {
            let lc = trimmed.to_ascii_lowercase();
            match lc.as_str() {
                "r" | "ref" | "reference" => {
                    self.rotate_state = RotateState::WaitingForRefSrc1(pivot);
                    self.set_prompt("rotate-R: click SOURCE point 1 (defines current direction)");
                    return;
                }
                "c" | "cp" | "copy" => {
                    self.rotate_copy = !self.rotate_copy;
                    self.set_prompt(format!(
                        "rotate (pivot=({:.2},{:.2})): copy {} — click to pick angle, type number, R=reference",
                        pivot.x, pivot.y,
                        if self.rotate_copy { "ON" } else { "off" }));
                    return;
                }
                _ => {
                    if let Ok(deg) = self.eval_number(trimmed) {
                        let rad = deg.to_radians();
                        self.apply_rotate_or_copy(pivot, rad);
                        self.rotate_state = RotateState::Off;
                        self.rotate_copy = false;
                        self.clear_prompt();
                        return;
                    }
                    // Not a number / sub-command: fall through to the
                    // parser (e.g. user typed `esc` or another global).
                }
            }
        }
        // ---- Scale sub-command intercept (AutoCAD SCALE flow) ------
        // Same shape as rotate: typed number = factor; `R` = reference;
        // `C` = toggle copy. WaitingForNewLength also accepts a typed
        // number as the new length (factor = new / ref_d).
        if let ScaleState::WaitingForFactor(pivot) = self.scale_state {
            let lc = trimmed.to_ascii_lowercase();
            match lc.as_str() {
                "r" | "ref" | "reference" => {
                    self.scale_state = ScaleState::WaitingForRefStart(pivot);
                    self.set_prompt("scale-R: click REFERENCE start (defines old length)");
                    return;
                }
                "c" | "cp" | "copy" => {
                    self.scale_copy = !self.scale_copy;
                    self.set_prompt(format!(
                        "scale (pivot=({:.2},{:.2})): copy {} — click for factor, type number, R=reference",
                        pivot.x, pivot.y,
                        if self.scale_copy { "ON" } else { "off" }));
                    return;
                }
                _ => {
                    if let Ok(factor) = self.eval_number(trimmed) {
                        self.apply_scale_or_copy(pivot, factor);
                        self.scale_state = ScaleState::Off;
                        self.scale_copy = false;
                        self.clear_prompt();
                        return;
                    }
                }
            }
        }
        if let ScaleState::WaitingForNewLength(pivot, ref_d) = self.scale_state {
            if let Ok(new_len) = self.eval_number(trimmed) {
                if new_len > EPS && ref_d > EPS {
                    self.apply_scale_or_copy(pivot, new_len / ref_d);
                }
                self.scale_state = ScaleState::Off;
                self.scale_copy = false;
                self.clear_prompt();
                return;
            }
        }
        // Rotate-R target step also accepts a typed angle (degrees).
        // dtheta = typed - src_angle.
        if let RotateState::WaitingForRefTgt(pivot, src_angle) = self.rotate_state {
            if let Ok(deg) = self.eval_number(trimmed) {
                let tgt = deg.to_radians();
                let mut dtheta = (tgt - src_angle).rem_euclid(std::f64::consts::TAU);
                if dtheta > std::f64::consts::PI {
                    dtheta -= std::f64::consts::TAU;
                }
                self.apply_rotate_or_copy(pivot, dtheta);
                self.rotate_state = RotateState::Off;
                self.rotate_copy = false;
                self.clear_prompt();
                return;
            }
        }
        // ---- AREA sub-option intercept (AutoCAD AREA Add/Subtract) ----
        // While an area session is live, `a`/`add` (add mode) and
        // `s`/`sub`/`subtract` flip the sign of subsequent measurements.
        // The empty-Enter fold-in is handled in update() (empty input
        // never reaches run_command).
        if self.area_state.is_some() && self.area_suboption(trimmed) {
            return;
        }
        // ---- DIVIDE / MEASURE value intercept ---------------------------
        // After the curve pick, the typed reply IS the value: a whole
        // segment COUNT (≥ 2) for divide, or a positive LENGTH for
        // measure. Anything else falls through to the parser.
        if let PtDistribState::DivideValue(idx) = self.ptdist_state {
            if let Ok(v) = self.eval_number(trimmed) {
                if v.fract().abs() < 1e-9 && v >= 2.0 {
                    self.ptdist_state = PtDistribState::Off;
                    self.apply_divmeasure_marks(idx, v, false);
                    self.clear_prompt();
                    return;
                }
                self.history
                    .push("  ! divide: the number of segments must be a whole number ≥ 2".into());
                return;
            }
        }
        if let PtDistribState::MeasureValue(idx) = self.ptdist_state {
            if let Ok(v) = self.eval_number(trimmed) {
                if v > 1e-9 {
                    self.ptdist_state = PtDistribState::Off;
                    self.apply_divmeasure_marks(idx, v, true);
                    self.clear_prompt();
                    return;
                }
                self.history
                    .push("  ! measure: the segment length must be positive".into());
                return;
            }
        }
        // ---- Selection-mode shortcut intercept ----
        //
        // While a select session is active, single-letter input is a
        // SELECTION SUB-COMMAND, not the same-letter global command.
        // See memo `feedback_rust_cad_selection_shortcuts`. We rewrite
        // the input below so the parser hands back the right Command.
        let effective = if self.select_mode != SelectMode::Off {
            let lc = raw.trim().to_ascii_lowercase();
            match lc.as_str() {
                "w" => "window".to_string(),
                "c" | "cr" => "crossing".to_string(),
                "a" => "all".to_string(),
                "b" | "bef" => "before".to_string(),
                "l" => "last".to_string(),
                "n" => "none".to_string(),
                _ => raw.to_string(),
            }
        } else {
            raw.to_string()
        };
        let parsed = parse(&effective);
        // Commit-on-interrupt: starting a real command while a PLINE/SPLINE draw
        // is in progress FINISHES it — placed vertices are never discarded.
        // (Snap overrides are mid-draw modifiers, not interrupts; pline
        // sub-commands were already consumed + returned above.)
        if matches!(self.tool, Tool::Polyline | Tool::Spline)
            && matches!(&parsed, Ok(c) if !matches!(c, Command::SnapOverride(_)))
        {
            self.commit_active_draw();
        }
        // Remember the line as the "last command" for Enter-on-empty
        // repeat — ONLY on a successful parse. Set BEFORE dispatch so
        // each Ok arm can clear / overwrite it if its own semantics
        // demand. Sub-command intercepts (PLINE, rotate, scale, …)
        // happened above and returned early; numbers / R / C typed
        // inside those sessions never reach here.
        //
        // Why "successful only": a failed line like `1` typed by
        // mistake used to overwrite last_command, so the next Enter
        // re-ran `1` and printed "unknown command '1'" forever. The
        // user's rule (2026-06-06): Enter-on-empty must repeat the
        // last VALID command, never a sub-command and never a typo.
        if !trimmed.is_empty() && parsed.is_ok() {
            self.last_command = Some(trimmed.to_string());
        }
        // Echo the canonical command name into the history "log book"
        // so a glance shows `Fillet` whether the user typed `f`, `F`,
        // or `fillet`. Only fires for real commands, not Add/SetTool
        // (those are noisy + their own history lines already cover it).
        if let Ok(ref c) = parsed {
            let canon = c.canonical_name();
            let raw_lc = trimmed.to_ascii_lowercase();
            let aliased = !raw_lc.starts_with(&canon.to_ascii_lowercase());
            let interesting = !matches!(
                c,
                Command::Add(_) | Command::SetTool(_) | Command::SnapOverride(_)
            );
            if aliased && interesting {
                self.history.push(format!("  command: {}", canon));
            }
        }
        // Phase-10 instrumentation: record any command that runs while a hatch
        // (or a hatch's boundary) is in the basket, with the doc size before
        // and after. `move`, `erase`, `copy` and friends acting on a fill are
        // where the confusing behaviour shows up, and the log used to stop at
        // creation — so the interesting half was invisible.
        if let Ok(ref c) = parsed {
            self.hatch_dbg_command_on_selection(&c.canonical_name(), trimmed);
        }
        match parsed {
            Ok(Command::Add(e)) => self.add_dobject(e, "command"),
            Ok(Command::Delete(i)) => {
                if i < self.doc.dobjects.len() {
                    let orphans = self.doc.erase_dobjects(vec![i]);
                    self.history.push(format!("  - removed #{}", i));
                    if !orphans.is_empty() {
                        self.history.push(format!(
                            "  - also removed {} orphaned hatch boundary/ies {:?}",
                            orphans.len(),
                            orphans
                        ));
                    }
                    self.selected = None;
                    self.intersections.clear();
                    self.index_dirty = true;
                } else {
                    self.history.push(format!("  ! no dobject #{}", i));
                }
            }
            Ok(Command::Clear) => {
                self.clear_all();
                self.history.push("  cleared".into());
            }
            Ok(Command::Help) => {
                for line in HELP.lines() {
                    self.history.push(format!("  {}", line));
                }
            }
            Ok(Command::SetTool(kind)) => {
                // Bare drawing keywords (`line`, `circle`, `ci`, `arc`,
                // `ellipse`, `polyline`, `point`) enter the matching draw
                // tool. The user then clicks to place points. Pending
                // points from any prior session are cleared.
                self.tool = match kind {
                    ToolKind::Line => Tool::Line,
                    ToolKind::Wall => Tool::Wall,
                    ToolKind::Text => Tool::Text,
                    ToolKind::Circle => Tool::Circle,
                    ToolKind::Arc => Tool::Arc,
                    ToolKind::Ellipse => Tool::Ellipse,
                    ToolKind::EllipseArc => Tool::EllipseArc,
                    ToolKind::Point => Tool::Point,
                    ToolKind::Polyline => Tool::Polyline,
                    ToolKind::Spline => Tool::Spline,
                    ToolKind::Rectangle => Tool::Rectangle,
                    // Merged-in RUST-AutoRASM tools: map onto the nearest
                    // existing tool so the command still does something sane.
                    ToolKind::Polygon => Tool::Polyline,
                    ToolKind::QuadBezier => Tool::Spline,
                    ToolKind::Leader => Tool::Text,
                    // ATTDEF / ATTEDIT run as state machines (tool = None);
                    // the flow prompts begin in the branch below.
                    ToolKind::AttrDef | ToolKind::AttEdit => Tool::None,
                };
                // Issue #47 — `qb`/QuadBezier drafts a QUADRATIC B-spline:
                // the spline tool at degree 2 (three control clicks P0..P2,
                // Enter commits). Any other tool switch clears the override.
                if kind == ToolKind::QuadBezier {
                    self.spline_degree_override = Some(2);
                } else {
                    self.spline_degree_override = None;
                }
                self.pending.clear();
                if self.spline_degree_override == Some(2) {
                    self.history.push(
                        "  quadbezier (qb): click P0, P1, then P2   [Enter = commit, Esc cancels]"
                            .to_string(),
                    );
                    self.set_prompt(
                        "quadbezier: click P0, P1, then P2   [Enter = commit, Esc cancels]",
                    );
                } else if kind == ToolKind::AttrDef {
                    self.attr_def_prompt();
                } else if kind == ToolKind::AttEdit {
                    self.attedit_start();
                } else {
                    self.set_prompt(current_hint(self.tool, self.arc_method, 0));
                }
            }
            Ok(Command::SnapOverride(kind)) => {
                // PER and TAN need a "from" anchor. That anchor is the draw's
                // last pending point OR the active edit op's base/pivot —
                // exactly what `card_anchor()` reports. The old guard only
                // accepted a draw pending point, so PER/TAN were refused
                // during move/copy/STRETCH/mirror/rotate point picks (the
                // reported "PER not active during stretch").
                if kind.requires_from() && self.card_anchor().is_none() {
                    self.history.push(format!(
                        "  ! {} needs an anchor — start a draw or an edit op (e.g. stretch base), then type {}",
                        kind.name(), kind.name().to_lowercase()
                    ));
                } else {
                    self.snap_override = Some(kind);
                    self.history.push(format!(
                        "  ↳ {} armed — hover the target and click",
                        kind.name()
                    ));
                }
            }
            Ok(Command::GripsToggle) => {
                self.env.GrpEnb = !self.env.GrpEnb;
                self.history.push(format!(
                    "  grips {} (GrpEnb)",
                    if self.env.GrpEnb { "ON" } else { "OFF" }
                ));
            }
            Ok(Command::List) => {
                // If something is already selected, dump full details
                // immediately. Matches AutoCAD's pickfirst semantics.
                // After dumping we clear the basket so the pulsing
                // highlight goes away (user told us 2026-06-06: nothing
                // should stay selected after the report).
                if !self.selection.is_empty() {
                    let sel = self.selection.clone();
                    self.history.push(format!(
                        "  list — {} dobject(s) currently selected:",
                        sel.len()
                    ));
                    for &i in &sel {
                        if let Some(d) = self.doc.dobjects.get(i) {
                            self.history.push(format!("    ─── #{} ───", i));
                            for line in list_full_details(&d.geom) {
                                self.history.push(format!("      {}", line));
                            }
                        }
                    }
                    self.selection_prev = self.selection.clone();
                    self.selection.clear();
                    self.selected = None;
                } else {
                    self.begin_selection(SelectMode::ForList);
                    self.history.push(
                        "  list — Select dobjects: click to add/toggle, click empty corners for window (L→R inside, R→L crossing), Enter when done (Esc cancels)".into());
                    self.history.push(
                        "         Sub-commands: all | before (re-select last) | none | remove | addmode".into());
                }
            }
            Ok(Command::Select) => {
                self.begin_selection(SelectMode::ForSelect);
                self.history.push(
                    "  select — Select dobjects: click to add/toggle, click empty corners for window (L→R inside, R→L crossing), Enter when done (Esc cancels)".into());
                self.history.push(
                    "          Sub-commands: all | before (re-select last) | none | remove | addmode".into());
            }
            // ---- Selection sub-commands (only meaningful while a session
            //      is active; gracefully reject otherwise so a stray `all`
            //      doesn't surprise the user).
            Ok(Command::SelectAll) => {
                if self.select_mode == SelectMode::Off {
                    self.history
                        .push("  ! `all` only works during a select session".into());
                } else {
                    let added = self.add_all_to_selection();
                    self.history.push(format!(
                        "    + {} dobject(s) via 'all' (current: {})",
                        added,
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::SelectPrevious) => {
                if self.select_mode == SelectMode::Off {
                    self.history
                        .push("  ! `before` only works during a select session".into());
                } else if self.selection_prev.is_empty() {
                    self.history
                        .push("  ! no previous selection to re-add".into());
                } else {
                    let mut added = 0usize;
                    let prev = self.selection_prev.clone();
                    for i in prev {
                        if i < self.doc.dobjects.len() && !self.selection.contains(&i) {
                            self.selection.push(i);
                            added += 1;
                        }
                    }
                    self.history.push(format!(
                        "    + {} dobject(s) via 'before' (current: {})",
                        added,
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::SelectNone) => {
                if self.select_mode == SelectMode::Off {
                    self.history
                        .push("  ! `none` only works during a select session".into());
                } else {
                    let n = self.selection.len();
                    self.selection.clear();
                    self.window_first = None;
                    self.history.push(format!("    – cleared {} selected", n));
                }
            }
            Ok(Command::SelectRemoveMode) => {
                if self.select_mode == SelectMode::Off {
                    self.history
                        .push("  ! `remove` only works during a select session".into());
                } else {
                    self.select_remove_mode = true;
                    self.history
                        .push("    → REMOVE mode (clicks now subtract)".into());
                }
            }
            Ok(Command::SelectAddMode) => {
                if self.select_mode == SelectMode::Off {
                    self.history
                        .push("  ! `addmode` only works during a select session".into());
                } else {
                    self.select_remove_mode = false;
                    self.history
                        .push("    → ADD mode (clicks now add/toggle)".into());
                }
            }
            Ok(Command::SelectWindow) => {
                if self.select_mode == SelectMode::Off {
                    self.history.push("  ! `window` / `w` only works during a select session (run `select` or `trim` first)".into());
                } else {
                    self.window_first = None;
                    self.armed_window_inside = Some(true); // forced inside
                    self.history.push(
                        "    window armed — click FIRST corner, then OPPOSITE. Only dobjects FULLY INSIDE the box are added.".into());
                }
            }
            Ok(Command::SelectCrossing) => {
                if self.select_mode == SelectMode::Off {
                    self.history
                        .push("  ! `crossing` / `c` only works during a select session".into());
                } else {
                    self.window_first = None;
                    self.armed_window_inside = Some(false); // forced crossing
                    self.history.push(
                        "    crossing armed — click FIRST corner, then OPPOSITE. Any dobject TOUCHING the box is added.".into());
                }
            }
            Ok(Command::SelectLast) => {
                if self.select_mode == SelectMode::Off {
                    self.history
                        .push("  ! `last` / `l` only works during a select session".into());
                } else if self.doc.dobjects.is_empty() {
                    self.history.push("  ! last: document is empty".into());
                } else {
                    let last = self.doc.dobjects.len() - 1;
                    if !self.selection.contains(&last) {
                        self.selection.push(last);
                        self.history.push(format!(
                            "    + last drawn dobject #{} added (basket: {})",
                            last,
                            self.selection.len()
                        ));
                    } else {
                        self.history.push(format!(
                            "    last drawn dobject #{} already in basket",
                            last
                        ));
                    }
                }
            }
            Ok(Command::Open(path)) => {
                self.do_open(&path);
            }
            Ok(Command::SaveAs(path)) => {
                self.do_save(&path);
            }
            Ok(Command::Copy) => {
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Copy;
                    self.history.push(
                        "  copy — Select dobjects to copy, Enter to continue (Esc cancels)".into(),
                    );
                } else {
                    self.copy_state = CopyState::WaitingForBase;
                    self.history.push(format!(
                        "  copy — {} dobject(s) selected. Click BASE point (Esc cancels)",
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::Rotate) => {
                self.rotate_copy = false;
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Rotate;
                    self.set_prompt("rotate: select dobjects, Enter to continue  [Esc=cancel]");
                } else {
                    self.rotate_state = RotateState::WaitingForPivot;
                    self.set_prompt(format!(
                        "rotate ({} dobject(s)): click PIVOT point  [Esc=cancel]",
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::Scale) => {
                self.scale_copy = false;
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Scale;
                    self.set_prompt("scale: select dobjects, Enter to continue  [Esc=cancel]");
                } else {
                    self.scale_state = ScaleState::WaitingForPivot;
                    self.set_prompt(format!(
                        "scale ({} dobject(s)): click PIVOT (base point)  [Esc=cancel]",
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::Mirror) => {
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Mirror;
                    self.history
                        .push("  mirror — Select dobjects, Enter to continue (Esc cancels)".into());
                } else {
                    self.mirror_state = MirrorState::WaitingForA;
                    self.history.push(format!(
                        "  mirror — {} dobject(s) selected. Click FIRST axis point",
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::DeleteSelected) => {
                // AutoCAD ERASE flow: if basket is empty, enter select
                // mode; on Enter the queued Erase op resumes and runs
                // the actual delete. If basket is non-empty, delete
                // immediately (pickfirst).
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Erase;
                    self.set_prompt(
                        "erase: select dobjects to delete, Enter to commit  [Esc=cancel]",
                    );
                } else {
                    self.snapshot_doc();
                    // Remove from highest index downward so earlier indices stay valid.
                    let mut sorted = self.selection.clone();
                    sorted.sort_unstable();
                    sorted.dedup();
                    let n = sorted.len();
                    // OOPS buffer: the erased dobjects, in doc order, so the
                    // `oops` command can restore the LAST erase independent of
                    // undo. (Aux boundaries swept below are NOT buffered.)
                    self.last_erased = sorted
                        .iter()
                        .filter_map(|&idx| self.doc.dobjects.get(idx).cloned())
                        .collect();
                    // Erasing a HATCH must also erase the invisible auxiliary
                    // boundary it owns — shared rule for every delete path
                    // (`erase_dobjects` in cad_kernel/document.rs; one
                    // handle→index pass, O(N + refs)). Only `hatch_aux`
                    // boundaries are swept, and only when no OTHER surviving
                    // hatch still references them. Otherwise the boundary
                    // stays unreachable garbage — and a closed region the
                    // pick-point scan could still choose, so the NEXT hatch
                    // in that area silently reused the deleted hatch's
                    // outline instead of the polyline the user drew.
                    let orphans = self.doc.erase_dobjects(sorted);
                    if !orphans.is_empty() {
                        self.hatch_dbg(format!(
                            "--- [10] erase: also removing {} orphaned hatch boundary/ies {:?}",
                            orphans.len(),
                            orphans
                        ));
                    }
                    self.selection_prev = self.selection.clone();
                    self.selection.clear();
                    self.selected = None;
                    self.intersections.clear();
                    self.index_dirty = true;
                    self.touch_view();
                    self.history.push(format!("  - erased {} dobject(s)", n));
                }
            }
            Ok(Command::Oops) => {
                // Restore the last-erased dobjects (independent of undo).
                if self.last_erased.is_empty() {
                    self.fail_op("oops: nothing to restore — nothing erased yet");
                } else {
                    self.snapshot_doc();
                    let n = self.last_erased.len();
                    let mut reinserted: Vec<DObject> = std::mem::take(&mut self.last_erased);
                    self.doc.dobjects.append(&mut reinserted);
                    self.intersections.clear();
                    self.index_dirty = true;
                    self.gpu_dirty = true;
                    self.history
                        .push(format!("  ↺ oops: restored {n} erased dobject(s)"));
                }
            }
            Ok(Command::Undo) => self.do_undo(),
            Ok(Command::Redo) => self.do_redo(),
            Ok(Command::MatchProps) => {
                // AutoCAD flow: pick SOURCE (1), then paint TARGETS (many).
                // The current selection is PRESERVED as the "bank" — after the
                // source is picked, `B` applies to it in one shot.
                self.matchprops_state = MatchPropsState::WaitingForSource;
                self.set_prompt("matchprop: click the SOURCE object  [Esc=cancel]");
                let n = self.selection.len();
                if n > 0 {
                    self.history.push(format!(
                        "  matchprop — click the SOURCE object  ({} in bank; apply with B after the source)", n));
                } else {
                    self.history
                        .push("  matchprop — click the SOURCE object".into());
                }
            }
            Ok(Command::Reverse) => self.apply_reverse(),
            Ok(Command::ChangeLayer) => self.apply_chlayer(),
            Ok(Command::Layers) => {
                // AutoCAD LAYER / LA — open the Layer Properties Manager.
                self.layer_panel_open = true;
            }
            Ok(Command::ChProp(arg)) => {
                match arg {
                    None => {
                        // Bare `props` / `properties` / `chprop` → open the
                        // DObject Properties dialog (edits the current
                        // selection — single or group). The `chprop <what>
                        // <val>` CLI form below still does a scripted set.
                        self.info_window_open = true;
                        let n = if !self.selection.is_empty() {
                            self.selection.len()
                        } else if self.selected.is_some() {
                            1
                        } else {
                            0
                        };
                        self.history.push(if n == 0 {
                            "  Inspector — select dobject(s) to edit".into()
                        } else {
                            format!("  Inspector — editing {} dobject(s)", n)
                        });
                    }
                    Some((prop, val)) => {
                        if self.selection.is_empty() {
                            self.begin_selection(SelectMode::ForSelect);
                            self.queued_op = QueuedOp::ChProp(prop.clone(), val.clone());
                            self.set_prompt(format!(
                                "chprop {}={}: pick dobjects, Enter to apply  [Esc=cancel]",
                                prop, val
                            ));
                        } else {
                            self.apply_chprop(&prop, &val);
                        }
                    }
                }
            }
            Ok(Command::Linetype(name_opt)) => {
                // `linetype`        → list available linetypes
                // `linetype <name>` → set the ACTIVE layer's linetype
                //                     (case-insensitive lookup)
                match name_opt {
                    None => {
                        let names: Vec<String> = self
                            .doc
                            .linetypes
                            .linetypes
                            .iter()
                            .enumerate()
                            .map(|(i, lt)| format!("  {} {} — {}", i, lt.name, lt.description))
                            .collect();
                        self.history
                            .push(format!("  linetypes ({} available):", names.len()));
                        for n in names {
                            self.history.push(n);
                        }
                    }
                    Some(name) => match self.doc.linetypes.find(&name) {
                        Some(id) => {
                            let active = self.doc.layers.active;
                            if let Some(l) = self.doc.layers.get_mut(active) {
                                l.linetype = id;
                                self.history.push(format!(
                                    "  layer '{}' linetype → '{}' (id {})",
                                    l.name, name, id
                                ));
                                self.touch_view();
                            }
                        }
                        None => {
                            let available: Vec<&str> = self
                                .doc
                                .linetypes
                                .linetypes
                                .iter()
                                .map(|l| l.name.as_str())
                                .collect();
                            self.history.push(format!(
                                "  ! linetype '{}' not found — try one of: {}",
                                name,
                                available.join(", ")
                            ));
                        }
                    },
                }
            }
            Ok(Command::DbgRecorder) => {
                // Toggle the recorder window's visibility. Pure UI —
                // does NOT auto-start a recording (use the Start button
                // inside the window).
                self.dbg_window_open = !self.dbg_window_open;
                self.history.push(format!(
                    "  🛰 Session Recorder window {}",
                    if self.dbg_window_open {
                        "OPENED"
                    } else {
                        "CLOSED"
                    }
                ));
            }
            Ok(Command::TextStyle(name_opt)) => {
                // Open the New / Edit Text Style dialog. With a name,
                // try to find the existing style and edit it; falling
                // back to a blank new-style form if the name doesn't
                // match anything.
                let dialog = match name_opt {
                    Some(ref name) => match self.doc.text_styles.find(name) {
                        Some(id) => {
                            let style = self.doc.text_styles.get(id).unwrap();
                            TextStyleDialog::from_existing(id, style)
                        }
                        None => {
                            let mut d = TextStyleDialog::new_blank();
                            d.name = name.clone();
                            d
                        }
                    },
                    None => TextStyleDialog::new_blank(),
                };
                self.text_style_dialog = Some(dialog);
                // Surface in history so the user sees the command was
                // received and the dialog SHOULD be visible.
                self.history.push(format!(
                    "  text style dialog opened ({})",
                    match name_opt {
                        Some(n) => format!("editing '{}'", n),
                        None => "new style".into(),
                    }
                ));
            }
            Ok(Command::Text(s_opt)) => {
                // `text "Hello"` → enter draft, capture string immediately,
                // wait for ONE click to commit Text at that anchor with
                // the persisted height.
                // `text`          → enter draft, prompt for position first
                // then capture next non-empty cmd as the string.
                // `H` sub-option during the draft sets height (intercepted
                // before the global parser so it doesn't collide with
                // the Hatch command's `h` alias).
                self.tool = Tool::Text;
                self.pending.clear();
                self.text_draft = TextDraftState::WaitingForPosition;
                self.text_waiting_height = false;
                self.text_waiting_angle = false;
                if let Some(string) = s_opt {
                    self.pending_text = Some(string.clone());
                    self.set_prompt(format!(
                        "text: click anchor to place \"{}\"   h={}  a={}\u{00B0}   [ Height / Angle ]",
                        string, self.env.TxHt, self.text_dialog_angle_deg));
                } else {
                    self.pending_text = None;
                    // Dockable command dialogs are MUTUALLY EXCLUSIVE — close the
                    // Hatch panel before opening Text (two open at once caused a
                    // hang; see TEXT_DIALOG_DESIGN.md). No-op if it isn't open.
                    if self.hatch_dialog_open {
                        self.hatch_dialog_open = false;
                    }
                    // Open the dockable Text editor panel (TEXT_DIALOG_DESIGN.md).
                    // It stays open across placements; the first canvas click sets
                    // the anchor, then the user types + Apply. Seed style/height/
                    // oblique from the sticky style so the panel opens configured.
                    self.text_input_dialog_open = true;
                    self.text_input_dialog_anchor = None;
                    // Seed the dialog height from the sticky TxHt so the pill note,
                    // the dialog field, and the placed text all agree from the start.
                    self.text_input_dialog_height = self.env.TxHt.max(1e-3);
                    // The write box takes focus only AFTER the anchor click
                    // (WaitingForString). Before that, the command line owns the
                    // keyboard so Height/Angle sub-commands can be typed.
                    self.text_input_dialog_focus = false;
                    self.text_input_dialog_buf.clear();
                    if (self.text_input_dialog_style_id as usize) >= self.doc.text_styles.len() {
                        self.text_input_dialog_style_id = cad_kernel::TextStyleTable::STANDARD;
                    }
                    let sid = self.text_input_dialog_style_id;
                    self.seed_text_dialog_from_style(sid);
                    let p = self.text_anchor_prompt(false);
                    self.set_prompt(p);
                }
            }
            Ok(Command::Dim) => {
                // Smart-dim: enter Tool::Dim, wait for first click.
                // Sub-kind (Linear / Radius / Diameter) decided when
                // the user clicks: hit a circle/arc → Radius (D to
                // toggle Diameter); hit empty → Linear waiting for p2.
                self.tool = Tool::Dim;
                self.pending.clear();
                self.dim_draft = DimDraftState::WaitingForP1;
                self.set_prompt(
                    "dim: click first point (or click a circle/arc for radius/diameter)  [Esc cancels]"
                    .to_string());
            }
            Ok(Command::Boundary) => {
                self.tool = Tool::None;
                self.boundary_state = BoundaryState::WaitingForPick;
                self.set_prompt("boundary: click INSIDE a closed region  [Esc exits]".to_string());
            }
            Ok(Command::Region) => {
                self.tool = Tool::None;
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Region;
                    self.set_prompt(
                        "region: select closed curves  [Enter=convert  Esc=cancel]".to_string(),
                    );
                } else {
                    self.apply_region();
                }
            }
            Ok(Command::QDim) => self.apply_qdim(),
            Ok(Command::QSelect) => {
                self.tool = Tool::None;
                self.qselect.open = true;
            }
            Ok(Command::DimStyle(name_opt)) => {
                // Open the Dimension Style Manager (the `dimstyle` page).
                // With a name, pre-select that style in the list (and
                // make it current) if it exists.
                if let Some(ref name) = name_opt {
                    if let Some(id) = self.doc.dim_styles.find(name) {
                        self.dim_style_manager_sel = id;
                        self.current_dim_style = id;
                    }
                } else {
                    // Default the selection to whatever is current.
                    self.dim_style_manager_sel = self.current_dim_style;
                }
                self.dim_style_manager_open = true;
                self.history.push("  Dimension Style Manager opened".into());
            }
            Ok(Command::WallStyle(name_opt)) => {
                // Open the Wall Style Manager (Dry Wall / Structural / …).
                if let Some(ref name) = name_opt {
                    if let Some(id) = self.doc.wall_styles.find(name) {
                        self.wall_style_manager_sel = id;
                        self.current_wall_style = id;
                    }
                } else {
                    self.wall_style_manager_sel = self.current_wall_style;
                }
                self.wall_style_manager_open = true;
                self.history.push("  Wall Style Manager opened".into());
            }
            Ok(Command::BlockDef(name_opt)) => {
                // `block <name>` — create a definition from the selection.
                // Bare `block` opens the Block dialog to collect the name.
                match name_opt {
                    None => {
                        // Default base = the selection's gravity centre, so an
                        // undefined insertion point lands the block on its centre.
                        let base = self.selection_centroid();
                        self.block_dialog = Some(BlockDialog::new(base));
                        self.history.push("  Block dialog opened".into());
                    }
                    Some(name) => {
                        self.start_block_def(name.clone());
                    }
                }
            }
            Ok(Command::Insert(name_opt)) => {
                let names: Vec<String> = self
                    .doc
                    .blocks
                    .blocks
                    .iter()
                    .map(|b| b.name.clone())
                    .collect();
                match name_opt {
                    None => {
                        if names.is_empty() {
                            self.history.push(
                                "  ! insert: no blocks defined — create one with `block <name>`"
                                    .into(),
                            );
                        } else {
                            // Bare `insert` → open the Insert Block dialog.
                            self.insert_dialog = Some(InsertDialog::new(Some(0)));
                        }
                    }
                    Some(ref name) => match self.doc.blocks.find(name) {
                        Some(id) => {
                            // Open the Insert dialog with this block preselected —
                            // it shows scale/rotation/point AND, for a smart block,
                            // a field per parameter (the "vector" values).
                            self.insert_dialog = Some(InsertDialog::new(Some(id)));
                        }
                        None => {
                            self.history.push(format!(
                                "  ! insert: no block named '{}'  (available: {})",
                                name,
                                if names.is_empty() {
                                    "none".to_string()
                                } else {
                                    names.join(", ")
                                }
                            ));
                        }
                    },
                }
            }
            Ok(Command::Card(set_opt)) => {
                // CARD — same state F8 and the status badge flip.
                self.env.CrdEnb = match set_opt {
                    Some(v) => v,
                    None => !self.env.CrdEnb,
                };
                let _ = self.env.save();
                self.history.push(format!(
                    "  CARD {}",
                    if self.env.CrdEnb { "on" } else { "off" }
                ));
            }
            Ok(Command::Units(set_opt, rescale)) => {
                // UNITS — declare what one drawing unit means. This NEVER moves geometry: the
                // numbers in the drawing are untouched, only their interpretation at the 2D→3D
                // boundaries changes. So it is safe to set, safe to set back, and a mistake
                // costs nothing but a re-promote.
                match set_opt {
                    None => {
                        let u = self.doc.units.clone();
                        let how = match u.source {
                            cad_kernel::UnitSource::Assumed => "assumed (never set)",
                            cad_kernel::UnitSource::Declared => "declared by the file",
                            cad_kernel::UnitSource::User => "set by you",
                        };
                        self.history.push(format!(
                            "  UNITS: 1 drawing unit = {} ({}) — {}",
                            u.label(),
                            u.metres_per_unit,
                            how
                        ));
                        self.history.push(
                            "  set with: units mm | cm | m | in | ft   (affects 3D scale only)"
                                .into(),
                        );
                        self.history.push(
                            "  add `rescale` to also resize the 3D model already built".into(),
                        );
                    }
                    Some(k) => {
                        self.snapshot_doc();
                        let was = self.doc.units.clone();
                        self.doc.units = cad_kernel::Units::from_metres_per_unit(
                            k,
                            cad_kernel::UnitSource::User,
                        );
                        if rescale {
                            // The RATIO between the old and new reading of the same drawing
                            // numbers. A model built while the drawing was read as metres
                            // (k_old = 1) but which is really millimetres (k_new = 0.001) is
                            // 1000x too big, so it wants x0.001 — i.e. k_new / k_old.
                            let ratio = (k / was.metres_per_unit) as f32;
                            self.snapshot_factory();
                            self.factory.rescale_world(ratio);
                            self.factory.fit();
                            self.history.push(format!(
                                "  UNITS: 1 drawing unit = {} (was {}) — and the 3D model was \
                                 rescaled x{ratio} to match (Ctrl+Z undoes it)",
                                self.doc.units.label(),
                                was.label()
                            ));
                        } else {
                            self.history.push(format!(
                            "  UNITS: 1 drawing unit = {} (was {}) — existing 3D solids are NOT \
                             rescaled; re-promote, or `units {} rescale`, to bring them along",
                            self.doc.units.label(), was.label(), self.doc.units.label()));
                        }
                        // SET, AND THEN SANITY-CHECKED. Typing a unit is the other way a drawing
                        // ends up claiming a scale it does not have -- the gym plan was `(User)`,
                        // not declared by its file.
                        if let Some(w) = self.unit_sanity_note() {
                            self.history.push(w);
                        }
                    }
                }
            }
            Ok(Command::Diag) => {
                let report = self.factory_geometry_report();
                for line in &report {
                    self.history.push(line.clone());
                }
                // Also into the RECORDER, so a session dump carries it — that is the channel
                // this project already uses to get state out, and it saves copying the panel.
                let n = self.factory.model.features.len();
                self.factory_op_evt("diag", "command", report.join(" | "), n);
                self.factory.status = report
                    .iter()
                    .find(|l| l.contains("OVERLAPPING"))
                    .cloned()
                    .unwrap_or_else(|| "geometry report written to the history panel".into());
            }
            Ok(Command::Scene) => {
                // Into the RECORDER whether or not it is running: when it is, the capture lands
                // in the dump; when it is not, `push` is a no-op and the history panel below is
                // the whole output. Either way the user gets it without arming anything first.
                let evt = self.factory_scene_capture("`scene` command");
                let text = crate::dbg_recorder::format_event_oneline(&evt);
                self.dbg.push(evt, std::panic::Location::caller());
                for line in text.lines() {
                    self.history.push(format!("  {}", line.trim_end()));
                }
                self.factory.status = if self.dbg.recording {
                    "3D scene captured — in the history panel AND the running recording".into()
                } else {
                    "3D scene written to the history panel — press Start in the Recorder to \
                     capture it into a dump too"
                        .into()
                };
            }
            // FITTINGS THAT STAND NOWHERE NEAR THE BUILDING — report, and remove on `purge`.
            //
            // Reported as: *"the report is showing wrong number of lights theres only 31 lights in
            // simlux file."* There were 68 in `light.luminaires`: the 31 real ones, and 37 stranded
            // at x ≈ 3.5 — a thousandth of the building's coordinates, left behind by the units
            // declaration that read a metre drawing as millimetres. They are 3.5 km from the plan,
            // so no light of theirs ever reaches it and every lux figure was right; but they are
            // counted, and they carried the schedule to 68 fittings and 1360 W against the true 31
            // and 620 W. A power density on a report is a number somebody signs.
            //
            // THE TEST IS DISTANCE, NOT PROVENANCE. A fitting with no plan symbol is perfectly
            // ordinary — that is what placing one by hand produces — so "has no symbol" would
            // delete legitimate work. Standing kilometres outside the model is not ambiguous.
            Ok(Command::StrayLights(purge)) => {
                let strays = self.stray_light_ids();
                if strays.is_empty() {
                    self.history.push(format!(
                        "  straylights: none — all {} fitting(s) stand within the building",
                        self.light.luminaires.len(),
                    ));
                } else if purge {
                    self.light.stage_undo();
                    let before = self.light.luminaires.len();
                    self.light.luminaires.retain(|l| !strays.contains(&l.id));
                    self.light.selected.retain(|s| !strays.contains(s));
                    for id in &strays {
                        self.light.symbol_of.remove(id);
                    }
                    self.light.results_stale = true;
                    self.history.push(format!(
                        "  straylights: removed {} of {} fitting(s) — {} left. Re-run Calculate; \
                         Ctrl+Z undoes it.",
                        strays.len(),
                        before,
                        self.light.luminaires.len(),
                    ));
                    self.touch_view();
                } else {
                    // NAMED, NOT ACTED ON. Deleting fittings is not something a report command
                    // should do because somebody typed it to look.
                    self.history.push(format!(
                        "  ⚠ straylights: {} of {} fitting(s) stand outside the building and are \
                         counted in the schedule anyway. `straylights purge` removes them.",
                        strays.len(),
                        self.light.luminaires.len(),
                    ));
                    for id in strays.iter().take(4) {
                        if let Some(l) = self.light.luminaires.iter().find(|l| l.id == *id) {
                            self.history.push(format!(
                                "      #{id} at ({:.2}, {:.2}, {:.2})",
                                l.position.x, l.position.y, l.position.z,
                            ));
                        }
                    }
                    if strays.len() > 4 {
                        self.history
                            .push(format!("      … and {} more", strays.len() - 4));
                    }
                }
            }
            Ok(Command::RepairCuts) => {
                self.snapshot_factory();
                // MEASURE THE SOLID FIRST. A repair that edits booleans can carve away far more
                // than it opens, and a version of this command shipped that removed 30% of a real
                // building — "verified", at the time, with the same heuristic that had done the
                // editing, so the check could only ever agree with itself. The evaluated mesh is
                // the one witness independent of whatever the repair believed about the model.
                let tris_before = self.factory.model.eval().tri_count();
                let (fixed, notes) = self.factory_repair_shallow_cuts();
                let skipped = notes.len();
                // Deepening a stopped opening cuts a reveal, which ADDS surfaces. It has no
                // business destroying the building, so losing a tenth of it means the repair was
                // wrong about something — put it back rather than hand over a wrecked model.
                if fixed > 0 {
                    let tris_after = self.factory.model.eval().tri_count();
                    if tris_after * 10 < tris_before * 9 {
                        self.do_undo();
                        self.factory.status = format!(
                            "repair ABANDONED — it would have cut the model from {tris_before} to \
                             {tris_after} triangles. Nothing was changed."
                        );
                        self.history.push(format!(
                            "  repaircuts: REFUSED — {tris_before} → {tris_after} triangles. \
                             Deepening an opening cannot destroy geometry, so a measurement was \
                             wrong somewhere. The model is untouched."
                        ));
                        return;
                    }
                }
                if fixed == 0 {
                    self.undo_stack.pop();
                    self.factory.status = if skipped > 0 {
                        format!(
                            "{skipped} shallow cut(s) found, but none could be attributed to \
                                 a wall — see the history panel for each"
                        )
                    } else {
                        "no shallow cuts found — every opening goes through".into()
                    };
                } else {
                    self.factory.recompute();
                    self.factory.status = format!(
                        "{fixed} opening(s) re-cut through the full wall — Ctrl+Z undoes it"
                    );
                    self.history.push(format!(
                        "  repaircuts: {fixed} opening(s) deepened{}",
                        if skipped > 0 {
                            format!(", {skipped} left alone")
                        } else {
                            String::new()
                        }
                    ));
                }
                // Name every one left alone, with the distance that decided it. A bare count is
                // not actionable; these have to be redrawn by hand and the user needs to know
                // WHICH, and why the repair would not have been safe.
                if skipped > 0 {
                    self.history.push(format!(
                        "  {skipped} opening(s) left alone — the repair only deepens a cut it can \
                         attribute to a wall, because guessing punches holes in the wrong one:"
                    ));
                    for n in &notes {
                        self.history.push(n.clone());
                    }
                }
            }
            Ok(Command::Dedupe) => {
                self.snapshot_factory();
                let (removed, kept_cut) = self.factory_dedupe_solids();
                if removed == 0 {
                    self.undo_stack.pop();
                    self.factory.status = if kept_cut > 0 {
                        format!(
                            "no duplicates removed — {kept_cut} carry their own cuts, so \
                                 deleting them would take the cuts with them; remove those by hand"
                        )
                    } else {
                        "no duplicate solids found".into()
                    };
                } else {
                    self.factory.recompute();
                    self.factory.status =
                        format!("{removed} duplicate solid(s) deleted — Ctrl+Z puts them back");
                    self.history
                        .push(format!("  dedupe: {removed} duplicate solid(s) deleted"));
                    if kept_cut > 0 {
                        self.history.push(format!(
                            "  dedupe: {kept_cut} duplicate(s) LEFT because they carry cuts"
                        ));
                    }
                }
            }
            Ok(Command::Explode) => {
                // Select-first like erase: empty basket queues the op.
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Explode;
                    self.set_prompt(
                        "explode: select blocks / walls / polylines, Enter to apply  [Esc=cancel]",
                    );
                } else {
                    self.apply_explode();
                }
            }
            Ok(Command::BlockDiff(arg)) => match arg {
                Some((name_a, name_b)) => self.run_block_diff(&name_a, &name_b),
                None => self.begin_block_diff_pick(),
            },
            Ok(Command::BlockTaskRecorder) => {
                // `btr` now opens the ISOLATED Block Editor on the selected
                // block instance's definition (replaces the old explode-to-
                // main-screen recorder — all editing stays in the dialog).
                let bid = self
                    .selection
                    .iter()
                    .copied()
                    .chain(self.selected)
                    .filter_map(|i| match self.doc.dobjects.get(i).map(|d| &d.geom) {
                        Some(Geom::BlockRef(b)) => Some(b.block),
                        _ => None,
                    })
                    .next();
                match bid {
                    Some(id) => self.open_block_editor(id),
                    None => self.history.push(
                        "  ! Block Editor: select a block instance first, \
                         or use ‘Edit ▶’ in the Block dialog"
                            .into(),
                    ),
                }
            }
            Ok(Command::BlockTaskFinish) => self.finish_block_task_recorder(),
            Ok(Command::Wall(t_opt)) => {
                // Persist thickness if user supplied one, then enter
                // the Wall drafting tool. Two clicks → two side lines
                // via cad_kernel::wall_sides.
                if let Some(v) = t_opt {
                    if (self.env.WlThk - v).abs() > 1e-12 {
                        self.env.WlThk = v;
                        let _ = self.env.save();
                    }
                }
                self.tool = Tool::Wall;
                self.pending.clear();
                self.history.push(format!(
                    "  wall: thickness {} — click first centerline endpoint  [Esc cancels]",
                    self.env.WlThk
                ));
            }
            Ok(Command::Offset(d_opt)) => {
                // AutoCAD-style flow:
                //   offset            → enter with persisted distance
                //                       (env.OfsDis); user picks objects
                //                       one-at-a-time inside the command
                //   offset <d>        → set + persist distance, same flow
                // Sub-options (t/e/l/u) handled in the run_command
                // intercept above. Esc / Enter at WaitingForObject exits.
                if let Some(v) = d_opt {
                    if (self.env.OfsDis - v).abs() > 1e-12 {
                        self.env.OfsDis = v;
                        let _ = self.env.save();
                    }
                }
                let d = self.env.OfsDis;
                // Transient command-scoped flags start clean each invocation.
                self.offset_erase = false;
                self.offset_layer_src = false;
                self.offset_applied_count = 0;
                self.offset_state = OffsetState::WaitingForObject(OffsetMode::Distance(d));
                self.refresh_offset_prompt();
            }
            Ok(Command::Lengthen(d)) => {
                if self.selection.is_empty() {
                    self.history
                        .push("  ! lengthen: empty basket — `select` first".into());
                } else {
                    self.lengthen_state = LengthenState::WaitingForSide(d);
                    self.history.push(format!(
                        "  lengthen — delta {:.3}; {} in basket. Click END to extend (Esc cancels)",
                        d,
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::Break) => {
                if self.selection.is_empty() {
                    self.history
                        .push("  ! break: empty basket — `select` first".into());
                } else {
                    self.break_state = BreakState::WaitingForPoint;
                    self.history.push(format!(
                        "  break — {} in basket. Click CUT point (Esc cancels)",
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::Align) => {
                // Universal selection model: empty basket → open a select
                // session and queue the op (same as Move/Copy/Rotate);
                // pre-selected basket skips straight to point capture.
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Align;
                    self.set_prompt("align: select dobjects, Enter to continue  [Esc=cancel]");
                } else {
                    self.align_state = AlignState::WaitingForSrc1;
                    self.set_prompt(format!(
                        "align ({} dobject(s)): click SOURCE point 1  [Esc=cancel]",
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::Stretch) => {
                // Drop any active draw tool so its click-only semantics don't
                // hijack the crossing-window drag.
                self.tool = Tool::None;
                self.pending.clear();
                self.stretch_window_box = None;
                if self.selection.is_empty() {
                    // Select-during: crossing window picks the objects; Shift
                    // excludes; Enter finishes → base/dest. The crossing box
                    // is captured for the per-vertex test.
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Stretch;
                    self.set_prompt(
                        "stretch: crossing window to select (Shift-click excludes), \
                         Enter when done  [Esc=cancel]",
                    );
                } else {
                    // Select-first (pickfirst): no crossing box → the whole
                    // selection's bbox is the test region (whole-object move).
                    let (mn, mx) = self.selection_bbox();
                    self.stretch_state = StretchState::WaitingForBase(mn, mx);
                    self.set_prompt("stretch: click BASE point  [Esc=cancel]");
                }
            }
            Ok(Command::Trim) => {
                self.pre_op_selection = std::mem::take(&mut self.selection);
                self.trim_dbg_session_start("TRIM");
                self.trim_state = TrimState::SelectingCutters;
                self.begin_selection(SelectMode::ForCuttingEdges);
                self.set_prompt(
                    "trim: pick CUTTING edges (Enter = all)  [w/c/a/b/l/n  Esc=cancel]",
                );
            }
            Ok(Command::Extend) => {
                self.pre_op_selection = std::mem::take(&mut self.selection);
                self.trim_dbg_session_start("EXTEND");
                self.extend_state = ExtendState::SelectingBoundaries;
                self.begin_selection(SelectMode::ForBoundaryEdges);
                self.set_prompt(
                    "extend: pick BOUNDARY edges (Enter = all)  [w/c/a/b/l/n  Esc=cancel]",
                );
            }
            Ok(Command::Move) => {
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Move;
                    self.set_prompt("move: select dobjects, Enter to continue  [Esc=cancel]");
                } else {
                    self.move_state = MoveState::WaitingForBase;
                    self.set_prompt(format!(
                        "move ({} dobject(s)): click BASE point  [Esc=cancel]",
                        self.selection.len()
                    ));
                }
            }
            Ok(Command::Hatch {
                pattern,
                scale,
                angle_deg,
            }) => {
                // Block re-entry while a preview confirm panel is open
                // — otherwise typing `hatch` again stacks duplicate
                // fills on the same boundary before the user gets to
                // accept/reject the first one (the bug from the user's
                // log dump).
                if self.hatch_confirm_open {
                    self.history.push(
                        "  ! hatch: a hatch preview is awaiting your decision — Confirm, Discard, or pick an action in the panel first".into());
                    return;
                }
                // Auto-open the Hatch Debug window so the user sees the
                // live log without having to hunt for the toolbar
                // toggle. Mirrors the trim/extend pattern. Does NOT
                // clear prior entries — accumulating context is useful
                // when comparing successive `hatch` runs.
                self.hatch_dbg_session_start();
                self.hatch_dbg(format!(
                    "Command::Hatch parsed — pattern={:?}, scale={}, angle={}",
                    pattern, scale, angle_deg
                ));
                // No args → open the attributes dialog so the user
                // picks pattern + scale + angle with a live preview
                // BEFORE picking the boundary. With args, run directly
                // (the scriptable / power-user path).
                if pattern.is_none() && (scale - 1.0).abs() < 1e-9 && angle_deg.abs() < 1e-9 {
                    self.hatch_dialog_open = true;
                    self.hatch_dbg("  no args → opened Choose Hatch Attributes dialog".to_string());
                    self.set_prompt(
                        "hatch: pick pattern + scale + angle in the dialog, then click OK",
                    );
                    return;
                }
                self.pending_hatch_pattern = (pattern.clone(), scale, angle_deg);
                self.hatch_dbg(format!(
                    "  pending_hatch_pattern = ({:?}, {}, {})",
                    pattern, scale, angle_deg
                ));
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Hatch;
                    let style = pattern.as_deref().unwrap_or("SOLID");
                    self.hatch_dbg(format!(
                        "  empty selection → ForSelect + QueuedOp::Hatch ({})",
                        style
                    ));
                    self.set_prompt(format!(
                        "hatch ({}): pick CLOSED boundary dobject(s), Enter to fill  [Esc=cancel]",
                        style
                    ));
                } else {
                    self.hatch_dbg(format!(
                        "  selection has {} dobject(s) → applying immediately",
                        self.selection.len()
                    ));
                    self.apply_hatch();
                }
            }
            Ok(Command::Fillet(r_opt)) => {
                if let Some(r) = r_opt {
                    self.env.FltRad = r;
                    let _ = self.env.save();
                }
                // Drop any active (sticky) draw tool so its click semantics and
                // object-snap markers don't bleed into the fillet pick phase.
                self.tool = Tool::None;
                self.pending.clear();
                let r = self.env.FltRad;
                self.fillet_state = FilletState::WaitingForFirst(r);
                // Continuous by default: keep filleting pair after pair until
                // Esc. `R` changes the radius mid-command (persists as the new
                // default); `M` toggles back to single-shot; `P` = polyline
                // (round every corner of one picked polyline).
                self.fillet_multiple = true;
                self.fillet_poly_all = false;
                self.refresh_fillet_prompt();
            }
            Ok(Command::Chamfer(opt)) => {
                if let Some((d1, d2_opt)) = opt {
                    let d2 = d2_opt.unwrap_or(d1);
                    self.env.ChmDs1 = d1;
                    self.env.ChmDs2 = d2;
                    let _ = self.env.save();
                }
                self.tool = Tool::None;
                self.pending.clear();
                let d1 = self.env.ChmDs1;
                let d2 = self.env.ChmDs2;
                self.chamfer_state = ChamferState::WaitingForFirst(d1, d2);
                // Continuous by default (like fillet): chamfer pair after pair
                // until Esc. `D` changes distances mid-command; `M` toggles
                // back to single-shot; `P` = polyline (bevel every corner).
                self.chamfer_multiple = true;
                self.chamfer_poly_all = false;
                self.refresh_chamfer_prompt();
            }
            Ok(Command::Join) => {
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Join;
                    self.set_prompt("join: select dobjects to merge, Enter to apply  [Esc=cancel]");
                } else {
                    self.apply_join();
                }
            }
            Ok(Command::Dist) => {
                // Pure measurement: prompt for two clicks, log
                // distance + dx + dy + angle. Doesn't touch the doc.
                self.dist_state = DistState::WaitingForP1;
                self.set_prompt("dist: click FIRST point  [Esc=cancel]");
            }
            // WP-SCRIPT: run Python on the worker (never blocks here); replies
            // are drained per-frame in `poll_script_engine`.
            Ok(Command::Python(Some(code))) => {
                self.history.push(format!("  py> {}", code));
                self.script
                    .get_or_insert_with(cad_script::ScriptEngine::new)
                    .submit_text(code);
                self.refocus_cmd = true;
            }
            Ok(Command::Python(None)) => {
                // Bare `py` toggles the docked Python console.
                self.py_console_open = !self.py_console_open;
                if self.py_console_open {
                    self.scan_py_examples();
                    self.history.push(
                        "  python: console opened — `help(rasm)` lists the scripting surface; Esc stops a running script".into());
                }
            }
            Ok(Command::PythonFile(path)) => {
                self.clear_script_preview();
                self.history.push(format!("  pyfile> {}", path));
                self.script
                    .get_or_insert_with(cad_script::ScriptEngine::new)
                    .submit_file(std::path::PathBuf::from(path));
                self.refocus_cmd = true;
            }
            // WP-SCRIPT: `run <name>` executes scripts/<name>.py; bare `run`
            // opens the console and lists what's available.
            // WP-SCRIPT slice 5: `run <name> [args…]` executes scripts/<name>.py
            // with the args passed to the script; bare `run` opens the console
            // and lists what's available.
            Ok(Command::Script(name, args)) => {
                self.run_script_command(name, args);
            }
            // WP-SCRIPT slice 5: `pyhelp` shows the full scripting API
            // reference (the AI-agent document).
            Ok(Command::PyApiDoc) => {
                self.open_scripting_doc();
            }
            Ok(Command::PlotStyle) => {
                // Open the Plot Style Table Editor (CTB color->pen editor). App
                // dialog only — no geometry, no undo. Edits the document's own
                // table; not bound to a CTB file (the CTB menu binds one).
                self.plotstyle_edit_file = None;
                self.plotstyle_open = true;
                if self.plotstyle_sel.is_empty() {
                    self.plotstyle_sel = vec![1];
                    self.plotstyle_anchor = Some(1);
                }
                self.history.push("  plot style table editor".into());
            }
            Ok(Command::Plot) => {
                // Open the Plot dialog. On the MODEL tab it is the model-space
                // dialog (paper/area/scale + table); on a LAYOUT tab the dialog
                // renders the layout's 1:1 paper plot instead (same target).
                // App dialog only — read-only on the doc, no undo.
                // dialog only — read-only on the doc, no undo. Start from the
                // document's saved PAGESETUP.
                let ps = self.doc.page_setup.clone();
                self.plot_paper = ps.paper;
                self.plot_unit_inch = ps.unit_inch;
                self.plot_scale_fit = ps.scale_fit;
                if !ps.scale_fit && ps.scale_n > 1e-9 {
                    self.plot_scale_n = ps.scale_n;
                }
                self.plot_dialog_open = true;
                self.plot_win_pick = 0; // defensive: never reopen mid-pick (parked)
                self.plot_win_p1 = None;
                if self.plot_pdf_path.is_empty() {
                    self.plot_pdf_path = self.default_plot_pdf_path();
                }
                self.set_prompt("plot — set paper / area / scale in the dialog, then Plot");
                self.history.push("  plot".into());
            }
            Ok(Command::PageSetup) => {
                self.tool = Tool::None;
                self.pagesetup_open = true;
            }
            Ok(Command::Purge) => {
                self.tool = Tool::None;
                self.commit_purge();
            }
            Ok(Command::LayerState(args)) => {
                self.tool = Tool::None;
                self.apply_layerstate(&args);
            }
            Ok(Command::Ucs(args)) => {
                self.tool = Tool::None;
                self.apply_ucs(&args);
            }
            Ok(Command::Overkill) => {
                self.tool = Tool::None;
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::Overkill;
                    self.set_prompt("overkill: select dobjects  [Enter = whole drawing]");
                } else {
                    self.commit_overkill(Some(self.selection.clone()));
                }
            }
            Ok(Command::CenterMark(size_override)) => {
                // Click-to-place: a circle/arc click sizes the mark to the
                // entity; empty clicks use the typed override or the default.
                self.tool = Tool::None;
                self.centermark_state = CenterMarkState::WaitingForClick { size_override };
                self.set_prompt(if size_override.is_some() {
                    format!("centermark: click a circle/arc (or any point)  size={:.3}  [Esc cancels]",
                        size_override.unwrap_or(0.0))
                } else {
                    "centermark: click a circle/arc — or any point for a default-size mark  [Esc cancels]"
                        .to_string()
                });
            }
            Ok(Command::WBlock) => {
                self.tool = Tool::None;
                if self.selection.is_empty() {
                    self.begin_selection(SelectMode::ForSelect);
                    self.queued_op = QueuedOp::WBlock;
                    self.set_prompt("wblock: select dobjects  (Enter = whole drawing)".to_string());
                } else {
                    self.apply_wblock();
                }
            }
            Ok(Command::Xref(args)) => {
                self.tool = Tool::None;
                self.apply_xref(&args);
            }
            Ok(Command::Xline) => {
                self.tool = Tool::None;
                self.xline_state = XlineState::WaitingForBase;
                self.set_prompt("xline: pick base point  [H/V/A/Off  Esc exits]".to_string());
            }
            Ok(Command::Ray) => {
                self.tool = Tool::None;
                self.ray_state = RayState::WaitingForBase;
                self.set_prompt("ray: pick base point  [H/V/A  Esc exits]".to_string());
            }
            Ok(Command::Donut) => {
                self.tool = Tool::None;
                self.donut_state = DonutState::WaitingCenter;
                self.set_prompt(
                    "donut: click CENTER, then outer radius, then inner radius  [Esc exits]"
                        .to_string(),
                );
            }
            Ok(Command::Wipeout) => {
                self.tool = Tool::None;
                self.wipeout_state = WipeoutState::WaitingFirstCorner;
                self.set_prompt("wipeout: pick first corner  [Esc exits]".to_string());
            }
            Ok(Command::Area) => {
                // AREA — click a closed object (its area is measured) or
                // accumulate a point polygon. `a`/`s` toggle add/subtract;
                // Enter folds the polygon in; Esc exits. Pure inspection.
                self.tool = Tool::None;
                self.area_state = Some(AreaState {
                    total: 0.0,
                    sign: 1.0,
                    pts: Vec::new(),
                });
                self.set_prompt(
                    "area: pick a closed object or pick points  [A=add  S=subtract  Enter=finish  Esc]"
                        .to_string());
            }
            Ok(Command::LayIso) => {
                self.tool = Tool::None;
                self.layer_pick = LayerPickState::Iso;
                self.set_prompt(
                    "layiso: click a dobject — all OTHER layers freeze  [Esc exits]".to_string(),
                );
            }
            Ok(Command::LayFrz) => {
                self.tool = Tool::None;
                self.layer_pick = LayerPickState::Frz;
                self.set_prompt(
                    "layfrz: click a dobject to freeze its layer  [Esc exits]".to_string(),
                );
            }
            Ok(Command::LayOff) => {
                self.tool = Tool::None;
                self.layer_pick = LayerPickState::OffLayer;
                self.set_prompt(
                    "layoff: click a dobject to turn off its layer  [Esc exits]".to_string(),
                );
            }
            Ok(Command::LayOn) => {
                // Immediate — no pick. Restore every layer to visible +
                // thawed in one undo entry; a no-op reports visibly and
                // leaves no undo entry behind.
                self.tool = Tool::None;
                let changed = self
                    .doc
                    .layers
                    .layers
                    .iter()
                    .filter(|l| !l.visible || l.frozen)
                    .count();
                if changed == 0 {
                    self.history
                        .push("  ! layon: no hidden or frozen layers to restore".into());
                    return;
                }
                self.snapshot_doc();
                for l in &mut self.doc.layers.layers {
                    if !l.visible || l.frozen {
                        l.visible = true;
                        l.frozen = false;
                    }
                }
                self.gpu_dirty = true;
                self.index_dirty = true;
                self.history
                    .push(format!("  layon: {} layer(s) restored", changed));
            }
            Ok(Command::LayWalk) => {
                self.tool = Tool::None;
                self.laywalk_open = true;
                self.laywalk_restore = Some(
                    self.doc
                        .layers
                        .layers
                        .iter()
                        .map(|l| (l.visible, l.frozen))
                        .collect(),
                );
                self.layer_search.clear();
                self.set_prompt("laywalk: pick a layer to preview  [Close restores]".to_string());
            }

            Ok(Command::Divide) => {
                // Pick a curve, then type the segment count. A pre-selected
                // basket skips straight to the value prompt (pickfirst).
                self.tool = Tool::None;
                if let Some(&i) = self.selection.first() {
                    self.ptdist_state = PtDistribState::DivideValue(i);
                    self.set_prompt(
                        "divide: type the number of segments  [Esc cancels]".to_string(),
                    );
                } else {
                    self.ptdist_state = PtDistribState::DivideObject;
                    self.set_prompt("divide: click a curve to divide  [Esc exits]".to_string());
                }
            }
            Ok(Command::Measure) => {
                // Same flow as DIVIDE, but the typed value is a segment
                // LENGTH measured along the curve from its start.
                self.tool = Tool::None;
                if let Some(&i) = self.selection.first() {
                    self.ptdist_state = PtDistribState::MeasureValue(i);
                    self.set_prompt(
                        "measure: type the segment LENGTH (drawing units)  [Esc cancels]"
                            .to_string(),
                    );
                } else {
                    self.ptdist_state = PtDistribState::MeasureObject;
                    self.set_prompt(
                        "measure: click a curve to measure along  [Esc exits]".to_string(),
                    );
                }
            }
            Err(e) => {
                // ---- Command-line calculator & user variables ------------
                // The parser-Err path is the ONLY calc entry point, so
                // commands and SYSVARs always win. Handled here:
                //   calc / calc help      → syntax summary
                //   name=expr             → define a lazy variable
                //   expression / var name → evaluate, echo `= value`, store `ans`
                // Anything else keeps today's "unknown command" error.
                if self.calc_idle_input(trimmed) {
                    return;
                }
                self.history.push(format!("  ! {}", e));
            }
            // ---- Commands the merged RUST-AutoRASM kernel parses but this
            // build does not implement. Reported, not silent. ----------------
            _ => {
                self.history.push(format!(
                    "  ! '{}' is recognised but not available in this build",
                    trimmed
                ));
            }
        }
    }

    /// The calculator fallback at the idle command prompt (parser `Err` path).
    /// Returns true when the input was consumed by the calculator.
    fn calc_idle_input(&mut self, trimmed: &str) -> bool {
        // `calc` / `calc help` — the syntax summary.
        let lc = trimmed.to_ascii_lowercase();
        if lc == "calc" || lc == "calc help" {
            for line in CALC_HELP.lines() {
                self.history.push(format!("  {}", line));
            }
            return true;
        }
        // `name=expr` — define a variable (SYSVAR names rejected).
        match crate::calc::try_assign(&mut self.calc, trimmed) {
            Ok(Some((name, value))) => {
                match value {
                    Some(v) => {
                        self.history
                            .push(format!("  {} = {}", name, crate::calc::fmt_value(v)))
                    }
                    // Lazy store: not evaluable yet (unknown vars) — echo the
                    // stored expression so the definition is visible.
                    None => {
                        let expr = self.calc.expr_of(&name).unwrap_or("");
                        self.history.push(format!("  {} = {}", name, expr));
                    }
                }
                self.save_calc_sidecar();
                return true;
            }
            Ok(None) => {} // not an assignment — fall through
            Err(e) => {
                self.history.push(format!("  ! calc: {}", e));
                return true;
            }
        }
        // Bare expression / defined variable name → evaluate + store `ans`.
        let is_defined_name = self.calc.contains(trimmed) && crate::calc::valid_name(trimmed);
        if crate::calc::looks_like_expr(&self.calc, trimmed) || is_defined_name {
            match crate::calc::eval(&self.calc, trimmed) {
                Ok(v) => {
                    self.history
                        .push(format!("  = {}", crate::calc::fmt_value(v)));
                    self.calc.set_ans(v);
                    return true;
                }
                Err(e) => {
                    self.history.push(format!("  ! calc: {}", e));
                    return true;
                }
            }
        }
        false
    }

    /// Parse a number the way every numeric prompt does: plain `f64::parse`
    /// fast path first (untouched — validation ranges and positive-only
    /// checks keep working), then the calculator. The error carries the
    /// unified `calc: <reason>` prefix.
    fn eval_number(&self, s: &str) -> Result<f64, String> {
        let t = s.trim();
        if let Ok(v) = t.parse::<f64>() {
            return Ok(v);
        }
        crate::calc::eval(&self.calc, t).map_err(|e| format!("calc: {}", e))
    }

    /// Same as [`Self::eval_number`] for dialog text fields.
    fn eval_field(&self, buf: &str) -> Result<f64, String> {
        self.eval_number(buf)
    }

    /// The string handed to `env_set` for a SYSVAR value. Numeric kinds go
    /// through the calculator (so `setvar TxHt` then `0.3*2` works); Text /
    /// Choice / Bool / Color keep the raw text — an expression must never
    /// rewrite a path or an option word.
    fn sysvar_value_string(&self, name: &str, raw: &str) -> String {
        use crate::varreg::Kind;
        let numeric = match crate::varreg::find(name).map(|v| v.kind) {
            Some(Kind::Float { .. } | Kind::Int { .. } | Kind::U8 { .. }) => true,
            _ => false,
        };
        if numeric {
            if let Ok(v) = self.eval_number(raw) {
                return crate::calc::fmt_value(v);
            }
        }
        raw.to_string()
    }

    /// Persist variables to the drawing's sidecar, on a WORKER so a large
    /// sidecar (tens of MB of furniture geometry) never freezes the command
    /// line. A sidecar that already exists gets ONLY its `vars` field patched
    /// (cheap, and never risks the rest of the config — the factory block,
    /// the materials — that a fresh config would drop); a drawing with no
    /// sidecar yet gets a fresh one built from LIVE state on the main thread
    /// (nothing is on disk to lose).
    ///
    /// Skipped — variables ride on the next save/autosave instead, no error —
    /// while a modal save/load, an autosave, or a previous patch is in
    /// flight: writing the sidecar concurrently with `save_file_worker` could
    /// clobber the newer config it just wrote, and a second patch could read
    /// stale state. No drawing open → session-only, no error.
    fn save_calc_sidecar(&mut self) {
        if self.busy.is_some() || self.autosave_rx.is_some() || self.calc_sidecar_rx.is_some() {
            return;
        }
        let Some(path) = self.current_file.clone() else {
            return;
        };
        // EMBEDDED MODE HAS NO SIDECAR TO PATCH. The variables ride on the next
        // save/autosave, which embeds them in the file with everything else — and
        // `unsaved` is marked so the close guard and the autosave timer actually
        // fire one. (Patching a fresh sidecar beside an embedded file would
        // recreate the two-copies situation every open asks about. Rewriting the
        // whole drawing per variable assignment would make every keystroke a
        // multi-hundred-MB save.)
        if self.extra_store == crate::simlux_io::ExtraDataStore::Embedded {
            self.unsaved = true;
            self.history.push(
                "  calc variable set — the drawing stores its data inside the file, so \
                 variables are embedded on the next save"
                    .into(),
            );
            return;
        }
        let vars = self.calc.persist_map();
        // A cheap stat decides the path. The no-sidecar case builds the fresh
        // config HERE (the worker cannot borrow the app), so the new sidecar
        // carries the live layers/materials/settings, not an empty shell.
        let fresh = if crate::simlux_io::sidecar_path(&path).exists() {
            None
        } else {
            Some(self.build_simlux_config_common())
        };
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        self.calc_sidecar_rx = Some(rx);
        std::thread::spawn(move || {
            let cfg = match fresh {
                // Sidecar exists → patch ONLY `vars`; everything else on disk
                // stays exactly as the last save wrote it.
                None => {
                    let cfg = match crate::simlux_io::load(&path) {
                        Ok(Some(mut cfg)) => {
                            cfg.vars = vars;
                            cfg
                        }
                        // Vanished between the stat and here — fall back to a
                        // minimal record (the next real save fills the rest).
                        Ok(None) => {
                            let mut cfg = crate::simlux_io::SimluxConfig::default();
                            cfg.vars = vars;
                            cfg
                        }
                        Err(e) => {
                            let _ = tx.send(Err(format!(
                                "variables stay in this session — sidecar read failed: {}",
                                e
                            )));
                            return;
                        }
                    };
                    cfg
                }
                Some(mut cfg) => {
                    cfg.vars = vars;
                    cfg
                }
            };
            if let Err(e) = crate::simlux_io::save(&path, &cfg) {
                let _ = tx.send(Err(format!(
                    "variables stay in this session — sidecar save failed: {}",
                    e
                )));
                return;
            }
            let _ = tx.send(Ok(()));
        });
    }

    /// Drain the calculator-variables sidecar patch result (once per frame).
    fn tick_calc_sidecar(&mut self) {
        let Some(rx) = &self.calc_sidecar_rx else {
            return;
        };
        use std::sync::mpsc::TryRecvError;
        match rx.try_recv() {
            Ok(Err(e)) => self.history.push(format!("  ! calc: {}", e)),
            Ok(Ok(())) => {}
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {}
        }
        self.calc_sidecar_rx = None;
    }

    /// One-stop "wipe everything geometry-related". Called from the toolbar's
    /// "clear all" button AND the typed `clear` command — both used to diverge.
    fn clear_all(&mut self) {
        self.doc.dobjects.clear();
        self.intersections.clear();
        self.blockdiff_overlay = None;
        self.param_name_dialog = None;
        self.block_task_rec = None;
        self.pending.clear();
        self.selected = None;
        self.snap_override = None;
        self.index = None;
        self.index_dirty = true;
        self.index_label.clear();
        self.last_intersect_label.clear();
        self.touch_view();
    }

    // ---- selection helpers (list / select commands) -------------------

    fn begin_selection(&mut self, mode: SelectMode) {
        // A fresh selection session — abandon any in-progress draw / pick /
        // move. The session starts empty; the user can grow it with clicks,
        // window drags, or the `before` sub-command.
        self.select_mode = mode;
        self.selection.clear();
        self.window_first = None;
        self.select_remove_mode = false;
        self.tool = Tool::None;
        self.pending.clear();
        self.picking_source = false;
        self.move_state = MoveState::Off;
    }

    fn cancel_selection(&mut self) {
        self.select_mode = SelectMode::Off;
        self.selection.clear();
        self.window_first = None;
        self.armed_window_inside = None;
        self.select_remove_mode = false;
        let had_queued = self.queued_op != QueuedOp::None;
        self.queued_op = QueuedOp::None;
        self.history.push(if had_queued {
            "  selection cancelled — pending operation aborted".into()
        } else {
            "  selection cancelled".into()
        });
    }

    fn finalise_selection(&mut self) {
        match self.select_mode {
            SelectMode::Off => return,
            SelectMode::ForList => {
                self.history.push(format!(
                    "  list — {} dobject(s) selected:",
                    self.selection.len()
                ));
                let sel = self.selection.clone();
                for i in &sel {
                    if let Some(d) = self.doc.dobjects.get(*i) {
                        self.history.push(format!("    ─── #{} ───", i));
                        for line in list_full_details(&d.geom) {
                            self.history.push(format!("      {}", line));
                        }
                    }
                }
                // Clear the basket so the dashed pulse goes away —
                // 'list' is an inspection command, not a selection
                // command. (Same rule the user set for the pickfirst
                // branch above.)
                self.selection_prev = self.selection.clone();
                self.selection.clear();
                self.selected = None;
            }
            SelectMode::ForSelect => {
                self.history.push(format!(
                    "  select — {} dobject(s) kept as the active selection",
                    self.selection.len()
                ));
            }
            SelectMode::ForCuttingEdges => {
                let cutters = std::mem::take(&mut self.selection);
                // Restore the user's main selection — trim must not nuke it.
                self.selection = std::mem::take(&mut self.pre_op_selection);
                self.select_mode = SelectMode::Off;
                self.window_first = None;
                self.select_remove_mode = false;
                // Empty cutter basket = "use every current dobject as a
                // cutter, recomputed each click". This is AutoCAD's default
                // ("press Enter to select all") AND it's the only way pieces
                // created by THIS session's trims keep acting as cutters.
                // See memos `feedback_rust_cad_trim_default_all_cutters` +
                // `feedback_rust_cad_trim_breaks_into_all_segments`.
                if cutters.is_empty() {
                    if self.doc.dobjects.is_empty() {
                        self.history
                            .push("  ! trim: document is empty — cancelled".into());
                        self.trim_dbg("CUTTERS = []  (empty doc → session cancelled)");
                        self.trim_state = TrimState::Off;
                        return;
                    }
                    self.history.push(format!(
                        "  trim — no cutters picked; using ALL dobjects (dynamic) as cutters"
                    ));
                    self.trim_dbg(format!(
                        "CUTTERS = ALL (dynamic; doc currently has {} dobjects, recomputed each click)",
                        self.doc.dobjects.len()));
                    self.history.push(
                        "  trim — every dobject is a cutter (warm orange). Click each TARGET to cut. Enter / Esc to finish.".into());
                    self.trim_state = TrimState::PickingTargetsAll;
                    return;
                }
                self.trim_dbg(format!(
                    "CUTTERS captured = {} indices: {:?}",
                    cutters.len(),
                    cutters
                ));
                // Dump every cutter's full geometry — bug-reportable input.
                let cutters_for_log = cutters.clone();
                for idx in &cutters_for_log {
                    self.trim_dbg_dobject(*idx, "CUTTER");
                }
                self.history.push(format!(
                    "  trim — {} cutter(s) ready (warm orange). Click each TARGET to cut. Enter / Esc to finish.",
                    cutters.len()));
                self.trim_state = TrimState::PickingTargets(cutters);
                return;
            }
            SelectMode::ForBoundaryEdges => {
                let bounds = std::mem::take(&mut self.selection);
                self.selection = std::mem::take(&mut self.pre_op_selection);
                self.select_mode = SelectMode::Off;
                self.window_first = None;
                self.select_remove_mode = false;
                if bounds.is_empty() {
                    if self.doc.dobjects.is_empty() {
                        self.history
                            .push("  ! extend: document is empty — cancelled".into());
                        self.trim_dbg("BOUNDARIES = []  (empty doc → session cancelled)");
                        self.extend_state = ExtendState::Off;
                        return;
                    }
                    self.history.push(format!(
                        "  extend — no boundaries picked; using ALL dobjects (dynamic) as boundaries"));
                    self.trim_dbg(format!(
                        "BOUNDARIES = ALL (dynamic; doc currently has {} dobjects, recomputed each click)",
                        self.doc.dobjects.len()));
                    self.history.push(
                        "  extend — every dobject is a boundary (warm amber). Click each TARGET END. Enter / Esc to finish.".into());
                    self.extend_state = ExtendState::PickingTargetsAll;
                    return;
                }
                self.trim_dbg(format!(
                    "BOUNDARIES captured = {} indices: {:?}",
                    bounds.len(),
                    bounds
                ));
                self.history.push(format!(
                    "  extend — {} boundary edge(s) ready (warm amber). Click each TARGET END to extend. Enter / Esc to finish.",
                    bounds.len()));
                self.extend_state = ExtendState::PickingTargets(bounds);
                return;
            }
        }
        self.select_mode = SelectMode::Off;
        self.window_first = None;
        self.select_remove_mode = false;
        // Snapshot for `before`. Only update when the finalised set is
        // non-empty — pressing Enter on an empty set shouldn't wipe the
        // previous memory.
        if !self.selection.is_empty() {
            self.selection_prev = self.selection.clone();
        }

        // Dispatch any operation that was queued behind this selection
        // (e.g. `move` opened the session; finalising it transitions
        // straight to base-point capture).
        let queued = std::mem::replace(&mut self.queued_op, QueuedOp::None);
        // BlockReopen always restores its dialog (even with an empty
        // basket — the user can just select again from there). Overkill
        // with an empty basket means the WHOLE drawing (AutoCAD).
        if queued != QueuedOp::None
            && queued != QueuedOp::BlockReopen
            && queued != QueuedOp::Overkill
            && self.selection.is_empty()
        {
            self.history.push(format!(
                "  ! {:?}: nothing selected — operation cancelled",
                queued
            ));
            // Drop any parked dialog so we don't leak it.
            self.block_dialog_stash = None;
            return;
        }
        match queued {
            QueuedOp::None => {}
            QueuedOp::Move => {
                self.move_state = MoveState::WaitingForBase;
                self.set_prompt(format!(
                    "move ({} dobject(s)): click BASE point  [Esc=cancel]",
                    self.selection.len()
                ));
            }
            QueuedOp::Copy => {
                self.copy_state = CopyState::WaitingForBase;
                self.set_prompt(format!(
                    "copy ({} dobject(s)): click BASE point  [Esc=cancel]",
                    self.selection.len()
                ));
            }
            QueuedOp::Rotate => {
                self.rotate_state = RotateState::WaitingForPivot;
                self.set_prompt(format!(
                    "rotate ({} dobject(s)): click PIVOT  [Esc=cancel]",
                    self.selection.len()
                ));
            }
            QueuedOp::Scale => {
                self.scale_state = ScaleState::WaitingForPivot;
                self.set_prompt(format!(
                    "scale ({} dobject(s)): click PIVOT  [Esc=cancel]",
                    self.selection.len()
                ));
            }
            QueuedOp::Mirror => {
                self.mirror_state = MirrorState::WaitingForA;
                self.set_prompt(format!(
                    "mirror ({} dobject(s)): click FIRST axis point  [Esc=cancel]",
                    self.selection.len()
                ));
            }
            QueuedOp::Join => {
                self.apply_join();
            }
            QueuedOp::PeditJoin(h) => {
                self.pedit_join_selected(h);
            }
            QueuedOp::PeditStart => {
                // The user picked the object to edit — run pedit on it.
                self.pedit_start();
            }
            QueuedOp::Hatch => {
                self.apply_hatch();
            }
            QueuedOp::Array => {
                // Selection basket now holds the array source(s). Re-show
                // the array dialog so the user can set rows/cols/dx/dy
                // and click Generate. We don't drain the basket — the
                // dialog uses `self.selection` directly.
                self.array_open = true;
                self.clear_prompt();
                self.history.push(format!(
                    "  array: {} source dobject(s) picked — set rows/cols and Generate",
                    self.selection.len()
                ));
            }
            QueuedOp::Align => {
                self.align_state = AlignState::WaitingForSrc1;
                self.set_prompt(format!(
                    "align ({} dobject(s)): click SOURCE point 1  [Esc=cancel]",
                    self.selection.len()
                ));
            }
            QueuedOp::BlockDef(name) => {
                self.set_prompt(format!(
                    "block '{}' ({} dobject(s)): click BASE point  [Esc=cancel]",
                    name,
                    self.selection.len()
                ));
                self.block_def_state = BlockDefState::WaitingForBase { name };
            }
            QueuedOp::BlockReopen => {
                // Selection done — restore the parked Block dialog so the
                // user can finish (set base / color / smart / OK).
                if let Some(dlg) = self.block_dialog_stash.take() {
                    self.block_dialog = Some(dlg);
                }
                self.clear_prompt();
            }
            QueuedOp::Stretch => {
                // Selection done. Test region = the captured crossing box,
                // or (no crossing window) the selection's bbox → whole move.
                let (mn, mx) = self
                    .stretch_window_box
                    .take()
                    .unwrap_or_else(|| self.selection_bbox());
                self.stretch_state = StretchState::WaitingForBase(mn, mx);
                self.set_prompt(format!(
                    "stretch ({} dobject(s)): click BASE point  [Esc=cancel]",
                    self.selection.len()
                ));
            }
            QueuedOp::Explode => {
                self.apply_explode();
            }
            QueuedOp::Erase => {
                // Replay the same path Command::DeleteSelected uses.
                self.run_command("erase");
            }
            QueuedOp::Region => {
                self.apply_region();
                self.clear_prompt();
            }
            QueuedOp::WBlock => {
                // Empty selection + Enter → whole drawing (AutoCAD WBLOCK).
                self.apply_wblock();
            }
            QueuedOp::Overkill => {
                // Empty selection (Enter with nothing picked) → whole drawing.
                let subset = if self.selection.is_empty() {
                    None
                } else {
                    Some(self.selection.clone())
                };
                self.commit_overkill(subset);
                self.clear_prompt();
            }
            QueuedOp::ChProp(prop, val) => {
                self.apply_chprop(&prop, &val);
            }
            QueuedOp::MatchPropPaint(src) => {
                let targets: Vec<usize> = self
                    .selection
                    .iter()
                    .copied()
                    .filter(|&t| t != src)
                    .collect();
                if targets.is_empty() {
                    self.history.push("  matchprop: no targets selected".into());
                } else {
                    self.snapshot_doc();
                    let mut n = 0;
                    for t in &targets {
                        if self.apply_matchprop_one(src, *t) {
                            n += 1;
                        }
                    }
                    self.history
                        .push(format!("  ✓ matchprop: source #{} → {} dobject(s)", src, n));
                }
                self.clear_prompt();
            }
        }
        // self.selection persists so follow-up commands (move, list, …) can use it.
    }

    /// Resolve a hatch's boundary handles into a list of vertex loops
    /// in world coords. Each closed polyline boundary contributes one
    /// loop; bulges are tessellated to short chord segments so curved
    /// boundaries fill smoothly. Handles that no longer resolve (the
    /// user deleted a boundary) are silently skipped, so the hatch
    /// just shrinks rather than crashing.
    fn resolve_hatch_loops(&self, h: &cad_kernel::Hatch) -> Vec<Vec<Vec2>> {
        // Delegates to the SHARED kernel resolver (cad_kernel::resolve_hatch_loops)
        // so on-screen RENDER and DXF EXPORT compute hatch fill loops from the
        // exact same code — the whole point of promoting it to the kernel.
        //
        // The kernel resolver adds what the inline copy below didn't have:
        // spline boundaries, invisible/frozen-layer gating (issue #17, with the
        // `hatch_aux` synthetic-boundary exception) and the shared tessellation
        // helpers.
        cad_kernel::resolve_hatch_loops(h, &self.doc)
    }

    /// Paint a hatch — solid fill of its resolved boundary loops with
    /// the AutoCAD-style even-odd rule (outer loop fills, next is a
    /// hole, then a hole-in-hole, etc.). For multi-loop fills we
    /// triangulate ourselves via ear-clipping with the loops cut into
    /// a single ring; for one loop we just hand to egui's path
    /// tessellator. Pattern variants (parallel lines, ANSI / ISO) will
    /// dispatch on `h.pattern` here later.
    fn render_hatch_fill(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        h: &cad_kernel::Hatch,
        color: egui::Color32,
    ) {
        let loops = self.resolve_hatch_loops(h);
        if loops.is_empty() {
            return;
        }
        match &h.pattern {
            cad_kernel::HatchPattern::Solid => {
                self.render_hatch_solid(painter, rect, &loops, color)
            }
            cad_kernel::HatchPattern::Pattern {
                name,
                scale,
                angle_deg,
            } => self.render_hatch_pattern(painter, rect, &loops, color, name, *scale, *angle_deg),
        }
    }

    /// Solid fill path — geometric even-odd by containment DEPTH: a loop at
    /// even depth (outer, hole-in-hole, …) fills in the hatch colour, odd
    /// depth subtracts via canvas-background overdraw. Loops are painted in
    /// depth order so a nested island re-fills its parent hole's interior
    /// (letter counters inside punched-out letters hatch again — AutoCAD
    /// even-odd semantics). The old index-based alternation broke as soon
    /// as two sibling islands appeared (the second got re-filled).
    fn render_hatch_solid(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        loops: &[Vec<Vec2>],
        color: egui::Color32,
    ) {
        if loops.is_empty() {
            return;
        }
        if loops.len() == 1 {
            let pts: Vec<egui::Pos2> = loops[0].iter().map(|w| self.w2s(*w, rect)).collect();
            if pts.len() < 3 {
                return;
            }
            painter.add(egui::Shape::Path(egui::epaint::PathShape {
                points: pts,
                closed: true,
                fill: color,
                stroke: egui::epaint::PathStroke::NONE,
            }));
            return;
        }
        let bg = egui::Color32::from_rgb(18, 22, 28);
        // Containment depth per loop, computed ONCE (see `loop_depths`) —
        // recomputing inside a sort key or per paint is O(n²·verts) per
        // frame for island-heavy fills. Loops paint in depth order so a
        // nested island re-fills its parent hole's interior.
        let depths = loop_depths(loops);
        let mut order: Vec<usize> = (0..loops.len()).collect();
        order.sort_by_cached_key(|&i| depths[i]);
        for i in order {
            let pts: Vec<egui::Pos2> = loops[i].iter().map(|w| self.w2s(*w, rect)).collect();
            if pts.len() < 3 {
                continue;
            }
            let fill = if depths[i] % 2 == 0 { color } else { bg };
            painter.add(egui::Shape::Path(egui::epaint::PathShape {
                points: pts,
                closed: true,
                fill,
                stroke: egui::epaint::PathStroke::NONE,
            }));
        }
    }

    /// Named-pattern fill — for each line family in the catalog entry,
    /// generate parallel lines covering the boundary bbox, clip each
    /// against ALL loops via even-odd along the line, draw the
    /// surviving segments. Unknown pattern name → no lines drawn.
    fn render_hatch_pattern(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        loops: &[Vec<Vec2>],
        color: egui::Color32,
        name: &str,
        user_scale: f64,
        user_angle_deg: f64,
    ) {
        let pat = cad_kernel::patterns::lookup(name);
        if pat.is_empty() {
            return;
        }
        // Union bbox of all loops in world coords.
        let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for l in loops {
            for v in l {
                if v.x < min.x {
                    min.x = v.x;
                }
                if v.y < min.y {
                    min.y = v.y;
                }
                if v.x > max.x {
                    max.x = v.x;
                }
                if v.y > max.y {
                    max.y = v.y;
                }
            }
        }
        if !min.x.is_finite() || !max.x.is_finite() {
            return;
        }
        let user_angle = user_angle_deg.to_radians();
        let stroke = egui::Stroke::new(0.9, color);
        let families = match &pat {
            cad_kernel::patterns::Pattern::Families(fs) => fs.as_slice(),
            cad_kernel::patterns::Pattern::Tile {
                period_x,
                period_y,
                segments,
                circles,
            } => {
                self.render_hatch_tile(
                    painter, rect, loops, stroke, *period_x, *period_y, segments, circles,
                    user_scale, user_angle, min, max,
                );
                return;
            }
        };
        for fam in families {
            // Effective angle + spacing after user transform.
            let theta = fam.angle + user_angle;
            let spacing = fam.spacing * user_scale.abs().max(1e-9);
            // Line direction u, normal n (CCW perp of u).
            let cos = theta.cos();
            let sin = theta.sin();
            let u = Vec2::new(cos, sin);
            let n = Vec2::new(-sin, cos);
            // Project bbox corners onto n to find the range of
            // s-values (offset along n) that the pattern needs to
            // cover. The bbox of a rotated axis-aligned rect is
            // bounded by the projection of its 4 corners.
            let corners = [
                Vec2::new(min.x, min.y),
                Vec2::new(max.x, min.y),
                Vec2::new(min.x, max.y),
                Vec2::new(max.x, max.y),
            ];
            let base = Vec2::new(fam.base_x, fam.base_y);
            let mut s_min = f64::INFINITY;
            let mut s_max = f64::NEG_INFINITY;
            for c in &corners {
                let s = (*c - base).dot(n);
                if s < s_min {
                    s_min = s;
                }
                if s > s_max {
                    s_max = s;
                }
            }
            // First line at s = ceil(s_min / spacing) * spacing.
            let mut s = (s_min / spacing).ceil() * spacing;
            // Safety cap: spacing too small for the world bbox would
            // generate millions of lines and freeze. Bail out if the
            // family would produce > 10 000 lines for this hatch.
            let line_count_estimate = ((s_max - s_min) / spacing).ceil();
            if line_count_estimate > 10_000.0 {
                continue;
            }
            while s <= s_max + 1e-9 {
                let line_origin = base + n * s;
                // Clip this infinite line against the loops via the shared
                // kernel even-odd clipper — per-loop pairing + XOR across
                // loops handles shared edges (a chord hatched as two
                // half-discs) and nested islands correctly.
                let intervals = cad_kernel::patterns::hatch_line_intervals(loops, line_origin, u);
                for (t0, t1) in intervals {
                    let p0 = line_origin + u * t0;
                    let p1 = line_origin + u * t1;
                    painter.line_segment([self.w2s(p0, rect), self.w2s(p1, rect)], stroke);
                }
                s += spacing;
            }
        }
    }

    /// Tile-pattern renderer — tiles a finite-segment cell across the
    /// boundary bbox and clips each segment against the loops using the
    /// same infinite-line + even-odd machinery as families, then clamps
    /// the resulting visible intervals to the segment's own length so
    /// only its in-cell portion is drawn.
    ///
    /// `user_scale` multiplies the period AND the segment coords; the
    /// user_angle rotates the whole pattern about the origin.
    #[allow(clippy::too_many_arguments)]
    fn render_hatch_tile(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        loops: &[Vec<Vec2>],
        stroke: egui::Stroke,
        period_x: f64,
        period_y: f64,
        segments: &[cad_kernel::patterns::PatternSegment],
        circles: &[cad_kernel::patterns::PatternCircle],
        user_scale: f64,
        user_angle: f64,
        min: Vec2,
        max: Vec2,
    ) {
        let s = user_scale.abs().max(1e-9);
        let px = period_x * s;
        let py = period_y * s;
        if px < 1e-9 || py < 1e-9 {
            return;
        }
        let cos = user_angle.cos();
        let sin = user_angle.sin();
        // Cells in the AXIS-ALIGNED pattern frame that need rendering.
        // After user_angle rotation the cell axes no longer align with
        // the world bbox — invert the rotation on each world-bbox corner
        // to find the bbox in pattern frame, then iterate cells.
        let corners_world = [
            Vec2::new(min.x, min.y),
            Vec2::new(max.x, min.y),
            Vec2::new(max.x, max.y),
            Vec2::new(min.x, max.y),
        ];
        let mut pmin = Vec2::new(f64::INFINITY, f64::INFINITY);
        let mut pmax = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        for c in &corners_world {
            // inverse rotation: (cos, sin) → (cos, -sin)
            let px_w = c.x * cos + c.y * sin;
            let py_w = -c.x * sin + c.y * cos;
            if px_w < pmin.x {
                pmin.x = px_w;
            }
            if py_w < pmin.y {
                pmin.y = py_w;
            }
            if px_w > pmax.x {
                pmax.x = px_w;
            }
            if py_w > pmax.y {
                pmax.y = py_w;
            }
        }
        let i0 = (pmin.x / px).floor() as i64 - 1;
        let i1 = (pmax.x / px).ceil() as i64 + 1;
        let j0 = (pmin.y / py).floor() as i64 - 1;
        let j1 = (pmax.y / py).ceil() as i64 + 1;
        // Safety cap — millions of cells would freeze the UI.
        let tile_count = (i1 - i0).max(0) * (j1 - j0).max(0);
        if tile_count > 200_000 {
            return;
        }
        for j in j0..=j1 {
            for i in i0..=i1 {
                let ox = (i as f64) * px;
                let oy = (j as f64) * py;
                for seg in segments {
                    // Segment endpoints in PATTERN frame (with user scale).
                    let ax_p = ox + seg.x1 * s;
                    let ay_p = oy + seg.y1 * s;
                    let bx_p = ox + seg.x2 * s;
                    let by_p = oy + seg.y2 * s;
                    // Rotate to WORLD frame by user_angle.
                    let ax = ax_p * cos - ay_p * sin;
                    let ay = ax_p * sin + ay_p * cos;
                    let bx = bx_p * cos - by_p * sin;
                    let by = bx_p * sin + by_p * cos;
                    let a = Vec2::new(ax, ay);
                    let b = Vec2::new(bx, by);
                    let dvec = b - a;
                    let seg_len2 = dvec.x * dvec.x + dvec.y * dvec.y;
                    if seg_len2 < 1e-18 {
                        continue;
                    }
                    // Clip against the loops with the same per-loop + XOR
                    // machinery as the family lines (kernel
                    // hatch_line_intervals), then clamp the resulting
                    // intervals to the segment's own t-range [0, 1]. The
                    // XOR runs over the infinite line, so a segment
                    // starting/ending INSIDE a loop is handled by intervals
                    // that span past the clamp window.
                    let intervals = cad_kernel::patterns::hatch_line_intervals(&loops, a, dvec);
                    for (t0, t1) in intervals {
                        let (t0, t1) = (t0.clamp(0.0, 1.0), t1.clamp(0.0, 1.0));
                        if t1 - t0 > 1e-6 {
                            let p0 = a + dvec * t0;
                            let p1 = a + dvec * t1;
                            painter.line_segment([self.w2s(p0, rect), self.w2s(p1, rect)], stroke);
                        }
                    }
                }
                // Circles in the cell — paint each, but only when its
                // CENTRE lies inside the resolved hatch boundary loops
                // (even-odd). v1 simplification: a circle is "in or out"
                // as a whole. Boundary-intersecting circles render with
                // their full ring beyond the boundary; refine later by
                // arc-clipping when a circle straddles the boundary.
                for c in circles {
                    let cx_p = ox + c.cx * s;
                    let cy_p = oy + c.cy * s;
                    let cx = cx_p * cos - cy_p * sin;
                    let cy = cx_p * sin + cy_p * cos;
                    let centre = Vec2::new(cx, cy);
                    let r_world = c.radius * s;
                    if r_world < 1e-9 {
                        continue;
                    }
                    let inside = loops.iter().fold(false, |acc, l| {
                        acc ^ point_in_polygon(centre, l.iter().copied())
                    });
                    if !inside {
                        continue;
                    }
                    let r_px = (r_world as f32) * self.scale;
                    if r_px < 0.5 {
                        continue;
                    }
                    painter.circle_stroke(self.w2s(centre, rect), r_px, stroke);
                }
            }
        }
    }

    /// Pick-point boundary finder — returns the INDEX in
    /// `self.doc.dobjects` of the smallest closed dobject whose
    /// interior contains `world`. Used by the hatch dialog's "Pick
    /// Point" button. "Smallest" is measured by bbox area (a cheap
    /// proxy for actual enclosed area — fine for nested loops; falls
    /// down when two disjoint loops have similar bboxes but the
    /// click is in only one of them, which the point-in test filters
    /// out anyway).
    ///
    /// Closed dobject types considered today:
    ///   * Closed Polyline — even-odd ray cast on the vertex polygon
    ///   * Circle          — distance from centre < radius
    ///   * Ellipse         — local coords + (x/a)² + (y/b)² < 1
    ///
    /// Open chains (line + arc loops) and Splines are skipped — they
    /// need boundary-traversal to chain into a loop, which is the
    /// pick-point v2b slice's job.
    /// Same containment test as `find_smallest_containing_closed`, but
    /// returns ALL containing closed dobjects with their bbox area + kind
    /// label — useful for the hatch-debug log so the user can see every
    /// candidate the cheap path considered, not just the winner.
    fn collect_closed_containing(&self, world: Vec2) -> Vec<(usize, f64, &'static str)> {
        self.collect_closed_containing_scoped(world, None)
    }

    /// Scoped variant — when `scope` is `Some`, only iterates those
    /// indices (typically the viewport_scope set). Lets hatch avoid
    /// scanning all 400k+ dobjects when only ~50 are near the click.
    fn collect_closed_containing_scoped(
        &self,
        world: Vec2,
        scope: Option<&[usize]>,
    ) -> Vec<(usize, f64, &'static str)> {
        let mut out = Vec::new();
        let iter: Box<dyn Iterator<Item = (usize, &DObject)>> = match scope {
            Some(s) => Box::new(
                s.iter()
                    .filter_map(|&i| self.doc.dobjects.get(i).map(|d| (i, d))),
            ),
            None => Box::new(self.doc.dobjects.iter().enumerate()),
        };
        for (i, d) in iter {
            // A boundary the user cannot SEE must not define a clickable
            // region. This skips the invisible synthetic polylines the trace
            // path materialises for its own hatches (`hatch_aux`), plus
            // anything hidden or on an off/frozen layer.
            //
            // Without this, every traced hatch left boundaries behind that
            // became candidates for the NEXT pick-point: a second click in the
            // same area saw "2+ candidates contain the seed", declared a
            // partial overlap, and deferred to the slow trace path — which
            // materialised yet more hidden boundaries. The pollution compounded
            // across a session. The island scan already filtered hidden
            // dobjects (`skip (hidden)`); this scan did not, and the two
            // disagreeing is what produced the asymmetry.
            if !self.doc.is_visible(i) {
                continue;
            }
            let contains = match &d.geom {
                // Treat polylines with coincident endpoints as closed too
                // (the "drew it as a loop but forgot to type c Enter" case).
                Geom::Polyline(p) if polyline_is_effectively_closed(p) => {
                    let verts = closed_dobject_polygon(&d.geom);
                    point_in_polygon(world, verts)
                }
                Geom::Circle(c) => (world - c.center).len() < c.radius,
                Geom::Ellipse(e) => {
                    let dvec = world - e.center;
                    let a = e.semi_major().max(1e-12);
                    let b = e.semi_minor().max(1e-12);
                    let u = dvec.dot(e.u_hat()) / a;
                    let v = dvec.dot(e.v_hat()) / b;
                    u * u + v * v < 1.0
                }
                _ => false,
            };
            if !contains {
                continue;
            }
            let (bmin, bmax) = d.geom.bbox();
            let area = (bmax.x - bmin.x).abs() * (bmax.y - bmin.y).abs();
            out.push((i, area, dobject_kind_name(&d.geom)));
        }
        out
    }

    /// Indices of dobjects whose bbox overlaps the current viewport
    /// world-bbox (`last_visible`). Uses the spatial index when it's
    /// fresh — O(visible cells), typically a few dozen entries — and
    /// falls back to the full N range when the index is stale or
    /// missing. Returns `None` only when no viewport has ever been
    /// rendered (first frame).
    ///
    /// THIS IS THE FIX for the 400k-dobjects perf glitch: every hatch
    /// helper (cheap-path candidate scan, island scan, verbose dump,
    /// trace tessellation) now restricts its iteration to this set
    /// instead of the full document. A click in a 100-dobject
    /// neighbourhood of a 400k-dobject drawing iterates ~100, not 400k.
    fn viewport_scope(&self) -> Option<Vec<usize>> {
        let (vmin, vmax) = self.last_visible?;
        let scope: Vec<usize> = if let (Some(g), false) = (self.index.as_ref(), self.index_dirty) {
            g.query_bbox(vmin, vmax)
                .into_iter()
                .map(|u| u as usize)
                .collect()
        } else {
            (0..self.doc.dobjects.len()).collect()
        };
        Some(scope)
    }

    /// True if the dobject at `outer_idx` has its boundary crossed by
    /// any other visible dobject — which means the cheap path's "hatch
    /// the whole outer + auto islands" answer is WRONG for partial
    /// overlaps. In that case we route to the trace path so the
    /// planar-subdivision face containing the seed gets hatched
    /// instead of the whole outer.
    ///
    /// Detection reuses the trace path's tessellator + seg-seg
    /// intersection helper — same primitive that v2c splits on, just
    /// asked as a yes/no question without actually splitting.
    fn outer_has_crossings_with_others(&self, outer_idx: usize) -> bool {
        self.outer_has_crossings_with_others_scoped(outer_idx, None)
    }

    /// Scoped variant — `scope` limits which dobjects are tessellated
    /// (typically `viewport_scope()`). Used by the hover preview so a
    /// per-frame crossing check stays bounded by the visible set, not
    /// the whole document.
    fn outer_has_crossings_with_others_scoped(
        &self,
        outer_idx: usize,
        scope: Option<&[usize]>,
    ) -> bool {
        let segs = match scope {
            Some(s) => crate::hatch_trace::tessellate_doc_in_view_cancellable(
                &self.doc,
                s,
                &crate::hatch_trace::never_cancelled(),
            ),
            None => crate::hatch_trace::tessellate_doc(&self.doc),
        };
        let outer_segs: Vec<usize> = segs
            .iter()
            .enumerate()
            .filter(|(_, s)| s.src == outer_idx)
            .map(|(i, _)| i)
            .collect();
        if outer_segs.is_empty() {
            return false;
        }
        // The outer's closed polygon for point-in tests. `None` for
        // dobjects that aren't a single closed loop (the cheap path
        // doesn't use them anyway).
        let outer_poly = self
            .doc
            .dobjects
            .get(outer_idx)
            .map(|d| closed_dobject_polygon(&d.geom))
            .filter(|v| v.len() >= 3);
        // Bbox of the outer polygon — a segment can only dip INTO the
        // outer if it overlaps this box, so distant dobjects skip the
        // point-in-polygon cost entirely (the pre-existing interior-
        // crossing test above still runs for every segment).
        let outer_bbox = outer_poly.as_ref().map(|poly| {
            let mut min = Vec2::new(f64::INFINITY, f64::INFINITY);
            let mut max = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
            for v in poly {
                if v.x < min.x {
                    min.x = v.x;
                }
                if v.y < min.y {
                    min.y = v.y;
                }
                if v.x > max.x {
                    max.x = v.x;
                }
                if v.y > max.y {
                    max.y = v.y;
                }
            }
            (min, max)
        });
        for &i in &outer_segs {
            for other in segs.iter() {
                if other.src == outer_idx {
                    continue;
                }
                // Match split_at_intersections' tolerance — endpoint
                // touches don't count, only interior crossings do.
                if let Some((ti, tj, _)) =
                    crate::hatch_trace::seg_seg_intersect_params(&segs[i], other)
                {
                    if ti > 1e-6 && ti < 1.0 - 1e-6 && tj > 1e-6 && tj < 1.0 - 1e-6 {
                        return true;
                    }
                }
                // A crossing at an ENDPOINT (a divider drawn snapped to
                // the outer's boundary) is invisible to the
                // interior-interior test above, yet it still subdivides
                // the region — the cheap path would hatch the whole
                // outer. Detect any other segment that DIPS INTO the
                // outer: an endpoint or the midpoint strictly inside
                // means it subdivides.
                if let (Some(poly), Some((bmin, bmax))) = (&outer_poly, &outer_bbox) {
                    if other.a.x < bmin.x
                        || other.a.y < bmin.y
                        || other.a.x > bmax.x
                        || other.a.y > bmax.y
                    {
                        continue;
                    }
                    if point_in_polygon(other.a, poly.iter().copied())
                        || point_in_polygon(other.b, poly.iter().copied())
                    {
                        return true;
                    }
                    let mid = (other.a + other.b) * 0.5;
                    if point_in_polygon(mid, poly.iter().copied()) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// True when any visible Text dobject's bbox overlaps the candidate
    /// outer's bbox. When text touches the region, the cheap path must
    /// defer to the trace — only the trace carries the glyph contours, so
    /// letters get punched out as islands instead of being filled over.
    /// (Bbox-only check: cheap enough for per-hover routing.)
    /// Scoped variant — `scope` limits which dobjects are checked for text
    /// overlap (typically `viewport_scope()`), so the per-click pick-path
    /// test stays bounded by the visible set, not the whole document.
    fn outer_overlaps_text_scoped(&self, outer_idx: usize, scope: Option<&[usize]>) -> bool {
        let Some(d) = self.doc.dobjects.get(outer_idx) else {
            return false;
        };
        let (omin, omax) = d.geom.bbox();
        let ids: Box<dyn Iterator<Item = usize>> = match scope {
            Some(s) => Box::new(s.iter().copied()),
            None => Box::new(0..self.doc.dobjects.len()),
        };
        for i in ids {
            if i == outer_idx {
                continue;
            }
            let Some(o) = self.doc.dobjects.get(i) else {
                continue;
            };
            if !o.style.visible {
                continue;
            }
            let Geom::Text(t) = &o.geom else { continue };
            if t.text.trim().is_empty() {
                continue;
            }
            let (tmin, tmax) = o.geom.bbox();
            if tmin.x > omax.x || tmax.x < omin.x || tmin.y > omax.y || tmax.y < omin.y {
                continue;
            }
            return true;
        }
        false
    }

    /// Verdict-per-dobject log for the island scan: shows, for each
    /// other dobject in the doc, why it was or wasn't accepted as an
    /// island inside `outer_idx`. Returns lines for the caller to push
    /// into `hatch_dbg`. The actual island list is built by
    /// `collect_islands_inside` — this is pure instrumentation.
    fn dbg_island_scan_verdicts(&self, outer_idx: usize, seed: Vec2) -> Vec<String> {
        let mut out = Vec::new();
        let Some(outer_d) = self.doc.dobjects.get(outer_idx) else {
            return out;
        };
        let outer_polygon = closed_dobject_polygon(&outer_d.geom);
        if outer_polygon.len() < 3 {
            out.push(format!(
                "    outer #{} has no polygon — cannot scan",
                outer_idx
            ));
            return out;
        }
        let (omin, omax) = outer_d.geom.bbox();
        for (i, d) in self.doc.dobjects.iter().enumerate() {
            if i == outer_idx {
                continue;
            }
            let kind = dobject_kind_name(&d.geom);
            if !d.style.visible {
                out.push(format!("    #{:02} {} — skip (hidden)", i, kind));
                continue;
            }
            let is_closed_type = match &d.geom {
                Geom::Polyline(p) => polyline_is_effectively_closed(p),
                Geom::Circle(_) | Geom::Ellipse(_) => true,
                _ => false,
            };
            if !is_closed_type {
                out.push(format!(
                    "    #{:02} {} — skip (not a closed boundary type)",
                    i, kind
                ));
                continue;
            }
            let (bmin, bmax) = d.geom.bbox();
            let bbox_inside = bmin.x >= omin.x - 1e-9
                && bmin.y >= omin.y - 1e-9
                && bmax.x <= omax.x + 1e-9
                && bmax.y <= omax.y + 1e-9;
            if !bbox_inside {
                out.push(format!(
                    "    #{:02} {} — skip (bbox ({:.2},{:.2})→({:.2},{:.2}) NOT fully inside outer bbox)",
                    i, kind, bmin.x, bmin.y, bmax.x, bmax.y));
                continue;
            }
            let cand_poly = closed_dobject_polygon(&d.geom);
            if cand_poly.len() < 3 {
                out.push(format!(
                    "    #{:02} {} — skip (degenerate polygon)",
                    i, kind
                ));
                continue;
            }
            if point_in_polygon(seed, cand_poly.clone()) {
                out.push(format!(
                    "    #{:02} {} — skip (CONTAINS SEED — would be outer, not island)",
                    i, kind
                ));
                continue;
            }
            let n = cand_poly.len();
            let samples = [
                cand_poly[0],
                cand_poly[n / 5],
                cand_poly[(2 * n) / 5],
                cand_poly[(3 * n) / 5],
                cand_poly[(4 * n) / 5],
            ];
            let inside_count = samples
                .iter()
                .filter(|p| point_in_polygon(**p, outer_polygon.clone()))
                .count();
            if inside_count == samples.len() {
                out.push(format!(
                    "    #{:02} {} — ISLAND ACCEPTED (bbox inside, {}/{} boundary samples inside outer, seed-not-inside)",
                    i, kind, inside_count, samples.len()));
            } else {
                out.push(format!(
                    "    #{:02} {} — skip (only {}/{} boundary samples inside outer — partial overlap, not nested)",
                    i, kind, inside_count, samples.len()));
            }
        }
        out
    }

    /// After cheap path picks an OUTER, auto-detect any other closed
    /// dobjects whose bbox is fully inside the outer's bbox AND whose
    /// boundary samples are all inside the outer's polygon — those are
    /// islands. Matches AutoCAD BPOLY's "scan for nested shapes" pass.
    /// Used only by the cheap path; the trace path discovers islands
    /// via its own ray-cast analysis.
    fn collect_islands_inside(&self, outer_idx: usize, seed: Vec2) -> Vec<usize> {
        self.collect_islands_inside_scoped(outer_idx, seed, None)
    }

    /// Scoped variant — see `collect_closed_containing_scoped`.
    fn collect_islands_inside_scoped(
        &self,
        outer_idx: usize,
        seed: Vec2,
        scope: Option<&[usize]>,
    ) -> Vec<usize> {
        let Some(outer_d) = self.doc.dobjects.get(outer_idx) else {
            return Vec::new();
        };
        let outer_polygon = closed_dobject_polygon(&outer_d.geom);
        if outer_polygon.len() < 3 {
            return Vec::new();
        }
        let (omin, omax) = outer_d.geom.bbox();
        let mut islands = Vec::new();
        let iter: Box<dyn Iterator<Item = (usize, &DObject)>> = match scope {
            Some(s) => Box::new(
                s.iter()
                    .filter_map(|&i| self.doc.dobjects.get(i).map(|d| (i, d))),
            ),
            None => Box::new(self.doc.dobjects.iter().enumerate()),
        };
        for (i, d) in iter {
            if i == outer_idx {
                continue;
            }
            if !d.style.visible {
                continue;
            }
            // Must be a self-closed boundary type. Effectively-closed
            // open polylines (endpoint gap < ε) are accepted too.
            let is_candidate = match &d.geom {
                Geom::Polyline(p) => polyline_is_effectively_closed(p),
                Geom::Circle(_) | Geom::Ellipse(_) => true,
                _ => false,
            };
            if !is_candidate {
                continue;
            }
            // bbox-inside test (cheap reject) — must be FULLY inside outer's bbox
            let (bmin, bmax) = d.geom.bbox();
            if !(bmin.x >= omin.x - 1e-9
                && bmin.y >= omin.y - 1e-9
                && bmax.x <= omax.x + 1e-9
                && bmax.y <= omax.y + 1e-9)
            {
                continue;
            }
            let cand_poly = closed_dobject_polygon(&d.geom);
            if cand_poly.len() < 3 {
                continue;
            }
            // If candidate contains the seed it's NOT an island — that
            // would have made it the outer instead (smaller bbox area).
            if point_in_polygon(seed, cand_poly.clone()) {
                continue;
            }
            // Sample test: every k-th vertex of candidate must be inside
            // outer's polygon. Five evenly-spaced samples is enough to
            // catch the common "small circle inside big circle" case
            // without paying full polygon-in-polygon containment.
            let n = cand_poly.len();
            let samples = [
                cand_poly[0],
                cand_poly[n / 5],
                cand_poly[(2 * n) / 5],
                cand_poly[(3 * n) / 5],
                cand_poly[(4 * n) / 5],
            ];
            if samples
                .iter()
                .all(|p| point_in_polygon(*p, outer_polygon.clone()))
            {
                islands.push(i);
            }
        }
        islands
    }

    fn find_smallest_containing_closed(&self, world: Vec2) -> Option<usize> {
        self.find_smallest_containing_closed_scoped(world, None)
    }

    /// Scoped variant — see `collect_closed_containing_scoped`.
    ///
    /// Deliberately delegates to the candidate scan instead of maintaining a
    /// second copy of the eligibility rule (visibility filter, closed-shape
    /// kinds, PIP test). The two functions MUST agree — when only the
    /// collector filtered, the candidate list showed the user's polyline
    /// while this picker returned a hidden `hatch_aux` boundary with a
    /// smaller bbox ("candidates: #16" then "chose #17").
    fn find_smallest_containing_closed_scoped(
        &self,
        world: Vec2,
        scope: Option<&[usize]>,
    ) -> Option<usize> {
        self.collect_closed_containing_scoped(world, scope)
            .into_iter()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _, _)| i)
    }

    /// Smart hatch finaliser: collect handles of every CLOSED polyline
    /// in the current selection and add ONE Hatch dobject that
    /// REFERENCES all of them as boundary loops (outer first, holes
    /// next via even-odd at render). Boundary dobjects stay where
    /// they are — moving / editing them later auto-updates the hatch
    /// because the render path resolves handles each frame.
    ///
    /// Non-closed-polyline selections are silently skipped with a
    /// message. Multi-loop selection = one hatch with islands; single
    /// loop = solid disc.
    /// Bulk-set one style attribute on every dobject in the current
    /// selection. Driven by the `chprop` command. Snapshots the doc
    /// before mutating so a single undo reverts the whole batch.
    /// Unknown property or value → no-op + error in history.
    fn apply_chprop(&mut self, prop: &str, val: &str) {
        if self.selection.is_empty() {
            self.history.push("  ! chprop: empty selection".into());
            return;
        }
        // Resolve VALUE → target style field. Done once before the
        // loop so the error message fires before any mutation.
        enum Target {
            Layer(LayerId),
            Color(cad_kernel::Color),
            Linetype(u32),
        }
        let target = match prop {
            "layer" => match self.doc.layers.find(val) {
                Some(id) => Target::Layer(id),
                None => {
                    let names: Vec<String> = self
                        .doc
                        .layers
                        .layers
                        .iter()
                        .map(|l| l.name.clone())
                        .collect();
                    self.history.push(format!(
                        "  ! chprop: layer '{}' not found — available: {}",
                        val,
                        names.join(", ")
                    ));
                    return;
                }
            },
            "color" => {
                let val_lc = val.to_ascii_lowercase();
                match val_lc.as_str() {
                    "bylayer" => Target::Color(cad_kernel::Color::ByLayer),
                    "byblock" => Target::Color(cad_kernel::Color::ByBlock),
                    _ => match val.parse::<u8>() {
                        Ok(n) => Target::Color(cad_kernel::Color::Aci(n)),
                        Err(_) => {
                            self.history.push(format!(
                                "  ! chprop color '{}': expected ACI 0-255, 'bylayer', or 'byblock'",
                                val));
                            return;
                        }
                    },
                }
            }
            "linetype" => {
                let val_lc = val.to_ascii_lowercase();
                let lt_id = if val_lc == "bylayer" {
                    cad_kernel::LinetypeTable::CONTINUOUS
                } else {
                    match self.doc.linetypes.find(val) {
                        Some(id) => id,
                        None => {
                            let names: Vec<&str> = self
                                .doc
                                .linetypes
                                .linetypes
                                .iter()
                                .map(|l| l.name.as_str())
                                .collect();
                            self.history.push(format!(
                                "  ! chprop linetype '{}' not found — available: {}",
                                val,
                                names.join(", ")
                            ));
                            return;
                        }
                    }
                };
                Target::Linetype(lt_id)
            }
            other => {
                self.history
                    .push(format!("  ! chprop: unknown property '{}'", other));
                return;
            }
        };
        // Snapshot before mutating so one undo reverts the whole batch.
        self.snapshot_doc();
        let mut changed = 0usize;
        for &i in &self.selection {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                match &target {
                    Target::Layer(id) => d.style.layer = *id,
                    Target::Color(c) => d.style.color = *c,
                    Target::Linetype(id) => d.style.linetype = *id,
                }
                changed += 1;
            }
        }
        self.index_dirty = true;
        self.touch_view();
        self.history.push(format!(
            "  ⊛ chprop {}={}: {} dobject(s) updated",
            prop, val, changed
        ));
    }

    fn apply_hatch(&mut self) {
        self.hatch_dbg(format!(
            "apply_hatch() entry — selection.len() = {}, idx = {:?}",
            self.selection.len(),
            self.selection
        ));
        // Verbose dump of every selected dobject so the user can
        // verify the algorithm's view matches what's visible on the
        // canvas. Especially useful for polylines where `closed=false`
        // with a near-zero endpoint gap looks identical to the closed
        // form when rendered.
        for &idx in &self.selection.clone() {
            if let Some(d) = self.doc.dobjects.get(idx) {
                self.hatch_dbg(format!(
                    "    selected #{:02}: {}",
                    idx,
                    describe_verbose(&d.geom)
                ));
            } else {
                self.hatch_dbg(format!("    selected #{:02}: <out of range>", idx));
            }
        }
        let mut handles: Vec<cad_kernel::Handle> = Vec::new();
        let mut skipped = 0_usize;
        // Capture per-idx decisions first WITHOUT borrowing self mutably,
        // then push the log lines + handles after — avoids the
        // hatch_dbg-vs-self.selection borrow conflict.
        let mut decisions: Vec<(usize, &'static str, Option<u64>, Option<usize>)> = Vec::new();
        for &idx in &self.selection {
            let Some(d) = self.doc.dobjects.get(idx) else {
                continue;
            };
            let kind = dobject_kind_name(&d.geom);
            match &d.geom {
                Geom::Polyline(p) if polyline_is_effectively_closed(p) => {
                    decisions.push((idx, kind, Some(d.handle), Some(p.vertices.len())));
                }
                Geom::Circle(_) | Geom::Ellipse(_) => {
                    decisions.push((idx, kind, Some(d.handle), None));
                }
                _ => decisions.push((idx, kind, None, None)),
            }
        }
        for (idx, kind, h_opt, verts_opt) in decisions {
            if let Some(h) = h_opt {
                // AutoCAD semantics (owner ruling): the hatch owns a HIDDEN
                // boundary of its own, and the dobject the user drew stays an
                // independent object. So do NOT bind to `h` — bake the picked
                // shape into an invisible auxiliary polyline and bind to that.
                //
                // This is what the TRACE path has always done; the cheap /
                // select-objects path used to reference the user's dobject
                // directly, which is why a hatched rectangle / polygon / pline
                // dragged its fill along while a traced circle or ellipse did
                // not. Same command, two behaviours — now one.
                match self.materialise_hatch_boundary(idx) {
                    Some(aux) => {
                        handles.push(aux);
                        match verts_opt {
                            Some(nv) => self.hatch_dbg(format!(
                                "  accept #{} ({}, closed, {} verts) → baked to independent \
                                 boundary 0x{:x} (source dobject untouched)",
                                idx, kind, nv, aux
                            )),
                            None => self.hatch_dbg(format!(
                                "  accept #{} ({}) → baked to independent boundary 0x{:x} \
                                 (source dobject untouched)",
                                idx, kind, aux
                            )),
                        }
                    }
                    None => {
                        // Could not tessellate a usable loop — fall back to the
                        // old direct reference rather than dropping the boundary.
                        handles.push(h);
                        self.hatch_dbg(format!(
                            "  accept #{} ({}) — ! could not bake an independent boundary, \
                             referencing the source dobject directly (it will move with the hatch)",
                            idx, kind
                        ));
                    }
                }
            } else {
                skipped += 1;
                self.hatch_dbg(format!(
                    "  skip   #{} ({}) — not a closed boundary",
                    idx, kind
                ));
            }
        }
        if handles.is_empty() {
            self.hatch_dbg("  → 0 boundaries accepted; aborting".to_string());
            self.history
                .push("  ! hatch: no closed boundary in selection".into());
            return;
        }
        // Preview snapshot — taken once at the start of the hatch flow.
        // While the confirm panel is open, all "Change pattern/Scale"
        // tweaks mutate in place; one Discard reverts everything;
        // Confirm promotes the snap onto undo_stack so a future undo
        // reverts the whole flow as one step.
        if self.hatch_preview_snap.is_none() {
            self.hatch_preview_snap = Some(self.doc.clone());
        }
        let loop_count = handles.len();
        // Build pattern from the args the user provided to `hatch`.
        // None → Solid; Some(name) → Pattern { name, scale, angle }.
        // Reset to defaults after consuming so the next bare `hatch`
        // doesn't reuse a stale pattern.
        let (pat_name, pat_scale, pat_angle) =
            std::mem::replace(&mut self.pending_hatch_pattern, (None, 1.0, 0.0));
        let pattern = match pat_name {
            None => cad_kernel::HatchPattern::Solid,
            Some(name) => cad_kernel::HatchPattern::Pattern {
                name: name.to_ascii_uppercase(),
                scale: pat_scale,
                angle_deg: pat_angle,
            },
        };
        let pattern_label = match &pattern {
            cad_kernel::HatchPattern::Solid => "SOLID".to_string(),
            cad_kernel::HatchPattern::Pattern { name, .. } => name.clone(),
        };
        // Phase 2 — pattern selection. Log which scenario was taken and, for a
        // named pattern, whether the name actually resolves. An unknown name
        // silently yields an EMPTY pattern (renders nothing, no error), which
        // is one of the failures that reads to a user as "hatch is broken".
        self.hatch_phase(2, "pattern selection");
        match &pattern {
            cad_kernel::HatchPattern::Solid => self
                .hatch_dbg("    scenario B: SOLID fill (pattern generation bypassed)".to_string()),
            cad_kernel::HatchPattern::Pattern {
                name,
                scale,
                angle_deg,
            } => {
                let known = cad_kernel::patterns::PATTERN_NAMES
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(name));
                self.hatch_dbg(format!(
                    "    scenario A: predefined '{}' scale={:.4} angle={:.2}deg",
                    name, scale, angle_deg
                ));
                if !known {
                    self.hatch_dbg(format!(
                        "    ! UNKNOWN PATTERN '{}' — resolves to an EMPTY definition, \
                         so the hatch will render NOTHING. Known: {}",
                        name,
                        cad_kernel::patterns::PATTERN_NAMES.join(", ")
                    ));
                }
                if *scale <= 0.0 {
                    self.hatch_dbg(format!(
                        "    ! scale {:.4} is not positive — spacing collapses, \
                         expect no visible lines",
                        scale
                    ));
                }
            }
        }
        self.hatch_phase_absent(
            2,
            "scenario C user-defined spacing/double, \
                                    scenario D gradient, scenario E inherit-properties",
        );
        // EDIT-MODE: when the confirm panel's "Change pattern/Scale"
        // button armed this flow, REPLACE the pattern of the last
        // hatch (and its boundary handles, if a new selection was
        // gathered) instead of pushing a new dobject. Empty selection
        // re-uses the existing boundary — typical "tweak the look".
        let edit_target = if self.hatch_dialog_edit_mode {
            self.hatch_last_idx
        } else {
            None
        };
        self.hatch_dialog_edit_mode = false;
        if let Some(idx) = edit_target {
            self.hatch_dbg(format!(
                "  EDIT mode — patching hatch #{} pattern→{}, scale={:.3}, angle={:.2}",
                idx, pattern_label, pat_scale, pat_angle
            ));
            if let Some(d) = self.doc.dobjects.get_mut(idx) {
                if let Geom::Hatch(h) = &mut d.geom {
                    h.pattern = pattern;
                    // Only overwrite boundary if the user selected new
                    // dobjects to hatch. A bare "change pattern/scale"
                    // with no selection just retints the existing fill.
                    if !handles.is_empty() && self.selection.iter().any(|i| *i != idx) {
                        h.boundary_handles = handles;
                    }
                }
            }
            self.touch_view();
            self.index_dirty = true;
            self.history
                .push(format!("  ⊛ hatch #{} updated → {}", idx, pattern_label));
        } else {
            self.hatch_dbg(format!(
                "  pushing Hatch dobject: pattern={}, scale={:.3}, angle={:.2}, {} boundary handle(s)",
                pattern_label, pat_scale, pat_angle, loop_count));
            // §5/WP6.1: a fresh HATCH is an interactive draw → stamp current
            // specs + active layer here (moved out of the now-dumb Document::push).
            let mut hd: DObject = cad_kernel::Hatch {
                boundary_handles: handles,
                pattern,
            }
            .into();
            self.stamp_fresh_style(&mut hd.style);
            self.doc.push(hd);
            self.touch_view();
            self.index_dirty = true;
            self.hatch_last_idx = Some(self.doc.dobjects.len() - 1);
            self.history.push(format!(
                "  + hatch ({}): 1 fill, {} boundary loop(s){}",
                pattern_label,
                loop_count,
                if skipped > 0 {
                    format!("  ({} non-closed-polyline dobject(s) skipped)", skipped)
                } else {
                    String::new()
                },
            ));
        }
        // PAUSE any active pick-point session — otherwise the next
        // canvas click would stack another hatch before the user gets
        // to see/confirm the current one. The panel's "+ Pick Point"
        // button re-arms the session for the next region.
        if self.hatch_pick_point_armed {
            self.hatch_pick_point_armed = false;
            self.hatch_dbg("  pick-point session paused — confirm panel open".to_string());
        }
        // Pop the confirmation panel so the user can decide next steps
        // (Confirm / Discard / Change pattern/Scale / + Pick Point / + Dobject)
        // without having to retype `hatch`. Prompt mirrors the panel.
        self.hatch_confirm_open = true;
        self.set_prompt(
            "hatch preview ready — Confirm(c)/Discard(d)/Change(ch)/+Point(p)/+Dobject(D) — type in cmd OR click panel buttons".to_string());
        // Phases 4/5/7 — verify what the hatch we just created actually
        // RESOLVES and GENERATES. Both of these can come back empty while the
        // command reports success, which is what makes them hard to report.
        self.hatch_verify_last();
        // Auto-dump after each apply so the log captures bbox + line-count
        // diagnostics for every hatch in the document.
        self.dump_hatch_state();
    }

    /// Post-creation verification of the most recently created hatch
    /// (workflow phases 4, 5 and 7). Resolves its boundary loops and runs the
    /// real pattern generator over them, then reports anything that would make
    /// the fill invisible — the failure modes that otherwise produce a
    /// successfully-created hatch that simply does not appear:
    ///   * boundary handles that no longer resolve (deleted / hidden layer)
    ///   * a loop with fewer than 3 vertices
    ///   * a pattern whose spacing x scale exceeds the region (zero lines)
    ///   * a runaway cap hit inside the generator (silently emits nothing)
    fn hatch_verify_last(&mut self) {
        let Some(idx) = self.hatch_last_idx else {
            return;
        };
        let Some(d) = self.doc.dobjects.get(idx) else {
            return;
        };
        let Geom::Hatch(hh) = &d.geom else { return };
        // Copy what we need out of the document up front — the logging calls
        // below take `&mut self`.
        let (h, pattern) = (hh.clone(), hh.pattern.clone());
        let (handle, layer) = (d.handle, d.style.layer);
        self.hatch_phase(4, "boundary processing — resolve loops");
        let declared = h.boundary_handles.len();
        let loops = cad_kernel::resolve_hatch_loops(&h, &self.doc);
        self.hatch_dbg(format!(
            "    {} handle(s) declared -> {} loop(s) resolved",
            declared,
            loops.len()
        ));
        if loops.len() < declared {
            self.hatch_dbg(format!(
                "    ! {} boundary handle(s) did NOT resolve (deleted, or on a \
                 hidden/frozen layer) — those loops contribute nothing",
                declared - loops.len()
            ));
        }
        if loops.is_empty() {
            self.hatch_dbg(
                "    ! NO loops resolved — this hatch cannot render \
                            anything at all"
                    .to_string(),
            );
            return;
        }
        for (i, l) in loops.iter().enumerate() {
            let role = if i == 0 { "outer" } else { "island(even-odd)" };
            if l.len() < 3 {
                self.hatch_dbg(format!(
                    "    ! loop {} ({}) has {} vertex/vertices (<3) — degenerate, dropped",
                    i,
                    role,
                    l.len()
                ));
            }
        }
        self.hatch_phase_absent(
            4,
            "self-intersection check, coincident-overlap check, \
                                    loop-orientation (CCW outer / CW island), gap tolerance",
        );

        // Phase 5 — run the real generator and count what comes out.
        self.hatch_phase(5, "pattern generation + clipping");
        match &pattern {
            cad_kernel::HatchPattern::Solid => {
                self.hatch_dbg(
                    "    SOLID — no pattern lines; filled by \
                                triangulation with even-odd holes"
                        .to_string(),
                );
            }
            cad_kernel::HatchPattern::Pattern {
                name,
                scale,
                angle_deg,
            } => {
                let (segs, circs) = cad_kernel::patterns::hatch_geometry(
                    &loops,
                    &cad_kernel::patterns::lookup(name),
                    *scale,
                    *angle_deg,
                );
                self.hatch_dbg(format!(
                    "    '{}' scale={:.4} angle={:.2} -> {} clipped segment(s), {} circle(s)",
                    name,
                    scale,
                    angle_deg,
                    segs.len(),
                    circs.len()
                ));
                if segs.is_empty() && circs.is_empty() {
                    // The doc's "Very small areas" / "Pattern too dense" cases.
                    let (mut mn, mut mx) =
                        (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
                    for l in &loops {
                        for v in l {
                            mn.x = mn.x.min(v.x);
                            mn.y = mn.y.min(v.y);
                            mx.x = mx.x.max(v.x);
                            mx.y = mx.y.max(v.y);
                        }
                    }
                    self.hatch_dbg(format!(
                        "    ! PATTERN PRODUCED NOTHING — region is {:.4} x {:.4}. \
                         Either the spacing (x scale {:.4}) is larger than the region, \
                         the pattern name is unknown, or a generation cap was hit. \
                         Lower the scale to make lines appear.",
                        mx.x - mn.x,
                        mx.y - mn.y,
                        scale
                    ));
                }
            }
        }
        self.hatch_phase(7, "hatch object created");
        self.hatch_dbg(format!(
            "    dobject #{} handle=0x{:x} layer={} — associativity: BY HANDLE \
             (boundary geometry is NOT baked into the hatch)",
            idx, handle, layer
        ));
    }

    /// Pick-point hatch: BPOLY-style flow. First tries the cheap
    /// "self-closed dobject containing the seed" path; if that fails,
    /// runs the full trace pipeline (tessellate doc + endpoint graph
    /// + horizontal ray cast + CCW-turn loop walk + island classify)
    /// to discover a closed boundary surrounding the seed even when
    /// it's formed by a chain of separate line/arc/polyline dobjects.
    ///
    /// On trace success, materialises every loop (outer + islands) as
    /// a new closed Polyline dobject on the current layer, then pushes
    /// a Hatch referencing those new dobjects' handles. Materialised
    /// polylines are normal dobjects — the user can grip-edit them
    /// later (the hatch updates because it's handle-referenced).
    /// Spawn the trace pipeline on a worker thread. Mid-op Esc fires
    /// because the worker reads `op_cancel` between phases (and inside
    /// the O(N²) split scan) while the UI thread keeps spinning;
    /// `poll_hatch_worker` drains the result the frame after the
    /// worker finishes.
    ///
    /// Replaces any existing in-flight worker (cancel old + spawn
    /// new). The previous worker's result is silently discarded — its
    /// receiver gets dropped.
    /// Cancel any in-flight hatch-trace worker and DROP its receiver, so a
    /// completed-but-undrained trace can't materialize after the cancel —
    /// the Background-Ops invariant I-3 ("cancel = drop the result"). Setting
    /// `op_cancel` only makes the worker exit sooner; dropping the receiver
    /// (`hatch_worker = None`) is what guarantees `poll_hatch_worker` won't
    /// apply the stale result (it early-returns when `hatch_worker` is None).
    /// Single-sourced so Esc, a new command, and spawn-replace all cancel
    /// identically. Returns whether a worker was running.
    fn cancel_hatch_worker(&mut self) -> bool {
        if self.hatch_worker.is_some() {
            self.op_cancel.store(true, Ordering::Relaxed);
            self.hatch_worker = None;
            true
        } else {
            false
        }
    }

    fn spawn_hatch_worker(&mut self, seed: Vec2) {
        let scope = self
            .viewport_scope()
            .unwrap_or_else(|| (0..self.doc.dobjects.len()).collect());
        self.spawn_hatch_worker_scoped(seed, scope);
    }

    fn spawn_hatch_worker_scoped(&mut self, seed: Vec2, scope: Vec<usize>) {
        // If a worker is already running, cancel it. The old thread
        // will exit at its next cancel-check; its send will fail
        // harmlessly because we drop the receiver here.
        if self.cancel_hatch_worker() {
            self.hatch_dbg("  (cancelled previous in-flight hatch worker)");
        }
        // Build the pattern args from the current pick-point session
        // snapshot (or fall back to SOLID if no session — shouldn't
        // happen via Pick Point button, but defensive).
        let (pat_name, pat_scale, pat_angle) = self
            .hatch_pick_point_session
            .clone()
            .unwrap_or_else(|| self.pending_hatch_pattern.clone());
        let pattern = match pat_name {
            None => cad_kernel::HatchPattern::Solid,
            Some(name) => cad_kernel::HatchPattern::Pattern {
                name: name.to_ascii_uppercase(),
                scale: pat_scale,
                angle_deg: pat_angle,
            },
        };
        let active_layer = self.doc.layers.active;
        // Fresh cancel flag for this worker — old flag may have just
        // been set to cancel the previous worker. Replace with new.
        let cancel = StdArc::new(AtomicBool::new(false));
        self.op_cancel = cancel.clone();
        // Snapshot only the scoped dobjects (~tens to a few thousand),
        // not the whole Document. At 9M dobjects, `self.doc.clone()`
        // is ~1 GB of memcpy — multi-second freeze per pick-point click.
        // The worker's tessellation only ever touches `scope`, so the
        // dobjects outside scope are dead weight in the snapshot.
        //
        // The new doc carries the small ancillary tables (layers,
        // linetypes, pens, truecolors) verbatim — they're cheap and
        // the worker may resolve colors / styles through them.
        // `scope_for_thread` is remapped to `[0..scoped_n)` since the
        // dobjects are now at fresh contiguous indices in the snapshot.
        let mut doc_snapshot = cad_kernel::Document::default();
        doc_snapshot.layers = self.doc.layers.clone();
        doc_snapshot.linetypes = self.doc.linetypes.clone();
        doc_snapshot.pens = self.doc.pens.clone();
        doc_snapshot.truecolors = self.doc.truecolors.clone();
        doc_snapshot.text_styles = self.doc.text_styles.clone();
        doc_snapshot.dobjects = scope
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).cloned())
            .collect();
        let scoped_n = doc_snapshot.dobjects.len();
        // The worker's scope is now the full range of the snapshot
        // (every dobject in the snapshot WAS in the original scope).
        // No remapping needed downstream — poll_hatch_worker only
        // consumes the trace's vertex loops, not the src indices.
        let scope_for_thread: Vec<usize> = (0..scoped_n).collect();
        // Text glyph boundaries are rendered HERE (main thread — the
        // FontManager is not Send) against the snapshot's indices, then
        // moved into the worker.
        let text_for_thread = {
            let mut fm = self.font_manager.borrow_mut();
            crate::hatch_trace::text_hatch_geom(&doc_snapshot, &mut fm, &scope_for_thread)
        };
        let cancel_for_thread = cancel.clone();
        let (tx, rx) = mpsc::channel::<HatchWorkerResult>();
        thread::spawn(move || {
            // A3: a worker panic must still send a Failure (not die silently),
            // so `poll_hatch_worker` can log it instead of leaving the doc
            // untouched with a "hatching…" prompt stuck on screen.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut log: Vec<String> = Vec::new();
                log.push(format!(
                    "  worker started: seed=({:.3},{:.3}), {} dobjects in viewport scope ({} total)",
                    seed.x, seed.y, scope_for_thread.len(), doc_snapshot.dobjects.len()));
                // Single end-to-end call — tessellates only viewport dobjects,
                // splits at intersections, traces. Cancellable between phases.
                // The `_diag` variant additionally reports a count per stage,
                // per-stage timings, a typed failure reason, and a gap probe.
                let (tb, diag) = crate::hatch_trace::trace_boundary_at_in_view_diag(
                    &doc_snapshot,
                    &scope_for_thread,
                    seed,
                    &cancel_for_thread,
                    &text_for_thread,
                );
                // Every stage of boundary detection, in order — this is the
                // section that used to be a black box.
                log.extend(diag.lines());
                if cancel_for_thread.load(Ordering::Relaxed) {
                    return HatchWorkerResult::Cancelled { log_lines: log };
                }
                match tb {
                    Some(tb) => {
                        log.push(format!(
                            "  worker: traced outer {} verts, {} island(s)",
                            tb.outer.len(),
                            tb.islands.len()
                        ));
                        let mut loops = Vec::with_capacity(1 + tb.islands.len());
                        loops.push(tb.outer);
                        loops.extend(tb.islands);
                        HatchWorkerResult::Success {
                            loops,
                            log_lines: log,
                        }
                    }
                    None => {
                        // Typed reason instead of "trace returned None", and the
                        // gap probe (if any) turns the dead end into an
                        // instruction the user can act on.
                        let mut reason = diag
                            .fail
                            .as_ref()
                            .map(|f| f.describe())
                            .unwrap_or_else(|| "trace produced no boundary".into());
                        if let Some(g) = &diag.gap_probe {
                            reason.push_str(&format!(
                                " — it WOULD close at tolerance {:e} ({}x default), \
                                 so look for a gap of about that size",
                                g.eps, g.factor as i64
                            ));
                        }
                        log.push(format!("  worker: {}", reason));
                        HatchWorkerResult::Failure {
                            reason,
                            log_lines: log,
                        }
                    }
                }
            }));
            match result {
                Ok(r) => {
                    let _ = tx.send(r);
                }
                Err(msg) => {
                    let detail = if let Some(s) = msg.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = msg.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic payload".to_string()
                    };
                    let _ = tx.send(HatchWorkerResult::Failure {
                        reason: format!("hatch worker panicked: {}", detail),
                        log_lines: vec![format!("  worker PANIC: {}", detail)],
                    });
                }
            }
        });
        self.hatch_worker = Some(HatchWorker {
            seed,
            pattern,
            active_layer,
            cancel,
            rx,
        });
        self.hatch_dbg(format!(
            "  spawned hatch worker for seed ({:.3},{:.3}) — UI stays responsive; Esc cancels",
            seed.x, seed.y
        ));
        self.set_prompt("hatching… UI stays responsive — press Esc to cancel".to_string());
    }

    /// Called every frame from `update`. Drains the worker channel if
    /// the trace has completed; materialises the result into the doc
    /// (Success) or logs the failure / cancel and falls back to the
    /// cheap path if applicable (Failure).
    fn poll_hatch_worker(&mut self) {
        let Some(worker) = &self.hatch_worker else {
            return;
        };
        let result = match worker.rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                // Worker thread died without sending (e.g. panic).
                // Clear the field and log; don't fall back blindly.
                self.hatch_dbg(
                    "  worker disconnected without result (likely cancelled or panicked)",
                );
                self.hatch_worker = None;
                return;
            }
        };
        // Take ownership of the worker so we can mutate `self.doc` etc.
        let worker = self.hatch_worker.take().unwrap();
        match result {
            HatchWorkerResult::Success { loops, log_lines } => {
                for line in log_lines {
                    self.hatch_dbg(line);
                }
                // Off-screen warning: if the traced outer loop extends
                // beyond the current viewport bbox, the user can't see
                // the full hatch. Tell them.
                if let (Some((vmin, vmax)), Some(outer)) = (self.last_visible, loops.first()) {
                    let mut omin = Vec2::new(f64::INFINITY, f64::INFINITY);
                    let mut omax = Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
                    for v in outer {
                        if v.x < omin.x {
                            omin.x = v.x;
                        }
                        if v.y < omin.y {
                            omin.y = v.y;
                        }
                        if v.x > omax.x {
                            omax.x = v.x;
                        }
                        if v.y > omax.y {
                            omax.y = v.y;
                        }
                    }
                    let outside =
                        omin.x < vmin.x || omin.y < vmin.y || omax.x > vmax.x || omax.y > vmax.y;
                    if outside {
                        self.history.push(
                            "  ⚠ traced hatch boundary extends off-screen — zoom out to see the full region"
                            .into());
                        self.hatch_dbg(
                            "  worker: outer bbox extends beyond viewport — zoom out to see full hatch");
                    }
                }
                // Preview snapshot (see apply_hatch's twin) — taken once
                // per hatch flow, restored on Discard, promoted to undo
                // on Confirm.
                if self.hatch_preview_snap.is_none() {
                    self.hatch_preview_snap = Some(self.doc.clone());
                }
                let handles = self.hatch_bake_loops(loops, worker.active_layer);
                if handles.is_empty() {
                    self.hatch_dbg("  worker result had no usable polylines");
                    self.clear_prompt();
                    return;
                }
                let pattern_label = match &worker.pattern {
                    cad_kernel::HatchPattern::Solid => "SOLID".to_string(),
                    cad_kernel::HatchPattern::Pattern { name, .. } => name.clone(),
                };
                let n_loops = handles.len();
                self.commit_baked_hatch(
                    handles,
                    worker.pattern.clone(),
                    worker.active_layer,
                    format!(
                        "  + hatch ({}): traced boundary, {} loop(s) materialised (async)",
                        pattern_label, n_loops
                    ),
                );
            }
            HatchWorkerResult::Failure { reason, log_lines } => {
                for line in log_lines {
                    self.hatch_dbg(line);
                }
                self.hatch_dbg(format!("  worker failure: {}", reason));
                // Fall back to cheap path with auto-islands — same
                // logic apply_pick_point_hatch's sync fallback uses.
                let seed = worker.seed;
                // Viewport-scoped (mirror apply_pick_point_hatch): only
                // in-view text can contain the in-view click, so the TEXT
                // PATH fallback below never renders off-screen glyphs.
                let _ = self.ensure_index();
                let fallback_scope: Vec<usize> = self
                    .viewport_scope()
                    .unwrap_or_else(|| (0..self.doc.dobjects.len()).collect());
                if let Some(idx) = self.find_smallest_containing_closed(seed) {
                    let kind = self
                        .doc
                        .dobjects
                        .get(idx)
                        .map(|d| dobject_kind_name(&d.geom))
                        .unwrap_or("?");
                    let islands = self.collect_islands_inside(idx, seed);
                    self.hatch_dbg(format!(
                        "  fallback → CHEAP PATH outer #{} ({}) + {} island(s)",
                        idx,
                        kind,
                        islands.len()
                    ));
                    // Restore the pending pattern so apply_hatch picks it up.
                    if let Some(sess) = self.hatch_pick_point_session.clone() {
                        self.pending_hatch_pattern = sess;
                    }
                    self.selection = std::iter::once(idx).chain(islands).collect();
                    self.apply_hatch();
                    self.selection.clear();
                } else if let Some(loops) = self.text_hatch_loops_at(seed, &fallback_scope) {
                    // Text fallback: a glyph loop containing the click —
                    // the letter itself is the boundary (outer + counters).
                    self.hatch_dbg(format!(
                        "  fallback → TEXT PATH {} glyph loop(s)",
                        loops.len()
                    ));
                    // Preview snapshot (see the Success arm's twin) — taken
                    // BEFORE any bake so Discard reverts the aux polylines +
                    // hatch and Confirm promotes the pre-hatch state onto the
                    // undo stack. Without it, Discard is a no-op and the
                    // hatch can never be undone.
                    if self.hatch_preview_snap.is_none() {
                        self.hatch_preview_snap = Some(self.doc.clone());
                    }
                    // Same pattern restore as the cheap fallback above —
                    // the session's pattern (if armed) wins over whatever
                    // the pending hatch pattern holds.
                    if let Some(sess) = self.hatch_pick_point_session.clone() {
                        self.pending_hatch_pattern = sess;
                    }
                    let (pat_name, pat_scale, pat_angle) =
                        std::mem::replace(&mut self.pending_hatch_pattern, (None, 1.0, 0.0));
                    let pattern = match pat_name {
                        None => cad_kernel::HatchPattern::Solid,
                        Some(name) => cad_kernel::HatchPattern::Pattern {
                            name: name.to_ascii_uppercase(),
                            scale: pat_scale,
                            angle_deg: pat_angle,
                        },
                    };
                    let pattern_label = match &pattern {
                        cad_kernel::HatchPattern::Solid => "SOLID".to_string(),
                        cad_kernel::HatchPattern::Pattern { name, .. } => name.clone(),
                    };
                    let layer = self.doc.layers.active;
                    let handles = self.hatch_bake_loops(loops, layer);
                    if handles.is_empty() {
                        self.hatch_dbg("  text fallback had no usable glyph loops");
                        self.history.push(
                            "  ! hatch pick-point: no closed boundary contains the click".into(),
                        );
                    } else {
                        let n_loops = handles.len();
                        self.commit_baked_hatch(
                            handles,
                            pattern,
                            layer,
                            format!(
                                "  + hatch ({}): TEXT PATH, {} glyph loop(s) materialised",
                                pattern_label, n_loops
                            ),
                        );
                    }
                } else {
                    self.history
                        .push("  ! hatch pick-point: no closed boundary contains the click".into());
                }
                // Restore prompt if session armed. If a fallback just
                // committed (cheap/text path), the commit already paused the
                // session and set the confirm-panel prompt — don't clear it.
                if self.hatch_pick_point_armed {
                    let style = self
                        .hatch_pick_point_session
                        .as_ref()
                        .and_then(|(n, _, _)| n.clone())
                        .unwrap_or_else(|| "SOLID".to_string());
                    self.set_prompt(format!(
                        "hatch ({}): click another region OR Enter to finish  [Esc=cancel]",
                        style
                    ));
                } else if !self.hatch_confirm_open {
                    self.clear_prompt();
                }
            }
            HatchWorkerResult::Cancelled { log_lines } => {
                for line in log_lines {
                    self.hatch_dbg(line);
                }
                self.hatch_dbg("  worker: trace cancelled by user (Esc)");
                self.history.push("  hatch trace cancelled".into());
                self.clear_prompt();
            }
        }
    }

    /// Bake the closed shape at `src_idx` into an INVISIBLE auxiliary polyline
    /// owned by the hatch, and return its handle.
    ///
    /// This is what keeps a hatch independent of the geometry the user drew:
    /// the hatch references this copy, so moving / erasing / editing the
    /// original leaves the fill alone (AutoCAD non-associative behaviour, and
    /// the owner's ruling — "hatch will have hidden boundary of itself and
    /// original boundary will remain as independent dobject").
    ///
    /// The copy is flagged `hatch_aux` so `resolve_hatch_loops` still resolves
    /// it even though `visible` is false (a user-hidden boundary is skipped;
    /// a synthetic one is not). Returns `None` when the shape does not
    /// tessellate to a usable closed loop, so the caller can fall back.
    fn materialise_hatch_boundary(&mut self, src_idx: usize) -> Option<cad_kernel::Handle> {
        let src = self.doc.dobjects.get(src_idx)?;
        let layer = src.style.layer;
        // Single-loop case of `hatch_bake_loops` — the dedup / ≥3-vert /
        // invisible + hatch_aux flag rules live in exactly one place.
        // (`closed_dobject_polygon` may repeat the first vertex; the bake
        // drops the closing duplicate.)
        self.hatch_bake_loops(vec![closed_dobject_polygon(&src.geom)], layer)
            .into_iter()
            .next()
    }

    /// Bake traced/glyph vertex loops into SYNTHETIC boundary polylines on
    /// `layer` and return their handles. Each loop becomes one invisible
    /// (`visible` false) but still resolved (`hatch_aux`) closed polyline —
    /// never rendered, existing only so the hatch has something to reference
    /// by handle. Loops with fewer than 3 usable vertices are dropped.
    /// Returns handles in order; empty when nothing was usable.
    fn hatch_bake_loops(
        &mut self,
        loops: Vec<Vec<Vec2>>,
        layer: cad_kernel::LayerId,
    ) -> Vec<cad_kernel::Handle> {
        let mut handles: Vec<cad_kernel::Handle> = Vec::new();
        for loop_verts in loops {
            let mut verts: Vec<cad_kernel::PolyVertex> = loop_verts
                .iter()
                .map(|v| cad_kernel::PolyVertex {
                    pos: *v,
                    bulge: 0.0,
                })
                .collect();
            if verts.len() >= 2 {
                let first = verts[0].pos;
                let last = verts[verts.len() - 1].pos;
                if (last - first).len() < crate::hatch_trace::JOIN_EPS {
                    verts.pop();
                }
            }
            if verts.len() < 3 {
                continue;
            }
            let pl = cad_kernel::Polyline {
                vertices: verts,
                closed: true,
                widths: Vec::new(),
            };
            let mut d = cad_kernel::DObject::from(pl);
            d.style = cad_kernel::Style::on_layer(layer);
            // Synthetic auxiliary boundary — exists only so the
            // hatch has something to reference by handle. Don't
            // render it; the user never asked for this line on
            // the drawing. `hatch_aux` tells the hatch-loop
            // resolver this is a synthetic boundary (still
            // resolves) rather than a user-hidden one (skipped).
            d.style.visible = false;
            d.style.hatch_aux = true;
            let idx = self.doc.push(d);
            handles.push(self.doc.dobjects[idx].handle);
        }
        handles
    }

    /// Push a Hatch dobject bound to already-baked boundary handles, mark
    /// the scene dirty, and open the confirm panel (pausing any active
    /// pick-point session so consecutive clicks don't stack hatches before
    /// the user confirms or discards the first one).
    fn commit_baked_hatch(
        &mut self,
        handles: Vec<cad_kernel::Handle>,
        pattern: cad_kernel::HatchPattern,
        layer: cad_kernel::LayerId,
        history_msg: String,
    ) {
        let mut d = cad_kernel::DObject::from(cad_kernel::Hatch {
            boundary_handles: handles,
            pattern,
        });
        d.style = cad_kernel::Style::on_layer(layer);
        self.doc.push(d);
        self.touch_view();
        self.index_dirty = true;
        self.hatch_last_idx = Some(self.doc.dobjects.len() - 1);
        self.history.push(history_msg);
        self.dump_hatch_state();
        // PAUSE any active pick-point session and open the confirm
        // panel — otherwise consecutive clicks would stack hatches
        // before the user gets to confirm or discard the first one.
        // The panel's "+ Pick Point" button re-arms the session.
        if self.hatch_pick_point_armed {
            self.hatch_pick_point_armed = false;
            self.hatch_dbg("  pick-point session paused — confirm panel open".to_string());
        }
        self.hatch_confirm_open = true;
        self.set_prompt(
            "hatch preview ready — Confirm(c)/Discard(d)/Change(ch)/+Point(p)/+Dobject(D) — type in cmd OR click panel buttons".to_string());
    }

    fn apply_pick_point_hatch(&mut self, seed: Vec2) -> bool {
        // === Viewport scope ===
        // Restrict EVERY hatch operation to the dobjects whose bbox
        // overlaps the current viewport — at 400k+ dobjects, iterating
        // the full doc per click is unworkable. The spatial index
        // makes this O(visible cells) instead of O(N). Fallback to
        // full doc only when the index is stale or no frame has
        // rendered yet.
        //
        // CRITICAL: rebuild the index FIRST if it's dirty. Otherwise
        // viewport_scope() falls back to full-doc range, which defeats
        // the whole point of scoping. This bug bit on the 2nd click in
        // a multi-pick session — the 1st hatch's `index_dirty = true`
        // turned the 2nd click into a full-doc scan.
        let _ = self.ensure_index();
        let view_scope: Vec<usize> = self
            .viewport_scope()
            .unwrap_or_else(|| (0..self.doc.dobjects.len()).collect());
        let total = self.doc.dobjects.len();
        let in_view = view_scope.len();
        self.hatch_dbg(format!(
            "  --- viewport scope: {} of {} dobjects in view ({:.1}%) — iterating scope only ---",
            in_view,
            total,
            100.0 * in_view as f32 / total.max(1) as f32
        ));

        // Verbose dump: every dobject + every coordinate the trace
        // pipeline will see this click. Lets the user verify "is this
        // polyline actually closed?", "do these line endpoints really
        // meet?" without guessing. Skipped when the debug window is
        // closed (`hatch_dbg` is a no-op then).
        // ONLY in-view dobjects are dumped — out-of-view ones can't
        // affect the hatch in the visible region.
        self.hatch_dbg(format!(
            "  --- doc snapshot at pick-point click ({:.3},{:.3})  zoom={:.2} px/world  world_per_px={:.4} ---",
            seed.x, seed.y, self.scale, 1.0 / (self.scale as f64).max(1e-9)));
        let doc_lines: Vec<String> = view_scope
            .iter()
            .filter_map(|&i| self.doc.dobjects.get(i).map(|d| (i, d)))
            .map(|(i, d)| {
                let (bmin, bmax) = d.geom.bbox();
                let closed_tag = match &d.geom {
                    Geom::Polyline(p) => {
                        if p.closed {
                            " [closed]".to_string()
                        } else if polyline_is_effectively_closed(p) {
                            " [closed-by-gap]".to_string()
                        } else {
                            " [open]".to_string()
                        }
                    }
                    Geom::Circle(_) | Geom::Ellipse(_) => " [closed]".to_string(),
                    _ => String::new(),
                };
                format!(
                    "    #{:02} [{}] bbox=({:.2},{:.2})→({:.2},{:.2}){} {}",
                    i,
                    if d.style.visible { "vis" } else { "hid" },
                    bmin.x,
                    bmin.y,
                    bmax.x,
                    bmax.y,
                    closed_tag,
                    describe_verbose(&d.geom)
                )
            })
            .collect();
        for line in doc_lines {
            self.hatch_dbg(line);
        }
        // Cheap path first: hits the typical "click inside one closed
        // shape" workflow without paying for the trace.
        // Per-dobject verdict log so the user can see exactly WHY each
        // dobject was accepted / rejected as a candidate. The actual
        // decision still comes from collect_closed_containing — this
        // is pure instrumentation.
        self.hatch_dbg("  --- cheap-path verdict per dobject (viewport only) ---");
        let verdict_lines: Vec<String> = view_scope.iter().filter_map(|&i| {
                self.doc.dobjects.get(i).map(|d| (i, d))
            })
            .map(|(i, d)| {
                let kind = dobject_kind_name(&d.geom);
                // Mirror the scan's own guard so the log explains the skip
                // rather than silently omitting a dobject the reader can see
                // listed in the snapshot above.
                if !self.doc.is_visible(i) {
                    return format!(
                        "    #{:02} {} — skip (not visible: hidden{}, or off/frozen layer)",
                        i, kind,
                        if d.style.hatch_aux { " — synthetic hatch boundary" } else { "" });
                }
                match &d.geom {
                    Geom::Polyline(p) => {
                        if !polyline_is_effectively_closed(p) {
                            format!("    #{:02} {} — SKIP (open polyline, gap≥{:.0e})",
                                i, kind, POLYLINE_EFFECTIVELY_CLOSED_EPS)
                        } else {
                            let verts = closed_dobject_polygon(&d.geom);
                            if point_in_polygon(seed, verts) {
                                let (bmin, bmax) = d.geom.bbox();
                                let area = (bmax.x - bmin.x).abs() * (bmax.y - bmin.y).abs();
                                format!("    #{:02} {} — CANDIDATE (contains seed, bbox area {:.3})",
                                    i, kind, area)
                            } else {
                                format!("    #{:02} {} — skip (closed but does NOT contain seed)", i, kind)
                            }
                        }
                    }
                    Geom::Circle(c) => {
                        let dist = (seed - c.center).len();
                        if dist < c.radius {
                            let area = (2.0 * c.radius).powi(2);
                            format!("    #{:02} {} — CANDIDATE (contains seed, dist {:.3} < r {:.3}, bbox area {:.3})",
                                i, kind, dist, c.radius, area)
                        } else {
                            format!("    #{:02} {} — skip (seed dist {:.3} ≥ r {:.3})",
                                i, kind, dist, c.radius)
                        }
                    }
                    Geom::Ellipse(e) => {
                        let dvec = seed - e.center;
                        let a = e.semi_major().max(1e-12);
                        let b = e.semi_minor().max(1e-12);
                        let u = dvec.dot(e.u_hat()) / a;
                        let v = dvec.dot(e.v_hat()) / b;
                        let val = u * u + v * v;
                        if val < 1.0 {
                            format!("    #{:02} {} — CANDIDATE (contains seed, u²+v²={:.3} < 1)",
                                i, kind, val)
                        } else {
                            format!("    #{:02} {} — skip (u²+v²={:.3} ≥ 1)", i, kind, val)
                        }
                    }
                    _ => format!("    #{:02} {} — SKIP (not a self-closed boundary type)", i, kind),
                }
            }).collect();
        for line in verdict_lines {
            self.hatch_dbg(line);
        }
        let cheap_candidates = self.collect_closed_containing_scoped(seed, Some(&view_scope));
        if !cheap_candidates.is_empty() {
            self.hatch_dbg(format!(
                "  --- cheap-path candidates ({} found, picked smallest by bbox area) ---",
                cheap_candidates.len()
            ));
            for (idx, area, kind) in &cheap_candidates {
                self.hatch_dbg(format!("    #{:02} {} — bbox area {:.3}", idx, kind, area));
            }
        } else {
            self.hatch_dbg("  --- cheap-path: 0 self-closed dobjects contain the click ---");
        }
        // Routing rule:
        //  * 0 closed candidates contain the seed → trace path (the
        //    boundary is formed by open primitives chained together).
        //  * Exactly 1 candidate AND no other dobject crosses its
        //    boundary → cheap path (single self-closed boundary +
        //    auto-islands inside).
        //  * Exactly 1 candidate BUT its boundary is crossed by some
        //    other dobject → trace path (the planar subdivision has
        //    sub-faces inside the outer; the cheap path would
        //    incorrectly hatch the whole outer).
        //  * Exactly 1 candidate BUT text overlaps it → trace path: the
        //    letters must be punched out as islands, and only the trace
        //    carries their glyph contours.
        //  * 2+ candidates contain the seed → trace path.
        let multiple = cheap_candidates.len() > 1;
        let single_outer_crossed = !multiple
            && cheap_candidates.len() == 1
            && self.outer_has_crossings_with_others(cheap_candidates[0].0);
        if single_outer_crossed {
            self.hatch_dbg(format!(
                "  --- single outer #{} has crossings with other dobjects → deferring to trace path ---",
                cheap_candidates[0].0));
        }
        // Viewport-scoped like every sibling scan here — text outside the
        // viewport can't overlap an in-view outer, so a full-doc sweep per
        // click is pure waste (no early exit on text-less drawings either).
        let single_outer_overlaps_text = !multiple
            && !single_outer_crossed
            && cheap_candidates.len() == 1
            && self.outer_overlaps_text_scoped(cheap_candidates[0].0, Some(&view_scope));
        if single_outer_overlaps_text {
            self.hatch_dbg(format!(
                "  --- single outer #{} overlaps text → deferring to trace path (letters are shapes) ---",
                cheap_candidates[0].0));
        }
        if !multiple && !single_outer_crossed && !single_outer_overlaps_text {
            if let Some(idx) = self.find_smallest_containing_closed_scoped(seed, Some(&view_scope))
            {
                let kind = self
                    .doc
                    .dobjects
                    .get(idx)
                    .map(|d| dobject_kind_name(&d.geom))
                    .unwrap_or("?");
                // AutoCAD BPOLY also auto-adds any closed dobjects sitting
                // ENTIRELY INSIDE the chosen outer as islands — the user
                // doesn't have to pre-select them. Scoped to viewport.
                self.hatch_dbg(format!(
                    "  --- island scan inside outer #{} (viewport only) ---",
                    idx
                ));
                let scan_lines = self.dbg_island_scan_verdicts(idx, seed);
                for line in scan_lines {
                    self.hatch_dbg(line);
                }
                let islands = self.collect_islands_inside_scoped(idx, seed, Some(&view_scope));
                if !islands.is_empty() {
                    self.hatch_dbg(format!(
                        "  → CHEAP PATH chose outer #{} ({}) + auto-detected {} island(s): {:?}",
                        idx,
                        kind,
                        islands.len(),
                        islands
                    ));
                } else {
                    self.hatch_dbg(format!(
                        "  → CHEAP PATH chose smallest containing dobject #{} ({}), 0 auto-islands",
                        idx, kind
                    ));
                }
                self.selection = std::iter::once(idx).chain(islands).collect();
                self.apply_hatch();
                self.selection.clear();
                return true;
            }
        } else if multiple {
            self.hatch_dbg(format!(
                "  --- {} candidates contain seed → PARTIAL OVERLAP, deferring to trace path ---",
                cheap_candidates.len()
            ));
        }
        // (the single_outer_crossed log was emitted above before the branch)
        // Full trace path — handles arbitrary chains AND partial overlaps
        // (intersection-splitting inside trace_boundary_at).
        //
        // Backgrounded: spawn the heavy work (tessellate + split +
        // cluster + trace) on a worker thread. The UI keeps spinning
        // and Esc presses are seen mid-op via the cancel flag.
        // `poll_hatch_worker` (called each frame from `update`)
        // drains the result and materialises the hatch when ready.
        self.hatch_dbg("  --- TRACE PATH engaged (async, worker thread) ---");
        self.spawn_hatch_worker(seed);
        true
    }

    /// Click on a dobject during a selection session. Plain click = ADD
    /// (no-op if already in the basket). Shift+click = REMOVE (no-op if
    /// not in the basket). The persistent `remove` sub-command from the
    /// command line flips the default: while it's on, plain clicks act
    /// like Shift+clicks and Shift+clicks act like plain clicks.
    #[track_caller]
    /// Selection click model:
    ///   • Alt (or active deselect mode) → REMOVE the dobject.
    ///   • Shift                         → ADD to the current selection.
    ///   • plain + `fresh`               → REPLACE (select only this).
    ///   • plain inside a command select session → ADD (accumulate the basket).
    /// `fresh` is true for pointer-mode picks (a new selection bunch).
    fn click_select(&mut self, i: usize, shift: bool, alt: bool, fresh: bool) {
        let basket_before = self.selection.clone();
        let remove = alt || (self.select_remove_mode && !shift);
        if remove {
            if let Some(pos) = self.selection.iter().position(|&x| x == i) {
                self.selection.remove(pos);
                self.history.push(format!("    – #{} removed", i));
            } else {
                self.history.push(format!("    (skip) #{} not selected", i));
            }
        } else {
            if !shift && fresh {
                // Plain click = start a fresh selection (replace).
                self.selection.clear();
                self.selected = None;
            }
            if !self.selection.contains(&i) {
                self.selection.push(i);
                self.history.push(format!("    + #{} selected", i));
            }
        }
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::SelectChange {
                n_before: 0,
                n_after: 0, // stamped by push() from the real vec lens
                basket_before,
                basket_after: self.selection.clone(),
                cause: format!(
                    "click_select(i={}, shift={}, alt={}, fresh={})",
                    i, shift, alt, fresh
                ),
            }
        );
        // Phase-10 (post-creation) instrumentation: record every selection that
        // touches a hatch or a hatch's boundary. Without this the log covered
        // hatch CREATION only, and the most-reported problems — "it selected 2
        // things", "the boundary moved with the fill" — happened afterwards,
        // invisible to the log.
        self.hatch_dbg_selection("click_select", i);
    }

    /// Every VISIBLE dobject within `tol_world` of `w`, nearest first.
    /// Shared by selection cycling (pointer-mode Tab).
    fn pick_candidates_at(&self, w: Vec2, tol_world: f64) -> Vec<usize> {
        let pool: Vec<usize> = if let (Some(g), false) = (self.index.as_ref(), self.index_dirty) {
            g.query_near(w, tol_world)
                .into_iter()
                .map(|u| u as usize)
                .collect()
        } else {
            (0..self.doc.dobjects.len()).collect()
        };
        let mut cands: Vec<(usize, f64)> = Vec::new();
        for i in pool {
            if !self.doc.is_visible(i) {
                continue;
            }
            let d = self.doc.dobjects[i].distance_to_point(w);
            if d < tol_world {
                cands.push((i, d));
            }
        }
        cands.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        cands.into_iter().map(|(i, _)| i).collect()
    }

    /// Pointer-mode Tab — selection cycling through stacked dobjects under
    /// the cursor. A fresh spot selects the top candidate (like a click
    /// would); repeated Tabs at the same spot advance to the next one and
    /// leave `pick_cycle_at` for the next click to honor.
    fn cycle_pick_candidate(&mut self, w: Vec2, cell: (i32, i32), tol_world: f64) {
        let fresh = self.pick_cycle_cell != Some(cell);
        if fresh {
            self.pick_cands = self.pick_candidates_at(w, tol_world);
            self.pick_cycle_index = 0;
            self.pick_cycle_cell = Some(cell);
        }
        if self.pick_cands.len() < 2 {
            // Nothing stacked — Tab behaves like a plain pick of whatever
            // is under the cursor (and stays silent when the spot is empty).
            self.pick_cycle_at = None;
            if let Some(&i) = self.pick_cands.first() {
                self.click_select(i, false, false, true);
            }
            return;
        }
        if !fresh {
            self.pick_cycle_index = (self.pick_cycle_index + 1) % self.pick_cands.len();
        }
        let i = self.pick_cands[self.pick_cycle_index];
        self.pick_cycle_at = Some((self.pick_cands.len(), self.pick_cycle_index, w));
        // Highlight the candidate: it becomes the current selection so the
        // user sees what the next click will grab.
        self.click_select(i, false, false, true);
    }

    /// The candidate the next pointer-mode click should select: the Tab-
    /// cycled stacked dobject when the click lands at the cycle spot and a
    /// deeper candidate was highlighted, else `None` (click normally).
    fn cycled_pick_for(&self, w: Vec2, tol_world: f64) -> Option<usize> {
        match self.pick_cycle_at {
            Some((n, idx, at)) if n > 1 && idx > 0 && (w - at).len() <= tol_world => {
                self.pick_cands.get(idx).copied()
            }
            _ => None,
        }
    }

    /// Pointer-mode clicks (and any fresh select) consume the cycle state.
    fn clear_pick_cycle(&mut self) {
        self.pick_cycle_index = 0;
        self.pick_cycle_cell = None;
        self.pick_cands.clear();
        self.pick_cycle_at = None;
    }

    /// The dobjects whose grips are live for the current selection:
    /// the basket + `selected`, minus locked-layer objects, PLUS the
    /// invisible auxiliary boundary of any selected hatch.
    ///
    /// `Geom::Hatch` has no geometry of its own, so `grip_points()` on it is
    /// empty and a hatch was the one dobject you could select but not
    /// grip-edit. Since a hatch owns an invisible auxiliary boundary,
    /// substituting that boundary's index here gives the fill real,
    /// draggable vertex grips for free: the same target set feeds grip
    /// drawing, hover highlight, and grab-on-click, so one change covers
    /// all three.
    ///
    /// The user's own drawn shape is never pulled in — only `hatch_aux`
    /// boundaries, which exist solely to serve their hatch.
    fn editable_grip_targets(&self) -> Vec<usize> {
        let mut t: Vec<usize> = self.selection.clone();
        if let Some(s) = self.selected {
            t.push(s);
        }
        let hatch_boundaries: Vec<usize> = t
            .iter()
            .filter_map(|&i| match self.doc.dobjects.get(i).map(|d| &d.geom) {
                Some(Geom::Hatch(h)) => Some(h.boundary_handles.clone()),
                _ => None,
            })
            .flatten()
            .filter_map(|bh| self.doc.dobjects.iter().position(|d| d.handle == bh))
            .filter(|&bi| {
                self.doc
                    .dobjects
                    .get(bi)
                    .map(|d| d.style.hatch_aux)
                    .unwrap_or(false)
            })
            .collect();
        t.sort_unstable();
        t.dedup();
        // Drop locked-layer objects.
        t.retain(|i| self.doc.is_selectable(*i));
        // Append the aux boundaries AFTER that filter. `is_selectable` rejects
        // anything invisible, and these are invisible by design — but that
        // check exists to stop a hidden dobject being CLICKED, which is still
        // true here: the user selected the hatch, and we are only borrowing its
        // boundary's grips. The locked-layer half of the rule must still apply,
        // so test that directly.
        for bi in hatch_boundaries {
            if self.doc.layers.selectable(
                self.doc
                    .dobjects
                    .get(bi)
                    .map(|d| d.style.layer)
                    .unwrap_or(0),
            ) && !t.contains(&bi)
            {
                t.push(bi);
            }
        }
        t
    }

    /// Log a command that is about to run against a basket containing a hatch
    /// or a hatch's boundary, naming which is which. Silent otherwise, so the
    /// log stays about hatch.
    fn hatch_dbg_command_on_selection(&mut self, canon: &str, raw: &str) {
        if self.selection.is_empty() {
            return;
        }
        let sel = self.selection.clone();
        let mut hatches = Vec::new();
        let mut boundaries = Vec::new();
        for &i in &sel {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            if matches!(d.geom, Geom::Hatch(_)) {
                hatches.push(i);
                continue;
            }
            let h = d.handle;
            if self.doc.dobjects.iter().any(|x| {
                matches!(&x.geom,
                Geom::Hatch(hh) if hh.boundary_handles.contains(&h))
            }) {
                boundaries.push(i);
            }
        }
        if hatches.is_empty() && boundaries.is_empty() {
            return;
        }
        self.hatch_dbg(format!(
            "--- [10] command '{}' (typed '{}') on a basket of {} — hatches {:?}, \
             hatch-boundaries {:?}; doc has {} dobject(s) before",
            canon,
            raw,
            sel.len(),
            hatches,
            boundaries,
            self.doc.dobjects.len()
        ));
        if !boundaries.is_empty() {
            self.hatch_dbg(format!(
                "      NOTE: {:?} are boundaries a hatch references — an edit here \
                 changes that fill too (this is the bonded case)",
                boundaries
            ));
        }
    }

    /// Log a selection event when it involves a hatch or something a hatch
    /// references. `focus` is the dobject the user just clicked.
    ///
    /// Reports the relationship, which is the thing that is hard to see on
    /// canvas: whether the clicked dobject IS a hatch, or is a boundary some
    /// hatch points at, and which other basket members are tied to it.
    fn hatch_dbg_selection(&mut self, cause: &str, focus: usize) {
        // Cheap pre-check: only log when a hatch is involved at all.
        let any_hatch = self
            .doc
            .dobjects
            .iter()
            .any(|d| matches!(d.geom, Geom::Hatch(_)));
        if !any_hatch {
            return;
        }
        let sel = self.selection.clone();
        let focus_handle = self.doc.dobjects.get(focus).map(|d| d.handle);
        // Which hatches reference the clicked dobject?
        let owners: Vec<usize> = match focus_handle {
            Some(h) => self
                .doc
                .dobjects
                .iter()
                .enumerate()
                .filter(
                    |(_, d)| matches!(&d.geom, Geom::Hatch(hh) if hh.boundary_handles.contains(&h)),
                )
                .map(|(i, _)| i)
                .collect(),
            None => Vec::new(),
        };
        let focus_kind = self
            .doc
            .dobjects
            .get(focus)
            .map(|d| dobject_kind_name(&d.geom))
            .unwrap_or("?");
        let is_hatch = matches!(
            self.doc.dobjects.get(focus).map(|d| &d.geom),
            Some(Geom::Hatch(_))
        );
        let mut line = format!(
            "--- [10] selection ({}) — clicked #{} ({}); basket now {:?}",
            cause, focus, focus_kind, sel
        );
        if is_hatch {
            if let Some(Geom::Hatch(h)) = self.doc.dobjects.get(focus).map(|d| &d.geom) {
                let n = h.boundary_handles.len();
                line.push_str(&format!("; it is a HATCH over {} boundary handle(s)", n));
            }
        }
        if !owners.is_empty() {
            line.push_str(&format!(
                "; this dobject IS a boundary of hatch {:?} — editing it edits that fill",
                owners
            ));
        }
        self.hatch_dbg(line);
        // Spell out every hatch/boundary pair inside the basket, so a
        // "2 dobjects selected but they move together" report is explained by
        // the log rather than needing a screenshot.
        for &i in &sel {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            if let Geom::Hatch(h) = &d.geom {
                let bound: Vec<String> = h
                    .boundary_handles
                    .iter()
                    .map(
                        |bh| match self.doc.dobjects.iter().position(|x| x.handle == *bh) {
                            Some(bi) => {
                                let also = if sel.contains(&bi) {
                                    " ALSO-IN-BASKET"
                                } else {
                                    ""
                                };
                                let vis = self
                                    .doc
                                    .dobjects
                                    .get(bi)
                                    .map(|b| {
                                        if b.style.hatch_aux {
                                            "aux"
                                        } else if b.style.visible {
                                            "user"
                                        } else {
                                            "hidden"
                                        }
                                    })
                                    .unwrap_or("?");
                                format!("#{}({}){}", bi, vis, also)
                            }
                            None => format!("0x{:x}(missing)", bh),
                        },
                    )
                    .collect();
                self.hatch_dbg(format!(
                    "      hatch #{} → boundaries [{}]",
                    i,
                    bound.join(", ")
                ));
            }
        }
    }

    /// Translate every dobject in `self.selection` by `v`. Used by the
    /// `move` command after the user clicks BASE and DESTINATION. Edits the
    /// dobjects in place; invalidates the spatial index and any cached
    /// intersections so the next ∩ query rebuilds. Snapshots the basket into
    /// `selection_prev` and clears the visible selection — the dobjects
    /// revert to their normal colour, but the user can re-grab them with
    /// `before` in the next selection session.
    fn apply_move(&mut self, v: Vec2) {
        if v.len() < EPS {
            return;
        }
        self.snapshot_doc();
        let targets = self.transform_targets_with_hatch_boundaries();
        for &i in &targets {
            if let Some(d) = self.doc.dobjects.get_mut(i) {
                *d = d.translated(v);
            }
        }
        // Only the moved dobjects changed — re-bucket THOSE, not the whole drawing.
        let moved = targets.clone();
        self.index_absorb(&moved);
        // Save for `before`, then clear the live highlight.
        if !self.selection.is_empty() {
            self.selection_prev = self.selection.clone();
        }
        self.selection.clear();
        self.intersections.clear();
        self.index_dirty = true;
        self.touch_view();
    }

    /// A hatch has no geometry of its own — it IS its boundary — so
    /// `Geom::Hatch::{translated,rotated,scaled,mirrored}` are kernel no-ops and
    /// the fill only follows if the boundary transforms. When the selection
    /// contains a hatch, widen the transform target set to ALSO cover the
    /// dobjects its boundary handles reference (matching AutoCAD: a hatch moves
    /// with its boundary). They remain separate dobjects; this only widens WHICH
    /// dobjects the transform touches. Used by every IN-PLACE transform:
    /// `apply_move`, `apply_rotate`, `apply_scale`, and `apply_mirror`'s
    /// `keep_original == false` branch. The COPY branches deliberately do NOT use
    /// it — a hatch copied alone still references the ORIGINAL boundary
    /// (duplicate_dobjects only remaps handles inside the duplicated set).
    fn transform_targets_with_hatch_boundaries(&self) -> Vec<usize> {
        let mut targets: Vec<usize> = self.selection.clone();
        let mut seen: std::collections::HashSet<usize> = self.selection.iter().copied().collect();
        for &i in &self.selection {
            let Some(d) = self.doc.dobjects.get(i) else {
                continue;
            };
            let Geom::Hatch(h) = &d.geom else { continue };
            for handle in &h.boundary_handles {
                // Dangling handles (boundary deleted) are skipped — the hatch
                // shrinks rather than crashing.
                if let Some(idx) = self.doc.index_of_handle(*handle) {
                    if seen.insert(idx) {
                        targets.push(idx);
                    }
                }
            }
        }
        targets
    }

    /// Add every dobject index to the selection. Used by the `all` sub-command.
    fn add_all_to_selection(&mut self) -> usize {
        // Same quadratic as `add_window_selection`, and worse: Select All visits EVERY object, so
        // the scan it used to run per object was over a selection growing to the whole document.
        // A mask built once turns it linear. See the note there on why it is rebuilt rather than
        // maintained.
        let mut in_sel = vec![false; self.doc.dobjects.len()];
        for &s in &self.selection {
            if s < in_sel.len() {
                in_sel[s] = true;
            }
        }
        let mut added = 0usize;
        for i in 0..self.doc.dobjects.len() {
            // "All" means all the objects you are allowed to have. A locked or hidden layer is
            // excluded here rather than filtered afterwards, so the count reported back to the
            // user is the number actually selected.
            if !in_sel[i] && self.doc.is_selectable(i) {
                self.selection.push(i);
                added += 1;
            }
        }
        added
    }

    /// Lock or unlock a layer — and take its objects out of the selection when it locks.
    ///
    /// THE BASKET IS PART OF THE LOCK. Guarding only the ways INTO a selection leaves the obvious
    /// hole open: select an object, lock its layer, then move — the edit runs off a basket
    /// assembled while the lock was not there, and the lock does nothing.
    ///
    /// Unlocking does not restore anything. A selection is a thing the user made; putting objects
    /// back into it because a checkbox changed would be a surprise in the opposite direction.
    pub fn set_layer_locked(&mut self, id: LayerId, locked: bool) {
        let Some(l) = self.doc.layers.get_mut(id) else {
            return;
        };
        if l.locked == locked {
            return;
        }
        l.locked = locked;
        if !locked {
            self.history.push(format!("  layer #{id} unlocked"));
            return;
        }
        let before = self.selection.len();
        // Borrow-free: `is_selectable` reads the doc, `retain` writes the selection.
        let keep: Vec<bool> = (0..self.doc.dobjects.len())
            .map(|i| self.doc.is_selectable(i))
            .collect();
        self.selection
            .retain(|&i| keep.get(i).copied().unwrap_or(false));
        let dropped = before - self.selection.len();
        self.history.push(if dropped > 0 {
            format!("  layer #{id} locked — {dropped} object(s) dropped from the selection")
        } else {
            format!("  layer #{id} locked")
        });
        if dropped > 0 {
            self.selected = self.selection.first().copied();
        }
    }

    /// Close a window-selection rectangle. Direction = mode:
    ///   L→R drag → "inside" window (only dobjects whose bbox is fully in).
    ///   R→L drag → "crossing" window (any overlap counts).
    /// Modifier = sign: `shift` (or the persistent `select_remove_mode`)
    /// makes the window SUBTRACT instead of ADD.
    #[track_caller]
    /// Narrow-phase crossing test: does the REAL geometry of `geom` enter the
    /// window rectangle [rmin,rmax]? Run after the cheap bbox broad-phase so
    /// crossing-selection doesn't false-positive on objects whose bounding box
    /// overlaps the window but whose stroke never crosses it (e.g. a long
    /// diagonal line — the #7 bug). Tests the geometry against the 4 window
    /// edges via the intersection kernel.
    ///
    /// Types `intersect()` can't handle (Point / Hatch / Spline / Text /
    /// Dimension) fall back to TRUE — i.e. keep the prior bbox-overlap result
    /// (the caller only invokes this when the bbox already overlaps), so this
    /// change can never DROP a selection that used to work.
    fn geom_enters_window(geom: &Geom, rmin: Vec2, rmax: Vec2) -> bool {
        match geom {
            Geom::Point(_)
            | Geom::Hatch(_)
            | Geom::Spline(_)
            | Geom::Text(_)
            | Geom::Dimension(_) => true,
            _ => {
                let c = [
                    Vec2::new(rmin.x, rmin.y),
                    Vec2::new(rmax.x, rmin.y),
                    Vec2::new(rmax.x, rmax.y),
                    Vec2::new(rmin.x, rmax.y),
                ];
                for k in 0..4 {
                    let edge = Geom::Line(Line {
                        a: c[k],
                        b: c[(k + 1) % 4],
                    });
                    if !intersect(geom, &edge).is_empty() {
                        return true;
                    }
                }
                false
            }
        }
    }

    fn add_window_selection(&mut self, p1: Vec2, p2: Vec2, shift: bool, alt: bool, fresh: bool) {
        let basket_before = self.selection.clone();
        let bbox_min = Vec2::new(p1.x.min(p2.x), p1.y.min(p2.y));
        let bbox_max = Vec2::new(p1.x.max(p2.x), p1.y.max(p2.y));
        // CAPTURE the armed-window override BEFORE .take() consumes it —
        // the recorder needs to know whether the mode came from a typed
        // override (`w`/`c`) or from direction-default.
        let armed_at_entry = self.armed_window_inside;
        // === Hard rule (feedback_rust_cad_universal_selection_model) ===
        // Typed `w` / `c` ALWAYS beats drag direction. The .take() is
        // critical — the override is consumed by the first completing
        // window so it doesn't carry over to the next gesture. Do NOT
        // reorder this match without re-reading the memo: any future
        // "smart direction detection" must still fall under the
        // Some(_) arms, not over them.
        let crossing = match self.armed_window_inside.take() {
            Some(true) => false, // armed window → inside-only
            Some(false) => true, // armed crossing
            None => p2.x < p1.x, // direction-default (R→L = crossing)
        };
        let want_remove = alt || (self.select_remove_mode && !shift);
        // Plain window-drag = fresh selection (replace) when `fresh`.
        if !want_remove && !shift && fresh {
            self.selection.clear();
            self.selected = None;
        }

        let cands: Vec<usize> = match (self.index.as_ref(), self.index_dirty) {
            (Some(g), false) => g
                .query_bbox(bbox_min, bbox_max)
                .into_iter()
                .map(|u| u as usize)
                .collect(),
            _ => (0..self.doc.dobjects.len()).collect(),
        };
        // O(1) MEMBERSHIP FOR THIS CALL. `self.selection.contains(&i)` is a linear scan of a Vec,
        // and it sat inside the candidate loop — so one crossing window over a big drawing was
        // candidates × selected comparisons. MEASURED 22.4 s of frozen UI at 500k objects, and
        // 830 ms at 100k, which is a size you reach today.
        //
        // REBUILT PER CALL, NOT MAINTAINED. This is the pattern `sel_mask` already proves two
        // thousand lines up: a mask that lives across frames would need invalidating at 43+ sites
        // that mutate `selection`, and a stale one silently selects the wrong objects. Built here,
        // used here, dropped here — nothing to keep in step.
        let mut in_sel = vec![false; self.doc.dobjects.len()];
        for &s in &self.selection {
            if s < in_sel.len() {
                in_sel[s] = true;
            }
        }
        // The REMOVE path was quadratic twice over: `position()` to find the entry, then
        // `Vec::remove` shifting every element after it. Removals are marked here and applied in
        // ONE `retain` after the loop, which also preserves the order of everything that stays.
        let mut drop_from_sel = vec![false; self.doc.dobjects.len()];
        let mut any_dropped = false;
        // SESSION RECORDER — record EVERY candidate's verdict so the
        // user can see exactly which dobjects were considered and why
        // each one was rejected. This is the diagnostic that nails
        // "I dragged crossing but got 0 hits" bugs.
        let cand_count = cands.len();
        let mut verdicts: Vec<String> = Vec::new();

        let mut changed = 0usize;
        for i in &cands {
            let i = *i;
            // A LOCKED OR HIDDEN OBJECT IS NOT IN THE WINDOW. Skipped before any geometry work,
            // so dragging a window across a locked layer costs nothing extra — and skipped on the
            // REMOVE path too, which is right: a subtracting window over a locked object has
            // nothing to subtract, because it could never have been selected.
            if !self.doc.is_selectable(i) {
                continue;
            }
            // BlockRefs resolve their real extent through the block table
            // (the kernel bbox is just the insertion point).
            let (emin, emax) = match &self.doc.dobjects[i].geom {
                Geom::BlockRef(br) => self.resolved_blockref_bbox(br),
                _ => self.doc.dobjects[i].bbox(),
            };
            let bbox_inside = emin.x >= bbox_min.x
                && emax.x <= bbox_max.x
                && emin.y >= bbox_min.y
                && emax.y <= bbox_max.y;
            let inside = if crossing {
                // CROSSING: select if the geometry is fully inside the window
                // (bbox inside ⟹ geom inside) OR its REAL geometry actually
                // enters the window. Bbox-overlap alone over-selects — a long
                // diagonal object has a big bbox that overlaps the window even
                // when its stroke never crosses it (the #7 false-positive).
                let bbox_overlaps = !(emax.x < bbox_min.x
                    || emin.x > bbox_max.x
                    || emax.y < bbox_min.y
                    || emin.y > bbox_max.y);
                bbox_inside
                    || (bbox_overlaps
                        && Self::geom_enters_window(&self.doc.dobjects[i].geom, bbox_min, bbox_max))
            } else {
                // INSIDE (window) mode: bbox fully inside ⟹ geom fully inside.
                bbox_inside
            };
            // Recorder verdict (only push first 20 to bound dump size).
            if verdicts.len() < 20 {
                verdicts.push(format!(
                    "#{}: bbox=({:.2},{:.2})→({:.2},{:.2}) → {}",
                    i,
                    emin.x,
                    emin.y,
                    emax.x,
                    emax.y,
                    if inside { "HIT" } else { "miss" }
                ));
            }
            if !inside {
                continue;
            }
            if want_remove {
                if i < in_sel.len() && in_sel[i] {
                    in_sel[i] = false;
                    drop_from_sel[i] = true;
                    any_dropped = true;
                    changed += 1;
                }
            } else if i >= in_sel.len() || !in_sel[i] {
                if i < in_sel.len() {
                    in_sel[i] = true;
                }
                self.selection.push(i);
                changed += 1;
            }
        }
        // One pass for every removal, instead of one shift per removal.
        if any_dropped {
            self.selection
                .retain(|&x| x >= drop_from_sel.len() || !drop_from_sel[x]);
        }
        self.history.push(format!(
            "    {} {} dobject(s) via {} window (current: {})",
            if want_remove { "−" } else { "+" },
            changed,
            if crossing { "crossing" } else { "inside" },
            self.selection.len(),
        ));
        // FULL DIAGNOSTIC — what the classifier saw, what it picked,
        // why, and what each candidate looked like vs the window.
        let mode_reason = match armed_at_entry {
            Some(true) => "armed_window=inside (typed `w`)".to_string(),
            Some(false) => "armed_window=crossing (typed `c`)".to_string(),
            None => format!(
                "direction-default (p2.x {} p1.x → {})",
                if p2.x < p1.x { "<" } else { ">=" },
                if crossing { "crossing" } else { "inside" }
            ),
        };
        let verdict_dump = if verdicts.len() == cand_count {
            verdicts.join("  ·  ")
        } else {
            format!(
                "{}  · …{} more",
                verdicts.join("  ·  "),
                cand_count - verdicts.len()
            )
        };
        crate::dbg_event!(
            self,
            crate::dbg_recorder::DbgEvent::SelectChange {
                n_before: 0,
                n_after: 0, // stamped by push() from the real vec lens
                basket_before,
                basket_after: self.selection.clone(),
                cause: format!(
                    "add_window_selection  p1=({:.3},{:.3}) p2=({:.3},{:.3})  \
                     window_bbox=({:.3},{:.3})→({:.3},{:.3})  shift={} remove_mode={} \
                     armed_inside_at_entry={:?}  MODE={}  REASON={}  \
                     candidates({} from {})={{ {} }}",
                    p1.x,
                    p1.y,
                    p2.x,
                    p2.y,
                    bbox_min.x,
                    bbox_min.y,
                    bbox_max.x,
                    bbox_max.y,
                    shift,
                    self.select_remove_mode,
                    armed_at_entry,
                    if crossing { "crossing" } else { "inside" },
                    mode_reason,
                    cand_count,
                    if self.index_dirty || self.index.is_none() {
                        "full doc scan"
                    } else {
                        "spatial index"
                    },
                    verdict_dump
                ),
            }
        );
    }
}

impl CadApp {
    /// Item 1 — clear the live status line when no edit phase remains active (e.g. `apply_fillet`
    /// ended its state). Keeps the command area free of stale prompts.
    ///
    /// Split out of the end of the frame so it can be tested: the bug it caused ("is it even
    /// working?") was invisible from anywhere else, because the prompt appeared and was gone again
    /// before the next frame drew.
    fn sweep_stale_prompt(&mut self) {
        let any_edit_active = self.tool != Tool::None
            || self.select_mode != SelectMode::Off
            || matches!(self.trim_state,
                TrimState::SelectingCutters
                | TrimState::PickingTargets(_)
                | TrimState::PickingTargetsAll)
            || matches!(self.extend_state,
                ExtendState::SelectingBoundaries
                | ExtendState::PickingTargets(_)
                | ExtendState::PickingTargetsAll)
            || self.move_state       != MoveState::Off
            || self.copy_state       != CopyState::Off
            || self.paste_state      != PasteState::Off
            || self.pedit_state      != PeditState::Off
            || self.rotate_state     != RotateState::Off
            || self.scale_state      != ScaleState::Off
            || self.mirror_state     != MirrorState::Off
            || self.align_state      != AlignState::Off
            || self.break_state      != BreakState::Off
            || self.lengthen_state   != LengthenState::Off
            || self.offset_state     != OffsetState::Off
            || self.stretch_state    != StretchState::Off
            || self.matchprops_state != MatchPropsState::Off
            || self.fillet_state     != FilletState::Off
            || self.chamfer_state    != ChamferState::Off
            || self.picking_source
            || self.intersect_pending_click
            // THE 3D PLACEMENT PROMPT COUNTS AS AN ACTIVE EDIT.
            //
            // Reported as "the placement is still confusing. is it even working?", with a
            // screenshot showing `command: place` in the history and the IDLE prompt underneath.
            // `place` was working perfectly: it set `Placement [Click/Centre/Origin/Offset]
            // <offset>:` and this sweep wiped it one frame later, because the list above is every
            // 2D edit state and a 3D one had never needed to be in it.
            //
            // Worse than invisible: `place_prompt_open` stayed true with nothing on screen saying
            // so, and the next word typed was eaten as a bad answer to a question the user could
            // not see.
            || self.place_prompt_open
            || self.place_coord_prompt;
        if !any_edit_active && !self.current_prompt.is_empty() {
            self.clear_prompt();
        }
    }
}
// Shared canvas cursors — ONE implementation, used by the 2D canvas AND the
// 3D Factory viewport, so the two views can never drift apart.
//
// The rule (COMMAND_LINE_AND_MOUSE_RULES.md Part B):
//   · drafting / point-pick phase → square (pickbox) + crosshair, and PRESS
//     fires the click (a small drift never becomes a drag)
//   · selection phase            → pickbox + "^" arrow — visually distinct, so
//     "picking objects" never looks like "placing a point"
//   · idle pointer               → the OS arrow (we draw nothing)
// ===================================================================

/// Pickbox half-edge, px. (`PkBxSz` is the user-facing SYSVAR for the 2D pick
/// tolerance; these are the on-screen glyph dimensions.)
const CUR_PICK_HALF: f32 = 7.0;
/// Crosshair arm half-length, px.
const CUR_ARM: f32 = 14.0;
const CUR_COL: egui::Color32 = egui::Color32::from_rgb(235, 235, 245);

/// DRAFTING cursor — square + cross. Drawn iff `in_click_only_phase`; it is the
/// visual cue that press-fires-click is in effect ("I'm in a command, click to
/// place a point"), exactly as AutoCAD does.
pub fn draw_draft_cursor(painter: &egui::Painter, p: egui::Pos2) {
    let stroke = egui::Stroke::new(1.0, CUR_COL);
    let sq = egui::Rect::from_center_size(p, egui::vec2(CUR_PICK_HALF * 2.0, CUR_PICK_HALF * 2.0));
    painter.rect_stroke(sq, 0.0, stroke);
    painter.line_segment(
        [
            egui::pos2(p.x - CUR_ARM, p.y),
            egui::pos2(p.x + CUR_ARM, p.y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(p.x, p.y - CUR_ARM),
            egui::pos2(p.x, p.y + CUR_ARM),
        ],
        stroke,
    );
}

/// SELECTION cursor — pickbox + a "^" arrow pointing up-left out of it.
/// Deliberately NOT a crosshair: selection picks an OBJECT, drafting places a
/// POINT, and the two must be distinguishable at a glance.
pub fn draw_select_cursor(painter: &egui::Painter, p: egui::Pos2) {
    let stroke = egui::Stroke::new(1.0, CUR_COL);
    let sq = egui::Rect::from_center_size(p, egui::vec2(CUR_PICK_HALF * 2.0, CUR_PICK_HALF * 2.0));
    painter.rect_stroke(sq, 0.0, stroke);
    // "^" — an arrowhead above-left of the box, its tip AT the hot spot side so
    // the box still reads as the pick aperture.
    let tip = egui::pos2(p.x - CUR_PICK_HALF - 2.0, p.y - CUR_PICK_HALF - 2.0);
    let len = 9.0_f32;
    painter.line_segment([tip, egui::pos2(tip.x + len, tip.y)], stroke);
    painter.line_segment([tip, egui::pos2(tip.x, tip.y + len)], stroke);
    painter.line_segment(
        [tip, egui::pos2(tip.x + len * 0.75, tip.y + len * 0.75)],
        stroke,
    );
}

/// EVERY SHORTCUT, IN ONE TABLE.
///
/// Asked for as: "in the help menu add hot keys and categorize them by 2d factory and 3d factory
/// so the user could see them."
///
/// One table rather than a hand-written help page, because a help page written beside the code is
/// a help page that drifts from it: the shortcut moves, the page does not, and the user is now
/// being lied to by their own documentation. Everything on screen is generated from here.
pub struct ShortcutRow {
    pub keys: &'static str,
    pub what: &'static str,
}

pub struct ShortcutGroup {
    pub title: &'static str,
    /// When it applies — the gate, stated, because a key that does nothing looks broken.
    pub scope: &'static str,
    pub rows: &'static [ShortcutRow],
}

const fn r(keys: &'static str, what: &'static str) -> ShortcutRow {
    ShortcutRow { keys, what }
}

pub const SHORTCUTS: &[ShortcutGroup] = &[
    ShortcutGroup {
        title: "Everywhere",
        scope: "any view",
        rows: &[
            r(
                "Ctrl + Z",
                "Undo — always the drawing, never the command line's own text",
            ),
            r("Ctrl + Y", "Redo"),
            r("Esc", "Cancel the running command, prompt or pick"),
            r("Delete", "Delete the selection"),
        ],
    },
    ShortcutGroup {
        title: "2D drafting",
        scope: "the 2D canvas",
        rows: &[
            r("F7", "Grid on / off"),
            r(
                "F8",
                "Cardinal lock — constrain to horizontal or vertical from the anchor",
            ),
            r("F9", "Grid snap on / off"),
            r("Ctrl + C / Ctrl + V", "Copy / paste"),
            r("Arrows", "Nudge the selection"),
            r(
                "Type a command",
                "l, c, m, ro, … — the command line is the primary interface",
            ),
        ],
    },
    ShortcutGroup {
        title: "3D Factory",
        scope: "the 3D viewport must be ACTIVE — click in it first. These never fire while a \
                sketch is open, or while anything has keyboard focus, so they cannot take a \
                letter out of the command line.",
        rows: &[
            r("M", "Move gizmo"),
            r("R", "Rotate gizmo"),
            r("Tab", "Cycle the gizmo — move ↔ rotate"),
            r("F", "Frame the model"),
            r("1 / 2 / 3 / 0", "Front / Right / Top / Iso view"),
            r("G", "Grid on / off"),
            r("H", "Hide ceilings"),
            r("X", "X-ray"),
            r("Shift + X", "Cutaway"),
            r("S", "3D snapping on / off"),
            r("[ / ]", "Storey down / up"),
            r("A", "Select all"),
            r("Alt + A", "Deselect all"),
            r("Esc", "Clear the selection"),
            r("Ctrl + D", "Duplicate in place"),
            r("Ctrl + C / Ctrl + V", "Copy / paste"),
            r("Arrows", "Nudge the selection"),
        ],
    },
    ShortcutGroup {
        title: "3D Factory — mouse",
        scope: "the 3D viewport",
        rows: &[
            r(
                "Left drag",
                "Select, or drag a gizmo handle — never the camera",
            ),
            r("Middle drag", "Orbit"),
            r("Alt + Right drag", "Orbit (for mice with no middle button)"),
            r("Shift + Left drag", "Pan"),
            r("Scroll", "Zoom"),
            r("Right click a face", "Sketch on that face"),
        ],
    },
    ShortcutGroup {
        title: "SIMLUX 3D view",
        scope: "the SIMLUX viewport",
        rows: &[
            r("Left drag", "Orbit"),
            r("Shift or Middle drag", "Pan"),
            r("Scroll", "Zoom"),
        ],
    },
];

/// A SKETCH BELONGS TO ITS OWN PLANE.
///
/// Reported as: "i drew a sketch on a wall … see how the same drawing is getting drawn in the
/// ground plane too."
///
/// `factory_enter_sketch` REPLACES `self.doc` with the sketch and parks the real drawing in
/// `session.saved_doc`. Every site that means THE PLAN must therefore say so — the swap makes the
/// wrong document the default, and reading `self.doc` at a plan site lays the sketch's u/v out as
/// world x/y on the ground.
/// A `debug_assert!` MUST NOT BE THE THING THAT DOES THE WORK.
///
/// It expands to `if cfg!(debug_assertions) { assert!(…) }`, and `cfg!` is a compile-time
/// constant — so in a RELEASE build the expression inside is never evaluated at all.
///
/// THIS SHIPPED. `factory_restore_stashed_cutters` wrapped its restore call in one, so the
/// installed binary put NOTHING back when a cutout edit was escaped: the openings stayed deleted,
/// which is precisely the data loss the stash exists to prevent. Every test runs in a debug build,
/// so no amount of testing the behaviour could have found it — only reading the code, or the user.

/// A LOOP INSIDE ANOTHER LOOP IS A HOLE IN IT.
///
/// Every closed loop on a face sketch used to extrude as its own solid, so a plate drawn with four
/// bolt circles came out as five posts and the drafter had to cut the holes back out by hand —
/// four more features, each of which then had to be kept beside the plate for ever.
// A hatch has NO geometry of its own — `Geom::Hatch::{rotated,scaled,mirrored}`
// are kernel no-ops, so the fill only follows if the BOUNDARY transforms. These
// cover the in-place transforms beyond `apply_move`.

/// THE FOLDER BROWSER SHOWS YOU WHAT IS IN THE FOLDER.
///

/// WHAT ONE FRAME OF 3D LINE WORK COSTS, on a real drawing.
///
///   SIMLUX_PLAN="D:\...\1 Mashrabiya.dxf" cargo test -p cad_app --bin simlux \
///       frame_cost_of_real_drawing -- --ignored --nocapture
///
/// The 3D view rebuilds its whole line buffer every frame from FIVE separate builders and then
/// concatenates them. None of it is cached and none of it depends on the camera, so panning a
/// drawing re-flattens every entity in it, sixty times a second.
///
/// The plan's estimate for this was "drawing one object costs 30 ms" and "panning allocates
/// megabytes per frame". M6 is the reason this is measured rather than trusted: the last two
/// estimates in this plan were both wrong, one of them by having been taken against a BSP that
/// was collapsing.
#[test]
#[ignore = "needs SIMLUX_PLAN=<drawing path>"]
fn frame_cost_of_real_drawing() {
    let Ok(path) = std::env::var("SIMLUX_PLAN") else {
        println!("set SIMLUX_PLAN to a .dxf path");
        return;
    };
    let text = std::fs::read_to_string(&path)
        .or_else(|_| std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()))
        .expect("read the drawing");
    let t = std::time::Instant::now();
    let doc = cad_io::dxf::read_dxf(&text).expect("parse dxf");
    let parse_ms = t.elapsed().as_secs_f64() * 1000.0;

    let mut app = CadApp::default();
    app.doc = doc;
    println!(
        "\n{}\n  {} dobjects, {} layers, {} blocks — parsed in {:.0} ms",
        path,
        app.doc.dobjects.len(),
        app.doc.layers.len(),
        app.doc.blocks.len(),
        parse_ms,
    );

    // Each builder, timed on its own, then the concatenation the frame actually performs.
    let bench = |label: &str, f: &dyn Fn() -> usize| {
        // A few passes, because one is dominated by first-touch page faults on the allocator.
        const N: u32 = 5;
        let t = std::time::Instant::now();
        let mut verts = 0;
        for _ in 0..N {
            verts = f();
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / N as f64;
        println!(
            "  {label:<26} {ms:>9.2} ms   {verts:>9} verts   {:>7} KB",
            verts * 40 / 1024
        );
        ms
    };

    println!("\n  per-frame line builders (mean of 5)");
    let mut total = 0.0;
    total += bench("overlay_lines", &|| app.factory.overlay_lines().len());
    total += bench("sketch_lines", &|| app.factory.sketch_lines(&app.doc).len());
    total += bench("picked_face_lines", &|| {
        app.factory.picked_face_lines().len()
    });
    total += bench("live_sketch_lines", &|| {
        app.factory.live_sketch_lines(&app.doc).len()
    });
    total += bench("plan_lines", &|| {
        app.factory.plan_lines(&app.doc, 0.0).len()
    });

    // THE CONCATENATION, which is the part a partial cache would leave behind. This is the exact
    // sequence the frame runs.
    let concat = bench("→ concatenated frame", &|| {
        let mut lines = app.factory.overlay_lines();
        lines.extend(app.factory.sketch_lines(&app.doc));
        lines.extend(app.factory.picked_face_lines());
        lines.extend(app.factory.live_sketch_lines(&app.doc));
        lines.extend(app.factory.plan_lines(&app.doc, 0.0));
        lines.len()
    });

    // WHAT THE DISPLAY FLATTENER ADDED. Block references used to flatten to nothing, which is
    // why an imported plan was very nearly invisible in 3D; expanding them is the fix, and this
    // is its price. Worth knowing separately from the cost of the drawing itself, because it is
    // the difference between "this was always slow" and "we have just made it slow".
    let k = app.doc.units.metres_per_unit;
    let count = |f: &dyn Fn(&Geom) -> Vec<Vec<glam::Vec2>>| -> usize {
        app.doc
            .dobjects
            .iter()
            .map(|d| f(&d.geom).iter().map(|p| p.len()).sum::<usize>())
            .sum()
    };
    let buildable = count(&|g| cad_solid::geom_outlines_scaled(g, k));
    let displayed = count(&|g| cad_solid::geom_display_outlines_scaled(g, &app.doc, k));
    println!(
        "\n  flattened points: {buildable} buildable-only vs {displayed} with blocks/splines/walls \
         expanded ({}x)",
        displayed / buildable.max(1),
    );

    // THE CACHED FRAME — what the app actually runs now. The first call builds; the rest are the
    // steady state, which is what panning and orbiting a still drawing cost.
    let cached = {
        const N: u32 = 20;
        app.refresh_cached_lines();
        let t = std::time::Instant::now();
        let mut n = 0;
        for _ in 0..N {
            app.refresh_cached_lines(); // settled → a signature compare, not a rebuild
            let mut lines = app.factory.overlay_lines();
            lines.extend_from_slice(&app.cached_sketch_lines);
            lines.extend(app.factory.picked_face_lines());
            lines.extend(app.factory.live_sketch_lines(&app.doc));
            lines.extend_from_slice(&app.cached_plan_lines);
            n = lines.len();
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / N as f64;
        println!("  → cached frame (settled)   {ms:>9.2} ms   {n:>9} verts");
        ms
    };

    println!("\n  builders summed        {total:>9.2} ms");
    println!("  whole frame            {concat:>9.2} ms");
    println!(
        "  cached frame           {cached:>9.2} ms  ({:.0}x)",
        concat / cached.max(1e-9)
    );
    println!(
        "  of a 16.7 ms budget:   {:>8.1}% uncached  ->  {:.1}% cached",
        100.0 * concat / 16.7,
        100.0 * cached / 16.7,
    );
    // WHAT IS LEFT IS THE CONCATENATION, which is the thing the plan warned about by name. The
    // cached buffers are copied into one Vec every frame because the renderer takes a single line
    // list; removing that means giving the static lines their own persistent GPU buffer and a
    // second draw call, rather than rebuilding the combined one. Named here so the next person
    // measuring this knows the remaining cost is understood rather than mysterious.
    println!(
        "  remaining {cached:.2} ms is the per-frame memcpy of {} KB of cached lines\n",
        (app.cached_plan_lines.len() + app.cached_sketch_lines.len()) * 40 / 1024,
    );
}

/// ACTIVE-VIEWPORT indicator — drawn ON the viewport, where the eyes already are.
///
/// A 1px border plus a dot in the top-right corner: **yellow = active, gray = idle**.
/// The active viewport is the one that owns the next command (move/copy/rotate/… act on
/// ITS objects), so this must be readable without looking away from what you're drawing —
/// a pill up in the header is too far from the work.
///
/// Only drawn when there is genuinely a choice (both viewports visible). With 2D alone
/// there is nothing to disambiguate and a permanently-yellow frame would be noise.
pub fn draw_viewport_active_frame(painter: &egui::Painter, rect: egui::Rect, active: bool) {
    let col = if active {
        egui::Color32::from_rgb(0xf2, 0xb5, 0x3d) // yellow — this view owns the command
    } else {
        egui::Color32::from_rgb(0x5a, 0x63, 0x6d) // gray — idle
    };
    // shrink by half a pixel so the 1px stroke lands ON the boundary, not straddling it
    painter.rect_stroke(rect.shrink(0.5), 0.0, egui::Stroke::new(1.0, col));
    let c = egui::pos2(rect.right() - 11.0, rect.top() + 11.0);
    painter.circle_filled(c, 4.0, col);
    if active {
        // a soft ring so the live one reads at a glance in peripheral vision
        painter.circle_stroke(c, 6.5, egui::Stroke::new(1.0, col.gamma_multiply(0.5)));
    }
}

/// The 3D-Factory nav-cube gizmo (the "VIEW" icon): a glowing wireframe cube shown in the
/// LIVE camera orientation, inside an orbit ring with a heading dot. Purely painted — the
/// caller senses the drag (horizontal = yaw, vertical = pitch) and orbits the camera.
pub fn draw_nav_cube(
    painter: &egui::Painter,
    rect: egui::Rect,
    yaw: f32,
    pitch: f32,
    highlight: Option<crate::factory::StdView>,
) {
    use glam::Vec3;
    let c = rect.center();
    let radius = (rect.width().min(rect.height()) * 0.5) - 3.0;

    // Camera basis → orthographic projection of the cube as the eye sees it (so the cube
    // turns exactly as you orbit — a true orientation indicator).
    let (cp, sp) = (pitch.cos(), pitch.sin());
    let (cy, sy) = (yaw.cos(), yaw.sin());
    let f = Vec3::new(cp * cy, cp * sy, sp);
    let r = {
        let x = f.cross(Vec3::Z);
        if x.length() < 1e-4 {
            Vec3::X
        } else {
            x.normalize()
        }
    };
    let u = r.cross(f).normalize();

    // Orbit ring + heading dot.
    let ring = egui::Color32::from_rgba_unmultiplied(120, 200, 255, 170);
    painter.circle_stroke(c, radius, egui::Stroke::new(1.6, ring));
    let ha = -yaw - std::f32::consts::FRAC_PI_2;
    let dot = egui::pos2(c.x + radius * ha.cos(), c.y + radius * ha.sin());
    painter.circle_filled(dot, 3.0, egui::Color32::from_rgb(150, 215, 255));

    // Wireframe cube.
    let s = radius * 0.60;
    let corners = [
        Vec3::new(-1., -1., -1.),
        Vec3::new(1., -1., -1.),
        Vec3::new(1., 1., -1.),
        Vec3::new(-1., 1., -1.),
        Vec3::new(-1., -1., 1.),
        Vec3::new(1., -1., 1.),
        Vec3::new(1., 1., 1.),
        Vec3::new(-1., 1., 1.),
    ];
    let p: Vec<(egui::Pos2, f32)> = corners
        .iter()
        .map(|&v| (egui::pos2(c.x + v.dot(r) * s, c.y - v.dot(u) * s), v.dot(f)))
        .collect();
    // Hover highlight — fill the face the cursor is over, so it reads as clickable.
    if let Some(hv) = highlight {
        use crate::factory::StdView as V;
        let fc: &[usize] = match hv {
            V::Top => &[4, 5, 6, 7],
            V::Bottom => &[0, 1, 2, 3],
            V::Front => &[0, 1, 5, 4],
            V::Back => &[3, 2, 6, 7],
            V::Right => &[1, 2, 6, 5],
            V::Left => &[0, 3, 7, 4],
            V::Iso => &[],
        };
        if !fc.is_empty() {
            let poly: Vec<egui::Pos2> = fc.iter().map(|&k| p[k].0).collect();
            painter.add(egui::Shape::convex_polygon(
                poly,
                egui::Color32::from_rgba_unmultiplied(120, 200, 255, 70),
                egui::Stroke::NONE,
            ));
        }
    }

    // Face visibility: a face is front-facing when its outward normal points toward the
    // eye (`normal · f > 0`, `f` = target→eye). An edge is VISIBLE if EITHER of its two
    // faces is front-facing; HIDDEN only when both face away.
    // Face order: 0=-Z 1=+Z 2=-Y 3=+Y 4=-X 5=+X.
    let face_n = [
        Vec3::new(0., 0., -1.),
        Vec3::new(0., 0., 1.),
        Vec3::new(0., -1., 0.),
        Vec3::new(0., 1., 0.),
        Vec3::new(-1., 0., 0.),
        Vec3::new(1., 0., 0.),
    ];
    let front = |k: usize| face_n[k].dot(f) > 0.0;
    // ((corner_i, corner_j), (face_a, face_b))
    let edges: [((usize, usize), (usize, usize)); 12] = [
        ((0, 1), (0, 2)),
        ((1, 2), (0, 5)),
        ((2, 3), (0, 3)),
        ((3, 0), (0, 4)),
        ((4, 5), (1, 2)),
        ((5, 6), (1, 5)),
        ((6, 7), (1, 3)),
        ((7, 4), (1, 4)),
        ((0, 4), (2, 4)),
        ((1, 5), (2, 5)),
        ((2, 6), (3, 5)),
        ((3, 7), (3, 4)),
    ];
    // Draw HIDDEN first (thin gray), so the VISIBLE (thick, bright) sit on top.
    for pass_visible in [false, true] {
        for ((i, j), (fa, fb)) in edges {
            let visible = front(fa) || front(fb);
            if visible != pass_visible {
                continue;
            }
            let (a, _) = p[i];
            let (b, _) = p[j];
            if visible {
                painter.line_segment(
                    [a, b],
                    egui::Stroke::new(4.0, egui::Color32::from_rgba_unmultiplied(60, 140, 230, 90)),
                ); // soft glow
                painter.line_segment(
                    [a, b],
                    egui::Stroke::new(2.2, egui::Color32::from_rgb(150, 215, 255)),
                ); // thick bright core
            } else {
                painter.line_segment(
                    [a, b],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(110, 120, 135)),
                ); // thin gray
            }
        }
    }

    // Face labels — so the cube reads as an orientation, not just a wireframe. Drawn on
    // the FRONT-facing faces only (a label on a back face would show through the cube and
    // read backwards). Normal · f gives how square-on the face is; fade the label as it
    // turns edge-on so it doesn't smear across the silhouette. The label→normal pairing
    // is IDENTICAL to `nav_cube_pick`, so what you read is what you click.
    let labels = [
        (Vec3::new(0., 0., 1.), "TOP"),
        (Vec3::new(0., 0., -1.), "BOT"),
        (Vec3::new(0., -1., 0.), "FRONT"),
        (Vec3::new(0., 1., 0.), "BACK"),
        (Vec3::new(1., 0., 0.), "RIGHT"),
        (Vec3::new(-1., 0., 0.), "LEFT"),
    ];
    for (n, txt) in labels {
        let facing = n.dot(f);
        if facing <= 0.15 {
            continue; // edge-on or hidden
        }
        // Push the label slightly off the cube face toward the eye so it sits ON the face
        // rather than being swallowed by the bright edges.
        let ctr = egui::pos2(c.x + n.dot(r) * s, c.y - n.dot(u) * s);
        let alpha = (facing.powf(0.5) * 255.0).clamp(70.0, 255.0) as u8;
        painter.text(
            ctr,
            egui::Align2::CENTER_CENTER,
            txt,
            egui::FontId::proportional(9.0),
            egui::Color32::from_rgba_unmultiplied(235, 245, 255, alpha),
        );
    }
}

/// Which standard view a point over the nav cube lands on — the nearest FRONT-facing face
/// whose projected centre the point is near, or `None` if it misses the faces (ring / gap).
/// Used for both the click (snap) and the hover highlight, and mirrors `draw_nav_cube`'s
/// projection so the pick lines up with what's drawn.
pub fn nav_cube_pick(
    rect: egui::Rect,
    yaw: f32,
    pitch: f32,
    at: egui::Pos2,
) -> Option<crate::factory::StdView> {
    use crate::factory::StdView;
    use glam::Vec3;
    let c = rect.center();
    let radius = (rect.width().min(rect.height()) * 0.5) - 3.0;
    let (cp, sp) = (pitch.cos(), pitch.sin());
    let (cy, sy) = (yaw.cos(), yaw.sin());
    let f = Vec3::new(cp * cy, cp * sy, sp);
    let r = {
        let x = f.cross(Vec3::Z);
        if x.length() < 1e-4 {
            Vec3::X
        } else {
            x.normalize()
        }
    };
    let u = r.cross(f).normalize();
    let s = radius * 0.60;
    // (face outward normal, the view that looks straight at that face)
    let faces = [
        (Vec3::new(0., 0., 1.), StdView::Top),
        (Vec3::new(0., 0., -1.), StdView::Bottom),
        (Vec3::new(0., -1., 0.), StdView::Front),
        (Vec3::new(0., 1., 0.), StdView::Back),
        (Vec3::new(1., 0., 0.), StdView::Right),
        (Vec3::new(-1., 0., 0.), StdView::Left),
    ];
    let mut best: Option<(f32, StdView)> = None;
    for (n, v) in faces {
        if n.dot(f) <= 0.05 {
            continue;
        } // back-facing → not pickable
        let ctr = egui::pos2(c.x + n.dot(r) * s, c.y - n.dot(u) * s);
        let d = (at - ctr).length();
        if d <= s * 0.9 && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, v));
        }
    }
    best.map(|(_, v)| v)
}
